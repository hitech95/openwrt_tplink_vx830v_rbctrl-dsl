//! `NlCb` — safe wrapper around `struct nl_cb`.
//!
//! Callback set for message dispatch via `NlSocket::recvmsgs`.

use std::io;

use tinyln_rs_sys::nl_cb;

/// Owned callback set. Calls `nl_cb_put` on drop.
pub struct NlCb {
    ptr: *mut nl_cb,
}

impl Drop for NlCb {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe { tinyln_rs_sys::nl_cb_put(self.ptr) };
        }
    }
}

/// Callback kind (defaults).
#[derive(Clone, Copy)]
pub enum CbKind {
    Default,
    Verbose,
    Debug,
    Custom,
}

impl CbKind {
    fn as_raw(&self) -> tinyln_rs_sys::nl_cb_kind {
        match self {
            Self::Default => tinyln_rs_sys::NL_CB_DEFAULT,
            Self::Verbose => tinyln_rs_sys::NL_CB_VERBOSE,
            Self::Debug => tinyln_rs_sys::NL_CB_DEBUG,
            Self::Custom => tinyln_rs_sys::NL_CB_CUSTOM,
        }
    }
}

impl NlCb {
    /// Allocate a callback set with the given default kind.
    pub fn alloc(kind: CbKind) -> io::Result<Self> {
        let ptr = unsafe { tinyln_rs_sys::nl_cb_alloc(kind.as_raw()) };
        if ptr.is_null() {
            return Err(io::Error::new(io::ErrorKind::Other, "nl_cb_alloc failed"));
        }
        Ok(Self { ptr })
    }

    /// Clone a callback set.
    pub fn clone_cb(&self) -> io::Result<Self> {
        let ptr = unsafe { tinyln_rs_sys::nl_cb_clone(self.ptr) };
        if ptr.is_null() {
            return Err(io::Error::new(io::ErrorKind::Other, "nl_cb_clone failed"));
        }
        Ok(Self { ptr })
    }

    /// Raw pointer for `NlSocket::recvmsgs`.
    pub(crate) fn as_ptr(&self) -> *mut nl_cb {
        self.ptr
    }
}
