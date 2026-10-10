//! Brush engine tests: falloff curves and focal shift, the cursor's inner
//! ring, pen pressure, Lazy Mouse, Move's grab, mask painting and brush size.
//! The whole-stroke tests every brush shares (undo, mirror exactness) are in
//! `crate::tests`.

use glam::{Vec2, Vec3};

use super::falloff::MAX_FOCAL_BIAS;
use super::input::{LazyRope, PenPressure};
use super::kernels;
use super::*;
use crate::camera::{Camera, View};
use crate::mesh::primitives::{self, Primitive};

const SIZE: Vec2 = Vec2::new(1600.0, 900.0);

fn sphere(tris: usize) -> Mesh {
    primitives::build(Primitive::Sphere, tris, 0.5)
}

fn view_of(mesh: &Mesh) -> View {
    let mut camera = Camera::default();
    camera.frame(mesh.bounds(), SIZE.x / SIZE.y, false);
    camera.view(Vec2::ZERO, SIZE, camera.distance, 1.0)
}

fn line(from: Vec2, to: Vec2, n: usize) -> Vec<Vec2> {
    (0..=n).map(|i| from.lerp(to, i as f32 / n as f32)).collect()
}

/// Drags the pointer along `points` (pixels), a few samples per frame.
fn run_stroke(mesh: &mut Mesh, sculptor: &mut Sculptor, view: &View, points: &[Vec2]) -> Option<StrokeDelta> {
    let sample = |i: usize| PointerSample { pos: points[i], pressure: None, time: i as f64 / 240.0 };
    assert!(sculptor.begin(mesh, view, sample(0), false, false), "stroke missed the mesh");
    for i in 1..points.len() {
        sculptor.sample(mesh, view, sample(i));
        if i % 4 == 0 {
            sculptor.frame(mesh, view, 1.0 / 60.0);
            let _ = mesh.take_dirty();
        }
    }
    let delta = sculptor.end(mesh, view);
    let _ = mesh.take_dirty();
    delta
}

/// A sculptor holding `brush` at a fixed size on the model.
fn sculptor_with(brush: Brush, radius: f32) -> Sculptor {
    let mut sculptor = Sculptor::default();
    let s = &mut sculptor.settings;
    s.brush = brush;
    s.screen_space_size = false;
    s.radius_world = radius;
    s.per_brush[brush.index()].strength = 0.5;
    sculptor
}

/// One press of `sculptor`'s brush on the point of the mesh nearest the
/// camera along the ray to `at`, left open so the test can look at the stroke.
fn press(mesh: &mut Mesh, sculptor: &mut Sculptor, view: &View, at: Vec3) {
    let pos = view.project(at).expect("the point is in front of the camera");
    assert!(sculptor.begin(mesh, view, PointerSample { pos, pressure: None, time: 0.0 }, false, false));
}

fn ring(settings: &Settings) -> f32 {
    settings.half_weight_radius(settings.brush)
}

// ------------------------------------------------------------ focal shift

#[test]
fn zero_shift_is_bitwise_identity() {
    let none = Falloff::new(0.0, None);
    assert_eq!(focal_k(0.0), 0.0);
    for i in 0..=1000 {
        let t = i as f32 / 1000.0;
        assert_eq!(shifted(t, focal_k(0.0)).to_bits(), t.to_bits());
        // Every brush's weight is what it computed before there were curves.
        let same = |new: f32, old: f32| assert_eq!(new.to_bits(), old.to_bits(), "at {t}");
        same(none.at(t, kernels::smooth), kernels::smooth(1.0 - t));
        same(Falloff::BUILT_IN.at(t, kernels::smooth), kernels::smooth(1.0 - t));
        same(none.at(t, kernels::quintic), kernels::quintic(1.0 - t));
        same(none.at(t, kernels::layer_built_in), kernels::quintic((1.0 - t) / (1.0 - 0.35)));
        same(none.at(t, kernels::crease_built_in), kernels::smooth(1.0 - t).powi(3));
        for soft in [0.05, 0.6, 1.0] {
            // Past the edge too: Clay measures against a swept tip.
            let u = t * 1.5;
            same(kernels::clay_weight(&none, u, soft), kernels::clay_falloff(1.0 - u, soft));
        }
    }
}

#[test]
fn shift_keeps_the_ends_and_stays_monotone() {
    for focal in [-1.0, -0.5, 0.3, 1.0] {
        let k = focal_k(focal);
        assert!(shifted(0.0, k).abs() < 1e-6);
        assert!((shifted(1.0, k) - 1.0).abs() < 1e-6);
        let mut prev = shifted(0.0, k);
        for i in 1..=200 {
            let s = shifted(i as f32 / 200.0, k);
            assert!(s >= prev, "focal {focal} goes backwards at {i}");
            prev = s;
        }
    }
}

#[test]
fn negative_is_harder_at_every_distance() {
    let weight = |focal: f32, u: f32| Falloff::new(focal, None).at(u, kernels::smooth);
    for i in 1..100 {
        let u = i as f32 / 100.0;
        assert!(weight(-0.5, u) > weight(0.0, u), "u={u}");
        assert!(weight(0.5, u) < weight(0.0, u), "u={u}");
    }
}

