//! Typed message builders — the protobuf analog of [`crate::oem::pack`].
//!
//! Native (model) types in, `pb::Request` out, so the daemon can drive
//! the protobuf backend with the same values it feeds to
//! `oem::pack::pack_dsl_line`. The inverse lives in [`super::unpack`].
//!
//! Enum mappings worth noting (vocabularies differ slightly):
//!
//! | native ([`model`](crate::model)) | protobuf | note |
//! |---|---|---|
//! | `Modulation::AdslGdmtBis` (3) | `ADSL2` | G.992.3 |
//! | `Modulation::AdslMultimode` (5) | `MULTIMODE` | pb folds the ADSL-only multimode into the general one |
//! | — | (none) | every firmware annex code is representable |
//! | `DataPath::Unknown(_)` | `UNSPECIFIED` | lossy by design |

use super::framing;
use super::rbctl::{self as pb};
use crate::model::{
    Annex, AtmEncap, AtmLinkType, AtmQos, DataPath, Modulation, Vdsl2Profiles,
};

// Conversions operate on the shared model types; re-exported here so
// `protobuf::pack` is the one-stop facade for pb mapping.
pub use crate::model::{AtmParams, LineParams, PtmParams};

// ─── enum conversions ─────────────────────────────────────────────────

/// Native → protobuf modulation.
pub fn modulation_to_pb(m: Modulation) -> pb::ModulationType {
    use pb::ModulationType as P;
    match m {
        Modulation::AdslAnsiT1413 => P::AdslT1413,
        Modulation::AdslGdmt => P::AdslGdmt,
        Modulation::AdslGlite => P::AdslGlite,
        Modulation::AdslGdmtBis => P::Adsl2,
        Modulation::Adsl2Plus => P::Adsl2plus,
        Modulation::AdslMultimode | Modulation::Multimode => P::Multimode,
        Modulation::Vdsl2 => P::Vdsl2,
    }
}

/// Protobuf → native modulation (`None` for UNSPECIFIED).
pub fn modulation_from_pb(m: pb::ModulationType) -> Option<Modulation> {
    use pb::ModulationType as P;
    Some(match m {
        P::AdslT1413 => Modulation::AdslAnsiT1413,
        P::AdslGdmt => Modulation::AdslGdmt,
        P::AdslGlite => Modulation::AdslGlite,
        P::Adsl2 => Modulation::AdslGdmtBis,
        P::Adsl2plus => Modulation::Adsl2Plus,
        P::Multimode => Modulation::Multimode,
        P::Vdsl2 => Modulation::Vdsl2,
        P::ModulationUnspecified => return None,
    })
}

/// Native → protobuf annex. `None` for the exotic `AL`/`ALM`/`BJ` codes
/// the protobuf vocabulary does not model.
pub fn annex_to_pb(a: Annex) -> Option<pb::AnnexType> {
    use pb::AnnexType as P;
    Some(match a {
        Annex::A => P::A,
        Annex::B => P::B,
        Annex::I => P::I,
        Annex::M => P::M,
        Annex::J => P::J,
        Annex::Auto => P::Auto,
        Annex::AL => P::Al,
        Annex::ALM => P::Alm,
        Annex::BJ => P::Bj,
    })
}

/// Protobuf → native annex (`None` for UNSPECIFIED).
pub fn annex_from_pb(a: pb::AnnexType) -> Option<Annex> {
    use pb::AnnexType as P;
    Some(match a {
        P::A => Annex::A,
        P::B => Annex::B,
        P::I => Annex::I,
        P::M => Annex::M,
        P::J => Annex::J,
        P::Auto => Annex::Auto,
        P::Al => Annex::AL,
        P::Alm => Annex::ALM,
        P::Bj => Annex::BJ,
        P::AnnexUnspecified => return None,
    })
}

/// Native → protobuf data path (lossy: `Unknown` → `UNSPECIFIED`).
pub fn data_path_to_pb(d: DataPath) -> pb::DataPath {
    match d {
        DataPath::Atm => pb::DataPath::Atm,
        DataPath::Ptm => pb::DataPath::Ptm,
        DataPath::Unknown(_) => pb::DataPath::Unspecified,
    }
}

