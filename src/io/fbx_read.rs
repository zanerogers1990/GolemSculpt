//! Import of Autodesk FBX, for models from ZBrush, Blender, Maya and the
//! other programs that write one.
//!
//! Only binary FBX is read, versions 7.1 to 7.7 (every FBX written since
//! 2010, and what every exporter writes unless told otherwise). A text FBX
//! is turned away with a word on how to export the binary kind.
//!
//! The file is 27 bytes of heading (a fixed text, then the version), then a
//! tree of named records, as `fbx.rs` writes it. Each record starts with
//! where in the file it ends, how many values it holds and their length in
//! bytes, and its name; then come its values, then its children, closed by an
//! empty record. From 7.5 on those first three numbers are eight bytes each,
//! not four. A value is a letter saying its kind, then the value: a number,
//! text, raw bytes, or an array of numbers that may be zlib-packed.
//!
//! Reading builds the tree with every value left where it lies in the file,
//! so the only arrays ever unpacked are the few a mesh needs (its points, its
//! corners, its colours), each once and straight into its own `Vec`;
//! animation and everything else is stepped over without being decoded. A
//! file of tens of millions of polygons is two passes over two arrays. Every
//! offset and length is checked against the bytes really there before it is
//! used or anything is set aside for it, so a damaged file gives a plain
//! error, never a crash.
//!
//! What is taken from the file:
//! - **Meshes and models**: a `Geometry` of class "Mesh" holds `Vertices`
//!   (x, y, z for each point) and `PolygonVertexIndex` (each polygon's
//!   corners in turn, the last of them written as minus itself, less one). A
//!   `Model` is a thing in the scene, placed by its `Properties70`.
//!   `Connections` (`C "OO" child parent`) hang a geometry from a model, and a
//!   model from another model or from the scene.
//! - **One object per model with a mesh**, named as the model is: ZBrush's
//!   FBX Export puts each subtool in a model of its own, so each comes in as
//!   an object. A mesh two models share comes in twice, once in each place; a
//!   mesh hung from no model comes in where it is, under its own name.
//! - **Places**: a model's place is FBX's full chain, T · Roff · Rp · Rpre ·
//!   R · Rpost⁻¹ · Rp⁻¹ · Soff · Sp · S · Sp⁻¹, under its parent's place, and
//!   then its geometric offset (which its children do not take on).
//!   Rotations are Euler angles in degrees, in the model's `RotationOrder`:
//!   XYZ, the default, turns about X first, so R = Rz · Ry · Rx. Pre- and
//!   post-rotation always go XYZ and, as in Autodesk's own reader and
//!   Blender's, count (with the rotation order) only when `RotationActive` is
//!   set. Each place is baked into its object's points. A place that mirrors
//!   (an odd number of negative scales) also turns every polygon's corners
//!   round, so the faces still look outward.
//! - **Axes and units** (`GlobalSettings`): the file says which of its axes
//!   points up (`UpAxis`, `UpAxisSign`), which toward the viewer (`FrontAxis`,
//!   `FrontAxisSign`) and which to the right (`CoordAxis`, `CoordAxisSign`),
//!   and how many centimetres one of its units is (`UnitScaleFactor`). Every
//!   point is turned so the file's up is the app's +Y, its front +Z and its
//!   right +X (the app is right-handed, Y up), and scaled to the app's
//!   centimetre, so the model keeps its real size. A ZBrush or Maya file says
//!   Y up and is left as it is. Blender's exporter, by default, turns its
//!   Z-up scene itself (a turn of -90° about X on each top model) and says Y
//!   up, so composing the places stands it up; a file that says Z up (3ds
//!   Max, or Blender told to keep Z) is turned here, its (x, y, z) becoming
//!   (x, z, -y). Blender's metres arrive either as a unit of 100 cm or as a
//!   scale of 100 on each top model, and come to the same size either way.
//! - **Colour**: the first `LayerElementColor`, given per point or per corner
//!   (a point then takes the colour of its first corner), becomes paint, its
//!   alpha saying how much paint covers the point (the app's exports write
//!   1). A layer that is white or has no alpha wherever it says anything
//!   (Blender's default white, 3D-Coat's grey at alpha 0) is what an
//!   unpainted model carries, and is left out; one plain colour all over is
//!   kept, as game models often colour each part so.
//! - **UVs**: UV set 0, its `LayerElementUV`, one for each polygon corner
//!   (`fbx_read/uvs.rs`), turned round with the corners where a place
//!   mirrors.
//!
//! Everything else (materials, normals, which the app works out again,
//! animation, skins, blend shapes, NURBS, cameras, lights) is passed over.
//! The polygons go to `polygons.rs`, which keeps quads as quads, so a ZBrush
//! model's subdivision levels work on them.

