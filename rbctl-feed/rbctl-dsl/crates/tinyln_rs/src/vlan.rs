//! VLAN sub-interface management via netlink (libnl-tiny).
//!
//! Creates and removes 802.1Q VLAN sub-interfaces using `RTM_NEWLINK` /
//! `RTM_DELLINK` rtnetlink messages — the same path as `ip link add type vlan`.
//! Interface up/down uses `RTM_SETLINK`.

use std::ffi::CString;
use std::io;
use std::os::raw::c_void;

use tinylibnl_sys::{ifinfomsg, nl_msg, nlattr, nlmsghdr, unl, IFLA_INFO_DATA,
    IFLA_INFO_KIND, IFLA_IFNAME, IFLA_LINK, IFLA_LINKINFO, IFLA_VLAN_ID,
    RTM_DELLINK, RTM_NEWLINK, RTM_SETLINK};

// ── netlink constants not exported by libnl-tiny ─────────────────────────

const NLA_F_NESTED: i32 = 0x8000;
const NLMSG_ALIGNTO: usize = 4;

// ── helpers reimplementing libnl-tiny static-inline functions ────────────

#[inline]
fn nlmsg_align(len: usize) -> usize {
    (len + NLMSG_ALIGNTO - 1) & !(NLMSG_ALIGNTO - 1)
}

/// Pointer to the end of the current message data (like `nlmsg_tail`).
#[inline]
unsafe fn nlmsg_tail_ptr(msg: *const nl_msg) -> *const u8 {
    let nlh = (*msg).nm_nlh;
    (nlh as *const u8).add(nlmsg_align((*nlh).nlmsg_len as usize))
}

/// `nla_put_u16(msg, attrtype, value)`
fn nla_put_u16(msg: *mut nl_msg, attrtype: i32, value: u16) -> i32 {
    let bytes = value.to_ne_bytes();
    unsafe { tinylibnl_sys::nla_put(msg, attrtype, 2, bytes.as_ptr() as *const c_void) }
}

/// `nla_put_u32(msg, attrtype, value)`
fn nla_put_u32(msg: *mut nl_msg, attrtype: i32, value: u32) -> i32 {
    let bytes = value.to_ne_bytes();
    unsafe { tinylibnl_sys::nla_put(msg, attrtype, 4, bytes.as_ptr() as *const c_void) }
}

/// `nla_put_string(msg, attrtype, cstr)`
fn nla_put_string(msg: *mut nl_msg, attrtype: i32, s: &CString) -> i32 {
    let bytes = s.to_bytes_with_nul();
    unsafe { tinylibnl_sys::nla_put(msg, attrtype, bytes.len() as i32, bytes.as_ptr() as *const c_void) }
}

/// `nla_nest_start(msg, attrtype)` — begin a nested attribute.
fn nla_nest_start(msg: *mut nl_msg, attrtype: i32) -> *mut nlattr {
    unsafe {
        let start = nlmsg_tail_ptr(msg as *const nl_msg) as *mut nlattr;
        if tinylibnl_sys::nla_put(msg, attrtype | NLA_F_NESTED, 0, std::ptr::null()) < 0 {
            return std::ptr::null_mut();
        }
        start
    }
}

/// `nla_nest_end(msg, start)` — finalize a nested attribute by writing its length.
fn nla_nest_end(msg: *mut nl_msg, start: *mut nlattr) -> i32 {
    unsafe {
        let tail = nlmsg_tail_ptr(msg as *const nl_msg);
        let size = (tail as usize) - (start as usize);
        (*start).nla_len = size as u16;
    }
    0
}

// ── misc helpers ─────────────────────────────────────────────────────────

const IFF_UP: u32 = 1;

fn check(ret: libc::c_int) -> io::Result<()> {
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn ifindex(name: &str) -> io::Result<libc::c_int> {
    let cname = CString::new(name).unwrap();
    let idx = unsafe { libc::if_nametoindex(cname.as_ptr()) };
    if idx == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(idx as libc::c_int)
    }
}

fn build_ifinfomsg(idx: libc::c_int, flags: u32, change: u32) -> ifinfomsg {
    ifinfomsg {
        ifi_family: libc::AF_UNSPEC as u8,
        __ifi_pad: 0,
        ifi_type: 0,
        ifi_index: idx,
        ifi_flags: flags,
        ifi_change: change,
    }
}

// ── public API ───────────────────────────────────────────────────────────

/// Netlink connection for VLAN / interface management, backed by libnl-tiny.
pub struct VlanControl {
    unl: unl,
}

impl Drop for VlanControl {
    fn drop(&mut self) {
        unsafe { tinylibnl_sys::unl_free(&mut self.unl) };
    }
}

