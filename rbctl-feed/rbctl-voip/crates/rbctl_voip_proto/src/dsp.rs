// SPDX-License-Identifier: GPL-2.0-or-later
//! The 132-byte DSP record carried in DD01/`16` and DD02/`18`.
//!
//! Layout: BE32 command at record offset 0, then a 128-byte body. Only the
//! command's *meaningful* bytes ([`Command::body_len`]) are written; the
//! rest of the record is always zero so nothing leaks onto the wire.
//!
//! Command numbers and names come from the `(integer, name)` tables in the
//! OEM proxy (`406e90`) and client (`443230`); see
//! `docs/voip/board-link.md#inner-dsp-inventory`. Body offsets in this module
//! are **relative to the body** (record offset minus 4).
//!
//! Four body layouts exist:
//! * [`Body::Words`] — a run of BE32 words (most commands);
//! * [`Body::Cid`] — `CID_PLAY` (command 4);
//! * [`Body::Tone`] — `TONE_PLAY` (command 13);
//! * [`Body::Media`] — `RTP_ACTIVE`/`RTP_UPDATE` (commands 10/11);
//! * [`Body::Opaque`] — commands whose layout is unknown; all 128 bytes are
//!   preserved verbatim.

use crate::{Error, Result};

/// Bytes in a complete DSP record.
pub const RECORD_LEN: usize = 132;
/// Bytes of body storage after the command word.
pub const BODY_LEN: usize = 128;
/// Maximum number of BE32 words a [`Body::Words`] body can hold.
pub const MAX_WORDS: usize = BODY_LEN / 4;

/// Direction of a record. Two commands have different bodies per direction:
/// `DTMF_END` (12-byte host helper vs 8-byte peer event) and `FXS_STAT_GET`
/// (4-byte slot request vs opaque 128-byte statistics reply).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Master → board (DD01/`16`).
    ToPeer,
    /// Board → master (DD02/`18`).
    FromPeer,
}

/// DSP command identifiers (`VOIP_` names from the firmware tables).
/// Gaps (25–30, 32–69, 75–90) are reserved; 94 is `DSPMSG_CMDMAX`, a
/// sentinel, not a command.
#[allow(missing_docs)] // names are the firmware names; meaning in board-link.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum Command {
    /// Host: DSP defaults / hardware-profile selector (locale index).
    DspDefault = 0,
    /// Host: SLIC country 0..=20. Peer: SLIC init result (−1, −2 errors).
    SlicLocale = 1,
    /// Host: start DSP (word 1). Peer: DSP state (0 running, 1 stopped).
    DspStart = 2,
    DspStop = 3,
    /// Host: caller-ID data for the next ring.
    CidPlay = 4,
    RingStart = 5,
    RingStop = 6,
    ChannelDefault = 7,
    /// Host: 11-word channel settings, word 0 = slot.
    ChannelOpen = 8,
    ChannelUpdate = 9,
    RtpActive = 10,
    RtpUpdate = 11,
    /// Host: deactivate media on slot (word 0).
    RtpDeactive = 12,
    /// Host: play tone; selector 0 is the stop path.
    TonePlay = 13,
    ToneStop = 14,
    ThreeWayStart = 15,
    ThreeWayStop = 16,
    /// Peer: handset on-hook, word 0 = line.
    HookOn = 17,
    /// Peer: handset off-hook, word 0 = line.
    HookOff = 18,
    /// Peer: hook flash, word 0 = line.
    HookFlash = 19,
    /// Peer: completed digit (line, ASCII). Host: 12-byte helper.
    DtmfEnd = 20,
    FaxStart = 21,
    FaxmodemEnd = 22,
    SlicControl = 23,
    /// Peer: digit leading edge (line, ASCII).
    DtmfStart = 24,
    ChannelClose = 31,
    Mgmt70 = 70,
    Dtmf2833 = 71,
    Call72 = 72,
    Call73 = 73,
    CallInternalTransfer = 74,
    FxsStatGet = 91,
    FxsStatReset = 92,
    VmsStatus = 93,
}

