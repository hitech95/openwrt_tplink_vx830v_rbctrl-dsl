//! Protobuf L2 framing — encode, fragment, parse, reassemble.
//!
//! Builds on the protocol-agnostic 24-byte header in [`crate::frame`]
//! (same layout and CRC-16/ARC coverage as the OEM protocol). The
//! protobuf-specific parts are the byte at `0x0F` and fragmentation:
//!
//! ```text
//! 0x0E  magic — always 0x50 ('P'). Constant: it is the protocol/version
//!       demux slot (OEM 0x11/0x10 vs protobuf 0x50) and is NEVER modified
//!       by flags, so the "is this ours?" check stays an exact compare.
//! 0x0F  message_type byte:
//!         bit 7   (0x80) = fragmented-payload flag
//!         bits 0-6       = MessageType (1=REQUEST 2=RESPONSE 3=EVENT)
//!       One read of this byte yields dispatch class AND fragmentation
//!       state — the fast path the flag bit exists for.
//! ```
//!
//! A fragmented frame's payload starts with an 8-byte sub-header:
//!
//! ```text
//! +0x00  2  fragment_index (u16 BE, 0-based)
//! +0x02  2  fragment_count (u16 BE, total frames in the message)
//! +0x04  4  message_length (u32 BE, reassembled size)
//! +0x08  .. fragment bytes
//! ```
//!
//! All fragments of one message share the frame `sequence` value — the
//! reassembly key and the retry correlation (a retry reuses the sequence,
//! replacing any partial reassembly). Capacity: 65535 fragments × ~1468 B
//! ≈ 96 MB, far above the 1 MB sanity ceiling enforced on
//! [`MAX_MESSAGE_LEN`].

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::checksum;
use crate::frame::{self, ETHTYPE_BOARD, HEADER_LEN};

/// Magic byte: protobuf frame (ASCII 'P'). Distinguishes from OEM magic
/// `0x11` (command) / `0x10` (response). Constant — the version slot.
pub const MAGIC_PROTOBUF: u8 = 0x50;

/// Fragmented-payload flag: bit 7 of the message_type byte (`0x0F`).
/// NOT part of the `MessageType` value (bits 0-6).
pub const FRAG_FLAG: u8 = 0x80;

/// Size of the fragment sub-header at the start of a fragmented payload.
pub const FRAG_SUBHEADER_LEN: usize = 8;

/// Default maximum frame payload: MTU 1500 − 14 B Ethernet header − 10 B
/// rbctl header (magic onward) = 1476 B. A fragmented frame carries
/// `FRAG_SUBHEADER_LEN` fewer bytes of message data.
pub const DEFAULT_MAX_PAYLOAD: usize = 1476;

/// Sanity ceiling for reassembled messages — a corrupt `message_length`
/// must not be able to make the peer allocate unbounded memory.
pub const MAX_MESSAGE_LEN: u32 = 1024 * 1024;

/// Reassembly timeout: partial fragments older than this are dropped and
/// the sender's sequence-based retry resends the whole message
/// (plans/protobuf.md → Fragmentation → reliability).
pub const DEFAULT_REASSEMBLY_TIMEOUT: Duration = Duration::from_millis(500);

// ─── message type ─────────────────────────────────────────────────────

/// Dispatch class carried in bits 0-6 of the byte at `0x0F`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MessageType {
    Request = 1,
    Response = 2,
    Event = 3,
}

impl MessageType {
    fn from_bits(v: u8) -> Option<Self> {
        Some(match v {
            1 => Self::Request,
            2 => Self::Response,
            3 => Self::Event,
            _ => return None,
        })
    }
}

// ─── errors ───────────────────────────────────────────────────────────

