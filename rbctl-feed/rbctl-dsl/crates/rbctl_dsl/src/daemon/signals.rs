//! Signal handling — SIGTERM/SIGINT/SIGHUP → process-global AtomicBool
//! flags.
//!
//! The flags are read by both the main control loop and the board worker
//! thread; written by the signal handlers and the IPC command handler.
//! Plain `AtomicBool` is fine because we only ever treat them as level
//! triggers (swap-to-clear).

use std::sync::atomic::{AtomicBool, Ordering};

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
pub(crate) fn install() {
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
