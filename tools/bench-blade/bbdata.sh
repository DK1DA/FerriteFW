#!/bin/zsh
# bbdata.sh NAME ROUTE OFFSET_HZ R17NAME: route ROUTE, the bladeRF plays r17/R17NAME.sc16 OFFSET_HZ above 446.475; 13 s raw 16 kHz.
S=${BENCH_OUT:-/tmp/bench-blade}; mkdir -p $S
cd /Users/infamy/Code/ferritefw/ferrite
L() { ../.venv/bin/python tools/ferrite_link.py ${PORT:-$(ls /dev/cu.usbserial-* | head -1)} "$@"; }
n=$1; r=$2; off=$3; src=$4
L term "bk rxaf $(printf '%X' $(( (r << 12) | 0x700 )))" >/dev/null 2>&1; L term "bk rxif 5a20" >/dev/null 2>&1
scp -q $S/r17/$src.sc16 infamy@192.168.7.162:/tmp/bb.sc16
f=$((446475000-200000+off))
(ssh infamy@192.168.7.162 "bladeRF-cli -e 'set frequency tx $f; set samplerate tx 1000000; set bandwidth tx 1500000; set txvga1 -35; set txvga2 0; tx config file=/tmp/bb.sc16 format=bin repeat=1; tx start; tx wait' >/dev/null 2>&1" &)
sleep 0.5
../.venv/bin/python tools/ferrite_link.py ${PORT:-$(ls /dev/cu.usbserial-* | head -1)} audio 14 $S/bb-$n.wav --raw 16000 | grep -o "[0-9]* frames lost.*clipped blocks"
while pgrep -f "/tmp/bb.sc16" >/dev/null; do sleep 0.5; done
