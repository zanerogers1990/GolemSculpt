//! The gizmo's arrows on a picked point of the path: the tool driven the way
//! the app drives it, against the toolkit's bench. The Unity code these are
//! ported from is `ArrayController.RefreshGizmoTarget` and `ArrayPointTarget`:
//! the arrows (and only the arrows) sit on a picked point, a drag of one slides
//! the point along that axis and the copies follow, and a drag is one undo step.

use glam::{Mat4, Quat, Vec2, Vec3};
use sculpt_toolkit::testing::{Bench, cube};
use sculpt_toolkit::{Button, Command, Draw, Event, GizmoPhase, Key, ObjectId, Placement, Scene, Tool};

use crate::copies;
use crate::layout::compute;
use crate::tool::pose_of;
use crate::ArrayTool;

/// A small cube at the right of the picture with a straight column of five
/// copies standing up from it, 0.5 apart (the camera looks down -Z with +X to
/// the right, and one unit is about 104 pixels at the origin). Mirrored across
/// `axes` through the world's middle when that is not 0.
fn column(axes: u8) -> (Bench, ArrayTool, ObjectId) {
    let mut bench = Bench::new();
    let object = bench.add("Cube", cube(0.2));
    let placement = Placement::at(Vec3::new(1.2, -1.0, 0.0));
    bench.call(|cx| cx.commit("Place", vec![Command::Place { object, placement }]));
    let mut tool = ArrayTool::default();
    bench.enter(&mut tool);
    bench.call(|cx| tool.make_line(cx));
    bench.call(|cx| {
        tool.edit(cx, "Move Array Point", false, |a| {
            a.points[1] = Vec3::new(0.0, 2.0, 0.0);
            a.settings.space_by_size = false;
        })
    });
    if axes != 0 {
        bench.mirror(object, axes);
    }
    bench.frame(&mut tool);
    (bench, tool, object)
}

/// The same with a bend: three points, so a point can be taken out, and a
/// taper so that the copies are not all alike.
fn bend() -> (Bench, ArrayTool, ObjectId) {
    let (mut bench, mut tool, object) = column(0);
    bench.call(|cx| {
        tool.edit(cx, "Move Array Point", false, |a| {
            a.insert_point(0, 0.5);
            a.points[1].x += 0.8;
            a.settings.end_scale = 0.5;
        })
    });
    bench.frame(&mut tool);
    (bench, tool, object)
}

/// The array's circle: eight copies round a centre to the left of the object.
fn ring() -> (Bench, ArrayTool, ObjectId) {
    let mut bench = Bench::new();
    let object = bench.add("Cube", cube(0.2));
    let placement = Placement::at(Vec3::new(1.2, -0.4, 0.0));
    bench.call(|cx| cx.commit("Place", vec![Command::Place { object, placement }]));
    let mut tool = ArrayTool::default();
    bench.enter(&mut tool);
    bench.call(|cx| tool.make_circle(cx));
    bench.frame(&mut tool);
    (bench, tool, object)
}

fn copies_of(draw: &Draw, object: ObjectId) -> Vec<Mat4> {
    draw.object_copies().iter().filter(|(id, _)| *id == object).flat_map(|(_, m)| m.clone()).collect()
}

fn placement(bench: &Bench, object: ObjectId) -> Placement {
    bench.doc.info(object).unwrap().placement
}

/// Where the point is in the world.
fn world(bench: &Bench, tool: &ArrayTool, object: ObjectId, index: usize) -> Vec3 {
    placement(bench, object).position + tool.state(object).points[index]
}

#[track_caller]
fn near(got: Vec3, expected: Vec3, what: &str) {
    assert!(got.distance(expected) < 1e-5, "{what}: {got} is not {expected}");
}

/// Picks a point by clicking its dot, as the user does.
fn pick(bench: &mut Bench, tool: &mut ArrayTool, object: ObjectId, index: usize) {
    let at = bench.px(world(bench, tool, object, index));
    bench.click(tool, at, Button::Left);
    bench.frame(tool);
    assert_eq!(tool.picked(), Some(index));
}

/// The copies the array should be drawing, worked out afresh from its state.
fn expected_copies(bench: &Bench, tool: &ArrayTool, object: ObjectId) -> Vec<Mat4> {
    let state = tool.state(object);
    let info = bench.doc.info(object).unwrap();
    copies::drawn(&compute(&state.settings, &state.points), &pose_of(info.placement), info.mirror, 1)
}

