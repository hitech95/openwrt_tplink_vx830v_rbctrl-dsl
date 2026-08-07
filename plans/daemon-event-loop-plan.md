# Plan — Non-blocking daemon main loop

> Resolves the **"OPEN ARCHITECTURE POINT — single-threaded poll loop"**
> documented inline in `rbctl-feed/rbctl-dsl/crates/rbctl_dsl/src/daemon.rs`
> (just above the `while !SHOULD_EXIT` loop, ~line 351).

## 1. Problem

`daemon::run()` is a single, fully sequential loop:

```
ipc.accept_one()          ── non-blocking (UnixListener, accept_one)
SHOULD_RELOAD handling    ── BLOCKING: board.line_config_down() + line_config_up()
SHOULD_RESTART_LINE       ── BLOCKING: down() + sleep(1s) + up()
board.get_line_obj()      ── BLOCKING: up to timeout(2s) × (retries(3)+1) ≈ 8 s
ubus.poll_one()           ── non-blocking, but transport::wait_recv sleeps 1 ms
thread::sleep(1 s)
```

Every `Board` operation goes through `Board::request()`, which is a blocking
send + recv-with-`SO_RCVTIMEO` + retransmit loop. When the board is silent,
one loop iteration can take **~8 s** (poll) or **~17 s** (restart-line). During
that window:

- `rbctl-dsl status` / `stop` (IPC `UnixListener`) are **not served**.
- `ubus call dsl metrics` is **not served** (ubus stream is non-blocking but
  nothing polls it while the board call owns the thread).
- `transport::wait_recv` only works when *it* is on the CPU, so ubus handshakes
  are starved for the same window.

**Goal:** IPC and ubus stay responsive (target p50 < 50 ms) regardless of board
behaviour, including total board silence and slow line (re)config.

**Non-goals:** changing the wire protocol, the on-disk UCI schema, or the
ubus object surface; reducing the board's own retry budget (a slow board can
legitimately take seconds to train).

---

## 2. Current ownership (what moves where)

| Concern | Today | After |
|---|---|---|
| `Board` (socket + opcodes) | main loop | **board worker thread** |
| `DslConfig` (`cfg`) | main loop, mutated in-place | worker owns it; display fields mirrored into `SharedState` |
| `SharedState` (`Arc<Mutex<DslState>>`) | shared already | shared (worker writes, main/ubus read) |
| `IpcListener` | main loop | main loop (unchanged) |
| ubus `UbusConnection` | main loop | main loop (unchanged) |
| signal atomics (`SHOULD_EXIT/RELOAD/RESTART_LINE`) | static | static — **both** threads read them |
| notify-script emission | main loop | worker (it observes board transitions) |

---

## 3. Options considered

| # | Approach | Pro | Con | Verdict |
|---|---|---|---|---|
| **A** | **Dedicated board worker thread** (blocking `Board` unchanged; main thread runs IPC + ubus only) | Smallest change; `board.rs` untouched; directly fixes the stall; matches the inline note already in the code | 2 threads; shutdown needs a bounded join; worker still blocks *itself* during a long `line_config_up` (acceptable — IPC/ubus unaffected) | **Recommended (Phase 1)** |
| B | Single-thread `poll(2)` event loop; `Board::request` → non-blocking state machine driven by fd readability + deadlines | Truly unified, one thread, no locks, cheapest CPU | Large rewrite of `board.rs` (Idle→Sent→AwaitResp(deadline)→Retransmit→Done externalised across poll wakeups); retransmit/seq logic spread across iterations; higher risk | Optional Phase 2 |
| C | Async runtime (tokio / smol / mio) | Ergonomic, mature | Heaviest dependency; violates the minimal-deps / `opt-level="z"`+`lto`+`panic="abort"` ethos; overkill for 3 fds | Rejected (mio == Option B with a lib) |

