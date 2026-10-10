//! Layer where mirror planes cross.
//!
//! Layer raises the surface to a set height and no further, however many
//! times the stroke passes over it. Next to one mirror plane a dab and its
//! image are applied in both orders and averaged, each only topping up the
//! other. Next to two or three planes at once there are four or eight copies,
//! applied to the same starting surface and their moves added up and shared
//! out by how much the copies overlap (`Sculptor::apply_symmetric`). A
//! quarter of a radius from two planes the four copies count as 2.93 dabs
//! while all four cover the middle, so the middle came up 4 / 2.93 = 1.37
//! times the height (1.33 to 1.37 measured here, 1.50 with three planes), and
//! Layer never takes anything back down. Such a brush is now also held to the
//! furthest any one copy moved a vertex (`kernels::held_to_furthest`), as it
//! already was round a radial axis.
//!
//! These tests measure that: the height reached on the line the planes cross
//! along and across the whole dab, against the height the brush is set to,
//! and against the highest any one copy of the mark would have raised each
//! vertex on its own.

use glam::Quat;
use sculpt_symmetry::Placement;

use super::*;

/// A place on the ball where two mirror planes cross, and what is sculpted
/// with there.
#[derive(Clone, Copy, Debug)]
struct Corner {
    /// The mirror axes that are ticked.
    mirror: [bool; 3],
    /// The line the two planes cross along, out through the surface.
    along: Vec3,
    /// The normals of the two planes.
    planes: [Vec3; 2],
    /// The brush's radius on the model.
    radius: f32,
}

/// A brush big enough that a dab by two planes is within reach of the third
/// as well, so all eight copies are walked as one set.
const WIDE_RADIUS: f32 = 0.36;

const CORNERS: [Corner; 6] = [
    Corner { mirror: [true, true, false], along: Vec3::Z, planes: [Vec3::X, Vec3::Y], radius: BRUSH_RADIUS },
    Corner { mirror: [true, false, true], along: Vec3::Y, planes: [Vec3::X, Vec3::Z], radius: BRUSH_RADIUS },
    Corner { mirror: [false, true, true], along: Vec3::X, planes: [Vec3::Y, Vec3::Z], radius: BRUSH_RADIUS },
    // All three ticked: the third plane is out of reach of a dab this size,
    // so the eight copies are two sets of four, one on each side of the ball.
    Corner { mirror: [true, true, true], along: Vec3::Z, planes: [Vec3::X, Vec3::Y], radius: BRUSH_RADIUS },
    Corner { mirror: [true, true, true], along: Vec3::Y, planes: [Vec3::X, Vec3::Z], radius: BRUSH_RADIUS },
    Corner { mirror: [true, true, true], along: Vec3::Z, planes: [Vec3::X, Vec3::Y], radius: WIDE_RADIUS },
];

/// Where a dab is laid, in brush radii from each of the two planes: on both,
/// a quarter and half a radius and one radius from both, well clear of both
/// (no plane in reach, the plain walk), and unevenly (the last is next to one
/// plane only, the pair that is applied in both orders).
const DISTANCES: [(f32, f32); 8] =
    [(0.0, 0.0), (0.25, 0.25), (0.5, 0.5), (1.0, 1.0), (4.0, 4.0), (0.0, 0.25), (0.25, 1.0), (0.25, 4.0)];

impl Corner {
    /// The point on the ball `a` and `b` brush radii from the two planes.
    fn place(&self, a: f32, b: f32) -> Vec3 {
        let off = (self.planes[0] * a + self.planes[1] * b) * self.radius;
        let out = (BALL_RADIUS * BALL_RADIUS - off.length_squared()).sqrt();
        point_on_ball((off + self.along * out).normalize())
    }

