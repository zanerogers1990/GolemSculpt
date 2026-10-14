//! Painting, on the app's side: the Paint Layers card's commands, what a
//! Paint stroke needs before it begins, the eyedropper, the paint commands
//! of the Masking card, and the bridge's `paint_layer` and `paint_settings`.
//!
//! What a layer is, and what each change to the stack does, is
//! `mesh/paint.rs` and its `ops.rs`; what the brush does is
//! `sculpt/paint.rs`. Here is how the running app carries the commands out
//! and records them:
//!
//! - **Every command is one undo step** (`history::Edit::Paint`), and a drag
//!   of a layer's Opacity slider is one step however far it went: every
//!   frame it is held shows the layer at the slider's value with no step
//!   (`Mesh::preview_paint_row`), and letting go puts the row back and sets
//!   it once, as a step. Anything else that happens while it is held lets it
//!   go first (`paint_before`).
//! - **A Paint stroke on an object with no paint layer makes one first**
//!   ("Paint 1"), as its own undo step before the stroke's, and says so: a
//!   brush that did nothing until a card was found would look broken. A
//!   locked layer refuses the stroke, in words. So does a ninth material on
//!   one object (the most it can have, `MOST_MATERIALS`).
//! - **The material a stroke lays is the scene's** (`ScenePaint::active_material`,
//!   set on the Materials card or here by the eyedropper); it joins the
//!   object's local material table as its own step the first time it is
//!   painted there.
//! - **The eyedropper** (Sample mode's click, the I key, the bridge's
//!   `pick_paint`) takes the colour and the strongest material of the paint
//!   under the pointer: the composite, what is seen, not one layer's.

use std::cell::RefCell;

use glam::Vec2;
use serde_json::{Value, json};

use super::Running;
use crate::doc::{Action, Doc, PaintAction};
use crate::mesh::Mesh;
use crate::mesh::paint::{
    ChannelSet, ColourBlend, FillArea, MaskSource, MaterialBlend, PaintRefusal, PaintRow, PaintStep, PaintTarget,
    PaintMode as Toward,
};
use crate::sculpt::Brush;
use crate::sculpt::paint::{PaintMode, PaintSettings, ZADD_BRUSHES};

/// An Opacity slider that is being held.
struct Held {
    /// The mesh it is on (`Mesh::uid`), and the layer's row as it was at the press.
    uid: u64,
    was: PaintRow,
    opacity: u8,
    /// A command for it has come since the last frame's look.
    fresh: bool,
}

thread_local! {
    /// Kept here and not in `Running`, like the Layers card's slider: nothing else reads it.
    static HELD: RefCell<Option<Held>> = const { RefCell::new(None) };
}

fn quoted(row: &PaintRow) -> String {
    format!("\"{}\"", row.name)
}

/// The words for where a fill goes.
fn area_words(area: FillArea) -> String {
    match area {
        FillArea::All => "the whole layer".into(),
        FillArea::SculptMask { invert: false } => "what is masked".into(),
        FillArea::SculptMask { invert: true } => "what is not masked".into(),
        FillArea::Group(g) => format!("face group {g}"),
    }
}

impl Doc {
    /// The palette slot the scene paints with, if the palette has it.
    pub(crate) fn paint_slot(&self) -> Option<u8> {
        let paint = &self.opts.paint;
        paint.active_material.filter(|slot| paint.palette.iter().any(|e| e.slot == *slot))
    }

    /// Everything a Paint stroke about to begin on the active object needs:
    /// the scene's material handed to the brush, a layer to paint on (made
    /// if there is none, as an undo step of its own), the material in the
    /// object's table. Gives back a line to say if something was made, or
    /// why the stroke cannot be made. Nothing for any other brush.
    pub(crate) fn paint_prepare(&mut self, smooth_held: bool) -> Result<Option<String>, String> {
        let slot = self.paint_slot();
        let s = &mut self.sculptor.settings;
        if s.mask_mode || s.brush != Brush::Paint {
            return Ok(None);
        }
        s.paint.slot = slot;
        let mode = if smooth_held { PaintMode::Blur } else { s.paint.mode };
        if mode == PaintMode::Sample {
            return Ok(None);
        }
        if mode == PaintMode::Paint && s.paint.rgb && !s.paint.layer_mask {
            // Among the swatches from the first stroke made with it.
            let colour = s.paint.colour;
            s.paint.remember(colour);
        }
        let (layer_mask, wants_material) = (s.paint.layer_mask, s.paint.material && !s.paint.layer_mask);
        let mut said = None;
        match self.mesh.paint_writable() {
            Ok(_) => {}
            Err(PaintRefusal::NoLayer) if matches!(mode, PaintMode::Paint) && !layer_mask => {
                let (id, step) = self.mesh.add_paint_layer(None).map_err(|why| why.to_string())?;
                self.history.push_paint(step);
                let name = self.mesh.paint_rows().into_iter().find(|r| r.id == id).map(|r| r.name).unwrap_or_default();
                said = Some(format!(
                    "Made the paint layer \"{name}\" to paint on (Paint Layers card, on the Mesh tab). Undo (Z) takes \
                     the stroke back, and then the layer."
                ));
            }
            Err(PaintRefusal::NoLayer) => {
                return Err("There is no paint on this object to erase, blur or smear yet: paint some first.".into());
            }
            Err(why) => return Err(why.to_string()),
        }
        if layer_mask {
            let active = self.mesh.paint_active();
            let row = self.mesh.paint_rows().into_iter().find(|r| Some(r.id) == active);
            if row.is_some_and(|r| r.mask != MaskSource::Paintable) {
                return Err(
                    "Paint Layer Mask paints the layer's own mask, and this layer's mask is not Paintable. Set its \
                     Mask to Paintable on the Paint Layers card, or untick Paint Layer Mask."
                        .into(),
                );
            }
        }
        if let Some(slot) = slot.filter(|_| wants_material) {
            if !self.mesh.paint_table().contains(&slot) {
                match self.mesh.add_paint_material(slot) {
                    Ok((_, step)) => self.history.push_paint(step),
                    Err(PaintRefusal::Unchanged) => {}
                    Err(why) => return Err(why.to_string()),
                }
            }
        }
        Ok(said)
    }
}

