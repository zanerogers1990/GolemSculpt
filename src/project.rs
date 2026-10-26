//! Project Detail: the detail of one object put onto another of the same
//! form, the way ZBrush's Project All and Blender's Shrinkwrap carry a
//! sculpt onto new topology.
//!
//! The usual reason is clean topology. A sculpt grown with Remesh and dynamic
//! topology has triangles that follow nothing; Quad Remesh a copy of it,
//! Subdivide the copy a few times, and Project from the original after each
//! Subdivide: the copy takes on the sculpt's detail and keeps its own even
//! quads and levels underneath.
//!
//! **How each point finds its place.** A point of the target looks along its
//! own normal, outward and inward, for the source's surface, and goes to the
//! nearer of the two hits. Along the normal rather than to the nearest point,
//! because the nearest point bunches points up in every hollow and leaves
//! ridges bare; along the normal they keep the spacing they had, which is the
//! spacing the quads were made with. A hit counts only where the source faces
//! the same way as the point: the far side of a thin ear, or the inside of a
//! lip, is never taken for the near side. A point whose normal finds nothing
//! within reach (the very edge of a crease, say) goes to the nearest point of
//! the source facing its way, if that is within reach, and otherwise stays.
//!
//! **Reach** is how far a point may move, in the scene's units. Too little and
//! the deeper detail is missed; too much and a point may jump to a part of the
//! model it does not belong to (a finger to the next finger).
//!
//! The mask protects as it does from a brush: a fully masked point stays, a
//! half-masked one goes half way. Hidden points stay, as under any stroke.
//!
//! Both objects are taken where they stand in the scene, each with its own
//! position, rotation and scale. Nothing here changes a mesh: it hands back
//! the points to move and where to, and the app moves them as one stroke
//! (`app/project.rs`).

use glam::{Mat3, Mat4, Vec3};
use rayon::prelude::*;

use crate::mesh::Mesh;
use crate::mesh::bvh::nearest::closest_on_triangle;

/// A mesh to take detail from, and where its object stands in the scene.
pub struct Source<'a> {
    pub mesh: &'a Mesh,
    pub to_world: Mat4,
}

/// What a projection came to: the target points that move (in the target's
/// own space), and how many points looked and found nothing within reach.
#[derive(Default)]
pub struct Projected {
    pub indices: Vec<u32>,
    pub positions: Vec<Vec3>,
    pub missed: usize,
}

/// The matrix that carries normals the way `m` carries points.
fn normal_matrix(m: Mat4) -> Mat3 {
    Mat3::from_mat4(m).inverse().transpose()
}

/// A source made ready to be asked: its inverse and its normal matrix. A
/// placement that is a reflection needs nothing more: the inverse transpose
/// carries an outward normal to an outward normal, mirror or not.
struct Ready<'a> {
    mesh: &'a Mesh,
    to_world: Mat4,
    to_local: Mat4,
    normals: Mat3,
    /// The most the placement shrinks a length: what turns a reach in the
    /// scene into a reach in the mesh's own space that is surely long enough.
    least_scale: f32,
}

impl<'a> Ready<'a> {
    fn new(source: &Source<'a>) -> Ready<'a> {
        let (scale, _, _) = source.to_world.to_scale_rotation_translation();
        Ready {
            mesh: source.mesh,
            to_world: source.to_world,
            to_local: source.to_world.inverse(),
            normals: normal_matrix(source.to_world),
            least_scale: scale.abs().min_element().max(1e-12),
        }
    }

    /// The source's outward normal at `hit`, in the scene.
    fn outward(&self, normal: Vec3) -> Vec3 {
        (self.normals * normal).normalize_or_zero()
    }

    /// The nearest point of this source to `p` (in the scene) along `normal`
    /// either way, or failing that the nearest at all, that faces along
    /// `normal` and is within `reach`; with its distance.
    fn find(&self, p: Vec3, normal: Vec3, reach: f32) -> Option<(Vec3, f32)> {
        let origin = self.to_local.transform_point3(p);
        let along = self.to_local.transform_vector3(normal);
        let mut best: Option<(Vec3, f32)> = None;
        for dir in [along, -along] {
            let Some(hit) = self.mesh.raycast(origin, dir) else { continue };
            let at = self.to_world.transform_point3(hit.pos);
            let distance = at.distance(p);
            let faces = self.outward(self.mesh.hit_normal(&hit, false)).dot(normal) > 0.0;
            if distance <= reach && faces && best.is_none_or(|(_, d)| distance < d) {
                best = Some((at, distance));
            }
        }
        if best.is_some() {
            return best;
        }
        let local_reach = reach / self.least_scale;
        let hidden = self.mesh.hidden_triangles();
        let (positions, indices) = (&self.mesh.positions, &self.mesh.indices);
        let skip = |t: u32| hidden.is_some_and(|h| h[t as usize]);
        let (t, _) = self.mesh.bvh.nearest_triangle(positions, indices, origin, local_reach * local_reach, skip)?;
        let corner = |c: usize| positions[indices[t as usize * 3 + c] as usize];
        let (a, b, c) = (corner(0), corner(1), corner(2));
        let at = self.to_world.transform_point3(closest_on_triangle(origin, a, b, c));
        let distance = at.distance(p);
        let faces = self.outward((b - a).cross(c - a)).dot(normal) > 0.0;
        (distance <= reach && faces).then_some((at, distance))
    }
}

/// Projects `sources` onto `target`, whose object stands at `target_to_world`,
/// moving no point further than `reach` (in the scene's units). Fully masked
/// and hidden points are not asked about.
pub fn project(target: &Mesh, target_to_world: Mat4, sources: &[Source], reach: f32) -> Projected {
    let ready: Vec<Ready> = sources.iter().map(Ready::new).collect();
    let to_local = target_to_world.inverse();
    let normals = normal_matrix(target_to_world);
    let found: Vec<Option<Option<Vec3>>> = (0..target.vertex_count())
        .into_par_iter()
        .map(|v| {
            let free = 1.0 - target.mask[v].clamp(0.0, 1.0);
            if free <= 0.0 || target.is_vertex_hidden(v as u32) {
                return None;
            }
            let p = target_to_world.transform_point3(target.positions[v]);
            let n = (normals * target.normals[v]).normalize_or_zero();
            if n == Vec3::ZERO {
                return Some(None);
            }
            let nearest = ready
                .iter()
                .filter_map(|source| source.find(p, n, reach))
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(at, _)| to_local.transform_point3(p.lerp(at, free)));
            Some(nearest)
        })
        .collect();
    let mut out = Projected::default();
    for (v, found) in found.into_iter().enumerate() {
        match found {
            Some(Some(at)) => {
                out.indices.push(v as u32);
                out.positions.push(at);
            }
            Some(None) => out.missed += 1,
            None => {}
        }
    }
    out
}

/// The reach a projection starts at, as a share of the target's size: enough
/// for the gap between a sculpt and a subdivided quad remesh of it.
pub const DEFAULT_REACH: f32 = 0.02;
/// The range the Reach slider offers, as shares of the target's size.
pub const REACH_RANGE: std::ops::RangeInclusive<f32> = 0.001..=0.25;

#[cfg(test)]
mod tests;
