//! The tool driven the way the app drives it, against the toolkit's bench.
//!
//! The bench's scene is a cube of half-width 0.5, so the scene's reference height is 1 and the starting
//! vase stands 1 unit tall beside it. Positions are worked out from the profile with the tool's own
//! plane, so the tests press exactly where a handle is drawn.

use std::time::{Duration, Instant};

use glam::{Vec2, Vec3};
use sculpt_geom::TriMesh;
use sculpt_toolkit::testing::{Bench, cube};
use sculpt_toolkit::{Button, Draw, Key, ObjectId, Scene, Shape, Tool, theme};

use super::{Drag, Lathe, wants_draft};
use crate::{LathePreset, curve};

fn bench() -> (Bench, Lathe) {
    let mut bench = Bench::new();
    bench.add("Cube", cube(0.5));
    let mut tool = Lathe::new();
    bench.enter(&mut tool);
    (bench, tool)
}

/// Where a profile point is drawn on the canonical side.
fn px(bench: &Bench, tool: &Lathe, p: Vec2) -> Vec2 {
    tool.plane.screen(&bench.env.view, p, 1.0).expect("in front of the camera")
}

/// Where control point `i` is drawn.
fn handle(bench: &Bench, tool: &Lathe, i: usize) -> Vec2 {
    px(bench, tool, tool.profile[i].position)
}

fn position(tool: &Lathe, i: usize) -> Vec2 {
    tool.profile[i].position
}

fn press_handle(bench: &mut Bench, tool: &mut Lathe, i: usize, button: Button) -> bool {
    let at = handle(bench, tool, i);
    bench.press(tool, at, button)
}

fn click_handle(bench: &mut Bench, tool: &mut Lathe, i: usize) {
    let at = handle(bench, tool, i);
    bench.click(tool, at, Button::Left);
}

/// Moves the pointer to where handle `i` is drawn now, plus `by`, with the button down.
fn drag_handle_to(bench: &mut Bench, tool: &mut Lathe, i: usize, by: Vec2) {
    let at = handle(bench, tool, i) + by;
    bench.drag_to(tool, at);
}

/// Presses on handle `i`, drags it `by` pixels and lets go.
fn drag_handle(bench: &mut Bench, tool: &mut Lathe, i: usize, by: Vec2) {
    let from = handle(bench, tool, i);
    bench.drag(tool, from, from + by);
}

fn hover_handle(bench: &mut Bench, tool: &Lathe, i: usize) {
    let at = handle(bench, tool, i);
    bench.hover(at);
}

