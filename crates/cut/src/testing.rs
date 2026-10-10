//! Shapes, cameras and checks the tests share. Compiled for tests only.

use std::collections::HashMap;

use glam::{Mat4, Quat, Vec2, Vec3, Vec4};
use rayon::prelude::*;
use sculpt_geom::TriMesh;

use crate::region::CutView;

/// A small repeatable random source, so a failing test fails the same way twice.
pub(crate) struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    /// Uniform in 0..1.
    pub fn unit(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }

    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
}

// The cameras below are the app's own (see `src/camera.rs`): right-handed, the
// view looking down -Z, reverse depth, and pixel (0, 0) at the top-left.

fn camera_basis(yaw_deg: f32, pitch_deg: f32) -> (Vec3, Vec3, Vec3) {
    let rot = Quat::from_rotation_y(-yaw_deg.to_radians()) * Quat::from_rotation_x(-pitch_deg.to_radians());
    (rot * Vec3::X, rot * Vec3::Y, rot * Vec3::NEG_Z)
}

fn look_to(eye: Vec3, right: Vec3, up: Vec3, forward: Vec3) -> Mat4 {
    Mat4::from_cols(
        Vec4::new(right.x, up.x, -forward.x, 0.0),
        Vec4::new(right.y, up.y, -forward.y, 0.0),
        Vec4::new(right.z, up.z, -forward.z, 0.0),
        Vec4::new(-right.dot(eye), -up.dot(eye), forward.dot(eye), 1.0),
    )
}

/// An orthographic camera looking at `target`, showing `half_height` world
/// units above and below it.
pub(crate) fn ortho_view(target: Vec3, yaw: f32, pitch: f32, half_height: f32, origin: Vec2, size: Vec2) -> CutView {
    let (right, up, forward) = camera_basis(yaw, pitch);
    let view = look_to(target - forward * 50.0, right, up, forward);
    let (near, far) = (0.1, 1000.0);
    let range = far - near;
    let proj = Mat4::from_cols(
        Vec4::new(1.0 / (half_height * size.x / size.y), 0.0, 0.0, 0.0),
        Vec4::new(0.0, 1.0 / half_height, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 1.0 / range, 0.0),
        Vec4::new(0.0, 0.0, far / range, 1.0),
    );
    CutView::new(proj * view, view, origin, size)
}

/// A 60 degree perspective camera `distance` away from `target`.
pub(crate) fn perspective_view(target: Vec3, yaw: f32, pitch: f32, distance: f32, origin: Vec2, size: Vec2) -> CutView {
    let (right, up, forward) = camera_basis(yaw, pitch);
    let view = look_to(target - forward * distance, right, up, forward);
    let f = 1.0 / 30f32.to_radians().tan();
    let proj = Mat4::from_cols(
        Vec4::new(f / (size.x / size.y), 0.0, 0.0, 0.0),
        Vec4::new(0.0, f, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 0.0, -1.0),
        Vec4::new(0.0, 0.0, 0.01, 0.0),
    );
    CutView::new(proj * view, view, origin, size)
}

/// Turns every triangle of a shape that is star-shaped about the origin to
/// face outward.
fn wind_outward(mesh: &mut TriMesh) {
    let positions = &mesh.positions;
    for t in mesh.indices.chunks_exact_mut(3) {
        let [a, b, c] = [t[0], t[1], t[2]].map(|i| positions[i as usize]);
        if (b - a).cross(c - a).dot(a + b + c) < 0.0 {
            t.swap(1, 2);
        }
    }
}

