//! Daemon main loop — ties together board control, UCI config, hotplug
//! events, ubus `dsl` object, and IPC control socket into a single loop.
//!
//! ## Architecture
//!
//! ```text
//! rbctl-dsl daemon
//! ├── init: load UCI (+CLI overrides) → open board → line_config_up
//! │         → link_add → VLAN create → bind IPC socket → register ubus
//! └── loop (1 s):
//!     ├── ipc.accept_one() → handle status/reload/restart/stop
//!     ├── board.get_line_obj() → update shared state
//!     ├── detect state transition → emit hotplug event
//!     ├── ubus.poll_one() → serve "metrics" / "statistics"
//!     ├── check SIGTERM / SIGHUP
//!     └── thread::sleep(1s)
//! ```

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use rbctl_proto::oem::pack::AtmEncap;
use rbctl_proto::oem::validate;

use crate::board::Board;
use crate::board_worker::BoardWorker;
use crate::constants;
use crate::hotplug::{self, LineEvent};
use crate::ipc::{IpcListener, StatusSnapshot};
use crate::mgmt_link;
use crate::uci_cfg::{CliOverrides, DslConfig, XferMode};
use crate::ubus_obj;
use crate::utils::{mapping, xdsl};

// ── signal flags ─────────────────────────────────────────────────────────
//
// Read by both the main control loop and the board worker thread; written by
// the signal handlers and the IPC command handler. Plain `AtomicBool` is fine
// because we only ever treat them as level triggers (swap-to-clear).

pub(crate) static SHOULD_EXIT: AtomicBool = AtomicBool::new(false);
pub(crate) static SHOULD_RELOAD: AtomicBool = AtomicBool::new(false);
pub(crate) static SHOULD_RESTART_LINE: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_sigterm(_sig: libc::c_int) {
    SHOULD_EXIT.store(true, Ordering::SeqCst);
}

extern "C" fn handle_sighup(_sig: libc::c_int) {
    SHOULD_RELOAD.store(true, Ordering::SeqCst);
}

fn install_signal_handlers() {
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = handle_sigterm as usize;
        libc::sigaction(libc::SIGTERM, &sa, std::ptr::null_mut());
        libc::sigaction(libc::SIGINT, &sa, std::ptr::null_mut());

        let mut sa_hup: libc::sigaction = std::mem::zeroed();
        sa_hup.sa_sigaction = handle_sighup as usize;
        libc::sigaction(libc::SIGHUP, &sa_hup, std::ptr::null_mut());
    }
}

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
fn build_snapshot(state: &ubus_obj::SharedState, config_iface: &str) -> StatusSnapshot {
    let st = state.lock().unwrap();
    let fw = st.fw_status.as_str();
    let board_reachable = st
        .last_poll_ok
        .map_or(false, |t| t.elapsed() < constants::POLL_STALE_TIMEOUT);
    let link = mgmt_link::MgmtLinkStatus::probe(config_iface);

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
            ..Default::default()
        },
    };
    snap
}

