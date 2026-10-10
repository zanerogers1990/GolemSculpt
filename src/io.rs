//! Files: the app's own `.sclp` scenes, and mesh import and export as
//! Wavefront OBJ and STL (binary and ASCII).
//!
//! A `.sclp` keeps the whole scene exactly: every mesh as raw arrays, then
//! one JSON section for everything else (objects, look, camera, tool state).
//! Its layout, and how to add to it, are set out above `SCENE_MAGIC` below.
//! Saving never harms the file already there: see `replace_file`.
//!
//! For the mesh formats:
//!
//! Axes and units are settled here, once. The app is right-handed with Y up,
//! like OBJ, so an OBJ goes in and out as it is. STL is right-handed with Z up
//! and written in millimetres, which is what every slicer assumes, so the STL
//! side is a quarter turn about X plus the scale in `sculpt_geom::units`: a
//! point in the file, (x, y, z) mm, is (x, z, -y) / 10 in the app, and back
//! again it is (x, -z, y) * 10. A quarter turn is a rotation, not a mirror, so
//! triangle winding survives it unchanged, a model standing up in the viewport
//! stands up on the print bed, and the +Z side that faces you in the default
//! view faces -Y, the front of a bed. (The Unity app also negates X on the
//! way through, because Unity is left-handed; this app is not, so there is
//! nothing to undo.)
//!
//! An export bakes the object's position, rotation and scale into the file, so
//! what reaches the slicer is what is on screen.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use glam::{Mat3, Mat4, Quat, Vec3};
use rayon::prelude::*;
use sculpt_geom::units;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::doc::Options;
use crate::mesh::{Mesh, Polys};

pub type Result<T> = std::result::Result<T, String>;

/// A model as read from a file, already in the app's axes and units.
pub struct RawMesh {
    pub positions: Vec<Vec3>,
    pub indices: Vec<u32>,
    /// The file states a real size (an STL, in millimetres), so the model keeps
    /// it. An OBJ's units are anyone's guess, so it is fitted to the scene
    /// instead: see [`place_import`].
    pub real_size: bool,
}

pub fn import(path: &Path) -> Result<RawMesh> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|mut f| f.read_to_end(&mut bytes))
        .map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    let raw = match ext.as_str() {
        "obj" => parse_obj(&bytes)?,
        "stl" => parse_stl(&bytes)?,
        other => return Err(format!("Unsupported file type \".{other}\" (use .obj or .stl)")),
    };
    if raw.indices.is_empty() {
        return Err("The file contains no triangles".into());
    }
    Ok(raw)
}

fn parse_obj(bytes: &[u8]) -> Result<RawMesh> {
    let text = String::from_utf8_lossy(bytes);
    let mut positions: Vec<Vec3> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut face: Vec<u32> = Vec::new();
    for line in text.lines() {
        let mut parts = line.split_ascii_whitespace();
        match parts.next() {
            Some("v") => {
                let mut c = [0.0f32; 3];
                for slot in &mut c {
                    *slot = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
                }
                positions.push(Vec3::from_array(c));
            }
            Some("f") => {
                face.clear();
                for token in parts {
                    // "v", "v/vt", "v//vn" or "v/vt/vn"; negative is relative.
                    let Some(Ok(i)) = token.split('/').next().map(str::parse::<i64>) else {
                        continue;
                    };
                    let index = if i < 0 { positions.len() as i64 + i } else { i - 1 };
                    if index >= 0 && (index as usize) < positions.len() {
                        face.push(index as u32);
                    }
                }
                // Fan-triangulate polygons.
                for k in 1..face.len().saturating_sub(1) {
                    indices.extend_from_slice(&[face[0], face[k], face[k + 1]]);
                }
            }
            _ => {}
        }
    }
    let (positions, indices) = weld_exact(positions, indices);
    Ok(tidy(positions, indices, false))
}

// ---------------------------------------------------------------- STL axes

/// A point in an STL file (Z up, millimetres) as the app sees it.
fn from_stl(p: Vec3) -> Vec3 {
    Vec3::new(units::from_millimetres(p.x), units::from_millimetres(p.z), units::from_millimetres(-p.y))
}

/// An app point (Y up, world units) as it is written to an STL file.
fn to_stl(p: Vec3) -> Vec3 {
    Vec3::new(units::to_millimetres(p.x), units::to_millimetres(-p.z), units::to_millimetres(p.y))
}

// ---------------------------------------------------------------- STL reading

const STL_HEADER: usize = 80;
const STL_FACET: usize = 50;

fn parse_stl(bytes: &[u8]) -> Result<RawMesh> {
    let corners = if looks_binary(bytes) { read_binary_stl(bytes) } else { read_ascii_stl(bytes) };
    if corners.len() < 3 {
        return Err("No triangles found. Is this really an STL file?".into());
    }
    let (positions, indices) = weld_corners(&corners);
    drop(corners);
    let mut raw = tidy(positions, indices, true);
    if raw.indices.is_empty() {
        return Err("Every triangle in this STL has no area, so there is nothing to sculpt.".into());
    }
    // Normals are no help when a writer leaves them zero, and a whole file can
    // be wound the wrong way: a closed surface that encloses negative volume
    // is simply inside out.
    if is_inside_out(&raw.positions, &raw.indices) {
        for t in raw.indices.chunks_exact_mut(3) {
            t.swap(1, 2);
        }
    }
    for p in &mut raw.positions {
        *p = from_stl(*p);
    }
    Ok(raw)
}

/// Whether these bytes are a binary STL.
///
/// The header is no evidence: the format leaves its 80 bytes free and plenty
/// of writers start them with "solid", which is also how every ASCII file
/// begins. The length is: a binary file is exactly 84 bytes plus 50 per
/// triangle. A file that misses that sum is damaged or padded (a cut-off
/// download, a colour table on the end, a wrong count) and is still binary
/// unless its first lines read as text, so that is the next question.
fn looks_binary(bytes: &[u8]) -> bool {
    if bytes.len() < STL_HEADER + 4 {
        return false;
    }
    let count = u32::from_le_bytes(bytes[STL_HEADER..STL_HEADER + 4].try_into().unwrap()) as u64;
    if (STL_HEADER as u64 + 4) + count * STL_FACET as u64 == bytes.len() as u64 {
        return true;
    }
    let probe = &bytes[..bytes.len().clamp(0, 512)];
    !looks_like_ascii_stl(probe)
}

/// Text all the way down (high bytes are fine: a model name may be UTF-8) that
/// says "facet" somewhere. Binary floats hit control bytes within a few dozen
/// bytes, and a header that merely says "solid" has no facet in it.
fn looks_like_ascii_stl(probe: &[u8]) -> bool {
    let text_byte = |b: u8| b >= 0x20 && b != 0x7f || matches!(b, b'\t' | b'\n' | b'\r');
    probe.iter().all(|&b| text_byte(b)) && probe.windows(5).any(|w| w.eq_ignore_ascii_case(b"facet"))
}

/// The triangles of a binary STL as a flat run of corners, three to a facet.
/// Where the count in the header disagrees with the length, the file wins; a
/// count of zero on a file that has data is a streaming writer that never
/// went back to fill it in.
fn read_binary_stl(bytes: &[u8]) -> Vec<Vec3> {
    let body = &bytes[STL_HEADER + 4..];
    let available = body.len() / STL_FACET;
    let declared = u32::from_le_bytes(bytes[STL_HEADER..STL_HEADER + 4].try_into().unwrap()) as usize;
    let count = (if declared == 0 { available } else { declared }).clamp(0, available);

    let mut corners = Vec::with_capacity(count * 3);
    for facet in body.chunks_exact(STL_FACET).take(count) {
        let float = |at: usize| f32::from_le_bytes(facet[at..at + 4].try_into().unwrap());
        let point = |at: usize| Vec3::new(float(at), float(at + 4), float(at + 8));
        push_facet(&mut corners, point(0), point(12), point(24), point(36));
    }
    corners
}

/// Three numbers off the front of a line, if they are all there.
fn read_vec3<'a>(parts: &mut impl Iterator<Item = &'a str>) -> Option<Vec3> {
    let mut c = [0.0f32; 3];
    for slot in &mut c {
        *slot = parts.next()?.parse().ok()?;
    }
    Some(Vec3::from_array(c))
}

/// The same from an ASCII STL. Only "facet" and "vertex" lines carry anything,
/// so the rest is read loosely: a stray blank line, Windows line endings or
/// several solids in one file all just work.
fn read_ascii_stl(bytes: &[u8]) -> Vec<Vec3> {
    let text = String::from_utf8_lossy(bytes);
    let mut corners = Vec::new();
    let mut outline: Vec<Vec3> = Vec::with_capacity(4);
    let mut normal = Vec3::ZERO;
    for line in text.lines() {
        let mut parts = line.split_ascii_whitespace();
        let Some(word) = parts.next() else { continue };
        if word.eq_ignore_ascii_case("vertex") {
            outline.extend(read_vec3(&mut parts));
        } else if word.eq_ignore_ascii_case("facet") {
            // "facet normal nx ny nz"; writers that do not care leave it zero.
            parts.next();
            normal = read_vec3(&mut parts).unwrap_or(Vec3::ZERO);
            outline.clear();
        } else if word.eq_ignore_ascii_case("endloop") || word.eq_ignore_ascii_case("endfacet") {
            // Three corners in every real file; a longer loop is fanned rather than dropped.
            for k in 1..outline.len().saturating_sub(1) {
                push_facet(&mut corners, normal, outline[0], outline[k], outline[k + 1]);
            }
            outline.clear();
        }
    }
    corners
}

/// Appends one facet's corners, wound to agree with its stored normal.
///
/// STL keeps both the normal and the corner order and they may disagree: CAD
/// exports and repaired meshes often carry corners in the wrong order and rely
/// on readers to follow the normal. Left alone those facets face inward and
/// show as holes the brush cannot take. A zero or missing normal says nothing
/// and leaves the order as written.
fn push_facet(corners: &mut Vec<Vec3>, normal: Vec3, a: Vec3, b: Vec3, c: Vec3) {
    if !(a.is_finite() && b.is_finite() && c.is_finite()) {
        return;
    }
    let (b, c) = if normal.dot((b - a).cross(c - a)) < 0.0 { (c, b) } else { (b, c) };
    corners.extend_from_slice(&[a, b, c]);
}

/// Corners across the bounding box diagonal: how fine the welding grid is.
const WELD_CELLS: f32 = 1_000_000.0;

/// Turns the triangle soup of an STL, where every facet carries its own three
/// corners, into a mesh whose triangles share vertices. Sculpting needs that:
/// smoothing, remeshing and symmetry all assume neighbours share vertices, and
/// a soup tears apart along every edge at the first stroke.
///
/// Corners weld when they land in the same cell of a grid a millionth of the
/// model's diagonal across. The grid is relative because STL has no units, so
/// a fixed tolerance would be useless on a model in metres or ruinous on one
/// in millimetres. Corners the writer copied from one vertex are bit-identical
/// and always meet; the tolerance is for files that have been through a lossy
/// round trip, where two values a hair apart can fall either side of a cell
/// wall, so a corner that has no vertex in its own cell also looks across any
/// wall it is close to. The first corner to claim a spot keeps its exact
/// coordinates.
fn weld_corners(corners: &[Vec3]) -> (Vec<Vec3>, Vec<u32>) {
    let mut vertices: Vec<Vec3> = Vec::new();
    let mut indices = Vec::with_capacity(corners.len());
    if corners.is_empty() {
        return (vertices, indices);
    }
    let (lo, hi) = bounds_of(corners);
    let cell = ((hi - lo).length() / WELD_CELLS).max(1e-9);
    let reach = cell * 0.25;
    // Each axis index is at most WELD_CELLS + 2 once offset by one, inside 21 bits.
    let key = |c: [i64; 3]| ((c[0] + 1) as u64) << 42 | ((c[1] + 1) as u64) << 21 | (c[2] + 1) as u64;
    let lean = |f: f32| -> i64 { if f < 0.25 { -1 } else if f > 0.75 { 1 } else { 0 } };
    let mut seen: HashMap<u64, u32> = HashMap::with_capacity(corners.len() / 3);

    for &p in corners {
        let at = (p - lo) / cell;
        let home = [at.x as i64, at.y as i64, at.z as i64];
        if let Some(&id) = seen.get(&key(home)) {
            indices.push(id);
            continue;
        }
        let frac = at - at.floor();
        let leans = [lean(frac.x), lean(frac.y), lean(frac.z)];
        let mut found = None;
        for pick in 1..8usize {
            let mut next = home;
            let mut usable = true;
            for axis in 0..3 {
                if pick >> axis & 1 == 1 {
                    usable &= leans[axis] != 0;
                    next[axis] += leans[axis];
                }
            }
            if !usable {
                continue;
            }
            if let Some(&id) = seen.get(&key(next)) {
                if (vertices[id as usize] - p).abs().max_element() <= reach {
                    found = Some(id);
                    break;
                }
            }
        }
        let id = found.unwrap_or_else(|| {
            vertices.push(p);
            vertices.len() as u32 - 1
        });
        seen.insert(key(home), id);
        indices.push(id);
    }
    (vertices, indices)
}

/// True for a closed surface, wound consistently, that encloses negative
/// volume: every triangle faces inward. Anything open is left alone, because
/// the sign of its volume means nothing.
fn is_inside_out(positions: &[Vec3], indices: &[u32]) -> bool {
    let edge = |a: u32, b: u32| (a as u64) << 32 | b as u64;
    let mut forward: Vec<u64> = Vec::with_capacity(indices.len());
    let mut backward: Vec<u64> = Vec::with_capacity(indices.len());
    for t in indices.chunks_exact(3) {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            forward.push(edge(a, b));
            backward.push(edge(b, a));
        }
    }
    forward.par_sort_unstable();
    backward.par_sort_unstable();
    // Closed and consistent: each edge is used once in each direction.
    if forward != backward || forward.windows(2).any(|w| w[0] == w[1]) {
        return false;
    }
    // Measured from a point on the model so a far-off one costs no precision.
    let origin = positions[indices[0] as usize];
    let six_volumes: f64 = indices
        .chunks_exact(3)
        .map(|t| {
            let p = |i: u32| (positions[i as usize] - origin).as_dvec3();
            p(t[0]).dot(p(t[1]).cross(p(t[2])))
        })
        .sum();
    six_volumes < 0.0
}

fn bounds_of(points: &[Vec3]) -> (Vec3, Vec3) {
    points.iter().fold((Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)), |(lo, hi), &p| {
        (lo.min(p), hi.max(p))
    })
}

/// Merges vertices at exactly the same position. OBJ files often split
/// vertices along seams; sculpting needs a connected surface or the mesh tears
/// apart under the brush.
fn weld_exact(positions: Vec<Vec3>, indices: Vec<u32>) -> (Vec<Vec3>, Vec<u32>) {
    let mut map: HashMap<[u32; 3], u32> = HashMap::with_capacity(positions.len());
    let mut out: Vec<Vec3> = Vec::new();
    let mut remap = Vec::with_capacity(positions.len());
    for p in &positions {
        // +0.0 so that -0.0 and 0.0 weld together.
        let key = [(p.x + 0.0).to_bits(), (p.y + 0.0).to_bits(), (p.z + 0.0).to_bits()];
        let id = *map.entry(key).or_insert_with(|| {
            out.push(*p);
            out.len() as u32 - 1
        });
        remap.push(id);
    }
    let indices = indices.iter().map(|&i| remap[i as usize]).collect();
    (out, indices)
}

