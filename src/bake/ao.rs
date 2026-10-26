//! Ambient occlusion: how much open sky a point of the sculpt sees.
//!
//! From the point found on the sculpt, rays go out over the half of the sky
//! its normal faces, and the share that run into the sculpt again closer
//! than `within` is how shut in the point is; the texel is the rest of the
//! sky, white for wide open. The rays are spread by the cosine of their angle
//! to the normal (more of them near the normal, where light falls hardest on
//! a matte surface), so the plain share of blocked rays is the occlusion a
//! matte surface feels, with no weighting afterwards.
//!
//! The directions come from a fixed, evenly spread set (Hammersley's), the
//! same for every texel but turned by an amount worked out from the texel's
//! place in the picture. Fixed, so a bake comes out the same every time and
//! however the work is shared between threads; turned, so neighbouring
//! texels' small errors do not line up into rings and bands but fall as a
//! fine grain, which more rays smooth away.
//!
//! The ray starts a hair off the surface along the triangle's flat normal,
//! and a direction that the smooth normal leans below the triangle's own
//! plane is reflected back above it; otherwise a ray could start into the
//! very triangle it leaves and darken a flat surface.

use std::f32::consts::TAU;

use glam::Vec3;

use super::cast::{Ready, blocked};

/// How the sky is sampled for one bake.
pub(super) struct Sky {
    /// The fixed set: two numbers from 0 to 1 per ray.
    samples: Vec<[f32; 2]>,
    /// How far a ray is followed, and how far off the surface it starts, in
    /// scene units.
    within: f32,
    lift: f32,
}

/// The `k`th number of van der Corput's sequence: `k`'s binary digits read
/// backwards after the point.
fn radical_inverse(k: u32) -> f32 {
    k.reverse_bits() as f32 * (1.0 / 4_294_967_296.0)
}

/// Two numbers from 0 to 1 drawn from a texel's place, the same every time.
fn turn(x: u32, y: u32) -> [f32; 2] {
    // A small integer hash (Wang's, and a second round for the other number).
    let hash = |mut h: u32| {
        h = (h ^ 61) ^ (h >> 16);
        h = h.wrapping_mul(9);
        h ^= h >> 4;
        h = h.wrapping_mul(0x27d4_eb2d);
        h ^ (h >> 15)
    };
    let a = hash(x.wrapping_mul(0x9e37_79b9) ^ y.wrapping_mul(0x85eb_ca6b));
    let b = hash(a ^ 0x68e3_1da4);
    [(a >> 8) as f32 / 16_777_216.0, (b >> 8) as f32 / 16_777_216.0]
}

impl Sky {
    pub(super) fn new(rays: u32, within: f32, lift: f32) -> Sky {
        let samples = (0..rays).map(|k| [(k as f32 + 0.5) / rays as f32, radical_inverse(k)]).collect();
        Sky { samples, within, lift }
    }

    /// The share of sky seen from `point` on the sculpt, whose smooth normal
    /// is `normal` and flat normal `face`, for the texel at `x, y`.
    pub(super) fn openness(&self, sources: &[Ready], point: Vec3, normal: Vec3, face: Vec3, x: u32, y: u32) -> f32 {
        let face = if face == Vec3::ZERO { normal } else { face };
        let origin = point + face * self.lift;
        let (across, up) = normal.any_orthonormal_pair();
        let [du, dv] = turn(x, y);
        let mut shut = 0usize;
        for &[a, b] in &self.samples {
            let (a, b) = ((a + du).fract(), (b + dv).fract());
            let r = a.sqrt();
            let phi = TAU * b;
            let mut dir = across * (r * phi.cos()) + up * (r * phi.sin()) + normal * (1.0 - a).max(0.0).sqrt();
            let below = dir.dot(face);
            if below <= 0.0 {
                dir -= face * (2.0 * below);
            }
            if blocked(sources, origin, dir.normalize_or(face), self.within) {
                shut += 1;
            }
        }
        1.0 - shut as f32 / self.samples.len() as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_turn_is_the_same_every_time_and_differs_between_neighbours() {
        assert_eq!(turn(10, 20), turn(10, 20));
        assert_ne!(turn(10, 20), turn(11, 20));
        assert_ne!(turn(10, 20), turn(10, 21));
        for (x, y) in [(0, 0), (8191, 8191), (5, 900)] {
            assert!(turn(x, y).iter().all(|v| (0.0..1.0).contains(v)));
        }
    }

    #[test]
    fn the_fixed_set_spreads_over_both_numbers() {
        let sky = Sky::new(64, 1.0, 0.0);
        let mut first: Vec<f32> = sky.samples.iter().map(|s| s[1]).collect();
        first.sort_by(f32::total_cmp);
        for (k, v) in first.iter().enumerate() {
            assert!((v - k as f32 / 64.0).abs() < 1e-6, "every 64th once");
        }
    }
}
