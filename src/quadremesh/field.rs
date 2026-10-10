//! The quad remesher's two fields (Jakob et al. 2015, "Instant Field-Aligned
//! Meshes", extrinsic 4-RoSy / 4-PoSy variants), solved coarsest level first.
//!
//! Each vertex carries an orientation q (a unit tangent; q and n x q are the
//! two quad edge directions, each up to sign, so q is only defined up to
//! 90-degree turns) and a position offset o (a point near the vertex that
//! sits on the vertex's local quad lattice of spacing `scale`). Neighbours
//! never compare raw values: first they find the rotation or lattice shift
//! under which the two agree best.

use glam::Vec3;
use rayon::prelude::*;

use super::curvature::Curvature;
use super::hierarchy::{Hierarchy, Level};
use super::poly::{NONE, normalize_or};
use crate::util::SharedMut;

/// Smoothing sweeps per level.
const ITERATIONS: usize = 6;

/// How hard the orientation field is pulled onto the curvature directions
/// where the curvature field is fully confident: the share of each sweep's
/// result that comes from the guide rather than from the neighbours.
const CURVATURE_STRENGTH: f32 = 0.35;

/// The pair of representatives of q0's and q1's 4-fold symmetry classes that
/// are closest to each other: among {q0, n0 x q0} and {q1, n1 x q1}, the pair
/// with the largest |dot|, with q1's sign flipped to match.
#[inline]
pub fn compat_orientation(q0: Vec3, n0: Vec3, q1: Vec3, n1: Vec3) -> (Vec3, Vec3) {
    let (t0, t1) = (n0.cross(q0), n1.cross(q1));
    let (d00, d01, d10, d11) = (q0.dot(q1), q0.dot(t1), t0.dot(q1), t0.dot(t1));
    let (a00, a01, a10, a11) = (d00.abs(), d01.abs(), d10.abs(), d11.abs());
    let sign = |v: Vec3, d: f32| if d >= 0.0 { v } else { -v };
    if a00 >= a01 && a00 >= a10 && a00 >= a11 {
        (q0, sign(q1, d00))
    } else if a01 >= a10 && a01 >= a11 {
        (q0, sign(t1, d01))
    } else if a10 >= a11 {
        (t0, sign(q1, d10))
    } else {
        (t0, sign(t1, d11))
    }
}

/// A point both tangent planes agree is "between" p0 and p1: the midpoint,
/// pulled off the chord toward where the two planes meet, so on a curved
/// surface the two lattices are compared near the surface, not inside it.
#[inline]
fn middle_point(p0: Vec3, n0: Vec3, p1: Vec3, n1: Vec3) -> Vec3 {
    let (n0p0, n0p1, n1p0, n1p1, n0n1) = (n0.dot(p0), n0.dot(p1), n1.dot(p0), n1.dot(p1), n0.dot(n1));
    let denom = 1.0 / (1.0 - n0n1 * n0n1 + 1e-4);
    let lambda0 = 2.0 * (n0p1 - n0p0 - n0n1 * (n1p0 - n1p1)) * denom;
    let lambda1 = 2.0 * (n1p0 - n1p1 - n0n1 * (n0p1 - n0p0)) * denom;
    (p0 + p1) * 0.5 - (n0 * lambda0 + n1 * lambda1) * 0.25
}

/// The lattice point (of the lattice through o, axes q and n x q) nearest p.
#[inline]
fn position_round(o: Vec3, q: Vec3, n: Vec3, p: Vec3, scale: f32, inv: f32) -> Vec3 {
    let t = n.cross(q);
    let d = p - o;
    o + q * ((q.dot(d) * inv).round() * scale) + t * ((t.dot(d) * inv).round() * scale)
}

/// The two lattice points, one from each vertex's lattice and both near the
/// pair's middle point, that are closest to each other, as integer
/// coordinates relative to each vertex's own offset. q0 / q1 must already be
/// rotation-matched. Equal indices mean o0 and o1 are the same lattice
/// vertex; indices one step apart mean they are lattice neighbours.
#[inline]
pub fn compat_position_index(
    p0: Vec3,
    n0: Vec3,
    q0: Vec3,
    o0: Vec3,
    p1: Vec3,
    n1: Vec3,
    q1: Vec3,
    o1: Vec3,
    scale: f32,
    inv: f32,
) -> ([i32; 2], [i32; 2]) {
    let (t0, t1) = (n0.cross(q0), n1.cross(q1));
    let middle = middle_point(p0, n0, p1, n1);
    // Cell below the middle point in each lattice.
    let (d0, d1) = (middle - o0, middle - o1);
    let (fi0, fj0) = ((q0.dot(d0) * inv).floor(), (t0.dot(d0) * inv).floor());
    let (fi1, fj1) = ((q1.dot(d1) * inv).floor(), (t1.dot(d1) * inv).floor());
    let b0 = o0 + (q0 * fi0 + t0 * fj0) * scale;
    let b1 = o1 + (q1 * fi1 + t1 * fj1) * scale;
    let (u0, v0, u1, v1) = (q0 * scale, t0 * scale, q1 * scale, t1 * scale);

    let mut best = f32::MAX;
    let (mut best_a, mut best_b) = (0, 0);
    for a in 0..4 {
        let c0 = b0 + u0 * (a & 1) as f32 + v0 * (a >> 1) as f32;
        for b in 0..4 {
            let cost = c0.distance_squared(b1 + u1 * (b & 1) as f32 + v1 * (b >> 1) as f32);
            if cost < best {
                (best, best_a, best_b) = (cost, a, b);
            }
        }
    }
    (
        [fi0 as i32 + (best_a & 1), fj0 as i32 + (best_a >> 1)],
        [fi1 as i32 + (best_b & 1), fj1 as i32 + (best_b >> 1)],
    )
}

