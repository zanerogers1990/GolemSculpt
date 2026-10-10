//! The working parts the two cap triangulators share: ear clipping, Delaunay
//! edge flips and centroid refinement, all in a flat 2D chart.
//!
//! The recipe is the Unity app's (see `cap` for why each step is there). Two
//! things are done differently, for speed on the rims a dense sculpt produces,
//! without changing what comes out:
//!
//! - The ear clipper asks a grid which points could lie inside a candidate ear
//!   instead of testing every remaining point. A cut through a 20M-triangle
//!   model leaves a rim of ten thousand points and more, where the plain
//!   all-pairs test is hundreds of millions of point-in-triangle checks.
//! - The patch keeps each triangle's neighbours up to date, so a flip is a
//!   local edit and only the edges around a change are looked at again. The
//!   Unity version rebuilt an edge table from every triangle for every pass,
//!   which once took 4.4 s of a 4.4 s trim.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use glam::{Vec2, Vec3};

use crate::polygon::cross;

/// "No triangle": the far side of an edge that must not be flipped.
const NONE: u32 = u32::MAX;

/// Twice the area of an equilateral triangle of side 1 is sqrt(3)/2; its area
/// is 0.433. A triangle is split at 1.6 times the target rather than at the
/// target itself: a centroid split cuts area to a third, so a triangle a hair
/// over target comes back as three well under it, and splitting on the nose
/// overshoots the density it was aiming for by about half again.
pub(crate) fn split_area(target_edge: f32) -> f64 {
    target_edge as f64 * target_edge as f64 * 0.433 * 1.6
}

// ------------------------------------------------------------------ ear clipping

/// Ring positions bucketed by where their points are, so an ear only has to be
/// tested against the points near it. Clipped positions are taken out again.
struct PointGrid {
    min: Vec2,
    scale: Vec2,
    nx: usize,
    ny: usize,
    /// Cell `c` holds `items[start[c]..start[c] + live[c]]`.
    start: Vec<u32>,
    live: Vec<u32>,
    items: Vec<u32>,
}

impl PointGrid {
    fn new(points: &[Vec2], ring: &[u32]) -> PointGrid {
        let n = ring.len();
        let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
        for &i in ring {
            lo = lo.min(points[i as usize]);
            hi = hi.max(points[i as usize]);
        }
        let size = (hi - lo).max(Vec2::splat(1e-30));
        // About one point per cell, in cells shaped like the bounding box.
        let nx = ((n as f32 * size.x / size.y).sqrt().ceil() as usize).clamp(1, n);
        let ny = n.div_ceil(nx).max(1);
        let mut grid = PointGrid {
            min: lo,
            scale: Vec2::new(nx as f32, ny as f32) / size,
            nx,
            ny,
            start: vec![0; nx * ny + 1],
            live: vec![0; nx * ny],
            items: vec![0; n],
        };
        for &i in ring {
            let cell = grid.cell(points[i as usize]);
            grid.start[cell + 1] += 1;
        }
        for c in 0..nx * ny {
            grid.start[c + 1] += grid.start[c];
        }
        for (v, &i) in ring.iter().enumerate() {
            let cell = grid.cell(points[i as usize]);
            grid.items[(grid.start[cell] + grid.live[cell]) as usize] = v as u32;
            grid.live[cell] += 1;
        }
        grid
    }

    // A float-to-integer cast saturates, so a point outside the box (or not a
    // number at all) lands in an edge cell rather than out of range.
    #[inline]
    fn column(&self, x: f32) -> usize {
        (((x - self.min.x) * self.scale.x) as usize).min(self.nx - 1)
    }

    #[inline]
    fn row(&self, y: f32) -> usize {
        (((y - self.min.y) * self.scale.y) as usize).min(self.ny - 1)
    }

    #[inline]
    fn cell(&self, p: Vec2) -> usize {
        self.row(p.y) * self.nx + self.column(p.x)
    }

    fn remove(&mut self, v: u32, p: Vec2) {
        let cell = self.cell(p);
        let first = self.start[cell] as usize;
        let live = &mut self.items[first..first + self.live[cell] as usize];
        if let Some(at) = live.iter().position(|&item| item == v) {
            live.swap(at, live.len() - 1);
            self.live[cell] -= 1;
        }
    }
}

