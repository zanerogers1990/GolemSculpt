//! The app's side of the SSphere tool: routing the pointer, wheel and keys to
//! it while it is up, keeping its drawing current, and turning its skin into
//! a scene object.

use std::time::Instant;

use glam::{Quat, Vec2, Vec3};
use winit::event::MouseButton;
use winit::keyboard::{KeyCode, ModifiersState};

use super::{DOUBLE_CLICK, Drag, Running};
use crate::camera;
use crate::doc::{Action, Tool};
use crate::gizmo::{Handle, Handles, Target};
use crate::mesh::Mesh;
use crate::render::spheres::SkinLook;
use crate::ssphere::{Body, Ctx, EditMode, Hit, Press, SSpheres};

impl Running {
    pub(super) fn spheres_active(&self) -> bool {
        self.doc.tool == Tool::SSpheres
    }

    /// Runs `f` on the tool with what its gestures need from the app.
    fn with_spheres<R>(&mut self, f: impl FnOnce(&mut SSpheres, &Ctx) -> R) -> R {
        let doc = &mut self.doc;
        let (scene, mesh) = (&doc.scene, &doc.mesh);
        // A gesture must see the rig where its object is now, not where it
        // was when the last frame was drawn.
        doc.spheres.follow(|id| scene.get(id).map(Body::of));
        let surface = |origin: Vec3, dir: Vec3| {
            let hit = scene.pick(origin, dir, mesh)?;
            Some(Hit { point: hit.pos, normal: hit.normal, body: Body::of(scene.get(hit.id)?) })
        };
        let body = Some(Body::of(scene.active()));
        let cx = Ctx { view: &self.view, pivot: doc.camera.pivot, surface: &surface, body };
        f(&mut doc.spheres, &cx)
    }

    /// The first sphere of a rig is sized off what is already in the scene.
    fn spheres_size_from_scene(&mut self) {
        let scene = self.doc.scene.bounds(&self.doc.mesh);
        if !scene.is_empty() {
            self.doc.spheres.default_radius = ((scene.size() * 0.5).element_sum() / 3.0 * 0.35).clamp(0.02, 1.0);
        }
    }

    /// New SSphere Rig: brings the tool up and places the first sphere where
    /// the camera is looking, on the selected object's mirror plane, resting
    /// on the object if it is in the way (and then attached to it). Never
    /// clears spheres that are already there.
    pub(super) fn spheres_new_rig(&mut self) {
        self.set_tool(Tool::SSpheres);
        self.sync_view();
        self.spheres_size_from_scene();
        let time = self.time();
        let was_attached = self.doc.spheres.attached_to();
        let started = self.with_spheres(|s, cx| {
            // The tool may have been away: its clock times the note below.
            s.tick(time);
            s.start_new_rig(cx)
        });
        let doc = &mut self.doc;
        let landed_on = doc.spheres.attached_to().filter(|&id| was_attached != Some(id));
        let text = match (started, landed_on.and_then(|id| doc.scene.get(id))) {
            (false, _) => "There are spheres already: drag off one, or use Clear All to start over.".to_owned(),
            (true, Some(model)) => format!(
                "Started a rig with one sphere, resting on \"{}\" and attached to it. Drag off the sphere to pull out the next one.",
                model.name
            ),
            (true, None) => "Started a rig with one sphere. Drag off it to pull out the next one.".to_owned(),
        };
        doc.spheres.say(text);
    }

    pub(super) fn set_tool(&mut self, tool: Tool) {
        if self.doc.tool == tool {
            return;
        }
        self.doc.spheres.leave();
        self.transform_leave();
        self.input.gauge = None;
        self.ui.radial = None;
        self.doc.tool = tool;
        self.toast(match tool {
            Tool::Sculpt => "Sculpt",
            Tool::Transform => "Transform",
            Tool::SSpheres => "SSpheres",
            // A hosted tool is announced by name where it is brought up (app/tools.rs).
            Tool::Plugin => return,
        });
    }

    /// What the transform gizmo sits on while the tool is up: arrows on the
    /// selected sphere in Move mode (see ssphere/gizmo.rs for why only
    /// arrows). The gizmo's own routines take it from here.
    pub(super) fn spheres_gizmo(&self) -> Option<(Target, Handles)> {
        Some(self.doc.spheres.gizmo()?.target())
    }