/// The smoothstep's half-weight radius is exactly 1 - b = 0.5 - 0.45 * focal.
#[test]
fn cursor_ring_tracks_focal_shift_on_smoothstep_brushes() {
    let mut settings = Settings::default();
    settings.brush = Brush::Inflate;
    for focal in [-1.0, -0.4, 0.0, 0.6, 1.0] {
        settings.current_mut().focal_shift = focal;
        assert!((ring(&settings) - (0.5 - MAX_FOCAL_BIAS * focal)).abs() < 2e-3, "focal {focal}: {}", ring(&settings));
    }
}

#[test]
fn every_brush_has_a_ring_that_moves_with_the_shift() {
    for brush in Brush::ALL {
        let mut settings = Settings::default();
        settings.brush = brush;
        settings.current_mut().focal_shift = -0.5;
        let hard = ring(&settings);
        settings.current_mut().focal_shift = 0.5;
        let soft = ring(&settings);
        assert!(hard > soft, "{brush:?}: {hard} vs {soft}");
        assert!(soft > 0.0, "{brush:?}");
        assert!(hard <= 1.0, "{brush:?}");
    }
}

#[test]
fn cursor_ring_follows_a_custom_curve() {
    let mut settings = Settings::default();
    settings.brush = Brush::Inflate;
    let slot = Brush::Inflate.index();
    // Half weight at half radius.
    settings.falloff[slot] = Some(FalloffCurve::preset(FalloffPreset::Linear));
    assert!((ring(&settings) - 0.5).abs() < 2e-3, "{}", ring(&settings));
    // Full weight to 0.6, 0.35 at 0.85.
    settings.falloff[slot] = Some(FalloffCurve::preset(FalloffPreset::Plateau));
    assert!(ring(&settings) > 0.6 && ring(&settings) < 0.85, "{}", ring(&settings));
    // A curve belongs to its brush.
    assert!((settings.half_weight_radius(Brush::Smooth) - 0.5).abs() < 2e-3);
}

/// Clay used to weigh vertices by the unshifted distance while its ring
/// showed the shift.
#[test]
fn clay_follows_its_focal_shift() {
    let built = |focal: f32| {
        let mut mesh = sphere(80_000);
        let view = view_of(&mesh);
        let mut sculptor = sculptor_with(Brush::Clay, 0.12);
        sculptor.settings.clay_relax = 0.0;
        sculptor.settings.current_mut().focal_shift = focal;
        let c = SIZE * 0.5;
        run_stroke(&mut mesh, &mut sculptor, &view, &line(c - Vec2::new(60.0, 0.0), c + Vec2::new(60.0, 0.0), 40));
        mesh.positions.iter().map(|p| (p.length() - 0.5).max(0.0) as f64).sum::<f64>()
    };
    let (hard, neutral, soft) = (built(-0.8), built(0.0), built(0.8));
    assert!(hard > neutral * 1.05 && neutral > soft * 1.05, "{hard} / {neutral} / {soft}");
}

// ---------------------------------------------------------- falloff curves

#[test]
fn falloff_curve_passes_through_its_points_without_overshoot() {
    for preset in FalloffPreset::ALL {
        let curve = FalloffCurve::preset(preset);
        for p in &curve.points {
            assert!((curve.evaluate(p.x) - p.y).abs() < 1e-5, "{preset:?} misses its point {p}");
        }
        // Every preset falls from centre to edge; the curve must never rise.
        let mut last = curve.evaluate(0.0);
        for i in 1..=200 {
            let y = curve.evaluate(i as f32 / 200.0);
            assert!(y <= last + 1e-6, "{preset:?} rises at {}", i as f32 / 200.0);
            assert!((0.0..=1.0).contains(&y));
            last = y;
        }
    }

    // The baked table reproduces the curve.
    let plateau = Falloff::new(0.0, Some(&FalloffCurve::preset(FalloffPreset::Plateau)));
    assert!((plateau.at(0.3, |_| -1.0) - 1.0).abs() < 1e-3);
    assert!(plateau.at(1.0, |_| -1.0).abs() < 1e-3);
    // With no curve the built-in falloff passes through untouched.
    assert_eq!(Falloff::BUILT_IN.at(0.5, |_| 0.123), 0.123);
}

#[test]
fn falloff_curve_stays_valid_whatever_is_done_to_its_points() {
    let mut curve = FalloffCurve { points: vec![Vec2::new(0.7, 2.0), Vec2::new(0.2, -1.0), Vec2::new(0.4, 0.5)] };
    curve.normalize();
    let xs: Vec<f32> = curve.points.iter().map(|p| p.x).collect();
    assert_eq!(xs, [0.0, 0.4, 1.0], "in order, with the ends pinned");
    assert!(curve.points.iter().all(|p| (0.0..=1.0).contains(&p.y)));

    let mut empty = FalloffCurve { points: Vec::new() };
    empty.normalize();
    assert_eq!(empty.points, [Vec2::new(0.0, 1.0), Vec2::new(1.0, 0.0)]);

    // Two points on top of each other are a step, not a division by zero.
    let step = FalloffCurve { points: vec![Vec2::new(0.0, 1.0), Vec2::new(0.5, 1.0), Vec2::new(0.5, 0.0), Vec2::new(1.0, 0.0)] };
    for i in 0..=100 {
        assert!(step.evaluate(i as f32 / 100.0).is_finite());
    }
}