/// Frames until the scene holds `objects` objects.
fn wait_for_objects(bench: &mut Bench, tool: &mut Lathe, objects: usize) {
    let start = Instant::now();
    while bench.doc.objects().len() < objects {
        assert!(start.elapsed() < Duration::from_secs(20), "the create never landed");
        bench.frame(tool);
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn clear(bench: &mut Bench, tool: &mut Lathe) {
    bench.call(|cx| tool.clear_profile(cx));
}

fn dots(draw: &Draw) -> Vec<(Vec2, f32, bool)> {
    draw.overlay
        .shapes
        .iter()
        .filter_map(|shape| match shape {
            Shape::Dot { center, radius, square, .. } => Some((*center, *radius, *square)),
            _ => None,
        })
        .collect()
}

// ------------------------------------------------------------------------------ the start

#[test]
fn the_tool_is_described_for_its_button() {
    let info = Lathe::new().info();
    assert_eq!((info.id, info.name, info.icon), ("lathe", "Lathe", "tool_lathe"));
    assert!(info.hotkey.is_none(), "the Unity app gives the lathe no key");
    assert!(info.tip.contains("turned solid"));
}

#[test]
fn entering_seats_the_starting_vase_beside_the_scene_and_frames_both() {
    let (bench, tool) = bench();
    assert_eq!(tool.profile.len(), 7);
    assert!(!tool.profile.closed_loop());
    // The cube's half-width, then three quarters of the reference height, then the vase's own middle.
    assert!((tool.plane.root.x - 1.25).abs() < 1e-4, "{:?}", tool.plane.root);
    assert!((tool.plane.root.y + 0.5).abs() < 1e-4, "the vase's middle is level with the cube's");
    let framed = bench.framed.expect("the camera was asked to bring both into view");
    assert!(framed.contains(Vec3::new(-0.5, 0.0, 0.0)) && framed.contains(Vec3::new(1.5, 0.0, 0.0)), "{framed:?}");
    assert!(!tool.can_undo(false) && !tool.can_undo(true), "the starting shape is not an undo step");
    assert!(bench.status.as_ref().is_some_and(|s| s.1));
}

#[test]
fn the_preview_is_a_lit_mesh_standing_where_the_handles_are() {
    let (mut bench, mut tool) = bench();
    let draw = bench.frame(&mut tool);
    let previews = draw.previews();
    assert_eq!(previews.len(), 1);
    assert_eq!(previews[0].transforms, [tool.plane.matrix()]);
    assert_eq!(previews[0].style.look, sculpt_toolkit::Look::Solid);
    let data = previews[0].mesh.data();
    assert!(data.positions.len() > 1000 && data.normals.len() == data.positions.len());
    // The handle of the widest point is on the rim of the drawn solid: the mesh reaches that far.
    let reach = data.positions.iter().map(|p| p.x.abs().max(p.z.abs())).fold(0.0, f32::max);
    assert!((reach - 0.32).abs() < 0.02, "{reach}");
    assert!(tool.create_triangles() > 1000);
}

#[test]
fn the_preview_is_rebuilt_only_when_the_profile_or_the_detail_changes() {
    let (mut bench, mut tool) = bench();
    bench.frame(&mut tool);
    let version = tool.preview().version();
    for _ in 0..4 {
        bench.frame(&mut tool);
    }
    assert_eq!(tool.preview().version(), version, "nothing changed");

    drag_handle(&mut bench, &mut tool, 3, Vec2::new(20.0, 0.0));
    bench.frame(&mut tool);
    let after_drag = tool.preview().version();
    assert!(after_drag > version);
    bench.frame(&mut tool);
    assert_eq!(tool.preview().version(), after_drag);

    tool.set_segments(64);
    bench.frame(&mut tool);
    assert!(tool.preview().version() > after_drag, "a detail change rebuilds");
}

#[test]
fn the_preview_is_lighter_than_create_and_the_readout_is_for_create() {
    let (mut bench, mut tool) = bench();
    tool.set_segments(1024);
    bench.frame(&mut tool);
    let preview_triangles = tool.preview().data().indices.len() / 3;
    assert!(preview_triangles < tool.create_triangles() / 4, "{preview_triangles} against {}", tool.create_triangles());
    let counted = tool.builder.count(&tool.profile, &tool.settings).unwrap().triangles;
    assert_eq!(tool.create_triangles(), counted);
}

#[test]
fn a_slow_build_during_a_drag_asks_for_a_lighter_one() {
    assert!(wants_draft(true, 20.0));
    assert!(!wants_draft(true, 5.0));
    assert!(!wants_draft(false, 20.0), "never when not dragging: the full preview comes back on release");
}

// -------------------------------------------------------------------------------- drawing

#[test]
fn handles_are_drawn_on_both_sides_and_a_corner_is_a_square() {
    let (mut bench, mut tool) = bench();
    let draw = bench.frame(&mut tool);
    let dots = dots(&draw);
    // Seven points; six are off the axis and so have a mirror image.
    assert_eq!(dots.len(), 13);
    // The vase has two corners; the one on the axis has no mirror.
    assert_eq!(dots.iter().filter(|d| d.2).count(), 3);
    let first = handle(&bench, &tool, 3);
    assert!(dots.iter().any(|d| d.0.distance(first) < 0.01));
    let mirrored = tool.plane.screen(&bench.env.view, position(&tool, 3), -1.0).unwrap();
    assert!(dots.iter().any(|d| d.0.distance(mirrored) < 0.01 && d.1 < 5.0), "the far one is smaller");
}

#[test]
fn nothing_is_drawn_while_the_tool_is_away_or_the_view_is_clean() {
    let (mut bench, mut tool) = bench();
    assert!(!bench.frame(&mut tool).overlay.shapes.is_empty());
    bench.env.clean_view = true;
    let clean = bench.frame(&mut tool);
    assert!(clean.overlay.shapes.is_empty() && clean.previews().len() == 1, "the solid stays, the handles go");
    assert!(!press_handle(&mut bench, &mut tool, 3, Button::Left), "and the view is look-only");
    bench.release(&mut tool);
    bench.env.clean_view = false;
    bench.env.active = false;
    assert!(bench.frame(&mut tool).is_empty());
    bench.env.active = true;
    assert!(!bench.frame(&mut tool).is_empty(), "the shape is still there next time");
}

#[test]
fn the_hovered_handle_grows() {
    let (mut bench, mut tool) = bench();
    hover_handle(&mut bench, &tool, 3);
    let draw = bench.frame(&mut tool);
    let at = handle(&bench, &tool, 3);
    assert!(dots(&draw).iter().any(|d| d.0.distance(at) < 0.01 && d.1 > 6.0));
    bench.env.hovering = false;
    let draw = bench.frame(&mut tool);
    assert!(dots(&draw).iter().all(|d| d.1 < 6.0));
}

// ------------------------------------------------------------------------------ gestures

#[test]
fn clicks_on_empty_space_draw_a_profile_point_by_point() {
    let (mut bench, mut tool) = bench();
    clear(&mut bench, &mut tool);
    let steps = tool.history.steps();
    let wanted = [Vec2::new(0.3, 0.1), Vec2::new(0.3, 0.9)];
    for p in wanted {
        let at = px(&bench, &tool, p);
        bench.click(&mut tool, at, Button::Left);
    }
    assert_eq!(tool.profile.len(), 2);
    for (i, p) in wanted.into_iter().enumerate() {
        assert!(position(&tool, i).distance(p) < 1e-3, "{:?}", position(&tool, i));
    }
    assert_eq!(tool.selected(), Some(1));
    assert_eq!(tool.history.steps(), steps + 2, "each click is a step of its own");

    // Near the axis the end lands on it, closing that end into a pole.
    let near_axis = px(&bench, &tool, Vec2::new(0.04, 1.1));
    bench.click(&mut tool, near_axis, Button::Left);
    assert_eq!(tool.profile.len(), 3);
    assert_eq!(position(&tool, 2), Vec2::new(0.0, 1.1));
    assert!(bench.frame(&mut tool).previews().len() == 1, "the solid appears once there are two points");
}

#[test]
fn the_first_point_says_what_comes_next() {
    let (mut bench, mut tool) = bench();
    clear(&mut bench, &mut tool);
    let at = px(&bench, &tool, Vec2::new(0.3, 0.1));
    bench.click(&mut tool, at, Button::Left);
    assert!(bench.status.as_ref().is_some_and(|s| s.0.contains("First point placed")));
    assert!(bench.frame(&mut tool).previews().is_empty(), "one point makes no solid");
    assert!(tool.built.error.is_some());
}

#[test]
fn a_press_on_nothing_is_passed_on_so_a_drag_orbits() {
    let (mut bench, mut tool) = bench();
    let version = tool.profile.version();
    assert!(!bench.press(&mut tool, Vec2::new(20.0, 20.0), Button::Left));
    bench.drag_to(&mut tool, Vec2::new(200.0, 80.0));
    bench.release(&mut tool);
    assert_eq!(tool.profile.version(), version);
    assert_eq!(tool.history.steps(), 0);
    // A press that never moves is a click, and a click on nothing extends the profile.
    let before = tool.profile.len();
    bench.click(&mut tool, Vec2::new(20.0, 20.0), Button::Left);
    assert_eq!(tool.profile.len(), before + 1);
}

#[test]
fn dragging_a_handle_moves_that_point_and_is_one_undo_step() {
    let (mut bench, mut tool) = bench();
    let original = position(&tool, 3);
    let (from, to) = (handle(&bench, &tool, 3), handle(&bench, &tool, 3) + Vec2::new(30.0, -20.0));
    bench.drag(&mut tool, from, to);
    let moved = position(&tool, 3);
    assert!(moved.x > original.x && moved.y > original.y, "{moved}");
    assert!(handle(&bench, &tool, 3).distance(to) < 0.5, "the handle ended under the pointer");
    assert_eq!(tool.selected(), Some(3));
    assert_eq!(tool.history.steps(), 1);

    assert!(tool.can_undo(false) && !tool.can_undo(true));
    assert!(bench.undo(&mut tool));
    assert_eq!(position(&tool, 3), original);
    assert!(tool.can_undo(true));
    assert!(bench.redo(&mut tool));
    assert_eq!(position(&tool, 3), moved);
}

#[test]
fn the_handle_stays_where_it_was_grabbed() {
    let (mut bench, mut tool) = bench();
    let original = position(&tool, 3);
    let off_centre = handle(&bench, &tool, 3) + Vec2::new(6.0, 5.0);
    bench.press(&mut tool, off_centre, Button::Left);
    bench.drag_to(&mut tool, off_centre + Vec2::new(0.0, 40.0));
    bench.release(&mut tool);
    let moved = handle(&bench, &tool, 3);
    assert!(moved.distance(off_centre + Vec2::new(0.0, 40.0) - Vec2::new(6.0, 5.0)) < 0.5, "{moved}");
    assert!(position(&tool, 3).y < original.y);
}

#[test]
fn a_press_that_never_moves_selects_without_changing_anything() {
    let (mut bench, mut tool) = bench();
    let version = tool.profile.version();
    click_handle(&mut bench, &mut tool, 2);
    assert_eq!(tool.selected(), Some(2));
    assert_eq!(tool.profile.version(), version);
    assert_eq!(tool.history.steps(), 0);
}

#[test]
fn grabbing_the_far_silhouette_edits_the_same_point() {
    let (mut bench, mut tool) = bench();
    let original = position(&tool, 3);
    let far = tool.plane.screen(&bench.env.view, original, -1.0).unwrap();
    // Dragging the left-hand handle further left widens the shape, as it looks.
    bench.drag(&mut tool, far, far + Vec2::new(-25.0, 0.0));
    assert!(position(&tool, 3).x > original.x + 0.05, "{}", position(&tool, 3));
}

#[test]
fn shift_keeps_a_drag_to_the_direction_it_mostly_goes() {
    let (mut bench, mut tool) = bench();
    let original = position(&tool, 3);
    bench.env.mods.shift = true;
    drag_handle(&mut bench, &mut tool, 3, Vec2::new(40.0, 10.0));
    assert!(position(&tool, 3).x > original.x);
    assert_eq!(position(&tool, 3).y, original.y);
}

#[test]
fn a_point_stops_at_the_axis_and_an_end_dropped_near_it_lands_on_it() {
    let (mut bench, mut tool) = bench();
    let left_of_axis = px(&bench, &tool, Vec2::new(-0.5, position(&tool, 3).y));
    let grabbed = handle(&bench, &tool, 3);
    bench.drag(&mut tool, grabbed, left_of_axis);
    let held = position(&tool, 3).x;
    assert!(held > 0.0 && held == tool.profile.min_interior_radius(), "held a hair off the axis: {held}");

    // The top end, dropped within reach of the axis, closes onto it.
    let top = tool.profile.len() - 1;
    let near = px(&bench, &tool, Vec2::new(0.05, position(&tool, top).y));
    let grabbed = handle(&bench, &tool, top);
    bench.drag(&mut tool, grabbed, near);
    assert_eq!(position(&tool, top).x, 0.0);
}

#[test]
fn pressing_on_the_curve_inserts_a_point_and_keeps_dragging_it() {
    let (mut bench, mut tool) = bench();
    let on_curve = px(&bench, &tool, curve::evaluate(tool.profile.points(), false, 3, 0.5));
    assert!(bench.press(&mut tool, on_curve, Button::Left), "taken: the point goes on being dragged");
    assert_eq!(tool.profile.len(), 8);
    assert_eq!(tool.selected(), Some(4));
    bench.drag_to(&mut tool, on_curve + Vec2::new(25.0, 0.0));
    bench.release(&mut tool);
    assert!(position(&tool, 4).x > 0.3, "{}", position(&tool, 4));
    assert_eq!(tool.history.steps(), 1, "the insert and the drag are one step");
    assert!(bench.undo(&mut tool));
    assert_eq!(tool.profile.len(), 7);
}

#[test]
fn escape_on_an_insert_takes_the_new_point_away_too() {
    let (mut bench, mut tool) = bench();
    let on_curve = px(&bench, &tool, curve::evaluate(tool.profile.points(), false, 3, 0.5));
    assert!(bench.press(&mut tool, on_curve, Button::Left));
    bench.drag_to(&mut tool, on_curve + Vec2::new(25.0, 0.0));
    assert!(bench.key(&mut tool, Key::Escape));
    bench.release(&mut tool);
    assert_eq!(tool.profile.len(), 7);
    assert_eq!(tool.history.steps(), 0);
}

#[test]
fn a_press_on_a_cap_inserts_a_point_and_keeps_the_old_end_a_corner() {
    let (mut bench, mut tool) = bench();
    // The vase's top is open and capped by a flat disc from its rim to the axis.
    let top = tool.profile.len() - 1;
    let rim = position(&tool, top);
    assert!(!tool.profile[top].sharp);
    let on_cap = px(&bench, &tool, Vec2::new(rim.x * 0.3, rim.y));
    assert!(bench.press(&mut tool, on_cap, Button::Left));
    bench.release(&mut tool);
    assert_eq!(tool.profile.len(), 8);
    assert!(tool.profile[top].sharp, "where the cap met the wall stays a corner");
    let new = position(&tool, 7);
    assert!(new.x > 0.0 && new.x < rim.x && (new.y - rim.y).abs() < 1e-4, "on the disc: {new}");
}

#[test]
fn right_click_on_a_handle_or_delete_removes_it() {
    let (mut bench, mut tool) = bench();
    assert!(press_handle(&mut bench, &mut tool, 2, Button::Right));
    bench.release(&mut tool);
    assert_eq!(tool.profile.len(), 6);
    assert!(!bench.press(&mut tool, Vec2::new(30.0, 30.0), Button::Right), "a right press on nothing is not the tool's");
    bench.release(&mut tool);

    click_handle(&mut bench, &mut tool, 1);
    assert!(bench.key(&mut tool, Key::Delete));
    assert_eq!(tool.profile.len(), 5);
    assert!(!bench.key(&mut tool, Key::Delete), "nothing selected: not a key for the tool");
    assert!(bench.undo(&mut tool) && tool.profile.len() == 6);
}

#[test]
fn deleting_a_point_of_a_three_point_loop_opens_it() {
    let (mut bench, mut tool) = bench();
    bench.call(|cx| tool.apply_preset(cx, LathePreset::Ring));
    assert!(tool.profile.closed_loop());
    while tool.profile.len() > 3 {
        let n = tool.profile.len();
        bench.call(|cx| tool.delete_point(cx, n - 1));
    }
    assert!(tool.profile.closed_loop());
    bench.call(|cx| tool.delete_point(cx, 0));
    assert_eq!(tool.profile.len(), 2);
    assert!(!tool.profile.closed_loop(), "two points cannot make a loop");
}

#[test]
fn a_double_click_on_a_handle_toggles_a_corner() {
    let (mut bench, mut tool) = bench();
    assert!(!tool.profile[3].sharp);
    click_handle(&mut bench, &mut tool, 3);
    click_handle(&mut bench, &mut tool, 3);
    assert!(tool.profile[3].sharp);
    assert_eq!(tool.history.steps(), 1);
    assert!(bench.status.as_ref().is_some_and(|s| s.0.contains("Corner")));

    // Two slow clicks are two selections, not a double click.
    bench.env.time += 1.0;
    click_handle(&mut bench, &mut tool, 3);
    bench.env.time += 1.0;
    click_handle(&mut bench, &mut tool, 3);
    assert!(tool.profile[3].sharp);
}

#[test]
fn escape_cancels_a_drag_then_deselects_then_is_left_to_the_app() {
    let (mut bench, mut tool) = bench();
    let original = position(&tool, 3);
    press_handle(&mut bench, &mut tool, 3, Button::Left);
    drag_handle_to(&mut bench, &mut tool, 3, Vec2::new(50.0, 0.0));
    assert!(position(&tool, 3).x > original.x);
    assert!(bench.key(&mut tool, Key::Escape));
    bench.release(&mut tool);
    assert_eq!(position(&tool, 3), original, "put back exactly");
    assert_eq!(tool.history.steps(), 0);

    assert_eq!(tool.selected(), Some(3));
    assert!(bench.key(&mut tool, Key::Escape), "lets go of the selection");
    assert_eq!(tool.selected(), None);
    assert!(!bench.key(&mut tool, Key::Escape), "nothing left: the app puts the tool away");
}

#[test]
fn leaving_mid_drag_keeps_what_the_drag_did() {
    let (mut bench, mut tool) = bench();
    let original = position(&tool, 3);
    press_handle(&mut bench, &mut tool, 3, Button::Left);
    drag_handle_to(&mut bench, &mut tool, 3, Vec2::new(30.0, 0.0));
    bench.leave(&mut tool);
    assert!(!tool.dragging());
    assert!(position(&tool, 3).x > original.x);
    assert_eq!(tool.history.steps(), 1);
}

#[test]
fn a_ctrl_stroke_sketches_a_new_profile_with_its_ends_on_the_axis() {
    let (mut bench, mut tool) = bench();
    let outline = [(0.0, 0.0), (0.26, 0.0), (0.26, 0.04), (0.06, 0.1), (0.05, 0.38), (0.3, 0.56), (0.36, 0.94), (0.3, 1.0), (0.0, 1.0)];
    let mut path = Vec::new();
    for pair in outline.windows(2) {
        for step in 0..20 {
            path.push(Vec2::from(pair[0]).lerp(Vec2::from(pair[1]), step as f32 / 20.0));
        }
    }
    path.push(Vec2::new(0.0, 1.0));
    let on_screen: Vec<Vec2> = path.iter().map(|&p| px(&bench, &tool, p)).collect();

    bench.env.mods.ctrl = true;
    assert!(bench.press(&mut tool, on_screen[0], Button::Left), "a Ctrl press is a sketch, wherever it lands");
    for &at in &on_screen {
        bench.drag_to(&mut tool, at);
    }
    let draw = bench.frame(&mut tool);
    assert!(draw.overlay.shapes.iter().any(|s| matches!(s, Shape::Line { color, .. } if *color == super::overlay::SKETCH)));
    bench.release(&mut tool);

    assert!((4..=18).contains(&tool.profile.len()), "{} points", tool.profile.len());
    assert!(!tool.profile.closed_loop());
    let last = tool.profile.len() - 1;
    assert_eq!((position(&tool, 0).x, position(&tool, last).x), (0.0, 0.0), "drawn to the axis, so closed on it");
    assert!(tool.profile.max_radius() > 0.3);
    assert_eq!(tool.history.steps(), 1);
    assert!(bench.undo(&mut tool));
    assert_eq!(tool.profile.len(), 7, "the vase is back");
}

#[test]
fn a_stroke_that_comes_back_to_where_it_began_makes_a_ring() {
    let (mut bench, mut tool) = bench();
    let circle: Vec<Vec2> = (0..=48)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / 48.0;
            px(&bench, &tool, Vec2::new(0.5 + 0.12 * a.cos(), 0.5 + 0.12 * a.sin()))
        })
        .collect();
    bench.env.mods.ctrl = true;
    bench.press(&mut tool, circle[0], Button::Left);
    for &at in &circle {
        bench.drag_to(&mut tool, at);
    }
    bench.release(&mut tool);
    assert!(tool.profile.closed_loop() && tool.profile.len() >= 3);
    assert!(tool.profile.points().iter().all(|p| p.position.x > 0.3), "clear of the axis");
}

