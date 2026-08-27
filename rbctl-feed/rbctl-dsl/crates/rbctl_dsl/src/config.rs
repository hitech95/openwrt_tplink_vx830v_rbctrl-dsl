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

use rbctl_proto::oem::pack::{
    Annex, AtmEncap, AtmLinkParams, AtmLinkType, AtmQos, Modulation, Vdsl2Profiles,
};

use crate::constants;

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
    /// Decide from the trained modulation (see [`Self::for_modulation`]).
    /// The op2 `data_path` field is NOT trustworthy for this: it is a
    /// power-on default that does not follow training (verified
    /// on-target — it read `Atm` while a VDSL2 line was in showtime).
    Auto,
}

impl XferMode {
    /// Stable wire token.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Atm => "Atm",
            Self::Ptm => "Ptm",
            Self::Auto => "Auto",
        }
    }

    /// Whether a link of this type can carry a line that TRAINED as
    /// `trained` — used for auto-correction of queued links (F4).
    /// Unresolvable combos (Auto) defer to the caller.
    pub fn compatible_with_trained(self, trained: XferMode) -> bool {
        match (self, trained) {
            (Self::Auto, _) | (_, Self::Auto) => true,
            (a, b) => a == b,
        }
    }

    /// TC layer implied by a trained modulation: VDSL2 → PTM, ADSL
    /// family → ATM. Used to resolve `Auto` (and the selftest) — the
    /// board does not derive the data-link type from the connection by
    /// itself, and the op2 `data_path` field does not follow training.
    pub fn for_modulation(m: Modulation) -> Self {
        match m {
            Modulation::Vdsl2 | Modulation::Multimode => Self::Ptm,
            _ => Self::Atm,
        }
    }

    /// Whether this transport can carry a line trained in `m`.
    ///
    /// This is a *physical* compatibility check, not a preference: an
    /// explicitly configured transport is honored as an operator
    /// override even when unusual (e.g. ATM on VDSL2 is spec-legal) —
    /// only combinations the standards cannot carry are rejected.
    ///
    /// - ADSL standards (G.992.x) are **ATM-only** — PTM is impossible.
    /// - VDSL2 (G.993.2) carries **both**; PTM is the norm, ATM is the
    ///   legal override.
    /// - Multimode resolves at train time to a concrete standard, so
    ///   both are accepted here and the final check runs against the
    ///   *trained* modulation.
    /// - Future modes slot in here, e.g. G.fast (G.9701) is PTM-only.
    pub fn compatible_with(self, m: Modulation) -> bool {
        match m {
            Modulation::Vdsl2 | Modulation::Multimode => true,
            // ADSL family: ANSI T1.413, G.dmt, G.lite, G.dmt.bis, ADSL2+,
            // ADSL-multimode.
            _ => self == Self::Atm,
        }
    }
}

/// Static config validation of the modulation × transport pair.
///
/// Blocks only what can never work even before the line trains: a
/// **fixed** ADSL line mode with forced PTM. `Multimode` is accepted
/// with any transport (the concrete standard appears at train time and
/// is then checked dynamically against the trained modulation); ATM on
/// VDSL2 is a spec-legal operator override and passes.
fn validate_xfer(modulation: Modulation, xfer: XferMode) -> Result<(), ConfigError> {
    if xfer == XferMode::Ptm
        && !matches!(
            modulation,
            Modulation::Vdsl2 | Modulation::Multimode
        )
    {
        return Err(ConfigError(format!(
            "xfer_mode 'ptm' is impossible for line_mode {modulation:?} \
             (ADSL standards are ATM-only)"
        )));
    }
    Ok(())
}