fn tool_steps(tool: &ArrayTool) -> usize {
    tool.history.undo.len()
}

// ---- when the arrows show --------------------------------------------------------------------

#[test]
fn a_picked_point_has_the_arrows_and_nothing_else_on_it() {
    let (mut bench, mut tool, object) = column(0);
    // Making a line picks its end point, as in the Unity app.
    let target = tool.gizmo().expect("the end point is picked and the arrows are on it");
    assert!(target.moves && !target.turns && !target.scales, "a point can only be moved");
    assert_eq!(target.placement.rotation, Quat::IDENTITY);
    near(target.placement.position, world(&bench, &tool, object, 1), "the arrows are on the point");
    assert_eq!(target.radius, 0.0, "the middle of the gizmo is the point's own to grab");

    // The square start point is a point too.
    pick(&mut bench, &mut tool, object, 0);
    near(tool.gizmo().unwrap().placement.position, Vec3::new(1.2, -1.0, 0.0), "the object's own place");

    // Esc lets go of it, and the arrows go.
    assert!(bench.key(&mut tool, Key::Escape));
    bench.frame(&mut tool);
    assert_eq!(tool.picked(), None);
    assert!(tool.gizmo().is_none());
}

#[test]
fn the_arrows_are_only_there_while_the_tool_is_up_and_the_panels_are_showing() {
    let (mut bench, mut tool, object) = column(0);
    assert!(tool.gizmo().is_some());

    bench.env.active = false;
    bench.frame(&mut tool);
    assert!(tool.gizmo().is_none(), "the tool is away: the point is still picked, the gizmo is not offered");
    assert_eq!(tool.picked(), Some(1));
    bench.env.active = true;
    bench.frame(&mut tool);
    assert!(tool.gizmo().is_some(), "and it is back when the tool is");

    bench.env.clean_view = true;
    bench.frame(&mut tool);
    assert!(tool.gizmo().is_none(), "no handles in the clean view");
    bench.env.clean_view = false;

    // Another object: the point belonged to the first one's array.
    let other = bench.add("Other", cube(0.1));
    bench.frame(&mut tool);
    assert!(tool.gizmo().is_none() && tool.picked().is_none());
    bench.call(|cx| cx.commit("Select", vec![Command::Select { object }]));
    bench.frame(&mut tool);
    assert!(tool.gizmo().is_none(), "and picking has to be done again");
    let _ = other;
}

#[test]
fn a_click_on_nothing_lets_go_of_the_point_and_so_does_taking_the_array_off() {
    let (mut bench, mut tool, object) = column(0);
    assert!(tool.gizmo().is_some());
    bench.click(&mut tool, Vec2::new(40.0, 40.0), Button::Left);
    bench.frame(&mut tool);
    assert!(tool.gizmo().is_none() && tool.picked().is_none());

    pick(&mut bench, &mut tool, object, 1);
    assert!(tool.gizmo().is_some());
    bench.call(|cx| tool.remove(cx));
    bench.frame(&mut tool);
    assert!(tool.gizmo().is_none(), "no array, no point");
}

#[test]
fn deleting_the_point_or_undoing_its_adding_takes_the_arrows_with_it() {
    let (mut bench, mut tool, object) = bend();
    assert_eq!(tool.state(object).points.len(), 3);
    pick(&mut bench, &mut tool, object, 1);
    assert!(tool.gizmo().is_some());
    assert!(bench.key(&mut tool, Key::Delete));
    bench.frame(&mut tool);
    assert_eq!(tool.state(object).points.len(), 2);
    assert!(tool.gizmo().is_none(), "the point is gone");

    // Undone, the point is back but not picked.
    assert!(bench.undo(&mut tool));
    bench.frame(&mut tool);
    assert_eq!(tool.state(object).points.len(), 3);
    assert!(tool.gizmo().is_none());

    // Picked again and then undone away from under the gizmo.
    pick(&mut bench, &mut tool, object, 2);
    assert!(tool.gizmo().is_some());
    while tool.state(object).points.len() > 2 {
        assert!(bench.undo(&mut tool));
    }
    bench.frame(&mut tool);
    assert_eq!(tool.picked(), None, "the point that was picked is not there any more");
    assert!(tool.gizmo().is_none());
}

