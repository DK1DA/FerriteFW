#!/bin/zsh
XEON=${FERRITE_XEON:-xeon} # the box with the bladeRF (an ssh host)
# meterrun.sh NAME [REG=VAL ...]: route 9 + 5A20, bk hold each REG=VAL, meter a sweep12; prints the low end against 1 kHz.
S=${BENCH_OUT:-/tmp/bench-blade}; mkdir -p $S
cd "$(dirname "$0")/../.."
L() { ../.venv/bin/python tools/ferrite_link.py ${PORT:-$(ls /dev/cu.usbserial-* | head -1)} "$@"; }
n=$1; shift
L term "bk rxaf 9700" >/dev/null 2>&1; L term "bk rxif 5a20" >/dev/null 2>&1; L term "bk hold off" >/dev/null 2>&1
for h in "$@"; do L term "bk hold ${h/=/ }" >/dev/null 2>&1; done
sleep 1
L term "meter on" >/dev/null 2>&1
f=$((446475000-200000))
ssh $XEON "bladeRF-cli -e 'set frequency tx $f; set samplerate tx 1000000; set bandwidth tx 1500000; set txvga1 -35; set txvga2 0; tx config file=/tmp/s12.sc16 format=bin repeat=1; tx start; tx wait' 2>&1 | grep -i -E 'error|fail' || true"
L term "meter off" >/dev/null 2>&1
L term "meter" 2>&1 | tail -6 > $S/m-$n.txt
r=$(for x in 28 73 7E; do L term "bk 1 $x" | tail -1 | awk '{print $2"="$4}'; done | tr '\n' ' ')
"${PY:-python3}" -I -c "
import re,sys
t=open('$S/m-$n.txt').read(); v=dict((int(a),float(b)) for a,b in re.findall(r'(\d+)\s+(-?\d+\.\d)',t.split('\n',1)[1]))
r=v[1000]; print('%-10s'%'$n', ' '.join('%d:%+.1f'%(f,v[f]-r) for f in (50,80,100,125,160,200,250,315,400,500,630,800,4000,8000)), ' [$r]')"
