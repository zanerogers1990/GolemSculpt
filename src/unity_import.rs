//! Opening the Unity app's saved sculpts (`.sculpt`), so work begun there
//! carries on here. Read only: this app never writes the format, and never
//! writes over such a file (see [`refuse_to_overwrite`]).
//!
//! # The file
//!
//! As `SceneSerializer.cs` wrote it, all little-endian:
//!
//! ```text
//! "SCLPTSV\0"                 8 bytes
//! version       i32           1, or 2 when any object has hidden faces
//! json length   i32
//! json          UTF-8         one SculptSaveData: settings, then one entry per object
//! then for each object, in the order of the entries:
//!     vertices  f32 x 3 per vertex
//!     normals   f32 x 3 per vertex      (stepped over: a mesh works its own out)
//!     triangles i32 x 3 per triangle
//!     mask      f32 per vertex          only where the entry says hasMask
//!     hidden    u8 per triangle         only where it says hasHidden; 1 = hidden
//! ```
//!
//! The JSON comes first and says how long every array is, so the whole
//! layout is checked against the length of the file before anything is read
//! or any memory taken. The arrays are then read whole, straight into the
//! vectors they will live in, as a `.sclp`'s are.
//!
//! # Axes
//!
//! Unity is left-handed, this app is right-handed, and both have Y up. The
//! Unity app settled which axis turns over when it wrote models out: its OBJ
//! and STL exporters negate X and reverse every triangle. This reader does
//! exactly that ([`axes`]), so a sculpt opened from its `.sculpt` and the same
//! sculpt exported from Unity as an OBJ and imported here land on the same
//! spot, the same way round. Everything stored in coordinates goes through
//! the one mapping:
//!
//! - a point (x, y, z) becomes (-x, y, z), and a triangle (a, b, c) becomes
//!   (a, c, b), which keeps its outward side outward;
//! - a rotation (x, y, z, w) becomes (x, -y, -z, w): the same turn seen in the
//!   mirror. Scale is along the object's own axes and does not change;
//! - mirror axes keep their letters (the plane X = 0 is its own mirror image),
//!   and the centre of a live mirror has its X negated. Radial symmetry
//!   repeats as many times about the same axis; only a custom axis, being a
//!   direction, has its X negated;
//! - the camera's yaw gains half a turn. Unity measured yaw from a camera
//!   looking along +Z, this app from one looking along -Z, and with X negated
//!   those two are the same camera. Pitch, distance and lens are unchanged.
//!
//! What that looks like: the view from this app's Front, Top and Right is
//! pixel for pixel the Unity app's view from its Z and Y bubbles and from the
//! bubble that sits on the right of its gizmo. Only the letter on that last
//! bubble differs (Unity calls it -X), as it must between a left-handed and a
//! right-handed gizmo. (The Unity app started with its camera on -Z, so what
//! was sculpted facing that camera faces this app's Back view; the saved
//! view comes across, so the sculpt opens looked at as it was left.)
//!
//! Lengths need nothing: one unit is 10 mm in both apps.
//!
//! # What goes where
//!
//! Every object comes in with its mesh, mask, hidden faces, name, place,
//! visibility and order; instances stay linked to their mesh, and a live
//! mirror stays live. The material, finish, lighting, HDRI, background,
//! effects and camera go to the same settings here, which were ported from
//! the Unity app with their numbers (see [`look_of`]). The object that was
//! selected is the active one, and its mirror axes and radial symmetry
//! become the scene's: the Unity app kept symmetry per object, this app has
//! one setting. Dynamic topology is set as the file has it, because the
//! Unity app restored it with the file.
//!
//! A live array stays a live array: it goes in the object's entry of the
//! details under [`io::slot::ARRAY`], exactly as the Array tool saves its own,
//! so the object opens with its array and the tool lays the copies out. Its
//! handle points go through the flip like any point. The layout arithmetic is
//! the same numbers in both apps, which is why the copies land where Unity drew
//! them, with one exception: a circle about Y or Z runs the other way round once
//! X is negated (a turn seen in a mirror), so it is kept as a sweep in the other
//! direction ([`through_the_flip`]). [`LiveArray::placements`] is where Unity
//! drew each copy, in this app's axes, for anything that wants to check.
//!
//! The rest has nowhere to go: the other objects' own symmetry, symmetry fixed
//! in the world, and the brush settings (which are the user's own here, not a
//! file's). It is still parsed, brought into this app's axes and handed back
//! in [`Extras`], and [`LeftBehind`] says in plain words what the user is
//! missing.
//!
//! The JSON's own shape, class for class, is in `header.rs`.

mod header;

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use glam::{Quat, Vec3};
use rayon::prelude::*;
use sculpt_array::{ArrayKind, ArrayPlacement, ArraySettings, ArrayState};
use serde::{Deserialize, Serialize};
use serde_json::json;

pub use header::{ArrayEntry, Brush, Environment, Header, Material, ObjectEntry, View, Xyz, Xyzw};

