"""Paints a matte teal ZBrush-style matcap: muted grey-teal facing the viewer,
saturated cyan toward the rim, dark blue then warm brown at the very edge
(that edge is what draws the thin lines in grooves), lit from upper left,
no shine. Usage: python make_teal_matcap.py out.png"""
import sys
import numpy as np
from PIL import Image

N = 512
out = sys.argv[1]

def smooth(a, b, x):
    t = np.clip((x - a) / (b - a), 0, 1)
    return t * t * (3 - 2 * t)

def ramp(t, stops):
    t = np.clip(t, 0, 1)
    xs = [s[0] for s in stops]
    cols = np.array([s[1] for s in stops], float)
    return np.stack([np.interp(t, xs, cols[:, c]) for c in range(3)], -1)

# supersample 3x3 for a clean edge
S = 3
g = (np.arange(N * S) + 0.5) / (N * S) * 2 - 1
x, y = np.meshgrid(g, -g)
r2 = x * x + y * y
r = np.sqrt(np.minimum(r2, 1))
z = np.sqrt(np.clip(1 - r2, 0, 1))
n = np.stack([x, y, z], -1)

L = np.array([-0.45, 0.6, 0.66]); L /= np.linalg.norm(L)
ndl = n @ L
diffuse = np.clip((ndl + 0.35) / 1.35, 0, 1)  # wrapped, so the shadow side stays teal

# body: deep teal in shadow up to pale grey-teal in the light
body = ramp(diffuse, [
    (0.00, (4, 20, 34)),
    (0.30, (22, 60, 76)),
    (0.55, (56, 98, 110)),
    (0.78, (84, 122, 130)),
    (1.00, (110, 148, 152)),
])

# saturated cyan band as the surface turns away (stronger where lit)
band = smooth(0.55, 0.86, r) * (1 - smooth(0.87, 0.94, r))
cyan = np.array([92, 178, 200], float)
body = body + (cyan - body) * (band * (0.15 + 0.85 * diffuse))[..., None]

# very edge: dark blue, then a warm brown line
edge = smooth(0.89, 0.955, r)
body = body + (np.array([16, 36, 48.]) - body) * (edge * 0.9)[..., None]
warm = smooth(0.94, 0.985, r)
body = body + (np.array([70, 52, 46.]) - body) * (warm * 0.8)[..., None]

body = np.clip(body, 0, 255)
# outside the disc: keep the edge colour so filtering never pulls in black
img = body.reshape(N, S, N, S, 3).mean((1, 3))
Image.fromarray(np.round(img).astype(np.uint8)).save(out)
print("wrote", out)