impl Running {
    fn paint_rows(&self) -> Vec<PaintRow> {
        self.doc.mesh.paint_rows()
    }

    fn paint_row(&self, id: u32) -> Option<PaintRow> {
        self.paint_rows().into_iter().find(|r| r.id == id)
    }

    /// Records a change to the paint that the mesh has already made, and says `text`.
    fn paint_changed(&mut self, step: PaintStep, text: String) {
        self.doc.history.push_paint(step);
        self.redraw = true;
        self.status(text, true);
    }

    /// Says why a paint command did nothing. "That changes nothing" is not
    /// worth a red line: the press did what was asked.
    fn paint_refused(&mut self, why: PaintRefusal) {
        if why != PaintRefusal::Unchanged {
            self.status(why.to_string(), false);
        }
    }

    fn paint_result(&mut self, result: Result<PaintStep, PaintRefusal>, text: impl FnOnce() -> String) {
        match result {
            Ok(step) => {
                let text = text();
                self.paint_changed(step, text);
            }
            Err(why) => self.paint_refused(why),
        }
    }

    /// Carries out a command of the Paint Layers card, the Paint card or
    /// the Masking card's paint commands.
    pub(super) fn paint_act(&mut self, action: PaintAction) {
        match action {
            PaintAction::New => match self.doc.mesh.add_paint_layer(None) {
                Ok((id, step)) => {
                    let name = self.paint_row(id).map_or_else(String::new, |r| quoted(&r));
                    self.toast("New Paint Layer");
                    self.paint_changed(step, format!("Added the paint layer {name}: what you paint now goes on it."));
                }
                Err(why) => self.paint_refused(why),
            },
            PaintAction::Delete(id) => {
                let name = self.paint_row(id).map_or_else(String::new, |r| quoted(&r));
                let result = self.doc.mesh.delete_paint_layer(id);
                self.paint_result(result, || format!("Deleted the paint layer {name}. Undo (Z) brings it back."));
            }
            PaintAction::Rename(id, name) => {
                let result = self.doc.mesh.rename_paint_layer(id, &name);
                self.paint_result(result, || format!("The paint layer is now called \"{}\".", name.trim()));
            }
            PaintAction::Select(id) => {
                let name = self.paint_row(id).map_or_else(String::new, |r| quoted(&r));
                let result = self.doc.mesh.set_paint_active(id);
                self.paint_result(result, || format!("Painting on {name}."));
            }
            PaintAction::Move(id, to) => {
                let name = self.paint_row(id).map_or_else(String::new, |r| quoted(&r));
                let result = self.doc.mesh.move_paint_layer(id, to);
                self.paint_result(result, || format!("Moved {name}."));
            }
            PaintAction::Duplicate(id) => match self.doc.mesh.duplicate_paint_layer(id) {
                Ok((new, step)) => {
                    let name = self.paint_row(new).map_or_else(String::new, |r| quoted(&r));
                    self.paint_changed(step, format!("Made {name}, a copy, and painting goes on it."));
                }
                Err(why) => self.paint_refused(why),
            },
            PaintAction::MergeDown(id) => {
                let name = self.paint_row(id).map_or_else(String::new, |r| quoted(&r));
                let result = self.doc.mesh.merge_paint_down(id);
                self.paint_result(result, || {
                    format!("Merged {name} into the layer under it: the paint looks the same. Undo (Z) splits them again.")
                });
            }
            PaintAction::FlattenAll => {
                let result = self.doc.mesh.flatten_paint();
                self.paint_result(result, || {
                    "Flattened the paint layers into one, as they showed (hidden ones are dropped). Undo (Z) brings \
                     them back."
                        .into()
                });
            }
            PaintAction::Row(row) => self.paint_set_row(row),
            PaintAction::Opacity(id, opacity) => self.paint_opacity(id, opacity),
            PaintAction::OpacityDone => self.paint_opacity_done(),
            PaintAction::Fill { area, colour, material } => self.paint_fill(area, colour, material),
            PaintAction::FillPickedGroup { colour, material } => {
                if let Some(group) = self.group_picked_or_say() {
                    self.paint_fill(FillArea::Group(group), colour, material);
                }
            }
            PaintAction::Clear(id) => {
                let name = self.paint_row(id).map_or_else(String::new, |r| quoted(&r));
                let result = self.doc.mesh.clear_paint(id, ChannelSet::SURFACE);
                self.paint_result(result, || format!("Cleared {name}: it is empty now. Undo (Z) brings its paint back."));
            }
            PaintAction::RecomputeCavity(id) => {
                let result = self.doc.mesh.snapshot_paint_cavity(id);
                self.paint_result(result, || "The layer's cavity mask follows the surface as it is now.".into());
            }
            PaintAction::MaskByPickedGroup(id) => {
                if let (Some(group), Some(row)) = (self.group_picked_or_say(), self.paint_row(id)) {
                    self.paint_set_row(PaintRow { mask: MaskSource::FaceGroup(group), ..row });
                }
            }
            PaintAction::PickAt(at) => match self.paint_pick(at) {
                Ok(text) => self.status(text, true),
                Err(text) => self.status(text, false),
            },
            PaintAction::MaskByColour => self.paint_mask(Masking::Colour),
            PaintAction::MaskByMaterial => self.paint_mask(Masking::Material),
            PaintAction::MaskByLayer => self.paint_mask(Masking::Layer),
        }
    }

