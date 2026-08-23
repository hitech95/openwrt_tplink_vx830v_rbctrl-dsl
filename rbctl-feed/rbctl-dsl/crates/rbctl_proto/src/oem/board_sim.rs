//! Board simulator — a host-side model of the real `remote_board` firmware
//! logic, ported 1:1 from the disassembled MIPS binary
//! (`vx830v/xDSL/dsl-binaries/remote_board.board`, analyzed 2026-08-22).
//!
//! Purpose: every TX payload a daemon build produces can be validated
//! against the *board's own* decoding rules, not against a mock that shares
//! the daemon's assumptions. The session-2 field failure (an opcode-echo
//! byte prepended to every payload, shifting all fields) survived 15 days
//! because the test mock fabricated the same misconception.
//!
//! Each rule cites the function it was ported from. The simulator is
//! deliberately small: it models the *observable protocol state machine*
//! (config cache, line open/close, link tables), not the DSL driver.
//!
//! Enabled with `cfg(test)` or the `test-sim` feature (so `rbctl_dsl`'s
//! test suite can reuse it without shipping in release binaries).

#![cfg(any(test, feature = "test-sim"))]

use super::pack::{Annex, Modulation};

/// Status byte returned in 4-byte opcode responses (opcodes.md §return codes).
pub mod status {
    pub const OK: u8 = 0;
    pub const FAIL: u8 = 1;
    pub const INVALID_ARG: u8 = 2;
}

/// A queued data-plane link (op5/op15 received while the line was down —
/// `dsl_insertInterfaceList` analog; the board's 2 s status thread
/// materializes it at showtime).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedLink {
    pub vlan_id: u16,
    pub atm: bool,
}

/// PTM link table record (`g_ptmLinks[8]` @ `0x429820` analog).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PtmLinkRecord {
    pub valid: bool,
    pub tag_vid: u16,
    pub vlan_id: u16,
}

/// Board protocol state — everything the host can observe or influence.
#[derive(Debug)]
pub struct BoardSim {
    /// `l_lastDslCfg` @ `0x429840` — the 8-byte op1 cache. Zeroed at
    /// construction ("board just booted with cache cleared").
    last_dsl_cfg: [u8; 8],
    /// Whether the modem is open for training. Set by a *changed-config*
    /// op1 (finalize writes `wan adsl open`, `FUN_004043a4`), cleared by
    /// op3 (`wan adsl close`, `FUN_00404298`).
    pub line_open: bool,
    /// Out-of-band test input: whether the (simulated) physical line has
    /// reached showtime. Drives op2's link field together with `line_open`.
    pub trained: bool,
    /// Pending op5/op15 requests queued while the line is not up
    /// (`g_interfaceList` analog; status-thread materializes at showtime).
    pub queued_links: Vec<QueuedLink>,
    /// Materialized PTM links (`g_ptmLinks[8]` analog).
    pub ptm_links: Vec<PtmLinkRecord>,
    /// Count of op1 requests that took the full apply path (setter burst +
    /// `wan adsl open`) — the observable that proves a config change.
    pub apply_bursts: usize,
    /// Count of op3 closes performed.
    pub close_count: usize,
}

impl Default for BoardSim {
    fn default() -> Self {
        Self::new()
    }
}

impl BoardSim {
    /// A board that has just booted — cache zeroed, line closed, not
    /// trained. (`msg_init` applies the default config against the zeroed
    /// cache, which *does* take the apply path — see [`boot`].)
    pub fn new() -> Self {
        Self {
            last_dsl_cfg: [0u8; 8],
            line_open: false,
            trained: false,
            queued_links: Vec::new(),
            ptm_links: Vec::new(),
            apply_bursts: 0,
            close_count: 0,
        }
    }

