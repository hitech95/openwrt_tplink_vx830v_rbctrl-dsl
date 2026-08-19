//! Daemon — the policy gate and control loop.
//!
//! ## Architecture
//!
//! ```text
//! Daemon (this module)                     BoardWorker (worker.rs)
//! ─────────────────────                    ─────────────────────────────
//! owns: policy state                       owns: board socket
//!   cfg / config_applied /                 does: op2 poll → WorkerEvent
//!   link_provisioned / vlan_owned …        does: execute WorkerCmd verbatim
//! gate: ALL board-operation decisions
//!   WorkerCmd  ─────────────────────────►  ApplyLineConfig / ProvisionLink /
//!   WorkerEvent ◄───────────────────────── RestartLine / LineDown / …
//! ```
//!
//! The worker makes no autonomous decisions. Monitor mode (`--monitor`) is
//! enforced here — the daemon simply never issues configuration commands.
//!
//! Submodules:
//! - [`init`] — startup phase (config, validation, socket)
//! - [`worker`] — board poll + command executor (the daemon's agent)
//! - [`ipc`] — control-socket server + wire protocol
//! - [`hotplug`] — DSL notify-script emission
//! - [`firmware`] — `FIRMWARE <path>` IPC streaming handler

mod firmware;
mod hotplug;
mod init;
mod ipc;
mod snapshot;
mod state;
mod ubus_object;
mod worker;

pub(crate) use ipc::SOCK_PATH;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Instant;

use rbctl_proto::oem::unpack::{DataPath, LinkStatus};

use crate::config::{CliOverrides, DslConfig, XferMode};
use crate::constants;
use crate::utils::{mapping, ubus as ubus_util, vlan};

pub(crate) use state::{new_shared_state, FwStatus, SharedState};

use hotplug::{self, LineEvent};
use ipc::{IpcAction, IpcListener, StatusSnapshot};
use worker::{BoardWorker, WorkerCmd, WorkerEvent};

// ── signal flags ─────────────────────────────────────────────────────────
//
// Read by the control loop below and by the worker (SHOULD_EXIT only);
// written by the signal handlers and the IPC Stop action. Plain
// `AtomicBool` is fine because we only ever treat them as level triggers
// (swap-to-clear).

pub(crate) static SHOULD_EXIT: AtomicBool = AtomicBool::new(false);
pub(crate) static SHOULD_RELOAD: AtomicBool = AtomicBool::new(false);
pub(crate) static SHOULD_RESTART_LINE: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_sigterm(_sig: libc::c_int) {
    SHOULD_EXIT.store(true, Ordering::SeqCst);
}

extern "C" fn handle_sighup(_sig: libc::c_int) {
    SHOULD_RELOAD.store(true, Ordering::SeqCst);
}

/// Install SIGTERM/SIGINT → exit flag, SIGHUP → reload flag.
fn install_signal_handlers() {
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = handle_sigterm as usize;
        libc::sigaction(libc::SIGTERM, &sa, std::ptr::null_mut());
        libc::sigaction(libc::SIGINT, &sa, std::ptr::null_mut());

        let mut sa_hup: libc::sigaction = std::mem::zeroed();
        sa_hup.sa_sigaction = handle_sighup as usize;
        libc::sigaction(libc::SIGHUP, &sa_hup, std::ptr::null_mut());
    }
}

use SHOULD_EXIT as EXIT;
use SHOULD_RELOAD as RELOAD;
use SHOULD_RESTART_LINE as RESTART;

// ── free helpers ─────────────────────────────────────────────────────────

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
        || old.transport_vlan_base != new.transport_vlan_base
}

// ── Daemon ───────────────────────────────────────────────────────────────

/// The daemon: owns the policy state and the control loop. Constructed via
/// [`Daemon::startup`], driven by [`Daemon::run`].
pub(crate) struct Daemon {
    // ── immutable after startup ──
    pub(super) config_iface: String,
    notify_script: Option<String>,
    overrides: CliOverrides,
    pub(super) monitor: bool,
    parent: String,
    pub(super) state: SharedState,
    ipc: Option<IpcListener>,
    cmd_tx: mpsc::Sender<WorkerCmd>,
    event_rx: mpsc::Receiver<WorkerEvent>,
    worker_done_rx: mpsc::Receiver<()>,
    ubus_conn: Option<ubus::server::UbusConnection<crate::utils::ubus_transport::UnixUbusTransport>>,

