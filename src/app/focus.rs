//! What the window does when it loses focus (Alt-Tab, a click on another
//! program, a notification stealing the keyboard).
//!
//! The release of a button or a key that happens while another window is in
//! front is never delivered to this one. Without this, a stroke begun just
//! before the Alt-Tab goes on following the pointer, `Action::Undo` is
//! refused for as long as the sculptor believes a stroke is running, and
//! Alt, S, F, D or Space stay "held" until the next click or keypress sorts
//! them out. So losing focus lets go of everything, the way a release at the
//! last known pointer position would: a stroke is finished and becomes its
//! one undo step (not thrown away), a gizmo or tool drag ends, and nothing is
//! left down.
//!
//! `--demo-tool <tool>:focus` runs the same thing in a real window for a look
//! (see [`Running::focus_demo`]). The pointer leaving the window with a
//! button down needs nothing here: Windows keeps the pointer captured for
//! the app until the button is up, so the moves and the release still arrive.

use glam::Vec2;
use winit::event::MouseButton;
use winit::keyboard::{KeyCode, ModifiersState};

use super::{Drag, Input, Running};
use crate::camera::View;
use crate::doc::{Action, Doc};
use crate::sculpt::Brush;

/// Finishes the brush stroke in progress: every queued pointer sample is
/// fed to it, and what it changed becomes one undo step.
pub(super) fn end_stroke(input: &mut Input, doc: &mut Doc, local: &View) {
    for s in input.samples.drain(..) {
        doc.sculptor.sample(&mut doc.mesh, local, s);
    }
    if let Some(delta) = doc.sculptor.end(&mut doc.mesh, local) {
        doc.history.push_stroke(delta);
    }
}

impl Input {
    /// Remembers a key going down or coming up.
    pub(super) fn note_key(&mut self, code: KeyCode, down: bool) {
        self.held.retain(|&k| k != code);
        if down {
            self.held.push(code);
        }
    }
}

/// Lets go of everything the pointer and keyboard hold that needs only the
/// document: the brush stroke is finished, no drag or button is left, no
/// modifier, pen or key-and-drag gauge is still on. Returns the keys that
/// were down, for the caller to release in the places that watch them.
pub(super) fn let_go(input: &mut Input, doc: &mut Doc, local: &View) -> Vec<KeyCode> {
    if input.drag == Drag::Sculpt {
        end_stroke(input, doc, local);
    }
    input.drag = Drag::None;
    input.button = None;
    input.samples.clear();
    input.mods = ModifiersState::empty();
    input.pen_down = false;
    input.pen_pressure = None;
    input.gauge = None;
    std::mem::take(&mut input.held)
}

impl Running {
    /// The window lost focus: ends every drag as a release at the last known
    /// pointer position, and puts every held button, modifier and key down.
    pub(super) fn focus_lost(&mut self) {
        // The drags that need the whole app end first, each only if it holds
        // one; the tool's pending click is dropped rather than made.
        self.spheres_released();
        self.transform_released();
        self.tools_focus_lost();
        let stroke = self.input.drag == Drag::Sculpt;
        let keys = let_go(&mut self.input, &mut self.doc, &self.local);
        if stroke {
            self.stroke_ended = Some(std::time::Instant::now());
            // As `finish_stroke` does: a stroke is held to leaving the
            // surface no more open than it found it, however it ended.
            self.stroke_check();
        }
        // The same releases the keyboard would have delivered.
        for code in keys {
            self.key_released(code);
        }
        self.ui.radial = None;
        self.redraw = true;
    }
}

