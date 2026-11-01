#!/usr/bin/env python3
"""Ferrite Flash Map: where the firmware image's bytes go, as an interactive treemap (one HTML file).

    python3 tools/sizemap.py                          # the release ELF as last built -> target/sizemap.html
    python3 tools/sizemap.py --build tx               # run tools/mkbtf.sh tx first (also rx, tx-bench)
    python3 tools/sizemap.py -o map.html --json map.json
    python3 tools/sizemap.py --diff old.json          # biggest changes from an earlier --json to this build
    python3 tools/sizemap.py new.json --diff old.json # the same between two saved trees (no ELF needed)

The tree is region (section) > crate > file or module > function or symbol. Code is attributed by its DWARF line
rows (arm-none-eabi-addr2line -i): inlined code counts in the file it came from, except the standard library's, which
counts where it was used (`opt.map(..)` in home.rs is home.rs bytes; --innermost puts it under core). Code with no
line row falls back to its symbol. Named constants go by symbol (fonts and images by name); unnamed constants
(literal strings, const tables) are split at every address the code or data points to and go to the file that
refers to them. What is left shows as "other", so the leaves add up to the image size exactly (the check line).
The release ELF is shared by every build: the script tells which one it is by comparing it with target/*.bin.
Needs arm-none-eabi-objdump, -nm and -addr2line on the PATH (as tools/slowcheck.py does).
"""
import argparse, collections, json, os, re, struct, subprocess, sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ELF = os.path.join(ROOT, "target/thumbv7em-none-eabihf/release/ferrite")
APP_BASE, APP_SIZE = 0x08003000, 1012 * 1024  # firmware/memory.x (decision 338)
FAST_END = 0x08040000  # zero-wait flash ends at 256 KB (firmware/memory.x)
BUILDS = {"rx": ("", "ferrite.bin"), "tx": ("tx", "ferrite-tx.bin"), "tx-bench": ("tx-bench", "ferrite-txbench.bin")}
REGION = {".vector_table": "vectors & tags", ".model_tag": "vectors & tags", ".model_tag_boot": "vectors & tags",
          ".fast_gap1": "fast .text", ".fast_gap2": "fast .text", ".text": "fast .text", ".rodata": "fast .rodata",
          ".fast_ui": ".fast_ui", ".slow_rodata": "slow .rodata", ".slow_text": "slow .text", ".data": "data init"}
STD = "core (Rust std)"


def run(*a, inp=None):
    return subprocess.run(a, capture_output=True, text=True, check=True, input=inp).stdout


# ---- ELF ------------------------------------------------------------------------------------------------------------
def read_elf(path):
    b = open(path, "rb").read()
    phoff, shoff = struct.unpack_from("<II", b, 0x1C)
    phentsize, phnum, shentsize, shnum, shstrndx = struct.unpack_from("<HHHHH", b, 0x2A)
    phdrs = [struct.unpack_from("<8I", b, phoff + i * phentsize) for i in range(phnum)]
    raw = [struct.unpack_from("<10I", b, shoff + i * shentsize) for i in range(shnum)]
    stro = raw[shstrndx][4]
    secs = []
    for name, typ, flags, addr, off, size, *_ in raw:
        if typ != 1 or not flags & 2 or size == 0:  # PROGBITS, ALLOC
            continue
        nm = b[stro + name:b.index(b"\0", stro + name)].decode()
        lma = addr
        for _t, poff, _v, paddr, filesz, *_r in phdrs:
            if _t == 1 and poff <= off < poff + filesz:
                lma = paddr + off - poff
        secs.append(dict(name=nm, vma=addr, lma=lma, size=size, data=b[off:off + size], code=bool(flags & 4)))
    secs.sort(key=lambda s: s["lma"])
    base = secs[0]["lma"]
    end = max(s["lma"] + s["size"] for s in secs)
    img = bytearray(end - base)
    for s in secs:
        img[s["lma"] - base:s["lma"] - base + s["size"]] = s["data"]
    return secs, base, bytes(img)


class Image:
    def __init__(self, path):
        self.secs, self.base, self.img = read_elf(path)
        self.n = len(self.img)

    def off(self, vma):
        """Image offset of a VMA (None if not in the image)."""
        for s in self.secs:
            if s["vma"] <= vma < s["vma"] + s["size"]:
                return s["lma"] - self.base + vma - s["vma"]
        return None

    def sec_at(self, o):
        for s in self.secs:
            if s["lma"] - self.base <= o < s["lma"] - self.base + s["size"]:
                return s
        return None

    def word(self, o):
        return struct.unpack_from("<I", self.img, o)[0] if 0 <= o <= self.n - 4 else None


