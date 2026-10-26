//! Export as Autodesk FBX, for Blender, Unity and the other programs that
//! take one.
//!
//! The file is binary FBX 7.4, the form every one of them reads (Blender reads
//! no text FBX at all). FBX has no public specification; the layout here is
//! the one Blender's exporter writes, which Autodesk's own reader accepts,
//! down to the fixed file id and closing bytes that reader checks.
//!
//! A binary FBX is a tree of named records. Each begins with the offset in the
//! file at which it ends, so the whole size is worked out before a byte is
//! written (`Node::len`), and the big arrays are then written straight from
//! the mesh rather than gathered in memory first (`Fill`).
//!
//! Axes and units: FBX says which way is up and how long a unit is, and the
//! reader converts. The app is right-handed with Y up, which is FBX's own
//! default, so points go out as they are; one app unit is 10 mm, which is the
//! centimetre FBX counts in, so the model arrives at its real size.
//!
//! As the OBJ does, an export bakes each place's position, rotation and scale
//! into its points, one mesh for the object and one for each live copy, and
//! keeps a quad-remeshed mesh's quads. A painted mesh also takes its colour
//! and its materials, as `fbx/paint.rs` sets out, and a mesh with UVs its
//! UVs, as `fbx/uvs.rs` does.

use std::io::Write;
use std::path::Path;

use glam::{Mat3, Mat4, Vec3};
use sculpt_geom::units;

use super::painted::{ExportPaint, Painted};
use super::{Points, Result, export_points, nothing_to_export, replace_file, time_stamp};
use crate::mesh::Mesh;

mod paint;
mod uvs;

const VERSION: u32 = 7400;
const MAGIC: &[u8; 23] = b"Kaydara FBX Binary  \x00\x1a\x00";
/// What ends a record's list of children: an empty record.
const SENTINEL: [u8; 13] = [0; 13];
/// The id, time and closing bytes go together: Autodesk's reader checks the
/// last against the first two, by a rule nobody outside has worked out. These
/// are the three Blender writes into every file.
const FILE_ID: &[u8; 16] = b"\x28\xb3\x2a\xeb\xb6\x24\xcc\xc2\xbf\xc8\xb0\x2a\xa9\x2b\xfc\xf1";
const FILE_TIME: &str = "1970-01-01 10:00:00:000";
const FOOT_ID: &[u8; 16] = b"\xfa\xbc\xab\x09\xd0\xc8\xd4\x66\xb1\x76\xfb\x83\x1c\xf7\x26\x7e";
const FOOT_END: &[u8; 16] = b"\xf8\x5a\x8c\x6a\xde\xf5\xd9\x7e\xec\xe9\x0c\xe3\x75\x8f\x29\x0b";
const CREATOR: &str = "GolemSculpt";

/// Writes the numbers of one array, which `Value` has already counted.
type Fill<'a> = Box<dyn Fn(&mut dyn Write) -> std::io::Result<()> + 'a>;

enum Value<'a> {
    Int(i32),
    Long(i64),
    Double(f64),
    Text(Vec<u8>),
    Raw(&'static [u8]),
    Ints(usize, Fill<'a>),
    Doubles(usize, Fill<'a>),
}

impl Value<'_> {
    /// Bytes in the file, the letter that says its kind included.
    fn len(&self) -> u64 {
        match self {
            Value::Int(_) => 5,
            Value::Long(_) | Value::Double(_) => 9,
            Value::Text(bytes) => 5 + bytes.len() as u64,
            Value::Raw(bytes) => 5 + bytes.len() as u64,
            Value::Ints(count, _) => 13 + 4 * *count as u64,
            Value::Doubles(count, _) => 13 + 8 * *count as u64,
        }
    }

    fn write(&self, w: &mut dyn Write) -> std::io::Result<()> {
        let lead = |w: &mut dyn Write, kind: u8, length: usize| {
            w.write_all(&[kind])?;
            w.write_all(&(length as u32).to_le_bytes())
        };
        // An array says how many numbers, that they are not compressed, and how many bytes.
        let array = |w: &mut dyn Write, kind: u8, count: usize, width: usize, fill: &Fill| {
            lead(w, kind, count)?;
            w.write_all(&0u32.to_le_bytes())?;
            w.write_all(&((count * width) as u32).to_le_bytes())?;
            fill(w)
        };
        match self {
            Value::Int(v) => {
                w.write_all(b"I")?;
                w.write_all(&v.to_le_bytes())
            }
            Value::Long(v) => {
                w.write_all(b"L")?;
                w.write_all(&v.to_le_bytes())
            }
            Value::Double(v) => {
                w.write_all(b"D")?;
                w.write_all(&v.to_le_bytes())
            }
            Value::Text(bytes) => {
                lead(w, b'S', bytes.len())?;
                w.write_all(bytes)
            }
            Value::Raw(bytes) => {
                lead(w, b'R', bytes.len())?;
                w.write_all(bytes)
            }
            Value::Ints(count, fill) => array(w, b'i', *count, 4, fill),
            Value::Doubles(count, fill) => array(w, b'd', *count, 8, fill),
        }
    }
}

struct Node<'a> {
    name: &'static str,
    values: Vec<Value<'a>>,
    children: Vec<Node<'a>>,
}

impl Node<'_> {
    fn values_len(&self) -> u64 {
        self.values.iter().map(Value::len).sum()
    }

    /// Bytes in the file. A record with children closes them with a sentinel;
    /// one with nothing in it at all has a sentinel too, unless it is the last
    /// of its family (which is how Autodesk's files are, so it is kept).
    fn len(&self, last: bool) -> u64 {
        13 + self.name.len() as u64 + self.values_len() + self.tail_len(last)
    }

    fn tail_len(&self, last: bool) -> u64 {
        if self.children.is_empty() {
            return if self.values.is_empty() && !last { SENTINEL.len() as u64 } else { 0 };
        }
        children_len(&self.children)
    }

    /// Writes the record, which begins at `start` in the file, and returns where it ends.
    fn write(&self, w: &mut dyn Write, start: u64, last: bool) -> std::io::Result<u64> {
        let end = start + self.len(last);
        w.write_all(&(end as u32).to_le_bytes())?;
        w.write_all(&(self.values.len() as u32).to_le_bytes())?;
        w.write_all(&(self.values_len() as u32).to_le_bytes())?;
        w.write_all(&[self.name.len() as u8])?;
        w.write_all(self.name.as_bytes())?;
        for value in &self.values {
            value.write(w)?;
        }
        if self.children.is_empty() {
            if self.values.is_empty() && !last {
                w.write_all(&SENTINEL)?;
            }
        } else {
            write_children(w, &self.children, end - children_len(&self.children))?;
        }
        Ok(end)
    }
}

