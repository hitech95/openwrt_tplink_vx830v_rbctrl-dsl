// SPDX-License-Identifier: GPL-2.0-or-later
//! The board link state machine: DD01 requests with retransmission, DD02
//! event delivery with duplicate suppression, and DD03 media.
//!
//! [`Link`] is sans-I/O. The owner:
//! 1. sends requests with [`Link::request`];
//! 2. feeds every received frame to [`Link::receive`];
//! 3. calls [`Link::tick`] at [`Link::deadline`] while a request is pending;
//! 4. drains events with [`Link::pop`] after every receive (even on error).
//!
//! Frames are written synchronously through a [`Transport`] supplied per
//! call; the link never stores it. All calls must come from one owner/event
//! loop. See `docs/voip/foss-design.md` for the integration pattern.
//!
//! Deliberate differences from the OEM proxy (all tested):
//! * replies must match subtype **and** sequence and come from the peer;
//! * a nonzero peer status and transport failures are reported, not hidden;
//! * a refused event (too short or queue full) is answered with a nonzero
//!   status and is **not** cached, so the peer's retry can still succeed;
//! * the first sequence number is caller-seeded (use a random seed: the
//!   peer caches the last sequence across master restarts);
//! * subtype `1c` is rejected — the stock peer never answers it.

use crate::dsp::RECORD_LEN;
use crate::wire::{
    self, Frame, Header, Mac, Media, Selector, BROADCAST, ETH_CONTROL, ETH_EVENT, ETH_MEDIA,
    FRAME_MAX, HEADER_LEN, MARKER_HOST, MARKER_PEER, MIN_CONTROL_FRAME, SUB_ATTACH, SUB_EVENT,
    SUB_INIT, SUB_MEDIA, SUB_RECORD,
};
use crate::{Error, Result};

/// Writes one complete Ethernet frame (untagged, no FCS) synchronously.
///
/// Return `Err(Error::Transport(errno))` on failure. The frame is borrowed
/// only for the duration of the call.
pub trait Transport {
    /// Send `frame` now.
    fn send(&mut self, frame: &[u8]) -> Result<()>;
}

impl<F: FnMut(&[u8]) -> Result<()>> Transport for F {
    fn send(&mut self, frame: &[u8]) -> Result<()> {
        self(frame)
    }
}

/// Retransmit interval for `1a`/`1b`, ms (OEM `4024b0(…, 1000, 120)`).
pub const BOOT_INTERVAL_MS: u64 = 1000;
/// Retransmits allowed for `1a`/`1b` after the first send.
pub const BOOT_RETRIES: u32 = 120;
/// Retransmit interval for `16` DSP records, ms (OEM `4024b0(…, 300, 2)`).
pub const RECORD_INTERVAL_MS: u64 = 300;
/// Retransmits allowed for `16` after the first send.
pub const RECORD_RETRIES: u32 = 2;
/// Depth of the local DD02 event FIFO (matches the OEM host's 15 entries).
pub const EVENT_QUEUE: usize = 15;
/// Init/attach payload: the OEM writes native little-endian `1`, not BE32.
const BOOT_PAYLOAD: [u8; 4] = [1, 0, 0, 0];

/// What [`Link::receive`] accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Received<'a> {
    /// The pending DD01 request completed with status 0.
    Control,
    /// A DD02 event was handled (queued or refused) and answered; drain
    /// with [`Link::pop`].
    Event,
    /// A DD03 datagram; borrows the input frame.
    Media(Media<'a>),
}

/// Retransmit state of the one outstanding DD01 request.
#[derive(Debug, Clone)]
struct Pending {
    frame: [u8; HEADER_LEN + RECORD_LEN],
    len: usize,
    subtype: u8,
    seq: u32,
    interval_ms: u64,
    retries_left: u32,
    sent_ms: u64,
}