use crate::camera::SavedView;
use crate::io::{self, Details, Mirror, SavedObject, SavedScene, Shading};
use crate::look::{self, Finish, Look};
use crate::mesh::Mesh;
use crate::sculpt::dyntopo::{self, DynTopo};
use crate::sculpt::{Radial, RadialAxis};

/// What the Unity app's files are called.
pub const EXTENSION: &str = "sculpt";
/// How every one of them begins.
pub const MAGIC: &[u8; 8] = b"SCLPTSV\0";
/// The newest layout this reader knows. Unity moved the number only when a
/// change would make an older reader take arrays from the wrong bytes, so a
/// higher one cannot be read safely.
const NEWEST_VERSION: i32 = 2;
/// Bytes before the JSON: the magic, the version and the JSON's length.
const PREAMBLE: u64 = 16;

/// Names this reader keeps things under in [`io::Details`], beside the app's
/// own. Neither is ever written to a `.sclp`: a save builds its details from
/// the running app.
pub mod slot {
    /// [`super::LeftBehind`]: what the file held that did not come across.
    pub const LEFT_BEHIND: &str = "unity_left_behind";
    /// [`super::SavedDynamicTopology`]: Unity restored it with the file, so
    /// opening one here does too.
    pub const DYNAMIC_TOPOLOGY: &str = "unity_dynamic_topology";
}

/// Whether `path` is named as a Unity sculpt.
pub fn is_unity(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case(EXTENSION))
}

/// Stops a save that would land on a Unity sculpt. The app cannot write that
/// format, so anything written there would replace the user's Unity file
/// with one the Unity app can no longer open.
pub fn refuse_to_overwrite(path: &Path) -> io::Result<()> {
    if is_unity(path) {
        return Err(format!(
            "{} is a sculpt from the Unity app and is kept as it is. Save this scene as a .sclp instead.",
            path.display()
        ));
    }
    Ok(())
}

/// The name a save should offer for a scene opened from `path`: the same
/// name, as this app's own kind of file.
pub fn save_name(path: &Path) -> String {
    let stem = path.file_stem().map_or("sculpt".into(), |s| s.to_string_lossy().into_owned());
    format!("{stem}.sclp")
}

// ---------------------------------------------------------------- axes

/// The one mapping between the Unity app's axes and this app's (see the top
/// of this file for why it is this one).
pub mod axes {
    use glam::{Quat, Vec3};

    /// A position, a direction or an offset.
    #[inline]
    pub fn point(p: Vec3) -> Vec3 {
        // Taken from zero rather than negated, so that zero stays plain
        // zero and never reaches a file as "-0.0".
        Vec3::new(0.0 - p.x, p.y, p.z)
    }

    /// A rotation: the same turn, seen in the mirror.
    #[inline]
    pub fn rotation(q: Quat) -> Quat {
        Quat::from_xyzw(q.x, 0.0 - q.y, 0.0 - q.z, q.w)
    }

    /// The camera's yaw in degrees, brought into one turn. Unity's keeps
    /// counting as the view is orbited and can be thousands of degrees.
    pub fn yaw(degrees: f32) -> f32 {
        (degrees + 180.0).rem_euclid(360.0)
    }

    /// The level direction a Unity camera with this yaw faces.
    pub fn heading(yaw_degrees: f32) -> Vec3 {
        let (sin, cos) = yaw_degrees.to_radians().sin_cos();
        point(Vec3::new(sin, 0.0, cos))
    }

    /// Turns every triangle over, which negating one axis calls for. The
    /// last two corners swap and the first stays, as in the Unity app's own
    /// mirroring, so a triangle's place in the list (and with it its hidden
    /// flag) does not move.
    pub fn rewind(indices: &mut [u32]) {
        use rayon::prelude::*;
        indices.par_chunks_exact_mut(3).with_min_len(1 << 14).for_each(|t| t.swap(1, 2));
    }
}

// ---------------------------------------------------------------- the arrays

/// One object's arrays as the file has them: Unity's axes, Unity's winding.
struct RawObject {
    positions: Vec<Vec3>,
    indices: Vec<u32>,
    mask: Option<Vec<f32>>,
    /// One flag per triangle of `indices`.
    hidden: Option<Vec<bool>>,
}

/// How many bytes one object's arrays take, or `None` for counts no file
/// could hold.
fn block_bytes(entry: &ObjectEntry) -> Option<u64> {
    let verts = u64::try_from(entry.vertex_count).ok().filter(|&v| v <= u32::MAX as u64)?;
    let corners = u64::try_from(entry.triangle_index_count).ok().filter(|&c| c % 3 == 0)?;
    let mut bytes = verts.checked_mul(24)?.checked_add(corners.checked_mul(4)?)?;
    if entry.has_mask {
        bytes = bytes.checked_add(verts * 4)?;
    }
    if entry.has_hidden {
        bytes = bytes.checked_add(corners / 3)?;
    }
    Some(bytes)
}

/// Reads `count` values straight into the vector they will live in.
fn read_array<T: bytemuck::Pod>(r: &mut impl Read, count: usize) -> std::io::Result<Vec<T>> {
    let mut values = vec![<T as bytemuck::Zeroable>::zeroed(); count];
    r.read_exact(bytemuck::cast_slice_mut(&mut values))?;
    Ok(values)
}