# ---- names ----------------------------------------------------------------------------------------------------------
def crate_dirs():
    """Package name -> (display crate, src dir) for the workspace."""
    out = {}
    for d in ["firmware"] + [f"{p}/{x}" for p in ("crates", "drivers") for x in sorted(os.listdir(os.path.join(ROOT, p)))]:
        t = os.path.join(ROOT, d, "Cargo.toml")
        if os.path.exists(t):
            m = re.search(r'^name\s*=\s*"([^"]+)"', open(t).read(), re.M)
            if m:
                out[m.group(1).replace("-", "_")] = (d, os.path.join(ROOT, d, "src"))
    return out


CRATES = crate_dirs()
UDIGI = {}  # udigimodes crate -> its src dir (from the DWARF paths; names from Cargo.lock)
try:
    for _n in re.findall(r'name = "([^"]+)"\nversion = "[^"]+"\nsource = "git\+[^"]*udigimodes', open(os.path.join(ROOT, "Cargo.lock")).read()):
        UDIGI[_n.replace("-", "_")] = None
except OSError:
    pass


def crate_display(ident):
    if ident in ("core", "alloc", "std"):
        return STD
    if ident == "ferrite":
        return "firmware"
    if ident in UDIGI:
        return "udigimodes"
    return ident.replace("_", "-")


def file_node(path):
    """(crate, file) for a DWARF source path."""
    p = path
    if p.startswith(ROOT + "/"):
        p = p[len(ROOT) + 1:]
    for ident, (d, _src) in CRATES.items():
        if p.startswith(d + "/src/"):
            return crate_display(ident), p[len(d) + 5:]
    m = re.search(r"/rustlib/src/rust/library/([^/]+)/(?:src/)?(.*)", p)
    if m:
        if m.group(1).startswith("compiler"):
            return "compiler-builtins", m.group(2)
        return STD, f"{m.group(1)}/{m.group(2)}"
    m = re.search(r"/git/checkouts/udigimodes-[^/]+/[^/]+/crates/([^/]+)/(?:src/)?(.*)", p)
    if m:
        UDIGI[m.group(1)] = p[:m.start(2)]
        return "udigimodes", f"{m.group(1)}/{m.group(2)}"
    m = re.search(r"/git/checkouts/([^/]+?)-[0-9a-f]{16}/[^/]+/(.*)", p)
    if m:
        return m.group(1), m.group(2)
    m = re.search(r"/registry/src/[^/]+/([^/]+?)-\d[^/]*/(?:src/)?(.*)", p)
    if m:
        return m.group(1), m.group(2)
    m = re.match(r"([a-z_0-9]+)\.[0-9a-f]+-cgu\.\d+$", os.path.basename(p))
    if m:
        return crate_display(m.group(1)), "(generated)"
    return "other", p


def strip_generics(n):
    out, i, depth = [], 0, 0
    while i < len(n):
        c = n[i]
        if depth:
            depth += (c == "<") - (c == ">")
        elif c == "<" and i and (n[i - 1].isalnum() or n[i - 1] in "_:"):
            depth = 1
            if out[-2:] == [":", ":"]:
                del out[-2:]
        else:
            out.append(c)
        i += 1
    return "".join(out)


def short(n):
    """A readable function name: no generic arguments, paths cut to their last two parts."""
    n = strip_generics(n)
    n = re.sub(r"(?:[A-Za-z_][A-Za-z0-9_]*::)+([A-Za-z_][A-Za-z0-9_]*::[A-Za-z_{][^\s<>:]*)", r"\1", n)
    return n[:140]


def sym_crate(name):
    m = re.search(r"([A-Za-z_][A-Za-z0-9_]*)::", name)
    return m.group(1) if m else None


def module_file(ident, segs):
    """The source file of a module path inside a workspace crate, if it can be found."""
    if UDIGI.get(ident):
        f = module_file_in(UDIGI[ident], segs)
        return f"{ident}/{f}" if f else None
    if ident not in CRATES:
        return None
    return module_file_in(CRATES[ident][1], segs)


def module_file_in(src, segs):
    for k in range(len(segs), 0, -1):
        rel = "/".join(segs[:k])
        for f in (rel + ".rs", rel + "/mod.rs"):
            if os.path.exists(os.path.join(src, f)):
                return f
    for f in ("lib.rs", "main.rs"):
        if os.path.exists(os.path.join(src, f)):
            return f
    return None


def font_notes():
    notes = {}
    try:
        t = open(os.path.join(ROOT, "tools/mkassets.py")).read()
        for name, face, px, use in re.findall(r'\("(\w+)", (\w+), (\d+), \w+, \w+, "([^"]*)"\)', t):
            notes[name.upper()] = f"{face.lower()} {px} px: {use}"
    except OSError:
        pass
    return notes