use std::borrow::Cow;
use std::collections::HashMap;

use glam::{DMat3, DMat4, DVec3, Vec2, Vec3};
use miniz_oxide::inflate::TINFLStatus;
use miniz_oxide::inflate::core::{DecompressorOxide, decompress, inflate_flags};
use rayon::prelude::*;
use sculpt_geom::units;

use super::polygons::{Faces, raw_mesh};
use super::{RawMesh, Result};
use crate::mesh::paint::Rgba8;

#[cfg(test)]
mod tests;
mod uvs;

const MAGIC: &[u8; 23] = b"Kaydara FBX Binary  \x00\x1a\x00";
/// Where the records begin: after the heading and the version.
const FIRST_RECORD: usize = MAGIC.len() + 4;
/// Deeper than any real file nests its records. A damaged one could claim
/// any depth, and must not run the reader out of stack.
const DEEPEST: usize = 64;
const DAMAGED: &str = "This FBX file is damaged: it ends early or its contents do not add up. Try exporting it again.";
/// A point the colour layer gives nothing for: no paint on it.
const UNPAINTED: Rgba8 = Rgba8([0; 4]);

/// The objects of a binary FBX file, in the order its models come.
pub(super) fn parse(bytes: &[u8]) -> Result<Vec<RawMesh>> {
    let top = read_tree(bytes)?;
    let find = |name: &[u8]| top.iter().find(|r| r.name == name);
    let objects = find(b"Objects").ok_or("This FBX file holds no objects.")?;
    let space = file_space(find(b"GlobalSettings"));

    let mut meshes: Vec<(i64, &Record)> = Vec::new();
    let mut models: Vec<(i64, &Record)> = Vec::new();
    for record in &objects.children {
        let Some(id) = record.id() else { continue };
        match record.name {
            b"Geometry" if record.text(2) == Some(b"Mesh") => meshes.push((id, record)),
            b"Model" => models.push((id, record)),
            _ => {}
        }
    }
    if meshes.is_empty() {
        return Err("This FBX file holds no mesh to sculpt (only things such as animation, cameras or curves).".into());
    }
    let mesh_at: HashMap<i64, usize> = meshes.iter().enumerate().map(|(at, &(id, _))| (id, at)).collect();
    let model_at: HashMap<i64, usize> = models.iter().enumerate().map(|(at, &(id, _))| (id, at)).collect();

    // Which meshes each model holds, and which model each hangs from.
    let mut held: Vec<Vec<usize>> = vec![Vec::new(); models.len()];
    let mut parent: Vec<Option<usize>> = vec![None; models.len()];
    let mut placed = vec![false; meshes.len()];
    let links = find(b"Connections").map_or(&[][..], |c| c.children.as_slice());
    for link in links.iter().filter(|c| c.name == b"C" && c.text(0) == Some(b"OO")) {
        let (Some(child), Some(up)) = (link.long(1), link.long(2)) else { continue };
        let Some(&model) = model_at.get(&up) else { continue };
        if let Some(&mesh) = mesh_at.get(&child) {
            held[model].push(mesh);
            placed[mesh] = true;
        } else if let Some(&below) = model_at.get(&child) {
            if below != model && parent[below].is_none() {
                parent[below] = Some(model);
            }
        }
    }

    let locals: Vec<DMat4> = models.iter().map(|(_, model)| local_place(model)).collect();
    // A model's place in the scene: its own under each of its parents' in
    // turn. A damaged file could hang models from one another in a ring, so
    // the climb stops after as many steps as there are models.
    let global = |model: usize| {
        let (mut place, mut at) = (locals[model], model);
        for _ in 0..models.len() {
            let Some(up) = parent[at] else { break };
            place = locals[up] * place;
            at = up;
        }
        place
    };

    let mut out = Vec::new();
    for (m, (_, model)) in models.iter().enumerate().filter(|(m, _)| !held[*m].is_empty()) {
        let place = space * global(m) * geometric_place(model);
        for &mesh in &held[m] {
            out.extend(mesh_of(meshes[mesh].1, name_of(model), place)?);
        }
    }
    for (_, geometry) in meshes.iter().zip(&placed).filter(|(_, placed)| !**placed).map(|(mesh, _)| mesh) {
        out.extend(mesh_of(geometry, name_of(geometry), space)?);
    }
    Ok(out)
}

