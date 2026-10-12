# The Blender end of `sculpt.exe --demo-blender round` (src/app/blender.rs):
# waits for what GolemSculpt sends, works on it as a person would, and sends
# it back. Run in a Blender with no window, on the same folder:
#
#     blender --background --factory-startup --python tests/blender_link/reply.py
#
# with GOLEMSCULPT_LINK_DIR naming the run's scratch folder. What it does to
# what arrives: the first object is made twice as wide by its size in Blender
# (not applied), the second is left exactly as it came, and a new block 30 mm
# across is made, standing on the floor. All of them are sent back.

import importlib.util
import os
import sys
import time

import bpy

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(HERE, "..", "..", "assets", "blender", "golemsculpt_link.py")

if not os.environ.get("GOLEMSCULPT_LINK_DIR"):
    print("FAIL set GOLEMSCULPT_LINK_DIR to the run's scratch folder first")
    sys.exit(1)

spec = importlib.util.spec_from_file_location("golemsculpt_link", SCRIPT)
link = importlib.util.module_from_spec(spec)
spec.loader.exec_module(link)
link.register()

began = time.time()
taken = 0
while not taken and time.time() - began < 150:
    looked = time.time()
    taken = link.take_packages()
    if not taken:
        time.sleep(0.2)
print("taken in in %.2f s" % (time.time() - looked))
if not taken:
    print("FAIL nothing came from GolemSculpt")
    sys.exit(1)

came = [o for o in bpy.data.objects if link.TAG_NAME in o]
scene = bpy.context.scene
print("unit scale %g, %s" % (scene.unit_settings.scale_length, scene.unit_settings.length_unit))
for obj in came:
    print("came: %-24s %8.3f x %8.3f x %8.3f at (%.3f, %.3f, %.3f)%s" % ((obj.name,) + tuple(obj.dimensions) + tuple(obj.location) + (" copy" if obj[link.TAG_COPY] else "",)))

own = [o for o in came if not o[link.TAG_COPY]]
if own:
    own[0].scale.x = 2.0
bpy.ops.mesh.primitive_cube_add(size=30.0, location=(0.0, 0.0, 15.0))
made = bpy.context.active_object
made.name = "Mold Half"
bpy.context.view_layer.update()

began = time.time()
went, said = link.send(came + [made], bpy.context, open_app=False)
print(said)
print("sent in %.2f s" % (time.time() - began))
link.unregister()
sys.exit(0 if went else 1)