fn children_len(children: &[Node]) -> u64 {
    let last = children.len() - 1;
    children.iter().enumerate().map(|(i, child)| child.len(i == last)).sum::<u64>() + SENTINEL.len() as u64
}

fn write_children(w: &mut dyn Write, children: &[Node], mut at: u64) -> std::io::Result<()> {
    let last = children.len() - 1;
    for (i, child) in children.iter().enumerate() {
        at = child.write(w, at, i == last)?;
    }
    w.write_all(&SENTINEL)
}

fn node<'a>(name: &'static str, values: Vec<Value<'a>>, children: Vec<Node<'a>>) -> Node<'a> {
    Node { name, values, children }
}

fn text<'a>(text: &str) -> Value<'a> {
    Value::Text(text.as_bytes().to_vec())
}

/// A record holding one value and nothing else.
fn leaf<'a>(name: &'static str, value: Value<'a>) -> Node<'a> {
    node(name, vec![value], Vec::new())
}

/// An object's name as FBX keeps it: the name, then what kind of thing it is.
fn named<'a>(name: &str, class: &str) -> Value<'a> {
    let mut bytes = name.as_bytes().to_vec();
    bytes.extend_from_slice(b"\x00\x01");
    bytes.extend_from_slice(class.as_bytes());
    Value::Text(bytes)
}

/// One line of a `Properties70` list: the property's name, its kind twice
/// over (FBX's own and the interface's), its flags, then its values.
fn property<'a>(name: &str, kind: [&str; 2], flags: &str, values: Vec<Value<'a>>) -> Node<'a> {
    let mut all = vec![text(name), text(kind[0]), text(kind[1]), text(flags)];
    all.extend(values);
    node("P", all, Vec::new())
}

fn whole<'a>(name: &str, value: i32) -> Node<'a> {
    property(name, ["int", "Integer"], "", vec![Value::Int(value)])
}

fn vector<'a>(name: &str, v: [f64; 3]) -> Node<'a> {
    property(name, [name, ""], "A", v.map(Value::Double).into())
}

/// Autodesk FBX of one object as it sits in the scene: placed by `model`, as
/// [`super::export_obj`] places it.
#[allow(dead_code)] // the app writes placed copies (`export_fbx_placed`); this is the one-place form
pub fn export_fbx(mesh: &Mesh, model: Mat4, path: &Path) -> Result<usize> {
    export_fbx_placed(mesh, &[("", model)], path)
}

/// [`export_fbx`] for a mesh drawn in several places at once (an object and
/// its live copies): an object of its own in the file for each, under the
/// name it is given, so what reaches the other program is what is on screen.
///
/// Returns how many points of the mesh were not numbers and were written
/// mended, as [`super::export_obj_placed`] does.
#[allow(dead_code)] // the app writes every selected object (`export_fbx_objects`); this is the one-mesh form
pub fn export_fbx_placed(mesh: &Mesh, places: &[(&str, Mat4)], path: &Path) -> Result<usize> {
    nothing_to_export(mesh)?;
    export_fbx_objects(&[(mesh, places)], path)
}

/// [`export_fbx_placed`] for several objects at once, all in the one file:
/// each mesh with the places it is drawn. They arrive in the other program
/// together, each an object of its own under its own name, standing to one
/// another as they do in the scene.
///
/// A mesh with no surface is left out; with none that has one, nothing is
/// written. Returns how many points were written mended, over all of them.
#[allow(dead_code)] // the app writes with the scene's paint settings (`export_fbx_painted`)
pub fn export_fbx_objects(objects: &[(&Mesh, &[(&str, Mat4)])], path: &Path) -> Result<usize> {
    export_fbx_painted(objects, &ExportPaint::default(), path)
}

/// [`export_fbx_objects`] with the scene's paint settings: a painted mesh
/// goes with its colour and materials (see `fbx/paint.rs`).
pub fn export_fbx_painted(objects: &[(&Mesh, &[(&str, Mat4)])], paint: &ExportPaint, path: &Path) -> Result<usize> {
    let solid: Vec<&(&Mesh, &[(&str, Mat4)])> = objects.iter().filter(|(mesh, _)| !mesh.indices.is_empty()).collect();
    if solid.is_empty() {
        return Err("There is nothing to export: no selected object has a surface.".into());
    }
    let points: Vec<(Points, usize)> = solid.iter().map(|(mesh, _)| export_points(mesh)).collect();
    let mended = points.iter().map(|(_, mended)| mended).sum();
    let painted: Vec<Option<Painted>> = solid.iter().map(|(mesh, _)| Painted::of(mesh, paint)).collect();
    let shading: Vec<Option<paint::Shading>> = solid
        .iter()
        .zip(&painted)
        .map(|((mesh, _), painted)| painted.as_ref().map(|p| paint::Shading::of(mesh, p, paint)))
        .collect();
    let pieces: Vec<Piece> = solid
        .iter()
        .zip(&points)
        .zip(&shading)
        .map(|((&&(mesh, places), (points, _)), shading)| Piece { mesh, points, places, shading: shading.as_ref() })
        .collect();
    let top = document(&pieces, paint);
    // Every offset in the file is four bytes.
    let nodes_end = MAGIC.len() as u64 + 4 + children_len(&top);
    if nodes_end > u32::MAX as u64 {
        return Err("That is more than an FBX file holds (4 GB). Lower the array's count, or the object's detail.".into());
    }
    replace_file(path, |w| {
        w.write_all(MAGIC)?;
        w.write_all(&VERSION.to_le_bytes())?;
        write_children(w, &top, MAGIC.len() as u64 + 4)?;
        // The close: its id, then zeros up to a multiple of sixteen bytes (a
        // whole sixteen when already there), the version again, and the end.
        w.write_all(FOOT_ID)?;
        w.write_all(&[0; 4])?;
        let at = nodes_end + FOOT_ID.len() as u64 + 4;
        let pad = 16 - (at % 16) as usize;
        w.write_all(&[0u8; 16][..pad])?;
        w.write_all(&VERSION.to_le_bytes())?;
        w.write_all(&[0; 120])?;
        w.write_all(FOOT_END)
    })?;
    Ok(mended)
}

/// One mesh of an export: its points as they are written, and its places.
struct Piece<'a> {
    mesh: &'a Mesh,
    points: &'a Points<'a>,
    places: &'a [(&'a str, Mat4)],
    /// Its paint, if it has any.
    shading: Option<&'a paint::Shading<'a>>,
}