#[test]
fn a_circle_has_arrows_on_the_object_and_on_its_centre_and_nowhere_else() {
    let (mut bench, mut tool, object) = ring();
    let centre = world(&bench, &tool, object, 1);
    near(tool.gizmo().expect("making a circle picks its centre").placement.position, centre, "the centre");
    pick(&mut bench, &mut tool, object, 0);
    near(tool.gizmo().unwrap().placement.position, placement(&bench, object).position, "the object");
    // Nothing else on a circle is a point to pick.
    let state = tool.state(object);
    assert_eq!(state.handle_count(), 2);
}

// ---- what a drag does ------------------------------------------------------------------------

#[test]
fn a_drag_of_an_arrow_moves_the_point_by_exactly_the_drag_along_that_axis_and_the_copies_follow() {
    for by in [Vec3::new(0.6, 0.0, 0.0), Vec3::new(0.0, -0.7, 0.0), Vec3::new(0.0, 0.0, 0.45)] {
        let (mut bench, mut tool, object) = column(0);
        let before = tool.state(object);
        let target = tool.gizmo().unwrap().placement;
        assert!(bench.gizmo(&mut tool, Placement::at(target.position + by)));

        let after = tool.state(object);
        near(after.points[1], before.points[1] + by, "the point has gone exactly as far as the arrow was dragged");
        assert_eq!(after.points[0], before.points[0], "the others are where they were");
        assert_eq!(after.settings, before.settings);
        for axis in 0..3 {
            if by[axis] == 0.0 {
                assert_eq!(after.points[1][axis], before.points[1][axis], "no drift off the axis ({axis})");
            }
        }

        // The copies follow as the layout says: drawn from the new path, the last at its end.
        let draw = bench.frame(&mut tool);
        let drawn = copies_of(&draw, object);
        assert_eq!(drawn, expected_copies(&bench, &tool, object));
        assert_eq!(drawn.len(), 4);
        near(drawn[3].w_axis.truncate(), world(&bench, &tool, object, 1), "the last copy sits on the end point");
        // The arrows went with the point.
        near(tool.gizmo().unwrap().placement.position, target.position + by, "the arrows follow it");
    }
}

#[test]
fn a_drag_is_one_step_of_the_tools_own_history_and_undo_puts_everything_back_exactly() {
    let (mut bench, mut tool, object) = column(0);
    let (array_before, steps, scene_steps) = (tool.state(object), tool_steps(&tool), bench.doc.undo_steps());
    let target = tool.gizmo().unwrap().placement;

    // The gizmo's own drag is a Begin, a Move for each sample and an End.
    bench.send(&mut tool, Event::Gizmo(target, GizmoPhase::Begin));
    for i in 1..=5 {
        let at = target.position + Vec3::new(0.1, 0.2, 0.05) * i as f32;
        bench.send(&mut tool, Event::Gizmo(Placement::at(at), GizmoPhase::Move));
        bench.frame(&mut tool);
    }
    let end = target.position + Vec3::new(0.5, 1.0, 0.25);
    bench.send(&mut tool, Event::Gizmo(Placement::at(end), GizmoPhase::End));
    let after = tool.state(object);
    near(world(&bench, &tool, object, 1), end, "where the drag ended");

    assert_eq!(tool_steps(&tool), steps + 1, "however many samples, one step");
    assert_eq!(bench.doc.undo_steps(), scene_steps, "and none of the scene's: the array is the tool's");
    assert!(tool.can_undo(false));

    assert!(bench.undo(&mut tool));
    assert_eq!(tool.state(object), array_before, "bit for bit what it was");
    assert_eq!(bench.doc.undo_steps(), scene_steps);
    assert!(bench.redo(&mut tool));
    assert_eq!(tool.state(object), after);
}

#[test]
fn a_press_on_an_arrow_that_never_moves_is_no_edit() {
    let (mut bench, mut tool, object) = column(0);
    let (before, steps) = (tool.state(object), tool_steps(&tool));
    let target = tool.gizmo().unwrap().placement;
    bench.send(&mut tool, Event::Gizmo(target, GizmoPhase::Begin));
    bench.send(&mut tool, Event::Gizmo(target, GizmoPhase::End));
    assert_eq!(tool.state(object), before);
    assert_eq!(tool_steps(&tool), steps);
    // Nor is one that wandered off and came back to within the rounding of the gizmo's numbers.
    bench.send(&mut tool, Event::Gizmo(target, GizmoPhase::Begin));
    let grain = Placement::at(target.position + Vec3::splat(1e-8));
    bench.send(&mut tool, Event::Gizmo(grain, GizmoPhase::Move));
    bench.send(&mut tool, Event::Gizmo(grain, GizmoPhase::End));
    assert_eq!(tool.state(object), before);
    assert_eq!(tool_steps(&tool), steps);
}

