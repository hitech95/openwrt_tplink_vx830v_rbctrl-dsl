//! UCI config loader — reads `/etc/config/network` and maps the standard
//! OpenWrt DSL schema (`dsl` + `atm-bridge` sections) to typed board config.
//!
//! Reuses existing UCI options per [openwrt.md](../../docs/openwrt.md) §2.4.
//! No vendor-specific extensions are needed — the standard schema covers
//! everything the daemon uses.
//!
//! ## Parsing helpers
//!
//! The `parse_*` functions are pure and host-testable. The `DslConfig::load`
//! method wraps them with UCI access (requires libuci, target-only).

use rbctl_proto::pack::{Annex, AtmEncap, AtmLinkType, AtmQos, Modulation, Vdsl2Profiles};

// ── error type ───────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct ConfigError(pub String);

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for ConfigError {}

// ── config types ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XferMode {
    Atm,
    Ptm,
}

#[derive(Debug, Clone)]
pub struct AtmConfig {
    pub vpi: u8,
    pub vci: u16,
    pub encap: AtmEncap,
    pub link_type: AtmLinkType,
    pub qos: AtmQos,
    pub pcr: u32,
}

#[derive(Debug, Clone)]
pub struct DslConfig {
    pub modulation: Modulation,
    pub annex: Annex,
    pub profiles: Vdsl2Profiles,
    pub xfer_mode: XferMode,
    /// Present only when `xfer_mode == Atm`.
    pub atm: Option<AtmConfig>,
    /// Bitswap enable (opcode 1 byte 0x02, `X_TP_BitswapEnable`).
    pub bitswap: bool,
    /// SRA enable (opcode 1 byte 0x03, `X_TP_SRAEnable`).
    pub sra: bool,
    /// Transport VLAN base index (0–7). Actual VLAN id = base + 2000.
    pub transport_vlan_base: u8,
}

// ── parsing helpers (host-testable, no UCI dependency) ───────────────────

/// Map UCI `line_mode` + `xfer_mode` → board [`Modulation`].
///
/// `line_mode=vdsl` → `Vdsl2`; `line_mode=adsl` → `Adsl2Plus`.
/// When `line_mode` is absent, `xfer_mode=ptm` implies VDSL2.
pub fn parse_modulation(line_mode: &str, _xfer_mode: &str) -> Result<Modulation, ConfigError> {
    match line_mode {
        "vdsl" | "" => Ok(Modulation::Vdsl2),
        "adsl" => Ok(Modulation::Adsl2Plus),
        other => Err(ConfigError(format!("bad line_mode '{other}'"))),
    }
}

/// Map UCI `annex` letter → board [`Annex`].
pub fn parse_annex(s: &str) -> Result<Annex, ConfigError> {
    match s.trim() {
        "a" | "A" => Ok(Annex::A),
        "b" | "B" => Ok(Annex::B),
        "i" | "I" => Ok(Annex::I),
        "j" | "J" => Ok(Annex::J),
        "m" | "M" => Ok(Annex::M),
        other => Err(ConfigError(format!("bad annex '{other}'"))),
    }
}

/// Map UCI `tone` → [`Vdsl2Profiles`] bitmask.
///
/// Supported values:
/// - `"av"` → all VDSL2 profiles
/// - Space-separated profile names: `"8a 17a 30a"`
/// - Single profile: `"35b"`
/// - `"a"` (ADSL tone group) → empty bitmask (no VDSL2 profiles)
pub fn parse_tone(s: &str) -> Result<Vdsl2Profiles, ConfigError> {
    let s = s.trim();
    if s.is_empty() || s == "a" {
        return Ok(Vdsl2Profiles::default());
    }
    if s == "av" {
        return Ok(Vdsl2Profiles::EIGHT_A
            | Vdsl2Profiles::EIGHT_B
            | Vdsl2Profiles::EIGHT_C
            | Vdsl2Profiles::EIGHT_D
            | Vdsl2Profiles::TWELVE_A
            | Vdsl2Profiles::TWELVE_B
            | Vdsl2Profiles::SEVENTEEN_A
            | Vdsl2Profiles::THIRTY_A
            | Vdsl2Profiles::THIRTYFIVE_B);
    }
    let mut profiles = Vdsl2Profiles::default();
    for tok in s.split_whitespace() {
        let p = match tok {
            "8a" => Vdsl2Profiles::EIGHT_A,
            "8b" => Vdsl2Profiles::EIGHT_B,
            "8c" => Vdsl2Profiles::EIGHT_C,
            "8d" => Vdsl2Profiles::EIGHT_D,
            "12a" => Vdsl2Profiles::TWELVE_A,
            "12b" => Vdsl2Profiles::TWELVE_B,
            "17a" => Vdsl2Profiles::SEVENTEEN_A,
            "30a" => Vdsl2Profiles::THIRTY_A,
            "35b" => Vdsl2Profiles::THIRTYFIVE_B,
            other => return Err(ConfigError(format!("bad tone profile '{other}'"))),
        };
        profiles = profiles | p;
    }
    Ok(profiles)
}