#[test]
fn a_stroke_too_short_to_use_leaves_the_profile_alone() {
    let (mut bench, mut tool) = bench();
    bench.env.mods.ctrl = true;
    let at = px(&bench, &tool, Vec2::new(0.5, 0.5));
    bench.click(&mut tool, at, Button::Left);
    assert_eq!(tool.profile.len(), 7);
    assert!(bench.status.as_ref().is_some_and(|s| !s.1));
}

#[test]
fn extending_past_an_end_closed_on_the_axis_keeps_it_closed() {
    let (mut bench, mut tool) = bench();
    bench.call(|cx| tool.apply_preset(cx, LathePreset::Egg));
    let before = tool.profile.len();
    // Above the egg's top pole, off to one side.
    let at = px(&bench, &tool, Vec2::new(0.2, 1.3));
    bench.click(&mut tool, at, Button::Left);
    assert_eq!(tool.profile.len(), before + 1);
    let last = tool.profile.len() - 1;
    assert!(position(&tool, last).distance(Vec2::new(0.0, 1.3)) < 1e-3 && position(&tool, last).x == 0.0, "a new pole level with it");
    assert!(position(&tool, last - 1).distance(Vec2::new(0.2, 1.3)) < 1e-3, "the old pole became the point");
    assert!(position(&tool, last - 1).x > 0.1, "not left pinched against the axis");
    assert!(bench.undo(&mut tool) && tool.profile.len() == before);
}

