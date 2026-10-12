# Checks the Blender side of the GolemSculpt link (assets/blender/golemsculpt_link.py)
# in a Blender with no window:
#
#     blender --background --factory-startup --python tests/blender_link/check_link.py
#
# It works in the folder GOLEMSCULPT_LINK_DIR names (a scratch one; never the
# user's own), prints a line per check and exits with 1 if any failed.

import importlib.util
import os
import sys

import bpy
import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(HERE, "..", "..", "assets", "blender", "golemsculpt_link.py")

folder = os.environ.get("GOLEMSCULPT_LINK_DIR")
if not folder:
    print("FAIL set GOLEMSCULPT_LINK_DIR to a scratch folder first")
    sys.exit(1)

spec = importlib.util.spec_from_file_location("golemsculpt_link", SCRIPT)
link = importlib.util.module_from_spec(spec)
spec.loader.exec_module(link)
link.register()

failed = 0


def say(what, ok, detail=""):
    global failed
    failed += not ok
    print("%s %s%s" % ("ok  " if ok else "FAIL", what, (" (%s)" % detail) if detail and not ok else ""))


def volume(points, triangles):
    """Signed volume: positive when the faces look outward."""
    a, b, c = (points[triangles[:, i]].astype(np.float64) for i in range(3))
    return float(np.einsum("ij,ij->i", a, np.cross(b, c)).sum() / 6.0)


def world_points(obj):
    mesh = obj.data
    co = np.empty(len(mesh.vertices) * 3, np.float32)
    mesh.vertices.foreach_get("co", co)
    m = np.array(obj.matrix_world, np.float64)
    return co.reshape((-1, 3)).astype(np.float64) @ m[:3, :3].T + m[:3, 3]


def box(centre, half):
    """A box in GolemSculpt's space, faces outward, as triangles."""
    corners = np.array([[x, y, z] for x in (-1, 1) for y in (-1, 1) for z in (-1, 1)], np.float32)
    points = corners * np.array(half, np.float32) + np.array(centre, np.float32)
    quads = [(0, 1, 3, 2), (4, 6, 7, 5), (0, 4, 5, 1), (2, 3, 7, 6), (0, 2, 6, 4), (1, 5, 7, 3)]
    triangles = np.array([t for q in quads for t in ((q[0], q[1], q[2]), (q[0], q[2], q[3]))], np.uint32)
    return points, triangles, quads


def entry(name, ref, centre, half, mark, same_as="", polys=False):
    e = link.Entry(name)
    e.ref, e.mark, e.same_as, e.origin = ref, mark, same_as, centre
    e.points, e.triangles, quads = box(centre, half)
    if polys:
        e.poly_sizes = np.full(len(quads), 4, np.uint32)
        e.poly_corners = np.array(quads, np.uint32).ravel()
    return e


def newest(tray):
    names = sorted(n for n in os.listdir(tray) if n.endswith(link.ENDING))
    return os.path.join(tray, names[-1])


to_blender = os.path.join(folder, "to_blender")
to_sculpt = os.path.join(folder, "to_sculpt")
for tray in (to_blender, to_sculpt):
    os.makedirs(tray, exist_ok=True)
    for name in os.listdir(tray):
        os.remove(os.path.join(tray, name))

scene = bpy.context.scene

# ---- a package comes in

# A box 10 cm wide, 20 tall and 30 deep, its middle 40 cm up; and a flat one kept as quads.
first = os.path.join(to_blender, "1.gslink")
link.write_package(
    first,
    "golemsculpt",
    [
        entry("Tall Box", "run1:5", (0.0, 40.0, 0.0), (5.0, 10.0, 15.0), "s1"),
        entry("Slab", "run1:9", (20.0, 0.0, 0.0), (4.0, 1.0, 4.0), "s2", polys=True),
    ],
)
header, entries = link.read_package(first)
say("a package reads back as it was written", [e.name for e in entries] == ["Tall Box", "Slab"] and entries[1].poly_sizes is not None)
said = link.receive(first)
tall, slab = bpy.data.objects.get("Tall Box"), bpy.data.objects.get("Slab")
say("both objects arrive", tall is not None and slab is not None, said)
units = scene.unit_settings
say("a new file is set to millimetres", abs(units.scale_length - 0.001) < 1e-9 and units.length_unit == "MILLIMETERS", said)
dims = tuple(round(v, 3) for v in tall.dimensions)
say("at its real size: 100 x 300 x 200 mm, Z up", dims == (100.0, 300.0, 200.0), str(dims))
at = tuple(round(v, 3) for v in tall.location)
say("standing where it stood: 400 mm up", at == (0.0, 0.0, 400.0), str(at))
say("with no turn or scale of its own", tuple(tall.scale) == (1.0, 1.0, 1.0) and tuple(tall.rotation_euler) == (0.0, 0.0, 0.0))
tris = np.empty(len(tall.data.loops), np.int32)
tall.data.loops.foreach_get("vertex_index", tris)
say("faces outward", volume(world_points(tall), tris.reshape((-1, 3))) > 0)
say("the quads came as quads", len(slab.data.polygons) == 6 and all(len(p.vertices) == 4 for p in slab.data.polygons))
say("in the GolemSculpt collection", tall.name in bpy.data.collections[link.COLLECTION].objects)
say("marked as GolemSculpt's", tall[link.TAG_REF] == "run1:5" and tall[link.TAG_MARK] == "s1" and tall[link.TAG_PRINT] != "")

