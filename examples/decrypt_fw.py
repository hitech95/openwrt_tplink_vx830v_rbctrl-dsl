#!/usr/bin/env python3
"""decrypt_fw.py — Decrypt a TP-Link multi-image firmware container.

The firmware is encrypted with AES-128-CBC.  The AES key and IV are embedded
inside the RSA-2048 PSS signature's salt field.  This script extracts them
using only the public key (no private key needed — PSS salt is recoverable
during verification).

Usage:
    # First, extract the RSA public key from libcmm.so:
    python3 extract_rsa_keys.py libcmm.so --output-dir keys/

    # Then decrypt and split:
    python3 decrypt_fw.py --rsa-key keys/rsa2048_pub.bin firmware.bin --split output/

    # Or write a single decrypted file:
    python3 decrypt_fw.py --rsa-key keys/rsa2048_pub.bin firmware.bin

Requires: pycryptodome  (pip install pycryptodome)
"""

import hashlib
import struct
import sys
import os
from pathlib import Path

from Crypto.Cipher import AES


# ── CryptoAPI PUBLICKEYBLOB parsing ──────────────────────────────────────

def parse_rsa_pubkey_blob(blob: bytes) -> tuple[int, int]:
    """Parse a CryptoAPI PUBLICKEYBLOB and return (modulus_n, exponent_e).

    Layout:
      [0]  BYTE  bType      = 0x06
      [1]  BYTE  bVersion   = 0x02
      [4]  DWORD aiKeyAlg
      [8]  DWORD magic      = "RSA1"
      [12] DWORD bitlen
      [16] DWORD pubexp
      [20] BYTE[bitlen/8]  modulus (little-endian)
    """
    assert blob[0] == 0x06, f"not a PUBLICKEYBLOB (bType=0x{blob[0]:02X})"
    assert blob[8:12] == b"RSA1", "magic is not RSA1"
    bitlen = struct.unpack_from("<I", blob, 12)[0]
    pubexp = struct.unpack_from("<I", blob, 16)[0]
    modulus = blob[20 : 20 + bitlen // 8]
    n = int.from_bytes(modulus, "little")
    return n, pubexp


# ── MGF1 (SHA-256) ──────────────────────────────────────────────────────

def mgf1(seed: bytes, length: int) -> bytes:
    result = b""
    counter = 0
    while len(result) < length:
        result += hashlib.sha256(seed + counter.to_bytes(4, "big")).digest()
        counter += 1
    return result[:length]


# ── AES key extraction from RSA-PSS signature ────────────────────────────

def extract_aes_key_from_pss(
    signature: bytes, n: int, e: int
) -> tuple[bytes, bytes, bool]:
    """Extract AES key+IV from RSA-PSS signature salt.

    Returns (aes_key, aes_iv, hash_valid).
    """
    sig_int = int.from_bytes(signature[::-1], "big")
    recovered = pow(sig_int, e, n)
    em = recovered.to_bytes(256, "big")

    if em[-1] != 0xBC:
        return b"", b"", False

    h_len = 32
    em_len = len(em)
    H = em[em_len - h_len - 1 : em_len - 1]
    masked_db = em[: em_len - h_len - 1]

    db_mask = mgf1(H, len(masked_db))
    db = bytes(a ^ b for a, b in zip(masked_db, db_mask))
    db = bytes([db[0] & 0x7F]) + db[1:]

    idx = 0
    while idx < len(db) and db[idx] == 0:
        idx += 1
    if idx >= len(db) or db[idx] != 0x01:
        return b"", b"", False

    salt = db[idx + 1 :]
    if len(salt) < 32:
        return b"", b"", False

    return salt[:16], salt[16:32], True


# ── Chunk metadata parsing ──────────────────────────────────────────────

def parse_chunks(data: bytes) -> tuple[int, bytes]:
    """Parse chunk metadata at offset 0x200.

    Returns (tag_len, signature_bytes).
    """
    offset = 0x200
    total = 0
    signature = b""
    found_sig = False

    while offset < len(data) - 8:
        chunk_type = struct.unpack_from("<I", data, offset)[0]
        chunk_len = struct.unpack_from("<I", data, offset + 4)[0]
        if chunk_type == 0:
            total += chunk_len
            break
        if chunk_type == 1 and not found_sig:
            sig_start = offset + 8
            signature = data[sig_start : sig_start + min(chunk_len - 8, 0x100)]
            found_sig = True
        total += chunk_len
        offset += chunk_len

    return 0x200 + total, signature


# ── Main ────────────────────────────────────────────────────────────────

def main():
    args = sys.argv[1:]
    if not args or args[0] in ("-h", "--help"):
        print(__doc__)
        sys.exit(0)

    fw_path = None
    rsa_key_path = None
    output = None
    split_dir = None

    i = 0
    while i < len(args):
        if args[i] == "--rsa-key" and i + 1 < len(args):
            rsa_key_path = args[i + 1]
            i += 2
        elif args[i] == "--output" and i + 1 < len(args):
            output = args[i + 1]
            i += 2
        elif args[i] == "--split" and i + 1 < len(args):
            split_dir = args[i + 1]
            i += 2
        elif fw_path is None:
            fw_path = args[i]
            i += 1
        else:
            i += 1

    if fw_path is None:
        print(__doc__)
        sys.exit(1)
    if rsa_key_path is None:
        print("error: --rsa-key is required (use extract_rsa_keys.py to produce it)")
        sys.exit(1)

    fw = Path(fw_path).read_bytes()
    key_blob = Path(rsa_key_path).read_bytes()
    n, e = parse_rsa_pubkey_blob(key_blob)

    version = struct.unpack_from("<I", fw, 0)[0]
    total_img_len = struct.unpack_from("<I", fw, 0x70)[0]
    tag_len, signature = parse_chunks(fw)

    print(f"Decrypting: {fw_path}")
    print(f"\n=== Firmware info ===")
    print(f"  Version:        0x{version:08X}")
    print(f"  File size:      {len(fw)}")
    print(f"  Total image:    0x{total_img_len:X}")
    print(f"  Tag length:     0x{tag_len:X}")
    print(f"  RSA key bits:   {n.bit_length()}")

    if not signature:
        print("error: no RSA signature chunk found")
        sys.exit(1)

    aes_key, aes_iv, pss_ok = extract_aes_key_from_pss(signature, n, e)

    print(f"  AES key:        {aes_key.hex()}")
    print(f"  AES IV:         {aes_iv.hex()}")
    print(f"  PSS valid:      {pss_ok}")

    if not pss_ok:
        print("error: PSS signature verification failed")
        sys.exit(1)

    enc_data = fw[tag_len:]
    dec_len = (len(enc_data) // 16) * 16
    cipher = AES.new(aes_key, AES.MODE_CBC, aes_iv)
    decrypted = cipher.decrypt(enc_data[:dec_len])

    result = fw[:tag_len] + decrypted

    if split_dir:
        os.makedirs(split_dir, exist_ok=True)
        host_path = os.path.join(split_dir, "host_kernel.bin")
        remote_path = os.path.join(split_dir, "remote_board.bin")

        Path(host_path).write_bytes(result[tag_len : tag_len + total_img_len])
        Path(remote_path).write_bytes(result[tag_len + total_img_len :])

        host = result[tag_len : tag_len + total_img_len]
        remote = result[tag_len + total_img_len :]
        print(f"\n  Host kernel:  {host_path} ({len(host)} bytes)")
        print(f"  Remote board: {remote_path} ({len(remote)} bytes)")

        if remote[:4] == b"2RDH":
            ver_end = remote.find(b"\x00", 16)
            ver = remote[16:ver_end].decode("ascii", errors="replace")
            print(f"  Remote version: {ver}")

        ubi_count = host.count(b"UBI!")
        print(f"  Host UBI! headers: {ubi_count}")
    else:
        out = output or os.path.splitext(fw_path)[0] + "_decrypted.bin"
        Path(out).write_bytes(result)
        print(f"\n  Output: {out} ({len(result)} bytes)")


if __name__ == "__main__":
    main()
