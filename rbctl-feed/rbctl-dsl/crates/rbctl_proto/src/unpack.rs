//! RX payload decoders for the `0x88B5` board protocol.
//!
//! Ported from `examples/unpack.py`. Field names and offsets are confirmed
//! against the board-side `remote_board` (EcoNet EN7516 MIPS) ground truth
//! — see `docs/xdsl/responses.md` and `docs/server.md`.

use crate::pack::{Annex, Modulation, Vdsl2Profiles};

// ─── Link status ────────────────────────────────────────────────────────

/// Board-reported link state (`linkStatus` field at reply offset `0x04`).
///
/// Codes 0 and 2 both mean `NoSignal` (duplicate in the original firmware).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkStatus {
    NoSignal,
    Up,
    Initializing,
    EstablishingLink,
    Unknown(u8),
}

impl LinkStatus {
    pub fn from_code(code: u8) -> Self {
        match code {
            0 | 2 => Self::NoSignal,
            1 => Self::Up,
            3 => Self::Initializing,
            4 => Self::EstablishingLink,
            other => Self::Unknown(other),
        }
    }
}

// ─── Data path ──────────────────────────────────────────────────────────

/// Board-reported data path (`dataPath` field at reply offset `0x06`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataPath {
    Atm,
    Ptm,
    Unknown(u8),
}

impl DataPath {
    pub fn from_code(code: u8) -> Self {
        match code {
            0 => Self::Atm,
            1 => Self::Ptm,
            other => Self::Unknown(other),
        }
    }
}

// ─── Opcode 2 reply: get_line_obj (63 bytes) ────────────────────────────

/// Line metrics — 12 × `u32` at fixed offsets, all big-endian.
///
/// Field names and types confirmed against board-side `dslCfgGet`
/// (`FUN_004048c8` on the EcoNet MIPS `remote_board`). Noise margin,
/// attenuation, and output power values are in dB × 10 (e.g. 63 = 6.3 dB).
///
/// The up/down ordering within each pair follows the board's read order
/// from `/proc/tc3162/adsl_stats`; confirm exact DS/US assignment per pair
/// with a live capture.
#[derive(Debug, Clone, Copy, Default)]
pub struct LineMetrics {
    pub down_rate: u32,              // 0x08 — current line rate (kbps)
    pub up_rate: u32,                // 0x0c — current line rate (kbps)
    pub up_output_power: u32,        // 0x10 — dB × 10
    pub down_output_power: u32,      // 0x14 — dB × 10
    pub up_noise_margin: u32,        // 0x18 — dB × 10
    pub down_noise_margin: u32,      // 0x1c — dB × 10
    pub up_attenuation: u32,         // 0x20 — dB × 10
    pub down_attenuation: u32,       // 0x24 — dB × 10
    pub up_attainable_rate: u32,     // 0x28 — kbps
    pub down_attainable_rate: u32,   // 0x2c — kbps
    pub up_crc_errors: u32,          // 0x30 — count
    pub down_crc_errors: u32,        // 0x34 — count
}

/// Parsed opcode-2 reply (`dsl_get_line_obj`, 63 bytes).
#[derive(Debug, Clone)]
pub struct LineObj {
    pub status: u32,
    pub link_status: LinkStatus,
    pub modulation_code: u8,
    pub data_path: DataPath,
    pub annex_code: u8,
    pub vdsl2_profile_bitmask: u16,
    pub is_atm: bool,
    pub uptime_secs: u32,
    pub metrics: LineMetrics,
}

impl LineObj {
    /// Typed modulation if the code is known, else `None`.
    pub fn modulation(&self) -> Option<Modulation> {
        modulation_from_code(self.modulation_code)
    }
    /// Typed annex if the code is known, else `None`.
    pub fn annex(&self) -> Option<Annex> {
        annex_from_code(self.annex_code)
    }
    /// Resolved VDSL2 profiles (only meaningful when modulation == VDSL2).
    pub fn vdsl2_profiles(&self) -> Option<Vdsl2Profiles> {
        if self.modulation_code == Modulation::Vdsl2 as u8 {
            Some(Vdsl2Profiles(self.vdsl2_profile_bitmask))
        } else {
            None
        }
    }
}

/// Parse a 63-byte opcode-2 reply.
pub fn parse_line_obj(data: &[u8]) -> Result<LineObj, &'static str> {
    const LEN: usize = 63;
    if data.len() < LEN {
        return Err("line obj reply is 63 B");
    }
    let u32_at = |off: usize| -> u32 {
        u32::from_be_bytes(data[off..off + 4].try_into().unwrap())
    };
    Ok(LineObj {
        status: u32_at(0x00),
        link_status: LinkStatus::from_code(data[0x04]),
        modulation_code: data[0x05],
        data_path: DataPath::from_code(data[0x06]),
        annex_code: data[0x07],
        vdsl2_profile_bitmask: u16::from_be_bytes([data[0x39], data[0x3a]]),
        is_atm: data[0x38] != 0,
        uptime_secs: u32_at(0x3b),
        metrics: LineMetrics {
            down_rate: u32_at(0x08),
            up_rate: u32_at(0x0c),
            up_output_power: u32_at(0x10),
            down_output_power: u32_at(0x14),
            up_noise_margin: u32_at(0x18),
            down_noise_margin: u32_at(0x1c),
            up_attenuation: u32_at(0x20),
            down_attenuation: u32_at(0x24),
            up_attainable_rate: u32_at(0x28),
            down_attainable_rate: u32_at(0x2c),
            up_crc_errors: u32_at(0x30),
            down_crc_errors: u32_at(0x34),
        },
    })
}

