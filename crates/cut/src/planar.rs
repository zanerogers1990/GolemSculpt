//! Triangulates flat polygons with holes and brings the result up to the
//! density of the surface around it.
//!
//! `cap::fill_loop` only takes a single loop. A cut through a hollow part, or
//! through a ring, leaves a face that is a polygon with holes; so does the
//! ring of cut face left around a registration key once its footprint is taken
//! out.
//!
//! Holes are merged into the outer loop with bridges, the textbook way: each
//! hole is joined to the outline by a pair of coincident edges running out and
//! back, which turns the region into one (weakly) simple polygon an ordinary
//! ear clipper can eat. The bridge is the closest vertex pair that crosses
//! nothing.
//!
//! The clean-up afterwards is `cap`'s recipe, for the reasons given there:
//! constrained Delaunay flips to remove the ear clipper's slivers, then
//! alternating rounds of centroid splits and flips until nothing is over size.
//! Centroid splits never touch an edge, so every loop edge survives exactly,
//! which is what keeps the patch welded to whatever is around it.
//!
//! Everything here is in the 2D chart the caller hands in; mapping new points
//! back into 3D is the caller's job (it knows what surface the chart is of).

use std::collections::HashSet;

use glam::{Vec2, Vec3};
use rayon::prelude::*;

use crate::patch::{Patch, Space, ear_clip, split_area};
use crate::polygon::{cross, edge_key, segments_cross, signed_area_of};

/// Bound on the ear clipper's point-in-triangle tests. It exists so a
/// self-intersecting outline fails in bounded time instead of hanging.
const EAR_TEST_BUDGET: usize = 40_000_000;

/// What a caller is told when a region will not triangulate.
pub const CANNOT_TRIANGULATE: &str = "the outline crosses itself or one of its holes";

/// Most pairs of points (one on the outline, one on a hole) the last-resort
/// search for a bridge will list: twelve bytes each, so 48 MB, and as much
/// again to sort them.
///
/// The search is only reached when no point of a hole can be joined to the
/// outline point nearest it, which on a sound region is next to never: it is
/// what a hole lying outside its outline, or tangled with it, comes to, and
/// now and then a hole beside a slit, whose nearest outline points are all
/// across it. Listing every pair for the two rims of a cut through a dense
/// model, 100,000 points each, asked for 120 GB. Within the limit are a hole
/// of 64 points in an outline of 62,000 (a key's footing in a large cut
/// face) and two rims of 2,000 points each.
///
/// Past the limit the search is not given up: it goes on a hole point at a
/// time instead, which needs no list (`bridge_from_each_hole_point`).
pub const MAX_BRIDGE_PAIRS: usize = 4_000_000;

/// How much work the search for a bridge past [`MAX_BRIDGE_PAIRS`] may do
/// before it is given up, counted in outline and hole edges looked at: a
/// pair turned down by the corners at its two ends counts one, a pair that
/// has to be held against every edge counts them all.
///
/// A sound region needs a sliver of it. The limit is for the hole that
/// cannot be joined, where every pair fails and there are ten thousand
/// million of them between two rims of 100,000: a few seconds' work shared
/// between the threads, where trying them all would be hours.
const BRIDGE_SEARCH_WORK: u64 = 1 << 34;

/// The share of [`BRIDGE_SEARCH_WORK`] one hole point may use before the
/// search moves on to the next: one in this many.
const BRIDGE_POINT_SHARE: u64 = 32;

/// Hole points searched at once, each on a thread of its own.
const BRIDGE_POINTS_AT_ONCE: usize = 64;

/// What a caller is told when a hole would not join to its outline, the two
/// are too large for [`MAX_BRIDGE_PAIRS`], and the search that takes its
/// place ran out of [`BRIDGE_SEARCH_WORK`] before it had tried every pair.
/// The region is then not triangulated, as for [`CANNOT_TRIANGULATE`].
pub const TOO_LARGE_TO_BRIDGE: &str =
    "a hole in the cut face would not join to its outline, and the two have too many points to try every pair";

/// Triangulates the region inside `outer` and outside every loop in `holes`,
/// all given as indices into `points`. Loops may run either way round. The
/// triangles (three indices each) are counter-clockwise in the chart.
///
/// Fails when the region cannot be triangulated cleanly: a loop crosses itself
/// or another loop, or a hole is not inside the outline. The caller must not
/// use the region then.
pub fn triangulate(points: &[Vec2], outer: &[u32], holes: &[Vec<u32>]) -> Result<Vec<u32>, String> {
    triangulate_with(points, outer, holes, true)
}

/// As `triangulate`, for a caller that has to close the region whatever shape
/// it is in. Where the loops touch or cross (by a rounding error, where a cut
/// ran through slivers) and no clean triangulation exists, whatever cannot be
/// clipped is closed with a fan: the triangles there may overlap, but they
/// still make one sheet with exactly the given loops as its edge. Fails only
/// when a hole cannot be joined to the outline at all.
pub fn triangulate_lenient(points: &[Vec2], outer: &[u32], holes: &[Vec<u32>]) -> Result<Vec<u32>, String> {
    triangulate_with(points, outer, holes, false)
}

/// `triangulate` if that works and `triangulate_lenient` if it does not, with
/// the same result as asking for each in turn. The holes are joined to the
/// outline once for both: a hole that cannot be joined cannot be joined the
/// second time either, and looking twice was half of what such a region
/// cost.
pub fn triangulate_somehow(points: &[Vec2], outer: &[u32], holes: &[Vec<u32>]) -> Result<Vec<u32>, String> {
    let ring = joined(points, outer, holes)?;
    clipped(points, &ring, true).or_else(|_| clipped(points, &ring, false))
}

fn triangulate_with(points: &[Vec2], outer: &[u32], holes: &[Vec<u32>], strict: bool) -> Result<Vec<u32>, String> {
    clipped(points, &joined(points, outer, holes)?, strict)
}