/// Drops what could only hurt a sculpt: triangles that collapsed to a line or
/// a point (no normal, and a NaN from one spreads to every neighbour at the
/// first smooth) or that touch a coordinate which is not a number, then the
/// vertices nothing uses any more. Order is otherwise kept.
fn tidy(positions: Vec<Vec3>, indices: Vec<u32>, real_size: bool) -> RawMesh {
    let mut used = vec![false; positions.len()];
    let mut kept = Vec::with_capacity(indices.len());
    for t in indices.chunks_exact(3) {
        let sound = t[0] != t[1] && t[1] != t[2] && t[0] != t[2] && t.iter().all(|&i| positions[i as usize].is_finite());
        if sound {
            kept.extend_from_slice(t);
            for &i in t {
                used[i as usize] = true;
            }
        }
    }
    let mut new_id = vec![u32::MAX; positions.len()];
    let mut out = Vec::new();
    for (i, p) in positions.iter().enumerate() {
        if used[i] {
            new_id[i] = out.len() as u32;
            out.push(*p);
        }
    }
    for i in &mut kept {
        *i = new_id[*i as usize];
    }
    RawMesh { positions: out, indices: kept, real_size }
}

/// Readies a freshly imported model to be added as an object, the way the
/// Unity app does, and returns the position to give that object.
///
/// An OBJ carries no units (metres, centimetres and arbitrary units are all
/// common), so it is scaled to the scene: its longest half-side becomes the
/// average width of the first object, `first_half_size` being that object's
/// mesh half-sizes (`None` leaves the size alone). An STL is millimetres by
/// convention and was scaled to app units as it was read, so it keeps its real
/// size and a measurement of it matches the part.
///
/// The model is then *placed* by position, not by moving its vertices: the
/// returned position puts its bounds centre on the scene centre, where every
/// new object arrives. The vertices stay where the file had them, apart from
/// that one uniform scale, so a symmetry plane the modeller built about X = 0
/// is still the mesh's own X = 0.
pub fn place_import(raw: &mut RawMesh, first_half_size: Option<Vec3>) -> Vec3 {
    let (mut lo, mut hi) = bounds_of(&raw.positions);
    if !raw.real_size {
        if let Some(first) = first_half_size {
            let target = (first.x + first.y + first.z) / 3.0 * 2.0;
            let model = ((hi - lo) * 0.5).max_element();
            if model > 1e-5 && target > 1e-5 {
                let scale = target / model;
                for p in &mut raw.positions {
                    *p *= scale;
                }
                (lo, hi) = (lo * scale, hi * scale);
            }
        }
    }
    -(lo + hi) * 0.5
}

// ---------------------------------------------------------------- .sclp files
//
// A `.sclp` file is a whole scene. After the eight bytes "SCLP0004" it is a
// run of blocks, each a four-letter tag, the length of its body (u64) and
// the body:
//
//   MESH  one per mesh, in order (the first is mesh 0): vertex count (u64),
//         index count (u64), the positions (three f32 each), the triangle
//         indices (u32 each). Raw arrays, so a mesh of any size is written
//         and read as fast as the disk allows, with no text in between.
//   MASK  after its mesh, if anything on it is masked: the mesh's number
//         (u32), then one f32 per vertex.
//   QUAD  after its mesh, if it was quad-remeshed: the mesh's number (u32),
//         face count and corner count (u64 each), where each face starts
//         (u32, one more of them than faces), the corners (u32 each).
//   HIDE  after its mesh, if any of its faces are hidden: the mesh's number
//         (u32), then a byte per triangle, 1 for hidden.
//   INFO  last, once: everything else, as JSON (see `Details`).
//
// All numbers are little-endian. A reader steps over blocks whose tag it does
// not know and JSON fields it does not know, and fills in whatever is
// missing, so neither kind of addition needs a new version number. Something
// per mesh and too big for text gets a tag of its own (`HIDE` is the pattern
// to copy: a constant here, a few lines in `write_scene`, a branch in
// `read_blocks` and a field of `LoadedMesh`); anything small gets a field in
// the JSON.
//
// The older layouts ("SCLP0001" to "SCLP0003": fixed records, with no room
// to grow) are still read, never written.

const NATIVE_MAGIC: &[u8; 8] = b"SCLP0001";
/// Scenes from before objects could be turned or scaled.
const SCENE_MAGIC_PLACED: &[u8; 8] = b"SCLP0002";
/// Scenes of fixed records: every mesh, then every object, and nothing else.
const SCENE_MAGIC_FIXED: &[u8; 8] = b"SCLP0003";
/// Scenes of named blocks; what is written today.
const SCENE_MAGIC: &[u8; 8] = b"SCLP0004";

const MESH: &[u8; 4] = b"MESH";
const MASK: &[u8; 4] = b"MASK";
const QUAD: &[u8; 4] = b"QUAD";
const HIDE: &[u8; 4] = b"HIDE";
const INFO: &[u8; 4] = b"INFO";

// ---- writing a file without harming the one already there

/// The name a file is written under until it is whole: beside the real one,
/// so that putting it in place is a rename on the same disk.
fn temp_beside(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}

/// Writes all of a new file under a temporary name beside `path`, and waits
/// until it is on the disk itself, not only in the system's cache. `path` is
/// not touched: if anything here fails, or the power goes, the file that was
/// there is still there, whole. Returns the temporary file for [`commit`].
fn write_beside(path: &Path, write: impl FnOnce(&mut BufWriter<File>) -> std::io::Result<()>) -> Result<PathBuf> {
    let temp = temp_beside(path);
    let written = (|| -> std::io::Result<()> {
        if let Some(folder) = path.parent().filter(|f| !f.as_os_str().is_empty()) {
            std::fs::create_dir_all(folder)?;
        }
        let mut w = BufWriter::with_capacity(1 << 20, File::create(&temp)?);
        write(&mut w)?;
        w.into_inner().map_err(|e| e.into_error())?.sync_all()
    })();
    match written {
        Ok(()) => Ok(temp),
        Err(e) => {
            let _ = std::fs::remove_file(&temp);
            Err(format!("Could not write {}: {e}", path.display()))
        }
    }
}

/// Puts a finished temporary file in place of `path` as one rename, so at
/// every moment `path` holds either the old file or the new one, whole.
fn commit(temp: &Path, path: &Path) -> Result<()> {
    std::fs::rename(temp, path).map_err(|e| {
        let _ = std::fs::remove_file(temp);
        format!("Could not write {}: {e}", path.display())
    })
}

/// Writes a file so that the one already at `path` is never left damaged:
/// the new one is written in full beside it, then swapped in. A full disk,
/// a pulled drive or a crash part-way leaves the old file as it was.
pub(crate) fn replace_file(path: &Path, write: impl FnOnce(&mut BufWriter<File>) -> std::io::Result<()>) -> Result<()> {
    let temp = write_beside(path, write)?;
    commit(&temp, path)
}

// ---- JSON that any version can read

/// Lays the JSON from a file over the JSON of a complete value, field by
/// field: what the file has replaces what is there, what it lacks stays, and
/// a field of the wrong kind (a word where a number belongs) is passed over.
/// `null` counts as lacking; it is also how JSON writes a number that was
/// not a number.
fn merge(base: &mut Value, file: &Value) {
    match (base, file) {
        (Value::Object(base), Value::Object(file)) => {
            for (key, value) in file {
                match base.get_mut(key) {
                    Some(slot) => merge(slot, value),
                    None => {
                        base.insert(key.clone(), value.clone());
                    }
                }
            }
        }
        (_, Value::Null) => {}
        (base, file) => {
            if base.is_null() || std::mem::discriminant(&*base) == std::mem::discriminant(file) {
                *base = file.clone();
            }
        }
    }
}

/// Reads a `T` out of JSON written by any version of the app. Fields the
/// JSON lacks are taken from `base`, fields `T` does not have are ignored,
/// and a field that still does not read (a name this version has never heard
/// of, say) is left as `base` has it without costing the fields around it.
pub(crate) fn read_over<T: Serialize + DeserializeOwned>(base: &T, file: &Value) -> Option<T> {
    let mut kept = serde_json::to_value(base).ok()?;
    let mut merged = kept.clone();
    merge(&mut merged, file);
    if let Ok(value) = T::deserialize(&merged) {
        return Some(value);
    }
    // Something in it does not read: go through it part by part, and inside
    // the parts that fail, keeping everything that does read.
    let reads = |whole: &Value| T::deserialize(whole).is_ok();
    settle(&mut kept, &mut Vec::new(), &merged, &reads);
    T::deserialize(&kept).ok()
}

/// Tries `wanted` at `path` inside `whole`. If the whole no longer reads,
/// what was there goes back, and where that was a set of fields each field
/// of `wanted` is tried on its own.
fn settle(whole: &mut Value, path: &mut Vec<String>, wanted: &Value, reads: &dyn Fn(&Value) -> bool) {
    let before = put_at(whole, path, Some(wanted.clone()));
    if reads(whole) {
        return;
    }
    let had_fields = matches!(before, Some(Value::Object(_)));
    put_at(whole, path, before);
    if let (true, Value::Object(fields)) = (had_fields, wanted) {
        for (key, value) in fields {
            path.push(key.clone());
            settle(whole, path, value, reads);
            path.pop();
        }
    }
}

/// Puts `value` at `path` inside `whole` (`None` takes away what is there)
/// and gives back what it replaced.
fn put_at(whole: &mut Value, path: &[String], value: Option<Value>) -> Option<Value> {
    let Some((last, parents)) = path.split_last() else {
        return value.map(|value| std::mem::replace(whole, value));
    };
    let mut at = whole;
    for key in parents {
        at = at.get_mut(key)?;
    }
    let fields = at.as_object_mut()?;
    match value {
        Some(value) => fields.insert(last.clone(), value),
        None => fields.remove(last),
    }
}

// ---- what a file keeps besides its meshes

/// Names of the parts kept in a file's [`Details`].
pub mod slot {
    // For the whole scene. Written and read by `src/app/files.rs`.
    /// [`super::Shading`]: matcap, base colour, cavity, background.
    pub const SHADING: &str = "shading";
    /// `look::Look`: finish, lighting, HDRI, post effects.
    pub const LOOK: &str = "look";
    /// `camera::SavedView`.
    pub const CAMERA: &str = "camera";
    /// [`super::Mirror`]: the brush mirror axes, radial symmetry and their guides.
    pub const MIRROR: &str = "mirror";

    // Per object, under its row of the object list. `MIRROR_COPIES` is
    // written and read by `scene/mirror.rs`; the others are for features that
    // are still being ported: nothing writes them yet, the names are only
    // spoken for.
    /// An object's own symmetry settings (mirror axes, radial count, space).
    #[allow(dead_code)]
    pub const SYMMETRY: &str = "symmetry";
    /// A live array of the object: kind, count, handle points.
    #[allow(dead_code)]
    pub const ARRAY: &str = "array";
    /// Live mirror copies: axes, and the point their planes pass through.
    pub const MIRROR_COPIES: &str = "mirror_copies";
}

/// Fields of the JSON that [`Details`] does not hand out: the file's own.
const SCENE_FIELDS: [&str; 4] = ["app", "saved", "active", "objects"];
const OBJECT_FIELDS: [&str; 6] = ["name", "visible", "mesh", "position", "rotation", "scale"];

/// Everything a `.sclp` file keeps that is not a mesh or an object's place
/// in the scene: the look, the camera, the tools' state. It is the file's
/// JSON section, by name, so a part can be added without touching the format
/// or this file. Every part is optional: a file from before a part existed
/// gives `None` for it, and the app keeps what it has.
///
/// To keep something new, give it a name and two lines in
/// `src/app/files.rs`. Say each object gains symmetry settings of its own:
///
/// ```ignore
/// details.set_object(i, io::slot::SYMMETRY, &object.symmetry);                 // in `Running::details`
/// if let Some(s) = details.get_object(i, io::slot::SYMMETRY) { object.symmetry = s; }   // in `Running::apply_details`
/// ```
///
/// The value only has to derive `Serialize` and `Deserialize`. Give its
/// fields `#[serde(default)]` (or read it with [`Details::get_over`]) and a
/// later version can add to it freely.
#[derive(Clone, Debug, Default)]
pub struct Details {
    scene: Map<String, Value>,
    /// In the order of the object list. May be shorter than it.
    objects: Vec<Map<String, Value>>,
}

impl Details {
    /// Keeps `value` for the whole scene under the name `slot`.
    pub fn set<T: Serialize>(&mut self, slot: &str, value: &T) {
        debug_assert!(!SCENE_FIELDS.contains(&slot), "{slot} is one of the file's own fields");
        if let Ok(value) = serde_json::to_value(value) {
            self.scene.insert(slot.to_owned(), value);
        }
    }

    /// What the file kept under `slot`, if it kept anything that reads.
    pub fn get<T: DeserializeOwned>(&self, slot: &str) -> Option<T> {
        T::deserialize(self.scene.get(slot)?).ok()
    }

    /// The same, with whatever the file lacks filled in from `base` (see
    /// [`read_over`]): for parts with many fields that grow over time.
    pub fn get_over<T: Serialize + DeserializeOwned>(&self, slot: &str, base: &T) -> Option<T> {
        read_over(base, self.scene.get(slot)?)
    }

    /// Keeps `value` for one object (by its row in the object list).
    #[allow(dead_code)] // for the per-object slots above
    pub fn set_object<T: Serialize>(&mut self, object: usize, slot: &str, value: &T) {
        debug_assert!(!OBJECT_FIELDS.contains(&slot), "{slot} is one of the file's own fields");
        if let Ok(value) = serde_json::to_value(value) {
            if self.objects.len() <= object {
                self.objects.resize_with(object + 1, Map::new);
            }
            self.objects[object].insert(slot.to_owned(), value);
        }
    }

    /// What the file kept for one object under `slot`.
    #[allow(dead_code)]
    pub fn get_object<T: DeserializeOwned>(&self, object: usize, slot: &str) -> Option<T> {
        T::deserialize(self.objects.get(object)?.get(slot)?).ok()
    }

    /// The JSON section of a file: the objects, then every part kept here.
    fn document(&self, objects: &[SavedObject], active: usize) -> Vec<u8> {
        let mut root = self.scene.clone();
        // Who wrote it and when, for a person reading the file; nothing reads these back.
        root.insert("app".into(), concat!("Sculpt ", env!("CARGO_PKG_VERSION")).into());
        root.insert("saved".into(), time_stamp().into());
        root.insert("active".into(), active.into());
        let entries = objects
            .iter()
            .enumerate()
            .map(|(row, o)| {
                let mut entry = self.objects.get(row).cloned().unwrap_or_default();
                entry.insert("name".into(), o.name.as_str().into());
                entry.insert("visible".into(), o.visible.into());
                entry.insert("mesh".into(), o.mesh.into());
                entry.insert("position".into(), json!(o.position.to_array()));
                entry.insert("rotation".into(), json!(o.rotation.to_array()));
                entry.insert("scale".into(), json!(o.scale.to_array()));
                Value::Object(entry)
            })
            .collect();
        root.insert("objects".into(), Value::Array(entries));
        serde_json::to_vec(&Value::Object(root)).unwrap_or_default()
    }
}

