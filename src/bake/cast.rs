//! Finding the sculpt from a point of the low model, and asking whether a
//! ray from the sculpt runs into it again.
//!
//! The finding is Project Detail's rule (`project.rs`, which says why it is
//! along the normal and why only a surface facing the same way counts): look
//! along the low normal outward and inward, take the nearer hit within reach
//! whose surface faces the way the low normal does; failing that, the nearest
//! point within reach that faces that way. It is written again here rather
//! than borrowed because the baker needs more of the answer than a place: the
//! triangle and the weights in it, which the smooth normal and the colour are
//! read from. It is built from the same parts (`Mesh::raycast`,
//! `Mesh::hit_normal`, `Bvh::nearest_triangle`, `closest_on_triangle`), so
//! hidden faces are passed through here exactly as there.
//!
//! Every source is asked in its own space: the point and direction go in
//! through its inverse placement and the answer comes out through its
//! placement, normals through the inverse transpose, which keeps them square
//! to a surface that has been stretched and outward on a reflected copy.

use glam::{Mat3, Mat4, Vec3};

use super::{Source, normal_matrix};
use crate::mesh::Mesh;
use crate::mesh::bvh::Hit;
use crate::mesh::bvh::nearest::closest_on_triangle;

/// A source made ready to be asked.
pub(super) struct Ready<'a> {
    mesh: &'a Mesh,
    to_world: Mat4,
    to_local: Mat4,
    normals: Mat3,
    /// The most the placement shrinks a length: what turns a reach in the
    /// scene into a reach in the mesh's own space that is surely long enough.
    least_scale: f32,
    colours: Option<&'a [[u8; 4]]>,
}

/// What a point of the low model found on the sculpt.
pub(super) struct Found {
    /// Which source, its triangle, and the weights of the triangle's corners.
    pub source: usize,
    pub tri: u32,
    pub bary: [f32; 3],
    /// Where, in the scene.
    pub pos: Vec3,
    /// The sculpt's smooth normal there, in the scene, unit length.
    pub normal: Vec3,
    /// The flat normal of the triangle found, outward, unit length.
    pub face: Vec3,
    pub distance: f32,
}