    /// The distances that fit on the ball: the wide brush is most of its size.
    fn distances(&self) -> impl Iterator<Item = (f32, f32)> + '_ {
        DISTANCES.into_iter().filter(|&(a, b)| a.max(b) * self.radius < 0.6 * BALL_RADIUS)
    }

    /// The camera the marks are made under: looking at the corner from the
    /// side the marks are on.
    fn view(&self, mesh: &Mesh) -> View {
        view_from(mesh, self.along * 2.0 + self.planes[0] + self.planes[1])
    }

    fn group(&self) -> SymmetryGroup {
        Symmetry { mirror: self.mirror, repeats: 1 }.group()
    }

    /// The height Layer is set to with this brush.
    fn set_height(&self) -> f32 {
        self.radius * Settings::default().layer_height
    }

    fn name(&self) -> String {
        let axes: String = (0..3).filter(|&axis| self.mirror[axis]).map(|axis| ["X", "Y", "Z"][axis]).collect();
        let size = if self.radius == BRUSH_RADIUS { "" } else { ", wide brush" };
        let along = ["X", "Y", "Z"][self.along.abs().max_position()];
        format!("mirror {axes} where the planes cross along {along}{size}")
    }
}

/// A camera on the whole ball from the direction `from`.
fn view_from(mesh: &Mesh, from: Vec3) -> View {
    let from = from.normalize();
    let mut camera = Camera::default();
    camera.frame(mesh.bounds(), SIZE.x / SIZE.y, false);
    (camera.yaw, camera.pitch) = ((-from.x).atan2(from.z).to_degrees(), from.y.asin().to_degrees());
    camera.view(Vec2::ZERO, SIZE, camera.distance, 1.0)
}

fn layer(radius: f32, strength: f32, mirror: [bool; 3]) -> Sculptor {
    let mut sculptor = sculptor_with(Brush::Layer);
    sculptor.settings.radius_world = radius;
    sculptor.settings.current_mut().strength = strength;
    sculptor.settings.symmetry = mirror;
    sculptor
}

/// What a mark left behind.
struct Raised {
    /// How far it moved each vertex of the ball.
    by: Vec<f32>,
    /// How far the ball is from mapping onto itself under the symmetry.
    pair: f32,
}

impl Raised {
    fn peak(&self) -> f32 {
        self.by.iter().fold(0.0f32, |a, &b| a.max(b))
    }

    /// The furthest any vertex went past `ceiling`.
    fn over(&self, ceiling: &[f32]) -> f32 {
        self.by.iter().zip(ceiling).map(|(by, ceiling)| by - ceiling).fold(0.0f32, f32::max)
    }
}

/// Makes a mark with `sculptor` (one dab where `points` is a single place, a
/// stroke through them otherwise), measures it and takes it back.
fn mark(
    ball: &mut Ball,
    view: &View,
    mut sculptor: Sculptor,
    points: &[Vec3],
    partners: &[(SymmetryOp, Vec<u32>)],
) -> Raised {
    let delta = match points {
        &[at] => {
            let pos = view.project(at).expect("the dab is in front of the camera");
            let press = PointerSample { pos, pressure: None, time: 0.0 };
            assert!(sculptor.begin(&mut ball.mesh, view, press, false, false), "the dab missed the ball");
            let landed = sculptor.last_hit.expect("a stroke that began has a place");
            assert!(landed.distance(at) < 0.02 * BRUSH_RADIUS, "the dab landed at {landed}, not {at}");
            let delta = sculptor.end(&mut ball.mesh, view);
            assert_eq!(sculptor.dabs_applied, 1, "a press and a release are one dab");
            delta
        }
        _ => stroke_through(&mut ball.mesh, &mut sculptor, view, points),
    };
    measured(ball, delta, partners)
}