impl VlanControl {
    /// Connect to the rtnetlink socket.
    pub fn new() -> io::Result<Self> {
        let mut unl: unl = unsafe { std::mem::zeroed() };
        check(unsafe { tinylibnl_sys::unl_rtnl_init(&mut unl) })?;
        Ok(Self { unl })
    }

    /// Create a 802.1Q VLAN sub-interface.
    pub fn add_vlan(&mut self, parent: &str, vid: u16) -> io::Result<String> {
        let name = format!("{parent}.{vid}");
        let parent_idx = ifindex(parent)?;
        let cname = CString::new(name.as_str()).unwrap();
        let ckind = CString::new("vlan").unwrap();

        let msg = unsafe { tinylibnl_sys::unl_rtnl_msg(&mut self.unl, RTM_NEWLINK as i32, false) };
        if msg.is_null() {
            return Err(io::Error::new(io::ErrorKind::Other, "unl_rtnl_msg NULL"));
        }

        let ifi = build_ifinfomsg(0, 0, 0);
        unsafe {
            tinylibnl_sys::nlmsg_append(
                msg,
                &ifi as *const _ as *mut c_void,
                std::mem::size_of::<ifinfomsg>(),
                NLMSG_ALIGNTO as i32,
            );
        }

        nla_put_string(msg, IFLA_IFNAME as i32, &cname);
        nla_put_u32(msg, IFLA_LINK as i32, parent_idx as u32);

        let linkinfo = nla_nest_start(msg, IFLA_LINKINFO as i32);
        nla_put_string(msg, IFLA_INFO_KIND as i32, &ckind);
        let data = nla_nest_start(msg, IFLA_INFO_DATA as i32);
        nla_put_u16(msg, IFLA_VLAN_ID as i32, vid);
        nla_nest_end(msg, data);
        nla_nest_end(msg, linkinfo);

        check(unsafe { tinylibnl_sys::nl_send_auto_complete(self.unl.sock, msg) })?;
        check(unsafe { tinylibnl_sys::nl_wait_for_ack(self.unl.sock) })?;
        unsafe { tinylibnl_sys::nlmsg_free(msg) };
        Ok(name)
    }

    /// Delete a VLAN sub-interface by name.
    pub fn del_vlan(&mut self, name: &str) -> io::Result<()> {
        let idx = ifindex(name)?;
        let msg = unsafe { tinylibnl_sys::unl_rtnl_msg(&mut self.unl, RTM_DELLINK as i32, false) };
        if msg.is_null() {
            return Err(io::Error::new(io::ErrorKind::Other, "unl_rtnl_msg NULL"));
        }

        let ifi = build_ifinfomsg(idx, 0, 0);
        unsafe {
            tinylibnl_sys::nlmsg_append(
                msg,
                &ifi as *const _ as *mut c_void,
                std::mem::size_of::<ifinfomsg>(),
                NLMSG_ALIGNTO as i32,
            );
        }

        check(unsafe { tinylibnl_sys::nl_send_auto_complete(self.unl.sock, msg) })?;
        check(unsafe { tinylibnl_sys::nl_wait_for_ack(self.unl.sock) })?;
        unsafe { tinylibnl_sys::nlmsg_free(msg) };
        Ok(())
    }

    /// Bring an interface up.
    pub fn set_up(&mut self, name: &str) -> io::Result<()> {
        self.set_flags(ifindex(name)?, IFF_UP, IFF_UP)
    }

    /// Bring an interface down.
    pub fn set_down(&mut self, name: &str) -> io::Result<()> {
        self.set_flags(ifindex(name)?, 0, IFF_UP)
    }

    fn set_flags(&mut self, idx: libc::c_int, flags: u32, change: u32) -> io::Result<()> {
        let msg = unsafe { tinylibnl_sys::unl_rtnl_msg(&mut self.unl, RTM_SETLINK as i32, false) };
        if msg.is_null() {
            return Err(io::Error::new(io::ErrorKind::Other, "unl_rtnl_msg NULL"));
        }

        let ifi = build_ifinfomsg(idx, flags, change);
        unsafe {
            tinylibnl_sys::nlmsg_append(
                msg,
                &ifi as *const _ as *mut c_void,
                std::mem::size_of::<ifinfomsg>(),
                NLMSG_ALIGNTO as i32,
            );
        }

        check(unsafe { tinylibnl_sys::nl_send_auto_complete(self.unl.sock, msg) })?;
        check(unsafe { tinylibnl_sys::nl_wait_for_ack(self.unl.sock) })?;
        unsafe { tinylibnl_sys::nlmsg_free(msg) };
        Ok(())
    }
}