    /// A row's settings changed on the card: the eye, the lock, a blend, the
    /// channels, the mask source. One step (a Cavity mask with no snapshot
    /// yet takes one in the same step: `Mesh::set_paint_row`).
    fn paint_set_row(&mut self, row: PaintRow) {
        let Some(was) = self.paint_row(row.id) else { return };
        let text = row_words(&was, &row);
        let result = self.doc.mesh.set_paint_row(row);
        self.paint_result(result, || text);
    }

    /// The Opacity slider of layer `id` is held at `opacity`.
    fn paint_opacity(&mut self, id: u32, opacity: u8) {
        let uid = self.doc.mesh.uid;
        let open = HELD.with(|held| held.borrow().as_ref().map(|h| (h.uid, h.was.id, h.opacity)));
        match open {
            Some((mesh, layer, at)) if (mesh, layer) == (uid, id) => {
                HELD.with(|held| held.borrow_mut().as_mut().expect("just seen").fresh = true);
                if at == opacity {
                    return;
                }
            }
            Some(_) => self.paint_opacity_done(),
            None => {}
        }
        let Some(row) = self.paint_row(id) else { return };
        let fresh = HELD.with(|held| held.borrow().is_none());
        if fresh {
            // The first move of a drag; one that goes nowhere opens nothing.
            if row.opacity == opacity {
                return;
            }
            HELD.with(|held| *held.borrow_mut() = Some(Held { uid, was: row.clone(), opacity: row.opacity, fresh: true }));
        }
        self.doc.mesh.preview_paint_row(PaintRow { opacity, ..row });
        HELD.with(|held| {
            if let Some(held) = held.borrow_mut().as_mut() {
                held.opacity = opacity;
            }
        });
        self.redraw = true;
    }

    /// The Opacity slider was let go: the drag becomes one undo step.
    fn paint_opacity_done(&mut self) {
        let Some(held) = HELD.with(|held| held.borrow_mut().take()) else { return };
        if held.uid != self.doc.mesh.uid {
            return;
        }
        let Some(now) = self.paint_row(held.was.id) else { return };
        // Back as it was at the press, and then set as one step.
        self.doc.mesh.preview_paint_row(held.was.clone());
        let percent = (now.opacity as f32 / 2.55).round();
        let name = quoted(&now);
        let result = self.doc.mesh.set_paint_row(now);
        self.paint_result(result, || format!("{name} is at {percent}% opacity."));
    }

    /// Before any other command: an Opacity slider still held is let go, so
    /// that its drag is a step of its own.
    pub(super) fn paint_before(&mut self, action: &Action) {
        if !matches!(action, Action::Paint(PaintAction::Opacity(..) | PaintAction::OpacityDone)) {
            self.paint_opacity_done();
        }
    }

    /// Once a frame: a held slider no command came for since the last look
    /// was let go without the card saying so (the card went away under it).
    pub(super) fn paint_watch(&mut self) {
        let stale = HELD.with(|held| match held.borrow_mut().as_mut() {
            Some(held) => !std::mem::replace(&mut held.fresh, false),
            None => false,
        });
        if stale {
            self.paint_opacity_done();
        }
    }