    /// Whether the pointer is on a sphere, which keeps a press even where an
    /// arrow of the gizmo is drawn over it.
    pub(super) fn spheres_claims_pointer(&mut self, pos: Vec2) -> bool {
        self.with_spheres(|s, cx| s.claims_pointer(cx, pos))
    }

    /// A button went down on the viewport while the tool is up.
    pub(super) fn spheres_pressed(&mut self, button: MouseButton) -> Drag {
        self.sync_view();
        // An arrow of the gizmo on the selected sphere wins the press, as the
        // gizmo's handles do everywhere (unless a sphere is in the way).
        if button == MouseButton::Left && self.gizmo_pressed() {
            return Drag::Gizmo;
        }
        let mods = self.input.mods;
        let pos = self.input.pointer;
        let delete = button == MouseButton::Right || mods.control_key();
        match self.with_spheres(|s, cx| s.press(cx, pos, mods.shift_key(), delete)) {
            Press::Drag => Drag::Spheres,
            // Empty space: the camera orbits, unless it turns out to be a click.
            Press::Empty if button == MouseButton::Left => Drag::Orbit,
            Press::Empty | Press::Done => Drag::Ui,
        }
    }

    /// The pointer moved. True when the tool used the movement, so nothing
    /// else should.
    pub(super) fn spheres_moved(&mut self, pos: Vec2) -> bool {
        if !self.spheres_active() {
            return false;
        }
        if self.doc.spheres.gauge_move(pos) {
            return true;
        }
        matches!(self.input.drag, Drag::Spheres | Drag::Orbit) && self.with_spheres(|s, cx| s.drag_to(cx, pos))
    }

    pub(super) fn spheres_released(&mut self) {
        if self.spheres_active() && matches!(self.input.drag, Drag::Spheres | Drag::Orbit) {
            self.with_spheres(|s, cx| s.release(cx));
        }
    }

    /// True when the tool is up: over a sphere the wheel resizes it, anywhere
    /// else it zooms (never the brush, which is put away).
    pub(super) fn spheres_wheel(&mut self, lines: f32) -> bool {
        if !self.spheres_active() {
            return false;
        }
        if !self.doc.spheres.wheel(lines) {
            let factor = 1.0 - lines.signum() * camera::WHEEL_ZOOM_STEP;
            self.doc.camera.zoom(factor, self.view_depth, self.scene_size, None);
        }
        true
    }

    /// True when the key was the tool's. Ctrl shortcuts stay the app's.
    pub(super) fn spheres_key_pressed(&mut self, code: KeyCode) -> bool {
        if !self.spheres_active() || self.input.mods.control_key() {
            return false;
        }
        let idle = self.input.drag == Drag::None;
        let modes = [KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3, KeyCode::Digit4];
        if let Some(i) = modes.iter().position(|&k| k == code) {
            if idle {
                self.doc.spheres.set_mode(EditMode::ALL[i]);
                self.toast(EditMode::ALL[i].name());
            }
            return true;
        }
        match code {
            KeyCode::KeyS if idle => self.doc.spheres.gauge_begin(self.input.pointer),
            KeyCode::KeyF if idle => {
                // F F frames, as when sculpting; a lone F has no meaning here.
                let now = Instant::now();
                if self.input.last_f.is_some_and(|t| now - t < DOUBLE_CLICK) {
                    self.input.last_f = None;
                    self.frame_view();
                } else {
                    self.input.last_f = Some(now);
                }
            }
            KeyCode::KeyA if idle => {
                let s = &mut self.doc.spheres;
                s.preview = !s.preview;
                let on = s.preview;
                self.toast(if on { "Solid Preview" } else { "Spheres" });
            }
            KeyCode::KeyX if idle => {
                let s = &mut self.doc.spheres;
                let on = !s.symmetry();
                let report = s.set_symmetry(on);
                s.say(report);
                self.toast(if on { "Symmetry On" } else { "Symmetry Off" });
            }
            KeyCode::Delete | KeyCode::Backspace if idle => {
                self.doc.spheres.delete_selected();
            }
            // Mid-drag: put the rig back. Otherwise: let go of the selection,
            // and failing that leave Esc to the app.
            KeyCode::Escape => {
                let taken = self.doc.spheres.escape();
                // A drag of the gizmo's arrow is cancelled with the rest.
                self.transform_cancelled();
                return taken;
            }
            // The brush keys have nothing to act on while the tool is up.
            KeyCode::KeyS
            | KeyCode::KeyF
            | KeyCode::KeyA
            | KeyCode::KeyX
            | KeyCode::KeyD
            | KeyCode::KeyM
            | KeyCode::KeyR
            | KeyCode::Delete
            | KeyCode::Backspace
            | KeyCode::Digit5
            | KeyCode::Digit6
            | KeyCode::Digit7
            | KeyCode::Digit8
            | KeyCode::Digit9
            | KeyCode::Digit0 => {}
            _ => return false,
        }
        true
    }

