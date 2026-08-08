# Firmware Container Format & Encryption

The TP-Link multi-image firmware container (`*.bin`) wraps a host (MT7986)
kernel image and a remote-board (EcoNet EN75xx) firmware image in a signed,
encrypted package. This page documents the full format, reverse-engineered
from `libcmm.so` (`rsl_sys_updateFirmware`, `checkFwSignNoTag` /
`FUN_00169ee8`) and `libcutil.so`
(`cen_rsaVerifyPSSSignByBase64EncodePublicKeyBlob`, `FUN_00117a70`).

## Container layout

```
Offset 0x000  ┌──────────────────────────────────────────┐
              │  Tag header (512 bytes, plaintext)        │
              │    [0x00] u32  image_version              │
              │    [0x34] u32  product_id                 │
              │    [0x38] u32  product_ver                │
              │    [0x3C] u32  add_hw_ver                 │
              │    [0x40] [u8;16]  md5_digest             │
              │    [0x70] u32  total_image_len             │
              │    [0x74] u32  kernel_offset (always 0)   │
              │    [0x78] u32  kernel_length               │
              │    [0x7C] u32  rootfs_offset               │
              │    [0x80] u32  rootfs_length (0 = none)   │
              │    [0x8C] u32  sw_revision                 │
              │    [0x94] u32  special_ver                 │
              │    [0xD0] [u8;128] rsa_signature (tag)    │
              │    ...                                    │
Offset 0x200  ├──────────────────────────────────────────┤
              │  Chunk metadata area (plaintext)          │
              │    type=1 chunk: RSA-2048 PSS signature   │
              │      (256-byte signature + metadata)      │
              │    type=0 chunk: terminator               │
              │  → getTagLen() returns 0x200 + chunk data │
              │    (typically 0x330 for RSA2048 firmware) │
Offset tagLen ├──────────────────────────────────────────┤
              │  AES-128-CBC encrypted firmware data      │
              │  (host kernel + remote board image)       │
              │                                           │
              │  [0 .. kernel_len)       Host UBI image   │
              │  [kernel_len .. end)     Remote board fw  │
              └──────────────────────────────────────────┘
```

### `getTagLen()` computation

The tag header at offset 0 is always 512 bytes (`0x200`). Starting at
`0x200`, a series of typed chunks provide additional metadata:

| Chunk type | Meaning | Layout |
|------------|---------|--------|
| 1 | RSA signature | `type(4) + len(4) + sig_data(len-8)` |
| 0 | Terminator | `type(4) + len(4)`, `len` = padding size |

`len` **includes** the 8-byte header. `getTagLen()` sums all chunk `len`
fields and adds `0x200`:

```python
def get_tag_len(data):
    offset = 0x200
    total = 0
    while True:
        ctype, clen = struct.unpack_from('<II', data, offset)
        total += clen
        if ctype == 0:
            break
        offset += clen
    return 0x200 + total
# → 0x330 for VX830v firmware
```

## Encryption scheme

### The problem solved

The firmware data (from `tagLen` to end-of-file) is **AES-128-CBC
encrypted**. The AES key is not stored anywhere in the binary — it is
**embedded inside the RSA-2048 PSS signature** as the PSS salt.

### How it works

```
Signing (TP-Link, with RSA private key):
  1. Generate random AES-128 key (16 bytes) and IV (16 bytes)
  2. AES-128-CBC encrypt the firmware data
  3. Place key+IV as the "salt" in RSA-PSS signature of the ciphertext
  4. RSA-2048 PSS sign SHA-256(encrypted_firmware_data)

Verification + decryption (device, with RSA public key):
  1. RSA-2048 PSS verify signature → extract salt (AES key + IV)
  2. AES-128-CBC decrypt firmware data in-place
  3. Split decrypted data into host kernel + remote board image
```

Only someone with the RSA private key can create valid firmware — the AES
key is protected by RSA. But the public key alone is sufficient to decrypt
existing firmware (the PSS salt is recoverable during verification).

### Call chain (libcmm.so → libcutil.so)

```
rdp_updateFirmware (libcmm.so @ 0x0015bea4)
  └→ rsl_sys_updateFirmware (libcmm.so @ 0x0016cdc4)
       ├→ checkFwSignNoTag (libcmm.so @ 0x00169ee8)
       │    ├→ version < 0x03000004:
       │    │    cen_rsaVerifySignByBase64EncodePublicKeyBlob
       │    │    (RSA-1024 PKCS#1, NO decryption)
       │    │
       │    └→ version ≥ 0x03000004:
       │         cen_rsaVerifyPSSSignByBase64EncodePublicKeyBlob
       │         (libcutil.so @ 0x00118088)
       │         └→ FUN_00117e7c
       │              └→ FUN_00117a70  ← PSS verify + AES decrypt
       │                   (recovers the AES key + IV from the PSS salt,
       │                    then AES-128-CBC decrypts the buffer in place —
       │                    see "AES key extraction algorithm" below)
       │
       ├→ dm_preHook (config migration, NOT decryption)
       ├→ oal_remote_upgradeImage (send remote image to board via op 8)
       └→ oal_sys_writeImage → do_upgrade.sh → ubiformat
```

