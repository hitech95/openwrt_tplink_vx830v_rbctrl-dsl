# Plan — Wire-format fixes, cache-safe bring-up, version banner, board-sim tests

> **Status (2026-08-23): steps 1–6, 8, 9 IMPLEMENTED** (this working
> session) — echo-byte removal, `BringLineUp` double apply, watchdog
> rework (30 s warn / 5 min re-apply), version banner + `build.rs`, CLI
> case-insensitivity + fatal-on-bad-value, `NOSIGNAL_REAPPLY`, docs
> (§5.1–§5.3 incl. board-side research docs), 0.4.0 SDK build verified
> (string-level). **Step 7 (QEMU E2E §3.5) IMPLEMENTED and GREEN**:
> `tests/e2e/run.sh` runs the real MIPS `remote_board` under qemu-mips
> against the real aarch64 daemon under qemu-aarch64 over a veth —
> scenarios E1 E2 E3 E5 E6 E8 all pass (`SCENARIOS='E7'` = the 5-min
> watchdog soak, opt-in). Capture instrument: strace (`openat`+`write`)
> tailer → lossless `tcci_cmd` trace; state injection via tmpfs
> `/proc/tc3162`; no host dependencies beyond qemu-user + strace.
> §3.4 on-target tester matrix: **EXECUTED across debug sessions 1–4**
> (stop/start cycles, watchdog soak, metric decode, PPPoE sweep with
> host pcaps) — session 4 additionally root-caused the dead data plane
> on-target and validated the bearer-tag fix (working PPPoE link;
> follow-up in [data-link-uci-plan.md](data-link-uci-plan.md), released
> as 0.5.0).

> Resolves the session-2 field failure: **the line never trains because every
> request payload is field-shifted (bogus opcode-echo byte) and op3 — sent
> right after op1 at startup and by the watchdog — is `wan adsl close`, so the
> daemon opens the modem and closes it 12 ms later.**
>
> Evidence: the session-2 field-failure analysis and the disassembled
> `remote_board` firmware binary — both live in the private research tree
> (not distributed with this repo; see `docs/e2e.md` for how the
> binary is used). Ground truth was verified on 2026-08-22.

---

## 1. Problem summary

| # | Defect | Evidence | Consequence |
|---|--------|----------|-------------|
| 1.1 | TX payloads prepend an "opcode echo" byte; the board reads the payload from byte 0 (`req = g_frameBuf+0x602`) | wire `01 06 01 …` decoded by board as mod=gdmt annex=j; op16 `10 07d0 03` → status 2 every shutdown | op1 configures garbage; op5/6/15/16 fields shifted; links never deleted board-side |
| 1.2 | op3 = `wan adsl close` (string @ `0x416a34`), not a retrain | disasm of `board_op3_handler` → `FUN_00404298`; pcap: op1+op3 windows are 100% NoSignal, op1-only windows cycle NoSignal↔Initializing | startup and watchdog actively keep the line down |
| 1.3 | `dslCfgSet` dedups an op1 identical to `l_lastDslCfg` (`0x429840`) — no-op, **no `wan adsl open`** | disasm of diff-check `FUN_00404538` (field-by-field compare) | after our shutdown-op3, a restart with unchanged config cannot reopen the line |
| 1.4 | daemon version not printed at startup | message_3.txt | tester logs cannot be mapped to a source tree |
| 1.5 | test mock fabricates the same echo byte in responses (`vec![subtype, 0x00]`) and no host test models the real handler logic | `board.rs` `default_response_payload` | wire bugs of this class cannot be caught on host |
| 1.6 | board-side docs stated wrong facts: op3 = "retrain (`wan adsl reset`)" (it is `wan adsl close`), 1-based modulation table (codes are 0-based); the echo-byte misconception was even documented as board behavior in source comments | FINDINGS.md; `dsl-config.md`/`opcodes.md`/`led-mac-misc.md` (partly corrected already, §5.1) | future work re-derives from bad references |

## 2. Fix design

### 2.1 Remove the opcode-echo byte from all TX payloads (`board.rs`)

