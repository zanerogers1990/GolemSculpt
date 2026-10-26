//! The app's side of live mirror copies: drawing them, the commands of the
//! Mirror section of the Objects card, finding the placement under the
//! pointer (the object, an instance, or one of its copies), the gizmo on a
//! copy, and a scripted run to look at it all (`--demo-mirror`).
//!
//! What a copy is, and the arithmetic of it, are in `scene/mirror.rs` and
//! `sculpt_symmetry::mirror`. Here are only the places the running app meets
//! it: the draw list, the pointer, the undo history and the status line.

use glam::{Mat4, Quat, Vec2, Vec3};
use sculpt_symmetry::MirrorRepeater;
use winit::event::MouseButton;
use winit::keyboard::ModifiersState;

use super::{Drag, Running};
use crate::doc::{Action, Tool};
use crate::gizmo::{Frame, Handles, Placement, Target};
use crate::render::{DrawItem, flash};
use crate::scene::{MirrorCommand, Object, Scene, TOOL_COPY};
use crate::sculpt::Brush;

/// The gizmo's target for an object: its own place, or, when it is the active
/// object and one of its live copies is the side selected, that copy's.
pub(super) fn shown_target(scene: &Scene, object: &Object, radius: f32) -> Target {
    if object.id != scene.active_id() || scene.selected_side() == 0 {
        return Target::object(object, radius);
    }
    let shown = scene.shown_placement(object);
    let placement = Placement { position: shown.position, rotation: shown.rotation, scale: shown.scale };
    Target { placement, ..Target::object(object, radius) }
}

/// Moves `id` so that it is where a drag of the gizmo has put its target (see
/// `shown_target`): the same thing for an object, the original reflected back
/// for a copy.
pub(super) fn place_shown(scene: &mut Scene, id: u64, placement: &Placement) {
    if id != scene.active_id() || scene.selected_side() == 0 {
        if let Some(object) = scene.get_mut(id) {
            placement.apply(object);
        }
        return;
    }
    let Placement { position, rotation, scale } = *placement;
    scene.place_shown(id, sculpt_symmetry::Placement::new(position, rotation, scale));
}

impl Running {
    /// The mirror copies of the objects shown, as more things to draw. They
    /// are the object's own mesh again through a reflected matrix, and a copy
    /// is never the active object, which is what the last element of the
    /// matrix says.
    pub(super) fn mirror_items(&self, mut items: Vec<DrawItem>) -> Vec<DrawItem> {
        let shown = |o: &&Object| o.visible && !self.spheres_stand_in_for(o.id) && !self.tools_hide(o.id);
        for o in self.doc.scene.objects.iter().filter(|o| o.mirror.is_some()).filter(shown) {
            items.extend(o.placements().skip(1).map(|(_, model)| {
                let mut model = model.to_cols_array_2d();
                model[3][3] = 0.0;
                DrawItem { mesh: o.mesh, model }
            }));
        }
        items
    }

    /// The active object, its live mirror copies and the copies tools keep of it (an
    /// array's) as the places an export writes, with the names they go under: all of them
    /// are part of the model on screen. The object comes first, in its own place.
    pub(super) fn export_places(&self) -> Vec<(String, Mat4)> {
        let scene = &self.doc.scene;
        scene.export_places(scene.active())
    }

    /// What an export's status line adds when the object has copies.
    pub(super) fn export_note(&self) -> String {
        let scene = &self.doc.scene;
        scene.export_note(scene.active())
    }

    /// The pulse of an object that was picked or undone, on its copies too.
    pub(super) fn pulses_of(&self, id: u64, color: [f32; 4]) -> Vec<flash::Item> {
        let scene = &self.doc.scene;
        let Some(o) = scene.get(id).filter(|o| o.visible && !self.spheres_stand_in_for(id)) else { return Vec::new() };
        o.placements().map(|(_, m)| flash::Item { mesh: o.mesh, model: m.to_cols_array_2d(), color }).collect()
    }

    /// The nearest placement of any object under a pixel: the object, and
    /// which of its places (0 for the object itself, else a live mirror
    /// copy's flip mask or `TOOL_COPY` and up for a copy a tool keeps: see
    /// `Picked::copy`).
    pub(super) fn pick_placed(&self, px: Vec2) -> Option<(u64, u16)> {
        let (origin, dir) = self.view.ray(px);
        self.doc.scene.pick(origin, dir, &self.doc.mesh).map(|p| (p.id, p.copy))
    }

    /// A click on the viewport with the Transform tool up, after it has
    /// picked the object: a click on one of its mirror copies puts the gizmo
    /// on that side, a click on the object puts it back. Shift or Ctrl +
    /// click edits the selection, and leaves the side alone.
    pub(super) fn mirror_click_side(&mut self) {
        let mods = self.input.mods;
        if mods.shift_key() || mods.control_key() {
            return;
        }
        if let Some((id, copy)) = self.pick_placed(self.input.pointer) {
            // The gizmo sits on the object or on one of its mirror copies; a
            // copy a tool keeps is the tool's, and the object's own place is
            // where the gizmo goes for it.
            self.doc.scene.select_side(id, if copy < TOOL_COPY { copy as u8 } else { 0 });
            self.redraw = true;
        }
    }