/// The outline with every hole joined into it by a bridge: one loop, counter
/// clockwise, that an ear clipper can eat.
fn joined(points: &[Vec2], outer: &[u32], holes: &[Vec<u32>]) -> Result<Vec<u32>, String> {
    let fail = || CANNOT_TRIANGULATE.to_string();
    if outer.len() < 3 {
        return Err(fail());
    }
    let mut ring = outer.to_vec();
    if signed_area_of(points, &ring) < 0.0 {
        ring.reverse();
    }
    let mut pending: Vec<Vec<u32>> = holes.iter().filter(|hole| hole.len() >= 3).cloned().collect();
    for hole in &mut pending {
        // Clockwise, so that walking it keeps the region on the left: the same
        // convention as the outline, which lets one cone test serve both.
        if signed_area_of(points, hole) > 0.0 {
            hole.reverse();
        }
    }
    // Rightmost hole first is the classic order; any order works with a
    // crossing-checked bridge, but this one keeps later bridges short.
    let max_x = |hole: &Vec<u32>| hole.iter().map(|&i| points[i as usize].x).fold(f32::MIN, f32::max);
    pending.sort_by(|a, b| max_x(b).total_cmp(&max_x(a)));
    for i in 0..pending.len() {
        bridge(points, &mut ring, &pending[i], &pending[i + 1..])?;
    }
    Ok(ring)
}

/// The triangles of a loop `joined` made. Strictly, or (when the loop
/// touches or crosses itself) with whatever cannot be clipped closed by a
/// fan: see `triangulate_lenient`.
fn clipped(points: &[Vec2], ring: &[u32], strict: bool) -> Result<Vec<u32>, String> {
    let mut triangles = Vec::with_capacity(ring.len());
    if !ear_clip(points, ring, 1.0, EAR_TEST_BUDGET, strict, &mut triangles) && strict {
        return Err(CANNOT_TRIANGULATE.to_string());
    }
    Ok(triangles.into_flattened())
}

/// Every edge of the given loops, as the set `improve` must not flip.
pub fn loop_edges(outer: &[u32], holes: &[Vec<u32>]) -> HashSet<u64> {
    let mut edges = HashSet::new();
    for ring in std::iter::once(outer).chain(holes.iter().map(Vec::as_slice)) {
        for i in 0..ring.len() {
            edges.insert(edge_key(ring[i], ring[(i + 1) % ring.len()]));
        }
    }
    edges
}

/// Constrained Delaunay flips, then (when `target_edge` is positive)
/// alternating centroid splits and flips until no triangle is much over the
/// area of an equilateral triangle of that side.
///
/// `triangles` must be counter-clockwise, as `triangulate` leaves them. New
/// points are appended to `points`, at most `point_budget` of them; the caller
/// maps them into 3D. Returns, for each new point in order, the three earlier
/// points it was placed in the middle of.
pub fn improve(
    points: &mut Vec<Vec2>,
    triangles: &mut Vec<u32>,
    constraints: &HashSet<u64>,
    target_edge: f32,
    point_budget: usize,
) -> Vec<[u32; 3]> {
    let tris: Vec<[u32; 3]> = triangles.chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect();
    // Nothing here has shown that the loops keep clear of one another, so the
    // triangles are not taken to be free of overlaps.
    let mut patch = Patch::new(std::mem::take(points), tris, 1.0, false, |u, v| constraints.contains(&edge_key(u, v)));
    patch.flip_to_delaunay();
    if target_edge > 0.0 {
        patch.improve(split_area(target_edge), point_budget, None);
    }
    *points = patch.points;
    *triangles = patch.tris.into_flattened();
    patch.parents
}

/// As `improve`, for a chart that does not keep the surface's proportions:
/// size is judged in space alone, and each new point is placed in space by
/// `lift` as it is made. `positions` holds one position per point and is
/// extended in step with `points`.
pub fn improve_on_surface(
    points: &mut Vec<Vec2>,
    positions: &mut Vec<Vec3>,
    triangles: &mut Vec<u32>,
    constraints: &HashSet<u64>,
    lift: &dyn Fn(Vec2) -> Vec3,
    target_edge: f32,
    point_budget: usize,
) -> Vec<[u32; 3]> {
    let tris: Vec<[u32; 3]> = triangles.chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect();
    let mut patch = Patch::new(std::mem::take(points), tris, 1.0, false, |u, v| constraints.contains(&edge_key(u, v)));
    let mut space = Space { positions: std::mem::take(positions), lift: Some(lift), chart_counts: false };
    patch.flip_to_delaunay();
    if target_edge > 0.0 {
        patch.improve(split_area(target_edge), point_budget, Some(&mut space));
    }
    *points = patch.points;
    *positions = space.positions;
    *triangles = patch.tris.into_flattened();
    patch.parents
}

// ---------------------------------------------------------------------- bridging

