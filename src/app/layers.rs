//! Sculpt layers, on the app's side: what the Layers card asks for.
//!
//! What a layer is, and what every kind of edit does to one, is
//! `mesh/layers.rs`. Here is how the running app carries the card's commands
//! out and records them for Undo.
//!
//! - **Every command is one undo step** that keeps the layers and the
//!   positions it changed, and puts exactly those back (`history::Edit::Layers`).
//!   A drag of a Strength slider is one step however far it went: the first
//!   move opens it, every frame the slider is held puts the shape where the
//!   slider is (worked out from where it was at the press, not from the frame
//!   before), and letting go closes it. Anything else that happens while one
//!   is open closes it first (`layers_before`), so a step never straddles
//!   another.
//! - **A stroke that would move the surface is refused while the layer that
//!   records is switched off or at 0%**, with a line saying why: there would
//!   be nothing on screen to see it by. Painting a mask is not refused.
//! - **A stroke with no layer recording is told what kind of brush made it**
//!   (`ride_of`, from `Doc::begin_stroke`): the Pose brush turns the layers
//!   with what it turns, and every other brush leaves them standing as they
//!   stood. `mesh/layers.rs` says why that is the difference that matters.
//! - **Strength, the eye and Delete are refused below the finest subdivision
//!   level**, with a line saying to step up (`Mesh::layer_strength_refusal`
//!   says why).
//! - **Whatever puts a new mesh in the object's place merges the layers into
//!   it**: the new mesh is built from the shape as it is, and has no layers.
//!   The old mesh, with its layers, is what that command's own undo step
//!   keeps. `levels_watch` adds the line that says so.

use std::cell::RefCell;

use glam::{Quat, Vec2, Vec3};
use serde_json::Value;

use super::Running;
use super::levels::{rebuilders, second_object};
use crate::doc::{Action, LayerAction, MaskOp};
use crate::io;
use crate::mesh::Mesh;
use crate::mesh::layers::{LayerRow, MOST_LAYERS, Ride, StrengthDrag};
use crate::sculpt::Brush;

/// What a stroke of `brush` on the model underneath does to the sculpt
/// layers riding on it (`mesh/layers.rs` says why it depends on the brush).
/// Pose swings what it moves round a joint, and the layers swing with it.
/// Every other brush pushes the surface along, and takes whatever stands on
/// it along as it stands.
pub(crate) fn ride_of(brush: Brush) -> Ride {
    if brush == Brush::Pose { Ride::Turned } else { Ride::Still }
}

/// A Strength slider that is being held.
struct Held {
    /// The mesh it is on (`Mesh::uid`).
    uid: u64,
    layer: u32,
    /// Where the slider was last put, so that a frame it has not moved in
    /// costs nothing.
    percent: u8,
    drag: StrengthDrag,
    /// A command for it has come since the last frame's look.
    fresh: bool,
}

thread_local! {
    /// Kept here and not in `Running`, like the levels' own note between
    /// frames: nothing else reads it.
    static HELD: RefCell<Option<Held>> = const { RefCell::new(None) };
}

fn quoted(row: &LayerRow) -> String {
    format!("\"{}\"", row.name)
}

impl Running {
    fn layer_row(&self, id: u32) -> Option<LayerRow> {
        self.doc.mesh.layers.as_ref().and_then(|layers| layers.row(id))
    }

    /// Records a change to the layers that the mesh has already made.
    fn layers_changed(&mut self, step: crate::mesh::layers::LayersStep) {
        self.doc.history.push_layers(step);
        // The surface under the brush ring may have moved.
        self.doc.sculptor.last_hit = None;
        self.redraw = true;
    }

    /// Says why Strength, the eye and Delete cannot be used just now, if
    /// they cannot.
    fn layer_strength_refused(&mut self) -> bool {
        match self.doc.mesh.layer_strength_refusal() {
            Some(why) => {
                self.status(why, false);
                true
            }
            None => false,
        }
    }

    /// Carries out a command of the Layers card.
    pub(super) fn layer_act(&mut self, action: LayerAction) {
        match action {
            LayerAction::New(name) => self.layer_new(name),
            LayerAction::Rename(id, name) => self.layer_rename(id, &name),
            LayerAction::Record(id) => self.layer_record(id),
            LayerAction::Strength(id, percent) => self.layer_strength(id, percent),
            LayerAction::StrengthDone => self.layer_strength_done(),
            LayerAction::Show(id, on) => self.layer_show(id, on),
            LayerAction::Delete(id) => self.layer_remove(id, false),
            LayerAction::Bake(id) => self.layer_remove(id, true),
            LayerAction::BakeAll => self.layer_bake_all(),
        }
    }

    fn layer_new(&mut self, name: Option<String>) {
        if self.doc.mesh.layer_count() >= MOST_LAYERS {
            let text = format!("An object can have {MOST_LAYERS} layers. Bake the ones that are finished to make room.");
            return self.status(text, false);
        }
        let (id, step) = self.doc.mesh.add_layer(name);
        self.layers_changed(step);
        let name = self.layer_row(id).map_or(String::new(), |row| quoted(&row));
        self.toast("New Layer");
        self.status(
            format!(
                "Added the layer {name}. It is recording: what you sculpt now goes into it, and can be turned down, \
                 hidden or deleted without touching the rest. Undo (Z) removes it."
            ),
            true,
        );
    }

    fn layer_rename(&mut self, id: u32, name: &str) {
        if let Some(step) = self.doc.mesh.rename_layer(id, name) {
            self.layers_changed(step);
            self.status(format!("The layer is now called \"{}\".", name.trim()), true);
        }
    }

    fn layer_record(&mut self, id: Option<u32>) {
        let Some(step) = self.doc.mesh.record_on_layer(id) else { return };
        self.layers_changed(step);
        let text = match id.and_then(|id| self.layer_row(id)) {
            Some(row) if !row.on || row.percent == 0 => format!(
                "Recording on {}, but it is {}: switch it on and raise its Strength before sculpting.",
                quoted(&row),
                if row.on { "at 0%" } else { "switched off" }
            ),
            Some(row) => format!("Recording on {}: what you sculpt goes into this layer.", quoted(&row)),
            None => "No layer is recording: what you sculpt changes the model underneath, and the layers ride on it."
                .to_owned(),
        };
        self.status(text, true);
    }