    /// Runs a command of the Mirror section on the active object. All but
    /// choosing a side are one undo step.
    pub(super) fn mirror_command(&mut self, command: MirrorCommand) {
        let (id, name) = (self.doc.scene.active_id(), self.doc.scene.active().name.clone());
        match command {
            MirrorCommand::Set(axes) => {
                let center = self.doc.scene.active().mirror.map_or(Vec3::ZERO, |m| m.center);
                if !self.edit_scene(|scene, _| scene.set_mirror(id, axes & 7, center)) {
                    return;
                }
                if axes & 7 == 0 {
                    self.status(format!("Removed the mirror of \"{name}\". Undo (Ctrl+Z) brings it back."), true);
                    self.toast("Mirror Off");
                } else {
                    let across = MirrorRepeater::axis_name_of(axes & 7);
                    self.status(format!("Mirrored \"{name}\" across {across}. Undo (Ctrl+Z) takes it off again."), true);
                    self.toast("Mirror");
                }
            }
            MirrorCommand::Remove => self.mirror_command(MirrorCommand::Set(0)),
            MirrorCommand::Validate => {
                let mut made = Vec::new();
                let done = self.edit_scene(|scene, slot| match scene.bake_mirror(id, slot) {
                    Some(objects) => {
                        made = objects;
                        true
                    }
                    None => false,
                });
                if !done {
                    return;
                }
                self.doc.mesh_replaced = true;
                let count = made.len();
                let what = if count == 1 {
                    "1 mirror copy is now a separate object".to_owned()
                } else {
                    format!("{count} mirror copies are now separate objects")
                };
                let text =
                    format!("Validated: {what}. Undo (Ctrl+Z) makes {} live again.", if count == 1 { "it" } else { "them" });
                self.status(text, true);
                self.toast("Validate Mirror");
                for new in made {
                    self.flash.start(new, flash::Kind::Select);
                }
            }
            MirrorCommand::Side(copy) => {
                self.doc.scene.select_side(id, copy);
                self.redraw = true;
            }
        }
    }

    // ---- for visual checks -------------------------------------------------------

    /// `--demo-mirror`: the sphere moved off the middle and turned, its live
    /// mirror on, and a few strokes laid down by way of the real pointer
    /// routines, some beginning on the copy. The environment variable
    /// `SCULPT_DEMO_MIRROR` picks which:
    ///
    /// - unset: mirrored across X
    /// - `xz`: across X and Z, which is three copies, one of them not reversed
    /// - `gizmo`: the Transform tool up with the gizmo on the copy
    /// - `validate`: then Validate, so the copies are objects of their own
    /// - `list`: a cube added, so the mirrored sphere is an unselected row of the list
    /// - `front`, `back`: seen from straight ahead or from behind
    /// - `play`: works the wiring through the real routines and prints what came of each step
    pub(super) fn mirror_demo(&mut self) {
        let what = std::env::var("SCULPT_DEMO_MIRROR").unwrap_or_default();
        self.input.inside = true;
        self.input.mods = ModifiersState::empty();
        self.demo_mirror_set_up(if what == "xz" { 1 | 4 } else { 1 });
        if what == "play" {
            self.mirror_play();
            return;
        }
        self.demo_mirror_strokes();
        match what.as_str() {
            "gizmo" => {
                self.doc.scene.select_side(self.doc.scene.active_id(), 1);
                self.doc.tool = Tool::Transform;
            }
            "validate" => self.act(Action::Mirror(MirrorCommand::Validate)),
            // Another object selected, so the mirrored one shows in the list as it does unselected.
            "list" => self.act(Action::New(crate::mesh::primitives::Primitive::Cube)),
            // Straight on and from behind, where the two sides can be compared.
            "front" => (self.doc.camera.yaw, self.doc.camera.pitch) = (0.0, 8.0),
            "back" => (self.doc.camera.yaw, self.doc.camera.pitch) = (180.0, 8.0),
            _ => {}
        }
        // The brush ring is where the last stroke ended; leave it out of the picture.
        self.input.inside = false;
        self.sync_view();
    }

    /// The sphere off to one side and turned, mirrored across `axes`, and the
    /// camera on all of it.
    fn demo_mirror_set_up(&mut self, axes: u8) {
        let object = self.doc.scene.active_mut();
        object.position = Vec3::new(0.75, 0.1, if axes & 4 != 0 { 0.55 } else { 0.0 });
        object.rotation = Quat::from_rotation_y(0.5) * Quat::from_rotation_x(-0.3);
        object.scale = Vec3::new(0.85, 1.15, 0.85);
        self.act(Action::Mirror(MirrorCommand::Set(axes)));
        let aspect = self.view.size.x / self.view.size.y.max(1.0);
        let bounds = self.doc.scene.bounds(&self.doc.mesh);
        self.doc.camera.frame(bounds, aspect, false);
        self.doc.camera.yaw = 25.0;
        self.doc.camera.pitch = 18.0;
        // Room round it: the panels cover the sides of the picture.
        self.doc.camera.distance *= 1.25;
        self.sync_view();
        self.demo_symmetry(Some([false; 3]));
    }