/// Frame parse/reassembly failures.
#[derive(Debug, PartialEq, Eq)]
pub enum FrameError {
    /// Fewer bytes than a bare header.
    TooShort,
    /// Magic is not [`MAGIC_PROTOBUF`] — not a protobuf frame at all.
    BadMagic(u8),
    /// EtherType is not `0x88B5`.
    BadEtherType(u16),
    /// Bits 0-6 of the message_type byte are not a known MessageType.
    BadMessageType(u8),
    /// `payload_len` does not match the bytes actually present.
    LengthMismatch { declared: u16, actual: usize },
    /// CRC-16/ARC mismatch — the frame is corrupt, drop it.
    CrcMismatch { expected: u16, got: u16 },
    /// Fragment flag set but the payload is shorter than the sub-header.
    FragmentHeaderTooShort,
    /// Fragment flag set with `fragment_count < 2` — the encoder never
    /// produces this (a fitting message is sent unfragmented); its
    /// presence indicates a broken sender.
    SingleFragmentMessage,
    /// `fragment_index >= fragment_count`.
    BadFragmentIndex { index: u16, count: u16 },
    /// `message_length` exceeds [`MAX_MESSAGE_LEN`].
    MessageTooLarge(u32),
    /// A fragment for an in-flight sequence disagrees with the partial
    /// message's count/length (corruption or seq reuse without expiry).
    FragmentMismatch,
    /// Reassembled size differs from the sub-header's `message_length`.
    AssembledLengthMismatch { declared: u32, actual: usize },
    /// Fragmentation needed but the per-frame payload budget cannot hold
    /// even the sub-header plus one byte.
    PayloadBudgetTooSmall,
}

// ─── encode ───────────────────────────────────────────────────────────

/// Encode a message into one or more frames.
///
/// If `payload` fits within `max_frame_payload` a single unfragmented
/// frame is produced; otherwise the payload is split into fragments of
/// `max_frame_payload − FRAG_SUBHEADER_LEN` message bytes, each prefixed
/// by the fragment sub-header, all sharing `seq`.
///
/// `max_frame_payload` exists so tests can force fragmentation with small
/// budgets; production callers pass [`DEFAULT_MAX_PAYLOAD`].
pub fn encode_message(
    dst_mac: &[u8; 6],
    src_mac: &[u8; 6],
    message_type: MessageType,
    seq: u32,
    payload: &[u8],
    max_frame_payload: usize,
) -> Result<Vec<Vec<u8>>, FrameError> {
    if payload.len() <= max_frame_payload {
        return Ok(vec![build_one(dst_mac, src_mac, message_type, seq, false, payload)]);
    }
    let chunk = max_frame_payload
        .checked_sub(FRAG_SUBHEADER_LEN)
        .filter(|c| *c >= 1)
        .ok_or(FrameError::PayloadBudgetTooSmall)?;
    let count = payload.len().div_ceil(chunk);
    let count_u16 = u16::try_from(count).map_err(|_| FrameError::MessageTooLarge(u32::MAX))?;
    let mut frames = Vec::with_capacity(count);
    for (index, part) in payload.chunks(chunk).enumerate() {
        let mut frag_payload = Vec::with_capacity(FRAG_SUBHEADER_LEN + part.len());
        frag_payload.extend_from_slice(&(index as u16).to_be_bytes());
        frag_payload.extend_from_slice(&count_u16.to_be_bytes());
        frag_payload.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        frag_payload.extend_from_slice(part);
        frames.push(build_one(dst_mac, src_mac, message_type, seq, true, &frag_payload));
    }
    Ok(frames)
}

/// Build a single frame (shared by the unfragmented and per-fragment
/// paths). `subtype_byte` = MessageType bits | FRAG_FLAG if fragmented.
fn build_one(
    dst_mac: &[u8; 6],
    src_mac: &[u8; 6],
    message_type: MessageType,
    seq: u32,
    fragmented: bool,
    payload: &[u8],
) -> Vec<u8> {
    let subtype = message_type as u8 | if fragmented { FRAG_FLAG } else { 0 };
    let mut buf = vec![0u8; HEADER_LEN + payload.len()];
    frame::build_frame(&mut buf, dst_mac, src_mac, MAGIC_PROTOBUF, subtype, seq, payload);
    buf
}

// ─── parse ────────────────────────────────────────────────────────────

/// A parsed fragment sub-header (borrowed view into the frame payload).
#[derive(Debug, PartialEq, Eq)]
pub struct Fragment<'a> {
    pub index: u16,
    pub count: u16,
    pub msg_len: u32,
    pub data: &'a [u8],
}

/// What follows the common header fields.
#[derive(Debug, PartialEq, Eq)]
pub enum PayloadKind<'a> {
    /// A whole protobuf message — decode directly.
    Whole(&'a [u8]),
    /// One fragment of a larger message — feed to [`Reassembler`].
    Fragment(Fragment<'a>),
}

/// A parsed frame: common header fields plus the payload kind.
#[derive(Debug, PartialEq, Eq)]
pub struct ParsedFrame<'a> {
    pub dst_mac: [u8; 6],
    pub src_mac: [u8; 6],
    pub message_type: MessageType,
    pub seq: u32,
    pub kind: PayloadKind<'a>,
}

