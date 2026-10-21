//! Tests of the pen-feel changes of 2026-10-21: pressure that reaches zero
//! and follows the pen closely, a Strength slider that spans its range,
//! Smooth's capped passes, Layer easing up to its height, Front Facing on
//! by default and Move following the pen all the way.

use glam::{Vec2, Vec3};

use super::input::{LazyRope, PenPressure};
use super::stroke_tests::{SIZE, across, draw, height, sculptor_with, sphere, twins, view_of};
use super::*;
use crate::mesh::primitives::{self, Primitive};

// ----------------------------------------------------------------- pressure

/// The filter closes most of a jump in the pen's pressure within about 17
/// milliseconds, and nearly all of it within 50 (where it used to have
/// closed two thirds).
#[test]
fn smoothed_pressure_follows_the_pen_within_a_few_milliseconds() {
    let mut pen = PenPressure::default();
    pen.touch(0.0, 0.0);
    let mut time = 0.0;
    let mut after = |ms: f64, pen: &mut PenPressure| {
        while time < ms / 1000.0 - 1e-9 {
            time += 1.0 / 1000.0;
            pen.step(Some(1.0), time);
        }
        pen.smoothed
    };
    let at_17 = after(17.5, &mut pen);
    assert!((0.55..0.75).contains(&at_17), "17 ms after the jump the filter has {at_17}");
    let at_50 = after(50.0, &mut pen);
    assert!(at_50 > 0.9, "50 ms after it has {at_50}");
}

/// Draws a short stroke at a steady pressure `raw` and gives back the
/// pressure the stroke opened with, as the brushes read it.
fn opened_at(sculptor: &mut Sculptor, mesh: &mut Mesh, raw: f32, time: f64) -> f32 {
    let view = view_of(mesh);
    let path = across(&view, -0.2, 0.2, 8);
    let sample = |i: usize| PointerSample { pos: path[i], pressure: Some(raw), time: time + i as f64 / 240.0 };
    assert!(sculptor.begin(mesh, &view, sample(0), false, false));
    let opened = sculptor.stroke.as_ref().unwrap().pressure;
    for i in 1..path.len() {
        sculptor.sample(mesh, &view, sample(i));
    }
    sculptor.end(mesh, &view);
    opened
}

/// A pen's stroke opens at the pen's own first reading, whatever pressure
/// the stroke before it ended on: a light touch after a firm stroke opens
/// light (no blob), and a firm one after a light stroke opens firm (no gap).
#[test]
fn a_pen_stroke_opens_at_its_own_first_pressure() {
    let mut mesh = sphere(20_000);
    let mut sculptor = sculptor_with(Brush::Standard, 0.1, 0.5);
    sculptor.settings.pressure_curve = 1.0;
    let firm = opened_at(&mut sculptor, &mut mesh, 1.0, 1.0);
    assert_eq!(firm, 1.0);
    let light = opened_at(&mut sculptor, &mut mesh, 0.1, 2.0);
    assert!((light - 0.1).abs() < 1e-6, "a light touch after a firm stroke opened at {light}");
    let firm_again = opened_at(&mut sculptor, &mut mesh, 0.9, 3.0);
    assert!((firm_again - 0.9).abs() < 1e-6, "a firm touch after a light stroke opened at {firm_again}");
}

