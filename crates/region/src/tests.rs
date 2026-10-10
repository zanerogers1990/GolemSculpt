//! The tools driven the way the app drives them, against the toolkit's bench:
//! a camera 5 units out on +z looking at the origin, 800 x 600, so +x is
//! right on screen, +y is up, and one unit at the origin is about 104 pixels.

use std::time::{Duration, Instant};

use glam::{EulerRot, Quat, Vec2, Vec3};
use sculpt_cut::ScreenRegion;
use sculpt_toolkit::testing::Bench;
use sculpt_toolkit::{Button, Command, Draw, Event, Key, Look, ObjectId, Placement, Scene, Shape as Drawn, Tool, theme};

use crate::testing::{closed_error, sphere, volume};
use crate::{Action, Extract, Region, Shape, hide};

mod copies;
mod mirror;

const CENTRE: Vec2 = Vec2::new(400.0, 300.0);

/// A unit-radius ball at the origin with `tool` brought up on it.
fn bench<T: Tool>(mut tool: T) -> (Bench, T, ObjectId) {
    let mut bench = Bench::new();
    let ball = bench.add("Ball", sphere(4, 1.0));
    bench.enter(&mut tool);
    (bench, tool, ball)
}

fn trim_tool(shape: Shape) -> Region {
    let mut tool = Region::trim();
    tool.shape = shape;
    tool
}

fn positions(bench: &Bench, id: ObjectId) -> Vec<Vec3> {
    bench.doc.mesh(id).unwrap().positions.to_vec()
}

fn mask(bench: &Bench, id: ObjectId) -> Vec<f32> {
    bench.doc.mesh(id).unwrap().mask.to_vec()
}

/// One flag per triangle: hidden or not.
fn hidden(bench: &Bench, id: ObjectId) -> Vec<bool> {
    let mesh = bench.doc.mesh(id).unwrap();
    mesh.hidden_triangles.map_or_else(|| vec![false; mesh.triangle_count()], <[bool]>::to_vec)
}

fn hidden_count(bench: &Bench, id: ObjectId) -> usize {
    hidden(bench, id).iter().filter(|h| **h).count()
}

/// Whether every corner of each triangle passes `test`.
fn wholly(bench: &Bench, id: ObjectId, test: impl Fn(Vec3) -> bool) -> Vec<bool> {
    let mesh = bench.doc.mesh(id).unwrap();
    mesh.indices.chunks_exact(3).map(|corners| corners.iter().all(|&i| test(mesh.positions[i as usize]))).collect()
}

/// The left half of the picture, and so of the ball, with its middle seam.
const LEFT_HALF: [Vec2; 2] = [Vec2::ZERO, Vec2::new(CENTRE.x + 3.0, 600.0)];

fn volume_of(bench: &Bench, id: ObjectId) -> f64 {
    let mesh = bench.doc.mesh(id).unwrap();
    volume(mesh.positions, mesh.indices)
}

#[track_caller]
fn assert_closed(bench: &Bench, id: ObjectId) {
    let mesh = bench.doc.mesh(id).unwrap();
    assert_eq!(closed_error(mesh.positions.len(), mesh.indices), None, "{}", bench.doc.info(id).unwrap().name);
    assert!(volume(mesh.positions, mesh.indices) > 0.0, "wound inside out");
}

#[track_caller]
fn assert_near(actual: f64, expected: f64, relative: f64) {
    assert!((actual - expected).abs() <= expected.abs() * relative, "{actual} is not within {relative} of {expected}");
}

fn status(bench: &Bench) -> (&str, bool) {
    let (text, ok) = bench.status.as_ref().expect("the tool said something");
    (text.as_str(), *ok)
}

fn circle(centre: Vec2, radius: f32, points: usize) -> Vec<Vec2> {
    (0..points)
        .map(|i| {
            let angle = i as f32 / points as f32 * std::f32::consts::TAU;
            centre + Vec2::new(angle.cos(), angle.sin()) * radius
        })
        .collect()
}

/// Presses at the first point and drags through the rest, button still down.
fn draw(bench: &mut Bench, tool: &mut dyn Tool, path: &[Vec2], button: Button) -> bool {
    let taken = bench.press(tool, path[0], button);
    for &p in &path[1..] {
        bench.drag_to(tool, p);
    }
    taken
}

fn stroke(bench: &mut Bench, tool: &mut dyn Tool, path: &[Vec2]) {
    assert!(draw(bench, tool, path, Button::Left), "the tool takes the press");
    bench.release(tool);
}

/// Frames until `done`, for results that arrive from a background job.
fn frames_until(bench: &mut Bench, tool: &mut dyn Tool, done: impl Fn(&Bench) -> bool) {
    let start = Instant::now();
    while !done(bench) {
        assert!(start.elapsed() < Duration::from_secs(20), "the job never landed");
        bench.frame(tool);
        std::thread::sleep(Duration::from_millis(1));
    }
}

// ------------------------------------------------------------------------ mask

#[test]
fn a_box_masks_what_it_covers_and_one_undo_clears_it() {
    let (mut bench, mut tool, ball) = bench(Region::mask());
    // The left half of the screen, and so of the ball: front and back alike.
    bench.drag(&mut tool, Vec2::ZERO, Vec2::new(CENTRE.x + 3.0, 600.0));
    let (points, masked) = (positions(&bench, ball), mask(&bench, ball));
    for (p, m) in points.iter().zip(&masked) {
        assert_eq!(*m, if p.x <= 1e-6 { 1.0 } else { 0.0 }, "{p}");
    }
    let covered = masked.iter().filter(|m| **m > 0.0).count();
    assert!(points.iter().zip(&masked).any(|(p, m)| *m > 0.0 && p.z < -0.5), "the far side is covered too");
    assert_eq!(status(&bench), (format!("Masked {covered} vertices.").as_str(), true));
    assert_eq!(bench.doc.undo_steps(), 1, "one step, like a brush stroke");

    assert!(bench.undo(&mut tool));
    assert!(mask(&bench, ball).iter().all(|m| *m == 0.0));
    assert!(bench.redo(&mut tool));
    assert_eq!(mask(&bench, ball), masked);
}

#[test]
fn ctrl_and_the_right_button_unmask_and_shift_takes_everything_else() {
    let (mut bench, mut tool, ball) = bench(Region::mask());
    bench.drag(&mut tool, Vec2::ZERO, Vec2::new(CENTRE.x + 3.0, 600.0));

    // Ctrl over the top-left quarter takes that much back out.
    bench.env.mods.ctrl = true;
    bench.drag(&mut tool, Vec2::ZERO, Vec2::new(CENTRE.x + 3.0, CENTRE.y + 3.0));
    bench.env.mods.ctrl = false;
    assert!(status(&bench).0.starts_with("Unmasked "));
    for (p, m) in positions(&bench, ball).iter().zip(mask(&bench, ball)) {
        assert_eq!(m, if p.x <= 1e-6 && p.y < -1e-6 { 1.0 } else { 0.0 }, "{p}");
    }

    // The right button does the same with no key held.
    let quarter = [Vec2::new(0.0, CENTRE.y - 3.0), Vec2::new(CENTRE.x + 3.0, 600.0)];
    assert!(draw(&mut bench, &mut tool, &quarter, Button::Right));
    bench.release(&mut tool);
    assert!(mask(&bench, ball).iter().all(|m| *m == 0.0));

    // Shift: everything the box does not cover.
    bench.env.mods.shift = true;
    bench.drag(&mut tool, Vec2::ZERO, Vec2::new(CENTRE.x + 3.0, 600.0));
    bench.env.mods.shift = false;
    for (p, m) in positions(&bench, ball).iter().zip(mask(&bench, ball)) {
        assert_eq!(m, if p.x <= 1e-6 { 0.0 } else { 1.0 }, "{p}");
    }
}