#[test]
fn a_click_on_a_loop_goes_into_the_nearest_segment() {
    let (mut bench, mut tool) = bench();
    bench.call(|cx| tool.apply_preset(cx, LathePreset::Ring));
    let before = tool.profile.len();
    // Outside the ring's outer wall.
    let at = px(&bench, &tool, Vec2::new(0.72, 0.5));
    bench.click(&mut tool, at, Button::Left);
    assert_eq!(tool.profile.len(), before + 1);
    assert!(tool.profile.closed_loop());
    assert!(position(&tool, 4).x > 0.7 || position(&tool, 3).x > 0.7, "between the right-hand points");
}

// ------------------------------------------------------------------------------- the axis

#[test]
fn dragging_the_axis_moves_the_lathe_and_keeps_its_distance_from_the_camera() {
    let (mut bench, mut tool) = bench();
    let root = tool.plane.root;
    let depth = bench.env.view.depth_of(root);
    let on_axis = px(&bench, &tool, Vec2::new(0.0, 0.55));
    bench.hover(on_axis);
    bench.env.hovering = true;
    let lit = bench.frame(&mut tool);
    assert!(lit.overlay.shapes.iter().any(|s| matches!(s, Shape::Line { dash: Some(_), width, .. } if *width > 1.9)), "the axis lights up");

    assert!(bench.press(&mut tool, on_axis, Button::Left), "the axis is grabbed, not passed on to the camera");
    bench.drag_to(&mut tool, on_axis + Vec2::new(-60.0, -30.0));
    bench.release(&mut tool);
    let moved = tool.plane.root;
    assert!(moved.x < root.x && moved.y > root.y, "{moved}");
    assert!((bench.env.view.depth_of(moved) - depth).abs() < 1e-3);
    assert_eq!(tool.profile.len(), 7);
    assert_eq!(tool.history.steps(), 1);
    assert!(bench.undo(&mut tool));
    assert_eq!(tool.plane.root, root);
}