**Why A first:** the existing `Board` is a clean *blocking* abstraction with a
solid mock-backed test suite. Isolating it on a thread preserves that and
delivers the responsiveness goal with the least risk. Option B remains a valid
future consolidation if a single-thread model is ever required.

---

## 4. Recommended design — board worker thread

### 4.1 Threads

```
main thread (control)                 board worker thread
─────────────────────                 ────────────────────
loop {                                loop {
  ipc.accept_one()  → set atomics       check SHOULD_RELOAD  → re-UCI + down/up
  ubus.poll_one()                      check SHOULD_RESTART  → down/sleep/up
  check SHOULD_EXIT                    poll board.get_line_obj()
  sleep(tick ~50 ms)                   write SharedState; emit hotplug
}                                      sleep(poll_interval ~1 s)
                                       check SHOULD_EXIT
}                                    }
                                     on exit: line_config_down(); exit
```

- The **main tick drops from 1 s to ~50 ms** so IPC/ubus latency is bounded by
  the tick, not by the board.
- The worker keeps its own ~1 s board-poll cadence.
- Commands flow through the **existing static atomics** — no new channel needed.
  IPC handlers and the signal handler already set them; the worker simply also
  reads them.

### 4.2 New module: `crates/rbctl_dsl/src/board_worker.rs`

```rust
pub struct BoardWorker {
    board: Board,                          // owned exclusively by the worker
    cfg: DslConfig,                        // authoritative config
    overrides: CliOverrides,               // for reload (Clone, passed in)
    state: SharedState,                    // Arc<Mutex<DslState>>
    notify_script: Option<String>,         // owned
    // transition bookkeeping (was on the main loop's stack):
    last_status: Option<LinkStatus>,
    up_since: Option<Instant>,
    tc_emitted: bool,
}

impl BoardWorker {
    pub fn spawn(self) -> JoinHandle<()>;  // moves self into the thread
    fn run(&mut self);                      // the worker loop above
}
```

`daemon::run()` constructs it from the objects it already creates (board socket,
loaded cfg, shared state, notify path, overrides), calls `.spawn()`, then enters
a slim main loop.

### 4.3 Shared state additions

`ubus_obj::DslState` already carries `line_obj`, `uptime_secs`, `xfer_mode`.
Add the small set the IPC snapshot needs so it never reads `cfg` directly:

```rust
pub struct DslState {
    pub line_obj: Option<LineObj>,
    pub uptime_secs: u64,
    pub xfer_mode: Option<XferMode>,
    // new: config-derived display fields, refreshed by the worker on reload
    pub modulation: Modulation,
    pub annex: Annex,
    pub last_board_error: Option<&'static str>, // “timeout” / “unreachable”
}
```

`build_snapshot()` (IPC `status`) reads only `SharedState` — no board call, no
`cfg` borrow → constant time, no stall.

### 4.4 Shutdown (bounded join)

The worker owns the board, so it must run `line_config_down()` on exit. To avoid
a wedged socket hanging shutdown:

```rust
let worker = worker.spawn();
// main exits loop on SHOULD_EXIT:
let done = Arc::new(AtomicBool::new(false));
let done2 = done.clone();
let join = thread::spawn(move || { worker.join().ok(); done2.store(true, SeqCst); });
thread::sleep(SHUTDOWN_DEADLINE);           // e.g. 3 s
if !done.load(SeqCst) {
    log::warn!("board worker didn't finish in {SHUTDOWN_DEADLINE:?}, abandoning");
    // process exits; kernel closes the socket. Documented limitation.
}
```

(Process termination is the real backstop; a half-open board socket at exit is
harmless — the board times out its side and re-syncs on next start.)

### 4.5 `transport::wait_recv`

No longer on the critical path (board no longer blocks the main thread). The
1 ms sleep is acceptable inside ubus handshakes; leave it. Optional polish:
replace the sleep with a `poll()` on the ubus fd for a tight single-thread
event source on the main loop — tracked as a follow-up, not required for the
responsiveness goal.

