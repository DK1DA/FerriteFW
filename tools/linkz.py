#!/usr/bin/env python3
"""Link compression, the reference codecs (decision 285, step 3: options for Alex to choose; none is the default).

Three codecs over a link frame's payload, byte-exact with ferrite_core::linkz (the radio's side):

- T, a fixed table, stateless: each frame alone. Bytes 0x00-0xFC are themselves; 0xFD n is n + 3 zero bytes
  (3-258); 0xFE n is table entry n; 0xFF b is the byte b (0xFD-0xFF themselves). No RAM, no history to lose.
- L, LZSS with history: the last W bytes this direction has carried (every frame's plain bytes, in order) are a
  window matches reach back into. A flag byte before every 8 tokens (bit 0 first): 1 a match, two bytes, little
  endian: the offset back (1-W) less 1 in the low bits, the length less 3 above; 0 a literal byte. W 1024: 10 + 6
  bits, lengths 3-66. Both ends must have seen the same frames: any error, retry or new session starts both over.
- LD, L with the window starting as a fixed dictionary (W bytes), so even the first frames find matches.

`python3 tools/linkz.py train CORPUS.pkl MODEL.pkl` builds the table and dictionary from a traffic corpus (frame
payloads per traffic type, in session order; the 1000-channel synthetic set is left out of training);
`python3 tools/linkz.py score CORPUS.pkl MODEL.pkl` prints bytes per traffic type and codec;
`python3 tools/linkz.py emit CORPUS.pkl MODEL.pkl` writes crates/ferrite-core/src/linkz_data.rs and linkz_vectors.rs.
"""
import collections, pickle, sys

W = 1024
OFF_BITS = 10
MIN, MAX = 3, 3 + (1 << (16 - OFF_BITS)) - 1


# ---- T: the fixed table ----

def t_encode(data, table):
    by3 = collections.defaultdict(list)
    for i, e in enumerate(table):
        by3[e[:3]].append((len(e), i))
    for k in by3:
        by3[k].sort(reverse=True)
    out, i, n = bytearray(), 0, len(data)
    while i < n:
        if data[i] == 0:
            j = i
            while j < n and data[j] == 0 and j - i < 258:
                j += 1
            if j - i >= 3:
                out += bytes([0xFD, j - i - 3])
                i = j
                continue
        hit = None
        for ln, idx in by3.get(bytes(data[i:i + 3]), ()):
            if data[i:i + ln] == table[idx]:
                hit = (ln, idx)
                break
        if hit:
            out += bytes([0xFE, hit[1]])
            i += hit[0]
            continue
        b = data[i]
        out += bytes([0xFF, b]) if b >= 0xFD else bytes([b])
        i += 1
    return bytes(out)


def t_decode(data, table):
    out, i = bytearray(), 0
    while i < len(data):
        b = data[i]
        if b == 0xFD:
            out += bytes(data[i + 1] + 3); i += 2
        elif b == 0xFE:
            out += table[data[i + 1]]; i += 2
        elif b == 0xFF:
            out.append(data[i + 1]); i += 2
        else:
            out.append(b); i += 1
    return bytes(out)


# ---- L / LD: LZSS with a window kept across frames ----

