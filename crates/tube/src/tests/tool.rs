//! The tool driven the way the app drives it, against the toolkit's bench.

use std::time::{Duration, Instant};

use glam::{Vec2, Vec3};
use sculpt_geom::TriMesh;
use sculpt_toolkit::testing::{Bench, cube};
use sculpt_toolkit::{Button, Command, Draw, Key, Loss, ObjectId, Placement, Scene, Shape, Symmetry, Tool};

use crate::TubeTool;
use crate::testing::{assert_closed, icosphere, shells, unmatched_across};

/// A sphere of radius 0.5 in front of the camera (which looks down -z from
/// z = 5) and the tool brought up on it.
pub(super) fn bench() -> (Bench, TubeTool, ObjectId) {
    let mut bench = Bench::new();
    let sphere = bench.add("Body", icosphere(4, 0.5, Vec3::ZERO));
    let mut tool = TubeTool::default();
    bench.enter(&mut tool);
    (bench, tool, sphere)
}

/// The point of the sphere's front that is drawn over (x, y).
pub(super) fn on_sphere(x: f32, y: f32) -> Vec3 {
    Vec3::new(x, y, (0.25 - x * x - y * y).max(0.0).sqrt())
}

/// A stroke through world points: pressed at the first, dragged through the
/// rest in small steps, let go at the last.
pub(super) fn stroke(bench: &mut Bench, tool: &mut TubeTool, through: &[Vec3]) {
    let px: Vec<Vec2> = through.iter().map(|&p| bench.px(p)).collect();
    bench.press(tool, px[0], Button::Left);
    for pair in px.windows(2) {
        for step in 1..=10 {
            bench.drag_to(tool, pair[0].lerp(pair[1], step as f32 / 10.0));
        }
    }
    bench.release(tool);
}

/// A horn drawn from the sphere's right shoulder out past its outline, with
/// a bend in it: three or four points.
pub(super) fn draw_horn(bench: &mut Bench, tool: &mut TubeTool) {
    let start = on_sphere(0.3, 0.1);
    let z = start.z;
    stroke(bench, tool, &[start, Vec3::new(0.6, 0.45, z), Vec3::new(1.0, 0.5, z)]);
}

pub(super) fn handle_px(bench: &Bench, tool: &TubeTool, tube: usize, point: usize, copy: usize) -> Vec2 {
    bench.px(tool.handle_world(tube, point, copy).expect("the handle exists"))
}

pub(super) fn preview_mesh(draw: &Draw, index: usize) -> TriMesh {
    let data = draw.previews()[index].mesh.data();
    TriMesh::new(data.positions.clone(), data.indices.clone())
}

pub(super) fn status(bench: &Bench) -> &str {
    bench.status.as_ref().map_or("", |s| s.0.as_str())
}

