//! Daemon main loop — ties together board control, UCI config, hotplug
//! events, and the ubus `dsl` object into a single event loop.
//!
//! ## Architecture
//!
//! ```text
//! rbctl-dsl daemon
//! ├── init: load UCI → open board → line_config_up → link_add → VLAN create
//! ├── ubus: register "dsl" object (optional — degrades gracefully)
//! └── loop (1 s):
//!     ├── board.get_line_obj() → update shared state
//!     ├── detect state transition → emit hotplug event
//!     ├── ubus.poll_one() → serve "metrics" / "statistics"
//!     ├── check SIGTERM → clean shutdown
//!     └── check SIGHUP → reload UCI config
//! ```
//!
//! No threads — everything runs in the main thread. The ubus transport is
//! non-blocking, and the board poll is synchronous with a short timeout.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rbctl_proto::pack::{AtmEncap, AtmLinkParams, AtmLinkType, AtmQos};
use rbctl_proto::unpack::LinkStatus;

use crate::board::{Board, BoardError};
use crate::hotplug::{self, LineEvent};
use crate::logger::Logger;
use crate::uci_cfg::{AtmConfig, DslConfig, XferMode};
use crate::ubus_obj;

// ── signal flags ─────────────────────────────────────────────────────────

static SHOULD_EXIT: AtomicBool = AtomicBool::new(false);
static SHOULD_RELOAD: AtomicBool = AtomicBool::new(false);

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

/// Map board [`LinkStatus`] → hotplug [`LineEvent`].
fn link_status_to_event(status: LinkStatus) -> LineEvent {
    match status {
        LinkStatus::Up => LineEvent::Up,
        LinkStatus::Initializing => LineEvent::Training,
        LinkStatus::EstablishingLink => LineEvent::Handshake,
        LinkStatus::NoSignal | LinkStatus::Unknown(_) => LineEvent::Down,
    }
}

/// Derive the parent physical interface from a VLAN interface name.
///
/// `lan0.500` → `lan0`; `eth0` → `eth0` (no VLAN suffix).
fn parent_iface(iface: &str) -> &str {
    match iface.rfind('.') {
        Some(i) => &iface[..i],
        None => iface,
    }
}

/// Build [`AtmLinkParams`] from config (or defaults).
fn atm_params(atm: Option<&AtmConfig>, vlan_id: u16) -> AtmLinkParams<'static> {
    let a = atm.cloned().unwrap_or(AtmConfig {
        vpi: 8,
        vci: 35,
        encap: AtmEncap::Llc,
        link_type: AtmLinkType::Eoa,
        qos: AtmQos::Ubr,
        pcr: 0,
    });
    AtmLinkParams {
        vpi: a.vpi,
        vci: a.vci,
        encap: a.encap,
        link_type: a.link_type,
        qos: a.qos,
        pcr: a.pcr,
        scr: 0,
        mbs: 0,
        vlan_id,
        tag_enable: 0,
        tag_vid: 0xffff,
        tag_pri: 0xff,
        _phantom: std::marker::PhantomData,
    }
}