/// The half of the look that lives in `doc::Options` rather than in
/// `look::Look`: how the surface is shaded and what is behind it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shading {
    pub matcap_on: bool,
    /// The matcap picture, as "Category/Name". Empty, or one this computer
    /// does not have, leaves the matcap as it is.
    pub matcap: String,
    pub flat_shading: bool,
    pub cavity: bool,
    pub cavity_ridge: f32,
    pub cavity_valley: f32,
    pub base_color: [f32; 3],
    pub smoothness: f32,
    pub gradient_background: bool,
    pub background: [f32; 3],
    pub background_top: [f32; 3],
}

impl Default for Shading {
    fn default() -> Self {
        Self::of(&Options::default(), String::new())
    }
}

impl Shading {
    pub fn of(o: &Options, matcap: String) -> Self {
        Self {
            matcap_on: o.matcap_on,
            matcap,
            flat_shading: o.flat_shading,
            cavity: o.cavity,
            cavity_ridge: o.cavity_ridge,
            cavity_valley: o.cavity_valley,
            base_color: o.base_color,
            smoothness: o.smoothness,
            gradient_background: o.gradient_background,
            background: o.background,
            background_top: o.background_top,
        }
    }

    /// Everything but the matcap picture, which the app has to look up.
    pub fn apply(&self, o: &mut Options) {
        let unit = |c: [f32; 3]| c.map(|v| v.clamp(0.0, 1.0));
        o.matcap_on = self.matcap_on;
        o.flat_shading = self.flat_shading;
        o.cavity = self.cavity;
        o.cavity_ridge = self.cavity_ridge.clamp(0.0, 2.0);
        o.cavity_valley = self.cavity_valley.clamp(0.0, 2.0);
        o.base_color = unit(self.base_color);
        o.smoothness = self.smoothness.clamp(0.0, 1.0);
        o.gradient_background = self.gradient_background;
        o.background = unit(self.background);
        o.background_top = unit(self.background_top);
    }
}

/// The brush symmetry: which axes strokes are mirrored across, how they are
/// repeated round an axis, and whether the guides are drawn.
///
/// `radial` came after the rest. A file from before it reads as radial
/// symmetry switched off, with the count and axis the app starts with.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Mirror {
    pub x: bool,
    pub y: bool,
    pub z: bool,
    pub guides: bool,
    pub radial: crate::sculpt::Radial,
}

impl Default for Mirror {
    fn default() -> Self {
        Self { x: false, y: false, z: false, guides: Options::default().symmetry_guides, radial: Default::default() }
    }
}

/// How `look::Look` writes a `Vec3`: three numbers.
pub mod vec3 {
    use glam::Vec3;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(v: &Vec3, s: S) -> std::result::Result<S::Ok, S::Error> {
        v.to_array().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Vec3, D::Error> {
        <[f32; 3]>::deserialize(d).map(Vec3::from_array)
    }
}

/// How `look::Look` writes which preset is picked: by the preset's id, not
/// its place in the list, so a file still means the same preset after the
/// list has been added to or reordered.
pub mod preset {
    use serde::{Deserialize, Deserializer, Serializer};

    fn write<S: Serializer>(ids: &[&str], index: usize, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(ids[index.clamp(0, ids.len() - 1)])
    }

    fn read<'de, D: Deserializer<'de>>(ids: &[&str], d: D) -> std::result::Result<usize, D::Error> {
        let id = String::deserialize(d)?;
        let place = ids.iter().position(|known| *known == id);
        place.ok_or_else(|| serde::de::Error::custom("a preset this version does not have"))
    }

    macro_rules! by_id {
        ($($name:ident: $table:ident),* $(,)?) => {$(
            pub mod $name {
                pub fn serialize<S: serde::Serializer>(index: &usize, s: S) -> std::result::Result<S::Ok, S::Error> {
                    super::write(&crate::look::$table.each_ref().map(|p| p.id), *index, s)
                }

                pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<usize, D::Error> {
                    super::read(&crate::look::$table.each_ref().map(|p| p.id), d)
                }
            }
        )*};
    }

