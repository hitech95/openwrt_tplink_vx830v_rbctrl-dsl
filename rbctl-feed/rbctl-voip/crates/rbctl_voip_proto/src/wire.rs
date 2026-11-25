// SPDX-License-Identifier: GPL-2.0-or-later
//! Voice envelope: Ethernet header + 10-byte voice header + body, the
//! CRC-16/ARC checksum, and the DD03 media prefix.
//!
//! Frame layout (untagged; the VLAN device adds/strips 802.1Q), offsets hex:
//!
//! | Offset | Size | Field |
//! |---|---|---|
//! | `00` | 6 | destination MAC |
//! | `06` | 6 | source MAC |
//! | `0c` | 2 | EtherType `DD01`/`DD02`/`DD03`, BE |
//! | `0e` | 1 | marker: master writes `0x11`, Econet writes `0x10` |
//! | `0f` | 1 | subtype (`16`, `18`, `19`, `1a`, `1b`) |
//! | `10` | 4 | sequence, BE32 (zero on master media frames) |
//! | `14` | 2 | declared body length, BE16 |
//! | `16` | 2 | CRC-16/ARC, BE16 |
//! | `18` | n | body, then zero padding up to 60 bytes for DD01/DD02 |
//!
//! The CRC covers the voice header `0e..16` with the CRC field taken as zero,
//! followed by exactly the declared body — never MACs, EtherType or padding.
//! See `docs/voip/board-link.md#envelope-checksum-and-transactions`.

use crate::{Error, Result};
use crc::{Crc, CRC_16_ARC};

/// EtherType of DD01: master requests and the board's status replies.
pub const ETH_CONTROL: u16 = 0xdd01;
/// EtherType of DD02: board events and the master's status replies.
pub const ETH_EVENT: u16 = 0xdd02;
/// EtherType of DD03: RTP/RTCP datagrams in either direction.
pub const ETH_MEDIA: u16 = 0xdd03;

/// Marker byte the master writes at offset `0e`.
pub const MARKER_HOST: u8 = 0x11;
/// Marker byte the Econet peer writes (verified for DD01 replies, DD02
/// events and DD03 media, `peer.md`).
pub const MARKER_PEER: u8 = 0x10;

/// DD01 subtype carrying one 132-byte DSP record.
pub const SUB_RECORD: u8 = 0x16;
/// DD02 subtype of a board event (and of the master's reply to it).
pub const SUB_EVENT: u8 = 0x18;
/// DD03 subtype of a media datagram.
pub const SUB_MEDIA: u8 = 0x19;
/// DD01 subtype: initialize the link (peer stores link state 2).
pub const SUB_INIT: u8 = 0x1a;
/// DD01 subtype: attach the local controller (peer connects its DSP server
/// and reports the cached DSP state as event 2).
pub const SUB_ATTACH: u8 = 0x1b;

/// Ethernet + voice header bytes before the body.
pub const HEADER_LEN: usize = 24;
/// Largest untagged Ethernet frame handled, excluding FCS (1500-byte MTU).
pub const FRAME_MAX: usize = 1514;
/// Largest body that fits in [`FRAME_MAX`].
pub const BODY_MAX: usize = FRAME_MAX - HEADER_LEN;
/// DD01/DD02 frames are zero-padded to the Ethernet minimum (60 bytes
/// before FCS), as the OEM proxy does. DD03 frames are never padded.
pub const MIN_CONTROL_FRAME: usize = 60;

/// A 6-byte Ethernet address in wire order.
pub type Mac = [u8; 6];
/// The broadcast address; also the "not yet learned" peer address.
pub const BROADCAST: Mac = [0xff; 6];

const ARC: Crc<u16> = Crc::<u16>::new(&CRC_16_ARC);

/// CRC-16/ARC of `data` (init 0, reflected poly `0xa001`, no final XOR).
///
/// Check value: `crc16(b"123456789") == 0xbb3d`.
pub fn crc16(data: &[u8]) -> u16 {
    ARC.checksum(data)
}

/// Decoded voice header (frame bytes `00..14`), all integers native order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// Destination MAC (`00`).
    pub dst: Mac,
    /// Source MAC (`06`).
    pub src: Mac,
    /// [`ETH_CONTROL`], [`ETH_EVENT`] or [`ETH_MEDIA`] (`0c`).
    pub ethertype: u16,
    /// [`MARKER_HOST`] or [`MARKER_PEER`] (`0e`).
    pub marker: u8,
    /// Transaction subtype (`0f`).
    pub subtype: u8,
    /// Sequence number (`10`, BE32 on the wire).
    pub seq: u32,
}

/// A successfully decoded frame. `body` borrows the input buffer and is
/// exactly the declared length (padding excluded).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame<'a> {
    /// Decoded header fields.
    pub header: Header,
    /// Declared body (offset `18`), borrowed from the input frame.
    pub body: &'a [u8],
}