/// Map UCI `xfer_mode` → [`XferMode`].
pub fn parse_xfer_mode(s: &str) -> Result<XferMode, ConfigError> {
    match s.trim() {
        "ptm" | "" => Ok(XferMode::Ptm),
        "atm" => Ok(XferMode::Atm),
        other => Err(ConfigError(format!("bad xfer_mode '{other}'"))),
    }
}

/// Map UCI `encaps` → [`AtmEncap`].
pub fn parse_encaps(s: &str) -> Result<AtmEncap, ConfigError> {
    match s.trim() {
        "llc" => Ok(AtmEncap::Llc),
        "vcmux" => Ok(AtmEncap::Vcmux),
        other => Err(ConfigError(format!("bad encaps '{other}'"))),
    }
}

/// Map UCI `payload` → [`AtmLinkType`].
pub fn parse_payload(s: &str) -> Result<AtmLinkType, ConfigError> {
    match s.trim() {
        "bridged" => Ok(AtmLinkType::Eoa),
        "routed" => Ok(AtmLinkType::Ipoa),
        "pppoa" => Ok(AtmLinkType::Pppoa),
        other => Err(ConfigError(format!("bad payload '{other}'"))),
    }
}

// ── CLI overrides ────────────────────────────────────────────────────────

/// Optional CLI parameter overrides. Each field takes priority over UCI
/// when present; UCI takes priority over built-in defaults.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct CliOverrides {
    pub annex: Option<String>,
    pub line_mode: Option<String>,
    pub tone: Option<String>,
    pub xfer_mode: Option<String>,
    pub vpi: Option<String>,
    pub vci: Option<String>,
    pub encaps: Option<String>,
    pub payload: Option<String>,
    pub bitswap: Option<bool>,
    pub sra: Option<bool>,
    pub transport_vlan: Option<u8>,
}

// ── UCI loader (target-only, links libuci) ───────────────────────────────

impl DslConfig {
    /// Load DSL config from `/etc/config/network`, applying CLI overrides.
    pub fn load(overrides: &CliOverrides) -> Result<Self, ConfigError> {
        let mut uci = rust_uci::Uci::new().map_err(|e| ConfigError(e.to_string()))?;
        Self::build(&mut uci, overrides)
    }

    /// Load from a specific UCI config dir (for testing).
    pub fn load_from_dir(config_dir: &str, overrides: &CliOverrides) -> Result<Self, ConfigError> {
        let mut uci = rust_uci::Uci::new().map_err(|e| ConfigError(e.to_string()))?;
        uci.set_config_dir(config_dir).map_err(|e| ConfigError(e.to_string()))?;
        Self::build(&mut uci, overrides)
    }

    /// Build config from UCI + CLI overrides.
    ///
    /// Priority: CLI override > UCI value > built-in default.
    fn build(uci: &mut rust_uci::Uci, ov: &CliOverrides) -> Result<Self, ConfigError> {
        let uci_get = |uci: &mut rust_uci::Uci, key: &str| -> String {
            uci.get_opt(key).unwrap_or(None).unwrap_or_default()
        };
        let pick = |ov_val: &Option<String>, uci_val: String, default: &str| -> String {
            ov_val.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| {
                if uci_val.is_empty() { default.into() } else { uci_val }
            })
        };

        let annex_str   = pick(&ov.annex,     uci_get(uci, "network.dsl.annex"),     "b");
        let line_mode   = pick(&ov.line_mode, uci_get(uci, "network.dsl.line_mode"), "vdsl");
        let tone        = pick(&ov.tone,      uci_get(uci, "network.dsl.tone"),      "av");
        let xfer_str    = pick(&ov.xfer_mode, uci_get(uci, "network.dsl.xfer_mode"), "ptm");

        let modulation = parse_modulation(&line_mode, &xfer_str)?;
        let annex = parse_annex(&annex_str)?;
        let profiles = parse_tone(&tone)?;
        let xfer_mode = parse_xfer_mode(&xfer_str)?;

        let atm = if xfer_mode == XferMode::Atm {
            let vpi_s  = pick(&ov.vpi,  uci_get(uci, "network.@atm-bridge[0].vpi"),  "8");
            let vci_s  = pick(&ov.vci,  uci_get(uci, "network.@atm-bridge[0].vci"),  "35");
            let encap  = pick(&ov.encaps, uci_get(uci, "network.@atm-bridge[0].encaps"), "llc");
            let payload = pick(&ov.payload, uci_get(uci, "network.@atm-bridge[0].payload"), "bridged");
            Some(AtmConfig {
                vpi: vpi_s.parse().unwrap_or(8),
                vci: vci_s.parse().unwrap_or(35),
                encap: parse_encaps(&encap)?,
                link_type: parse_payload(&payload)?,
                qos: AtmQos::Ubr,
                pcr: 0,
            })
        } else {
            None
        };

