//! The tool driven the way the app drives it, against the toolkit's bench.

use std::time::{Duration, Instant};

use glam::{Mat4, Quat, Vec2, Vec3};
use sculpt_toolkit::testing::{Bench, cube};
use sculpt_toolkit::{Button, Command, Draw, Event, Key, ObjectId, Placement, Scene, Shape, Tool, theme};

use crate::testing::uv_sphere;
use crate::{Apply, ArrayKind, ArrayTool};

/// A unit cube in front of the camera (which looks down -Z with +X to the
/// right) and the tool brought up on it.
fn setup() -> (Bench, ArrayTool, ObjectId) {
    let mut bench = Bench::new();
    let object = bench.add("Cube", cube(0.5));
    let mut tool = ArrayTool::default();
    bench.enter(&mut tool);
    (bench, tool, object)
}

fn circle() -> (Bench, ArrayTool, ObjectId) {
    let (mut bench, mut tool, object) = setup();
    bench.call(|cx| tool.make_circle(cx));
    (bench, tool, object)
}

fn line() -> (Bench, ArrayTool, ObjectId) {
    let (mut bench, mut tool, object) = setup();
    bench.call(|cx| tool.make_line(cx));
    (bench, tool, object)
}

fn copies(draw: &Draw, object: ObjectId) -> Vec<Mat4> {
    draw.object_copies().iter().filter(|(id, _)| *id == object).flat_map(|(_, m)| m.clone()).collect()
}

fn dots(draw: &Draw) -> Vec<(Vec2, bool)> {
    draw.overlay
        .shapes
        .iter()
        .filter_map(|shape| match shape {
            Shape::Dot { center, square, .. } => Some((*center, *square)),
            _ => None,
        })
        .collect()
}

fn translation(m: &Mat4) -> Vec3 {
    m.w_axis.truncate()
}

fn point(tool: &ArrayTool, object: ObjectId, index: usize) -> Vec3 {
    tool.state(object).points[index]
}

fn settle(bench: &mut Bench, tool: &mut ArrayTool) {
    // Past the moment a run of slider ticks waits before it is one step.
    bench.env.time += 1.0;
    bench.frame(tool);
}

// ---- making arrays --------------------------------------------------------------------------

#[test]
fn a_line_starts_at_the_object_and_runs_right_across_the_screen() {
    let (mut bench, mut tool, object) = line();
    let state = tool.state(object);
    assert_eq!((state.settings.kind, state.settings.count), (ArrayKind::Line, 5));
    assert!(state.points[1].distance(Vec3::new(4.4, 0.0, 0.0)) < 1e-4, "{:?}", state.points[1]);
    assert_eq!(tool.picked(), Some(1), "the end point is picked: it is the one to drag");

    // The object itself is the scene's own; the four others are drawn again.
    let draw = bench.frame(&mut tool);
    let m = copies(&draw, object);
    assert_eq!(m.len(), 4);
    assert!(translation(&m[3]).distance(Vec3::new(4.4, 0.0, 0.0)) < 1e-3, "{:?}", translation(&m[3]));
    // A handle on each of the two points, the first square.
    assert_eq!(dots(&draw).iter().map(|d| d.1).collect::<Vec<_>>(), [true, false]);
}

#[test]
fn a_circle_goes_counter_clockwise_from_the_object_on_screen() {
    let (mut bench, mut tool, object) = circle();
    let state = tool.state(object);
    assert_eq!((state.settings.kind, state.settings.count, state.settings.axis), (ArrayKind::Radial, 8, 2));
    assert!(state.points[1].x < 0.0, "the centre is to the left of the object");

    let draw = bench.frame(&mut tool);
    let m = copies(&draw, object);
    assert_eq!(m.len(), 7);
    // Copy 1 is up and to the left of the object, copy 2 higher still and
    // further left, copy 4 is across the ring: counter-clockwise from the right.
    let at = |i: usize| translation(&m[i - 1]);
    assert!(at(1).y > 0.0 && at(1).x < 0.0, "{:?}", at(1));
    assert!(at(2).y > at(1).y);
    assert!(at(4).x < -2.0 && at(4).y.abs() < 1e-3, "the far side of the ring: {:?}", at(4));
    assert!(at(6).y < 0.0, "and back round underneath");
    // The ring and its spoke and axis are drawn, with a square on each of the two points.
    assert_eq!(dots(&draw).len(), 2);
    assert!(draw.overlay.shapes.iter().any(|s| matches!(s, Shape::Line { dash: Some(_), .. })), "the axis is dashed");
}

#[test]
fn drawing_a_curve_with_the_pen_makes_a_curve_array_that_starts_on_the_object() {
    let (mut bench, mut tool, object) = setup();
    bench.call(|cx| tool.set_armed(cx, true));
    let at = |x: f32, y: f32| bench.px(Vec3::new(x, y, 0.0));
    let start = at(-2.0, 0.0);
    assert!(bench.press(&mut tool, start, Button::Left), "armed, a press anywhere is the stroke's");
    for i in 1..=40 {
        let t = i as f32 / 40.0;
        let p = bench.px(Vec3::new(-2.0 + 4.0 * t, 1.0 * (t * 6.0).sin(), 0.0));
        bench.drag_to(&mut tool, p);
    }
    let draw = bench.frame(&mut tool);
    assert!(
        draw.overlay.shapes.iter().any(|s| matches!(s, Shape::Line { width, .. } if *width == 1.5)),
        "the stroke so far is drawn"
    );
    bench.release(&mut tool);

    let state = tool.state(object);
    assert_eq!(state.settings.kind, ArrayKind::Curve);
    assert!(!tool.is_armed(), "drawing is over: Draw Curve has to be pressed again");
    assert_eq!(state.points[0], Vec3::ZERO, "the path begins on the object wherever the pen began");
    assert!(state.points.len() >= 4, "{} points", state.points.len());
    assert!(state.points.last().unwrap().x > 3.5, "and ends where the pen ended: {:?}", state.points.last());
    assert!(state.settings.count >= 3);
    // One step of the tool's own history.
    assert!(bench.undo(&mut tool));
    assert!(tool.state(object).is_none());
}

