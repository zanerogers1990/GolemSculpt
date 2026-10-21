//! Tests of how the brushes build (2026-10-21, the second part): Standard
//! and Inflate building each stroke to a height, Inflate's normals brought
//! up to date and evened out, Steady Plane on Clay and Flatten, Pinch
//! weighing the surface where the stroke found it, and the settings that
//! start them.

use glam::{DVec3, Vec2, Vec3};

use super::stroke_tests::{across, draw, height, rough_ball, round, sphere, twins, view_of};
use super::*;
use crate::mesh::primitives::{self, Primitive};

const RADIUS: f32 = 0.1;

/// A sculptor holding the base `brush` as it starts, at a fixed size, with
/// `tune` done to its settings.
fn holding(brush: Brush, tune: impl Fn(&mut BrushSettings)) -> Sculptor {
    let mut sculptor = Sculptor::default();
    let s = &mut sculptor.settings;
    (s.brush, s.screen_space_size, s.radius_world) = (brush, false, RADIUS);
    // Clay's finishing pass is paced by the frame, not the dab.
    s.clay_relax = 0.0;
    tune(s.current_mut());
    sculptor
}

/// `passes` passes there and back along `path` in one stroke.
fn passes(path: &[Vec2], passes: usize) -> Vec<Vec2> {
    let mut all = path.to_vec();
    for pass in 1..passes {
        if pass % 2 == 1 {
            all.extend(path.iter().rev().skip(1));
        } else {
            all.extend(path.iter().skip(1));
        }
    }
    all
}

/// The most any vertex stands above the ball, in radii.
fn tallest(mesh: &Mesh) -> f32 {
    mesh.positions.iter().map(|&p| height(p)).fold(0.0f32, f32::max) / RADIUS
}

/// Curvature at `v` times the brush radius: 1/R on a ball of radius R,
/// positive where the surface bulges out.
fn curvature(mesh: &Mesh, v: u32) -> f32 {
    let ring = mesh.neighbours(v);
    let p = mesh.positions[v as usize].as_dvec3();
    let (mut sum, mut len2) = (DVec3::ZERO, 0.0f64);
    for &o in ring {
        let q = mesh.positions[o as usize].as_dvec3();
        sum += q;
        len2 += (q - p).length_squared();
    }
    let n = ring.len() as f64;
    let k = -2.0 * (sum / n - p).dot(mesh.normals[v as usize].as_dvec3()) / (len2 / n).max(1e-30);
    (k * RADIUS as f64) as f32
}

/// How sharply the top of what a stroke raised curves: the mean curvature
/// of the vertices that rose nine tenths as high as the most.
fn crest(mesh: &Mesh) -> f32 {
    let top = tallest(mesh) * RADIUS;
    let ks: Vec<f32> =
        (0..mesh.vertex_count() as u32).filter(|&v| height(mesh.positions[v as usize]) >= 0.9 * top).map(|v| curvature(mesh, v)).collect();
    ks.iter().sum::<f32>() / ks.len().max(1) as f32
}

/// What a stroke of the brush as `sculptor` makes it raises along the front
/// of the ball: one pass, three passes in one stroke, three strokes.
fn builds(sculptor: impl Fn() -> Sculptor) -> [f32; 3] {
    let ball = || sphere(80_000);
    let view = view_of(&ball());
    let line = across(&view, -0.3, 0.3, 60);
    let mut one = ball();
    draw(&mut one, &mut sculptor(), &view, &line, 4, Some(&|_| 1.0));
    let mut three = ball();
    draw(&mut three, &mut sculptor(), &view, &passes(&line, 3), 4, Some(&|_| 1.0));
    let mut strokes = ball();
    for _ in 0..3 {
        draw(&mut strokes, &mut sculptor(), &view, &line, 4, Some(&|_| 1.0));
    }
    [tallest(&one), tallest(&three), tallest(&strokes)]
}

/// Standard and Inflate as they start build a stroke to a height: going over
/// the same place in the stroke twice more adds little and never passes the
/// height, and two more strokes build on top. With Accumulate on, every pass
/// adds as much again.
#[test]
fn standard_and_inflate_build_each_stroke_to_a_height() {
    for brush in [Brush::Standard, Brush::Inflate] {
        let [one, three, strokes] = builds(|| holding(brush, |_| {}));
        println!("{brush:?}: one pass {one:.3}, three passes {three:.3}, three strokes {strokes:.3} radii");
        assert!(one > 0.06 && one < 0.16, "{brush:?}: one pass at its own Strength built {one}");
        assert!(three < one * 1.35, "{brush:?}: three passes in one stroke built {three}, one {one}");
        assert!(strokes > one * 2.5, "{brush:?}: three strokes built {strokes}, one {one}");
        let [one, three, _] = builds(|| holding(brush, |b| b.accumulate = true));
        assert!(three > one * 2.0, "{brush:?} with Accumulate: three passes built {three}, one {one}");
    }
    // At full Strength one stroke stands no taller than its height.
    let [one, three, _] = builds(|| holding(Brush::Standard, |b| b.strength = 1.0));
    assert!(one > 0.25 && three <= 0.35 * 1.001, "Standard at 100%: one pass {one}, three {three}");
}

