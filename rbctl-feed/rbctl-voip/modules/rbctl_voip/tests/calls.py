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
import math

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / 'tests/peer-emu'))
from peer import Peer, crc


class Board:
    def __init__(self, sock, line, sample):
        self.sock, self.line, self.sample = sock, line, sample
        self.peer = Peer()
        self.seq = self.rtp = 0
        self.commands = []
        self.played = []
        self.by_slot = [[], []]
        self.fault = None
        self.lost_seq = None
        self.attach_count = 0
        self.payload = None
        self.host = bytes.fromhex('020000000001')

    def frame(self, et, sub, seq, body):
        h = struct.pack('>BBIH', 0x10, sub, seq, len(body))
        self.sock.send(self.host + self.peer.mac + struct.pack('>H', et) + h +
                       struct.pack('>H', crc(h + bytes(2) + body)) + body)

    def event(self, cmd, arg, val=0):
        self.seq += 1
        self.frame(0xdd02, 0x18, self.seq, struct.pack('>III', cmd, arg, val) + bytes(120))

    def mic(self, line=None, sample=None):
        self.rtp += 1
        packet = struct.pack('>BBHII', 128, 8, self.rtp, self.rtp * 160, 3456) + (self.payload or bytes([self.sample if sample is None else sample])*160)
        self.frame(0xdd03, 0x19, 0, struct.pack('>HH', self.line if line is None else line, 3) + packet)

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
            if slot>1 or (self.line is not None and slot!=self.line) or sel!=3 or f[29] & 127 != 8:
                raise ValueError('unexpected board media format')
            self.played.append(f[40:200])
            self.by_slot[slot].append(f[40:200])

    def ids(self):
        return [c for c, _ in self.commands]


