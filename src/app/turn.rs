//! What the view turns about.
//!
//! The camera has a pivot in the middle of the picture, and a turn used to
//! go about it wherever it was. But the pivot is only where the last pan,
//! zoom or stroke-pivot turn left it: after a few of those it is off to one
//! side of the model, or well behind it, and the next turn swings the model
//! on the end of that arm, smaller and smaller and out of the picture. An
//! axis view did the same and, switching to orthographic, also resized the
//! model by how far off the pivot's depth was.
//!
//! So every turn made by hand is given a point on the model to go about
//! ([`turn_point`]), chosen once when the turn begins and kept until it
//! ends, and the pivot is first brought level with that point
//! (`Camera::seat_pivot`, which changes nothing in the picture). The point
//! is, in order:
//!
//! 1. the last place sculpted, for a drag on the view with Stroke Pivot on
//!    and that place in view, as before;
//! 2. the middle of the model on the line of sight through the middle of the
//!    view: half way between where the line goes into the object in front
//!    and where it last comes out. A whole model in view turns in place; a
//!    close-up turns about the part being looked at, not about the middle of
//!    a figure whose hand is on screen;
//! 3. the same under the pointer, then on the nearest line to the middle
//!    that meets anything, when the middle of the view is empty (a model off
//!    to one side, the gap between two legs);
//! 4. the middle of the scene's bounds, when the model is in view but too
//!    small or thin for any of those lines to meet.
//!
//! An axis view goes orthographic, and the first turn made by hand after it
//! goes back to perspective (`Camera::ortho_from_axis`), as Blender's Auto
//! Perspective does: a side view is for looking straight at, and turning
//! away from it in orthographic flattens the model in a way that reads as
//! wrong. Orthographic chosen with the Ortho / Persp button stays on. The
//! switch back is made with the pivot already seated on the model, so the
//! model keeps its size on screen; so is the button's (`toggle_ortho`).
//!
//! With none of them (nothing in view at all) the turn is about the pivot,
//! as it always was.
//!
//! The same press also decides a double-click: the second click on the
//! background frames the model, and is not then a turn as well. It used to
//! be, and beginning the turn stopped the glide it had just asked for.

use glam::{Vec2, Vec3};

use super::{Drag, Running};
use crate::camera::View;
use crate::mesh::Mesh;
use crate::scene::Scene;

/// Lines of sight tried each way across the view when its middle is empty.
const GRID: usize = 9;

/// The middle of the model along the line of sight through a pixel: half way
/// between where the line goes into the nearest object and where it last
/// comes out of that object's mesh. `None` where the line meets nothing.
fn middle_along(view: &View, scene: &Scene, slot: &Mesh, px: Vec2) -> Option<Vec3> {
    let (origin, dir) = view.ray(px);
    let near = scene.pick(origin, dir, slot)?;
    let enter = (near.pos - origin).dot(dir);
    // The way out is the first thing met looking back from beyond the scene.
    let bounds = scene.bounds(slot);
    let beyond = (bounds.center() - origin).dot(dir) + bounds.size().length();
    let leave = scene
        .get(near.id)
        .and_then(|object| scene.pick_mesh(origin + dir * beyond, -dir, object.mesh, slot))
        .map_or(enter, |far| (far.pos - origin).dot(dir).max(enter));
    Some(origin + dir * ((enter + leave) * 0.5))
}

/// The point on the model a turn of `view` should go about (2 to 4 of the
/// list at the top of this file), or `None` with nothing of the scene in
/// view. `pointer` is where a press on the view was made, if one was.
pub(super) fn turn_point(view: &View, scene: &Scene, slot: &Mesh, pointer: Option<Vec2>) -> Option<Vec3> {
    let centre = view.origin + view.size * 0.5;
    let middle = |px: Vec2| middle_along(view, scene, slot, px);
    if let Some(point) = middle(centre) {
        return Some(point);
    }
    if let Some(point) = pointer.filter(|px| view.contains(*px)).and_then(middle) {
        return Some(point);
    }
    let mut cells: Vec<Vec2> = (0..GRID * GRID)
        .map(|i| Vec2::new((i % GRID) as f32 + 0.5, (i / GRID) as f32 + 0.5) / GRID as f32)
        .map(|uv| view.origin + view.size * uv)
        .collect();
    cells.sort_by(|a, b| a.distance_squared(centre).total_cmp(&b.distance_squared(centre)));
    if let Some(point) = cells.into_iter().find_map(middle) {
        return Some(point);
    }
    let bounds = scene.bounds(slot);
    if bounds.is_empty() {
        return None;
    }
    let point = bounds.center();
    view.project(point).is_some_and(|px| view.contains(px)).then_some(point)
}