/// A Standard stroke gone over three times in one stroke is a rounded bead:
/// its top curves about as gently as one pass's, and far more gently than
/// the top of the peaked ridge three passes with Accumulate build.
#[test]
fn a_standard_stroke_gone_over_again_is_a_rounded_bead() {
    let top = |accumulate: bool, count: usize| {
        let mut mesh = sphere(80_000);
        let view = view_of(&mesh);
        let line = passes(&across(&view, -0.3, 0.3, 60), count);
        draw(&mut mesh, &mut holding(Brush::Standard, |b| b.accumulate = accumulate), &view, &line, 4, Some(&|_| 1.0));
        mesh.recompute_all_normals();
        crest(&mesh)
    };
    let (one, three, piled) = (top(false, 1), top(false, 3), top(true, 3));
    println!("crest curvature x radius: one pass {one:.2}, three passes {three:.2}, three with Accumulate {piled:.2}");
    assert!(three < one * 1.2, "three passes {three}, one {one}");
    assert!(three < piled * 0.7, "three passes {three}, three with Accumulate {piled}");
}

/// Without Accumulate Inflate raises each vertex along its own normal as the
/// stroke found the surface: with Relax Normals at nought, every vertex it
/// moved went straight out along the normal it had before the stroke.
#[test]
fn inflate_raises_each_vertex_along_its_own_normal() {
    let mut mesh = rough_ball(60_000);
    mesh.recompute_all_normals();
    let (before, normals) = (mesh.positions.clone(), mesh.normals.clone());
    let view = view_of(&mesh);
    let line = passes(&across(&view, -0.3, 0.3, 60), 3);
    // The kernel alone: Auto Smooth and Even Out move vertices sideways.
    let tune = |b: &mut BrushSettings| (b.strength, b.relax_normals, b.auto_smooth, b.even_out) = (1.0, 0.0, 0.0, 0.0);
    draw(&mut mesh, &mut holding(Brush::Inflate, tune), &view, &line, 4, None);
    let mut moved = 0;
    for ((p, p0), n0) in mesh.positions.iter().zip(&before).zip(&normals) {
        let d = *p - *p0;
        if d.length() > 1e-5 {
            let sideways = (d - *n0 * d.dot(*n0)).length() / d.length();
            assert!(sideways < 1e-3 && d.dot(*n0) > 0.0, "moved {d} against a normal of {n0}");
            moved += 1;
        }
    }
    assert!(moved > 500, "only {moved} vertices moved");
}

/// Relax Normals keeps a strong Inflate stroke smooth on rough clay: the way
/// it moved each vertex differs less from the way it moved its neighbours
/// with the normals evened out, so the surface swells as one and its
/// roughness is not split further apart.
#[test]
fn relax_normals_keeps_inflate_smooth_on_rough_clay() {
    for accumulate in [false, true] {
        let rough = |relax: f32| {
            let mut mesh = rough_ball(60_000);
            let before = mesh.positions.clone();
            let view = view_of(&mesh);
            let line = passes(&across(&view, -0.3, 0.3, 60), 3);
            // Relax Normals alone, without Auto Smooth and Even Out.
            let mut sculptor = holding(Brush::Inflate, |b| {
                (b.strength, b.relax_normals, b.accumulate, b.auto_smooth, b.even_out) = (1.0, relax, accumulate, 0.0, 0.0)
            });
            draw(&mut mesh, &mut sculptor, &view, &line, 4, None);
            // How far each vertex's move near the line is from the mean of
            // its neighbours' (root mean square), against how far it moved.
            let moved = |v: usize| (mesh.positions[v] - before[v]).as_dvec3();
            let (mut off, mut far, mut n) = (0.0f64, 0.0f64, 0.0f64);
            for v in 0..mesh.vertex_count() {
                let p = before[v];
                if p.z > 0.0 && p.y.abs() < 0.3 * RADIUS && round(p).abs() < 0.25 {
                    let ring = mesh.neighbours(v as u32);
                    let mean = ring.iter().map(|&o| moved(o as usize)).sum::<DVec3>() / ring.len() as f64;
                    off += (moved(v) - mean).length_squared();
                    far += moved(v).length_squared();
                    n += 1.0;
                }
            }
            (off / far).sqrt()
        };
        let (plain, relaxed) = (rough(0.0), rough(0.6));
        println!("Inflate (accumulate {accumulate}): moves {plain:.3e} off their neighbours' with Relax Normals at 0, {relaxed:.3e} at 60%");
        assert!(relaxed < plain * 0.9, "accumulate {accumulate}: {relaxed} against {plain}");
    }
}