// ─── Opcode 4 reply: get_channel_stats (28 bytes) ───────────────────────

/// Parsed opcode-4 reply (`dsl_get_channel_stats`, 28 bytes).
#[derive(Debug, Clone, Copy, Default)]
pub struct ChannelStats {
    pub status: u32,              // 0x00
    pub receive_blocks: u32,      // 0x04
    pub receive_errors: u32,      // 0x08
    pub receive_discards: u32,    // 0x0c
    pub transmit_blocks: u32,     // 0x10
    pub transmit_errors: u32,     // 0x14
    pub transmit_discards: u32,   // 0x18
}

/// Parse a 28-byte opcode-4 reply.
pub fn parse_channel_stats(data: &[u8]) -> Result<ChannelStats, &'static str> {
    const LEN: usize = 28;
    if data.len() < LEN {
        return Err("channel stats reply is 28 B");
    }
    let u32_at = |off: usize| -> u32 {
        u32::from_be_bytes(data[off..off + 4].try_into().unwrap())
    };
    Ok(ChannelStats {
        status: u32_at(0x00),
        receive_blocks: u32_at(0x04),
        receive_errors: u32_at(0x08),
        receive_discards: u32_at(0x0c),
        transmit_blocks: u32_at(0x10),
        transmit_errors: u32_at(0x14),
        transmit_discards: u32_at(0x18),
    })
}

// ─── Reverse enum lookups ───────────────────────────────────────────────

fn modulation_from_code(code: u8) -> Option<Modulation> {
    Some(match code {
        0 => Modulation::AdslAnsiT1413,
        1 => Modulation::AdslGdmt,
        2 => Modulation::AdslGlite,
        3 => Modulation::AdslGdmtBis,
        4 => Modulation::Adsl2Plus,
        5 => Modulation::AdslMultimode,
        6 => Modulation::Vdsl2,
        7 => Modulation::Multimode,
        _ => return None,
    })
}

fn annex_from_code(code: u8) -> Option<Annex> {
    Some(match code {
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_line_reply() -> Vec<u8> {
        let mut buf = vec![0u8; 63];
        // status = 0
        buf[0x04] = 1;   // Up
        buf[0x05] = 6;   // VDSL2
        buf[0x06] = 1;   // PTM
        buf[0x07] = 1;   // Annex B
        buf[0x38] = 0;   // is_atm = false (PTM)
        buf[0x39..0x3b].copy_from_slice(&(0x040u16 | 0x080u16).to_be_bytes()); // 17a + 30a
        buf[0x3b..0x3f].copy_from_slice(&3600u32.to_be_bytes()); // uptime = 1h
        // Fill metrics with distinctive values
        buf[0x08..0x0c].copy_from_slice(&100_000u32.to_be_bytes()); // down_rate
        buf[0x18..0x1c].copy_from_slice(&150u32.to_be_bytes());     // up_noise_margin
        buf[0x1c..0x20].copy_from_slice(&200u32.to_be_bytes());     // down_noise_margin
        buf
    }

    #[test]
    fn parse_line_obj_up_vdsl2() {
        let data = build_line_reply();
        let line = parse_line_obj(&data).unwrap();
        assert_eq!(line.link_status, LinkStatus::Up);
        assert_eq!(line.modulation(), Some(Modulation::Vdsl2));
        assert_eq!(line.annex(), Some(Annex::B));
        assert_eq!(line.data_path, DataPath::Ptm);
        assert!(!line.is_atm);
        assert_eq!(line.uptime_secs, 3600);
        assert_eq!(line.vdsl2_profile_bitmask, 0x0c0); // 17a + 30a
        assert_eq!(line.metrics.down_rate, 100_000);
        assert_eq!(line.metrics.up_noise_margin, 150);
        assert_eq!(line.metrics.down_noise_margin, 200);
    }

    #[test]
    fn parse_line_obj_nosignal() {
        let mut data = build_line_reply();
        data[0x04] = 0; // NoSignal
        let line = parse_line_obj(&data).unwrap();
        assert_eq!(line.link_status, LinkStatus::NoSignal);
    }

    #[test]
    fn parse_line_obj_too_short() {
        assert!(parse_line_obj(&[0; 50]).is_err());
    }

    #[test]
    fn parse_channel_stats_basic() {
        let mut data = [0u8; 28];
        data[0x04..0x08].copy_from_slice(&42u32.to_be_bytes()); // receive_blocks
        data[0x10..0x14].copy_from_slice(&99u32.to_be_bytes()); // transmit_blocks
        let cs = parse_channel_stats(&data).unwrap();
        assert_eq!(cs.receive_blocks, 42);
        assert_eq!(cs.transmit_blocks, 99);
    }

    #[test]
    fn parse_channel_stats_too_short() {
        assert!(parse_channel_stats(&[0; 20]).is_err());
    }
}
