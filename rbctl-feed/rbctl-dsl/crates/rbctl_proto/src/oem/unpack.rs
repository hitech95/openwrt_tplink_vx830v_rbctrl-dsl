//! RX payload decoders for the `0x88B5` board protocol.
//!
//! Ported from `examples/unpack.py`. Field names and offsets are confirmed
//! against the board-side `remote_board` (EcoNet EN7516 MIPS) ground truth
//! — see `docs/xdsl/responses.md` and `docs/server.md`.

use super::pack::{Annex, Modulation, Vdsl2Profiles};

// ─── Link status ────────────────────────────────────────────────────────

/// Canonical line state — the shared vocabulary of the board protocol
/// (op2 `linkStatus` byte), the daemon IPC wire, and the Lantiq/LuCI
/// ubus ABI.
///
/// Board codes map onto the training/showtime states (see [`Self::from_code`]);
/// the remaining variants (`NotInitialized`, `Idle`, `ShowtimeNoSync`) are
/// lifecycle states the board never reports but the daemon/ubus ABI define
/// (e.g. `NotInitialized` before the first successful poll, `ShowtimeNoSync`
/// reserved by the Lantiq state machine).
///
/// Serialized like [`Annex`](super::pack::Annex)/[`Modulation`](super::pack::Modulation):
/// the wire token is the exact variant name, parsed back via `FromStr`.
/// `Unknown` keeps its board code: `Unknown(9)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineState {
    #[default]
    NotInitialized,
    Idle,
    Silent,
    Handshake,
    FullInit,
    ShowtimeNoSync,
    ShowtimeTcSync,
    Unknown(u8),
}

impl LineState {
    /// Decode the op2 `linkStatus` byte (reply offset `0x04`).
    ///
    /// Codes 0 and 2 both mean NoSignal (duplicate in the original firmware)
    /// → [`Self::Silent`]; board code 1 trains with TC-layer sync.
    pub fn from_code(code: u8) -> Self {
        match code {
            0 | 2 => Self::Silent,
            1 => Self::ShowtimeTcSync,
            3 => Self::FullInit,
            4 => Self::Handshake,
            other => Self::Unknown(other),
        }
    }

    /// True for the showtime states (line trained and passing data).
    pub fn is_up(self) -> bool {
        matches!(self, Self::ShowtimeNoSync | Self::ShowtimeTcSync)
    }
}

impl core::fmt::Display for LineState {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotInitialized => f.write_str("NotInitialized"),
            Self::Idle => f.write_str("Idle"),
            Self::Silent => f.write_str("Silent"),
            Self::Handshake => f.write_str("Handshake"),
            Self::FullInit => f.write_str("FullInit"),
            Self::ShowtimeNoSync => f.write_str("ShowtimeNoSync"),
            Self::ShowtimeTcSync => f.write_str("ShowtimeTcSync"),
            Self::Unknown(code) => write!(f, "Unknown({code})"),
        }
    }
}

impl core::str::FromStr for LineState {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "NotInitialized" => Self::NotInitialized,
            "Idle" => Self::Idle,
            "Silent" => Self::Silent,
            "Handshake" => Self::Handshake,
            "FullInit" => Self::FullInit,
            "ShowtimeNoSync" => Self::ShowtimeNoSync,
            "ShowtimeTcSync" => Self::ShowtimeTcSync,
            other => {
                let code = other
                    .strip_prefix("Unknown(")
                    .ok_or(())?
                    .strip_suffix(')')
                    .ok_or(())?;
                Self::Unknown(code.parse().map_err(|_| ())?)
            }
        })
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
/// Field assignment verified EMPIRICALLY against the live board binary
/// (qemu + gdb struct dump + distinct-value injection, session 3): the
/// board's `read_adsl_stats_file` fills a fixed slot table from
/// `/proc/tc3162/adsl_stats` needles and `dslCfgGet` copies the pairs
/// below. Noise margin, attenuation, and output power are dB × 10
/// (e.g. 63 = 6.3 dB). "near-end" rates are DOWNSTREAM (TrendChip
/// convention: near end = what the CPE receives).
#[derive(Debug, Clone, Copy, Default)]
pub struct LineMetrics {
    pub down_rate: u32,              // 0x08 — near-end (DS) rate (kbps)
    pub up_rate: u32,                // 0x0c — far-end (US) rate (kbps)
    pub down_attainable_rate: u32,   // 0x10 — kbps
    pub up_attainable_rate: u32,     // 0x14 — kbps
    pub down_noise_margin: u32,      // 0x18 — dB × 10
    pub up_noise_margin: u32,        // 0x1c — dB × 10
    pub down_attenuation: u32,       // 0x20 — dB × 10
    pub up_attenuation: u32,         // 0x24 — dB × 10
    pub down_output_power: u32,      // 0x28 — dBm × 10
    pub up_output_power: u32,        // 0x2c — dBm × 10
    pub uptime_showtime_secs: u32,   // 0x30 — 'ADSL activetime second'
    pub uptime_total_secs: u32,      // 0x34 — 'ADSL total ativetime second'
}

