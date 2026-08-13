# Plan — Auto-mode, board-reachability detection, and startup fixes

> Resolves the field-reported failure: **the daemon never brings the line up
> at boot, and `status` shows empty values instead of an error.**
>
> Root-cause analysis lives in the conversation that produced this plan; the
> key findings are summarised in §1.

---

## 1. Problem summary

Five independent defects combine to produce the reported behaviour:

### 1.1 Parent interface `lan0` never brought up at boot (root cause)

| Component | Brings up `lan0.500`? | Brings up `lan0`? |
|---|---|---|
| `files/dsl_defaults` (UCI) | declares `dsl_mgmt` device | **no device section for `lan0`** |
| `files/dsl_control` (init) | `ip link set lan0.500 up` (line 35) | **no** — `parent` var computed but never used for bring-up |
| `daemon::run()` | `iface_exists(lan0.500)` check only | **no** — and **should not** (design decision, §2.1) |
| `selftest::test_link_up` | `bring_up(lan0.500)` | **`bring_up(lan0)`** ← correct, this is the selftest's job |

Without `lan0` up there is no carrier on VLAN 500 → every `0x88B5` frame is
dropped → op1/op15/op2 all time out → `line_obj` stays `None` → `status`
shows config defaults.

The fix belongs in the **init script** (system-config concern), not in the
daemon. The daemon must not alter management interfaces — it only checks
communication and proceeds.

### 1.2 No way to select Multimode / Annex-Auto

`uci_cfg.rs` parsing has no path to the enum variants that the board's own
boot defaults use (the only config that demonstrably trains):

| Board boot default (trains) | Closest daemon config |
|---|---|
| `Multimode` (7) | `Vdsl2` (6) — hardcoded in `parse_modulation` |
| `Annex::Auto` (8) | `B` (1) or `A` (0) — `parse_annex` rejects `"auto"` |
| `sra = off` | `sra = on` (UCI default `'1'`) |

### 1.3 `status` shows empty values when board is unreachable

When every op2 times out, `line_obj` is `None`. The IPC `status` handler
returns `OK` with zeroed fields instead of an error. The user sees:

```
OK
state:
state_num: 0
up: false
...
```

No indication that the management link is down.

### 1.4 No retry of op1/op15 after initial failure

op1/op15 are sent once at startup, best-effort (daemon.rs:372-393). If they
fail (board unreachable), they are **never retried** — the worker only polls
op2. Even when the board later becomes reachable, it never receives the
daemon's config.

### 1.5 Selftest sends wrong opcode and malformed params

The selftest hardcodes op1 with `Vdsl2/B/30a` then sends **op5 (ATM link
add)** with `vlan_id=0`, `tag_vid=0xffff`, `tag_pri=0xff` (unset defaults) —
wrong opcode for a VDSL2 line and invalid VLAN params. This is what
"bricks" the board state after the selftest runs.

---

## 2. Design principles (locked decisions)

1. **The daemon does NOT bring up or alter management interfaces.** It
   reads carrier status (sysfs, read-only) and proceeds or reports an
   error. Interface configuration is the init script's / netifd's job.

2. **The selftest IS responsible for bringing up interfaces.** It is a
   user-executed diagnostic command; the user reads the result and acts.

3. **The selftest must be static and reproducible.** The only user-driven
   parameter is `--config-iface`. All board params (modulation, annex,
   profiles, VLAN, ATM params) are hardcoded constants — no UCI, no CLI
   overrides.

4. **op1 is sent FIRST in the selftest** with proper automatic modulation
   (`Multimode` / `Annex::Auto` / all profiles), matching the board's own
   boot defaults. Then op2 detects what the line negotiated, and the
   correct link opcode (op15 PTM or op5 ATM) follows.

---

## 3. Phased design

> Phases are numbered in **execution order**. Each phase builds on the
> ones before it. The dependency graph is in §6.

### Phase 1 — Extract `MgmtLinkStatus` module (foundation)

