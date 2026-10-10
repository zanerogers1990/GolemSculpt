//! Mirroring the array, clicking a copy to pose them all with the gizmo, and
//! Copies Back onto the Path: the tool driven the way the app drives it, against
//! the toolkit's bench. The Unity tests these are ported from are
//! `ArrayRepeaterTests` (the mirrored array, the path that starts off the
//! object); the rest check the gizmo and the undo steps against what
//! `ArrayController` does.
//!
//! The scene's Z is stepped with `bench.undo`, which asks the tool first and
//! then the scene, as the app does. The app draws a frame between one Z and
//! the next, which is when the tool sees what the scene undid, so these tests
//! call `bench.frame` between them too.

use glam::{Mat4, Quat, Vec2, Vec3};
use sculpt_geom::mirror::reflect_point;
use sculpt_toolkit::testing::{Bench, cube};
use sculpt_toolkit::{Button, Command, Draw, Key, ObjectId, Placement, Scene, Symmetry, Tool};

use crate::tool_tests::Panel;
use crate::{Apply, ArrayState, ArrayTool, CopyPick};

/// A small cube at the right of the picture, `column(0)` with a straight column
/// of five copies standing up from it, 0.5 apart (the camera looks down -Z with
/// +X to the right, and one unit is about 104 pixels at the origin). Mirrored
/// across `axes` through the world's middle when that is not 0, so the copies
/// have their reflections at the left.
fn column(axes: u8) -> (Bench, ArrayTool, ObjectId) {
    let mut bench = Bench::new();
    let object = bench.add("Cube", cube(0.2));
    bench.call(|cx| cx.commit("Place", vec![Command::Place { object, placement: Placement::at(Vec3::new(1.2, -1.0, 0.0)) }]));
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

/// A ring of eight copies turning as they go round, so that a copy's frame on
/// the path is not the identity.
fn ring() -> (Bench, ArrayTool, ObjectId) {
    let mut bench = Bench::new();
    let object = bench.add("Cube", cube(0.2));
    bench.call(|cx| cx.commit("Place", vec![Command::Place { object, placement: Placement::at(Vec3::new(1.2, -0.4, 0.0)) }]));
    let mut tool = ArrayTool::default();
    bench.enter(&mut tool);
    bench.call(|cx| tool.make_circle(cx));
    bench.frame(&mut tool);
    (bench, tool, object)
}

fn copies(draw: &Draw, object: ObjectId) -> Vec<Mat4> {
    draw.object_copies().iter().filter(|(id, _)| *id == object).flat_map(|(_, m)| m.clone()).collect()
}

fn at(m: &Mat4) -> Vec3 {
    m.w_axis.truncate()
}

fn placement(bench: &Bench, object: ObjectId) -> Placement {
    bench.doc.info(object).unwrap().placement
}

/// The angle between two rotations in radians (through the vector part of their difference: `acos` of
/// a dot product that rounds to 1 gives 5e-4 for two rotations that are the same).
fn apart(a: Quat, b: Quat) -> f32 {
    let d = a * b.inverse();
    2.0 * d.xyz().length().atan2(d.w.abs())
}

#[track_caller]
fn near(got: Vec3, expected: Vec3, what: &str) {
    assert!(got.distance(expected) < 1e-4, "{what}: {got} is not {expected}");
}

/// Where each copy is in the world: the object's place plus the copy's offset.
fn copy_positions(tool: &ArrayTool, bench: &Bench, object: ObjectId) -> Vec<Vec3> {
    let origin = placement(bench, object).position;
    tool.arrays.get(object).unwrap().layout.iter().map(|p| origin + p.offset).collect()
}

/// Clicks on the surface of the object's mesh as it is drawn at `centre`, a
/// little off to the side so the click is clear of the path (a click on the
/// path adds a point there).
fn click_copy(bench: &mut Bench, tool: &mut ArrayTool, centre: Vec3, aside: f32) {
    let on = bench.px(centre + Vec3::new(aside, 0.0, 0.2));
    bench.click(tool, on, Button::Left);
}

fn picked(tool: &ArrayTool) -> Option<(usize, u8)> {
    tool.picked_copy().map(|p| (p.copy, p.side))
}

// ---- the mirrored array ---------------------------------------------------------------------

/// The Unity test `AMirroredObjectsArrayIsMirroredToo`.
#[test]
fn a_mirrored_objects_array_is_drawn_again_reflected_on_the_other_side() {
    let (mut bench, mut tool, object) = column(1);
    let all = copies(&bench.frame(&mut tool), object);
    assert_eq!(all.len(), 8, "four copies, and their four reflections");
    let signs = Vec3::new(-1.0, 1.0, 1.0);
    for i in 0..4 {
        let (straight, reflected) = (all[i], all[4 + i]);
        // Any point of the object's shape lands on its mirror image.
        let local = Vec3::new(0.1, -0.05, 0.15);
        let want = reflect_point(straight.transform_point3(local), Vec3::ZERO, signs);
        near(reflected.transform_point3(local), want, &format!("copy {i} is the mirror image"));
        assert!(straight.determinant() > 0.0 && reflected.determinant() < 0.0, "copy {i} is drawn reflected");
    }
    // The first four are where the unmirrored array has them: the object's own mirror copy is the app's to draw.
    near(at(&all[0]), Vec3::new(1.2, -0.5, 0.0), "the first copy beside the object");
    near(at(&all[4]), Vec3::new(-1.2, -0.5, 0.0), "and its reflection");

    // Two axes: three reflections of each copy, and X together with Z is a rotation.
    bench.mirror(object, 1 | 4);
    let two = copies(&bench.frame(&mut tool), object);
    assert_eq!(two.len(), 16);
    let reflecting: Vec<bool> = two.iter().map(|m| m.determinant() < 0.0).collect();
    assert_eq!(reflecting[12..16], [false; 4], "X and Z together is a rotation, not a reflection");
    assert_eq!(reflecting[4..8], [true; 4]);

    bench.mirror(object, 0);
    assert_eq!(copies(&bench.frame(&mut tool), object).len(), 4, "mirror off: only the straight copies are left");
}

/// A copy count is the kind of value a limit can go wrong on: the fewest copies there can be, and the most.
#[test]
fn the_fewest_and_the_most_copies_are_mirrored_too() {
    let (mut bench, mut tool, object) = column(7);
    for (count, perside) in [(crate::ArraySettings::MIN_COUNT, 1), (3, 2), (crate::ArraySettings::MAX_COUNT, 99)] {
        bench.call(|cx| tool.edit(cx, "Array Count", false, |a| a.settings.count = count));
        let all = copies(&bench.frame(&mut tool), object);
        assert_eq!(all.len(), perside * 8, "{count} copies, the object's own mirrors left to the app");
        let reflecting = all.iter().filter(|m| m.determinant() < 0.0).count();
        assert_eq!(reflecting, perside * 4, "four of the eight sides are reflections");
    }
}

#[test]
fn a_hidden_object_shows_no_mirrored_copies_either() {
    let (mut bench, mut tool, object) = column(1);
    bench.call(|cx| cx.commit("Hide", vec![Command::Show { object, visible: false }]));
    assert!(copies(&bench.frame(&mut tool), object).is_empty());
}

#[test]
fn while_the_object_is_dragged_it_is_drawn_with_its_own_mirror_copy() {
    let (mut bench, mut tool, object) = column(1);
    let start = bench.px(Vec3::new(1.2, -1.0, 0.0));
    assert!(bench.press(&mut tool, start, Button::Left), "the square start point");
    bench.drag_to(&mut tool, start + Vec2::new(0.0, -40.0));
    let draw = bench.frame(&mut tool);
    assert_eq!(draw.hidden(), [object], "the scene's own is left out while the drag shows it elsewhere");
    let all = copies(&draw, object);
    assert_eq!(all.len(), 10, "five copies with the object, and their five reflections");
    let (here, there) = (at(&all[0]), at(&all[5]));
    assert!(here.x > 1.0 && (there.x + here.x).abs() < 1e-4 && (there.y - here.y).abs() < 1e-4, "{here} and {there}");
    bench.release(&mut tool);
    let all = copies(&bench.frame(&mut tool), object);
    assert_eq!(all.len(), 8, "the object's own mirror is the app's to draw again");
}

#[test]
fn separate_objects_of_a_mirrored_array_are_mirrored_too_and_one_undo_takes_them_all_away() {
    let (mut bench, mut tool, object) = column(1 | 2);
    let mirror = bench.doc.info(object).unwrap().mirror;
    assert!(mirror.is_some());
    let steps = bench.doc.undo_steps();
    bench.call(|cx| tool.apply(cx, Apply::Separate));

    let objects = bench.doc.objects();
    assert_eq!(objects.len(), 5, "the object and a new one for each of the four copies");
    assert_eq!(bench.doc.undo_steps(), steps + 1, "one step");
    for &id in &objects {
        let info = bench.doc.info(id).unwrap();
        assert_eq!(info.mirror, mirror, "{} has the same live mirror", info.name);
        assert_eq!(info.placements().len(), 4);
    }
    // Where each is: straight up from the object, as the copies were.
    let second = bench.doc.info(objects[2]).unwrap();
    near(second.placement.position, Vec3::new(1.2, 0.0, 0.0), "the third copy is where the array had it");

    assert!(bench.undo_scene());
    assert_eq!(bench.doc.objects(), vec![object]);
    bench.frame(&mut tool);
    assert!(tool.has_array(object), "the live array is back, still mirrored");
    assert_eq!(bench.doc.info(object).unwrap().mirror, mirror);
    assert_eq!(copies(&bench.frame(&mut tool), object).len(), 4 * 4);
}

/// Each object an array is turned into is sculpted the way the arrayed object
/// is: it starts with that object's own symmetry settings, as a clone does
/// (the Unity app made them with the same call a clone is made with).
#[test]
fn separate_objects_start_with_the_arrayed_objects_own_symmetry() {
    let (mut bench, mut tool, object) = column(0);
    let own = Symmetry { mirror: [true, false, false], radial_count: 6, ..Symmetry::NONE };
    bench.set_symmetry(object, own);
    bench.call(|cx| tool.apply(cx, Apply::Separate));
    let objects = bench.doc.objects();
    assert_eq!(objects.len(), 5, "the object and a new one for each of the four copies");
    for &id in &objects {
        let name = &bench.doc.info(id).unwrap().name;
        assert_eq!(bench.doc.symmetry(id), own, "{name} is sculpted the way the arrayed object is");
    }
}

#[test]
fn one_object_keeps_the_objects_mirror_so_the_other_side_still_has_every_copy() {
    let (mut bench, mut tool, object) = column(1);
    let before = bench.doc.info(object).unwrap().vertices;
    bench.call(|cx| tool.apply(cx, Apply::One));
    let info = bench.doc.info(object).unwrap();
    assert_eq!(info.vertices, before * 5, "every copy's geometry is in the object");
    assert_eq!(info.mirror.map(|m| m.axis_mask), Some(1), "and the mirror is the object's own, so they are on both sides");
    assert_eq!(info.placements().len(), 2);
    assert!(!tool.has_array(object));
}

#[test]
fn the_mirror_buttons_switch_the_objects_own_mirror_one_step_each() {
    let (mut bench, mut tool, object) = column(0);
    let mut panel = Panel::new();
    let steps = bench.doc.undo_steps();
    assert_eq!(bench.call(|cx| tool.mirror_axes(cx)), 0);

    panel.click(&mut bench, &mut tool, "X");
    let mirror = bench.doc.info(object).unwrap().mirror.expect("X is on");
    assert_eq!((mirror.axis_mask, mirror.center), (1, Vec3::ZERO));
    assert_eq!(bench.doc.undo_steps(), steps + 1, "a step of the scene's history");
    assert_eq!(copies(&bench.frame(&mut tool), object).len(), 8);

    panel.click(&mut bench, &mut tool, "Z");
    assert_eq!(bench.doc.info(object).unwrap().mirror.map(|m| m.axis_mask), Some(1 | 4));
    assert_eq!(copies(&bench.frame(&mut tool), object).len(), 16);
    assert_eq!(bench.doc.undo_steps(), steps + 2);

    panel.click(&mut bench, &mut tool, "X");
    assert_eq!(bench.doc.info(object).unwrap().mirror.map(|m| m.axis_mask), Some(4), "X is switched off again");
    panel.click(&mut bench, &mut tool, "Z");
    assert_eq!(bench.doc.info(object).unwrap().mirror, None, "and so is the last");
    assert_eq!(bench.status.as_ref().map(|s| s.0.as_str()), Some("Mirror off."));
    assert_eq!(copies(&bench.frame(&mut tool), object).len(), 4);
}

#[test]
fn z_takes_the_mirror_back_in_order_with_the_tools_own_edits() {
    let (mut bench, mut tool, object) = column(0);
    bench.call(|cx| tool.edit(cx, "Array Count", false, |a| a.settings.count = 6));
    bench.call(|cx| tool.toggle_mirror(cx, 0));
    assert!(bench.doc.info(object).unwrap().mirror.is_some());

    // The newest edit is the mirror, a step of the scene's, so Z goes to the scene...
    assert!(bench.undo(&mut tool));
    assert_eq!(bench.doc.info(object).unwrap().mirror, None);
    assert_eq!(tool.state(object).settings.count, 6, "and the count is still the edit made before it");
    bench.frame(&mut tool);
    // ...and then to the tool's own, the count.
    assert!(bench.undo(&mut tool));
    assert_eq!(tool.state(object).settings.count, 5);
    bench.frame(&mut tool);

    // Forward again in the same order.
    assert!(bench.redo(&mut tool));
    assert_eq!(tool.state(object).settings.count, 6);
    assert_eq!(bench.doc.info(object).unwrap().mirror, None);
    bench.frame(&mut tool);
    assert!(bench.redo(&mut tool));
    assert!(bench.doc.info(object).unwrap().mirror.is_some());
    bench.frame(&mut tool);
    assert!(!bench.redo(&mut tool));
}

// ---- picking a copy ---------------------------------------------------------------------------

#[test]
fn clicking_a_copy_puts_the_gizmo_on_it_and_a_click_on_nothing_lets_go() {
    let (mut bench, mut tool, object) = column(1);
    click_copy(&mut bench, &mut tool, Vec3::new(1.2, 0.0, 0.0), 0.15);
    assert_eq!(picked(&tool), Some((2, 0)));
    assert!(bench.status.as_ref().is_some_and(|s| s.0.starts_with("Turn or move this copy")));
    bench.frame(&mut tool);
    let target = tool.gizmo().expect("the gizmo is on the copy");
    near(target.placement.position, Vec3::new(1.2, 0.0, 0.0), "at the copy's middle");
    assert!(target.moves && target.turns && !target.scales, "arrows and rings; a copy's size is the array's to set");
    assert!(target.radius > 0.05, "{}", target.radius);

    // A copy on the far side of the mirror, a click on the object itself and on its own mirror copy.
    click_copy(&mut bench, &mut tool, Vec3::new(-1.2, 0.5, 0.0), 0.0);
    assert_eq!(picked(&tool), Some((3, 1)));
    bench.frame(&mut tool);
    near(tool.gizmo().unwrap().placement.position, Vec3::new(-1.2, 0.5, 0.0), "the gizmo sits where the copy is drawn");
    click_copy(&mut bench, &mut tool, Vec3::new(1.2, -1.0, 0.0), 0.15);
    assert_eq!(picked(&tool), Some((0, 0)), "the object is copy 0");
    click_copy(&mut bench, &mut tool, Vec3::new(-1.2, -1.0, 0.0), 0.0);
    assert_eq!(picked(&tool), Some((0, 1)), "and so is its own reflection");
    assert_eq!(tool.picked_copy().unwrap().object, object);

    bench.click(&mut tool, Vec2::new(20.0, 20.0), Button::Left);
    assert_eq!(tool.picked_copy(), None);
    bench.frame(&mut tool);
    assert!(tool.gizmo().is_none());
}

#[test]
fn a_point_and_a_copy_are_never_picked_together_and_esc_lets_go_of_each_in_turn() {
    let (mut bench, mut tool, _) = column(0);
    click_copy(&mut bench, &mut tool, Vec3::new(1.2, 0.5, 0.0), 0.15);
    assert!(tool.picked_copy().is_some());
    // The end of the path is a handle: a press on it picks it and drops the copy.
    let end = bench.px(Vec3::new(1.2, 1.0, 0.0));
    assert!(bench.press(&mut tool, end, Button::Left));
    bench.release(&mut tool);
    assert_eq!((tool.picked(), tool.picked_copy()), (Some(1), None));
    // A copy again drops the point.
    click_copy(&mut bench, &mut tool, Vec3::new(1.2, 0.5, 0.0), 0.15);
    assert_eq!((tool.picked(), picked(&tool)), (None, Some((3, 0))));

    assert!(bench.key(&mut tool, Key::Escape), "the first Esc lets go of the copy");
    assert_eq!(tool.picked_copy(), None);
    assert!(!bench.key(&mut tool, Key::Escape), "the next is the app's: it puts the tool away");
}

#[test]
fn the_gizmo_is_only_there_while_the_tool_is_up_and_not_in_the_clean_view() {
    let (mut bench, mut tool, _) = column(0);
    click_copy(&mut bench, &mut tool, Vec3::new(1.2, 0.0, 0.0), 0.15);
    bench.frame(&mut tool);
    assert!(tool.gizmo().is_some());
    bench.env.clean_view = true;
    bench.frame(&mut tool);
    assert!(tool.gizmo().is_none());
    bench.env.clean_view = false;
    bench.frame(&mut tool);
    assert!(tool.gizmo().is_some());
    bench.leave(&mut tool);
    bench.env.active = false;
    bench.frame(&mut tool);
    assert!(tool.gizmo().is_none() && tool.picked_copy().is_none());
}

#[test]
fn a_picked_copy_is_let_go_when_it_is_no_longer_there() {
    let (mut bench, mut tool, object) = column(1);
    click_copy(&mut bench, &mut tool, Vec3::new(1.2, 0.5, 0.0), 0.15);
    assert_eq!(picked(&tool), Some((3, 0)));
    bench.call(|cx| tool.edit(cx, "Array Count", false, |a| a.settings.count = 3));
    bench.frame(&mut tool);
    assert_eq!(tool.picked_copy(), None, "the array has no fourth copy now");

    // And a copy seen through a side of the mirror that is switched off.
    bench.call(|cx| tool.edit(cx, "Array Count", false, |a| a.settings.count = 5));
    click_copy(&mut bench, &mut tool, Vec3::new(-1.2, 0.0, 0.0), 0.0);
    assert_eq!(picked(&tool), Some((2, 1)));
    bench.mirror(object, 0);
    bench.frame(&mut tool);
    assert_eq!(tool.picked_copy(), None);
}

#[test]
fn clicking_another_object_still_switches_to_it_and_a_nearer_object_hides_a_copy_behind_it() {
    let (mut bench, mut tool, object) = column(0);
    let other = bench.add("Other", cube(0.2));
    bench.call(|cx| cx.commit("Place", vec![Command::Place { object: other, placement: Placement::at(Vec3::new(1.2, 0.0, 1.0)) }]));
    bench.call(|cx| cx.commit("Select", vec![Command::Select { object }]));
    // Copy 2 is at (1.2, 0, 0) with the other cube in front of it: the click is on the cube.
    let on = bench.px(Vec3::new(1.2 + 0.15, 0.0, 1.2));
    bench.click(&mut tool, on, Button::Left);
    assert_eq!(bench.doc.active(), other);
    assert_eq!(tool.picked_copy(), None);
}

// ---- posing the copies with the gizmo ---------------------------------------------------------

/// The copy picked and the world around it before a drag.
struct Before {
    layout: Vec<crate::ArrayPlacement>,
    object: Placement,
    state: ArrayState,
    positions: Vec<Vec3>,
}

fn before(tool: &ArrayTool, bench: &Bench, object: ObjectId) -> Before {
    Before {
        layout: tool.arrays.get(object).unwrap().layout.clone(),
        object: placement(bench, object),
        state: tool.state(object),
        positions: copy_positions(tool, bench, object),
    }
}

#[test]
fn moving_a_copy_with_the_gizmo_moves_the_object_and_every_copy_the_same_way_and_leaves_the_path() {
    for ring in [false, true] {
        let (mut bench, mut tool, object) = if ring { self::ring() } else { column(0) };
        let k = 2;
        let was = before(&tool, &bench, object);
        let scene_steps = bench.doc.undo_steps();
        tool.copy = Some(CopyPick { object, copy: k, side: 0 });
        bench.frame(&mut tool);
        let target = tool.gizmo().expect("the gizmo is on the copy").placement;
        near(target.position, was.positions[k], "the gizmo starts at the copy");

        // The copy is dragged by (0.3, 0.1, 0.2), as the arrows would.
        let by = Vec3::new(0.3, 0.1, 0.2);
        assert!(bench.gizmo(&mut tool, Placement { position: target.position + by, ..target }));

        let now = placement(&bench, object);
        let local = now.position - was.object.position;
        let frame = was.layout[k];
        near(frame.rotation * (local * frame.scale), by, "the object went by the move, taken back to its own place on the path");
        assert_eq!(now.rotation, was.object.rotation, "a move does not turn anything");

        // The picked copy is where the gizmo put it; every copy went by the same, each in its own frame.
        let after = copy_positions(&tool, &bench, object);
        near(after[k], target.position + by, "the copy followed the gizmo");
        for (i, (&a, &b)) in after.iter().zip(&was.positions).enumerate() {
            near(a - b, was.layout[i].rotation * (local * was.layout[i].scale), &format!("copy {i}"));
        }
        // The path stayed where it was in the world: the object went by `local` and every point by `-local`.
        let state = tool.state(object);
        for i in 1..state.points.len() {
            near(now.position + state.points[i], was.object.position + was.state.points[i], &format!("point {i}"));
        }
        assert!(state.has_copy_offset());

        // One step of the scene's history, with the tool's own noting it.
        assert_eq!(bench.doc.undo_steps(), scene_steps + 1);
        assert!(tool.history.undo.last().unwrap().scene.is_some());
        assert_eq!(tool.picked_copy().map(|p| p.copy), Some(k), "the copy stays picked");
        assert!(bench.status.as_ref().unwrap().0.starts_with("Moved every copy the same way"));

        // Z takes the object back through the scene, and the tool puts the points back to match.
        assert!(bench.undo(&mut tool));
        assert_eq!(placement(&bench, object), was.object);
        bench.frame(&mut tool);
        assert_eq!(tool.state(object), was.state, "exactly as it was");
        assert!(bench.redo(&mut tool));
        bench.frame(&mut tool);
        assert_eq!(placement(&bench, object), now);
        assert_eq!(tool.state(object), state, "and exactly as the pose left it");
    }
}

#[test]
fn the_object_is_drawn_where_the_drag_has_it_until_the_drag_ends() {
    let (mut bench, mut tool, object) = column(1);
    tool.copy = Some(CopyPick { object, copy: 3, side: 0 });
    bench.frame(&mut tool);
    let target = tool.gizmo().unwrap().placement;
    let was = placement(&bench, object);
    let steps = bench.doc.undo_steps();

    bench.send(&mut tool, sculpt_toolkit::Event::Gizmo(target, sculpt_toolkit::GizmoPhase::Begin));
    let moved = Placement { position: target.position + Vec3::new(0.0, 0.4, 0.0), ..target };
    bench.send(&mut tool, sculpt_toolkit::Event::Gizmo(moved, sculpt_toolkit::GizmoPhase::Move));
    assert_eq!(placement(&bench, object), was, "nothing is committed while the pointer moves");
    assert_eq!(bench.doc.undo_steps(), steps);
    let draw = bench.frame(&mut tool);
    assert_eq!(draw.hidden(), [object]);
    let all = copies(&draw, object);
    assert_eq!(all.len(), 10, "the object as the first copy, on both sides");
    near(at(&all[0]), was.position + Vec3::new(0.0, 0.4, 0.0), "the object is drawn where the drag took it");
    near(at(&all[3]), moved.position, "and the picked copy is where the gizmo is");
    near(at(&all[5 + 3]), Vec3::new(-1.2, 0.5 + 0.4, 0.0), "its reflection with it");

    bench.send(&mut tool, sculpt_toolkit::Event::Gizmo(moved, sculpt_toolkit::GizmoPhase::End));
    assert_eq!(bench.doc.undo_steps(), steps + 1);
    assert!(bench.frame(&mut tool).hidden().is_empty());
}

#[test]
fn a_press_on_a_handle_that_never_moves_is_no_edit() {
    let (mut bench, mut tool, object) = column(0);
    tool.copy = Some(CopyPick { object, copy: 1, side: 0 });
    bench.frame(&mut tool);
    let target = tool.gizmo().unwrap().placement;
    let steps = (bench.doc.undo_steps(), tool.history.undo.len());
    bench.send(&mut tool, sculpt_toolkit::Event::Gizmo(target, sculpt_toolkit::GizmoPhase::Begin));
    bench.send(&mut tool, sculpt_toolkit::Event::Gizmo(target, sculpt_toolkit::GizmoPhase::End));
    assert_eq!((bench.doc.undo_steps(), tool.history.undo.len()), steps);
    assert!(!tool.state(object).has_copy_offset());
}

#[test]
fn turning_a_copy_with_the_gizmo_turns_the_object_and_every_copy_in_its_own_frame() {
    let (mut bench, mut tool, object) = ring();
    let k = 3;
    let was = before(&tool, &bench, object);
    let scene_steps = bench.doc.undo_steps();
    tool.copy = Some(CopyPick { object, copy: k, side: 0 });
    bench.frame(&mut tool);
    let target = tool.gizmo().unwrap().placement;

    let turn = Quat::from_rotation_y(0.6) * Quat::from_rotation_x(-0.3);
    let to = Placement { rotation: turn * target.rotation, ..target };
    assert!(bench.gizmo(&mut tool, to));

    let now = placement(&bench, object);
    assert_eq!(now.position, was.object.position, "a turn does not move the object");
    let faces = |i: usize| was.layout[i].rotation * now.rotation;
    assert!(apart(faces(k), to.rotation) < 1e-4, "the picked copy faces where the gizmo was turned");
    for i in 0..was.layout.len() {
        // Every copy turns about its own middle, by the turn the picked one was given as seen in its own
        // frame on the path: the same turn carried across by how its frame differs from the picked copy's.
        let before_facing = was.layout[i].rotation * was.object.rotation;
        let turned = faces(i) * before_facing.inverse();
        let across = was.layout[i].rotation * was.layout[k].rotation.inverse();
        let expected = across * turn * across.inverse();
        assert!(apart(turned, expected) < 1e-4, "copy {i} turned by the turn carried into its frame");
    }
    let facing_before = was.layout[k].rotation * was.object.rotation;
    assert!(apart(faces(k) * facing_before.inverse(), turn) < 1e-4, "the picked copy turned by exactly what the gizmo did");
    near(copy_positions(&tool, &bench, object)[k], was.positions[k], "in place");
    assert_eq!(tool.state(object), was.state, "the array itself is untouched by a turn");

    // A scene step, with the tool's own noting it for the order of Z.
    assert_eq!(bench.doc.undo_steps(), scene_steps + 1);
    assert!(bench.status.as_ref().unwrap().0.starts_with("Turned every copy"));
    assert!(bench.undo(&mut tool), "Z goes to the scene, not to the tool's older step (the ring)");
    assert_eq!(placement(&bench, object), was.object);
    assert!(tool.has_array(object));
}

#[test]
fn dragging_a_mirrored_copy_right_moves_the_original_left() {
    let (mut bench, mut tool, object) = column(1);
    let k = 2;
    tool.copy = Some(CopyPick { object, copy: k, side: 1 });
    bench.frame(&mut tool);
    let target = tool.gizmo().unwrap().placement;
    near(target.position, Vec3::new(-1.2, 0.0, 0.0), "the gizmo is on the copy at the left");
    let was = placement(&bench, object).position;

    // The copy is dragged 0.3 to the right, and 0.2 up.
    assert!(bench.gizmo(&mut tool, Placement { position: target.position + Vec3::new(0.3, 0.2, 0.0), ..target }));
    let now = placement(&bench, object).position;
    near(now - was, Vec3::new(-0.3, 0.2, 0.0), "the original went left, and up");
    let all = copies(&bench.frame(&mut tool), object);
    // Copy 2 on the mirrored side: the second copy of the second side.
    near(at(&all[4 + k - 1]), Vec3::new(-0.9, 0.2, 0.0), "the copy is where it was dragged to");
    assert_eq!(tool.picked_copy().map(|p| (p.copy, p.side)), Some((k, 1)));
    near(tool.gizmo().unwrap().placement.position, Vec3::new(-0.9, 0.2, 0.0), "and the gizmo went with it");
}

#[test]
fn turning_a_mirrored_copy_turns_the_original_the_other_way() {
    let (mut bench, mut tool, object) = column(1);
    tool.copy = Some(CopyPick { object, copy: 1, side: 1 });
    bench.frame(&mut tool);
    let target = tool.gizmo().unwrap().placement;
    let was = placement(&bench, object).rotation;
    // The copy is turned a quarter about the screen's axis (Z).
    let to = Placement { rotation: Quat::from_rotation_z(0.5) * target.rotation, ..target };
    assert!(bench.gizmo(&mut tool, to));
    let now = placement(&bench, object).rotation;
    // A reflection across X reverses a turn about Z.
    assert!(apart(now * was.inverse(), Quat::from_rotation_z(-0.5)) < 1e-4, "{now:?}");
    bench.frame(&mut tool);
    assert!(apart(tool.gizmo().unwrap().placement.rotation, to.rotation) < 1e-4, "the gizmo shows the copy as turned");
}

// ---- Copies Back onto the Path -----------------------------------------------------------------

#[test]
fn copies_back_onto_the_path_undoes_a_move_and_leaves_a_turn() {
    let (mut bench, mut tool, object) = ring();
    let was = before(&tool, &bench, object);
    tool.copy = Some(CopyPick { object, copy: 4, side: 0 });
    bench.frame(&mut tool);
    let target = tool.gizmo().unwrap().placement;
    let turned = Quat::from_rotation_z(0.4) * target.rotation;
    assert!(bench.gizmo(&mut tool, Placement { rotation: turned, ..target }));
    bench.frame(&mut tool);
    let target = tool.gizmo().unwrap().placement;
    assert!(bench.gizmo(&mut tool, Placement { position: target.position + Vec3::new(0.2, -0.1, 0.3), ..target }));
    assert!(tool.has_array(object) && tool.state(object).has_copy_offset());
    let posed = placement(&bench, object);

    let steps = bench.doc.undo_steps();
    bench.call(|cx| tool.copies_onto_path(cx));
    assert_eq!(bench.doc.undo_steps(), steps + 1, "one step");
    assert!(!tool.state(object).has_copy_offset());
    let now = placement(&bench, object);
    near(now.position, was.object.position, "the object is back at the path's start");
    assert_eq!(now.rotation, posed.rotation, "the turn is not undone");
    for (i, (a, b)) in tool.state(object).points.iter().zip(&was.state.points).enumerate() {
        near(*a, *b, &format!("point {i} is where it was"));
    }
    for (i, (a, b)) in copy_positions(&tool, &bench, object).iter().zip(&was.positions).enumerate() {
        near(*a, *b, &format!("copy {i} is back on the path"));
    }
    assert_eq!(bench.status.as_ref().map(|s| s.0.as_str()), Some("Copies are back on the path."));

    // Z puts the move back, and the next puts the object where it was before the move.
    assert!(bench.undo(&mut tool));
    bench.frame(&mut tool);
    assert_eq!(placement(&bench, object), posed);
    assert!(tool.state(object).has_copy_offset());
    assert!(bench.redo(&mut tool));
    bench.frame(&mut tool);
    assert!(!tool.state(object).has_copy_offset());
}

#[test]
fn the_copies_back_button_is_there_to_press_only_when_the_copies_are_off_the_path() {
    let (mut bench, mut tool, object) = column(0);
    let mut panel = Panel::new();
    let steps = (bench.doc.undo_steps(), tool.history.undo.len());
    panel.click(&mut bench, &mut tool, "Copies Back onto the Path");
    assert_eq!((bench.doc.undo_steps(), tool.history.undo.len()), steps, "nothing to put back");

    tool.copy = Some(CopyPick { object, copy: 2, side: 0 });
    bench.frame(&mut tool);
    let target = tool.gizmo().unwrap().placement;
    assert!(bench.gizmo(&mut tool, Placement { position: target.position + Vec3::new(0.0, 0.0, 0.5), ..target }));
    assert!(tool.state(object).has_copy_offset());
    panel.click(&mut bench, &mut tool, "Copies Back onto the Path");
    assert!(!tool.state(object).has_copy_offset());
    assert_eq!(tool.history.undo.len(), steps.1 + 2, "the pose, and putting it back");
}

#[test]
fn a_move_off_the_path_made_by_the_gizmo_shows_in_what_a_new_path_keeps() {
    // A path drawn over an array whose copies are off the path keeps them off (`make::PathStart`).
    let (mut bench, mut tool, object) = column(0);
    tool.copy = Some(CopyPick { object, copy: 1, side: 0 });
    bench.frame(&mut tool);
    let target = tool.gizmo().unwrap().placement;
    assert!(bench.gizmo(&mut tool, Placement { position: target.position + Vec3::new(0.0, 0.0, 0.5), ..target }));
    let offset = tool.state(object).points[0];
    assert!(offset != Vec3::ZERO);
    bench.call(|cx| tool.make_line(cx));
    assert_eq!(tool.state(object).points[0], offset, "making a new line keeps the copies where they were posed");
}

// ---- undo, in order and out of order -----------------------------------------------------------

#[test]
fn z_takes_a_pose_back_before_the_edits_made_earlier_and_after_the_ones_made_later() {
    let (mut bench, mut tool, object) = column(0);
    // A count (the tool's own), then a pose (the scene's).
    bench.call(|cx| tool.edit(cx, "Array Count", false, |a| a.settings.count = 6));
    tool.copy = Some(CopyPick { object, copy: 2, side: 0 });
    bench.frame(&mut tool);
    let target = tool.gizmo().unwrap().placement;
    let was = placement(&bench, object);
    assert!(bench.gizmo(&mut tool, Placement { position: target.position + Vec3::new(0.2, 0.0, 0.0), ..target }));

    assert!(bench.undo(&mut tool), "the pose is the newest edit");
    assert_eq!(placement(&bench, object), was);
    assert_eq!(tool.state(object).settings.count, 6, "the count is still what it was made");
    bench.frame(&mut tool);
    assert!(!tool.state(object).has_copy_offset());
    assert!(bench.undo(&mut tool));
    assert_eq!(tool.state(object).settings.count, 5, "then the count");
    bench.frame(&mut tool);

    // And a pose, then a count: the count first.
    assert!(bench.redo(&mut tool) && bench.redo(&mut tool));
    bench.frame(&mut tool);
    assert!(tool.state(object).has_copy_offset());
    bench.call(|cx| tool.edit(cx, "Array Follow", false, |a| a.settings.follow = false));
    assert!(bench.undo(&mut tool));
    assert!(tool.state(object).settings.follow, "the later edit is the first undone");
    assert!(tool.state(object).has_copy_offset(), "and the pose is still done");
    bench.frame(&mut tool);
    assert!(bench.undo(&mut tool));
    assert_eq!(placement(&bench, object), was);
}

#[test]
fn the_scenes_own_undo_takes_a_pose_back_when_the_tool_is_away_and_the_array_follows() {
    let (mut bench, mut tool, object) = column(0);
    tool.copy = Some(CopyPick { object, copy: 2, side: 0 });
    bench.frame(&mut tool);
    let target = tool.gizmo().unwrap().placement;
    let was = (placement(&bench, object), tool.state(object));
    assert!(bench.gizmo(&mut tool, Placement { position: target.position + Vec3::new(0.2, 0.3, 0.0), ..target }));
    // The user goes back to sculpting and edits the array's count from the card.
    bench.leave(&mut tool);
    bench.env.active = false;
    bench.call(|cx| tool.edit(cx, "Array Count", false, |a| a.settings.count = 7));

    assert!(bench.undo_scene(), "Z with the tool away is the scene's alone");
    bench.frame(&mut tool);
    assert_eq!(placement(&bench, object), was.0);
    let state = tool.state(object);
    assert_eq!(state.settings.count, 7, "an edit made since is kept");
    assert!(!state.has_copy_offset(), "and the path is where it was relative to the object");
    for (a, b) in state.points.iter().zip(&was.1.points) {
        near(*a, *b, "the points are back");
    }
    assert!(bench.redo_scene());
    bench.frame(&mut tool);
    assert!(tool.state(object).has_copy_offset());
    assert_eq!(tool.state(object).settings.count, 7);
}

#[test]
fn putting_the_tool_away_in_the_middle_of_a_drag_ends_it_where_it_is() {
    let (mut bench, mut tool, object) = column(0);
    tool.copy = Some(CopyPick { object, copy: 2, side: 0 });
    bench.frame(&mut tool);
    let target = tool.gizmo().unwrap().placement;
    let steps = bench.doc.undo_steps();
    bench.send(&mut tool, sculpt_toolkit::Event::Gizmo(target, sculpt_toolkit::GizmoPhase::Begin));
    let moved = Placement { position: target.position + Vec3::new(0.0, 0.0, 0.4), ..target };
    bench.send(&mut tool, sculpt_toolkit::Event::Gizmo(moved, sculpt_toolkit::GizmoPhase::Move));
    bench.leave(&mut tool);
    assert!(!tool.is_posing());
    assert_eq!(bench.doc.undo_steps(), steps + 1, "what was dragged is kept");
    // The End the host sends after that finds nothing to end.
    bench.send(&mut tool, sculpt_toolkit::Event::Gizmo(moved, sculpt_toolkit::GizmoPhase::End));
    assert_eq!(bench.doc.undo_steps(), steps + 1);
}

#[test]
fn removing_the_array_or_applying_it_forgets_its_pose_steps() {
    let (mut bench, mut tool, object) = column(0);
    tool.copy = Some(CopyPick { object, copy: 2, side: 0 });
    bench.frame(&mut tool);
    let target = tool.gizmo().unwrap().placement;
    assert!(bench.gizmo(&mut tool, Placement { position: target.position + Vec3::new(0.2, 0.0, 0.0), ..target }));
    assert!(tool.history.undo.iter().any(|s| s.scene.is_some()));
    bench.call(|cx| tool.apply(cx, Apply::One));
    assert!(tool.history.undo.iter().all(|s| s.object != object), "no step of an array that is real now");
}

// ---- the demos ----------------------------------------------------------------------------------

/// A cube at the origin with the tool up, as `--demo-tool array:what` finds the app.
fn plain() -> (Bench, ArrayTool) {
    let mut bench = Bench::new();
    bench.add("Cube", cube(0.5));
    let mut tool = ArrayTool::default();
    bench.enter(&mut tool);
    (bench, tool)
}

#[test]
fn the_mirror_and_pose_demos_set_up_what_they_say() {
    let (mut bench, mut tool) = plain();
    bench.call(|cx| tool.demo(cx, "mirror"));
    let object = bench.doc.active();
    assert_eq!(bench.doc.info(object).unwrap().mirror.map(|m| m.axis_mask), Some(1));
    let all = copies(&bench.frame(&mut tool), object);
    assert_eq!(all.len(), (tool.state(object).settings.count as usize - 1) * 2, "every copy and its reflection");
    assert!(tool.picked_copy().is_none());
    assert!(bench.framed.is_some(), "the camera is asked to take in both sides");

    let (mut bench, mut tool) = plain();
    bench.call(|cx| tool.demo(cx, "copy"));
    assert_eq!(tool.picked_copy().map(|p| p.side), Some(1), "a copy on the far side of the mirror");
    bench.frame(&mut tool);
    assert!(tool.gizmo().is_some());

    let (mut bench, mut tool) = plain();
    bench.call(|cx| tool.demo(cx, "posed"));
    let object = bench.doc.active();
    assert!(tool.state(object).has_copy_offset(), "a copy was moved off the path");
    assert!(bench.doc.undo_steps() >= 3, "the placing, a move and a turn");
    assert!(tool.picked_copy().is_some() && !tool.is_posing());
}
