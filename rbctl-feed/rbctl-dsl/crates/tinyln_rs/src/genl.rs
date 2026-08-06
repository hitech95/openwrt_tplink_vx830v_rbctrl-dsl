//! `GenlSocket` — generic netlink family resolution and messaging.
//!
//! Wraps `unl_genl_init()` for family resolution and message construction.

use std::ffi::CString;
use std::io;

use tinyln_rs_sys::unl;

use crate::check;
use crate::msg::NlMsg;

/// Generic netlink connection.
pub struct GenlSocket {
    unl: unl,
}

impl Drop for GenlSocket {
    fn drop(&mut self) {
        unsafe { tinyln_rs_sys::unl_free(&mut self.unl) };
    }
}

impl GenlSocket {
    /// Resolve a generic netlink family by name (e.g. `"nl80211"`).
    pub fn new(family_name: &str) -> io::Result<Self> {
        let mut unl: unl = unsafe { std::mem::zeroed() };
        let cname = CString::new(family_name).unwrap();
        check(unsafe { tinyln_rs_sys::unl_genl_init(&mut unl, cname.as_ptr()) })?;
        Ok(Self { unl })
    }

    /// Allocate a genl message for the given command.
    pub fn msg(&mut self, cmd: i32, dump: bool) -> NlMsg {
        let ptr = unsafe { tinyln_rs_sys::unl_genl_msg(&mut self.unl, cmd, dump) };
        assert!(!ptr.is_null(), "unl_genl_msg returned NULL");
        NlMsg::from_ptr(ptr)
    }

    /// Resolve a multicast group name to its ID.
    pub fn multicast_id(&mut self, name: &str) -> io::Result<i32> {
        let cname = CString::new(name).unwrap();
        let id = unsafe { tinyln_rs_sys::unl_genl_multicast_id(&mut self.unl, cname.as_ptr()) };
        if id < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(id)
        }
    }

    /// Subscribe to a multicast group by name.
    pub fn subscribe(&mut self, name: &str) -> io::Result<()> {
        let cname = CString::new(name).unwrap();
        check(unsafe { tinyln_rs_sys::unl_genl_subscribe(&mut self.unl, cname.as_ptr()) })
    }

    /// Unsubscribe from a multicast group.
    pub fn unsubscribe(&mut self, name: &str) -> io::Result<()> {
        let cname = CString::new(name).unwrap();
        check(unsafe { tinyln_rs_sys::unl_genl_unsubscribe(&mut self.unl, cname.as_ptr()) })
    }
}
