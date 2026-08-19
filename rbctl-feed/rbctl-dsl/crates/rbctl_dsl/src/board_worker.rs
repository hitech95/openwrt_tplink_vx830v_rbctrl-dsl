//! Board worker — polls the board (op2) and executes commands sent by the
//! daemon's control loop.
//!
//! The worker makes **no autonomous decisions**: every configuration or
//! provisioning action is gated by the daemon, which sends [`WorkerCmd`]s
//! and consumes [`WorkerEvent`]s. The worker's own responsibilities are
//! exactly two:
//!
//! 1. Poll `get_line_obj` (op2) once per [`constants::POLL_INTERVAL`] and
//!    publish the result into the shared state.
//! 2. Report state transitions (line state, board reachability) back to
//!    the daemon as events, and execute whatever commands arrive.
//!
//! This keeps the board socket single-owned (no locking) while the daemon
//! stays the sole policy maker — monitor mode, retries, and deferred
//! provisioning all live in `daemon/mod.rs`.

use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use rbctl_proto::oem::unpack::{DataPath, LinkStatus};

use crate::board::{Board, BoardError, FwProgress, FwUpgradeResult};
use crate::constants;
use crate::daemon::signals::SHOULD_EXIT;
use crate::uci_cfg::{DslConfig, XferMode};
use crate::ubus_obj::SharedState;
use crate::utils::xdsl;

/// Worker tick — bounds how long a command waits before being acted upon.
const WORKER_TICK: Duration = constants::POLL_TICK;

/// Commands sent from the daemon (the gate) to the worker.
pub enum WorkerCmd {
    /// op1 — apply `cfg` line config to the board.
    ApplyLineConfig,
    /// op5/op15 — provision the data-plane link for `xfer`.
    ProvisionLink { xfer: XferMode },
    /// op3 + delay + op1 — bounce the line.
    RestartLine,
    /// op3 — best-effort teardown (zero retries).
    LineDown,
    /// Replace the worker's config copy (display fields + command params).
    UpdateConfig(Box<DslConfig>),
    /// Upload firmware to the board. `reply_tx` receives progress + result.
    FirmwareUpgrade {
        image: Vec<u8>,
        reply_tx: mpsc::Sender<FwEvent>,
    },
    /// Exit the worker loop.
    Shutdown,
}

/// Events reported from the worker back to the daemon (the gate decides
/// what to do with them).
pub enum WorkerEvent {
    /// The board started responding to op2.
    BoardReachable,
    /// The board stopped responding to op2.
    BoardUnreachable,
    /// Line state transition. `data_path` is the op2-observed TC mode,
    /// meaningful when `status` is `Up` (drives auto provisioning).
    LineState { status: LinkStatus, data_path: DataPath },
    /// op1 acknowledged by the board.
    LineConfigApplied,
    /// op5/op15 acknowledged; `vlan` is the board-assigned transport VLAN.
    LinkProvisioned { vlan: u16 },
}

/// Events sent from the worker back to the firmware requester.
pub enum FwEvent {
    Progress(FwProgress),
    Done(Result<FwUpgradeResult, String>),
}

pub struct BoardWorker {
    board: Board,
    cfg: DslConfig,
    state: SharedState,
    cmd_rx: mpsc::Receiver<WorkerCmd>,
    event_tx: mpsc::Sender<WorkerEvent>,
    poll_interval: Duration,
    last_poll: Option<Instant>,
    last_status: Option<LinkStatus>,
    up_since: Option<Instant>,
    board_reachable: bool,
}

impl BoardWorker {
    pub(crate) fn new(
        board: Board,
        cfg: DslConfig,
        state: SharedState,
        cmd_rx: mpsc::Receiver<WorkerCmd>,
        event_tx: mpsc::Sender<WorkerEvent>,
    ) -> Self {
        Self {
            board,
            cfg,
            state,
            cmd_rx,
            event_tx,
            poll_interval: constants::POLL_INTERVAL,
            last_poll: None,
            last_status: None,
            up_since: None,
            board_reachable: false,
        }
    }

