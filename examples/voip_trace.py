"""Summarize DD01/DD02/DD03 metadata in offline Ethernet pcaps (no media dumps)."""
import argparse
import struct
from collections import Counter
from pathlib import Path

from checksum import crc16_arc


def decode(frame):
    if len(frame) < 14:
        return None
    pos = 12
    etype = int.from_bytes(frame[pos:pos + 2], 'big')
    vlans = []
    while etype in (0x8100, 0x88a8, 0x9100):
        if len(frame) < pos + 6:
            return None
        tag, etype = struct.unpack_from('>HH', frame, pos + 2)
        vlans.append(tag & 4095)
        pos += 4
    if etype not in (0xdd01, 0xdd02, 0xdd03):
        return None
    body = frame[pos + 2:]
    if len(body) < 10:
        return 'short voice header'
    marker, subtype, seq, size, crc = struct.unpack_from('>BBIHH', body)
    prefix = f'{etype:04x}/{subtype:02x} marker={marker:02x} seq={seq} len={size}'
    if vlans:
        prefix += f' vlan={vlans}'
    payload = body[10:10 + size]
    if len(payload) != size:
        return prefix + ' truncated'
    if crc16_arc(body[:8] + b'\0\0' + payload) != crc:
        return prefix + ' CRC mismatch'
    if (etype, subtype) in ((0xdd01, 0x16), (0xdd02, 0x18)) and size == 132:
        prefix += f' dsp={int.from_bytes(payload[:4], "big")}'
    if etype == 0xdd03 and size >= 4:
        slot, selector = struct.unpack_from('>HH', payload)
        prefix += f' slot={slot} selector={selector}'
    return prefix + ' CRC ok'


def scan(path, verbose):
    counts = Counter()
    with path.open('rb') as stream:
        header = stream.read(24)
        orders = {bytes.fromhex(s): order for s, order in
                  [('d4c3b2a1', '<'), ('4d3cb2a1', '<'),
                   ('a1b2c3d4', '>'), ('a1b23c4d', '>')]}
        order = orders.get(header[:4])
        if not order or len(header) != 24 or struct.unpack_from(order + 'H', header, 4)[0] != 2:
            raise ValueError('expected classic pcap; convert pcapng to pcap first')
        if struct.unpack_from(order + 'I', header, 20)[0] & 0xffff != 1:
            raise ValueError('expected Ethernet link type')
        while record := stream.read(16):
            if len(record) != 16:
                raise ValueError('incomplete pcap record header')
            _, _, size, _ = struct.unpack(order + 'IIII', record)
            if size > 16 * 1024 * 1024:
                raise ValueError(f'oversized pcap record after {counts["frames"]} readable frames '
                                 f'({counts["voice"]} voice)')
            frame = stream.read(size)
            if len(frame) != size:
                raise ValueError('incomplete pcap record')
            counts['frames'] += 1
            info = decode(frame)
            if info is not None:
                counts['voice'] += 1
                counts['crc_ok'] += info.endswith('CRC ok')
                if verbose:
                    print(f'{counts["frames"]}: {info}')
    print(f'{path}: {counts["frames"]} frames, {counts["voice"]} voice, {counts["crc_ok"]} CRC ok')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('captures', nargs='+', type=Path)
    parser.add_argument('-v', '--verbose', action='store_true')
    args = parser.parse_args()
    for capture in args.captures:
        try:
            scan(capture, args.verbose)
        except (OSError, ValueError) as error:
            parser.exit(1, f'{capture}: {error}\n')