/// With Lazy Mouse on, pressure trails the pen by the share the rope takes
/// up in the same time, and still comes down to the pen's own as the pen
/// lifts, so the stroke tapers off. With it off, it is the pen's (through
/// the filter) at once.
#[test]
fn lazy_mouse_eases_pressure_as_it_eases_the_brush() {
    let run = |lazy: bool| {
        let mut mesh = sphere(20_000);
        let view = view_of(&mesh);
        let mut sculptor = sculptor_with(Brush::Standard, 0.1, 0.5);
        let s = &mut sculptor.settings;
        (s.pressure_curve, s.lazy_mouse, s.lazy_strength, s.lazy_radius) = (1.0, lazy, 0.3, 5.0);
        let path = across(&view, -0.3, 0.3, 80);
        let sample = |i: usize, raw: f32| PointerSample { pos: path[i], pressure: Some(raw), time: i as f64 / 240.0 };
        assert!(sculptor.begin(&mut mesh, &view, sample(0, 1.0), false, false));
        let mut seen = Vec::new();
        for i in 1..=80 {
            // Firm for the first half, then the pen lifts off over a few samples.
            let raw = if i <= 40 { 1.0 } else { (1.0 - (i - 40) as f32 / 8.0).max(0.0) };
            sculptor.sample(&mut mesh, &view, sample(i, raw));
            seen.push(sculptor.stroke.as_ref().unwrap().pressure);
        }
        sculptor.end(&mut mesh, &view);
        seen
    };
    let (plain, lazy) = (run(false), run(true));
    // Two samples into the lift the filter alone has come most of the way
    // down, the lazy stroke less of it.
    let (p, l) = (plain[41], lazy[41]);
    assert!(l > p + 0.02, "two samples into the lift: plain {p}, lazy {l}");
    // Both come down all the way through the lift: the stroke tapers.
    assert!(lazy[41..].windows(2).all(|w| w[1] <= w[0]), "the lazy pressure rose during the lift: {lazy:?}");
    assert!(plain[79] < 0.01 && lazy[79] < 0.1, "at the end: plain {}, lazy {}", plain[79], lazy[79]);
    // The rope's share over a step is what the pressure takes up.
    let dt = 1.0 / 240.0;
    assert!(LazyRope::follow(0.3, dt) > 0.0 && LazyRope::follow(0.3, dt) < 1.0);
    assert_eq!(LazyRope::follow(1.0, dt), 1.0, "full Lazy Strength leaves the pressure alone");
}

/// At the brushes' own Pressure -> Strength the lightest touch of the pen
/// does next to nothing, on every brush that lays dabs (Clay Strips and
/// Dam Standard keep theirs, and with them a trace at the lightest touch).
#[test]
fn the_lightest_touch_does_next_to_nothing() {
    for brush in [Brush::Standard, Brush::Inflate, Brush::Layer, Brush::Clay, Brush::Crease, Brush::Pinch] {
        let stroke = |pen: f32| {
            let mut mesh = sphere(40_000);
            let before = mesh.positions.clone();
            let view = view_of(&mesh);
            let mut sculptor = Sculptor::default();
            let s = &mut sculptor.settings;
            (s.brush, s.screen_space_size, s.radius_world) = (brush, false, 0.1);
            // Clay's finishing pass evens out what the frame's dabs touched
            // whatever their strength: not what is measured here.
            s.clay_relax = 0.0;
            let path = across(&view, -0.3, 0.3, 60);
            draw(&mut mesh, &mut sculptor, &view, &path, 4, Some(&|_| pen));
            mesh.positions.iter().zip(&before).map(|(a, b)| a.distance(*b)).fold(0.0f32, f32::max)
        };
        let (full, light) = (stroke(1.0), stroke(0.02));
        assert!(full > 0.002, "{brush:?} at full pressure did nothing");
        assert!(light < full * 0.02, "{brush:?}: the lightest touch moved {light}, full pressure {full}");
    }
}

// ----------------------------------------------------------------- strength

/// The tallest one pass along the front of the ball builds, in radii.
fn one_pass(brush: Brush, strength: f32) -> f32 {
    let mut mesh = sphere(80_000);
    let view = view_of(&mesh);
    let mut sculptor = Sculptor::default();
    let s = &mut sculptor.settings;
    (s.brush, s.screen_space_size, s.radius_world) = (brush, false, 0.1);
    s.per_brush[brush.index()].strength = strength;
    let path = across(&view, -0.3, 0.3, 80);
    draw(&mut mesh, &mut sculptor, &view, &path, 4, Some(&|_| 1.0));
    mesh.positions.iter().map(|&p| height(p)).fold(0.0f32, f32::max) / 0.1
}