/// One object's arrays. The caller has checked that they are all in the file.
fn read_object(r: &mut BufReader<File>, entry: &ObjectEntry) -> Option<RawObject> {
    let (verts, corners) = (entry.vertex_count as usize, entry.triangle_index_count as usize);
    let positions: Vec<Vec3> = read_array(r, verts).ok()?;
    // The normals: every mesh works its own out when it is built.
    r.seek_relative(verts as i64 * 12).ok()?;
    let indices: Vec<u32> = read_array(r, corners).ok()?;
    let mask = if entry.has_mask { Some(read_array::<f32>(r, verts).ok()?) } else { None };
    let hidden = if entry.has_hidden {
        let flags: Vec<u8> = read_array(r, corners / 3).ok()?;
        Some(flags.iter().map(|&flag| flag != 0).collect())
    } else {
        None
    };
    // A coordinate that is not a number, or a corner that names no vertex,
    // is a damaged file; Unity refused those too.
    let sound = positions.par_iter().with_min_len(1 << 14).all(|p| p.is_finite())
        && indices.par_iter().with_min_len(1 << 14).all(|&i| (i as usize) < verts);
    sound.then_some(RawObject { positions, indices, mask, hidden })
}

impl RawObject {
    /// The mesh in this app's axes, ready to sculpt.
    fn build(mut self) -> Mesh {
        self.positions.par_iter_mut().with_min_len(1 << 14).for_each(|p| *p = axes::point(*p));
        axes::rewind(&mut self.indices);
        // A mask is 0 to 1; anything else in the file is noise.
        let mask = self.mask.map(|mut mask| {
            mask.par_iter_mut()
                .with_min_len(1 << 14)
                .for_each(|m| *m = if m.is_finite() { m.clamp(0.0, 1.0) } else { 0.0 });
            mask
        });
        match self.hidden {
            Some(hidden) => Mesh::new_with_hidden(self.positions, self.indices, mask, &hidden),
            None => Mesh::new(self.positions, self.indices, mask),
        }
    }
}

// ---------------------------------------------------------------- what has nowhere to go

/// Where an object's brush symmetry lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymmetrySpace {
    /// Through the object's own origin, along its own axes.
    Local,
    /// Through the world's origin, along the world's axes.
    World,
}

/// An object's own brush symmetry. The Unity app kept one per object; this
/// app has one for the whole scene, which takes the selected object's (see
/// [`Extras`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ObjectSymmetry {
    /// Strokes mirrored across X, Y, Z.
    pub mirror: [bool; 3],
    pub show_planes: bool,
    /// Radial symmetry as this app keeps it: on or off, how many times
    /// round, and the axis in the object's own space, in this app's axes.
    pub radial: Radial,
    pub space: SymmetrySpace,
}

impl ObjectSymmetry {
    /// Whether strokes on the object were repeated at all.
    fn any(&self) -> bool {
        self.mirror.contains(&true) || self.radial.on
    }

    /// Whether a stroke is repeated the same way under both. What a
    /// switched-off radial symmetry would do if it were switched on is not
    /// compared: nothing on screen differs for it.
    fn acts_like(&self, other: &ObjectSymmetry) -> bool {
        let radial = |s: &ObjectSymmetry| s.radial.on.then(|| (s.radial.count, s.radial.axis, s.radial.axis_vector()));
        self.mirror == other.mirror && radial(self) == radial(other)
    }
}

/// A live array the Unity app had on an object, as this app has it now: the
/// object's own array, in the object's entry of the details.
#[allow(dead_code)] // the app reads the array out of the details; these are for the tests and whatever checks them
#[derive(Clone, Debug, PartialEq)]
pub struct LiveArray {
    /// The settings the live array has: the file's, except that a circle about
    /// Y or Z has its sweep negated, which is how the copies stay where Unity
    /// drew them (see [`through_the_flip`]).
    pub settings: ArraySettings,
    /// Handle points as offsets from the object's position, in this app's axes.
    pub points: Vec<Vec3>,
    /// Where the Unity app drew each copy relative to the object, in this app's
    /// axes; the first is the object itself. The Array tool's layout of
    /// `settings` and `points` is these.
    pub placements: Vec<ArrayPlacement>,
}

/// What the file kept for one of its objects beyond what the scene took.
#[allow(dead_code)] // see `Extras`
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectExtras {
    /// The object's row in the scene's object list.
    pub row: usize,
    pub symmetry: ObjectSymmetry,
    pub array: Option<LiveArray>,
}

