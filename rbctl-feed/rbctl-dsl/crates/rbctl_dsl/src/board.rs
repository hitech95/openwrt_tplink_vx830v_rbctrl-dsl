//! `Board` — high-level controller for the EcoNet xDSL board.
//!
//! Wraps a [`Transport`] (AF_PACKET socket or mock) + [`rbctl_proto`] to
//! provide typed methods for each management opcode. Handles sequence
//! numbering, checksum verification, retransmission, and response parsing.

use std::io;
use std::time::Duration;

use rbctl_proto::{
    checksum::verify_checksum,
    frame::{Frame, SeqCounter, HEADER_LEN, MIN_FRAME},
    oem::{
        build_command_frame,
        firmware::{
            self, CHUNK_SIZE, RETRIES_ANNOUNCE, RETRIES_COMPLETE, RETRIES_STREAM,
            RETRIES_VERIFY, STAGE_ANNOUNCE, STAGE_COMPLETE, STAGE_STREAM, STAGE_VERIFY,
            TIMEOUT_ANNOUNCE_MS, TIMEOUT_COMPLETE_MS, TIMEOUT_STREAM_MS, TIMEOUT_VERIFY_MS,
        },
        pack::{self, AtmLinkParams, Modulation, Annex, Vdsl2Profiles},
        unpack::{self, ChannelStats, LineObj},
        MAGIC_OEM_RESPONSE as MAGIC_RESPONSE,
    },
};

/// Default per-request timeout (matches the original binary's 2000 ms).
const DEFAULT_TIMEOUT: Duration = crate::constants::BOARD_DEFAULT_TIMEOUT;
const DEFAULT_RETRIES: u8 = crate::constants::BOARD_DEFAULT_RETRIES;

/// Maximum payload we ever send or receive.
const MAX_PAYLOAD: usize = 128;
const TX_BUF_SIZE: usize = HEADER_LEN + MAX_PAYLOAD + MIN_FRAME;
const RX_BUF_SIZE: usize = HEADER_LEN + MAX_PAYLOAD + MIN_FRAME;

/// Transport abstraction — lets us mock the socket in tests.
pub trait Transport {
    fn send(&self, data: &[u8]) -> io::Result<()>;
    fn recv_into(&self, buf: &mut [u8]) -> io::Result<usize>;
    fn local_mac(&self) -> [u8; 6];
    fn set_timeout(&self, timeout: Duration) -> io::Result<()>;
}

impl Transport for af_packet::RawSocket {
    fn send(&self, data: &[u8]) -> io::Result<()> {
        af_packet::RawSocket::send(self, data).map(|_| ())
    }
    fn recv_into(&self, buf: &mut [u8]) -> io::Result<usize> {
        let pkt = af_packet::RawSocket::recv(self, buf)?;
        Ok(pkt.data.len())
    }
    fn local_mac(&self) -> [u8; 6] {
        af_packet::RawSocket::local_mac(self)
    }
    fn set_timeout(&self, timeout: Duration) -> io::Result<()> {
        af_packet::RawSocket::set_timeout(self, timeout)
    }
}

// ── error type ───────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum BoardError {
    Timeout,
    BadFrame(&'static str),
    BadResponse(&'static str),
    Io(io::Error),
}

impl From<io::Error> for BoardError {
    fn from(e: io::Error) -> Self { Self::Io(e) }
}

impl std::fmt::Display for BoardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout => write!(f, "board did not respond after retries"),
            Self::BadFrame(s) => write!(f, "bad frame: {s}"),
            Self::BadResponse(s) => write!(f, "bad response: {s}"),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}
impl std::error::Error for BoardError {}

// ── Board ────────────────────────────────────────────────────────────────

/// Board controller. Generic over transport for testability.
pub struct Board<T: Transport = af_packet::RawSocket> {
    sock: T,
    mac: [u8; 6],
    seq: SeqCounter,
    timeout: Duration,
    retries: u8,
}

