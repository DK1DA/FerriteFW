#!/usr/bin/env python3
"""Pack and unpack RT-950 Pro .BTF update files (the format the stock bootloader takes).

    python3 tools/btf.py pack build/target/hwtest.bin hwtest.btf
    python3 tools/btf.py unpack RT_950Pro_V0.27_260203.BTF v027.bin
    python3 tools/btf.py info hwtest.btf
    python3 tools/btf.py selftest [--stock ../firmware/stock]

Format (checked against the bootloader and every stock file; see ../analysis/bootloader.md):
  0x000-0x3FF  image bytes 0x000-0x3FF, plain (vectors; model tag "RT-950      " at 0x3E0)
  0x400-0x7FF  key block: 16-byte key, then filler the bootloader ignores. Not flashed.
  0x800-       image bytes 0x400 on, XORed with the expanded key at (file offset % 128),
               except bytes equal to 0x00, 0xFF, the key byte or its complement, and except the
               last 1 KB block of the file, which is left plain.
The last-block rule matches all three parts of Radtel's chain: their updater announces
(blocks - 1), the bootloader decrypts only below (announced - 1) KB of flash, and every stock file
has a plain last block. (Decrypting that block too, as earlier tools and the published
"decrypted" images do, scrambles the end of the image.)
The expanded key is 8 rows of 16: row r is the key with bytes 0-7 rotated left r bits and
bytes 8-15 rotated right r bits. The bootloader flashes file block n (n != 1) to
0x08003000 + max(n - 1, 0) * 0x400 and checks nothing else about the image.

Our images use an all-zero key (the expanded key is then all zero, so encryption is a no-op) and are
padded with 0xFF to whole 1 KB blocks, so they flash the same with Radtel's updater, Hertzz58's script
and tools/flash.py.
"""
import argparse
import os
import struct
import sys

APP_BASE = 0x08003000
APP_MAX = 948 * 1024            # our limit: stop below the stock marker sector at 0x080F0000
BOOT_MAX = 0xFD000              # the bootloader's own limit, for reading stock files
MODEL_OFF = 0x3E0
MODEL_TAG = b"RT-950      "     # 12 bytes; the bootloader compares exactly these
# The bootloader also compares the tag at image offset 0x7E0 (0x080037E0) at every power-on and shows
# "Firmware Error!" instead of starting the app when it differs (0x08000654; found on the radio Oct 11, 2026).
MODEL_OFF_BOOT = 0x7E0
KEY_OFF, BLOCK = 0x400, 0x400
ENC_START = 0x800


def _rot(v, n, left):
    n %= 8
    return (((v << n) | (v >> (8 - n))) if left else ((v >> n) | (v << (8 - n)))) & 0xFF if n else v


def expand(key):
    if len(key) != 16:
        raise ValueError("key must be 16 bytes")
    return bytes(_rot(key[i], r, i < 8) for r in range(8) for i in range(16))


