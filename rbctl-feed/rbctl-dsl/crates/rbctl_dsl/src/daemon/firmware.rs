//! `FIRMWARE <path>` IPC command handler — validates the image, sends it
//! to the worker, and streams progress back to the IPC client.
//!
//! The IPC connection is held open for the duration of the upload — if the
//! client disconnects, the worker continues and the result is stored in
//! shared state.

use std::io::Write;
use std::sync::mpsc;

use super::worker::{FwEvent, WorkerCmd};
use crate::constants;
use super::state::{FwStatus, SharedState};

pub(crate) fn handle_firmware_ipc(
    path: &str,
    stream: &mut std::os::unix::net::UnixStream,
    cmd_tx: &mpsc::Sender<WorkerCmd>,
    state: &SharedState,
) {
    // Check if an upgrade is already running
    {
        let st = state.lock().unwrap();
        if !matches!(
            st.fw_status,
            FwStatus::Idle | FwStatus::Done | FwStatus::Failed(_)
        ) {
            let _ = stream.write_all(b"ERR: upgrade already in progress\n.\n");
            return;
        }
    }

    // Stat first so we never slurp a non-regular file or an oversized image
    // into memory (D7). MAX_IMAGE_SIZE is the board's own 8 MB cap.
    let max = rbctl_proto::oem::firmware::MAX_IMAGE_SIZE as u64;
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) => {
            let _ = stream.write_all(format!("ERR: cannot stat {path}: {e}\n.\n").as_bytes());
            return;
        }
    };
    if !meta.is_file() {
        let _ = stream.write_all(b"ERR: not a regular file\n.\n");
        return;
    }
    if meta.len() > max {
        let _ = stream.write_all(
            format!("ERR: image too large ({} > {max} bytes)\n.\n", meta.len()).as_bytes(),
        );
        return;
    }

    // Read and validate the file
    let image = match std::fs::read(path) {
        Ok(data) => data,
        Err(e) => {
            let _ = stream.write_all(format!("ERR: cannot read {path}: {e}\n.\n").as_bytes());
            return;
        }
    };

    if let Err(e) = rbctl_proto::oem::firmware::validate_image(&image) {
        let _ = stream.write_all(format!("ERR: {e}\n.\n").as_bytes());
        return;
    }

    let hdr = rbctl_proto::oem::firmware::parse_header(&image).ok();
    let version = hdr
        .as_ref()
        .map(|h| rbctl_proto::oem::firmware::version_str(h).to_string());

    // Send acknowledgment
    let _ = stream.write_all(b"OK\n");
    if let Some(ref v) = version {
        let _ = stream.write_all(format!("IMAGE version={v} size={}\n", image.len()).as_bytes());
    }
    let _ = stream.flush();

    // Send to worker
    let (reply_tx, reply_rx) = mpsc::channel::<FwEvent>();
    if cmd_tx.send(WorkerCmd::FirmwareUpgrade { image, reply_tx }).is_err() {
        let _ = stream.write_all(b"ERR: worker thread unavailable\n.\n");
        return;
    }

    // Stream progress until done
    let _ = stream.set_write_timeout(Some(constants::IPC_FW_STREAM_WRITE_TIMEOUT));
    while let Ok(event) = reply_rx.recv() {
        match event {
            FwEvent::Progress(p) => {
                let line = if p.pct > 0 {
                    format!("PROGRESS stage={} pct={}\n", p.stage, p.pct)
                } else {
                    format!("PROGRESS stage={}\n", p.stage)
                };
                if stream.write_all(line.as_bytes()).is_err() {
                    break; // client disconnected
                }
                let _ = stream.flush();
            }
            FwEvent::Done(Ok(result)) => {
                let _ = stream.write_all(
                    format!(
                        "DONE version=0x{:08X} status={}\n.\n",
                        result.version, result.status
                    )
                    .as_bytes(),
                );
                break;
            }
            FwEvent::Done(Err(msg)) => {
                let _ = stream.write_all(format!("ERR: {msg}\n.\n").as_bytes());
                break;
            }
        }
    }
}