    /// The slider of layer `id` is held at `percent`.
    fn layer_strength(&mut self, id: u32, percent: u8) {
        let percent = percent.min(100);
        let uid = self.doc.mesh.uid;
        let open = HELD.with(|held| held.borrow().as_ref().map(|held| (held.uid, held.layer, held.percent)));
        match open {
            Some((mesh, layer, at)) if (mesh, layer) == (uid, id) => {
                HELD.with(|held| held.borrow_mut().as_mut().expect("just seen").fresh = true);
                if at == percent {
                    return;
                }
            }
            Some(_) => self.layer_strength_done(),
            None => {}
        }
        let Some(row) = self.layer_row(id) else { return };
        if open.is_none_or(|(mesh, layer, _)| (mesh, layer) != (uid, id)) {
            // The first move of a drag. One that goes nowhere opens nothing.
            if row.percent == percent || self.layer_strength_refused() {
                return;
            }
            let Some(drag) = self.doc.mesh.drag_layer(id) else { return };
            HELD.with(|held| *held.borrow_mut() = Some(Held { uid, layer: id, percent: row.percent, drag, fresh: true }));
        }
        HELD.with(|held| {
            let mut held = held.borrow_mut();
            let held = held.as_mut().expect("opened above");
            self.doc.mesh.drag_layer_to(&held.drag, row.on, percent);
            held.percent = percent;
        });
        self.doc.sculptor.last_hit = None;
        self.redraw = true;
    }

    /// The slider was let go: the drag becomes one undo step.
    fn layer_strength_done(&mut self) {
        let Some(held) = HELD.with(|held| held.borrow_mut().take()) else { return };
        if held.uid != self.doc.mesh.uid {
            return;
        }
        let Some(step) = self.doc.mesh.end_layer_drag(held.drag) else { return };
        self.layers_changed(step);
        if let Some(row) = self.layer_row(held.layer) {
            let mut text = format!("{} is at {}%.", quoted(&row), row.percent);
            if row.recording && row.percent == 0 {
                text += " It is the layer that records, so raise it again before sculpting.";
            }
            self.status(text, true);
        }
    }

    fn layer_show(&mut self, id: u32, on: bool) {
        if self.layer_strength_refused() {
            return;
        }
        let Some(row) = self.layer_row(id) else { return };
        let Some(step) = self.doc.mesh.set_layer(id, on, row.percent) else { return };
        self.layers_changed(step);
        let text = if on {
            format!("{} is showing again, at {}%.", quoted(&row), row.percent)
        } else {
            format!("{} is hidden: the model is shown without it. What was sculpted on it is kept.", quoted(&row))
        };
        self.status(text, true);
    }

    /// Delete, or with `bake` Bake.
    fn layer_remove(&mut self, id: u32, bake: bool) {
        if !bake && self.layer_strength_refused() {
            return;
        }
        let Some(row) = self.layer_row(id) else { return };
        let Some(step) = self.doc.mesh.remove_layer(id, bake) else { return };
        self.layers_changed(step);
        let name = quoted(&row);
        let text = match (bake, row.on && row.percent > 0) {
            (false, _) => format!("Deleted the layer {name} and what it added. Undo (Z) brings it back."),
            (true, true) => format!(
                "Baked {name} into the model at {}%: the shape is the same and the layer is gone. Undo (Z) brings it \
                 back.",
                row.percent
            ),
            (true, false) => format!(
                "{name} was not showing, so there was nothing of it to bake: the shape is the same and the layer is \
                 gone. Undo (Z) brings it back."
            ),
        };
        self.toast(if bake { "Bake" } else { "Delete Layer" });
        self.status(text, true);
    }

    fn layer_bake_all(&mut self) {
        let count = self.doc.mesh.layer_count();
        let Some(step) = self.doc.mesh.bake_all_layers() else {
            return self.status("This object has no layers to bake.", true);
        };
        self.layers_changed(step);
        let layers = if count == 1 { "The layer is".to_owned() } else { format!("All {count} layers are") };
        self.toast("Bake All");
        self.status(
            format!("{layers} part of the model now, as they showed: the shape is the same. Undo (Z) brings them back."),
            true,
        );
    }

    /// Before any other command: a Strength slider still held is let go, so
    /// that its drag is a step of its own and not one that straddles this.
    pub(super) fn layers_before(&mut self, action: &Action) {
        if !matches!(action, Action::Layer(LayerAction::Strength(..) | LayerAction::StrengthDone)) {
            self.layer_strength_done();
        }
    }

    /// Once a frame: a held slider that no command came for since the last
    /// look has been let go without the card saying so (the card went away
    /// under it), and is closed here.
    pub(super) fn layers_watch(&mut self) {
        let stale = HELD.with(|held| match held.borrow_mut().as_mut() {
            Some(held) => !std::mem::replace(&mut held.fresh, false),
            None => false,
        });
        if stale {
            self.layer_strength_done();
        }
    }

    /// Whether a stroke about to begin has to be refused because the layer
    /// that would record it is switched off or at 0%; says why if so. A mask
    /// stroke moves nothing and is never refused.
    pub(super) fn layer_refuses_stroke(&mut self) -> bool {
        match self.doc.layer_stroke_refusal() {
            Some(why) => {
                self.status(why, false);
                true
            }
            None => false,
        }
    }
}

impl crate::doc::Doc {
    /// Why a stroke begun now on the active object would be refused by its
    /// layers, if it would (see `Running::layer_refuses_stroke`).
    pub(crate) fn layer_stroke_refusal(&self) -> Option<String> {
        if self.sculptor.settings.mask_mode { None } else { self.mesh.layer_refusal() }
    }
}

// ---- for the Claude bridge -----------------------------------------------------

/// A mesh's sculpt layers as the bridge's `get_state` reports them, top of
/// the list first. Empty for an object without layers.
pub(super) fn state_of(mesh: Option<&Mesh>) -> Value {
    let row = |row: LayerRow| {
        serde_json::json!({
            "id": row.id,
            "name": row.name,
            "on": row.on,
            "strength_percent": row.percent,
            "recording": row.recording,
        })
    };
    Value::Array(mesh.map_or_else(Vec::new, |mesh| mesh.layer_rows()).into_iter().map(row).collect())
}

impl Running {
    /// The layer the bridge was asked for: `layer` is its name, or its id
    /// from `get_state`.
    fn layer_asked(&self, args: &Value) -> Result<LayerRow, String> {
        let rows = self.doc.mesh.layer_rows();
        let found = match args.get("layer") {
            Some(Value::Number(id)) => rows.iter().find(|row| Some(row.id as u64) == id.as_u64()),
            Some(Value::String(name)) => rows
                .iter()
                .find(|row| row.name == *name)
                .or_else(|| rows.iter().find(|row| row.name.eq_ignore_ascii_case(name))),
            _ => return Err("This command needs layer: a layer's name or id, from get_state.".into()),
        };
        found.cloned().ok_or_else(|| {
            let names: Vec<String> = rows.iter().map(quoted).collect();
            let has = if names.is_empty() { "no layers".to_owned() } else { format!("the layers {}", names.join(", ")) };
            format!("The active object has {has}, and none is that one.")
        })
    }

