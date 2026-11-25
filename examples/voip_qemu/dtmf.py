"""Observe the OEM controller's distinct DSP and SIP INFO digit interfaces."""
from pathlib import Path
import struct
import subprocess
import tempfile

from run import prepare

with tempfile.TemporaryDirectory(prefix='voip-dtmf-') as tmp:
    command = prepare(Path(tmp), 'dtmf.c')('voip_client')
    data = subprocess.check_output(command, cwd=tmp, timeout=10)
    offset = 0
    for port in range(2):
        for digit in b'0123456789*#':
            for event in (0x5a, 0x33, 0x33):
                assert struct.unpack_from('<III', data, offset) == (0x10000000 | event, port | digit << 16, 0)
                offset += 12
            assert struct.unpack_from('<IIIB', data, offset) == (0x800003fa, port, 2, digit)
            offset += 16
    assert offset == len(data)
    print('OEM voip_client: 72 DSP inputs dispatched with unchanged keys; '
          '24 independent SIP INFO requests serialized, character and call index verified.')
