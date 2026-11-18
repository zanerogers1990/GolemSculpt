//! The few pieces of the desktop app's file module that the core reaches for.
//! The web build keeps scenes in memory and in OBJ/.sclp downloads, so the
//! scene-detail slots are a plain map here.
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};

pub mod slot {
    pub const PAINT: &str = "paint";
    /// The reference pictures' names and places (`references.rs`).
    pub const REFERENCES: &str = "references";
}

#[derive(Clone, Debug, Default)]
pub struct Details {
    scene: Map<String, Value>,
    /// The reference pictures' image files (the desktop's `io/references.rs`).
    pictures: Vec<std::sync::Arc<[u8]>>,
}

/// The desktop's `io::read_over`, as much as the web needs: what `file`
/// holds, read as a `T` (whose missing fields take their defaults).
pub(crate) fn read_over<T: Serialize + DeserializeOwned>(_base: &T, file: &Value) -> Option<T> {
    T::deserialize(file).ok()
}

impl Details {
    pub fn set<T: Serialize>(&mut self, slot: &str, value: &T) {
        if let Ok(value) = serde_json::to_value(value) {
            self.scene.insert(slot.to_owned(), value);
        }
    }

    pub fn get_over<T: Serialize + DeserializeOwned>(&self, slot: &str, _base: &T) -> Option<T> {
        T::deserialize(self.scene.get(slot)?).ok()
    }

    pub fn get<T: DeserializeOwned>(&self, slot: &str) -> Option<T> {
        T::deserialize(self.scene.get(slot)?).ok()
    }

    pub fn set_pictures(&mut self, images: Vec<std::sync::Arc<[u8]>>) {
        self.pictures = images;
    }

    pub fn pictures(&self) -> &[std::sync::Arc<[u8]>] {
        &self.pictures
    }
}

