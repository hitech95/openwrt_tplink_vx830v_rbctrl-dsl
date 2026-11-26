# SPDX-License-Identifier: GPL-2.0-or-later
"""Offline M3 preflight: real baresip against one synthetic UDP registrar."""
import os
import select
import struct
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / 'tests/peer-emu'))
from peer import Peer, crc


def response(request):
    lines = request.decode('ascii').split('\r\n')
    if not lines[0].startswith('REGISTER sip:127.0.0.1:'):
        raise ValueError('unexpected SIP method or destination')
    headers = {k.lower(): v.strip() for k, v in
               (line.split(':', 1) for line in lines[1:] if line)}
    contact = headers['contact']
    expires = '0' if headers.get('expires') == '0' or ';expires=0' in contact else '60'
    fields = ['SIP/2.0 200 OK', 'Via: ' + headers['via'], 'From: ' + headers['from'],
              'To: ' + headers['to'] + ';tag=fixture', 'Call-ID: ' + headers['call-id'],
              'CSeq: ' + headers['cseq'], 'Contact: ' + contact, 'Expires: ' + expires,
              'Content-Length: 0', '', '']
    return '\r\n'.join(fields).encode('ascii'), expires != '0'


def main(binary, modules, shim, scenario='stopped'):
    if scenario not in ('stopped', 'running', 'late-board', 'missing-state', 'missing-start', 'missing-locale', 'reject-default'):
        raise ValueError(scenario)
    warm = scenario == 'running'
    locale = 7 if warm else 10
    expected = [(cmd, slot) for slot in range(4) for cmd in (12, 31)] if warm else []
    expected += [(1, locale), (0, locale)] + ([] if warm else [(2, 1)])
    expected += [(7, i) for i in range(4)]
    if scenario == 'missing-state':
        expected = []
    elif scenario == 'missing-locale':
        expected = expected[:1]
    elif scenario == 'missing-start':
        expected = expected[:3]
    with tempfile.TemporaryDirectory() as tmp, socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
        root = Path(tmp)
        sock.bind(('127.0.0.1', 0))
        sock.settimeout(0.1)
        for name in ('account', 'menu', 'g711'):
            (root / (name + '.so')).symlink_to(Path(modules).resolve() / (name + '.so'))
        (root / 'rbctl_voip.so').symlink_to(Path(shim).resolve())
        (root / 'config').write_text(f'module_path {root}\nnet_interface 127.0.0.1\nsip_transports udp\n'
                                     'sip_listen 127.0.0.1:0\nmodule g711.so\nmodule_app menu.so\n'
                                     f'rbctl_voip_locale {locale}\n'
                                     'rbctl_voip_device fixture\nmodule_app rbctl_voip.so\nmodule_app account.so\n')
        (root / 'accounts').write_text(f'<sip:m3@127.0.0.1:{sock.getsockname()[1]}>;regint=60\n')
        board, inherited = socket.socketpair(socket.AF_UNIX, socket.SOCK_DGRAM)
        responder = Peer()
        event_ack = False
        attach_seen = False
        seen = []
        seq = 0
        acks = []
        def event(frame, cmd, value):
            nonlocal seq
            seq += 1
            body = struct.pack('>II', cmd, value) + bytes(124)
            hdr = struct.pack('>BBIH', 0x10, 0x18, seq, len(body))
            board.send(frame[6:12] + responder.mac + b'\xdd\x02' + hdr +
                       struct.pack('>H', crc(hdr + bytes(2) + body)) + body)
        with board, inherited, (root / 'run.log').open('w+') as log:
            duration = '8' if scenario == 'late-board' else '7' if scenario.startswith('missing-') else '2'
            proc = subprocess.Popen([binary, '-4', '-c', '-f', tmp, '-t', duration], cwd=tmp,
                                    stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT,
                                    pass_fds=(inherited.fileno(),),
                                    env={**os.environ, 'LC_ALL': 'C', 'RBCTL_VOIP_TEST_FD': str(inherited.fileno())})
            registered = False
            try:
                deadline = time.monotonic() + 9
                while proc.poll() is None and time.monotonic() < deadline:
                    readable, _, _ = select.select([sock, board], [], [], 0.1)
                    if board in readable:
                        frame = board.recv(2048)
                        if attach_seen and frame[15] == 0x1a and (scenario.startswith('missing-') or scenario == 'reject-default'):
                            continue  # Keep the negative fixture offline after its first failure.
                        reply = responder.reply(frame)
                        if reply:
                            if frame[15] == 0x1b:
                                if scenario == 'late-board' and not attach_seen:
                                    attach_seen = True
                                    board.send(reply)  # No DSP state until the next attach.
                                    continue
                                attach_seen = True
                                if warm or scenario == 'missing-state':
                                    board.send(reply)
                                if scenario == 'missing-state':
                                    continue
                                event(frame, 2, int(not warm))
                                if warm:
                                    continue
                            if frame[15] == 0x16:
                                cmd, arg = struct.unpack_from('>II', frame, 24)
                                seen.append((cmd, arg))
                                if seen != expected[:len(seen)]:
                                    raise ValueError(f'unexpected startup sequence: {seen}')
                                if cmd == 1 and scenario != 'missing-locale':
                                    event(frame, cmd, arg)  # completion before ACK
                                if scenario == 'reject-default' and (cmd, arg) == (7, 3):
                                    status = struct.pack('>I', 1)
                                    reply = reply[:22] + struct.pack('>H', crc(reply[14:22] + bytes(2) + status)) + status
                                board.send(reply)
                                if cmd == 2 and scenario != 'missing-start':
                                    event(frame, cmd, 0)  # fresh completion after ACK
                            else:
                                board.send(reply)
                        elif frame[12:16] == b'\xdd\x02\x11\x18':
                            event_ack |= frame[16:22] == struct.pack('>IH', 1, 4) and frame[24:28] == bytes(4)
                            if frame[20:22] == b'\x00\x04' and frame[24:28] == bytes(4):
                                acks.append(struct.unpack_from('>I', frame, 16)[0])
                    if sock not in readable:
                        continue
                    packet, peer = sock.recvfrom(8192)
                    reply, active = response(packet)
                    sock.sendto(reply, peer)
                    registered |= active
            finally:
                if proc.poll() is None:
                    proc.kill()
                proc.wait()
            log.seek(0)
            output = log.read()
            if scenario == 'missing-state':
                attach_ok = ('rbctl_voip: attach failed (Connection timed out' in output
                             and 'rbctl_voip: attached' not in output and not event_ack)
            else:
                state = 'running' if warm else 'stopped'
                attach_ok = event_ack and f'rbctl_voip: attached (DSP {state})' in output
            ready = 'rbctl_voip: board ready' in output
            if scenario in ('stopped', 'running', 'late-board'):
                path = 'warm' if warm else 'cold'
                startup_ok = f'rbctl_voip: board ready ({path}, locale {locale})' in output
                if scenario == 'late-board':
                    startup_ok &= 'rbctl_voip: attach failed (Connection timed out' in output
            elif scenario == 'missing-state':
                startup_ok = not ready
            else:
                reason = 'Remote I/O error' if scenario == 'reject-default' else 'Connection timed out'
                startup_ok = not ready and f'rbctl_voip: startup failed ({reason}' in output
            if not (proc.returncode == 0 and registered and 'baresip v3.16.0' in output
                    and any('m3@127.0.0.1:' in line and '200 OK' in line and '[1 binding]' in line
                            for line in output.splitlines())
                    and attach_seen and attach_ok and startup_ok and seen == expected
                    and acks == list(range(1, seq + 1))):
                raise RuntimeError('registration/module preflight failed:\n' + output)


if __name__ == '__main__':
    main(*sys.argv[1:])
