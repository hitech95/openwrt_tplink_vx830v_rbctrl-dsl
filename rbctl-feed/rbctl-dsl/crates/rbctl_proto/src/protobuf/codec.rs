//! Message ↔ frames codec — the protobuf transport shim for the daemon
//! (and the agent).
//!
//! Mirrors how the OEM side splits responsibilities: `oem::mod` builds
//! frames, the daemon's `Board` drives the socket (seq, retries,
//! TX-echo drain). Here, [`pack`] builds typed messages, this module
//! turns them into frames and decodes incoming frames back into
//! messages — including transparent reassembly of fragmented ones.
//!
//! ```text
//!     pack::line_open(id, &params)          // typed message
//!       │  codec::encode_request(...)       // → Vec<frame>
//!       ▼  socket send/recv (daemon-owned)
//!     codec::IncomingDecoder::feed(frame)   // → Incoming::Response(..)
//!       ▼
//!     unpack::…                             // native structs
//! ```

use prost::Message;

use super::framing::{self, FrameError, MessageType};
use super::rbctl::{self as pb};

/// Errors from the codec layer.
#[derive(Debug, PartialEq, Eq)]
pub enum CodecError {
    /// Framing/CRC/fragment-subheader problem (see [`FrameError`]).
    Frame(FrameError),
    /// The reassembled/whole payload is not a decodable protobuf message
    /// of the type the frame's `message_type` announced. A well-behaved
    /// peer never sends this; treat as protocol violation.
    Malformed,
}

impl From<FrameError> for CodecError {
    fn from(e: FrameError) -> Self {
        Self::Frame(e)
    }
}

/// Encode a request into one or more frames (see
/// [`framing::encode_message`]).
pub fn encode_request(
    dst_mac: &[u8; 6],
    src_mac: &[u8; 6],
    seq: u32,
    req: &pb::Request,
    max_frame_payload: usize,
) -> Result<Vec<Vec<u8>>, CodecError> {
    encode_typed(dst_mac, src_mac, MessageType::Request, seq, req, max_frame_payload)
}

/// Encode a response (agent / fake-board side).
pub fn encode_response(
    dst_mac: &[u8; 6],
    src_mac: &[u8; 6],
    seq: u32,
    resp: &pb::Response,
    max_frame_payload: usize,
) -> Result<Vec<Vec<u8>>, CodecError> {
    encode_typed(dst_mac, src_mac, MessageType::Response, seq, resp, max_frame_payload)
}

/// Encode an async event (agent / fake-board side).
pub fn encode_event(
    dst_mac: &[u8; 6],
    src_mac: &[u8; 6],
    seq: u32,
    ev: &pb::Event,
    max_frame_payload: usize,
) -> Result<Vec<Vec<u8>>, CodecError> {
    encode_typed(dst_mac, src_mac, MessageType::Event, seq, ev, max_frame_payload)
}

fn encode_typed<T: Message>(
    dst_mac: &[u8; 6],
    src_mac: &[u8; 6],
    mt: MessageType,
    seq: u32,
    msg: &T,
    max_frame_payload: usize,
) -> Result<Vec<Vec<u8>>, CodecError> {
    let payload = msg.encode_to_vec();
    Ok(framing::encode_message(dst_mac, src_mac, mt, seq, &payload, max_frame_payload)?)
}

/// A decoded incoming message.
#[derive(Debug, PartialEq)]
pub enum Incoming {
    Request(pb::Request),
    Response(pb::Response),
    Event(pb::Event),
}

/// Incremental decoder for a stream of received frames.
///
/// Feed every received frame (in any interleaved order — responses,
/// events and fragments of several sequences may share the wire); whole
/// messages and completed reassemblies come back as [`Some(Incoming)`].
/// The reassembly timeout follows the framing spec (caller drives
/// [`Self::expire`] from its poll loop and the sender's retry resends
/// the whole message).
pub struct IncomingDecoder {
    reassembler: framing::Reassembler,
}

impl IncomingDecoder {
    pub fn new() -> Self {
        Self { reassembler: framing::Reassembler::new(framing::DEFAULT_REASSEMBLY_TIMEOUT) }
    }

    /// Parse one frame; returns the decoded message when this frame
    /// completed it.
    pub fn feed(&mut self, bytes: &[u8]) -> Result<Option<Incoming>, CodecError> {
        let parsed = framing::parse_frame(bytes)?;
        match parsed.kind {
            framing::PayloadKind::Whole(payload) => decode_as(parsed.message_type, payload),
            framing::PayloadKind::Fragment(frag) => {
                let seq = parsed.seq;
                match self.reassembler.feed(seq, &frag)? {
                    Some(msg) => decode_as(parsed.message_type, &msg),
                    None => Ok(None),
                }
            }
        }
    }