    /// Where the middle of copy `copy` of the active object is on screen.
    fn copy_pixel(&self, copy: u8, from_middle: Vec3) -> Option<Vec2> {
        let model = self.doc.scene.active().placed_matrix(copy);
        self.view.project(model.transform_point3(from_middle))
    }

    /// Presses at `from` and drags through `to`, by way of the routines the
    /// window feeds, with a brush `radius` pixels across. A press that misses
    /// the model would orbit, so one that does is said, not left to move the
    /// view.
    pub(super) fn demo_drag_stroke(&mut self, brush: Brush, strength: f32, radius: f32, from: Vec2, to: Vec2) {
        let settings = &mut self.doc.sculptor.settings;
        settings.brush = brush;
        settings.radius_px = radius;
        settings.per_brush[brush.index()].strength = strength;
        self.pointer_moved(from, None);
        self.sync_view();
        self.pointer_pressed(MouseButton::Left, None);
        if self.input.drag != Drag::Sculpt {
            eprintln!("demo-mirror: a {brush:?} stroke began off the model at {from}");
        }
        for i in 1..=24 {
            self.pointer_moved(from.lerp(to, i as f32 / 24.0), None);
        }
        self.pointer_released(MouseButton::Left);
        self.sync_view();
    }

    /// Strokes laid out in units of how big the sphere is on screen, so they
    /// land on it however far the view is.
    fn demo_mirror_strokes(&mut self) {
        // On the copy: a ridge across it, a crease and a bump.
        if let Some(c) = self.copy_pixel(1, Vec3::ZERO) {
            let r = c.distance(self.copy_pixel(1, Vec3::Y * 0.5).unwrap_or(c));
            let at = |x: f32, y: f32| c + Vec2::new(x, y) * r;
            self.demo_drag_stroke(Brush::Clay, 0.7, r * 0.3, at(-0.55, -0.45), at(0.35, -0.6));
            self.demo_drag_stroke(Brush::Crease, 0.9, r * 0.2, at(-0.5, 0.0), at(0.5, 0.15));
            self.demo_drag_stroke(Brush::Inflate, 0.6, r * 0.28, at(-0.3, 0.5), at(0.4, 0.45));
        }
        // And on the object itself.
        if let Some(c) = self.copy_pixel(0, Vec3::ZERO) {
            let r = c.distance(self.copy_pixel(0, Vec3::Y * 0.5).unwrap_or(c));
            let at = |x: f32, y: f32| c + Vec2::new(x, y) * r;
            self.demo_drag_stroke(Brush::SnakeHook, 0.5, r * 0.35, at(0.25, -0.5), at(0.55, -1.0));
        }
        self.doc.sculptor.settings.brush = Brush::Clay;
        self.doc.sculptor.settings.demo_setup(true);
    }