/// The Strength slider spans its range on the brushes that build: one pass
/// at full pressure builds about a hundredth of the radius at 10%, a tenth
/// at 50% and a third at 100% (as `--brush-feel` measures on a finer ball).
#[test]
fn strength_spans_the_slider_for_the_brushes_that_build() {
    for brush in [Brush::Standard, Brush::Inflate, Brush::Layer] {
        let [low, mid, high] = [0.1, 0.5, 1.0].map(|s| one_pass(brush, s));
        assert!((0.003..0.03).contains(&low), "{brush:?} at 10% built {low}");
        assert!((0.06..0.16).contains(&mid), "{brush:?} at 50% built {mid}");
        assert!((0.14..0.4).contains(&high), "{brush:?} at 100% built {high}");
    }
}

/// The shipped brushes on the mapped kernels do what they did: their
/// Strength is the one that works as the old number did.
#[test]
fn shipped_brushes_keep_what_their_strength_did() {
    let library = library::Library::default();
    let s = Settings::default();
    let strength = |name: &str| {
        let i = library.named(name).unwrap();
        let e = &library.entries()[i];
        strength::effective(e.kernel, library.record(i, &s).settings.strength)
    };
    for (name, old) in [
        ("Trim", 0.6),
        ("Fill", 0.6),
        ("Planar", 0.8),
        ("Polish", 0.25),
        ("Smooth Peaks", 0.3),
        ("Smooth Valleys", 0.3),
        ("Relax", 0.5),
        ("Keep Volume", 0.3),
        ("Rake", 0.06),
        ("Surface Noise", 0.045),
        ("Spread", 0.3),
    ] {
        assert!((strength(name) - old).abs() < 0.003, "{name} works at {} where it worked at {old}", strength(name));
    }
    // A dragged print's Strength is its height and was not mapped.
    let scales = library.record(library.named("Scales").unwrap(), &s);
    assert_eq!(scales.settings.strength, 0.5);
}

/// Clay Strips and Dam Standard are exactly as they were: the same numbers
/// as before the base brushes they are made of changed.
#[test]
fn clay_strips_and_dam_standard_keep_their_settings() {
    let library = library::Library::default();
    let s = Settings::default();
    let strips = library.record(library.named("Clay Strips").unwrap(), &s).settings;
    assert_eq!(
        (strips.strength, strips.accumulate, strips.pressure_strength, strips.front_facing, strips.spacing, strips.focal_shift),
        (0.5, false, 0.88, false, 0.3, -0.5)
    );
    let dam = library.record(library.named("Dam Standard").unwrap(), &s).settings;
    assert_eq!(
        (dam.strength, dam.accumulate, dam.pressure_strength, dam.pressure_size, dam.front_facing, dam.positive),
        (0.7, false, 0.6, 0.85, false, false)
    );
}

// ------------------------------------------------------------------- smooth

/// A Smooth dab makes four passes at the most, the last a fraction of one.
#[test]
fn a_smooth_dab_makes_four_passes_at_the_most() {
    assert_eq!(kernels::smooth_passes(1.0), 4.0);
    assert_eq!(kernels::smooth_passes(3.0), 4.0, "a dab standing in for several still makes four");
    assert_eq!(kernels::smooth_passes(0.3), 1.2);
    assert_eq!(kernels::smooth_passes(0.0), 0.0);
    // The old 30% was three passes of 40%: 1.2 passes' worth, which is what
    // the slider's new value for it makes.
    let at = strength::effective(Brush::Smooth, strength::from_old(Brush::Smooth, 0.3));
    assert!((kernels::smooth_passes(at) - 1.2).abs() < 1e-4);
}

// -------------------------------------------------------------------- layer

