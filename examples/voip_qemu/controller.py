# SPDX-License-Identifier: GPL-2.0-or-later
"""Validate the OEM call controller against a private DSP and CMSIP peer."""
import io
import os
from pathlib import Path
import struct
import subprocess
import tempfile

from run import prepare


def take(stream, n):
    pos = stream.tell()
    data = stream.read(n)
    assert len(data) == n, f'truncated record at {pos}: expected {n}, got {len(data)}'
    return data


def dsp(stream, *words):
    pos = stream.tell()
    record = take(stream, 132)
    expected = struct.pack(f'>{len(words)}I', *words).ljust(132, b'\0')
    assert record == expected, f'DSP offset {pos}: expected {words}, got {record.hex()}'


def invite(stream, seq):
    record = take(stream, 816)
    hdr = struct.unpack_from('<4I', record)
    assert hdr == (0x800003ee, 0, seq, 0), f'make-call header: {hdr}'
    assert record[16:].split(b'\0', 1)[0] == b'<sip:5550100@example.invalid:5060>'


def check(raw, *, bad_digits=False, teardown=False):
    stream = io.BytesIO(raw)
    for cmd, arg in ((1, 7), (0, 7), (2, 1), *((7, i) for i in range(4))):
        dsp(stream, cmd, arg)
    for line in range(2):
        dsp(stream, 8, line, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0)
        dsp(stream, 13, line, 4)
        dsp(stream, 13, line, 0)
        # Native endpoint words AFTER digit release. The guest separately
        # checks digit phase 4 and an empty buffer immediately after START.
        sample = struct.unpack('<3i', take(stream, 12))
        assert sample == (1, 0, line), f'line {line} endpoint: {sample}'

    if bad_digits:
        # Only line 1 originated, so it received the first sequence. The
        # guest verified its slot by looking up this sequence in the OEM table.
        invite(stream, 40)
    else:
        for line in range(2):
            invite(stream, 40 + line)
        for line in range(2):
            dsp(stream, 10, 0xfffe0000, 0, 0, 0, line, 3, 2, 8, 8, 0,
                20, 20, 1, 101, 101, 0x7f000001, (4000 + 2 * line) << 16,
                0x7f000001, ((5000 + 2 * line) << 16) | (5001 + 2 * line),
                *([0] * 10), 2)
            dsp(stream, 12, line)
            dsp(stream, 91, line)
    if teardown:
        # Teardown emits tone-stop and channel-close before the end marker.
        for line in range(2):
            dsp(stream, 13, line, 0)
            dsp(stream, 31, line)
        dsp(stream, 0)
        for line in range(2):
            hdr = struct.unpack_from('<4I', take(stream, 816))
            assert hdr == (0x800003fc, 2 + line, 0, 0), f'line {line} release: {hdr}'
    assert not stream.read(1), f'unexpected transcript after byte {stream.tell()-1}'


def main():
    if not __debug__:
        raise SystemExit('controller oracle requires assertions')
    scenarios = {'default': (), 'bad_digits': ('BAD_DIGITS',), 'teardown': ('TEARDOWN',),
                 'concurrent': ('TEARDOWN', 'CONCURRENT'), 'startup_failure': ('STARTUP_FAILURE',),
                 'peer_interrupt': ('TEARDOWN', 'PEER_INTERRUPT')}
    env = os.environ.copy()
    for flags in scenarios.values():
        for flag in flags:
            env.pop('VOIP_' + flag, None)
    with tempfile.TemporaryDirectory(prefix='voip-controller-') as tmp:
        cmd = prepare(Path(tmp), 'controller.c')('voip_client')
        for name, flags in scenarios.items():
            result = subprocess.run(cmd, cwd=tmp, env={**env, **{'VOIP_' + f: '1' for f in flags}},
                                    capture_output=True, timeout=10)
            if result.returncode:
                raise RuntimeError(f'{name}: guest exit {result.returncode}; '
                                   f'{result.stderr[-1200:].decode(errors="replace")}')
            try:
                check(result.stdout, bad_digits='BAD_DIGITS' in flags, teardown='TEARDOWN' in flags)
            except AssertionError as exc:
                raise AssertionError(f'{name}: {exc}') from exc
            print(f'{name}: PASS ({len(result.stdout)} transcript bytes)')


if __name__ == '__main__':
    main()
