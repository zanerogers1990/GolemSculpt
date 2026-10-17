//! `--demo-tool array[:what]`: the tool in states worth a screenshot, set up
//! through the same functions the buttons and the pen call.
//!
//! | what | shows |
//! | --- | --- |
//! | (none), `circle` | Eight copies of the selected object round a circle, tapering as in the Unity design picture |
//! | `line` | A row of bricks, shrinking toward the end |
//! | `curve` | Bricks along a hand-drawn S, shrinking toward the end |
//! | `spaced` | The S of bricks after the fourth was dragged on along the curve: every gap opened alike, the last bricks carried on straight past the end |
//! | `packed` | The same dragged back instead: every gap closed alike |
//! | `away` | The circle with the tool put away: the copies stay on screen |
//! | `made` | The circle made real as one object |
//! | `separate` | The circle made real as separate objects |
//! | `mirror` | A column of copies rising from an object off the middle, mirrored across X: every copy has its reflection |
//! | `copy` | The same, with a copy on the far side picked: the gizmo's arrows and rings sit on it |
//! | `posed` | The same after that copy was moved and turned with the gizmo: every copy off the path the same way |
//! | `onpath` | And then Copies Back onto the Path: the move is undone and the turn stays |
//! | `column` | The same column with no mirror: what the region tools and the brushes are shown over (`hide:array`, `array:stroke`) |
//! | `point` | The column with the end of its path picked: the gizmo's arrows sit on it |
//! | `arrows` | The same after the arrows were dragged sideways: the end swings out and every copy follows |
//! | `start` | The arrows on the square start point, dragged to the left: the object and its array move together |
//! | `as-is` | Nothing is made: the tool comes up on the arrays a file brought, the first object with one selected |
//! | `several` | Three objects with an array each: a ring, a curve, a mirrored column; what `--demo-files array` saves |
//!
//! `line`, `curve` hide the selected object and add a brick in its place, so
//! the way copies turn to follow a path can be seen; a sphere looks the same
//! whichever way it faces.

use glam::{Quat, Vec2, Vec3};
use sculpt_geom::Aabb;
use sculpt_toolkit::testing::cube;
use sculpt_toolkit::{Command, Ctx, GizmoPhase, MirrorRepeater, NewObject, ObjectId, Placement, Tool};

use crate::finish::Apply;
use crate::pose::CopyPick;
use crate::tool::ArrayTool;

pub fn demo(tool: &mut ArrayTool, cx: &mut Ctx, what: &str) {
    match what {
        "line" => line(tool, cx),
        "curve" => curve(tool, cx),
        "spaced" => spaced(tool, cx, 0.5),
        "packed" => spaced(tool, cx, -0.25),
        "away" => {
            circle(tool, cx);
            cx.finish();
        }
        "made" => made(tool, cx, Apply::One),
        "separate" => made(tool, cx, Apply::Separate),
        "mirror" => _ = column(tool, cx, true),
        "column" => _ = column(tool, cx, false),
        "point" => point(tool, cx, 1, false),
        "arrows" => point(tool, cx, 1, true),
        "start" => point(tool, cx, 0, true),
        "copy" => {
            let object = column(tool, cx, true);
            tool.picked = None;
            tool.copy = Some(CopyPick { object, copy: 2, side: 1 });
        }
        "as-is" => as_is(tool, cx),
        "several" => several(tool, cx),
        "posed" => posed(tool, cx, false),
        "onpath" => posed(tool, cx, true),
        _ => circle(tool, cx),
    }
}