    pub(super) fn spheres_key_released(&mut self, code: KeyCode) {
        if code == KeyCode::KeyS {
            self.doc.spheres.gauge_end();
        }
    }

    /// Frames the rig rather than the active object while the tool is up.
    pub(super) fn spheres_frame_view(&mut self) -> bool {
        if !self.spheres_active() || self.doc.spheres.rig.is_empty() {
            return false;
        }
        let aspect = self.view.size.x / self.view.size.y.max(1.0);
        let bounds = self.doc.spheres.bounds();
        self.doc.camera.frame(bounds, aspect, true);
        true
    }

    /// Z while the tool is up. The rig has its own history, so forty sphere
    /// drags do not bury the mesh edits either side of them; with nothing
    /// left there, the press falls through to the scene's history (false).
    pub(super) fn spheres_undo(&mut self, redo: bool) -> bool {
        if !self.spheres_active() {
            return false;
        }
        let s = &mut self.doc.spheres;
        if s.is_dragging() {
            // Swallowed: stepping the rig back under a live drag would leave
            // the drag holding start state for spheres that moved.
            return true;
        }
        if !(if redo { s.redo() } else { s.undo() }) {
            return false;
        }
        self.toast(if redo { "Redo" } else { "Undo" });
        true
    }

    /// Once a frame, before drawing: lets the tool follow the cursor and
    /// keep its skin current, and hands the renderer what to draw.
    pub(super) fn spheres_frame(&mut self) {
        // With the tool put away too: an attached rig has to keep up with its
        // object while the Transform gizmo moves it.
        let scene = &self.doc.scene;
        self.doc.spheres.follow(|id| scene.get(id).map(Body::of));
        if !self.spheres_active() {
            self.renderer.spheres.hide();
            return;
        }
        let time = self.time();
        let pos = self.input.pointer;
        let blocked = !self.input.inside
            || !self.view.contains(pos)
            || self.over_ui()
            || self.ui.radial.is_some()
            || self.input.mods.alt_key()
            // On an arrow of the gizmo the pointer is the gizmo's.
            || self.transform.on_tool_handle
            || !matches!(self.input.drag, Drag::None | Drag::Spheres);
        // A screenshot run has no time to wait for the skin to settle.
        let settle_now = self.args.screenshot.is_some();
        self.spheres_size_from_scene();
        self.with_spheres(|s, cx| {
            s.tick(time);
            s.hover(cx, pos, blocked);
            s.refresh_skin(settle_now);
        });
        let s = &self.doc.spheres;
        let look = if s.preview { SkinLook::Solid } else { SkinLook::Shell };
        let model = s.frame().matrix();
        self.renderer.spheres.show(&self.gpu, &self.view, &s.armature(), s.skin(), look, s.shell_lift(), model);
        if s.is_settling() {
            self.redraw = true;
        }
    }

    /// Whether an object is left out of the viewport: the skin made from the
    /// rig, while the rig is being edited. The live skin stands in for it;
    /// drawn as well, it would bury the spheres inside their own skin.
    pub(super) fn spheres_stand_in_for(&self, object: u64) -> bool {
        let s = &self.doc.spheres;
        self.spheres_active() && !s.rig.is_empty() && s.skin_object == Some(object)
    }

