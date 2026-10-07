#!/usr/bin/env python3
"""Build the predictive-text dictionary (T9) for the SPI flash from assets/words/.

    python3 tools/mkwords.py --scowl PATH [--big] [--min-count 2] [--out target/words]

Ranking, best first:
  1. core.txt, in its order (the words people use most in short messages);
  2. ham.txt, in its order;
  3. aprs_counts.txt (word<TAB>count from real APRS messages, tools/aprs_corpus.py), by count: only words seen
     at least --min-count times that are ordinary English (SCOWL level 50 or below) or on our own lists, which
     keeps software banners out ("uptime", "python");
  4. with --big only: SCOWL's most common band (level 35), shorter words first (about 39,000 words, 172 KB).
--scowl points at SCOWL's data/scowl-pre.txt (github.com/en-wl/wordlist, MIT-like licence; its notice ships as
words-licence.txt). The default build is the small one: our lists plus what people actually type.

Words are letters (and an apostrophe) only: digits are typed as digits (hold a key), so "73" or "2m" are quick
messages and short forms, not dictionary entries.

Output:
  <out>.txt  the ranked list (word, key sequence, rank), for review;
  <out>.bin  the packed dictionary (format below);
  a report: words, bytes, and how often each core/ham word is the first candidate for its keys.

Packed format (little-endian), read a bucket at a time from SPI flash:
  0   "FT9D", version u8, reserved u8, word count u16
  8   bucket table: for each two-digit prefix 22..99 (64 buckets, first-key-major), u32 offset, u16 entries;
      single-letter words live in bucket "d0" entries 64..71 (keys 2-9)
  ... entries, sorted by key sequence, then rank: prefix length shared with the previous entry's letters (u4),
      suffix length (u4), suffix letters 5 bits each (a=1..z=26, '=27), padded to a byte, then rank u8
      (255 best).
"""
import argparse
import json
import os
import re
import struct
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
WORDS = os.path.join(HERE, "..", "assets", "words")
KEYS = {c: k for k, cs in {"2": "abc", "3": "def", "4": "ghi", "5": "jkl", "6": "mno", "7": "pqrs", "8": "tuv", "9": "wxyz"}.items() for c in cs}
KEYS["'"] = "1"


def plain(path):
    out = []
    for l in open(path, encoding="utf-8"):
        l = l.split("#", 1)[0].strip()
        if l:
            out.append(l)
    return out


def ok(w):
    return re.fullmatch(r"[a-z]+(?:'[a-z]+)?", w) is not None and len(w) <= 15


def keyseq(w):
    return "".join(KEYS[c] for c in w)


def scowl_levels(path):
    """Each word's most common SCOWL level."""
    lev = {}
    for line in open(path, encoding="utf-8"):
        if not line.strip() or line.startswith("#"):
            continue
        m = re.match(r"^(.*?):\s+(\S+)(?:\s+<[^>]*>)?(?:\s+\([^)]*\))?(?::\s*(.*))?$", line.rstrip("\n"))
        if not m:
            continue
        levs = [int(x) for x in re.findall(r"(?:^|\s)(\d+)(?=\s|$)", m.group(1))]
        if not levs:
            continue
        forms = [m.group(2)] + ([w.strip().split()[0] for w in re.split(r"[,|]", m.group(3)) if w.strip()] if m.group(3) else [])
        for f in forms:
            if re.fullmatch(r"[a-z]+", f):
                lev[f] = min(lev.get(f, 99), min(levs))
    return lev


def ranked(scowl_path, big, min_count):
    rank = {}                                   # word -> score (higher better)
    def add(w, score):
        w = w.lower()
        if ok(w) and score > rank.get(w, -1):
            rank[w] = score
    core = plain(os.path.join(WORDS, "core.txt"))
    for i, w in enumerate(core):
        add(w, 4_000_000 - i)
    for i, w in enumerate(plain(os.path.join(WORDS, "ham.txt"))):
        add(w, 3_000_000 - i)
    lev = scowl_levels(scowl_path)
    counts = os.path.join(WORDS, "aprs_counts.txt")
    if os.path.exists(counts):
        for l in plain(counts):
            w, c = l.split("\t")
            if int(c) >= min_count and len(w) > 1 and (lev.get(w, 99) <= 50 or w in rank):
                add(w, 2_000_000 + int(c))
    if big:
        for w, l in lev.items():
            if l <= 35:
                add(w, 1_000_000 - len(w) * 1000)
    order = sorted(rank, key=lambda w: -rank[w])
    return order, core


