//! A painted mesh in an FBX file: its colour, its materials, and (when asked)
//! its material weights for game shaders.
//!
//! - **Colour**: one `LayerElementColor` named "GolemSculpt Paint", the
//!   colour seen at each vertex (the paint over the Base Colour, sRGB, as
//!   every export writes it: `io/painted.rs`), mapped `ByPolygonVertex` with
//!   `IndexToDirect`: one colour per vertex in `Colors`, and for every corner
//!   of every face, in the order `PolygonVertexIndex` walks them, which
//!   colour it is in `ColorIndex`. That is how Blender writes colours, and
//!   what Unity and Blender read; Unity reads the first colour set only, so
//!   there is one. The numbers are sRGB, as Blender's exporter writes them by
//!   default and its importer reads them.
//! - **Materials**: a face has the material with most weight over its
//!   corners (`Painted::dominant`), the Look's own finish ("GolemSculpt Base")
//!   where no painted material has more. One `Material` object per material
//!   any face has, named as the palette names it, its `DiffuseColor` the
//!   palette swatch (in linear light, as Blender writes and reads a
//!   material's colour), connected to each model that uses it; the
//!   `LayerElementMaterial` says, per face (`ByPolygon`, `IndexToDirect`),
//!   which of the model's materials it has, counted in the order they are
//!   connected. Unity makes a sub-mesh per material, Blender a material slot.
//!   A model painted with colour alone has no materials, as before.
//! - **Weights in UV sets** (the File tab's option, off unless asked for): a
//!   game engine has no idea of our materials, but a shader of one's own can
//!   blend between textures by numbers it is given. UV set 0 is all zero (the
//!   sculpt has no UVs), and UV sets 1, 2, 3, 4 hold the weights of the
//!   model's materials two at a time (U the first, V the second), in the
//!   order the UV sets' names give ("Weights <first> + <second>"). Unity
//!   reads them as `uv2` to `uv5` (Mesh.uv2...), Blender as UV maps.

use super::{Fill, Node, Value, faces, leaf, named, node, property, text};
use crate::io::painted::{BASE_MATERIAL, ExportPaint, Painted, linear};
use crate::mesh::Mesh;

/// A material of the file: `None` for the Look's own finish, else a palette slot.
pub(super) type Key = Option<u8>;

/// One mesh's paint, ready to be written into each of its places.
pub(super) struct Shading<'a> {
    painted: &'a Painted,
    /// The materials its faces have, in the order they are connected to each
    /// of its models: the material layer counts in this order.
    pub keys: Vec<Key>,
    /// Each face's material, as a place in `keys`.
    face: Vec<i32>,
    /// The names of the materials whose weights go in UV sets, table order.
    weight_names: Vec<String>,
}

impl<'a> Shading<'a> {
    pub(super) fn of(mesh: &Mesh, painted: &'a Painted, paint: &ExportPaint) -> Shading<'a> {
        let weight_names = painted.materials.iter().map(|&k| paint.name_of(painted.table[k])).collect();
        if painted.materials.is_empty() {
            return Shading { painted, keys: Vec::new(), face: Vec::new(), weight_names };
        }
        let key_of = |k: Option<usize>| k.map(|k| painted.table[k]);
        let dominant: Vec<Key> = faces(mesh).map(|corners| key_of(painted.dominant(corners))).collect();
        let mut keys: Vec<Key> = Vec::new();
        // The base first, then the materials in table order: the same order on every model.
        for key in std::iter::once(None).chain(painted.materials.iter().map(|&k| key_of(Some(k)))) {
            if dominant.contains(&key) {
                keys.push(key);
            }
        }
        let face = dominant.iter().map(|key| keys.iter().position(|k| k == key).unwrap_or(0) as i32).collect();
        Shading { painted, keys, face, weight_names }
    }
}

/// The name and colour (linear) of a material of the file.
pub(super) fn described(key: Key, paint: &ExportPaint) -> (String, [f64; 3]) {
    let (name, swatch) = match key {
        None => (BASE_MATERIAL.to_owned(), paint.base_unit()),
        Some(slot) => (paint.name_of(slot), paint.swatch_of(slot)),
    };
    (name, swatch.map(|c| f64::from(linear(c.clamp(0.0, 1.0)))))
}

/// The `Material` object of the file for `key`.
pub(super) fn material<'a>(id: i64, key: Key, paint: &ExportPaint) -> Node<'a> {
    let (name, colour) = described(key, paint);
    let rgb = |c: [f64; 3]| c.map(Value::Double).into();
    node("Material", vec![Value::Long(id), named(&name, "Material"), text("")], vec![
        leaf("Version", Value::Int(102)),
        leaf("ShadingModel", text("Phong")),
        leaf("MultiLayer", Value::Int(0)),
        node("Properties70", Vec::new(), vec![
            property("ShadingModel", ["KString", ""], "", vec![text("Phong")]),
            property("DiffuseColor", ["Color", ""], "A", rgb(colour)),
            property("DiffuseFactor", ["Number", ""], "A", vec![Value::Double(1.0)]),
            // The older name some readers still look for.
            property("Diffuse", ["Vector3D", "Vector"], "", rgb(colour)),
        ]),
    ])
}

/// The point each corner of each face names, in the order the faces' corners are written.
fn corners(mesh: &Mesh, mirrored: bool) -> impl Iterator<Item = u32> + '_ {
    faces(mesh).flat_map(move |face| (0..face.len()).map(move |k| face[if mirrored { face.len() - 1 - k } else { k }]))
}

