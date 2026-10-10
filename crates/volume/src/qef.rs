//! Places the one vertex a cell (or one patch of a cell) owns, from the
//! points where the surface crosses the cell's edges and the surface normal
//! at each of them.
//!
//! Plain Surface Nets puts the vertex at the average of the crossing points.
//! That average is a low-pass filter over the cell, and it chamfers every
//! sharp feature by a fixed fraction of a cell however fine the grid is:
//! measured on a box turned off the lattice, the averaged surface sat 0.42
//! cells inside the true crease and up to 0.91 at the corners, at every
//! resolution. Constant in cells is the mark of a bias rather than a sampling
//! limit, and it is why "remesh smooths my detail away" does not go away when
//! the density is turned up.
//!
//! Each crossing point `p` with normal `n` says the surface locally satisfies
//! `n . (x - p) = 0`. Minimising the squared residuals (the quadratic error
//! function) puts the vertex on the intersection of those planes: on the
//! crease for a cell straddling one, on the corner point for a cell
//! straddling a corner. Flat and smoothly curved cells are underdetermined by
//! design; a damped inverse leaves them exactly as smooth as Surface Nets
//! made them.

use glam::Vec3;

/// How far outside its own cell the vertex may land, in cells. Nearly
/// parallel planes have a solution far along their shared direction, and
/// letting it fly makes spikes. Slightly outside, not exactly inside, so a
/// crease passing just beyond a corner is still represented.
const CLAMP_MARGIN: f32 = 0.25;

/// Damping, as a fraction of the largest eigenvalue. Applied as
/// `lambda / (lambda^2 + eps^2)` in place of `1 / lambda` rather than by
/// discarding small eigenvalues: discarding is a step function of the input,
/// and near that step a change of 1e-5 in a normal moved the answer by half a
/// cell. Damping is continuous, so neighbouring cells on one smooth surface
/// agree, and the two halves of a symmetric model do too.
const DAMPING: f32 = 0.03;

/// How aligned the normals must be for the cell to count as locally flat:
/// cos 5.7 degrees. Not delicate; a real crease is far beyond it.
const PLANAR_COSINE: f32 = 0.995;

/// An eigenvalue above this fraction of the largest counts toward the feature
/// rank. A crease turning by about 20 degrees or more registers; the spread
/// of normals across one cell of a rounded edge does not.
pub const FEATURE_FRACTION: f32 = 0.03;

/// Solves for the vertex in the cell's own `[0, 1]^3` coordinates, which is
/// also what `points` are in. Returns the position and the feature rank: how
/// many independent directions the normals carry (1 flat or smoothly curved,
/// 2 a crease, 3 a corner, 0 when there were no crossings at all).
pub fn solve(points: &[Vec3], normals: &[Vec3]) -> (Vec3, u8) {
    let count = points.len().min(normals.len());
    if count == 0 {
        return (Vec3::splat(0.5), 0);
    }
    let (points, normals) = (&points[..count], &normals[..count]);
    let scale = 1.0 / count as f32;
    let mass = points.iter().copied().sum::<Vec3>() * scale;
    let normal_sum = normals.iter().copied().sum::<Vec3>();

    // Flat and smoothly curved cells, the great majority of any model, have
    // all their normals pointing essentially one way. The general solve then
    // reduces to sliding the mass point along that direction, so do that
    // directly rather than run an eigen-decomposition to be told the same.
    let sum_length = normal_sum.length();
    if sum_length >= count as f32 * PLANAR_COSINE {
        let axis = normal_sum / sum_length;
        let along: f32 = points.iter().map(|&p| axis.dot(p - mass)).sum();
        // The same damping the general path gives its dominant direction, so
        // the two agree where they meet.
        return (clamp(mass + axis * (along * scale / (1.0 + DAMPING * DAMPING))), 1);
    }

    // Normal equations about the mass point rather than the origin: same
    // minimiser, far better conditioned.
    let mut a = [0.0f32; 6];
    let mut b = Vec3::ZERO;
    for (&p, &n) in points.iter().zip(normals) {
        let d = n.dot(p - mass);
        a[0] += n.x * n.x;
        a[1] += n.x * n.y;
        a[2] += n.x * n.z;
        a[3] += n.y * n.y;
        a[4] += n.y * n.z;
        a[5] += n.z * n.z;
        b += n * d;
    }
    let (vectors, values) = symmetric_eigen(a);
    let largest = values.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    if largest <= 1e-12 {
        return (clamp(mass), 1);
    }
    let cut = largest * FEATURE_FRACTION;
    let rank = values.iter().filter(|v| v.abs() > cut).count() as u8;

    let eps = largest * DAMPING;
    let eps_sq = eps * eps;
    let mut offset = Vec3::ZERO;
    for (e, l) in vectors.into_iter().zip(values) {
        offset += e * (e.dot(b) * l / (l * l + eps_sq));
    }
    (clamp(mass + offset), rank)
}