/// One master-side session with the voice board.
#[derive(Debug, Clone)]
pub struct Link {
    local: Mac,
    /// Learned DD01/DD02 peer; broadcast until the first matching reply.
    control: Mac,
    /// Learned DD03 peer, independent of `control`.
    media: Mac,
    seq: u32,
    pending: Option<Pending>,
    /// Last status of a completed request (0 = success).
    peer_status: u32,
    /// Cached ACK of the last *accepted* event: (source, sequence, frame).
    last_ack: Option<(Mac, u32, [u8; MIN_CONTROL_FRAME])>,
    events: [[u8; RECORD_LEN]; EVENT_QUEUE],
    first: usize,
    count: usize,
}

impl Link {
    /// New session for local interface address `local`. The first request
    /// uses sequence `seed + 1` (wrapping). Prefer a random seed in
    /// production and a fixed one in tests.
    pub fn new(local: Mac, seed: u32) -> Self {
        Link {
            local,
            control: BROADCAST,
            media: BROADCAST,
            seq: seed,
            pending: None,
            peer_status: 0,
            last_ack: None,
            events: [[0; RECORD_LEN]; EVENT_QUEUE],
            first: 0,
            count: 0,
        }
    }

    /// Local MAC this link transmits from.
    pub fn local_mac(&self) -> Mac {
        self.local
    }
    /// Learned control peer (broadcast until learned).
    pub fn control_peer(&self) -> Mac {
        self.control
    }
    /// Learned media peer (broadcast until learned).
    pub fn media_peer(&self) -> Mac {
        self.media
    }
    /// Sequence of the most recent request.
    pub fn seq(&self) -> u32 {
        self.seq
    }
    /// True while a DD01 request awaits its reply.
    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }
    /// Status of the last completed request (nonzero after
    /// [`Error::PeerStatus`]).
    pub fn peer_status(&self) -> u32 {
        self.peer_status
    }
    /// Monotonic ms at which [`Link::tick`] must next run, if pending.
    pub fn deadline(&self) -> Option<u64> {
        self.pending
            .as_ref()
            .map(|p| p.sent_ms.saturating_add(p.interval_ms))
    }
    /// Number of queued events.
    pub fn queued(&self) -> usize {
        self.count
    }

    /// Start a DD01 request and send it immediately.
    ///
    /// * `SUB_INIT` (`1a`) / `SUB_ATTACH` (`1b`): `record` must be `None`;
    ///   1000 ms interval, 120 retransmits.
    /// * `SUB_RECORD` (`16`): `record` is one encoded 132-byte DSP record;
    ///   300 ms interval, 2 retransmits.
    ///
    /// Errors: [`Error::Busy`] while another request is pending;
    /// [`Error::Unsupported`] for any other subtype (including `1c`);
    /// [`Error::InvalidArgument`] for a missing/extra record; a transport
    /// error (the request is then dropped).
    pub fn request<T: Transport + ?Sized>(
        &mut self,
        tx: &mut T,
        subtype: u8,
        record: Option<&[u8; RECORD_LEN]>,
        now_ms: u64,
    ) -> Result<()> {
        if self.pending.is_some() {
            return Err(Error::Busy);
        }
        let (body, interval_ms, retries_left): (&[u8], u64, u32) = match (subtype, record) {
            (SUB_RECORD, Some(r)) => (r, RECORD_INTERVAL_MS, RECORD_RETRIES),
            (SUB_INIT | SUB_ATTACH, None) => (&BOOT_PAYLOAD, BOOT_INTERVAL_MS, BOOT_RETRIES),
            (SUB_RECORD | SUB_INIT | SUB_ATTACH, _) => return Err(Error::InvalidArgument),
            _ => return Err(Error::Unsupported),
        };
        let seq = self.seq.wrapping_add(1);
        let header = self.header(self.control, ETH_CONTROL, subtype, seq);
        let mut frame = [0u8; HEADER_LEN + RECORD_LEN];
        let len = wire::encode_frame(&mut frame, &header, body)?;
        self.seq = seq;
        self.peer_status = 0;
        self.pending = Some(Pending {
            frame,
            len,
            subtype,
            seq,
            interval_ms,
            retries_left,
            sent_ms: now_ms,
        });
        self.transmit(tx, now_ms)
    }

    /// Drop the pending request without waiting (e.g. to send cleanup after
    /// a lost ACK). Learned peers, sequence and queued events are kept.
    pub fn abandon(&mut self) {
        self.pending = None;
    }

    /// Advance retransmission at monotonic `now_ms`.
    ///
    /// Resends the identical saved frame when the interval has elapsed and
    /// retries remain. Errors: [`Error::TimedOut`] once the last send got its
    /// full interval without a reply; a transport error. Both end the request.
    pub fn tick<T: Transport + ?Sized>(&mut self, tx: &mut T, now_ms: u64) -> Result<()> {
        let Some(p) = self.pending.as_mut() else {
            return Ok(());
        };
        if now_ms.wrapping_sub(p.sent_ms) < p.interval_ms {
            return Ok(());
        }
        if p.retries_left == 0 {
            self.pending = None;
            return Err(Error::TimedOut);
        }
        p.retries_left -= 1;
        self.transmit(tx, now_ms)
    }

    fn transmit<T: Transport + ?Sized>(&mut self, tx: &mut T, now_ms: u64) -> Result<()> {
        let Some(p) = self.pending.as_mut() else {
            return Ok(());
        };
        p.sent_ms = now_ms;
        let frame = p.frame.get(..p.len).ok_or(Error::MessageSize)?;
        let rc = tx.send(frame);
        if rc.is_err() {
            self.pending = None;
        }
        rc
    }

    fn header(&self, dst: Mac, ethertype: u16, subtype: u8, seq: u32) -> Header {
        Header {
            dst,
            src: self.local,
            ethertype,
            marker: MARKER_HOST,
            subtype,
            seq,
        }
    }

    /// Process one received frame.
    ///
    /// Frames not addressed to us, not from the peer marker, or from an
    /// invalid source return [`Error::NotMatched`]. Decode errors
    /// ([`Error::MessageSize`], [`Error::Protocol`], [`Error::BadChecksum`])
    /// pass through. A DD01 reply for the pending request returns
    /// [`Received::Control`] or [`Error::PeerStatus`]. A DD02 event is
    /// answered on DD02 and returns [`Received::Event`] (or the ACK's
    /// transport error — the event is still queued, so drain anyway).
    pub fn receive<'a, T: Transport + ?Sized>(
        &mut self,
        tx: &mut T,
        frame: &'a [u8],
    ) -> Result<Received<'a>> {
        let f = wire::decode_frame(frame)?;
        let h = &f.header;
        let src_ok = h.src[0] & 1 == 0 && h.src != [0; 6] && h.src != self.local;
        if h.marker != MARKER_PEER || !(h.dst == self.local || h.dst == BROADCAST) || !src_ok {
            return Err(Error::NotMatched);
        }
        match h.ethertype {
            ETH_MEDIA => {
                if h.subtype != SUB_MEDIA || !accepts(self.media, h.src) {
                    return Err(Error::NotMatched);
                }
                // Validate before learning: a bad prefix must not steer media.
                let media = wire::decode_media(f.body)?;
                self.media = h.src;
                Ok(Received::Media(media))
            }
            _ if !accepts(self.control, h.src) => Err(Error::NotMatched),
            ETH_EVENT if h.subtype == SUB_EVENT => self.event(tx, &f),
            ETH_EVENT => Err(Error::NotMatched),
            _ => self.reply(&f),
        }
    }

    fn reply(&mut self, f: &Frame<'_>) -> Result<Received<'static>> {
        let h = &f.header;
        // A stale or unrelated reply must not complete this request or
        // teach us a MAC.
        match &self.pending {
            Some(p) if p.seq == h.seq && p.subtype == h.subtype => {}
            _ => return Err(Error::NotMatched),
        }
        let status: [u8; 4] = f.body.try_into().map_err(|_| Error::MessageSize)?;
        self.control = h.src;
        self.pending = None;
        self.peer_status = u32::from_be_bytes(status);
        match self.peer_status {
            0 => Ok(Received::Control),
            s => Err(Error::PeerStatus(s)),
        }
    }

    fn event<T: Transport + ?Sized>(
        &mut self,
        tx: &mut T,
        f: &Frame<'_>,
    ) -> Result<Received<'static>> {
        let h = &f.header;
        if let Some((src, seq, ack)) = &self.last_ack {
            if *src == h.src && *seq == h.seq {
                // Retry of an accepted event: same ACK, no second delivery.
                let ack = *ack;
                return tx.send(&ack).map(|()| Received::Event);
            }
        }
        // 1 = shorter than a record (OEM status), 2 = local FIFO full.
        let status: u32 = if f.body.len() < RECORD_LEN {
            1
        } else if self.count == EVENT_QUEUE {
            2
        } else {
            0
        };
        let mut ack = [0u8; MIN_CONTROL_FRAME];
        let header = self.header(h.src, ETH_EVENT, SUB_EVENT, h.seq);
        wire::encode_frame(&mut ack, &header, &status.to_be_bytes())?;
        if status == 0 {
            let record: [u8; RECORD_LEN] = f
                .body
                .get(..RECORD_LEN)
                .and_then(|r| r.try_into().ok())
                .ok_or(Error::MessageSize)?;
            let slot = (self.first + self.count) % EVENT_QUEUE;
            if let Some(e) = self.events.get_mut(slot) {
                *e = record;
            }
            self.count += 1;
            // Cache before sending: if the ACK is lost, the retry must not
            // enqueue the event twice.
            self.last_ack = Some((h.src, h.seq, ack));
        }
        tx.send(&ack).map(|()| Received::Event)
    }

    /// Take the oldest queued raw event record, if any.
    pub fn pop(&mut self) -> Option<[u8; RECORD_LEN]> {
        if self.count == 0 {
            return None;
        }
        let rec = *self.events.get(self.first)?;
        self.first = (self.first + 1) % EVENT_QUEUE;
        self.count -= 1;
        Some(rec)
    }

    /// Send one RTP/RTCP datagram on DD03 to the learned media peer
    /// (broadcast until learned). Sequence is always 0.
    ///
    /// Errors: [`Error::InvalidArgument`] (slot ≥ 4, empty datagram),
    /// [`Error::MessageSize`], or a transport error.
    pub fn send_media<T: Transport + ?Sized>(
        &mut self,
        tx: &mut T,
        slot: u16,
        selector: Selector,
        datagram: &[u8],
    ) -> Result<()> {
        let mut body = [0u8; FRAME_MAX - HEADER_LEN];
        let n = wire::encode_media(&mut body, slot, selector, datagram)?;
        let mut frame = [0u8; FRAME_MAX];
        let header = self.header(self.media, ETH_MEDIA, SUB_MEDIA, 0);
        let len = wire::encode_frame(
            &mut frame,
            &header,
            body.get(..n).ok_or(Error::MessageSize)?,
        )?;
        tx.send(frame.get(..len).ok_or(Error::MessageSize)?)
    }
}

