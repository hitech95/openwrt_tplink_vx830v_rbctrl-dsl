//! Board worker — owns the [`Board`] and runs the periodic line-status poll,
//! reload, restart-line, and firmware-upgrade logic on a dedicated thread so
//! the main loop's IPC and ubus paths stay responsive regardless of board
//! latency.
//!
//! Commands (reload / restart-line / firmware-upgrade / stop) flow through an
//! mpsc channel (`WorkerCmd`); the worker polls it on a short tick. See
//! `plans/daemon-event-loop-plan.md` and `plans/firmware-upgrade-plan.md`.

use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use rbctl_proto::unpack::LinkStatus;

use crate::board::{Board, BoardError, FwProgress, FwUpgradeResult};
use crate::daemon::{SHOULD_EXIT, SHOULD_RELOAD, SHOULD_RESTART_LINE};
use crate::hotplug::{self, LineEvent};
use crate::uci_cfg::{CliOverrides, DslConfig};
use crate::ubus_obj::SharedState;

/// Worker tick — bounds how long a command waits before being acted upon.
const WORKER_TICK: Duration = Duration::from_millis(50);

/// Commands sent from the main thread to the worker.
pub enum WorkerCmd {
    /// Reload UCI config (same as SIGHUP).
    Reload,
    /// Restart DSL line (bounce).
    RestartLine,
    /// Upload firmware to the board. `reply_tx` receives progress + result.
    FirmwareUpgrade {
        image: Vec<u8>,
        reply_tx: mpsc::Sender<FwEvent>,
    },
}

/// Events sent from the worker back to the firmware requester.
pub enum FwEvent {
    Progress(FwProgress),
    Done(Result<FwUpgradeResult, String>),
}

pub struct BoardWorker {
    board: Board,
    cfg: DslConfig,
    overrides: CliOverrides,
    state: SharedState,
    notify_script: Option<String>,
    poll_interval: Duration,
    last_poll: Option<Instant>,
    last_status: Option<LinkStatus>,
    up_since: Option<Instant>,
    tc_emitted: bool,
    cmd_rx: mpsc::Receiver<WorkerCmd>,
}

impl BoardWorker {
    pub(crate) fn new(
        board: Board,
        cfg: DslConfig,
        overrides: CliOverrides,
        state: SharedState,
        notify_script: Option<String>,
        cmd_rx: mpsc::Receiver<WorkerCmd>,
    ) -> Self {
        Self {
            board,
            cfg,
            overrides,
            state,
            notify_script,
            poll_interval: Duration::from_secs(1),
            last_poll: None,
            last_status: None,
            up_since: None,
            tc_emitted: false,
            cmd_rx,
        }
    }

    /// Run the worker loop until `SHOULD_EXIT`, then perform a best-effort
    /// `line_config_down()`.
    pub(crate) fn run(mut self) {
        log::info!("board worker started");
        while !SHOULD_EXIT.load(Ordering::SeqCst) {
            // Check for commands from main thread (non-blocking via timeout)
            match self.cmd_rx.recv_timeout(WORKER_TICK) {
                Ok(WorkerCmd::Reload) => self.handle_reload(),
                Ok(WorkerCmd::RestartLine) => self.handle_restart(),
                Ok(WorkerCmd::FirmwareUpgrade { image, reply_tx }) => {
                    self.handle_firmware_upgrade(image, reply_tx);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    log::warn!("worker command channel disconnected, exiting");
                    break;
                }
            }

            self.maybe_poll_board();
        }

        // Best-effort line-down.
        self.board.set_retries(0);
        let _ = self.board.line_config_down();
        log::info!("board worker exiting");
    }

    /// Publish cfg-derived display fields into shared state (callers read
    /// these instead of borrowing cfg, which lives on this thread).
    fn publish_config(&self) {
        let mut st = self.state.lock().unwrap();
        st.xfer_mode = Some(self.cfg.xfer_mode);
        st.modulation = self.cfg.modulation;
        st.annex = self.cfg.annex;
    }

    fn handle_reload(&mut self) {
        log::info!("reload — re-reading UCI config");
        if let Ok(new_cfg) = DslConfig::load(&self.overrides) {
            if config_changed(&self.cfg, &new_cfg) {
                log::info!("line params changed — reconfiguring");
                let _ = self.board.line_config_down();
                let _ = self.board.line_config_up(
                    new_cfg.modulation, new_cfg.annex, new_cfg.profiles,
                    new_cfg.bitswap, new_cfg.sra,
                );
                self.cfg = new_cfg;
                self.publish_config();
                self.tc_emitted = false;
            } else {
                log::info!("no line-param changes");
            }
        }
    }