/// Presses `sculptor`'s brush at `at` and holds it there for `frames` frames
/// (Layer keeps working while the pointer rests), measures and takes it back.
fn press_and_hold(
    ball: &mut Ball,
    view: &View,
    mut sculptor: Sculptor,
    at: Vec3,
    frames: usize,
    partners: &[(SymmetryOp, Vec<u32>)],
) -> Raised {
    let pos = view.project(at).expect("the dab is in front of the camera");
    let press = PointerSample { pos, pressure: None, time: 0.0 };
    assert!(sculptor.begin(&mut ball.mesh, view, press, false, false), "the dab missed the ball");
    for _ in 0..frames {
        sculptor.frame(&mut ball.mesh, view, 1.0 / 60.0);
        let _ = ball.mesh.take_dirty();
    }
    let delta = sculptor.end(&mut ball.mesh, view);
    assert!(sculptor.dabs_applied > 5, "holding the brush still laid only {} dabs", sculptor.dabs_applied);
    measured(ball, delta, partners)
}

/// What the mark that `delta` records did to the ball, which is put back.
fn measured(ball: &mut Ball, delta: Option<StrokeDelta>, partners: &[(SymmetryOp, Vec<u32>)]) -> Raised {
    let by = ball.mesh.positions.iter().zip(&ball.start).map(|(a, b)| a.distance(*b)).collect();
    let raised = Raised { by, pair: pair_error(&ball.mesh, partners) };
    ball.undo(delta);
    raised
}

/// The highest any one copy of a mark would raise each vertex by itself.
/// `alone` is what the mark does with no symmetry, and a copy does the same
/// to the vertices those are carried onto (a mirror image is its own way back).
fn highest_of_any_copy(alone: &[f32], partners: &[(SymmetryOp, Vec<u32>)]) -> Vec<f32> {
    let highest = |v: usize| partners.iter().fold(alone[v], |most, (_, partner)| most.max(alone[partner[v] as usize]));
    (0..alone.len()).map(highest).collect()
}

/// Every case that failed, so that one run says how far off each of them is
/// rather than stopping at the first.
#[derive(Default)]
struct Failures(Vec<String>);

impl Failures {
    fn check(&mut self, holds: bool, what: &str, report: &str) {
        if !holds {
            self.0.push(format!("{what}: {report}"));
        }
    }

    fn none(self) {
        assert!(self.0.is_empty(), "{} cases failed:\n{}", self.0.len(), self.0.join("\n"));
    }
}

/// How far past the height a mark may come out, as a share of the height,
/// where its copies are applied one after another: the pair by one plane
/// alone (in both orders, averaged), and dab after dab along a stroke. Each
/// raises along the normal of its own footprint and tops up what it measures
/// as missing along that, so on a surface as bumpy as the ball's two that
/// stand on different slopes leave up to 5% more than the height between
/// them. (A Layer stroke with no symmetry at all ends 0 to 13% over on this
/// ball for the same reason.) Piled-up copies were 16 to 50% over.
const IN_TURN: f32 = 0.06;

/// The same where the copies are shared out and held to the furthest any one
/// of them went, by two planes or three: nothing to speak of.
const HELD: f32 = 0.01;

/// The holding itself: a move no longer than the furthest one copy went is
/// left alone, a longer one is cut to that length along its own direction,
/// and a vertex nothing moved stays a vertex nothing moved.
#[test]
fn a_shared_out_move_is_held_to_the_furthest_one_copy_went() {
    let step = Vec3::new(0.0, 3.0, 4.0);
    assert_eq!(kernels::held_to_furthest(step, 36.0), step);
    assert_eq!(kernels::held_to_furthest(step, 25.0), step);
    let held = kernels::held_to_furthest(step, 4.0);
    assert!((held.length() - 2.0).abs() < 1e-6 && held.normalize().distance(step.normalize()) < 1e-6, "{held}");
    assert_eq!(kernels::held_to_furthest(step, 0.0), Vec3::ZERO);
    assert_eq!(kernels::held_to_furthest(Vec3::ZERO, 0.0), Vec3::ZERO);
}