# ---- attribution -----------------------------------------------------------------------------------------------------
def analyse(elf, innermost=False):
    im = Image(elf)
    n = im.n
    leaves, lkey = [], {}

    def leaf(*key):
        if key not in lkey:
            lkey[key] = len(leaves)
            leaves.append(key)
        return lkey[key]

    owner = [-1] * n
    for s in im.secs:  # default: the section's "other"
        o = s["lma"] - im.base
        owner[o:o + s["size"]] = [leaf(REGION.get(s["name"], s["name"]), "other", s["name"])] * s["size"]
    pad = leaf("padding", "padding")
    owner = [pad if x < 0 else x for x in owner]

    def region(o):
        s = im.sec_at(o)
        return REGION.get(s["name"], s["name"]) if s else "padding"

    # code: DWARF line rows and their inlined frames (addr2line -i, innermost first)
    starts, ranges, prev = set(), [], None
    for l in run("arm-none-eabi-objdump", "--dwarf=decodedline", elf).splitlines():
        p = l.split()
        if len(p) >= 3 and p[2].startswith("0x"):
            a = int(p[2], 16)
            if prev is not None and a > prev and a - prev < 0x10000:
                ranges.append((prev, a))
            prev = None if p[1] == "-" else a
    rows = sorted({r for r in ranges if im.off(r[0]) is not None})
    out = run("arm-none-eabi-addr2line", "-e", elf, "-f", "-i", "-a", "-C",
              inp="\n".join(f"{a:#x}" for a, _ in rows) + "\n").splitlines()
    frames, i = {}, 0
    while i < len(out):
        if out[i].startswith("0x"):
            a, fs = int(out[i], 16), []
            i += 1
            while i + 1 < len(out) and not out[i].startswith("0x"):
                fs.append((out[i], out[i + 1].rsplit(":", 1)[0]))
                i += 2
            if fs:
                frames[a] = fs
        else:
            i += 1
    for path in {p for fs in frames.values() for _f, p in fs}:  # learns where the udigimodes crates are
        file_node(path)
    # std code inlined into ours (Option::map, slice indexing, min, ...) counts where it was used: the innermost
    # frame outside the standard library, unless innermost is asked for or the code is std's own
    frame = {}
    for a, fs in frames.items():
        frame[a] = fs[0] if innermost else next((f for f in fs if file_node(f[1])[0] != STD), fs[0])
    # symbols
    fnotes = font_notes()
    syms, named = [], bytearray(n)
    for l in run("arm-none-eabi-nm", "-S", "-C", elf).splitlines():
        p = l.split(" ", 3)
        if len(p) < 4 or p[2] not in "tTrRdDbB" or p[3].startswith("__") or p[3].startswith("$"):
            continue
        a, sz, t, name = int(p[0], 16), int(p[1], 16), p[2], p[3]
        o = im.off(a)
        if o is None or sz == 0 or o + sz > n:
            continue
        syms.append((o, sz, t, name))
    syms.sort()
    symleaf = {}
    for o, sz, t, name in syms:
        reg = region(o)
        ident = sym_crate(name)
        cr = crate_display(ident) if ident else "other"
        m = re.match(r"ferrite_ui::assets::([A-Z]+_?\d+[A-Z]?)(_BITS|_GLYPHS|_CODES)?$", name)
        mi = re.match(r"ferrite_ui::assets::((?:LOGO|ICON|WORD)\w*?)(_BITS)?$", name)
        if mi and not mi.group(1).startswith("WORD"):
            k = leaf(reg, cr, "images", mi.group(1))
        elif m:
            f = m.group(1)
            k = leaf(reg, cr, "fonts", f"{f} font" + (f" ({fnotes[f].split(':')[0]})" if f in fnotes else ""))
        elif t in "tT":
            k = leaf(reg, cr, "(no line info)", short(name))
        else:
            segs = [x for x in name.split("::")[1:-1]] if ident else []
            f = module_file(ident, segs) if ident and all(re.match(r"^\w+$", x) for x in segs) else None
            if f is None and ident:
                f = "/".join(segs) or "(crate root)"
            k = leaf(reg, cr, f or "(symbols)", short(name.split("::")[-1] if ident else name))
        symleaf[o] = k
        owner[o:o + sz] = [k] * sz
        named[o:o + sz] = b"\1" * sz

    # code: DWARF rows over the symbols
    for a, b in rows:
        fn, path = frame.get(a, ("??", "??"))
        o = im.off(a)
        s = im.sec_at(o)
        if not s or not s["code"] or path.startswith("??"):
            continue
        cr, f = file_node(path)
        b = min(b, s["vma"] + s["size"])
        if fn == "??":
            continue
        k = leaf(region(o), cr, f, short(fn))
        owner[o:o + (b - a)] = [k] * (b - a)

    # unnamed constants: split at every pointer into them, owned by whoever points there
    ro = [s for s in im.secs if not s["code"] and s["name"] in (".rodata", ".slow_rodata")]
    unnamed = bytearray(n)
    for s in ro:
        o = s["lma"] - im.base
        for j in range(o, o + s["size"]):
            if not named[j]:
                unnamed[j] = 1
    refs = collections.defaultdict(list)  # target offset -> [owner leaf or ("piece", target)]

    def ref(src_leaf, target_vma):
        t = im.off(target_vma)
        if t is not None and unnamed[t]:
            refs[t].append(src_leaf)

    movw = {}
    lit = re.compile(r"^\s*([0-9a-f]+):\s+(\S+)\s+([^@]*)@ \(([0-9a-f]+)")
    mv = re.compile(r"^\s*([0-9a-f]+):\s+(movw|movt)\S*\s+(\w+), #(\d+)")
    for l in run("arm-none-eabi-objdump", "-d", "--no-show-raw-insn", elf).splitlines():
        m = lit.match(l)
        if m and "[pc" in m.group(3):
            ia, la = int(m.group(1), 16), int(m.group(4), 16)
            lo, io = im.off(la), im.off(ia)
            if lo is None or io is None:
                continue
            for w in ((0, 4) if m.group(2).startswith("ldrd") else (0,)):
                v = im.word(lo + w)
                if v is not None:
                    ref(owner[io], v)
            continue
        m = mv.match(l)
        if m:
            ia, op, r, v = int(m.group(1), 16), m.group(2), m.group(3), int(m.group(4))
            if op == "movw":
                movw[r] = (ia, v)
            elif r in movw and ia - movw[r][0] < 16:
                io = im.off(ia)
                if io is not None:
                    ref(owner[io], (v << 16) | movw[r][1])
    for o, sz, t, name in syms:  # pointers in named data
        if t in "rRdD":
            for j in range((o + 3) & ~3, o + sz - 3, 4):
                ref(symleaf[o], im.word(j))
    # pieces
    cuts = sorted(refs)
    piece_of = {}
    pieces = []  # (start, end)
    for s in ro:
        o, e = s["lma"] - im.base, s["lma"] - im.base + s["size"]
        j = o
        while j < e:
            if not unnamed[j]:
                j += 1
                continue
            k = j
            while k < e and unnamed[k]:
                k += 1
            pts = sorted({j} | {c for c in cuts[_bisect(cuts, j):_bisect(cuts, k)]})
            for x, y in zip(pts, pts[1:] + [k]):
                piece_of[x] = len(pieces)
                pieces.append((x, y))
            j = k
    # pointers inside unnamed pieces (tables of strings): resolved after their own owner
    inner = collections.defaultdict(list)  # piece -> target offsets
    for pi, (x, y) in enumerate(pieces):
        for j in range((x + 3) & ~3, y - 3, 4):
            t = im.off(im.word(j))
            if t is not None and unnamed[t] and t in piece_of:
                inner[pi].append(t)
    pown = [None] * len(pieces)
    for pi, (x, y) in enumerate(pieces):
        if x in refs:
            pown[pi] = collections.Counter(refs[x]).most_common(1)[0][0]
    for _ in range(6):
        ch = False
        for pi, ts in inner.items():
            if pown[pi] is None:
                continue
            for t in ts:
                q = piece_of[t]
                if pown[q] is None:
                    pown[q] = pown[pi]
                    ch = True
        if not ch:
            break
    for pi, (x, y) in enumerate(pieces):
        d = im.img[x:y]
        text = sum(32 <= c < 127 or c >= 0xC2 or c in (9, 10) for c in d) >= 0.9 * len(d) and len(d) >= 2
        what = "strings" if text else "constants"
        reg = region(x)
        if pown[pi] is None:
            k = leaf(reg, "other", "unreferenced constants", what)
        else:
            src = leaves[pown[pi]]
            cr, f = (src[1], src[2]) if len(src) >= 3 else ("other", src[-1])
            if src[1] == "other":
                cr, f = "other", "constants"
            k = leaf(reg, cr, f, what)
        owner[x:y] = [k] * (y - x)

    # count
    cnt = collections.Counter(owner)
    tree = {"name": "image", "children": {}}
    for k, sz in cnt.items():
        node = tree
        for part in leaves[k][:-1]:
            node = node["children"].setdefault(part, {"name": part, "children": {}})
        lf = node["children"].setdefault(leaves[k][-1], {"name": leaves[k][-1], "size": 0})
        if "children" in lf:  # a leaf name that is also a folder
            lf = lf["children"].setdefault("(self)", {"name": "(self)", "size": 0})
        lf["size"] += sz
    slow = min(s["lma"] for s in im.secs if s["name"].startswith(".slow"))  # the fast part ends where slow starts
    fast = slow - im.base
    return finish(tree), dict(image=n, base=im.base, fast_used=fast, fast_size=FAST_END - APP_BASE,
                              app_size=APP_SIZE, img=im.img)


