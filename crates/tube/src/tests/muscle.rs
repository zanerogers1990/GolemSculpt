//! The Muscle tool, checked where there is an answer to check against.

use glam::{Vec2, Vec3};
use sculpt_toolkit::testing::Bench;
use sculpt_toolkit::{Button, Command, Event, Key, Loss, ObjectId, Scene, Tool};

use super::tool::{on_sphere, status, stroke, wait_for_merge};
use crate::testing::{assert_closed, icosphere, shells};
use crate::types::{Kind, TubeSettings};
use crate::{TubePoint, TubeTool, muscle};

/// A sphere of radius 0.5 in front of the camera and the Muscle tool brought up on it.
fn bench() -> (Bench, TubeTool, ObjectId) {
    let mut bench = Bench::new();
    let sphere = bench.add("Body", icosphere(4, 0.5, Vec3::ZERO));
    let mut tool = TubeTool::muscle();
    bench.enter(&mut tool);
    assert_eq!((tool.info().id, tool.kind()), ("muscle", Kind::Muscle));
    assert!(tool.on_surface(), "a muscle starts on the surface");
    (bench, tool, sphere)
}

/// How far each station's footing is from the sphere's surface, at worst, as a fraction of the radius.
fn worst_gap(tool: &TubeTool, tube: usize) -> f32 {
    let live = &tool.tubes[tube];
    let settings = &live.state.settings;
    live.copies[0]
        .samples
        .iter()
        .map(|s| {
            let ground = s.center - s.up * muscle::lift(settings, s.radius, s.flat);
            (ground.length() - 0.5).abs() / settings.radius
        })
        .fold(0.0, f32::max)
}

#[test]
fn a_muscle_is_thin_at_both_ends_and_widest_at_its_belly() {
    let settings = TubeSettings::default().muscle();
    let width = |t: f32| muscle::profile(&settings, t);
    assert!(width(0.0) < 0.25 && width(1.0) < 0.25, "{} {}", width(0.0), width(1.0));
    assert!((width(0.5) - 1.0).abs() < 1e-5);
    assert!((0..=100).all(|i| width(i as f32 / 100.0) <= 1.0 + 1e-6));
    // The belly goes where it is put.
    let high = TubeSettings { belly: 0.3, ..settings };
    assert!((muscle::profile(&high, 0.3) - 1.0).abs() < 1e-5);
    assert!(muscle::profile(&high, 0.3) > muscle::profile(&high, 0.5));
}

#[test]
fn a_muscle_between_two_points_lies_on_the_surface_all_the_way() {
    let (mut bench, mut tool, _) = bench();
    stroke(&mut bench, &mut tool, &[on_sphere(-0.3, -0.1), on_sphere(0.3, 0.15)]);
    assert_eq!(tool.tube_count(), 1);
    assert_eq!(tool.point_count(0), 2, "a straight line is two points, whatever the surface under it does");
    bench.frame(&mut tool);
    // The straight run between the two points passes 0.1 under the surface, more than a radius and a
    // half; the muscle's footing is on it.
    assert!(worst_gap(&tool, 0) < 0.12, "{}", worst_gap(&tool, 0));
}

#[test]
fn a_stroke_started_just_off_the_model_is_pulled_onto_it() {
    let (mut bench, mut tool, _) = bench();
    let edge = bench.px(Vec3::new(-0.5, 0.0, 0.0));
    let centre = bench.px(Vec3::ZERO);
    let outward = (edge - centre).normalize();
    // Twelve points of the interface outside the outline.
    let start = edge + outward * 12.0 * bench.env.view.pixels_per_point;
    bench.press(&mut tool, start, Button::Left);
    for step in 1..=20 {
        bench.drag_to(&mut tool, start.lerp(bench.px(on_sphere(0.1, 0.1)), step as f32 / 20.0));
    }
    bench.release(&mut tool);
    assert_eq!(tool.tube_count(), 1, "the stroke was taken");
    assert!(tool.points(0).iter().all(TubePoint::is_on_surface), "{:?}", tool.points(0));
    assert!((tool.points(0)[0].position.length() - 0.5).abs() < 0.01);

    // Far off the model the press is not the tool's: the camera orbits, as everywhere.
    let far = edge + outward * 200.0 * bench.env.view.pixels_per_point;
    assert!(!bench.press(&mut tool, far, Button::Left));
}