    /// The bridge's commands for sculpt layers, by the name its `command`
    /// tool was given with the underscores taken out. `None` if `name` is not
    /// one of them.
    pub(super) fn layers_command(&mut self, name: &str, args: &Value) -> Option<Result<String, String>> {
        let layer = |app: &Running| app.layer_asked(args);
        let actions: Result<Vec<LayerAction>, String> = match name {
            // Named as it is made, if a name came with it.
            "layernew" => Ok(vec![LayerAction::New(args.get("layer").and_then(Value::as_str).map(str::to_owned))]),
            "layerrecord" => match args.get("layer") {
                None | Some(Value::Null) => Ok(vec![LayerAction::Record(None)]),
                Some(Value::String(none)) if none.eq_ignore_ascii_case("none") => Ok(vec![LayerAction::Record(None)]),
                Some(_) => layer(self).map(|row| vec![LayerAction::Record(Some(row.id))]),
            },
            "layerstrength" => match args.get("percent").and_then(Value::as_f64).filter(|p| (0.0..=100.0).contains(p)) {
                Some(percent) => layer(self).map(|row| {
                    // The slider dragged there and let go: one undo step.
                    vec![LayerAction::Strength(row.id, percent.round() as u8), LayerAction::StrengthDone]
                }),
                None => Err("layer_strength needs percent, from 0 to 100.".into()),
            },
            "layershow" => layer(self).map(|row| vec![LayerAction::Show(row.id, true)]),
            "layerhide" => layer(self).map(|row| vec![LayerAction::Show(row.id, false)]),
            "layerdelete" => layer(self).map(|row| vec![LayerAction::Delete(row.id)]),
            "layerbake" => layer(self).map(|row| vec![LayerAction::Bake(row.id)]),
            "layerbakeall" => Ok(vec![LayerAction::BakeAll]),
            "layerrename" => match args.get("new_name").and_then(Value::as_str).filter(|name| !name.trim().is_empty()) {
                Some(new) => layer(self).map(|row| vec![LayerAction::Rename(row.id, new.to_owned())]),
                None => Err("layer_rename needs new_name.".into()),
            },
            _ => return None,
        };
        Some(actions.and_then(|actions| self.said(|app| actions.into_iter().for_each(|a| app.act(Action::Layer(a))))))
    }
}

// ---- for visual checks -------------------------------------------------------

/// A hard pose, as a rule for every vertex: the top of the model swung over
/// about a line through it, further the higher up, and one side swollen.
fn bent(p: Vec3) -> Vec3 {
    let up = ((p.y + 0.1) / 0.6).clamp(0.0, 1.0);
    let lean = Quat::from_rotation_z(-0.9 * up * up);
    let swell = 0.25 * (-((p - Vec3::new(-0.45, -0.1, 0.1)).length_squared()) / 0.08).exp();
    lean * (p + p.normalize() * swell) + Vec3::new(0.25 * up, 0.0, 0.0)
}

impl Running {
    /// Moves every vertex `to` gives a new place, as one stroke through the
    /// mesh's own stroke routines, so that the layers are told as they are
    /// for a stroke of the pen's.
    fn layers_shape(&mut self, to: impl Fn(Vec3) -> Option<Vec3> + Sync) {
        let doc = &mut self.doc;
        // As a press on the model does: the shape Reset Shape goes back to is the one before this.
        doc.scene.keep_baseline(&doc.mesh);
        // A move by rule turns what it moves, as a pose does.
        doc.mesh.layer_ride = Ride::Turned;
        let leaves: Vec<u32> = (0..doc.mesh.bvh.leaves.len() as u32).collect();
        doc.mesh.begin_stroke();
        doc.mesh.move_vertices(&leaves, |_, vertex| to(vertex.pos));
        doc.mesh.positions_changed(&leaves);
        if let Some(delta) = doc.mesh.end_stroke(false) {
            doc.history.push_stroke(delta);
        }
        self.redraw = true;
    }

    fn layer_named(&self, name: &str) -> u32 {
        self.doc.mesh.layer_rows().iter().find(|row| row.name == name).map_or(0, |row| row.id)
    }

    /// A new layer called `name`, recording.
    fn layers_add(&mut self, name: &str) -> u32 {
        self.act(Action::Layer(LayerAction::New(Some(name.to_owned()))));
        self.doc.mesh.layer_rows().last().map_or(0, |row| row.id)
    }

    /// The slider of `id` dragged to `percent` and let go.
    fn layers_slide(&mut self, id: u32, percent: u8) {
        self.act(Action::Layer(LayerAction::Strength(id, percent)));
        self.act(Action::Layer(LayerAction::StrengthDone));
    }

    /// A big change of form made with real brushes, as a sculptor makes one:
    /// the Move brush at a wide radius pulls the top of the model over and
    /// one side out, and Inflate swells the other side.
    fn layers_reshape(&mut self) {
        self.levels_stroke(Brush::Move, 1.0, 240.0, Vec2::new(40.0, -170.0), Vec2::new(150.0, -135.0));
        self.levels_stroke(Brush::Move, 1.0, 200.0, Vec2::new(-150.0, 120.0), Vec2::new(-215.0, 160.0));
        self.levels_stroke(Brush::Inflate, 0.35, 110.0, Vec2::new(150.0, 100.0), Vec2::new(170.0, 115.0));
    }

    /// A broad swelling on the front of the model, as a stroke.
    fn layers_swell(&mut self, height: f32) {
        self.layers_shape(move |p| {
            let d = p.distance(Vec3::new(0.15, 0.1, 0.45)) / 0.4;
            (d < 1.0).then(|| p + p.normalize() * height * (1.0 - d * d).powi(2))
        });
    }

    /// `--demo-layers <what>`: `play` works sculpt layers through the
    /// routines the card runs and prints a line per check (`layers_play`).
    /// Any other word sets up a picture:
    ///
    ///   - `card`: a sculpted ball with three layers, the Mesh tab showing.
    ///   - `full`, `half`, `off`: a ball with a layer of fine detail at 100%,
    ///     at 50%, and switched off.
    ///   - `under`: the same after a big change of form made with no layer
    ///     recording; `under-off`: that with the detail layer switched off.
    ///   - `dyntopo`: a layer sculpted with dynamic topology on, triangles
    ///     showing; `dyntopo-off`: that layer switched off.
    ///   - `levels`: three levels, a detail layer on the finest, the form
    ///     changed on the coarsest and stepped back up; `levels-off`: that
    ///     with the detail layer switched off; `levels-low`: the coarsest
    ///     level after the change, triangles showing.
    pub(super) fn layers_demo(&mut self, what: &str) {
        if what == "play" {
            return self.layers_play();
        }
        self.doc.opts.tabs.sculpt = crate::ui::tabs::Sculpt::Mesh;
        let levels = what.starts_with("levels");
        self.levels_ball(if levels { 1_280 } else { 80_000 });
        if levels {
            (0..3).for_each(|_| self.act(Action::Subdivide));
        }
        match what {
            "dyntopo" | "dyntopo-off" => self.layers_demo_dyntopo(what == "dyntopo-off"),
            "card" => self.layers_demo_card(),
            _ => self.layers_demo_detail(what),
        }
        self.doc.sculptor.settings.brush = Brush::Clay;
        self.frame_view();
        self.doc.camera.update(10.0);
        self.sync_view();
        println!("{}", self.ui.status.0);
    }

