# Plan — File-structure refactor v3: daemon-centric tree

> Goal: every file's responsibility identifiable from its path, using three
> placement rules agreed in review:
> 1. Everything the daemon owns — control surfaces, its worker agent, its
>    state blackboard, its ubus publisher — lives **under `daemon/`**.
>    No separate `control/` or `status/` directories.
> 2. **Single-file modules stay flat** — no directory for one file.
> 3. Helpers that use **only common/external types** go to **`utils/`**,
>    kept generic for future expansion.

---

## 1. Placement rules (locked)

**Rule A — `daemon/` owns its whole runtime sphere.** The worker is the
daemon's agent ("only polls and notifies back"); `DslState` is the
daemon↔worker blackboard; the ubus object, IPC socket, hotplug, and
firmware streaming are daemon surfaces. All under `daemon/`.

**Rule B — flat single file** when a concern is one file.

**Rule C — `utils/` admission test**: no specialized types from other
crate modules; external-crate types and self-defined types are fine.

| Candidate | Depends on | Verdict |
|---|---|---|
| `mapping.rs` | `rbctl_proto` enums, `u8` | ✅ utils |
| `vlan.rs` | `tinyln_rs::RtnlLink`, libc | ✅ utils |
| `mgmt_link.rs` | sysfs, libc | ✅ utils |
| `ubus.rs` (conn) | `ubus` crate + transport | ✅ utils |
| `ubus_transport.rs` | `ubus` crate only | ✅ utils |
| `DslState` model | consumed by daemon+worker | ✗ → `daemon/state.rs` |
| ubus object builder | `DslState` ❌ | ✗ → `daemon/ubus_object.rs` |
| atm_params fn | `config::AtmConfig` ❌ | ✗ → `config.rs` |
| snapshot builder | `DslState` + Daemon fields ❌ | ✗ → `daemon/snapshot.rs` |

### Resulting layering (acyclic, daemon-centric)

```
main.rs ──► daemon::run, cli (IPC client), diag::*, board (selftest)
cli.rs  ──► daemon::ipc (protocol types only)
daemon/* ──► board.rs, config.rs, utils/*, constants
diag/*   ──► board.rs, utils/*
board.rs ──► rbctl_proto, af_packet          (below everything)
utils/*  ──► external crates only
```

`board_worker.rs` moving into `daemon/worker.rs` removes the awkward
board→status dependency of v2: the worker (daemon sphere) uses Board
(downward) and state (same sphere).

---

## 2. Target tree

```
src/
├── main.rs               CLI dispatch only
├── constants.rs          shared constants (prefixed groups)
├── logging.rs            stderr/syslog init                     (was log_init.rs)
├── cli.rs                IPC client half: send_command,          (was ipc.rs, client half)
│                         send_firmware — used by main.rs for
│                         status/reload/restart/stop/firmware
├── config.rs             UCI+CLI → DslConfig, parsers,           (was uci_cfg.rs)
│                         + atm_params() (from utils/xdsl.rs)
├── board.rs              Board<T>: 0x88B5 opcode methods,        (was board.rs — stays flat:
│                         request/retry, firmware upload           single file per Rule B)
│
├── daemon/               ── the daemon and everything it owns
│   ├── mod.rs            Daemon struct, run loop, event handlers   (the gate)
│                         + signal flags & handlers (SHOULD_* statics,
│                         install()) — inlined from signals.rs
│   ├── init.rs           startup phase (config, validation, socket)
│   ├── worker.rs         BoardWorker: op2 poll + WorkerCmd exec  (was board_worker.rs)
│   ├── state.rs          DslState / SharedState / FwStatus       (was ubus_obj.rs, state half)
│   ├── ubus_object.rs    ubus 'dsl' object: metrics/statistics    (was ubus_obj.rs, object half)
│   ├── snapshot.rs       build_snapshot() → StatusSnapshot        (extracted from mod.rs)
│   ├── ipc.rs            IPC server + protocol: IpcListener,      (was ipc.rs, server half;
│                         IpcAction, wire commands, StatusSnapshot  protocol is daemon-owned)
│   ├── hotplug.rs        DSL notify-script emission              (was hotplug.rs)
│   └── firmware.rs       FIRMWARE <path> IPC streaming
│
├── diag/                 ── diagnostic subcommands
│   ├── mod.rs
│   ├── selftest.rs       selftest orchestration + board probe     (was selftest.rs, half)
│   ├── sniff.rs          passive 0x88B5/0x88B6 listener + hexdump (was selftest.rs, half)
│   └── pcap.rs           pcap writer                             (was pcap.rs)
│
└── utils/                ── generic, dependency-light helpers
    ├── mod.rs
    ├── mapping.rs        protocol enums → LuCI/IPC strings, lstate ABI
    ├── vlan.rs           VLAN netdev create/delete/exists         (was utils/xdsl.rs)
    ├── mgmt_link.rs      read-only sysfs interface probe          (was mgmt_link.rs)
    ├── ubus.rs           socket discovery + connect_and_register
    └── ubus_transport.rs AF_UNIX ubus-zero adapter                 (was transport.rs)
```