/// A stroke weighs vertices with its brush's curve: flat to the edge, every
/// vertex under one dab moves alike; all zero, nothing moves at all.
#[test]
fn strokes_follow_the_custom_curve() {
    let flat = |weight: f32| FalloffCurve { points: vec![Vec2::new(0.0, weight), Vec2::new(1.0, weight)] };
    let dab = |curve: Option<FalloffCurve>| {
        let mut mesh = sphere(80_000);
        let view = view_of(&mesh);
        let before = mesh.positions.clone();
        let mut sculptor = sculptor_with(Brush::Inflate, 0.1);
        sculptor.settings.falloff[Brush::Inflate.index()] = curve;
        press(&mut mesh, &mut sculptor, &view, Vec3::new(0.0, 0.0, 0.5));
        sculptor.end(&mut mesh, &view);
        let moves: Vec<f32> =
            mesh.positions.iter().zip(&before).map(|(a, b)| a.distance(*b)).filter(|&d| d > 0.0).collect();
        moves
    };
    let built_in = dab(None);
    let (least, most) = built_in.iter().fold((f32::MAX, 0.0f32), |(lo, hi), &d| (lo.min(d), hi.max(d)));
    assert!(built_in.len() > 200 && least < most * 0.1, "the built-in falloff tapers: {least} .. {most}");

    let even = dab(Some(flat(1.0)));
    let (least, most) = even.iter().fold((f32::MAX, 0.0f32), |(lo, hi), &d| (lo.min(d), hi.max(d)));
    assert!(even.len() >= built_in.len(), "full weight right to the edge reaches no less of the surface");
    assert!(least > most * 0.999, "a flat curve moves every vertex alike: {least} .. {most}");

    assert!(dab(Some(flat(0.0))).is_empty(), "a curve of no weight moves nothing");
}

#[test]
fn move_grab_follows_the_custom_curve_too() {
    let mut mesh = sphere(80_000);
    let view = view_of(&mesh);
    let mut sculptor = sculptor_with(Brush::Move, 0.1);
    sculptor.settings.falloff[Brush::Move.index()] =
        Some(FalloffCurve { points: vec![Vec2::new(0.0, 1.0), Vec2::new(1.0, 1.0)] });
    press(&mut mesh, &mut sculptor, &view, Vec3::new(0.0, 0.0, 0.5));
    let grab = sculptor.stroke.as_ref().unwrap().grab.as_ref().unwrap();
    assert!(grab.verts.len() > 200);
    assert!(grab.weights.iter().all(|&w| w == 1.0));
}

// ------------------------------------------------------------- Lazy Mouse

#[test]
fn rope_passes_the_pointer_through_when_off_or_not_pressed() {
    let mut rope = LazyRope::default();
    let p = Vec2::new(10.0, 20.0);
    assert_eq!(rope.advance(p, 0.016, true, false, 25.0, 1.0), p);
    assert!(!rope.active);
    assert_eq!(rope.advance(p, 0.016, false, true, 25.0, 1.0), p);
    assert!(!rope.active);
    assert_eq!(rope.raw, p);
}

#[test]
fn rope_starts_under_the_pointer_then_trails_it_taut_at_full_strength() {
    let mut rope = LazyRope::default();
    let start = Vec2::new(100.0, 100.0);
    assert_eq!(rope.advance(start, 0.0, true, true, 25.0, 1.0), start, "a fresh stroke starts under the pointer");
    assert!(rope.active);

    assert_eq!(
        rope.advance(start + Vec2::new(20.0, 0.0), 0.016, true, true, 25.0, 1.0),
        start,
        "movement inside the rope's length never reaches the brush"
    );

    let pulled = rope.advance(start + Vec2::new(100.0, 0.0), 0.016, true, true, 25.0, 1.0);
    assert!(pulled.distance(start + Vec2::new(75.0, 0.0)) < 1e-4, "taut: exactly one rope length behind");

    // Two samples sharing a timestamp still pull the brush along.
    let again = rope.advance(start + Vec2::new(140.0, 0.0), 0.0, true, true, 25.0, 1.0);
    assert!(again.distance(start + Vec2::new(115.0, 0.0)) < 1e-4, "taut at a zero-length step too");

    rope.advance(start, 0.016, false, true, 25.0, 1.0);
    let restart = Vec2::new(400.0, 300.0);
    assert_eq!(rope.advance(restart, 0.016, true, true, 25.0, 1.0), restart, "the next stroke starts fresh");
}