---

## 5. Migration (phased)

### Phase 1 — board worker (this plan)
1. Add fields to `DslState` (§4.3); update `build_metrics_reply`/`build_snapshot`
   to read them instead of `cfg`.
2. Create `board_worker.rs` (`BoardWorker` + `spawn` + `run`); move the board
   poll, reload/restart, transition/hotplug, and `up_since`/`tc_emitted`
   logic verbatim off the main loop into `BoardWorker::run`.
3. Slim `daemon::run()`’s loop to: `ipc.accept_one` → `ubus.poll_one` →
   `SHOULD_EXIT` check → `sleep(50 ms)`. Construct + spawn the worker before it.
4. Bounded shutdown join (§4.4); worker owns `line_config_down` at exit.
5. Replace the inline **OPEN ARCHITECTURE POINT** comment with a one-line
   cross-link to this plan + a note that it is implemented.
6. Keep `board.rs` **unchanged**.

### Phase 2 — optional event-loop unification (deferred)
Make the board socket non-blocking, turn `Board::request` into a
deadline-driven state machine, and drive IPC + ubus + board fds from one
`poll(2)` loop on the main thread (Option B). Only justified if a second
thread is ever undesirable.

---

## 6. Testing

| Layer | Test |
|---|---|
| `board.rs` (unchanged) | existing mock suite stays green |
| `board_worker.rs` (new) | inject a `MockTransport` that delays/silent; assert `SharedState` updates, hotplug transitions, and that a `Reload` command re-applies config |
| Responsiveness regression | mock board that never responds; assert an `ipc.send_command(Status)` round-trip completes in < 100 ms while the worker is “stuck” (proves decoupling) |
| Shutdown | mock board whose recv blocks; assert main exits within `SHUTDOWN_DEADLINE + ε` |
| On-target | disconnect board → `rbctl-dsl status` returns instantly; `reload`/`restart-line` while board down → IPC still answers; `stop` latency under load |

Mock transport reuse: `Board<MockTransport>` is already generic over
`Transport`; the worker can be made generic over `T: Transport` for the same
host-runnable tests, with `T = af_packet::RawSocket` only in production.

---

## 7. Risks & mitigations

| Risk | Mitigation |
|---|---|
| Lock contention on `SharedState` between worker (write) and ubus/IPC (read) | Critical section is a struct swap (~tens of bytes); hold time µs-scale. `try_lock` fallback in ubus handler if ever needed. |
| Mutex poisoning | `panic = "abort"` (release) → poisoning is moot (process dies). `unwrap()` on lock stays acceptable, matching current code. |
| `cfg`/`overrides`/`notify_script` ownership across threads | `CliOverrides: Clone`; pass owned copies into the worker; `notify_script` becomes `Option<String>` (owned). |
| Worker wedged on `line_config_up` at shutdown | Bounded join (§4.4) + process-exit backstop; documented. |
| Two threads logging | `log` + syslog are thread-safe; no change. |
| Reload re-reads UCI from the worker thread | `rust_uci::Uci` is not `Send`/`Sync` → construct a fresh `Uci` inside the worker per reload (already how `DslConfig::load` works). |

---

## 8. Acceptance criteria

- `rbctl-dsl status` answers in **< 100 ms** p99 with the board physically silent.
- `ubus call dsl metrics` answers in **< 100 ms** p99 with the board silent.
- `reload` / `restart-line` no longer delay IPC/ubus answers.
- `stop` exits within **`SHUTDOWN_DEADLINE`** (default 3 s) even with a wedged board.
- All existing `cargo test` for `rbctl_proto` / `af_packet` / `board` stay green;
  new `board_worker` tests pass.
- SDK cross-build (`make package/rbctl-dsl/compile`) produces an apk of
  comparable size (thread + mpsc add negligible bytes under `opt-level="z"`+lto).