#[test]
fn a_stroke_that_ends_where_it_began_is_a_closed_loop() {
    let (mut bench, mut tool, object) = setup();
    bench.call(|cx| tool.set_armed(cx, true));
    let tau = std::f32::consts::TAU;
    let ring: Vec<Vec2> = (0..=60)
        .map(|i| {
            bench.px(Vec3::new(-2.0 + 1.5 * (i as f32 / 60.0 * tau).cos(), 1.5 * (i as f32 / 60.0 * tau).sin(), 0.0))
        })
        .collect();
    bench.press(&mut tool, ring[0], Button::Left);
    for &p in &ring[1..] {
        bench.drag_to(&mut tool, p);
    }
    bench.release(&mut tool);
    let state = tool.state(object);
    assert!(state.settings.closed && state.settings.kind == ArrayKind::Curve);
}

#[test]
fn a_stroke_that_is_too_short_makes_nothing_and_leaves_drawing_armed() {
    let (mut bench, mut tool, object) = setup();
    bench.call(|cx| tool.set_armed(cx, true));
    let from = bench.px(Vec3::ZERO);
    bench.drag(&mut tool, from, from + Vec2::new(10.0, 3.0));
    assert!(tool.state(object).is_none());
    assert!(tool.is_armed());
    assert_eq!(bench.status.as_ref().map(|s| s.1), Some(false), "it says to drag further");
}

#[test]
fn drawing_over_an_existing_array_keeps_its_count_and_sizes() {
    let (mut bench, mut tool, object) = circle();
    bench.call(|cx| tool.edit(cx, "Array Count", false, |a| a.settings.count = 11));
    bench.call(|cx| tool.set_armed(cx, true));
    bench.drag(&mut tool, bench.px(Vec3::new(-2.0, 0.0, 0.0)), bench.px(Vec3::new(2.0, 1.0, 0.0)));
    let state = tool.state(object);
    assert_eq!((state.settings.kind, state.settings.count), (ArrayKind::Curve, 11));
}

// ---- the handles ----------------------------------------------------------------------------

#[test]
fn dragging_a_control_point_moves_it_on_the_plane_facing_the_camera() {
    let (mut bench, mut tool, object) = circle();
    let before = point(&tool, object, 1);
    let from = bench.px(before);
    assert!(bench.press(&mut tool, from, Button::Left), "a handle is the tool's");
    bench.release(&mut tool);

    bench.drag(&mut tool, from, bench.px(Vec3::new(-2.4, 0.5, 0.0)));
    let after = point(&tool, object, 1);
    assert!(after.distance(Vec3::new(-2.4, 0.5, 0.0)) < 0.02, "{after:?}");
    assert_eq!(after.z, before.z, "depth unchanged");
    // Copy 0 is the object and never moves with a point.
    assert_eq!(point(&tool, object, 0), Vec3::ZERO);
    // The ring is bigger now: the copy opposite the object is further away.
    let draw = bench.frame(&mut tool);
    assert!(translation(&copies(&draw, object)[3]).length() > 3.5);
}

#[test]
fn a_handle_stays_under_the_pointer_rather_than_jumping_to_it() {
    let (mut bench, mut tool, object) = circle();
    let at = point(&tool, object, 1);
    let grabbed = bench.px(at) + Vec2::new(7.0, -5.0);
    bench.drag(&mut tool, grabbed, grabbed + Vec2::new(0.0, 100.0));
    let moved = bench.px(point(&tool, object, 1)) - bench.px(at);
    assert!(moved.distance(Vec2::new(0.0, 100.0)) < 1.0, "moved by the pointer's own travel: {moved:?}");
}

#[test]
fn dragging_the_start_point_moves_the_object_and_its_array_as_one_scene_step() {
    let (mut bench, mut tool, object) = circle();
    let state = tool.state(object);
    let start = bench.px(Vec3::ZERO);
    assert!(bench.press(&mut tool, start, Button::Left));
    bench.drag_to(&mut tool, bench.px(Vec3::new(0.0, 1.0, 0.0)));

    // While it is dragged the object is drawn where the drag has it, as the
    // first of the copies, and the scene's own is left out; nothing is committed.
    let draw = bench.frame(&mut tool);
    assert_eq!(draw.hidden(), [object]);
    let m = copies(&draw, object);
    assert_eq!(m.len(), 8);
    assert!(translation(&m[0]).distance(Vec3::new(0.0, 1.0, 0.0)) < 0.02, "{:?}", translation(&m[0]));
    assert_eq!(bench.doc.undo_steps(), 0);

    bench.release(&mut tool);
    let moved = bench.doc.info(object).unwrap().placement.position;
    assert!(moved.distance(Vec3::new(0.0, 1.0, 0.0)) < 0.02, "{moved:?}");
    assert_eq!(bench.doc.undo_steps(), 1, "one step of the scene's history");
    assert_eq!(tool.history.undo.len(), 1, "and none added to the tool's own: only the circle is there");
    assert_eq!(tool.state(object), state, "the array is relative to the object, so it came along");
    let draw = bench.frame(&mut tool);
    assert!(draw.hidden().is_empty());
    assert_eq!(copies(&draw, object).len(), 7);

    // Z steps the tool's own history first (the circle), then the scene's, which
    // puts the object back; the array follows it home.
    assert!(bench.undo(&mut tool));
    assert!(tool.state(object).is_none());
    assert!(bench.undo(&mut tool));
    assert_eq!(bench.doc.info(object).unwrap().placement.position, Vec3::ZERO);
}

#[test]
fn escape_during_a_drag_puts_the_point_back() {
    let (mut bench, mut tool, object) = circle();
    let before = point(&tool, object, 1);
    bench.press(&mut tool, bench.px(before), Button::Left);
    bench.drag_to(&mut tool, bench.px(Vec3::new(-3.0, 1.0, 0.0)));
    assert!(point(&tool, object, 1).distance(before) > 1.0);
    assert!(bench.key(&mut tool, Key::Escape), "Esc is the drag's first");
    bench.release(&mut tool);
    assert_eq!(point(&tool, object, 1), before);
    assert!(!tool.is_dragging());
    // The drag left no step behind: the next Z takes back the circle itself.
    assert!(bench.undo(&mut tool));
    assert!(tool.state(object).is_none());
}

