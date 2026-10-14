# GolemSculpt link: sends objects between Blender and GolemSculpt, the
# sculpting app, at their real size. GolemSculpt keeps this file and puts it
# in Blender itself (see src/app/blender.rs); there is nothing to install by
# hand.
#
# How the two programs meet: a folder both know, `blender_link` in
# GolemSculpt's settings folder. Whichever program sends writes one package
# into the other's tray there (`to_blender`, `to_sculpt`), and the other looks
# in its tray twice a second. Nothing listens on a port and nothing is sent
# anywhere else.
#
# A package is one file, all little-endian:
#
#     "GSLINK01"            8 bytes
#     length of the header  u32
#     the header            JSON, UTF-8
#     then, for each object of the header, in order:
#         its points        vertices x 3 x f32
#         its triangles     triangles x 3 x u32
#         and, where "polys" is not 0, its faces as they were before they were
#         cut into triangles:
#             their sizes   polys x u32
#             their corners poly_corners x u32
#     then, only if some object of the header has "paint":
#         "GSPAINT1"        8 bytes
#         for each object that has "paint", in order:
#             the composite      vertices x 4 x u8   (sRGB red, green, blue, alpha)
#             each layer listed  vertices x 4 x u8   (sRGB red, green, blue, coverage)
#             each material      vertices x f32      (its weight, 0 to 1)
#
# The paint rides at the end, after every object, where a copy of this script
# from before paint (which reads its objects and stops) never looks, so the
# format is still 1. An object's "paint" in the header names the attributes:
# {"composite": "GolemSculpt Paint", "layers": [{"name", "attribute",
# "visible", "opacity"}], "materials": [{"name", "attribute", "slot"}]}. Here
# each becomes an attribute on the points: the composite and each layer a
# colour attribute (the composite is the active one, which the viewport's
# Attribute colour shows), each material a number attribute (an Attribute
# node of its name reads it in a shader). Going back, only the composite goes:
# "GolemSculpt Paint" if the object has it, else its active colour attribute.
#
# Everything in a package is in GolemSculpt's own terms whichever way it is
# going: Y up, right-handed, 1 unit = 1 cm, and every point where it stands in
# the scene (an object's turn and size are in its points). `src/io/link.rs`
# reads and writes the same thing and says the same.
#
# Each object of a package has a "mark", a name for that sending of it. Both
# programs remember the mark of what they last exchanged for an object, and
# what the object was like then; one that has not changed since goes with
# "same_as" set to that mark, and the other side, if it still has the object,
# leaves its own alone: it is at least as new. So sending a whole selection
# back and forth only ever replaces what was really worked on, and work done
# on one side is not undone by the other sending back what it was given.
# Changed or not, an object also goes with "was", the mark of what was last
# exchanged for it: no two marks are alike, so that says exactly which object
# it is on the other side, whatever each side has come to call it.
#
# In Blender that is Z up and whatever length the scene's units give a Blender
# unit, so a model is the same real size in both programs: (x, y, z) cm there
# is (x, -z, y) * 0.01 / unit scale here. A quarter turn, not a mirror, so the
# faces keep facing out.

bl_info = {
    "name": "GolemSculpt Link",
    "author": "GolemSculpt",
    "version": (1, 1, 0),
    "blender": (3, 6, 0),
    "location": "3D Viewport > Sidebar > GolemSculpt",
    "description": "Send objects to GolemSculpt and take them back, at their real size",
    "category": "Import-Export",
}

import atexit
import hashlib
import json
import os
import struct
import subprocess
import sys
import time
import traceback

import bpy
import numpy as np
from mathutils import Matrix

# What this copy of the script is; GolemSculpt replaces an older one.
LINK_VERSION = 2
MODULE = "golemsculpt_link"
MAGIC = b"GSLINK01"
FORMAT = 1
# What begins the paint at the end of a package.
PAINT_MAGIC = b"GSPAINT1"
# The colour attribute that is the paint as GolemSculpt shows it.
COMPOSITE = "GolemSculpt Paint"
ENDING = ".gslink"
# A package nobody took in this long was meant for a session that has gone.
STALE_SECONDS = 600.0
# How often the tray is looked at.
TICK_SECONDS = 0.5
COLLECTION = "GolemSculpt"

# What came from GolemSculpt is marked, so it can go back as itself.
TAG_NAME = "golemsculpt_name"
TAG_REF = "golemsculpt_ref"
TAG_COPY = "golemsculpt_copy"
# The mark of what was last exchanged for the object, and its fingerprint then.
TAG_MARK = "golemsculpt_mark"
TAG_PRINT = "golemsculpt_print"
# The modifiers that were part of the shape GolemSculpt was last sent.
TAG_BAKED = "golemsculpt_baked"

SENDABLE = {"MESH", "CURVE", "SURFACE", "FONT", "META"}

# The line the panel shows, and when it was written.
_said = {"text": "", "at": 0.0}


# ---- the folder