#[test]
fn the_same_stroke_gives_the_same_muscle() {
    let draw = || {
        let (mut bench, mut tool, _) = bench();
        stroke(&mut bench, &mut tool, &[on_sphere(-0.25, 0.2), on_sphere(0.0, 0.05), on_sphere(0.3, -0.2)]);
        bench.frame(&mut tool);
        let centres: Vec<Vec3> = tool.tubes[0].copies[0].samples.iter().map(|s| s.center).collect();
        (tool.points(0).to_vec(), centres, tool.tubes[0].preview.data().positions.clone())
    };
    assert_eq!(draw(), draw());
}

#[test]
fn undo_puts_a_muscle_back_exactly() {
    let (mut bench, mut tool, _) = bench();
    stroke(&mut bench, &mut tool, &[on_sphere(-0.3, 0.0), on_sphere(0.3, 0.1)]);
    bench.frame(&mut tool);
    let before = (tool.points(0).to_vec(), tool.settings(), tool.tubes[0].preview.data().positions.clone());

    // Move an end, then reshape it.
    let end: Vec2 = bench.px(tool.handle_world(0, 1, 0).unwrap());
    bench.press(&mut tool, end, Button::Left);
    bench.drag_to(&mut tool, bench.px(on_sphere(0.2, 0.3)));
    bench.release(&mut tool);
    tool.set_swell(0.1);
    tool.set_depth(0.8);
    tool.commit_edit();
    bench.frame(&mut tool);
    assert_ne!(tool.points(0), &before.0[..]);

    while tool.can_undo() && tool.next_undo_label() != Some("Draw Muscle") {
        assert!(tool.undo());
    }
    bench.frame(&mut tool);
    let after = (tool.points(0).to_vec(), tool.settings(), tool.tubes[0].preview.data().positions.clone());
    assert_eq!(after, before);
}

#[test]
fn the_muscle_tool_speaks_of_muscles_and_keeps_its_own() {
    let (mut bench, mut tool, _) = bench();
    assert_eq!(tool.info().hotkey, Some(Key::B));
    assert_eq!(status(&bench), "Drag across the model to lay a muscle on it.");
    stroke(&mut bench, &mut tool, &[on_sphere(-0.3, 0.0), on_sphere(0.3, 0.1)]);
    assert!(status(&bench).starts_with("Muscle drawn on Body: 2 points."), "{}", status(&bench));
    assert_eq!(tool.next_undo_label(), Some("Draw Muscle"));
    assert_eq!(tool.scaffold_at_risk(Loss::Closing).as_deref(), Some("1 unmerged muscle (Merge or Keep Separate first)"));
    // The Tube tool beside it is still the Tube tool.
    let tube = crate::tool();
    assert_eq!((tube.info().id, tube.info().hotkey), ("tube", Some(Key::T)));
}

/// How far the muscle's footing is from a sphere of `radius`, at worst, in muscle radii.
fn worst_gap_from(tool: &TubeTool, radius: f32) -> f32 {
    let live = &tool.tubes[0];
    let settings = &live.state.settings;
    let gap = |s: &crate::Sample| {
        let ground = s.center - s.up * muscle::lift(settings, s.radius, s.flat);
        (ground.length() - radius).abs() / settings.radius
    };
    live.copies[0].samples.iter().map(gap).fold(0.0, f32::max)
}

#[test]
fn a_muscle_follows_the_model_when_its_shape_changes() {
    let (mut bench, mut tool, body) = bench();
    stroke(&mut bench, &mut tool, &[on_sphere(-0.25, -0.1), on_sphere(0.25, 0.15)]);
    bench.frame(&mut tool);
    assert!(worst_gap_from(&tool, 0.5) < 0.12);

    // A new mesh in the body's place, a little larger: the muscle lies on that one.
    bench.call(|cx| cx.commit("Swell", vec![Command::Replace { object: body, mesh: icosphere(4, 0.54, Vec3::ZERO).into() }]));
    bench.frame(&mut tool);
    assert!(worst_gap_from(&tool, 0.54) < 0.12, "{}", worst_gap_from(&tool, 0.54));
    // Its handles are on the surface as it is now, where its points were put down on the old one.
    let handle = tool.handle_world(0, 0, 0).unwrap();
    assert!((handle.length() - 0.54).abs() < 0.005, "{}", handle.length());
    assert!((tool.points(0)[0].position.length() - 0.5).abs() < 0.005, "the point itself is as it was drawn");

    // The scene stepped back, the muscle goes back down with the model.
    assert!(bench.undo_scene());
    bench.frame(&mut tool);
    assert!(worst_gap_from(&tool, 0.5) < 0.12, "{}", worst_gap_from(&tool, 0.5));

    // A step of the scene's history that the tool passes on (it has nothing of its own to redo) may
    // have reshaped a model without changing which mesh it is: every muscle is laid again.
    assert!(!bench.send(&mut tool, Event::Redo));
    assert!(tool.scene_stepped);
    bench.frame(&mut tool);
    assert!(!tool.scene_stepped);
}

