//! `--demo-paint play`: the Paint brush and the Paint Layers card worked in
//! the real app, through the same doors the pointer, the card and the
//! bridge use, with a line printed per check (ok / FAIL); and `--demo-paint
//! brush` / `--demo-paint layers`, a painted ball with the Paint brush's
//! settings, or the Paint Layers card, open at 75% interface size, for a
//! picture with `--screenshot`.

use glam::{Vec2, Vec3};
use serde_json::json;

use super::Running;
use crate::doc::{Action, PaintAction};
use crate::mesh::paint::{ColourBlend, FillArea, MaskSource, PaintRow, Texel};
use crate::sculpt::Brush;
use crate::sculpt::paint::PaintMode;

fn say(what: &str, ok: bool) {
    println!("{} {what}", if ok { "ok  " } else { "FAIL" });
}

impl Running {
    fn paint_steps(&self) -> usize {
        self.doc.history.steps().0
    }

    /// The active object's paint as a number (`Mesh::paint_print_by_place`).
    fn paint_print(&self) -> u64 {
        self.doc.mesh.paint_print_by_place()
    }

    fn paint_texel(&self, layer: u32, v: usize) -> Texel {
        self.doc.mesh.paint_stack().and_then(|s| s.layer(layer).map(|l| l.data.texel(v))).unwrap_or(Texel::BARE)
    }

    fn paint_active_id(&self) -> u32 {
        self.doc.mesh.paint_active().unwrap_or(0)
    }

    /// Vertices of the active layer with any colour on them.
    fn paint_covered(&self) -> usize {
        let layer = self.paint_active_id();
        (0..self.doc.mesh.vertex_count()).filter(|&v| self.paint_texel(layer, v).colour.0[3] > 0).count()
    }

    /// A stroke through the bridge between two window points (0 to 1).
    fn paint_stroke(&mut self, from: Vec2, to: Vec2) -> Result<String, String> {
        self.call("stroke", &json!({ "points": [[from.x, from.y], [to.x, to.y]] }))
    }

    fn paint_settings(&mut self, args: serde_json::Value) {
        let mut args = args;
        args["name"] = json!("paint_settings");
        if let Err(why) = self.call("command", &args) {
            say(&format!("paint_settings {args}: {why}"), false);
        }
    }

    /// Undo then redo of the last step leaves the paint as `after` and puts
    /// it back to `before` in between.
    fn paint_round_trip(&mut self, what: &str, before: u64, after: u64) {
        self.act(Action::Undo);
        let undone = self.paint_print() == before;
        self.act(Action::Redo);
        say(&format!("{what}: undo puts the paint back and redo lays it again"), undone && self.paint_print() == after);
    }

    pub(super) fn paint_play(&mut self) {
        self.levels_ball(80_000);
        (self.doc.camera.yaw, self.doc.camera.pitch) = (0.0, 0.0);
        self.frame_view();
        self.doc.camera.update(10.0);
        self.sync_view();
        self.paint_play_strokes();
        self.paint_play_modes();
        self.paint_play_layers();
        self.paint_play_masks_and_materials();
        self.paint_play_zadd_and_dyntopo();
        self.paint_play_bridge();
        println!("     {}", self.ui.status.0);
    }