/// A unit sphere from an icosahedron split `level` times: 20 x 4^level
/// triangles of nearly equal size, with no poles.
pub(crate) fn icosphere(level: u32) -> TriMesh {
    let phi = (1.0 + 5f32.sqrt()) * 0.5;
    let mut positions: Vec<Vec3> = [
        (-1.0, phi, 0.0),
        (1.0, phi, 0.0),
        (-1.0, -phi, 0.0),
        (1.0, -phi, 0.0),
        (0.0, -1.0, phi),
        (0.0, 1.0, phi),
        (0.0, -1.0, -phi),
        (0.0, 1.0, -phi),
        (phi, 0.0, -1.0),
        (phi, 0.0, 1.0),
        (-phi, 0.0, -1.0),
        (-phi, 0.0, 1.0),
    ]
    .iter()
    .map(|&(x, y, z)| Vec3::new(x, y, z).normalize())
    .collect();
    let mut indices: Vec<u32> = vec![
        0, 11, 5, 0, 5, 1, 0, 1, 7, 0, 7, 10, 0, 10, 11, 1, 5, 9, 5, 11, 4, 11, 10, 2, 10, 7, 6, 7, 1, 8, 3, 9, 4, 3,
        4, 2, 3, 2, 6, 3, 6, 8, 3, 8, 9, 4, 9, 5, 2, 4, 11, 6, 2, 10, 8, 6, 7, 9, 8, 1,
    ];
    for _ in 0..level {
        let mut midpoints: HashMap<(u32, u32), u32> = HashMap::with_capacity(indices.len());
        let mut next = Vec::with_capacity(indices.len() * 4);
        for t in indices.chunks_exact(3) {
            let mut mid = [0u32; 3];
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                mid[k] = *midpoints.entry((a.min(b), a.max(b))).or_insert_with(|| {
                    positions.push(((positions[a as usize] + positions[b as usize]) * 0.5).normalize());
                    positions.len() as u32 - 1
                });
            }
            next.extend_from_slice(&[t[0], mid[0], mid[2], t[1], mid[1], mid[0], t[2], mid[2], mid[1]]);
            next.extend_from_slice(&mid);
        }
        indices = next;
    }
    let mut mesh = TriMesh::new(positions, indices);
    wind_outward(&mut mesh);
    mesh
}

/// The cube from -1 to 1 with every face an `n` x `n` grid, welded along its
/// edges: 12 n^2 triangles.
pub(crate) fn cube(n: u32) -> TriMesh {
    let mut lattice: HashMap<[u32; 3], u32> = HashMap::new();
    let mut positions = Vec::new();
    let mut indices = Vec::new();
    let mut vertex = |at: [u32; 3]| -> u32 {
        *lattice.entry(at).or_insert_with(|| {
            positions.push(Vec3::new(at[0] as f32, at[1] as f32, at[2] as f32) * (2.0 / n as f32) - Vec3::ONE);
            positions.len() as u32 - 1
        })
    };
    for axis in 0..3 {
        for side in [0, n] {
            for i in 0..n {
                for j in 0..n {
                    let corner = |di: u32, dj: u32| {
                        let mut at = [0u32; 3];
                        at[axis] = side;
                        at[(axis + 1) % 3] = i + di;
                        at[(axis + 2) % 3] = j + dj;
                        at
                    };
                    let q = [vertex(corner(0, 0)), vertex(corner(1, 0)), vertex(corner(1, 1)), vertex(corner(0, 1))];
                    indices.extend_from_slice(&[q[0], q[1], q[2], q[0], q[2], q[3]]);
                }
            }
        }
    }
    let mut mesh = TriMesh::new(positions, indices);
    wind_outward(&mut mesh);
    mesh
}

/// The grid cube blown out onto the unit sphere: an even sphere with any
/// number of triangles (12 n^2).
pub(crate) fn cube_sphere(n: u32) -> TriMesh {
    let mut mesh = cube(n);
    for p in &mut mesh.positions {
        *p = p.normalize();
    }
    mesh
}