/// Clips ears off the polygon `points[ring[..]]` until it is all triangles.
///
/// `orientation` is +1 for a counter-clockwise ring and -1 for a clockwise
/// one; every emitted triangle has the ring's own orientation. A ring may
/// visit the same point index twice (the two ends of a bridge to a hole);
/// those copies are one point, not an obstruction to each other.
///
/// Ear clipping is quadratic on a well-behaved ring and cubic on a
/// pathological one, so `budget` bounds the point-in-triangle tests. Returns
/// whether the ring was clipped cleanly, every triangle an honest ear. When
/// the clip stalls or runs out instead, a `strict` call stops there, with
/// `out` holding whatever was clipped so far, and a lenient one closes
/// whatever is left with a fan: it can overlap itself, but it always closes.
pub(crate) fn ear_clip(
    points: &[Vec2],
    ring: &[u32],
    orientation: f64,
    mut budget: usize,
    strict: bool,
    out: &mut Vec<[u32; 3]>,
) -> bool {
    let n = ring.len();
    if n < 3 {
        return false;
    }
    let count = n as u32;
    let mut prev: Vec<u32> = (0..count).map(|i| (i + count - 1) % count).collect();
    let mut next: Vec<u32> = (0..count).map(|i| (i + 1) % count).collect();
    let mut grid = PointGrid::new(points, ring);
    let at = |v: u32| points[ring[v as usize] as usize];
    let mut emit = |a: u32, b: u32, c: u32| {
        let tri = [ring[a as usize], ring[b as usize], ring[c as usize]];
        if strict || (tri[0] != tri[1] && tri[1] != tri[2] && tri[2] != tri[0]) {
            out.push(tri);
        }
    };

    let (mut remaining, mut current, mut stalled) = (n, 0u32, 0usize);
    while remaining > 3 && stalled <= remaining && budget > 0 {
        let (p, c) = (prev[current as usize], next[current as usize]);
        if is_ear(points, ring, &grid, [p, current, c], orientation, &mut budget) {
            emit(p, current, c);
            next[p as usize] = c;
            prev[c as usize] = p;
            grid.remove(current, at(current));
            remaining -= 1;
            current = p;
            stalled = 0;
        } else {
            current = next[current as usize];
            stalled += 1;
        }
    }

    let (a, d) = (prev[current as usize], next[current as usize]);
    let clean = remaining == 3 && cross(at(a), at(current), at(d)) * orientation > 0.0;
    if strict && !clean {
        return false;
    }
    // The last triangle, or a fan over whatever a stalled clip left behind.
    let mut b = d;
    for _ in 0..remaining - 2 {
        let c = next[b as usize];
        emit(current, b, c);
        b = c;
    }
    clean
}

/// Whether the corner at ring position `ear[1]` can be cut off: it turns the
/// ring's own way and no other point of the ring lies in the triangle.
fn is_ear(
    points: &[Vec2],
    ring: &[u32],
    grid: &PointGrid,
    ear: [u32; 3],
    orientation: f64,
    budget: &mut usize,
) -> bool {
    let ids = ear.map(|v| ring[v as usize]);
    let [a, b, d] = ids.map(|i| points[i as usize]);
    if cross(a, b, d) * orientation <= 0.0 {
        return false; // reflex, or flat
    }
    let (lo, hi) = (a.min(b).min(d), a.max(b).max(d));
    for row in grid.row(lo.y)..=grid.row(hi.y) {
        for cell in row * grid.nx + grid.column(lo.x)..=row * grid.nx + grid.column(hi.x) {
            let first = grid.start[cell] as usize;
            for &v in &grid.items[first..first + grid.live[cell] as usize] {
                let id = ring[v as usize];
                if ear.contains(&v) || ids.contains(&id) {
                    continue;
                }
                let q = points[id as usize];
                if q.x < lo.x || q.x > hi.x || q.y < lo.y || q.y > hi.y {
                    continue;
                }
                *budget = budget.saturating_sub(1);
                if *budget == 0 {
                    return false;
                }
                let inside = cross(a, b, q) * orientation >= 0.0
                    && cross(b, d, q) * orientation >= 0.0
                    && cross(d, a, q) * orientation >= 0.0;
                if inside {
                    return false;
                }
            }
        }
    }
    true
}

// ---------------------------------------------------------------------- the patch

/// Where refinement measures size and puts new points, beyond the chart.
pub(crate) struct Space<'a> {
    /// One position per chart point; new points are appended.
    pub positions: Vec<Vec3>,
    /// Chart to surface. Without one a new point goes at the mean of the three
    /// corners around it, which only lands on the surface where that is flat.
    pub lift: Option<&'a (dyn Fn(Vec2) -> Vec3 + 'a)>,
    /// Whether a triangle's area in the chart counts toward its size as well.
    /// It should whenever the chart is in the surface's own units; it cannot
    /// where the chart stretches the surface by different amounts in different
    /// places.
    pub chart_counts: bool,
}

