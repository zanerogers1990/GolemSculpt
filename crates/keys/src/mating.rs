//! Finds the faces along which two parts were cut apart: where the surface of
//! one lies on the surface of the other, facing the opposite way.
//!
//! The splitter knows the cut faces of a split it has just made, but the key
//! tool is handed two objects, not a split: halves from a saved scene, parts
//! cut in another program and imported, a split made a moment ago by another
//! tool. This works them out from the geometry alone.
//!
//! Coincidence is tested triangle by triangle against a hash grid of the
//! other part: a triangle mates when its centre and all three corners lie
//! within `tolerance` of the other part's surface, on a triangle facing back
//! at it. The two faces of one cut are triangulated independently, so this
//! compares against surfaces, never against vertices.

use glam::{IVec3, Vec3};
use rayon::prelude::*;
use sculpt_geom::edge_key::edge_key;
use sculpt_geom::{Aabb, tri, units};

use crate::joint::KeyPart;

/// How squarely two triangles must face each other to mate.
const OPPOSITE_COS: f32 = -0.95;
/// A mating face smaller than this many triangles is a coincidental touch.
const MIN_FACE_TRIANGLES: usize = 8;

/// Fills `a.cut_faces` and `b.cut_faces` with the mating faces found
/// (connected pieces, largest first) and returns their pairing as (index
/// into `a.cut_faces`, index into `b.cut_faces`). `tolerance` is a world
/// distance; zero or less picks one from the parts' size.
pub fn find(a: &mut KeyPart, b: &mut KeyPart, tolerance: f32) -> Vec<(usize, usize)> {
    a.cut_faces.clear();
    b.cut_faces.clear();
    if a.triangle_count() == 0 || b.triangle_count() == 0 {
        return Vec::new();
    }
    let (bounds_a, bounds_b) = (Aabb::of(a.world()), Aabb::of(b.world()));
    let tolerance = if tolerance > 0.0 {
        tolerance
    } else {
        (bounds_a.size().length().max(bounds_b.size().length()) * 1e-5).max(units::from_millimetres(0.002))
    };

    // Only the region where the two could touch.
    let lo = bounds_a.min.max(bounds_b.min) - Vec3::splat(tolerance);
    let hi = bounds_a.max.min(bounds_b.max) + Vec3::splat(tolerance);
    if lo.cmpgt(hi).any() {
        return Vec::new();
    }

    let (faces_a, faces_b, partner_a) = {
        let (grid_a, grid_b) =
            rayon::join(|| TriangleHash::new(a, lo, hi, tolerance), || TriangleHash::new(b, lo, hi, tolerance));
        let ((mating_a, partner_a), (mating_b, _)) =
            rayon::join(|| mating(a, &grid_b, lo, hi, tolerance), || mating(b, &grid_a, lo, hi, tolerance));
        (pieces(a, &mating_a), pieces(b, &mating_b), partner_a)
    };

    // Which B face each B triangle ended up in, then a vote per A face.
    let mut face_of_b = vec![u32::MAX; b.triangle_count()];
    for (f, face) in faces_b.iter().enumerate() {
        for &t in face {
            face_of_b[t as usize] = f as u32;
        }
    }
    let mut pairs = Vec::new();
    let mut taken = vec![false; faces_b.len()];
    for (f, face) in faces_a.iter().enumerate() {
        let mut votes = vec![0usize; faces_b.len()];
        for &t in face {
            let partner = partner_a[t as usize];
            if partner != u32::MAX && face_of_b[partner as usize] != u32::MAX {
                votes[face_of_b[partner as usize] as usize] += 1;
            }
        }
        // The most votes; the lower face on a tie.
        let free = (0..faces_b.len()).filter(|&fb| votes[fb] > 0 && !taken[fb]);
        let best = free.max_by_key(|&fb| (votes[fb], usize::MAX - fb));
        if let Some(best) = best {
            taken[best] = true;
            pairs.push((f, best));
        }
    }
    a.cut_faces = faces_a;
    b.cut_faces = faces_b;
    pairs
}

