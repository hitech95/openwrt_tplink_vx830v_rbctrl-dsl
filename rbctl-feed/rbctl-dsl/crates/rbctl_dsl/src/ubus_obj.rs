//! ubus `dsl` object — publishes live DSL metrics on the ubus bus per
//! [openwrt.md](../../docs/openwrt.md) §4.
//!
//! Two methods:
//! - `metrics` — full line status (state, rates, SNR, attenuation)
//! - `statistics` — returns empty (board exposes no per-tone data, §4.4)
//!
//! The `metrics` schema follows the Lantiq ABI: field names and enum values
//! match `dsl_cpe_ubus.c` so LuCI works unmodified.

use std::sync::{Arc, Mutex};

use rbctl_proto::unpack::{LineObj, LineMetrics, LinkStatus};
use ubus::server::UbusObject;
use ubus::blobmsg::{BlobBuilder, BlobMsgTable};
use ubus::error::UbusError;

use crate::uci_cfg::XferMode;

// ── LSTATE_MAP ABI enum values (from dsl_cpe_ubus.c, do not renumber) ────

mod lstate {
    pub const UNKNOWN: i32 = -1;
    pub const NOT_INITIALIZED: i32 = 0;
    pub const IDLE: i32 = 2;
    pub const SILENT: i32 = 3;
    pub const HANDSHAKE: i32 = 4;
    pub const FULL_INIT: i32 = 5;
    pub const SHOWTIME_NO_SYNC: i32 = 6;
    pub const SHOWTIME_TC_SYNC: i32 = 7;
}

// ── shared state ─────────────────────────────────────────────────────────

/// Live DSL metrics shared between the board poller and the ubus handler.
#[derive(Clone, Default)]
pub struct DslState {
    /// Latest op 2 reply, or `None` if no successful poll yet.
    pub line_obj: Option<LineObj>,
    /// Seconds since the line first reached `Up` (0 if not up).
    pub uptime_secs: u64,
    /// The configured xfer mode (for `.mode` field).
    pub xfer_mode: Option<XferMode>,
}

pub type SharedState = Arc<Mutex<DslState>>;

pub fn new_shared_state() -> SharedState {
    Arc::new(Mutex::new(DslState::default()))
}

// ── object builder ───────────────────────────────────────────────────────

/// Build the `dsl` ubus object with `metrics` and `statistics` methods.
///
/// The handlers capture a clone of the [`SharedState`] arc, so they always
/// return the latest polled data.
pub fn build_dsl_object(state: SharedState) -> UbusObject {
    let metrics_state = Arc::clone(&state);
    let stats_state = Arc::clone(&state);

    UbusObject::new("dsl")
        .method("metrics", move |_args| {
            let st = metrics_state.lock().unwrap();
            build_metrics_reply(&st)
        })
        .method("statistics", move |_args| {
            // Board exposes no per-tone data (openwrt.md §4.4).
            // Return an empty table — LuCI degrades gracefully.
            let _ = &stats_state;
            Ok(BlobMsgTable::empty())
        })
}

// ── metrics reply builder ────────────────────────────────────────────────

