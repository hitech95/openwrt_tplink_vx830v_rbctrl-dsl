//! Typed response/event decoders — the protobuf analog of
//! [`crate::oem::unpack`].
//!
//! `pb::Response`/`pb::Event` in, the **same native structs the daemon
//! already consumes** out, so the state machine, snapshot and ubus layers
//! are backend-agnostic:
//!
//! * [`BasicStatus`] — basic poll tier; `modulation()` /
//!   `native_state()` land on the shared model types
//!   (`model::LineState`) the daemon gates on.
//! * `LineMetrics` — reused **as-is** from the model (same units:
//!   rates kbps, SNR/attenuation dB×10, uptime seconds), so the snapshot
//!   code is untouched by a backend switch.
//! * [`ConfigData`] — `get_config` readback with per-link queued state.

use super::pack::{annex_from_pb, modulation_from_pb};
use super::rbctl::{self as pb};
use crate::model::{
    Annex, AtmParams, DataPath, LineMetrics, LineParams, LineState, Modulation, PtmParams,
};

// ─── basic status ─────────────────────────────────────────────────────

/// Native view of `pb::LineStatusResponse` (the poll tier).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BasicStatus {
    pub link_state: pb::LinkState,
    pub modulation: Option<Modulation>,
    pub data_path: DataPath,
    pub annex: Option<Annex>,
    pub uptime_secs: u32,
}

impl BasicStatus {
    pub fn from_pb(s: &pb::LineStatusResponse) -> Self {
        Self {
            link_state: pb::LinkState::try_from(s.link_state)
                .unwrap_or(pb::LinkState::Unspecified),
            modulation: pb::ModulationType::try_from(s.modulation).ok().and_then(modulation_from_pb),
            data_path: match pb::DataPath::try_from(s.data_path).ok() {
                Some(pb::DataPath::Atm) => DataPath::Atm,
                Some(pb::DataPath::Ptm) => DataPath::Ptm,
                _ => DataPath::Unknown(0),
            },
            annex: pb::AnnexType::try_from(s.annex).ok().and_then(annex_from_pb),
            uptime_secs: s.uptime_secs,
        }
    }

    /// Map to the daemon's native `LineState` vocabulary. `CLOSED` (the
    /// admin state) maps to `Silent` — operationally a closed line is not
    /// training — so poll-loop gating keeps working; callers that need to
    /// distinguish admin-down check [`Self::link_state`] directly.
    pub fn native_state(&self) -> LineState {
        match self.link_state {
            pb::LinkState::Showtime => LineState::ShowtimeTcSync,
            pb::LinkState::Handshake => LineState::Handshake,
            pb::LinkState::Training => LineState::FullInit,
            pb::LinkState::NoSignal | pb::LinkState::Closed => LineState::Silent,
            pb::LinkState::Unspecified => LineState::NotInitialized,
        }
    }

    pub fn is_up(&self) -> bool {
        self.link_state == pb::LinkState::Showtime
    }

    pub fn is_closed(&self) -> bool {
        self.link_state == pb::LinkState::Closed
    }
}

// ─── metrics ──────────────────────────────────────────────────────────

/// Directional pair helper (both fields default 0 when absent).
fn dir(d: &Option<pb::DirectionalU32>) -> (u32, u32) {
    match d {
        Some(d) => (d.downstream, d.upstream),
        None => (0, 0),
    }
}

/// Convert the full metrics tier into the native [`LineMetrics`] the
/// daemon's snapshot already holds — identical units on both protocols
/// (rates kbps, dB×10 signals, seconds).
pub fn metrics_from_pb(m: &pb::LineMetricsResponse) -> LineMetrics {
    let (down_rate, up_rate) = dir(&m.current_rate);
    let (down_attainable_rate, up_attainable_rate) = dir(&m.attainable_rate);
    let (down_noise_margin, up_noise_margin) = dir(&m.noise_margin);
    let (down_attenuation, up_attenuation) = dir(&m.attenuation);
    let (down_output_power, up_output_power) = dir(&m.output_power);
    LineMetrics {
        down_rate,
        up_rate,
        down_attainable_rate,
        up_attainable_rate,
        down_noise_margin,
        up_noise_margin,
        down_attenuation,
        up_attenuation,
        down_output_power,
        up_output_power,
        uptime_showtime_secs: m.current_act_seconds,
        uptime_total_secs: m.total_act_seconds,
    }
}