/// Frames until the merge has landed.
pub(super) fn wait_for_merge(bench: &mut Bench, tool: &mut TubeTool) {
    let start = Instant::now();
    while tool.is_merging() {
        assert!(start.elapsed() < Duration::from_secs(60), "the merge never landed");
        bench.frame(tool);
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn a_stroke_from_the_model_becomes_a_live_tube_growing_out_of_it() {
    let (mut bench, mut tool, body) = bench();
    // Sized to the scene on the way in: neither a thread nor a trunk.
    assert!((tool.settings().radius - 0.06).abs() < 0.002, "{}", tool.settings().radius);
    draw_horn(&mut bench, &mut tool);
    assert_eq!(tool.tube_count(), 1);
    assert_eq!(tool.mergeable_count(), 1, "drawn on the body, so it can be merged into it");
    assert_eq!(tool.active_tube(), Some(0));
    let points = tool.points(0);
    assert!((3..=5).contains(&points.len()), "the stroke was simplified to {} points", points.len());
    // It starts on the surface where the pen went down and stays at that
    // depth: a plane facing the view.
    let start = on_sphere(0.3, 0.1);
    assert!(points[0].position.distance(start) < 0.01, "{:?}", points[0].position);
    assert!(points.iter().all(|p| (p.position.z - start.z).abs() < 0.01 && !p.is_on_surface()));
    assert!(points.last().unwrap().position.distance(Vec3::new(1.0, 0.5, start.z)) < 0.01);
    assert!(status(&bench).starts_with("Tube drawn on Body"), "{}", status(&bench));

    // The live tube is drawn as a closed mesh, with a handle per point.
    let draw = bench.frame(&mut tool);
    assert_eq!(draw.previews().len(), 1);
    let mesh = preview_mesh(&draw, 0);
    assert_closed(&mesh, "the live tube");
    assert_eq!(shells(&mesh), 1);
    let dots = draw.overlay.shapes.iter().filter(|s| matches!(s, Shape::Dot { .. })).count();
    assert_eq!(dots, tool.point_count(0));
    assert_eq!(bench.doc.objects(), vec![body], "nothing has touched the scene yet");
    assert_eq!(bench.doc.undo_steps(), 0);
}

#[test]
fn a_click_is_not_a_tube_and_a_press_on_nothing_is_passed_on() {
    let (mut bench, mut tool, _) = bench();
    bench.click(&mut tool, bench.px(on_sphere(0.1, 0.1)), Button::Left);
    assert_eq!(tool.tube_count(), 0);
    assert_eq!(status(&bench), "Drag further to draw a tube.");
    assert!(bench.frame(&mut tool).previews().is_empty(), "the draft is gone");

    // Off the model: the camera's, so the tool neither takes nor draws.
    let corner = Vec2::new(30.0, 30.0);
    assert!(!bench.press(&mut tool, corner, Button::Left));
    bench.drag_to(&mut tool, Vec2::new(300.0, 120.0));
    bench.release(&mut tool);
    assert_eq!(tool.tube_count(), 0);
    assert!(!bench.press(&mut tool, corner, Button::Right));
    bench.release(&mut tool);
    assert!(!bench.send(&mut tool, sculpt_toolkit::Event::Wheel(1.0)), "the wheel off any point zooms");

    // With Ctrl the same drag draws a tube in mid-air, at the model's depth.
    bench.env.mods.ctrl = true;
    bench.drag(&mut tool, corner, Vec2::new(300.0, 120.0));
    assert_eq!(tool.tube_count(), 1);
    assert!(tool.points(0).iter().all(|p| p.position.z.abs() < 1e-3), "on the plane through the body's centre");
}

#[test]
fn dragging_a_point_moves_it_and_escape_puts_it_back() {
    let (mut bench, mut tool, _) = bench();
    draw_horn(&mut bench, &mut tool);
    let last = tool.point_count(0) - 1;
    let before = tool.points(0)[last];
    let from = handle_px(&bench, &tool, 0, last, 0);
    let to = bench.px(Vec3::new(1.1, 0.0, before.position.z));
    bench.drag(&mut tool, from, to);
    let after = tool.points(0)[last];
    assert!(after.position.distance(Vec3::new(1.1, 0.0, before.position.z)) < 0.01, "{:?}", after.position);
    assert_eq!(tool.selected_point(), Some(last));
    assert_eq!(tool.point_count(0), last + 1, "the press grabbed the point rather than starting a tube");
    // The preview follows.
    let draw = bench.frame(&mut tool);
    let reach = preview_mesh(&draw, 0).positions.iter().map(|p| p.x).fold(f32::MIN, f32::max);
    assert!((reach - (1.1 + tool.settings().radius)).abs() < 0.02, "the tube reaches x = {reach}");

    // Esc mid-drag puts the point back exactly.
    bench.press(&mut tool, bench.px(after.position), Button::Left);
    bench.drag_to(&mut tool, Vec2::new(400.0, 100.0));
    assert!(tool.points(0)[last].position.distance(after.position) > 0.1);
    assert!(bench.key(&mut tool, Key::Escape));
    bench.release(&mut tool);
    assert_eq!(tool.points(0)[last], after);
}

#[test]
fn the_radius_at_a_point_changes_by_its_handle_the_wheel_and_s() {
    let (mut bench, mut tool, _) = bench();
    draw_horn(&mut bench, &mut tool);
    let end = tool.point_count(0) - 1;
    let at = handle_px(&bench, &tool, 0, end, 0);
    bench.click(&mut tool, at, Button::Left);
    assert_eq!(tool.selected_point(), Some(end));
    assert_eq!(tool.selected_point_size(), 1.0);

    // The square handle beside the selected point: twice as far from the
    // point is twice as thick there.
    bench.frame(&mut tool);
    let ring = tool.ring_handle(&bench.env.view).expect("the selected point has a radius handle");
    assert!(ring.at.distance(ring.centre) > 10.0);
    bench.drag(&mut tool, ring.at, ring.centre + (ring.at - ring.centre) * 2.0);
    assert!((tool.selected_point_size() - 2.0).abs() < 0.02, "{}", tool.selected_point_size());
    assert_eq!(tool.point_count(0), end + 1);
    let draw = bench.frame(&mut tool);
    let mesh = preview_mesh(&draw, 0);
    let tip = tool.handle_world(0, end, 0).unwrap();
    let thickness = mesh.positions.iter().filter(|p| (p.x - tip.x).abs() < 0.01).map(|p| p.distance(tip)).fold(0.0, f32::max);
    assert!((thickness - 2.0 * tool.settings().radius).abs() < 0.015, "the tube is {thickness} thick at the point");

    // The wheel over a point, a notch at a time; several notches are one
    // undo step.
    bench.hover(at);
    assert!(bench.send(&mut tool, sculpt_toolkit::Event::Wheel(1.0)));
    assert!(bench.send(&mut tool, sculpt_toolkit::Event::Wheel(1.0)));
    assert!((tool.selected_point_size() - 2.0 * 1.21).abs() < 0.03);
    assert!(bench.send(&mut tool, sculpt_toolkit::Event::Wheel(-1.0)));
    assert!((tool.selected_point_size() - 2.2).abs() < 0.03);
    assert!(bench.undo(&mut tool));
    assert!((tool.points(0)[end].size - 2.0).abs() < 0.02, "one Ctrl+Z takes the whole run of notches back");
    assert!(bench.undo(&mut tool));
    assert_eq!(tool.points(0)[end].size, 1.0);
    assert_eq!(tool.selected_point(), None, "stepping the history lets go of the point");

    // S held, with no button: sideways travel sizes the point under the
    // pointer, doubling every 160 points.
    bench.hover(at);
    assert!(bench.send(&mut tool, sculpt_toolkit::Event::KeyDown(Key::S)));
    bench.hover(at + Vec2::new(160.0, 5.0));
    bench.frame(&mut tool);
    bench.send(&mut tool, sculpt_toolkit::Event::KeyUp(Key::S));
    assert!((tool.selected_point_size() - 2.0).abs() < 0.01, "{}", tool.selected_point_size());
    // With nothing selected it is the tube's radius, then the next tube's.
    assert!(bench.key(&mut tool, Key::Escape) && bench.key(&mut tool, Key::Escape));
    assert_eq!(tool.active_tube(), None);
    let radius = tool.settings().radius;
    bench.hover(Vec2::new(100.0, 500.0));
    bench.send(&mut tool, sculpt_toolkit::Event::KeyDown(Key::S));
    bench.hover(Vec2::new(180.0, 500.0));
    bench.frame(&mut tool);
    bench.send(&mut tool, sculpt_toolkit::Event::KeyUp(Key::S));
    assert!((tool.settings().radius / radius - 2f32.sqrt()).abs() < 0.01);
}

#[test]
fn the_scaffold_has_its_own_undo_and_redo() {
    let (mut bench, mut tool, _) = bench();
    assert!(!Tool::can_undo(&tool, false) && !Tool::can_undo(&tool, true));
    draw_horn(&mut bench, &mut tool);
    let drawn = tool.points(0).to_vec();
    assert_eq!(tool.next_undo_label(), Some("Draw Tube"));

    let last = drawn.len() - 1;
    let at = handle_px(&bench, &tool, 0, last, 0);
    bench.drag(&mut tool, at, bench.px(Vec3::new(1.2, -0.2, drawn[last].position.z)));
    let moved = tool.points(0).to_vec();
    assert_ne!(moved, drawn);
    assert_eq!(tool.next_undo_label(), Some("Move Point"));

    assert!(bench.undo(&mut tool));
    assert_eq!(tool.points(0), drawn.as_slice(), "the point is back where it was");
    assert!(Tool::can_undo(&tool, true));
    assert!(bench.undo(&mut tool));
    assert_eq!(tool.tube_count(), 0, "the stroke is taken back");
    assert!(bench.frame(&mut tool).previews().is_empty());
    // Nothing of the tool's is left: Z now reaches the scene, which has
    // nothing either.
    assert!(!bench.undo(&mut tool));

    assert!(bench.redo(&mut tool));
    assert_eq!(tool.points(0), drawn.as_slice());
    assert!(bench.redo(&mut tool));
    assert_eq!(tool.points(0), moved.as_slice());
    assert!(!bench.redo(&mut tool));
    assert_eq!(bench.doc.undo_steps(), 0, "none of it was a step of the scene's");

    // A new edit ends the redo.
    assert!(bench.undo(&mut tool));
    tool.set_taper(0.5);
    tool.commit_edit();
    assert!(!Tool::can_undo(&tool, true));
    assert_eq!(tool.next_undo_label(), Some("Taper"));
}

#[test]
fn points_are_added_made_sharp_extended_and_deleted() {
    let (mut bench, mut tool, _) = bench();
    let z = on_sphere(0.3, 0.0).z;
    stroke(&mut bench, &mut tool, &[on_sphere(0.3, 0.0), Vec3::new(1.2, 0.0, z)]);
    assert_eq!(tool.point_count(0), 2, "a straight stroke is its two ends");

    // A click on the selected tube's curve adds a point there without
    // moving the tube.
    bench.click(&mut tool, bench.px(Vec3::new(0.75, 0.0, z)), Button::Left);
    assert_eq!(tool.point_count(0), 3);
    assert_eq!(tool.selected_point(), Some(1));
    assert!(tool.points(0)[1].position.distance(Vec3::new(0.75, 0.0, z)) < 0.02, "{:?}", tool.points(0)[1].position);
    assert_eq!(tool.next_undo_label(), Some("Add Point"));

    // Pressing on the curve and dragging adds a point and moves it.
    bench.drag(&mut tool, bench.px(Vec3::new(0.5, 0.0, z)), bench.px(Vec3::new(0.5, 0.3, z)));
    assert_eq!(tool.point_count(0), 4);
    assert!(tool.points(0)[1].position.distance(Vec3::new(0.5, 0.3, z)) < 0.02);

    // Two presses on a point in quick succession: sharp, and again smooth.
    let corner = handle_px(&bench, &tool, 0, 1, 0);
    bench.click(&mut tool, corner, Button::Left);
    bench.click(&mut tool, corner, Button::Left);
    assert!(tool.points(0)[1].sharp && tool.selected_is_sharp());
    let draw = bench.frame(&mut tool);
    assert!(draw.overlay.shapes.iter().any(|s| matches!(s, Shape::Dot { square: true, .. })), "a sharp point is a square");
    // Slowly, they are two selections.
    bench.click(&mut tool, corner, Button::Left);
    bench.env.time += 1.0;
    bench.click(&mut tool, corner, Button::Left);
    assert!(tool.points(0)[1].sharp);
    tool.toggle_selected_sharp();
    assert!(!tool.points(0)[1].sharp);

    // Shift + drag an end pulls a new end out of it.
    let end = handle_px(&bench, &tool, 0, 3, 0);
    bench.env.mods.shift = true;
    bench.drag(&mut tool, end, bench.px(Vec3::new(1.5, 0.4, z)));
    bench.env.mods.shift = false;
    assert_eq!(tool.point_count(0), 5);
    assert!(tool.points(0)[4].position.distance(Vec3::new(1.5, 0.4, z)) < 0.02);
    assert!(tool.points(0)[3].position.distance(Vec3::new(1.2, 0.0, z)) < 0.02, "the old end stayed");
    // Shift on a middle point just moves it.
    bench.env.mods.shift = true;
    let at = handle_px(&bench, &tool, 0, 2, 0);
    bench.drag(&mut tool, at, bench.px(Vec3::new(0.8, -0.2, z)));
    bench.env.mods.shift = false;
    assert_eq!(tool.point_count(0), 5);

    // Right-click deletes the point under the pointer; Delete the selected
    // one, and with none selected the tube.
    let at = handle_px(&bench, &tool, 0, 2, 0);
    assert!(bench.press(&mut tool, at, Button::Right));
    bench.release(&mut tool);
    assert_eq!(tool.point_count(0), 4);
    let at = handle_px(&bench, &tool, 0, 0, 0);
    bench.click(&mut tool, at, Button::Left);
    assert!(bench.key(&mut tool, Key::Delete));
    assert_eq!(tool.point_count(0), 3);
    assert_eq!(tool.selected_point(), None);
    assert!(bench.key(&mut tool, Key::Backspace));
    assert_eq!(tool.tube_count(), 0);
    assert!(bench.key(&mut tool, Key::Delete), "Delete stays the tool's, or it would delete the object");
    assert_eq!(bench.doc.objects().len(), 1);
    // Each was a step of the tool's own.
    assert!(bench.undo(&mut tool) && tool.tube_count() == 1 && tool.point_count(0) == 3);
    assert!(bench.undo(&mut tool) && tool.point_count(0) == 4);
}

#[test]
fn a_tube_left_with_one_point_goes_and_escape_works_outward() {
    let (mut bench, mut tool, _) = bench();
    let z = on_sphere(0.3, 0.0).z;
    stroke(&mut bench, &mut tool, &[on_sphere(0.3, 0.0), Vec3::new(1.2, 0.0, z)]);
    stroke(&mut bench, &mut tool, &[on_sphere(-0.3, 0.0), Vec3::new(-1.2, 0.0, z)]);
    assert_eq!((tool.tube_count(), tool.active_tube()), (2, Some(1)));

    // A click on the other tube selects it; it does not add a point.
    bench.click(&mut tool, bench.px(Vec3::new(0.8, 0.0, z)), Button::Left);
    assert_eq!((tool.active_tube(), tool.point_count(0)), (Some(0), 2));
    let at = handle_px(&bench, &tool, 0, 1, 0);
    bench.click(&mut tool, at, Button::Left);
    assert_eq!(tool.selected_point(), Some(1));

    assert!(bench.key(&mut tool, Key::Escape), "lets go of the point");
    assert_eq!((tool.selected_point(), tool.active_tube()), (None, Some(0)));
    assert!(bench.key(&mut tool, Key::Escape), "then of the tube");
    assert_eq!(tool.active_tube(), None);
    assert!(!bench.key(&mut tool, Key::Escape), "then it is the app's, which puts the tool away");

    let at = handle_px(&bench, &tool, 0, 0, 0);
    assert!(bench.press(&mut tool, at, Button::Right));
    bench.release(&mut tool);
    assert_eq!(tool.tube_count(), 1, "one point is not a tube");
    assert!(tool.points(0)[0].position.x < 0.0, "the other tube is the one left");
}

#[test]
fn settings_edit_the_selected_tube_and_the_next_one() {
    let (mut bench, mut tool, _) = bench();
    tool.set_taper(0.7);
    tool.set_caps(crate::CapStyle::Flat);
    assert!(!Tool::can_undo(&tool, false), "with no tube there is nothing to undo");
    draw_horn(&mut bench, &mut tool);
    assert_eq!(tool.settings().taper, 0.7, "the next tube starts as the panel was set");
    assert_eq!(tool.settings().caps, crate::CapStyle::Flat);

    // A slider is a stream of small changes: one undo step.
    for step in 1..=5 {
        tool.set_radius(0.06 + step as f32 * 0.01);
    }
    bench.env.time += 1.0;
    bench.frame(&mut tool);
    tool.set_smoothness(0.2);
    tool.set_blend(9.0);
    assert_eq!(tool.settings().blend, crate::TubeSettings::MAX_BLEND);
    assert!(bench.undo(&mut tool), "the blend");
    assert!(bench.undo(&mut tool), "the smoothness");
    assert_eq!(tool.settings().smoothness, 0.6);
    assert!((tool.settings().radius - 0.11).abs() < 1e-6);
    assert!(bench.undo(&mut tool), "the radius, all five steps of it");
    assert!((tool.settings().radius - 0.06).abs() < 0.002);
    assert_eq!(tool.tube_count(), 1);

    // Blend and placement do not change the shape: the preview is kept.
    let version = bench.frame(&mut tool).previews()[0].mesh.version();
    tool.set_blend(0.2);
    tool.set_on_surface(true);
    assert_eq!(bench.frame(&mut tool).previews()[0].mesh.version(), version);
    tool.set_flip_taper(true);
    assert_ne!(bench.frame(&mut tool).previews()[0].mesh.version(), version);
}

#[test]
fn on_surface_strokes_stick_to_the_model_half_sunk() {
    let (mut bench, mut tool, _) = bench();
    // Closer in, so the points of a stroke over the model are well apart.
    bench.env.view = sculpt_toolkit::View::perspective(Vec3::new(0.0, 0.0, 2.5), Vec3::ZERO, 60.0, Vec2::new(800.0, 600.0));
    tool.set_on_surface(true);
    stroke(&mut bench, &mut tool, &[on_sphere(-0.35, -0.2), on_sphere(0.0, 0.25), on_sphere(0.35, -0.2)]);
    assert_eq!(tool.tube_count(), 1);
    let radius = tool.settings().radius;
    for (i, point) in tool.points(0).iter().enumerate() {
        assert!(point.is_on_surface());
        assert!((point.position.length() - 0.5).abs() < 0.01, "point {i} is on the sphere");
        assert!(point.surface_normal.dot(point.position.normalize()) > 0.98, "its normal is the sphere's");
        // The centre line rides half a radius above the surface.
        let handle = tool.handle_world(0, i, 0).unwrap();
        assert!((handle.length() - (0.5 + radius * 0.5)).abs() < 0.01, "handle {i} at {}", handle.length());
    }
    // A dragged point slides over the surface, and off the model floats.
    let from = handle_px(&bench, &tool, 0, 0, 0);
    bench.drag(&mut tool, from, bench.px(on_sphere(-0.2, 0.3)));
    assert!(tool.points(0)[0].is_on_surface() && (tool.points(0)[0].position.length() - 0.5).abs() < 0.01);
    let from = handle_px(&bench, &tool, 0, 0, 0);
    bench.drag(&mut tool, from, Vec2::new(60.0, 60.0));
    assert!(!tool.points(0)[0].is_on_surface());
    // A point added between two surface points is a surface point. Between
    // the two that are farthest apart on screen, to be clear of both.
    let count = tool.point_count(0);
    let gap = |i: usize| handle_px(&bench, &tool, 0, i, 0).distance(handle_px(&bench, &tool, 0, i + 1, 0));
    let widest = (1..count - 1).max_by(|&a, &b| gap(a).total_cmp(&gap(b))).unwrap();
    assert!(gap(widest) > 24.0, "points {widest} and the next are {} pixels apart", gap(widest));
    let between = (tool.handle_world(0, widest, 0).unwrap() + tool.handle_world(0, widest + 1, 0).unwrap()) * 0.5;
    bench.click(&mut tool, bench.px(between), Button::Left);
    assert_eq!(tool.point_count(0), count + 1);
    assert_eq!(tool.selected_point(), Some(widest + 1));
    let added = tool.points(0)[widest + 1];
    assert!(added.is_on_surface() && (added.position.length() - 0.5).abs() < 0.02, "{added:?}");
}

#[test]
fn mirror_axes_make_a_pair_and_a_copy_edits_the_tube_through_the_mirror() {
    let (mut bench, mut tool, _) = bench();
    bench.env.mirror = [true, false, false];
    draw_horn(&mut bench, &mut tool);
    assert_eq!((tool.tube_count(), tool.copy_count(0)), (1, 2));
    let draw = bench.frame(&mut tool);
    let mesh = preview_mesh(&draw, 0);
    assert_closed(&mesh, "the mirrored pair");
    assert_eq!(shells(&mesh), 2);
    assert_eq!(unmatched_across(&mesh, 0).0, 0, "the pair is exactly symmetric");

    // Grabbing the mirror image's end moves the tube's own end.
    let last = tool.point_count(0) - 1;
    let z = tool.points(0)[last].position.z;
    let image = handle_px(&bench, &tool, 0, last, 1);
    assert!(image.x < 400.0, "the copy's handle is on the other side of the screen");
    bench.drag(&mut tool, image, bench.px(Vec3::new(-1.2, 0.1, z)));
    assert!(tool.points(0)[last].position.distance(Vec3::new(1.2, 0.1, z)) < 0.01, "{:?}", tool.points(0)[last].position);

    // Mirror off: one tube again, with no further edit.
    bench.env.mirror = [false; 3];
    let draw = bench.frame(&mut tool);
    assert_eq!(tool.copy_count(0), 1);
    assert_eq!(shells(&preview_mesh(&draw, 0)), 1);
}

/// Symmetry belongs to objects, and a tube takes that of the object it is
/// drawn on, whichever object is selected: the Unity app's tubes took their
/// anchor's. Kept separate, the new object is sculpted the way that object is.
#[test]
fn a_tube_is_made_under_the_symmetry_of_the_object_it_is_drawn_on() {
    let (mut bench, mut tool, body) = bench();
    bench.env.mirror = [true, false, false];
    // A second object, out of the way, selected, with no symmetry of its own.
    let other = bench.add("Other", cube(0.1));
    let aside = Placement::at(Vec3::new(0.0, 1.4, 0.0));
    bench.call(|cx| cx.commit("Place", vec![Command::Place { object: other, placement: aside }]));
    assert_eq!((bench.doc.active(), bench.env.mirror), (other, [false; 3]));

    // Drawn on the body, it is made under the body's Mirror X.
    draw_horn(&mut bench, &mut tool);
    assert_eq!(tool.copy_count(0), 2, "the body's own symmetry, not the selected object's");
    // Changing the selected object's symmetry changes nothing for it...
    bench.env.mirror = [false, true, true];
    bench.frame(&mut tool);
    assert_eq!(tool.copy_count(0), 2);
    // ...and changing the body's does.
    bench.set_symmetry(body, Symmetry { radial_count: 4, ..Symmetry::NONE });
    bench.frame(&mut tool);
    assert_eq!(tool.copy_count(0), 4);

    bench.call(|cx| tool.keep_separate(cx));
    let made = bench.doc.active();
    assert_eq!(bench.doc.info(made).unwrap().name, "Tube");
    assert_eq!(bench.doc.symmetry(made), bench.doc.symmetry(body), "the kept tube takes the body's settings");
    assert_eq!(bench.doc.symmetry(other).mirror, [false, true, true], "and the object that was selected keeps its own");
}

/// A symmetry fixed in the world, on an object that has been moved, turned
/// and resized: the tube's copy is its mirror image across the world's plane,
/// not across the object's own.
#[test]
fn a_tube_on_an_object_with_a_world_symmetry_is_mirrored_across_the_worlds_plane() {
    let (mut bench, mut tool, body) = bench();
    let (position, rotation) = (Vec3::new(0.3, -0.1, 0.0), glam::Quat::from_rotation_z(0.4));
    let placement = Placement { position, rotation, scale: Vec3::splat(1.2) };
    bench.call(|cx| cx.commit("Place", vec![Command::Place { object: body, placement }]));
    bench.env.mirror = [true, false, false];
    bench.env.symmetry_world = Some(placement);
    let start = placement.matrix().transform_point3(on_sphere(0.2, 0.1));
    stroke(&mut bench, &mut tool, &[start, start + Vec3::new(0.6, 0.3, 0.0), start + Vec3::new(1.0, 0.3, 0.0)]);
    assert_eq!(tool.copy_count(0), 2);
    for point in 0..tool.points(0).len() {
        let (own, copy) = (tool.handle_world(0, point, 0).unwrap(), tool.handle_world(0, point, 1).unwrap());
        assert!(own.x > 0.2, "the tube is drawn to the right of the world's plane: {own}");
        assert!(copy.distance(own * Vec3::new(-1.0, 1.0, 1.0)) < 1e-4, "point {point}: {copy} is not {own} mirrored");
    }
    // The object moves on, the plane stays: the copy is still the world's mirror image.
    let moved = Placement { position: Vec3::new(0.8, 0.2, 0.1), ..placement };
    bench.call(|cx| cx.commit("Place", vec![Command::Place { object: body, placement: moved }]));
    bench.env.symmetry_world = Some(moved);
    bench.frame(&mut tool);
    let (own, copy) = (tool.handle_world(0, 0, 0).unwrap(), tool.handle_world(0, 0, 1).unwrap());
    assert!(own.x > 0.7 && copy.distance(own * Vec3::new(-1.0, 1.0, 1.0)) < 1e-4, "{own} and {copy}");
}

#[test]
fn keep_separate_is_one_undo_step_that_hands_the_tube_back() {
    let (mut bench, mut tool, body) = bench();
    bench.env.mirror = [true, false, false];
    draw_horn(&mut bench, &mut tool);
    let kept = tool.points(0).to_vec();
    bench.call(|cx| tool.keep_separate(cx));
    assert_eq!(tool.tube_count(), 0);
    assert_eq!(bench.doc.objects().len(), 2);
    assert_eq!(bench.doc.undo_steps(), 1);
    let made = bench.doc.active_info().unwrap();
    assert_eq!(made.name, "Tube");
    assert_eq!(made.placement, Placement::IDENTITY, "it stands where the body does");
    let mesh = bench.doc.active_mesh().unwrap().to_tri_mesh();
    assert_closed(&mesh, "the kept tube");
    assert_eq!(shells(&mesh), 2, "both of the pair, in one object");
    assert_eq!(unmatched_across(&mesh, 0).0, 0);
    assert!(status(&bench).starts_with("Made 1 tube object"), "{}", status(&bench));
    assert!(bench.frame(&mut tool).previews().is_empty());
    assert!(!Tool::can_undo(&tool, false), "the tool's own history went with the tube");

    // Ctrl+Z reaches the scene, the object goes, and the tube is live again.
    assert!(bench.undo(&mut tool));
    assert_eq!(bench.doc.objects(), vec![body]);
    bench.frame(&mut tool);
    assert_eq!(tool.tube_count(), 1);
    assert_eq!(tool.points(0), kept.as_slice());
    assert!(status(&bench).contains("live again"), "{}", status(&bench));
    // Ctrl+Shift+Z makes the object again and takes the tube away.
    assert!(bench.redo(&mut tool));
    bench.frame(&mut tool);
    assert_eq!((tool.tube_count(), bench.doc.objects().len()), (0, 2));
    // An object deleted by hand once the scene has moved on is not an undo:
    // its tube stays gone.
    bench.add("Extra", cube(0.1));
    let made = bench.doc.objects()[1];
    bench.call(|cx| cx.commit("Delete", vec![Command::Delete { object: made }]));
    bench.frame(&mut tool);
    assert_eq!(tool.tube_count(), 0);
    assert!(bench.undo_scene(), "the deletion");
    // And back once more, to be edited and kept again as a new step.
    assert!(bench.undo(&mut tool));
    bench.frame(&mut tool);
    tool.set_radius(0.1);
    bench.call(|cx| tool.keep_separate(cx));
    assert_eq!((tool.tube_count(), bench.doc.objects().len(), bench.doc.undo_steps()), (0, 2, 1));
}

#[test]
fn a_kept_tube_stands_with_its_anchor_at_its_own_size() {
    let (mut bench, mut tool, body) = bench();
    let placement = Placement { position: Vec3::new(0.2, -0.1, 0.0), rotation: glam::Quat::from_rotation_z(0.4), scale: Vec3::splat(1.5) };
    bench.call(|cx| cx.commit("Place", vec![Command::Place { object: body, placement }]));
    let before = bench.doc.undo_steps();
    let start = placement.matrix().transform_point3(on_sphere(0.2, 0.1));
    stroke(&mut bench, &mut tool, &[start, start + Vec3::new(0.6, 0.3, 0.0), start + Vec3::new(1.0, 0.3, 0.0)]);
    assert_eq!(tool.tube_count(), 1);
    // The points are kept in the body's own space.
    assert!(tool.points(0)[0].position.distance(on_sphere(0.2, 0.1)) < 0.01, "{:?}", tool.points(0)[0].position);
    let handle = tool.handle_world(0, 0, 0).unwrap();
    assert!(handle.distance(start) < 0.01);
    let radius = tool.settings().radius;
    let live = preview_mesh(&bench.frame(&mut tool), 0);

    bench.call(|cx| tool.keep_separate(cx));
    assert_eq!(bench.doc.undo_steps(), before + 1);
    let made = bench.doc.active_info().unwrap();
    assert_eq!(made.placement, Placement { scale: Vec3::ONE, ..placement });
    // Shown through its placement, the object is where the live tube was,
    // and as thick: the anchor's scale did not fatten it.
    let mut mesh = bench.doc.active_mesh().unwrap().to_tri_mesh();
    mesh.transform(&made.placement.matrix());
    assert!((mesh.signed_volume() / live.signed_volume() - 1.0).abs() < 0.08);
    assert!(mesh.bounds().center().distance(live.bounds().center()) < 0.01);
    let thickness = mesh.positions.iter().map(|p| p.distance(start)).fold(f32::MAX, f32::min);
    assert!((thickness - radius).abs() < 0.01, "{thickness} thick for a radius of {radius}");
}

#[test]
fn merge_runs_in_the_background_and_is_one_undo_step_that_hands_the_tube_back() {
    let (mut bench, mut tool, body) = bench();
    draw_horn(&mut bench, &mut tool);
    let drawn = tool.points(0).to_vec();
    let before = bench.doc.info(body).unwrap().mesh;
    let sphere = bench.doc.mesh(body).unwrap().to_tri_mesh();

    assert!(bench.key(&mut tool, Key::Enter));
    assert!(tool.is_merging());
    assert_eq!(bench.doc.undo_steps(), 0, "nothing changes until the merge lands");
    // The tubes hold still meanwhile: presses pass, and Z is the scene's.
    let at = handle_px(&bench, &tool, 0, 0, 0);
    assert!(!bench.press(&mut tool, at, Button::Left));
    bench.release(&mut tool);
    assert!(!bench.send(&mut tool, sculpt_toolkit::Event::Undo));
    wait_for_merge(&mut bench, &mut tool);

    assert_eq!(tool.tube_count(), 0);
    assert_eq!(bench.doc.objects(), vec![body]);
    assert_eq!(bench.doc.undo_steps(), 1);
    assert_ne!(bench.doc.info(body).unwrap().mesh, before);
    let merged = bench.doc.mesh(body).unwrap().to_tri_mesh();
    assert_closed(&merged, "the body with its horn");
    assert_eq!(shells(&merged), 1);
    assert!(merged.signed_volume() > sphere.signed_volume() * 1.01);
    let reach = merged.positions.iter().map(|p| p.x).fold(f32::MIN, f32::max);
    assert!((reach - (1.0 + 0.06)).abs() < 0.03, "the horn reaches x = {reach}");
    assert!(status(&bench).starts_with("Merged 1 tube in"), "{}", status(&bench));
    assert!(bench.status.as_ref().unwrap().1);

    // One Z un-merges and hands the tube back live, bringing the tool up.
    bench.leave(&mut tool);
    bench.env.active = false;
    assert!(bench.undo_scene());
    assert_eq!(bench.doc.info(body).unwrap().mesh, before);
    bench.started = false;
    bench.frame(&mut tool);
    assert!(bench.started, "the tool asks to be brought up to show the tube");
    bench.env.active = true;
    bench.enter(&mut tool);
    assert_eq!(tool.tube_count(), 1);
    assert_eq!(tool.points(0), drawn.as_slice());
    assert_eq!(bench.frame(&mut tool).previews().len(), 1);
    // Ctrl+Shift+Z merges it again: the tube goes, with nothing recomputed.
    assert!(bench.redo(&mut tool));
    bench.frame(&mut tool);
    assert_eq!(tool.tube_count(), 0);
    assert_eq!(bench.doc.mesh(body).unwrap().to_tri_mesh(), merged);
}

#[test]
fn a_cancelled_merge_and_a_changed_model_leave_the_tubes_live() {
    let (mut bench, mut tool, body) = bench();
    draw_horn(&mut bench, &mut tool);
    bench.call(|cx| tool.merge_all(cx));
    assert!(bench.key(&mut tool, Key::Escape), "Esc stops the merge");
    wait_for_merge(&mut bench, &mut tool);
    assert_eq!((tool.tube_count(), bench.doc.undo_steps()), (1, 0));
    assert!(status(&bench).contains("cancelled"), "{}", status(&bench));

    // The model replaced while the merge ran: the result is for a mesh
    // that is no longer there.
    bench.call(|cx| tool.merge_all(cx));
    bench.call(|cx| cx.commit("Remesh", vec![Command::Replace { object: body, mesh: icosphere(3, 0.5, Vec3::ZERO).into() }]));
    wait_for_merge(&mut bench, &mut tool);
    assert_eq!((tool.tube_count(), bench.doc.undo_steps()), (1, 1));
    assert!(status(&bench).contains("changed while"), "{}", status(&bench));
    assert!(!bench.status.as_ref().unwrap().1);
}

#[test]
fn tubes_with_nothing_to_merge_into_say_so() {
    let mut bench = Bench::new();
    let body = bench.add("Body", cube(0.5));
    let mut tool = TubeTool::default();
    bench.enter(&mut tool);
    assert!(bench.key(&mut tool, Key::Enter));
    assert_eq!(status(&bench), "Draw a tube first.");
    // The only object hidden: a mid-air stroke has no anchor.
    bench.call(|cx| cx.commit("Hide", vec![Command::Show { object: body, visible: false }]));
    bench.env.mods.ctrl = true;
    bench.drag(&mut tool, Vec2::new(200.0, 300.0), Vec2::new(500.0, 250.0));
    bench.env.mods.ctrl = false;
    assert_eq!((tool.tube_count(), tool.mergeable_count()), (1, 0));
    assert!(bench.key(&mut tool, Key::Enter));
    assert!(!tool.is_merging());
    assert!(status(&bench).contains("Keep Separate"), "{}", status(&bench));
    // Kept, it is centred on itself.
    bench.call(|cx| tool.keep_separate(cx));
    let made = bench.doc.active_info().unwrap();
    assert!(made.placement.position.length() > 0.01);
    let mesh = bench.doc.active_mesh().unwrap().to_tri_mesh();
    assert!(mesh.bounds().center().length() < 1e-4);
    assert_closed(&mesh, "the loose tube");
}

#[test]
fn a_tube_goes_with_the_object_it_was_drawn_on() {
    let (mut bench, mut tool, body) = bench();
    let other = bench.add("Other", cube(0.1));
    bench.call(|cx| cx.commit("Move", vec![Command::Place { object: other, placement: Placement::at(Vec3::new(0.0, 1.5, 0.0)) }]));
    draw_horn(&mut bench, &mut tool);
    assert_eq!(tool.tube_count(), 1);
    // It rides along when its anchor moves.
    let at = tool.handle_world(0, 0, 0).unwrap();
    bench.call(|cx| cx.commit("Move", vec![Command::Place { object: body, placement: Placement::at(Vec3::new(0.0, -0.5, 0.0)) }]));
    bench.frame(&mut tool);
    assert!(tool.handle_world(0, 0, 0).unwrap().distance(at + Vec3::new(0.0, -0.5, 0.0)) < 1e-5);
    bench.call(|cx| cx.commit("Delete", vec![Command::Delete { object: body }]));
    bench.frame(&mut tool);
    assert_eq!(tool.tube_count(), 0, "it has nowhere to live");
}

#[test]
fn only_the_tube_shows_in_a_clean_view_and_nothing_while_put_away() {
    let (mut bench, mut tool, _) = bench();
    draw_horn(&mut bench, &mut tool);
    let draw = bench.frame(&mut tool);
    assert!(!draw.overlay.shapes.is_empty() && draw.previews().len() == 1);
    bench.env.clean_view = true;
    let draw = bench.frame(&mut tool);
    assert!(draw.overlay.shapes.is_empty() && draw.previews().len() == 1);
    let at = handle_px(&bench, &tool, 0, 0, 0);
    assert!(!bench.press(&mut tool, at, Button::Left), "no handles to press");
    bench.release(&mut tool);
    bench.env.clean_view = false;
    bench.leave(&mut tool);
    bench.env.active = false;
    assert!(bench.frame(&mut tool).is_empty());
    bench.env.active = true;
    bench.enter(&mut tool);
    assert_eq!(bench.frame(&mut tool).previews().len(), 1, "the tube is still there next time");
}

#[test]
fn hovering_lights_what_a_press_would_take() {
    let (mut bench, mut tool, _) = bench();
    draw_horn(&mut bench, &mut tool);
    let rings = |draw: &Draw| draw.overlay.shapes.iter().filter(|s| matches!(s, Shape::Line { closed: true, .. })).count();
    // Over nothing: the dashed ring of the next stroke's radius.
    bench.hover(bench.px(on_sphere(-0.2, -0.2)));
    let draw = bench.frame(&mut tool);
    assert_eq!(rings(&draw), 1);
    assert!(draw.overlay.shapes.iter().any(|s| matches!(s, Shape::Line { closed: true, dash: Some(_), .. })));
    // Over a point: its thickness ring instead.
    bench.hover(handle_px(&bench, &tool, 0, 1, 0));
    let draw = bench.frame(&mut tool);
    assert_eq!(rings(&draw), 1);
    assert!(draw.overlay.shapes.iter().all(|s| !matches!(s, Shape::Line { dash: Some(_), .. })));
    // Over the curve between points: the dot where a click would add one.
    let between = (tool.handle_world(0, 0, 0).unwrap() + tool.handle_world(0, 1, 0).unwrap()) * 0.5;
    bench.hover(bench.px(between));
    let dots = |draw: &Draw| draw.overlay.shapes.iter().filter(|s| matches!(s, Shape::Dot { .. })).count();
    assert_eq!(dots(&bench.frame(&mut tool)), tool.point_count(0) + 1);
    // Not while the pointer is over a panel.
    bench.env.hovering = false;
    assert_eq!(dots(&bench.frame(&mut tool)), tool.point_count(0));
}

#[test]
fn the_panel_builds_and_the_demos_run() {
    for what in ["", "mirror", "radial", "radial-mirror", "surface", "object", "merged", "radial-merged"] {
        let (mut bench, mut tool, body) = bench();
        let before = bench.doc.info(body).unwrap().mesh;
        bench.call(|cx| tool.demo(cx, what));
        match what {
            "object" => assert_eq!((tool.tube_count(), bench.doc.objects().len()), (0, 2)),
            "merged" | "radial-merged" => {
                assert_eq!(tool.tube_count(), 0);
                assert_ne!(bench.doc.info(body).unwrap().mesh, before);
                assert_eq!(bench.env.radial_count, if what == "radial-merged" { 6 } else { 1 });
            }
            _ => {
                assert_eq!(tool.tube_count(), 1, "demo {what:?}");
                assert_eq!(bench.env.mirror[0], what == "mirror" || what == "radial-mirror");
                let repeats = match what {
                    "radial" => 6,
                    "radial-mirror" => 3,
                    _ => 1,
                };
                assert_eq!(bench.env.radial_count, repeats);
                assert_eq!(tool.on_surface(), what == "surface");
                assert!(tool.selected_point().is_some() && tool.selected_point_size() > 1.5);
                let draw = bench.frame(&mut tool);
                let pieces = match what {
                    "mirror" => 2,
                    "radial" => 6,
                    "radial-mirror" => 6,
                    _ => 1,
                };
                assert_eq!(tool.copy_count(0), pieces);
                assert_eq!(shells(&preview_mesh(&draw, 0)), pieces, "demo {what:?}");
            }
        }
        assert_eq!(bench.doc.undo_steps(), usize::from(matches!(what, "object" | "merged" | "radial-merged")));

        let ctx = egui::Context::default();
        sculpt_toolkit::theme::install(&ctx);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            bench.call(|cx| Tool::panel(&mut tool, ui, cx));
        });
        assert!(!output.shapes.is_empty(), "the panel drew something");
        output.textures_delta.clear();
    }
}