/// [`compat_position_index`], returning the two matched lattice points.
#[inline]
fn compat_position(
    p0: Vec3,
    n0: Vec3,
    q0: Vec3,
    o0: Vec3,
    p1: Vec3,
    n1: Vec3,
    q1: Vec3,
    o1: Vec3,
    scale: f32,
    inv: f32,
) -> (Vec3, Vec3) {
    let (i0, i1) = compat_position_index(p0, n0, q0, o0, p1, n1, q1, o1, scale, inv);
    (
        o0 + (q0 * i0[0] as f32 + n0.cross(q0) * i0[1] as f32) * scale,
        o1 + (q1 * i1[0] as f32 + n1.cross(q1) * i1[1] as f32) * scale,
    )
}

/// `v` projected into the plane with unit normal `n`, normalised.
#[inline]
pub fn tangent_unit(v: Vec3, n: Vec3, fallback: Vec3) -> Vec3 {
    normalize_or(v - n * n.dot(v), fallback)
}

/// Some unit tangent of `n`: deterministic, for when nothing better is known.
#[inline]
pub fn any_tangent(n: Vec3) -> Vec3 {
    let axis = if n.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
    normalize_or(n.cross(axis), Vec3::Z)
}

/// A field being swept: readable at any vertex while one phase is written.
#[derive(Clone, Copy)]
struct Field<'a>(SharedMut<'a, Vec3>);

impl Field<'_> {
    #[inline]
    fn get(&self, i: usize) -> Vec3 {
        // Safety: see `sweep`.
        unsafe { self.0.read(i) }
    }
}

/// One Gauss-Seidel sweep: every vertex is replaced by `update`, which reads
/// the newest values of the vertex and its neighbours. The vertices of one
/// colour phase share no edge, so a phase runs in parallel and the result is
/// the same as the serial order whatever the thread scheduling.
fn sweep(level: &Level, field: &mut [Vec3], update: impl Fn(Field, usize) -> Vec3 + Sync) {
    let shared = Field(SharedMut::new(field));
    for phase in level.phase_start.windows(2) {
        let verts = &level.phase_verts[phase[0] as usize..phase[1] as usize];
        let run = |&v: &u32| {
            let value = update(shared, v as usize);
            // Safety: `update` reads only v and its neighbours, none of which
            // is in this phase, and v is written by this call alone.
            unsafe { shared.0.write(v as usize, value) }
        };
        if verts.len() < 2048 {
            verts.iter().for_each(run);
        } else {
            verts.par_iter().with_min_len(1024).for_each(run);
        }
    }
}

/// Orientation field for every level (index = level): per vertex, a tangent
/// direction, smoothed so neighbours agree up to 90-degree turns. Coarse
/// levels hold the finest solution averaged back up, ready for the position
/// solve.
pub fn solve_orientation(h: &Hierarchy, curvature: Option<&Curvature>) -> Vec<Vec<Vec3>> {
    let levels = h.levels.len();
    let mut q: Vec<Vec<Vec3>> = vec![Vec::new(); levels];
    let top = &h.levels[levels - 1];
    q[levels - 1] = (0..top.count())
        .map(|v| tangent_unit(random_unit(v), top.normal[v], any_tangent(top.normal[v])))
        .collect();

    for l in (0..levels).rev() {
        let level = &h.levels[l];
        if l + 1 < levels {
            let coarse = &h.levels[l + 1];
            let mut fine = vec![Vec3::ZERO; level.count()];
            for c in 0..coarse.count() {
                for f in [coarse.fine_a[c], coarse.fine_b[c]] {
                    if f != NONE {
                        let n = level.normal[f as usize];
                        fine[f as usize] = tangent_unit(q[l + 1][c], n, any_tangent(n));
                    }
                }
            }
            q[l] = fine;
        }
        let guide = curvature.map(|c| (&c.direction[l], &c.confidence[l]));
        for _ in 0..ITERATIONS {
            sweep(level, &mut q[l], |q, v| smooth_orientation(level, q, v, guide));
        }
    }

    // Fine -> coarse, so every level carries the converged field.
    for l in 0..levels - 1 {
        let (fine, coarse) = (&h.levels[l], &h.levels[l + 1]);
        let up: Vec<Vec3> = (0..coarse.count())
            .map(|c| {
                let n = coarse.normal[c];
                let a = coarse.fine_a[c] as usize;
                let qa = tangent_unit(q[l][a], n, any_tangent(n));
                if coarse.fine_b[c] == NONE {
                    return qa;
                }
                let b = coarse.fine_b[c] as usize;
                let qb = tangent_unit(q[l][b], n, qa);
                let (x, y) = compat_orientation(qa, n, qb, n);
                tangent_unit(x * fine.area[a] + y * fine.area[b], n, qa)
            })
            .collect();
        q[l + 1] = up;
    }
    q
}