Handlers read the payload from offset 0; **no** opcode byte belongs in it.
Change the six builders to send the packed descriptor directly:

| Method | Today | After |
|---|---|---|
| `line_config_up` | `vec![0x01] + pack_dsl_line (13 B)` | `pack_dsl_line (12 B)` |
| `line_retrain` | `[0x03]` | `[]` (payload-less, `plen=0`) |
| `get_line_obj` | `[0x02]` | `[]` |
| `get_channel_stats` | `[0x04]` | `[]` |
| `atm_link_add` | `vec![0x05] + pack_atm_link (25 B)` | `pack_atm_link (24 B)` |
| `atm_link_del` | `vec![0x06] + pack_link_del (4 B)` | `pack_link_del (3 B)` |
| `ptm_link_add` | `vec![0x0F] + pack_ptm_link (9 B)` | `pack_ptm_link (8 B)` |
| `ptm_link_del` | `vec![0x10] + pack_link_del (4 B)` | `pack_link_del (3 B)` |

Board-side length gates (verified): op1 `payload_len < 8` → status 1; op15
`< 8` → status 1; op16 `< 3` → status 1. Payload-less op2/op3/op4 are safe
(handlers never touch the request). The empty payload still produces a valid
CRC (coverage = header + 0 bytes).

**Also delete `strip_echo()`** and its 12 call sites: responses never carry
the echo either (`msg_sendResponse` writes the handler's response buffer
directly at `+0x18`). Replace with direct reads (`r[0]`, `parse_line_obj(&r)`,
…). Update the misleading doc-comment above the removed function.

`WorkerCmd::LineDown` keeps sending op3 at shutdown (semantics now correct:
deliberate close), with the log line updated from "forcing DSL retrain" to
"closing DSL line (wan adsl close)".

### 2.2 Cache-safe bring-up: guaranteed-differing double apply

Replace startup `ApplyLineConfig` + `RetrainLine` with a single command that
performs **two op1 applies differing in exactly the bitswap byte**:

```text
apply(intended.with_bitswap(!intended.bitswap))   // A — cache → A'
apply(intended)                                   // B — always ≠ cache → full
                                                  //     path + wan adsl open
```

Why bitswap: it is an OLR parameter — it does not change which standard or
annex is negotiated, so even if A fully executes (cache ∉ {A, B}) the modem
momentarily holds the right mode with one harmless setting flipped. Exhaustion
over the three possible cache states (== B, == A, other) shows B always
executes the full setter path and leaves the intended config active. No state
needs persisting; works after board reboot (cache = boot default), after our
own shutdown (cache = previous intended), after any foreign writer.

Implementation:

- `worker.rs`: new `WorkerCmd::BringLineUp` (replaces the startup
  `RetrainLine`); executes the double apply, then emits the existing
  `LineConfigApplied` event so the provisioning gate is unchanged.
- `daemon/mod.rs` `startup()`: send `ApplyLineConfig` → `BringLineUp`? No —
  **collapse into one**: startup sends only `BringLineUp` (it applies config
  itself); `ApplyLineConfig` remains for reload/config-change paths where the
  config *did* change (dedup-safe by construction).
- Keep the "line left DOWN by request" shutdown behavior (§2.1 op3).

### 2.3 Watchdog: op3 → slow double-apply, or nothing

Current 30 s op3 kick re-closes the line forever. Change `watchdog_tick` to:

