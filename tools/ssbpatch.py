#!/usr/bin/env python3
"""Find the Si473x SSB patch in a stock RT-950 Pro firmware file, so Ferrite can import it from the user's own copy.

    python3 tools/ssbpatch.py RT_950Pro_V0.29_OEM.BTF ssb.bin       # also takes an unpacked image (.bin)
    python3 tools/flash.py <port> ferrite.btf --ssb-from RT_950Pro_V0.29_OEM.BTF   # the usual way: see below

Ferrite never ships the patch (its origin is unclear and its users discourage redistribution). Instead the
owner's own stock firmware file is handed to the flasher, which bundles the patch into the image it is about to
flash; Ferrite moves it into SPI flash (region 0x06B000, see docs/firmware-design.md). This tool knows nothing of the patch's contents:
it finds the longest run of 8-byte Si473x patch records (command byte 0x15 starts a block, 0x16 continues
it), which is how the chip's patch download is framed. Prints the record count, size and SHA-256.

Delivery: the flasher bundles the patch into the image it is about to flash, on the user's machine (never a
published file). The bundle is a 64-byte header and the patch, placed at the first 1 KB boundary after the
image. Ferrite looks there on boot, checks it and copies it to SPI flash, where later updates leave it alone.
Header: b"FERRSSB1", u32 length, u32 record count, u32 reserved, 32-byte SHA-256 of the patch, zeros to 64.
"""
import hashlib
import os
import struct
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import btf  # noqa: E402

REC = 8
KNOWN_SHA256 = "343076b9e4726056e8db469834f2610e5683095bf19b41674eb5d5bf0fa3dc9f"   # stock V0.18-V0.29
MAGIC = b"FERRSSB1"
HEADER = 64


def find(img):
    """(offset, records) of the longest run of patch records, aligned to 8 bytes within the run."""
    best = (0, 0)
    for phase in range(REC):
        start, n = None, 0
        for off in range(phase, len(img) - REC + 1, REC):
            ok = img[off] in (0x15, 0x16) and (n or img[off] == 0x15)
            if ok:
                if start is None:
                    start, n = off, 0
                n += 1
            else:
                if start is not None and n > best[1]:
                    best = (start, n)
                start, n = None, 0
        if start is not None and n > best[1]:
            best = (start, n)
    return best


def extract(path):
    """The patch from a stock .BTF or image, or None."""
    data = open(path, "rb").read()
    img = btf.unpack(data)[0] if path.upper().endswith(".BTF") else data
    off, n = find(img)
    return img[off:off + n * REC] if n >= 500 else None


def bundle(img, patch):
    """Image with the patch appended at the next 1 KB boundary after its last non-0xFF byte."""
    end = len(img.rstrip(b"\xFF"))
    at = -(-end // 1024) * 1024
    sha = hashlib.sha256(patch).digest()
    head = MAGIC + struct.pack("<III", len(patch), len(patch) // REC, 0) + sha
    head += bytes(HEADER - len(head))
    return img[:end] + b"\xFF" * (at - end) + head + patch, at


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    data = open(sys.argv[1], "rb").read()
    img = btf.unpack(data)[0] if sys.argv[1].upper().endswith(".BTF") else data
    off, n = find(img)
    if n < 500:                       # the known patch is ~2,000 records; a short run is not it
        sys.exit(f"no SSB patch found (longest record run: {n})")
    patch = img[off:off + n * REC]
    open(sys.argv[2], "wb").write(patch)
    blocks = sum(1 for i in range(0, len(patch), REC) if patch[i] == 0x15)
    print(f"{sys.argv[2]}: {len(patch)} bytes, {n} records in {blocks} block(s), "
          f"at 0x{btf.APP_BASE + off:08X}, sha256 {hashlib.sha256(patch).hexdigest()}")


if __name__ == "__main__":
    main()