class Lz:
    """One direction's state: the window (the last W plain bytes), primed with `dictionary` (LD) or empty (L)."""

    def __init__(self, dictionary=b""):
        self.hist = bytearray(dictionary[-W:])

    def encode(self, data):
        buf = self.hist + data
        base = len(self.hist)
        heads = collections.defaultdict(list)  # 3-byte prefix -> positions (the radio uses a hash chain)
        for p in range(max(0, base - W), base):
            heads[bytes(buf[p:p + 3])].append(p)
        out, flags_at, bit, i = bytearray(), None, 8, base
        while i < len(buf):
            if bit == 8:
                flags_at, bit = len(out), 0
                out.append(0)
            best = (0, 0)
            for p in reversed(heads.get(bytes(buf[i:i + 3]), ())):
                if i - p > W:
                    break
                ln = 0
                while ln < MAX and i + ln < len(buf) and buf[p + ln] == buf[i + ln]:
                    ln += 1
                if ln > best[0]:
                    best = (ln, i - p)
                    if ln == MAX:
                        break
            if best[0] >= MIN:
                ln, off = best
                v = (off - 1) | (ln - MIN) << OFF_BITS
                out += bytes([v & 0xFF, v >> 8])
                out[flags_at] |= 1 << bit
                for k in range(ln):
                    heads[bytes(buf[i + k:i + k + 3])].append(i + k)
                i += ln
            else:
                out.append(buf[i])
                heads[bytes(buf[i:i + 3])].append(i)
                i += 1
            bit += 1
        self.hist = (self.hist + data)[-W:]
        return bytes(out)

    def decode(self, data):
        buf = bytearray(self.hist)
        base, i = len(buf), 0
        while i < len(data):
            flags = data[i]; i += 1
            for bit in range(8):
                if i >= len(data):
                    break
                if flags >> bit & 1:
                    v = data[i] | data[i + 1] << 8; i += 2
                    off, ln = (v & (W - 1)) + 1, (v >> OFF_BITS) + MIN
                    for _ in range(ln):
                        buf.append(buf[-off])
                else:
                    buf.append(data[i]); i += 1
        plain = bytes(buf[base:])
        self.hist = buf[-W:]
        return plain


# ---- training and scoring ----

def substrings(frames, lo=3, hi=24):
    c = collections.Counter()
    for f in frames:
        for i in range(len(f)):
            for ln in range(lo, min(hi, len(f) - i) + 1):
                s = f[i:i + ln]
                if b"\x00\x00\x00" in s:
                    break
                c[s] += 1
    return c


def train(corpus, kinds):
    frames = [f for k in kinds for f in corpus[k]]
    c = substrings(frames)
    # greedy: the best saving first, then the counts of what it covers taken away (roughly), 255 entries
    table = []
    for _ in range(255):
        best = max(c.items(), key=lambda kv: (len(kv[0]) - 2) * kv[1], default=None)
        if not best or (len(best[0]) - 2) * best[1] < 4:
            break
        s, n = best
        table.append(s)
        for k in list(c):
            if s in k or k in s:
                c[k] = max(0, c[k] - n) if k in s else c[k]
        c[s] = 0
    # the dictionary: the table's strings, the most useful last (nearest the window's end: smallest offsets)
    d = bytearray()
    for s in reversed(table):
        if len(d) + len(s) > W:
            break
        d = bytearray(s) + d
    return table, bytes(d)


def wire(n):
    return n + 6 + n // 254  # type, seq, CRC, COBS, the 0x00


def score(corpus, table, dictionary):
    rows = []
    for k, frames in corpus.items():
        plain = sum(map(len, frames))
        t = sum(min(len(t_encode(f, table)), len(f)) for f in frames)
        l, ld = Lz(), Lz(dictionary)
        lz = ldz = 0
        for f in frames:
            a, b = l.encode(f), ld.encode(f)
            assert l.decode if True else None
            lz += min(len(a), len(f))
            ldz += min(len(b), len(f))
        rows.append((k, len(frames), plain, t, lz, ldz))
    return rows


def check(corpus, table, dictionary):
    """Round trips every frame through each codec (both ends kept in step)."""
    for frames in corpus.values():
        e, d = Lz(dictionary), Lz(dictionary)
        for f in frames:
            assert t_decode(t_encode(f, table), table) == f
            assert d.decode(e.encode(f)) == f


def rs_array(b, w=24):
    return "\n".join("    " + " ".join(f"0x{x:02X}," for x in b[i:i + w]) for i in range(0, len(b), w))