    /// Fill: the active layer painted all over `area` with the Paint card's
    /// colour (`colour`) and the scene's material (`material`).
    fn paint_fill(&mut self, area: FillArea, colour: bool, material: bool) {
        let slot = self.doc.paint_slot().filter(|_| material);
        let settings = self.doc.sculptor.settings.paint.clone();
        let mut channels = ChannelSet::NONE;
        if colour {
            channels = channels.with(ChannelSet::COLOUR);
        }
        if slot.is_some() {
            channels = channels.with(ChannelSet::MATERIAL);
        }
        if channels.is_empty() {
            let why = if material { "No material is picked to fill with: pick one on the Materials card." } else { "Nothing to fill with." };
            return self.status(why, false);
        }
        let colour_now = settings.colour;
        let texel = PaintSettings { slot, ..settings }.texel();
        let target = PaintTarget { texel, channels, mode: Toward::Paint };
        if let Some(slot) = slot.filter(|s| !self.doc.mesh.paint_table().contains(s)) {
            match self.doc.mesh.add_paint_material(slot) {
                Ok((_, step)) => self.doc.history.push_paint(step),
                Err(PaintRefusal::Unchanged) => {}
                Err(why) => return self.paint_refused(why),
            }
        }
        let result = self.doc.mesh.fill_paint(&target, area);
        if result.is_ok() && colour {
            self.doc.sculptor.settings.paint.remember(colour_now);
        }
        let what = match (colour, slot.is_some()) {
            (true, true) => "the colour and the material",
            (true, false) => "the colour",
            _ => "the material",
        };
        self.paint_result(result, || format!("Filled {} with {what}. Undo (Z) takes it back.", area_words(area)));
    }

    /// The eyedropper at window position `at`: the colour and the strongest
    /// material of the paint there become the ones to paint with.
    pub(super) fn paint_pick(&mut self, at: Vec2) -> Result<String, String> {
        self.sync_view();
        let placed = self.pick_placed(at).filter(|&(id, _)| self.doc.scene.shows_active_mesh(id));
        let Some((id, copy)) = placed else {
            return Err("The eyedropper picks paint from the selected object, and it is not under the pointer.".into());
        };
        self.doc.scene.aim_brush(id, copy);
        let local = self.view.local_to(self.doc.scene.brush_matrix());
        self.doc.scene.release_brush();
        let (origin, dir) = local.ray(at);
        let picked = crate::sculpt::paint::pick_along(&self.doc.mesh, origin, dir);
        let Some(picked) = picked.filter(|p| p.coverage > 0.5 / 255.0 || p.material.is_some()) else {
            return Err("There is no paint there to pick up.".into());
        };
        let mut said = Vec::new();
        if picked.coverage > 0.5 / 255.0 {
            let paint = &mut self.doc.sculptor.settings.paint;
            paint.colour = picked.colour;
            paint.remember(picked.colour);
            let [r, g, b] = picked.colour;
            said.push(format!("the colour (red {r}, green {g}, blue {b})"));
        }
        self.doc.opts.paint.active_material = picked.material;
        match picked.material.and_then(|slot| self.doc.opts.paint.palette.iter().find(|e| e.slot == slot)) {
            Some(entry) => said.push(format!("the material {}", entry.name)),
            None => said.push("no material (colour only)".into()),
        }
        self.toast("Picked");
        Ok(format!("Picked up {}: Paint lays them now.", said.join(" and ")))
    }

    /// A press on the model with the Paint brush in Sample mode: the
    /// eyedropper, and no stroke. Gives back whether the press was that.
    pub(super) fn paint_samples_press(&mut self, at: Vec2, shift: bool) -> bool {
        let s = &self.doc.sculptor.settings;
        if s.mask_mode || s.brush != Brush::Paint || s.paint.mode != PaintMode::Sample || shift {
            return false;
        }
        match self.paint_pick(at) {
            Ok(text) => self.status(text, true),
            Err(text) => self.status(text, false),
        }
        true
    }

    /// Whether a stroke about to begin must be refused because it would
    /// paint and has nowhere to (see `Doc::paint_prepare`); says why if so,
    /// and says what was made for it if something was.
    pub(super) fn paint_refuses_stroke(&mut self, shift: bool) -> bool {
        match self.doc.paint_prepare(shift) {
            Ok(Some(text)) => {
                self.status(text, true);
                false
            }
            Ok(None) => false,
            Err(why) => {
                self.status(why, false);
                true
            }
        }
    }

    /// P: the Paint brush in hand, or again, the brush that was in hand before it.
    pub(super) fn paint_key(&mut self) {
        let sculptor = &mut self.doc.sculptor;
        let (library, s) = (&mut sculptor.library, &mut sculptor.settings);
        if s.brush == Brush::Paint && !s.mask_mode {
            let back = s.paint.came_from.take().and_then(|name| library.named(&name));
            let back = back.unwrap_or(crate::sculpt::library::Library::base(Brush::Clay));
            library.select(back, s);
            let name = library.entries()[back].name.clone();
            self.toast(name);
        } else {
            s.paint.came_from = Some(library.entries()[library.selected(s)].name.clone());
            library.select(crate::sculpt::library::Library::base(Brush::Paint), s);
            self.toast("Paint");
        }
    }

