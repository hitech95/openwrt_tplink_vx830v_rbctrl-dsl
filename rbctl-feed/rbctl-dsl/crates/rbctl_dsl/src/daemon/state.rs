//! Daemon shared state — the blackboard between the worker (writer)
//! and the daemon / ubus object (readers).
//!
//! `DslState` holds the latest op2 poll, derived display fields, firmware
//! status, and reachability bookkeeping. All access goes through the
//! [`SharedState`] mutex.

use std::sync::{Arc, Mutex};

use rbctl_proto::oem::pack::{Annex, Modulation};
use rbctl_proto::oem::unpack::LineObj;

use crate::config::XferMode;

// ── shared state ─────────────────────────────────────────────────────────

/// Live DSL metrics shared between the board poller and the ubus handler.
#[derive(Clone)]
pub struct DslState {
    /// Latest op 2 reply, or `None` if no successful poll yet (or stale).
    pub line_obj: Option<LineObj>,
    /// Seconds since the line first reached `Up` (0 if not up).
    pub uptime_secs: u64,
    /// The configured xfer mode (for `.mode` field).
    pub xfer_mode: Option<XferMode>,
    /// Configured modulation / annex — mirrored from `DslConfig` so IPC/ubus
    /// status readers never need to borrow the config (which lives on the
    /// board worker thread). Refreshed on reload.
    pub modulation: Modulation,
    pub annex: Annex,
    /// Firmware upgrade status.
    pub fw_status: FwStatus,
    /// Instant of the last successful op2 poll, or None if never.
    pub last_poll_ok: Option<std::time::Instant>,
    /// True if op2 has succeeded at least once since startup.
    pub board_ever_seen: bool,
}

/// Firmware upgrade lifecycle status, queryable via IPC `status` and ubus.
#[derive(Debug, Clone)]
pub enum FwStatus {
    Idle,
    Upgrading,
    UpgradingProgress { stage: u8, pct: u8 },
    Done,
    Failed(String),
}

impl Default for FwStatus {
    fn default() -> Self {
        Self::Idle
    }
}

impl FwStatus {
    pub fn as_str(&self) -> String {
        match self {
            Self::Idle => "idle".into(),
            Self::Upgrading => "upgrading".into(),
            Self::UpgradingProgress { stage, pct } => {
                format!("upgrading (stage={stage}, pct={pct}%)")
            }
            Self::Done => "done".into(),
            Self::Failed(msg) => format!("failed: {msg}"),
        }
    }
}

impl Default for DslState {
    fn default() -> Self {
        Self {
            line_obj: None,
            uptime_secs: 0,
            xfer_mode: None,
            modulation: Modulation::Vdsl2,
            annex: Annex::B,
            fw_status: FwStatus::default(),
            last_poll_ok: None,
            board_ever_seen: false,
        }
    }
}

pub type SharedState = Arc<Mutex<DslState>>;

pub fn new_shared_state() -> SharedState {
    Arc::new(Mutex::new(DslState::default()))
}


// (object builder lives in daemon/ubus_object.rs)
