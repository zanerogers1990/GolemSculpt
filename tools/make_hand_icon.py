"""Draws assets/icons/tool_hands.png, the Hand & Foot tool's icon, from a hand the tool itself made.

The hand is an OBJ the crate's `write_samples` test writes (see crates/hands/src/lib.rs):

    SCULPT_HANDS_OBJ=<folder> cargo test -p sculpt-hands --release write_samples -- --ignored
    python tools/make_hand_icon.py <folder>/hand_Spread.obj assets/icons/tool_hands.png

Its silhouette seen from the back becomes a white outline over a faint fill, in the style of the
other tool icons: 128 pixels square, white on clear.
"""
import sys

from PIL import Image, ImageChops, ImageDraw, ImageFilter

SIZE = 128
SUPER = 4


def load(path):
    vs, fs = [], []
    with open(path) as f:
        for line in f:
            if line.startswith('v '):
                vs.append([float(x) for x in line.split()[1:4]])
            elif line.startswith('f '):
                fs.append([int(x.split('/')[0]) - 1 for x in line.split()[1:4]])
    return vs, fs


def silhouette(vs, fs, size, margin):
    xs, ys = [v[0] for v in vs], [v[1] for v in vs]
    lo_x, hi_x, lo_y, hi_y = min(xs), max(xs), min(ys), max(ys)
    scale = (size - 2 * margin) / max(hi_x - lo_x, hi_y - lo_y)
    off_x = (size - (hi_x - lo_x) * scale) / 2
    off_y = (size - (hi_y - lo_y) * scale) / 2
    mask = Image.new('L', (size, size), 0)
    draw = ImageDraw.Draw(mask)
    for f in fs:
        pts = [((vs[i][0] - lo_x) * scale + off_x, size - ((vs[i][1] - lo_y) * scale + off_y)) for i in f]
        draw.polygon(pts, fill=255)
    return mask


def main():
    source, out = sys.argv[1], sys.argv[2]
    vs, fs = load(source)
    big = SIZE * SUPER
    mask = silhouette(vs, fs, big, 10 * SUPER)
    stroke = 7 * SUPER
    outer = mask.filter(ImageFilter.MaxFilter(stroke // 2 * 2 + 1))
    inner = mask.filter(ImageFilter.MinFilter(stroke // 2 * 2 + 1))
    ring = ImageChops.subtract(outer, inner)
    fill = inner.point(lambda v: v * 70 // 255)
    alpha = ImageChops.lighter(ring, fill).resize((SIZE, SIZE), Image.LANCZOS)
    icon = Image.new('RGBA', (SIZE, SIZE), (255, 255, 255, 0))
    icon.putalpha(alpha)
    icon.save(out)


main()