impl Running {
    /// `--demo-tool <tool>:focus`: begins a stroke with Alt, Shift and S
    /// held, loses the focus mid-stroke, moves the pointer on, and prints
    /// `ok` / `FAIL` for what must then be true. The same is done with a
    /// camera drag and a drag that a hosted tool took. The picture shows one
    /// short stroke on the ball, not a smear after the pointer.
    pub(super) fn focus_demo(&mut self, index: usize) {
        fn say(what: &str, ok: bool) {
            println!("{} {what}", if ok { "ok  " } else { "FAIL" });
        }
        self.sync_view();
        self.input.inside = true;
        self.doc.camera.update(10.0);
        self.sync_view();
        let centre = self.view.size * 0.5;
        let r = self.view.size.y * 0.1;

        // A hosted tool's drag: the press goes to the tool, then the focus goes.
        self.tools_activate(index);
        self.pointer_moved(centre, None);
        self.pointer_pressed(MouseButton::Left, None);
        let taken = self.tools_holds_drag();
        println!("     the tool {} the press", if taken { "took" } else { "passed on" });
        self.pointer_moved(centre + Vec2::new(r * 0.5, 0.0), None);
        self.focus_lost();
        say("no tool drag is left after the focus went", !self.tools_holds_drag());
        say("no button or drag is left (tool)", self.input.button.is_none() && self.input.drag == Drag::None);
        self.tools_leave();
        self.doc.tool = crate::doc::Tool::Sculpt;

        // The camera: Alt and a drag, then the focus goes.
        self.input.mods = ModifiersState::ALT;
        self.pointer_moved(centre, None);
        self.pointer_pressed(MouseButton::Left, None);
        say("Alt and a press orbit", self.input.drag == Drag::Orbit);
        self.pointer_moved(centre + Vec2::new(30.0, 0.0), None);
        let yaw = self.doc.camera.yaw;
        self.focus_lost();
        self.pointer_moved(centre + Vec2::new(200.0, 40.0), None);
        say("the camera stops turning once the focus went", self.doc.camera.yaw == yaw);
        say("Alt is not held any more", self.input.mods.is_empty() && self.input.drag == Drag::None);
        self.doc.camera.update(10.0);
        self.sync_view();

        // A stroke with the modifiers and a gauge key held.
        let (rest, steps) = (self.doc.mesh.positions.clone(), self.doc.history.steps().0);
        let settings = &mut self.doc.sculptor.settings;
        settings.brush = Brush::Inflate;
        settings.radius_px = r * 0.6;
        self.input.mods = ModifiersState::empty();
        self.pointer_moved(centre - Vec2::new(r, 0.0), None);
        self.sync_view();
        self.pointer_pressed(MouseButton::Left, None);
        say("a press on the ball begins a stroke", self.input.drag == Drag::Sculpt);
        for i in 1..=10 {
            self.pointer_moved(centre + Vec2::new((i as f32 - 5.0) * r * 0.2, 0.0), None);
        }
        self.input.mods = ModifiersState::ALT | ModifiersState::SHIFT;
        self.input.note_key(KeyCode::KeyS, true);
        self.input.gauge = Some(super::Gauge {
            kind: super::GaugeKind::Size,
            anchor: self.input.pointer,
            start: self.doc.sculptor.settings.size(),
        });
        self.focus_lost();
        let (after, steps_after) = (self.doc.mesh.positions.clone(), self.doc.history.steps().0);
        say("the stroke is over", !self.doc.sculptor.is_active());
        say("it changed the ball", after != rest);
        say("as one undo step", steps_after == steps + 1);
        say("no button, drag, modifier, key or gauge is held", {
            let i = &self.input;
            i.button.is_none() && i.drag == Drag::None && i.mods.is_empty() && i.held.is_empty() && i.gauge.is_none()
        });
        for i in 1..=20 {
            self.pointer_moved(centre + Vec2::new(r, 0.0) + Vec2::new(i as f32 * 8.0, i as f32 * 5.0), None);
        }
        self.frame_sculpt_for_demo();
        say("later pointer moves change no vertices", self.doc.mesh.positions == after);

        self.act(Action::Undo);
        say("Undo works at once and puts the ball back", self.doc.mesh.positions == rest);
        self.act(Action::Redo);
        say("Redo brings the stroke back", self.doc.mesh.positions == after);

        // Back from the other window: a new press starts a clean stroke.
        self.pointer_moved(centre + Vec2::new(r * 1.4, -r * 0.9), None);
        self.pointer_pressed(MouseButton::Left, None);
        say("the next press starts a stroke", self.input.drag == Drag::Sculpt);
        self.pointer_released(MouseButton::Left);
        say("which ends with its release", !self.doc.sculptor.is_active());
        self.act(Action::Undo);
        say("and Undo takes only that one", self.doc.mesh.positions == after);

        // The brush ring is where the pointer last was; leave it out of the picture.
        self.input.inside = false;
        self.sync_view();
    }

