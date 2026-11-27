# SPDX-License-Identifier: GPL-2.0-or-later
"""Exercise REGISTER recovery and a real message-summary SIP subscription."""
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

from calls import Board
from registration import response


def run(binary, modules, shim):
    with contextlib.ExitStack() as stack:
        root=Path(stack.enter_context(tempfile.TemporaryDirectory()))
        sip=stack.enter_context(socket.socket(socket.AF_INET,socket.SOCK_DGRAM));sip.bind(('127.0.0.1',0))
        port=sip.getsockname()[1]; aor=f'sip:701@127.0.0.1:{port}'
        for m in ('g711','account','mwi'):(root/(m+'.so')).symlink_to(Path(modules).resolve()/(m+'.so'))
        (root/'rbctl_voip.so').symlink_to(Path(shim).resolve())
        (root/'config').write_text(f'module_path {root}\nnet_interface 127.0.0.1\nsip_transports udp\nsip_listen 127.0.0.1:0\n'
            'rbctl_voip_device fixture\nrbctl_voip_require_registration0 1\nmodule g711.so\nmodule_app rbctl_voip.so\nmodule_app mwi.so\nmodule_app account.so\n')
        (root/'accounts').write_text(f'<{aor}>;regint=60;mwi=yes\n')
        x,y=socket.socketpair(socket.AF_UNIX,socket.SOCK_DGRAM);stack.enter_context(x);stack.enter_context(y)
        board=Board(x,0,0xa5);subs=[];regs=[];answers=[];registered=False
        log=stack.enter_context((root/'log').open('w+'));p=subprocess.Popen([binary,'-4','-f',str(root)],cwd=root,pass_fds=(y.fileno(),),env={**os.environ,'RBCTL_VOIP_TEST_FD':str(y.fileno())},stdin=subprocess.DEVNULL,stdout=log,stderr=log);stack.callback(p.wait);stack.callback(p.kill)
        def wait(pred):
            end=time.monotonic()+5
            while not pred():
                if p.poll() is not None or time.monotonic()>end:
                    log.seek(0);raise RuntimeError(f'{board.ids()} {log.read()}')
                ready,_,_=select.select([x,sip],[],[],.02)
                if x in ready:board.receive()
                if sip in ready:
                    msg,addr=sip.recvfrom(8192);lines=msg.decode().split('\r\n');h=dict((k.lower(),v.strip()) for k,v in (l.split(':',1) for l in lines[1:] if ':' in l))
                    if msg.startswith(b'REGISTER '):
                        regs.append((msg,addr))
                        if registered:sip.sendto(response(msg)[0],addr)
                    elif msg.startswith(b'SUBSCRIBE '):
                        assert h['event']=='message-summary';subs.append((h,addr))
                        out=['SIP/2.0 200 OK','Via: '+h['via'],'From: '+h['from'],'To: '+h['to']+';tag=pbx','Call-ID: '+h['call-id'],'CSeq: '+h['cseq'],f'Contact: <{aor}>','Expires: 600','Content-Length: 0','',''];sip.sendto('\r\n'.join(out).encode(),addr)
                    elif msg.startswith(b'SIP/2.0 '):answers.append((lines[0],h.get('cseq')))
        def selector():return [struct.unpack_from('>I',b,4)[0] for c,b in board.commands if c==13]
        wait(lambda:board.ids().count(7)==4 and regs);board.event(18,0);wait(lambda:selector()==[2]);registered=True
        req,addr=regs[-1];sip.sendto(response(req)[0],addr);wait(lambda:subs and selector()[-1]==4)
        h,addr=subs[-1];target=h['contact'].split('<')[-1].split('>')[0]
        for seq,(value,tone) in enumerate([('yes',19),('no',4)],1):
            body=f'Messages-Waiting: {value}\r\nVoice-Message: 0/0\r\n'
            out=[f'NOTIFY {target} SIP/2.0',f'Via: SIP/2.0/UDP 127.0.0.1:{port};branch=z9hG4bKn{seq}',
                 'To: '+h['from'],'From: '+h['to']+';tag=pbx','Call-ID: '+h['call-id'],f'CSeq: {seq} NOTIFY',
                 'Max-Forwards: 4',f'Contact: <{aor}>','Event: message-summary','Subscription-State: active;expires=600',
                 'Content-Type: application/simple-message-summary',f'Content-Length: {len(body)}','',body]
            sip.sendto('\r\n'.join(out).encode(),addr);wait(lambda:selector()[-1]==tone and ('SIP/2.0 200 OK',f'{seq} NOTIFY') in answers)
        board.event(17,0);wait(lambda:board.ids()[-1]==31)


if __name__=='__main__':run(*sys.argv[1:])