    by_id!(lighting: LIGHTING, lure: LURES, metal: METALS, clay: CLAYS, wood: WOODS, glow: GLOWS);
}

/// One row of a saved scene; `mesh` indexes the file's mesh list, so
/// instances come back sharing a mesh.
pub struct SavedObject {
    pub name: String,
    pub visible: bool,
    pub mesh: usize,
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

pub struct SavedScene {
    pub meshes: Vec<Mesh>,
    pub objects: Vec<SavedObject>,
    pub active: usize,
    /// The look, camera and tool state the file carried. Empty for a file
    /// from before these were kept.
    pub details: Details,
}

// ---- the one-mesh file from before scenes existed

/// Native save: the exact mesh and its mask, as raw arrays. Loads back
/// instantly and loses nothing, unlike a round trip through OBJ.
#[cfg_attr(not(test), allow(dead_code))] // the app saves whole scenes; this is the old one-mesh file
pub fn save_native(mesh: &Mesh, path: &Path) -> Result<()> {
    let io = |e: std::io::Error| format!("Could not write {}: {e}", path.display());
    let mut w = BufWriter::with_capacity(1 << 20, File::create(path).map_err(io)?);
    w.write_all(NATIVE_MAGIC).map_err(io)?;
    w.write_all(&(mesh.positions.len() as u64).to_le_bytes()).map_err(io)?;
    w.write_all(&(mesh.indices.len() as u64).to_le_bytes()).map_err(io)?;
    w.write_all(bytemuck::cast_slice(&mesh.positions)).map_err(io)?;
    w.write_all(bytemuck::cast_slice(&mesh.indices)).map_err(io)?;
    w.write_all(bytemuck::cast_slice(&mesh.mask)).map_err(io)?;
    w.flush().map_err(io)
}

pub fn load_native(path: &Path) -> Result<Mesh> {
    let bad = || format!("{} is not a sculpt file", path.display());
    let bytes = std::fs::read(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    if bytes.len() < 24 || &bytes[..8] != NATIVE_MAGIC {
        return Err(bad());
    }
    let count = |at: usize| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap()) as usize;
    let (verts, indices) = (count(8), count(16));
    let need = verts
        .checked_mul(16)
        .and_then(|v| indices.checked_mul(4).and_then(|i| v.checked_add(i)))
        .and_then(|n| n.checked_add(24));
    if need != Some(bytes.len()) {
        return Err(bad());
    }
    // Copy out of the byte buffer rather than cast it: a file read is not
    // guaranteed to be aligned for f32/u32.
    let floats = |at: usize, n: usize| -> Vec<f32> {
        bytes[at..at + n * 4].chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect()
    };
    let positions: Vec<Vec3> =
        floats(24, verts * 3).chunks_exact(3).map(|c| Vec3::new(c[0], c[1], c[2])).collect();
    let at = 24 + verts * 12;
    let index_data: Vec<u32> = bytes[at..at + indices * 4]
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
        .collect();
    let mask = floats(at + indices * 4, verts);
    if positions.iter().any(|p| !p.is_finite()) {
        return Err(bad());
    }
    Ok(Mesh::new(positions, index_data, Some(mask)))
}

// ---- writing a scene

fn block(w: &mut impl Write, tag: &[u8; 4], length: usize) -> std::io::Result<()> {
    w.write_all(tag)?;
    w.write_all(&(length as u64).to_le_bytes())
}

/// Native save of every object: each mesh once (exact, with its mask, its
/// quads and which of its faces are hidden), then the object list. The file already at `path` is replaced
/// only once the new one is whole (see [`replace_file`]).
#[cfg_attr(not(test), allow(dead_code))] // the app always has details to keep
pub fn save_scene(meshes: &[&Mesh], objects: &[SavedObject], active: usize, path: &Path) -> Result<()> {
    save_scene_with(meshes, objects, active, &Details::default(), path)
}

/// [`save_scene`], with the look, camera and tool state to keep alongside.
pub fn save_scene_with(
    meshes: &[&Mesh],
    objects: &[SavedObject],
    active: usize,
    details: &Details,
    path: &Path,
) -> Result<()> {
    let info = details.document(objects, active);
    replace_file(path, |w| write_scene(w, meshes, &info))
}

/// The bytes of a scene file: the magic, each mesh's blocks, then `info`,
/// the JSON section.
fn write_scene(w: &mut impl Write, meshes: &[&Mesh], info: &[u8]) -> std::io::Result<()> {
    w.write_all(SCENE_MAGIC)?;
    for (number, mesh) in meshes.iter().enumerate() {
        let number = (number as u32).to_le_bytes();
        let positions: &[u8] = bytemuck::cast_slice(&mesh.positions);
        let indices: &[u8] = bytemuck::cast_slice(&mesh.indices);
        block(w, MESH, 16 + positions.len() + indices.len())?;
        w.write_all(&(mesh.positions.len() as u64).to_le_bytes())?;
        w.write_all(&(mesh.indices.len() as u64).to_le_bytes())?;
        w.write_all(positions)?;
        w.write_all(indices)?;
        // Most meshes have nothing masked, and then a mask is four bytes a
        // vertex of nothing.
        if mesh.mask.len() == mesh.positions.len() && mesh.mask.iter().any(|&m| m != 0.0) {
            let mask: &[u8] = bytemuck::cast_slice(&mesh.mask);
            block(w, MASK, 4 + mask.len())?;
            w.write_all(&number)?;
            w.write_all(mask)?;
        }
        if let Some(polys) = &mesh.polys {
            let start: &[u8] = bytemuck::cast_slice(&polys.start);
            let corners: &[u8] = bytemuck::cast_slice(&polys.indices);
            block(w, QUAD, 4 + 16 + start.len() + corners.len())?;
            w.write_all(&number)?;
            w.write_all(&(polys.start.len().saturating_sub(1) as u64).to_le_bytes())?;
            w.write_all(&(polys.indices.len() as u64).to_le_bytes())?;
            w.write_all(start)?;
            w.write_all(corners)?;
        }
        if let Some(hidden) = mesh.hidden_triangles() {
            let flags: Vec<u8> = hidden.iter().map(|&h| h as u8).collect();
            block(w, HIDE, 4 + flags.len())?;
            w.write_all(&number)?;
            w.write_all(&flags)?;
        }
    }
    block(w, INFO, info.len())?;
    w.write_all(info)
}

// ---- reading a scene

/// A mesh as its blocks gave it, before the sculpting structures are built.
struct LoadedMesh {
    positions: Vec<Vec3>,
    indices: Vec<u32>,
    mask: Option<Vec<f32>>,
    polys: Option<Polys>,
    /// One flag per triangle of `indices`, where any are hidden.
    hidden: Option<Vec<bool>>,
}

impl LoadedMesh {
    fn build(self) -> Mesh {
        let verts = self.positions.len();
        let sound = |p: &Polys| {
            p.start.first() == Some(&0)
                && p.start.last().map(|&end| end as usize) == Some(p.indices.len())
                && p.start.windows(2).all(|w| w[0] <= w[1])
                && p.indices.iter().all(|&i| (i as usize) < verts)
        };
        let Some(polys) = self.polys.filter(sound) else {
            return match self.hidden {
                Some(hidden) => Mesh::new_with_hidden(self.positions, self.indices, self.mask, &hidden),
                None => Mesh::new(self.positions, self.indices, self.mask),
            };
        };
        // Building a mesh reorders its vertices; the quads' corners follow.
        let saved = self.hidden.map(|hidden| (self.indices.clone(), hidden));
        let (mut mesh, moved) = Mesh::new_mapped(self.positions, self.indices, self.mask);
        let went = |i: u32| moved.get(i as usize).copied().unwrap_or(u32::MAX);
        let corners: Vec<u32> = polys.indices.iter().map(|&i| went(i)).collect();
        if !corners.contains(&u32::MAX) {
            mesh.polys = Some(Polys { start: polys.start, indices: corners });
        }
        if let Some((indices, hidden)) = saved {
            // It reorders the triangles too, and only tells where the
            // vertices went, so each triangle is found again by its corners.
            // (A quad-remeshed mesh is small; this costs nothing to speak of.)
            let key = |a: u32, b: u32, c: u32| {
                let mut corners = [a, b, c];
                corners.sort_unstable();
                corners
            };
            let was: HashMap<[u32; 3], bool> =
                indices.chunks_exact(3).zip(&hidden).map(|(t, &flag)| (key(went(t[0]), went(t[1]), went(t[2])), flag)).collect();
            let flags: Vec<bool> =
                mesh.indices.chunks_exact(3).map(|t| was.get(&key(t[0], t[1], t[2])).copied().unwrap_or(false)).collect();
            if flags.contains(&true) {
                mesh.restore_hidden_triangles(&flags);
            }
        }
        mesh
    }
}

/// A block-format file as read, with its meshes still raw.
struct Loaded {
    meshes: Vec<LoadedMesh>,
    objects: Vec<SavedObject>,
    active: usize,
    details: Details,
}

impl Loaded {
    /// Builds the meshes (their spatial index, adjacency and normals: the
    /// slow part of opening a large scene), several at once.
    fn into_scene(self) -> SavedScene {
        let meshes = self.meshes.into_par_iter().map(LoadedMesh::build).collect();
        SavedScene { meshes, objects: self.objects, active: self.active, details: self.details }
    }
}

/// Reads `count` values straight into the vector they will live in: one
/// copy off the disk, nothing in between.
fn read_array<T: bytemuck::Pod>(r: &mut impl Read, count: usize) -> std::io::Result<Vec<T>> {
    let mut values = vec![<T as bytemuck::Zeroable>::zeroed(); count];
    r.read_exact(bytemuck::cast_slice_mut(&mut values))?;
    Ok(values)
}

fn read_u32(r: &mut impl Read) -> Option<u32> {
    let mut bytes = [0u8; 4];
    r.read_exact(&mut bytes).ok()?;
    Some(u32::from_le_bytes(bytes))
}

fn read_u64(r: &mut impl Read) -> Option<u64> {
    let mut bytes = [0u8; 8];
    r.read_exact(&mut bytes).ok()?;
    Some(u64::from_le_bytes(bytes))
}

/// The body of a `MESH` block, `length` bytes long. The counts inside must
/// add up to exactly that, and the caller has checked that many bytes are
/// really in the file, so a damaged count can never ask for more memory than
/// the file is big.
fn read_mesh(r: &mut impl Read, length: u64) -> Option<LoadedMesh> {
    let (verts, indices) = (read_u64(r)?, read_u64(r)?);
    let need = verts.checked_mul(12)?.checked_add(indices.checked_mul(4)?)?.checked_add(16)?;
    if need != length {
        return None;
    }
    let positions = read_array::<Vec3>(r, verts as usize).ok()?;
    let indices = read_array::<u32>(r, indices as usize).ok()?;
    Some(LoadedMesh { positions, indices, mask: None, polys: None, hidden: None })
}

/// The body of a `QUAD` block, and the mesh it belongs to.
fn read_quads(r: &mut impl Read, length: u64) -> Option<(usize, Polys)> {
    let number = read_u32(r)? as usize;
    let (faces, corners) = (read_u64(r)?, read_u64(r)?);
    let starts = faces.checked_add(1)?;
    let need = starts.checked_mul(4)?.checked_add(corners.checked_mul(4)?)?.checked_add(20)?;
    if need != length {
        return None;
    }
    let start = read_array::<u32>(r, starts as usize).ok()?;
    let indices = read_array::<u32>(r, corners as usize).ok()?;
    Some((number, Polys { start, indices }))
}

/// One entry of the JSON object list. The mesh it names has to exist; a
/// placement that makes no sense falls back to "where the mesh itself sits"
/// rather than costing the whole file.
fn saved_object(entry: &Map<String, Value>, mesh_count: usize) -> Option<SavedObject> {
    let floats = |key: &str, count: usize| -> Option<Vec<f32>> {
        let list = entry.get(key)?.as_array()?;
        let values: Vec<f32> = list.iter().filter_map(|v| v.as_f64()).map(|v| v as f32).collect();
        (list.len() == count && values.len() == count).then_some(values)
    };
    let mesh = entry.get("mesh")?.as_u64()? as usize;
    if mesh >= mesh_count {
        return None;
    }
    let position = floats("position", 3).map(|p| Vec3::new(p[0], p[1], p[2])).filter(|p| p.is_finite());
    let rotation = floats("rotation", 4)
        .map(|q| Quat::from_xyzw(q[0], q[1], q[2], q[3]))
        .filter(|q| q.is_finite() && q.length() > 0.5);
    let scale = floats("scale", 3)
        .map(|s| Vec3::new(s[0], s[1], s[2]))
        .filter(|s| s.is_finite() && s.min_element() > 0.0);
    Some(SavedObject {
        name: entry.get("name").and_then(Value::as_str).unwrap_or("Object").to_owned(),
        visible: entry.get("visible").and_then(Value::as_bool).unwrap_or(true),
        mesh,
        position: position.unwrap_or(Vec3::ZERO),
        rotation: rotation.map_or(Quat::IDENTITY, Quat::normalize),
        scale: scale.unwrap_or(Vec3::ONE),
    })
}

/// Reads a block-format file (everything after its magic).
fn read_blocks(file: File, path: &Path) -> Result<Loaded> {
    let bad = || format!("{} is damaged (it ends early, or its parts do not add up), so it was not opened.", path.display());
    let total = file.metadata().map_err(|e| format!("Could not read {}: {e}", path.display()))?.len();
    let mut r = BufReader::with_capacity(1 << 16, file);
    let mut at = SCENE_MAGIC.len() as u64;
    let mut meshes: Vec<LoadedMesh> = Vec::new();
    let mut info = None;
    while at < total {
        let mut head = [0u8; 12];
        r.read_exact(&mut head).map_err(|_| bad())?;
        let length = u64::from_le_bytes(head[4..].try_into().unwrap());
        at += 12;
        // A block cannot be longer than what is left of the file.
        if at > total || length > total - at {
            return Err(bad());
        }
        let tag = &head[..4];
        if tag == MESH {
            meshes.push(read_mesh(&mut r, length).ok_or_else(bad)?);
        } else if tag == MASK {
            let number = read_u32(&mut r).ok_or_else(bad)? as usize;
            let mesh = meshes.get_mut(number).ok_or_else(bad)?;
            let verts = mesh.positions.len();
            if length != 4 + verts as u64 * 4 {
                return Err(bad());
            }
            mesh.mask = Some(read_array(&mut r, verts).map_err(|_| bad())?);
        } else if tag == QUAD {
            let (number, polys) = read_quads(&mut r, length).ok_or_else(bad)?;
            meshes.get_mut(number).ok_or_else(bad)?.polys = Some(polys);
        } else if tag == HIDE {
            let number = read_u32(&mut r).ok_or_else(bad)? as usize;
            let mesh = meshes.get_mut(number).ok_or_else(bad)?;
            let triangles = mesh.indices.len() / 3;
            if length != 4 + triangles as u64 {
                return Err(bad());
            }
            let flags: Vec<u8> = read_array(&mut r, triangles).map_err(|_| bad())?;
            mesh.hidden = Some(flags.iter().map(|&flag| flag != 0).collect());
        } else if tag == INFO {
            let mut bytes = vec![0u8; length as usize];
            r.read_exact(&mut bytes).map_err(|_| bad())?;
            info = Some(bytes);
        } else {
            // A block from a later version: step over it.
            r.seek_relative(length as i64).map_err(|_| bad())?;
        }
        at += length;
    }
    // The JSON goes in last, so a file cut short anywhere is missing it.
    let info = info.ok_or_else(bad)?;
    let Ok(Value::Object(mut root)) = serde_json::from_slice::<Value>(&info) else { return Err(bad()) };
    if meshes.par_iter().any(|m| m.positions.par_iter().any(|p| !p.is_finite())) {
        return Err(bad());
    }
    let Some(Value::Array(entries)) = root.remove("objects") else { return Err(bad()) };
    let mut objects = Vec::new();
    let mut kept = Vec::new();
    for entry in entries {
        let Value::Object(mut entry) = entry else { return Err(bad()) };
        objects.push(saved_object(&entry, meshes.len()).ok_or_else(bad)?);
        entry.retain(|key, _| !OBJECT_FIELDS.contains(&key.as_str()));
        kept.push(entry);
    }
    if objects.is_empty() {
        return Err(bad());
    }
    let active = root.get("active").and_then(Value::as_u64).unwrap_or(0) as usize;
    let active = active.clamp(0, objects.len() - 1);
    root.retain(|key, _| !SCENE_FIELDS.contains(&key.as_str()));
    Ok(Loaded { meshes, objects, active, details: Details { scene: root, objects: kept } })
}

/// Opens a native file of any version. A file from before scenes existed
/// holds one mesh and loads as a single object named after the file.
pub fn load_scene(path: &Path) -> Result<SavedScene> {
    let mut file = File::open(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    let mut magic = [0u8; 8];
    if file.read_exact(&mut magic).is_err() {
        return Err(format!("{} is not a sculpt file", path.display()));
    }
    if &magic == SCENE_MAGIC {
        return Ok(read_blocks(file, path)?.into_scene());
    }
    drop(file);
    let newer = magic.starts_with(b"SCLP") && magic[4..].iter().all(u8::is_ascii_digit) && magic[4..] > SCENE_MAGIC[4..];
    if newer {
        return Err(format!("{} was saved by a newer version of Sculpt, which this one cannot open.", path.display()));
    }
    load_fixed(path)
}

/// The layouts before blocks: "SCLP0001" (one mesh), "SCLP0002" and
/// "SCLP0003" (fixed records). They carry no look or camera.
fn load_fixed(path: &Path) -> Result<SavedScene> {
    let bad = || format!("{} is not a sculpt file", path.display());
    let bytes = std::fs::read(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    if bytes.len() >= 8 && &bytes[..8] == NATIVE_MAGIC {
        let name = path.file_stem().map_or("Sculpt".into(), |s| s.to_string_lossy().into_owned());
        let only =
            SavedObject { name, visible: true, mesh: 0, position: Vec3::ZERO, rotation: Quat::IDENTITY, scale: Vec3::ONE };
        return Ok(SavedScene { meshes: vec![load_native(path)?], objects: vec![only], active: 0, details: Details::default() });
    }
    if bytes.len() < 20 || (&bytes[..8] != SCENE_MAGIC_FIXED && &bytes[..8] != SCENE_MAGIC_PLACED) {
        return Err(bad());
    }
    let turned = &bytes[..8] == SCENE_MAGIC_FIXED;
    let mut at = 8usize;
    let mut take = |n: usize| -> Result<&[u8]> {
        let chunk = at.checked_add(n).and_then(|end| bytes.get(at..end)).ok_or_else(bad)?;
        at += n;
        Ok(chunk)
    };
    let u32s = |b: &[u8]| -> Vec<u32> {
        b.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect()
    };
    let f32s = |b: &[u8]| -> Vec<f32> {
        b.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect()
    };
    let head = u32s(take(12)?);
    let (mesh_count, object_count, active) = (head[0] as usize, head[1] as usize, head[2] as usize);
    // Counts come from the file: never reserve by them, only read what is there.
    let mut meshes = Vec::new();
    for _ in 0..mesh_count {
        let sizes = take(16)?;
        let verts = u64::from_le_bytes(sizes[..8].try_into().unwrap()) as usize;
        let indices = u64::from_le_bytes(sizes[8..].try_into().unwrap()) as usize;
        let floats = f32s(take(verts.checked_mul(12).ok_or_else(bad)?)?);
        let positions: Vec<Vec3> = floats.chunks_exact(3).map(|c| Vec3::new(c[0], c[1], c[2])).collect();
        let index_data = u32s(take(indices.checked_mul(4).ok_or_else(bad)?)?);
        let mask = f32s(take(verts.checked_mul(4).ok_or_else(bad)?)?);
        if positions.iter().any(|p| !p.is_finite()) {
            return Err(bad());
        }
        meshes.push(Mesh::new(positions, index_data, Some(mask)));
    }
    let mut objects = Vec::new();
    for _ in 0..object_count {
        let len = u32s(take(4)?)[0] as usize;
        let name = String::from_utf8(take(len)?.to_vec()).map_err(|_| bad())?;
        let visible = take(1)?[0] != 0;
        let mesh = u32s(take(4)?)[0] as usize;
        let p = f32s(take(12)?);
        let position = Vec3::new(p[0], p[1], p[2]);
        let (mut rotation, mut scale) = (Quat::IDENTITY, Vec3::ONE);
        if turned {
            let v = f32s(take(28)?);
            rotation = Quat::from_xyzw(v[0], v[1], v[2], v[3]);
            scale = Vec3::new(v[4], v[5], v[6]);
        }
        let sound = position.is_finite() && rotation.is_finite() && rotation.length() > 0.5 && scale.is_finite();
        if mesh >= meshes.len() || !sound || scale.min_element() <= 0.0 {
            return Err(bad());
        }
        objects.push(SavedObject { name, visible, mesh, position, rotation: rotation.normalize(), scale });
    }
    if objects.is_empty() || active >= objects.len() {
        return Err(bad());
    }
    Ok(SavedScene { meshes, objects, active, details: Details::default() })
}

fn nothing_to_export(mesh: &Mesh) -> Result<()> {
    if mesh.indices.is_empty() {
        return Err("There is nothing to export: this object has no surface.".into());
    }
    Ok(())
}

/// Wavefront OBJ of one object as it sits in the scene. `model` is the
/// object's own space into world space (`Object::matrix`), so its position,
/// rotation and scale are baked into the vertices and normals; an instance
/// passes its own, and comes out as its own placed copy. Lengths are the app's
/// own units (OBJ carries none) and the axes are left alone, because OBJ is
/// right-handed with Y up, like this app. A quad-remeshed mesh goes out as its
/// quads.
#[allow(dead_code)] // the app writes placed copies (`export_obj_placed`); this is the one-place form
pub fn export_obj(mesh: &Mesh, model: Mat4, path: &Path) -> Result<()> {
    export_obj_placed(mesh, &[("", model)], path)
}

/// [`export_obj`] for a mesh drawn in several places at once (an object and
/// its live mirror copies): one `o` block of its own for each, under the name
/// it is given, so what reaches the other program is what is on screen. A
/// single place is written as `export_obj` does, with no `o` line.
pub fn export_obj_placed(mesh: &Mesh, places: &[(&str, Mat4)], path: &Path) -> Result<()> {
    nothing_to_export(mesh)?;
    let io = |e: std::io::Error| format!("Could not write {}: {e}", path.display());
    let mut w = BufWriter::with_capacity(1 << 20, File::create(path).map_err(io)?);
    writeln!(w, "# Exported from Sculpt").map_err(io)?;
    // OBJ numbers its vertices through the whole file, so each place carries
    // on from the last.
    let mut base = 0;
    for &(name, model) in places {
        if places.len() > 1 {
            writeln!(w, "o {name}").map_err(io)?;
        }
        write_object(&mut w, mesh, model, base).map_err(io)?;
        base += mesh.positions.len() as u32;
    }
    w.flush().map_err(io)
}

/// The vertices, normals and faces of `mesh` placed by `model`, numbered from `base`.
fn write_object(w: &mut impl Write, mesh: &Mesh, model: Mat4, base: u32) -> std::io::Result<()> {
    for p in &mesh.positions {
        let q = model.transform_point3(*p);
        writeln!(w, "v {} {} {}", q.x, q.y, q.z)?;
    }
    // Normals go through the inverse transpose: a plain rotation would leave
    // them skewed against a surface that has been stretched.
    let linear = Mat3::from_mat4(model);
    let determinant = linear.determinant();
    let normal_matrix = if determinant.abs() > 1e-20 { linear.inverse().transpose() } else { linear };
    let with_normals = mesh.normals.len() == mesh.positions.len();
    if with_normals {
        for n in &mesh.normals {
            let q = (normal_matrix * *n).normalize_or_zero();
            writeln!(w, "vn {:.5} {:.5} {:.5}", q.x, q.y, q.z)?;
        }
    }
    // A negative scale mirrors the model and turns every face inside out.
    let mirrored = determinant < 0.0;
    if let Some(polys) = &mesh.polys {
        // A quad-remeshed mesh goes out as its quads, not as their halves.
        for f in polys.start.windows(2) {
            write_face(w, &polys.indices[f[0] as usize..f[1] as usize], mirrored, with_normals, base)?;
        }
    } else {
        for t in mesh.indices.chunks_exact(3) {
            write_face(w, t, mirrored, with_normals, base)?;
        }
    }
    Ok(())
}

fn write_face(w: &mut impl Write, corners: &[u32], reversed: bool, with_normals: bool, base: u32) -> std::io::Result<()> {
    write!(w, "f")?;
    let mut put = |i: u32| {
        let i = i + base + 1;
        if with_normals { write!(w, " {i}//{i}") } else { write!(w, " {i}") }
    };
    if reversed {
        for &i in corners.iter().rev() {
            put(i)?;
        }
    } else {
        for &i in corners {
            put(i)?;
        }
    }
    writeln!(w)
}

/// Binary STL of one object as it sits in the scene, for a slicer: placed by
/// `model` (see [`export_obj`]), in millimetres at the real-world size set by
/// `sculpt_geom::units`, and turned from the app's Y up to STL's Z up (see the
/// top of this file). Triangles keep their winding, so the surface stays
/// outward-facing.
#[allow(dead_code)] // as `export_obj`
pub fn export_stl(mesh: &Mesh, model: Mat4, path: &Path) -> Result<()> {
    export_stl_placed(mesh, &[model], path)
}

/// [`export_stl`] for a mesh drawn in several places at once (an object and
/// its live mirror copies), all in the one file, as the model on screen.
pub fn export_stl_placed(mesh: &Mesh, models: &[Mat4], path: &Path) -> Result<()> {
    nothing_to_export(mesh)?;
    let io = |e: std::io::Error| format!("Could not write {}: {e}", path.display());
    let mut w = BufWriter::with_capacity(1 << 20, File::create(path).map_err(io)?);
    // Must not start with "solid", or readers that sniff the format by that
    // word take this binary file for an ASCII one.
    let mut header = [0u8; STL_HEADER];
    let note = b"Exported from Sculpt (millimetres, Z up)";
    header[..note.len()].copy_from_slice(note);
    w.write_all(&header).map_err(io)?;
    let triangles = mesh.triangle_count() * models.len();
    w.write_all(&(triangles as u32).to_le_bytes()).map_err(io)?;
    for model in models {
        let points: Vec<Vec3> = mesh.positions.iter().map(|&p| to_stl(model.transform_point3(p))).collect();
        let mirrored = Mat3::from_mat4(*model).determinant() < 0.0;
        for t in mesh.indices.chunks_exact(3) {
            let (a, mut b, mut c) = (points[t[0] as usize], points[t[1] as usize], points[t[2] as usize]);
            if mirrored {
                (b, c) = (c, b);
            }
            // From the corners as written, so it cannot disagree with their order.
            let normal = (b - a).cross(c - a).normalize_or_zero();
            let mut facet = [0u8; STL_FACET];
            for (slot, v) in [normal, a, b, c].iter().enumerate() {
                for (axis, value) in v.to_array().iter().enumerate() {
                    let at = slot * 12 + axis * 4;
                    facet[at..at + 4].copy_from_slice(&value.to_le_bytes());
                }
            }
            w.write_all(&facet).map_err(io)?;
        }
    }
    w.flush().map_err(io)
}

/// What the save dialog offers as a file name: the object's name and the time,
/// "Lure_2026-10-10_153012.stl", so repeated exports never overwrite one
/// another. Characters Windows will not take in a file name become underscores.
pub fn export_file_name(object: &str, extension: &str) -> String {
    let cleaned: String = object
        .chars()
        .map(|c| if c.is_control() || "<>:\"/\\|?*".contains(c) { '_' } else { c })
        .collect();
    // Windows drops a trailing space or dot from a name, which would break the stamp.
    let cleaned = cleaned.trim_matches(|c| c == ' ' || c == '.');
    let name = if cleaned.is_empty() { "Sculpt" } else { cleaned };
    format!("{name}_{}.{extension}", time_stamp())
}

/// Local time as year-month-day_hourminutesecond.
#[cfg(windows)]
fn time_stamp() -> String {
    #[repr(C)]
    #[derive(Default)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetLocalTime(time: *mut SystemTime);
    }
    let mut now = SystemTime::default();
    // SAFETY: GetLocalTime only fills in the SYSTEMTIME it is given, which has the layout above.
    unsafe { GetLocalTime(&mut now) };
    format!("{:04}-{:02}-{:02}_{:02}{:02}{:02}", now.year, now.month, now.day, now.hour, now.minute, now.second)
}

/// Everywhere else there is no clock without a crate, so UTC from the epoch.
#[cfg(not(windows))]
fn time_stamp() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    stamp_from_unix(seconds)
}

/// A Unix time (seconds, UTC) in the same form as [`time_stamp`].
#[cfg_attr(windows, allow(dead_code))] // Windows asks the system for local time instead
fn stamp_from_unix(seconds: u64) -> String {
    let (days, rest) = ((seconds / 86_400) as i64, seconds % 86_400);
    // Days since 1970-01-01 to a calendar date (Howard Hinnant's civil_from_days).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_from_march = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_from_march + 2) / 5 + 1;
    let month = if month_from_march < 10 { month_from_march + 3 } else { month_from_march - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}_{:02}{:02}{:02}", rest / 3_600, rest % 3_600 / 60, rest % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::SavedView;
    use crate::look::{Finish, Look, Tonemap};
    use crate::mesh::primitives::{self, Primitive};
    use crate::scene::Scene;
    use crate::sculpt::{Radial, RadialAxis};

    /// A file path under the temp folder. Tests run side by side, so every
    /// test gives its files names of its own.
    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("sculpt_io_tests");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    fn sphere(tris: usize) -> Mesh {
        primitives::build(Primitive::Sphere, tris, 0.5)
    }

    /// The twelve triangles of a cube `half` either side of the origin, wound
    /// counter-clockwise seen from outside (outward normals).
    fn cube_facets(half: f32) -> Vec<[Vec3; 3]> {
        let mut out = Vec::new();
        for axis in 0..3 {
            for sign in [1.0f32, -1.0] {
                let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
                let corner = |a: f32, b: f32| {
                    let mut p = Vec3::ZERO;
                    p[axis] = sign * half;
                    p[u] = a * half;
                    p[v] = b * half;
                    p
                };
                let mut quad = [corner(-1.0, -1.0), corner(1.0, -1.0), corner(1.0, 1.0), corner(-1.0, 1.0)];
                if sign < 0.0 {
                    quad.reverse();
                }
                out.push([quad[0], quad[1], quad[2]]);
                out.push([quad[0], quad[2], quad[3]]);
            }
        }
        out
    }

    /// Triangles as loose corners.
    fn facets(positions: &[Vec3], indices: &[u32]) -> Vec<[Vec3; 3]> {
        indices.chunks_exact(3).map(|t| [0, 1, 2].map(|k| positions[t[k] as usize])).collect()
    }

    fn facets_of(mesh: &Mesh) -> Vec<[Vec3; 3]> {
        facets(&mesh.positions, &mesh.indices)
    }

    /// How a test writer fills in each facet's stored normal.
    #[derive(Clone, Copy, PartialEq)]
    enum Normals {
        Outward,
        Inward,
        Zero,
    }

    fn facet_normal(f: &[Vec3; 3], how: Normals) -> Vec3 {
        let n = (f[1] - f[0]).cross(f[2] - f[0]).normalize_or_zero();
        match how {
            Normals::Outward => n,
            Normals::Inward => -n,
            Normals::Zero => Vec3::ZERO,
        }
    }

    fn binary_stl(facets: &[[Vec3; 3]], normals: Normals) -> Vec<u8> {
        let stored: Vec<Vec3> = facets.iter().map(|f| facet_normal(f, normals)).collect();
        binary_stl_with(facets, &stored)
    }

    /// Binary STL with each facet's stored normal given.
    fn binary_stl_with(facets: &[[Vec3; 3]], stored: &[Vec3]) -> Vec<u8> {
        let mut bytes = vec![0u8; STL_HEADER];
        bytes.extend_from_slice(&(facets.len() as u32).to_le_bytes());
        for (f, n) in facets.iter().zip(stored) {
            for v in [*n, f[0], f[1], f[2]] {
                for c in v.to_array() {
                    bytes.extend_from_slice(&c.to_le_bytes());
                }
            }
            bytes.extend_from_slice(&[0, 0]);
        }
        bytes
    }

    /// `{:e}` prints the shortest text that reads back as the same f32.
    fn ascii_stl(facets: &[[Vec3; 3]], normals: Normals) -> Vec<u8> {
        let mut text = String::from("solid test\n");
        for f in facets {
            let n = facet_normal(f, normals);
            text += &format!("  facet normal {:e} {:e} {:e}\n    outer loop\n", n.x, n.y, n.z);
            for v in f {
                text += &format!("      vertex {:e} {:e} {:e}\n", v.x, v.y, v.z);
            }
            text += "    endloop\n  endfacet\n";
        }
        text += "endsolid test\n";
        text.into_bytes()
    }

    /// Signed volume of an imported model; positive when it faces outward.
    fn volume(raw: &RawMesh) -> f64 {
        facet_volume(&facets(&raw.positions, &raw.indices))
    }

    /// Signed volume of loose triangles; positive when they face outward.
    fn facet_volume(facets: &[[Vec3; 3]]) -> f64 {
        facets
            .iter()
            .map(|f| {
                let p = f.map(|v| v.as_dvec3());
                p[0].dot(p[1].cross(p[2])) / 6.0
            })
            .sum()
    }

    /// An STL's triangles exactly as written: millimetres, Z up, no conversion.
    fn read_file_facets(path: &Path) -> Vec<[Vec3; 3]> {
        let bytes = std::fs::read(path).unwrap();
        assert!(looks_binary(&bytes), "the export is a binary STL");
        assert!(!bytes.starts_with(b"solid"), "a header that says solid makes readers guess ASCII");
        let float = |b: &[u8], at: usize| f32::from_le_bytes(b[at..at + 4].try_into().unwrap());
        bytes[84..]
            .chunks_exact(STL_FACET)
            .map(|f| [0, 1, 2].map(|k| Vec3::new(float(f, 12 + k * 12), float(f, 16 + k * 12), float(f, 20 + k * 12))))
            .collect()
    }

    fn assert_close(a: Vec3, b: Vec3, tolerance: f32, what: &str) {
        assert!((a - b).abs().max_element() <= tolerance, "{what}: {a} vs {b}");
    }

    #[test]
    fn stl_export_then_import_returns_every_vertex() {
        let mesh = sphere(5_000);
        let path = temp("round_trip.stl");
        export_stl(&mesh, Mat4::IDENTITY, &path).unwrap();
        let raw = import(&path).unwrap();
        assert!(raw.real_size);
        assert_eq!(raw.positions.len(), mesh.vertex_count(), "welds back to the same vertices");
        assert_eq!(raw.indices.len(), mesh.indices.len());
        // Same triangles in the same order, corner for corner.
        for (back, was) in facets(&raw.positions, &raw.indices).iter().zip(facets_of(&mesh)) {
            for k in 0..3 {
                assert_close(back[k], was[k], 1e-6, "vertex after the round trip");
            }
        }
    }

    #[test]
    fn a_moved_turned_and_scaled_object_exports_where_it_stands() {
        let mesh = sphere(2_000);
        let model = Mat4::from_scale_rotation_translation(
            Vec3::new(2.0, 1.0, 0.5),
            Quat::from_rotation_z(std::f32::consts::FRAC_PI_2) * Quat::from_rotation_x(0.3),
            Vec3::new(2.0, 1.0, -3.0),
        );

        let stl = temp("placed.stl");
        export_stl(&mesh, model, &stl).unwrap();
        let written = read_file_facets(&stl);
        for (got, was) in written.iter().zip(facets_of(&mesh)) {
            for k in 0..3 {
                let want = to_stl(model.transform_point3(was[k]));
                assert_close(got[k], want, 1e-3, "STL vertex");
            }
        }

        let obj = temp("placed.obj");
        export_obj(&mesh, model, &obj).unwrap();
        let text = std::fs::read_to_string(&obj).unwrap();
        let vertices: Vec<Vec3> = text
            .lines()
            .filter_map(|l| l.strip_prefix("v "))
            .map(|l| {
                let c: Vec<f32> = l.split(' ').map(|s| s.parse().unwrap()).collect();
                Vec3::new(c[0], c[1], c[2])
            })
            .collect();
        assert_eq!(vertices.len(), mesh.vertex_count());
        for (got, was) in vertices.iter().zip(&mesh.positions) {
            assert_close(*got, model.transform_point3(*was), 1e-5, "OBJ vertex");
        }
    }

    #[test]
    fn a_mirroring_transform_still_exports_outward_faces() {
        let mesh = sphere(2_000);
        let path = temp("mirrored.stl");
        export_stl(&mesh, Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0)), &path).unwrap();
        let volume = facet_volume(&read_file_facets(&path));
        assert!(volume > 0.0, "a mirrored object must not export inside out: {volume}");
    }

