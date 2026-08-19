//! Generic, dependency-light helpers shared across the crate.
//!
//! Admission rule: a file lives here only if it depends on no specialized
//! types from other crate modules — external crates and self-defined
//! types only.

pub mod mapping;
pub mod mgmt_link;
pub mod ubus;
pub mod ubus_transport;
pub mod vlan;