def link_dir():
    """The folder both programs know: GolemSculpt's settings folder says where."""
    given = os.environ.get("GOLEMSCULPT_LINK_DIR")
    if given:
        return given
    settings = os.environ.get("SCULPT_SETTINGS_DIR")
    if not settings:
        if sys.platform == "win32":
            settings = os.path.join(os.environ.get("APPDATA", os.path.expanduser("~")), "Sculpt")
        else:
            config = os.environ.get("XDG_CONFIG_HOME") or os.path.join(os.path.expanduser("~"), ".config")
            settings = os.path.join(config, "sculpt")
    return os.path.join(settings, "blender_link")


def _tray(name):
    path = os.path.join(link_dir(), name)
    os.makedirs(path, exist_ok=True)
    return path


def _read_json(path):
    try:
        with open(path, "r", encoding="utf-8") as f:
            return json.load(f)
    except (OSError, ValueError):
        return None


def _write_whole(path, write):
    """Writes a file beside `path` and swaps it in, so nobody reads half of one."""
    temp = "%s.%d.tmp" % (path, os.getpid())
    with open(temp, "wb") as f:
        write(f)
    os.replace(temp, path)


def _write_json(path, value):
    _write_whole(path, lambda f: f.write(json.dumps(value).encode("utf-8")))


def process_is_running(pid):
    if sys.platform != "win32":
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return False
        except OSError:
            return True
        return True
    # Never os.kill here: on Windows it ends the process it is asked about.
    import ctypes
    from ctypes import wintypes

    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.OpenProcess.restype = wintypes.HANDLE
    kernel.OpenProcess.argtypes = (wintypes.DWORD, wintypes.BOOL, wintypes.DWORD)
    kernel.GetExitCodeProcess.argtypes = (wintypes.HANDLE, ctypes.POINTER(wintypes.DWORD))
    kernel.CloseHandle.argtypes = (wintypes.HANDLE,)
    process = kernel.OpenProcess(0x1000, False, int(pid))
    if not process:
        # Refused: it is there, but another account's.
        return ctypes.get_last_error() == 5
    code = wintypes.DWORD(0)
    known = kernel.GetExitCodeProcess(process, ctypes.byref(code))
    kernel.CloseHandle(process)
    return not known or code.value == 259


# ---- packages


class Entry:
    """One object of a package."""

    def __init__(self, name):
        self.name = name
        # The object of GolemSculpt's this one is, as GolemSculpt wrote it.
        self.ref = ""
        # The name it went to Blender under, on its way back.
        self.sent_as = ""
        # A live mirror or array copy, which GolemSculpt draws itself.
        self.copy = False
        self.origin = (0.0, 0.0, 0.0)
        self.mark = ""
        # Set when the object is as it was when this mark was exchanged.
        self.same_as = ""
        # The mark of what was last exchanged for it, changed since or not.
        self.was = ""
        self.points = np.zeros((0, 3), np.float32)
        self.triangles = np.zeros((0, 3), np.uint32)
        self.poly_sizes = None
        self.poly_corners = None
        # Its paint: {"composite": name, "colours": (vertices, 4) uint8, "layers": [{"name",
        # "attribute", "visible", "opacity", "colours"}], "materials": [{"name", "attribute",
        # "slot", "weights": (vertices,) float32}]}, or None.
        self.paint = None


def _paint_fits(paint, count):
    """Whether every list of an object's paint has one entry per point."""
    if not paint:
        return False
    sound = len(paint["colours"]) == count
    sound = sound and all(len(layer["colours"]) == count for layer in paint.get("layers", []))
    return sound and all(len(m["weights"]) == count for m in paint.get("materials", []))


def write_package(path, sender, entries, smooth=True):
    objects = []
    # Paint that does not fit its object is not written at all: the header and the end must agree.
    painted = [e.paint if _paint_fits(e.paint, len(e.points)) else None for e in entries]
    for e, paint in zip(entries, painted):
        polys = 0 if e.poly_sizes is None else len(e.poly_sizes)
        o = {
            "name": e.name,
            "ref": e.ref,
            "sent_as": e.sent_as,
            "copy": bool(e.copy),
            "origin": [float(v) for v in e.origin],
            "mark": e.mark,
            "same_as": e.same_as,
            "was": e.was,
            "vertices": int(len(e.points)),
            "triangles": int(len(e.triangles)),
            "polys": int(polys),
            "poly_corners": int(len(e.poly_corners)) if polys else 0,
        }
        if paint:
            layers = paint.get("layers", [])
            materials = paint.get("materials", [])
            o["paint"] = {
                "composite": paint["composite"],
                "layers": [{k: layer[k] for k in ("name", "attribute", "visible", "opacity")} for layer in layers],
                "materials": [{k: m[k] for k in ("name", "attribute", "slot")} for m in materials],
            }
        objects.append(o)
    header = json.dumps({"format": FORMAT, "from": sender, "smooth": bool(smooth), "objects": objects}).encode("utf-8")

    def write(f):
        f.write(MAGIC)
        f.write(struct.pack("<I", len(header)))
        f.write(header)
        for e, o in zip(entries, objects):
            f.write(np.ascontiguousarray(e.points, dtype="<f4").tobytes())
            f.write(np.ascontiguousarray(e.triangles, dtype="<u4").tobytes())
            if o["polys"]:
                f.write(np.ascontiguousarray(e.poly_sizes, dtype="<u4").tobytes())
                f.write(np.ascontiguousarray(e.poly_corners, dtype="<u4").tobytes())
        if any(p is not None for p in painted):
            f.write(PAINT_MAGIC)
            for paint in painted:
                if paint is None:
                    continue
                f.write(np.ascontiguousarray(paint["colours"], dtype=np.uint8).tobytes())
                for layer in paint.get("layers", []):
                    f.write(np.ascontiguousarray(layer["colours"], dtype=np.uint8).tobytes())
                for material in paint.get("materials", []):
                    f.write(np.ascontiguousarray(material["weights"], dtype="<f4").tobytes())

    _write_whole(path, write)


