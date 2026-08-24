//! Daemon IPC server + wire protocol.
//!
//! The daemon binds `/var/run/rbctl-dsl.sock` (non-blocking
//! `UnixListener`); a second `rbctl-dsl` instance (the client half, in
//! `crate::cli`) connects, sends one command, and reads the reply:
//!
//! ```text
//! rbctl-dsl status        → print live line state, then exit
//! rbctl-dsl reload        → tell daemon to reload UCI, then exit
//! rbctl-dsl restart-line  → tell daemon to bounce the DSL line, then exit
//! rbctl-dsl stop          → tell daemon to shut down, then exit
//! ```
//!
//! ## Wire protocol
//!
//! Line-based, ASCII, human-readable (debuggable with `nc -U`):
//!
//! **Request** — one line: `STATUS`, `RELOAD`, `RESTART`, `STOP`.
//!
//! **Response** — first line is `OK` or `ERR: <msg>`.
//! For `STATUS`, subsequent lines are `key: value` pairs terminated by `.`
//! on its own line.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::str::FromStr;

use rbctl_proto::oem::pack::{Annex, Modulation};
use rbctl_proto::oem::unpack::LineState;

use crate::config::XferMode;

use super::state::FwStatus;

pub const SOCK_PATH: &str = crate::constants::IPC_SOCK_PATH;

// ── commands ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcCmd {
    Status,
    Reload,
    Restart,
    Stop,
}

impl IpcCmd {
    /// Parse a CLI verb into an IPC command (client-side helper).
    #[allow(dead_code)]
    pub fn from_arg(s: &str) -> Option<Self> {
        match s {
            "status" | "info" => Some(Self::Status),
            "reload" => Some(Self::Reload),
            "restart-line" | "restart" => Some(Self::Restart),
            "stop" | "shutdown" => Some(Self::Stop),
            _ => None,
        }
    }

    pub fn as_wire(&self) -> &'static str {
        match self {
            Self::Status => "STATUS",
            Self::Reload => "RELOAD",
            Self::Restart => "RESTART",
            Self::Stop => "STOP",
        }
    }
}

// ── status payload (shared between daemon and client) ────────────────────

/// Snapshot of daemon state returned by `STATUS`.
///
/// Fields that only have a meaning once the line trained (modulation,
/// annex, uptime, rates, SNR margins) are `Option`s: `None` until the line
/// reaches showtime. `Default::default()` therefore yields exactly the
/// valid "untrained line" snapshot — builders only fill what they truly
/// know.
///
/// # Wire contract: typed values, fixed units, no pre-formatted strings
///
/// Every field crosses the wire as a typed value — enums as stable
/// identifier tokens (parsed back via `FromStr`/`mapping::parse_*`),
/// measurements as integers with a fixed unit — so clients can render any
/// display format (`1h 35m`, dB, dBm, Mbps…) themselves:
///
/// | field         | type      | unit / domain                          |
/// |---------------|-----------|----------------------------------------|
/// | `state`       | `LineState` | shared enum token (`ShowtimeTcSync`, `Silent`, `Unknown(9)`; `NotInitialized` = no successful poll yet) |
/// | `uptime`      | u64       | seconds                                |
/// | `modulation`  | [`Modulation`](rbctl_proto::oem::pack::Modulation) | enum token, e.g. `Vdsl2` |
/// | `annex`       | [`Annex`](rbctl_proto::oem::pack::Annex) | enum token (`A`, `B`, …)  |
/// | `xfer_mode`   | [`XferMode`](crate::config::XferMode) | enum token (`Ptm`, `Atm`, `Auto`) |
/// | `fw_status`   | [`FwStatus`](super::state::FwStatus) | enum token + data, see its wire grammar |
/// | `down_rate` / `up_rate` | u32 | kbps                        |
/// | `down_snr` / `up_snr`   | u32 | dB × 10 (63 = 6.3 dB)       |
///
/// The `status` CLI renders these for humans (`cli::format_status_body`);
/// machine consumers parse the tokens/numbers directly.
#[derive(Debug, Clone, Default)]
pub struct StatusSnapshot {
    /// Latest line state — the shared [`LineState`] enum, `NotInitialized`
    /// until the first successful op2 poll. The LuCI label and Lantiq
    /// number mappings are presentation, applied by ubus and the client.
    pub state: LineState,
    /// Convenience: true while the line is up (showtime).
    pub up: bool,
    /// None until showtime. Seconds since the line came up.
    pub uptime_secs: Option<u64>,
    /// Active (trained) modulation — None until showtime.
    pub modulation: Option<Modulation>,
    /// Active (trained) annex — None until showtime.
    pub annex: Option<Annex>,
    /// Configured transfer mode (None only before the first config load).
    pub xfer_mode: Option<XferMode>,
    /// None until showtime. kbps.
    pub down_rate: Option<u32>,
    /// None until showtime. kbps.
    pub up_rate: Option<u32>,
    /// None until showtime. dB×10 tenths (63 = 6.3 dB) —
    /// unit rendering is the client's job (see `cli::format_status_body`).
    pub down_snr: Option<u32>,
    /// None until showtime. dB×10 tenths.
    pub up_snr: Option<u32>,
    pub fw_status: FwStatus,
    pub board_reachable: bool,
    pub mgmt_iface: String,
    pub mgmt_carrier: bool,
    /// Monitor mode active — daemon polls only, never configures.
    pub monitor: bool,
}

