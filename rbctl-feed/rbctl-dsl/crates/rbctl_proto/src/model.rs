//! Shared DSL vocabulary — the semantic layer both wire protocols map to.
//!
//! `Modulation`, `Annex`, `LineState`, `LineMetrics` … are **DSL
//! concepts**, not implementation details of either wire protocol:
//! the OEM binary opcodes and the protobuf messages are two
//! *encodings* of the same values. This module owns the values; the
//! [`crate::oem`] and [`crate::protobuf`] modules own only the
//! conversions to/from their wire forms, and the daemon consumes
//! [`crate::model`] types exclusively — backend-agnostic by
//! construction.
//!
//! What stays wire-specific:
//!
//! * `oem::pack` — the byte descriptors (`pack_dsl_line`, OEM
//!   `AtmLinkParams` incl. vlan/tag fields)
//! * `oem::unpack` — the reply structs (`LineObj`, `ChannelStats`)
//! * `protobuf::pack` / `protobuf::unpack` — the pb enum bridges and
//!   message builders
//!
//! `no_std`-safe: enums and POD structs only.
//!
//! ## Serializations — who owns what
//!
//! The vocabulary has exactly three wire/text forms, none of them
//! implemented here:
//!
//! | form | owner | mechanism |
//! |---|---|---|
//! | OEM binary codes | [`crate::oem`] | `as u8` / `from_code` (firmware numbering) |
//! | Protobuf enums | [`crate::protobuf`] | bridge fns in `protobuf::pack`/`unpack` |
//! | **Daemon IPC tokens** | this module | `as_str` / `FromStr` |
//!
//! The `as_str`/`FromStr` impls below are the **daemon IPC**
//! serialization (the typed status wire: `state: ShowtimeTcSync`), NOT
//! part of either board protocol — they exist so the host-internal
//! line protocol round-trips the same vocabulary without a second
//! enum. The `from_code` constructors map the firmware's own
//! numbering, shared by both backends (the pb agent reads the same
//! firmware state via `/proc/tc3162/*`).

// ─── Modulation ───────────────────────────────────────────────────────

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

impl Modulation {
    /// Daemon-IPC token (canonical display name). Not a board wire
    /// form — see the module docs on serializations.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AdslAnsiT1413 => "AdslAnsiT1413",
            Self::AdslGdmt => "AdslGdmt",
            Self::AdslGlite => "AdslGlite",
            Self::AdslGdmtBis => "AdslGdmtBis",
            Self::Adsl2Plus => "Adsl2Plus",
            Self::AdslMultimode => "AdslMultimode",
            Self::Vdsl2 => "Vdsl2",
            Self::Multimode => "Multimode",
        }
    }
}

impl core::str::FromStr for Modulation {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "AdslAnsiT1413" => Self::AdslAnsiT1413,
            "AdslGdmt" => Self::AdslGdmt,
            "AdslGlite" => Self::AdslGlite,
            "AdslGdmtBis" => Self::AdslGdmtBis,
            "Adsl2Plus" => Self::Adsl2Plus,
            "AdslMultimode" => Self::AdslMultimode,
            "Vdsl2" => Self::Vdsl2,
            "Multimode" => Self::Multimode,
            _ => return Err(()),
        })
    }
}

// ─── Annex ────────────────────────────────────────────────────────────

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

impl Annex {
    /// Daemon-IPC token (single letter, "Auto" for auto-negotiation).
    /// Not a board wire form — see the module docs.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
            Self::I => "I",
            Self::M => "M",
            Self::AL => "AL",
            Self::ALM => "ALM",
            Self::J => "J",
            Self::BJ => "BJ",
            Self::Auto => "Auto",
        }
    }
}

impl core::str::FromStr for Annex {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "A" => Self::A,
            "B" => Self::B,
            "I" => Self::I,
            "M" => Self::M,
            "AL" => Self::AL,
            "ALM" => Self::ALM,
            "J" => Self::J,
            "BJ" => Self::BJ,
            "Auto" => Self::Auto,
            _ => return Err(()),
        })
    }
}

// ─── VDSL2 profiles ───────────────────────────────────────────────────

/// VDSL2 profile bitmask (`profiles` @ `0x3eddb0`).
///
/// OR together multiple profiles: `Vdsl2Profiles::SEVENTEEN_A | Vdsl2Profiles::THIRTY_A`.
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

    /// Bitmask value for the wire (big-endian u32 in both protocols).
    pub fn bitmask(self) -> u32 { self.0 as u32 }
}

impl core::ops::BitOr for Vdsl2Profiles {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self { Self(self.0 | rhs.0) }
}

// ─── Line state ───────────────────────────────────────────────────────

