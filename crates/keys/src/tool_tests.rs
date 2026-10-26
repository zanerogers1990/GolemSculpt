//! The tool driven as the app drives it, on a ball cut in two with its cut
//! face toward the camera.

use std::time::{Duration, Instant};

use glam::{Vec2, Vec3};
use sculpt_geom::TriMesh;
use sculpt_geom::units::from_millimetres as mm;
use sculpt_toolkit::testing::{Bench, cube};
use sculpt_toolkit::{
    Arrival, Button, Command, Document, Event, Key, Look, Loss, MeshId, ObjectId, Placement, Scene, Shape, Tool,
    theme,
};

use crate::cutting::split_by_plane;
use crate::input::ViewSide;
use crate::pair::Showing;
use crate::profile::KeyShape;
use crate::testing::{closed_error, icosphere};
use crate::tool::Keys;

/// A 20 mm ball about the origin cut across z = -0.1, seen from +z: the far,
/// thinner half shows its cut face to the camera. Returns (far, near), with
/// the far half selected.
fn cut_ball(bench: &mut Bench) -> (ObjectId, ObjectId) {
    let cut = split_by_plane(&icosphere(4, 1.0), Vec3::new(0.0, 0.0, -0.1), Vec3::Z).unwrap();
    let far = bench.add("Far", cut.outside.mesh);
    let near = bench.add("Near", cut.inside.mesh);
    bench.select(&[far]);
    (far, near)
}

fn up_on_a_cut_ball() -> (Bench, Keys, ObjectId, ObjectId) {
    let mut bench = Bench::new();
    let (far, near) = cut_ball(&mut bench);
    let mut tool = Keys::default();
    // The pointer parked off the model, so nothing is hovered.
    bench.hover(Vec2::new(5.0, 5.0));
    bench.enter(&mut tool);
    (bench, tool, far, near)
}

fn said(bench: &Bench) -> (&str, bool) {
    bench.status.as_ref().map(|(text, ok)| (text.as_str(), *ok)).expect("the tool said something")
}

/// Where key `k` of the first joint is, in the world and on screen.
fn key_at(bench: &Bench, tool: &Keys, k: usize) -> (Vec3, Vec2) {
    let jp = &tool.pair.as_ref().unwrap().plan.joints[0];
    let world = jp.frame(&jp.keys[k]).origin;
    (world, bench.px(world))
}

fn jp_dimensions(tool: &Keys) -> crate::types::KeyDimensions {
    tool.pair.as_ref().unwrap().plan.joints[0].dimensions
}

fn key_positions(tool: &Keys) -> Vec<Vec2> {
    tool.pair.as_ref().unwrap().plan.joints[0].keys.iter().map(|key| key.position).collect()
}

fn mesh_of(bench: &Bench, id: ObjectId) -> MeshId {
    bench.doc.info(id).unwrap().mesh
}

fn mesh(bench: &Bench, id: ObjectId) -> TriMesh {
    bench.doc.mesh(id).unwrap().to_tri_mesh()
}

