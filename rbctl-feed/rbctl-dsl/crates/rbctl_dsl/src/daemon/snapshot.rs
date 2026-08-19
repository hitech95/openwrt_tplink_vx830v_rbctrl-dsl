//! Status snapshot builder — renders the daemon's shared state into the
//! [`StatusSnapshot`] served over IPC `STATUS`.
//!
//! Reads only the state mutex (never the board, never `cfg`), so it is
//! constant-time and safe to call from the IPC path while the worker is
//! mid-request.

use crate::config::XferMode;
use crate::constants;
use super::ipc::StatusSnapshot;
use super::Daemon;
use super::state::SharedState;
use crate::utils::{mapping, mgmt_link};

impl Daemon {
    /// Build a [`StatusSnapshot`] from the current shared state.
    pub(super) fn build_snapshot(&self) -> StatusSnapshot {
        build_snapshot(&self.state, &self.config_iface, self.monitor)
    }
}

/// Free-function form (state + iface + monitor) so it can be unit-tested
/// without constructing a full daemon.
pub(crate) fn build_snapshot(
    state: &SharedState,
    config_iface: &str,
    monitor: bool,
) -> StatusSnapshot {
    let st = state.lock().unwrap();
    let fw = st.fw_status.as_str();
    let board_reachable = st
        .last_poll_ok
        .map_or(false, |t| t.elapsed() < constants::POLL_STALE_TIMEOUT);
    let link = mgmt_link::MgmtLinkStatus::probe(config_iface);

    let snap = match &st.line_obj {
        Some(line) => {
            let (state_str, state_num, up) = mapping::map_link_status(line.link_status);
            // Training-dependent fields are only filled once the line is
            // Up: on an idle/training line the board reports driver
            // defaults (T1.413 / Annex A / zeroed metrics) which would be
            // misleading — they stay None (rendered as `null`).
            let trained = up.then(|| {
                (
                    format!("{:?}", line.modulation_code),
                    mapping::annex_string(line.annex_code).to_string(),
                )
            });
            StatusSnapshot {
                state: state_str.into(),
                state_num,
                up,
                uptime_secs: up.then_some(st.uptime_secs),
                modulation: trained.as_ref().map(|(m, _)| m.clone()),
                annex: trained.map(|(_, a)| a),
                down_rate: up.then_some(line.metrics.down_rate),
                up_rate: up.then_some(line.metrics.up_rate),
                down_snr: up.then_some(line.metrics.down_noise_margin),
                up_snr: up.then_some(line.metrics.up_noise_margin),
                xfer_mode: format!("{:?}", st.xfer_mode.unwrap_or(XferMode::Ptm)),
                fw_status: fw,
                board_reachable,
                mgmt_iface: config_iface.into(),
                mgmt_carrier: link.carrier,
                monitor,
            }
        }
        // No line data at all (never polled / stale): training-dependent
        // fields keep their Default (None).
        None => StatusSnapshot {
            xfer_mode: format!("{:?}", st.xfer_mode.unwrap_or(XferMode::Ptm)),
            fw_status: fw,
            board_reachable,
            mgmt_iface: config_iface.into(),
            mgmt_carrier: link.carrier,
            monitor,
            ..Default::default()
        },
    };
    snap
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::new_shared_state;
    use rbctl_proto::oem::unpack::{DataPath, LineObj, LinkStatus};

    fn state_with_line(link_status: LinkStatus) -> SharedState {
        let state = new_shared_state();
        let mut st = state.lock().unwrap();
        st.line_obj = Some(LineObj {
            status: 0,
            link_status,
            // Untrained-line driver defaults — must NOT leak into status.
            modulation_code: 0,
            data_path: DataPath::Atm,
            annex_code: 0,
            vdsl2_profile_bitmask: 0,
            is_atm: true,
            uptime_secs: 0,
            metrics: Default::default(),
        });
        drop(st);
        state
    }

    #[test]
    fn untrained_line_reports_null_mod_annex() {
        let state = state_with_line(LinkStatus::NoSignal);
        let snap = build_snapshot(&state, "lan0.500", false);
        assert!(!snap.up);
        assert_eq!(snap.modulation, None);
        assert_eq!(snap.annex, None);
        assert_eq!(snap.uptime_secs, None);
        assert_eq!(snap.down_rate, None);
        assert_eq!(snap.up_snr, None);
    }

    #[test]
    fn training_line_reports_null_mod_annex() {
        let state = state_with_line(LinkStatus::Initializing);
        let snap = build_snapshot(&state, "lan0.500", false);
        assert!(!snap.up);
        assert_eq!(snap.modulation, None);
        assert_eq!(snap.annex, None);
    }

    #[test]
    fn trained_line_reports_real_mod_annex() {
        let state = state_with_line(LinkStatus::Up);
        let mut st = state.lock().unwrap();
        st.uptime_secs = 42;
        st.line_obj.as_mut().unwrap().metrics.down_rate = 40000;
        drop(st);
        let snap = build_snapshot(&state, "lan0.500", false);
        assert!(snap.up);
        assert_eq!(snap.modulation.as_deref(), Some("0"));
        assert_eq!(snap.annex.as_deref(), Some("A"));
        assert_eq!(snap.uptime_secs, Some(42));
        assert_eq!(snap.down_rate, Some(40000));
    }
}