/// Parse and validate one frame (magic, EtherType, lengths, CRC, message
/// type, fragment sub-header).
pub fn parse_frame(bytes: &[u8]) -> Result<ParsedFrame<'_>, FrameError> {
    if bytes.len() < HEADER_LEN {
        return Err(FrameError::TooShort);
    }
    if bytes[frame::OFF_MAGIC] != MAGIC_PROTOBUF {
        return Err(FrameError::BadMagic(bytes[frame::OFF_MAGIC]));
    }
    let etype = u16::from_be_bytes([bytes[0x0C], bytes[0x0D]]);
    if etype != ETHTYPE_BOARD {
        return Err(FrameError::BadEtherType(etype));
    }
    let mt_byte = bytes[frame::OFF_SUBTYPE];
    let fragmented = mt_byte & FRAG_FLAG != 0;
    let message_type =
        MessageType::from_bits(mt_byte & !FRAG_FLAG).ok_or(FrameError::BadMessageType(mt_byte))?;

    let declared = u16::from_be_bytes([bytes[frame::OFF_PAYLOAD_LEN], bytes[frame::OFF_PAYLOAD_LEN + 1]]);
    let actual = bytes.len() - HEADER_LEN;
    if declared as usize != actual {
        return Err(FrameError::LengthMismatch { declared, actual });
    }
    if !checksum::verify_checksum(bytes) {
        let got = checksum::crc_covered_region(bytes);
        let expected = u16::from_be_bytes([bytes[frame::OFF_CHECKSUM], bytes[frame::OFF_CHECKSUM + 1]]);
        return Err(FrameError::CrcMismatch { expected, got });
    }

    let seq = u32::from_be_bytes(bytes[frame::OFF_SEQ..frame::OFF_SEQ + 4].try_into().unwrap());
    let dst_mac: [u8; 6] = bytes[0x00..0x06].try_into().unwrap();
    let src_mac: [u8; 6] = bytes[0x06..0x0C].try_into().unwrap();
    let payload = &bytes[HEADER_LEN..];

    let kind = if fragmented {
        if payload.len() < FRAG_SUBHEADER_LEN {
            return Err(FrameError::FragmentHeaderTooShort);
        }
        let index = u16::from_be_bytes([payload[0], payload[1]]);
        let count = u16::from_be_bytes([payload[2], payload[3]]);
        let msg_len = u32::from_be_bytes(payload[4..8].try_into().unwrap());
        if count < 2 {
            return Err(FrameError::SingleFragmentMessage);
        }
        if index >= count {
            return Err(FrameError::BadFragmentIndex { index, count });
        }
        if msg_len > MAX_MESSAGE_LEN {
            return Err(FrameError::MessageTooLarge(msg_len));
        }
        PayloadKind::Fragment(Fragment { index, count, msg_len, data: &payload[FRAG_SUBHEADER_LEN..] })
    } else {
        PayloadKind::Whole(payload)
    };

    Ok(ParsedFrame { dst_mac, src_mac, message_type, seq, kind })
}

// ─── reassemble ───────────────────────────────────────────────────────

/// Partial message under reassembly.
struct Partial {
    fragments: Vec<Option<Vec<u8>>>,
    have: u16,
    msg_len: u32,
    first_seen: Instant,
}

/// Reassembles fragmented messages by `sequence`.
///
/// Reliability model (see plans/protobuf.md → Fragmentation): the
/// receiver reassembles by sequence; if not all fragments arrive within
/// `timeout`, the partial state is dropped and the sender's normal
/// sequence-based retry resends the whole message. Duplicates are
/// tolerated idempotently (retransmissions can produce them).
pub struct Reassembler {
    partials: HashMap<u32, Partial>,
    timeout: Duration,
}

impl Reassembler {
    pub fn new(timeout: Duration) -> Self {
        Self { partials: HashMap::new(), timeout }
    }