    // ── gate state (all board-operation decisions) ──
    cfg: DslConfig,
    /// op1 acknowledged by the board.
    config_applied: bool,
    /// op5/op15 acknowledged by the board.
    link_provisioned: bool,
    /// Board-assigned transport VLAN (authoritative for cleanup).
    provisioned_vlan: Option<u16>,
    /// True when this daemon instance created the host-side VLAN — only
    /// then is it deleted at shutdown.
    vlan_owned: bool,
    /// Throttle for deferred op1 retries (board-reachable transitions).
    last_apply: Option<Instant>,
    /// One TC-layer hotplug emission per Up session.
    tc_emitted: bool,
}

impl Daemon {
    /// Run the startup phase and construct the daemon. The worker thread is
    /// already spawned and the initial commands queued when this returns.
    /// Returns `Err(exit_code)` on fatal startup errors.
    pub(crate) fn startup(
        config_iface: &str,
        notify_script: Option<&str>,
        overrides: &CliOverrides,
        monitor: bool,
    ) -> Result<Self, i32> {
        let init = match init::startup(config_iface, overrides) {
            Ok(i) => i,
            Err(code) => return Err(code),
        };
        let init::Init { board, cfg, parent, .. } = init;

        // Bind IPC socket
        let ipc = match IpcListener::bind(ipc::SOCK_PATH) {
            Ok(l) => {
                log::info!("IPC socket at {}", ipc::SOCK_PATH);
                Some(l)
            }
            Err(e) => {
                log::warn!("IPC bind failed: {e} — control socket disabled");
                None
            }
        };

        // Shared state + ubus object
        let state = new_shared_state();
        {
            let mut st = state.lock().unwrap();
            st.xfer_mode = Some(cfg.xfer_mode);
            st.modulation = cfg.modulation;
            st.annex = cfg.annex;
        }
        let ubus_obj_handle = ubus_object::build_dsl_object(Arc::clone(&state));
        let ubus_conn = match ubus_util::connect_and_register(ubus_obj_handle) {
            Some(c) => {
                log::info!("ubus object 'dsl' registered");
                Some(c)
            }
            None => {
                log::warn!("ubus unavailable — metrics will not be published");
                None
            }
        };

        // Spawn the board worker (poll-only executor).
        let (cmd_tx, cmd_rx) = mpsc::channel::<WorkerCmd>();
        let (event_tx, event_rx) = mpsc::channel::<WorkerEvent>();
        let worker = BoardWorker::new(board, cfg.clone(), Arc::clone(&state), cmd_rx, event_tx);
        let (worker_done_tx, worker_done_rx) = mpsc::channel::<()>();
        std::thread::spawn(move || {
            worker.run();
            let _ = worker_done_tx.send(());
        });

        let mut daemon = Self {
            config_iface: config_iface.to_string(),
            notify_script: notify_script.map(str::to_string),
            overrides: overrides.clone(),
            monitor,
            parent,
            state,
            ipc,
            cmd_tx,
            event_rx,
            worker_done_rx,
            ubus_conn,
            cfg,
            config_applied: false,
            link_provisioned: false,
            provisioned_vlan: None,
            vlan_owned: false,
            last_apply: None,
            tc_emitted: false,
        };

        // Initial configuration — the only place startup config is decided.
        if daemon.monitor {
            log::info!("monitor mode: observing the board, no configuration will be sent");
        } else {
            let _ = daemon.cmd_tx.send(WorkerCmd::ApplyLineConfig);
            if let XferMode::Ptm | XferMode::Atm = daemon.cfg.xfer_mode {
                let _ = daemon.cmd_tx.send(WorkerCmd::ProvisionLink {
                    xfer: daemon.cfg.xfer_mode,
                    vlan: daemon.transport_vlan(),
                });
            }
            // Emit initial TC-layer status (monitor mode has no configured
            // mode to report; it emits on Up transitions instead).
            if let Some(script) = daemon.notify_script.as_deref() {
                hotplug::emit_tc_layer(script, daemon.cfg.xfer_mode);
            }
        }

        Ok(daemon)
    }