#[test]
fn move_axis_to_view_centre_puts_the_shape_where_the_camera_looks() {
    let (mut bench, mut tool) = bench();
    bench.call(|cx| tool.move_axis_to_view_centre(cx));
    let middle = tool.plane.root + Vec3::Y * tool.middle_height();
    let at = bench.env.view.project(middle).unwrap();
    assert!(at.distance(bench.env.view.size * 0.5) < 0.5, "{at}");
    assert!(bench.undo(&mut tool));
}

// ------------------------------------------------------------------- numbers and options

#[test]
fn the_sliders_stretch_the_shape_and_a_stream_of_moves_is_one_step() {
    let (mut bench, mut tool) = bench();
    let base = tool.profile.min_height();
    for target in [0.35, 0.4, 0.45, 0.5] {
        bench.call(|cx| tool.set_radius(cx, target));
        bench.env.time += 0.1;
        bench.frame(&mut tool);
    }
    assert!((tool.radius() - 0.5).abs() < 1e-4);
    assert_eq!(tool.history.steps(), 0, "still open: the slider may yet move");
    assert!(tool.can_undo(false), "but Z already has it");
    bench.env.time += 1.0;
    bench.frame(&mut tool);
    assert_eq!(tool.history.steps(), 1);

    bench.call(|cx| tool.set_height(cx, 2.0));
    assert!((tool.height() - 2.0).abs() < 1e-4 && tool.profile.min_height() == base, "stretched up from its base");
    bench.call(|cx| tool.set_scale(cx, 2.0));
    assert!((tool.height() - 4.0).abs() < 1e-4 && (tool.radius() - 1.0).abs() < 1e-4);
    bench.call(|cx| tool.set_scale(cx, 99.0));
    assert_eq!(tool.plane.scale, 5.0, "the Scale slider stops at 5");

    assert!(bench.undo(&mut tool), "the scale stream");
    assert!(bench.undo(&mut tool), "the height stream");
    assert!((tool.radius() - 0.5).abs() < 1e-4);
    assert!(bench.undo(&mut tool), "the radius stream");
    assert!((tool.radius() - 0.32).abs() < 1e-4);
}