/// Joins `hole` into `ring` along the shortest segment that crosses no edge of
/// the ring, the hole, or any hole still waiting to be joined, and that leaves
/// both of its ends into the region rather than into a hole or out of the
/// outline.
///
/// The Unity version sorted every ring-hole vertex pair by distance, which for
/// a cut through a dense model is tens of millions of pairs to sort for a
/// bridge that is almost always the very first one tried. Here each hole
/// vertex first offers only its own nearest ring vertex. If none of those
/// will do, every pair is listed nearest first, when there are few enough to
/// list ([`MAX_BRIDGE_PAIRS`]); and when there are not, each hole vertex in
/// turn is tried against the whole ring, nearest first, which finds a bridge
/// wherever there is one without listing anything, though not always the
/// shortest of them.
///
/// Every search here is shared between threads and takes the first pair
/// that will do in the order a single thread would have come to them, so the
/// answer never depends on how the work fell out.
///
/// Fails, with the reason, when no pair will do, or when the search was given
/// up for the work it was taking ([`TOO_LARGE_TO_BRIDGE`]).
fn bridge(points: &[Vec2], ring: &mut Vec<u32>, hole: &[u32], pending: &[Vec<u32>]) -> Result<(), String> {
    let (n, m) = (ring.len(), hole.len());
    let pair = Pair { points, ring: ring.as_slice(), hole, pending };

    // Each hole vertex with the ring position nearest it, nearest pair first.
    let mut nearest = nearest_on_ring(points, ring, hole);
    nearest.sort_by(|a, b| a.0.total_cmp(&b.0));
    // An earlier bridge leaves its ring end in the ring twice, as one point
    // with two different pairs of neighbours; either may serve. Ring positions
    // in order of the point they hold find the copies without a walk round
    // the ring for every hole vertex.
    let mut by_point: Vec<u32> = (0..n as u32).collect();
    by_point.sort_unstable_by_key(|&k| (ring[k as usize], k));
    let copies = |i: u32| {
        let point = ring[i as usize];
        let from = by_point.partition_point(|&k| ring[k as usize] < point);
        by_point[from..].iter().map(|&k| k as usize).take_while(move |&k| pair.ring[k] == point)
    };
    let mut found = nearest
        .par_iter()
        .find_map_first(|&(_, i, j)| copies(i).find(|&copy| pair.joins(copy, j as usize)).map(|copy| (copy, j as usize)));
    if found.is_none() {
        found = if n.checked_mul(m).is_some_and(|pairs| pairs <= MAX_BRIDGE_PAIRS) {
            let mut all: Vec<(f32, u32, u32)> = (0..n * m)
                .into_par_iter()
                .map(|k| (pair.at_ring(k / m).distance_squared(pair.at_hole(k % m)), (k / m) as u32, (k % m) as u32))
                .collect();
            all.par_sort_by(|a, b| a.0.total_cmp(&b.0));
            let first = all.par_iter().find_first(|&&(_, i, j)| pair.joins(i as usize, j as usize));
            first.map(|&(_, i, j)| (i as usize, j as usize))
        } else {
            bridge_from_each_hole_point(&pair, &nearest)?
        };
    }
    let Some((i, j)) = found else {
        return Err(CANNOT_TRIANGULATE.to_string());
    };

    // The ring up to i, the hole from j all the way round and back to j, then
    // i again and the rest of the ring.
    let mut spliced = Vec::with_capacity(n + m + 2);
    spliced.extend_from_slice(&ring[..=i]);
    spliced.extend((0..=m).map(|k| hole[(j + k) % m]));
    spliced.extend_from_slice(&ring[i..]);
    *ring = spliced;
    Ok(())
}

/// A ring, a hole to join into it, and the holes still waiting: what a
/// bridge between a ring position and a hole position is judged against.
#[derive(Clone, Copy)]
struct Pair<'a> {
    points: &'a [Vec2],
    ring: &'a [u32],
    hole: &'a [u32],
    pending: &'a [Vec<u32>],
}

impl Pair<'_> {
    fn at_ring(&self, i: usize) -> Vec2 {
        self.points[self.ring[i] as usize]
    }

    fn at_hole(&self, j: usize) -> Vec2 {
        self.points[self.hole[j] as usize]
    }

    /// Whether a bridge from ring position `i` to hole position `j` leaves
    /// both of its ends into the region: a look at the two corners, no more.
    fn leaves_into_the_region(&self, i: usize, j: usize) -> bool {
        let (n, m) = (self.ring.len(), self.hole.len());
        let (p, q) = (self.at_ring(i), self.at_hole(j));
        self.ring[i] != self.hole[j]
            && p != q
            && in_cone(self.at_ring((i + n - 1) % n), p, self.at_ring((i + 1) % n), q)
            && in_cone(self.at_hole((j + m - 1) % m), q, self.at_hole((j + 1) % m), p)
    }

    /// Whether that bridge crosses no edge of the ring, the hole or any hole
    /// still waiting: a walk along all of them.
    fn crosses_nothing(&self, i: usize, j: usize) -> bool {
        let (pi, qj) = (self.ring[i], self.hole[j]);
        let (p, q) = (self.at_ring(i), self.at_hole(j));
        !crosses_loop(self.points, self.ring, p, q, pi, qj)
            && !crosses_loop(self.points, self.hole, p, q, pi, qj)
            && !self.pending.iter().any(|other| crosses_loop(self.points, other, p, q, pi, qj))
    }

    fn joins(&self, i: usize, j: usize) -> bool {
        self.leaves_into_the_region(i, j) && self.crosses_nothing(i, j)
    }

    /// Edges a bridge has to be held against.
    fn edges(&self) -> u64 {
        (self.ring.len() + self.hole.len() + self.pending.iter().map(Vec::len).sum::<usize>()) as u64
    }
}

/// For each point of `hole`, in order: how far (squared) the nearest point
/// of `ring` is, its position in the ring, and the hole point's own
/// position. Of several ring points equally near, the first in the ring.
fn nearest_on_ring(points: &[Vec2], ring: &[u32], hole: &[u32]) -> Vec<(f32, u32, u32)> {
    let at = |i: u32| points[i as usize];
    (0..hole.len())
        .into_par_iter()
        .map(|j| {
            let q = at(hole[j]);
            let (mut best, mut best_i) = (f32::MAX, 0);
            for (i, &r) in ring.iter().enumerate() {
                let d = at(r).distance_squared(q);
                if d < best {
                    (best, best_i) = (d, i);
                }
            }
            (best, best_i as u32, j as u32)
        })
        .collect()
}

