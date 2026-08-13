//! Read-only management interface status.
//!
//! The daemon never alters management interfaces — this module only
//! inspects them. Write operations (bring-up, VLAN create) live in the
//! init script and selftest respectively.

use std::ffi::CString;

/// Snapshot of a management interface's state at a point in time.
/// All fields are read from sysfs / ioctls — no side effects.
#[derive(Debug, Clone)]
pub struct MgmtLinkStatus {
    /// The VLAN sub-interface the daemon communicates over (e.g. "lan0.500").
    pub iface: String,
    /// The parent physical interface, if the name contains '.' (e.g. "lan0").
    pub parent: Option<String>,
    /// Whether the interface name resolves to an index (if_nametoindex).
    pub exists: bool,
    /// operstate ("up", "down", "unknown") — None if unreadable.
    pub operstate: Option<String>,
    /// Carrier present on the config interface.
    pub carrier: bool,
    /// Carrier present on the parent (None if no parent).
    pub parent_carrier: Option<bool>,
}

impl MgmtLinkStatus {
    /// Probe the current state. Reads sysfs only — no writes, no ioctls
    /// that change state.
    pub fn probe(config_iface: &str) -> Self {
        let parent = parent_of(config_iface).map(str::to_string);
        let exists = iface_exists(config_iface);
        let carrier = read_carrier(config_iface).unwrap_or(false);
        let parent_carrier = parent.as_deref().and_then(read_carrier);
        let operstate = read_operstate(config_iface);
        Self {
            iface: config_iface.to_string(),
            parent,
            exists,
            operstate,
            carrier,
            parent_carrier,
        }
    }

    /// Interface exists AND has carrier on both itself and its parent.
    pub fn is_ready(&self) -> bool {
        self.exists && self.carrier && self.parent_carrier.unwrap_or(true)
    }

    /// True if the parent exists but lacks carrier (most common failure).
    pub fn parent_down(&self) -> bool {
        self.parent_carrier == Some(false)
    }

    /// Human-readable one-liner for log/error messages.
    pub fn diagnostic(&self) -> String {
        if !self.exists {
            return format!("'{}' does not exist", self.iface);
        }
        if self.parent_down() {
            return format!(
                "no carrier on parent '{}' — ensure it is up (dsl_control / netifd / board.d)",
                self.parent.as_deref().unwrap_or("?")
            );
        }
        if !self.carrier {
            return format!("no carrier on '{}' (parent ok)", self.iface);
        }
        "ok".into()
    }
}

/// Extract parent name: `"lan0.500"` → `Some("lan0")`, `"eth0"` → `None`.
pub fn parent_of(iface: &str) -> Option<&str> {
    iface.rsplit_once('.').map(|(p, _)| p)
}

/// `if_nametoindex` wrapper — true if the interface exists.
pub fn iface_exists(name: &str) -> bool {
    CString::new(name)
        .map(|c| unsafe { libc::if_nametoindex(c.as_ptr()) } != 0)
        .unwrap_or(false)
}

/// Read `/sys/class/net/<iface>/carrier` → `Some(true/false)`, `None` on error.
pub fn read_carrier(iface: &str) -> Option<bool> {
    read_sysfs_u32(iface, "carrier").map(|v| v == 1)
}

/// Read `/sys/class/net/<iface>/operstate` → `Some("up"/"down"/...)`.
pub fn read_operstate(iface: &str) -> Option<String> {
    std::fs::read_to_string(format!("/sys/class/net/{iface}/operstate"))
        .ok()
        .map(|s| s.trim().to_string())
}

fn read_sysfs_u32(iface: &str, attr: &str) -> Option<u32> {
    std::fs::read_to_string(format!("/sys/class/net/{iface}/{attr}"))
        .ok()
        .and_then(|s| s.trim().parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_of_vlan() {
        assert_eq!(parent_of("lan0.500"), Some("lan0"));
        assert_eq!(parent_of("eth0.1.500"), Some("eth0.1"));
    }

    #[test]
    fn parent_of_no_dot() {
        assert_eq!(parent_of("eth0"), None);
        assert_eq!(parent_of(""), None);
    }

    #[test]
    fn diagnostic_not_exists() {
        let s = MgmtLinkStatus {
            iface: "eth9.999".into(),
            parent: Some("eth9".into()),
            exists: false,
            operstate: None,
            carrier: false,
            parent_carrier: None,
        };
        assert!(s.diagnostic().contains("does not exist"));
    }

    #[test]
    fn diagnostic_parent_down() {
        let s = MgmtLinkStatus {
            iface: "lan0.500".into(),
            parent: Some("lan0".into()),
            exists: true,
            operstate: Some("down".into()),
            carrier: false,
            parent_carrier: Some(false),
        };
        let msg = s.diagnostic();
        assert!(msg.contains("no carrier on parent 'lan0'"));
        assert!(!s.is_ready());
        assert!(s.parent_down());
    }

    #[test]
    fn diagnostic_no_carrier_parent_ok() {
        let s = MgmtLinkStatus {
            iface: "lan0.500".into(),
            parent: Some("lan0".into()),
            exists: true,
            operstate: Some("down".into()),
            carrier: false,
            parent_carrier: Some(true),
        };
        let msg = s.diagnostic();
        assert!(msg.contains("no carrier on 'lan0.500'"));
        assert!(msg.contains("parent ok"));
        assert!(!s.is_ready());
    }

    #[test]
    fn diagnostic_ready() {
        let s = MgmtLinkStatus {
            iface: "lan0.500".into(),
            parent: Some("lan0".into()),
            exists: true,
            operstate: Some("up".into()),
            carrier: true,
            parent_carrier: Some(true),
        };
        assert_eq!(s.diagnostic(), "ok");
        assert!(s.is_ready());
        assert!(!s.parent_down());
    }

    #[test]
    fn diagnostic_no_parent() {
        let s = MgmtLinkStatus {
            iface: "eth0".into(),
            parent: None,
            exists: true,
            operstate: Some("up".into()),
            carrier: true,
            parent_carrier: None,
        };
        assert!(s.is_ready());
        assert_eq!(s.diagnostic(), "ok");
    }
}
