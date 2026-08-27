//! Protobuf protocol — self-describing messages on magic `0x50`.
//!
//! This is the new wire protocol that replaces the legacy OEM binary opcodes.
//! Frames carry protobuf-encoded `Request` / `Response` / `Event` messages.
//! The magic byte `0x50` (ASCII 'P') distinguishes protobuf frames from OEM
//! frames (`0x11`/`0x10`) on the same EtherType, allowing coexistence during
//! migration.
//!
//! The generated Rust types live in [`rbctl`] (compiled from `proto/rbctl.proto`
//! via `prost-build` + `protox`).

pub mod codec;
pub mod framing;
pub mod pack;
pub mod unpack;

pub use framing::{FRAG_FLAG, MAGIC_PROTOBUF};

pub mod rbctl {
    include!(concat!(env!("OUT_DIR"), "/rbctl.rs"));
}

#[cfg(all(test, feature = "protobuf"))]
mod tests {
    use prost::Message;
    // Round-trips of the semantics that replace the OEM quirks:
    // double-apply open, unreadable config cache, client-side retrain,
    // blind link provisioning. Plus the status/metrics/link split.
    use super::rbctl::{self as pb};

    fn roundtrip(req: pb::Request) -> pb::Request {
        let bytes = pb::Request::encode_to_vec(&req);
        pb::Request::decode(bytes.as_slice()).expect("decode")
    }

    #[test]
    fn line_open_replaces_double_apply() {
        // The daemon's OpenLine (OEM: two op1s differing in bitswap)
        // becomes ONE line_open carrying the config.
        let req = pb::Request {
            id: 7,
            payload: Some(pb::request::Payload::Config(pb::ConfigRequest {
                action: Some(pb::config_request::Action::LineOpen(pb::LineOpen {
                    config: Some(pb::LineConfig {
                        modulation: pb::ModulationType::Vdsl2 as i32,
                        annex: pb::AnnexType::B as i32,
                        bitswap: true,
                        sra: true,
                        vdsl2_profile: 0x1ff,
                        ..Default::default()
                    }),
                })),
            })),
        };
        let back = roundtrip(req);
        let pb::request::Payload::Config(cfg) = back.payload.unwrap() else {
            panic!("wrong payload");
        };
        let pb::config_request::Action::LineOpen(open) = cfg.action.unwrap() else {
            panic!("wrong action");
        };
        let line = open.config.unwrap();
        assert!(line.bitswap && line.sra);
        assert_eq!(line.modulation, pb::ModulationType::Vdsl2 as i32);
    }

    #[test]
    fn open_response_reports_what_happened() {
        // opened/applied tell the host whether the line was already open
        // and whether the config deduplicated (OEM status 0 was blind).
        let resp = pb::Response {
            id: 7,
            ok: true,
            error: String::new(),
            payload: Some(pb::response::Payload::Config(pb::ConfigResponse {
                applied: false, // cache hit
                opened: true,   // line was closed, this call opened it
            })),
        };
        let back = pb::Response::decode(pb::Response::encode_to_vec(&resp).as_slice()).unwrap();
        let pb::response::Payload::Config(cfg) = back.payload.unwrap() else {
            panic!("wrong payload");
        };
        assert!(!cfg.applied, "cache hit must be distinguishable");
        assert!(cfg.opened);
    }

    #[test]
    fn closed_is_part_of_line_status() {
        // Admin up/down lives in the status enum: CLOSED (not training)
        // vs NO_SIGNAL (operational down while open).
        let resp = pb::Response {
            id: 9,
            ok: true,
            error: String::new(),
            payload: Some(pb::response::Payload::LineStatus(pb::LineStatusResponse {
                link_state: pb::LinkState::Closed as i32,
                modulation: pb::ModulationType::Vdsl2 as i32,
                uptime_secs: 0,
                ..Default::default()
            })),
        };
        let back = pb::Response::decode(pb::Response::encode_to_vec(&resp).as_slice()).unwrap();
        let pb::response::Payload::LineStatus(st) = back.payload.unwrap() else {
            panic!("wrong payload");
        };
        assert_eq!(st.link_state, pb::LinkState::Closed as i32);
    }