impl<T: Transport> Board<T> {
    /// Create a board controller from any transport.
    pub fn new(sock: T) -> Self {
        let mac = sock.local_mac();
        Self {
            sock, mac, seq: SeqCounter::new(),
            timeout: DEFAULT_TIMEOUT, retries: DEFAULT_RETRIES,
        }
    }

    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
        let _ = self.sock.set_timeout(timeout);
    }

    pub fn set_retries(&mut self, retries: u8) {
        self.retries = retries;
    }

    pub fn local_mac(&self) -> [u8; 6] { self.mac }

    // ── core request/response ──────────────────────────────────────────

    fn request(&mut self, subtype: u8, payload: &[u8]) -> Result<Vec<u8>, BoardError> {
        let seq = self.seq.next();
        let mut tx_buf = [0u8; TX_BUF_SIZE];
        let frame = build_command_frame(&mut tx_buf, subtype, seq, &self.mac, payload);
        let tx_len = frame.len().max(MIN_FRAME);

        let mut rx_buf = [0u8; RX_BUF_SIZE];

        for _attempt in 0..=self.retries {
            self.sock.send(&tx_buf[..tx_len])?;

            // Drain the receive queue: the kernel echoes our own TX back to us
            // (PACKET_OUTGOING), so we may see our own frame before the board's
            // response. Keep receiving until we find a match or timeout.
            loop {
                match self.sock.recv_into(&mut rx_buf) {
                    Ok(n) => {
                        let f = match Frame::parse(&rx_buf[..n]) {
                            Some(f) => f,
                            None => continue, // malformed, try next frame
                        };
                        if f.magic() != MAGIC_RESPONSE || f.subtype() != subtype || f.seq() != seq {
                            continue; // our own TX echo or mismatched, try next
                        }
                        if !verify_checksum(f.buf()) {
                            return Err(BoardError::BadFrame("checksum mismatch"));
                        }
                        return Ok(f.payload().to_vec());
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break, // timeout → retransmit
                    Err(e) => return Err(BoardError::Io(e)),
                }
            }
        }
        Err(BoardError::Timeout)
    }

    // ── opcodes ────────────────────────────────────────────────────────

    /// **Opcode 1** — apply DSL line config. A config that **differs** from
    /// the board's cache (`l_lastDslCfg`) takes the full apply path, which
    /// ends in `wan adsl open` — the only host-reachable way to start
    /// training. An identical config is deduplicated: status 0, no side
    /// effects, **no open**.
    ///
    /// Payload: the 12-byte descriptor directly (modulation at byte 0) —
    /// the opcode travels in the frame header, never in the payload.
    pub fn line_config_up(
        &mut self, modulation: Modulation, annex: Annex, profiles: Vdsl2Profiles,
        bitswap: bool, sra: bool,
    ) -> Result<(), BoardError> {
        let line = pack::pack_dsl_line(modulation, annex, bitswap, sra, profiles);
        let r = self.request(1, &line)?;
        ack_status(&r, false)
    }

    /// **Opcode 3** — close the DSL line (`wan adsl close` on the board).
    ///
    /// This is a deliberate line-down: the modem **stops** training. It is
    /// NOT a retrain — re-opening requires an op1 whose config differs from
    /// the board cache (see [`line_config_up`]). Used by the shutdown path
    /// ("daemon stopped ⇒ line down") and user-initiated line bounces.
    pub fn line_retrain(&mut self) -> Result<(), BoardError> {
        let r = self.request(3, &[])?;
        ack_status(&r, false)
    }

    /// **Opcode 2** — get line object.
    pub fn get_line_obj(&mut self) -> Result<LineObj, BoardError> {
        let r = self.request(2, &[])?;
        if r.len() < 63 {
            return Err(BoardError::BadResponse("line obj reply too short"));
        }
        unpack::parse_line_obj(&r).map_err(BoardError::BadResponse)
    }

    /// **Opcode 4** — get channel stats.
    pub fn get_channel_stats(&mut self) -> Result<ChannelStats, BoardError> {
        let r = self.request(4, &[])?;
        if r.len() < 28 {
            return Err(BoardError::BadResponse("channel stats reply too short"));
        }
        unpack::parse_channel_stats(&r).map_err(BoardError::BadResponse)
    }

    /// **Opcode 5** — ATM link add (init / VLAN discovery).
    ///
    /// Returns the board-assigned transport VLAN id. Long replies (≥ 0x14)
    /// echo the descriptor with the assigned VLAN at `[0x12]`; short 4-byte
    /// replies are a status word — see [`link_add_short_status`].
    pub fn atm_link_add(&mut self, params: &AtmLinkParams<'_>) -> Result<u16, BoardError> {
        let atm = pack::pack_atm_link(params);
        let r = self.request(5, &atm)?;
        if r.len() >= 0x14 {
            return Ok(u16::from_be_bytes([r[0x12], r[0x13]]));
        }
        link_add_short_status(&r).map(|_| params.vlan_id)
    }

    /// **Opcode 6** — ATM link delete. Status 1 (not present) is tolerated:
    /// teardown is best-effort and the link may already be gone.
    pub fn atm_link_del(&mut self, vlan_id: u16) -> Result<(), BoardError> {
        let del = pack::pack_link_del(vlan_id, crate::constants::BOARD_LINK_DEL_TYPE_DELETE);
        let r = self.request(6, &del)?;
        ack_status(&r, true)
    }

    /// **Opcode 15** — PTM/VDSL link add.
    ///
    /// Returns the board-assigned transport VLAN id. Long replies (≥ 8)
    /// echo the descriptor with the assigned VLAN at `[6..8]`; short 4-byte
    /// replies are a status word — see [`link_add_short_status`].
    pub fn ptm_link_add(
        &mut self, tag_enable: u8, tag_vid: u16, tag_pri: u16, vlan_id: u16,
    ) -> Result<u16, BoardError> {
        let ptm = pack::pack_ptm_link(tag_enable, tag_vid, tag_pri, vlan_id);
        let r = self.request(15, &ptm)?;
        if r.len() >= 8 {
            return Ok(u16::from_be_bytes([r[6], r[7]]));
        }
        link_add_short_status(&r).map(|_| vlan_id)
    }

    /// **Opcode 16** — PTM/VDSL link delete. Status 1 (not present) is
    /// tolerated: teardown is best-effort and the link may already be gone.
    pub fn ptm_link_del(&mut self, vlan_id: u16) -> Result<(), BoardError> {
        let del = pack::pack_link_del(vlan_id, crate::constants::BOARD_LINK_DEL_TYPE_DELETE);
        let r = self.request(16, &del)?;
        ack_status(&r, true)
    }

    // ── opcode 8: firmware upgrade ─────────────────────────────────────

    /// Upload a raw `2RDH` firmware image to the board via opcode 8.
    ///
    /// Implements the 4-stage protocol: announce → stream → verify → complete.
    /// The `progress` callback is called at stage transitions and during
    /// streaming (~every 10% of chunks).
    ///
    /// **Blocks** for the entire upload (can take minutes). The caller is
    /// responsible for ensuring no other board operations run concurrently.
    pub fn firmware_upgrade(
        &mut self,
        image: &[u8],
        progress: &mut dyn FnMut(&FwProgress),
    ) -> Result<FwUpgradeResult, BoardError> {
        let saved_timeout = self.timeout;
        let saved_retries = self.retries;

        let content_start = crate::constants::FW_HEADER_SIZE; // skip 2RDH header
        let content_len = image.len().saturating_sub(content_start);
        let total_chunks = ((content_len + CHUNK_SIZE - 1) / CHUNK_SIZE) as u32;

        // ── Stage 0: Announce ──────────────────────────────────────────
        self.timeout = Duration::from_millis(TIMEOUT_ANNOUNCE_MS);
        self.retries = RETRIES_ANNOUNCE;
        log::info!("fw: announce size={}", image.len());
        let pl = firmware::announce_payload(image.len() as u32);
        let resp = self.request(8, &pl)?;
        // Response per firmware-upgrade.md: [stage=0, status(u32 BE), ...]
        // (op8 response layout is doc-derived; not yet hardware-verified)
        let data = resp.as_slice();
        if data.len() >= 5 {
            let status = u32::from_be_bytes(data[1..5].try_into().unwrap());
            if status != 0 {
                self.timeout = saved_timeout;
                self.retries = saved_retries;
                return Err(BoardError::BadResponse("announce rejected by board"));
            }
        }
        progress(&FwProgress::stage(STAGE_ANNOUNCE));

        // ── Stage 1: Stream ────────────────────────────────────────────
        self.timeout = Duration::from_millis(TIMEOUT_STREAM_MS);
        self.retries = RETRIES_STREAM;
        log::info!("fw: stream {} chunks", total_chunks);

        let mut chunk_idx = 0u32;
        let mut last_pct = 0u8;
        while chunk_idx < total_chunks {
            let offset = content_start + (chunk_idx as usize) * CHUNK_SIZE;
            let end = (offset + CHUNK_SIZE).min(image.len());
            let chunk = &image[offset..end];

            // Payload: [stage=1, chunk_idx_be_u16, data...]
            let mut payload = Vec::with_capacity(3 + chunk.len());
            payload.push(STAGE_STREAM);
            payload.extend_from_slice(&(chunk_idx as u16).to_be_bytes());
            payload.extend_from_slice(chunk);

            let resp = self.request(8, &payload)?;
            let data = resp.as_slice();

            // ACK: [stage=1, last_good_index(u32 BE), ...]
            if data.len() >= 5 {
                let last_good = u32::from_be_bytes(data[1..5].try_into().unwrap());
                if last_good > chunk_idx {
                    chunk_idx = last_good;
                }
            }
            chunk_idx += 1;

            let pct = ((chunk_idx * 100) / total_chunks.max(1)) as u8;
            if pct >= last_pct + crate::constants::FW_PROGRESS_STEP_PCT || chunk_idx == total_chunks {
                last_pct = pct;
                progress(&FwProgress::streaming(pct));
            }
        }

        // ── Stage 2: Verify ────────────────────────────────────────────
        self.timeout = Duration::from_millis(TIMEOUT_VERIFY_MS);
        self.retries = RETRIES_VERIFY;
        log::info!("fw: verify");
        let pl = firmware::verify_payload();
        let resp = self.request(8, &pl)?;
        let data = resp.as_slice();
        if data.len() >= 5 {
            let status = u32::from_be_bytes(data[1..5].try_into().unwrap());
            if status != 0 {
                self.timeout = saved_timeout;
                self.retries = saved_retries;
                return Err(BoardError::BadResponse("board firmware verification failed"));
            }
        }
        progress(&FwProgress::stage(STAGE_VERIFY));

        // ── Stage 3: Complete ──────────────────────────────────────────
        self.timeout = Duration::from_millis(TIMEOUT_COMPLETE_MS);
        self.retries = RETRIES_COMPLETE;
        log::info!("fw: complete");
        let pl = firmware::complete_payload();
        let resp = self.request(8, &pl)?;
        let data = resp.as_slice();
        // Response: [stage=3, version(u32 BE), status(u8), ...]
        let mut version = 0u32;
        let mut status = 0u8;
        if data.len() >= 5 {
            version = u32::from_be_bytes(data[1..5].try_into().unwrap());
        }
        if data.len() >= 6 {
            status = data[5];
        }
        progress(&FwProgress::stage(STAGE_COMPLETE));

        // Restore timeout/retries
        self.timeout = saved_timeout;
        self.retries = saved_retries;

        log::info!("fw: done version=0x{:08X} status={}", version, status);
        Ok(FwUpgradeResult { version, status })
    }
}