    #[test]
    fn an_upright_model_is_tallest_along_z_and_not_mirrored() {
        // Three times as tall as it is wide, with a nose pointing at the viewer.
        let mesh = sphere(5_000);
        let model = Mat4::from_scale(Vec3::new(1.0, 3.0, 1.0));
        let path = temp("tall.stl");
        export_stl(&mesh, model, &path).unwrap();
        let facets = read_file_facets(&path);
        let (lo, hi) = bounds_of(&facets.iter().flatten().copied().collect::<Vec<_>>());
        let size = hi - lo;
        assert!(size.z > 2.9 * size.x && size.z > 2.9 * size.y, "Z is the tall axis: {size}");
        assert!((size.z - 30.0).abs() < 0.2, "3 units tall is 30 mm: {size}");

        // Up in the viewport is up on the bed, the side facing the viewer
        // (+Z) is the front of the bed (-Y), and the right (+X) stays right.
        assert_close(to_stl(Vec3::Y), Vec3::new(0.0, 0.0, 10.0), 1e-5, "up");
        assert_close(to_stl(Vec3::Z), Vec3::new(0.0, -10.0, 0.0), 1e-5, "toward the viewer");
        assert_close(to_stl(Vec3::X), Vec3::new(10.0, 0.0, 0.0), 1e-5, "right");
        // A rotation, not a mirror: the facets still face outward.
        // The sphere stretched threefold, in cubic millimetres (1000 to a cubic unit).
        let volume = facet_volume(&facets);
        let expected = facet_volume(&facets_of(&mesh)) * 3.0 * 1000.0;
        assert!(volume > 0.0 && (volume - expected).abs() < expected * 1e-3, "volume {volume}, expected {expected}");
    }

    #[test]
    fn the_default_sphere_exports_ten_millimetres_across() {
        // The startup sphere is 1 unit across: 10 mm at 10 mm per unit, the
        // same ball the Unity app exports.
        let mesh = sphere(20_000);
        let path = temp("default_sphere.stl");
        export_stl(&mesh, Mat4::IDENTITY, &path).unwrap();
        let facets = read_file_facets(&path);
        let (lo, hi) = bounds_of(&facets.iter().flatten().copied().collect::<Vec<_>>());
        let size = hi - lo;
        assert!((size - Vec3::splat(10.0)).abs().max_element() < 0.05, "{size}");
    }

    #[test]
    fn ascii_and_binary_stl_import_identically() {
        let mesh = sphere(2_000);
        let facets = facets_of(&mesh);
        let (binary_path, ascii_path) = (temp("same_model_binary.stl"), temp("same_model_ascii.stl"));
        std::fs::write(&binary_path, binary_stl(&facets, Normals::Outward)).unwrap();
        std::fs::write(&ascii_path, ascii_stl(&facets, Normals::Outward)).unwrap();
        let (binary, ascii) = (import(&binary_path).unwrap(), import(&ascii_path).unwrap());
        assert_eq!(binary.indices, ascii.indices);
        assert_eq!(binary.positions, ascii.positions);
        assert_eq!(binary.positions.len(), mesh.vertex_count(), "both welded back to the model's own vertices");
    }