#[test]
fn a_lasso_masks_only_what_is_inside_it() {
    let (mut bench, mut tool, ball) = bench(Region::mask());
    bench.call(|cx| tool.set_shape(cx, Shape::Lasso));
    let loop_ = circle(bench.px(Vec3::new(0.0, 0.5, 0.0)), 40.0, 40);
    stroke(&mut bench, &mut tool, &loop_);
    let masked = mask(&bench, ball);
    let count = masked.iter().filter(|m| **m > 0.0).count();
    assert!(count > 10 && count < masked.len() / 4, "{count} of {}", masked.len());
    for (p, m) in positions(&bench, ball).iter().zip(&masked) {
        if *m > 0.0 {
            assert!(bench.px(*p).distance(loop_[0] - Vec2::new(40.0, 0.0)) < 41.0, "{p} is outside the lasso");
        }
    }
}

#[test]
fn a_click_clears_the_mask() {
    let (mut bench, mut tool, ball) = bench(Region::mask());
    bench.click(&mut tool, CENTRE, Button::Left);
    assert_eq!(status(&bench), ("Nothing is masked.", true));
    assert_eq!(bench.doc.undo_steps(), 0);

    bench.drag(&mut tool, Vec2::ZERO, Vec2::new(CENTRE.x, 600.0));
    bench.click(&mut tool, CENTRE, Button::Left);
    assert_eq!(status(&bench), ("Cleared the mask.", true));
    assert!(mask(&bench, ball).iter().all(|m| *m == 0.0));
    // Undoable like any other edit.
    assert!(bench.undo(&mut tool));
    assert!(mask(&bench, ball).iter().any(|m| *m > 0.0));
}

#[test]
fn a_mirror_axis_masks_both_sides() {
    let (mut bench, mut tool, ball) = bench(Region::mask());
    bench.env.mirror = [true, false, false];
    let edge = bench.px(Vec3::new(0.5, 0.0, 0.0)).x;
    bench.drag(&mut tool, Vec2::new(edge, 0.0), Vec2::new(800.0, 600.0));
    let (points, masked) = (positions(&bench, ball), mask(&bench, ball));
    for (p, m) in points.iter().zip(&masked) {
        let twin = points.iter().position(|q| q.distance(Vec3::new(-p.x, p.y, p.z)) < 1e-5).unwrap();
        assert_eq!(*m, masked[twin], "{p} and its mirror twin go together");
    }
    assert!(masked.iter().any(|m| *m > 0.0) && points.iter().zip(&masked).all(|(p, m)| *m == 0.0 || p.x.abs() > 0.3));
}

/// Radial symmetry about the axis the camera looks along: a box over the
/// right-hand edge of the ball takes the top, left and bottom edges with it.
#[test]
fn radial_symmetry_masks_every_sector() {
    let (mut bench, mut tool, ball) = bench(Region::mask());
    (bench.env.radial_count, bench.env.radial_axis) = (4, Vec3::Z);
    let edge = bench.px(Vec3::new(0.5, 0.0, 0.0)).x;
    bench.drag(&mut tool, Vec2::new(edge, 0.0), Vec2::new(800.0, 600.0));
    let (points, masked) = (positions(&bench, ball), mask(&bench, ball));
    // The box's edge is at x = 0.5 in the middle of the ball, and between 0.4
    // and 0.6 across its depth (the camera is 5 away): past that on any of
    // the four sides is masked, and nearer the axis than that is not.
    let (mut inside, mut outside) = (0, 0);
    for (p, m) in points.iter().zip(&masked) {
        let from_axis = p.x.abs().max(p.y.abs());
        if from_axis > 0.61 {
            assert_eq!(*m, 1.0, "{p} is under a copy of the box");
            inside += 1;
        } else if from_axis < 0.39 {
            assert_eq!(*m, 0.0, "{p} is under none");
            outside += 1;
        }
    }
    assert!(inside > 100 && outside > 100, "{inside} / {outside}");
    for side in [Vec3::X, Vec3::Y, Vec3::NEG_X, Vec3::NEG_Y] {
        assert!(points.iter().zip(&masked).any(|(p, m)| *m == 1.0 && p.dot(side) > 0.9), "nothing masked toward {side}");
    }
}

// ------------------------------------------------------------------------ hide

#[test]
fn a_box_hides_the_faces_it_wholly_covers_and_one_undo_shows_them_again() {
    let (mut bench, mut tool, ball) = bench(Region::hide());
    let view = bench.env.view;
    let left = ScreenRegion::rect(LEFT_HALF[0], LEFT_HALF[1]).unwrap();
    let plan = bench.call(|cx| hide::plan(cx, &view, &left, false, true, &mut Vec::new())).unwrap();
    assert!(plan.len() > 900 && plan.len() <= 1024, "{}", plan.len());

    bench.drag(&mut tool, LEFT_HALF[0], LEFT_HALF[1]);
    // Exactly the faces the plan named: those with all three corners
    // covered. One straddling the edge of the box stays.
    assert_eq!(hidden_count(&bench, ball), plan.len());
    assert_eq!(hidden(&bench, ball), wholly(&bench, ball, |p| p.x <= 1e-6));
    // Nothing is removed, and the vertices along the edge still show.
    let mesh = bench.doc.mesh(ball).unwrap();
    assert_eq!((mesh.vertex_count(), mesh.triangle_count()), (1026, 2048));
    for (p, gone) in mesh.positions.iter().zip(mesh.hidden_vertices.unwrap()) {
        assert_eq!(*gone, p.x < -1e-6, "{p}");
    }
    assert_eq!(status(&bench), (format!("Hid {} faces. A click shows everything again.", plan.len()).as_str(), true));
    assert_eq!(bench.doc.undo_steps(), 1, "one step, like a mask");

    assert!(bench.undo(&mut tool));
    assert!(bench.doc.mesh(ball).unwrap().hidden_triangles.is_none());
    assert!(bench.redo(&mut tool));
    assert_eq!(hidden_count(&bench, ball), plan.len());
}

