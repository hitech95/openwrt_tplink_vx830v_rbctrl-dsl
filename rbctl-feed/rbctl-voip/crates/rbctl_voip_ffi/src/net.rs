// SPDX-License-Identifier: GPL-2.0-or-later
//! Board transport bridge for the single-owner libre loop.
use super::*;
use core::ffi::{c_char, CStr};
use rbctl_voip_net::BoardSocket;

/// Opaque owner of the three nonblocking packet sockets.
pub struct rbctl_voip_socket(BoardSocket);

/// Open a Linux voice transport; returns 0 or negative errno. The three fds
/// are borrowed (do not close); unregister them before freeing the owner.
/// # Safety
/// `iface` is a NUL-terminated interface name. `out`, `mac[6]`, and `fds[3]`
/// must be separate writable buffers. Outputs are valid on success only.
#[no_mangle]
pub unsafe extern "C" fn rbctl_voip_socket_open(
    iface: *const c_char,
    out: *mut *mut rbctl_voip_socket,
    mac: *mut u8,
    fds: *mut c_int,
) -> c_int {
    if iface.is_null() || out.is_null() || mac.is_null() || fds.is_null() {
        return -libc::EINVAL;
    }
    // SAFETY: caller supplies a terminated string and checked output buffers.
    let Ok(name) = (unsafe { CStr::from_ptr(iface) }).to_str() else {
        return -libc::EINVAL;
    };
    let s = match BoardSocket::open(name) {
        Ok(s) => s,
        Err(e) => return -e.raw_os_error().unwrap_or(libc::EIO),
    };
    unsafe {
        ptr::copy_nonoverlapping(s.mac().as_ptr(), mac, 6);
        ptr::copy_nonoverlapping(s.fds().as_ptr(), fds, 3);
        out.write(Box::into_raw(Box::new(rbctl_voip_socket(s))));
    }
    0
}

/// Write one complete host frame; compatible with the link send callback.
/// # Safety
/// `ctx` is a live socket owner, exclusively accessible during this call;
/// `frame` is readable for `len` bytes.
#[no_mangle]
pub unsafe extern "C" fn rbctl_voip_socket_send(
    ctx: *mut c_void,
    frame: *const u8,
    len: usize,
) -> c_int {
    let Some(s) = (unsafe { ctx.cast::<rbctl_voip_socket>().as_mut() }) else {
        return -libc::EINVAL;
    };
    let Some(f) = (unsafe { bytes(frame, len) }) else {
        return -libc::EINVAL;
    };
    s.0.send(f).map_or_else(neg, |()| 0)
}

/// Receive one frame, returning its length, 0 when idle, or negative errno.
/// # Safety
/// `s` is a live, exclusively accessible socket owner. `frame` is writable
/// for RBCTL_VOIP_FRAME_MAX bytes and does not overlap `s`.
#[no_mangle]
pub unsafe extern "C" fn rbctl_voip_socket_recv(
    s: *mut rbctl_voip_socket,
    frame: *mut u8,
) -> c_int {
    let Some(s) = (unsafe { s.as_mut() }) else {
        return -libc::EINVAL;
    };
    let Some(buf) = (unsafe { frame.cast::<[u8; RBCTL_VOIP_FRAME_MAX]>().as_mut() }) else {
        return -libc::EINVAL;
    };
    match s.0.recv(buf) {
        Ok(n) => n.unwrap_or(0) as c_int,
        Err(e) => -e.raw_os_error().unwrap_or(libc::EIO),
    }
}

/// Close all three fds. NULL is ignored.
/// # Safety
/// `s` must be a unique handle returned by socket_open and not used afterwards.
#[no_mangle]
pub unsafe extern "C" fn rbctl_voip_socket_free(s: *mut rbctl_voip_socket) {
    if !s.is_null() {
        drop(unsafe { Box::from_raw(s) });
    }
}
