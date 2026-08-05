#!/usr/bin/env python3
"""
Reference implementation of the remote_board frame checksum (P0).

Reverse-engineered from remote_board:proto_compute_checksum /
proto_set_checksum / proto_verify_checksum. This is the algorithm every 0x88B5
frame the CLI replacement must produce, and every board reply it must validate.

Identified as **CRC-16/ARC** (poly 0x8005, init 0x0000, refin=refout=true,
xorout=0), computed over a covered region of the frame with the checksum field
itself zeroed. See docs/checksum.md.

Run directly to execute the self-test:
    python3 examples/checksum.py
"""
from __future__ import annotations
import struct

# ---------------------------------------------------------------------------
# Frame header layout  (proto_frame_hdr, 25 bytes; multi-byte fields big-endian)
# ---------------------------------------------------------------------------
# 0x00 dst_mac     [6]   not covered
# 0x06 src_mac     [6]   not covered
# 0x0C ethertype   [2]   not covered  (0x88B5)
ETH_HEADER = 14
OFF_MAGIC = 0x0E               # bMagic        [1]  covered region starts here
OFF_SUBTYPE = 0x0F             # bSubtype      [1]  = opcode
OFF_SEQ = 0x10                 # dwSeq         [4]
OFF_PAYLOAD_LEN = 0x14         # wPayload_len  [2]  bytes from bPayload_type onward
OFF_CHECKSUM = 0x16            # wChecksum     [2]  <-- what we compute
OFF_PAYLOAD_TYPE = 0x18        # bPayload_type [1]
# payload bytes follow at 0x19..

COVER_START = OFF_MAGIC        # checksum covers [0x0E .. 0x0E + payload_len + 10 - 1]
COVER_HEADER_BYTES = 10        # magic..checksum inclusive (1+1+4+2+2)

# ---------------------------------------------------------------------------
# The nibble table  (extracted from remote_board @ g_awCrcTable / 0x404b80)
# 16 x ushort, read little-endian from the AArch64 binary.
# ---------------------------------------------------------------------------
# This is the CRC-16/ARC (reflected poly 0xA001) 4-bit lookup table.
# GF(2)-linear: T[a^b] == T[a] ^ T[b].
CRC16_ARC_NIBBLE_TABLE = (
    0x0000, 0xCC01, 0xD801, 0x1400,
    0xF001, 0x3C00, 0x2800, 0xE401,
    0xA001, 0x6C00, 0x7800, 0xB401,
    0x5000, 0x9C01, 0x8801, 0x4400,
)


# ---------------------------------------------------------------------------
# 1. Authoritative port -- byte-for-byte match of proto_compute_checksum
# ---------------------------------------------------------------------------
def proto_compute_checksum(acc: int, buf: bytes) -> int:
    """Nibble-table CRC. `acc` is the seed (0 for a fresh frame)."""
    T = CRC16_ARC_NIBBLE_TABLE
    acc &= 0xFFFF
    for b in buf:
        lo = b & 0x0F
        hi = b >> 4
        # low nibble first, then high nibble (reflected / LSB-first processing)
        t = (acc >> 4) ^ T[acc & 0x0F] ^ T[lo]
        acc = (t >> 4) ^ T[t & 0x0F] ^ T[hi]
        acc &= 0xFFFF
    return acc


# ---------------------------------------------------------------------------
# 2. Equivalent standard CRC-16/ARC (bit-by-bit) -- for cross-validation only
# ---------------------------------------------------------------------------
def crc16_arc(data: bytes, init: int = 0x0000) -> int:
    """CRC-16/ARC: width=16, poly=0x8005, refin=true, refout=true, xorout=0."""
    crc = init & 0xFFFF
    for b in data:
        crc ^= b
        for _ in range(8):
            crc = (crc >> 1) ^ 0xA001 if (crc & 1) else (crc >> 1)
    return crc & 0xFFFF


