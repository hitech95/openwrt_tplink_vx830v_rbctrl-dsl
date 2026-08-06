//! Frame checksum — CRC-16/ARC over the covered region.
//!
//! Algorithm: poly `0x8005`, init `0x0000`, refin = refout = true, xorout `0x0000`
//! (the canonical CRC-16/ARC profile). Delegated entirely to the [`crc`] crate
//! — we do not maintain a nibble table.
//!
//! The checksum covers `frame[0x0E .. 0x0E + payload_len + 10]` with the
//! checksum field itself treated as zero. See `docs/checksum.md`.
//!
//! [`crc`]: https://crates.io/crates/crc

use crate::frame::{OFF_CHECKSUM, OFF_MAGIC, OFF_PAYLOAD_LEN};
use crc::{Crc, CRC_16_ARC};

/// One-time CRC-16/ARC engine (const, no runtime init).
const CRC: Crc<u16> = Crc::<u16>::new(&CRC_16_ARC);

/// Magic constants from the protocol spec.
const COVER_HEADER_BYTES: usize = 10; // magic(1)+subtype(1)+seq(4)+payload_len(2)+checksum(2)

/// Compute CRC-16/ARC over an arbitrary byte slice.
pub fn compute_checksum(data: &[u8]) -> u16 {
    let mut digest = CRC.digest();
    digest.update(data);
    digest.finalize()
}

/// Read the wire `payload_len` field (offset `0x14`, big-endian u16).
fn read_payload_len(frame: &[u8]) -> usize {
    u16::from_be_bytes([frame[OFF_PAYLOAD_LEN], frame[OFF_PAYLOAD_LEN + 1]]) as usize
}

/// Compute the checksum over the covered region, treating the checksum field as
/// zero. Used by both [`set_checksum`] and [`verify_checksum`].
///
/// We split the CRC feed into two slices — before and after the checksum field —
/// so we never need to mutate the frame (no allocation, `no_std`-safe).
fn crc_covered_region(frame: &[u8]) -> u16 {
    let payload_len = read_payload_len(frame);
    let cover_end = OFF_MAGIC + payload_len + COVER_HEADER_BYTES;
    assert!(
        frame.len() >= cover_end,
        "frame too short: need {cover_len}, have {len}",
        cover_len = cover_end,
        len = frame.len(),
    );
    let mut digest = CRC.digest();
    // Part 1: magic through payload_len (offsets 0x0E..0x16).
    digest.update(&frame[OFF_MAGIC..OFF_CHECKSUM]);
    // Part 2: the checksum field itself — treated as zero during computation.
    // We feed explicit zeros because CRC-16/ARC processes zero bytes
    // non-trivially (8 rounds of polynomial division per byte).
    digest.update(&[0u8, 0u8]);
    // Part 3: payload_type + payload (offsets 0x18..cover_end).
    digest.update(&frame[OFF_CHECKSUM + 2..cover_end]);
    digest.finalize()
}

/// **TX:** compute the CRC and store it big-endian at the checksum field.
///
/// The frame must already hold wire-order bytes (multi-byte fields
/// big-endian) and a valid `payload_len` at offset `0x14`.
pub fn set_checksum(frame: &mut [u8]) {
    let crc = crc_covered_region(frame);
    frame[OFF_CHECKSUM..OFF_CHECKSUM + 2].copy_from_slice(&crc.to_be_bytes());
}

/// **RX:** verify the stored checksum against a fresh computation.
///
/// Returns `true` on match, `false` on mismatch (or frame too short).
pub fn verify_checksum(frame: &[u8]) -> bool {
    let payload_len = match frame.len().checked_sub(OFF_MAGIC + COVER_HEADER_BYTES) {
        Some(n) if n >= read_payload_len(frame) => read_payload_len(frame),
        _ => return false,
    };
    let cover_end = OFF_MAGIC + payload_len + COVER_HEADER_BYTES;
    if frame.len() < cover_end {
        return false;
    }
    let stored = u16::from_be_bytes([frame[OFF_CHECKSUM], frame[OFF_CHECKSUM + 1]]);
    stored == crc_covered_region(frame)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reproduce the synthetic `dsl_config_down` frame from `examples/checksum.py`.
    /// Opcode 3, no payload, seq = 1, broadcast, magic `0x11` (command). Expected
    /// checksum: `0x1ea0`.
    ///
    /// NOTE: an earlier version of the Python example used magic `0x10` (wrongly
    /// labelled "command magic"). Ghidra analysis of `proto_frame_init` confirmed
    /// the real command magic is `0x11`. The vector changed from `0xe2a4` → `0x1ea0`.
    fn build_sample_frame() -> Vec<u8> {
        let mut f = vec![0u8; 25]; // 14 (eth) + 10 (covered header) + 1 (payload_type)
        f[0x00..0x06].copy_from_slice(&[0xff; 6]); // dst = broadcast
        f[0x06..0x0c].copy_from_slice(&[0x00, 0x11, 0x22, 0x33, 0x44, 0x55]); // src
        f[0x0c..0x0e].copy_from_slice(&0x88B5u16.to_be_bytes()); // ethertype
        f[0x0e] = 0x11; // magic = command (confirmed via Ghidra proto_frame_init)
        f[0x0f] = 0x03; // subtype = opcode 3
        f[0x10..0x14].copy_from_slice(&1u32.to_be_bytes()); // seq = 1
        f[0x14..0x16].copy_from_slice(&1u16.to_be_bytes()); // payload_len = 1
        f[0x16..0x18].copy_from_slice(&0u16.to_be_bytes()); // checksum placeholder
        f[0x18] = 0x00; // payload_type
        f
    }

    #[test]
    fn known_vector_e2a4() {
        let mut f = build_sample_frame();
        set_checksum(&mut f);
        let cs = u16::from_be_bytes([f[OFF_CHECKSUM], f[OFF_CHECKSUM + 1]]);
        assert_eq!(cs, 0x1ea0, "checksum must match the reference vector");
    }

    #[test]
    fn round_trip_set_verify() {
        let mut f = build_sample_frame();
        set_checksum(&mut f);
        assert!(verify_checksum(&f), "verify must pass after set");
    }

    #[test]
    fn corruption_detected() {
        let mut f = build_sample_frame();
        set_checksum(&mut f);
        f[OFF_MAGIC + 2] ^= 0x01; // flip one bit in the covered region
        assert!(!verify_checksum(&f), "verify must reject corrupted frame");
    }

    #[test]
    fn uncovered_byte_ignored() {
        let mut f = build_sample_frame();
        set_checksum(&mut f);
        f[0] ^= 0x01; // flip dst_mac (offset 0x00 — NOT in covered region)
        assert!(verify_checksum(&f), "dst_mac change must not affect checksum");
    }
}