#[test]
fn presets_replace_the_profile_as_one_undo_step() {
    let (mut bench, mut tool) = bench();
    bench.call(|cx| tool.apply_preset(cx, LathePreset::Ring));
    assert!(tool.profile.closed_loop() && tool.profile.len() == 8);
    assert!(bench.status.as_ref().is_some_and(|s| s.0.contains("ring")));
    bench.call(|cx| tool.apply_preset(cx, LathePreset::Bowl));
    assert!(!tool.profile.closed_loop() && tool.profile.len() == 9);
    assert_eq!(tool.history.steps(), 2);
    assert!(tool.history.next_undo(&tool.profile, &tool.plane) == Some("Preset: Bowl"));
    assert!(bench.undo(&mut tool) && bench.undo(&mut tool));
    assert!(!tool.profile.closed_loop() && tool.profile.len() == 7, "the vase again");
    assert!(!bench.undo(&mut tool) || !tool.can_undo(false), "past the start Z goes to the scene");
}

#[test]
fn a_preset_that_would_stand_out_of_sight_brings_the_view_round_it_and_one_in_sight_leaves_it() {
    let (mut bench, mut tool) = bench();
    bench.framed = None;
    bench.call(|cx| tool.apply_preset(cx, LathePreset::Egg));
    assert!(bench.framed.is_none(), "the vase's place is in view already: the camera stays");
    tool.plane.root.x = 4.0;
    bench.call(|cx| tool.apply_preset(cx, LathePreset::Bowl));
    let framed = bench.framed.expect("off to the side, under the panel: the camera glides to it");
    assert!(framed.contains(Vec3::new(4.0, 0.0, 0.0)) && framed.contains(Vec3::ZERO), "{framed:?}");
}

#[test]
fn closing_a_loop_needs_three_points_and_the_toggle_is_an_undo_step() {
    let (mut bench, mut tool) = bench();
    clear(&mut bench, &mut tool);
    for p in [Vec2::new(0.4, 0.2), Vec2::new(0.4, 0.8)] {
        let at = px(&bench, &tool, p);
        bench.click(&mut tool, at, Button::Left);
    }
    bench.call(|cx| tool.set_closed_loop(cx, true));
    assert!(!tool.profile.closed_loop());
    assert!(bench.status.as_ref().is_some_and(|s| !s.1 && s.0.contains("3 points")));
    let at = px(&bench, &tool, Vec2::new(0.7, 0.5));
    bench.click(&mut tool, at, Button::Left);
    let steps = tool.history.steps();
    bench.call(|cx| tool.set_closed_loop(cx, true));
    assert!(tool.profile.closed_loop());
    assert_eq!(tool.history.steps(), steps + 1);
    assert!(bench.undo(&mut tool) && !tool.profile.closed_loop());
}

#[test]
fn detail_settings_round_and_are_not_undo_steps() {
    let (_, mut tool) = bench();
    tool.set_segments(250);
    let rounded = tool.settings().radial_segments;
    assert!(rounded % 4 == 0 && (248..=252).contains(&rounded), "{rounded}");
    tool.set_segments(1);
    assert_eq!(tool.settings().radial_segments, 8, "at the lowest input");
    tool.set_segments(100_000);
    assert_eq!(tool.settings().radial_segments, 1024);
    tool.set_cap_ends(false);
    tool.set_even_triangles(false);
    assert!(!tool.settings().cap_ends && !tool.settings().even_triangles);
    assert_eq!(tool.history.steps(), 0);
}

#[test]
fn an_open_end_is_flagged_in_the_readout_when_caps_are_off() {
    let (mut bench, mut tool) = bench();
    tool.set_cap_ends(false);
    bench.frame(&mut tool);
    assert!(!tool.build.watertight, "the vase's top is left open");
    tool.set_cap_ends(true);
    bench.frame(&mut tool);
    assert!(tool.build.watertight);
}