/// Protobuf → native data path.
pub fn data_path_from_pb(d: pb::DataPath) -> DataPath {
    match d {
        pb::DataPath::Atm => DataPath::Atm,
        pb::DataPath::Ptm => DataPath::Ptm,
        pb::DataPath::Unspecified => DataPath::Unknown(0),
    }
}

// ─── line params: pb conversions (the struct lives in `crate::model`) ─

impl LineParams {
    pub fn to_pb(self) -> pb::LineConfig {
        pb::LineConfig {
            modulation: modulation_to_pb(self.modulation) as i32,
            annex: annex_to_pb(self.annex).map(|a| a as i32).unwrap_or(0),
            bitswap: self.bitswap,
            sra: self.sra,
            vdsl2_profile: self.profiles.bitmask(),
            ..Default::default()
        }
    }

    /// Inverse of [`Self::to_pb`] for the common subset. `None` if the
    /// message carries values outside the native vocabulary
    /// (UNSPECIFIED enums, or an annex the OEM protocol never had).
    pub fn from_pb(cfg: &pb::LineConfig) -> Option<Self> {
        Some(Self {
            modulation: modulation_from_pb(pb::ModulationType::try_from(cfg.modulation).ok()?)?,
            annex: annex_from_pb(pb::AnnexType::try_from(cfg.annex).ok()?)?,
            bitswap: cfg.bitswap,
            sra: cfg.sra,
            profiles: Vdsl2Profiles(cfg.vdsl2_profile as u16),
        })
    }
}

// ─── ATM/PTM link params: pb conversions (structs live in `crate::model`) ─

impl AtmParams {
    pub fn to_pb(self) -> pb::AtmLinkParams {
        pb::AtmLinkParams {
            vpi: self.vpi as u32,
            vci: self.vci as u32,
            encap: self.encap as u32,
            link_type: self.link_type as u32,
            qos: self.qos as u32,
            pcr: self.pcr,
            scr: self.scr,
            mbs: self.mbs,
        }
    }