impl Running {
    /// A turn of the view is beginning: picks the point it goes about and
    /// brings the camera's pivot level with it. `stroke` lets the last place
    /// sculpted be that point (Stroke Pivot); the view gizmo and the axis
    /// views are not about any one place on the model, and pass false.
    fn turn_begin(&mut self, pointer: Option<Vec2>, stroke: bool) -> Option<Vec3> {
        // The camera may have moved since the last frame was drawn.
        self.sync_view();
        let stroke = if stroke { self.anchor() } else { None };
        let about = stroke.or_else(|| turn_point(&self.view, &self.doc.scene, &self.doc.mesh, pointer));
        if let Some(point) = about {
            self.doc.camera.seat_pivot(point);
        }
        about
    }

    /// A press on the view became an orbit, a pan or a zoom.
    pub(super) fn navigation_began(&mut self) {
        // The second click of a double-click has just asked for the model to
        // be framed: that is all it does.
        if self.doc.camera.glide_is_new() {
            self.input.drag = Drag::Ui;
            return;
        }
        self.doc.camera.cancel_glide();
        if self.input.drag == Drag::Orbit {
            self.input.turn_about = self.turn_begin(Some(self.input.pointer), true);
        }
    }

    /// The pointer moved by `delta` pixels during an orbit drag.
    pub(super) fn turn_by(&mut self, delta: Vec2) {
        self.leave_axis_view(delta);
        self.doc.camera.orbit(delta, self.input.turn_about);
    }

    /// A drag on the view gizmo, which turns the view as a drag on the
    /// background does. `begins` on the first move of each drag.
    pub(super) fn turn_from_gizmo(&mut self, by: Vec2, begins: bool) {
        if begins {
            self.input.turn_about = self.turn_begin(None, false);
        }
        self.leave_axis_view(by);
        self.doc.camera.orbit(by, self.input.turn_about);
    }

    /// A turn by hand out of an axis view goes back to perspective. The
    /// pivot was seated when the turn began, so nothing changes size.
    fn leave_axis_view(&mut self, by: Vec2) {
        let camera = &mut self.doc.camera;
        if camera.ortho_from_axis && by != Vec2::ZERO {
            camera.ortho = false;
            camera.ortho_from_axis = false;
        }
    }

    /// Turns to an axis view, which is orthographic.
    pub(super) fn snap_view(&mut self, yaw: f32, pitch: f32) {
        // Seated while the view is still as it was: orthographic shows at
        // their true size the things level with the pivot.
        let about = self.turn_begin(None, false);
        let camera = &mut self.doc.camera;
        // Already orthographic by choice: it stays a choice.
        camera.ortho_from_axis = !camera.ortho || camera.ortho_from_axis;
        camera.ortho = true;
        camera.snap_to(yaw, pitch, about);
    }

    /// The Ortho / Persp button. The pivot is seated on the model first, as
    /// for an axis view: orthographic shows the things level with the pivot
    /// at the size they are, and a pivot left behind the model by a pan or a
    /// zoom used to make the model jump in size at the press.
    pub(super) fn toggle_ortho(&mut self) {
        self.turn_begin(None, false);
        let camera = &mut self.doc.camera;
        camera.ortho = !camera.ortho;
        camera.ortho_from_axis = false;
    }
}