/// The records at the top of the file, in the order FBX has them.
fn document<'a>(pieces: &[Piece<'a>], paint: &ExportPaint) -> Vec<Node<'a>> {
    let count = pieces.iter().map(|piece| piece.places.len()).sum::<usize>() as i32;
    // Ids only have to differ from one another, and from 0, which is the scene itself.
    let geometry_id = |place: usize| 1_000 + 2 * place as i64;
    let model_id = |place: usize| 1_001 + 2 * place as i64;
    // Above every geometry and model id: no file has half a billion places.
    let material_id = |material: usize| 1_000_000_000 + material as i64;

    let mut objects = Vec::new();
    let mut links = Vec::new();
    // Every material of the file, once, whichever models share it.
    let mut materials: Vec<paint::Key> = Vec::new();
    let link = |child: i64, parent: i64| node("C", vec![text("OO"), Value::Long(child), Value::Long(parent)], Vec::new());
    // Every place of every mesh, numbered through the file.
    let places = pieces.iter().flat_map(|piece| piece.places.iter().map(move |place| (piece, place)));
    for (i, (piece, &(name, model))) in places.enumerate() {
        let name = if name.is_empty() { "Object" } else { name };
        objects.push(geometry(geometry_id(i), name, piece, model, paint.weights_in_uvs));
        objects.push(node("Model", vec![Value::Long(model_id(i)), named(name, "Model"), text("Mesh")], vec![
            leaf("Version", Value::Int(232)),
            node("Properties70", Vec::new(), vec![
                vector("Lcl Translation", [0.0; 3]),
                vector("Lcl Rotation", [0.0; 3]),
                vector("Lcl Scaling", [1.0; 3]),
            ]),
            leaf("Culling", text("CullingOff")),
        ]));
        links.push(link(model_id(i), 0));
        links.push(link(geometry_id(i), model_id(i)));
        // In the order the material layer counts them.
        for &key in piece.shading.map_or(&[][..], |s| s.keys.as_slice()) {
            let at = materials.iter().position(|&k| k == key).unwrap_or_else(|| {
                materials.push(key);
                materials.len() - 1
            });
            links.push(link(material_id(at), model_id(i)));
        }
    }
    let material_count = materials.len() as i32;
    for (at, &key) in materials.iter().enumerate() {
        objects.push(paint::material(material_id(at), key, paint));
    }
    let mut kinds = vec![kind_of("GlobalSettings", 1), kind_of("Geometry", count), kind_of("Model", count)];
    if material_count > 0 {
        kinds.push(kind_of("Material", material_count));
    }

    let centimetres = f64::from(units::MILLIMETRES_PER_UNIT) / 10.0;
    vec![
        node("FBXHeaderExtension", Vec::new(), vec![
            leaf("FBXHeaderVersion", Value::Int(1003)),
            leaf("FBXVersion", Value::Int(VERSION as i32)),
            leaf("EncryptionType", Value::Int(0)),
            created(),
            leaf("Creator", text(CREATOR)),
        ]),
        leaf("FileId", Value::Raw(FILE_ID)),
        leaf("CreationTime", text(FILE_TIME)),
        leaf("Creator", text(CREATOR)),
        node("GlobalSettings", Vec::new(), vec![
            leaf("Version", Value::Int(1000)),
            node("Properties70", Vec::new(), vec![
                // Y up, Z toward the viewer, X to the right: the app's own axes.
                whole("UpAxis", 1),
                whole("UpAxisSign", 1),
                whole("FrontAxis", 2),
                whole("FrontAxisSign", 1),
                whole("CoordAxis", 0),
                whole("CoordAxisSign", 1),
                whole("OriginalUpAxis", -1),
                whole("OriginalUpAxisSign", 1),
                // How many centimetres one unit of the file is.
                property("UnitScaleFactor", ["double", "Number"], "", vec![Value::Double(centimetres)]),
                property("OriginalUnitScaleFactor", ["double", "Number"], "", vec![Value::Double(centimetres)]),
            ]),
        ]),
        node("Documents", Vec::new(), vec![
            leaf("Count", Value::Int(1)),
            node("Document", vec![Value::Long(999), text("Scene"), text("Scene")], vec![
                node("Properties70", Vec::new(), vec![
                    property("SourceObject", ["object", ""], "", Vec::new()),
                    property("ActiveAnimStackName", ["KString", ""], "", vec![text("")]),
                ]),
                leaf("RootNode", Value::Long(0)),
            ]),
        ]),
        node("References", Vec::new(), Vec::new()),
        node(
            "Definitions",
            Vec::new(),
            [leaf("Version", Value::Int(100)), leaf("Count", Value::Int(1 + 2 * count + material_count))]
                .into_iter()
                .chain(kinds)
                .collect(),
        ),
        node("Objects", Vec::new(), objects),
        node("Connections", Vec::new(), links),
        node("Takes", Vec::new(), vec![leaf("Current", text(""))]),
    ]
}

/// One kind of object of the file, and how many of it there are, as `Definitions` lists them.
fn kind_of<'a>(name: &str, count: i32) -> Node<'a> {
    node("ObjectType", vec![text(name)], vec![leaf("Count", Value::Int(count))])
}

/// When the file was made, as the header keeps it: each part a number of its own.
fn created<'a>() -> Node<'a> {
    // `time_stamp` is year-month-day_hourminutesecond: fourteen digits in all.
    let digits: Vec<i32> = time_stamp().bytes().filter(u8::is_ascii_digit).map(|d| i32::from(d - b'0')).collect();
    let part = |from: usize, to: usize| digits.get(from..to).map_or(0, |d| d.iter().fold(0, |sum, d| sum * 10 + d));
    node("CreationTimeStamp", Vec::new(), vec![
        leaf("Version", Value::Int(1000)),
        leaf("Year", Value::Int(part(0, 4))),
        leaf("Month", Value::Int(part(4, 6))),
        leaf("Day", Value::Int(part(6, 8))),
        leaf("Hour", Value::Int(part(8, 10))),
        leaf("Minute", Value::Int(part(10, 12))),
        leaf("Second", Value::Int(part(12, 14))),
        leaf("Millisecond", Value::Int(0)),
    ])
}

/// The faces of a mesh as an export writes them: a quad-remeshed mesh's
/// quads, not their halves, and otherwise its triangles. With UVs, a quad
/// whose triangles disagree on a UV is written as its triangles
/// (`Mesh::faces_with_uvs`, which `fbx/uvs.rs` writes the UVs of).
fn faces(mesh: &Mesh) -> Box<dyn Iterator<Item = &[u32]> + '_> {
    match &mesh.polys {
        Some(_) if mesh.has_uvs() => Box::new(mesh.faces_with_uvs().map_or(Vec::new(), |f| f.faces).into_iter()),
        Some(polys) => Box::new(polys.start.windows(2).map(|f| &polys.indices[f[0] as usize..f[1] as usize])),
        None => Box::new(mesh.indices.chunks_exact(3)),
    }
}