    /// The Masking card's paint commands: a mask made from the paint, as one
    /// ordinary mask step.
    fn paint_mask(&mut self, by: Masking) {
        let mesh = &self.doc.mesh;
        if !mesh.has_paint() {
            return self.status("This object has no paint to mask by.", false);
        }
        let settings = &self.doc.sculptor.settings.paint;
        let (colour, tolerance, slot) = (settings.colour, settings.tolerance, self.doc.paint_slot());
        let values: Vec<f32> = match by {
            Masking::Colour => {
                let streams = crate::mesh::paint::export_streams(mesh).unwrap_or_default();
                streams.iter().map(|r| colour_match([r[0], r[1], r[2]], r[3], colour, tolerance)).collect()
            }
            Masking::Material => {
                let Some(slot) = slot else {
                    return self.status("Pick the material to mask by on the Materials card first.", false);
                };
                let Some(k) = mesh.paint_table().iter().position(|&s| s == slot) else {
                    return self.status("That material is not painted on this object.", false);
                };
                let streams = crate::mesh::paint::export_streams(mesh).unwrap_or_default();
                streams.iter().map(|r| r[8 + k] as f32 / 255.0).collect()
            }
            Masking::Layer => match layer_alpha(mesh) {
                Some(values) => values,
                None => return self.status("There is no active paint layer to mask by.", false),
            },
        };
        if values.len() != mesh.vertex_count() {
            return;
        }
        let delta = self.doc.mesh.remap_mask(|_, v, _| values[v as usize]);
        let masked = values.iter().filter(|&&m| m > 0.5).count();
        let text = match by {
            Masking::Colour => "where the paint is near the Paint card's colour",
            Masking::Material => "where the material is painted",
            Masking::Layer => "where the active paint layer has paint",
        };
        match delta {
            Some(delta) => {
                self.doc.history.push_stroke(delta);
                self.redraw = true;
                if masked == 0 {
                    self.status(format!("Nothing is masked: there is nowhere {text}."), true);
                } else {
                    self.status(format!("Masked {text}. Masked surface is safe from brushes; Invert to work only there."), true);
                }
            }
            None => self.status(format!("The mask is already {text}."), true),
        }
    }
}

/// What the Masking card masks by.
#[derive(Clone, Copy)]
enum Masking {
    Colour,
    Material,
    Layer,
}

/// How far painted colour `rgb` (with coverage `cover`) is inside the Mask
/// by Colour of `colour` at `tolerance`: 1 within it, easing to 0 a little
/// past it, and no more than the paint covers. Distance is in sRGB, the
/// way the colours look, as a share of the most two colours can differ.
pub(crate) fn colour_match(rgb: [u8; 3], cover: u8, colour: [u8; 3], tolerance: f32) -> f32 {
    const SOFT: f32 = 0.05;
    let d2: f32 = (0..3).map(|k| ((rgb[k] as f32 - colour[k] as f32) / 255.0).powi(2)).sum();
    let distance = (d2 / 3.0).sqrt();
    let inside = ((tolerance + SOFT - distance) / SOFT).clamp(0.0, 1.0);
    inside * cover as f32 / 255.0
}

/// How much paint the active layer holds at each vertex: the most of its
/// colour's coverage and its material, roughness and metallic weights.
fn layer_alpha(mesh: &Mesh) -> Option<Vec<f32>> {
    use rayon::prelude::*;
    let stack = mesh.paint_stack()?;
    let layer = stack.layer(stack.active?)?;
    let data = &layer.data;
    Some(
        (0..mesh.vertex_count())
            .into_par_iter()
            .with_min_len(1 << 13)
            .map(|v| {
                let t = data.texel(v);
                let most = t.colour.0[3].max(t.material.0[1]).max(t.rough_metal.0[1]).max(t.rough_metal.0[3]);
                most as f32 / 255.0
            })
            .collect(),
    )
}

/// The status line for a row's change from `was` to `now`.
fn row_words(was: &PaintRow, now: &PaintRow) -> String {
    let name = quoted(now);
    if was.visible != now.visible {
        return if now.visible { format!("{name} is showing again.") } else { format!("{name} is hidden. Its paint is kept.") };
    }
    if was.locked != now.locked {
        return if now.locked {
            format!("{name} is locked: no stroke, fill or clear changes it.")
        } else {
            format!("{name} is unlocked.")
        };
    }
    if was.colour_blend != now.colour_blend {
        return format!("{name} blends its colour by {}.", now.colour_blend.name());
    }
    if was.material_blend != now.material_blend {
        let how = if now.material_blend == MaterialBlend::Add { "adds its material to" } else { "lays its material over" };
        return format!("{name} {how} what is under it.");
    }
    if was.mask != now.mask {
        return format!("{name} shows {}.", mask_words(now.mask));
    }
    if was.channels != now.channels {
        return format!("{name} shows {}.", channel_words(now.channels));
    }
    format!("{name} changed.")
}

pub(crate) fn mask_words(mask: MaskSource) -> String {
    match mask {
        MaskSource::None => "everywhere".into(),
        MaskSource::Paintable => "where its own mask is open (paint it with Paint Layer Mask)".into(),
        MaskSource::SculptMask { invert: false } => "where the surface is masked".into(),
        MaskSource::SculptMask { invert: true } => "where the surface is not masked".into(),
        MaskSource::Cavity => "in the recesses of the surface".into(),
        MaskSource::FaceGroup(g) => format!("on face group {g}"),
    }
}

fn channel_words(set: ChannelSet) -> String {
    let names = [
        (ChannelSet::COLOUR, "colour"),
        (ChannelSet::MATERIAL, "material"),
        (ChannelSet::ROUGHNESS, "roughness"),
        (ChannelSet::METALLIC, "metallic"),
    ];
    let on: Vec<&str> = names.iter().filter(|(c, _)| set.has(*c)).map(|(_, n)| *n).collect();
    if on.is_empty() { "none of its paint".into() } else { format!("its {}", on.join(", ")) }
}

