//! `Unl` — high-level micro-netlink helpers.
//!
//! Wraps `struct unl` from `<unl.h>`. Provides simplified `rtnl()` / `genl()`
//! initialization, one-shot request/response, and event loop support.

use std::ffi::CString;
use std::io;

use tinyln_rs_sys::unl;

use crate::check;
use crate::msg::NlMsg;

/// High-level netlink connection (rtnl or genl).
pub struct Unl {
    pub(crate) unl: unl,
}

impl Drop for Unl {
    fn drop(&mut self) {
        unsafe { tinyln_rs_sys::unl_free(&mut self.unl) };
    }
}

impl Unl {
    /// Initialize for rtnetlink (RTM_* operations).
    pub fn rtnl() -> io::Result<Self> {
        let mut unl: unl = unsafe { std::mem::zeroed() };
        check(unsafe { tinyln_rs_sys::unl_rtnl_init(&mut unl) })?;
        Ok(Self { unl })
    }

    /// Initialize for generic netlink with a given family name.
    pub fn genl(family: &str) -> io::Result<Self> {
        let mut unl: unl = unsafe { std::mem::zeroed() };
        let cname = CString::new(family).unwrap();
        check(unsafe { tinyln_rs_sys::unl_genl_init(&mut unl, cname.as_ptr()) })?;
        Ok(Self { unl })
    }

    /// Create an rtnetlink message.
    pub fn rtnl_msg(&mut self, cmd: i32, dump: bool) -> NlMsg {
        let ptr = unsafe { tinyln_rs_sys::unl_rtnl_msg(&mut self.unl, cmd, dump) };
        assert!(!ptr.is_null(), "unl_rtnl_msg returned NULL");
        NlMsg::from_ptr(ptr)
    }

    /// Create a genl message.
    pub fn genl_msg(&mut self, cmd: i32, dump: bool) -> NlMsg {
        let ptr = unsafe { tinyln_rs_sys::unl_genl_msg(&mut self.unl, cmd, dump) };
        assert!(!ptr.is_null(), "unl_genl_msg returned NULL");
        NlMsg::from_ptr(ptr)
    }

    /// Send a request and wait for ACK.
    pub fn send(&mut self, msg: &NlMsg) -> io::Result<()> {
        check(unsafe { tinyln_rs_sys::nl_send_auto_complete(self.unl.sock, msg.as_ptr()) })?;
        check(unsafe { tinyln_rs_sys::nl_wait_for_ack(self.unl.sock) })
    }
}
