# SPDX-License-Identifier: GPL-2.0-or-later
"""Exercise the real CLI startup loop using Unix datagrams only."""
import os
import socket
import struct
import subprocess
import sys

from peer import Peer, crc

HOST = bytes.fromhex('020000000001')
BOARD = bytes.fromhex('020000000002')


def frame(etype, sub, seq, body):
    header = struct.pack('>BBIH', 0x10, sub, seq, len(body))
    return HOST + BOARD + struct.pack('>H', etype) + header + struct.pack('>H', crc(header + bytes(2) + body)) + body


def scenario(exe, mode):
    peer = Peer(BOARD)
    a, b = socket.socketpair(socket.AF_UNIX, socket.SOCK_DGRAM)
    with a, b:
        a.settimeout(2)
        watch = mode in ('events','monitor')
        cmd = [exe, 'events' if watch else 'init', '--device', 'fixture', '--locale', '7', '--timeout-ms', '800']
        keypad = mode.startswith('keypad')
        if keypad:
            cmd[1] = 'keypad'
        if mode == 'monitor':
            cmd[1] = 'monitor'
        ring = mode.startswith('ring')
        tone = mode.startswith('tone')
        loop = mode.startswith('loop')
        if loop:
            cmd=[exe,'loop','--device','fixture','--line','0','--to','1','--duration-ms','200']
        if tone:
            cmd[1] = 'tone'
            cmd += ['--line','0','--sel','0' if mode=='tone-stop' else '4','--duration-ms','60']
        if ring:
            cmd[1] = 'ring'
            cmd += ['--line','1','--duration-ms','60']
            if mode == 'ring-cid':
                cmd[cmd.index('--locale')+1] = '6'
                cmd += ['--cid','BoardTest,5550101']
            else:
                del cmd[cmd.index('--locale'):cmd.index('--locale')+2]
        proc = subprocess.Popen(cmd, env={**os.environ, 'RBCTL_VOIP_TEST_FD': str(b.fileno())}, pass_fds=(b.fileno(),), stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        seq = 0

        def event(cmd, arg, value=0):
            nonlocal seq
            seq += 1
            a.send(frame(0xdd02, 0x18, seq, struct.pack('>III', cmd, arg, value) + bytes(120)))

        def ack_event():
            r = a.recv(2048)
            assert r[:12] == BOARD + HOST
            assert struct.unpack_from('>HBBIH', r, 12) == (0xdd02, 0x11, 0x18, seq, 4)
            assert r[24:28] == bytes(4)

        try:
            warm = mode.startswith('warm')
            recovery = [(22, code, slot) for slot in range(4) for code in (12,31)] if warm else []
            start = [] if warm else [(22,2,1)]
            for sub, dsp, arg in [(26,None,0), (27,None,0), *recovery, (22,1,7), (22,0,7), *start, *((22,7,i) for i in range(4))]:
                r = a.recv(2048)
                assert r[15] == sub
                if dsp is not None:
                    assert struct.unpack_from('>II', r, 24) == (dsp,arg)
                reply = peer.reply(r)
                assert reply is not None
                if (mode == 'reject' and dsp == 0) or (mode == 'warm-reject' and dsp == 31):
                    a.send(frame(0xdd01, sub, int.from_bytes(r[16:20], 'big'), struct.pack('>I', 1)))
                    break
                if sub == 27 or dsp == 1:
                    # Both are legal: attach state after ACK; SLIC result before ACK.
                    if sub == 27:
                        a.send(reply)
                    state = int(not (keypad or mode=='monitor' or tone or ring or loop or warm)) if sub==27 else (0xffffffff if mode=='slic-error' else 0)
                    event(2 if sub == 27 else 1, state)
                    ack_event()
                    if (keypad or mode == 'monitor' or ring or tone or loop) and sub == 27:
                        break
                    if mode == 'slic-error' and dsp == 1:
                        break
                    if dsp == 1:
                        a.send(reply)
                else:
                    a.send(reply)
                if dsp == 2:
                    if mode == 'cached-only':
                        break
                    event(2,0)
                    ack_event()
            if loop:
                for port in (0,1):
                    op=a.recv(2048); assert struct.unpack_from('>II',op,24)==(8,port); a.send(peer.reply(op))
                    op=a.recv(2048); assert struct.unpack_from('>I',op,24)==(10,) and struct.unpack_from('>5I',op,44)==(port,3,2,8,8)
                    assert struct.unpack_from('>5I',op,68)==(20,20,1,101,101)
                    if mode=='loop-reject' and port==1:
                        a.send(frame(0xdd01,22,int.from_bytes(op[16:20],'big'),struct.pack('>I',1)))
                    else: a.send(peer.reply(op))
                if mode=='loop':
                    for port in (0,1):
                        for sel in (3,4):
                            payload=struct.pack('>BBHII',128,8,5,160,port+1)+bytes([0xd5])*160 if sel==3 else bytes.fromhex('80c9000100000001')
                            a.send(frame(0xdd03,25,0,struct.pack('>HH',port,sel)+payload))
                            sent=a.recv(2048); assert sent[:12]==BOARD+HOST and sent[12:16]==bytes.fromhex('dd031119') and sent[24:]==struct.pack('>HH',1-port,sel)+payload
                for port in (1,0):
                    for cmd_id in (12,31):
                        op=a.recv(2048); assert struct.unpack_from('>II',op,24)==(cmd_id,port); a.send(peer.reply(op))
            if tone:
                def expect(words, timeout=2):
                    a.settimeout(timeout)
                    packet = a.recv(2048)
                    assert packet[15] == 22 and packet[24:] == struct.pack('>'+'I'*len(words),*words).ljust(132,b'\0')
                    a.send(peer.reply(packet))
                if mode != 'tone-stop':
                    expect([8,0,0,0,1,0,0,0,0,0,1,0]); expect([13,0,4])
                expect([13,0,0])
                if mode != 'tone-stop': expect([31,0],5)
            if ring:
                if mode == 'ring-cid':
                    prof = a.recv(2048)
                    assert prof[24:] == struct.pack('>II',0,6)+bytes(124)
                    a.send(peer.reply(prof))
                    c = a.recv(2048)
                    assert struct.unpack_from('>II',c,24)==(4,1) and c[48:56].isdigit()
                    assert c[57:67]==b'BoardTest\0' and c[97:105]==b'5550101\0' and c[140:144]==struct.pack('>I',2)
                    a.send(peer.reply(c))
                start = a.recv(2048)
                assert start[15] == 22 and start[24:] == struct.pack('>7I',5,1,0,0,0xffffffff,2,0)+bytes(104)
                if mode != 'ring-lost-ack':
                    a.send(peer.reply(start))
                else:
                    assert a.recv(2048) == start and a.recv(2048) == start
                stop = a.recv(2048)
                assert stop[15] == 22 and stop[24:] == struct.pack('>II',6,1)+bytes(124)
                a.send(peer.reply(stop))
            if keypad:
                for port in (0,1):
                    event(18, port); ack_event()
                    for digit in b'1234567890*#':
                        event(24, port, digit); ack_event()
                        event(20, port, digit if mode=='keypad' else digit+256); ack_event()
                        if mode!='keypad': break
                    if mode!='keypad': break
                    event(17, port); ack_event()
            if watch:
                for line in range(2):
                    for cmd in (18,19,17):
                        event(cmd,line)
                        ack_event()
                # The ACK precedes local delivery. Wait for the last printed
                # event before terminating the long-running monitor.
                lines = []
                for line_text in proc.stdout:
                    lines.append(line_text)
                    if 'HOOK_ON line=1' in line_text:
                        break
                proc.terminate()
            out, err = proc.communicate(timeout=2)
            if watch:
                out = ''.join(lines) + out
            if loop:
                assert (proc.returncode==0)==(mode=='loop'),(out,err)
                if mode=='loop': assert 'RTP 0->1=1 1->0=1' in out
            elif tone:
                assert proc.returncode == 0 and 'tone selector=0: DD01/16 status=0' in out, (out,err)
            elif ring:
                assert (proc.returncode == 0) == (mode != 'ring-lost-ack') and 'ring-stop: DD01/16 status=0' in out, (out,err)
            elif keypad:
                if mode == 'keypad':
                    assert proc.returncode == 0 and all(f'line={i} PASS' in out for i in (0,1))
                else:
                    assert proc.returncode != 0 and 'FAIL after 1 matched edges' in err
            elif mode in ('events','init','monitor','warm'):
                marker = 'event monitor armed' if mode == 'monitor' else 'ready: elapsed_ms='
                assert marker in out and (watch or proc.returncode == 0), (out,err)
                if watch:
                    for line in range(2):
                        for edge in ('ON','OFF','FLASH'):
                            assert f'HOOK_{edge} line={line}' in out
            else:
                expected = {'reject':'peer status=1', 'slic-error':'result=-1', 'cached-only':'waiting for DD02 completion','warm-reject':'recover-close slot=0 failed'}[mode]
                assert proc.returncode and expected in out+err and 'ready:' not in out, (out,err)
        finally:
            if proc.poll() is None:
                proc.kill()
            proc.wait()


if __name__ == '__main__':
    for name in ('init','events','cached-only','slic-error','reject','keypad','keypad-bad','monitor','ring','ring-lost-ack','ring-cid','tone','tone-stop','loop','loop-reject','warm','warm-reject'):
        scenario(sys.argv[1],name)
        print(name, 'passed')