/// Everything a `.sculpt` held that this app has no place for, parsed and in
/// this app's axes, so each can be connected if it ever gets one:
///
/// - **Each object's own symmetry**: `objects[i].symmetry`. The mirror axes
///   and radial symmetry are the scene's here, and take the selected
///   object's; the other objects' are only here, as is whether an object's
///   symmetry was fixed in the world rather than in its own space, which
///   this app does not have.
/// - **Live arrays**: `objects[i].array`. The array is the object's own in the
///   scene's details (see [`LiveArray`]); what is here is for comparing.
/// - **Brush settings**: `brush`, the whole block. Only dynamic topology is
///   applied (see [`SavedDynamicTopology`]).
///
/// A `.sculpt` holds no SSphere rig, no mold and no hard-surface data (the
/// Unity app never saved those), and no quads: a quad-remeshed object was
/// saved as its triangles.
#[allow(dead_code)] // nothing reads these: they are for whoever connects the above
#[derive(Clone, Debug, PartialEq)]
pub struct Extras {
    /// One per object of the file, in the file's order.
    pub objects: Vec<ObjectExtras>,
    pub brush: Brush,
    pub saved_utc: String,
    pub unity_version: String,
}

/// Dynamic topology as a `.sculpt` kept it. The Unity app restored it when
/// the file was opened, so opening one here sets it too.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedDynamicTopology {
    pub enabled: bool,
    /// The longest edge left under the brush, as a fraction of its radius:
    /// the same measure in both apps.
    pub detail: f32,
}

impl SavedDynamicTopology {
    fn of(brush: &Brush) -> Self {
        // 0 is a file from before the setting existed, and means the default.
        let kept = brush.dynamic_topology_detail;
        let detail = if kept.is_finite() && kept > 0.0 { kept } else { dyntopo::DEFAULT_DETAIL };
        Self { enabled: brush.dynamic_topology, detail: detail.clamp(dyntopo::MIN_DETAIL, dyntopo::MAX_DETAIL) }
    }
}

/// Sets dynamic topology the way an opened Unity sculpt left it. Nothing
/// happens for any other file.
pub fn restore_dynamic_topology(details: &Details, dyntopo: &mut DynTopo) {
    if let Some(kept) = details.get::<SavedDynamicTopology>(slot::DYNAMIC_TOPOLOGY) {
        dyntopo.enabled = kept.enabled;
        dyntopo.detail = kept.detail.clamp(dyntopo::MIN_DETAIL, dyntopo::MAX_DETAIL);
    }
}

/// What a Unity sculpt held that did not come across, in plain words for the
/// status line.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LeftBehind {
    /// About its objects: missed whether the file is opened or added.
    pub objects: Vec<String>,
    /// About the scene as a whole: missed only when the file is opened in
    /// place of the scene, since adding a file never takes its settings.
    pub settings: Vec<String>,
}

/// What to add to the status line about a file that has just come in:
/// `replaced` for one opened in place of the scene, otherwise one whose
/// objects were added. `None` for any file but a Unity sculpt.
pub fn status_note(details: &Details, replaced: bool) -> Option<String> {
    let left: LeftBehind = details.get(slot::LEFT_BEHIND)?;
    let mut missing = left.objects;
    let mut note = String::new();
    if replaced {
        missing.extend(left.settings);
        note += "It is a sculpt from the Unity app: saving makes a .sclp and leaves that file as it is.";
    }
    if !missing.is_empty() {
        if !note.is_empty() {
            note += " ";
        }
        note += "Left behind: ";
        note += &missing.join("; ");
        note += ".";
    }
    (!note.is_empty()).then_some(note)
}

// ---------------------------------------------------------------- the scene

/// A Unity sculpt, read and brought into this app's axes.
pub struct Imported {
    pub scene: SavedScene,
    /// What the file held that the scene could not take.
    #[allow(dead_code)] // see `Extras`
    pub extras: Extras,
    /// The same, in plain words for the user. Also kept in the scene's
    /// details under [`slot::LEFT_BEHIND`].
    #[allow(dead_code)] // the app reads it back out of the details (`status_note`)
    pub left_behind: LeftBehind,
}

fn damaged(path: &Path) -> String {
    format!("{} is damaged (it ends early, or its parts do not add up), so it was not opened.", path.display())
}