def _read_paint(f, heads, entries):
    """Reads the paint at the end of a package onto the entries whose header said they had
    some. Returns whether it could; if not, no entry has any (the shapes stand without it)."""
    if not any(heads):
        return True
    if f.read(len(PAINT_MAGIC)) != PAINT_MAGIC:
        return False

    def take(count, kind, width):
        size = count * width * np.dtype(kind).itemsize
        data = f.read(size)
        if len(data) != size:
            raise ValueError("the paint is cut short")
        array = np.frombuffer(data, dtype=kind)
        return array.reshape((count, width)) if width > 1 else array

    read = []
    try:
        for head, e in zip(heads, entries):
            if not head:
                read.append(None)
                continue
            count = len(e.points)
            paint = {"composite": str(head.get("composite") or COMPOSITE), "colours": take(count, np.uint8, 4)}
            paint["layers"] = [
                {
                    "name": str(layer.get("name", "")),
                    "attribute": str(layer.get("attribute", "")),
                    "visible": bool(layer.get("visible", True)),
                    "opacity": int(layer.get("opacity", 255)),
                    "colours": take(count, np.uint8, 4),
                }
                for layer in head.get("layers", [])
            ]
            paint["materials"] = [
                {
                    "name": str(m.get("name", "")),
                    "attribute": str(m.get("attribute", "")),
                    "slot": int(m.get("slot", 0)),
                    "weights": take(count, "<f4", 1),
                }
                for m in head.get("materials", [])
            ]
            read.append(paint)
    except ValueError:
        return False
    for e, paint in zip(entries, read):
        e.paint = paint
    return True


def read_package(path):
    """The header and the objects of a package. Raises ValueError for a file that is not one."""
    with open(path, "rb") as f:
        if f.read(8) != MAGIC:
            raise ValueError("not a GolemSculpt package")
        (length,) = struct.unpack("<I", f.read(4))
        if length > 64 * 1024 * 1024:
            raise ValueError("the package's header is damaged")
        header = json.loads(f.read(length).decode("utf-8"))
        if header.get("format") != FORMAT:
            raise ValueError("the package is from a newer GolemSculpt than this link")

        def block(count, kind, width):
            data = f.read(count * width * 4)
            if len(data) != count * width * 4:
                raise ValueError("the package is cut short")
            array = np.frombuffer(data, dtype=kind)
            return array.reshape((count, width)) if width > 1 else array

        entries = []
        heads = []
        for o in header.get("objects", []):
            paint = o.get("paint")
            heads.append(paint if isinstance(paint, dict) else None)
            e = Entry(str(o.get("name", "Object")))
            e.ref = str(o.get("ref", ""))
            e.sent_as = str(o.get("sent_as", ""))
            e.copy = bool(o.get("copy", False))
            e.origin = tuple(float(v) for v in o.get("origin", (0, 0, 0)))[:3]
            e.mark = str(o.get("mark", ""))
            e.same_as = str(o.get("same_as", ""))
            e.was = str(o.get("was", ""))
            e.points = block(int(o["vertices"]), "<f4", 3)
            e.triangles = block(int(o["triangles"]), "<u4", 3)
            if int(o.get("polys", 0)):
                e.poly_sizes = block(int(o["polys"]), "<u4", 1)
                e.poly_corners = block(int(o["poly_corners"]), "<u4", 1)
            count = len(e.points)
            if len(e.triangles) and int(e.triangles.max()) >= count:
                raise ValueError("the package names points it does not hold")
            if e.poly_sizes is not None:
                sound = (
                    int(e.poly_sizes.sum()) == len(e.poly_corners)
                    and (len(e.poly_corners) == 0 or int(e.poly_corners.max()) < count)
                    and (len(e.poly_sizes) == 0 or int(e.poly_sizes.min()) >= 3)
                )
                if not sound:
                    e.poly_sizes = e.poly_corners = None
            entries.append(e)
        header["paint_lost"] = not _read_paint(f, heads, entries)
        return header, entries


# ---- the two spaces


def _per_cm(scene):
    """How many Blender units a centimetre is in this scene."""
    length = scene.unit_settings.scale_length
    return 0.01 / (length if length > 1e-12 else 1.0)


def _to_blender(points, per_cm):
    out = np.empty((len(points), 3), np.float64)
    out[:, 0] = points[:, 0]
    out[:, 1] = -points[:, 2].astype(np.float64)
    out[:, 2] = points[:, 1]
    out *= per_cm
    return out


def _to_sculpt(points, per_cm):
    out = np.empty((len(points), 3), np.float64)
    out[:, 0] = points[:, 0]
    out[:, 1] = points[:, 2]
    out[:, 2] = -points[:, 1]
    out /= per_cm
    return out


# ---- paint on a mesh