# ---- and goes back

went, said = link.send([tall, slab], bpy.context, open_app=False)
header, back = link.read_package(newest(to_sculpt))
say("unchanged objects say so on the way back", went and [e.same_as for e in back] == ["s1", "s2"], said)
say("under the names and refs they came with", [(e.sent_as, e.ref) for e in back] == [("Tall Box", "run1:5"), ("Slab", "run1:9")])
sent, _, _ = box((0.0, 40.0, 0.0), (5.0, 10.0, 15.0))
drift = float(np.abs(np.sort(back[0].points, axis=0) - np.sort(sent, axis=0)).max())
say("and come back where they were, to a hundredth of a micron", drift < 1e-5, "%g cm" % drift)
say("faces still outward", volume(back[0].points, back[0].triangles.astype(np.int64)) > 0)

# Moved 50 mm along Blender's Y (GolemSculpt's -Z), and given a modifier.
tall.location.y += 50.0
tall.modifiers.new("Thicken", "SOLIDIFY").thickness = 2.0
bpy.context.view_layer.update()
went, said = link.send([tall], bpy.context, open_app=False)
header, back = link.read_package(newest(to_sculpt))
moved = back[0]
say("a changed object goes as new", went and moved.same_as == "" and moved.mark not in ("", "s1"), said)
middle = (moved.points.max(axis=0) + moved.points.min(axis=0)) / 2
say("where Blender has it now: 5 cm toward -Z", np.allclose(middle, (0.0, 40.0, -5.0), atol=0.11), str(middle))
say("with what its modifier made of it", len(moved.points) > 8)
say("its modifier is remembered as sent", tall.get(link.TAG_BAKED, "") == "Thicken")
say("its centre is where Blender's is", np.allclose(moved.origin, (0.0, 40.0, -5.0), atol=1e-4), str(moved.origin))

# GolemSculpt sends the same thing back untouched: Blender's object, modifier and all, stays.
again = os.path.join(to_blender, "2.gslink")
link.write_package(again, "golemsculpt", [entry("Tall Box", "run1:5", (0.0, 40.0, -5.0), (5.0, 10.0, 15.0), moved.mark, same_as=moved.mark)])
said = link.receive(again)
say("what GolemSculpt did not change is left alone", tall.modifiers["Thicken"].show_viewport and "nothing new" in said, said)

# GolemSculpt sculpts it and sends it: the mesh is replaced, in the same object, and the modifier steps aside.
third = os.path.join(to_blender, "3.gslink")
link.write_package(third, "golemsculpt", [entry("Tall Box", "run1:5", (0.0, 40.0, -5.0), (6.0, 10.0, 15.0), "s3")])
said = link.receive(third)
same_object = bpy.data.objects.get("Tall Box") == tall and len([o for o in bpy.data.objects if o.get(link.TAG_NAME) == "Tall Box"]) == 1
say("a sculpted object is updated in place", same_object and round(tall.dimensions.x, 3) == 120.0, said)
say("and the modifier it already has in it is switched off", not tall.modifiers["Thicken"].show_viewport and "switched off" in said, said)

# ---- something made in Blender

bpy.ops.mesh.primitive_cube_add(size=20.0, location=(0.0, 0.0, 10.0))
made = bpy.context.active_object
made.name = "Mold Half"
went, said = link.send([made], bpy.context, open_app=False)
header, back = link.read_package(newest(to_sculpt))
new = back[0]
say("an object made in Blender goes as a new one", went and (new.name, new.ref, new.sent_as) == ("Mold Half", "", ""), said)
size = new.points.max(axis=0) - new.points.min(axis=0)
say("at its real size: 20 mm is 2 cm", np.allclose(size, (2.0, 2.0, 2.0), atol=1e-5), str(size))
say("standing on the floor, Y up", abs(float(new.points[:, 1].min())) < 1e-5)
say("and is marked to go back as itself next time", made.get(link.TAG_NAME) == "Mold Half")
went, said = link.send([made], bpy.context, open_app=False)
header, back = link.read_package(newest(to_sculpt))
say("which it does", back[0].sent_as == "Mold Half" and back[0].same_as == new.mark, said)