#[test]
fn rope_follow_is_independent_of_the_step_rate() {
    // Half strength over one 60 Hz frame takes up half the slack; two 120 Hz steps must too.
    assert!((LazyRope::follow(0.5, 1.0 / 60.0) - 0.5).abs() < 1e-5);
    let remaining = (1.0 - LazyRope::follow(0.5, 1.0 / 120.0)) * (1.0 - LazyRope::follow(0.5, 1.0 / 120.0));
    assert!((remaining - 0.5).abs() < 1e-5);
    assert_eq!(LazyRope::follow(1.0, 0.0), 1.0, "full strength is taut even for a zero-length step");
}

/// The rope as the Unity app first computed it, kept as the reference the
/// one in use must match bit for bit.
#[derive(Default)]
struct OriginalRope {
    pos: Vec2,
    active: bool,
}

impl OriginalRope {
    fn advance(&mut self, raw: Vec2, dt: f32, pressed: bool, enabled: bool, radius: f32, strength: f32) -> Vec2 {
        if !enabled || !pressed {
            self.active = false;
            return raw;
        }
        if !self.active {
            self.active = true;
            self.pos = raw;
            return self.pos;
        }
        let delta = raw - self.pos;
        let dist = delta.length();
        if dist > radius {
            let follow = if strength >= 1.0 { 1.0 } else { 1.0 - (1.0 - strength).powf(dt.max(0.0) * 60.0) };
            self.pos += delta / dist * ((dist - radius) * follow);
        }
        self.pos
    }
}

#[test]
fn rope_matches_the_original_implementation_bit_for_bit() {
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut random = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 40) as f32 / (1u64 << 24) as f32
    };
    for strength in [1.0, 0.6, 0.05] {
        let mut rope = LazyRope::default();
        let mut original = OriginalRope::default();
        let mut pointer = Vec2::new(500.0, 400.0);
        for i in 0..2000 {
            pointer += Vec2::new((random() - 0.5) * 60.0, (random() - 0.5) * 60.0);
            let dt = random() * 0.03;
            // Strokes with short gaps between them.
            let pressed = i % 300 < 280;
            let a = rope.advance(pointer, dt, pressed, true, 25.0, strength);
            let b = original.advance(pointer, dt, pressed, true, 25.0, strength);
            assert!(a.x.to_bits() == b.x.to_bits() && a.y.to_bits() == b.y.to_bits(), "step {i} at strength {strength}: {a} vs {b}");
        }
    }
}

// ------------------------------------------------------------ pen pressure

/// Unity's version of this test feeds the filter the pen's hover samples.
/// Here they never reach it: a hovering pen arrives with no pressure, like a
/// mouse. So it checks the same promise from this side: samples without
/// pressure do not drag the held value toward 0, and the touch that follows
/// picks up from it.
#[test]
fn samples_without_pressure_leave_pen_pressure_alone() {
    let mut pen = PenPressure { smoothed: 0.6, last_time: 1.0 };
    pen.step(None, 1.300);
    pen.step(None, 1.305);
    assert_eq!(pen.smoothed, 0.6, "hover does not drag the held pressure toward 0");

    pen.step(Some(0.6), 1.310);
    assert!((pen.smoothed - 0.6).abs() < 1e-6, "the touch picks up from the held value");
    assert_eq!(pen.last_time, 1.310);
}

#[test]
fn tip_down_samples_ease_toward_their_pressure() {
    let mut pen = PenPressure { smoothed: 0.2, last_time: 1.0 };
    // 1/60 s at 20/s eases a third of the way: 0.2 + (0.8 - 0.2) / 3.
    pen.step(Some(0.8), 1.0 + 1.0 / 60.0);
    assert!((pen.smoothed - 0.4).abs() < 1e-5);

    // A mouse sample moves only the clock.
    pen.step(None, 2.0);
    assert!((pen.smoothed - 0.4).abs() < 1e-5);
    assert_eq!(pen.last_time, 2.0);

    // However long the pen was lifted, a touch eases on by one frame's worth.
    pen.step(Some(1.0), 60.0);
    assert!((pen.smoothed - 0.6).abs() < 1e-5);
}

/// The filter used to start at 0, so the first pen stroke after the app
/// opened began far weaker than the pen was pressing.
#[test]
fn the_first_pen_stroke_of_a_session_opens_at_full_pressure() {
    let mut mesh = sphere(20_000);
    let view = view_of(&mesh);
    let mut sculptor = Sculptor::default();
    sculptor.settings.brush = Brush::Inflate;
    let sample = PointerSample { pos: SIZE * 0.5, pressure: Some(1.0), time: 12.0 };
    assert!(sculptor.begin(&mut mesh, &view, sample, false, false));
    assert!((sculptor.stroke.as_ref().unwrap().pressure - 1.0).abs() < 1e-6);
}

// ------------------------------------------------------------------- Move