// ---- for the Claude bridge -----------------------------------------------------

/// A mesh's paint layers as the bridge reports them, top of the stack first.
pub(super) fn state_of(mesh: Option<&Mesh>) -> Value {
    let Some(mesh) = mesh.filter(|m| m.has_paint()) else { return json!([]) };
    let active = mesh.paint_active();
    let rows: Vec<Value> = mesh.paint_rows().into_iter().rev().map(|r| row_json(&r, active == Some(r.id))).collect();
    Value::Array(rows)
}

fn row_json(r: &PaintRow, active: bool) -> Value {
    json!({
        "id": r.id,
        "name": r.name,
        "active": active,
        "visible": r.visible,
        "locked": r.locked,
        "opacity_percent": (r.opacity as f32 / 2.55).round(),
        "blend": r.colour_blend.name(),
        "material_blend": if r.material_blend == MaterialBlend::Add { "add" } else { "normal" },
        "mask": mask_name(r.mask),
        "channels": {
            "colour": r.channels.has(ChannelSet::COLOUR),
            "material": r.channels.has(ChannelSet::MATERIAL),
            "roughness": r.channels.has(ChannelSet::ROUGHNESS),
            "metallic": r.channels.has(ChannelSet::METALLIC),
        },
    })
}

fn mask_name(mask: MaskSource) -> String {
    match mask {
        MaskSource::None => "none".into(),
        MaskSource::Paintable => "paintable".into(),
        MaskSource::SculptMask { invert: false } => "sculpt_mask".into(),
        MaskSource::SculptMask { invert: true } => "sculpt_mask_inverted".into(),
        MaskSource::Cavity => "cavity".into(),
        MaskSource::FaceGroup(g) => format!("face_group {g}"),
    }
}

/// The Paint brush's settings as the bridge reports them.
pub(super) fn settings_of(s: &PaintSettings, slot: Option<u8>) -> Value {
    let [r, g, b] = s.colour;
    json!({
        "colour": [r, g, b],
        "colour_hex": format!("#{r:02x}{g:02x}{b:02x}"),
        "recent": s.recent,
        "mode": s.mode.name(),
        "opacity_percent": (s.opacity * 100.0).round(),
        "channels": { "rgb": s.rgb, "material": s.material, "roughness": s.roughness, "metallic": s.metallic },
        "roughness_percent": (s.roughness_value * 100.0).round(),
        "metallic_percent": (s.metallic_value * 100.0).round(),
        "zadd": s.zadd,
        "zadd_brush": s.zadd_brush.name(),
        "layer_mask": s.layer_mask,
        "tolerance_percent": (s.tolerance * 100.0).round(),
        "material_slot": slot,
    })
}

/// A colour from the bridge: `[r, g, b]` (0 to 255) or `"#rrggbb"`.
fn colour_arg(value: &Value) -> Result<[u8; 3], String> {
    const SHAPE: &str = "colour is [red, green, blue], each 0 to 255, or \"#rrggbb\".";
    if let Some(text) = value.as_str() {
        let hex = text.trim().trim_start_matches('#');
        if hex.len() != 6 {
            return Err(SHAPE.into());
        }
        let part = |k: usize| u8::from_str_radix(&hex[k..k + 2], 16).map_err(|_| SHAPE.to_owned());
        return Ok([part(0)?, part(2)?, part(4)?]);
    }
    let list = value.as_array().filter(|l| l.len() == 3).ok_or(SHAPE)?;
    let mut out = [0u8; 3];
    for (slot, v) in out.iter_mut().zip(list) {
        let x = v.as_f64().filter(|x| (0.0..=255.0).contains(x)).ok_or(SHAPE)?;
        *slot = x.round() as u8;
    }
    Ok(out)
}

fn percent(args: &Value, name: &str) -> Result<Option<f32>, String> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_f64()
            .filter(|x| (0.0..=100.0).contains(x))
            .map(|x| Some(x as f32 / 100.0))
            .ok_or_else(|| format!("{name} is a number from 0 to 100.")),
    }
}

fn mask_arg(text: &str, group: Option<u16>) -> Result<MaskSource, String> {
    Ok(match text.trim().to_lowercase().replace([' ', '-'], "_").as_str() {
        "none" => MaskSource::None,
        "paintable" => MaskSource::Paintable,
        "sculpt_mask" => MaskSource::SculptMask { invert: false },
        "sculpt_mask_inverted" => MaskSource::SculptMask { invert: true },
        "cavity" => MaskSource::Cavity,
        "face_group" => MaskSource::FaceGroup(group.ok_or("mask face_group needs group, the group's number.")?),
        _ => {
            return Err(
                "mask is none, paintable, sculpt_mask, sculpt_mask_inverted, cavity or face_group (with group).".into()
            );
        }
    })
}