    /// Works the mirror the way a person would, through the real pointer, key
    /// and undo routines, and prints what came of each step: a check of the
    /// wiring that needs no one at the tablet.
    fn mirror_play(&mut self) {
        fn say(what: &str, ok: bool) {
            println!("{} {what}", if ok { "ok  " } else { "FAIL" });
        }
        let id = self.doc.scene.active_id();
        let steps = |app: &Running| app.doc.history.steps().0;
        let rest = self.doc.mesh.positions.clone();
        say("the mirror is on, with a copy to see", self.doc.scene.active().placements().count() == 2);
        say("the copy is under the pointer where the geometry says", {
            self.copy_pixel(1, Vec3::ZERO).and_then(|px| self.pick_placed(px)) == Some((id, 1))
        });

        // A stroke that begins on the copy.
        let Some(centre) = self.copy_pixel(1, Vec3::ZERO) else { return say("the copy is in view", false) };
        let count = steps(self);
        self.pointer_moved(centre, None);
        self.sync_view();
        say("the pointer over the copy counts as over the mesh", self.hover_on_active());
        self.pointer_pressed(MouseButton::Left, None);
        say("a press on the copy begins a stroke", self.input.drag == Drag::Sculpt);
        let copy_matrix = self.doc.scene.active().placed_matrix(1);
        say("which goes through the copy's matrix", self.doc.scene.brush_matrix() == copy_matrix);
        for i in 1..=15 {
            self.pointer_moved(centre + Vec2::new(i as f32 * 2.0, 0.0), None);
        }
        self.pointer_released(MouseButton::Left);
        let moved: Vec<Vec3> =
            self.doc.mesh.positions.iter().zip(&rest).filter(|(a, b)| a != b).map(|(_, &b)| b).collect();
        let middle = moved.iter().copied().sum::<Vec3>() / moved.len().max(1) as f32;
        let expect = self.view.ray(centre);
        let seen = self.doc.scene.pick(expect.0, expect.1, &self.doc.mesh).map(|p| p.pos);
        let want = seen.map(|p| copy_matrix.inverse().transform_point3(p));
        println!("     {} vertices moved, around {middle}", moved.len());
        say(
            "it moves the vertices the copy shows under the pointer",
            !moved.is_empty() && want.is_some_and(|w| w.distance(middle) < 0.25),
        );
        say("as one undo step", steps(self) == count + 1);
        say("the orbit pivot is where the copy is", {
            let anchor = self.anchor();
            anchor.is_some_and(|a| {
                a.distance(copy_matrix.transform_point3(self.doc.sculptor.last_hit.unwrap_or(Vec3::ZERO))) < 1e-4
            })
        });

        // Hover and a stroke on the object itself.
        if let Some(own) = self.copy_pixel(0, Vec3::ZERO) {
            self.pointer_moved(own, None);
            say(
                "over the object the brush frame is the object's",
                self.hover_on_active() && self.local_is(self.doc.scene.active().matrix()),
            );
        }

        // Undo and redo of the stroke, then of the mirror.
        let after = self.doc.mesh.positions.clone();
        self.act(Action::Undo);
        say("undo puts the stroke back", self.doc.mesh.positions == rest);
        self.act(Action::Redo);
        say("redo does it again", self.doc.mesh.positions == after);

        let count = steps(self);
        self.act(Action::Mirror(MirrorCommand::Set(1 | 2)));
        say(
            "changing the axes is one step and makes three copies",
            steps(self) == count + 1 && self.doc.scene.active().placements().count() == 4,
        );
        self.act(Action::Undo);
        say("undo brings the one copy back", self.doc.scene.active().placements().count() == 2);
        self.act(Action::Mirror(MirrorCommand::Remove));
        say(
            "Remove takes the copies away",
            self.doc.scene.active().placements().count() == 1 && self.pick_placed(centre).is_none_or(|p| p.1 == 0),
        );
        self.act(Action::Undo);
        say("and undo puts them back", self.doc.scene.active().placements().count() == 2);

        // Validate, undo, redo.
        let objects = self.doc.scene.objects.len();
        self.act(Action::Mirror(MirrorCommand::Validate));
        say(
            "Validate makes the copy an object of its own",
            self.doc.scene.objects.len() == objects + 1 && self.doc.scene.active().mirror.is_none(),
        );
        self.act(Action::Undo);
        say(
            "undo makes it live again",
            self.doc.scene.objects.len() == objects && self.doc.scene.active().mirror.is_some(),
        );
        self.act(Action::Redo);
        say("redo validates it again", self.doc.scene.objects.len() == objects + 1);
        self.act(Action::Undo);

        // The gizmo on the copy: a click picks the side, a drag of an arrow moves the original the other way.
        self.set_tool(Tool::Transform);
        self.doc.scene.select_side(id, 0);
        if let Some(px) = self.copy_pixel(1, Vec3::ZERO) {
            self.pointer_moved(px, None);
            self.pointer_pressed(MouseButton::Left, None);
            self.pointer_released(MouseButton::Left);
        }
        say("a click on the copy puts the gizmo on the copy", self.doc.scene.selected_side() == 1);
        if let Some(px) = self.copy_pixel(0, Vec3::ZERO) {
            self.pointer_moved(px, None);
            self.pointer_pressed(MouseButton::Left, None);
            self.pointer_released(MouseButton::Left);
        }
        say("a click on the object puts it back on the object", self.doc.scene.selected_side() == 0);
        self.doc.scene.select_side(id, 1);
        self.sync_view();
        let (_, targets) = super::transform::selection_targets(&self.doc.scene, &self.doc.mesh);
        let ppp = self.egui_ctx.pixels_per_point();
        let orientation = self.doc.opts.gizmo_axes;
        let Some(frame) = Frame::on(&targets, orientation, Handles::ALL, &self.view, ppp) else {
            return say("the gizmo is up on the copy", false);
        };
        let object = self.doc.scene.active().clone();
        let copy_at = self.doc.scene.shown_placement(&object).position;
        say("the gizmo sits on the copy, not the object", frame.pivot.distance(copy_at) < 1e-4);
        let (start, count) = (object.position, steps(self));
        let grab = frame.pivot + frame.axis(0) * frame.arm * 1.1;
        let to = grab + frame.axis(0) * 0.4;
        let project = |app: &Running, p: Vec3| app.view.project(p).unwrap_or(app.view.size * 0.5);
        let from = project(self, grab);
        self.pointer_moved(from, None);
        self.pointer_pressed(MouseButton::Left, None);
        say("a press on the copy's red arrow takes the drag", self.input.drag == Drag::Gizmo);
        for i in 1..=8 {
            let at = project(self, grab.lerp(to, i as f32 / 8.0));
            self.pointer_moved(at, None);
        }
        self.pointer_released(MouseButton::Left);
        let object = self.doc.scene.active().clone();
        let now = self.doc.scene.shown_placement(&object).position;
        println!("     the copy moved by {:?}, the object by {:?}", now - copy_at, object.position - start);
        say("the copy moved with the pointer", (now - copy_at).distance(frame.axis(0) * 0.4) < 0.05);
        let across = Vec3::new(-1.0, 1.0, 1.0);
        say("and the object moved the other way across X", object.position.distance(now * across) < 1e-4);
        say("as one undo step", steps(self) == count + 1);
        self.act(Action::Undo);
        say("which puts the object back", self.doc.scene.active().position == start);
        self.set_tool(Tool::Sculpt);

        // Save and open: the live mirror is in the file.
        let folder = crate::prefs::scratch("sculpt_mirror_demo");
        let _ = std::fs::remove_dir_all(&folder);
        if std::fs::create_dir_all(&folder).is_err() {
            return say("a folder to write a file in", false);
        }
        // Its picture, too, goes there, never beside the user's own.
        self.ui.recent.restore(Vec::new(), Some(folder.join("thumbnails")));
        let file = folder.join("mirrored.sclp");
        let mirror = self.doc.scene.active().mirror;
        say("the scene saves", self.save_scene(&file).is_ok());
        self.act(Action::Mirror(MirrorCommand::Remove));
        say("with the mirror off there is nothing to bring back", self.doc.scene.active().mirror.is_none());
        self.load(file.clone(), true);
        say(
            "opening the file brings the live mirror back",
            mirror.is_some() && self.doc.scene.active().mirror == mirror,
        );
        say("with its copy on screen", self.doc.scene.active().placements().count() == 2);
        let objects = self.doc.scene.objects.len();
        self.load(file, false);
        let added = self.doc.scene.objects.len() == objects * 2;
        say("adding the file's objects brings theirs with them", added && self.doc.scene.active().mirror == mirror);
        self.act(Action::Undo);
        say("which one undo takes back", self.doc.scene.objects.len() == objects);
        let _ = std::fs::remove_dir_all(&folder);
    }