def _bisect(a, x):
    import bisect
    return bisect.bisect_left(a, x)


def finish(node):
    if "children" in node:
        kids = [finish(c) for c in node["children"].values()]
        kids.sort(key=lambda c: -c["size"])
        return {"name": node["name"], "size": sum(c["size"] for c in kids), "children": kids}
    return node


def prune(node, minb):
    if "children" not in node:
        return node
    keep, small, cnt = [], 0, 0
    for c in node["children"]:
        if c["size"] < minb:
            small += c["size"]
            cnt += 1
        else:
            keep.append(prune(c, minb))
    if cnt == 1 and small:
        keep.append({"name": [c for c in node["children"] if c["size"] < minb][0]["name"], "size": small})
    elif cnt:
        keep.append({"name": f"small ({cnt} items under {minb} B)", "size": small})
    return {"name": node["name"], "size": node["size"], "children": keep}


def leaves_of(node, path=()):
    if "children" not in node:
        yield path + (node["name"],), node["size"]
    else:
        for c in node["children"]:
            yield from leaves_of(c, path + (node["name"],) if path or node["name"] != "image" else ())


def walk(node, path=()):
    yield path, node["size"]
    for c in node.get("children", []):
        yield from walk(c, path + (c["name"],))


# ---- report ----------------------------------------------------------------------------------------------------------
def which_build(img):
    for name, (_f, binf) in BUILDS.items():
        p = os.path.join(ROOT, "target", binf)
        if os.path.exists(p) and open(p, "rb").read() == img:
            return name
    return None


