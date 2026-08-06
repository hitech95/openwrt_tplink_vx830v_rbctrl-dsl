//! `0x88B5` frame layout — 24-byte header + payload.
//!
//! ```text
//! Offset  Size  Field
//! ------  ----  -------------------------------------------
//! 0x00    6     dst MAC (broadcast FF:FF:FF:FF:FF:FF on init)
//! 0x06    6     src MAC (local interface MAC)
//! 0x0C    2     EtherType 0x88B5 (big-endian)
//! 0x0E    1     magic: 0x11 command / 0x10 response
//! 0x0F    1     subtype (= opcode / cmm key)
//! 0x10    4     sequence (big-endian u32)
//! 0x14    2     payload_len (big-endian u16, bytes from 0x18 onward)
//! 0x16    2     checksum (CRC-16/ARC, big-endian, zeroed during compute)
//! 0x18    ..    payload (TX: byte 0 = payload_type; RX: raw data, no echo)
//! ```
//!
//! Multi-byte fields are big-endian on the wire. See `docs/protocol.md`.

use crate::{set_checksum, MAGIC_COMMAND};

/// Board management EtherType.
pub const ETHTYPE_BOARD: u16 = 0x88B5;

/// Total header size (bytes `0x00`–`0x17`). Payload starts at `0x18`.
pub const HEADER_LEN: usize = 0x18;

/// Minimum Ethernet frame length (padded by the sender).
pub const MIN_FRAME: usize = 60;

// Field offsets within a frame buffer.
pub const OFF_MAGIC: usize = 0x0E;
pub const OFF_SUBTYPE: usize = 0x0F;
pub const OFF_SEQ: usize = 0x10;
pub const OFF_PAYLOAD_LEN: usize = 0x14;
pub const OFF_CHECKSUM: usize = 0x16;
pub const OFF_PAYLOAD: usize = 0x18;

const BROADCAST_MAC: [u8; 6] = [0xff; 6];

// ─── TX: build a command frame ─────────────────────────────────────────

/// Fill `buf` with a command frame (magic `0x11`) and compute the checksum.
///
/// `payload` is the full bytes from offset `0x18` onward (byte 0 = payload_type).
/// The caller is responsible for encoding the payload via [`crate::pack`].
///
/// Returns the slice of `buf` that holds the complete frame (header + payload,
/// **not** padded to `MIN_FRAME` — padding is the socket layer's job).
///
/// # Panics
/// Panics if `buf` is too small for `HEADER_LEN + payload.len()`.
pub fn build_command_frame<'a>(
    buf: &'a mut [u8],
    subtype: u8,
    seq: u32,
    src_mac: &[u8; 6],
    payload: &[u8],
) -> &'a mut [u8] {
    let total = HEADER_LEN + payload.len();
    assert!(buf.len() >= total, "buf too small: need {total}, have {}", buf.len());
    let f = &mut buf[..total];

    // Ethernet header
    f[0x00..0x06].copy_from_slice(&BROADCAST_MAC);
    f[0x06..0x0c].copy_from_slice(src_mac);
    f[0x0c..0x0e].copy_from_slice(&ETHTYPE_BOARD.to_be_bytes());

    // Protocol header
    f[OFF_MAGIC] = MAGIC_COMMAND;
    f[OFF_SUBTYPE] = subtype;
    f[OFF_SEQ..OFF_SEQ + 4].copy_from_slice(&seq.to_be_bytes());
    f[OFF_PAYLOAD_LEN..OFF_PAYLOAD_LEN + 2].copy_from_slice(&(payload.len() as u16).to_be_bytes());
    f[OFF_CHECKSUM..OFF_CHECKSUM + 2].copy_from_slice(&0u16.to_be_bytes());

    // Payload
    f[OFF_PAYLOAD..].copy_from_slice(payload);

    // Checksum (covers the region with the checksum field treated as zero)
    set_checksum(f);
    f
}

// ─── RX: parse a received frame ────────────────────────────────────────

