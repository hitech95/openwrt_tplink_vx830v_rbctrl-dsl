//! `RtnlAddr` — IP address management via `RTM_NEWADDR`/`RTM_DELADDR`.

use std::ffi::CString;
use std::io;

use tinyln_rs_sys::{ifaddrmsg, unl, IFA_ADDRESS, IFA_LOCAL, RTM_DELADDR, RTM_NEWADDR};

use crate::attr;
use crate::check;
use crate::msg::NlMsg;

fn ifindex(name: &str) -> io::Result<i32> {
    let cname = CString::new(name).unwrap();
    let idx = unsafe { libc::if_nametoindex(cname.as_ptr()) };
    if idx == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(idx as i32)
    }
}

/// Rtnetlink connection for IP address management.
pub struct RtnlAddr {
    unl: unl,
}

impl Drop for RtnlAddr {
    fn drop(&mut self) {
        unsafe { tinyln_rs_sys::unl_free(&mut self.unl) };
    }
}

impl RtnlAddr {
    pub fn new() -> io::Result<Self> {
        let mut unl: unl = unsafe { std::mem::zeroed() };
        check(unsafe { tinyln_rs_sys::unl_rtnl_init(&mut unl) })?;
        Ok(Self { unl })
    }

    fn alloc_msg(&mut self, cmd: i32) -> NlMsg {
        let ptr = unsafe { tinyln_rs_sys::unl_rtnl_msg(&mut self.unl, cmd, false) };
        assert!(!ptr.is_null(), "unl_rtnl_msg returned NULL");
        NlMsg::from_ptr(ptr)
    }

    fn send(&mut self, msg: &NlMsg) -> io::Result<()> {
        check(unsafe { tinyln_rs_sys::nl_send_auto_complete(self.unl.sock, msg.as_ptr()) })?;
        check(unsafe { tinyln_rs_sys::nl_wait_for_ack(self.unl.sock) })
    }

    /// Add an IPv4 address to an interface.
    pub fn add_v4(&mut self, iface: &str, addr: std::net::Ipv4Addr, prefix_len: u8) -> io::Result<()> {
        self.add_del(RTM_NEWADDR as i32, iface, addr, prefix_len)
    }

    /// Delete an IPv4 address from an interface.
    pub fn del_v4(&mut self, iface: &str, addr: std::net::Ipv4Addr, prefix_len: u8) -> io::Result<()> {
        self.add_del(RTM_DELADDR as i32, iface, addr, prefix_len)
    }

    fn add_del(&mut self, cmd: i32, iface: &str, addr: std::net::Ipv4Addr, prefix_len: u8) -> io::Result<()> {
        let idx = ifindex(iface)?;
        let msg = self.alloc_msg(cmd);
        let ifa = ifaddrmsg {
            ifa_family: libc::AF_INET as u8,
            ifa_prefixlen: prefix_len,
            ifa_flags: 0,
            ifa_scope: 0,
            ifa_index: idx as u32,
        };
        msg.append_struct(&ifa)?;

        let bytes = addr.octets();
        attr::put(&msg, IFA_LOCAL as i32, &bytes);
        attr::put(&msg, IFA_ADDRESS as i32, &bytes);

        self.send(&msg)
    }
}