/// The selected object made smaller and stood to one side of the world's middle,
/// with a column of copies rising from it. With `mirrored` it is mirrored
/// across X: the array is repeated at the other side of the scene's centre.
fn column(tool: &mut ArrayTool, cx: &mut Ctx, mirrored: bool) -> ObjectId {
    let scene = cx.scene();
    let object = scene.active();
    let Some(info) = scene.info(object) else { return object };
    let reach = scene.bounds(object).size().max_element() * 0.5;
    let placement = Placement {
        position: Vec3::new(reach * 1.9, -reach * 1.1, 0.0),
        rotation: info.placement.rotation,
        scale: info.placement.scale * 0.6,
    };
    let mirror = MirrorRepeater::new(1, Vec3::ZERO).filter(|_| mirrored);
    cx.commit("Demo Array Column", vec![Command::Place { object, placement }, Command::Mirror { object, mirror }]);
    tool.make_line(cx);
    // The mirrored column keeps its copies close, a stack; the plain one spaces them out, so
    // that a shape drawn over one of them is over that one alone.
    let (span, count) = if mirrored { (reach * 2.6, 6) } else { (reach * 6.4, 5) };
    tool.edit(cx, "Move Array Point", false, |a| {
        a.points[1] = Vec3::Y * span;
        a.settings.count = count;
        a.settings.end_scale = 0.6;
    });
    if mirrored {
        fit_both_sides(tool, cx);
    } else {
        fit(tool, cx);
    }
    object
}

/// The column with a point of its path picked, so that the gizmo's arrows sit
/// on it (`index` 1 is the end of the path, 0 the square start point), and
/// with `drag` the arrows then dragged sideways, as the gizmo's own events
/// would: the end of the path swings out and every copy follows it.
fn point(tool: &mut ArrayTool, cx: &mut Ctx, index: usize, drag: bool) {
    let object = column(tool, cx, false);
    let reach = cx.scene().bounds(object).size().max_element();
    tool.picked = Some(index);
    tool.refresh_gizmo(cx);
    if drag {
        let Some(from) = tool.gizmo().map(|g| g.placement) else { return };
        let by = if index == 0 { Vec3::new(-3.0, 0.5, 0.0) } else { Vec3::new(2.2, 0.0, 0.0) } * reach;
        let halfway = Placement::at(from.position + by * 0.5);
        let to = Placement::at(from.position + by);
        tool.gizmo_event(cx, from, GizmoPhase::Begin);
        tool.gizmo_event(cx, halfway, GizmoPhase::Move);
        tool.gizmo_event(cx, to, GizmoPhase::Move);
        tool.gizmo_event(cx, to, GizmoPhase::End);
        tool.refresh_gizmo(cx);
    }
    fit_with_room(tool, cx, 2.5);
}

/// The column with the picked mirror copy moved and turned by the gizmo's own
/// path (the events it sends), and with `back` then put back onto the path.
fn posed(tool: &mut ArrayTool, cx: &mut Ctx, back: bool) {
    let object = column(tool, cx, true);
    let reach = cx.scene().bounds(object).size().max_element();
    tool.picked = None;
    tool.copy = Some(CopyPick { object, copy: 2, side: 1 });
    tool.refresh_gizmo(cx);
    for drag in 0..2 {
        let Some(target) = tool.gizmo().map(|g| g.placement) else { return };
        let to = if drag == 0 {
            // Toward the middle and a little up: on the far side that moves the object the other way.
            Placement { position: target.position + Vec3::new(1.4 * reach, 0.5 * reach, 0.0), ..target }
        } else {
            Placement { rotation: Quat::from_rotation_z(0.6) * target.rotation, ..target }
        };
        tool.gizmo_event(cx, target, GizmoPhase::Begin);
        tool.gizmo_event(cx, to, GizmoPhase::Move);
        tool.gizmo_event(cx, to, GizmoPhase::End);
        tool.refresh_gizmo(cx);
    }
    if back {
        tool.copies_onto_path(cx);
    }
    fit_both_sides(tool, cx);
}

/// The tool up on the arrays the scene has already, with none made: the first object that has one
/// is selected, so that its path and handles show, and its copies are framed.
fn as_is(tool: &mut ArrayTool, cx: &mut Ctx) {
    let Some(object) = cx.scene().objects().into_iter().find(|&object| tool.arrays.has(object)) else { return };
    cx.commit("Demo Select", vec![Command::Select { object }]);
    fit(tool, cx);
}