/// Render an optional value; `None` becomes `null` on the wire.
fn opt<T: std::fmt::Display>(v: &Option<T>) -> String {
    match v {
        Some(x) => x.to_string(),
        None => "null".to_string(),
    }
}

/// `opt` for Copy enums serialized via `as_str` (no `Display` dependency).
fn opt_map<T: Copy, F: Fn(T) -> &'static str>(v: &Option<T>, f: F) -> String {
    match v {
        Some(x) => f(*x).to_string(),
        None => "null".to_string(),
    }
}

/// Parse a wire value back into an Option (`null` → None).
fn unopt<T, F: Fn(&str) -> Option<T>>(s: &str, f: F) -> Option<T> {
    if s == "null" { None } else { f(s) }
}

impl StatusSnapshot {
    /// Render as `key: value` lines (without the leading `OK` or trailing `.`).
    ///
    /// Values are typed tokens/integers in fixed units (see the struct's
    /// wire contract table) — this is a machine-facing wire; human unit
    /// rendering happens in the client (`cli::format_status_body`).
    pub fn to_lines(&self) -> String {
        format!(
            "state: {}\nup: {}\nuptime: {}\nmodulation: {}\nannex: {}\n\
             xfer_mode: {}\ndown_rate: {}\nup_rate: {}\ndown_snr: {}\nup_snr: {}\n\
             fw_status: {}\nmonitor: {}",
            self.state, self.up, opt(&self.uptime_secs),
            opt_map(&self.modulation, Modulation::as_str),
            opt_map(&self.annex, Annex::as_str),
            opt_map(&self.xfer_mode, XferMode::as_str),
            opt(&self.down_rate), opt(&self.up_rate), opt(&self.down_snr), opt(&self.up_snr),
            self.fw_status.to_wire(), self.monitor,
        )
    }

    /// Parse `key: value` lines back into a snapshot. Round-trip helper
    /// for the wire format (used by tests; kept for future consumers).
    /// Unparseable enum tokens / numbers fall back to their field default.
    #[allow(dead_code)]
    pub fn from_lines(body: &str) -> Self {
        let mut s = Self::default();
        for line in body.lines() {
            if let Some((k, v)) = line.split_once(": ") {
                match k.trim() {
                    "state" => s.state = v.trim().parse().unwrap_or_default(),
                    "up" => s.up = v.trim() == "true",
                    "uptime" => s.uptime_secs = unopt(v.trim(), |x| x.parse().ok()),
                    "modulation" => s.modulation = unopt(v.trim(), |x| Modulation::from_str(x).ok()),
                    "annex" => s.annex = unopt(v.trim(), |x| Annex::from_str(x).ok()),
                    "xfer_mode" => s.xfer_mode = unopt(v.trim(), |x| XferMode::from_str(x).ok()),
                    "down_rate" => s.down_rate = unopt(v.trim(), |x| x.parse().ok()),
                    "up_rate" => s.up_rate = unopt(v.trim(), |x| x.parse().ok()),
                    "down_snr" => s.down_snr = unopt(v.trim(), |x| x.parse().ok()),
                    "up_snr" => s.up_snr = unopt(v.trim(), |x| x.parse().ok()),
                    "fw_status" => s.fw_status = FwStatus::from_wire(v.trim()).unwrap_or_default(),
                    "monitor" => s.monitor = v.trim() == "true",
                    _ => {}
                }
            }
        }
        s
    }
}

