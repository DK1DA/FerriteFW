#!/usr/bin/env python3
"""Build the predictive-text dictionary (T9) for the firmware image from assets/words/.

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

Words are letters only. A word written in capitals in a list (QSL, TNX, APRS) is a Q-code, shorthand or
acronym: it is shown and sent in capitals (manual TEXT-4).

Packed format v2 (little-endian), linked into the firmware's slow flash and read in place (manual TEXT-8):
  0   "FT9D", version u8 (2), reserved u8, word count u16
  8   short-form section offset u32, quick-message section offset u32 (from the start of the blob)
  16  bucket table: 72 entries of u32 offset (from the start of the blob) and u16 count. Buckets 0-63 are the
      words of two or more letters by their first two keys, (k1 - 2) * 8 + (k2 - 2); 64-71 the one-letter words
      by their key, 64 + k1 - 2
  ... entries, sorted by key sequence, then rank (best first): the letters shared with the previous entry of the
      bucket (u4, high) and the number of new letters (u4, low), the new letters 5 bits each (a=1..z=26), padded
      to a byte, then a byte: bit 7 capitals, bits 6-0 the rank (127 best)
  short forms: u8 count, then word NUL short-form NUL, one word each (assets/words/abbrev.txt; phrases of more
      than one word are skipped)
  quick messages: u8 count, then each message NUL-terminated (assets/words/quick.txt, the factory set)

The committed copy is assets/words/dict.bin (and dict.txt, the ranked list, for review):
    python3 tools/mkwords.py --scowl PATH --out assets/words/dict
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
    return re.fullmatch(r"[a-z]+", w) is not None and len(w) <= 15


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
    caps = {}                                   # word -> shown in capitals (the best-ranked listing decides)
    def add(w, score):
        c, w = w.isupper(), w.lower()
        if ok(w) and score > rank.get(w, -1):
            rank[w] = score
            caps[w] = c
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
    return order, core, caps


def letters(w):
    """5 bits a letter (a=1..z=26), padded to a byte."""
    bits, nb, out = 0, 0, bytearray()
    for c in w:
        bits = bits << 5 | (ord(c) - 96)
        nb += 5
        while nb >= 8:
            nb -= 8
            out.append(bits >> nb & 0xFF)
    if nb:
        out.append(bits << (8 - nb) & 0xFF)
    return out


def bucket(k):
    return (int(k[0]) - 2) * 8 + (int(k[1]) - 2) if len(k) > 1 else 64 + int(k[0]) - 2


def pack(order, caps):
    """The packed dictionary and the 1-127 rank given to each word."""
    n = len(order)
    r7 = {w: max(1, 127 - (i * 126) // max(n - 1, 1)) for i, w in enumerate(order)}
    pos = {w: i for i, w in enumerate(order)}
    entries = sorted(order, key=lambda w: (keyseq(w), pos[w]))
    buckets = {}
    for w in entries:
        buckets.setdefault(bucket(keyseq(w)), []).append(w)
    head_len, table_len = 16, 72 * 6
    body = bytearray()
    table = []
    for b in range(72):
        ws = buckets.get(b, [])
        table.append((head_len + table_len + len(body), len(ws)))
        prev = ""
        for w in ws:
            p = 0
            while p < min(len(prev), len(w), 15) and prev[p] == w[p]:
                p += 1
            suf = w[p:]
            body.append(p << 4 | len(suf))
            body += letters(suf)
            body.append((0x80 if caps.get(w) else 0) | r7[w])
            prev = w
    shorts = []
    for l in plain(os.path.join(WORDS, "abbrev.txt")):
        word, short = l.split("\t")
        if " " not in word:
            shorts.append((word, short))
    sec_short = bytearray([len(shorts)])
    for word, short in shorts:
        sec_short += word.encode() + b"\0" + short.upper().encode() + b"\0"
    quick = plain(os.path.join(WORDS, "quick.txt"))
    sec_quick = bytearray([len(quick)])
    for q in quick:
        sec_quick += q.encode() + b"\0"
    short_off = head_len + table_len + len(body)
    quick_off = short_off + len(sec_short)
    head = b"FT9D" + struct.pack("<BBHII", 2, 0, n, short_off, quick_off)
    tab = b"".join(struct.pack("<IH", off, cnt) for off, cnt in table)
    return head + tab + bytes(body) + bytes(sec_short) + bytes(sec_quick), r7


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
    order, core, caps = ranked(a.scowl, a.big, a.min_count)
    blob, r8 = pack(order, caps)
    os.makedirs(os.path.dirname(a.out), exist_ok=True)
    open(a.out + ".bin", "wb").write(blob)
    with open(a.out + ".txt", "w") as f:
        for w in order:
            f.write(f"{w.upper() if caps.get(w) else w}\t{keyseq(w)}\t{r8[w]}\n")
    ham = [w.lower() for w in plain(os.path.join(WORDS, "ham.txt"))]
    core = [w.lower() for w in core]
    f1, n1, c1 = first_rate(core, order, r8)
    f2, n2, c2 = first_rate(ham, order, r8)
    print(f"{len(order)} words, {len(blob)} bytes packed ({len(blob) / 1024:.1f} KB, {len(blob) / max(len(order), 1):.1f} B/word)")
    print(f"core words first for their keys: {f1}/{n1}; ham words: {f2}/{n2}")
    if c1:
        print("core words behind another word with the same keys:", ", ".join(f"{w} (after {o})" for w, o in c1[:30]))


if __name__ == "__main__":
    main()
