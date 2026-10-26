//! glTF 2.0 import: a `.glb` (one binary file) or a `.gltf` (a JSON file with
//! its data in `.bin` files beside it, or packed into it as base64). This is
//! what Blender, Sketchfab, Unity and most web viewers write.
//!
//! The file is a scene of nodes. Each node may hold a mesh, and the nodes sit
//! inside each other, each placed relative to its parent. The default scene is
//! walked from its top nodes down, the placements multiplied together as it
//! goes, and every node that holds a mesh gives one object (two nodes using the
//! same mesh are two objects, as they are two things in the scene). The
//! placement is baked into the object's points, so the objects stand in the
//! scene as they did in the program that wrote them, and then the points are
//! scaled from metres, what glTF counts in, to the centimetres this app counts
//! in. Both use Y up and a right-handed space, so nothing else turns. A
//! placement that mirrors (a negative scale) turns the triangles inside out, so
//! their winding is flipped to keep them facing outward.
//!
//! All of a mesh's primitives (glTF splits a mesh by material) are joined into
//! its one object. Triangles, strips and fans are read; points and lines have
//! no surface and are left out. Beside the points, the first vertex colours
//! (`COLOR_0`) are read. glTF keeps them as linear light and the paint here is
//! sRGB, so they are converted. So are the first UVs (`TEXCOORD_0`, floats or
//! normalized bytes or shorts), whose V runs down the picture where the app's
//! (as OBJ's and FBX's) runs up, so each is read as (u, 1 - v); a primitive
//! without them has none on its corners (see `polygons.rs`). Textures,
//! materials, cameras, lights and animations are not read.
//!
//! A skinned mesh is read as stored, in its bind pose, under its node's own
//! placement: the bones are not applied. Morph targets are not applied either.
//!
//! Data is read straight from the file's bytes (honouring each view's offset
//! and stride) and the big arrays are filled in parallel, so a model of tens
//! of millions of triangles opens in a moment. Every offset and length is
//! checked against the file before it is used, so a damaged file gives a
//! message and never a crash. glTF splits a vertex wherever the texture or
//! normals have a seam; `polygons::raw_mesh` welds those back together.
//!
//! Compressed models (Draco, Meshopt) and sparse accessors are not read; the
//! error says so, and that the model should be exported again without them.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;

use glam::{Mat3, Mat4, Quat, Vec2, Vec3};
use rayon::prelude::*;
use serde::Deserialize;
use serde_json::Value;

use super::polygons::{Faces, raw_mesh};
use super::{RawMesh, Result};
use crate::mesh::paint::Rgba8;

/// glTF counts in metres, the app in centimetres.
const CM_PER_METRE: f32 = 100.0;
/// How deep nodes may sit in nodes before the file is taken to loop.
const MAX_DEPTH: usize = 512;
/// How many nodes a file may visit in all, so a file naming one node as the
/// child of many cannot multiply without end.
const MAX_VISITS: usize = 1_000_000;
/// Arrays at least this long are filled on all cores.
const PARALLEL_FROM: usize = 1 << 16;

const COMPRESSED: &str = "This model is compressed (Draco or Meshopt), which GolemSculpt cannot read. Export it again without compression and open that file.";
const SPARSE: &str = "This model stores some of its points in a sparse form that GolemSculpt cannot read. Export it again with sparse storage turned off and open that file.";

/// Reads a glTF file's objects. `path` is where the file is, for the data files
/// a `.gltf` names beside itself.
pub(super) fn parse(bytes: &[u8], path: &Path) -> Result<Vec<RawMesh>> {
    let (json, bin) = if bytes.starts_with(b"glTF") { split_glb(bytes)? } else { (bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes), None) };
    let mut root: Root = serde_json::from_slice(json).map_err(|e| format!("This file is not a readable glTF model ({e})"))?;
    if root.asset.version.starts_with('1') {
        return Err("This is a glTF version 1 file, which is not read. Export it again as glTF 2.0.".into());
    }
    let mut buffers = Buffers { glb_bin: bin, folder: path.parent().map(Path::to_path_buf), defs: std::mem::take(&mut root.buffers), slots: Vec::new() };
    buffers.slots.resize_with(buffers.defs.len(), || None);
    objects(&root, &mut buffers)
}

// ---- the file's description --------------------------------------------------------------------

