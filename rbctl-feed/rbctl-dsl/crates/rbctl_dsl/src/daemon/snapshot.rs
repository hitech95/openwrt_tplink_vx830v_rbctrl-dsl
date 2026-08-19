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
            StatusSnapshot {
                state: state_str.into(),
                state_num,
                up,
                uptime_secs: st.uptime_secs,
                modulation: format!("{:?}", line.modulation_code),
                annex: format!("{}", mapping::annex_string(line.annex_code)),
                xfer_mode: format!("{:?}", st.xfer_mode.unwrap_or(XferMode::Ptm)),
                down_rate: line.metrics.down_rate,
                up_rate: line.metrics.up_rate,
                down_snr: line.metrics.down_noise_margin,
                up_snr: line.metrics.up_noise_margin,
                fw_status: fw,
                board_reachable,
                mgmt_iface: config_iface.into(),
                mgmt_carrier: link.carrier,
                monitor,
            }
        }
        None => StatusSnapshot {
            modulation: format!("{:?}", st.modulation),
            annex: format!("{:?}", st.annex),
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
