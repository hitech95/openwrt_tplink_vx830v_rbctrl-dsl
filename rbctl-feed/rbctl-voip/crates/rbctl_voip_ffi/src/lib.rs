// SPDX-License-Identifier: GPL-2.0-or-later
//! C ABI for embedding the voice-link protocol in the baresip module.
//!
//! The header `include/rbctl_voip.h` is generated from this file by
//! `cbindgen` (see `cbindgen.toml`); doc comments below become the C
//! comments. Conventions for every function:
//!
//! * integers are native order; wire byte order is handled inside;
//! * return `>= 0` on success and `-errno` on failure (Linux values);
//! * NULL where a pointer is required returns `-EINVAL` (or NULL/0);
//! * buffers are borrowed only for the duration of the call, except the
//!   media datagram view returned by `rbctl_voip_receive`, which borrows the
//!   caller's frame buffer;
//! * a link is single-threaded: serialize all calls on one event loop, and
//!   never call back into the link from the send callback.
//!
//! No function panics on any input; release builds use `panic = "abort"`.
#![allow(non_camel_case_types)]
// FFI types: unions and raw pointers have no meaningful Debug output.
#![allow(missing_debug_implementations)]
#![deny(missing_docs)]

use core::ffi::{c_int, c_void};
use core::ptr;
use core::slice;
use rbctl_voip_proto::dsp::{self, Body, Command, Direction, Record, RECORD_LEN};
use rbctl_voip_proto::link::{Link, Received, Transport};
use rbctl_voip_proto::wire::{self, Selector};
use rbctl_voip_proto::{Error, Result};

/// Bytes in one DSP record (BE32 command + 128-byte body).
pub const RBCTL_VOIP_RECORD_LEN: usize = 132;
/// Largest untagged Ethernet frame handled, excluding FCS.
pub const RBCTL_VOIP_FRAME_MAX: usize = 1514;
/// EtherType returned by `rbctl_voip_receive` for a completed request.
pub const RBCTL_VOIP_CONTROL: c_int = 0xdd01;
/// EtherType returned by `rbctl_voip_receive` for a handled event.
pub const RBCTL_VOIP_EVENT: c_int = 0xdd02;
/// EtherType returned by `rbctl_voip_receive` for a media datagram.
pub const RBCTL_VOIP_MEDIA: c_int = 0xdd03;
/// DD01 subtype: one DSP record.
pub const RBCTL_VOIP_SUB_RECORD: u8 = 0x16;
/// DD01 subtype: initialize the link.
pub const RBCTL_VOIP_SUB_INIT: u8 = 0x1a;
/// DD01 subtype: attach the controller (peer reports DSP state as event 2).
pub const RBCTL_VOIP_SUB_ATTACH: u8 = 0x1b;
/// DD03 selector: RTP.
pub const RBCTL_VOIP_RTP: u16 = 3;
/// DD03 selector: RTCP.
pub const RBCTL_VOIP_RTCP: u16 = 4;

/// Frame writer supplied by the embedder (e.g. AF_PACKET `sendto`).
///
/// Must write `len` bytes of `frame` synchronously and return 0, or a
/// negative errno. `frame` is valid only during the call. Must not call
/// back into the link.
pub type rbctl_voip_send_fn =
    Option<unsafe extern "C" fn(ctx: *mut c_void, frame: *const u8, len: usize) -> c_int>;

/// Opaque link state. Create with `rbctl_voip_link_new`, release with
/// `rbctl_voip_link_free`.
pub struct rbctl_voip_link {
    link: Link,
    send: rbctl_voip_send_fn,
    ctx: *mut c_void,
}

struct CSend {
    send: rbctl_voip_send_fn,
    ctx: *mut c_void,
}

impl Transport for CSend {
    fn send(&mut self, frame: &[u8]) -> Result<()> {
        let f = self.send.ok_or(Error::InvalidArgument)?;
        // SAFETY: the embedder promised a valid callback and context; the
        // frame pointer/len describe a live slice for the call duration.
        let rc = unsafe { f(self.ctx, frame.as_ptr(), frame.len()) };
        if rc == 0 {
            Ok(())
        } else {
            Err(Error::Transport(
                rc.checked_neg().filter(|e| *e > 0).unwrap_or(libc::EIO),
            ))
        }
    }
}

fn neg(e: Error) -> c_int {
    -e.errno()
}

