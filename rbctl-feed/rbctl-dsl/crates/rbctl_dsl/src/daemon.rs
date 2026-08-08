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

use std::ffi::CString;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rbctl_proto::pack::{AtmEncap, AtmLinkParams, AtmLinkType, AtmQos};
use rbctl_proto::validate;
use tinyln_rs::rtnl::RtnlLink;

use crate::board::Board;
use crate::board_worker::BoardWorker;
use crate::hotplug::{self, LineEvent};
use crate::ipc::{IpcListener, StatusSnapshot};
use crate::uci_cfg::{AtmConfig, CliOverrides, DslConfig, XferMode};
use crate::ubus_obj;

// ── signal flags ─────────────────────────────────────────────────────────
//
// Read by both the main control loop and the board worker thread; written by
// the signal handlers and the IPC command handler. Plain `AtomicBool` is fine
// because we only ever treat them as level triggers (swap-to-clear).

pub(crate) static SHOULD_EXIT: AtomicBool = AtomicBool::new(false);
pub(crate) static SHOULD_RELOAD: AtomicBool = AtomicBool::new(false);
pub(crate) static SHOULD_RESTART_LINE: AtomicBool = AtomicBool::new(false);

/// How long the main thread waits for the board worker to finish its
/// best-effort `line_config_down()` before abandoning it at shutdown. The
/// worker drops its retry budget first, so a responsive board finishes well
/// within this; a silent board is abandoned (process exit reaps the thread).
const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(3);

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

fn parent_iface(iface: &str) -> &str {
    match iface.rfind('.') {
        Some(i) => &iface[..i],
        None => iface,
    }
}

fn atm_params(atm: Option<&AtmConfig>, vlan_id: u16) -> AtmLinkParams<'static> {
    let a = atm.cloned().unwrap_or(AtmConfig {
        vpi: 8,
        vci: 35,
        encap: AtmEncap::Llc,
        link_type: AtmLinkType::Eoa,
        qos: AtmQos::Ubr,
        pcr: 0,
    });
    let mut p = AtmLinkParams::default();
    p.vpi = a.vpi;
    p.vci = a.vci;
    p.encap = a.encap;
    p.link_type = a.link_type;
    p.qos = a.qos;
    p.pcr = a.pcr;
    p.vlan_id = vlan_id;
    p
}

fn create_vlan_iface(parent: &str, vlan_id: u16) -> Result<(), String> {
    let name = format!("{parent}.{vlan_id}");
    let mut rtnl = RtnlLink::new().map_err(|e| format!("rtnl: {e}"))?;

    // RTM_NEWLINK uses NLM_F_CREATE | NLM_F_EXCL, so EEXIST means the
    // interface is already present (e.g. after a daemon restart) — benign.
    match rtnl.add_vlan(parent, vlan_id) {
        Ok(_) => {}
        Err(e) if e.raw_os_error() == Some(libc::EEXIST) => {}
        Err(e) => return Err(format!("add_vlan {name}: {e}")),
    }

    // Ensure it is up whether we just created it or it pre-existed.
    rtnl.set_up(&name).map_err(|e| format!("set_up {name}: {e}"))
}

fn delete_vlan_iface(parent: &str, vlan_id: u16) {
    let name = format!("{parent}.{vlan_id}");
    match RtnlLink::new().and_then(|mut r| r.del(&name)) {
        Ok(()) => {}
        // Interface already gone (normal during shutdown) — silently ignore.
        Err(e) if matches!(e.raw_os_error(), Some(libc::ENODEV) | Some(libc::ENXIO)) => {}
        Err(e) => log::warn!("delete {name}: {e}"),
    }
}

/// Check whether a network interface exists (via `if_nametoindex`).
fn iface_exists(name: &str) -> bool {
    CString::new(name)
        .map(|c| unsafe { libc::if_nametoindex(c.as_ptr()) } != 0)
        .unwrap_or(false)
}

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
    for path in &[
        "/var/run/ubus.sock",
        "/var/run/ubus/ubus.sock",
        "/tmp/run/ubus.sock",
        "/tmp/run/ubus/ubus.sock",
    ] {
        if std::os::unix::net::UnixStream::connect(path).is_ok() {
            return Some((*path).into());
        }
    }
    None
}

