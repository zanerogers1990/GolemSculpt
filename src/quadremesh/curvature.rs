//! Where the sculpt bends more one way than the other, and which way: the
//! guide that makes edge loops run around limbs and along ridges instead of
//! wherever the smoothing happened to settle.
//!
//! Curvature is measured at the scale of the quads, not of the input
//! triangles. Each finest vertex gets Taubin's integral tensor (the sum over
//! its edges of normal curvature along the edge times the edge direction,
//! squared) whose eigenvectors are the principal directions. That tensor is
//! linear, so area-weighted averaging up the hierarchy gives the curvature of
//! ever larger patches; the level whose clusters are about one quad in area
//! is the one the fine levels read from, which keeps skin-pore detail on a
//! dense sculpt from steering quads many times its size.

use glam::Vec3;
use rayon::prelude::*;

use super::field::any_tangent;
use super::hierarchy::{Hierarchy, Level};
use super::poly::NONE;

/// Per vertex of every hierarchy level: a unit tangent along a principal
/// curvature direction (either one: the orientation field is 4-fold
/// symmetric, so the two are the same constraint) and a confidence that is
/// zero on flat and evenly round surface (a plane, a sphere) and one where
/// the two principal curvatures differ by a clear fraction of a quad.
pub struct Curvature {
    pub direction: Vec<Vec<Vec3>>,
    pub confidence: Vec<Vec<f32>>,
}

/// |k1 - k2| * quad edge below which a surface counts as having no preferred
/// direction, and the value at which confidence reaches one. 0.6 is roughly
/// a cylinder ten quads around.
const CONFIDENCE_FLOOR: f32 = 0.06;
const CONFIDENCE_FULL: f32 = 0.6;
const SMOOTH_PASSES: usize = 2;

/// A symmetric 3x3 tensor: xx, yy, zz, xy, xz, yz.
type Tensor = [f32; 6];

impl Curvature {
    pub fn build(h: &Hierarchy, quad_edge: f32, target_quads: usize) -> Curvature {
        let levels = h.levels.len();
        let mut tensor: Vec<Vec<Tensor>> = vec![finest_tensor(&h.levels[0])];
        for l in 0..levels - 1 {
            let up = restrict(&h.levels[l], &h.levels[l + 1], &tensor[l]);
            tensor.push(up);
        }

        // The level whose clusters are about half a quad each.
        let mut quad_level = 0;
        while quad_level + 1 < levels && h.levels[quad_level].count() > target_quads * 2 {
            quad_level += 1;
        }
        for _ in 0..SMOOTH_PASSES {
            tensor[quad_level] = smooth(&h.levels[quad_level], &tensor[quad_level]);
        }

        let mut direction = vec![Vec::new(); levels];
        let mut confidence = vec![Vec::new(); levels];
        // Of each vertex of the level being filled, its ancestor on quad_level.
        let mut ancestor: Vec<u32> = Vec::new();
        for l in (0..levels).rev() {
            let level = &h.levels[l];
            if l == quad_level {
                ancestor = (0..level.count() as u32).collect();
            } else if l < quad_level {
                ancestor = level.to_coarse.iter().map(|&c| ancestor[c as usize]).collect();
            }
            let source = &tensor[l.max(quad_level)];
            let (dir, conf): (Vec<Vec3>, Vec<f32>) = (0..level.count())
                .into_par_iter()
                .map(|v| {
                    let i = if l < quad_level { ancestor[v] as usize } else { v };
                    principal(&source[i], level.normal[v], quad_edge)
                })
                .unzip();
            direction[l] = dir;
            confidence[l] = conf;
        }
        Curvature { direction, confidence }
    }
}

fn finest_tensor(level: &Level) -> Vec<Tensor> {
    let (p, nrm) = (&level.position, &level.normal);
    (0..level.count())
        .into_par_iter()
        .map(|v| {
            let mut m = [0.0f32; 6];
            let mut count = 0;
            for k in level.links(v) {
                let u = level.adj[k] as usize;
                let mut e = p[u] - p[v];
                let len2 = e.length_squared();
                if !(len2 > 1e-30) {
                    continue;
                }
                let kappa = (nrm[u] - nrm[v]).dot(e) / len2;
                // Edge direction in the tangent plane.
                e -= nrm[v] * nrm[v].dot(e);
                let t2 = e.length_squared();
                if !(t2 > 1e-30) {
                    continue;
                }
                let w = kappa / t2;
                m[0] += w * e.x * e.x;
                m[1] += w * e.y * e.y;
                m[2] += w * e.z * e.z;
                m[3] += w * e.x * e.y;
                m[4] += w * e.x * e.z;
                m[5] += w * e.y * e.z;
                count += 1;
            }
            if count > 0 {
                m.iter_mut().for_each(|x| *x /= count as f32);
            }
            m
        })
        .collect()
}

fn restrict(fine: &Level, coarse: &Level, mf: &[Tensor]) -> Vec<Tensor> {
    (0..coarse.count())
        .map(|c| {
            let a = coarse.fine_a[c] as usize;
            if coarse.fine_b[c] == NONE {
                return mf[a];
            }
            let b = coarse.fine_b[c] as usize;
            let (mut wa, mut wb) = (fine.area[a], fine.area[b]);
            let sum = wa + wb;
            if sum > 0.0 {
                wa /= sum;
                wb /= sum;
            } else {
                (wa, wb) = (0.5, 0.5);
            }
            std::array::from_fn(|i| mf[a][i] * wa + mf[b][i] * wb)
        })
        .collect()
}

/// One Jacobi pass: each vertex becomes the area-weighted mean of itself and
/// its neighbours.
fn smooth(level: &Level, m: &[Tensor]) -> Vec<Tensor> {
    (0..level.count())
        .into_par_iter()
        .map(|v| {
            let w = level.area[v].max(1e-30);
            let mut total = w;
            let mut sum = m[v].map(|x| x * w);
            for k in level.links(v) {
                let u = level.adj[k] as usize;
                let w = level.area[u].max(1e-30);
                total += w;
                for i in 0..6 {
                    sum[i] += m[u][i] * w;
                }
            }
            sum.map(|x| x / total)
        })
        .collect()
}

/// Principal direction and confidence of a tensor, in the tangent plane of `n`.
fn principal(m: &Tensor, n: Vec3, quad_edge: f32) -> (Vec3, f32) {
    let [xx, yy, zz, xy, xz, yz] = *m;
    let s = any_tangent(n);
    let t = n.cross(s);
    let mul = |v: Vec3| Vec3::new(xx * v.x + xy * v.y + xz * v.z, xy * v.x + yy * v.y + yz * v.z, xz * v.x + yz * v.y + zz * v.z);
    // The 2x2 tangent-plane block.
    let (ms, mt) = (mul(s), mul(t));
    let (a, b, c) = (s.dot(ms), t.dot(ms), t.dot(mt));
    let theta = 0.5 * (2.0 * b).atan2(a - c);
    // Taubin's tensor has eigenvalues (3 k1 + k2) / 8 and (k1 + 3 k2) / 8:
    // their difference is a quarter of k1 - k2.
    let anisotropy = 4.0 * ((a - c) * (a - c) + 4.0 * b * b).sqrt() * quad_edge;
    let confidence = ((anisotropy - CONFIDENCE_FLOOR) / (CONFIDENCE_FULL - CONFIDENCE_FLOOR)).clamp(0.0, 1.0);
    (s * theta.cos() + t * theta.sin(), confidence)
}