    /// Simulate `msg_init` (lifecycle.md): apply the boot default config
    /// `07 08 01 00 000001ff` against the zeroed cache. Because the cache
    /// differs, this takes the full apply path and ends in
    /// `wan adsl open` — a freshly booted board trains on its own.
    pub fn boot(&mut self) {
        let boot_cfg = [0x07u8, 0x08, 0x01, 0x00, 0x00, 0x00, 0x01, 0xff];
        let _ = self.op1(&boot_cfg);
    }

    /// Opcode 1 — `board_op1_handler` @ `0x408d84` → `dslCfgSet` @ `0x4047d0`.
    ///
    /// * `payload.len() < 8` → status 1 (length gate in the handler).
    /// * modulation byte ≥ 8 → status 2 (`apiSetAdslType` @ `0x403454`
    ///   scans the 8-entry table at `0x429034`; codes are 0-based).
    /// * config-diff check `FUN_00404538`: field-by-field compare against
    ///   the cache. Identical → **no-op** (status 0, nothing happens — no
    ///   `wan adsl open`). Different → apply (cache updated, apply burst
    ///   counted, `line_open = true`).
    pub fn op1(&mut self, payload: &[u8]) -> u8 {
        if payload.len() < 8 {
            return status::FAIL;
        }
        if payload[0] >= 8 {
            return status::INVALID_ARG;
        }
        let cfg: [u8; 8] = payload[..8].try_into().unwrap();
        if cfg == self.last_dsl_cfg {
            // Deduplicated: no setters, no wan adsl open.
            return status::OK;
        }
        self.last_dsl_cfg = cfg;
        self.apply_bursts += 1;
        self.line_open = true; // finalize FUN_004043a4 → "wan adsl open"
        status::OK
    }

    /// Opcode 3 — `board_op3_handler` @ `0x408ef8` → `FUN_00404298`:
    /// writes `wan adsl close`. **Closes** the line; never retrains.
    pub fn op3(&mut self) -> u8 {
        self.line_open = false;
        self.trained = false;
        self.close_count += 1;
        status::OK
    }

    /// Opcode 2 link field — `dslCfgGet` semantics: the line reports `Up`
    /// only when the modem is open **and** showtime has been reached
    /// (set the latter out-of-band via `trained`).
    pub fn link_open_and_trained(&self) -> bool {
        self.line_open && self.trained
    }

    /// Opcode 15 — `board_op15_handler` @ `0x409630` → `vdsl_linkAdd`
    /// (`0x408208`), with queueing.
    ///
    /// Payload (8 B): `tagEnable(1) tagVid(2) tagPri(2) rsv(1) vlanId(2)`,
    /// all big-endian, read from payload byte 0 (`f+0x602` = wire `0x18`).
    ///
    /// * `len < 8` → 1; vlanId ∉ 2000–2007 → 2.
    /// * Line not up (`!link_open_and_trained()`) → queued, status 1
    ///   (benign; materializes at showtime via [`set_showtime`]).
    /// * Duplicate tagVid in the table → 1 (idempotent re-add).
    /// * Table full (8 entries) → 1.
    pub fn op15(&mut self, payload: &[u8]) -> u8 {
        if payload.len() < 8 {
            return status::FAIL;
        }
        let tag_vid = u16::from_be_bytes([payload[1], payload[2]]);
        let vlan_id = u16::from_be_bytes([payload[6], payload[7]]);
        if !(2000..=2007).contains(&vlan_id) {
            return status::INVALID_ARG;
        }
        if !self.link_open_and_trained() {
            self.queued_links.push(QueuedLink { vlan_id, atm: false });
            return status::FAIL; // queued (materializes at showtime)
        }
        self.vdsl_link_add(tag_vid, vlan_id)
    }

    fn vdsl_link_add(&mut self, tag_vid: u16, vlan_id: u16) -> u8 {
        if self.ptm_links.iter().any(|l| l.tag_vid == tag_vid) {
            return status::FAIL; // duplicate re-add — idempotent
        }
        if self.ptm_links.len() >= 8 {
            return status::FAIL; // g_ptmLinks[8] full
        }
        self.ptm_links.push(PtmLinkRecord { valid: true, tag_vid, vlan_id });
        status::OK
    }