// ---------------------------------------------------------------- the records

/// One value of a record, still where it lies in the file.
enum Value<'a> {
    Bool(bool),
    Short(i16),
    Int(i32),
    Float(f32),
    Double(f64),
    Long(i64),
    Text(&'a [u8]),
    /// Raw bytes (a file id, a thumbnail), which nothing here needs.
    Raw,
    Array(Array<'a>),
}

impl Value<'_> {
    fn number(&self) -> Option<f64> {
        match *self {
            Value::Bool(v) => Some(f64::from(u8::from(v))),
            Value::Short(v) => Some(f64::from(v)),
            Value::Int(v) => Some(f64::from(v)),
            Value::Float(v) => Some(f64::from(v)),
            Value::Double(v) => Some(v),
            Value::Long(v) => Some(v as f64),
            Value::Text(_) | Value::Raw | Value::Array(_) => None,
        }
    }
}

/// An array of numbers as the file holds it: its kind's letter, how many
/// numbers, whether they are zlib-packed, and the bytes.
#[derive(Clone, Copy)]
struct Array<'a> {
    kind: u8,
    count: usize,
    packed: bool,
    data: &'a [u8],
}

struct Record<'a> {
    name: &'a [u8],
    values: Vec<Value<'a>>,
    children: Vec<Record<'a>>,
}

impl<'a> Record<'a> {
    fn child(&self, name: &[u8]) -> Option<&Record<'a>> {
        self.children.iter().find(|c| c.name == name)
    }

    fn text(&self, at: usize) -> Option<&'a [u8]> {
        match self.values.get(at) {
            Some(Value::Text(text)) => Some(text),
            _ => None,
        }
    }

    fn long(&self, at: usize) -> Option<i64> {
        match self.values.get(at)? {
            Value::Long(v) => Some(*v),
            Value::Int(v) => Some(i64::from(*v)),
            _ => None,
        }
    }

    /// An object's id, which is its first value.
    fn id(&self) -> Option<i64> {
        self.long(0)
    }

    /// The array a child record of this name holds.
    fn array(&self, name: &[u8]) -> Option<Array<'a>> {
        match self.child(name)?.values.first()? {
            Value::Array(array) => Some(*array),
            _ => None,
        }
    }

    /// The text a child record of this name holds.
    fn child_text(&self, name: &[u8]) -> Option<&'a [u8]> {
        self.child(name)?.text(0)
    }

    /// The values of a `Properties70` line, after its name and its three
    /// words of kind and flags.
    fn property(&self, name: &[u8]) -> Option<&[Value<'a>]> {
        let line = self.child(b"Properties70")?.children.iter().find(|p| p.name == b"P" && p.text(0) == Some(name))?;
        Some(line.values.get(4..).unwrap_or(&[]))
    }

    fn number(&self, name: &[u8], default: f64) -> f64 {
        self.property(name).and_then(|v| v.first()?.number()).filter(|n| n.is_finite()).unwrap_or(default)
    }

    fn vector(&self, name: &[u8], default: DVec3) -> DVec3 {
        self.property(name)
            .and_then(|v| Some(DVec3::new(v.first()?.number()?, v.get(1)?.number()?, v.get(2)?.number()?)))
            .filter(|v| v.is_finite())
            .unwrap_or(default)
    }
}