/// A triangulation being improved in place.
pub(crate) struct Patch {
    pub points: Vec<Vec2>,
    pub tris: Vec<[u32; 3]>,
    /// For each point `refine` added, in order, the corners of the triangle it
    /// was put at the centre of. Every corner is an earlier point, so a value
    /// known on the original points can be carried to the new ones in one
    /// forward sweep.
    pub parents: Vec<[u32; 3]>,
    /// `adjacent[t][k]` is the triangle on the far side of the edge from
    /// corner `k` to corner `k + 1`, or `NONE` for an edge that must stay.
    adjacent: Vec<[u32; 3]>,
    /// +1 when the triangles run counter-clockwise in the chart, -1 otherwise.
    orientation: f64,
    /// Whether the triangles might overlap one another, so that turning an
    /// edge has to check it would not double one that is already there (see
    /// `joined`).
    guarded: bool,
    /// Edges (a triangle and one of its corners) whose two triangles have
    /// changed since the edge was last tested, and may now want turning.
    stack: Vec<(u32, u8)>,
    /// Triangles made or changed since their size was last looked at. Going
    /// back over only these, and only the edges round them, is what keeps a
    /// round of refinement proportional to what the round before changed
    /// rather than to the whole face: a cut face of a hundred thousand
    /// triangles is otherwise swept in full fourteen times over.
    unmeasured: Vec<u32>,
}

impl Patch {
    /// `locked` names the edges that are constraints: rim edges, which hold
    /// the patch to the surface around it. Edges with no triangle on the far
    /// side, or with more than two triangles on them (a fan that overlapped
    /// itself), are never flipped either.
    ///
    /// `sound` is the caller's word that the triangles tile a region without
    /// overlapping: they were clipped cleanly from an outline that does not
    /// cross itself. Without it every flip is checked against doubling an
    /// edge, which costs about as much again as the flipping.
    pub fn new(
        points: Vec<Vec2>,
        tris: Vec<[u32; 3]>,
        orientation: f64,
        sound: bool,
        locked: impl Fn(u32, u32) -> bool,
    ) -> Patch {
        // Each directed edge and the triangle corner that walks it.
        const SHARED: u32 = u32::MAX;
        let mut walkers: HashMap<u64, u32> = HashMap::with_capacity(tris.len() * 3);
        let directed = |a: u32, b: u32| ((a as u64) << 32) | b as u64;
        for (t, tri) in tris.iter().enumerate() {
            for k in 0..3 {
                match walkers.entry(directed(tri[k], tri[(k + 1) % 3])) {
                    Entry::Vacant(slot) => {
                        slot.insert((t * 3 + k) as u32);
                    }
                    Entry::Occupied(mut slot) => *slot.get_mut() = SHARED,
                }
            }
        }
        let mut adjacent = vec![[NONE; 3]; tris.len()];
        for (t, tri) in tris.iter().enumerate() {
            for k in 0..3 {
                let (a, b) = (tri[k], tri[(k + 1) % 3]);
                if locked(a, b) || walkers[&directed(a, b)] == SHARED {
                    continue;
                }
                if let Some(&other) = walkers.get(&directed(b, a))
                    && other != SHARED
                {
                    adjacent[t][k] = other / 3;
                }
            }
        }
        // To begin with every edge is untested and every triangle unmeasured.
        let mut stack = Vec::with_capacity(tris.len() * 2);
        for (t, neighbours) in adjacent.iter().enumerate() {
            for (k, &n) in neighbours.iter().enumerate() {
                if n != NONE && (t as u32) < n {
                    stack.push((t as u32, k as u8));
                }
            }
        }
        let unmeasured = (0..tris.len() as u32).collect();
        Patch { points, tris, parents: Vec::new(), adjacent, orientation, guarded: !sound, stack, unmeasured }
    }

    /// Points `n`'s record of the edge it walks from `from` to `to` at a new
    /// neighbour.
    fn relink(&mut self, n: u32, from: u32, to: u32, neighbour: u32) {
        if n == NONE {
            return;
        }
        let tri = self.tris[n as usize];
        if let Some(k) = (0..3).find(|&k| tri[k] == from && tri[(k + 1) % 3] == to) {
            self.adjacent[n as usize][k] = neighbour;
        }
    }

