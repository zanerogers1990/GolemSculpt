//! Sculpt layers on the web: the Layers card on the Mesh tab, and what its
//! commands do.
//!
//! What a layer is, and what every kind of edit does to one, is the
//! desktop's `mesh/layers.rs`, compiled here as it is: strokes already go
//! into the layer that records, and their undo records carry the layers.
//! This file is the desktop's `ui/layers.rs` (the card) and `app/layers.rs`
//! (carrying its commands out) cut down to the web version, which has one
//! undo list (`Step::Layers`) and no Pose brush.
//!
//! - Every command is one undo step. A drag of a Strength slider is one step
//!   however far it went: the first move opens it, each frame the slider is
//!   held puts the shape where the slider is, and letting go closes it.
//!   Anything else (a stroke, undo, another command) closes it first.
//! - A stroke that would move the surface is refused while the layer that
//!   records is hidden or at 0% (`Mesh::layer_refusal`), and Erase Layer
//!   with no layer to rub out (`erase::refusal`), with a line saying why.
//! - Whatever puts a new mesh in the object's place (Remesh, Decimate, a new
//!   shape) builds it from the shape as it shows, so the layers are merged
//!   into it; the old mesh, layers and all, is what that step's undo keeps.

use std::cell::RefCell;

use eframe::egui::{self, Align2, Color32, Id, Key, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

use super::ui::theme::{self, ACCENT, ButtonStyle};
use super::ui::{Pictures, tip};
use super::{GolemApp, Step};
use crate::mesh::Mesh;
use crate::mesh::layers::{LayerRow, LayersStep, MOST_LAYERS, StrengthDrag};

/// The card's colour on the panel, as on the desktop.
const TINT: Color32 = Color32::from_rgb(235, 150, 120);
/// The dot of the layer that records.
const RECORDING: Color32 = Color32::from_rgb(240, 84, 72);
/// The warning colour of the desktop's notes.
const WARNING: Color32 = Color32::from_rgb(242, 190, 77);
/// The height of a row's first line: dot, name, eye and bin.
const LINE: f32 = 24.0;
/// Room round a row's contents inside its plate.
const PAD: f32 = 6.0;
/// How wide the Bake button beside the slider is.
const BAKE: f32 = 50.0;

/// What the card asks for.
pub enum LayerAction {
    New,
    Rename(u32, String),
    /// Record on this layer, or on none.
    Record(Option<u32>),
    /// The Strength slider of a layer is held at this percent.
    Strength(u32, u8),
    StrengthDone,
    Show(u32, bool),
    Delete(u32),
    Bake(u32),
    BakeAll,
}

/// A Strength slider that is being held.
struct Held {
    /// The mesh it is on (`Mesh::uid`).
    uid: u64,
    layer: u32,
    percent: u8,
    drag: StrengthDrag,
    /// A command for it has come since the last frame's look.
    fresh: bool,
}

/// A layer's name being typed over.
#[derive(Clone)]
struct Rename {
    mesh: u64,
    id: u32,
    text: String,
    fresh: bool,
}

thread_local! {
    static HELD: RefCell<Option<Held>> = const { RefCell::new(None) };
}

fn rename_id() -> Id {
    Id::new("web_layer_rename")
}

fn quoted(row: &LayerRow) -> String {
    format!("\"{}\"", row.name)
}

/// "3 layers, recording Pores".
fn summary(rows: &[LayerRow]) -> String {
    let count = match rows.len() {
        0 => return String::new(),
        1 => "1 layer".to_owned(),
        n => format!("{n} layers"),
    };
    match rows.iter().find(|row| row.recording) {
        Some(row) => format!("{count}, recording {}", row.name),
        None => count,
    }
}

/// "184 MB", or "1.2 GB" from a thousand megabytes up.
pub(super) fn megabytes(bytes: usize) -> String {
    let mb = bytes as f64 / (1 << 20) as f64;
    if mb >= 1000.0 { format!("{:.1} GB", mb / 1024.0) } else { format!("{:.0} MB", mb.max(1.0)) }
}

impl GolemApp {
    fn layer_row(&self, id: u32) -> Option<LayerRow> {
        self.mesh.layers.as_ref().and_then(|layers| layers.row(id))
    }

    /// Records a change to the layers that the mesh has already made.
    fn layers_changed(&mut self, step: LayersStep) {
        self.push_undo(Step::Layers(self.active, step));
        self.mark_changed();
    }

    /// Says why Strength, the eye and Delete cannot be used just now, if
    /// they cannot (the object is below its finest subdivision level).
    fn layer_strength_refused(&mut self) -> bool {
        match self.mesh.layer_strength_refusal() {
            Some(why) => {
                self.status = why.to_owned();
                true
            }
            None => false,
        }
    }

    /// Why a stroke about to begin on the active object has to be refused,
    /// if it has: the layer that would record it is hidden or at 0%, or it
    /// is Erase Layer's with no layer to rub out. Mask strokes never are.
    pub(super) fn layer_stroke_refusal(&self, smooth_held: bool) -> Option<String> {
        let s = &self.sculptor.settings;
        if s.mask_mode {
            return None;
        }
        if let Some(why) = self.mesh.layer_refusal() {
            return Some(why);
        }
        let erasing = self.sculptor.effective_brush(smooth_held) == crate::sculpt::Brush::EraseLayer;
        if erasing { crate::sculpt::erase::refusal(&self.mesh) } else { None }
    }

    pub(super) fn layer_act(&mut self, action: LayerAction) {
        if !matches!(action, LayerAction::Strength(..) | LayerAction::StrengthDone) {
            self.layer_strength_done();
        }
        match action {
            LayerAction::New => self.layer_new(),
            LayerAction::Rename(id, name) => {
                if let Some(step) = self.mesh.rename_layer(id, &name) {
                    self.layers_changed(step);
                    self.status = format!("The layer is now called \"{}\".", name.trim());
                }
            }
            LayerAction::Record(id) => self.layer_record(id),
            LayerAction::Strength(id, percent) => self.layer_strength(id, percent),
            LayerAction::StrengthDone => self.layer_strength_done(),
            LayerAction::Show(id, on) => self.layer_show(id, on),
            LayerAction::Delete(id) => self.layer_remove(id, false),
            LayerAction::Bake(id) => self.layer_remove(id, true),
            LayerAction::BakeAll => self.layer_bake_all(),
        }
    }

    fn layer_new(&mut self) {
        if self.mesh.layer_count() >= MOST_LAYERS {
            self.status = format!("An object can have {MOST_LAYERS} layers. Bake the ones that are finished to make room.");
            return;
        }
        let (id, step) = self.mesh.add_layer(None);
        self.layers_changed(step);
        let name = self.layer_row(id).map_or(String::new(), |row| quoted(&row));
        self.status = format!(
            "Added the layer {name}. It is recording: what you sculpt now goes into it, and can be turned down, hidden \
             or deleted without touching the rest. Ctrl+Z removes it."
        );
    }

    fn layer_record(&mut self, id: Option<u32>) {
        let Some(step) = self.mesh.record_on_layer(id) else { return };
        self.layers_changed(step);
        self.status = match id.and_then(|id| self.layer_row(id)) {
            Some(row) if !row.on || row.percent == 0 => format!(
                "Recording on {}, but it is {}: show it and raise its Strength before sculpting.",
                quoted(&row),
                if row.on { "at 0%" } else { "hidden" }
            ),
            Some(row) => format!("Recording on {}: what you sculpt goes into this layer.", quoted(&row)),
            None => "No layer is recording: what you sculpt changes the model underneath, and the layers ride on it.".to_owned(),
        };
    }

    /// The slider of layer `id` is held at `percent`.
    fn layer_strength(&mut self, id: u32, percent: u8) {
        let percent = percent.min(100);
        let uid = self.mesh.uid;
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
            let Some(drag) = self.mesh.drag_layer(id) else { return };
            HELD.with(|held| *held.borrow_mut() = Some(Held { uid, layer: id, percent: row.percent, drag, fresh: true }));
        }
        HELD.with(|held| {
            let mut held = held.borrow_mut();
            let held = held.as_mut().expect("opened above");
            self.mesh.drag_layer_to(&held.drag, row.on, percent);
            held.percent = percent;
        });
    }

    /// The slider was let go (or something else came first): the drag
    /// becomes one undo step.
    pub(super) fn layer_strength_done(&mut self) {
        let Some(held) = HELD.with(|held| held.borrow_mut().take()) else { return };
        if held.uid != self.mesh.uid {
            return;
        }
        let Some(step) = self.mesh.end_layer_drag(held.drag) else { return };
        self.layers_changed(step);
        if let Some(row) = self.layer_row(held.layer) {
            let mut text = format!("{} is at {}%.", quoted(&row), row.percent);
            if row.recording && row.percent == 0 {
                text += " It is the layer that records, so raise it again before sculpting.";
            }
            self.status = text;
        }
    }

    /// Once a frame: a held slider no command came for since the last look
    /// was let go without the card saying so (the card went away under it).
    pub(super) fn layers_watch(&mut self) {
        let stale = HELD.with(|held| match held.borrow_mut().as_mut() {
            Some(held) => !std::mem::replace(&mut held.fresh, false),
            None => false,
        });
        if stale {
            self.layer_strength_done();
        }
    }

    fn layer_show(&mut self, id: u32, on: bool) {
        if self.layer_strength_refused() {
            return;
        }
        let Some(row) = self.layer_row(id) else { return };
        let Some(step) = self.mesh.set_layer(id, on, row.percent) else { return };
        self.layers_changed(step);
        self.status = if on {
            format!("{} is showing again, at {}%.", quoted(&row), row.percent)
        } else {
            format!("{} is hidden: the model is shown without it. What was sculpted on it is kept.", quoted(&row))
        };
    }

    /// Delete, or with `bake` Bake.
    fn layer_remove(&mut self, id: u32, bake: bool) {
        if !bake && self.layer_strength_refused() {
            return;
        }
        let Some(row) = self.layer_row(id) else { return };
        let Some(step) = self.mesh.remove_layer(id, bake) else { return };
        self.layers_changed(step);
        let name = quoted(&row);
        self.status = match (bake, row.on && row.percent > 0) {
            (false, _) => format!("Deleted the layer {name} and what it added. Ctrl+Z brings it back."),
            (true, true) => format!(
                "Baked {name} into the model at {}%: the shape is the same and the layer is gone. Ctrl+Z brings it back.",
                row.percent
            ),
            (true, false) => format!(
                "{name} was not showing, so there was nothing of it to bake: the shape is the same and the layer is gone. \
                 Ctrl+Z brings it back."
            ),
        };
    }

    fn layer_bake_all(&mut self) {
        let count = self.mesh.layer_count();
        let Some(step) = self.mesh.bake_all_layers() else {
            self.status = "This object has no layers to bake.".into();
            return;
        };
        self.layers_changed(step);
        let layers = if count == 1 { "The layer is".to_owned() } else { format!("All {count} layers are") };
        self.status = format!("{layers} part of the model now, as they showed: the shape is the same. Ctrl+Z brings them back.");
    }

    // ---- the card ---------------------------------------------------------------

    /// The Layers card (Mesh tab), as the desktop's.
    pub(super) fn layers_card(&mut self, ui: &mut Ui) {
        let tips = self.ui.tips;
        let mesh = &self.mesh;
        let rows = mesh.layer_rows();
        let pictures = &self.ui.pictures;
        let mut actions = Vec::new();
        theme::category(ui, "Layers", TINT, &summary(&rows), false, |ui| {
            theme::about(
                ui,
                "Passes of sculpting kept apart from the model. Each can be turned down, hidden or deleted without \
                 touching the rest. The red dot marks the layer that records what you sculpt; with no dot lit you sculpt \
                 the model underneath, and the layers ride on it.\n\n\
                 Remesh, Decimate and the other commands that rebuild the mesh merge the layers into the model first. \
                 Ctrl+Z brings them back.",
            );
            new_layer(ui, rows.len(), tips, &mut actions);
            if rows.is_empty() {
                return;
            }
            ui.add_space(4.0);
            let held = mesh.layer_strength_refusal().is_some();
            for row in &rows {
                layer(ui, mesh.uid, row, held, pictures, tips, &mut actions);
                ui.add_space(4.0);
            }
            notes(ui, &rows, held);
            let help = "Makes every layer part of the model as it shows now, and empties the list: the shape does not \
                        change. A layer that is turned down is baked turned down, and a hidden one adds nothing. Ctrl+Z \
                        brings them back.";
            if tip(theme::button(ui, ui.available_width(), "Bake All", ButtonStyle::default()), tips, help).clicked() {
                actions.push(LayerAction::BakeAll);
            }
            cost(ui, mesh, rows.len());
        });
        for action in actions {
            self.layer_act(action);
        }
    }
}