#[test]
fn hiding_the_whole_object_is_refused_with_a_status_line() {
    let (mut bench, mut tool, ball) = bench(Region::hide());
    // In one overshooting drag.
    bench.drag(&mut tool, Vec2::ZERO, Vec2::new(800.0, 600.0));
    assert_eq!(status(&bench), ("That would hide the whole object - skipped.", false));
    assert_eq!((hidden_count(&bench, ball), bench.doc.undo_steps()), (0, 0));

    // And in two: the left half, then a box over all that is left.
    bench.drag(&mut tool, LEFT_HALF[0], LEFT_HALF[1]);
    let half = hidden_count(&bench, ball);
    bench.drag(&mut tool, Vec2::new(CENTRE.x - 3.0, 0.0), Vec2::new(800.0, 600.0));
    assert_eq!(status(&bench), ("That would hide the whole object - skipped.", false));
    assert_eq!((hidden_count(&bench, ball), bench.doc.undo_steps()), (half, 1));

    // A drag over what is hidden already has nothing to add.
    bench.drag(&mut tool, LEFT_HALF[0], LEFT_HALF[1]);
    assert_eq!(status(&bench), ("Nothing new to hide there.", false));
    assert_eq!(bench.doc.undo_steps(), 1);
}

#[test]
fn ctrl_and_the_right_button_show_again_shift_hides_everything_else_and_a_click_shows_all() {
    let (mut bench, mut tool, ball) = bench(Region::hide());
    bench.click(&mut tool, CENTRE, Button::Left);
    assert_eq!(status(&bench), ("Nothing is hidden.", true));
    assert_eq!(bench.doc.undo_steps(), 0);

    bench.drag(&mut tool, LEFT_HALF[0], LEFT_HALF[1]);
    let half = hidden_count(&bench, ball);

    // Ctrl over the top-left quarter brings that much back.
    let top_left = [Vec2::ZERO, Vec2::new(CENTRE.x + 3.0, CENTRE.y + 3.0)];
    bench.env.mods.ctrl = true;
    bench.drag(&mut tool, top_left[0], top_left[1]);
    bench.env.mods.ctrl = false;
    assert!(status(&bench).0.starts_with("Showed "), "{:?}", bench.status);
    let shown = wholly(&bench, ball, |p| p.x <= 1e-6 && p.y >= -1e-6);
    let left = wholly(&bench, ball, |p| p.x <= 1e-6);
    let expected: Vec<bool> = left.iter().zip(&shown).map(|(left, shown)| *left && !*shown).collect();
    assert_eq!(hidden(&bench, ball), expected);
    assert!(hidden_count(&bench, ball) < half);

    // The right button shows too, with no key held.
    let bottom_left = [Vec2::new(0.0, CENTRE.y - 3.0), Vec2::new(CENTRE.x + 3.0, 600.0)];
    assert!(draw(&mut bench, &mut tool, &bottom_left, Button::Right));
    bench.release(&mut tool);
    assert!(bench.doc.mesh(ball).unwrap().hidden_triangles.is_none(), "the two quarters make the half");

    // A click shows everything, as one more undo step.
    bench.drag(&mut tool, LEFT_HALF[0], LEFT_HALF[1]);
    assert_eq!(hidden_count(&bench, ball), half);
    let steps = bench.doc.undo_steps();
    bench.click(&mut tool, CENTRE, Button::Left);
    assert_eq!(status(&bench), ("Everything is showing again.", true));
    assert!(bench.doc.mesh(ball).unwrap().hidden_triangles.is_none());
    assert_eq!(bench.doc.undo_steps(), steps + 1);

    // Shift: everything the box does not cover ("isolate this part").
    bench.env.mods.shift = true;
    bench.drag(&mut tool, LEFT_HALF[0], LEFT_HALF[1]);
    bench.env.mods.shift = false;
    assert_eq!(hidden(&bench, ball), wholly(&bench, ball, |p| p.x > 1e-6));
}

#[test]
fn a_mask_skips_hidden_vertices() {
    let (mut bench, mut hider, ball) = bench(Region::hide());
    bench.drag(&mut hider, LEFT_HALF[0], LEFT_HALF[1]);
    bench.leave(&mut hider);

    let mut masker = Region::mask();
    bench.enter(&mut masker);
    bench.drag(&mut masker, Vec2::ZERO, Vec2::new(800.0, 600.0));
    let mesh = bench.doc.mesh(ball).unwrap();
    let gone = mesh.hidden_vertices.unwrap();
    for ((p, m), gone) in mesh.positions.iter().zip(mesh.mask).zip(gone) {
        assert_eq!(*m, if *gone { 0.0 } else { 1.0 }, "{p}");
    }
    let showing = gone.iter().filter(|g| !**g).count();
    assert!(showing > 500 && showing < 600, "{showing}");
    assert_eq!(status(&bench), (format!("Masked {showing} vertices.").as_str(), true));
    // With every vertex under the box hidden, there is nothing to mask.
    bench.env.mods.ctrl = true;
    bench.drag(&mut masker, Vec2::ZERO, Vec2::new(CENTRE.x - 30.0, 600.0));
    assert_eq!(status(&bench), ("Region covered no vertices.", false));
}

#[test]
fn a_mirror_axis_hides_both_sides() {
    let (mut bench, mut tool, ball) = bench(Region::hide());
    bench.env.mirror = [true, false, false];
    let edge = bench.px(Vec3::new(0.5, 0.0, 0.0)).x;
    bench.drag(&mut tool, Vec2::new(edge, 0.0), Vec2::new(800.0, 600.0));
    let flags = hidden(&bench, ball);
    let right = wholly(&bench, ball, |p| p.x > 0.0);
    let left = wholly(&bench, ball, |p| p.x < 0.0);
    let on = |side: &[bool]| flags.iter().zip(side).filter(|(hidden, side)| **hidden && **side).count();
    assert!(on(&right) > 50 && on(&right) == on(&left), "{} right, {} left", on(&right), on(&left));
    assert_eq!(on(&right) + on(&left), hidden_count(&bench, ball));
}

#[test]
fn the_hide_wheel_shows_all_and_inverts() {
    let (mut bench, mut tool, ball) = bench(Region::hide());
    let labels: Vec<&str> = tool.radial().iter().map(|tile| tile.label).collect();
    assert_eq!(labels, ["Box", "Lasso", "Show All", "Invert"]);
    bench.send(&mut tool, Event::Radial(3));
    assert_eq!(status(&bench), ("Nothing is hidden, so there is nothing to swap.", true));
    assert_eq!(bench.doc.undo_steps(), 0);

    bench.drag(&mut tool, LEFT_HALF[0], LEFT_HALF[1]);
    let half = hidden_count(&bench, ball);
    bench.send(&mut tool, Event::Radial(3));
    assert_eq!(hidden_count(&bench, ball), 2048 - half);
    assert_eq!(status(&bench), ("Swapped the hidden and the showing parts.", true));
    bench.send(&mut tool, Event::Radial(2));
    assert_eq!(hidden_count(&bench, ball), 0);
    assert_eq!(status(&bench), ("Everything is showing again.", true));
    assert_eq!(bench.doc.undo_steps(), 3);
}

// ------------------------------------------------------------------------ trim