/// Borrow `len` bytes at `p`; NULL is only valid for `len == 0`.
///
/// # Safety
/// `p` must be valid for `len` bytes if non-NULL.
unsafe fn bytes<'a>(p: *const u8, len: usize) -> Option<&'a [u8]> {
    if p.is_null() {
        return (len == 0).then_some(&[]);
    }
    // SAFETY: caller contract.
    Some(unsafe { slice::from_raw_parts(p, len) })
}

/// Split the link into its state machine and a transport for one call.
fn parts(l: &mut rbctl_voip_link) -> (&mut Link, CSend) {
    (
        &mut l.link,
        CSend {
            send: l.send,
            ctx: l.ctx,
        },
    )
}

/// Create a link for the local interface MAC `mac[6]`.
///
/// The first request uses sequence `seed + 1`; pass a random seed (see
/// `rbctl_voip_random_seed`) because the board remembers the last sequence
/// across master restarts. `send` must be non-NULL; `ctx` is passed back to
/// it unchanged. Returns NULL on invalid arguments or allocation failure.
///
/// # Safety
/// `mac` must point to 6 readable bytes.
#[no_mangle]
pub unsafe extern "C" fn rbctl_voip_link_new(
    mac: *const u8,
    seed: u32,
    send: rbctl_voip_send_fn,
    ctx: *mut c_void,
) -> *mut rbctl_voip_link {
    // SAFETY: caller contract for mac.
    let Some(m) = (unsafe { bytes(mac, 6) }) else {
        return ptr::null_mut();
    };
    let (Ok(mac), Some(_)) = (<[u8; 6]>::try_from(m), send) else {
        return ptr::null_mut();
    };
    Box::into_raw(Box::new(rbctl_voip_link {
        link: Link::new(mac, seed),
        send,
        ctx,
    }))
}

/// Release a link created by `rbctl_voip_link_new`. NULL is ignored.
///
/// # Safety
/// `link` must come from `rbctl_voip_link_new` and not be used afterwards.
#[no_mangle]
pub unsafe extern "C" fn rbctl_voip_link_free(link: *mut rbctl_voip_link) {
    if !link.is_null() {
        // SAFETY: allocated by Box::into_raw in rbctl_voip_link_new.
        drop(unsafe { Box::from_raw(link) });
    }
}

/// Fill `*out` with a random seed from `getrandom(2)`.
/// Returns 0, `-EINVAL` for NULL, or `-errno` from the kernel.
///
/// # Safety
/// `out` must be valid for writing a `uint32_t`.
#[no_mangle]
pub unsafe extern "C" fn rbctl_voip_random_seed(out: *mut u32) -> c_int {
    if out.is_null() {
        return -libc::EINVAL;
    }
    let mut b = [0u8; 4];
    // SAFETY: b is valid for 4 bytes.
    let n = unsafe { libc::getrandom(b.as_mut_ptr().cast(), b.len(), 0) };
    if n != 4 {
        return -std::io::Error::last_os_error()
            .raw_os_error()
            .unwrap_or(libc::EIO);
    }
    // SAFETY: out checked non-NULL; caller contract.
    unsafe { out.write(u32::from_ne_bytes(b)) };
    0
}

/// Start a DD01 request and send it now.
///
/// `sub` = `RBCTL_VOIP_SUB_INIT`/`_ATTACH` with `record = NULL, len = 0`, or
/// `RBCTL_VOIP_SUB_RECORD` with one encoded 132-byte record. `now_ms` is a
/// monotonic clock in milliseconds. Returns 0, `-EBUSY` (request pending),
/// `-EOPNOTSUPP` (other subtype, incl. 0x1c), `-EINVAL`, or the send error.
///
/// # Safety
/// `link` from `rbctl_voip_link_new`; `record` valid for `len` bytes.
#[no_mangle]
pub unsafe extern "C" fn rbctl_voip_request(
    link: *mut rbctl_voip_link,
    sub: u8,
    record: *const u8,
    len: usize,
    now_ms: u64,
) -> c_int {
    // SAFETY: caller contract.
    let (Some(l), Some(r)) = (unsafe { link.as_mut() }, unsafe { bytes(record, len) }) else {
        return -libc::EINVAL;
    };
    let rec: Option<&[u8; RECORD_LEN]> = match r.len() {
        0 => None,
        _ => match r.try_into() {
            Ok(a) => Some(a),
            Err(_) => return -libc::EINVAL,
        },
    };
    let (link, mut tx) = parts(l);
    link.request(&mut tx, sub, rec, now_ms)
        .map_or_else(neg, |()| 0)
}

