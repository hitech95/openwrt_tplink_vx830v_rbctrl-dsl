//! Minimal pcap (libpcap) file writer — raw-Edition capture only.
//!
//! Writes a standard little-endian pcap file with `LINKTYPE_ETHERNET`
//! (`network = 1`), openable directly in Wireshark / tcpdump. The format is a
//! 24-byte global header followed by per-packet records (16-byte record header
//! + frame bytes). Hand-rolled here to avoid pulling in a crate for ~40 lines
//! of a fixed, stable binary format.
//!
//! Only depends on `std`; no allocation on the hot path (records are written
//! straight to the file).

use std::fs::File;
use std::io::{self, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// pcap magic (little-endian microsecond resolution).
const PCAP_MAGIC: u32 = 0xa1b2c3d4;
const VERSION_MAJOR: u16 = 2;
const VERSION_MINOR: u16 = 4;
/// Maximum captured length we advertise. Frames fit well within this.
const SNAPLEN: u32 = 65535;
/// `LINKTYPE_ETHERNET` — the captured bytes are full Ethernet frames.
const LINKTYPE_ETHERNET: u32 = 1;

/// Open `path` for writing and emit the pcap global header.
///
/// The file is created (or truncated) and each subsequent [`Writer::write`]
/// appends one packet record. Writes go straight to the kernel page cache via
/// `write(2)`, so capture data survives a Ctrl+C / SIGINT kill without an
/// explicit flush (unlike a userspace `BufWriter`).
pub fn create(path: impl AsRef<Path>) -> io::Result<File> {
    let mut f = File::create(path)?;
    let mut hdr = [0u8; 24];
    hdr[0x00..0x04].copy_from_slice(&PCAP_MAGIC.to_le_bytes());
    hdr[0x04..0x06].copy_from_slice(&VERSION_MAJOR.to_le_bytes());
    hdr[0x06..0x08].copy_from_slice(&VERSION_MINOR.to_le_bytes());
    // hdr[0x08..0x0c] thiszone = 0 (already zero)
    // hdr[0x0c..0x10] sigfigs = 0 (already zero)
    hdr[0x10..0x14].copy_from_slice(&SNAPLEN.to_le_bytes());
    hdr[0x14..0x18].copy_from_slice(&LINKTYPE_ETHERNET.to_le_bytes());
    f.write_all(&hdr)?;
    Ok(f)
}

/// Append one Ethernet frame as a pcap record.
///
/// `now` is captured outside (at receive time) so the timestamp reflects when
/// the frame arrived, not when it is written. `incl_len == orig_len == frame`
/// (we never truncate).
pub fn write(writer: &mut impl Write, now: SystemTime, frame: &[u8]) -> io::Result<()> {
    let dur = now.duration_since(UNIX_EPOCH).unwrap_or_default();
    let ts_sec = dur.as_secs() as u32;
    let ts_usec = dur.subsec_micros() as u32;
    let len = frame.len() as u32;

    let mut rec = [0u8; 16];
    rec[0x00..0x04].copy_from_slice(&ts_sec.to_le_bytes());
    rec[0x04..0x08].copy_from_slice(&ts_usec.to_le_bytes());
    rec[0x08..0x0c].copy_from_slice(&len.to_le_bytes()); // incl_len
    rec[0x0c..0x10].copy_from_slice(&len.to_le_bytes()); // orig_len
    writer.write_all(&rec)?;
    writer.write_all(frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn global_header_is_24_bytes_and_correct() {
        // Build the header into a buffer by creating a temp pcap and reading it back.
        let tmp = std::env::temp_dir().join(format!("rbctl_pcap_test_{}.pcap", std::process::id()));
        {
            let _f = create(&tmp).unwrap();
        }
        let bytes = std::fs::read(&tmp).unwrap();
        let _ = std::fs::remove_file(&tmp);
        assert_eq!(bytes.len(), 24);
        assert_eq!(u32::from_le_bytes(bytes[0x00..0x04].try_into().unwrap()), PCAP_MAGIC);
        assert_eq!(u16::from_le_bytes(bytes[0x04..0x06].try_into().unwrap()), VERSION_MAJOR);
        assert_eq!(u16::from_le_bytes(bytes[0x06..0x08].try_into().unwrap()), VERSION_MINOR);
        assert_eq!(u32::from_le_bytes(bytes[0x10..0x14].try_into().unwrap()), SNAPLEN);
        assert_eq!(u32::from_le_bytes(bytes[0x14..0x18].try_into().unwrap()), LINKTYPE_ETHERNET);
    }

    #[test]
    fn write_appends_16_byte_record_header_plus_frame() {
        let mut buf = Cursor::new(Vec::<u8>::new());
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::new(0x11223344, 5_000_000);
        let frame = [0xffu8; 14]; // a tiny "ethernet" frame
        write(&mut buf, now, &frame).unwrap();
        let out = buf.into_inner();

        assert_eq!(out.len(), 16 + frame.len());
        assert_eq!(u32::from_le_bytes(out[0x00..0x04].try_into().unwrap()), 0x11223344);
        assert_eq!(u32::from_le_bytes(out[0x04..0x08].try_into().unwrap()), 5_000);
        let len = frame.len() as u32;
        assert_eq!(u32::from_le_bytes(out[0x08..0x0c].try_into().unwrap()), len); // incl_len
        assert_eq!(u32::from_le_bytes(out[0x0c..0x10].try_into().unwrap()), len); // orig_len
        assert_eq!(&out[16..], &frame);
    }

    #[test]
    fn multiple_writes_are_contiguous_records() {
        let mut buf = Cursor::new(Vec::<u8>::new());
        let now = SystemTime::UNIX_EPOCH;
        write(&mut buf, now, &[0x01, 0x02]).unwrap();
        write(&mut buf, now, &[0x03, 0x04, 0x05]).unwrap();
        let out = buf.into_inner();
        // record 1: 16 + 2, record 2: 16 + 3
        assert_eq!(out.len(), (16 + 2) + (16 + 3));
    }
}
