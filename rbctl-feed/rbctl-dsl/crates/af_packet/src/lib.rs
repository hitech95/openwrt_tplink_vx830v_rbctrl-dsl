//! `AF_PACKET` raw Ethernet socket for Linux.
//!
//! Creates a `SOCK_RAW` socket bound to a specific network interface. A
//! classic-BPF kernel filter limits delivery to a given ethertype. Because the
//! socket is typically bound to a VLAN sub-interface, the kernel transparently
//! adds/strips the 802.1Q tag — the application never sees or builds VLAN
//! headers.
//!
//! This crate depends only on `libc` — no external C libraries.

use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::Duration;

// ── BPF instruction codes (from <linux/filter.h>, stable UAPI) ───────────

const BPF_LD_H_ABS: u16 = 0x28; // BPF_LD | BPF_H | BPF_ABS
const BPF_JEQ_K: u16 = 0x15; // BPF_JMP | BPF_JEQ | BPF_K
const BPF_RET_K: u16 = 0x06; // BPF_RET | BPF_K

// ── helpers ──────────────────────────────────────────────────────────────

fn check(ret: libc::c_int) -> io::Result<()> {
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Parse a MAC address from `XX:XX:XX:XX:XX:XX` format.
fn parse_mac(s: &str) -> io::Result<[u8; 6]> {
    let mac: Vec<u8> = s
        .trim()
        .split(':')
        .map(|b| {
            u8::from_str_radix(b, 16)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
        })
        .collect::<io::Result<Vec<_>>>()?;
    if mac.len() != 6 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid MAC: expected 6 octets, got {}", mac.len()),
        ));
    }
    Ok(mac.try_into().unwrap())
}

/// Read the MAC address of a network interface from
/// `/sys/class/net/<iface>/address`.
fn read_mac(iface: &str) -> io::Result<[u8; 6]> {
    let s = std::fs::read_to_string(format!("/sys/class/net/{iface}/address"))?;
    parse_mac(&s)
}

/// Convert an ethertype to the BPF `jeq` constant.
///
/// The kernel BPF interpreter uses `get_unaligned_be16()` for `ldh`, which
/// always returns the big-endian interpretation of the wire bytes. So the
/// constant is simply the ethertype value itself (e.g. 0x88B5).
fn ethertype_bpf_k(ethertype: u16) -> u32 {
    ethertype as u32
}

/// Build a 4-instruction classic-BPF program matching the given ethertype.
fn build_bpf(ethertype: u16) -> [libc::sock_filter; 4] {
    [
        libc::sock_filter { code: BPF_LD_H_ABS, jt: 0, jf: 0, k: 12 },
        libc::sock_filter { code: BPF_JEQ_K, jt: 1, jf: 0, k: ethertype_bpf_k(ethertype) },
        libc::sock_filter { code: BPF_RET_K, jt: 0, jf: 0, k: 0 },
        libc::sock_filter { code: BPF_RET_K, jt: 0, jf: 0, k: 0xFFFF },
    ]
}

// ── public API ───────────────────────────────────────────────────────────

/// Raw `AF_PACKET` socket bound to a specific interface.
pub struct RawSocket {
    fd: OwnedFd,
    ifindex: libc::c_int,
    local_mac: [u8; 6],
    ethertype: u16,
}

/// Packet received: frame bytes + sender MAC.
pub struct RxPacket<'a> {
    pub data: &'a [u8],
    pub src_mac: [u8; 6],
}

impl RawSocket {
    /// Create and configure a raw socket on `iface` (e.g. `"lan0.500"`).
    ///
    /// Performs: `socket()` → `SO_BROADCAST` → read MAC → `SO_ATTACH_FILTER`
    /// → `bind(sockaddr_ll)`.
    pub fn open(iface: &str, ethertype: u16) -> io::Result<Self> {
        let fd =
            unsafe { libc::socket(libc::AF_PACKET, libc::SOCK_RAW, ethertype.to_be() as i32) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };

        // SO_BROADCAST — allow sending to FF:FF:FF:FF:FF:FF
        let one: libc::c_int = 1;
        unsafe {
            check(libc::setsockopt(
                fd.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_BROADCAST,
                &one as *const _ as *const libc::c_void,
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            ))?;
        }

        // MAC from /sys/class/net — no ioctl constant needed
        let local_mac = read_mac(iface)?;

        // if_nametoindex
        let cname = CString::new(iface).unwrap();
        let ifindex = unsafe { libc::if_nametoindex(cname.as_ptr()) };
        if ifindex == 0 {
            return Err(io::Error::last_os_error());
        }

        // SO_ATTACH_FILTER — classic BPF for ethertype matching
        let mut filter = build_bpf(ethertype);
        let fprog = libc::sock_fprog {
            len: filter.len() as u16,
            filter: filter.as_mut_ptr(),
        };
        unsafe {
            check(libc::setsockopt(
                fd.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_ATTACH_FILTER,
                &fprog as *const _ as *const libc::c_void,
                std::mem::size_of::<libc::sock_fprog>() as libc::socklen_t,
            ))?;
        }

        // bind(sockaddr_ll) — receive only from this interface
        let sa = libc::sockaddr_ll {
            sll_family: libc::AF_PACKET as u16,
            sll_protocol: ethertype.to_be(),
            sll_ifindex: ifindex as i32,
            sll_hatype: 0,
            sll_pkttype: 0,
            sll_halen: 0,
            sll_addr: [0; 8],
        };
        unsafe {
            check(libc::bind(
                fd.as_raw_fd(),
                &sa as *const _ as *const libc::sockaddr,
                std::mem::size_of::<libc::sockaddr_ll>() as libc::socklen_t,
            ))?;
        }

        Ok(Self {
            fd,
            ifindex: ifindex as i32,
            local_mac,
            ethertype,
        })
    }