    /// Turns the triangulation into the constrained Delaunay one by flipping
    /// every interior edge that fails the in-circle test, until none does.
    /// Returns whether anything was flipped.
    ///
    /// This is what removes the slivers: ear clipping produces a valid
    /// triangulation, not a good-looking one, and Delaunay is the
    /// triangulation that maximises the smallest angle. Locked edges are never
    /// flipped, and neither is an edge whose two triangles form a quad that is
    /// not convex: the replacement pair would overlap and stick out of the
    /// loop, which on a concave cut face means geometry outside the model.
    pub fn flip_to_delaunay(&mut self) -> bool {
        // Flipping is certain to finish in exact arithmetic. The cap is here
        // for the quads rounding cannot decide, so they cannot trade places
        // for ever.
        let mut allowance = self.tris.len() * 64 + 1024;
        let mut any = false;
        while let Some((t, k)) = self.stack.pop() {
            if allowance == 0 {
                self.stack.clear();
                break;
            }
            if self.flip(t, k as usize) {
                allowance -= 1;
                any = true;
            }
        }
        any
    }

    fn flip(&mut self, t0: u32, k: usize) -> bool {
        let t1 = self.adjacent[t0 as usize][k];
        if t1 == NONE {
            return false;
        }
        let tri0 = self.tris[t0 as usize];
        let (a, b, c) = (tri0[k], tri0[(k + 1) % 3], tri0[(k + 2) % 3]);
        let tri1 = self.tris[t1 as usize];
        let back = self.adjacent[t1 as usize];
        let Some(j) = (0..3).find(|&j| tri1[j] == b && tri1[(j + 1) % 3] == a && back[j] == t0) else {
            return false;
        };
        let d = tri1[(j + 2) % 3];

        // t0 = (a, b, c) and t1 = (b, a, d); the flip gives (a, d, c) and
        // (d, b, c), which is only sound if a-d-b-c is strictly convex.
        let [pa, pb, pc, pd] = [a, b, c, d].map(|i| self.points[i as usize]);
        let o = self.orientation;
        let convex = cross(pa, pd, pb) * o > 0.0
            && cross(pd, pb, pc) * o > 0.0
            && cross(pb, pc, pa) * o > 0.0
            && cross(pc, pa, pd) * o > 0.0;
        if !convex || !in_circle(pa, pb, pc, pd, o) || (self.guarded && self.joined(t0, c, d)) {
            return false;
        }

        let (n_bc, n_ca) = (self.adjacent[t0 as usize][(k + 1) % 3], self.adjacent[t0 as usize][(k + 2) % 3]);
        let (n_ad, n_db) = (back[(j + 1) % 3], back[(j + 2) % 3]);
        self.tris[t0 as usize] = [a, d, c];
        self.adjacent[t0 as usize] = [n_ad, t1, n_ca];
        self.tris[t1 as usize] = [d, b, c];
        self.adjacent[t1 as usize] = [n_db, n_bc, t0];
        self.relink(n_ad, d, a, t0);
        self.relink(n_bc, c, b, t1);
        self.stack.extend_from_slice(&[(t0, 0), (t0, 2), (t1, 0), (t1, 1)]);
        self.unmeasured.extend_from_slice(&[t0, t1]);
        true
    }

    /// Whether some triangle round `c` already joins it to `d`, starting the
    /// search from `t`, a triangle with `c` as a corner.
    ///
    /// In a sound triangulation the far corners of two triangles that share an
    /// edge and make a convex quad are never already joined. But a rim that
    /// crosses itself by a rounding error is closed with a fan that overlaps
    /// itself, and there they can be: turning the edge would then put a second
    /// edge between the same two points, four triangles round one edge, which
    /// is a surface no longer.
    fn joined(&self, t: u32, c: u32, d: u32) -> bool {
        for outgoing in [true, false] {
            let mut current = t;
            for _ in 0..self.tris.len() {
                let tri = self.tris[current as usize];
                let Some(k) = tri.iter().position(|&v| v == c) else {
                    break;
                };
                if tri.contains(&d) {
                    return true;
                }
                // Across the edge leaving `c`, or the one arriving at it.
                let next = self.adjacent[current as usize][if outgoing { k } else { (k + 2) % 3 }];
                if next == NONE || next == t {
                    break;
                }
                current = next;
            }
        }
        false
    }