def git_rev():
    try:
        r = run("git", "-C", ROOT, "rev-parse", "--short", "HEAD").strip()
        dirty = run("git", "-C", ROOT, "status", "--porcelain", "--untracked-files=no").strip()
        return r + ("+dirty" if dirty else "")
    except Exception:
        return "?"


def kb(b):
    return f"{b / 1024:.1f} KB"


def summary(doc):
    t, img = doc["tree"], doc["meta"]["image"]
    print(f"{doc['meta']['build']} build {doc['meta']['commit']}: image {img} B ({kb(img)}) of {kb(APP_SIZE)}, "
          f"free {kb(APP_SIZE - img)}")
    for r in t["children"]:
        print(f"  {r['name']:16} {r['size']:8} B {100 * r['size'] / img:5.1f}%")
    crates = collections.Counter()
    for r in t["children"]:
        for c in r.get("children", []):
            crates[c["name"]] += c["size"]
    print("  by crate: " + ", ".join(f"{c} {kb(s)}" for c, s in crates.most_common(12)))
    tot = sum(s for _p, s in leaves_of(t))
    print(f"check: leaves {tot} B {'==' if tot == img else '!='} image {img} B: {'ok' if tot == img else 'MISMATCH'}")
    return tot == img


def diff(new, old, top=25):
    def flat(t):
        return {p: s for p, s in walk(t)}
    a, b = flat(old["tree"]), flat(new["tree"])
    d = old["meta"]["image"], new["meta"]["image"]
    print(f"image {d[0]} -> {d[1]} B ({d[1] - d[0]:+d})  [{old['meta']['build']} {old['meta']['commit']} -> "
          f"{new['meta']['build']} {new['meta']['commit']}]")

    def show(title, keys):
        rows = sorted(((b.get(k, 0) - a.get(k, 0), k) for k in keys), key=lambda x: -abs(x[0]))
        rows = [r for r in rows if r[0]][:top]
        if rows:
            print(title)
            for dv, k in rows:
                print(f"  {dv:+8d}  {a.get(k, 0):8d} -> {b.get(k, 0):8d}  {' > '.join(k)}")
    keys = set(a) | set(b)
    show("by region:", [k for k in keys if len(k) == 1])
    ca, cb = collections.Counter(), collections.Counter()
    for src, dst in ((a, ca), (b, cb)):
        for k, s in src.items():
            if len(k) == 2:
                dst[k[1]] += s
    print("by crate (all regions):")
    for dv, c in sorted(((cb[c] - ca[c], c) for c in set(ca) | set(cb)), key=lambda x: -abs(x[0]))[:12]:
        if dv:
            print(f"  {dv:+8d}  {ca[c]:8d} -> {cb[c]:8d}  {c}")
    show("by file:", [k for k in keys if len(k) == 3])
    la, lb = dict(leaves_of(old["tree"])), dict(leaves_of(new["tree"]))
    a, b = la, lb
    show("by leaf:", set(la) | set(lb))