// -------------------------------------------------------------------------------- create

fn created_object(bench: &Bench) -> ObjectId {
    *bench.doc.objects().last().unwrap()
}

fn mesh_of(bench: &Bench, id: ObjectId) -> TriMesh {
    let view = bench.doc.mesh(id).unwrap();
    TriMesh::new(view.positions.to_vec(), view.indices.to_vec())
}

#[test]
fn create_adds_one_object_in_one_undo_step_and_puts_the_tool_away() {
    let (mut bench, mut tool) = bench();
    let root = tool.plane.root;
    assert!(bench.key(&mut tool, Key::Enter));
    wait_for_objects(&mut bench, &mut tool, 2);
    assert!(bench.finished, "straight on to sculpting what was just made");

    let id = created_object(&bench);
    let info = bench.doc.info(id).unwrap();
    assert_eq!(info.name, "Lathe");
    assert!(info.active, "selected, ready to sculpt");
    assert_eq!(info.placement.scale, Vec3::ONE, "the scale is baked into the vertices");
    // Origin at the shape's vertical middle, on the axis.
    assert!(info.placement.position.distance(root + Vec3::Y * 0.5) < 1e-4, "{:?}", info.placement.position);
    let mesh = mesh_of(&bench, id);
    assert!(mesh.signed_volume() > 0.1, "a closed solid, outward");
    let size = bench.doc.bounds(id).size();
    assert!((size.y - 1.0).abs() < 1e-3 && (size.x - 0.64).abs() < 0.03, "{size}");
    assert_eq!(info.triangles, tool.create_triangles(), "what the readout promised");
    assert_eq!(bench.doc.undo_steps(), 1);
    assert!(bench.status.as_ref().is_some_and(|s| s.1 && s.0.starts_with("Created Lathe")));

    // The profile is kept for another, and the tool's own history is empty, so Z reaches the scene.
    assert_eq!(tool.profile.len(), 7);
    assert!(!tool.can_undo(false));
    assert!(bench.undo(&mut tool));
    assert_eq!(bench.doc.objects().len(), 1);
}

#[test]
fn create_builds_at_the_full_detail_the_preview_does_not_show() {
    let (mut bench, mut tool) = bench();
    tool.set_segments(512);
    bench.frame(&mut tool);
    let preview = tool.preview().data().indices.len() / 3;
    bench.key(&mut tool, Key::Enter);
    wait_for_objects(&mut bench, &mut tool, 2);
    let made = bench.doc.info(created_object(&bench)).unwrap().triangles;
    assert!(made > preview * 3, "{made} against the preview's {preview}");
}

#[test]
fn create_bakes_the_scale_into_the_object() {
    let (mut bench, mut tool) = bench();
    bench.call(|cx| tool.set_scale(cx, 2.0));
    let root = tool.plane.root;
    bench.key(&mut tool, Key::Enter);
    wait_for_objects(&mut bench, &mut tool, 2);
    let id = created_object(&bench);
    assert!((bench.doc.bounds(id).size().y - 2.0).abs() < 1e-3);
    // The shape's middle is half its profile height up, times the scale.
    let at = bench.doc.info(id).unwrap().placement.position;
    assert!(at.distance(root + Vec3::Y * 1.0) < 1e-4, "{at}");
}

#[test]
fn a_profile_that_cannot_be_built_says_why_and_makes_nothing() {
    let (mut bench, mut tool) = bench();
    clear(&mut bench, &mut tool);
    assert!(bench.key(&mut tool, Key::Enter));
    assert!(tool.job.is_none());
    assert_eq!(bench.doc.objects().len(), 1);
    assert!(bench.status.as_ref().is_some_and(|s| !s.1 && s.0.contains("profile points")));
}

#[test]
fn a_create_still_running_when_the_tool_is_put_away_lands_all_the_same() {
    let (mut bench, mut tool) = bench();
    bench.key(&mut tool, Key::Enter);
    bench.leave(&mut tool);
    bench.env.active = false;
    wait_for_objects(&mut bench, &mut tool, 2);
    assert_eq!(bench.doc.objects().len(), 2);
}

#[test]
fn undoing_the_create_hands_the_profile_back_and_redo_puts_the_tool_away_again() {
    let (mut bench, mut tool) = bench();
    bench.key(&mut tool, Key::Enter);
    wait_for_objects(&mut bench, &mut tool, 2);
    let made = created_object(&bench);
    assert_eq!(tool.created(), Some(made));
    // The app has put the tool away.
    bench.env.active = false;
    bench.started = false;
    bench.frame(&mut tool);
    assert!(!bench.started, "nothing to hand back while the object stands");

    assert!(bench.undo_scene(), "Z, with the tool away");
    bench.frame(&mut tool);
    assert!(bench.started, "the tool asks to come back up");
    assert_eq!(tool.created(), None);
    assert_eq!(tool.profile.len(), 7, "with the profile as it was");
    assert!(bench.status.as_ref().is_some_and(|s| s.0.contains("Create undone")));

    // Back up, the user redoes: the object returns and the tool steps aside.
    bench.env.active = true;
    bench.finished = false;
    bench.frame(&mut tool);
    assert!(!bench.finished);
    assert!(bench.redo_scene());
    bench.frame(&mut tool);
    assert!(bench.finished);
    assert_eq!(tool.created(), Some(made), "the same object, under the same id");
}