    /// Three layers of different kinds on a sculpted ball, for the card.
    fn layers_demo_card(&mut self) {
        self.layers_swell(0.05);
        let forms = self.layers_add("Brow and cheek");
        self.levels_stroke(Brush::Clay, 0.7, 60.0, Vec2::new(-160.0, -90.0), Vec2::new(150.0, -70.0));
        self.levels_stroke(Brush::Inflate, 0.6, 50.0, Vec2::new(-90.0, 90.0), Vec2::new(-60.0, 120.0));
        self.layers_add("Wrinkles");
        self.levels_detail();
        let scar = self.layers_add("Scar");
        self.levels_stroke(Brush::Crease, 0.9, 12.0, Vec2::new(60.0, -160.0), Vec2::new(150.0, 40.0));
        self.act(Action::Layer(LayerAction::Show(scar, false)));
        self.layers_slide(forms, 60);
        let wrinkles = self.layer_named("Wrinkles");
        self.act(Action::Layer(LayerAction::Record(Some(wrinkles))));
    }

    /// A layer of fine detail on the ball, and what `what` asks for after.
    fn layers_demo_detail(&mut self, what: &str) {
        let detail = self.layers_add("Detail");
        self.levels_detail();
        self.act(Action::Layer(LayerAction::Record(None)));
        match what {
            "half" => self.layers_slide(detail, 50),
            "off" => self.act(Action::Layer(LayerAction::Show(detail, false))),
            "under" | "under-off" => self.layers_reshape(),
            "levels" | "levels-off" | "levels-low" => {
                self.act(Action::Level(0));
                self.layers_reshape();
                if what != "levels-low" {
                    self.act(Action::LevelBy(3));
                }
            }
            _ => {}
        }
        if what.ends_with("-off") {
            self.act(Action::Layer(LayerAction::Show(detail, false)));
        }
        self.doc.opts.wireframe = what == "levels-low";
    }

    /// A layer sculpted with dynamic topology on, and then perhaps hidden.
    fn layers_demo_dyntopo(&mut self, off: bool) {
        self.levels_ball(6_000);
        let layer = self.layers_add("Horn");
        self.doc.sculptor.dyntopo.enabled = true;
        self.doc.sculptor.dyntopo.detail = 0.2;
        for y in [-60.0, 0.0, 60.0] {
            self.levels_stroke(Brush::Clay, 0.9, 45.0, Vec2::new(-120.0, y), Vec2::new(120.0, y + 20.0));
        }
        self.levels_stroke(Brush::Crease, 0.9, 14.0, Vec2::new(-100.0, -30.0), Vec2::new(100.0, 40.0));
        self.doc.sculptor.dyntopo.enabled = false;
        if off {
            self.act(Action::Layer(LayerAction::Show(layer, false)));
        }
        self.doc.opts.wireframe = true;
    }

    // ---- the scripted play ---------------------------------------------------

    /// The print of the active mesh with its layers (`Mesh::layers_print`).
    fn layers_print(&self) -> u64 {
        self.doc.mesh.layers_print()
    }

    fn layers_steps(&self) -> usize {
        self.doc.history.steps().0
    }

    /// The active mesh's positions in its level's own numbering, which a
    /// change of level and back does not disturb (the mesh's own does).
    fn level_shape(&self) -> Vec<Vec3> {
        let mesh = &self.doc.mesh;
        mesh.levels.as_ref().map_or_else(|| mesh.positions.clone(), |levels| levels.ordered_positions(mesh))
    }

    /// `--demo-layers play`: every command of the Layers card, and
    /// everything else that meets a mesh with layers, worked in the real app
    /// with a line printed per check (ok / FAIL) and the status line the
    /// steps that matter leave.
    fn layers_play(&mut self) {
        fn say(what: &str, ok: bool) {
            println!("{} {what}", if ok { "ok  " } else { "FAIL" });
        }
        let folder = crate::prefs::scratch("sculpt_layers_demo");
        let _ = std::fs::remove_dir_all(&folder);
        if let Err(e) = std::fs::create_dir_all(&folder) {
            return println!("FAIL could not make {}: {e}", folder.display());
        }
        self.levels_ball(20_000);
        // What the first press on the model does: Reset Shape goes back to the ball.
        self.doc.scene.keep_baseline(&self.doc.mesh);
        let plain = self.layers_print();
        let apart = |a: &[Vec3], b: &[Vec3]| a.iter().zip(b).map(|(a, b)| a.distance(*b)).fold(0.0, f32::max);

        // ---- the card's own commands
        self.act(Action::Layer(LayerAction::New(None)));
        println!("     {}", self.ui.status.0);
        let first = self.doc.mesh.layer_rows();
        say(
            "New Layer adds \"Layer 1\", on, at 100%, recording, as one undo step",
            first.len() == 1 && first[0].name == "Layer 1" && first[0].recording && self.layers_steps() == 1,
        );
        let detail = first[0].id;
        let underneath = self.doc.mesh.positions.clone();
        self.levels_detail();
        let strokes = self.layers_steps() - 1;
        let sculpted = self.doc.mesh.positions.clone();
        let full = self.layers_print();
        say(
            "strokes with it recording leave the model underneath where it was, to the bit",
            self.doc.mesh.model_underneath() == underneath && apart(&sculpted, &underneath) > 0.005,
        );
        // The slider, dragged through several values over several frames and let go.
        for percent in [90, 70, 70, 35, 50] {
            self.act(Action::Layer(LayerAction::Strength(detail, percent)));
            self.layers_watch();
        }
        self.act(Action::Layer(LayerAction::StrengthDone));
        println!("     {}", self.ui.status.0);
        let half: Vec<Vec3> = underneath.iter().zip(&sculpted).map(|(a, b)| (*a + *b) * 0.5).collect();
        say(
            "a drag of Strength to 50% is halfway, and one undo step",
            apart(&self.doc.mesh.positions, &half) < 1e-6 && self.layers_steps() == strokes + 2,
        );
        self.act(Action::Undo);
        say("Undo of the drag is the sculpted shape, to the bit", self.layers_print() == full);
        self.act(Action::Redo);
        // A slider that is held and then lost (the card went away under it) still closes.
        self.act(Action::Layer(LayerAction::Strength(detail, 20)));
        self.layers_watch();
        self.layers_watch();
        say("a slider let go without a word is still one step", self.layers_steps() == strokes + 3);
        // And anything else that happens while one is held closes it first.
        self.act(Action::Layer(LayerAction::Strength(detail, 80)));
        self.act(Action::Layer(LayerAction::Rename(detail, "Detail".into())));
        let rows = self.doc.mesh.layer_rows();
        say(
            "a command given while a slider is held makes two steps, in order",
            rows[0].percent == 80 && rows[0].name == "Detail" && self.layers_steps() == strokes + 5,
        );
        self.layers_slide(detail, 0);
        let off = apart(&self.doc.mesh.positions, &underneath);
        println!("     at 0% the shape is {off:e} from the model underneath");
        say("at 0% the shape is the model underneath, to rounding", off <= f32::EPSILON);
        self.layers_slide(detail, 100);
        let back = apart(&self.doc.mesh.positions, &sculpted);
        println!("     back at 100% it is {back:e} from the shape that was sculpted");
        say("and back at 100% it is the sculpted shape, to rounding", back <= 2.0 * f32::EPSILON);
        self.act(Action::Layer(LayerAction::Show(detail, false)));
        println!("     {}", self.ui.status.0);
        say("the eye takes the layer out of the shape", apart(&self.doc.mesh.positions, &underneath) <= f32::EPSILON);

        // ---- what is refused, in words
        say("with the recording layer hidden a stroke is refused, in words", {
            let refused = self.layer_refuses_stroke();
            refused && self.ui.status.0.contains("\"Detail\" is recording but is switched off")
        });
        println!("     {}", self.ui.status.0);
        self.doc.sculptor.settings.mask_mode = true;
        say("painting a mask is not", !self.layer_refuses_stroke());
        self.doc.sculptor.settings.mask_mode = false;
        self.act(Action::Layer(LayerAction::Show(detail, true)));
        self.layers_slide(detail, 0);
        say(
            "nor is a stroke allowed at 0%",
            self.layer_refuses_stroke() && self.ui.status.0.contains("is at 0%"),
        );
        self.layers_slide(detail, 100);
        say("at any strength above that it is", !self.layer_refuses_stroke());
        while self.layers_steps() > strokes + 1 {
            self.act(Action::Undo);
        }
        say(
            "Undo back through all of it is the sculpted shape, to the bit",
            self.layers_print() == full && full != plain && self.doc.mesh.positions == sculpted,
        );

        self.layers_play_stack(underneath, sculpted);
        self.layers_play_brushes();
        self.layers_play_bridge();
        self.layers_play_rebuilders();
        self.layers_play_keepers();
        self.layers_play_levels();
        self.layers_play_files(&folder);
        println!("{}", self.ui.status.0);
    }