    /// The control loop. Returns the process exit code.
    pub(crate) fn run(mut self) -> i32 {
        log::info!("entering control loop");
        // Exit is deferred while a firmware upgrade is in flight: killing
        // the worker mid-flash would brick the board. Signals set EXIT from
        // the handler (which can't take the state mutex), so the flag is
        // re-checked here every tick — the loop keeps serving IPC/ubus
        // until the flash completes, then exits.
        let mut exit_deferred_logged = false;
        loop {
            if EXIT.load(Ordering::SeqCst) {
                if self.flashing() {
                    if !exit_deferred_logged {
                        log::warn!("shutdown deferred: firmware upgrade in progress");
                        exit_deferred_logged = true;
                    }
                } else {
                    break;
                }
            }

            // D10: detect board-worker exit (panic, early return, or channel
            // close). The worker owns the board socket; without it the daemon
            // can only zombie-run and silently drop control commands — shut
            // down instead.
            match self.worker_done_rx.try_recv() {
                Ok(()) | Err(mpsc::TryRecvError::Disconnected) => {
                    if !EXIT.load(Ordering::SeqCst) {
                        log::error!("board worker exited unexpectedly — shutting down");
                        EXIT.store(true, Ordering::SeqCst);
                    }
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }

            // Worker events → policy decisions.
            while let Ok(ev) = self.event_rx.try_recv() {
                self.on_worker_event(ev);
            }

            // Signals → commands.
            if RELOAD.swap(false, Ordering::SeqCst) {
                self.on_reload();
            }
            if RESTART.swap(false, Ordering::SeqCst) {
                self.on_restart();
            }

            // IPC commands.
            if let Some(ipc) = &self.ipc {
                let snap = self.build_snapshot();
                match ipc.accept_one(&snap) {
                    Ok(Some(action)) => self.on_ipc(action),
                    Ok(None) => {} // no client
                    Err(e) => log::error!("IPC error: {e}"),
                }
            }

            // ubus (non-blocking).
            if let Some(conn) = self.ubus_conn.as_mut() {
                let _ = conn.poll_one();
            }

            std::thread::sleep(constants::POLL_TICK);
        }

        self.shutdown();
        0
    }

    // ── queries ──────────────────────────────────────────────────────────

    /// Requested transport VLAN id — computed here, never in the worker
    /// (the worker only owns the management transport).
    fn transport_vlan(&self) -> u16 {
        self.cfg.transport_vlan_base as u16 + constants::VLAN_TRANSPORT_BASE_OFFSET
    }

    /// Whether a firmware upgrade is in progress (checked under the state
    /// mutex — the same lock the worker uses to publish progress).
    fn flashing(&self) -> bool {
        let st = self.state.lock().unwrap();
        matches!(
            st.fw_status,
            FwStatus::Upgrading | FwStatus::UpgradingProgress { .. }
        )
    }

    }

    // ── event handlers (the gate) ────────────────────────────────────────