Notes:
- `daemon/` is large (9 files) but each file is one responsibility and
  the subsystem is the orchestrator — it *should* own its surfaces.
- **`signals.rs` is inlined into `daemon/mod.rs`**: the `SHOULD_*`
  statics and handlers are used only by the gate loop (and one read by
  `worker.rs`, which imports them from `daemon` anyway) — 35 lines not
  worth their own file. `mod.rs` gains a small `signals:` section
  (statics + `install()`), keeping the loop code that consumes them
  adjacent.
- **IPC is split by role**: the daemon owns the protocol + server
  (`daemon/ipc.rs`); the CLI owns the client (`cli.rs`, flat per Rule B,
  used only by `main.rs`). Dependency direction `cli.rs → daemon::ipc`
  (protocol types: `IpcCmd`, `StatusSnapshot`) matches `main.rs →
  daemon::run` — the client speaks the daemon's protocol, never the
  reverse.
- `board/` from v2 disappears: with the worker reassigned to `daemon/`,
  only `controller.rs` would remain → flat `board.rs` per Rule B.
- `utils/xdsl.rs` disappears: VLAN half → `utils/vlan.rs`, ATM half →
  `config.rs` (beside the `AtmConfig` type it consumes).

### Validity notes

- **`daemon/ipc.rs` (server + protocol) and `cli.rs` (client)**: the wire
  protocol (`IpcCmd` values, `StatusSnapshot` rendering) is defined by the
  daemon — the side that serves it — so the types live in `daemon/ipc.rs`
  and `cli.rs` imports them, exactly as `main.rs` imports `daemon::run`.
  No daemon → client dependency ever exists.
- **`daemon/worker.rs`**: the worker's lifecycle, commands, and events
  are daemon-defined; it merely *uses* `board.rs` downward. Matches the
  established "daemon is the gate" architecture. Its single signal
  dependency (`SHOULD_EXIT`, read to stop polling during shutdown)
  imports from `daemon` — its own subsystem.
- **`daemon/state.rs` as shared blackboard**: worker writes, daemon and
  ubus_object read — all within one subsystem, no cross-subsystem
  coupling.

---

## 3. Migration mapping

