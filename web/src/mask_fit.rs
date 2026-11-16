//! The desktop's Mask Fit tool (`crates/maskfit`) in the web version.
//!
//! The tool itself is hosted with the others (`tools.rs`). It is the one
//! hosted tool that looks at a second object: the shell is the active
//! object, and the head it is fitted to is another. The web tool scene shows
//! a tool only the active object, so for Mask Fit it also shows the other
//! objects that are shown, to look at only, each placed in the active
//! object's space (where the tool works).

use glam::Mat4;
use sculpt_toolkit::{MeshId, MeshView, ObjectId, ObjectInfo, Placement};

use crate::mesh::Mesh;
use crate::scene::Object;

/// The ids the other objects go under, clear of the tool scene's own (the
/// model is 1, objects a tool makes count up from 1 000 000).
const FIRST_OTHER: u64 = 2_000_000;

/// Another object of the scene, as the tool sees it.
pub struct Other<'a> {
    pub id: ObjectId,
    name: &'a str,
    mesh: &'a Mesh,
    /// Its place in the active object's space.
    placement: Placement,
}

/// The other objects the tool `tool` sees: for Mask Fit every other object
/// shown, for the rest none.
pub fn others<'a>(tool: &str, objects: &'a [Object], active: u64) -> Vec<Other<'a>> {
    if tool != "maskfit" {
        return Vec::new();
    }
    let Some(model) = objects.iter().find(|o| o.id == active).map(Object::matrix) else { return Vec::new() };
    let to_active = model.inverse();
    objects
        .iter()
        .filter(|o| o.id != active && o.visible)
        .filter_map(|o| {
            let mesh = o.mesh.as_ref()?;
            let (scale, rotation, position) = (to_active * o.matrix()).to_scale_rotation_translation();
            Some(Other { id: ObjectId(FIRST_OTHER + o.id), name: &o.name, mesh, placement: Placement { position, rotation, scale } })
        })
        .collect()
}

fn find<'b, 'a>(others: &'b [Other<'a>], id: ObjectId) -> Option<&'b Other<'a>> {
    others.iter().find(|o| o.id == id)
}

pub fn info<'b>(others: &'b [Other<'_>], id: ObjectId) -> Option<ObjectInfo<'b>> {
    let o = find(others, id)?;
    Some(ObjectInfo {
        id,
        name: o.name,
        visible: true,
        active: false,
        mesh: MeshId(o.mesh.uid),
        placement: o.placement,
        mirror: None,
        live_copies: &[],
        vertices: o.mesh.vertex_count(),
        triangles: o.mesh.triangle_count(),
    })
}

pub fn mesh<'b>(others: &'b [Other<'_>], id: ObjectId) -> Option<MeshView<'b>> {
    let o = find(others, id)?;
    let m = o.mesh;
    Some(MeshView {
        id: MeshId(m.uid),
        positions: &m.positions,
        normals: &m.normals,
        indices: &m.indices,
        mask: &m.mask,
        hidden_triangles: m.hidden_triangles(),
        hidden_vertices: m.hidden_vertices(),
    })
}

/// Its box in the active object's space.
pub fn bounds(others: &[Other<'_>], id: ObjectId) -> sculpt_geom::Aabb {
    let Some(o) = find(others, id) else { return sculpt_geom::Aabb::EMPTY };
    let b = o.mesh.bounds();
    if b.is_empty() {
        return sculpt_geom::Aabb::EMPTY;
    }
    let m: Mat4 = o.placement.matrix();
    let mut out = sculpt_geom::Aabb::EMPTY;
    for i in 0..8 {
        let pick = |bit: usize, lo: f32, hi: f32| if i & bit == 0 { lo } else { hi };
        let corner = glam::Vec3::new(pick(1, b.min.x, b.max.x), pick(2, b.min.y, b.max.y), pick(4, b.min.z, b.max.z));
        let p = m.transform_point3(corner);
        out = sculpt_geom::Aabb::new(out.min.min(p), out.max.max(p));
    }
    out
}