impl std::str::FromStr for XferMode {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "Atm" => Self::Atm,
            "Ptm" => Self::Ptm,
            "Auto" => Self::Auto,
            _ => return Err(()),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtmConfig {
    pub vpi: u8,
    pub vci: u16,
    pub encap: AtmEncap,
    pub link_type: AtmLinkType,
    pub qos: AtmQos,
    pub pcr: u32,
}

/// Build op5 wire params from the UCI ATM config (or defaults) plus the
/// daemon-computed transport VLAN id.
pub fn atm_params(atm: Option<&AtmConfig>, vlan_id: u16) -> AtmLinkParams<'static> {
    let a = atm.cloned().unwrap_or(AtmConfig {
        vpi: constants::ATM_DEFAULT_VPI,
        vci: constants::ATM_DEFAULT_VCI,
        encap: AtmEncap::Llc,
        link_type: AtmLinkType::Eoa,
        qos: AtmQos::Ubr,
        pcr: 0,
    });
    let mut p = AtmLinkParams::default();
    p.vpi = a.vpi;
    p.vci = a.vci;
    p.encap = a.encap;
    p.link_type = a.link_type;
    p.qos = a.qos;
    p.pcr = a.pcr;
    p.vlan_id = vlan_id;
    p
}

#[derive(Debug, Clone)]
pub struct DslConfig {
    pub modulation: Modulation,
    pub annex: Annex,
    pub profiles: Vdsl2Profiles,
    pub xfer_mode: XferMode,
    /// Present only when `xfer_mode == Atm` (legacy single-link ATM).
    pub atm: Option<AtmConfig>,
    /// Bitswap enable (opcode 1 byte 0x02, `X_TP_BitswapEnable`).
    pub bitswap: bool,
    /// SRA enable (opcode 1 byte 0x03, `X_TP_SRAEnable`).
    pub sra: bool,
    /// Transport VLAN base index (0–7). Actual VLAN id = base + 2000.
    pub transport_vlan_base: u8,
    /// Data links from named `network.<name>=ptm|atm` sections.
    /// Empty = one implicit default untagged PTM link at index 0
    /// (resolved by [`DslConfig::data_links`]).
    pub links: Vec<DataLink>,
}

/// One configured data link (UCI: `network.<name>=ptm` or `=atm`).
///
/// `index` is the stable data-link identity: transport VLAN =
/// `2000 + index` (board PVC slot). It is EXPLICIT in config so that
/// UCI section order changes can never re-point netifd interfaces
/// (pppoe/ipoe hook `lan0.<transport>` by name) at the wrong link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataLink {
    /// UCI section name (e.g. `internet`) — for diagnostics.
    pub name: String,
    /// Data-link index 0–7 → board transport VLAN `2000 + index`.
    pub index: u8,
    /// Whether `index` was explicit in config (validation: required on
    /// every section when more than one link exists).
    pub index_explicit: bool,
    /// Link type: PTM (op15) or ATM (op5).
    pub xfer: XferMode, // Ptm | Atm only
    /// Service VLAN tagging applied by the BOARD (op15/op5 tag fields):
    /// `None` = untagged (smux mode 1), `Some(vid, pbit)` = smux mode 3.
    /// PTM only.
    pub bearer_tag: Option<(u16, u8)>,
    /// ATM PVC parameters (ATM only).
    pub atm: Option<AtmConfig>,
}

impl DataLink {
    /// Transport (host↔board) VLAN id for this link.
    pub fn transport_vlan(&self) -> u16 {
        crate::constants::VLAN_TRANSPORT_BASE_OFFSET + self.index as u16
    }
}

// ── parsing helpers (host-testable, no UCI dependency) ───────────────────

/// Map UCI `line_mode` + `xfer_mode` → board [`Modulation`].
///
/// Case-insensitive. Aliases: `vdsl`/`vdsl2`; `adsl`/`adsl2`/`adsl2+`/
/// `adsl2plus`; `auto`/`multi`/`multimode`. When `line_mode` is absent,
/// `xfer_mode=ptm` implies VDSL2 (resolved by the caller's defaults).
pub fn parse_modulation(line_mode: &str, _xfer_mode: &str) -> Result<Modulation, ConfigError> {
    match line_mode.trim().to_ascii_lowercase().as_str() {
        "vdsl" | "vdsl2" | "" => Ok(Modulation::Vdsl2),
        "adsl" | "adsl2" | "adsl2+" | "adsl2plus" => Ok(Modulation::Adsl2Plus),
        "auto" | "multi" | "multimode" => Ok(Modulation::Multimode),
        other => Err(ConfigError(format!(
            "bad line_mode '{other}' (valid: vdsl|vdsl2, adsl|adsl2|adsl2plus, auto|multi|multimode)"
        ))),
    }
}