/// What a Layer stroke along the front of the ball raises, in radii: the
/// most and the mirror error, after `passes` passes over the same line in
/// one stroke.
fn layer_stroke(strength: f32, passes: usize, mirror: bool, mask: f32) -> (f32, f32) {
    let mut mesh = sphere(80_000);
    mesh.mask.iter_mut().for_each(|m| *m = mask);
    let twins = twins(&mesh);
    let view = view_of(&mesh);
    let mut sculptor = Sculptor::default();
    let s = &mut sculptor.settings;
    (s.brush, s.screen_space_size, s.radius_world, s.symmetry) = (Brush::Layer, false, 0.1, [mirror, false, false]);
    s.per_brush[Brush::Layer.index()].strength = strength;
    let there = across(&view, -0.05, 0.4, 60);
    let mut path = there.clone();
    for pass in 1..passes {
        if pass % 2 == 1 {
            path.extend(there.iter().rev().skip(1));
        } else {
            path.extend(there.iter().skip(1));
        }
    }
    draw(&mut mesh, &mut sculptor, &view, &path, 4, Some(&|_| 1.0));
    let most = mesh.positions.iter().map(|&p| height(p)).fold(0.0f32, f32::max) / 0.1;
    let flip = Vec3::new(-1.0, 1.0, 1.0);
    let error = twins
        .iter()
        .enumerate()
        .map(|(v, &t)| mesh.positions[t as usize].distance(mesh.positions[v] * flip))
        .fold(0.0f32, f32::max);
    (most, error / 0.1)
}

/// Layer eases up to its height: a light pass raises a low step, more
/// passes in the same stroke raise it to the height and never past it, and
/// a vertex masked half way rises half way at the most.
#[test]
fn layer_eases_up_to_its_height_and_no_further() {
    let full = Settings::default().layer_height;
    let (light, _) = layer_stroke(0.2, 1, false, 0.0);
    let (firm, _) = layer_stroke(0.6, 1, false, 0.0);
    let (again, _) = layer_stroke(0.6, 5, false, 0.0);
    assert!(light < firm && firm < again, "one pass at 20% {light}, at 60% {firm}; five passes {again}");
    assert!(light < full * 0.4, "a light pass rose {light} of a height of {full}");
    assert!(again > full * 0.95 && again <= full * 1.001, "five passes rose {again}, the height is {full}");
    let (masked, _) = layer_stroke(1.0, 5, false, 0.5);
    assert!(masked <= full * 0.5 * 1.001 && masked > full * 0.4, "masked half way it rose {masked}");
    // Across the mirror plane it stays the mirror image of itself.
    let (mirrored, error) = layer_stroke(0.6, 3, true, 0.0);
    assert!(mirrored > 0.05 && error < mirrored * 1e-3, "mirrored it rose {mirrored} with an error of {error}");
}

// ------------------------------------------------------------- front facing

/// A slab thinner than the brush, seen face on.
fn slab() -> Mesh {
    let cube = primitives::build(Primitive::Cube, 80_000, 0.5);
    let thin = cube.positions.iter().map(|&p| Vec3::new(p.x, p.y, p.z * 0.06)).collect();
    Mesh::new(thin, cube.indices.clone(), None)
}

