# SPDX-License-Identifier: GPL-2.0-or-later
"""T4.18 relative TX/RX bench; no microphone audio is echoed or saved."""
import argparse
import re
import shlex
import subprocess as sp
import threading
import time

import numpy as np


def measure(args, tx, rx):
    fs = 48000
    t = np.arange(fs * 6) / fs
    env = np.minimum(1, np.minimum(t, t[-1]-t) / .03)
    signal = .045*32767 * env * np.sin(2*np.pi*1500*t)
    pcm = np.column_stack([signal, np.zeros_like(t)]).astype('<i2').tobytes()
    cap = sp.Popen(['pw-record', '--raw', '--target', '0', '-P', '{ node.name = t418_gain_meter }',
                    '--channels','1','--channel-map','MONO','--format','s16','--rate',str(fs),
                    '--latency','256','-'], stdout=sp.PIPE)
    chunks = []
    reader = threading.Thread(target=lambda: chunks.append(cap.stdout.read()))
    reader.start(); play = None
    try:
        for _ in range(80):
            ports = [x for x in sp.check_output(['pw-link','-i'],text=True).splitlines() if x.startswith('t418_gain_meter:')]
            if len(ports)==1: break
            time.sleep(.04)
        if len(ports)!=1: raise RuntimeError('missing gain capture port')
        sp.run(['pw-link',args.mic,ports[0]],check=True)
        play = sp.Popen(['pw-play','--raw','--target',args.sink,'--channels','2','--format','s16',
                         '--rate',str(fs),'--latency','256','-'],stdin=sp.PIPE)
        sender=threading.Thread(target=lambda:play.communicate(pcm));sender.start()
        time.sleep(.5)
        cli=[args.binary,'gain','--line',str(args.line),'--tx',str(tx),'--rx',str(rx)]
        out=sp.check_output(['ssh','-o','BatchMode=yes',args.router,shlex.join(cli)],text=True,timeout=15)
        sender.join(9)
        if play.poll()!=0: raise RuntimeError('incomplete playback')
    finally:
        cap.terminate();cap.wait();reader.join()
        if play and play.poll() is None:play.terminate();play.wait()
    x=np.frombuffer(chunks.pop(),'<i2').astype(float);frames=x[:len(x)//960*960].reshape(-1,960)
    w=np.hanning(960); amps=abs(frames@(w*np.exp(-2j*np.pi*1000*np.arange(960)/fs)))*2/w.sum()
    levels=20*np.log10(np.maximum(amps,0.01)/32768); low,med=np.percentile(levels,[15,65]); peak=max(abs(x))
    m=re.search(r'frames=(\d+) tx1500_dbfs=([-\d.]+) input_peak=(\d+)',out)
    if not m: raise RuntimeError(out)
    print(f'line={args.line} tx={tx:+} rx={rx:+} frames={m[1]} TX1500={m[2]} RX1000={med:.2f} floor={low:.1f} tx_peak={m[3]} mic_peak={int(peak)}',flush=True)
    if med-low<10 or med < -82 or float(m[2]) < -65:raise RuntimeError('insufficient marker level/SNR; check coupling and microphone power')
    if int(m[3])>=32000 or peak>=32760:raise RuntimeError('near clipping; stop sweep')
    return float(m[2]),med


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('router');p.add_argument('binary');p.add_argument('sink');p.add_argument('mic');p.add_argument('--line',required=True,type=int,choices=[0,1]);p.add_argument('--span',type=int,default=1,choices=range(1,7));p.add_argument('--bracket',action='store_true',help='compare ±span one word at a time, each between zero references');a=p.parse_args()
    if a.router.startswith('-'):p.error('invalid SSH destination')
    rows=[]
    levels=[v for n in range(1,a.span+1) for v in (-n,n)]
    points=[(0,0)]+[(n,0) for n in levels]+[(0,n) for n in levels]+[(0,0)]
    if a.bracket:
        points=[(0,0)]
        for pair in [(-a.span,0),(a.span,0),(0,-a.span),(0,a.span)]:points.extend([pair,(0,0)])
    for tx,rx in points:
        rows.append(measure(a,tx,rx))
    print('baseline shift (TX/RX dB):',np.round(np.subtract(rows[-1],rows[0]),2))
    if a.bracket:
        for i in range(1,len(rows),2):
            print(points[i],'TX/RX delta vs bracketing zero average:',np.round(rows[i]-np.mean([rows[i-1],rows[i+1]],axis=0),2),
                  'zero-to-zero drift:',np.round(np.subtract(rows[i+1],rows[i-1]),2))
