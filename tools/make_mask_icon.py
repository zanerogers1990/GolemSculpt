"""Draws assets/icons/tool_mask.png, the Mask Fit tool's icon: a face mask with two eye holes.

    python tools/make_mask_icon.py assets/icons/tool_mask.png

A white outline over a faint fill, in the style of the other tool icons: 128 pixels square, white on clear.
"""
import math
import sys

from PIL import Image, ImageChops, ImageDraw, ImageFilter

SIZE = 128
SUPER = 4


def mask_shape(big):
    """The mask's outline: an egg, broad at the brow and narrowing to a rounded chin."""
    cx, cy = big / 2, big * 0.42
    pts = []
    for k in range(360):
        t = 2 * math.pi * k / 360
        s = math.sin(t)
        if s >= 0:  # the lower half, toward the chin
            x, y = big * 0.38 * math.cos(t) * (1 - 0.35 * s), cy + big * 0.46 * s
        else:  # the brow
            x, y = big * 0.38 * math.cos(t), cy + big * 0.30 * s
        pts.append((cx + x, y))
    return pts


def eye(big, side):
    cx, cy = big / 2 + side * big * 0.15, big * 0.40
    w, h = big * 0.10, big * 0.05
    return [(cx + w * math.cos(a) , cy - h * math.sin(a) * (1.3 if math.sin(a) > 0 else 0.8))
            for a in (2 * math.pi * k / 120 for k in range(120))]


def main():
    out = sys.argv[1]
    big = SIZE * SUPER
    solid = Image.new('L', (big, big), 0)
    d = ImageDraw.Draw(solid)
    d.polygon(mask_shape(big), fill=255)
    for side in (-1, 1):
        d.polygon(eye(big, side), fill=0)
    stroke = 7 * SUPER
    outer = solid.filter(ImageFilter.MaxFilter(stroke // 2 * 2 + 1))
    inner = solid.filter(ImageFilter.MinFilter(stroke // 2 * 2 + 1))
    ring = ImageChops.subtract(outer, inner)
    fill = inner.point(lambda v: v * 70 // 255)
    alpha = ImageChops.lighter(ring, fill).resize((SIZE, SIZE), Image.LANCZOS)
    icon = Image.new('RGBA', (SIZE, SIZE), (255, 255, 255, 0))
    icon.putalpha(alpha)
    icon.save(out)


main()
