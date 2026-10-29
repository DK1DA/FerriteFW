#!/bin/zsh
# bladetx.sh FILE.sc16 [MHZ]: send the file once from the Xeon's bladeRF, centre MHZ - 0.2 (the file's signal is
# 200 kHz above), at the lowest TX gains.
set -eu
f=$(python3 -c "print(int(round(${2:-446.475}*1e6))-200000)")
scp -q $1 infamy@192.168.7.162:/tmp/tx.sc16
ssh infamy@192.168.7.162 "bladeRF-cli -e 'set frequency tx $f; set samplerate tx 1000000; set bandwidth tx 1500000; set txvga1 -35; set txvga2 0; tx config file=/tmp/tx.sc16 format=bin repeat=${3:-1}; tx start; tx wait' 2>&1 | grep -i -E 'error|fail' || true"
