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
//!   WorkerCmd  ─────────────────────────►  OpenLine / ApplyLineConfig /
//!   WorkerEvent ◄───────────────────────── ProvisionLink / RestartLine /
//!                                           LineDown / …
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

pub(crate) mod firmware;
pub(crate) mod hotplug;
mod init;
pub(crate) mod ipc;
mod snapshot;
pub(crate) mod state;
mod ubus_object;
pub(crate) mod worker;

pub(crate) use ipc::SOCK_PATH;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Instant;

use rbctl_proto::oem::pack::Modulation;
use rbctl_proto::oem::unpack::{DataPath, LineState};

use crate::config::{CliOverrides, DslConfig, XferMode};
use crate::constants;
use crate::utils::{ubus as ubus_util, vlan};

pub(crate) use state::{new_shared_state, FwStatus, SharedState};

use hotplug::LineEvent;
use ipc::{IpcAction, IpcListener};
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

// ── single-instance guard ────────────────────────────────────────────────

/// Exclusive `flock` on [`constants::DAEMON_LOCK_PATH`] — held for the
/// daemon's lifetime.
///
/// The kernel releases the lock when the process dies (any exit path,
/// including SIGKILL), so there is no stale-pidfile race to handle. This
/// prevents a second daemon from running concurrently: a second instance
/// would steal the IPC socket (`bind()` unlinks the stale path), collide
/// with the first on the ubus `dsl` object (ADD_OBJECT → duplicate path →
/// `Status(2)`), and double-configure the board (op1/op3/op15).
struct PidLock {
    fd: std::os::unix::io::OwnedFd,
}

impl PidLock {
    /// Acquire the singleton lock. `Err` if another daemon holds it.
    fn acquire(path: &str) -> std::io::Result<Self> {
        use std::os::unix::io::AsRawFd;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)?;
        match unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } {
            0 => {
                // Best-effort pid content for diagnostics; the lock itself
                // is the source of truth, never the file contents.
                let _ = std::fs::write(path, format!("{}\n", std::process::id()));
                Ok(Self { fd: file.into() })
            }
            _ if std::io::Error::last_os_error().kind() == std::io::ErrorKind::WouldBlock => {
                let holder = std::fs::read_to_string(path)
                    .ok()
                    .and_then(|s| s.trim().parse::<i32>().ok())
                    .map(|pid| format!(" (pid {pid}?)"))
                    .unwrap_or_default();
                Err(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    format!("another rbctl-dsl instance is running{holder}"),
                ))
            }
            _ => Err(std::io::Error::last_os_error()),
        }
    }
}

// ── free helpers ─────────────────────────────────────────────────────────

