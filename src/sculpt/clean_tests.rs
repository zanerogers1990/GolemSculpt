//! Tests of the clean-mesh part of 2026-10-21 (the third): Smooth's Surface
//! mode keeping volume, smoothing along an open rim, Auto Smooth and Even Out
//! behind a brush, Snake Hook's pressure, Thickness and turn, and all of it
//! staying a mirror image of itself under symmetry and one undo step.

use glam::{Vec2, Vec3};

use super::kernels::Ring;
use super::stroke_tests::{across, draw, height, rough_ball, round, sphere, twins, view_of};
use super::*;

const RADIUS: f32 = 0.1;

/// A sculptor holding the base `brush` as it starts, at a fixed size, with
/// `tune` done to its settings.
fn holding(brush: Brush, tune: impl Fn(&mut BrushSettings)) -> Sculptor {
    let mut sculptor = Sculptor::default();
    let s = &mut sculptor.settings;
    (s.brush, s.screen_space_size, s.radius_world) = (brush, false, RADIUS);
    tune(s.current_mut());
    sculptor
}

fn volume(mesh: &Mesh) -> f64 {
    mesh.indices
        .chunks_exact(3)
        .map(|t| {
            let [a, b, c] = [0, 1, 2].map(|k| mesh.positions[t[k] as usize].as_dvec3());
            a.dot(b.cross(c)) / 6.0
        })
        .sum()
}

/// Root mean square of how far each vertex near the front of the ball sits
/// off the middle of its neighbours, along its normal: its fine roughness.
fn roughness(mesh: &Mesh) -> f64 {
    let (mut sum, mut n) = (0.0f64, 0.0f64);
    for v in 0..mesh.vertex_count() as u32 {
        let p = mesh.positions[v as usize];
        if p.z < 0.45 || p.y.abs() > 0.05 {
            continue;
        }
        let ring = mesh.neighbours(v);
        let mean = ring.iter().map(|&o| mesh.positions[o as usize].as_dvec3()).sum::<glam::DVec3>() / ring.len() as f64;
        sum += (mean - p.as_dvec3()).dot(p.normalize().as_dvec3()).powi(2);
        n += 1.0;
    }
    (sum / n.max(1.0)).sqrt()
}

/// A ball with a fine roughness over it, a vertex at a time.
fn noisy_ball() -> Mesh {
    let ball = sphere(60_000);
    let noisy = ball
        .positions
        .iter()
        .enumerate()
        .map(|(i, &p)| p * (1.0 + 0.01 * (((i as u32).wrapping_mul(2_654_435_761) >> 8 & 0xffff) as f32 / 65535.0 - 0.5)))
        .collect();
    Mesh::new(noisy, ball.indices.clone(), None)
}

/// Smooth in Surface mode takes a fine roughness off a ball about as well
/// as plain smoothing (All) at the same Strength, and none of its volume;
/// plain smoothing thins the ball with every stroke. A lump as big as the
/// brush it mostly leaves: that is what All is for, and why Smooth starts
/// with All.
#[test]
fn surface_smoothing_keeps_the_volume_plain_smoothing_takes() {
    let run = |mode: SmoothMode, ball: fn() -> Mesh| {
        let mut mesh = ball();
        let view = view_of(&mesh);
        let (volume_before, rough_before) = (volume(&mesh), roughness(&mesh));
        let line = across(&view, -0.4, 0.4, 60);
        for _ in 0..10 {
            draw(&mut mesh, &mut holding(Brush::Smooth, |b| b.smooth_mode = mode), &view, &line, 4, Some(&|_| 1.0));
        }
        mesh.recompute_all_normals();
        ((volume(&mesh) - volume_before) / RADIUS.powi(3) as f64, roughness(&mesh) / rough_before)
    };
    let (all_dv, all_rough) = run(SmoothMode::All, noisy_ball);
    let (surface_dv, surface_rough) = run(SmoothMode::Surface, noisy_ball);
    println!("ten strokes on fine noise: All took {all_dv:.4} cubic radii, left {all_rough:.3} of it;");
    println!("                           Surface took {surface_dv:.4}, left {surface_rough:.3}");
    assert!(all_dv < -0.01, "All took {all_dv}");
    assert!(surface_dv.abs() < 0.05 * all_dv.abs(), "Surface took {surface_dv}, All {all_dv}");
    assert!(surface_rough < 0.2 && surface_rough < all_rough * 1.6, "Surface left {surface_rough}, All {all_rough}");
    let (_, all_lumps) = run(SmoothMode::All, || rough_ball(60_000));
    let (_, surface_lumps) = run(SmoothMode::Surface, || rough_ball(60_000));
    println!("ten strokes on lumps: All left {all_lumps:.3} of them, Surface {surface_lumps:.3}");
    assert!(all_lumps < surface_lumps * 0.6, "All left {all_lumps} of the lumps, Surface {surface_lumps}");
    assert_eq!(Settings::default().per_brush[Brush::Smooth.index()].smooth_mode, SmoothMode::All);
}

