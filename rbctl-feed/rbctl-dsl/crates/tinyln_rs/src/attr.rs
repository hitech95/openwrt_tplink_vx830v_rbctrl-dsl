//! Netlink attribute helpers — typed put/get, nesting, finding.
//!
//! The `static inline` functions from `<netlink/attr.h>` are reimplemented
//! here in safe Rust on top of the exported `nla_put()`.

use std::ffi::CString;
use std::os::raw::c_void;
use std::ptr;

use tinyln_rs_sys::nlattr;

use crate::{nla_align, NLA_F_NESTED, NLA_HDRLEN};

// ── put (write) helpers ──────────────────────────────────────────────────

/// Put raw bytes as a netlink attribute.
pub fn put(msg: &crate::msg::NlMsg, attrtype: i32, data: &[u8]) -> i32 {
    unsafe {
        tinyln_rs_sys::nla_put(
            msg.as_ptr(),
            attrtype,
            data.len() as i32,
            if data.is_empty() {
                ptr::null()
            } else {
                data.as_ptr() as *const c_void
            },
        )
    }
}

pub fn put_u8(msg: &crate::msg::NlMsg, attrtype: i32, val: u8) -> i32 {
    put(msg, attrtype, &[val])
}

pub fn put_u16(msg: &crate::msg::NlMsg, attrtype: i32, val: u16) -> i32 {
    put(msg, attrtype, &val.to_ne_bytes())
}

pub fn put_u32(msg: &crate::msg::NlMsg, attrtype: i32, val: u32) -> i32 {
    put(msg, attrtype, &val.to_ne_bytes())
}

pub fn put_u64(msg: &crate::msg::NlMsg, attrtype: i32, val: u64) -> i32 {
    put(msg, attrtype, &val.to_ne_bytes())
}

pub fn put_string(msg: &crate::msg::NlMsg, attrtype: i32, s: &str) -> i32 {
    // Map interior-NUL failure to a negative return (nla_put's error
    // convention) instead of panicking. Interface names / "vlan" literals
    // never contain NUL, but user-supplied strings flow through here.
    match CString::new(s) {
        Ok(cstr) => put(msg, attrtype, cstr.to_bytes_with_nul()),
        Err(_) => -1,
    }
}

pub fn put_flag(msg: &crate::msg::NlMsg, attrtype: i32) -> i32 {
    unsafe { tinyln_rs_sys::nla_put(msg.as_ptr(), attrtype, 0, ptr::null()) }
}

// ── nest helpers ─────────────────────────────────────────────────────────

/// Opaque handle to an open nested attribute.
///
/// Stores the **byte offset** from the `nlmsghdr` to the nest `nlattr`, not an
/// absolute pointer. This survives `nla_put`-triggered reallocations of the
/// message buffer between [`nest_start`] and [`nest_end`] — capturing a raw
/// pointer (the old API) would dangle if any child `put_*` grew the buffer.
pub struct Nest {
    offset: usize,
}

/// Begin a nested attribute. Returns a handle to pass to [`nest_end`].
///
/// Returns `None` if the nest-header `nla_put` fails (e.g. ENOMEM).
pub fn nest_start(msg: &crate::msg::NlMsg, attrtype: i32) -> Option<Nest> {
    let offset = unsafe {
        let nlh = (*msg.as_ptr()).nm_nlh;
        crate::nlmsg_align((*nlh).nlmsg_len as usize)
    };
    if unsafe { tinyln_rs_sys::nla_put(msg.as_ptr(), attrtype | NLA_F_NESTED, 0, ptr::null()) } < 0 {
        return None;
    }
    Some(Nest { offset })
}

/// Finalize a nested attribute by writing its total length.
///
/// Re-derives the nest header position from `msg` + the stored offset, so the
/// write lands in the current (possibly reallocated) buffer.
pub fn nest_end(msg: &crate::msg::NlMsg, nest: Nest) {
    let nlh = unsafe { (*msg.as_ptr()).nm_nlh };
    let base = nlh as *const u8;
    let tail = unsafe { base.add(crate::nlmsg_align((*nlh).nlmsg_len as usize)) };
    let start = unsafe { base.add(nest.offset) as *mut nlattr };
    // tail >= start always holds: nest_end is called after the child puts that
    // grew the message past the nest header.
    let size = (tail as usize) - (start as usize);
    unsafe { (*start).nla_len = size as u16 };
}

