#!/usr/bin/env python3
"""extract_rsa_keys.py — Extract RSA public keys from a TP-Link binary.

The firmware signature verification keys are embedded as base64-encoded
CryptoAPI PUBLICKEYBLOB strings inside the code section of libcmm.so
(function ``checkFwSignNoTag`` / ``FUN_00169ee8``).

Every CryptoAPI PUBLICKEYBLOB starts with the same 8-byte header:
  bType=0x06 (PUBLICKEYBLOB), bVersion=0x02, reserved=0x0000,
  aiKeyAlg=0x0000A400 (CALG_RSA_KEYX)

In base64 these 8 bytes always encode to the prefix ``BgIAAACk``.
This script searches for that prefix, validates each candidate, and
writes the raw binary blobs to output files.

Usage:
    python3 extract_rsa_keys.py <libcmm.so> [--output-dir <dir>]

Output files:
    rsa1024_pub.bin   — RSA-1024 public key (PKCS#1 v1.5, old firmware)
    rsa2048_pub.bin   — RSA-2048 public key (PSS + AES decrypt, new firmware)
"""

import base64
import re
import struct
import sys
from pathlib import Path

PREFIX = b"BgIAAACk"  # base64 of 06 02 00 00 00 A4 00 00


def find_pubkey_blobs(data: bytes) -> list[tuple[int, bytes, dict]]:
    """Find all CryptoAPI PUBLICKEYBLOB base64 strings in binary data.

    Returns list of (offset, raw_blob, info_dict).
    """
    results = []
    for match in re.finditer(PREFIX + b"[A-Za-z0-9+/=]+", data):
        b64_bytes = match.group()
        try:
            blob = base64.b64decode(b64_bytes)
        except Exception:
            continue
        if len(blob) < 20:
            continue
        if blob[0] != 0x06 or blob[8:12] != b"RSA1":
            continue
        bitlen = struct.unpack_from("<I", blob, 12)[0]
        pubexp = struct.unpack_from("<I", blob, 16)[0]
        expected_len = 20 + bitlen // 8
        if len(blob) != expected_len:
            continue
        results.append(
            (
                match.start(),
                blob,
                {"bitlen": bitlen, "pubexp": pubexp, "size": len(blob)},
            )
        )
    return results


def main():
    args = sys.argv[1:]
    if not args or args[0] in ("-h", "--help"):
        print(__doc__)
        sys.exit(0)

    bin_path = args[0]
    out_dir = Path(args[2]) if len(args) >= 4 and args[1] == "--output-dir" else Path(".")

    data = Path(bin_path).read_bytes()
    keys = find_pubkey_blobs(data)

    if not keys:
        print(f"No RSA public keys found in {bin_path}")
        sys.exit(1)

    print(f"Found {len(keys)} RSA public key(s) in {bin_path}:\n")
    for offset, blob, info in keys:
        name = f"rsa{info['bitlen']}_pub.bin"
        out_path = out_dir / name
        out_path.write_bytes(blob)
        print(f"  {name}  (bitlen={info['bitlen']}, exp={info['pubexp']})")
        print(f"    offset in binary:  0x{offset:X}")
        print(f"    blob size:         {info['size']} bytes")
        print(f"    modulus:           {info['bitlen']//8} bytes")
        print(f"    written to:        {out_path}")
        print()


if __name__ == "__main__":
    main()