**Goal:** consolidate the scattered management-interface status logic into
a single read-only module, so that all subsequent phases (daemon carrier
check, IPC status, selftest) share one common interface.

#### 1a. Why this comes first

The mgmt-interface inspection logic is currently spread across five
locations with duplicated code:

| Where | What | Read-only? |
|---|---|---|
| `daemon::iface_exists()` (daemon.rs:128) | `if_nametoindex` | yes |
| `daemon::parent_iface()` (daemon.rs:74) | string split on `.` | yes |
| `selftest::has_carrier()` (selftest.rs:94) | sysfs `/carrier` | yes |
| `selftest::bring_up()` (selftest.rs:78) | sysfs `/operstate` **+ rtnl set_up** | **no** (writes) |
| Phase 3 + 4 (this plan) would add more inline carrier reads | sysfs | yes |

Extracting the read-only half into a module first means Phase 3 (daemon
carrier check), Phase 4 (IPC status), and Phase 2 (selftest) all call
`MgmtLinkStatus::probe()` instead of duplicating sysfs reads. No throwaway
inline helpers.

#### 1b. Module: `crates/rbctl_dsl/src/mgmt_link.rs`

```rust
//! Read-only management interface status.
//!
//! The daemon never alters management interfaces — this module only
//! inspects them. Write operations (bring-up, VLAN create) live in the
//! init script and selftest respectively.

use std::ffi::CString;

/// Snapshot of a management interface's state at a point in time.
/// All fields are read from sysfs / ioctls — no side effects.
#[derive(Debug, Clone)]
pub struct MgmtLinkStatus {
    /// The VLAN sub-interface the daemon communicates over (e.g. "lan0.500").
    pub iface: String,
    /// The parent physical interface, if the name contains '.' (e.g. "lan0").
    pub parent: Option<String>,
    /// Whether the interface name resolves to an index (if_nametoindex).
    pub exists: bool,
    /// operstate ("up", "down", "unknown") — None if unreadable.
    pub operstate: Option<String>,
    /// Carrier present on the config interface.
    pub carrier: bool,
    /// Carrier present on the parent (None if no parent).
    pub parent_carrier: Option<bool>,
}

impl MgmtLinkStatus {
    /// Probe the current state. Reads sysfs only — no writes, no ioctls
    /// that change state.
    pub fn probe(config_iface: &str) -> Self {
        let parent = parent_of(config_iface).map(str::to_string);
        let exists = iface_exists(config_iface);
        let carrier = read_carrier(config_iface).unwrap_or(false);
        let parent_carrier = parent.as_deref().and_then(read_carrier);
        let operstate = read_operstate(config_iface);
        Self { iface: config_iface.to_string(), parent, exists, operstate, carrier, parent_carrier }
    }

    /// Interface exists AND has carrier on both itself and its parent.
    pub fn is_ready(&self) -> bool {
        self.exists && self.carrier && self.parent_carrier.unwrap_or(true)
    }

    /// True if the parent exists but lacks carrier (most common failure).
    pub fn parent_down(&self) -> bool {
        self.parent_carrier == Some(false)
    }

    /// Human-readable one-liner for log/error messages.
    pub fn diagnostic(&self) -> String {
        if !self.exists {
            return format!("'{}' does not exist", self.iface);
        }
        if self.parent_down() {
            return format!(
                "no carrier on parent '{}' — ensure it is up (dsl_control / netifd / board.d)",
                self.parent.as_deref().unwrap_or("?")
            );
        }
        if !self.carrier {
            return format!("no carrier on '{}' (parent ok)", self.iface);
        }
        "ok".into()
    }
}

// ── free functions (reusable independently) ──

/// Extract parent name: `"lan0.500"` → `Some("lan0")`, `"eth0"` → `None`.
pub fn parent_of(iface: &str) -> Option<&str> {
    iface.rsplit_once('.').map(|(p, _)| p)
}

/// `if_nametoindex` wrapper — true if the interface exists.
pub fn iface_exists(name: &str) -> bool {
    CString::new(name)
        .map(|c| unsafe { libc::if_nametoindex(c.as_ptr()) } != 0)
        .unwrap_or(false)
}

/// Read `/sys/class/net/<iface>/carrier` → `Some(true/false)`, `None` on error.
pub fn read_carrier(iface: &str) -> Option<bool> {
    read_sysfs_u32(iface, "carrier").map(|v| v == 1)
}

/// Read `/sys/class/net/<iface>/operstate` → `Some("up"/"down"/...)`.
pub fn read_operstate(iface: &str) -> Option<String> {
    std::fs::read_to_string(format!("/sys/class/net/{iface}/operstate"))
        .ok()
        .map(|s| s.trim().to_string())
}

fn read_sysfs_u32(iface: &str, attr: &str) -> Option<u32> {
    std::fs::read_to_string(format!("/sys/class/net/{iface}/{attr}"))
        .ok()
        .and_then(|s| s.trim().parse().ok())
}
```