// ── get (read) helpers ───────────────────────────────────────────────────
//
// All getters validate the attribute payload is large enough before reading.
// A malformed/truncated attribute (e.g. `nla_len` smaller than the type) would
// otherwise read out of bounds — netlink messages are untrusted wire data.

/// Read a `u8` from an attribute. `None` if the payload is too small or
/// the attribute is malformed.
pub fn get_u8(attr: &nlattr) -> Option<u8> {
    let p = data_ptr(attr);
    if payload_len(attr)? >= 1 {
        Some(unsafe { *p })
    } else {
        None
    }
}

/// Read a `u16` from an attribute (native endian). `None` if too small.
pub fn get_u16(attr: &nlattr) -> Option<u16> {
    let p = data_ptr(attr);
    if payload_len(attr)? >= 2 {
        Some(unsafe { *(p as *const u16) })
    } else {
        None
    }
}

/// Read a `u32` from an attribute (native endian). `None` if too small.
pub fn get_u32(attr: &nlattr) -> Option<u32> {
    let p = data_ptr(attr);
    if payload_len(attr)? >= 4 {
        Some(unsafe { *(p as *const u32) })
    } else {
        None
    }
}

/// Read a `u64` from an attribute (native endian). `None` if too small.
pub fn get_u64(attr: &nlattr) -> Option<u64> {
    let p = data_ptr(attr);
    if payload_len(attr)? >= 8 {
        Some(unsafe { *(p as *const u64) })
    } else {
        None
    }
}

/// Read a NUL-terminated string from an attribute.
///
/// Returns `None` if the attribute is malformed or the payload is not valid
/// UTF-8 (netlink strings are arbitrary bytes; never assume valid UTF-8).
pub fn get_string(attr: &nlattr) -> Option<&str> {
    let len = payload_len(attr)?;
    let p = data_ptr(attr);
    let slice = unsafe { std::slice::from_raw_parts(p, len) };
    let end = slice.iter().position(|&b| b == 0).unwrap_or(len);
    std::str::from_utf8(&slice[..end]).ok()
}

/// Pointer to the attribute payload (past the `nlattr` header).
pub fn data_ptr(attr: &nlattr) -> *const u8 {
    unsafe { (attr as *const nlattr as *const u8).add(NLA_HDRLEN) }
}

/// Payload length: total `nla_len` minus the `nlattr` header.
///
/// Returns `None` if the attribute is malformed (`nla_len < NLA_HDRLEN`),
/// guarding against underflow on untrusted wire data.
pub fn payload_len(attr: &nlattr) -> Option<usize> {
    (attr.nla_len as usize).checked_sub(NLA_HDRLEN)
}

/// Iterate over attributes in a buffer. Returns each valid `nlattr`.
pub struct NlaIter<'a> {
    ptr: *const nlattr,
    remaining: i32,
    _phantom: std::marker::PhantomData<&'a nlattr>,
}

impl<'a> NlaIter<'a> {
    pub fn new(start: *const nlattr, len: usize) -> Self {
        Self { ptr: start, remaining: len as i32, _phantom: Default::default() }
    }
}

impl<'a> Iterator for NlaIter<'a> {
    type Item = &'a nlattr;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining <= 0 {
            return None;
        }
        if unsafe { tinyln_rs_sys::nla_ok(self.ptr, self.remaining) } == 0 {
            return None;
        }
        let attr = unsafe { &*self.ptr };
        let total = nla_align(attr.nla_len as usize);
        self.ptr = unsafe { (self.ptr as *const u8).add(total) as *const nlattr };
        self.remaining -= total as i32;
        Some(attr)
    }
}

