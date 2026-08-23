# Changelog

Public-facing changes to the `rbctl-dsl` OpenWrt package. Dates are in
YYYY-MM-DD format. Internal refactors and documentation-only changes are
summarized, not itemized.

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
