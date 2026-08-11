//! rbctl_proto — the `0x88B5` board management protocol core.
//!
//! Pure Rust, zero C dependencies. Builds on any host (no SDK required).
//!
//! ## Modules
//!
//! | Module | Purpose |
//! |---|---|
//! | [`checksum`] | CRC-16/ARC frame checksum (set / verify) |
//! | [`frame`] | 24-byte frame builder (protocol-agnostic) + sequence counter |
//! | [`oem`] | OEM binary protocol (magic `0x11`/`0x10`): pack / unpack / firmware / validate |
//!
//! ## Wire protocol constants
//!
//! | Constant | Value | Meaning |
//! |---|---|---|
//! | `ETHTYPE_BOARD` | `0x88B5` | board management EtherType |
//! | `oem::MAGIC_OEM_COMMAND` | `0x11` | TX (host → board) |
//! | `oem::MAGIC_OEM_RESPONSE` | `0x10` | RX (board → host) |
//! | `HEADER_LEN` | `24` | bytes (`0x18`), payload starts after |
//! | `MIN_FRAME` | `60` | minimum Ethernet frame (padded by sender) |

#![cfg_attr(not(test), no_std)]

// ── Shared layer ───────────────────────────────────────────────────────

pub mod checksum;
pub mod frame;

// ── OEM binary protocol (magic 0x11/0x10) ─────────────────────────────

pub mod oem;

// ── Shared re-exports (convenience) ────────────────────────────────────

pub use checksum::{set_checksum, verify_checksum};
pub use frame::{build_frame, Frame, SeqCounter, ETHTYPE_BOARD, HEADER_LEN, MIN_FRAME};