/// One dab, at every distance from the planes and at three strengths: the
/// default (which rises 40% of the way a dab), one that gets there in a dab,
/// and full. Nothing is raised past the set height, nor past what the highest
/// single copy would have made of it: on the line the planes cross along,
/// which every copy covers alike, and everywhere else under the dab. And the
/// ball still maps onto itself across every ticked plane.
#[test]
fn a_layer_dab_by_two_mirror_planes_rises_no_higher_than_one_copy_would() {
    let mut ball = Ball::new();
    let mut failures = Failures::default();
    for corner in CORNERS {
        let view = corner.view(&ball.mesh);
        let partners = ball.partners_under(&corner.group());
        let on_the_line = ball.in_mesh[ball.layout.index_of_direction(corner.along)] as usize;
        let set = corner.set_height();
        println!("{}: set height {set:.3e}", corner.name());
        for (a, b) in corner.distances() {
            let at = [corner.place(a, b)];
            // In reach of one plane only, the dab and its image are a pair.
            let within = |radii: f32| radii < 1.5;
            let slack = if within(a) != within(b) { IN_TURN } else { HELD };
            // At full strength a dab alone goes all the way to the height.
            let full = mark(&mut ball, &view, layer(corner.radius, 1.0, [false; 3]), &at, &[]);
            assert!((0.97..=1.03).contains(&(full.peak() / set)), "a dab alone reached {:.3e}, not {set:.3e}", full.peak());
            let ceiling = highest_of_any_copy(&full.by, &partners);
            for strength in [0.1, 0.5, 1.0] {
                let mirrored = mark(&mut ball, &view, layer(corner.radius, strength, corner.mirror), &at, &partners);
                let (peak, over) = (mirrored.peak(), mirrored.over(&ceiling));
                let report = format!(
                    "{}, {a} and {b} radii from the planes, strength {strength}: peak {:.3} of the set height, \
                     {:.3} on the line (one copy alone {:.3}), {:.3} over one copy at worst, pair {:.1e}",
                    corner.name(),
                    peak / set,
                    mirrored.by[on_the_line] / set,
                    ceiling[on_the_line] / set,
                    over / set,
                    mirrored.pair
                );
                println!("  {report}");
                failures.check(peak <= (1.0 + slack) * set, "past the set height", &report);
                failures.check(over <= slack * set, "past what any one copy would raise", &report);
                failures.check(mirrored.pair <= Tolerance::OTHERS.allows(peak), "no longer mirrored", &report);
            }
        }
    }
    failures.none();
}

/// Short strokes by the corner, each from and to a place in brush radii from
/// the two planes: along one plane a quarter radius from it and across the
/// other, slantwise through the corner, and one that stays inside it.
const STROKES: [((f32, f32), (f32, f32)); 3] =
    [((0.25, -1.5), (0.25, 1.5)), ((-1.2, -0.9), (1.1, 1.3)), ((0.25, 0.25), (0.7, 0.4))];

/// A stroke passes over the corner many times. It still ends at the set
/// height there and not above it (but for what dabs laid one after another
/// leave, see [`IN_TURN`]), and it gets as high as the same stroke does with
/// no symmetry.
#[test]
fn a_layer_stroke_by_two_mirror_planes_keeps_to_its_height() {
    let mut ball = Ball::new();
    let mut failures = Failures::default();
    for corner in CORNERS.into_iter().filter(|corner| corner.radius == BRUSH_RADIUS) {
        let view = corner.view(&ball.mesh);
        let partners = ball.partners_under(&corner.group());
        let on_the_line = ball.in_mesh[ball.layout.index_of_direction(corner.along)] as usize;
        let set = corner.set_height();
        for ((from_a, from_b), (to_a, to_b)) in STROKES {
            let points: Vec<Vec3> = (0..STROKE_FRAMES)
                .map(|frame| {
                    let t = frame as f32 / (STROKE_FRAMES - 1) as f32;
                    corner.place(from_a + (to_a - from_a) * t, from_b + (to_b - from_b) * t)
                })
                .collect();
            for strength in [0.1, 0.5, 1.0] {
                let alone = mark(&mut ball, &view, layer(corner.radius, strength, [false; 3]), &points, &[]);
                let mirrored = mark(&mut ball, &view, layer(corner.radius, strength, corner.mirror), &points, &partners);
                let (peak, on_the_line) = (mirrored.peak(), mirrored.by[on_the_line]);
                let report = format!(
                    "{}, a stroke from {from_a}, {from_b} to {to_a}, {to_b} radii, strength {strength}: \
                     peak {:.3} of the set height ({:.3} with no symmetry), {:.3} on the line, pair {:.1e}",
                    corner.name(),
                    peak / set,
                    alone.peak() / set,
                    on_the_line / set,
                    mirrored.pair
                );
                println!("{report}");
                failures.check(peak <= (1.0 + IN_TURN) * set, "past the set height", &report);
                failures.check(on_the_line <= (1.0 + IN_TURN) * set, "past the set height on the line", &report);
                failures.check(peak >= 0.9 * alone.peak(), "lower than the stroke alone", &report);
                failures.check(mirrored.pair <= Tolerance::OTHERS.allows(peak), "no longer mirrored", &report);
            }
        }
    }
    failures.none();
}