### The mmap detail

The firmware file is opened with `mmap(PROT_READ|PROT_WRITE, MAP_SHARED)`.
When `checkFwSignNoTag` decrypts the buffer in-place, the **file on disk
is also modified** (mmap write-back). This is why `ubiformat` — which
reads from the file path, not the memory buffer — receives decrypted data.

## RSA public keys — where to find them

The RSA public keys are **not reproduced here**. They are embedded as
base64-encoded CryptoAPI `PUBLICKEYBLOB` string literals inside
`checkFwSignNoTag` (`FUN_00169ee8`) in `libcmm.so`.

### Location

| Key | Offset in libcmm.so | CryptoAPI format |
|-----|---------------------|------------------|
| RSA-1024 (PKCS#1, old fw) | `0x241EA0` | BLOBHEADER + RSAPUBKEY + 128-byte modulus |
| RSA-2048 (PSS + decrypt)  | `0x241F70` | BLOBHEADER + RSAPUBKEY + 256-byte modulus |

Both keys use:
- Algorithm: `CALG_RSA_KEYX` (`0xA400`)
- Magic: `RSA1`
- Exponent: `65537` (`0x10001`)

### CryptoAPI PUBLICKEYBLOB format

```
BLOBHEADER (8 bytes):
  [0] BYTE  bType      = 0x06 (PUBLICKEYBLOB)
  [1] BYTE  bVersion   = 0x02
  [2] WORD  reserved   = 0x0000
  [4] DWORD aiKeyAlg   = 0x0000A400 (CALG_RSA_KEYX)

RSAPUBKEY (12 bytes):
  [8]  DWORD magic     = 0x31415352 ("RSA1")
  [12] DWORD bitlen     = 1024 or 2048
  [16] DWORD pubexp     = 65537

Modulus (bitlen/8 bytes, stored LITTLE-ENDIAN):
  [20] BYTE[bitlen/8]  n
```

The base64 encoding of the 8-byte BLOBHEADER always produces the prefix
`BgIAAACk`, which can be used to locate the keys programmatically.

### Extraction

Use [`examples/extract_rsa_keys.py`](../../examples/extract_rsa_keys.py) to
extract both keys from a `libcmm.so` binary:

```bash
python3 examples/extract_rsa_keys.py squashfs-root/lib/libcmm.so --output-dir keys/
```

This produces `rsa1024_pub.bin` and `rsa2048_pub.bin` — raw binary blobs
that can be fed to [`fwextract --decrypt`](#fwextract-decrypt-flag) or the
[reference Python script](#reference-implementation).

## AES key extraction algorithm

The AES key is recoverable from the firmware file using **only the RSA
public key** — no private key needed. The PSS salt (containing the key) is
extracted during signature verification:

```python
# 1. Read 256-byte RSA signature from chunk type 1 (at offset 0x208)
signature = fw[0x208:0x208+256]

# 2. RSA verify: recovered = sig^e mod n
sig_int = int.from_bytes(signature[::-1], 'big')  # reverse for BE
recovered = pow(sig_int, e, n)
em = recovered.to_bytes(256, 'big')

# 3. Check PSS trailer
assert em[-1] == 0xBC

# 4. Split EM: maskedDB || H || 0xBC
H = em[223:255]          # 32-byte SHA-256 hash
maskedDB = em[:223]       # 223-byte masked data block

# 5. MGF1 unmask
dbMask = mgf1(H, 223)
DB = xor(maskedDB, dbMask)
DB[0] &= 0x7F            # clear high bit

# 6. Parse DB: 0x00* || 0x01 || salt
idx = find_01_separator(DB)
salt = DB[idx+1:]

# 7. Extract AES key + IV
aes_key = salt[0:16]      # AES-128-CBC key
aes_iv  = salt[16:32]     # AES-128-CBC IV
```

## "Already decrypted" marker

The decryption function (`FUN_00117a70`) checks a 20-byte marker at offset
`0x24` of the firmware data **before** decrypting. If present, decryption is
skipped (the data is already plaintext). This prevents double-decryption if
someone feeds an already-decrypted firmware to the upgrade process.

## Extracted content (VX830v_1.0_WI_20250703.bin)

After AES decryption, the firmware data splits into three parts:

| Component | File offset | Size | Notes |
|-----------|-------------|------|-------|
| Host kernel | 0x330 | 0x01980000 (25.5 MB) | UBI image: 199 EC + 204 VID headers |
| uImage (inside host UBI) | 0x41030 | ~669 KB | ARM, entry `0x41E00000`, name "seconduboot" |
| Gap (gzip tar manifest) | 0x01980330 | 154 bytes | gzip → 10 KB tar listing of `upgrade_exe/` |
| Remote board (`2RDH`) | 0x019803CA | 6.2 MB | `tclinux.trx` container, see below |

### Gap data

The 154 bytes between the host kernel length and `totalImageLen` are a
**gzip-compressed tar listing** of an `upgrade_exe/` directory (10,240 bytes
decompressed). This is likely used by the host's upgrade script to verify
or lay out files before flashing.

### Remote board image — `2RDH` / `tclinux.trx` format

Confirmed by [OpenWrt econet target
`tclinux-trx.sh`](https://git.openwrt.org/?p=openwrt/openwrt.git;a=blob;f=target/linux/econet/image/tclinux-trx.sh;hb=HEAD)
(merged September 2025). The `2RDH` magic is the standard EcoNet SDK
firmware container, used across the EN75xx family (EN751221, EN7528, etc.).

```
Offset Size  Field               Value (this firmware)
------ ----  ------------------  ---------------------------
0x00   4     magic               "2RDH"
0x04   BE32  header_length       256 (0x100)
0x08   BE32  total_length        6,188,862 (header + content)
0x0C   BE32  crc32_content       CRC-32/JAMCRC of content ✓
0x10   32    version_string      "7.3.261.1_v016\n" (null-padded)
0x30   32    customer_version    (newline, null-padded)
0x50   BE32  kernel_length       1,788,874 (1.7 MB, LZMA compressed)
0x54   BE32  rootfs_length       4,399,484 (4.2 MB)
0x58   BE32  romfile_length      0
0x5C   32    model_string        "3 6035 122 0\n"
0x7C   BE32  load_address        0x80002000 (MIPS KSEG0)
0x80   128   reserved            all zeros
─── 0x100: content begins ───
0x100        LZMA kernel          props=0x5D (lc=3,lp=0,pb=2), dict=8 MB
             0xFF padding         (to rootfs alignment)
             squashfs rootfs      (rootfs_length bytes)
```

> All multi-byte fields are **big-endian** (MIPS native). The CRC is
> CRC-32/**JAMCRC** (one's complement of standard CRC-32), computed over
> the content starting at offset 0x100.

#### LZMA kernel

The kernel at offset `0x100` is **LZMA-compressed** (standard alone
format, props `0x5D`, 8 MB dictionary). Decompression yields 5,273,216
bytes of valid **MIPS big-endian** code:

| Evidence | Detail |
|----------|--------|
| Reset vector | branch to a high kernel address (MIPS `j` instruction) |
| Function prologues | standard MIPS epilogue pattern (`addiu/lui/sw` on `$sp`/`$s0`) |
| Linux version | `3.18.21` (gcc 4.6.3, Buildroot 2015.08.1) |
| Subsystems found | JFFS2, ATM (`atm_dev_register`), 802.1Q VLAN, SPI NAND, NTFS |
| Load address | `0x80002000` (MIPS KSEG0 unmapped cached segment) |

This is the **EcoNet EN75xx Linux kernel** — the autonomous DSL SoC's
operating system. It can be loaded into Ghidra as `MIPS:BE:32:default`
for further analysis.

#### Rootfs

The squashfs rootfs follows the LZMA kernel (with `0xFF` padding to
alignment). Size: 4,399,484 bytes (4.2 MB).

### EcoNet platform context

The OpenWrt econet target (merged September 2025) documents the broader
EN75xx platform:

- **Bootloader**: accessible via UART at 115200 baud, supports xmodem
  flashing (`xmdm 0x80020000 <len>` → `flash 0x80000 0x80020000 <len>`)
- **Default bootloader credentials**: documented for similar TP-Link
  devices in the OpenWrt econet target
- **Dual-image layout**: `tclinux` (OS_A) + `tclinux_alt` (OS_B),
  selected by a boot flag in the `reserve`/`reservearea` partition
- **Flash layout**: `bootloader` (256 KB) → `romfile` (256 KB) →
  `tclinux` (kernel + rootfs) → `tclinux_alt` → other partitions
- **Related devices**: SmartFiber XP8421-B (EN751221), TP-Link Archer
  VR1200v v2 — same SoC family, same `2RDH` format

## fwextract `--decrypt` flag

The Rust [`fwextract`](../../fwextract/) tool supports full decryption when
built with `--features decrypt`:

```bash
# Build with decryption support
cargo build --release --features decrypt

# Extract keys (one-time)
python3 examples/extract_rsa_keys.py libcmm.so --output-dir keys/

# Decrypt and split
fwextract --decrypt --rsa-key keys/rsa2048_pub.bin firmware.bin --split output/
```

The `--decrypt` feature adds:
- `num-bigint-dig` — RSA modular exponentiation
- `sha2` — SHA-256 for PSS verification and MGF1
- `aes` + `cbc` — AES-128-CBC decryption
- `base64` — decoding the PUBLICKEYBLOB

## Reference implementation

See [`examples/decrypt_fw.py`](../../examples/decrypt_fw.py) for a Python
script that:
1. Reads the RSA public key from a file (produced by `extract_rsa_keys.py`)
2. Parses the tag header and chunk metadata
3. Extracts the AES key/IV from the RSA-PSS signature
4. AES-128-CBC decrypts the firmware
5. Splits into host kernel and remote board images

```bash
python3 examples/decrypt_fw.py --rsa-key keys/rsa2048_pub.bin firmware.bin --split output/
```