#[test]
fn a_line_trim_leaves_a_closed_half_and_one_undo_restores_the_whole() {
    let (mut bench, mut tool, ball) = bench(trim_tool(Shape::Line));
    let (whole, before) = (volume_of(&bench, ball), bench.doc.info(ball).unwrap().mesh);
    // Drawn top to bottom, a line covers what is on its left looking along
    // it: the right-hand side of the screen.
    bench.drag(&mut tool, Vec2::new(CENTRE.x, 30.0), Vec2::new(CENTRE.x, 570.0));
    assert_closed(&bench, ball);
    assert_near(volume_of(&bench, ball), whole * 0.5, 0.02);
    assert!(positions(&bench, ball).iter().all(|p| p.x <= 1e-4), "the right-hand half is gone");
    let (text, ok) = status(&bench);
    assert!(ok && text.starts_with("Trimmed: 2,048 -> ") && text.ends_with("of them the new cut face."), "{text}");
    assert_ne!(bench.doc.info(ball).unwrap().mesh, before);
    assert_eq!(bench.doc.undo_steps(), 1);

    assert!(bench.undo(&mut tool));
    assert_eq!(bench.doc.info(ball).unwrap().mesh, before);
    assert_near(volume_of(&bench, ball), whole, 1e-9);
}

#[test]
fn ctrl_keeps_the_covered_part_instead() {
    let (mut bench, mut tool, ball) = bench(trim_tool(Shape::Box));
    bench.env.mods.ctrl = true;
    bench.drag(&mut tool, Vec2::ZERO, Vec2::new(CENTRE.x, 600.0));
    assert_closed(&bench, ball);
    assert!(positions(&bench, ball).iter().all(|p| p.x <= 1e-4), "cropped down to the box");

    // Shift on a line only turns it: 4 degrees off upright snaps upright,
    // and the covered side still goes.
    let (mut bench, mut tool, ball) = self::bench(trim_tool(Shape::Line));
    bench.env.mods.shift = true;
    bench.drag(&mut tool, Vec2::new(CENTRE.x, 30.0), Vec2::new(CENTRE.x + 38.0, 570.0));
    assert_closed(&bench, ball);
    assert!(positions(&bench, ball).iter().all(|p| p.x <= 1e-4));
}

#[test]
fn a_mirrored_trim_cuts_both_sides() {
    let (mut bench, mut tool, ball) = bench(trim_tool(Shape::Box));
    let whole = volume_of(&bench, ball);
    let edge = bench.px(Vec3::new(0.5, 0.0, 0.0)).x;
    let box_ = [Vec2::new(edge, 0.0), Vec2::new(800.0, 600.0)];

    bench.drag(&mut tool, box_[0], box_[1]);
    let one_side = bench.doc.bounds(ball);
    assert!((one_side.min.x + 1.0).abs() < 1e-4 && one_side.max.x < 0.65, "{one_side:?}");
    let one_cut = volume_of(&bench, ball);
    assert!(bench.undo(&mut tool));

    bench.env.mirror = [true, false, false];
    bench.drag(&mut tool, box_[0], box_[1]);
    assert_closed(&bench, ball);
    let both = bench.doc.bounds(ball);
    assert!((both.min.x + both.max.x).abs() < 1e-3 && both.max.x < 0.65, "{both:?}");
    // Each side loses the same amount.
    assert_near(whole - volume_of(&bench, ball), 2.0 * (whole - one_cut), 0.01);
    assert_eq!(bench.doc.undo_steps(), 1, "both sides in one step");
    assert!(bench.undo(&mut tool));
    assert_near(volume_of(&bench, ball), whole, 1e-9);
}

/// The same box under radial symmetry about the view axis cuts all four
/// sides off, in one step.
#[test]
fn a_radial_trim_cuts_all_the_way_round() {
    let (mut bench, mut tool, ball) = bench(trim_tool(Shape::Box));
    let whole = volume_of(&bench, ball);
    (bench.env.radial_count, bench.env.radial_axis) = (4, Vec3::Z);
    let edge = bench.px(Vec3::new(0.5, 0.0, 0.0)).x;
    bench.drag(&mut tool, Vec2::new(edge, 0.0), Vec2::new(800.0, 600.0));
    assert_closed(&bench, ball);
    let cut = bench.doc.bounds(ball);
    for axis in 0..2 {
        assert!(cut.max[axis] < 0.65 && cut.max[axis] > 0.35, "{cut:?}");
        assert!((cut.min[axis] + cut.max[axis]).abs() < 1e-3, "{cut:?}");
    }
    assert!((cut.min.z + 1.0).abs() < 1e-4 && (cut.max.z - 1.0).abs() < 1e-4, "the cut runs along the view: {cut:?}");
    assert!(volume_of(&bench, ball) < whole * 0.75);
    assert_eq!(bench.doc.undo_steps(), 1, "every side in one step");
    assert!(bench.undo(&mut tool));
    assert_near(volume_of(&bench, ball), whole, 1e-9);
}

/// The matrices handed to `sculpt_cut` are built here from the camera and
/// the object's placement, so this is the check that they are the right way
/// round: a box over the left half of the picture of a moved, turned and
/// stretched ball removes exactly what was drawn left of its middle.
#[test]
fn a_box_over_the_left_of_a_moved_and_turned_object_trims_what_is_on_the_left_of_the_screen() {
    let (mut bench, mut tool, ball) = bench(trim_tool(Shape::Box));
    let placement = Placement {
        position: Vec3::new(0.7, -0.4, 0.6),
        rotation: Quat::from_euler(EulerRot::YXZ, 0.8, -0.5, 0.3),
        scale: Vec3::new(1.2, 0.7, 0.9),
    };
    bench.call(|cx| cx.commit("Place", vec![Command::Place { object: ball, placement }]));
    let model = placement.matrix();
    let on_screen = |bench: &Bench, local: Vec3| bench.px(model.transform_point3(local));
    let whole = volume_of(&bench, ball);
    let edge = on_screen(&bench, Vec3::ZERO).x;
    let left_before = positions(&bench, ball).iter().filter(|p| on_screen(&bench, **p).x < edge - 1.0).count();
    assert!(left_before > 300, "the ball straddles the edge: {left_before}");

    bench.drag(&mut tool, Vec2::ZERO, Vec2::new(edge, 600.0));
    assert_closed(&bench, ball);
    let after = positions(&bench, ball);
    let furthest_left = after.iter().map(|p| on_screen(&bench, *p).x).fold(f32::MAX, f32::min);
    assert!(furthest_left >= edge - 0.05, "something is left of the edge: {furthest_left} < {edge}");
    assert!(after.iter().any(|p| on_screen(&bench, *p).x > edge + 60.0), "the right-hand side is still there");
    // The box's edge, swept from the eye, is a plane through the middle of
    // the ball, and any such plane halves it however it is stretched.
    assert_near(volume_of(&bench, ball), whole * 0.5, 0.02);
    assert_eq!(bench.doc.info(ball).unwrap().placement, placement, "the object stays where it was");
}

