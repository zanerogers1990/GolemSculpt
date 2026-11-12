//! The Strap tool, checked where there is an answer to check against.

use glam::{Mat4, Vec2, Vec3};
use sculpt_geom::{SymmetryOp, TriMesh};
use sculpt_toolkit::testing::Bench;
use sculpt_toolkit::{ObjectId, Scene, Tool};

use super::tool::{on_sphere, stroke, wait_for_merge};
use crate::testing::{assert_closed, icosphere, shells};
use crate::types::{Kind, Tip, TubeSettings};
use crate::{TubePoint, TubeTool, strap};

/// A sphere of radius 0.5 in front of the camera and the Strap tool brought up on it.
fn bench() -> (Bench, TubeTool, ObjectId) {
    let mut bench = Bench::new();
    let sphere = bench.add("Body", icosphere(4, 0.5, Vec3::ZERO));
    let mut tool = TubeTool::strap();
    bench.enter(&mut tool);
    assert_eq!((tool.info().id, tool.kind()), ("strap", Kind::Strap));
    assert!(tool.on_surface(), "a strap starts on the surface");
    (bench, tool, sphere)
}

/// The strap's surface as Keep Separate would make it, in the world.
fn strap_mesh(tool: &TubeTool, tube: usize) -> TriMesh {
    let live = &tool.tubes[tube];
    let mut mesh = live.strap_mesh(None);
    mesh.transform(&live.frame.frame_to_world());
    mesh
}

/// A world point to a pixel and back, for scripted wrap lines.
fn line_px(bench: &Bench, a: Vec3, b: Vec3) -> (Vec2, Vec2) {
    (bench.px(a), bench.px(b))
}

#[test]
fn every_ring_of_a_strap_has_as_many_points_whatever_its_width() {
    let wide = strap::profile(1.0, 0.2, 0.5).len();
    assert_eq!(strap::profile(0.01, 0.2, 0.5).len(), wide, "a narrow tip");
    assert_eq!(strap::profile(1.0, 0.2, 0.0).len(), wide, "square edges");
    assert_eq!(strap::profile(1.0, 0.2, 1.0).len(), wide, "round edges");
    // Within its width and its thickness, and touching all four sides.
    let ring = strap::profile(1.0, 0.2, 0.5);
    let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    for &(x, y) in &ring {
        lo = lo.min(Vec2::new(x, y));
        hi = hi.max(Vec2::new(x, y));
    }
    assert!((lo - Vec2::new(-1.0, 0.0)).abs().max_element() < 1e-5, "{lo}");
    assert!((hi - Vec2::new(1.0, 0.2)).abs().max_element() < 1e-5, "{hi}");
}

#[test]
fn a_strap_drawn_on_the_model_rests_on_it_and_is_a_closed_surface() {
    let (mut bench, mut tool, _) = bench();
    stroke(&mut bench, &mut tool, &[on_sphere(-0.3, -0.1), on_sphere(0.0, 0.05), on_sphere(0.3, 0.15)]);
    assert_eq!(tool.tube_count(), 1);
    bench.frame(&mut tool);
    let settings = tool.settings();
    let mesh = strap_mesh(&tool, 0);
    assert_closed(&mesh, "an open strap with its ends closed");
    assert_eq!(shells(&mesh), 1);
    // Nowhere inside the body (its flat faces sag in a hair from the true sphere), and lying on it:
    // never further off it than its thickness and gap and a little for the sphere curving away
    // under its width.
    let t = settings.strap_thickness();
    let nearest = mesh.positions.iter().map(|p| p.length()).fold(f32::MAX, f32::min);
    let farthest = mesh.positions.iter().map(|p| p.length()).fold(0.0, f32::max);
    assert!(nearest > 0.5 - 0.001, "inside the body: {nearest}");
    assert!(farthest < 0.5 + 1.2 * t + settings.radius * settings.radius + 0.002, "floating off it: {farthest}");
    // As wide as it was set to be.
    let laid = tool.tubes[0].copies[0].laid.as_ref().unwrap();
    let middle = laid.centre.len() / 2;
    assert!((laid.half_width[middle] - settings.radius).abs() < 1e-5);
    assert!((laid.thickness - t).abs() < 1e-7);
}

#[test]
fn a_stroke_that_comes_back_to_its_start_closes_into_a_loop() {
    let (mut bench, mut tool, _) = bench();
    let round: Vec<Vec3> = (0..=12)
        .map(|i| {
            let angle = i as f32 / 12.0 * std::f32::consts::TAU;
            on_sphere(0.3 * angle.cos(), 0.3 * angle.sin())
        })
        .collect();
    stroke(&mut bench, &mut tool, &round);
    assert_eq!(tool.tube_count(), 1);
    assert!(tool.settings().closed, "came back to its start: {}", tool.status);
    bench.frame(&mut tool);
    let mesh = strap_mesh(&tool, 0);
    // No ends: a ring.
    assert_closed(&mesh, "a closed strap");
    assert_eq!(shells(&mesh), 1);
    // Opened again, it has two ends.
    tool.set_closed(false);
    bench.frame(&mut tool);
    assert_closed(&strap_mesh(&tool, 0), "opened again");
}