#[test]
fn the_next_visit_after_a_create_stands_the_shape_beside_the_new_object() {
    let (mut bench, mut tool) = bench();
    let first = tool.plane.root;
    bench.key(&mut tool, Key::Enter);
    wait_for_objects(&mut bench, &mut tool, 2);
    bench.leave(&mut tool);
    bench.framed = None;
    bench.enter(&mut tool);
    assert!(tool.plane.root.x > first.x + 0.9, "{:?}: the vase now stands where it was", tool.plane.root);
    assert!(bench.framed.is_some());
    // And only once: a later visit leaves it where the artist put it.
    let placed = tool.plane.root;
    bench.leave(&mut tool);
    bench.enter(&mut tool);
    assert_eq!(tool.plane.root, placed);
}

// ---------------------------------------------------------------------------------- panel

#[test]
fn the_panel_lays_out_in_every_state() {
    let mut bench = Bench::new();
    bench.add("Cube", cube(0.5));
    let mut tool = Lathe::new();
    bench.enter(&mut tool);
    let ctx = egui::Context::default();
    theme::install(&ctx);
    let draw = |bench: &mut Bench, tool: &mut Lathe| {
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            bench.call(|cx| tool.panel(ui, cx));
        });
        assert!(!output.shapes.is_empty());
        // No renderer here to take the font atlas.
        output.textures_delta.clear();
    };
    draw(&mut bench, &mut tool);
    drag_handle(&mut bench, &mut tool, 3, Vec2::new(10.0, 0.0));
    bench.frame(&mut tool);
    draw(&mut bench, &mut tool);
    // A running job, a cleared profile (no mesh), a ring, a crossing outline.
    bench.key(&mut tool, Key::Enter);
    draw(&mut bench, &mut tool);
    wait_for_objects(&mut bench, &mut tool, 2);
    clear(&mut bench, &mut tool);
    bench.frame(&mut tool);
    draw(&mut bench, &mut tool);
    bench.call(|cx| tool.apply_preset(cx, LathePreset::Ring));
    bench.frame(&mut tool);
    draw(&mut bench, &mut tool);
    let crossing = [(0.1, 0.0), (0.5, 1.0), (0.5, 0.0), (0.1, 1.0)].map(|(r, h)| crate::ProfilePoint::smooth(r, h));
    tool.profile.set_points(&crossing, false);
    bench.frame(&mut tool);
    assert!(tool.build.self_intersecting);
    draw(&mut bench, &mut tool);
}

// ---------------------------------------------------------------------------------- demo

#[test]
fn the_demos_reach_the_states_they_describe() {
    let run = |what: &str| {
        let (mut bench, mut tool) = bench();
        bench.call(|cx| Tool::demo(&mut tool, cx, what));
        bench.frame(&mut tool);
        (bench, tool)
    };

    let (bench, tool) = run("");
    assert_eq!((tool.profile.len(), tool.history.steps()), (7, 0));
    assert!(bench.status.is_some());

    let (_, tool) = run("edit");
    assert_eq!(tool.selected(), Some(4));
    assert!(position(&tool, 4).x > 0.3, "the shoulder was pulled out");
    assert_eq!(tool.history.next_undo(&tool.profile, &tool.plane), Some("Move Point"));
    assert!(!tool.dragging());

    let (_, tool) = run("bowl");
    assert_eq!(tool.profile.len(), 9);
    let (_, tool) = run("ring");
    assert!(tool.profile.closed_loop());

    let (_, tool) = run("sketch");
    assert!((4..=18).contains(&tool.profile.len()) && tool.profile.len() != 7, "{}", tool.profile.len());
    assert_eq!(tool.history.next_undo(&tool.profile, &tool.plane), Some("Sketch Profile"));

    let (bench, tool) = run("created");
    assert_eq!(bench.doc.objects().len(), 2);
    assert!(bench.finished);
    assert!(tool.created().is_some());
}

#[test]
fn no_gesture_is_left_hanging_by_an_operation_from_the_panel() {
    let (mut bench, mut tool) = bench();
    press_handle(&mut bench, &mut tool, 3, Button::Left);
    drag_handle_to(&mut bench, &mut tool, 3, Vec2::new(30.0, 0.0));
    assert!(matches!(tool.drag, Drag::Point { .. }));
    bench.call(|cx| tool.apply_preset(cx, LathePreset::Bottle));
    assert!(matches!(tool.drag, Drag::None));
    assert_eq!(tool.history.steps(), 2, "the drag, then the preset");
}

/// What one drag step costs: the point moves, the profile is retessellated and the preview rebuilt.
/// `cargo test -p sculpt-lathe --target-dir target/w/lathe --release -- --ignored --nocapture timing`
#[test]
#[ignore = "timing run"]
fn timing_of_a_preview_rebuild_during_a_drag() {
    for preset in [LathePreset::Vase, LathePreset::Bowl, LathePreset::Ring] {
        for segments in [128, 256, 1024] {
            let (mut bench, mut tool) = bench();
            bench.call(|cx| tool.apply_preset(cx, preset));
            tool.set_segments(segments);
            bench.frame(&mut tool);
            let grabbed = handle(&bench, &tool, 2);
            bench.press(&mut tool, grabbed, Button::Left);
            let steps = 60;
            let started = Instant::now();
            for step in 1..=steps {
                bench.drag_to(&mut tool, grabbed + Vec2::new(step as f32 * 0.5, 0.0));
                bench.frame(&mut tool);
            }
            let per_step = started.elapsed().as_secs_f64() * 1000.0 / steps as f64;
            bench.release(&mut tool);
            println!(
                "{:<5} {:>4} segments: {per_step:.2} ms per drag step, {} preview triangles, last build {:.2} ms",
                preset.name(),
                segments,
                tool.preview().data().indices.len() / 3,
                tool.built.last_milliseconds
            );
        }
    }
}
