//! Firmware upgrade protocol — opcode 8 frame helpers and `2RDH` header
//! validation.
//!
//! The opcode-8 firmware upload is a 4-stage request/response protocol:
//!
//! ```text
//! stage 0  ANNOUNCE   host → board: image size     board: erases flash
//! stage 1  STREAM     host → board: 1 KB chunks    board: ACK(last_good)
//! stage 2  VERIFY     host → board: (empty)        board: checksum flash
//! stage 3  COMPLETE   host → board: (empty)        board: finalize + reboot
//! ```
//!
//! See `docs/commands/firmware.md` for the full wire-level analysis and
//! `docs/server.md` for the board-side handler (`board_op8_handler`).

// ─── Stage constants ────────────────────────────────────────────────────

/// Payload byte 0 for each stage.
pub const STAGE_ANNOUNCE: u8 = 0;
pub const STAGE_STREAM: u8 = 1;
pub const STAGE_VERIFY: u8 = 2;
pub const STAGE_COMPLETE: u8 = 3;

/// Chunk size for the streaming phase (from host-side `fw_stream`).
pub const CHUNK_SIZE: usize = 1024;

/// Maximum in-flight chunks before waiting for an ACK (sliding window).
pub const WINDOW_SIZE: usize = 100;

// ─── Timeouts (from host-side `firmware_upgrade` analysis) ──────────────

/// Announce stage: board erases flash, needs a moment.
pub const TIMEOUT_ANNOUNCE_MS: u64 = 1500;
pub const RETRIES_ANNOUNCE: u8 = 5;

/// Stream stage: per-ACK wait.
pub const TIMEOUT_STREAM_MS: u64 = 300;
pub const RETRIES_STREAM: u8 = 20;

/// Verify stage: board checksums the full image (up to 8 MB).
pub const TIMEOUT_VERIFY_MS: u64 = 60_000;
pub const RETRIES_VERIFY: u8 = 20;

/// Complete stage: board writes boot flag and may reboot.
pub const TIMEOUT_COMPLETE_MS: u64 = 60_000;
pub const RETRIES_COMPLETE: u8 = 20;

// ─── Payload builders (fixed-size, no_std friendly) ─────────────────────

/// Build the announce payload: `[stage=0, u32 size_be]`.
pub const fn announce_payload(image_size: u32) -> [u8; 5] {
    [
        STAGE_ANNOUNCE,
        (image_size >> 24) as u8,
        (image_size >> 16) as u8,
        (image_size >> 8) as u8,
        image_size as u8,
    ]
}

/// Build the verify payload: `[stage=2]`.
pub const fn verify_payload() -> [u8; 1] {
    [STAGE_VERIFY]
}

/// Build the complete payload: `[stage=3]`.
pub const fn complete_payload() -> [u8; 1] {
    [STAGE_COMPLETE]
}

// ─── 2RDH header parser ─────────────────────────────────────────────────

/// Minimum and maximum image sizes accepted by the board's opcode-8 handler.
pub const MIN_IMAGE_SIZE: usize = 0x20_0000; // 2 MB
pub const MAX_IMAGE_SIZE: usize = 0x80_0000; // 8 MB

/// `2RDH` magic bytes.
pub const MAGIC_2RDH: [u8; 4] = *b"2RDH";

/// Parsed `2RDH` / `tclinux.trx` header (256 bytes, big-endian fields).
///
/// Confirmed by the OpenWrt econet target `tclinux-trx.sh` and board-side
/// analysis. See `docs/firmware_encryption.md`.
#[derive(Debug, Clone)]
pub struct FwHeader {
    pub header_len: u32,
    pub total_len: u32,
    pub crc32: u32,
    pub version: [u8; 31],
    pub kernel_len: u32,
    pub rootfs_len: u32,
    pub load_addr: u32,
}