#[test]
fn a_mask_is_carried_across_a_trim() {
    let (mut bench, mut tool, ball) = bench(trim_tool(Shape::Line));
    let top: Vec<f32> = positions(&bench, ball).iter().map(|p| if p.y > 0.3 { 1.0 } else { 0.0 }).collect();
    bench.call(|cx| cx.commit("Mask", vec![Command::Mask { object: ball, mask: top }]));
    bench.drag(&mut tool, Vec2::new(CENTRE.x, 30.0), Vec2::new(CENTRE.x, 570.0));
    let (points, masked) = (positions(&bench, ball), mask(&bench, ball));
    assert_eq!(points.len(), masked.len());
    // The surface that survived keeps its values exactly; the new cut face
    // (at x = 0) blends the values round its rim across itself.
    for (p, m) in points.iter().zip(&masked).filter(|(p, _)| p.x < -0.05) {
        if p.y > 0.4 {
            assert_eq!(*m, 1.0, "{p}");
        }
        if p.y < 0.2 {
            assert_eq!(*m, 0.0, "{p}");
        }
    }
    assert!(masked.iter().all(|m| (0.0..=1.0).contains(m)));
    let on_face = |p: &Vec3| p.x.abs() < 1e-4 && p.length() < 0.9;
    assert!(points.iter().zip(&masked).any(|(p, m)| on_face(p) && p.y > 0.6 && *m > 0.9), "the face is masked near the top");
    assert!(points.iter().zip(&masked).any(|(p, m)| on_face(p) && p.y < -0.6 && *m < 0.1), "and free near the bottom");
}

#[test]
fn a_shape_that_misses_says_so_and_changes_nothing() {
    let (mut bench, mut tool, ball) = bench(trim_tool(Shape::Box));
    bench.drag(&mut tool, Vec2::new(5.0, 5.0), Vec2::new(60.0, 60.0));
    let (text, ok) = status(&bench);
    assert!(!ok && text.starts_with("Nothing trimmed - "), "{text}");
    assert_eq!(bench.doc.undo_steps(), 0);
    assert_eq!(positions(&bench, ball).len(), 1026);

    // A click is not a region: it says what to do instead.
    bench.click(&mut tool, CENTRE, Button::Left);
    assert_eq!(status(&bench), ("Drag a shape out to trim.", true));
    assert_eq!(bench.doc.undo_steps(), 0);
}

#[test]
fn a_large_trim_runs_as_a_job_and_lands_on_a_later_frame() {
    let (mut bench, mut tool, ball) = bench(trim_tool(Shape::Line));
    tool.job_above = 0;
    let whole = volume_of(&bench, ball);
    bench.drag(&mut tool, Vec2::new(CENTRE.x, 30.0), Vec2::new(CENTRE.x, 570.0));
    assert!(tool.is_busy());
    assert_near(volume_of(&bench, ball), whole, 1e-9);
    // No second cut on top of one still being worked out: the camera gets
    // the drag.
    assert!(!bench.press(&mut tool, CENTRE, Button::Left));
    bench.release(&mut tool);

    // It lands even if the user has moved on to another tool.
    bench.env.active = false;
    frames_until(&mut bench, &mut tool, |bench| bench.doc.undo_steps() == 1);
    assert!(!tool.is_busy());
    assert_closed(&bench, ball);
    assert_near(volume_of(&bench, ball), whole * 0.5, 0.02);
    assert!(status(&bench).0.starts_with("Trimmed: "));
    assert!(bench.undo(&mut tool));
    assert_near(volume_of(&bench, ball), whole, 1e-9);
}

#[test]
fn a_cut_that_comes_back_to_a_changed_object_is_dropped() {
    let (mut bench, mut tool, ball) = bench(trim_tool(Shape::Line));
    tool.job_above = 0;
    bench.drag(&mut tool, Vec2::new(CENTRE.x, 30.0), Vec2::new(CENTRE.x, 570.0));
    // Something else replaces the geometry while the cut is in flight.
    bench.call(|cx| cx.commit("Remesh", vec![Command::Replace { object: ball, mesh: sphere(2, 1.0).into() }]));
    bench.call(|cx| tool.settle(cx));
    assert_eq!(status(&bench), ("The object changed while it was being cut - nothing was done.", false));
    assert_eq!(bench.doc.info(ball).unwrap().triangles, 128);
}

// ----------------------------------------------------------------------- split

#[test]
fn a_lasso_split_gives_two_closed_objects_whose_volumes_add_up() {
    let (mut bench, mut tool, ball) = bench(Region::split());
    bench.call(|cx| tool.set_shape(cx, Shape::Lasso));
    let (whole, before) = (volume_of(&bench, ball), bench.doc.info(ball).unwrap().mesh);
    // A bite out of the right-hand edge.
    let bite = circle(bench.px(Vec3::new(0.9, 0.1, 0.0)), 80.0, 48);
    stroke(&mut bench, &mut tool, &bite);

    let objects = bench.doc.objects();
    assert_eq!(objects.len(), 2);
    let part = objects[1];
    assert_eq!(bench.doc.info(part).unwrap().name, "Ball Part");
    assert_eq!(bench.doc.active(), ball, "the cut object stays selected, ready for the next cut");
    assert_closed(&bench, ball);
    assert_closed(&bench, part);
    let (kept, piece) = (volume_of(&bench, ball), volume_of(&bench, part));
    assert!(piece > whole * 0.03 && piece < whole * 0.4, "{piece} of {whole}");
    assert_near(kept + piece, whole, 0.01);
    assert_eq!(status(&bench), ("Split into Ball and Ball Part.", true));
    assert_eq!(bench.toasts.last().map(String::as_str), Some("Split"));

    assert_eq!(bench.doc.undo_steps(), 1);
    assert!(bench.undo(&mut tool));
    assert_eq!(bench.doc.objects(), vec![ball], "one Z brings the whole object back");
    assert_eq!(bench.doc.info(ball).unwrap().mesh, before);
    assert_near(volume_of(&bench, ball), whole, 1e-9);
}

#[test]
fn a_lasso_inside_the_outline_splits_a_plug_out_of_the_middle() {
    let (mut bench, mut tool, ball) = bench(Region::split());
    bench.call(|cx| tool.set_shape(cx, Shape::Lasso));
    let whole = volume_of(&bench, ball);
    stroke(&mut bench, &mut tool, &circle(CENTRE + Vec2::new(7.0, -5.0), 45.0, 40));
    let objects = bench.doc.objects();
    assert_eq!(objects.len(), 2, "{:?}", bench.status);
    assert_closed(&bench, ball);
    assert_closed(&bench, objects[1]);
    assert_near(volume_of(&bench, ball) + volume_of(&bench, objects[1]), whole, 0.01);
}

#[test]
fn a_line_split_puts_the_piece_where_the_object_is() {
    let (mut bench, mut tool, ball) = bench(Region::split());
    assert_eq!(tool.shape(), Shape::Line, "a flat cut is what a split is for");
    let placement = Placement { position: Vec3::new(0.3, 0.2, -0.5), rotation: Quat::from_rotation_z(0.6), scale: Vec3::splat(0.8) };
    bench.call(|cx| cx.commit("Place", vec![Command::Place { object: ball, placement }]));
    let whole = volume_of(&bench, ball);
    let middle = bench.px(placement.position);
    bench.drag(&mut tool, Vec2::new(middle.x, 20.0), Vec2::new(middle.x, 580.0));

    let part = bench.doc.objects()[1];
    assert_eq!(bench.doc.info(part).unwrap().placement, placement);
    assert_near(volume_of(&bench, ball), whole * 0.5, 0.02);
    assert_near(volume_of(&bench, part), whole * 0.5, 0.02);
    // The covered side (right of a line drawn downward) is the new piece.
    let model = placement.matrix();
    assert!(positions(&bench, part).iter().all(|p| bench.px(model.transform_point3(*p)).x >= middle.x - 0.05));
    assert!(positions(&bench, ball).iter().all(|p| bench.px(model.transform_point3(*p)).x <= middle.x + 0.05));
}