fn accepts(peer: Mac, src: Mac) -> bool {
    peer == BROADCAST || peer == src
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::wire::{encode_frame, MARKER_PEER};

    const HOST: Mac = [2, 0, 0, 0, 0, 1];
    const PEER: Mac = [2, 0, 0, 0, 0, 2];

    /// Deterministic transport: records writes, can be told to fail.
    #[derive(Default)]
    struct Fake {
        frame: Vec<u8>,
        sends: u32,
        fail: Option<i32>,
    }
    impl Transport for Fake {
        fn send(&mut self, frame: &[u8]) -> Result<()> {
            self.frame = frame.to_vec();
            self.sends += 1;
            self.fail.map_or(Ok(()), |e| Err(Error::Transport(e)))
        }
    }

    fn hdr(ethertype: u16, subtype: u8, seq: u32) -> Header {
        Header {
            dst: HOST,
            src: PEER,
            ethertype,
            marker: MARKER_PEER,
            subtype,
            seq,
        }
    }

    fn inject<'a>(
        l: &mut Link,
        f: &mut Fake,
        h: Header,
        body: &[u8],
        buf: &'a mut [u8; FRAME_MAX],
    ) -> Result<Received<'a>> {
        let n = encode_frame(buf, &h, body).unwrap();
        l.receive(f, &buf[..n])
    }

    #[test]
    fn requests_retry_match_and_fail() {
        let mut f = Fake::default();
        let mut l = Link::new(HOST, u32::MAX - 1);
        let rec = [0u8; RECORD_LEN];
        let mut buf = [0u8; FRAME_MAX];
        assert_eq!(l.request(&mut f, 0x1c, None, 0), Err(Error::Unsupported));
        assert!(!l.is_pending() && f.sends == 0);
        for (k, (sub, step, limit)) in [
            (SUB_INIT, 1000u64, 120u32),
            (SUB_ATTACH, 1000, 120),
            (SUB_RECORD, 300, 2),
        ]
        .into_iter()
        .enumerate()
        {
            let prev = f.sends;
            let r = if sub == SUB_RECORD { Some(&rec) } else { None };
            l.request(&mut f, sub, r, 0).unwrap();
            assert_eq!(l.seq(), (u32::MAX).wrapping_add(k as u32));
            assert_eq!(&f.frame[..6], &BROADCAST);
            assert_eq!(l.request(&mut f, SUB_INIT, None, 0), Err(Error::Busy));
            let saved = f.frame.clone();
            for i in 1..=limit {
                l.tick(&mut f, u64::from(i) * step - 1).unwrap();
                assert_eq!(f.sends, prev + i);
                l.tick(&mut f, u64::from(i) * step).unwrap();
                assert_eq!(f.sends, prev + i + 1);
                assert_eq!(f.frame, saved);
            }
            assert_eq!(
                l.tick(&mut f, u64::from(limit + 1) * step),
                Err(Error::TimedOut)
            );
            assert!(!l.is_pending());
        }
        l.request(&mut f, SUB_INIT, None, 0).unwrap();
        let mut h = hdr(ETH_CONTROL, SUB_INIT, l.seq().wrapping_sub(1));
        assert_eq!(
            inject(&mut l, &mut f, h, &rec[..4], &mut buf),
            Err(Error::NotMatched)
        );
        h.seq = l.seq();
        h.subtype = SUB_ATTACH;
        assert_eq!(
            inject(&mut l, &mut f, h, &rec[..4], &mut buf),
            Err(Error::NotMatched)
        );
        h.subtype = SUB_INIT;
        assert_eq!(
            inject(&mut l, &mut f, h, &rec[..5], &mut buf),
            Err(Error::MessageSize)
        );
        assert_eq!(l.control_peer(), BROADCAST);
        h.dst[5] += 1;
        assert_eq!(
            inject(&mut l, &mut f, h, &rec[..4], &mut buf),
            Err(Error::NotMatched)
        );
        h.dst[5] -= 1;
        assert_eq!(
            inject(&mut l, &mut f, h, &rec[..4], &mut buf),
            Ok(Received::Control)
        );
        assert_eq!(l.control_peer(), PEER);
        l.request(&mut f, SUB_RECORD, Some(&rec), 0).unwrap();
        assert_eq!(&f.frame[..6], &PEER);
        h.seq = l.seq();
        h.subtype = SUB_RECORD;
        h.src[5] += 1;
        assert_eq!(
            inject(&mut l, &mut f, h, &rec[..4], &mut buf),
            Err(Error::NotMatched)
        );
        h.src[5] -= 1;
        assert_eq!(
            inject(&mut l, &mut f, h, &[0, 0, 0, 7], &mut buf),
            Err(Error::PeerStatus(7))
        );
        assert!(!l.is_pending() && l.peer_status() == 7);
        f.fail = Some(libc::ENETDOWN);
        assert_eq!(
            l.request(&mut f, SUB_ATTACH, None, 0),
            Err(Error::Transport(libc::ENETDOWN))
        );
        assert!(!l.is_pending());
        f.fail = None;
        l.request(&mut f, SUB_ATTACH, None, 0).unwrap();
        f.fail = Some(libc::EIO);
        assert_eq!(l.tick(&mut f, 1000), Err(Error::Transport(libc::EIO)));
        assert!(!l.is_pending());
    }

    #[test]
    fn events_cache_refusals_and_media() {
        let mut f = Fake::default();
        let mut l = Link::new(HOST, 7);
        let mut b = [0u8; 133];
        let mut buf = [0u8; FRAME_MAX];
        let mut h = hdr(ETH_EVENT, SUB_EVENT, 0);
        // ACK send fails after enqueue: the event is kept and cached.
        f.fail = Some(libc::EIO);
        assert_eq!(
            inject(&mut l, &mut f, h, &b, &mut buf),
            Err(Error::Transport(libc::EIO))
        );
        assert_eq!(l.queued(), 1);
        f.fail = None;
        let ack = f.frame.clone();
        assert_eq!(inject(&mut l, &mut f, h, &b, &mut buf), Ok(Received::Event));
        assert_eq!(f.frame, ack);
        assert_eq!(l.queued(), 1);
        assert_eq!(f.frame[27], 0);
        assert_eq!(&f.frame[..6], &PEER);
        assert_eq!(&l.pop().unwrap()[..], &b[..132]);
        h.seq += 1;
        assert_eq!(
            inject(&mut l, &mut f, h, &b[..131], &mut buf),
            Ok(Received::Event)
        );
        assert_eq!(f.frame[27], 1);
        assert!(l.pop().is_none());
        for i in 0..15u8 {
            b[7] = i;
            h.seq += 1;
            inject(&mut l, &mut f, h, &b[..132], &mut buf).unwrap();
            assert_eq!(f.frame[27], 0);
        }
        h.seq += 1;
        inject(&mut l, &mut f, h, &b[..132], &mut buf).unwrap();
        assert_ne!(f.frame[27], 0);
        let refused = f.frame.clone();
        for i in 0..15u8 {
            assert_eq!(l.pop().unwrap()[7], i);
        }
        // The refused event was not cached: its retry is accepted now.
        inject(&mut l, &mut f, h, &b[..132], &mut buf).unwrap();
        assert_eq!(f.frame[27], 0);
        assert_ne!(f.frame, refused);
        assert_eq!(l.queued(), 1);
        let ack = f.frame.clone();
        inject(&mut l, &mut f, h, &b[..132], &mut buf).unwrap();
        assert_eq!((f.frame.clone(), l.queued()), (ack, 1));
        assert_eq!(&l.pop().unwrap()[..], &b[..132]);

        l.send_media(&mut f, 1, Selector::Rtp, &b[..12]).unwrap();
        assert_eq!(&f.frame[..6], &BROADCAST);
        let mh = hdr(ETH_MEDIA, SUB_MEDIA, 0);
        assert_eq!(
            inject(&mut l, &mut f, mh, &[0, 2, 0, 5, 1], &mut buf),
            Err(Error::Protocol)
        );
        assert_eq!(l.media_peer(), BROADCAST);
        assert!(matches!(
            inject(&mut l, &mut f, mh, &[0, 2, 0, 4, 1], &mut buf),
            Ok(Received::Media(_))
        ));
        assert_eq!(l.control_peer(), BROADCAST);
        l.send_media(&mut f, 2, Selector::Rtcp, &[1]).unwrap();
        assert_eq!(f.frame.len(), 29);
        assert_eq!(&f.frame[..6], &PEER);
        assert_eq!(&f.frame[24..], &[0, 2, 0, 4, 1]);
    }

    #[test]
    fn own_echo_and_host_marker_ignored() {
        let mut f = Fake::default();
        let mut l = Link::new(HOST, 0);
        let mut buf = [0u8; FRAME_MAX];
        let mut h = hdr(ETH_EVENT, SUB_EVENT, 1);
        h.marker = MARKER_HOST;
        assert_eq!(
            inject(&mut l, &mut f, h, &[0; 132], &mut buf),
            Err(Error::NotMatched)
        );
        h.marker = MARKER_PEER;
        h.src = HOST;
        assert_eq!(
            inject(&mut l, &mut f, h, &[0; 132], &mut buf),
            Err(Error::NotMatched)
        );
    }
}