/// Build a [`StatusSnapshot`] from the current shared state. Reads only the
/// mutex (never the board, never `cfg`), so it is constant-time and safe to
/// call from the IPC path while the board worker is mid-request.
fn build_snapshot(state: &ubus_obj::SharedState) -> StatusSnapshot {
    let st = state.lock().unwrap();
    match &st.line_obj {
        Some(line) => {
            let (state_str, state_num, up) = crate::ubus_obj::map_link_status(line.link_status);
            StatusSnapshot {
                state: state_str.into(),
                state_num,
                up,
                uptime_secs: st.uptime_secs,
                modulation: format!("{:?}", line.modulation_code),
                annex: format!("{}", crate::ubus_obj::annex_string(line.annex_code)),
                xfer_mode: format!("{:?}", st.xfer_mode.unwrap_or(XferMode::Ptm)),
                down_rate: line.metrics.down_rate,
                up_rate: line.metrics.up_rate,
                down_snr: line.metrics.down_noise_margin,
                up_snr: line.metrics.up_noise_margin,
            }
        }
        None => StatusSnapshot {
            modulation: format!("{:?}", st.modulation),
            annex: format!("{:?}", st.annex),
            xfer_mode: format!("{:?}", st.xfer_mode.unwrap_or(XferMode::Ptm)),
            ..Default::default()
        },
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
    let transport_hint = match cfg.xfer_mode {
        XferMode::Ptm => validate::TransportHint::Ptm,
        XferMode::Atm => validate::TransportHint::Atm,
    };
    if let Err(e) = validate::validate_xfer_mode(cfg.modulation, transport_hint) {
        log::error!("config validation failed: {e}");
        return 1;
    }

    // 1c. Warn if ATM-specific options given with PTM mode (or vice versa)
    if cfg.xfer_mode == XferMode::Ptm && cfg.atm.is_some() {
        log::warn!("ATM config present but xfer_mode=ptm — ATM params ignored");
    }
    if cfg.xfer_mode == XferMode::Atm && cfg.atm.is_none() {
        log::warn!("xfer_mode=atm but no atm-bridge section found — using defaults (vpi=8 vci=35 llc bridged)");
    }

    // Compute transport VLAN id from base index
    let transport_vlan = cfg.transport_vlan_base as u16 + 2000;

    // 2. Verify management interface exists
    if !iface_exists(config_iface) {
        log::error!(
            "management interface {config_iface} does not exist — \
             ensure it is declared in UCI (network device section) \
             or created by the init script before starting the daemon"
        );
        return 1;
    }

    // 3. Open board socket
    let sock = match af_packet::RawSocket::open(config_iface, 0x88B5) {
        Ok(s) => s,
        Err(e) => {
            log::error!("socket: open {config_iface}: {e}");
            return 1;
        }
    };
    let mut board: Board = Board::new(sock);
    board.set_timeout(Duration::from_millis(2000));
    board.set_retries(3);
    log::info!("board socket on {config_iface} (mac={:02x?})", board.local_mac());

    // 3. Configure DSL line (op 1)
    log::info!(
        "line_config_up: mod={:?} annex={:?} profiles=0x{:x} bitswap={} sra={}",
        cfg.modulation, cfg.annex, cfg.profiles.bitmask(), cfg.bitswap, cfg.sra
    );
    if let Err(e) = board.line_config_up(cfg.modulation, cfg.annex, cfg.profiles, cfg.bitswap, cfg.sra) {
        log::error!("line_config_up: {e}");
    }

    // 4. Add data-plane link
    let vlan = match cfg.xfer_mode {
        XferMode::Ptm => {
            log::info!("ptm_link_add: vlan={transport_vlan}");
            board.ptm_link_add(0, 0, 0, transport_vlan).unwrap_or(transport_vlan)
        }
        XferMode::Atm => {
            let a = cfg.atm.as_ref();
            log::info!(
                "atm_link_add: vpi={} vci={} encap={:?} vlan={transport_vlan}",
                a.map(|a| a.vpi).unwrap_or(8),
                a.map(|a| a.vci).unwrap_or(35),
                a.map(|a| a.encap).unwrap_or(AtmEncap::Llc),
            );
            let params = atm_params(cfg.atm.as_ref(), transport_vlan);
            board.atm_link_add(&params).unwrap_or(transport_vlan)
        }
    };
    log::info!("board assigned transport vlan: {vlan}");

    // 5. Create host-side transport VLAN
    let parent = parent_iface(config_iface);
    match create_vlan_iface(parent, vlan) {
        Ok(()) => log::info!("created {parent}.{vlan}"),
        Err(e) => log::warn!("VLAN create {parent}.{vlan}: {e} (may already exist)"),
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
    //
    // The board (which can block for seconds on a silent/unresponsive line)
    // lives on its own thread (`BoardWorker`) so this loop — which serves IPC
    // and ubus — stays responsive at all times. Commands (reload / restart /
    // stop) flow through the `SHOULD_*` static atomics that both threads read.
    // Design: plans/daemon-event-loop-plan.md.
    let worker = BoardWorker::new(
        board,
        cfg,
        overrides.clone(),
        Arc::clone(&state),
        notify_script.map(str::to_string),
    );
    let (worker_done_tx, worker_done_rx) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        worker.run();
        let _ = worker_done_tx.send(());
    });

    log::info!("entering control loop");
    let tick = Duration::from_millis(50);
    while !SHOULD_EXIT.load(Ordering::SeqCst) {
        // Handle IPC commands
        if let Some(ipc) = &ipc {
            let snap = build_snapshot(&state);
            match ipc.accept_one(&snap) {
                Ok(Some(action)) => {
                    if action.should_reload {
                        SHOULD_RELOAD.store(true, Ordering::SeqCst);
                    }
                    if action.should_restart_line {
                        SHOULD_RESTART_LINE.store(true, Ordering::SeqCst);
                    }
                    if action.should_stop {
                        SHOULD_EXIT.store(true, Ordering::SeqCst);
                    }
                }
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
    match worker_done_rx.recv_timeout(SHUTDOWN_DEADLINE) {
        Ok(()) => log::info!("board worker exited cleanly"),
        Err(_) => log::warn!(
            "board worker didn't exit within {SHUTDOWN_DEADLINE:?}; abandoning (process exit reaps it)"
        ),
    }
    delete_vlan_iface(parent, vlan);
    let _ = std::fs::remove_file(crate::ipc::SOCK_PATH);

    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_iface_vlan() {
        assert_eq!(parent_iface("lan0.500"), "lan0");
        assert_eq!(parent_iface("eth0"), "eth0");
    }
}