#[test]
fn a_wrap_line_across_the_body_lays_a_belt_all_the_way_round() {
    let (mut bench, mut tool, _) = bench();
    tool.set_wrap_mode(true);
    let (from, to) = line_px(&bench, Vec3::new(-0.8, 0.1, 0.0), Vec3::new(0.8, 0.1, 0.0));
    bench.drag(&mut tool, from, to);
    assert_eq!(tool.tube_count(), 1, "{}", tool.status);
    let live = &tool.tubes[0];
    assert!(live.state.settings.closed);
    assert!(live.state.wrap.is_some(), "the line stays on the belt");
    let points = &live.state.points;
    assert!((8..=24).contains(&points.len()), "{}", points.len());
    // Round the sphere at the line's height: the back too.
    assert!(points.iter().all(|p| (p.position.length() - 0.5).abs() < 0.01 && (p.position.y - 0.1).abs() < 0.02));
    assert!(points.iter().any(|p| p.position.z < -0.3), "round the back");
    bench.frame(&mut tool);
    let mesh = strap_mesh(&tool, 0);
    assert_closed(&mesh, "a belt");
    assert_eq!(shells(&mesh), 1);
}

#[test]
fn the_belt_goes_round_the_part_the_line_was_drawn_over_not_the_arm_beside_it() {
    let mut bench = Bench::new();
    let mut body = icosphere(4, 0.5, Vec3::ZERO);
    body.append(&icosphere(3, 0.15, Vec3::new(0.8, 0.0, 0.0)));
    bench.add("Body", body);
    let mut tool = TubeTool::strap();
    bench.enter(&mut tool);
    tool.set_wrap_mode(true);
    // The line crosses the body and stops short of the arm, but the plane it makes cuts both.
    let (from, to) = line_px(&bench, Vec3::new(-0.6, 0.0, 0.0), Vec3::new(0.3, 0.0, 0.4));
    bench.drag(&mut tool, from, to);
    assert_eq!(tool.tube_count(), 1, "{}", tool.status);
    assert!(tool.tubes[0].state.points.iter().all(|p| p.position.x < 0.55), "laid round the arm");
}

#[test]
fn dragging_an_end_of_the_belts_line_tilts_it_and_a_hand_edit_ends_the_line() {
    let (mut bench, mut tool, _) = bench();
    tool.set_wrap_mode(true);
    let (from, to) = line_px(&bench, Vec3::new(-0.8, 0.0, 0.0), Vec3::new(0.8, 0.0, 0.0));
    bench.drag(&mut tool, from, to);
    assert_eq!(tool.tube_count(), 1);
    bench.frame(&mut tool);
    let level = tool.tubes[0].state.points.clone();
    assert!(level.iter().all(|p| p.position.y.abs() < 0.02));

    // The right end of the line raised: the belt tilts up on the right.
    let line = tool.tubes[0].state.wrap.unwrap();
    let right = if line.a.x > line.b.x { line.a } else { line.b };
    let grab = bench.px(right);
    bench.drag(&mut tool, grab, grab + Vec2::new(0.0, -80.0));
    let tilted = &tool.tubes[0].state.points;
    assert!(tool.tubes[0].state.wrap.is_some());
    let high_right = tilted.iter().filter(|p| p.position.x > 0.3).all(|p| p.position.y > 0.03);
    assert!(high_right, "{:?}", tilted.iter().map(|p| p.position).collect::<Vec<_>>());
    // One undo step for the drag.
    assert!(tool.undo());
    assert_eq!(tool.tubes[0].state.points, level);
    assert!(tool.redo());

    // A point dragged by hand: the line goes, the belt is the hand's now.
    tool.select_tube(0, None);
    bench.frame(&mut tool);
    let front = (0..tool.point_count(0))
        .max_by(|&a, &b| tool.tubes[0].state.points[a].position.z.total_cmp(&tool.tubes[0].state.points[b].position.z))
        .unwrap();
    let at = bench.px(tool.handle_world(0, front, 0).unwrap());
    // Off the wrap line's own handles.
    bench.drag(&mut tool, at, at + Vec2::new(4.0, 30.0));
    assert!(tool.tubes[0].state.wrap.is_none(), "{}", tool.status);
}

