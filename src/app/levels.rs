//! Subdivision levels, on the app's side: the Subdivision card's buttons and
//! their keys (Ctrl + D, Page Up, Page Down).
//!
//! What a level is and how a mesh holds its levels is `mesh/levels.rs`; the
//! geometry is the `sculpt-multires` crate. Here is how the running app
//! carries the commands out.
//!
//! - **A change of level is a new mesh in the object's place**, recorded as
//!   one undo step that keeps the old mesh whole, exactly as a remesh is. The
//!   old mesh carries its own levels, so Undo and Redo are a swap of the two.
//!   Deleting levels changes nothing of the mesh, and is a step that keeps
//!   only the levels as they were.
//! - **Whatever else puts a new mesh in an object's place drops the levels**
//!   (remesh, decimate, a boolean, a join, a mesh back from Blender, a tool
//!   that rebuilds the object...). None of them has to know about levels: the
//!   new mesh was built without any, and the old one, with its levels, is
//!   what their own undo step keeps. Nothing is asked. What is owed is a
//!   line saying so, because levels gone without a word would look like a
//!   bug the next time Page Down did nothing: `levels_watch` looks once a
//!   frame for an object whose mesh was replaced and whose levels went with
//!   it, and adds that line to whatever the status line says. Sculpt layers
//!   (`app/layers.rs`) go the same way, merged into the shape the new mesh
//!   was built from, and the same line says so.
//! - **Everything else reads the level that is current**: brushes, the mask,
//!   hiding, export, Send to Blender, the printing check, a measure. They
//!   see an ordinary mesh.
//! - **Dynamic topology is off for an object with levels** (`Mesh::refine`
//!   does nothing on one), and the Remesh card says so.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Instant;

use glam::{Mat4, Quat, Vec2, Vec3};
use sculpt_multires::Split;
use serde_json::Value;
use sculpt_volume::BooleanOp;

use super::Running;
use crate::doc::{Action, MaskOp, Tool};
use crate::io;
use crate::mesh::primitives::{self, Primitive};
use crate::mesh::{Mesh, levels};
use crate::printable::grouped;
use crate::sculpt::{Brush, PointerSample};

/// What the frame before saw: how many edits the history had recorded, and
/// for each object with subdivision levels or sculpt layers, its mesh
/// (`Mesh::uid`), how many levels and how many layers.
#[derive(Default)]
struct Seen {
    edits: u64,
    levels: HashMap<u64, (u64, usize, usize)>,
}

/// "A" / "A and B", in the order of the names.
fn listed(mut names: Vec<String>) -> String {
    names.sort();
    names.join(" and ")
}

/// What to say about the objects whose meshes were rebuilt: `levels` names
/// those that lost subdivision levels and `layers` those whose sculpt layers
/// were merged into the model. One sentence where the two lists are the same
/// objects, which is the usual case of one object that had both.
fn rebuilt_note(levels: Vec<String>, layers: Vec<String>, can_undo: bool) -> String {
    let many = |names: &[String]| names.len() > 1;
    let rebuilt = |names: &[String]| if many(names) { "their meshes were rebuilt" } else { "its mesh was rebuilt" };
    let whose = |names: &[String]| if many(names) { "their" } else { "its" };
    let undo = if can_undo { " Undo (Z) brings them back." } else { "" };
    let (same, both) = (levels == layers, !levels.is_empty() && !layers.is_empty());
    let dropped = |names: Vec<String>| {
        format!("The subdivision levels of {} were dropped: {}.", listed(names.clone()), rebuilt(&names))
    };
    let merged = |names: Vec<String>| {
        format!("The sculpt layers of {} were merged into the model: {}.", listed(names.clone()), rebuilt(&names))
    };
    let said = match (both, same) {
        (true, true) => format!(
            "The subdivision levels of {} were dropped and {} sculpt layers merged into the model: {}.",
            listed(levels.clone()),
            whose(&levels),
            rebuilt(&levels)
        ),
        (true, false) => format!("{} {}", dropped(levels), merged(layers)),
        (false, _) if !levels.is_empty() => dropped(levels),
        (false, _) => merged(layers),
    };
    said + undo
}

thread_local! {
    /// Kept here and not in `Running`, like the dynamic topology demo's
    /// samples: it is this module's own note between frames, and nothing
    /// else reads it.
    static SEEN: RefCell<Seen> = RefCell::new(Seen::default());
}

/// A mesh's subdivision levels as the bridge's `get_state` reports them:
/// which level it is on (1 is the coarsest) and the triangles of each. Null
/// for an object without levels.
pub(super) fn state_of(mesh: Option<&Mesh>) -> Value {
    let Some(levels) = mesh.and_then(|mesh| mesh.levels.as_ref()) else { return Value::Null };
    let triangles: Vec<usize> = (0..levels.count()).map(|level| levels.triangle_count(level)).collect();
    let quads = levels.scheme() == sculpt_multires::Scheme::CatmullClark;
    serde_json::json!({ "current": levels.current() + 1, "count": levels.count(), "triangles": triangles, "quads": quads })
}