#[test]
fn a_merged_muscle_is_the_muscle_that_was_drawn_and_not_a_swollen_one() {
    let (mut bench, mut tool, body) = bench();
    tool.set_radius(0.08);
    stroke(&mut bench, &mut tool, &[on_sphere(-0.25, 0.0), on_sphere(0.25, 0.0)]);
    bench.frame(&mut tool);
    let settings = tool.settings();
    let sphere = bench.doc.mesh(body).unwrap().to_tri_mesh();

    assert!(bench.key(&mut tool, Key::Enter));
    wait_for_merge(&mut bench, &mut tool);
    assert!(status(&bench).starts_with("Merged 1 muscle in"), "{}", status(&bench));
    let merged = bench.doc.mesh(body).unwrap().to_tri_mesh();
    assert_closed(&merged, "the body with its muscle");
    assert_eq!(shells(&merged), 1);
    assert!(merged.signed_volume() > sphere.signed_volume());
    // It stands as far off the body as it did live, and no further: the fillet is sized by its height.
    let highest = merged.positions.iter().map(|p| p.length()).fold(0.0, f32::max) - 0.5;
    assert!(highest > settings.proud() * 0.8 && highest < settings.proud() * 1.15, "{highest} against {}", settings.proud());
    // And it is no wider: nothing of the body a width and a half from its line has moved out.
    let beside = merged.positions.iter().filter(|p| p.z > 0.0 && p.y.abs() > settings.radius * 1.5);
    let swollen = beside.map(|p| p.length() - 0.5).fold(0.0, f32::max);
    assert!(swollen < settings.proud() * 0.1, "{swollen}");
}

#[test]
fn a_muscle_drawn_free_over_the_model_floats_in_front_of_it() {
    let (mut bench, mut tool, _) = bench();
    tool.set_on_surface(false);
    stroke(&mut bench, &mut tool, &[on_sphere(-0.12, 0.0), on_sphere(0.12, 0.05)]);
    assert_eq!(tool.tube_count(), 1);
    assert!(tool.points(0).iter().all(|p| !p.is_on_surface()));
    bench.frame(&mut tool);
    let nearest = tool.tubes[0].preview.data().positions.iter().map(|p| p.length()).fold(f32::MAX, f32::min);
    assert!(nearest > 0.5, "all of it is outside the body: {nearest}");
    assert!(nearest < 0.5 + tool.settings().radius, "and close in front of it: {nearest}");
}

#[test]
fn with_free_placement_a_dragged_point_lifts_off_the_surface_and_keeps_the_way_it_was_turned() {
    let (mut bench, mut tool, _) = bench();
    stroke(&mut bench, &mut tool, &[on_sphere(-0.25, 0.0), on_sphere(0.25, 0.0)]);
    let was = tool.points(0)[1];
    assert!(was.is_on_surface());

    // On the surface a dragged point slides over the model and stays on it.
    let end = bench.px(tool.handle_world(0, 1, 0).unwrap());
    bench.drag(&mut tool, end, bench.px(on_sphere(0.2, 0.15)));
    assert!(tool.points(0)[1].is_on_surface());

    // With Free it floats from where it was, and is still thin the way the surface faced.
    tool.set_on_surface(false);
    let before = tool.points(0)[1];
    let end = bench.px(tool.handle_world(0, 1, 0).unwrap());
    bench.drag(&mut tool, end, end + Vec2::new(40.0, -30.0));
    let lifted = tool.points(0)[1];
    assert!(!lifted.is_on_surface());
    assert_eq!(lifted.up(), before.up());
    assert!(tool.points(0)[0].is_on_surface(), "one end on the model, one off it: a muscle can be both");

    // Back with On Surface, dragging it over the model sticks it down again.
    tool.set_on_surface(true);
    let end = bench.px(tool.handle_world(0, 1, 0).unwrap());
    bench.drag(&mut tool, end, bench.px(on_sphere(0.1, -0.1)));
    assert!(tool.points(0)[1].is_on_surface());
}
