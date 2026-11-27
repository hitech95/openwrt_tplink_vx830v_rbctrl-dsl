# SPDX-License-Identifier: GPL-2.0-or-later
"""T4.10: summarize one earpiece tone via PipeWire, without saving audio."""
import argparse
import math
import re
import shlex
import subprocess
import threading
import time

import numpy as np


def summary(samples, rate=48000):
    """20-ms RMS envelope and aggregate spectral peaks. No semantic labels."""
    block = rate // 50
    frames = samples[:len(samples) // block * block].reshape(-1, block)
    if not len(frames):
        raise ValueError('empty microphone window')
    # Last ~1 s is the post-STOP reference. Detect tonal power over its
    # per-bin noise floor rather than rejecting a quiet but clean signal.
    spectra = abs(np.fft.rfft(frames*np.hanning(block), n=8192))**2
    background=np.maximum(np.percentile(spectra[-35:],75,axis=0),1e-6)
    candidate=spectra[20:-45]
    if not len(candidate): return 'window too short'
    # A one-shot tone can occupy <5% of the play window (selectors 16/17).
    # A consecutive-frame pair captures a brief burst but rejects a single
    # isolated FFT frame. Use post-stop noise and manual corroboration.
    delta=np.maximum(np.max(np.minimum(candidate[:-1],candidate[1:]),axis=0)-background,0)
    valid=(np.arange(len(delta))*rate/8192>=200)&(np.arange(len(delta))*rate/8192<=3500)&(delta>12*background)
    if not np.any(valid):return 'no resolved tone above background (confirm by ear)'
    k=int(np.argmax(np.where(valid,delta,0)))
    active=spectra[:,k] > background[k]+max(6*background[k],delta[k]*0.12)
    if not np.any(active):
        return 'below detection threshold (confirm by ear)'
    power = np.maximum(np.mean(spectra[active],axis=0)-background,0)
    bins = np.arange(1, len(power)-1)
    peaks = bins[(power[bins]>power[bins-1]) & (power[bins]>=power[bins+1]) &
                 (bins*rate/8192 >= 200) & (bins*rate/8192 <= 3500)]
    ordered = sorted(peaks, key=lambda k: power[k], reverse=True)
    kept = []
    for k in ordered:
        if power[k] < power[ordered[0]] * .025: break
        if power[k] < 6*background[k]: continue
        if all(abs(k-j)*rate/8192 > 80 for j in kept): kept.append(k)
    changes = np.r_[0, np.flatnonzero(np.diff(active))+1, len(active)]
    runs = [('on' if active[i] else 'off', int(j-i)*20) for i,j in zip(changes, changes[1:])]
    return (f'peaks Hz={[round(k*rate/8192) for k in kept]}, '
            f'envelope ms={runs}, peak={20*math.log10(max(abs(samples))):.1f} dBFS')


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('router'); ap.add_argument('binary'); ap.add_argument('source')
    ap.add_argument('--line', type=int, choices=[0,1], required=True)
    ap.add_argument('--seconds', type=int, choices=range(2,16), default=5)
    ap.add_argument('selectors', type=int, nargs='+'); args=ap.parse_args()
    if not re.fullmatch(r'[a-zA-Z0-9_.@-]+',args.router) or args.router.startswith('-'):
        ap.error('invalid SSH destination')
    for sel in args.selectors:
        cmd=[args.binary,'tone','--line',str(args.line),'--sel',str(sel),'--duration-ms',str(args.seconds*1000)]
        n=(args.seconds+2)*48000
        with subprocess.Popen(['pw-record','--raw','--target',args.source,'--rate','48000','--format','s16',
                               '--channels','1','--channel-map','MONO','--latency','256','-'], stdout=subprocess.PIPE) as mic:
            # Drain continuously: leaving the pipe full after collecting the
            # requested window can block pw-record's shutdown.
            buf=bytearray()
            def drain():
                while block:=mic.stdout.read(4096):
                    buf.extend(block[:max(0,n*2-len(buf))])
            reader=threading.Thread(target=drain,daemon=True); reader.start()
            try:
                time.sleep(.35)
                print(f'line={args.line} selector={sel} starting', flush=True)
                subprocess.run(['ssh','-o','BatchMode=yes',args.router,shlex.join(cmd)],check=True,
                               timeout=args.seconds+14,stdout=subprocess.DEVNULL)
                if sel == 0:  # standalone STOP returns immediately; measure background
                    time.sleep(args.seconds+2)
            finally:
                mic.terminate(); mic.wait(); reader.join(2)
        if len(buf)!=n*2: raise RuntimeError('incomplete mic window')
        print(summary(np.frombuffer(buf,'<i2').astype(float)/32768),flush=True)


if __name__=='__main__': main()
