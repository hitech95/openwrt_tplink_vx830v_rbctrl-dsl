//! Status mapping helpers — LuCI presentation values and Lantiq ABI
//! numbers for the canonical [`LineState`] (defined in `rbctl_proto`,
//! shared by the board protocol, the IPC wire, and ubus).
//!
//! Both tables key off [`LineState`] directly, so IPC and ubus can never
//! disagree.

use rbctl_proto::oem::unpack::LineState;

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

/// [`LineState`] → LuCI display label (ubus ABI strings, shared
/// with the status client).
pub fn state_label(state: LineState) -> &'static str {
    match state {
        LineState::NotInitialized => "Not initialized",
        LineState::Idle => "Idle",
        LineState::Silent => "Silent",
        LineState::Handshake => "Handshake",
        LineState::FullInit => "Full init",
        LineState::ShowtimeNoSync => "Showtime without TC-Layer sync",
        LineState::ShowtimeTcSync => "Showtime with TC-Layer sync",
        LineState::Unknown(_) => "Unknown",
    }
}

/// [`LineState`] → LSTATE_MAP ABI number (ubus `state_num`).
pub fn lstate_num(state: LineState) -> i32 {
    match state {
        LineState::NotInitialized => lstate::NOT_INITIALIZED,
        LineState::Idle => lstate::IDLE,
        LineState::Silent => lstate::SILENT,
        LineState::Handshake => lstate::HANDSHAKE,
        LineState::FullInit => lstate::FULL_INIT,
        LineState::ShowtimeNoSync => lstate::SHOWTIME_NO_SYNC,
        LineState::ShowtimeTcSync => lstate::SHOWTIME_TC_SYNC,
        LineState::Unknown(_) => lstate::UNKNOWN,
    }
}

/// [`LineState`] → (LuCI state string, LSTATE_MAP enum, up bool).
/// Composite for the ubus object.
pub fn map_link_status(status: LineState) -> (&'static str, i32, bool) {
    (state_label(status), lstate_num(status), status.is_up())
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
