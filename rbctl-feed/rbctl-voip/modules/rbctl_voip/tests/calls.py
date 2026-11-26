# SPDX-License-Identifier: GPL-2.0-or-later
"""Two real SIP UAs, two synthetic FXS boards, no network outside loopback."""
import contextlib
import os
from pathlib import Path
import select
import socket
import struct
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / 'tests/peer-emu'))
from peer import Peer, crc


class Board:
    def __init__(self, sock, line, sample):
        self.sock, self.line, self.sample = sock, line, sample
        self.peer = Peer()
        self.seq = self.rtp = 0
        self.commands = []
        self.played = []
        self.fault = None
        self.lost_seq = None
        self.attach_count = 0
        self.host = bytes.fromhex('020000000001')

    def frame(self, et, sub, seq, body):
        h = struct.pack('>BBIH', 0x10, sub, seq, len(body))
        self.sock.send(self.host + self.peer.mac + struct.pack('>H', et) + h +
                       struct.pack('>H', crc(h + bytes(2) + body)) + body)

    def event(self, cmd, arg, val=0):
        self.seq += 1
        self.frame(0xdd02, 0x18, self.seq, struct.pack('>III', cmd, arg, val) + bytes(120))

    def mic(self):
        self.rtp += 1
        packet = struct.pack('>BBHII', 128, 8, self.rtp, self.rtp * 160, 3456) + bytes([self.sample])*160
        self.frame(0xdd03, 0x19, 0, struct.pack('>HH', self.line, 3) + packet)

    def receive(self):
        f = self.sock.recv(2048)
        reply = self.peer.reply(f)
        if reply:
            if f[15] == 0x16 and struct.unpack_from('>I',f,24)[0] == 13:
                if self.fault == 'reject-tone':
                    self.fault = None
                    self.frame(0xdd01,0x16,int.from_bytes(f[16:20],'big'),struct.pack('>I',1))
                    self.commands.append((13,f[28:156]))
                    return
                if self.fault == 'lost-ack':
                    self.lost_seq = f[16:20]; self.fault = None
                if self.lost_seq == f[16:20]: return
            self.sock.send(reply)
            if f[15] == 0x1b:
                self.attach_count += 1
                self.event(2, int(self.attach_count == 1))
            elif f[15] == 0x16:
                cmd, arg = struct.unpack_from('>II', f, 24)
                self.commands.append((cmd, f[28:156]))
                if cmd in (1, 2):
                    self.event(cmd, arg if cmd == 1 else 0)
        elif f[12:16] == b'\xdd\x03\x11\x19':
            slot, sel = struct.unpack_from('>HH', f, 24)
            if (slot, sel) != (self.line, 3) or f[29] & 127 != 8:
                raise ValueError('unexpected board media format')
            self.played.append(f[40:200])

    def ids(self):
        return [c for c, _ in self.commands]


