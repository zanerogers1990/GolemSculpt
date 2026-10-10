//! The two halves of a cut as the key code sees them, and the joints between
//! them: which cut faces mate, and the one direction each joint pulls apart
//! in.

use std::collections::HashMap;

use glam::{Mat4, Vec2, Vec3};
use rayon::prelude::*;
use sculpt_geom::TriMesh;
use sculpt_geom::adjacency::connected_components;

use crate::types::any_perpendicular;

/// One half of a cut: its geometry, where it is in the world, and the cut
/// faces on it that mate with the other half.
///
/// Keys are sized in real millimetres, so they are planned in world space;
/// the part keeps its vertices there too, worked out once.
#[derive(Clone, Debug, Default)]
pub struct KeyPart {
    mesh: TriMesh,
    local_to_world: Mat4,
    world: Vec<Vec3>,
    /// The triangles of each cut face. `split` reports them for a fresh cut;
    /// `mating::find` works them out for any two parts that touch.
    pub cut_faces: Vec<Vec<u32>>,
}

impl KeyPart {
    /// `mesh` is in the part's own space, as the object holds it.
    pub fn new(mesh: TriMesh, local_to_world: Mat4, cut_faces: Vec<Vec<u32>>) -> KeyPart {
        let world = mesh.positions.par_iter().with_min_len(4096).map(|p| local_to_world.transform_point3(*p)).collect();
        KeyPart { mesh, local_to_world, world, cut_faces }
    }

    /// The part's own geometry, in its own space.
    pub fn mesh(&self) -> &TriMesh {
        &self.mesh
    }

    pub fn local_to_world(&self) -> Mat4 {
        self.local_to_world
    }

    /// Vertex positions in world space.
    pub fn world(&self) -> &[Vec3] {
        &self.world
    }

    pub fn indices(&self) -> &[u32] {
        &self.mesh.indices
    }

    pub fn triangle_count(&self) -> usize {
        self.mesh.triangle_count()
    }

    /// The world corners of triangle `t`.
    #[inline]
    pub fn corners(&self, t: u32) -> [Vec3; 3] {
        let i = &self.mesh.indices[t as usize * 3..t as usize * 3 + 3];
        [self.world[i[0] as usize], self.world[i[1] as usize], self.world[i[2] as usize]]
    }
}

/// The two halves of one separable joint: the cut faces on A that mate with
/// cut faces on B, and the one direction the halves pull apart in. Keys on a
/// joint all share that axis, or the parts could not be put together.
///
/// A cut through one thing makes one joint. A single cut through TWO things
/// (both legs of a figure) makes one joint per leg: each foot comes away from
/// the body on its own, and each needs its own keys. The faces are grouped by
/// which connected piece of each half they are on, which is exactly that.
#[derive(Clone, Debug, Default)]
pub struct KeyJoint {
    /// Indices into `a.cut_faces` and `b.cut_faces`, pair by pair.
    pub faces_a: Vec<usize>,
    pub faces_b: Vec<usize>,
    /// World space. `axis` points out of A's faces, into B.
    pub origin: Vec3,
    pub axis: Vec3,
    pub u: Vec3,
    pub v: Vec3,
    pub area: f32,
    /// How far the faces' own normals stray from `axis` at most, in degrees:
    /// large when one joint's faces point different ways (the halves may not
    /// come apart along any line).
    pub spread: f32,
    /// Mean edge length of the faces: the density keys are built at.
    pub edge: f32,
}

impl KeyJoint {
    /// Groups mating face pairs into joints and gives each its frame.
    pub fn group(a: &KeyPart, b: &KeyPart, pairs: &[(usize, usize)]) -> Vec<KeyJoint> {
        let piece_a = connected_components(a.mesh.vertex_count(), &a.mesh.indices).vertex_label;
        let piece_b = connected_components(b.mesh.vertex_count(), &b.mesh.indices).vertex_label;
        let mut by_pieces: HashMap<(u32, u32), usize> = HashMap::new();
        let mut joints: Vec<KeyJoint> = Vec::new();
        for &(fa, fb) in pairs {
            let (Some(face_a), Some(face_b)) = (a.cut_faces.get(fa), b.cut_faces.get(fb)) else { continue };
            let (Some(&ta), Some(&tb)) = (face_a.first(), face_b.first()) else { continue };
            let key = (
                piece_a[a.mesh.indices[ta as usize * 3] as usize],
                piece_b[b.mesh.indices[tb as usize * 3] as usize],
            );
            let index = *by_pieces.entry(key).or_insert_with(|| {
                joints.push(KeyJoint::default());
                joints.len() - 1
            });
            joints[index].faces_a.push(fa);
            joints[index].faces_b.push(fb);
        }
        for joint in &mut joints {
            joint.build_frame(a);
        }
        joints
    }