/// The New Layer button.
fn new_layer(ui: &mut Ui, count: usize, tips: bool, actions: &mut Vec<LayerAction>) {
    ui.add_enabled_ui(count < MOST_LAYERS, |ui| {
        let style = ButtonStyle { active: Some(ACCENT()), ..Default::default() };
        let help = "Adds an empty layer and starts recording on it: what you sculpt from now on goes into the layer, and \
                    the model underneath is left as it is. Ctrl+Z removes it.";
        if tip(theme::button(ui, ui.available_width(), "New Layer", style), tips, help).clicked() {
            actions.push(LayerAction::New);
        }
    });
    if count >= MOST_LAYERS {
        theme::hint(ui, &format!("An object can have {MOST_LAYERS} layers. Bake the ones that are finished."));
    }
}

/// The parts of a row's plate: the dot, the name, the eye, the bin, the
/// slider and Bake.
fn parts(plate: Rect) -> [Rect; 6] {
    let inner = plate.shrink(PAD);
    let line = Rect::from_min_size(inner.min, vec2(inner.width(), LINE));
    let dot = Rect::from_min_size(line.min, vec2(26.0, LINE));
    let bin = Rect::from_min_max(pos2(line.right() - 24.0, line.top()), line.max);
    let eye = bin.translate(vec2(-28.0, 0.0));
    let name = Rect::from_min_max(pos2(dot.right() + 2.0, line.top()), pos2(eye.left() - 6.0, line.bottom()));
    let under = Rect::from_min_max(pos2(inner.left(), line.bottom() + 2.0), inner.max);
    let bake = Rect::from_min_max(pos2(under.right() - BAKE, under.bottom() - 24.0), under.max);
    let slider = Rect::from_min_max(under.min, pos2(bake.left() - 10.0, under.bottom()));
    [dot, name, eye, bin, slider, bake]
}

