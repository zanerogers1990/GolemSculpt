//! The tool driven through the toolkit's headless bench: seating, the preview, the gizmo, its own
//! undo, and Accept.

use std::time::{Duration, Instant};

use glam::{Quat, Vec3};
use sculpt_geom::EdgeCensus;
use sculpt_toolkit::testing::{Bench, cube};
use sculpt_toolkit::{Key, Placement, Scene, Tool, theme};

use crate::tool::{HandTool, Kind, Side, mirror_placement};

fn bench() -> (Bench, HandTool) {
    let mut bench = Bench::new();
    bench.add("Body", cube(0.5));
    let mut tool = HandTool::new();
    bench.enter(&mut tool);
    (bench, tool)
}

/// Runs frames until the scene has `count` objects.
fn wait_for(bench: &mut Bench, tool: &mut HandTool, count: usize) {
    let start = Instant::now();
    while bench.doc.objects().len() < count {
        assert!(start.elapsed() < Duration::from_secs(30), "the build never landed");
        bench.frame(tool);
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn coming_up_puts_a_hand_beside_the_model_with_the_gizmo_on_its_wrist() {
    let (_bench, tool) = bench();
    let at = tool.setup().hand_at;
    // A right hand on the figure's right: the -X side of the cube, clear of it.
    assert!(at.position.x < -0.5, "{at:?}");
    assert!(at.scale.x > 0.2 && at.scale.x < 2.0, "sized to the cube: {at:?}");
    let gizmo = tool.gizmo().unwrap();
    assert_eq!(gizmo.placement, at);
    assert!(gizmo.moves && gizmo.turns && gizmo.scales);
}

#[test]
fn a_frame_shows_one_preview_or_two_for_a_pair() {
    let (mut bench, mut tool) = bench();
    let draw = bench.frame(&mut tool);
    assert_eq!(draw.previews().len(), 1);
    assert!(!draw.previews()[0].mesh.is_empty());
    bench.call(|cx| tool.edit(cx, |s| s.side = Side::Pair));
    let draw = bench.frame(&mut tool);
    assert_eq!(draw.previews().len(), 2);
    let (a, b) = (draw.previews()[0].transforms[0], draw.previews()[1].transforms[0]);
    assert!((a.w_axis.x + b.w_axis.x).abs() < 1e-5, "the second is across X = 0");
}

#[test]
fn nothing_is_drawn_while_the_tool_is_away() {
    let (mut bench, mut tool) = bench();
    bench.env.active = false;
    assert!(bench.frame(&mut tool).previews().is_empty());
}

#[test]
fn the_gizmo_moves_it_and_ctrl_z_puts_it_back() {
    let (mut bench, mut tool) = bench();
    let start = tool.setup().hand_at;
    let to = Placement { position: Vec3::new(2.0, 1.0, 0.0), rotation: Quat::from_rotation_z(0.5), scale: Vec3::splat(0.7) };
    assert!(bench.gizmo(&mut tool, to));
    assert_eq!(tool.setup().hand_at, to);
    assert!(tool.can_undo(false));
    assert!(bench.undo(&mut tool));
    assert_eq!(tool.setup().hand_at, start);
    assert!(bench.redo(&mut tool));
    assert_eq!(tool.setup().hand_at, to);
}

#[test]
fn a_gizmo_press_that_moves_nothing_leaves_no_step() {
    let (mut bench, mut tool) = bench();
    let start = tool.setup().hand_at;
    assert!(bench.gizmo(&mut tool, start));
    assert!(!tool.can_undo(false));
}

#[test]
fn a_change_is_a_step_of_the_tools_own_and_its_history_falls_through_to_the_scene() {
    let (mut bench, mut tool) = bench();
    bench.call(|cx| tool.edit(cx, |s| s.kind = Kind::Foot));
    assert_eq!(tool.setup().kind, Kind::Foot);
    assert_eq!(tool.gizmo().unwrap().placement, tool.setup().foot_at, "the gizmo goes to the foot");
    assert!(bench.undo(&mut tool));
    assert_eq!(tool.setup().kind, Kind::Hand);
    assert!(!tool.can_undo(false));
}

#[test]
fn slider_moves_close_together_are_one_step() {
    let (mut bench, mut tool) = bench();
    for (i, t) in [10.0, 10.1, 10.2, 10.3].into_iter().enumerate() {
        bench.env.time = t;
        let before = *tool.setup();
        let mut changed = before;
        changed.hand.finger_length = 1.0 + 0.1 * (i + 1) as f32;
        tool.set_setup(changed);
        tool.changed(before, t, false);
    }
    assert!(bench.undo(&mut tool));
    assert_eq!(tool.setup().hand.finger_length, 1.0);
    assert!(!tool.can_undo(false));
}

#[test]
fn enter_makes_one_closed_object_in_one_undo_step_and_puts_the_tool_away() {
    let (mut bench, mut tool) = bench();
    tool.set_detail(120.0);
    let at = tool.setup().hand_at;
    assert!(bench.key(&mut tool, Key::Enter));
    wait_for(&mut bench, &mut tool, 2);
    let made = bench.doc.active_info().unwrap();
    assert_eq!(made.name, "Right Hand");
    assert!(bench.finished);
    assert_eq!(made.placement.position, at.position);
    assert_eq!(made.placement.scale, Vec3::ONE, "the size is in the mesh");
    let mesh = bench.doc.mesh(bench.doc.active()).unwrap().to_tri_mesh();
    assert!(EdgeCensus::of(&mesh).is_closed());
    let height = mesh.bounds().size().y;
    assert!(height > at.scale.y * 0.95 && height < at.scale.y * 1.5, "{height} for a hand {} long", at.scale.y);
    assert!(bench.undo(&mut tool));
    assert_eq!(bench.doc.objects().len(), 1);
}

#[test]
fn a_pair_is_two_objects_mirrored_across_the_world() {
    let (mut bench, mut tool) = bench();
    tool.set_detail(100.0);
    bench.call(|cx| tool.edit(cx, |s| {
        s.side = Side::Pair;
        s.hand_at.rotation = Quat::from_rotation_y(0.4);
    }));
    assert!(bench.key(&mut tool, Key::Enter));
    wait_for(&mut bench, &mut tool, 3);
    let ids = bench.doc.objects();
    let right = bench.doc.info(ids[1]).unwrap();
    let left = bench.doc.info(ids[2]).unwrap();
    assert_eq!((&*right.name, &*left.name), ("Right Hand", "Left Hand"));
    assert_eq!(left.placement, mirror_placement(right.placement));
    // In the world, every point of one has its reflection on the other.
    let world = |id| {
        let info = bench.doc.info(id).unwrap();
        let mut mesh = bench.doc.mesh(id).unwrap().to_tri_mesh();
        mesh.transform(&info.matrix());
        mesh.bounds()
    };
    let (a, b) = (world(ids[1]), world(ids[2]));
    assert!((a.min.x + b.max.x).abs() < 1e-3 && (a.max.x + b.min.x).abs() < 1e-3, "{a:?} {b:?}");
    assert!((a.min.y - b.min.y).abs() < 1e-3);
    // One step takes both away.
    assert!(bench.undo(&mut tool));
    assert_eq!(bench.doc.objects().len(), 1);
}

#[test]
fn a_left_foot_is_named_and_made_as_one() {
    let (mut bench, mut tool) = bench();
    tool.set_detail(100.0);
    bench.call(|cx| tool.edit(cx, |s| {
        s.kind = Kind::Foot;
        s.side = Side::Left;
    }));
    bench.call(|cx| tool.demo(cx, "made"));
    let made = bench.doc.active_info().unwrap();
    assert_eq!(made.name, "Left Foot");
    let mesh = bench.doc.mesh(bench.doc.active()).unwrap().to_tri_mesh();
    // The big toe of a left foot is on its -X side.
    let bounds = mesh.bounds();
    assert!(bounds.min.x.abs() > bounds.max.x.abs() * 0.8, "{bounds:?}");
    assert!(mesh.signed_volume() > 0.0, "still wound outward");
}

#[test]
fn the_panel_builds_for_a_hand_and_a_foot() {
    let (mut bench, mut tool) = bench();
    let ctx = egui::Context::default();
    theme::install(&ctx);
    for kind in [Kind::Hand, Kind::Foot] {
        bench.call(|cx| tool.edit(cx, |s| s.kind = kind));
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            bench.call(|cx| tool.panel(ui, cx));
        });
        assert!(!output.shapes.is_empty());
        output.textures_delta.clear();
    }
    // Drawing the panel changed nothing.
    assert_eq!(tool.setup().kind, Kind::Foot);
}

#[test]
fn the_estimate_follows_detail() {
    let (mut bench, mut tool) = bench();
    bench.frame(&mut tool);
    tool.set_detail(100.0);
    let low = tool.estimate_triangles();
    tool.set_detail(400.0);
    assert!(tool.estimate_triangles() > low * 10);
    tool.set_detail(f32::NAN);
    assert!(tool.detail() >= 100.0);
}