    /// Drop partial reassemblies past the timeout; returns expired
    /// sequence numbers (diagnostics).
    pub fn expire(&mut self) -> Vec<u32> {
        self.reassembler.expire()
    }
}

impl Default for IncomingDecoder {
    fn default() -> Self {
        Self::new()
    }
}

fn decode_as(mt: MessageType, payload: &[u8]) -> Result<Option<Incoming>, CodecError> {
    let incoming = match mt {
        MessageType::Request => Incoming::Request(pb::Request::decode(payload).map_err(|_| CodecError::Malformed)?),
        MessageType::Response => Incoming::Response(pb::Response::decode(payload).map_err(|_| CodecError::Malformed)?),
        MessageType::Event => Incoming::Event(pb::Event::decode(payload).map_err(|_| CodecError::Malformed)?),
    };
    Ok(Some(incoming))
}

#[cfg(test)]
mod tests {
    use prost::Message;

    use super::super::pack;
    use super::super::unpack;
    use super::*;

    const DST: [u8; 6] = [0x02, 0, 0, 0, 0, 1];
    const SRC: [u8; 6] = [0x02, 0, 0, 0, 0, 2];

    /// Full loop: typed request → frames → decoder → typed request,
    /// with a payload budget forcing multi-frame messages.
    #[test]
    fn request_roundtrip_fragmented() {
        use crate::model::{Annex, Modulation, Vdsl2Profiles};
        let params = pack::LineParams {
            modulation: Modulation::Vdsl2,
            annex: Annex::B,
            bitswap: true,
            sra: true,
            profiles: Vdsl2Profiles(0x1ff),
        };
        let req = pack::line_open(7, &params);
        let frames = encode_request(&DST, &SRC, 42, &req, 16).unwrap();
        assert!(frames.len() > 1, "small budget must fragment: {} bytes", req.encoded_len());

        let mut dec = IncomingDecoder::new();
        let mut got = None;
        for f in &frames {
            if let Some(Incoming::Request(r)) = dec.feed(f).unwrap() {
                got = Some(r);
            }
        }
        let got = got.expect("message must complete");
        assert_eq!(got, req);
        let pb::request::Payload::Config(cfg) = got.payload.unwrap() else { panic!() };
        let pb::config_request::Action::LineOpen(open) = cfg.action.unwrap() else { panic!() };
        assert_eq!(open.config.unwrap().vdsl2_profile, 0x1ff);
    }

    #[test]
    fn response_with_metrics_roundtrip() {
        let mut m = pb::LineMetricsResponse::default();
        m.current_rate = Some(pb::DirectionalU32 { downstream: 93000, upstream: 20000 });
        m.noise_margin = Some(pb::DirectionalU32 { downstream: 63, upstream: 55 });
        m.current_act_seconds = 120;
        m.total_act_seconds = 6000;
        let resp = pack::response_line_metrics(3, m);

        let frames = encode_response(&DST, &SRC, 3, &resp, 48).unwrap();
        let mut dec = IncomingDecoder::new();
        let mut got = None;
        for f in &frames {
            if let Some(Incoming::Response(r)) = dec.feed(f).unwrap() {
                got = Some(r);
            }
        }
        let got = got.expect("complete");
        assert_eq!(got.id, 3);
        assert!(got.ok);
        let pb::response::Payload::LineMetrics(m) = got.payload.unwrap() else { panic!() };

        // …and the native conversion the daemon would do.
        let native = unpack::metrics_from_pb(&m);
        assert_eq!(native.down_rate, 93000);
        assert_eq!(native.up_rate, 20000);
        assert_eq!(native.down_noise_margin, 63);
        assert_eq!(native.uptime_showtime_secs, 120);
        assert_eq!(native.uptime_total_secs, 6000);
    }

    #[test]
    fn interleaved_sequences_demultiplex() {
        // A response stream with an event pushed between two fragments of
        // another sequence — the decoder must not conflate them.
        let resp = pack::response_config(5, true, true);
        let ev = pack::event_link_active(2000, crate::model::DataPath::Ptm);

        let mut wire = Vec::new();
        wire.extend(encode_response(&DST, &SRC, 10, &resp, 40).unwrap());
        wire.extend(encode_event(&DST, &SRC, 11, &ev, 24).unwrap()); // interleaved

        let mut dec = IncomingDecoder::new();
        let mut results = Vec::new();
        for f in &wire {
            if let Some(inc) = dec.feed(f).unwrap() {
                results.push(inc);
            }
        }
        assert_eq!(results.len(), 2, "two complete messages");
        assert!(matches!(&results[0], Incoming::Response(r) if r.id == 5));
        assert!(matches!(&results[1], Incoming::Event(_)));
    }

