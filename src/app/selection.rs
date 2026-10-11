//! The app's side of working with several objects at once, and of the object
//! a brush lands on: adding to the selection from the list and the viewport,
//! joining the selection into one object or combining it with a boolean,
//! Reset Shape, new shapes at the Unity app's size and names, the question
//! the Delete key asks first, and sending a stroke through whichever
//! placement of the active mesh is under the pointer.

use std::f32::consts::FRAC_PI_4;
use std::time::Instant;

use glam::{Vec2, Vec3};
use sculpt_geom::TriMesh;
use sculpt_volume::{BooleanOp, BooleanSettings};
use winit::event::MouseButton;
use winit::keyboard::{KeyCode, ModifiersState};

use super::{Drag, Running};
use crate::doc::{Action, Tool};
use crate::history::History;
use crate::mesh::Mesh;
use crate::mesh::primitives::{self, Primitive};
use crate::printable::Cure;
use crate::remesh::{self, Notes};
use crate::render::flash;
use crate::scene::{Others, Scene};

/// What the Unity app says when the booleans have nothing to work on.
const NO_CUTTER: &str = "Select the target first, then Ctrl + click the other object.";
const MISS_SUBTRACT: &str = "Nothing changed - the cutter does not overlap the target - nothing to subtract.";
const MISS_INTERSECT: &str = "Nothing changed - the objects do not overlap - the intersection would be empty.";

/// What a boolean made, for the status line.
#[derive(Debug)]
pub(super) struct Combined {
    /// How many cutters went into it.
    pub others: usize,
    pub triangles: usize,
    /// What the engine did about objects that were not closed, and anything
    /// it could not finish.
    pub notes: Notes,
}

/// Subtract, Union or Intersect on the selection, as one undo step. The
/// active object is the target: it keeps its place and takes the result as a
/// mesh of its own, with a blank mask (no vertex of the new surface was in
/// the old one). The other selected objects are the cutters, and are hidden
/// or deleted afterwards as `others` says; undo puts the target's old mesh
/// back and the cutters with it.
///
/// It all happens in the target's own space, at `settings.resolution` voxels
/// along the target's longest side. An error means nothing was changed, and
/// says why in words for the status line.
pub(super) fn boolean(
    scene: &mut Scene,
    slot: &mut Mesh,
    history: &mut History,
    op: BooleanOp,
    settings: &BooleanSettings,
    others: Others,
) -> Result<Combined, String> {
    let (target, mut cutters) = scene.boolean_solids(slot).ok_or(NO_CUTTER)?;
    cutters.retain(|cutter| !cutter.is_empty());
    if cutters.is_empty() {
        return Err(NO_CUTTER.into());
    }
    // Refusing beats "succeeding" here: with no overlap a Subtract would only
    // resample the target and an Intersect erase it, and the user would be
    // left wondering which they got. Boxes are a cautious test: cutters that
    // come close without touching still just resample.
    let bounds = target.bounds();
    if op != BooleanOp::Union && !cutters.iter().any(|cutter| cutter.bounds().overlaps(&bounds)) {
        return Err(if op == BooleanOp::Subtract { MISS_SUBTRACT } else { MISS_INTERSECT }.into());
    }
    // The engine takes a handful of solids at a time. Several closed shells
    // in one mesh read as their union, which is all Subtract and Union ask
    // of the cutters, so any beyond that ride along in the last one.
    // (Intersect needs each on its own; past the limit the engine says so.)
    let most = sculpt_volume::boolean::MAX_SOLIDS - 1;
    if op != BooleanOp::Intersect && cutters.len() > most {
        for extra in cutters.split_off(most) {
            cutters[most - 1].append(&extra);
        }
    }
    let solids: Vec<&TriMesh> = cutters.iter().collect();
    // What the engine calls the first object is the one being kept, which has a name.
    let built = sculpt_volume::boolean_with(&target, &solids, op, settings)
        .map_err(|e| format!("Nothing changed. {}", remesh::named(e, &scene.active().name)))?;
    let notes = Notes::of(&built);
    let mesh = Mesh::new(built.mesh.positions, built.mesh.indices, None);
    let triangles = mesh.triangle_count();
    let before = scene.state();
    let count = scene.install_combined(mesh, others, slot);
    history.push_scene(before);
    Ok(Combined { others: count - 1, triangles, notes })
}