fn clamp(p: Vec3) -> Vec3 {
    p.clamp(Vec3::splat(-CLAMP_MARGIN), Vec3::splat(1.0 + CLAMP_MARGIN))
}

/// Eigenvectors and eigenvalues of a symmetric 3x3 matrix given as
/// (xx, xy, xz, yy, yz, zz), by cyclic Jacobi rotations.
///
/// Jacobi rather than the closed-form cubic: this runs once per vertex on
/// matrices that are routinely rank-deficient, and it stays accurate on
/// exactly those degenerate cases (flat patch, straight crease), where the
/// closed form loses precision under the cube root. Six sweeps is well past
/// convergence for 3x3.
fn symmetric_eigen(a: [f32; 6]) -> ([Vec3; 3], [f32; 3]) {
    let mut m = [[a[0], a[1], a[2]], [a[1], a[3], a[4]], [a[2], a[4], a[5]]];
    let mut v = [[1.0f32, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for _ in 0..6 {
        if m[0][1] * m[0][1] + m[0][2] * m[0][2] + m[1][2] * m[1][2] <= 1e-20 {
            break;
        }
        for (p, q) in [(0, 1), (0, 2), (1, 2)] {
            if m[p][q].abs() > 1e-20 {
                rotate(&mut m, &mut v, p, q);
            }
        }
    }
    let column = |c: usize| Vec3::new(v[0][c], v[1][c], v[2][c]);
    ([column(0), column(1), column(2)], [m[0][0], m[1][1], m[2][2]])
}

/// One Jacobi rotation in the (p, q) plane, zeroing `m[p][q]`.
fn rotate(m: &mut [[f32; 3]; 3], v: &mut [[f32; 3]; 3], p: usize, q: usize) {
    let theta = (m[q][q] - m[p][p]) / (2.0 * m[p][q]);
    let sign = if theta >= 0.0 { 1.0 } else { -1.0 };
    let t = sign / (theta.abs() + (theta * theta + 1.0).sqrt());
    let c = 1.0 / (t * t + 1.0).sqrt();
    let s = t * c;
    let r = 3 - p - q;

    let (mpp, mqq, mpq) = (m[p][p], m[q][q], m[p][q]);
    let (mrp, mrq) = (m[r][p], m[r][q]);
    m[p][p] = c * c * mpp - 2.0 * s * c * mpq + s * s * mqq;
    m[q][q] = s * s * mpp + 2.0 * s * c * mpq + c * c * mqq;
    m[p][q] = 0.0;
    m[q][p] = 0.0;
    m[r][p] = c * mrp - s * mrq;
    m[p][r] = m[r][p];
    m[r][q] = s * mrp + c * mrq;
    m[q][r] = m[r][q];
    for row in v.iter_mut() {
        let (vp, vq) = (row[p], row[q]);
        row[p] = c * vp - s * vq;
        row[q] = s * vp + c * vq;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Crossings of the plane through `through` with normal `n` on the edges
    /// of the unit cell.
    fn plane_crossings(through: Vec3, n: Vec3, points: &mut Vec<Vec3>, normals: &mut Vec<Vec3>) {
        let n = n.normalize();
        let value = |p: Vec3| n.dot(p - through);
        for c in 0..8u32 {
            let a = Vec3::new((c & 1) as f32, (c >> 1 & 1) as f32, (c >> 2 & 1) as f32);
            for axis in 0..3 {
                if a[axis] != 0.0 {
                    continue;
                }
                let mut b = a;
                b[axis] = 1.0;
                let (va, vb) = (value(a), value(b));
                if (va < 0.0) != (vb < 0.0) {
                    points.push(a.lerp(b, va / (va - vb)));
                    normals.push(n);
                }
            }
        }
    }

    #[test]
    fn a_flat_cell_lands_on_its_plane() {
        let (through, n) = (Vec3::new(0.4, 0.55, 0.5), Vec3::new(0.3, 1.0, -0.2));
        let (mut p, mut nn) = (Vec::new(), Vec::new());
        plane_crossings(through, n, &mut p, &mut nn);
        let (x, rank) = solve(&p, &nn);
        assert_eq!(rank, 1);
        assert!(n.normalize().dot(x - through).abs() < 2e-3, "off the plane: {x}");
    }

    #[test]
    fn a_crease_lands_on_the_line_and_a_corner_on_the_point() {
        let corner = Vec3::new(0.62, 0.41, 0.57);
        let planes = [Vec3::new(1.0, 0.1, 0.0), Vec3::new(0.0, 1.0, 0.15), Vec3::new(0.1, 0.0, 1.0)];
        let (mut p, mut n) = (Vec::new(), Vec::new());
        plane_crossings(corner, planes[0], &mut p, &mut n);
        plane_crossings(corner, planes[1], &mut p, &mut n);
        let (x, rank) = solve(&p, &n);
        assert_eq!(rank, 2);
        for plane in &planes[..2] {
            assert!(plane.normalize().dot(x - corner).abs() < 5e-3, "off the crease: {x}");
        }
        plane_crossings(corner, planes[2], &mut p, &mut n);
        let (x, rank) = solve(&p, &n);
        assert_eq!(rank, 3);
        assert!(x.distance(corner) < 5e-3, "off the corner: {x}");
    }

    #[test]
    fn mirrored_input_gives_the_mirrored_answer() {
        let (mut p, mut n) = (Vec::new(), Vec::new());
        plane_crossings(Vec3::new(0.3, 0.6, 0.5), Vec3::new(1.0, 0.4, 0.2), &mut p, &mut n);
        plane_crossings(Vec3::new(0.3, 0.6, 0.5), Vec3::new(-0.2, 1.0, 0.5), &mut p, &mut n);
        let flip = |v: Vec3| Vec3::new(1.0 - v.x, v.y, v.z);
        let mp: Vec<Vec3> = p.iter().map(|&v| flip(v)).collect();
        let mn: Vec<Vec3> = n.iter().map(|&v| Vec3::new(-v.x, v.y, v.z)).collect();
        let (a, ra) = solve(&p, &n);
        let (b, rb) = solve(&mp, &mn);
        assert_eq!(ra, rb);
        assert!(flip(a).distance(b) < 1e-5);
    }

    #[test]
    fn nothing_to_go_on_gives_the_cell_centre_not_nan() {
        assert_eq!(solve(&[], &[]), (Vec3::splat(0.5), 0));
        let (x, _) = solve(&[Vec3::new(0.2, 0.5, 0.5), Vec3::new(0.8, 0.5, 0.5)], &[Vec3::ZERO, Vec3::ZERO]);
        assert!(x.is_finite() && x.distance(Vec3::splat(0.5)) < 1e-6);
    }
}
