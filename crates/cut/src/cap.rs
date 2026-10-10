//! Fills a closed 3D loop with a patch whose triangles are about the same size
//! and shape as the mesh around it. Built for the trimmer's cut faces; nothing
//! in here knows what a trim is, only that it has a rim and a flat-ish 2D chart
//! to work in.
//!
//! Why it exists. A cut face used to be one ear-clipped fan of the rim and
//! nothing else. Measured in the Unity app on a trimmed 50k-triangle sphere:
//! 636 cap triangles, no interior vertices, the largest 94 times the area of an
//! average shell triangle, and every one a sliver under 5 degrees. Two things
//! go wrong with that, and both are visible:
//!
//! - Shading. Vertex normals are area-weighted across the triangles around a
//!   vertex, so how far a rim vertex's normal is pulled off the shell and
//!   toward the flat cap depends on how much cap area happens to hang off it.
//!   Across neighbouring rim vertices that varied by a factor of 300 million,
//!   which renders as a ruffled, streaky band along the cut.
//! - It cannot be sculpted. With no interior vertices there is nothing for a
//!   brush to move; the cut face is a dead flat sheet in a sculpting app.
//!
//! The patch is left welded to the shell: it reuses the rim's own vertices and
//! never splits a rim edge. Giving the cap its own copies would render a touch
//! crisper, but the two copies then drift apart under any brush that reads
//! neighbours or normals, and the model splits open along the cut. A welded
//! cap at mesh density instead leaves a one-edge-wide crease, which is how
//! every other sharp feature on a dense sculpt reads.

use glam::{Vec2, Vec3};

use crate::patch::{Patch, Space, ear_clip, split_area};
use crate::polygon;

/// Ceiling on the ear clipper's point-in-triangle tests for one loop; whatever
/// is left when it runs out is closed by a fan. Sized for the rims real
/// sculpts produce: a cut through a 1.4M-triangle model leaves a rim of four
/// to five thousand vertices, and at a smaller budget the clipper ran out
/// every time and handed the rest to the fan, which on a lasso drawn with a
/// shaky hand joined rim vertices a third of the model apart.
const EAR_CLIP_BUDGET: usize = 64_000_000;

/// A loop filled with triangles.
#[derive(Clone, Debug, Default)]
pub struct CapFill {
    /// Three indices per triangle, wound the way the loop was given. Indices
    /// below the loop's length are loop vertices; the rest are that length
    /// plus an index into `extra_positions`.
    pub triangles: Vec<u32>,
    /// The interior vertices the fill added.
    pub extra_positions: Vec<Vec3>,
    /// For each added vertex, the three earlier vertices (same numbering as
    /// `triangles`) it was placed in the middle of. A per-vertex value such as
    /// a mask is carried onto the new vertices by averaging these in order.
    pub extra_parents: Vec<[u32; 3]>,
}