    fn handle_restart(&mut self) {
        log::info!("restart-line — bouncing DSL line");
        let _ = self.board.line_config_down();
        std::thread::sleep(Duration::from_secs(1));
        let _ = self.board.line_config_up(
            self.cfg.modulation, self.cfg.annex, self.cfg.profiles,
            self.cfg.bitswap, self.cfg.sra,
        );
        self.tc_emitted = false;
    }

    fn handle_firmware_upgrade(
        &mut self,
        image: Vec<u8>,
        reply_tx: mpsc::Sender<FwEvent>,
    ) {
        // Set fw_status in shared state
        {
            let mut st = self.state.lock().unwrap();
            st.fw_status = crate::ubus_obj::FwStatus::Upgrading;
        }

        log::info!("firmware upgrade: {} bytes", image.len());
        let result = self.board.firmware_upgrade(&image, &mut |p| {
            let _ = reply_tx.send(FwEvent::Progress(p.clone()));
            // Update shared state for status queries
            let mut st = self.state.lock().unwrap();
            st.fw_status = crate::ubus_obj::FwStatus::UpgradingProgress {
                stage: p.stage,
                pct: p.pct,
            };
        });

        let event = match &result {
            Ok(r) => {
                log::info!("firmware upgrade done: version=0x{:08X}", r.version);
                let mut st = self.state.lock().unwrap();
                st.fw_status = crate::ubus_obj::FwStatus::Done;
                FwEvent::Done(Ok(r.clone()))
            }
            Err(e) => {
                log::error!("firmware upgrade failed: {e}");
                let mut st = self.state.lock().unwrap();
                st.fw_status = crate::ubus_obj::FwStatus::Failed(e.to_string());
                FwEvent::Done(Err(e.to_string()))
            }
        };
        let _ = reply_tx.send(event);
    }

    fn maybe_poll_board(&mut self) {
        let due = self.last_poll.map_or(true, |t| t.elapsed() >= self.poll_interval);
        if !due {
            return;
        }
        self.last_poll = Some(Instant::now());

        match self.board.get_line_obj() {
            Ok(line) => {
                let status = line.link_status;

                if status == LinkStatus::Up && self.up_since.is_none() {
                    self.up_since = Some(Instant::now());
                } else if status != LinkStatus::Up {
                    self.up_since = None;
                }

                {
                    let mut st = self.state.lock().unwrap();
                    st.line_obj = Some(line.clone());
                    st.uptime_secs = self.up_since.map(|t| t.elapsed().as_secs()).unwrap_or(0);
                    st.xfer_mode = Some(self.cfg.xfer_mode);
                    st.modulation = self.cfg.modulation;
                    st.annex = self.cfg.annex;
                }

                if Some(status) != self.last_status {
                    let event = link_status_to_event(status);
                    log::info!("line state: {status:?} → {event:?}");
                    if let Some(script) = self.notify_script.as_deref() {
                        hotplug::emit_status(script, event);
                    }
                    if status == LinkStatus::Up && !self.tc_emitted {
                        if let Some(script) = self.notify_script.as_deref() {
                            hotplug::emit_tc_layer(script, self.cfg.xfer_mode);
                        }
                        self.tc_emitted = true;
                    }
                    self.last_status = Some(status);
                }
            }
            Err(BoardError::Timeout) => {}
            Err(e) => log::error!("poll error: {e}"),
        }
    }
}

// ── helpers (moved from daemon.rs — used only by the worker) ──────────────

fn link_status_to_event(status: LinkStatus) -> LineEvent {
    match status {
        LinkStatus::Up => LineEvent::Up,
        LinkStatus::Initializing => LineEvent::Training,
        LinkStatus::EstablishingLink => LineEvent::Handshake,
        LinkStatus::NoSignal | LinkStatus::Unknown(_) => LineEvent::Down,
    }
}

fn config_changed(old: &DslConfig, new: &DslConfig) -> bool {
    old.modulation != new.modulation
        || old.annex != new.annex
        || old.profiles.bitmask() != new.profiles.bitmask()
        || old.xfer_mode != new.xfer_mode
}

#[cfg(test)]
mod tests {
    use super::*;
    use rbctl_proto::pack::Annex;

    #[test]
    fn link_status_events() {
        assert_eq!(link_status_to_event(LinkStatus::Up), LineEvent::Up);
        assert_eq!(link_status_to_event(LinkStatus::NoSignal), LineEvent::Down);
        assert_eq!(link_status_to_event(LinkStatus::Initializing), LineEvent::Training);
        assert_eq!(link_status_to_event(LinkStatus::EstablishingLink), LineEvent::Handshake);
    }

    #[test]
    fn config_change_detection() {
        let a = DslConfig::default();
        let b = DslConfig { annex: Annex::A, ..DslConfig::default() };
        assert!(config_changed(&a, &b));
    }

    #[test]
    fn config_no_change() {
        let a = DslConfig::default();
        let b = DslConfig::default();
        assert!(!config_changed(&a, &b));
    }
}