/// The top half of a ball, open along its middle (y = 0): its rim runs
/// across the front of the ball, where a stroke from the front can reach it.
fn open_ball() -> Mesh {
    let ball = sphere(60_000);
    let keep: Vec<u32> = ball
        .indices
        .chunks_exact(3)
        .filter(|t| t.iter().all(|&v| ball.positions[v as usize].y >= -1e-6))
        .flatten()
        .copied()
        .collect();
    Mesh::new(ball.positions.clone(), keep, None)
}

/// Smoothing leaves an open rim on its own line: a vertex on it is averaged
/// with its two neighbours along it, so it smooths along the rim and is not
/// drawn in off it, and the opening keeps its place. (Peaks, Valleys and
/// Relax move a vertex along or across its own normal, which on a rim leans
/// a hair off the line.)
#[test]
fn smoothing_keeps_an_open_rim_on_its_line() {
    for mode in [SmoothMode::All, SmoothMode::Surface] {
        let mut mesh = open_ball();
        let rim: Vec<u32> = (0..mesh.vertex_count() as u32).filter(|&v| Ring::of(&mesh, v) == Ring::Rim).collect();
        assert!(rim.len() > 100, "{} vertices on the rim", rim.len());
        assert!(rim.iter().all(|&v| mesh.positions[v as usize].y.abs() < 1e-6));
        let view = view_of(&mesh);
        // Just above the rim, so the stroke lands on the surface.
        let line: Vec<Vec2> = across(&view, -0.4, 0.4, 60).into_iter().map(|p| p - Vec2::new(0.0, 4.0)).collect();
        let before = mesh.positions.clone();
        draw(&mut mesh, &mut holding(Brush::Smooth, |b| (b.smooth_mode, b.strength) = (mode, 1.0)), &view, &line, 4, Some(&|_| 1.0));
        let moved = rim.iter().map(|&v| mesh.positions[v as usize].distance(before[v as usize])).fold(0.0f32, f32::max);
        let off = rim.iter().map(|&v| mesh.positions[v as usize].y.abs()).fold(0.0f32, f32::max);
        println!("{}: rim moved up to {moved:.2e}, off its line {off:.2e}", mode.name());
        assert!(off < 1e-6, "{}: the rim was drawn {off} off its line", mode.name());
    }
}

/// A vertex with two neighbours (the corner of a lone triangle) is left
/// where it is; one on an open edge is averaged along the edge; one inside a
/// closed surface with all of its neighbours.
#[test]
fn which_neighbours_a_vertex_is_smoothed_with() {
    // Two triangles making a square: two corners with two neighbours, two
    // on the diagonal with three, of which the other diagonal end is inside.
    let square = Mesh::new(
        vec![Vec3::ZERO, Vec3::X, Vec3::new(1.0, 1.0, 0.0), Vec3::Y],
        vec![0, 1, 2, 0, 2, 3],
        None,
    );
    let rings: Vec<Ring> = (0..4).map(|v| Ring::of(&square, v)).collect();
    assert_eq!(rings.iter().filter(|&&r| r == Ring::Held).count(), 2, "{rings:?}");
    assert_eq!(rings.iter().filter(|&&r| r == Ring::Rim).count(), 2, "{rings:?}");
    let ball = sphere(20_000);
    assert!((0..ball.vertex_count() as u32).all(|v| Ring::of(&ball, v) == Ring::All));
}

/// Auto Smooth behind Inflate: a strong swelling on a rough ball comes out
/// smoother, and as tall, as without it. A masked part is left alone.
#[test]
fn auto_smooth_cleans_up_behind_inflate_and_keeps_off_the_mask() {
    let run = |auto: f32| {
        let mut mesh = rough_ball(60_000);
        // Everything on the far side of x = -0.02 is masked.
        for (m, p) in mesh.mask.iter_mut().zip(&mesh.positions) {
            *m = if p.x < -0.02 { 1.0 } else { 0.0 };
        }
        let before = mesh.positions.clone();
        let view = view_of(&mesh);
        let line = across(&view, -0.3, 0.3, 60);
        draw(&mut mesh, &mut holding(Brush::Inflate, |b| (b.strength, b.auto_smooth, b.even_out) = (1.0, auto, 0.0)), &view, &line, 4, Some(&|_| 1.0));
        let masked_moved =
            mesh.positions.iter().zip(&before).zip(&mesh.mask).filter(|(_, m)| **m >= 1.0).map(|((p, q), _)| p.distance(*q)).fold(0.0f32, f32::max);
        mesh.recompute_all_normals();
        let tallest = mesh.positions.iter().filter(|p| p.x > 0.0).map(|&p| height(p)).fold(0.0f32, f32::max) / RADIUS;
        (roughness(&mesh), tallest, masked_moved)
    };
    let (rough0, tall0, _) = run(0.0);
    let (rough, tall, masked) = run(0.3);
    println!("Inflate at 100%: roughness {rough0:.2e} -> {rough:.2e}, tallest {tall0:.3} -> {tall:.3} radii");
    assert!(rough < rough0 * 0.85, "roughness {rough0} -> {rough}");
    assert!((tall - tall0).abs() < 0.1 * tall0, "tallest {tall0} -> {tall}");
    assert_eq!(masked, 0.0, "a masked vertex moved {masked}");
}