    #[test]
    fn basic_status_stays_small() {
        // The poll tier carries state + trained modulation only; the
        // heavy fields must live in line_metrics.
        let resp = pb::Response {
            id: 12,
            ok: true,
            error: String::new(),
            payload: Some(pb::response::Payload::LineStatus(pb::LineStatusResponse {
                link_state: pb::LinkState::Showtime as i32,
                modulation: pb::ModulationType::Vdsl2 as i32,
                data_path: pb::DataPath::Ptm as i32,
                annex: pb::AnnexType::B as i32,
                uptime_secs: 3600,
            })),
        };
        let bytes = pb::Response::encode_to_vec(&resp);
        assert!(bytes.len() < 40, "basic status must stay frame-sized: {}", bytes.len());
        let back = pb::Response::decode(bytes.as_slice()).unwrap();
        let pb::response::Payload::LineStatus(st) = back.payload.unwrap() else {
            panic!("wrong payload");
        };
        assert_eq!(st.uptime_secs, 3600);
    }

    #[test]
    fn metrics_carry_the_full_set() {
        // line_metrics absorbs the old line_stats + channel_stats.
        let resp = pb::Response {
            id: 13,
            ok: true,
            error: String::new(),
            payload: Some(pb::response::Payload::LineMetrics(pb::LineMetricsResponse {
                channel_blocks: Some(pb::ChannelBlocks {
                    rx_blocks: 42,
                    tx_blocks: 24,
                    ..Default::default()
                }),
                total_act_seconds: 100,
                current_act_seconds: 10,
                ..Default::default()
            })),
        };
        let back = pb::Response::decode(pb::Response::encode_to_vec(&resp).as_slice()).unwrap();
        let pb::response::Payload::LineMetrics(m) = back.payload.unwrap() else {
            panic!("wrong payload");
        };
        let blocks = m.channel_blocks.unwrap();
        assert_eq!((blocks.rx_blocks, blocks.tx_blocks), (42, 24));
        assert_eq!((m.total_act_seconds, m.current_act_seconds), (100, 10));
    }

    #[test]
    fn retrain_is_one_message() {
        let req = pb::Request {
            id: 8,
            payload: Some(pb::request::Payload::Config(pb::ConfigRequest {
                action: Some(pb::config_request::Action::LineRetrain(
                    pb::LineOp {},
                )),
            })),
        };
        let back = roundtrip(req);
        let pb::request::Payload::Config(cfg) = back.payload.unwrap() else {
            panic!("wrong payload");
        };
        assert!(matches!(
            cfg.action.unwrap(),
            pb::config_request::Action::LineRetrain(_)
        ));
    }

    #[test]
    fn link_create_roundtrips() {
        // Data links are their own request (out of ConfigRequest), keyed
        // by VLAN, ATM carrying its own parameter set.
        let req = pb::Request {
            id: 14,
            payload: Some(pb::request::Payload::Link(pb::LinkRequest {
                action: Some(pb::link_request::Action::Create(pb::LinkCreate {
                    vlan_id: 2001,
                    kind: Some(pb::link_create::Kind::Atm(pb::AtmLinkParams {
                        vpi: 8,
                        vci: 35,
                        qos: 1,
                        ..Default::default()
                    })),
                })),
            })),
        };
        let back = roundtrip(req);
        let pb::request::Payload::Link(l) = back.payload.unwrap() else {
            panic!("wrong payload");
        };
        let pb::link_request::Action::Create(c) = l.action.unwrap() else {
            panic!("wrong action");
        };
        assert_eq!(c.vlan_id, 2001);
        let pb::link_create::Kind::Atm(atm) = c.kind.unwrap() else {
            panic!("wrong kind");
        };
        assert_eq!((atm.vpi, atm.vci), (8, 35));
    }