def _srgb_to_linear(c):
    return np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)


def _linear_to_srgb(c):
    c = np.clip(c, 0.0, None)
    return np.where(c <= 0.0031308, c * 12.92, 1.055 * np.power(c, 1.0 / 2.4) - 0.055)


def _set_colours(attribute, colours):
    """Puts sRGB bytes (points x 4) into a colour attribute. A byte colour keeps
    them exactly: it holds sRGB bytes itself."""
    values = np.ascontiguousarray(colours, np.float32).ravel() / 255.0
    try:
        attribute.data.foreach_set("color_srgb", values)
    except (AttributeError, TypeError, RuntimeError):
        rgba = values.reshape((-1, 4)).copy()
        rgba[:, :3] = _srgb_to_linear(rgba[:, :3])
        attribute.data.foreach_set("color", rgba.ravel())


def _get_colours(attribute):
    """A colour attribute's colours as sRGB numbers from 0 to 1 (count x 4)."""
    values = np.empty(len(attribute.data) * 4, np.float32)
    try:
        attribute.data.foreach_get("color_srgb", values)
        return values.reshape((-1, 4))
    except (AttributeError, TypeError, RuntimeError):
        attribute.data.foreach_get("color", values)
        rgba = values.reshape((-1, 4))
        rgba[:, :3] = _linear_to_srgb(rgba[:, :3])
        return rgba


def _paint_attribute(mesh):
    """The colour attribute that goes to GolemSculpt: "GolemSculpt Paint", else the active one."""
    attributes = getattr(mesh, "color_attributes", None)
    if attributes is None:
        return None
    found = attributes.get(COMPOSITE)
    if found is None:
        found = getattr(attributes, "active_color", None)
    if found is None or found.domain not in ("POINT", "CORNER"):
        return None
    return found


def _colours_of(mesh):
    """The colour that goes to GolemSculpt, one per point as sRGB bytes, and
    the name of the attribute it is; None for a mesh with none. Colour on the
    corners of faces is the mean of each point's corners."""
    attribute = _paint_attribute(mesh)
    if attribute is None:
        return None
    values = _get_colours(attribute)
    count = len(mesh.vertices)
    if attribute.domain == "CORNER":
        corners = np.empty(len(mesh.loops), np.int32)
        mesh.loops.foreach_get("vertex_index", corners)
        shares = np.maximum(np.bincount(corners, minlength=count), 1).astype(np.float64)
        values = np.stack([np.bincount(corners, weights=values[:, k], minlength=count) / shares for k in range(4)], axis=1)
    if len(values) != count:
        return None
    return attribute.name, np.clip(np.rint(values * 255.0), 0, 255).astype(np.uint8)


def _put_paint(mesh, paint):
    """Gives a mesh made from a package its paint: each colour an attribute on
    the points, each material's weight a number attribute, and the composite
    the colour that shows and renders. Returns whether it had any."""
    if not paint or not _paint_fits(paint, len(mesh.vertices)):
        return False
    attributes = mesh.color_attributes
    composite = attributes.new(paint["composite"] or COMPOSITE, "BYTE_COLOR", "POINT")
    _set_colours(composite, paint["colours"])
    name = composite.name
    for layer in paint.get("layers", []):
        made = attributes.new(layer["attribute"] or layer["name"] or "Paint", "BYTE_COLOR", "POINT")
        _set_colours(made, layer["colours"])
    for m in paint.get("materials", []):
        made = mesh.attributes.new(m["attribute"] or m["name"] or "Material", "FLOAT", "POINT")
        made.data.foreach_set("value", np.ascontiguousarray(m["weights"], np.float32))
    # Found again by name: making attributes can move the ones made before.
    try:
        attributes.active_color = attributes.get(name)
    except (AttributeError, TypeError):
        pass
    for k, attribute in enumerate(attributes):
        if attribute.name == name:
            try:
                attributes.render_color_index = k
            except (AttributeError, TypeError):
                pass
    return True


# ---- the scene the link works in


def _window():
    windows = bpy.context.window_manager.windows
    return bpy.context.window or (windows[0] if windows else None)


def _scene_and_layer():
    window = _window()
    if window is not None:
        return window.scene, window.view_layer
    return bpy.context.scene, bpy.context.view_layer


def _busy(layer):
    """Whether something is being edited that a new mesh must not land on."""
    active = layer.objects.active
    return active is not None and active.mode != "OBJECT"


def _collection(scene):
    found = bpy.data.collections.get(COLLECTION)
    if found is None:
        found = bpy.data.collections.new(COLLECTION)
    if found is not scene.collection and found not in scene.collection.children_recursive:
        scene.collection.children.link(found)
    return found


def _millimetres_for_a_new_file(scene, worked_in=None):
    """A file nobody has worked in yet is set up for work at the size of a
    sculpt: a Blender unit is a millimetre, so sizes read in mm and an STL
    written from here is the size a slicer expects. A saved file, one that has
    been worked in, or one whose units were chosen is left exactly as it is:
    the model arrives at its real size by whatever units the file has."""
    units = scene.unit_settings
    untouched = units.system == "METRIC" and abs(units.scale_length - 1.0) < 1e-9 and units.length_unit == "METERS"
    worked_in = bpy.data.is_dirty if worked_in is None else worked_in
    in_use = bpy.data.filepath or worked_in or any(TAG_NAME in o for o in bpy.data.objects)
    if in_use or not untouched:
        return False
    units.scale_length = 0.001
    units.length_unit = "MILLIMETERS"
    for screen in bpy.data.screens:
        for area in screen.areas:
            for space in area.spaces:
                if space.type == "VIEW_3D":
                    space.clip_start = 0.5
                    space.clip_end = 10000.0
                    space.overlay.grid_scale = 0.001
    return True


