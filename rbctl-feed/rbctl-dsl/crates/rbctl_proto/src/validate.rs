//! Config validation guard — rejects inconsistent `(modulation, annex,
//! profile)` triples before TX, hardening beyond the original firmware
//! (which silently serializes whatever it's given).
//!
//! Rules sourced from [modulation_annex.md](../../docs/xdsl/modulation_annex.md).

use crate::pack::{Annex, Modulation, Vdsl2Profiles};

// ── error type ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineConfigError {
    /// Non-zero VDSL2 profile bitmask with an ADSL modulation code.
    ProfileRequiresVdsl2 { modulation: u8, profile_bitmask: u32 },
    /// Annex specified but the modulation standard has no annex concept
    /// (T1.413, G.lite — `valid_annexes` is NULL).
    AnnexesNotDefined { modulation: u8 },
    /// An annex letter is not in the modulation's `valid_annexes` set.
    AnnexNotInStandard { annex: u8, modulation: u8, letter: char, valid: &'static str },
    /// VDSL2 modulation (6) requires PTM transport; ADSL codes (0–5) require ATM.
    XferModeMismatch { modulation: u8, expected: TransportHint, actual: TransportHint },
}

/// Transport mode hint for xfer_mode validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportHint {
    Atm,
    Ptm,
}

impl core::fmt::Display for LineConfigError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ProfileRequiresVdsl2 { modulation, profile_bitmask } => write!(
                f, "VDSL2 profile bitmask 0x{profile_bitmask:x} requires modulation 6 (VDSL2) or 7 (Multimode), got {modulation}"
            ),
            Self::AnnexesNotDefined { modulation } => write!(
                f, "modulation {modulation} has no annex concept (T1.413/G.lite); only Annex auto is valid"
            ),
            Self::AnnexNotInStandard { annex, modulation, letter, valid } => write!(
                f, "annex {annex} letter '{letter}' not in modulation {modulation}'s valid set '{valid}'"
            ),
            Self::XferModeMismatch { modulation, expected, actual } => write!(
                f, "modulation {modulation} requires {expected:?} transport, got {actual:?}"
            ),
        }
    }
}

// ── lookup tables (from modulation_annex.md) ─────────────────────────────

/// `valid_annexes` per modulation code, from `modulationTypes` @ `0x3edc20`.
///
/// Returns `None` for T1.413 (0) and G.lite (2) — these have `NULL`
/// (no annex concept). Otherwise returns the letter string, e.g. `"ABCIJLM"`.
pub fn valid_annexes(modulation: u8) -> Option<&'static str> {
    match modulation {
    // 0=T1.413, 2=G.lite: NULL → no annex concept
       0 | 2 => None,
    // 1=G.992.1
       1 => Some("ABC"),
    // 3=G.992.3/4
       3 => Some("ABCIJM"),
    // 4=ADSL2+, 5=ADSL Multimode, 6=VDSL2, 7=Multimode
       4 | 5 | 6 | 7 => Some("ABCIJLM"),
       _ => None,
    }
}

/// ITU-T annex letters for each annex code, from `annexTypes` @ `0x3edd20`.
pub fn annex_letters(annex: u8) -> &'static [char] {
    match annex {
        0 => &['A'],           // Annex A
        1 => &['B'],           // Annex B
        2 => &['I'],           // Annex I
        3 => &['M'],           // Annex M
        4 => &['A', 'L'],      // Annex A/L
        5 => &['A', 'L', 'M'], // Annex A/L/M
        6 => &['J'],           // Annex J
        7 => &['B', 'J'],      // Annex B/J
        8 => &[],              // Annex auto — always valid, no letters to check
        _ => &[],
    }
}

/// Whether a modulation code is ADSL (codes 0–5).
pub fn is_adsl(modulation: u8) -> bool {
    matches!(modulation, 0 | 1 | 2 | 3 | 4 | 5)
}