    /// Bakes the skin into a real object: a new entry in the scene, active
    /// and ready to sculpt, with its pivot in the middle of the skin. A skin
    /// made earlier from the same spheres is hidden rather than deleted,
    /// since it may have been sculpted on. One undo step.
    pub(super) fn spheres_make_skin(&mut self) {
        match self.doc.spheres.make_skin() {
            Ok(made) => {
                let skin = made.skin;
                let (tris, ms) = (skin.stats.triangles, skin.stats.ms);
                let mesh = Mesh::new(skin.positions, skin.indices, None);
                let scene = &self.doc.scene;
                let earlier = self.doc.spheres.skin_object.and_then(|id| scene.get(id)).filter(|o| o.visible);
                let earlier = earlier.map(|o| (o.id, o.name.clone()));
                let mut id = 0;
                self.edit_scene(|scene, slot| {
                    if let Some(old) = earlier.as_ref().and_then(|(id, _)| scene.get_mut(*id)) {
                        old.visible = false;
                    }
                    id = scene.add("SSphere Mesh", mesh, slot);
                    // Every new object starts at the centre of the scene;
                    // this one belongs where the spheres are.
                    if let Some(object) = scene.get_mut(id) {
                        (object.position, object.rotation, object.scale) = (made.position, made.rotation, made.scale);
                    }
                    true
                });
                let s = &mut self.doc.spheres;
                s.skin_object = s.keep_rig.then_some(id);
                // Straight into sculpting what was just made.
                self.set_tool(Tool::Sculpt);
                self.toast("Make Skin");
                let name = self.doc.scene.active().name.clone();
                let mut text = format!("Made \"{name}\" from the spheres: {tris} triangles in {ms:.0} ms.");
                text += &match earlier {
                    Some((_, old)) => format!(" The earlier skin \"{old}\" is hidden, not deleted."),
                    None => " Undo (Z) removes it.".to_owned(),
                };
                self.status_checked(text, crate::printable::Cure::Remesh);
            }
            Err(e) => {
                self.doc.spheres.say(e.clone());
                self.status(e, false);
            }
        }
    }

    /// One press-drag-release, through the handlers the window feeds.
    fn play_drag(&mut self, from: Vec2, to: Vec2) {
        self.pointer_moved(from, None);
        self.pointer_pressed(MouseButton::Left, None);
        for i in 1..=8 {
            self.pointer_moved(from.lerp(to, i as f32 / 8.0), None);
        }
        self.pointer_released(MouseButton::Left);
    }

    /// Builds a small rig the way a person would (clicks, drags, the wheel
    /// and keys, all through the real input handlers) and prints what came
    /// of it.
    fn spheres_play(&mut self) {
        self.set_tool(Tool::SSpheres);
        self.input.inside = true;
        self.doc.spheres.set_next_radius(0.1);
        let px = |app: &Running, p: Vec3| app.view.project(p).unwrap_or(app.view.size * 0.5);
        let selected = |app: &Running| {
            let s = &app.doc.spheres;
            s.selected.and_then(|i| s.rig.get(i)).map_or(Vec3::ZERO, |n| s.frame().to_world(n.position))
        };
        // The first sphere: on the starting shape, where the mirror plane
        // crosses it.
        let on_model = px(self, Vec3::new(0.0, 0.3, 0.4));
        self.play_drag(on_model, on_model);
        // Pull two spheres out, one after the other, then click empty space
        // to carry the chain on.
        let root = px(self, selected(self));
        self.play_drag(root, root + Vec2::new(140.0, -60.0));
        let first = px(self, selected(self));
        self.play_drag(first, first + Vec2::new(120.0, 80.0));
        let tap = px(self, selected(self)) + Vec2::new(130.0, 30.0);
        self.play_drag(tap, tap);
        // The wheel over the first sphere makes it bigger.
        self.pointer_moved(root, None);
        self.spheres_frame();
        for _ in 0..3 {
            self.wheel(1.0);
        }
        // Move mode: lift the first pulled sphere, with everything below it.
        self.spheres_key_pressed(KeyCode::Digit2);
        self.play_drag(first, first + Vec2::new(0.0, -70.0));
        self.pointer_moved(Vec2::new(self.view.size.x * 0.5, 30.0), None);

        let s = &self.doc.spheres;
        println!("play: {} spheres, {:?}", s.rig.alive_count(), s.undo_summary());
        for (i, n) in s.rig.nodes().iter().enumerate().filter(|(_, n)| n.alive) {
            println!("  {i}: parent {:?} at {:.3} radius {:.3}", n.parent, n.position, n.radius);
        }
    }