    fn on_worker_event(&mut self, ev: WorkerEvent) {
        match ev {
            WorkerEvent::BoardReachable => {
                // Deferred config: op1 failed at startup because the board
                // was unreachable — retry now (throttled to the poll cadence
                // so a failing link_add doesn't re-run op1 every tick).
                if !self.monitor && !self.config_applied {
                    let due = self
                        .last_apply
                        .map_or(true, |t| t.elapsed() >= constants::POLL_INTERVAL);
                    if due {
                        self.last_apply = Some(Instant::now());
                        log::info!("board reachable — applying deferred config");
                        let _ = self.cmd_tx.send(WorkerCmd::ApplyLineConfig);
                    }
                }
            }
            WorkerEvent::BoardUnreachable => {
                log::warn!("board became unreachable");
                self.config_applied = false;
                self.link_provisioned = false;
                self.vlan_owned = false;
            }
            WorkerEvent::LineState { status, data_path } => {
                let event = link_status_to_event(status);
                log::info!("line state: {status:?} → {event:?}");
                if let Some(script) = self.notify_script.as_deref() {
                    hotplug::emit_status(script, event);
                }
                if status == LinkStatus::Up {
                    self.on_line_up(data_path);
                }
            }
            WorkerEvent::LineConfigApplied => {
                self.config_applied = true;
                // Fixed-mode deferred retry: provisioning follows config.
                if !self.monitor && !self.link_provisioned {
                    if let XferMode::Ptm | XferMode::Atm = self.cfg.xfer_mode {
                        let _ = self.cmd_tx.send(WorkerCmd::ProvisionLink {
                            xfer: self.cfg.xfer_mode,
                            vlan: self.transport_vlan(),
                        });
                    }
                }
            }
            WorkerEvent::LinkProvisioned { vlan } => {
                self.link_provisioned = true;
                self.provisioned_vlan = Some(vlan);
                log::info!("board assigned transport vlan: {vlan}");
                // Track whether WE created the host-side interface, so
                // shutdown only deletes what it owns (a pre-existing VLAN
                // from an earlier daemon run is left alone).
                match vlan::create_vlan_iface(&self.parent, vlan) {
                    Ok(true) => {
                        self.vlan_owned = true;
                        log::info!("created {}.{}", self.parent, vlan);
                    }
                    Ok(false) => {
                        log::info!("{}.{} already exists — reusing", self.parent, vlan)
                    }
                    Err(e) => log::warn!("VLAN create {}.{}: {e}", self.parent, vlan),
                }
            }
        }
    }

    /// Line reached Up — record the observed TC mode, emit hotplug, and
    /// trigger auto provisioning when appropriate.
    fn on_line_up(&mut self, data_path: DataPath) {
        // Observed mode from op2 — authoritative for monitor display and
        // auto provisioning.
        let observed = match data_path {
            DataPath::Ptm => XferMode::Ptm,
            DataPath::Atm => XferMode::Atm,
            DataPath::Unknown(_) => self.cfg.xfer_mode,
        };
        if self.monitor || self.cfg.xfer_mode == XferMode::Auto {
            let mut st = self.state.lock().unwrap();
            st.xfer_mode = Some(observed);
        }
        if !self.tc_emitted {
            let emit_mode = if self.monitor || self.cfg.xfer_mode == XferMode::Auto {
                observed
            } else {
                self.cfg.xfer_mode
            };
            if let Some(script) = self.notify_script.as_deref() {
                hotplug::emit_tc_layer(script, emit_mode);
            }
            self.tc_emitted = true;
        }
        // Auto mode: provision the link now that the actual TC mode is
        // known.
        if !self.monitor && self.cfg.xfer_mode == XferMode::Auto && !self.link_provisioned {
            let _ = self.cmd_tx.send(WorkerCmd::ProvisionLink {
                xfer: observed,
                vlan: self.transport_vlan(),
            });
        }
    }

    // ── signal / IPC handlers ────────────────────────────────────────────

    fn on_reload(&mut self) {
        log::info!("reload — re-reading UCI config");
        let Ok(new_cfg) = DslConfig::load(&self.overrides) else {
            return;
        };
        let changed = config_changed(&self.cfg, &new_cfg);
        self.cfg = new_cfg;
        // Always refresh the worker's display copy.
        let _ = self.cmd_tx.send(WorkerCmd::UpdateConfig(Box::new(self.cfg.clone())));
        if self.monitor {
            log::info!("monitor mode: only refreshing display config");
            return;
        }
        if changed {
            log::info!("line params changed — reconfiguring");
            self.config_applied = false;
            self.link_provisioned = false;
            self.tc_emitted = false;
            let _ = self.cmd_tx.send(WorkerCmd::LineDown);
            let _ = self.cmd_tx.send(WorkerCmd::ApplyLineConfig);
            if let XferMode::Ptm | XferMode::Atm = self.cfg.xfer_mode {
                let _ = self.cmd_tx.send(WorkerCmd::ProvisionLink {
                    xfer: self.cfg.xfer_mode,
                    vlan: self.transport_vlan(),
                });
            }
        } else {
            log::info!("no line-param changes");
        }
    }