#[test]
fn a_drag_cut_short_by_putting_the_tool_away_ends_where_it_is_as_one_step() {
    let (mut bench, mut tool, object) = column(0);
    let steps = tool_steps(&tool);
    let target = tool.gizmo().unwrap().placement;
    let to = target.position + Vec3::new(0.0, 0.0, 0.8);
    bench.send(&mut tool, Event::Gizmo(target, GizmoPhase::Begin));
    bench.send(&mut tool, Event::Gizmo(Placement::at(to), GizmoPhase::Move));
    bench.leave(&mut tool);
    near(world(&bench, &tool, object, 1), to, "the point stays where the arrow took it");
    assert_eq!(tool_steps(&tool), steps + 1);
    assert!(bench.undo(&mut tool));
    near(world(&bench, &tool, object, 1), target.position, "and one Z takes it back");
}

#[test]
fn dragging_the_dot_itself_is_still_the_free_move_and_the_arrows_follow_it() {
    let (mut bench, mut tool, object) = column(0);
    let dot = bench.px(world(&bench, &tool, object, 1));
    let steps = tool_steps(&tool);
    // A press on the point is the tool's own (`GizmoTarget::radius`), never an arrow's.
    assert!(bench.press(&mut tool, dot, Button::Left));
    bench.drag_to(&mut tool, dot + Vec2::new(70.0, 20.0));
    bench.drag_to(&mut tool, dot + Vec2::new(140.0, 40.0));
    bench.release(&mut tool);
    let moved = world(&bench, &tool, object, 1);
    assert!(moved.x > 1.2 + 0.5, "{moved}: the point went across the screen");
    assert_eq!(tool_steps(&tool), steps + 1);
    bench.frame(&mut tool);
    near(tool.gizmo().unwrap().placement.position, moved, "the arrows are where the point is");
}

// ---- the square start point ------------------------------------------------------------------

#[test]
fn the_arrows_on_the_square_move_the_object_and_its_array_as_one_step_of_the_scene() {
    let (mut bench, mut tool, object) = column(0);
    pick(&mut bench, &mut tool, object, 0);
    let (array_before, steps, scene_steps) = (tool.state(object), tool_steps(&tool), bench.doc.undo_steps());
    let (from, target) = (placement(&bench, object), tool.gizmo().unwrap().placement);
    near(target.position, from.position, "the arrows are on the object's own place");
    let by = Vec3::new(-0.3, 0.4, 0.2);

    bench.send(&mut tool, Event::Gizmo(target, GizmoPhase::Begin));
    bench.send(&mut tool, Event::Gizmo(Placement::at(target.position + by * 0.5), GizmoPhase::Move));
    bench.send(&mut tool, Event::Gizmo(Placement::at(target.position + by), GizmoPhase::Move));
    // Shown while it is dragged: the object where the drag has it, the scene's own left out.
    let draw = bench.frame(&mut tool);
    assert_eq!(draw.hidden(), [object]);
    assert_eq!(placement(&bench, object), from, "nothing is committed to the scene yet");
    let shown = copies_of(&draw, object);
    assert_eq!(shown.len(), 5, "the object and its four copies, all carried");
    near(shown[0].w_axis.truncate(), from.position + by, "the object is where the drag has it");
    near(tool.gizmo().unwrap().placement.position, target.position + by, "and the arrows with it");

    bench.send(&mut tool, Event::Gizmo(Placement::at(target.position + by), GizmoPhase::End));
    near(placement(&bench, object).position, from.position + by, "the object is where the arrows took it");
    assert_eq!(placement(&bench, object).rotation, from.rotation);
    assert_eq!(tool.state(object), array_before, "the array's points are offsets from the object: they ride along");
    assert_eq!(bench.doc.undo_steps(), scene_steps + 1, "one step of the scene's history");
    assert_eq!(tool_steps(&tool), steps, "and none of the tool's");
    let draw = bench.frame(&mut tool);
    assert!(draw.hidden().is_empty());
    assert_eq!(copies_of(&draw, object), expected_copies(&bench, &tool, object));
    assert_eq!(tool.picked(), Some(0), "the square is still picked");

    // Z answers the tool's own steps first (the edit that set the column up); this one is the scene's.
    assert!(bench.undo_scene());
    assert_eq!(placement(&bench, object), from, "exactly where it was");
}

