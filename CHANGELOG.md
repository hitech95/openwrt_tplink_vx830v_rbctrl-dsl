# Changelog

Public-facing changes to the `rbctl-dsl` OpenWrt package. Dates are in
YYYY-MM-DD format. Internal refactors and documentation-only changes are
summarized, not itemized.

## 0.5.0 — 2026-08-27

### Added — data-link configuration (multi-link, board-side tagging)

- **UCI data-link sections** (`network.<name>=ptm|atm`): one named
  section per data link with a stable explicit `index` (transport VLAN =
  `2000 + index`, immune to UCI reordering — netifd hooks
  `lan0.<transport>` by name), `vid`/`pbit` service-VLAN tagging applied
  by the BOARD (op15 tag fields → smux mode 3), and ATM PVC params
  (`vpi`, `vci`, `encaps`, `payload`). Validation enforces the board's
  physical limits at load with named errors: indices 0–7 unique, ≤ 8
  links, service vids unique (board vid-dedup), at most one untagged
  PTM link, `ptp` rejected as the IEEE-1588 typo of `ptm`. No sections =
  the implicit legacy single link. CLI `--link-index/--link-vid/
  --link-pbit` override the implicit link.
- **Queue-at-open provisioning** (the session-4 tester-verified OEM
  flow): all links are op5/op15-queued while the line is still DOWN and
  materialized by the board's up-transition handler. Shutdown/reload
  tear down per-link (op16/op6 for both types — covers queued-not-yet-
  materialized nodes); reload diffs links by index and swaps changed
  ones. Post-up provisioning (reload adds, auto corrections) logs the
  F3 warning naming the `restart-line` remedy — the econet direct path
  is real-hardware-verified but historically fragile.
- **Auto-mode type correction (F4)**: with `line_mode auto` and no link
  sections, an optimistic untagged PTM link is queued at open; if the
  line trains ADSL the daemon swaps it to ATM post-up (op16 the queued
  PTM node + op5 with default PVC 8/35), and re-swaps if a mixed-mode
  line re-trains as the other family. Explicitly typed links are never
  silently retyped — they keep the physical-compatibility error.

### Fixed

- The 0.4.2-r2 TEST-DIRTY hardcodes (worker tag=100, forced queueing)
  are superseded by the config-driven flow above; `PKG_RELEASE` is 1.

### E2E harness (tests/e2e)

- Fake `/proc/tc3162/xdsl_stats` now uses the real `tccicmd.ko` colon
  format (`"1:2"` etc.); `/proc/modules` swaps with the line family
  (mt7510ptm/mt7510sar). The harness pre-creates the 24 interfaces the
  board's `ptmCreateVLAN` polls for, so materialization, the
  handled-flag teardown gate and the vid-dedup re-add cycle all
  exercise the real board logic.
- New scenarios: ED (two-link daemon config end-to-end), EX (both
  materialization paths + smux mode 1/3 semantics), EA (auto-mode
  ADSL swap), E9b (7 invalid data-link configs). E2 flipped: link-adds
  are REQUIRED at open, while the line is down.

## 0.4.2 — 2026-08-24

### Fixed

- **Data-link provisioning keyed off the wrong source.** The op2
  `data_path` field is a power-on default that does NOT follow training
  (read `Atm` while a VDSL2 line was in showtime), and the daemon
  provisioned its configured link type while the line was still down —
  before the actual connection type was knowable. Now: an explicit
  `xfer_mode` is honored as an operator override even when unusual
  (ATM on VDSL2 is spec-legal); the default/auto resolution derives
  PTM (VDSL) / ATM (ADSL) from the **trained** modulation, and
  provisioning runs only once the line reaches showtime. Only
  physically impossible combinations are rejected — PTM on ADSL
  standards is blocked (at config load for fixed modes, at line-up for
  a Multimode line that trained ADSL) with an error; future
  PTM-only/ATM-only modes (e.g. G.fast) slot into the same rules
  table. A stale op2 `data_path` field disagreeing with the trained
  modulation is logged in auto mode.
- **Stale data links are no longer left on the board.** `selftest`
  provisioned a data link for its link-op test and never removed it;
  a follow-up daemon then stacked a second (differently-typed) link
  next to it — the likely cause of the one-way (TX-only) data path
  observed on-target. `selftest` now tears its link down before
  exiting, and the daemon clears any leftover link of either type on
  its transport VLAN at startup (crashed previous daemon, selftest
  residue), so exactly one correctly-typed link exists at any time.

## 0.4.1 — 2026-08-24

### Fixed

