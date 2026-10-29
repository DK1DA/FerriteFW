#!/bin/zsh
# rxrun.sh NAME SECONDS fmgen-args...: generate, send from the bladeRF, record black's audio meanwhile.
S=/private/tmp/claude-501/-Users-infamy-Code-rt950pro/6e3a761c-8ad1-4445-8144-aa17805e9947/scratchpad
n=$1; sec=$2; shift 2
cd $S && /Users/infamy/Code/rt950pro/.venv/bin/python -I fmgen.py $n.sc16 "$@" >/dev/null
cd /Users/infamy/Code/rt950pro/ferrite
($S/bladetx.sh $S/$n.sc16 446.475 1 > $S/tx.log 2>&1 &)
sleep 1
../.venv/bin/python tools/ferrite_link.py /dev/cu.usbserial-1460 audio $sec $S/$n.wav >/dev/null 2>&1
while pgrep -f bladetx.sh >/dev/null; do sleep 1; done
