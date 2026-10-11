//! Decimation checked against what it promises: the count, the surface (as
//! closed, as connected and as many-handled as it was, nothing turned over),
//! the shape, borders, locked vertices, what is carried, and its speed.

use std::collections::HashSet;

use glam::{Quat, Vec3};

use super::*;
use crate::testing::{cube, grid, tetrahedron, uv_sphere};
use crate::{EdgeCensus, MeshAdjacency, MeshEdges, TriMesh};

fn run_with(mesh: &TriMesh, values: Option<&[f32]>, locked: &[bool], target: usize) -> Decimated {
    decimate(&mesh.positions, &mesh.indices, values, locked, target, &mut |_| true).expect("not cancelled")
}

fn run(mesh: &TriMesh, target: usize) -> TriMesh {
    let done = run_with(mesh, None, &[], target);
    TriMesh::new(done.positions, done.indices)
}

/// Vertices less edges plus faces: 2 for a ball, 0 for a ring, and the same
/// before and after anything that keeps a surface's connectivity.
fn euler(mesh: &TriMesh) -> i64 {
    let triangles: Vec<[u32; 3]> = mesh.indices.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
    let used: HashSet<u32> = mesh.indices.iter().copied().collect();
    used.len() as i64 - edges_of(&triangles).len() as i64 + triangles.len() as i64
}

/// The smallest triangle's area over the mean one's, and whether any two
/// triangles are the same three vertices.
fn worst_triangle(mesh: &TriMesh) -> (f64, bool) {
    let areas: Vec<f64> = (0..mesh.triangle_count())
        .map(|t| {
            let [a, b, c] = mesh.triangle(t).map(|p| p.as_dvec3());
            (b - a).cross(c - a).length() * 0.5
        })
        .collect();
    let mean = areas.iter().sum::<f64>() / areas.len().max(1) as f64;
    let least = areas.iter().copied().fold(f64::MAX, f64::min);
    let mut seen = HashSet::new();
    let doubled = mesh.indices.chunks_exact(3).any(|c| {
        let mut key = [c[0], c[1], c[2]];
        key.sort_unstable();
        !seen.insert(key)
    });
    (least / mean, doubled)
}

/// A closed surface, every triangle the same way out as its neighbours, none
/// of them a sliver of nothing or doubled, and every vertex in use.
fn assert_closed_and_sound(mesh: &TriMesh, what: &str) {
    assert!(EdgeCensus::of(mesh).is_closed(), "{what}: {:?}", EdgeCensus::of(mesh));
    let adjacency = MeshAdjacency::of(mesh);
    let edges = MeshEdges::of(mesh, &adjacency);
    assert!(edges.is_consistently_wound(&mesh.indices), "{what}: a triangle is turned over");
    let (least, doubled) = worst_triangle(mesh);
    assert!(least > 1e-6 && !doubled, "{what}: smallest triangle {least} of the mean, doubled {doubled}");
    let used: HashSet<u32> = mesh.indices.iter().copied().collect();
    assert_eq!(used.len(), mesh.vertex_count(), "{what}: a vertex nothing uses");
}

/// A ball with lumps on it, off its own centre and turned: a model, not a
/// test shape. `2 * segs * (rings - 1)` triangles.
fn lumpy(rings: usize, segs: usize) -> TriMesh {
    let mut ball = uv_sphere(rings, segs);
    let turn = Quat::from_euler(glam::EulerRot::XYZ, 0.3, 0.5, -0.2);
    for p in &mut ball.positions {
        let d = *p;
        let lumps = (d.y * 7.0 + 1.0).sin() * (d.z * 6.0 + 0.5).sin() + 0.5 * (d.x * 9.0 + d.y * 5.0).sin();
        *p = turn * (d * (1.0 + 0.12 * lumps)) + Vec3::new(3.0, -1.0, 0.5);
    }
    ball
}

