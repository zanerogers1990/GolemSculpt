//! What Mask Fit works out: which points of a shell are its inside, how far
//! each sits from the face and how thick the wall is, and where the inside
//! goes to fit the face again.
//!
//! Everything is in the shell's own space. The head is carried into it once
//! ([`Face::new`]), so a shell and a head that stand anywhere, turned or
//! scaled, are compared where they are seen together. Lengths handed in are
//! lengths in that space; the tool turns the card's millimetres into them.
//!
//! **Inside or outside.** A point is the inside when the head is within reach
//! of it and its surface faces the head: its normal points against the
//! head's normal at the nearest point (more than 60 degrees past square). The
//! outside faces the same way as the head, and the rim, which faces sideways,
//! is neither and is never moved. A point with no head within reach (a horn
//! sculpted far out) is the outside.
//!
//! **The wall** at an outside point is how far it is from the inside surface
//! (the triangles whose three corners are all inside). The rim has no wall
//! of its own: it is the wall's edge, as thick as the wall. It is signed: an
//! inside that has been pushed out through the outside gives a wall below
//! zero, so it reads as thin however far through it went.

use glam::{Mat4, Vec3};
use rayon::prelude::*;
use sculpt_volume::TriangleBvh;

/// A point is the inside when its normal and the head's, at the nearest
/// point, are further apart than this (a dot product): 120 degrees.
const FACING: f32 = -0.5;
/// And the outside when they are closer than this: 60 degrees. Between the
/// two is the rim.
const FACING_OUT: f32 = 0.5;

/// Below this many points a parallel split costs more than it saves.
const MIN_PARALLEL: usize = 4096;

/// The card's two lengths, in millimetres.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Settings {
    /// How far the inside sits off the skin.
    pub gap_mm: f32,
    /// The thinnest wall that is not marked.
    pub min_wall_mm: f32,
}

impl Default for Settings {
    /// Three millimetres of room, for comfort and a thin foam lining, and a
    /// two millimetre wall, about the least a printed mask stays stiff at.
    fn default() -> Settings {
        Settings { gap_mm: 3.0, min_wall_mm: 2.0 }
    }
}

impl Settings {
    pub const GAP_RANGE: std::ops::RangeInclusive<f32> = 0.0..=15.0;
    pub const WALL_RANGE: std::ops::RangeInclusive<f32> = 0.4..=10.0;

    /// In range, whatever a script or a bad number put in.
    pub fn fixed(self) -> Settings {
        let fix = |v: f32, r: std::ops::RangeInclusive<f32>, d: f32| if v.is_finite() { v.clamp(*r.start(), *r.end()) } else { d };
        let d = Settings::default();
        Settings { gap_mm: fix(self.gap_mm, Settings::GAP_RANGE, d.gap_mm), min_wall_mm: fix(self.min_wall_mm, Settings::WALL_RANGE, d.min_wall_mm) }
    }
}

/// The head's surface, carried into the shell's space and made ready to ask
/// for the nearest point.
pub struct Face {
    positions: Vec<Vec3>,
    normals: Vec<Vec3>,
    indices: Vec<u32>,
    bvh: TriangleBvh,
}

impl Face {
    /// The head's triangles, carried by `to_shell` (the head's placement,
    /// then the inverse of the shell's). A placement that mirrors turns the
    /// triangles inside out, so their order is put back.
    pub fn new(positions: &[Vec3], indices: &[u32], to_shell: Mat4) -> Face {
        let positions: Vec<Vec3> =
            positions.par_iter().with_min_len(MIN_PARALLEL).map(|&p| to_shell.transform_point3(p)).collect();
        let mut indices: Vec<u32> = indices[..indices.len() / 3 * 3].to_vec();
        if to_shell.determinant() < 0.0 {
            indices.chunks_exact_mut(3).for_each(|t| t.swap(1, 2));
        }
        let mut normals = vec![Vec3::ZERO; positions.len()];
        for t in indices.chunks_exact(3) {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| i as usize);
            // Area-weighted: the cross product's length is twice the area.
            let n = (positions[b] - positions[a]).cross(positions[c] - positions[a]);
            normals[a] += n;
            normals[b] += n;
            normals[c] += n;
        }
        normals.iter_mut().for_each(|n| *n = n.normalize_or_zero());
        let bvh = TriangleBvh::build(&positions, &indices);
        Face { positions, normals, indices, bvh }
    }

    pub fn is_empty(&self) -> bool {
        self.bvh.is_empty()
    }

    /// The nearest point of the head to `p`, if within `reach`, with the
    /// head's smooth outward normal there (its corners' normals, blended).
    pub fn nearest(&self, p: Vec3, reach: f32) -> Option<(Vec3, Vec3)> {
        let hit = self.bvh.closest_within(p, reach)?;
        let t = hit.triangle as usize * 3;
        let [a, b, c] = [0, 1, 2].map(|k| self.indices[t + k] as usize);
        let [w0, w1, w2] = barycentric(hit.point, self.positions[a], self.positions[b], self.positions[c]);
        let blended = self.normals[a] * w0 + self.normals[b] * w1 + self.normals[c] * w2;
        let normal = blended.try_normalize().unwrap_or_else(|| self.bvh.triangle_normal(hit.triangle));
        Some((hit.point, normal))
    }
}

