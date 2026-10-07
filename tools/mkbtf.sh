#!/bin/sh
# Build and pack a Ferrite image for the stock bootloader, then check it with the flasher's dry run.
#
#   tools/mkbtf.sh            # target/ferrite.btf, the normal receive-only build
#   tools/mkbtf.sh bringup    # target/ferrite-bringup.btf: no SPI flash writes, key test on a held key, cable log
#
# Flash with: python3 tools/flash.py PORT target/ferrite-bringup.btf --backup flash.bin
set -e
cd "$(dirname "$0")/.."
name=ferrite
alias=fw
if [ "$1" = bringup ]; then
    name=ferrite-bringup
    alias=fw-bringup
elif [ -n "$1" ]; then
    echo "usage: $0 [bringup]" >&2
    exit 2
fi
cargo $alias
python3 tools/slowcheck.py | tail -1
bin=$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/host: //p')/bin
"$bin/llvm-objcopy" -O binary target/thumbv7em-none-eabihf/release/ferrite "target/$name.bin"
python3 tools/btf.py pack "target/$name.bin" "target/$name.btf"
python3 tools/flash.py --dry-run - "target/$name.btf"
shasum -a 256 "target/$name.btf"