impl Running {
    /// The layer an argument names (by id or name), or the active one.
    fn paint_layer_asked(&self, args: &Value) -> Result<PaintRow, String> {
        let rows = self.paint_rows();
        if rows.is_empty() {
            return Err("The active object has no paint layers. paint_layer with do: \"new\" makes one.".into());
        }
        let found = match args.get("layer") {
            None | Some(Value::Null) => {
                let active = self.doc.mesh.paint_active();
                rows.iter().find(|r| Some(r.id) == active)
            }
            Some(Value::Number(n)) => rows.iter().find(|r| n.as_u64() == Some(r.id as u64)),
            Some(Value::String(name)) => rows.iter().find(|r| r.name.eq_ignore_ascii_case(name.trim())),
            Some(_) => None,
        };
        found.cloned().ok_or_else(|| "There is no such paint layer. get_state lists them under paint_layers.".into())
    }

    /// The bridge's paint commands, by the name the `command` tool was given
    /// with the underscores taken out. `None` if `name` is not one of them.
    pub(super) fn paint_command(&mut self, name: &str, args: &Value) -> Option<Result<String, String>> {
        Some(match name {
            "paintlayer" => self.paint_layer_command(args),
            "paintsettings" => self.paint_settings_command(args),
            "pickpaint" => {
                let (Some(x), Some(y)) = (args.get("x").and_then(Value::as_f64), args.get("y").and_then(Value::as_f64)) else {
                    return Some(Err("pick_paint needs x and y, each from 0 to 1.".into()));
                };
                self.sync_view();
                let at = Vec2::new(x as f32, y as f32) * self.view.size;
                self.paint_pick(at).map(|text| format!("{text} {}", self.paint_settings_json()))
            }
            "maskbycolour" | "maskbycolor" => self.said(|app| app.act(Action::Paint(PaintAction::MaskByColour))),
            "maskbymaterial" => self.said(|app| app.act(Action::Paint(PaintAction::MaskByMaterial))),
            "maskbylayer" => self.said(|app| app.act(Action::Paint(PaintAction::MaskByLayer))),
            _ => return None,
        })
    }

    fn paint_settings_json(&self) -> Value {
        settings_of(&self.doc.sculptor.settings.paint, self.doc.paint_slot())
    }

    fn paint_settings_command(&mut self, args: &Value) -> Result<String, String> {
        let mut s = self.doc.sculptor.settings.paint.clone();
        if let Some(colour) = args.get("colour").or_else(|| args.get("color")).filter(|v| !v.is_null()) {
            s.colour = colour_arg(colour)?;
            s.remember(s.colour);
        }
        if let Some(mode) = args.get("mode").and_then(Value::as_str) {
            s.mode = PaintMode::named(mode).ok_or("mode is Paint, Erase, Blur, Smear or Sample.")?;
        }
        if let Some(x) = percent(args, "opacity")? {
            s.opacity = x.max(0.01);
        }
        if let Some(channels) = args.get("channels").filter(|v| v.is_object()) {
            let on = |name: &str, was: bool| channels.get(name).and_then(Value::as_bool).unwrap_or(was);
            (s.rgb, s.material, s.roughness, s.metallic) =
                (on("rgb", s.rgb), on("material", s.material), on("roughness", s.roughness), on("metallic", s.metallic));
        }
        if let Some(x) = percent(args, "roughness")? {
            s.roughness_value = x;
        }
        if let Some(x) = percent(args, "metallic")? {
            s.metallic_value = x;
        }
        if let Some(x) = percent(args, "tolerance")? {
            s.tolerance = x;
        }
        if let Some(on) = args.get("zadd").and_then(Value::as_bool) {
            s.zadd = on;
        }
        if let Some(name) = args.get("zadd_brush").and_then(Value::as_str) {
            s.zadd_brush = ZADD_BRUSHES.into_iter().find(|b| b.name().eq_ignore_ascii_case(name.trim())).ok_or_else(|| {
                let names: Vec<&str> = ZADD_BRUSHES.iter().map(|b| b.name()).collect();
                format!("zadd_brush is one of {}.", names.join(", "))
            })?;
        }
        if let Some(on) = args.get("layer_mask").and_then(Value::as_bool) {
            s.layer_mask = on;
        }
        self.doc.sculptor.settings.paint = s;
        Ok(self.paint_settings_json().to_string())
    }

