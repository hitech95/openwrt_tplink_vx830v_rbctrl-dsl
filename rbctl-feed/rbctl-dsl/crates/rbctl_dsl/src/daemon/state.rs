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
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// Serialize to the IPC wire token. Grammar (parseable by [`from_wire`]):
    ///
    /// ```text
    /// Idle
    /// Upgrading
    /// UpgradingProgress stage=<u8> pct=<u8>
    /// Done
    /// Failed <free-text message>     // message = everything after the first space
    /// ```
    pub fn to_wire(&self) -> String {
        match self {
            Self::Idle => "Idle".into(),
            Self::Upgrading => "Upgrading".into(),
            Self::UpgradingProgress { stage, pct } => {
                format!("UpgradingProgress stage={stage} pct={pct}")
            }
            Self::Done => "Done".into(),
            Self::Failed(msg) => format!("Failed {msg}"),
        }
    }

    /// Parse a wire token back into an [`FwStatus`] (inverse of [`to_wire`]).
    pub fn from_wire(s: &str) -> Option<Self> {
        let (token, rest) = match s.split_once(' ') {
            Some((t, r)) => (t, r.trim_start()),
            None => (s, ""),
        };
        match token {
            "Idle" => Some(Self::Idle),
            "Upgrading" => Some(Self::Upgrading),
            "Done" => Some(Self::Done),
            "Failed" => Some(Self::Failed(rest.to_string())),
            "UpgradingProgress" => {
                let mut stage = None;
                let mut pct = None;
                for kv in rest.split_whitespace() {
                    match kv.split_once('=') {
                        Some(("stage", v)) => stage = v.parse().ok(),
                        Some(("pct", v)) => pct = v.parse().ok(),
                        _ => return None,
                    }
                }
                match (stage, pct) {
                    (Some(stage), Some(pct)) => {
                        Some(Self::UpgradingProgress { stage, pct })
                    }
                    _ => None,
                }
            }
            _ => None,
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

#[cfg(test)]
mod tests {
    use super::FwStatus;

    #[test]
    fn fw_status_wire_roundtrip() {
        for s in [
            FwStatus::Idle,
            FwStatus::Upgrading,
            FwStatus::UpgradingProgress { stage: 2, pct: 45 },
            FwStatus::Done,
            FwStatus::Failed("board rejected image".into()),
        ] {
            assert_eq!(FwStatus::from_wire(&s.to_wire()), Some(s));
        }
    }

    #[test]
    fn fw_status_wire_exact_forms() {
        assert_eq!(FwStatus::Idle.to_wire(), "Idle");
        assert_eq!(
            FwStatus::UpgradingProgress { stage: 3, pct: 90 }.to_wire(),
            "UpgradingProgress stage=3 pct=90"
        );
        assert_eq!(
            FwStatus::Failed("timeout".into()).to_wire(),
            "Failed timeout"
        );
    }

    #[test]
    fn fw_status_rejects_garbage() {
        assert!(FwStatus::from_wire("idle").is_none()); // legacy lowercase
        assert!(FwStatus::from_wire("Bogus").is_none());
        assert!(FwStatus::from_wire("UpgradingProgress stage=1").is_none()); // missing pct
    }
}


// (object builder lives in daemon/ubus_object.rs)
