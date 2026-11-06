#!/bin/zsh
# rxber.sh NAME RXDATA-ARGS -- FMGEN-ARGS: generate (12 s), play once from the bladeRF, record black's raw 16 kHz, decode.
S=${BENCH_OUT:-/tmp/bench-blade}; mkdir -p $S
B=/Users/infamy/Code/ferritefw/ferrite/tools/bench-blade
P=/Users/infamy/Code/ferritefw/.venv/bin/python
n=$1; shift; rx=(); while [[ $1 != -- ]]; do rx+=$1; shift; done; shift
mkdir -p $S/r17; cd $S/r17
[[ -f $n.sc16 ]] || $P -I $B/fmgen.py $n.sc16 "$@" >/dev/null
scp -q $n.sc16 infamy@192.168.7.162:/tmp/r17.sc16
f=$((446475000-200000))
(ssh infamy@192.168.7.162 "bladeRF-cli -e 'set frequency tx $f; set samplerate tx 1000000; set bandwidth tx 1500000; set txvga1 ${VGA1:--35}; set txvga2 0; tx config file=/tmp/r17.sc16 format=bin repeat=1; tx start; tx wait' 2>&1 | grep -i -E 'error|fail'" > tx.log 2>&1 &)
sleep 0.5
cd /Users/infamy/Code/ferritefw/ferrite
$P tools/ferrite_link.py ${PORT:-$(ls /dev/cu.usbserial-* | head -1)} audio 15 $S/r17/$n.wav --raw 16000 > $S/r17/$n.raw.log 2>&1
while pgrep -f "r17.sc16" >/dev/null; do sleep 0.5; done
cd $S/r17
grep -o "[0-9]* frames lost.*clipped blocks" $n.raw.log
$P -I $B/rxdata.py $n.wav $rx | tail -2
$P -I $B/rxdata.py $n.wav $rx eq | tail -2 | sed 's/^/  eq/'