impl TryFrom<u32> for Command {
    type Error = Error;
    /// Unknown, reserved and sentinel IDs map to [`Error::Unsupported`].
    fn try_from(id: u32) -> Result<Self> {
        use Command::*;
        Ok(match id {
            0 => DspDefault,
            1 => SlicLocale,
            2 => DspStart,
            3 => DspStop,
            4 => CidPlay,
            5 => RingStart,
            6 => RingStop,
            7 => ChannelDefault,
            8 => ChannelOpen,
            9 => ChannelUpdate,
            10 => RtpActive,
            11 => RtpUpdate,
            12 => RtpDeactive,
            13 => TonePlay,
            14 => ToneStop,
            15 => ThreeWayStart,
            16 => ThreeWayStop,
            17 => HookOn,
            18 => HookOff,
            19 => HookFlash,
            20 => DtmfEnd,
            21 => FaxStart,
            22 => FaxmodemEnd,
            23 => SlicControl,
            24 => DtmfStart,
            31 => ChannelClose,
            70 => Mgmt70,
            71 => Dtmf2833,
            72 => Call72,
            73 => Call73,
            74 => CallInternalTransfer,
            91 => FxsStatGet,
            92 => FxsStatReset,
            93 => VmsStatus,
            _ => return Err(Error::Unsupported),
        })
    }
}

/// Which [`Body`] variant a command uses in a given direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// `n` BE32 words.
    Words(usize),
    /// [`Cid`] (116 bytes).
    Cid,
    /// [`Tone`] (100 bytes).
    Tone,
    /// [`MediaConfig`] (120 bytes).
    Media,
    /// All 128 bytes preserved without interpretation.
    Opaque,
}

impl Command {
    /// Every implemented command, in ID order (useful for exhaustive tests).
    pub const ALL: [Command; 34] = {
        use Command::*;
        [
            DspDefault,
            SlicLocale,
            DspStart,
            DspStop,
            CidPlay,
            RingStart,
            RingStop,
            ChannelDefault,
            ChannelOpen,
            ChannelUpdate,
            RtpActive,
            RtpUpdate,
            RtpDeactive,
            TonePlay,
            ToneStop,
            ThreeWayStart,
            ThreeWayStop,
            HookOn,
            HookOff,
            HookFlash,
            DtmfEnd,
            FaxStart,
            FaxmodemEnd,
            SlicControl,
            DtmfStart,
            ChannelClose,
            Mgmt70,
            Dtmf2833,
            Call72,
            Call73,
            CallInternalTransfer,
            FxsStatGet,
            FxsStatReset,
            VmsStatus,
        ]
    };

    /// Numeric command ID (BE32 at record offset 0).
    pub fn id(self) -> u32 {
        self as u32
    }

    /// Body layout for this command and direction.
    pub fn layout(self, dir: Direction) -> Layout {
        use Command::*;
        match self {
            DspDefault | SlicLocale | DspStart | DspStop | RingStop | ChannelDefault
            | RtpDeactive | HookOn | HookOff | FaxmodemEnd | ChannelClose | Call72 | Call73
            | FxsStatReset | VmsStatus => Layout::Words(1),
            ThreeWayStart | ThreeWayStop | HookFlash | DtmfStart | Dtmf2833 | FaxStart
            | SlicControl => Layout::Words(2),
            DtmfEnd if dir == Direction::FromPeer => Layout::Words(2),
            DtmfEnd => Layout::Words(3),
            Mgmt70 => Layout::Words(4),
            RingStart => Layout::Words(6),
            ChannelOpen => Layout::Words(11),
            FxsStatGet if dir == Direction::FromPeer => Layout::Opaque,
            FxsStatGet => Layout::Words(1),
            CidPlay => Layout::Cid,
            TonePlay => Layout::Tone,
            RtpActive | RtpUpdate => Layout::Media,
            ChannelUpdate | ToneStop | CallInternalTransfer => Layout::Opaque,
        }
    }

    /// Meaningful body bytes for this command and direction (the rest of the
    /// 128-byte body is zero on encode and ignored on decode).
    pub fn body_len(self, dir: Direction) -> usize {
        match self.layout(dir) {
            Layout::Words(n) => n * 4,
            Layout::Cid => 116,
            Layout::Tone => 100,
            Layout::Media => 120,
            Layout::Opaque => BODY_LEN,
        }
    }
}

