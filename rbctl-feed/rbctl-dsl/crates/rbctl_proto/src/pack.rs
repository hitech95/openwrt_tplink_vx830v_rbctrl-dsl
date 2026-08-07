//! TX payload encoders for the `0x88B5` board protocol.
//!
//! Ported from `examples/pack.py`. Each function returns a fixed-size array
//! (the full payload including the payload_type byte at index 0). All multi-byte
//! fields are big-endian.
//!
//! ## Enum tables
//!
//! The Python uses string-keyed dicts; we use `#[repr(u8)]` enums for
//! compile-time exhaustiveness. The numeric values match `libcmm.so` exactly.

use crate::frame::OFF_PAYLOAD;

// ─── Enums (extracted from libcmm.so globals, see examples/pack.py) ─────

/// DSL modulation type (`modulationTypes` @ `0x3edc20` in libcmm.so).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modulation {
    AdslAnsiT1413 = 0,
    AdslGdmt = 1,
    AdslGlite = 2,
    AdslGdmtBis = 3,
    Adsl2Plus = 4,
    AdslMultimode = 5,
    Vdsl2 = 6,
    Multimode = 7,
}

/// DSL annex type (`annexTypes` @ `0x3edd20`).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Annex {
    A = 0,
    B = 1,
    I = 2,
    M = 3,
    AL = 4,
    ALM = 5,
    J = 6,
    BJ = 7,
    Auto = 8,
}

/// VDSL2 profile bitmask (`profiles` @ `0x3eddb0`).
///
/// OR together multiple profiles: `Vdsl2Profile::SEVENTEEN_A | Vdsl2Profile::THIRTY_A`.
/// Only meaningful when [`Modulation`] is `Vdsl2` or `Multimode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Vdsl2Profiles(pub u16);

impl Vdsl2Profiles {
    pub const EIGHT_A: Self = Self(0x001);
    pub const EIGHT_B: Self = Self(0x002);
    pub const EIGHT_C: Self = Self(0x004);
    pub const EIGHT_D: Self = Self(0x008);
    pub const TWELVE_A: Self = Self(0x010);
    pub const TWELVE_B: Self = Self(0x020);
    pub const SEVENTEEN_A: Self = Self(0x040);
    pub const THIRTY_A: Self = Self(0x080);
    pub const THIRTYFIVE_B: Self = Self(0x100);

    /// Bitmask value for the wire (big-endian u32 in the payload).
    pub fn bitmask(self) -> u32 { self.0 as u32 }
}

impl core::ops::BitOr for Vdsl2Profiles {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self { Self(self.0 | rhs.0) }
}

/// ATM encapsulation (`oal_atm_linkObjToMsg`).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtmEncap { Llc = 0, Vcmux = 1 }

/// ATM link type (`oal_atm_linkObjToMsg`).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtmLinkType { Eoa = 0, Pppoa = 6, Ipoa = 7 }

/// ATM QoS category (`oal_atm_qosObjToMsg`). The value 0 is unused.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtmQos { Ubr = 1, Cbr = 2, VbrNrt = 3, VbrRt = 4 }

impl AtmQos {
    /// Whether this category carries SCR/MBS fields.
    pub fn carries_scr(self) -> bool { matches!(self, Self::VbrNrt | Self::VbrRt) }
}

// ─── Opcode 1: dsl_config_up (12 bytes) ─────────────────────────────────

/// Build the 12-byte DSL line config descriptor (opcode 1).
///
/// `bitswap` controls byte `[0x02]` (`X_TP_BitswapEnable`).
/// `sra` controls byte `[0x03]` (`X_TP_SRAEnable`).
/// The VDSL2 profile bitmask is only meaningful for `Vdsl2` / `Multimode`;
/// for ADSL modes it is sent as zero (matching `libcmm.so`).
pub fn pack_dsl_line(
    modulation: Modulation,
    annex: Annex,
    bitswap: bool,
    sra: bool,
    profiles: Vdsl2Profiles,
) -> [u8; 12] {
    let bitmask = match modulation {
        Modulation::Vdsl2 | Modulation::Multimode => profiles.bitmask(),
        _ => 0,
    };
    let mut out = [0u8; 12];
    out[0] = modulation as u8;
    out[1] = annex as u8;
    out[2] = bitswap as u8;
    out[3] = sra as u8;
    out[4..8].copy_from_slice(&bitmask.to_be_bytes());
    // out[8..12] already zero (padding)
    out
}