    /// The first stroke makes a layer; a stroke moves nothing; undo and redo.
    fn paint_play_strokes(&mut self) {
        let set = self.call("set_brush", &json!({ "brush": "Paint", "radius": 0.06 }));
        say("set_brush takes up the Paint brush", set.is_ok() && self.doc.sculptor.settings.brush == Brush::Paint);
        self.paint_settings(json!({ "colour": [205, 52, 40], "mode": "Paint", "opacity": 100 }));
        let (positions, steps) = (self.doc.mesh.positions.clone(), self.paint_steps());
        let drawn = self.paint_stroke(Vec2::new(0.42, 0.42), Vec2::new(0.58, 0.46));
        println!("     {drawn:?}");
        say("the first Paint stroke makes a layer and says so", drawn.as_ref().is_ok_and(|t| t.contains("Made the paint layer")));
        say("the layer and the stroke are two undo steps", self.paint_steps() == steps + 2 && self.doc.mesh.paint_rows().len() == 1);
        say("a Paint stroke paints", self.paint_covered() > 300);
        say("a Paint stroke moves no vertex, to the bit", self.doc.mesh.positions == positions);
        let after = self.paint_print();
        self.act(Action::Undo);
        let undone = self.paint_covered() == 0 && self.doc.mesh.paint_rows().len() == 1;
        self.act(Action::Redo);
        say("undo takes the stroke off and redo lays it again exactly", undone && self.paint_print() == after);
        // A stroke across the X plane under Mirror X is its own mirror image, on a layer of its own.
        self.act(Action::Paint(PaintAction::New));
        self.doc.edit_symmetry(|s| s.mirror = [true, false, false]);
        let _ = self.paint_stroke(Vec2::new(0.40, 0.60), Vec2::new(0.52, 0.56));
        let mesh = &self.doc.mesh;
        let key = |p: Vec3| [(p.x + 0.0).to_bits(), (p.y + 0.0).to_bits(), (p.z + 0.0).to_bits()];
        let map: std::collections::HashMap<[u32; 3], usize> = mesh.positions.iter().enumerate().map(|(i, &p)| (key(p), i)).collect();
        let layer = self.paint_active_id();
        let mirrored = (0..mesh.vertex_count()).all(|v| {
            let twin = map.get(&key(mesh.positions[v] * Vec3::new(-1.0, 1.0, 1.0)));
            twin.is_none_or(|&t| self.paint_texel(layer, v) == self.paint_texel(layer, t))
        });
        say("a mirrored stroke is its own mirror image, texel for texel", mirrored && self.paint_covered() > 300);
        self.doc.edit_symmetry(|s| s.mirror = [false; 3]);
        // The layer and its stroke taken back.
        self.act(Action::Undo);
        self.act(Action::Undo);
    }

    /// Erase, Blur, Smear, Sample, and the eyedropper key's command.
    fn paint_play_modes(&mut self) {
        let positions = self.doc.mesh.positions.clone();
        let covered = self.paint_covered();
        self.paint_settings(json!({ "mode": "Erase" }));
        let _ = self.paint_stroke(Vec2::new(0.46, 0.40), Vec2::new(0.50, 0.50));
        say("Erase takes paint away", self.paint_covered() < covered);
        self.paint_settings(json!({ "mode": "Blur" }));
        let before = self.paint_print();
        let _ = self.paint_stroke(Vec2::new(0.42, 0.44), Vec2::new(0.58, 0.44));
        say("Blur changes the paint under it", self.paint_print() != before);
        self.paint_settings(json!({ "mode": "Smear" }));
        let before = self.paint_print();
        let _ = self.paint_stroke(Vec2::new(0.45, 0.44), Vec2::new(0.62, 0.40));
        say("Smear changes the paint under it", self.paint_print() != before);
        say("Erase, Blur and Smear move no vertex", self.doc.mesh.positions == positions);
        // Sample: a stroke's first point picks up the colour there.
        self.paint_settings(json!({ "colour": [10, 200, 10], "mode": "Sample" }));
        let steps = self.paint_steps();
        let picked = self.paint_stroke(Vec2::new(0.50, 0.45), Vec2::new(0.52, 0.45));
        println!("     {picked:?}");
        let colour = self.doc.sculptor.settings.paint.colour;
        say("Sample picks up the paint's colour and makes no undo step", colour[0] > 150 && colour[1] < 90 && self.paint_steps() == steps);
        self.doc.sculptor.settings.paint.colour = [10, 200, 10];
        let at = Vec2::new(0.50, 0.45) * self.view.size;
        self.act(Action::Paint(PaintAction::PickAt(at)));
        say("the eyedropper (I) picks it up too", self.doc.sculptor.settings.paint.colour[0] > 150);
        self.paint_settings(json!({ "mode": "Paint" }));
    }