    pub(crate) fn run(mut self) {
        log::info!("board worker started (poll + command executor)");
        loop {
            match self.cmd_rx.recv_timeout(WORKER_TICK) {
                Ok(WorkerCmd::Shutdown) => break,
                Ok(cmd) => self.handle_cmd(cmd),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    log::warn!("worker command channel disconnected, exiting");
                    break;
                }
            }
            if !SHOULD_EXIT.load(Ordering::SeqCst) {
                self.maybe_poll_board();
            }
        }
        log::info!("board worker exiting");
    }

    // ── command execution (no decisions — the daemon already decided) ──

    fn handle_cmd(&mut self, cmd: WorkerCmd) {
        match cmd {
            WorkerCmd::ApplyLineConfig => {
                match self.board.line_config_up(
                    self.cfg.modulation,
                    self.cfg.annex,
                    self.cfg.profiles,
                    self.cfg.bitswap,
                    self.cfg.sra,
                ) {
                    Ok(()) => {
                        let _ = self.event_tx.send(WorkerEvent::LineConfigApplied);
                    }
                    Err(e) => log::warn!("line_config_up: {e} (daemon will retry)"),
                }
            }
            WorkerCmd::ProvisionLink { xfer } => {
                let vlan = self.cfg.transport_vlan_base as u16
                    + constants::VLAN_TRANSPORT_BASE_OFFSET;
                let result = match xfer {
                    XferMode::Ptm => self.board.ptm_link_add(0, 0, 0, vlan),
                    XferMode::Atm => {
                        let params = xdsl::atm_params(self.cfg.atm.as_ref(), vlan);
                        self.board.atm_link_add(&params)
                    }
                    XferMode::Auto => {
                        log::warn!("ProvisionLink with XferMode::Auto — ignored");
                        return;
                    }
                };
                match result {
                    Ok(v) => {
                        let _ = self.event_tx.send(WorkerEvent::LinkProvisioned { vlan: v });
                    }
                    Err(e) => log::warn!("link provisioning ({xfer:?}): {e} (daemon will retry)"),
                }
            }
            WorkerCmd::RestartLine => {
                log::info!("restart-line — bouncing DSL line");
                let _ = self.board.line_config_down();
                std::thread::sleep(constants::POLL_RESTART_DELAY);
                if let Err(e) = self.board.line_config_up(
                    self.cfg.modulation,
                    self.cfg.annex,
                    self.cfg.profiles,
                    self.cfg.bitswap,
                    self.cfg.sra,
                ) {
                    log::warn!("line_config_up after restart: {e}");
                }
            }
            WorkerCmd::LineDown => {
                self.board.set_retries(constants::BOARD_SHUTDOWN_RETRIES);
                let _ = self.board.line_config_down();
            }
            WorkerCmd::UpdateConfig(cfg) => {
                self.cfg = *cfg;
                self.publish_config();
            }
            WorkerCmd::FirmwareUpgrade { image, reply_tx } => {
                self.handle_firmware_upgrade(image, reply_tx);
            }
            WorkerCmd::Shutdown => unreachable!("handled in run()"),
        }
    }

    /// Publish cfg-derived display fields into shared state (callers read
    /// these instead of borrowing cfg, which lives on this thread).
    fn publish_config(&self) {
        let mut st = self.state.lock().unwrap();
        st.modulation = self.cfg.modulation;
        st.annex = self.cfg.annex;
    }

    fn handle_firmware_upgrade(
        &mut self,
        image: Vec<u8>,
        reply_tx: mpsc::Sender<FwEvent>,
    ) {
        {
            let mut st = self.state.lock().unwrap();
            st.fw_status = crate::ubus_obj::FwStatus::Upgrading;
        }

        log::info!("firmware upgrade: {} bytes", image.len());
        let result = self.board.firmware_upgrade(&image, &mut |p| {
            let _ = reply_tx.send(FwEvent::Progress(p.clone()));
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

    // ── status polling (the worker's only autonomous activity) ─────────

    fn maybe_poll_board(&mut self) {
        let due = self
            .last_poll
            .map_or(true, |t| t.elapsed() >= self.poll_interval);
        if !due {
            return;
        }
        self.last_poll = Some(Instant::now());

        match self.board.get_line_obj() {
            Ok(line) => {
                let status = line.link_status;
                let data_path = line.data_path;

                if !self.board_reachable {
                    self.board_reachable = true;
                    let _ = self.event_tx.send(WorkerEvent::BoardReachable);
                }

                if status == LinkStatus::Up && self.up_since.is_none() {
                    self.up_since = Some(Instant::now());
                } else if status != LinkStatus::Up {
                    self.up_since = None;
                }

                {
                    let mut st = self.state.lock().unwrap();
                    st.last_poll_ok = Some(Instant::now());
                    st.board_ever_seen = true;
                    st.line_obj = Some(line.clone());
                    st.uptime_secs = self.up_since.map(|t| t.elapsed().as_secs()).unwrap_or(0);
                }

                if Some(status) != self.last_status {
                    self.last_status = Some(status);
                    let _ = self.event_tx.send(WorkerEvent::LineState { status, data_path });
                }
            }
            Err(BoardError::Timeout) => {
                if self.board_reachable {
                    self.board_reachable = false;
                    let _ = self.event_tx.send(WorkerEvent::BoardUnreachable);
                }
                let mut st = self.state.lock().unwrap();
                let stale = st
                    .last_poll_ok
                    .map_or(true, |t| t.elapsed() > constants::POLL_STALE_TIMEOUT);
                if stale {
                    st.line_obj = None;
                }
            }
            Err(e) => log::error!("poll error: {e}"),
        }
    }
}