def _frame(objects):
    """Brings what just arrived into view."""
    if not objects:
        return
    for window in bpy.context.window_manager.windows:
        for area in window.screen.areas:
            if area.type != "VIEW_3D":
                continue
            region = next((r for r in area.regions if r.type == "WINDOW"), None)
            if region is None:
                continue
            try:
                with bpy.context.temp_override(window=window, area=area, region=region):
                    bpy.ops.view3d.view_selected()
            except Exception:
                pass


def _redraw():
    for window in bpy.context.window_manager.windows:
        for area in window.screen.areas:
            if area.type == "VIEW_3D":
                area.tag_redraw()


def _say(text):
    _said["text"] = text
    _said["at"] = time.time()
    print("GolemSculpt link: " + text)
    if not bpy.app.background:
        _redraw()


def _listed(names):
    names = ['"%s"' % n for n in names]
    if len(names) <= 1:
        return "".join(names)
    if len(names) <= 3:
        return ", ".join(names[:-1]) + " and " + names[-1]
    return "%s, %s and %d more" % (names[0], names[1], len(names) - 2)


# ---- what a mesh is, for telling whether it changed


def _fingerprint(obj, scene):
    """A mark of an object's shape, place and the colour that goes with it. Empty for one
    whose shape is not simply its mesh (it has modifiers or shape keys): that always counts
    as changed."""
    if obj.type != "MESH" or len(obj.modifiers) or obj.data.shape_keys is not None:
        return ""
    mesh = obj.data
    mark = hashlib.blake2b(digest_size=16)
    points = np.empty(len(mesh.vertices) * 3, np.float32)
    mesh.vertices.foreach_get("co", points)
    mark.update(points.tobytes())
    corners = np.empty(len(mesh.loops), np.int32)
    mesh.loops.foreach_get("vertex_index", corners)
    mark.update(corners.tobytes())
    starts = np.empty(len(mesh.polygons), np.int32)
    mesh.polygons.foreach_get("loop_start", starts)
    mark.update(starts.tobytes())
    mark.update(np.array(obj.matrix_world, np.float32).tobytes())
    mark.update(struct.pack("<d", scene.unit_settings.scale_length))
    # Painting in Blender changes the object as much as sculpting does.
    colours = _colours_of(mesh)
    if colours is not None:
        mark.update(colours[0].encode("utf-8"))
        mark.update(colours[1].tobytes())
    return mark.hexdigest()


# ---- taking a package in


def _build_mesh(name, points, entry, smooth):
    mesh = bpy.data.meshes.new(name)
    if entry.poly_sizes is not None:
        sizes = entry.poly_sizes.astype(np.int32)
        corners = entry.poly_corners.astype(np.int32)
    else:
        sizes = np.full(len(entry.triangles), 3, np.int32)
        corners = entry.triangles.astype(np.int32).ravel()
    starts = np.zeros(len(sizes), np.int32)
    if len(sizes) > 1:
        np.cumsum(sizes[:-1], out=starts[1:])
    mesh.vertices.add(len(points))
    mesh.vertices.foreach_set("co", np.ascontiguousarray(points, np.float32).ravel())
    mesh.loops.add(len(corners))
    mesh.loops.foreach_set("vertex_index", corners)
    mesh.polygons.add(len(sizes))
    mesh.polygons.foreach_set("loop_start", starts)
    if bpy.app.version < (4, 0, 0):
        mesh.polygons.foreach_set("loop_total", sizes)
    mesh.update(calc_edges=True)
    if smooth:
        if hasattr(mesh, "shade_smooth"):
            mesh.shade_smooth()
        else:
            mesh.polygons.foreach_set("use_smooth", np.ones(len(sizes), bool))
    return mesh


def _find(entry):
    """The object a package's entry was last time, if it is still here."""
    tagged = [o for o in bpy.data.objects if o.type == "MESH" and TAG_NAME in o]
    if entry.ref:
        for o in tagged:
            if o.get(TAG_REF, "") == entry.ref and o.get(TAG_NAME, "") == entry.name:
                return o
        for o in tagged:
            if o.get(TAG_REF, "") == entry.ref and not entry.copy and not o.get(TAG_COPY, False):
                return o
    # The very thing that was last exchanged, whatever GolemSculpt calls it now.
    if entry.was:
        for o in tagged:
            if o.get(TAG_MARK, "") == entry.was:
                return o
    for o in tagged:
        if o.get(TAG_NAME, "") == entry.name:
            return o
    return None


