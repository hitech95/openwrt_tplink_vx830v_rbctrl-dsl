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
use std::time::{Duration, Instant};

use rbctl_proto::pack::{AtmEncap, AtmLinkParams, AtmLinkType, AtmQos};
use rbctl_proto::unpack::LinkStatus;
use rbctl_proto::validate;
use tinyln_rs::rtnl::RtnlLink;

use crate::board::{Board, BoardError};
use crate::hotplug::{self, LineEvent};
use crate::ipc::{IpcListener, StatusSnapshot};
use crate::uci_cfg::{AtmConfig, CliOverrides, DslConfig, XferMode};
use crate::ubus_obj;

// ── signal flags ─────────────────────────────────────────────────────────

static SHOULD_EXIT: AtomicBool = AtomicBool::new(false);
static SHOULD_RELOAD: AtomicBool = AtomicBool::new(false);
static SHOULD_RESTART_LINE: AtomicBool = AtomicBool::new(false);

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

fn link_status_to_event(status: LinkStatus) -> LineEvent {
    match status {
        LinkStatus::Up => LineEvent::Up,
        LinkStatus::Initializing => LineEvent::Training,
        LinkStatus::EstablishingLink => LineEvent::Handshake,
        LinkStatus::NoSignal | LinkStatus::Unknown(_) => LineEvent::Down,
    }
}

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

fn config_changed(old: &DslConfig, new: &DslConfig) -> bool {
    old.modulation != new.modulation
        || old.annex != new.annex
        || old.profiles.bitmask() != new.profiles.bitmask()
        || old.xfer_mode != new.xfer_mode
}

/// Build a [`StatusSnapshot`] from the current shared state.
fn build_snapshot(state: &ubus_obj::SharedState, cfg: &DslConfig) -> StatusSnapshot {
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
                xfer_mode: format!("{:?}", cfg.xfer_mode),
                down_rate: line.metrics.down_curr_rate,
                up_rate: line.metrics.up_curr_rate,
                down_snr: line.metrics.down_snr_margin,
                up_snr: line.metrics.up_snr_margin,
            }
        }
        None => StatusSnapshot {
            modulation: format!("{:?}", cfg.modulation),
            annex: format!("{:?}", cfg.annex),
            xfer_mode: format!("{:?}", cfg.xfer_mode),
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

    // 9. Main loop
    let poll_interval = Duration::from_secs(1);
    let mut last_status: Option<LinkStatus> = None;
    let mut up_since: Option<Instant> = None;
    let mut tc_emitted = false;

    log::info!("entering poll loop");
    while !SHOULD_EXIT.load(Ordering::SeqCst) {
        // Handle IPC commands
        if let Some(ipc) = &ipc {
            let snap = build_snapshot(&state, &cfg);
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

        // Reload config (SIGHUP or IPC reload)
        if SHOULD_RELOAD.swap(false, Ordering::SeqCst) {
            log::info!("reload — re-reading UCI config");
            if let Ok(new_cfg) = DslConfig::load(overrides) {
                if config_changed(&cfg, &new_cfg) {
                    log::info!("line params changed — reconfiguring");
                    let _ = board.line_config_down();
                    let _ = board.line_config_up(
                        new_cfg.modulation, new_cfg.annex, new_cfg.profiles,
                        new_cfg.bitswap, new_cfg.sra,
                    );
                    cfg = new_cfg;
                    tc_emitted = false;
                } else {
                    log::info!("no line-param changes");
                }
            }
        }

        // Restart line (IPC restart-line — bounce without config change)
        if SHOULD_RESTART_LINE.swap(false, Ordering::SeqCst) {
            log::info!("restart-line — bouncing DSL line");
            let _ = board.line_config_down();
            std::thread::sleep(Duration::from_secs(1));
            let _ = board.line_config_up(
                cfg.modulation, cfg.annex, cfg.profiles, cfg.bitswap, cfg.sra,
            );
            tc_emitted = false;
        }

        // Poll board (op 2)
        match board.get_line_obj() {
            Ok(line) => {
                let status = line.link_status;

                if status == LinkStatus::Up && up_since.is_none() {
                    up_since = Some(Instant::now());
                } else if status != LinkStatus::Up {
                    up_since = None;
                }

                {
                    let mut st = state.lock().unwrap();
                    st.line_obj = Some(line.clone());
                    st.uptime_secs = up_since.map(|t| t.elapsed().as_secs()).unwrap_or(0);
                    st.xfer_mode = Some(cfg.xfer_mode);
                }

                if Some(status) != last_status {
                    let event = link_status_to_event(status);
                    log::info!("line state: {status:?} → {event:?}");
                    if let Some(script) = notify_script {
                        hotplug::emit_status(script, event);
                    }
                    if status == LinkStatus::Up && !tc_emitted {
                        if let Some(script) = notify_script {
                            hotplug::emit_tc_layer(script, cfg.xfer_mode);
                        }
                        tc_emitted = true;
                    }
                    last_status = Some(status);
                }
            }
            Err(BoardError::Timeout) => {}
            Err(e) => log::error!("poll error: {e}"),
        }

        // Poll ubus (non-blocking)
        if let Some(conn) = ubus_conn.as_mut() {
            let _ = conn.poll_one();
        }

        std::thread::sleep(poll_interval);
    }

    // 10. Clean shutdown
    log::info!("shutting down");
    if let Some(script) = notify_script {
        hotplug::emit_status(script, LineEvent::Down);
    }
    let _ = board.line_config_down();
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

    #[test]
    fn link_status_events() {
        assert_eq!(link_status_to_event(LinkStatus::Up), LineEvent::Up);
        assert_eq!(link_status_to_event(LinkStatus::NoSignal), LineEvent::Down);
        assert_eq!(link_status_to_event(LinkStatus::Initializing), LineEvent::Training);
        assert_eq!(link_status_to_event(LinkStatus::EstablishingLink), LineEvent::Handshake);
    }

    #[test]
    fn config_change_detection() {
        let a = DslConfig::default();
        let b = DslConfig { annex: rbctl_proto::pack::Annex::A, ..DslConfig::default() };
        assert!(config_changed(&a, &b));
    }

    #[test]
    fn config_no_change() {
        let a = DslConfig::default();
        let b = DslConfig::default();
        assert!(!config_changed(&a, &b));
    }
}