/// The back of the body masked and hidden: out of the way of a stroke on the
/// front, which a hidden face would not let the pointer hit. Returns how
/// many vertices and triangles the body has.
fn mask_and_hide_the_back(bench: &mut Bench, body: ObjectId) -> (usize, usize) {
    let mesh = bench.doc.mesh(body).unwrap().to_tri_mesh();
    let at_back = |t: usize| mesh.indices[3 * t..3 * t + 3].iter().all(|&i| mesh.positions[i as usize].z < -0.1);
    let hidden: Vec<u32> = (0..mesh.triangle_count()).filter(|&t| at_back(t)).map(|t| t as u32).collect();
    let mask: Vec<f32> = mesh.positions.iter().map(|p| if p.z < -0.1 { 1.0 } else { 0.0 }).collect();
    assert!(!hidden.is_empty() && mask.contains(&1.0));
    let receipt = bench.call(|cx| {
        cx.commit("Mask", vec![Command::Mask { object: body, mask }]);
        cx.commit("Hide", vec![Command::HideFaces { object: body, triangles: hidden, hide: true }])
    });
    assert!(receipt.changed);
    (mesh.vertex_count(), mesh.triangle_count())
}

/// What the Unity app did (`SculptableMesh.ReplaceGeometry`, which its tube
/// merge ended in, as Remesh does): a merged model starts with a clear mask
/// and everything showing, because the new surface has no mapping onto the
/// old one's mask or faces.
#[test]
fn a_merge_clears_the_mask_and_shows_every_face_as_a_remesh_does() {
    let (mut bench, mut tool, body) = bench();
    let (vertices, triangles) = mask_and_hide_the_back(&mut bench, body);
    let view = bench.doc.mesh(body).unwrap();
    assert!(view.mask.iter().sum::<f32>() > 0.0 && view.hidden_triangles.is_some());
    assert!(view.visible_triangle_count() < triangles);
    assert_eq!(view.mask.len(), vertices);

    draw_horn(&mut bench, &mut tool);
    assert!(bench.key(&mut tool, Key::Enter));
    wait_for_merge(&mut bench, &mut tool);
    let merged = bench.doc.mesh(body).unwrap();
    assert_ne!(merged.vertex_count(), vertices, "the model was rebuilt");
    assert_eq!(merged.mask.len(), merged.vertex_count());
    assert_eq!(merged.mask.iter().sum::<f32>(), 0.0, "the mask is clear");
    assert!(merged.hidden_triangles.is_none() && merged.hidden_vertices.is_none(), "nothing is hidden");
    assert_eq!(merged.visible_triangle_count(), merged.triangle_count());
}