/// Auto Smooth and Even Out do as much at any Spacing: what Inflate and
/// Standard build, with them on as they start, is the same at half the
/// brush's own spacing and at twice it as at its own, to a hundredth.
#[test]
fn tidying_builds_the_same_at_any_spacing() {
    for (brush, accumulate) in [(Brush::Inflate, true), (Brush::Inflate, false), (Brush::Standard, false)] {
        let own = brush.default_spacing();
        let built = [own * 0.5, own, own * 2.0].map(|spacing| {
            let mut mesh = sphere(120_000);
            let before = mesh.positions.clone();
            let view = view_of(&mesh);
            let mut sculptor = holding(brush, |b| (b.spacing, b.accumulate, b.strength) = (spacing, accumulate, 0.3));
            sculptor.settings.radius_world = 0.08;
            draw(&mut mesh, &mut sculptor, &view, &across(&view, -0.45, 0.45, 150), 4, None);
            let in_the_middle = |p: &Vec3| p.z > 0.0 && (-0.15..0.15).contains(&round(*p));
            before.iter().zip(&mesh.positions).filter(|(was, _)| in_the_middle(was)).map(|(was, now)| (now.length() - was.length()) as f64).sum::<f64>()
        });
        let (low, high) = built.iter().fold((f64::MAX, 0.0f64), |(l, h), &b| (l.min(b), h.max(b)));
        println!("{brush:?} accumulate {accumulate}: built {built:.4?}");
        assert!(high > 0.0 && (high - low) / high < 0.01, "{brush:?} accumulate {accumulate}: {built:?}");
    }
}

/// A Snake Hook pull from `from` to `to` on the screen, on a fresh ball,
/// pressed at `pen`: the ball after it.
fn pull(tune: impl Fn(&mut BrushSettings), pen: f32, mirror: bool, path: impl Fn(&View) -> Vec<Vec2>) -> (Mesh, Vec<Vec3>) {
    let mut mesh = sphere(60_000);
    let before = mesh.positions.clone();
    let view = view_of(&mesh);
    let mut sculptor = holding(Brush::SnakeHook, tune);
    sculptor.settings.symmetry = [mirror, false, false];
    draw(&mut mesh, &mut sculptor, &view, &path(&view), 2, Some(&move |_| pen));
    (mesh, before)
}

/// Straight out of the ball's outline on the right, three radii.
fn out_of_the_outline(view: &View) -> Vec<Vec2> {
    let x0 = 0.48;
    let from = Vec3::new(x0, 0.0, (0.25f32 - x0 * x0).sqrt());
    let (a, b) = (view.project(from).unwrap(), view.project(from + Vec3::X * 3.0 * RADIUS).unwrap());
    (0..=60).map(|i| a.lerp(b, i as f32 / 60.0)).collect()
}

/// Snake Hook follows the pen's pressure: a light touch pulls the surface a
/// short way, a firm one the whole way.
#[test]
fn snake_hook_follows_the_pens_pressure() {
    let reach = |pen: f32| {
        let (mesh, _) = pull(|_| {}, pen, false, out_of_the_outline);
        mesh.positions.iter().map(|p| p.x).fold(0.0f32, f32::max) - 0.5
    };
    let (light, firm) = (reach(0.15), reach(1.0));
    println!("pulled out {light:.3} at a light touch, {firm:.3} pressed firmly");
    assert!(firm > 2.5 * RADIUS, "a firm pull reached {firm}");
    assert!(light < 0.5 * firm, "a light pull reached {light}, a firm one {firm}");
}