    pub fn local_mac(&self) -> [u8; 6] {
        self.local_mac
    }

    pub fn set_timeout(&self, timeout: Duration) -> io::Result<()> {
        let tv = libc::timeval {
            tv_sec: timeout.as_secs() as libc::time_t,
            tv_usec: timeout.subsec_micros() as i64,
        };
        unsafe {
            check(libc::setsockopt(
                self.fd.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_RCVTIMEO,
                &tv as *const _ as *const libc::c_void,
                std::mem::size_of::<libc::timeval>() as libc::socklen_t,
            ))?;
        }
        Ok(())
    }

    pub fn send(&self, frame: &[u8]) -> io::Result<usize> {
        let sa = libc::sockaddr_ll {
            sll_family: libc::AF_PACKET as u16,
            sll_protocol: self.ethertype.to_be(),
            sll_ifindex: self.ifindex,
            sll_hatype: 1, // ARPHRD_ETHER
            sll_pkttype: 0,
            sll_halen: 6,
            sll_addr: [0; 8],
        };
        let ret = unsafe {
            libc::sendto(
                self.fd.as_raw_fd(),
                frame.as_ptr() as *const libc::c_void,
                frame.len(),
                0,
                &sa as *const _ as *const libc::sockaddr,
                std::mem::size_of::<libc::sockaddr_ll>() as libc::socklen_t,
            )
        };
        if ret < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(ret as usize)
        }
    }

    pub fn recv<'a>(&self, buf: &'a mut [u8]) -> io::Result<RxPacket<'a>> {
        let mut sa: libc::sockaddr_ll = unsafe { std::mem::zeroed() };
        let mut sa_len = std::mem::size_of::<libc::sockaddr_ll>() as libc::socklen_t;

        let ret = unsafe {
            libc::recvfrom(
                self.fd.as_raw_fd(),
                buf.as_mut_ptr() as *mut libc::c_void,
                buf.len(),
                0,
                &mut sa as *mut _ as *mut libc::sockaddr,
                &mut sa_len,
            )
        };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(RxPacket {
            data: &buf[..ret as usize],
            src_mac: sa.sll_addr[..6].try_into().unwrap(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_mac ────────────────────────────────────────────────────

    #[test]
    fn parse_mac_valid() {
        assert_eq!(parse_mac("00:11:22:33:44:55").unwrap(), [0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);
        assert_eq!(parse_mac("ff:ff:ff:ff:ff:ff").unwrap(), [0xff; 6]);
        assert_eq!(parse_mac("AA:BB:CC:DD:EE:FF").unwrap(), [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
    }

    #[test]
    fn parse_mac_with_newline() {
        assert_eq!(parse_mac("00:11:22:33:44:55\n").unwrap(), [0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);
    }

    #[test]
    fn parse_mac_too_short() {
        assert!(parse_mac("00:11:22:33:44").is_err());
    }

    #[test]
    fn parse_mac_invalid_hex() {
        assert!(parse_mac("00:11:22:33:44:GG").is_err());
    }

    // ── BPF ──────────────────────────────────────────────────────────

    #[test]
    fn bpf_filter_has_4_instructions() {
        assert_eq!(build_bpf(0x88B5).len(), 4);
    }

    #[test]
    fn bpf_filter_instruction_0_loads_ethertype() {
        let f = build_bpf(0x88B5);
        assert_eq!(f[0].code, BPF_LD_H_ABS);
        assert_eq!(f[0].k, 12); // offset 12 in Ethernet header
    }

    #[test]
    fn bpf_filter_instruction_1_compares_ethertype() {
        let f = build_bpf(0x88B5);
        assert_eq!(f[1].code, BPF_JEQ_K);
        assert_eq!(f[1].jt, 1); // if match: skip reject → accept
        assert_eq!(f[1].jf, 0); // if no match: fall to reject
    }

    #[test]
    fn bpf_filter_reject_then_accept() {
        let f = build_bpf(0x88B5);
        assert_eq!(f[2].code, BPF_RET_K);
        assert_eq!(f[2].k, 0);       // reject = return 0 bytes
        assert_eq!(f[3].code, BPF_RET_K);
        assert_eq!(f[3].k, 0xFFFF);  // accept = return up to 65535 bytes
    }

    // ── ethertype_bpf_k ──────────────────────────────────────────────

    #[test]
    fn ethertype_bpf_constant() {
        // Kernel's BPF interpreter uses get_unaligned_be16, so the constant
        // is always the ethertype value itself, regardless of host endianness.
        assert_eq!(ethertype_bpf_k(0x88B5), 0x88B5);
        assert_eq!(ethertype_bpf_k(0x0800), 0x0800);
        assert_eq!(ethertype_bpf_k(0x8100), 0x8100);
    }

    #[test]
    fn ethertype_bpf_round_trips() {
        // For any ethertype, the BPF constant equals the ethertype value.
        for &et in &[0x0800u16, 0x0806, 0x8100, 0x88B5, 0x88B6, 0x86DD] {
            assert_eq!(ethertype_bpf_k(et), et as u32);
        }
    }
}