#### 1c. What stays OUT of this module

- **Write operations** (`rtnl set_up`, `add_vlan`, `del_vlan`): these stay
  in `daemon.rs` (for the transport VLAN) and `selftest.rs` (for bring-up).
  The module's contract is strictly read-only.
- **Transport VLAN management** (`create_vlan_iface` / `delete_vlan_iface`):
  unrelated to the management interface, stays in `daemon.rs`.

Pure refactor — all existing tests must pass unchanged.

---

### Phase 2 — Selftest rewrite (static, reproducible, auto-modulation)

**Goal:** the selftest uses a fixed op-sequence with auto modulation,
detects the line type, and sends the correct link opcode. Fully static —
only `--config-iface` is user-driven.

This phase depends on Phase 1 (uses `mgmt_link::parent_of()` and
`mgmt_link::read_carrier()` instead of its own duplicated copies). It is
otherwise independent of the daemon phases and gives you a working
diagnostic tool before touching the daemon.

#### 2a. Hardcoded constants, deterministic sequence

All board-level params are module-level constants in `selftest.rs`:

```rust
// ── selftest board params (static, no user input) ──────────────
// Matches the board's own boot defaults — the config that trains.
const ST_MODULATION: Modulation = Modulation::Multimode;  // 7
const ST_ANNEX: Annex = Annex::Auto;                       // 8
const ST_PROFILES: Vdsl2Profiles = Vdsl2Profiles::EIGHT_A   // 0x1ff
    | Vdsl2Profiles::EIGHT_B | Vdsl2Profiles::EIGHT_C
    | Vdsl2Profiles::EIGHT_D | Vdsl2Profiles::TWELVE_A
    | Vdsl2Profiles::TWELVE_B | Vdsl2Profiles::SEVENTEEN_A
    | Vdsl2Profiles::THIRTY_A | Vdsl2Profiles::THIRTYFIVE_B;
const ST_BITSWAP: bool = true;
const ST_SRA: bool = false;

// Transport VLAN for the test link (fixed, does not conflict with daemon).
const ST_TEST_VLAN: u16 = 2001;

// ATM params (used only if the line trains as ADSL/ATM).
const ST_ATM_VPI: u8 = 8;
const ST_ATM_VCI: u16 = 35;
```