#[test]
fn a_belt_round_a_mirrored_body_is_made_once_and_a_strap_off_to_one_side_twice() {
    let (mut bench, mut tool, body) = bench();
    bench.call(|cx| cx.set_mirror([true, false, false]));
    assert_eq!(bench.call(|cx| cx.symmetry_of(body).mirror), [true, false, false]);
    tool.set_wrap_mode(true);
    let (from, to) = line_px(&bench, Vec3::new(-0.8, 0.1, 0.0), Vec3::new(0.8, 0.1, 0.0));
    bench.drag(&mut tool, from, to);
    bench.frame(&mut tool);
    assert_eq!(tool.copy_count(0), 2);
    assert!(tool.tubes[0].copies[1].duplicate, "its own mirror image");
    assert_eq!(shells(&tool.tubes[0].strap_mesh(None)), 1, "one belt, not two inside each other");

    tool.set_wrap_mode(false);
    stroke(&mut bench, &mut tool, &[on_sphere(0.15, -0.3), on_sphere(0.3, 0.0), on_sphere(0.2, 0.3)]);
    bench.frame(&mut tool);
    assert!(!tool.tubes[1].copies[1].duplicate);
    assert_eq!(shells(&tool.tubes[1].strap_mesh(None)), 2, "the strap and its mirror image");
}

/// The surface `y = 0` with a groove `depth` deep where `|x| < half`, as the tool feels a model: down
/// from above a place, along the way given.
fn grooved(half: f32, depth: f32) -> impl Fn(Vec3, Vec3, f32) -> Option<(Vec3, Vec3)> {
    move |at: Vec3, _up: Vec3, _reach: f32| {
        let y = if at.x.abs() < half { -depth } else { 0.0 };
        Some((Vec3::new(at.x, y, at.z), Vec3::Y))
    }
}

#[test]
fn a_tight_strap_bridges_a_hollow_and_a_loose_one_follows_it_down() {
    let points = [TubePoint::on_surface(Vec3::new(-1.0, 0.0, 0.0), Vec3::Y), TubePoint::on_surface(Vec3::new(1.0, 0.0, 0.0), Vec3::Y)];
    let underside_at_middle = |tight: f32| {
        let settings = TubeSettings { radius: 0.1, tight, smoothness: 0.0, ..TubeSettings::default() }.strap();
        let ground = grooved(0.3, 0.05);
        let (_, samples, laid) = strap::sample(&points, &settings, &Mat4::IDENTITY, &SymmetryOp::IDENTITY, Some(&ground));
        let middle = samples.iter().enumerate().min_by(|a, b| a.1.center.x.abs().total_cmp(&b.1.center.x.abs())).unwrap().0;
        laid.centre[middle].y + laid.bottom[middle][strap::ACROSS / 2]
    };
    let tight = underside_at_middle(1.0);
    let loose = underside_at_middle(0.0);
    assert!(tight > -0.005, "drawn taut across the groove: {tight}");
    assert!(loose < -0.04, "down in the groove: {loose}");
}

#[test]
fn a_pointed_end_narrows_to_a_tip_and_a_square_one_does_not() {
    let points = [TubePoint::free(Vec3::new(-1.0, 0.0, 0.0)), TubePoint::free(Vec3::new(1.0, 0.0, 0.0))];
    let ends = |tip: Tip| {
        let settings = TubeSettings { radius: 0.1, tip, ..TubeSettings::default() }.strap();
        let (_, _, laid) = strap::sample(&points, &settings, &Mat4::IDENTITY, &SymmetryOp::IDENTITY, None);
        assert_closed(&laid.mesh(None), "a strap in space");
        (laid.half_width[0], laid.half_width[laid.half_width.len() / 2])
    };
    let (square_end, square_middle) = ends(Tip::Square);
    assert!((square_end - square_middle).abs() < 1e-6);
    let (point_end, point_middle) = ends(Tip::Point);
    assert!(point_end < 0.1 * point_middle, "{point_end} {point_middle}");
    let (round_end, _) = ends(Tip::Round);
    assert!(round_end < 0.2 * square_end);
}

#[test]
fn keep_separate_makes_the_strap_as_shown_and_merge_fuses_it_into_the_body() {
    let (mut bench, mut tool, body) = bench();
    stroke(&mut bench, &mut tool, &[on_sphere(-0.3, -0.1), on_sphere(0.3, 0.15)]);
    bench.frame(&mut tool);
    let shown = strap_mesh(&tool, 0);
    bench.call(|cx| tool.keep_separate(cx));
    let objects = bench.doc.objects();
    assert_eq!(objects.len(), 2);
    let kept = bench.doc.mesh(objects[1]).unwrap().to_tri_mesh();
    assert_eq!(kept.positions.len(), shown.positions.len(), "exactly the surface shown");
    assert_eq!(tool.tube_count(), 0);
    // Undo hands it back live.
    assert!(bench.undo_scene());
    bench.frame(&mut tool);
    assert_eq!(tool.tube_count(), 1);

    let before = bench.doc.mesh(body).unwrap().triangle_count();
    bench.call(|cx| tool.merge_all(cx));
    wait_for_merge(&mut bench, &mut tool);
    let merged = bench.doc.mesh(body).unwrap().to_tri_mesh();
    assert_eq!(shells(&merged), 1, "the strap is part of the body: {}", tool.status);
    assert!(merged.triangle_count() > before / 2);
}
