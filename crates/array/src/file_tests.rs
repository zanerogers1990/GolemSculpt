//! Arrays through a scene file, driven the way the app drives the tool: made with
//! the tool's own gestures, written by `Tool::save_object`, read back by
//! `Tool::load_objects` into a fresh tool on new objects, as Open does.

use glam::{Mat4, Vec3};
use sculpt_toolkit::testing::{Bench, cube};
use sculpt_toolkit::{Arrival, Button, Draw, ObjectId, Placement, Saved, Scene, Tool};

use crate::{ArrayKind, ArrayState, ArrayTool};

fn copies(draw: &Draw, object: ObjectId) -> Vec<Mat4> {
    draw.object_copies().iter().filter(|(id, _)| *id == object).flat_map(|(_, m)| m.clone()).collect()
}

/// Draws a wavy curve for the active object with the pen.
fn draw_curve(bench: &mut Bench, tool: &mut ArrayTool) {
    bench.call(|cx| tool.set_armed(cx, true));
    let start = bench.px(Vec3::new(-2.0, 0.0, 0.0));
    assert!(bench.press(tool, start, Button::Left));
    for i in 1..=40 {
        let t = i as f32 / 40.0;
        let p = bench.px(Vec3::new(-2.0 + 4.0 * t, (t * 6.0).sin(), 0.0));
        bench.drag_to(tool, p);
    }
    bench.release(tool);
}

/// Four cubes: a line on the third, a circle on the second, a drawn curve on the fourth, which
/// is mirrored too, and nothing on the first.
fn scene() -> (Bench, ArrayTool, [ObjectId; 4]) {
    let mut bench = Bench::new();
    let ids = [
        bench.add("One", cube(0.5)),
        bench.add("Two", cube(0.5)),
        bench.add("Three", cube(0.5)),
        bench.add("Four", cube(0.5)),
    ];
    let mut tool = ArrayTool::default();
    bench.enter(&mut tool);
    bench.select(&[ids[2]]);
    bench.call(|cx| tool.make_line(cx));
    bench.select(&[ids[1]]);
    bench.call(|cx| tool.make_circle(cx));
    bench.select(&[ids[3]]);
    bench.mirror(ids[3], 1);
    draw_curve(&mut bench, &mut tool);
    bench.frame(&mut tool);
    (bench, tool, ids)
}

fn kinds(tool: &ArrayTool, ids: &[ObjectId]) -> Vec<ArrayKind> {
    ids.iter().map(|&id| tool.state(id).settings.kind).collect()
}

#[test]
fn a_line_a_circle_a_drawn_curve_and_a_mirrored_array_come_back_from_a_file_on_their_own_objects() {
    let (mut bench, tool, old) = scene();
    assert_eq!(kinds(&tool, &old), [ArrayKind::None, ArrayKind::Radial, ArrayKind::Line, ArrayKind::Curve]);
    let saved = bench.save_tool(&tool);
    assert_eq!(saved.iter().map(Option::is_some).collect::<Vec<_>>(), [false, true, true, true], "by row");

    // A file is a fresh start: new tool, new objects (the mirror is the object's own and the app's to keep).
    let new = bench.open(vec![("One", cube(0.5)), ("Two", cube(0.5)), ("Three", cube(0.5)), ("Four", cube(0.5))]);
    bench.mirror(new[3], 1);
    let mut opened = ArrayTool::default();
    bench.load_tool(&mut opened, &new, saved.clone(), Arrival::Replaced);
    for (was, now) in old.iter().zip(&new) {
        assert_eq!(opened.state(*now), tool.state(*was), "the array of {was:?}");
    }
    assert_eq!(kinds(&opened, &new), kinds(&tool, &old));
    assert_eq!(bench.save_tool(&opened), saved, "saving what was opened writes the same file");

    // Live: the copies are drawn, as many as before, where they were, and the mirror's side of each too.
    let draw = bench.frame(&mut opened);
    for (was, now) in old.iter().zip(&new).filter(|(was, _)| !tool.state(**was).is_none()) {
        let per_side = tool.state(*was).settings.count as usize - 1;
        let sides = if *now == new[3] { 2 } else { 1 };
        assert_eq!(copies(&draw, *now).len(), per_side * sides, "copies of {now:?}");
    }
    let straight = copies(&draw, new[2]);
    assert!(straight[3].w_axis.truncate().distance(Vec3::new(4.4, 0.0, 0.0)) < 1e-3, "the line ends where it did");
}