#[test]
fn a_split_that_misses_adds_nothing() {
    let (mut bench, mut tool, _) = bench(Region::split());
    bench.drag(&mut tool, Vec2::new(20.0, 30.0), Vec2::new(20.0, 570.0));
    let (text, ok) = status(&bench);
    assert!(!ok && text.starts_with("Nothing split - "), "{text}");
    assert_eq!(bench.doc.objects().len(), 1);
    assert_eq!(bench.toasts.last().map(String::as_str), Some("Nothing split"));
    assert_eq!(bench.doc.undo_steps(), 0);
}

// --------------------------------------------------------------------- gesture

#[test]
fn a_press_starts_a_region_anywhere_and_is_passed_on_when_none_could_come_of_it() {
    // On the model and on empty background alike: a box has to be able to
    // start beside the model.
    let (mut bench, mut tool, ball) = bench(Region::mask());
    assert!(bench.press(&mut tool, CENTRE, Button::Left));
    bench.release(&mut tool);
    assert!(bench.press(&mut tool, Vec2::new(8.0, 8.0), Button::Left));
    bench.release(&mut tool);

    // Nothing to act on while the selected object is hidden.
    bench.call(|cx| cx.commit("Hide", vec![Command::Show { object: ball, visible: false }]));
    assert!(!bench.press(&mut tool, CENTRE, Button::Left));
    bench.release(&mut tool);
    assert_eq!(status(&bench), ("The selected object is hidden.", false));
    assert!(!tool.is_dragging());
    assert_eq!(bench.doc.undo_steps(), 1, "only the hiding");
}

#[test]
fn esc_abandons_a_drag_and_is_otherwise_left_to_the_app() {
    let (mut bench, mut tool, ball) = bench(trim_tool(Shape::Box));
    assert!(draw(&mut bench, &mut tool, &[Vec2::ZERO, Vec2::new(CENTRE.x, 600.0)], Button::Left));
    assert!(tool.is_dragging());
    assert!(bench.key(&mut tool, Key::Escape));
    assert!(!tool.is_dragging());
    // The button is still down; letting it go does nothing.
    bench.drag_to(&mut tool, Vec2::new(500.0, 500.0));
    bench.release(&mut tool);
    assert_eq!(status(&bench), ("Cancelled.", true));
    assert_eq!(bench.doc.undo_steps(), 0);
    assert_eq!(positions(&bench, ball).len(), 1026);
    // With nothing to cancel the app gets Esc, and puts the tool away.
    assert!(!bench.key(&mut tool, Key::Escape));
}

#[test]
fn the_tools_key_steps_through_its_shapes_and_then_lets_go() {
    let (mut bench, mut tool, _) = bench(Region::split());
    assert!(bench.key(&mut tool, Key::K));
    assert_eq!(tool.shape(), Shape::Lasso);
    assert!(bench.key(&mut tool, Key::K));
    assert_eq!(tool.shape(), Shape::Box);
    assert_eq!(bench.toasts.last().map(String::as_str), Some("Box Split"));
    // Past the last the key is the app's, which puts the tool away; next
    // time it starts from the line again.
    assert!(!bench.key(&mut tool, Key::K));
    assert_eq!(tool.shape(), Shape::Line);

    let (mut bench, mut tool, _) = self::bench(Region::mask());
    assert!(bench.key(&mut tool, Key::N));
    assert_eq!(tool.shape(), Shape::Lasso);
    assert!(!bench.key(&mut tool, Key::N));
    // Another tool's key is never taken.
    assert!(!bench.key(&mut tool, Key::K) && !bench.key(&mut tool, Key::T));

    // A brush key is the way straight back to sculpting.
    assert!(bench.key(&mut tool, Key::Num3) && bench.finished);
}

#[test]
fn the_wheel_picks_shapes_and_for_the_mask_clears_and_inverts() {
    let (mut bench, mut tool, ball) = bench(Region::mask());
    let labels: Vec<&str> = tool.radial().iter().map(|tile| tile.label).collect();
    assert_eq!(labels, ["Box", "Lasso", "Clear", "Invert"]);
    assert!(tool.radial()[0].on && !tool.radial()[1].on);
    bench.send(&mut tool, Event::Radial(1));
    assert_eq!(tool.shape(), Shape::Lasso);
    bench.send(&mut tool, Event::Radial(3));
    assert!(mask(&bench, ball).iter().all(|m| *m == 1.0));
    bench.send(&mut tool, Event::Radial(2));
    assert!(mask(&bench, ball).iter().all(|m| *m == 0.0));

    let trim = Region::trim();
    assert_eq!(trim.radial().iter().map(|tile| tile.label).collect::<Vec<_>>(), ["Box", "Lasso", "Line"]);
    assert_eq!(trim.radial()[2].icon, "region_line_trim");
}

fn lines(draw: &Draw) -> usize {
    draw.overlay.shapes.iter().filter(|s| matches!(s, Drawn::Line { .. })).count()
}

fn fills(draw: &Draw) -> usize {
    draw.overlay.shapes.iter().filter(|s| matches!(s, Drawn::Fill { .. })).count()
}

#[test]
fn the_marquee_follows_the_drag_and_a_crosshair_marks_the_armed_pointer() {
    let (mut bench, mut tool, _) = bench(Region::mask());
    bench.hover(Vec2::new(200.0, 200.0));
    let idle = bench.frame(&mut tool);
    assert_eq!((lines(&idle), fills(&idle)), (4, 0), "the four arms of the crosshair");
    bench.env.hovering = false;
    assert!(bench.frame(&mut tool).is_empty(), "no crosshair over a panel");
    bench.env.hovering = true;

    // A box: its shaded inside and its outline.
    draw(&mut bench, &mut tool, &[Vec2::new(100.0, 100.0), Vec2::new(300.0, 250.0)], Button::Left);
    let boxed = bench.frame(&mut tool);
    assert_eq!((lines(&boxed), fills(&boxed)), (1, 1));
    let Drawn::Line { color: plain, .. } = &boxed.overlay.shapes[1] else { panic!() };
    assert_eq!(*plain, Action::Mask.tint());
    // Ctrl turns it the erasing red before the button is let go.
    bench.env.mods.ctrl = true;
    let Drawn::Line { color, .. } = &bench.frame(&mut tool).overlay.shapes[1] else { panic!() };
    assert_eq!(*color, crate::action::REMOVE);
    bench.env.mods.ctrl = false;
    bench.release(&mut tool);

    // A lasso mid-draw: the path and the edge that will close it, no fill.
    bench.call(|cx| tool.set_shape(cx, Shape::Lasso));
    let path = circle(CENTRE, 90.0, 40);
    draw(&mut bench, &mut tool, &path[..30], Button::Left);
    let lasso = bench.frame(&mut tool);
    assert_eq!((lines(&lasso), fills(&lasso)), (2, 0));
    let Drawn::Line { points, closed, .. } = &lasso.overlay.shapes[0] else { panic!() };
    assert_eq!((points.len(), *closed), (30, false));
    assert!(tool.is_dragging());

    // Put away mid-drag, the drag is dropped.
    bench.leave(&mut tool);
    assert!(!tool.is_dragging());

    // A cut line: the shaded half, the line across the screen, its ticks.
    let (mut bench, mut tool, _) = self::bench(trim_tool(Shape::Line));
    draw(&mut bench, &mut tool, &[Vec2::new(300.0, 500.0), Vec2::new(500.0, 100.0)], Button::Left);
    let line = bench.frame(&mut tool);
    assert_eq!(fills(&line), 1);
    assert!(lines(&line) >= 5);
}

