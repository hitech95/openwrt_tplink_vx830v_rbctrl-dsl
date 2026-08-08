//! fwextract — extract and optionally decrypt the remote-board (xDSL) firmware
//! from a TP-Link multi-image container.
//!
//! ## Basic usage (split only, no decryption)
//!
//! ```text
//! fwextract firmware.bin --split output/
//! ```
//!
//! The output files are still AES-encrypted. Use `--decrypt` for plaintext.
//!
//! ## Decryption (requires `--features decrypt`)
//!
//! ```text
//! # Extract RSA public key from libcmm.so (one-time)
//! python3 examples/extract_rsa_keys.py libcmm.so --output-dir keys/
//!
//! # Decrypt and split
//! fwextract --decrypt --rsa-key keys/rsa2048_pub.bin firmware.bin --split output/
//! ```
//!
//! ## Container layout
//!
//! Reverse-engineered from `libcmm.so` `rsl_sys_updateFirmware` and
//! `checkFwSignNoTag`. See `docs/firmware_encryption.md` for full details.
//!
//! ```text
//! Offset 0x000   Tag header (512 bytes, plaintext)
//! Offset 0x200   Chunk metadata (type=1 RSA-PSS sig, type=0 terminator)
//! Offset tagLen  AES-128-CBC encrypted firmware data
//!                  [0 .. kernel_len)      Host UBI image
//!                  [kernel_len .. end)    Remote board firmware ("2RDH")
//! ```
//!
//! The AES key is embedded inside the RSA-2048 PSS signature's salt — it is
//! NOT a static value. See `docs/firmware_encryption.md` and
//! `examples/extract_rsa_keys.py`.

use std::env;
use std::fs::File;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

const TAG_HEADER_LEN: usize = 0x200;
const TAG_OFF_VERSION: usize = 0x00;
const TAG_OFF_PRODUCT_ID: usize = 0x34;
const TAG_OFF_PRODUCT_VER: usize = 0x38;
const TAG_OFF_HW_VER: usize = 0x3C;
const TAG_OFF_MD5: usize = 0x40;
const TAG_OFF_TOTAL_IMAGE_LEN: usize = 0x70;
const TAG_OFF_SW_REV: usize = 0x8C;
const TAG_OFF_SPECIAL_VER: usize = 0x94;

// Remote board image size range (from remote_board: 2–8 MB)
const MIN_REMOTE_SIZE: u32 = 0x20_0000;
const MAX_REMOTE_SIZE: u32 = 0x80_0000;