def run(binary, mods, shim, hangup):
    with contextlib.ExitStack() as stack:
        root = Path(stack.enter_context(tempfile.TemporaryDirectory()))
        def udp():
            s = stack.enter_context(socket.socket(socket.AF_INET, socket.SOCK_DGRAM))
            s.bind(('127.0.0.1', 0))
            return s
        ports = [udp(), udp()]
        a, b = [s.getsockname()[1] for s in ports]
        logs, boards, procs = [], [], []
        for i, (line, sample, port, user) in enumerate([(0, 0xa5, a, '101'), (1, 0x37, b, '202')]):
            d = root / str(i); d.mkdir()
            for m in ('g711', 'account'):
                (d / (m+'.so')).symlink_to(Path(mods).resolve() / (m+'.so'))
            (d/'rbctl_voip.so').symlink_to(Path(shim).resolve())
            (d/'config').write_text(f'module_path {d}\nnet_interface 127.0.0.1\nsip_listen 127.0.0.1:{port}\n'
                f'sip_transports udp\nrbctl_voip_device fixture\nrbctl_voip_line {line}\n'
                'audio_source rbctl_voip,phone\naudio_player rbctl_voip,phone\naudio_txmode poll\n'
                f'rbctl_voip_playout_clock {"board" if hangup=="board-clock" else "timer"}\n'
                'module g711.so\nmodule_app rbctl_voip.so\nmodule_app account.so\n')
            (d/'accounts').write_text(f'<sip:{user}@127.0.0.1:{b}>;regint=0;audio_codecs=PCMA/8000/1\n')
            x, y = socket.socketpair(socket.AF_UNIX, socket.SOCK_DGRAM)
            stack.enter_context(x); stack.enter_context(y)
            boards.append(Board(x, line, sample))
            log = stack.enter_context((d/'log').open('w+')); logs.append(log)
            ports[i].close()
            proc = subprocess.Popen([binary,'-4','-c','-f',str(d)],stdin=subprocess.DEVNULL,
                stdout=log,stderr=subprocess.STDOUT,cwd=d,pass_fds=(y.fileno(),),
                env={**os.environ,'RBCTL_VOIP_TEST_FD':str(y.fileno()),
                     **({'RBCTL_VOIP_TEST_FAIL_OPEN':'1'} if i==0 and hangup=='send-failure' else {})})
            procs.append(proc)
            stack.callback(proc.wait); stack.callback(proc.kill)

        x,y=boards
        def wait(check, media=False, timeout=4):
            end=time.monotonic()+timeout; tick=0
            while not check():
                if time.monotonic()>=end or any(p.poll() is not None for p in procs):
                    for log in logs:
                        log.seek(0); print(log.read(),file=sys.stderr)
                    raise RuntimeError(f'call timeout; DSP commands: {x.ids()}, {y.ids()}')
                if media and time.monotonic()>=tick:
                    for v in boards: v.mic()
                    tick=time.monotonic()+.02
                ready,_,_=select.select([v.sock for v in boards],[],[],.01)
                for v in boards:
                    if v.sock in ready: v.receive()

        wait(lambda: all(v.ids().count(7)==4 for v in boards))
        if hangup in ('lost-ack','reject-tone'): x.fault = hangup
        x.event(18,0)
        if hangup == 'send-failure':
            x.event(17,0); x.event(18,0)
        if hangup == 'lost-ack':
            wait(lambda: x.attach_count==2 and x.ids().count(7)==8,timeout=8)
            x.event(17,0); x.event(18,0)
        wait(lambda: x.ids()[-2:]==[8,13])
        assert struct.unpack_from('>II',x.commands[-1][1])==(0,4)
        for k in ('#'+'1'*33+'202#' if hangup=='reject-tone' else '202#'):
            x.event(24,0,ord(k)); x.event(20,0,ord(k))
        if hangup == 'board-clock':
            wait(lambda: 5 in y.ids() and 10 in x.ids())
            # With a silent peer there must be no independent 20 ms tick.
            until=time.monotonic()+.1
            wait(lambda: time.monotonic()>=until)
            assert not x.played
            x.mic(); x.rtp-=1; x.mic()  # duplicate sequence must not add a tick
            wait(lambda: bool(x.played))
            until=time.monotonic()+.06
            wait(lambda: time.monotonic()>=until)
            assert len(x.played)==1 and len(set(x.played[0]))>2
        else:
            wait(lambda: 5 in y.ids() and any(len(set(p))>2 for p in x.played))
        cid=next(body for cmd,body in y.commands if cmd==4)
        assert cid[69:73]==b'101\0' and struct.unpack_from('>I',cid,112)[0]==2
        y.event(18,1)
        wait(lambda: 6 in y.ids() and 10 in y.ids())
        wait(lambda: all(bytes([other.sample])*160 in v.played for v,other in [(x,y),(y,x)]),media=True)
        v=x if hangup=='caller' else y
        v.event(17,v.line)
        wait(lambda: all(v.ids()[-3:]==[12,13,31] for v in boards))
        if hangup=='reject-tone': assert x.attach_count==1 and x.fault is None
        if hangup=='send-failure': assert x.ids().count(8)==1 and x.attach_count==1


if __name__=='__main__':
    run(*sys.argv[1:])