    /// The card's commands, each one undo step that undoes exactly.
    fn paint_play_layers(&mut self) {
        let one = |app: &mut Running, what: &str, action: PaintAction, check: &dyn Fn(&Running) -> bool| {
            let (before, steps) = (app.paint_print(), app.paint_steps());
            app.act(Action::Paint(action));
            let after = app.paint_print();
            say(&format!("{what}: {}", app.ui.status.0), check(app) && app.paint_steps() == steps + 1);
            app.paint_round_trip(what, before, after);
        };
        one(self, "New Layer", PaintAction::New, &|app| app.doc.mesh.paint_rows().len() == 2);
        let top = self.paint_active_id();
        one(self, "Fill All", PaintAction::Fill { area: FillArea::All, colour: true, material: false }, &|app| {
            app.paint_covered() == app.doc.mesh.vertex_count()
        });
        let row = self.doc.mesh.paint_rows().into_iter().find(|r| r.id == top).unwrap();
        one(self, "Blend Multiply", PaintAction::Row(PaintRow { colour_blend: ColourBlend::Multiply, ..row.clone() }), &|app| {
            app.doc.mesh.paint_rows().iter().any(|r| r.colour_blend == ColourBlend::Multiply)
        });
        // An Opacity drag: many moves, one step.
        let (before, steps) = (self.paint_print(), self.paint_steps());
        for opacity in [200u8, 150, 100, 128] {
            self.act(Action::Paint(PaintAction::Opacity(top, opacity)));
        }
        self.act(Action::Paint(PaintAction::OpacityDone));
        let held = self.doc.mesh.paint_rows().iter().any(|r| r.id == top && r.opacity == 128);
        say("an Opacity drag is one undo step", held && self.paint_steps() == steps + 1);
        let after = self.paint_print();
        self.paint_round_trip("Opacity", before, after);
        one(self, "Duplicate", PaintAction::Duplicate(top), &|app| app.doc.mesh.paint_rows().len() == 3);
        let copy = self.paint_active_id();
        one(self, "Move to the bottom", PaintAction::Move(copy, 0), &|app| app.doc.mesh.paint_rows()[0].id == copy);
        one(self, "Cavity mask", PaintAction::Row(PaintRow { mask: MaskSource::Cavity, ..self.paint_row_of(copy) }), &|app| {
            app.doc.mesh.paint_rows().iter().any(|r| r.mask == MaskSource::Cavity)
        });
        let composite = crate::mesh::paint::export_streams(&self.doc.mesh).unwrap_or_default();
        one(self, "Merge Down", PaintAction::MergeDown(top), &|app| app.doc.mesh.paint_rows().len() == 2);
        let merged = crate::mesh::paint::export_streams(&self.doc.mesh).unwrap_or_default();
        let worst = composite.iter().zip(&merged).map(|(a, b)| (0..4).map(|k| a[k].abs_diff(b[k])).max().unwrap_or(0)).max().unwrap_or(0);
        say(&format!("Merge Down leaves the colour as it showed ({worst} apart at most)"), worst <= 2);
        one(self, "Flatten All", PaintAction::FlattenAll, &|app| app.doc.mesh.paint_rows().len() == 1);
        let only = self.paint_active_id();
        one(self, "Clear", PaintAction::Clear(only), &|app| app.paint_covered() == 0);
        self.act(Action::Undo);
    }

    fn paint_row_of(&self, id: u32) -> PaintRow {
        self.doc.mesh.paint_rows().into_iter().find(|r| r.id == id).expect("the layer")
    }

    /// Mask by Colour, by Layer, and a material painted.
    fn paint_play_masks_and_materials(&mut self) {
        let colour = {
            let layer = self.paint_active_id();
            let v = (0..self.doc.mesh.vertex_count()).find(|&v| self.paint_texel(layer, v).colour.0[3] == 255).unwrap_or(0);
            let c = self.paint_texel(layer, v).colour.0;
            [c[0], c[1], c[2]]
        };
        self.doc.sculptor.settings.paint.colour = colour;
        let steps = self.paint_steps();
        self.act(Action::Paint(PaintAction::MaskByColour));
        let masked = self.doc.mesh.mask.iter().filter(|&&m| m > 0.5).count();
        say(&format!("Mask by Colour masks the painted colour ({masked} vertices), as one mask step"), masked > 100 && self.paint_steps() == steps + 1);
        self.act(Action::Paint(PaintAction::MaskByLayer));
        let by_layer = self.doc.mesh.mask.iter().filter(|&&m| m > 0.5).count();
        say("Mask by Layer masks where the layer has paint", by_layer >= masked);
        self.act(Action::Mask(crate::doc::MaskOp::Clear));
        // A material: the first metal of the library, painted.
        let metal = self.doc.opts.materials.library.named("Metal - Antique Gold");
        let slot = metal.and_then(|i| self.doc.paint_with(i).ok());
        let _ = self.paint_stroke(Vec2::new(0.40, 0.52), Vec2::new(0.60, 0.52));
        let in_table = slot.is_some_and(|s| self.doc.mesh.paint_table().contains(&s));
        let weight = crate::mesh::paint::export_streams(&self.doc.mesh)
            .map_or(0, |streams| streams.iter().map(|r| r[8]).max().unwrap_or(0));
        say("a stroke with a material picked joins it to the object's table and paints it", in_table && weight == 255);
        self.act(Action::Paint(PaintAction::MaskByMaterial));
        let by_material = self.doc.mesh.mask.iter().filter(|&&m| m > 0.5).count();
        say("Mask by Material masks where it is painted", by_material > 100);
        self.act(Action::Mask(crate::doc::MaskOp::Clear));
        self.act(Action::Material(crate::materials::Ask::ColourOnly));
    }

