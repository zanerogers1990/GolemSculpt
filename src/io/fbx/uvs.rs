//! A mesh's UVs in an FBX file (see `mesh/uvs.rs` for what they are): one
//! `LayerElementUV` named "UVMap", UV set 0, written as Blender's exporter
//! writes one. It is mapped `ByPolygonVertex` with `IndexToDirect`: each
//! different UV once in `UV` (U then V), and for every corner of every face,
//! in the order `PolygonVertexIndex` walks them, which of those it is in
//! `UVIndex`. Blender, Unity and Autodesk's own reader take set 0 for the
//! model's UVs; with material weights asked for in UV sets (`paint.rs`),
//! those come after it, in sets 1 on.
//!
//! The faces are the ones `fbx::faces` gives, so a quad whose triangles
//! disagree on a UV goes out as its triangles, in both lists alike
//! (`Mesh::faces_with_uvs`).

use std::collections::HashMap;

use super::{Fill, Node, Value, leaf, node, text};
use crate::mesh::Mesh;

/// The UV record of `mesh`'s geometry, its faces turned round when
/// `mirrored` as their corners are; `None` for a mesh without UVs.
pub(super) fn element<'a>(mesh: &Mesh, mirrored: bool) -> Option<Node<'a>> {
    let faces = mesh.faces_with_uvs()?;
    let mut seen: HashMap<[u32; 2], i32> = HashMap::new();
    let mut table = Vec::new();
    let mut index = Vec::with_capacity(faces.uvs.len());
    let mut first = 0;
    for face in &faces.faces {
        let n = face.len();
        for k in 0..n {
            let uv = faces.uvs[first + if mirrored { n - 1 - k } else { k }];
            // +0.0 so that -0.0 and 0.0 are one entry.
            let key = [(uv.x + 0.0).to_bits(), (uv.y + 0.0).to_bits()];
            index.push(*seen.entry(key).or_insert_with(|| {
                table.push(uv);
                table.len() as i32 - 1
            }));
        }
        first += n;
    }
    let (values, corners) = (2 * table.len(), index.len());
    let uv: Fill = Box::new(move |w| {
        table.iter().try_for_each(|uv| {
            w.write_all(&f64::from(uv.x).to_le_bytes())?;
            w.write_all(&f64::from(uv.y).to_le_bytes())
        })
    });
    let uv_index: Fill = Box::new(move |w| index.iter().try_for_each(|i| w.write_all(&i.to_le_bytes())));
    Some(node("LayerElementUV", vec![Value::Int(0)], vec![
        leaf("Version", Value::Int(101)),
        leaf("Name", text("UVMap")),
        leaf("MappingInformationType", text("ByPolygonVertex")),
        leaf("ReferenceInformationType", text("IndexToDirect")),
        leaf("UV", Value::Doubles(values, uv)),
        leaf("UVIndex", Value::Ints(corners, uv_index)),
    ]))
}
