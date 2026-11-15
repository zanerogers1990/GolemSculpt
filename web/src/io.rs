//! The few pieces of the desktop app's file module that the core reaches for.
//! The web build keeps scenes in memory and in OBJ/.sclp downloads, so the
//! scene-detail slots are a plain map here.
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};

pub mod slot {
    pub const PAINT: &str = "paint";
}

#[derive(Clone, Debug, Default)]
pub struct Details {
    scene: Map<String, Value>,
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

use std::io::Read;

use crate::mesh::Mesh;
use crate::mesh::layers::SavedLayers;
use crate::mesh::levels::SavedLevels;

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
    let vertices = positions.len();
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
        groups: None,
        paint: None,
        uvs: None,
        lost,
    };
    let (mesh, lost) = levels::build(loaded);
    (mesh, lost[0] || lost[1])
}