The `bring_up` / `has_carrier` logic stays in selftest.rs (it's the
write-side — the selftest's job), but delegates the read-only parent /
carrier checks to `mgmt_link` functions from Phase 1.

#### 2b. Sequence: op1 first, then detect, then provision

Replace `test_board()` with a fixed sequence:

```
1. op1  line_config_up(Multimode, Auto, all_profiles, bitswap=true, sra=false)
       → sends the board's default/auto config. If the board already has
         this config, dslCfgSet's diff check is a no-op (no spurious
         retrain). If it had a different config (e.g. leftover from a
         previous daemon run), it retrains to auto.

2. Poll op2 until Up or timeout (e.g. 60s, polling every 2s)
       → waits for the line to train with auto-negotiation
       → reports the training progress (NoSignal → Initializing → Up)

3. op2  final read → detect modulation_code + data_path
       → VDSL2/PTM: send op15 (ptm_link_add) with ST_TEST_VLAN
       → ADSL/ATM:  send op5  (atm_link_add)  with ST_TEST_VLAN + ATM params

4. op4  get_channel_stats → read counters

5. Report all results, PASS/FAIL per step.
```

The detection logic (step 3) uses **two independent signals** from the op2
reply and trusts `data_path` (offset 0x06) — it reflects the actual TC
layer the chipset activated. This is the same logic the daemon uses in
Phase 6c.

#### 2c. op1 before op2 — why this order

The current selftest sends op2 first (read status) then op1 (disruptive
retrain). This is backwards:
- If the line was already up (board's defaults), op2 reads `Up` — but then
  op1's `Vdsl2/B/30a` forces a retrain to a config that may not work,
  leaving the line broken when the selftest exits.
- Sending op1 first with `Multimode/Auto` is either a no-op (if the board
  already has this config) or a retrain **to the known-good auto config**.
  Either way, the line is left in a good state when the selftest exits.

#### 2d. Result reporting

Each step logs its result. The selftest reports what it detected so the
user can diagnose:

```
[INFO]  op1: ACK (Multimode/Auto/0x1ff, bitswap=1 sra=0)
[INFO]  waiting for line to train (up to 60s)...
[INFO]  poll: NoSignal (5s)
[INFO]  poll: Initializing (10s)
[INFO]  poll: Initializing (15s)
[INFO]  poll: Up (20s) — mod=VDSL2 datapath=PTM annex=B
[INFO]  detected: VDSL2 / PTM → provisioning PTM link
[INFO]  op15: ACK vlan=2001
[INFO]  op4: rx_blocks=0 tx_blocks=42
[INFO]  PASS board: line up, VDSL2/PTM, down=96492 up=20518 kbps
```

If the line doesn't come up within the timeout:

```
[INFO]  op1: ACK (Multimode/Auto/0x1ff, bitswap=1 sra=0)
[INFO]  waiting for line to train (up to 60s)...
[INFO]  poll: NoSignal (60s) — TIMEOUT
[INFO]  PASS board: board is alive (responds to op1/op2) but line did not train
[INFO]  hint: check physical line, DSLAM config, annex compatibility
```

Note: "board is alive" is still a PASS — the selftest's job is to report
the state, not to fix it. The user decides what to do.

#### 2e. Cleanup

The selftest should NOT tear down what it created (no op3, no op6/op16, no
link deletion). Rationale:
- The test link (VLAN 2001) is harmless if left in place.
- Sending op3 would drop the line, defeating the purpose.
- The user analyzes the result and cleans up manually if needed.

The test VLAN interface (`lan0.2001`) created by `test_vlan()` is already
deleted at the end of that sub-test (selftest.rs:128) — that's the VLAN
lifecycle test, separate from the board test, and stays as-is.

---

### Phase 3 — Init script fix + daemon carrier check (critical, P0)

**Goal:** `lan0` is up before the daemon starts; the daemon checks carrier
via `MgmtLinkStatus` (Phase 1) and reports clearly without altering
anything.

#### 3a. Fix `files/dsl_control` init script

The `parent` variable is already computed (line 29) but never used for
bring-up. Add one line:

```sh
# After the VLAN create block (current line 34), BEFORE setting config_iface up:
ip link set "$parent" up 2>/dev/null        # ← ADD
ip link set "$config_iface" up 2>/dev/null   # (already exists)
```

Also add a carrier wait with timeout so the daemon doesn't race the PHY:

```sh
# Wait up to 5s for carrier on the management interface
i=0
while [ $i -lt 50 ] && [ "$(cat /sys/class/net/$config_iface/carrier 2>/dev/null)" != "1" ]; do
    sleep 0.1
    i=$((i + 1))
done
```

#### 3b. Daemon: read-only carrier check via `MgmtLinkStatus`

Replace the existence-only check in `daemon::run()` (daemon.rs:344-352).
The daemon uses `MgmtLinkStatus::probe()` from Phase 1 — **reads** sysfs
but never calls `set_up`:

```rust
let link = mgmt_link::MgmtLinkStatus::probe(config_iface);

if !link.exists {
    log::error!("management interface {}", link.diagnostic());
    log::error!("  create it via UCI device section or board.d before starting the daemon");
    return 1;
}

if !link.is_ready() {
    log::error!("management interface not ready: {}", link.diagnostic());
    // Don't abort: the worker will report board-unreachable (Phase 4)
    // and retry op1 when carrier appears (Phase 5).
} else {
    log::info!("management interface {} ready (carrier ok)", config_iface);
}
```

The daemon proceeds regardless of carrier state — the worker's polling
(Phase 4) and op1 retry (Phase 5) handle the board-appears-later case.

#### 3c. `files/dsl_defaults`: document the requirement

Add a comment block (no code change needed if board.d handles `lan0`):

```sh
# NOTE: the physical interface 'lan0' must be declared and brought up
# by board.d / netifd (typically a switch-port device section).
# dsl_control ensures it is up before starting the daemon, but the
# device itself must exist in UCI for netifd to manage it persistently.
```

If this device is not managed by netifd at all (raw switch port), the
recommended approach is a board.d script that creates the UCI device
section at first boot.

---

### Phase 4 — Board reachability detection + status error reporting

**Goal:** `status` (and all IPC commands) report a clear error when the
board is unreachable, instead of empty values. Uses `MgmtLinkStatus`
(Phase 1) for the carrier fields.

#### 4a. Add reachability state to `SharedState`

In `ubus_obj.rs`, extend `DslState`:

```rust
pub struct DslState {
    // ... existing fields ...
    /// Instant of the last successful op2 poll, or None if never.
    pub last_poll_ok: Option<Instant>,
    /// True if op2 has succeeded at least once since startup.
    pub board_ever_seen: bool,
}
```

#### 4b. Worker updates reachability on every poll

In `board_worker.rs::maybe_poll_board()`:

```rust
match self.board.get_line_obj() {
    Ok(line) => {
        let mut st = self.state.lock().unwrap();
        st.last_poll_ok = Some(Instant::now());
        st.board_ever_seen = true;
        st.line_obj = Some(line);
        // ... existing transition logic ...
    }
    Err(BoardError::Timeout) => {
        let mut st = self.state.lock().unwrap();
        let stale = st.last_poll_ok.map_or(true, |t| t.elapsed() > Duration::from_secs(30));
        if stale {
            st.line_obj = None;  // clear stale data so status shows "unreachable"
        }
    }
    Err(e) => log::error!("poll error: {e}"),
}
```

#### 4c. IPC `status` returns error when board unreachable

Add to `StatusSnapshot` (ipc.rs):

```rust
pub struct StatusSnapshot {
    // ... existing fields ...
    pub board_reachable: bool,
    pub mgmt_iface: String,
    pub mgmt_carrier: bool,
}
```

`daemon::build_snapshot()` populates these by calling
`MgmtLinkStatus::probe()` (Phase 1) + reading `last_poll_ok` from
SharedState.

When `board_reachable == false`, the STATUS reply becomes:

```
ERR: board unreachable on lan0.500
mgmt_iface: lan0.500
mgmt_carrier: false
parent: lan0
parent_carrier: false
hint: ensure management interfaces are up (check dsl_control / netifd)
.
```

When carrier is present but board doesn't respond to op2:

```
ERR: board unreachable on lan0.500 (carrier ok, no op2 response)
mgmt_iface: lan0.500
mgmt_carrier: true
last_seen: 45s ago
.
```

When reachable but line down, keep the current `OK` format with real
line-state values.

#### 4d. `restart-line` / `reload` behaviour when unreachable

- `restart-line`: returns `ERR: board unreachable` to the IPC client
  (there's nothing to restart).
- `reload`: always succeeds (re-reads UCI, updates SharedState display
  fields).
- `stop`: always works.

---

### Phase 5 — op1 retry on board becoming reachable

**Goal:** if op1 failed at startup (board unreachable), retry it when the
worker detects the board has become reachable. Depends on Phase 4's
reachability tracking.

#### 5a. Track reachability transitions in the worker

```rust
struct BoardWorker {
    // ... existing ...
    board_reachable: bool,
    config_applied: bool,   // op1 succeeded at least once
}
```

In `maybe_poll_board()`, after a successful op2:

```rust
if !self.board_reachable {
    log::info!("board is now reachable");
    self.board_reachable = true;
}
```

After a timeout (board was reachable, now silent):

```rust
if self.board_reachable {
    log::warn!("board became unreachable");
    self.board_reachable = false;
    self.config_applied = false;
    self.link_provisioned = false;
}
```

#### 5b. Retry op1 when board (re)appears and config not yet applied

In the worker loop, after `maybe_poll_board()`:

```rust
if self.board_reachable && !self.config_applied {
    log::info!("applying line config (deferred from startup)");
    if self.board.line_config_up(
        self.cfg.modulation, self.cfg.annex, self.cfg.profiles,
        self.cfg.bitswap, self.cfg.sra,
    ).is_ok() {
        self.config_applied = true;
    }
}
```

For fixed mode: also retry op5/op15 here (they queue on the board and
materialize on the next Up transition).

For auto mode: op1 is sent (with Multimode/Auto) but the link add is
deferred to Phase 6c's Up-transition handler.

---

### Phase 6 — Auto-mode with deferred link provisioning

**Goal:** `line_mode=auto` lets the board negotiate, then the worker
creates the right data-plane link after the line comes up. Depends on
Phase 5's reachability tracking and op1 retry.

Replicates how the original `libcmm.so` works: reads op2's modulation byte
to decide PTM vs ATM (`oal_dsl_getConfigModulateType`, layers.md:110-126).

#### 6a. Extend config parsing to support Multimode / Annex-Auto

In `uci_cfg.rs`:

```rust
pub fn parse_modulation(line_mode: &str, _: &str) -> Result<Modulation, _> {
    match line_mode {
        "vdsl" | ""      => Ok(Modulation::Vdsl2),
        "adsl"            => Ok(Modulation::Adsl2Plus),
        "auto" | "multi"  => Ok(Modulation::Multimode),   // ← ADD
        other => Err(...),
    }
}

pub fn parse_annex(s: &str) -> Result<Annex, _> {
    match s.trim() {
        "a"|"A" => Ok(Annex::A),
        "b"|"B" => Ok(Annex::B),
        // ... existing ...
        "auto"|"AUTO" => Ok(Annex::Auto),                 // ← ADD
        other => Err(...),
    }
}
```

`validate.rs` already accepts `Multimode` (7) and `Annex::Auto` (8) — no
changes needed there (existing tests `profile_with_multimode_ok`,
`auto_annex_with_every_modulation_ok`).

When `line_mode=auto`:
- `annex` defaults to `"auto"` → `Annex::Auto`
- `tone` defaults to `"av"` (all profiles)
- `sra` defaults to **false** (match board default)
- `bitswap` defaults to true

#### 6b. Add `XferMode::Auto` variant

```rust
pub enum XferMode {
    Atm,
    Ptm,
    Auto,   // ← ADD: decide after line-up based on op2 response
}
```

When `line_mode=auto`, `xfer_mode` defaults to `Auto`.

#### 6c. Deferred link provisioning in the worker

In auto mode, the daemon sends op1 (Multimode/Auto) at startup (or on
Phase 5 retry) but does **not** send op5/op15. Instead, the worker
provisions the link on the first `Up` transition:

Add to `BoardWorker`:

```rust
struct BoardWorker {
    // ... existing ...
    link_provisioned: bool,  // reset to false on Down transition
}
```

In `maybe_poll_board()`, on `link_status == Up && !link_provisioned`:

```rust
if status == LinkStatus::Up && !self.link_provisioned {
    // Two independent signals from op2:
    let mode = line.modulation_code;   // offset 0x05: 6=VDSL2
    let dp = line.data_path;            // offset 0x06: 0=ATM, 1=PTM

    // Trust data_path — it reflects the actual TC layer activated.
    let xfer = match dp {
        DataPath::Ptm => XferMode::Ptm,
        DataPath::Atm => XferMode::Atm,
    };

    log::info!("auto: line up (mod={}, datapath={:?}), provisioning {:?} link",
               mode, dp, xfer);

    match xfer {
        XferMode::Ptm => {
            self.board.ptm_link_add(0, 0, 0, transport_vlan)?;
        }
        XferMode::Atm => {
            self.board.atm_link_add(&atm_params)?;
        }
        _ => unreachable!(),
    }
    // Create host-side VLAN, set link_provisioned = true, update st.xfer_mode
}
```

On `link_status != Up`: `self.link_provisioned = false`.

If `modulation_code` and `data_path` disagree (e.g. mod=6 but dp=ATM),
log a warning but trust `data_path` — it reflects what the chipset
actually activated.

#### 6d. Host-side VLAN creation timing in auto mode

The transport VLAN creation (`create_vlan_iface`, daemon.rs:398) runs at
startup in fixed mode. In auto mode, defer it to the worker's post-Up
provisioning step. The VLAN id is known (`transport_vlan_base + 2000`);
only the creation timing changes.

#### 6e. Fixed mode unchanged

When `xfer_mode` is explicitly `Ptm` or `Atm` (not `Auto`), the existing
startup-time op1 + op5/op15 sequence is preserved. Auto mode is strictly
additive.

---

## 4. File-by-file change list

| File | Phase | Change |
|---|---|---|
| `crates/rbctl_dsl/src/mgmt_link.rs` | 1 | **NEW** — `MgmtLinkStatus` struct + `probe()`, `parent_of()`, `iface_exists()`, `read_carrier()`, `read_operstate()` (read-only) |
| `crates/rbctl_dsl/src/main.rs` | 1 | add `mod mgmt_link;` |
| `crates/rbctl_dsl/src/selftest.rs` | 2 | hardcoded auto params; op1-first sequence; modulation detection; correct link opcode; poll-for-up with timeout; static result reporting; reuse `mgmt_link` read functions for parent/carrier checks |
| `files/dsl_control` | 3a | add `ip link set "$parent" up` + carrier wait loop |
| `files/dsl_defaults` | 3c | add comment documenting `lan0` requirement |
| `crates/rbctl_dsl/src/daemon.rs` | 3b, 4c, 6d | use `MgmtLinkStatus::probe()` (carrier check); add reachability to snapshot; defer VLAN create in auto mode |
| `crates/rbctl_dsl/src/ubus_obj.rs` | 4a | add `last_poll_ok` / `board_ever_seen` to `DslState` |
| `crates/rbctl_dsl/src/board_worker.rs` | 4b, 5a, 5b, 6c | reachability tracking; deferred link provisioning; op1 retry |
| `crates/rbctl_dsl/src/ipc.rs` | 4c, 4d | add `board_reachable` / `mgmt_carrier` to `StatusSnapshot`; error reply when unreachable; guard `restart-line` |
| `crates/rbctl_dsl/src/uci_cfg.rs` | 6a, 6b | add `"auto"` → `Multimode`/`Annex::Auto`; add `XferMode::Auto`; fix `sra` default for auto |

---

## 5. Testing

### 5.1 Unit tests (host, `cargo test`)

- `mgmt_link.rs`: `parent_of("lan0.500")` → `Some("lan0")`;
  `parent_of("eth0")` → `None`; `MgmtLinkStatus::diagnostic()` produces
  correct messages for exists-but-no-carrier vs parent-down vs ok.
  (Use mock sysfs paths or test the string-parsing logic in isolation;
  `iface_exists` is best tested on-device.)
- `uci_cfg.rs`: `parse_modulation("auto")` → `Multimode`;
  `parse_annex("auto")` → `Annex::Auto`; `XferMode::Auto` default when
  `line_mode=auto`; `sra=false` when auto.
- `board_worker.rs`: mock worker — simulate board-unreachable → reachable
  transition → verify op1 retried; simulate Up transition with mock
  `modulation_code=6` / `data_path=Ptm` → verify op15 sent (not op5);
  simulate `data_path=Atm` → verify op5 sent.

### 5.2 On-device validation

```sh
# Phase 1: module compiles, existing tests pass (pure refactor)
cargo test -p rbctl_dsl

# Phase 2: selftest (static, auto-modulation)
rbctl-dsl selftest --config-iface lan0.500
# should report: op1 ACK → poll for Up → detect VDSL2/PTM → op15 → PASS
rbctl-dsl status                # line should still be up after selftest

# Phase 3: init script brings up lan0
ip link set lan0 down           # simulate the broken boot state
/etc/init.d/dsl_control restart
cat /sys/class/net/lan0/carrier # should be 1
rbctl-dsl status                # should show real data or clear error

# Phase 4: error reporting (disconnect the board cable)
rbctl-dsl status                # should show ERR: board unreachable

# Phase 6: auto mode
uci set network.dsl.line_mode='auto'
uci set network.dsl.annex='auto'
uci set network.dsl.sra='0'
uci commit network
/etc/init.d/dsl_control restart
# wait for line to train, then:
rbctl-dsl status                # should show Up with detected modulation
ip link show lan0.2000          # transport VLAN created by worker
```

### 5.3 Regression

- Fixed mode (`line_mode=vdsl`, `xfer_mode=ptm`) behaves exactly as before
  when the board is reachable from the start.
- Daemon restart with line already up does not force a retrain if config
  is unchanged (board's `dslCfgSet` diff check handles this — verify).
- Selftest does not break the line state (op1 with Multimode/Auto matches
  board defaults → no-op diff check → no spurious retrain).

---

## 6. Sequencing

```
Phase 1  ──→  MgmtLinkStatus module          (foundation, pure refactor)
   │
   ├── Phase 2  ──→  selftest rewrite         (uses mgmt_link; diagnostic tool)
   │
   ├── Phase 3  ──→  init script + carrier    (uses mgmt_link; P0 fix)
   │       │
   │       └── Phase 4  ──→  status errors    (uses mgmt_link; reachability)
   │               │
   │               └── Phase 5  ──→  op1 retry (uses Phase 4's reachability)
   │                       │
   │                       └── Phase 6  ──→  auto-mode (uses Phase 5's retry)
```

| Phase | Depends on | Can parallel with | What it delivers |
|---|---|---|---|
| **1** | — | — | shared `MgmtLinkStatus` module |
| **2** | 1 | 3, 4, 5, 6 | working selftest (validation tool) |
| **3** | 1 | 2 | **fixes the reported boot failure** (P0) |
| **4** | 3 | 2, 5, 6 | clear `status` errors |
| **5** | 4 | 2, 6 | self-healing (op1 retry) |
| **6** | 5 | 2 | auto-detection production mode |

- **Phase 1** is the foundation — everything else uses it.
- **Phase 2** gives you a diagnostic tool to validate subsequent phases.
- **Phase 3** alone fixes the reported boot failure.
- **Phases 4+5** make the daemon self-healing and self-reporting.
- **Phase 6** adds the auto-detection capability for production use.