    /// What the sculptor does on a frame, so the demo can see whether any
    /// stroke is still taking pointer samples.
    fn frame_sculpt_for_demo(&mut self) {
        let doc = &mut self.doc;
        doc.sculptor.frame(&mut doc.mesh, &self.local, 1.0 / 60.0);
        let _ = doc.mesh.take_dirty();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::Camera;
    use crate::mesh::primitives::{self, Primitive};
    use crate::scene::Scene;
    use crate::sculpt::PointerSample;

    const SIZE: Vec2 = Vec2::new(1600.0, 900.0);

    /// A document with a ball in it, and the view onto it.
    fn ball() -> (Doc, View) {
        let mesh = primitives::build(Primitive::Sphere, 4000, 0.5);
        let mut camera = Camera::default();
        camera.frame(mesh.bounds(), SIZE.x / SIZE.y, false);
        let view = camera.view(Vec2::ZERO, SIZE, camera.distance, 1.0);
        let scene = Scene::new("Ball");
        let mut doc = Doc {
            mesh,
            scene,
            mesh_replaced: true,
            history: Default::default(),
            sculptor: Default::default(),
            camera,
            opts: Default::default(),
            path: None,
            stats: Default::default(),
            tool: crate::doc::Tool::Sculpt,
            spheres: Default::default(),
            tools: Default::default(),
            turntable: Default::default(),
            timelapse: Default::default(),
        };
        doc.history.target = doc.scene.active_mesh();
        (doc, view)
    }

    fn sample(pos: Vec2, time: f64) -> PointerSample {
        PointerSample { pos, pressure: None, time }
    }

    /// What `pointer_pressed` and `pointer_moved` do for a stroke, minus the
    /// window: the press begins it on the model, each move queues a sample.
    fn start_stroke(input: &mut Input, doc: &mut Doc, view: &View) -> Vec2 {
        let centre = SIZE * 0.5;
        doc.sculptor.settings.brush = Brush::Inflate;
        doc.sculptor.settings.radius_px = 80.0;
        assert!(doc.begin_stroke(view, sample(centre, 0.0), false, false), "the press missed the ball");
        input.button = Some(MouseButton::Left);
        input.drag = Drag::Sculpt;
        input.samples.clear();
        let mut at = centre;
        for i in 1..=12 {
            at = centre + Vec2::new(i as f32 * 6.0, 0.0);
            input.samples.push(sample(at, i as f64 / 120.0));
        }
        at
    }

    #[test]
    fn losing_focus_mid_stroke_ends_it_as_one_undo_step() {
        let (mut doc, view) = ball();
        let mut input = Input::default();
        let rest = doc.mesh.positions.clone();
        let last = start_stroke(&mut input, &mut doc, &view);
        input.mods = ModifiersState::ALT | ModifiersState::SHIFT;
        input.pen_down = true;
        input.pen_pressure = Some(0.7);
        for code in [KeyCode::KeyS, KeyCode::Space] {
            input.note_key(code, true);
        }
        assert!(doc.sculptor.is_active());

        let keys = let_go(&mut input, &mut doc, &view);

        assert!(!doc.sculptor.is_active(), "the stroke is still running, so Undo would be refused");
        assert_eq!(doc.history.steps(), (1, 0), "the stroke must be exactly one undo step");
        let after = doc.mesh.positions.clone();
        assert_ne!(after, rest, "the stroke was thrown away instead of finished");
        assert_eq!((input.drag, input.button), (Drag::None, None), "a drag or button is still down");
        assert!(input.samples.is_empty());
        assert!(input.mods.is_empty(), "a modifier is still held");
        assert!(!input.pen_down && input.pen_pressure.is_none(), "the pen is still down");
        assert!(input.gauge.is_none());
        assert!(input.held.is_empty());
        assert_eq!(keys, [KeyCode::KeyS, KeyCode::Space], "the held keys are handed back to be released");

        // The pointer keeps moving over the window it left: nothing follows it.
        let mut later = last;
        for i in 1..=10 {
            later += Vec2::new(5.0, 3.0 * i as f32);
            if input.drag == Drag::Sculpt {
                input.samples.push(sample(later, 1.0 + i as f64 / 120.0));
            }
        }
        assert!(input.samples.is_empty());
        assert_eq!(doc.mesh.positions, after, "the pointer moved vertices after the focus was lost");

        // Undo works straight away and puts the ball back exactly.
        assert_eq!(doc.history.undo(&mut doc.mesh).map(|_| ()), Some(()));
        assert_eq!(doc.mesh.positions, rest);

        // And the next press starts a clean stroke.
        assert!(doc.begin_stroke(&view, sample(SIZE * 0.5, 2.0), false, false));
        doc.sculptor.end(&mut doc.mesh, &view);
    }

    #[test]
    fn losing_focus_with_nothing_held_changes_nothing() {
        let (mut doc, view) = ball();
        let mut input = Input::default();
        let rest = doc.mesh.positions.clone();
        assert!(let_go(&mut input, &mut doc, &view).is_empty());
        assert_eq!(doc.history.steps(), (0, 0));
        assert_eq!(doc.mesh.positions, rest);
    }

    #[test]
    fn a_held_key_is_remembered_once_and_forgotten_when_it_comes_up() {
        let mut input = Input::default();
        input.note_key(KeyCode::KeyS, true);
        input.note_key(KeyCode::KeyS, true);
        input.note_key(KeyCode::KeyF, true);
        assert_eq!(input.held, [KeyCode::KeyS, KeyCode::KeyF]);
        input.note_key(KeyCode::KeyS, false);
        assert_eq!(input.held, [KeyCode::KeyF]);
    }
}