#[test]
fn escape_cancels_then_stops_drawing_then_lets_go_of_the_point_then_leaves() {
    let (mut bench, mut tool, object) = circle();
    bench.call(|cx| tool.set_armed(cx, true));
    assert!(tool.picked().is_none(), "arming lets go of the picked point");
    assert!(bench.key(&mut tool, Key::Escape), "stops drawing");
    assert!(!tool.is_armed());
    let centre = bench.px(point(&tool, object, 1));
    bench.click(&mut tool, centre, Button::Left);
    assert!(tool.picked().is_some());
    assert!(bench.key(&mut tool, Key::Escape), "lets go of the picked point");
    assert!(tool.picked().is_none());
    assert!(!bench.key(&mut tool, Key::Escape), "nothing left: the app puts the tool away");
}

#[test]
fn clicking_the_path_adds_a_point_exactly_where_the_path_already_is() {
    let (mut bench, mut tool, object) = line();
    let before = crate::path::evaluate(&tool.state(object).points, false, 0, 0.5);
    let on_path = bench.px(before);
    assert!(bench.press(&mut tool, on_path, Button::Left), "the path is the tool's");
    bench.release(&mut tool);
    let state = tool.state(object);
    assert_eq!(state.points.len(), 3);
    assert_eq!(state.settings.kind, ArrayKind::Curve, "a line with a point in the middle is a curve");
    assert!(state.points[1].distance(before) < 0.05, "{:?} vs {before:?}", state.points[1]);
    assert_eq!(tool.picked(), Some(1));
    assert!(bench.undo(&mut tool));
    assert_eq!(tool.state(object).points.len(), 2);
}

#[test]
fn a_new_point_can_be_dragged_straight_away() {
    let (mut bench, mut tool, object) = line();
    let middle = bench.px(crate::path::evaluate(&tool.state(object).points, false, 0, 0.5));
    bench.drag(&mut tool, middle, middle + Vec2::new(0.0, -80.0));
    let state = tool.state(object);
    assert_eq!(state.points.len(), 3);
    assert!(state.points[1].y > 0.3, "pulled up: {:?}", state.points[1]);
    // The whole drag, insertion included, is one step.
    assert!(bench.undo(&mut tool));
    assert_eq!(tool.state(object).points.len(), 2);
    assert!(bench.undo(&mut tool));
    assert!(tool.state(object).is_none(), "and the step before it was the line itself");
}

#[test]
fn shift_drag_on_the_last_point_extends_the_path_from_there() {
    let (mut bench, mut tool, object) = line();
    let end = point(&tool, object, 1);
    bench.env.mods.shift = true;
    bench.drag(&mut tool, bench.px(end), bench.px(end + Vec3::new(0.0, 1.5, 0.0)));
    let state = tool.state(object);
    assert_eq!(state.points.len(), 3);
    assert_eq!(state.points[1], end, "the old end stays");
    assert!(state.points[2].distance(end + Vec3::new(0.0, 1.5, 0.0)) < 0.03);
    assert_eq!(state.settings.kind, ArrayKind::Curve);
}

#[test]
fn a_point_is_deleted_by_right_click_or_the_delete_key_but_not_the_ones_that_must_stay() {
    let (mut bench, mut tool, object) = setup();
    bench.call(|cx| tool.make_line(cx));
    // Make it a curve with four points.
    for t in [0.33, 0.66] {
        let on = bench.px(crate::path::evaluate(&tool.state(object).points, false, 0, t));
        bench.click(&mut tool, on, Button::Left);
    }
    assert_eq!(tool.state(object).points.len(), 4);

    // Right-click the second point.
    let victim = bench.px(point(&tool, object, 2));
    assert!(bench.press(&mut tool, victim, Button::Right));
    bench.release(&mut tool);
    assert_eq!(tool.state(object).points.len(), 3);

    // Pick another and press Delete.
    let second = bench.px(point(&tool, object, 1));
    bench.click(&mut tool, second, Button::Left);
    assert_eq!(tool.picked(), Some(1));
    assert!(bench.key(&mut tool, Key::Delete));
    assert_eq!(tool.state(object).points.len(), 2);

    // Two points is the least a path has, and the object's own cannot go.
    let last = bench.px(point(&tool, object, 1));
    bench.click(&mut tool, last, Button::Left);
    bench.key(&mut tool, Key::Delete);
    assert_eq!(tool.state(object).points.len(), 2);
    assert_eq!(bench.status.as_ref().map(|s| s.1), Some(false));
    bench.press(&mut tool, bench.px(Vec3::ZERO), Button::Right);
    assert_eq!(tool.state(object).points.len(), 2);
}

#[test]
fn a_circle_cannot_lose_its_centre() {
    let (mut bench, mut tool, object) = circle();
    let centre = bench.px(point(&tool, object, 1));
    bench.press(&mut tool, centre, Button::Right);
    assert!(!tool.state(object).is_none());
    assert_eq!(tool.state(object).points.len(), 2);
    assert_eq!(bench.status.as_ref().map(|s| s.1), Some(false));
}

#[test]
fn a_press_on_nothing_of_the_tools_is_passed_on_so_the_camera_orbits() {
    // No array yet: the whole viewport is the camera's, the model included.
    let (mut bench, mut tool, _) = setup();
    assert!(!bench.press(&mut tool, bench.px(Vec3::new(0.1, 0.1, 0.5)), Button::Left));
    bench.release(&mut tool);
    assert!(!bench.press(&mut tool, Vec2::new(20.0, 20.0), Button::Left));
    bench.release(&mut tool);

    // With one, only its handles and path are the tool's.
    bench.call(|cx| tool.make_circle(cx));
    assert!(!bench.press(&mut tool, Vec2::new(20.0, 20.0), Button::Left), "empty background");
    bench.release(&mut tool);
    assert!(!bench.press(&mut tool, bench.px(Vec3::new(0.2, 0.2, 0.5)), Button::Left), "the model, off every handle");
    bench.release(&mut tool);
    assert!(!tool.is_dragging());
}

#[test]
fn a_click_on_another_object_switches_to_it_without_an_undo_step() {
    let (mut bench, mut tool, first) = circle();
    let second = bench.add("Other", cube(0.2));
    bench.call(|cx| cx.commit("Select", vec![Command::Select { object: first }]));
    bench.call(|cx| {
        cx.commit("Place", vec![Command::Place { object: second, placement: Placement::at(Vec3::new(0.0, -1.2, 0.0)) }])
    });
    let steps = bench.doc.undo_steps();
    bench.click(&mut tool, bench.px(Vec3::new(0.0, -1.2, 0.2)), Button::Left);
    assert_eq!(bench.doc.active(), second);
    assert_eq!(bench.doc.undo_steps(), steps);
    // Its handles are not the first object's, and the first keeps its array.
    bench.frame(&mut tool);
    assert!(tool.picked().is_none());
    assert!(tool.has_array(first) && !tool.has_array(second));
}