    fn on_restart(&mut self) {
        if self.monitor {
            log::warn!("restart-line refused in monitor mode");
            return;
        }
        let _ = self.cmd_tx.send(WorkerCmd::RestartLine);
        self.tc_emitted = false;
    }

    fn on_ipc(&mut self, action: IpcAction) {
        match action {
            IpcAction::Reload => self.on_reload(),
            IpcAction::RestartLine => self.on_restart(),
            IpcAction::Stop => {
                // Guard: killing the worker mid-flash would brick the
                // board. The IPC client sees the daemon stay alive; a
                // second attempt after the flash succeeds.
                if self.flashing() {
                    log::warn!("stop refused: firmware upgrade in progress");
                } else {
                    EXIT.store(true, Ordering::SeqCst);
                }
            }
            IpcAction::FirmwareUpgrade { path, stream } => {
                // Spawn a dedicated thread so the main loop keeps serving
                // ubus and other IPC clients during the (potentially
                // multi-minute) upload.
                let cmd_tx = self.cmd_tx.clone();
                let state = Arc::clone(&self.state);
                std::thread::spawn(move || {
                    let mut stream = stream;
                    firmware::handle_firmware_ipc(&path, &mut stream, &cmd_tx, &state);
                });
            }
            IpcAction::None => {}
        }
    }

    // ── shutdown ─────────────────────────────────────────────────────────

    fn shutdown(&mut self) {
        log::info!("shutting down");
        if let Some(script) = self.notify_script.as_deref() {
            hotplug::emit_status(script, LineEvent::Down);
        }
        // Teardown through the gate: monitor mode never configured the
        // board, so it must not tear it down either.
        if !self.monitor {
            let _ = self.cmd_tx.send(WorkerCmd::LineDown);
        }
        let _ = self.cmd_tx.send(WorkerCmd::Shutdown);
        match self.worker_done_rx.recv_timeout(constants::DAEMON_SHUTDOWN_DEADLINE) {
            Ok(()) => log::info!("board worker exited cleanly"),
            Err(_) => log::warn!(
                "board worker didn't exit within {:?}; abandoning (process exit reaps it)",
                constants::DAEMON_SHUTDOWN_DEADLINE
            ),
        }
        // Delete only the VLAN we created; a pre-existing one (earlier
        // daemon run, operator-created) is left in place.
        if !self.monitor && self.vlan_owned {
            if let Some(vlan) = self.provisioned_vlan {
                vlan::delete_vlan_iface(&self.parent, vlan);
            }
        }
        let _ = std::fs::remove_file(ipc::SOCK_PATH);
    }
}

// ── entry point ──────────────────────────────────────────────────────────

/// Run the daemon. Returns process exit code.
pub fn run(
    config_iface: &str,
    notify_script: Option<&str>,
    overrides: &CliOverrides,
    monitor: bool,
) -> i32 {
    install_signal_handlers();

    let daemon = match Daemon::startup(config_iface, notify_script, overrides, monitor) {
        Ok(d) => d,
        Err(code) => return code,
    };
    daemon.run()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rbctl_proto::oem::pack::Annex;

    #[test]
    fn link_status_events() {
        assert_eq!(link_status_to_event(LinkStatus::Up), LineEvent::Up);
        assert_eq!(link_status_to_event(LinkStatus::NoSignal), LineEvent::Down);
        assert_eq!(
            link_status_to_event(LinkStatus::Initializing),
            LineEvent::Training
        );
        assert_eq!(
            link_status_to_event(LinkStatus::EstablishingLink),
            LineEvent::Handshake
        );
    }

    #[test]
    fn config_change_detection() {
        let a = DslConfig::default();
        let b = DslConfig {
            annex: Annex::A,
            ..DslConfig::default()
        };
        assert!(config_changed(&a, &b));
    }

    #[test]
    fn config_no_change() {
        let a = DslConfig::default();
        let b = DslConfig::default();
        assert!(!config_changed(&a, &b));
    }
}