- **Line metrics DS/US pairs were crossed or mislabelled.** The op2
  status decoder mapped five field pairs wrong: SNR margin, attenuation
  and output power had downstream/upstream swapped or exchanged with
  each other, the "attainable rate" pair was actually output power (and
  vice versa), and the "CRC errors" pair is really the showtime/total
  uptime counters. Verified empirically against the real board firmware
  (direction-distinct values injected into the stats file and read back
  from live op2 replies). Field reports of `down_snr: 0 / up_snr: 55`
  now decode correctly as downstream 5.5 dB, upstream 0.0 dB.
- `status` now prints SNR in dB with one decimal (`down_snr: 5.5`), the
  modulation name (`modulation: VDSL2`) and the human line-state label.
  The IPC status reply is fully typed — enums cross the wire as stable
  identifier tokens parsed back into typed values (`state:
  ShowtimeTcSync`, `modulation: Vdsl2`, `annex: B`, `xfer_mode: Ptm`,
  `fw_status: UpgradingProgress stage=2 pct=45`; unknown states keep
  their code, e.g. `Unknown(9)`) and measurements as integers with
  fixed units (uptime in seconds, rates in kbps, SNR in dB×10). No
  pre-formatted strings on the wire: machine consumers parse the
  tokens/numbers directly and future output formats/units are CLI-only
  changes. The line-state vocabulary now lives in the protocol crate as
  one shared enum (`LineState`: board states plus the LuCI lifecycle
  states `NotInitialized`/`Idle`/`ShowtimeNoSync`), so IPC and ubus
  report the same states; the redundant `state_num` field was dropped
  from the IPC reply (the Lantiq numeric ABI remains on the ubus
  interface, where it belongs).
- The `VLAN create … (os error 6)` warning on restart was a false
  alarm: libnl-tiny returns its own error codes, not errnos, and code 6
  (`NLE_EXIST`) was displayed as `ENXIO`. Existing-interface (EEXIST)
  is now correctly recognized as the idempotent success path.
- `-t/--transport-vlan` accepts the full VLAN id (`2000`–`2007`) and
  the interface form (`lan0.2000`) in addition to the base index 0–7.

### Testing

- The E2E suite's fake driver stats file now matches the board parser's
  needle-strict format (single space after `Opmode:`, the firmware's
  `ativetime` spelling), which unblocked end-to-end metric verification:
  E5 now asserts the decoded rates/SNR of a showtime injection against
  the real board binary.

## 0.4.0 — 2026-08-23

### Fixed

- **Line configuration now reaches the modem as intended.** Every write
  request (opcodes 1, 5, 6, 15, 16) carried a spurious leading byte — a
  long-standing misreading of the OEM wire format — which shifted every
  field by one. The board accepted the garbage as a *valid but wrong*
  config: the daemon's "VDSL2 Annex B" was applied as **G.DMT Annex J**,
  which no European DSLAM answers. This is the root cause of the
  "line never trains" reports from both field-test rounds. Request
  payloads are now the bare descriptors exactly as the board firmware
  parses them; link deletes (op6/op16), previously always rejected by the
  board, now work.