def receive(path):
    """Brings a package's objects into the scene. Returns the line that says what came."""
    header, entries = read_package(path)
    scene, layer = _scene_and_layer()
    metric = _millimetres_for_a_new_file(scene)
    per_cm = _per_cm(scene)
    smooth = bool(header.get("smooth", True))
    collection = None
    added, updated, same, switched_off, painted = [], [], 0, 0, 0
    for entry in entries:
        if len(entry.triangles) == 0:
            continue
        obj = _find(entry)
        if obj is not None and entry.same_as:
            # GolemSculpt has not touched it since it was last exchanged, so the
            # one here is at least as new, and whatever was done to it here stays.
            same += 1
            continue
        world = _to_blender(entry.points, per_cm)
        origin = _to_blender(np.array([entry.origin], np.float64), per_cm)[0]
        mesh = _build_mesh(entry.name, world - origin, entry, smooth)
        painted += _put_paint(mesh, entry.paint)
        if obj is None:
            obj = bpy.data.objects.new(entry.name, mesh)
            collection = collection or _collection(scene)
            collection.objects.link(obj)
            added.append(obj)
        else:
            old = obj.data
            for material in old.materials:
                mesh.materials.append(material)
            obj.data = mesh
            if old.users == 0:
                bpy.data.meshes.remove(old)
            # The shape GolemSculpt was sent had these in it, so the one it sends back has too.
            for name in str(obj.get(TAG_BAKED, "")).split("\n"):
                modifier = obj.modifiers.get(name) if name else None
                if modifier is not None and (modifier.show_viewport or modifier.show_render):
                    modifier.show_viewport = modifier.show_render = False
                    switched_off += 1
            updated.append(obj)
        if TAG_BAKED in obj:
            del obj[TAG_BAKED]
        obj.matrix_world = Matrix.Translation(tuple(origin))
        obj[TAG_NAME] = entry.name
        obj[TAG_REF] = entry.ref
        obj[TAG_COPY] = bool(entry.copy)
        obj[TAG_MARK] = entry.mark
    layer.update()
    arrived = added + updated
    for obj in arrived:
        obj[TAG_PRINT] = _fingerprint(obj, scene)
    if arrived and not _busy(layer):
        for obj in layer.objects:
            obj.select_set(False, view_layer=layer)
        for obj in arrived:
            if obj.name in layer.objects:
                obj.select_set(True, view_layer=layer)
        if arrived[0].name in layer.objects:
            layer.objects.active = arrived[0]
        _frame(added)

    parts = []
    if added:
        parts.append("added " + _listed([o.name for o in added]))
    if updated:
        parts.append("updated " + _listed([o.name for o in updated]))
    if not parts:
        parts.append("nothing new" if same else "nothing with a surface")
    text = "From GolemSculpt: " + " and ".join(parts) + "."
    if same and arrived:
        text += " %d unchanged." % same
    if switched_off:
        text += " %d modifier%s switched off: the sculpt has %s in it already." % (
            switched_off,
            "" if switched_off == 1 else "s",
            "it" if switched_off == 1 else "them",
        )
    if painted:
        text += ' The paint is the colour attribute "%s" (Solid shading, Color: Attribute, shows it).' % COMPOSITE
    if header.get("paint_lost"):
        text += " The paint that came could not be read, so the shapes came without it."
    if metric:
        text += " This new file was set to millimetres."
    return text


# ---- sending

_marks = iter(range(1, 1 << 62))


def _entry_of(obj, scene, depsgraph, per_cm):
    """What goes into a package for one object, or None if it has no surface."""
    entry = Entry(obj.name)
    entry.ref = str(obj.get(TAG_REF, ""))
    entry.sent_as = str(obj.get(TAG_NAME, ""))
    entry.copy = bool(obj.get(TAG_COPY, False))
    world = np.array(obj.matrix_world, np.float64)
    entry.origin = tuple(_to_sculpt(world[:3, 3].reshape((1, 3)), per_cm)[0])
    if obj.mode == "EDIT":
        obj.update_from_editmode()
    entry.mark = "b%d-%d-%d" % (os.getpid(), int(time.time() * 1000), next(_marks))
    was, now = obj.get(TAG_MARK, ""), _fingerprint(obj, scene)
    entry.was = str(was)
    if was and now and now == obj.get(TAG_PRINT, ""):
        entry.mark = entry.same_as = was

    shown = obj.evaluated_get(depsgraph)
    mesh = shown.to_mesh()
    try:
        if mesh is None or len(mesh.polygons) == 0:
            return None
        local = np.empty(len(mesh.vertices) * 3, np.float32)
        mesh.vertices.foreach_get("co", local)
        local = local.reshape((-1, 3)).astype(np.float64)
        placed = local @ world[:3, :3].T + world[:3, 3]
        entry.points = _to_sculpt(placed, per_cm).astype(np.float32)
        # Its colour, as it shows: what was painted here goes back as a paint layer.
        colours = _colours_of(mesh)
        if colours is not None:
            entry.paint = {"composite": colours[0], "colours": colours[1], "layers": [], "materials": []}

        mesh.calc_loop_triangles()
        triangles = np.empty(len(mesh.loop_triangles) * 3, np.int32)
        mesh.loop_triangles.foreach_get("vertices", triangles)
        triangles = triangles.reshape((-1, 3))
        # An object turned inside out by its size has its faces the wrong way round.
        inside_out = np.linalg.det(world[:3, :3]) < 0.0
        if inside_out:
            triangles = triangles[:, ::-1]
        entry.triangles = triangles.astype(np.uint32)

        sizes = np.empty(len(mesh.polygons), np.int32)
        mesh.polygons.foreach_get("loop_total", sizes)
        if not inside_out and len(sizes) and int(sizes.max()) > 3:
            corners = np.empty(len(mesh.loops), np.int32)
            mesh.loops.foreach_get("vertex_index", corners)
            entry.poly_sizes = sizes.astype(np.uint32)
            entry.poly_corners = corners.astype(np.uint32)
    finally:
        shown.to_mesh_clear()
    return entry if len(entry.triangles) else None