fn put(w: &mut dyn Write, v: Vec3) -> std::io::Result<()> {
    let mut bytes = [0u8; 24];
    for (axis, value) in v.to_array().iter().enumerate() {
        bytes[axis * 8..axis * 8 + 8].copy_from_slice(&f64::from(*value).to_le_bytes());
    }
    w.write_all(&bytes)
}

/// A piece's mesh placed by `model`: its points, its faces, its normals when
/// it has one for every point, and its paint when it has any.
fn geometry<'a>(id: i64, name: &str, piece: &Piece<'a>, model: Mat4, weights_in_uvs: bool) -> Node<'a> {
    let (mesh, points) = (piece.mesh, piece.points);
    let linear = Mat3::from_mat4(model);
    let determinant = linear.determinant();
    // A negative scale mirrors the model and turns every face inside out.
    let mirrored = determinant < 0.0;
    // Normals go through the inverse transpose: a plain rotation would leave
    // them skewed against a surface that has been stretched.
    let normal_matrix = if determinant.abs() > 1e-20 { linear.inverse().transpose() } else { linear };

    let point_count = points.positions.len();
    let positions: Fill = Box::new(move |w| points.positions.iter().try_for_each(|&p| put(w, model.transform_point3(p))));
    // A face is its corners in turn, and the last of them says so by being
    // written as minus itself, less one.
    let corner_count = faces(mesh).map(<[u32]>::len).sum();
    let corners: Fill = Box::new(move |w| {
        for face in faces(mesh) {
            for k in 0..face.len() {
                let corner = face[if mirrored { face.len() - 1 - k } else { k }] as i32;
                let corner = if k + 1 == face.len() { !corner } else { corner };
                w.write_all(&corner.to_le_bytes())?;
            }
        }
        Ok(())
    });
    let mut children = vec![
        leaf("GeometryVersion", Value::Int(124)),
        leaf("Vertices", Value::Doubles(3 * point_count, positions)),
        leaf("PolygonVertexIndex", Value::Ints(corner_count, corners)),
    ];
    // What each layer holds: the kind of record and its number.
    let mut layers: Vec<Vec<(&'static str, i32)>> = vec![Vec::new()];
    if points.normals.len() == point_count {
        let normals: Fill =
            Box::new(move |w| points.normals.iter().try_for_each(|&n| put(w, (normal_matrix * n).normalize_or_zero())));
        children.push(node("LayerElementNormal", vec![Value::Int(0)], vec![
            leaf("Version", Value::Int(101)),
            leaf("Name", text("")),
            // One normal to a point, in the points' own order ("ByVertice" is FBX's spelling).
            leaf("MappingInformationType", text("ByVertice")),
            leaf("ReferenceInformationType", text("Direct")),
            leaf("Normals", Value::Doubles(3 * point_count, normals)),
        ]));
        layers[0].push(("LayerElementNormal", 0));
    }
    // Its UVs, UV set 0, before any set of material weights (`fbx/uvs.rs`).
    if let Some(record) = uvs::element(mesh, mirrored) {
        children.push(record);
        layers[0].push(("LayerElementUV", 0));
    }
    if let Some(shading) = piece.shading {
        for (record, kind, index) in paint::elements(shading, mesh, mirrored, weights_in_uvs) {
            children.push(record);
            let at = index as usize;
            if layers.len() <= at {
                layers.resize_with(at + 1, Vec::new);
            }
            layers[at].push((kind, index));
        }
    }
    for (number, held) in layers.into_iter().enumerate().filter(|(_, held)| !held.is_empty()) {
        let mut layer = vec![leaf("Version", Value::Int(100))];
        layer.extend(held.into_iter().map(|(kind, index)| {
            node("LayerElement", Vec::new(), vec![leaf("Type", text(kind)), leaf("TypedIndex", Value::Int(index))])
        }));
        children.push(node("Layer", vec![Value::Int(number as i32)], layer));
    }
    node("Geometry", vec![Value::Long(id), named(name, "Geometry"), text("Mesh")], children)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::primitives::{self, Primitive};

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("sculpt_io_fbx_tests");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[derive(Debug, PartialEq)]
    enum Got {
        Int(i32),
        Long(i64),
        Double(f64),
        Text(Vec<u8>),
        Raw(Vec<u8>),
        Ints(Vec<i32>),
        Doubles(Vec<f64>),
    }

    struct Record {
        name: String,
        values: Vec<Got>,
        children: Vec<Record>,
    }

    impl Record {
        fn child(&self, name: &str) -> &Record {
            self.children.iter().find(|c| c.name == name).unwrap_or_else(|| panic!("no {name} in {}", self.name))
        }

        fn all(&self, name: &str) -> Vec<&Record> {
            self.children.iter().filter(|c| c.name == name).collect()
        }

        fn doubles(&self, name: &str) -> &[f64] {
            match &self.child(name).values[0] {
                Got::Doubles(v) => v,
                other => panic!("{name} is {other:?}"),
            }
        }

        fn ints(&self, name: &str) -> &[i32] {
            match &self.child(name).values[0] {
                Got::Ints(v) => v,
                other => panic!("{name} is {other:?}"),
            }
        }

        fn text(&self, at: usize) -> String {
            match &self.values[at] {
                Got::Text(v) => String::from_utf8_lossy(v).into_owned(),
                other => panic!("{} value {at} is {other:?}", self.name),
            }
        }
    }

    fn u32_at(bytes: &[u8], at: usize) -> usize {
        u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize
    }

    /// A family of records, up to and past the sentinel that closes it, as a
    /// reader that trusts nothing reads it: every stated offset and length
    /// has to be where the bytes really end.
    fn read_family(bytes: &[u8], at: &mut usize) -> Vec<Record> {
        let mut family = Vec::new();
        loop {
            let end = u32_at(bytes, *at);
            if end == 0 {
                assert_eq!(bytes[*at..*at + 13], SENTINEL);
                *at += 13;
                return family;
            }
            let (count, values_len, name_len) = (u32_at(bytes, *at + 4), u32_at(bytes, *at + 8), bytes[*at + 12] as usize);
            *at += 13;
            let name = String::from_utf8(bytes[*at..*at + name_len].to_vec()).unwrap();
            *at += name_len;
            let values_end = *at + values_len;
            let values = (0..count).map(|_| read_value(bytes, at)).collect();
            assert_eq!(*at, values_end, "{name}: its values are not the length it says");
            let children = if *at < end { read_family(bytes, at) } else { Vec::new() };
            assert_eq!(*at, end, "{name}: it does not end where it says");
            family.push(Record { name, values, children });
        }
    }

    fn read_value(bytes: &[u8], at: &mut usize) -> Got {
        let kind = bytes[*at];
        *at += 1;
        let mut take = |n: usize| {
            *at += n;
            &bytes[*at - n..*at]
        };
        match kind {
            b'I' => Got::Int(i32::from_le_bytes(take(4).try_into().unwrap())),
            b'L' => Got::Long(i64::from_le_bytes(take(8).try_into().unwrap())),
            b'D' => Got::Double(f64::from_le_bytes(take(8).try_into().unwrap())),
            b'S' | b'R' => {
                let length = u32::from_le_bytes(take(4).try_into().unwrap()) as usize;
                let held = take(length).to_vec();
                if kind == b'S' { Got::Text(held) } else { Got::Raw(held) }
            }
            b'i' | b'd' => {
                let head = take(12);
                let (count, packed, length) = (u32_at(head, 0), u32_at(head, 4), u32_at(head, 8));
                assert_eq!(packed, 0, "arrays are written as they are");
                let width = if kind == b'i' { 4 } else { 8 };
                assert_eq!(length, count * width);
                let held = take(length);
                if kind == b'i' {
                    Got::Ints(held.chunks_exact(4).map(|c| i32::from_le_bytes(c.try_into().unwrap())).collect())
                } else {
                    Got::Doubles(held.chunks_exact(8).map(|c| f64::from_le_bytes(c.try_into().unwrap())).collect())
                }
            }
            other => panic!("a value of kind {:?}", other as char),
        }
    }

    /// The whole file: its records under one root, with the close checked.
    fn read(path: &Path) -> Record {
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(&bytes[..23], MAGIC);
        assert_eq!(u32_at(&bytes, 23), 7400);
        let mut at = 27;
        let children = read_family(&bytes, &mut at);
        assert_eq!(&bytes[at..at + 16], FOOT_ID);
        // Zeros up to a multiple of sixteen bytes, then the 140 that end every file.
        assert_eq!((bytes.len() - 140) % 16, 0, "the close is padded to sixteen bytes");
        let pad = bytes.len() - 140 - (at + 20);
        assert!((1..=16).contains(&pad), "{pad} bytes of padding");
        assert!(bytes[at + 16..bytes.len() - 140].iter().all(|&b| b == 0));
        assert_eq!(u32_at(&bytes, bytes.len() - 140), 7400);
        assert!(bytes.ends_with(FOOT_END));
        Record { name: String::new(), values: Vec::new(), children }
    }

    fn vec3s(numbers: &[f64]) -> Vec<Vec3> {
        numbers.chunks_exact(3).map(|c| Vec3::new(c[0] as f32, c[1] as f32, c[2] as f32)).collect()
    }

    /// The faces of a geometry record, each as its corners.
    fn faces_of(geometry: &Record) -> Vec<Vec<usize>> {
        let mut faces = vec![Vec::new()];
        for &corner in geometry.ints("PolygonVertexIndex") {
            let face = faces.last_mut().unwrap();
            if corner < 0 {
                face.push(!corner as usize);
                faces.push(Vec::new());
            } else {
                face.push(corner as usize);
            }
        }
        assert_eq!(faces.pop(), Some(Vec::new()), "the last face is closed");
        faces
    }

    /// Six times the volume the faces enclose: above zero when they face outward.
    fn volume(points: &[Vec3], faces: &[Vec<usize>]) -> f32 {
        let fan = |f: &Vec<usize>| (1..f.len() - 1).map(|k| points[f[0]].dot(points[f[k]].cross(points[f[k + 1]]))).sum::<f32>();
        faces.iter().map(fan).sum()
    }

    #[test]
    fn each_place_is_an_object_of_its_own_where_it_stands() {
        let mesh = primitives::build(Primitive::Sphere, 2_000, 0.5);
        let moved = Mat4::from_scale_rotation_translation(
            Vec3::new(1.0, 2.0, 0.5),
            glam::Quat::from_rotation_z(0.7),
            Vec3::new(3.0, -1.0, 2.0),
        );
        let mirrored = Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0));
        let path = temp("places.fbx");
        assert_eq!(export_fbx_placed(&mesh, &[("Ball", moved), ("Ball_MirrorX", mirrored)], &path).unwrap(), 0);
        let file = read(&path);

        let order: Vec<&str> = file.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(order, [
            "FBXHeaderExtension",
            "FileId",
            "CreationTime",
            "Creator",
            "GlobalSettings",
            "Documents",
            "References",
            "Definitions",
            "Objects",
            "Connections",
            "Takes",
        ]);
        assert_eq!(file.child("FileId").values, [Got::Raw(FILE_ID.to_vec())]);
        let year = &file.child("FBXHeaderExtension").child("CreationTimeStamp").child("Year").values[0];
        assert!(matches!(year, Got::Int(2024..=2200)), "{year:?}");

        let objects = file.child("Objects");
        let (geometries, models) = (objects.all("Geometry"), objects.all("Model"));
        assert_eq!((geometries.len(), models.len()), (2, 2));
        assert_eq!(models[0].text(1), "Ball\u{0}\u{1}Model");
        assert_eq!(models[1].text(1), "Ball_MirrorX\u{0}\u{1}Model");
        assert_eq!(geometries[1].text(1), "Ball_MirrorX\u{0}\u{1}Geometry");
        for (geometry, model) in geometries.iter().zip([moved, mirrored]) {
            let points = vec3s(geometry.doubles("Vertices"));
            assert_eq!(points.len(), mesh.positions.len());
            for (got, was) in points.iter().zip(&mesh.positions) {
                assert!((*got - model.transform_point3(*was)).length() < 1e-5);
            }
            let faces = faces_of(geometry);
            assert_eq!(faces.len(), mesh.triangle_count());
            assert!(faces.iter().all(|f| f.len() == 3));
            assert!(volume(&points, &faces) > 0.0, "the faces look outward, mirrored or not");
            // A normal to a point, of length one, on the outward side of its faces.
            let normals = vec3s(geometry.child("LayerElementNormal").doubles("Normals"));
            assert_eq!(normals.len(), points.len());
            assert!(normals.iter().all(|n| (n.length() - 1.0).abs() < 1e-4));
            let face = &faces[0];
            let facing = (points[face[1]] - points[face[0]]).cross(points[face[2]] - points[face[0]]);
            assert!(face.iter().all(|&i| normals[i].dot(facing) > 0.0));
        }

        // Each model hangs from the scene (0), and each mesh from its model.
        let id = |record: &Record| match record.values[0] {
            Got::Long(id) => id,
            _ => panic!("no id"),
        };
        let links: Vec<(i64, i64)> = file
            .child("Connections")
            .all("C")
            .iter()
            .map(|c| match (&c.values[1], &c.values[2]) {
                (Got::Long(child), Got::Long(parent)) => (*child, *parent),
                _ => panic!("a link without ids"),
            })
            .collect();
        for (geometry, model) in geometries.iter().zip(&models) {
            assert!(links.contains(&(id(model), 0)) && links.contains(&(id(geometry), id(model))));
        }
        assert_eq!(links.len(), 4);
        assert_eq!(file.child("Definitions").child("Count").values, [Got::Int(5)]);
    }

    #[test]
    fn the_file_is_y_up_in_centimetres_which_is_the_models_real_size() {
        let mesh = primitives::build(Primitive::Sphere, 500, 0.5);
        let path = temp("units.fbx");
        export_fbx(&mesh, Mat4::IDENTITY, &path).unwrap();
        let file = read(&path);
        let settings = file.child("GlobalSettings").child("Properties70");
        let setting = |name: &str| {
            let found = settings.children.iter().find(|p| p.text(0) == name).unwrap();
            match found.values[4] {
                Got::Int(v) => f64::from(v),
                Got::Double(v) => v,
                _ => panic!("{name} is not a number"),
            }
        };
        assert_eq!((setting("UpAxis"), setting("FrontAxis"), setting("CoordAxis")), (1.0, 2.0, 0.0));
        // The default sphere is one unit across, which is 10 mm: one centimetre, written as 1.
        assert_eq!(setting("UnitScaleFactor"), 1.0);
        let geometry = file.child("Objects").child("Geometry");
        let tallest = vec3s(geometry.doubles("Vertices")).iter().map(|p| p.y).fold(0.0, f32::max);
        assert!((tallest - 0.5).abs() < 1e-3, "{tallest}");
        // With no name given it still has one.
        assert_eq!(file.child("Objects").child("Model").text(1), "Object\u{0}\u{1}Model");
    }

    #[test]
    fn quads_go_out_as_quads() {
        let corners = [Vec3::ZERO, Vec3::X, Vec3::new(1.0, 1.0, 0.0), Vec3::Y];
        let (mut mesh, to_new) = Mesh::new_mapped(corners.to_vec(), vec![0, 1, 2, 0, 2, 3], None);
        let quad: Vec<u32> = [0usize, 1, 2, 3].iter().map(|&i| to_new[i]).collect();
        mesh.polys = Some(crate::mesh::Polys { start: vec![0, 4], indices: quad.clone() });
        let path = temp("quad.fbx");
        export_fbx(&mesh, Mat4::IDENTITY, &path).unwrap();
        let file = read(&path);
        let geometry = file.child("Objects").child("Geometry");
        let want: Vec<usize> = quad.iter().map(|&i| i as usize).collect();
        assert_eq!(faces_of(geometry), [want.clone()]);

        // Mirrored, the same quad the other way round.
        export_fbx(&mesh, Mat4::from_scale(Vec3::new(1.0, 1.0, -1.0)), &path).unwrap();
        let file = read(&path);
        let turned: Vec<usize> = want.into_iter().rev().collect();
        assert_eq!(faces_of(file.child("Objects").child("Geometry")), [turned]);
    }

    #[test]
    fn a_damaged_model_is_written_mended_and_an_empty_one_not_at_all() {
        let mut mesh = primitives::build(Primitive::Sphere, 2_000, 0.5);
        mesh.positions[10] = Vec3::NAN;
        mesh.positions[20] = Vec3::new(f32::INFINITY, 0.0, f32::NEG_INFINITY);
        let path = temp("damaged.fbx");
        let places = [("a", Mat4::IDENTITY), ("b", Mat4::from_translation(Vec3::X))];
        assert_eq!(export_fbx_placed(&mesh, &places, &path).unwrap(), 2, "the model's points, not its places'");
        let file = read(&path);
        for geometry in file.child("Objects").all("Geometry") {
            assert!(geometry.doubles("Vertices").iter().all(|n| n.is_finite()));
            assert!(geometry.child("LayerElementNormal").doubles("Normals").iter().all(|n| n.is_finite()));
        }
        assert!(mesh.positions[10].is_nan(), "the model is not touched");

        let empty = Mesh::new(Vec::new(), Vec::new(), None);
        assert!(export_fbx(&empty, Mat4::IDENTITY, &temp("empty.fbx")).is_err());
    }

    #[test]
    fn several_objects_go_into_one_file_each_with_its_own_mesh() {
        let ball = primitives::build(Primitive::Sphere, 2_000, 0.5);
        let mut block = primitives::build(Primitive::Cube, 500, 0.5);
        block.positions[3] = Vec3::NAN;
        let empty = Mesh::new(Vec::new(), Vec::new(), None);
        assert_ne!(ball.positions.len(), block.positions.len());
        let aside = Mat4::from_translation(Vec3::new(4.0, 0.0, 0.0));
        let mirrored = Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0));
        let objects: [(&Mesh, &[(&str, Mat4)]); 3] = [
            (&ball, &[("Ball", Mat4::IDENTITY), ("Ball_MirrorX", mirrored)]),
            (&empty, &[("Nothing", Mat4::IDENTITY)]),
            (&block, &[("Block", aside)]),
        ];
        let path = temp("several.fbx");
        assert_eq!(export_fbx_objects(&objects, &path).unwrap(), 1, "the block's one damaged point");
        let file = read(&path);

        let records = file.child("Objects");
        let (geometries, models) = (records.all("Geometry"), records.all("Model"));
        let names: Vec<String> = models.iter().map(|m| m.text(1)).collect();
        // The one with no surface is not in the file.
        assert_eq!(names, ["Ball\u{0}\u{1}Model", "Ball_MirrorX\u{0}\u{1}Model", "Block\u{0}\u{1}Model"]);
        let wanted = [(&ball, Mat4::IDENTITY), (&ball, mirrored), (&block, aside)];
        assert_eq!(geometries.len(), wanted.len());
        for (geometry, (mesh, model)) in geometries.iter().zip(wanted) {
            let points = vec3s(geometry.doubles("Vertices"));
            assert_eq!(points.len(), mesh.positions.len());
            assert!(points.iter().all(|p| p.is_finite()));
            for (got, was) in points.iter().zip(&mesh.positions).filter(|(_, was)| was.is_finite()) {
                assert!((*got - model.transform_point3(*was)).length() < 1e-5);
            }
            let faces = faces_of(geometry);
            assert_eq!(faces.len(), mesh.triangle_count());
            assert!(faces.iter().flatten().all(|&corner| corner < points.len()), "a face names its own mesh's points");
        }

        // No two records share an id, and each mesh hangs from the model of its own place.
        let id = |record: &&Record| match record.values[0] {
            Got::Long(id) => id,
            _ => panic!("no id"),
        };
        let mut ids: Vec<i64> = geometries.iter().chain(&models).map(id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 6);
        let links: Vec<(i64, i64)> = file
            .child("Connections")
            .all("C")
            .iter()
            .map(|c| match (&c.values[1], &c.values[2]) {
                (Got::Long(child), Got::Long(parent)) => (*child, *parent),
                _ => panic!("a link without ids"),
            })
            .collect();
        assert_eq!(links.len(), 6);
        for (geometry, model) in geometries.iter().zip(&models) {
            assert!(links.contains(&(id(model), 0)) && links.contains(&(id(geometry), id(model))));
        }
        assert_eq!(file.child("Definitions").child("Count").values, [Got::Int(7)]);

        assert!(export_fbx_objects(&[(&empty, &[("Nothing", Mat4::IDENTITY)])], &temp("none.fbx")).is_err());
        assert!(export_fbx_objects(&[], &temp("none.fbx")).is_err());
    }

    // ---------------------------------------------------------------- paint

    use crate::mesh::paint::palette::{PaletteEntry, ScenePaint};
    use crate::mesh::paint::{Channels, Mat8, PaintLayer, PaintRow, PaintStack, Rgba8};

    /// A ball painted blue on the vertices above its middle, slot 3 painted
    /// fully on the right-hand ones, slot 5 on every vertex left of
    /// x = -0.3 at 200, and slot 6 at 60 on every fifth vertex.
    fn painted_ball(colour_only: bool) -> Mesh {
        let mut mesh = primitives::build(Primitive::Sphere, 1_000, 0.5);
        let n = mesh.vertex_count();
        let p = mesh.positions.clone();
        let colour = (0..n).map(|v| if p[v].y > 0.0 { Rgba8([0, 0, 255, 255]) } else { Rgba8([0; 4]) }).collect();
        let material = (0..n)
            .map(|v| match p[v].x {
                x if x > 0.1 => Mat8([3, 255]),
                x if x < -0.3 => Mat8([5, 200]),
                _ if v % 5 == 0 => Mat8([6, 60]),
                _ => Mat8([0, 0]),
            })
            .collect();
        let data = Channels { colour: Some(colour), material: (!colour_only).then_some(material), ..Channels::default() };
        let mut stack = PaintStack::new(n);
        stack.layers.push(PaintLayer { row: PaintRow::new(1, "Paint 1".into()), data });
        (stack.next_id, stack.active, stack.table) = (2, Some(1), vec![3, 5, 6]);
        mesh.set_paint(Some(stack));
        mesh
    }

    fn palette() -> ExportPaint {
        let entry = |slot: u8, name: &str, swatch: [f32; 3]| PaletteEntry { slot, name: name.into(), swatch, ..PaletteEntry::default() };
        let palette = vec![entry(3, "Metal - Rust", [1.0, 0.0, 0.0]), entry(5, "Clay - Terracotta", [0.5, 0.25, 0.1])];
        ExportPaint::of([0.5, 0.5, 0.5], &ScenePaint { palette, ..ScenePaint::default() })
    }

    fn id_of(record: &Record) -> i64 {
        match record.values[0] {
            Got::Long(id) => id,
            _ => panic!("no id"),
        }
    }

    fn links_of(file: &Record) -> Vec<(i64, i64)> {
        file.child("Connections")
            .all("C")
            .iter()
            .map(|c| match (&c.values[1], &c.values[2]) {
                (Got::Long(child), Got::Long(parent)) => (*child, *parent),
                _ => panic!("a link without ids"),
            })
            .collect()
    }

    /// What a `Layer` record lists: each element's kind and number.
    fn layer_list(layer: &Record) -> Vec<(String, i32)> {
        layer
            .all("LayerElement")
            .iter()
            .map(|e| match &e.child("TypedIndex").values[0] {
                Got::Int(i) => (e.child("Type").text(0), *i),
                other => panic!("{other:?}"),
            })
            .collect()
    }

    /// The point each corner names, in the order `PolygonVertexIndex` has them.
    fn corner_points(geometry: &Record) -> Vec<i32> {
        geometry.ints("PolygonVertexIndex").iter().map(|&c| if c < 0 { !c } else { c }).collect()
    }

    #[test]
    fn a_painted_model_takes_its_colour_and_its_materials() {
        let mesh = painted_ball(false);
        let paint = palette();
        let painted = Painted::of(&mesh, &paint).unwrap();
        let mirrored = Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0));
        let path = temp("painted.fbx");
        export_fbx_painted(&[(&mesh, &[("Ball", Mat4::IDENTITY), ("Ball_MirrorX", mirrored)])], &paint, &path).unwrap();
        let file = read(&path);
        let objects = file.child("Objects");
        let geometries = objects.all("Geometry");
        let models = objects.all("Model");
        let (n, faces) = (mesh.vertex_count(), mesh.triangle_count());

        for geometry in &geometries {
            let colour = geometry.child("LayerElementColor");
            assert_eq!(colour.child("Name").text(0), "GolemSculpt Paint");
            assert_eq!(colour.child("MappingInformationType").text(0), "ByPolygonVertex");
            assert_eq!(colour.child("ReferenceInformationType").text(0), "IndexToDirect");
            let colours = colour.doubles("Colors");
            assert_eq!(colours.len(), 4 * n);
            for v in [0, n / 3, n - 1] {
                let want = painted.seen[v].map(|b| f64::from(b) / 255.0);
                assert_eq!(&colours[4 * v..4 * v + 4], &[want[0], want[1], want[2], 1.0]);
            }
            // One index per corner, naming the corner's own point, mirrored or not.
            let index = colour.ints("ColorIndex");
            assert_eq!(index.len(), 3 * faces);
            assert_eq!(index, corner_points(geometry).as_slice());

            let material = geometry.child("LayerElementMaterial");
            assert_eq!(material.child("MappingInformationType").text(0), "ByPolygon");
            assert_eq!(material.child("ReferenceInformationType").text(0), "IndexToDirect");
            let per_face = material.ints("Materials");
            assert_eq!(per_face.len(), faces);
            // Base first, then the materials in table order: rust, terracotta (slot 6 never wins a face).
            let want: Vec<i32> = mesh
                .indices
                .chunks_exact(3)
                .map(|t| match painted.dominant(t) {
                    None => 0,
                    Some(0) => 1,
                    Some(1) => 2,
                    other => panic!("{other:?} has most of a face"),
                })
                .collect();
            assert_eq!(per_face, want.as_slice());
            assert!(want.contains(&0) && want.contains(&1) && want.contains(&2));

            let layer = geometry.child("Layer");
            let held = layer_list(layer);
            let names: Vec<&str> = held.iter().map(|(kind, _)| kind.as_str()).collect();
            assert_eq!(names, ["LayerElementNormal", "LayerElementColor", "LayerElementMaterial"]);
            assert!(geometry.all("LayerElementUV").is_empty(), "no UVs unless asked for");
        }

        // One material object each, shared by both models, linked in the order the layer counts them.
        let materials = objects.all("Material");
        let names: Vec<String> = materials.iter().map(|m| m.text(1)).collect();
        assert_eq!(names, ["GolemSculpt Base\u{0}\u{1}Material", "Metal - Rust\u{0}\u{1}Material", "Clay - Terracotta\u{0}\u{1}Material"]);
        let diffuse = |m: &Record| -> Vec<f64> {
            let p = m.child("Properties70").children.iter().find(|p| p.text(0) == "DiffuseColor").unwrap();
            p.values[4..].iter().map(|v| if let Got::Double(d) = v { *d } else { panic!() }).collect()
        };
        assert_eq!(diffuse(materials[1]), [1.0, 0.0, 0.0], "the swatch, in linear light");
        let base = f64::from(crate::io::painted::linear(128.0 / 255.0));
        assert!(diffuse(materials[0]).iter().all(|c| (c - base).abs() < 1e-6));
        let links = links_of(&file);
        for model in &models {
            let mine: Vec<i64> = links.iter().filter(|(_, parent)| *parent == id_of(model)).map(|(child, _)| *child).collect();
            let want: Vec<i64> = materials.iter().map(|m| id_of(m)).collect();
            assert_eq!(mine[1..], want, "its geometry first, then base, rust, terracotta");
        }
        assert_eq!(file.child("Definitions").child("Count").values, [Got::Int(1 + 4 + 3)]);
        let kinds: Vec<String> = file.child("Definitions").all("ObjectType").iter().map(|k| k.text(0)).collect();
        assert_eq!(kinds, ["GlobalSettings", "Geometry", "Model", "Material"]);
    }

    #[test]
    fn material_weights_go_in_uv_sets_when_asked() {
        let mesh = painted_ball(false);
        let mut paint = palette();
        paint.weights_in_uvs = true;
        let painted = Painted::of(&mesh, &paint).unwrap();
        let path = temp("weights.fbx");
        export_fbx_painted(&[(&mesh, &[("Ball", Mat4::IDENTITY)])], &paint, &path).unwrap();
        let file = read(&path);
        let geometry = file.child("Objects").child("Geometry");
        let n = mesh.vertex_count();
        let uvs = geometry.all("LayerElementUV");
        let names: Vec<String> = uvs.iter().map(|u| u.child("Name").text(0)).collect();
        assert_eq!(names, ["UVMap", "Weights Metal - Rust + Clay - Terracotta", "Weights Material 6"]);
        let corners = corner_points(geometry);
        assert_eq!(uvs[0].doubles("UV"), [0.0, 0.0]);
        assert!(uvs[0].ints("UVIndex").len() == corners.len() && uvs[0].ints("UVIndex").iter().all(|&i| i == 0));
        for (set, uv) in uvs[1..].iter().enumerate() {
            assert_eq!(uv.ints("UVIndex"), corners.as_slice());
            let values = uv.doubles("UV");
            assert_eq!(values.len(), 2 * n);
            for v in 0..n {
                let first = f64::from(painted.weight(v, 2 * set));
                let second = if 2 * set + 1 < 3 { f64::from(painted.weight(v, 2 * set + 1)) } else { 0.0 };
                assert_eq!((values[2 * v], values[2 * v + 1]), (first, second));
            }
        }
        // Layer 0 holds UV set 0 with the rest; each further set has a layer of its own.
        let layers = geometry.all("Layer");
        assert_eq!(layers.len(), 3);
        assert!(layer_list(layers[0]).contains(&("LayerElementUV".into(), 0)));
        assert_eq!(layer_list(layers[1]), [("LayerElementUV".to_owned(), 1)]);
        assert_eq!(layer_list(layers[2]), [("LayerElementUV".to_owned(), 2)]);
    }

    /// Writes `painted.fbx` and what Blender's importer should find in it
    /// into the folder GOLEMSCULPT_PAINT_CHECK names, for
    /// tests/blender_link/check_fbx_paint.py (see tests/blender_link/README.md).
    #[test]
    #[ignore = "writes the file tests/blender_link/check_fbx_paint.py reads in Blender"]
    fn blender_fbx_check_file() {
        let folder = std::path::PathBuf::from(std::env::var_os("GOLEMSCULPT_PAINT_CHECK").expect("set GOLEMSCULPT_PAINT_CHECK"));
        std::fs::create_dir_all(&folder).unwrap();
        let mesh = painted_ball(false);
        let mut paint = palette();
        paint.weights_in_uvs = true;
        let painted = Painted::of(&mesh, &paint).unwrap();
        let mirrored = Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0)) * Mat4::from_translation(Vec3::new(2.0, 0.0, 0.0));
        export_fbx_painted(&[(&mesh, &[("Ball", Mat4::IDENTITY), ("Ball_MirrorX", mirrored)])], &paint, &folder.join("painted.fbx")).unwrap();
        let shading = paint::Shading::of(&mesh, &painted, &paint);
        let names: Vec<String> = shading.keys.iter().map(|&key| paint::described(key, &paint).0).collect();
        let faces: Vec<i32> = mesh
            .indices
            .chunks_exact(3)
            .map(|t| shading.keys.iter().position(|k| *k == painted.dominant(t).map(|k| painted.table[k])).unwrap() as i32)
            .collect();
        let weights: Vec<Vec<f32>> = painted.materials.iter().map(|&k| (0..mesh.vertex_count()).map(|v| painted.weight(v, k)).collect()).collect();
        let expect = serde_json::json!({
            "vertices": mesh.vertex_count(),
            "seen": painted.seen,
            "materials": names,
            "faces": faces,
            "weights": weights,
        });
        std::fs::write(folder.join("fbx_expect.json"), expect.to_string()).unwrap();
    }

    #[test]
    fn a_model_painted_with_colour_alone_has_no_materials_and_an_unpainted_one_no_paint() {
        let painted = painted_ball(true);
        let plain = primitives::build(Primitive::Cube, 500, 0.5);
        let path = temp("colour only.fbx");
        let objects: [(&Mesh, &[(&str, Mat4)]); 2] = [(&painted, &[("Ball", Mat4::IDENTITY)]), (&plain, &[("Block", Mat4::IDENTITY)])];
        export_fbx_painted(&objects, &palette(), &path).unwrap();
        let file = read(&path);
        let geometries = file.child("Objects").all("Geometry");
        assert_eq!(geometries[0].child("LayerElementColor").ints("ColorIndex").len(), 3 * painted.triangle_count());
        assert!(geometries[0].all("LayerElementMaterial").is_empty());
        assert!(geometries[1].all("LayerElementColor").is_empty() && geometries[1].all("LayerElementMaterial").is_empty());
        assert!(file.child("Objects").all("Material").is_empty());
        assert_eq!(links_of(&file).len(), 4);
    }
}