/// Position field for every level, given the orientation field: per vertex,
/// a lattice offset, smoothed so neighbours' lattices of spacing `scale`
/// line up.
pub fn solve_position(h: &Hierarchy, q: &[Vec<Vec3>], scale: f32) -> Vec<Vec<Vec3>> {
    let levels = h.levels.len();
    let inv = 1.0 / scale;
    let mut o: Vec<Vec<Vec3>> = vec![Vec::new(); levels];
    o[levels - 1] = h.levels[levels - 1].position.clone();

    for l in (0..levels).rev() {
        let level = &h.levels[l];
        if l + 1 < levels {
            let coarse = &h.levels[l + 1];
            let mut fine = vec![Vec3::ZERO; level.count()];
            for c in 0..coarse.count() {
                let oc = o[l + 1][c];
                for f in [coarse.fine_a[c], coarse.fine_b[c]] {
                    if f != NONE {
                        let (n, p) = (level.normal[f as usize], level.position[f as usize]);
                        fine[f as usize] = oc - n * n.dot(oc - p);
                    }
                }
            }
            o[l] = fine;
        }
        let ql = &q[l];
        for _ in 0..ITERATIONS {
            sweep(level, &mut o[l], |o, v| smooth_position(level, ql, o, v, scale, inv));
        }
    }
    o
}

fn smooth_orientation(level: &Level, q: Field, v: usize, guide: Option<(&Vec<Vec3>, &Vec<f32>)>) -> Vec3 {
    let n = level.normal[v];
    let mut sum = q.get(v);
    let mut weight_sum = 0.0f32;
    for k in level.links(v) {
        let u = level.adj[k] as usize;
        let w = level.adj_weight[k];
        let (a, b) = compat_orientation(sum, n, q.get(u), level.normal[u]);
        sum = a * weight_sum + b * w;
        sum -= n * n.dot(sum);
        weight_sum += w;
        let len = sum.length();
        if len > 1e-20 {
            sum /= len;
        }
    }
    if let Some((direction, confidence)) = guide {
        let gw = confidence[v] * CURVATURE_STRENGTH;
        if gw > 0.0 {
            let (a, b) = compat_orientation(sum, n, direction[v], n);
            sum = a * (1.0 - gw) + b * gw;
        }
    }
    tangent_unit(sum, n, q.get(v))
}

fn smooth_position(level: &Level, q: &[Vec3], o: Field, v: usize, scale: f32, inv: f32) -> Vec3 {
    let (n, p, qv) = (level.normal[v], level.position[v], q[v]);
    let mut sum = o.get(v);
    let mut weight_sum = 0.0f32;
    for k in level.links(v) {
        let u = level.adj[k] as usize;
        let w = level.adj_weight[k];
        // No rotation matching needed: a square lattice is the same set of
        // points whichever of its four directions q names.
        let (a, b) = compat_position(p, n, qv, sum, level.position[u], level.normal[u], q[u], o.get(u), scale, inv);
        sum = a * weight_sum + b * w;
        weight_sum += w;
        if weight_sum > 1e-20 {
            sum /= weight_sum;
        }
        sum -= n * n.dot(sum - p);
    }
    position_round(sum, qv, n, p, scale, inv)
}

/// Deterministic pseudo-random unit vector from an index (splitmix-style).
fn random_unit(i: usize) -> Vec3 {
    let mut x = (i as u64).wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(0x632BE59BD9B4E019);
    let mut next = || {
        x ^= x >> 30;
        x = x.wrapping_mul(0xbf58476d1ce4e5b9);
        x ^= x >> 27;
        x = x.wrapping_mul(0x94d049bb133111eb);
        x ^= x >> 31;
        (x >> 40) as f32 * (1.0 / (1 << 24) as f32) * 2.0 - 1.0
    };
    normalize_or(Vec3::new(next(), next(), next()), Vec3::X)
}
