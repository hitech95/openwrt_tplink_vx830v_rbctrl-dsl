//! OEM binary protocol — fixed binary opcodes on magic `0x11`/`0x10`.
//!
//! This is the legacy wire protocol used by the original `remote_board`
//! daemon. The board receives requests with magic `0x11` and responds with
//! magic `0x10`. Payloads are packed binary structs (not protobuf).
//!
//! ## Modules
//!
//! | Module | Purpose |
//! |---|---|
//! | [`pack`] | TX payload encoders (opcodes 1 / 5 / 15 / 6 / 16) |
//! | [`unpack`] | RX payload decoders (opcodes 2 / 4) |
//! | [`firmware`] | 2RDH header parser, stage constants, payload builders |
//! | [`validate`] | Config validation guard (modulation × annex × profile) |

pub mod firmware;
pub mod pack;
pub mod unpack;
pub mod validate;

// Re-export commonly used types at the oem:: level.
pub use firmware::{
    announce_payload, complete_payload, parse_header, validate_image, verify_payload,
    FwHeader, MAGIC_2RDH, MAX_IMAGE_SIZE, MIN_IMAGE_SIZE, STAGE_ANNOUNCE, STAGE_COMPLETE,
    STAGE_STREAM, STAGE_VERIFY, CHUNK_SIZE,
};
pub use pack::{Annex, Modulation, Vdsl2Profiles};
pub use unpack::{DataPath, LinkStatus};

/// Magic byte: OEM command/request frame (host → board).
pub const MAGIC_OEM_COMMAND: u8 = 0x11;

/// Magic byte: OEM response frame (board → host).
pub const MAGIC_OEM_RESPONSE: u8 = 0x10;

const BROADCAST_MAC: [u8; 6] = [0xff; 6];

// ── OEM frame builder ──────────────────────────────────────────────────

/// Fill `buf` with an OEM command frame (magic `0x11`, dst=broadcast) and
/// compute the checksum.
///
/// This is a thin convenience wrapper over
/// [`crate::frame::build_frame`] that hardcodes the OEM command magic and
/// broadcast destination (matching the original `remote_board` client
/// behaviour).
///
/// `payload` is the full bytes from offset `0x18` onward (byte 0 = payload_type).
/// Returns the slice of `buf` holding the complete frame.
pub fn build_command_frame<'a>(
    buf: &'a mut [u8],
    subtype: u8,
    seq: u32,
    src_mac: &[u8; 6],
    payload: &[u8],
) -> &'a mut [u8] {
    crate::frame::build_frame(
        buf,
        &BROADCAST_MAC,
        src_mac,
        MAGIC_OEM_COMMAND,
        subtype,
        seq,
        payload,
    )
}

/// Fill `buf` with an OEM response frame (magic `0x10`).
///
/// Used by the board-side agent to respond to host requests.
pub fn build_response_frame<'a>(
    buf: &'a mut [u8],
    dst_mac: &[u8; 6],
    src_mac: &[u8; 6],
    opcode: u8,
    seq: u32,
    payload: &[u8],
) -> &'a mut [u8] {
    crate::frame::build_frame(
        buf,
        dst_mac,
        src_mac,
        MAGIC_OEM_RESPONSE,
        opcode,
        seq,
        payload,
    )
}
