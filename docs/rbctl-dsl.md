# rbctl-dsl — the Rust daemon

This is the **onboarding doc** for `rbctl-dsl`, the Rust daemon that replaces
the proprietary `remote_board` + `libcmm.so` stack for managing an EcoNet xDSL
board on mainline OpenWrt. If you're new to the project, read this first — it
explains *what the binary does at runtime*, *how the crates fit together*, and
*where the unsafe lives*.

> **Sibling docs.** The reverse-engineering side of the house (the original
> `remote_board` ELF, the `0x88B5`/`0x88B6` wire formats, the cmm bus) is in
> [index.md](index.md). This file covers only the **replacement Rust binary**
> living under [`rbctl-feed/rbctl-dsl/`](../rbctl-feed/rbctl-dsl/). For the
> safety posture (every `unsafe` site, drop guarantees, deferred items) see
> [safety-audit.md](safety-audit.md).

---

## TL;DR

`rbctl-dsl` is a single OpenWrt daemon binary that:

1. Reads DSL config from UCI (`/etc/config/network`).
2. Opens a raw `AF_PACKET` socket on the management VLAN (default `lan0.500`).
3. Speaks the proprietary `0x88B5` protocol to the EcoNet board — brings the
   line up, queues the configured data links while the line is down (the
   board materializes them at showtime — the OEM flow), and polls status
   every second.
4. Creates the board-assigned transport VLAN on the host (via rtnetlink).
5. Publishes live metrics on the ubus `dsl` object (LuCI-compatible) and emits
   hotplug events on line-state transitions.
6. Accepts control commands (`status` / `reload` / `restart-line` / `stop`)
   over a Unix socket so a second invocation can drive the running daemon.

It is the clean-room reimplementation of the binary analysed in
[architecture.md](architecture.md).

---

## The two-plane picture

The daemon sits in the **control plane** only. User traffic never passes
through it.

```mermaid
flowchart LR
    subgraph Host["HOST  (MediaTek MT7986 / AArch64, OpenWrt Linux)"]
        direction TB
        UCI["UCI config<br/>/etc/config/network"]
        DAEMON["rbctl-dsl<br/>(this daemon)"]
        UBUS["ubusd<br/>+ LuCI"]
        HK["hotplug<br/>led_dsl.sh / pppoa.sh"]
        VLAN["lan0.&lt;vlan&gt;<br/>(transport interface)"]
        UCI --> DAEMON
        DAEMON <-->|"ubus 'dsl' object"| UBUS
        DAEMON -->|"fork + env"| HK
        DAEMON -->|"rtnetlink RTM_NEWLINK"| VLAN
    end
    subgraph Board["REMOTE BOARD  (EcoNet MIPS SoC)"]
        PHY["DSL chipset<br/>L1 + ATM/PTM decap<br/>speaks 0x88B5"]
    end
    DAEMON <-->|"0x88B5 mgmt frames<br/>(control plane)"| PHY
    PHY ==>|"decapped Ethernet<br/>(DATA plane)"| VLAN
    style Host fill:#1a1a2e,color:#fff
    style Board fill:#0f3460,color:#fff
    style DAEMON fill:#3a1a5a,color:#fff
```

The double arrow on `0x88B5` is the only path `rbctl-dsl` owns end-to-end;
everything else (UCI read, ubus publish, hotplug fork, VLAN create) is a host
side-effect of what it learns over that socket.

---

## Workspace layout

Five crates, one workspace. The dependency graph is strictly layered: the
daemon binary depends on four library crates, two of which are pure Rust and
two of which carry the only `unsafe` in the tree.