#[derive(Deserialize, Default)]
#[serde(default)]
struct Root {
    asset: AssetDef,
    scene: Option<usize>,
    scenes: Vec<SceneDef>,
    nodes: Vec<NodeDef>,
    meshes: Vec<MeshDef>,
    accessors: Vec<AccessorDef>,
    #[serde(rename = "bufferViews")]
    buffer_views: Vec<ViewDef>,
    buffers: Vec<BufferDef>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct AssetDef {
    version: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SceneDef {
    nodes: Vec<usize>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct NodeDef {
    name: Option<String>,
    children: Vec<usize>,
    mesh: Option<usize>,
    matrix: Option<[f32; 16]>,
    translation: Option<[f32; 3]>,
    rotation: Option<[f32; 4]>,
    scale: Option<[f32; 3]>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct MeshDef {
    name: Option<String>,
    primitives: Vec<PrimitiveDef>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PrimitiveDef {
    attributes: HashMap<String, usize>,
    indices: Option<usize>,
    mode: Option<u32>,
    extensions: HashMap<String, Value>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct AccessorDef {
    #[serde(rename = "bufferView")]
    buffer_view: Option<usize>,
    #[serde(rename = "byteOffset")]
    byte_offset: usize,
    #[serde(rename = "componentType")]
    component_type: u32,
    normalized: bool,
    count: usize,
    #[serde(rename = "type")]
    kind: String,
    sparse: Option<Value>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ViewDef {
    buffer: usize,
    #[serde(rename = "byteOffset")]
    byte_offset: usize,
    #[serde(rename = "byteLength")]
    byte_length: usize,
    #[serde(rename = "byteStride")]
    byte_stride: Option<usize>,
    extensions: HashMap<String, Value>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct BufferDef {
    uri: Option<String>,
    #[serde(rename = "byteLength")]
    byte_length: usize,
}

// ---- the container and the data files ----------------------------------------------------------

/// A `.glb`'s JSON text and its binary chunk (the data `buffers[0]` without a
/// uri stands for), if it has one.
fn split_glb(bytes: &[u8]) -> Result<(&[u8], Option<&[u8]>)> {
    let cut = || "This glb file is cut short: it ends before its data does. It may not have finished copying or downloading.".to_string();
    let word = |at: usize| -> Result<usize> { Ok(u32::from_le_bytes(bytes.get(at..at + 4).ok_or_else(cut)?.try_into().map_err(|_| cut())?) as usize) };
    if bytes.len() < 12 {
        return Err(cut());
    }
    let version = word(4)?;
    if version != 2 {
        return Err(format!("This glb file is version {version}, and only glTF 2.0 is read."));
    }
    let total = word(8)?;
    if total > bytes.len() {
        return Err(cut());
    }
    let (mut json, mut bin) = (None, None);
    let mut at = 12;
    while at + 8 <= total {
        let length = word(at)?;
        let kind = word(at + 4)?;
        let start = at + 8;
        let end = start.checked_add(length).filter(|&e| e <= total).ok_or_else(cut)?;
        match kind {
            0x4E4F_534A if json.is_none() => json = Some(&bytes[start..end]),
            0x004E_4942 if bin.is_none() => bin = Some(&bytes[start..end]),
            _ => {}
        }
        // Chunks are padded to four bytes.
        at = end.saturating_add(3) & !3;
    }
    let json = json.ok_or("This glb file holds no model description, so it is damaged.")?;
    Ok((json, bin))
}

/// The files' binary data, each loaded the first time something needs it (so a
/// data file nothing uses may be missing).
struct Buffers<'a> {
    glb_bin: Option<&'a [u8]>,
    folder: Option<std::path::PathBuf>,
    defs: Vec<BufferDef>,
    slots: Vec<Option<Cow<'a, [u8]>>>,
}

impl<'a> Buffers<'a> {
    fn load(&mut self, i: usize) -> Result<()> {
        if self.slots.get(i).ok_or("The file is damaged: it uses data it does not hold.")?.is_some() {
            return Ok(());
        }
        let def = &self.defs[i];
        let data: Cow<'a, [u8]> = match def.uri.as_deref() {
            None => match (i, self.glb_bin) {
                (0, Some(bin)) => Cow::Borrowed(bin),
                _ => return Err("The file is damaged: it has no data where its description says there is some.".into()),
            },
            Some(uri) if uri.starts_with("data:") => Cow::Owned(data_uri(uri)?),
            Some(uri) => Cow::Owned(self.external(uri)?),
        };
        if data.len() < def.byte_length {
            return Err("The file is cut short: its data ends before the length it says it has.".into());
        }
        self.slots[i] = Some(data);
        Ok(())
    }

    /// A data file beside the `.gltf`.
    fn external(&self, uri: &str) -> Result<Vec<u8>> {
        if uri.contains("://") {
            return Err("This model keeps its data at a web address. Save it with the data inside the file (a .glb) and open that.".into());
        }
        let name = percent_decoded(uri);
        let path = match &self.folder {
            Some(folder) => folder.join(&name),
            None => name.clone().into(),
        };
        std::fs::read(&path).map_err(|e| format!("This model's data file \"{name}\" could not be read from beside it: {e}"))
    }
}

/// The bytes of a `data:...;base64,...` address.
fn data_uri(uri: &str) -> Result<Vec<u8>> {
    let (head, body) = uri.split_once(',').ok_or("The file is damaged: a packed-in data block is not complete.")?;
    if !head.ends_with(";base64") {
        return Err("The file packs its data in a form that is not read (only base64).".into());
    }
    base64_decoded(body)
}

/// Plain base64 (and the web-safe alphabet), ignoring line breaks and stopping
/// at the padding.
fn base64_decoded(text: &str) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3 + 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for b in text.bytes() {
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            b' ' | b'\r' | b'\n' | b'\t' => continue,
            _ => return Err("The file is damaged: its packed-in data has characters that do not belong.".into()),
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

/// A file address with its `%20`s and the like turned back into characters.
fn percent_decoded(uri: &str) -> String {
    let bytes = uri.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |k: usize| bytes.get(k).and_then(|&b| (b as char).to_digit(16));
        match (bytes[i], hex(i + 1), hex(i + 2)) {
            (b'%', Some(h), Some(l)) => {
                out.push((h * 16 + l) as u8);
                i += 3;
            }
            (b, _, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---- reading an accessor -----------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Component {
    I8,
    U8,
    I16,
    U16,
    U32,
    F32,
}

impl Component {
    fn from_code(code: u32) -> Option<Component> {
        Some(match code {
            5120 => Component::I8,
            5121 => Component::U8,
            5122 => Component::I16,
            5123 => Component::U16,
            5125 => Component::U32,
            5126 => Component::F32,
            _ => return None,
        })
    }

    fn size(self) -> usize {
        match self {
            Component::I8 | Component::U8 => 1,
            Component::I16 | Component::U16 => 2,
            Component::U32 | Component::F32 => 4,
        }
    }
}

/// An accessor's elements, laid out in the file's bytes: element `n` begins
/// `n * stride` bytes into `data`, which ends exactly where the last element
/// does, so every element read is inside it.
struct Source<'a> {
    data: &'a [u8],
    count: usize,
    stride: usize,
    component: Component,
    normalized: bool,
    columns: usize,
}

impl Source<'_> {
    /// One number of an element at byte `at`, as a float (an integer marked
    /// normalized is mapped onto 0 to 1, or -1 to 1 when signed).
    fn number(&self, at: usize) -> f32 {
        let d = self.data;
        match self.component {
            Component::F32 => f32::from_le_bytes([d[at], d[at + 1], d[at + 2], d[at + 3]]),
            Component::U8 => d[at] as f32 / if self.normalized { 255.0 } else { 1.0 },
            Component::I8 => {
                let v = d[at] as i8 as f32;
                if self.normalized { (v / 127.0).max(-1.0) } else { v }
            }
            Component::U16 => u16::from_le_bytes([d[at], d[at + 1]]) as f32 / if self.normalized { 65535.0 } else { 1.0 },
            Component::I16 => {
                let v = i16::from_le_bytes([d[at], d[at + 1]]) as f32;
                if self.normalized { (v / 32767.0).max(-1.0) } else { v }
            }
            Component::U32 => u32::from_le_bytes([d[at], d[at + 1], d[at + 2], d[at + 3]]) as f32,
        }
    }

    fn vec3(&self, n: usize) -> Vec3 {
        let (at, size) = (n * self.stride, self.component.size());
        Vec3::new(self.number(at), self.number(at + size), self.number(at + 2 * size))
    }

    /// Element `n` of an index accessor.
    fn index(&self, n: usize) -> u32 {
        let (at, d) = (n * self.stride, self.data);
        match self.component {
            Component::U8 => d[at] as u32,
            Component::U16 => u16::from_le_bytes([d[at], d[at + 1]]) as u32,
            _ => u32::from_le_bytes([d[at], d[at + 1], d[at + 2], d[at + 3]]),
        }
    }

    /// Element `n` of a UV accessor, turned over to V up.
    fn uv(&self, n: usize) -> Vec2 {
        let (at, size) = (n * self.stride, self.component.size());
        Vec2::new(self.number(at), 1.0 - self.number(at + size))
    }

    /// Element `n` of a colour accessor, as sRGB bytes.
    fn colour(&self, n: usize) -> Rgba8 {
        let (at, size) = (n * self.stride, self.component.size());
        let alpha = if self.columns == 4 { self.number(at + 3 * size).clamp(0.0, 1.0) } else { 1.0 };
        Rgba8([srgb_byte(self.number(at)), srgb_byte(self.number(at + size)), srgb_byte(self.number(at + 2 * size)), (alpha * 255.0 + 0.5) as u8])
    }
}

/// Linear light 0 to 1 (what glTF keeps colours in) as an sRGB byte.
fn srgb_byte(linear: f32) -> u8 {
    let c = linear.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0 + 0.5) as u8
}

const DAMAGED_DATA: &str = "The file is damaged: its description points at data that is not there.";

/// The data file an accessor reads from, checked and loaded.
fn prepare(root: &Root, buffers: &mut Buffers, accessor: usize) -> Result<()> {
    let a = root.accessors.get(accessor).ok_or(DAMAGED_DATA)?;
    if a.sparse.is_some() {
        return Err(SPARSE.into());
    }
    let view = root.buffer_views.get(a.buffer_view.ok_or(DAMAGED_DATA)?).ok_or(DAMAGED_DATA)?;
    if view.extensions.keys().any(|k| k.contains("meshopt") || k.contains("draco")) {
        return Err(COMPRESSED.into());
    }
    buffers.load(view.buffer)
}

/// An accessor's bytes, once [`prepare`]d, with every offset and length
/// checked against the view and the file.
fn source<'a>(root: &Root, buffers: &'a Buffers, accessor: usize) -> Result<Source<'a>> {
    let a = root.accessors.get(accessor).ok_or(DAMAGED_DATA)?;
    let view = root.buffer_views.get(a.buffer_view.ok_or(DAMAGED_DATA)?).ok_or(DAMAGED_DATA)?;
    let buffer: &[u8] = buffers.slots.get(view.buffer).and_then(|s| s.as_deref()).ok_or(DAMAGED_DATA)?;
    let component = Component::from_code(a.component_type).ok_or("The file is damaged: it stores numbers in a form that is not known.")?;
    let columns = match a.kind.as_str() {
        "SCALAR" => 1,
        "VEC2" => 2,
        "VEC3" => 3,
        "VEC4" => 4,
        _ => return Err("The file stores its points or triangles in a shape that is not expected.".into()),
    };
    let width = component.size() * columns;
    let stride = view.byte_stride.unwrap_or(width);
    if stride < width {
        return Err(DAMAGED_DATA.into());
    }
    let view_end = view.byte_offset.checked_add(view.byte_length).filter(|&e| e <= buffer.len()).ok_or(DAMAGED_DATA)?;
    let start = view.byte_offset.checked_add(a.byte_offset).ok_or(DAMAGED_DATA)?;
    let data = if a.count == 0 {
        &buffer[..0]
    } else {
        let span = (a.count - 1).checked_mul(stride).and_then(|s| s.checked_add(width)).ok_or(DAMAGED_DATA)?;
        let end = start.checked_add(span).filter(|&e| e <= view_end).ok_or(DAMAGED_DATA)?;
        &buffer[start..end]
    };
    Ok(Source { data, count: a.count, stride, component, normalized: a.normalized, columns })
}

// ---- walking the scene -------------------------------------------------------------------------

/// One object for each node that holds a mesh, in the order the default scene
/// walks them.
fn objects(root: &Root, buffers: &mut Buffers) -> Result<Vec<RawMesh>> {
    let mut out = Vec::new();
    if root.nodes.is_empty() {
        // No scene at all: each mesh stands as it is.
        for mesh in &root.meshes {
            out.push(object(root, buffers, mesh.name.clone().unwrap_or_default(), mesh, Mat4::IDENTITY)?);
        }
        return Ok(out);
    }
    let mut stack: Vec<(usize, Mat4, usize)> = top_nodes(root)?.into_iter().rev().map(|n| (n, Mat4::IDENTITY, 0)).collect();
    let mut visits = 0;
    while let Some((n, parent, depth)) = stack.pop() {
        visits += 1;
        if depth > MAX_DEPTH || visits > MAX_VISITS {
            return Err("The file is damaged: its objects are placed inside one another without end.".into());
        }
        let node = root.nodes.get(n).ok_or("The file is damaged: a scene names an object that is not there.")?;
        let world = parent * local_matrix(node);
        stack.extend(node.children.iter().rev().map(|&c| (c, world, depth + 1)));
        if let Some(m) = node.mesh {
            let mesh = root.meshes.get(m).ok_or("The file is damaged: an object names a mesh that is not there.")?;
            let name = node.name.clone().filter(|n| !n.is_empty()).or_else(|| mesh.name.clone()).unwrap_or_default();
            out.push(object(root, buffers, name, mesh, world)?);
        }
    }
    Ok(out)
}

/// The nodes the walk starts from: the default scene's, else the first
/// scene's, else every node nothing else holds.
fn top_nodes(root: &Root) -> Result<Vec<usize>> {
    if let Some(s) = root.scene.or(if root.scenes.is_empty() { None } else { Some(0) }) {
        return Ok(root.scenes.get(s).ok_or("The file is damaged: its main scene is not there.")?.nodes.clone());
    }
    let mut held = vec![false; root.nodes.len()];
    for &c in root.nodes.iter().flat_map(|n| &n.children) {
        if let Some(h) = held.get_mut(c) {
            *h = true;
        }
    }
    Ok((0..root.nodes.len()).filter(|&n| !held[n]).collect())
}

/// A node's placement in its parent.
fn local_matrix(node: &NodeDef) -> Mat4 {
    if let Some(m) = node.matrix {
        return Mat4::from_cols_array(&m);
    }
    let t = node.translation.map_or(Vec3::ZERO, Vec3::from);
    let s = node.scale.map_or(Vec3::ONE, Vec3::from);
    let q = node.rotation.map_or(Quat::IDENTITY, Quat::from_array);
    let q = if q.length_squared() > 1e-12 && q.is_finite() { q.normalize() } else { Quat::IDENTITY };
    Mat4::from_scale_rotation_translation(s, q, t)
}

// ---- building an object ------------------------------------------------------------------------

/// Fills `out` with `f(0)`, `f(1)`, ..., on all cores when it is long.
fn fill<T: Send>(out: &mut [T], f: impl Fn(usize) -> T + Sync) {
    if out.len() >= PARALLEL_FROM {
        out.par_iter_mut().enumerate().with_min_len(4096).for_each(|(i, o)| *o = f(i));
    } else {
        out.iter_mut().enumerate().for_each(|(i, o)| *o = f(i));
    }
}

/// One mesh's primitives as one object, placed by `world` and scaled to
/// centimetres.
fn object(root: &Root, buffers: &mut Buffers, name: String, mesh: &MeshDef, world: Mat4) -> Result<RawMesh> {
    let place = Mat4::from_scale(Vec3::splat(CM_PER_METRE)) * world;
    // A mirroring placement turns every triangle inside out.
    let flip = Mat3::from_mat4(world).determinant() < 0.0;
    let mut positions: Vec<Vec3> = Vec::new();
    let mut colours: Vec<Rgba8> = Vec::new();
    let mut any_colour = false;
    // Each point's UV, once some primitive has given UVs; none (NaN) on the points of those that have not.
    let mut uvs: Vec<Vec2> = Vec::new();
    let mut corners: Vec<u32> = Vec::new();

    for primitive in &mesh.primitives {
        let mode = primitive.mode.unwrap_or(4);
        // Points and lines have no surface.
        if !(4..=6).contains(&mode) {
            continue;
        }
        if primitive.extensions.keys().any(|k| k.contains("draco") || k.contains("meshopt")) {
            return Err(COMPRESSED.into());
        }
        let Some(&at_points) = primitive.attributes.get("POSITION") else { continue };
        let at_colours = primitive.attributes.get("COLOR_0").copied();
        prepare(root, buffers, at_points)?;
        if let Some(i) = primitive.indices {
            prepare(root, buffers, i)?;
        }
        if let Some(c) = at_colours {
            prepare(root, buffers, c)?;
        }
        // UVs that cannot be read (sparse, compressed, their data missing) are
        // passed over, as UVs of a form not expected are below: the model is
        // whole without them, and opened before they were read at all.
        let at_uvs = primitive.attributes.get("TEXCOORD_0").copied().filter(|&t| prepare(root, buffers, t).is_ok());

        let points = source(root, buffers, at_points)?;
        if points.columns != 3 || points.component == Component::U32 {
            return Err("The file stores its points in a form that is not expected.".into());
        }
        let n = points.count;
        let base = positions.len();
        if base + n >= u32::MAX as usize {
            return Err("This model has more points than can be opened.".into());
        }
        positions.resize(base + n, Vec3::ZERO);
        fill(&mut positions[base..], |i| place.transform_point3(points.vec3(i)));

        if let Some(c) = at_colours {
            let c = source(root, buffers, c)?;
            let fine = (c.columns == 3 || c.columns == 4) && matches!(c.component, Component::F32 | Component::U8 | Component::U16);
            if !fine || c.count < n {
                return Err("The file's vertex colours do not match its points.".into());
            }
            let c = Source { normalized: true, ..c };
            if !any_colour {
                any_colour = true;
                colours.resize(base, Rgba8([255; 4]));
            }
            colours.resize(base + n, Rgba8([255; 4]));
            fill(&mut colours[base..], |i| c.colour(i));
        } else if any_colour {
            colours.resize(base + n, Rgba8([255; 4]));
        }

        let texcoords = at_uvs.and_then(|t| source(root, buffers, t).ok()).filter(|t| {
            t.columns == 2 && t.count >= n && matches!(t.component, Component::F32 | Component::U8 | Component::U16)
        });
        if let Some(t) = texcoords {
            // Bytes and shorts are always normalized, as the format has it.
            let t = Source { normalized: true, ..t };
            uvs.resize(base + n, Vec2::NAN);
            fill(&mut uvs[base..], |i| t.uv(i));
        }

        let listed = match primitive.indices {
            Some(i) => {
                let s = source(root, buffers, i)?;
                if s.columns != 1 || !matches!(s.component, Component::U8 | Component::U16 | Component::U32) {
                    return Err("The file stores its triangles in a form that is not expected.".into());
                }
                let mut list = vec![0u32; s.count];
                fill(&mut list, |k| s.index(k));
                list
            }
            None => (0..n as u32).collect(),
        };
        let out_of_range = |list: &[u32]| if list.len() >= PARALLEL_FROM { list.par_iter().any(|&i| i as usize >= n) } else { list.iter().any(|&i| i as usize >= n) };
        if out_of_range(&listed) {
            return Err("The file is damaged: a triangle uses a point that is not there.".into());
        }
        let mut triangles = triangles_of(mode, listed);
        if corners.len() + triangles.len() >= u32::MAX as usize {
            return Err("This model has more triangles than can be opened.".into());
        }
        let base = base as u32;
        let shift = |t: &mut [u32]| {
            t.iter_mut().for_each(|c| *c += base);
            if flip {
                t.swap(1, 2);
            }
        };
        if triangles.len() >= PARALLEL_FROM {
            triangles.par_chunks_mut(3).for_each(shift);
        } else {
            triangles.chunks_mut(3).for_each(shift);
        }
        if corners.is_empty() {
            corners = triangles;
        } else {
            corners.extend_from_slice(&triangles);
        }
    }

    let mut faces = Faces::default();
    if !corners.is_empty() {
        faces.start = (0..=corners.len() as u32 / 3).map(|t| t * 3).collect();
        if !uvs.is_empty() {
            // Each corner the UV of its point; a point past the last that had one has none.
            uvs.resize(positions.len(), Vec2::NAN);
            faces.uvs = corners.par_iter().with_min_len(4096).map(|&c| uvs[c as usize]).collect();
        }
        faces.corners = corners;
    }
    Ok(raw_mesh(name, positions, &faces, true, if any_colour { colours } else { Vec::new() }))
}

/// A primitive's listed corners as separate triangles (a multiple of three
/// corners), whether they were triangles, a strip or a fan. A strip's every
/// second triangle is turned about so that all face the same way.
fn triangles_of(mode: u32, mut listed: Vec<u32>) -> Vec<u32> {
    match mode {
        5 => {
            let mut out = Vec::with_capacity(listed.len().saturating_sub(2) * 3);
            for (i, w) in listed.windows(3).enumerate() {
                if i % 2 == 0 {
                    out.extend_from_slice(&[w[0], w[1], w[2]]);
                } else {
                    out.extend_from_slice(&[w[1], w[0], w[2]]);
                }
            }
            out
        }
        6 => {
            let mut out = Vec::with_capacity(listed.len().saturating_sub(2) * 3);
            if let Some((&hub, rest)) = listed.split_first() {
                for w in rest.windows(2) {
                    out.extend_from_slice(&[hub, w[0], w[1]]);
                }
            }
            out
        }
        _ => {
            listed.truncate(listed.len() / 3 * 3);
            listed
        }
    }
}

#[cfg(test)]
mod tests;