#[test]
fn nothing_is_drawn_or_dragged_over_a_clean_view() {
    let (mut bench, mut tool, _) = bench(Region::mask());
    draw(&mut bench, &mut tool, &[Vec2::new(100.0, 100.0), Vec2::new(300.0, 250.0)], Button::Left);
    bench.env.clean_view = true;
    assert!(bench.frame(&mut tool).is_empty());
    assert!(!tool.is_dragging());
    bench.release(&mut tool);
    assert!(!bench.press(&mut tool, CENTRE, Button::Left));
    bench.release(&mut tool);
    assert_eq!(bench.doc.undo_steps(), 0);
}

// --------------------------------------------------------------------- extract

/// The ball with its top masked and Extract brought up on it.
fn extract_bench() -> (Bench, Extract, ObjectId) {
    let mut bench = Bench::new();
    let ball = bench.add("Ball", sphere(4, 1.0));
    let top: Vec<f32> = positions(&bench, ball).iter().map(|p| if p.y > 0.3 { 1.0 } else { 0.0 }).collect();
    bench.call(|cx| cx.commit("Mask", vec![Command::Mask { object: ball, mask: top }]));
    let mut tool = Extract::default();
    bench.enter(&mut tool);
    (bench, tool, ball)
}

#[test]
fn extract_previews_the_masked_area_and_accept_adds_one_object() {
    let (mut bench, mut tool, ball) = extract_bench();
    let whole = volume_of(&bench, ball);
    let shown = bench.frame(&mut tool);
    assert_eq!(shown.previews().len(), 1);
    assert_eq!(shown.previews()[0].style.look, Look::Shell);
    assert!(tool.preview_triangles() > 400 && tool.error().is_none());
    assert_eq!(bench.doc.objects().len(), 1, "a preview is not an object");

    // A drag is the camera's: the tool takes no presses.
    assert!(!bench.press(&mut tool, CENTRE, Button::Left));
    bench.release(&mut tool);

    assert!(bench.key(&mut tool, Key::Enter));
    let objects = bench.doc.objects();
    assert_eq!(objects.len(), 2);
    let made = bench.doc.info(objects[1]).unwrap();
    assert_eq!((made.name, made.active), ("Ball Extract", true));
    assert_eq!(made.triangles, tool.preview_triangles());
    assert_closed(&bench, objects[1]);
    // A shell over the top of a unit ball sits above the mask's edge.
    let shell = bench.doc.bounds(objects[1]);
    assert!(shell.min.y > 0.1 && shell.max.y > 1.0 && shell.max.y < 1.2, "{shell:?}");
    assert!(bench.finished && status(&bench).0.starts_with("Extracted \"Ball Extract\": "));
    assert_eq!(volume_of(&bench, ball), whole, "the source is untouched");

    assert!(bench.undo(&mut tool));
    assert_eq!(bench.doc.objects(), vec![ball]);
}

#[test]
fn extract_with_nothing_masked_says_so_and_adds_nothing() {
    let (mut bench, mut tool, _) = bench(Extract::default());
    assert!(bench.frame(&mut tool).previews().is_empty());
    assert_eq!(tool.error(), Some("Nothing masked - paint a mask first."));
    assert!(bench.key(&mut tool, Key::Enter));
    assert_eq!(status(&bench), ("Nothing masked - paint a mask first.", false));
    assert_eq!(bench.doc.objects().len(), 1);
    assert!(!bench.finished);
}

#[test]
fn the_preview_follows_the_settings_the_mask_and_the_placement() {
    let (mut bench, mut tool, ball) = extract_bench();
    let first = bench.frame(&mut tool);
    let version = first.previews()[0].mesh.version();
    assert_eq!(bench.frame(&mut tool).previews()[0].mesh.version(), version, "nothing changed, nothing rebuilt");

    // A setting: rebuilt on the next frame.
    let mut settings = *tool.settings();
    settings.thickness_fraction = 0.2;
    tool.set_settings(settings);
    let thicker = bench.frame(&mut tool);
    assert!(thicker.previews()[0].mesh.version() > version);
    let top = |draw: &Draw| draw.previews()[0].mesh.data().positions.iter().map(|p| p.y).fold(f32::MIN, f32::max);
    assert!(top(&thicker) > top(&first) + 0.1);

    // The mask (an undo, say): noticed at the next look.
    let triangles = tool.preview_triangles();
    let more: Vec<f32> = positions(&bench, ball).iter().map(|p| if p.y > -0.2 { 1.0 } else { 0.0 }).collect();
    bench.call(|cx| cx.commit("Mask", vec![Command::Mask { object: ball, mask: more }]));
    bench.env.time += 1.0;
    bench.frame(&mut tool);
    assert!(tool.preview_triangles() > triangles);

    // The shell is drawn through the source's placement, so it goes with it.
    let placement = Placement::at(Vec3::new(0.5, 0.0, 0.0));
    bench.call(|cx| cx.commit("Place", vec![Command::Place { object: ball, placement }]));
    assert_eq!(bench.frame(&mut tool).previews()[0].transforms, vec![placement.matrix()]);

    // Put away, the preview goes with the tool.
    bench.leave(&mut tool);
    bench.env.active = false;
    assert!(bench.frame(&mut tool).previews().is_empty());
    assert_eq!(bench.doc.objects().len(), 1);
}

#[test]
fn a_large_extract_previews_and_lands_through_jobs() {
    let (mut bench, mut tool, _) = extract_bench();
    tool.job_above = 0;
    assert!(bench.frame(&mut tool).previews().is_empty(), "still being worked out");
    let start = Instant::now();
    while tool.preview_triangles() == 0 {
        assert!(start.elapsed() < Duration::from_secs(20), "the preview never arrived");
        bench.frame(&mut tool);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(bench.key(&mut tool, Key::Enter));
    assert!(bench.finished);
    assert_eq!(bench.doc.objects().len(), 1, "being baked");
    // The tool has been put away by now; the object still arrives.
    bench.leave(&mut tool);
    bench.env.active = false;
    frames_until(&mut bench, &mut tool, |bench| bench.doc.objects().len() == 2);
    assert_closed(&bench, bench.doc.objects()[1]);
    assert_eq!(bench.doc.undo_steps(), 2, "the mask, then the new object");
}

// ------------------------------------------------------------- panels and demos

#[test]
fn the_panels_build() {
    let ctx = egui::Context::default();
    theme::install(&ctx);
    for mut tool in crate::tools() {
        let mut bench = Bench::new();
        bench.add("Ball", sphere(3, 1.0));
        bench.enter(tool.as_mut());
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            bench.call(|cx| tool.panel(ui, cx));
        });
        assert!(!output.shapes.is_empty(), "{} drew a panel", tool.info().name);
        output.textures_delta.clear();
    }
}