#[test]
fn a_click_on_empty_space_lets_go_of_the_picked_point() {
    let (mut bench, mut tool, _) = circle();
    assert!(tool.picked().is_some());
    bench.click(&mut tool, Vec2::new(20.0, 20.0), Button::Left);
    assert!(tool.picked().is_none());
}

// ---- the tool's own history -----------------------------------------------------------------

#[test]
fn z_steps_the_tools_own_history_and_then_falls_through_to_the_scene() {
    let (mut bench, mut tool, object) = circle();
    let made = tool.state(object);
    bench.call(|cx| tool.edit(cx, "Array Count", false, |a| a.settings.count = 12));
    bench.call(|cx| tool.edit(cx, "Array Follow", false, |a| a.settings.follow = false));
    assert!(tool.can_undo(false) && !tool.can_undo(true));

    assert!(bench.undo(&mut tool));
    assert!(tool.state(object).settings.follow);
    assert_eq!(tool.state(object).settings.count, 12);
    assert!(tool.can_undo(true));
    assert!(bench.undo(&mut tool));
    assert_eq!(tool.state(object).settings.count, 8);
    assert!(bench.undo(&mut tool), "the circle itself is a step");
    assert!(tool.state(object).is_none());
    assert!(!tool.can_undo(false));
    assert!(!bench.undo(&mut tool), "nothing in the tool's history, and nothing in the scene's");
    assert_eq!(bench.doc.undo_steps(), 0, "the tool never touched the scene's history");

    for _ in 0..3 {
        assert!(bench.redo(&mut tool));
    }
    assert_eq!(tool.state(object).settings.count, 12);
    assert!(!tool.state(object).settings.follow);
    assert_ne!(tool.state(object), made);
    assert!(!bench.redo(&mut tool));
}

#[test]
fn a_new_edit_after_an_undo_forgets_what_was_undone() {
    let (mut bench, mut tool, object) = circle();
    bench.call(|cx| tool.edit(cx, "Array Count", false, |a| a.settings.count = 12));
    bench.undo(&mut tool);
    bench.call(|cx| tool.edit(cx, "Array Count", false, |a| a.settings.count = 4));
    assert!(!tool.can_undo(true));
    assert_eq!(tool.state(object).settings.count, 4);
}

#[test]
fn a_run_of_slider_ticks_is_one_step() {
    let (mut bench, mut tool, object) = circle();
    for count in [9, 10, 11, 12] {
        bench.call(|cx| tool.edit(cx, "Array Count", true, |a| a.settings.count = count));
        bench.frame(&mut tool);
    }
    assert_eq!(tool.state(object).settings.count, 12);
    // Z while the run is still open closes it first, then takes it all back.
    assert!(tool.can_undo(false));
    settle(&mut bench, &mut tool);
    assert!(bench.undo(&mut tool));
    assert_eq!(tool.state(object).settings.count, 8, "all four ticks in one Z");
    assert!(bench.undo(&mut tool));
    assert!(tool.state(object).is_none(), "the next is the circle");
}

#[test]
fn undoing_during_an_open_edit_closes_it_first() {
    let (mut bench, mut tool, object) = circle();
    bench.call(|cx| tool.edit(cx, "Array Count", true, |a| a.settings.count = 12));
    assert!(bench.undo(&mut tool));
    assert_eq!(tool.state(object).settings.count, 8);
    assert!(bench.redo(&mut tool));
    assert_eq!(tool.state(object).settings.count, 12);
}

#[test]
fn an_edit_that_changes_nothing_is_not_a_step() {
    let (mut bench, mut tool, _) = circle();
    bench.call(|cx| tool.edit(cx, "Array Count", false, |a| a.settings.count = 8));
    assert!(bench.undo(&mut tool));
    assert!(!tool.can_undo(false), "that undid the circle; the no-op was never a step");
}

#[test]
fn remove_array_takes_it_off_and_z_brings_it_back() {
    let (mut bench, mut tool, object) = circle();
    let made = tool.state(object);
    bench.call(|cx| tool.remove(cx));
    assert!(!tool.has_array(object));
    assert!(bench.undo(&mut tool));
    assert_eq!(tool.state(object), made);
}

#[test]
fn z_in_the_middle_of_a_stroke_cancels_it() {
    let (mut bench, mut tool, object) = setup();
    bench.call(|cx| tool.set_armed(cx, true));
    bench.press(&mut tool, bench.px(Vec3::new(-2.0, 0.0, 0.0)), Button::Left);
    bench.drag_to(&mut tool, bench.px(Vec3::new(0.0, 1.0, 0.0)));
    assert!(bench.undo(&mut tool));
    bench.release(&mut tool);
    assert!(tool.state(object).is_none() && !tool.is_dragging());
}

// ---- settings -------------------------------------------------------------------------------

#[test]
fn changing_the_count_sizes_and_sweep_moves_the_copies_without_rebuilding_anything() {
    let (mut bench, mut tool, object) = circle();
    bench.call(|cx| tool.edit(cx, "Array Count", false, |a| a.settings.count = 4));
    bench.call(|cx| tool.edit(cx, "Array End Size", false, |a| a.settings.end_scale = 0.5));
    let draw = bench.frame(&mut tool);
    let m = copies(&draw, object);
    assert_eq!(m.len(), 3);
    // The last copy is half the size of the object.
    let scale = m[2].x_axis.truncate().length();
    assert!((scale - 0.5).abs() < 1e-4, "{scale}");

    bench.call(|cx| tool.edit(cx, "Array Sweep", false, |a| a.settings.sweep = 90.0));
    let draw = bench.frame(&mut tool);
    let last = translation(&copies(&draw, object)[2]);
    // A quarter turn of the ring ends a quarter of the way round.
    let radius = tool.state(object).points[1].length();
    assert!(last.distance(Vec3::new(-radius, radius, 0.0)) < 0.01, "{last:?}");
}