/// Drop the pending request without waiting for its reply (e.g. to send a
/// cleanup command after a lost ACK). Learned peers and queued events stay.
///
/// # Safety
/// `link` from `rbctl_voip_link_new` or NULL (ignored).
#[no_mangle]
pub unsafe extern "C" fn rbctl_voip_abandon(link: *mut rbctl_voip_link) {
    // SAFETY: caller contract.
    if let Some(l) = unsafe { link.as_mut() } {
        l.link.abandon();
    }
}

/// Retransmit if due. Call at the time from `rbctl_voip_deadline`.
/// Returns 0, `-ETIMEDOUT` when the retry budget ran out, or the send
/// error (both end the request).
///
/// # Safety
/// `link` from `rbctl_voip_link_new`.
#[no_mangle]
pub unsafe extern "C" fn rbctl_voip_tick(link: *mut rbctl_voip_link, now_ms: u64) -> c_int {
    // SAFETY: caller contract.
    let Some(l) = (unsafe { link.as_mut() }) else {
        return -libc::EINVAL;
    };
    let (link, mut tx) = parts(l);
    link.tick(&mut tx, now_ms).map_or_else(neg, |()| 0)
}

/// If a request is pending, store the monotonic ms of the next required
/// `rbctl_voip_tick` in `*out` and return 1; return 0 when idle.
///
/// # Safety
/// `link` from `rbctl_voip_link_new`; `out` writable.
#[no_mangle]
pub unsafe extern "C" fn rbctl_voip_deadline(link: *const rbctl_voip_link, out: *mut u64) -> c_int {
    // SAFETY: caller contract.
    let Some(l) = (unsafe { link.as_ref() }) else {
        return -libc::EINVAL;
    };
    match l.link.deadline() {
        Some(d) if !out.is_null() => {
            // SAFETY: out checked non-NULL.
            unsafe { out.write(d) };
            1
        }
        Some(_) => -libc::EINVAL,
        None => 0,
    }
}

/// Status word of the last completed request (nonzero after
/// `-EREMOTEIO`). Returns 0 for NULL.
///
/// # Safety
/// `link` from `rbctl_voip_link_new` or NULL.
#[no_mangle]
pub unsafe extern "C" fn rbctl_voip_peer_status(link: *const rbctl_voip_link) -> u32 {
    // SAFETY: caller contract.
    unsafe { link.as_ref() }.map_or(0, |l| l.link.peer_status())
}

/// A DD03 datagram view returned by `rbctl_voip_receive`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct rbctl_voip_media {
    /// Media slot 0..3.
    pub slot: u16,
    /// `RBCTL_VOIP_RTP` or `RBCTL_VOIP_RTCP`.
    pub selector: u16,
    /// Complete RTP/RTCP datagram; points into the caller's frame buffer.
    pub datagram: *const u8,
    /// Datagram length in bytes.
    pub len: usize,
}

/// Process one received frame (untagged, no FCS).
///
/// Returns `RBCTL_VOIP_CONTROL` (pending request completed with status 0),
/// `RBCTL_VOIP_EVENT` (event answered; drain with `rbctl_voip_pop`, also
/// after an error), `RBCTL_VOIP_MEDIA` (fills `*media` if non-NULL), or:
/// `-EAGAIN` not for us, `-EREMOTEIO` nonzero peer status (see
/// `rbctl_voip_peer_status`), `-EMSGSIZE`/`-EPROTO`/`-EBADMSG` malformed,
/// or the ACK send error.
///
/// # Safety
/// `link` from `rbctl_voip_link_new`; `frame` valid for `len` bytes and kept
/// alive while `media->datagram` is used.
#[no_mangle]
pub unsafe extern "C" fn rbctl_voip_receive(
    link: *mut rbctl_voip_link,
    frame: *const u8,
    len: usize,
    media: *mut rbctl_voip_media,
) -> c_int {
    // SAFETY: caller contract.
    let (Some(l), Some(f)) = (unsafe { link.as_mut() }, unsafe { bytes(frame, len) }) else {
        return -libc::EINVAL;
    };
    let (link, mut tx) = parts(l);
    match link.receive(&mut tx, f) {
        Ok(Received::Control) => RBCTL_VOIP_CONTROL,
        Ok(Received::Event) => RBCTL_VOIP_EVENT,
        Ok(Received::Media(m)) => {
            // SAFETY: media is NULL or writable per caller contract.
            if let Some(out) = unsafe { media.as_mut() } {
                *out = rbctl_voip_media {
                    slot: m.slot,
                    selector: m.selector as u16,
                    datagram: m.datagram.as_ptr(),
                    len: m.datagram.len(),
                };
            }
            RBCTL_VOIP_MEDIA
        }
        Err(e) => neg(e),
    }
}