/// Keep Separate leaves the body it was drawn on alone, as it did in Unity
/// (the new objects are built from the tubes alone): its mask and hidden
/// faces stay as they were, and the tube objects start clear.
#[test]
fn keep_separate_leaves_the_bodys_mask_and_hidden_faces_and_the_tube_starts_clear() {
    let (mut bench, mut tool, body) = bench();
    mask_and_hide_the_back(&mut bench, body);
    let (mask, hidden) = {
        let view = bench.doc.mesh(body).unwrap();
        (view.mask.to_vec(), view.hidden_triangles.unwrap().to_vec())
    };
    draw_horn(&mut bench, &mut tool);
    bench.call(|cx| tool.keep_separate(cx));
    assert_eq!(bench.doc.objects().len(), 2);

    let body_view = bench.doc.mesh(body).unwrap();
    assert_eq!(body_view.mask, mask.as_slice(), "the body keeps its mask");
    assert_eq!(body_view.hidden_triangles, Some(hidden.as_slice()), "and its hidden faces");
    let kept = bench.doc.active_mesh().unwrap();
    assert_eq!(kept.mask.iter().sum::<f32>(), 0.0);
    assert!(kept.hidden_triangles.is_none());
}

/// What the app asks before it closes, and before a file takes the scene's place.
fn at_risk(tool: &TubeTool, loss: Loss) -> Option<String> {
    Tool::scaffold_at_risk(tool, loss)
}

