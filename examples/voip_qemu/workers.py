"""Normal traffic fixture for the stock AArch64 proxy's main and worker threads."""
from contextlib import ExitStack
import os
from pathlib import Path
import select
import socket
import struct
import subprocess
import tempfile
import time

from run import prepare, crc16_arc

HOST = bytes.fromhex('020000000001')
BOARD = bytes.fromhex('020000000002')


def frame(etype, sub, seq, body):
    hdr = struct.pack('>BBIH', 17, sub, seq, len(body))
    crc = crc16_arc(hdr + bytes(2) + body)
    return HOST + BOARD + struct.pack('>H', etype) + hdr + struct.pack('>H', crc) + body


def expect(sock, etype, sub, seq, body):
    packet = sock.recv(2048)
    wanted = frame(etype, sub, seq, body)
    assert packet[6:12] == HOST and packet[12:len(wanted)] == wanted[12:], (etype, sub, seq, packet.hex())
    assert not any(packet[len(wanted):])
    return packet


def dsp(cmd, args=b''):
    return struct.pack('>I', cmd) + args.ljust(128, b'\0')


def word(x):
    return struct.pack('>I', x)


def udp(port=0):
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        s.bind(('127.0.0.1', port))
    except OSError:
        s.close()
        raise
    s.settimeout(3)
    return s


def local_pair():
    for _ in range(100):
        with udp() as s:
            port = s.getsockname()[1]
            if port >= 65535:
                continue
            try:
                with udp(port+1):
                    return port
            except OSError:
                pass
    raise RuntimeError('no available loopback UDP port pair')