- Keep the 30 s dwell as the *detection* window only.
- On expiry, instead of `RetrainLine`, log once ("line in NoSignal for Xs —
  modem appears closed or untrainable") and escalate on a **5-minute**
  boundary (`NOSIGNAL_REAPPLY`, new constant) by re-sending
  `BringLineUp` (double apply). Rationale: an open modem auto-retrains by
  itself (pcap: minutes of NoSignal↔Initializing with zero host traffic);
  a faster loop adds nothing while handshaking, but a slow re-apply covers
  pathological states (e.g. closed by a previous daemon version's watchdog).
- `Initializing`/`EstablishingLink` keep resetting the dwell (training in
  progress — never touch).

### 2.4 Version banner at daemon start

`main.rs` daemon path, first log line after argv parse, before any board
interaction:

```
[INFO]  rbctl-dsl <version> (git <short-sha><-dirty?>, built <YYYY-MM-DD>) starting
```

- `version` from `env!("CARGO_PKG_VERSION")` (workspace 0.3.3).
- git sha via `build.rs`: `emit rustc-env=RBCTL_GIT_SHA=<short>` from
  `git rev-parse --short HEAD` + `-dirty` suffix from `git status --porcelain`
  (fallback `unknown` outside a repo / source tarball).
- build date via `env!("BUILD_DATE")` set in the same `build.rs`
  (`chrono`-free: `std::process::Command("date" "+%Y-%m-%d")`, host-only).
- Same banner line in `selftest` header (cheap, same helper function in a
  small `src/version.rs`).
- Purpose: field logs (message_3.txt style) can be diffed against an exact
  source tree — this session's analysis was slowed by not knowing the build.

### 2.5 CLI hardening (made the tester's matrix a no-op)

`config.rs` value parsing:

- Accept values case-insensitively and by alias: `vdsl2|Vdsl2|VDSL2`,
  `adsl2+|adsl2plus`, `auto` for both line-mode and annex, `ptm|atm` for
  xfer-mode, tone profiles `8a..30a` (comma list → bitmask).
- **An explicitly passed invalid value becomes fatal** (exit 1 with the
  offending flag), not `WARN … using defaults`. Only a *missing* config file
  falls back to defaults. This is what silently voided 9 of the tester's 11
  runs.

## 3. Host tests — protocol validation against board-binary logic

New test-only module `rbctl_proto::oem::board_sim` (mirrors the disassembled
handlers; compiled `#[cfg(test)]` + `#[cfg(feature = "test-sim")]` so
`rbctl_dsl` tests can reuse it):

```rust
pub struct BoardSim {
    last_dsl_cfg: [u8; 8],        // l_lastDslCfg @ 0x429840 — zeroed at "boot"
    line_open: bool,              // wan adsl open/close state
    atm_links: Vec<AtmLinkRecord>,// g_interfaceList analog
    ptm_links: Vec<PtmLinkRecord> // g_ptmLinks[8] analog
}
impl BoardSim {
    pub fn boot_defaults(&mut self);      // msg_init: cfg = 07 08 01 00 000001ff + open
    pub fn op1(&mut self, payload: &[u8]) -> u8;   // status byte
    pub fn op3(&mut self);                // → line_open = false
    pub fn op2(&self, payload_len: usize) -> SimStatus; // incl. short-frame → status 1
    pub fn op5(&mut self, payload: &[u8]) -> u8;
    pub fn op15(&mut self, payload: &[u8]) -> u8;
    pub fn op16(&mut self, payload: &[u8]) -> u8;
}
```

Sim rules ported 1:1 from the disassembly (each rule cites its function):

| Rule | Source |
|---|---|
| op1: `len < 8` → 1 | `board_op1_handler` @ `0x408d84` |
| op1: field-by-field cache compare (8 B: mod, annex, bs, sra, profiles BE) — equal → no-op status 0; differ → apply + `line_open = true` | diff-check `FUN_00404538`, finalize `FUN_004043a4` |
| op1: unknown modulation (≥ 8) → 2 | `apiSetAdslType` `FUN_00403454` |
| op3: always 0, `line_open = false` | `FUN_00404298` |
| op2: build 63-B status; `link = Up` only if `line_open` && `trained` (sim injects `trained` externally) | `dslCfgGet` |
| op15/op5: `len < 8`/`< 24` → 1; line not open → queue, status 1 (materializes on `set_showtime`) | `board_op15_handler` @ `0x409630` |
| op15: vlanId ∉ 2000–2007 → 2; duplicate tagVid → 1; table full → 1 | `vdsl_linkAdd` `FUN_00408208` |
| op16: vlanId ∉ 2000–2007 → 2; unknown slot → 1; valid → delete | `vdsl_linkDel` `FUN_00408bd0` |

### 3.1 Test matrix (`rbctl_proto` — pure pack/unpack vs sim)

1. `op1_payload_has_no_echo_byte` — `pack_dsl_line` bytes land at payload
   offset 0; sim decodes Vdsl2/Annex::B/profiles 0x1ff correctly.
2. `op1_cache_dedup` — second identical op1 is a no-op (`line_open` unchanged,
   status 0); differing op1 sets `line_open = true`.
3. `op1_boot_default_idempotent` — op1 with the boot-default bytes against a
   freshly booted sim is a no-op (regression guard for the 0.3.1 "zombie").
4. `op1_garbage_modulation_rejected` — modulation ≥ 8 → status 2.
5. `op1_short_payload_rejected` — 7-byte payload → status 1.
6. `op3_closes_line` — open sim + op3 → `line_open = false`; op2 reports
   NoSignal thereafter.
7. `double_apply_always_opens` — property test over all reachable cache
   states (== intended, == intended-with-flipped-bs, boot default, garbage):
   double apply leaves `line_open = true` and `last_dsl_cfg == intended`.
8. `op15_validation_and_queueing` — not-open → status 1 + queued;
   `set_showtime` materializes; bad vlan → 2; duplicate → 1.
9. `op16_delete_roundtrip` — add (at showtime) then delete with the fixed
   3-byte payload → slot freed; shifted-echo payload `10 07d0 03` → status 2
   (documents the session-2 bug as a permanent regression fixture).
10. `op2_response_parsing` — sim-built 63-B status parses via
    `parse_line_obj` (field offsets round-trip; no echo on RX either).

### 3.2 Wire-level tests (`rbctl_dsl::board` — MockTransport rewrite)

Replace `default_response_payload`'s fabricated echo bytes with `BoardSim`
outputs:

- `MockTransport` holds a `BoardSim`; `line_config_up` TX frame is fed to
  `sim.op1`, the real status byte comes back — asserts the *frame* (CRC,
  payload_len, no echo) not just the API result.
- `last_tx` inspection tests: op1 frame `payload_len == 12`; op2/3/4
  `payload_len == 0`; op16 payload is exactly `07 d0 03`.
- Golden fixtures: session-2 wire payloads (`01060101 010000 01ff…`,
  `1007d003`, `0f00000000000007d0`) as hex constants asserted **rejected /
  misdecoded** by the sim — the bug stays reproducible forever.

### 3.3 Daemon-flow tests (`rbctl_dsl::daemon`)

- startup sequence emits exactly `BringLineUp` (no `RetrainLine`); after
  completion exactly 2 op1 frames hit the transport.
- watchdog: NoSignal 30 s → log only; 5 min → one `BringLineUp`; Initializing
  resets dwell (fake clock injection, `tokio::time`-style or manual
  `Instant` override as the existing tests do).
- shutdown still sends exactly one op3 and logs "closing DSL line".

### 3.4 On-target validation (manual, tester-run)

1. Power-cycle the board with the daemon stopped → line must train alone
   (validates line/DSLAM, no code needed).
2. Start daemon → expect in logs: version banner, `line_config_up` with the
   *intended* fields, NoSignal → Initializing/EstablishingLink within ~10 s.
3. `rbctl-dsl status` during Initializing shows `null` trained fields
   (0.3.2 behavior preserved).
4. Stop/start cycle ×3 → line reopens every time (double apply vs cache).
5. Leave daemon running 10 min on an unplugged line → exactly one re-apply
   burst at ~5 min in the log, no op3 storms (verify via pcap).

### 3.5 E2E against the real board binary under QEMU (host, automated)

The strongest possible host-side validation: run the **actual MIPS
`remote_board`** under `qemu-mips` user-mode emulation against a native
(x86-64 debug) build of the daemon, bridged by a veth pair — the mock and the
sim are replaced by the binary whose logic we reverse-engineered.

Feasibility (verified): `remote_board.board` is ELF32 MSB dynamically linked
against **uClibc** (`PT_INTERP = /lib/ld-uClibc.so.0`) and the complete
a complete sysroot exists in the extracted OEM rootfs (`ECONET_ROOTFS`, a
private artifact) — `qemu-mips -L $ECONET_ROOTFS` resolves it. The binary is pure userspace over AF_PACKET + `/proc` + `system()`
— all of which work or can be faked under qemu-user without modifying it.

#### Harness layout (`tests/e2e/` in `rbctl_dsl`, feature `e2e`, `#[ignore]`-tagged)

```
┌───────────────── user+net+mount namespace (unshare -rnm) ─────────────────┐
│                                                                            │
│  veth: eth0.1.500 ◄──── 0x88B5 / EtherType 88B5 ────► lan0.500            │
│   │ (exact iface name the                          │                      │
│   │  binary binds to)                              ▼                      │
│  qemu-mips -L econet_rootfs                rbctl-dsl daemon (native)       │
│  ./remote_board                            + tcpdump -i lan0.500 -w e2e.pcap│
│   │                                                                        │
│   ├─ stderr → board.log (freopen'd console output)                         │
│   ├─ /proc  ← overmounted tmpfs with fake driver files (below)             │
│   └─ system()/brctl/vconfig/ifconfig → fail harmlessly (tolerated by       │
│      msg_init; the veth already exists) — recorded via optional shim       │
└────────────────────────────────────────────────────────────────────────────┘
```

Fake `/proc` (mount `tmpfs` over `/proc` inside the mount namespace — the
binary reads only these paths):

| Path | Role | Driven by |
|---|---|---|
| `/proc/tc3162/tcci_cmd` | **the observable**: every `wan adsl open/close/reset`, `ghs set annex …`, `olr` write lands here; `fopen("w")`-truncate semantics make each command a fresh file content — watch with inotify and append to `tcci.log` | board writes, harness reads |
| `/proc/tc3162/adsl_stats` | synthetic line state — harness writes NoSignal / Initializing / full showtime (rates, Opmode, ANNEX_x) per `proc-formats.md`; the board's 2 s status thread + op2 parse the **real parsing code** | harness writes |
| `/proc/tc3162/vdsl_interface_config` | `CurrentProfiles:` for VDSL2 status | harness writes |
| `/proc/modules` | with/without `mt7510sar` to steer the ATM/PTM flag in op2 | harness writes |

`tcci.log` is the ground-truth pipe for open/close/dedup: it *proves* whether
an op1 executed the setter path (annex/sra writes + `wan adsl open`) or was
deduplicated (silence).

#### Scenarios (each = script + golden assertions; map 1:1 to §3.4's manual matrix)

| # | Scenario | Assertions |
|---|---|---|
| E1 | boot board alone | `tcci.log` ends with `wan adsl open`; boot apply wrote the default-config setter commands (cache starts zeroed) |
| E2 | start daemon (fresh board) | two op1 acks = 0; `tcci.log` shows exactly one full setter burst ending `wan adsl open` (first apply may dedup if boot default == intended — flip-byte still forces one); pcap: op1 payload_len == 12, **no echo byte** |
| E3 | daemon restart, same config (cache warm from run 1) | line reopens — `wan adsl open` present in run 2's tcci.log despite identical intended config (the session-1 killer, now structural) |
| E4 | daemon shutdown | exactly one op3; `tcci.log` gains `wan adsl close`; subsequent polls show NoSignal |
| E5 | line-Up simulation | harness writes showtime adsl_stats → board status thread flips → daemon sees Up → op15 materializes on the board (ack 0 after earlier queued ack 1); `status` shows real parsed rates/SNR from our synthetic file (validates the 63-B decode E2E) |
| E6 | op16 delete | ack 0 (was 2 on the wire in session 2); re-add then delete round-trips |
| E7 | watchdog | harness holds NoSignal: no tcci activity for 4 min (auto-retrain does it — simulate by cycling Initializing), then one `wan adsl open` burst at the 5-min re-apply; never `wan adsl close` from the daemon |
| E8 | regression fixtures | replay session-2 wire payloads (op1 `01 06 01…`, op16 `10 07d0 03`) through a raw socket: board must decode garbage (E2-style assertion *inverted*) — permanent proof of what the bug looked like |

#### Optional level 2: shell-command recorder

Build a tiny MIPS BE `LD_PRELOAD` shim (`tests/e2e/shim.c`, cross-compiled
with the SDK `toolchain-mips_24kc` gcc, `-shared`, interposing `system` →
log + return 0) so `smuxctl`/`ifconfig nas8 …` from `vdsl_linkAdd/Del` are
recorded instead of failing. Not required for E1–E8 (protocol assertions
don't depend on shell success) but documents board-side provisioning.

#### CI constraints

- Needs `qemu-mips` (qemu-user package) and a runner allowing user + network
  namespaces (`unshare -rnm`); rootless containers qualify when
  `kernel.unprivileged_userns_clone` is on. If the CI runner forbids netns,
  the same scripts run on any dev machine — keep them out of the default
  `cargo test` (`--features e2e` / `#[ignore]`).
- The daemon side is a **native host build** (`cargo build`, x86-64 debug) —
  same code as the aarch64 target; AF_PACKET, rtnetlink, and the frame layer
  are architecture-independent. The aarch64 artifact keeps being validated by
  §3.4 on-target.

## 4. Implementation order

| Step | Content | Tests |
|------|---------|-------|
| 1 | `board_sim` module in `rbctl_proto` (rules table above) | §3.1 self-consistency |
| 2 | Echo-byte removal in `board.rs` TX + `strip_echo` deletion | §3.2 wire tests |
| 3 | `BringLineUp` double apply in `worker.rs`, startup & watchdog rework in `daemon/mod.rs` | §3.3 |
| 4 | `version.rs` + `build.rs` + banner in `main.rs`/selftest | banner unit test |
| 5 | CLI fatal-on-bad-explicit-value + case-insensitive parse | config.rs tests |
| 6 | `constants.rs`: `NOSIGNAL_REAPPLY`, remove dead retrain consts | — |
| 7 | E2E harness §3.5: namespace/veth setup script, fake-`/proc` overlay, `tcci.log` watcher, scenarios E1–E8, optional MIPS `system()` shim | `cargo test --features e2e` |
| 8 | Full clean SDK build (`rm -rf build_dir/target-…/rbctl-dsl-0.3.X` first), `strings` check for banner + absence of `strip_echo` artifacts, `cargo check --tests` on host with staging env | §3.4 handoff to tester |
| 9 | Documentation corrections per §5.2/§5.3 (board-side research docs, source comments, `docs/rbctl-dsl.md` mermaid, `CHANGELOG.md` 0.4.0) | review |

Version bump: 0.4.0 (wire format is a breaking fix); `PKG_RELEASE` stays 1
until released. Documentation updates are tracked in §5 (new §5.1 entries are
already applied as uncommitted edits in the research tree).

## 5. Documentation corrections

The session-2 analysis showed several **board-side docs were wrong** and one
host-side misconception was documented as fact — these docs are the reference
for every future change and must be fixed together with the code.

### 5.1 Already corrected this session (research tree — private, not in this repo)

| File | Correction |
|---|---|
| `dsl-config.md` (research tree) | modulation table re-verified against the binary (`0x429034`, 0-based codes; old table was shifted by one); Reset section rewritten: op1-finalize = `wan adsl open`, op3 = `wan adsl close`, `wan adsl reset` only inside annex/sra setters of a changed apply |
| `opcodes.md` (research tree) | op3 dispatch row: "DSL retrain / config-apply" → "DSL line **close**" |
| `led-mac-misc.md` (research tree) | op3 section rewritten with the `FUN_00404298` → `"wan adsl close"` (string `@0x416a34`) disassembly proof |

### 5.2 Still to fix — board-side research docs (private research tree, not in this repo)

| File | Correction |
|---|---|
| `dsl-config.md` | mermaid flow labels the finalize step `R[reset FUN_004043a4]` — it is the *open* step (`wan adsl open`), rename node; "Effect" paragraph says "Setting config causes a DSL retrain" → rewrite: *changed* config ends in `wan adsl open` (starts training); identical config is a deduplicated no-op |
| `lifecycle.md` | `msg_init` section: add that the boot-time `dslCfgSet(&default_cfg)` runs against a **zeroed** `l_lastDslCfg`, so it always takes the full path and ends in `wan adsl open` — the line trains by itself at board boot (the fact the whole session-2 diagnosis turned on) |
| `transport.md` | add a caution note at the frame-layout table: the request payload starts at byte 0 — there is **no** opcode echo inside the payload (the exact misconception that broke session 2; handlers read from `+0x602` = wire `0x18`) |
| sweep (grep the research-tree docs for "retrain / wan adsl reset / echo") | catch remaining stale claims in `architecture.md`, `index.md`, `dsl-status.md`, `status-thread.md`, `driver-control.md`, `plans/protobuf.md` (its `line_retrain` mapping note); fix or annotate each hit |
| `dsl-status.md` | (review, low priority) field-order table at `+0x08..0x2c` vs our `LineMetrics` struct — the DS/US ordering was never confirmed against a live capture; mark as unverified rather than asserting |

### 5.3 Still to fix — host-side docs and source comments (repo)

| File | Correction |
|---|---|
| `rbctl_dsl/src/board.rs` | `line_retrain` doc comment claims "force a DSL retrain (`wan adsl reset`) … the only reliable way to revive a reset line" — rewrite to *close the line* (`wan adsl close`), revive = changed-config op1; the `strip_echo` doc-comment block ("The board inconsistently echoes the opcode…") is deleted with the function — it documented the bug as board behavior |
| `rbctl_dsl/src/daemon/worker.rs` | `RetrainLine` / `RestartLine` comment blocks repeating the retrain claim; update to the close/double-apply semantics |
| `rbctl_dsl/src/daemon/mod.rs` | watchdog doc comment ("recovers zombie states… op3 retrain kick") and the startup comment block explaining the op1+op3 pairing rationale (now obsolete) — rewrite for §2.2/§2.3 behavior |
| `docs/rbctl-dsl.md` | bring-up mermaid sequence: remove the startup op3, show the double op1 (toggle-bitswap) with the cache-dedup annotation; document the version banner log line; note watchdog 30 s detect / 5 min re-apply behavior |
| `CHANGELOG.md` | 0.4.0 entry (echo-byte removal, close-semantics fix, double apply, watchdog rework, version banner, CLI fatal-on-bad-value) |
| `examples/unpack.py` (if it still carries the echo-byte parsing) | align with the corrected protocol or annotate as historical; it is the file the wrong `strip_echo` comments cite as origin |

### 5.4 Rule going forward

Any doc statement about board behavior must cite its evidence (address in
`remote_board.board`, disassembled function, or pcap timestamp) — the two doc
bugs that survived (op3 semantics, 1-based modulation table) both lacked
citations and were never re-verified. The corrected files already carry
"Verified YYYY-MM-DD against …" notes; keep that convention.

## 6. Risks / open questions

- **qemu-user fidelity gaps**: no real DSL driver, so `adsl_stats` content is
  synthetic and `system()` side effects fail (tolerated by the binary's
  ignored return codes — verified in `msg_init`/`vdsl_linkAdd` disassembly).
  The E2E validates protocol + state-machine logic, not RF/DSLAM behavior.
  If a future fix depends on driver-internal behavior, escalate to
  `qemu-system-mips` + a fake-proc kernel module (stretch goal, not in
  scope).

- **Empty-payload op2/op3/op4**: the board's recv path requires
  `n ≥ payload_len + 0x18` — a 24-byte frame passes; CRC over header-only is
  well-defined (payload_len + 10 bytes). Low risk, but step-2 on-target smoke
  (§3.4.2) must confirm before the tester matrix.
- If any *other* host tool (OEM `libcmm`-based) ever talks to the same board,
  the shared `g_lastSeq` dedup could swallow our first frame after theirs —
  mitigated by the existing retry logic; not new to this change.
- The `lan0.2000` ENXIO host VLAN failure (FINDINGS §secondary-3) is
  **excluded** from this plan — separate investigation, does not affect
  training.