#[test]
fn the_axis_can_be_changed_and_a_centre_that_would_sit_on_it_swings_out() {
    let (mut bench, mut tool, object) = circle();
    // The ring goes round Z with its centre on -X. Round X would put the centre on the axis.
    bench.call(|cx| tool.edit(cx, "Array Axis", false, |a| a.set_axis(0)));
    let state = tool.state(object);
    assert_eq!(state.settings.axis, 0);
    assert!(state.points[1].x.abs() < 1e-6 && state.points[1].length() > 1.0, "{:?}", state.points[1]);
}

#[test]
fn only_a_curve_can_be_closed() {
    let (mut bench, mut tool, object) = line();
    bench.call(|cx| tool.edit(cx, "Array Closed Loop", false, |a| a.settings.closed = true));
    assert!(!tool.state(object).settings.closed);
}

// ---- the copies stay on screen --------------------------------------------------------------

#[test]
fn the_copies_are_still_drawn_after_the_tool_is_put_away_and_the_handles_are_not() {
    let (mut bench, mut tool, object) = circle();
    assert!(!bench.frame(&mut tool).overlay.shapes.is_empty());
    bench.leave(&mut tool);
    bench.env.active = false;
    let draw = bench.frame(&mut tool);
    assert_eq!(copies(&draw, object).len(), 7, "the copies are the object's own mesh, drawn again");
    assert!(draw.overlay.shapes.is_empty(), "no path or handles when the tool is away");

    // Back up again, and it is all still there.
    bench.env.active = true;
    bench.enter(&mut tool);
    assert!(tool.has_array(object));
    assert!(!bench.frame(&mut tool).overlay.shapes.is_empty());
}

#[test]
fn the_copies_follow_the_object_when_it_is_moved_turned_or_resized() {
    let (mut bench, mut tool, object) = circle();
    let at_rest = copies(&bench.frame(&mut tool), object);

    let placement =
        Placement { position: Vec3::new(1.0, 2.0, 0.0), rotation: Quat::from_rotation_z(0.5), scale: Vec3::splat(2.0) };
    bench.call(|cx| cx.commit("Place", vec![Command::Place { object, placement }]));
    let moved = copies(&bench.frame(&mut tool), object);
    // Copy k is the object's matrix with the array's own placement on top.
    let state = tool.state(object);
    let layout = crate::compute_layout(&state.settings, &state.points);
    for (k, m) in moved.iter().enumerate() {
        let expected =
            layout[k + 1].world_matrix(&crate::Pose::new(placement.position, placement.rotation, placement.scale));
        assert!(m.abs_diff_eq(expected, 1e-5), "copy {}", k + 1);
    }
    assert_ne!(moved[0], at_rest[0]);
}

#[test]
fn a_hidden_object_shows_no_copies_and_a_deleted_one_comes_back_with_its_array() {
    let (mut bench, mut tool, object) = circle();
    let other = bench.add("Other", cube(0.1));
    bench.call(|cx| cx.commit("Hide", vec![Command::Show { object, visible: false }]));
    assert!(copies(&bench.frame(&mut tool), object).is_empty());
    bench.call(|cx| cx.commit("Show", vec![Command::Show { object, visible: true }]));

    bench.call(|cx| cx.commit("Delete", vec![Command::Delete { object }]));
    assert!(copies(&bench.frame(&mut tool), object).is_empty(), "nothing to draw them from");
    assert!(tool.has_array(object), "but the array is kept");
    assert!(bench.undo_scene());
    assert_eq!(copies(&bench.frame(&mut tool), object).len(), 7);
    assert!(bench.doc.exists(other));
}

#[test]
fn every_object_keeps_its_own_array() {
    let (mut bench, mut tool, first) = circle();
    let second = bench.add("Second", cube(0.3));
    bench.frame(&mut tool);
    bench.call(|cx| tool.make_line(cx));
    assert_eq!(tool.state(first).settings.kind, ArrayKind::Radial);
    assert_eq!(tool.state(second).settings.kind, ArrayKind::Line);
    let draw = bench.frame(&mut tool);
    assert_eq!((copies(&draw, first).len(), copies(&draw, second).len()), (7, 4));
    // The handles shown are the selected object's.
    assert_eq!(dots(&draw).len(), 2);
}

// ---- making it real -------------------------------------------------------------------------

#[test]
fn making_it_real_as_one_object_is_one_undo_step_that_swaps_in_every_copy() {
    let (mut bench, mut tool, object) = circle();
    let made = tool.state(object);
    let before = bench.doc.info(object).unwrap().mesh;
    bench.call(|cx| tool.apply(cx, Apply::One));

    let info = bench.doc.info(object).unwrap();
    assert_eq!((info.vertices, info.triangles), (64, 96), "eight copies of a cube");
    assert_ne!(info.mesh, before);
    assert_eq!(bench.doc.objects().len(), 1);
    assert_eq!(bench.doc.undo_steps(), 1, "a single step of the scene's history");
    assert!(bench.finished, "the tool is put away, back to sculpting");
    assert!(!tool.has_array(object), "the array is spent");
    assert_eq!(bench.status.as_ref().map(|s| s.1), Some(true));
    assert!(copies(&bench.frame(&mut tool), object).is_empty(), "no live copies drawn on top of the real ones");

    // The baked copies are where the live ones were.
    let mesh = bench.doc.mesh(object).unwrap();
    let layout = crate::compute_layout(&made.settings, &made.points);
    let bounds = sculpt_geom::Aabb::of(&mesh.positions[8 * 4..8 * 5]);
    let expected = layout[4].offset;
    assert!(bounds.center().distance(expected) < 1e-4, "copy 4 sits at {expected:?}, found {:?}", bounds.center());

    // Z gives the object back and hands the live array back with it.
    assert!(bench.undo(&mut tool));
    assert_eq!(bench.doc.info(object).unwrap().mesh, before);
    assert!(!tool.has_array(object), "until the tool next looks");
    bench.frame(&mut tool);
    assert_eq!(tool.state(object), made);
    assert_eq!(copies(&bench.frame(&mut tool), object).len(), 7);

    // Shift+Z does it again, and the array is spent again.
    assert!(bench.redo(&mut tool));
    bench.frame(&mut tool);
    assert!(!tool.has_array(object));
    assert_eq!(bench.doc.info(object).unwrap().vertices, 64);
}