#[test]
fn live_tubes_are_work_that_closing_would_lose_until_they_are_merged_kept_or_deleted() {
    let (mut bench, mut tool, _) = bench();
    for loss in [Loss::Closing, Loss::SceneReplaced] {
        assert_eq!(at_risk(&tool, loss), None, "the tool up with nothing drawn has nothing to lose");
    }

    draw_horn(&mut bench, &mut tool);
    let one = Some("1 unmerged tube (Merge or Keep Separate first)".to_owned());
    assert_eq!(at_risk(&tool, Loss::Closing), one);
    assert_eq!(at_risk(&tool, Loss::SceneReplaced), one, "it stands on the body, which a new scene takes away");
    // Put away, the tubes wait out of sight: they are still there to lose.
    bench.leave(&mut tool);
    assert_eq!(at_risk(&tool, Loss::Closing), one);
    bench.enter(&mut tool);

    let start = on_sphere(-0.3, 0.1);
    stroke(&mut bench, &mut tool, &[start, Vec3::new(-0.6, 0.45, start.z), Vec3::new(-1.0, 0.5, start.z)]);
    let two = Some("2 unmerged tubes (Merge or Keep Separate first)".to_owned());
    assert_eq!(at_risk(&tool, Loss::Closing), two);

    // Deleted: one fewer. Kept separate: none, they are objects of the scene now.
    tool.delete_active_tube();
    assert_eq!(at_risk(&tool, Loss::Closing), one);
    bench.call(|cx| tool.keep_separate(cx));
    for loss in [Loss::Closing, Loss::SceneReplaced] {
        assert_eq!(at_risk(&tool, loss), None, "kept separate, the tube is in the scene and in its file");
    }
    // Z hands the tube back live, and with it the work.
    assert!(bench.undo(&mut tool));
    bench.frame(&mut tool);
    assert_eq!(at_risk(&tool, Loss::Closing), one);

    // Merged: none.
    assert!(bench.key(&mut tool, Key::Enter));
    wait_for_merge(&mut bench, &mut tool);
    for loss in [Loss::Closing, Loss::SceneReplaced] {
        assert_eq!(at_risk(&tool, loss), None, "merged, the tube is part of the body");
    }
}