/// A ring: `2 * around * across` triangles, one handle.
fn torus(around: usize, across: usize) -> TriMesh {
    let mut positions = Vec::new();
    for i in 0..around {
        let a = std::f32::consts::TAU * i as f32 / around as f32;
        for j in 0..across {
            let b = std::f32::consts::TAU * j as f32 / across as f32;
            let r = 1.0 + 0.35 * b.cos();
            positions.push(Vec3::new(r * a.cos(), 0.35 * b.sin(), r * a.sin()));
        }
    }
    let at = |i: usize, j: usize| ((i % around) * across + j % across) as u32;
    let mut indices = Vec::new();
    for i in 0..around {
        for j in 0..across {
            let (a, b, c, d) = (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
            indices.extend_from_slice(&[a, b, c, a, c, d]);
        }
    }
    let mesh = TriMesh::new(positions, indices);
    if mesh.signed_volume() < 0.0 {
        let mut flipped = mesh;
        flipped.flip();
        return flipped;
    }
    mesh
}

/// How far the surface of `mesh` is from the unit sphere, at its vertices, at worst.
fn off_the_sphere(mesh: &TriMesh) -> f32 {
    mesh.positions.iter().map(|p| (p.length() - 1.0).abs()).fold(0.0, f32::max)
}

#[test]
fn a_closed_model_comes_down_to_the_count_asked_for_and_stays_closed() {
    let model = lumpy(120, 240);
    let before = model.triangle_count();
    assert_eq!(before, 57_120);
    assert_closed_and_sound(&model, "the model itself");
    for percent in [75, 50, 25, 10, 2] {
        let target = before * percent / 100;
        let done = run_with(&model, None, &[], target);
        assert!(done.reached(), "{percent}%: {} left of {target}", done.triangle_count());
        // A collapse inside a closed surface takes two triangles: the count is the target or one under.
        assert!(done.triangle_count() + 1 >= target, "{percent}%: {} is well under {target}", done.triangle_count());
        assert_eq!((done.triangles_before, done.target, done.locked), (before, target, 0));
        let mesh = TriMesh::new(done.positions.clone(), done.indices.clone());
        assert_closed_and_sound(&mesh, &format!("{percent}%"));
        assert_eq!(euler(&mesh), 2, "{percent}%: still one ball");
        // The shape: its volume and the box round it.
        let (was, now) = (model.signed_volume(), mesh.signed_volume());
        let allowed = if percent >= 10 { 0.004 } else { 0.03 };
        assert!((now / was - 1.0).abs() < allowed, "{percent}%: volume {now} against {was}");
        let (a, b) = (model.bounds(), mesh.bounds());
        let slack = if percent >= 10 { 0.01 } else { 0.05 };
        assert!((a.min - b.min).abs().max_element() < slack && (a.max - b.max).abs().max_element() < slack, "{percent}%");
    }
}

#[test]
fn the_error_goes_where_the_surface_is_flat_and_a_sphere_stays_round() {
    let ball = uv_sphere(100, 200);
    let done = run(&ball, ball.triangle_count() / 10);
    assert_closed_and_sound(&done, "a tenth of a sphere");
    // 3,960 triangles on a unit sphere sit within a part in a thousand of it.
    assert!(off_the_sphere(&done) < 2e-3, "{}", off_the_sphere(&done));
    assert!((done.signed_volume() / ball.signed_volume() - 1.0).abs() < 3e-3);

    // A box with every face cut into a hundred triangles is twelve triangles' worth of shape:
    // it comes all the way down, and its corners are where they were to the last digit or so.
    let mut fine = TriMesh::new(Vec::new(), Vec::new());
    for (axis, side) in [(0, 1.0), (0, -1.0), (1, 1.0), (1, -1.0), (2, 1.0), (2, -1.0)] {
        let mut face = grid(7, 7);
        for p in &mut face.positions {
            let (u, v) = (p.x / 3.5 - 1.0, p.y / 3.5 - 1.0);
            *p = match axis {
                0 => Vec3::new(side, u, v * side),
                1 => Vec3::new(v * side, side, u),
                _ => Vec3::new(u, v * side, side),
            };
        }
        fine.append(&face);
    }
    // Welded along the box's edges: one vertex for each place.
    let mut place = std::collections::HashMap::new();
    let mut welded = TriMesh::new(Vec::new(), Vec::new());
    for &i in &fine.indices {
        let p = fine.positions[i as usize];
        let key = p.to_array().map(|c| (c * 1024.0).round() as i32);
        let index = *place.entry(key).or_insert_with(|| {
            welded.positions.push(p);
            welded.positions.len() as u32 - 1
        });
        welded.indices.push(index);
    }
    assert_closed_and_sound(&welded, "the box of small triangles");
    let done = run(&welded, 12);
    assert_eq!(done.triangle_count(), 12);
    assert_closed_and_sound(&done, "the box");
    assert!((done.signed_volume() - 8.0).abs() < 1e-4, "{}", done.signed_volume());
    for p in &done.positions {
        assert!((p.abs() - Vec3::ONE).abs().max_element() < 1e-4, "a corner at {p}");
    }
}

#[test]
fn a_handle_stays_a_handle_and_two_shells_stay_two() {
    let ring = torus(96, 48);
    assert_eq!(euler(&ring), 0);
    for target in [4000, 400, 40, 4] {
        let done = run(&ring, target);
        assert_closed_and_sound(&done, &format!("the ring at {target}"));
        assert_eq!(euler(&done), 0, "the hole through the ring is still there at {target}");
    }
    // Asked for next to nothing, it stops where another collapse would close the hole.
    let least = run_with(&ring, None, &[], 4);
    assert!(!least.reached() && least.triangle_count() >= 14 && least.triangle_count() < 60, "{}", least.triangle_count());

    let mut two = uv_sphere(40, 80);
    let mut second = uv_sphere(40, 80);
    second.positions.iter_mut().for_each(|p| *p = *p * 0.4 + Vec3::X * 3.0);
    two.append(&second);
    let done = run(&two, 600);
    assert_closed_and_sound(&done, "two balls");
    assert_eq!(euler(&done), 4, "two balls still");
    // The small one is not spent first: it keeps a share that still looks like a ball.
    let small = done.positions.iter().filter(|p| p.x > 2.0).count();
    assert!(small >= 20, "{small} vertices left on the small ball");
}

#[test]
fn the_smallest_meshes_are_left_as_they_must_be() {
    // A tetrahedron has nothing to give: any collapse leaves two triangles back to back.
    for target in [0, 1, 3, 4, 100] {
        let done = run_with(&tetrahedron(), None, &[], target);
        assert_eq!((done.triangle_count(), done.target), (4, 4), "asked for {target}");
        assert_closed_and_sound(&TriMesh::new(done.positions, done.indices), "the tetrahedron");
    }
    // A cube's twelve come down as far as a closed surface can go.
    let done = run(&cube(), 4);
    assert_closed_and_sound(&done, "the cube");
    assert!(done.triangle_count() >= 4 && done.triangle_count() <= 8, "{}", done.triangle_count());
    // Nothing at all, and triangles that are not triangles.
    let none = decimate(&[], &[], None, &[], 10, &mut |_| true).unwrap();
    assert!(none.positions.is_empty() && none.indices.is_empty() && none.triangles_before == 0);
    let points = [Vec3::ZERO, Vec3::X, Vec3::Y];
    let odd = decimate(&points, &[0, 1, 2, 0, 0, 1, 0, 1, 9], Some(&[0.5, 0.25, 1.0]), &[], 1, &mut |_| true).unwrap();
    assert_eq!((odd.indices.len(), odd.triangles_before, &odd.triangle_source[..]), (3, 1, &[0][..]));
    // Asked for more than there is: the mesh as it was, its own points to the bit.
    let ball = uv_sphere(8, 12);
    let same = run_with(&ball, None, &[], 10_000);
    assert_eq!(same.triangle_count(), ball.triangle_count());
    let (now, was): (HashSet<_>, HashSet<_>) = (
        same.positions.iter().map(|p| p.to_array().map(f32::to_bits)).collect(),
        ball.positions.iter().map(|p| p.to_array().map(f32::to_bits)).collect(),
    );
    assert_eq!(now, was);
}

#[test]
fn an_open_sheet_keeps_its_border_and_stays_flat() {
    let sheet = grid(60, 40);
    let done = run(&sheet, 300);
    let census = EdgeCensus::of(&done);
    assert_eq!((census.crowded, census.holes), (0, Some(1)), "one border still: {census:?}");
    assert_eq!(euler(&done), 1, "a disc");
    assert!(done.triangle_count() <= 300);
    // Flat to the bit, and the same rectangle: the border was only moved along itself.
    assert!(done.positions.iter().all(|p| p.z == 0.0));
    let bounds = done.bounds();
    assert_eq!((bounds.min, bounds.max), (Vec3::ZERO, Vec3::new(60.0, 40.0, 0.0)));
    let area: f32 = (0..done.triangle_count())
        .map(|t| {
            let [a, b, c] = done.triangle(t);
            (b - a).cross(c - a).z * 0.5
        })
        .sum();
    assert!((area - 2400.0).abs() < 1e-2, "{area}: nothing overlaps, and every triangle faces up");
    for corner in [Vec3::ZERO, Vec3::new(60.0, 0.0, 0.0), Vec3::new(0.0, 40.0, 0.0), Vec3::new(60.0, 40.0, 0.0)] {
        assert!(done.positions.contains(&corner), "the corner at {corner} is gone");
    }

    // A sphere with a hole cut in it keeps the one hole, with its rim where it was.
    let ball = uv_sphere(60, 120);
    let cut: Vec<u32> =
        ball.indices.chunks_exact(3).filter(|c| c.iter().any(|&v| ball.positions[v as usize].y < 0.8)).flatten().copied().collect();
    let holed = TriMesh::new(ball.positions.clone(), cut);
    let done = run(&holed, 800);
    let census = EdgeCensus::of(&done);
    assert_eq!((census.crowded, census.holes), (0, Some(1)), "{census:?}");
    let top = done.positions.iter().map(|p| p.y).fold(f32::MIN, f32::max);
    assert!((top - 0.8).abs() < 0.03, "the rim is at {top}");
}

#[test]
fn locked_vertices_stay_and_what_is_carried_is_carried() {
    let ball = uv_sphere(80, 160);
    // The top of the ball is locked; the value carried is 1 there, falling to 0 below a soft edge.
    let value = |p: &Vec3| ((p.y - 0.3) * 5.0).clamp(0.0, 1.0);
    let values: Vec<f32> = ball.positions.iter().map(value).collect();
    let locked: Vec<bool> = values.iter().map(|&v| v >= 0.999).collect();
    let locked_count = locked.iter().filter(|&&l| l).count();
    assert!(locked_count > 2_000);
    let whole = |c: &[u32]| c.iter().all(|&v| locked[v as usize]);
    let locked_triangles = ball.indices.chunks_exact(3).filter(|c| whole(c)).count();

    let done = run_with(&ball, Some(&values), &locked, ball.triangle_count() / 2);
    assert!(done.reached());
    assert_eq!(done.locked, locked_count);
    let mesh = TriMesh::new(done.positions.clone(), done.indices.clone());
    assert_closed_and_sound(&mesh, "half, with the top locked");
    // Every locked vertex is still there, where it was to the bit, with its value.
    let carried = done.values.as_deref().expect("values were given");
    assert_eq!(carried.len(), done.positions.len());
    let now: std::collections::HashMap<[u32; 3], f32> =
        done.positions.iter().zip(carried).map(|(p, &v)| (p.to_array().map(f32::to_bits), v)).collect();
    for (p, _) in ball.positions.iter().zip(&locked).filter(|(_, l)| **l) {
        assert_eq!(now.get(&p.to_array().map(f32::to_bits)), Some(&1.0), "the locked vertex at {p}");
    }
    // The surface with every corner locked is as it was, triangle for triangle.
    let kept = done
        .triangle_source
        .iter()
        .filter(|&&t| whole(&ball.indices[t as usize * 3..t as usize * 3 + 3]))
        .count();
    assert_eq!(kept, locked_triangles);
    // All of the saving came out of the rest.
    assert!(done.positions.iter().filter(|p| p.y < 0.3).count() < ball.positions.iter().filter(|p| p.y < 0.3).count() / 3);
    // Carried values are what the surface had there, between the ends of the edges that went.
    for (p, &v) in done.positions.iter().zip(carried) {
        assert!((0.0..=1.0).contains(&v));
        assert!((v - value(p)).abs() < 0.15, "value {v} at {p}, where the surface had {}", value(p));
    }

    // With everything but a patch locked, the count cannot be reached, and that is said.
    let mostly: Vec<bool> = ball.positions.iter().map(|p| p.y < 0.97).collect();
    let done = run_with(&ball, None, &mostly, 100);
    assert!(!done.reached() && done.triangle_count() > ball.triangle_count() * 9 / 10);
    assert_closed_and_sound(&TriMesh::new(done.positions, done.indices), "nearly all locked");
    // A list of locks shorter than the mesh locks only what it reaches.
    assert!(run_with(&ball, None, &[true; 10], 1000).reached());
}

#[test]
fn a_triangle_of_the_result_is_a_triangle_of_the_input() {
    let model = lumpy(60, 120);
    let done = run_with(&model, None, &[], model.triangle_count() / 4);
    assert_eq!(done.triangle_source.len(), done.triangle_count());
    let distinct: HashSet<u32> = done.triangle_source.iter().copied().collect();
    assert_eq!(distinct.len(), done.triangle_count(), "each from a triangle of its own");
    assert!(done.triangle_source.iter().all(|&t| (t as usize) < model.triangle_count()));
    // And it is still about where that triangle was: what was hidden with it stays hidden with it.
    let centre = |mesh: &[Vec3], indices: &[u32], t: usize| {
        (mesh[indices[t * 3] as usize] + mesh[indices[t * 3 + 1] as usize] + mesh[indices[t * 3 + 2] as usize]) / 3.0
    };
    for (t, &from) in done.triangle_source.iter().enumerate() {
        let (now, was) = (centre(&done.positions, &done.indices, t), centre(&model.positions, &model.indices, from as usize));
        assert!(now.distance(was) < 0.25, "triangle {t} is {} from where it came from", now.distance(was));
    }
}

#[test]
fn it_can_be_stopped_and_it_says_how_far_it_has_got() {
    let model = lumpy(80, 160);
    let mut said = Vec::new();
    let done = decimate(&model.positions, &model.indices, None, &[], 2000, &mut |at| {
        said.push(at);
        true
    });
    assert!(done.is_some());
    assert!(said.len() > 3 && said[0] == 0.0 && *said.last().unwrap() == 1.0, "{said:?}");
    assert!(said.windows(2).all(|w| w[0] <= w[1]) && said.iter().all(|at| (0.0..=1.0).contains(at)));
    // Stopped before it starts, and part of the way through.
    assert!(decimate(&model.positions, &model.indices, None, &[], 2000, &mut |_| false).is_none());
    let mut calls = 0;
    let stopped = decimate(&model.positions, &model.indices, None, &[], 2000, &mut |_| {
        calls += 1;
        calls < 3
    });
    assert!(stopped.is_none() && calls == 3);
}

#[test]
fn the_same_mesh_gives_the_same_result_on_one_core_and_on_all() {
    let model = lumpy(60, 120);
    let values: Vec<f32> = model.positions.iter().map(|p| p.x.fract().abs()).collect();
    let run = || run_with(&model, Some(&values), &[], model.triangle_count() / 5);
    let one_core = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap().install(run);
    assert_eq!(one_core, run());
    assert_eq!(one_core, run(), "and the same again");
}

#[test]
fn a_model_far_from_its_own_centre_is_decimated_as_well_as_one_on_it() {
    let near = lumpy(60, 120);
    let mut far = near.clone();
    far.positions.iter_mut().for_each(|p| *p += Vec3::new(4000.0, -2500.0, 1000.0));
    let (a, b) = (run(&near, 2000), run(&far, 2000));
    assert_closed_and_sound(&b, "far from its centre");
    // Not the same collapses (the points are rounded differently out there), but as good a shape.
    assert!((a.signed_volume() / near.signed_volume() - 1.0).abs() < 0.01);
    assert!((b.signed_volume() / near.signed_volume() - 1.0).abs() < 0.01, "{}", b.signed_volume());
}

#[test]
fn edges_with_three_sheets_and_slivers_of_nothing_do_not_stop_it() {
    // A ball with a fin standing on one of its edges: three triangles on that edge.
    let mut ball = uv_sphere(30, 60);
    let (a, b) = (ball.indices[300], ball.indices[301]);
    ball.positions.push((ball.positions[a as usize] + ball.positions[b as usize]) * 0.75);
    let tip = ball.positions.len() as u32 - 1;
    ball.indices.extend_from_slice(&[a, b, tip]);
    let before = EdgeCensus::of(&ball);
    assert_eq!(before.crowded, 1);
    let done = run(&ball, 500);
    assert!(done.triangle_count() <= 501);
    let after = EdgeCensus::of(&done);
    assert!(after.crowded <= before.crowded && after.open <= before.open, "{after:?} against {before:?}");

    // A sheet with a triangle of no area in it (three points on a line).
    let mut sheet = grid(20, 20);
    sheet.positions[21 + 5] = (sheet.positions[21 + 4] + sheet.positions[21 + 6]) * 0.5;
    let done = run(&sheet, 100);
    assert!(done.triangle_count() <= 100 && EdgeCensus::of(&done).holes == Some(1));
}

/// Run with `--release -- --ignored --nocapture`. The sizes are the app's
/// own benchmark's (200k and 1M triangles) and one between; the targets are
/// what a game model is cut down to.
#[test]
#[ignore]
fn timing_of_decimation_at_200k_500k_and_1m_triangles() {
    for (rings, segs, name) in [(224, 448, "200k"), (354, 708, "500k"), (500, 1000, "1M")] {
        let model = lumpy(rings, segs);
        let before = model.triangle_count();
        let volume = model.signed_volume();
        for percent in [50, 20] {
            let target = before * percent / 100;
            let mut best = f64::MAX;
            let mut done = None;
            for _ in 0..3 {
                let clock = std::time::Instant::now();
                done = Some(run_with(&model, None, &[], target));
                best = best.min(clock.elapsed().as_secs_f64());
            }
            let done = done.unwrap();
            let mesh = TriMesh::new(done.positions.clone(), done.indices.clone());
            let drift = (mesh.signed_volume() / volume - 1.0).abs();
            println!(
                "decimate {name}: {before} -> {} triangles ({percent}%) in {:.0} ms (best of 3), volume off by {:.4}%",
                mesh.triangle_count(),
                best * 1e3,
                drift * 100.0
            );
            assert!(done.reached() && done.triangle_count() + 1 >= target);
            assert_closed_and_sound(&mesh, name);
            assert_eq!(euler(&mesh), 2);
            assert!(drift < 1e-3, "{name} to {percent}%: volume off by {drift}");
        }
    }
}