/// Map UCI `annex` letter → board [`Annex`]. Case-insensitive.
pub fn parse_annex(s: &str) -> Result<Annex, ConfigError> {
    match s.trim().to_ascii_lowercase().as_str() {
        "a" => Ok(Annex::A),
        "b" => Ok(Annex::B),
        "i" => Ok(Annex::I),
        "j" => Ok(Annex::J),
        "m" => Ok(Annex::M),
        "auto" => Ok(Annex::Auto),
        other => Err(ConfigError(format!(
            "bad annex '{other}' (valid: a, b, i, j, m, auto)"
        ))),
    }
}

/// Map UCI `tone` → [`Vdsl2Profiles`] bitmask. Case-insensitive.
///
/// Supported values:
/// - `"av"` / `"auto"` / `"all"` → all VDSL2 profiles
/// - Space- or comma-separated profile names: `"8a 17a 30a"`, `"17a,30a"`
/// - Single profile: `"35b"`
/// - `"a"` (ADSL tone group) → empty bitmask (no VDSL2 profiles)
pub fn parse_tone(s: &str) -> Result<Vdsl2Profiles, ConfigError> {
    let s = s.trim().to_ascii_lowercase();
    if s.is_empty() || s == "a" {
        return Ok(Vdsl2Profiles::default());
    }
    if s == "av" || s == "auto" || s == "all" {
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
    for tok in s.split(|c: char| c.is_whitespace() || c == ',') {
        let tok = tok.trim();
        if tok.is_empty() {
            continue;
        }
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
            other => {
                return Err(ConfigError(format!(
                    "bad tone profile '{other}' (valid: av|auto|all, or 8a 8b 8c 8d 12a 12b 17a 30a 35b)"
                )))
            }
        };
        profiles = profiles | p;
    }
    Ok(profiles)
}

/// Map UCI `xfer_mode` → [`XferMode`]. Case-insensitive.
pub fn parse_xfer_mode(s: &str) -> Result<XferMode, ConfigError> {
    match s.trim().to_ascii_lowercase().as_str() {
        "ptm" | "" => Ok(XferMode::Ptm),
        "atm" => Ok(XferMode::Atm),
        "auto" => Ok(XferMode::Auto),
        other => Err(ConfigError(format!(
            "bad xfer_mode '{other}' (valid: ptm, atm, auto)"
        ))),
    }
}

/// Map UCI `encaps` → [`AtmEncap`]. Case-insensitive.
pub fn parse_encaps(s: &str) -> Result<AtmEncap, ConfigError> {
    match s.trim().to_ascii_lowercase().as_str() {
        "llc" => Ok(AtmEncap::Llc),
        "vcmux" => Ok(AtmEncap::Vcmux),
        other => Err(ConfigError(format!(
            "bad encaps '{other}' (valid: llc, vcmux)"
        ))),
    }
}