/// A torus around the y axis: `nu` steps round the ring, `nv` round the tube.
pub(crate) fn torus(major: f32, minor: f32, nu: u32, nv: u32) -> TriMesh {
    let tau = std::f32::consts::TAU;
    let positions: Vec<Vec3> = (0..nu * nv)
        .into_par_iter()
        .map(|i| {
            let (u, v) = ((i / nv) as f32 / nu as f32 * tau, (i % nv) as f32 / nv as f32 * tau);
            let ring = major + minor * v.cos();
            Vec3::new(ring * u.cos(), minor * v.sin(), ring * u.sin())
        })
        .collect();
    let indices: Vec<u32> = (0..nu * nv)
        .into_par_iter()
        .flat_map_iter(|i| {
            let (iu, iv) = (i / nv, i % nv);
            let at = |du: u32, dv: u32| ((iu + du) % nu) * nv + (iv + dv) % nv;
            [at(0, 0), at(0, 1), at(1, 1), at(0, 0), at(1, 1), at(1, 0)]
        })
        .collect();
    let mut mesh = TriMesh::new(positions, indices);
    if mesh.signed_volume() < 0.0 {
        mesh.flip();
    }
    mesh
}

/// Why a mesh is not a closed, consistently wound surface, or `None` when it
/// is: every edge walked exactly once each way.
pub(crate) fn closed_error(mesh: &TriMesh) -> Option<String> {
    if !mesh.indices.len().is_multiple_of(3) {
        return Some("the index count is not a multiple of three".into());
    }
    if let Some(&i) = mesh.indices.iter().find(|&&i| i as usize >= mesh.positions.len()) {
        return Some(format!("index {i} is past the last vertex"));
    }
    if mesh.indices.chunks_exact(3).any(|t| t[0] == t[1] || t[1] == t[2] || t[2] == t[0]) {
        return Some("a triangle uses one vertex twice".into());
    }
    let mut directed: Vec<u64> = mesh
        .indices
        .par_chunks_exact(3)
        .flat_map_iter(|t| (0..3).map(move |k| ((t[k] as u64) << 32) | t[(k + 1) % 3] as u64))
        .collect();
    directed.par_sort_unstable();
    if directed.windows(2).any(|w| w[0] == w[1]) {
        return Some("an edge is used twice the same way".into());
    }
    let open = directed.par_iter().filter(|&&e| directed.binary_search(&e.rotate_left(32)).is_err()).count();
    (open > 0).then(|| format!("{open} open edges"))
}

#[track_caller]
pub(crate) fn assert_closed(mesh: &TriMesh, what: &str) {
    if let Some(problem) = closed_error(mesh) {
        panic!("{what}: {problem}");
    }
    assert!(mesh.signed_volume() > 0.0, "{what}: wound inside out");
}

#[track_caller]
pub(crate) fn assert_near(actual: f64, expected: f64, relative: f64, what: &str) {
    assert!(
        (actual - expected).abs() <= expected.abs() * relative,
        "{what}: {actual} is not within {}% of {expected}",
        relative * 100.0
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shapes_are_closed_and_the_right_size() {
        let sphere = icosphere(4);
        assert_eq!(sphere.triangle_count(), 20 * 256);
        assert_closed(&sphere, "icosphere");
        assert_near(sphere.signed_volume(), 4.0 / 3.0 * std::f64::consts::PI, 0.01, "icosphere volume");

        let cube = cube(6);
        assert_eq!((cube.triangle_count(), cube.vertex_count()), (12 * 36, 6 * 36 + 2));
        assert_closed(&cube, "cube");
        assert_near(cube.signed_volume(), 8.0, 1e-6, "cube volume");
        assert_closed(&cube_sphere(8), "cube sphere");

        let ring = torus(2.0, 0.5, 64, 32);
        assert_closed(&ring, "torus");
        assert_near(ring.signed_volume(), 2.0 * std::f64::consts::PI.powi(2) * 2.0 * 0.25, 0.01, "torus volume");
    }

    #[test]
    fn the_closed_check_sees_holes_and_flips() {
        let mut open = cube(2);
        open.indices.truncate(open.indices.len() - 3);
        assert!(closed_error(&open).unwrap().contains("open"));
        let mut flipped = cube(2);
        flipped.indices.swap(1, 2);
        assert!(closed_error(&flipped).unwrap().contains("twice"));
    }
}
