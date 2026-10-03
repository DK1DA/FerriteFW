"""Ferrite Mono Tight: JetBrains Mono with the white space squeezed out, for the frequency digits.

    python3 tools/tighten.py fonts/JetBrainsMono-Bold.ttf 540 fonts/FerriteMonoTight-Bold.ttf

The glyph shapes are untouched. Digits keep one shared advance (so frequencies never shift as they change), cut
from JetBrains Mono's 600 units to the given advance, each re-centred in it. The point, comma, colon, hyphen and
space shrink to their own ink plus a small margin. The family is renamed, as the OFL asks of modified versions.

Licence: SIL Open Font License 1.1, as JetBrains Mono (fonts/OFL-JetBrainsMono.txt); JetBrains Mono reserves
no font name.
"""
import sys
from fontTools.ttLib import TTFont

src, digit_adv, dst = sys.argv[1], int(sys.argv[2]), sys.argv[3]
MARGIN = 50          # font units each side of a narrow glyph's ink
SPACE = 280          # the space between MHz and the last two digits
font = TTFont(src)
glyf, hmtx, cmap = font["glyf"], font["hmtx"], font.getBestCmap()


def shift(name, dx):
    g = glyf[name]
    if g.isComposite():
        for c in g.components:
            c.x += dx
    elif g.numberOfContours > 0:
        g.coordinates.translate((dx, 0))
    g.recalcBounds(glyf)


for ch in "0123456789":
    n = cmap[ord(ch)]
    adv, _ = hmtx[n]
    dx = -(adv - digit_adv) // 2
    shift(n, dx)
    hmtx[n] = (digit_adv, glyf[n].xMin)
for ch in ".,:-":
    n = cmap[ord(ch)]
    g = glyf[n]
    g.recalcBounds(glyf)
    shift(n, MARGIN - g.xMin)
    hmtx[n] = (glyf[n].xMax + MARGIN, MARGIN)
hmtx[cmap[ord(" ")]] = (SPACE, 0)

for t in ("fpgm", "prep", "cvt ", "hdmx", "LTSH", "VDMX", "GPOS", "kern"):
    if t in font:
        del font[t]
for name in font.getGlyphOrder():
    g = glyf[name]
    if hasattr(g, "program"):
        g.program.fromBytecode(b"")
font["post"].isFixedPitch = 0
weight = font["name"].getDebugName(2) or "Bold"
for rec in font["name"].names:
    if rec.nameID in (1, 16):
        rec.string = "Ferrite Mono Tight"
    elif rec.nameID in (2, 17):
        rec.string = weight
    elif rec.nameID == 4:
        rec.string = f"Ferrite Mono Tight {weight}"
    elif rec.nameID == 6:
        rec.string = f"FerriteMonoTight-{weight}"
    elif rec.nameID == 3:
        rec.string = f"Ferrite Mono Tight {weight}; derived from JetBrains Mono"
font.save(dst)
print(f"{dst}: digits {digit_adv}/1000")