/// An object's name as FBX keeps it is the name, then `\0\x01`, then what
/// kind of thing it is; only the name is wanted.
fn name_of(record: &Record) -> String {
    let full = record.text(1).unwrap_or(b"");
    let cut = full.windows(2).position(|w| w == b"\x00\x01").unwrap_or(full.len());
    String::from_utf8_lossy(&full[..cut]).into_owned()
}

/// Reads the bytes of a file in order, never past `limit` (the end of the
/// record or values being read).
struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
    limit: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.at.checked_add(n).filter(|&end| end <= self.limit && end <= self.bytes.len()).ok_or(DAMAGED)?;
        let held = &self.bytes[self.at..end];
        self.at = end;
        Ok(held)
    }

    fn fixed<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut out = [0; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.fixed()?))
    }

    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.fixed()?))
    }

    fn length(&mut self) -> Result<usize> {
        Ok(self.u32()? as usize)
    }

    fn value(&mut self) -> Result<Value<'a>> {
        let [kind] = self.fixed()?;
        Ok(match kind {
            b'C' => Value::Bool(self.fixed::<1>()?[0] != 0),
            b'Y' => Value::Short(i16::from_le_bytes(self.fixed()?)),
            b'I' => Value::Int(i32::from_le_bytes(self.fixed()?)),
            b'F' => Value::Float(f32::from_le_bytes(self.fixed()?)),
            b'D' => Value::Double(f64::from_le_bytes(self.fixed()?)),
            b'L' => Value::Long(i64::from_le_bytes(self.fixed()?)),
            b'S' => {
                let length = self.length()?;
                Value::Text(self.take(length)?)
            }
            b'R' => {
                let length = self.length()?;
                self.take(length)?;
                Value::Raw
            }
            b'f' | b'd' | b'l' | b'i' | b'b' => {
                let (count, encoding, length) = (self.length()?, self.u32()?, self.length()?);
                let data = self.take(length)?;
                if encoding > 1 {
                    return Err("This FBX file packs its numbers in a way this app does not know.".into());
                }
                Value::Array(Array { kind, count, packed: encoding == 1, data })
            }
            _ => return Err(DAMAGED.into()),
        })
    }
}

/// The records at the top of the file, after its heading is checked.
fn read_tree(bytes: &[u8]) -> Result<Vec<Record<'_>>> {
    if !bytes.starts_with(MAGIC) {
        return Err(if looks_like_text(bytes) {
            "This FBX is the text kind; export it as binary FBX.".into()
        } else {
            "This is not an FBX file: it does not begin as one does.".into()
        });
    }
    let version = Cursor { bytes, at: MAGIC.len(), limit: bytes.len() }.u32()?;
    let named = format!("{}.{}", version / 1000, version % 1000 / 100);
    if version < 7100 {
        return Err(format!("This FBX is an old kind (version {named}, from before 2011). Export it again as FBX 2011 or later."));
    }
    if version >= 8000 {
        return Err(format!("This FBX is a newer kind (version {named}) than this app reads. Export it as FBX 2020 or earlier."));
    }
    let tree = Tree { bytes, wide: version >= 7500 };
    let mut top = Vec::new();
    let mut at = FIRST_RECORD;
    // The top family ends with an empty record, as every family does; a file
    // that runs out before it is cut short.
    loop {
        let (record, next) = tree.record(at, bytes.len(), 0)?;
        at = next;
        match record {
            Some(record) => top.push(record),
            None => return Ok(top),
        }
    }
}

/// Whether the bytes look like a text FBX: it begins "; FBX 7.4.0 project
/// file", and names its records in words.
fn looks_like_text(bytes: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]);
    head.trim_start_matches('\u{feff}').trim_start().starts_with("; FBX") || head.contains("FBXHeaderExtension")
}

