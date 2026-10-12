# For `sculpt.exe --demo-blender live` (src/app/blender.rs): makes the Blender
# that GolemSculpt starts answer by itself and close, so the whole road can be
# checked with nobody at the screen. It stands in for the person: once what
# GolemSculpt sent is in the scene, it makes the first object twice as wide,
# presses Send to GolemSculpt on everything that came, and quits.
#
# It goes in the `startup` folder of a scripts folder of the run's own (named
# by BLENDER_USER_SCRIPTS), never the user's: tests/blender_link/README.md.

import os
import time

import bpy

_began = time.time()
_log = os.path.join(os.environ.get("GOLEMSCULPT_LINK_DIR", "."), "answer.log")


def _note(text):
    with open(_log, "a", encoding="utf-8") as f:
        f.write(text + "\n")


def _quit():
    bpy.ops.wm.quit_blender()
    return None


def _answer():
    came = [o for o in bpy.data.objects if "golemsculpt_name" in o]
    if not came:
        if time.time() - _began > 120:
            _note("FAIL nothing came from GolemSculpt in two minutes")
            return _quit()
        return 0.5
    window = bpy.context.window_manager.windows[0]
    scene = window.scene
    _note("unit scale %g, %s" % (scene.unit_settings.scale_length, scene.unit_settings.length_unit))
    for obj in came:
        _note("came: %s %.3f x %.3f x %.3f" % ((obj.name,) + tuple(obj.dimensions)))
    _note("the note that says Blender is open: %s" % os.path.isfile(os.path.join(os.environ["GOLEMSCULPT_LINK_DIR"], "blender-%d.json" % os.getpid())))
    _note("the panel is there: %s" % hasattr(bpy.types, "GOLEMSCULPT_PT_link"))
    own = [o for o in came if not o["golemsculpt_copy"]]
    own[0].scale.x = 2.0
    for obj in window.view_layer.objects:
        obj.select_set(obj in came)
    area = next(a for a in window.screen.areas if a.type == "VIEW_3D")
    region = next(r for r in area.regions if r.type == "WINDOW")
    with bpy.context.temp_override(window=window, area=area, region=region):
        result = bpy.ops.golemsculpt.send()
    _note("the button: %s" % (result,))
    bpy.app.timers.register(_quit, first_interval=2.0)
    return None


def register():
    if not bpy.app.background:
        bpy.app.timers.register(_answer, first_interval=1.0, persistent=True)


def unregister():
    pass
