# SPDX-License-Identifier: GPL-2.0-or-later
"""Offline managed-module smoke. Run with a matching AArch64 OpenWrt staging
directory, baresip prefix and RBCTL_UBUS + RBCTL_HARNESS module (in that order).
All sockets/configs are private; SIP binds only to loopback.
"""
import contextlib
import json
import os
from pathlib import Path
import select
import socket
import subprocess as sp
import sys
import tempfile
import threading
import time

pkg = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(pkg/'modules/rbctl_voip/tests'))
from calls import Board
from registration import response


def run(staging, prefix, module):
    root = staging/'root-mediatek'
    lib = f'{prefix}/lib:{staging}/usr/lib:{root}/usr/lib'
    q = ['qemu-aarch64', '-L', str(root), '-E', f'LD_LIBRARY_PATH={lib}']
    with contextlib.ExitStack() as stack:
        d = Path(stack.enter_context(tempfile.TemporaryDirectory()))
        for directory in ('uci', 'out', 'mods', 'acl'):
            (d/directory).mkdir()
        # Standalone ubusd runs as the invoking user, grant access only on this
        # private socket. Production rpcd authorization is tested in T5.7.
        import pwd
        me = pwd.getpwuid(os.getuid()).pw_name
        (d/'acl/test.json').write_text(json.dumps({'user': me, 'publish':['voip'],
            'access':{'voip':{'methods':['*']}}, 'send':['voip.*'], 'listen':['voip.*']}))
        def spawn(args, **kwargs):
            p = sp.Popen(args, **kwargs)
            stack.callback(p.wait); stack.callback(p.kill)
            return p
        bus = str(d/'ubus.sock')
        daemon=spawn(q+[staging/'usr/sbin/ubusd','-s',bus,'-A',d/'acl'])
        for _ in range(80):
            if os.path.exists(bus): break
            if daemon.poll() is not None: raise RuntimeError('ubusd needs a writable private /run (use bwrap)')
            time.sleep(.05)
        base = (pkg/'files/voip.config').read_text()
        registrar=stack.enter_context(socket.socket(socket.AF_INET,socket.SOCK_DGRAM))
        registrar.bind(('127.0.0.1',0))
        domain=f'127.0.0.1:{registrar.getsockname()[1]}'
        contacts={}
        def render(user, origin=True):
            (d/'uci/voip').write_text(base.replace("enabled '0'", "enabled '1'")
                .replace("user ''", f"user '{user}'").replace("domain ''", f"domain '{domain}'\n\toption expires '60'" + ('' if origin else "\n\toption check_origin '0'")))
            sp.run(q+[root/'usr/bin/ucode','-L',str(root/'usr/lib/ucode/*.so'),
                pkg/'files/render.uc',d/'out',d/'uci'],check=True)
            cfg = d/'out/config'
            cfg.write_text(cfg.read_text().replace('/usr/lib/baresip/modules',str(d/'mods'))
                .replace('lan0.500','fixture')+f'net_interface 127.0.0.1\nsip_listen 127.0.0.1:0\nrbctl_voip_ubus_socket {bus}\n')
        render('test1')
        for p in (prefix/'lib/baresip/modules').glob('*.so'):
            (d/'mods'/p.name).symlink_to(p)
        (d/'mods/rbctl_voip.so').unlink(missing_ok=True)
        (d/'mods/rbctl_voip.so').symlink_to(module)
        x,y = socket.socketpair(socket.AF_UNIX,socket.SOCK_DGRAM)
        stack.enter_context(x); stack.enter_context(y)
        board=Board(x,None,0xd5); stop=threading.Event()
        def service():
            while not stop.is_set():
                ready=select.select([x,registrar],[],[],.1)[0]
                if x in ready: board.receive()
                if registrar in ready:
                    pkt,peer=registrar.recvfrom(8192)
                    if pkt.startswith(b'REGISTER '):
                        registrar.sendto(response(pkt)[0],peer); contacts['ua']=peer
        thread=threading.Thread(target=service); thread.start()
        stack.callback(thread.join); stack.callback(stop.set)
        ua=spawn(q+[prefix/'bin/baresip','-4','-c','-f',d/'out'],pass_fds=(y.fileno(),),
            stdin=sp.DEVNULL, env={**os.environ,'RBCTL_VOIP_TEST_FD':str(y.fileno())})
        def rpc(method, args='{}', check=True):
            return sp.run(q+[staging/'usr/bin/ubus','-s',bus,'-t','2','call','voip',method,args],
                          capture_output=True,text=True,timeout=5,check=check)
        for _ in range(60):
            if ua.poll() is not None: raise RuntimeError('baresip exited')
            result=rpc('status',check=False)
            if not result.returncode and json.loads(result.stdout)['board']['state']=='ready':break
            time.sleep(.1)
        else: raise RuntimeError(f'no ready status: {result.stderr}')
        assert json.loads(result.stdout)['board']['peer_mac']
        assert json.loads(rpc('calls').stdout)=={'calls':[]}
        assert len(json.loads(rpc('lines').stdout)['lines'])==2
        render('test2');rpc('reload')
        for _ in range(30):
            if json.loads(rpc('status').stdout)['accounts']==[{'aor':f'sip:test2@{domain}','registered':True}]: break
            time.sleep(.1)
        else: raise AssertionError('replacement account did not register')
        assert board.attach_count==1
        def incoming(user, tag, allowed):
            # A different loopback address, not just a different source port.
            with socket.socket(socket.AF_INET,socket.SOCK_DGRAM) as sip:
                sip.bind(('127.0.0.2',0)); sip.settimeout(3)
                addr=f'127.0.0.2:{sip.getsockname()[1]}'
                uri=f'sip:{user}@{domain}'
                body='v=0\r\no=- 1 1 IN IP4 127.0.0.2\r\ns=test\r\nc=IN IP4 127.0.0.2\r\nt=0 0\r\nm=audio 15580 RTP/AVP 8\r\n'
                def send(method, data):
                    msg=(f'{method} {uri} SIP/2.0\r\nVia: SIP/2.0/UDP {addr};branch=z9hG4bK{tag}\r\n'
                         f'From: <sip:test@{addr}>;tag={tag}\r\nTo: <{uri}>\r\nCall-ID: {tag}@fixture\r\n'
                         f'CSeq: 1 {method}\r\nMax-Forwards: 4\r\nContact: <sip:test@{addr}>\r\n'
                         + ('Content-Type: application/sdp\r\n' if data else '') + f'Content-Length: {len(data)}\r\n\r\n'+data)
                    sip.sendto(msg.encode(),contacts['ua'])
                rings=board.ids().count(5); send('INVITE',body)
                while True:
                    code=int(sip.recv(8192).split()[1])
                    if code!=100: break
                if allowed:
                    for _ in range(50):
                        if board.ids().count(5)>rings: break
                        time.sleep(.02)
                    assert code==180 and board.ids().count(5)>rings, code
                    send('CANCEL','')
                else:
                    assert code==403 and board.ids().count(5)==rings, code
        incoming('test2','restricted',False)
        # Invalid candidate must leave previously registered UAs in place.
        cfg=d/'out/config'; good=cfg.read_text()
        cfg.write_text(good.replace('rbctl_voip_tx_gain0 0','rbctl_voip_tx_gain0 99'))
        assert rpc('reload',check=False).returncode
        assert json.loads(rpc('status').stdout)['accounts'][0]['registered']
        cfg.write_text(good)
        assert rpc('dial','{"line":2,"uri":"sip:invalid@127.0.0.1"}',False).returncode
        rpc('dial','{"line":1,"uri":"sip:123@127.0.0.1:15999"}')
        assert json.loads(rpc('calls').stdout)['calls'][0]['line']==1
        render('test3',False); rpc('reload')
        status=json.loads(rpc('status').stdout)
        assert status['reload_pending'] and status['accounts'][0]['aor']==f'sip:test2@{domain}'
        assert json.loads(rpc('calls').stdout)['calls'][0]['line']==1
        rpc('hangup','{"line":1}');assert not json.loads(rpc('calls').stdout)['calls']
        for _ in range(30):
            status=json.loads(rpc('status').stdout)
            if not status['reload_pending'] and status['accounts']==[{'aor':f'sip:test3@{domain}','registered':True}]: break
            time.sleep(.1)
        else: raise AssertionError('deferred reload did not register latest AOR')
        incoming('test3','unrestricted',True)
        print('PASS: UCI -> two lines, ready/status, new AOR without reattach, reload queued and applied after hangup')


if __name__=='__main__':
    run(*(Path(s).resolve() for s in sys.argv[1:]))