struct Tree<'a> {
    bytes: &'a [u8],
    /// From version 7.5 a record's first three numbers are eight bytes each.
    wide: bool,
}

impl<'a> Tree<'a> {
    /// Reads the record at `at`, which must end by `limit`, and returns it
    /// with where it ends: `None` for the empty record that closes a family.
    fn record(&self, at: usize, limit: usize, depth: usize) -> Result<(Option<Record<'a>>, usize)> {
        let mut cursor = Cursor { bytes: self.bytes, at, limit };
        let (end, count, values_len) = if self.wide {
            (cursor.u64()?, cursor.u64()?, cursor.u64()?)
        } else {
            (u64::from(cursor.u32()?), u64::from(cursor.u32()?), u64::from(cursor.u32()?))
        };
        let [name_len] = cursor.fixed()?;
        if end == 0 {
            return Ok((None, cursor.at));
        }
        let end = usize::try_from(end).ok().filter(|&end| end > at && end <= limit).ok_or(DAMAGED)?;
        if depth > DEEPEST {
            return Err(DAMAGED.into());
        }
        cursor.limit = end;
        let name = cursor.take(usize::from(name_len))?;
        let values_end = usize::try_from(values_len)
            .ok()
            .and_then(|len| cursor.at.checked_add(len))
            .filter(|&values_end| values_end <= end)
            .ok_or(DAMAGED)?;
        cursor.limit = values_end;
        // Every value takes two bytes at the least, which bounds how many
        // there can be before any room is set aside for them.
        let count = usize::try_from(count).ok().filter(|&count| count <= (values_end - cursor.at) / 2).ok_or(DAMAGED)?;
        let mut values = Vec::with_capacity(count);
        for _ in 0..count {
            values.push(cursor.value()?);
        }
        let mut children = Vec::new();
        let mut at = values_end;
        while at < end {
            let (child, next) = self.record(at, end, depth + 1)?;
            at = next;
            match child {
                Some(child) => children.push(child),
                None => break,
            }
        }
        Ok((Some(Record { name, values, children }), end))
    }
}

// ---------------------------------------------------------------- the numbers

fn f64_at(bytes: &[u8], at: usize) -> f64 {
    let mut b = [0; 8];
    b.copy_from_slice(&bytes[at..at + 8]);
    f64::from_le_bytes(b)
}

fn f32_at(bytes: &[u8], at: usize) -> f32 {
    let mut b = [0; 4];
    b.copy_from_slice(&bytes[at..at + 4]);
    f32::from_le_bytes(b)
}

fn i32_at(bytes: &[u8], at: usize) -> i32 {
    let mut b = [0; 4];
    b.copy_from_slice(&bytes[at..at + 4]);
    i32::from_le_bytes(b)
}

fn i64_at(bytes: &[u8], at: usize) -> i64 {
    let mut b = [0; 8];
    b.copy_from_slice(&bytes[at..at + 8]);
    i64::from_le_bytes(b)
}

impl<'a> Array<'a> {
    fn width(&self) -> usize {
        match self.kind {
            b'd' | b'l' => 8,
            b'b' => 1,
            _ => 4,
        }
    }