// ─── Opcode 5: atm_link_add (24 bytes) ──────────────────────────────────

/// Parameters for [`pack_atm_link`].
pub struct AtmLinkParams<'a> {
    pub vpi: u8,
    pub vci: u16,
    pub encap: AtmEncap,
    pub link_type: AtmLinkType,
    pub qos: AtmQos,
    pub pcr: u32,
    pub scr: u32,    // ignored unless qos is VBR
    pub mbs: u32,    // ignored unless qos is VBR
    pub vlan_id: u16,
    pub tag_enable: u8,
    pub tag_vid: u16,
    pub tag_pri: u8,
    _phantom: core::marker::PhantomData<&'a ()>,
}

impl Default for AtmLinkParams<'_> {
    fn default() -> Self {
        Self {
            vpi: 0, vci: 0,
            encap: AtmEncap::Llc, link_type: AtmLinkType::Eoa, qos: AtmQos::Ubr,
            pcr: 0, scr: 0, mbs: 0,
            vlan_id: 0, tag_enable: 0, tag_vid: 0xffff, tag_pri: 0xff,
            _phantom: core::marker::PhantomData,
        }
    }
}

/// Build the 24-byte ATM link descriptor (opcode 5).
///
/// `vlan_id` is the *local* vlan (dslVlan + 2000); `tag_*` fields are the
/// 802.1Q tag the board applies to decapsulated frames.
pub fn pack_atm_link(p: &AtmLinkParams<'_>) -> [u8; 24] {
    let scr = if p.qos.carries_scr() { p.scr } else { 0 };
    let mbs = if p.qos.carries_scr() { p.mbs } else { 0 };
    let mut o = [0u8; 24];
    o[0x00] = p.qos as u8;
    o[0x01] = p.vpi;
    o[0x02..0x04].copy_from_slice(&p.vci.to_be_bytes());
    o[0x04..0x08].copy_from_slice(&p.pcr.to_be_bytes());
    o[0x08..0x0c].copy_from_slice(&scr.to_be_bytes());
    o[0x0c..0x10].copy_from_slice(&mbs.to_be_bytes());
    o[0x10] = p.encap as u8;
    o[0x11] = p.link_type as u8;
    o[0x12..0x14].copy_from_slice(&p.vlan_id.to_be_bytes());
    o[0x14] = p.tag_enable;
    o[0x15..0x17].copy_from_slice(&p.tag_vid.to_be_bytes());
    o[0x17] = p.tag_pri;
    o
}

// ─── Opcode 15: ptm_link_add (8 bytes) ──────────────────────────────────

/// Build the 8-byte PTM/VDSL link descriptor (opcode 15).
pub fn pack_ptm_link(tag_enable: u8, tag_vid: u16, tag_pri: u16, vlan_id: u16) -> [u8; 8] {
    let mut o = [0u8; 8];
    o[0] = tag_enable;
    o[1..3].copy_from_slice(&tag_vid.to_be_bytes());
    o[3..5].copy_from_slice(&tag_pri.to_be_bytes());
    // o[5] = reserved (0)
    o[6..8].copy_from_slice(&vlan_id.to_be_bytes());
    o
}

// ─── Opcode 6 / 16: link_del (3 bytes) ──────────────────────────────────

/// Build the 3-byte link-delete descriptor (opcode 6 = ATM, 16 = PTM).
///
/// `op_type` = 3 means delete (the only value used by `libcmm.so`).
pub fn pack_link_del(vlan_id: u16, op_type: u8) -> [u8; 3] {
    let mut o = [0u8; 3];
    o[0..2].copy_from_slice(&vlan_id.to_be_bytes());
    o[2] = op_type;
    o
}