    /// Whether the brush's view is the active mesh seen through `model`.
    fn local_is(&self, model: Mat4) -> bool {
        let probe = Vec2::new(self.view.size.x * 0.5, self.view.size.y * 0.5);
        let (a, b) = (self.local.ray(probe), self.view.local_to(model).ray(probe));
        a.0.distance(b.0) < 1e-4 && a.1.distance(b.1) < 1e-4
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use glam::EulerRot;

    use super::super::install;
    use super::*;
    use crate::io::{self, Details, SavedObject, slot};
    use crate::mesh::Mesh;
    use crate::mesh::primitives::{self, Primitive};
    use crate::scene::saved_mirror;

    fn ball() -> Mesh {
        primitives::build(Primitive::Sphere, 1_000, 0.5)
    }

    /// The Unity tests' object, off to one side, turned and scaled.
    fn placed(object: &mut Object) {
        object.position = Vec3::new(0.7, 0.2, -0.3);
        object.rotation = Quat::from_euler(EulerRot::YXZ, 30f32.to_radians(), 10f32.to_radians(), -20f32.to_radians());
        object.scale = Vec3::splat(0.8);
    }

    fn folder(name: &str) -> PathBuf {
        let folder = std::env::temp_dir().join("sculpt_mirror_test").join(name);
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    fn saved_object(name: &str, mesh: usize) -> SavedObject {
        SavedObject {
            name: name.into(),
            visible: true,
            mesh,
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        }
    }

    /// What `Running::save_scene` writes for a scene, without a window.
    fn save(scene: &Scene, slot: &Mesh, path: &std::path::Path) {
        let mut ids: Vec<u64> = Vec::new();
        let objects: Vec<SavedObject> = scene
            .objects
            .iter()
            .map(|o| {
                let mesh = ids.iter().position(|&id| id == o.mesh).unwrap_or_else(|| {
                    ids.push(o.mesh);
                    ids.len() - 1
                });
                SavedObject {
                    position: o.position,
                    rotation: o.rotation,
                    scale: o.scale,
                    ..saved_object(&o.name, mesh)
                }
            })
            .collect();
        let meshes: Vec<&Mesh> = ids.iter().map(|&id| scene.mesh(id, slot).unwrap()).collect();
        let mut details = Details::default();
        scene.keep_mirrors(&mut details);
        io::save_scene_with(&meshes, &objects, scene.index_of(scene.active_id()).unwrap(), &details, path).unwrap();
    }

    /// What opening a file does for the live mirrors: `install`, then `apply_details`.
    fn open(path: &std::path::Path) -> Scene {
        let saved = io::load_scene(path).unwrap();
        let (mut scene, mut slot) = (Scene::new("Placeholder"), ball());
        let details = install(&mut scene, &mut slot, saved);
        scene.restore_mirrors(&details);
        scene
    }

    #[test]
    fn a_saved_scene_comes_back_with_its_mirrors_and_older_files_open_without() {
        let dir = folder("save");
        let mut slot = ball();
        let mut scene = Scene::new("Ball");
        let first = scene.active_id();
        placed(scene.active_mut());
        scene.add("Cube", primitives::build(Primitive::Cube, 300, 0.5), &mut slot);
        scene.select(first, &mut slot);
        scene.instance_active();
        scene.select(first, &mut slot);
        let center = Vec3::new(0.1, -0.2, 0.3);
        assert!(scene.set_mirror(first, 1 | 4, center));

        let path = dir.join("mirrored.sclp");
        save(&scene, &slot, &path);
        let back = open(&path);
        let rows = |s: &Scene| s.objects.iter().map(|o| o.mirror).collect::<Vec<_>>();
        assert_eq!(rows(&back), rows(&scene));
        assert_eq!(rows(&back).iter().filter(|m| m.is_some()).count(), 1);
        assert_eq!(back.objects[0].mirror, MirrorRepeater::new(5, center));
        assert_eq!(back.objects[0].placements().count(), 4, "three copies and the object, drawn again");
        // The instance is another object of the same mesh, and is not mirrored.
        let twin = back.objects.iter().find(|o| o.name == "Ball Instance").unwrap();
        assert!(twin.mirror.is_none() && back.users(twin.mesh) == 2);

        // A file written before live mirrors, or by a version that had none, has none.
        let old = dir.join("old.sclp");
        io::save_scene(&[&slot], &[saved_object("Old", 0)], 0, &old).unwrap();
        assert!(open(&old).objects.iter().all(|o| o.mirror.is_none()));

        // An entry that does not read leaves its object as it was, and costs the file nothing.
        let mut details = Details::default();
        details.set_object(0, slot::MIRROR_COPIES, &"not a mirror");
        details.set_object(
            1,
            slot::MIRROR_COPIES,
            &serde_json::json!({ "axes": 2, "center": [0.0, 1.0, 0.0], "later": true }),
        );
        let odd = dir.join("odd.sclp");
        io::save_scene_with(&[&slot], &[saved_object("A", 0), saved_object("B", 0)], 0, &details, &odd).unwrap();
        let loaded = io::load_scene(&odd).unwrap();
        assert_eq!(saved_mirror(&loaded.details, 0), None);
        assert_eq!(saved_mirror(&loaded.details, 1), MirrorRepeater::new(2, Vec3::Y));
        let _ = std::fs::remove_dir_all(&dir);
    }

    type Block = (String, Vec<Vec3>, Vec<[usize; 3]>);

    /// The name, vertices and faces of each `o` block of an OBJ, faces numbered from the block's own first vertex.
    fn objects_of(text: &str) -> Vec<Block> {
        let mut blocks: Vec<Block> = Vec::new();
        let (mut first, mut seen) = (0, 0);
        for line in text.lines() {
            let mut parts = line.split_ascii_whitespace();
            match parts.next() {
                Some("o") => {
                    first = seen;
                    blocks.push((parts.next().unwrap().to_owned(), Vec::new(), Vec::new()));
                }
                Some("v") => {
                    let c: Vec<f32> = parts.map(|n| n.parse().unwrap()).collect();
                    blocks.last_mut().unwrap().1.push(Vec3::new(c[0], c[1], c[2]));
                    seen += 1;
                }
                Some("f") => {
                    let at: Vec<usize> =
                        parts.map(|c| c.split('/').next().unwrap().parse::<usize>().unwrap() - 1 - first).collect();
                    blocks.last_mut().unwrap().2.push([at[0], at[1], at[2]]);
                }
                _ => {}
            }
        }
        blocks
    }

    fn volume(points: &[Vec3], faces: &[[usize; 3]]) -> f64 {
        faces
            .iter()
            .map(|f| points[f[0]].as_dvec3().dot(points[f[1]].as_dvec3().cross(points[f[2]].as_dvec3())) / 6.0)
            .sum()
    }

    #[test]
    fn an_export_writes_the_copies_too_each_the_right_way_out() {
        let dir = folder("export");
        let (mesh, mut scene) = (ball(), Scene::new("Ball"));
        let id = scene.active_id();
        placed(scene.active_mut());
        scene.set_mirror(id, 1, Vec3::ZERO);
        let object = scene.active().clone();
        let tris = mesh.triangle_count();
        let places = [("Ball", object.matrix()), ("Ball_MirrorX", object.placed_matrix(1))];

        // One block each, numbered on from the last, and both come out as solids the right way out.
        let obj = dir.join("both.obj");
        io::export_obj_placed(&mesh, &places, &obj).unwrap();
        let blocks = objects_of(&std::fs::read_to_string(&obj).unwrap());
        assert_eq!(blocks.iter().map(|b| b.0.as_str()).collect::<Vec<_>>(), ["Ball", "Ball_MirrorX"]);
        assert!(blocks.iter().all(|b| b.1.len() == mesh.vertex_count() && b.2.len() == tris));
        let (own, copy) = (volume(&blocks[0].1, &blocks[0].2), volume(&blocks[1].1, &blocks[1].2));
        assert!(own > 0.0 && (copy - own).abs() < 1e-4 * own, "volumes {own} and {copy}: a copy came out inside out");
        for (a, b) in blocks[0].1.iter().zip(&blocks[1].1) {
            assert!((*a * Vec3::new(-1.0, 1.0, 1.0)).distance(*b) < 1e-5, "{a} is not the mirror of {b}");
        }

        // One place is what it always was: no `o` line.
        let one = dir.join("one.obj");
        io::export_obj(&mesh, object.matrix(), &one).unwrap();
        let text = std::fs::read_to_string(&one).unwrap();
        assert!(!text.lines().any(|l| l.starts_with("o ")));
        assert_eq!(text.lines().filter(|l| l.starts_with("f ")).count(), tris);

        // The STL holds both, one after the other.
        let stl = dir.join("both.stl");
        let models: Vec<Mat4> = places.iter().map(|p| p.1).collect();
        io::export_stl_placed(&mesh, &models, &stl).unwrap();
        let bytes = std::fs::read(&stl).unwrap();
        assert_eq!(u32::from_le_bytes(bytes[80..84].try_into().unwrap()) as usize, 2 * tris);
        let facets: Vec<[Vec3; 3]> = bytes[84..]
            .chunks_exact(50)
            .map(|f| {
                let at = |k: usize| {
                    let c =
                        |i: usize| f32::from_le_bytes(f[12 + k * 12 + i * 4..16 + k * 12 + i * 4].try_into().unwrap());
                    Vec3::new(c(0), c(1), c(2))
                };
                [at(0), at(1), at(2)]
            })
            .collect();
        assert_eq!(facets.len(), 2 * tris);
        let solid = |half: &[[Vec3; 3]]| -> f64 {
            half.iter().map(|t| t[0].as_dvec3().dot(t[1].as_dvec3().cross(t[2].as_dvec3())) / 6.0).sum()
        };
        let (a, b) = (solid(&facets[..tris]), solid(&facets[tris..]));
        assert!(a > 0.0 && (a - b).abs() < 1e-4 * a, "{a} and {b}: a copy came out inside out");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The copies of a real array as the Array tool says them: a ring of six round a centre to the right of the
    /// object, turning as they go round and shrinking toward the last, then the same again through the object's
    /// mirror.
    fn ring_of_copies(scene: &Scene) -> Vec<Mat4> {
        use sculpt_array::{ArrayKind, ArraySettings, Pose, compute_layout, drawn_copies};
        let object = scene.active();
        let mut settings = ArraySettings::new(ArrayKind::Radial);
        (settings.count, settings.axis, settings.end_scale) = (6, 2, 0.5);
        let layout = compute_layout(&settings, &[Vec3::ZERO, Vec3::new(1.8, 0.0, 0.0)]);
        drawn_copies(&layout, &Pose::new(object.position, object.rotation, object.scale), object.mirror, 1)
    }

    /// Where a mesh's vertices are through a matrix.
    fn through(points: &[Vec3], m: Mat4) -> Vec<Vec3> {
        points.iter().map(|&p| m.transform_point3(p)).collect()
    }

    fn bounds(points: &[Vec3]) -> (Vec3, Vec3) {
        points.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(lo, hi), p| (lo.min(*p), hi.max(*p)))
    }

    #[test]
    fn an_export_of_an_arrayed_object_writes_every_copy_in_its_place_and_the_right_way_out() {
        let dir = folder("export array");
        let (mesh, mut scene) = (ball(), Scene::new("Ball"));
        let id = scene.active_id();
        placed(scene.active_mut());
        scene.set_mirror(id, 1, Vec3::ZERO);
        let copies = ring_of_copies(&scene);
        assert_eq!(copies.len(), 10, "five copies, and their reflections");
        scene.set_tool_places(std::collections::HashMap::from([(id, copies.clone())]));

        // What is written: the object, its mirror, then each copy of the run and the same through the mirror.
        let places = scene.export_places(scene.active());
        let names: Vec<&str> = places.iter().map(|p| p.0.as_str()).collect();
        assert_eq!(
            names,
            [
                "Ball", "Ball_MirrorX", "Ball_Array2", "Ball_Array3", "Ball_Array4", "Ball_Array5", "Ball_Array6",
                "Ball_Array2MirrorX",
                "Ball_Array3MirrorX",
                "Ball_Array4MirrorX",
                "Ball_Array5MirrorX",
                "Ball_Array6MirrorX",
            ]
        );
        let object = scene.active().clone();
        let models: Vec<Mat4> = places.iter().map(|p| p.1).collect();
        assert_eq!(models[0], object.matrix());
        assert_eq!(models[1], object.placed_matrix(1));
        assert_eq!(models[2..], copies[..]);
        assert_eq!(scene.export_note(&object), " with its mirror copy and its 10 array copies");

        // The object's own volume, and what each place must come to: the matrix scales volume by its determinant.
        let faces: Vec<[usize; 3]> =
            mesh.indices.chunks_exact(3).map(|t| [t[0] as usize, t[1] as usize, t[2] as usize]).collect();
        let local = volume(&mesh.positions, &faces);
        assert!(local > 0.0);
        let tris = mesh.triangle_count();

        let obj = dir.join("array.obj");
        let named: Vec<(&str, Mat4)> = places.iter().map(|(n, m)| (n.as_str(), *m)).collect();
        io::export_obj_placed(&mesh, &named, &obj).unwrap();
        let blocks = objects_of(&std::fs::read_to_string(&obj).unwrap());
        assert_eq!(blocks.iter().map(|b| b.0.as_str()).collect::<Vec<_>>(), names);
        for ((name, vertices, faces), model) in blocks.iter().zip(&models) {
            assert_eq!((vertices.len(), faces.len()), (mesh.vertex_count(), tris), "{name}");
            let wanted = local * model.determinant().abs() as f64;
            let got = volume(vertices, faces);
            let right = got > 0.0 && (got - wanted).abs() < 2e-3 * wanted;
            assert!(right, "{name}: volume {got}, wanted {wanted}: wound the wrong way out?");
            // Where it is: every vertex is the mesh's through the place's matrix (as written, to the digits OBJ keeps).
            for (v, expected) in vertices.iter().zip(through(&mesh.positions, *model)) {
                assert!(v.distance(expected) < 1e-4, "{name}: a vertex is {v}, not {expected}");
            }
        }
        // The reflected places really are reflections, so the check above is worth something.
        assert_eq!(models.iter().filter(|m| m.determinant() < 0.0).count(), 6);

        // The STL holds all twelve one after the other, in millimetres and Z up, each a solid.
        let stl = dir.join("array.stl");
        io::export_stl_placed(&mesh, &models, &stl).unwrap();
        let bytes = std::fs::read(&stl).unwrap();
        assert_eq!(u32::from_le_bytes(bytes[80..84].try_into().unwrap()) as usize, 12 * tris);
        let corner = |f: &[u8], k: usize| {
            let c = |i: usize| f32::from_le_bytes(f[12 + k * 12 + i * 4..16 + k * 12 + i * 4].try_into().unwrap());
            Vec3::new(c(0), c(1), c(2))
        };
        let facets: Vec<[Vec3; 3]> =
            bytes[84..].chunks_exact(50).map(|f| [corner(f, 0), corner(f, 1), corner(f, 2)]).collect();
        for (n, model) in models.iter().enumerate() {
            let half = &facets[n * tris..(n + 1) * tris];
            let signed: f64 =
                half.iter().map(|t| t[0].as_dvec3().dot(t[1].as_dvec3().cross(t[2].as_dvec3())) / 6.0).sum();
            // Millimetres: ten to the unit, so a thousand times the volume.
            let wanted = local * model.determinant().abs() as f64 * 1000.0;
            let right = signed > 0.0 && (signed - wanted).abs() < 2e-3 * wanted;
            assert!(right, "{}: STL volume {signed}, wanted {wanted}", names[n]);
            // And its box is the mesh's through the matrix, then to STL's axes: (x, y, z) -> (x, -z, y) x 10.
            let corners: Vec<Vec3> = half.iter().flatten().copied().collect();
            let expected: Vec<Vec3> =
                through(&mesh.positions, *model).iter().map(|p| Vec3::new(p.x, -p.z, p.y) * 10.0).collect();
            let (got, want) = (bounds(&corners), bounds(&expected));
            let same = got.0.distance(want.0) < 1e-2 && got.1.distance(want.1) < 1e-2;
            assert!(same, "{}: box {got:?}, wanted {want:?}", names[n]);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_export_without_an_array_is_what_it_was() {
        let mut scene = Scene::new("Ball");
        let id = scene.active_id();
        let object = scene.active().clone();
        assert_eq!(scene.export_places(&object).iter().map(|p| p.0.as_str()).collect::<Vec<_>>(), ["Ball"]);
        assert_eq!(scene.export_note(&object), "");
        scene.set_mirror(id, 1 | 2, Vec3::ZERO);
        let object = scene.active().clone();
        let places = scene.export_places(&object);
        assert_eq!(places.len(), 4);
        assert_eq!(scene.export_note(&object), " with its 3 mirror copies");
        // One copy, no mirror: a run of two.
        let mut scene = Scene::new("Cube Ball");
        let id = scene.active_id();
        scene.set_tool_places(std::collections::HashMap::from([(id, vec![Mat4::from_translation(Vec3::X)])]));
        let places = scene.export_places(&scene.active().clone());
        assert_eq!(places.iter().map(|p| p.0.as_str()).collect::<Vec<_>>(), ["Cube_Ball", "Cube_Ball_Array2"]);
        assert_eq!(scene.export_note(scene.active()), " with its array copy");
    }

    #[test]
    fn the_gizmos_target_and_placement_follow_the_side_that_is_selected() {
        let mut scene = Scene::new("Ball");
        let id = scene.active_id();
        placed(scene.active_mut());
        scene.set_mirror(id, 1, Vec3::ZERO);
        let object = scene.active().clone();

        // On the object: what the gizmo always had.
        let own = shown_target(&scene, &object, 0.4);
        assert_eq!(own.placement, Placement::of(&object));

        // On the copy: the copy's place, which a drag turns back into the object's.
        scene.select_side(id, 1);
        let target = shown_target(&scene, &object, 0.4);
        let copy = MirrorRepeater::new(1, Vec3::ZERO).unwrap().copies()[0];
        assert!(target.placement.position.distance(object.position * copy.signs) < 1e-6);
        assert!(target.turns && target.scales && target.radius == 0.4);
        let moved = Placement { position: target.placement.position + Vec3::new(-0.5, 0.0, 0.2), ..target.placement };
        place_shown(&mut scene, id, &moved);
        assert!(scene.get(id).unwrap().position.distance(object.position + Vec3::new(0.5, 0.0, 0.2)) < 1e-5);

        // Another object is placed as it always was.
        let mut slot = ball();
        let other = scene.add("Other", primitives::build(Primitive::Cube, 300, 0.5), &mut slot);
        let to = Placement { position: Vec3::new(2.0, 0.0, 0.0), ..Placement::of(scene.get(other).unwrap()) };
        place_shown(&mut scene, other, &to);
        assert_eq!(scene.get(other).unwrap().position, Vec3::new(2.0, 0.0, 0.0));
    }
}