/// An object put in the scene at `at`, made the selected one.
fn stand(cx: &mut Ctx, name: &str, at: Vec3, scale: Vec3, mirror: Option<MirrorRepeater>) -> ObjectId {
    let placement = Placement { position: at, rotation: Quat::IDENTITY, scale };
    cx.commit("Demo Object", vec![Command::Add(NewObject::new(name, cube(0.5)).placed(placement).mirrored(mirror))])
        .added[0]
}

/// Three objects, each with an array of its own, to be saved and opened: a ring of copies on the
/// sphere the scene starts with, a curve on a brick below it (the second row of the list, and
/// the selected object), and on a post to the side a column that is mirrored across a plane
/// of its own. Every kind of array a file keeps, on objects that are not all first.
fn several(tool: &mut ArrayTool, cx: &mut Ctx) {
    circle(tool, cx);
    let sphere = cx.scene().active();

    let brick = stand(cx, "Brick", Vec3::new(-2.0, -5.5, 0.0), Vec3::new(0.6, 1.0, 0.35), None);
    let start = cx.scene().info(brick).map_or(Vec3::ZERO, |info| info.placement.position);
    let view = cx.view;
    let stroke: Vec<Vec2> = (0..=60)
        .filter_map(|i| {
            let t = i as f32 / 60.0;
            let up = 1.1 * (t * std::f32::consts::TAU).sin() * (1.0 - 0.35 * t);
            view.project(start + view.right * 4.2 * t + view.up * up)
        })
        .collect();
    tool.draw_stroke(cx, &stroke);
    taper(tool, cx, 0.4);

    let across = MirrorRepeater::new(1, Vec3::new(8.0, 0.0, 0.0));
    let foot = Vec3::new(5.5, -4.0, 0.0);
    let post = stand(cx, "Post", foot, Vec3::new(0.5, 0.5, 0.5), across);
    tool.make_line(cx);
    tool.edit(cx, "Move Array Point", false, |a| _ = a.set_point_world(1, Vec3::new(5.5, 1.0, 0.0), foot));
    tool.edit(cx, "Array Count", false, |a| a.settings.count = 5);
    taper(tool, cx, 0.6);

    // The brick, whose path and handles are the ones shown; the camera takes in all three.
    cx.commit("Demo Select", vec![Command::Select { object: brick }]);
    let mut bounds = Aabb::EMPTY;
    for object in [sphere, brick, post] {
        let Some(live) = tool.arrays.get(object) else { continue };
        let Some(info) = cx.scene().info(object) else { continue };
        for placement in &live.layout {
            let at = info.placement.position + placement.offset;
            bounds.grow(at);
            if object == post {
                bounds.grow(Vec3::new(16.0 - at.x, at.y, at.z));
            }
        }
    }
    cx.frame_view(bounds.expanded(2.0));
}

/// Brings the camera round to take in the array and its reflection.
fn fit_both_sides(tool: &ArrayTool, cx: &mut Ctx) {
    let object = tool.target(cx);
    let Some(live) = tool.arrays.get(object) else { return };
    let Some(info) = cx.scene().info(object) else { return };
    let reach = cx.scene().bounds(object).size().max_element();
    let mut bounds = Aabb::EMPTY;
    for placement in &live.layout {
        let at = info.placement.position + placement.offset;
        bounds.grow(at);
        bounds.grow(at * Vec3::new(-1.0, 1.0, 1.0));
    }
    cx.frame_view(bounds.expanded(reach * 1.2));
}

/// A brick standing in for the selected object, to the left of the middle of
/// the view so the path has room to run to the right. The old object is hidden.
fn brick(cx: &mut Ctx) -> ObjectId {
    let before = cx.scene().active();
    let at = -cx.view.right * 1.9 - cx.view.up * 0.4;
    let placement = Placement { position: at, rotation: Quat::IDENTITY, scale: Vec3::new(0.6, 1.0, 0.35) };
    let receipt = cx.commit(
        "Demo Brick",
        vec![
            Command::Show { object: before, visible: false },
            Command::Add(NewObject::new("Brick", cube(0.5)).placed(placement)),
        ],
    );
    receipt.added[0]
}