    /// Several layers, the model underneath, delete and bake.
    fn layers_play_stack(&mut self, underneath: Vec<Vec3>, sculpted: Vec<Vec3>) {
        fn say(what: &str, ok: bool) {
            println!("{} {what}", if ok { "ok  " } else { "FAIL" });
        }
        let apart = |a: &[Vec3], b: &[Vec3]| a.iter().zip(b).map(|(a, b)| a.distance(*b)).fold(0.0, f32::max);
        let steps = self.layers_steps();
        let detail = self.layer_named("Layer 1");
        let before = self.layers_print();

        // A second layer, recorded at 40%.
        let swelling = self.layers_add("Swelling");
        self.layers_slide(swelling, 40);
        self.layers_swell(0.04);
        let both = self.doc.mesh.positions.clone();
        say(
            "a second layer records without touching the first",
            self.doc.mesh.layer_rows().iter().filter(|row| row.recording).count() == 1 && apart(&both, &sculpted) > 0.03,
        );
        self.layers_slide(swelling, 100);
        let rise = apart(&self.doc.mesh.positions, &sculpted);
        println!("     a stroke 0.040 high recorded at 40% stands {rise:.3} high at 100%");
        say("recorded at 40%, a stroke stands two and a half times as high at 100%", (rise - 0.1).abs() < 2e-3);
        self.layers_slide(swelling, 40);
        self.act(Action::Layer(LayerAction::Show(detail, false)));
        let only: Vec<Vec3> = (0..both.len()).map(|v| underneath[v] + (both[v] - sculpted[v])).collect();
        say("either can be hidden without the other", apart(&self.doc.mesh.positions, &only) < 1e-6);
        self.act(Action::Layer(LayerAction::Show(detail, true)));

        // The model underneath, with no layer recording.
        self.act(Action::Layer(LayerAction::Record(None)));
        println!("     {}", self.ui.status.0);
        let held = self.layers_print();
        // The whole model turned and shifted, as the Pose brush turns a limb.
        let turn = Quat::from_axis_angle(Vec3::new(0.3, 1.0, 0.2).normalize(), 1.2);
        let posed = move |p: Vec3| turn * p + Vec3::new(0.1, -0.05, 0.02);
        self.layers_shape(move |p| Some(posed(p)));
        let moved = self.doc.mesh.positions.clone();
        say("with none recording a stroke changes the model underneath", apart(&moved, &both) > 0.1);
        self.act(Action::Layer(LayerAction::Show(detail, false)));
        self.act(Action::Layer(LayerAction::Show(swelling, false)));
        let bare: Vec<Vec3> = underneath.iter().map(|&p| posed(p)).collect();
        let ghost = apart(&self.doc.mesh.positions, &bare);
        println!("     both layers hidden after a turn of 1.2 rad underneath: {ghost:e} from the bare model turned");
        say("and the layers went round with it: hidden, they leave the bare model turned", ghost < 2e-6);
        (0..3).for_each(|_| self.act(Action::Undo));
        say("Undo of the two eyes and the turn is every bit as it was", self.layers_print() == held);

        // Delete and Bake.
        self.act(Action::Layer(LayerAction::Delete(swelling)));
        println!("     {}", self.ui.status.0);
        say(
            "Delete takes the layer and what it added",
            self.doc.mesh.layer_count() == 1 && apart(&self.doc.mesh.positions, &sculpted) < 1e-6,
        );
        self.act(Action::Undo);
        say("Undo brings it back, to the bit", self.layers_print() == held);
        let shape = self.doc.mesh.positions.clone();
        self.act(Action::Layer(LayerAction::Bake(swelling)));
        println!("     {}", self.ui.status.0);
        say(
            "Bake keeps the shape and forgets the layer",
            self.doc.mesh.layer_count() == 1 && self.doc.mesh.positions == shape,
        );
        self.act(Action::Layer(LayerAction::Show(detail, false)));
        let baked: Vec<Vec3> = (0..both.len()).map(|v| underneath[v] + (both[v] - sculpted[v])).collect();
        say("what was baked is part of the model underneath now", apart(&self.doc.mesh.positions, &baked) < 1e-6);
        self.act(Action::Layer(LayerAction::Show(detail, true)));
        self.act(Action::Layer(LayerAction::BakeAll));
        println!("     {}", self.ui.status.0);
        say("Bake All leaves an ordinary mesh of the same shape", {
            self.doc.mesh.layers.is_none() && self.doc.mesh.positions == shape
        });
        self.act(Action::Layer(LayerAction::BakeAll));
        say("and says so when there is nothing to bake", self.ui.status.0.contains("no layers to bake"));
        while self.layers_steps() > steps {
            self.act(Action::Undo);
        }
        say("Undo of everything since is the one layer as sculpted, to the bit", self.layers_print() == before);
        let redo = self.doc.history.steps().1;
        (0..redo).for_each(|_| self.act(Action::Redo));
        say("Redo of all of it ends on the ordinary mesh again", {
            self.doc.mesh.layers.is_none() && self.doc.mesh.positions == shape
        });
        (0..redo).for_each(|_| self.act(Action::Undo));

        // Dynamic topology.
        let triangles = self.doc.mesh.triangle_count();
        self.doc.sculptor.dyntopo.enabled = true;
        self.doc.sculptor.dyntopo.detail = crate::sculpt::dyntopo::MIN_DETAIL;
        let strokes = self.layers_steps();
        self.levels_stroke(Brush::Clay, 0.8, 50.0, Vec2::new(-110.0, -20.0), Vec2::new(110.0, -10.0));
        self.doc.sculptor.dyntopo.enabled = false;
        say("the stroke landed", self.layers_steps() == strokes + 1);
        let grown = self.doc.mesh.triangle_count();
        let with = self.doc.mesh.positions.clone();
        self.act(Action::Layer(LayerAction::Show(detail, false)));
        let hidden = self.doc.mesh.positions.clone();
        // With the layer hidden the new vertices lie on the ball they were split out of.
        let off_ball = hidden.iter().map(|p| (p.length() - 0.5).abs()).fold(0.0, f32::max);
        println!("     {} triangles added; layer hidden, the model is within {off_ball:e} of the ball", grown - triangles);
        say(
            "a stroke with dynamic topology on is recorded, new vertices and all",
            grown > triangles && hidden.len() == with.len() && off_ball < 2e-3 && apart(&with, &hidden) > 0.005,
        );
        (0..2).for_each(|_| self.act(Action::Undo));
        say(
            "Undo of it takes the new triangles and their share of the layer away",
            self.doc.mesh.triangle_count() == triangles && self.layers_print() == before,
        );
    }