    /// Feed one fragment. Returns `Some(message)` when this fragment
    /// completed the message (the entry is then consumed).
    pub fn feed(&mut self, seq: u32, frag: &Fragment<'_>) -> Result<Option<Vec<u8>>, FrameError> {
        let now = Instant::now();
        let entry = self.partials.entry(seq).or_insert_with(|| Partial {
            fragments: vec![None; frag.count as usize],
            have: 0,
            msg_len: frag.msg_len,
            first_seen: now,
        });
        if entry.fragments.len() != frag.count as usize || entry.msg_len != frag.msg_len {
            return Err(FrameError::FragmentMismatch);
        }
        let slot = &mut entry.fragments[frag.index as usize];
        if slot.is_some() {
            return Ok(None); // duplicate retransmission — already held
        }
        *slot = Some(frag.data.to_vec());
        entry.have += 1;
        if entry.have != frag.count {
            return Ok(None);
        }
        // Complete: assemble in index order and validate the declared size.
        let parts = self.partials.remove(&seq).expect("entry just inserted");
        let mut msg = Vec::with_capacity(parts.msg_len as usize);
        for part in &parts.fragments {
            msg.extend_from_slice(part.as_ref().expect("have == count ⇒ no holes"));
        }
        if msg.len() as u32 != parts.msg_len {
            return Err(FrameError::AssembledLengthMismatch {
                declared: parts.msg_len,
                actual: msg.len(),
            });
        }
        Ok(Some(msg))
    }

    /// Drop partials older than the reassembly timeout; returns the
    /// sequences expired (for diagnostics/logging).
    pub fn expire(&mut self) -> Vec<u32> {
        let now = Instant::now();
        let expired: Vec<u32> = self
            .partials
            .iter()
            .filter(|(_, p)| now.duration_since(p.first_seen) > self.timeout)
            .map(|(&seq, _)| seq)
            .collect();
        for &seq in &expired {
            self.partials.remove(&seq);
        }
        expired
    }

    /// Number of partial messages currently held.
    pub fn in_flight(&self) -> usize {
        self.partials.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    const DST: [u8; 6] = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01];
    const SRC: [u8; 6] = [0x02, 0x00, 0x00, 0x00, 0x00, 0x02];

    /// Encode, then parse each frame (borrowing it) with common asserts.
    fn roundtrip(
        mt: MessageType,
        seq: u32,
        payload: &[u8],
        budget: usize,
    ) -> Vec<Vec<u8>> {
        let frames = encode_message(&DST, &SRC, mt, seq, payload, budget).unwrap();
        for f in &frames {
            let parsed = parse_frame(f).expect("parse");
            assert_eq!(parsed.message_type, mt);
            assert_eq!(parsed.seq, seq);
        }
        frames
    }