/// 1234567 as "1,234,567".
fn grouped(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// What a join made, for the status line.
pub(super) struct Joined {
    /// How many objects went into it.
    pub count: usize,
    pub triangles: usize,
    /// Why the remesh that was asked for did not run, if it did not.
    pub remesh_failed: Option<String>,
    /// What a remesh that ran did about parts that were not closed.
    pub notes: Notes,
}

/// Joins the selected objects into the active one, as one undo step: the
/// separate objects and their meshes come back whole on undo. `rebuild` is
/// the voxel resolution and mirror planes to remesh the result with, which
/// welds the shells into one surface (and, like any remesh, clears the mask).
/// A remesh that cannot run leaves the plain join. `None`, and nothing
/// changes, unless two or more objects are selected.
pub(super) fn join(
    scene: &mut Scene,
    slot: &mut Mesh,
    history: &mut History,
    rebuild: Option<(u32, [bool; 3])>,
) -> Option<Joined> {
    let mut mesh = scene.joined_mesh(slot)?;
    let (mut remesh_failed, mut notes) = (None, Notes::default());
    if let Some((resolution, symmetry)) = rebuild {
        match remesh::voxel_remesh_noted(&mesh, resolution, symmetry) {
            Ok((rebuilt, noted)) => (mesh, notes) = (rebuilt, noted),
            Err(e) => remesh_failed = Some(e),
        }
    }
    let triangles = mesh.triangle_count();
    let before = scene.state();
    let count = scene.install_joined(mesh, slot);
    history.push_scene(before);
    Some(Joined { count, triangles, remesh_failed, notes })
}

impl Running {
    /// Says how much is selected, when it is more than one object: nothing
    /// else on screen says which of them the brushes and a Join will take.
    fn selection_status(&mut self) {
        let scene = &self.doc.scene;
        let count = scene.selected().len();
        if count > 1 {
            let name = &scene.active().name;
            let text = format!("{count} objects selected. Brushes work on \"{name}\", and a Join keeps it.");
            self.status(text, true);
        }
    }

    /// Ctrl or Shift + click on a row of the list: adds that object to the
    /// selection.
    pub(super) fn select_also(&mut self, id: u64) {
        if self.doc.scene.select_also(id) {
            self.flash.start(id, flash::Kind::Select);
            self.selection_status();
        }
        self.redraw = true;
    }

    /// A click on an object in the viewport while the Transform tool is up.
    /// Plain, it selects that object alone. With Shift or Ctrl it adds the
    /// object to the selection, or takes it out again: both keys are free
    /// here, where no brush is listening for them.
    pub(super) fn transform_click(&mut self, id: u64) {
        let mods = self.input.mods;
        if mods.shift_key() || mods.control_key() {
            let doc = &mut self.doc;
            if doc.scene.toggle_selected(id, &mut doc.mesh) {
                self.scene_changed();
                if self.doc.scene.is_selected(id) {
                    self.flash.start(id, flash::Kind::Select);
                }
                self.selection_status();
            }
        } else if id != self.doc.scene.active_id() || self.doc.scene.selected().len() > 1 {
            // A click on the only selected object changes nothing.
            self.act(Action::SelectObject(id));
        }
    }

    /// Join: merges the selected objects into the active one.
    pub(super) fn join_selected(&mut self, rebuild: bool) {
        let started = Instant::now();
        let doc = &mut self.doc;
        // The object the join keeps is the one whose symmetry the result has.
        let mirrors = doc.scene.active_symmetry().exact_mirrors();
        let rebuild = rebuild.then(|| (doc.opts.remesh_resolution as u32, mirrors));
        let Some(joined) = join(&mut doc.scene, &mut doc.mesh, &mut doc.history, rebuild) else {
            self.status("Join needs two or more objects selected. Ctrl + click rows in the list to add them.", false);
            return;
        };
        doc.mesh_replaced = true;
        self.scene_changed();
        let name = &self.doc.scene.active().name;
        let Joined { count, triangles, remesh_failed, notes } = joined;
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        let mut text = format!("Joined {count} objects into \"{name}\" ({triangles} triangles) in {ms:.0} ms.");
        // A plain join keeps every hole its parts had; a remesh closes them.
        let remeshed = rebuild.is_some() && remesh_failed.is_none();
        text += &match (rebuild, remesh_failed) {
            (_, Some(why)) => {
                format!(" The remesh did not run ({}), so the parts are as they were.", remesh::named(why, name))
            }
            (Some(_), None) => format!(" It was remeshed into one surface.{}", notes.after_remesh()),
            (None, None) => String::new(),
        };
        text += " Undo (Z) brings the separate objects back.";
        self.status_checked(text, if remeshed { Cure::OtherResolution } else { Cure::Remesh });
        self.toast("Join");
        self.flash.start(self.doc.scene.active_id(), flash::Kind::Select);
    }

    /// Subtract, Union or Intersect: the active object is cut by the other
    /// selected objects, takes them in, or keeps only what it shares with
    /// them.
    pub(super) fn boolean_selected(&mut self, op: BooleanOp, resolution: u32, delete: bool) {
        let started = Instant::now();
        let doc = &mut self.doc;
        // Mirror-exact across the planes strokes on the kept object are
        // mirrored across, where the solids are: the same promise a remesh
        // makes.
        let mirror = doc.scene.active_symmetry().exact_mirrors();
        let settings = BooleanSettings { mirror, ..BooleanSettings::new(resolution) };
        let others = if delete { Others::Deleted } else { Others::Hidden };
        let made = match boolean(&mut doc.scene, &mut doc.mesh, &mut doc.history, op, &settings, others) {
            Ok(made) => made,
            Err(why) => return self.status(why, false),
        };
        doc.mesh_replaced = true;
        self.scene_changed();
        let (verb, name) = match op {
            BooleanOp::Subtract => ("Subtracted", "Subtract"),
            BooleanOp::Union => ("United", "Union"),
            BooleanOp::Intersect => ("Intersected", "Intersect"),
        };
        let (count, plural) = (made.others, if made.others == 1 { "" } else { "s" });
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        // The Unity app's line, then what it leaves unsaid: how to get things back.
        let text = if delete {
            format!(
                "{verb} {count} object{plural}, deleted. {} triangles in {ms:.0} ms. Undo (Z) brings back the shape and what was deleted.",
                grouped(made.triangles)
            )
        } else {
            format!(
                "{verb} {count} object{plural}, hidden. {} triangles in {ms:.0} ms. The eye in the list shows a hidden one again; undo (Z) brings back the shape and shows them.",
                grouped(made.triangles)
            )
        };
        self.status_checked(text + &made.notes.after_boolean(), Cure::OtherResolution);
        self.toast(name);
        self.flash.start(self.doc.scene.active_id(), flash::Kind::Select);
    }

    /// Reset Shape: puts the active mesh back to how it was before it was
    /// sculpted. One undo step.
    pub(super) fn reset_shape(&mut self) {
        let doc = &mut self.doc;
        let name = doc.scene.active().name.clone();
        match doc.scene.reset_shape(&mut doc.mesh) {
            Some(delta) => {
                doc.history.push_stroke(delta);
                doc.sculptor.last_hit = None;
                self.toast("Reset Shape");
                self.status(
                    format!("\"{name}\" is back to the shape it had before sculpting. Undo (Z) brings the sculpt back."),
                    true,
                );
            }
            None => self.status(format!("\"{name}\" has not been sculpted since it was made or last remeshed."), true),
        }
    }

    /// Adds a new shape at the centre, sized against the top object of the
    /// list and named by count, as the Unity app does. One undo step.
    pub(super) fn new_shape(&mut self, kind: Primitive) {
        let doc = &mut self.doc;
        let size = doc.scene.new_shape_size(&doc.mesh);
        let mesh = primitives::build(kind, doc.opts.new_shape_tris as usize, size);
        let name = doc.scene.spawn_name(kind.name());
        self.add_object(&name, mesh);
        let name = self.doc.scene.active().name.clone();
        self.status(format!("Added \"{name}\" at the centre. Undo (Z) removes it."), true);
    }

    /// The Delete key: asks before deleting the active object. Delete sits
    /// one key from Backspace, and an accidental press should not take a
    /// model out of the viewport unasked, undo or no undo.
    pub(super) fn delete_key(&mut self) {
        let scene = &self.doc.scene;
        if scene.objects.len() < 2 {
            self.status("The last object cannot be deleted.", false);
            return;
        }
        self.ui.graph.ask_delete(scene.active_id());
    }

    /// Keys while that question is up: Enter answers yes, Esc no, and every
    /// other key waits. True when the question took the key.
    pub(super) fn confirm_key(&mut self, code: KeyCode) -> bool {
        if !self.ui.graph.asking() {
            return false;
        }
        match code {
            KeyCode::Enter | KeyCode::NumpadEnter => {
                if let Some(action) = self.ui.graph.answer(true) {
                    self.act(action);
                }
            }
            KeyCode::Escape => {
                self.ui.graph.answer(false);
            }
            _ => {}
        }
        true
    }

    /// Whether a press at `pos` is a stroke on the active mesh, given the
    /// placement under the pointer (an object and which of its places, 0 for
    /// the object itself: see `Picked::copy`). If it is, the stroke is sent
    /// through the placement of the mesh that was pressed: the active object,
    /// any instance of it, or a mirror copy or an array copy of either, which
    /// show the same mesh somewhere else.
    pub(super) fn press_on_active(&mut self, picked: Option<(u64, u16)>, pos: Vec2) -> bool {
        self.doc.scene.release_brush();
        let on = |at: (u64, u16)| self.doc.scene.shows_active_mesh(at.0).then_some(at);
        let Some((target, copy)) = picked.and_then(|at| on(at).or_else(|| self.reach_past(pos))) else { return false };
        let doc = &mut self.doc;
        doc.scene.aim_brush(target, copy);
        self.local = self.view.local_to(doc.scene.brush_matrix());
        // The shape Reset Shape goes back to is the one before this stroke.
        // Mask strokes move nothing.
        if !doc.sculptor.settings.mask_mode {
            doc.scene.keep_baseline(&doc.mesh);
        }
        true
    }

    /// A press on another object that is in front of the active mesh. Mostly
    /// that press is the other object's, and sculpting what lies hidden
    /// behind it would leave a dab nobody could see. Not where the two meet,
    /// though: working clay into the join between them puts the middle of
    /// the brush on the other object as often as not. So the press still
    /// counts when the mesh lies no more than a brush radius behind and shows
    /// through somewhere inside the brush ring. Returns the placement of the
    /// mesh the stroke should go through. The Unity app's rule.
    fn reach_past(&self, pos: Vec2) -> Option<(u64, u16)> {
        let doc = &self.doc;
        let (scene, mesh) = (&doc.scene, doc.scene.active_mesh());
        let (origin, dir) = self.view.ray(pos);
        let other = scene.pick(origin, dir, &doc.mesh)?;
        let own = scene.pick_mesh(origin, dir, mesh, &doc.mesh)?;
        let ring = doc.sculptor.settings.radius_px;
        let radius = ring * self.view.world_per_pixel(self.view.depth_of(own.pos));
        if (own.pos - origin).dot(dir) - (other.pos - origin).dot(dir) > radius {
            return None;
        }
        // Two rings of eight points, at half the brush radius and at its edge.
        (1..=2).flat_map(|ring_no| (0..8).map(move |i| (ring_no, i))).find_map(|(ring_no, i)| {
            let angle = i as f32 * FRAC_PI_4;
            let at = pos + Vec2::new(angle.cos(), angle.sin()) * (ring * ring_no as f32 * 0.5);
            let (origin, dir) = self.view.ray(at);
            let seen = scene.pick(origin, dir, &doc.mesh)?;
            scene.shows_active_mesh(seen.id).then_some((own.id, own.copy))
        })
    }

    /// Whether the pointer is over the active mesh: the active object, an
    /// instance of it, or a live mirror copy or an array copy of either. Over one of those the
    /// brush cursor is worked out in that placement's space until the view is
    /// next rebuilt, so the ring sits on what is under the pointer. Runs each
    /// frame.
    pub(super) fn hover_on_active(&mut self) -> bool {
        let picked = self.pick_placed(self.input.pointer);
        if self.doc.sculptor.is_active() {
            // A stroke stays on the placement it began on.
            let scene = &self.doc.scene;
            return picked == Some((scene.brush_object().id, scene.brush_copy()));
        }
        if self.doc.scene.release_brush() {
            self.local = self.view.local_to(self.doc.scene.active().matrix());
        }
        let scene = &self.doc.scene;
        let mesh = scene.active_mesh();
        let Some((id, copy)) = picked else { return false };
        let Some(under) = scene.get(id).filter(|o| o.mesh == mesh) else { return false };
        if (id, copy) != (scene.active_id(), 0) {
            self.local = self.view.local_to(scene.place_matrix(under, copy));
        }
        true
    }

    /// Selects three of the demo scene's objects, so the list shows a
    /// selection of several; used with `--screenshot`. The environment
    /// variable `SCULPT_DEMO_SELECT` asks for more: `ask` also presses Delete,
    /// to show the question that asks, `local` turns Local View on for the
    /// three, `subtract`, `union` and `intersect` show that boolean of the
    /// cube and the sphere, `apart`, `aside` and `behind` ask for a union and
    /// a remesh that will not be started (`demo_too_big`), and `play` runs
    /// `selection_play` first.
    pub(super) fn demo_select(&mut self) {
        let extra = std::env::var("SCULPT_DEMO_SELECT").unwrap_or_default();
        if extra == "play" {
            self.selection_play();
        }
        let shown: Vec<u64> = self.doc.scene.objects.iter().filter(|o| o.visible).map(|o| o.id).collect();
        let pick = |i: usize| shown.get(i).or(shown.last()).copied();
        if let (Some(first), Some(second), Some(third)) = (pick(3), pick(5), pick(6)) {
            self.act(Action::SelectObject(first));
            self.select_also(second);
            self.select_also(third);
        }
        match extra.as_str() {
            "ask" => self.delete_key(),
            // Local View on the three, with the glide onto them finished.
            "local" => {
                self.local_view_key();
                while self.doc.camera.update(1.0 / 60.0) {}
                self.sync_view();
            }
            "subtract" => self.demo_boolean(BooleanOp::Subtract),
            "union" => self.demo_boolean(BooleanOp::Union),
            "intersect" => self.demo_boolean(BooleanOp::Intersect),
            "apart" | "aside" | "behind" => self.demo_too_big(&extra),
            _ => {}
        }
    }

    /// A job the engine will not start, at the highest resolution the sliders
    /// have, to see what the status line says of it. `apart`: the demo
    /// scene's cube united with its sphere sixty cube-lengths away on two
    /// axes. `aside`: the cube's own mesh moved forty lengths off its X
    /// mirror plane (as a piece split off a figure lies off the figure's),
    /// and remeshed there with Mirror X on. `behind`: the same on the other
    /// side of the plane, where a mirrored rebuild would keep nothing.
    fn demo_too_big(&mut self, what: &str) {
        let apart = what == "apart";
        let named = |app: &Running, name: &str| app.doc.scene.objects.iter().find(|o| o.name == name).map(|o| o.id);
        let (Some(cube), Some(ball)) = (named(self, "Cube"), named(self, "Sphere (1)")) else { return };
        self.act(Action::SelectObject(cube));
        let across = self.doc.mesh.bounds().size().max_element();
        if apart {
            let far = self.doc.scene.active().position + Vec3::new(60.0, 60.0, 0.0) * across;
            if let Some(o) = self.doc.scene.get_mut(ball) {
                o.position = far;
            }
            self.act(Action::SelectAlso(ball));
            self.act(Action::Boolean { op: BooleanOp::Union, resolution: 1024, delete: false });
        } else {
            let side = if what == "behind" { -1.0 } else { 1.0 };
            let aside = |p: &Vec3| *p + Vec3::X * 40.0 * across * side;
            let positions = self.doc.mesh.positions.iter().map(aside).collect();
            self.replace_mesh(Mesh::new(positions, self.doc.mesh.indices.clone(), None), true);
            self.doc.edit_symmetry(|symmetry| symmetry.mirror = [true, false, false]);
            self.doc.opts.remesh_resolution = 1024.0;
            self.act(Action::Remesh);
        }
    }

    /// The demo scene's sphere moved onto the near top corner of its cube and
    /// combined with it, and the view brought in on the result. The cube's
    /// instances behind it keep the shape they had.
    fn demo_boolean(&mut self, op: BooleanOp) {
        let named = |app: &Running, name: &str| app.doc.scene.objects.iter().find(|o| o.name == name).map(|o| o.id);
        let (Some(cube), Some(ball)) = (named(self, "Cube"), named(self, "Sphere (1)")) else { return };
        self.act(Action::SelectObject(cube));
        let half = self.doc.mesh.bounds().size() * 0.5;
        let corner = self.doc.scene.active().position + half * Vec3::new(-1.0, 1.0, 1.0);
        if let Some(o) = self.doc.scene.get_mut(ball) {
            o.position = corner;
        }
        self.act(Action::SelectAlso(ball));
        self.act(Action::Boolean { op, resolution: 128, delete: false });
        let aspect = self.view.size.x / self.view.size.y.max(1.0);
        let bounds = self.doc.scene.selection_bounds(&self.doc.mesh);
        self.doc.camera.frame(bounds, aspect, false);
        self.sync_view();
    }

    /// Works the selection, hide-key and brush-placement code of the demo
    /// scene the way a person would, through the real pointer and key
    /// routines, and prints what came of each step: a check that needs no
    /// one at the tablet.
    fn selection_play(&mut self) {
        fn say(what: &str, ok: bool) {
            println!("{} {what}", if ok { "ok  " } else { "FAIL" });
        }
        let named = |app: &Running, name: &str| {
            app.doc.scene.objects.iter().find(|o| o.name == name).map_or(0, |o| o.id)
        };
        // A pixel well inside what can be seen of an object.
        let pixel_on = |app: &Running, id: u64| -> Option<Vec2> {
            let size = app.view.size;
            let grid = (0..(size.y / 6.0) as usize)
                .flat_map(|j| (0..(size.x / 6.0) as usize).map(move |i| Vec2::new(i as f32, j as f32) * 6.0 + 3.0));
            let hits: Vec<Vec2> = grid.filter(|&p| app.pick(p) == Some(id)).collect();
            let middle = hits.iter().copied().sum::<Vec2>() / hits.len().max(1) as f32;
            hits.into_iter().min_by(|a, b| a.distance(middle).total_cmp(&b.distance(middle)))
        };
        let shown = |app: &Running| -> Vec<u64> {
            app.doc.scene.objects.iter().filter(|o| o.visible).map(|o| o.id).collect()
        };
        let settle = |app: &mut Running| while app.doc.camera.update(1.0 / 60.0) {};
        let (cube, twin) = (named(self, "Cube"), named(self, "Cube Instance"));
        let (ball, can, cone) = (named(self, "Sphere (1)"), named(self, "Cylinder"), named(self, "Cone"));
        self.input.inside = true;
        self.input.mods = ModifiersState::empty();
        self.act(Action::SelectObject(cube));
        self.sync_view();
        let settings = &mut self.doc.sculptor.settings;
        (settings.brush, settings.mask_mode) = (crate::sculpt::Brush::Inflate, false);
        settings.radius_px = 40.0;
        self.doc.edit_symmetry(|symmetry| symmetry.mirror = [false; 3]);

        // A stroke on an instance of the active mesh.
        let rest = self.doc.mesh.positions.clone();
        let steps = self.doc.history.steps().0;
        match pixel_on(self, twin) {
            Some(px) => {
                self.pointer_moved(px, None);
                self.sync_view();
                say("the pointer over an instance counts as over the active mesh", self.hover_on_active());
                let (origin, dir) = self.view.ray(px);
                let seen = self.doc.scene.pick(origin, dir, &self.doc.mesh).map(|p| p.pos);
                self.pointer_pressed(MouseButton::Left, None);
                say("a press on an instance begins a stroke", self.input.drag == Drag::Sculpt);
                say("which goes through the instance's placement", self.doc.scene.brush_object().id == twin);
                for i in 1..=15 {
                    self.pointer_moved(px + Vec2::new(i as f32 * 2.0, 0.0), None);
                }
                self.pointer_released(MouseButton::Left);
                let moved: Vec<Vec3> =
                    self.doc.mesh.positions.iter().zip(&rest).filter(|(a, b)| a != b).map(|(_, &b)| b).collect();
                let middle = moved.iter().copied().sum::<Vec3>() / moved.len().max(1) as f32;
                let placed = self.doc.scene.get(twin).map(|o| o.matrix().inverse());
                let expect = seen.zip(placed).map(|(p, m)| m.transform_point3(p));
                println!("     {} vertices moved, around {middle}; pressed at {expect:?}", moved.len());
                say(
                    "and moves the vertices seen under the pointer",
                    !moved.is_empty() && expect.is_some_and(|e| e.distance(middle) < 0.2),
                );
                say("as one undo step", self.doc.history.steps().0 == steps + 1);
                self.sync_view();
                say("Reset Shape is on offer", self.doc.scene.can_reset(&self.doc.mesh));
                self.act(Action::ResetShape);
                say("and puts the mesh back", self.doc.mesh.positions == rest);
                self.act(Action::Undo);
                say("until undo brings the sculpt back", self.doc.mesh.positions != rest);
            }
            None => say("an instance is in view to press on", false),
        }

        // A press on another object, with nothing of the active mesh near.
        match pixel_on(self, ball) {
            Some(px) => {
                self.pointer_moved(px, None);
                self.pointer_pressed(MouseButton::Left, None);
                say("a press on another object orbits and sculpts nothing", self.input.drag == Drag::Orbit);
                self.pointer_released(MouseButton::Left);
            }
            None => say("the sphere is in view to press on", false),
        }

        // The cylinder moved half into the cube: a press on the cylinder right
        // where the two meet is still a stroke on the cube.
        let centre = self.doc.scene.get(cube).map_or(Vec3::ZERO, |o| o.position);
        let half = self.doc.mesh.bounds().size().x * 0.5;
        if let Some(o) = self.doc.scene.get_mut(can) {
            o.position = centre + Vec3::new(half, 0.0, half * 0.66);
        }
        self.sync_view();
        // On the cylinder's wall, a little in front of the cube's front face.
        let (dx, r) = (-0.86 * half, half);
        let meet = centre + Vec3::new(half + dx, 0.0, half * 0.66 + (r * r - dx * dx).sqrt());
        match self.view.project(meet) {
            Some(px) => {
                self.pointer_moved(px, None);
                say("that spot shows the cylinder", self.pick(px) == Some(can));
                self.pointer_pressed(MouseButton::Left, None);
                say("a press where another object meets the active one still sculpts it", self.input.drag == Drag::Sculpt);
                self.pointer_released(MouseButton::Left);
            }
            None => say("where the cylinder meets the cube is in view", false),
        }

        // The hide keys and Local View.
        self.act(Action::SelectObject(ball));
        self.hide_key();
        let scene = &self.doc.scene;
        say(
            "H hides the selection and moves the brushes to an object that shows",
            scene.get(ball).is_some_and(|o| !o.visible) && scene.active().visible && scene.active_id() != ball,
        );
        self.input.mods = ModifiersState::ALT;
        self.hide_key();
        say("Alt + H shows everything", shown(self).len() == self.doc.scene.objects.len());
        self.act(Action::SelectObject(ball));
        self.input.mods = ModifiersState::SHIFT;
        self.hide_key();
        say("Shift + H leaves only the selection", shown(self) == [ball]);
        self.input.mods = ModifiersState::ALT;
        self.hide_key();
        self.input.mods = ModifiersState::empty();
        let (pivot, distance) = (self.doc.camera.pivot, self.doc.camera.distance);
        self.local_view_key();
        say(
            "/ solos the selection and glides onto it",
            self.doc.scene.in_local_view() && shown(self) == [ball] && self.doc.camera.is_animating(),
        );
        settle(self);
        say("the view ends up on it", self.doc.camera.pivot.distance(self.doc.scene.active().position) < 1e-3);
        self.local_view_key();
        settle(self);
        let camera = &self.doc.camera;
        say(
            "/ again shows the rest and puts the view back",
            shown(self).len() == self.doc.scene.objects.len()
                && camera.pivot.distance(pivot) < 1e-4
                && (camera.distance - distance).abs() < 1e-4,
        );

        // The question the Delete key asks.
        let count = self.doc.scene.objects.len();
        self.delete_key();
        say("Delete asks first", self.ui.graph.asking() && self.doc.scene.objects.len() == count);
        say("the question holds other keys", self.confirm_key(KeyCode::KeyR) && self.ui.graph.asking());
        self.confirm_key(KeyCode::Escape);
        say("Esc takes it down", !self.ui.graph.asking() && self.doc.scene.objects.len() == count);
        self.delete_key();
        self.confirm_key(KeyCode::Enter);
        say("Enter deletes", self.doc.scene.objects.len() == count - 1 && self.doc.scene.get(ball).is_none());
        self.act(Action::Undo);
        say("and undo restores it", self.doc.scene.objects.len() == count && self.doc.scene.active_id() == ball);

        // Clicks in the viewport with the Transform tool up.
        self.set_tool(Tool::Transform);
        self.act(Action::SelectObject(cube));
        self.input.mods = ModifiersState::SHIFT;
        self.transform_click(ball);
        say("Shift + click adds to the selection", self.doc.scene.selected() == [cube, ball]);
        self.transform_click(cube);
        let scene = &self.doc.scene;
        say("and takes the active one out, promoting the next", scene.selected() == [ball] && scene.active_id() == ball);
        self.input.mods = ModifiersState::empty();
        self.transform_click(cube);
        say("a plain click selects one alone", self.doc.scene.selected() == [cube]);
        self.set_tool(Tool::Sculpt);

        // Join, and new shapes.
        self.act(Action::SelectObject(ball));
        self.act(Action::SelectAlso(cone));
        self.act(Action::Join { remesh: false });
        let scene = &self.doc.scene;
        say("Join leaves one object where there were two", scene.objects.len() == count - 1 && scene.active_id() == ball);
        self.act(Action::Undo);
        let scene = &self.doc.scene;
        say("and one undo brings both back", scene.objects.len() == count && scene.selected() == [ball, cone]);
        self.act(Action::New(Primitive::Cube));
        let across = self.doc.mesh.bounds().size().x;
        println!("     new shape \"{}\", {across} across", self.doc.scene.active().name);
        say("a new cube is named by count and sized to the top object", self.doc.scene.active().name == "Cube (1)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::Next;

    fn cube() -> Mesh {
        primitives::build(Primitive::Cube, 2_400, 0.5)
    }

    fn volume(mesh: &Mesh) -> f64 {
        mesh.indices
            .chunks_exact(3)
            .map(|t| {
                let p = [0, 1, 2].map(|c| mesh.positions[t[c] as usize].as_dvec3());
                p[0].dot(p[1].cross(p[2])) / 6.0
            })
            .sum()
    }

    #[test]
    fn join_is_one_undo_step_with_or_without_the_remesh() {
        for rebuild in [None, Some((64, [false; 3]))] {
            let mut mesh = cube();
            let mut scene = Scene::new("Cube");
            let mut history = History::default();
            let first = scene.active_id();
            // Nothing to join yet, and no undo step for trying.
            assert!(join(&mut scene, &mut mesh, &mut history, rebuild).is_none());
            assert!(!history.can_undo());

            // A second cube half inside the first.
            let second = scene.add("Cube", cube(), &mut mesh);
            scene.get_mut(second).unwrap().position.x = 0.5;
            scene.select(first, &mut mesh);
            scene.select_also(second);

            let joined = join(&mut scene, &mut mesh, &mut history, rebuild).expect("two are selected");
            assert_eq!((joined.count, joined.triangles), (2, mesh.triangle_count()));
            assert!(joined.remesh_failed.is_none());
            // Two closed cubes that overlap are not a model with holes, and are not called one.
            assert_eq!(joined.notes, Notes::default());
            assert!(crate::printable::census(&mesh).is_closed());
            assert_eq!(scene.objects.len(), 1);
            assert_eq!(scene.active_id(), first);
            match rebuild {
                // Plain: both shells whole, overlap and all.
                None => assert!((volume(&mesh) - 2.0).abs() < 1e-4),
                // Remeshed: one surface around the two, 1.5 units of it.
                Some(_) => assert!((volume(&mesh) - 1.5).abs() < 0.03, "volume {}", volume(&mesh)),
            }

            assert_eq!(history.steps(), (1, 0));
            assert_eq!(history.next_undo(), Some(Next::Scene));
            scene.collect(|id| history.keeps_mesh(id));
            assert!(history.step_scene(true, &mut scene, &mut mesh));
            assert_eq!(scene.objects.len(), 2);
            assert_eq!(scene.selected(), [first, second]);
            assert!((volume(&mesh) - 1.0).abs() < 1e-5);
        }
    }

    /// Voxels along the target for the boolean tests, and the lattice cell
    /// that gives on a cube one unit across.
    const RESOLUTION: u32 = 64;
    const CELL: f64 = 1.0 / RESOLUTION as f64;
    /// How far off a volume may be: a skin a tenth of a cell thick over ten
    /// square units, more surface than any result here has.
    const TOLERANCE: f64 = 10.0 * 0.1 * CELL;

    /// A cube to cut and a second one to cut it with, both selected, in a
    /// frame that is moved, turned and twice the size. `at` is where the
    /// second sits in the first one's own space, in which both are one unit
    /// across. Returns the scene, the active mesh, the target and the cutter.
    fn pair(at: Vec3) -> (Scene, Mesh, u64, u64) {
        let turn = glam::Quat::from_rotation_y(0.4) * glam::Quat::from_rotation_x(0.2);
        let mut mesh = cube();
        let mut scene = Scene::new("Cube");
        let target = scene.active_id();
        let object = scene.active_mut();
        (object.position, object.rotation, object.scale) = (Vec3::new(1.0, 2.0, 3.0), turn, Vec3::splat(2.0));
        let frame = object.matrix();
        let cutter = scene.add("Cube", cube(), &mut mesh);
        let object = scene.get_mut(cutter).unwrap();
        (object.position, object.rotation, object.scale) = (frame.transform_point3(at), turn, Vec3::splat(2.0));
        scene.select(target, &mut mesh);
        scene.select_also(cutter);
        (scene, mesh, target, cutter)
    }

    #[test]
    fn booleans_of_two_cubes_have_the_volumes_they_should_and_undo_in_one_step() {
        // The cutter covers a 0.6 x 0.7 x 0.8 corner of the target.
        let shared = 0.6 * 0.7 * 0.8;
        let cases = [
            (BooleanOp::Union, 2.0 - shared),
            (BooleanOp::Subtract, 1.0 - shared),
            (BooleanOp::Intersect, shared),
        ];
        for (op, want) in cases {
            for others in [Others::Hidden, Others::Deleted] {
                let (mut scene, mut mesh, target, cutter) = pair(Vec3::new(0.4, 0.3, 0.2));
                let mut history = History::default();
                // The target has a mask and has been sculpted on before.
                mesh.mask.fill(1.0);
                scene.keep_baseline(&mesh);
                let (old, verts) = (scene.active_mesh(), mesh.vertex_count());
                let placed = scene.active().matrix();

                let settings = BooleanSettings::new(RESOLUTION);
                let made = boolean(&mut scene, &mut mesh, &mut history, op, &settings, others).unwrap();
                assert_eq!((made.others, made.triangles), (1, mesh.triangle_count()));
                assert_eq!(made.notes, Notes::default(), "two closed cubes need nothing done for them");
                assert!(crate::printable::census(&mesh).is_closed());
                // The surface is rebuilt to within a fraction of a cell, so
                // the volume is right to a thin skin over a few faces.
                let found = volume(&mesh);
                assert!((found - want).abs() < TOLERANCE, "{op:?}: {found} against {want}");

                // The target keeps its place, alone in the selection, with a
                // mesh of its own: blank mask, and at rest as it is.
                assert_eq!((scene.active_id(), scene.selected()), (target, &[target][..]));
                assert_eq!(scene.active().matrix(), placed);
                assert_ne!(scene.active_mesh(), old);
                assert!(mesh.mask.iter().all(|&m| m == 0.0));
                assert!(!scene.can_reset(&mesh));
                match others {
                    Others::Hidden => assert!(scene.get(cutter).is_some_and(|o| !o.visible)),
                    Others::Deleted => assert!(scene.get(cutter).is_none()),
                }

                // One undo: the old mesh, mask and resting shape, and the
                // cutter back in view and in the selection.
                assert_eq!(history.steps(), (1, 0));
                scene.collect(|id| history.keeps_mesh(id));
                assert!(history.step_scene(true, &mut scene, &mut mesh));
                assert_eq!((scene.active_mesh(), mesh.vertex_count()), (old, verts));
                assert!((volume(&mesh) - 1.0).abs() < 1e-5);
                assert!(mesh.mask.iter().all(|&m| m == 1.0));
                assert!(scene.can_reset(&mesh));
                assert!(scene.get(cutter).is_some_and(|o| o.visible));
                assert_eq!((scene.active_id(), scene.selected()), (target, &[target, cutter][..]));

                // And redo does it again.
                scene.collect(|id| history.keeps_mesh(id));
                assert!(history.step_scene(false, &mut scene, &mut mesh));
                assert!((volume(&mesh) - want).abs() < TOLERANCE);
                assert!(!scene.get(cutter).is_some_and(|o| o.visible));
            }
        }
    }

    /// A cube with the faces round its `+X` side taken out.
    fn open_cube() -> Mesh {
        let whole = cube();
        let kept = whole.indices.chunks_exact(3).filter(|t| t.iter().any(|&v| whole.positions[v as usize].x < 0.49));
        let indices: Vec<u32> = kept.flatten().copied().collect();
        assert!(indices.len() < whole.indices.len());
        Mesh::new(whole.positions.clone(), indices, None)
    }

    #[test]
    fn a_boolean_and_a_join_say_when_a_part_had_holes() {
        // The cutter is open on the side away from the target.
        let (mut scene, mut mesh, target, cutter) = pair(Vec3::new(0.4, 0.3, 0.2));
        scene.select(cutter, &mut mesh);
        mesh = open_cube();
        scene.select(target, &mut mesh);
        scene.select_also(cutter);
        let mut history = History::default();
        let settings = BooleanSettings::new(RESOLUTION);
        let made = boolean(&mut scene, &mut mesh, &mut history, BooleanOp::Subtract, &settings, Others::Hidden).unwrap();
        assert_eq!(made.notes, Notes { capped: 1, ..Notes::default() });
        assert_eq!(made.notes.after_boolean(), " One of the objects had holes, which were closed over flat first.");
        // Capped, the cutter is the cube it was: the same corner comes out.
        assert!((volume(&mesh) - (1.0 - 0.6 * 0.7 * 0.8)).abs() < TOLERANCE, "volume {}", volume(&mesh));
        assert!(crate::printable::census(&mesh).is_closed());

        // Joined plainly the hole is still there; joined with a remesh it is closed, and said to have been.
        for rebuild in [None, Some((64, [false; 3]))] {
            let mut mesh = open_cube();
            let mut scene = Scene::new("Cube");
            let first = scene.active_id();
            let second = scene.add("Cube", cube(), &mut mesh);
            scene.get_mut(second).unwrap().position.x = -0.5;
            scene.select(first, &mut mesh);
            scene.select_also(second);
            let joined = join(&mut scene, &mut mesh, &mut History::default(), rebuild).unwrap();
            let closed = crate::printable::census(&mesh).is_closed();
            match rebuild {
                None => assert!(joined.notes == Notes::default() && !closed),
                Some(_) => assert!(joined.notes == Notes { capped: 1, ..Notes::default() } && closed),
            }
        }
    }

    #[test]
    fn a_cutter_that_misses_changes_nothing_and_says_so() {
        let (mut scene, mut mesh, target, _) = pair(Vec3::new(3.0, 0.0, 0.0));
        let mut history = History::default();
        let (uid, before) = (mesh.uid, scene.state());
        let settings = BooleanSettings::new(RESOLUTION);
        // Word for word what the Unity app says.
        let said = [
            (BooleanOp::Subtract, "Nothing changed - the cutter does not overlap the target - nothing to subtract."),
            (BooleanOp::Intersect, "Nothing changed - the objects do not overlap - the intersection would be empty."),
        ];
        for (op, message) in said {
            let refused = boolean(&mut scene, &mut mesh, &mut history, op, &settings, Others::Hidden);
            assert_eq!(refused.unwrap_err(), message);
            assert_eq!((mesh.uid, scene.state()), (uid, before.clone()));
            assert!(!history.can_undo());
        }

        // A union needs no overlap: both cubes, as one object of two shells.
        let made = boolean(&mut scene, &mut mesh, &mut history, BooleanOp::Union, &settings, Others::Hidden).unwrap();
        assert_eq!(made.others, 1);
        assert!((volume(&mesh) - 2.0).abs() < TOLERANCE);

        // With only the target selected there is nothing to combine it with.
        scene.select(target, &mut mesh);
        let alone = boolean(&mut scene, &mut mesh, &mut history, BooleanOp::Subtract, &settings, Others::Hidden);
        assert_eq!(alone.unwrap_err(), "Select the target first, then Ctrl + click the other object.");
        assert_eq!(history.steps(), (1, 0));
    }

    /// Jobs the engine will not start for the memory they would take, at the
    /// highest setting the Voxel Resolution slider has: nothing is changed,
    /// nothing goes on the undo history, and the line says what to do.
    #[test]
    fn a_boolean_too_big_to_start_changes_nothing_and_says_what_to_do() {
        let highest = BooleanSettings::new(1024);
        // United with a cube 60 lengths away along two axes: the grid would have to cover all between.
        let (mut scene, mut mesh, _, _) = pair(Vec3::new(60.0, 60.0, 0.0));
        let mut history = History::default();
        let (uid, before) = (mesh.uid, scene.state());
        let refused = boolean(&mut scene, &mut mesh, &mut history, BooleanOp::Union, &highest, Others::Hidden);
        let said = refused.unwrap_err();
        let start = "Nothing changed. The objects are too far apart to unite at this resolution: the grid has to \
                     reach across all of them. Move them closer together, or lower the resolution to ";
        assert!(said.starts_with(start) && said.ends_with(" or less."), "{said}");
        assert_eq!((mesh.uid, scene.state()), (uid, before));
        assert!(!history.can_undo());

        // Cut mirrored, with the kept object's own mesh 40 lengths from its mirror plane: it is named.
        let (mut scene, mut mesh, _, _) = pair(Vec3::new(40.4, 0.3, 0.2));
        mesh.positions.iter_mut().for_each(|p| p.x += 40.0);
        let (uid, before) = (mesh.uid, scene.state());
        let mirrored = BooleanSettings { mirror: [true, false, false], ..highest };
        for op in [BooleanOp::Subtract, BooleanOp::Intersect] {
            let said = boolean(&mut scene, &mut mesh, &mut history, op, &mirrored, Others::Hidden).unwrap_err();
            let start = "Nothing changed. \"Cube\" is about 40 times its own size away from its X mirror plane. ";
            assert!(said.starts_with(start), "{op:?}: {said}");
            assert!(said.contains(" Turn the X mirror off for this, or lower the resolution to "), "{op:?}: {said}");
            assert_eq!((mesh.uid, scene.state()), (uid, before.clone()));
            assert!(!history.can_undo());
        }
        // With the mirror off the same cut is an ordinary one.
        let plain = BooleanSettings::new(RESOLUTION);
        boolean(&mut scene, &mut mesh, &mut history, BooleanOp::Subtract, &plain, Others::Hidden).unwrap();
        assert!((volume(&mesh) - (1.0 - 0.6 * 0.7 * 0.8)).abs() < TOLERANCE, "volume {}", volume(&mesh));
    }

    #[test]
    fn a_boolean_leaves_instances_outside_the_selection_as_they_were() {
        let (mut scene, mut mesh, target, cutter) = pair(Vec3::new(0.4, 0.3, 0.2));
        let mut history = History::default();
        let shared = scene.active_mesh();
        let twin = scene.instance_active();
        scene.select(target, &mut mesh);
        scene.select_also(cutter);
        let settings = BooleanSettings::new(RESOLUTION);
        boolean(&mut scene, &mut mesh, &mut history, BooleanOp::Subtract, &settings, Others::Hidden).unwrap();
        assert_ne!(scene.active_mesh(), shared);
        let twin = scene.get(twin).expect("the instance is still there");
        assert!(twin.visible && twin.mesh == shared);
        let kept = scene.mesh(shared, &mesh).expect("and still has its mesh");
        assert!((volume(kept) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn more_cutters_than_the_engine_takes_at_once_still_subtract() {
        // A small cube on each corner of the target: eight cutters, one more
        // than the engine combines in one go, each taking a 0.1 cube away.
        let mut mesh = cube();
        let mut scene = Scene::new("Cube");
        let mut history = History::default();
        let target = scene.active_id();
        let mut cutters = Vec::new();
        for corner in 0..8 {
            let side = |bit: usize| if corner >> bit & 1 == 0 { -0.5 } else { 0.5 };
            let id = scene.add("Cube", cube(), &mut mesh);
            let object = scene.get_mut(id).unwrap();
            (object.position, object.scale) = (Vec3::new(side(0), side(1), side(2)), Vec3::splat(0.2));
            cutters.push(id);
        }
        scene.select(target, &mut mesh);
        for &id in &cutters {
            scene.select_also(id);
        }
        let settings = BooleanSettings::new(RESOLUTION);

        // Intersect cannot fold them together, and says what to do instead.
        let refused = boolean(&mut scene, &mut mesh, &mut history, BooleanOp::Intersect, &settings, Others::Hidden);
        assert!(refused.unwrap_err().starts_with("Nothing changed. That is too many objects at once."));
        assert!(!history.can_undo());

        let made = boolean(&mut scene, &mut mesh, &mut history, BooleanOp::Subtract, &settings, Others::Hidden).unwrap();
        assert_eq!(made.others, 8);
        let found = volume(&mesh);
        assert!((found - (1.0 - 8.0 * 0.001)).abs() < TOLERANCE, "volume {found}");
        assert!(cutters.iter().all(|&id| scene.get(id).is_some_and(|o| !o.visible)));
        assert_eq!(history.steps(), (1, 0));
    }

    #[test]
    fn big_counts_read_in_threes() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1_000), "1,000");
        assert_eq!(grouped(12_345), "12,345");
        assert_eq!(grouped(1_234_567), "1,234,567");
    }
}