    #[test]
    fn link_response_reports_queueing() {
        // link_create before showtime is queued agent-side (like the OEM
        // board queues provisioning until showtime); queued=true says the
        // link is deferred, not active.
        let resp = pb::Response {
            id: 14,
            ok: true,
            error: String::new(),
            payload: Some(pb::response::Payload::Link(pb::LinkResponse {
                vlan_id: 2001,
                queued: true,
            })),
        };
        let back = pb::Response::decode(pb::Response::encode_to_vec(&resp).as_slice()).unwrap();
        let pb::response::Payload::Link(l) = back.payload.unwrap() else {
            panic!("wrong payload");
        };
        assert!(l.queued);
        assert_eq!(l.vlan_id, 2001);
    }

    #[test]
    fn config_readback_reports_queued_links() {
        // get_config returns the active line config + the link list with
        // per-entry queued state (requested vs materialized) — the
        // authoritative answer where "line is up" is only approximate.
        let resp = pb::Response {
            id: 11,
            ok: true,
            error: String::new(),
            payload: Some(pb::response::Payload::ConfigData(pb::ConfigDataResponse {
                config: Some(pb::LineConfig {
                    modulation: pb::ModulationType::Vdsl2 as i32,
                    ..Default::default()
                }),
                links: vec![
                    pb::LinkEntry {
                        vlan_id: 2000,
                        kind: Some(pb::link_entry::Kind::Ptm(pb::PtmLinkParams {
                            tag_vid: 100,
                            vlan_mode: pb::VlanMode::Tagged as i32,
                            ..Default::default()
                        })),
                        queued: false, // materialized
                    },
                    pb::LinkEntry {
                        vlan_id: 2001,
                        kind: Some(pb::link_entry::Kind::Atm(pb::AtmLinkParams {
                            vpi: 8,
                            vci: 35,
                            ..Default::default()
                        })),
                        queued: true, // deferred until showtime
                    },
                ],
            })),
        };
        let back = pb::Response::decode(pb::Response::encode_to_vec(&resp).as_slice()).unwrap();
        let pb::response::Payload::ConfigData(data) = back.payload.unwrap() else {
            panic!("wrong payload");
        };
        assert_eq!(data.config.unwrap().modulation, pb::ModulationType::Vdsl2 as i32);
        assert_eq!(data.links.len(), 2);
        assert_eq!(data.links[0].vlan_id, 2000);
        assert!(!data.links[0].queued, "active link must read back as not queued");
        assert!(data.links[1].queued, "deferred link must read back as queued");
        assert!(matches!(
            data.links[1].kind.unwrap(),
            pb::link_entry::Kind::Atm(_)
        ));
    }

    #[test]
    fn multimode_is_config_only() {
        // line_config accepts MULTIMODE (auto-detect, OEM code 7); line
        // status never reports it — the trained modulation is concrete.
        let req = pb::Request {
            id: 16,
            payload: Some(pb::request::Payload::Config(pb::ConfigRequest {
                action: Some(pb::config_request::Action::LineOpen(pb::LineOpen {
                    config: Some(pb::LineConfig {
                        modulation: pb::ModulationType::Multimode as i32,
                        annex: pb::AnnexType::B as i32,
                        ..Default::default()
                    }),
                })),
            })),
        };
        let back = roundtrip(req);
        let pb::request::Payload::Config(cfg) = back.payload.unwrap() else {
            panic!("wrong payload");
        };
        let pb::config_request::Action::LineOpen(open) = cfg.action.unwrap() else {
            panic!("wrong action");
        };
        assert_eq!(
            open.config.unwrap().modulation,
            pb::ModulationType::Multimode as i32
        );
        // ...while status carries the concrete trained value.
        assert_ne!(
            pb::ModulationType::Multimode as i32,
            pb::ModulationType::Vdsl2 as i32
        );
    }