/// With Thickness a long pull keeps its thickness: half way out it is
/// thicker than one pulled without, which thins to a point.
#[test]
fn thickness_keeps_a_long_pull_thick() {
    let thick = |thickness: f32| {
        let (mesh, _) = pull(|b| b.thickness = thickness, 1.0, false, out_of_the_outline);
        let x = 0.5 + 1.5 * RADIUS;
        let slice: Vec<Vec3> = mesh.positions.iter().copied().filter(|p| p.x > x).collect();
        let mid = slice.iter().map(|p| Vec2::new(p.y, p.z)).sum::<Vec2>() / slice.len().max(1) as f32;
        let spread = slice.iter().map(|p| Vec2::new(p.y, p.z).distance(mid)).fold(0.0f32, f32::max);
        (slice.len(), spread / RADIUS)
    };
    let ((count0, spread0), (count, spread)) = (thick(0.0), thick(crate::sculpt::DEFAULT_THICKNESS));
    println!("past 1.5 radii out: {count0} vertices {spread0:.3} radii across without, {count} {spread:.3} with");
    assert!(spread > spread0 * 1.2, "{spread0} -> {spread}");
}

/// Under the mirror, a pull that curves (turning the tip) across the plane,
/// Inflate with Auto Smooth and Even Out, and a Move with Even Out at the
/// release each leave the ball a mirror image of itself.
#[test]
fn tidying_and_turning_stay_mirror_images() {
    let flip = Vec3::new(-1.0, 1.0, 1.0);
    let check = |name: &str, mesh: &Mesh, before: &[Vec3]| {
        let twins = twins(&sphere(60_000));
        let (mut worst, mut biggest) = (0.0f32, 0.0f32);
        for (v, &t) in twins.iter().enumerate() {
            worst = worst.max(mesh.positions[t as usize].distance(mesh.positions[v] * flip));
            biggest = biggest.max(mesh.positions[v].distance(before[v]));
        }
        println!("{name}: moved {biggest:.4}, mirror error {worst:.2e}");
        assert!(biggest > 1e-3, "{name} did nothing");
        assert!(worst <= biggest * 1e-5, "{name}: mirror error {worst} of {biggest}");
    };
    let curve = |view: &View| {
        let c = view.project(Vec3::new(0.0, 0.0, 0.5)).unwrap();
        (0..=60).map(|i| c + Vec2::new(-90.0 + 3.0 * i as f32, 60.0 * ((i as f32) * 0.1).sin())).collect::<Vec<_>>()
    };
    let (mesh, before) = pull(|b| (b.even_out, b.auto_smooth) = (0.3, 0.1), 1.0, true, curve);
    check("Snake Hook", &mesh, &before);
    for brush in [Brush::Inflate, Brush::Move] {
        let mut mesh = sphere(60_000);
        let before = mesh.positions.clone();
        let view = view_of(&mesh);
        let mut sculptor = holding(brush, |b| (b.strength, b.even_out) = (1.0, 0.4));
        sculptor.settings.symmetry = [true, false, false];
        draw(&mut mesh, &mut sculptor, &view, &curve(&view), 2, Some(&|_| 1.0));
        check(brush.name(), &mesh, &before);
    }
}

/// Move's Even Out is part of the drag's stroke: one undo takes the drag and
/// the evening out back together, to the bit; and it does even the
/// triangles out, without moving what is under the pen.
#[test]
fn move_evens_out_inside_its_own_undo_step() {
    let drag = |even: f32| {
        let mut mesh = sphere(60_000);
        let before = mesh.positions.clone();
        let view = view_of(&mesh);
        let mut sculptor = holding(Brush::Move, |b| b.even_out = even);
        let c = view.project(Vec3::new(0.0, 0.0, 0.5)).unwrap();
        let sample = |i: usize| PointerSample { pos: c + Vec2::new(4.0 * i as f32, 0.0), pressure: None, time: i as f64 / 60.0 };
        assert!(sculptor.begin(&mut mesh, &view, sample(0), false, false));
        for i in 1..=30 {
            sculptor.sample(&mut mesh, &view, sample(i));
            sculptor.frame(&mut mesh, &view, 1.0 / 60.0);
        }
        let delta = sculptor.end(&mut mesh, &view).expect("the drag moved the surface");
        let after = mesh.positions.clone();
        mesh.apply_delta(&delta, true);
        assert!(mesh.positions == before, "one undo puts the whole drag back");
        (after, before)
    };
    let (plain, before) = drag(0.0);
    let (evened, _) = drag(0.5);
    // The vertex nearest the point taken hold of stays where the pen took it.
    let held = (0..before.len()).min_by(|&a, &b| before[a].distance(Vec3::new(0.0, 0.0, 0.5)).total_cmp(&before[b].distance(Vec3::new(0.0, 0.0, 0.5)))).unwrap();
    assert!(evened[held].distance(plain[held]) < 1e-3 * RADIUS, "the grabbed point slid {}", evened[held].distance(plain[held]));
    let changed = plain.iter().zip(&evened).filter(|(a, b)| a != b).count();
    assert!(changed > 100, "Even Out moved {changed} vertices");
}

