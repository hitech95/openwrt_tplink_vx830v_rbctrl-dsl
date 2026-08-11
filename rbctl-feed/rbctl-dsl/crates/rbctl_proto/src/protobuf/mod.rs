//! Protobuf protocol — self-describing messages on magic `0x50`.
//!
//! This is the new wire protocol that replaces the legacy OEM binary opcodes.
//! Frames carry protobuf-encoded `Request` / `Response` / `Event` messages.
//! The magic byte `0x50` (ASCII 'P') distinguishes protobuf frames from OEM
//! frames (`0x11`/`0x10`) on the same EtherType, allowing coexistence during
//! migration.
//!
//! The generated Rust types live in [`rbctl`] (compiled from `proto/rbctl.proto`
//! via `prost-build` + `protox`).

/// Magic byte: protobuf frame (ASCII 'P' for Protobuf).
///
/// Distinguishes from OEM magic `0x11` (command) / `0x10` (response).
pub const MAGIC_PROTOBUF: u8 = 0x50;

/// Fragment flag bit (OR'd into the magic byte when the protobuf message
/// spans multiple L2 frames).
pub const FRAG_FLAG: u8 = 0x80;

pub mod rbctl {
    include!(concat!(env!("OUT_DIR"), "/rbctl.rs"));
}