fn taper(tool: &mut ArrayTool, cx: &mut Ctx, end: f32) {
    tool.edit(cx, "Array End Size", false, |a| a.settings.end_scale = end);
}

fn circle(tool: &mut ArrayTool, cx: &mut Ctx) {
    tool.make_circle(cx);
    taper(tool, cx, 0.45);
    fit(tool, cx);
}

fn line(tool: &mut ArrayTool, cx: &mut Ctx) {
    let object = brick(cx);
    let start = cx.scene().info(object).map_or(Vec3::ZERO, |info| info.placement.position);
    tool.make_line(cx);
    // Stretched across the view and aimed a little up, as dragging the end point does.
    let end = start + cx.view.right * 3.8 + cx.view.up * 0.7;
    tool.edit(cx, "Move Array Point", false, |a| _ = a.set_point_world(1, end, start));
    tool.edit(cx, "Array Count", false, |a| a.settings.count = 7);
    taper(tool, cx, 0.4);
    fit(tool, cx);
}

fn curve(tool: &mut ArrayTool, cx: &mut Ctx) {
    let object = brick(cx);
    let start = cx.scene().info(object).map_or(Vec3::ZERO, |info| info.placement.position);
    let view = cx.view;
    // An S, rising from the brick and settling to the right, as screen
    // positions: drawn through the same code a pen stroke goes through.
    let stroke: Vec<Vec2> = (0..=60)
        .filter_map(|i| {
            let t = i as f32 / 60.0;
            let across = 3.8 * t;
            let up = 1.1 * (t * std::f32::consts::TAU).sin() * (1.0 - 0.35 * t);
            view.project(start + view.right * across + view.up * up)
        })
        .collect();
    tool.draw_stroke(cx, &stroke);
    taper(tool, cx, 0.4);
    fit(tool, cx);
}

/// The S of bricks with the fourth dragged `by` brick lengths along the curve,
/// as its gizmo's arrow would: every copy is spaced the same way.
fn spaced(tool: &mut ArrayTool, cx: &mut Ctx, by: f32) {
    curve(tool, cx);
    let object = cx.scene().active();
    let reach = cx.scene().bounds(object).size().max_element();
    let state = tool.arrays.state(object);
    let Some(at) = crate::layout::spacing(&state.settings, &state.points, 3) else { return };
    tool.picked = None;
    tool.copy = Some(CopyPick { object, copy: 3, side: 0 });
    tool.refresh_gizmo(cx);
    let Some(target) = tool.gizmo().map(|g| g.placement) else { return };
    let to = Placement { position: target.position + at.direction * by * reach * 3.0, ..target };
    tool.gizmo_event(cx, target, GizmoPhase::Begin);
    tool.gizmo_event(cx, to, GizmoPhase::Move);
    tool.gizmo_event(cx, to, GizmoPhase::End);
    tool.refresh_gizmo(cx);
    fit(tool, cx);
}

fn made(tool: &mut ArrayTool, cx: &mut Ctx, apply: Apply) {
    circle(tool, cx);
    tool.apply(cx, apply);
}

/// Brings the camera round to take in the whole array.
fn fit(tool: &ArrayTool, cx: &mut Ctx) {
    fit_with_room(tool, cx, 0.8);
}

/// The same with `room` object sizes to spare all round, for the gizmo's arms
/// to have somewhere to be.
fn fit_with_room(tool: &ArrayTool, cx: &mut Ctx, room: f32) {
    let object = tool.target(cx);
    let Some(live) = tool.arrays.get(object) else { return };
    let Some(info) = cx.scene().info(object) else { return };
    let reach = cx.scene().bounds(object).size().max_element();
    let mut bounds = Aabb::EMPTY;
    for placement in &live.layout {
        bounds.grow(info.placement.position + placement.offset);
    }
    cx.frame_view(bounds.expanded(reach * room));
}