/// Copy the oldest queued event record into `out[132]`.
/// Returns 0, `-EAGAIN` when empty, `-EINVAL` for NULL.
///
/// # Safety
/// `link` from `rbctl_voip_link_new`; `out` writable for 132 bytes.
#[no_mangle]
pub unsafe extern "C" fn rbctl_voip_pop(link: *mut rbctl_voip_link, out: *mut u8) -> c_int {
    // SAFETY: caller contract.
    let Some(l) = (unsafe { link.as_mut() }) else {
        return -libc::EINVAL;
    };
    if out.is_null() {
        return -libc::EINVAL;
    }
    match l.link.pop() {
        Some(r) => {
            // SAFETY: out writable for RECORD_LEN bytes.
            unsafe { ptr::copy_nonoverlapping(r.as_ptr(), out, RECORD_LEN) };
            0
        }
        None => -libc::EAGAIN,
    }
}

/// Send one RTP (`selector` 3) or RTCP (4) datagram on `slot` 0..3.
/// Returns 0, `-EINVAL`, `-EMSGSIZE`, or the send error.
///
/// # Safety
/// `link` from `rbctl_voip_link_new`; `datagram` valid for `len` bytes.
#[no_mangle]
pub unsafe extern "C" fn rbctl_voip_send_media(
    link: *mut rbctl_voip_link,
    slot: u16,
    selector: u16,
    datagram: *const u8,
    len: usize,
) -> c_int {
    // SAFETY: caller contract.
    let (Some(l), Some(d)) = (unsafe { link.as_mut() }, unsafe { bytes(datagram, len) }) else {
        return -libc::EINVAL;
    };
    let Ok(sel) = Selector::try_from(selector) else {
        return -libc::EINVAL;
    };
    let (link, mut tx) = parts(l);
    link.send_media(&mut tx, slot, sel, d)
        .map_or_else(neg, |()| 0)
}

/// CRC-16/ARC (init 0, reflected 0xa001, no final XOR) of `p[len]`.
/// Returns 0 for NULL with nonzero `len`.
///
/// # Safety
/// `p` valid for `len` bytes.
#[no_mangle]
pub unsafe extern "C" fn rbctl_voip_crc16(p: *const u8, len: usize) -> u16 {
    // SAFETY: caller contract.
    unsafe { bytes(p, len) }.map_or(0, wire::crc16)
}

// ── DSP records ──────────────────────────────────────────────────────────

/// `CID_PLAY` body (command 4). Offsets are decimal, body-relative. The
/// board copies at most 15 bytes of name and number.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct rbctl_voip_cid {
    /// 0: analog line 0/1.
    pub line: u32,
    /// 4..19: opaque words (zero).
    pub attr: [u32; 4],
    /// 20: date text, e.g. "MMDDhhmm" + NUL.
    pub date: [u8; 9],
    /// 29: name bytes, NUL-padded.
    pub name: [u8; 40],
    /// 69: number bytes, NUL-padded.
    pub number: [u8; 40],
    /// 112: line-selection mask (1 << line).
    pub select: u32,
}

/// `TONE_PLAY` body (command 13). Offsets decimal, body-relative.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct rbctl_voip_tone {
    /// 0..15: target, selector (4 dial tone, 0 stop), two arguments.
    pub args: [u32; 4],
    /// 16, 56: two 40-byte text fields.
    pub text: [[u8; 40]; 2],
    /// 96: final argument.
    pub last: u32,
}

