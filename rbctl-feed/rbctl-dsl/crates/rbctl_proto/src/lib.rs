//! rbctl_proto — the `0x88B5` board management protocol core.
//!
//! Pure Rust, zero C dependencies. Builds on any host (no SDK required).
//!
//! ## Modules
//!
//! | Module | Purpose | Ported from |
//! |---|---|---|
//! | [`checksum`] | CRC-16/ARC frame checksum (set / verify) | `examples/checksum.py` |
//! | [`frame`] | 24-byte `proto_frame_hdr` builder + sequence counter | `docs/protocol.md` |
//! | [`pack`] | TX payload encoders (opcodes 1 / 5 / 15 / 6 / 16) | `examples/pack.py` |
//! | [`unpack`] | RX payload decoders (opcodes 2 / 4) | `examples/unpack.py` |
//! | [`validate`] | Config validation guard (modulation × annex × profile) | — |
//!
//! ## Wire protocol constants
//!
//! | Constant | Value | Meaning |
//! |---|---|---|
//! | `ETHTYPE_BOARD` | `0x88B5` | board management EtherType |
//! | `MAGIC_COMMAND` | `0x11` | TX (host → board) |
//! | `MAGIC_RESPONSE` | `0x10` | RX (board → host) |
//! | `HEADER_LEN` | `24` | bytes (`0x18`), payload starts after |
//! | `MIN_FRAME` | `60` | minimum Ethernet frame (padded by sender) |

#![cfg_attr(not(test), no_std)]

pub mod checksum;
pub mod firmware;
pub mod frame;
pub mod pack;
pub mod unpack;
pub mod validate;

pub use checksum::{set_checksum, verify_checksum};
pub use frame::{Frame, SeqCounter, build_command_frame, ETHTYPE_BOARD, HEADER_LEN, MIN_FRAME};

/// Magic byte: command frame (host → board).
pub const MAGIC_COMMAND: u8 = 0x11;
/// Magic byte: response frame (board → host).
pub const MAGIC_RESPONSE: u8 = 0x10;