// ── Tag header ──────────────────────────────────────────────────────────

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
        if buf.len() < TAG_HEADER_LEN {
            return Err(format!(
                "file too small: {} bytes, need at least 0x200 for tag header",
                buf.len()
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
            "RSA2048 PSS (+ AES decrypt)"
        } else {
            "RSA1024 PKCS#1 v1.5 (no encryption)"
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
        writeln!(
            f,
            "  total_image_len: {} (0x{:X})",
            self.total_image_len, self.total_image_len
        )?;
        writeln!(f, "  signing_scheme : {}", self.signing_scheme())?;
        Ok(())
    }
}

// ── Chunk metadata / getTagLen ──────────────────────────────────────────

/// Parse the chunk metadata area starting at offset 0x200.
///
/// Returns `(tag_len, signature_bytes)` where `tag_len` is the full
/// header+metadata length (typically 0x330) and `signature_bytes` is the
/// raw RSA signature extracted from the type-1 chunk.
fn parse_chunks(data: &[u8]) -> (usize, Vec<u8>) {
    let mut offset = TAG_HEADER_LEN;
    let mut total = 0usize;
    let mut signature = Vec::new();

    while offset + 8 <= data.len() {
        let chunk_type = read_u32_le(data, offset);
        let chunk_len = read_u32_le(data, offset + 4) as usize;
        if chunk_len < 8 || offset + chunk_len > data.len() {
            break;
        }
        if chunk_type == 0 {
            total += chunk_len;
            break;
        }
        if chunk_type == 1 && signature.is_empty() {
            let sig_start = offset + 8;
            let sig_end = (sig_start + chunk_len - 8).min(sig_start + 256);
            signature = data[sig_start..sig_end].to_vec();
        }
        total += chunk_len;
        offset += chunk_len;
    }

    (TAG_HEADER_LEN + total, signature)
}

// ── Decryption module (feature-gated) ───────────────────────────────────

#[cfg(feature = "decrypt")]
mod decrypt {
    use aes::cipher::generic_array::GenericArray;
    use aes::cipher::{BlockDecrypt, KeyInit};
    use aes::Aes128;
    use num_bigint_dig::BigUint;
    use sha2::{Digest, Sha256};

    /// Parsed RSA public key from a CryptoAPI PUBLICKEYBLOB.
    pub struct RsaPublicKey {
        pub n: BigUint,
        pub e: BigUint,
    }

    /// Parse a CryptoAPI PUBLICKEYBLOB (raw binary, not base64).
    ///
    /// Layout:
    /// ```text
    /// [0]  BYTE  bType      = 0x06
    /// [1]  BYTE  bVersion   = 0x02
    /// [4]  DWORD aiKeyAlg
    /// [8]  DWORD magic      = "RSA1"
    /// [12] DWORD bitlen
    /// [16] DWORD pubexp
    /// [20] BYTE[bitlen/8]  modulus (little-endian)
    /// ```
    pub fn parse_rsa_pubkey_blob(blob: &[u8]) -> Result<RsaPublicKey, String> {
        if blob.len() < 20 {
            return Err("blob too short".into());
        }
        if blob[0] != 0x06 {
            return Err(format!("not a PUBLICKEYBLOB (bType=0x{:02X})", blob[0]));
        }
        if &blob[8..12] != b"RSA1" {
            return Err("magic is not RSA1".into());
        }
        let bitlen = u32::from_le_bytes(blob[12..16].try_into().unwrap()) as usize;
        let pubexp = u32::from_le_bytes(blob[16..20].try_into().unwrap());
        let modulus_len = bitlen / 8;
        if blob.len() < 20 + modulus_len {
            return Err("blob truncated".into());
        }
        // CryptoAPI stores modulus in little-endian; BigUint::from_bytes_be expects BE
        let modulus_le = &blob[20..20 + modulus_len];
        let modulus_be: Vec<u8> = modulus_le.iter().rev().copied().collect();
        Ok(RsaPublicKey {
            n: BigUint::from_bytes_be(&modulus_be),
            e: BigUint::from_bytes_be(&pubexp.to_be_bytes()),
        })
    }

    /// MGF1 mask generation using SHA-256.
    fn mgf1(seed: &[u8], length: usize) -> Vec<u8> {
        let mut result = Vec::with_capacity(length);
        let mut counter: u32 = 0;
        while result.len() < length {
            let mut hasher = Sha256::new();
            hasher.update(seed);
            hasher.update(counter.to_be_bytes());
            result.extend_from_slice(&hasher.finalize());
            counter += 1;
        }
        result.truncate(length);
        result
    }

    /// Extract AES-128 key and IV from an RSA-2048 PSS signature.
    ///
    /// The salt in the PSS structure contains 32 bytes: key[0:16] + iv[16:32].
    /// Only the RSA public key is needed — PSS salt is recoverable during
    /// verification.
    pub fn extract_aes_key(
        signature: &[u8],
        key: &RsaPublicKey,
    ) -> Result<([u8; 16], [u8; 16]), String> {
        let sig_len = signature.len();
        if sig_len != 256 {
            return Err(format!("expected 256-byte signature, got {}", sig_len));
        }

        // Reverse signature bytes (code in FUN_00117e7c reverses before RSA)
        let sig_reversed: Vec<u8> = signature.iter().rev().copied().collect();
        let sig_int = BigUint::from_bytes_be(&sig_reversed);

        // RSA: recovered = sig^e mod n
        let recovered = sig_int.modpow(&key.e, &key.n);
        let em = recovered.to_bytes_be();

        // Pad to 256 bytes (leading zeros may be stripped by BigUint)
        let mut em_padded = vec![0u8; 256 - em.len()];
        em_padded.extend_from_slice(&em);
        let em = &em_padded[..];

        // Check PSS trailer
        if em[255] != 0xBC {
            return Err("PSS verification failed: no 0xBC trailer".into());
        }

        let h_len = 32;
        let em_len = 256;
        let h = &em[em_len - h_len - 1..em_len - 1];
        let masked_db = &em[..em_len - h_len - 1];

        // MGF1 unmask
        let db_mask = mgf1(h, masked_db.len());
        let mut db: Vec<u8> = masked_db
            .iter()
            .zip(db_mask.iter())
            .map(|(a, b)| a ^ b)
            .collect();
        db[0] &= 0x7F; // Clear high bit per PSS spec

        // Find 0x01 separator after zero padding
        let mut idx = 0;
        while idx < db.len() && db[idx] == 0 {
            idx += 1;
        }
        if idx >= db.len() || db[idx] != 0x01 {
            return Err("PSS: no 0x01 separator found".into());
        }
        let salt = &db[idx + 1..];
        if salt.len() < 32 {
            return Err(format!("salt too short: {} bytes", salt.len()));
        }

        let mut aes_key = [0u8; 16];
        let mut aes_iv = [0u8; 16];
        aes_key.copy_from_slice(&salt[..16]);
        aes_iv.copy_from_slice(&salt[16..32]);
        Ok((aes_key, aes_iv))
    }

    /// AES-128-CBC decrypt in-place (no padding — length must be 16-byte aligned).
    pub fn aes_cbc_decrypt(data: &mut [u8], key: &[u8; 16], iv: &[u8; 16]) {
        let cipher = Aes128::new(GenericArray::from_slice(key));
        let mut prev = GenericArray::clone_from_slice(iv);

        for chunk in data.chunks_exact_mut(16) {
            let mut block = GenericArray::from_mut_slice(chunk);
            let ciphertext_block = *block;
            cipher.decrypt_block(&mut block);
            for i in 0..16 {
                block[i] ^= prev[i];
            }
            prev = ciphertext_block;
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn read_u32_le(buf: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn write_file(path: &PathBuf, data: &[u8]) -> io::Result<()> {
    let mut f = File::create(path)?;
    f.write_all(data)?;
    f.sync_all()?;
    Ok(())
}

// ── CLI ─────────────────────────────────────────────────────────────────

struct Args {
    fw_path: PathBuf,
    output: Option<PathBuf>,
    extract_all: bool,
    decrypt: bool,
    rsa_key: Option<PathBuf>,
    split_dir: Option<PathBuf>,
}

fn parse_args() -> Result<Args, ExitCode> {
    let argv: Vec<String> = env::args().collect();
    let mut fw_path: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    let mut extract_all = false;
    let mut decrypt = false;
    let mut rsa_key: Option<PathBuf> = None;
    let mut split_dir: Option<PathBuf> = None;

    let mut i = 1;
    while i < argv.len() {
        match argv[i].as_str() {
            "-h" | "--help" => {
                print_usage();
                return Err(ExitCode::SUCCESS);
            }
            "-o" | "--output" => {
                i += 1;
                if i >= argv.len() {
                    eprintln!("error: -o requires a path");
                    return Err(ExitCode::from(2));
                }
                output = Some(PathBuf::from(&argv[i]));
            }
            "-a" | "--all" => {
                extract_all = true;
            }
            "-d" | "--decrypt" => {
                #[cfg(not(feature = "decrypt"))]
                {
                    eprintln!("error: --decrypt requires building with --features decrypt");
                    return Err(ExitCode::from(2));
                }
                decrypt = true;
            }
            "-k" | "--rsa-key" => {
                i += 1;
                if i >= argv.len() {
                    eprintln!("error: --rsa-key requires a path");
                    return Err(ExitCode::from(2));
                }
                rsa_key = Some(PathBuf::from(&argv[i]));
            }
            "-s" | "--split" => {
                i += 1;
                if i >= argv.len() {
                    eprintln!("error: --split requires a directory");
                    return Err(ExitCode::from(2));
                }
                split_dir = Some(PathBuf::from(&argv[i]));
            }
            s if s.starts_with('-') => {
                eprintln!("error: unknown option '{}'", s);
                print_usage();
                return Err(ExitCode::from(2));
            }
            _ => {
                if fw_path.is_some() {
                    eprintln!("error: multiple input files");
                    return Err(ExitCode::from(2));
                }
                fw_path = Some(PathBuf::from(&argv[i]));
            }
        }
        i += 1;
    }

    let fw_path = match fw_path {
        Some(p) => p,
        None => {
            print_usage();
            return Err(ExitCode::from(2));
        }
    };

    if decrypt && rsa_key.is_none() {
        eprintln!("error: --decrypt requires --rsa-key <PATH>");
        return Err(ExitCode::from(2));
    }

    Ok(Args {
        fw_path,
        output,
        extract_all,
        decrypt,
        rsa_key,
        split_dir,
    })
}

fn print_usage() {
    let decrypt_note = if cfg!(feature = "decrypt") {
        ""
    } else {
        "\n\
         NOTE: This binary was built without 'decrypt' support.\n\
         Build with: cargo build --release --features decrypt\n"
    };
    eprintln!(
        "Usage: fwextract [OPTIONS] <firmware.bin>\n\
         \n\
         Extract and optionally decrypt the remote-board firmware from a\n\
         TP-Link multi-image container.\n\
         \n\
         Options:\n\
         \x20 -o, --output <PATH>     Write remote board image to PATH\n\
         \x20 -a, --all               Also extract host kernel image\n\
         \x20 -d, --decrypt           Decrypt firmware (requires --rsa-key)\n\
         \x20 -k, --rsa-key <PATH>    RSA public key blob file (CryptoAPI format)\n\
         \x20 -s, --split <DIR>       Write host_kernel.bin + remote_board.bin to DIR\n\
         \x20 -h, --help              Show this help\n\
         {decrypt_note}"
    );
}

// ── Main ────────────────────────────────────────────────────────────────

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(code) => return code,
    };

    let mut data = match std::fs::read(&args.fw_path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: cannot read '{}': {}", args.fw_path.display(), e);
            return ExitCode::from(1);
        }
    };

    let tag = match TagHeader::parse(&data) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: {}", e);
            return ExitCode::from(1);
        }
    };

    let (tag_len, signature) = parse_chunks(&data);

    println!("=== Firmware container info ===");
    println!("{}", tag);
    println!("  file_size      : {} (0x{:X})", data.len(), data.len());
    println!("  tag_len        : 0x{:X}", tag_len);
    println!(
        "  signature_len  : {} bytes",
        if signature.is_empty() { 0 } else { signature.len() }
    );

    if tag.total_image_len as usize + tag_len > data.len() {
        eprintln!(
            "error: total_image_len ({}) + tag_len ({}) exceeds file size ({})",
            tag.total_image_len,
            tag_len,
            data.len()
        );
        return ExitCode::from(1);
    }

    // ── Decrypt (optional) ──
    #[cfg(feature = "decrypt")]
    if args.decrypt {
        let rsa_key_path = args.rsa_key.as_ref().unwrap();
        let key_blob = match std::fs::read(rsa_key_path) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("error: cannot read RSA key '{}': {}", rsa_key_path.display(), e);
                return ExitCode::from(1);
            }
        };

        let rsa_key = match decrypt::parse_rsa_pubkey_blob(&key_blob) {
            Ok(k) => k,
            Err(e) => {
                eprintln!("error: invalid RSA key blob: {}", e);
                return ExitCode::from(1);
            }
        };

        if signature.is_empty() {
            eprintln!("error: no RSA signature chunk found in firmware");
            return ExitCode::from(1);
        }

        let (aes_key, aes_iv) = match decrypt::extract_aes_key(&signature, &rsa_key) {
            Ok(k) => k,
            Err(e) => {
                eprintln!("error: AES key extraction failed: {}", e);
                return ExitCode::from(1);
            }
        };

        println!("\n=== Decryption ===");
        println!("  aes_key        : {}", hex(&aes_key));
        println!("  aes_iv         : {}", hex(&aes_iv));

        let enc_start = tag_len;
        let enc_len = data.len() - enc_start;
        let aligned_len = enc_len & !0xF; // Round down to 16-byte boundary
        println!(
            "  decrypt range  : 0x{:X} .. 0x{:X} ({} bytes)",
            enc_start,
            enc_start + aligned_len,
            aligned_len
        );

        decrypt::aes_cbc_decrypt(&mut data[enc_start..enc_start + aligned_len], &aes_key, &aes_iv);
        println!("  status         : decrypted (in-place)");

        // Quick verification: check for UBI magic
        if enc_start + 4 <= data.len() {
            let magic = &data[enc_start..enc_start + 4];
            print!("  first 4 bytes  : {:?} ", std::str::from_utf8(magic).unwrap_or("(non-ASCII)"));
            if magic == b"UBI!" || magic == b"UBI#" {
                println!("→ valid UBI header ✓");
            } else {
                println!();
            }
        }
    }

    #[cfg(not(feature = "decrypt"))]
    if args.decrypt {
        unreachable!("--decrypt rejected at parse time without feature");
    }

    // ── Split ──
    let host_start = tag_len;
    let host_len = tag.total_image_len as usize;
    let remote_start = tag_len + host_len;
    let remote_len = data.len() - remote_start;

    println!("\n=== Image layout ===");
    println!(
        "  host kernel    : 0x{:06X} .. 0x{:06X} ({} bytes, {:.1} MB)",
        host_start,
        host_start + host_len,
        host_len,
        host_len as f64 / 1048576.0
    );
    println!(
        "  remote board   : 0x{:06X} .. 0x{:06X} ({} bytes, {:.1} MB)",
        remote_start,
        remote_start + remote_len,
        remote_len,
        remote_len as f64 / 1048576.0
    );

    if remote_len == 0 {
        eprintln!("\nwarning: no remote board image in this container");
        return ExitCode::SUCCESS;
    }

    if (remote_len as u32) < MIN_REMOTE_SIZE || (remote_len as u32) > MAX_REMOTE_SIZE {
        eprintln!(
            "\nwarning: remote image size {} is outside the board's accepted range (2–8 MB)",
            remote_len
        );
    }

    // Show remote board version if "2RDH" header present
    let remote_data = &data[remote_start..];
    if remote_data.len() >= 16 && &remote_data[..4] == b"2RDH" {
        let ver_end = remote_data[16..]
            .iter()
            .position(|&b| b == 0)
            .map(|p| 16 + p)
            .unwrap_or(32);
        let version = String::from_utf8_lossy(&remote_data[16..ver_end]);
        println!("  remote version : {}", version);
    }

    // ── Write output ──
    if let Some(dir) = &args.split_dir {
        std::fs::create_dir_all(dir).ok();
        let host_path = dir.join("host_kernel.bin");
        let remote_path = dir.join("remote_board.bin");

        match write_file(&host_path, &data[host_start..host_start + host_len]) {
            Ok(()) => println!("\n  Host kernel  → {} ({} bytes)", host_path.display(), host_len),
            Err(e) => {
                eprintln!("error: cannot write '{}': {}", host_path.display(), e);
                return ExitCode::from(1);
            }
        }
        match write_file(&remote_path, remote_data) {
            Ok(()) => println!("  Remote board → {} ({} bytes)", remote_path.display(), remote_len),
            Err(e) => {
                eprintln!("error: cannot write '{}': {}", remote_path.display(), e);
                return ExitCode::from(1);
            }
        }
    } else {
        let remote_out = args.output.unwrap_or_else(|| {
            let base = args.fw_path.file_stem().unwrap_or_default().to_string_lossy();
            args.fw_path.with_file_name(format!("{}_remote.bin", base))
        });
        match write_file(&remote_out, remote_data) {
            Ok(()) => println!("\nRemote board image → {} ({} bytes)", remote_out.display(), remote_len),
            Err(e) => {
                eprintln!("error: cannot write '{}': {}", remote_out.display(), e);
                return ExitCode::from(1);
            }
        }

        if args.extract_all {
            let main_out = {
                let base = args.fw_path.file_stem().unwrap_or_default().to_string_lossy();
                args.fw_path.with_file_name(format!("{}_main.bin", base))
            };
            match write_file(&main_out, &data[host_start..host_start + host_len]) {
                Ok(()) => println!("Host kernel image  → {} ({} bytes)", main_out.display(), host_len),
                Err(e) => {
                    eprintln!("error: cannot write '{}': {}", main_out.display(), e);
                    return ExitCode::from(1);
                }
            }
        }
    }

    ExitCode::SUCCESS
}
