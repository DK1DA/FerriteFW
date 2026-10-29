#!/bin/zsh
# txsw2.sh NAME "txtest ARGS": arm the txtest on black (cable), the bladeRF capturing, a 12 s key-up, sdrtx's sweep analysis.
S=/private/tmp/claude-501/-Users-infamy-Code-rt950pro/6e3a761c-8ad1-4445-8144-aa17805e9947/scratchpad
cd /Users/infamy/Code/rt950pro/ferrite; P=/dev/cu.usbserial-1460; py=../.venv/bin/python
$py tools/ferrite_link.py $P term "txtest $2" 2>&1 | grep -i -E 'refus|invalid|usage' | head -2
($S/bladecap.sh 446.475 18 $S/$1.u8 60 > $S/cap.log 2>&1 &)
sleep 4
$py tools/ferrite_link.py $P key --seconds 12 >/dev/null 2>&1
while pgrep -f bladecap.sh >/dev/null; do sleep 1; done
$py tools/sdrtx.py 446.475 --load $S/$1.u8 --sweep --out $S 2>&1 | grep -E 'deviation:|^ +[0-9]+ +[0-9]+ ' | awk '/deviation/{print "  " $0; next} {printf "%s:%s(%s) ", $1, $3, $2}'; echo
