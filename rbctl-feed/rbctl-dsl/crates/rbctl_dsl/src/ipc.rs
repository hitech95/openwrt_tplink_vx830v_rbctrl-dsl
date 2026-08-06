//! IPC control socket — lets a second `rbctl-dsl` instance act as a
//! control client instead of spawning a duplicate daemon.
//!
//! The daemon binds `/var/run/rbctl-dsl.sock` (non-blocking `UnixListener`).
//! A second instance that connects sends one command and reads the reply:
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
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::time::Duration;

use crate::uci_cfg::XferMode;

pub const SOCK_PATH: &str = "/var/run/rbctl-dsl.sock";

// ── commands ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcCmd {
    Status,
    Reload,
    Restart,
    Stop,
}

impl IpcCmd {
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
#[derive(Debug, Clone, Default)]
pub struct StatusSnapshot {
    pub state: String,
    pub state_num: i32,
    pub up: bool,
    pub uptime_secs: u64,
    pub modulation: String,
    pub annex: String,
    pub xfer_mode: String,
    pub down_rate: u32,
    pub up_rate: u32,
    pub down_snr: u32,
    pub up_snr: u32,
}

impl StatusSnapshot {
    /// Render as `key: value` lines (without the leading `OK` or trailing `.`).
    pub fn to_lines(&self) -> String {
        format!(
            "state: {}\nstate_num: {}\nup: {}\nuptime: {}\nmodulation: {}\nannex: {}\n\
             xfer_mode: {}\ndown_rate: {}\nup_rate: {}\ndown_snr: {}\nup_snr: {}",
            self.state, self.state_num, self.up, self.uptime_secs,
            self.modulation, self.annex, self.xfer_mode,
            self.down_rate, self.up_rate, self.down_snr, self.up_snr,
        )
    }

    /// Parse `key: value` lines back into a snapshot.
    pub fn from_lines(body: &str) -> Self {
        let mut s = Self::default();
        for line in body.lines() {
            if let Some((k, v)) = line.split_once(": ") {
                match k.trim() {
                    "state" => s.state = v.trim().to_string(),
                    "state_num" => s.state_num = v.trim().parse().unwrap_or(0),
                    "up" => s.up = v.trim() == "true",
                    "uptime" => s.uptime_secs = v.trim().parse().unwrap_or(0),
                    "modulation" => s.modulation = v.trim().to_string(),
                    "annex" => s.annex = v.trim().to_string(),
                    "xfer_mode" => s.xfer_mode = v.trim().to_string(),
                    "down_rate" => s.down_rate = v.trim().parse().unwrap_or(0),
                    "up_rate" => s.up_rate = v.trim().parse().unwrap_or(0),
                    "down_snr" => s.down_snr = v.trim().parse().unwrap_or(0),
                    "up_snr" => s.up_snr = v.trim().parse().unwrap_or(0),
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
                stream.set_read_timeout(Some(Duration::from_secs(5)))?;
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
pub struct IpcAction {
    pub should_reload: bool,
    pub should_restart_line: bool,
    pub should_stop: bool,
}

fn handle_client(mut stream: UnixStream, snapshot: &StatusSnapshot) -> io::Result<IpcAction> {
    let mut reader = BufReader::new(&stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;

    let cmd = line.trim();
    let mut action = IpcAction {
        should_reload: false,
        should_restart_line: false,
        should_stop: false,
    };

    let response = match cmd {
        "STATUS" => {
            format!("OK\n{}\n.\n", snapshot.to_lines())
        }
        "RELOAD" => {
            action.should_reload = true;
            "OK reloading\n".to_string()
        }
        "RESTART" => {
            action.should_restart_line = true;
            "OK restarting line\n".to_string()
        }
        "STOP" => {
            action.should_stop = true;
            "OK stopping\n".to_string()
        }
        other => format!("ERR: unknown command '{other}'\n"),
    };

    stream.write_all(response.as_bytes())?;
    Ok(action)
}

// ── client side ──────────────────────────────────────────────────────────

/// Connect to a running daemon and send a command. Returns the raw reply.
pub fn send_command(path: &str, cmd: IpcCmd) -> io::Result<String> {
    let mut stream = UnixStream::connect(Path::new(path))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.write_all(format!("{}\n", cmd.as_wire()).as_bytes())?;

    let mut response = String::new();
    let mut reader = BufReader::new(&stream);
    reader.read_to_string(&mut response)?;
    Ok(response)
}

/// Try to connect to a running daemon. Returns `Ok(())` if one exists.
pub fn daemon_running() -> bool {
    UnixStream::connect(SOCK_PATH).is_ok()
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
            state: "Up".into(),
            state_num: 7,
            up: true,
            uptime_secs: 300,
            modulation: "Vdsl2".into(),
            annex: "B".into(),
            xfer_mode: "Ptm".into(),
            down_rate: 50000,
            up_rate: 10000,
            down_snr: 150,
            up_snr: 120,
        };
        let text = s.to_lines();
        let parsed = StatusSnapshot::from_lines(&text);
        assert_eq!(parsed.state, "Up");
        assert_eq!(parsed.state_num, 7);
        assert!(parsed.up);
        assert_eq!(parsed.uptime_secs, 300);
        assert_eq!(parsed.down_rate, 50000);
        assert_eq!(parsed.up_snr, 120);
    }
}