#[test]
fn making_it_real_as_separate_objects_adds_one_per_copy_in_one_undo_step() {
    let (mut bench, mut tool, object) = circle();
    let made = tool.state(object);
    bench.call(|cx| tool.apply(cx, Apply::Separate));

    assert_eq!(bench.doc.objects().len(), 8);
    assert_eq!(bench.doc.undo_steps(), 1);
    assert_eq!(bench.doc.active(), object, "the original stays selected");
    let layout = crate::compute_layout(&made.settings, &made.points);
    let names: Vec<String> =
        bench.doc.objects().iter().map(|&id| bench.doc.info(id).unwrap().name.to_owned()).collect();
    assert_eq!(names[..3], ["Cube", "Cube 2", "Cube 3"]);
    for (k, &id) in bench.doc.objects().iter().enumerate().skip(1) {
        let info = bench.doc.info(id).unwrap();
        assert_eq!((info.vertices, info.triangles), (8, 12), "a whole mesh of its own");
        assert!(info.placement.position.distance(layout[k].offset) < 1e-5, "copy {k}");
        assert!((info.placement.scale.x - layout[k].scale).abs() < 1e-5);
        assert!(info.visible);
    }

    assert!(bench.undo(&mut tool));
    assert_eq!(bench.doc.objects().len(), 1);
    bench.frame(&mut tool);
    assert_eq!(tool.state(object), made, "the live array is back");
    assert!(bench.redo(&mut tool));
    bench.frame(&mut tool);
    assert_eq!(bench.doc.objects().len(), 8);
    assert!(!tool.has_array(object));
}

#[test]
fn a_hidden_object_makes_hidden_copies() {
    let (mut bench, mut tool, object) = circle();
    bench.call(|cx| cx.commit("Hide", vec![Command::Show { object, visible: false }]));
    bench.call(|cx| tool.apply(cx, Apply::Separate));
    assert!(bench.doc.objects().iter().all(|&id| !bench.doc.info(id).unwrap().visible));
}

#[test]
fn a_masked_object_keeps_its_mask_on_every_copy() {
    let (mut bench, mut tool, object) = circle();
    bench.call(|cx| cx.commit("Mask", vec![Command::Mask { object, mask: vec![0.5; 8] }]));
    bench.call(|cx| tool.apply(cx, Apply::One));
    let mask = bench.doc.mesh(object).unwrap().mask.to_vec();
    assert_eq!(mask.len(), 64);
    assert!(mask.iter().all(|&m| m == 0.5));
}

#[test]
fn applying_with_no_array_says_so_and_changes_nothing() {
    let (mut bench, mut tool, _) = setup();
    bench.call(|cx| tool.apply(cx, Apply::One));
    assert_eq!(bench.doc.undo_steps(), 0);
    assert!(!bench.finished);
    assert_eq!(bench.status.as_ref().map(|s| s.1), Some(false));
}

#[test]
fn too_much_to_apply_is_refused_with_a_reason_and_nothing_changes() {
    // A quarter of a million vertices a copy, a hundred copies: past what one mesh holds.
    let mut bench = Bench::new();
    let object = bench.add("Ball", uv_sphere(500, 500, 0.5));
    let mut tool = ArrayTool::default();
    bench.enter(&mut tool);
    bench.call(|cx| tool.make_line(cx));
    bench.call(|cx| tool.edit(cx, "Array Count", false, |a| a.settings.count = 100));
    bench.call(|cx| tool.apply(cx, Apply::One));
    assert!(!tool.is_building() && !bench.finished);
    assert_eq!(bench.doc.undo_steps(), 0);
    assert!(tool.has_array(object));
    let (text, ok) = bench.status.clone().unwrap();
    assert!(!ok && text.contains("Too big for one object"), "{text}");
}