def scenario(tmp, args, ring_records):
    with ExitStack() as stack:
        peers, env = [], {}
        for key in ('DD01_FD', 'DD02_FD', 'DD03_FD'):
            a, b = socket.socketpair(socket.AF_UNIX, socket.SOCK_DGRAM)
            stack.enter_context(a); stack.enter_context(b)
            a.settimeout(4); peers.append(a); env[key] = str(b.fileno())
        ctl, events, media = peers
        proc = subprocess.Popen(args, cwd=tmp, env={**os.environ, **env},
                                pass_fds=tuple(map(int, env.values())))
        stack.callback(proc.wait); stack.callback(proc.kill)
        seq = 0
        def transaction(sub, body, retry=False):
            nonlocal seq
            seq += 1
            pkt = expect(ctl, 0xdd01, sub, seq, body)
            assert pkt[:6] == (bytes([255])*6 if seq == 1 else BOARD)
            if retry:
                assert ctl.recv(2048) == pkt  # omit one ACK; normal timeout retransmission
            ctl.send(frame(0xdd01, sub, seq, bytes(4)))

        transaction(0x1a, b'\1\0\0\0')
        client = stack.enter_context(socket.socket(socket.AF_UNIX, socket.SOCK_STREAM))
        client.settimeout(4)
        for _ in range(100):
            try:
                client.connect(str(tmp/'dsp')); break
            except (FileNotFoundError, ConnectionRefusedError):
                time.sleep(.02)
        else:
            raise RuntimeError('OEM DSP listener did not start')
        transaction(0x1b, b'\1\0\0\0')
        assert not (tmp/'init-marker').exists()
        print('boot: DD01 1a broadcast -> success -> DSP connect -> 1b unicast')

        event_seq = 0
        def event(cmd, args):
            nonlocal event_seq
            event_seq += 1
            rec = dsp(cmd, args)
            events.send(frame(0xdd02, 0x18, event_seq, rec))
            expect(events, 0xdd02, 0x18, event_seq, bytes(4))
            assert client.recv(132, socket.MSG_WAITALL) == rec

        def command(cmd, args, retry=False):
            rec = dsp(cmd, args); client.sendall(rec); transaction(0x16, rec, retry)

        for line in (0, 1):
            with udp() as remote:
                slot = 3 - line  # distinguish FXS identity from the media slot
                port = local_pair(); dst = remote.getsockname()[1]
                for index, cmd in enumerate((4,5)):
                    start=(2*line+index)*132
                    record=ring_records[start:start+132]
                    assert record[:8]==word(cmd)+word(line)
                    if cmd==4:
                        assert record[24:32]==b'01020304' and record[33:40]==b'Fixture' and record[73:80]==b'5550100'
                    command(cmd,record[4:],retry=cmd==5 and line==0)
                event(18, word(line)); command(6, word(line))
                # Projected IPv4 media parameters. The fixture chooses the
                # slot and codec; the proxy does not negotiate a SIP session.
                cfg = bytearray(120)
                for pos, value in ((0x10,slot),(0x18,2),(0x28,20),(0x74,2)):
                    struct.pack_into('>I', cfg, pos, value)
                cfg[60:64] = cfg[68:72] = socket.inet_aton('127.0.0.1')
                struct.pack_into('>H',cfg,64,port); struct.pack_into('>HH',cfg,72,dst,dst)
                command(10, cfg)
                rtp = struct.pack('>BBHII',128,8,1,160,line+1) + bytes([0xd5])*160
                prefix = struct.pack('>HH',slot,3)
                # Relay pipe and control ACK are independent. Poll with local
                # media until the worker has installed its socket.
                for _ in range(50):
                    remote.sendto(rtp, ('127.0.0.1',port))
                    if select.select([media],[],[],.05)[0]: break
                expect(media,0xdd03,0x19,0,prefix+rtp)
                media.send(frame(0xdd03,0x19,0,prefix+rtp))
                assert remote.recvfrom(2048) == (rtp, ('127.0.0.1',port))
                rtcp = bytes.fromhex('80c9000100000001')
                remote.sendto(rtcp, ('127.0.0.1',port+1))
                rtcp_body = struct.pack('>HH',slot,4)+rtcp
                expect(media,0xdd03,0x19,0,rtcp_body)
                # IPv4 consumer sets RTCP destination to its own local RTP+1.
                # Check what actually comes back, rather than asserting the
                # advertised remote RTCP port must have received it.
                media.send(frame(0xdd03,0x19,0,rtcp_body))
                expect(media,0xdd03,0x19,0,rtcp_body)
                assert not select.select([remote],[],[],.05)[0]
                new_port = local_pair()
                struct.pack_into('>H',cfg,64,new_port)
                command(11,cfg)
                for _ in range(50):
                    remote.sendto(rtp, ('127.0.0.1',new_port))
                    if select.select([media],[],[],.05)[0]: break
                expect(media,0xdd03,0x19,0,prefix+rtp)
                with udp(port), udp(port+1): pass
                port = new_port
                for digit_event in (24,20): event(digit_event,word(line)+word(5))
                event(17, word(line)); command(12, word(slot)); command(31, word(slot))
                for _ in range(100):
                    try:
                        with udp(port), udp(port+1): break
                    except OSError:
                        time.sleep(.01)
                else: raise RuntimeError('media sockets not released')
                print(f'port {line+1}, slot {slot}: OEM CID+ring -> off-hook -> stop ring -> RTP both ways; '
                      'IPv4 RTCP return observed on DD03; port update -> DTMF edges -> on-hook -> teardown')
        client.close()
        # Local EOF is handled asynchronously by msg_pre_process. A new
        # connection can be rejected until the old singleton slot clears.
        deadline = time.monotonic()+4
        while time.monotonic()<deadline:
            client = stack.enter_context(socket.socket(socket.AF_UNIX, socket.SOCK_STREAM))
            client.settimeout(3); client.connect(str(tmp/'dsp'))
            if select.select([ctl],[],[],.1)[0]: break
            client.close()
        else: raise RuntimeError('local reattach did not notify peer')
        transaction(0x1b,b'\1\0\0\0')
        print(f'{seq} DD01 transactions including local reattach, one identical retry, '
              f'{event_seq} DD02 events; proxy still running:', proc.poll() is None)
        assert proc.poll() is None


if __name__ == '__main__':
    with tempfile.TemporaryDirectory(prefix='voip-workers-') as temp:
        path = Path(temp)
        cmd = prepare(path, 'worker_io.c')
        records = subprocess.check_output(cmd('voip_client'), cwd=path, timeout=5)
        assert len(records)==528
        scenario(path, cmd('voip_server_proxy'), records)
