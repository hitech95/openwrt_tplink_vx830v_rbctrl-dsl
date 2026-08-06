//! Netlink attribute helpers — typed put/get, nesting, finding.
//!
//! The `static inline` functions from `<netlink/attr.h>` are reimplemented
//! here in safe Rust on top of the exported `nla_put()`.

use std::ffi::CString;
use std::os::raw::c_void;
use std::ptr;

use tinyln_rs_sys::{nl_msg, nlattr};

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
    let cstr = CString::new(s).unwrap();
    let bytes = cstr.to_bytes_with_nul();
    put(msg, attrtype, bytes)
}

pub fn put_flag(msg: &crate::msg::NlMsg, attrtype: i32) -> i32 {
    unsafe { tinyln_rs_sys::nla_put(msg.as_ptr(), attrtype, 0, ptr::null()) }
}

// ── nest helpers ─────────────────────────────────────────────────────────

/// Begin a nested attribute. Returns a handle to pass to [`nest_end`].
pub fn nest_start(msg: &crate::msg::NlMsg, attrtype: i32) -> *mut nlattr {
    let nlh = unsafe { (*msg.as_ptr()).nm_nlh };
    let start = unsafe {
        (nlh as *const u8).add(crate::nlmsg_align((*nlh).nlmsg_len as usize)) as *mut nlattr
    };
    if unsafe { tinyln_rs_sys::nla_put(msg.as_ptr(), attrtype | NLA_F_NESTED, 0, ptr::null()) } < 0 {
        return ptr::null_mut();
    }
    start
}

/// Finalize a nested attribute by writing its total length.
pub fn nest_end(msg: &crate::msg::NlMsg, start: *mut nlattr) -> i32 {
    let nlh = unsafe { (*msg.as_ptr()).nm_nlh };
    let tail = unsafe { (nlh as *const u8).add(crate::nlmsg_align((*nlh).nlmsg_len as usize)) };
    let size = (tail as usize) - (start as usize);
    unsafe { (*start).nla_len = size as u16 };
    0
}

// ── get (read) helpers ───────────────────────────────────────────────────

/// Read a `u8` from an attribute.
pub fn get_u8(attr: &nlattr) -> u8 {
    unsafe { *(data_ptr(attr) as *const u8) }
}

/// Read a `u16` from an attribute (native endian).
pub fn get_u16(attr: &nlattr) -> u16 {
    unsafe { *(data_ptr(attr) as *const u16) }
}

/// Read a `u32` from an attribute (native endian).
pub fn get_u32(attr: &nlattr) -> u32 {
    unsafe { *(data_ptr(attr) as *const u32) }
}

/// Read a `u64` from an attribute (native endian).
pub fn get_u64(attr: &nlattr) -> u64 {
    unsafe { *(data_ptr(attr) as *const u64) }
}

/// Read a NUL-terminated string from an attribute.
pub fn get_string(attr: &nlattr) -> &str {
    let len = nla_len(attr);
    let ptr = data_ptr(attr) as *const libc::c_char;
    unsafe {
        let slice = std::slice::from_raw_parts(ptr as *const u8, len);
        let end = slice.iter().position(|&b| b == 0).unwrap_or(len);
        std::str::from_utf8_unchecked(&slice[..end])
    }
}

/// Pointer to the attribute payload (past the `nlattr` header).
pub fn data_ptr(attr: &nlattr) -> *const u8 {
    unsafe { (attr as *const nlattr as *const u8).add(NLA_HDRLEN) }
}

/// Payload length (total `nla_len` minus header).
pub fn nla_len(attr: &nlattr) -> usize {
    attr.nla_len as usize - NLA_HDRLEN
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
        assert_eq!(get_u16(unsafe { &*found }), 0x1234);
    }

    #[test]
    fn put_get_u32_roundtrip() {
        let msg = NlMsg::alloc_simple(0x10, 0x01).unwrap();
        put_u32(&msg, 2, 0xDEADBEEF);
        let (start, len) = attr_region(&msg);
        let found = find(start, len, 2);
        assert!(!found.is_null());
        assert_eq!(get_u32(unsafe { &*found }), 0xDEADBEEF);
    }

    #[test]
    fn put_get_string_roundtrip() {
        let msg = NlMsg::alloc_simple(0x10, 0x01).unwrap();
        put_string(&msg, 3, "vlan");
        let (start, len) = attr_region(&msg);
        let found = find(start, len, 3);
        assert!(!found.is_null());
        assert_eq!(get_string(unsafe { &*found }), "vlan");
    }

    #[test]
    fn put_flag_roundtrip() {
        let msg = NlMsg::alloc_simple(0x10, 0x01).unwrap();
        put_flag(&msg, 99);
        let (start, len) = attr_region(&msg);
        let found = find(start, len, 99);
        assert!(!found.is_null());
        assert_eq!(nla_len(unsafe { &*found }), 0); // flag has no payload
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
        let outer = nest_start(&msg, 10);
        put_u16(&msg, 11, 0xABCD);
        put_string(&msg, 12, "inner");
        nest_end(&msg, outer);

        let (start, len) = attr_region(&msg);
        let outer_ptr = find(start, len, 10);
        assert!(!outer_ptr.is_null());

        let outer_attr = unsafe { &*outer_ptr };
        let inner_start = data_ptr(outer_attr) as *const nlattr;
        let inner_len = nla_len(outer_attr);
        let types: Vec<u16> = NlaIter::new(inner_start, inner_len).map(|a| a.nla_type).collect();
        assert_eq!(types, vec![11, 12]);
    }
}