#[test]
fn a_large_apply_runs_as_a_job_while_the_live_array_stays_up_and_lands_in_a_later_frame() {
    let mut bench = Bench::new();
    let object = bench.add("Ball", uv_sphere(220, 220, 0.5));
    let mut tool = ArrayTool::default();
    bench.enter(&mut tool);
    bench.call(|cx| tool.make_line(cx));
    let vertices = bench.doc.info(object).unwrap().vertices;
    bench.call(|cx| tool.edit(cx, "Array Count", false, |a| a.settings.count = 12));
    assert!(vertices * 12 > 400_000, "big enough to go to a job");

    bench.call(|cx| tool.apply(cx, Apply::One));
    assert!(tool.is_building());
    assert_eq!(bench.doc.undo_steps(), 0, "nothing committed yet");
    assert!(tool.has_array(object), "the live array stays up while it builds");
    assert!(!bench.finished);

    let start = Instant::now();
    while tool.is_building() {
        assert!(start.elapsed() < Duration::from_secs(60), "the job never landed");
        bench.frame(&mut tool);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(bench.doc.info(object).unwrap().vertices, vertices * 12);
    assert_eq!(bench.doc.undo_steps(), 1);
    assert!(bench.finished && !tool.has_array(object));
}

#[test]
fn a_build_that_finds_the_object_changed_underneath_it_applies_nothing() {
    let mut bench = Bench::new();
    let object = bench.add("Ball", uv_sphere(220, 220, 0.5));
    let mut tool = ArrayTool::default();
    bench.enter(&mut tool);
    bench.call(|cx| tool.make_line(cx));
    bench.call(|cx| tool.edit(cx, "Array Count", false, |a| a.settings.count = 12));
    bench.call(|cx| tool.apply(cx, Apply::One));
    assert!(tool.is_building());
    // Something else replaces the mesh while the copies are being made.
    bench.call(|cx| cx.commit("Remesh", vec![Command::Replace { object, mesh: cube(0.5).into() }]));
    let start = Instant::now();
    while tool.is_building() {
        assert!(start.elapsed() < Duration::from_secs(60));
        bench.frame(&mut tool);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(bench.doc.info(object).unwrap().vertices, 8, "the new mesh is untouched");
    assert_eq!(bench.doc.undo_steps(), 1, "only the replace");
    assert_eq!(bench.status.as_ref().map(|s| s.1), Some(false));
    assert!(tool.has_array(object), "and the array is still there to apply again");
}

#[test]
fn hovering_lights_a_handle_and_shows_where_a_click_on_the_path_would_add_a_point() {
    let (mut bench, mut tool, object) = line();
    let radius_of = |draw: &Draw| -> Vec<f32> {
        draw.overlay
            .shapes
            .iter()
            .filter_map(|s| if let Shape::Dot { radius, .. } = s { Some(*radius) } else { None })
            .collect()
    };
    let end = bench.px(point(&tool, object, 1));
    bench.hover(end);
    let draw = bench.frame(&mut tool);
    assert_eq!(radius_of(&draw), [5.0, 6.5], "the end point is lit (it is picked as well)");

    let on_path = bench.px(crate::path::evaluate(&tool.state(object).points, false, 0, 0.4));
    bench.hover(on_path);
    let draw = bench.frame(&mut tool);
    assert_eq!(radius_of(&draw), [5.0, 6.5, 4.0], "and a small dot where a click would add a point");

    bench.hover(Vec2::new(20.0, 20.0));
    assert_eq!(radius_of(&bench.frame(&mut tool)), [5.0, 6.5]);
    bench.env.hovering = false;
    bench.hover(on_path);
    assert_eq!(radius_of(&bench.frame(&mut tool)), [5.0, 6.5], "not over the viewport: no hover");
}

#[test]
fn the_clean_view_draws_no_handles_but_the_copies_stay() {
    let (mut bench, mut tool, object) = circle();
    bench.env.clean_view = true;
    let draw = bench.frame(&mut tool);
    assert!(draw.overlay.shapes.is_empty());
    assert_eq!(copies(&draw, object).len(), 7);
}

#[test]
fn escape_during_a_move_of_the_object_leaves_it_where_it_was() {
    let (mut bench, mut tool, object) = circle();
    bench.press(&mut tool, bench.px(Vec3::ZERO), Button::Left);
    bench.drag_to(&mut tool, bench.px(Vec3::new(0.0, 1.0, 0.0)));
    assert!(bench.key(&mut tool, Key::Escape));
    bench.release(&mut tool);
    assert_eq!(bench.doc.undo_steps(), 0);
    assert_eq!(bench.doc.info(object).unwrap().placement.position, Vec3::ZERO);
    assert!(bench.frame(&mut tool).hidden().is_empty());
}

#[test]
fn the_wheel_and_other_keys_are_left_to_the_app() {
    let (mut bench, mut tool, _) = circle();
    assert!(!bench.send(&mut tool, Event::Wheel(1.0)), "the wheel zooms");
    assert!(!bench.key(&mut tool, Key::Y), "its own key is the app's to put the tool away with");
    assert!(!bench.key(&mut tool, Key::A));
}

#[test]
fn leaving_the_tool_and_coming_back_to_a_different_object_drops_the_picked_handle() {
    let (mut bench, mut tool, _) = circle();
    assert!(tool.picked().is_some());
    bench.add("Other", cube(0.2));
    bench.frame(&mut tool);
    assert!(tool.picked().is_none());
}

// ---- the tool as the app sees it ------------------------------------------------------------

#[test]
fn leaving_mid_gesture_puts_things_back_and_keeps_the_array() {
    let (mut bench, mut tool, object) = circle();
    let before = point(&tool, object, 1);
    bench.press(&mut tool, bench.px(before), Button::Left);
    bench.drag_to(&mut tool, bench.px(Vec3::new(-3.0, 1.0, 0.0)));
    bench.leave(&mut tool);
    assert_eq!(point(&tool, object, 1), before);
    assert!(!tool.is_dragging() && tool.has_array(object));

    bench.call(|cx| tool.set_armed(cx, true));
    bench.press(&mut tool, Vec2::new(100.0, 100.0), Button::Left);
    bench.leave(&mut tool);
    assert!(!tool.is_armed() && !tool.is_dragging());
}

#[test]
fn y_is_the_hotkey_and_the_info_is_what_the_app_lists() {
    let info = ArrayTool::default().info();
    assert_eq!((info.id, info.name, info.icon, info.hotkey), ("array", "Array", "tool_array", Some(Key::Y)));
    assert_eq!(crate::tool().info().id, "array");
    assert!(!setup().1.can_undo(false) && !setup().1.can_undo(true));
}

#[test]
fn the_panel_lays_out_for_every_kind_of_array_and_for_none() {
    let ctx = egui::Context::default();
    theme::install(&ctx);
    let (mut bench, mut tool, object) = setup();
    for step in 0..5 {
        match step {
            1 => bench.call(|cx| tool.make_line(cx)),
            2 => bench.call(|cx| tool.make_circle(cx)),
            3 => {
                bench.call(|cx| tool.set_armed(cx, true));
                bench.drag(&mut tool, bench.px(Vec3::new(-2.0, 0.0, 0.0)), bench.px(Vec3::new(2.0, 1.0, 0.0)));
            }
            4 => bench.call(|cx| tool.edit(cx, "Array Closed Loop", false, |a| a.settings.closed = true)),
            _ => {}
        }
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            bench.call(|cx| tool.panel(ui, cx));
        });
        assert!(!output.shapes.is_empty(), "step {step}: the panel drew something");
        output.textures_delta.clear();
    }
    assert_eq!(tool.state(object).settings.kind, ArrayKind::Curve);
}

#[test]
fn the_demos_make_what_they_say() {
    for (what, kind) in
        [("", ArrayKind::Radial), ("circle", ArrayKind::Radial), ("line", ArrayKind::Line), ("curve", ArrayKind::Curve)]
    {
        let (mut bench, mut tool, _) = setup();
        bench.call(|cx| tool.demo(cx, what));
        let object = bench.doc.active();
        assert_eq!(tool.state(object).settings.kind, kind, "{what:?}");
        assert!(!bench.finished);
        assert!(bench.framed.is_some(), "the camera is asked to take it all in");
        let draw = bench.frame(&mut tool);
        assert!(!copies(&draw, object).is_empty() && !draw.overlay.shapes.is_empty(), "{what:?} draws the array");
    }

    let (mut bench, mut tool, object) = setup();
    bench.call(|cx| tool.demo(cx, "away"));
    assert!(bench.finished && tool.has_array(object));

    let (mut bench, mut tool, object) = setup();
    bench.call(|cx| tool.demo(cx, "made"));
    assert_eq!(bench.doc.info(object).unwrap().vertices, 64);
    assert!(bench.finished);

    let (mut bench, mut tool, _) = setup();
    bench.call(|cx| tool.demo(cx, "separate"));
    assert_eq!(bench.doc.objects().len(), 8);

    // A demo curve is drawn through the pen's own path: more than the two points a line has.
    let (mut bench, mut tool, _) = setup();
    bench.call(|cx| tool.demo(cx, "curve"));
    let object = bench.doc.active();
    assert!(tool.state(object).points.len() >= 4);
}