def pack(order):
    """The packed dictionary and the 0-255 rank given to each word."""
    n = len(order)
    r8 = {w: max(1, 255 - (i * 254) // max(n - 1, 1)) for i, w in enumerate(order)}
    entries = sorted(order, key=lambda w: (keyseq(w), -r8[w], w))
    buckets = {}
    for w in entries:
        k = keyseq(w)
        b = (int(k[0]) - 2) * 8 + (int(k[1]) - 2) if len(k) > 1 and k[1] != "1" and k[0] != "1" else 64 + max(int(k[0]) - 2, 0)
        buckets.setdefault(b, []).append(w)
    body = bytearray()
    table = []
    for b in range(72):
        ws = buckets.get(b, [])
        table.append((len(body), len(ws)))
        prev = ""
        for w in ws:
            p = 0
            while p < min(len(prev), len(w), 15) and prev[p] == w[p]:
                p += 1
            suf = w[p:]
            body.append(p << 4 | len(suf))
            bits, nb, out = 0, 0, bytearray()
            for c in suf:
                bits = bits << 5 | (27 if c == "'" else ord(c) - 96)
                nb += 5
                while nb >= 8:
                    nb -= 8
                    out.append(bits >> nb & 0xFF)
            if nb:
                out.append(bits << (8 - nb) & 0xFF)
            body += out
            body.append(r8[w])
            prev = w
    head = b"FT9D" + struct.pack("<BBH", 1, 0, n)
    tab = b"".join(struct.pack("<IH", off, cnt) for off, cnt in table)
    return head + tab + bytes(body), r8


def first_rate(words, order, r8):
    """How many of `words` come first among the dictionary words that share their keys."""
    by = {}
    for w in order:
        by.setdefault(keyseq(w), []).append(w)
    first = sum(1 for w in words if ok(w) and max(by[keyseq(w)], key=lambda x: r8[x]) == w)
    clash = [(w, max(by[keyseq(w)], key=lambda x: r8[x])) for w in words if ok(w) and max(by[keyseq(w)], key=lambda x: r8[x]) != w]
    return first, sum(1 for w in words if ok(w)), clash


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--scowl", required=True, help="SCOWL data/scowl-pre.txt (the English check; with --big, the last tier)")
    ap.add_argument("--big", action="store_true", help="add SCOWL's common band, about 39,000 words")
    ap.add_argument("--min-count", type=int, default=2, help="times a word must be seen in real messages")
    ap.add_argument("--out", default=os.path.join(HERE, "..", "target", "words"))
    a = ap.parse_args()
    order, core = ranked(a.scowl, a.big, a.min_count)
    blob, r8 = pack(order)
    os.makedirs(os.path.dirname(a.out), exist_ok=True)
    open(a.out + ".bin", "wb").write(blob)
    with open(a.out + ".txt", "w") as f:
        for w in order:
            f.write(f"{w}\t{keyseq(w)}\t{r8[w]}\n")
    ham = plain(os.path.join(WORDS, "ham.txt"))
    f1, n1, c1 = first_rate(core, order, r8)
    f2, n2, c2 = first_rate(ham, order, r8)
    print(f"{len(order)} words, {len(blob)} bytes packed ({len(blob) / 1024:.1f} KB, {len(blob) / max(len(order), 1):.1f} B/word)")
    print(f"core words first for their keys: {f1}/{n1}; ham words: {f2}/{n2}")
    if c1:
        print("core words behind another word with the same keys:", ", ".join(f"{w} (after {o})" for w, o in c1[:30]))


if __name__ == "__main__":
    main()