    /// Attaches the demo figure to the starting shape, which is moved and
    /// turned either first (the spheres stay where they are and only the
    /// mirror plane goes to the shape) or, with `carried`, afterwards, along
    /// with an even resize (the figure goes with the shape). Prints where
    /// one sphere ended up either way.
    fn spheres_demo_attach(&mut self, carried: bool) {
        let chest = |app: &Running| {
            let s = &app.doc.spheres;
            s.selected.and_then(|i| s.rig.get(i)).map_or(Vec3::ZERO, |n| s.frame().to_world(n.position))
        };
        let put = |app: &mut Running, size: f32| {
            let object = app.doc.scene.active_mut();
            object.position = Vec3::new(0.7, 0.35, 0.0);
            object.rotation = Quat::from_rotation_z(-0.5) * Quat::from_rotation_y(0.6);
            object.scale = Vec3::splat(size);
        };
        let before = chest(self);
        if !carried {
            put(self, 1.0);
        }
        let body = Body::of(self.doc.scene.active());
        let plane_moved = self.doc.spheres.attach(body);
        if carried {
            put(self, 0.8);
        }
        let scene = &self.doc.scene;
        self.doc.spheres.follow(|id| scene.get(id).map(Body::of));
        let s = &self.doc.spheres;
        println!(
            "attach: plane moved {plane_moved}, attached to {:?}, chest {before:.3} -> {:.3}, frame {:?}",
            s.attached_to(),
            chest(self),
            s.frame()
        );
    }

    /// The figure's elbow selected in Move mode, and the arrows pulled by the
    /// real pointer routines: out along the red one, then up along the green
    /// one, which lifts the hand and its fingers while the other arm follows
    /// as a mirror image. With `how` `-held` the second pull is still going;
    /// with `-play` it checks what the arrows do and prints what came of it.
    fn spheres_demo_gizmo(&mut self, elbow: usize, how: &str) {
        self.input.inside = true;
        self.input.mods = ModifiersState::empty();
        self.doc.spheres.set_mode(EditMode::Move);
        self.doc.spheres.selected = Some(elbow);
        self.sync_view();
        if how == "-play" {
            return self.spheres_gizmo_play(elbow);
        }
        // Pulled back a little, so the arrows clear the buttons in the corner.
        self.doc.camera.distance *= 1.3;
        self.sync_view();
        for (axis, by) in [(0, 0.25), (1, 0.3)] {
            let Some(frame) = self.demo_frame() else { return };
            let grab = frame.pivot + frame.axis(axis) * frame.arm * 1.1;
            let release = how != "-held" || axis == 0;
            self.demo_drag(&[grab, grab + frame.axis(axis) * by], release);
        }
    }