/// Two balls of radius 0.2 with a 0.04 gap between them, one each side of
/// the X = 0 plane.
fn two_spheres() -> Mesh {
    let ball = sphere(20_000);
    let n = ball.vertex_count() as u32;
    let mut positions = Vec::new();
    let mut indices = Vec::new();
    for (k, x) in [-0.22f32, 0.22].into_iter().enumerate() {
        positions.extend(ball.positions.iter().map(|&p| p * 0.4 + Vec3::new(x, 0.0, 0.0)));
        indices.extend(ball.indices.iter().map(|&i| i + n * k as u32));
    }
    Mesh::new(positions, indices, None)
}

#[test]
fn move_connected_only_leaves_the_neighbouring_piece_alone() {
    let mut mesh = two_spheres();
    let view = view_of(&mesh);
    // On the left ball, close to the gap; the ring reaches across it.
    let at = Vec3::new(-0.03, 0.0, (0.04f32 - 0.19 * 0.19).sqrt());
    let mut grab = |connected: bool| {
        let mut sculptor = sculptor_with(Brush::Move, 0.15);
        sculptor.settings.move_connected = connected;
        press(&mut mesh, &mut sculptor, &view, at);
        let verts = sculptor.stroke.as_ref().unwrap().grab.as_ref().unwrap().verts.clone();
        sculptor.end(&mut mesh, &view);
        verts
    };
    let (all, connected) = (grab(false), grab(true));
    let on_right = |verts: &[u32]| verts.iter().filter(|&&v| mesh.positions[v as usize].x > 0.0).count();
    assert!(on_right(&all) > 0, "the radius should reach the other ball, or the test proves nothing");
    assert_eq!(on_right(&connected), 0, "Connected Only grabbed the other piece");
    assert_eq!(connected.len(), all.len() - on_right(&all), "Connected Only dropped part of the piece under the pointer");
    assert!(connected.windows(2).all(|w| w[0] < w[1]), "still in ascending order, so the drag stays on all threads");
}

/// With symmetry on, each copy of the grab keeps its own piece.
#[test]
fn move_connected_only_holds_for_the_mirrored_copy() {
    let mut mesh = two_spheres();
    let view = view_of(&mesh);
    let at = Vec3::new(-0.03, 0.0, (0.04f32 - 0.19 * 0.19).sqrt());
    let mut sculptor = sculptor_with(Brush::Move, 0.15);
    sculptor.settings.move_connected = true;
    sculptor.settings.symmetry = [true, false, false];
    press(&mut mesh, &mut sculptor, &view, at);
    let grab = sculptor.stroke.as_ref().unwrap().grab.as_ref().unwrap();
    let (mut left, mut right) = (0, 0);
    for (slot, &v) in grab.verts.iter().enumerate() {
        let (own, mirrored) = (grab.weights[slot * 2], grab.weights[slot * 2 + 1]);
        assert!(own > 0.0 || mirrored > 0.0, "a vertex no copy holds was kept");
        if mesh.positions[v as usize].x < 0.0 {
            assert_eq!(mirrored, 0.0, "the mirrored copy reached back across the gap");
            left += 1;
        } else {
            assert_eq!(own, 0.0, "the copy under the pointer reached across the gap");
            right += 1;
        }
    }
    assert!(left > 50 && left == right, "{left} / {right}");
}

/// Front Facing Only on Move used to be a hard cut; it is the same soft ramp
/// every other brush has.
#[test]
fn move_front_facing_fades_out_toward_the_silhouette() {
    let mut mesh = sphere(80_000);
    let view = view_of(&mesh);
    // Well round toward the edge of the ball as the camera sees it.
    let at = Vec3::new(0.46, 0.0, (0.25f32 - 0.46 * 0.46).sqrt());
    let mut weights = |front_facing: bool| {
        let mut sculptor = sculptor_with(Brush::Move, 0.2);
        sculptor.settings.current_mut().front_facing = front_facing;
        press(&mut mesh, &mut sculptor, &view, at);
        let grab = sculptor.stroke.as_ref().unwrap().grab.as_ref().unwrap();
        let pairs: Vec<(u32, f32)> = grab.verts.iter().copied().zip(grab.weights.iter().copied()).collect();
        sculptor.end(&mut mesh, &view);
        pairs
    };
    let (plain, facing) = (weights(false), weights(true));
    assert!(facing.len() < plain.len(), "the far side is left out");
    let plain: std::collections::HashMap<u32, f32> = plain.into_iter().collect();
    let partly = facing.iter().filter(|&&(v, w)| w > plain[&v] * 0.05 && w < plain[&v] * 0.95).count();
    assert!(partly > 20, "only {partly} vertices sit on the ramp: the edge is a hard cut");
}

// ------------------------------------------------------------------- mask

/// How squarely the surface at `p` (with normal `n`) faces the camera: 1 head
/// on, 0 edge on.
fn facing_cos(view: &View, p: Vec3, n: Vec3) -> f32 {
    n.dot((view.pos - p).normalize())
}

