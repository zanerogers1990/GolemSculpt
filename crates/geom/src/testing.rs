//! Small meshes and a seeded generator the tests of this crate share.

use glam::Vec3;

use crate::mesh::TriMesh;

/// A small deterministic generator, so tests fire the same rays and build the
/// same random meshes every run.
pub struct Rng(pub u64);

impl Rng {
    /// Uniform in [0, 1).
    pub fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform in `0..n`.
    pub fn below(&mut self, n: usize) -> usize {
        ((self.next() * n as f64) as usize).min(n - 1)
    }
}

/// Winds every triangle counter-clockwise seen from outside, for a mesh that
/// encloses the origin star-shaped: a triangle faces outward when its normal
/// points the way its centroid does.
pub fn outward(mut mesh: TriMesh) -> TriMesh {
    for t in 0..mesh.triangle_count() {
        let [a, b, c] = mesh.triangle(t);
        if (b - a).cross(c - a).dot(a + b + c) < 0.0 {
            mesh.indices.swap(t * 3 + 1, t * 3 + 2);
        }
    }
    mesh
}

/// Four triangles, four vertices, volume 1/6.
pub fn tetrahedron() -> TriMesh {
    let p = vec![Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::Z];
    TriMesh::new(p, vec![0, 2, 1, 0, 1, 3, 0, 3, 2, 1, 2, 3])
}

/// The cube [-1, 1]^3: 8 vertices, 12 triangles, volume 8.
pub fn cube() -> TriMesh {
    let positions: Vec<Vec3> = (0..8u32)
        .map(|i| Vec3::new(sign(i & 1), sign(i >> 1 & 1), sign(i >> 2 & 1)))
        .collect();
    let quads = [[0, 1, 3, 2], [4, 5, 7, 6], [0, 1, 5, 4], [2, 3, 7, 6], [0, 2, 6, 4], [1, 3, 7, 5]];
    let indices = quads.iter().flat_map(|q| [q[0], q[1], q[2], q[0], q[2], q[3]]).collect();
    outward(TriMesh::new(positions, indices))
}

fn sign(bit: u32) -> f32 {
    if bit == 0 { -1.0 } else { 1.0 }
}

/// A closed sphere of radius 1 with one vertex at each pole: `2 + (rings - 1) * segs`
/// vertices and `2 * segs * (rings - 1)` triangles.
pub fn uv_sphere(rings: usize, segs: usize) -> TriMesh {
    let mut positions = vec![Vec3::Y, -Vec3::Y];
    for r in 1..rings {
        let polar = std::f64::consts::PI * r as f64 / rings as f64;
        for j in 0..segs {
            let azimuth = std::f64::consts::TAU * j as f64 / segs as f64;
            positions.push(Vec3::new(
                (polar.sin() * azimuth.cos()) as f32,
                polar.cos() as f32,
                (polar.sin() * azimuth.sin()) as f32,
            ));
        }
    }
    let ring = |r: usize, j: usize| (2 + (r - 1) * segs + j % segs) as u32;
    let mut indices = Vec::new();
    for j in 0..segs {
        indices.extend_from_slice(&[0, ring(1, j), ring(1, j + 1)]);
        indices.extend_from_slice(&[1, ring(rings - 1, j + 1), ring(rings - 1, j)]);
    }
    for r in 1..rings - 1 {
        for j in 0..segs {
            let (a, b, c, d) = (ring(r, j), ring(r, j + 1), ring(r + 1, j + 1), ring(r + 1, j));
            indices.extend_from_slice(&[a, d, b, b, d, c]);
        }
    }
    outward(TriMesh::new(positions, indices))
}

/// An open sheet in the XY plane of `nx * ny` cells (two triangles each), wound
/// counter-clockwise seen from +Z, with unit cells from the origin.
pub fn grid(nx: usize, ny: usize) -> TriMesh {
    let vertex = |i: usize, j: usize| (j * (nx + 1) + i) as u32;
    let positions = (0..=ny).flat_map(|j| (0..=nx).map(move |i| Vec3::new(i as f32, j as f32, 0.0))).collect();
    let mut indices = Vec::new();
    for j in 0..ny {
        for i in 0..nx {
            let (a, b, c, d) = (vertex(i, j), vertex(i + 1, j), vertex(i + 1, j + 1), vertex(i, j + 1));
            indices.extend_from_slice(&[a, b, c, a, c, d]);
        }
    }
    TriMesh::new(positions, indices)
}

/// `count` triangles over `vertices` vertices with random corners: repeated
/// corners (degenerate triangles), shared edges of three or more triangles and
/// unreferenced vertices all turn up.
pub fn random_soup(vertices: usize, count: usize, rng: &mut Rng) -> TriMesh {
    let positions = (0..vertices).map(|_| Vec3::new(rng.next() as f32, rng.next() as f32, rng.next() as f32)).collect();
    let indices = (0..count * 3).map(|_| rng.below(vertices) as u32).collect();
    TriMesh::new(positions, indices)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shared_meshes_enclose_what_they_should() {
        assert!((tetrahedron().signed_volume() - 1.0 / 6.0).abs() < 1e-9);
        assert!((cube().signed_volume() - 8.0).abs() < 1e-9);
        let sphere = uv_sphere(32, 64);
        assert_eq!(sphere.vertex_count(), 2 + 31 * 64);
        assert_eq!(sphere.triangle_count(), 2 * 31 * 64);
        let volume = sphere.signed_volume();
        assert!(volume > 4.1 && volume < 4.1888, "{volume}");
        assert_eq!(grid(3, 2).triangle_count(), 12);
    }
}
