//! `RtnlRoute` — routing table management via `RTM_NEWROUTE`/`RTM_DELROUTE`.

use std::io;

use tinyln_rs_sys::{rtmsg, unl, RTM_DELROUTE, RTM_NEWROUTE};

// RTA_* constants from <linux/rtnetlink.h> enum (may not be in bindgen output)
const RTA_DST: i32 = 1;
const RTA_GATEWAY: i32 = 5;
const RTA_OIF: i32 = 4;

use crate::attr;
use crate::check;
use crate::msg::NlMsg;

fn ifindex(name: &str) -> io::Result<u32> {
    let cname = std::ffi::CString::new(name).unwrap();
    let idx = unsafe { libc::if_nametoindex(cname.as_ptr()) };
    if idx == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(idx)
    }
}

/// Rtnetlink connection for route management.
pub struct RtnlRoute {
    unl: unl,
}

impl Drop for RtnlRoute {
    fn drop(&mut self) {
        unsafe { tinyln_rs_sys::unl_free(&mut self.unl) };
    }
}

impl RtnlRoute {
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

    /// Add an IPv4 route via a gateway and output interface.
    pub fn add_v4(&mut self, dst: std::net::Ipv4Addr, prefix_len: u8, gateway: Option<std::net::Ipv4Addr>, oif: &str) -> io::Result<()> {
        self.add_del(RTM_NEWROUTE as i32, dst, prefix_len, gateway, oif)
    }

    /// Delete an IPv4 route.
    pub fn del_v4(&mut self, dst: std::net::Ipv4Addr, prefix_len: u8, gateway: Option<std::net::Ipv4Addr>, oif: &str) -> io::Result<()> {
        self.add_del(RTM_DELROUTE as i32, dst, prefix_len, gateway, oif)
    }

    fn add_del(&mut self, cmd: i32, dst: std::net::Ipv4Addr, prefix_len: u8, gateway: Option<std::net::Ipv4Addr>, oif: &str) -> io::Result<()> {
        let idx = ifindex(oif)?;
        let msg = self.alloc_msg(cmd);

        let rtm = rtmsg {
            rtm_family: libc::AF_INET as u8,
            rtm_dst_len: prefix_len,
            rtm_src_len: 0,
            rtm_tos: 0,
            rtm_table: 0, // RT_TABLE_MAIN
            rtm_protocol: 0, // RTPROT_STATIC
            rtm_scope: 0,
            rtm_type: 0, // RTN_UNICAST
            rtm_flags: 0,
        };
        msg.append_struct(&rtm)?;

        attr::put(&msg, RTA_DST as i32, &dst.octets());
        if let Some(gw) = gateway {
            attr::put(&msg, RTA_GATEWAY as i32, &gw.octets());
        }
        attr::put_u32(&msg, RTA_OIF as i32, idx);

        self.send(&msg)
    }
}
