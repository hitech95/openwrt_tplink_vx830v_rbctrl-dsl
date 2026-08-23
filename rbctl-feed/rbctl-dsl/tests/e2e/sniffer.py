#!/usr/bin/env python3
"""AF_PACKET sniffer for 0x88B5 frames — the E2E wire recorder.

Full harness documentation: docs/e2e.md

Two instances are spawned, one per end of the veth, because a raw socket
bound to `lan0.1.500` only observes the board's responses plus the daemon's
own outgoing echoes; the daemon's requests are cleanly visible from the
board side (`eth0.1.500`) as incoming frames.
"""
import socket, struct, sys, time

iface, out, tag = sys.argv[1], sys.argv[2], sys.argv[3]
s = socket.socket(socket.AF_PACKET, socket.SOCK_RAW, socket.htons(0x88B5))
s.bind((iface, 0))
log = open(out, "a", buffering=1)
while True:
    data, _ = s.recvfrom(2048)
    if len(data) < 24 or data[12:14] != b"\x88\xb5":
        continue
    magic, op = data[14], data[15]
    seq, = struct.unpack(">I", data[16:20])
    plen, = struct.unpack(">H", data[20:22])
    dirn = "REQ " if magic == 0x11 else "RESP"
    log.write(f"{time.time():.3f} {tag}{dirn} op={op:<2d} seq={seq:<4d} "
              f"plen={plen:<3d} {data[24:24+plen].hex()}\n")