fn known(ethertype: u16, marker: u8) -> bool {
    (ETH_CONTROL..=ETH_MEDIA).contains(&ethertype)
        && (marker == MARKER_HOST || marker == MARKER_PEER)
}

fn be16(p: &[u8], at: usize) -> Option<u16> {
    let b = p.get(at..at.checked_add(2)?)?;
    Some(u16::from_be_bytes([*b.first()?, *b.get(1)?]))
}

fn be32(p: &[u8], at: usize) -> Option<u32> {
    let b: [u8; 4] = p.get(at..at.checked_add(4)?)?.try_into().ok()?;
    Some(u32::from_be_bytes(b))
}

/// CRC over header bytes `0e..16` with a zero CRC field, then the body.
fn checksum(header: &[u8], body: &[u8]) -> Option<u16> {
    let mut d = ARC.digest();
    d.update(header.get(14..22)?);
    d.update(&[0, 0]);
    d.update(body);
    Some(d.finalize())
}

/// Encode `header` + `body` into `out` and return the frame length.
///
/// DD01/DD02 frames shorter than [`MIN_CONTROL_FRAME`] are zero-padded;
/// the padding is not part of the declared length or the CRC.
///
/// Errors: [`Error::InvalidArgument`] for an unknown EtherType or marker;
/// [`Error::MessageSize`] if `body` exceeds [`BODY_MAX`] or `out` is too
/// small. On error the contents of `out` are unspecified.
pub fn encode_frame(out: &mut [u8], header: &Header, body: &[u8]) -> Result<usize> {
    if !known(header.ethertype, header.marker) {
        return Err(Error::InvalidArgument);
    }
    if body.len() > BODY_MAX {
        return Err(Error::MessageSize);
    }
    let end = HEADER_LEN + body.len();
    let len = if header.ethertype != ETH_MEDIA {
        end.max(MIN_CONTROL_FRAME)
    } else {
        end
    };
    let frame = out.get_mut(..len).ok_or(Error::MessageSize)?;
    let declared = u16::try_from(body.len()).map_err(|_| Error::MessageSize)?;
    let (head, rest) = frame.split_at_mut(HEADER_LEN);
    let (payload, padding) = rest.split_at_mut(body.len());
    head.copy_from_slice(&{
        let mut h = [0u8; HEADER_LEN];
        h[..6].copy_from_slice(&header.dst);
        h[6..12].copy_from_slice(&header.src);
        h[12..14].copy_from_slice(&header.ethertype.to_be_bytes());
        h[14] = header.marker;
        h[15] = header.subtype;
        h[16..20].copy_from_slice(&header.seq.to_be_bytes());
        h[20..22].copy_from_slice(&declared.to_be_bytes());
        h
    });
    payload.copy_from_slice(body);
    padding.fill(0);
    let crc = checksum(head, body).ok_or(Error::MessageSize)?;
    head.get_mut(22..24)
        .ok_or(Error::MessageSize)?
        .copy_from_slice(&crc.to_be_bytes());
    Ok(len)
}

/// Decode and validate one frame.
///
/// Accepts either marker (structure only; [`crate::link::Link`] decides who
/// may send what). Trailing bytes after the declared body are padding.
///
/// Errors: [`Error::MessageSize`] if the frame is shorter than the header,
/// longer than [`FRAME_MAX`], or shorter than its declared body;
/// [`Error::Protocol`] for an unknown EtherType or marker;
/// [`Error::BadChecksum`] for a CRC mismatch.
pub fn decode_frame(frame: &[u8]) -> Result<Frame<'_>> {
    if frame.len() < HEADER_LEN || frame.len() > FRAME_MAX {
        return Err(Error::MessageSize);
    }
    let field = |o| be16(frame, o).ok_or(Error::MessageSize);
    let ethertype = field(12)?;
    let marker = *frame.get(14).ok_or(Error::MessageSize)?;
    if !known(ethertype, marker) {
        return Err(Error::Protocol);
    }
    let size = usize::from(field(20)?);
    let body = frame
        .get(HEADER_LEN..HEADER_LEN + size)
        .ok_or(Error::MessageSize)?;
    if checksum(frame, body) != Some(field(22)?) {
        return Err(Error::BadChecksum);
    }
    let mac = |o: usize| -> Result<Mac> {
        frame
            .get(o..o + 6)
            .and_then(|m| m.try_into().ok())
            .ok_or(Error::MessageSize)
    };
    Ok(Frame {
        header: Header {
            dst: mac(0)?,
            src: mac(6)?,
            ethertype,
            marker,
            subtype: *frame.get(15).ok_or(Error::MessageSize)?,
            seq: be32(frame, 16).ok_or(Error::MessageSize)?,
        },
        body,
    })
}

/// Which UDP socket a DD03 datagram belongs to (BE16 at body offset 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selector {
    /// Selector 3: RTP.
    Rtp = 3,
    /// Selector 4: RTCP.
    Rtcp = 4,
}