/// Channel blocks (absorbed old channel_stats / opcode 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ChannelBlocks {
    pub rx_blocks: u32,
    pub rx_errors: u32,
    pub rx_discards: u32,
    pub tx_blocks: u32,
    pub tx_errors: u32,
    pub tx_discards: u32,
}

impl ChannelBlocks {
    pub fn from_pb(b: &Option<pb::ChannelBlocks>) -> Self {
        match b {
            Some(b) => Self {
                rx_blocks: b.rx_blocks,
                rx_errors: b.rx_errors,
                rx_discards: b.rx_discards,
                tx_blocks: b.tx_blocks,
                tx_errors: b.tx_errors,
                tx_discards: b.tx_discards,
            },
            None => Self::default(),
        }
    }
}

// ─── config readback ──────────────────────────────────────────────────

/// One requested data link from `get_config`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkEntryData {
    pub vlan: u16,
    pub kind: LinkKind,
    /// Requested but deferred until the line trains (not yet
    /// materialized). Authoritative — "line is up" only approximates it.
    pub queued: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    Atm(AtmParams),
    Ptm(PtmParams),
}

/// Native view of `pb::ConfigDataResponse` (get_config readback).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConfigData {
    pub line: Option<LineParams>,
    pub links: Vec<LinkEntryData>,
}

impl ConfigData {
    pub fn from_pb(c: &pb::ConfigDataResponse) -> Self {
        Self {
            line: c.config.as_ref().and_then(LineParams::from_pb),
            links: c
                .links
                .iter()
                .map(|e| LinkEntryData {
                    vlan: e.vlan_id as u16,
                    kind: match &e.kind {
                        Some(pb::link_entry::Kind::Atm(a)) => LinkKind::Atm(AtmParams::from_pb(a)),
                        Some(pb::link_entry::Kind::Ptm(p)) => LinkKind::Ptm(PtmParams::from_pb(p)),
                        None => LinkKind::Ptm(PtmParams::default()),
                    },
                    queued: e.queued,
                })
                .collect(),
        }
    }
}

// ─── link-ops results & events ────────────────────────────────────────

/// Native view of `pb::LinkResponse` (create/remove result).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkResult {
    pub vlan: u16,
    /// Deferred until showtime (create on a down line).
    pub queued: bool,
}

impl LinkResult {
    pub fn from_pb(r: &pb::LinkResponse) -> Self {
        Self { vlan: r.vlan_id as u16, queued: r.queued }
    }
}

/// Native view of `pb::LinkActiveEvent` — the data-plane-materialized push.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkActive {
    pub vlan: u16,
    pub kind: DataPath,
    pub timestamp: u64,
}

impl LinkActive {
    pub fn from_pb(e: &pb::LinkActiveEvent) -> Self {
        Self {
            vlan: e.vlan_id as u16,
            kind: match pb::DataPath::try_from(e.kind).ok() {
                Some(pb::DataPath::Atm) => DataPath::Atm,
                Some(pb::DataPath::Ptm) => DataPath::Ptm,
                _ => DataPath::Unknown(0),
            },
            timestamp: e.timestamp,
        }
    }
}

// ─── errors ───────────────────────────────────────────────────────────

/// Split an error response into its stable machine prefix and human
/// description (see plans/protobuf.md → Error handling). Returns `None`
/// for a success response.
///
/// ```
/// // "link_exists: PTM link on vlan 2000 already active — remove it first"
/// //   → ("link_exists", "PTM link on vlan 2000 …")
/// ```
pub fn split_error(r: &pb::Response) -> Option<(&str, &str)> {
    if r.ok {
        return None;
    }
    let (prefix, rest) = r.error.split_once(':')?;
    // Everything after ": " — tolerate a missing space for robustness.
    let desc = rest.strip_prefix(' ').unwrap_or(rest);
    Some((prefix, desc))
}