- **Bring-up no longer closes the line it just opened.** Firmware
  analysis proved opcode 3 is `wan adsl close` — a deliberate line-down,
  not the retrain the old code assumed. Startup and the watchdog used to
  send op1 (open) followed by op3 (close) milliseconds later, keeping the
  line down permanently. op3 now only appears at shutdown ("daemon
  stopped ⇒ line down") and in explicit line bounces.

- **Daemon restart reliably re-opens the line.** The board deduplicates a
  config identical to its internal cache (and the cache cannot be read
  over the wire). Startup's line-open command applies the intended
  config with bitswap flipped, then the intended config: the second
  apply can never match the cache, so it always executes the full board
  path ending in `wan adsl open`. "Daemon start ⇒ line trains" is now
  deterministic for any prior board state. Config reloads (where the
  config actually changed) still use a single apply.

- **Invalid CLI/UCI values are fatal instead of silently ignored.**
  A mistyped option used to log a warning and substitute defaults — which
  voided an entire field-test matrix (`--line-mode=Vdsl2` and
  `--tone=auto` were rejected case-sensitively, so 9 of 11 runs tested
  nothing). Values are now case-insensitive with aliases
  (`vdsl2`, `adsl2+`, `auto`, `17a,30a`, …), and a bad explicit value
  exits with a clear error naming the option and the valid values.

### Changed

- **NoSignal watchdog is warn-only.** An open modem retries training on
  its own; the old 30 s op3 kick actively closed it again every 30 s.
  The watchdog now only logs a diagnostic after 30 s of continuous
  `NoSignal` (pointing at `rbctl-dsl restart-line` for a manual bounce)
  and never touches the line. Board-recovery re-opens are handled by the
  reachability path; training states never trigger it.

- **`--help` documents every value.** All daemon flags list their valid
  values (`--line-mode`, `--annex`, `--tone`, `--xfer-mode`, `--encaps`,
  `--payload`, `--transport-vlan`, `--bitswap`/`--sra`), `--notify`
  explains it takes a path to an executable shell script, and every
  subcommand carries a proper description.

### Added

- **Version banner at startup.** The daemon and selftest log
  `rbctl-dsl <version> (git <sha>, built <date>)` as their first line, so
  field captures can be mapped to an exact source tree.

- **Board-firmware simulator in the test suite** (`rbctl_proto`
  `board_sim`): the decompiled opcode handlers (config cache, open/close,
  link queueing and validation) ported to Rust, with the field-failure
  wire payloads kept as permanent regression fixtures.

- **Host end-to-end test suite** (`tests/e2e/`): the **real MIPS board
  firmware** runs under qemu-user against the **real daemon binary** over
  a virtual Ethernet pair in a namespace — no board hardware needed.
  Scenarios assert on the wire frames, the board's actual chipset-command
  trace (via strace), and daemon logs: boot self-open, cache-safe
  bring-up, restart-after-close, injected showtime with link
  materialization, teardown, the 5-minute watchdog soak, and a replay of
  the field-failure wire payloads (kept as permanent fixtures proving the
  bug's exact on-wire signature). Requires only `qemu-user` + `strace`;
  the firmware image and SDK paths are supplied via env vars
  (`docs/e2e.md` documents setup).

## 0.3.3 — 2026-08-19

### Fixed

- **Second daemon instance no longer disrupts a running one.** Starting
  `rbctl-dsl daemon` while another instance is alive now fails fast with
  `singleton lock: another rbctl-dsl instance is running (pid N?)` and
  exit code 1, before touching the board, ubus, or the control socket.
  Previously a second instance silently stole the IPC socket, failed to
  register the ubus `dsl` object (`HELLO/ADD_OBJECT failed: Status(2)`),
  and double-configured the DSL line — a likely cause of intermittent
  "works after boot, dead after restart" reports. The guard is a kernel
  file lock (`/var/run/rbctl-dsl.lock`) that is released automatically on
  any exit, including crashes and `SIGKILL`; no stale-lock recovery is
  ever needed.

## 0.3.2 — 2026-08-19

### Fixed

- **Daemon restart leaving the line permanently down.** On shutdown the
  daemon deliberately resets the DSL line (opcode 3, `wan adsl reset`);
  on restart it re-sent the same configuration (opcode 1), which the
  board's config-cache deduplicates — no retrain was ever triggered, so
  the line sat in `Silent` forever. This was fully reproducible with
  `--line-mode auto`, whose configuration is byte-identical to the
  board's boot defaults. Startup now always pairs the config apply with
  an explicit retrain kick: starting the daemon deterministically trains
  the line, and stopping it deterministically leaves it down.

- **Status no longer reports phantom line data.** While the line is
  untrained, the board's status reply carries driver defaults
  (`modulation: 0`, `annex: A`). `rbctl-dsl status`, the IPC snapshot,
  and ubus `metrics` now report `null` for every training-dependent
  field (modulation, annex, uptime, rates, SNR margins) until the line
  actually reaches `Up`.

- **Link provisioning failures are now detected.** The board's short
  acknowledgement for the link-add opcodes (5 ATM / 15 PTM) is decoded:
  status 0 = applied, 1 = queued or idempotent re-add (the board
  materializes it at showtime), 2 or higher = rejected. A rejected link
  no longer creates the host-side transport VLAN, and the daemon's
  deferred retry stays armed.

- **Duplicate startup commands.** The initial apply no longer sends the
  line config and link provisioning twice when the board is slow to
  answer; provisioning follows from the board's acknowledgement rather
  than racing it.

### Added

- **NoSignal watchdog.** With configuration applied and the board
  reachable, a line stuck in `NoSignal` for 30 seconds gets an automatic
  retrain kick every 30 seconds. Training and handshake states never
  trigger it. This also self-heels lines left dead by earlier daemon
  versions.

- **Board-side provisioning is now documented** in
  [docs/rbctl-dsl.md](docs/rbctl-dsl.md), including the bring-up
  sequence diagram and why link provisioning is queued on the board
  until showtime.

### Restored

- The per-opcode transmit log lines lost in the 0.3.1→0.3.2 daemon
  restructure (`line_config_up: …`, `ptm_link_add: …`,
  `atm_link_add: …`). They now also cover deferred applies and reloads,
  not just startup.

## 0.3.1 → 0.3.2 (internal)

Between the public releases above, the daemon internals were
restructured (no behavior change on their own): the daemon became the
single policy gate (`daemon/mod.rs`), the board worker a poll-only
executor, IPC/ubus/hotplug/firmware moved into the `daemon/` directory,
selftest/sniff moved into `diag/`, and dead board-side teardown code was
removed. Firmware flashing gained a shutdown guard (exit is deferred
until an in-flight upload completes).