/// One layer's row. `held` is that strength cannot be changed just now
/// (the object is below its finest subdivision level).
fn layer(ui: &mut Ui, mesh: u64, row: &LayerRow, held: bool, pictures: &Pictures, tips: bool, actions: &mut Vec<LayerAction>) {
    let height = PAD * 2.0 + LINE + 2.0 + theme::SLIDER_HEIGHT;
    let (plate, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    if !ui.is_rect_visible(plate) {
        return;
    }
    let [dot, name, eye, bin, slider, bake] = parts(plate);
    let edge = if row.recording { RECORDING.gamma_multiply(0.8) } else { theme::CONTROL() };
    ui.painter().rect_filled(plate, 6.0, theme::FIELD());
    ui.painter().rect_stroke(plate, 6.0, Stroke::new(1.0, edge), StrokeKind::Inside);

    record_dot(ui, dot, row, tips, actions);
    layer_name(ui, name, mesh, row, tips, actions);
    let wait = if held { " On a lower subdivision level this waits for the finest one (Page Up)." } else { "" };

    // The eye.
    let response = ui.interact(eye, Id::new(("web_layer_eye", row.id)), if held { Sense::hover() } else { Sense::click() });
    let hovered = response.hovered() && !held;
    let help = format!("Shows this layer in the shape, or leaves it out. What was sculpted on it is kept.{wait}");
    if tip(response, tips, &help).clicked() {
        actions.push(LayerAction::Show(row.id, !row.on));
    }
    icon(ui, eye, pictures, if row.on { "eye" } else { "eye_off" }, if row.on { 235 } else { 90 }, hovered, held);

    // The bin.
    let response = ui.interact(bin, Id::new(("web_layer_bin", row.id)), if held { Sense::hover() } else { Sense::click() });
    let hovered = response.hovered() && !held;
    let help = format!("Deletes this layer and what it added to the shape. Ctrl+Z brings it back.{wait}");
    if tip(response, tips, &help).clicked() {
        actions.push(LayerAction::Delete(row.id));
    }
    ui.painter().rect_filled(bin, 5.0, theme::CONTROL());
    icon(ui, bin, pictures, "trash", 210, hovered, held);

    strength(ui, slider, row, held, tips, actions);

    // Bake.
    let response = ui.interact(bake, Id::new(("web_layer_bake", row.id)), Sense::click());
    let fill = if response.hovered() { theme::lit(theme::CONTROL()) } else { theme::CONTROL() };
    ui.painter().rect_filled(bake, 5.0, fill);
    ui.painter().text(bake.center(), Align2::CENTER_CENTER, "Bake", theme::font(12.0), theme::TEXT());
    let help = "Makes what this layer adds, as it shows now, part of the model underneath, and takes the layer off the \
                list: the shape does not change. Turned down, it is baked turned down. Ctrl+Z brings it back.";
    if tip(response, tips, help).clicked() {
        actions.push(LayerAction::Bake(row.id));
    }
}

/// A row's icon button, dimmed when it cannot be used.
fn icon(ui: &Ui, rect: Rect, pictures: &Pictures, name: &str, alpha: u8, hovered: bool, held: bool) {
    if hovered {
        ui.painter().rect_filled(rect, 5.0, Color32::from_white_alpha(20));
    }
    if let Some(icon) = pictures.icon(name) {
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        let alpha = if held { alpha / 3 } else { alpha };
        ui.painter().image(icon, Rect::from_center_size(rect.center(), Vec2::splat(15.0)), uv, Color32::from_white_alpha(alpha));
    }
}

/// The dot: lit on the layer that records. A click moves the recording to
/// this layer, or on the lit one stops it.
fn record_dot(ui: &mut Ui, rect: Rect, row: &LayerRow, tips: bool, actions: &mut Vec<LayerAction>) {
    let response = ui.interact(rect, Id::new(("web_layer_dot", row.id)), Sense::click());
    let centre = pos2(rect.left() + 11.0, rect.center().y);
    let painter = ui.painter();
    if row.recording {
        painter.circle_filled(centre, 6.5, RECORDING);
        painter.circle_stroke(centre, 9.0, Stroke::new(1.0, RECORDING.gamma_multiply(0.5)));
    } else {
        let ring = if response.hovered() { RECORDING.gamma_multiply(0.8) } else { theme::HINT() };
        painter.circle_stroke(centre, 6.5, Stroke::new(1.5, ring));
    }
    let help = if row.recording {
        "This layer is recording: what you sculpt goes into it. Click to stop, and sculpt the model underneath instead."
    } else {
        "Click to record on this layer: what you sculpt then goes into it and nowhere else. One layer records at a time."
    };
    if tip(response, tips, help).clicked() {
        actions.push(LayerAction::Record((!row.recording).then_some(row.id)));
    }
}

/// The name: shown, or being typed over after a double click (or tap).
fn layer_name(ui: &mut Ui, rect: Rect, mesh: u64, row: &LayerRow, tips: bool, actions: &mut Vec<LayerAction>) {
    let mut rename: Option<Rename> = ui.data(|data| data.get_temp(rename_id()));
    if let Some(edit) = rename.as_mut().filter(|edit| (edit.mesh, edit.id) == (mesh, row.id)) {
        let id = Id::new(("web_layer_rename_field", row.id));
        let field = egui::TextEdit::singleline(&mut edit.text).id(id).font(theme::font(12.0)).desired_width(rect.width());
        let response = ui.put(rect, field);
        let mut done = false;
        if edit.fresh {
            // Start with the whole name selected, ready to type over.
            edit.fresh = false;
            response.request_focus();
            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), id) {
                let end = egui::text::CCursor::new(edit.text.chars().count());
                let all = egui::text::CCursorRange::two(egui::text::CCursor::new(0), end);
                state.cursor.set_char_range(Some(all));
                state.store(ui.ctx(), id);
            }
        } else if response.lost_focus() {
            // Enter or a click elsewhere keeps the name, Escape drops it.
            let text = edit.text.trim();
            if !ui.input(|i| i.key_pressed(Key::Escape)) && !text.is_empty() {
                actions.push(LayerAction::Rename(row.id, text.to_owned()));
            }
            done = true;
        }
        let edit = edit.clone();
        ui.data_mut(|data| {
            if done {
                data.remove::<Rename>(rename_id());
            } else {
                data.insert_temp(rename_id(), edit);
            }
        });
        return;
    }
    let response = ui.interact(rect, Id::new(("web_layer_name", row.id)), Sense::click());
    let ink = if row.on { theme::TEXT() } else { theme::TEXT().gamma_multiply(0.55) };
    let clip = rect.intersect(ui.painter().clip_rect());
    ui.painter().with_clip_rect(clip).text(rect.left_center(), Align2::LEFT_CENTER, &row.name, theme::font(12.0), ink);
    if response.double_clicked() {
        let edit = Rename { mesh, id: row.id, text: row.name.clone(), fresh: true };
        ui.data_mut(|data| data.insert_temp(rename_id(), edit));
    }
    tip(response, tips, "Double-click to rename the layer.");
}

