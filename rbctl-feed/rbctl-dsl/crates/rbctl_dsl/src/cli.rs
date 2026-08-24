//! CLI-side IPC client — talks to a running daemon over its control
//! socket. Used by `main.rs` for the `status` / `reload` / `restart-line`
//! / `stop` / `firmware-upgrade` subcommands.
//!
//! The wire protocol is defined by the daemon (`daemon::ipc`); this module
//! only connects and speaks it. Dependency direction is strictly
//! cli → daemon::ipc, never the reverse.
//!
//! The daemon reply carries RAW board units (SNR in dB×10, rates in
//! kbps). All human-facing unit rendering (dB, dBm, …) lives here in the
//! client, so future output flags (raw/json/alternate units) stay
//! CLI-only changes.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::str::FromStr;

use crate::daemon::ipc::{IpcCmd, SOCK_PATH};

/// Connect to a running daemon and send a command. Returns the raw reply.
pub fn send_command(path: &str, cmd: IpcCmd) -> io::Result<String> {
    let mut stream = UnixStream::connect(Path::new(path))?;
    stream.set_read_timeout(Some(crate::constants::IPC_READ_TIMEOUT))?;
    stream.write_all(format!("{}\n", cmd.as_wire()).as_bytes())?;

    let mut response = String::new();
    let mut reader = BufReader::new(&stream);
    reader.read_to_string(&mut response)?;
    Ok(response)
}

/// Connect to a running daemon, send `FIRMWARE <path>`, and stream progress
/// lines to the caller via `on_progress`. Returns the final reply text.
///
/// The connection stays open until the daemon sends `DONE` or `ERR`.
pub fn send_firmware(path: &str, on_progress: &mut dyn FnMut(&str)) -> io::Result<String> {
    let mut stream = UnixStream::connect(Path::new(SOCK_PATH))?;
    // Long read timeout — firmware upload can take minutes
    stream.set_read_timeout(Some(crate::constants::IPC_FW_UPLOAD_TIMEOUT))?;

    stream.write_all(format!("FIRMWARE {path}\n").as_bytes())?;

    let mut reader = BufReader::new(&stream);
    let mut result = String::new();
    let mut line = String::new();

    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            break; // connection closed
        }

        let trimmed = line.trim();
        if trimmed == "." {
            break; // end of response
        }
        if trimmed.starts_with("ERR:") || trimmed.starts_with("DONE") {
            result = trimmed.to_string();
            break;
        }
        if trimmed.starts_with("OK") {
            // Initial acknowledgment — keep reading
            continue;
        }
        // Progress line
        on_progress(trimmed);
    }

    Ok(result)
}

/// Modulation wire token → human name (matches the board's
/// `apiGetAdslType` table: 0=T1.413 … 6=VDSL2, 7=multimode).
fn modulation_display(token: &str) -> Option<&'static str> {
    Some(match token {
        "AdslAnsiT1413" => "T1.413",
        "AdslGdmt" => "G.dmt",
        "AdslGlite" => "G.lite",
        "AdslGdmtBis" => "G.dmt.bis",
        "Adsl2Plus" => "ADSL2+",
        "AdslMultimode" => "ADSL multimode",
        "Vdsl2" => "VDSL2",
        "Multimode" => "multimode",
        _ => return None,
    })
}

/// Firmware wire token → human phrasing.
fn fw_display(wire: &str) -> Option<String> {
    use crate::daemon::state::FwStatus;
    let st = FwStatus::from_wire(wire)?;
    Some(match st {
        FwStatus::Idle => "idle".into(),
        FwStatus::Upgrading => "upgrading".into(),
        FwStatus::UpgradingProgress { stage, pct } => {
            format!("upgrading (stage={stage}, pct={pct}%)")
        }
        FwStatus::Done => "done".into(),
        FwStatus::Failed(msg) => format!("failed: {msg}"),
    })
}

