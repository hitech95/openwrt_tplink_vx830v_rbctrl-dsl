//! fwextract — extract the remote-board (xDSL) firmware from a TP-Link
//! multi-image container.
//!
//! Container layout (reverse-engineered from libcmm.so `rsl_sys_updateFirmware`):
//!
//! ```text
//! Offset 0x000  Tag header (512 bytes)
//!   [0x00] u32  image_version   (≥0x03000004 → RSA2048 PSS, older → RSA1024 PKCS)
//!   [0x34] u32  product_id
//!   [0x38] u32  product_ver
//!   [0x3C] u32  add_hw_ver
//!   [0x40] [u8;16]  md5_digest
//!   [0x70] u32  total_image_len  (main board image length, excluding tag header)
//!   [0x8C] u32  sw_revision
//!   [0x94] u32  special_ver
//!   [0xD0] [u8;256]  rsa_signature (RSA2048 PSS or RSA1024 PKCS)
//! Offset 0x200  Main board image (total_image_len bytes)
//! Offset 0x200 + total_image_len  Remote board image (remaining bytes)
//! ```
//!
//! The entire container is RSA-signed (signature stored in the tag header at
//! offset 0xD0). The remote board image extracted by this tool is raw flash
//! data — it is covered by the container signature but has no separate
//! signature of its own. The board performs its own checksum verification
//! during the `fw_verify` stage of the opcode-8 upload protocol.

use std::env;
use std::fs::File;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

const TAG_LEN: usize = 0x200; // 512-byte tag header
const TAG_OFF_VERSION: usize = 0x00;
const TAG_OFF_PRODUCT_ID: usize = 0x34;
const TAG_OFF_PRODUCT_VER: usize = 0x38;
const TAG_OFF_HW_VER: usize = 0x3C;
const TAG_OFF_MD5: usize = 0x40;
const TAG_OFF_TOTAL_IMAGE_LEN: usize = 0x70;
const TAG_OFF_SW_REV: usize = 0x8C;
const TAG_OFF_SPECIAL_VER: usize = 0x94;

// Minimum remote board image size (from remote_board: 0x200000 = 2 MB)
const MIN_REMOTE_SIZE: u32 = 0x20_0000;
// Maximum remote board image size (from remote_board: 0x800000 = 8 MB)
const MAX_REMOTE_SIZE: u32 = 0x80_0000;

struct TagHeader {
    image_version: u32,
    product_id: u32,
    product_ver: u32,
    hw_ver: u32,
    md5: [u8; 16],
    total_image_len: u32,
    sw_revision: u32,
    special_ver: u32,
}

impl TagHeader {
    fn parse(buf: &[u8]) -> Result<Self, String> {
        if buf.len() < TAG_LEN {
            return Err(format!(
                "file too small: {} bytes, need at least {} for tag header",
                buf.len(),
                TAG_LEN
            ));
        }
        Ok(Self {
            image_version: read_u32_le(buf, TAG_OFF_VERSION),
            product_id: read_u32_le(buf, TAG_OFF_PRODUCT_ID),
            product_ver: read_u32_le(buf, TAG_OFF_PRODUCT_VER),
            hw_ver: read_u32_le(buf, TAG_OFF_HW_VER),
            md5: buf[TAG_OFF_MD5..TAG_OFF_MD5 + 16].try_into().unwrap(),
            total_image_len: read_u32_le(buf, TAG_OFF_TOTAL_IMAGE_LEN),
            sw_revision: read_u32_le(buf, TAG_OFF_SW_REV),
            special_ver: read_u32_le(buf, TAG_OFF_SPECIAL_VER),
        })
    }

    fn signing_scheme(&self) -> &'static str {
        if self.image_version >= 0x0300_0004 {
            "RSA2048 PSS"
        } else {
            "RSA1024 PKCS#1 v1.5"
        }
    }
}

impl std::fmt::Display for TagHeader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "  image_version  : 0x{:08X}", self.image_version)?;
        writeln!(f, "  product_id     : 0x{:08X}", self.product_id)?;
        writeln!(f, "  product_ver    : 0x{:08X}", self.product_ver)?;
        writeln!(f, "  hw_ver         : 0x{:08X}", self.hw_ver)?;
        writeln!(f, "  sw_revision    : 0x{:08X}", self.sw_revision)?;
        writeln!(f, "  special_ver    : 0x{:08X}", self.special_ver)?;
        writeln!(f, "  md5_digest     : {}", hex(&self.md5))?;
        writeln!(f, "  total_image_len: {} (0x{:X})", self.total_image_len, self.total_image_len)?;
        writeln!(f, "  signing_scheme : {}", self.signing_scheme())?;
        Ok(())
    }
}