    /// Zadd raises what it paints, one step; nothing refines with dynamic topology on.
    fn paint_play_zadd_and_dyntopo(&mut self) {
        let (positions, steps, triangles) = (self.doc.mesh.positions.clone(), self.paint_steps(), self.doc.mesh.triangle_count());
        self.paint_settings(json!({ "zadd": true, "zadd_brush": "Layer", "colour": [240, 200, 60] }));
        let _ = self.paint_stroke(Vec2::new(0.44, 0.36), Vec2::new(0.56, 0.36));
        let raised = self.doc.mesh.positions.iter().zip(&positions).filter(|(a, b)| a != b).count();
        say(&format!("Zadd raises what it paints ({raised} vertices moved), one undo step"), raised > 100 && self.paint_steps() == steps + 1);
        self.act(Action::Undo);
        say("undoing Zadd puts every vertex back", self.doc.mesh.positions == positions);
        self.act(Action::Redo);
        self.doc.sculptor.dyntopo.enabled = true;
        self.doc.sculptor.dyntopo.detail = 0.05;
        let _ = self.paint_stroke(Vec2::new(0.42, 0.62), Vec2::new(0.58, 0.64));
        self.paint_settings(json!({ "zadd": false, "mode": "Blur" }));
        let _ = self.paint_stroke(Vec2::new(0.42, 0.62), Vec2::new(0.58, 0.64));
        self.paint_settings(json!({ "mode": "Paint" }));
        self.doc.sculptor.dyntopo.enabled = false;
        say("with dynamic topology on, Paint and Zadd strokes add no triangle", self.doc.mesh.triangle_count() == triangles);
    }

    /// The bridge's paint_layer and paint_settings.
    fn paint_play_bridge(&mut self) {
        let cmd = |app: &mut Running, args: serde_json::Value| {
            let mut args = args;
            args["name"] = json!("paint_layer");
            app.call("command", &args)
        };
        let first = self.paint_row_of(self.paint_active_id()).name;
        let made = cmd(self, json!({ "do": "new", "layer_name": "Freckles" }));
        say("paint_layer new names the layer", made.as_ref().is_ok_and(|t| t.contains("Freckles")));
        let blend = cmd(self, json!({ "do": "blend", "blend": "Overlay" }));
        say("paint_layer blend", blend.as_ref().is_ok_and(|t| t.contains("Overlay")));
        let opacity = cmd(self, json!({ "do": "opacity", "percent": 60 }));
        say("paint_layer opacity", opacity.as_ref().is_ok_and(|t| t.contains("\"opacity_percent\":60")));
        let fill = cmd(self, json!({ "do": "fill", "area": "all", "fill_material": false }));
        say("paint_layer fill", fill.is_ok());
        let select = cmd(self, json!({ "do": "select", "layer": first.clone() }));
        say("paint_layer select", select.is_ok() && self.paint_row_of(self.paint_active_id()).name == first);
        let delete = cmd(self, json!({ "do": "delete", "layer": "Freckles" }));
        say("paint_layer delete", delete.is_ok() && self.doc.mesh.paint_rows().len() == 1);
        let wrong = cmd(self, json!({ "do": "blend", "blend": "Sparkle" }));
        say("a blend that is none is refused in words", wrong.as_ref().is_err_and(|t| t.contains("Multiply")));
        let state = self.call("get_state", &json!({})).unwrap_or_default();
        say("get_state lists the paint layers and the brush's paint", state.contains("paint_layers") && state.contains("\"zadd\""));
        let settings = self.call("command", &json!({ "name": "paint_settings", "colour": "#3366ff", "opacity": 50 }));
        say("paint_settings takes a colour as #rrggbb", settings.is_ok() && self.doc.sculptor.settings.paint.colour == [0x33, 0x66, 0xff]);
        let mode = self.doc.sculptor.settings.paint.mode;
        say("the brush is back on Paint", mode == PaintMode::Paint);
    }

