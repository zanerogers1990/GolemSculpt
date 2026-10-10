//! Which vertices and triangles of a mesh a screen region covers, and what a
//! hide or mask gesture should therefore change.
//!
//! A region covers everything under it, front and back, straight through the
//! model, not just the surface facing the camera: that is what "hide the arms
//! so I can work on the abdomen" needs, and it is why nothing here casts a ray.
//! These are the pure halves of the box and lasso hide and mask tools; the
//! gesture, the hidden-face state and the undo step belong to the caller.
//!
//! Everything visits every vertex or triangle once, in parallel.

use std::sync::atomic::{AtomicBool, Ordering};

use glam::Vec3;
use rayon::prelude::*;
use sculpt_geom::SymmetryOp;

use crate::region::{ScreenProjection, ScreenRegion};

/// Below this many items a parallel split costs more than it saves.
const MIN_PARALLEL: usize = 4096;
/// Triangles handed to one worker at a time when gathering a list.
const TRIANGLE_CHUNK: usize = 1 << 15;

/// Flags every vertex the region covers, in the mesh's own vertex order, and
/// returns how many there are. `inside` is resized to fit, so a caller can keep
/// one buffer across gestures instead of allocating per drag.
///
/// `projections` has one entry per place the mesh is drawn: the object itself,
/// then each live mirror copy. A region drawn over a copy covers the vertices
/// the copy shows. `symmetry` is the object's symmetry (`SymmetryGroup::ops`);
/// a vertex also counts as covered when its image under any of them is, so
/// dragging over one arm takes both. An empty list means no symmetry.
///
/// `outside` flips the whole answer at the end rather than at each test, so
/// "outside" means "not covered under any mirror" instead of "outside under
/// each mirror separately", which would select nothing once symmetry was on.
pub fn mark_vertices_in_region(
    positions: &[Vec3],
    projections: &[ScreenProjection],
    symmetry: &[SymmetryOp],
    region: &ScreenRegion,
    outside: bool,
    inside: &mut Vec<bool>,
) -> usize {
    let identity = [SymmetryOp::IDENTITY];
    let symmetry = if symmetry.is_empty() { &identity[..] } else { symmetry };
    inside.clear();
    inside.resize(positions.len(), false);
    inside
        .par_iter_mut()
        .zip(positions.par_iter())
        .with_min_len(MIN_PARALLEL)
        .map(|(flag, &p)| {
            // A vertex is under copy k of the region when its inverse image is
            // under the region itself.
            let covered = projections.iter().any(|projection| {
                symmetry.iter().any(|op| {
                    projection.project(op.apply_inverse_point(p)).is_some_and(|screen| region.contains(screen))
                })
            });
            *flag = covered != outside;
            *flag as usize
        })
        .sum()
}

/// As `mark_vertices_in_region`, into a fresh buffer.
pub fn vertices_in_region(
    positions: &[Vec3],
    projections: &[ScreenProjection],
    symmetry: &[SymmetryOp],
    region: &ScreenRegion,
    outside: bool,
) -> Vec<bool> {
    let mut inside = Vec::new();
    mark_vertices_in_region(positions, projections, symmetry, region, outside, &mut inside);
    inside
}

/// The triangles whose three corners are all flagged, in ascending order.
///
/// All three rather than any: a triangle straddling the edge of the region
/// keeps its vertices sculptable, so the border of a hidden area behaves like
/// ordinary surface rather than a frozen wall.
pub fn triangles_inside(indices: &[u32], inside: &[bool]) -> Vec<u32> {
    gather_counted(indices, |_, t| (covered(t, inside), false)).0
}

#[inline]
fn covered(t: &[u32], inside: &[bool]) -> bool {
    inside[t[0] as usize] && inside[t[1] as usize] && inside[t[2] as usize]
}

/// One pass over the triangles answering two questions of each: is it listed,
/// and is it counted. Returns the list, in ascending order, and the count.
fn gather_counted<F>(indices: &[u32], judge: F) -> (Vec<u32>, usize)
where
    F: Fn(usize, &[u32]) -> (bool, bool) + Sync,
{
    let parts: Vec<(Vec<u32>, usize)> = indices
        .par_chunks(TRIANGLE_CHUNK * 3)
        .enumerate()
        .map(|(chunk, corners)| {
            let (mut listed, mut counted) = (Vec::new(), 0);
            for (k, tri) in corners.chunks_exact(3).enumerate() {
                let t = chunk * TRIANGLE_CHUNK + k;
                let (list, count) = judge(t, tri);
                if list {
                    listed.push(t as u32);
                }
                counted += count as usize;
            }
            (listed, counted)
        })
        .collect();
    let mut listed = Vec::with_capacity(parts.iter().map(|p| p.0.len()).sum());
    let mut counted = 0;
    for (part, count) in parts {
        listed.extend_from_slice(&part);
        counted += count;
    }
    (listed, counted)
}

