//! `RtnlLink` — network interface management via `RTM_NEWLINK`/`RTM_DELLINK`/`RTM_SETLINK`.

use std::ffi::CString;
use std::io;

use tinyln_rs_sys::{
    ifinfomsg, unl, IFLA_INFO_DATA, IFLA_INFO_KIND, IFLA_IFNAME, IFLA_LINK,
    IFLA_LINKINFO, IFLA_VLAN_ID, RTM_DELLINK, RTM_NEWLINK, RTM_SETLINK,
};

use crate::attr;
use crate::check;
use crate::msg::NlMsg;

// Netlink message flags (from <linux/netlink.h>).
const NLM_F_REQUEST: u16 = 0x01;
const NLM_F_ACK: u16 = 0x04;
const NLM_F_CREATE: u16 = 0x0400;
const NLM_F_EXCL: u16 = 0x0200;

const IFF_UP: u32 = 1;

fn ifindex(name: &str) -> io::Result<i32> {
    let cname = CString::new(name).unwrap();
    let idx = unsafe { libc::if_nametoindex(cname.as_ptr()) };
    if idx == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(idx as i32)
    }
}

fn build_ifi(idx: i32, flags: u32, change: u32) -> ifinfomsg {
    ifinfomsg {
        ifi_family: libc::AF_UNSPEC as u8,
        __ifi_pad: 0,
        ifi_type: 0,
        ifi_index: idx,
        ifi_flags: flags,
        ifi_change: change,
    }
}

/// Rtnetlink connection for interface management.
///
/// Wraps a `struct unl` (micro-netlink) connected to `NETLINK_ROUTE`.
pub struct RtnlLink {
    unl: unl,
}

impl Drop for RtnlLink {
    fn drop(&mut self) {
        unsafe { tinyln_rs_sys::unl_free(&mut self.unl) };
    }
}

impl RtnlLink {
    /// Connect to the rtnetlink socket.
    pub fn new() -> io::Result<Self> {
        let mut unl: unl = unsafe { std::mem::zeroed() };
        check(unsafe { tinyln_rs_sys::unl_rtnl_init(&mut unl) })?;
        Ok(Self { unl })
    }

    fn alloc_msg(&mut self, cmd: i32) -> NlMsg {
        let ptr = unsafe { tinyln_rs_sys::unl_rtnl_msg(&mut self.unl, cmd, false) };
        assert!(!ptr.is_null(), "unl_rtnl_msg returned NULL");
        let mut msg = NlMsg::from_ptr(ptr);
        // unl_rtnl_msg sets flags=0; set NLM_F_REQUEST for all operations.
        msg.header_mut().nlmsg_flags = NLM_F_REQUEST | NLM_F_ACK;
        msg
    }

    fn send(&mut self, msg: &NlMsg) -> io::Result<()> {
        let ret = unsafe { tinyln_rs_sys::nl_send_auto_complete(self.unl.sock, msg.as_ptr()) };
        if ret < 0 { return Err(io::Error::from_raw_os_error(-ret)); }
        let ret = unsafe { tinyln_rs_sys::nl_wait_for_ack(self.unl.sock) };
        if ret < 0 { return Err(io::Error::from_raw_os_error(-ret)); }
        Ok(())
    }

    /// Create a 802.1Q VLAN sub-interface.
    pub fn add_vlan(&mut self, parent: &str, vid: u16) -> io::Result<String> {
        let name = format!("{parent}.{vid}");
        let parent_idx = ifindex(parent)?;

        let mut msg = self.alloc_msg(RTM_NEWLINK as i32);
        msg.header_mut().nlmsg_flags |= NLM_F_CREATE | NLM_F_EXCL;
        msg.append_struct(&build_ifi(0, 0, 0))?;

        attr::put_string(&msg, IFLA_IFNAME as i32, &name);
        attr::put_u32(&msg, IFLA_LINK as i32, parent_idx as u32);

        let li = attr::nest_start(&msg, IFLA_LINKINFO as i32)
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "nest_start IFLA_LINKINFO"))?;
        attr::put_string(&msg, IFLA_INFO_KIND as i32, "vlan");
        let data = attr::nest_start(&msg, IFLA_INFO_DATA as i32)
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "nest_start IFLA_INFO_DATA"))?;
        attr::put_u16(&msg, IFLA_VLAN_ID as i32, vid);
        attr::nest_end(&msg, data);
        attr::nest_end(&msg, li);

        self.send(&msg)?;
        Ok(name)
    }

    /// Delete an interface by name.
    pub fn del(&mut self, name: &str) -> io::Result<()> {
        let idx = ifindex(name)?;
        let msg = self.alloc_msg(RTM_DELLINK as i32);
        msg.append_struct(&build_ifi(idx, 0, 0))?;
        self.send(&msg)
    }

    /// Bring an interface up.
    pub fn set_up(&mut self, name: &str) -> io::Result<()> {
        self.set_flags(ifindex(name)?, IFF_UP, IFF_UP)
    }

    /// Bring an interface down.
    pub fn set_down(&mut self, name: &str) -> io::Result<()> {
        self.set_flags(ifindex(name)?, 0, IFF_UP)
    }

    fn set_flags(&mut self, idx: i32, flags: u32, change: u32) -> io::Result<()> {
        let msg = self.alloc_msg(RTM_SETLINK as i32);
        msg.append_struct(&build_ifi(idx, flags, change))?;
        self.send(&msg)
    }
}