/// Create a VLAN sub-interface via `ip link` (idempotent — ignores "exists").
fn create_vlan_iface(parent: &str, vlan_id: u16) -> Result<(), String> {
    let name = format!("{parent}.{vlan_id}");
    let output = std::process::Command::new("ip")
        .args([
            "link", "add", "link", parent,
            "name", &name, "type", "vlan",
            "id", &vlan_id.to_string(),
        ])
        .output()
        .map_err(|e| e.to_string())?;

    if output.status.success() {
        // Bring it up
        let _ = std::process::Command::new("ip")
            .args(["link", "set", "up", "dev", &name])
            .output();
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("exists") || stderr.contains("File exists") {
        Ok(())
    } else {
        Err(stderr.trim().to_string())
    }
}

fn delete_vlan_iface(parent: &str, vlan_id: u16) {
    let _ = std::process::Command::new("ip")
        .args(["link", "del", &format!("{parent}.{vlan_id}")])
        .output();
}

/// Connect to ubusd and register the `dsl` object. Returns `None` on failure.
fn connect_ubus(
    obj: ubus::server::UbusObject,
) -> Option<ubus::server::UbusConnection<crate::transport::UnixUbusTransport>> {
    let transport = crate::transport::UnixUbusTransport::connect("/var/run/ubus.sock").ok()?;
    ubus::server::UbusConnection::connect_and_register(transport, obj).ok()
}

fn config_changed(old: &DslConfig, new: &DslConfig) -> bool {
    old.modulation != new.modulation
        || old.annex != new.annex
        || old.profiles.bitmask() != new.profiles.bitmask()
        || old.xfer_mode != new.xfer_mode
}

// ── daemon entry point ───────────────────────────────────────────────────

/// Run the daemon. Returns process exit code.
///
/// - `config_iface`: management VLAN interface (e.g. `lan0.500`)
/// - `notify_script`: path to `-n` hotplug notify script (e.g. `/sbin/dsl_notify.sh`)
/// - `transport_vlan`: board transport VLAN id (e.g. 2001)
pub fn run(config_iface: &str, notify_script: Option<&str>, transport_vlan: u16) -> i32 {
    let log = Logger::new("daemon");
    install_signal_handlers();

    // 1. Load UCI config
    let mut cfg = match DslConfig::load() {
        Ok(c) => {
            log.line(format!(
                "UCI: mod={:?} annex={:?} profiles=0x{:x} xfer={:?}",
                c.modulation, c.annex, c.profiles.bitmask(), c.xfer_mode
            ));
            c
        }
        Err(e) => {
            log.line(format!("UCI load failed ({e}), using defaults"));
            DslConfig::default()
        }
    };

    // 2. Open board socket
    let sock = match af_packet::RawSocket::open(config_iface, 0x88B5) {
        Ok(s) => s,
        Err(e) => {
            log.fail("socket", format!("open {config_iface}: {e}"));
            return 1;
        }
    };
    let mut board: Board = Board::new(sock);
    board.set_timeout(Duration::from_millis(2000));
    board.set_retries(3);
    log.line(format!("board socket on {config_iface} (mac={:02x?})", board.local_mac()));

    // 3. Configure DSL line (op 1)
    log.line(format!(
        "line_config_up: mod={:?} annex={:?} profiles=0x{:x}",
        cfg.modulation, cfg.annex, cfg.profiles.bitmask()
    ));
    if let Err(e) = board.line_config_up(cfg.modulation, cfg.annex, cfg.profiles) {
        log.fail("line_config_up", &e);
    }

    // 4. Add data-plane link (op 5 ATM / op 15 PTM)
    let vlan = match cfg.xfer_mode {
        XferMode::Ptm => {
            log.line(format!("ptm_link_add: vlan={transport_vlan}"));
            board.ptm_link_add(0, 0, 0, transport_vlan).unwrap_or(transport_vlan)
        }
        XferMode::Atm => {
            let a = cfg.atm.as_ref();
            log.line(format!(
                "atm_link_add: vpi={} vci={} encap={:?} vlan={transport_vlan}",
                a.map(|a| a.vpi).unwrap_or(8),
                a.map(|a| a.vci).unwrap_or(35),
                a.map(|a| a.encap).unwrap_or(AtmEncap::Llc),
            ));
            let params = atm_params(cfg.atm.as_ref(), transport_vlan);
            board.atm_link_add(&params).unwrap_or(transport_vlan)
        }
    };
    log.line(format!("board assigned transport vlan: {vlan}"));

    // 5. Create host-side transport VLAN interface
    let parent = parent_iface(config_iface);
    match create_vlan_iface(parent, vlan) {
        Ok(()) => log.line(format!("created {parent}.{vlan}")),
        Err(e) => log.line(format!("VLAN create {parent}.{vlan}: {e} (may already exist)")),
    }

    // 6. Register ubus object (optional — degrades gracefully)
    let state = ubus_obj::new_shared_state();
    {
        let mut st = state.lock().unwrap();
        st.xfer_mode = Some(cfg.xfer_mode);
    }
    let ubus_obj = ubus_obj::build_dsl_object(Arc::clone(&state));
    let mut ubus_conn = match connect_ubus(ubus_obj) {
        Some(c) => {
            log.line("ubus object 'dsl' registered");
            Some(c)
        }
        None => {
            log.line("ubus unavailable — metrics will not be published");
            None
        }
    };

    // 7. Emit initial TC-layer status
    if let Some(script) = notify_script {
        hotplug::emit_tc_layer(script, cfg.xfer_mode);
    }

    // 8. Main loop
    let poll_interval = Duration::from_secs(1);
    let mut last_status: Option<LinkStatus> = None;
    let mut up_since: Option<Instant> = None;
    let mut tc_emitted = false;

    log.line("entering poll loop");
    while !SHOULD_EXIT.load(Ordering::SeqCst) {
        // Reload config on SIGHUP
        if SHOULD_RELOAD.swap(false, Ordering::SeqCst) {
            log.line("SIGHUP — reloading UCI config");
            if let Ok(new_cfg) = DslConfig::load() {
                if config_changed(&cfg, &new_cfg) {
                    log.line("line params changed — reconfiguring");
                    let _ = board.line_config_down();
                    let _ = board.line_config_up(
                        new_cfg.modulation, new_cfg.annex, new_cfg.profiles,
                    );
                    cfg = new_cfg;
                    tc_emitted = false;
                } else {
                    log.line("no line-param changes");
                }
            }
        }

        // Poll board (op 2)
        match board.get_line_obj() {
            Ok(line) => {
                let status = line.link_status;

                // Track uptime
                if status == LinkStatus::Up && up_since.is_none() {
                    up_since = Some(Instant::now());
                } else if status != LinkStatus::Up {
                    up_since = None;
                }

                // Update shared state for ubus
                {
                    let mut st = state.lock().unwrap();
                    st.line_obj = Some(line.clone());
                    st.uptime_secs = up_since.map(|t| t.elapsed().as_secs()).unwrap_or(0);
                    st.xfer_mode = Some(cfg.xfer_mode);
                }

                // Emit hotplug events on transitions
                if Some(status) != last_status {
                    let event = link_status_to_event(status);
                    log.line(format!("line state: {status:?} → {event:?}"));
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
            Err(BoardError::Timeout) => {
                // Board didn't respond in time — keep polling
            }
            Err(e) => {
                log.line(format!("poll error: {e}"));
            }
        }

        // Poll ubus (non-blocking)
        if let Some(conn) = ubus_conn.as_mut() {
            let _ = conn.poll_one();
        }

        std::thread::sleep(poll_interval);
    }

    // 9. Clean shutdown
    log.line("shutting down");
    if let Some(script) = notify_script {
        hotplug::emit_status(script, LineEvent::Down);
    }
    let _ = board.line_config_down();
    delete_vlan_iface(parent, vlan);

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