/// Parse a 2RDH header from the first 256 bytes of a firmware image.
pub fn parse_header(data: &[u8]) -> Result<FwHeader, &'static str> {
    if data.len() < 0x100 {
        return Err("file too small for 2RDH header (need 256 bytes)");
    }
    if data[..4] != MAGIC_2RDH {
        return Err("not a 2RDH firmware image");
    }

    // All fields are big-endian per tclinux-trx.sh
    let u32_be = |off: usize| -> u32 {
        u32::from_be_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
    };

    let header_len = u32_be(0x04);
    if header_len != 0x100 {
        return Err("2RDH header_len is not 256");
    }

    let mut version = [0u8; 31];
    version.copy_from_slice(&data[0x10..0x2F]);

    Ok(FwHeader {
        header_len,
        total_len: u32_be(0x08),
        crc32: u32_be(0x0C),
        version,
        kernel_len: u32_be(0x50),
        rootfs_len: u32_be(0x54),
        load_addr: u32_be(0x7C),
    })
}

/// Validate a firmware image: header + size range + total_len match.
pub fn validate_image(data: &[u8]) -> Result<FwHeader, &'static str> {
    let hdr = parse_header(data)?;

    if data.len() < MIN_IMAGE_SIZE {
        return Err("image too small (must be >= 2 MB)");
    }
    if data.len() > MAX_IMAGE_SIZE {
        return Err("image too large (must be <= 8 MB)");
    }
    if hdr.total_len as usize != data.len() {
        return Err("2RDH total_len does not match file size");
    }

    Ok(hdr)
}

/// Return the version string (null-terminated ASCII), borrowed.
pub fn version_str(hdr: &FwHeader) -> &str {
    let end = hdr.version.iter().position(|&b| b == 0).unwrap_or(31);
    core::str::from_utf8(&hdr.version[..end]).unwrap_or("(invalid utf8)")
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn build_2rdh(size: u32) -> Vec<u8> {
        let mut buf = vec![0u8; 0x100];
        buf[..4].copy_from_slice(&MAGIC_2RDH);
        // header_len = 256 (BE)
        buf[0x04..0x08].copy_from_slice(&0x100u32.to_be_bytes());
        // total_len (BE)
        buf[0x08..0x0C].copy_from_slice(&size.to_be_bytes());
        // version string
        let ver = b"7.3.261.1_v016\n";
        buf[0x10..0x10 + ver.len()].copy_from_slice(ver);
        // kernel_len and rootfs_len (dummy)
        buf[0x50..0x54].copy_from_slice(&1000u32.to_be_bytes());
        buf[0x54..0x58].copy_from_slice(&2000u32.to_be_bytes());
        // load_addr
        buf[0x7C..0x80].copy_from_slice(&0x80002000u32.to_be_bytes());
        // Pad to the claimed size
        buf.resize(size as usize, 0xFF);
        buf
    }

    #[test]
    fn parse_valid_header() {
        let data = build_2rdh(0x20_0000);
        let hdr = parse_header(&data).unwrap();
        assert_eq!(hdr.header_len, 0x100);
        assert_eq!(hdr.total_len, 0x20_0000);
        assert_eq!(version_str(&hdr), "7.3.261.1_v016\n");
        assert_eq!(hdr.load_addr, 0x80002000);
    }

    #[test]
    fn reject_bad_magic() {
        let mut data = build_2rdh(0x20_0000);
        data[..4].copy_from_slice(b"XXXX");
        assert!(parse_header(&data).is_err());
    }

    #[test]
    fn validate_size_range() {
        assert!(validate_image(&build_2rdh(0x20_0000)).is_ok());
        assert!(validate_image(&build_2rdh(0x80_0000)).is_ok());
        assert!(validate_image(&build_2rdh(0x1F_FFFF)).is_err()); // too small
        assert!(validate_image(&build_2rdh(0x80_0001)).is_err()); // too large
    }

    #[test]
    fn validate_total_len_mismatch() {
        let mut data = build_2rdh(0x20_0000);
        // Corrupt total_len to not match
        data[0x08..0x0C].copy_from_slice(&0x30_0000u32.to_be_bytes());
        assert!(validate_image(&data).is_err());
    }

    #[test]
    fn announce_payload_layout() {
        let pl = announce_payload(0x12345678);
        assert_eq!(pl[0], STAGE_ANNOUNCE);
        assert_eq!(&pl[1..5], &0x12345678u32.to_be_bytes());
    }

    #[test]
    fn verify_payload_layout() {
        assert_eq!(verify_payload(), [STAGE_VERIFY]);
    }

    #[test]
    fn complete_payload_layout() {
        assert_eq!(complete_payload(), [STAGE_COMPLETE]);
    }
}