    /// Splits every triangle bigger than `target_area` into three by putting a
    /// point at its centre, and returns how many points that added. Only the
    /// triangles present when the sweep began are looked at, lowest first, and
    /// of those only the ones made or changed since they were last measured:
    /// the rest were not over size then and are not now.
    ///
    /// A one-to-three centroid split rather than the more usual edge bisection
    /// because it touches no edge: rim edges cannot be subdivided, so the
    /// patch stays exactly welded to the shell. Bisecting would put a vertex
    /// in the middle of a rim edge that the shell's own triangle does not
    /// have: a T-junction, which is a visible crack.
    pub fn refine(&mut self, target_area: f64, budget: &mut usize, mut space: Option<&mut Space>) -> usize {
        let mut added = 0;
        let mut sweep = std::mem::take(&mut self.unmeasured);
        sweep.sort_unstable();
        sweep.dedup();
        for (done, &t) in sweep.iter().enumerate() {
            if *budget == 0 {
                self.unmeasured.extend_from_slice(&sweep[done..]);
                break;
            }
            let t = t as usize;
            let [i0, i1, i2] = self.tris[t];
            let [c0, c1, c2] = [i0, i1, i2].map(|i| self.points[i as usize]);
            let chart_area = cross(c0, c1, c2) * 0.5;
            // Over size in either the chart or in space. Area in space is the
            // honest measure of density, but it cannot see a triangle that is
            // huge across the face and merely happens to have its three
            // surface points nearly in line, and those are exactly the
            // triangles whose flat faces overlap their neighbours. The chart
            // is fitted to mesh units, so its area is comparable, and it
            // shrinks to exactly a third per split, which is what makes the
            // refinement certain to finish.
            let oversize = match &space {
                Some(space) => {
                    let [a, b, c] = [i0, i1, i2].map(|i| space.positions[i as usize]);
                    (b - a).cross(c - a).length() as f64 * 0.5 > target_area
                        || (space.chart_counts && chart_area.abs() > target_area)
                }
                None => chart_area * self.orientation > target_area,
            };
            if !oversize {
                continue;
            }

            // The centre in the chart, then that chart point put back onto the
            // surface. Averaging the three corners in space instead puts the
            // new vertex on the chord between them rather than on the surface,
            // and on a curved cut face that is the difference between a clean
            // cut and one with fangs across it.
            let centre = (c0 + c1 + c2) / 3.0;
            let m = self.points.len() as u32;
            self.points.push(centre);
            self.parents.push([i0, i1, i2]);
            if let Some(space) = space.as_deref_mut() {
                let [a, b, c] = [i0, i1, i2].map(|i| space.positions[i as usize]);
                space.positions.push(match space.lift {
                    Some(lift) => lift(centre),
                    None => (a + b + c) / 3.0,
                });
            }
            *budget -= 1;
            added += 1;

            // The parent's slot becomes the first child; the other two are new.
            let [n01, n12, n20] = self.adjacent[t];
            let (t, t1, t2) = (t as u32, self.tris.len() as u32, self.tris.len() as u32 + 1);
            self.tris[t as usize] = [i0, i1, m];
            self.adjacent[t as usize] = [n01, t1, t2];
            self.tris.push([i1, i2, m]);
            self.adjacent.push([n12, t2, t]);
            self.tris.push([i2, i0, m]);
            self.adjacent.push([n20, t, t1]);
            self.relink(n12, i2, i1, t1);
            self.relink(n20, i0, i2, t2);
            // Each child keeps one of the parent's edges, as its first.
            self.stack.extend_from_slice(&[(t, 0), (t1, 0), (t2, 0)]);
            self.unmeasured.extend_from_slice(&[t, t1, t2]);
        }
        added
    }

    /// Alternates a sweep of splits with a round of flips until nothing is
    /// over size and nothing wants turning.
    ///
    /// Splitting and flipping have to alternate: a sliver has almost no area,
    /// so a sweep leaves it alone, and the flip that later turns it into a fat
    /// triangle hands back something well over the target. Doing all the
    /// splitting first and flipping once at the end left cap triangles 60
    /// times the size of a shell triangle. And the loop ends only when a sweep
    /// found nothing to split and the flips after it found nothing to turn,
    /// the one state that is actually settled: flips are judged in the chart,
    /// so one can hand back a triangle well over the target in space.
    pub fn improve(&mut self, target_area: f64, mut budget: usize, mut space: Option<&mut Space>) {
        for _ in 0..MAX_REFINEMENT_ROUNDS {
            if budget == 0 {
                break;
            }
            let added = self.refine(target_area, &mut budget, space.as_deref_mut());
            let flipped = self.flip_to_delaunay();
            if added == 0 && !flipped {
                break;
            }
        }
    }
}