    /// `--demo-paint brush` and `--demo-paint layers`: the ball painted in
    /// a few layers by real strokes, with the Paint brush's settings or the
    /// Paint Layers card open at 75% size.
    pub(super) fn paint_brush_picture(&mut self, what: &str) {
        self.levels_ball(160_000);
        (self.doc.camera.yaw, self.doc.camera.pitch) = (-20.0, 10.0);
        self.frame_view();
        self.doc.camera.update(10.0);
        self.sync_view();
        let _ = self.call("set_brush", &json!({ "brush": "Paint", "radius": 0.05 }));
        let gold = self.doc.opts.materials.library.named("Metal - Antique Gold");
        // A base coat over the lower half, then stripes, a soft blush and raised dots.
        self.paint_settings(json!({ "colour": [70, 110, 160], "opacity": 100 }));
        let _ = self.call("command", &json!({ "name": "paint_layer", "do": "new", "layer_name": "Base" }));
        let _ = self.remap_lower_mask();
        let _ = self.call("command", &json!({ "name": "paint_layer", "do": "fill", "area": "masked", "fill_material": false }));
        self.act(Action::Mask(crate::doc::MaskOp::Clear));
        let _ = self.call("command", &json!({ "name": "paint_layer", "do": "new", "layer_name": "Stripes" }));
        self.paint_settings(json!({ "colour": [215, 60, 45] }));
        for k in 0..3 {
            let y = 0.36 + 0.07 * k as f32;
            let _ = self.paint_stroke(Vec2::new(0.38, y), Vec2::new(0.62, y + 0.03));
        }
        if let Some(i) = gold {
            let _ = self.doc.paint_with(i);
        }
        let _ = self.call("command", &json!({ "name": "paint_layer", "do": "new", "layer_name": "Gold Dots" }));
        self.paint_settings(json!({ "colour": [230, 190, 80], "zadd": true, "zadd_brush": "Inflate" }));
        for k in 0..4 {
            let x = 0.42 + 0.05 * k as f32;
            let _ = self.paint_stroke(Vec2::new(x, 0.60), Vec2::new(x + 0.004, 0.60));
        }
        self.act(Action::Material(crate::materials::Ask::ColourOnly));
        let _ = self.call("command", &json!({ "name": "paint_layer", "do": "new", "layer_name": "Blush" }));
        let _ = self.call("set_brush", &json!({ "brush": "Airbrush", "radius": 0.09 }));
        self.paint_settings(json!({ "colour": [250, 140, 170], "zadd": false }));
        for _ in 0..3 {
            let _ = self.paint_stroke(Vec2::new(0.55, 0.30), Vec2::new(0.62, 0.38));
        }
        let _ = self.call("command", &json!({ "name": "paint_layer", "do": "blend", "blend": "Multiply" }));
        let _ = self.call("set_brush", &json!({ "brush": "Paint" }));
        self.doc.opts.ui_scale = 0.75;
        self.doc.opts.matcap_on = false;
        let cards: Vec<(&str, bool)> = if what == "layers" {
            self.doc.opts.tabs.sculpt = crate::ui::tabs::Sculpt::Mesh;
            vec![("Paint Layers", true), ("Symmetry", false), ("Remesh", false), ("Subdivision Levels", false), ("Layers", false)]
        } else {
            self.doc.opts.tabs.sculpt = crate::ui::tabs::Sculpt::Brush;
            vec![("Brush Library", false), ("Brush Settings", true)]
        };
        let cards = cards.into_iter().map(|(card, open)| (card.to_owned(), open)).collect();
        crate::prefs::cards::restore(&self.egui_ctx, &cards);
        self.materials_frame();
        println!("{}", self.ui.status.0);
    }

    /// Masks the lower half of the ball, as one mask step.
    fn remap_lower_mask(&mut self) -> bool {
        let delta = self.doc.mesh.remap_mask(|m, v, _| if m.positions[v as usize].y < -0.05 { 1.0 } else { 0.0 });
        let made = delta.is_some();
        if let Some(delta) = delta {
            self.doc.history.push_stroke(delta);
        }
        made
    }
}