    #[test]
    fn a_file_that_says_solid_but_is_binary_still_reads() {
        let facets = cube_facets(5.0);
        let mut bytes = binary_stl(&facets, Normals::Outward);
        bytes[..11].copy_from_slice(b"solid  cube");
        // Exactly sized, with the word on top.
        assert_eq!(parse_stl(&bytes).unwrap().indices.len(), 36);
        // Padded, as if a writer had appended something: the length sum fails.
        let mut padded = bytes.clone();
        padded.extend_from_slice(&[0, 1, 2, 3, 4, 5, 6]);
        assert_eq!(parse_stl(&padded).unwrap().indices.len(), 36);
        // Cut short in the last facet: what is whole is kept.
        let cut = &bytes[..bytes.len() - 20];
        assert_eq!(parse_stl(cut).unwrap().indices.len(), 33);
        // A count of zero on a file with triangles in it.
        let mut uncounted = bytes.clone();
        uncounted[STL_HEADER..STL_HEADER + 4].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(parse_stl(&uncounted).unwrap().indices.len(), 36);
        // A count that is nonsense must not make the reader reserve gigabytes.
        let mut absurd = bytes.clone();
        absurd[STL_HEADER..STL_HEADER + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(parse_stl(&absurd).unwrap().indices.len(), 36);
        // Not an STL at all.
        assert!(parse_stl(b"hello there").is_err());
        assert!(parse_stl(b"solid empty\nendsolid empty\n").is_err());
    }

    #[test]
    fn a_file_with_inverted_winding_comes_in_facing_outward() {
        let right = cube_facets(5.0);
        let inverted: Vec<[Vec3; 3]> = right.iter().map(|f| [f[0], f[2], f[1]]).collect();
        let wanted = 1.0; // a 10 mm cube is 1 unit^3
        let sound = parse_stl(&binary_stl(&right, Normals::Outward)).unwrap();
        assert!((volume(&sound) - wanted).abs() < 1e-4);
        // Corners in the wrong order, stored normals pointing outward: read the normals.
        let a = parse_stl(&binary_stl(&inverted, Normals::Outward)).unwrap();
        // Corners in the wrong order and no normals: a closed surface that is inside out.
        let b = parse_stl(&binary_stl(&inverted, Normals::Zero)).unwrap();
        // Everything inverted, normals included.
        let c = parse_stl(&binary_stl(&inverted, Normals::Inward)).unwrap();
        // The same, as text.
        let d = parse_stl(&ascii_stl(&inverted, Normals::Zero)).unwrap();
        for (what, raw) in [("normals outward", a), ("no normals", b), ("normals inward", c), ("ascii", d)] {
            assert!((volume(&raw) - wanted).abs() < 1e-4, "{what}: volume {}", volume(&raw));
        }
    }

    #[test]
    fn facets_that_disagree_with_their_own_normal_are_turned_one_by_one() {
        // Half the facets wound backwards, the rest right: nothing consistent
        // for a whole-model test to go on, so the stored normals decide.
        let mixed: Vec<[Vec3; 3]> = cube_facets(5.0)
            .iter()
            .enumerate()
            .map(|(i, f)| if i % 2 == 0 { [f[0], f[2], f[1]] } else { *f })
            .collect();
        let outward: Vec<Vec3> = cube_facets(5.0).iter().map(|f| facet_normal(f, Normals::Outward)).collect();
        let raw = parse_stl(&binary_stl_with(&mixed, &outward)).unwrap();
        assert!((volume(&raw) - 1.0).abs() < 1e-4, "volume {}", volume(&raw));
    }

    #[test]
    fn an_open_surface_is_not_turned_inside_out() {
        // One face of the cube alone: no volume to be negative.
        let sheet: Vec<[Vec3; 3]> = cube_facets(5.0).into_iter().take(2).map(|f| [f[0], f[2], f[1]]).collect();
        let raw = parse_stl(&binary_stl(&sheet, Normals::Zero)).unwrap();
        assert_eq!(raw.indices.len(), 6);
        let first = [0, 1, 2].map(|k| to_stl(raw.positions[raw.indices[k] as usize]));
        assert_close(first[1], sheet[0][1], 1e-4, "winding left as written");
    }

    #[test]
    fn a_cube_stl_imports_as_eight_vertices_not_thirty_six() {
        let raw = parse_stl(&binary_stl(&cube_facets(5.0), Normals::Outward)).unwrap();
        assert_eq!(raw.positions.len(), 8);
        assert_eq!(raw.indices.len(), 36);
        // And it comes in at its real size: 10 mm is one unit.
        let (lo, hi) = bounds_of(&raw.positions);
        assert_close(hi - lo, Vec3::ONE, 1e-6, "cube size in units");
    }

    #[test]
    fn corners_a_hair_apart_weld_even_across_a_cell_wall() {
        // Diagonal 1000, so a cell is 0.001 across.
        let (lo, hi) = (Vec3::ZERO, Vec3::new(1000.0, 0.0, 0.0));
        let cell = 1000.0 / WELD_CELLS;
        let (just_below, just_above) = (Vec3::new(4.95 * cell, 0.0, 0.0), Vec3::new(5.05 * cell, 0.0, 0.0));
        let apart = Vec3::new(8.0 * cell, 0.0, 0.0);
        let (vertices, indices) = weld_corners(&[lo, hi, just_below, just_above, apart, just_above]);
        assert_eq!(vertices.len(), 4, "the pair a tenth of a cell apart is one vertex: {vertices:?}");
        assert_eq!(indices, vec![0, 1, 2, 2, 3, 2]);
        assert_eq!(vertices[2], just_below, "the first corner keeps its coordinates");
    }

    #[test]
    fn triangles_with_no_area_are_dropped() {
        let mut facets = cube_facets(5.0);
        facets.push([Vec3::ONE, Vec3::ONE, Vec3::ZERO]);
        facets.push([Vec3::new(f32::NAN, 0.0, 0.0), Vec3::ZERO, Vec3::X]);
        let raw = parse_stl(&binary_stl(&facets, Normals::Outward)).unwrap();
        assert_eq!(raw.indices.len(), 36);
        assert_eq!(raw.positions.len(), 8);
    }

    fn quad_cube() -> Mesh {
        // A cube of six quads, each also given as its two triangles.
        let facets = cube_facets(0.5);
        let (positions, indices) = weld_corners(&facets.concat());
        let (mut mesh, to_new) = Mesh::new_mapped(positions, indices.clone(), None);
        let mut start = vec![0u32];
        let mut corners = Vec::new();
        for pair in indices.chunks_exact(6) {
            // Two triangles (a, b, c) and (a, c, d) make the quad a, b, c, d.
            let quad = [pair[0], pair[1], pair[2], pair[5]];
            corners.extend(quad.iter().map(|&i| to_new[i as usize]));
            start.push(corners.len() as u32);
        }
        mesh.polys = Some(crate::mesh::Polys { start, indices: corners });
        mesh
    }

    #[test]
    fn obj_round_trip_keeps_quads_and_vertices() {
        let mesh = quad_cube();
        let path = temp("quads.obj");
        export_obj(&mesh, Mat4::IDENTITY, &path).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        let faces: Vec<usize> = text.lines().filter(|l| l.starts_with("f ")).map(|l| l.split(' ').count() - 1).collect();
        assert_eq!(faces, vec![4; 6], "six quads, not twelve triangles");

        let raw = import(&path).unwrap();
        assert!(!raw.real_size);
        assert_eq!(raw.positions.len(), 8);
        assert_eq!(raw.indices.len(), 36, "each quad comes back as two triangles");
        assert!((volume(&raw) - 1.0).abs() < 1e-5, "outward, unit cube: {}", volume(&raw));
        // OBJ and the app share their axes, so nothing moved.
        let mut was: Vec<[i32; 3]> = mesh.positions.iter().map(|p| (*p * 1000.0).round().to_array().map(|c| c as i32)).collect();
        let mut got: Vec<[i32; 3]> = raw.positions.iter().map(|p| (*p * 1000.0).round().to_array().map(|c| c as i32)).collect();
        was.sort();
        got.sort();
        assert_eq!(was, got);
    }

    #[test]
    fn obj_export_of_plain_triangles_round_trips_too() {
        let mesh = sphere(3_000);
        let path = temp("triangles.obj");
        export_obj(&mesh, Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0)), &path).unwrap();
        let raw = import(&path).unwrap();
        assert_eq!(raw.positions.len(), mesh.vertex_count());
        assert_eq!(raw.indices.len(), mesh.indices.len());
        let (lo, hi) = bounds_of(&raw.positions);
        assert_close((lo + hi) * 0.5, Vec3::new(1.0, 2.0, 3.0), 1e-4, "placed at the object's position");
    }

    #[test]
    fn imports_are_fitted_and_centred_like_the_unity_app() {
        // An OBJ 100 across, centred well off to one side. The first object is
        // the startup sphere, half-sizes 0.5.
        let mut obj = RawMesh {
            positions: cube_facets(50.0).concat().iter().map(|p| *p + Vec3::new(30.0, 0.0, 0.0)).collect(),
            indices: (0..36).collect(),
            real_size: false,
        };
        let at = place_import(&mut obj, Some(Vec3::splat(0.5)));
        let (lo, hi) = bounds_of(&obj.positions);
        // Its longest half-side becomes the sphere's average width (1.0).
        assert_close((hi - lo) * 0.5, Vec3::splat(1.0), 1e-5, "fitted half-size");
        assert_close(at, -(lo + hi) * 0.5, 1e-6, "position puts the centre at the origin");
        assert_close(at, Vec3::new(-30.0 * 0.02, 0.0, 0.0), 1e-5, "centre was at x = 30, scaled by 0.02");
        // Nothing to fit to: size is left alone, but it is still centred.
        let mut alone = RawMesh { positions: vec![Vec3::new(10.0, 0.0, 0.0), Vec3::new(20.0, 4.0, 2.0), Vec3::ZERO], indices: vec![0, 1, 2], real_size: false };
        assert_close(place_import(&mut alone, None), Vec3::new(-10.0, -2.0, -1.0), 1e-6, "centred only");
        assert_eq!(alone.positions[1], Vec3::new(20.0, 4.0, 2.0));
        // An STL keeps the size it was written at.
        let mut stl = parse_stl(&binary_stl(&cube_facets(50.0), Normals::Outward)).unwrap();
        place_import(&mut stl, Some(Vec3::splat(0.5)));
        let (lo, hi) = bounds_of(&stl.positions);
        assert_close(hi - lo, Vec3::splat(10.0), 1e-4, "a 100 mm cube is 10 units");
    }

    #[test]
    fn an_instance_exports_as_its_own_placed_copy() {
        let slot = sphere(2_000);
        let mut scene = Scene::new("Sphere");
        let original_at = scene.active().position;
        scene.instance_active();
        scene.active_mut().position = Vec3::new(4.0, 1.0, 0.0);
        scene.active_mut().scale = Vec3::splat(2.0);
        assert_ne!(scene.active().position, original_at);

        let path = temp("instance.stl");
        export_stl(&slot, scene.active().matrix(), &path).unwrap();
        let (lo, hi) = bounds_of(&read_file_facets(&path).concat());
        // Centre (4, 1, 0) in the app is (40, 0, 10) mm; twice 10 mm across.
        assert_close((lo + hi) * 0.5, Vec3::new(40.0, 0.0, 10.0), 0.05, "the instance's own place");
        assert!((hi - lo - Vec3::splat(20.0)).abs().max_element() < 0.1, "and its own size: {}", hi - lo);
        // And the shared mesh itself was not moved or resized.
        assert!((slot.bounds().size() - Vec3::ONE).abs().max_element() < 0.01);
    }

    #[test]
    fn file_names_carry_the_object_and_the_time() {
        let name = export_file_name("My Lure", "stl");
        assert!(name.starts_with("My Lure_") && name.ends_with(".stl"), "{name}");
        let stamp = &name["My Lure_".len()..name.len() - ".stl".len()];
        let digits = |range: std::ops::Range<usize>| stamp[range].chars().all(|c| c.is_ascii_digit());
        assert_eq!(stamp.len(), 17, "{stamp}");
        assert!(digits(0..4) && &stamp[4..5] == "-" && digits(5..7) && &stamp[7..8] == "-" && digits(8..10), "{stamp}");
        assert!(&stamp[10..11] == "_" && digits(11..17), "{stamp}");

        assert!(export_file_name("a/b:c?", "obj").starts_with("a_b_c__"));
        assert!(export_file_name("  ", "obj").starts_with("Sculpt_"));
        assert_eq!(stamp_from_unix(0), "1970-01-01_000000");
        assert_eq!(stamp_from_unix(1_700_000_000), "2023-11-14_221320");
        assert_eq!(stamp_from_unix(1_709_164_800 + 3_661), "2024-02-29_010101", "a leap day");
    }

    // ---- .sclp files

    /// The scene writer as it stood while "SCLP0003" was what the app saved,
    /// kept word for word so tests can still make such files.
    fn save_scene_fixed(meshes: &[&Mesh], objects: &[SavedObject], active: usize, path: &Path) -> Result<()> {
        let io = |e: std::io::Error| format!("Could not write {}: {e}", path.display());
        let mut w = BufWriter::with_capacity(1 << 20, File::create(path).map_err(io)?);
        w.write_all(SCENE_MAGIC_FIXED).map_err(io)?;
        for n in [meshes.len(), objects.len(), active] {
            w.write_all(&(n as u32).to_le_bytes()).map_err(io)?;
        }
        for mesh in meshes {
            w.write_all(&(mesh.positions.len() as u64).to_le_bytes()).map_err(io)?;
            w.write_all(&(mesh.indices.len() as u64).to_le_bytes()).map_err(io)?;
            w.write_all(bytemuck::cast_slice(&mesh.positions)).map_err(io)?;
            w.write_all(bytemuck::cast_slice(&mesh.indices)).map_err(io)?;
            w.write_all(bytemuck::cast_slice(&mesh.mask)).map_err(io)?;
        }
        for o in objects {
            w.write_all(&(o.name.len() as u32).to_le_bytes()).map_err(io)?;
            w.write_all(o.name.as_bytes()).map_err(io)?;
            w.write_all(&[o.visible as u8]).map_err(io)?;
            w.write_all(&(o.mesh as u32).to_le_bytes()).map_err(io)?;
            w.write_all(bytemuck::cast_slice(&o.position.to_array())).map_err(io)?;
            w.write_all(bytemuck::cast_slice(&o.rotation.to_array())).map_err(io)?;
            w.write_all(bytemuck::cast_slice(&o.scale.to_array())).map_err(io)?;
        }
        w.flush().map_err(io)
    }

    /// A whole scene file exactly as the app wrote it while "SCLP0003" was
    /// its format: a tetrahedron with two of its corners masked, shown by two
    /// objects (the second an instance, hidden, and the active one).
    const SAVED_AS_0003: &str = "53434c503030303301000000020000000100000004000000000000000c00000000000000000000000000000000000000000000000000803f000000000000803f000000000000000000000000000000000000803f000000000100000002000000000000000200000003000000000000000300000001000000020000000100000003000000000000000000003f000000000000803f05000000546574726101000000000000803f00000040000040400000000077577d3e00000000a50a783f0000803f000000400000003f0e000000546574726120496e7374616e63650000000000000080bf000000000000803e0000000000000000000000000000803f0000803f0000803f0000803f";

    fn unhex(text: &str) -> Vec<u8> {
        (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap()).collect()
    }

    fn object(name: &str, mesh: usize, x: f32) -> SavedObject {
        SavedObject {
            name: name.into(),
            visible: x != 1.0,
            mesh,
            position: Vec3::new(x, 0.5, -x),
            rotation: Quat::from_rotation_y(x),
            scale: Vec3::new(1.0, 1.0 + x, 0.5),
        }
    }

    fn assert_same_objects(back: &[SavedObject], were: &[SavedObject]) {
        assert_eq!(back.len(), were.len());
        for (a, b) in back.iter().zip(were) {
            assert_eq!((&a.name, a.visible, a.mesh, a.position, a.scale), (&b.name, b.visible, b.mesh, b.position, b.scale));
            assert!(a.rotation.angle_between(b.rotation) < 1e-4, "{}: {} vs {}", a.name, a.rotation, b.rotation);
        }
    }

    /// Every vertex of a mesh with its mask, as exact bits, in an order that
    /// does not depend on how the mesh numbers them: building a mesh
    /// reorders its vertices, but must neither move nor lose one.
    fn exact_vertices(mesh: &Mesh) -> Vec<[u32; 4]> {
        let mut all: Vec<[u32; 4]> =
            mesh.positions.iter().zip(&mesh.mask).map(|(p, m)| [p.x.to_bits(), p.y.to_bits(), p.z.to_bits(), m.to_bits()]).collect();
        all.sort_unstable();
        all
    }

    fn assert_same_mesh(back: &Mesh, was: &Mesh) {
        assert_eq!((back.vertex_count(), back.triangle_count()), (was.vertex_count(), was.triangle_count()));
        assert_eq!(exact_vertices(back), exact_vertices(was), "every vertex and its mask, to the bit");
        let (a, b) = (facet_volume(&facets_of(back)), facet_volume(&facets_of(was)));
        assert!((a - b).abs() <= b.abs() * 1e-9, "the same surface: volume {a} vs {b}");
    }

    /// The blocks of a block-format file: each tag, and where its body is.
    fn blocks(bytes: &[u8]) -> Vec<([u8; 4], std::ops::Range<usize>)> {
        let mut found = Vec::new();
        let mut at = 8;
        while at < bytes.len() {
            let length = u64::from_le_bytes(bytes[at + 4..at + 12].try_into().unwrap()) as usize;
            found.push((bytes[at..at + 4].try_into().unwrap(), at + 12..at + 12 + length));
            at += 12 + length;
        }
        assert_eq!(at, bytes.len(), "the blocks fill the file exactly");
        found
    }

    fn tags(bytes: &[u8]) -> Vec<String> {
        blocks(bytes).iter().map(|(tag, _)| String::from_utf8_lossy(tag).into_owned()).collect()
    }

    #[test]
    fn a_file_saved_before_the_format_changed_still_opens() {
        let bytes = unhex(SAVED_AS_0003);
        assert_eq!(&bytes[..8], b"SCLP0003");
        let path = temp("saved_as_0003.sclp");
        std::fs::write(&path, &bytes).unwrap();
        let back = load_scene(&path).unwrap();

        assert_eq!(back.meshes.len(), 1);
        let mesh = &back.meshes[0];
        assert_eq!((mesh.vertex_count(), mesh.triangle_count()), (4, 4));
        assert!((facet_volume(&facets_of(mesh)) - 1.0 / 6.0).abs() < 1e-9, "the unit tetrahedron, facing outward");
        let masked = |corner: Vec3| mesh.mask[mesh.positions.iter().position(|p| *p == corner).unwrap()];
        assert_eq!([masked(Vec3::ZERO), masked(Vec3::X), masked(Vec3::Y), masked(Vec3::Z)], [0.0, 0.0, 0.5, 1.0]);

        let were = [
            SavedObject {
                name: "Tetra".into(),
                visible: true,
                mesh: 0,
                position: Vec3::new(1.0, 2.0, 3.0),
                rotation: Quat::from_rotation_y(0.5),
                scale: Vec3::new(1.0, 2.0, 0.5),
            },
            SavedObject {
                name: "Tetra Instance".into(),
                visible: false,
                mesh: 0,
                position: Vec3::new(-1.0, 0.0, 0.25),
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            },
        ];
        assert_same_objects(&back.objects, &were);
        assert_eq!(back.active, 1);
        // Nothing but objects was kept then, so nothing of the app's is replaced.
        assert!(back.details.get::<SavedView>(slot::CAMERA).is_none());
        assert!(back.details.get_over(slot::LOOK, &Look::default()).is_none());

        // Saved again it becomes today's format, and nothing is lost on the way.
        let again = temp("saved_as_0003_again.sclp");
        save_scene(&[mesh], &back.objects, back.active, &again).unwrap();
        let bytes = std::fs::read(&again).unwrap();
        assert_eq!(&bytes[..8], b"SCLP0004");
        assert_eq!(tags(&bytes), ["MESH", "MASK", "INFO"]);
        let twice = load_scene(&again).unwrap();
        assert_same_mesh(&twice.meshes[0], mesh);
        assert_same_objects(&twice.objects, &were);
    }

    #[test]
    fn every_older_layout_still_opens() {
        let (ball, block) = (sphere(2_000), primitives::build(Primitive::Cube, 500, 0.5));
        let objects = [object("Ball", 0, 0.0), object("Block", 1, 1.0), object("Ball Instance", 0, 2.0)];
        // Fixed records, as the writer of the time made them.
        let fixed = temp("layout_0003.sclp");
        save_scene_fixed(&[&ball, &block], &objects, 2, &fixed).unwrap();
        let back = load_scene(&fixed).unwrap();
        assert_same_mesh(&back.meshes[0], &ball);
        assert_same_mesh(&back.meshes[1], &block);
        assert_same_objects(&back.objects, &objects);
        assert_eq!(back.active, 2);

        // The layout before that had no rotation or scale: the same file
        // with those 28 bytes taken out of each object.
        let bytes = std::fs::read(&fixed).unwrap();
        let mut placed = b"SCLP0002".to_vec();
        let mut at = 8 + 12;
        for mesh in [&ball, &block] {
            at += 16 + mesh.vertex_count() * 16 + mesh.indices.len() * 4;
        }
        placed.extend_from_slice(&bytes[8..at]);
        for o in &objects {
            let record = 4 + o.name.len() + 1 + 4 + 12;
            placed.extend_from_slice(&bytes[at..at + record]);
            at += record + 28;
        }
        assert_eq!(at, bytes.len());
        let older = temp("layout_0002.sclp");
        std::fs::write(&older, placed).unwrap();
        let back = load_scene(&older).unwrap();
        assert_eq!((back.meshes.len(), back.objects.len(), back.active), (2, 3, 2));
        for (a, b) in back.objects.iter().zip(&objects) {
            assert_eq!((&a.name, a.visible, a.mesh, a.position), (&b.name, b.visible, b.mesh, b.position));
            assert_eq!((a.rotation, a.scale), (Quat::IDENTITY, Vec3::ONE));
        }

        // And the one-mesh file from before scenes: one object, named after the file.
        let first = temp("layout_0001.sclp");
        save_native(&ball, &first).unwrap();
        let back = load_scene(&first).unwrap();
        assert_same_mesh(&back.meshes[0], &ball);
        assert_eq!((back.objects.len(), back.objects[0].name.as_str()), (1, "layout_0001"));
    }

    #[test]
    fn a_scene_comes_back_with_its_look_camera_and_mirror() {
        let (mut ball, block) = (sphere(3_000), primitives::build(Primitive::Cube, 600, 0.5));
        // Half the ball masked, to see that each vertex keeps its own value.
        for v in 0..ball.vertex_count() {
            ball.mask[v] = if ball.positions[v].x > 0.0 { 0.25 + 0.5 * ball.positions[v].y.abs() } else { 0.0 };
        }
        let objects = [object("Ball", 0, 0.0), object("Block", 1, 1.0), object("Ball Instance", 0, 2.0)];

        // Nothing here is as the app starts out.
        let mut options = Options::default();
        options.matcap_on = false;
        options.flat_shading = true;
        options.cavity = false;
        options.cavity_ridge = 1.5;
        options.base_color = [0.8, 0.3, 0.1];
        options.smoothness = 0.9;
        options.gradient_background = true;
        options.background = [0.02, 0.03, 0.04];
        options.background_top = [0.1, 0.2, 0.3];
        let look = &mut options.look;
        look.metallic = 0.7;
        look.matcap_tint = 0.3;
        look.finish = Finish::Metal;
        look.metal.preset = 3;
        look.metal.gloss = 0.4;
        look.clay.preset = 1;
        look.wood.axis = 2;
        look.glow.on = true;
        look.glow.preset = 4;
        look.glow.hue_shift = 0.25;
        look.lighting = 5;
        look.five_point = true;
        look.brightness = 1.4;
        look.rotation = 120.0;
        look.follow_camera = false;
        look.world_forward = Vec3::new(0.6, 0.0, -0.8);
        look.shadows = false;
        look.hdri.enabled = true;
        look.hdri.path = Some(PathBuf::from("C:\\pictures\\studio small.hdr"));
        look.hdri.rotation = 45.0;
        look.hdri.exposure = 1.5;
        look.hdri_background = true;
        look.gradient_bias = 2.0;
        look.ambient_occlusion = false;
        look.bloom = false;
        look.bloom_threshold = 0.8;
        look.vignette_intensity = 0.5;
        look.grading = true;
        look.saturation = 20.0;
        look.contrast = -10.0;
        look.tonemap = Tonemap::Aces;
        look.depth_of_field = true;
        look.focus_distance = 2.5;
        look.aperture = 2.8;
        // True only while the app runs, so not for the file.
        look.hdri.loaded = true;
        look.hdri.status = "studio small.hdr (2048x1024)".into();

        let shading = Shading::of(&options, "Ben Simonds/MatCap_BS1".into());
        let view = SavedView { yaw: 132.5, pitch: -20.0, distance: 4.25, pivot: [0.1, 0.2, -0.3], ortho: true, fov_y: 45.0 };
        let radial = Radial { on: true, count: 6, axis: RadialAxis::Custom, custom_axis: Vec3::new(0.6, 0.0, -0.8) };
        let mirror = Mirror { x: true, y: false, z: true, guides: false, radial };
        let mut details = Details::default();
        details.set(slot::SHADING, &shading);
        details.set(slot::LOOK, &options.look);
        details.set(slot::CAMERA, &view);
        details.set(slot::MIRROR, &mirror);
        details.set_object(2, slot::SYMMETRY, &[true, false, false]);

        let path = temp("whole_scene.sclp");
        save_scene_with(&[&ball, &block], &objects, 2, &details, &path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        // The ball has a mask to keep; the block has none, and pays nothing for it.
        assert_eq!(tags(&bytes), ["MESH", "MASK", "MESH", "INFO"]);
        let back = load_scene(&path).unwrap();

        assert_eq!(back.meshes.len(), 2);
        assert_same_mesh(&back.meshes[0], &ball);
        assert_same_mesh(&back.meshes[1], &block);
        assert_same_objects(&back.objects, &objects);
        assert_eq!(back.active, 2);

        assert_eq!(back.details.get_over(slot::SHADING, &Shading::default()), Some(shading));
        assert_eq!(back.details.get(slot::CAMERA), Some(view));
        assert_eq!(back.details.get(slot::MIRROR), Some(mirror));
        assert_eq!(back.details.get_object(2, slot::SYMMETRY), Some([true, false, false]));
        assert_eq!(back.details.get_object::<[bool; 3]>(0, slot::SYMMETRY), None);

        // The whole look, field for field (compared as what would be saved).
        let got: Look = back.details.get_over(slot::LOOK, &Look::default()).unwrap();
        assert_eq!(serde_json::to_value(&got).unwrap(), serde_json::to_value(&options.look).unwrap());
        assert_eq!((got.finish, got.metal.preset, got.metal.gloss), (Finish::Metal, 3, 0.4));
        assert_eq!((got.lighting, got.five_point, got.follow_camera), (5, true, false));
        assert_eq!(got.world_forward, Vec3::new(0.6, 0.0, -0.8));
        assert_eq!(got.hdri.path.as_deref(), Some(Path::new("C:\\pictures\\studio small.hdr")));
        assert_eq!((got.hdri.enabled, got.hdri.rotation, got.hdri_background), (true, 45.0, true));
        assert_eq!((got.depth_of_field, got.focus_distance, got.aperture), (true, 2.5, 2.8));
        assert_eq!((got.bloom, got.grading, got.tonemap), (false, true, Tonemap::Aces));
        assert!(!got.hdri.loaded && got.hdri.status.is_empty(), "what is only true while running is not kept");

        // Presets are written by name, so adding one to a list later does
        // not change what an old file means.
        let (_, info) = blocks(&bytes).pop().unwrap();
        let text = std::str::from_utf8(&bytes[info]).unwrap();
        for id in [crate::look::METALS[3].id, crate::look::LIGHTING[5].id, crate::look::GLOWS[4].id] {
            assert!(text.contains(&format!("\"{id}\"")), "{id} is not in {text}");
        }
    }

    #[test]
    fn a_save_that_does_not_finish_leaves_the_old_file_whole() {
        let (ball, block) = (sphere(2_000), primitives::build(Primitive::Cube, 500, 0.5));
        let path = temp("kept_safe.sclp");
        let beside = temp_beside(&path);
        save_scene(&[&ball], &[object("Ball", 0, 0.0)], 0, &path).unwrap();
        let before = std::fs::read(&path).unwrap();

        // Interrupted after the new file was written and before it took the
        // old one's place (the power went, the app was killed): the old file
        // has not been touched.
        let info = Details::default().document(&[object("Block", 0, 0.0)], 0);
        let waiting = write_beside(&path, |w| write_scene(w, &[&block], &info)).unwrap();
        assert_eq!(waiting, beside);
        assert!(beside.is_file());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(load_scene(&path).unwrap().objects[0].name, "Ball");

        // The write itself failing part-way (a full disk): an error in plain
        // words, the old file whole, and nothing left lying beside it.
        let failed = replace_file(&path, |w| {
            w.write_all(&before[..before.len() / 2])?;
            Err(std::io::Error::other("there is not enough space on the disk"))
        });
        assert!(failed.unwrap_err().contains("not enough space"));
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(!beside.exists());

        // A save that does finish replaces the old file, in one step.
        let waiting = write_beside(&path, |w| write_scene(w, &[&block], &info)).unwrap();
        commit(&waiting, &path).unwrap();
        assert!(!beside.exists());
        let back = load_scene(&path).unwrap();
        assert_eq!(back.objects[0].name, "Block");
        assert_same_mesh(&back.meshes[0], &block);

        // A folder that is not there yet is made; somewhere that cannot be
        // written says so and leaves nothing behind.
        let deeper = temp("made_on_save").join("inner").join("scene.sclp");
        let _ = std::fs::remove_dir_all(temp("made_on_save"));
        save_scene(&[&ball], &[object("Ball", 0, 0.0)], 0, &deeper).unwrap();
        assert!(deeper.is_file());
        let impossible = path.join("not a folder.sclp");
        assert!(save_scene(&[&ball], &[object("Ball", 0, 0.0)], 0, &impossible).is_err());
    }

    #[test]
    fn blocks_and_fields_from_a_later_version_are_stepped_over() {
        let ball = sphere(1_000);
        let path = temp("from_a_later_version.sclp");
        save_scene(&[&ball], &[object("Ball", 0, 0.0)], 0, &path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let parts = blocks(&bytes);
        assert_eq!(tags(&bytes), ["MESH", "INFO"]);

        // The same file as a later version might write it: blocks this one
        // has never heard of, and more in the JSON than it knows what to do with.
        let mut info: Value = serde_json::from_slice(&bytes[parts[1].1.clone()]).unwrap();
        info["hologram"] = json!({ "layers": [1, 2, 3], "tint": "teal" });
        info["objects"][0]["tentacles"] = json!(8);
        info["look"] = json!({ "brightness": 1.75, "some_future_dial": 11 });
        info.as_object_mut().unwrap().remove("active");
        let info = serde_json::to_vec(&info).unwrap();
        let mut later = b"SCLP0004".to_vec();
        let mut put = |tag: &[u8; 4], body: &[u8]| {
            later.extend_from_slice(tag);
            later.extend_from_slice(&(body.len() as u64).to_le_bytes());
            later.extend_from_slice(body);
        };
        put(b"WXYZ", b"something new, before the meshes");
        put(MESH, &bytes[parts[0].1.clone()]);
        put(b"WXYZ", &[]);
        put(b"NEW!", &[7u8; 1000]);
        put(INFO, &info);
        std::fs::write(&path, later).unwrap();

        let back = load_scene(&path).unwrap();
        assert_same_mesh(&back.meshes[0], &ball);
        assert_same_objects(&back.objects, &[object("Ball", 0, 0.0)]);
        assert_eq!(back.active, 0, "a missing field takes its usual value");
        // What it does not know it keeps hold of, for whoever does.
        assert_eq!(back.details.get::<Value>("hologram").unwrap()["tint"], "teal");
        assert_eq!(back.details.get_object::<u32>(0, "tentacles"), Some(8));
        // A look with one field in it: that field, and the rest as the app has them.
        let look: Look = back.details.get_over(slot::LOOK, &Look::default()).unwrap();
        assert_eq!(look.brightness, 1.75);
        assert_eq!(look.aperture, Look::default().aperture);
        assert!(back.details.get::<Mirror>(slot::MIRROR).is_none(), "and a part that is not there is simply not there");
    }

    /// A file from before radial symmetry was kept has the mirror axes and
    /// nothing else: it opens with them, and with radial symmetry off.
    #[test]
    fn mirror_settings_from_before_radial_symmetry_still_read() {
        let mut details = Details::default();
        details.set(slot::MIRROR, &json!({ "x": true, "y": false, "z": true, "guides": false }));
        let usual = Mirror::default();
        let mirror: Mirror = details.get_over(slot::MIRROR, &usual).unwrap();
        assert_eq!((mirror.x, mirror.y, mirror.z, mirror.guides), (true, false, true, false));
        assert_eq!(mirror.radial, Radial::default());
        assert!(!mirror.radial.on && mirror.radial.count == 8 && mirror.radial.axis == RadialAxis::Y);

        // As it is written now, and read back by name.
        let radial = Radial { on: true, count: 12, axis: RadialAxis::Z, custom_axis: Vec3::new(0.0, 0.5, -0.75) };
        details.set(slot::MIRROR, &Mirror { radial, ..usual });
        let written: Value = details.get(slot::MIRROR).unwrap();
        assert_eq!(written["radial"], json!({ "on": true, "count": 12, "axis": "z", "custom_axis": [0.0, 0.5, -0.75] }));
        assert_eq!(details.get_over(slot::MIRROR, &usual), Some(Mirror { radial, ..usual }));

        // An axis this version has never heard of, and a count that is not
        // one, are passed over; what is beside them is still read.
        details.set(slot::MIRROR, &json!({ "x": true, "radial": { "on": true, "count": "lots", "axis": "w" } }));
        let mirror: Mirror = details.get_over(slot::MIRROR, &usual).unwrap();
        assert!(mirror.x && mirror.radial.on);
        assert_eq!((mirror.radial.count, mirror.radial.axis), (8, RadialAxis::Y));

        // Numbers out of range are brought into it before they are used.
        let wild = Radial { on: true, count: 0, axis: RadialAxis::Custom, custom_axis: Vec3::ZERO };
        assert_eq!(wild.sanitized(), Radial { count: 2, custom_axis: Vec3::Y, ..wild });
        assert_eq!(Radial { count: 500, ..wild }.sanitized().count, 32);
        assert_eq!(Radial { custom_axis: Vec3::new(0.0, 0.0, -3.0), ..wild }.sanitized().custom_axis, Vec3::NEG_Z);
        assert_eq!(Radial { custom_axis: Vec3::new(f32::NAN, 1.0, 0.0), ..wild }.sanitized().custom_axis, Vec3::Y);
    }

    #[test]
    fn a_look_that_does_not_all_read_keeps_what_does() {
        let mut details = Details::default();
        details.set(
            slot::LOOK,
            &json!({
                "brightness": 1.75,
                "finish": "Chrome Dipped",
                "metal": { "gloss": 0.25, "preset": "no-such-metal" },
                "clay": { "gloss": 0.5, "preset": crate::look::CLAYS[1].id },
                "wood": { "grain": "lots", "sheen": 0.3 },
                "bloom": null,
                "tonemap": "Aces",
                "lighting": crate::look::LIGHTING[2].id,
            }),
        );
        let look: Look = details.get_over(slot::LOOK, &Look::default()).unwrap();
        let usual = Look::default();
        assert_eq!(look.brightness, 1.75);
        assert_eq!(look.tonemap, Tonemap::Aces);
        assert_eq!(look.lighting, 2);
        assert_eq!(look.finish, usual.finish, "a finish this version has never heard of");
        // A preset it does not have is left as it was; the dial beside it is still read.
        assert_eq!((look.metal.preset, look.metal.gloss), (usual.metal.preset, 0.25));
        assert_eq!((look.clay.preset, look.clay.gloss), (1, 0.5));
        // A word where a number belongs is passed over; the number beside it is read.
        assert_eq!((look.wood.grain, look.wood.sheen), (usual.wood.grain, 0.3));
        assert_eq!(look.bloom, usual.bloom, "null is the same as not being there");

        // A camera that makes no sense is refused, and the view stays put.
        let mut camera = crate::camera::Camera::default();
        let was = camera.saved_view();
        for bad in [SavedView { distance: 0.0, ..was }, SavedView { yaw: f32::NAN, ..was }, SavedView { pivot: [f32::INFINITY, 0.0, 0.0], ..was }] {
            assert!(!camera.restore_view(&bad));
            assert_eq!(camera.saved_view(), was);
        }
        let good = SavedView { yaw: -60.0, pitch: 30.0, distance: 2.0, pivot: [1.0, 2.0, 3.0], ortho: true, fov_y: 60.0 };
        assert!(camera.restore_view(&good));
        assert_eq!(camera.saved_view(), good);
    }

    #[test]
    fn a_damaged_file_is_refused_not_half_opened() {
        let mut ball = sphere(1_000);
        ball.mask[0] = 1.0;
        let path = temp("damaged.sclp");
        save_scene(&[&ball], &[object("Ball", 0, 0.0)], 0, &path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let parts = blocks(&bytes);
        assert_eq!(tags(&bytes), ["MESH", "MASK", "INFO"]);
        let refused = |what: &str, damaged: &[u8]| {
            std::fs::write(&path, damaged).unwrap();
            let result = load_scene(&path);
            assert!(result.is_err(), "{what} opened");
            result.err().unwrap()
        };

        // Cut short: inside each block, exactly between blocks, by one byte.
        let inside_mesh = (parts[0].1.start + parts[0].1.end) / 2;
        for end in [3, 8, 14, inside_mesh, parts[0].1.end, parts[1].1.end, parts[2].1.start, bytes.len() - 1] {
            refused(&format!("a file cut off after {end} bytes"), &bytes[..end]);
        }
        // Counts that lie: a block longer than the file, a mesh with more
        // vertices than its block has room for. Neither may be believed
        // (and given memory) before the file has been checked.
        let mut long = bytes.clone();
        long[12..20].copy_from_slice(&u64::MAX.to_le_bytes());
        refused("a block longer than the file", &long);
        let mut crowded = bytes.clone();
        crowded[parts[0].1.start..parts[0].1.start + 8].copy_from_slice(&(u64::MAX / 16).to_le_bytes());
        refused("a mesh with an impossible vertex count", &crowded);
        // A mask for a mesh that is not there.
        let mut stray = bytes.clone();
        stray[parts[1].1.start..parts[1].1.start + 4].copy_from_slice(&9u32.to_le_bytes());
        refused("a mask for no mesh", &stray);
        // A position that is not a number would spread through every brush.
        let mut nan = bytes.clone();
        nan[parts[0].1.start + 16..parts[0].1.start + 20].copy_from_slice(&f32::NAN.to_le_bytes());
        refused("a vertex that is not a number", &nan);
        // JSON that is not JSON, or names a mesh the file does not have.
        let rewrite = |info: &[u8]| {
            let mut out = bytes[..parts[2].1.start - 12].to_vec();
            out.extend_from_slice(INFO);
            out.extend_from_slice(&(info.len() as u64).to_le_bytes());
            out.extend_from_slice(info);
            out
        };
        refused("broken JSON", &rewrite(b"{ \"objects\": ["));
        refused("an object of a missing mesh", &rewrite(br#"{ "objects": [{ "name": "Ball", "mesh": 4 }] }"#));
        refused("a scene with no objects", &rewrite(br#"{ "objects": [] }"#));

        // Not this app's file at all, and one from a version yet to come.
        assert!(refused("some other file", b"hello there, I am a text file").contains("not a sculpt file"));
        refused("an empty file", b"");
        let mut newer = bytes.clone();
        newer[..8].copy_from_slice(b"SCLP0005");
        assert!(refused("a newer file", &newer).contains("newer version"));

        // Whereas an object with only a name and a mesh is given the usual
        // place, and the undamaged file still opens.
        std::fs::write(&path, rewrite(br#"{ "objects": [{ "name": "Ball", "mesh": 0, "scale": [0, 1, 1] }] }"#)).unwrap();
        let back = load_scene(&path).unwrap();
        assert_eq!((back.objects[0].position, back.objects[0].rotation, back.objects[0].scale), (Vec3::ZERO, Quat::IDENTITY, Vec3::ONE));
        std::fs::write(&path, &bytes).unwrap();
        assert_same_mesh(&load_scene(&path).unwrap().meshes[0], &ball);
    }

    /// Each face of a mesh as the exact positions of its corners, in an
    /// order that does not depend on the mesh's own numbering.
    fn exact_faces<'a>(mesh: &Mesh, faces: impl Iterator<Item = &'a [u32]>) -> Vec<Vec<[u32; 3]>> {
        let mut all: Vec<Vec<[u32; 3]>> = faces
            .map(|corners| {
                let mut face: Vec<[u32; 3]> = corners.iter().map(|&i| mesh.positions[i as usize].to_array().map(f32::to_bits)).collect();
                face.sort_unstable();
                face
            })
            .collect();
        all.sort_unstable();
        all
    }

    fn quads_of(mesh: &Mesh) -> Vec<Vec<[u32; 3]>> {
        let polys = mesh.polys.as_ref().expect("the mesh has its quads");
        exact_faces(mesh, polys.start.windows(2).map(|f| &polys.indices[f[0] as usize..f[1] as usize]))
    }

    fn hidden_faces_of(mesh: &Mesh) -> Vec<Vec<[u32; 3]>> {
        let hidden = mesh.hidden_triangles().expect("the mesh has hidden faces");
        exact_faces(mesh, mesh.indices.chunks_exact(3).zip(hidden).filter(|(_, hidden)| **hidden).map(|(t, _)| t))
    }

    #[test]
    fn quads_and_hidden_faces_are_kept() {
        // A quad-remeshed object keeps its quads.
        let cube = quad_cube();
        // Part of a sculpt hidden with the box or lasso stays hidden.
        let mut ball = sphere(4_000);
        let lower: Vec<u32> =
            (0..ball.triangle_count() as u32).filter(|&t| ball.positions[ball.indices[t as usize * 3] as usize].y < 0.1).collect();
        ball.hide(&lower);
        assert!(ball.hidden_triangle_count() > 500 && ball.visible_triangle_count() > 500);
        // And a mesh can have both.
        let mut both = quad_cube();
        both.hide(&[0, 1, 2, 7]);

        let path = temp("quads_and_hidden.sclp");
        let objects = [object("Cube", 0, 0.0), object("Ball", 1, 1.0), object("Both", 2, 2.0)];
        save_scene(&[&cube, &ball, &both], &objects, 0, &path).unwrap();
        assert_eq!(tags(&std::fs::read(&path).unwrap()), ["MESH", "QUAD", "MESH", "HIDE", "MESH", "QUAD", "HIDE", "INFO"]);
        let back = load_scene(&path).unwrap();

        assert_same_mesh(&back.meshes[0], &cube);
        assert_eq!(quads_of(&back.meshes[0]).len(), 6);
        assert_eq!(quads_of(&back.meshes[0]), quads_of(&cube));
        assert!(!back.meshes[0].any_hidden());

        assert_same_mesh(&back.meshes[1], &ball);
        assert_eq!(back.meshes[1].hidden_triangle_count(), ball.hidden_triangle_count());
        assert_eq!(hidden_faces_of(&back.meshes[1]), hidden_faces_of(&ball), "the same faces, not just as many");
        assert!(back.meshes[1].polys.is_none());

        assert_eq!(quads_of(&back.meshes[2]), quads_of(&both));
        assert_eq!(hidden_faces_of(&back.meshes[2]), hidden_faces_of(&both));
    }

    /// Not a check but a measurement, so it does not run with the others:
    /// `cargo test --target-dir target/w/app four_million -- --ignored --nocapture`
    #[test]
    #[ignore = "a timing of saving and loading a large scene; run it by name"]
    fn a_four_million_triangle_scene_saves_and_loads_at_disk_speed() {
        use std::time::Instant;
        let mut mesh = sphere(4_000_000);
        for v in 0..mesh.vertex_count() {
            mesh.mask[v] = if mesh.positions[v].x > 0.0 { 1.0 } else { 0.0 };
        }
        let (tris, verts) = (mesh.triangle_count(), mesh.vertex_count());
        let path = temp("four_million.sclp");
        let objects = [object("Ball", 0, 0.0)];
        let megabytes = |bytes: u64| bytes as f64 / 1e6;
        let ms = |since: Instant| since.elapsed().as_secs_f64() * 1000.0;

        let _ = std::fs::remove_file(&path);
        let start = Instant::now();
        save_scene(&[&mesh], &objects, 0, &path).unwrap();
        let first_save = ms(start);
        let start = Instant::now();
        save_scene(&[&mesh], &objects, 0, &path).unwrap();
        let save_over = ms(start);
        let size = megabytes(std::fs::metadata(&path).unwrap().len());

        let start = Instant::now();
        let mut file = File::open(&path).unwrap();
        file.read_exact(&mut [0u8; 8]).unwrap();
        let loaded = read_blocks(file, &path).unwrap();
        let read = ms(start);
        let start = Instant::now();
        let scene = loaded.into_scene();
        let build = ms(start);
        assert_eq!((scene.meshes[0].triangle_count(), scene.meshes[0].vertex_count()), (tris, verts));

        println!("{tris} triangles, {verts} vertices, {size:.1} MB on disk");
        println!("  save, new file:          {first_save:7.0} ms  ({:.0} MB/s, flushed to the disk)", size / first_save * 1000.0);
        println!("  save over the old file:  {save_over:7.0} ms  ({:.0} MB/s)", size / save_over * 1000.0);
        println!("  load, reading the file:  {read:7.0} ms  ({:.0} MB/s)", size / read * 1000.0);
        println!("  load, building the mesh: {build:7.0} ms  (spatial index, adjacency, normals: the same work as an import)");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn nothing_to_export_says_so() {
        let empty = Mesh::new(Vec::new(), Vec::new(), None);
        assert!(export_stl(&empty, Mat4::IDENTITY, &temp("empty.stl")).is_err());
        assert!(export_obj(&empty, Mat4::IDENTITY, &temp("empty.obj")).is_err());
    }
}