/// `CID_PLAY` body. Offsets are **decimal**, body-relative.
///
/// The Econet peer copies at most 15 bytes of `name` and `number`
/// (`peer.md`, caller ID); longer values are truncated on the board.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cid {
    /// 0: analog line 0/1, BE32.
    pub line: u32,
    /// 4..19: four opaque BE32 words (OEM writes zero).
    pub attr: [u32; 4],
    /// 20: date text, 9 bytes (OEM: `MMDDhhmm` + NUL).
    pub date: [u8; 9],
    /// 29: caller name bytes, NUL-padded.
    pub name: [u8; 40],
    /// 69: caller number bytes, NUL-padded.
    pub number: [u8; 40],
    /// 112: line-selection bit mask (`1 << line`), BE32. Not read by the peer.
    pub select: u32,
}

impl Default for Cid {
    fn default() -> Self {
        Cid {
            line: 0,
            attr: [0; 4],
            date: [0; 9],
            name: [0; 40],
            number: [0; 40],
            select: 0,
        }
    }
}

/// `TONE_PLAY` body (100 bytes). Offsets decimal, body-relative.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tone {
    /// 0..15: target line/slot, tone selector, two tone arguments (BE32).
    /// Selector 4 = dial tone, 0 = stop (`peer.md` tone table).
    pub args: [u32; 4],
    /// 16 and 56: two 40-byte text fields (used by the CID tone variant).
    pub text: [[u8; 40]; 2],
    /// 96: final tone argument, BE32.
    pub last: u32,
}

impl Default for Tone {
    fn default() -> Self {
        Tone {
            args: [0; 4],
            text: [[0; 40]; 2],
            last: 0,
        }
    }
}

/// Address family word at body `0x74`. Always written as BE32.
///
/// Note: the stock OEM proxy reads this word without byte swapping, so a
/// BE `10` is seen as `0x0a000000` there; the Econet peer does not rewrite
/// it either. Only IPv4 (2) is validated on hardware.
pub const AF_INET: u32 = 2;
/// IPv6 address family value (see [`AF_INET`]).
pub const AF_INET6: u32 = 10;

/// `RTP_ACTIVE`/`RTP_UPDATE` body (120 bytes). Offsets **hex**,
/// body-relative; see `board-link.md#media-configuration-vs-media-packets`.
///
/// Integers are native here and BE on the wire; address arrays are network
/// octets. For IPv4 the peer replaces both addresses with `127.0.0.1` and
/// all ports with its own per-slot ports (`peer.md`, RTP addressing), so
/// these values never reach the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MediaConfig {
    /// 00 (BE16): activation argument; the OEM controller passes `0xfffe`.
    pub activation: u16,
    /// 08, 0c: controller configuration words (meaning unconfirmed).
    pub config: [u32; 2],
    /// 10: media slot 0..=3.
    pub slot: u32,
    /// 14: DSP direction: 0 none (no RTP!), 1 sendonly, 2 recvonly,
    /// 3 sendrecv (`peer.md` T2.6 follow-up; hardware-verified 0 vs 3).
    pub direction: u32,
    /// 18: DSP codec ID: PCMU 0, G726-32 1, PCMA 2, G722 3, G729 4, T.38 5.
    /// Not an RTP payload type.
    pub codec: u32,
    /// 1c: transmit voice RTP payload type.
    pub tx_pt: u32,
    /// 20: receive voice RTP payload type.
    pub rx_pt: u32,
    /// 24: call-state word (meaning unconfirmed).
    pub state: u32,
    /// 28: transmit packetization, ms (10..60 step 10).
    pub tx_ms: u32,
    /// 2c: receive packetization, ms.
    pub rx_ms: u32,
    /// 30: DTMF relay mode; 1 = RFC 4733 telephone-event.
    pub dtmf: u32,
    /// 34: transmit telephone-event payload type (peer uses 96 if zero).
    pub tx_event_pt: u32,
    /// 38: receive telephone-event payload type.
    pub rx_event_pt: u32,
    /// 3c: local IPv4 address.
    pub local4: [u8; 4],
    /// 40 (BE16): local RTP port.
    pub local_port: u16,
    /// 44: remote IPv4 address.
    pub remote4: [u8; 4],
    /// 48 (BE16): remote RTP port.
    pub remote_port: u16,
    /// 4a (BE16): remote RTCP port.
    pub control_port: u16,
    /// 4c: IP TOS for the RTP socket.
    pub tos: u8,
    /// 4d: IP TOS for the RTCP socket.
    pub control_tos: u8,
    /// 50: additional controller state word.
    pub media_state: u32,
    /// 54: local IPv6 address.
    pub local6: [u8; 16],
    /// 64: remote IPv6 address.
    pub remote6: [u8; 16],
    /// 74: [`AF_INET`] or [`AF_INET6`].
    pub family: u32,
}

