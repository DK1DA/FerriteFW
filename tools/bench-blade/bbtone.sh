#!/bin/zsh
# bbtone.sh NAME ROUTE OFFSET_HZ [FILE]: route ROUTE (REG_47), REG_43 5A20; the bladeRF sends FILE (default the plain carrier $S/cw.sc16, copied to the Xeon as /tmp/cw.sc16: DEV=0 fmgen.py cw.sc16 tone 1000 8) OFFSET_HZ from 446.475 MHz; records the raw 16 kHz stream and prints the strongest lines.
S=${BENCH_OUT:-/tmp/bench-blade}; mkdir -p $S
cd /Users/infamy/Code/ferritefw/ferrite
L() { ../.venv/bin/python tools/ferrite_link.py ${PORT:-$(ls /dev/cu.usbserial-* | head -1)} "$@"; }
n=$1; r=$2; off=$3; file=${4:-cw}
L term "bk rxaf $(printf '%X' $(( (r << 12) | 0x700 )))" >/dev/null 2>&1; L term "bk rxif 5a20" >/dev/null 2>&1
f=$((446475000-200000+off))
(ssh infamy@192.168.7.162 "bladeRF-cli -e 'set frequency tx $f; set samplerate tx 1000000; set bandwidth tx 1500000; set txvga1 -35; set txvga2 0; tx config file=/tmp/$file.sc16 format=bin repeat=1; tx start; tx wait' >/dev/null 2>&1" &)
sleep 2.5
../.venv/bin/python tools/ferrite_link.py ${PORT:-$(ls /dev/cu.usbserial-* | head -1)} audio 5 $S/rc-$n.wav --raw 16000 > /dev/null 2>&1
while pgrep -f "/tmp/$file.sc16" >/dev/null; do sleep 0.5; done
/Users/infamy/Code/ferritefw/.venv/bin/python -I -c "
import wave,numpy as np
w=wave.open('$S/rc-$n.wav'); x=np.frombuffer(w.readframes(w.getnframes()),np.int16).astype(float); fs=w.getframerate()
x=x[fs//2:]; x-=x.mean(); X=np.abs(np.fft.rfft(x*np.hanning(len(x))))*2/np.sum(np.hanning(len(x))); f=np.fft.rfftfreq(len(x),1/fs)
db=20*np.log10(X/32768+1e-12); m=(f>40)
idx=np.argsort(db*m)[::-1]; peaks=[]
for i in idx:
    if all(abs(f[i]-p)>80 for p,_ in peaks): peaks.append((f[i],db[i]))
    if len(peaks)==4: break
print('%-9s route %2d off %+6d | rms %.1f dBFS | lines '%('$n',$r,$off,20*np.log10(x.std()*1.414/32768)) + ' '.join('%.0f Hz %.1f'%p for p in peaks))"