/// Immutable view over a received `0x88B5` frame.
///
/// Created via [`Frame::parse`]. Does not copy — borrows the underlying buffer.
/// All multi-byte accessors return host-order values.
#[derive(Clone, Copy)]
pub struct Frame<'a> {
    buf: &'a [u8],
}

impl<'a> Frame<'a> {
    /// Parse a frame from a byte slice. Returns `None` if shorter than
    /// [`HEADER_LEN`] or the payload extends past the buffer end.
    pub fn parse(buf: &'a [u8]) -> Option<Self> {
        if buf.len() < HEADER_LEN {
            return None;
        }
        let plen = u16::from_be_bytes([buf[OFF_PAYLOAD_LEN], buf[OFF_PAYLOAD_LEN + 1]]) as usize;
        if buf.len() < OFF_PAYLOAD + plen {
            return None;
        }
        Some(Self { buf })
    }

    pub fn magic(&self) -> u8 { self.buf[OFF_MAGIC] }
    pub fn subtype(&self) -> u8 { self.buf[OFF_SUBTYPE] }
    pub fn seq(&self) -> u32 { u32::from_be_bytes(self.buf[OFF_SEQ..OFF_SEQ + 4].try_into().unwrap()) }
    pub fn payload_len(&self) -> u16 { u16::from_be_bytes([self.buf[OFF_PAYLOAD_LEN], self.buf[OFF_PAYLOAD_LEN + 1]]) }
    pub fn is_command(&self) -> bool { self.buf[OFF_MAGIC] == crate::MAGIC_COMMAND }
    pub fn is_response(&self) -> bool { self.buf[OFF_MAGIC] == crate::MAGIC_RESPONSE }
    pub fn dst_mac(&self) -> &[u8] { &self.buf[0x00..0x06] }
    pub fn src_mac(&self) -> &[u8] { &self.buf[0x06..0x0c] }

    /// Raw frame bytes (the entire Ethernet + protocol frame).
    pub fn buf(&self) -> &'a [u8] { self.buf }

    /// The payload slice (offset `0x18`, length = `payload_len`).
    /// TX payloads start with the opcode byte; RX responses contain raw
    /// data directly (no opcode echo, confirmed via hardware capture).
    pub fn payload(&self) -> &'a [u8] {
        let len = self.payload_len() as usize;
        &self.buf[OFF_PAYLOAD..OFF_PAYLOAD + len]
    }
}

// ─── Sequence counter ──────────────────────────────────────────────────

/// Monotonic sequence counter for TX frames (`g_dwProtoSeq` in the C daemon).
/// Wraps on overflow. Not `Sync` — the socket layer owns it single-threaded.
#[derive(Debug, Default)]
pub struct SeqCounter(u32);

impl SeqCounter {
    pub const fn new() -> Self { Self(0) }
    pub fn next(&mut self) -> u32 { self.0 = self.0.wrapping_add(1); self.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verify_checksum;

    #[test]
    fn build_and_parse_round_trip() {
        let payload = [0x06u8, 0x01, 0x00, 0x00, 0x00, 0x00, 0xc0, 0x00, 0x00, 0x00, 0x00, 0x00];
        let src = [0x00u8, 0x11, 0x22, 0x33, 0x44, 0x55];

        let mut buf = [0u8; 128];
        let frame_bytes = build_command_frame(&mut buf, 0x01, 42, &src, &payload);

        // Verify checksum is valid
        assert!(verify_checksum(frame_bytes), "checksum must verify");

        // Parse and check fields
        let frame = Frame::parse(frame_bytes).unwrap();
        assert_eq!(frame.subtype(), 0x01);
        assert_eq!(frame.seq(), 42);
        assert_eq!(frame.payload_len(), payload.len() as u16);
        assert_eq!(frame.payload(), &payload);
        assert!(frame.is_command());
        assert_eq!(frame.dst_mac(), &[0xff; 6]);
    }

    #[test]
    fn seq_counter_increments() {
        let mut c = SeqCounter::new();
        assert_eq!(c.next(), 1);
        assert_eq!(c.next(), 2);
        assert_eq!(c.next(), 3);
    }

    #[test]
    fn parse_rejects_short() {
        assert!(Frame::parse(&[0; 10]).is_none());
    }
}
