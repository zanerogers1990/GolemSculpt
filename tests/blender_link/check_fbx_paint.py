# Reads a painted FBX that GolemSculpt wrote (src/io/fbx.rs, the ignored test
# blender_fbx_check_file) with Blender's own FBX importer, in a Blender with no
# window, and checks what it made of the colour, the materials and the
# material weights in the UV sets:
#
#     blender --background --factory-startup --python tests/blender_link/check_fbx_paint.py
#
# GOLEMSCULPT_PAINT_CHECK is the folder painted.fbx and fbx_expect.json are in.
# Prints a line per check; exits with 1 if any failed.

import json
import os
import sys

import bpy
import numpy as np

check = os.environ.get("GOLEMSCULPT_PAINT_CHECK")
if not check:
    print("FAIL set GOLEMSCULPT_PAINT_CHECK first")
    sys.exit(1)
with open(os.path.join(check, "fbx_expect.json"), "r", encoding="utf-8") as f:
    expect = json.load(f)

failed = 0


def say(what, ok, detail=""):
    global failed
    failed += not ok
    print("%s %s%s" % ("ok  " if ok else "FAIL", what, (" (%s)" % detail) if detail and not ok else ""))


for obj in list(bpy.data.objects):
    bpy.data.objects.remove(obj)
bpy.ops.import_scene.fbx(filepath=os.path.join(check, "painted.fbx"))
objects = {o.name: o for o in bpy.data.objects if o.type == "MESH"}
say("both places arrive as objects of their own", sorted(objects) == ["Ball", "Ball_MirrorX"], sorted(objects))

seen = np.array(expect["seen"], np.float64) / 255.0
n = expect["vertices"]
for name, obj in sorted(objects.items()):
    mesh = obj.data
    say("%s: every point" % name, len(mesh.vertices) == n)
    colours = mesh.color_attributes
    say("%s: one colour attribute, \"GolemSculpt Paint\"" % name, [a.name for a in colours] == ["GolemSculpt Paint"], [a.name for a in colours])
    if len(colours):
        attribute = colours[0]
        values = np.empty(len(attribute.data) * 4, np.float32)
        attribute.data.foreach_get("color_srgb", values)
        values = values.reshape((-1, 4)).astype(np.float64)
        if attribute.domain == "CORNER":
            corners = np.empty(len(mesh.loops), np.int32)
            mesh.loops.foreach_get("vertex_index", corners)
            want = seen[corners]
        else:
            want = seen
        error = float(np.abs(values[:, :3] - want).max())
        say("%s: each corner the colour of its point, as exported (%s)" % (name, attribute.domain), error <= 0.5 / 255.0 + 1e-6, error)
        say("%s: opaque" % name, bool(np.all(values[:, 3] > 0.999)))
    materials = [m.name for m in mesh.materials]
    say("%s: its materials, in order" % name, materials == expect["materials"], materials)
    faces = np.empty(len(mesh.polygons), np.int32)
    mesh.polygons.foreach_get("material_index", faces)
    say("%s: each face the material most of it is painted with" % name, faces.tolist() == expect["faces"])
    uvs = [u.name for u in mesh.uv_layers]
    sets = len(expect["weights"]) // 2 + len(expect["weights"]) % 2
    say("%s: UV set 0 and the weight sets" % name, len(uvs) == 1 + sets, uvs)
    corners = np.empty(len(mesh.loops), np.int32)
    mesh.loops.foreach_get("vertex_index", corners)
    for s in range(1, min(len(uvs), 1 + sets)):
        layer = mesh.uv_layers[s]
        uv = np.empty(len(layer.data) * 2, np.float32)
        layer.data.foreach_get("uv", uv)
        uv = uv.reshape((-1, 2))
        weights = expect["weights"]
        first = np.array(weights[2 * (s - 1)], np.float32)[corners]
        second = np.array(weights[2 * (s - 1) + 1], np.float32)[corners] if 2 * (s - 1) + 1 < len(weights) else np.zeros(len(corners), np.float32)
        error = float(max(np.abs(uv[:, 0] - first).max(), np.abs(uv[:, 1] - second).max()))
        say("%s: UV set %d holds the weights of \"%s\"" % (name, s, layer.name), error < 1e-6, error)
    if uvs:
        zero = np.empty(len(mesh.uv_layers[0].data) * 2, np.float32)
        mesh.uv_layers[0].data.foreach_get("uv", zero)
        say("%s: UV set 0 is all zero" % name, bool(np.all(zero == 0.0)))

mats = {m.name: m for m in bpy.data.materials}
rust = mats.get("Metal - Rust")
if rust is not None and rust.use_nodes:
    principled = next((n for n in rust.node_tree.nodes if n.type == "BSDF_PRINCIPLED"), None)
    colour = tuple(principled.inputs["Base Color"].default_value)[:3] if principled else ()
    say("the material's colour is its swatch", principled is not None and abs(colour[0] - 1.0) < 1e-4 and colour[1] < 1e-4, colour)
else:
    say("the material's colour is its swatch", rust is not None and abs(rust.diffuse_color[0] - 1.0) < 1e-4)

print("%d failed" % failed)
sys.exit(1 if failed else 0)