    /// Works the arrows the way a person would, through the real pointer, key
    /// and undo routines, and prints what came of each step: a check of the
    /// wiring that needs no one at the tablet.
    fn spheres_gizmo_play(&mut self, elbow: usize) {
        fn say(what: &str, ok: bool) {
            println!("{} {what}", if ok { "ok  " } else { "FAIL" });
        }
        let world = |app: &Running| -> Vec<Vec3> {
            let s = &app.doc.spheres;
            s.rig.nodes().iter().map(|n| s.frame().to_world(n.position)).collect()
        };
        let mut branch = Vec::new();
        self.doc.spheres.rig.collect_subtree(elbow, &mut branch);
        let steps = |app: &Running| app.doc.spheres.undo_summary();
        let skin_version = |app: &Running| app.doc.spheres.skin().map(|(_, generation)| generation);
        let before = world(self);
        self.spheres_frame();
        let skin_before = skin_version(self);

        let Some(frame) = self.demo_frame() else { return say("the arrows are up on the selected sphere", false) };
        say("the arrows sit on the selected sphere, on the world's axes", {
            frame.pivot.distance(before[elbow]) < 1e-4 && (0..3).all(|i| frame.axis(i).distance(Vec3::AXES[i]) < 1e-4)
        });
        say("and there is no ring or cube, only arrows", {
            (0..3).all(|i| !frame.shown.has(Handle::Rotate(i)) && !frame.shown.has(Handle::Scale(i)))
                && !frame.shown.has(Handle::Uniform)
        });

        // A press on the sphere itself is the sphere's, not an arrow's.
        let px = |app: &Running, p: Vec3| app.view.project(p).unwrap_or(app.view.size * 0.5);
        let centre = px(self, frame.pivot);
        self.pointer_moved(centre, None);
        self.pointer_pressed(MouseButton::Left, None);
        say("a press on the sphere itself is a drag of the sphere", self.input.drag == Drag::Spheres);
        self.pointer_released(MouseButton::Left);
        say("which left no step when it did not move", steps(self).is_none());

        // The red arrow, pulled out 0.25.
        let grab = frame.pivot + frame.axis(0) * frame.arm * 1.1;
        say("a press on the red arrow takes the drag", self.demo_drag(&[grab, grab + Vec3::X * 0.25], false));
        say("and it is a drag of the arrow", self.input.drag == Drag::Gizmo);
        // Z in the middle of a drag must not pull the spheres out from under it.
        self.act(Action::Undo);
        say("Z during the drag does nothing", self.transform_dragging() && self.doc.spheres.is_dragging());
        self.spheres_frame();
        say("the skin follows the spheres while the arrow is held", skin_version(self) != skin_before);
        self.pointer_released(MouseButton::Left);
        let after = world(self);
        let moved = (0..after.len()).all(|i| {
            let want = if branch.contains(&i) { before[i] + Vec3::X * 0.25 } else { before[i] };
            after[i].distance(want) < 0.02
        });
        say("the sphere and its branch moved 0.25 out, and nothing else", moved);
        println!("     the elbow went from {:.3} to {:.3}", before[elbow], after[elbow]);
        let s = &self.doc.spheres;
        let twin = s.rig.geometry(s.symmetry()).0.into_iter().find(|t| t.node == elbow && t.mirrored);
        let real = s.rig.nodes()[elbow].position;
        say("its twin on the other side is still its mirror image", {
            twin.is_some_and(|t| t.centre.distance(Vec3::new(-real.x, real.y, real.z)) < 1e-5)
        });
        say("one drag is one step in the spheres' own history", steps(self) == Some(("Move Sphere", 1)));
        self.act(Action::Undo);
        say("which Z takes back, every sphere exactly where it was", world(self) == before && steps(self).is_none());
        self.act(Action::Redo);
        say("and Shift+Z brings back", world(self) == after);
        self.act(Action::Undo);

        // Esc mid-drag puts them back and the rest of the drag does nothing.
        let Some(frame) = self.demo_frame() else { return say("the arrows are still up", false) };
        let grab = frame.pivot + frame.axis(1) * frame.arm * 1.1;
        self.demo_drag(&[grab, grab + Vec3::Y * 0.3], false);
        say("the green arrow moved them", world(self)[elbow].distance(before[elbow]) > 0.2);
        self.spheres_key_pressed(KeyCode::Escape);
        say("Esc puts them back and ends the drag", world(self) == before && !self.transform_dragging());
        self.pointer_moved(px(self, grab + Vec3::Y * 0.5), None);
        self.pointer_released(MouseButton::Left);
        say("the rest of that drag moves nothing and leaves no step", world(self) == before && steps(self).is_none());
        say("and the sphere is still selected", self.doc.spheres.selected == Some(elbow));

        // What puts the arrows away.
        self.spheres_key_pressed(KeyCode::Digit3);
        say("the Scale mode has no arrows", self.demo_frame().is_none());
        self.spheres_key_pressed(KeyCode::Digit2);
        say("the Move mode has them", self.demo_frame().is_some());
        self.set_tool(Tool::Sculpt);
        self.transform_frame();
        say("putting the tool away takes them off", self.ui.gizmo.is_empty() && self.demo_frame().is_none());
        self.set_tool(Tool::SSpheres);
        self.transform_frame();
        say("and bringing it back puts them on the same sphere", !self.ui.gizmo.is_empty());
        let corner = self.view.origin + Vec2::new(30.0, 30.0);
        self.pointer_moved(corner, None);
        self.pointer_pressed(MouseButton::Left, None);
        self.pointer_released(MouseButton::Left);
        say("a click on empty space deselects, and the arrows go", self.demo_frame().is_none());
        self.doc.spheres.selected = Some(elbow);
        self.spheres_key_pressed(KeyCode::Delete);
        say("deleting the sphere takes them off", self.demo_frame().is_none());
        self.act(Action::Undo);
        say("and undoing the delete brings them back", self.demo_frame().is_some());
    }