/// The search for a bridge when there are too many pairs to list: each hole
/// point in turn, nearest to the ring first, against every position of the
/// ring in order of distance from it. Nothing is kept but one hole point's
/// distances at a time.
///
/// `Ok(None)` when every pair was tried and none will do. `Err` when the
/// work allowed ([`BRIDGE_SEARCH_WORK`]) ran out first.
///
/// The work is counted, not timed, and each hole point's search is given
/// what it may spend before it starts, so that the answer is the same on
/// any machine and however the threads share it out.
fn bridge_from_each_hole_point(pair: &Pair, nearest: &[(f32, u32, u32)]) -> Result<Option<(usize, usize)>, String> {
    let n = pair.ring.len();
    let (edges, share) = (pair.edges(), BRIDGE_SEARCH_WORK / BRIDGE_POINT_SHARE);
    // What one hole point found, what looking cost, and whether it looked at every position of the ring.
    let search = |j: usize| -> (Option<usize>, u64, bool) {
        let q = pair.at_hole(j);
        let mut order: Vec<(f32, u32)> = (0..n).map(|i| (pair.at_ring(i).distance_squared(q), i as u32)).collect();
        order.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        // Measuring and sorting them, near enough.
        let mut spent = 8 * n as u64;
        for &(_, i) in &order {
            if !pair.leaves_into_the_region(i as usize, j) {
                spent += 1;
                continue;
            }
            if spent + edges > share {
                return (None, spent, false);
            }
            spent += edges;
            if pair.crosses_nothing(i as usize, j) {
                return (Some(i as usize), spent, true);
            }
        }
        (None, spent, true)
    };

    let (mut left, mut every_pair_tried) = (BRIDGE_SEARCH_WORK, true);
    for together in nearest.chunks(BRIDGE_POINTS_AT_ONCE) {
        let searched: Vec<_> = together.par_iter().map(|&(_, _, j)| search(j as usize)).collect();
        for (&(_, _, j), (found, spent, whole_ring)) in together.iter().zip(searched) {
            if let Some(i) = found {
                return Ok(Some((i, j as usize)));
            }
            every_pair_tried &= whole_ring;
            if spent >= left {
                return Err(TOO_LARGE_TO_BRIDGE.to_string());
            }
            left -= spent;
        }
    }
    if every_pair_tried { Ok(None) } else { Err(TOO_LARGE_TO_BRIDGE.to_string()) }
}

/// Whether segment `pq` properly crosses any edge of `ring`, or runs through
/// one of its vertices. Edges and vertices that are `p` or `q` themselves (by
/// index) are the segment's own ends and do not count.
fn crosses_loop(points: &[Vec2], ring: &[u32], p: Vec2, q: Vec2, pi: u32, qi: u32) -> bool {
    let n = ring.len();
    let (lo, hi) = (p.min(q), p.max(q));
    for k in 0..n {
        let (a, b) = (ring[k], ring[(k + 1) % n]);
        let (pa, pb) = (points[a as usize], points[b as usize]);
        if pa.x.max(pb.x) < lo.x || pa.x.min(pb.x) > hi.x || pa.y.max(pb.y) < lo.y || pa.y.min(pb.y) > hi.y {
            continue;
        }
        if a != pi && a != qi && on_segment(pa, p, q) {
            return true;
        }
        if a == pi || a == qi || b == pi || b == qi {
            continue;
        }
        if segments_cross(p, q, pa, pb) {
            return true;
        }
    }
    false
}

/// Whether `x` lies on the open segment `ab`.
fn on_segment(x: Vec2, a: Vec2, b: Vec2) -> bool {
    let ab = b - a;
    let len = ab.length_squared();
    if len <= 0.0 {
        return false;
    }
    let t = (x - a).dot(ab) / len;
    t > 0.0 && t < 1.0 && (a + ab * t - x).length_squared() <= len * 1e-12
}