fn mask_stroke(setup: impl FnOnce(&mut Settings)) -> Mesh {
    let mut mesh = sphere(80_000);
    let view = view_of(&mesh);
    let mut sculptor = Sculptor::default();
    sculptor.settings.screen_space_size = false;
    sculptor.settings.radius_world = 0.2;
    sculptor.settings.mask_mode = true;
    setup(&mut sculptor.settings);
    // Out toward the silhouette, ending where the surface has turned most of
    // the way from the camera: the ring reaches on round past that.
    let on_ball = |angle: f32| Vec3::new(angle.sin(), 0.0, angle.cos()) * 0.5;
    let turned = (0..900)
        .map(|i| (i as f32 * 0.1).to_radians())
        .find(|&a| facing_cos(&view, on_ball(a), on_ball(a) * 2.0) <= 0.3)
        .expect("the ball turns away from the camera somewhere");
    let px = |angle: f32| view.project(on_ball(angle)).unwrap();
    let path = line(px(turned * 0.5), px(turned), 60);
    run_stroke(&mut mesh, &mut sculptor, &view, &path);
    mesh
}

/// Front Facing Only belongs to the selected brush; a mask is painted where
/// the ring is, whatever that brush's setting.
#[test]
fn mask_paint_ignores_the_brush_front_facing_setting() {
    let plain = mask_stroke(|_| {});
    let facing = mask_stroke(|s| s.current_mut().front_facing = true);
    assert!(plain.mask == facing.mask);
    // And the stroke did reach surface the setting would have held back.
    let view = view_of(&plain);
    let grazing = (0..plain.vertex_count())
        .filter(|&v| plain.mask[v] > 0.2 && facing_cos(&view, plain.positions[v], plain.normals[v]) < 0.15)
        .count();
    assert!(grazing > 20, "only {grazing} masked vertices face away: the test proves nothing");
}

/// Mask dabs are paced the same whichever brush is selected. With Crease
/// selected they used to come twice as close together.
#[test]
fn mask_paint_is_paced_the_same_whichever_brush_is_selected() {
    let with_move = mask_stroke(|s| s.brush = Brush::Move);
    let with_crease = mask_stroke(|s| s.brush = Brush::Crease);
    assert!(with_move.mask.iter().any(|&m| m > 0.0));
    assert!(with_move.mask == with_crease.mask);
}

// ------------------------------------------------------------- Clay relax

/// The finishing pass measures curvature against the mesh's own average, in
/// the Unity app's units: a ball reads -0.5 all over at any size and any
/// density, so nothing on it stands out.
#[test]
fn clay_relax_measures_curvature_against_the_mesh_itself() {
    for (tris, size) in [(20_000, 1.0f32), (80_000, 1.0), (20_000, 40.0)] {
        let ball = sphere(tris);
        let positions: Vec<Vec3> = ball.positions.iter().map(|&p| p * size).collect();
        let mesh = Mesh::new(positions, ball.indices.clone(), None);
        let base = kernels::curvature_base(&mesh);
        assert!((base.scale - 0.5 * size).abs() < 1e-3 * size);
        assert!((base.mean + 0.5).abs() < 0.02, "{tris} tris x{size}: mean {}", base.mean);
        let worst = (0..mesh.vertex_count() as u32)
            .map(|v| (kernels::raw_curvature(&mesh, v, base.scale) - base.mean).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 0.5, "{tris} tris x{size}: a ball departs from itself by {worst}");
    }

    // On a cube the flat of a face is ordinary and an edge is not. (Its
    // average is not far off a ball's: the edges are few but very sharp.)
    let cube = primitives::build(Primitive::Cube, 20_000, 0.5);
    let base = kernels::curvature_base(&cube);
    let departure_near = |at: Vec3| {
        let nearest = |a: &u32, b: &u32| {
            cube.positions[*a as usize].distance(at).total_cmp(&cube.positions[*b as usize].distance(at))
        };
        let v = (0..cube.vertex_count() as u32).min_by(nearest).unwrap();
        (kernels::raw_curvature(&cube, v, base.scale) - base.mean).abs()
    };
    let (face, edge) = (departure_near(Vec3::new(0.0, 0.0, 0.5)), departure_near(Vec3::new(0.5, 0.5, 0.0)));
    assert!(face < 1.0, "the middle of a face departs by {face}");
    assert_eq!(kernels::relax_gate(face), 0.08);
    assert!(edge > 3.0 && kernels::relax_gate(edge) > 0.08, "an edge departs by only {edge}");

    // Out of the way of ordinary shaping, full strength on a real pinch.
    assert_eq!(kernels::relax_gate(0.0), 0.08);
    assert_eq!(kernels::relax_gate(3.0), 0.08);
    assert!((kernels::relax_gate(9.0) - 0.54).abs() < 1e-6);
    assert_eq!(kernels::relax_gate(15.0), 1.0);
    assert_eq!(kernels::relax_gate(64.0), 1.0);
}

