//! `NlMsg` — safe wrapper around `struct nl_msg`.
//!
//! Netlink message builder. Allocates an `nl_msg`, appends headers and
//! data, and frees it on drop. Use [`crate::attr`] for typed attribute
//! operations.

use std::io;
use std::os::raw::c_void;
use std::ptr;

use tinyln_rs_sys::{nl_msg, nlmsghdr};

use crate::check;
use crate::NLMSG_ALIGNTO;

/// Owned netlink message. Calls `nlmsg_free` on drop.
pub struct NlMsg {
    ptr: *mut nl_msg,
}

impl Drop for NlMsg {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe { tinyln_rs_sys::nlmsg_free(self.ptr) };
        }
    }
}

impl NlMsg {
    /// Allocate an empty message.
    pub fn alloc() -> io::Result<Self> {
        let ptr = unsafe { tinyln_rs_sys::nlmsg_alloc() };
        if ptr.is_null() {
            return Err(io::Error::new(io::ErrorKind::Other, "nlmsg_alloc failed"));
        }
        Ok(Self { ptr })
    }

    /// Allocate a message with a pre-filled `nlmsghdr`.
    ///
    /// `nlmsg_type` is the message type (e.g. `RTM_NEWLINK`), `flags` is the
    /// NLM_F_* bitmask (e.g. `NLM_F_REQUEST | NLM_F_CREATE`).
    pub fn alloc_simple(msg_type: i32, flags: i32) -> io::Result<Self> {
        let ptr = unsafe { tinyln_rs_sys::nlmsg_alloc_simple(msg_type, flags) };
        if ptr.is_null() {
            return Err(io::Error::new(io::ErrorKind::Other, "nlmsg_alloc_simple failed"));
        }
        Ok(Self { ptr })
    }

    /// Append raw data to the message payload.
    pub fn append(&self, data: &[u8]) -> io::Result<()> {
        check(unsafe {
            tinyln_rs_sys::nlmsg_append(
                self.ptr,
                data.as_ptr() as *mut c_void,
                data.len(),
                NLMSG_ALIGNTO as i32,
            )
        })
    }

    /// Append a typed value.
    ///
    /// `T` must be `Copy` (POD / `#[repr(C)]`) so that reinterpreting its bytes
    /// as a raw byte slice is sound — types with drop glue or owning pointers
    /// (e.g. `Vec`) are rejected at compile time. Padding bytes are copied as
    /// stored (standard C-struct serialization).
    pub fn append_struct<T: Copy>(&self, val: &T) -> io::Result<()> {
        let data = unsafe {
            std::slice::from_raw_parts(val as *const T as *const u8, std::mem::size_of::<T>())
        };
        self.append(data)
    }

    /// Reserve `len` bytes at the end of the message and return a mutable
    /// pointer to the reserved space.
    pub fn reserve(&self, len: usize, pad: i32) -> io::Result<*mut c_void> {
        let p = unsafe { tinyln_rs_sys::nlmsg_reserve(self.ptr, len, pad) };
        if p.is_null() {
            Err(io::Error::new(io::ErrorKind::Other, "nlmsg_reserve failed"))
        } else {
            Ok(p)
        }
    }

    /// Borrow the `nlmsghdr` (read-only).
    pub fn header(&self) -> &nlmsghdr {
        unsafe { &*(*self.ptr).nm_nlh }
    }

    /// Mutably borrow the `nlmsghdr` (for setting flags etc.).
    ///
    /// Takes `&mut self` so the borrow checker prevents aliasing `&mut` with
    /// concurrent `&self` reads — deriving `&mut` from `&self` would be UB.
    pub fn header_mut(&mut self) -> &mut nlmsghdr {
        unsafe { &mut *(*self.ptr).nm_nlh }
    }

    /// Pointer to the message payload (after `nlmsghdr`).
    pub fn data_ptr(&self) -> *const u8 {
        let nlh = unsafe { (*self.ptr).nm_nlh };
        unsafe { (nlh as *const u8).add(crate::nlmsg_align((*nlh).nlmsg_len as usize)) }
    }

    /// Raw `*mut nl_msg` pointer (for FFI / attr operations).
    pub(crate) fn as_ptr(&self) -> *mut nl_msg {
        self.ptr
    }

    /// Wrap an existing `*mut nl_msg` — takes ownership (will free on drop).
    pub(crate) fn from_ptr(ptr: *mut nl_msg) -> Self {
        Self { ptr }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_simple_works() {
        let msg = NlMsg::alloc_simple(0x10, 0x01);
        assert!(msg.is_ok());
    }

    #[test]
    fn header_preserves_type_and_flags() {
        let msg = NlMsg::alloc_simple(0x10, 0x01).unwrap();
        assert_eq!(msg.header().nlmsg_type, 0x10);
        assert_eq!(msg.header().nlmsg_flags, 0x01);
    }

    #[test]
    fn append_grows_message() {
        let msg = NlMsg::alloc_simple(0x10, 0x01).unwrap();
        let before = msg.header().nlmsg_len;
        msg.append(&[0xDE, 0xAD, 0xBE, 0xEF]).unwrap();
        assert!(msg.header().nlmsg_len > before);
    }

    #[test]
    fn reserve_returns_writable_ptr() {
        let msg = NlMsg::alloc_simple(0x10, 0x01).unwrap();
        let ptr = msg.reserve(8, 4).unwrap();
        assert!(!ptr.is_null());
    }
}