// ── server side ──────────────────────────────────────────────────────────

/// Non-blocking IPC listener. Poll `accept_one()` in the daemon loop.
pub struct IpcListener {
    listener: UnixListener,
}

impl IpcListener {
    /// Bind the socket, removing any stale socket file first.
    pub fn bind(path: &str) -> io::Result<Self> {
        let _ = std::fs::remove_file(path);
        let listener = UnixListener::bind(Path::new(path))?;
        listener.set_nonblocking(true)?;
        // Lock the socket to its owner (root). Default perms are umask-governed
        // and would otherwise let any local user issue STOP / FIRMWARE (D6).
        if unsafe { libc::fchmod(listener.as_raw_fd(), 0o600) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { listener })
    }

    /// Non-blocking: returns `Ok(None)` when no client is waiting.
    ///
    /// When a client connects, reads one command line, invokes the handler,
    /// writes the response, and closes — all synchronously. The handler
    /// returns `(reply_body, should_exit, should_reload, should_restart_line)`.
    pub fn accept_one(
        &self,
        snapshot: &StatusSnapshot,
    ) -> io::Result<Option<IpcAction>> {
        match self.listener.accept() {
            Ok((stream, _)) => {
                if !peer_is_root(&stream) {
                    log::warn!("IPC: rejected connection from non-root peer");
                    return Ok(None);
                }
                stream.set_read_timeout(Some(crate::constants::IPC_READ_TIMEOUT))?;
                let action = handle_client(stream, snapshot)?;
                Ok(Some(action))
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// What the daemon should do after handling an IPC command.
#[derive(Debug)]
pub enum IpcAction {
    None,
    Reload,
    RestartLine,
    Stop,
    /// Firmware upgrade requested — the stream is kept open for progress.
    FirmwareUpgrade {
        path: String,
        stream: UnixStream,
    },
}

impl Default for IpcAction {
    fn default() -> Self {
        Self::None
    }
}

/// Verify the peer is root via `SO_PEERCRED` (Linux). Defense in depth on top
/// of the `0600` socket mode: denies any connection that reached us without
/// being uid 0 (D6).
fn peer_is_root(stream: &UnixStream) -> bool {
    let fd = stream.as_raw_fd();
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let cred: libc::ucred = unsafe {
        let mut cred: libc::ucred = std::mem::zeroed();
        let rc = libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut _ as *mut libc::c_void,
            &mut len,
        );
        if rc != 0 {
            return false;
        }
        cred
    };
    cred.uid == 0
}

fn handle_client(mut stream: UnixStream, snapshot: &StatusSnapshot) -> io::Result<IpcAction> {
    // Bound the read so a client can't grow memory by withholding `\n` (D8).
    let max = crate::constants::IPC_MAX_CMD_LINE as u64;
    let mut reader = BufReader::new(&stream).take(max);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    if line.len() >= crate::constants::IPC_MAX_CMD_LINE && !line.contains('\n') {
        let _ = stream.write_all(b"ERR: command too long\n");
        return Ok(IpcAction::None);
    }

    let cmd = line.trim();

    // FIRMWARE <path> — keep the stream open for progress streaming
    if let Some(path) = cmd.strip_prefix("FIRMWARE ") {
        return Ok(IpcAction::FirmwareUpgrade {
            path: path.trim().to_string(),
            stream,
        });
    }

    let response = match cmd {
        "STATUS" => {
            if snapshot.board_reachable {
                format!("OK\n{}\n.\n", snapshot.to_lines())
            } else {
                let carrier_str = if snapshot.mgmt_carrier { "ok" } else { "down" };
                format!(
                    "ERR: board unreachable on {} (carrier {})\n\
                     mgmt_iface: {}\n\
                     mgmt_carrier: {}\n\
                     hint: ensure management interfaces are up (check dsl_control / netifd)\n\
                     .\n",
                    snapshot.mgmt_iface, carrier_str,
                    snapshot.mgmt_iface,
                    carrier_str,
                )
            }
        }
        "RELOAD" => {
            let _ = stream.write_all(b"OK reloading\n");
            return Ok(IpcAction::Reload);
        }
        "RESTART" => {
            if snapshot.monitor {
                let _ = stream.write_all(b"ERR: restart-line not available in monitor mode\n");
                return Ok(IpcAction::None);
            } else if snapshot.board_reachable {
                let _ = stream.write_all(b"OK restarting line\n");
                return Ok(IpcAction::RestartLine);
            } else {
                let _ = stream.write_all(b"ERR: board unreachable - cannot restart line\n");
                return Ok(IpcAction::None);
            }
        }
        "STOP" => {
            let _ = stream.write_all(b"OK stopping\n");
            return Ok(IpcAction::Stop);
        }
        other => format!("ERR: unknown command '{other}'\n"),
    };

    stream.write_all(response.as_bytes())?;
    Ok(IpcAction::None)
}

// ── tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmd_parsing() {
        assert_eq!(IpcCmd::from_arg("status"), Some(IpcCmd::Status));
        assert_eq!(IpcCmd::from_arg("reload"), Some(IpcCmd::Reload));
        assert_eq!(IpcCmd::from_arg("restart-line"), Some(IpcCmd::Restart));
        assert_eq!(IpcCmd::from_arg("stop"), Some(IpcCmd::Stop));
        assert_eq!(IpcCmd::from_arg("bogus"), None);
    }

    #[test]
    fn status_roundtrip() {
        let s = StatusSnapshot {
            state: LineState::ShowtimeTcSync,
            up: true,
            uptime_secs: Some(300),
            modulation: Some(Modulation::Vdsl2),
            annex: Some(Annex::B),
            xfer_mode: Some(XferMode::Ptm),
            down_rate: Some(50000),
            up_rate: Some(10000),
            down_snr: Some(150),
            up_snr: Some(120),
            fw_status: FwStatus::UpgradingProgress { stage: 2, pct: 45 },
            board_reachable: true,
            mgmt_iface: "lan0.500".into(),
            mgmt_carrier: true,
            monitor: false,
        };
        let text = s.to_lines();
        // IPC wire carries TYPED tokens/values: the board's own enums as
        // identifier tokens, SNR in dB×10 tenths — clients do the rendering.
        assert!(text.contains("state: ShowtimeTcSync"));
        assert!(text.contains("modulation: Vdsl2"));
        assert!(text.contains("annex: B"));
        assert!(text.contains("xfer_mode: Ptm"));
        assert!(text.contains("down_snr: 150"));
        assert!(text.contains("up_snr: 120"));
        assert!(text.contains("fw_status: UpgradingProgress stage=2 pct=45"));
        let parsed = StatusSnapshot::from_lines(&text);
        assert_eq!(parsed.state, LineState::ShowtimeTcSync);
        assert!(parsed.up);
        assert_eq!(parsed.uptime_secs, Some(300));
        assert_eq!(parsed.modulation, Some(Modulation::Vdsl2));
        assert_eq!(parsed.annex, Some(Annex::B));
        assert_eq!(parsed.xfer_mode, Some(XferMode::Ptm));
        assert_eq!(parsed.down_rate, Some(50000));
        assert_eq!(parsed.up_snr, Some(120));
        assert_eq!(parsed.fw_status, FwStatus::UpgradingProgress { stage: 2, pct: 45 });
    }

    #[test]
    fn unknown_state_keeps_board_code_on_wire() {
        let s = StatusSnapshot {
            state: LineState::Unknown(9),
            ..Default::default()
        };
        let text = s.to_lines();
        assert!(text.contains("state: Unknown(9)"));
        assert_eq!(StatusSnapshot::from_lines(&text).state, LineState::Unknown(9));
    }

    #[test]
    fn untrained_snapshot_renders_null() {
        let s = StatusSnapshot::default();
        assert_eq!(s.state, LineState::NotInitialized);
        let text = s.to_lines();
        assert!(text.contains("state: NotInitialized"));
        assert!(text.contains("modulation: null"));
        assert!(text.contains("annex: null"));
        assert!(text.contains("xfer_mode: null"));
        assert!(text.contains("uptime: null"));
        assert!(text.contains("down_rate: null"));
        assert!(text.contains("up_snr: null"));
        assert!(text.contains("fw_status: Idle"));
        // Round-trip keeps them None.
        let parsed = StatusSnapshot::from_lines(&text);
        assert_eq!(parsed.modulation, None);
        assert_eq!(parsed.xfer_mode, None);
        assert_eq!(parsed.fw_status, FwStatus::Idle);
        assert_eq!(parsed.down_rate, None);
    }
}