    /// Every brush, with a layer recording and then with none: recorded, a
    /// stroke leaves the model underneath where it was; with none recording
    /// it is the model underneath that moves.
    fn layers_play_brushes(&mut self) {
        fn say(what: &str, ok: bool) {
            println!("{} {what}", if ok { "ok  " } else { "FAIL" });
        }
        let apart = |a: &[Vec3], b: &[Vec3]| a.iter().zip(b).map(|(a, b)| a.distance(*b)).fold(0.0, f32::max);
        let (steps, before) = (self.layers_steps(), self.layers_print());
        let detail = self.layer_named("Layer 1");
        for mirrored in [false, true] {
            self.doc.edit_symmetry(|symmetry| symmetry.mirror = [mirrored, false, false]);
            let (mut recorded, mut worst, mut idle) = (Vec::new(), 0.0f32, Vec::new());
            for brush in Brush::ALL {
                let under = self.doc.mesh.model_underneath();
                let (shape, strokes) = (self.doc.mesh.positions.clone(), self.layers_steps());
                self.levels_stroke(brush, 0.7, 45.0, Vec2::new(-150.0, 60.0), Vec2::new(40.0, 110.0));
                if self.layers_steps() == strokes || apart(&self.doc.mesh.positions, &shape) < 1e-5 {
                    idle.push(brush.name());
                    continue;
                }
                recorded.push(brush.name());
                worst = worst.max(apart(&self.doc.mesh.model_underneath(), &under));
            }
            let how = if mirrored { "mirrored across X" } else { "one-sided" };
            println!("     {how}, recorded: {}; nothing to do here: {}", recorded.join(", "), idle.join(", "));
            println!("     the model underneath moved by at most {worst:e}");
            say(
                &format!("{how}, every brush's stroke goes into the recording layer and leaves the model underneath"),
                recorded.len() >= 8 && worst < 1e-6,
            );
        }
        self.doc.edit_symmetry(|symmetry| symmetry.mirror = [false; 3]);
        // The same strokes with no layer recording move the model underneath.
        self.act(Action::Layer(LayerAction::Record(None)));
        let under = self.doc.mesh.model_underneath();
        let held = |app: &Running| app.doc.mesh.layers.as_ref().and_then(|l| l.offsets(detail)).map(<[Vec3]>::to_vec);
        let before_push = held(self);
        self.levels_stroke(Brush::Clay, 0.7, 45.0, Vec2::new(-150.0, -60.0), Vec2::new(40.0, -110.0));
        self.levels_stroke(Brush::Move, 1.0, 150.0, Vec2::new(60.0, 150.0), Vec2::new(140.0, 190.0));
        say("with none recording the same stroke moves the model underneath", {
            apart(&self.doc.mesh.model_underneath(), &under) > 1e-3
        });
        say("a brush that pushes the surface leaves the layer standing as it stood", held(self) == before_push);
        self.levels_stroke(Brush::Pose, 1.0, 120.0, Vec2::new(-40.0, -150.0), Vec2::new(120.0, -150.0));
        say("the Pose brush turns the layer with what it turns", held(self) != before_push);
        self.act(Action::Layer(LayerAction::Show(detail, false)));
        say("and hidden, the layer leaves that stroke standing", apart(&self.doc.mesh.positions, &under) > 1e-3);
        while self.layers_steps() > steps {
            self.act(Action::Undo);
        }
        say("Undo of every one of them is the layer as it was, to the bit", self.layers_print() == before);
    }

    /// The Claude bridge: the state it reports and its layer commands.
    fn layers_play_bridge(&mut self) {
        fn say(what: &str, ok: bool) {
            println!("{} {what}", if ok { "ok  " } else { "FAIL" });
        }
        let (steps, before) = (self.layers_steps(), self.layers_print());
        let command = |app: &mut Running, args: Value| app.call("command", &args);
        // What get_state says of the first object, under `key`.
        fn reported(app: &mut Running, key: &str) -> Value {
            let state: Option<Value> = app.call("get_state", &Value::Null).ok().and_then(|s| serde_json::from_str(&s).ok());
            state.map_or(Value::Null, |state| state["objects"][0][key].clone())
        }
        let layers = |app: &mut Running| reported(app, "layers").as_array().cloned().unwrap_or_default();
        let listed = layers(self);
        say(
            "get_state lists the object's layers",
            listed.len() == 1 && listed[0]["name"] == "Layer 1" && listed[0]["recording"] == true,
        );
        let made = command(self, serde_json::json!({ "name": "layer_new", "layer": "From Claude" }));
        println!("     {made:?}");
        let listed = layers(self);
        say(
            "layer_new adds a named layer that records",
            made.is_ok() && listed.len() == 2 && listed[1]["name"] == "From Claude" && listed[1]["recording"] == true,
        );
        let set = command(self, serde_json::json!({ "name": "layer_strength", "layer": "from claude", "percent": 35 }));
        say("layer_strength sets it, as one undo step", set.is_ok() && layers(self)[1]["strength_percent"] == 35);
        let hidden = command(self, serde_json::json!({ "name": "layer_hide", "layer": listed[1]["id"] }));
        say("layer_hide takes a layer by its id", hidden.is_ok() && layers(self)[1]["on"] == false);
        let stroke = self.call("stroke", &serde_json::json!({ "points": [[0.45, 0.5], [0.55, 0.5]] }));
        println!("     {stroke:?}");
        say("a stroke is refused while the recording layer is hidden", {
            stroke.is_err_and(|why| why.contains("is recording but is switched off"))
        });
        let stopped = command(self, serde_json::json!({ "name": "layer_record" }));
        say("layer_record with no layer stops recording", stopped.is_ok() && layers(self).iter().all(|l| l["recording"] == false));
        let wrong = command(self, serde_json::json!({ "name": "layer_bake", "layer": "Nobody" }));
        println!("     {wrong:?}");
        say("a layer that is not there is named as such", wrong.is_err_and(|why| why.contains("\"Layer 1\", \"From Claude\"")));
        let baked = command(self, serde_json::json!({ "name": "layer_bake_all" }));
        say("layer_bake_all leaves none", baked.is_ok() && layers(self).is_empty());
        // Levels through the bridge.
        let up = command(self, serde_json::json!({ "name": "subdivide", "flat": false }));
        let levels = reported(self, "levels");
        println!("     {up:?} {levels}");
        say("subdivide adds a level, and get_state reports the levels", {
            up.is_ok() && levels["count"] == 2 && levels["current"] == 2 && levels["triangles"][1] == self.doc.mesh.triangle_count()
        });
        let down = command(self, serde_json::json!({ "name": "level", "level": 1 }));
        say("level goes to the level asked for", down.is_ok() && self.doc.mesh.levels.as_ref().is_some_and(|l| l.current() == 0));
        let none = command(self, serde_json::json!({ "name": "level", "level": 7 }));
        say("a level that is not there is refused in words", none.is_err_and(|why| why.contains("levels 1 to 2")));
        while self.layers_steps() > steps {
            self.act(Action::Undo);
        }
        self.levels_watch();
        say("Undo of all the bridge did is the layer as it was, to the bit", self.layers_print() == before);
    }