/// Render a status reply for humans: converts the typed wire values
/// (enum tokens, dB×10 tenths) to display forms. Non-matching lines
/// (framing, other keys, unknown tokens) pass through verbatim.
///
/// Unit/label table (key → display, wire):
/// - `state`: `LineState` token → LuCI label (`ShowtimeTcSync` →
///   "Showtime with TC-Layer sync")
/// - `modulation`: `Modulation` token → name (`Vdsl2` → "VDSL2")
/// - `fw_status`: `FwStatus` wire form → human phrasing
/// - `down_snr` / `up_snr`: dB×10 tenths → dB, one decimal
///
/// Extend the match when new metric fields join the snapshot (e.g.
/// attenuation dB×10 → dB, output power dBm×10 → dBm).
pub fn format_status_body(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    for (i, line) in body.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if let Some((k, v)) = line.split_once(": ") {
            let rendered = match k {
                "state" => rbctl_proto::oem::unpack::LineState::from_str(v.trim())
                    .ok()
                    .map(|ls| {
                        let label = crate::utils::mapping::state_label(ls);
                        format!("{k}: {label}")
                    }),
                "modulation" => modulation_display(v.trim())
                    .map(|name| format!("{k}: {name}")),
                "fw_status" => fw_display(v.trim())
                    .map(|human| format!("{k}: {human}")),
                "down_snr" | "up_snr" => v
                    .trim()
                    .parse::<u32>()
                    .ok()
                    .map(|tenths| format!("{k}: {:.1}", tenths as f32 / 10.0)),
                _ => None,
            };
            out.push_str(rendered.as_deref().unwrap_or(line));
        } else {
            out.push_str(line);
        }
    }
    if body.ends_with('\n') {
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::ipc::IpcCmd;

    #[test]
    fn cmd_from_arg() {
        assert_eq!(IpcCmd::from_arg("status"), Some(IpcCmd::Status));
        assert_eq!(IpcCmd::from_arg("reload"), Some(IpcCmd::Reload));
        assert_eq!(IpcCmd::from_arg("restart-line"), Some(IpcCmd::Restart));
        assert_eq!(IpcCmd::from_arg("stop"), Some(IpcCmd::Stop));
        assert_eq!(IpcCmd::from_arg("bogus"), None);
    }

    #[test]
    fn status_body_renders_snr_db() {
        let body = "OK\nstate: FullInit\ndown_snr: 63\nup_snr: 61\n.";
        let out = format_status_body(body);
        assert!(out.contains("down_snr: 6.3"), "{out}");
        assert!(out.contains("up_snr: 6.1"), "{out}");
        assert!(out.starts_with("OK\n"), "{out}");
        assert!(out.ends_with("\n."), "{out}");
    }

    #[test]
    fn status_body_renders_state_label() {
        let out = format_status_body("state: ShowtimeTcSync\n");
        assert!(out.contains("state: Showtime with TC-Layer sync"), "{out}");
        assert!(format_status_body("state: Silent\n").contains("state: Silent"));
        // never-polled lifecycle state → human phrasing
        assert!(format_status_body("state: NotInitialized\n").contains("state: Not initialized"));
        // unknown code keeps diagnostics on the wire, renders as Unknown
        assert!(format_status_body("state: Unknown(9)\n").contains("state: Unknown"));
        // unknown tokens pass through verbatim
        assert!(format_status_body("state: Bogus\n").contains("state: Bogus"));
    }

    #[test]
    fn status_body_renders_modulation_name() {
        let out = format_status_body("modulation: Vdsl2\n");
        assert!(out.contains("modulation: VDSL2"), "{out}");
        // unknown token passes through verbatim (no bogus name)
        assert!(format_status_body("modulation: Foo\n").contains("modulation: Foo"));
        // untrained line stays null
        assert!(format_status_body("modulation: null\n").contains("modulation: null"));
    }

    #[test]
    fn status_body_renders_fw_status() {
        assert!(format_status_body("fw_status: Idle\n").contains("fw_status: idle"));
        assert!(format_status_body("fw_status: Upgrading\n").contains("fw_status: upgrading"));
        assert!(
            format_status_body("fw_status: UpgradingProgress stage=2 pct=45\n")
                .contains("fw_status: upgrading (stage=2, pct=45%)")
        );
        assert!(
            format_status_body("fw_status: Failed board timeout\n")
                .contains("fw_status: failed: board timeout")
        );
    }

    #[test]
    fn status_body_zero_and_null_verbatim() {
        let out = format_status_body("down_snr: 0\nup_snr: null\nstate: Silent\n");
        assert!(out.contains("down_snr: 0.0"), "{out}");
        assert!(out.contains("up_snr: null"), "{out}"); // untrained — no conversion
        assert!(out.contains("state: Silent"), "{out}");
        assert!(out.ends_with('\n'), "{out}");
    }

    #[test]
    fn status_body_leaves_unknown_keys_alone() {
        let body = "OK\nxfer_mode: Ptm\nannex: B\ndown_rate: 40000\n.";
        assert_eq!(format_status_body(body), body);
    }
}