def crypt(buf, key):
    """Encrypt or decrypt (the same operation) a whole BTF file; returns a copy."""
    ks = expand(key)
    out = bytearray(buf)
    for i in range(ENC_START, (-(-len(out) // BLOCK) - 1) * BLOCK):   # the last block stays plain
        k, p = ks[i % 128], out[i]
        if p not in (0x00, 0xFF, k, k ^ 0xFF):
            out[i] = p ^ k
    return bytes(out)


def check_image(img, limit=APP_MAX):
    """Problems that would stop the bootloader starting the image, or hurt the radio. Empty = fine."""
    errs = []
    if len(img) < MODEL_OFF_BOOT + len(MODEL_TAG):
        return [f"image is only {len(img)} bytes"]
    sp, reset = struct.unpack_from("<II", img, 0)
    if sp & 0x2FFE0000 != 0x20000000:
        errs.append(f"initial SP {sp:#010x} fails the bootloader's check (must be 0x2000xxxx-0x2001xxxx)")
    if not reset & 1 or not APP_BASE <= reset & ~1 < APP_BASE + len(img):
        errs.append(f"reset vector {reset:#010x} is not a Thumb address inside the image")
    if img[MODEL_OFF:MODEL_OFF + len(MODEL_TAG)] != MODEL_TAG:
        errs.append(f"no model tag {MODEL_TAG!r} at {MODEL_OFF:#x} (the stock updater sends it to the bootloader)")
    if img[MODEL_OFF_BOOT:MODEL_OFF_BOOT + len(MODEL_TAG)] != MODEL_TAG:
        errs.append(f"no model tag {MODEL_TAG!r} at {MODEL_OFF_BOOT:#x} (the bootloader checks it at power-on: "
                    "\"Firmware Error!\")")
    if len(img) > limit:
        errs.append(f"image is {len(img)} bytes; over {limit} would reach the stock marker sector at 0x080F0000")
    return errs


def pack(img, key=None, keyblock=None, pad=True):
    """Image -> BTF. Default: zero key and zero filler (see the module notes), padded with 0xFF to
    whole 1 KB blocks. Pass keyblock (and pad=False) to rebuild a stock file exactly."""
    if keyblock is None:
        keyblock = bytes(key or bytes(16)) + bytes(BLOCK - 16)
    if len(keyblock) != BLOCK:
        raise ValueError("key block must be 1024 bytes")
    raw = img[:KEY_OFF] + keyblock + img[KEY_OFF:]
    return crypt(raw + b"\xFF" * (-len(raw) % BLOCK) if pad else raw, keyblock[:16])


def unpack(btf):
    """BTF -> (image, key block). Trailing 0xFF block padding is part of the returned image."""
    if len(btf) < ENC_START:
        raise ValueError("file too short to be a BTF")
    keyblock = btf[KEY_OFF:KEY_OFF + BLOCK]
    plain = crypt(btf, keyblock[:16])
    return plain[:KEY_OFF] + plain[ENC_START:], keyblock


def describe(img):
    sp, reset = struct.unpack_from("<II", img, 0)
    tag = img[MODEL_OFF:MODEL_OFF + 12]
    return (f"{len(img)} bytes ({len(img) / 1024:.1f} KB), SP {sp:#010x}, reset {reset:#010x}, "
            f"tag {tag!r}, crc32 {__import__('zlib').crc32(img):08X}")


def selftest(stock_dir=None):
    fails = 0
    # An arbitrary key (each stock file carries its own in its key block at 0x400): row r of the expansion is
    # the key with bytes 0-7 rotated left r bits and bytes 8-15 rotated right r bits.
    k = bytes.fromhex("0123456789abcdeffedcba9876543210")
    rol = lambda b, r: ((b << r) | (b >> (8 - r))) & 0xFF
    row1 = bytes(rol(b, 1) for b in k[:8]) + bytes(rol(b, 7) for b in k[8:])
    if expand(k)[16:32] != row1:
        print("FAIL key expansion"); fails += 1
    img = bytearray(os.urandom(5000))
    img[0:8] = struct.pack("<II", 0x20018000, APP_BASE + 0x201)
    img[MODEL_OFF:MODEL_OFF + 12] = MODEL_TAG
    img[MODEL_OFF_BOOT:MODEL_OFF_BOOT + 12] = MODEL_TAG
    img = bytes(img)
    btf = pack(img)
    back, _ = unpack(btf)
    pad = -(len(img) + BLOCK) % BLOCK
    if (back != img + b"\xFF" * pad or len(btf) % BLOCK or btf[KEY_OFF:ENC_START] != bytes(BLOCK)
            or pack(img) != btf or check_image(img)):
        print("FAIL synthetic round trip"); fails += 1
    keyed = pack(img, key=k, pad=False)
    if unpack(keyed)[0] != img or keyed[ENC_START:] == btf[ENC_START:len(keyed)]:
        print("FAIL keyed round trip"); fails += 1
    if crypt(crypt(btf, k), k) != btf:
        print("FAIL crypt is not its own inverse"); fails += 1
    if not check_image(img[:0x100]) or not check_image(b"\x00" * 0x400):
        print("FAIL bad images not rejected"); fails += 1
    for root, _, files in os.walk(stock_dir) if stock_dir else []:
        for f in sorted(files):
            if not f.upper().endswith(".BTF"):
                continue
            btf = open(os.path.join(root, f), "rb").read()
            img, kb = unpack(btf)
            ok = pack(img, keyblock=kb, pad=False) == btf and not check_image(img, BOOT_MAX)
            dec = [x for x in files if x.endswith("_decrypted.bin")]
            if dec:   # published decrypted files keep the key block, and wrongly decrypt the last block
                last = (-(-len(btf) // BLOCK) - 1) * BLOCK
                ok &= crypt(btf, kb[:16])[:last] == open(os.path.join(root, dec[0]), "rb").read()[:last]
            # the plain last block continues the data before it; decrypted it would be noise
            tail = btf[(-(-len(btf) // BLOCK) - 1) * BLOCK:]
            ok &= tail.count(0x1A) > len(tail) // 16
            print(f"{'ok  ' if ok else 'FAIL'} {f}: {describe(img)}")
            fails += not ok
    print("btf selftest:", "all passed" if not fails else f"{fails} failure(s)")
    return fails == 0


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("pack"); p.add_argument("bin"); p.add_argument("btf")
    p.add_argument("--key", help="16-byte key as 32 hex digits (default: all zero, i.e. not encrypted)")
    p = sub.add_parser("unpack"); p.add_argument("btf"); p.add_argument("bin")
    p = sub.add_parser("info"); p.add_argument("file")
    p = sub.add_parser("selftest"); p.add_argument("--stock", help="folder of stock .BTF files to round-trip")
    p = sub.add_parser("lastblock", help="what the last block of a stock file looks like in MCU flash, "
                       "decrypted or not (compare with the hwtest console: mdump <addr> 32)")
    p.add_argument("btf")
    a = ap.parse_args()

    if a.cmd == "pack":
        img = open(a.bin, "rb").read()
        errs = check_image(img)
        if errs:
            sys.exit("refusing to pack:\n  " + "\n  ".join(errs))
        btf = pack(img, key=bytes.fromhex(a.key) if a.key else None)
        open(a.btf, "wb").write(btf)
        print(f"{a.btf}: {len(btf)} bytes, {-(-len(btf) // BLOCK)} blocks; image {describe(img)}")
    elif a.cmd == "unpack":
        img, kb = unpack(open(a.btf, "rb").read())
        open(a.bin, "wb").write(img)
        print(f"{a.bin}: {describe(img)}; key {kb[:16].hex()}")
    elif a.cmd == "info":
        data = open(a.file, "rb").read()
        img = unpack(data)[0] if a.file.upper().endswith(".BTF") else data
        print(describe(img))
        for e in check_image(img):
            print("  problem:", e)
    elif a.cmd == "lastblock":
        data = open(a.btf, "rb").read()
        img, kb = unpack(data)
        nblk = -(-len(data) // BLOCK)
        off = (nblk - 2) * BLOCK                  # flash offset of the last file block
        wrong = crypt(data + bytes(BLOCK), kb[:16])[off + BLOCK:off + BLOCK + 32]
        print(f"last block of {nblk} lands at {APP_BASE + off:#010x} ({len(data) - (nblk - 1) * BLOCK} bytes)")
        print(f"  console:               mdump {APP_BASE + off:#x} 32")
        print(f"  expected (plain):      {img[off:off + 32].hex().upper()}")
        print(f"  if it were decrypted:  {wrong.hex().upper()}")
    else:
        sys.exit(0 if selftest(a.stock) else 1)


if __name__ == "__main__":
    main()