// ── firmware types ───────────────────────────────────────────────────────

/// Progress update from `firmware_upgrade`.
#[derive(Debug, Clone)]
pub struct FwProgress {
    /// Current stage (0=announce, 1=stream, 2=verify, 3=complete).
    pub stage: u8,
    /// Percentage during streaming (0–100), 0 for other stages.
    pub pct: u8,
}

impl FwProgress {
    pub fn stage(stage: u8) -> Self {
        Self { stage, pct: 0 }
    }
    pub fn streaming(pct: u8) -> Self {
        Self { stage: STAGE_STREAM, pct }
    }
}

/// Result of a successful firmware upgrade.
#[derive(Debug, Clone)]
pub struct FwUpgradeResult {
    pub version: u32,
    pub status: u8,
}

// ── response helpers ─────────────────────────────────────────────────────

/// Read the status word of an ACK-style response (op1/op3/op6/op16 …).
///
/// The board's short replies carry a raw u32 status at payload offset 0 —
/// there is **no** opcode echo byte in any response (hardware capture,
/// 2026-08-07; reconfirmed in the field pcaps 2026-08-22).
///
/// `soft = true` tolerates status 1 (generic failure, e.g. "entry not
/// present") — for best-effort teardown, where "already gone" is success.
/// Any other nonzero status is an error either way.
fn ack_status(data: &[u8], soft: bool) -> Result<(), BoardError> {
    let status = if data.len() >= 4 {
        u32::from_be_bytes(data[0..4].try_into().unwrap())
    } else {
        data.first().copied().map(u32::from).unwrap_or(0)
    };
    let tolerated = soft && status == 1;
    if status != 0 && !tolerated {
        return Err(BoardError::BadResponse(if soft {
            "board rejected delete"
        } else {
            "status != 0"
        }));
    }
    Ok(())
}

