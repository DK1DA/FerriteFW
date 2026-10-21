"""The logo files in assets/logo, from the master mark (assets/logo/ferrite-logo.svg, the toroid the firmware
draws too): the mark and a lockup with the FERRITE wordmark (Barlow Semi Condensed SemiBold, as outlines so it
renders without the font) for dark and light backgrounds, and the 1280 x 640 social preview.

    python3 tools/mklogo.py && tools/mkicons.sh

Needs fontTools.
"""
import math
import os
import re
from fontTools.ttLib import TTFont
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
FONTS = os.path.join(ROOT, "fonts") + "/"
OUT = os.path.join(ROOT, "assets", "logo")
src = open(os.path.join(OUT, "ferrite-logo.svg")).read()
LINES = re.findall(r'<line [^>]*/>', src)

DARK = dict(ring="#9AA3AE", wind="#4DA3FF", text="#F0F3F6", sub="#9AA3AE", bg="#0B0D10")
LIGHT = dict(ring="#5B6470", wind="#1F6FD1", text="#0B0D10", sub="#5B6470", bg="#FFFFFF")

def mark(c, x=0, y=0, s=1.0):
    return (f'<g transform="translate({x} {y}) scale({s})">'
            f'<circle cx="60" cy="60" r="38" fill="none" stroke="{c["ring"]}" stroke-width="16"/>'
            f'<g stroke="{c["wind"]}" stroke-width="5" stroke-linecap="round">{"".join(LINES)}</g></g>')

def text_path(txt, font, size, x, y, tracking=0.0):
    """Outlines of `txt`, baseline at y; returns (path d, advance width)."""
    f = TTFont(FONTS + font)
    gs, cmap, hmtx = f.getGlyphSet(), f.getBestCmap(), f["hmtx"]
    k = size / f["head"].unitsPerEm
    pen = SVGPathPen(gs)
    pos = 0.0
    for ch in txt:
        g = cmap[ord(ch)]
        gs[g].draw(TransformPen(pen, (k, 0, 0, -k, x + pos, y)))
        pos += hmtx[g][0] * k + tracking
    return pen.getCommands(), pos - tracking

def svg(w, h, body, bg=None):
    b = f'<rect width="{w}" height="{h}" fill="{bg}"/>' if bg else ""
    return f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" width="{w}" height="{h}">{b}{body}</svg>\n'

out = OUT
for name, c in (("dark", DARK), ("light", LIGHT)):
    open(f"{out}/ferrite-mark-{name}.svg", "w").write(svg(120, 120, mark(c)))
    d, wd = text_path("FERRITE", "BarlowSemiCondensed-SemiBold.ttf", 84, 136, 88, tracking=4)
    open(f"{out}/ferrite-lockup-{name}.svg", "w").write(
        svg(int(136 + wd + 4), 120, mark(c) + f'<path d="{d}" fill="{c["text"]}"/>'))

# Social preview, 1280 x 640 (GitHub's size): lockup centred, tagline under it, on the dark theme's background.
c = DARK
d1, w1 = text_path("FERRITE", "BarlowSemiCondensed-SemiBold.ttf", 150, 0, 0, tracking=6)
tag = "New firmware for the Radtel RT-950 Pro"
d2, w2 = text_path(tag, "AtkinsonHyperlegible-Regular.ttf", 40, 0, 0)
ms = 1.75  # mark 210 px
lock_w = 120 * ms + 36 + w1
x0 = (1280 - lock_w) / 2
body = (mark(c, x0, 170, ms)
        + f'<path transform="translate({x0 + 120 * ms + 36} {327})" d="{d1}" fill="{c["text"]}"/>'
        + f'<path transform="translate({(1280 - w2) / 2} {470})" d="{d2}" fill="{c["sub"]}"/>')
open(f"{out}/social-preview.svg", "w").write(svg(1280, 640, body, c["bg"]))


# Icons: the mark on a rounded square of the dark background. The full mark for large sizes; a simplified one
# (7 thicker windings, a thicker ring) for 16-48 px, where 14 thin windings blur together.
def windings(n, w, r0, r1):
    """n windings over 270 degrees with the gap at the bottom, as the master mark (inner radius 24, outer 52)."""
    out = []
    for k in range(n):
        a = math.radians(225 - 270 * k / (n - 1))
        x0, y0 = 60 + r0 * math.cos(a), 60 - r0 * math.sin(a)
        x1, y1 = 60 + r1 * math.cos(a), 60 - r1 * math.sin(a)
        out.append(f'<line x1="{x0:.1f}" y1="{y0:.1f}" x2="{x1:.1f}" y2="{y1:.1f}"/>')
    return f'<g stroke="{DARK["wind"]}" stroke-width="{w}" stroke-linecap="round">{"".join(out)}</g>'


def icon(mark_body, pad):
    s = (512 - 2 * pad) / 120
    return svg(512, 512, f'<rect width="512" height="512" rx="112" fill="{DARK["bg"]}"/>'
               f'<g transform="translate({pad} {pad}) scale({s:.4f})">{mark_body}</g>')


full = (f'<circle cx="60" cy="60" r="38" fill="none" stroke="{DARK["ring"]}" stroke-width="16"/>'
        f'<g stroke="{DARK["wind"]}" stroke-width="5" stroke-linecap="round">{"".join(LINES)}</g>')
small = (f'<circle cx="60" cy="60" r="36" fill="none" stroke="{DARK["ring"]}" stroke-width="22"/>'
         + windings(7, 10, 26, 54))
open(f"{out}/ferrite-icon.svg", "w").write(icon(full, 56))
open(f"{out}/ferrite-icon-small.svg", "w").write(icon(small, 40))
