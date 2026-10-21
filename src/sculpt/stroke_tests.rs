//! Stroke shaping tests: what the pen's pressure does to each brush's
//! strength and size, Spacing and the correction that keeps a stroke
//! building at one rate whatever it is set to, Scatter, Size Jitter and
//! Random Rotation and their being the same every time, and a stroke that
//! falls behind the pen being finished without gaps (`pace.rs`).

use std::time::Duration;

use glam::{Vec2, Vec3};

use super::input::{self, PenMeter};
use super::jitter::{self, Jitter};
use super::*;
use crate::camera::{Camera, View};
use crate::mesh::primitives::{self, Primitive};

pub(super) const SIZE: Vec2 = Vec2::new(1600.0, 900.0);

pub(super) fn sphere(tris: usize) -> Mesh {
    primitives::build(Primitive::Sphere, tris, 0.5)
}

/// The ball from in front: the middle of the screen is the point (0, 0, 0.5).
pub(super) fn view_of(mesh: &Mesh) -> View {
    let mut camera = Camera::default();
    camera.frame(mesh.bounds(), SIZE.x / SIZE.y, false);
    camera.view(Vec2::ZERO, SIZE, camera.distance, 1.0)
}

/// The point of the ball `angle` radians round from the front, toward +X.
pub(super) fn on_ball(angle: f32) -> Vec3 {
    Vec3::new(angle.sin(), 0.0, angle.cos()) * 0.5
}

/// A path across the front of the ball, from `from` radians round to `to`,
/// as `n` + 1 pointer positions.
pub(super) fn across(view: &View, from: f32, to: f32, n: usize) -> Vec<Vec2> {
    (0..=n).map(|i| view.project(on_ball(from + (to - from) * i as f32 / n as f32)).unwrap()).collect()
}

/// A sculptor holding `brush` at a fixed size on the model.
pub(super) fn sculptor_with(brush: Brush, radius: f32, strength: f32) -> Sculptor {
    let mut sculptor = Sculptor::default();
    let s = &mut sculptor.settings;
    s.brush = brush;
    s.screen_space_size = false;
    s.radius_world = radius;
    s.per_brush[brush.index()].strength = strength;
    // Clay's finishing pass is paced by the frame, not the dab.
    s.clay_relax = 0.0;
    sculptor
}

/// Draws a stroke through `points`, `per_frame` samples to a frame. Each
/// sample carries the pressure `pen` gives for how far along the stroke it
/// is (0 to 1), or none for a mouse. Returns how many dabs it laid.
pub(super) fn draw(
    mesh: &mut Mesh,
    sculptor: &mut Sculptor,
    view: &View,
    points: &[Vec2],
    per_frame: usize,
    pen: Option<&dyn Fn(f32) -> f32>,
) -> u64 {
    let last = (points.len() - 1).max(1) as f32;
    let sample = |i: usize| PointerSample {
        pos: points[i],
        pressure: pen.map(|pen| pen(i as f32 / last)),
        time: 50.0 + i as f64 / 240.0,
    };
    let before = sculptor.dabs_applied;
    assert!(sculptor.begin(mesh, view, sample(0), false, false), "the stroke missed the mesh");
    for i in 1..points.len() {
        sculptor.sample(mesh, view, sample(i));
        if i % per_frame == 0 {
            sculptor.frame(mesh, view, 1.0 / 60.0);
            let _ = mesh.take_dirty();
        }
    }
    sculptor.end(mesh, view);
    let _ = mesh.take_dirty();
    sculptor.dabs_applied - before
}