/// The triangles a hide (or, with `hide` false, a show) gesture should flip:
/// those wholly covered whose hidden flag is not already what is wanted.
///
/// `hidden` is the mesh's per-triangle hidden flags, or `None` when nothing is
/// hidden. Fails, with what to tell the user, when the gesture would change
/// nothing or would leave nothing on screen. The second is refused rather than
/// allowed: an object that has vanished completely gives no clue that it is
/// still there and only hidden, and a tool that can make the model disappear
/// by overshooting a drag is not one people trust.
pub fn plan_hide(indices: &[u32], inside: &[bool], hidden: Option<&[bool]>, hide: bool) -> Result<Vec<u32>, String> {
    let triangle_count = indices.len() / 3;
    if triangle_count == 0 {
        return Err("No geometry.".to_string());
    }
    let (changed, hidden_after) = gather_counted(indices, |t, tri| {
        let covered = covered(tri, inside);
        let was_hidden = hidden.is_some_and(|h| h[t]);
        (covered && was_hidden != hide, if covered { hide } else { was_hidden })
    });
    if changed.is_empty() {
        return Err(if hide { "Nothing new to hide there." } else { "Nothing hidden there." }.to_string());
    }
    if hidden_after >= triangle_count {
        return Err("That would hide the whole object - skipped.".to_string());
    }
    Ok(changed)
}

/// The vertices a mask gesture should write: those the region covers that are
/// not hidden. Hidden vertices are skipped because hiding is how a limb is got
/// out of the way, and a mask box dragged over the torso silently masking the
/// arm behind it would defeat that entirely.
///
/// `hidden_vertices` is `hidden_vertices`' answer, or `None` when nothing is
/// hidden.
pub fn mask_targets(inside: &[bool], hidden_vertices: Option<&[bool]>) -> Result<Vec<u32>, String> {
    let targets: Vec<u32> = inside
        .par_iter()
        .enumerate()
        .with_min_len(MIN_PARALLEL)
        .filter(|&(i, &covered)| covered && !hidden_vertices.is_some_and(|h| h[i]))
        .map(|(i, _)| i as u32)
        .collect();
    if targets.is_empty() {
        return Err("Region covered no vertices.".to_string());
    }
    Ok(targets)
}

/// Per-vertex hidden flags from per-triangle ones: a vertex is hidden only
/// when every triangle using it is. This is what brushes should read, so the
/// border vertices of a hidden area stay sculptable. A vertex no triangle uses
/// comes out hidden, which is honest: nothing draws it either way.
pub fn hidden_vertices(vertex_count: usize, indices: &[u32], hidden_triangles: &[bool]) -> Vec<bool> {
    // Every vertex starts hidden and is cleared by the first visible triangle
    // that uses it. Threads only ever store `false`, so the order they run in
    // cannot change the answer.
    let flags: Vec<AtomicBool> = (0..vertex_count).into_par_iter().map(|_| AtomicBool::new(true)).collect();
    indices
        .par_chunks_exact(3)
        .zip(hidden_triangles.par_iter())
        .with_min_len(MIN_PARALLEL)
        .filter(|&(_, &hidden)| !hidden)
        .for_each(|(tri, _)| {
            for &i in tri {
                flags[i as usize].store(false, Ordering::Relaxed);
            }
        });
    flags.into_iter().map(AtomicBool::into_inner).collect()
}

/// The least distance in front of `origin`, along the unit vector `forward`,
/// of the given vertices: how near the camera a selection comes. Vertices at
/// or behind `origin` are skipped, as a depth probe skips what is behind the
/// eye; infinity when none is in front.
pub fn nearest_view_depth(positions: &[Vec3], indices: &[u32], origin: Vec3, forward: Vec3) -> f32 {
    indices
        .par_iter()
        .with_min_len(MIN_PARALLEL)
        .map(|&i| (positions[i as usize] - origin).dot(forward))
        .filter(|&depth| depth > 0.0)
        .reduce(|| f32::INFINITY, f32::min)
}