/// A record body, by layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Body {
    /// BE32 words; only the first `Command::body_len / 4` are meaningful.
    Words([u32; MAX_WORDS]),
    /// `CID_PLAY`.
    Cid(Cid),
    /// `TONE_PLAY`.
    Tone(Tone),
    /// `RTP_ACTIVE` / `RTP_UPDATE`.
    Media(MediaConfig),
    /// Unknown layout: all 128 bytes.
    Opaque([u8; BODY_LEN]),
}

/// One DSP record: command + body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record {
    /// Command identifier.
    pub command: Command,
    /// Body; must match `command.layout(dir)` when encoding.
    pub body: Body,
}

impl Record {
    /// Convenience constructor for word-layout commands. Missing words are
    /// zero. Errors with [`Error::InvalidArgument`] if more than
    /// [`MAX_WORDS`] words are given.
    pub fn words(command: Command, words: &[u32]) -> Result<Self> {
        let mut w = [0u32; MAX_WORDS];
        w.get_mut(..words.len())
            .ok_or(Error::InvalidArgument)?
            .copy_from_slice(words);
        Ok(Record {
            command,
            body: Body::Words(w),
        })
    }

    /// Word `i` of a word-layout body (0 if out of range or another layout).
    pub fn word(&self, i: usize) -> u32 {
        match &self.body {
            Body::Words(w) => w.get(i).copied().unwrap_or(0),
            _ => 0,
        }
    }

    /// Encode into a full 132-byte record.
    ///
    /// Errors: [`Error::InvalidArgument`] if the body variant does not match
    /// the command's layout in `dir`, or a media body has slot ≥ 4 or a
    /// family other than 2/10.
    pub fn encode(&self, dir: Direction) -> Result<[u8; RECORD_LEN]> {
        let mut out = [0u8; RECORD_LEN];
        let (head, body) = out.split_at_mut(4);
        head.copy_from_slice(&self.command.id().to_be_bytes());
        let mut w = Cursor { buf: body };
        match (self.command.layout(dir), &self.body) {
            (Layout::Words(n), Body::Words(words)) => {
                for (i, v) in words.iter().take(n).enumerate() {
                    w.u32(i * 4, *v)?;
                }
            }
            (Layout::Opaque, Body::Opaque(b)) => w.bytes(0, b)?,
            (Layout::Cid, Body::Cid(c)) => {
                w.u32(0, c.line)?;
                for (i, v) in c.attr.iter().enumerate() {
                    w.u32(4 + i * 4, *v)?;
                }
                w.bytes(20, &c.date)?;
                w.bytes(29, &c.name)?;
                w.bytes(69, &c.number)?;
                w.u32(112, c.select)?;
            }
            (Layout::Tone, Body::Tone(t)) => {
                for (i, v) in t.args.iter().enumerate() {
                    w.u32(i * 4, *v)?;
                }
                w.bytes(16, &t.text[0])?;
                w.bytes(56, &t.text[1])?;
                w.u32(96, t.last)?;
            }
            (Layout::Media, Body::Media(m)) => {
                if m.slot >= 4 || (m.family != AF_INET && m.family != AF_INET6) {
                    return Err(Error::InvalidArgument);
                }
                w.u16(0x00, m.activation)?;
                w.u32(0x08, m.config[0])?;
                w.u32(0x0c, m.config[1])?;
                w.u32(0x10, m.slot)?;
                w.u32(0x14, m.direction)?;
                w.u32(0x18, m.codec)?;
                w.u32(0x1c, m.tx_pt)?;
                w.u32(0x20, m.rx_pt)?;
                w.u32(0x24, m.state)?;
                w.u32(0x28, m.tx_ms)?;
                w.u32(0x2c, m.rx_ms)?;
                w.u32(0x30, m.dtmf)?;
                w.u32(0x34, m.tx_event_pt)?;
                w.u32(0x38, m.rx_event_pt)?;
                w.bytes(0x3c, &m.local4)?;
                w.u16(0x40, m.local_port)?;
                w.bytes(0x44, &m.remote4)?;
                w.u16(0x48, m.remote_port)?;
                w.u16(0x4a, m.control_port)?;
                w.bytes(0x4c, &[m.tos, m.control_tos])?;
                w.u32(0x50, m.media_state)?;
                w.bytes(0x54, &m.local6)?;
                w.bytes(0x64, &m.remote6)?;
                w.u32(0x74, m.family)?;
            }
            _ => return Err(Error::InvalidArgument),
        }
        Ok(out)
    }