/// The baseline is measured once per mesh and kept.
#[test]
fn clay_keeps_one_curvature_baseline_per_mesh() {
    let mut mesh = sphere(20_000);
    let view = view_of(&mesh);
    let mut sculptor = sculptor_with(Brush::Clay, 0.1);
    let c = SIZE * 0.5;
    for _ in 0..3 {
        run_stroke(&mut mesh, &mut sculptor, &view, &line(c - Vec2::new(40.0, 0.0), c + Vec2::new(40.0, 0.0), 30));
    }
    assert_eq!(sculptor.curvature.len(), 1);
    let (uid, base) = sculptor.curvature[0];
    assert_eq!(uid, mesh.uid);
    assert!((base.mean + 0.5).abs() < 0.02, "measured before the first stroke moved anything: {}", base.mean);

    let mut other = sphere(5_000);
    let view = view_of(&other);
    run_stroke(&mut other, &mut sculptor, &view, &line(c - Vec2::new(40.0, 0.0), c + Vec2::new(40.0, 0.0), 30));
    assert_eq!(sculptor.curvature.len(), 2);
}

// ------------------------------------------------------------ hidden faces

/// How far a point of the ball is from the middle of its front, measured on
/// directions alone, so that pushing the point in or out does not change it.
fn off_front(p: Vec3) -> f32 {
    p.normalize().distance(Vec3::Z)
}

/// A ball with a patch on its front hidden. With `bulge`, the middle of the
/// patch is pushed well out first, far enough inside the patch that every
/// vertex which still shows, and every normal of one, is as it would be
/// without the bulge.
fn ball_with_a_hidden_patch(bulge: bool) -> Mesh {
    let mut mesh = sphere(80_000);
    if bulge {
        let leaves: Vec<u32> = (0..mesh.bvh.leaves.len() as u32).collect();
        mesh.begin_stroke();
        mesh.move_vertices(&leaves, |_, vt| {
            let rise = 0.08 * kernels::smooth((0.20 - off_front(vt.pos)) / 0.04);
            (rise > 0.0).then(|| vt.pos + vt.pos.normalize() * rise)
        });
        mesh.positions_changed(&leaves);
        mesh.end_stroke(false);
        let _ = mesh.take_dirty();
    }
    let patch: Vec<u32> = (0..mesh.triangle_count() as u32)
        .filter(|&t| (0..3).any(|c| off_front(mesh.positions[mesh.indices[t as usize * 3 + c] as usize]) < 0.28))
        .collect();
    assert!(mesh.hide(&patch).is_some());
    mesh
}

/// A point on the plain ball, just outside the hidden patch.
fn beside_the_patch() -> Vec3 {
    let angle = 2.0 * (0.36f32 / 2.0).asin();
    Vec3::new(angle.sin(), 0.0, angle.cos()) * 0.5
}

/// Hidden parts are out of a brush's reach for reading as well as writing:
/// the plane a Flatten dab levels to is fitted to the surface that shows,
/// whatever shape the hidden part beside it has.
#[test]
fn a_flatten_dab_beside_a_hidden_bulge_levels_to_the_visible_surface_only() {
    let radius = 0.15;
    let dab = |bulge: bool| {
        let mut mesh = ball_with_a_hidden_patch(bulge);
        let before = mesh.positions.clone();
        let view = view_of(&mesh);
        let mut sculptor = sculptor_with(Brush::Flatten, radius);
        press(&mut mesh, &mut sculptor, &view, beside_the_patch());
        sculptor.end(&mut mesh, &view);
        (mesh, before)
    };
    let ((plain, plain_before), (bulged, bulged_before)) = (dab(false), dab(true));
    let hidden = bulged.hidden_vertices().expect("part of the ball is hidden");

    // The bulge is under the ring, or the test proves nothing.
    let in_reach = (0..bulged.vertex_count())
        .filter(|&v| hidden[v] && plain_before[v].distance(beside_the_patch()) < radius)
        .filter(|&v| bulged_before[v].distance(plain_before[v]) > 0.02)
        .filter(|&v| bulged_before[v].distance(beside_the_patch()) < radius)
        .count();
    assert!(in_reach > 20, "only {in_reach} bulged vertices are inside the ring");

    let (mut moved, mut worst) = (0, 0.0f32);
    for v in 0..bulged.vertex_count() {
        if hidden[v] {
            assert_eq!(bulged.positions[v], bulged_before[v], "a hidden vertex moved");
            continue;
        }
        assert_eq!(bulged_before[v], plain_before[v], "the bulge reached a vertex that shows");
        moved += usize::from(bulged.positions[v] != bulged_before[v]);
        worst = worst.max(bulged.positions[v].distance(plain.positions[v]));
    }
    assert!(moved > 200, "the dab moved only {moved} vertices");
    // Not to the last bit: the bulge makes the dab look through a few more
    // leaves of the spatial index, so the plane's sums are grouped
    // differently. Read into the fit, the bulge would shift it by thousands
    // of times this.
    assert!(worst < 1e-6, "the hidden bulge moved the visible surface by {worst}");
}

