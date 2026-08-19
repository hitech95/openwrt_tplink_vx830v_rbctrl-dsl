//! Daemon startup phase — everything that must happen before the control
//! loop runs: config load + validation, management-link probe, board
//! socket open. Pure setup, no loop, no IPC.

use rbctl_proto::oem::validate;

use crate::board::Board;
use crate::constants;
use crate::mgmt_link;
use crate::uci_cfg::{CliOverrides, DslConfig, XferMode};

/// Result of a successful startup — the ingredients the control loop needs.
pub(crate) struct Init {
    pub board: Board,
    pub cfg: DslConfig,
    /// Transport VLAN id = base + 2000 (requested; board may assign another).
    pub transport_vlan: u16,
    /// Parent interface of the config iface (host-side VLAN anchor).
    pub parent: String,
}

/// Run the startup sequence. Returns `Err(exit_code)` on fatal errors.
pub(crate) fn startup(
    config_iface: &str,
    overrides: &CliOverrides,
) -> Result<Init, i32> {
    // 1. Load config (UCI + CLI overrides)
    let cfg = match DslConfig::load(overrides) {
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
        return Err(1);
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
            return Err(1);
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
        return Err(1);
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
            return Err(1);
        }
    };
    let mut board: Board = Board::new(sock);
    board.set_timeout(constants::BOARD_DEFAULT_TIMEOUT);
    board.set_retries(constants::BOARD_DEFAULT_RETRIES);
    log::info!("board socket on {config_iface} (mac={:02x?})", board.local_mac());

    let parent = mgmt_link::parent_of(config_iface)
        .unwrap_or(config_iface)
        .to_string();

    Ok(Init { board, cfg, transport_vlan, parent })
}