/// Every bit of a mesh's positions and mask, folded into one number.
pub(super) fn fingerprint(mesh: &Mesh) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut fold = |bits: u32| {
        for byte in bits.to_le_bytes() {
            hash = (hash ^ byte as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for p in &mesh.positions {
        p.to_array().into_iter().for_each(|c| fold(c.to_bits()));
    }
    mesh.mask.iter().for_each(|m| fold(m.to_bits()));
    hash
}

/// How far a point of the sculpted ball stands above the ball it was.
pub(super) fn height(p: Vec3) -> f32 {
    p.length() - 0.5
}

/// How far round the ball from its front a point is, toward +X, in radians.
pub(super) fn round(p: Vec3) -> f32 {
    p.x.atan2(p.z)
}

/// The height of the stroke's ridge along a path made with [`across`]: the
/// tallest vertex (the deepest, for a brush that digs) within a hair of the
/// path in each of `bins` equal stretches from `from` to `to` radians.
fn ridge(mesh: &Mesh, from: f32, to: f32, bins: usize) -> Vec<f32> {
    let mut tallest = vec![0.0f32; bins];
    for &p in &mesh.positions {
        let at = (round(p) - from) / (to - from);
        if p.z > 0.0 && p.y.abs() < 0.008 && (0.0..1.0).contains(&at) {
            let bin = &mut tallest[(at * bins as f32) as usize];
            if height(p).abs() > bin.abs() {
                *bin = height(p);
            }
        }
    }
    tallest
}

/// How much a stroke along such a path built between `from` and `to`
/// radians round: the heights of every vertex there, added up.
fn built(mesh: &Mesh, from: f32, to: f32) -> f64 {
    let there = |p: &&Vec3| p.z > 0.0 && (from..to).contains(&round(**p));
    mesh.positions.iter().filter(there).map(|&p| height(p).abs() as f64).sum()
}

/// How far apart, as a share of the larger, the largest and smallest of
/// `values` are.
pub(super) fn spread(values: &[f64]) -> f64 {
    let (low, high) = values.iter().fold((f64::MAX, f64::MIN), |(low, high), &v| (low.min(v), high.max(v)));
    (high - low) / high.abs().max(1e-30)
}

// ----------------------------------------------------------------- pressure

/// Pressure -> Strength at its old default (Clay Strips keeps it) is the old
/// Light-Touch Floor of 0.12 to rounding, and both maps give a mouse (and
/// full pressure) exactly 1.
#[test]
fn the_pressure_maps_keep_todays_feel_and_leave_a_mouse_alone() {
    for i in 0..=100 {
        let pen = i as f32 / 100.0;
        let old = 0.12 + (1.0 - 0.12) * pen;
        assert!((input::pen_strength(OLD_PRESSURE_STRENGTH, pen) - old).abs() < 1e-6, "at {pen}");
        assert!((input::pen_strength(DEFAULT_PRESSURE_STRENGTH, pen) - pen).abs() < 1e-6, "at {pen}");
        // Off, neither changes anything, to the bit.
        assert_eq!(input::pen_strength(0.0, pen), 1.0);
        assert_eq!(input::pen_size(0.0, pen), 1.0);
        // On, more pressure is never weaker or smaller.
        for amount in [0.3, 1.0] {
            assert!(input::pen_strength(amount, pen) <= input::pen_strength(amount, (pen + 0.01).min(1.0)));
            assert!(input::pen_size(amount, pen) <= input::pen_size(amount, (pen + 0.01).min(1.0)));
            assert!(input::pen_size(amount, pen) >= input::MIN_PEN_SIZE, "a dab never shrinks to nothing");
        }
    }
    for amount in [0.0, 0.37, DEFAULT_PRESSURE_STRENGTH, 1.0] {
        assert_eq!(input::pen_strength(amount, 1.0), 1.0, "full pressure is the brush's own strength");
        assert_eq!(input::pen_size(amount, 1.0), 1.0, "and the ring's own size");
    }
    assert_eq!(input::pen_strength(1.0, 0.0), 0.0, "at 100% a feather touch does nothing");
    assert_eq!(input::pen_size(1.0, 0.0), input::MIN_PEN_SIZE);
    assert_eq!(input::pen_size(0.5, 0.0), 0.5, "at 50% the lightest touch is half the ring");
    // Every brush starts with pressure taking all of its strength away at
    // the lightest touch, and changing its size not at all.
    for b in Settings::default().per_brush {
        assert_eq!((b.pressure_strength, b.pressure_size), (DEFAULT_PRESSURE_STRENGTH, 0.0));
    }
}

/// A stroke with a brush whose dose is 1 and whose dabs are the ring's size
/// is handed the stroke's own settings untouched: what every stroke was
/// before any of this, to the bit.
#[test]
fn a_dab_at_the_brushs_own_spacing_and_the_rings_size_is_the_stroke_itself() {
    for brush in Brush::ALL {
        let spacing = brush.default_spacing();
        assert_eq!(Settings::default().per_brush[brush.index()].spacing, spacing);
        assert_eq!(Stroke::dose(spacing, spacing), 1.0, "{brush:?}");
    }
    let mut mesh = sphere(20_000);
    let view = view_of(&mesh);
    let mut sculptor = sculptor_with(Brush::Standard, 0.1, 0.5);
    assert!(sculptor.begin(&mut mesh, &view, PointerSample { pos: SIZE * 0.5, pressure: None, time: 0.0 }, false, false));
    let stroke = sculptor.stroke.as_ref().unwrap();
    let p = stroke.params.for_dab(stroke.radius_at(1.0), stroke.spacing(), Stroke::dose(stroke.spacing(), stroke.tuned_spacing));
    assert_eq!((p.radius, p.spacing, p.dose), (stroke.params.radius, 0.2, 1.0));
    assert_eq!(p.share(0.3), 0.3);
    // Twice the dose closes the gap twice over; half of it, half as surely.
    let double = Params { dose: 2.0, ..p };
    assert!((double.share(0.3) - (1.0 - 0.7 * 0.7)).abs() < 1e-6);
    assert!((Params { dose: 0.5, ..p }.share(0.36) - 0.2).abs() < 1e-6);
    assert_eq!(double.share(1.0), 1.0);
    sculptor.end(&mut mesh, &view);
}

/// How broad a stroke's trace is between `from` and `to` radians round the
/// ball: everything it raised there, added up, against the most it raised
/// any one vertex. For strokes of one shape that goes as their width, and
/// unlike the furthest vertex raised it does not come in steps the size of
/// the mesh's triangles.
fn trace_width(mesh: &Mesh, from: f32, to: f32) -> f32 {
    let there = |p: &&Vec3| p.z > 0.0 && (from..to).contains(&round(**p));
    let (sum, most) = mesh.positions.iter().filter(there).fold((0.0f64, 0.0f32), |(sum, most), &p| {
        (sum + height(p).max(0.0) as f64, most.max(height(p)))
    });
    if most > 0.0 { (sum / most as f64) as f32 } else { 0.0 }
}

/// How far from its path (which lies along Y = 0) a stroke reached between
/// `from` and `to` radians round the ball: the distance over the ball to
/// the furthest vertex it raised by more than a tenth of the most it raised
/// any.
fn trace_reach(mesh: &Mesh, from: f32, to: f32) -> f32 {
    let most = mesh.positions.iter().map(|&p| height(p)).fold(0.0f32, f32::max);
    let raised = |p: &&Vec3| height(**p) > most * 0.1 && (from..to).contains(&round(**p));
    mesh.positions.iter().filter(raised).map(|p| (p.y / 0.5).asin().abs() * 0.5).fold(0.0f32, f32::max)
}

/// Pressure -> Size: a light pen draws a narrower stroke than a firm one,
/// by the share the setting says; the dabs come closer together to match;
/// the ring shows the size the brush is working at; and a mouse is not
/// touched by any of it.
#[test]
fn pressure_makes_the_brush_smaller_and_spacing_follows() {
    let radius = 0.1;
    let stroke = |pressure_size: f32, pen: Option<f32>| {
        let mut mesh = sphere(200_000);
        let view = view_of(&mesh);
        let mut sculptor = sculptor_with(Brush::Inflate, radius, 0.05);
        // The pen read plainly, and its strength left out of it.
        sculptor.settings.pressure_curve = 1.0;
        let bs = sculptor.settings.current_mut();
        (bs.pressure_size, bs.pressure_strength) = (pressure_size, 0.0);
        // The pen has been held at this pressure: no easing in from another.
        sculptor.pen.smoothed = pen.unwrap_or(1.0);
        let hold = |_: f32| pen.unwrap_or(1.0);
        let pen = pen.map(|_| &hold as &dyn Fn(f32) -> f32);
        let path = across(&view, -0.4, 0.4, 80);
        let dabs = draw(&mut mesh, &mut sculptor, &view, &path, 4, pen);
        (trace_width(&mesh, -0.2, 0.2), dabs, fingerprint(&mesh))
    };
    let (full, full_dabs, full_print) = stroke(1.0, Some(1.0));
    let (light, light_dabs, _) = stroke(1.0, Some(0.25));
    assert!(full > 0.0, "the full-pressure stroke did nothing");
    let share = light / full;
    assert!((0.22..0.28).contains(&share), "a quarter pressure drew a stroke {share} as wide, not a quarter");
    let more = light_dabs as f32 / full_dabs as f32;
    assert!((3.4..4.6).contains(&more), "at a quarter the size the dabs came {more} times as thick, not 4");
    // Half way on the slider, half way to it.
    let (half, ..) = stroke(0.5, Some(0.0));
    assert!((0.46..0.54).contains(&(half / full)), "50% at no pressure drew {} of the full width", half / full);

    // A mouse has no pressure: full size whatever the setting, to the bit.
    let (_, mouse_dabs, mouse_print) = stroke(1.0, None);
    let (_, _, plain_print) = stroke(0.0, None);
    assert_eq!((mouse_print, mouse_dabs), (plain_print, full_dabs));
    // And a pen at full pressure is the ring's own size, as a mouse is.
    assert_eq!(full_print, plain_print);
}

/// A stroke pressed harder as it goes swells along its length, and the ring
/// is the size the brush is working at all the way.
#[test]
fn a_stroke_tapers_with_the_pens_pressure_and_the_ring_shows_it() {
    let mut mesh = sphere(80_000);
    let view = view_of(&mesh);
    let mut sculptor = sculptor_with(Brush::Inflate, 0.1, 0.05);
    sculptor.settings.pressure_curve = 1.0;
    sculptor.settings.current_mut().pressure_size = 1.0;
    sculptor.pen.smoothed = 0.0;
    let path = across(&view, -0.5, 0.5, 120);
    let pressure = |along: f32| along;
    let sample =
        |i: usize| PointerSample { pos: path[i], pressure: Some(pressure(i as f32 / 120.0)), time: 9.0 + i as f64 / 240.0 };
    assert!(sculptor.begin(&mut mesh, &view, sample(0), false, false));
    let mut rings = vec![sculptor.stroke_cursor().unwrap().1];
    for i in 1..=120 {
        sculptor.sample(&mut mesh, &view, sample(i));
        if i % 4 == 0 {
            sculptor.frame(&mut mesh, &view, 1.0 / 60.0);
            rings.push(sculptor.stroke_cursor().unwrap().1);
        }
    }
    let stroke = sculptor.stroke.as_ref().unwrap();
    assert_eq!(stroke.params.radius, 0.1, "the ring's own size is frozen for the stroke");
    assert_eq!(*rings.last().unwrap(), stroke.radius_at(stroke.pressure));
    sculptor.end(&mut mesh, &view);
    assert!(rings[0] <= 0.1 * 0.12, "the stroke opens as a point: {}", rings[0]);
    assert!(rings.windows(2).all(|w| w[0] <= w[1]), "the ring grows with the pressure");
    assert!(*rings.last().unwrap() > 0.08, "and ends near the ring's size: {:?}", rings.last());
    let (thin, middle, thick) = (trace_width(&mesh, -0.4, -0.2), trace_width(&mesh, -0.1, 0.1), trace_width(&mesh, 0.2, 0.4));
    assert!(thin < middle * 0.75 && middle < thick * 0.85, "widths along the stroke: {thin}, {middle}, {thick}");
}

/// Pressure -> Strength is each brush's own: at 0 the pen sculpts as a
/// mouse does, at 100% a light touch does next to nothing, and setting it
/// on one brush leaves the others as they were.
#[test]
fn pressure_to_strength_is_the_brushs_own() {
    let stroke = |brush: Brush, set_on: Brush, amount: f32, pen: Option<f32>| {
        let mut mesh = sphere(40_000);
        let view = view_of(&mesh);
        // Without Accumulate, and gently: a stroke that builds in proportion
        // to its strength.
        let mut sculptor = sculptor_with(brush, 0.1, 0.05);
        sculptor.settings.per_brush[brush.index()].accumulate = false;
        sculptor.settings.per_brush[set_on.index()].pressure_strength = amount;
        sculptor.pen.smoothed = pen.unwrap_or(1.0);
        let hold = |_: f32| pen.unwrap_or(1.0);
        let pen = pen.map(|_| &hold as &dyn Fn(f32) -> f32);
        let path = across(&view, -0.3, 0.3, 60);
        draw(&mut mesh, &mut sculptor, &view, &path, 4, pen);
        (built(&mesh, -0.5, 0.5), fingerprint(&mesh))
    };
    let (mouse, mouse_print) = stroke(Brush::Inflate, Brush::Inflate, DEFAULT_PRESSURE_STRENGTH, None);
    let (off, off_print) = stroke(Brush::Inflate, Brush::Inflate, 0.0, Some(0.2));
    assert_eq!(off_print, mouse_print, "at 0% a light pen is a mouse: {off} against {mouse}");
    let (usual, _) = stroke(Brush::Inflate, Brush::Inflate, OLD_PRESSURE_STRENGTH, Some(0.2));
    let (all, _) = stroke(Brush::Inflate, Brush::Inflate, 1.0, Some(0.2));
    assert!(usual < mouse * 0.6 && all < usual * 0.6, "a light touch built {usual} at 88% and {all} at 100%, of {mouse}");
    // Set on Clay, Inflate does not hear of it: it keeps its own 100%.
    let (other, _) = stroke(Brush::Inflate, Brush::Clay, 0.0, Some(0.2));
    assert_eq!(other, all);
}

/// A stroke whose first sample has no pressure and whose later ones do is a
/// pen's: its lift (no pressure again) holds the pen's value and does not
/// jump to a mouse's full strength.
#[test]
fn a_stroke_that_gains_pressure_is_a_pens_from_then_on() {
    let mut mesh = sphere(20_000);
    let view = view_of(&mesh);
    let mut sculptor = sculptor_with(Brush::Inflate, 0.1, 0.5);
    let at = |i: usize, pressure| PointerSample {
        pos: SIZE * 0.5 + Vec2::new(i as f32 * 3.0, 0.0),
        pressure,
        time: 3.0 + i as f64 / 240.0,
    };
    assert!(sculptor.begin(&mut mesh, &view, at(0, None), false, false));
    for i in 1..=40 {
        sculptor.sample(&mut mesh, &view, at(i, Some(0.2)));
    }
    let held = sculptor.stroke.as_ref().unwrap().pressure;
    assert!(held < 0.5, "the pen's light pressure is what the stroke reads: {held}");
    sculptor.sample(&mut mesh, &view, at(41, None));
    assert_eq!(sculptor.stroke.as_ref().unwrap().pressure, held, "the lift holds it");
    sculptor.end(&mut mesh, &view);
}

/// The readout's side of the pen: it has seen pressure once any arrives,
/// shows the latest while the tip is down, and a touch too light to measure
/// opens a stroke at no pressure once a pen is known to be there, and as a
/// mouse before.
#[test]
fn the_pen_meter_follows_the_pen() {
    let mut pen = PenMeter::default();
    assert_eq!(pen.down(None), None, "nothing says this is a pen: it is a mouse's press");
    assert_eq!(pen, PenMeter::default());
    pen.lifted();
    assert_eq!(pen.down(Some(0.4)), Some(0.4));
    assert_eq!((pen.seen, pen.now), (true, Some(0.4)));
    pen.moved(Some(0.7));
    assert_eq!(pen.now, Some(0.7));
    pen.moved(None);
    assert_eq!(pen.now, Some(0.7), "a sample without pressure keeps the last reading");
    pen.moved(Some(3.0));
    assert_eq!(pen.now, Some(1.0));
    pen.lifted();
    assert_eq!((pen.seen, pen.now), (true, None));
    assert_eq!(pen.down(None), Some(0.0), "a pen is known to be there: its lightest touch is no pressure, not a mouse");
    assert_eq!(pen.now, Some(0.0));
}

/// Mask painting keeps its own pace and size whichever brush is selected
/// and whatever that brush's stroke settings are.
#[test]
fn mask_painting_does_not_take_the_selected_brushs_stroke_settings() {
    let masked = |shape: &dyn Fn(&mut BrushSettings)| {
        let mut mesh = sphere(40_000);
        let view = view_of(&mesh);
        let mut sculptor = sculptor_with(Brush::Standard, 0.1, 0.3);
        sculptor.settings.mask_mode = true;
        shape(sculptor.settings.current_mut());
        sculptor.pen.smoothed = 0.4;
        let dabs = draw(&mut mesh, &mut sculptor, &view, &across(&view, -0.3, 0.3, 60), 4, Some(&|_| 0.4));
        assert!(mesh.mask.iter().any(|&m| m > 0.0));
        (fingerprint(&mesh), dabs)
    };
    let plain = masked(&|_| {});
    let shaped = masked(&|b| (b.spacing, b.scatter, b.size_jitter, b.pressure_size) = (1.5, 1.0, 0.8, 1.0));
    assert_eq!(plain, shaped);
    // Its strength under pressure is the brush's, as its Strength is.
    assert_ne!(plain.0, masked(&|b| b.pressure_strength = 0.0).0);
}

// ------------------------------------------------------------------ spacing

/// Something for Smooth and Flatten to work on: the ball with a fine,
/// regular roughness all over its front.
pub(super) fn rough_ball(tris: usize) -> Mesh {
    let ball = sphere(tris);
    let bumpy = ball.positions.iter().map(|&p| p * (1.0 + 0.012 * (p.x * 90.0).sin() * (p.y * 90.0).sin())).collect();
    Mesh::new(bumpy, ball.indices.clone(), None)
}

/// The same stroke at three spacings. Returns, for each, how much it did
/// along the middle of its length (how far it moved every vertex there,
/// added up; away from its two ends, where there is one dab more or less)
/// and how many dabs it laid.
fn at_spacings(brush: Brush, accumulate: bool, spacings: [f32; 3]) -> ([f64; 3], [u64; 3]) {
    let rough = matches!(brush, Brush::Smooth | Brush::Flatten);
    let mut out = ([0.0; 3], [0; 3]);
    for (k, spacing) in spacings.into_iter().enumerate() {
        let mut mesh = if rough { rough_ball(120_000) } else { sphere(120_000) };
        let before = mesh.positions.clone();
        let view = view_of(&mesh);
        // Gently: a stroke that has not run into the most its brush builds,
        // nor moved the surface so far that the pen's path over it changes.
        let mut sculptor = sculptor_with(brush, 0.08, if accumulate || brush == Brush::Smooth { 0.03 } else { 0.1 });
        let bs = sculptor.settings.current_mut();
        // The kernels' own build. Auto Smooth and Even Out slide vertices
        // sideways as well, by more or less with the spacing, which this
        // adds up; what they build is held to the spacing in clean_tests.rs.
        (bs.spacing, bs.accumulate, bs.auto_smooth, bs.even_out) = (spacing, accumulate, 0.0, 0.0);
        out.1[k] = draw(&mut mesh, &mut sculptor, &view, &across(&view, -0.45, 0.45, 150), 4, None);
        let in_the_middle = |p: &Vec3| p.z > 0.0 && (-0.15..0.15).contains(&round(*p));
        let moved = before.iter().zip(&mesh.positions).filter(|(was, _)| in_the_middle(was));
        out.0[k] = moved.map(|(was, now)| was.distance(*now) as f64).sum();
    }
    out
}

/// Spacing changes how smooth a stroke is, not how much it builds: at half
/// the brush's own spacing and at twice it, with twice and half as many
/// dabs, the stroke builds what it does at its own, for the brushes that
/// add a step each dab and for the ones that seek a height alike.
#[test]
fn spacing_does_not_change_how_much_a_stroke_builds() {
    let cases = [
        (Brush::Standard, true),
        (Brush::Standard, false),
        (Brush::Inflate, true),
        (Brush::Inflate, false),
        (Brush::Clay, true),
        (Brush::Clay, false),
        (Brush::Layer, false),
        (Brush::Crease, true),
        (Brush::Crease, false),
        (Brush::Flatten, false),
        (Brush::Smooth, false),
    ];
    let mut apart = Vec::new();
    for (brush, accumulate) in cases {
        let own = brush.default_spacing();
        let (build, dabs) = at_spacings(brush, accumulate, [own * 0.5, own, own * 2.0]);
        println!("{brush:?} accumulate {accumulate}: did {build:.4?} with {dabs:?} dabs, {:.1}% apart", spread(&build) * 100.0);
        assert!(build[1] > 0.0, "{brush:?} did nothing");
        assert!(dabs[0] as f32 > dabs[1] as f32 * 1.7 && (dabs[1] as f32) > dabs[2] as f32 * 1.7, "{brush:?}: {dabs:?} dabs");
        // Crease with Accumulate on is let off a little at twice its
        // spacing: it cuts into the surface its own path is found on, the
        // path over a groove cut in bigger steps is longer, and it lays its
        // dabs by that distance (31 of them here where 29 would do).
        let most = if brush == Brush::Crease && accumulate { 0.08 } else { 0.03 };
        if spread(&build) >= most {
            apart.push(format!("{brush:?} accumulate {accumulate} did {build:?} at half, its own and twice its spacing"));
        }
    }
    assert!(apart.is_empty(), "{apart:#?}");
}

/// Past a spacing of one radius the dabs are separate prints. Each stands
/// as tall as the stroke the brush draws at its own spacing, however far
/// apart they are set, with the untouched surface between them.
#[test]
fn widely_spaced_dabs_are_prints_as_tall_as_the_stroke() {
    let profile = |spacing: f32| {
        let mut mesh = sphere(200_000);
        let view = view_of(&mesh);
        // As strong as 3% was before the slider was mapped (`strength.rs`).
        let mut sculptor = sculptor_with(Brush::Inflate, 0.06, strength::from_old(Brush::Inflate, 0.03));
        // A brush that adds, as Inflate started until 2026-10-21: how much a
        // print does is the dose (one that builds to a height for the stroke
        // stands about a sixth taller as a print than as a stroke).
        sculptor.settings.current_mut().accumulate = true;
        sculptor.settings.current_mut().spacing = spacing;
        let dabs = draw(&mut mesh, &mut sculptor, &view, &across(&view, -0.5, 0.5, 200), 4, None);
        (ridge(&mesh, -0.5, 0.5, 50), dabs)
    };
    let (stroke, _) = profile(0.2);
    let middle = &stroke[15..35];
    let tall = middle.iter().sum::<f32>() / middle.len() as f32;
    assert!(tall > 0.001 && middle.iter().all(|&h| (h / tall - 1.0).abs() < 0.1), "the stroke's ridge: {middle:?}");
    for spacing in [2.2, 2.5] {
        let (prints, dabs) = profile(spacing);
        // One print every `spacing` radii of the path, which is 0.5 long.
        let expected = (0.5 / (spacing * 0.06)).floor() as u64 + 1;
        assert_eq!(dabs, expected, "at {spacing}");
        let is_peak = |i: &usize| prints[*i] > prints[i - 1] && prints[*i] >= prints[i + 1] && prints[*i] > tall * 0.5;
        let peaks: Vec<usize> = (1..49).filter(is_peak).collect();
        assert!(peaks.len() as u64 >= expected - 2, "at {spacing}: {} prints show of {expected}: {prints:?}", peaks.len());
        for &peak in &peaks {
            assert!((prints[peak] / tall - 1.0).abs() < 0.12, "at {spacing} a print stands {}, the stroke {tall}", prints[peak]);
        }
        // Between one print and the next the surface is as it was.
        for pair in peaks.windows(2) {
            let lowest = prints[pair[0]..pair[1]].iter().fold(f32::MAX, |low, &h| low.min(h));
            assert!(lowest < tall * 0.1, "at {spacing} the prints run together: {lowest} between two of them: {prints:?}");
        }
    }
}

/// A brush that keeps working while the pointer rests does so at the pace
/// it always did, whatever its Spacing: that is about dabs along a stroke.
/// Unless it is set to lay separate prints: a print goes where the stroke
/// puts it, and resting adds none.
#[test]
fn a_resting_brush_keeps_its_pace_whatever_its_spacing() {
    let rested = |spacing: f32| {
        let mut mesh = sphere(20_000);
        let view = view_of(&mesh);
        let mut sculptor = sculptor_with(Brush::Layer, 0.1, 0.05);
        sculptor.settings.current_mut().spacing = spacing;
        assert!(sculptor.begin(&mut mesh, &view, PointerSample { pos: SIZE * 0.5, pressure: None, time: 0.0 }, false, false));
        for _ in 0..60 {
            sculptor.frame(&mut mesh, &view, 1.0 / 60.0);
        }
        sculptor.end(&mut mesh, &view);
        sculptor.dabs_applied
    };
    let usual = rested(0.2);
    assert!((10..=12).contains(&usual), "a second's rest laid {usual} dabs");
    for spacing in [0.02, 0.07, 0.5, 0.9] {
        assert!(rested(spacing).abs_diff(usual) <= 1, "at {spacing}: {} dabs, against {usual}", rested(spacing));
    }
    assert_eq!(rested(1.0), 1, "a print, and no more for resting on it");
    assert_eq!(rested(2.5), 1);

    // A pause at the end of a widely spaced stroke: nothing is owed for the
    // way already come, so it brings one dab in its turn and not a burst.
    let paused = |spacing: f32, frames: usize| {
        let mut mesh = sphere(20_000);
        let view = view_of(&mesh);
        let mut sculptor = sculptor_with(Brush::Layer, 0.05, 0.05);
        sculptor.settings.current_mut().spacing = spacing;
        let path = across(&view, -0.2, 0.2, 40);
        let sample = |i: usize| PointerSample { pos: path[i], pressure: None, time: i as f64 / 240.0 };
        assert!(sculptor.begin(&mut mesh, &view, sample(0), false, false));
        for i in 1..path.len() {
            sculptor.sample(&mut mesh, &view, sample(i));
        }
        // The stroke's own travel is this first frame's; then it rests.
        (0..=frames).for_each(|_| sculptor.frame(&mut mesh, &view, 1.0 / 60.0));
        sculptor.end(&mut mesh, &view);
        sculptor.dabs_applied
    };
    // 0.2 of the way round a ball of radius 0.5, a dab every 0.9 of 0.05.
    let moving = paused(0.9, 0);
    assert!((5..=6).contains(&moving), "{moving} dabs along the path");
    assert!(paused(0.9, 1) <= moving + 1, "one frame's pause laid {} dabs more", paused(0.9, 1) - moving);
    let rest = paused(0.9, 30) - moving;
    assert!((4..=6).contains(&rest), "half a second's pause laid {rest} dabs");
}

// ---------------------------------------------------- scatter and its kin

#[test]
fn a_dabs_random_numbers_are_in_range_and_the_same_every_time() {
    let seed = jitter::seed(Vec3::new(0.1, -0.2, 0.47));
    assert_eq!(seed, jitter::seed(Vec3::new(0.1, -0.2, 0.47)));
    assert_ne!(seed, jitter::seed(Vec3::new(0.1, -0.2, 0.47000003)), "the next float along is another stroke");
    let (mut centre, mut shrink, mut turn, mut far) = (Vec2::ZERO, 0.0, 0.0, 0);
    let n = 4000;
    for dab in 0..n {
        let draw = Jitter::of(seed, dab);
        assert_eq!(draw, Jitter::of(seed, dab));
        assert!(draw.scatter.length() <= 1.0 + 1e-6 && (0.0..1.0).contains(&draw.shrink) && (-1.0..1.0).contains(&draw.turn));
        centre += draw.scatter;
        shrink += draw.shrink;
        turn += draw.turn;
        far += usize::from(draw.scatter.length() > 0.5);
        assert_ne!(draw, Jitter::of(seed, dab + 1));
        assert_ne!(draw, Jitter::of(seed ^ 1, dab));
    }
    let n = n as f32;
    assert!((centre / n).length() < 0.03, "scatter leans {}", centre / n);
    assert!((shrink / n - 0.5).abs() < 0.03 && (turn / n).abs() < 0.03, "{} {}", shrink / n, turn / n);
    // Evenly over the disc: three quarters of its area is past half way out.
    assert!((far as f32 / n - 0.75).abs() < 0.03, "{far} of {n} dabs past half way");
}

/// A stroke with everything random about it switched on: Scatter, Size
/// Jitter and an alpha with Random Rotation.
fn random_stroke(brush: Brush, setup: &dyn Fn(&mut Sculptor), from: f32) -> (Mesh, u64) {
    let mut mesh = sphere(80_000);
    let view = view_of(&mesh);
    let mut sculptor = sculptor_with(brush, 0.07, 0.4);
    let slot = brush.index();
    let bs = &mut sculptor.settings.per_brush[slot];
    (bs.scatter, bs.size_jitter, bs.spacing) = (0.8, 0.5, 0.4);
    if brush.takes_alpha() {
        sculptor.settings.alpha[slot] =
            alpha::AlphaSettings { image: alpha::find("Scales"), random_rotation: 180.0, ..Default::default() };
        assert!(sculptor.settings.alpha[slot].image.is_some());
    }
    setup(&mut sculptor);
    let dabs = draw(&mut mesh, &mut sculptor, &view, &across(&view, from, from + 0.7, 70), 4, None);
    (mesh, dabs)
}

/// Random is not the same as different every time: the same stroke made
/// again is the same to the bit, another stroke is not like it, and each of
/// the three settings does something.
#[test]
fn a_random_stroke_made_again_is_the_same_stroke() {
    for brush in [Brush::Standard, Brush::Clay, Brush::Inflate, Brush::Layer, Brush::Flatten, Brush::Crease] {
        let (first, dabs) = random_stroke(brush, &|_| {}, -0.35);
        let (again, again_dabs) = random_stroke(brush, &|_| {}, -0.35);
        assert!(dabs >= 8 && dabs == again_dabs, "{brush:?}: {dabs} and {again_dabs} dabs");
        assert!(first.positions == again.positions, "{brush:?}: made again, the stroke came out different");
        let slot = brush.index();
        let without = |setup: &dyn Fn(&mut Sculptor)| fingerprint(&random_stroke(brush, setup, -0.35).0);
        if brush == Brush::Flatten || brush == Brush::Crease {
            continue;
        }
        let whole = fingerprint(&first);
        assert_ne!(whole, without(&|s| s.settings.per_brush[slot].scatter = 0.0), "{brush:?}: Scatter did nothing");
        assert_ne!(whole, without(&|s| s.settings.per_brush[slot].size_jitter = 0.0), "{brush:?}: Size Jitter did nothing");
        assert_ne!(whole, without(&|s| s.settings.alpha[slot].random_rotation = 0.0), "{brush:?}: Random Rotation did nothing");
    }
}

/// Scattered dabs land off the path, by no more than Scatter allows, and
/// on the surface.
#[test]
fn scatter_throws_dabs_off_the_path_along_the_surface() {
    let reach = |scatter: f32| {
        let mut mesh = sphere(80_000);
        let view = view_of(&mesh);
        let mut sculptor = sculptor_with(Brush::Inflate, 0.03, 0.5);
        let bs = sculptor.settings.current_mut();
        (bs.scatter, bs.spacing) = (scatter, 1.0);
        draw(&mut mesh, &mut sculptor, &view, &across(&view, -0.4, 0.4, 100), 4, None);
        trace_reach(&mesh, -0.3, 0.3)
    };
    let (plain, thrown) = (reach(0.0), reach(2.0));
    assert!(plain < 0.035, "an unscattered stroke reaches {plain} from its path");
    // Up to twice the radius off the path, and a dab's own reach beyond.
    assert!(thrown > plain + 0.03 && thrown < 0.095, "scattered, it reaches {thrown}");
}

/// The mirror image of every vertex across X.
pub(super) fn twins(mesh: &Mesh) -> Vec<u32> {
    let key = |p: Vec3| [(p.x + 0.0).to_bits(), (p.y + 0.0).to_bits(), (p.z + 0.0).to_bits()];
    let map: std::collections::HashMap<[u32; 3], u32> =
        mesh.positions.iter().enumerate().map(|(i, &p)| (key(p), i as u32)).collect();
    mesh.positions.iter().map(|&p| map[&key(p * Vec3::new(-1.0, 1.0, 1.0))]).collect()
}

/// A dab's size and its random numbers are settled before it is mirrored:
/// a stroke that tapers with the pen, scattered and jittered, with an alpha
/// turned at random, is still the mirror image of itself across the plane,
/// to the same closeness as a plain one.
#[test]
fn shaped_strokes_stay_mirror_images_under_symmetry() {
    for brush in [Brush::Standard, Brush::Clay, Brush::Inflate, Brush::Layer, Brush::Flatten, Brush::Crease, Brush::Smooth] {
        let mut mesh = sphere(80_000);
        if brush == Brush::Smooth {
            // Something to smooth, itself a mirror image.
            for p in &mut mesh.positions {
                *p *= 1.0 + 0.01 * (p.x.abs() * 60.0).sin() * (p.y * 60.0).sin();
            }
            mesh = Mesh::new(mesh.positions.clone(), mesh.indices.clone(), None);
        }
        let before = mesh.positions.clone();
        let twins = twins(&mesh);
        let view = view_of(&mesh);
        let mut sculptor = sculptor_with(brush, 0.09, 0.5);
        sculptor.settings.symmetry = [true, false, false];
        sculptor.settings.pressure_curve = 1.0;
        let slot = brush.index();
        let bs = &mut sculptor.settings.per_brush[slot];
        (bs.scatter, bs.size_jitter, bs.spacing, bs.pressure_size) = (0.5, 0.5, 0.3, 0.8);
        if brush.takes_alpha() {
            sculptor.settings.alpha[slot] =
                alpha::AlphaSettings { image: alpha::find("Scales"), random_rotation: 180.0, ..Default::default() };
        }
        sculptor.pen.smoothed = 0.3;
        // From one side, across the mirror plane and back along it.
        let c = SIZE * 0.5;
        let (side, plane, along) = (c + Vec2::new(-110.0, 60.0), c + Vec2::new(6.0, 10.0), c + Vec2::new(14.0, -90.0));
        let mut path: Vec<Vec2> = (0..=40).map(|i| side.lerp(plane, i as f32 / 40.0)).collect();
        path.extend((1..=40).map(|i| plane.lerp(along, i as f32 / 40.0)));
        let dabs = draw(&mut mesh, &mut sculptor, &view, &path, 4, Some(&|along| 0.3 + 0.7 * along));
        assert!(dabs > 20, "{brush:?}: {dabs} dabs");
        let flip = Vec3::new(-1.0, 1.0, 1.0);
        let (mut worst, mut biggest) = (0.0f32, 0.0f32);
        for (v, &t) in twins.iter().enumerate() {
            worst = worst.max(mesh.positions[t as usize].distance(mesh.positions[v] * flip));
            biggest = biggest.max(mesh.positions[v].distance(before[v]));
        }
        println!("{brush:?}: moved {biggest:.5}, mirror error {worst:.2e}");
        assert!(biggest > 1e-4, "{brush:?} did nothing");
        assert!(worst / biggest < 0.01, "{brush:?} drifted {:.2}% off symmetry", worst / biggest * 100.0);
    }
}

/// A print being dragged out is made afresh every frame. With Random
/// Rotation it must be the same print each time: dragged out over many
/// frames it ends as it does dragged out in one, and not as it would with
/// no rotation.
#[test]
fn a_dragged_print_keeps_its_random_turn_while_it_is_dragged() {
    let print = |random: f32, steps: usize| {
        let mut mesh = sphere(80_000);
        let view = view_of(&mesh);
        let mut sculptor = sculptor_with(Brush::Standard, 0.1, 0.6);
        sculptor.settings.alpha[Brush::Standard.index()] =
            alpha::AlphaSettings { image: alpha::find("Rake"), drag: true, random_rotation: random, ..Default::default() };
        let c = SIZE * 0.5;
        let at = |i: usize| PointerSample {
            pos: c + Vec2::new(70.0, 20.0) * (i as f32 / steps as f32),
            pressure: None,
            time: i as f64 / 60.0,
        };
        assert!(sculptor.begin(&mut mesh, &view, at(0), false, false));
        for i in 1..=steps {
            sculptor.sample(&mut mesh, &view, at(i));
            sculptor.frame(&mut mesh, &view, 1.0 / 60.0);
        }
        sculptor.end(&mut mesh, &view);
        assert!(mesh.positions.iter().any(|&p| height(p) > 1e-3), "nothing was printed");
        fingerprint(&mesh)
    };
    assert_eq!(print(180.0, 1), print(180.0, 24));
    assert_ne!(print(180.0, 24), print(0.0, 24));
}

// ------------------------------------------------- falling behind the pen

/// What [`fast_stroke`] is drawn on: something rough for the two brushes
/// that level.
fn fast_stroke_ball(brush: Brush) -> Mesh {
    if matches!(brush, Brush::Smooth | Brush::Flatten) { rough_ball(80_000) } else { sphere(80_000) }
}

/// A fast stroke: a long path given as a handful of samples, so each is
/// many dabs' worth of travel. `at_once` hands the whole path over before
/// the first frame (the furthest behind a stroke can be) and then lets
/// that many frames pass; otherwise two samples arrive a frame, as from a
/// pen on the move. With `lift` the path leaves the mesh and comes back on
/// the way.
fn fast_stroke(
    brush: Brush,
    mask: bool,
    pace: Pace,
    at_once: Option<usize>,
    lift: bool,
    setup: &dyn Fn(&mut Sculptor),
) -> (Mesh, u64) {
    let mut mesh = fast_stroke_ball(brush);
    let view = view_of(&mesh);
    let mut sculptor = sculptor_with(brush, 0.05, 0.1);
    // Clay's finishing pass is once a frame's work, and is on.
    sculptor.settings.clay_relax = Settings::default().clay_relax;
    sculptor.settings.mask_mode = mask;
    sculptor.pace = pace;
    setup(&mut sculptor);
    let mut path = across(&view, -0.45, 0.2, 9);
    if lift {
        path.push(SIZE * 0.5 + Vec2::new(560.0, -40.0));
    }
    path.extend(across(&view, 0.25, 0.45, 3).into_iter().map(|p| p + Vec2::new(0.0, 30.0)));
    let sample = |i: usize| PointerSample { pos: path[i], pressure: None, time: i as f64 / 240.0 };
    let frame = |mesh: &mut Mesh, sculptor: &mut Sculptor| {
        sculptor.frame(mesh, &view, 1.0 / 60.0);
        let _ = mesh.take_dirty();
    };
    assert!(sculptor.begin(&mut mesh, &view, sample(0), false, false));
    for i in 1..path.len() {
        sculptor.sample(&mut mesh, &view, sample(i));
        if at_once.is_none() && i % 2 == 0 {
            frame(&mut mesh, &mut sculptor);
        }
    }
    (0..at_once.unwrap_or(0)).for_each(|_| frame(&mut mesh, &mut sculptor));
    sculptor.end(&mut mesh, &view);
    let _ = mesh.take_dirty();
    (mesh, sculptor.dabs_applied)
}

/// A stroke that a frame can afford only two dabs of falls behind the pen
/// and is finished at the release: every dab, each where it belongs, for
/// every brush that can fall behind. The frames used to drop the travel
/// they could not afford, which left such a stroke as a row of separate
/// dabs.
///
/// The same stroke comes out the same to the bit however its samples reach
/// it: two a frame, or all of them before the first frame with any number
/// of frames to work them off in before the release. And it comes out as
/// the stroke of a machine that kept up does, to within what the order of
/// frames and dabs does to any stroke (normals are brought up to date once
/// a frame, so dabs see them a little staler or fresher).
#[test]
fn a_stroke_that_falls_behind_comes_out_as_if_it_had_kept_up() {
    let kinds = [
        (Brush::Standard, false),
        (Brush::Clay, false),
        (Brush::Inflate, false),
        (Brush::Layer, false),
        (Brush::Flatten, false),
        (Brush::Crease, false),
        (Brush::Smooth, false),
        (Brush::SnakeHook, false),
        (Brush::Standard, true),
    ];
    let mut far_off = Vec::new();
    for (brush, mask) in kinds {
        let (behind, behind_dabs) = fast_stroke(brush, mask, Pace::Dabs(2), None, false, &|_| {});
        // (Still behind after any of these: a brush that works while the
        // pointer rests would otherwise go on to do that.)
        for frames in [0, 5, 15] {
            let (all_at_once, at_once_dabs) = fast_stroke(brush, mask, Pace::Dabs(2), Some(frames), false, &|_| {});
            assert_eq!(at_once_dabs, behind_dabs, "{brush:?} mask {mask}, {frames} frames");
            let same = all_at_once.positions == behind.positions && all_at_once.mask == behind.mask;
            assert!(same, "{brush:?} mask {mask}: the stroke depends on when its samples arrived ({frames} frames)");
        }

        // Against the stroke that kept up, without Clay's finishing pass:
        // that works once a frame on the frame's dabs, as hard for two as
        // for eight, so how much of it a stroke gets has always gone by how
        // many frames the stroke took.
        let plain = |sculptor: &mut Sculptor| sculptor.settings.clay_relax = 0.0;
        let was = fast_stroke_ball(brush);
        let (kept_up, dabs) = fast_stroke(brush, mask, Pace::Unpaced, None, false, &plain);
        assert!(dabs >= 40, "{brush:?}: the path is only {dabs} dabs long");
        let moved = |mesh: &Mesh| mesh.positions.iter().zip(&was.positions).map(|(a, b)| a.distance(*b)).fold(0.0f32, f32::max);
        let most = moved(&kept_up).max(kept_up.mask.iter().fold(0.0f32, |m, &v| m.max(v)));
        assert!(most > 1e-4, "{brush:?} did nothing");
        let (behind, behind_dabs) = fast_stroke(brush, mask, Pace::Dabs(2), None, false, &plain);
        let (one_a_frame, _) = fast_stroke(brush, mask, Pace::Time(Duration::ZERO), None, false, &plain);
        let apart = |a: &Mesh, b: &Mesh| {
            let off = a.positions.iter().zip(&b.positions).map(|(a, b)| a.distance(*b)).fold(0.0f32, f32::max);
            off.max(a.mask.iter().zip(&b.mask).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max))
        };
        // The yardstick: two strokes that both kept up, one with a frame
        // every two samples and one with no frame until its release.
        let (one_frame, _) = fast_stroke(brush, mask, Pace::Unpaced, Some(0), false, &plain);
        let frames_alone = apart(&one_frame, &kept_up);
        for (name, late) in [("two dabs a frame", &behind), ("one dab a frame", &one_a_frame)] {
            let off = apart(late, &kept_up);
            println!("{brush:?} mask {mask}, {name}: {behind_dabs} dabs against {dabs}; {off:.2e} from the stroke that kept up");
            println!("    which moved {most:.2e}; where the frames fall makes {frames_alone:.2e} by itself");
            if off >= most * 0.01 + frames_alone * 1.5 {
                far_off.push(format!("{brush:?} mask {mask}, {name}: {off} from the stroke that kept up, which moved {most}"));
            }
        }
        assert!(behind_dabs.abs_diff(dabs) <= 1, "{brush:?} mask {mask}: {behind_dabs} dabs against {dabs}");
    }
    assert!(far_off.is_empty(), "{far_off:#?}");
    // With symmetry, scattered and jittered, and leaving the mesh and
    // coming back on the way, as well. (At a pace with room to spare: the
    // trip off the mesh counts as path still to lay until it is worked
    // through, and a backlog that looks hopeless is laid coarser.)
    let shaped = |pace: Pace, at_once: Option<usize>| {
        fast_stroke(Brush::Standard, false, pace, at_once, true, &|sculptor| {
            sculptor.settings.symmetry = [true, true, false];
            let bs = sculptor.settings.current_mut();
            (bs.scatter, bs.size_jitter) = (0.6, 0.4);
        })
    };
    let (kept_up, dabs) = shaped(Pace::Unpaced, None);
    for at_once in [None, Some(0), Some(3)] {
        let (late, late_dabs) = shaped(Pace::Dabs(8), at_once);
        let off = late.positions.iter().zip(&kept_up.positions).map(|(a, b)| a.distance(*b)).fold(0.0f32, f32::max);
        println!("shaped, {at_once:?}: {late_dabs} dabs against {dabs}; {off:.2e} from the stroke that kept up");
        assert_eq!(late_dabs, dabs);
        assert!(off < 2e-4, "a shaped stroke that fell behind is {off} from the one that kept up");
    }
}

/// A pace has a least: one dab a frame, so a stroke always moves on.
#[test]
fn a_frame_always_has_room_for_one_dab() {
    for pace in [Pace::Dabs(0), Pace::Dabs(1), Pace::Time(Duration::ZERO)] {
        let mut meter = pace::Meter::default();
        assert!(meter.has_room(pace), "{pace:?}");
        meter.dab(Duration::from_millis(3));
        assert!(!meter.has_room(pace), "{pace:?} has room for a second");
        meter.new_frame(pace);
        assert!(meter.has_room(pace), "{pace:?} in the next frame");
    }
    let mut meter = pace::Meter::default();
    for _ in 0..3 {
        assert!(meter.has_room(Pace::Dabs(3)) && meter.has_room(Pace::Unpaced));
        meter.dab(Duration::from_millis(4));
    }
    assert!(!meter.has_room(Pace::Dabs(3)) && meter.has_room(Pace::Unpaced));
    // What a second lays: sixty frames' dabs, or the working share of a
    // second at what a dab has been costing; and no limit when unpaced.
    assert_eq!(meter.per_second(Pace::Dabs(0)), 60.0);
    assert_eq!(meter.per_second(Pace::Dabs(3)), 180.0);
    assert!((meter.per_second(Pace::LIVE) - 150.0).abs() < 1.0, "{}", meter.per_second(Pace::LIVE));
    assert_eq!(meter.per_second(Pace::Unpaced), f32::INFINITY);
    assert_eq!(pace::Meter::default().per_second(Pace::LIVE), f32::INFINITY, "no dab has been timed yet");
}

/// While it is behind, the stroke has laid only what its frames could
/// afford and knows there is more; nothing along its path is skipped (the
/// ridge it leaves has no gap in it); and the release finishes it.
#[test]
fn a_fast_stroke_leaves_no_gaps_and_the_release_finishes_it() {
    let mut mesh = sphere(200_000);
    let view = view_of(&mesh);
    let mut sculptor = sculptor_with(Brush::Inflate, 0.05, strength::from_old(Brush::Inflate, 0.1));
    sculptor.pace = Pace::Dabs(2);
    let path = across(&view, -0.6, 0.6, 14);
    let sample = |i: usize| PointerSample { pos: path[i], pressure: None, time: i as f64 / 240.0 };
    assert!(sculptor.begin(&mut mesh, &view, sample(0), false, false));
    let mut frames = 0;
    for i in 1..path.len() {
        sculptor.sample(&mut mesh, &view, sample(i));
        if i % 2 == 0 {
            sculptor.frame(&mut mesh, &view, 1.0 / 60.0);
            frames += 1;
        }
    }
    let laid = sculptor.dabs_applied;
    assert!(laid <= 2 * frames + 2, "{laid} dabs in {frames} frames of two each");
    assert!(sculptor.stroke.as_ref().unwrap().behind(), "the stroke should be behind the pen");
    // So far the ridge reaches only part of the way along the path.
    let so_far = ridge(&mesh, -0.6, 0.6, 60);
    let reached = so_far.iter().rposition(|&h| h > 1e-4).unwrap();
    assert!(reached < 30, "after {laid} dabs the ridge already reaches stretch {reached} of 60");
    assert!(so_far[1..reached].iter().all(|&h| h > 1e-4), "there is a gap behind the brush: {so_far:?}");

    sculptor.end(&mut mesh, &view);
    let dabs = sculptor.dabs_applied;
    // 1.2 radians round a ball of radius 0.5, a dab every fifth of 0.05.
    assert!((58..=62).contains(&dabs), "{dabs} dabs in the whole stroke");
    let whole = ridge(&mesh, -0.6, 0.6, 60);
    let middle = &whole[3..57];
    let usual = middle.iter().sum::<f32>() / middle.len() as f32;
    assert!(usual > 0.001, "the ridge is only {usual} tall");
    for (i, &h) in middle.iter().enumerate() {
        assert!((h / usual - 1.0).abs() < 0.15, "the ridge is {h} at stretch {} against {usual} along it: {whole:?}", i + 3);
    }
}

/// A stroke the machine cannot keep up with at all: more waiting than about
/// a second can lay. Its backlog is laid coarser, each dab doing as much
/// more, so the release is quick, the stroke is whole and it stands as tall.
#[test]
fn a_hopeless_backlog_is_laid_coarser_not_dropped_and_not_shallower() {
    let stroke = |pace: Pace| {
        let mut mesh = sphere(320_000);
        let view = view_of(&mesh);
        let mut sculptor = sculptor_with(Brush::Inflate, 0.03, 0.1);
        sculptor.pace = pace;
        let dabs = draw(&mut mesh, &mut sculptor, &view, &across(&view, -1.0, 1.0, 20), 4, None);
        (ridge(&mesh, -1.0, 1.0, 80), dabs)
    };
    let (kept_up, all) = stroke(Pace::Unpaced);
    // One dab a frame: sixty is a second's work.
    let (coarser, laid) = stroke(Pace::Dabs(1));
    assert!((160..=172).contains(&all), "{all} dabs at the brush's own spacing");
    assert!(laid < 90 && laid > 45, "{laid} dabs were laid of {all}: about a second's worth was the most to wait for");
    let mean = |ridge: &[f32]| ridge[6..74].iter().sum::<f32>() / 68.0;
    let (usual, tall) = (mean(&kept_up), mean(&coarser));
    assert!((tall / usual - 1.0).abs() < 0.1, "laid coarser the ridge stands {tall}, against {usual}");
    for (i, &h) in coarser[6..74].iter().enumerate() {
        assert!(h > usual * 0.6, "the ridge drops to {h} at stretch {} of 80, against {usual}: {coarser:?}", i + 6);
    }
}

// ------------------------------------------------- Clay sweeping a print

/// A Clay stroke with the Square alpha along the front of the ball: how
/// tall the strip stands in each of 120 stretches of its length, and how
/// many dabs it took.
fn square_strip(accumulate: bool, alpha_on: bool) -> (Vec<f32>, u64) {
    let mut mesh = sphere(600_000);
    let view = view_of(&mesh);
    let mut sculptor = sculptor_with(Brush::Clay, 0.08, 0.5);
    let s = &mut sculptor.settings;
    let slot = Brush::Clay.index();
    if alpha_on {
        s.alpha[slot] = alpha::AlphaSettings { image: alpha::find("Square"), ..Default::default() };
    }
    (s.per_brush[slot].accumulate, s.per_brush[slot].focal_shift, s.per_brush[slot].spacing) = (accumulate, -0.5, 0.3);
    (s.clay_depth, s.clay_softness) = (0.2, 0.1);
    let dabs = draw(&mut mesh, &mut sculptor, &view, &across(&view, -0.4, 0.4, 80), 4, None);
    (ridge(&mesh, -0.6, 0.6, 120), dabs)
}

/// Clay sweeps its tip from each dab to the next, and a stroke that prints
/// an alpha this way has no stamp where it landed: the strip rises over one
/// print's length at its head as it falls at its tail. With the stamp the
/// head stood about a tenth taller than the strip for one print's length
/// and then stepped down: a line across every stroke, one print in.
#[test]
fn a_clay_strip_printing_an_alpha_starts_as_it_ends() {
    for accumulate in [false, true] {
        let (strip, dabs) = square_strip(accumulate, true);
        let level = strip[50..70].iter().sum::<f32>() / 20.0;
        let tallest = strip.iter().copied().fold(0.0, f32::max);
        assert!(level > 0.005, "accumulate {accumulate}: the strip is laid ({level})");
        assert!(tallest < level * 1.01, "accumulate {accumulate}: the head stands {tallest} over a strip of {level}");
        // Rising all the way to the strip's level: no step back down.
        let first = strip.iter().position(|&h| h > 0.0).unwrap();
        let drop = strip[first..60].windows(2).map(|pair| pair[0] - pair[1]).fold(0.0, f32::max);
        assert!(drop < level * 0.01, "accumulate {accumulate}: the head steps down by {drop} of {level}");
        // And as long in the rising as the tail is in the falling.
        let last = strip.iter().rposition(|&h| h > 0.0).unwrap();
        let over_half = |&h: &f32| h > level * 0.5;
        let head = strip[first..].iter().position(over_half).unwrap() as i32;
        let tail = strip[..=last].iter().rev().position(over_half).unwrap() as i32;
        assert!((head - tail).abs() <= 2, "accumulate {accumulate}: half height {head} bins in at the head, {tail} at the tail");
        // One dab fewer than the same stroke with no alpha, which opens
        // with its stamp as it always has.
        let (_, plain) = square_strip(accumulate, false);
        assert_eq!(dabs + 1, plain, "accumulate {accumulate}");
    }
}

/// A press that never sets off is still one print: when it lifts, or once
/// it has been held a moment, and never twice.
#[test]
fn a_press_with_a_swept_alpha_that_never_sets_off_prints_once() {
    let press = |held: f64| {
        let mut mesh = sphere(120_000);
        let view = view_of(&mesh);
        let mut sculptor = sculptor_with(Brush::Clay, 0.1, 0.5);
        sculptor.settings.alpha[Brush::Clay.index()] = alpha::AlphaSettings { image: alpha::find("Square"), ..Default::default() };
        let at = |time: f64| PointerSample { pos: SIZE * 0.5, pressure: Some(0.8), time };
        assert!(sculptor.begin(&mut mesh, &view, at(50.0), false, false));
        // The pen held where it landed: it still reports, without moving.
        let mut time = 50.0;
        while time < 50.0 + held {
            time += 1.0 / 60.0;
            sculptor.sample(&mut mesh, &view, at(time));
            sculptor.frame(&mut mesh, &view, 1.0 / 60.0);
        }
        let while_held = sculptor.dabs_applied;
        sculptor.end(&mut mesh, &view);
        (while_held, sculptor.dabs_applied, fingerprint(&mesh))
    };
    let untouched = fingerprint(&sphere(120_000));
    let (while_held, tap, tapped) = press(0.0);
    assert_eq!((while_held, tap), (0, 1), "a tap prints as it lifts");
    assert_ne!(tapped, untouched);
    let (while_held, short, _) = press(0.1);
    assert_eq!((while_held, short), (0, 1), "a landing is not yet a print");
    let (while_held, long, held) = press(0.6);
    assert_eq!((while_held, long), (1, 1), "held a moment, it prints without lifting, and once");
    assert_eq!(held, tapped, "the same print either way");
}