    fn paint_layer_command(&mut self, args: &Value) -> Result<String, String> {
        let what = args.get("do").and_then(Value::as_str).ok_or(
            "paint_layer needs do: new, delete, select, rename, move, duplicate, merge_down, flatten, show, hide, lock, \
             unlock, opacity, blend, material_blend, channels, mask, recompute_cavity, fill or clear.",
        )?;
        let what = what.trim().to_lowercase().replace([' ', '-'], "_");
        let act = |app: &mut Running, action: PaintAction| app.said(|app| app.act(Action::Paint(action)));
        let result = match what.as_str() {
            "new" => {
                let result = act(self, PaintAction::New);
                if let (Ok(_), Some(name)) = (&result, args.get("layer_name").and_then(Value::as_str)) {
                    if let Some(id) = self.doc.mesh.paint_active() {
                        let _ = act(self, PaintAction::Rename(id, name.to_owned()));
                    }
                }
                result
            }
            "flatten" | "flatten_all" => act(self, PaintAction::FlattenAll),
            other => {
                let row = self.paint_layer_asked(args)?;
                let id = row.id;
                match other {
                    "delete" => act(self, PaintAction::Delete(id)),
                    "select" => act(self, PaintAction::Select(id)),
                    "rename" => {
                        let name = args.get("new_name").and_then(Value::as_str).ok_or("rename needs new_name.")?;
                        act(self, PaintAction::Rename(id, name.to_owned()))
                    }
                    "move" => {
                        // Counted from the top, as get_state lists them.
                        let place = args.get("to").and_then(Value::as_u64).ok_or("move needs to: 1 for the top, and so on.")?;
                        let count = self.paint_rows().len();
                        let from_bottom = count.saturating_sub(place.max(1) as usize);
                        act(self, PaintAction::Move(id, from_bottom))
                    }
                    "duplicate" => act(self, PaintAction::Duplicate(id)),
                    "merge_down" => act(self, PaintAction::MergeDown(id)),
                    "show" => act(self, PaintAction::Row(PaintRow { visible: true, ..row })),
                    "hide" => act(self, PaintAction::Row(PaintRow { visible: false, ..row })),
                    "lock" => act(self, PaintAction::Row(PaintRow { locked: true, ..row })),
                    "unlock" => act(self, PaintAction::Row(PaintRow { locked: false, ..row })),
                    "opacity" => {
                        let x = percent(args, "percent")?.ok_or("opacity needs percent, from 0 to 100.")?;
                        let byte = (x * 255.0).round() as u8;
                        // The slider dragged there and let go: one undo step.
                        let _ = act(self, PaintAction::Opacity(id, byte));
                        act(self, PaintAction::OpacityDone)
                    }
                    "blend" => {
                        let name = args.get("blend").and_then(Value::as_str).ok_or("blend needs blend, a mode's name.")?;
                        let wanted = name.trim().to_lowercase().replace([' ', '_', '-'], "");
                        let blend = ColourBlend::ALL
                            .into_iter()
                            .find(|b| b.name().to_lowercase().replace(' ', "") == wanted || (wanted == "color" && *b == ColourBlend::Colour))
                            .ok_or_else(|| {
                                let names: Vec<&str> = ColourBlend::ALL.iter().map(|b| b.name()).collect();
                                format!("blend is one of {}.", names.join(", "))
                            })?;
                        act(self, PaintAction::Row(PaintRow { colour_blend: blend, ..row }))
                    }
                    "material_blend" => {
                        let add = match args.get("blend").and_then(Value::as_str).map(|t| t.trim().to_lowercase()) {
                            Some(t) if t == "add" => true,
                            Some(t) if t == "normal" => false,
                            _ => return Err("material_blend needs blend: normal or add.".into()),
                        };
                        let material_blend = if add { MaterialBlend::Add } else { MaterialBlend::Normal };
                        act(self, PaintAction::Row(PaintRow { material_blend, ..row }))
                    }
                    "channels" => {
                        let channels = args.get("channels").filter(|v| v.is_object()).ok_or(
                            "channels needs channels: { colour, material, roughness, metallic }, each true or false.",
                        )?;
                        let mut set = row.channels;
                        for (name, bit) in [
                            ("colour", ChannelSet::COLOUR),
                            ("material", ChannelSet::MATERIAL),
                            ("roughness", ChannelSet::ROUGHNESS),
                            ("metallic", ChannelSet::METALLIC),
                        ] {
                            match channels.get(name).and_then(Value::as_bool) {
                                Some(true) => set = set.with(bit),
                                Some(false) => set = set.without(bit),
                                None => {}
                            }
                        }
                        act(self, PaintAction::Row(PaintRow { channels: set, ..row }))
                    }
                    "mask" => {
                        let text = args.get("mask").and_then(Value::as_str).ok_or("mask needs mask.")?;
                        let group = args.get("group").and_then(Value::as_u64).map(|g| g.min(u16::MAX as u64) as u16);
                        let mask = mask_arg(text, group)?;
                        act(self, PaintAction::Row(PaintRow { mask, ..row }))
                    }
                    "recompute_cavity" => act(self, PaintAction::RecomputeCavity(id)),
                    "clear" => act(self, PaintAction::Clear(id)),
                    "fill" => {
                        if self.doc.mesh.paint_active() != Some(id) {
                            act(self, PaintAction::Select(id))?;
                        }
                        let area = match args.get("area").and_then(Value::as_str).unwrap_or("all") {
                            "all" => FillArea::All,
                            "mask" | "masked" => FillArea::SculptMask { invert: false },
                            "unmasked" | "mask_inverted" => FillArea::SculptMask { invert: true },
                            "group" => FillArea::Group(
                                args.get("group").and_then(Value::as_u64).ok_or("area group needs group.")?.min(u16::MAX as u64)
                                    as u16,
                            ),
                            _ => return Err("area is all, masked, unmasked or group.".into()),
                        };
                        let colour = args.get("fill_colour").and_then(Value::as_bool).unwrap_or(true);
                        let material = args.get("fill_material").and_then(Value::as_bool).unwrap_or(true);
                        act(self, PaintAction::Fill { area, colour, material })
                    }
                    _ => return Err(format!("paint_layer has no do \"{other}\".")),
                }
            }
        };
        result.map(|text| format!("{text} {}", state_of(Some(&self.doc.mesh))))
    }
}
