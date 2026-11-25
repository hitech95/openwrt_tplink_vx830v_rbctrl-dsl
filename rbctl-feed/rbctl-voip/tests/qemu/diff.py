# SPDX-License-Identifier: GPL-2.0-or-later
"""T2.13: compare the Rust encoders with hash-pinned OEM AArch64 serializers, offline.

Usage: diff.py <oracle_vectors binary>  (cargo build -p rbctl_voip_proto --example oracle_vectors)
"""
import os
import shutil
from pathlib import Path
import struct
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[3] / 'examples/voip_qemu'))


def main(tmp):
    if not shutil.which('qemu-aarch64') or not all(
        (HERE.parents[3]/'vx830v/squashfs-root/usr/bin'/name).is_file()
        for name in ('voip_client','voip_server_proxy')):
        print('SKIP qemu_diff: qemu-aarch64 or extracted OEM rootfs missing')
        raise SystemExit(77)
    from run import prepare, crc16_arc
    expected = []
    def add(label, data):
        expected.append((label, data))

    def oracle(hook):
        path = tmp / hook
        path.mkdir()
        build = prepare(path, hook + '.c')
        def invoke(name, data=b''):
            env = {k: v for k, v in os.environ.items() if not k.startswith('VOIP_')}
            return subprocess.check_output(build(name), cwd=path, env=env, input=data, timeout=15)
        return invoke

    rings = oracle('worker_io')('voip_client')
    assert len(rings) == 4*132
    opaque = bytearray()
    for i in range(4):
        r = rings[132*i:132*(i+1)]
        add(f'line {i//2} {"ring" if i%2 else "CID"}', r)
        if i%2:
            opaque.extend(r[24:28])
    probe = oracle('probe')
    simple = probe('voip_client')
    assert len(simple) == 6*132
    for i in range(6):
        add(f'stop-ring {i}' if i<2 else f'channel default {i-2}', simple[132*i:132*(i+1)])
    trace = oracle('controller')('voip_client')
    assert len(trace) == 4164
    for line in range(2):
        p = 924 + line*(3*132+12)
        add(f'open {line}', trace[p:p+132])
        p = 3372 + line*3*132
        add(f'activate {line}', trace[p:p+132]); add(f'deactivate {line}', trace[p+132:p+264])

    event = struct.pack('>II',18,1) + bytes(124)
    hdr = struct.pack('>BBIH',0x10,0x18,9,132)
    inbound = bytes(6)+bytes.fromhex('020000000002dd02')+hdr+struct.pack('>H',crc16_arc(hdr+bytes(2)+event))+event
    media = struct.pack('>HH',1,3)+bytes.fromhex('80080001000000a001020304')+b'\xd5'*160
    frames = probe('voip_server_proxy',simple[132:264]+inbound+media)
    assert len(frames)==668
    add('DD01 init frame',frames[:60]); add('DD01 DSP frame',frames[60:216]); add('DD03 frame',frames[468:])

    if len(sys.argv) != 2:
        raise SystemExit('usage: diff.py <oracle_vectors binary>')
    exe = Path(sys.argv[1])
    actual = subprocess.check_output([exe],input=opaque)
    p = 0
    for label, want in expected:
        got = actual[p:p+len(want)]
        if got != want:
            offset = next((i for i,(a,b) in enumerate(zip(got,want)) if a!=b),len(got))
            raise AssertionError(f'{label}: first mismatch at byte {offset}')
        p += len(want)
    assert p==len(actual)
    print(f'T2.13: {len(expected)} records/frames matched ({p} bytes; final ring word replayed on each line)')


if __name__=='__main__':
    if not __debug__:
        raise SystemExit('assertions required')
    with tempfile.TemporaryDirectory(prefix='voice-diff-') as tmp:
        main(Path(tmp))
