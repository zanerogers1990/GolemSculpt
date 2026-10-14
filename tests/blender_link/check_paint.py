# Checks paint across the Blender link (assets/blender/golemsculpt_link.py) in
# a Blender with no window, against a package GolemSculpt's own code wrote:
#
#     cargo test ... -- --ignored blender_paint_check_package
#     blender --background --factory-startup --python tests/blender_link/check_paint.py
#     cargo test ... -- --ignored blender_paint_check_answer
#
# GOLEMSCULPT_PAINT_CHECK is the folder the package and what it should hold
# (expect.json) are in, and where the answer is left (from_blender.gslink).
# GOLEMSCULPT_LINK_DIR is a scratch link folder (never the user's own). With
# OLD_LINK_SCRIPT naming a copy of the script from before paint, it checks
# that the old reader takes a package with paint as one without, and that
# this one takes an old package. Prints a line per check; exits with 1 if any
# failed.

import importlib.util
import json
import os
import shutil
import sys

import bpy
import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(HERE, "..", "..", "assets", "blender", "golemsculpt_link.py")

check = os.environ.get("GOLEMSCULPT_PAINT_CHECK")
folder = os.environ.get("GOLEMSCULPT_LINK_DIR")
if not check or not folder:
    print("FAIL set GOLEMSCULPT_PAINT_CHECK and GOLEMSCULPT_LINK_DIR to scratch folders first")
    sys.exit(1)


def load(path, name):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


link = load(SCRIPT, "golemsculpt_link")
link.register()

failed = 0


def say(what, ok, detail=""):
    global failed
    failed += not ok
    print("%s %s%s" % ("ok  " if ok else "FAIL", what, (" (%s)" % detail) if detail and not ok else ""))


package = os.path.join(check, "from_sculpt.gslink")
with open(os.path.join(check, "expect.json"), "r", encoding="utf-8") as f:
    expect = json.load(f)
n = expect["vertices"]

# ---- the package as read
header, entries = link.read_package(package)
entry = entries[0]
say("the package reads, its paint whole", not header.get("paint_lost") and entry.paint is not None)
paint = entry.paint or {"colours": [], "layers": [], "materials": []}
say("the header is still format 1", header.get("format") == 1)
want = np.array(expect["composite"]["colours"], np.uint8)
say("the composite, byte for byte", np.array_equal(paint["colours"], want) and paint["composite"] == "GolemSculpt Paint")
layers_ok = len(paint["layers"]) == len(expect["layers"]) and all(
    got["attribute"] == w["attribute"] and np.array_equal(got["colours"], np.array(w["colours"], np.uint8))
    for got, w in zip(paint["layers"], expect["layers"])
)
say("each layer, byte for byte", layers_ok, [l["attribute"] for l in paint["layers"]])
materials_ok = len(paint["materials"]) == len(expect["materials"]) and all(
    got["attribute"] == w["attribute"] and np.array_equal(got["weights"], np.array(w["weights"], np.float32))
    for got, w in zip(paint["materials"], expect["materials"])
)
say("each material's weights, to the bit", materials_ok)

# ---- taken into the scene
to_blender = os.path.join(folder, "to_blender")
to_sculpt = os.path.join(folder, "to_sculpt")
for tray in (to_blender, to_sculpt):
    os.makedirs(tray, exist_ok=True)
    for name in os.listdir(tray):
        os.remove(os.path.join(tray, name))
taken = os.path.join(to_blender, "1-1.gslink")
shutil.copy(package, taken)
text = link.receive(taken)
print("     " + text)
obj = bpy.data.objects.get("Painted Ball")
say("the object arrived", obj is not None and len(obj.data.vertices) == n)
mesh = obj.data
colours = mesh.color_attributes
names = [a.name for a in colours]
want_names = ["GolemSculpt Paint"] + [l["attribute"] for l in expect["layers"]]
say("a colour attribute for the composite and for each layer", sorted(names) == sorted(want_names), names)
say(
    "on the points, as bytes",
    all(colours[name].domain == "POINT" and colours[name].data_type == "BYTE_COLOR" for name in names),
)


def bytes_of(attribute):
    values = np.empty(len(attribute.data) * 4, np.float32)
    attribute.data.foreach_get("color_srgb", values)
    return np.rint(values.reshape((-1, 4)) * 255.0).astype(np.uint8)


say("the composite holds exactly what was sent", np.array_equal(bytes_of(colours["GolemSculpt Paint"]), want))
for layer in expect["layers"]:
    got = bytes_of(colours[layer["attribute"]])
    say("layer \"%s\" holds exactly what was sent" % layer["attribute"], np.array_equal(got, np.array(layer["colours"], np.uint8)))