    pub fn from_pb(p: &pb::AtmLinkParams) -> Self {
        Self {
            vpi: p.vpi as u8,
            vci: p.vci as u16,
            encap: AtmEncap::from_code(p.encap as u8),
            link_type: AtmLinkType::from_code(p.link_type as u8),
            qos: AtmQos::from_code(p.qos as u8),
            pcr: p.pcr,
            scr: p.scr,
            mbs: p.mbs,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────

impl PtmParams {
    pub fn to_pb(self) -> pb::PtmLinkParams {
        pb::PtmLinkParams {
            tag_enable: self.tag_enable,
            tag_vid: self.tag_vid as u32,
            tag_pri: self.tag_pri as u32,
        }
    }

    pub fn from_pb(p: &pb::PtmLinkParams) -> Self {
        Self { tag_enable: p.tag_enable, tag_vid: p.tag_vid as u16, tag_pri: p.tag_pri as u16 }
    }
}

// ─── request builders (host → board) ──────────────────────────────────

fn req(id: u32, payload: pb::request::Payload) -> pb::Request {
    pb::Request { id, payload: Some(payload) }
}

pub fn hello(id: u32) -> pb::Request {
    req(id, pb::request::Payload::Hello(pb::HelloRequest {}))
}

pub fn get_line_status(id: u32) -> pb::Request {
    req(id, pb::request::Payload::LineStatus(pb::GetLineStatusRequest {}))
}

pub fn get_line_metrics(id: u32) -> pb::Request {
    req(id, pb::request::Payload::LineMetrics(pb::GetLineMetricsRequest {}))
}

pub fn get_config(id: u32) -> pb::Request {
    req(id, pb::request::Payload::GetConfig(pb::GetConfigRequest {}))
}

/// Params-only apply (never opens; cache hits reported via
/// `ConfigResponse.applied`; closed line → envelope error).
pub fn line_config(id: u32, params: &LineParams) -> pb::Request {
    req(id, pb::request::Payload::Config(pb::ConfigRequest {
        action: Some(pb::config_request::Action::LineConfig(params.to_pb())),
    }))
}

/// Apply config AND ensure the line is open — the double-apply
/// replacement. Idempotent; reports `applied`/`opened`.
pub fn line_open(id: u32, params: &LineParams) -> pb::Request {
    req(id, pb::request::Payload::Config(pb::ConfigRequest {
        action: Some(pb::config_request::Action::LineOpen(pb::LineOpen {
            config: Some(params.to_pb()),
        })),
    }))
}

pub fn line_close(id: u32) -> pb::Request {
    req(id, pb::request::Payload::Config(pb::ConfigRequest {
        action: Some(pb::config_request::Action::LineClose(pb::LineOp {})),
    }))
}

/// Atomic agent-side close + open with the current params.
pub fn line_retrain(id: u32) -> pb::Request {
    req(id, pb::request::Payload::Config(pb::ConfigRequest {
        action: Some(pb::config_request::Action::LineRetrain(pb::LineOp {})),
    }))
}

/// Create a data link (queued agent-side until showtime if the line is
/// down — see `LinkResponse.queued` / `LinkActiveEvent`).
pub fn link_create_atm(id: u32, vlan: u16, p: &AtmParams) -> pb::Request {
    req(id, pb::request::Payload::Link(pb::LinkRequest {
        action: Some(pb::link_request::Action::Create(pb::LinkCreate {
            vlan_id: vlan as u32,
            kind: Some(pb::link_create::Kind::Atm(p.to_pb())),
        })),
    }))
}

pub fn link_create_ptm(id: u32, vlan: u16, p: &PtmParams) -> pb::Request {
    req(id, pb::request::Payload::Link(pb::LinkRequest {
        action: Some(pb::link_request::Action::Create(pb::LinkCreate {
            vlan_id: vlan as u32,
            kind: Some(pb::link_create::Kind::Ptm(p.to_pb())),
        })),
    }))
}

pub fn link_remove(id: u32, vlan: u16) -> pb::Request {
    req(id, pb::request::Payload::Link(pb::LinkRequest {
        action: Some(pb::link_request::Action::Remove(pb::LinkRemove {
            vlan_id: vlan as u32,
        })),
    }))
}

// ─── response / event builders (board → host) ─────────────────────────
// Used by the agent and by test/fake boards; the daemon only decodes.

fn resp_ok(id: u32, payload: pb::response::Payload) -> pb::Response {
    pb::Response { id, ok: true, error: String::new(), payload: Some(payload) }
}

/// Error response with the `prefix: description` convention from
/// plans/protobuf.md → Error handling (prefix must be a stable
/// machine-readable identifier).
pub fn response_error(id: u32, prefix: &str, description: &str) -> pb::Response {
    pb::Response {
        id,
        ok: false,
        error: format!("{prefix}: {description}"),
        payload: None,
    }
}

pub fn response_line_status(id: u32, s: pb::LineStatusResponse) -> pb::Response {
    resp_ok(id, pb::response::Payload::LineStatus(s))
}

pub fn response_line_metrics(id: u32, m: pb::LineMetricsResponse) -> pb::Response {
    resp_ok(id, pb::response::Payload::LineMetrics(m))
}

pub fn response_config(id: u32, applied: bool, opened: bool) -> pb::Response {
    resp_ok(id, pb::response::Payload::Config(pb::ConfigResponse { applied, opened }))
}

pub fn response_link(id: u32, vlan: u16, queued: bool) -> pb::Response {
    resp_ok(id, pb::response::Payload::Link(pb::LinkResponse {
        vlan_id: vlan as u32,
        queued,
    }))
}

pub fn event_link_status(old: pb::LinkState, new: pb::LinkState) -> pb::Event {
    pb::Event {
        payload: Some(pb::event::Payload::LinkStatus(pb::LinkStatusEvent {
            old_state: old as i32,
            new_state: new as i32,
            timestamp: 0, // caller stamps
        })),
    }
}

pub fn event_link_active(vlan: u16, kind: DataPath) -> pb::Event {
    pb::Event {
        payload: Some(pb::event::Payload::LinkActive(pb::LinkActiveEvent {
            vlan_id: vlan as u32,
            kind: data_path_to_pb(kind) as i32,
            timestamp: 0, // caller stamps
        })),
    }
}

// Re-exported for codec callers: the message type of an outgoing frame is
// implied by which builder produced the message.
pub use framing::MessageType;