    /// Decode a 132-byte record.
    ///
    /// Bytes beyond the command's meaningful length are ignored (keep the
    /// raw record if you need to relay exact bytes). Errors:
    /// [`Error::MessageSize`] unless `record` is exactly 132 bytes;
    /// [`Error::Unsupported`] for an unknown command ID.
    pub fn decode(record: &[u8], dir: Direction) -> Result<Self> {
        if record.len() != RECORD_LEN {
            return Err(Error::MessageSize);
        }
        let (head, body) = record.split_at(4);
        let id = u32::from_be_bytes(head.try_into().map_err(|_| Error::MessageSize)?);
        let command = Command::try_from(id)?;
        let r = Reader { buf: body };
        let body = match command.layout(dir) {
            Layout::Words(n) => {
                let mut w = [0u32; MAX_WORDS];
                for (i, v) in w.iter_mut().take(n).enumerate() {
                    *v = r.u32(i * 4)?;
                }
                Body::Words(w)
            }
            Layout::Opaque => Body::Opaque(r.array(0)?),
            Layout::Cid => Body::Cid(Cid {
                line: r.u32(0)?,
                attr: [r.u32(4)?, r.u32(8)?, r.u32(12)?, r.u32(16)?],
                date: r.array(20)?,
                name: r.array(29)?,
                number: r.array(69)?,
                select: r.u32(112)?,
            }),
            Layout::Tone => Body::Tone(Tone {
                args: [r.u32(0)?, r.u32(4)?, r.u32(8)?, r.u32(12)?],
                text: [r.array(16)?, r.array(56)?],
                last: r.u32(96)?,
            }),
            Layout::Media => {
                let [tos, control_tos]: [u8; 2] = r.array(0x4c)?;
                Body::Media(MediaConfig {
                    activation: r.u16(0x00)?,
                    config: [r.u32(0x08)?, r.u32(0x0c)?],
                    slot: r.u32(0x10)?,
                    direction: r.u32(0x14)?,
                    codec: r.u32(0x18)?,
                    tx_pt: r.u32(0x1c)?,
                    rx_pt: r.u32(0x20)?,
                    state: r.u32(0x24)?,
                    tx_ms: r.u32(0x28)?,
                    rx_ms: r.u32(0x2c)?,
                    dtmf: r.u32(0x30)?,
                    tx_event_pt: r.u32(0x34)?,
                    rx_event_pt: r.u32(0x38)?,
                    local4: r.array(0x3c)?,
                    local_port: r.u16(0x40)?,
                    remote4: r.array(0x44)?,
                    remote_port: r.u16(0x48)?,
                    control_port: r.u16(0x4a)?,
                    tos,
                    control_tos,
                    media_state: r.u32(0x50)?,
                    local6: r.array(0x54)?,
                    remote6: r.array(0x64)?,
                    family: r.u32(0x74)?,
                })
            }
        };
        Ok(Record { command, body })
    }
}

/// Bounds-checked BE writer over the body.
struct Cursor<'a> {
    buf: &'a mut [u8],
}

impl Cursor<'_> {
    fn bytes(&mut self, at: usize, src: &[u8]) -> Result<()> {
        let end = at.checked_add(src.len()).ok_or(Error::InvalidArgument)?;
        self.buf
            .get_mut(at..end)
            .ok_or(Error::InvalidArgument)?
            .copy_from_slice(src);
        Ok(())
    }
    fn u32(&mut self, at: usize, v: u32) -> Result<()> {
        self.bytes(at, &v.to_be_bytes())
    }
    fn u16(&mut self, at: usize, v: u16) -> Result<()> {
        self.bytes(at, &v.to_be_bytes())
    }
}

/// Bounds-checked BE reader over the body.
struct Reader<'a> {
    buf: &'a [u8],
}

