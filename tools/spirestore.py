"""Sectors of the SPI flash put back from a whole-chip backup over the cable (DATA_READ's CRC to compare, then
DATA_WRITE's erase and program, each sector's CRC read back): only the sectors that differ.

  spirestore.py PORT BACKUP.bin START END       (hex addresses, 4 KB aligned)

Ferrite routes writes inside the SSTV picture range through its own store (no update state); elsewhere a raw write
puts the radio in update mode until it restarts (LINK-61). Never calibration or the update area (the link refuses)."""
import os, sys, zlib
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from ferrite_link import DATA_READ, DATA_WRITE, Link, Port, STATUS  # noqa: E402
import struct, time  # noqa: E402

SECTOR = 4096
BUSY = STATUS.index("busy")


def ask(link, ty, payload):
    """One request, again while the radio says busy (its store still writing the last ones)."""
    for _ in range(2000):
        s, rest = link.ask(ty, payload)
        if s == 0:
            return rest
        if s != BUSY:
            sys.exit(f"0x{ty:02X}: {STATUS[s] if s < len(STATUS) else s}")
        time.sleep(0.02)
    sys.exit("the radio stayed busy")


def crc(link, a):
    return struct.unpack("<I", ask(link, DATA_READ, struct.pack("<BII", 1, a, SECTOR)))[0]


def main():
    port, path, a0, a1 = sys.argv[1], sys.argv[2], int(sys.argv[3], 16), int(sys.argv[4], 16)
    assert a0 % SECTOR == 0 and a1 % SECTOR == 0 and a0 < a1
    b = open(path, "rb").read()
    link = Link(Port(port))
    link.hello(b"spirestore.py")
    fixed = same = 0
    for s in range(a0, a1, SECTOR):
        want = b[s:s + SECTOR]
        if crc(link, s) == zlib.crc32(want):
            same += 1
            continue
        ask(link, DATA_WRITE, struct.pack("<BI", 0, s))
        for o in range(0, SECTOR, 64):
            chunk = want[o:o + 64]
            if chunk != b"\xff" * 64:
                ask(link, DATA_WRITE, struct.pack("<BI", 1, s + o) + chunk)
        got = crc(link, s)
        if got != zlib.crc32(want):
            sys.exit(f"{s:#08x}: CRC {got:#010x} after writing, want {zlib.crc32(want):#010x}")
        fixed += 1
        print(f"{s:#08x} restored", flush=True)
    print(f"{fixed} sectors restored, {same} already right")


main()
