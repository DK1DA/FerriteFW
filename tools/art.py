"""Shapes shared by the proofs and the firmware assets: the Ferrite logo and small UI icons.

Every shape is a function draw(d, n) that draws white on an n x n Pillow "L" image. mask() renders it
at 8x and scales down, then quantises to 4 bits, which is what the firmware stores.
"""
import math

from PIL import Image, ImageDraw


def mask(size, draw_fn):
    big = Image.new("L", (size * 8, size * 8), 0)
    draw_fn(ImageDraw.Draw(big), size * 8)
    small = big.resize((size, size), Image.LANCZOS)
    return bytes((v * 15 + 127) // 255 for v in small.getdata())


# Ferrite logo: a toroid core (ring) with 14 windings over 270 degrees, in a 120-unit box.
# Same geometry as docs/img/ferrite-logo.svg. Ring and windings are separate masks (two colours).
def toroid_ring(d, n):
    k = n / 120
    d.ellipse([22 * k, 22 * k, 98 * k, 98 * k], outline=255, width=int(16 * k))


def toroid_wind(d, n):
    k = n / 120
    for i in range(14):
        a = math.radians(-225 + i * 270 / 13)
        d.line([((60 + 24 * math.cos(a)) * k, (60 + 24 * math.sin(a)) * k),
                ((60 + 52 * math.cos(a)) * k, (60 + 52 * math.sin(a)) * k)], fill=255, width=max(1, int(5 * k)))


def check(d, n):
    d.line([(n * .15, n * .52), (n * .40, n * .76), (n * .86, n * .26)], fill=255, width=int(n * .14), joint="curve")


def star(d, n):
    """A five-pointed star (★), point up: the text fonts' drawn glyph for the priority channel (decision 157)."""
    c, ro, ri = n / 2, n / 2, n / 2 * 0.42
    pts = [(c + (ro if i % 2 == 0 else ri) * math.sin(math.radians(i * 36)),
            c + 0.06 * n - (ro if i % 2 == 0 else ri) * math.cos(math.radians(i * 36))) for i in range(10)]
    d.polygon(pts, fill=255)


def dot(d, n):
    d.ellipse([n * .25, n * .25, n * .75, n * .75], outline=255, width=int(n * .1))


def cross(d, n):
    w = int(n * .14)
    d.line([(n * .22, n * .22), (n * .78, n * .78)], fill=255, width=w)
    d.line([(n * .78, n * .22), (n * .22, n * .78)], fill=255, width=w)


def lock(d, n):
    d.rounded_rectangle([n * .2, n * .45, n * .8, n * .9], radius=n * .08, outline=255, width=int(n * .1))
    d.arc([n * .32, n * .12, n * .68, n * .62], 180, 360, fill=255, width=int(n * .1))


def gps(d, n):
    d.ellipse([n * .22, n * .22, n * .78, n * .78], outline=255, width=int(n * .11))
    for a, b in (((.5, 0), (.5, .2)), ((.5, .8), (.5, 1)), ((0, .5), (.2, .5)), ((.8, .5), (1, .5))):
        d.line([(a[0] * n, a[1] * n), (b[0] * n, b[1] * n)], fill=255, width=int(n * .11))


def bt(d, n):
    pts = [(.25, .3), (.75, .7), (.5, .92), (.5, .08), (.75, .3), (.25, .7)]
    d.line([(x * n, y * n) for x, y in pts], fill=255, width=int(n * .1), joint="curve")
