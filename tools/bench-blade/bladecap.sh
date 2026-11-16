#!/bin/zsh
XEON=${FERRITE_XEON:-xeon} # the box with the bladeRF (an ssh host)
# bladecap.sh MHZ SECONDS OUT.u8 [GAIN]: a bladeRF capture on the Xeon as sdrtx.py's rtl_sdr file (1.024 MS/s u8 IQ, the
# signal 250 kHz below the tuned centre), copied here.
set -eu
f=$(python3 -c "print(int(round($1*1e6))+250000)"); n=$(python3 -c "print(int($2*1024000))"); g=${4:-50}
ssh $XEON "rm -f /tmp/cap.sc16; bladeRF-cli -e 'set frequency rx $f; set samplerate rx 1024000; set bandwidth rx 1500000; set gain rx $g; rx config file=/tmp/cap.sc16 format=bin n=$n; rx start; rx wait' >/dev/null 2>&1; ls -la /tmp/cap.sc16 | awk '{print \$5}'"
scp -q $XEON:/tmp/cap.sc16 $3.sc16
"${PY:-python3}" -I -c "
import numpy as np, sys
iq = np.fromfile('$3.sc16', np.int16).astype(np.float32).reshape(-1, 2)[:, ::-1].ravel()  # bladeRF's spectrum is the mirror of rtl_sdr's: swap I and Q
iq -= iq.mean()
k = 100 / max(np.percentile(np.abs(iq), 99.9), 1)
np.clip(iq * k + 127.5, 0, 255).astype(np.uint8).tofile('$3')
print('u8 written, scale', round(k, 4))
"