def run(binary, mods, shim, hangup):
    isolation = hangup=='shared-isolation'
    multi = hangup in ('dual','waiting','shared','hunt') or isolation
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
            extra=['srtp'] if hangup=='srtp' else ['g722','auresamp'] if hangup=='g722' else []
            for m in ('g711', 'account',*extra):
                (d / (m+'.so')).symlink_to(Path(mods).resolve() / (m+'.so'))
            (d/'rbctl_voip.so').symlink_to(Path(shim).resolve())
            (d/'config').write_text(f'module_path {d}\nnet_interface 127.0.0.1\nsip_listen 127.0.0.1:{port}\n'
                f'sip_transports udp\nrbctl_voip_device fixture\nrbctl_voip_line {line}\n'
                'audio_source rbctl_voip,phone\naudio_player rbctl_voip,phone\naudio_txmode poll\n'
                f'rbctl_voip_playout_clock {"board" if hangup=="board-clock" else "timer"}\n'
                'module g711.so\nmodule_app rbctl_voip.so\nmodule_app account.so\n')
            (d/'accounts').write_text(f'<sip:{user}@127.0.0.1:{b}>;regint=0;audio_codecs=PCMA/8000/1\n')
            with (d/'config').open('a') as cfg:
                for m in extra: print(f'module {m}.so',file=cfg)
                if hangup=='g722': print('ausrc_srate 8000\nauplay_srate 8000',file=cfg)
                if hangup=='policy':
                    print(f'rbctl_voip_digitmap{line} 20[2-4]|31xT\nrbctl_voip_inband{line} 1\nrbctl_voip_tx_gain{line} -2\nrbctl_voip_rx_gain{line} 3',file=cfg)
                if hangup=='shortmap':print(f'rbctl_voip_digitmap{line} 202T|2021\nrbctl_voip_short_digit{line} 1',file=cfg)
                if hangup=='fax':print(f'rbctl_voip_fax_profile{line} 1\naudio_jitter_buffer_type fixed\naudio_buffer_mode fixed',file=cfg)
                if hangup in ('timers','regblock','postdisc','longmap'):
                    print(f'rbctl_voip_first_digit{line} 1\nrbctl_voip_howler_duration{line} 1\nrbctl_voip_offhook_warning{line} 1',file=cfg)
                    if hangup=='regblock':print(f'rbctl_voip_require_registration{line} 1',file=cfg)
                    if hangup=='longmap':print(f'rbctl_voip_digitmap{line} 202\nrbctl_voip_long_digit{line} 1',file=cfg)
                if hangup=='waiting':print('rbctl_voip_cw_cid0 1',file=cfg)
            if extra:
                codec='G722/16000/1' if hangup=='g722' else 'PCMA/8000/1;mediaenc=srtp-mand'
                (d/'accounts').write_text(f'<sip:{user}@127.0.0.1:{b}>;regint=0;audio_codecs={codec}\n')
            if multi:
                users=('101','102') if i==0 else ('201','202')
                with (d/'config').open('a') as cfg, (d/'accounts').open('w') as acc:
                    for slot,u in enumerate(users):
                        shared=(hangup in ('shared','hunt') or isolation) and i==1
                        if shared: u=users[0]
                        aor=f'sip:{u}@127.0.0.1:{b}'
                        print(f'rbctl_voip_account{slot} {aor}',file=cfg)
                        if hangup=='hunt':print(f'rbctl_voip_hunt{slot} 1',file=cfg)
                        if shared and slot==1:continue
                        print(f'<{aor}>;regint=0;audio_codecs=PCMA/8000/1;audio_source=rbctl_voip,phone{slot};audio_player=rbctl_voip,phone{slot}',file=acc)
            x, y = socket.socketpair(socket.AF_UNIX, socket.SOCK_DGRAM)
            stack.enter_context(x); stack.enter_context(y)
            boards.append(Board(x, None if multi else line, sample))
            log = stack.enter_context((d/'log').open('w+')); logs.append(log)
            ports[i].close()
            panel_env={}
            if hangup=='dual' and i==1:
                worker=d/'panel-worker'
                worker.write_text('#!/bin/sh\nexec cat > "${0%/*}/panel-states"\n');worker.chmod(0o700)
                panel_env={'RBCTL_PANEL_FIXTURE':str(worker)}
            proc = subprocess.Popen([binary,'-4','-c','-f',str(d)],stdin=subprocess.DEVNULL,
                stdout=log,stderr=subprocess.STDOUT,cwd=d,pass_fds=(y.fileno(),),
                env={**os.environ,**panel_env,'RBCTL_VOIP_TEST_FD':str(y.fileno()),
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
                    for i,v in enumerate(boards):
                        if multi:
                            for slot in (0,1): v.mic(slot,samples[i][slot])
                        else: v.mic()
                    tick=time.monotonic()+.02
                ready,_,_=select.select([v.sock for v in boards],[],[],.01)
                for v in boards:
                    if v.sock in ready: v.receive()

        wait(lambda: all(v.ids().count(7)==4 for v in boards))
        if hangup=='longmap':
            x.event(18,0);x.event(20,0,ord('2')); wait(lambda:x.ids().count(10)==1)
            assert 5 not in y.ids(); x.event(17,0);wait(lambda:x.ids()[-1]==31);return
        if hangup in ('timers','regblock'):
            x.event(18,0);wait(lambda:31 in x.ids(),timeout=5)
            selectors=[struct.unpack_from('>I',body,4)[0] for c,body in x.commands if c==13]
            assert selectors[0]==(2 if hangup=='regblock' else 4) and 10 in x.ids() and 12 in x.ids()
            x.event(17,0);x.event(18,0);wait(lambda:x.ids().count(8)==2)
            return
        if hangup in ('shared','hunt') or isolation:
            samples=[[0xa5,0x39],[0x37,0xa8]]
            def place(slot):
                x.event(18,slot)
                for k in '201#': x.event(20,slot,ord(k))
            place(0); broadcast=hangup!='hunt'; first=int(broadcast)
            wait(lambda:y.ids().count(5)==1+int(broadcast)); y.event(18,first)
            wait(lambda:bytes([samples[0][0]])*160 in y.by_slot[first] and bytes([samples[1][first]])*160 in x.by_slot[0],media=True)
            place(1);wait(lambda:y.ids().count(5)==2+int(broadcast));y.event(18,1-first)
            wait(lambda:bytes([samples[0][1]])*160 in y.by_slot[1-first] and bytes([samples[1][1-first]])*160 in x.by_slot[1],media=True)
            if isolation:
                # Four unrelated, low-level AC waveforms; PT 8 throughout.
                # Each frame contains an integer number of periods, so no
                # artificial edge is introduced at the RTP packet boundary.
                def alaw(v):
                    mask=0xd5 if v>=0 else 0x55
                    a=v if v>=0 else -v-1; seg=max(0,a.bit_length()-8)
                    return ((seg<<4)|((a>>max(4,seg+3))&15))^mask
                def level(c):
                    a=c^85; seg=(a>>4)&7; n=(a&15)*16
                    return (n+8 if not seg else (n+264)<<(seg-1))*(1 if a&128 else -1)
                blocks=[[bytes(alaw(round(500*math.sin(2*math.pi*f*t/8000))) for t in range(160))
                         for f in pair] for pair in ((550,850),(1150,1450))]
                def measure(seconds,remaining):
                    marks=[[len(b.by_slot[s]) for s in (0,1)] for b in boards]
                    tick=time.monotonic();end=tick+seconds
                    while time.monotonic()<end:
                        if time.monotonic()>=tick:
                            for i,b in enumerate(boards):
                                for slot in remaining[i]:
                                    b.payload=blocks[i][slot]; b.mic(slot)
                            tick+=.02
                        for b in boards:
                            if select.select([b.sock],[],[],.002)[0]: b.receive()
                    for i,b in enumerate(boards):
                        for slot in (0,1):
                            frames=b.by_slot[slot][marks[i][slot]:]
                            if slot not in remaining[i]:
                                assert not frames, ('closed slot still playing',i,slot)
                                continue
                            target=blocks[1-i][1-slot]
                            # Allow the previous stimulus in the jitter buffer
                            # before first marker, but none after it arrives.
                            start=frames.index(target); window=frames[start:]
                            assert len(window)>=seconds*35
                            assert all(f in (target,bytes([0xd5])*160,bytes([0x55])*160) for f in window), (i,slot,'unexpected PCM')
                            assert window.count(target)>=.95*len(window)
                            peak=max(abs(level(c)) for f in set(window) for c in f)
                            print(f'board={i} slot={slot} frames={len(window)} matching={window.count(target)} peak={peak}',flush=True)
                            assert peak<520
                measure(4,((0,1),(0,1)))
                x.event(17,0);wait(lambda:x.ids()[-1]==31 and y.ids()[-2:]==[12,13]); y.event(17,1)
                wait(lambda:y.ids()[-1]==31)
                measure(2,((1,),(0,)))
                x.event(17,1);wait(lambda:x.ids().count(31)==2);return
            for slot in (0,1):x.event(17,slot)
            wait(lambda:x.ids().count(31)==2)
            return
        if hangup=='waiting':
            samples=[[0xa5,0x39],[0x37,0xa8]]
            def dial(slot):
                x.event(18,slot)
                for k in '201#': x.event(20,slot,ord(k))
            def fresh(slot,tag):
                samples[0][slot], samples[1][0] = tag,tag+1
                wait(lambda: bytes([tag])*160 in y.by_slot[0] and
                     bytes([tag+1])*160 in x.by_slot[slot],media=True)
                assert bytes([tag+1])*160 not in x.by_slot[1-slot]
            dial(0); wait(lambda:5 in y.ids()); y.event(18,0)
            fresh(0,0x42)
            before=len(y.by_slot[0]); dial(1)
            wait(lambda:any(len(set(f))>8 for f in y.by_slot[0][before:]),media=True)
            fresh(0,0x48)  # waiting alert didn't release A
            assert y.ids().count(5)==1 and 31 not in y.ids()
            assert any(c==13 and struct.unpack_from('>I',body,4)[0]==31 for c,body in y.commands)
            y.event(19,0); fresh(1,0x55)
            y.event(19,0); fresh(0,0x63)
            y.event(17,0); wait(lambda:y.ids().count(5)==2)  # B re-rings
            y.event(18,0); fresh(1,0x69)
            x.event(17,1); wait(lambda:y.ids()[-2:]==[12,13])
            y.event(17,0);wait(lambda:y.ids()[-1]==31)
            return
        if hangup=='dual':
            samples=[[0xa5,0x39],[0x37,0xa8]]
            for slot,ext in enumerate(('201#','202#')):
                x.event(18,slot)
                for k in ext: x.event(20,slot,ord(k))
                wait(lambda: y.ids().count(5)==slot+1)
                cid=[body for cmd,body in y.commands if cmd==4][-1]
                assert cid[69:73]==(b'102\0' if slot else b'101\0')
                assert struct.unpack_from('>I',cid,0)[0]==slot
                y.event(18,slot)
                wait(lambda: y.ids().count(10)==slot+1)
            def delivered(slots):
                return all(bytes([samples[1-i][slot]])*160 in v.by_slot[slot]
                           for i,v in enumerate(boards) for slot in slots)
            wait(lambda:delivered((0,1)),media=True)
            states=root/'1'/'panel-states'
            wait(lambda:states.exists() and states.read_text().splitlines()[-1:]==['ACTIVE'],media=True)
            marker=len(states.read_text())
            for i,v in enumerate(boards):
                for slot in (0,1): assert bytes([samples[1-i][1-slot]])*160 not in v.by_slot[slot]
            x.event(17,0)
            wait(lambda: any(cmd==31 and body[:4]==bytes(4) for cmd,body in x.commands))
            y.event(17,0)
            # New signatures can only arrive through the surviving live dialog.
            samples[0][1],samples[1][1]=0xb4,0x46
            wait(lambda:delivered((1,)),media=True)
            until=time.monotonic()+.55
            wait(lambda:time.monotonic()>=until,media=True)
            assert states.read_text()[marker:] == '', 'surviving call lost its active indication'
            for v in boards: assert not any(cmd==31 and body[:4]==struct.pack('>I',1) for cmd,body in v.commands)
            y.event(17,1)
            wait(lambda: y.ids().count(31)==2 and x.ids()[-2:]==[12,13])
            x.event(17,1);wait(lambda:x.ids().count(31)==2)
            wait(lambda:states.read_text().splitlines()[-1:]==['FAULT'])  # regint=0 fixture
            return
        if hangup in ('lost-ack','reject-tone'): x.fault = hangup
        x.event(18,0)
        if hangup == 'send-failure':
            x.event(17,0); x.event(18,0)
        if hangup == 'lost-ack':
            wait(lambda: x.attach_count==2 and x.ids().count(7)==8,timeout=8)
            x.event(17,0); x.event(18,0)
        wait(lambda: x.ids()[-2:]==[8,13])
        assert struct.unpack_from('>II',x.commands[-1][1])==(0,4)
        for k in ('#'+'1'*33+'202#' if hangup=='reject-tone' else '202' if hangup in ('policy','shortmap') else '202#'):
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
        if hangup=='g722':
            def alaw(c):
                c ^= 85; v=(c & 15)*16; seg=(c>>4)&7
                return (v+8 if not seg else (v+264)<<(seg-1))*(1 if c&128 else -1)
            wait(lambda:all(any(abs(sum(alaw(c) for c in f)/160-alaw(other.sample))<200 for f in v.played) for v,other in [(x,y),(y,x)]),media=True)
        else:wait(lambda: all(bytes([other.sample])*160 in v.played for v,other in [(x,y),(y,x)]),media=True)
        if hangup=='policy':
            assert struct.unpack_from('>ii',next(body for c,body in x.commands if c==8),4)==(3,-2)
            for v in boards:
                assert next(body for c,body in v.commands if c==10)[48:60]==bytes(12)
            for v,other,digit,pair in [(x,y,'5',(770,1336)),(y,x,'9',(852,1477))]:
                before=len(other.played);v.event(24,v.line,ord(digit))
                def has_digit(f):
                    def pcm(c):
                        c ^=85; seg=(c>>4)&7; n=(c&15)*16
                        return (n+8 if not seg else (n+264)<<(seg-1))*(1 if c&128 else -1)
                    vals=[pcm(c) for c in f]; power=sum(n*n for n in vals)
                    if not power:return False
                    return all(abs(sum(n*complex(math.cos(i*w),math.sin(i*w)) for i,n in enumerate(vals)))**2/(160*power)>.12 for w in [2*math.pi*f/8000 for f in pair])
                wait(lambda:any(has_digit(f) for f in other.played[before:]),media=True)
                v.event(20,v.line,ord(digit)); before=len(other.played)
                wait(lambda:bytes([v.sample])*160 in other.played[before:],media=True)
        if hangup=='solo':
            x.event(19,0);wait(lambda:x.ids()[-2:]==[12,13]); x.event(19,0)
            x.sample,y.sample=0x49,0x61
            wait(lambda:all(bytes([other.sample])*160 in v.played for v,other in [(x,y),(y,x)]),media=True)
        if hangup=='fax':
            for cmd,mode in [(21,0),(22,1)]:
                n=x.ids().count(11);x.event(cmd,0)
                wait(lambda:x.ids().count(11)>n)
                assert struct.unpack_from('>I',x.commands[-1][1],48)[0]==mode
            def alaw(v):
                mask=0xd5 if v>=0 else 0x55
                a=v if v>=0 else -v-1; seg=max(0,a.bit_length()-8)
                return ((seg<<4)|((a>>max(4,seg+3))&15))^mask
            x.payload=bytes(alaw(int(6500*math.sin(2*math.pi*2100*t/8000))) for t in range(160))
            wait(lambda:x.ids().count(11)==3,media=True)
            x.payload=None;x.event(22,0);wait(lambda:x.ids().count(11)==4)
            x.sample,y.sample=0x49,0x61
            wait(lambda:all(bytes([other.sample])*160 in v.played for v,other in [(x,y),(y,x)]),media=True)
        if hangup=='g722':
            assert all('Set audio decoder: G722' in Path(l.name).read_text() for l in logs)
        v=x if hangup=='caller' else y
        remote=y if v is x else x
        v.event(17,v.line)
        wait(lambda: v.ids()[-3:]==[12,13,31] and remote.ids()[-2:]==[12,13])
        assert struct.unpack_from('>II',remote.commands[-1][1])==(remote.line,1)
        if hangup=='postdisc':
            wait(lambda:remote.ids()[-1]==31,timeout=5)
            remote.event(17,remote.line);remote.event(18,remote.line);wait(lambda:remote.ids()[-2:]==[8,13]);return
        remote.event(17,remote.line)
        wait(lambda: remote.ids()[-3:]==[13,13,31])
        assert struct.unpack_from('>II',remote.commands[-2][1])==(remote.line,0)
        if hangup=='send-failure': assert x.ids().count(8)==1 and x.attach_count==1
        remote.event(18,remote.line)
        wait(lambda: remote.ids()[-2:]==[8,13])
        assert struct.unpack_from('>II',remote.commands[-1][1])==(remote.line,4)
        remote.event(17,remote.line)
        wait(lambda: remote.ids()[-1]==31)
        if hangup=='reject-tone': assert x.attach_count==1 and x.fault is None


if __name__=='__main__':
    run(*sys.argv[1:])