impl<'a> Ready<'a> {
    pub(super) fn new(source: &Source<'a>) -> Ready<'a> {
        let (scale, _, _) = source.to_world.to_scale_rotation_translation();
        Ready {
            mesh: source.mesh,
            to_world: source.to_world,
            to_local: source.to_world.inverse(),
            normals: normal_matrix(source.to_world),
            least_scale: scale.abs().min_element().max(1e-12),
            colours: source.colours,
        }
    }

    /// The corners of the box around this source, in the scene.
    pub(super) fn world_bounds(&self) -> (Vec3, Vec3) {
        let b = self.mesh.bounds();
        let (mut lo, mut hi) = (Vec3::INFINITY, Vec3::NEG_INFINITY);
        for k in 0..8 {
            let corner = Vec3::new(
                if k & 1 == 0 { b.min.x } else { b.max.x },
                if k & 2 == 0 { b.min.y } else { b.max.y },
                if k & 4 == 0 { b.min.z } else { b.max.z },
            );
            let at = self.to_world.transform_point3(corner);
            (lo, hi) = (lo.min(at), hi.max(at));
        }
        (lo, hi)
    }

    /// A normal of the mesh, in the scene.
    fn outward(&self, normal: Vec3) -> Vec3 {
        (self.normals * normal).normalize_or_zero()
    }

    /// The flat outward normal of triangle `tri`, in the mesh's own space.
    fn face_normal(&self, tri: u32) -> Vec3 {
        let [a, b, c] = self.corners(tri);
        (b - a).cross(c - a)
    }

    fn corners(&self, tri: u32) -> [Vec3; 3] {
        let i = tri as usize * 3;
        [0, 1, 2].map(|k| self.mesh.positions[self.mesh.indices[i + k] as usize])
    }

    /// The smooth normal at `bary` in triangle `tri`, in the mesh's own space.
    fn smooth_normal(&self, tri: u32, bary: [f32; 3]) -> Vec3 {
        let i = tri as usize * 3;
        let mut n = Vec3::ZERO;
        for (k, w) in bary.iter().enumerate() {
            n += self.mesh.normals[self.mesh.indices[i + k] as usize] * *w;
        }
        n
    }

    /// The paint at `bary` in triangle `tri`, if this source is painted.
    pub(super) fn colour(&self, tri: u32, bary: [f32; 3]) -> Option<[u8; 4]> {
        let colours = self.colours?;
        let i = tri as usize * 3;
        let mut sum = [0.0f32; 4];
        for (k, w) in bary.iter().enumerate() {
            let c = colours[self.mesh.indices[i + k] as usize];
            for (s, c) in sum.iter_mut().zip(c) {
                *s += c as f32 * w;
            }
        }
        Some(sum.map(|s| s.round().clamp(0.0, 255.0) as u8))
    }

    /// What this source has to show for a point `p` of the low model whose
    /// normal is `normal` (both in the scene), within `reach`.
    fn find(&self, p: Vec3, normal: Vec3, reach: f32) -> Option<Found> {
        let origin = self.to_local.transform_point3(p);
        let along = self.to_local.transform_vector3(normal);
        let mut best: Option<Found> = None;
        for dir in [along, -along] {
            let Some(hit) = self.mesh.raycast(origin, dir) else { continue };
            let at = self.to_world.transform_point3(hit.pos);
            let distance = at.distance(p);
            if distance > reach || best.as_ref().is_some_and(|b| b.distance <= distance) {
                continue;
            }
            let smooth = self.outward(self.mesh.hit_normal(&hit, false));
            if smooth.dot(normal) > 0.0 {
                best = Some(self.found(&hit, at, smooth, distance));
            }
        }
        if best.is_some() {
            return best;
        }
        let local_reach = reach / self.least_scale;
        let hidden = self.mesh.hidden_triangles();
        let skip = |t: u32| hidden.is_some_and(|h| h[t as usize]);
        let (positions, indices) = (&self.mesh.positions, &self.mesh.indices);
        let (tri, _) = self.mesh.bvh.nearest_triangle(positions, indices, origin, local_reach * local_reach, skip)?;
        let [a, b, c] = self.corners(tri);
        let local = closest_on_triangle(origin, a, b, c);
        let at = self.to_world.transform_point3(local);
        let distance = at.distance(p);
        let face = self.outward(self.face_normal(tri));
        if distance > reach || face.dot(normal) <= 0.0 {
            return None;
        }
        let bary = weights(local, a, b, c);
        let smooth = self.outward(self.smooth_normal(tri, bary));
        let smooth = if smooth == Vec3::ZERO { face } else { smooth };
        Some(Found { source: 0, tri, bary, pos: at, normal: smooth, face, distance })
    }

    fn found(&self, hit: &Hit, at: Vec3, smooth: Vec3, distance: f32) -> Found {
        let face = self.outward(self.face_normal(hit.tri));
        Found { source: 0, tri: hit.tri, bary: hit.bary, pos: at, normal: smooth, face, distance }
    }

    /// Whether a ray from `origin` along the unit direction `dir` (in the
    /// scene) meets this source closer than `within`.
    fn blocks(&self, origin: Vec3, dir: Vec3, within: f32) -> bool {
        let from = self.to_local.transform_point3(origin);
        let along = self.to_local.transform_vector3(dir);
        self.mesh
            .raycast(from, along)
            .is_some_and(|hit| self.to_world.transform_point3(hit.pos).distance(origin) < within)
    }
}

/// The nearest of what `sources` have to show for the low model's point `p`
/// with normal `normal`, within `reach`.
pub(super) fn find(sources: &[Ready], p: Vec3, normal: Vec3, reach: f32) -> Option<Found> {
    let mut best: Option<Found> = None;
    for (i, source) in sources.iter().enumerate() {
        if let Some(found) = source.find(p, normal, reach)
            && best.as_ref().is_none_or(|b| found.distance < b.distance)
        {
            best = Some(Found { source: i, ..found });
        }
    }
    best
}

/// Whether a ray from `origin` along the unit direction `dir` meets any of
/// `sources` closer than `within`.
pub(super) fn blocked(sources: &[Ready], origin: Vec3, dir: Vec3, within: f32) -> bool {
    sources.iter().any(|s| s.blocks(origin, dir, within))
}

/// The weights of the corners `a b c` at `p`, a point of the triangle.
fn weights(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> [f32; 3] {
    let (e1, e2, d) = (b - a, c - a, p - a);
    let (d11, d12, d22) = (e1.dot(e1), e1.dot(e2), e2.dot(e2));
    let (dp1, dp2) = (d.dot(e1), d.dot(e2));
    let den = d11 * d22 - d12 * d12;
    if den.abs() < 1e-30 {
        return [1.0, 0.0, 0.0];
    }
    let v = ((d22 * dp1 - d12 * dp2) / den).clamp(0.0, 1.0);
    let w = ((d11 * dp2 - d12 * dp1) / den).clamp(0.0, 1.0);
    let sum = v + w;
    let (v, w) = if sum > 1.0 { (v / sum, w / sum) } else { (v, w) };
    [1.0 - v - w, v, w]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_weights_of_a_point_give_the_point_back() {
        let (a, b, c) = (Vec3::new(0.0, 0.0, 1.0), Vec3::new(2.0, 0.5, 0.0), Vec3::new(0.3, 1.5, 0.2));
        for w in [[1.0, 0.0, 0.0], [0.2, 0.3, 0.5], [0.0, 0.5, 0.5], [0.6, 0.0, 0.4]] {
            let p = a * w[0] + b * w[1] + c * w[2];
            let got = weights(p, a, b, c);
            for k in 0..3 {
                assert!((got[k] - w[k]).abs() < 1e-5, "{got:?} against {w:?}");
            }
        }
    }
}
