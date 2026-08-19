//! Daemon — the policy gate and control loop.
//!
//! ## Architecture
//!
//! ```text
//! rbctl-dsl daemon (this module)
//! ├── init:    load UCI (+CLI overrides) → probe mgmt link → open board
//! ├── gate:    ALL board-operation decisions happen here
//! │     ├── sends WorkerCmd to the worker (config, provisioning, restart)
//! │     └── consumes WorkerEvent from the worker (transitions, results)
//! └── loop (POLL_TICK):
//!       ├── worker events → decide + command (retry, auto provisioning…)
//!       ├── IPC accept → status / reload / restart / stop / firmware
//!       ├── signals (SIGTERM/SIGHUP) → commands
//!       └── ubus poll → serve "metrics" / "statistics"
//! ```
//!
//! The board worker (`board_worker.rs`) owns the board socket but makes no
//! decisions: it polls op2, reports transitions as events, and executes
//! the daemon's commands. Monitor mode (`--monitor`) is enforced here —
//! the daemon simply never issues configuration commands.
//!
//! Submodules:
//! - [`init`] — startup phase (config, validation, socket)
//! - [`signals`] — SIGTERM/SIGINT/SIGHUP → AtomicBool flags
//! - [`firmware`] — `FIRMWARE <path>` IPC streaming handler

mod firmware;
mod init;
pub(crate) mod signals;

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

use rbctl_proto::oem::unpack::{DataPath, LinkStatus};

use crate::board_worker::{BoardWorker, WorkerCmd, WorkerEvent};
use crate::constants;
use crate::hotplug::{self, LineEvent};
use crate::ipc::{IpcListener, StatusSnapshot};
use crate::uci_cfg::{CliOverrides, DslConfig, XferMode};
use crate::ubus_obj;
use crate::utils::{mapping, xdsl};

use signals::{SHOULD_EXIT as EXIT, SHOULD_RELOAD as RELOAD, SHOULD_RESTART_LINE as RESTART};

// ── helpers ──────────────────────────────────────────────────────────────

fn connect_ubus(
    obj: ubus::server::UbusObject,
) -> Option<ubus::server::UbusConnection<crate::transport::UnixUbusTransport>> {
    let path = find_ubus_socket()?;
    log::info!("ubus: connecting to {path}");
    let transport = match crate::transport::UnixUbusTransport::connect(&path) {
        Ok(t) => t,
        Err(e) => {
            log::warn!("ubus: connect to {path} failed: {e}");
            return None;
        }
    };
    match ubus::server::UbusConnection::connect_and_register(transport, obj) {
        Ok(c) => Some(c),
        Err(e) => {
            log::warn!("ubus: HELLO/ADD_OBJECT failed: {e:?}");
            None
        }
    }
}

/// Try common ubus socket paths (varies between OpenWrt versions/devices).
fn find_ubus_socket() -> Option<String> {
    for path in constants::UBUS_SOCKET_PATHS {
        if std::os::unix::net::UnixStream::connect(path).is_ok() {
            return Some((*path).into());
        }
    }
    None
}

/// Build a [`StatusSnapshot`] from the current shared state. Reads only the
/// mutex (never the board, never `cfg`), so it is constant-time and safe to
/// call from the IPC path while the board worker is mid-request.
fn build_snapshot(
    state: &ubus_obj::SharedState,
    config_iface: &str,
    monitor: bool,
) -> StatusSnapshot {
    let st = state.lock().unwrap();
    let fw = st.fw_status.as_str();
    let board_reachable = st
        .last_poll_ok
        .map_or(false, |t| t.elapsed() < constants::POLL_STALE_TIMEOUT);
    let link = crate::mgmt_link::MgmtLinkStatus::probe(config_iface);

    let snap = match &st.line_obj {
        Some(line) => {
            let (state_str, state_num, up) = mapping::map_link_status(line.link_status);
            StatusSnapshot {
                state: state_str.into(),
                state_num,
                up,
                uptime_secs: st.uptime_secs,
                modulation: format!("{:?}", line.modulation_code),
                annex: format!("{}", mapping::annex_string(line.annex_code)),
                xfer_mode: format!("{:?}", st.xfer_mode.unwrap_or(XferMode::Ptm)),
                down_rate: line.metrics.down_rate,
                up_rate: line.metrics.up_rate,
                down_snr: line.metrics.down_noise_margin,
                up_snr: line.metrics.up_noise_margin,
                fw_status: fw,
                board_reachable,
                mgmt_iface: config_iface.into(),
                mgmt_carrier: link.carrier,
                monitor,
            }
        }
        None => StatusSnapshot {
            modulation: format!("{:?}", st.modulation),
            annex: format!("{:?}", st.annex),
            xfer_mode: format!("{:?}", st.xfer_mode.unwrap_or(XferMode::Ptm)),
            fw_status: fw,
            board_reachable,
            mgmt_iface: config_iface.into(),
            mgmt_carrier: link.carrier,
            monitor,
            ..Default::default()
        },
    };
    snap
}

