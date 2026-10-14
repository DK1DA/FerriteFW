#!/bin/sh
# Build and pack a Ferrite image for the stock bootloader, then check it with the flasher's dry run.
#
#   tools/mkbtf.sh            # target/ferrite.btf, the normal receive-only build
#   tools/mkbtf.sh bringup    # target/ferrite-bringup.btf: no SPI flash writes, key test on a held key, cable log
#   tools/mkbtf.sh tx         # target/ferrite-tx.btf: the transmit build
#   tools/mkbtf.sh tx-dry     # target/ferrite-txdry.btf: the transmit build with the PA drive held at 0
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
    tx-dry) name=ferrite-txdry build="fw --features tx-dry" ;;
    *) echo "usage: $0 [bringup | tx | tx-dry]" >&2; exit 2 ;;
esac
cargo $build
python3 tools/slowcheck.py | tail -1
bin=$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/host: //p')/bin
"$bin/llvm-objcopy" -O binary target/thumbv7em-none-eabihf/release/ferrite "target/$name.bin"
python3 tools/btf.py pack "target/$name.bin" "target/$name.btf"
python3 tools/flash.py --dry-run - "target/$name.btf"
shasum -a 256 "target/$name.btf"