fn link_status_to_event(status: LineState) -> LineEvent {
    match status {
        LineState::ShowtimeTcSync | LineState::ShowtimeNoSync => LineEvent::Up,
        LineState::FullInit => LineEvent::Training,
        LineState::Handshake => LineEvent::Handshake,
        LineState::Silent | LineState::Unknown(_) => LineEvent::Down,
        LineState::NotInitialized | LineState::Idle => LineEvent::Down,
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
    /// Single-instance flock — held for the daemon's lifetime.
    _pid_lock: PidLock,
    cmd_tx: mpsc::Sender<WorkerCmd>,
    event_rx: mpsc::Receiver<WorkerEvent>,
    worker_done_rx: mpsc::Receiver<()>,
    ubus_conn: Option<ubus::server::UbusConnection<crate::utils::ubus_transport::UnixUbusTransport>>,

    // ── gate state (all board-operation decisions) ──
    cfg: DslConfig,
    /// op1 acknowledged by the board.
    config_applied: bool,
    /// op5/op15 acknowledged by the board, per transport VLAN
    /// (multi-link: one entry per configured data link).
    links_provisioned: std::collections::BTreeSet<u16>,
    /// Host-side VLANs this daemon instance created — only those are
    /// deleted at shutdown (pre-existing interfaces are left alone).
    vlans_owned: std::collections::BTreeSet<u16>,
    /// Provisioning queued at startup (queue-at-open): true until every
    /// configured link has been sent while the line was down.
    startup_provisioning_sent: bool,
    /// Last-applied data-link set (reload diffing).
    links_snapshot: Vec<crate::config::DataLink>,
    /// F4 auto-correction bookkeeping: transport vlan → the type the
    /// link was LAST swapped to (drives re-swaps on family re-trains;
    /// absent = still the configured/queued type).
    links_auto_corrected: std::collections::BTreeMap<u16, XferMode>,
    /// Throttle for deferred op1 retries (board-reachable transitions).
    last_apply: Option<Instant>,
    /// One TC-layer hotplug emission per Up session.
    tc_emitted: bool,
    /// When the line has been continuously in NoSignal (watchdog dwell).
    nosignal_since: Option<Instant>,
    /// Whether the current silent episode already logged its warning
    /// (one warning per episode; any non-NoSignal state resets both).
    nosignal_warned: bool,
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
        // 0. Single-instance guard — before touching the board, the IPC
        //    socket, or ubus. A concurrent instance would steal the control
        //    socket, collide on the ubus 'dsl' object (duplicate path), and
        //    double-configure the line. Hard-fail, never degrade.
        let _pid_lock = match PidLock::acquire(constants::DAEMON_LOCK_PATH) {
            Ok(l) => l,
            Err(e) => {
                log::error!("singleton lock: {e}");
                return Err(1);
            }
        };

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

        let links_snapshot = cfg.data_links();
        let mut daemon = Self {
            config_iface: config_iface.to_string(),
            notify_script: notify_script.map(str::to_string),
            overrides: overrides.clone(),
            monitor,
            parent,
            state,
            ipc,
            _pid_lock,
            cmd_tx,
            event_rx,
            worker_done_rx,
            ubus_conn,
            cfg,
            config_applied: false,
            links_provisioned: Default::default(),
            vlans_owned: Default::default(),
            startup_provisioning_sent: false,
            links_snapshot,
            links_auto_corrected: Default::default(),
            last_apply: None,
            tc_emitted: false,
            nosignal_since: None,
            nosignal_warned: false,
        };

        // Initial configuration — the only place startup config is decided.
        if daemon.monitor {
            log::info!("monitor mode: observing the board, no configuration will be sent");
        } else {
            // Guaranteed line open: the board dedups an op1 identical to
            // its config cache and the cache is unreadable, so a single
            // op1 cannot reliably (re)open a line — e.g. one left closed
            // by a previous daemon's shutdown op3 with an unchanged
            // config. OpenLine's double apply guarantees the final op1
            // executes the full path ending in `wan adsl open`.
            //
            // First, startup hygiene: a previous daemon (crash) or the
            // selftest may have left a data link on the transport VLAN —
            // possibly of the wrong type for the line about to train.
            // Clear it before opening; provisioning re-runs at line-Up
            // with the type derived from the trained modulation.
            for link in daemon.cfg.data_links() {
                let _ = daemon.cmd_tx.send(WorkerCmd::CleanupLink {
                    vlan: link.transport_vlan(),
                });
            }
            let _ = daemon.cmd_tx.send(WorkerCmd::OpenLine);
            // Queue the data links NOW, while the line is down: the
            // board materializes them at the up-TRANSITION (the OEM
            // flow, session-4 verified). Post-up provisioning is only
            // used for corrections/reloads (with a warning).
            daemon.provision_links_at_open();
            daemon.startup_provisioning_sent = true;
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

            // NoSignal watchdog — recover a line that trains nowhere.
            self.watchdog_tick();

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

    // ── event handlers (the gate) ────────────────────────────────────────

    /// NoSignal watchdog — detection and diagnostics only, never action.
    ///
    /// An *open* modem retries training on its own (field pcaps show
    /// minutes of NoSignal↔Initializing cycling with zero host traffic),
    /// so re-applying the config while the modem is handshaking is pure
    /// noise, and op3 (`wan adsl close`) would kill the auto-retry.
    /// Recovery paths elsewhere already cover the actionable cases:
    ///
    /// - board unreachable → reachable: `BoardReachable` re-opens the
    ///   line automatically (and a rebooted board has self-opened);
    /// - line closed by a foreign tool or an old daemon: the operator
    ///   bounces it explicitly.
    ///
    /// So after [`constants::NOSIGNAL_WATCHDOG`] (30 s) of *continuous*
    /// NoSignal with config applied and the board reachable, log exactly
    /// one warning per silent episode — continuous NoSignal with no
    /// cycling is the signature of a closed modem or no cable, both
    /// outside the daemon's authority. `Initializing`/`EstablishingLink`
    /// mean training is in progress and reset the dwell.
    fn watchdog_tick(&mut self) {
        if self.monitor || !self.config_applied || self.flashing() {
            self.nosignal_since = None;
            self.nosignal_warned = false;
            return;
        }
        let status = self
            .state
            .lock()
            .unwrap()
            .line_obj
            .as_ref()
            .map(|l| l.link_status);
        match status {
            Some(LineState::Silent) if !self.nosignal_warned => {
                let since = *self.nosignal_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= constants::NOSIGNAL_WATCHDOG {
                    log::warn!(
                        "line silent (NoSignal) for {:?} with no training attempts — \
                         modem closed or no cable; manual bounce: rbctl-dsl restart-line",
                        since.elapsed()
                    );
                    self.nosignal_warned = true;
                }
            }
            // Training in progress, Up, or no fresh poll — reset the episode.
            _ => {
                self.nosignal_since = None;
                self.nosignal_warned = false;
            }
        }
    }

    fn on_worker_event(&mut self, ev: WorkerEvent) {
        match ev {
            WorkerEvent::BoardReachable => {
                // Deferred open: the startup open failed because the board
                // was unreachable — retry now (throttled to the poll
                // cadence so a failing link_add doesn't re-run op1 every
                // tick). OpenLine, not ApplyLineConfig: the board may have
                // rebooted while unreachable, so its cache state is
                // unknown and the open must be guaranteed.
                if !self.monitor && !self.config_applied {
                    let due = self
                        .last_apply
                        .map_or(true, |t| t.elapsed() >= constants::POLL_INTERVAL);
                    if due {
                        self.last_apply = Some(Instant::now());
                        log::info!("board reachable — opening line (deferred)");
                        let _ = self.cmd_tx.send(WorkerCmd::OpenLine);
                    }
                }
            }
            WorkerEvent::BoardUnreachable => {
                log::warn!("board became unreachable");
                self.config_applied = false;
                self.links_provisioned.clear();
                self.vlans_owned.clear();
                // Keep links_auto_corrected: the board (if it comes back)
                // still holds the swapped link type, not the config one.
            }
            WorkerEvent::LineState { status, data_path, modulation } => {
                let event = link_status_to_event(status);
                log::info!("line state: {status:?} → {event:?}");
                if let Some(script) = self.notify_script.as_deref() {
                    hotplug::emit_status(script, event);
                }
                if status.is_up() {
                    self.on_line_up(data_path, modulation);
                }
            }
            WorkerEvent::LineConfigApplied => {
                self.config_applied = true;
                // Data-plane provisioning deliberately does NOT run here:
                // the link type is only known once the line trains, so it
                // is derived from the trained modulation at line-Up
                // (on_line_up) for every mode — fixed and Auto alike.
            }
            WorkerEvent::LinkProvisioned { vlan } => {
                self.links_provisioned.insert(vlan);
                log::info!("board assigned transport vlan: {vlan}");
                // Track whether WE created the host-side interface, so
                // shutdown only deletes what it owns (a pre-existing VLAN
                // from an earlier daemon run is left alone).
                match vlan::create_vlan_iface(&self.parent, vlan) {
                    Ok(true) => {
                        self.vlans_owned.insert(vlan);
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

    /// Line reached Up — resolve the data-link type and provision.
    ///
    /// Resolution order:
    ///
    /// 1. **Explicit `xfer_mode`** (operator override) is honored
    ///    as-is — including unusual-but-legal combos like ATM on VDSL2.
    ///    Only physically impossible combinations are blocked (PTM on a
    ///    trained ADSL standard; see [`XferMode::compatible_with`] —
    ///    future G.fast would be PTM-only). A block is an error + no
    ///    provisioning.
    /// 2. **Auto** derives from the **trained modulation**
    ///    (VDSL2→PTM, ADSL→ATM). The op2 `data_path` field is NOT
    ///    authoritative — it is a power-on default that does not follow
    ///    training (verified on-target: read `Atm` while a VDSL2 line
    ///    was in showtime); it serves only as the fallback for an
    ///    unknown modulation code, and a disagreement is logged.
    /// Queue every configured data link while the line is DOWN — the
    /// board materializes each at the up-TRANSITION (OEM flow,
    /// session-4 verified on-target). Call before/at OpenLine.
    fn provision_links_at_open(&self) {
        for link in self.cfg.data_links() {
            let (tag_en, tag_vid, tag_pbit) = match link.bearer_tag {
                Some((vid, pbit)) => (1u8, vid, pbit as u16),
                None => (0u8, 0u16, 0u16),
            };
            let _ = self.cmd_tx.send(WorkerCmd::ProvisionLink {
                xfer: link.xfer,
                vlan: link.transport_vlan(),
                tag_enable: tag_en,
                tag_vid,
                tag_pbit,
                atm: link.atm.clone(),
            });
        }
    }

    /// Provision (or correct) links AFTER the line is up. This relies on
    /// the board's direct materialization path — verified on hardware,
    /// but historically fragile (the ':' parse); warn (F3).
    fn provision_link_post_up(
        &mut self,
        link: &crate::config::DataLink,
        reason: &str,
    ) {
        if self.monitor {
            return;
        }
        log::warn!(
            "provisioning data link '{}' (vlan {}) while line is up ({reason}): \
             the econet firmware's direct path may hollow-ACK this request — \
             if the link does not pass data, restart the line (rbctl-dsl restart-line)",
            link.name,
            link.transport_vlan()
        );
        let (tag_en, tag_vid, tag_pbit) = match link.bearer_tag {
            Some((vid, pbit)) => (1u8, vid, pbit as u16),
            None => (0u8, 0u16, 0u16),
        };
        let _ = self.cmd_tx.send(WorkerCmd::ProvisionLink {
            xfer: link.xfer,
            vlan: link.transport_vlan(),
            tag_enable: tag_en,
            tag_vid,
            tag_pbit,
            atm: link.atm.clone(),
        });
    }

    /// Line reached Up: the startup links were queued at open and are
    /// materialized by the board's up-transition handler. What remains
    /// here: auto-mode corrections (F4), unreachable-reset re-provision,
    /// state/hotplug publication.
    fn on_line_up(&mut self, data_path: DataPath, modulation: Option<Modulation>) {
        // Trained type (authoritative): from the trained modulation when
        // known, else the op2 field (stale — warn if it disagrees).
        let trained = modulation.map(XferMode::for_modulation).or_else(|| {
            match data_path {
                DataPath::Ptm => Some(XferMode::Ptm),
                DataPath::Atm => Some(XferMode::Atm),
                DataPath::Unknown(code) => {
                    log::warn!(
                        "line up with unknown modulation and data_path code {code}"
                    );
                    None
                }
            }
        });
        if let (Some(m), Some(t)) = (modulation, trained) {
            let field = match data_path {
                DataPath::Ptm => Some(XferMode::Ptm),
                DataPath::Atm => Some(XferMode::Atm),
                DataPath::Unknown(_) => None,
            };
            if field.is_some_and(|f| f != t) {
                log::warn!(
                    "op2 data_path field says {data_path:?} but trained modulation \
                     is {m:?} — using {t:?} (field does not follow training)"
                );
            }
        }

        if let Some(t) = trained {
            let mut st = self.state.lock().unwrap();
            st.xfer_mode = Some(t);
        }
        if !self.tc_emitted {
            let emit = trained.unwrap_or(self.cfg.xfer_mode);
            if let Some(script) = self.notify_script.as_deref() {
                hotplug::emit_tc_layer(script, emit);
            }
            self.tc_emitted = true;
        }

        // F4 — auto-mode type correction. The implicit default link of an
        // `xfer auto` config is queued at open as optimistic PTM; if the
        // line trains ADSL, swap it to ATM via the post-up path (warned,
        // once per trained type). NOTE: LinkProvisioned fires on the
        // hollow queue-time ack, so links_provisioned CANNOT gate this —
        // a dedicated corrected-type map does (and allows a re-swap when
        // a mixed-mode line re-trains as the other family). Explicitly
        // typed links (sections, forced xfer) are never silently
        // retyped: they keep the physical-compatibility error.
        if let Some(trained) = trained {
            let optimistic = !self.monitor
                && self.cfg.xfer_mode == XferMode::Auto
                && self.cfg.links.is_empty();
            for link in self.cfg.data_links() {
                let vlan = link.transport_vlan();
                // Current board-side type: the corrected one if a swap
                // already happened, else the configured/queued one.
                let current = self
                    .links_auto_corrected
                    .get(&vlan)
                    .copied()
                    .unwrap_or(link.xfer);
                if current.compatible_with_trained(trained) {
                    continue;
                }
                if optimistic {
                    log::warn!(
                        "auto correction: the default link is {:?} but the line \
                         trained {:?} — swapping (post-up path)",
                        current, trained
                    );
                    let _ = self.cmd_tx.send(WorkerCmd::RemoveLink {
                        xfer: current,
                        vlan,
                    });
                    let corrected = crate::config::DataLink {
                        xfer: trained,
                        atm: if trained == XferMode::Atm {
                            // worker falls back to atm_params defaults
                            // (vpi/vci from constants) when None
                            None
                        } else {
                            link.atm.clone()
                        },
                        ..link.clone()
                    };
                    self.provision_link_post_up(&corrected, "auto type correction");
                    self.links_auto_corrected.insert(vlan, trained);
                } else {
                    log::error!(
                        "data link '{}' is {:?} but the line trained {:?} — \
                         not provisioning (fix the link type / line mode in config)",
                        link.name, current, trained
                    );
                }
            }
        }

        // Board came back reachable with the line still up and links
        // unprovisioned (unreachable reset them): re-provision post-up.
        if !self.monitor && !self.startup_provisioning_sent {
            for link in self.cfg.data_links() {
                let vlan = link.transport_vlan();
                if !self.links_provisioned.contains(&vlan) {
                    self.provision_link_post_up(&link, "board returned reachable");
                }
            }
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
            // Per-link diff: remove links that disappeared or changed,
            // queue new links for the retrain's up-transition.
            let new_links_vec = self.cfg.data_links();
            let new_links: std::collections::BTreeMap<u16, &crate::config::DataLink> =
                new_links_vec.iter().map(|l| (l.transport_vlan(), l)).collect();
            let old_vlans: Vec<u16> = self.links_provisioned.iter().copied().collect();
            for vlan in old_vlans {
                match new_links.get(&vlan) {
                    None => {
                        log::info!("reload: link on vlan {vlan} removed");
                        let _ = self.cmd_tx.send(WorkerCmd::RemoveLink { xfer: XferMode::Ptm, vlan });
                        let _ = self.cmd_tx.send(WorkerCmd::RemoveLink { xfer: XferMode::Atm, vlan });
                        self.links_provisioned.remove(&vlan);
                    }
                    Some(l) => {
                        // type/tag change on the same index: swap
                        let prev = self
                            .links_snapshot
                            .iter()
                            .find(|p| p.transport_vlan() == vlan)
                            .cloned();
                        if prev.is_some_and(|p| p != **l) {
                            log::info!("reload: link on vlan {vlan} changed — swapping");
                            let _ = self.cmd_tx.send(WorkerCmd::RemoveLink { xfer: XferMode::Ptm, vlan });
                            let _ = self.cmd_tx.send(WorkerCmd::RemoveLink { xfer: XferMode::Atm, vlan });
                            self.links_provisioned.remove(&vlan);
                        }
                    }
                }
            }
            self.links_snapshot = self.cfg.data_links();
            // The re-queue below re-establishes the CONFIG types — old
            // auto-corrections are stale (on_line_up re-corrects if the
            // trained line contradicts the new config).
            self.links_auto_corrected.clear();
            self.config_applied = false;
            self.tc_emitted = false;
            // A changed config differs from the board cache by
            // construction, so a plain op1 always takes the full apply
            // path (setters + `wan adsl reset` inside them + final
            // `wan adsl open`) — no deliberate line-down needed. The line
            // retrains; removed links are gone, new links queued below
            // materialize at the new up-transition.
            let _ = self.cmd_tx.send(WorkerCmd::ApplyLineConfig);
            // Queue all still-missing links at (presumably) line-down —
            // if the line happens to be up these become post-up adds.
            self.provision_links_at_open();
            self.startup_provisioning_sent = true;
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
            // Deprovision the board side first (both del opcodes per vlan:
            // we may hold queued-not-materialized nodes of either type),
            // then the line.
            for vlan in self.links_provisioned.iter().copied().collect::<Vec<_>>() {
                let _ = self.cmd_tx.send(WorkerCmd::RemoveLink { xfer: XferMode::Ptm, vlan });
                let _ = self.cmd_tx.send(WorkerCmd::RemoveLink { xfer: XferMode::Atm, vlan });
            }
            let _ = self.cmd_tx.send(WorkerCmd::LineDown);
            // Deliberate final state: daemon stopped ⇒ line is DOWN (op3
            // `wan adsl close`). Starting the daemon again re-opens it via
            // the guaranteed double apply at startup.
            log::info!("line left DOWN by request — daemon stopped");
        }
        let _ = self.cmd_tx.send(WorkerCmd::Shutdown);
        match self.worker_done_rx.recv_timeout(constants::DAEMON_SHUTDOWN_DEADLINE) {
            Ok(()) => log::info!("board worker exited cleanly"),
            Err(_) => log::warn!(
                "board worker didn't exit within {:?}; abandoning (process exit reaps it)",
                constants::DAEMON_SHUTDOWN_DEADLINE
            ),
        }
        // Delete only the VLANs we created; pre-existing ones (earlier
        // daemon run, operator-created) are left in place.
        if !self.monitor {
            for vlan in self.vlans_owned.iter().copied().collect::<Vec<_>>() {
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
    fn pid_lock_is_exclusive() {
        let dir = std::env::temp_dir().join(format!("rbctl-lock-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("daemon.lock");
        let _ = std::fs::remove_file(&path);

        let first = PidLock::acquire(path.to_str().unwrap()).unwrap();
        let second = PidLock::acquire(path.to_str().unwrap());
        assert!(second.is_err(), "second acquire must fail");
        let msg = second.err().unwrap().to_string();
        assert!(msg.contains("another rbctl-dsl instance"), "got: {msg}");

        // Releasing (drop) frees the lock for the next acquirer.
        drop(first);
        assert!(PidLock::acquire(path.to_str().unwrap()).is_ok());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn link_status_events() {
        assert_eq!(link_status_to_event(LineState::ShowtimeTcSync), LineEvent::Up);
        assert_eq!(link_status_to_event(LineState::Silent), LineEvent::Down);
        assert_eq!(
            link_status_to_event(LineState::FullInit),
            LineEvent::Training
        );
        assert_eq!(
            link_status_to_event(LineState::Handshake),
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