    #[test]
    fn error_response_splits() {
        let err = pack::response_error(9, "link_exists", "PTM link on vlan 2000 already active");
        let (prefix, desc) = unpack::split_error(&err).expect("error response");
        assert_eq!(prefix, "link_exists");
        assert_eq!(desc, "PTM link on vlan 2000 already active");
        assert!(unpack::split_error(&pack::response_config(9, true, false)).is_none());
    }

    #[test]
    fn basic_status_to_native() {
        let s = pb::LineStatusResponse {
            link_state: pb::LinkState::Showtime as i32,
            modulation: pb::ModulationType::Vdsl2 as i32,
            data_path: pb::DataPath::Ptm as i32,
            annex: pb::AnnexType::B as i32,
            uptime_secs: 30,
        };
        let native = unpack::BasicStatus::from_pb(&s);
        assert_eq!(native.native_state(), crate::model::LineState::ShowtimeTcSync);
        assert_eq!(native.modulation, Some(crate::model::Modulation::Vdsl2));
        assert_eq!(native.data_path, crate::model::DataPath::Ptm);
        assert!(native.is_up() && !native.is_closed());

        // CLOSED maps to the native "silent" operational reality…
        let s = pb::LineStatusResponse { link_state: pb::LinkState::Closed as i32, ..Default::default() };
        let native = unpack::BasicStatus::from_pb(&s);
        assert_eq!(native.native_state(), crate::model::LineState::Silent);
        assert!(native.is_closed() && !native.is_up());
    }

    #[test]
    fn config_readback_to_native() {
        let c = pb::ConfigDataResponse {
            config: Some(pb::LineConfig {
                modulation: pb::ModulationType::Multimode as i32,
                annex: pb::AnnexType::Auto as i32,
                bitswap: true,
                sra: false,
                vdsl2_profile: 0x1ff,
                ..Default::default()
            }),
            links: vec![pb::LinkEntry {
                vlan_id: 2001,
                kind: Some(pb::link_entry::Kind::Atm(pb::AtmLinkParams {
                    vpi: 8,
                    vci: 35,
                    ..Default::default()
                })),
                queued: true,
            }],
        };
        let native = unpack::ConfigData::from_pb(&c);
        let line = native.line.expect("line params");
        assert_eq!(line.modulation, crate::model::Modulation::Multimode);
        assert_eq!(line.annex, crate::model::Annex::Auto);
        assert_eq!(native.links.len(), 1);
        assert!(matches!(native.links[0].kind, unpack::LinkKind::Atm(_)));
        assert!(native.links[0].queued);
        assert_eq!(native.links[0].vlan, 2001);
    }

    #[test]
    fn oem_params_survive_the_pb_roundtrip() {
        // The pack→unpack identity for every native value the daemon
        // sends: if this holds, the backend switch is transparent.
        use crate::model::{Annex, Modulation, Vdsl2Profiles};
        for m in [
            Modulation::AdslAnsiT1413, Modulation::AdslGdmt, Modulation::AdslGlite,
            Modulation::AdslGdmtBis, Modulation::Adsl2Plus,
            Modulation::Vdsl2, Modulation::Multimode,
        ] {
            assert_eq!(pack::modulation_from_pb(pack::modulation_to_pb(m)), Some(m), "{m:?}");
        }
        // The one documented fold: the pb vocabulary has no ADSL-only
        // multimode, so AdslMultimode widens to the general Multimode.
        assert_eq!(
            pack::modulation_from_pb(pack::modulation_to_pb(Modulation::AdslMultimode)),
            Some(Modulation::Multimode)
        );
        // The full firmware annex table round-trips — the pb vocabulary
        // is a complete mirror (no folds on annex).
        for a in [
            Annex::A, Annex::B, Annex::I, Annex::M, Annex::J, Annex::Auto,
            Annex::AL, Annex::ALM, Annex::BJ,
        ] {
            assert_eq!(pack::annex_from_pb(pack::annex_to_pb(a).unwrap()), Some(a), "{a:?}");
        }
        let params = pack::LineParams {
            modulation: Modulation::Vdsl2,
            annex: Annex::B,
            bitswap: true,
            sra: false,
            profiles: Vdsl2Profiles(0x1ff),
        };
        assert_eq!(pack::LineParams::from_pb(&params.to_pb()), Some(params));
    }
}