/// Shared out, one dab by the corner is lower than the height wherever fewer
/// copies cover a vertex than the set counts as: half a radius from both
/// planes its highest point is nine tenths of it. Layer keeps working while
/// the pointer rests, and a press held there for a second and a half comes up
/// to the height all the same, at the default strength too, and stops at it.
#[test]
fn a_layer_dab_held_by_two_mirror_planes_comes_up_to_its_height_and_stops() {
    let mut ball = Ball::new();
    let mut failures = Failures::default();
    for corner in CORNERS.into_iter().filter(|corner| corner.radius == BRUSH_RADIUS) {
        let view = corner.view(&ball.mesh);
        let partners = ball.partners_under(&corner.group());
        let on_the_line = ball.in_mesh[ball.layout.index_of_direction(corner.along)] as usize;
        let set = corner.set_height();
        for (a, b) in [(0.25, 0.25), (0.5, 0.5), (0.0, 0.25), (0.25, 1.0)] {
            let at = corner.place(a, b);
            let full = mark(&mut ball, &view, layer(corner.radius, 1.0, [false; 3]), &[at], &[]);
            let ceiling = highest_of_any_copy(&full.by, &partners);
            for strength in [0.1, 1.0] {
                let brush = layer(corner.radius, strength, corner.mirror);
                let held = press_and_hold(&mut ball, &view, brush, at, 90, &partners);
                let (peak, over) = (held.peak(), held.over(&ceiling));
                let report = format!(
                    "{}, held {a} and {b} radii from the planes, strength {strength}: peak {:.3} of the set height, \
                     {:.3} on the line (one copy alone {:.3}), {:.3} over one copy at worst, pair {:.1e}",
                    corner.name(),
                    peak / set,
                    held.by[on_the_line] / set,
                    ceiling[on_the_line] / set,
                    over / set,
                    held.pair
                );
                println!("{report}");
                failures.check(peak >= 0.97 * set, "short of the set height", &report);
                failures.check(peak <= (1.0 + IN_TURN) * set, "past the set height", &report);
                failures.check(held.by[on_the_line] <= (1.0 + IN_TURN) * set, "past the set height on the line", &report);
                failures.check(held.pair <= Tolerance::OTHERS.allows(peak), "no longer mirrored", &report);
            }
        }
    }
    failures.none();
}