/// Bringing the normals under a dab up to date gives them exactly as a full
/// recount would, inside the radius, and leaves the rest as they were.
#[test]
fn normals_brought_up_to_date_under_a_dab_are_the_surfaces_own() {
    let mut mesh = sphere(40_000);
    let centre = Vec3::new(0.0, 0.0, 0.5);
    let mut leaves = Vec::new();
    mesh.bvh.leaves_in_sphere(centre, 0.15, &mut leaves);
    leaves.sort_unstable();
    mesh.begin_stroke();
    mesh.move_vertices(&leaves, |_, vt| {
        let d = vt.pos.distance(centre);
        (d < 0.15).then(|| vt.pos + Vec3::new(0.03, 0.01, 0.0) * (1.0 - d / 0.15))
    });
    mesh.positions_changed(&leaves);
    let stale = mesh.normals.clone();
    mesh.refresh_normals_within(&leaves, centre, 0.1);
    // Each vertex's normal from where its triangles' corners are now, worked
    // out as every normal is: the areas' directions added up in double.
    let now = |v: usize| {
        let mut sum = DVec3::ZERO;
        for &t in mesh.vert_tris(v as u32) {
            let [a, b, c] = [0, 1, 2].map(|k| mesh.positions[mesh.indices[t as usize * 3 + k] as usize].as_dvec3());
            sum += (b - a).cross(c - a);
        }
        (sum / sum.length()).as_vec3()
    };
    let mut fresh = 0;
    for v in 0..mesh.vertex_count() {
        if mesh.start_pos(v as u32).distance(centre) < 0.1 {
            assert_eq!(mesh.normals[v], now(v), "vertex {v}");
            fresh += usize::from(stale[v] != now(v));
        } else {
            assert_eq!(mesh.normals[v], stale[v], "vertex {v} is outside the radius");
        }
    }
    assert!(fresh > 100, "only {fresh} normals were stale");
    let _ = mesh.end_stroke(false);
}

/// Standard and Inflate building to a height stay mirror images of
/// themselves across a mirror plane they are drawn over.
#[test]
fn standard_and_inflate_stay_mirror_images() {
    for brush in [Brush::Standard, Brush::Inflate] {
        let mut mesh = sphere(80_000);
        let twins = twins(&mesh);
        let view = view_of(&mesh);
        let mut sculptor = holding(brush, |b| b.strength = 0.8);
        sculptor.settings.symmetry = [true, false, false];
        draw(&mut mesh, &mut sculptor, &view, &passes(&across(&view, -0.05, 0.4, 60), 3), 4, Some(&|t| 0.3 + 0.7 * t));
        let flip = Vec3::new(-1.0, 1.0, 1.0);
        let error = twins.iter().enumerate().map(|(v, &t)| mesh.positions[t as usize].distance(mesh.positions[v] * flip)).fold(0.0f32, f32::max);
        let most = tallest(&mesh);
        println!("{brush:?} mirrored: rose {most:.3} radii, mirror error {error:.2e}");
        assert!(most > 0.05 && error < most * RADIUS * 1e-5, "{brush:?}: rose {most} with an error of {error}");
    }
}

/// A ball with ribs across a stroke along x: rippled in and out along x by
/// `amplitude`, `rib` from crest to crest.
fn ribbed(amplitude: f32, rib: f32) -> Mesh {
    let mesh = primitives::build(Primitive::Sphere, 120_000, 0.5);
    let positions =
        mesh.positions.iter().map(|&p| p * (1.0 + amplitude / 0.5 * (std::f32::consts::TAU * p.x / rib).sin())).collect();
    Mesh::new(positions, mesh.indices.clone(), None)
}