/// Each round splits every over-size triangle once, cutting its area to a
/// third, so this many rounds covers a cap starting out 3^14 times too coarse.
/// Eight was enough for a test sphere and not for a real sculpt: at 359k
/// triangles a lasso cut still had cap triangles 21 times a shell triangle
/// when it ran out, because the ear clipper's first triangles span a face that
/// wants twenty thousand triangles to fill.
const MAX_REFINEMENT_ROUNDS: usize = 14;

/// Whether `d` lies inside the circumcircle of triangle `abc`, whose winding is
/// `orientation`. The standard in-circle determinant, in double.
///
/// Four points that are on one circle as nearly as rounding can tell are left
/// alone: the determinant has to clear the noise in its own terms, or the two
/// ways of splitting such a quad would each look worse than the other.
fn in_circle(a: Vec2, b: Vec2, c: Vec2, d: Vec2, orientation: f64) -> bool {
    let (ax, ay) = (a.x as f64 - d.x as f64, a.y as f64 - d.y as f64);
    let (bx, by) = (b.x as f64 - d.x as f64, b.y as f64 - d.y as f64);
    let (cx, cy) = (c.x as f64 - d.x as f64, c.y as f64 - d.y as f64);
    let (a2, b2, c2) = (ax * ax + ay * ay, bx * bx + by * by, cx * cx + cy * cy);
    let det = a2 * (bx * cy - by * cx) - b2 * (ax * cy - ay * cx) + c2 * (ax * by - ay * bx);
    let noise = a2 * ((bx * cy).abs() + (by * cx).abs())
        + b2 * ((ax * cy).abs() + (ay * cx).abs())
        + c2 * ((ax * by).abs() + (ay * bx).abs());
    det * orientation > noise * 1e-12
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::polygon;
    use crate::testing::Rng;

    /// Checks a triangulation of a region tiles it exactly: every triangle
    /// wound `orientation`'s way, areas adding up to `area`, every interior
    /// edge walked once each way and every edge of `rim` exactly once.
    #[track_caller]
    pub(crate) fn assert_tiles(points: &[Vec2], tris: &[[u32; 3]], orientation: f64, area: f64, rim: &[(u32, u32)]) {
        let mut total = 0.0;
        let mut walked: HashMap<(u32, u32), u32> = HashMap::new();
        for tri in tris {
            let [a, b, c] = tri.map(|i| points[i as usize]);
            let doubled = cross(a, b, c) * orientation;
            assert!(doubled > 0.0, "triangle {tri:?} is flat or wound the wrong way");
            total += doubled * 0.5;
            for k in 0..3 {
                *walked.entry((tri[k], tri[(k + 1) % 3])).or_default() += 1;
            }
        }
        assert!((total - area).abs() <= area * 1e-4, "area {total} should be {area}");
        assert!(walked.values().all(|&n| n == 1), "an edge is walked twice the same way");
        let rim_edges: std::collections::HashSet<u64> = rim.iter().map(|&(a, b)| polygon::edge_key(a, b)).collect();
        for &(a, b) in walked.keys() {
            let has_twin = walked.contains_key(&(b, a));
            assert_eq!(has_twin, !rim_edges.contains(&polygon::edge_key(a, b)), "edge {a}-{b}");
        }
        assert_eq!(walked.len(), tris.len() * 3);
    }

    fn ring_edges(n: u32) -> Vec<(u32, u32)> {
        (0..n).map(|i| (i, (i + 1) % n)).collect()
    }

    fn comb() -> Vec<Vec2> {
        // Teeth pointing up from a base: reflex corners between every pair.
        let mut outline = vec![Vec2::new(0.0, 0.0), Vec2::new(12.0, 0.0)];
        for tooth in (0..6).rev() {
            let x = tooth as f32 * 2.0;
            outline.extend([
                Vec2::new(x + 2.0, 5.0),
                Vec2::new(x + 1.5, 1.0),
                Vec2::new(x + 0.5, 1.0),
                Vec2::new(x, 5.0),
            ]);
        }
        outline.dedup();
        outline
    }

    #[test]
    fn ear_clipping_fills_concave_rings_either_way_round() {
        for reversed in [false, true] {
            let mut outline = comb();
            if reversed {
                outline.reverse();
            }
            let area = polygon::signed_area(&outline);
            let orientation = area.signum();
            assert_eq!(orientation, if reversed { -1.0 } else { 1.0 });
            let ring: Vec<u32> = (0..outline.len() as u32).collect();
            for strict in [false, true] {
                let mut tris = Vec::new();
                assert!(ear_clip(&outline, &ring, orientation, 1_000_000, strict, &mut tris));
                assert_eq!(tris.len(), outline.len() - 2);
                assert_tiles(&outline, &tris, orientation, area.abs(), &ring_edges(ring.len() as u32));
            }
        }
    }

    #[test]
    fn the_smallest_rings_clip_too() {
        // Three points are one triangle, whichever way round and however the
        // grid of points comes out: a single cell, a single row, a single column.
        for points in [
            [Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0), Vec2::new(0.0, 1.0)],
            [Vec2::new(0.0, 0.0), Vec2::new(1e6, 1e-3), Vec2::new(5e5, 2e-3)],
            [Vec2::new(0.0, 0.0), Vec2::new(1e-3, 1e6), Vec2::new(-1e-3, 5e5)],
        ] {
            for strict in [false, true] {
                let mut tris = Vec::new();
                assert!(ear_clip(&points, &[0, 1, 2], 1.0, 100, strict, &mut tris));
                assert_eq!(tris, vec![[0, 1, 2]]);
            }
        }
        // Fewer is nothing, and three points in one place is no triangle,
        // though the lenient clip still hands the ring back closed.
        let mut tris = Vec::new();
        assert!(!ear_clip(&[Vec2::ZERO, Vec2::X], &[0, 1], 1.0, 100, false, &mut tris));
        assert!(tris.is_empty());
        let same = [Vec2::ONE; 3];
        assert!(!ear_clip(&same, &[0, 1, 2], 1.0, 100, true, &mut tris));
        assert!(tris.is_empty());
        assert!(!ear_clip(&same, &[0, 1, 2], 1.0, 100, false, &mut tris));
        assert_eq!(tris.len(), 1);
        // A square is the smallest ring with a choice to make.
        let square = [Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y];
        let mut tris = Vec::new();
        assert!(ear_clip(&square, &[0, 1, 2, 3], 1.0, 100, true, &mut tris));
        assert_tiles(&square, &tris, 1.0, 1.0, &ring_edges(4));
    }

    #[test]
    fn a_ring_that_cannot_be_clipped_fails_strictly_and_fans_leniently() {
        // A bow tie: the outline crosses itself, so no clean triangulation exists.
        let outline =
            [Vec2::new(0.0, 0.0), Vec2::new(4.0, 3.0), Vec2::new(4.0, 0.0), Vec2::new(0.0, 3.0), Vec2::new(-1.0, 1.5)];
        let ring: Vec<u32> = (0..5).collect();
        let orientation = if polygon::signed_area(&outline) >= 0.0 { 1.0 } else { -1.0 };
        let mut tris = Vec::new();
        ear_clip(&outline, &ring, orientation, 1_000_000, false, &mut tris);
        assert_eq!(tris.len(), 3, "a fan always closes the ring");
        // Out of budget on an honest ring: the same two outcomes.
        let outline = comb();
        let ring: Vec<u32> = (0..outline.len() as u32).collect();
        let mut strict = Vec::new();
        assert!(!ear_clip(&outline, &ring, 1.0, 3, true, &mut strict));
        let mut lenient = Vec::new();
        assert!(!ear_clip(&outline, &ring, 1.0, 3, false, &mut lenient), "closed, but not cleanly");
        assert_eq!(lenient.len(), outline.len() - 2);
    }

    #[test]
    fn the_grid_finds_the_same_ears_as_testing_every_point() {
        // A wobbly ring of many points: any ear the grid wrongly allowed would
        // leave a triangle with a point inside it and the tiling would fail.
        let mut rng = Rng::new(11);
        let n = 3000;
        let outline: Vec<Vec2> = (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                let r = 100.0 + 30.0 * (a * 9.0).sin() + rng.range(-0.4, 0.4);
                Vec2::new(a.cos() * r * 1.7, a.sin() * r)
            })
            .collect();
        let ring: Vec<u32> = (0..n as u32).collect();
        let mut tris = Vec::new();
        assert!(ear_clip(&outline, &ring, 1.0, 64_000_000, true, &mut tris));
        assert_tiles(&outline, &tris, 1.0, polygon::signed_area(&outline), &ring_edges(n as u32));
    }

    /// The share of triangles with a corner sharper than `degrees`.
    pub(crate) fn sliver_share(points: &[Vec2], tris: &[[u32; 3]], degrees: f32) -> f32 {
        let slivers = tris.iter().filter(|tri| {
            let p = tri.map(|i| points[i as usize]);
            (0..3).any(|k| {
                let (u, v) = (p[(k + 1) % 3] - p[k], p[(k + 2) % 3] - p[k]);
                u.angle_to(v).abs().to_degrees() < degrees
            })
        });
        slivers.count() as f32 / tris.len() as f32
    }

    #[test]
    fn flips_reach_delaunay_and_keep_the_tiling() {
        // Points on a circle, ear-clipped into a fan of slivers, with one point
        // in the middle added by refinement: flipping must fatten it.
        let n = 48;
        let outline: Vec<Vec2> = (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                Vec2::new(a.cos(), a.sin()) * (10.0 + (i % 3) as f32 + 0.5 * (i % 5) as f32)
            })
            .collect();
        let ring: Vec<u32> = (0..n as u32).collect();
        let area = polygon::signed_area(&outline);
        let mut tris = Vec::new();
        assert!(ear_clip(&outline, &ring, 1.0, 1_000_000, true, &mut tris));
        let rim = n as u32;
        let is_rim = |u: u32, v: u32| u < rim && v < rim && (u.abs_diff(v) == 1 || u.abs_diff(v) == rim - 1);
        let mut patch = Patch::new(outline.clone(), tris, 1.0, true, is_rim);
        patch.flip_to_delaunay();
        assert!(!patch.flip_to_delaunay(), "a second round finds nothing left to turn");
        assert_tiles(&patch.points, &patch.tris, 1.0, area, &ring_edges(rim));

        // With nothing but the rim to work with, most triangles are slivers.
        // Filling the face at a set density is what fattens them.
        let before = sliver_share(&patch.points, &patch.tris, 20.0);
        patch.improve(split_area(2.0), 10_000, None);
        assert!(patch.points.len() > n * 2, "refinement added points: {}", patch.points.len());
        assert_eq!(patch.parents.len(), patch.points.len() - n);
        assert_tiles(&patch.points, &patch.tris, 1.0, area, &ring_edges(rim));
        let largest = patch.tris.iter().map(|t| {
            cross(patch.points[t[0] as usize], patch.points[t[1] as usize], patch.points[t[2] as usize]) * 0.5
        });
        assert!(largest.fold(0.0, f64::max) <= split_area(2.0));
        let after = sliver_share(&patch.points, &patch.tris, 20.0);
        assert!(after < before * 0.5 && after < 0.25, "slivers went from {before} to {after}");
        for (i, parents) in patch.parents.iter().enumerate() {
            assert!(parents.iter().all(|&p| (p as usize) < n + i), "a parent comes before its child");
        }
    }

    #[test]
    fn locally_delaunay_after_flipping() {
        let mut rng = Rng::new(3);
        // A square with random interior points put in by splitting whichever
        // triangle holds each one is beyond this module; instead refine a
        // square hard and check every interior edge passes the circle test.
        let outline = [Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.3), Vec2::new(10.4, 9.0), Vec2::new(-0.5, 10.0)];
        let wobble: Vec<Vec2> =
            outline.iter().map(|p| *p + Vec2::new(rng.range(-0.2, 0.2), rng.range(-0.2, 0.2))).collect();
        let mut tris = Vec::new();
        assert!(ear_clip(&wobble, &[0, 1, 2, 3], 1.0, 1000, true, &mut tris));
        let mut patch =
            Patch::new(wobble, tris, 1.0, true, |u, v| u < 4 && v < 4 && (u.abs_diff(v) == 1 || u.abs_diff(v) == 3));
        patch.improve(split_area(0.5), 100_000, None);
        assert!(patch.tris.len() > 400);
        let mut owners: HashMap<u64, Vec<usize>> = HashMap::new();
        for (t, tri) in patch.tris.iter().enumerate() {
            for k in 0..3 {
                owners.entry(polygon::edge_key(tri[k], tri[(k + 1) % 3])).or_default().push(t);
            }
        }
        let mut checked = 0;
        for pair in owners.values().filter(|o| o.len() == 2) {
            let (t0, t1) = (patch.tris[pair[0]], patch.tris[pair[1]]);
            let d = *t1.iter().find(|v| !t0.contains(v)).unwrap();
            let [a, b, c, d] = [t0[0], t0[1], t0[2], d].map(|i| patch.points[i as usize]);
            assert!(!in_circle(a, b, c, d, 1.0), "an interior edge still fails the circle test");
            checked += 1;
        }
        assert!(checked > 500);
    }
}