def _sculpt():
    """What GolemSculpt last left about itself: where it is, and whether it is open."""
    left = _read_json(os.path.join(link_dir(), "sculpt.json")) or {}
    exe = left.get("exe", "")
    pid = left.get("pid", 0)
    running = bool(pid) and process_is_running(int(pid))
    return (exe if exe and os.path.isfile(exe) else ""), running


def _open_sculpt(exe):
    flags = 0
    if sys.platform == "win32":
        # No console of Blender's, and not closed with it.
        flags = 0x00000008 | 0x00000200
    subprocess.Popen(
        [exe],
        cwd=os.path.dirname(exe) or None,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        creationflags=flags,
        close_fds=True,
    )


def send(objects, context=None, open_app=True):
    """Sends these objects to GolemSculpt. Returns (whether anything went, the line that says what)."""
    context = context or bpy.context
    scene = context.scene
    wanted = [o for o in objects if o.type in SENDABLE]
    if not wanted:
        return False, "Select the objects to send to GolemSculpt first."
    per_cm = _per_cm(scene)
    depsgraph = context.evaluated_depsgraph_get()
    entries, bare = [], 0
    for obj in wanted:
        entry = _entry_of(obj, scene, depsgraph, per_cm)
        if entry is None:
            bare += 1
        else:
            entries.append((obj, entry))
    if not entries:
        return False, "Nothing to send: what is selected has no surface."

    name = "%d-%d%s" % (int(time.time() * 1000), os.getpid(), ENDING)
    write_package(os.path.join(_tray("to_sculpt"), name), "blender", [e for _, e in entries])
    # What went is what GolemSculpt has now, modifiers and all.
    for obj, entry in entries:
        obj[TAG_MARK] = entry.mark
        obj[TAG_PRINT] = _fingerprint(obj, scene)
        if TAG_NAME not in obj:
            # So that it goes back as itself next time.
            obj[TAG_NAME] = obj.name
            obj[TAG_REF] = ""
            obj[TAG_COPY] = False
        baked = [m.name for m in obj.modifiers if m.show_viewport] if obj.type == "MESH" else []
        if baked:
            obj[TAG_BAKED] = "\n".join(baked)
        elif TAG_BAKED in obj:
            del obj[TAG_BAKED]

    note = " (%d with no surface left out)" % bare if bare else ""
    sent = "Sent %s to GolemSculpt%s." % (_listed([e.name for _, e in entries]), note)
    exe, running = _sculpt()
    if running:
        return True, sent
    if exe and open_app:
        try:
            _open_sculpt(exe)
            return True, sent + " Opening GolemSculpt..."
        except OSError as e:
            return True, sent + " GolemSculpt could not be opened (%s): open it and they will arrive." % e
    return True, sent + " GolemSculpt is not open: open it within ten minutes and they will arrive."


# ---- the tray, looked at on a timer


def _packages():
    tray = _tray("to_blender")
    found = []
    with os.scandir(tray) as entries:
        for entry in entries:
            if not entry.is_file():
                continue
            if entry.name.endswith(ENDING):
                found.append(entry.path)
            elif ".taking" in entry.name or entry.name.endswith(".tmp"):
                # Left by a Blender that closed part-way.
                try:
                    if time.time() - entry.stat().st_mtime > STALE_SECONDS:
                        os.remove(entry.path)
                except OSError:
                    pass
    return sorted(found)


def take_packages():
    """Takes in every package that is waiting. Returns how many were."""
    taken = 0
    for path in _packages():
        try:
            age = time.time() - os.path.getmtime(path)
        except OSError:
            continue
        if age > STALE_SECONDS:
            try:
                os.remove(path)
            except OSError:
                pass
            continue
        _, layer = _scene_and_layer()
        if _busy(layer):
            if "Object Mode" not in _said["text"]:
                _say("GolemSculpt has sent something. It will arrive when you are back in Object Mode.")
            return taken
        # Another Blender may be looking at the same tray: whoever renames it has it.
        mine = "%s.taking%d" % (path, os.getpid())
        try:
            os.rename(path, mine)
        except OSError:
            continue
        try:
            _say(receive(mine))
            taken += 1
        except Exception as e:
            traceback.print_exc()
            _say("A package from GolemSculpt could not be read: %s" % e)
        finally:
            try:
                os.remove(mine)
            except OSError:
                pass
    return taken


def _tick():
    try:
        take_packages()
    except Exception:
        traceback.print_exc()
    return TICK_SECONDS


# ---- saying that Blender is open, and where it is


def _alive_file():
    return os.path.join(link_dir(), "blender-%d.json" % os.getpid())