def html(doc, minb):
    t = prune(doc["tree"], minb)
    data = json.dumps({"tree": t, "meta": {k: v for k, v in doc["meta"].items()}}, separators=(",", ":"))
    return PAGE.replace("/*DATA*/null", data.replace("</", "<\\/"))


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("input", nargs="?", default=ELF, help="the firmware ELF, or a tree saved with --json")
    ap.add_argument("--build", choices=list(BUILDS), help="run tools/mkbtf.sh for this build first")
    ap.add_argument("-o", "--out", default=os.path.join(ROOT, "target/sizemap.html"), help="the HTML map")
    ap.add_argument("--json", metavar="FILE", help="also write the full tree as JSON")
    ap.add_argument("--diff", metavar="OLD.json", help="print the biggest changes from OLD.json to this build")
    ap.add_argument("--min", type=int, default=64, help="leaves under this many bytes merge in the map (64)")
    ap.add_argument("--innermost", action="store_true",
                    help="count inlined std code (Option::map, indexing, ...) under std, not where it was used")
    ap.add_argument("--top", type=int, default=25, help="rows per --diff section (25)")
    a = ap.parse_args()
    if a.build:
        subprocess.run(["sh", os.path.join(ROOT, "tools/mkbtf.sh")] + ([BUILDS[a.build][0]] if BUILDS[a.build][0] else []),
                       check=True, stdout=sys.stderr)
    if a.input.endswith(".json"):
        doc = json.load(open(a.input))
    else:
        tree, meta = analyse(a.input, a.innermost)
        build = which_build(meta.pop("img"))
        if a.build and build != a.build:
            print(f"warning: the ELF does not match target/{BUILDS[a.build][1]}", file=sys.stderr)
        if build is None:
            print("warning: the ELF matches no target/*.bin (rebuilt since?): build shown as 'unknown'", file=sys.stderr)
        meta.update(build=build or "unknown", commit=git_rev(), elf=os.path.relpath(a.input, ROOT))
        doc = {"tree": tree, "meta": meta}
        open(a.out, "w").write(html(doc, a.min))
        print(f"wrote {a.out} ({os.path.getsize(a.out) // 1024} KB)")
    ok = summary(doc)
    if a.json:
        json.dump(doc, open(a.json, "w"), separators=(",", ":"))
        print(f"wrote {a.json}")
    if a.diff:
        diff(doc, json.load(open(a.diff)), a.top)
    sys.exit(0 if ok else 1)