/// Parsed opcode-2 reply (`dsl_get_line_obj`, 63 bytes).
#[derive(Debug, Clone)]
pub struct LineObj {
    pub status: u32,
    pub link_status: LineState,
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
        link_status: LineState::from_code(data[0x04]),
        modulation_code: data[0x05],
        data_path: DataPath::from_code(data[0x06]),
        annex_code: data[0x07],
        vdsl2_profile_bitmask: u16::from_be_bytes([data[0x39], data[0x3a]]),
        is_atm: data[0x38] != 0,
        uptime_secs: u32_at(0x3b),
        metrics: LineMetrics {
            down_rate: u32_at(0x08),
            up_rate: u32_at(0x0c),
            down_attainable_rate: u32_at(0x10),
            up_attainable_rate: u32_at(0x14),
            down_noise_margin: u32_at(0x18),
            up_noise_margin: u32_at(0x1c),
            down_attenuation: u32_at(0x20),
            up_attenuation: u32_at(0x24),
            down_output_power: u32_at(0x28),
            up_output_power: u32_at(0x2c),
            uptime_showtime_secs: u32_at(0x30),
            uptime_total_secs: u32_at(0x34),
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
        // Fill metrics with the E2E session-3 distinct values
        buf[0x08..0x0c].copy_from_slice(&100_000u32.to_be_bytes()); // down_rate (near-end)
        buf[0x0c..0x10].copy_from_slice(&40_000u32.to_be_bytes());  // up_rate (far-end)
        buf[0x10..0x14].copy_from_slice(&45_000u32.to_be_bytes());  // attain DS
        buf[0x14..0x18].copy_from_slice(&15_000u32.to_be_bytes());  // attain US
        buf[0x18..0x1c].copy_from_slice(&63u32.to_be_bytes());      // down_noise_margin 6.3
        buf[0x1c..0x20].copy_from_slice(&61u32.to_be_bytes());      // up_noise_margin 6.1
        buf[0x20..0x24].copy_from_slice(&201u32.to_be_bytes());     // down_attenuation 20.1
        buf[0x24..0x28].copy_from_slice(&105u32.to_be_bytes());     // up_attenuation 10.5
        buf[0x28..0x2c].copy_from_slice(&142u32.to_be_bytes());     // down_output_power 14.2
        buf[0x2c..0x30].copy_from_slice(&125u32.to_be_bytes());     // up_output_power 12.5
        buf[0x30..0x34].copy_from_slice(&5u32.to_be_bytes());       // uptime showtime
        buf[0x34..0x38].copy_from_slice(&6u32.to_be_bytes());       // uptime total
        buf
    }

    #[test]
    fn parse_line_obj_up_vdsl2() {
        let data = build_line_reply();
        let line = parse_line_obj(&data).unwrap();
        assert_eq!(line.link_status, LineState::ShowtimeTcSync);
        assert_eq!(line.modulation(), Some(Modulation::Vdsl2));
        assert_eq!(line.annex(), Some(Annex::B));
        assert_eq!(line.data_path, DataPath::Ptm);
        assert!(!line.is_atm);
        assert_eq!(line.uptime_secs, 3600);
        assert_eq!(line.vdsl2_profile_bitmask, 0x0c0); // 17a + 30a
        assert_eq!(line.metrics.down_rate, 100_000);
        assert_eq!(line.metrics.up_rate, 40_000);
        // DS/US pair order: empirically verified (session 3)
        assert_eq!(line.metrics.down_attainable_rate, 45_000);
        assert_eq!(line.metrics.up_attainable_rate, 15_000);
        assert_eq!(line.metrics.down_noise_margin, 63);
        assert_eq!(line.metrics.up_noise_margin, 61);
        assert_eq!(line.metrics.down_attenuation, 201);
        assert_eq!(line.metrics.up_attenuation, 105);
        assert_eq!(line.metrics.down_output_power, 142);
        assert_eq!(line.metrics.up_output_power, 125);
        assert_eq!(line.metrics.uptime_showtime_secs, 5);
        assert_eq!(line.metrics.uptime_total_secs, 6);
    }

    #[test]
    fn parse_line_obj_nosignal() {
        let mut data = build_line_reply();
        data[0x04] = 0; // NoSignal
        let line = parse_line_obj(&data).unwrap();
        assert_eq!(line.link_status, LineState::Silent);
        // code 2 is the duplicate NoSignal
        data[0x04] = 2;
        assert_eq!(parse_line_obj(&data).unwrap().link_status, LineState::Silent);
    }

    #[test]
    fn line_state_codes() {
        assert_eq!(LineState::from_code(0), LineState::Silent);
        assert_eq!(LineState::from_code(1), LineState::ShowtimeTcSync);
        assert_eq!(LineState::from_code(3), LineState::FullInit);
        assert_eq!(LineState::from_code(4), LineState::Handshake);
        assert_eq!(LineState::from_code(9), LineState::Unknown(9));
        assert!(LineState::ShowtimeTcSync.is_up());
        assert!(LineState::ShowtimeNoSync.is_up());
        assert!(!LineState::FullInit.is_up());
        assert!(!LineState::Silent.is_up());
    }

    #[test]
    fn line_state_token_roundtrip() {
        use core::str::FromStr;
        for ls in [
            LineState::NotInitialized,
            LineState::Idle,
            LineState::Silent,
            LineState::Handshake,
            LineState::FullInit,
            LineState::ShowtimeNoSync,
            LineState::ShowtimeTcSync,
            LineState::Unknown(9),
        ] {
            let token = ls.to_string();
            assert_eq!(LineState::from_str(&token), Ok(ls), "{token}");
        }
        assert_eq!(LineState::Unknown(9).to_string(), "Unknown(9)");
        assert!(LineState::from_str("NoSignal").is_err()); // old board name
        assert!(LineState::from_str("Up").is_err());
        assert!(LineState::from_str("Unknown(x)").is_err());
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