/// Whether a modulation code is VDSL2 (code 6).
pub fn is_vdsl2(modulation: u8) -> bool {
    modulation == 6
}

// ── validation ───────────────────────────────────────────────────────────

/// Validate the `(modulation, annex, profile)` triple.
///
/// Checks:
/// 1. **profile ↔ modulation**: VDSL2 profiles only valid for VDSL2 (6) / Multimode (7).
/// 2. **annex ↔ modulation**: annex letters must be in the modulation's `valid_annexes`.
pub fn validate_line_config(
    modulation: Modulation,
    annex: Annex,
    profiles: Vdsl2Profiles,
) -> Result<(), LineConfigError> {
    let mod_code = modulation as u8;
    let annex_code = annex as u8;
    let bitmask = profiles.bitmask();

    // Rule 1: profile ↔ modulation
    if bitmask != 0 && !matches!(mod_code, 6 | 7) {
        return Err(LineConfigError::ProfileRequiresVdsl2 {
            modulation: mod_code,
            profile_bitmask: bitmask,
        });
    }

    // Rule 2: annex ↔ modulation
    if annex_code != 8 {
        // Annex auto (8) always passes
        let valid = valid_annexes(mod_code).ok_or(LineConfigError::AnnexesNotDefined {
            modulation: mod_code,
        })?;
        for &letter in annex_letters(annex_code) {
            if !valid.contains(letter) {
                return Err(LineConfigError::AnnexNotInStandard {
                    annex: annex_code,
                    modulation: mod_code,
                    letter,
                    valid,
                });
            }
        }
    }

    Ok(())
}

/// Validate the `(modulation, transport)` consistency.
///
/// Rule 3: VDSL2 (6) requires PTM; ADSL codes (0–5) require ATM.
/// Multimode (7) accepts either.
pub fn validate_xfer_mode(modulation: Modulation, transport: TransportHint) -> Result<(), LineConfigError> {
    let mod_code = modulation as u8;
    if is_vdsl2(mod_code) && transport != TransportHint::Ptm {
        return Err(LineConfigError::XferModeMismatch {
            modulation: mod_code,
            expected: TransportHint::Ptm,
            actual: transport,
        });
    }
    if is_adsl(mod_code) && transport != TransportHint::Atm {
        return Err(LineConfigError::XferModeMismatch {
            modulation: mod_code,
            expected: TransportHint::Atm,
            actual: transport,
        });
    }
    Ok(())
}

// ── tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // Rule 1: profile ↔ modulation

    #[test]
    fn profile_with_adsl_rejected() {
        let err = validate_line_config(
            Modulation::Adsl2Plus, Annex::B, Vdsl2Profiles::SEVENTEEN_A,
        );
        assert!(matches!(err, Err(LineConfigError::ProfileRequiresVdsl2 { .. })));
    }

    #[test]
    fn profile_zero_with_adsl_ok() {
        assert!(validate_line_config(
            Modulation::Adsl2Plus, Annex::B, Vdsl2Profiles::default(),
        ).is_ok());
    }

    #[test]
    fn profile_with_vdsl2_ok() {
        assert!(validate_line_config(
            Modulation::Vdsl2, Annex::B, Vdsl2Profiles::SEVENTEEN_A | Vdsl2Profiles::THIRTY_A,
        ).is_ok());
    }

    #[test]
    fn profile_with_multimode_ok() {
        assert!(validate_line_config(
            Modulation::Multimode, Annex::A, Vdsl2Profiles::THIRTY_A,
        ).is_ok());
    }

    // Rule 2: annex ↔ modulation

    #[test]
    fn annex_m_with_g9921_rejected() {
        // G.992.1 valid = ABC; M not included
        let err = validate_line_config(
            Modulation::AdslGdmt, Annex::M, Vdsl2Profiles::default(),
        );
        assert!(matches!(err, Err(LineConfigError::AnnexNotInStandard { letter: 'M', .. })));
    }

    #[test]
    fn annex_i_with_g9921_rejected() {
        // G.992.1 valid = ABC; I not included
        let err = validate_line_config(
            Modulation::AdslGdmt, Annex::I, Vdsl2Profiles::default(),
        );
        assert!(matches!(err, Err(LineConfigError::AnnexNotInStandard { letter: 'I', .. })));
    }

    #[test]
    fn any_annex_with_t1413_rejected() {
        // T1.413 has no annex concept
        let err = validate_line_config(
            Modulation::AdslAnsiT1413, Annex::A, Vdsl2Profiles::default(),
        );
        assert!(matches!(err, Err(LineConfigError::AnnexesNotDefined { .. })));
    }

    #[test]
    fn any_annex_with_glite_rejected() {
        let err = validate_line_config(
            Modulation::AdslGlite, Annex::A, Vdsl2Profiles::default(),
        );
        assert!(matches!(err, Err(LineConfigError::AnnexesNotDefined { .. })));
    }

    #[test]
    fn auto_annex_with_t1413_ok() {
        assert!(validate_line_config(
            Modulation::AdslAnsiT1413, Annex::Auto, Vdsl2Profiles::default(),
        ).is_ok());
    }

    #[test]
    fn annex_a_with_g9923_ok() {
        assert!(validate_line_config(
            Modulation::AdslGdmtBis, Annex::A, Vdsl2Profiles::default(),
        ).is_ok());
    }

    #[test]
    fn annex_al_with_adsl2plus_ok() {
        // ADSL2+ valid = ABCIJLM; A and L both in set
        assert!(validate_line_config(
            Modulation::Adsl2Plus, Annex::AL, Vdsl2Profiles::default(),
        ).is_ok());
    }

    #[test]
    fn annex_al_with_g9921_partial_rejected() {
        // G.992.1 valid = ABC; A ok but L not in set → rejected
        let err = validate_line_config(
            Modulation::AdslGdmt, Annex::AL, Vdsl2Profiles::default(),
        );
        assert!(matches!(err, Err(LineConfigError::AnnexNotInStandard { letter: 'L', .. })));
    }

    #[test]
    fn all_single_annex_with_vdsl2_ok() {
        for annex in [Annex::A, Annex::B, Annex::I, Annex::M, Annex::J] {
            assert!(validate_line_config(
                Modulation::Vdsl2, annex, Vdsl2Profiles::default(),
            ).is_ok(), "VDSL2 + {:?} should be valid", annex);
        }
    }

    #[test]
    fn auto_annex_with_every_modulation_ok() {
        for mod_code in [
            Modulation::AdslAnsiT1413, Modulation::AdslGdmt, Modulation::AdslGlite,
            Modulation::AdslGdmtBis, Modulation::Adsl2Plus, Modulation::AdslMultimode,
            Modulation::Vdsl2, Modulation::Multimode,
        ] {
            assert!(validate_line_config(
                mod_code, Annex::Auto, Vdsl2Profiles::default(),
            ).is_ok(), "{:?} + Auto should be valid", mod_code);
        }
    }

    // Rule 3: xfer_mode ↔ modulation

    #[test]
    fn vdsl2_requires_ptm() {
        assert!(validate_xfer_mode(Modulation::Vdsl2, TransportHint::Ptm).is_ok());
        assert!(validate_xfer_mode(Modulation::Vdsl2, TransportHint::Atm).is_err());
    }

    #[test]
    fn adsl_requires_atm() {
        assert!(validate_xfer_mode(Modulation::Adsl2Plus, TransportHint::Atm).is_ok());
        assert!(validate_xfer_mode(Modulation::Adsl2Plus, TransportHint::Ptm).is_err());
    }

    #[test]
    fn multimode_accepts_either() {
        assert!(validate_xfer_mode(Modulation::Multimode, TransportHint::Ptm).is_ok());
        assert!(validate_xfer_mode(Modulation::Multimode, TransportHint::Atm).is_ok());
    }
}