    /// Opcode 16 — `board_op16_handler` @ `0x40994c` → `vdsl_linkDel`
    /// (`0x408bd0`).
    ///
    /// Payload (3 B): `vlanId(2 BE) cmd(1)`, cmd 3 = delete. Read from
    /// payload byte 0. `vlanId ∉ 2000–2007` → 2; no matching link → 1.
    pub fn op16(&mut self, payload: &[u8]) -> u8 {
        if payload.len() < 3 {
            return status::FAIL;
        }
        let vlan_id = u16::from_be_bytes([payload[0], payload[1]]);
        let cmd = payload[2];
        if !(2000..=2007).contains(&vlan_id) {
            return status::INVALID_ARG;
        }
        if cmd != 3 {
            return status::INVALID_ARG; // only delete is modeled
        }
        match self.ptm_links.iter().position(|l| l.vlan_id == vlan_id) {
            Some(i) => {
                self.ptm_links.swap_remove(i);
                status::OK
            }
            None => status::FAIL,
        }
    }

    /// Out-of-band: the physical line reached showtime. Materializes any
    /// queued op5/op15 requests (the board's 2 s status thread does this on
    /// the DOWN→UP transition — status-thread.md).
    pub fn set_showtime(&mut self) {
        self.trained = true;
        let queued = core::mem::take(&mut self.queued_links);
        for q in queued {
            if q.atm {
                // ATM links (op5) are not table-modeled here; accepting is
                // enough for protocol-level assertions.
                continue;
            }
            let _ = self.vdsl_link_add(0, q.vlan_id);
        }
    }

    /// The board's cached op1 config (unreadable over the wire — no opcode
    /// exposes `l_lastDslCfg`; this accessor exists for test assertions).
    pub fn last_dsl_cfg(&self) -> &[u8; 8] {
        &self.last_dsl_cfg
    }
}

