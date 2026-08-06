//! Hotplug event emitter — forks the DSL notify script on line-state
//! transitions, per [openwrt.md](../../docs/openwrt.md) §3.2.
//!
//! The daemon sets `DSL_NOTIFICATION_TYPE` + `DSL_INTERFACE_STATUS` (or
//! `DSL_TC_LAYER_STATUS`) environment variables and execs the `-n` script
//! path. The script in turn calls `/sbin/hotplug-call dsl`, which triggers
//! `led_dsl.sh` and `pppoa.sh`.

use std::process::Command;

use crate::uci_cfg::XferMode;

/// Line-state events mapped to hotplug `DSL_INTERFACE_STATUS` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEvent {
    Down,
    Handshake,
    Training,
    Up,
}

impl LineEvent {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Down => "DOWN",
            Self::Handshake => "HANDSHAKE",
            Self::Training => "TRAINING",
            Self::Up => "UP",
        }
    }
}

/// TC-layer mode mapped to hotplug `DSL_TC_LAYER_STATUS` values.
///
/// Note: PTM is reported as `EFM`, not `PTM` — historical quirk preserved
/// for compatibility with `10_atm.sh` / `10_ptm.sh` hooks.
pub fn tc_layer_status(xfer_mode: XferMode) -> &'static str {
    match xfer_mode {
        XferMode::Atm => "ATM",
        XferMode::Ptm => "EFM",
    }
}

/// Emit a `DSL_INTERFACE_STATUS` notification by forking the notify script.
///
/// Non-fatal: errors are logged to stderr but don't crash the daemon.
pub fn emit_status(notify_script: &str, event: LineEvent) {
    let result = Command::new(notify_script)
        .env("DSL_NOTIFICATION_TYPE", "DSL_INTERFACE_STATUS")
        .env("DSL_INTERFACE_STATUS", event.as_str())
        .spawn();
    if let Err(e) = result {
        eprintln!("[hotplug] failed to exec {notify_script}: {e}");
    }
}

/// Emit a `DSL_STATUS` (TC-layer) notification.
pub fn emit_tc_layer(notify_script: &str, xfer_mode: XferMode) {
    let result = Command::new(notify_script)
        .env("DSL_NOTIFICATION_TYPE", "DSL_STATUS")
        .env("DSL_TC_LAYER_STATUS", tc_layer_status(xfer_mode))
        .spawn();
    if let Err(e) = result {
        eprintln!("[hotplug] failed to exec {notify_script}: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_strings() {
        assert_eq!(LineEvent::Up.as_str(), "UP");
        assert_eq!(LineEvent::Down.as_str(), "DOWN");
        assert_eq!(LineEvent::Handshake.as_str(), "HANDSHAKE");
        assert_eq!(LineEvent::Training.as_str(), "TRAINING");
    }

    #[test]
    fn tc_layer_efm_not_ptm() {
        assert_eq!(tc_layer_status(XferMode::Ptm), "EFM");
        assert_eq!(tc_layer_status(XferMode::Atm), "ATM");
    }
}