/// Handle a `FIRMWARE <path>` IPC command: validate the image, send it to the
/// worker, and stream progress back to the IPC client.  The IPC connection is
/// held open for the duration of the upload — if the client disconnects, the
/// worker continues and the result is stored in shared state.
fn handle_firmware_ipc(
    path: &str,
    stream: &mut std::os::unix::net::UnixStream,
    cmd_tx: &std::sync::mpsc::Sender<crate::board_worker::WorkerCmd>,
    state: &ubus_obj::SharedState,
) {
    use std::io::Write;

    // Check if an upgrade is already running
    {
        let st = state.lock().unwrap();
        if !matches!(st.fw_status, ubus_obj::FwStatus::Idle | ubus_obj::FwStatus::Done | ubus_obj::FwStatus::Failed(_)) {
            let _ = stream.write_all(b"ERR: upgrade already in progress\n.\n");
            return;
        }
    }

    // Stat first so we never slurp a non-regular file or an oversized image
    // into memory (D7). MAX_IMAGE_SIZE is the board's own 8 MB cap.
    let max = rbctl_proto::oem::firmware::MAX_IMAGE_SIZE as u64;
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) => {
            let _ = stream.write_all(format!("ERR: cannot stat {path}: {e}\n.\n").as_bytes());
            return;
        }
    };
    if !meta.is_file() {
        let _ = stream.write_all(b"ERR: not a regular file\n.\n");
        return;
    }
    if meta.len() > max {
        let _ = stream
            .write_all(format!("ERR: image too large ({} > {max} bytes)\n.\n", meta.len()).as_bytes());
        return;
    }

    // Read and validate the file
    let image = match std::fs::read(path) {
        Ok(data) => data,
        Err(e) => {
            let _ = stream.write_all(format!("ERR: cannot read {path}: {e}\n.\n").as_bytes());
            return;
        }
    };

    if let Err(e) = rbctl_proto::oem::firmware::validate_image(&image) {
        let _ = stream.write_all(format!("ERR: {e}\n.\n").as_bytes());
        return;
    }

    let hdr = rbctl_proto::oem::firmware::parse_header(&image).ok();
    let version = hdr.as_ref().map(|h| rbctl_proto::oem::firmware::version_str(h).to_string());

    // Send acknowledgment
    let _ = stream.write_all(b"OK\n");
    if let Some(ref v) = version {
        let _ = stream.write_all(format!("IMAGE version={v} size={}\n", image.len()).as_bytes());
    }
    let _ = stream.flush();

    // Send to worker
    let (reply_tx, reply_rx) = std::sync::mpsc::channel::<crate::board_worker::FwEvent>();
    if cmd_tx
        .send(crate::board_worker::WorkerCmd::FirmwareUpgrade {
            image,
            reply_tx,
        })
        .is_err()
    {
        let _ = stream.write_all(b"ERR: worker thread unavailable\n.\n");
        return;
    }

    // Stream progress until done
    let _ = stream.set_write_timeout(Some(constants::IPC_FW_STREAM_WRITE_TIMEOUT));
    while let Ok(event) = reply_rx.recv() {
        match event {
            crate::board_worker::FwEvent::Progress(p) => {
                let line = if p.pct > 0 {
                    format!("PROGRESS stage={} pct={}\n", p.stage, p.pct)
                } else {
                    format!("PROGRESS stage={}\n", p.stage)
                };
                if stream.write_all(line.as_bytes()).is_err() {
                    break; // client disconnected
                }
                let _ = stream.flush();
            }
            crate::board_worker::FwEvent::Done(Ok(result)) => {
                let _ = stream.write_all(
                    format!("DONE version=0x{:08X} status={}\n.\n", result.version, result.status).as_bytes(),
                );
                break;
            }
            crate::board_worker::FwEvent::Done(Err(msg)) => {
                let _ = stream.write_all(format!("ERR: {msg}\n.\n").as_bytes());
                break;
            }
        }
    }
}

// ── daemon entry point ───────────────────────────────────────────────────