/// The summed area of the faces in `pairs` on part `a`: how much of the two
/// parts touches, for choosing between several candidates.
pub fn mating_area(a: &KeyPart, pairs: &[(usize, usize)]) -> f32 {
    pairs
        .iter()
        .flat_map(|&(fa, _)| a.cut_faces[fa].iter())
        .map(|&t| {
            let [p0, p1, p2] = a.corners(t);
            (p1 - p0).cross(p2 - p0).length() as f64 * 0.5
        })
        .sum::<f64>() as f32
}

/// Which triangles of `part` lie on the other part's surface, facing it; and
/// for each, the other part's triangle it lies on (`u32::MAX` for none).
fn mating(part: &KeyPart, other: &TriangleHash, lo: Vec3, hi: Vec3, tolerance: f32) -> (Vec<bool>, Vec<u32>) {
    let partner: Vec<u32> = (0..part.triangle_count() as u32)
        .into_par_iter()
        .with_min_len(1024)
        .map(|t| {
            let [p0, p1, p2] = part.corners(t);
            let c = (p0 + p1 + p2) / 3.0;
            if c.cmplt(lo).any() || c.cmpgt(hi).any() {
                return u32::MAX;
            }
            let cross = (p1 - p0).cross(p2 - p0);
            let len = cross.length();
            if len <= 0.0 {
                return u32::MAX;
            }
            let Some(hit) = other.nearest(c, cross / len, tolerance) else { return u32::MAX };
            let far = |p: Vec3| other.distance_near(p, hit) > tolerance * 2.0;
            if far(p0) || far(p1) || far(p2) { u32::MAX } else { hit }
        })
        .collect();
    (partner.iter().map(|&p| p != u32::MAX).collect(), partner)
}

/// Edge-connected pieces of the flagged triangles, largest first; slivers of
/// a few triangles (a coincidental touch) are dropped.
fn pieces(part: &KeyPart, flagged: &[bool]) -> Vec<Vec<u32>> {
    let indices = part.indices();
    let mut edges: Vec<(u64, u32)> = flagged
        .par_iter()
        .enumerate()
        .filter(|&(_, &on)| on)
        .flat_map_iter(|(t, _)| {
            let c = &indices[t * 3..t * 3 + 3];
            (0..3).map(move |k| (edge_key(c[k], c[(k + 1) % 3]), t as u32))
        })
        .collect();
    edges.par_sort_unstable();

    // Triangles sharing an edge are one piece.
    let mut parent: Vec<u32> = (0..flagged.len() as u32).collect();
    fn root(parent: &mut [u32], mut x: u32) -> u32 {
        while parent[x as usize] != x {
            parent[x as usize] = parent[parent[x as usize] as usize];
            x = parent[x as usize];
        }
        x
    }
    for pair in edges.windows(2) {
        if pair[0].0 == pair[1].0 {
            let (a, b) = (root(&mut parent, pair[0].1), root(&mut parent, pair[1].1));
            // The lower triangle is the root, so the pieces come out the same
            // whatever order the edges were met in.
            parent[a.max(b) as usize] = a.min(b);
        }
    }

    let mut slot = vec![u32::MAX; flagged.len()];
    let mut found: Vec<Vec<u32>> = Vec::new();
    for t in (0..flagged.len() as u32).filter(|&t| flagged[t as usize]) {
        let r = root(&mut parent, t) as usize;
        if slot[r] == u32::MAX {
            slot[r] = found.len() as u32;
            found.push(Vec::new());
        }
        found[slot[r] as usize].push(t);
    }
    found.retain(|piece| piece.len() >= MIN_FACE_TRIANGLES);
    // Stable, so pieces of one size stay in the order of their first triangle.
    found.sort_by_key(|piece| std::cmp::Reverse(piece.len()));
    found
}

/// A part's triangles binned in a uniform grid over the contact region: the
/// cells as sorted keys, each with a run of the triangles that reach it.
struct TriangleHash<'a> {
    part: &'a KeyPart,
    lo: Vec3,
    cell: f32,
    keys: Vec<u64>,
    /// `starts[k]..starts[k + 1]` of `triangles` are in cell `keys[k]`.
    starts: Vec<u32>,
    triangles: Vec<u32>,
}