impl Reader<'_> {
    fn array<const N: usize>(&self, at: usize) -> Result<[u8; N]> {
        let end = at.checked_add(N).ok_or(Error::MessageSize)?;
        self.buf
            .get(at..end)
            .and_then(|s| s.try_into().ok())
            .ok_or(Error::MessageSize)
    }
    fn u32(&self, at: usize) -> Result<u32> {
        self.array(at).map(u32::from_be_bytes)
    }
    fn u16(&self, at: usize) -> Result<u16> {
        self.array(at).map(u16::from_be_bytes)
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Same independent inventory as the C prototype's `check.c`:
    /// (id, meaningful bytes toward the peer).
    const CASES: [(u32, usize); 34] = [
        (0, 4),
        (1, 4),
        (2, 4),
        (3, 4),
        (4, 116),
        (5, 24),
        (6, 4),
        (7, 4),
        (8, 44),
        (9, 128),
        (10, 120),
        (11, 120),
        (12, 4),
        (13, 100),
        (14, 128),
        (15, 8),
        (16, 8),
        (17, 4),
        (18, 4),
        (19, 8),
        (20, 12),
        (21, 8),
        (22, 4),
        (23, 8),
        (24, 8),
        (31, 4),
        (70, 16),
        (71, 8),
        (72, 4),
        (73, 4),
        (74, 128),
        (91, 4),
        (92, 4),
        (93, 4),
    ];

    #[test]
    fn inventory_round_trip() {
        for (id, to_peer) in CASES {
            for dir in [Direction::ToPeer, Direction::FromPeer] {
                let len = match (dir, id) {
                    (Direction::FromPeer, 20) => 8,
                    (Direction::FromPeer, 91) => 128,
                    _ => to_peer,
                };
                let cmd = Command::try_from(id).unwrap();
                assert_eq!(cmd.body_len(dir), len, "id {id}");
                let mut wire = [0u8; RECORD_LEN];
                wire[3] = id as u8;
                for j in 0..len {
                    wire[j + 4] = (j + 1) as u8;
                }
                if id == 10 || id == 11 {
                    // reserved gaps of the media layout
                    wire[6..12].fill(0);
                    wire[20..24].fill(0);
                    wire[70..72].fill(0);
                    wire[82..84].fill(0);
                    wire[120..124].fill(0);
                    wire[123] = if dir == Direction::FromPeer { 2 } else { 10 };
                } else if id == 4 {
                    wire[113..116].fill(0);
                }
                let rec = Record::decode(&wire, dir).unwrap();
                match rec.body {
                    Body::Media(m) => {
                        assert_eq!(m.activation, 0x0102);
                        assert_eq!(m.config[0], 0x090a0b0c);
                        assert_eq!(m.remote_port, 0x494a);
                        assert_eq!(m.remote6[0], 101);
                    }
                    Body::Cid(c) => {
                        assert_eq!(c.line, 0x01020304);
                        assert_eq!(c.name[0], 30);
                        assert_eq!(c.select, 0x71727374);
                    }
                    Body::Tone(t) => {
                        assert_eq!(t.args[1], 0x05060708);
                        assert_eq!(t.text[1][0], 57);
                        assert_eq!(t.last, 0x61626364);
                    }
                    Body::Words(w) => assert_eq!(w[0], 0x01020304),
                    Body::Opaque(_) => assert_eq!(len, 128),
                }
                assert_eq!(rec.encode(dir).unwrap(), wire, "id {id} {dir:?}");
            }
        }
    }

    #[test]
    fn rejections() {
        let mut wire = [0u8; RECORD_LEN];
        wire[3] = 94; // DSPMSG_CMDMAX sentinel
        assert_eq!(
            Record::decode(&wire, Direction::ToPeer),
            Err(Error::Unsupported)
        );
        assert_eq!(
            Record::decode(&wire[..131], Direction::ToPeer),
            Err(Error::MessageSize)
        );
        let bad = Record {
            command: Command::RtpActive,
            body: Body::Words([0; MAX_WORDS]),
        };
        assert_eq!(bad.encode(Direction::ToPeer), Err(Error::InvalidArgument));
        let m = MediaConfig {
            slot: 4,
            family: AF_INET,
            ..Default::default()
        };
        let r = Record {
            command: Command::RtpActive,
            body: Body::Media(m),
        };
        assert_eq!(r.encode(Direction::ToPeer), Err(Error::InvalidArgument));
        assert_eq!(
            Record::words(Command::DspStart, &[0; 33]),
            Err(Error::InvalidArgument)
        );
    }
}