// ---- beside the gizmo on a copy ----------------------------------------------------------------

/// A click on the surface of the cube as it is drawn at `centre`, a little off
/// to the side so the click is clear of the path (a click on the path adds a
/// point there).
fn click_copy(bench: &mut Bench, tool: &mut ArrayTool, centre: Vec3) {
    let on = bench.px(centre + Vec3::new(0.12, 0.0, 0.2));
    bench.click(tool, on, Button::Left);
    bench.frame(tool);
}

#[test]
fn clicking_a_copy_moves_the_gizmo_from_the_point_to_the_copy_and_clicking_the_point_moves_it_back() {
    let (mut bench, mut tool, object) = column(0);
    pick(&mut bench, &mut tool, object, 1);
    assert!(tool.picked_copy().is_none());
    let arrows = tool.gizmo().unwrap();
    assert!(!arrows.turns);

    // The third copy of five stands at 1.0 above the object.
    let third = placement(&bench, object).position + Vec3::new(0.0, 1.0, 0.0);
    click_copy(&mut bench, &mut tool, third);
    assert_eq!(tool.picked(), None, "the gizmo is on a point or on a copy, not both");
    assert_eq!(tool.picked_copy().map(|c| (c.copy, c.side)), Some((2, 0)));
    let copy = tool.gizmo().expect("the gizmo is on the copy now");
    assert!(copy.moves && copy.turns && !copy.scales, "a copy has its rings as well");
    near(copy.placement.position, third, "at the copy");

    // And back: a point picks the point and lets go of the copy.
    pick(&mut bench, &mut tool, object, 1);
    assert!(tool.picked_copy().is_none());
    let back = tool.gizmo().unwrap();
    assert!(!back.turns);
    near(back.placement.position, world(&bench, &tool, object, 1), "on the point again");
}

#[test]
fn a_drag_of_the_gizmo_goes_to_what_it_sits_on() {
    let (mut bench, mut tool, object) = column(0);
    // On a copy it poses them all, a step of the scene's.
    let third = placement(&bench, object).position + Vec3::new(0.0, 1.0, 0.0);
    click_copy(&mut bench, &mut tool, third);
    let scene_steps = bench.doc.undo_steps();
    let array_before = tool.state(object);
    let target = tool.gizmo().unwrap().placement;
    assert!(bench.gizmo(&mut tool, Placement::at(target.position + Vec3::new(0.5, 0.0, 0.0))));
    assert_eq!(bench.doc.undo_steps(), scene_steps + 1);
    assert_ne!(tool.state(object), array_before, "the path stayed and the object moved off it");
    assert!(bench.undo(&mut tool));
    bench.frame(&mut tool);
    assert_eq!(tool.state(object), array_before, "the tool follows when the scene takes the move back");

    // Then on a point it moves the point, and does not touch the object.
    let from = placement(&bench, object);
    pick(&mut bench, &mut tool, object, 1);
    let target = tool.gizmo().unwrap().placement;
    assert!(bench.gizmo(&mut tool, Placement::at(target.position + Vec3::new(0.0, 0.0, 0.5))));
    assert_eq!(placement(&bench, object), from);
    near(tool.state(object).points[1], array_before.points[1] + Vec3::new(0.0, 0.0, 0.5), "the point");
}

// ---- a mirrored array ----------------------------------------------------------------------------

#[test]
fn the_arrows_are_on_the_straight_side_and_both_sides_follow_the_drag() {
    let (mut bench, mut tool, object) = column(1);
    // The path is drawn on the object's side only, so the arrows are there.
    near(tool.gizmo().unwrap().placement.position, world(&bench, &tool, object, 1), "on the object's side");
    let target = tool.gizmo().unwrap().placement;
    let by = Vec3::new(0.4, 0.3, 0.0);
    assert!(bench.gizmo(&mut tool, Placement::at(target.position + by)));

    let draw = bench.frame(&mut tool);
    let drawn = copies_of(&draw, object);
    assert_eq!(drawn.len(), 8, "four copies, and their four reflections");
    assert_eq!(drawn, expected_copies(&bench, &tool, object));
    for i in 0..4 {
        let (straight, reflected) = (drawn[i].w_axis.truncate(), drawn[4 + i].w_axis.truncate());
        near(reflected, Vec3::new(-straight.x, straight.y, straight.z), "the far side follows");
    }
    assert!(bench.undo(&mut tool));
    bench.frame(&mut tool);
    assert_eq!(copies_of(&bench.frame(&mut tool), object), expected_copies(&bench, &tool, object));
}

