#!/usr/bin/env python3
"""Compare a BK4829 register dump from the radio with the reset defaults in Beken's "BK4829 Registers Table".

    python3 tools/bkcheck.py bk0.txt [bk1.txt]
    python3 tools/bkcheck.py --diff before.txt after.txt

Reads both dump formats: hwtest's `bk 0` (two read alignments side by side) and Ferrite's terminal `bk 0 dump`
(lines "bk0 30: BFF1 ...", eight registers each, already in the right alignment). A Ferrite dump is taken after
Ferrite's own start-up, so its differences from the reset defaults are mostly our writes; --diff compares two
dumps register by register (before and after a change, stock-like and ours, chip 0 and chip 1).

Capture the dump on the hwtest console right after power-on, before anything else runs: `bk 0` and `bk 1`
(the image only reads the BK4829s, never writes them, so every register should still hold its reset value).
Save each command's output to a file. Defaults come from datasheets/md/bk4829_registers_table/all.md
(DRT01-230606-C01 V1.0, section 2). Status, interrupt and RSSI registers change on their own, so a
mismatch there means little; mismatches in configuration registers say the table is not this chip.
Both read alignments of the dump are checked; the one that matches more registers is reported.
"""
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
TABLE = os.path.join(HERE, "..", "..", "datasheets", "md", "bk4829_registers_table", "all.md")
LIVE = {0x02, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x27, 0x5F, 0x63, 0x64, 0x65, 0x67, 0x68, 0x6C, 0x6F}  # status, RSSI, FIFO


def defaults():
    text = open(TABLE, encoding="utf-8").read()
    text = text[text.index("Registers’ Default Value"):].replace("**", "")
    out = {}
    for reg, val in re.findall(r"\|REG_([0-9A-Fa-f]{2})\|([0-9A-Fa-fx]{0,4})(?=\|)", text):
        if val and "x" not in val:          # REG_00 reads "48x9": the family ID, 0x4829 on this chip
            out[int(reg, 16)] = int(val, 16)
        elif val:
            out[int(reg, 16)] = 0x4829
    # The PDF's last row converts with merged cells: "REG_1F<br>5454".
    for reg, val in re.findall(r"REG_([0-9A-Fa-f]{2})<br>([0-9A-Fa-f]{4})\b", text):
        out.setdefault(int(reg, 16), int(val, 16))
    return out


def dump(path):
    """{alignment: {reg: value}} from `bk N` console output (hwtest) or `bk N dump` (Ferrite's terminal)."""
    a, b, f = {}, {}, {}
    for line in open(path):
        m = re.match(r"\s*bk\d ([0-9A-F]{2}):((?: [0-9A-F]{4}){8})\s*$", line)
        if m:
            base = int(m.group(1), 16)
            for i, v in enumerate(m.group(2).split()):
                f[base + i] = int(v, 16)
            continue
        m = re.match(r"\s*([0-9A-F]{2}):\s+((?:[0-9A-F]{4}\s+){4})\|\s+((?:[0-9A-F]{4}\s*){4})", line)
        if m:
            base = int(m.group(1), 16)
            for i, v in enumerate(m.group(2).split()):
                a[base + i] = int(v, 16)
            for i, v in enumerate(m.group(3).split()):
                b[base + i] = int(v, 16)
    if f:
        return {"Ferrite dump": f}
    return {"datasheet timing": a, "alternative timing": b}


def diff(p1, p2):
    a = max(dump(p1).values(), key=len)
    b = max(dump(p2).values(), key=len)
    regs = sorted(set(a) | set(b))
    changed = [r for r in regs if a.get(r) != b.get(r)]
    print(f"{p1} vs {p2}: {len(changed)} of {len(regs)} registers differ")
    for r in changed:
        x = "----" if r not in a else f"{a[r]:04X}"
        y = "----" if r not in b else f"{b[r]:04X}"
        note = "  (status/RSSI, changes by itself)" if r in LIVE else ""
        print(f"  REG_{r:02X}: {x} -> {y}{note}")


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    if sys.argv[1] == "--diff":
        if len(sys.argv) != 4:
            sys.exit(__doc__)
        return diff(sys.argv[2], sys.argv[3])
    want = defaults()
    for path in sys.argv[1:]:
        best = None
        for name, regs in dump(path).items():
            cmp = [(r, want[r], regs[r]) for r in sorted(want) if r in regs]
            ok = [c for c in cmp if c[1] == c[2]]
            if best is None or len(ok) > len(best[2]):
                best = (name, cmp, ok)
        name, cmp, ok = best
        bad = [c for c in cmp if c[1] != c[2]]
        print(f"{path}: {len(ok)} of {len(cmp)} documented defaults match ({name})")
        for r, w, g in bad:
            note = "  (status/RSSI, changes by itself)" if r in LIVE else ""
            print(f"  REG_{r:02X}: table {w:04X}, chip {g:04X}{note}")


if __name__ == "__main__":
    main()