    /// The array's bytes, unpacked if they were packed: borrowed from the
    /// file when they were not.
    fn bytes(&self) -> Result<Cow<'a, [u8]>> {
        let want = self.count.checked_mul(self.width()).ok_or(DAMAGED)?;
        if !self.packed {
            return if self.data.len() == want { Ok(Cow::Borrowed(self.data)) } else { Err(DAMAGED.into()) };
        }
        // zlib makes at most about a thousand bytes of each one it is given,
        // so a count past that is damage, caught before room is set aside.
        if want > self.data.len().saturating_mul(1100).saturating_add(64) {
            return Err(DAMAGED.into());
        }
        // Room for one byte more than wanted, so that data running past the
        // count shows as too long rather than stopping short of its end.
        let mut out = vec![0u8; want + 1];
        let flags = inflate_flags::TINFL_FLAG_PARSE_ZLIB_HEADER
            | inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF
            | inflate_flags::TINFL_FLAG_IGNORE_ADLER32;
        let (status, _, written) = decompress(&mut DecompressorOxide::default(), self.data, &mut out, 0, flags);
        if status != TINFLStatus::Done || written != want {
            return Err(DAMAGED.into());
        }
        out.truncate(want);
        Ok(Cow::Owned(out))
    }

    fn wrong_kind() -> String {
        "This FBX file is damaged: a list of numbers in it is of the wrong kind.".into()
    }

    fn doubles(&self) -> Result<Vec<f64>> {
        let bytes = self.bytes()?;
        Ok(match self.kind {
            b'd' => bytes.par_chunks_exact(8).map(|c| f64_at(c, 0)).collect(),
            b'f' => bytes.par_chunks_exact(4).map(|c| f64::from(f32_at(c, 0))).collect(),
            b'i' => bytes.par_chunks_exact(4).map(|c| f64::from(i32_at(c, 0))).collect(),
            b'l' => bytes.par_chunks_exact(8).map(|c| i64_at(c, 0) as f64).collect(),
            _ => return Err(Self::wrong_kind()),
        })
    }

    fn ints(&self) -> Result<Vec<i32>> {
        let bytes = self.bytes()?;
        let ints = ints_of(self.kind, &bytes)?.collect();
        Ok(ints)
    }

    /// The points of a `Vertices` array, each placed by `place`: worked in
    /// double precision, as the file has them, then kept as the app keeps them.
    fn points(&self, place: DMat4) -> Result<Vec<Vec3>> {
        let bytes = self.bytes()?;
        let put = |p: DVec3| place.transform_point3(p).as_vec3();
        Ok(match self.kind {
            b'd' => bytes.par_chunks_exact(24).map(|c| put(DVec3::new(f64_at(c, 0), f64_at(c, 8), f64_at(c, 16)))).collect(),
            b'f' => bytes
                .par_chunks_exact(12)
                .map(|c| put(DVec3::new(f64::from(f32_at(c, 0)), f64::from(f32_at(c, 4)), f64::from(f32_at(c, 8)))))
                .collect(),
            _ => return Err(Self::wrong_kind()),
        })
    }
}

/// The whole numbers of unpacked array bytes, as indices: a number too big
/// to be one becomes one that names nothing.
fn ints_of<'b>(kind: u8, bytes: &'b [u8]) -> Result<Box<dyn Iterator<Item = i32> + 'b>> {
    match kind {
        b'i' => Ok(Box::new(bytes.chunks_exact(4).map(|c| i32_at(c, 0)))),
        b'l' => Ok(Box::new(bytes.chunks_exact(8).map(|c| i32::try_from(i64_at(c, 0)).unwrap_or(i32::MAX)))),
        _ => Err(Array::wrong_kind()),
    }
}

// ---------------------------------------------------------------- places

/// Turns about X, then Y, then Z by these angles in degrees, or in another
/// order (FBX's `RotationOrder`: XYZ, XZY, YZX, YXZ, ZXY, ZYX; spherical,
/// the seventh, is taken as XYZ). The first named turns first, so it stands
/// rightmost.
fn euler(degrees: DVec3, order: i64) -> DMat4 {
    let x = DMat4::from_rotation_x(degrees.x.to_radians());
    let y = DMat4::from_rotation_y(degrees.y.to_radians());
    let z = DMat4::from_rotation_z(degrees.z.to_radians());
    match order {
        1 => y * z * x,
        2 => x * z * y,
        3 => z * x * y,
        4 => y * x * z,
        5 => x * y * z,
        _ => z * y * x,
    }
}

