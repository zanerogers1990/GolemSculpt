//! The UVs of an FBX geometry (see `mesh/uvs.rs` for what they are): its
//! `LayerElementUV` of UV set 0 (the first there is, if none says it is set
//! 0), given for each corner of each polygon (`ByPolygonVertex`) or for each
//! point (`ByVertice`, which every corner on the point then takes), either
//! as they are (`Direct`) or through a list of which UV each takes
//! (`IndexToDirect`). An FBX's V runs up the picture, as the app's does.
//!
//! A layer that cannot be read, or of a kind not read, costs the UVs and
//! nothing else: the model opened without them before they were read at
//! all. A corner the layer gives none for (an index of -1, or past the list)
//! has none; `polygons.rs` puts it at (0, 0), and takes a layer that gives
//! every corner the same UV for no UVs.

use glam::Vec2;

use super::Record;

/// The UV of each corner, in the order `corners` (the geometry's
/// `PolygonVertexIndex`) lists them; empty for a geometry without a layer
/// that can be read.
pub(super) fn uvs_of(geometry: &Record, corners: impl Iterator<Item = i32>) -> Vec<Vec2> {
    let layers = || geometry.children.iter().filter(|r| r.name == b"LayerElementUV");
    let Some(layer) = layers().find(|r| r.long(0) == Some(0)).or_else(|| layers().next()) else {
        return Vec::new();
    };
    let Some(Ok(table)) = layer.array(b"UV").map(|uv| uv.doubles()) else { return Vec::new() };
    let indexed = matches!(layer.child_text(b"ReferenceInformationType"), Some(b"IndexToDirect" | b"Index"));
    let index = match layer.array(b"UVIndex").map(|index| index.ints()) {
        Some(Ok(index)) if indexed => index,
        _ if indexed => return Vec::new(),
        _ => Vec::new(),
    };
    // The UV of the `k`th entry the mapping counts.
    let pick = |k: usize| -> Vec2 {
        let at = if indexed { index.get(k).and_then(|&i| usize::try_from(i).ok()) } else { Some(k) };
        let uv = at.and_then(|at| table.get(at.checked_mul(2)?..at.checked_mul(2)?.checked_add(2)?));
        uv.map_or(Vec2::NAN, |uv| Vec2::new(uv[0] as f32, uv[1] as f32))
    };
    match layer.child_text(b"MappingInformationType") {
        Some(b"ByPolygonVertex") => corners.enumerate().map(|(k, _)| pick(k)).collect(),
        Some(b"ByVertice" | b"ByVertex" | b"ByControlPoint") => {
            corners.map(|corner| pick((if corner < 0 { !corner } else { corner }) as usize)).collect()
        }
        _ => Vec::new(),
    }
}
