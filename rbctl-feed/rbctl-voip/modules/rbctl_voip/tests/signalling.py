# SPDX-License-Identifier: GPL-2.0-or-later
"""Real baresip, loopback SIP and a simulated PHONE2. No credentials or audio files."""
import contextlib
from pathlib import Path
import os
import re
import select
import socket
import struct
import subprocess
import sys
import tempfile
import time

from calls import Board


def run(binary, modules, shim, case):
    with contextlib.ExitStack() as stack:
        root=Path(stack.enter_context(tempfile.TemporaryDirectory()))
        def udp():
            s=stack.enter_context(socket.socket(socket.AF_INET,socket.SOCK_DGRAM));s.bind(('127.0.0.1',0));return s
        peer,ua=udp(),udp(); dst=ua.getsockname(); ua.close(); port=peer.getsockname()[1]
        for name in ('account','g711'):
            (root/(name+'.so')).symlink_to(Path(modules).resolve()/(name+'.so'))
        (root/'rbctl_voip.so').symlink_to(Path(shim).resolve())
        (root/'config').write_text(f'module_path {root}\nnet_interface 127.0.0.1\nsip_listen 127.0.0.1:{dst[1]}\n'
            'sip_transports udp\nrbctl_voip_device fixture\nrbctl_voip_line 1\nrbctl_voip_alert_info1 internal\nrbctl_voip_alert_entry1 2\n'
            'audio_source rbctl_voip,phone\naudio_player rbctl_voip,phone\nmodule g711.so\nmodule_app rbctl_voip.so\nmodule_app account.so\n')
        (root/'accounts').write_text(f'<sip:701@127.0.0.1:{port}>;regint=0;audio_codecs=PCMA/8000/1\n')
        x,y=socket.socketpair(socket.AF_UNIX,socket.SOCK_DGRAM);stack.enter_context(x);stack.enter_context(y)
        board=Board(x,1,0xa5); packets=[]
        log=stack.enter_context((root/'log').open('w+'))
        proc=subprocess.Popen([binary,'-4','-c','-f',str(root)],cwd=root,pass_fds=(y.fileno(),),
            env={**os.environ,'RBCTL_VOIP_TEST_FD':str(y.fileno())},stdin=subprocess.DEVNULL,stdout=log,stderr=log)
        stack.callback(proc.wait); stack.callback(proc.kill)
        def wait(pred):
            end=time.monotonic()+4
            while not pred():
                if proc.poll() is not None or time.monotonic()>end:
                    log.seek(0);raise RuntimeError(f'{board.ids()}: {log.read()}')
                readable,_,_=select.select([x,peer],[],[],.05)
                if x in readable:board.receive()
                if peer in readable:packets.append(peer.recvfrom(8192))
        wait(lambda:board.ids().count(7)==4)
        if case=='early':
            net=stack.enter_context(socket.socket(socket.AF_INET,socket.SOCK_DGRAM)); net.bind(('127.0.0.1',0))
            board.event(18,1)
            for k in '777#':board.event(20,1,ord(k))
            wait(lambda:any(p.startswith(b'INVITE ') for p,_ in packets));req,addr=next((p,a) for p,a in packets if p.startswith(b'INVITE '))
            hdr=[l for l in req.split(b'\r\n') if l.lower().startswith((b'via:',b'to:',b'from:',b'call-id:',b'cseq:'))]
            hdr=[l+b';tag=early' if l.lower().startswith(b'to:') else l for l in hdr]
            peer.sendto(b'\r\n'.join([b'SIP/2.0 180 Ringing',*hdr,b'Content-Length: 0',b'',b'']),addr)
            wait(lambda:any(len(set(p))>4 for p in board.played))
            port_rx=int(re.search(rb'm=audio (\d+) RTP/AVP',req).group(1)); body=f'v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=e\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=audio {net.getsockname()[1]} RTP/AVP 8\r\na=sendrecv\r\n'.encode()
            peer.sendto(b'\r\n'.join([b'SIP/2.0 183 Session Progress',*hdr,f'Contact: <sip:777@127.0.0.1:{port}>'.encode(),b'Content-Type: application/sdp',f'Content-Length: {len(body)}'.encode(),b'',body]),addr)
            for seq in range(40):
                net.sendto(struct.pack('>BBHII',128,8,seq,160*seq,777)+b'\x37'*160,('127.0.0.1',port_rx))
                until=time.monotonic()+.02;wait(lambda:time.monotonic()>=until)
            assert b'\x37'*160 in board.played and 31 not in board.ids()
            board.event(17,1);wait(lambda:board.ids()[-1]==31);return
        if case=='failures':
            for code,selector in [(486,1),(600,1),(603,1),(404,None),(410,None),(484,None),(604,None),
                                  (408,1),(480,1),(487,1),(500,2),(503,2)]:
                packets.clear(); board.commands.clear();board.event(18,1)
                for k in '777#':board.event(20,1,ord(k))
                def invite():return next(((p,a) for p,a in packets if p.startswith(b'INVITE ')),None)
                wait(invite);req,addr=invite()
                hdr=[l for l in req.split(b'\r\n') if l.lower().startswith((b'via:',b'to:',b'from:',b'call-id:',b'cseq:'))]
                hdr=[l+b';tag=fixture' if l.lower().startswith(b'to:') else l for l in hdr]
                peer.sendto(b'\r\n'.join([f'SIP/2.0 {code} fixture'.encode(),*hdr,b'Content-Length: 0',b'',b'']),addr)
                if selector is None:wait(lambda:10 in board.ids())
                else:wait(lambda:any(c==13 and struct.unpack_from('>I',b,4)[0]==selector for c,b in board.commands))
                board.event(17,1);wait(lambda:board.ids()[-1]==31)
            return
        for no,(extra,num,name,entry) in enumerate([
                ('P-Asserted-Identity: "Bench" <sip:12345678901234567@example.invalid>\r\nRemote-Party-ID: <sip:444@example.invalid>\r\nAlert-Info: <urn:alert:test>;info=internal\r\n','123456789012345','Bench',2),
                ('Remote-Party-ID: "Remote" <sip:444@example.invalid>\r\n','444','Remote',0),
                ('Privacy: id\r\nP-Asserted-Identity: "Secret" <sip:333@example.invalid>\r\n','P','',0),
                ('','O','From',0)]):
            uri=f'sip:701@127.0.0.1:{dst[1]}';branch=f'z9hG4bK-lab{no}';board.commands.clear()
            def request(method,body=''):
                headers=(f'{method} {uri} SIP/2.0\r\nVia: SIP/2.0/UDP 127.0.0.1:{port};branch={branch}\r\n'
                    f'From: "From" <sip:lab@127.0.0.1:{port}>;tag=f{no}\r\nTo: <{uri}>\r\nCall-ID: bench{no}@localhost\r\n'
                    f'CSeq: 1 {method}\r\nMax-Forwards: 3\r\nContact: <sip:lab@127.0.0.1:{port}>\r\n'+
                    (extra+'Content-Type: application/sdp\r\n' if body else '')+f'Content-Length: {len(body)}\r\n\r\n'+body)
                peer.sendto(headers.encode(),dst)
            request('INVITE',f'v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=test\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=audio {port} RTP/AVP 8\r\n');wait(lambda:5 in board.ids())
            cid=next(b for c,b in board.commands if c==4)
            assert cid[69:109].split(b'\0')[0]==num.encode(),cid[69:109]
            assert cid[29:69].split(b'\0')[0]==name.encode(),cid[29:69]
            assert cid[20:28].isdigit() and struct.unpack_from('>I',board.commands[-1][1],8)[0]==entry
            request('CANCEL');wait(lambda:6 in board.ids())


if __name__=='__main__':run(*sys.argv[1:])