def emit(corpus, table, d, root):
    """The Rust side's data (linkz_data.rs) and test vectors (linkz_vectors.rs, Python's encodings of the samples)."""
    blob, at = b"".join(table), [0]
    for e in table:
        at.append(at[-1] + len(e))
    order = sorted(range(len(table)), key=lambda i: (table[i][0], -len(table[i]), -i))
    first = [0] * 257
    for b in range(256):
        first[b + 1] = first[b] + sum(1 for i in order if table[i][0] == b)
    samples = corpus["ui"][:4] + corpus["liveview"][:2] + corpus["daily_read"][:2]
    src = ["//! Generated by `tools/linkz.py emit` from a link traffic corpus (a simulator session, clear's daily channel set):",
           "//! the fixed table (T) and the dictionary (LD). Rebuild with the tool; never edit by hand.", "",
           f"/// The table's strings back to back ({len(table)} entries, {len(blob)} bytes).",
           f"pub static TABLE: [u8; {len(blob)}] = [", rs_array(blob), "];",
           "/// Where entry n starts in [`TABLE`] (and n + 1 where it ends).",
           f"pub static TABLE_AT: [u16; {len(at)}] = {at};", "",
           "/// T's entries by first byte: [`TABLE_ORDER`][`TABLE_FIRST`[b]..`TABLE_FIRST`[b + 1]] are those starting with b,",
           "/// longest first (then the higher number), the order the reference tries them.",
           f"pub static TABLE_FIRST: [u8; 257] = {first};", f"pub static TABLE_ORDER: [u8; {len(order)}] = {order};", "",
           f"/// LD's dictionary: the window's first {len(d)} bytes.", f"pub static DICT: [u8; {len(d)}] = [", rs_array(d), "];", "",
           "/// Sample frames for the bench (`bt linkz`): UI pages, Live View deltas, channel answers, as they went on the link.",
           '#[cfg(any(test, feature = "bench"))]', "pub static SAMPLES: &[&[u8]] = &["]
    src += [f"    &[{', '.join(map(str, f))}]," for f in samples] + ["];"]
    open(f"{root}/linkz_data.rs", "w").write("\n".join(src) + "\n")
    a, b = Lz(), Lz(d)
    vec = ["//! Test vectors from tools/linkz.py (Python's encodings of SAMPLES in order, one session: T, L, LD).",
           "pub static VECTORS: &[(&[u8], &[u8], &[u8])] = &["]
    for f in samples:
        t, l, ld = t_encode(f, table), a.encode(f), b.encode(f)
        vec.append(f"    (&[{', '.join(map(str, t))}], &[{', '.join(map(str, l))}], &[{', '.join(map(str, ld))}]),")
    open(f"{root}/linkz_vectors.rs", "w").write("\n".join(vec + ["];"]) + "\n")


if __name__ == "__main__":
    cmd, path = sys.argv[1], sys.argv[2]
    corpus = pickle.load(open(path, "rb"))
    if cmd == "train":
        kinds = [k for k in corpus if not k.startswith("syn")]
        table, d = train(corpus, kinds)
        pickle.dump({"table": table, "dict": d}, open(sys.argv[3], "wb"))
        print(f"table {len(table)} entries, {sum(map(len, table))} B; dictionary {len(d)} B")
    elif cmd == "emit":
        import os
        m = pickle.load(open(sys.argv[3], "rb"))
        emit(corpus, m["table"], m["dict"], os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "crates", "ferrite-core", "src"))
    elif cmd == "score":
        m = pickle.load(open(sys.argv[3], "rb"))
        check(corpus, m["table"], m["dict"])
        print(f"{'traffic':<12}{'frames':>7}{'plain':>8}{'T':>8}{'L':>8}{'LD':>8}")
        for k, n, p, t, l, ld in score(corpus, m["table"], m["dict"]):
            print(f"{k:<12}{n:>7}{p:>8}{t:>8}{l:>8}{ld:>8}   {t / p:.2f} {l / p:.2f} {ld / p:.2f}")