impl TryFrom<u16> for Selector {
    type Error = Error;
    fn try_from(v: u16) -> Result<Self> {
        match v {
            3 => Ok(Selector::Rtp),
            4 => Ok(Selector::Rtcp),
            _ => Err(Error::Protocol),
        }
    }
}

/// Number of media slots on the board (DSP channels 0..=3). Not the number
/// of phone lines (two).
pub const SLOTS: u16 = 4;

/// A decoded DD03 body: `be16 slot, be16 selector, datagram`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Media<'a> {
    /// Media slot `0..SLOTS`.
    pub slot: u16,
    /// RTP or RTCP.
    pub selector: Selector,
    /// The complete RTP/RTCP datagram (header included), borrowed.
    pub datagram: &'a [u8],
}

/// Write the DD03 body prefix and `datagram` into `out`; returns body length.
///
/// Errors: [`Error::InvalidArgument`] for slot ≥ 4 or an empty datagram;
/// [`Error::MessageSize`] if it does not fit in a frame or in `out`.
pub fn encode_media(
    out: &mut [u8],
    slot: u16,
    selector: Selector,
    datagram: &[u8],
) -> Result<usize> {
    if slot >= SLOTS || datagram.is_empty() {
        return Err(Error::InvalidArgument);
    }
    if datagram.len() > BODY_MAX - 4 {
        return Err(Error::MessageSize);
    }
    let len = datagram.len() + 4;
    let body = out.get_mut(..len).ok_or(Error::MessageSize)?;
    let (prefix, data) = body.split_at_mut(4);
    prefix.copy_from_slice(&{
        let mut p = [0u8; 4];
        p[..2].copy_from_slice(&slot.to_be_bytes());
        p[2..].copy_from_slice(&(selector as u16).to_be_bytes());
        p
    });
    data.copy_from_slice(datagram);
    Ok(len)
}

/// Parse a DD03 body.
///
/// Errors: [`Error::MessageSize`] for a body of ≤ 4 or > [`BODY_MAX`] bytes;
/// [`Error::Protocol`] for slot ≥ 4 or a selector other than 3/4.
pub fn decode_media(body: &[u8]) -> Result<Media<'_>> {
    if body.len() <= 4 || body.len() > BODY_MAX {
        return Err(Error::MessageSize);
    }
    let slot = be16(body, 0).ok_or(Error::MessageSize)?;
    let selector = Selector::try_from(be16(body, 2).ok_or(Error::MessageSize)?)?;
    if slot >= SLOTS {
        return Err(Error::Protocol);
    }
    Ok(Media {
        slot,
        selector,
        datagram: body.get(4..).ok_or(Error::MessageSize)?,
    })
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, clippy::unwrap_used)]
mod tests {
    use super::*;

    const HOST: Mac = [2, 0, 0, 0, 0, 1];

    #[test]
    fn crc_vectors() {
        // board-link.md offline vectors, starting at the marker.
        let init = [17, 26, 0, 0, 0, 1, 0, 4, 0, 0, 1, 0, 0, 0];
        let ack = [17, 24, 0, 0, 0, 9, 0, 4, 0, 0, 0, 0, 0, 0];
        assert_eq!(crc16(&init), 0x277e);
        assert_eq!(crc16(&ack), 0x591f);
        assert_eq!(crc16(b"123456789"), 0xbb3d);
    }

    #[test]
    fn frame_round_trip_and_rejections() {
        let h = Header {
            dst: [2, 0, 0, 0, 0, 2],
            src: HOST,
            ethertype: ETH_CONTROL,
            marker: MARKER_HOST,
            subtype: SUB_INIT,
            seq: 1,
        };
        let body = [1, 0, 0, 0];
        let mut f = [0u8; 60];
        assert_eq!(
            encode_frame(&mut f[..59], &h, &body),
            Err(Error::MessageSize)
        );
        assert_eq!(encode_frame(&mut f, &h, &body), Ok(60));
        assert_eq!(&f[22..24], &[0x27, 0x7e]);
        f[59] = 1; // padding is outside the CRC
        let d = decode_frame(&f).unwrap();
        assert_eq!(d.header, h);
        assert_eq!(d.body, &body);
        assert_eq!(decode_frame(&f[..27]), Err(Error::MessageSize));
        f[24] ^= 1;
        assert_eq!(decode_frame(&f), Err(Error::BadChecksum));
    }

    #[test]
    fn media_prefix() {
        let mut b = [0u8; 8];
        assert_eq!(
            encode_media(&mut b, 4, Selector::Rtp, &[1]),
            Err(Error::InvalidArgument)
        );
        assert_eq!(encode_media(&mut b, 1, Selector::Rtcp, &[9, 8]), Ok(6));
        let m = decode_media(&b[..6]).unwrap();
        assert_eq!(
            (m.slot, m.selector, m.datagram),
            (1, Selector::Rtcp, &[9u8, 8][..])
        );
        assert_eq!(decode_media(&[0, 2, 0, 5, 1]), Err(Error::Protocol));
        assert_eq!(decode_media(&[0, 2, 0, 4]), Err(Error::MessageSize));
    }
}