/// Frames until the build that Enter started has landed.
fn wait_for_build(bench: &mut Bench, tool: &mut Keys) {
    let start = Instant::now();
    while tool.job.is_some() {
        assert!(start.elapsed() < Duration::from_secs(20), "the build never landed");
        bench.frame(tool);
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn the_selected_piece_finds_the_piece_that_mates_with_it() {
    let (bench, tool, far, near) = up_on_a_cut_ball();
    assert_eq!(tool.objects(), Some((far, near)));
    let (text, ok) = said(&bench);
    assert!(ok, "{text}");
    assert!(text.contains("Far + Near") && text.contains("the piece touching it"), "{text}");
    // Joints were planned as it came up: two round keys, pegs on the thinner
    // half, which is the far one.
    assert_eq!(tool.key_count(), 2);
    assert!(text.contains("2 joints"), "{text}");
    assert!(tool.pair.as_ref().unwrap().plan.joints[0].peg_on_a);
}

#[test]
fn two_selected_objects_are_the_pair_in_the_order_selected() {
    let mut bench = Bench::new();
    let (far, near) = cut_ball(&mut bench);
    // Something else touching nothing, to be passed over.
    let other = bench.add("Block", cube(0.2));
    bench.doc.place(other, Placement::at(Vec3::new(3.0, 0.0, 0.0)));
    bench.select(&[near, far]);
    let mut tool = Keys::default();
    bench.enter(&mut tool);
    assert_eq!(tool.objects(), Some((near, far)));
    assert!(said(&bench).0.contains("Near + Far (the two selected objects)"), "{}", said(&bench).0);
    // The near half is the thicker: sockets go into it, whichever is first.
    assert!(!tool.pair.as_ref().unwrap().plan.joints[0].peg_on_a);

    // Two selected objects that do not touch are refused by name.
    bench.select(&[far, other]);
    bench.enter(&mut tool);
    assert_eq!(tool.objects(), None);
    let (text, ok) = said(&bench);
    assert!(!ok && text.contains("Far and Block do not touch face to face"), "{text}");
}

#[test]
fn a_piece_that_touches_nothing_asks_for_its_mate() {
    let mut bench = Bench::new();
    let (far, _) = cut_ball(&mut bench);
    let block = bench.add("Block", cube(0.2));
    bench.doc.place(block, Placement::at(Vec3::new(1.6, 0.0, 0.0)));
    bench.select(&[block]);
    let mut tool = Keys::default();
    bench.enter(&mut tool);
    assert_eq!(tool.objects(), None);
    let (text, ok) = said(&bench);
    assert!(!ok && text.contains("Block touches no other object"), "{text}");
    // Nothing to draw, nothing to take a press with.
    assert!(bench.frame(&mut tool).is_empty());

    // A click on another piece offers it (the near half is the one in front);
    // these two do not mate.
    let on_ball = bench.px(Vec3::new(0.0, 0.3, 0.9));
    bench.click(&mut tool, on_ball, Button::Left);
    assert!(said(&bench).0.contains("Block and Near do not touch"), "{}", said(&bench).0);

    // Selecting a half and coming up again finds its pair.
    bench.select(&[far]);
    bench.enter(&mut tool);
    assert!(tool.objects().is_some());
}

#[test]
fn planned_joints_show_as_pegs_on_the_half_that_carries_them() {
    let (mut bench, mut tool, far, near) = up_on_a_cut_ball();
    let draw = bench.frame(&mut tool);
    // The socket half is left out so the face can be seen, and the pegs
    // stand on the far half's face, toward the camera.
    assert_eq!(draw.hidden(), [near]);
    assert_eq!(draw.previews().len(), 1, "the pegs that fit; none that do not");
    let pegs = &draw.previews()[0].mesh.data().positions;
    let jp = &tool.pair.as_ref().unwrap().plan.joints[0];
    let top = pegs.iter().map(|p| p.z).fold(f32::MIN, f32::max);
    assert!((top - (-0.1 + jp.height)).abs() < 1e-3, "pegs reach {top}");
    assert!(pegs.iter().all(|p| p.z > -0.1 - 1e-4), "a peg must stand out of its half, not into it");
    // The rim of the face and, for each key, its mouth, its peg outline and a
    // dot at its middle.
    let lines = draw.overlay.shapes.iter().filter(|s| matches!(s, Shape::Line { .. })).count();
    let dots = draw.overlay.shapes.iter().filter(|s| matches!(s, Shape::Dot { .. })).count();
    assert_eq!((lines, dots), (1 + 2 * 2, 2));

    // V goes round the views.
    assert!(bench.key(&mut tool, Key::V));
    let draw = bench.frame(&mut tool);
    assert_eq!((tool.view, draw.hidden()), (ViewSide::Holes, &[far][..]));
    // The sockets show through the near half as shells, sunk into it from
    // the cut face.
    let [sockets] = draw.previews() else { panic!("{} previews", draw.previews().len()) };
    assert_eq!((sockets.style.look, sockets.style.on_top), (Look::Shell, true));
    let depth = jp_dimensions(&tool).hole_depth();
    let floor = sockets.mesh.data().positions.iter().map(|p| p.z).fold(f32::MIN, f32::max);
    assert!((floor - (-0.1 + depth)).abs() < 1e-3, "sockets reach {floor}");
    assert!(sockets.mesh.data().positions.iter().all(|p| p.z > -0.1 - 1e-4), "a socket is sunk into the other half");
    assert!(bench.key(&mut tool, Key::V));
    let draw = bench.frame(&mut tool);
    assert!(draw.hidden().is_empty() && draw.previews().len() == 2, "pegs on one half, sockets in the other");
    assert!(bench.key(&mut tool, Key::V));
    assert_eq!(tool.view, ViewSide::Pegs);

    // With the panels hidden, the model alone.
    bench.env.clean_view = true;
    let draw = bench.frame(&mut tool);
    assert!(draw.previews().is_empty() && draw.overlay.shapes.is_empty());
}

#[test]
fn with_planning_off_the_face_starts_empty_and_a_click_adds_a_joint() {
    let mut bench = Bench::new();
    cut_ball(&mut bench);
    let mut tool = Keys::default();
    tool.auto_plan = false;
    bench.enter(&mut tool);
    assert_eq!(tool.key_count(), 0);

    // A ghost outline shows where a click would put one.
    let spot = bench.px(Vec3::new(0.2, 0.1, -0.1));
    bench.hover(spot);
    let lines = |bench: &mut Bench, tool: &mut Keys| {
        bench.frame(tool).overlay.shapes.iter().filter(|s| matches!(s, Shape::Line { .. })).count()
    };
    assert_eq!(lines(&mut bench, &mut tool), 2, "the rim and the ghost");

    bench.click(&mut tool, spot, Button::Left);
    assert_eq!(tool.key_count(), 1);
    let (world, _) = key_at(&bench, &tool, 0);
    assert!(world.distance(Vec3::new(0.2, 0.1, -0.1)) < 0.01, "the key is where the click was: {world}");
    assert!(said(&bench).0.starts_with("1 joint."), "{}", said(&bench).0);
    assert!(tool.can_undo(false));

    // One pressed onto the rim is kept, red, and said to be left out.
    let rim = bench.px(Vec3::new(0.9, 0.0, -0.1));
    bench.click(&mut tool, rim, Button::Left);
    assert_eq!((tool.key_count(), tool.invalid_count()), (1, 1));
    let (text, ok) = said(&bench);
    assert!(!ok && text.contains("will not be built"), "{text}");
    assert_eq!(bench.frame(&mut tool).previews().len(), 2, "one mesh of pegs that fit, one of pegs that do not");
}

#[test]
fn a_joint_is_moved_by_dragging_and_the_move_is_one_undo_step() {
    let (mut bench, mut tool, ..) = up_on_a_cut_ball();
    assert!(!tool.can_undo(false) && !tool.can_undo(true));
    let before = key_positions(&tool);
    let (world, from) = key_at(&bench, &tool, 0);
    let to = bench.px(world + Vec3::new(0.0, 0.25, 0.0));

    assert!(bench.press(&mut tool, from, Button::Left), "a press on a key takes the drag");
    bench.drag_to(&mut tool, from.lerp(to, 0.5));
    // While it is carried, its outline is the highlighted one.
    let draw = bench.frame(&mut tool);
    let highlighted = |s: &&Shape| matches!(s, Shape::Line { width, .. } if *width > 2.0);
    assert_eq!(draw.overlay.shapes.iter().filter(highlighted).count(), 1);
    bench.drag_to(&mut tool, to);
    bench.release(&mut tool);

    let (moved, _) = key_at(&bench, &tool, 0);
    assert!(moved.distance(world + Vec3::new(0.0, 0.25, 0.0)) < 0.01, "{moved}");
    assert_eq!(key_positions(&tool)[1], before[1], "the other key stayed put");
    assert!(tool.can_undo(false));

    assert!(bench.undo(&mut tool));
    assert_eq!(key_positions(&tool), before);
    assert!(!tool.can_undo(false) && tool.can_undo(true));
    assert!(bench.redo(&mut tool));
    assert!(key_at(&bench, &tool, 0).0.distance(moved) < 1e-5);

    // A click on a key that does not move it leaves no step behind.
    bench.undo(&mut tool);
    let (_, on_key) = key_at(&bench, &tool, 0);
    bench.click(&mut tool, on_key, Button::Left);
    assert!(!tool.can_undo(false));
    assert_eq!(key_positions(&tool), before);

    // Esc in mid-drag puts the key back and leaves no step either.
    bench.press(&mut tool, on_key, Button::Left);
    bench.drag_to(&mut tool, to);
    assert_ne!(key_positions(&tool), before);
    assert!(bench.key(&mut tool, Key::Escape));
    bench.release(&mut tool);
    assert_eq!(key_positions(&tool), before);
    assert!(!tool.can_undo(false));
    // With nothing in hand, Esc is the app's: it puts the tool away.
    assert!(!bench.key(&mut tool, Key::Escape));
}

#[test]
fn a_joint_is_removed_by_right_click_or_delete_and_z_brings_it_back() {
    let (mut bench, mut tool, ..) = up_on_a_cut_ball();
    let before = key_positions(&tool);
    let (_, first) = key_at(&bench, &tool, 0);
    assert!(bench.press(&mut tool, first, Button::Right));
    bench.release(&mut tool);
    assert_eq!(tool.key_count(), 1);
    assert!(said(&bench).0.contains("1 left"));

    // Delete takes the one under the pointer.
    let (_, second) = key_at(&bench, &tool, 0);
    bench.hover(second);
    assert!(bench.key(&mut tool, Key::Delete));
    assert_eq!(tool.key_count(), 0);
    // With no key under the pointer it is not the tool's.
    assert!(!bench.key(&mut tool, Key::Delete));

    assert!(bench.undo(&mut tool) && bench.undo(&mut tool));
    assert_eq!(key_positions(&tool), before);
    // The joints' own history is spent: the next Z would be the scene's.
    assert!(!bench.send(&mut tool, Event::Undo));
    assert!(bench.redo(&mut tool) && bench.redo(&mut tool));
    assert_eq!(tool.key_count(), 0);
}

#[test]
fn the_wheel_turns_a_keyed_shape_and_leaves_the_camera_the_rest() {
    let mut bench = Bench::new();
    cut_ball(&mut bench);
    let mut tool = Keys::default();
    tool.settings.shape = KeyShape::Hexagon;
    bench.enter(&mut tool);
    assert_eq!(tool.key_count(), 1, "one keyed shape is enough");
    let angle = |tool: &Keys| tool.pair.as_ref().unwrap().plan.joints[0].keys[0].angle;
    let start = angle(&tool);
    let (_, on_key) = key_at(&bench, &tool, 0);
    bench.hover(on_key);
    assert!(bench.send(&mut tool, Event::Wheel(1.0)));
    assert!((angle(&tool) - (start + 15.0)).abs() < 1e-4);
    bench.env.mods.shift = true;
    assert!(bench.send(&mut tool, Event::Wheel(-1.0)));
    assert!((angle(&tool) - (start + 10.0)).abs() < 1e-4);
    bench.env.mods.shift = false;
    assert!(bench.key(&mut tool, Key::R));
    assert!((angle(&tool) - (start + 100.0)).abs() < 1e-4);
    assert!(bench.undo(&mut tool) && bench.undo(&mut tool) && bench.undo(&mut tool));
    assert!((angle(&tool) - start).abs() < 1e-4);

    // Off the key the wheel zooms, and R is not the tool's.
    bench.hover(Vec2::new(5.0, 5.0));
    assert!(!bench.send(&mut tool, Event::Wheel(1.0)));
    assert!(!bench.key(&mut tool, Key::R));
}

#[test]
fn a_press_on_nothing_of_the_tools_is_passed_on() {
    let (mut bench, mut tool, ..) = up_on_a_cut_ball();
    let before = key_positions(&tool);
    // The corner of the viewport: background.
    assert!(!bench.press(&mut tool, Vec2::new(5.0, 5.0), Button::Left));
    bench.drag_to(&mut tool, Vec2::new(90.0, 40.0));
    bench.release(&mut tool);
    assert!(!bench.press(&mut tool, Vec2::new(5.0, 5.0), Button::Right));
    bench.release(&mut tool);
    // A click there means nothing to it either.
    bench.click(&mut tool, Vec2::new(5.0, 5.0), Button::Left);
    assert_eq!(key_positions(&tool), before);
    assert!(!tool.can_undo(false));
}

#[test]
fn settings_resize_the_joints_where_they_stand() {
    let (mut bench, mut tool, ..) = up_on_a_cut_ball();
    let before = key_positions(&tool);
    tool.settings.size = mm(3.0);
    tool.settings.height = mm(2.0);
    tool.settings_changed();
    let jp = &tool.pair.as_ref().unwrap().plan.joints[0];
    assert!((jp.size - mm(3.0)).abs() < 1e-6 && (jp.height - mm(2.0)).abs() < 1e-6);
    assert_eq!(key_positions(&tool), before);
    let pegs = bench.frame(&mut tool);
    let top = pegs.previews()[0].mesh.data().positions.iter().map(|p| p.z).fold(f32::MIN, f32::max);
    assert!((top - (-0.1 + mm(2.0))).abs() < 1e-3, "the previews follow: {top}");

    // Too big to fit where they are: kept, and marked.
    tool.settings.size = mm(9.0);
    tool.settings_changed();
    assert_eq!(tool.invalid_count(), 2);
    // Auto Place plans afresh, as one step that Ctrl+Z takes back.
    bench.call(|cx| tool.auto_place(cx));
    assert!(tool.key_count() >= 1 && tool.invalid_count() == 0);
    assert!(bench.undo(&mut tool));
    assert_eq!(key_positions(&tool), before);

    bench.call(|cx| tool.clear_keys(cx));
    assert_eq!(tool.key_count(), 0);
    bench.undo(&mut tool);
    let on_a = tool.pair.as_ref().unwrap().plan.joints[0].peg_on_a;
    bench.call(|cx| tool.flip_sides(cx));
    assert_eq!(tool.pair.as_ref().unwrap().plan.joints[0].peg_on_a, !on_a);
}

#[test]
fn apply_changes_both_pieces_in_one_undo_step_and_undo_hands_the_joints_back() {
    let (mut bench, mut tool, far, near) = up_on_a_cut_ball();
    let (plain_far, plain_near) = (mesh_of(&bench, far), mesh_of(&bench, near));
    let (triangles_far, triangles_near) = (mesh(&bench, far).triangle_count(), mesh(&bench, near).triangle_count());
    let planned = key_positions(&tool);
    // An edit first, so the joints have a history of their own to lose.
    let (world, from) = key_at(&bench, &tool, 0);
    bench.drag(&mut tool, from, bench.px(world + Vec3::new(0.0, 0.05, 0.0)));
    let edited = key_positions(&tool);
    assert_ne!(edited, planned);

    assert!(bench.key(&mut tool, Key::Enter));
    assert!(tool.job.is_some() && bench.doc.undo_steps() == 0, "the build runs off the interface thread");
    // Nothing can be edited while it runs.
    assert!(!bench.press(&mut tool, from, Button::Left));
    bench.release(&mut tool);
    wait_for_build(&mut bench, &mut tool);

    // Both pieces, one step.
    let (far_id, near_id) = (far.0, near.0);
    assert_eq!(bench.doc.log, ["begin Add Keys".to_string(), format!("replace {far_id}"), format!("replace {near_id}"), "end".to_string()]);
    assert_eq!(bench.doc.undo_steps(), 1);
    assert!(mesh_of(&bench, far) != plain_far && mesh_of(&bench, near) != plain_near);
    let (keyed_far, keyed_near) = (mesh(&bench, far), mesh(&bench, near));
    assert!(keyed_far.triangle_count() > triangles_far && keyed_near.triangle_count() > triangles_near);
    assert_eq!((closed_error(&keyed_far), closed_error(&keyed_near)), (None, None));
    // The pegs stand out of the far half toward the camera; the near half
    // lost the sockets' volume.
    let reach = keyed_far.positions.iter().map(|p| p.z).fold(f32::MIN, f32::max);
    let height = tool.pair.as_ref().unwrap().plan.joints[0].height;
    assert!((reach - (-0.1 + height)).abs() < 1e-3, "pegs reach z = {reach}");
    let (text, ok) = said(&bench);
    assert!(ok && text.starts_with("2 Circle keys") && text.contains("pegs on Far"), "{text}");
    assert!(bench.finished, "the tool puts itself away once the keys are in");
    assert!(!tool.can_undo(false), "the joints' own history ends with the build");

    // The scene's Ctrl+Z takes the keys off both pieces at once...
    assert!(bench.undo(&mut tool));
    assert_eq!((mesh_of(&bench, far), mesh_of(&bench, near)), (plain_far, plain_near));
    assert_eq!(bench.doc.undo_steps(), 0);
    // ...and the tool, on its next frame, has the joints back as they were.
    bench.frame(&mut tool);
    assert!(said(&bench).0.starts_with("Keys taken off"), "{}", said(&bench).0);
    assert_eq!(key_positions(&tool), edited);
    assert!(tool.editable().is_some());
    assert_eq!(bench.frame(&mut tool).previews().len(), 1, "previewed again");

    // Redo puts the keys back; the joints wait behind them, not editable.
    assert!(bench.redo_scene());
    bench.frame(&mut tool);
    assert_eq!(tool.pair.as_ref().unwrap().showing, Showing::Keyed);
    assert!(bench.frame(&mut tool).is_empty());
    bench.enter(&mut tool);
    assert!(said(&bench).0.contains("already have their keys"), "{}", said(&bench).0);
    assert!(!bench.press(&mut tool, from, Button::Left));
    bench.release(&mut tool);
    // Ctrl+Z with the tool up and nothing of its own to step is the scene's, and
    // the joints come straight back under the pointer.
    assert!(bench.undo(&mut tool));
    bench.frame(&mut tool);
    assert!(tool.editable().is_some());
    let (_, on_key) = key_at(&bench, &tool, 1);
    assert!(bench.press(&mut tool, on_key, Button::Left));
    bench.release(&mut tool);
}

#[test]
fn a_mask_painted_on_a_piece_survives_the_build() {
    let mut bench = Bench::new();
    let (far, near) = cut_ball(&mut bench);
    let count = bench.doc.mesh(far).unwrap().vertex_count();
    let painted: Vec<f32> = (0..count).map(|v| if v % 3 == 0 { 1.0 } else { 0.25 }).collect();
    let positions = mesh(&bench, far).positions;
    bench.call(|cx| cx.commit("Mask", vec![Command::Mask { object: far, mask: painted.clone() }]));
    bench.select(&[far]);
    let mut tool = Keys::default();
    bench.enter(&mut tool);
    bench.key(&mut tool, Key::Enter);
    wait_for_build(&mut bench, &mut tool);

    let view = bench.doc.mesh(far).unwrap();
    assert!(view.vertex_count() > count);
    // Every vertex the half came with keeps its value; the keys' own are
    // unmasked.
    let mut kept = 0;
    for (p, &m) in view.positions.iter().zip(view.mask) {
        match positions.iter().position(|q| q == p) {
            Some(v) => {
                assert_eq!(m, painted[v]);
                kept += 1;
            }
            None => assert_eq!(m, 0.0),
        }
    }
    assert!(kept > count / 2);
    assert!(bench.doc.mesh(near).unwrap().mask.iter().all(|&m| m == 0.0));
}

#[test]
fn leaving_keeps_the_joints_and_a_changed_piece_drops_them() {
    let (mut bench, mut tool, far, near) = up_on_a_cut_ball();
    let (world, from) = key_at(&bench, &tool, 0);
    bench.drag(&mut tool, from, bench.px(world + Vec3::new(0.1, 0.1, 0.0)));
    let edited = key_positions(&tool);

    // Put away and brought back: the same joints, still with their history.
    bench.leave(&mut tool);
    bench.enter(&mut tool);
    assert_eq!(key_positions(&tool), edited);
    assert!(tool.can_undo(false));

    // A piece moved since keeps its joints on it.
    bench.doc.place(far, Placement::at(Vec3::new(0.0, 0.4, 0.0)));
    bench.doc.place(near, Placement::at(Vec3::new(0.0, 0.4, 0.0)));
    bench.enter(&mut tool);
    assert_eq!(key_positions(&tool), edited);
    let pegs = bench.frame(&mut tool);
    let middle = pegs.previews()[0].transforms[0].transform_point3(Vec3::ZERO);
    assert!(middle.distance(Vec3::new(0.0, 0.4, 0.0)) < 1e-5, "the pegs are drawn where the half is now");
    let (_, on_key) = key_at(&bench, &tool, 1);
    let shift = bench.px(Vec3::new(0.0, 0.4, -0.1)) - bench.px(Vec3::new(0.0, 0.0, -0.1));
    assert!(bench.press(&mut tool, on_key + shift, Button::Left), "and are picked there");
    bench.release(&mut tool);

    // One of the pieces given other geometry: the joints no longer apply.
    bench.call(|cx| cx.commit("Remesh", vec![Command::Replace { object: near, mesh: cube(0.3).into() }]));
    bench.frame(&mut tool);
    assert_eq!(tool.objects(), None);
    let (text, ok) = said(&bench);
    assert!(!ok && text.contains("have changed"), "{text}");
}

#[test]
fn a_build_can_be_cancelled_and_changes_nothing() {
    let (mut bench, mut tool, far, _) = up_on_a_cut_ball();
    let plain = mesh_of(&bench, far);
    bench.call(|cx| tool.apply(cx, Duration::from_millis(150)));
    let start = Instant::now();
    while tool.job.as_ref().unwrap().progress() != "Building the joints" {
        assert!(start.elapsed() < Duration::from_secs(10), "the build never said what it was doing");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(bench.key(&mut tool, Key::Escape), "Esc stops a build before it leaves the tool");
    wait_for_build(&mut bench, &mut tool);
    assert_eq!(said(&bench), ("Cancelled.", false));
    assert_eq!(mesh_of(&bench, far), plain);
    assert_eq!(bench.doc.undo_steps(), 0);
    assert!(tool.editable().is_some() && !bench.finished);
}

#[test]
fn the_demos_reach_the_states_they_are_for() {
    for what in ["", "move", "holes", "both", "hexagon", "puzzletab", "applied"] {
        let mut bench = Bench::new();
        let ball = bench.add("Ball", icosphere(4, 0.5));
        let mut tool = Keys::default();
        bench.enter(&mut tool);
        assert!(!said(&bench).1, "one whole ball has nothing to key");
        bench.call(|cx| tool.demo(cx, what));
        assert_eq!(bench.doc.objects().len(), 2, "{what}: the ball was split");
        let piece = bench.doc.objects()[1];
        assert_eq!(tool.objects(), Some((ball, piece)), "{what}");
        match what {
            "move" => {
                assert!(tool.drag.is_some(), "a joint is in hand");
                assert!(tool.can_undo(false));
            }
            "holes" => assert_eq!(bench.frame(&mut tool).hidden(), [ball]),
            "both" => assert!(bench.frame(&mut tool).hidden().is_empty()),
            "hexagon" => assert_eq!(tool.settings().shape, KeyShape::Hexagon),
            "puzzletab" => assert_eq!(tool.settings().shape, KeyShape::PuzzleTab),
            "applied" => {
                assert_eq!(tool.pair.as_ref().unwrap().showing, Showing::Keyed);
                assert_eq!((closed_error(&mesh(&bench, ball)), closed_error(&mesh(&bench, piece))), (None, None));
                let apart = bench.doc.bounds(ball).center().distance(bench.doc.bounds(piece).center());
                assert!(apart > 0.8, "the pieces stand {apart} apart");
                assert!(bench.finished);
            }
            _ => {
                let draw = bench.frame(&mut tool);
                assert_eq!(draw.hidden(), [piece]);
                assert!(!draw.previews().is_empty(), "pegs are previewed");
            }
        }
        assert!(tool.key_count() + tool.invalid_count() >= 1, "{what}: {}", said(&bench).0);
    }
}

#[test]
fn the_panel_builds() {
    let (mut bench, mut tool, ..) = up_on_a_cut_ball();
    let ctx = egui::Context::default();
    theme::install(&ctx);
    for pass in 0..2 {
        if pass == 1 {
            // With a build running, the panel shows its progress instead.
            bench.call(|cx| tool.apply(cx, Duration::from_millis(50)));
        }
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            bench.call(|cx| tool.panel(ui, cx));
        });
        assert!(!output.shapes.is_empty(), "the panel drew something");
        output.textures_delta.clear();
    }
    wait_for_build(&mut bench, &mut tool);
    assert_eq!(tool.info().hotkey, Some(Key::J));
}

/// What the app asks before it closes, and before a file takes the scene's place.
fn at_risk(tool: &Keys) -> Option<String> {
    let closing = tool.scaffold_at_risk(Loss::Closing);
    let replaced = tool.scaffold_at_risk(Loss::SceneReplaced);
    assert_eq!(closing, replaced, "the joints stand on the pieces: a new scene costs them too");
    closing
}

#[test]
fn joints_the_tool_planned_by_itself_are_not_work_and_joints_changed_by_hand_are_until_built() {
    let (mut bench, mut tool, ..) = up_on_a_cut_ball();
    assert_eq!(tool.key_count(), 2, "the tool planned two joints as it came up");
    assert_eq!(at_risk(&tool), None, "nobody has touched them: there is nothing to lose");

    // One moved by hand: the plan is work now.
    let (world, from) = key_at(&bench, &tool, 0);
    bench.drag(&mut tool, from, bench.px(world + Vec3::new(0.0, 0.05, 0.0)));
    let two = Some("2 planned keys (Build Keys first)".to_owned());
    assert_eq!(at_risk(&tool), two);
    // Put away, the joints are kept for next time: still there to lose.
    bench.leave(&mut tool);
    assert_eq!(at_risk(&tool), two);
    bench.enter(&mut tool);
    assert_eq!(at_risk(&tool), two);

    // Ctrl+Z takes the move back: the joints are the tool's own again. Ctrl + Shift + Z: the user's.
    assert!(bench.undo(&mut tool));
    assert_eq!(at_risk(&tool), None);
    assert!(bench.redo(&mut tool));
    assert_eq!(at_risk(&tool), two);

    // One removed: one left to lose.
    let (_, first) = key_at(&bench, &tool, 0);
    assert!(bench.press(&mut tool, first, Button::Right));
    bench.release(&mut tool);
    let one = Some("1 planned key (Build Keys first)".to_owned());
    assert_eq!(at_risk(&tool), one);

    // Built: they are geometry of the two pieces, which the file keeps.
    assert!(bench.key(&mut tool, Key::Enter));
    wait_for_build(&mut bench, &mut tool);
    assert_eq!(at_risk(&tool), None, "built keys are part of the pieces");
    // The scene's Ctrl+Z takes the keys off and hands the joints back, hand edits and all.
    assert!(bench.undo(&mut tool));
    bench.frame(&mut tool);
    assert_eq!(at_risk(&tool), one);
}

#[test]
fn a_drag_that_is_cancelled_and_joints_all_removed_leave_nothing_to_lose() {
    let (mut bench, mut tool, ..) = up_on_a_cut_ball();
    let (world, from) = key_at(&bench, &tool, 0);
    // Esc in mid-drag puts the key back: no edit was made.
    bench.press(&mut tool, from, Button::Left);
    let to = bench.px(world + Vec3::new(0.0, 0.25, 0.0));
    bench.drag_to(&mut tool, to);
    assert!(bench.key(&mut tool, Key::Escape));
    bench.release(&mut tool);
    assert_eq!(at_risk(&tool), None);
    // Every joint cleared by hand: an edit, but with no joint left there is nothing to build or to lose.
    bench.call(|cx| tool.clear_keys(cx));
    assert_eq!((tool.key_count(), at_risk(&tool)), (0, None));
}

/// What Open does to planned joints, which is what `Loss::SceneReplaced` has to tell the truth about.
#[test]
fn a_scene_opened_in_place_of_this_one_takes_the_planned_joints() {
    let (mut bench, mut tool, ..) = up_on_a_cut_ball();
    let (world, from) = key_at(&bench, &tool, 0);
    bench.drag(&mut tool, from, bench.px(world + Vec3::new(0.0, 0.05, 0.0)));
    assert!(at_risk(&tool).is_some());
    let ids = bench.open(vec![("Opened", cube(0.5))]);
    bench.load_tool(&mut tool, &ids, vec![None], Arrival::Replaced);
    bench.frame(&mut tool);
    assert!(tool.pair.is_none(), "the pieces went, and the joints planned on them");
    assert_eq!(at_risk(&tool), None);
    // Undo brings the pieces back, but not the joints.
    assert!(bench.undo_scene());
    bench.frame(&mut tool);
    assert!(tool.pair.is_none());
}