    /// A small figure for visual checks without a person at the tablet:
    /// `rig` shows the armature under its shell, `preview` the solid skin,
    /// `skin` the finished object, `back` the spheres again after that, and
    /// `again` a second skin of the changed rig. `attached` attaches the
    /// figure to the starting shape after that has been moved and turned,
    /// `followed` moves, turns and resizes the shape after attaching, and
    /// `followed-skin` makes the skin of that. `wheel` holds the Space wheel
    /// open over the figure. `new` presses New SSphere Rig on the starting
    /// scene, and `play` builds a rig through scripted pointer input.
    /// `gizmo` pulls the arrows on the figure's elbow out and up, `gizmo-held`
    /// leaves the second pull going, and `gizmo-play` checks the arrows
    /// through the real pointer and keys.
    pub(super) fn spheres_demo(&mut self, what: &str) {
        if what == "play" {
            return self.spheres_play();
        }
        if what == "new" {
            self.sync_view();
            self.act(Action::NewRig);
            let s = &self.doc.spheres;
            let at = s.rig.nodes().first().map(|n| s.frame().to_world(n.position));
            println!("new: {} sphere(s) at {at:?}, attached to {:?}: {:?}", s.rig.alive_count(), s.attached_to(), s.note());
            return;
        }
        let attach = matches!(what, "attached" | "followed" | "followed-skin");
        // Clear of the starting shape, so the figure can be seen whole
        // (unless the figure is to be attached to it).
        if let Some(first) = self.doc.scene.objects.first_mut() {
            first.visible = attach;
        }
        self.set_tool(Tool::SSpheres);
        let rig = &mut self.doc.spheres.rig;
        let v = Vec3::new;
        let pelvis = rig.add_root(v(0.0, 1.0, 0.0), 0.16);
        let mut chain = |from: usize, spheres: &[(Vec3, f32)]| {
            spheres.iter().fold(from, |parent, &(p, r)| rig.add_child(parent, p, r).unwrap_or(parent))
        };
        let chest = chain(pelvis, &[(v(0.0, 1.2, 0.02), 0.14), (v(0.0, 1.42, 0.0), 0.18)]);
        chain(chest, &[(v(0.0, 1.63, 0.0), 0.065), (v(0.0, 1.8, 0.02), 0.12)]);
        let hand = chain(chest, &[(v(0.2, 1.52, 0.0), 0.08), (v(0.44, 1.33, 0.02), 0.055), (v(0.62, 1.1, 0.14), 0.045)]);
        for finger in [-1.0, 0.0, 1.0] {
            chain(hand, &[(v(0.7, 1.0, 0.18 + finger * 0.035), 0.018)]);
        }
        chain(pelvis, &[(v(0.1, 0.93, 0.0), 0.11), (v(0.13, 0.52, 0.04), 0.07), (v(0.14, 0.1, 0.0), 0.05)]);
        self.doc.spheres.selected = Some(chest);
        let elbow = self.doc.spheres.rig.parent_of(hand);
        self.doc.spheres.preview = what == "preview";
        if attach {
            self.spheres_demo_attach(what != "attached");
        }
        self.sync_view();
        self.frame_view();
        self.doc.camera.update(1.0);
        if let (Some(how), Some(elbow)) = (what.strip_prefix("gizmo"), elbow) {
            self.spheres_demo_gizmo(elbow, how);
        }
        if what == "wheel" {
            // In the interface's units, which no frame has set up yet.
            let per_point = self.window.scale_factor() as f32 * self.doc.opts.ui_scale;
            let middle = (self.view.origin + self.view.size * 0.5) / per_point;
            self.ui.radial = Some(egui::pos2(middle.x, middle.y));
        }
        if what == "followed-skin" {
            // The finished object should stand exactly where the spheres do.
            let spheres = self.doc.spheres.bounds();
            self.spheres_make_skin();
            let object = self.doc.scene.active();
            let model = object.matrix();
            let world = self.doc.mesh.positions.iter().map(|&p| model.transform_point3(p));
            let (lo, hi) = world.fold((Vec3::MAX, Vec3::MIN), |(lo, hi), p| (lo.min(p), hi.max(p)));
            println!("skin: spheres {:.3} to {:.3}", spheres.min, spheres.max);
            println!("skin: object  {lo:.3} to {hi:.3}, pivot {:.3}", object.position);
        }
        if matches!(what, "skin" | "back" | "again") {
            self.spheres_make_skin();
            self.frame_view();
            self.doc.camera.update(1.0);
        }
        // Back to the spheres after skinning them: the skin object steps
        // aside for the live skin.
        if matches!(what, "back" | "again") {
            self.set_tool(Tool::SSpheres);
        }
        // And a second skin of a changed rig supersedes the first.
        if what == "again" {
            let s = &mut self.doc.spheres;
            let head = s.rig.nodes()[4].position;
            s.rig.set_position(4, head + Vec3::new(0.0, 0.25, 0.0));
            self.spheres_make_skin();
            for o in &self.doc.scene.objects {
                println!("again: \"{}\" visible {}", o.name, o.visible);
            }
        }
    }
}
