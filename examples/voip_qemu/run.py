"""Run normal OEM VoIP serializers offline against connected local sockets."""
import hashlib
import os
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1] / 'vx830v/squashfs-root'
sys.path.insert(0, str(HERE.parent))
from checksum import crc16_arc

HASHES = {
    'voip_client': '716f620f8b761c20378d9cb8d0d22b8b6ef253af63f839b72b8e4a5e4f5bf3c8',
    'voip_server_proxy': '2e0c84398ba9c480784c2cedfd19933c444614c55905b4f4667b975252ac7d99',
}


def prepare(tmp, source):
    lib = tmp / 'lib'
    lib.mkdir()
    for src in (ROOT / 'lib').glob('*.so*'):
        if not src.is_file():
            continue
        (lib / src.name).symlink_to(src)
    # Extraction omitted some SONAME symlinks. Reconstruct them in scratch.
    for src in (ROOT / 'lib').glob('*.so*'):
        if src.is_file() and not src.name.endswith('.py'):
            out = subprocess.check_output(['readelf', '-d', src], env={**os.environ, 'LC_ALL': 'C'}, text=True)
            for row in out.splitlines():
                if '(SONAME)' in row:
                    alias = lib / row.split('[')[1].split(']')[0]
                    if not alias.exists():
                        alias.symlink_to(src)
    loader = lib / 'ld-musl-aarch64.so.1'
    if not loader.exists():
        loader.symlink_to(ROOT / 'lib/libc.so')
    clang = shutil.which('clang') or 'clang'
    linker = shutil.which('ld.lld')
    if not linker:
        sysroot = subprocess.check_output(['rustc', '--print', 'sysroot'], text=True).strip()
        linker = str(next(Path(sysroot).glob('lib/rustlib/*/bin/gcc-ld/ld.lld')))
    hook = tmp / 'probe.so'
    subprocess.run([clang, '--target=aarch64-linux-musl', f'--ld-path={linker}', '-fPIC',
                    '-shared', '-nostdlib', '-fno-builtin', '-fno-stack-protector', '-o', hook, HERE / source], check=True)

    def command(name):
        src = ROOT / 'usr/bin' / name
        if hashlib.sha256(src.read_bytes()).hexdigest() != HASHES[name]:
            raise ValueError(f'unrecognized build: {name}')
        exe = tmp / name
        shutil.copyfile(src, exe)
        exe.chmod(0o700)
        args = ['qemu-aarch64', '-L', tmp, '-E', f'LD_LIBRARY_PATH={lib}', '-E', f'LD_PRELOAD={hook}',
                '-U', 'PROBE_CLIENT']
        if name == 'voip_client':
            args += ['-E', 'PROBE_CLIENT=1']
        return [*args, exe]
    return command


def run(tmp):
    command = prepare(tmp, 'probe.c')
    def invoke(name, payload=b''):
        return subprocess.run(command(name), input=payload, stdout=subprocess.PIPE, check=True, timeout=10).stdout

    records = invoke('voip_client')
    expected = [(6, 0), (6, 1), *((7, slot) for slot in range(4))]
    assert records == b''.join(struct.pack('>II', cmd, ident) + bytes(124) for cmd, ident in expected)
    print('OEM DSP records (cmd, line/slot):', expected)
    ring_stop = records[132:264]
    off_hook = struct.pack('>II', 18, 1) + bytes(124)
    peer = bytes.fromhex('020000000002')
    def inbound(payload):
        hdr = struct.pack('>BBIH', 0x11, 0x18, 9, len(payload))
        crc = crc16_arc(hdr + bytes(2) + payload)
        return bytes(6) + peer + b'\xdd\x02' + hdr + struct.pack('>H', crc) + payload
    media = struct.pack('>HH', 1, 3) + bytes.fromhex('80080001000000a001020304') + b'\xd5' * 160
    raw = invoke('voip_server_proxy', ring_stop + inbound(off_hook) + media)
    assert len(raw) == 668
    for seq, (frame, subtype, payload) in enumerate(((raw[:60], 0x1a, b'\x01\0\0\0'),
                                                     (raw[60:216], 0x16, ring_stop)), 1):
        assert frame[12:22] == struct.pack('>HBBIH', 0xdd01, 0x11, subtype, seq, len(payload))
        assert frame[24:24 + len(payload)] == payload and not any(frame[24 + len(payload):])
        crc, = struct.unpack_from('>H', frame, 22)
        assert crc == crc16_arc(frame[14:22] + b'\0\0' + payload)
        print(f'OEM DD01/{subtype:02x}: seq={seq}, payload={len(payload)}, frame={len(frame)}, CRC={crc:04x}')
    ack = raw[216:276]
    assert raw[276:336] == ack and raw[336:468] == off_hook
    assert ack[:6] == peer and ack[12:] == bytes.fromhex('dd021118000000090004591f') + bytes(36)
    print('OEM DD02/18: off-hook, line 2; duplicate ACK identical, one queued event, exact DSP delivery')
    frame = raw[468:]
    assert frame[12:22] == struct.pack('>HBBIH', 0xdd03, 0x11, 0x19, 0, len(media))
    assert frame[24:] == media and int.from_bytes(frame[22:24], 'big') == crc16_arc(frame[14:22] + bytes(2) + media)
    print('OEM DD03/19: slot 1, RTP selector 3, 172-byte synthetic RTP datagram preserved')


if __name__ == '__main__':
    with tempfile.TemporaryDirectory(prefix='voip-qemu-') as temp:
        run(Path(temp))