/// The Strength slider. While it is held it says where on every frame, and
/// when it is let go it says so: the drag is one undo step.
fn strength(ui: &mut Ui, rect: Rect, row: &LayerRow, held: bool, tips: bool, actions: &mut Vec<LayerAction>) {
    let sense = if held { Sense::hover() } else { Sense::click_and_drag() };
    let response = ui.interact(rect, Id::new(("web_layer_strength", row.id)), sense);
    let down = response.is_pointer_button_down_on() && !held;
    let mut percent = row.percent as f32;
    let response = theme::slider_in(ui, rect, response, "Strength", &mut percent, 0.0..=100.0, |v| format!("{v:.0}%"));
    let was_down_id = Id::new(("web_layer_strength_down", row.id));
    let was_down: bool = ui.data(|data| data.get_temp(was_down_id)).unwrap_or(false);
    if down {
        actions.push(LayerAction::Strength(row.id, percent.round().clamp(0.0, 100.0) as u8));
    } else if was_down {
        actions.push(LayerAction::StrengthDone);
    }
    ui.data_mut(|data| data.insert_temp(was_down_id, down));
    if held {
        ui.painter().rect_filled(rect, 0.0, theme::FIELD().gamma_multiply(0.6));
    }
    let help = if held {
        "How much of this layer is in the shape. On a lower subdivision level this waits for the finest one (Page Up): \
         the layer's detail is up there."
    } else {
        "How much of this layer is in the shape: 100% as it was sculpted, 0% none of it. Drag to try the sculpt stronger \
         or fainter; nothing is lost at any setting. One drag is one undo step."
    };
    tip(response, tips, help);
}