/// `RTP_ACTIVE`/`RTP_UPDATE` body (commands 10/11). Offsets hex,
/// body-relative. For IPv4 the board substitutes its own loopback
/// addresses and ports, so these never reach the network.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct rbctl_voip_media_cfg {
    /// 00: activation argument (OEM 0xfffe).
    pub activation: u16,
    /// 08, 0c: controller configuration words.
    pub config: [u32; 2],
    /// 10: slot 0..3.
    pub slot: u32,
    /// 14: 0 none (no RTP), 1 sendonly, 2 recvonly, 3 sendrecv.
    pub direction: u32,
    /// 18: DSP codec: PCMU 0, G726-32 1, PCMA 2, G722 3, G729 4, T.38 5.
    pub codec: u32,
    /// 1c: TX voice RTP payload type.
    pub tx_pt: u32,
    /// 20: RX voice RTP payload type.
    pub rx_pt: u32,
    /// 24: call-state word.
    pub state: u32,
    /// 28: TX packetization ms.
    pub tx_ms: u32,
    /// 2c: RX packetization ms.
    pub rx_ms: u32,
    /// 30: DTMF relay mode (1 = RFC 4733).
    pub dtmf: u32,
    /// 34: TX telephone-event payload type.
    pub tx_event_pt: u32,
    /// 38: RX telephone-event payload type.
    pub rx_event_pt: u32,
    /// 3c: local IPv4 (network octets).
    pub local4: [u8; 4],
    /// 40: local RTP port.
    pub local_port: u16,
    /// 44: remote IPv4 (network octets).
    pub remote4: [u8; 4],
    /// 48: remote RTP port.
    pub remote_port: u16,
    /// 4a: remote RTCP port.
    pub control_port: u16,
    /// 4c: RTP IP TOS.
    pub tos: u8,
    /// 4d: RTCP IP TOS.
    pub control_tos: u8,
    /// 50: media-state word.
    pub media_state: u32,
    /// 54: local IPv6.
    pub local6: [u8; 16],
    /// 64: remote IPv6.
    pub remote6: [u8; 16],
    /// 74: 2 (AF_INET) or 10 (AF_INET6).
    pub family: u32,
}

/// Record body; the arm in use is defined by the command (see
/// docs/voip/board-link.md inventory).
#[repr(C)]
#[derive(Clone, Copy)]
pub union rbctl_voip_body {
    /// Word-layout commands, in inventory order.
    pub words: [u32; 32],
    /// Unknown layouts (commands 9, 14, 74; 91 from the peer).
    pub opaque: [u8; 128],
    /// Command 4.
    pub cid: rbctl_voip_cid,
    /// Command 13.
    pub tone: rbctl_voip_tone,
    /// Commands 10 and 11.
    pub media: rbctl_voip_media_cfg,
}

/// A DSP record: command ID and body. Zero it before filling.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct rbctl_voip_record {
    /// Command ID (native order; BE32 on the wire).
    pub command: u32,
    /// Body arm selected by `command`.
    pub body: rbctl_voip_body,
}

fn direction(from_peer: c_int) -> Direction {
    if from_peer != 0 {
        Direction::FromPeer
    } else {
        Direction::ToPeer
    }
}

fn to_record(r: &rbctl_voip_record, dir: Direction) -> Result<Record> {
    let command = Command::try_from(r.command)?;
    // SAFETY: every arm is plain-old-data covering initialized bytes of the
    // union (callers zero the record first); reading any arm is defined.
    let body = unsafe {
        match command.layout(dir) {
            dsp::Layout::Words(_) => Body::Words(r.body.words),
            dsp::Layout::Opaque => Body::Opaque(r.body.opaque),
            dsp::Layout::Cid => {
                let c = r.body.cid;
                Body::Cid(dsp::Cid {
                    line: c.line,
                    attr: c.attr,
                    date: c.date,
                    name: c.name,
                    number: c.number,
                    select: c.select,
                })
            }
            dsp::Layout::Tone => {
                let t = r.body.tone;
                Body::Tone(dsp::Tone {
                    args: t.args,
                    text: t.text,
                    last: t.last,
                })
            }
            dsp::Layout::Media => {
                let m = r.body.media;
                Body::Media(dsp::MediaConfig {
                    activation: m.activation,
                    config: m.config,
                    slot: m.slot,
                    direction: m.direction,
                    codec: m.codec,
                    tx_pt: m.tx_pt,
                    rx_pt: m.rx_pt,
                    state: m.state,
                    tx_ms: m.tx_ms,
                    rx_ms: m.rx_ms,
                    dtmf: m.dtmf,
                    tx_event_pt: m.tx_event_pt,
                    rx_event_pt: m.rx_event_pt,
                    local4: m.local4,
                    local_port: m.local_port,
                    remote4: m.remote4,
                    remote_port: m.remote_port,
                    control_port: m.control_port,
                    tos: m.tos,
                    control_tos: m.control_tos,
                    media_state: m.media_state,
                    local6: m.local6,
                    remote6: m.remote6,
                    family: m.family,
                })
            }
        }
    };
    Ok(Record { command, body })
}