/// Decode helper mirroring the board's reading of an op1 payload — used by
/// tests to prove what a TX payload *means* to the real firmware.
pub fn decode_op1_view(payload: &[u8]) -> Option<(Modulation, Annex, bool, bool, u32)> {
    if payload.len() < 8 {
        return None;
    }
    let modulation = match payload[0] {
        0 => Modulation::AdslAnsiT1413,
        1 => Modulation::AdslGdmt,
        2 => Modulation::AdslGlite,
        3 => Modulation::AdslGdmtBis,
        4 => Modulation::Adsl2Plus,
        5 => Modulation::AdslMultimode,
        6 => Modulation::Vdsl2,
        7 => Modulation::Multimode,
        _ => return None,
    };
    let annex = match payload[1] {
        0 => Annex::A,
        1 => Annex::B,
        2 => Annex::I,
        3 => Annex::M,
        4 => Annex::AL,
        5 => Annex::ALM,
        6 => Annex::J,
        7 => Annex::BJ,
        8 => Annex::Auto,
        _ => return None,
    };
    let profiles = u32::from_be_bytes(payload[4..8].try_into().unwrap());
    Some((modulation, annex, payload[2] != 0, payload[3] != 0, profiles))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The session-2 wire payloads, captured in sniff2.pcap — kept as
    /// permanent regression fixtures proving what the bug looked like to
    /// the board.
    mod session2_fixtures {
        /// op1 actually sent by the broken daemon (echo byte + intended
        /// Vdsl2/B/bs/sra/profiles 0x1ff). The board read:
        /// mod=1 (gdmt), annex=6 (j), profiles 0x01000001.
        pub const OP1_WITH_ECHO: [u8; 13] =
            [0x01, 0x06, 0x01, 0x01, 0x01, 0x00, 0x00, 0x01, 0xff, 0x00, 0x00, 0x00, 0x00];
        /// op16 delete actually sent (echo byte + vlan 2000, cmd 3). The
        /// board read vlanId = 0x1007 → status 2, every time.
        pub const OP16_WITH_ECHO: [u8; 4] = [0x10, 0x07, 0xd0, 0x03];
    }

    #[test]
    fn boot_applies_default_and_opens() {
        let mut sim = BoardSim::new();
        sim.boot();
        assert!(sim.line_open, "boot apply must end in wan adsl open");
        assert_eq!(sim.apply_bursts, 1);
        assert_eq!(sim.last_dsl_cfg(), &[0x07, 0x08, 0x01, 0x00, 0x00, 0x00, 0x01, 0xff]);
    }

    #[test]
    fn op1_identical_config_is_deduplicated() {
        let mut sim = BoardSim::new();
        sim.boot();
        let before = sim.apply_bursts;
        let cached = *sim.last_dsl_cfg();
        assert_eq!(sim.op1(&cached), status::OK);
        assert_eq!(sim.apply_bursts, before, "identical op1 must be a no-op");
        assert!(sim.line_open, "dedup must not close the line");
    }

    #[test]
    fn op1_differing_config_applies_and_opens() {
        let mut sim = BoardSim::new();
        sim.boot();
        sim.op3(); // daemon shutdown closed it
        assert!(!sim.line_open);
        let mut cfg = *sim.last_dsl_cfg();
        cfg[2] ^= 1; // flip bitswap
        assert_eq!(sim.op1(&cfg), status::OK);
        assert!(sim.line_open, "changed op1 must run wan adsl open");
        assert_eq!(sim.last_dsl_cfg(), &cfg);
    }

    #[test]
    fn op1_garbage_modulation_rejected() {
        let mut sim = BoardSim::new();
        let cfg = [8u8, 1, 1, 1, 0, 0, 0x01, 0xff];
        assert_eq!(sim.op1(&cfg), status::INVALID_ARG);
    }

    #[test]
    fn op1_short_payload_rejected() {
        let mut sim = BoardSim::new();
        assert_eq!(sim.op1(&[0u8; 7]), status::FAIL);
    }

    #[test]
    fn op3_closes_line() {
        let mut sim = BoardSim::new();
        sim.boot();
        sim.set_showtime();
        assert!(sim.link_open_and_trained());
        sim.op3();
        assert!(!sim.line_open);
        assert!(!sim.link_open_and_trained());
    }

    #[test]
    fn double_apply_always_opens_for_every_cache_state() {
        // Property: apply(intended-with-flipped-bitswap) then apply(intended)
        // ends with the intended config active and the line open, for every
        // reachable starting cache state.
        let intended = [0x06u8, 0x01, 0x01, 0x01, 0x00, 0x00, 0x01, 0xff]; // Vdsl2/B/bs/sra/0x1ff
        let mut flipped = intended;
        flipped[2] ^= 1;

        let starts: Vec<[u8; 8]> = vec![
            [0; 8],                                     // fresh boot, cache zeroed
            [0x07, 0x08, 0x01, 0x00, 0x00, 0, 0x01, 0xff], // board boot default
            intended,                                   // previous daemon run left cache == intended
            flipped,                                    // cache == first apply (pathological)
            [0x04, 0x00, 0x00, 0x00, 0, 0, 0, 0],       // foreign writer (adsl2plus/annex a)
        ];
        for start in starts {
            let mut sim = BoardSim::new();
            sim.last_dsl_cfg = start;
            sim.op3(); // worst case: line closed
            sim.op1(&flipped);
            sim.op1(&intended);
            assert!(sim.line_open, "start={start:?}");
            assert_eq!(sim.last_dsl_cfg(), &intended, "start={start:?}");
        }
    }

    #[test]
    fn op15_validates_vlan_range() {
        let mut sim = BoardSim::new();
        sim.boot();
        sim.set_showtime();
        let mut pl = [0u8; 8];
        pl[6..8].copy_from_slice(&1999u16.to_be_bytes());
        assert_eq!(sim.op15(&pl), status::INVALID_ARG);
        pl[6..8].copy_from_slice(&2008u16.to_be_bytes());
        assert_eq!(sim.op15(&pl), status::INVALID_ARG);
    }

    #[test]
    fn op15_queues_while_line_down() {
        let mut sim = BoardSim::new();
        sim.boot(); // open but not trained
        let mut pl = [0u8; 8];
        pl[6..8].copy_from_slice(&2000u16.to_be_bytes());
        assert_eq!(sim.op15(&pl), status::FAIL, "queued → status 1");
        assert_eq!(sim.queued_links.len(), 1);
        sim.set_showtime();
        assert!(sim.queued_links.is_empty(), "materialized at showtime");
        assert_eq!(sim.ptm_links.len(), 1);
    }

    #[test]
    fn op15_duplicate_is_idempotent() {
        let mut sim = BoardSim::new();
        sim.boot();
        sim.set_showtime();
        let mut pl = [0u8; 8];
        pl[6..8].copy_from_slice(&2000u16.to_be_bytes());
        assert_eq!(sim.op15(&pl), status::OK);
        assert_eq!(sim.op15(&pl), status::FAIL, "duplicate re-add → 1");
        assert_eq!(sim.ptm_links.len(), 1);
    }

    #[test]
    fn op16_delete_roundtrip() {
        let mut sim = BoardSim::new();
        sim.boot();
        sim.set_showtime();
        let mut pl = [0u8; 8];
        pl[6..8].copy_from_slice(&2000u16.to_be_bytes());
        sim.op15(&pl);
        let del = [0x07, 0xd0, 0x03];
        assert_eq!(sim.op16(&del), status::OK);
        assert!(sim.ptm_links.is_empty());
        assert_eq!(sim.op16(&del), status::FAIL, "already gone → 1");
    }

    #[test]
    fn session2_op16_fixture_is_rejected_by_board_logic() {
        // The bug's signature: with the echo byte prepended, the board read
        // vlanId=0x1007 and rejected with status 2. This test keeps that
        // failure mode documented forever.
        let mut sim = BoardSim::new();
        sim.boot();
        sim.set_showtime();
        let mut pl = [0u8; 8];
        pl[6..8].copy_from_slice(&2000u16.to_be_bytes());
        sim.op15(&pl);
        assert_eq!(sim.op16(&session2_fixtures::OP16_WITH_ECHO), status::INVALID_ARG);
        // The correct (fixed) payload does delete the link.
        assert_eq!(sim.op16(&[0x07, 0xd0, 0x03]), status::OK);
    }

    #[test]
    fn session2_op1_fixture_decodes_as_garbage_config() {
        // What the board made of our "Vdsl2/B" op1 in session 2.
        let (modulation, annex, _, _, profiles) =
            decode_op1_view(&session2_fixtures::OP1_WITH_ECHO).unwrap();
        assert_eq!(modulation, Modulation::AdslGdmt, "board saw G.992.1");
        assert_eq!(annex, Annex::J, "board saw annex J");
        assert_eq!(profiles, 0x0100_0001);
    }

    #[test]
    fn correct_op1_decodes_as_intended() {
        use super::super::pack::{pack_dsl_line, Vdsl2Profiles};
        let pl = pack_dsl_line(
            Modulation::Vdsl2, Annex::B, true, true,
            Vdsl2Profiles::EIGHT_A | Vdsl2Profiles::SEVENTEEN_A,
        );
        let (modulation, annex, bs, sra, profiles) = decode_op1_view(&pl).unwrap();
        assert_eq!(modulation, Modulation::Vdsl2);
        assert_eq!(annex, Annex::B);
        assert!(bs && sra);
        assert_eq!(profiles, 0x001 | 0x040);
    }
}
