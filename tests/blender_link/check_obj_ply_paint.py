# Reads a painted OBJ and PLY that GolemSculpt wrote (src/io/painted.rs, the
# ignored test blender_obj_ply_check_files) with Blender's own importers, in a
# Blender with no window, and checks each point's colour:
#
#     blender --background --factory-startup --python tests/blender_link/check_obj_ply_paint.py
#
# GOLEMSCULPT_PAINT_CHECK is the folder the files and obj_ply_expect.json are
# in. Prints a line per check; exits with 1 if any failed.

import json
import os
import sys

import bpy
import numpy as np

check = os.environ.get("GOLEMSCULPT_PAINT_CHECK")
if not check:
    print("FAIL set GOLEMSCULPT_PAINT_CHECK first")
    sys.exit(1)
with open(os.path.join(check, "obj_ply_expect.json"), "r", encoding="utf-8") as f:
    expect = json.load(f)
seen = np.array(expect["seen"], np.float64) / 255.0

failed = 0


def say(what, ok, detail=""):
    global failed
    failed += not ok
    print("%s %s%s" % ("ok  " if ok else "FAIL", what, (" (%s)" % detail) if detail and not ok else ""))


def per_point(mesh, attribute):
    values = np.empty(len(attribute.data) * 4, np.float32)
    attribute.data.foreach_get("color_srgb", values)
    values = values.reshape((-1, 4)).astype(np.float64)
    if attribute.domain == "CORNER":
        corners = np.empty(len(mesh.loops), np.int32)
        mesh.loops.foreach_get("vertex_index", corners)
        out = np.zeros((len(mesh.vertices), 4))
        out[corners] = values
        return out
    return values


for kind, load in (("OBJ", bpy.ops.wm.obj_import), ("PLY", bpy.ops.wm.ply_import)):
    for obj in list(bpy.data.objects):
        bpy.data.objects.remove(obj)
    load(filepath=os.path.join(check, "painted." + kind.lower()))
    meshes = [o for o in bpy.data.objects if o.type == "MESH"]
    say("%s: one object" % kind, len(meshes) == 1)
    if not meshes:
        continue
    mesh = meshes[0].data
    say("%s: every point" % kind, len(mesh.vertices) == expect["vertices"], len(mesh.vertices))
    colours = list(mesh.color_attributes)
    say("%s: a colour attribute" % kind, len(colours) == 1, [c.name for c in colours])
    if colours and len(mesh.vertices) == expect["vertices"]:
        got = per_point(mesh, colours[0])
        error = float(np.abs(got[:, :3] - seen).max())
        say("%s: each point the colour it was exported with (%s)" % (kind, colours[0].domain), error <= 0.5 / 255.0 + 1e-6, error)

print("%d failed" % failed)
sys.exit(1 if failed else 0)
