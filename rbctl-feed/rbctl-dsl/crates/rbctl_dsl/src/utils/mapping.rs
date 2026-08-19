//! Status mapping helpers — translate board protocol enums to LuCI/IPC
//! display strings and Lantiq ABI enum values.
//!
//! These were previously in `ubus_obj.rs` but are used by both the IPC
//! status snapshot (daemon.rs) and the ubus metrics builder.

use rbctl_proto::oem::unpack::LinkStatus;

// ── LSTATE_MAP ABI enum values (from dsl_cpe_ubus.c, do not renumber) ────
//
// IDLE / SHOWTIME_NO_SYNC are unused by this board's state machine but are
// part of the Lantiq ABI surface — kept for completeness.

#[allow(dead_code)]
pub mod lstate {
    pub const UNKNOWN: i32 = -1;
    pub const NOT_INITIALIZED: i32 = 0;
    pub const IDLE: i32 = 2;
    pub const SILENT: i32 = 3;
    pub const HANDSHAKE: i32 = 4;
    pub const FULL_INIT: i32 = 5;
    pub const SHOWTIME_NO_SYNC: i32 = 6;
    pub const SHOWTIME_TC_SYNC: i32 = 7;
}

/// Map board [`LinkStatus`] → (LuCI state string, LSTATE_MAP enum, up bool).
pub fn map_link_status(status: LinkStatus) -> (&'static str, i32, bool) {
    match status {
        LinkStatus::NoSignal => ("Silent", lstate::SILENT, false),
        LinkStatus::EstablishingLink => ("Handshake", lstate::HANDSHAKE, false),
        LinkStatus::Initializing => ("Full init", lstate::FULL_INIT, false),
        LinkStatus::Up => ("Showtime with TC-Layer sync", lstate::SHOWTIME_TC_SYNC, true),
        LinkStatus::Unknown(_) => ("Unknown", lstate::UNKNOWN, false),
    }
}

/// Map board annex code → single-letter string for display.
pub fn annex_string(code: u8) -> &'static str {
    match code {
        0 => "A",
        1 => "B",
        2 => "I",
        3 => "M",
        6 => "J",
        _ => "?",
    }
}