/// Canonical line state — the shared vocabulary of the board protocol,
/// the daemon IPC wire, and the Lantiq/LuCI ubus ABI.
///
/// Firmware codes map onto the training/showtime states (see
/// [`Self::from_code`]); the remaining variants (`NotInitialized`,
/// `Idle`, `ShowtimeNoSync`) are lifecycle states the board never
/// reports but the daemon/ubus ABI define (e.g. `NotInitialized`
/// before the first successful poll).
///
/// Daemon-IPC serialization like [`Annex`]/[`Modulation`]: the token
/// is the exact variant name, parsed back via `FromStr` (not a board
/// wire form — see the module docs). `Unknown` keeps its code:
/// `Unknown(9)`.
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
    /// Decode the firmware `linkStatus` byte (op2 reply offset `0x04`).
    ///
    /// Codes 0 and 2 both mean NoSignal (duplicate in the original
    /// firmware) → [`Self::Silent`]; code 1 trains with TC-layer sync.
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

// ─── Data path ────────────────────────────────────────────────────────

/// Board-reported data path (firmware `dataPath` field, op2 reply
/// offset `0x06`). A power-on default that does NOT follow training —
/// the trained modulation is the authority for link-type derivation.
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

// ─── Line metrics ─────────────────────────────────────────────────────

/// Line metrics — the unit contract shared by both backends:
/// rates in kbps, noise margin / attenuation in dB × 10, output power
/// in dBm × 10, uptimes in seconds.
///
/// Field assignment verified EMPIRICALLY against the live board binary
/// (qemu + gdb struct dump + distinct-value injection, session 3): the
/// board's `read_adsl_stats_file` fills a fixed slot table from
/// `/proc/tc3162/adsl_stats` needles and `dslCfgGet` copies the pairs
/// below. "near-end" rates are DOWNSTREAM (TrendChip convention:
/// near end = what the CPE receives).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LineMetrics {
    pub down_rate: u32,              // kbps
    pub up_rate: u32,                // kbps
    pub down_attainable_rate: u32,   // kbps
    pub up_attainable_rate: u32,     // kbps
    pub down_noise_margin: u32,      // dB × 10
    pub up_noise_margin: u32,        // dB × 10
    pub down_attenuation: u32,       // dB × 10
    pub up_attenuation: u32,         // dB × 10
    pub down_output_power: u32,      // dBm × 10
    pub up_output_power: u32,        // dBm × 10
    pub uptime_showtime_secs: u32,   // current session
    pub uptime_total_secs: u32,      // cumulative across sessions
}

// ─── ATM link parameters ──────────────────────────────────────────────

/// ATM encapsulation (`oal_atm_linkObjToMsg`).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtmEncap { Llc = 0, Vcmux = 1 }

impl AtmEncap {
    pub fn from_code(code: u8) -> Self {
        match code { 1 => Self::Vcmux, _ => Self::Llc }
    }
}

/// ATM link type (`oal_atm_linkObjToMsg`).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtmLinkType { Eoa = 0, Pppoa = 6, Ipoa = 7 }

impl AtmLinkType {
    pub fn from_code(code: u8) -> Self {
        match code { 6 => Self::Pppoa, 7 => Self::Ipoa, _ => Self::Eoa }
    }
}

/// ATM QoS category (`oal_atm_qosObjToMsg`). The value 0 is unused.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtmQos { Ubr = 1, Cbr = 2, VbrNrt = 3, VbrRt = 4 }

impl AtmQos {
    /// Whether this category carries SCR/MBS fields.
    pub fn carries_scr(self) -> bool { matches!(self, Self::VbrNrt | Self::VbrRt) }

    pub fn from_code(code: u8) -> Self {
        match code { 2 => Self::Cbr, 3 => Self::VbrNrt, 4 => Self::VbrRt, _ => Self::Ubr }
    }
}

// ─── Line / link parameter sets ───────────────────────────────────────

/// Semantic line configuration — exactly the parameters of the OEM
/// op1 descriptor (and the pb `LineConfig` common subset). One struct
/// drives either backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineParams {
    pub modulation: Modulation,
    pub annex: Annex,
    pub bitswap: bool,
    pub sra: bool,
    pub profiles: Vdsl2Profiles,
}

/// Semantic ATM link parameters (the link "kind"; transport VLAN is
/// assigned at create time and lives in the link entry, not here).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtmParams {
    pub vpi: u8,
    pub vci: u16,
    pub encap: AtmEncap,
    pub link_type: AtmLinkType,
    pub qos: AtmQos,
    pub pcr: u32,
    pub scr: u32,
    pub mbs: u32,
}

impl Default for AtmParams {
    /// Common defaults: LLC / EoA bridged / UBR, VPI 8 / VCI 35.
    fn default() -> Self {
        Self {
            vpi: 8,
            vci: 35,
            encap: AtmEncap::Llc,
            link_type: AtmLinkType::Eoa,
            qos: AtmQos::Ubr,
            pcr: 0,
            scr: 0,
            mbs: 0,
        }
    }
}

/// Semantic PTM link parameters (VLAN tag handling; `tag_enable=false`
/// = untagged, matching the OEM op15 encoding).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PtmParams {
    pub tag_enable: bool,
    pub tag_vid: u16,
    pub tag_pri: u16,
}
