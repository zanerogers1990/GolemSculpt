//! Helpers shared by the tests: small meshes, a seeded random source, and
//! rotation checks.

use glam::{Quat, Vec3};
use sculpt_geom::TriMesh;

use crate::types::ArrayPlacement;

/// A closed sphere of `lat` bands and `lon` columns, wound counter-clockwise
/// seen from outside. The polar rows are degenerate triangles, which hold no
/// volume.
pub fn uv_sphere(lat: usize, lon: usize, radius: f32) -> TriMesh {
    let mut positions = Vec::with_capacity((lat + 1) * lon);
    for i in 0..=lat {
        let theta = std::f32::consts::PI * i as f32 / lat as f32;
        for j in 0..lon {
            let phi = std::f32::consts::TAU * j as f32 / lon as f32;
            positions.push(Vec3::new(theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin()) * radius);
        }
    }
    let mut indices = Vec::with_capacity(lat * lon * 6);
    for i in 0..lat {
        for j in 0..lon {
            let a = (i * lon + j) as u32;
            let b = (i * lon + (j + 1) % lon) as u32;
            let c = ((i + 1) * lon + j) as u32;
            let d = ((i + 1) * lon + (j + 1) % lon) as u32;
            indices.extend_from_slice(&[a, b, c, b, d, c]);
        }
    }
    TriMesh::new(positions, indices)
}

/// The corner of a unit cube: a closed mesh wound outward, volume 1/6.
pub fn tetra() -> TriMesh {
    let p = vec![Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::Z];
    TriMesh::new(p, vec![0, 2, 1, 0, 1, 3, 0, 3, 2, 1, 2, 3])
}

/// The angle a rotation turns by, in degrees (0 to 180). Through the vector part
/// rather than `acos(w)`, which near zero turns float rounding in `w` into hundredths of a degree.
pub fn angle_deg(q: Quat) -> f64 {
    let vector = (q.x as f64).hypot(q.y as f64).hypot(q.z as f64);
    2.0 * vector.atan2(q.w.abs() as f64).to_degrees()
}

pub fn is_unit(q: Quat) -> bool {
    (q.length() as f64 - 1.0).abs() < 1e-4
}

pub fn is_finite(p: &ArrayPlacement) -> bool {
    p.offset.is_finite() && p.rotation.is_finite() && p.scale.is_finite()
}

/// A small deterministic random source, so a failing fuzz run can be repeated.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next_u64(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// An integer in `lo..hi`.
    pub fn int(&mut self, lo: i32, hi: i32) -> i32 {
        lo + (self.next_u64() % (hi - lo) as u64) as i32
    }

    /// A float in `lo..hi`.
    pub fn float(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32 * (hi - lo)
    }

    pub fn coin(&mut self) -> bool {
        self.next_u64() >> 63 == 0
    }
}
