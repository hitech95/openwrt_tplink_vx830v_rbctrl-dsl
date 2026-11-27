//! ubus `dsl` object — publishes live DSL metrics on the ubus bus per
//! [openwrt.md](../../docs/openwrt.md) §4.
//!
//! Two methods:
//! - `metrics` — full line status (state, rates, SNR, attenuation)
//! - `statistics` — returns empty (board exposes no per-tone data, §4.4)
//!
//! The `metrics` schema follows the Lantiq ABI: field names and enum values
//! match `dsl_cpe_ubus.c` so LuCI works unmodified.

use std::sync::Arc;

use rbctl_proto::oem::unpack::{LineObj, LineMetrics};
use ubus::blobmsg::{BlobBuilder, BlobMsgTable};
use ubus::error::UbusError;
use ubus::server::UbusObject;

use crate::utils::mapping;

use super::state::{DslState, SharedState};

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
            let (state_str, state_num, up) = mapping::map_link_status(line.link_status);

            bb.put_str(Some("state"), state_str);
            bb.put_i32(Some("state_num"), state_num);
            bb.put_bool(Some("up"), up);
            bb.put_i64(Some("uptime"), st.uptime_secs as i64);

            // Mode / standard / annex / profile
            let mode = mode_string(line);
            bb.put_str(Some("mode"), &mode);
            bb.put_str(Some("annex"), mapping::annex_string(line.annex_code));

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
            bb.put_i32(Some("state_num"), mapping::lstate::NOT_INITIALIZED);
            bb.put_bool(Some("up"), false);
            bb.put_i64(Some("uptime"), 0);
        }
    }

    bb.close_table();
    Ok(bb.finish_table())
}

fn mode_string(line: &LineObj) -> String {
    match line.modulation_code {
        6 => "G.993.2 (VDSL2)".to_string(),
        4 => "G.992.5 (ADSL2+)".to_string(),
        c => format!("G.992.x (modulation {c})"),
    }
}

fn profile_string(profiles: rbctl_proto::oem::pack::Vdsl2Profiles) -> String {
    use rbctl_proto::oem::pack::Vdsl2Profiles as P;
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

fn datarate(metric: u32) -> i32 {
    (metric as i32) * 1000
}

fn decibel(metric: u32) -> f64 {
    metric as f64 / 10.0
}

/// Add upstream or downstream metrics sub-table.
fn add_direction(bb: &mut BlobBuilder, name: &str, m: &LineMetrics, upstream: bool) {
    bb.open_table(Some(name));

    let (rate, attndr, snr, attn, power) = if upstream {
        (m.up_rate, m.up_attainable_rate, m.up_noise_margin, m.up_attenuation, m.up_output_power)
    } else {
        (m.down_rate, m.down_attainable_rate, m.down_noise_margin, m.down_attenuation, m.down_output_power)
    };

    // Match the Lantiq ubus ABI: rates are bps, while signal and power
    // measurements are floating-point dB/dBm rather than tenths.
    bb.put_i32(Some("data_rate"), datarate(rate));
    bb.put_i32(Some("attndr"), datarate(attndr));
    bb.put_double(Some("snr"), decibel(snr));
    bb.put_double(Some("latn"), decibel(attn));
    bb.put_double(Some("satn"), decibel(attn));
    bb.put_double(Some("actatp"), decibel(power));
    bb.put_i32(Some("interleave_delay"), 0);
    bb.put_i32(Some("inp"), 0);

    bb.close_table();
}

// ── tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ubus_metrics_use_lantiq_units() {
        assert_eq!(datarate(100_000), 100_000_000);
        assert_eq!(decibel(63), 6.3);
        assert_eq!(decibel(142), 14.2);
    }
    use crate::config::XferMode;
    use rbctl_proto::oem::unpack::{DataPath, LineState};

    use super::super::state::new_shared_state;

    #[test]
    fn map_status_up() {
        let (s, n, up) = mapping::map_link_status(LineState::ShowtimeTcSync);
        assert_eq!(s, "Showtime with TC-Layer sync");
        assert!(up);
        assert_eq!(n, mapping::lstate::SHOWTIME_TC_SYNC);
        assert!(up);
    }

    #[test]
    fn map_status_nosignal() {
        let (s, n, up) = mapping::map_link_status(LineState::Silent);
        assert_eq!(s, "Silent");
        assert!(!up);
        assert_eq!(n, mapping::lstate::SILENT);
        assert!(!up);
    }

    #[test]
    fn map_status_handshake() {
        let (s, n, _) = mapping::map_link_status(LineState::Handshake);
        assert_eq!(s, "Handshake");
        assert_eq!(n, mapping::lstate::HANDSHAKE);
    }

    #[test]
    fn map_status_training() {
        let (s, n, _) = mapping::map_link_status(LineState::FullInit);
        assert_eq!(s, "Full init");
        assert_eq!(n, mapping::lstate::FULL_INIT);
    }

    #[test]
    fn profile_string_multi() {
        use rbctl_proto::oem::pack::Vdsl2Profiles as P;
        let s = profile_string(P::EIGHT_A | P::SEVENTEEN_A | P::THIRTY_A);
        assert_eq!(s, "8a 17a 30a");
    }

    #[test]
    fn annex_string_basic() {
        assert_eq!(mapping::annex_string(0), "A");
        assert_eq!(mapping::annex_string(1), "B");
        assert_eq!(mapping::annex_string(99), "?");
    }

    #[test]
    fn build_metrics_with_line_obj() {
        let st = DslState {
            line_obj: Some(LineObj {
                status: 0,
                link_status: LineState::ShowtimeTcSync,
                modulation_code: 6,
                data_path: DataPath::Ptm,
                annex_code: 1,
                vdsl2_profile_bitmask: 0x040,
                is_atm: false,
                uptime_secs: 300,
                metrics: LineMetrics {
                    down_rate: 50000,
                    up_rate: 10000,
                    ..Default::default()
                },
            }),
            uptime_secs: 300,
            xfer_mode: Some(XferMode::Ptm),
            ..Default::default()
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