impl Running {
    /// `--demo-tool <tool>:turn`: leaves the pivot well behind the model, as
    /// a working session does, then turns the view each way it can be turned
    /// by hand (a drag, an axis view, the view gizmo) by way of the pointer
    /// routines, and frames the model with a double-click. Prints `ok` /
    /// `FAIL` for what must then be true of where the model is on screen.
    pub(super) fn turn_demo(&mut self) {
        use crate::doc::Action;
        use winit::event::MouseButton;
        use winit::keyboard::ModifiersState;

        fn say(what: &str, ok: bool) {
            println!("{} {what}", if ok { "ok  " } else { "FAIL" });
        }
        self.tools_leave();
        self.doc.tool = crate::doc::Tool::Sculpt;
        self.input.inside = true;
        self.doc.camera.update(10.0);
        self.sync_view();
        let bounds = self.doc.scene.bounds(&self.doc.mesh);
        let (middle, radius) = (bounds.center(), bounds.size().max_element() * 0.5);
        let height = self.view.size.y;
        // Where the model's middle is on screen, and how big the model is there.
        let place = |run: &mut Running| {
            run.sync_view();
            let view = &run.view;
            let at = view.project(middle).unwrap_or(Vec2::splat(-1e6));
            (at, view.project(middle + view.up * radius).map_or(0.0, |edge| edge.distance(at)))
        };
        // The pivot six model-widths behind the model, the picture as it was.
        let leave_pivot_behind = |run: &mut Running| {
            run.sync_view();
            let camera = &mut run.doc.camera;
            camera.pivot += run.view.forward * (radius * 12.0);
            if !camera.ortho {
                camera.distance += radius * 12.0;
            }
        };
        let stayed = |was: (Vec2, f32), now: (Vec2, f32)| {
            now.0.distance(was.0) < 0.02 * height && (now.1 / was.1.max(1e-6) - 1.0).abs() < 0.02
        };
        let centre = self.view.size * 0.5;

        // A drag on the view.
        leave_pivot_behind(self);
        let (was, yaw) = (place(self), self.doc.camera.yaw);
        self.input.mods = ModifiersState::ALT;
        self.pointer_moved(centre, None);
        self.pointer_pressed(MouseButton::Left, None);
        say("Alt and a press turn the view", self.input.drag == Drag::Orbit);
        say("about a point inside the model", self.input.turn_about.is_some_and(|p| p.distance(middle) < radius));
        for i in 1..=40 {
            self.pointer_moved(centre + Vec2::new(i as f32 * 12.0, i as f32 * 2.0), None);
        }
        self.pointer_released(MouseButton::Left);
        self.input.mods = ModifiersState::empty();
        say("the view turned a third of the way round", (self.doc.camera.yaw - yaw - 120.0).abs() < 1.0);
        let now = place(self);
        println!("     the model was at {:.0?}, {:.0} px tall; it is at {:.0?}, {:.0} px", was.0, was.1, now.0, now.1);
        say("the model is where it was, the size it was", stayed(was, now));

        // The view gizmo.
        leave_pivot_behind(self);
        let was = place(self);
        for i in 0..20 {
            self.act(Action::Turn { by: [-18.0, 5.0], begins: i == 0 });
        }
        say("a drag on the view gizmo leaves it there too", stayed(was, place(self)));

        // An axis view, which also goes orthographic.
        leave_pivot_behind(self);
        let was = place(self);
        self.act(Action::Snap(-90.0, 0.0));
        while self.doc.camera.update(0.05) {}
        say("an axis view turned the view", self.doc.camera.yaw == 270.0 && self.doc.camera.ortho);
        let now = place(self);
        println!("     the model was at {:.0?}, {:.0} px tall; it is at {:.0?}, {:.0} px", was.0, was.1, now.0, now.1);
        say("and the model is where it was, the size it was", stayed(was, now));

        // A turn by hand out of the axis view goes back to perspective, at the same size.
        leave_pivot_behind(self);
        let was = place(self);
        self.act(Action::Turn { by: [6.0, 0.0], begins: true });
        say("turning out of the axis view goes back to perspective", !self.doc.camera.ortho);
        say("and the model stays the size it was", stayed(was, place(self)));

        // The Ortho / Persp button, with the pivot left behind the model: both ways, the same size.
        for ortho in [true, false] {
            leave_pivot_behind(self);
            let was = place(self);
            self.act(Action::ToggleOrtho);
            say(&format!("the button turns orthographic {}", if ortho { "on" } else { "off" }), self.doc.camera.ortho == ortho);
            say("and the model stays the size it was", stayed(was, place(self)));
        }
        // Orthographic by the button stays on through a turn.
        self.act(Action::ToggleOrtho);
        self.act(Action::Turn { by: [6.0, 0.0], begins: true });
        say("orthographic chosen with the button stays on through a turn", self.doc.camera.ortho);
        self.act(Action::ToggleOrtho);

        // A double-click on the background, with the model off to one side.
        self.sync_view();
        self.doc.camera.pivot += self.view.right * (radius * 0.9);
        let (at, size) = place(self);
        let spot = [(0.0, -1.0), (0.0, 1.0), (1.0, 0.0), (-1.0, 0.0), (0.7, -0.7), (-0.7, -0.7), (0.7, 0.7), (-0.7, 0.7)]
            .into_iter()
            .map(|(x, y)| at + Vec2::new(x, y) * size * 1.5)
            .find(|&px| {
                self.input.pointer = px;
                self.view.contains(px) && self.pick(px).is_none() && !self.over_ui()
            });
        let Some(spot) = spot else { return say("there is background beside the model to click on", false) };
        self.pointer_moved(spot, None);
        for click in 0..2 {
            self.pointer_pressed(MouseButton::Left, None);
            if click == 1 {
                say("the second click on the background frames the model", self.doc.camera.is_animating());
                say("and is not a turn as well", self.input.drag == Drag::Ui);
                // The hand is never quite still between the press and the release.
                self.pointer_moved(spot + Vec2::new(2.0, 1.0), None);
            }
            self.pointer_released(MouseButton::Left);
        }
        while self.doc.camera.update(0.05) {}
        let now = place(self);
        say("the model is back in the middle of the view", now.0.distance(centre) < 0.02 * height);

        // The gizmo's hand and magnifier, then Home.
        let (was, distance) = (place(self), self.doc.camera.distance);
        self.act(Action::PanView([40.0, 0.0]));
        let now = place(self);
        println!("     the model was at {:.1?}; after a 40 px pan it is at {:.1?}", was.0, now.0);
        // As a middle drag: the surface in front follows the pen pixel for pixel, so the
        // model's middle, further back, goes the same way a little less far.
        let moved = now.0 - was.0;
        say("the hand pans: the model follows it", moved.x > 20.0 && moved.x < 41.0 && moved.y.abs() < 2.0);
        self.act(Action::ZoomView(-60.0));
        say("dragging the magnifier up comes nearer", self.doc.camera.distance < distance);
        self.act(Action::ToggleOrtho);
        self.act(Action::HomeView);
        while self.doc.camera.update(0.05) {}
        let camera = &self.doc.camera;
        say("Home turns to the starting angle, in perspective", (camera.yaw, camera.pitch) == (25.0, 15.0) && !camera.ortho);
        say("with the scene in the middle of the view", place(self).0.distance(centre) < 0.02 * height);

        // The pointer is left out of the picture.
        self.input.inside = false;
        self.sync_view();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::Camera;
    use crate::mesh::primitives::{self, Primitive};

    const SIZE: Vec2 = Vec2::new(1600.0, 900.0);

    fn view_of(camera: &Camera) -> View {
        camera.view(Vec2::ZERO, SIZE, camera.distance, 1.0)
    }

    /// A ball at the origin, and a camera that frames it.
    fn ball(ortho: bool) -> (Scene, Mesh, Camera) {
        let mesh = primitives::build(Primitive::Sphere, 4000, 0.5);
        let mut camera = Camera::default();
        camera.frame(mesh.bounds(), SIZE.x / SIZE.y, false);
        camera.ortho = ortho;
        (Scene::new("Ball"), mesh, camera)
    }

    /// How far the ball's middle is from the middle of the view, in view
    /// heights (0.5 is the top edge), or `None` with it behind the eye.
    fn off_centre(camera: &Camera) -> Option<f32> {
        let px = view_of(camera).project(Vec3::ZERO)?;
        Some((px - SIZE * 0.5).length() / SIZE.y)
    }

    /// A third of the way round, in the moves of a drag.
    fn turn(camera: &mut Camera, about: Option<Vec3>) {
        for _ in 0..48 {
            camera.orbit(Vec2::new(10.0, 2.0), about);
        }
    }

    /// The same as a drag on the view makes it: the point is chosen once,
    /// the pivot seated, then the moves made.
    fn drag_round(scene: &Scene, mesh: &Mesh, camera: &mut Camera) {
        let about = turn_point(&view_of(camera), scene, mesh, None);
        if let Some(point) = about {
            camera.seat_pivot(point);
        }
        turn(camera, about);
    }

    #[test]
    fn a_framed_model_turns_about_its_own_middle() {
        for ortho in [false, true] {
            let (scene, mesh, mut camera) = ball(ortho);
            let point = turn_point(&view_of(&camera), &scene, &mesh, None).expect("the ball is in view");
            assert!(point.length() < 0.01, "the turn point {point} is not the middle of the ball");
            drag_round(&scene, &mesh, &mut camera);
            assert!(off_centre(&camera).unwrap() < 0.01, "the ball left the middle of the view");
        }
    }

    /// The fault this file is for: the pivot left well behind the model, as
    /// stroke-pivot turns and pans leave it.
    #[test]
    fn a_pivot_left_off_the_model_no_longer_throws_it_out_of_view() {
        for ortho in [false, true] {
            let drifted = || {
                let (scene, mesh, mut camera) = ball(ortho);
                let forward = camera.rotation() * Vec3::NEG_Z;
                camera.pivot += forward * 1.5;
                if !ortho {
                    // The eye stays where it was.
                    camera.distance += 1.5;
                }
                (scene, mesh, camera)
            };
            let (scene, mesh, mut camera) = drifted();
            assert!(off_centre(&camera).unwrap() < 0.01, "the ball starts in the middle of the view");

            // About the pivot, as every turn used to be: the ball swings away.
            let mut old = drifted().2;
            turn(&mut old, None);
            assert!(off_centre(&old).is_none_or(|d| d > 0.2), "the old turn kept the ball in place after all");

            drag_round(&scene, &mesh, &mut camera);
            assert!(off_centre(&camera).unwrap() < 0.01, "the ball left the middle of the view (ortho {ortho})");
        }
    }

    #[test]
    fn seating_the_pivot_leaves_the_picture_alone() {
        for ortho in [false, true] {
            let mut camera = ball(ortho).2;
            (camera.yaw, camera.pitch) = (40.0, -25.0);
            let marks = [Vec3::new(0.2, 0.1, -0.15), Vec3::new(-0.25, 0.0, 0.2), Vec3::ZERO];
            let before: Vec<Vec2> = marks.iter().map(|&m| view_of(&camera).project(m).unwrap()).collect();
            let at = Vec3::new(0.3, -0.2, 0.25);
            camera.seat_pivot(at);
            let view = view_of(&camera);
            for (mark, was) in marks.iter().zip(before) {
                assert!(view.project(*mark).unwrap().distance(was) < 0.01, "a point moved on screen (ortho {ortho})");
            }
            let forward = camera.rotation() * Vec3::NEG_Z;
            assert!((at - camera.pivot).dot(forward).abs() < 1e-5, "the pivot is not level with the point");
            // The pivot is still the middle of the picture.
            assert!(view.project(camera.pivot).unwrap().distance(SIZE * 0.5) < 0.01);
        }
    }

    #[test]
    fn a_model_off_to_one_side_is_still_found_and_stays_where_it_is() {
        let (scene, mesh, mut camera) = ball(false);
        let radius = mesh.bounds().size().x * 0.5;
        // Panned until the ball is clear of the middle of the view.
        camera.pivot += camera.rotation() * Vec3::X * (radius * 1.5);
        let view = view_of(&camera);
        let (origin, dir) = view.ray(SIZE * 0.5);
        assert!(scene.pick(origin, dir, &mesh).is_none(), "the middle of the view is meant to be empty");
        let was = view.project(Vec3::ZERO).unwrap();
        let point = turn_point(&view, &scene, &mesh, None).expect("the ball is in view, off to the side");
        assert!(point.length() < radius, "the turn point {point} is not in the ball");
        drag_round(&scene, &mesh, &mut camera);
        let now = view_of(&camera).project(Vec3::ZERO).expect("the ball went behind the eye");
        assert!(now.distance(was) < 0.25 * SIZE.y, "the ball moved from {was} to {now}");
    }

    #[test]
    fn a_close_up_turns_about_what_is_in_view_not_the_middle_of_the_object() {
        // A long bar seen from the side, close in on a place near one end.
        let mesh = primitives::build(Primitive::Cube, 2000, 0.2);
        let mut scene = Scene::new("Bar");
        scene.active_mut().scale = Vec3::new(10.0, 1.0, 1.0);
        let place = Vec3::new(mesh.bounds().size().x * 4.0, 0.0, 0.0);
        let mut camera = Camera::default();
        (camera.pivot, camera.distance) = (place, 0.6);
        let point = turn_point(&view_of(&camera), &scene, &mesh, None).expect("the bar is in view");
        assert!((point - place).length() < 0.02, "the turn point is {point}, not {place}");
    }

    #[test]
    fn an_axis_view_turns_about_the_point_it_is_given() {
        let mut camera = ball(false).2;
        let about = Vec3::new(0.2, 0.1, 0.0);
        camera.pivot = Vec3::new(1.0, 0.5, 0.3);
        let was = view_of(&camera).project(about).unwrap();
        camera.snap_to(-90.0, 0.0, Some(about));
        // Saved half way, the view is the one the turn is going to.
        camera.update(0.1);
        let saved = camera.saved_view();
        while camera.update(0.05) {}
        assert_eq!((camera.yaw, camera.pitch), (270.0, 0.0));
        assert!(view_of(&camera).project(about).unwrap().distance(was) < 0.5, "the point left its pixel");
        assert!((Vec3::from_array(saved.pivot) - camera.pivot).length() < 1e-3);
    }

    #[test]
    fn an_empty_view_has_no_turn_point() {
        let (scene, mesh, mut camera) = ball(false);
        camera.pivot += camera.rotation() * Vec3::X * 50.0;
        assert_eq!(turn_point(&view_of(&camera), &scene, &mesh, None), None);
    }
}
