"""The firmware's fonts and masks as 4-bit Rust data: crates/ferrite-ui/src/assets.rs.

    python3 tools/mkassets.py          (needs Pillow with FreeType; output is committed)

Sources: fonts/*.ttf (OFL, licences beside them) and the shapes in tools/art.py (the logo is the same toroid as
assets/logo). Carried over from the rt950pro workspace's fw/tools/mkassets.py, Rust output only.
"""
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import art  # noqa: E402
from fontgen import Font  # noqa: E402

FONTS = os.path.join(HERE, "..", "fonts")
OUT = os.path.join(HERE, "..", "crates", "ferrite-ui", "src", "assets.rs")
REG, BOLD = "JetBrainsMono-Regular.ttf", "JetBrainsMono-Bold.ttf"
NUM = "FerriteMonoTight-Bold.ttf"  # JetBrains Mono Bold, white space squeezed (tools/tighten.py)
WORD = "BarlowSemiCondensed-SemiBold.ttf"

# Character sets: English only, and only what the UI draws. A string with a character outside its font's set
# fails ferrite-ui's and ferrite-app's tests (they check every string literal), so add it here and regenerate.
ASCII = "".join(chr(c) for c in range(0x20, 0x7F))
TEXT = ASCII + "\u00b7\u00b0\u00b1"  # the middle dot separates values ("VFO · VHF"), the degree sign bearings ("NE 48°"), plus-minus GPS accuracy ("±5 m"); arrows and marks are drawn shapes
DIG = " -.0123456789"  # frequencies
WORDMARK = " EFIRT"

# Characters the font files lack, drawn from tools/art.py at the font's cap height, centred in its advance; only
# in the fonts named (they cost flash in every font they are in). ★ marks a zone's priority channel (decision 157).
DRAWN = {"\u2605": art.star}
DRAWN_IN = ("ui_12", "ui_12b")


def add_drawn(f):
    """The DRAWN characters into font f: a square mask the height of its capitals, sitting on the baseline."""
    cap = -f.glyphs["H"][2]
    adv = f.glyphs["M"][0]
    for ch, shape in DRAWN.items():
        f.glyphs[ch] = (adv, (adv - cap) // 2, -cap, art.mask(cap, shape), cap, cap)


# Font and logo data goes to its own section, which the firmware's linker script places after all code and
# other constants (firmware/memory.x): code stays in the fast first 256 KB as the image grows.
SLOW = ".slow_rodata"

FONT_LIST = [  # name, file, px, chars, tabular, use
    ("ui_12", REG, 12, TEXT, False, "status bar, hints, labels, lists, values"),
    ("ui_12b", BOLD, 12, TEXT, False, "emphasis: clock, keys in hints, tags"),
    ("ui_15b", BOLD, 15, TEXT, False, "headings: setup pages, alerts; the callsign on the boot screen"),
    ("name_26", BOLD, 26, TEXT, False, "channel names on the large home card when they fit (13 characters); the callsign on the owner card"),
    ("name_22", BOLD, 22, TEXT, False, "channel names on the home cards (16 characters fit the large card)"),
    ("dig_46", NUM, 46, DIG, True, "active slot frequency"),
    ("dig_28", NUM, 28, DIG, True, "active slot last digits"),
    ("dig_26", NUM, 26, DIG, True, "watched slot frequency"),
    ("dig_17", NUM, 17, DIG, True, "watched slot last digits"),
    ("word_40", WORD, 40, WORDMARK, False, "FERRITE wordmark, boot screen"),
]
# The big fonts, run-length encoded (as the logo; font4.rs decodes a glyph's rows in turn): 6.2 KB smaller. The 12
# and 15 px fonts stay plain: their glyphs are mostly literal pixels and would grow (size review, Oct 20, 2026).
RLE_FONTS = ("name_26", "name_22", "dig_46", "dig_28", "word_40")
MASK_LIST = [  # name, size, shape, run-length encoded
    ("logo112_ring", 112, art.toroid_ring, True), ("logo112_wind", 112, art.toroid_wind, True),
    ("icon_lock", 14, art.lock, False), ("icon_gps", 12, art.gps, False), ("icon_bt", 11, art.bt, False),
]


def rle(a, w, h):
    """4-bit mask rows as runs (font4.rs decodes it). Each row's runs end at the row's edge, so a row decodes on
    its own, top to bottom, straight into the panel. Tokens:
        00nnnnnn  n+1 pixels of 0 (1-64)          01nnnnnn  n+1 pixels of 15 (1-64)
        1nnnnnnn  n+1 literal pixels (1-128) follow, two per byte, high nibble first
    The shortest of a few thresholds for "worth a run" wins; the decoder doesn't need to know which."""
    def enc(zmin, fmin):
        out = bytearray()
        for y in range(h):
            row, i, lit = a[y * w:(y + 1) * w], 0, []

            def flush():
                while lit:
                    n = min(len(lit), 128)
                    chunk = lit[:n]
                    del lit[:n]
                    out.append(0x80 | (n - 1))
                    out.extend(chunk[j] << 4 | (chunk[j + 1] if j + 1 < n else 0) for j in range(0, n, 2))
            while i < w:
                v, j = row[i], i
                while j < w and row[j] == v:
                    j += 1
                if (v == 0 and j - i >= zmin) or (v == 15 and j - i >= fmin):
                    flush()
                    run = j - i
                    while run:
                        n = min(run, 64)
                        out.append((0x40 if v else 0) | (n - 1))
                        run -= n
                    i = j
                else:
                    lit.append(v)
                    i += 1
            flush()
        return bytes(out)
    return min((enc(z, f) for z in range(1, 6) for f in range(1, 8)), key=len)


def rust_bytes(name, data, attrs, vis=""):
    rows = [", ".join(f"0x{b:02X}" for b in data[i:i + 16]) + "," for i in range(0, len(data), 16)]
    return [*attrs, f"{vis}static {name}: [u8; {len(data)}] = [", *("    " + r for r in rows), "];"]


def main():
    rs = ["// Generated by tools/mkassets.py. Do not edit.", "use crate::font4::{Font4, Glyph4, Mask4};", ""]
    total = 0
    for name, ttf, px, chars, tab, use in FONT_LIST:
        f = Font(os.path.join(FONTS, ttf), px, chars, tab)
        if name in DRAWN_IN:
            add_drawn(f)
        rs.append(f.rust_source(name, SLOW, rle if name in RLE_FONTS else None))
        total += f.size_bytes(rle if name in RLE_FONTS else None)
    for name, size, shape, packed_rle in MASK_LIST:
        a = art.mask(size, shape)
        up = name.upper()
        raw = bytes(a[i] << 4 | (a[i + 1] if i + 1 < len(a) else 0) for i in range(0, len(a), 2))
        data = rle(a, size, size) if packed_rle else raw
        rs += rust_bytes(f"{up}_BITS", data, [f'#[cfg_attr(target_os = "none", link_section = "{SLOW}")]'])
        rs.append(f"pub static {up}: Mask4 = Mask4 {{ w: {size}, h: {size}, rle: {str(packed_rle).lower()}, "
                  f"bits: &{up}_BITS }};")
        if packed_rle:
            # the uncompressed mask, for the test that checks the decoder (not in the firmware)
            rs += rust_bytes(f"{up}_RAW", raw, ["#[cfg(test)]"], "pub ")
        rs.append("")
        total += len(data)
    open(OUT, "w").write("\n".join(rs))
    print(f"{os.path.relpath(OUT)}: {len(FONT_LIST)} fonts, {len(MASK_LIST)} masks, about {total / 1024:.1f} KB of data")


if __name__ == "__main__":
    main()