        // Bitswap / SRA (opcode 1 bytes 0x02/0x03)
        let bitswap = ov.bitswap
            .unwrap_or_else(|| {
                uci_get(uci, "network.dsl.bitswap").parse::<u32>().map(|v| v != 0).unwrap_or(true)
            });
        let sra = ov.sra
            .unwrap_or_else(|| {
                uci_get(uci, "network.dsl.sra").parse::<u32>().map(|v| v != 0).unwrap_or(true)
            });

        // Transport VLAN base index (0–7, default 0 → VLAN 2000)
        let transport_vlan_base = ov.transport_vlan
            .unwrap_or_else(|| {
                uci_get(uci, "network.dsl.transport_vlan").parse().unwrap_or(0u8)
            })
            .min(7);

        Ok(Self {
            modulation,
            annex,
            profiles,
            xfer_mode,
            atm,
            bitswap,
            sra,
            transport_vlan_base,
        })
    }
}

impl Default for DslConfig {
    /// Sensible defaults: VDSL2 Annex B, all profiles, PTM, bitswap+sra on.
    fn default() -> Self {
        Self {
            modulation: Modulation::Vdsl2,
            annex: Annex::B,
            profiles: parse_tone("av").unwrap(),
            xfer_mode: XferMode::Ptm,
            atm: None,
            bitswap: true,
            sra: true,
            transport_vlan_base: 0,
        }
    }
}

// ── tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annex_parsing() {
        assert_eq!(parse_annex("a").unwrap(), Annex::A);
        assert_eq!(parse_annex("B").unwrap(), Annex::B);
        assert_eq!(parse_annex("j").unwrap(), Annex::J);
        assert!(parse_annex("x").is_err());
    }

    #[test]
    fn tone_av_all_profiles() {
        let p = parse_tone("av").unwrap();
        assert_eq!(p, Vdsl2Profiles::EIGHT_A | Vdsl2Profiles::THIRTY_A);
        assert_eq!(p.bitmask(), 0x1ff);
    }

    #[test]
    fn tone_individual_profiles() {
        let p = parse_tone("8a 17a 30a").unwrap();
        assert_eq!(p, Vdsl2Profiles::EIGHT_A | Vdsl2Profiles::SEVENTEEN_A | Vdsl2Profiles::THIRTY_A);
        assert_eq!(p.bitmask(), 0x0c1);
    }

    #[test]
    fn tone_single() {
        assert_eq!(parse_tone("35b").unwrap(), Vdsl2Profiles::THIRTYFIVE_B);
    }

    #[test]
    fn tone_adsl_empty() {
        assert_eq!(parse_tone("a").unwrap().bitmask(), 0);
        assert_eq!(parse_tone("").unwrap().bitmask(), 0);
    }

    #[test]
    fn tone_bad_profile() {
        assert!(parse_tone("99z").is_err());
    }

    #[test]
    fn xfer_mode_parsing() {
        assert_eq!(parse_xfer_mode("ptm").unwrap(), XferMode::Ptm);
        assert_eq!(parse_xfer_mode("atm").unwrap(), XferMode::Atm);
        assert_eq!(parse_xfer_mode("").unwrap(), XferMode::Ptm);
        assert!(parse_xfer_mode("xyz").is_err());
    }

    #[test]
    fn encaps_parsing() {
        assert_eq!(parse_encaps("llc").unwrap(), AtmEncap::Llc);
        assert_eq!(parse_encaps("vcmux").unwrap(), AtmEncap::Vcmux);
    }

    #[test]
    fn payload_parsing() {
        assert_eq!(parse_payload("bridged").unwrap(), AtmLinkType::Eoa);
        assert_eq!(parse_payload("routed").unwrap(), AtmLinkType::Ipoa);
        assert_eq!(parse_payload("pppoa").unwrap(), AtmLinkType::Pppoa);
    }

    #[test]
    fn modulation_vdsl() {
        assert_eq!(parse_modulation("vdsl", "ptm").unwrap(), Modulation::Vdsl2);
        assert_eq!(parse_modulation("", "ptm").unwrap(), Modulation::Vdsl2);
    }

    #[test]
    fn modulation_adsl() {
        assert_eq!(parse_modulation("adsl", "atm").unwrap(), Modulation::Adsl2Plus);
    }

    #[test]
    fn default_config() {
        let cfg = DslConfig::default();
        assert_eq!(cfg.modulation, Modulation::Vdsl2);
        assert_eq!(cfg.annex, Annex::B);
        assert_eq!(cfg.xfer_mode, XferMode::Ptm);
        assert_eq!(cfg.profiles.bitmask(), 0x1ff);
        assert!(cfg.atm.is_none());
    }
}
