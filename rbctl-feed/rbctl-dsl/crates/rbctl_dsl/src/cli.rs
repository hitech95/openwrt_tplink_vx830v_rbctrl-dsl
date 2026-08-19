//! CLI-side IPC client — talks to a running daemon over its control
//! socket. Used by `main.rs` for the `status` / `reload` / `restart-line`
//! / `stop` / `firmware-upgrade` subcommands.
//!
//! The wire protocol is defined by the daemon (`daemon::ipc`); this module
//! only connects and speaks it. Dependency direction is strictly
//! cli → daemon::ipc, never the reverse.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

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
}