#[test]
fn the_pose_of_the_copies_is_the_objects_own_place_and_the_first_point() {
    let (mut bench, mut tool, ids) = scene();
    // Posing a copy carries the object off the path: the path's first point is no longer the object's.
    let moved = Placement { position: Vec3::new(1.0, 2.0, 3.0), ..Placement::IDENTITY };
    bench.call(|cx| cx.commit("Place", vec![sculpt_toolkit::Command::Place { object: ids[2], placement: moved }]));
    bench.select(&[ids[2]]);
    bench.call(|cx| tool.edit(cx, "Pose", false, |state| state.points[0] = Vec3::new(0.0, 0.25, 0.0)));
    assert!(tool.state(ids[2]).has_copy_offset());
    let before = copies(&bench.frame(&mut tool), ids[2]);

    let saved = bench.save_tool(&tool);
    let new = bench.open(vec![("Three", cube(0.5))]);
    bench.call(|cx| cx.commit("Place", vec![sculpt_toolkit::Command::Place { object: new[0], placement: moved }]));
    let mut opened = ArrayTool::default();
    bench.load_tool(&mut opened, &new, vec![saved[2].clone()], Arrival::Replaced);
    assert!(opened.state(new[0]).has_copy_offset());
    let after = copies(&bench.frame(&mut opened), new[0]);
    assert_eq!(after.len(), before.len());
    for (a, b) in after.iter().zip(&before) {
        assert!(a.abs_diff_eq(*b, 1e-5), "a copy moved: {a:?} and {b:?}");
    }
}

#[test]
fn opening_a_file_ends_the_tools_history_but_keeps_the_arrays_for_undo_to_bring_back() {
    let (mut bench, mut tool, old) = scene();
    assert!(tool.can_undo(false), "there are array edits to undo");
    let before: Vec<ArrayState> = old.iter().map(|&id| tool.state(id)).collect();

    let new = bench.open(vec![("Only", cube(0.5))]);
    let saved = vec![Some(crate::state_to_saved(&before[2]))];
    bench.load_tool(&mut tool, &new, saved, Arrival::Replaced);
    assert!(
        !tool.can_undo(false) && !tool.can_undo(true),
        "no step of the tool is left to answer Z before the scene's"
    );
    assert_eq!(tool.state(new[0]), before[2]);

    // The first Z undoes the Open itself, not an array edit of an object that is not there.
    assert!(bench.undo(&mut tool));
    assert_eq!(bench.doc.objects(), old);
    let back: Vec<ArrayState> = old.iter().map(|&id| tool.state(id)).collect();
    assert_eq!(back, before, "the old arrays are as they were");
    let draw = bench.frame(&mut tool);
    assert_eq!(copies(&draw, old[2]).len(), 4, "and drawn again");
    assert!(copies(&draw, new[0]).is_empty(), "the opened object is gone, and its array draws nothing");

    // Redo brings the file's objects, with their arrays.
    assert!(bench.redo(&mut tool));
    assert_eq!(bench.doc.objects(), new);
    assert_eq!(copies(&bench.frame(&mut tool), new[0]).len(), 4);
}

#[test]
fn adding_a_file_leaves_the_scenes_own_arrays_and_history_alone() {
    let (mut bench, mut tool, old) = scene();
    let before: Vec<ArrayState> = old.iter().map(|&id| tool.state(id)).collect();
    let saved = vec![None, Some(crate::state_to_saved(&before[2]))];
    let steps = tool.can_undo(false);

    let added = bench
        .call(|cx| {
            cx.commit(
                "Add",
                vec![
                    sculpt_toolkit::Command::Add(sculpt_toolkit::NewObject::new("A", cube(0.5)).unselected()),
                    sculpt_toolkit::Command::Add(sculpt_toolkit::NewObject::new("B", cube(0.5)).unselected()),
                ],
            )
        })
        .added;
    bench.load_tool(&mut tool, &added, saved, Arrival::Added);
    assert_eq!(old.iter().map(|&id| tool.state(id)).collect::<Vec<_>>(), before, "the scene's arrays are as they were");
    assert!(tool.state(added[0]).is_none());
    assert_eq!(tool.state(added[1]), before[2], "the added one has its own");
    assert_eq!(tool.can_undo(false), steps, "the tool's history is untouched");
    assert!(bench.save_tool(&tool).iter().filter(|s| s.is_some()).count() == 4);
}

#[test]
fn an_array_on_an_instance_is_laid_out_from_the_instance() {
    let mut bench = Bench::new();
    let base = bench.add("Base", cube(0.5));
    let twin = bench.instance(base, "Base Instance", Placement::at(Vec3::new(0.0, 3.0, 0.0)));
    let mut tool = ArrayTool::default();
    bench.enter(&mut tool);
    bench.select(&[base]);
    bench.call(|cx| tool.make_line(cx));
    let saved = bench.save_tool(&tool);
    assert!(saved[0].is_some() && saved[1].is_none());

    // The file puts the base's array on the instance too (this app allows it; the Unity app did not).
    let mut opened = ArrayTool::default();
    let on_both = vec![(base, saved[0].clone().unwrap()), (twin, saved[0].clone().unwrap())];
    opened.load_objects(on_both, Arrival::Added);
    let draw = bench.frame(&mut opened);
    let (own, other) = (copies(&draw, base), copies(&draw, twin));
    assert_eq!((own.len(), other.len()), (4, 4));
    assert!(other[0].w_axis.truncate().distance(own[0].w_axis.truncate() + Vec3::Y * 3.0) < 1e-4, "from the instance");
}