for m in expect["materials"]:
    attribute = mesh.attributes.get(m["attribute"])
    ok = attribute is not None and attribute.domain == "POINT" and attribute.data_type == "FLOAT"
    if ok:
        values = np.empty(len(attribute.data), np.float32)
        attribute.data.foreach_get("value", values)
        ok = np.array_equal(values, np.array(m["weights"], np.float32))
    say("material \"%s\" is a number on each point, as sent" % m["attribute"], ok)
active = getattr(colours, "active_color", None)
say("the composite is the colour that shows", active is not None and active.name == "GolemSculpt Paint")
say("and the one that renders", list(colours)[colours.render_color_index].name == "GolemSculpt Paint")
say("the line says where the paint is", "GolemSculpt Paint" in text)

# ---- sent back untouched: it says so
bpy.context.view_layer.update()
went, said = link.send([obj], open_app=False)
back_header, back = link.read_package(os.path.join(to_sculpt, sorted(os.listdir(to_sculpt))[-1]))
say("sent back untouched, it is the same", went and back[0].same_as == "s-check-1", (back[0].same_as, back[0].mark))
say("and its colour goes with it", back[0].paint is not None and np.array_equal(back[0].paint["colours"], want))
for name in os.listdir(to_sculpt):
    os.remove(os.path.join(to_sculpt, name))

# ---- painted in Blender: ten points yellow, and sent back
composite = colours["GolemSculpt Paint"]
values = bytes_of(composite).astype(np.float32) / 255.0
yellow = list(range(0, n, 37))[:10]
values[yellow] = np.array([250, 240, 0, 255], np.float32) / 255.0
composite.data.foreach_set("color_srgb", values.ravel())
mesh.update()
went, said = link.send([obj], open_app=False)
print("     " + said)
answers = sorted(os.listdir(to_sculpt))
answer = os.path.join(to_sculpt, answers[-1])
back_header, back = link.read_package(answer)
say("painted, it is changed", went and back[0].same_as == "" and back[0].was == "s-check-1")
got = back[0].paint["colours"] if back[0].paint else np.zeros((0, 4), np.uint8)
changed = np.nonzero(np.any(got != want, axis=1))[0].tolist()
say("only the ten painted points differ", changed == yellow, changed)
say("each the yellow it was painted", all(list(got[v]) == [250, 240, 0, 255] for v in yellow))
shutil.copy(answer, os.path.join(check, "from_blender.gslink"))

# ---- colour on face corners, as a colour attribute made in Blender often is
bpy.ops.mesh.primitive_cube_add(size=2.0)
cube = bpy.context.active_object
corner = cube.data.color_attributes.new("Col", "FLOAT_COLOR", "CORNER")
values = np.zeros((len(corner.data), 4), np.float32)
values[:, 3] = 1.0
loops = np.empty(len(cube.data.loops), np.int32)
cube.data.loops.foreach_get("vertex_index", loops)
values[loops == 0, 0] = 1.0  # every corner of point 0 red
corner.data.foreach_set("color_srgb", values.ravel())
cube.data.color_attributes.active_color = corner
found = link._colours_of(cube.data)
say(
    "colour on the corners goes as each point's mean",
    found is not None and found[0] == "Col" and list(found[1][0]) == [255, 0, 0, 255] and list(found[1][1]) == [0, 0, 0, 255],
)
bpy.data.objects.remove(cube)

# ---- an older copy of the script, from before paint
old_script = os.environ.get("OLD_LINK_SCRIPT")
if old_script:
    old = load(old_script, "golemsculpt_link_old")
    old_header, old_entries = old.read_package(package)
    same = len(old_entries) == len(entries) and all(
        np.array_equal(a.points, b.points) and np.array_equal(a.triangles, b.triangles) for a, b in zip(old_entries, entries)
    )
    say("the script from before paint reads a package with paint, its shapes whole", same)
    plain = os.path.join(check, "old.gslink")
    e = old.Entry("Old")
    e.points, e.triangles = entries[0].points, entries[0].triangles
    old.write_package(plain, "blender", [e])
    new_header, new_entries = link.read_package(plain)
    say("a package from before paint reads here, with no paint", new_entries[0].paint is None and not new_header.get("paint_lost"))
else:
    print("     (OLD_LINK_SCRIPT not set: the older reader was not checked)")

link.unregister()
print("%d failed" % failed)
sys.exit(1 if failed else 0)