/// Map UCI `payload` → [`AtmLinkType`]. Case-insensitive.
pub fn parse_payload(s: &str) -> Result<AtmLinkType, ConfigError> {
    match s.trim().to_ascii_lowercase().as_str() {
        "bridged" => Ok(AtmLinkType::Eoa),
        "routed" => Ok(AtmLinkType::Ipoa),
        "pppoa" => Ok(AtmLinkType::Pppoa),
        other => Err(ConfigError(format!(
            "bad payload '{other}' (valid: bridged, routed, pppoa)"
        ))),
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
    /// Single-link overrides (apply to the implicit default link only;
    /// an error if explicit data-link sections exist).
    pub link_index: Option<u8>,
    pub link_vid: Option<u16>,
    pub link_pbit: Option<u8>,
}

// ── UCI loader (target-only, links libuci) ───────────────────────────────

impl DslConfig {
    /// Load DSL config from `/etc/config/network`, applying CLI overrides.
    pub fn load(overrides: &CliOverrides) -> Result<Self, ConfigError> {
        let mut uci = rust_uci::Uci::new().map_err(|e| ConfigError(e.to_string()))?;
        Self::build(&mut uci, overrides)
    }

    /// Load from a specific UCI config dir (testing hook).
    #[allow(dead_code)]
    pub fn load_from_dir(config_dir: &str, overrides: &CliOverrides) -> Result<Self, ConfigError> {
        let mut uci = rust_uci::Uci::new().map_err(|e| ConfigError(e.to_string()))?;
        uci.set_config_dir(config_dir).map_err(|e| ConfigError(e.to_string()))?;
        Self::build(&mut uci, overrides)
    }

    /// Build config from UCI + CLI overrides.
    ///
    /// Priority: CLI override > UCI value > smart default (derived from line_mode).
    ///
    /// Smart defaults cascade from `line_mode`:
    /// - `vdsl` → xfer_mode=ptm, tone="av" (all VDSL2 profiles)
    /// - `adsl` → xfer_mode=atm, tone="" (no VDSL2 profiles)
    ///
    /// If `line_mode` is unspecified but `xfer_mode` is, line_mode is inferred:
    /// `xfer_mode=atm` → `adsl`, `xfer_mode=ptm` → `vdsl`.
    fn build(uci: &mut rust_uci::Uci, ov: &CliOverrides) -> Result<Self, ConfigError> {
        let uci_get = |uci: &mut rust_uci::Uci, key: &str| -> String {
            uci.get_opt(key).unwrap_or(None).unwrap_or_default()
        };
        let nonempty = |s: &Option<String>| -> Option<String> {
            s.clone().filter(|v| !v.is_empty())
        };

        // ── Resolve line_mode (the primary selector) ──
        let line_mode = nonempty(&ov.line_mode)
            .or_else(|| { let v = uci_get(uci, "network.dsl.line_mode"); (!v.is_empty()).then_some(v) })
            .or_else(|| {
                // Infer from explicit xfer_mode override/UCI
                let xfer = nonempty(&ov.xfer_mode)
                    .or_else(|| { let v = uci_get(uci, "network.dsl.xfer_mode"); (!v.is_empty()).then_some(v) })?;
                match xfer.trim().to_ascii_lowercase().as_str() {
                    "atm" => Some("adsl".into()),
                    "ptm" => Some("vdsl".into()),
                    _ => None,
                }
            })
            .unwrap_or_else(|| "vdsl".into());

        // ── Smart defaults derived from line_mode (case-insensitive) ──
        let (default_xfer, default_tone, default_annex) = match line_mode.trim().to_ascii_lowercase().as_str() {
            "adsl" | "adsl2" | "adsl2+" | "adsl2plus" => ("atm", "", "a"),
            "auto" | "multi" | "multimode" => ("auto", "av", "auto"),
            _ => ("ptm", "av", "b"),
        };

        // ── Resolve remaining fields (override > UCI > smart default) ──
        let annex_str = nonempty(&ov.annex)
            .or_else(|| { let v = uci_get(uci, "network.dsl.annex"); (!v.is_empty()).then_some(v) })
            .unwrap_or_else(|| default_annex.into());
        let xfer_str = nonempty(&ov.xfer_mode)
            .or_else(|| { let v = uci_get(uci, "network.dsl.xfer_mode"); (!v.is_empty()).then_some(v) })
            .unwrap_or_else(|| default_xfer.into());
        let tone = nonempty(&ov.tone)
            .or_else(|| { let v = uci_get(uci, "network.dsl.tone"); (!v.is_empty()).then_some(v) })
            .unwrap_or_else(|| default_tone.into());

        let modulation = parse_modulation(&line_mode, &xfer_str)?;
        let annex = parse_annex(&annex_str)?;
        let mut profiles = parse_tone(&tone)?;
        let xfer_mode = parse_xfer_mode(&xfer_str)?;
        validate_xfer(modulation, xfer_mode)?;

        // VDSL2 profiles are meaningless for ADSL modulations — zero them
        // (matching pack_dsl_line and the original firmware behavior).
        if !matches!(modulation, Modulation::Vdsl2 | Modulation::Multimode) {
            profiles = Vdsl2Profiles::default();
        }

        let atm = if matches!(xfer_mode, XferMode::Atm | XferMode::Auto) {
            let vpi_s = nonempty(&ov.vpi)
                .or_else(|| { let v = uci_get(uci, "network.@atm-bridge[0].vpi"); (!v.is_empty()).then_some(v) })
                .unwrap_or_else(|| "8".into());
            let vci_s = nonempty(&ov.vci)
                .or_else(|| { let v = uci_get(uci, "network.@atm-bridge[0].vci"); (!v.is_empty()).then_some(v) })
                .unwrap_or_else(|| "35".into());
            let encap = nonempty(&ov.encaps)
                .or_else(|| { let v = uci_get(uci, "network.@atm-bridge[0].encaps"); (!v.is_empty()).then_some(v) })
                .unwrap_or_else(|| "llc".into());
            let payload = nonempty(&ov.payload)
                .or_else(|| { let v = uci_get(uci, "network.@atm-bridge[0].payload"); (!v.is_empty()).then_some(v) })
                .unwrap_or_else(|| "bridged".into());
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
        // Auto mode defaults sra=false to match the board's own boot defaults.
        let default_sra = !matches!(
            line_mode.trim().to_ascii_lowercase().as_str(),
            "auto" | "multi" | "multimode"
        );
        let bitswap = ov.bitswap
            .unwrap_or_else(|| {
                uci_get(uci, "network.dsl.bitswap").parse::<u32>().map(|v| v != 0).unwrap_or(true)
            });
        let sra = ov.sra
            .unwrap_or_else(|| {
                uci_get(uci, "network.dsl.sra").parse::<u32>().map(|v| v != 0).unwrap_or(default_sra)
            });

        // Transport VLAN base index (0–7, default 0 → VLAN 2000)
        let transport_vlan_base = ov.transport_vlan
            .unwrap_or_else(|| {
                uci_get(uci, "network.dsl.transport_vlan").parse().unwrap_or(0u8)
            })
            .min(crate::constants::VLAN_TRANSPORT_BASE_MAX);

        // ── Data links: named network.<name>=ptm|atm sections ─────────
        let links = parse_data_links(uci, ov)?;

        Ok(Self {
            modulation,
            annex,
            profiles,
            xfer_mode,
            atm,
            bitswap,
            sra,
            transport_vlan_base,
            links,
        })
    }

    /// Effective data links: the configured sections, or the implicit
    /// default when none are — one untagged link at index 0 whose type
    /// follows the configured transport (PTM for VDSL/auto, ATM for the
    /// legacy fixed-ATM config, carrying the @atm-bridge PVC params).
    pub fn data_links(&self) -> Vec<DataLink> {
        if self.links.is_empty() {
            let (xfer, atm) = if self.xfer_mode == XferMode::Atm {
                (XferMode::Atm, self.atm.clone())
            } else {
                (XferMode::Ptm, None)
            };
            vec![DataLink {
                name: "default".into(),
                index: 0,
                index_explicit: true,
                xfer,
                bearer_tag: None,
                atm,
            }]
        } else {
            self.links.clone()
        }
    }
}

/// Parse + validate the data-link sections from UCI.
///
/// Schema (see plans/data-link-uci-plan.md F1):
/// - `network.<name>=ptm`  → `index` (0–7, optional), `vid` (optional,
///   service VLAN → board-side tag, absent = untagged), `pbit` (0–7),
/// - `network.<name>=atm`  → `index`, `vpi`, `vci`, `encaps`, `payload`,
/// - legacy anonymous `network.@atm-bridge[0]` still read for the
///   single-link case (no explicit sections present).
///
/// Validation (board limits, session-4 RE): ≤ 8 links, indices unique
/// 0–7, service vids unique (board vid-dedup), at most one untagged PTM
/// link, and — the stable-identity rule — an explicit `index` on every
/// section when more than one exists.
fn parse_data_links(
    uci: &mut rust_uci::Uci,
    ov: &CliOverrides,
) -> Result<Vec<DataLink>, ConfigError> {
    let uci_get = |uci: &mut rust_uci::Uci, key: &str| -> String {
        uci.get_opt(key).unwrap_or(None).unwrap_or_default()
    };
    let sections = uci.get_sections("network").unwrap_or_default();
    let mut links: Vec<DataLink> = Vec::new();

    for sec in &sections {
        let sec_type = uci_get(uci, &format!("network.{sec}"));
        let sec_type = sec_type.trim().to_ascii_lowercase();
        let (xfer, is_ptm) = match sec_type.as_str() {
            "ptm" => (XferMode::Ptm, true),
            "atm" => (XferMode::Atm, false),
            // IEEE-1588 confusion guard — common typo for ptm.
            "ptp" => {
                return Err(ConfigError(format!(
                    "network.{sec}: section type 'ptp' is not valid \
                     (did you mean 'ptm'? 'ptp' is IEEE-1588 time sync)"
                )))
            }
            _ => continue,
        };
        let name = sec.clone();

        let index: Option<u8> = uci_get(uci, &format!("network.{sec}.index"))
            .trim().parse().ok();
        if let Some(i) = index {
            if i > crate::constants::VLAN_TRANSPORT_BASE_MAX {
                return Err(ConfigError(format!(
                    "network.{sec}: index {i} out of range (0-7; \
                     transport VLAN is 2000+index)"
                )));
            }
        }

        let link = if is_ptm {
            let vid: Option<u16> = uci_get(uci, &format!("network.{sec}.vid"))
                .trim().parse().ok();
            if let Some(v) = vid {
                if !(1..=4094).contains(&v) {
                    return Err(ConfigError(format!(
                        "network.{sec}: vid {v} out of range (1-4094)"
                    )));
                }
            }
            let pbit: u8 = uci_get(uci, &format!("network.{sec}.pbit"))
                .trim().parse().unwrap_or(0);
            if pbit > 7 {
                return Err(ConfigError(format!(
                    "network.{sec}: pbit {pbit} out of range (0-7)"
                )));
            }
            DataLink {
                name,
                index: index.unwrap_or(0),
                index_explicit: index.is_some(),
                xfer,
                bearer_tag: vid.map(|v| (v, pbit)),
                atm: None,
            }
        } else {
            let vpi_s = uci_get(uci, &format!("network.{sec}.vpi"));
            let vci_s = uci_get(uci, &format!("network.{sec}.vci"));
            let encap = uci_get(uci, &format!("network.{sec}.encaps"));
            let payload = uci_get(uci, &format!("network.{sec}.payload"));
            let encap = if encap.is_empty() { "llc".into() } else { encap };
            let payload = if payload.is_empty() { "bridged".into() } else { payload };
            let vpi: u8 = vpi_s.trim().parse().unwrap_or(constants::ATM_DEFAULT_VPI);
            let vci: u16 = vci_s.trim().parse().unwrap_or(constants::ATM_DEFAULT_VCI);
            DataLink {
                name,
                index: index.unwrap_or(0),
                index_explicit: index.is_some(),
                xfer,
                bearer_tag: None,
                atm: Some(AtmConfig {
                    vpi,
                    vci,
                    encap: parse_encaps(&encap)?,
                    link_type: parse_payload(&payload)?,
                    qos: AtmQos::Ubr,
                    pcr: 0,
                }),
            }
        };
        links.push(link);
    }

    // CLI single-link overrides apply only when no UCI sections exist.
    if links.is_empty() {
        if ov.link_index.is_some() || ov.link_vid.is_some() || ov.link_pbit.is_some() {
            let vid = ov.link_vid;
            return Ok(vec![DataLink {
                name: "cli".into(),
                index: ov.link_index.unwrap_or(0),
                index_explicit: true,
                xfer: XferMode::Ptm,
                bearer_tag: vid.map(|v| (v, ov.link_pbit.unwrap_or(0))),
                atm: None,
            }]);
        }
        return Ok(Vec::new()); // implicit default resolved by data_links()
    }

    // ── validation ──
    if links.len() > 8 {
        return Err(ConfigError(format!(
            "{} data-link sections (max 8 — board PVC limit)",
            links.len()
        )));
    }
    // Stable identity: >1 link requires an explicit index on every one
    // (collected structurally during the section loop above).
    if links.len() > 1 {
        for link in &links {
            if !link.index_explicit {
                return Err(ConfigError(format!(
                    "network.{}: explicit 'index' required when multiple \
                     data-link sections exist (transport VLAN = 2000+index, \
                     stable across UCI reordering)",
                    link.name
                )));
            }
        }
    }
    // unique indices
    for (i, a) in links.iter().enumerate() {
        for b in links.iter().skip(i + 1) {
            if a.index == b.index {
                return Err(ConfigError(format!(
                    "network.{}: duplicate index {} (also network.{})",
                    b.name, b.index, a.name
                )));
            }
        }
    }
    // unique service vids + at most one untagged PTM link (board rules:
    // vdsl_linkAdd dedups on the service vid; untagged = vid -1 = one slot)
    let mut untagged = 0;
    for (i, a) in links.iter().enumerate() {
        if a.xfer == XferMode::Ptm && a.bearer_tag.is_none() {
            untagged += 1;
        }
        for b in links.iter().skip(i + 1) {
            let va = a.bearer_tag.map(|(v, _)| v);
            let vb = b.bearer_tag.map(|(v, _)| v);
            if va == vb {
                return Err(ConfigError(format!(
                    "network.{}: duplicate service vid {} (also network.{}) — \
                     the board dedups links on the service vid",
                    b.name,
                    va.map(|v| v.to_string()).unwrap_or_else(|| "untagged".into()),
                    a.name
                )));
            }
        }
    }
    if untagged > 1 {
        return Err(ConfigError(
            "at most one untagged ptm data link allowed".into(),
        ));
    }
    Ok(links)
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
            links: Vec::new(),
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
        assert_eq!(parse_xfer_mode("ptm").unwrap(), XferMode::Ptm);        assert_eq!(parse_xfer_mode("atm").unwrap(), XferMode::Atm);
        assert_eq!(parse_xfer_mode("auto").unwrap(), XferMode::Auto);
        assert_eq!(parse_xfer_mode("").unwrap(), XferMode::Ptm);
        assert!(parse_xfer_mode("xyz").is_err());
    }

    #[test]
    fn encaps_parsing() {
        assert_eq!(parse_encaps("llc").unwrap(), AtmEncap::Llc);
        assert_eq!(parse_encaps("vcmux").unwrap(), AtmEncap::Vcmux);
    }

    #[test]
    fn xfer_mode_follows_modulation() {
        use XferMode::*;
        assert_eq!(XferMode::for_modulation(Modulation::Vdsl2), Ptm);
        assert_eq!(XferMode::for_modulation(Modulation::Multimode), Ptm);
        assert_eq!(XferMode::for_modulation(Modulation::Adsl2Plus), Atm);
        assert_eq!(XferMode::for_modulation(Modulation::AdslGdmt), Atm);
        assert_eq!(XferMode::for_modulation(Modulation::AdslMultimode), Atm);
    }

    #[test]
    fn xfer_compatibility_rules() {
        use XferMode::*;
        // ADSL standards are ATM-only: PTM is physically impossible.
        for m in [
            Modulation::AdslAnsiT1413, Modulation::AdslGdmt, Modulation::AdslGlite,
            Modulation::AdslGdmtBis, Modulation::Adsl2Plus, Modulation::AdslMultimode,
        ] {
            assert!(Atm.compatible_with(m), "ATM on {m:?}");
            assert!(!Ptm.compatible_with(m), "PTM on {m:?} must be blocked");
        }
        // VDSL2 carries both: PTM the norm, ATM a spec-legal override.
        assert!(Ptm.compatible_with(Modulation::Vdsl2));
        assert!(Atm.compatible_with(Modulation::Vdsl2));
        // Multimode resolves at train time — both accepted statically.
        assert!(Ptm.compatible_with(Modulation::Multimode));
        assert!(Atm.compatible_with(Modulation::Multimode));
    }

    #[test]
    fn data_link_transport_vlan() {
        let l = DataLink {
            name: "internet".into(), index: 3, index_explicit: true,
            xfer: XferMode::Ptm, bearer_tag: Some((100, 0)), atm: None,
        };
        assert_eq!(l.transport_vlan(), 2003);
        assert_eq!(
            DataLink { index: 0, ..l }.transport_vlan(),
            crate::constants::VLAN_TRANSPORT_BASE_OFFSET
        );
    }

    #[test]
    fn default_data_links_is_untagged_ptm_index0() {
        let c = DslConfig::default();
        let links = c.data_links();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].index, 0);
        assert_eq!(links[0].xfer, XferMode::Ptm);
        assert_eq!(links[0].bearer_tag, None);
        assert_eq!(links[0].transport_vlan(), 2000);
    }

    #[test]
    fn static_config_validation() {
        // Fixed ADSL + forced PTM is rejected at load.
        let e = validate_xfer(Modulation::Adsl2Plus, XferMode::Ptm).unwrap_err().0;
        assert!(e.contains("ATM-only"), "got: {e}");
        // Legal combos pass, including the unusual VDSL2+ATM override.
        assert!(validate_xfer(Modulation::Adsl2Plus, XferMode::Atm).is_ok());
        assert!(validate_xfer(Modulation::Vdsl2, XferMode::Ptm).is_ok());
        assert!(validate_xfer(Modulation::Vdsl2, XferMode::Atm).is_ok());
        assert!(validate_xfer(Modulation::Multimode, XferMode::Ptm).is_ok());
        assert!(validate_xfer(Modulation::Multimode, XferMode::Auto).is_ok());
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
    fn modulation_auto() {
        assert_eq!(parse_modulation("auto", "auto").unwrap(), Modulation::Multimode);
        assert_eq!(parse_modulation("multi", "auto").unwrap(), Modulation::Multimode);
    }

    // ── case-insensitivity / aliases (session-2: 'Vdsl2' and
    //    --tone=auto silently voided the tester's whole matrix) ──

    #[test]
    fn modulation_case_insensitive_and_aliases() {
        assert_eq!(parse_modulation("Vdsl2", "ptm").unwrap(), Modulation::Vdsl2);
        assert_eq!(parse_modulation("VDSL", "ptm").unwrap(), Modulation::Vdsl2);
        assert_eq!(parse_modulation("ADSL2Plus", "atm").unwrap(), Modulation::Adsl2Plus);
        assert_eq!(parse_modulation("adsl2+", "atm").unwrap(), Modulation::Adsl2Plus);
        assert_eq!(parse_modulation("Multimode", "auto").unwrap(), Modulation::Multimode);
        assert_eq!(parse_modulation("AUTO", "auto").unwrap(), Modulation::Multimode);
        assert!(parse_modulation("Vdsl3", "ptm").is_err());
    }

    #[test]
    fn tone_case_insensitive_and_aliases() {
        assert_eq!(parse_tone("AUTO").unwrap().bitmask(), 0x1ff);
        assert_eq!(parse_tone("All").unwrap().bitmask(), 0x1ff);
        assert_eq!(parse_tone("17A").unwrap(), Vdsl2Profiles::SEVENTEEN_A);
        // comma-separated lists
        assert_eq!(
            parse_tone("17a,30a").unwrap(),
            Vdsl2Profiles::SEVENTEEN_A | Vdsl2Profiles::THIRTY_A
        );
        assert_eq!(
            parse_tone(" 8a , 35b ").unwrap(),
            Vdsl2Profiles::EIGHT_A | Vdsl2Profiles::THIRTYFIVE_B
        );
    }

    #[test]
    fn annex_and_xfer_case_insensitive() {
        assert_eq!(parse_annex("B").unwrap(), Annex::B);
        assert_eq!(parse_annex("Auto").unwrap(), Annex::Auto);
        assert_eq!(parse_xfer_mode("PTM").unwrap(), XferMode::Ptm);
        assert_eq!(parse_xfer_mode("Atm").unwrap(), XferMode::Atm);
        assert_eq!(parse_encaps("LLC").unwrap(), AtmEncap::Llc);
        assert_eq!(parse_payload("Bridged").unwrap(), AtmLinkType::Eoa);
    }

    #[test]
    fn parse_errors_name_the_option() {
        let e = parse_modulation("g.fast", "ptm").unwrap_err().0;
        assert!(e.contains("line_mode"), "got: {e}");
        let e = parse_tone("17x").unwrap_err().0;
        assert!(e.contains("tone"), "got: {e}");
        let e = parse_annex("q").unwrap_err().0;
        assert!(e.contains("annex"), "got: {e}");
    }

    #[test]
    fn annex_auto() {
        assert_eq!(parse_annex("auto").unwrap(), Annex::Auto);
        assert_eq!(parse_annex("AUTO").unwrap(), Annex::Auto);
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