/// How tall the ribs stand along the middle of a stroke across them.
fn ribs(mesh: &Mesh, rib: f32) -> f64 {
    let k = std::f64::consts::TAU / rib as f64;
    let near: Vec<(f64, f64)> = mesh
        .positions
        .iter()
        .filter(|p| p.z > 0.0 && p.y.abs() < 0.2 * RADIUS && p.x.abs() < 0.12)
        .map(|p| (p.x as f64, p.length() as f64))
        .collect();
    let mean = near.iter().map(|h| h.1).sum::<f64>() / near.len() as f64;
    let (s, c) = near.iter().fold((0.0, 0.0), |(s, c), &(x, h)| (s + (h - mean) * (k * x).sin(), c + (h - mean) * (k * x).cos()));
    2.0 * (s * s + c * c).sqrt() / near.len() as f64
}

/// Steady Plane holds the plane Flatten and Clay work to steady over ribs
/// across the stroke: one pass leaves less of the ribs along its middle
/// with it on than with it at nought, and the brushes as they start have it
/// on.
#[test]
fn steady_plane_levels_over_bumps() {
    let rib = 1.5 * RADIUS;
    let ball = || ribbed(0.025 * RADIUS, rib);
    let view = view_of(&ball());
    let start = ribs(&ball(), rib);
    let line = across(&view, -0.3, 0.3, 60);
    for brush in [Brush::Flatten, Brush::Clay] {
        let left = |steady: f32| {
            let mut mesh = ball();
            draw(&mut mesh, &mut holding(brush, |b| (b.strength, b.steady_plane) = (0.5, steady)), &view, &line, 4, Some(&|_| 1.0));
            ribs(&mesh, rib) / start
        };
        let (rocking, steady) = (left(0.0), left(DEFAULT_STEADY_PLANE));
        println!("{brush:?}: ribs left {rocking:.3} at Steady Plane 0, {steady:.3} at 50%");
        assert!(steady < rocking * 0.97, "{brush:?}: {steady} against {rocking}");
        assert_eq!(Settings::default().per_brush[brush.index()].steady_plane, DEFAULT_STEADY_PLANE);
    }
}

/// Pinch draws the surface in to the line of the stroke and never across
/// it, however hard and however often. (Strokes that each go one way: where
/// a stroke turns right round, the way it is going swings through every
/// way across on the dabs at the turn, and those pinch along the line.)
#[test]
fn pinch_never_draws_the_surface_across_its_line() {
    let mut mesh = sphere(60_000);
    let before = mesh.positions.clone();
    let view = view_of(&mesh);
    let line = across(&view, -0.3, 0.3, 60);
    for _ in 0..5 {
        let mut sculptor = holding(Brush::Pinch, |b| (b.strength, b.build_up) = (1.0, 3.0));
        draw(&mut mesh, &mut sculptor, &view, &line, 4, None);
    }
    let mut drawn = 0.0f32;
    for (p, p0) in mesh.positions.iter().zip(&before) {
        if p0.z > 0.0 && p0.y.abs() > 1e-4 {
            // Each dab draws toward its own line, which may lie a hair off
            // the next one's: never across by more than a 200th of a radius.
            assert!(p.y * p0.y.signum() >= -0.005 * RADIUS, "a vertex at {p0} was drawn across the line to {p}");
            drawn = drawn.max(p0.y.abs() - p.y.abs());
        }
    }
    assert!(drawn > 0.1 * RADIUS, "the surface was drawn in by {drawn}");
}

/// Accumulate starts off on every brush that has it; Inflate's Relax
/// Normals and Clay's and Flatten's Steady Plane start on. Clay Strips and
/// Clay Tubes keep each dab's own plane, as they were made with; Surface
/// Noise keeps Accumulate on, as it was made with.
#[test]
fn the_brushes_start_as_they_should() {
    let s = Settings::default();
    for b in Brush::ALL {
        let bs = s.per_brush[b.index()];
        assert!(!bs.accumulate, "{}", b.name());
        let relax = if b == Brush::Inflate { DEFAULT_RELAX_NORMALS } else { 0.0 };
        let steady = if matches!(b, Brush::Clay | Brush::Flatten) { DEFAULT_STEADY_PLANE } else { 0.0 };
        assert_eq!((bs.relax_normals, bs.steady_plane), (relax, steady), "{}", b.name());
    }
    let library = library::Library::default();
    let record = |name: &str| library.record(library.named(name).unwrap(), &s).settings;
    // Trim is steadied less, so that it cuts as deep as it did (shipped.rs).
    for (name, steady) in [("Clay Strips", 0.0), ("Clay Tubes", 0.0), ("Trim", 0.3), ("Fill", 0.5), ("Polish", 0.5)] {
        assert_eq!(record(name).steady_plane, steady, "{name}");
    }
    assert!(record("Surface Noise").accumulate && !record("Rake").accumulate);
}