/// A model's place under its parent, from its `Properties70`, as Autodesk's
/// SDK works it out (see the module's notes).
fn local_place(model: &Record) -> DMat4 {
    let v = |name: &[u8]| model.vector(name, DVec3::ZERO);
    let move_by = DMat4::from_translation;
    let active = model.number(b"RotationActive", 0.0) != 0.0;
    let (order, pre, post) = if active {
        (model.number(b"RotationOrder", 0.0) as i64, v(b"PreRotation"), v(b"PostRotation"))
    } else {
        (0, DVec3::ZERO, DVec3::ZERO)
    };
    let (rotation_pivot, scaling_pivot) = (v(b"RotationPivot"), v(b"ScalingPivot"));
    move_by(v(b"Lcl Translation"))
        * move_by(v(b"RotationOffset"))
        * move_by(rotation_pivot)
        * euler(pre, 0)
        * euler(v(b"Lcl Rotation"), order)
        * euler(post, 0).transpose()
        * move_by(-rotation_pivot)
        * move_by(v(b"ScalingOffset"))
        * move_by(scaling_pivot)
        * DMat4::from_scale(model.vector(b"Lcl Scaling", DVec3::ONE))
        * move_by(-scaling_pivot)
}

/// A model's geometric offset: where its mesh sits within it, which the
/// model's children do not take on.
fn geometric_place(model: &Record) -> DMat4 {
    DMat4::from_translation(model.vector(b"GeometricTranslation", DVec3::ZERO))
        * euler(model.vector(b"GeometricRotation", DVec3::ZERO), 0)
        * DMat4::from_scale(model.vector(b"GeometricScaling", DVec3::ONE))
}

/// From the file's axes and units to the app's: Y up, Z toward the viewer,
/// X to the right, in centimetres (see the module's notes). Axes that do not
/// make three different ones are taken as FBX's default, which is the app's.
fn file_space(settings: Option<&Record>) -> DMat4 {
    let number = |name: &[u8], default: f64| settings.map_or(default, |s| s.number(name, default));
    let axis = |name: &[u8], default: f64| number(name, default) as i64;
    let sign = |name: &[u8]| if number(name, 1.0) < 0.0 { -1.0 } else { 1.0 };
    let (up, front, right) = (axis(b"UpAxis", 1.0), axis(b"FrontAxis", 2.0), axis(b"CoordAxis", 0.0));
    let mut sorted = [up, front, right];
    sorted.sort_unstable();
    let turn = if sorted == [0, 1, 2] {
        let mut columns = [DVec3::ZERO; 3];
        columns[up as usize] = DVec3::Y * sign(b"UpAxisSign");
        columns[front as usize] = DVec3::Z * sign(b"FrontAxisSign");
        columns[right as usize] = DVec3::X * sign(b"CoordAxisSign");
        DMat3::from_cols(columns[0], columns[1], columns[2])
    } else {
        DMat3::IDENTITY
    };
    let centimetres = number(b"UnitScaleFactor", 1.0);
    let centimetres = if centimetres > 0.0 { centimetres } else { 1.0 };
    let scale = centimetres * 10.0 / f64::from(units::MILLIMETRES_PER_UNIT);
    DMat4::from_mat3(turn * scale)
}

// ---------------------------------------------------------------- meshes

/// The object a mesh geometry makes, placed by `place`; `None` for one with
/// no points or no polygons.
fn mesh_of(geometry: &Record, name: String, place: DMat4) -> Result<Option<RawMesh>> {
    let (Some(points), Some(corners)) = (geometry.array(b"Vertices"), geometry.array(b"PolygonVertexIndex")) else {
        return Ok(None);
    };
    let positions = points.points(place)?;
    let corner_bytes = corners.bytes()?;
    let mirrored = DMat3::from_mat4(place).determinant() < 0.0;
    let uvs = uvs::uvs_of(geometry, ints_of(corners.kind, &corner_bytes)?);
    let faces = faces_of(ints_of(corners.kind, &corner_bytes)?, corners.count, mirrored, &uvs);
    let colours = colours_of(geometry, positions.len(), || ints_of(corners.kind, &corner_bytes))?;
    Ok(Some(raw_mesh(name, positions, &faces, true, colours)))
}