#[test]
fn a_tube_drawn_on_nothing_is_lost_by_closing_but_not_by_a_new_scene() {
    let mut bench = Bench::new();
    let body = bench.add("Body", cube(0.5));
    let mut tool = TubeTool::default();
    bench.enter(&mut tool);
    // The only object hidden: a mid-air stroke has no anchor.
    bench.call(|cx| cx.commit("Hide", vec![Command::Show { object: body, visible: false }]));
    bench.env.mods.ctrl = true;
    bench.drag(&mut tool, Vec2::new(200.0, 300.0), Vec2::new(500.0, 250.0));
    bench.env.mods.ctrl = false;
    assert_eq!((tool.tube_count(), tool.mergeable_count()), (1, 0));
    assert!(at_risk(&tool, Loss::Closing).is_some());
    assert_eq!(at_risk(&tool, Loss::SceneReplaced), None, "it stands in the world, whatever scene is open");
}

/// What Open does to live tubes, which is what `Loss::SceneReplaced` has to tell the truth about.
#[test]
fn a_scene_opened_in_place_of_this_one_takes_the_tubes_drawn_on_its_objects() {
    let (mut bench, mut tool, _) = bench();
    draw_horn(&mut bench, &mut tool);
    assert!(at_risk(&tool, Loss::SceneReplaced).is_some());
    let ids = bench.open(vec![("Opened", cube(0.5))]);
    bench.load_tool(&mut tool, &ids, vec![None], sculpt_toolkit::Arrival::Replaced);
    bench.frame(&mut tool);
    assert_eq!(tool.tube_count(), 0, "the body went, and its tube with it");
    // Undo brings the body back, but not the tube.
    assert!(bench.undo_scene());
    bench.frame(&mut tool);
    assert_eq!(tool.tube_count(), 0);
}