/// Opens a Unity sculpt.
#[allow(dead_code)] // the app comes in through `io::load_scene`; this is the way in for the extras
pub fn load(path: &Path) -> io::Result<Imported> {
    let mut file = File::open(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    let mut magic = [0u8; 8];
    if file.read_exact(&mut magic).is_err() || &magic != MAGIC {
        return Err(format!("{} is not a sculpt from the Unity app.", path.display()));
    }
    read(file, path)
}

/// [`load`] for `io::load_scene`, which has already read the magic off the
/// front of `file`.
pub fn load_scene(file: File, path: &Path) -> io::Result<SavedScene> {
    read(file, path).map(|imported| imported.scene)
}

/// Reads everything after the magic. Nothing is built until the whole file
/// has been read and checked, so a damaged one costs nothing but the error.
fn read(file: File, path: &Path) -> io::Result<Imported> {
    let bad = || damaged(path);
    let total = file.metadata().map_err(|e| format!("Could not read {}: {e}", path.display()))?.len();
    let mut r = BufReader::with_capacity(1 << 16, file);
    let mut head = [0u8; 8];
    r.read_exact(&mut head).map_err(|_| bad())?;
    let version = i32::from_le_bytes(head[..4].try_into().unwrap());
    let json_length = i32::from_le_bytes(head[4..].try_into().unwrap());
    if version > NEWEST_VERSION {
        return Err(format!(
            "{} was saved by a newer version of the Unity app (file format {version}; this app reads up to \
             {NEWEST_VERSION}), so it was not opened.",
            path.display()
        ));
    }
    if version < 1 || json_length <= 0 || PREAMBLE + json_length as u64 > total {
        return Err(bad());
    }
    let mut json = vec![0u8; json_length as usize];
    r.read_exact(&mut json).map_err(|_| bad())?;
    let header = Header::parse(&json).ok_or_else(bad)?;
    if header.objects.is_empty() {
        return Err(format!("{} has no objects in it, so there is nothing to open.", path.display()));
    }

    // Every array's length is known now: check that they are all there
    // before reading one, so a damaged count cannot ask for more memory than
    // the file is big. (Bytes after the last array are ignored, as Unity did.)
    let mut needed = PREAMBLE + json_length as u64;
    for entry in &header.objects {
        needed = block_bytes(entry).and_then(|bytes| needed.checked_add(bytes)).ok_or_else(bad)?;
    }
    if needed > total {
        return Err(bad());
    }
    let mut raw = Vec::with_capacity(header.objects.len());
    for entry in &header.objects {
        raw.push(read_object(&mut r, entry).ok_or_else(bad)?);
    }
    Ok(convert(header, raw))
}

/// A number from the file, or `fallback` if it is not one.
fn number(value: f32, fallback: f32) -> f32 {
    if value.is_finite() { value } else { fallback }
}

/// Where something sat, in this app's axes. A placement that makes no sense
/// falls back to "where the mesh itself sits", part by part, rather than
/// costing the object.
fn placement(position: Xyz, rotation: Xyzw, scale: Xyz) -> (Vec3, Quat, Vec3) {
    let position = Some(position.vec3()).filter(|p| p.is_finite()).unwrap_or(Vec3::ZERO);
    let rotation = Quat::from_xyzw(rotation.x, rotation.y, rotation.z, rotation.w);
    let rotation = if rotation.is_finite() && rotation.length() > 0.5 { rotation.normalize() } else { Quat::IDENTITY };
    // Size along the object's own axes: the same either side of the mirror.
    // Unity never let one reach zero or go negative.
    let scale = scale.vec3().to_array().map(|s| if s.is_finite() && s.abs() > 1e-9 { s.abs() } else { 1.0 });
    (axes::point(position), axes::rotation(rotation), Vec3::from_array(scale))
}

/// A name no row has yet.
fn unique(wanted: &str, taken: &[SavedObject]) -> String {
    let free = |name: &str| taken.iter().all(|o| o.name != name);
    if free(wanted) {
        return wanted.to_owned();
    }
    (2..).map(|n| format!("{wanted} ({n})")).find(|name| free(name)).unwrap_or_else(|| wanted.to_owned())
}

/// The live array of an entry, if it has one this app knows how to lay out,
/// in this app's axes. The rule for "has one" is the Unity loader's.
fn array_of(entry: &ArrayEntry) -> Option<LiveArray> {
    let kind = ArrayKind::from_id(entry.kind);
    if kind == ArrayKind::None || entry.points.len() < 2 {
        return None;
    }
    let settings = ArraySettings {
        kind,
        count: entry.count,
        end_scale: entry.end_scale,
        mid_scale: entry.mid_scale,
        closed: entry.closed,
        follow: entry.follow,
        space_by_size: entry.space_by_size,
        axis: entry.axis,
        sweep: entry.sweep,
    }
    .clamped();
    let points: Vec<Vec3> = entry.points.iter().map(|p| p.vec3()).collect();
    if points.iter().any(|p| !p.is_finite()) {
        return None;
    }
    // The layout is the same arithmetic in both apps, so on Unity's numbers
    // it gives Unity's placements.
    let placements = sculpt_array::compute_layout(&settings, &points);
    Some(LiveArray {
        settings: through_the_flip(settings),
        points: points.into_iter().map(axes::point).collect(),
        placements: placements
            .iter()
            .map(|p| ArrayPlacement {
                offset: axes::point(p.offset),
                rotation: axes::rotation(p.rotation),
                scale: p.scale,
            })
            .collect(),
    })
}

/// An array's settings as the same array reads once X is negated. A path's
/// copies need no change: its offsets are the path's points, and the frame it
/// carries along the path is made of cross products, which turn over with the
/// mirror exactly as a rotation seen in it does. A circle's turn is a fixed
/// angle about a fixed axis, and a turn about Y or Z seen in the mirror is the
/// opposite turn, so its sweep goes the other way (a negative sweep: see
/// `ArraySettings::sweep`). About X the axis lies in the mirror and nothing changes.
fn through_the_flip(settings: ArraySettings) -> ArraySettings {
    if settings.kind == ArrayKind::Radial && settings.axis != 0 {
        return ArraySettings { sweep: -settings.sweep, ..settings };
    }
    settings
}

/// An object's own symmetry, in this app's terms.
fn symmetry_of(entry: &ObjectEntry) -> ObjectSymmetry {
    // A count of 0 is a file from before radial symmetry: Unity then left
    // the object on what it starts with, which is also what this app starts with.
    let mut radial = Radial::default();
    if entry.radial_count > 0 {
        radial.on = entry.radial;
        radial.count = entry.radial_count.unsigned_abs();
        // X, Y, Z or Custom by number; one Unity did not know left the axis alone.
        radial.axis = usize::try_from(entry.radial_axis)
            .ok()
            .and_then(|axis| RadialAxis::ALL.get(axis).copied())
            .unwrap_or(radial.axis);
        // A direction in the object's own space, which is mirrored with its
        // mesh. How many times round is the same either side of the mirror.
        radial.custom_axis = axes::point(entry.radial_custom_axis.vec3());
    }
    ObjectSymmetry {
        mirror: [entry.mirror_x, entry.mirror_y, entry.mirror_z],
        show_planes: entry.show_mirror_planes,
        radial: radial.sanitized(),
        space: if entry.symmetry_space == 1 { SymmetrySpace::World } else { SymmetrySpace::Local },
    }
}

/// Brings a parsed file into this app: its axes, its scene, its settings.
fn convert(header: Header, raw: Vec<RawObject>) -> Imported {
    let mut rows: Vec<SavedObject> = Vec::new();
    let mut details = Details::default();
    let mut extras = Vec::with_capacity(header.objects.len());
    let mut left = LeftBehind::default();

    for (mesh, entry) in header.objects.iter().enumerate() {
        let row = rows.len();
        let name = if entry.name.is_empty() { "Object" } else { entry.name.as_str() };
        let (position, rotation, scale) = placement(entry.position, entry.rotation, entry.scale);
        let name = unique(name, &rows);
        rows.push(SavedObject { name: name.clone(), visible: entry.visible, mesh, position, rotation, scale });

        // Live mirror copies: the same planes, through the mirrored centre.
        let centre = Some(entry.mirror_repeater_center.vec3()).filter(|c| c.is_finite()).unwrap_or(Vec3::ZERO);
        let mirror = (entry.mirror_repeater_axes & 7 != 0)
            .then(|| json!({ "axes": entry.mirror_repeater_axes & 7, "center": axes::point(centre).to_array() }));
        if let Some(mirror) = &mirror {
            details.set_object(row, io::slot::MIRROR_COPIES, mirror);
        }

        for instance in &entry.instances {
            let (position, rotation, scale) = placement(instance.position, instance.rotation, instance.scale);
            let wanted = if instance.name.is_empty() { format!("{name} Instance") } else { instance.name.clone() };
            let name = unique(&wanted, &rows);
            rows.push(SavedObject { name, visible: instance.visible, mesh, position, rotation, scale });
        }

        // A live array stays one: the object's entry keeps it the way the Array
        // tool saves its own, and the tool takes it up when the file is opened.
        let array = array_of(&entry.array);
        if let Some(live) = &array {
            let state = ArrayState { settings: live.settings, points: live.points.clone() };
            details.set_tool_object(row, io::slot::ARRAY, &sculpt_array::state_to_saved(&state));
        } else if entry.array.kind != 0 {
            left.objects.push(format!("an array on \"{name}\" that this app cannot read"));
        }

        let symmetry = symmetry_of(entry);
        // Symmetry fixed in the world is the same thing as symmetry in the
        // object's own space only while the object sits square at the origin.
        let square = position.length() < 1e-6 && rotation.abs_diff_eq(Quat::IDENTITY, 1e-6);
        if symmetry.space == SymmetrySpace::World && symmetry.any() && !square {
            left.settings.push(format!("the world-space symmetry of \"{name}\" (symmetry here goes with the object)"));
        }
        extras.push(ObjectExtras { row, symmetry, array });
    }

    // The selected object: the active one here, and the one whose mirror
    // axes and radial symmetry the scene's one setting takes.
    let selected = header.primary_selection_index.clamp(0, header.objects.len() as i64 - 1) as usize;
    let active = extras[selected].row;
    let own = extras[selected].symmetry;
    if extras.iter().any(|e| !e.symmetry.acts_like(&own)) {
        left.settings.push(format!(
            "each object's own symmetry (this app has one setting for the scene, taken from \"{}\")",
            rows[active].name
        ));
    }
    let [x, y, z] = own.mirror;
    details.set(io::slot::MIRROR, &Mirror { x, y, z, guides: own.show_planes, radial: own.radial });

    details.set(io::slot::SHADING, &shading_of(&header.material, &header.environment));
    details.set(io::slot::LOOK, &look_of(&header.material, &header.environment));
    if let Some(view) = view_of(&header.camera) {
        details.set(io::slot::CAMERA, &view);
    }
    details.set(slot::DYNAMIC_TOPOLOGY, &SavedDynamicTopology::of(&header.brush));
    left.settings.push("the Unity app's brush settings".to_owned());
    details.set(slot::LEFT_BEHIND, &left);

    // Building a mesh (its spatial index, adjacency and normals) is the slow
    // part of opening a large scene, so they are built several at once.
    let meshes = raw.into_par_iter().map(RawObject::build).collect();
    Imported {
        scene: SavedScene { meshes, objects: rows, active, details },
        extras: Extras {
            objects: extras,
            brush: header.brush,
            saved_utc: header.saved_utc,
            unity_version: header.unity_version,
        },
        left_behind: left,
    }
}

// ---------------------------------------------------------------- the look

/// The half of the look kept in `io::Shading`: surface, matcap, background.
fn shading_of(m: &Material, e: &Environment) -> Shading {
    let base = Shading::default();
    Shading {
        // Unity only switched a matcap on if the file named one.
        matcap_on: m.matcap_enabled && !m.matcap_name.is_empty(),
        // By the picture's name alone, as Unity kept it; the app finds the folder.
        matcap: m.matcap_name.clone(),
        flat_shading: m.flat_shading,
        cavity: m.screen_cavity_enabled,
        cavity_ridge: number(m.cavity_ridge, base.cavity_ridge),
        cavity_valley: number(m.cavity_valley, base.cavity_valley),
        base_color: m.base_color.rgb(base.base_color),
        smoothness: number(m.smoothness, base.smoothness),
        // Flat, gradient, or the HDRI, which falls back to the gradient
        // while no picture is loaded.
        gradient_background: e.background_mode != 0,
        background: e.background_color_a.rgb(base.background),
        background_top: e.background_color_b.rgb(base.background_top),
    }
}

/// A preset's place in one of this app's lists, by the id both apps give it.
/// An id this app does not have (or none, in an older file) keeps `fallback`.
fn preset(ids: impl IntoIterator<Item = &'static str>, id: &str, fallback: usize) -> usize {
    ids.into_iter().position(|known| known == id).unwrap_or(fallback)
}

/// Finish, lighting, HDRI and effects. `look.rs` was ported from the Unity
/// app with its presets and its numbers, so each setting goes to the one of
/// the same meaning unchanged; only the two that are directions need the
/// axis mapping (the heading of a rig unlocked from the camera, and the
/// HDRI, see below).
pub fn look_of(m: &Material, e: &Environment) -> Look {
    let mut look = Look::default();
    look.metallic = number(m.metallic, look.metallic);
    look.normal_strength = number(m.normal_strength, look.normal_strength);
    look.normal_scale = number(m.normal_noise_scale, look.normal_scale);
    look.matcap_intensity = number(m.matcap_intensity, look.matcap_intensity);
    look.matcap_tint = number(m.matcap_tint_strength, look.matcap_tint);

    // Unity kept one switch per finish and never left two on; if a file
    // has, the last one it would have applied wins.
    look.finish = if m.wood_enabled {
        Finish::Wood
    } else if m.clay_enabled {
        Finish::Clay
    } else if m.metal_enabled {
        Finish::Metal
    } else if m.lure_enabled {
        Finish::LurePlastic
    } else {
        Finish::None
    };

    let lure = &mut look.lure;
    lure.preset = preset(look::LURES.iter().map(|p| p.id), &m.lure_preset_id, lure.preset);
    lure.flake_size = number(m.lure_flake_size, lure.flake_size);
    lure.flake_amount = number(m.lure_flake_amount, lure.flake_amount);
    lure.sparkle = number(m.lure_sparkle, lure.sparkle);
    lure.translucency = number(m.lure_translucency, lure.translucency);
    lure.gloss = number(m.lure_gloss, lure.gloss);
    lure.reference = number(m.lure_reference_size, 0.0).max(0.0);

    let metal = &mut look.metal;
    metal.preset = preset(look::METALS.iter().map(|p| p.id), &m.metal_preset_id, metal.preset);
    metal.exposure = number(m.metal_exposure, metal.exposure);
    metal.edge_wear = number(m.metal_edge_wear, metal.edge_wear);
    metal.wash = number(m.metal_wash, metal.wash);
    metal.detail = number(m.metal_detail, metal.detail);
    metal.pattern_size = number(m.metal_pattern_size, metal.pattern_size);
    metal.gloss = number(m.metal_gloss, metal.gloss);
    metal.seed = number(m.metal_pattern_seed, metal.seed);
    metal.reference = number(m.metal_reference_size, 0.0).max(0.0);

    let clay = &mut look.clay;
    clay.preset = preset(look::CLAYS.iter().map(|p| p.id), &m.clay_preset_id, clay.preset);
    clay.gloss = number(m.clay_gloss, clay.gloss);
    clay.wetness = number(m.clay_wetness, clay.wetness);
    clay.subsurface = number(m.clay_subsurface, clay.subsurface);
    clay.recess = number(m.clay_recess, clay.recess);
    clay.detail = number(m.clay_detail, clay.detail);
    clay.grain = number(m.clay_grain, clay.grain);
    clay.reference = number(m.clay_reference_size, 0.0).max(0.0);

    let wood = &mut look.wood;
    wood.preset = preset(look::WOODS.iter().map(|p| p.id), &m.wood_preset_id, wood.preset);
    // 0 vertical, 1 left to right, 2 front to back in both apps, and each of
    // those lines is its own mirror image.
    wood.axis = usize::try_from(m.wood_grain_axis).ok().filter(|&axis| axis <= 2).unwrap_or(0);
    wood.grain = number(m.wood_grain, wood.grain);
    wood.grain_scale = number(m.wood_grain_scale, wood.grain_scale);
    wood.edge_wear = number(m.wood_edge_wear, wood.edge_wear);
    wood.patina = number(m.wood_patina, wood.patina);
    wood.detail = number(m.wood_detail, wood.detail);
    wood.sheen = number(m.wood_sheen, wood.sheen);
    wood.cracks = number(m.wood_cracks, wood.cracks);
    wood.seed = number(m.wood_grain_seed, wood.seed);
    wood.reference = number(m.wood_reference_size, 0.0).max(0.0);

    let glow = &mut look.glow;
    glow.on = m.glow_enabled;
    glow.preset = preset(look::GLOWS.iter().map(|p| p.id), &m.glow_preset_id, glow.preset);
    glow.intensity = number(m.glow_intensity, glow.intensity);
    glow.coverage = number(m.glow_coverage, glow.coverage);
    glow.rim = number(m.glow_rim, glow.rim);
    glow.pattern_size = number(m.glow_pattern_size, glow.pattern_size);
    glow.speed = number(m.glow_speed, glow.speed);
    glow.pulse = number(m.glow_pulse, glow.pulse);
    glow.dim = number(m.glow_dim, glow.dim);
    glow.hue_shift = number(m.glow_hue_shift, glow.hue_shift);
    glow.reference = number(m.glow_reference_size, 0.0).max(0.0);

    look.lighting = preset(look::LIGHTING.iter().map(|p| p.id), &e.lighting_preset, look.lighting);
    look.five_point = e.lighting_five_point;
    look.brightness = number(e.lighting_brightness, look.brightness).clamp(0.0, look::MAX_BRIGHTNESS);
    // A turn of the rig about its own vertical, measured against the view
    // (forward swinging toward the right of the picture) in both apps, so
    // the mirror leaves it alone.
    look.rotation = number(e.lighting_rotation, look.rotation);
    look.follow_camera = e.lighting_follow_camera;
    // Unity kept the heading a world-locked rig faces as the yaw of the
    // camera it was unlocked from.
    look.world_forward = axes::heading(number(e.lighting_world_yaw, 0.0));
    look.shadows = e.lighting_shadows;

    let hdri = &mut look.hdri;
    hdri.enabled = e.hdri_enabled;
    hdri.path = (!e.hdri_path.is_empty()).then(|| e.hdri_path.clone().into());
    // The same angle: both apps look the picture up with the same formula,
    // which keeps what was straight ahead and straight behind in the front
    // view where it was. (Left and right of it swap: the Unity app drew the
    // panorama mirrored, this one draws it the right way round.)
    hdri.rotation = number(e.hdri_rotation, 0.0).rem_euclid(360.0);
    hdri.exposure = number(e.hdri_exposure, hdri.exposure);
    hdri.ambient = number(e.hdri_ambient_intensity, hdri.ambient);
    hdri.reflection = number(e.hdri_reflection_intensity, hdri.reflection);
    look.hdri_background = e.background_mode == 2;
    look.gradient_bias = number(e.gradient_bias, look.gradient_bias);

    // A scene saved without effects wrote zeros that mean nothing; Unity
    // skipped them, and here the effects stay as the app starts them.
    if e.post_available {
        look.bloom = e.bloom_enabled;
        look.bloom_intensity = number(e.bloom_intensity, look.bloom_intensity);
        look.bloom_threshold = number(e.bloom_threshold, look.bloom_threshold);
        look.vignette = e.vignette_enabled;
        look.vignette_intensity = number(e.vignette_intensity, look.vignette_intensity);
        look.vignette_smoothness = number(e.vignette_smoothness, look.vignette_smoothness);
        look.depth_of_field = e.dof_enabled;
        look.focus_distance = number(e.dof_focus_distance, look.focus_distance);
        look.aperture = number(e.dof_aperture, look.aperture);
        look.grading = e.color_adjustments_enabled;
        look.saturation = number(e.saturation, look.saturation);
        look.contrast = number(e.contrast, look.contrast);
    }
    look
}

/// The saved view, or `None` for a file from before views were kept (the
/// app then frames the model).
fn view_of(view: &View) -> Option<SavedView> {
    let pivot = view.pivot.vec3();
    let sound = view.valid
        && pivot.is_finite()
        && [view.yaw, view.pitch, view.distance].iter().all(|n| n.is_finite())
        && view.distance > 0.0;
    sound.then(|| SavedView {
        yaw: axes::yaw(view.yaw),
        pitch: view.pitch.clamp(-89.0, 89.0),
        distance: view.distance,
        pivot: axes::point(pivot).to_array(),
        ortho: view.orthographic,
        // The Unity camera's lens, which is also this app's.
        fov_y: SavedView::default().fov_y,
    })
}

#[cfg(test)]
mod tests;