/// Triangulates the closed loop `loop_positions` (already in the winding the
/// patch must have) and refines it until its triangles are roughly
/// `target_edge` across.
///
/// `loop_chart` is the loop in a 2D chart of the surface being filled, in the
/// same units as the 3D positions. Shape is judged in that chart and size in
/// 3D. It may run either way round.
///
/// `chart_to_position` turns a point of the chart into the point of the
/// surface being filled, and is what decides where a new interior vertex goes;
/// `None` falls back to averaging the corners around it, which only lands on
/// the surface where the surface is flat.
///
/// At most `extra_vertex_budget` interior vertices are added. Rim vertices and
/// rim edges are never split, so the patch always matches the shell it is sewn
/// onto.
pub fn fill_loop(
    loop_positions: &[Vec3],
    loop_chart: &[Vec2],
    chart_to_position: Option<&dyn Fn(Vec2) -> Vec3>,
    target_edge: f32,
    extra_vertex_budget: usize,
) -> CapFill {
    let rim = loop_positions.len().min(loop_chart.len());
    if rim < 3 {
        return CapFill::default();
    }
    let chart = loop_chart[..rim].to_vec();
    let ring: Vec<u32> = (0..rim as u32).collect();

    // The loop's own orientation in the chart. Every triangle inherits it, and
    // both the circle test and the convexity test are expressed relative to it,
    // so the patch works whichever way round the rim happens to run.
    let orientation = if polygon::signed_area(&chart) >= 0.0 { 1.0 } else { -1.0 };
    let mut tris = Vec::with_capacity(rim);
    let clean = ear_clip(&chart, &ring, orientation, EAR_CLIP_BUDGET, false, &mut tris);
    if tris.is_empty() {
        return CapFill::default();
    }
    // A rim that crosses itself in the chart (it can, by a rounding error,
    // where the cut ran through slivers) or that had to be closed with a fan
    // leaves triangles lying over one another, and the patch has to be told.
    let sound = clean && polygon::is_simple(&chart);

    // Rim vertices are 0..rim in loop order, so a rim edge is a consecutive
    // pair (the closing one included). A chord between two rim vertices that
    // are not neighbours is an ordinary interior edge and is free to flip.
    let rim_count = rim as u32;
    let is_rim_edge = |u: u32, v: u32| {
        let apart = u.abs_diff(v);
        u < rim_count && v < rim_count && (apart == 1 || apart == rim_count - 1)
    };
    let mut patch = Patch::new(chart, tris, orientation, sound, is_rim_edge);
    let mut space = Space { positions: loop_positions[..rim].to_vec(), lift: chart_to_position, chart_counts: true };

    // Tidy the ear clip up first so the first refinement sweep measures
    // triangles worth measuring.
    patch.flip_to_delaunay();
    if target_edge > 0.0 {
        patch.improve(split_area(target_edge), extra_vertex_budget, Some(&mut space));
    }

    CapFill {
        triangles: patch.tris.into_flattened(),
        extra_positions: space.positions.split_off(rim),
        extra_parents: patch.parents,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch::tests::{assert_tiles, sliver_share};

    /// A flat loop in the plane z = 2, wobbling in and out, with the chart
    /// simply its x and y.
    fn flat_loop(n: usize, clockwise: bool) -> (Vec<Vec3>, Vec<Vec2>) {
        let mut chart: Vec<Vec2> = (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                Vec2::new(a.cos(), a.sin()) * (3.0 + 0.8 * (a * 4.0).sin())
            })
            .collect();
        if clockwise {
            chart.reverse();
        }
        (chart.iter().map(|p| p.extend(2.0)).collect(), chart)
    }

    fn triples(fill: &CapFill) -> Vec<[u32; 3]> {
        fill.triangles.chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect()
    }

    #[test]
    fn a_bare_fill_uses_only_the_rim() {
        let (positions, chart) = flat_loop(60, false);
        let fill = fill_loop(&positions, &chart, None, 0.0, 1000);
        assert!(fill.extra_positions.is_empty());
        assert_eq!(fill.triangles.len(), (60 - 2) * 3);
        let rim: Vec<(u32, u32)> = (0..60).map(|i| (i, (i + 1) % 60)).collect();
        assert_tiles(&chart, &triples(&fill), 1.0, polygon::signed_area(&chart), &rim);
    }

    #[test]
    fn the_fill_matches_the_target_density_and_keeps_the_rim() {
        for clockwise in [false, true] {
            let (positions, chart) = flat_loop(120, clockwise);
            let orientation = if clockwise { -1.0 } else { 1.0 };
            let area = polygon::signed_area(&chart).abs();
            let target = 0.2;
            let fill = fill_loop(&positions, &chart, None, target, 100_000);

            let mut all = chart.clone();
            all.extend(fill.extra_positions.iter().map(|p| p.truncate()));
            let rim: Vec<(u32, u32)> = (0..120).map(|i| (i, (i + 1) % 120)).collect();
            let tris = triples(&fill);
            assert_tiles(&all, &tris, orientation, area, &rim);
            assert!(fill.extra_positions.iter().all(|p| (p.z - 2.0).abs() < 1e-5), "new vertices stay on the face");
            assert_eq!(fill.extra_parents.len(), fill.extra_positions.len());

            // About as many triangles as the area holds at this edge length,
            // none far over size, and few of them slivers.
            let ideal = area / (0.433 * (target * target) as f64);
            let count = tris.len() as f64;
            assert!(count > ideal * 0.6 && count < ideal * 2.5, "{count} triangles for an ideal {ideal}");
            assert!(sliver_share(&all, &tris, 15.0) < 0.1, "slivers: {}", sliver_share(&all, &tris, 15.0));
        }
    }

    #[test]
    fn new_vertices_follow_the_surface_they_are_given() {
        // The chart is the unrolled wall of a cylinder; lifting wraps it back.
        let radius = 2.0f32;
        let lift = move |p: Vec2| Vec3::new((p.x / radius).cos() * radius, p.y, (p.x / radius).sin() * radius);
        // A coarse rim, as a cut through big triangles leaves: a dozen points
        // reaching a third of the way round the cylinder.
        let chart: Vec<Vec2> = (0..12)
            .map(|i| {
                let a = i as f32 / 12.0 * std::f32::consts::TAU;
                Vec2::new(2.2 + a.cos() * 2.0, a.sin() * 1.2)
            })
            .collect();
        let positions: Vec<Vec3> = chart.iter().map(|&p| lift(p)).collect();

        let on_surface = fill_loop(&positions, &chart, Some(&lift), 0.15, 100_000);
        assert!(on_surface.extra_positions.len() > 100);
        for p in &on_surface.extra_positions {
            assert!((Vec2::new(p.x, p.z).length() - radius).abs() < 1e-4, "{p} is off the cylinder");
        }
        // Averaged instead, the interior sits on the chords between rim points,
        // well inside the surface.
        let averaged = fill_loop(&positions, &chart, None, 0.15, 100_000);
        let deepest =
            averaged.extra_positions.iter().map(|p| radius - Vec2::new(p.x, p.z).length()).fold(0.0, f32::max);
        assert!(deepest > 0.03, "averaging cuts the corner by {deepest}");
    }

    #[test]
    fn a_rim_that_crosses_itself_still_closes_without_doubling_an_edge() {
        // Two lobes joined by a crossing, each lobe a ring of many points: no
        // flat patch can fill it properly, but whatever comes out must still
        // be a sheet. Every rim edge is walked exactly once, the way the loop
        // was given, and every other edge exactly once each way.
        let n = 80;
        let chart: Vec<Vec2> = (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                Vec2::new(a.sin() * 4.0, (2.0 * a).sin() * 1.5)
            })
            .collect();
        assert!(!polygon::is_simple(&chart));
        let positions: Vec<Vec3> = chart.iter().map(|p| p.extend(0.0)).collect();
        let fill = fill_loop(&positions, &chart, None, 0.25, 50_000);
        assert!(fill.extra_positions.len() > 100);

        let mut walked = std::collections::HashMap::new();
        for t in fill.triangles.chunks_exact(3) {
            for k in 0..3 {
                *walked.entry((t[k], t[(k + 1) % 3])).or_insert(0) += 1;
            }
        }
        assert!(walked.values().all(|&times| times == 1), "an edge is walked twice the same way");
        let rim = n as u32;
        for &(a, b) in walked.keys() {
            let on_rim = a < rim && b < rim && (a + 1) % rim == b;
            let against_rim = a < rim && b < rim && (b + 1) % rim == a;
            assert!(!against_rim, "rim edge {b}-{a} is walked backwards");
            assert_eq!(walked.contains_key(&(b, a)), !on_rim, "edge {a}-{b}");
        }
        assert_eq!(walked.keys().filter(|&&(a, b)| a < rim && b < rim && (a + 1) % rim == b).count(), n);
    }

    #[test]
    fn the_budget_caps_the_vertices_added() {
        let (positions, chart) = flat_loop(60, false);
        let fill = fill_loop(&positions, &chart, None, 0.05, 37);
        assert_eq!(fill.extra_positions.len(), 37);
        assert_eq!(fill.triangles.len() / 3, 58 + 2 * 37);
        assert!(fill_loop(&positions[..2], &chart[..2], None, 0.1, 10).triangles.is_empty());
    }
}