fn read_u32_le(buf: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

struct Usage;

impl Usage {
    fn print() {
        eprintln!(
            "Usage: fwextract [OPTIONS] <firmware.bin>\n\
             \n\
             Extract the remote-board (xDSL) firmware from a TP-Link multi-image container.\n\
             \n\
             Options:\n\
             \x20 -o, --output <PATH>    Write remote board image to PATH (default: <base>_remote.bin)\n\
             \x20 -a, --all              Also extract the main board image (<base>_main.bin)\n\
             \x20 -h, --help             Show this help\n\
             \n\
             The tool reads the 512-byte tag header to determine the split point,\n\
             then writes the remote board portion (the bytes after the main image).\n"
        );
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let mut fw_path: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    let mut extract_all = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                Usage::print();
                return ExitCode::SUCCESS;
            }
            "-o" | "--output" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("error: -o requires a path");
                    return ExitCode::from(2);
                }
                output = Some(PathBuf::from(&args[i]));
            }
            "-a" | "--all" => {
                extract_all = true;
            }
            s if s.starts_with('-') => {
                eprintln!("error: unknown option '{}'", s);
                Usage::print();
                return ExitCode::from(2);
            }
            _ => {
                if fw_path.is_some() {
                    eprintln!("error: multiple input files");
                    return ExitCode::from(2);
                }
                fw_path = Some(PathBuf::from(&args[i]));
            }
        }
        i += 1;
    }

    let fw_path = match fw_path {
        Some(p) => p,
        None => {
            Usage::print();
            return ExitCode::from(2);
        }
    };

    // Read the entire file
    let data = match std::fs::read(&fw_path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: cannot read '{}': {}", fw_path.display(), e);
            return ExitCode::from(1);
        }
    };

    // Parse the tag header
    let tag = match TagHeader::parse(&data) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: {}", e);
            return ExitCode::from(1);
        }
    };

    // Compute split point
    let split = TAG_LEN + tag.total_image_len as usize;
    if split > data.len() {
        eprintln!(
            "error: total_image_len ({}) + tag header ({}) exceeds file size ({})",
            tag.total_image_len,
            TAG_LEN,
            data.len()
        );
        return ExitCode::from(1);
    }

    let remote_data = &data[split..];
    let remote_len = remote_data.len() as u32;

    // Print info
    println!("=== Firmware container info ===");
    println!("{}", tag);
    println!("  file_size      : {} (0x{:X})", data.len(), data.len());
    println!("  tag_offset     : 0x{:X}", TAG_LEN);
    println!("  main_offset    : 0x{:X}", TAG_LEN);
    println!("  main_size      : {} (0x{:X})", tag.total_image_len, tag.total_image_len);
    println!("  remote_offset  : 0x{:X}", split);
    println!("  remote_size    : {} (0x{:X})", remote_len, remote_len);

    if remote_len == 0 {
        eprintln!("\nwarning: no remote board image in this container");
        eprintln!("         (total_image_len == file_size - tag_len, so there's nothing after the main image)");
        return ExitCode::SUCCESS;
    }

    if remote_len < MIN_REMOTE_SIZE || remote_len > MAX_REMOTE_SIZE {
        eprintln!(
            "\nwarning: remote image size {} (0x{:X}) is outside the board's accepted range",
            remote_len, remote_len
        );
        eprintln!(
            "         (expected {}–{} bytes, i.e. 2–8 MB)",
            MIN_REMOTE_SIZE, MAX_REMOTE_SIZE
        );
        eprintln!("         the data may be padding, a signature trailer, or a corrupt image");
    }

    // Determine output path
    let remote_out = output.unwrap_or_else(|| {
        let base = fw_path.file_stem().unwrap_or_default().to_string_lossy();
        fw_path
            .with_file_name(format!("{}_remote.bin", base))
    });

    // Write remote board image
    match write_file(&remote_out, remote_data) {
        Ok(()) => {
            println!("\nRemote board image written to: {}", remote_out.display());
        }
        Err(e) => {
            eprintln!("error: cannot write '{}': {}", remote_out.display(), e);
            return ExitCode::from(1);
        }
    }

    // Optionally write main board image
    if extract_all {
        let main_out = {
            let base = fw_path.file_stem().unwrap_or_default().to_string_lossy();
            fw_path
                .with_file_name(format!("{}_main.bin", base))
        };
        match write_file(&main_out, &data[..split]) {
            Ok(()) => {
                println!("Main board image written to: {} ({} bytes)", main_out.display(), split);
            }
            Err(e) => {
                eprintln!("error: cannot write '{}': {}", main_out.display(), e);
                return ExitCode::from(1);
            }
        }
    }

    ExitCode::SUCCESS
}

fn write_file(path: &PathBuf, data: &[u8]) -> io::Result<()> {
    let mut f = File::create(path)?;
    f.write_all(data)?;
    f.sync_all()?;
    Ok(())
}