/// Whether the direction from `a` toward `b` points into the region at `a`:
/// between the edges `prev -> a` and `a -> next`, on their left. O'Rourke's
/// InCone, for a loop with the region on its left.
fn in_cone(prev: Vec2, a: Vec2, next: Vec2, b: Vec2) -> bool {
    if cross(a, next, prev) >= 0.0 {
        // Convex (or straight) at a.
        return cross(a, b, prev) > 0.0 && cross(b, a, next) > 0.0;
    }
    !(cross(a, b, next) >= 0.0 && cross(b, a, prev) >= 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch::tests::{assert_tiles, sliver_share};
    use crate::polygon::signed_area;

    /// Appends a regular polygon and returns its loop.
    fn add_ring(points: &mut Vec<Vec2>, centre: Vec2, radius: f32, sides: u32, clockwise: bool) -> Vec<u32> {
        let first = points.len() as u32;
        for i in 0..sides {
            let turn = if clockwise { -(i as f32) } else { i as f32 };
            let a = turn / sides as f32 * std::f32::consts::TAU + 0.2;
            points.push(centre + Vec2::new(a.cos(), a.sin()) * radius);
        }
        (first..first + sides).collect()
    }

    fn loop_area(points: &[Vec2], ring: &[u32]) -> f64 {
        signed_area_of(points, ring).abs()
    }

    fn rim_of(loops: &[&[u32]]) -> Vec<(u32, u32)> {
        loops.iter().flat_map(|ring| (0..ring.len()).map(|i| (ring[i], ring[(i + 1) % ring.len()]))).collect()
    }

    fn triples(triangles: &[u32]) -> Vec<[u32; 3]> {
        triangles.chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect()
    }

    #[test]
    fn an_annulus_is_filled_between_its_loops() {
        // Every pairing of windings: the triangulator sorts them out itself.
        for (outer_cw, hole_cw) in [(false, true), (false, false), (true, true), (true, false)] {
            let mut points = Vec::new();
            let outer = add_ring(&mut points, Vec2::new(5.0, 5.0), 4.0, 40, outer_cw);
            let hole = add_ring(&mut points, Vec2::new(5.5, 4.8), 2.0, 24, hole_cw);
            let triangles = triangulate(&points, &outer, std::slice::from_ref(&hole)).unwrap();
            // A polygon of n vertices with one hole of m: n + m triangles.
            assert_eq!(triangles.len() / 3, 40 + 24);
            let area = loop_area(&points, &outer) - loop_area(&points, &hole);
            assert_tiles(&points, &triples(&triangles), 1.0, area, &rim_of(&[&outer, &hole]));
        }
    }

    #[test]
    fn several_holes_and_a_concave_outline() {
        let mut points = vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(20.0, 0.0),
            Vec2::new(20.0, 12.0),
            Vec2::new(12.0, 12.0),
            Vec2::new(12.0, 6.0),
            Vec2::new(8.0, 6.0),
            Vec2::new(8.0, 12.0),
            Vec2::new(0.0, 12.0),
        ];
        let outer: Vec<u32> = (0..8).collect();
        let holes = vec![
            add_ring(&mut points, Vec2::new(4.0, 8.0), 2.0, 12, true),
            add_ring(&mut points, Vec2::new(16.0, 8.5), 2.5, 16, false),
            add_ring(&mut points, Vec2::new(10.0, 3.0), 1.5, 9, true),
            add_ring(&mut points, Vec2::new(4.0, 2.5), 1.2, 5, false),
        ];
        let mut triangles = triangulate(&points, &outer, &holes).unwrap();
        let area = loop_area(&points, &outer) - holes.iter().map(|h| loop_area(&points, h)).sum::<f64>();
        let mut loops: Vec<&[u32]> = vec![&outer];
        loops.extend(holes.iter().map(Vec::as_slice));
        let rim = rim_of(&loops);
        // n vertices and h holes: n + 2h - 2 triangles.
        assert_eq!(triangles.len() / 3, points.len() + 2 * 4 - 2);
        assert_tiles(&points, &triples(&triangles), 1.0, area, &rim);

        // Brought up to density: still an exact tiling with every loop edge
        // intact, new points only inside the region, and far fewer slivers
        // (those left lie along loop edges much longer than the target, which
        // may not be split).
        let original = points.len();
        let before = sliver_share(&points, &triples(&triangles), 15.0);
        let parents = improve(&mut points, &mut triangles, &loop_edges(&outer, &holes), 0.6, 100_000);
        assert_eq!(parents.len(), points.len() - original);
        assert!(points.len() > original + 300, "{} points added", points.len() - original);
        let tris = triples(&triangles);
        assert_tiles(&points, &tris, 1.0, area, &rim);
        let outline: Vec<Vec2> = outer.iter().map(|&i| points[i as usize]).collect();
        for p in &points[original..] {
            assert!(crate::polygon::contains(&outline, *p), "{p} is outside the outline");
            for hole in &holes {
                let ring: Vec<Vec2> = hole.iter().map(|&i| points[i as usize]).collect();
                assert!(!crate::polygon::contains(&ring, *p), "{p} is inside a hole");
            }
        }
        let after = sliver_share(&points, &tris, 15.0);
        assert!(after < before * 0.3, "slivers went from {before} to {after}");
        let largest =
            tris.iter().map(|t| cross(points[t[0] as usize], points[t[1] as usize], points[t[2] as usize]) * 0.5);
        assert!(largest.fold(0.0, f64::max) <= split_area(0.6));
    }

    #[test]
    fn a_hole_inside_a_hole_is_not_a_hole_of_the_outline() {
        // The caller pairs loops up by nesting; handed a hole that pokes out of
        // the outline, the triangulator must refuse rather than fill rubbish.
        let mut points = Vec::new();
        let outer = add_ring(&mut points, Vec2::ZERO, 3.0, 20, false);
        let outside = add_ring(&mut points, Vec2::new(10.0, 0.0), 1.0, 8, true);
        let crossing = add_ring(&mut points, Vec2::new(3.0, 0.0), 1.0, 8, true);
        assert_eq!(triangulate(&points, &outer, &[outside]).unwrap_err(), CANNOT_TRIANGULATE);
        assert!(triangulate(&points, &outer, &[crossing]).is_err());
        assert!(triangulate(&points, &outer[..2], &[]).is_err());
        // With no holes it is a plain polygon.
        let plain = triangulate(&points, &outer, &[]).unwrap();
        assert_tiles(&points, &triples(&plain), 1.0, signed_area(&points[..20]), &rim_of(&[&outer]));
    }

    #[test]
    fn a_hole_touching_the_outline_is_still_closed_leniently() {
        // The hole's corner rests on the outline's edge: no clean triangulation
        // of what is left exists, but the lenient one is still a single sheet
        // whose edge is the two loops, each edge once.
        let points = vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(10.0, 0.0),
            Vec2::new(10.0, 10.0),
            Vec2::new(0.0, 10.0),
            Vec2::new(5.0, 0.0),
            Vec2::new(7.0, 4.0),
            Vec2::new(3.0, 4.0),
        ];
        let (outer, hole) = (vec![0, 1, 2, 3], vec![4, 5, 6]);
        let triangles = triangulate_lenient(&points, &outer, std::slice::from_ref(&hole)).unwrap();
        let mut walked = std::collections::HashMap::new();
        for t in triangles.chunks_exact(3) {
            for k in 0..3 {
                *walked.entry((t[k], t[(k + 1) % 3])).or_insert(0) += 1;
            }
        }
        assert!(walked.values().all(|&times| times == 1));
        let rim: HashSet<u64> = rim_of(&[&outer, &hole]).iter().map(|&(a, b)| edge_key(a, b)).collect();
        for &(a, b) in walked.keys() {
            assert_eq!(walked.contains_key(&(b, a)), !rim.contains(&edge_key(a, b)), "edge {a}-{b}");
        }
        assert_eq!(walked.keys().filter(|&&(a, b)| rim.contains(&edge_key(a, b))).count(), 7);
        // And where a clean answer exists, lenient and strict agree.
        let mut clear = points.clone();
        clear[4] = Vec2::new(5.0, 1.0);
        let strict = triangulate(&clear, &outer, std::slice::from_ref(&hole)).unwrap();
        assert_eq!(triangulate_lenient(&clear, &outer, std::slice::from_ref(&hole)).unwrap(), strict);
    }

    /// A hole that is not inside its outline at all, both of them dense: no
    /// hole point can be joined to the outline point nearest it, so every
    /// pair used to be listed and sorted, 4.4 million of them here (53 MB,
    /// and as much again for the sort), and then tried one by one, to no end.
    /// The two rims of a cut through a dense model are fifty times longer
    /// each. Nothing is listed now, and every pair is still tried: the answer
    /// is that there is no bridge, and it comes quickly.
    #[test]
    fn a_hole_that_will_not_bridge_is_found_out_without_listing_its_pairs() {
        let mut points = Vec::new();
        let outer = add_ring(&mut points, Vec2::ZERO, 50.0, 2100, false);
        let outside = add_ring(&mut points, Vec2::new(200.0, 0.0), 30.0, 2100, true);
        assert!(outer.len() * outside.len() > MAX_BRIDGE_PAIRS);
        let started = std::time::Instant::now();
        for lenient in [false, true] {
            let tried = match lenient {
                false => triangulate(&points, &outer, std::slice::from_ref(&outside)),
                true => triangulate_lenient(&points, &outer, std::slice::from_ref(&outside)),
            };
            assert_eq!(tried.unwrap_err(), CANNOT_TRIANGULATE);
        }
        assert!(started.elapsed().as_secs_f32() < 4.0, "took {:?}", started.elapsed());

        // The same mistake small enough to list is listed, and says the same.
        let mut points = Vec::new();
        let outer = add_ring(&mut points, Vec2::ZERO, 50.0, 300, false);
        let outside = add_ring(&mut points, Vec2::new(200.0, 0.0), 30.0, 300, true);
        assert_eq!(triangulate(&points, &outer, &[outside]).unwrap_err(), CANNOT_TRIANGULATE);
    }

    /// Past what can be tried in the work allowed, the search is given up
    /// and says so: two rims of 100,000, of which one is not inside the
    /// other, are ten thousand million pairs that all fail, and trying each
    /// against 200,000 edges would be hours. It is the work that is counted,
    /// so the answer does not depend on how fast the machine is.
    #[test]
    fn a_hole_that_will_not_bridge_is_given_up_when_trying_every_pair_would_take_too_long() {
        let mut points = Vec::new();
        let outer = add_ring(&mut points, Vec2::ZERO, 50.0, 100_000, false);
        let outside = add_ring(&mut points, Vec2::new(200.0, 0.0), 30.0, 100_000, true);
        let started = std::time::Instant::now();
        assert_eq!(triangulate_somehow(&points, &outer, &[outside]).unwrap_err(), TOO_LARGE_TO_BRIDGE);
        assert!(started.elapsed().as_secs_f32() < 20.0, "took {:?}", started.elapsed());
    }

    /// The limit is on listing pairs only. Rims far over it that bridge at
    /// the first try are as welcome as ever, and so is a small hole in a
    /// very long outline when the search is needed.
    #[test]
    fn the_limit_on_pairs_leaves_ordinary_regions_alone() {
        let mut points = Vec::new();
        let outer = add_ring(&mut points, Vec2::ZERO, 50.0, 6000, false);
        let hole = add_ring(&mut points, Vec2::ZERO, 30.0, 6000, true);
        assert!(outer.len() * hole.len() > 8 * MAX_BRIDGE_PAIRS);
        let triangles = triangulate(&points, &outer, std::slice::from_ref(&hole)).unwrap();
        assert_eq!(triangles.len() / 3, 12_000);

        // A hole outside a long outline, few enough pairs to list: searched, and found wanting.
        let mut points = Vec::new();
        let outer = add_ring(&mut points, Vec2::ZERO, 50.0, 2000, false);
        let outside = add_ring(&mut points, Vec2::new(200.0, 0.0), 1.0, 64, true);
        assert!(outer.len() * outside.len() <= MAX_BRIDGE_PAIRS);
        assert_eq!(triangulate(&points, &outer, &[outside]).unwrap_err(), CANNOT_TRIANGULATE);
    }

    /// A square with a slit let into it from the left, and a round hole just
    /// under the slit. The slit's far wall has `wall` points along it and its
    /// near wall none but its two ends, so the outline point nearest every
    /// point of the hole is across the slit, where no bridge can go: the
    /// first that can is found only after nearly every pair has been tried.
    fn slit_and_hole(wall: u32, hole: u32) -> (Vec<Vec2>, Vec<u32>, Vec<u32>) {
        let mut points = vec![Vec2::new(0.0, 0.0), Vec2::new(100.0, 0.0), Vec2::new(100.0, 100.0), Vec2::new(0.0, 100.0)];
        points.extend((0..wall).map(|i| Vec2::new(90.0 * i as f32 / (wall - 1) as f32, 50.1)));
        points.extend([Vec2::new(90.0, 50.0), Vec2::new(0.0, 50.0)]);
        let outer: Vec<u32> = (0..points.len() as u32).collect();
        let ring = add_ring(&mut points, Vec2::new(45.0, 46.0), 3.0, hole, true);
        (points, outer, ring)
    }

    fn slit_area(points: &[Vec2], outer: &[u32], hole: &[u32]) -> f64 {
        loop_area(points, outer) - loop_area(points, hole)
    }

    /// The search of last resort is not given up for size. With 4,000,000
    /// pairs as the most it would list, a hole of 2000 points beside a slit
    /// in an outline of 1997 was triangulated and one in an outline of 2007
    /// refused as having "too many points to try every pair". Both are
    /// triangulated, and so is one fifty times the limit.
    #[test]
    fn a_hole_whose_nearest_outline_points_are_all_across_a_slit_is_bridged_at_any_size() {
        for (wall, hole) in [(1991, 2000), (2001, 2000), (2600, 2600)] {
            let (points, outer, ring) = slit_and_hole(wall, hole);
            let pairs = outer.len() * ring.len();
            let triangles = triangulate(&points, &outer, std::slice::from_ref(&ring))
                .unwrap_or_else(|error| panic!("{} x {} ({pairs} pairs): {error}", outer.len(), ring.len()));
            assert_eq!(triangles.len() / 3, outer.len() + ring.len());
            let area = slit_area(&points, &outer, &ring);
            assert_tiles(&points, &triples(&triangles), 1.0, area, &rim_of(&[&outer, &ring]));
        }
        let (_, outer, ring) = slit_and_hole(2001, 2000);
        assert!(outer.len() * ring.len() > MAX_BRIDGE_PAIRS, "the second case is meant to be over the limit");

        // Fifty times the limit: the hole is joined to the outline, by a bridge that crosses nothing. (The ear
        // clipper has a limit of its own on a loop this long, which is not what this is about.)
        let (points, outer, hole) = slit_and_hole(20_000, 10_000);
        // As `joined` takes it: clockwise, which `add_ring` was asked for.
        let clockwise = hole.clone();
        assert!(signed_area_of(&points, &clockwise) < 0.0);
        let ring = joined(&points, &outer, std::slice::from_ref(&hole)).unwrap();
        assert_eq!(ring.len(), outer.len() + hole.len() + 2);
        let leaves_the_outline = |i: &usize| ring[i + 1] != outer[(i + 1) % outer.len()];
        let from = (0..outer.len()).find(leaves_the_outline).expect("the hole is in the ring");
        let pair = Pair { points: &points, ring: &outer, hole: &clockwise, pending: &[] };
        let to = clockwise.iter().position(|&q| q == ring[from + 1]).expect("the bridge ends on the hole");
        assert!(pair.joins(from, to), "the bridge from {from} to {to} is not one");
    }

    /// The bridge search as it was first written, one pair at a time on one
    /// thread, with every pair listed whatever that takes: what `bridge`
    /// has to agree with wherever the two can be compared.
    fn bridge_plainly(points: &[Vec2], ring: &[u32], hole: &[u32], pending: &[Vec<u32>]) -> Option<Vec<u32>> {
        let (n, m) = (ring.len(), hole.len());
        let at = |i: u32| points[i as usize];
        let try_pair = |i: usize, j: usize| -> bool {
            let (pi, qj) = (ring[i], hole[j]);
            let (p, q) = (at(pi), at(qj));
            pi != qj
                && p != q
                && in_cone(at(ring[(i + n - 1) % n]), p, at(ring[(i + 1) % n]), q)
                && in_cone(at(hole[(j + m - 1) % m]), q, at(hole[(j + 1) % m]), p)
                && !crosses_loop(points, ring, p, q, pi, qj)
                && !crosses_loop(points, hole, p, q, pi, qj)
                && !pending.iter().any(|other| crosses_loop(points, other, p, q, pi, qj))
        };
        let mut nearest: Vec<(f32, usize, usize)> = Vec::new();
        for j in 0..m {
            let (mut best, mut best_i) = (f32::MAX, 0);
            for (i, &r) in ring.iter().enumerate() {
                let d = at(r).distance_squared(at(hole[j]));
                if d < best {
                    (best, best_i) = (d, i);
                }
            }
            nearest.push((best, best_i, j));
        }
        nearest.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut found = None;
        'search: for &(_, i, j) in &nearest {
            for copy in (0..n).filter(|&k| ring[k] == ring[i]) {
                if try_pair(copy, j) {
                    found = Some((copy, j));
                    break 'search;
                }
            }
        }
        if found.is_none() {
            let mut all: Vec<(f32, usize, usize)> =
                (0..n * m).map(|k| (at(ring[k / m]).distance_squared(at(hole[k % m])), k / m, k % m)).collect();
            all.sort_by(|a, b| a.0.total_cmp(&b.0));
            found = all.iter().map(|&(_, i, j)| (i, j)).find(|&(i, j)| try_pair(i, j));
        }
        let (i, j) = found?;
        let mut spliced = ring[..=i].to_vec();
        spliced.extend((0..=m).map(|k| hole[(j + k) % m]));
        spliced.extend_from_slice(&ring[i..]);
        Some(spliced)
    }

    /// Regions to bridge, each as its points, its outline and its holes (the
    /// holes clockwise, rightmost first, as `triangulate_with` hands them
    /// on): the shapes of the other tests, the slit under the limit on
    /// listed pairs, holes that cannot be joined, and scatterings of holes
    /// in a wavy outline with points that coincide to a bit.
    fn regions_to_bridge() -> Vec<(Vec<Vec2>, Vec<u32>, Vec<Vec<u32>>)> {
        let mut regions = Vec::new();
        let mut annulus = Vec::new();
        let outer = add_ring(&mut annulus, Vec2::new(5.0, 5.0), 4.0, 40, false);
        let hole = add_ring(&mut annulus, Vec2::new(5.5, 4.8), 2.0, 24, true);
        regions.push((annulus, outer, vec![hole]));
        for (wall, hole) in [(40, 30), (500, 700), (1900, 2000)] {
            let (points, outer, ring) = slit_and_hole(wall, hole);
            regions.push((points, outer, vec![ring]));
        }
        for (centre, radius) in [(Vec2::new(200.0, 0.0), 30.0), (Vec2::new(50.0, 0.0), 10.0), (Vec2::ZERO, 60.0)] {
            let mut points = Vec::new();
            let outer = add_ring(&mut points, Vec2::ZERO, 50.0, 300, false);
            let hole = add_ring(&mut points, centre, radius, 200, true);
            regions.push((points, outer, vec![hole]));
        }
        let mut rng = crate::testing::Rng::new(11);
        for sides in [7, 60, 400] {
            let mut points: Vec<Vec2> = (0..sides)
                .map(|i| {
                    let a = i as f32 / sides as f32 * std::f32::consts::TAU;
                    Vec2::new(a.cos(), a.sin()) * (40.0 + 6.0 * (a * 5.0).sin())
                })
                .collect();
            let outer: Vec<u32> = (0..sides).collect();
            let mut holes = Vec::new();
            for (k, x) in [-24.0f32, -8.0, 8.0, 24.0].into_iter().enumerate() {
                let centre = Vec2::new(x, rng.range(-10.0, 10.0));
                // Whole numbers of sides and round radii, so that some distances come out exactly equal.
                holes.push(add_ring(&mut points, centre, 4.0 + k as f32, 8 << k, true));
            }
            holes.sort_by(|a, b| {
                let max_x = |hole: &Vec<u32>| hole.iter().map(|&i| points[i as usize].x).fold(f32::MIN, f32::max);
                max_x(b).total_cmp(&max_x(a))
            });
            regions.push((points, outer, holes));
        }
        regions
    }

    /// Whatever is done to make the search quicker or smaller, it joins
    /// each hole to the outline where the plain search does, to the index.
    #[test]
    fn every_bridge_is_the_one_the_plain_search_finds() {
        let (mut bridged, mut refused) = (0, 0);
        for (points, outer, holes) in regions_to_bridge() {
            let (mut ring, mut plain) = (outer.clone(), outer.clone());
            for (h, hole) in holes.iter().enumerate() {
                assert!(ring.len() * hole.len() <= MAX_BRIDGE_PAIRS, "this is for regions within the limit");
                let expected = bridge_plainly(&points, &plain, hole, &holes[h + 1..]);
                let got = bridge(&points, &mut ring, hole, &holes[h + 1..]);
                match expected {
                    Some(spliced) => {
                        assert_eq!(got, Ok(()), "{} points, hole {h}", points.len());
                        assert!(ring == spliced, "{} points, hole {h}: joined somewhere else", points.len());
                        plain = spliced;
                        bridged += 1;
                    }
                    None => {
                        assert_eq!(got, Err(CANNOT_TRIANGULATE.to_string()), "{} points, hole {h}", points.len());
                        refused += 1;
                        break;
                    }
                }
            }
        }
        assert!(bridged >= 15 && refused >= 2, "{bridged} bridged, {refused} refused");
    }

    /// Clean if it can be, lenient if not, and the same either way as asking
    /// for each in turn; the holes are joined to the outline once for both.
    #[test]
    fn somehow_is_strict_then_lenient_with_the_holes_joined_once() {
        let in_turn = |points: &[Vec2], outer: &[u32], holes: &[Vec<u32>]| {
            triangulate(points, outer, holes).or_else(|_| triangulate_lenient(points, outer, holes))
        };
        // A clean region; one whose hole touches its outline, which only the lenient way closes; one that
        // cannot be joined at all.
        let (points, outer, ring) = slit_and_hole(60, 40);
        let clean = triangulate_somehow(&points, &outer, std::slice::from_ref(&ring)).unwrap();
        assert_eq!(clean, triangulate(&points, &outer, std::slice::from_ref(&ring)).unwrap());
        let touching = vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(10.0, 0.0),
            Vec2::new(10.0, 10.0),
            Vec2::new(0.0, 10.0),
            Vec2::new(5.0, 0.0),
            Vec2::new(7.0, 4.0),
            Vec2::new(3.0, 4.0),
        ];
        let (square, corner) = (vec![0, 1, 2, 3], vec![vec![4, 5, 6]]);
        assert!(triangulate(&touching, &square, &corner).is_err());
        let lenient = triangulate_somehow(&touching, &square, &corner).unwrap();
        assert_eq!(lenient, triangulate_lenient(&touching, &square, &corner).unwrap());
        assert_eq!(Ok(lenient), in_turn(&touching, &square, &corner));
        let mut apart = Vec::new();
        let outer = add_ring(&mut apart, Vec2::ZERO, 50.0, 300, false);
        let outside = vec![add_ring(&mut apart, Vec2::new(200.0, 0.0), 30.0, 300, true)];
        assert_eq!(triangulate_somehow(&apart, &outer, &outside), in_turn(&apart, &outer, &outside));
        assert_eq!(triangulate_somehow(&apart, &outer, &outside).unwrap_err(), CANNOT_TRIANGULATE);
    }

    /// What a hole that cannot be joined costs before it is given up, at the
    /// size of the two rims of a cut through a dense model, asked for the way
    /// a cap asks. `cargo test -p sculpt-cut --release doomed -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn timing_of_a_doomed_bridge_between_two_long_rims() {
        for sides in [2100, 20_000, 100_000] {
            let mut points = Vec::new();
            let outer = add_ring(&mut points, Vec2::ZERO, 50.0, sides, false);
            let outside = vec![add_ring(&mut points, Vec2::new(200.0, 0.0), 30.0, sides, true)];
            let started = std::time::Instant::now();
            let error = triangulate_somehow(&points, &outer, &outside).unwrap_err();
            println!("two rims of {sides}: {:.2} s to \"{error}\"", started.elapsed().as_secs_f32());
        }
    }

    /// What joining two sound rims costs, at the size of a cut through a
    /// dense tube. `cargo test -p sculpt-cut --release sound_bridge -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn timing_of_a_sound_bridge_between_two_long_rims() {
        for sides in [4000, 20_000, 100_000] {
            let mut points = Vec::new();
            let mut ring = add_ring(&mut points, Vec2::ZERO, 50.0, sides, false);
            let hole = add_ring(&mut points, Vec2::ZERO, 30.0, sides, true);
            let started = std::time::Instant::now();
            bridge(&points, &mut ring, &hole, &[]).unwrap();
            println!("two rims of {sides}: joined in {:.3} s", started.elapsed().as_secs_f32());
        }
    }

    #[test]
    fn dense_rings_bridge_without_sorting_every_pair() {
        // Two concentric rings of a few thousand points each: the size a cut
        // through a tube leaves. Would be 16M pairs if every one were sorted.
        let mut points = Vec::new();
        let outer = add_ring(&mut points, Vec2::ZERO, 50.0, 4000, false);
        let hole = add_ring(&mut points, Vec2::ZERO, 30.0, 4000, true);
        let triangles = triangulate(&points, &outer, std::slice::from_ref(&hole)).unwrap();
        let area = loop_area(&points, &outer) - loop_area(&points, &hole);
        assert_tiles(&points, &triples(&triangles), 1.0, area, &rim_of(&[&outer, &hole]));
    }
}