# ---------------------------------------------------------------------------
# Frame helpers -- operate on wire-order bytearrays (big-endian multi-byte)
# ---------------------------------------------------------------------------
def set_checksum(frame: bytearray) -> None:
    """Mirror of proto_set_checksum: compute CRC over the covered region and
    store it big-endian. Assumes the frame already holds wire-order bytes
    (the C proto_postprocess byte-swaps are equivalent to building the frame
    big-endian in the first place)."""
    payload_len = struct.unpack_from('>H', frame, OFF_PAYLOAD_LEN)[0]
    length = payload_len + COVER_HEADER_BYTES
    struct.pack_into('>H', frame, OFF_CHECKSUM, 0)           # zero the field
    region = bytes(frame[COVER_START:COVER_START + length])  # covered span
    crc = proto_compute_checksum(0, region)
    struct.pack_into('>H', frame, OFF_CHECKSUM, crc)          # store big-endian


def verify_checksum(frame: bytes) -> bool:
    """Mirror of proto_verify_checksum. True iff the stored checksum matches a
    fresh computation over the covered region."""
    payload_len = struct.unpack_from('>H', frame, OFF_PAYLOAD_LEN)[0]
    length = payload_len + COVER_HEADER_BYTES
    stored = struct.unpack_from('>H', frame, OFF_CHECKSUM)[0]
    tmp = bytearray(frame)
    struct.pack_into('>H', tmp, OFF_CHECKSUM, 0)
    region = bytes(tmp[COVER_START:COVER_START + length])
    return stored == proto_compute_checksum(0, region)


# ---------------------------------------------------------------------------
# Demo / self-test
# ---------------------------------------------------------------------------
def _build_sample_frame() -> bytearray:
    """A minimal synthetic 0x88B5 frame (opcode 3 = dsl_config_down, no payload).
    Real commands are filled in by P1 (TX encodings); here we only need a valid
    frame to exercise the checksum."""
    dst = bytes.fromhex('ffffffffffff')      # broadcast (remote_board uses bcast)
    src = bytes.fromhex('001122334455')      # host MAC (placeholder)
    ethertype = struct.pack('>H', 0x88B5)
    magic = bytes([0x11])                    # command magic (0x11 = TX, 0x10 = response)
    subtype = bytes([0x03])                  # opcode 3 = dsl_config_down
    seq = struct.pack('>I', 1)
    payload_type = bytes([0x00])
    payload = b''
    # payload_len counts bytes from bPayload_type onward (type + payload)
    payload_len = struct.pack('>H', len(payload_type) + len(payload))
    checksum = struct.pack('>H', 0)          # placeholder, filled by set_checksum
    return bytearray(dst + src + ethertype + magic + subtype + seq
                     + payload_len + checksum + payload_type + payload)


def _selftest() -> None:
    import os
    # 1. the nibble-table port and the standard bit-by-bit CRC-16/ARC agree
    for n in (0, 1, 2, 3, 7, 16, 64, 257, 1024):
        data = os.urandom(n)
        a = proto_compute_checksum(0, data)
        b = crc16_arc(data)
        assert a == b, f"mismatch len={n}: nibble={a:#06x} arc={b:#06x}"
    print("[ok] nibble-table port == CRC-16/ARC for all tested lengths")

    # 2. set_checksum then verify_checksum round-trips
    frame = _build_sample_frame()
    set_checksum(frame)
    assert verify_checksum(frame), "verify failed after set"
    cs = struct.unpack_from('>H', frame, OFF_CHECKSUM)[0]
    print(f"[ok] sample frame checksum set+verify: {cs:#06x}")

    # 3. a single-bit flip in the covered region must break verification
    bad = bytearray(frame)
    bad[OFF_SEQ] ^= 0x01
    assert not verify_checksum(bad), "verify passed on corrupted frame"
    print("[ok] corruption detected (covered bit flip rejected)")

    # 4. a change outside the covered region (dst_mac) is NOT detected
    ignore = bytearray(frame)
    ignore[0] ^= 0x01
    assert verify_checksum(ignore), "verify failed on uncovered-byte change"
    print("[ok] uncovered byte (dst_mac) change correctly ignored")


if __name__ == "__main__":
    _selftest()
    print("\nAll checks passed.")