/// The index buffer of the triangles that are not hidden, in their original
/// order. The vertex buffer keeps every vertex, hidden ones included: only the
/// indices shrink, which keeps every vertex index (undo records, mask,
/// symmetry maps) valid across a hide.
pub fn visible_indices(indices: &[u32], hidden_triangles: &[bool]) -> Vec<u32> {
    let visible = gather_counted(indices, |t, _| (!hidden_triangles[t], false)).0;
    let mut out = vec![0u32; visible.len() * 3];
    out.par_chunks_exact_mut(3).zip(visible.par_iter()).with_min_len(MIN_PARALLEL).for_each(|(corners, &t)| {
        corners.copy_from_slice(&indices[t as usize * 3..t as usize * 3 + 3]);
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{cube, icosphere, ortho_view, perspective_view, torus};
    use glam::Vec2;
    use std::time::Instant;

    const SCREEN: Vec2 = Vec2::new(1000.0, 800.0);

    /// One vertex at a time, straight from the definitions.
    fn brute_force(
        positions: &[Vec3],
        projections: &[ScreenProjection],
        ops: &[SymmetryOp],
        region: &ScreenRegion,
    ) -> Vec<bool> {
        positions
            .iter()
            .map(|&p| {
                projections.iter().any(|projection| {
                    ops.iter().any(|op| match projection.project(op.apply_inverse_point(p)) {
                        Some(screen) => region.contains(screen),
                        None => false,
                    })
                })
            })
            .collect()
    }

    fn lasso() -> ScreenRegion {
        // A crescent: concave, and off to one side of the screen.
        let outline: Vec<Vec2> = (0..40)
            .map(|i| {
                let a = i as f32 / 40.0 * std::f32::consts::TAU;
                let r = if (10..30).contains(&i) { 90.0 } else { 240.0 };
                Vec2::new(380.0 + a.cos() * r, 400.0 + a.sin() * r * 0.9)
            })
            .collect();
        ScreenRegion::lasso(&outline).unwrap()
    }

    #[test]
    fn regions_match_brute_force_under_both_cameras() {
        let sphere = icosphere(5);
        let views = [
            ortho_view(Vec3::ZERO, 25.0, 15.0, 1.4, Vec2::ZERO, SCREEN),
            perspective_view(Vec3::ZERO, -40.0, 10.0, 2.2, Vec2::new(30.0, 20.0), SCREEN),
        ];
        let regions = [
            ScreenRegion::rect(Vec2::new(200.0, 150.0), Vec2::new(520.0, 610.0)).unwrap(),
            lasso(),
            ScreenRegion::half_plane(Vec2::new(300.0, 100.0), Vec2::new(700.0, 650.0)).unwrap(),
        ];
        for view in &views {
            for region in &regions {
                let projections = [view.projection];
                let want = brute_force(&sphere.positions, &projections, &[SymmetryOp::IDENTITY], region);
                let got = vertices_in_region(&sphere.positions, &projections, &[], region, false);
                assert_eq!(got, want);
                let covered = want.iter().filter(|&&c| c).count();
                assert!(covered > 100 && covered < sphere.vertex_count() - 100, "a real split: {covered}");

                let mut reused = vec![true; 3];
                let count = mark_vertices_in_region(&sphere.positions, &projections, &[], region, true, &mut reused);
                assert_eq!(count, sphere.vertex_count() - covered);
                assert!(reused.iter().zip(&want).all(|(a, b)| a != b), "outside is the exact complement");
            }
        }
    }

    #[test]
    fn a_region_reaches_straight_through_the_model() {
        // A small box over the middle of a sphere takes the near and the far
        // surface alike.
        let sphere = icosphere(5);
        let view = ortho_view(Vec3::ZERO, 0.0, 0.0, 1.5, Vec2::ZERO, SCREEN);
        let region = ScreenRegion::rect(Vec2::new(440.0, 340.0), Vec2::new(560.0, 460.0)).unwrap();
        let inside = vertices_in_region(&sphere.positions, &[view.projection], &[], &region, false);
        let near = sphere.positions.iter().zip(&inside).filter(|(p, c)| **c && p.z > 0.0).count();
        let far = sphere.positions.iter().zip(&inside).filter(|(p, c)| **c && p.z < 0.0).count();
        assert!(near > 20 && near == far, "near {near}, far {far}");
    }

    #[test]
    fn symmetry_and_mirror_copies_widen_the_selection() {
        let sphere = icosphere(4);
        let view = ortho_view(Vec3::ZERO, 0.0, 0.0, 1.5, Vec2::ZERO, SCREEN);
        // Over the right-hand side of the screen only: +X.
        let region = ScreenRegion::rect(Vec2::new(650.0, 300.0), Vec2::new(760.0, 500.0)).unwrap();
        let projections = [view.projection];
        let plain = vertices_in_region(&sphere.positions, &projections, &[], &region, false);
        assert!(sphere.positions.iter().zip(&plain).all(|(p, c)| !*c || p.x > 0.0));

        let mirror = [SymmetryOp::IDENTITY, SymmetryOp::from_sign(Vec3::new(-1.0, 1.0, 1.0))];
        let both = vertices_in_region(&sphere.positions, &projections, &mirror, &region, false);
        assert_eq!(both, brute_force(&sphere.positions, &projections, &mirror, &region));
        for (p, covered) in sphere.positions.iter().zip(&both) {
            let twin = Vec3::new(-p.x, p.y, p.z);
            let twin_at = sphere.positions.iter().position(|q| q.distance(twin) < 1e-5).unwrap();
            assert_eq!(*covered, both[twin_at], "a vertex and its mirror twin go together");
        }
        assert_eq!(both.iter().filter(|c| **c).count(), 2 * plain.iter().filter(|c| **c).count());

        // A second place the mesh is drawn: the same mesh shifted left on
        // screen. The region now also covers what that copy shows under it.
        let copy = ScreenProjection {
            view_proj: view.projection.view_proj * glam::Mat4::from_translation(Vec3::new(-1.4, 0.0, 0.0)),
            ..view.projection
        };
        let two = vertices_in_region(&sphere.positions, &[view.projection, copy], &[], &region, false);
        assert_eq!(two, brute_force(&sphere.positions, &[view.projection, copy], &[SymmetryOp::IDENTITY], &region));
        assert!(two.iter().zip(&plain).all(|(t, p)| *t || !*p), "nothing is lost");
    }

    #[test]
    fn hiding_takes_whole_triangles_and_never_the_whole_object() {
        let mesh = cube(8);
        let view = ortho_view(Vec3::ZERO, 0.0, 0.0, 1.5, Vec2::ZERO, SCREEN);
        let left = ScreenRegion::rect(Vec2::new(0.0, 0.0), Vec2::new(500.0, 800.0)).unwrap();
        let inside = vertices_in_region(&mesh.positions, &[view.projection], &[], &left, false);

        let tris = triangles_inside(&mesh.indices, &inside);
        for t in 0..mesh.triangle_count() {
            let all = mesh.triangle(t).iter().all(|p| p.x <= 0.0);
            assert_eq!(tris.binary_search(&(t as u32)).is_ok(), all, "triangle {t}");
        }
        assert_eq!(plan_hide(&mesh.indices, &inside, None, true).unwrap(), tris);
        assert_eq!(plan_hide(&mesh.indices, &inside, None, false).unwrap_err(), "Nothing hidden there.");

        let mut hidden = vec![false; mesh.triangle_count()];
        for &t in &tris {
            hidden[t as usize] = true;
        }
        assert_eq!(plan_hide(&mesh.indices, &inside, Some(&hidden), true).unwrap_err(), "Nothing new to hide there.");
        assert_eq!(plan_hide(&mesh.indices, &inside, Some(&hidden), false).unwrap(), tris);

        // The rest of the cube, on top of what is already hidden.
        let rest: Vec<bool> = mesh.positions.iter().map(|p| p.x >= 0.0).collect();
        assert_eq!(
            plan_hide(&mesh.indices, &rest, Some(&hidden), true).unwrap_err(),
            "That would hide the whole object - skipped."
        );
        let all = vec![true; mesh.vertex_count()];
        assert!(plan_hide(&mesh.indices, &all, None, true).is_err());
    }

    #[test]
    fn hidden_vertices_are_those_with_no_visible_triangle() {
        let mesh = cube(8);
        let hidden: Vec<bool> =
            (0..mesh.triangle_count()).map(|t| mesh.triangle(t).iter().all(|p| p.x <= 0.0)).collect();
        let vertices = hidden_vertices(mesh.vertex_count(), &mesh.indices, &hidden);
        for (p, h) in mesh.positions.iter().zip(&vertices) {
            // The seam at x = 0 is shared with visible triangles, so it stays.
            assert_eq!(*h, p.x < 0.0, "{p}");
        }
        let visible = visible_indices(&mesh.indices, &hidden);
        assert_eq!(visible.len() / 3, hidden.iter().filter(|h| !**h).count());
        assert!(visible.chunks_exact(3).all(|t| t.iter().any(|&i| mesh.positions[i as usize].x > 0.0)));

        let everything = vec![true; mesh.vertex_count()];
        let targets = mask_targets(&everything, Some(&vertices)).unwrap();
        assert_eq!(targets.len(), vertices.iter().filter(|h| !**h).count());
        assert!(targets.iter().all(|&i| !vertices[i as usize]));
        assert_eq!(mask_targets(&vertices, Some(&vertices)).unwrap_err(), "Region covered no vertices.");
        assert_eq!(mask_targets(&everything, None).unwrap().len(), mesh.vertex_count());
    }

    #[test]
    fn nearest_view_depth_skips_what_is_behind() {
        let mesh = cube(4);
        let all: Vec<u32> = (0..mesh.vertex_count() as u32).collect();
        // From outside, looking in: the near face is 2 away.
        assert!((nearest_view_depth(&mesh.positions, &all, Vec3::new(0.0, 0.0, 3.0), Vec3::NEG_Z) - 2.0).abs() < 1e-6);
        // From the middle, only what is ahead counts.
        assert!((nearest_view_depth(&mesh.positions, &all, Vec3::ZERO, Vec3::X) - 0.5).abs() < 1e-6);
        let far_side: Vec<u32> = all.iter().copied().filter(|&i| mesh.positions[i as usize].z < -0.9).collect();
        assert!(
            (nearest_view_depth(&mesh.positions, &far_side, Vec3::new(0.0, 0.0, 3.0), Vec3::NEG_Z) - 4.0).abs() < 1e-6
        );
        assert_eq!(nearest_view_depth(&mesh.positions, &all, Vec3::new(0.0, 0.0, 3.0), Vec3::Z), f32::INFINITY);
        assert_eq!(nearest_view_depth(&mesh.positions, &[], Vec3::ZERO, Vec3::X), f32::INFINITY);
    }

    /// Timing: `cargo test -p sculpt-cut --release -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn timing_region_selection_over_four_million_vertices() {
        let mesh = torus(2.0, 0.7, 2000, 2000);
        let view = perspective_view(Vec3::ZERO, 30.0, 35.0, 7.0, Vec2::ZERO, Vec2::new(1920.0, 1080.0));
        let projections = [view.projection];
        let mirror = [SymmetryOp::IDENTITY, SymmetryOp::from_sign(Vec3::new(-1.0, 1.0, 1.0))];
        let outline: Vec<Vec2> = (0..400)
            .map(|i| {
                let a = i as f32 / 400.0 * std::f32::consts::TAU;
                Vec2::new(
                    900.0 + a.cos() * (330.0 + 90.0 * (a * 7.0).sin()),
                    540.0 + a.sin() * (300.0 + 90.0 * (a * 5.0).cos()),
                )
            })
            .collect();
        let shapes = [
            ("box", ScreenRegion::rect(Vec2::new(500.0, 200.0), Vec2::new(1300.0, 800.0)).unwrap()),
            ("lasso", ScreenRegion::lasso(&outline).unwrap()),
        ];
        println!(
            "{} vertices, {} triangles, {} threads",
            mesh.vertex_count(),
            mesh.triangle_count(),
            rayon::current_num_threads()
        );
        let mut inside = Vec::new();
        for (name, region) in &shapes {
            for (label, ops) in [("no symmetry", &mirror[..1]), ("mirror x", &mirror[..])] {
                mark_vertices_in_region(&mesh.positions, &projections, ops, region, false, &mut inside);
                let start = Instant::now();
                let count = mark_vertices_in_region(&mesh.positions, &projections, ops, region, false, &mut inside);
                println!("{name}, {label}: {count} vertices in {:.2} ms", start.elapsed().as_secs_f64() * 1e3);
            }
            let start = Instant::now();
            let tris = triangles_inside(&mesh.indices, &inside);
            let listed = start.elapsed();
            let start = Instant::now();
            let plan = plan_hide(&mesh.indices, &inside, None, true).unwrap();
            println!(
                "{name}: {} triangles listed in {:.2} ms, hide planned in {:.2} ms",
                tris.len(),
                listed.as_secs_f64() * 1e3,
                start.elapsed().as_secs_f64() * 1e3
            );
            let mut hidden = vec![false; mesh.triangle_count()];
            plan.iter().for_each(|&t| hidden[t as usize] = true);
            let start = Instant::now();
            let vertices = hidden_vertices(mesh.vertex_count(), &mesh.indices, &hidden);
            let flagged = start.elapsed();
            let start = Instant::now();
            let visible = visible_indices(&mesh.indices, &hidden);
            println!(
                "{name}: {} hidden vertices in {:.2} ms, {} visible triangles in {:.2} ms",
                vertices.iter().filter(|h| **h).count(),
                flagged.as_secs_f64() * 1e3,
                visible.len() / 3,
                start.elapsed().as_secs_f64() * 1e3
            );
        }
    }
}