// ---- the copies as places of the object --------------------------------------------------------

#[test]
fn the_copies_are_said_to_be_places_of_the_object_so_the_rest_of_the_app_sees_them() {
    let (mut bench, mut tool, object) = column(1);
    let said = bench.doc.info(object).unwrap().live_copies.to_vec();
    assert_eq!(said, expected_copies(&bench, &tool, object), "exactly the copies it draws");
    assert_eq!(said.len(), 8, "four copies and their four reflections");
    assert_eq!(bench.doc.info(object).unwrap().all_places().len(), 2 + 8, "with the object and its own mirror copy");

    // The scene's pointer meets a copy and names the object: the third copy stands 1.0 above it.
    let third = placement(&bench, object).position + Vec3::new(0.0, 1.0, 0.0);
    let on_it = bench.px(third + Vec3::new(0.05, 0.0, 0.2));
    let hit = bench.doc.raycast(&bench.env.view.ray(on_it)).expect("a copy is there");
    assert_eq!(hit.object, object);
    assert!((hit.position.z - 0.2).abs() < 1e-4 && hit.position.distance(third) < 0.4, "{:?}", hit.position);

    // Away, the copies are still on screen and still the object's.
    bench.env.active = false;
    bench.frame(&mut tool);
    assert_eq!(bench.doc.info(object).unwrap().live_copies.len(), 8);
    bench.env.active = true;

    // While a drag shows the object where the scene does not have it yet, they are drawn but not said.
    let start = bench.px(Vec3::new(1.2, -1.0, 0.0));
    assert!(bench.press(&mut tool, start, Button::Left));
    bench.drag_to(&mut tool, start + Vec2::new(0.0, -40.0));
    let draw = bench.frame(&mut tool);
    assert!(!copies_of(&draw, object).is_empty(), "drawn");
    let none_said = draw.object_places().is_empty() && bench.doc.info(object).unwrap().live_copies.is_empty();
    assert!(none_said, "but not places");
    bench.release(&mut tool);
    bench.frame(&mut tool);
    assert_eq!(bench.doc.info(object).unwrap().live_copies.len(), 8, "and again once the object is where it was put");

    // Hidden, the object has no copies on screen; taken off, none at all.
    bench.call(|cx| cx.commit("Hide", vec![Command::Show { object, visible: false }]));
    bench.frame(&mut tool);
    assert!(bench.doc.info(object).unwrap().live_copies.is_empty());
    bench.call(|cx| cx.commit("Show", vec![Command::Show { object, visible: true }]));
    bench.frame(&mut tool);
    assert_eq!(bench.doc.info(object).unwrap().live_copies.len(), 8);
    bench.call(|cx| tool.remove(cx));
    bench.frame(&mut tool);
    assert!(bench.doc.info(object).unwrap().live_copies.is_empty(), "no array, no places");
}

#[test]
fn making_the_copies_real_takes_the_places_away_and_undoing_it_gives_them_back() {
    let (mut bench, mut tool, object) = column(0);
    assert_eq!(bench.doc.info(object).unwrap().live_copies.len(), 4);
    bench.call(|cx| tool.apply(cx, crate::Apply::Separate));
    bench.frame(&mut tool);
    assert!(bench.doc.info(object).unwrap().live_copies.is_empty(), "the copies are objects of their own now");
    assert!(bench.undo_scene());
    bench.frame(&mut tool);
    assert_eq!(bench.doc.info(object).unwrap().live_copies.len(), 4, "the live array is back, and so are its places");
}

// ---- the hint -------------------------------------------------------------------------------------

#[test]
fn the_hint_says_what_the_arrows_are_for_while_a_point_is_picked() {
    let (mut bench, mut tool, object) = column(0);
    let hint = bench.call(|cx| tool.hint(cx));
    assert!(hint.contains("arrow"), "{hint}");
    pick(&mut bench, &mut tool, object, 0);
    let hint = bench.call(|cx| tool.hint(cx));
    assert!(hint.contains("square") && hint.contains("arrow"), "{hint}");
    bench.click(&mut tool, Vec2::new(40.0, 40.0), Button::Left);
    let hint = bench.call(|cx| tool.hint(cx));
    assert!(!hint.contains("arrow"), "{hint}");
}