/// Three planes at once, all eight copies on one patch. Planes through the
/// middle of a ball cannot do that to its surface (the point they share is
/// inside it), but planes fixed in the world can: the ball stands so that the
/// world's centre is a point of its surface, none of the three planes square
/// to it. A dab by that point has its eight copies all round it, and counts
/// as about five dabs a quarter radius from each plane, so the middle came up
/// half as high again as the height.
#[test]
fn a_layer_dab_where_three_mirror_planes_meet_keeps_to_its_height() {
    let mut ball = Ball::new();
    let mut failures = Failures::default();
    // Out through the surface where the planes meet, and a way along the
    // surface there that leaves all three planes at the same rate.
    let (out, aside) = (Vec3::new(1.0, 2.0, 1.0).normalize(), Vec3::new(1.0, -1.0, 1.0));
    let meet = point_on_ball(out);
    let view = view_from(&ball.mesh, out);
    let from_there = |v: &usize| ball.start[*v].distance(meet);
    let where_they_meet = (0..ball.start.len()).min_by(|a, b| from_there(a).total_cmp(&from_there(b))).unwrap();
    let set = BRUSH_RADIUS * Settings::default().layer_height;
    for radii in [0.0, 0.25, 0.5, 1.0, 4.0] {
        let at = [point_on_ball((meet + aside * (radii * BRUSH_RADIUS)).normalize())];
        for strength in [0.1, 0.5, 1.0] {
            let alone = mark(&mut ball, &view, layer(BRUSH_RADIUS, strength, [false; 3]), &at, &[]);
            let mut sculptor = layer(BRUSH_RADIUS, strength, [true; 3]);
            sculptor.settings.symmetry_world = Some(Placement::new(-meet, Quat::IDENTITY, Vec3::ONE));
            assert_eq!(sculptor.settings.symmetry_group().len(), 8);
            let mirrored = mark(&mut ball, &view, sculptor, &at, &[]);
            let report = format!(
                "three planes meeting on the surface, {radii} radii from each, strength {strength}: \
                 peak {:.3} of the set height ({:.3} with no symmetry), {:.3} where they meet",
                mirrored.peak() / set,
                alone.peak() / set,
                mirrored.by[where_they_meet] / set
            );
            println!("{report}");
            failures.check(alone.peak() <= (1.0 + HELD) * set && alone.peak() > 0.3 * set, "the dab alone", &report);
            failures.check(mirrored.peak() <= (1.0 + HELD) * set, "past the set height", &report);
            failures.check(mirrored.peak() > 0.2 * set, "the dab did next to nothing", &report);
        }
    }
    failures.none();
}

/// Mirrors and turns together. With turns among the copies every one of
/// them, mirror images included, is walked by `radial::Walk`, which holds
/// Layer to its height itself: by the axis with a mirror plane through it,
/// with two, with all three planes, and on the plane across the axis with the
/// copies round it close enough to meet their neighbours.
#[test]
fn layer_keeps_to_its_height_with_mirrors_and_repeats_together() {
    let mut ball = Ball::new();
    let view = view_from_above(&ball.mesh);
    let by_the_axis = [0.0, 0.3, 0.6, 1.0].map(|radii| radii * BRUSH_RADIUS);
    // A quarter radius above the plane across the axis.
    let by_the_plane = [(BALL_RADIUS * BALL_RADIUS - (0.25 * BRUSH_RADIUS).powi(2)).sqrt()];
    let cases: [(Symmetry, &[f32]); 4] = [
        (Symmetry { mirror: [true, false, false], repeats: 4 }, &by_the_axis),
        (Symmetry { mirror: [true, false, true], repeats: 3 }, &by_the_axis),
        (Symmetry { mirror: [true, true, true], repeats: 3 }, &by_the_axis),
        (Symmetry { mirror: [false, true, false], repeats: 32 }, &by_the_plane),
    ];
    for (symmetry, places) in cases {
        for &from_axis in places {
            for strength in [0.5, 1.0] {
                let brush = || layer(BRUSH_RADIUS, strength, [false; 3]);
                let alone = furthest_moved(&mut ball, &view, brush(), None, from_axis);
                let repeated = furthest_moved(&mut ball, &view, brush(), Some(symmetry), from_axis);
                let report = format!(
                    "Layer at strength {strength}, {symmetry:?}, {:.2} radii from the axis: \
                     {alone:.3e} alone and {repeated:.3e} repeated ({:.2}x)",
                    from_axis / BRUSH_RADIUS,
                    repeated / alone
                );
                println!("{report}");
                assert!((0.9..=1.03).contains(&(repeated / alone)), "{report}");
            }
        }
    }
}

