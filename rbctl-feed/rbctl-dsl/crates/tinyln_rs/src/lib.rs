//! tinyln-rs — safe Rust wrappers for OpenWrt libnl-tiny.
//!
//! A comprehensive netlink library for Rust, wrapping the OpenWrt `libnl-tiny`
//! C library. Provides safe abstractions for socket management, message
//! construction, attribute handling, routing netlink (link/addr/route),
//! and generic netlink.
//!
//! # Modules
//!
//! | Module | libnl-tiny header | Purpose |
//! |--------|-------------------|---------|
//! | [`socket`] | `<netlink/netlink.h>` | `NlSocket` — alloc, connect, send, recv |
//! | [`msg`] | `<netlink/msg.h>` | `NlMsg` — message builder |
//! | [`attr`] | `<netlink/attr.h>` | typed attribute put/get, nesting |
//! | [`cb`] | `<netlink/handlers.h>` | `NlCb` — callback dispatch |
//! | [`rtnl`] | `<linux/rtnetlink.h>` | `RtnlLink`, `RtnlAddr`, `RtnlRoute` |
//! | [`genl`] | `<netlink/genl/genl.h>` | `GenlSocket` — generic netlink |
//! | [`unl`] | `<unl.h>` | `Unl` — high-level micro-netlink |

pub mod attr;
pub mod cb;
pub mod genl;
pub mod msg;
pub mod rtnl;
pub mod socket;
pub mod unl;

pub use socket::NlSocket;
pub use msg::NlMsg;
pub use cb::NlCb;

// ── netlink constants not in libnl-tiny (from <linux/netlink.h>) ────────

pub const NETLINK_ROUTE: i32 = 0;
pub const NETLINK_GENERIC: i32 = 16;

pub const NLMSG_ALIGNTO: usize = 4;
pub const NLA_ALIGNTO: usize = 4;
pub const NLA_HDRLEN: usize = 4; // sizeof(struct nlattr)
pub const NLA_F_NESTED: i32 = 0x8000;
pub const NLA_F_NET_BYTEORDER: i32 = 0x4000;

#[inline]
pub fn nlmsg_align(len: usize) -> usize {
    (len + NLMSG_ALIGNTO - 1) & !(NLMSG_ALIGNTO - 1)
}

#[inline]
pub fn nla_align(len: usize) -> usize {
    (len + NLA_ALIGNTO - 1) & !(NLA_ALIGNTO - 1)
}

// ── error helper ─────────────────────────────────────────────────────────

pub(crate) fn check(ret: libc::c_int) -> std::io::Result<()> {
    if ret < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nlmsg_align_values() {
        assert_eq!(nlmsg_align(0), 0);
        assert_eq!(nlmsg_align(1), 4);
        assert_eq!(nlmsg_align(4), 4);
        assert_eq!(nlmsg_align(5), 8);
        assert_eq!(nlmsg_align(16), 16);
        assert_eq!(nlmsg_align(17), 20);
    }

    #[test]
    fn nla_align_values() {
        assert_eq!(nla_align(0), 0);
        assert_eq!(nla_align(1), 4);
        assert_eq!(nla_align(3), 4);
        assert_eq!(nla_align(4), 4);
        assert_eq!(nla_align(5), 8);
        assert_eq!(nla_align(7), 8);
    }

    #[test]
    fn constants_match_kernel_uapi() {
        assert_eq!(NLMSG_ALIGNTO, 4);
        assert_eq!(NLA_ALIGNTO, 4);
        assert_eq!(NLA_HDRLEN, 4);
        assert_eq!(NLA_F_NESTED, 0x8000);
        assert_eq!(NETLINK_ROUTE, 0);
        assert_eq!(NETLINK_GENERIC, 16);
    }
}