    fn triangles<'a>(&'a self, a: &'a KeyPart) -> impl Iterator<Item = [Vec3; 3]> + 'a {
        self.faces_a.iter().flat_map(|&f| a.cut_faces[f].iter()).map(|&t| a.corners(t))
    }

    fn build_frame(&mut self, a: &KeyPart) {
        let (mut normal_sum, mut centre) = (Vec3::ZERO, Vec3::ZERO);
        let (mut area, mut edges) = (0.0f64, 0.0f64);
        let mut count = 0usize;
        for [p0, p1, p2] in self.triangles(a) {
            let cross = (p1 - p0).cross(p2 - p0);
            let ar = cross.length() * 0.5;
            normal_sum += cross;
            centre += (p0 + p1 + p2) * (ar / 3.0);
            area += ar as f64;
            edges += (p0.distance(p1) + p1.distance(p2) + p2.distance(p0)) as f64;
            count += 1;
        }
        self.area = area as f32;
        self.origin = if area > 0.0 { centre / area as f32 } else { Vec3::ZERO };
        self.axis = normal_sum.normalize_or(Vec3::Y);
        self.edge = if count > 0 { (edges / (count * 3) as f64) as f32 } else { 0.0 };

        // Principal direction of the faces' area in their plane: keys line up
        // along it, and two keys spread along it.
        let u0 = any_perpendicular(self.axis);
        let v0 = self.axis.cross(u0).normalize();
        let (mut sxx, mut syy, mut sxy) = (0.0f64, 0.0f64, 0.0f64);
        let mut worst = 1.0f32;
        // Slivers carry no area and their normals are float noise: a dense
        // cut face has a few with ten-millionths of a normal triangle's area
        // (where crossings bunch at the rim), pointing anywhere. They must not
        // decide which way the face faces.
        let sliver = if count > 0 { (area / count as f64) as f32 * 1e-3 } else { 0.0 };
        for [p0, p1, p2] in self.triangles(a) {
            let cross = (p1 - p0).cross(p2 - p0);
            let ar = cross.length() * 0.5;
            if ar <= sliver {
                continue;
            }
            worst = worst.min((cross / (2.0 * ar)).dot(self.axis));
            let d = (p0 + p1 + p2) / 3.0 - self.origin;
            let (x, y) = (d.dot(u0) as f64, d.dot(v0) as f64);
            let ar = ar as f64;
            sxx += ar * x * x;
            syy += ar * y * y;
            sxy += ar * x * y;
        }
        let angle = 0.5 * (2.0 * sxy).atan2(sxx - syy);
        self.u = (u0 * angle.cos() as f32 + v0 * angle.sin() as f32).normalize();
        self.v = self.axis.cross(self.u).normalize();
        self.spread = worst.clamp(-1.0, 1.0).acos().to_degrees();
    }

    /// A world point in the joint's own plane: x along `u`, y along `v`.
    #[inline]
    pub fn to_plane(&self, world: Vec3) -> Vec2 {
        let d = world - self.origin;
        Vec2::new(d.dot(self.u), d.dot(self.v))
    }

    /// As `to_plane`, with the height along the axis in z.
    #[inline]
    pub fn to_local(&self, world: Vec3) -> Vec3 {
        let d = world - self.origin;
        Vec3::new(d.dot(self.u), d.dot(self.v), d.dot(self.axis))
    }

    /// The world point at plane position `p`, `height` along the axis.
    #[inline]
    pub fn from_plane(&self, p: Vec2, height: f32) -> Vec3 {
        self.origin + self.u * p.x + self.v * p.y + self.axis * height
    }
}