/// The geometry's paint records, and which layer each goes in (its own
/// number: layer 0 holds the colour, the materials and UV set 0).
pub(super) fn elements<'a>(
    shading: &'a Shading<'a>,
    mesh: &'a Mesh,
    mirrored: bool,
    weights_in_uvs: bool,
) -> Vec<(Node<'a>, &'static str, i32)> {
    let painted = shading.painted;
    let points = painted.seen.len();
    let corner_count = faces(mesh).map(<[u32]>::len).sum::<usize>();
    let per_corner: Fill = Box::new(move |w| corners(mesh, mirrored).try_for_each(|v| w.write_all(&(v as i32).to_le_bytes())));
    let colours: Fill = Box::new(move |w| {
        painted.seen.iter().try_for_each(|c| {
            let mut bytes = [0u8; 32];
            for (k, value) in [c[0], c[1], c[2], 255].iter().enumerate() {
                bytes[k * 8..k * 8 + 8].copy_from_slice(&(f64::from(*value) / 255.0).to_le_bytes());
            }
            w.write_all(&bytes)
        })
    });
    let mut out = vec![(
        node("LayerElementColor", vec![Value::Int(0)], vec![
            leaf("Version", Value::Int(101)),
            leaf("Name", text("GolemSculpt Paint")),
            leaf("MappingInformationType", text("ByPolygonVertex")),
            leaf("ReferenceInformationType", text("IndexToDirect")),
            leaf("Colors", Value::Doubles(4 * points, colours)),
            leaf("ColorIndex", Value::Ints(corner_count, per_corner)),
        ]),
        "LayerElementColor",
        0,
    )];
    if !shading.keys.is_empty() {
        let face = &shading.face;
        let materials: Fill = Box::new(move |w| face.iter().try_for_each(|m| w.write_all(&m.to_le_bytes())));
        out.push((
            node("LayerElementMaterial", vec![Value::Int(0)], vec![
                leaf("Version", Value::Int(101)),
                leaf("Name", text("")),
                leaf("MappingInformationType", text("ByPolygon")),
                leaf("ReferenceInformationType", text("IndexToDirect")),
                leaf("Materials", Value::Ints(face.len(), materials)),
            ]),
            "LayerElementMaterial",
            0,
        ));
    }
    if weights_in_uvs && !painted.materials.is_empty() {
        out.extend(weight_sets(shading, mesh, mirrored, corner_count));
    }
    out
}

/// UV set 0, all zero, then the weights two materials to a set (see the module notes).
fn weight_sets<'a>(shading: &'a Shading<'a>, mesh: &'a Mesh, mirrored: bool, corner_count: usize) -> Vec<(Node<'a>, &'static str, i32)> {
    let painted = shading.painted;
    let uv = |index: i32, name: &str, count: usize, uv: Fill<'a>, uv_index: Fill<'a>| {
        node("LayerElementUV", vec![Value::Int(index)], vec![
            leaf("Version", Value::Int(101)),
            leaf("Name", text(name)),
            leaf("MappingInformationType", text("ByPolygonVertex")),
            leaf("ReferenceInformationType", text("IndexToDirect")),
            leaf("UV", Value::Doubles(count, uv)),
            leaf("UVIndex", Value::Ints(corner_count, uv_index)),
        ])
    };
    // Every corner names the one point (0, 0).
    let zero: Fill = Box::new(|w| w.write_all(&[0u8; 16]));
    let to_zero: Fill = Box::new(move |w| (0..corner_count).try_for_each(|_| w.write_all(&0i32.to_le_bytes())));
    let mut out = vec![(uv(0, "UVMap", 2, zero, to_zero), "LayerElementUV", 0)];
    for (set, pair) in painted.materials.chunks(2).enumerate() {
        let pair: Vec<usize> = pair.to_vec();
        let names: Vec<&str> = (0..pair.len()).map(|i| shading.weight_names[set * 2 + i].as_str()).collect();
        let name = format!("Weights {}", names.join(" + "));
        let weights: Fill = Box::new(move |w| {
            (0..painted.seen.len()).try_for_each(|v| {
                let at = |i: usize| pair.get(i).map_or(0.0, |&k| f64::from(painted.weight(v, k)));
                w.write_all(&at(0).to_le_bytes())?;
                w.write_all(&at(1).to_le_bytes())
            })
        });
        let per_corner: Fill = Box::new(move |w| corners(mesh, mirrored).try_for_each(|v| w.write_all(&(v as i32).to_le_bytes())));
        let index = set as i32 + 1;
        out.push((uv(index, &name, 2 * painted.seen.len(), weights, per_corner), "LayerElementUV", index));
    }
    out
}
