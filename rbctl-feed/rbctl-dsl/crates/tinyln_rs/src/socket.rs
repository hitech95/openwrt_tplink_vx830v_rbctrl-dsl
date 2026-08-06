//! `NlSocket` — safe wrapper around `struct nl_sock`.
//!
//! Connect to a netlink family (`NETLINK_ROUTE`, `NETLINK_GENERIC`, etc.),
//! send messages, receive responses, and wait for ACKs.

use std::io;
use std::os::fd::AsRawFd;
use std::ptr;

use tinyln_rs_sys::nl_sock;

use crate::msg::NlMsg;
use crate::check;

/// Owned netlink socket. Calls `nl_socket_free` on drop.
pub struct NlSocket {
    ptr: *mut nl_sock,
}

impl Drop for NlSocket {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe { tinyln_rs_sys::nl_socket_free(self.ptr) };
        }
    }
}

impl NlSocket {
    /// Allocate a new netlink socket.
    pub fn alloc() -> io::Result<Self> {
        let ptr = unsafe { tinyln_rs_sys::nl_socket_alloc() };
        if ptr.is_null() {
            return Err(io::Error::new(io::ErrorKind::Other, "nl_socket_alloc failed"));
        }
        Ok(Self { ptr })
    }

    /// Connect to a netlink family (e.g. `NETLINK_ROUTE`).
    pub fn connect(family: i32) -> io::Result<Self> {
        let sock = Self::alloc()?;
        check(unsafe { tinyln_rs_sys::nl_connect(sock.ptr, family) })?;
        Ok(sock)
    }

    /// Send a message (caller fills seq/pid).
    pub fn send(&self, msg: &NlMsg) -> io::Result<()> {
        check(unsafe { tinyln_rs_sys::nl_send(self.ptr, msg.as_ptr()) })
    }

    /// Send a message with auto-filled seq/pid, and wait for ACK.
    pub fn send_auto(&self, msg: &NlMsg) -> io::Result<()> {
        check(unsafe { tinyln_rs_sys::nl_send_auto_complete(self.ptr, msg.as_ptr()) })?;
        check(unsafe { tinyln_rs_sys::nl_wait_for_ack(self.ptr) })
    }

    /// Send a simple message (type + flags + optional payload).
    pub fn send_simple(&self, family: i32, msg_type: i32, flags: i32, payload: &[u8]) -> io::Result<()> {
        check(unsafe {
            tinyln_rs_sys::nl_send_simple(
                self.ptr,
                msg_type,
                flags,
                if payload.is_empty() { ptr::null_mut() } else { payload.as_ptr() as *mut _ },
                payload.len(),
            )
        })
    }

    /// Receive messages and dispatch via callback.
    pub fn recvmsgs(&self, cb: &crate::cb::NlCb) -> io::Result<()> {
        check(unsafe { tinyln_rs_sys::nl_recvmsgs(self.ptr, cb.as_ptr()) })
    }

    /// Raw file descriptor (for `select`/`poll`/`uloop` integration).
    pub fn fd(&self) -> i32 {
        // libnl-tiny doesn't export nl_socket_get_fd, but the fd is at a
        // known offset. We use the s_fd field via the bindgen struct.
        // SAFETY: the socket is valid and s_fd is a plain int.
        unsafe { (*self.ptr).s_fd }
    }

    /// Disable sequence number checking (for multicast or async).
    pub fn disable_seq_check(&self) {
        unsafe { tinyln_rs_sys::nl_socket_disable_seq_check(self.ptr) };
    }

    /// Join a multicast group.
    pub fn add_membership(&self, group: i32) -> io::Result<()> {
        check(unsafe { tinyln_rs_sys::nl_socket_add_memberships(self.ptr, group, -1) })
    }

    /// Leave a multicast group.
    pub fn drop_membership(&self, group: i32) -> io::Result<()> {
        check(unsafe { tinyln_rs_sys::nl_socket_drop_memberships(self.ptr, group, -1) })
    }

    /// Set socket buffer size.
    pub fn set_buffer_size(&self, rxbuf: i32, txbuf: i32) -> io::Result<()> {
        check(unsafe { tinyln_rs_sys::nl_socket_set_buffer_size(self.ptr, rxbuf, txbuf) })
    }

    /// Set non-blocking mode.
    pub fn set_nonblocking(&self) -> io::Result<()> {
        check(unsafe { tinyln_rs_sys::nl_socket_set_nonblocking(self.ptr) })
    }

    /// Raw pointer for FFI interop (e.g. with `Unl`).
    pub(crate) fn as_ptr(&self) -> *mut nl_sock {
        self.ptr
    }
}