```mermaid
flowchart TD
    MAIN["rbctl_dsl<br/>(daemon binary)<br/>src/main.rs + 11 modules"]
    PROTO["rbctl_proto<br/>protocol core<br/>(pure Rust, no_std)"]
    AF["af_packet<br/>AF_PACKET socket<br/>(libc only)"]
    TINY["tinyln_rs<br/>safe libnl-tiny wrapper"]
    SYS["tinyln_rs_sys<br/>bindgen FFI<br/>(unsafe boundary)"]
    UCI["rust-uci<br/>(external git dep)"]
    UBUS["ubus-zero<br/>(external git dep)"]

    MAIN --> PROTO
    MAIN --> AF
    MAIN --> TINY
    MAIN --> UCI
    MAIN --> UBUS
    TINY --> SYS
    AF --> LIBC["libc"]
    SYS --> LIBC
    SYS -.->|"bindgen at build time"| NL["libnl-tiny (C, staged by SDK)"]
    UCI -.->|"libuci-sys bindgen"| LIBUCI["libuci (C, staged by SDK)"]

    style MAIN fill:#3a1a5a,color:#fff
    style PROTO fill:#1b4332,color:#fff
    style AF fill:#1b4332,color:#fff
    style TINY fill:#1b4332,color:#fff
    style SYS fill:#7a1a1a,color:#fff
```

Green = pure safe Rust, red = the FFI boundary, purple = the binary. The
safety story falls directly out of this shape: the entire business logic
(`rbctl_proto` + the non-FFI half of `rbctl_dsl`) is `unsafe`-free; see
[safety-audit.md](safety-audit.md).

---

## Crates

### `rbctl_proto` — protocol core  *(pure Rust, `no_std`)*

Zero C dependencies, builds on any host without the SDK. The `0x88B5` wire
format lives here. 18 unit tests, no `unsafe`.

| Module | Role | RE source |
|---|---|---|
| `checksum.rs` | CRC-16/ARC set / verify (poly `0x8005`, refin=refout=true) | [checksum.md](checksum.md), `examples/checksum.py` |
| `frame.rs` | 24-byte `proto_frame_hdr` builder, `Frame` parser, `SeqCounter` | [protocol.md](protocol.md) |
| `pack.rs` | TX payload encoders — opcodes 1 / 5 / 15 / 6 / 16 | `examples/pack.py`, [xdsl/payloads.md](xdsl/payloads.md) |
| `unpack.rs` | RX payload decoders — opcodes 2 (line obj) / 4 (channel stats) | `examples/unpack.py`, [xdsl/responses.md](xdsl/responses.md) |
| `validate.rs` | Config guard — modulation × annex × profile × xfer_mode | — |

Wire constants it exports: `ETHTYPE_BOARD = 0x88B5`, `MAGIC_COMMAND = 0x11`,
`MAGIC_RESPONSE = 0x10`, `HEADER_LEN = 24`, `MIN_FRAME = 60`.

**Dependencies:** `crc = "3"` (no_std CRC-16/ARC table generator). Nothing
else — not even `libc`.

### `af_packet` — raw Ethernet socket  *(libc-only)*

The thinnest possible layer over `socket(AF_PACKET, SOCK_RAW, ...)`. 10 unit
tests, 10 `unsafe` blocks (all `libc` calls).

- Binds to a named interface (`lan0.500`)
- Installs a classic BPF filter that matches ethertype `0x88B5`
- TX / RX with a configurable `SO_RCVTIMEO` timeout
- Reads the local MAC from `/sys/class/net/<iface>/address`
- RAII `OwnedFd` — closes on drop, no double-free possible

**Dependencies:** `libc = "0.2"` only.

### `tinyln-rs` / `tinyln-rs-sys` — libnl-tiny wrapper

The host-side VLAN management path. `tinyln_rs_sys` is a `bindgen`-generated
FFI layer against OpenWrt's `libnl-tiny` C library and the kernel rtnetlink
UAPI headers; `tinyln_rs` is the hand-written safe wrapper on top.

The safe crate exposes 9 modules:

| Module | Purpose |
|---|---|
| `socket.rs` | `NlSocket` — alloc / connect / send / recv / ACK |
| `msg.rs` | `NlMsg` — message builder, `append_struct<T: Copy>` |
| `attr.rs` | `NlAttr` — typed get/put, nesting, bounds-checked reads |
| `cb.rs` | `NlCb` — callback dispatch |
| `unl.rs` | `Unl` — high-level micro-netlink helpers |
| `rtnl/link.rs` | `RtnlLink` — VLAN create / delete / up / down |
| `rtnl/addr.rs` | `RtnlAddr` — IP address management |
| `rtnl/route.rs` | `RtnlRoute` — route management |
| `genl.rs` | `GenlSocket` — generic netlink, family lookup |

This is where 86 of the workspace's `unsafe` blocks live — all reviewed in
[safety-audit.md](safety-audit.md) §"Hardened / Accepted / Deferred".

> **Note:** the netlink path is only exercised by the daemon's VLAN
> create/delete on startup/shutdown and by `--selftest`. The board protocol
> itself never touches netlink.

**Dependencies:** `libc`, `tinyln-rs-sys` (path). Build-dep: `bindgen = "0.72"`.

### `rbctl_dsl` — the daemon binary

This is what ships in the `.apk`. One binary target (`rbctl-dsl`), organized
as daemon-centric subsystems — directories are runtime subsystems, single
files stay flat, `utils/` holds only dependency-light generic helpers:

| Module | Role |
|---|---|
| `main.rs` | `clap` subcommand dispatch (`daemon` / `status` / `reload` / `restart-line` / `stop` / `selftest` / `sniff`) |
| `cli.rs` | IPC client half — `send_command` / `send_firmware` (speaks the daemon's protocol) |
| `config.rs` | UCI config loader → typed `DslConfig`; `atm_params()` op5 wire builder |
| `logging.rs` | stderr / syslog init |
| `constants.rs` | shared constants (prefixed groups: `BOARD_`, `POLL_`, `VLAN_`, `IPC_`, …) |
| `board.rs` | `Board<T: Transport>` — typed methods for all 8 opcodes, seq, retries, checksum verify |
| `daemon/mod.rs` | **The gate**: `Daemon` struct, control loop, signal flags, all board-operation decisions |
| `daemon/init.rs` | Startup phase: config load + validation, mgmt-link probe, board socket |
| `daemon/worker.rs` | `BoardWorker` — op2 poll + `WorkerCmd` executor (the daemon's agent, no decisions) |
| `daemon/state.rs` | `DslState` / `SharedState` / `FwStatus` — worker↔daemon blackboard |
| `daemon/ubus_object.rs` | ubus `dsl` object (`metrics` / `statistics` methods, Lantiq ABI) |
| `daemon/snapshot.rs` | `StatusSnapshot` builder for IPC `STATUS` |
| `daemon/ipc.rs` | IPC server + wire protocol (`IpcListener`, `IpcCmd`, `StatusSnapshot`) |
| `daemon/hotplug.rs` | Forks the DSL notify script on line-state transitions |
| `daemon/firmware.rs` | `FIRMWARE <path>` IPC streaming handler |
| `diag/selftest.rs` | `--selftest` (socket + VLAN + board probe, auto-modulation sequence) |
| `diag/sniff.rs` | `--sniff` passive 0x88B5/0x88B6 listener + hexdump |
| `diag/pcap.rs` | pcap writer for `--sniff --dump` |
| `utils/mapping.rs` | protocol enums → LuCI/IPC strings, `lstate` ABI consts |
| `utils/vlan.rs` | transport VLAN netdev create/delete/exists |
| `utils/mgmt_link.rs` | read-only sysfs management-interface probe (`MgmtLinkStatus`) |
| `utils/ubus.rs` | ubus socket discovery + connect/register |
| `utils/ubus_transport.rs` | non-blocking AF_UNIX adapter for ubus-zero |

Layering (acyclic): `main → daemon/cli/diag → board/config/utils → external
crates`. The daemon consumes `WorkerEvent`s and issues `WorkerCmd`s — every
board-operation decision (monitor mode, retries, auto provisioning,
teardown) lives in `daemon/mod.rs`.

**Dependencies:** `rust-uci` (git, for UCI), `ubus-zero` (git, pure-Rust ubus
client+server), `af_packet` + `tinyln-rs` + `rbctl_proto` (workspace path
deps), `libc`, `log`, `nanologger`, `clap`.

---

## Workspace-level dependencies

Pinned once in the root `Cargo.toml` under `[workspace.dependencies]`:

| Crate | Version / pin | Why pinned this way |
|---|---|---|
| `rust-uci` | `namib-project` git rev `3e7b6153` | Pulls `libuci-sys 1.1.0` (bindgen 0.72) which fixes a clang ≥ 15 panic on `uci.h`'s anonymous union. The crates.io version still ships the broken 1.0.5. |
| `ubus` (`ubus-zero`) | `pawelchcki/ubus-zero` git rev `68b55de7` | Pure-Rust, server-capable ubus client — links no `.so`. |
| `libc` | `0.2` | Standard. |
| `log` | `0.4` (+`std`) | Facade used everywhere. |
| `nanologger` | `0.1` (+`log`) | Tiny backend; pairs with `log`. |
| `clap` | `4`, minimal features (`std`, `derive`, `help`, `usage`, `error-context`, `suggestions`) | Default features disabled to keep the binary small (`opt-level = "z"` + LTO). |

Release profile is tuned for size: `opt-level = "z"`, `lto = true`,
`codegen-units = 1`, `panic = "abort"`, `strip = true`. The `panic = "abort"`
choice is also a **safety property** — it eliminates unwind-through-FFI UB;
see [safety-audit.md](safety-audit.md) §"Accepted".

The build produces a ~357 KB dynamically-linked aarch64-musl ELF that links
against `libuci`, `libnl-tiny`, and `libc` (all staged by the OpenWrt SDK).

---

## Daemon lifecycle

### Startup sequence

`daemon::run()` → `Daemon::startup()` (`daemon/init.rs`) is deliberately
ordered so each step can fail fast and leave the system in a sane state.

```mermaid
flowchart TD
    A["0. version banner<br/>rbctl-dsl <ver> (git <sha>, built <date>)"]
    A1["1. install signal handlers<br/>SIGTERM/SIGINT/SIGHUP → AtomicBool"]
    B["2. DslConfig::load<br/>UCI + CLI overrides<br/>(bad explicit value → fatal)"]
    C["3. validate_line_config + validate_xfer_mode"]
    D["4. mgmt-link probe<br/>(carrier check, read-only)"]
    E["5. RawSocket::open(iface, 0x88B5)<br/>BPF filter installed"]
    F["6. IpcListener::bind<br/>/var/run/rbctl-dsl.sock"]
    G["7. connect_ubus + register 'dsl'<br/>(best-effort)"]
    H["8. spawn BoardWorker thread"]
    I["9. queue CleanupLink×N + OpenLine<br/>+ ProvisionLink×N (queue-at-open)<br/>(monitor mode: queue nothing)"]
    J["10. control loop (50 ms tick)<br/>events + IPC + ubus + watchdog"]

    A --> A1 --> B --> C --> D --> E --> F --> G --> H --> I --> J

    style A fill:#5a1a1a,color:#fff
    style E fill:#1b4332,color:#fff
    style I fill:#3a1a5a,color:#fff
    style J fill:#3a1a5a,color:#fff
```

Steps 1–5 must succeed or the process exits non-zero before touching the
board. All board operations happen **after** the worker is spawned — the
daemon only sends `WorkerCmd`s and consumes `WorkerEvent`s.

### Line bring-up (the important part)

Two board behaviors shape the whole sequence (both verified against the
board binary and field pcaps, 2026-08-22):

1. The board's `dslCfgSet` (op1 handler) keeps a **config cache**
   (`l_lastDslCfg`) and early-returns when the incoming config is
   identical — no chipset writes, **no `wan adsl open`**. The cache is
   unreadable over the wire.
2. **op3 is `wan adsl close`** — a deliberate line-down, *not* a retrain.
   The only host-reachable way to start training is a **changed-config
   op1**, whose finalize step writes `wan adsl open`.

A daemon restart therefore cannot rely on a single op1: the previous
instance's shutdown op3 closed the line, and an identical op1 is
deduplicated. (`ApplyLineConfig` — a single op1 — remains the command
for reloads where the config *changed*: it differs from the cache by
construction and executes the full path on its own.) The startup `OpenLine` command uses a **guaranteed-differing double apply** —
two op1s differing in exactly the bitswap byte. Whatever the cache holds
before, the second apply always differs from the post-first-apply cache
and therefore always executes the full path ending in `wan adsl open`:

| cache before | 1st apply (bitswap flipped) | 2nd apply (intended) |
|---|---|---|
| == intended | dedup (no-op) | **full apply + open** |
| == flipped | dedup (no-op) | **full apply + open** |
| anything else | full apply (cache := flipped) | **full apply + open** |

Bitswap is an OLR knob — flipping it never changes which standard or
annex is negotiated, so even a fully-executed first apply is harmless.

```mermaid
sequenceDiagram
    participant D as Daemon (gate)
    participant W as BoardWorker
    participant B as Board firmware
    participant T as Board status thread (2 s poll)

    Note over D,W: startup queues [CleanupLink×N, OpenLine, ProvisionLink×N]
    D->>W: WorkerCmd::CleanupLink (per configured vlan)
    W->>B: op16 / op6 (stale-link cleanup, quiet)

    D->>W: WorkerCmd::OpenLine
    W->>B: op1 line config (bitswap flipped)
    Note right of B: identical to cache → dedup<br/>different → apply + wan adsl open
    B-->>W: ack (status 0)
    W->>B: op1 line config (intended)
    Note right of B: differs from cache by construction<br/>→ full apply + wan adsl open
    B-->>W: ack (status 0)
    W-->>D: WorkerEvent::LineConfigApplied

    D->>W: WorkerCmd::ProvisionLink ×N (queue-at-open)
    Note over D: ALL configured links, line still down
    W->>B: op15 / op5 link add (line still training)
    Note right of B: not UP → enqueue in g_interfaceList<br/>(ack now, apply at showtime)
    B-->>W: 4-byte status ack
    Note over W,B: status decoded (decompiled handlers):<br/>0 = applied now · 1 = queued/duplicate<br/>(accepted — materializes at showtime) ·<br/>2 = rejected (vlan out of range) → Err,<br/>daemon does not create the host VLAN
    W-->>D: WorkerEvent::LinkProvisioned{vlan}
    D->>D: create host VLAN parent.vlan<br/>(rtnetlink, EEXIST tolerated)

    loop until showtime
        T->>T: poll /proc/tc3162/xdsl_stats
        W->>B: op2 get_line_obj (1 s daemon poll)
    end
    Note right of B: showtime reached
    T->>B: dslHandleStatusUp:<br/>materialize queued links<br/>(nas dev, bridge, eth0.1.vlan)
    B-->>W: op2: link_status=Up, data_path
    W-->>D: WorkerEvent::LineState{Up, data_path}
    D->>D: hotplug UP + tc_layer event<br/>(auto mode: trained-type swap check)
```

Three properties fall out of this sequence:

- **Provisioning before Up is intended.** The op5/op15 handlers explicitly
  queue link requests in the board's `g_interfaceList` when the line isn't
  up — "configuration is decoupled from link state". The board's own status
  thread materializes them at showtime and re-materializes them after every
  drop (DOWN resets nodes to *pending*, config preserved). The daemon never
  needs to re-send op5/op15 after a resync.
- **`wan adsl close` (op3) is the shutdown lever.** The daemon owns line
  state: stopping it deliberately closes the line (`LineDown`, zero-retry
  op3), and starting it re-opens deterministically via the double apply.
  op7, not op3, is the board reboot.
- **Auto mode** queues an *optimistic* untagged PTM link at startup (the
  TC mode is unknown until showtime, but PTM covers the common VDSL case
  with zero config). The `LineState{Up}` event carries the trained
  modulation; if the line trained ADSL, the daemon swaps the queued link
  post-up (op16 the PTM node + op5 the ATM replacement, warned — see the
  F3 note in [e2e.md](e2e.md) scenario EA) and re-swaps if a mixed-mode
  line re-trains as the other family. Explicitly typed links are never
  silently retyped.

### NoSignal watchdog — warn-only

The watchdog never acts on the line. An *open* modem retries training on
its own (field pcaps show minutes of `NoSignal ↔ Initializing` with zero
host traffic), so re-applying config while it handshakes is noise, and
op3 would kill the auto-retry. The actionable cases are covered
elsewhere:

| Situation | Recovery path |
|---|---|
| board unreachable → reachable | `BoardReachable` event → automatic `OpenLine` |
| line closed by a foreign tool / old daemon | operator runs `rbctl-dsl restart-line` |
| training in progress (`Initializing`/`Handshake`) | nothing — dwell resets, auto-retry continues |

After 30 s (`NOSIGNAL_WATCHDOG`) of *continuous* `NoSignal` with config
applied and the board reachable, the daemon logs **one** warning per
silent episode (continuous NoSignal without cycling = closed modem or no
cable), pointing at `rbctl-dsl restart-line` for a manual bounce.

### Two threads, one shared state

Once startup completes, the process splits in two:

```mermaid
flowchart LR
    subgraph MAIN["control thread  (50 ms tick)"]
        direction TB
        M1["drain WorkerEvents<br/>(gate decisions)"]
        M2["watchdog_tick<br/>(NoSignal retrain)"]
        M3["IPC accept_one"]
        M4["ubus poll_one"]
        M5["sleep 50 ms"]
        M1 --> M2 --> M3 --> M4 --> M5
    end
    subgraph WORKER["board worker thread"]
        direction TB
        W1["execute WorkerCmds<br/>(op1/op3/op5/op15/op16…)"]
        W2["maybe_poll_board<br/>(op2 every 1 s)"]
        W3["sleep 50 ms"]
        W1 --> W2 --> W3
    end
    STATE[("SharedState<br/>Arc&lt;Mutex&lt;DslState&gt;&gt;")]
    MAIN -->|"WorkerCmd"| WORKER
    WORKER -->|"WorkerEvent"| MAIN
    MAIN -.->|"build_snapshot read"| STATE
    WORKER -.->|"poll write"| STATE
    WORKER -.->|"fork + env"| HOTPLUG["notify script"]

    style MAIN fill:#16213e,color:#fff
    style WORKER fill:#3a1a5a,color:#fff
```

The worker is a pure executor ("no decisions"); the daemon is the gate that
owns all policy state (`config_applied`, `link_provisioned`, `vlan_owned`).
This split exists because the board can block for **seconds** on a silent or
training line (2 s timeout × 3 retries × drain-loop). Keeping it on its own
thread means `status` returns within microseconds even while the worker is
mid-retransmit.

### Clean shutdown

On `SIGTERM` / `SIGINT` / `Stop` IPC command:

1. `SHOULD_EXIT` is set (deferred while a firmware flash is in flight).
2. Control loop exits, emits a final `Down` hotplug event.
3. Daemon gates teardown through the worker: `RemoveLink` (op6/op16, board
   side) then `LineDown` — a **zero-retry op3 `wan adsl close`**. The line
   is left deliberately DOWN: "daemon stopped ⇒ line down" is a defined
   state, and the next daemon start re-opens it via the guaranteed
   double apply (`OpenLine`).
4. Main thread waits up to `SHUTDOWN_DEADLINE` (3 s) for the worker; a
   silent board is abandoned (process exit reaps the thread).
5. The host transport VLAN is deleted **only if this daemon instance
   created it** (`vlan_owned` — a pre-existing VLAN from an earlier run or
   the operator is left alone).
6. IPC socket file is removed.

---

## Inside a board request

`Board::request()` is the heart of the protocol side. Every opcode goes
through it. Here is what happens for one `get_line_obj()` (opcode 2):

```mermaid
sequenceDiagram
    participant Caller as board.get_line_obj()
    participant B as Board::request()
    participant S as af_packet::RawSocket
    participant K as Linux kernel
    participant Board as EcoNet board

    Caller->>B: request(2, &[0x02])
    B->>B: seq = SeqCounter.next()
    B->>B: build_command_frame(subtype=2, seq, mac, payload)
    B->>B: set_checksum (CRC-16/ARC)

    loop attempt 0..=retries (3)
        B->>S: send(tx_buf)
        S->>K: sendto(AF_PACKET)
        K->>Board: 0x88B5 frame on lan0.500

        loop drain RX queue
            S->>K: recvfrom (SO_RCVTIMEO)
            K-->>S: PACKET_OUTGOING echo of own TX
            S-->>B: own frame (seq matches, but magic=COMMAND)
            B->>B: skip — not a response

            alt board replies in time
                K-->>S: 0x88B5 response frame
                S-->>B: rx_buf (magic=RESPONSE, seq matches)
                B->>B: verify_checksum()
                B-->>Caller: Ok(payload)
            else timeout (WouldBlock)
                B->>B: break inner loop, retransmit
            end
        end
    end

    B-->>Caller: Err(Timeout)
```

Two details worth knowing as a newcomer:

- **TX echo drain.** `AF_PACKET` on Linux delivers a copy of your own
  transmitted frame back to the socket as `PACKET_OUTGOING`. The inner loop
  discards it by checking `is_response()` and `seq`/`subtype` match — without
  this, every request would see its own TX and treat it as the reply.
- **`strip_echo`.** The board inconsistently prefixes its payload with the
  opcode byte (opcode 2 does, opcodes 4/5 don't). `strip_echo()` detects the
  prefix and skips it; per-opcode parsers then work on stable offsets.

The 8 typed methods on `Board` map 1:1 to the original binary's opcodes — see
[commands/dispatch.md](commands/dispatch.md) for the C-side dispatch table and
[xdsl/opcodes.md](xdsl/opcodes.md) for the per-opcode payload breakdown.

---

## Testing strategy

| Layer | Command | What it covers |
|---|---|---|
| Protocol core | `cargo test -p rbctl_proto` | 58 tests — CRC, frame build/parse, all pack/unpack round-trips, board_sim (decompiled handler rules); +33 with the `protobuf` feature (codec/framing round-trips) |
| AF_PACKET | `cargo test -p af_packet` | 10 tests — BPF filter, MAC parsing, ethertype match |
| Board controller | `cargo check -p rbctl_dsl` (host) | The daemon links target FFI libs, so its tests run cross-compiled: `MockTransport` coverage executes on-device / under the E2E harness instead |
| E2E harness | `tests/e2e/run.sh` | Real MIPS firmware + real daemon under qemu-user: E1–E9b, ED, EX, EA — wire formats, bring-up, materialization, teardown, validation matrix |
| QEMU selftest | `rbctl-dsl selftest -i lan0.500` | Socket open + BPF + bind, VLAN create/up/down/delete round-trip, board opcodes, UCI/uloop init |
| Sniffer | `rbctl-dsl sniff -i lan0.500 --dump /tmp/cap.pcap` | Passive 0x88B5/0x88B6 capture, Wireshark-openable |

The `MockTransport` in `board.rs` is what makes the board logic testable on a
CI host with no hardware — it auto-generates valid response frames and can be
programmed to fail the first N attempts (for retransmit coverage) or never
respond (for timeout coverage).

---

## Where to go next

- **Wire formats & RE background** → [index.md](index.md), [architecture.md](architecture.md), [protocol.md](protocol.md)
- **Every `unsafe`, drop guarantee, and deferred item** → [safety-audit.md](safety-audit.md)
- **Per-opcode payloads and enums** → [xdsl/](xdsl/)
- **Build / package / on-device test instructions** → top-level [README.md](../README.md) and [test.md](test.md)
- **Phased build plan** → [`plans/rbctl-daemon-plan.md`](../plans/rbctl-daemon-plan.md)