fn link_status_to_event(status: LinkStatus) -> LineEvent {
    match status {
        LinkStatus::Up => LineEvent::Up,
        LinkStatus::Initializing => LineEvent::Training,
        LinkStatus::EstablishingLink => LineEvent::Handshake,
        LinkStatus::NoSignal | LinkStatus::Unknown(_) => LineEvent::Down,
    }
}

fn config_changed(old: &DslConfig, new: &DslConfig) -> bool {
    old.modulation != new.modulation
        || old.annex != new.annex
        || old.profiles.bitmask() != new.profiles.bitmask()
        || old.xfer_mode != new.xfer_mode
}

// ── daemon entry point ───────────────────────────────────────────────────

/// Run the daemon. Returns process exit code.
pub fn run(
    config_iface: &str,
    notify_script: Option<&str>,
    overrides: &CliOverrides,
    monitor: bool,
) -> i32 {
    signals::install();

    let init = match init::startup(config_iface, overrides) {
        Ok(i) => i,
        Err(code) => return code,
    };
    let init::Init { board, cfg, transport_vlan: _requested_vlan, parent } = init;

    // Bind IPC socket
    let ipc = match IpcListener::bind(crate::ipc::SOCK_PATH) {
        Ok(l) => {
            log::info!("IPC socket at {}", crate::ipc::SOCK_PATH);
            Some(l)
        }
        Err(e) => {
            log::warn!("IPC bind failed: {e} — control socket disabled");
            None
        }
    };

    // Shared state + ubus object
    let state = ubus_obj::new_shared_state();
    {
        let mut st = state.lock().unwrap();
        st.xfer_mode = Some(cfg.xfer_mode);
        st.modulation = cfg.modulation;
        st.annex = cfg.annex;
    }
    let ubus_obj = ubus_obj::build_dsl_object(Arc::clone(&state));
    let mut ubus_conn = match connect_ubus(ubus_obj) {
        Some(c) => {
            log::info!("ubus object 'dsl' registered");
            Some(c)
        }
        None => {
            log::warn!("ubus unavailable — metrics will not be published");
            None
        }
    };

    // Spawn the board worker (poll-only executor).
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<WorkerCmd>();
    let (event_tx, event_rx) = std::sync::mpsc::channel::<WorkerEvent>();
    let worker = BoardWorker::new(
        board,
        cfg.clone(),
        Arc::clone(&state),
        cmd_rx,
        event_tx,
    );
    let (worker_done_tx, worker_done_rx) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        worker.run();
        let _ = worker_done_tx.send(());
    });

    // ── The gate: daemon-owned policy state ─────────────────────────────
    let mut cfg = cfg;
    let mut config_applied = false;
    let mut link_provisioned = false;
    let mut provisioned_vlan: Option<u16> = None;
    let mut last_apply: Option<Instant> = None;
    let mut tc_emitted = false;

    // Initial configuration — the only place startup config is decided.
    if monitor {
        log::info!("monitor mode: observing the board, no configuration will be sent");
    } else {
        let _ = cmd_tx.send(WorkerCmd::ApplyLineConfig);
        if let XferMode::Ptm | XferMode::Atm = cfg.xfer_mode {
            let _ = cmd_tx.send(WorkerCmd::ProvisionLink { xfer: cfg.xfer_mode });
        }
    }

    // Emit initial TC-layer status (skipped in monitor mode — no
    // configured mode to report; emitted on Up transitions instead)
    if !monitor {
        if let Some(script) = notify_script {
            hotplug::emit_tc_layer(script, cfg.xfer_mode);
        }
    }

    log::info!("entering control loop");
    let tick = constants::POLL_TICK;
    while !EXIT.load(Ordering::SeqCst) {
        // D10: detect board-worker exit (panic, early return, or channel
        // close). The worker owns the board socket; without it the daemon can
        // only zombie-run and silently drop control commands — shut down
        // instead.
        match worker_done_rx.try_recv() {
            Ok(()) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                if !EXIT.load(Ordering::SeqCst) {
                    log::error!("board worker exited unexpectedly — shutting down");
                    EXIT.store(true, Ordering::SeqCst);
                }
                break;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }

        // ── Worker events → policy decisions ───────────────────────────
        while let Ok(ev) = event_rx.try_recv() {
            match ev {
                WorkerEvent::BoardReachable => {
                    // Deferred config: op1 failed at startup because the
                    // board was unreachable — retry now (throttled).
                    if !monitor && !config_applied {
                        let due = last_apply
                            .map_or(true, |t| t.elapsed() >= constants::POLL_INTERVAL);
                        if due {
                            last_apply = Some(Instant::now());
                            log::info!("board reachable — applying deferred config");
                            let _ = cmd_tx.send(WorkerCmd::ApplyLineConfig);
                        }
                    }
                }
                WorkerEvent::BoardUnreachable => {
                    log::warn!("board became unreachable");
                    config_applied = false;
                    link_provisioned = false;
                }
                WorkerEvent::LineState { status, data_path } => {
                    let event = link_status_to_event(status);
                    log::info!("line state: {status:?} → {event:?}");
                    if let Some(script) = notify_script {
                        hotplug::emit_status(script, event);
                    }
                    if status == LinkStatus::Up {
                        // Observed mode from op2 — authoritative for monitor
                        // display and auto provisioning.
                        let observed = match data_path {
                            DataPath::Ptm => XferMode::Ptm,
                            DataPath::Atm => XferMode::Atm,
                            DataPath::Unknown(_) => cfg.xfer_mode,
                        };
                        {
                            let mut st = state.lock().unwrap();
                            if monitor || cfg.xfer_mode == XferMode::Auto {
                                st.xfer_mode = Some(observed);
                            }
                        }
                        if !tc_emitted {
                            let emit_mode = if monitor || cfg.xfer_mode == XferMode::Auto {
                                observed
                            } else {
                                cfg.xfer_mode
                            };
                            if let Some(script) = notify_script {
                                hotplug::emit_tc_layer(script, emit_mode);
                            }
                            tc_emitted = true;
                        }
                        // Auto mode: provision the link now that the actual
                        // TC mode is known.
                        if !monitor && cfg.xfer_mode == XferMode::Auto && !link_provisioned {
                            let _ = cmd_tx.send(WorkerCmd::ProvisionLink { xfer: observed });
                        }
                    }
                }
                WorkerEvent::LineConfigApplied => {
                    config_applied = true;
                    // Fixed-mode deferred retry: provisioning follows config.
                    if !monitor && !link_provisioned {
                        if let XferMode::Ptm | XferMode::Atm = cfg.xfer_mode {
                            let _ = cmd_tx
                                .send(WorkerCmd::ProvisionLink { xfer: cfg.xfer_mode });
                        }
                    }
                }
                WorkerEvent::LinkProvisioned { vlan } => {
                    link_provisioned = true;
                    provisioned_vlan = Some(vlan);
                    log::info!("board assigned transport vlan: {vlan}");
                    match xdsl::create_vlan_iface(&parent, vlan) {
                        Ok(()) => log::info!("created {parent}.{vlan}"),
                        Err(e) => log::warn!("VLAN create {parent}.{vlan}: {e} (may already exist)"),
                    }
                }
            }
        }

        // ── Signals → commands ──────────────────────────────────────────
        if RELOAD.swap(false, Ordering::SeqCst) {
            reload_config(&cmd_tx, &mut cfg, overrides, monitor, &mut config_applied,
                          &mut link_provisioned, &mut tc_emitted);
        }
        if RESTART.swap(false, Ordering::SeqCst) {
            if monitor {
                log::warn!("restart-line refused in monitor mode");
            } else {
                let _ = cmd_tx.send(WorkerCmd::RestartLine);
                tc_emitted = false;
            }
        }

        // ── IPC commands ────────────────────────────────────────────────
        if let Some(ipc) = &ipc {
            let snap = build_snapshot(&state, config_iface, monitor);
            match ipc.accept_one(&snap) {
                Ok(Some(action)) => match action {
                    crate::ipc::IpcAction::Reload => {
                        reload_config(&cmd_tx, &mut cfg, overrides, monitor, &mut config_applied,
                                      &mut link_provisioned, &mut tc_emitted);
                    }
                    crate::ipc::IpcAction::RestartLine => {
                        if !monitor {
                            let _ = cmd_tx.send(WorkerCmd::RestartLine);
                            tc_emitted = false;
                        }
                    }
                    crate::ipc::IpcAction::Stop => {
                        EXIT.store(true, Ordering::SeqCst);
                    }
                    crate::ipc::IpcAction::FirmwareUpgrade { path, stream } => {
                        // Spawn a dedicated thread so the main loop keeps
                        // serving ubus and other IPC clients during the
                        // (potentially multi-minute) upload.
                        let cmd_tx = cmd_tx.clone();
                        let state = Arc::clone(&state);
                        std::thread::spawn(move || {
                            let mut stream = stream;
                            firmware::handle_firmware_ipc(&path, &mut stream, &cmd_tx, &state);
                        });
                    }
                    crate::ipc::IpcAction::None => {}
                },
                Ok(None) => {} // no client
                Err(e) => log::error!("IPC error: {e}"),
            }
        }

        // ── ubus ────────────────────────────────────────────────────────
        if let Some(conn) = ubus_conn.as_mut() {
            let _ = conn.poll_one();
        }

        std::thread::sleep(tick);
    }

    // ── Clean shutdown ─────────────────────────────────────────────────
    log::info!("shutting down");
    if let Some(script) = notify_script {
        hotplug::emit_status(script, LineEvent::Down);
    }
    // Teardown through the gate: monitor mode never configured the board,
    // so it must not tear it down either.
    if !monitor {
        let _ = cmd_tx.send(WorkerCmd::LineDown);
    }
    let _ = cmd_tx.send(WorkerCmd::Shutdown);
    match worker_done_rx.recv_timeout(constants::DAEMON_SHUTDOWN_DEADLINE) {
        Ok(()) => log::info!("board worker exited cleanly"),
        Err(_) => log::warn!(
            "board worker didn't exit within {:?}; abandoning (process exit reaps it)",
            constants::DAEMON_SHUTDOWN_DEADLINE
        ),
    }
    if !monitor {
        if let Some(vlan) = provisioned_vlan {
            xdsl::delete_vlan_iface(&parent, vlan);
        }
    }
    let _ = std::fs::remove_file(crate::ipc::SOCK_PATH);

    0
}

