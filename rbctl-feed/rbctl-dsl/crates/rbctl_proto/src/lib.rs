//! rbctl_proto — the `0x88B5` board management protocol core.
//!
//! One crate, two wire protocols, each in its own submodule:
//!
//! | Protocol | Magic | Module | Description |
//! |---|---|---|---|
//! | **OEM binary** | `0x11`/`0x10` | [`oem`] | Fixed binary opcodes (legacy) |
//! | **Protobuf** | `0x50` | [`protobuf`] (`protobuf` feature) | Self-describing messages |
//!
//! Both share the same L2 frame format (24-byte header, CRC-16/ARC, EtherType
//! `0x88B5`) — implemented in the shared [`frame`] and [`checksum`] modules.
//! Only the magic byte and payload encoding differ.
//!
//! ## Layout
//!
//! ```text
//! src/
//! ├── lib.rs          ← crate root, shared re-exports
//! ├── checksum.rs     ← CRC-16/ARC (shared by both protocols)
//! ├── frame.rs        ← Frame parser + build_frame (shared, magic-agnostic)
//! ├── oem/            ← OEM binary protocol (magic 0x11/0x10)
//! │   ├── mod.rs      ← MAGIC_OEM_COMMAND/RESPONSE, build_command_frame
//! │   ├── pack.rs     ← Request encoders (opcodes 1/5/15/6/16)
//! │   ├── unpack.rs   ← Response decoders (opcodes 2/4)
//! │   ├── firmware.rs ← 2RDH header parser + stage constants
//! │   └── validate.rs ← Config validation guard
//! └── protobuf/       ← Protobuf protocol (magic 0x50, needs `protobuf` feature)
//!     └── mod.rs      ← MAGIC_PROTOBUF, generated message types
//! ```
//!
//! ## `no_std` compatibility
//!
//! Without the `protobuf` feature, the crate is `no_std` (the OEM core has no
//! allocator dependency). Enabling `protobuf` pulls in `prost` which requires
//! `std` + `alloc`.

#![cfg_attr(not(any(test, feature = "protobuf")), no_std)]

// ── Shared layer (both protocols) ──────────────────────────────────────

pub mod checksum;
pub mod frame;

// ── OEM binary protocol (magic 0x11/0x10) ─────────────────────────────

pub mod oem;

// ── Protobuf protocol (magic 0x50) ────────────────────────────────────

#[cfg(feature = "protobuf")]
pub mod protobuf;

// ── Shared re-exports (convenience) ────────────────────────────────────

pub use checksum::{set_checksum, verify_checksum};
pub use frame::{build_frame, Frame, SeqCounter, ETHTYPE_BOARD, HEADER_LEN, MIN_FRAME};
