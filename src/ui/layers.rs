//! The Layers card of the Sculpt panel's Mesh tab: New Layer, the list of
//! the selected object's sculpt layers, Bake All, and what they cost.
//!
//! A row is one layer: the dot that says it is the one recording, its name
//! (double-click to rename, as in the object list), the eye, the bin, and
//! under them its Strength slider and Bake. The card only asks for things:
//! every press pushes a command the app carries out after the interface pass
//! (`app/layers.rs`), so each is one undo step however the frame falls. The
//! slider says where it is held on every frame it is held and that it was
//! let go when it is, so a whole drag is one step too.
//!
//! What a layer is, and its limits, are in `mesh/layers.rs`; the limits a
//! sculptor meets are said on the card where they bite: recording below 100%,
//! a layer that records but does not show, and a lower subdivision level.

use egui::{Align2, Color32, Id, Key, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

use super::assets::Assets;
use super::dyntopo::WARNING;
use super::levels::megabytes;
use super::theme::{self, ACCENT, ButtonStyle};
use super::tip;
use crate::doc::{Action, Doc, LayerAction};
use crate::mesh::Mesh;
use crate::mesh::layers::{LayerRow, MOST_LAYERS};

/// The card's colour on the panel.
const TINT: Color32 = Color32::from_rgb(235, 150, 120);
/// The dot of the layer that records.
const RECORDING: Color32 = Color32::from_rgb(240, 84, 72);

/// The height of a row's first line: dot, name, eye and bin.
const LINE: f32 = 24.0;
/// Room round a row's contents inside its plate.
const PAD: f32 = 6.0;
/// How wide the Bake button beside the slider is.
const BAKE: f32 = 50.0;

/// A layer's name being typed over.
#[derive(Clone)]
struct Rename {
    /// The mesh the layer is on (`Mesh::uid`) and its number there.
    mesh: u64,
    id: u32,
    text: String,
    /// The field has not been given the keyboard yet.
    fresh: bool,
}

fn rename_id() -> Id {
    Id::new("layer_rename")
}

/// "3 layers", and which records.
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

pub fn card(ui: &mut Ui, doc: &mut Doc, assets: &Assets, actions: &mut Vec<Action>) {
    let tips = doc.opts.tooltips;
    let mesh = &doc.mesh;
    let rows = mesh.layer_rows();
    theme::category(ui, "Layers", TINT, &summary(&rows), true, |ui| {
        theme::about(
            ui,
            "Passes of sculpting kept apart from the model. Each can be turned down, hidden or deleted without \
             touching the rest. The red dot marks the layer that records what you sculpt; with no dot lit you sculpt \
             the model underneath, and the layers ride on it.\n\n\
             Remesh, Decimate, Join, a boolean and the other commands that rebuild the mesh merge the layers into the \
             model first. Undo (Z) brings them back.",
        );
        new_layer(ui, rows.len(), tips, actions);
        if rows.is_empty() {
            return;
        }
        ui.add_space(4.0);
        let held = mesh.layer_strength_refusal().is_some();
        for row in &rows {
            layer(ui, mesh.uid, row, held, assets, tips, actions);
            ui.add_space(4.0);
        }
        notes(ui, &rows, held);
        let help = "Makes every layer part of the model as it shows now, and empties the list: the shape does not \
                    change. A layer that is turned down is baked turned down, and a hidden one adds nothing. Undo (Z) \
                    brings them back.";
        if tip(theme::button(ui, ui.available_width(), "Bake All", ButtonStyle::default()), tips, help).clicked() {
            actions.push(Action::Layer(LayerAction::BakeAll));
        }
        cost(ui, mesh, rows.len());
    });
}

/// The New Layer button.
fn new_layer(ui: &mut Ui, count: usize, tips: bool, actions: &mut Vec<Action>) {
    ui.add_enabled_ui(count < MOST_LAYERS, |ui| {
        let style = ButtonStyle { active: Some(ACCENT()), ..Default::default() };
        let help = "Adds an empty layer and starts recording on it: what you sculpt from now on goes into the layer, \
                    and the model underneath is left as it is. Undo (Z) removes it.";
        if tip(theme::button(ui, ui.available_width(), "New Layer", style), tips, help).clicked() {
            actions.push(Action::Layer(LayerAction::New(None)));
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
fn layer(ui: &mut Ui, mesh: u64, row: &LayerRow, held: bool, assets: &Assets, tips: bool, actions: &mut Vec<Action>) {
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
    let waits = " On a lower subdivision level this waits for the finest one (Page Up).";
    let wait = if held { waits } else { "" };

    // The eye.
    let response = ui.interact(eye, Id::new(("layer_eye", row.id)), if held { Sense::hover() } else { Sense::click() });
    let hovered = response.hovered() && !held;
    let help = format!("Shows this layer in the shape, or leaves it out. What was sculpted on it is kept.{wait}");
    if tip(response, tips, &help).clicked() {
        actions.push(Action::Layer(LayerAction::Show(row.id, !row.on)));
    }
    icon(ui, eye, assets, if row.on { "eye" } else { "eye_off" }, if row.on { 235 } else { 90 }, hovered, held);

    // The bin.
    let response = ui.interact(bin, Id::new(("layer_bin", row.id)), if held { Sense::hover() } else { Sense::click() });
    let hovered = response.hovered() && !held;
    let help = format!("Deletes this layer and what it added to the shape. Undo (Z) brings it back.{wait}");
    if tip(response, tips, &help).clicked() {
        actions.push(Action::Layer(LayerAction::Delete(row.id)));
    }
    ui.painter().rect_filled(bin, 5.0, theme::CONTROL());
    icon(ui, bin, assets, "trash", 210, hovered, held);

    strength(ui, slider, row, held, tips, actions);

    // Bake.
    let response = ui.interact(bake, Id::new(("layer_bake", row.id)), Sense::click());
    let fill = if response.hovered() { theme::lit(theme::CONTROL()) } else { theme::CONTROL() };
    ui.painter().rect_filled(bake, 5.0, fill);
    ui.painter().text(bake.center(), Align2::CENTER_CENTER, "Bake", theme::font(12.0), theme::TEXT());
    let help = "Makes what this layer adds, as it shows now, part of the model underneath, and takes the layer off \
                the list: the shape does not change. Turned down, it is baked turned down. Undo (Z) brings it back.";
    if tip(response, tips, help).clicked() {
        actions.push(Action::Layer(LayerAction::Bake(row.id)));
    }
}

/// A row's icon button, dimmed when it cannot be used.
fn icon(ui: &Ui, rect: Rect, assets: &Assets, name: &str, alpha: u8, hovered: bool, held: bool) {
    if hovered {
        ui.painter().rect_filled(rect, 5.0, Color32::from_white_alpha(20));
    }
    if let Some(icon) = assets.icon(name) {
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        let alpha = if held { alpha / 3 } else { alpha };
        ui.painter().image(icon, Rect::from_center_size(rect.center(), Vec2::splat(15.0)), uv, Color32::from_white_alpha(alpha));
    }
}

/// The dot: lit on the layer that records. A click moves the recording to
/// this layer, or on the lit one stops it.
fn record_dot(ui: &mut Ui, rect: Rect, row: &LayerRow, tips: bool, actions: &mut Vec<Action>) {
    let response = ui.interact(rect, Id::new(("layer_dot", row.id)), Sense::click());
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
        "This layer is recording: what you sculpt goes into it. Click to stop, and sculpt the model underneath \
         instead."
    } else {
        "Click to record on this layer: what you sculpt then goes into it and nowhere else. One layer records at a \
         time."
    };
    if tip(response, tips, help).clicked() {
        actions.push(Action::Layer(LayerAction::Record((!row.recording).then_some(row.id))));
    }
}

/// The name: shown, or being typed over after a double click.
fn layer_name(ui: &mut Ui, rect: Rect, mesh: u64, row: &LayerRow, tips: bool, actions: &mut Vec<Action>) {
    let mut rename: Option<Rename> = ui.data(|data| data.get_temp(rename_id()));
    if let Some(edit) = rename.as_mut().filter(|edit| (edit.mesh, edit.id) == (mesh, row.id)) {
        let id = Id::new(("layer_rename_field", row.id));
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
                actions.push(Action::Layer(LayerAction::Rename(row.id, text.to_owned())));
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
    let response = ui.interact(rect, Id::new(("layer_name", row.id)), Sense::click());
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
/// when it is let go it says so: the app makes one undo step of the drag.
fn strength(ui: &mut Ui, rect: Rect, row: &LayerRow, held: bool, tips: bool, actions: &mut Vec<Action>) {
    let sense = if held { Sense::hover() } else { Sense::click_and_drag() };
    let response = ui.interact(rect, Id::new(("layer_strength", row.id)), sense);
    let down = response.is_pointer_button_down_on() && !held;
    let mut percent = row.percent as f32;
    let response = theme::slider_in(ui, rect, response, "Strength", &mut percent, 0.0..=100.0, |v| format!("{v:.0}%"));
    let was_down_id = Id::new(("layer_strength_down", row.id));
    let was_down: bool = ui.data(|data| data.get_temp(was_down_id)).unwrap_or(false);
    if down {
        actions.push(Action::Layer(LayerAction::Strength(row.id, percent.round().clamp(0.0, 100.0) as u8)));
    } else if was_down {
        actions.push(Action::Layer(LayerAction::StrengthDone));
    }
    ui.data_mut(|data| data.insert_temp(was_down_id, down));
    if held {
        ui.painter().rect_filled(rect, 0.0, theme::FIELD().gamma_multiply(0.6));
    }
    let help = if held {
        "How much of this layer is in the shape. On a lower subdivision level this waits for the finest one (Page \
         Up): the layer's detail is up there."
    } else {
        "How much of this layer is in the shape: 100% as it was sculpted, 0% none of it. Drag to try the sculpt \
         stronger or fainter; nothing is lost at any setting. One drag is one undo step."
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
                "\"{}\" is recording but {}, so sculpting is off: there would be nothing to see it by. Show it and \
                 raise its Strength, or click its dot to sculpt the model underneath.",
                row.name,
                if row.on { "is at 0%" } else { "is hidden" }
            ),
        ),
        Some(row) if row.percent < 100 => warn(
            ui,
            &format!(
                "\"{}\" is recording at {}%. What you sculpt looks as you sculpt it and is kept {:.1} times as \
                 strong: raise the slider afterwards and it grows by that much.",
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
            "This is a lower subdivision level. You can sculpt on a layer, make one and bake one here; Strength, the \
             eye and Delete wait for the finest level (Page Up), where the layers' detail is.",
        );
    }
}

/// What the layers take in memory, and what merges them.
fn cost(ui: &mut Ui, mesh: &Mesh, count: usize) {
    let own = mesh.layers.as_ref().map_or(0, |layers| layers.bytes());
    let finest = mesh.levels.as_ref().map_or(0, |levels| levels.layer_bytes());
    let layers = if count == 1 { "this layer".to_owned() } else { format!("these {count} layers") };
    theme::hint(
        ui,
        &format!("A layer takes 12 bytes for each vertex: {} for {layers} on this object.", megabytes(own + finest)),
    );
}