/// Re-read UCI and, if line params changed, reconfigure through the gate.
fn reload_config(
    cmd_tx: &std::sync::mpsc::Sender<WorkerCmd>,
    cfg: &mut DslConfig,
    overrides: &CliOverrides,
    monitor: bool,
    config_applied: &mut bool,
    link_provisioned: &mut bool,
    tc_emitted: &mut bool,
) {
    log::info!("reload — re-reading UCI config");
    if let Ok(new_cfg) = DslConfig::load(overrides) {
        let changed = config_changed(cfg, &new_cfg);
        *cfg = new_cfg;
        // Always refresh the worker's display copy.
        let _ = cmd_tx.send(WorkerCmd::UpdateConfig(Box::new(cfg.clone())));
        if monitor {
            log::info!("monitor mode: only refreshing display config");
            return;
        }
        if changed {
            log::info!("line params changed — reconfiguring");
            *config_applied = false;
            *link_provisioned = false;
            *tc_emitted = false;
            let _ = cmd_tx.send(WorkerCmd::LineDown);
            let _ = cmd_tx.send(WorkerCmd::ApplyLineConfig);
            if let XferMode::Ptm | XferMode::Atm = cfg.xfer_mode {
                let _ = cmd_tx.send(WorkerCmd::ProvisionLink { xfer: cfg.xfer_mode });
            }
        } else {
            log::info!("no line-param changes");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rbctl_proto::oem::pack::Annex;

    #[test]
    fn link_status_events() {
        assert_eq!(link_status_to_event(LinkStatus::Up), LineEvent::Up);
        assert_eq!(link_status_to_event(LinkStatus::NoSignal), LineEvent::Down);
        assert_eq!(
            link_status_to_event(LinkStatus::Initializing),
            LineEvent::Training
        );
        assert_eq!(
            link_status_to_event(LinkStatus::EstablishingLink),
            LineEvent::Handshake
        );
    }

    #[test]
    fn config_change_detection() {
        let a = DslConfig::default();
        let b = DslConfig {
            annex: Annex::A,
            ..DslConfig::default()
        };
        assert!(config_changed(&a, &b));
    }

    #[test]
    fn config_no_change() {
        let a = DslConfig::default();
        let b = DslConfig::default();
        assert!(!config_changed(&a, &b));
    }
}