/// "level 2 of 4": levels are numbered from 1 where a person reads them.
fn place(mesh: &Mesh) -> String {
    let (level, count) = mesh.levels.as_ref().map_or((0, 1), |l| (l.current(), l.count()));
    format!("level {} of {count}", level + 1)
}

/// A second object, selected along with the active one, for the commands
/// that take two.
pub(super) fn second_object(app: &mut Running) {
    let first = app.doc.scene.active_id();
    app.act(Action::New(Primitive::Cube));
    app.doc.scene.active_mut().position = Vec3::new(0.35, 0.0, 0.0);
    let cube = app.doc.scene.active_id();
    app.act(Action::SelectObject(first));
    app.act(Action::SelectAlso(cube));
}

/// Everything that puts a new mesh in the active object's place, for the
/// scripted checks: what it is called, how many objects it needs selected
/// (see [`second_object`]), and how to run it.
pub(super) fn rebuilders() -> [(&'static str, usize, fn(&mut Running)); 10] {
    [
        ("Remesh", 1, |app| {
            app.doc.opts.remesh_resolution = 48.0;
            app.act(Action::Remesh);
            app.remesh_wait();
        }),
        ("Quad Remesh", 1, |app| {
            app.doc.opts.quad_target = 400.0;
            app.act(Action::QuadRemesh)
        }),
        ("Decimate", 1, |app| {
            app.doc.opts.decimate_target = 2_000.0;
            app.act(Action::DecimateAnyway);
            app.decimate_wait();
        }),
        ("Cut & Mirror", 1, |app| {
            app.act(Action::CutAndMirror { axis: sculpt_symmetry::Axis::X, from_positive: true })
        }),
        ("Join", 2, |app| app.act(Action::Join { remesh: false })),
        ("Join with a remesh", 2, |app| {
            app.doc.opts.remesh_resolution = 48.0;
            app.act(Action::Join { remesh: true })
        }),
        ("Boolean", 2, |app| app.act(Action::Boolean { op: BooleanOp::Subtract, resolution: 48, delete: false })),
        ("A mesh back from Blender", 1, |app| {
            // What the Blender link does with an object that comes back worked on.
            let id = app.doc.scene.active_id();
            let mesh = primitives::build(Primitive::Cone, 900, 0.5);
            app.edit_scene(|scene, slot| scene.replace_mesh_of(id, mesh, slot));
            app.doc.mesh_replaced = true;
            app.status("\"Sphere\" came back from Blender.", true);
        }),
        ("Trim (a tool that rebuilds the object)", 1, |app| {
            app.tools_demo("trim");
            app.act(Action::Tool(Tool::Sculpt));
        }),
        ("Split", 1, |app| {
            app.tools_demo("split");
            app.act(Action::Tool(Tool::Sculpt));
        }),
    ]
}

impl Running {
    /// Every object with subdivision levels or sculpt layers, as `Seen` keeps them.
    fn levels_now(&self) -> HashMap<u64, (u64, usize, usize)> {
        let scene = &self.doc.scene;
        let with_levels = |o: &crate::scene::Object| {
            let mesh = scene.mesh(o.mesh, &self.doc.mesh)?;
            let (levels, layers) = (mesh.level_count(), mesh.layer_count());
            (levels > 1 || layers > 0).then(|| (o.id, (mesh.uid, levels, layers)))
        };
        scene.objects.iter().filter_map(with_levels).collect()
    }

    /// Takes note of the levels as they are, with nothing to say about how
    /// they came to be: after this module's own commands.
    fn levels_seen(&mut self) {
        let seen = Seen { edits: self.doc.history.edits, levels: self.levels_now() };
        SEEN.with(|kept| *kept.borrow_mut() = seen);
    }

    /// Once a frame: if an edit recorded since the last look put a new mesh
    /// in the place of one that had subdivision levels or sculpt layers,
    /// says that the levels were dropped and the layers merged into the
    /// model, after whatever the edit itself said. Undo and Redo record
    /// nothing, so what comes and goes with them is not news; nor is an
    /// object that is gone, levels and all.
    pub(super) fn levels_watch(&mut self) {
        let now = self.levels_now();
        let edits = self.doc.history.edits;
        let was = SEEN.with(|kept| std::mem::replace(&mut *kept.borrow_mut(), Seen { edits, levels: now }));
        if was.edits == edits {
            return;
        }
        let scene = &self.doc.scene;
        // For each object seen with levels or layers whose mesh is another now: its name, and which it lost.
        let lost = |(id, (uid, levels, layers)): (&u64, &(u64, usize, usize))| {
            let object = scene.get(*id)?;
            let mesh = scene.mesh(object.mesh, &self.doc.mesh)?;
            let gone = (*levels > 1 && mesh.level_count() == 1, *layers > 0 && mesh.layer_count() == 0);
            (mesh.uid != *uid && (gone.0 || gone.1)).then(|| (format!("\"{}\"", object.name), gone))
        };
        let mut lost: Vec<(String, (bool, bool))> = was.levels.iter().filter_map(lost).collect();
        if lost.is_empty() {
            return;
        }
        lost.sort();
        let names = |which: fn(&(bool, bool)) -> bool| -> Vec<String> {
            lost.iter().filter(|(_, gone)| which(gone)).map(|(name, _)| name.clone()).collect()
        };
        let note = rebuilt_note(names(|gone| gone.0), names(|gone| gone.1), self.doc.history.can_undo());
        let text = format!("{} {note}", self.ui.status.0);
        self.ui.status.0 = text.trim_start().to_owned();
        self.ui.status_seen = crate::ui::status::Seen::at(self.time());
        self.redraw = true;
    }

    /// Subdivide: a finer level above the finest, and the object moves to it.
    pub(super) fn subdivide(&mut self) {
        let started = Instant::now();
        let split = if self.doc.opts.subdivide_flat { Split::Flat } else { Split::Smooth };
        // Without what dynamic topology's merges left: levels number every vertex.
        let tidy = crate::scene::tidied(&self.doc.mesh);
        let (mesh, note) = match levels::subdivided(tidy.as_ref().unwrap_or(&self.doc.mesh), split) {
            Ok(made) => made,
            Err(why) => return self.status(why, false),
        };
        let name = self.doc.scene.active().name.clone();
        let mut text = format!(
            "Subdivided \"{name}\" to {}: {} triangles, in {:.0} ms.",
            place(&mesh),
            grouped(mesh.triangle_count()),
            started.elapsed().as_secs_f64() * 1000.0
        );
        if note.quads_set_aside {
            text += " Its quads were not its triangles cut up, so it was subdivided as triangles and has no quads now.";
        }
        self.replace_mesh(mesh, true);
        self.levels_seen();
        self.toast("Subdivide");
        self.status(text, true);
    }

    /// Page Up and Page Down: `by` levels finer, or coarser if negative.
    pub(super) fn level_by(&mut self, by: i32) {
        let Some((level, top)) = self.doc.mesh.levels.as_ref().map(|l| (l.current(), l.top())) else {
            return self.status("This object has no subdivision levels yet. Subdivide (Ctrl + D) adds one.", true);
        };
        let to = (level as i64 + by as i64).clamp(0, top as i64) as usize;
        if to == level {
            let text = if by > 0 {
                "This is the finest level. Subdivide (Ctrl + D) adds a finer one."
            } else {
                "This is the coarsest level."
            };
            return self.status(text, true);
        }
        self.level_to(to);
    }

    /// Moves the active object to subdivision level `level` (0 is the
    /// coarsest). What was sculpted on the level it leaves goes with it.
    pub(super) fn level_to(&mut self, level: usize) {
        let started = Instant::now();
        let Some(from) = self.doc.mesh.levels.as_ref().map(|l| l.current()) else {
            return self.status("This object has no subdivision levels yet. Subdivide (Ctrl + D) adds one.", true);
        };
        if level == from {
            return;
        }
        let (mesh, carried) = match levels::stepped(&self.doc.mesh, level) {
            Ok(stepped) => stepped,
            Err(why) => return self.status(why, false),
        };
        let name = self.doc.scene.active().name.clone();
        let mut text = format!(
            "\"{name}\" is on {}: {} triangles ({:.0} ms).",
            place(&mesh),
            grouped(mesh.triangle_count()),
            started.elapsed().as_secs_f64() * 1000.0
        );
        if carried {
            text += &format!(" What was sculpted on level {} was carried into the finer levels.", from + 1);
        }
        self.replace_mesh(mesh, true);
        self.levels_seen();
        self.toast(format!("Level {}", level + 1));
        self.status(text, true);
    }

    /// Delete Lower and Delete Higher: drops the levels below the one the
    /// active object is on, or above it. The mesh on screen does not change.
    pub(super) fn delete_levels(&mut self, lower: bool) {
        let which = if lower { "lower" } else { "higher" };
        let Some((level, count)) = self.doc.mesh.levels.as_ref().map(|l| (l.current(), l.count())) else {
            return self.status("This object has no subdivision levels to delete.", true);
        };
        let gone = if lower { level } else { count - 1 - level };
        if gone == 0 {
            return self.status(format!("There are no {which} levels: this is {}.", place(&self.doc.mesh)), true);
        }
        let fewer = levels::without(&self.doc.mesh, lower);
        let doc = &mut self.doc;
        doc.history.push_levels(std::mem::replace(&mut doc.mesh.levels, fewer));
        self.levels_seen();
        self.redraw = true;
        let name = self.doc.scene.active().name.clone();
        let left = match &self.doc.mesh.levels {
            Some(_) => format!("It is on {} now.", place(&self.doc.mesh)),
            None => "It has no levels now: it is an ordinary mesh.".to_owned(),
        };
        let levels = if gone == 1 { "level" } else { "levels" };
        self.toast(if lower { "Delete Lower" } else { "Delete Higher" });
        self.status(
            format!("Deleted the {gone} {which} {levels} of \"{name}\". {left} Undo (Z) brings them back."),
            true,
        );
    }

    // ---- for the Claude bridge -------------------------------------------------

    /// Runs something that reports through the status line and gives back
    /// what it said: as an error if it said it in red.
    pub(super) fn said(&mut self, run: impl FnOnce(&mut Running)) -> Result<String, String> {
        let before = std::mem::take(&mut self.ui.status);
        run(self);
        if self.ui.status.0.is_empty() {
            self.ui.status = before;
            return Ok("Done.".into());
        }
        let (text, colour) = self.ui.status.clone();
        if colour == crate::ui::theme::ERROR { Err(text) } else { Ok(text) }
    }

    /// The bridge's commands for subdivision levels, by the name its
    /// `command` tool was given with the underscores taken out. `None` if
    /// `name` is not one of them.
    pub(super) fn levels_command(&mut self, name: &str, args: &Value) -> Option<Result<String, String>> {
        let action = match name {
            "subdivide" => {
                if let Some(flat) = args.get("flat").and_then(Value::as_bool) {
                    self.doc.opts.subdivide_flat = flat;
                }
                Action::Subdivide
            }
            "level" => {
                let count = self.doc.mesh.level_count();
                let wanted = args.get("level").and_then(Value::as_f64).map(|level| level.round());
                match wanted.filter(|level| (1.0..=count as f64).contains(level)) {
                    Some(level) => Action::Level(level as usize - 1),
                    None => {
                        let has = if count == 1 { "no subdivision levels".to_owned() } else { format!("levels 1 to {count}") };
                        return Some(Err(format!("level needs a level number; the active object has {has}.")));
                    }
                }
            }
            "levelup" => Action::LevelBy(1),
            "leveldown" => Action::LevelBy(-1),
            "deletelowerlevels" => Action::DeleteLevels { lower: true },
            "deletehigherlevels" => Action::DeleteLevels { lower: false },
            "reconstructsubdiv" => Action::ReconstructSubdiv,
            _ => return None,
        };
        Some(self.said(|app| app.act(action)))
    }

    // ---- for visual checks ---------------------------------------------------

    /// One scripted stroke across the view, in pixels from its middle at a
    /// window 900 high, through the routines a pen stroke runs.
    pub(super) fn levels_stroke(&mut self, brush: Brush, strength: f32, radius: f32, from: Vec2, to: Vec2) {
        self.sync_view();
        let (middle, k) = (self.view.origin + self.view.size * 0.5, self.view.size.y / 900.0);
        let settings = &mut self.doc.sculptor.settings;
        settings.brush = brush;
        settings.radius_px = radius * k;
        settings.per_brush[brush.index()].strength = strength;
        let count = 48;
        let sample = |i: usize| PointerSample {
            pos: middle + from.lerp(to, i as f32 / count as f32) * k,
            pressure: None,
            time: i as f64 / 240.0,
        };
        let doc = &mut self.doc;
        if !doc.begin_stroke(&self.local, sample(0), false, false) {
            return;
        }
        for i in 1..=count {
            doc.sculptor.sample(&mut doc.mesh, &self.local, sample(i));
            if i % 4 == 0 {
                doc.sculptor.frame(&mut doc.mesh, &self.local, 1.0 / 60.0);
            }
        }
        if let Some(delta) = doc.sculptor.end(&mut doc.mesh, &self.local) {
            doc.history.push_stroke(delta);
        }
    }

    /// A coarse ball in place of the scene's object, with nothing to undo.
    pub(super) fn levels_ball(&mut self, triangles: usize) {
        self.replace_mesh(primitives::build(Primitive::Sphere, triangles, 0.5), false);
        self.doc.edit_symmetry(|symmetry| symmetry.mirror = [false; 3]);
        self.scene_changed();
        self.frame_view();
        self.doc.camera.update(10.0);
        self.sync_view();
        self.levels_seen();
    }

    /// Fine detail on the front of the model: creases, ridges and a few lumps.
    pub(super) fn levels_detail(&mut self) {
        for (row, y) in [-150.0, -75.0, 0.0, 75.0, 150.0].into_iter().enumerate() {
            let brush = if row % 2 == 0 { Brush::Crease } else { Brush::Standard };
            self.levels_stroke(brush, 0.8, 9.0, Vec2::new(-190.0, y), Vec2::new(190.0, y + 30.0));
        }
        for x in [-120.0, 0.0, 120.0] {
            self.levels_stroke(Brush::Inflate, 0.7, 14.0, Vec2::new(x, -200.0), Vec2::new(x + 6.0, -196.0));
        }
    }

    /// A big change of form made on the level in hand, as the Move brush
    /// would make it with a wide radius: the top of the model leans over and
    /// one side swells. Every vertex moves by rule, so the run is the same
    /// each time; it is recorded as the one stroke it stands for.
    fn levels_bend(&mut self) {
        let doc = &mut self.doc;
        let verts: Vec<u32> = (0..doc.mesh.vertex_count() as u32).collect();
        let before = doc.mesh.positions.clone();
        let bent = |p: Vec3| {
            let up = ((p.y + 0.1) / 0.6).clamp(0.0, 1.0);
            let lean = Quat::from_rotation_z(-0.9 * up * up);
            let swell = 1.0 + 0.5 * (-((p - Vec3::new(-0.45, -0.1, 0.1)).length_squared()) / 0.08).exp();
            lean * (p * swell) + Vec3::new(0.25 * up, 0.0, 0.0)
        };
        let after: Vec<Vec3> = before.iter().map(|&p| bent(p)).collect();
        let delta = crate::mesh::StrokeDelta::Positions { verts, before, after, topology: None };
        doc.mesh.apply_delta(&delta, false);
        doc.history.push_stroke(delta);
        self.redraw = true;
    }

    /// `--demo-levels <what>`: `play` works subdivision levels through the
    /// routines the card and the keys run and prints a line per check
    /// (`levels_play`). Any other word sets up a picture, each one stage
    /// further through the same piece of work:
    ///
    ///   - `base`: a coarse ball, one level, its triangles showing.
    ///   - `fine`: subdivided three times and sculpted with fine detail.
    ///   - `down`: stepped down to the coarsest level.
    ///   - `bent`: a big form changed there.
    ///   - `up`: stepped back up to the finest: the detail is on the new form.
    ///   - `card`: the same, with the Mesh tab and its Subdivision card showing.
    ///   - `quads`: a quad remesh subdivided twice, wireframe on.
    pub(super) fn levels_demo(&mut self, what: &str) {
        if what == "play" {
            return self.levels_play();
        }
        self.doc.opts.tabs.sculpt = crate::ui::tabs::Sculpt::Mesh;
        self.levels_ball(1_280);
        if what == "quads" {
            self.doc.opts.quad_target = 300.0;
            self.act(Action::QuadRemesh);
            self.act(Action::Subdivide);
            self.act(Action::Subdivide);
            return self.levels_seen();
        }
        let stage = ["base", "fine", "down", "bent", "up", "card"].iter().position(|s| *s == what).unwrap_or(5);
        if stage >= 1 {
            (0..3).for_each(|_| self.act(Action::Subdivide));
            self.levels_detail();
        }
        if stage >= 2 {
            self.act(Action::Level(0));
        }
        if stage >= 3 {
            self.levels_bend();
        }
        if stage >= 4 {
            self.act(Action::LevelBy(3));
        }
        // The coarse levels are looked at with their triangles showing.
        self.doc.opts.wireframe = matches!(stage, 0 | 2 | 3);
        self.doc.sculptor.settings.brush = Brush::Clay;
        self.frame_view();
        self.doc.camera.update(10.0);
        self.sync_view();
        println!("{}", self.ui.status.0);
        self.levels_seen();
    }

    /// The print of the active mesh on its levels (`MeshLevels::print`), or
    /// 0 for a mesh without levels.
    fn levels_print(&self) -> u64 {
        self.doc.mesh.levels.as_ref().map_or(0, |levels| levels.print(&self.doc.mesh))
    }

    /// `--demo-levels play`: every command of the Subdivision card, and
    /// everything else that meets a mesh with levels, worked in the real app
    /// with a line printed per check (ok / FAIL) and the status line each
    /// step leaves.
    fn levels_play(&mut self) {
        fn say(what: &str, ok: bool) {
            println!("{} {what}", if ok { "ok  " } else { "FAIL" });
        }
        let folder = crate::prefs::scratch("sculpt_levels_demo");
        let _ = std::fs::remove_dir_all(&folder);
        if let Err(e) = std::fs::create_dir_all(&folder) {
            return println!("FAIL could not make {}: {e}", folder.display());
        }
        self.levels_ball(1_280);
        let base = self.doc.mesh.triangle_count();

        // ---- the card's own commands
        self.act(Action::LevelBy(-1));
        say("Page Down on an object without levels says what to do", self.ui.status.0.contains("Subdivide (Ctrl + D)"));
        self.act(Action::Subdivide);
        println!("     {}", self.ui.status.0);
        self.act(Action::Subdivide);
        let mesh = &self.doc.mesh;
        say(
            "Subdivide twice makes three levels and leaves the object on the finest",
            mesh.level_count() == 3 && mesh.triangle_count() == base * 16,
        );
        say("each is one undo step", self.doc.history.steps() == (2, 0));
        self.levels_detail();
        let strokes = self.doc.history.steps().0 - 2;
        let sculpted = self.levels_print();
        self.act(Action::LevelBy(-1));
        println!("     {}", self.ui.status.0);
        say("Page Down goes one level coarser", self.doc.mesh.triangle_count() == base * 4);
        self.act(Action::Level(0));
        say(
            "a row of the card goes straight to its level",
            self.doc.mesh.triangle_count() == base && self.ui.status.0.contains("level 1 of 3"),
        );
        self.act(Action::LevelBy(-1));
        say(
            "Page Down at the bottom says so and does nothing",
            self.ui.status.0.contains("coarsest") && self.doc.history.steps().0 == strokes + 4,
        );
        self.act(Action::Subdivide);
        say(
            "Subdivide from a lower level is refused in words",
            self.ui.status.0.contains("finest level") && self.doc.mesh.level_count() == 3,
        );
        self.act(Action::LevelBy(5));
        say("Page Up past the top stops at the finest level", self.doc.mesh.triangle_count() == base * 16);
        say("down and back up with nothing edited gives back every bit", self.levels_print() == sculpted);
        self.act(Action::LevelBy(1));
        say("Page Up at the top says how to add a level", self.ui.status.0.contains("Subdivide (Ctrl + D)"));

        // ---- undo and redo of level changes
        (0..3).for_each(|_| self.act(Action::Undo));
        say(
            "three Undos go back through the three changes of level",
            self.levels_print() == sculpted && self.doc.history.steps().1 == 3,
        );
        (0..3).for_each(|_| self.act(Action::Redo));
        say("and three Redos forward again", self.levels_print() == sculpted && self.doc.history.steps().1 == 0);

        // ---- a big form changed low, the detail carried up
        self.act(Action::Level(0));
        let low = self.levels_print();
        self.levels_bend();
        self.act(Action::LevelBy(2));
        println!("     {}", self.ui.status.0);
        say(
            "a stroke on the coarsest level is carried up",
            self.ui.status.0.contains("carried into the finer levels") && self.levels_print() != sculpted,
        );
        let bent = self.levels_print();
        (0..2).for_each(|_| self.act(Action::Undo));
        say("Undo of the step up and of the stroke gives the coarse level back as it was", self.levels_print() == low);
        (0..2).for_each(|_| self.act(Action::Redo));
        say("Redo lands on the same sculpt", self.levels_print() == bent);
        (0..3).for_each(|_| self.act(Action::Undo));
        say("and Undo once more is the fine level before any of it", self.levels_print() == sculpted);

        // ---- the mask and hidden faces
        let hide: Vec<u32> = (0..self.doc.mesh.triangle_count() as u32).filter(|t| t % 5 == 0).collect();
        self.act(Action::HideFaces { triangles: hide.clone(), hide: true });
        self.act(Action::Mask(MaskOp::Invert));
        let dressed = self.levels_print();
        self.act(Action::Level(0));
        say("fully masked stays fully masked on the coarsest level", self.doc.mesh.mask.iter().all(|&m| m == 1.0));
        say("a coarse face shows unless all of it was hidden", self.doc.mesh.hidden_triangle_count() < base / 5);
        self.act(Action::Level(2));
        say(
            "back up, the mask and the hidden faces are as they were",
            self.levels_print() == dressed && self.doc.mesh.hidden_triangle_count() == hide.len(),
        );
        (0..4).for_each(|_| self.act(Action::Undo));
        say("four Undos clear them again", self.levels_print() == sculpted);

        // ---- dynamic topology
        self.doc.sculptor.dyntopo.enabled = true;
        self.doc.sculptor.dyntopo.detail = crate::sculpt::dyntopo::MIN_DETAIL;
        let triangles = self.doc.mesh.triangle_count();
        self.levels_stroke(Brush::Standard, 0.6, 60.0, Vec2::new(-100.0, 20.0), Vec2::new(100.0, 20.0));
        say(
            "a stroke with dynamic topology on adds no triangles to an object with levels",
            self.doc.mesh.triangle_count() == triangles && self.levels_print() != sculpted,
        );
        self.act(Action::Undo);
        self.doc.sculptor.dyntopo.enabled = false;

        // ---- deleting levels
        self.act(Action::Level(1));
        self.act(Action::DeleteLevels { lower: true });
        println!("     {}", self.ui.status.0);
        let mesh = &self.doc.mesh;
        say(
            "Delete Lower leaves two levels with this one the coarsest",
            mesh.level_count() == 2
                && mesh.levels.as_ref().is_some_and(|l| l.current() == 0)
                && mesh.triangle_count() == base * 4,
        );
        self.act(Action::DeleteLevels { lower: true });
        say("with nothing lower it says so", self.ui.status.0.contains("no lower levels"));
        self.act(Action::DeleteLevels { lower: false });
        say("Delete Higher after it leaves an ordinary mesh", self.doc.mesh.levels.is_none());
        self.act(Action::Undo);
        self.act(Action::Undo);
        say("two Undos bring all three levels back", self.doc.mesh.level_count() == 3);
        self.act(Action::Redo);
        self.act(Action::Level(1));
        say("with the lower level deleted the finer one still holds the sculpt", {
            let now = self.doc.mesh.levels.as_ref().is_some_and(|l| l.count() == 2 && l.current() == 1);
            now && self.doc.mesh.triangle_count() == base * 16
        });
        (0..3).for_each(|_| self.act(Action::Undo));
        say("back on the finest of three levels", self.levels_print() == sculpted);

        self.levels_play_destroyers(sculpted);
        self.levels_play_keepers(sculpted);
        self.levels_play_files(&folder, sculpted);
        println!("{}", self.ui.status.0);
    }

    /// Everything that puts a new mesh in the object's place: each drops the
    /// levels as part of its own undo step, says so, and Undo brings them
    /// back exactly.
    fn levels_play_destroyers(&mut self, sculpted: u64) {
        fn say(what: &str, ok: bool) {
            println!("{} {what}", if ok { "ok  " } else { "FAIL" });
        }
        let ball = self.doc.scene.active_id();
        for (name, objects, run) in rebuilders() {
            let steps = self.doc.history.steps().0;
            if objects == 2 {
                second_object(self);
            }
            run(self);
            self.doc.scene.select(ball, &mut self.doc.mesh);
            self.scene_changed();
            self.levels_watch();
            println!("     {}", self.ui.status.0);
            let dropped = self.doc.mesh.levels.is_none() && self.doc.mesh.triangle_count() > 0;
            let told = self.ui.status.0.contains("subdivision levels of \"Sphere\" were dropped");
            say(&format!("{name} drops the levels and says so"), dropped && told);
            let mut undos = 0;
            while self.doc.history.steps().0 > steps && undos < 8 {
                self.act(Action::Undo);
                undos += 1;
            }
            self.doc.scene.select(ball, &mut self.doc.mesh);
            self.scene_changed();
            let said = self.ui.status.0.clone();
            self.levels_watch();
            let back = self.doc.mesh.level_count() == 3 && self.levels_print() == sculpted;
            say(
                &format!("Undo of {name} brings them back exactly, with nothing said"),
                back && self.ui.status.0 == said,
            );
        }
    }

    /// What does not rebuild the mesh leaves the levels alone, and a copy of
    /// the object keeps them.
    fn levels_play_keepers(&mut self, sculpted: u64) {
        fn say(what: &str, ok: bool) {
            println!("{} {what}", if ok { "ok  " } else { "FAIL" });
        }
        let ball = self.doc.scene.active_id();
        let steps = self.doc.history.steps().0;
        self.levels_stroke(Brush::Clay, 0.5, 40.0, Vec2::new(-80.0, 0.0), Vec2::new(80.0, 0.0));
        self.act(Action::ResetShape);
        self.act(Action::Mask(MaskOp::Invert));
        self.act(Action::Mask(MaskOp::Clear));
        self.levels_watch();
        say("a stroke, Reset Shape and the mask commands keep the levels", self.doc.mesh.level_count() == 3);
        // What Set Origin to Mesh does when the centre is off the mesh: the
        // mesh is rebuilt with every vertex shifted, under a new number.
        let uid = self.doc.mesh.uid;
        self.edit_scene(|scene, slot| {
            scene.move_origin_by(Vec3::new(0.3, 0.1, 0.0), slot);
            true
        });
        self.doc.mesh_replaced = true;
        self.levels_watch();
        let rebuilt = self.doc.mesh.uid != uid && self.levels_print() != sculpted;
        say(
            "Set Origin to Mesh rebuilds the mesh and keeps them",
            rebuilt && self.doc.mesh.level_count() == 3 && !self.ui.status.0.contains("dropped"),
        );
        let shifted = self.levels_print();
        self.act(Action::Level(0));
        self.act(Action::Level(2));
        say("and they still work: down and up is the shifted sculpt, to the bit", self.levels_print() == shifted);
        while self.doc.history.steps().0 > steps {
            self.act(Action::Undo);
        }
        self.doc.scene.select(ball, &mut self.doc.mesh);
        say("and Undo of all that is the sculpt again", self.levels_print() == sculpted);

        self.act(Action::InstanceObject);
        say("an instance shares the levels", self.doc.mesh.level_count() == 3 && self.levels_print() == sculpted);
        self.act(Action::MakeUnique);
        say(
            "made unique, it has its own, the same",
            self.doc.mesh.level_count() == 3 && self.levels_print() == sculpted,
        );
        self.act(Action::Level(0));
        self.act(Action::SelectObject(ball));
        say("stepping the copy down leaves the original where it was", self.levels_print() == sculpted);
        self.act(Action::CloneObject);
        say("a copy of the object keeps the levels", self.levels_print() == sculpted);
        while self.doc.history.steps().0 > steps {
            self.act(Action::Undo);
        }
        self.doc.scene.select(ball, &mut self.doc.mesh);
        self.scene_changed();
        say("the scene is as it was", self.doc.scene.objects.len() == 1 && self.levels_print() == sculpted);
    }

    /// Saving, a recovery copy, and export.
    fn levels_play_files(&mut self, folder: &std::path::Path, sculpted: u64) {
        fn say(what: &str, ok: bool) {
            println!("{} {what}", if ok { "ok  " } else { "FAIL" });
        }
        let print_of = |mesh: &Mesh| mesh.levels.as_ref().map_or(0, |levels| levels.print(mesh));
        for level in [2, 0] {
            self.act(Action::Level(level));
            let now = self.levels_print();
            let file = folder.join(format!("level{level}.sclp"));
            let saved = self.save_scene(&file);
            let opened = io::load_scene(&file);
            let same = opened.as_ref().is_ok_and(|scene| scene.meshes.len() == 1 && print_of(&scene.meshes[0]) == now);
            say(
                &format!("saved on level {} and opened again, the levels are the same to the bit", level + 1),
                saved.is_ok() && same,
            );
            // What a recovery copy is: the same scene through the same writer (`keep_copy`).
            let copy = folder.join(format!("recovery{level}.sclp"));
            let written = self.scene_for_file().and_then(|(meshes, objects, active)| {
                io::save_scene_with(&meshes, &objects, active, &self.details(), &copy)
            });
            let restored = io::load_scene(&copy);
            let same = restored.as_ref().is_ok_and(|scene| print_of(&scene.meshes[0]) == now);
            say(&format!("a recovery copy made on level {} keeps them", level + 1), written.is_ok() && same);
            if let Ok(mut scene) = opened {
                // The opened mesh is a working stack: up to the finest level, it is the sculpt.
                let mesh = scene.meshes.remove(0);
                let top = if level == 2 { Ok((mesh, false)) } else { levels::stepped(&mesh, 2) };
                say(
                    "and stepping the opened model to the finest level gives the sculpt",
                    top.is_ok_and(|(mesh, _)| print_of(&mesh) == sculpted),
                );
            }
        }
        // Export writes the level in hand.
        let faces = |path: &std::path::Path| {
            let text = std::fs::read_to_string(path).unwrap_or_default();
            let sizes: Vec<usize> =
                text.lines().filter(|l| l.starts_with("f ")).map(|l| l.split_whitespace().count() - 1).collect();
            (sizes.len(), sizes.iter().filter(|&&corners| corners == 4).count())
        };
        let obj = folder.join("level1.obj");
        let exported = io::export_obj_placed(&self.doc.mesh, &[("Sphere", Mat4::IDENTITY)], &obj);
        say("export writes the level in hand", exported.is_ok() && faces(&obj).0 == self.doc.mesh.triangle_count());
        self.act(Action::Level(2));

        // Quads: a quad remesh subdivided keeps quads, and exports them.
        self.doc.opts.quad_target = 300.0;
        self.act(Action::QuadRemesh);
        let quads = self.doc.mesh.polys.as_ref().map_or(0, |p| p.start.len() - 1);
        self.act(Action::Subdivide);
        println!("     {}", self.ui.status.0);
        let now = self.doc.mesh.polys.as_ref().map_or(0, |p| p.start.len() - 1);
        say(
            "a quad remesh subdivides into quads",
            quads > 0 && now >= quads * 4 - 40 && now <= quads * 4 + 40 && self.doc.mesh.triangle_count() == now * 2,
        );
        let obj = folder.join("quads.obj");
        let exported = io::export_obj_placed(&self.doc.mesh, &[("Sphere", Mat4::IDENTITY)], &obj);
        say("and exports them as quads at the level in hand", exported.is_ok() && faces(&obj) == (now, now));
        self.act(Action::Level(0));
        say(
            "its coarsest level is the quad remesh itself",
            self.doc.mesh.polys.as_ref().map_or(0, |p| p.start.len() - 1) == quads,
        );
        self.act(Action::Level(1));
    }
}

mod reconstruct;

#[cfg(test)]
mod tests;