/// The polygons of a `PolygonVertexIndex` list, each turned round when the
/// place mirrors, with their corners' UVs if `uvs` has one for every corner.
/// A last polygon left open is closed.
fn faces_of(corners: impl Iterator<Item = i32>, count: usize, mirrored: bool, uvs: &[Vec2]) -> Faces {
    let mut faces = Faces::default();
    faces.corners.reserve(count);
    let given = !uvs.is_empty() && uvs.len() == count;
    let (mut polygon, mut polygon_uvs) = (Vec::new(), Vec::new());
    let mut close = |polygon: &mut Vec<u32>, polygon_uvs: &mut Vec<Vec2>| {
        if mirrored {
            polygon.reverse();
            polygon_uvs.reverse();
        }
        faces.push_uvs(polygon, None, given.then_some(&polygon_uvs[..]));
        polygon.clear();
        polygon_uvs.clear();
    };
    for (k, corner) in corners.enumerate() {
        polygon.push(if corner < 0 { !corner } else { corner } as u32);
        if given {
            polygon_uvs.push(uvs.get(k).copied().unwrap_or(Vec2::NAN));
        }
        if corner < 0 {
            close(&mut polygon, &mut polygon_uvs);
        }
    }
    if !polygon.is_empty() {
        close(&mut polygon, &mut polygon_uvs);
    }
    faces
}

fn byte(x: f64) -> u8 {
    (x * 255.0).round().clamp(0.0, 255.0) as u8
}

/// The colour of each point, from the geometry's first colour layer, its
/// alpha as how much paint covers the point; empty when the geometry has no
/// layer, or one of a kind not read, or one that says nothing (see the
/// module's notes). `corners` walks the polygons' corners, for a layer given
/// per corner.
fn colours_of<'b>(
    geometry: &Record,
    count: usize,
    corners: impl Fn() -> Result<Box<dyn Iterator<Item = i32> + 'b>>,
) -> Result<Vec<Rgba8>> {
    let layers = || geometry.children.iter().filter(|r| r.name == b"LayerElementColor");
    let Some(layer) = layers().find(|r| r.long(0) == Some(0)).or_else(|| layers().next()) else {
        return Ok(Vec::new());
    };
    let Some(table) = layer.array(b"Colors") else { return Ok(Vec::new()) };
    let table = table.doubles()?;
    let indexed = matches!(layer.child_text(b"ReferenceInformationType"), Some(b"IndexToDirect" | b"Index"));
    let index = match layer.array(b"ColorIndex") {
        Some(index) if indexed => index.ints()?,
        _ => Vec::new(),
    };
    // The colour of the `k`th entry the mapping counts.
    let pick = |k: usize| -> Option<Rgba8> {
        let at = if indexed { usize::try_from(*index.get(k)?).ok()? } else { k };
        let c = table.get(at.checked_mul(4)?..at.checked_mul(4)?.checked_add(4)?)?;
        Some(Rgba8([byte(c[0]), byte(c[1]), byte(c[2]), byte(c[3])]))
    };
    let mut colours: Vec<Option<Rgba8>> = vec![None; count];
    match layer.child_text(b"MappingInformationType") {
        Some(b"ByVertice" | b"ByVertex") => {
            for (v, colour) in colours.iter_mut().enumerate() {
                *colour = pick(v);
            }
        }
        Some(b"ByPolygonVertex") => {
            for (k, corner) in corners()?.enumerate() {
                let v = (if corner < 0 { !corner } else { corner }) as usize;
                if let Some(colour @ None) = colours.get_mut(v) {
                    *colour = pick(k);
                }
            }
        }
        Some(b"AllSame") => colours.fill(pick(0)),
        _ => return Ok(Vec::new()),
    }
    let colours: Vec<Rgba8> = colours.into_iter().map(|c| c.unwrap_or(UNPAINTED)).collect();
    if colours.iter().all(|c| c.0[3] == 0 || c.0[..3] == [255; 3]) {
        return Ok(Vec::new());
    }
    Ok(colours)
}