// Re-export OFF_PAYLOAD for documentation cross-reference.
pub const _OFF_PAYLOAD: usize = OFF_PAYLOAD;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dsl_line_vdsl2_annex_b_17a_30a() {
        let dsl = pack_dsl_line(
            Modulation::Vdsl2, Annex::B, true, true,
            Vdsl2Profiles::SEVENTEEN_A | Vdsl2Profiles::THIRTY_A,
        );
        assert_eq!(dsl[0], 6);       // VDSL2
        assert_eq!(dsl[1], 1);       // Annex B
        assert_eq!(dsl[2], 1);       // bitswap enabled
        assert_eq!(dsl[3], 1);       // SRA enabled
        assert_eq!(u32::from_be_bytes(dsl[4..8].try_into().unwrap()), 0x040 | 0x080);
        assert_eq!(&dsl[8..12], &[0; 4]); // padding
    }

    #[test]
    fn dsl_line_adsl_bitmask_forced_zero() {
        let dsl = pack_dsl_line(
            Modulation::Adsl2Plus, Annex::A, false, false,
            Vdsl2Profiles::SEVENTEEN_A, // should be ignored
        );
        assert_eq!(u32::from_be_bytes(dsl[4..8].try_into().unwrap()), 0);
        assert_eq!(dsl[2], 0);       // bitswap disabled
        assert_eq!(dsl[3], 0);       // SRA disabled
    }

    #[test]
    fn atm_link_basic_ubr() {
        let p = AtmLinkParams {
            vpi: 8, vci: 35, encap: AtmEncap::Llc, link_type: AtmLinkType::Eoa,
            qos: AtmQos::Ubr, pcr: 1000, vlan_id: 2035, ..Default::default()
        };
        let atm = pack_atm_link(&p);
        assert_eq!(atm[0], 1);        // UBR
        assert_eq!(atm[1], 8);        // VPI
        assert_eq!(u16::from_be_bytes([atm[2], atm[3]]), 35);   // VCI
        assert_eq!(u32::from_be_bytes(atm[4..8].try_into().unwrap()), 1000); // PCR
        assert_eq!(atm[0x10], 0);     // LLC
        assert_eq!(atm[0x11], 0);     // EoA
        assert_eq!(u16::from_be_bytes([atm[0x12], atm[0x13]]), 2035); // vlan_id
        // SCR/MBS must be zero for UBR
        assert_eq!(&atm[0x08..0x10], &[0; 8]);
    }

    #[test]
    fn atm_link_vbr_carries_scr_mbs() {
        let p = AtmLinkParams {
            vpi: 8, vci: 35, encap: AtmEncap::Vcmux, link_type: AtmLinkType::Pppoa,
            qos: AtmQos::VbrNrt, pcr: 2000, scr: 1000, mbs: 50,
            ..Default::default()
        };
        let vbr = pack_atm_link(&p);
        assert_eq!(u32::from_be_bytes(vbr[0x08..0x0c].try_into().unwrap()), 1000); // SCR
        assert_eq!(u32::from_be_bytes(vbr[0x0c..0x10].try_into().unwrap()), 50);  // MBS
        assert_eq!(vbr[0x10], 1);    // VCMUX
        assert_eq!(vbr[0x11], 6);    // PPPoA
    }

    #[test]
    fn ptm_link() {
        let ptm = pack_ptm_link(1, 100, 2, 2001);
        assert_eq!(ptm[0], 1);
        assert_eq!(u16::from_be_bytes([ptm[1], ptm[2]]), 100);
        assert_eq!(u16::from_be_bytes([ptm[6], ptm[7]]), 2001);
    }

    #[test]
    fn link_del() {
        let d = pack_link_del(2035, 3);
        assert_eq!(d, [0x07, 0xf3, 0x03]); // 2035 = 0x07f3
    }
}