pub mod vec3 {
    use glam::Vec3;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(v: &Vec3, s: S) -> Result<S::Ok, S::Error> {
        v.to_array().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec3, D::Error> {
        <[f32; 3]>::deserialize(d).map(Vec3::from_array)
    }
}

// ---- a mesh's levels, layers and hidden faces, for the browser's storage ----
//
// The desktop's own `.sclp` blocks for them, compiled here as they are
// (`io/layers.rs`: LAYR, `io/levels.rs`: LVLS), so the browser keeps a
// mesh's subdivision levels and sculpt layers exactly as a desktop file
// does, and `levels::build` puts them back on the mesh it rebuilds.

#[path = "../../src/io/layers.rs"]
#[allow(dead_code)]
mod layers;
#[path = "../../src/io/levels.rs"]
#[allow(dead_code)]
mod levels;
// A mesh's paint layers as the desktop's file keeps them (`PNTL`).
#[path = "../../src/io/paint.rs"]
#[allow(dead_code)]
mod paint;

use std::io::Read;

use crate::mesh::Mesh;
use crate::mesh::layers::SavedLayers;
use crate::mesh::levels::SavedLevels;

/// What an FBX's paint goes out as (the desktop's `io/painted.rs`, mounted
/// in fbx.rs), where the desktop's FBX writer looks for it.
#[cfg(not(test))]
pub(crate) use crate::fbx::painted;

/// A mesh read from a file with its quads, its UVs or its colours, built the
/// way the desktop builds one (`io::groups::imported_mesh`, fbx.rs).
pub fn build_imported(
    positions: Vec<glam::Vec3>,
    indices: Vec<u32>,
    polys: Option<crate::mesh::Polys>,
    uvs: Option<Vec<glam::Vec2>>,
    paint: Option<crate::mesh::paint::PaintStack>,
) -> Mesh {
    let loaded = LoadedMesh {
        positions,
        indices,
        mask: None,
        polys,
        hidden: None,
        levels: None,
        layers: None,
        groups: None,
        paint,
        uvs,
        lost: [false; 5],
    };
    levels::build(loaded).0
}

/// The desktop's face groups as a file keeps them; the web version keeps
/// none, but `levels::build` takes the slot.
mod groups {
    pub struct SavedGroups {
        pub ids: Vec<u16>,
        pub levels: Vec<Option<Vec<u16>>>,
    }
}

/// A mesh as read, before it is built (the desktop's `io::LoadedMesh`).
struct LoadedMesh {
    positions: Vec<glam::Vec3>,
    indices: Vec<u32>,
    mask: Option<Vec<f32>>,
    polys: Option<crate::mesh::Polys>,
    hidden: Option<Vec<bool>>,
    levels: Option<SavedLevels>,
    layers: Option<SavedLayers>,
    groups: Option<groups::SavedGroups>,
    paint: Option<crate::mesh::paint::PaintStack>,
    uvs: Option<Vec<glam::Vec2>>,
    lost: [bool; 5],
}

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

/// `mesh`'s levels and layers as the desktop's file blocks (each a whole
/// body, mesh number 0 first), or empty where it has none.
pub fn level_blocks(mesh: &Mesh) -> (Vec<u8>, Vec<u8>) {
    let mut lvls = Vec::new();
    if let Some(l) = &mesh.levels {
        let _ = levels::write(&mut lvls, 0, &l.saved());
    }
    let mut layr = Vec::new();
    if let Some(l) = &mesh.layers {
        let _ = layers::write(&mut layr, 0, &l.saved(None));
    }
    (lvls, layr)
}

/// Builds a mesh from what the browser kept, with its hidden faces, levels
/// and layers back on it where they still fit (`lvls` and `layr` as
/// [`level_blocks`] made them, empty for none). The second value is whether
/// any of them had to be left off.
pub fn build_mesh(positions: Vec<glam::Vec3>, indices: Vec<u32>, mask: Vec<f32>, hidden: Vec<bool>, lvls: &[u8], layr: &[u8]) -> (Mesh, bool) {
    build_mesh_painted(positions, indices, mask, hidden, lvls, layr, &[], &[])
}

/// `mesh`'s face groups (one u16 a triangle) and paint (the desktop's
/// `PNTL` block body, mesh number 0 first), or empty where it has none: what
/// the browser keeps of them beside [`level_blocks`] (layout W04).
pub fn paint_blocks(mesh: &Mesh) -> (Vec<u8>, Vec<u8>) {
    let grps = if mesh.has_groups() { bytemuck::cast_slice(mesh.group_ids()).to_vec() } else { Vec::new() };
    let mut pntl = Vec::new();
    if let Some(stack) = mesh.paint_stack() {
        let _ = paint::write(&mut pntl, 0, &stack);
    }
    (grps, pntl)
}

/// [`build_mesh`] with the face groups and paint [`paint_blocks`] made
/// (empty for none) put back on the mesh too.
#[allow(clippy::too_many_arguments)]
pub fn build_mesh_painted(
    positions: Vec<glam::Vec3>,
    indices: Vec<u32>,
    mask: Vec<f32>,
    hidden: Vec<bool>,
    lvls: &[u8],
    layr: &[u8],
    grps: &[u8],
    pntl: &[u8],
) -> (Mesh, bool) {
    let vertices = positions.len();
    let triangles = indices.len() / 3;
    let ids: Vec<u16> = if grps.len() == triangles * 2 { bytemuck::pod_collect_to_vec(grps) } else { Vec::new() };
    let groups = (!ids.is_empty()).then_some(groups::SavedGroups { ids, levels: Vec::new() });
    let paint =
        pntl.get(4..).filter(|_| pntl.len() > 4).and_then(|mut body| paint::read(&mut body, pntl.len() as u64, vertices));
    let lost_more = (!grps.is_empty() && groups.is_none()) || (!pntl.is_empty() && paint.is_none());
    let mut lost = [false; 5];
    fn block(bytes: &[u8]) -> Option<&[u8]> {
        bytes.get(4..).filter(|_| bytes.len() > 4)
    }
    let levels = block(lvls).and_then(|mut body| levels::read(&mut body, lvls.len() as u64));
    lost[0] = !lvls.is_empty() && levels.is_none();
    let layers = block(layr).and_then(|mut body| layers::read(&mut body, layr.len() as u64, vertices));
    lost[1] = !layr.is_empty() && layers.is_none();
    let loaded = LoadedMesh {
        positions,
        indices,
        mask: Some(mask),
        polys: None,
        hidden: (!hidden.is_empty()).then_some(hidden),
        levels,
        layers,
        groups,
        paint,
        uvs: None,
        lost,
    };
    let (mesh, lost) = levels::build(loaded);
    (mesh, lost[0] || lost[1] || lost[3] || lost_more)
}