    #[test]
    fn link_active_event_announces_materialization() {
        // The push for the data plane: a queued link going live at
        // showtime (vlan + kind), so the daemon can fire its hotplug at
        // the actual materialization moment instead of inferring it
        // from the line state.
        let ev = pb::Event {
            payload: Some(pb::event::Payload::LinkActive(pb::LinkActiveEvent {
                vlan_id: 2001,
                kind: pb::DataPath::Atm as i32,
                timestamp: 1_700_000_000,
            })),
        };
        let back = pb::Event::decode(pb::Event::encode_to_vec(&ev).as_slice()).unwrap();
        let pb::event::Payload::LinkActive(a) = back.payload.unwrap() else {
            panic!("wrong payload");
        };
        assert_eq!(a.vlan_id, 2001);
        assert_eq!(a.kind, pb::DataPath::Atm as i32);
    }

    #[test]
    fn shell_flow_control_roundtrips() {
        // Credits ride the opposite direction's data messages; the
        // standalone ShellFlow control is window-exempt (no deadlock).
        let req = pb::Request {
            id: 17,
            payload: Some(pb::request::Payload::Shell(pb::ShellRequest {
                action: Some(pb::shell_request::Action::Flow(pb::ShellFlow {
                    session_id: 1,
                    stdout_acked: 8192,
                })),
            })),
        };
        let back = roundtrip(req);
        let pb::request::Payload::Shell(sh) = back.payload.unwrap() else {
            panic!("wrong payload");
        };
        let pb::shell_request::Action::Flow(f) = sh.action.unwrap() else {
            panic!("wrong action");
        };
        assert_eq!(f.stdout_acked, 8192);

        let resp = pb::Response {
            id: 17,
            ok: true,
            error: String::new(),
            payload: Some(pb::response::Payload::Shell(pb::ShellResponse {
                session_id: 1,
                stdin_consumed: 4096,
                ..Default::default()
            })),
        };
        let back = pb::Response::decode(pb::Response::encode_to_vec(&resp).as_slice()).unwrap();
        let pb::response::Payload::Shell(sh) = back.payload.unwrap() else {
            panic!("wrong payload");
        };
        assert_eq!(sh.stdin_consumed, 4096);
    }

    #[test]
    fn spectrum_absorbs_diagnostics() {
        // fresh=true triggers a measurement; in_progress says refetch.
        let resp = pb::Response {
            id: 15,
            ok: true,
            error: String::new(),
            payload: Some(pb::response::Payload::Spectrum(pb::SpectrumResponse {
                in_progress: true,
                ..Default::default()
            })),
        };
        let back = pb::Response::decode(pb::Response::encode_to_vec(&resp).as_slice()).unwrap();
        let pb::response::Payload::Spectrum(s) = back.payload.unwrap() else {
            panic!("wrong payload");
        };
        assert!(s.in_progress);
    }
    #[test]
    fn ptm_vlan_mode_semantics() {
        use crate::model::{PtmParams, PtmVlanMode};
        use pb::VlanMode;

        // Unspecified stays unspecified on the wire (agent resolves to
        // untagged; the schema documents the default).
        let dflt = PtmParams::default().to_pb();
        assert_eq!(dflt.vlan_mode, VlanMode::Unspecified as i32);

        // Explicit mode 2 (transparent/QinQ) survives the roundtrip —
        // unreachable via OEM op15, first-class here.
        let qinq = PtmParams { vlan_mode: PtmVlanMode::Transparent, ..Default::default() };
        let back = PtmParams::from_pb(&qinq.to_pb());
        assert_eq!(back.vlan_mode, PtmVlanMode::Transparent);

        // Tagged carries the service tag and the smux mode number.
        let tagged = PtmParams {
            tag_vid: 100,
            tag_pri: 5,
            vlan_mode: PtmVlanMode::Tagged,
        };
        let p = tagged.to_pb();
        assert_eq!(p.vlan_mode, VlanMode::Tagged as i32);
        assert_eq!((p.tag_vid, p.tag_pri), (100, 5));
        assert_eq!(PtmVlanMode::Tagged.smux_mode(), 3);
        assert_eq!(PtmVlanMode::Transparent.smux_mode(), 2);
        assert_eq!(PtmVlanMode::Untagged.smux_mode(), 1);
        assert_eq!(PtmVlanMode::Unspecified.or_default_untagged(), PtmVlanMode::Untagged);
    }

}