def _announce():
    try:
        os.makedirs(link_dir(), exist_ok=True)
        about = {
            "pid": os.getpid(),
            "exe": bpy.app.binary_path,
            "blender": bpy.app.version_string,
            "link": LINK_VERSION,
        }
        _write_json(_alive_file(), about)
        _write_json(os.path.join(link_dir(), "blender.json"), {"exe": bpy.app.binary_path})
    except OSError:
        traceback.print_exc()


def _goodbye():
    try:
        os.remove(_alive_file())
    except OSError:
        pass


# ---- the button


class GOLEMSCULPT_OT_send(bpy.types.Operator):
    """Send the selected objects to GolemSculpt, as they look now and at their real size. One that came from GolemSculpt goes back as itself; anything else arrives as a new object"""

    bl_idname = "golemsculpt.send"
    bl_label = "Send to GolemSculpt"
    bl_options = {"REGISTER"}

    @classmethod
    def poll(cls, context):
        return any(o.type in SENDABLE for o in context.selected_objects)

    def execute(self, context):
        try:
            went, text = send(list(context.selected_objects), context)
        except Exception as e:
            traceback.print_exc()
            went, text = False, "Could not send to GolemSculpt: %s" % e
        _say(text)
        self.report({"INFO"} if went else {"WARNING"}, text)
        return {"FINISHED"} if went else {"CANCELLED"}


class GOLEMSCULPT_PT_link(bpy.types.Panel):
    bl_label = "GolemSculpt"
    bl_idname = "GOLEMSCULPT_PT_link"
    bl_space_type = "VIEW_3D"
    bl_region_type = "UI"
    bl_category = "GolemSculpt"

    def draw(self, context):
        layout = self.layout
        column = layout.column()
        column.scale_y = 1.4
        column.operator(GOLEMSCULPT_OT_send.bl_idname, icon="EXPORT")
        chosen = sum(1 for o in context.selected_objects if o.type in SENDABLE)
        layout.label(text="%d selected" % chosen if chosen else "Select what to send")
        if _said["text"]:
            box = layout.box().column(align=True)
            # The sidebar is narrow: a long line is cut into a few.
            words, line = _said["text"].split(), ""
            for word in words:
                if len(line) + len(word) > 34 and line:
                    box.label(text=line)
                    line = ""
                line = (line + " " + word).strip()
            if line:
                box.label(text=line)
        layout.label(text="Send to Blender in GolemSculpt", icon="IMPORT")
        layout.label(text="brings objects here.")


def _menu(self, context):
    self.layout.separator()
    self.layout.operator(GOLEMSCULPT_OT_send.bl_idname, icon="EXPORT")


_classes = (GOLEMSCULPT_OT_send, GOLEMSCULPT_PT_link)


def _up():
    """What is registered in this Blender, kept where any copy of this script finds it
    (and where loading a file does not clear it)."""
    import types

    return sys.modules.setdefault(MODULE + "_up", types.ModuleType(MODULE + "_up"))


def _own_mark():
    try:
        with open(os.path.abspath(__file__), "rb") as f:
            return hashlib.blake2b(f.read(), digest_size=16).hexdigest()
    except (OSError, NameError):
        return ""


def register():
    # An older copy that is already up (Blender's own, when GolemSculpt starts
    # Blender with a newer one) steps down first.
    older = getattr(_up(), "link", None)
    if older is not None:
        try:
            older["unregister"]()
        except Exception:
            traceback.print_exc()
    for cls in _classes:
        bpy.utils.register_class(cls)
    bpy.types.VIEW3D_MT_object.append(_menu)
    _up().link = {"version": LINK_VERSION, "mark": _own_mark(), "unregister": unregister}
    # A render or a script run in the background is not someone to send things to.
    if not bpy.app.background:
        _announce()
        atexit.register(_goodbye)
        bpy.app.timers.register(_tick, first_interval=TICK_SECONDS, persistent=True)


def unregister():
    if bpy.app.timers.is_registered(_tick):
        bpy.app.timers.unregister(_tick)
    atexit.unregister(_goodbye)
    _goodbye()
    try:
        bpy.types.VIEW3D_MT_object.remove(_menu)
    except Exception:
        pass
    for cls in reversed(_classes):
        try:
            bpy.utils.unregister_class(cls)
        except Exception:
            pass
    _up().link = None


def keep_in_blender():
    """Puts this file where Blender loads it at every start, so the link is
    there however Blender is opened. Returns the place, or None if it could not."""
    try:
        here = os.path.abspath(__file__)
        folder = bpy.utils.user_resource("SCRIPTS", path="startup", create=True)
        if not folder:
            return None
        there = os.path.join(folder, MODULE + ".py")
        with open(here, "rb") as f:
            mine = f.read()
        try:
            with open(there, "rb") as f:
                same = f.read() == mine
        except OSError:
            same = False
        if not same:
            _write_whole(there, lambda f: f.write(mine))
        return there
    except Exception:
        traceback.print_exc()
        return None


# Started by GolemSculpt (`blender --python this file`): come up now, and stay for next time.
if __name__ == "__main__":
    up = getattr(_up(), "link", None)
    if up is None or up.get("mark", "") != _own_mark():
        register()
    keep_in_blender()
