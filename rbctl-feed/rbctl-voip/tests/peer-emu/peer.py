# SPDX-License-Identifier: GPL-2.0-or-later
"""Minimal Econet DD01 responder, independent of the implementation under test.

Offline stream mode: stdin and stdout carry frames prefixed by a BE16 length.
No output for ignored frames. A CI transport can also call Peer.reply(frame).
"""
import struct
import sys


def crc(data):
    value = 0
    for byte in data:
        value ^= byte
        for _ in range(8):
            value = (value >> 1) ^ (0xa001 if value & 1 else 0)
    return value


class Peer:
    def __init__(self, mac=b'\x02\0\0\0\0\x02'):
        self.mac = mac
        self.last = self.cached = None

    def reply(self, frame):
        if len(frame) < 24 or frame[:6] not in (self.mac, b'\xff'*6):
            return None
        etype, marker, sub, seq, size, check = struct.unpack_from('>HBBIHH', frame, 12)
        if (etype, marker) != (0xdd01, 0x11) or sub not in (0x16, 0x1a, 0x1b):
            return None
        body = frame[24:24+size]
        if len(body) != size or crc(frame[14:22] + bytes(2) + body) != check:
            return None
        if seq != self.last:
            status = struct.pack('>I', int(sub == 0x16 and size < 132))
            hdr = struct.pack('>BBIH', 0x10, sub, seq, 4)
            self.cached = (frame[6:12] + self.mac + b'\xdd\x01' + hdr +
                           struct.pack('>H', crc(hdr + bytes(2) + status)) + status).ljust(60, b'\0')
            self.last = seq
        return self.cached


if __name__ == '__main__':
    peer = Peer()
    while prefix := sys.stdin.buffer.read(2):
        size, = struct.unpack('>H', prefix)
        data = sys.stdin.buffer.read(size)
        if len(data) != size:
            raise SystemExit('truncated input frame')
        ack = peer.reply(data)
        if ack:
            sys.stdout.buffer.write(struct.pack('>H', len(ack)) + ack)
            sys.stdout.buffer.flush()