    /// Parse a roundtrip result into borrowed views.
    fn parse_all(frames: &[Vec<u8>]) -> Vec<ParsedFrame<'_>> {
        frames.iter().map(|f| parse_frame(f).expect("parse")).collect()
    }

    #[test]
    fn unfragmented_roundtrip() {
        let payload = b"hello protobuf";
        let frames = roundtrip(MessageType::Request, 42, payload, DEFAULT_MAX_PAYLOAD);
        let parsed = parse_all(&frames);
        let f = parsed.first().unwrap();
        assert_eq!(f.kind, PayloadKind::Whole(payload.as_slice()));
        assert_eq!(f.dst_mac, DST);
        assert_eq!(f.src_mac, SRC);
        // magic untouched by the frag flag; type byte carries it instead
        // (checked implicitly by BadMagic/CrcMismatch tests below).
    }

    #[test]
    fn fragmentation_splits_and_reassembles() {
        let payload: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
        let budget = 100; // 100-8 = 92 message bytes per fragment
        let frames = roundtrip(MessageType::Response, 7, &payload, budget);
        let parsed = parse_all(&frames);
        let expected_count = payload.len().div_ceil(100 - FRAG_SUBHEADER_LEN);
        assert_eq!(parsed.len(), expected_count);

        let mut re = Reassembler::new(Duration::from_secs(1));
        let mut assembled = None;
        for (i, f) in parsed.iter().enumerate() {
            let PayloadKind::Fragment(fr) = &f.kind else { panic!("must be fragmented") };
            assert_eq!(fr.count as usize, expected_count);
            assert_eq!(fr.msg_len as usize, payload.len());
            assert_eq!(fr.index as usize, i);
            // Magic stays exactly 0x50 even in fragmented frames.
            assert_eq!(fr.data.len(), (payload.len() - i * (100 - FRAG_SUBHEADER_LEN)).min(100 - FRAG_SUBHEADER_LEN));
            if let Some(msg) = re.feed(7, fr).unwrap() {
                assembled = Some(msg);
            }
        }
        assert_eq!(assembled.expect("completed"), payload);
        assert_eq!(re.in_flight(), 0, "completed entry consumed");
    }

    #[test]
    fn out_of_order_and_duplicate_fragments() {
        let payload: Vec<u8> = (0..500usize).map(|i| (i % 256) as u8).collect();
        let frames = roundtrip(MessageType::Event, 9, &payload, 60);
        let parsed = parse_all(&frames);
        let frags: Vec<&Fragment<'_>> = parsed
            .iter()
            .map(|f| match &f.kind {
                PayloadKind::Fragment(fr) => fr,
                PayloadKind::Whole(_) => panic!("fragmented expected"),
            })
            .collect();

        let mut re = Reassembler::new(Duration::from_secs(1));
        // reverse order (9..1), holding all but index 0 — completion must
        // come only when the missing fragment arrives
        for fr in frags[1..].iter().rev() {
            assert!(re.feed(9, fr).unwrap().is_none(), "not complete yet");
        }
        // duplicate of an already-held fragment: idempotent no-op
        assert!(re.feed(9, frags.last().unwrap()).unwrap().is_none());
        assert_eq!(re.in_flight(), 1);
        // the missing fragment completes the message
        let msg = re.feed(9, frags[0]).unwrap().expect("complete");
        assert_eq!(msg, payload);
    }

    #[test]
    fn expiry_drops_partial_state() {
        let payload = vec![7u8; 300];
        let frames = roundtrip(MessageType::Request, 5, &payload, 64);
        let parsed = parse_all(&frames);
        let mut re = Reassembler::new(Duration::from_millis(20));
        for f in &parsed[..parsed.len() - 1] {
            let PayloadKind::Fragment(fr) = &f.kind else { panic!() };
            re.feed(5, fr).unwrap();
        }
        assert_eq!(re.in_flight(), 1);
        sleep(Duration::from_millis(30));
        assert_eq!(re.expire(), vec![5]);
        assert_eq!(re.in_flight(), 0);
        // A late straggler for the expired sequence starts a fresh partial
        // instead of resurrecting the dropped one (ghost completion).
        let PayloadKind::Fragment(fr) = &parsed[parsed.len() - 1].kind else { panic!() };
        assert!(re.feed(5, fr).unwrap().is_none());
        assert_eq!(re.in_flight(), 1);
    }

    #[test]
    fn crc_corruption_detected() {
        let frames = encode_message(&DST, &SRC, MessageType::Response, 1, b"payload", DEFAULT_MAX_PAYLOAD).unwrap();
        let mut corrupt = frames.into_iter().next().unwrap();
        corrupt[HEADER_LEN] ^= 0xFF; // flip payload bits
        assert!(matches!(parse_frame(&corrupt), Err(FrameError::CrcMismatch { .. })));
    }

    #[test]
    fn bad_magic_and_ethertype_rejected() {
        let frames = encode_message(&DST, &SRC, MessageType::Request, 1, b"x", DEFAULT_MAX_PAYLOAD).unwrap();
        let mut oem = frames.into_iter().next().unwrap();
        let pristine = oem.clone();
        oem[frame::OFF_MAGIC] = 0x11; // OEM command magic
        assert_eq!(parse_frame(&oem), Err(FrameError::BadMagic(0x11)));

        let mut et = pristine;
        et[0x0C] = 0x88;
        et[0x0D] = 0xB6; // wrong EtherType
        // rebuild CRC so only the EtherType is wrong
        et[frame::OFF_CHECKSUM..frame::OFF_CHECKSUM + 2].copy_from_slice(&0u16.to_be_bytes());
        let crc = checksum::crc_covered_region(&et);
        et[frame::OFF_CHECKSUM..frame::OFF_CHECKSUM + 2].copy_from_slice(&crc.to_be_bytes());
        assert_eq!(parse_frame(&et), Err(FrameError::BadEtherType(0x88B6)));
    }

    #[test]
    fn truncated_and_bad_type_rejected() {
        assert_eq!(parse_frame(&[0u8; 10]), Err(FrameError::TooShort));

        let frames = encode_message(&DST, &SRC, MessageType::Event, 1, b"x", DEFAULT_MAX_PAYLOAD).unwrap();
        let mut bad = frames[0].clone();
        bad.truncate(bad.len() - 1); // payload_len now disagrees
        assert!(matches!(parse_frame(&bad), Err(FrameError::LengthMismatch { .. })));

        let mut mt = frames[0].clone();
        mt[frame::OFF_SUBTYPE] = 0x09; // no such type
        assert_eq!(parse_frame(&mt), Err(FrameError::BadMessageType(0x09)));
    }

    #[test]
    fn fragment_header_validation() {
        // Hand-build a frame with frag flag but count < 2.
        let mut sub = Vec::new();
        sub.extend_from_slice(&0u16.to_be_bytes()); // index
        sub.extend_from_slice(&1u16.to_be_bytes()); // count = 1 (illegal)
        sub.extend_from_slice(&4u32.to_be_bytes()); // msg_len
        sub.extend_from_slice(b"abcd");
        let f = build_one(&DST, &SRC, MessageType::Request, 3, true, &sub);
        assert_eq!(parse_frame(&f), Err(FrameError::SingleFragmentMessage));

        // index >= count
        let mut sub = Vec::new();
        sub.extend_from_slice(&3u16.to_be_bytes()); // index 3
        sub.extend_from_slice(&2u16.to_be_bytes()); // count 2
        sub.extend_from_slice(&4u32.to_be_bytes());
        sub.extend_from_slice(b"abcd");
        let f = build_one(&DST, &SRC, MessageType::Request, 3, true, &sub);
        assert_eq!(parse_frame(&f), Err(FrameError::BadFragmentIndex { index: 3, count: 2 }));
    }

    #[test]
    fn conflicting_fragment_rejected() {
        let payload = vec![1u8; 200];
        let frames = roundtrip(MessageType::Request, 12, &payload, 64);
        let parsed = parse_all(&frames);
        let mut re = Reassembler::new(Duration::from_secs(1));
        let PayloadKind::Fragment(fr) = &parsed[0].kind else { panic!() };
        re.feed(12, fr).unwrap();

        // A frame with the same seq but a different count/length must be
        // rejected, not merged (corruption or stale retry).
        let conflicting = Fragment { index: 0, count: fr.count + 1, msg_len: fr.msg_len, data: &[] };
        assert_eq!(re.feed(12, &conflicting), Err(FrameError::FragmentMismatch));
    }

    #[test]
    fn large_message_many_fragments() {
        // ~100 KB with a tiny budget → > 1000 fragments; exercises the
        // u16 count path well below its 65535 ceiling.
        let payload: Vec<u8> = (0..100_000usize).map(|i| (i % 256) as u8).collect();
        let frames = roundtrip(MessageType::Event, 77, &payload, 80);
        let parsed = parse_all(&frames);
        assert!(parsed.len() > 1000);
        let mut re = Reassembler::new(Duration::from_secs(5));
        let mut assembled = None;
        for f in &parsed {
            let PayloadKind::Fragment(fr) = &f.kind else { panic!() };
            if let Some(msg) = re.feed(77, fr).unwrap() {
                assembled = Some(msg);
            }
        }
        assert_eq!(assembled.expect("complete"), payload);
    }

    #[test]
    fn fitting_payload_stays_unfragmented() {
        let payload = vec![0u8; DEFAULT_MAX_PAYLOAD];
        let frames = encode_message(&DST, &SRC, MessageType::Request, 1, &payload, DEFAULT_MAX_PAYLOAD).unwrap();
        assert_eq!(frames.len(), 1);
        assert!(matches!(parse_frame(&frames[0]).unwrap().kind, PayloadKind::Whole(_)));

        // one byte over → fragments
        let payload = vec![0u8; DEFAULT_MAX_PAYLOAD + 1];
        let frames = encode_message(&DST, &SRC, MessageType::Request, 1, &payload, DEFAULT_MAX_PAYLOAD).unwrap();
        assert_eq!(frames.len(), 2);
    }

    #[test]
    fn message_type_byte_layout() {
        // The user-facing contract: one read of byte 0x0F yields dispatch
        // + fragmentation; magic stays a pure version/demux slot.
        let whole = encode_message(&DST, &SRC, MessageType::Event, 1, b"x", DEFAULT_MAX_PAYLOAD).unwrap().remove(0);
        assert_eq!(whole[frame::OFF_MAGIC], 0x50);
        assert_eq!(whole[frame::OFF_SUBTYPE], 0x03); // EVENT, no frag bit

        let payload = vec![0u8; 300];
        let frag = encode_message(&DST, &SRC, MessageType::Request, 1, &payload, 64).unwrap().remove(0);
        assert_eq!(frag[frame::OFF_MAGIC], 0x50); // unchanged by frag flag
        assert_eq!(frag[frame::OFF_SUBTYPE], 0x01 | FRAG_FLAG);
    }
}
