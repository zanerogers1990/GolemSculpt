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

fn triangulate_with(points: &[Vec2], outer: &[u32], holes: &[Vec<u32>], strict: bool) -> Result<Vec<u32>, String> {
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
        if !bridge(points, &mut ring, &pending[i], &pending[i + 1..]) {
            return Err(fail());
        }
    }

    let mut triangles = Vec::with_capacity(ring.len());
    if !ear_clip(points, &ring, 1.0, EAR_TEST_BUDGET, strict, &mut triangles) && strict {
        return Err(fail());
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
/// vertex first offers only its own nearest ring vertex; the full list is
/// built only if none of those will do.
fn bridge(points: &[Vec2], ring: &mut Vec<u32>, hole: &[u32], pending: &[Vec<u32>]) -> bool {
    let (n, m) = (ring.len(), hole.len());
    let at = |i: u32| points[i as usize];
    let try_pair = |ring: &[u32], i: usize, j: usize| -> bool {
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

    // Each hole vertex with the ring position nearest it, nearest pair first.
    let mut nearest: Vec<(f32, u32, u32)> = (0..m)
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
        .collect();
    nearest.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut found = None;
    'search: for &(_, i, j) in &nearest {
        // An earlier bridge leaves its ring end in the ring twice, as one
        // point with two different pairs of neighbours; either may serve.
        let same_point = ring[i as usize];
        for copy in (0..n).filter(|&k| ring[k] == same_point) {
            if try_pair(ring, copy, j as usize) {
                found = Some((copy, j as usize));
                break 'search;
            }
        }
    }
    if found.is_none() {
        let mut all: Vec<(f32, u32, u32)> = (0..n * m)
            .into_par_iter()
            .map(|k| (at(ring[k / m]).distance_squared(at(hole[k % m])), (k / m) as u32, (k % m) as u32))
            .collect();
        all.par_sort_by(|a, b| a.0.total_cmp(&b.0));
        found = all.iter().map(|&(_, i, j)| (i as usize, j as usize)).find(|&(i, j)| try_pair(ring, i, j));
    }
    let Some((i, j)) = found else {
        return false;
    };

    // The ring up to i, the hole from j all the way round and back to j, then
    // i again and the rest of the ring.
    let mut spliced = Vec::with_capacity(n + m + 2);
    spliced.extend_from_slice(&ring[..=i]);
    spliced.extend((0..=m).map(|k| hole[(j + k) % m]));
    spliced.extend_from_slice(&ring[i..]);
    *ring = spliced;
    true
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