/// Write `r` into `out`: zero the whole body, then store only the active
/// arm, so every byte the C side may read is initialized.
fn from_record(r: &Record, out: &mut rbctl_voip_record) {
    out.command = r.command.id();
    out.body = rbctl_voip_body { opaque: [0; 128] };
    match &r.body {
        Body::Words(w) => out.body.words = *w,
        Body::Opaque(o) => out.body.opaque = *o,
        Body::Cid(c) => {
            out.body.cid = rbctl_voip_cid {
                line: c.line,
                attr: c.attr,
                date: c.date,
                name: c.name,
                number: c.number,
                select: c.select,
            }
        }
        Body::Tone(t) => {
            out.body.tone = rbctl_voip_tone {
                args: t.args,
                text: t.text,
                last: t.last,
            }
        }
        Body::Media(m) => {
            out.body.media = rbctl_voip_media_cfg {
                activation: m.activation,
                config: m.config,
                slot: m.slot,
                direction: m.direction,
                codec: m.codec,
                tx_pt: m.tx_pt,
                rx_pt: m.rx_pt,
                state: m.state,
                tx_ms: m.tx_ms,
                rx_ms: m.rx_ms,
                dtmf: m.dtmf,
                tx_event_pt: m.tx_event_pt,
                rx_event_pt: m.rx_event_pt,
                local4: m.local4,
                local_port: m.local_port,
                remote4: m.remote4,
                remote_port: m.remote_port,
                control_port: m.control_port,
                tos: m.tos,
                control_tos: m.control_tos,
                media_state: m.media_state,
                local6: m.local6,
                remote6: m.remote6,
                family: m.family,
            }
        }
    }
}

/// Encode `*rec` into `out[132]`. `from_peer` = 0 for master→board
/// layouts, nonzero for board→master. Returns 132, `-EINVAL` (NULL, bad
/// media slot/family) or `-EOPNOTSUPP` (unknown command).
///
/// # Safety
/// `out` writable for 132 bytes; `rec` readable and zero-initialized
/// before its fields were set.
#[no_mangle]
pub unsafe extern "C" fn rbctl_voip_record_encode(
    out: *mut u8,
    rec: *const rbctl_voip_record,
    from_peer: c_int,
) -> c_int {
    // SAFETY: caller contract.
    let Some(r) = (unsafe { rec.as_ref() }) else {
        return -libc::EINVAL;
    };
    if out.is_null() {
        return -libc::EINVAL;
    }
    let dir = direction(from_peer);
    match to_record(r, dir).and_then(|r| r.encode(dir)) {
        Ok(b) => {
            // SAFETY: out writable for RECORD_LEN bytes.
            unsafe { ptr::copy_nonoverlapping(b.as_ptr(), out, RECORD_LEN) };
            RECORD_LEN as c_int
        }
        Err(e) => neg(e),
    }
}

/// Decode `p[len]` (must be 132) into `*rec` (fully overwritten).
/// Returns 0, `-EINVAL`, `-EMSGSIZE`, or `-EOPNOTSUPP` (unknown command).
///
/// # Safety
/// `rec` writable; `p` readable for `len` bytes.
#[no_mangle]
pub unsafe extern "C" fn rbctl_voip_record_decode(
    rec: *mut rbctl_voip_record,
    p: *const u8,
    len: usize,
    from_peer: c_int,
) -> c_int {
    // SAFETY: caller contract.
    let (Some(out), Some(b)) = (unsafe { rec.as_mut() }, unsafe { bytes(p, len) }) else {
        return -libc::EINVAL;
    };
    match Record::decode(b, direction(from_peer)) {
        Ok(r) => {
            from_record(&r, out);
            0
        }
        Err(e) => neg(e),
    }
}