#[test]
fn damaged_input_loads_what_it_can_and_never_fails() {
    let mut bench = Bench::new();
    let ids: Vec<ObjectId> = (0..8).map(|i| bench.add(&format!("O{i}"), cube(0.5))).collect();
    let point = |x: f32| Saved::numbers(&[x, 0.0, 0.0]);
    let line = |count: Saved, second: Saved| {
        Saved::fields([("kind", 1.into()), ("count", count), ("points", Saved::List(vec![point(0.0), second]))])
    };
    let objects = vec![
        (ids[0], line(0.into(), point(4.0))),
        (ids[1], line(1.into(), point(4.0))),
        (ids[2], line(1.0e9.into(), point(4.0))),
        (ids[3], line(5.into(), Saved::numbers(&[f32::NAN, 0.0, 0.0]))),
        (ids[4], Saved::Null),
        (ids[5], Saved::from("array")),
        (ids[6], Saved::fields([("kind", 3.into())])),
        (ObjectId(9999), line(5.into(), point(4.0))),
    ];
    let mut tool = ArrayTool::default();
    tool.load_objects(objects, Arrival::Replaced);
    let counts: Vec<i32> = ids.iter().map(|&id| tool.state(id).settings.count).collect();
    assert_eq!(counts[..3], [2, 2, 100], "a count of 0, 1 or a billion is brought into range");
    assert!(ids[3..].iter().all(|&id| tool.state(id).is_none()), "what cannot be read is no array");

    // All of it draws without a fuss, and the object that is not in the scene draws nothing.
    let draw = bench.frame(&mut tool);
    assert_eq!(copies(&draw, ids[0]).len(), 1);
    assert_eq!(copies(&draw, ids[2]).len(), 99);
    assert!(copies(&draw, ObjectId(9999)).is_empty());
    assert!(draw.object_copies().iter().flat_map(|(_, m)| m).all(|m| m.is_finite()));
}

/// The app tells that a scene has changes to save partly by what `save_object` would write: an
/// array reshaped is no step of the scene's history. So every edit has to show there, and nothing
/// but an edit may: frames drawn, another object selected, the tool put away and brought back
/// leave it exactly as it was, or a scene that was just saved would count as changed.
#[test]
fn every_edit_shows_in_what_the_file_would_keep_and_nothing_else_does() {
    let (mut bench, mut tool, ids) = scene();
    // The third cube has the line.
    bench.select(&[ids[2]]);
    bench.frame(&mut tool);
    let mut kept = bench.save_tool(&tool);

    // Nothing that is not an edit moves it.
    for _ in 0..5 {
        bench.frame(&mut tool);
    }
    bench.select(&[ids[0]]);
    bench.frame(&mut tool);
    bench.select(&[ids[2]]);
    bench.leave(&mut tool);
    bench.env.active = false;
    bench.frame(&mut tool);
    bench.env.active = true;
    bench.enter(&mut tool);
    bench.frame(&mut tool);
    assert_eq!(bench.save_tool(&tool), kept, "looking at the array is not an edit");

    // Each of the panel's controls and a handle dragged: one change each to what the file keeps.
    let edits: [(&'static str, fn(&mut ArrayState)); 7] = [
        ("Array Count", |a| a.settings.count += 3),
        ("Array End Size", |a| a.settings.end_scale = 0.45),
        ("Array Middle Size", |a| a.settings.mid_scale = 1.4),
        ("Array Follow", |a| a.settings.follow = !a.settings.follow),
        ("Array Spacing", |a| a.settings.space_by_size = !a.settings.space_by_size),
        ("Move Array Point", |a| a.points[1] += Vec3::new(0.0, 0.5, 0.25)),
        ("Add Array Point", |a| a.points.push(Vec3::new(4.0, 2.0, 0.0))),
    ];
    for (label, change) in edits {
        bench.call(|cx| tool.edit(cx, label, false, change));
        bench.frame(&mut tool);
        let now = bench.save_tool(&tool);
        assert_ne!(now, kept, "{label} does not show in what the file would keep");
        kept = now;
    }
    // Z takes the last edit back, and what the file would keep goes back with it.
    let with_the_point = kept.clone();
    assert!(bench.undo(&mut tool));
    bench.frame(&mut tool);
    assert_ne!(bench.save_tool(&tool), with_the_point);
    assert!(bench.redo(&mut tool));
    bench.frame(&mut tool);
    assert_eq!(bench.save_tool(&tool), with_the_point, "redone, it is exactly what it was");
}