    /// Everything that puts a new mesh in the object's place: each merges
    /// the layers into the model as part of its own undo step, says so, and
    /// Undo brings them back exactly.
    fn layers_play_rebuilders(&mut self) {
        fn say(what: &str, ok: bool) {
            println!("{} {what}", if ok { "ok  " } else { "FAIL" });
        }
        let ball = self.doc.scene.active_id();
        let before = self.layers_print();
        let size = self.doc.mesh.bounds();
        // A frame passes: the app has seen the object with its layers.
        self.levels_watch();
        for (name, objects, run) in rebuilders() {
            let steps = self.layers_steps();
            if objects == 2 {
                second_object(self);
            }
            run(self);
            self.doc.scene.select(ball, &mut self.doc.mesh);
            self.scene_changed();
            self.levels_watch();
            println!("     {}", self.ui.status.0);
            let merged = self.doc.mesh.layers.is_none() && self.doc.mesh.triangle_count() > 0;
            let told = self.ui.status.0.contains("The sculpt layers of \"Sphere\" were merged into the model");
            say(&format!("{name} merges the layers into the model and says so"), merged && told);
            let mut undos = 0;
            while self.layers_steps() > steps && undos < 8 {
                self.act(Action::Undo);
                undos += 1;
            }
            self.doc.scene.select(ball, &mut self.doc.mesh);
            self.scene_changed();
            let said = self.ui.status.0.clone();
            self.levels_watch();
            say(
                &format!("Undo of {name} brings them back exactly, with nothing said"),
                self.layers_print() == before && self.ui.status.0 == said,
            );
        }
        // The shape a rebuild starts from is the shape with the layers in it.
        self.doc.opts.remesh_resolution = 96.0;
        self.act(Action::Remesh);
        self.remesh_wait();
        let rebuilt = self.doc.mesh.bounds();
        say(
            "the mesh a rebuild makes has the layers' shape in it",
            (rebuilt.max - size.max).abs().max_element() < 0.02 && (rebuilt.min - size.min).abs().max_element() < 0.02,
        );
        self.act(Action::Undo);
        self.levels_watch();
    }

    /// What does not rebuild the mesh keeps the layers, and a copy of the
    /// object has its own.
    fn layers_play_keepers(&mut self) {
        fn say(what: &str, ok: bool) {
            println!("{} {what}", if ok { "ok  " } else { "FAIL" });
        }
        let apart = |a: &[Vec3], b: &[Vec3]| a.iter().zip(b).map(|(a, b)| a.distance(*b)).fold(0.0, f32::max);
        let ball = self.doc.scene.active_id();
        let (steps, before) = (self.layers_steps(), self.layers_print());
        let detail = self.layer_named("Layer 1");
        let rest = self.doc.mesh.model_underneath();
        let shape = self.doc.mesh.positions.clone();

        // Reset Shape puts the model underneath back and leaves the layers on it.
        self.act(Action::Layer(LayerAction::Record(None)));
        let turn = Quat::from_axis_angle(Vec3::new(0.3, 1.0, 0.2).normalize(), 1.2);
        self.layers_shape(move |p| Some(turn * p + Vec3::new(0.1, -0.05, 0.02)));
        self.act(Action::ResetShape);
        println!("     {}", self.ui.status.0);
        let reset = apart(&self.doc.mesh.positions, &shape);
        println!("     after a turn of the model underneath and Reset Shape the model is {reset:e} from where it was");
        say(
            "Reset Shape puts the model underneath back and leaves the layers on it",
            self.doc.mesh.layer_count() == 1 && reset < 2e-6 && apart(&self.doc.mesh.model_underneath(), &rest) < 1e-6,
        );
        // A hard pose that swings every part a different way, and real
        // strokes that push the surface about: Reset Shape undoes exactly
        // what each did to the layers, so they are back as they stood.
        self.layers_shape(|p| Some(bent(p)));
        self.layers_reshape();
        let moved = apart(&self.doc.mesh.positions, &shape);
        self.act(Action::ResetShape);
        let (reset, under) = (apart(&self.doc.mesh.positions, &shape), apart(&self.doc.mesh.model_underneath(), &rest));
        println!(
            "     after a hard pose and three big strokes underneath (the model {moved:.2} from where it was) and Reset \
             Shape: the model underneath {under:e} from where it was, the shape with its layer {reset:e}"
        );
        say("after a hard pose and big strokes underneath, Reset Shape brings the layers back as they stood", {
            moved > 0.1 && under < 1e-6 && reset < 5e-6
        });
        self.act(Action::Mask(MaskOp::Invert));
        self.act(Action::Mask(MaskOp::Clear));
        self.levels_watch();
        say("the mask commands keep the layers", self.doc.mesh.layer_count() == 1);
        // What Set Origin to Mesh does when the centre is off the mesh: the
        // mesh is rebuilt with every vertex shifted, under a new number.
        let uid = self.doc.mesh.uid;
        self.edit_scene(|scene, slot| {
            scene.move_origin_by(Vec3::new(0.3, 0.1, 0.0), slot);
            true
        });
        self.doc.mesh_replaced = true;
        self.levels_watch();
        say(
            "Set Origin to Mesh rebuilds the mesh and keeps them",
            self.doc.mesh.uid != uid && self.doc.mesh.layer_count() == 1 && !self.ui.status.0.contains("merged"),
        );
        let with = self.doc.mesh.positions.clone();
        self.act(Action::Layer(LayerAction::Show(detail, false)));
        say("and they still work there", apart(&self.doc.mesh.positions, &with) > 0.005);
        while self.layers_steps() > steps {
            self.act(Action::Undo);
        }
        self.doc.scene.select(ball, &mut self.doc.mesh);
        self.scene_changed();
        say("Undo of all that is the layer as sculpted, to the bit", self.layers_print() == before);

        self.act(Action::InstanceObject);
        say("an instance shares the layers", self.doc.mesh.layer_count() == 1 && self.layers_print() == before);
        self.act(Action::MakeUnique);
        say("made unique, it has its own, the same", self.layers_print() == before);
        self.act(Action::Layer(LayerAction::Show(detail, false)));
        self.act(Action::SelectObject(ball));
        say("hiding the copy's layer leaves the original's showing", self.layers_print() == before);
        self.act(Action::CloneObject);
        say("a copy of the object keeps the layers", self.layers_print() == before);
        while self.layers_steps() > steps {
            self.act(Action::Undo);
        }
        self.doc.scene.select(ball, &mut self.doc.mesh);
        self.scene_changed();
        say("the scene is as it was", self.doc.scene.objects.len() == 1 && self.layers_print() == before);
    }