| Old | New | Kind |
|---|---|---|
| `board.rs` | `board.rs` (unchanged path) | none |
| `board_worker.rs` | `daemon/worker.rs` | move |
| `daemon/signals.rs` (SHOULD_*, install) | inlined into `daemon/mod.rs` | merge+delete |
| `ipc.rs` (server + protocol: IpcListener, IpcAction, handle_client, IpcCmd, StatusSnapshot) | `daemon/ipc.rs` | split |
| `ipc.rs` (client: send_command, send_firmware, daemon_running, IpcCmd::from_arg) | `cli.rs` | split |
| `hotplug.rs` | `daemon/hotplug.rs` | move |
| `daemon/mod.rs` (snapshot fn) | `daemon/snapshot.rs` | extract |
| `ubus_obj.rs` (state types) | `daemon/state.rs` | split |
| `ubus_obj.rs` (object+methods) | `daemon/ubus_object.rs` | split |
| `uci_cfg.rs` | `config.rs` | move+rename |
| `utils/xdsl.rs` (vlan fns) | `utils/vlan.rs` | split |
| `utils/xdsl.rs` (atm_params) | `config.rs` | split+merge |
| `mgmt_link.rs` | `utils/mgmt_link.rs` | move |
| `transport.rs` | `utils/ubus_transport.rs` | move+rename |
| `selftest.rs` (Selftest) | `diag/selftest.rs` | split |
| `selftest.rs` (sniff) | `diag/sniff.rs` | split |
| `pcap.rs` | `diag/pcap.rs` | move |
| `log_init.rs` | `logging.rs` | move+rename |
| `main.rs`, `constants.rs`, `utils/{mod,mapping,ubus}.rs` | unchanged | import paths only |

---

## 4. Sequencing (one commit each, SDK-build gated)

| # | Commit | Content |
|---|---|---|
| 0 | *(in-flight WIP)* | Daemon struct + flash guard + VLAN ownership + vlan-from-daemon + utils/ubus.rs — verify build, commit |
| 1 | utils consolidation | transport→`utils/ubus_transport`, mgmt_link→`utils/`, xdsl→`utils/vlan` + atm_params→uci_cfg, delete `utils/xdsl.rs` |
| 2 | root renames | `uci_cfg.rs`→`config.rs`, `log_init.rs`→`logging.rs` |
| 3 | daemon additions | ipc+hotplug move in; **ipc server/client split** (`cli.rs`); board_worker→`daemon/worker.rs`; signals.rs inlined into mod.rs; snapshot.rs extract |
| 4 | daemon/state + ubus split | ubus_obj → `daemon/state.rs` + `daemon/ubus_object.rs` |
| 5 | diag/ | selftest/sniff split + pcap move |
| 6 | functional cleanup | §5 items 1–2 (behavioral — kept out of pure moves) |
| 7 | docs | module table + diagrams in `docs/rbctl-dsl.md` |

---

## 5. Additional refactor/cleanups found during analysis

1. **Board-side link teardown gap (functional).** `Board::atm_link_del` /
   `ptm_link_del` (op6/op16) are **never called**: on shutdown/reload we
   delete the host VLAN but never tell the board. Add
   `WorkerCmd::RemoveLink { xfer, vlan }`, sent by the daemon before
   `LineDown` when `link_provisioned` — symmetric deprovisioning and it
   removes the dead-code warnings on those methods.
2. **Dead code to resolve** (build warnings): `ipc::daemon_running`,
   `IpcCmd::from_arg`, `StatusSnapshot::from_lines`, `Board::line_status`,
   `lstate::{IDLE, SHOWTIME_NO_SYNC}`, `BOARD_LINK_DEL_TYPE_DELETE`
   (becomes used once item 1 lands). Use or remove; keep ABI consts with
   a documented `#[allow(dead_code)]` if retained for completeness.
3. **Test-only imports in shipped binary**: `set_checksum`,
   `MAGIC_OEM_COMMAND` etc. imported at module top but used only in
   `#[cfg(test)]` — move into the test module.
4. **`selftest::ifindex` duplicates `utils/mgmt_link::iface_exists`** —
   dedupe (selftest already delegates carrier reads).
5. **Worker `ProvisionLink { XferMode::Auto }` arm** is now unreachable
   (daemon resolves `observed` first) — make it `debug_assert!` + `return`
   or `unreachable!()`.
6. Optional, low priority: extract `Board::firmware_upgrade` 4-stage
   state machine into `board/firmware.rs` — only if board.rs growth
   continues (it is 765 lines, ~half tests).

## 6. Out of scope

- `rbctl_proto` / `af_packet` / `tinyln*` crates (separate, well-named).
- Splitting `constants.rs` per subsystem (deliberately one file).
- `files/` init scripts, `Makefile`, public API changes.
