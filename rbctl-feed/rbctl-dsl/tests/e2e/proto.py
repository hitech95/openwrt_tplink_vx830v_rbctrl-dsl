#!/usr/bin/env python3
"""0x88B5 frame builder/parser for E2E assertions and raw replay (E8).

Full harness documentation: docs/e2e.md

The CRC-16/ARC matches rbctl_proto::checksum (poly 0xA001, init 0,
reflected) — verified against the board's acceptance of our frames and its
responses.
"""
import socket, struct, time

ETHTYPE = 0x88B5
MAGIC_REQ, MAGIC_RESP = 0x11, 0x10


def crc16_arc(data: bytes) -> int:
    crc = 0
    for b in data:
        crc ^= b
        for _ in range(8):
            crc = (crc >> 1) ^ 0xA001 if crc & 1 else crc >> 1
    return crc


def build_frame(src_mac: bytes, dst_mac: bytes, magic: int, opcode: int,
                seq: int, payload: bytes) -> bytes:
    """Build a frame; checksum covers frame[0x0E:0x16] + 2 zero bytes +
    payload (the checksum field itself counts as zero)."""
    covered = (bytes([magic, opcode]) + struct.pack(">IH", seq, len(payload))
               + b"\x00\x00" + payload)
    crc = crc16_arc(covered)
    eth = dst_mac + src_mac + struct.pack(">H", ETHTYPE)
    return eth + struct.pack(">BBIH", magic, opcode, seq, len(payload)) \
        + struct.pack(">H", crc) + payload


def parse(buf: bytes):
    """Return (magic, opcode, seq, payload) or None."""
    if len(buf) < 24:
        return None
    et, = struct.unpack(">H", buf[12:14])
    if et != ETHTYPE:
        return None
    magic, opcode = buf[14], buf[15]
    seq, = struct.unpack(">I", buf[16:20])
    plen, = struct.unpack(">H", buf[20:22])
    return magic, opcode, seq, bytes(buf[24:24 + plen])


def parse_wire_log(path):
    """Parse the sniffer's text log into a list of dicts."""
    out = []
    for line in open(path):
        parts = line.split()
        if len(parts) < 6:
            continue
        t = float(parts[0])
        dirn = parts[1]          # REQ | RESP
        op = int(parts[2].split("=")[1])
        seq = int(parts[3].split("=")[1])
        plen = int(parts[4].split("=")[1])
        payload = bytes.fromhex(parts[5])
        out.append(dict(t=t, dir=dirn, op=op, seq=seq, plen=plen, payload=payload))
    return out


class RawClient:
    """Speak the board protocol directly from the harness (E8 replay)."""

    def __init__(self, iface: str, src_mac: bytes):
        self.s = socket.socket(socket.AF_PACKET, socket.SOCK_RAW,
                               socket.htons(ETHTYPE))
        self.s.bind((iface, 0))
        self.s.settimeout(2.0)
        self.mac = src_mac
        self.seq = 0

    def request(self, opcode: int, payload: bytes, retries: int = 3):
        self.seq += 1
        frame = build_frame(self.mac, b"\xff" * 6, MAGIC_REQ, opcode,
                            self.seq, payload)
        for _ in range(retries):
            self.s.send(frame)
            try:
                data, _ = self.s.recvfrom(2048)
            except socket.timeout:
                continue
            m = parse(data)
            if m and m[0] == MAGIC_RESP and m[1] == opcode and m[2] == self.seq:
                return m[3]
        raise TimeoutError(f"board did not answer op={opcode} seq={self.seq}")