fn build_metrics_reply(st: &DslState) -> Result<BlobMsgTable, UbusError> {
    let mut bb = BlobBuilder::new();
    bb.open_table(None);

    match &st.line_obj {
        Some(line) => {
            let (state_str, state_num, up) = map_link_status(line.link_status);

            bb.put_str(Some("state"), state_str);
            bb.put_i32(Some("state_num"), state_num);
            bb.put_bool(Some("up"), up);
            bb.put_i64(Some("uptime"), st.uptime_secs as i64);

            // Mode / standard / annex / profile
            let mode = mode_string(line);
            bb.put_str(Some("mode"), &mode);
            bb.put_str(Some("annex"), annex_string(line.annex_code));

            if let Some(profiles) = line.vdsl2_profiles() {
                bb.put_str(Some("profile"), &profile_string(profiles));
            }

            // Direction metrics
            add_direction(&mut bb, "upstream", &line.metrics, true);
            add_direction(&mut bb, "downstream", &line.metrics, false);

            // Version info (static placeholders — board doesn't report these)
            bb.put_str(Some("api_version"), "4.0");
            bb.put_str(Some("chipset"), "EcoNet EN75xx");
        }
        None => {
            bb.put_str(Some("state"), "Not initialized");
            bb.put_i32(Some("state_num"), lstate::NOT_INITIALIZED);
            bb.put_bool(Some("up"), false);
            bb.put_i64(Some("uptime"), 0);
        }
    }

    bb.close_table();
    Ok(bb.finish_table())
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

fn mode_string(line: &LineObj) -> String {
    match line.modulation_code {
        6 => "G.993.2 (VDSL2)".to_string(),
        4 => "G.992.5 (ADSL2+)".to_string(),
        c => format!("G.992.x (modulation {c})"),
    }
}

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

fn profile_string(profiles: rbctl_proto::pack::Vdsl2Profiles) -> String {
    use rbctl_proto::pack::Vdsl2Profiles as P;
    let mut parts: Vec<&str> = Vec::new();
    if profiles.0 & P::EIGHT_A.0 != 0 { parts.push("8a"); }
    if profiles.0 & P::EIGHT_B.0 != 0 { parts.push("8b"); }
    if profiles.0 & P::EIGHT_C.0 != 0 { parts.push("8c"); }
    if profiles.0 & P::EIGHT_D.0 != 0 { parts.push("8d"); }
    if profiles.0 & P::TWELVE_A.0 != 0 { parts.push("12a"); }
    if profiles.0 & P::TWELVE_B.0 != 0 { parts.push("12b"); }
    if profiles.0 & P::SEVENTEEN_A.0 != 0 { parts.push("17a"); }
    if profiles.0 & P::THIRTY_A.0 != 0 { parts.push("30a"); }
    if profiles.0 & P::THIRTYFIVE_B.0 != 0 { parts.push("35b"); }
    parts.join(" ")
}

/// Add upstream or downstream metrics sub-table.
fn add_direction(bb: &mut BlobBuilder, name: &str, m: &LineMetrics, upstream: bool) {
    bb.open_table(Some(name));

    let (rate, max_rate, snr, attn, power, attndr) = if upstream {
        (m.up_curr_rate, m.up_max_rate, m.up_snr_margin, m.up_attenuation, 0, m.up_rate)
    } else {
        (m.down_curr_rate, m.down_max_rate, m.down_snr_margin, m.down_attenuation, 0, m.down_rate)
    };

    bb.put_i32(Some("data_rate"), rate as i32);
    bb.put_i32(Some("max_data_rate"), max_rate as i32);
    bb.put_i32(Some("snr"), snr as i32);
    bb.put_i32(Some("latn"), attn as i32);
    bb.put_i32(Some("satn"), attn as i32);
    bb.put_i32(Some("actatp"), power as i32);
    bb.put_i32(Some("attndr"), attndr as i32);
    bb.put_i32(Some("interleave_delay"), 0);
    bb.put_i32(Some("inp"), 0);

    bb.close_table();
}

// ── tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rbctl_proto::unpack::LinkStatus;

    #[test]
    fn map_status_up() {
        let (s, n, up) = map_link_status(LinkStatus::Up);
        assert_eq!(s, "Showtime with TC-Layer sync");
        assert_eq!(n, lstate::SHOWTIME_TC_SYNC);
        assert!(up);
    }

    #[test]
    fn map_status_nosignal() {
        let (s, n, up) = map_link_status(LinkStatus::NoSignal);
        assert_eq!(s, "Silent");
        assert_eq!(n, lstate::SILENT);
        assert!(!up);
    }

    #[test]
    fn map_status_handshake() {
        let (s, n, _) = map_link_status(LinkStatus::EstablishingLink);
        assert_eq!(s, "Handshake");
        assert_eq!(n, lstate::HANDSHAKE);
    }

    #[test]
    fn map_status_training() {
        let (s, n, _) = map_link_status(LinkStatus::Initializing);
        assert_eq!(s, "Full init");
        assert_eq!(n, lstate::FULL_INIT);
    }

    #[test]
    fn profile_string_multi() {
        use rbctl_proto::pack::Vdsl2Profiles as P;
        let s = profile_string(P::EIGHT_A | P::SEVENTEEN_A | P::THIRTY_A);
        assert_eq!(s, "8a 17a 30a");
    }

    #[test]
    fn annex_string_basic() {
        assert_eq!(annex_string(0), "A");
        assert_eq!(annex_string(1), "B");
        assert_eq!(annex_string(99), "?");
    }

    #[test]
    fn build_metrics_with_line_obj() {
        let st = DslState {
            line_obj: Some(LineObj {
                status: 0,
                link_status: LinkStatus::Up,
                modulation_code: 6,
                annex_code: 1,
                vdsl2_profile_bitmask: 0x040,
                metrics: LineMetrics {
                    down_curr_rate: 50000,
                    up_curr_rate: 10000,
                    ..Default::default()
                },
            }),
            uptime_secs: 300,
            xfer_mode: Some(XferMode::Ptm),
        };
        let reply = build_metrics_reply(&st).unwrap();
        assert!(!reply.contents().is_empty());
    }

    #[test]
    fn build_metrics_no_line_obj() {
        let st = DslState::default();
        let reply = build_metrics_reply(&st).unwrap();
        assert!(!reply.contents().is_empty());
    }

    #[test]
    fn build_dsl_object_methods() {
        let state = new_shared_state();
        let obj = build_dsl_object(state);
        assert_eq!(obj.name, "dsl");
        assert_eq!(obj.methods.len(), 2);
        assert_eq!(obj.methods[0].name, "metrics");
        assert_eq!(obj.methods[1].name, "statistics");
    }
}