/// The other brushes that work toward a place rather than adding a step each
/// dab, by two mirror planes: how high each gets against the same dab with no
/// symmetry. Not a test of anything by itself; it prints what the sharing-out
/// does to them (`cargo test corner_peaks_of_the_other_brushes -- --ignored --nocapture`).
#[test]
#[ignore]
fn corner_peaks_of_the_other_brushes() {
    let mut ball = Ball::new();
    let corner = CORNERS[0];
    let view = corner.view(&ball.mesh);
    // Each with its own falloff, and with the flat-topped one Layer has built in.
    let tips = [
        ("Layer", Brush::Layer, false, false),
        ("Standard", Brush::Standard, false, false),
        ("Standard, flat-topped", Brush::Standard, false, true),
        ("Standard, accumulating", Brush::Standard, true, false),
        ("Inflate", Brush::Inflate, false, false),
        ("Inflate, flat-topped", Brush::Inflate, false, true),
        ("Inflate, accumulating", Brush::Inflate, true, false),
        ("Clay", Brush::Clay, false, false),
        ("Clay, accumulating", Brush::Clay, true, false),
        ("Crease", Brush::Crease, false, false),
    ];
    // A stroke along one plane a quarter radius from it, across the other.
    let stroke: Vec<Vec3> = (0..STROKE_FRAMES)
        .map(|frame| corner.place(0.25, 3.0 * frame as f32 / (STROKE_FRAMES - 1) as f32 - 1.5))
        .collect();
    for (name, brush, accumulate, flat_topped) in tips {
        for (strength, build_up) in [(0.1, 1.0), (0.5, 1.0), (1.0, 1.0), (1.0, 3.0)] {
            let mut made = |mirror: [bool; 3], points: &[Vec3]| {
                let mut sculptor = sculptor_with(brush);
                let tip = sculptor.settings.current_mut();
                (tip.strength, tip.build_up, tip.accumulate) = (strength, build_up, accumulate);
                sculptor.settings.falloff[brush.index()] = flat_topped.then(|| FalloffCurve::preset(FalloffPreset::Plateau));
                sculptor.settings.symmetry = mirror;
                // Without Clay's finishing pass, which is not a dab's doing.
                sculptor.settings.clay_relax = 0.0;
                mark(&mut ball, &view, sculptor, points, &[]).peak()
            };
            let mut against_alone = |points: &[Vec3]| made(corner.mirror, points) / made([false; 3], points);
            let quarter = against_alone(&[corner.place(0.25, 0.25)]);
            let half = against_alone(&[corner.place(0.5, 0.5)]);
            let through = against_alone(&stroke);
            println!(
                "{name:>22}, strength {strength}, build-up {build_up}: a dab a quarter radius from both planes is \
                 {quarter:.2}x what it is alone, half a radius {half:.2}x; a stroke through the corner {through:.2}x"
            );
        }
    }
}

/// What Layer leaves round an axis, with and without mirrors, as a number per
/// stroke: run before and after a change that must leave the radial walk
/// alone bit for bit, and compare the lines
/// (`cargo test layer_fingerprints_round_an_axis -- --ignored --nocapture`).
#[test]
#[ignore]
fn layer_fingerprints_round_an_axis() {
    let mut ball = Ball::new();
    let view = view_from_above(&ball.mesh);
    let cases = [RADIAL_3, RADIAL_12, RADIAL_4_MIRROR_X, Symmetry { mirror: [true, true, true], repeats: 3 }];
    for symmetry in cases {
        for radii_from_axis in [0.0, 0.3, 1.0, 2.5] {
            let mut sculptor = layer(BRUSH_RADIUS, 1.0, [false; 3]);
            symmetry.set(&mut sculptor.settings);
            let points = stroke_points(radii_from_axis * BRUSH_RADIUS, STROKE_FRAMES);
            let delta = stroke_through(&mut ball.mesh, &mut sculptor, &view, &points);
            println!("fingerprint Layer {symmetry:?} at {radii_from_axis}r: {:016x}", fingerprint(&ball.mesh));
            ball.undo(delta);
        }
    }
}