impl<'a> TriangleHash<'a> {
    fn new(part: &'a KeyPart, lo: Vec3, hi: Vec3, tolerance: f32) -> TriangleHash<'a> {
        let count = part.triangle_count();
        // A sample of edges is enough to size the cells by.
        let stride = (count / 4096).max(1);
        let sampled = (0..count).step_by(stride);
        let samples = sampled.len().max(1);
        let edge: f64 = sampled.map(|t| part.corners(t as u32)).map(|[p0, p1, _]| p0.distance(p1) as f64).sum();
        let cell = ((edge / samples as f64) as f32 * 2.0).max(tolerance * 4.0).max(1e-6);

        let cell_of = |p: Vec3| ((p - lo) / cell).floor().as_ivec3();
        // Nothing asks about a cell outside the region, so a triangle that
        // only passes through it is not filed in the cells beyond.
        let (first, last) = (cell_of(lo) - IVec3::ONE, cell_of(hi) + IVec3::ONE);
        let mut filed: Vec<(u64, u32)> = (0..count as u32)
            .into_par_iter()
            .with_min_len(1024)
            .flat_map_iter(|t| {
                let [p0, p1, p2] = part.corners(t);
                let mn = p0.min(p1).min(p2) - Vec3::splat(tolerance);
                let mx = p0.max(p1).max(p2) + Vec3::splat(tolerance);
                let outside = mx.cmplt(lo).any() || mn.cmpgt(hi).any();
                let (a, b) = if outside {
                    (IVec3::ONE, IVec3::ZERO)
                } else {
                    (cell_of(mn).max(first), cell_of(mx).min(last))
                };
                (a.x..=b.x).flat_map(move |x| {
                    (a.y..=b.y).flat_map(move |y| (a.z..=b.z).map(move |z| (cell_key(IVec3::new(x, y, z)), t)))
                })
            })
            .collect();
        filed.par_sort_unstable();

        let mut keys = Vec::new();
        let mut starts = Vec::new();
        for (i, &(key, _)) in filed.iter().enumerate() {
            if keys.last() != Some(&key) {
                keys.push(key);
                starts.push(i as u32);
            }
        }
        starts.push(filed.len() as u32);
        TriangleHash { part, lo, cell, keys, starts, triangles: filed.into_iter().map(|(_, t)| t).collect() }
    }

    /// The triangles filed in the cell `p` falls in.
    fn near(&self, p: Vec3) -> &[u32] {
        let key = cell_key(((p - self.lo) / self.cell).floor().as_ivec3());
        match self.keys.binary_search(&key) {
            Ok(k) => &self.triangles[self.starts[k] as usize..self.starts[k + 1] as usize],
            Err(_) => &[],
        }
    }

    /// The nearest triangle within `tolerance` of `p` that faces against
    /// `normal`.
    fn nearest(&self, p: Vec3, normal: Vec3, tolerance: f32) -> Option<u32> {
        let mut best = None;
        let mut best_distance = tolerance;
        for &t in self.near(p) {
            let [p0, p1, p2] = self.part.corners(t);
            let cross = (p1 - p0).cross(p2 - p0);
            let len = cross.length();
            if len <= 0.0 || (cross / len).dot(normal) > OPPOSITE_COS {
                continue;
            }
            let d = tri::closest_point(p, p0, p1, p2).distance(p);
            if d <= best_distance {
                best_distance = d;
                best = Some(t);
            }
        }
        best
    }

    /// Distance from `p` to triangle `t`, or to the nearest triangle in `p`'s
    /// cell that faces the same way as `t`: a corner of a mating triangle can
    /// sit over the next triangle of the other face.
    fn distance_near(&self, p: Vec3, t: u32) -> f32 {
        let [a, b, c] = self.part.corners(t);
        let mut d = tri::closest_point(p, a, b, c).distance(p);
        let normal = (b - a).cross(c - a).normalize_or_zero();
        for &other in self.near(p) {
            let [p0, p1, p2] = self.part.corners(other);
            if (p1 - p0).cross(p2 - p0).normalize_or_zero().dot(normal) < 0.9 {
                continue;
            }
            d = d.min(tri::closest_point(p, p0, p1, p2).distance(p));
        }
        d
    }
}

/// One number for a grid cell: 21 bits a coordinate.
fn cell_key(c: IVec3) -> u64 {
    let part = |v: i32| (v as u32 & 0x1f_ffff) as u64;
    (part(c.x) << 42) | (part(c.y) << 21) | part(c.z)
}