/// The same with the tool put away, when its frame does not look for objects
/// that have gone: the tubes must go all the same, or the question that said
/// they would be lost was wrong, closing the new scene would name tubes that
/// are nowhere in it, and Undo would bring them back on some days only.
#[test]
fn a_scene_put_in_place_of_this_one_takes_those_tubes_with_the_tool_put_away_too() {
    let (mut bench, mut tool, body) = bench();
    draw_horn(&mut bench, &mut tool);
    // One more in mid-air, which stands in the world (the body hidden, so the stroke lands on nothing).
    bench.call(|cx| cx.commit("Hide", vec![Command::Show { object: body, visible: false }]));
    bench.env.mods.ctrl = true;
    bench.drag(&mut tool, Vec2::new(200.0, 300.0), Vec2::new(500.0, 250.0));
    bench.env.mods.ctrl = false;
    assert_eq!((tool.tube_count(), tool.mergeable_count()), (2, 1));
    assert!(Tool::can_undo(&tool, false), "drawing them is the tool's own history");
    bench.leave(&mut tool);
    bench.env.active = false;
    bench.frame(&mut tool);

    let ids = bench.open(vec![("Opened", cube(0.5))]);
    bench.load_tool(&mut tool, &ids, vec![None], sculpt_toolkit::Arrival::Replaced);
    assert_eq!((tool.tube_count(), tool.mergeable_count()), (1, 0), "the tube on the body went with it, at once");
    assert_eq!(at_risk(&tool, Loss::SceneReplaced), None);
    assert!(at_risk(&tool, Loss::Closing).is_some(), "the one in the world is still there, and still work");
    assert!(!Tool::can_undo(&tool, false), "the next Z is the scene's: the tool's own steps were of those tubes");
    bench.frame(&mut tool);
    // Undo brings the body back, but not the tube, as with the tool up.
    assert!(bench.undo_scene());
    bench.frame(&mut tool);
    bench.env.active = true;
    bench.enter(&mut tool);
    bench.frame(&mut tool);
    assert_eq!((tool.tube_count(), tool.mergeable_count()), (1, 0));
}

/// Objects added beside the scene's own (Import) cost nothing: every tube stays.
#[test]
fn objects_added_to_the_scene_leave_the_tubes_and_their_history_alone() {
    let (mut bench, mut tool, _) = bench();
    draw_horn(&mut bench, &mut tool);
    let added = bench.add("Added", cube(0.2));
    bench.load_tool(&mut tool, &[added], vec![None], sculpt_toolkit::Arrival::Added);
    bench.frame(&mut tool);
    assert_eq!((tool.tube_count(), tool.mergeable_count()), (1, 1));
    assert!(Tool::can_undo(&tool, false));
}