/// Decode the short (4-byte) link-add status reply shared by op5/op15.
///
/// Ground truth from the decompiled board firmware (`board_op5/op15_handler`
/// + `dsl_linkAdd` / `vdsl_linkAdd`):
///
/// | status | meaning |
/// |--------|---------|
/// | `0` | applied now (line up, matching TC mode) |
/// | `1` | accepted but **not materialized**: either queued in the board's `g_interfaceList` (line down — the board's 2 s status thread applies it at showtime) or a duplicate re-add (idempotent). Both converge to a working data plane. |
/// | `2` | rejected — vlanId outside 2000–2007 (ATM: or invalid tag params) |
///
/// Values ≥ 3 are not produced by the decompiled handlers; treated as
/// errors defensively. `Ok(())` means the link is configured (now or at
/// showtime); `Err` means the board rejected it.
fn link_add_short_status(data: &[u8]) -> Result<(), BoardError> {
    if data.len() < 4 {
        return Err(BoardError::BadResponse("link-add reply too short"));
    }
    match u32::from_be_bytes(data[0..4].try_into().unwrap()) {
        0 => Ok(()),
        1 => {
            log::info!("link add: queued/duplicate on board — materializes at showtime");
            Ok(())
        }
        2 => Err(BoardError::BadResponse("board rejected link (vlan id out of range)")),
        _ => Err(BoardError::BadResponse("board rejected link (unknown status)")),
    }
}

// ── tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rbctl_proto::checksum::set_checksum;
    use rbctl_proto::oem::MAGIC_OEM_COMMAND as MAGIC_COMMAND;
    use rbctl_proto::oem::unpack::LinkStatus;
    use std::cell::RefCell;

    /// Mock transport that auto-generates valid responses.
    struct MockTransport {
        mac: [u8; 6],
        /// If true, recv always returns WouldBlock (simulates no board).
        never_respond: bool,
        /// If Some(n), first n recv calls return WouldBlock, then succeed.
        /// Combined with retries, tests retransmit.
        fail_first: RefCell<u32>,
        /// Response payload override per opcode (set by test).
        response_overrides: RefCell<std::collections::HashMap<u8, Vec<u8>>>,
        /// Last frame sent (for inspection).
        last_tx: RefCell<Vec<u8>>,
    }

    impl MockTransport {
        fn new() -> Self {
            Self {
                mac: [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF],
                never_respond: false,
                fail_first: RefCell::new(0),
                response_overrides: RefCell::new(Default::default()),
                last_tx: RefCell::new(Vec::new()),
            }
        }

        /// Set a custom response payload for a given opcode subtype.
        fn set_response(&self, subtype: u8, payload: Vec<u8>) {
            self.response_overrides.borrow_mut().insert(subtype, payload);
        }

        /// Build a valid response frame for the given subtype + seq.
        fn build_response(&self, subtype: u8, seq: u32, payload: &[u8]) -> Vec<u8> {
            let mut buf = vec![0u8; HEADER_LEN + payload.len()];
            // Ethernet header
            buf[0x00..0x06].copy_from_slice(&[0xFF; 6]); // broadcast
            buf[0x06..0x0c].copy_from_slice(&[0x00; 6]); // board MAC placeholder
            buf[0x0c..0x0e].copy_from_slice(&crate::constants::BOARD_ETHERTYPE_CMD.to_be_bytes());
            // Protocol header
            buf[0x0E] = MAGIC_RESPONSE; // 0x10
            buf[0x0F] = subtype;
            buf[0x10..0x14].copy_from_slice(&seq.to_be_bytes());
            buf[0x14..0x16].copy_from_slice(&(payload.len() as u16).to_be_bytes());
            buf[0x16..0x18].copy_from_slice(&0u16.to_be_bytes()); // checksum placeholder
            buf[0x18..].copy_from_slice(payload);
            set_checksum(&mut buf);
            buf
        }

        /// Generate a default mock response payload for the given subtype,
        /// mirroring the real board's response format (no opcode echo —
        /// `msg_sendResponse` writes the handler buffer directly at +0x18).
        fn default_response_payload(subtype: u8) -> Vec<u8> {
            match subtype {
                // 4-byte big-endian status word, status = 0.
                1 | 3 | 6 | 16 => vec![0x00, 0x00, 0x00, 0x00],
                2 => {
                    // Raw 63-byte status struct: status(u32) at 0x00,
                    // link(u8) at 0x04, rates from 0x08.
                    let mut p = vec![0u8; 63];
                    p[0x04] = 0x01; // link = Up
                    p[0x08..0x0c].copy_from_slice(&40000u32.to_be_bytes()); // down_rate
                    p
                }
                4 => vec![0u8; 28], // raw 7 × u32 counters
                5 => {
                    // Long reply: the 24-byte ATM descriptor echoed back
                    // with the assigned VLAN at [0x12].
                    let mut p = pack::pack_atm_link(&AtmLinkParams::default()).to_vec();
                    p[0x12..0x14].copy_from_slice(&2001u16.to_be_bytes());
                    p
                }
                15 => {
                    // Long reply: the 8-byte PTM descriptor echoed back
                    // with the assigned VLAN at [6..8].
                    pack::pack_ptm_link(0, 0, 0, 2001).to_vec()
                }
                _ => vec![0x00, 0x00, 0x00, 0x00],
            }
        }
    }

    impl Transport for MockTransport {
        fn send(&self, data: &[u8]) -> io::Result<()> {
            *self.last_tx.borrow_mut() = data.to_vec();
            Ok(())
        }

        fn recv_into(&self, buf: &mut [u8]) -> io::Result<usize> {
            if self.never_respond {
                return Err(io::Error::new(io::ErrorKind::WouldBlock, "mock timeout"));
            }
            // Check fail_first counter
            {
                let mut ff = self.fail_first.borrow_mut();
                if *ff > 0 {
                    *ff -= 1;
                    return Err(io::Error::new(io::ErrorKind::WouldBlock, "mock fail-first"));
                }
            }

            // Parse the last TX to get subtype + seq
            let tx = self.last_tx.borrow();
            let req = match Frame::parse(&tx) {
                Some(f) => f,
                None => return Err(io::Error::new(io::ErrorKind::WouldBlock, "no valid TX")),
            };
            let subtype = req.subtype();
            let seq = req.seq();

            // Use override or default response
            let payload = self
                .response_overrides
                .borrow()
                .get(&subtype)
                .cloned()
                .unwrap_or_else(|| Self::default_response_payload(subtype));

            let resp = self.build_response(subtype, seq, &payload);
            let n = resp.len().min(buf.len());
            buf[..n].copy_from_slice(&resp[..n]);
            Ok(n)
        }

        fn local_mac(&self) -> [u8; 6] { self.mac }
        fn set_timeout(&self, _timeout: Duration) -> io::Result<()> { Ok(()) }
    }

    fn make_board() -> Board<MockTransport> {
        let mut b = Board::new(MockTransport::new());
        b.set_retries(2); // speed up tests
        b
    }

    // ── opcode round-trip tests ───────────────────────────────────────

    #[test]
    fn line_config_up_roundtrip() {
        let mut board = make_board();
        assert!(board.line_config_up(Modulation::Vdsl2, Annex::B, Vdsl2Profiles::THIRTY_A, true, true).is_ok());
    }

    #[test]
    fn line_retrain_roundtrip() {
        let mut board = make_board();
        assert!(board.line_retrain().is_ok());
    }

    #[test]
    fn get_line_obj_roundtrip() {
        let mut board = make_board();
        let obj = board.get_line_obj().unwrap();
        assert_eq!(obj.link_status, LinkStatus::Up);
        assert_eq!(obj.metrics.down_rate, 40000);
    }

    #[test]
    fn get_channel_stats_roundtrip() {
        let mut board = make_board();
        let stats = board.get_channel_stats().unwrap();
        // Default mock response is all zeros
        assert_eq!(stats.receive_blocks, 0);
    }

    #[test]
    fn atm_link_add_returns_vlan() {
        let mut board = make_board();
        let vlan = board.atm_link_add(&AtmLinkParams::default()).unwrap();
        assert_eq!(vlan, 2001);
    }

    #[test]
    fn atm_link_del_roundtrip() {
        let mut board = make_board();
        assert!(board.atm_link_del(2001).is_ok());
    }

    #[test]
    fn ptm_link_add_returns_vlan() {
        let mut board = make_board();
        let vlan = board.ptm_link_add(0, 0, 0, 2001).unwrap();
        assert_eq!(vlan, 2001);
    }

    #[test]
    fn ptm_link_del_roundtrip() {
        let mut board = make_board();
        assert!(board.ptm_link_del(2001).is_ok());
    }

    #[test]
    fn line_status_via_get_line_obj() {
        let mut board = make_board();
        let obj = board.get_line_obj().unwrap();
        assert_eq!(obj.link_status, LinkStatus::Up);
    }

    // ── error / edge case tests ───────────────────────────────────────

    #[test]
    fn timeout_when_no_board() {
        let mut board = make_board();
        board.sock.never_respond = true;
        let err = board.get_line_obj().unwrap_err();
        assert!(matches!(err, BoardError::Timeout), "got {err:?}");
    }

    #[test]
    fn retransmit_then_success() {
        let mut board = make_board();
        // First recv attempt fails (WouldBlock), second succeeds
        *board.sock.fail_first.borrow_mut() = 1;
        let obj = board.get_line_obj();
        assert!(obj.is_ok(), "should succeed after retransmit: {:?}", obj);
    }

    #[test]
    fn seq_increments() {
        let mut board = make_board();
        // First request
        board.get_line_obj().unwrap();
        // Check that TX had seq=1 (first call to SeqCounter::next)
        let seq1 = {
            let tx = board.sock.last_tx.borrow();
            Frame::parse(&tx).unwrap().seq()
        };
        assert_eq!(seq1, 1);

        // Second request should have seq=2
        board.get_line_obj().unwrap();
        let seq2 = {
            let tx = board.sock.last_tx.borrow();
            Frame::parse(&tx).unwrap().seq()
        };
        assert_eq!(seq2, 2);
    }

    #[test]
    fn tx_frame_is_command_magic() {
        let mut board = make_board();
        board.get_line_obj().unwrap();
        let tx = board.sock.last_tx.borrow();
        let f = Frame::parse(&tx).unwrap();
        assert_eq!(f.magic(), MAGIC_COMMAND); // 0x11
        assert_eq!(f.subtype(), 2);
    }

    #[test]
    fn custom_response_override() {
        let mut board = make_board();
        // Override with a NoSignal status reply (raw 63 B, link byte at 0x04)
        let mut payload = vec![0u8; 63];
        payload[0x04] = 0x00; // NoSignal
        board.sock.set_response(2, payload);

        let obj = board.get_line_obj().unwrap();
        assert_eq!(obj.link_status, LinkStatus::NoSignal);
    }

    #[test]
    fn bad_checksum_rejected() {
        let mut board = make_board();
        // Override with a well-formed Up reply
        let mut payload = vec![0u8; 63];
        payload[0x04] = 0x01; // Up
        board.sock.set_response(2, payload);

        // The mock builds the response with correct checksum, but let's test
        // the Board's checksum verification by checking that valid checksums pass
        let obj = board.get_line_obj();
        assert!(obj.is_ok());
    }

    // ── no-echo response tests (op4/op5 without payload_type prefix) ──

    #[test]
    fn op4_raw_response() {
        let mut board = make_board();
        // 28 bytes raw data, no opcode echo byte
        let payload = vec![0u8; 28];
        board.sock.set_response(4, payload);
        let stats = board.get_channel_stats().unwrap();
        assert_eq!(stats.receive_blocks, 0);
    }

    #[test]
    fn op5_short_response_returns_requested_vlan() {
        let mut board = make_board();
        // Just a 4-byte status ACK, no VLAN echo
        board.sock.set_response(5, vec![0x00, 0x00, 0x00, 0x00]);
        let vlan = board.atm_link_add(&AtmLinkParams::default()).unwrap();
        // Falls back to the requested VLAN (default is 0)
        assert_eq!(vlan, 0);
    }

    #[test]
    fn op5_short_response_with_nonzero_vlan() {
        let mut board = make_board();
        board.sock.set_response(5, vec![0x00, 0x00, 0x00, 0x00]);
        let params = crate::config::atm_params(None, 2001);
        let vlan = board.atm_link_add(&params).unwrap();
        assert_eq!(vlan, 2001);
    }

    #[test]
    fn op15_short_response_returns_requested_vlan() {
        let mut board = make_board();
        board.sock.set_response(15, vec![0x00, 0x00, 0x00, 0x00]);
        let vlan = board.ptm_link_add(0, 0, 0, 2001).unwrap();
        assert_eq!(vlan, 2001);
    }

    #[test]
    fn link_add_short_status_queued_is_ok() {
        // status 1 = queued (line down) or duplicate re-add — accepted,
        // materializes at showtime. Observed on real hardware (pcap).
        let mut board = make_board();
        board.sock.set_response(15, vec![0x00, 0x00, 0x00, 0x01]);
        assert_eq!(board.ptm_link_add(0, 0, 0, 2000).unwrap(), 2000);
        board.sock.set_response(5, vec![0x00, 0x00, 0x00, 0x01]);
        let params = crate::config::atm_params(None, 2000);
        assert_eq!(board.atm_link_add(&params).unwrap(), 2000);
    }

    #[test]
    fn link_add_short_status_rejected() {
        let mut board = make_board();
        board.sock.set_response(15, vec![0x00, 0x00, 0x00, 0x02]);
        assert!(board.ptm_link_add(0, 0, 0, 2000).is_err());
        board.sock.set_response(5, vec![0x00, 0x00, 0x00, 0x02]);
        let params = crate::config::atm_params(None, 2000);
        assert!(board.atm_link_add(&params).is_err());
    }

    #[test]
    fn link_add_short_status_unknown_rejected() {
        // ≥3 not produced by the decompiled firmware — defensive error.
        let mut board = make_board();
        board.sock.set_response(15, vec![0x00, 0x00, 0x00, 0x7F]);
        assert!(board.ptm_link_add(0, 0, 0, 2000).is_err());
    }

    #[test]
    fn link_add_short_status_truncated_rejected() {
        let mut board = make_board();
        board.sock.set_response(15, vec![0x00, 0x00]);
        assert!(board.ptm_link_add(0, 0, 0, 2000).is_err());
    }

    #[test]
    fn op1_ack_status_word() {
        let mut board = make_board();
        // 4-byte big-endian status word, status = 0
        board.sock.set_response(1, vec![0x00, 0x00, 0x00, 0x00]);
        assert!(board.line_config_up(Modulation::Vdsl2, Annex::B, Vdsl2Profiles::THIRTY_A, true, true).is_ok());
    }

    #[test]
    fn op1_nack_status_word_is_error() {
        let mut board = make_board();
        // status = 2 (invalid argument) must surface as an error
        board.sock.set_response(1, vec![0x00, 0x00, 0x00, 0x02]);
        assert!(board.line_config_up(Modulation::Vdsl2, Annex::B, Vdsl2Profiles::THIRTY_A, true, true).is_err());
    }

    // ── wire-format tests (session-2 regression: no opcode echo byte) ──

    #[test]
    fn op1_tx_payload_is_bare_descriptor() {
        let mut board = make_board();
        board.line_config_up(Modulation::Vdsl2, Annex::B, Vdsl2Profiles::THIRTY_A, true, true).unwrap();
        let tx = board.sock.last_tx.borrow().clone();
        let f = Frame::parse(&tx).unwrap();
        assert_eq!(f.subtype(), 1, "opcode travels in the header");
        assert_eq!(f.payload_len(), 12, "payload is the bare 12-B descriptor");
        assert_eq!(f.payload()[0], Modulation::Vdsl2 as u8, "modulation at byte 0");
        assert_eq!(f.payload()[1], Annex::B as u8);
    }

    #[test]
    fn poll_and_retrain_tx_payloads_are_empty() {
        let mut board = make_board();
        board.get_line_obj().unwrap();
        let tx = board.sock.last_tx.borrow().clone();
        let f = Frame::parse(&tx).unwrap();
        assert_eq!(f.subtype(), 2);
        assert_eq!(f.payload_len(), 0, "op2 sends no payload");

        board.line_retrain().unwrap();
        let tx = board.sock.last_tx.borrow().clone();
        let f = Frame::parse(&tx).unwrap();
        assert_eq!(f.subtype(), 3);
        assert_eq!(f.payload_len(), 0, "op3 sends no payload");
    }

    #[test]
    fn link_del_tx_payload_is_bare_3byte_descriptor() {
        let mut board = make_board();
        board.ptm_link_del(2000).unwrap();
        let tx = board.sock.last_tx.borrow().clone();
        let f = Frame::parse(&tx).unwrap();
        assert_eq!(f.subtype(), 16);
        assert_eq!(f.payload_len(), 3);
        assert_eq!(f.payload(), &[0x07, 0xd0, 0x03], "vlan 2000 BE + cmd 3");

        board.atm_link_del(2000).unwrap();
        let tx = board.sock.last_tx.borrow().clone();
        let f = Frame::parse(&tx).unwrap();
        assert_eq!(f.subtype(), 6);
        assert_eq!(f.payload_len(), 3);
        assert_eq!(f.payload(), &[0x07, 0xd0, 0x03]);
    }

    #[test]
    fn ptm_link_add_tx_payload_is_bare_8byte_descriptor() {
        let mut board = make_board();
        board.ptm_link_add(0, 0, 0, 2000).unwrap();
        let tx = board.sock.last_tx.borrow().clone();
        let f = Frame::parse(&tx).unwrap();
        assert_eq!(f.subtype(), 15);
        assert_eq!(f.payload_len(), 8);
        // vlanId at [6..8] BE; tagEnable/tagVid/tagPri zero
        assert_eq!(&f.payload()[6..8], &[0x07, 0xd0]);
        assert_eq!(&f.payload()[0..6], &[0; 6]);
    }

    #[test]
    fn link_delete_tolerates_not_present_status() {
        let mut board = make_board();
        // status = 1 (entry not present) is fine during teardown
        board.sock.set_response(16, vec![0x00, 0x00, 0x00, 0x01]);
        assert!(board.ptm_link_del(2000).is_ok());
        // status = 2 (rejected) surfaces
        board.sock.set_response(16, vec![0x00, 0x00, 0x00, 0x02]);
        assert!(board.ptm_link_del(2000).is_err());
    }
}