# Turned inside out by a negative size: its faces are turned back.
made.scale.x = -1.0
bpy.context.view_layer.update()
went, said = link.send([made], bpy.context, open_app=False)
header, back = link.read_package(newest(to_sculpt))
say("a mirrored object still faces outward", volume(back[0].points, back[0].triangles.astype(np.int64)) > 0)
say("and says what it last went as", back[0].was == new.mark and back[0].same_as == "")

# GolemSculpt has its own name for it by now, and has reshaped it: it is still
# found, by the mark it last went under, and is not made a second time.
renamed = entry("Lower Mold", "run1:31", (0.0, 1.0, 0.0), (1.0, 1.0, 1.0), "s20")
renamed.was = made[link.TAG_MARK]
fifth = os.path.join(to_blender, "2b.gslink")
link.write_package(fifth, "golemsculpt", [renamed])
count = len(bpy.data.objects)
said = link.receive(fifth)
found = len(bpy.data.objects) == count and made.get(link.TAG_NAME) == "Lower Mold" and made.get(link.TAG_REF) == "run1:31"
say("an object GolemSculpt calls something else is still itself", found and len(made.data.vertices) == 8 and "updated" in said, said)

# ---- a new file that has been worked in keeps its metres

# (Blender only keeps track of whether a file has been worked in while it has
# a window, so without one this is checked by saying so for it.)
bpy.ops.wm.read_factory_settings(use_empty=True)
bpy.ops.mesh.primitive_cube_add(size=2.0)
worked_in = os.path.join(to_blender, "3b.gslink")
link.write_package(worked_in, "golemsculpt", [entry("Tall Box", "run3:5", (0.0, 40.0, 0.0), (5.0, 10.0, 15.0), "s8")])
units = bpy.context.scene.unit_settings
say("a new file that has been worked in is not set to millimetres", not link._millimetres_for_a_new_file(bpy.context.scene, worked_in=True) and abs(units.scale_length - 1.0) < 1e-9)
units.scale_length = 1.0
said = link.receive(worked_in) if bpy.data.is_dirty else ""
if said:
    say("and says nothing of it", "millimetres" not in said, said)

# ---- a file with units of its own is left as it is

bpy.ops.wm.read_factory_settings(use_empty=True)
scene = bpy.context.scene
scene.unit_settings.scale_length = 0.01
fourth = os.path.join(to_blender, "4.gslink")
link.write_package(fourth, "golemsculpt", [entry("Tall Box", "run2:5", (0.0, 40.0, 0.0), (5.0, 10.0, 15.0), "s9")])
said = link.receive(fourth)
tall = bpy.data.objects["Tall Box"]
say("a file whose units were chosen keeps them", abs(scene.unit_settings.scale_length - 0.01) < 1e-9 and "millimetres" not in said, said)
say("and the model is its real size in them: 10 cm is 10 units", round(tall.dimensions.x, 4) == 10.0, str(tuple(tall.dimensions)))

# ---- the tray

for name in os.listdir(to_blender):
    os.remove(os.path.join(to_blender, name))
link.write_package(os.path.join(to_blender, "5.gslink"), "golemsculpt", [entry("Late", "run2:7", (0.0, 0.0, 0.0), (1.0, 1.0, 1.0), "s10")])
old = os.path.join(to_blender, "0.gslink")
link.write_package(old, "golemsculpt", [entry("Stale", "run2:8", (0.0, 0.0, 0.0), (1.0, 1.0, 1.0), "s11")])
long_ago = os.path.getmtime(old) - link.STALE_SECONDS - 60
os.utime(old, (long_ago, long_ago))
taken = link.take_packages()
say("a waiting package is taken in, and the tray emptied", taken == 1 and "Late" in bpy.data.objects and os.listdir(to_blender) == [], str(os.listdir(to_blender)))
say("one left from a session long gone is not", "Stale" not in bpy.data.objects)

say("this Blender's own process counts as running", link.process_is_running(os.getpid()))
say("and a number nothing has does not", not link.process_is_running(0x7FFFFFF0))

link.unregister()
print("%d failed" % failed)
sys.exit(1 if failed else 0)