// ---- the panel, clicked ---------------------------------------------------------------------

/// The panel on a screen as wide as the real one, driven with pointer events.
struct Panel {
    ctx: egui::Context,
    last: Option<egui::FullOutput>,
}

const SCREEN: egui::Vec2 = egui::vec2(300.0, 1400.0);

impl Panel {
    fn new() -> Panel {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        Panel { ctx, last: None }
    }

    fn run(&mut self, bench: &mut Bench, tool: &mut ArrayTool, events: Vec<egui::Event>) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SCREEN)),
            events,
            ..Default::default()
        };
        let mut output = self.ctx.run_ui(input, |ui| {
            bench.call(|cx| tool.panel(ui, cx));
        });
        output.textures_delta.clear();
        self.last = Some(output);
    }

    /// Where the panel painted this text.
    fn text(&self, wanted: &str) -> egui::Pos2 {
        let output = self.last.as_ref().expect("the panel has been run");
        output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(t) if t.galley.text() == wanted => Some(t.pos),
                _ => None,
            })
            .unwrap_or_else(|| panic!("the panel shows no {wanted:?}"))
    }

    fn press(pos: egui::Pos2) -> Vec<egui::Event> {
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        vec![egui::Event::PointerMoved(pos), button(true)]
    }

    fn release(pos: egui::Pos2) -> Vec<egui::Event> {
        vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        }]
    }

    /// A click on the button or label whose text is `wanted`.
    fn click(&mut self, bench: &mut Bench, tool: &mut ArrayTool, wanted: &str) {
        self.run(bench, tool, vec![]);
        let at = self.text(wanted) + egui::vec2(4.0, 4.0);
        self.run(bench, tool, Panel::press(at));
        self.run(bench, tool, Panel::release(at));
        self.run(bench, tool, vec![]);
    }

    /// Drags the slider captioned `caption` to a fraction of its length and lets go.
    fn slide(&mut self, bench: &mut Bench, tool: &mut ArrayTool, caption: &str, fraction: f32) {
        self.run(bench, tool, vec![]);
        let label = self.text(caption);
        // The slider's box starts at the caption and ends as far from the right edge as it is from the left.
        let (left, right) = (label.x + 7.0, SCREEN.x - label.x - 7.0);
        let at = egui::pos2(left + (right - left) * fraction, label.y + 27.0);
        self.run(bench, tool, Panel::press(at));
        self.run(bench, tool, Panel::release(at));
        self.run(bench, tool, vec![]);
    }
}

#[test]
fn the_panels_make_buttons_make_arrays_and_mark_the_one_there_is() {
    let (mut bench, mut tool, object) = setup();
    let mut panel = Panel::new();
    panel.click(&mut bench, &mut tool, "Circle");
    assert_eq!(tool.state(object).settings.kind, ArrayKind::Radial);
    panel.click(&mut bench, &mut tool, "Line");
    assert_eq!(tool.state(object).settings.kind, ArrayKind::Line);
    panel.click(&mut bench, &mut tool, "Draw Curve");
    assert!(tool.is_armed());
    panel.click(&mut bench, &mut tool, "Draw Curve");
    assert!(!tool.is_armed(), "the button toggles");
    // Each of those is a step the tool's own Z takes back, newest first.
    assert!(bench.undo(&mut tool));
    assert_eq!(tool.state(object).settings.kind, ArrayKind::Radial, "the circle came back");
}

#[test]
fn the_panels_sliders_and_boxes_edit_the_array_and_a_drag_is_one_step() {
    let (mut bench, mut tool, object) = circle();
    let mut panel = Panel::new();

    panel.slide(&mut bench, &mut tool, "Copies", 0.5);
    let count = tool.state(object).settings.count;
    assert!((49..=52).contains(&count), "half way along 2 to 100: {count}");
    panel.slide(&mut bench, &mut tool, "End Size", 0.0);
    assert!((tool.state(object).settings.end_scale - 0.1).abs() < 0.02, "{}", tool.state(object).settings.end_scale);
    panel.slide(&mut bench, &mut tool, "Sweep", 0.0);
    assert!((tool.state(object).settings.sweep - 10.0).abs() < 1.5, "{}", tool.state(object).settings.sweep);

    panel.click(&mut bench, &mut tool, "Copies Turn to Follow");
    assert!(!tool.state(object).settings.follow);
    panel.slide(&mut bench, &mut tool, "End Size", 0.5);
    panel.click(&mut bench, &mut tool, "All the Same Size");
    assert_eq!(tool.state(object).settings.end_scale, 1.0);

    // The three slider drags, the tick box and the button: five steps, and the circle.
    settle(&mut bench, &mut tool);
    let mut steps = 0;
    while bench.undo(&mut tool) {
        steps += 1;
    }
    assert_eq!(steps, 7, "circle, count, end size, sweep, follow, end size, sizes");
    assert!(tool.state(object).is_none());
}

#[test]
fn the_panels_axis_buttons_make_real_buttons_and_remove_do_what_they_say() {
    let (mut bench, mut tool, object) = circle();
    let mut panel = Panel::new();
    panel.click(&mut bench, &mut tool, "Y");
    assert_eq!(tool.state(object).settings.axis, 1);
    panel.click(&mut bench, &mut tool, "X");
    assert_eq!(tool.state(object).settings.axis, 0);

    panel.click(&mut bench, &mut tool, "Remove Array");
    assert!(!tool.has_array(object));
    assert!(bench.undo(&mut tool) && tool.has_array(object));

    panel.click(&mut bench, &mut tool, "Separate Objects");
    assert_eq!(bench.doc.objects().len(), 8);
    assert!(bench.finished);
}

#[test]
fn the_done_button_puts_the_tool_away() {
    let (mut bench, mut tool, _) = circle();
    let mut panel = Panel::new();
    panel.click(&mut bench, &mut tool, "Done");
    assert!(bench.finished);
}
