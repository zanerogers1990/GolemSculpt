//! The tool driven the way the app drives it, against the toolkit's bench.

use glam::{Vec2, Vec3};
use sculpt_geom::LengthUnit;
use sculpt_toolkit::testing::{Bench, cube};
use sculpt_toolkit::{Arrival, Button, Draw, Key, Loss, ObjectId, Placement, Scene, Shape, Tool};

use crate::Measure;
use crate::panel::resize;

/// A unit cube in front of the camera and the tool brought up on it. The
/// cube's front face is the square z = 0.5; one unit is ten millimetres.
fn bench() -> (Bench, Measure, ObjectId) {
    let mut bench = Bench::new();
    let cube = bench.add("Cube", cube(0.5));
    let mut tool = Measure::default();
    bench.enter(&mut tool);
    (bench, tool, cube)
}

/// The pixel a point of the cube's front face is drawn at.
fn front(bench: &Bench, x: f32, y: f32) -> Vec2 {
    bench.px(Vec3::new(x, y, 0.5))
}

fn label_texts(draw: &Draw) -> Vec<String> {
    draw.overlay
        .shapes
        .iter()
        .filter_map(|shape| match shape {
            Shape::Label(label) => Some(label.runs.iter().map(|run| run.text.as_str()).collect::<String>()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_drag_across_the_model_lays_a_ruler_between_the_two_surface_points() {
    let (mut bench, mut tool, _) = bench();
    bench.drag(&mut tool, front(&bench, -0.4, 0.0), front(&bench, 0.4, 0.0));
    assert_eq!(tool.rulers().len(), 1);
    let ruler = tool.rulers().get(0).unwrap();
    assert!(ruler.a.distance(Vec3::new(-0.4, 0.0, 0.5)) < 1e-3, "{:?}", ruler.a);
    assert!(ruler.b.distance(Vec3::new(0.4, 0.0, 0.5)) < 1e-3, "{:?}", ruler.b);
    assert!(!tool.rulers().is_dragging());
    // 0.8 units at ten millimetres a unit.
    let draw = bench.frame(&mut tool);
    assert_eq!(label_texts(&draw), ["8.00 mm"]);
}

#[test]
fn the_reading_follows_the_display_unit() {
    let (mut bench, mut tool, _) = bench();
    bench.drag(&mut tool, front(&bench, -0.4, 0.0), front(&bench, 0.4, 0.0));
    assert!(bench.key(&mut tool, Key::U));
    assert_eq!(bench.env.unit, LengthUnit::Inches);
    assert_eq!(label_texts(&bench.frame(&mut tool)), ["0.315 in"]);
}

#[test]
fn a_slanted_selected_ruler_shows_its_axis_breakdown_and_legs() {
    let (mut bench, mut tool, _) = bench();
    bench.drag(&mut tool, front(&bench, -0.3, -0.2), front(&bench, 0.3, 0.2));
    let draw = bench.frame(&mut tool);
    let texts = label_texts(&draw);
    assert_eq!(texts.len(), 1);
    assert!(texts[0].starts_with("7.21 mm\n"), "{:?}", texts[0]);
    assert!(texts[0].contains("X 6.00") && texts[0].contains("Y 4.00") && texts[0].contains("Z 0.000"), "{:?}", texts[0]);
    // One dashed leg per axis the ruler runs along.
    let dashed = draw.overlay.shapes.iter().filter(|s| matches!(s, Shape::Line { dash: Some(_), .. })).count();
    assert_eq!(dashed, 2);
}

#[test]
fn a_click_then_a_click_lays_a_ruler_end_by_end() {
    let (mut bench, mut tool, _) = bench();
    bench.click(&mut tool, front(&bench, -0.3, 0.3), Button::Left);
    assert!(tool.rulers().is_placing());
    // The loose end follows the pointer with no button down.
    bench.hover(front(&bench, 0.1, 0.3));
    bench.frame(&mut tool);
    assert!((tool.rulers().get(0).unwrap().length() - 0.4).abs() < 1e-3);
    bench.click(&mut tool, front(&bench, 0.3, 0.3), Button::Left);
    assert!(!tool.rulers().is_dragging());
    assert!((tool.rulers().get(0).unwrap().length() - 0.6).abs() < 1e-3);
}

#[test]
fn two_clicks_on_one_spot_leave_nothing() {
    let (mut bench, mut tool, _) = bench();
    let spot = front(&bench, 0.1, 0.1);
    bench.click(&mut tool, spot, Button::Left);
    bench.click(&mut tool, spot, Button::Left);
    assert!(tool.rulers().is_empty());
}

#[test]
fn a_press_on_empty_background_is_passed_on_so_a_drag_there_orbits() {
    let (mut bench, mut tool, _) = bench();
    let corner = Vec2::new(20.0, 20.0);
    assert!(!bench.press(&mut tool, corner, Button::Left));
    bench.drag_to(&mut tool, Vec2::new(200.0, 80.0));
    bench.release(&mut tool);
    assert!(tool.rulers().is_empty());
}

#[test]
fn a_click_on_empty_background_starts_a_ruler_at_the_depth_of_the_model() {
    let (mut bench, mut tool, _) = bench();
    bench.click(&mut tool, Vec2::new(60.0, 300.0), Button::Left);
    assert!(tool.rulers().is_placing());
    let start = tool.rulers().get(0).unwrap().a;
    assert!(start.z.abs() < 1e-3, "on the plane through the centre of the cube: {start:?}");
    // The second click may be off the model too.
    bench.click(&mut tool, Vec2::new(740.0, 300.0), Button::Left);
    let ruler = tool.rulers().get(0).unwrap();
    assert!(!tool.rulers().is_dragging());
    assert!(ruler.b.z.abs() < 1e-3 && ruler.length() > 1.0, "{ruler:?}");
}

#[test]
fn ctrl_ignores_the_surface() {
    let (mut bench, mut tool, _) = bench();
    bench.env.mods.ctrl = true;
    bench.drag(&mut tool, front(&bench, -0.3, 0.0), front(&bench, 0.3, 0.0));
    let ruler = tool.rulers().get(0).unwrap();
    assert!(ruler.a.z.abs() < 1e-3 && ruler.b.z.abs() < 1e-3, "both ends on the plane through the centre: {ruler:?}");
}

#[test]
fn shift_locks_the_ruler_to_the_axis_it_runs_closest_to() {
    let (mut bench, mut tool, _) = bench();
    let (from, to) = (front(&bench, -0.3, -0.1), front(&bench, 0.3, 0.2));
    bench.press(&mut tool, from, Button::Left);
    bench.env.mods.shift = true;
    bench.drag_to(&mut tool, to);
    let texts = label_texts(&bench.frame(&mut tool));
    assert!(texts[0].ends_with("(X only)"), "{:?}", texts[0]);
    bench.release(&mut tool);
    let d = tool.rulers().get(0).unwrap().delta();
    assert!((d.x - 0.6).abs() < 1e-3 && d.y.abs() < 1e-6 && d.z.abs() < 1e-6, "{d:?}");
}

#[test]
fn dragging_an_end_re_measures_and_escape_puts_it_back() {
    let (mut bench, mut tool, _) = bench();
    bench.drag(&mut tool, front(&bench, -0.4, 0.0), front(&bench, 0.0, 0.0));
    let end = front(&bench, 0.0, 0.0);
    bench.drag(&mut tool, end, front(&bench, 0.4, 0.0));
    assert_eq!(tool.rulers().len(), 1, "the press grabbed the end rather than starting a ruler");
    assert!((tool.rulers().get(0).unwrap().length() - 0.8).abs() < 1e-3);

    bench.press(&mut tool, front(&bench, 0.4, 0.0), Button::Left);
    bench.drag_to(&mut tool, front(&bench, 0.4, 0.4));
    assert!(tool.rulers().get(0).unwrap().b.y > 0.3);
    assert!(bench.key(&mut tool, Key::Escape));
    bench.release(&mut tool);
    assert!(tool.rulers().get(0).unwrap().b.distance(Vec3::new(0.4, 0.0, 0.5)) < 1e-3);
}

#[test]
fn escape_cancels_then_deselects_then_is_left_to_the_app() {
    let (mut bench, mut tool, _) = bench();
    bench.drag(&mut tool, front(&bench, -0.4, 0.0), front(&bench, 0.4, 0.0));
    bench.click(&mut tool, front(&bench, 0.0, 0.3), Button::Left);
    assert_eq!(tool.rulers().len(), 2);
    assert!(bench.key(&mut tool, Key::Escape), "drops the ruler being placed");
    assert_eq!(tool.rulers().len(), 1);
    assert!(bench.key(&mut tool, Key::Escape), "lets go of the selection");
    assert!(tool.rulers().selected().is_none());
    assert!(!bench.key(&mut tool, Key::Escape), "nothing left: the app puts the tool away");
}

#[test]
fn clicking_a_ruler_selects_it_and_right_clicking_or_delete_removes_it() {
    let (mut bench, mut tool, _) = bench();
    bench.drag(&mut tool, front(&bench, -0.4, 0.3), front(&bench, 0.4, 0.3));
    bench.drag(&mut tool, front(&bench, -0.4, -0.3), front(&bench, 0.4, -0.3));
    assert!((tool.rulers().selected().unwrap().a.y + 0.3).abs() < 1e-3, "the newest is selected");

    bench.click(&mut tool, front(&bench, 0.0, 0.3), Button::Left);
    assert_eq!(tool.rulers().len(), 2, "a click on a ruler selects; it does not start another");
    assert!((tool.rulers().selected().unwrap().a.y - 0.3).abs() < 1e-3);

    assert!(bench.press(&mut tool, front(&bench, 0.1, -0.3), Button::Right));
    bench.release(&mut tool);
    assert_eq!(tool.rulers().len(), 1);
    assert!((tool.rulers().selected().unwrap().a.y - 0.3).abs() < 1e-3, "the selection followed its ruler up a row");

    assert!(bench.key(&mut tool, Key::Delete));
    assert!(tool.rulers().is_empty());
    assert!(!bench.key(&mut tool, Key::Delete), "nothing selected: not a key for the tool");
}

#[test]
fn nothing_is_drawn_while_the_tool_is_put_away_or_the_view_is_clean() {
    let (mut bench, mut tool, _) = bench();
    bench.drag(&mut tool, front(&bench, -0.4, 0.0), front(&bench, 0.4, 0.0));
    assert!(!bench.frame(&mut tool).is_empty());
    bench.env.clean_view = true;
    assert!(bench.frame(&mut tool).is_empty());
    bench.env.clean_view = false;
    bench.env.active = false;
    assert!(bench.frame(&mut tool).is_empty());
    bench.env.active = true;
    assert!(!bench.frame(&mut tool).is_empty(), "the rulers are still there next time");
}

#[test]
fn leaving_mid_placement_drops_the_unfinished_ruler() {
    let (mut bench, mut tool, _) = bench();
    bench.click(&mut tool, front(&bench, 0.0, 0.0), Button::Left);
    assert!(tool.rulers().is_placing());
    bench.leave(&mut tool);
    assert!(tool.rulers().is_empty());
}

#[test]
fn the_demo_lays_two_rulers_through_the_gesture_code() {
    let (mut bench, mut tool, _) = bench();
    bench.call(|cx| sculpt_toolkit::Tool::demo(&mut tool, cx, ""));
    assert_eq!(tool.rulers().len(), 2);
    let locked = tool.rulers().get(0).unwrap().delta();
    assert!(locked.x.abs() < 1e-6 && locked.y.abs() > 0.5, "the first is a pure height: {locked:?}");
    assert!(tool.rulers().selected().is_some());
}

#[test]
fn typing_a_size_scales_the_object_as_one_undo_step() {
    let (mut bench, _, cube) = bench();
    // 25 mm wide, keeping proportions: 2.5 units on every axis.
    bench.call(|cx| resize(cx, cube, 0, 2.5, true)).unwrap();
    let scale = bench.doc.info(cube).unwrap().placement.scale;
    assert!(scale.distance(Vec3::splat(2.5)) < 1e-5);
    assert_eq!(bench.doc.undo_steps(), 1);
    assert!(bench.undo_scene());
    assert_eq!(bench.doc.info(cube).unwrap().placement, Placement::IDENTITY);
}

#[test]
fn sizing_to_the_size_it_already_has_leaves_no_undo_step() {
    let (mut bench, _, cube) = bench();
    bench.call(|cx| resize(cx, cube, 1, 1.0, false)).unwrap();
    assert_eq!(bench.doc.undo_steps(), 0);
    assert!(bench.call(|cx| resize(cx, ObjectId(999), 1, 1.0, false)).is_err());
}

/// A ruler is a reading, not work: one drag brings it back, and nothing could keep it. The app is not
/// asked to hold up closing for one (see the top of lib.rs).
#[test]
fn rulers_are_not_work_the_app_is_asked_to_warn_about() {
    let (mut bench, mut tool, _) = bench();
    for loss in [Loss::Closing, Loss::SceneReplaced] {
        assert_eq!(tool.scaffold_at_risk(loss), None, "nothing laid");
    }
    bench.drag(&mut tool, front(&bench, -0.4, 0.0), front(&bench, 0.4, 0.0));
    bench.drag(&mut tool, front(&bench, 0.0, -0.4), front(&bench, 0.0, 0.4));
    assert_eq!(tool.rulers().len(), 2);
    for loss in [Loss::Closing, Loss::SceneReplaced] {
        assert_eq!(tool.scaffold_at_risk(loss), None, "two rulers laid");
    }
}

/// What Open does to rulers: they stand in the world, so they are still there on the new scene.
#[test]
fn a_scene_opened_in_place_of_this_one_leaves_the_rulers_where_they_are() {
    let (mut bench, mut tool, _) = bench();
    bench.drag(&mut tool, front(&bench, -0.4, 0.0), front(&bench, 0.4, 0.0));
    let laid = tool.rulers().get(0).unwrap();
    let ids = bench.open(vec![("Opened", cube(0.25))]);
    bench.load_tool(&mut tool, &ids, vec![None], Arrival::Replaced);
    bench.frame(&mut tool);
    assert_eq!(tool.rulers().len(), 1);
    let kept = tool.rulers().get(0).unwrap();
    assert_eq!((kept.a, kept.b), (laid.a, laid.b));
}

/// One frame of the Scene panel's part of the tool: its Measure & Size card
/// in a panel in view, or in one folded away and run out of sight. Returns
/// how tall the card was drawn (nothing when folded away).
fn card_frame(ctx: &egui::Context, bench: &mut Bench, tool: &mut Measure, folded: bool) -> f32 {
    let mut height = 0.0;
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 900.0));
    let input = egui::RawInput { screen_rect: Some(screen), ..Default::default() };
    ctx.run_ui(input, |root| {
        bench.call(|cx| {
            if folded {
                sculpt_toolkit::floating::only(root, "right", 240.0, |ui| sculpt_toolkit::Tool::card(tool, ui, cx));
                return;
            }
            egui::Area::new(egui::Id::new("scene_panel")).fixed_pos(egui::pos2(1300.0, 10.0)).show(root.ctx(), |ui| {
                ui.set_width(240.0);
                sculpt_toolkit::Tool::card(tool, ui, cx);
                height = ui.min_rect().height();
            });
        });
    })
    .drop_without_applying_deltas();
    height
}

/// Bringing the tool up opens its card in the Scene panel. With that panel
/// folded away at the time (and run out of sight, because some other card
/// floats), the card is open when the panel comes back.
#[test]
fn the_size_card_opens_for_the_tool_even_if_its_panel_was_folded_away_then() {
    let ctx = egui::Context::default();
    sculpt_toolkit::theme::install(&ctx);
    let mut bench = Bench::new();
    bench.add("Cube", cube(0.5));
    let mut tool = Measure::default();
    let closed = card_frame(&ctx, &mut bench, &mut tool, false);
    assert!(closed < 40.0, "the card starts closed, a header and no more: {closed}");

    sculpt_toolkit::floating::float(&ctx, "Brushes", egui::pos2(400.0, 100.0));
    bench.enter(&mut tool);
    for _ in 0..2 {
        card_frame(&ctx, &mut bench, &mut tool, true);
    }
    card_frame(&ctx, &mut bench, &mut tool, false);
    let open = card_frame(&ctx, &mut bench, &mut tool, false);
    assert!(open > 100.0, "back in view the card is open, sizes and all: {open}");
}
