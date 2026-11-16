#!/bin/zsh
# rxrun.sh NAME SECONDS fmgen-args...: generate, send from the bladeRF, record black's audio meanwhile.
S=${BENCH_DIR:-.}
n=$1; sec=$2; shift 2
cd $S && "${PY:-python3}" -I fmgen.py $n.sc16 "$@" >/dev/null
cd "$(dirname "$0")/../.."
($S/bladetx.sh $S/$n.sc16 446.475 1 > $S/tx.log 2>&1 &)
sleep 1
../.venv/bin/python tools/ferrite_link.py /dev/cu.usbserial-1460 audio $sec $S/$n.wav >/dev/null 2>&1
while pgrep -f bladetx.sh >/dev/null; do sleep 1; done