PAGE = r"""<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Ferrite Flash Map</title>
<style>
:root{color-scheme:light;--bg:#fcfcfb;--panel:#f3f2ef;--ink:#0b0b0b;--ink2:#52514e;--ink3:#8a8984;--line:#dcdbd6;
--free:#e7e6e1;--accent:#2a78d6;
--c1:#2a78d6;--c2:#eb6834;--c3:#1baf7a;--c4:#eda100;--c5:#e87ba4;--c6:#008300;--c7:#6250d6;--c8:#9b9a94}
@media (prefers-color-scheme:dark){:root:not([data-theme="light"]){color-scheme:dark;--bg:#1a1a19;--panel:#242422;
--ink:#f0efec;--ink2:#c3c2b7;--ink3:#8f8e86;--line:#3a3a37;--free:#2f2f2c;--accent:#3987e5;
--c1:#3987e5;--c2:#d95926;--c3:#199e70;--c4:#c98500;--c5:#d55181;--c6:#008300;--c7:#9085e9;--c8:#6d6c66}}
:root[data-theme="dark"]{color-scheme:dark;--bg:#1a1a19;--panel:#242422;--ink:#f0efec;--ink2:#c3c2b7;--ink3:#8f8e86;
--line:#3a3a37;--free:#2f2f2c;--accent:#3987e5;
--c1:#3987e5;--c2:#d95926;--c3:#199e70;--c4:#c98500;--c5:#d55181;--c6:#008300;--c7:#9085e9;--c8:#6d6c66}
*{box-sizing:border-box}
html,body{margin:0;background:var(--bg);color:var(--ink);font:14px/1.4 system-ui,-apple-system,"Segoe UI",sans-serif}
main{max-width:1200px;margin:0 auto;padding:16px}
header{display:flex;flex-wrap:wrap;align-items:baseline;gap:4px 16px}
h1{font-size:20px;margin:0}
.meta{color:var(--ink2);font-size:13px;flex:1}
button{font:inherit;color:var(--ink);background:var(--panel);border:1px solid var(--line);border-radius:6px;
padding:4px 10px;cursor:pointer}
.bar{margin:14px 0 4px;height:18px;border-radius:4px;background:var(--free);display:flex;overflow:hidden;position:relative}
.bar span{height:100%;border-right:2px solid var(--bg)}
.bar .fastmark{position:absolute;top:-3px;bottom:-3px;width:2px;background:var(--ink);border:0}
.barlbl{display:flex;flex-wrap:wrap;justify-content:space-between;gap:4px 12px;color:var(--ink2);font-size:13px}
.legend{display:flex;flex-wrap:wrap;gap:4px 14px;margin:10px 0;font-size:13px;color:var(--ink2)}
.legend i{display:inline-block;width:10px;height:10px;border-radius:2px;margin-right:5px;vertical-align:-1px}
.crumbs{margin:8px 0;font-size:14px;overflow-wrap:anywhere}
.crumbs a{color:var(--accent);cursor:pointer;text-decoration:none}
.crumbs a:hover{text-decoration:underline}
#map{position:relative;width:100%;height:min(68vh,640px);min-height:340px;background:var(--panel);border-radius:6px;overflow:hidden}
.n{position:absolute;overflow:hidden;border-radius:3px;cursor:pointer;font-size:12px;line-height:1.25;padding:2px 4px}
.g{background:transparent;border:1px solid var(--line)}
.g>.h{color:var(--ink);font-weight:600;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.l{color:#fff;text-shadow:0 0 2px rgba(0,0,0,.6);white-space:nowrap;text-overflow:ellipsis}
.l:hover,.g:hover{outline:2px solid var(--ink);outline-offset:-2px;z-index:2}
#tip{position:fixed;pointer-events:none;background:var(--bg);color:var(--ink);border:1px solid var(--line);
border-radius:6px;padding:6px 8px;font-size:13px;box-shadow:0 2px 10px rgba(0,0,0,.2);max-width:min(420px,90vw);
display:none;z-index:9;overflow-wrap:anywhere}
#tip b{display:block}
#tip .p{color:var(--ink2);font-size:12px}
h2{font-size:16px;margin:22px 0 6px}
table{width:100%;border-collapse:collapse;font-size:13px;table-layout:fixed}
th,td{text-align:left;padding:4px 6px;border-bottom:1px solid var(--line);vertical-align:top;overflow-wrap:anywhere}
th{color:var(--ink2);font-weight:600}
td.num,th.num{text-align:right;width:84px;font-variant-numeric:tabular-nums}
th.rk{width:30px}
td .p{color:var(--ink3);font-size:12px}
tbody tr{cursor:pointer}
tbody tr:hover{background:var(--panel)}
@media (max-width:600px){td.pc,th.pc{display:none}td.num,th.num{width:68px}}
</style></head><body><main>
<header><h1>Ferrite Flash Map</h1><div class="meta" id="meta"></div><button id="theme" type="button">Theme</button></header>
<div class="bar" id="bar"></div><div class="barlbl" id="barlbl"></div>
<div class="legend" id="legend"></div>
<div class="crumbs" id="crumbs"></div>
<div id="map"></div>
<h2>Biggest 20 leaves</h2>
<table><thead><tr><th class="rk">#</th><th>Leaf</th><th class="num">Bytes</th><th class="num pc">% image</th></tr></thead>
<tbody id="top"></tbody></table>
</main><div id="tip"></div>
<script src="https://cdnjs.cloudflare.com/ajax/libs/d3/7.9.0/d3.min.js"></script>
<script>
const DATA=/*DATA*/null;
const M=DATA.meta, IMG=M.image;
const fmt=b=>b>=10240?(b/1024).toFixed(1)+" KB":b.toLocaleString()+" B";
const pct=(a,b)=>b?(100*a/b).toFixed(1)+"%":"";
document.getElementById("meta").textContent=`${M.build} build ${M.commit} · image ${fmt(IMG)} (${IMG.toLocaleString()} B)`;
const root=d3.hierarchy(DATA.tree).sum(d=>d.children?0:d.size).sort((a,b)=>b.value-a.value);
// crate colours: one fixed slot per crate by total size, the rest grey
const crateTot=new Map();
root.children.forEach(r=>(r.children||[]).forEach(c=>{if(c.children||r.depth)crateTot.set(c.data.name,(crateTot.get(c.data.name)||0)+c.value)}));
const crates=[...crateTot].filter(([n])=>!/^(other|padding)$/.test(n)&&!n.startsWith(".")).sort((a,b)=>b[1]-a[1]).map(x=>x[0]);
const slot=new Map(crates.slice(0,7).map((c,i)=>[c,i+1]));
const crateOf=n=>{const a=n.ancestors().reverse();return a[2]?a[2].data.name:null};
const col=n=>`var(--c${slot.get(crateOf(n))||8})`;
document.getElementById("legend").innerHTML=crates.slice(0,7).map((c,i)=>`<span><i style="background:var(--c${i+1})"></i>${c}</span>`).join("")+`<span><i style="background:var(--c8)"></i>other crates</span>`;
// app region bar
(function(){const bar=document.getElementById("bar");let h="";
 h+=`<span title="fast part: ${fmt(M.fast_used)}" style="width:${100*M.fast_used/M.app_size}%;background:var(--accent)"></span>`;
 h+=`<span title="slow part: ${fmt(IMG-M.fast_used)}" style="width:${100*(IMG-M.fast_used)/M.app_size}%;background:var(--accent);opacity:.55"></span>`;
 h+=`<span class="fastmark" style="left:${100*M.fast_size/M.app_size}%" title="end of zero-wait flash (256 KB)"></span>`;
 bar.innerHTML=h;
 document.getElementById("barlbl").innerHTML=`<span>app region: <b>${fmt(IMG)}</b> used of ${fmt(M.app_size)} (${pct(IMG,M.app_size)}), <b>${fmt(M.app_size-IMG)}</b> free</span><span>fast part (dark): ${fmt(M.fast_used)} of the ${fmt(M.fast_size)} zero-wait flash (the mark)</span>`;})();
const tip=document.getElementById("tip");
function showTip(e,n){const p=n.parent;
 tip.innerHTML=`<b>${esc(n.data.name)}</b><span class="p">${esc(n.ancestors().reverse().slice(1,-1).map(a=>a.data.name).join(" › "))}</span><br>${n.value.toLocaleString()} B · ${pct(n.value,IMG)} of image${p?` · ${pct(n.value,p.value)} of ${esc(p.data.name)}`:""}`;
 tip.style.display="block";const x=Math.min(e.clientX+12,innerWidth-tip.offsetWidth-8),y=e.clientY+16+tip.offsetHeight>innerHeight?e.clientY-tip.offsetHeight-10:e.clientY+16;
 tip.style.left=x+"px";tip.style.top=y+"px"}
const esc=s=>String(s).replace(/[&<>"]/g,c=>({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;"}[c]));
let focus=root;
function draw(){
 const el=document.getElementById("map"),W=el.clientWidth,H=el.clientHeight;
 const h=d3.hierarchy(focus.data).sum(d=>d.children?0:d.size).sort((a,b)=>b.value-a.value);
 d3.treemap().tile(d3.treemapSquarify).size([W,H]).paddingOuter(3).paddingTop(d=>d.depth===1&&d.children?18:3).paddingInner(2).round(true)(h);
 // map the local layout back to the global nodes (same order)
 const glob=new Map();(function m(a,b){glob.set(a,b);(a.children||[]).forEach((c,i)=>m(c,b.children[i]))})(h,focus);
 let html="";const items=[];
 h.descendants().forEach(d=>{if(d.depth<1||d.depth>2)return;const w=d.x1-d.x0,hh=d.y1-d.y0;if(w<1||hh<1)return;
  const g=glob.get(d),grp=d.depth===1&&d.children;const i=items.push(g)-1;
  const label=w>40&&hh>14?`${esc(d.data.name)}${hh>28?`<br><span style="opacity:.85">${fmt(d.value)}</span>`:""}`:"";
  if(grp)html+=`<div class="n g" data-i="${i}" style="left:${d.x0}px;top:${d.y0}px;width:${w}px;height:${hh}px"><div class="h">${w>40?esc(d.data.name)+" · "+fmt(d.value):""}</div></div>`;
  else html+=`<div class="n l" data-i="${i}" style="left:${d.x0}px;top:${d.y0}px;width:${w}px;height:${hh}px;background:${col(g)};opacity:${d.depth===1||!d.children?1:0.92}">${label}</div>`;});
 el.innerHTML=html;
 el.onmousemove=e=>{const t=e.target.closest(".n");if(!t){tip.style.display="none";return}showTip(e,items[+t.dataset.i])};
 el.onmouseleave=()=>tip.style.display="none";
 el.onclick=e=>{const t=e.target.closest(".n");if(!t)return;let n=items[+t.dataset.i];
  if(!n.children)n=n.parent; // a leaf: zoom to the folder holding it
  zoom(n)};
 const c=focus.ancestors().reverse();
 document.getElementById("crumbs").innerHTML=c.map((a,i)=>i<c.length-1?`<a data-d="${i}">${esc(a.depth?a.data.name:"image")}</a>`:`<b>${esc(a.depth?a.data.name:"image")}</b> · ${fmt(a.value)} (${pct(a.value,IMG)})`).join(" › ");
 document.querySelectorAll("#crumbs a").forEach(a=>a.onclick=()=>zoom(c[+a.dataset.d]));
}
function zoom(n){if(!n||!n.children)return;focus=n;tip.style.display="none";draw()}
addEventListener("resize",()=>draw());
// top 20 leaves
const lv=root.leaves().filter(l=>!/^small \(/.test(l.data.name)).sort((a,b)=>b.value-a.value).slice(0,20);
document.getElementById("top").innerHTML=lv.map((l,i)=>`<tr data-i="${i}"><td>${i+1}</td><td>${esc(l.data.name)}<br><span class="p">${esc(l.ancestors().reverse().slice(1,-1).map(a=>a.data.name).join(" › "))}</span></td><td class="num">${l.value.toLocaleString()}</td><td class="num pc">${pct(l.value,IMG)}</td></tr>`).join("");
document.querySelectorAll("#top tr").forEach(r=>r.onclick=()=>{zoom(lv[+r.dataset.i].parent);document.getElementById("map").scrollIntoView({behavior:"smooth"})});
// theme toggle: light / dark, starting from the system's
document.getElementById("theme").onclick=()=>{const d=document.documentElement,cur=d.dataset.theme||(matchMedia("(prefers-color-scheme: dark)").matches?"dark":"light");d.dataset.theme=cur==="dark"?"light":"dark"};
draw();
</script></body></html>
"""

if __name__ == "__main__":
    main()