    /// Layers and subdivision levels together.
    fn layers_play_levels(&mut self) {
        fn say(what: &str, ok: bool) {
            println!("{} {what}", if ok { "ok  " } else { "FAIL" });
        }
        let steps = self.layers_steps();
        let detail = self.layer_named("Layer 1");
        self.act(Action::Subdivide);
        say(
            "Subdivide carries the layer up to the new level",
            self.doc.mesh.layer_count() == 1 && self.doc.mesh.level_count() == 2,
        );
        self.act(Action::Subdivide);
        let (top, top_shape) = (self.layers_print(), self.level_shape());
        self.act(Action::Level(0));
        println!("     {}", self.ui.status.0);
        say("on a lower level the layer is still there", self.doc.mesh.layer_count() == 1);
        let low = self.layers_steps();
        self.layers_slide(detail, 30);
        println!("     {}", self.ui.status.0);
        say(
            "Strength is refused below the finest level, in words",
            self.layers_steps() == low && self.ui.status.0.contains("Step up to the finest level first"),
        );
        self.act(Action::Layer(LayerAction::Show(detail, false)));
        self.act(Action::Layer(LayerAction::Delete(detail)));
        say("so are the eye and Delete", self.layers_steps() == low && self.doc.mesh.layer_count() == 1);
        self.act(Action::Level(2));
        say("down and back up with nothing edited is every bit of the layer", self.layers_print() == top);

        // A layer recorded on the coarsest level, under one from the finest.
        self.act(Action::Level(0));
        let form = self.layers_add("Form");
        self.layers_swell(0.06);
        self.act(Action::Level(2));
        let both = self.layers_print();
        say("a layer recorded on the coarsest level comes up with the mesh", both != top && self.doc.mesh.layer_count() == 2);
        self.act(Action::Layer(LayerAction::Show(form, false)));
        let apart = |a: &[Vec3], b: &[Vec3]| a.iter().zip(b).map(|(a, b)| a.distance(*b)).fold(0.0, f32::max);
        let left = apart(&self.level_shape(), &top_shape);
        println!("     the coarse layer hidden on the finest level leaves {left:e} of itself");
        say("hidden on the finest level, it leaves the model as it was before it, to rounding", left < 1e-6);
        self.act(Action::Undo);

        // The model underneath changed on the coarsest level, then the detail hidden.
        self.act(Action::Level(0));
        self.act(Action::Layer(LayerAction::Record(None)));
        self.layers_shape(|p| Some(bent(p)));
        self.act(Action::Level(2));
        println!("     {}", self.ui.status.0);
        say("a change of form underneath on the coarsest level is carried up under the layers", {
            self.doc.mesh.layer_count() == 2 && self.ui.status.0.contains("carried into the finer levels")
        });

        // Deleting levels keeps the layers working.
        self.act(Action::Level(1));
        self.act(Action::DeleteLevels { lower: false });
        let here = self.doc.mesh.positions.clone();
        self.act(Action::Layer(LayerAction::Show(detail, false)));
        say("after Delete Higher this level is the finest, and the eye works on it", self.doc.mesh.positions != here);
        self.act(Action::Undo);
        self.act(Action::Undo);
        self.act(Action::DeleteLevels { lower: true });
        self.act(Action::Level(1));
        say("after Delete Lower the layers come up as before", self.doc.mesh.layer_count() == 2);

        // A rebuild of an object with both says both in one sentence.
        self.doc.opts.remesh_resolution = 48.0;
        self.act(Action::Remesh);
        self.remesh_wait();
        self.levels_watch();
        println!("     {}", self.ui.status.0);
        say(
            "a rebuild of an object with levels and layers says both in one sentence",
            self.ui.status.0.contains("were dropped and its sculpt layers merged into the model"),
        );
        while self.layers_steps() > steps {
            self.act(Action::Undo);
        }
        self.levels_watch();
        say("Undo of all of it is the one layer on the one level", {
            self.doc.mesh.level_count() == 1 && self.doc.mesh.layer_count() == 1
        });
    }

    /// Saving, a recovery copy and a damaged file.
    fn layers_play_files(&mut self, folder: &std::path::Path) {
        fn say(what: &str, ok: bool) {
            println!("{} {what}", if ok { "ok  " } else { "FAIL" });
        }
        let print_of = |mesh: &Mesh| mesh.layers_print();
        self.layers_add("Second");
        self.layers_swell(0.03);
        self.act(Action::Subdivide);
        for level in [1, 0] {
            self.act(Action::Level(level));
            let now = self.layers_print();
            let file = folder.join(format!("layers{level}.sclp"));
            let saved = self.save_scene(&file);
            let opened = io::load_scene(&file);
            let same = opened.as_ref().is_ok_and(|scene| scene.meshes.len() == 1 && print_of(&scene.meshes[0]) == now);
            say(
                &format!("saved on level {} and opened again, the layers are the same to the bit", level + 1),
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
        }
        // A file whose layers cannot be read opens as the shape, and says so.
        let file = folder.join("layers0.sclp");
        let damaged = folder.join("damaged.sclp");
        let shape = self.doc.mesh.bounds();
        let written = std::fs::read(&file).map(|mut bytes| {
            if let Some(at) = bytes.windows(4).position(|tag| tag == b"LAYR") {
                // The layout number of the block: one this version does not know.
                bytes[at + 16] = 99;
            }
            std::fs::write(&damaged, bytes)
        });
        self.load(damaged, true);
        println!("     {}", self.ui.status.0);
        let now = self.doc.mesh.bounds();
        say(
            "a file whose layers cannot be read opens with the shape whole and says so on the status line",
            written.is_ok()
                && self.doc.mesh.layers.is_none()
                && self.doc.mesh.level_count() == 2
                && (now.max - shape.max).abs().max_element() < 1e-6
                && self.ui.status.0.contains("The sculpt layers saved with 1 object could not be read"),
        );
    }
}