/// Run the daemon. Returns process exit code.
pub fn run(
    config_iface: &str,
    notify_script: Option<&str>,
    overrides: &CliOverrides,
) -> i32 {
    let _log_tag = "daemon";
    install_signal_handlers();

    // 1. Load config (UCI + CLI overrides)
    let mut cfg = match DslConfig::load(overrides) {
        Ok(c) => {
            log::info!(
                "config: mod={:?} annex={:?} profiles=0x{:x} xfer={:?} bitswap={} sra={} vlan_base={}",
                c.modulation, c.annex, c.profiles.bitmask(), c.xfer_mode,
                c.bitswap, c.sra, c.transport_vlan_base
            );
            c
        }
        Err(e) => {
            log::warn!("config load failed ({e}), using defaults");
            DslConfig::default()
        }
    };

    // 1b. Validate config before TX (§3a.1)
    if let Err(e) = validate::validate_line_config(cfg.modulation, cfg.annex, cfg.profiles) {
        log::error!("config validation failed: {e}");
        return 1;
    }
    // Skip xfer_mode validation in auto mode — Multimode accepts either,
    // and the actual mode is decided after the line trains.
    if cfg.xfer_mode != XferMode::Auto {
        let transport_hint = match cfg.xfer_mode {
            XferMode::Ptm => validate::TransportHint::Ptm,
            XferMode::Atm => validate::TransportHint::Atm,
            XferMode::Auto => unreachable!(),
        };
        if let Err(e) = validate::validate_xfer_mode(cfg.modulation, transport_hint) {
            log::error!("config validation failed: {e}");
            return 1;
        }
    }

    // 1c. Warn if ATM-specific options given with PTM mode (or vice versa)
    if cfg.xfer_mode == XferMode::Ptm && cfg.atm.is_some() {
        log::warn!("ATM config present but xfer_mode=ptm — ATM params ignored");
    }
    if cfg.xfer_mode == XferMode::Atm && cfg.atm.is_none() {
        log::warn!("xfer_mode=atm but no atm-bridge section found — using defaults (vpi=8 vci=35 llc bridged)");
    }

    // Compute transport VLAN id from base index
    let transport_vlan = cfg.transport_vlan_base as u16 + constants::VLAN_TRANSPORT_BASE_OFFSET;

    // 2. Verify management interface — read-only carrier check via
    //    MgmtLinkStatus. The daemon never alters management interfaces;
    //    if they're down, dsl_control / netifd is responsible.
    let link = mgmt_link::MgmtLinkStatus::probe(config_iface);
    if !link.exists {
        log::error!("management interface {}", link.diagnostic());
        log::error!(
            "  create it via UCI device section or board.d before starting the daemon"
        );
        return 1;
    }
    if !link.is_ready() {
        log::warn!("management interface not ready: {}", link.diagnostic());
        log::warn!("  the daemon will continue — the worker will report board-unreachable");
    } else {
        log::info!("management interface {config_iface} ready (carrier ok)");
    }

    // 3. Open board socket
    let sock = match af_packet::RawSocket::open(config_iface, constants::BOARD_ETHERTYPE_CMD) {
        Ok(s) => s,
        Err(e) => {
            log::error!("socket: open {config_iface}: {e}");
            return 1;
        }
    };
    let mut board: Board = Board::new(sock);
    board.set_timeout(constants::BOARD_DEFAULT_TIMEOUT);
    board.set_retries(constants::BOARD_DEFAULT_RETRIES);
    log::info!("board socket on {config_iface} (mac={:02x?})", board.local_mac());

    // 3. Configure DSL line (op 1)
    log::info!(
        "line_config_up: mod={:?} annex={:?} profiles=0x{:x} bitswap={} sra={}",
        cfg.modulation, cfg.annex, cfg.profiles.bitmask(), cfg.bitswap, cfg.sra
    );
    if let Err(e) = board.line_config_up(cfg.modulation, cfg.annex, cfg.profiles, cfg.bitswap, cfg.sra) {
        log::error!("line_config_up: {e}");
    }

    // 4. Add data-plane link (skip in auto mode — worker provisions after Up).
    //    The board's reply (assigned VLAN) is authoritative: on error we do NOT
    //    assume the requested VLAN and skip host-side VLAN creation — the worker
    //    retries provisioning once the board is reachable (D9).
    let parent = mgmt_link::parent_of(config_iface).unwrap_or(config_iface);
    let provisioned_vlan: Option<u16> = match cfg.xfer_mode {
        XferMode::Ptm => {
            log::info!("ptm_link_add: vlan={transport_vlan}");
            match board.ptm_link_add(0, 0, 0, transport_vlan) {
                Ok(v) => {
                    log::info!("board assigned transport vlan: {v}");
                    Some(v)
                }
                Err(e) => {
                    log::warn!("ptm_link_add failed: {e} — host VLAN not created (worker will retry)");
                    None
                }
            }
        }
        XferMode::Atm => {
            let a = cfg.atm.as_ref();
            log::info!(
                "atm_link_add: vpi={} vci={} encap={:?} vlan={transport_vlan}",
                a.map(|a| a.vpi).unwrap_or(8),
                a.map(|a| a.vci).unwrap_or(35),
                a.map(|a| a.encap).unwrap_or(AtmEncap::Llc),
            );
            let params = xdsl::atm_params(cfg.atm.as_ref(), transport_vlan);
            match board.atm_link_add(&params) {
                Ok(v) => {
                    log::info!("board assigned transport vlan: {v}");
                    Some(v)
                }
                Err(e) => {
                    log::warn!("atm_link_add failed: {e} — host VLAN not created (worker will retry)");
                    None
                }
            }
        }
        XferMode::Auto => {
            log::info!("auto: deferring link provisioning until line trains");
            None
        }
    };
    // For best-effort cleanup at shutdown: the board-assigned VLAN when
    // provisioning succeeded, else the requested one.
    let vlan = provisioned_vlan.unwrap_or(transport_vlan);

    // 5. Create host-side transport VLAN — only when the board acknowledged
    //    the link (auto mode defers this to the worker's Up-transition path).
    if cfg.xfer_mode != XferMode::Auto && provisioned_vlan.is_some() {
        match xdsl::create_vlan_iface(parent, vlan) {
            Ok(()) => log::info!("created {parent}.{vlan}"),
            Err(e) => log::warn!("VLAN create {parent}.{vlan}: {e} (may already exist)"),
        }
    }

    // 6. Bind IPC socket
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

    // 7. Register ubus object (optional)
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

    // 8. Emit initial TC-layer status
    if let Some(script) = notify_script {
        hotplug::emit_tc_layer(script, cfg.xfer_mode);
    }

    // 9. Spawn the board worker + run a slim control loop.
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<crate::board_worker::WorkerCmd>();
    let worker = BoardWorker::new(
        board,
        cfg,
        overrides.clone(),
        Arc::clone(&state),
        notify_script.map(str::to_string),
        cmd_rx,
        transport_vlan,
        parent.to_string(),
    );
    let (worker_done_tx, worker_done_rx) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        worker.run();
        let _ = worker_done_tx.send(());
    });

    log::info!("entering control loop");
    let tick = constants::POLL_TICK;
    while !SHOULD_EXIT.load(Ordering::SeqCst) {
        // D10: detect board-worker exit (panic, early return, or channel
        // close). The worker owns the board socket; without it the daemon can
        // only zombie-run and silently drop control commands — shut down
        // instead. (Guarded by SHOULD_EXIT so a normal shutdown in progress
        // isn't misreported as unexpected.)
        match worker_done_rx.try_recv() {
            Ok(()) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                if !SHOULD_EXIT.load(Ordering::SeqCst) {
                    log::error!("board worker exited unexpectedly — shutting down");
                    SHOULD_EXIT.store(true, Ordering::SeqCst);
                }
                break;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }

        // Forward signal-driven flags to the worker command channel.
        // Commands are naturally deferred if a firmware upgrade is in
        // progress (mpsc FIFO buffers them until the worker is ready).
        if SHOULD_RELOAD.swap(false, Ordering::SeqCst) {
            let _ = cmd_tx.send(crate::board_worker::WorkerCmd::Reload);
        }
        if SHOULD_RESTART_LINE.swap(false, Ordering::SeqCst) {
            let _ = cmd_tx.send(crate::board_worker::WorkerCmd::RestartLine);
        }

        // Handle IPC commands
        if let Some(ipc) = &ipc {
            let snap = build_snapshot(&state, config_iface);
            match ipc.accept_one(&snap) {
                Ok(Some(action)) => match action {
                    crate::ipc::IpcAction::Reload => {
                        let _ = cmd_tx.send(crate::board_worker::WorkerCmd::Reload);
                    }
                    crate::ipc::IpcAction::RestartLine => {
                        let _ = cmd_tx.send(crate::board_worker::WorkerCmd::RestartLine);
                    }
                    crate::ipc::IpcAction::Stop => {
                        SHOULD_EXIT.store(true, Ordering::SeqCst);
                    }
                    crate::ipc::IpcAction::FirmwareUpgrade { path, stream } => {
                        // Spawn a dedicated thread so the main loop keeps
                        // serving ubus and other IPC clients during the
                        // (potentially multi-minute) upload.
                        let cmd_tx = cmd_tx.clone();
                        let state = Arc::clone(&state);
                        std::thread::spawn(move || {
                            let mut stream = stream;
                            handle_firmware_ipc(&path, &mut stream, &cmd_tx, &state);
                        });
                    }
                    crate::ipc::IpcAction::None => {}
                },
                Ok(None) => {} // no client
                Err(e) => log::error!("IPC error: {e}"),
            }
        }

        // Poll ubus (non-blocking)
        if let Some(conn) = ubus_conn.as_mut() {
            let _ = conn.poll_one();
        }

        std::thread::sleep(tick);
    }

    // 10. Clean shutdown
    log::info!("shutting down");
    if let Some(script) = notify_script {
        hotplug::emit_status(script, LineEvent::Down);
    }
    // The worker performs `line_config_down()` on its way out; bound the wait
    // so a silent board can't hold shutdown (it drops its retry budget first).
    match worker_done_rx.recv_timeout(constants::DAEMON_SHUTDOWN_DEADLINE) {
        Ok(()) => log::info!("board worker exited cleanly"),
        Err(_) => log::warn!(
            "board worker didn't exit within {:?}; abandoning (process exit reaps it)",
            constants::DAEMON_SHUTDOWN_DEADLINE
        ),
    }
    xdsl::delete_vlan_iface(parent, vlan);
    let _ = std::fs::remove_file(crate::ipc::SOCK_PATH);

    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_iface_vlan() {
        assert_eq!(mgmt_link::parent_of("lan0.500"), Some("lan0"));
        assert_eq!(mgmt_link::parent_of("eth0"), None);
    }
}