/// Find an attribute by type in an attribute stream.
pub fn find(start: *const nlattr, len: usize, attrtype: i32) -> *const nlattr {
    unsafe { tinyln_rs_sys::nla_find(start as *mut nlattr, len as i32, attrtype) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::msg::NlMsg;

    fn attr_region(msg: &NlMsg) -> (*const nlattr, usize) {
        let nlh = msg.header();
        let hdr_size = crate::nlmsg_align(16); // NLMSG_HDRLEN = sizeof(nlmsghdr) aligned
        let base = nlh as *const _ as usize;
        let start = (base + hdr_size) as *const nlattr;
        let len = nlh.nlmsg_len as usize - hdr_size;
        (start, len)
    }

    #[test]
    fn put_get_u16_roundtrip() {
        let msg = NlMsg::alloc_simple(0x10, 0x01).unwrap();
        put_u16(&msg, 1, 0x1234);
        let (start, len) = attr_region(&msg);
        let found = find(start, len, 1);
        assert!(!found.is_null());
        assert_eq!(get_u16(unsafe { &*found }), Some(0x1234));
    }

    #[test]
    fn put_get_u32_roundtrip() {
        let msg = NlMsg::alloc_simple(0x10, 0x01).unwrap();
        put_u32(&msg, 2, 0xDEADBEEF);
        let (start, len) = attr_region(&msg);
        let found = find(start, len, 2);
        assert!(!found.is_null());
        assert_eq!(get_u32(unsafe { &*found }), Some(0xDEADBEEF));
    }

    #[test]
    fn put_get_string_roundtrip() {
        let msg = NlMsg::alloc_simple(0x10, 0x01).unwrap();
        put_string(&msg, 3, "vlan");
        let (start, len) = attr_region(&msg);
        let found = find(start, len, 3);
        assert!(!found.is_null());
        assert_eq!(get_string(unsafe { &*found }), Some("vlan"));
    }

    #[test]
    fn put_flag_roundtrip() {
        let msg = NlMsg::alloc_simple(0x10, 0x01).unwrap();
        put_flag(&msg, 99);
        let (start, len) = attr_region(&msg);
        let found = find(start, len, 99);
        assert!(!found.is_null());
        assert_eq!(payload_len(unsafe { &*found }), Some(0)); // flag has no payload
    }

    /// A properly-aligned `nlattr` + payload so `data_ptr` reads stay in-bounds.
    #[repr(C)]
    struct TestAttr {
        hdr: nlattr,
        payload: [u8; 8],
    }

    #[test]
    fn get_u32_rejects_truncated_attr() {
        // nla_len claims 5 → payload_len = 1; too small for a u32 read.
        let a = TestAttr { hdr: nlattr { nla_len: (NLA_HDRLEN as u16) + 1, nla_type: 0 }, payload: [0; 8] };
        assert_eq!(get_u32(&a.hdr), None);
        // u8 needs >=1 byte → succeeds (reads payload[0]).
        assert_eq!(get_u8(&a.hdr), Some(0));
    }

    #[test]
    fn get_string_rejects_invalid_utf8() {
        // 2 payload bytes of 0xFF 0xFF — not valid UTF-8.
        let a = TestAttr {
            hdr: nlattr { nla_len: (NLA_HDRLEN as u16) + 2, nla_type: 0 },
            payload: [0xFF, 0xFF, 0, 0, 0, 0, 0, 0],
        };
        assert_eq!(get_string(&a.hdr), None);
    }

    #[test]
    fn payload_len_none_on_underflow() {
        // nla_len < NLA_HDRLEN → malformed, must not underflow.
        let a = TestAttr { hdr: nlattr { nla_len: 2, nla_type: 0 }, payload: [0; 8] };
        assert_eq!(payload_len(&a.hdr), None);
        assert_eq!(get_u32(&a.hdr), None); // underflowing len must not enable a read
    }

    #[test]
    fn nla_iter_visits_all() {
        let msg = NlMsg::alloc_simple(0x10, 0x01).unwrap();
        put_u8(&msg, 1, 42);
        put_u16(&msg, 2, 0x1234);
        put_u32(&msg, 3, 0xDEADBEEF);
        put_string(&msg, 4, "hello");

        let (start, len) = attr_region(&msg);
        let types: Vec<u16> = NlaIter::new(start, len).map(|a| a.nla_type).collect();
        assert_eq!(types, vec![1, 2, 3, 4]);
    }

    #[test]
    fn nest_produces_children() {
        let msg = NlMsg::alloc_simple(0x10, 0x01).unwrap();
        let outer = nest_start(&msg, 10).expect("nest_start outer");
        put_u16(&msg, 11, 0xABCD);
        put_string(&msg, 12, "inner");
        nest_end(&msg, outer);

        let (start, len) = attr_region(&msg);
        let outer_ptr = find(start, len, 10);
        assert!(!outer_ptr.is_null());

        let outer_attr = unsafe { &*outer_ptr };
        let inner_start = data_ptr(outer_attr) as *const nlattr;
        let inner_len = payload_len(outer_attr).expect("outer payload len");
        let types: Vec<u16> = NlaIter::new(inner_start, inner_len).map(|a| a.nla_type).collect();
        assert_eq!(types, vec![11, 12]);
    }
}