/// Move does not take hold of hidden surface, nor reach across it.
#[test]
fn move_leaves_hidden_surface_out_of_its_grab() {
    let mut mesh = ball_with_a_hidden_patch(false);
    let view = view_of(&mesh);
    let mut sculptor = sculptor_with(Brush::Move, 0.15);
    press(&mut mesh, &mut sculptor, &view, beside_the_patch());
    let grab = sculptor.stroke.as_ref().unwrap().grab.as_ref().unwrap();
    assert!(grab.verts.len() > 200);
    assert!(grab.verts.iter().all(|&v| !mesh.is_vertex_hidden(v)), "a hidden vertex was grabbed");
    // The ring does reach over the patch: shown, those vertices are grabbed.
    let grabbed = grab.verts.len();
    let mut shown = sphere(80_000);
    let mut sculptor = sculptor_with(Brush::Move, 0.15);
    press(&mut shown, &mut sculptor, &view, beside_the_patch());
    assert!(sculptor.stroke.as_ref().unwrap().grab.as_ref().unwrap().verts.len() > grabbed + 50);
}

// ------------------------------------------------------------- brush size

#[test]
fn a_fixed_size_brush_is_the_same_size_on_the_model_at_any_zoom() {
    let mut mesh = sphere(20_000);
    let radius_at = |sculptor: &mut Sculptor, mesh: &mut Mesh, distance: f32| {
        let mut camera = Camera::default();
        camera.distance = distance;
        let view = camera.view(Vec2::ZERO, SIZE, distance, 1.0);
        assert!(sculptor.begin(mesh, &view, PointerSample { pos: SIZE * 0.5, pressure: None, time: 0.0 }, false, false));
        let radius = sculptor.stroke_cursor().unwrap().1;
        sculptor.end(mesh, &view);
        radius
    };

    // Sized on screen, the brush covers less of the model from close up.
    let mut screen = Sculptor::default();
    screen.settings.brush = Brush::Move;
    let (near, far) = (radius_at(&mut screen, &mut mesh, 1.5), radius_at(&mut screen, &mut mesh, 6.0));
    assert!(far > near * 3.0, "{near} / {far}");

    // Sized on the model, it does not change.
    let mut fixed = sculptor_with(Brush::Move, 0.07);
    assert_eq!(radius_at(&mut fixed, &mut mesh, 1.5), 0.07);
    assert_eq!(radius_at(&mut fixed, &mut mesh, 6.0), 0.07);
}

#[test]
fn switching_screen_space_size_never_makes_the_ring_jump() {
    let mesh = sphere(20_000);
    let mut sculptor = Sculptor::default();
    let world_per_pixel = 0.002;

    // On: the fixed size follows the ring.
    sculptor.settings.radius_px = 60.0;
    sculptor.sync_size(&mesh, world_per_pixel);
    assert!((sculptor.settings.radius_world - 0.12).abs() < 1e-6);
    assert_eq!(sculptor.settings.size(), 60.0);

    // Off: the size is what the ring was showing, and the ring follows it
    // as the camera moves.
    sculptor.settings.screen_space_size = false;
    sculptor.sync_size(&mesh, world_per_pixel);
    assert!((sculptor.settings.size() - 0.12).abs() < 1e-6);
    assert!((sculptor.settings.radius_px - 60.0).abs() < 1e-3);
    sculptor.sync_size(&mesh, world_per_pixel * 0.5);
    assert!((sculptor.settings.radius_px - 120.0).abs() < 1e-3, "twice as close, twice the ring");

    // Back on: the ring stays where it was.
    sculptor.settings.screen_space_size = true;
    sculptor.sync_size(&mesh, world_per_pixel * 0.5);
    assert!((sculptor.settings.size() - 120.0).abs() < 1e-3);
}

#[test]
fn the_fixed_size_range_scales_with_the_model() {
    let ball = sphere(20_000);
    let big = Mesh::new(ball.positions.iter().map(|&p| p * 100.0).collect(), ball.indices.clone(), None);
    let mut sculptor = Sculptor::default();
    sculptor.settings.screen_space_size = false;

    sculptor.sync_size(&ball, 0.002);
    let (min, max) = sculptor.settings.size_range(900.0);
    assert!((min - 0.01).abs() < 1e-5 && (max - 8.0).abs() < 1e-3, "{min} .. {max}");
    sculptor.settings.set_size(1000.0, 900.0);
    assert!((sculptor.settings.radius_world - 8.0).abs() < 1e-3);
    assert!((sculptor.settings.size_per_drag_px() - 0.01).abs() < 1e-5);

    // The same brush on a model a hundred times the size: the size is pulled
    // into that model's range, which is a hundred times as wide.
    sculptor.settings.radius_world = 0.05;
    sculptor.sync_size(&big, 0.2);
    let (min, max) = sculptor.settings.size_range(900.0);
    assert!((min - 1.0).abs() < 1e-3 && (max - 800.0).abs() < 0.1, "{min} .. {max}");
    assert!((sculptor.settings.radius_world - 1.0).abs() < 1e-3);
    assert!((sculptor.settings.size_per_drag_px() - 1.0).abs() < 1e-3);

    // Sized on screen, the range is in pixels and follows the window.
    sculptor.settings.screen_space_size = true;
    assert_eq!(sculptor.settings.size_range(1080.0), (2.0, 600.0));
    sculptor.settings.set_size(5000.0, 1080.0);
    assert_eq!(sculptor.settings.radius_px, 600.0);
}