#[test]
fn the_tools_are_listed_with_the_unity_apps_keys() {
    let infos: Vec<_> = crate::tools().iter().map(|tool| tool.info()).collect();
    let listed: Vec<_> = infos.iter().map(|info| (info.id, info.hotkey)).collect();
    let expected = [("mask", Some(Key::N)), ("hide", None), ("trim", None), ("split", Some(Key::K)), ("extract", None)];
    assert_eq!(listed, expected);
    assert!(infos.iter().all(|info| !info.tip.is_empty() && !info.icon.is_empty()));
}

#[test]
fn the_demos_reach_their_states_through_the_gestures() {
    // A lasso mid-draw.
    let (mut bench, mut tool, ball) = bench(Region::mask());
    bench.call(|cx| tool.demo(cx, ""));
    assert!(tool.is_dragging() && tool.shape() == Shape::Lasso);
    assert_eq!(lines(&bench.frame(&mut tool)), 2);
    assert!(mask(&bench, ball).iter().all(|m| *m == 0.0));
    let (mut bench, mut tool, ball) = self::bench(Region::mask());
    bench.call(|cx| tool.demo(cx, "masked"));
    assert!(!tool.is_dragging() && mask(&bench, ball).iter().any(|m| *m > 0.0));

    // Half the model hidden from the side, a hole through the middle, and a
    // box mid-drag.
    let (mut bench, mut tool, ball) = self::bench(Region::hide());
    bench.call(|cx| tool.demo(cx, ""));
    let share = hidden_count(&bench, ball) as f32 / 2048.0;
    assert!(share > 0.4 && share < 0.55, "{share}");
    assert!(status(&bench).0.starts_with("Hid "));
    let (mut bench, mut tool, ball) = self::bench(Region::hide());
    bench.call(|cx| tool.demo(cx, "lasso"));
    assert!(hidden_count(&bench, ball) > 100 && hidden_count(&bench, ball) < 1000);
    let (mut bench, mut tool, ball) = self::bench(Region::hide());
    bench.call(|cx| tool.demo(cx, "drag"));
    assert!(tool.is_dragging() && tool.shape() == Shape::Box && hidden_count(&bench, ball) == 0);

    // A trimmed model: half of it, cut from the side so the face shows.
    for (what, least, most) in [("", 0.45, 0.55), ("lasso", 0.6, 0.97)] {
        let (mut bench, mut tool, ball) = self::bench(Region::trim());
        let whole = volume_of(&bench, ball);
        bench.call(|cx| tool.demo(cx, what));
        assert_closed(&bench, ball);
        let share = volume_of(&bench, ball) / whole;
        assert!(share > least && share < most, "{what}: {share}");
        // The cut face looks toward the camera: some of what was cut away
        // was nearer than anything that is left.
        assert!(bench.doc.bounds(ball).max.z < 0.99, "{what}");
    }
    let (mut bench, mut tool, _) = self::bench(Region::trim());
    bench.call(|cx| tool.demo(cx, "drag"));
    assert!(tool.is_dragging() && tool.shape() == Shape::Line);

    // A split with the piece moved aside.
    let (mut bench, mut tool, ball) = self::bench(Region::split());
    bench.call(|cx| tool.demo(cx, ""));
    let objects = bench.doc.objects();
    assert_eq!(objects.len(), 2);
    assert!(bench.doc.info(objects[1]).unwrap().placement.position.length() > 0.3);
    assert_eq!(bench.doc.info(ball).unwrap().placement, Placement::IDENTITY);

    // An extract preview on a patch masked for the purpose, then made.
    let (mut bench, mut tool, _) = self::bench(Extract::default());
    bench.call(|cx| tool.demo(cx, ""));
    assert!(tool.preview_triangles() > 0);
    assert_eq!(bench.frame(&mut tool).previews().len(), 1);
    let (mut bench, mut tool, _) = self::bench(Extract::default());
    bench.call(|cx| tool.demo(cx, "made"));
    assert_eq!(bench.doc.objects().len(), 2);
}

// ---------------------------------------------------------------------- timing

/// Timing: `cargo test -p sculpt-region --release --target-dir target/w/region -- --ignored --nocapture`.
#[test]
#[ignore]
fn timing_region_tools_on_two_million_triangles() {
    let mut bench = Bench::new();
    let ball = bench.add("Ball", sphere(9, 1.0));
    println!("{} triangles, {} threads", bench.doc.info(ball).unwrap().triangles, rayon::current_num_threads());

    let mut tool = Region::mask();
    bench.enter(&mut tool);
    let start = Instant::now();
    bench.drag(&mut tool, Vec2::ZERO, Vec2::new(CENTRE.x, 600.0));
    println!("box mask, press to masked: {:.1} ms ({})", start.elapsed().as_secs_f64() * 1e3, status(&bench).0);

    let mut tool = trim_tool(Shape::Line);
    bench.enter(&mut tool);
    let start = Instant::now();
    bench.drag(&mut tool, Vec2::new(CENTRE.x + 20.0, 30.0), Vec2::new(CENTRE.x - 30.0, 570.0));
    let handed_off = start.elapsed();
    assert!(tool.is_busy());
    frames_until(&mut bench, &mut tool, |bench| bench.doc.undo_steps() == 2);
    println!(
        "line trim: {:.1} ms on the interface thread, landed after {:.1} ms ({})",
        handed_off.as_secs_f64() * 1e3,
        start.elapsed().as_secs_f64() * 1e3,
        status(&bench).0
    );
    assert!(bench.undo(&mut tool));

    let mut tool = Region::split();
    bench.enter(&mut tool);
    bench.call(|cx| tool.set_shape(cx, Shape::Lasso));
    let bite = circle(bench.px(Vec3::new(0.9, 0.1, 0.0)), 80.0, 200);
    let start = Instant::now();
    stroke(&mut bench, &mut tool, &bite);
    frames_until(&mut bench, &mut tool, |bench| bench.doc.objects().len() == 2);
    println!("lasso split, press to two objects: {:.1} ms ({})", start.elapsed().as_secs_f64() * 1e3, status(&bench).0);

    let mut tool = Extract::default();
    bench.env.time += 10.0;
    bench.enter(&mut tool);
    let start = Instant::now();
    bench.frame(&mut tool);
    let started = start.elapsed();
    while tool.preview_triangles() == 0 {
        bench.frame(&mut tool);
        std::thread::sleep(Duration::from_millis(1));
    }
    println!(
        "extract preview: {:.1} ms to start, shown after {:.1} ms ({} triangles)",
        started.as_secs_f64() * 1e3,
        start.elapsed().as_secs_f64() * 1e3,
        tool.preview_triangles()
    );
}