/// With the brushes as they start, a stroke on the front of a thin slab
/// leaves its back alone; with Front Facing off the back is dragged along.
#[test]
fn a_stroke_on_the_front_of_a_thin_form_leaves_its_back_alone() {
    for brush in FRONT_FACING {
        let back = |front_facing: bool| {
            let mut mesh = slab();
            let normals = mesh.normals.clone();
            let before = mesh.positions.clone();
            let view = view_of(&mesh);
            let mut sculptor = Sculptor::default();
            let s = &mut sculptor.settings;
            (s.brush, s.screen_space_size, s.radius_world) = (brush, false, 0.1);
            assert!(s.per_brush[brush.index()].front_facing, "{brush:?} starts with Front Facing on");
            s.per_brush[brush.index()].front_facing = front_facing;
            // Something for the smoothing brushes to do on the flat front.
            if matches!(brush, Brush::Smooth | Brush::Flatten) {
                s.per_brush[brush.index()].strength = 1.0;
            }
            let at = |x: f32| view.project(Vec3::new(x, 0.0, 0.03)).unwrap();
            let path: Vec<Vec2> = (0..=60).map(|i| at(-0.15 + 0.3 * i as f32 / 60.0)).collect();
            draw(&mut mesh, &mut sculptor, &view, &path, 4, Some(&|_| 1.0));
            let moved = |v: usize| mesh.positions[v].distance(before[v]);
            let back = (0..before.len()).filter(|&v| normals[v].z < -0.9).map(moved).fold(0.0f32, f32::max);
            let front = (0..before.len()).filter(|&v| normals[v].z > 0.9).map(moved).fold(0.0f32, f32::max);
            (back, front)
        };
        let ((kept, front), (dragged, _)) = (back(true), back(false));
        if brush == Brush::Nudge || brush == Brush::Smooth || brush == Brush::Flatten {
            // These move little on a flat front; that the back is left is what counts.
            assert_eq!(kept, 0.0, "{brush:?} moved the back by {kept}");
            continue;
        }
        assert!(front > 1e-3, "{brush:?} did nothing to the front");
        assert_eq!(kept, 0.0, "{brush:?} moved the back by {kept} with Front Facing on");
        assert!(dragged > 1e-4, "{brush:?} with it off: the back moved {dragged}; the test reaches nothing");
    }
}

// --------------------------------------------------------------------- move

/// Move starts at full strength: the point taken hold of stays under the
/// pen, and a mirrored grab on the mirror plane carries it exactly as far
/// as a grab with no mirror off the plane does.
#[test]
fn move_starts_at_full_strength_on_and_off_the_mirror_plane() {
    assert_eq!(Settings::default().per_brush[Brush::Move.index()].strength, 1.0);
    let drag = |at: Vec3, mirror: bool| {
        let mut mesh = sphere(80_000);
        let view = view_of(&mesh);
        let grabbed = (0..mesh.vertex_count())
            .min_by(|&a, &b| mesh.positions[a].distance(at).total_cmp(&mesh.positions[b].distance(at)))
            .unwrap();
        let mut sculptor = Sculptor::default();
        let s = &mut sculptor.settings;
        (s.brush, s.screen_space_size, s.radius_world, s.symmetry) = (Brush::Move, false, 0.15, [mirror, false, false]);
        let from = view.project(mesh.positions[grabbed]).unwrap();
        // Along the mirror plane: up the screen.
        let to = from + Vec2::new(0.0, -60.0);
        let path: Vec<Vec2> = (0..=16).map(|i| from.lerp(to, i as f32 / 16.0)).collect();
        let before = mesh.positions[grabbed];
        let sample = |i: usize| PointerSample { pos: path[i], pressure: Some(1.0), time: i as f64 / 240.0 };
        assert!(sculptor.begin(&mut mesh, &view, sample(0), false, false));
        for i in 1..path.len() {
            sculptor.sample(&mut mesh, &view, sample(i));
            sculptor.frame(&mut mesh, &view, 1.0 / 60.0);
        }
        sculptor.end(&mut mesh, &view);
        let went = mesh.positions[grabbed] - before;
        (went.length(), view.project(mesh.positions[grabbed]).unwrap().distance(to))
    };
    let (on_plane, off_pen) = drag(Vec3::new(0.0, 0.0, 0.5), true);
    let (off_plane, _) = drag(Vec3::new(0.2, 0.0, (0.25f32 - 0.04).sqrt()), false);
    assert!(off_pen < 1.0, "the grabbed point ended {off_pen} px from the pen");
    assert!((on_plane / off_plane - 1.0).abs() < 0.03, "on the plane mirrored {on_plane}, off it alone {off_plane}");
    let _ = SIZE;
}