/// What the sculptor should know about the layers as they stand.
fn notes(ui: &mut Ui, rows: &[LayerRow], held: bool) {
    let warn = |ui: &mut Ui, text: &str| {
        ui.add(egui::Label::new(egui::RichText::new(text).font(theme::font(11.0)).italics().color(WARNING)).wrap());
    };
    match rows.iter().find(|row| row.recording) {
        Some(row) if !row.on || row.percent == 0 => warn(
            ui,
            &format!(
                "\"{}\" is recording but {}, so sculpting is off: there would be nothing to see it by. Show it and raise \
                 its Strength, or click its dot to sculpt the model underneath.",
                row.name,
                if row.on { "is at 0%" } else { "is hidden" }
            ),
        ),
        Some(row) if row.percent < 100 => warn(
            ui,
            &format!(
                "\"{}\" is recording at {}%. What you sculpt looks as you sculpt it and is kept {:.1} times as strong: \
                 raise the slider afterwards and it grows by that much.",
                row.name,
                row.percent,
                100.0 / row.percent as f32
            ),
        ),
        Some(_) => {}
        None => theme::hint(ui, "No layer is recording: you are sculpting the model underneath."),
    }
    if held {
        warn(
            ui,
            "This is a lower subdivision level. You can sculpt on a layer, make one and bake one here; Strength, the eye \
             and Delete wait for the finest level (Page Up), where the layers' detail is.",
        );
    }
}

/// What the layers take in memory.
fn cost(ui: &mut Ui, mesh: &Mesh, count: usize) {
    let own = mesh.layers.as_ref().map_or(0, |layers| layers.bytes());
    let finest = mesh.levels.as_ref().map_or(0, |levels| levels.layer_bytes());
    let layers = if count == 1 { "this layer".to_owned() } else { format!("these {count} layers") };
    theme::hint(ui, &format!("A layer takes 12 bytes for each vertex: {} for {layers} on this object.", megabytes(own + finest)));
}
