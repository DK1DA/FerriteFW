#!/bin/sh
# Build and pack a Ferrite image for the stock bootloader, then check it with the flasher's dry run.
#
#   tools/mkbtf.sh            # target/ferrite.btf, the normal receive-only build (a user build: no bench commands)
#   tools/mkbtf.sh bringup    # target/ferrite-bringup.btf: no SPI flash writes, key test on a held key, cable log,
#                             #   the bench commands
#   tools/mkbtf.sh tx         # target/ferrite-tx.btf: the transmit build (a user build: no bench commands)
#   tools/mkbtf.sh tx-dry     # target/ferrite-txdry.btf: the transmit build with the PA drive held at 0, and the
#                             #   bench commands (txtest, bk, si, mem, gpio, rx, mic, bt, gps query/nmea/standby)
#
# The bench commands are the `bench` cargo feature (decision 151, TERM-38): the firmware's `bringup` and `tx-dry`
# features turn it on (firmware/Cargo.toml), so `--features tx-dry` is tx + bench + the PA held at 0. The receive
# and tx images leave them out; their terminal says "not in this build: a bench build has it".
#
# Flash with: python3 tools/flash.py PORT target/ferrite-bringup.btf --backup flash.bin
set -e
cd "$(dirname "$0")/.."
name=ferrite
build="fw"
case "$1" in
    "") ;;
    bringup) name=ferrite-bringup build="fw-bringup" ;;
    tx) name=ferrite-tx build="fw --features tx" ;;
    tx-dry) name=ferrite-txdry build="fw --features tx-dry" ;; # tx + bench + dry
    *) echo "usage: $0 [bringup | tx | tx-dry]" >&2; exit 2 ;;
esac
cargo $build
# The fast-flash audit must pass: its last line, or the whole report and a stop when it fails.
check=$(python3 tools/slowcheck.py) || { echo "$check"; exit 1; }
echo "$check" | tail -1
bin=$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/host: //p')/bin
"$bin/llvm-objcopy" -O binary target/thumbv7em-none-eabihf/release/ferrite "target/$name.bin"
python3 tools/btf.py pack "target/$name.bin" "target/$name.btf"
python3 tools/flash.py --dry-run - "target/$name.btf"
shasum -a 256 "target/$name.btf"