/// The weights of `a`, `b` and `c` that give `p` (which lies on the
/// triangle); a third each for a triangle with no area.
fn barycentric(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> [f32; 3] {
    let (v0, v1, v2) = (b - a, c - a, p - a);
    let (d00, d01, d11, d20, d21) = (v0.dot(v0), v0.dot(v1), v1.dot(v1), v2.dot(v0), v2.dot(v1));
    let den = d00 * d11 - d01 * d01;
    if den.abs() <= f32::EPSILON * (d00 * d11).max(f32::MIN_POSITIVE) {
        return [1.0 / 3.0; 3];
    }
    let v = (d11 * d20 - d01 * d21) / den;
    let w = (d00 * d21 - d01 * d20) / den;
    [1.0 - v - w, v, w]
}

/// What a look at the shell found, one entry per shell point.
#[derive(Clone, Debug, Default)]
pub struct Survey {
    /// The point is on the inside surface.
    pub inside: Vec<bool>,
    /// For an inside point, how far it is off the face along the face's
    /// normal (below zero: into the face). Not a number for other points.
    pub off_face: Vec<f32>,
    /// For an outside point, the wall's thickness there (below zero: the
    /// inside has come through). Infinite when no inside is within reach,
    /// not a number for an inside point or the rim.
    pub wall: Vec<f32>,
}

impl Survey {
    pub fn inside_count(&self) -> usize {
        self.inside.iter().filter(|&&i| i).count()
    }

    /// The thinnest wall found, if any outside point had an inside in reach.
    pub fn thinnest(&self) -> Option<f32> {
        self.wall.iter().copied().filter(|w| w.is_finite()).min_by(f32::total_cmp)
    }

    /// Outside points whose wall is thinner than `min_wall`.
    pub fn is_thin(&self, v: usize, min_wall: f32) -> bool {
        self.wall[v] < min_wall
    }

    /// Inside points closer to the face than half `gap`, or into it.
    pub fn is_close(&self, v: usize, gap: f32) -> bool {
        self.off_face[v] < gap * 0.5
    }

    pub fn thin_count(&self, min_wall: f32) -> usize {
        (0..self.wall.len()).filter(|&v| self.is_thin(v, min_wall)).count()
    }

    pub fn close_count(&self, gap: f32) -> usize {
        (0..self.off_face.len()).filter(|&v| self.is_close(v, gap)).count()
    }
}

/// Looks at every point of the shell against `face`: which are the inside
/// (the head within `reach`), how far those sit off the face, and how thick
/// the wall is at the others (looked for within `wall_reach`).
pub fn survey(positions: &[Vec3], normals: &[Vec3], indices: &[u32], face: &Face, reach: f32, wall_reach: f32) -> Survey {
    let n = positions.len();
    // For each point: how it faces the head (None: no head within reach), and how far off it it is.
    let near: Vec<Option<(f32, f32)>> = (0..n)
        .into_par_iter()
        .with_min_len(MIN_PARALLEL)
        .map(|v| {
            let p = positions[v];
            let (at, normal) = face.nearest(p, reach)?;
            let own = normals.get(v).copied().unwrap_or(Vec3::ZERO).normalize_or_zero();
            Some((own.dot(normal), (p - at).dot(normal)))
        })
        .collect();
    let inside: Vec<bool> = near.iter().map(|n| n.is_some_and(|(f, _)| f < FACING)).collect();
    let outside: Vec<bool> = near.iter().map(|n| n.is_none_or(|(f, _)| f > FACING_OUT)).collect();
    let off_face: Vec<f32> = near.iter().map(|n| n.filter(|(f, _)| *f < FACING).map_or(f32::NAN, |(_, d)| d)).collect();

    let inner: Vec<u32> = indices
        .chunks_exact(3)
        .filter(|t| t.iter().all(|&i| inside.get(i as usize).copied().unwrap_or(false)))
        .flatten()
        .copied()
        .collect();
    let wall: Vec<f32> = if inner.is_empty() {
        (0..n).map(|v| if outside[v] { f32::INFINITY } else { f32::NAN }).collect()
    } else {
        let lining = TriangleBvh::build(positions, &inner);
        (0..n)
            .into_par_iter()
            .with_min_len(MIN_PARALLEL)
            .map(|v| {
                if !outside[v] {
                    return f32::NAN;
                }
                let Some(hit) = lining.closest_within(positions[v], wall_reach) else { return f32::INFINITY };
                let own = normals.get(v).copied().unwrap_or(Vec3::ZERO).normalize_or_zero();
                // The inside belongs behind the outside; in front, it has come through.
                let behind = (positions[v] - hit.point).dot(own) >= 0.0;
                let d = hit.distance();
                if behind { d } else { -d }
            })
            .collect()
    };
    Survey { inside, off_face, wall }
}

/// Where every shell point goes when the inside is refitted: each inside
/// point to `gap` off the head, every other point where it is.
///
/// A point moves along the head's normal, not to the nearest point plus the
/// gap: the two are the same where the inside is near the face, but in a
/// hollow (beside the nose, round the eyes) the nearest point bunches the
/// points that were spread over it and folds the surface. Along the normal
/// each keeps its place across the face, as Project Detail keeps spacing. A masked point moves only as far as it is free, as under a
/// brush; a hidden one stays.
pub fn refit(
    positions: &[Vec3],
    mask: &[f32],
    hidden: Option<&[bool]>,
    survey: &Survey,
    face: &Face,
    gap: f32,
    reach: f32,
) -> Vec<Vec3> {
    (0..positions.len())
        .into_par_iter()
        .with_min_len(MIN_PARALLEL)
        .map(|v| {
            let p = positions[v];
            let free = 1.0 - mask.get(v).copied().unwrap_or(0.0).clamp(0.0, 1.0);
            if !survey.inside.get(v).copied().unwrap_or(false) || free <= 0.0 || hidden.is_some_and(|h| h[v]) {
                return p;
            }
            match face.nearest(p, reach) {
                Some((at, normal)) => p + normal * ((gap - (p - at).dot(normal)) * free),
                None => p,
            }
        })
        .collect()
}
