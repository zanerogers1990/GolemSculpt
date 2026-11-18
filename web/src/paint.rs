//! Painting on the web: the Paint brush's settings, the Paint Layers card
//! on the Mesh tab, what a Paint stroke needs before it begins, the
//! eyedropper (Sample mode and the I key), P, and Mask by Paint on the
//! Masking card.
//!
//! What a layer is and what every change to the stack does is the
//! desktop's `mesh/paint.rs`, and what the brush does is its
//! `sculpt/paint.rs`, both compiled here as they are. This file is the
//! desktop's `ui/paint.rs` (the cards, word for word) and `app/paint.rs`
//! (carrying their commands out) cut down to the web version:
//!
//! - Every command is one undo step (`Step::Paint`), and a drag of a
//!   layer's Opacity slider is one step however far it went.
//! - A Paint stroke on an object with no paint layer makes one first, as its
//!   own undo step, and says so; a locked layer refuses the stroke in words.
//! - Colour only. The desktop also paints materials (from its Materials
//!   card), roughness and metallic, which its lit renderer shows; the web
//!   version has neither, so those settings are left out. Paint made on the
//!   desktop keeps them in a scene file all the same.

use std::cell::RefCell;

use eframe::egui::{self, Align2, Color32, Id, Key, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};
use glam::Vec2 as GVec2;

use super::ui::theme::{self, ACCENT, ButtonStyle};
use super::ui::{Pictures, tip};
use super::{GolemApp, Step};
use crate::mesh::Mesh;
use crate::mesh::paint::{
    ChannelSet, ColourBlend, FillArea, MOST_PAINT_LAYERS, MaskSource, PaintMode as Toward, PaintRefusal, PaintRow,
    PaintStep, PaintTarget,
};
use crate::sculpt::Brush;
use crate::sculpt::library::Library;
use crate::sculpt::paint::{PaintMode, PaintSettings, ZADD_BRUSHES};

/// The card's colour on the panel.
const TINT: Color32 = Color32::from_rgb(236, 128, 186);
/// The dot of the layer strokes go to.
const ACTIVE: Color32 = Color32::from_rgb(236, 128, 186);

const LINE: f32 = 24.0;
const PAD: f32 = 6.0;
const GRIP: f32 = 14.0;

/// What the paint cards ask for.
#[derive(Clone, Debug, PartialEq)]
pub enum PaintAction {
    New,
    Duplicate(u32),
    MergeDown(u32),
    FlattenAll,
    Delete(u32),
    Rename(u32, String),
    Select(u32),
    /// Move the layer to this place in the stack (0 the bottom).
    Move(u32, usize),
    /// A row's settings: eye, lock, blend, mask.
    Row(PaintRow),
    Opacity(u32, u8),
    OpacityDone,
    Fill(FillArea),
    FillPickedGroup,
    Clear(u32),
    RecomputeCavity(u32),
    MaskByPickedGroup(u32),
    MaskByColour,
    MaskByLayer,
}

/// An Opacity slider that is being held.
struct Held {
    uid: u64,
    was: PaintRow,
    opacity: u8,
    fresh: bool,
}

thread_local! {
    static HELD: RefCell<Option<Held>> = const { RefCell::new(None) };
}

/// A layer's name being typed over.
#[derive(Clone)]
struct Rename {
    mesh: u64,
    id: u32,
    text: String,
    fresh: bool,
}

fn rename_id() -> Id {
    Id::new("paint_layer_rename")
}

/// A row being dragged up or down the stack.
#[derive(Clone, Copy)]
struct Dragging {
    id: u32,
}

fn drag_id() -> Id {
    Id::new("paint_layer_drag")
}

fn quoted(row: &PaintRow) -> String {
    format!("\"{}\"", row.name)
}

fn area_words(area: FillArea) -> String {
    match area {
        FillArea::All => "the whole layer".into(),
        FillArea::SculptMask { invert: false } => "what is masked".into(),
        FillArea::SculptMask { invert: true } => "what is not masked".into(),
        FillArea::Group(g) => format!("face group {g}"),
    }
}

fn summary(rows: &[PaintRow], active: Option<u32>) -> String {
    let count = match rows.len() {
        0 => return String::new(),
        1 => "1 layer".to_owned(),
        n => format!("{n} layers"),
    };
    match rows.iter().find(|r| Some(r.id) == active) {
        Some(row) => format!("{count}, on {}", row.name),
        None => count,
    }
}

fn colour32([r, g, b]: [u8; 3]) -> Color32 {
    Color32::from_rgb(r, g, b)
}

fn percent(v: f32) -> String {
    format!("{:.0}%", v * 100.0)
}

/// A row of buttons of which one is on (the desktop's `look::choice`).
fn choice(ui: &mut Ui, labels: &[&str], selected: usize) -> Option<usize> {
    let mut picked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let w = theme::row_width(ui, labels.len(), 6.0);
        for (i, label) in labels.iter().enumerate() {
            let style = ButtonStyle { active: (selected == i).then_some(ACCENT()), ..Default::default() };
            if theme::button(ui, w, label, style).clicked() {
                picked = Some(i);
            }
        }
    });
    picked
}

// ------------------------------------------------------------ the layers card

impl GolemApp {
    /// The Paint Layers card (the desktop's `ui::paint::card`).
    pub(super) fn paint_card(&mut self, ui: &mut Ui) {
        let tips = self.ui.tips;
        let mut actions = Vec::new();
        let picked_group = self.group_picked().map(|p| p.group);
        let mesh = &self.mesh;
        let rows = mesh.paint_rows();
        let active = mesh.paint_active();
        let pictures = &self.ui.pictures;
        let paint = &self.sculptor.settings.paint;
        theme::category(ui, "Paint Layers", TINT, &summary(&rows, active), false, |ui| {
            theme::about(
                ui,
                "Colour painted on the model, in layers like a painting program's. The lit dot marks the layer the \
                 Paint brush paints on (P takes the brush up). Drag a row by its grip to move it up or down.",
            );
            commands(ui, &rows, active, tips, &mut actions);
            if rows.is_empty() {
                theme::hint(ui, "This object has no paint yet. The first Paint stroke makes a layer for it.");
                return;
            }
            ui.add_space(4.0);
            let mut plates = Vec::with_capacity(rows.len());
            let dragging: Option<Dragging> = ui.data(|d| d.get_temp(drag_id()));
            for row in rows.iter().rev() {
                let plate =
                    layer(ui, mesh.uid, row, active == Some(row.id), dragging.map(|d| d.id), pictures, tips, &mut actions);
                plates.push((row.id, plate));
                ui.add_space(4.0);
            }
            drop_target(ui, &plates, rows.len(), dragging, &mut actions);
            if let Some(row) = rows.iter().find(|r| Some(r.id) == active) {
                ui.add_space(2.0);
                active_settings(ui, row, picked_group, tips, &mut actions);
                fill(ui, row, paint, tips, &mut actions);
            }
            cost(ui, mesh, rows.len());
        });
        for action in actions {
            self.paint_act(action);
        }
    }
}

/// New, Duplicate, Merge Down, Flatten All.
fn commands(ui: &mut Ui, rows: &[PaintRow], active: Option<u32>, tips: bool, actions: &mut Vec<PaintAction>) {
    let full = rows.len() >= MOST_PAINT_LAYERS;
    let below = active.and_then(|id| rows.iter().position(|r| r.id == id)).is_some_and(|at| at > 0);
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, 2, 6.0);
        ui.add_enabled_ui(!full, |ui| {
            let style = ButtonStyle { active: Some(ACCENT()), ..Default::default() };
            let help = "Adds an empty paint layer above the one you paint on, and paints on it from now on. Undo (Ctrl+Z) \
                        removes it.";
            if tip(theme::button(ui, w, "New Layer", style), tips, help).clicked() {
                actions.push(PaintAction::New);
            }
        });
        ui.add_enabled_ui(active.is_some() && !full, |ui| {
            let help = "Copies the layer you paint on, just above it, and paints on the copy.";
            if tip(theme::button(ui, w, "Duplicate", ButtonStyle::default()), tips, help).clicked() {
                if let Some(id) = active {
                    actions.push(PaintAction::Duplicate(id));
                }
            }
        });
    });
    if rows.is_empty() {
        return;
    }
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, 2, 6.0);
        ui.add_enabled_ui(below, |ui| {
            let help = "Folds the layer you paint on into the one under it, as the two show together. Undo (Ctrl+Z) splits \
                        them again.";
            if tip(theme::button(ui, w, "Merge Down", ButtonStyle::default()), tips, help).clicked() {
                if let Some(id) = active {
                    actions.push(PaintAction::MergeDown(id));
                }
            }
        });
        ui.add_enabled_ui(rows.len() > 1, |ui| {
            let help = "Folds every layer that shows into one, as they show; hidden layers are dropped. Undo (Ctrl+Z) \
                        brings them back.";
            if tip(theme::button(ui, w, "Flatten All", ButtonStyle::default()), tips, help).clicked() {
                actions.push(PaintAction::FlattenAll);
            }
        });
    });
    if full {
        theme::hint(ui, &format!("An object can have {MOST_PAINT_LAYERS} paint layers. Merge the finished ones."));
    }
}

/// The parts of a row's plate: grip, dot, name, eye, lock, bin, slider.
fn parts(plate: Rect) -> [Rect; 7] {
    let inner = plate.shrink(PAD);
    let line = Rect::from_min_size(inner.min, vec2(inner.width(), LINE));
    let grip = Rect::from_min_size(line.min, vec2(GRIP, LINE));
    let dot = Rect::from_min_size(pos2(grip.right(), line.top()), vec2(22.0, LINE));
    let bin = Rect::from_min_max(pos2(line.right() - 24.0, line.top()), line.max);
    let lock = bin.translate(vec2(-26.0, 0.0));
    let eye = lock.translate(vec2(-26.0, 0.0));
    let name = Rect::from_min_max(pos2(dot.right() + 2.0, line.top()), pos2(eye.left() - 6.0, line.bottom()));
    let slider = Rect::from_min_max(pos2(inner.left(), line.bottom() + 2.0), inner.max);
    [grip, dot, name, eye, lock, bin, slider]
}

/// One layer's row. Gives back its plate, for dragging rows about.
#[allow(clippy::too_many_arguments)]
fn layer(
    ui: &mut Ui,
    mesh: u64,
    row: &PaintRow,
    active: bool,
    dragging: Option<u32>,
    pictures: &Pictures,
    tips: bool,
    actions: &mut Vec<PaintAction>,
) -> Rect {
    let height = PAD * 2.0 + LINE + 2.0 + theme::SLIDER_HEIGHT;
    let (plate, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    if !ui.is_rect_visible(plate) {
        return plate;
    }
    let [grip, dot, name, eye, lock, bin, slider] = parts(plate);
    let edge = if active { ACTIVE.gamma_multiply(0.8) } else { theme::CONTROL() };
    let fill = if dragging == Some(row.id) { theme::lit(theme::FIELD()) } else { theme::FIELD() };
    ui.painter().rect_filled(plate, 6.0, fill);
    ui.painter().rect_stroke(plate, 6.0, Stroke::new(1.0, edge), StrokeKind::Inside);

    grip_handle(ui, grip, row, tips);
    active_dot(ui, dot, row, active, tips, actions);
    layer_name(ui, name, mesh, row, tips, actions);

    let response = ui.interact(eye, Id::new(("paint_eye", row.id)), Sense::click());
    let hovered = response.hovered();
    if tip(response, tips, "Shows this layer's paint, or leaves it out. Its paint is kept.").clicked() {
        actions.push(PaintAction::Row(PaintRow { visible: !row.visible, ..row.clone() }));
    }
    icon(ui, eye, pictures, if row.visible { "eye" } else { "eye_off" }, if row.visible { 235 } else { 90 }, hovered);

    let response = ui.interact(lock, Id::new(("paint_lock", row.id)), Sense::click());
    let hovered = response.hovered();
    let help = "Locks this layer: no stroke, fill or clear changes it until it is unlocked.";
    if tip(response, tips, help).clicked() {
        actions.push(PaintAction::Row(PaintRow { locked: !row.locked, ..row.clone() }));
    }
    padlock(ui, lock, row.locked, hovered);

    let response = ui.interact(bin, Id::new(("paint_bin", row.id)), Sense::click());
    let hovered = response.hovered();
    if tip(response, tips, "Deletes this layer and its paint. Undo (Ctrl+Z) brings it back.").clicked() {
        actions.push(PaintAction::Delete(row.id));
    }
    ui.painter().rect_filled(bin, 5.0, theme::CONTROL());
    icon(ui, bin, pictures, "trash", 210, hovered);

    opacity(ui, slider, row, tips, actions);
    plate
}

fn icon(ui: &Ui, rect: Rect, pictures: &Pictures, name: &str, alpha: u8, hovered: bool) {
    if hovered {
        ui.painter().rect_filled(rect, 5.0, Color32::from_white_alpha(20));
    }
    if let Some(icon) = pictures.icon(name) {
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        ui.painter().image(icon, Rect::from_center_size(rect.center(), Vec2::splat(15.0)), uv, Color32::from_white_alpha(alpha));
    }
}

/// A padlock drawn in lines: shut and lit when locked, open and dim when not.
fn padlock(ui: &Ui, rect: Rect, locked: bool, hovered: bool) {
    let painter = ui.painter();
    if hovered {
        painter.rect_filled(rect, 5.0, Color32::from_white_alpha(20));
    }
    let ink = if locked { Color32::from_rgb(240, 200, 120) } else { Color32::from_white_alpha(90) };
    let c = rect.center();
    let body = Rect::from_center_size(c + vec2(0.0, 3.0), vec2(11.0, 8.0));
    painter.rect_filled(body, 1.5, ink);
    let lift = if locked { 0.0 } else { -3.0 };
    let (left, right) = (body.left() + 2.5, body.right() - 2.5);
    let top = body.top() - 5.0 + lift;
    let stroke = Stroke::new(1.6, ink);
    painter.line_segment([pos2(left, body.top()), pos2(left, top + 2.0)], stroke);
    painter.line_segment([pos2(right, body.top() + if locked { 0.0 } else { -2.0 }), pos2(right, top + 2.0)], stroke);
    painter.line_segment([pos2(left, top + 2.0), pos2(left + 1.5, top)], stroke);
    painter.line_segment([pos2(right, top + 2.0), pos2(right - 1.5, top)], stroke);
    painter.line_segment([pos2(left + 1.5, top), pos2(right - 1.5, top)], stroke);
}

/// The grip a row is dragged up or down the stack by.
fn grip_handle(ui: &mut Ui, rect: Rect, row: &PaintRow, tips: bool) {
    let response = ui.interact(rect, Id::new(("paint_grip", row.id)), Sense::drag());
    let ink = if response.hovered() || response.dragged() { theme::TEXT() } else { theme::HINT() };
    for dy in [-4.0, 0.0, 4.0] {
        for dx in [-2.0, 2.0] {
            ui.painter().circle_filled(rect.center() + vec2(dx, dy), 1.2, ink);
        }
    }
    if response.drag_started() {
        ui.data_mut(|d| d.insert_temp(drag_id(), Dragging { id: row.id }));
    }
    tip(response, tips, "Drag up or down to move this layer in the stack: the top layer paints over the rest.");
}

/// While a row is being dragged: the line where it would land, and when it
/// is let go, the move.
fn drop_target(ui: &mut Ui, plates: &[(u32, Rect)], count: usize, dragging: Option<Dragging>, actions: &mut Vec<PaintAction>) {
    let Some(drag) = dragging else { return };
    let Some(y) = ui.ctx().pointer_latest_pos().map(|p| p.y) else { return };
    let above = plates.iter().filter(|(_, r)| r.center().y < y).count();
    let line_y = match plates.get(above) {
        Some((_, r)) => r.top() - 2.0,
        None => plates.last().map_or(y, |(_, r)| r.bottom() + 2.0),
    };
    if let Some((_, first)) = plates.first() {
        let stroke = Stroke::new(2.0, ACTIVE);
        ui.painter().line_segment([pos2(first.left(), line_y), pos2(first.right(), line_y)], stroke);
    }
    let released = ui.input(|i| !i.pointer.any_down());
    if released {
        ui.data_mut(|d| d.remove::<Dragging>(drag_id()));
        let from_top = plates.iter().position(|(id, _)| *id == drag.id).unwrap_or(0);
        let to_top = if above > from_top { above - 1 } else { above };
        if to_top != from_top {
            let to_bottom = count - 1 - to_top.min(count - 1);
            actions.push(PaintAction::Move(drag.id, to_bottom));
        }
    }
}

/// The dot: lit on the layer strokes go to. A click paints on this one.
fn active_dot(ui: &mut Ui, rect: Rect, row: &PaintRow, active: bool, tips: bool, actions: &mut Vec<PaintAction>) {
    let response = ui.interact(rect, Id::new(("paint_dot", row.id)), Sense::click());
    let centre = pos2(rect.left() + 10.0, rect.center().y);
    let painter = ui.painter();
    if active {
        painter.circle_filled(centre, 6.5, ACTIVE);
        painter.circle_stroke(centre, 9.0, Stroke::new(1.0, ACTIVE.gamma_multiply(0.5)));
    } else {
        let ring = if response.hovered() { ACTIVE.gamma_multiply(0.8) } else { theme::HINT() };
        painter.circle_stroke(centre, 6.5, Stroke::new(1.5, ring));
    }
    let help = if active { "The Paint brush paints on this layer." } else { "Click to paint on this layer." };
    if tip(response, tips, help).clicked() && !active {
        actions.push(PaintAction::Select(row.id));
    }
}

fn layer_name(ui: &mut Ui, rect: Rect, mesh: u64, row: &PaintRow, tips: bool, actions: &mut Vec<PaintAction>) {
    let mut rename: Option<Rename> = ui.data(|data| data.get_temp(rename_id()));
    if let Some(edit) = rename.as_mut().filter(|edit| (edit.mesh, edit.id) == (mesh, row.id)) {
        let id = Id::new(("paint_rename_field", row.id));
        let field = egui::TextEdit::singleline(&mut edit.text).id(id).font(theme::font(12.0)).desired_width(rect.width());
        let response = ui.put(rect, field);
        let mut done = false;
        if edit.fresh {
            edit.fresh = false;
            response.request_focus();
            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), id) {
                let end = egui::text::CCursor::new(edit.text.chars().count());
                state.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), end)));
                state.store(ui.ctx(), id);
            }
        } else if response.lost_focus() {
            let text = edit.text.trim();
            if !ui.input(|i| i.key_pressed(Key::Escape)) && !text.is_empty() {
                actions.push(PaintAction::Rename(row.id, text.to_owned()));
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
    let response = ui.interact(rect, Id::new(("paint_name", row.id)), Sense::click());
    let ink = if row.visible { theme::TEXT() } else { theme::TEXT().gamma_multiply(0.55) };
    let clip = rect.intersect(ui.painter().clip_rect());
    let mut label = row.name.clone();
    if row.colour_blend != ColourBlend::Normal {
        label = format!("{label}  ({})", row.colour_blend.name());
    }
    ui.painter().with_clip_rect(clip).text(rect.left_center(), Align2::LEFT_CENTER, &label, theme::font(12.0), ink);
    if response.double_clicked() {
        let edit = Rename { mesh, id: row.id, text: row.name.clone(), fresh: true };
        ui.data_mut(|data| data.insert_temp(rename_id(), edit));
    } else if response.clicked() {
        actions.push(PaintAction::Select(row.id));
    }
    tip(response, tips, "Click to paint on this layer; double-click to rename it.");
}

/// The Opacity slider: one undo step per drag.
fn opacity(ui: &mut Ui, rect: Rect, row: &PaintRow, tips: bool, actions: &mut Vec<PaintAction>) {
    let response = ui.interact(rect, Id::new(("paint_opacity", row.id)), Sense::click_and_drag());
    let down = response.is_pointer_button_down_on();
    let mut percent = (row.opacity as f32 / 2.55).round();
    let response = theme::slider_in(ui, rect, response, "Opacity", &mut percent, 0.0..=100.0, |v| format!("{v:.0}%"));
    let was_down_id = Id::new(("paint_opacity_down", row.id));
    let was_down: bool = ui.data(|data| data.get_temp(was_down_id)).unwrap_or(false);
    if down {
        let byte = (percent.clamp(0.0, 100.0) * 2.55).round() as u8;
        actions.push(PaintAction::Opacity(row.id, byte));
    } else if was_down {
        actions.push(PaintAction::OpacityDone);
    }
    ui.data_mut(|data| data.insert_temp(was_down_id, down));
    tip(response, tips, "How much of this layer shows: 100% as painted, 0% none of it. One drag is one undo step.");
}

/// The settings of the layer being painted on: its colour blend and its mask.
fn active_settings(ui: &mut Ui, row: &PaintRow, picked: Option<u16>, tips: bool, actions: &mut Vec<PaintAction>) {
    theme::heading(ui, &format!("{} Settings", shortened(&row.name, 18)));
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Blend").font(theme::font(12.0)).color(theme::TEXT()));
        egui::ComboBox::from_id_salt(("paint_blend", row.id))
            .selected_text(row.colour_blend.name())
            .width(ui.available_width() - 8.0)
            .show_ui(ui, |ui| {
                for blend in ColourBlend::ALL {
                    if ui.selectable_label(blend == row.colour_blend, blend.name()).clicked() && blend != row.colour_blend {
                        actions.push(PaintAction::Row(PaintRow { colour_blend: blend, ..row.clone() }));
                    }
                }
            });
    });
    mask_source(ui, row, picked, tips, actions);
}

/// The layer's mask: where on the surface the layer shows.
fn mask_source(ui: &mut Ui, row: &PaintRow, picked: Option<u16>, tips: bool, actions: &mut Vec<PaintAction>) {
    let choices: [(&str, MaskSource); 5] = [
        ("Everywhere", MaskSource::None),
        ("Paintable", MaskSource::Paintable),
        ("Sculpt Mask", MaskSource::SculptMask { invert: false }),
        ("Not Masked", MaskSource::SculptMask { invert: true }),
        ("Cavity", MaskSource::Cavity),
    ];
    let current = match row.mask {
        MaskSource::FaceGroup(g) => format!("Face Group {g}"),
        m => choices.iter().find(|(_, c)| *c == m).map_or("Everywhere", |(n, _)| n).to_owned(),
    };
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Shows").font(theme::font(12.0)).color(theme::TEXT()));
        egui::ComboBox::from_id_salt(("paint_mask", row.id)).selected_text(current).width(ui.available_width() - 8.0).show_ui(
            ui,
            |ui| {
                for (name, mask) in choices {
                    if ui.selectable_label(row.mask == mask, name).clicked() && row.mask != mask {
                        actions.push(PaintAction::Row(PaintRow { mask, ..row.clone() }));
                    }
                }
                let on_group = matches!(row.mask, MaskSource::FaceGroup(_));
                if ui.selectable_label(on_group, "Face Group (the one picked)").clicked() {
                    actions.push(PaintAction::MaskByPickedGroup(row.id));
                }
            },
        );
    });
    match row.mask {
        MaskSource::None => {}
        MaskSource::Paintable => theme::hint(
            ui,
            "The layer shows where its own mask is open. Tick Paint Layer Mask in the Paint brush's settings to paint \
             it: Paint hides the layer, Ctrl brings it back.",
        ),
        MaskSource::SculptMask { invert } => theme::hint(
            ui,
            if invert { "The layer shows where the surface is not masked." } else { "The layer shows where the surface is masked (M)." },
        ),
        MaskSource::Cavity => {
            theme::hint(ui, "The layer shows in the recesses, as the surface was when the snapshot was taken.");
            let help = "Takes the recesses again from the surface as it is now.";
            if tip(theme::button(ui, ui.available_width(), "Recompute Cavity", ButtonStyle::default()), tips, help).clicked() {
                actions.push(PaintAction::RecomputeCavity(row.id));
            }
        }
        MaskSource::FaceGroup(g) => {
            theme::hint(ui, &format!("The layer shows on face group {g} only."));
            if picked.is_some_and(|p| p != g) {
                let help = "Shows the layer on the face group picked on the model (Ctrl + Shift + click) instead.";
                if tip(theme::button(ui, ui.available_width(), "Use Picked Group", ButtonStyle::default()), tips, help).clicked() {
                    actions.push(PaintAction::MaskByPickedGroup(row.id));
                }
            }
        }
    }
}

/// Fill and Clear.
fn fill(ui: &mut Ui, row: &PaintRow, paint: &PaintSettings, tips: bool, actions: &mut Vec<PaintAction>) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Fill with").font(theme::font(12.0)).color(theme::TEXT()));
        let (rect, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
        ui.painter().rect_filled(rect, 3.0, colour32(paint.colour));
        ui.label(egui::RichText::new("the Paint brush's colour").font(theme::font(11.0)).color(theme::HINT()));
    });
    let locked = row.locked;
    ui.add_enabled_ui(!locked, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let w = theme::row_width(ui, 3, 4.0);
            let fills = [
                ("Fill All", FillArea::All, "Paints the whole layer with the Paint brush's colour."),
                ("Fill Masked", FillArea::SculptMask { invert: false }, "Paints the layer as far as the surface is masked."),
                ("Fill Unmasked", FillArea::SculptMask { invert: true }, "Paints the layer where the surface is not masked."),
            ];
            for (label, area, help) in fills {
                if tip(theme::button(ui, w, label, ButtonStyle { height: 24.0, ..Default::default() }), tips, help).clicked() {
                    actions.push(PaintAction::Fill(area));
                }
            }
        });
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, 2, 6.0);
            let help = "Paints the face group picked on the model (Ctrl + Shift + click it, or Pick on the Face Groups card).";
            if tip(theme::button(ui, w, "Fill Picked Group", ButtonStyle::default()), tips, help).clicked() {
                actions.push(PaintAction::FillPickedGroup);
            }
            let help = "Empties this layer of its paint. Undo (Ctrl+Z) brings it back.";
            if tip(theme::button(ui, w, "Clear Layer", ButtonStyle::default()), tips, help).clicked() {
                actions.push(PaintAction::Clear(row.id));
            }
        });
    });
    if locked {
        theme::hint(ui, "This layer is locked: unlock it (the padlock) to paint, fill or clear it.");
    }
}

fn cost(ui: &mut Ui, mesh: &Mesh, count: usize) {
    let bytes = mesh.paint.as_ref().map_or(0, |p| p.bytes());
    let layers = if count == 1 { "this layer".to_owned() } else { format!("these {count} layers") };
    theme::hint(
        ui,
        &format!("Paint takes 4 bytes for each vertex a layer's colour covers: {} for {layers}.", super::layers::megabytes(bytes)),
    );
}

fn shortened(name: &str, most: usize) -> String {
    if name.chars().count() <= most { name.to_owned() } else { format!("{}...", name.chars().take(most - 3).collect::<String>()) }
}

// --------------------------------------------------------- the brush settings

/// The Paint brush's own settings, in the Brush Settings card (the
/// desktop's `ui::paint::brush_settings`, colour only).
pub(super) fn brush_settings(ui: &mut Ui, paint: &mut PaintSettings, mesh: &Mesh, tips: bool) {
    let modes: Vec<&str> = PaintMode::ALL.iter().map(|m| m.name()).collect();
    let at = PaintMode::ALL.iter().position(|m| *m == paint.mode).unwrap_or(0);
    if let Some(k) = choice(ui, &modes, at) {
        paint.mode = PaintMode::ALL[k];
    }
    theme::hint(ui, paint.mode.about());

    // The colour: a well that opens the picker, its sRGB numbers, the swatches.
    ui.horizontal(|ui| {
        let before = paint.colour;
        ui.color_edit_button_srgb(&mut paint.colour);
        let [r, g, b] = paint.colour;
        ui.label(egui::RichText::new(format!("Colour  #{r:02X}{g:02X}{b:02X}")).font(theme::font(12.0)).color(theme::TEXT()));
        if before != paint.colour {
            ui.ctx().request_repaint();
        }
    });
    swatches(ui, paint, tips);
    tip(
        theme::slider(ui, "Opacity", &mut paint.opacity, 0.01..=1.0, |v| format!("{:.0}%", v * 100.0)),
        tips,
        "The most one stroke lays: going over a place again in the same stroke never goes past it. Flow (above) is \
         how fast each dab gets there.",
    );
    layer_line(ui, paint, mesh, tips);

    // Zadd: colour and sculpt in one stroke.
    let help = "Colour and sculpt: each dab also runs the brush below, with this ring, falloff and alpha and that \
                brush's own Strength, so the paint stands up from the surface. Ctrl paints out and digs in.";
    tip(theme::checkbox(ui, &mut paint.zadd, "Zadd (Colour + Sculpt)", ACCENT()), tips, help);
    if paint.zadd {
        let names: Vec<&str> = ZADD_BRUSHES.iter().map(|b| b.name()).collect();
        let at = ZADD_BRUSHES.iter().position(|b| *b == paint.zadd_brush).unwrap_or(0);
        if let Some(k) = choice(ui, &names, at) {
            paint.zadd_brush = ZADD_BRUSHES[k];
        }
    }
    theme::about(ui, "Ctrl erases, Shift blurs the paint, I picks up the colour under the pointer, P puts the brush down.");
}

/// The colours painted with lately: a click takes one up again.
fn swatches(ui: &mut Ui, paint: &mut PaintSettings, tips: bool) {
    if paint.recent.is_empty() {
        return;
    }
    let size = 18.0;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
        for k in 0..paint.recent.len() {
            let c = paint.recent[k];
            let (rect, response) = ui.allocate_exact_size(vec2(size, size), Sense::click());
            ui.painter().rect_filled(rect, 3.0, colour32(c));
            let edge = if c == paint.colour { theme::TEXT() } else { Color32::from_black_alpha(120) };
            ui.painter().rect_stroke(rect, 3.0, Stroke::new(1.0, edge), StrokeKind::Inside);
            if tip(response, tips, "A colour painted with lately: click to paint with it again.").clicked() {
                paint.colour = c;
            }
        }
    });
}

/// The layer the brush paints on, and Paint Layer Mask where it can be used.
fn layer_line(ui: &mut Ui, paint: &mut PaintSettings, mesh: &Mesh, tips: bool) {
    let active = mesh.paint_active();
    let row = mesh.paint_rows().into_iter().find(|r| Some(r.id) == active);
    match &row {
        Some(row) if row.locked => theme::hint(ui, &format!("Painting on \"{}\", which is locked.", row.name)),
        Some(row) => theme::hint(ui, &format!("Painting on \"{}\" (Paint Layers card, Mesh tab).", row.name)),
        None => theme::hint(ui, "No paint layer yet: the first stroke makes one."),
    }
    let paintable = row.is_some_and(|r| r.mask == MaskSource::Paintable);
    if paintable || paint.layer_mask {
        let help = "Paint the layer's own mask instead of its paint: Paint hides the layer under the brush, Ctrl brings \
                    it back. Its mask must be Paintable.";
        tip(theme::checkbox(ui, &mut paint.layer_mask, "Paint Layer Mask", ACCENT()), tips, help);
    }
}

// ------------------------------------------------------------ masking by paint

impl GolemApp {
    /// The Masking card's paint commands, for an object with paint (the
    /// desktop's `ui::paint::masking`, less By Material).
    pub(super) fn paint_masking(&mut self, ui: &mut Ui) {
        if !self.mesh.has_paint() {
            return;
        }
        let tips = self.ui.tips;
        theme::heading(ui, "Mask by Paint");
        let paint = &mut self.sculptor.settings.paint;
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
            ui.painter().rect_filled(rect, 3.0, colour32(paint.colour));
            ui.label(egui::RichText::new("The Paint brush's colour").font(theme::font(11.0)).color(theme::HINT()));
        });
        tip(
            theme::slider(ui, "Colour Tolerance", &mut paint.tolerance, 0.0..=1.0, percent),
            tips,
            "How far from the colour still counts for Mask by Colour: 0% only that colour, more takes in its neighbours.",
        );
        let mut pressed = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let w = theme::row_width(ui, 2, 4.0);
            let buttons = [
                ("By Colour", PaintAction::MaskByColour, "Masks where the paint is the Paint brush's colour, within the tolerance."),
                ("By Layer", PaintAction::MaskByLayer, "Masks where the paint layer being painted on has paint."),
            ];
            for (label, action, help) in buttons {
                if tip(theme::button(ui, w, label, ButtonStyle { height: 24.0, ..Default::default() }), tips, help).clicked() {
                    pressed = Some(action);
                }
            }
        });
        if let Some(action) = pressed {
            self.paint_act(action);
        }
    }
}

// -------------------------------------------------------------- the commands

impl GolemApp {
    fn paint_row(&self, id: u32) -> Option<PaintRow> {
        self.mesh.paint_rows().into_iter().find(|r| r.id == id)
    }

    /// Records a change to the paint that the mesh has already made, and says `text`.
    fn paint_changed(&mut self, step: PaintStep, text: String) {
        self.push_undo(Step::Paint(self.active, step));
        self.note_changed();
        self.status = text;
    }

    /// Says why a paint command did nothing. "That changes nothing" is not
    /// worth a line: the press did what was asked.
    fn paint_refused(&mut self, why: PaintRefusal) {
        if why != PaintRefusal::Unchanged {
            self.status = why.to_string();
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

    /// Carries out a command of the Paint Layers card or the Masking card's
    /// paint commands.
    pub(super) fn paint_act(&mut self, action: PaintAction) {
        if self.sculptor.is_active() {
            return;
        }
        if !matches!(action, PaintAction::Opacity(..) | PaintAction::OpacityDone) {
            self.paint_opacity_done();
            self.layer_strength_done();
        }
        match action {
            PaintAction::New => match self.mesh.add_paint_layer(None) {
                Ok((id, step)) => {
                    let name = self.paint_row(id).map_or_else(String::new, |r| quoted(&r));
                    self.paint_changed(step, format!("Added the paint layer {name}: what you paint now goes on it."));
                }
                Err(why) => self.paint_refused(why),
            },
            PaintAction::Delete(id) => {
                let name = self.paint_row(id).map_or_else(String::new, |r| quoted(&r));
                let result = self.mesh.delete_paint_layer(id);
                self.paint_result(result, || format!("Deleted the paint layer {name}. Undo (Ctrl+Z) brings it back."));
            }
            PaintAction::Rename(id, name) => {
                let result = self.mesh.rename_paint_layer(id, &name);
                self.paint_result(result, || format!("The paint layer is now called \"{}\".", name.trim()));
            }
            PaintAction::Select(id) => {
                let name = self.paint_row(id).map_or_else(String::new, |r| quoted(&r));
                let result = self.mesh.set_paint_active(id);
                self.paint_result(result, || format!("Painting on {name}."));
            }
            PaintAction::Move(id, to) => {
                let name = self.paint_row(id).map_or_else(String::new, |r| quoted(&r));
                let result = self.mesh.move_paint_layer(id, to);
                self.paint_result(result, || format!("Moved {name}."));
            }
            PaintAction::Duplicate(id) => match self.mesh.duplicate_paint_layer(id) {
                Ok((new, step)) => {
                    let name = self.paint_row(new).map_or_else(String::new, |r| quoted(&r));
                    self.paint_changed(step, format!("Made {name}, a copy, and painting goes on it."));
                }
                Err(why) => self.paint_refused(why),
            },
            PaintAction::MergeDown(id) => {
                let name = self.paint_row(id).map_or_else(String::new, |r| quoted(&r));
                let result = self.mesh.merge_paint_down(id);
                self.paint_result(result, || {
                    format!("Merged {name} into the layer under it: the paint looks the same. Undo (Ctrl+Z) splits them again.")
                });
            }
            PaintAction::FlattenAll => {
                let result = self.mesh.flatten_paint();
                self.paint_result(result, || {
                    "Flattened the paint layers into one, as they showed (hidden ones are dropped). Undo (Ctrl+Z) brings \
                     them back."
                        .into()
                });
            }
            PaintAction::Row(row) => self.paint_set_row(row),
            PaintAction::Opacity(id, opacity) => self.paint_opacity(id, opacity),
            PaintAction::OpacityDone => self.paint_opacity_done(),
            PaintAction::Fill(area) => self.paint_fill(area),
            PaintAction::FillPickedGroup => {
                if let Some(group) = self.group_picked_or_say() {
                    self.paint_fill(FillArea::Group(group));
                }
            }
            PaintAction::Clear(id) => {
                let name = self.paint_row(id).map_or_else(String::new, |r| quoted(&r));
                let result = self.mesh.clear_paint(id, ChannelSet::SURFACE);
                self.paint_result(result, || format!("Cleared {name}: it is empty now. Undo (Ctrl+Z) brings its paint back."));
            }
            PaintAction::RecomputeCavity(id) => {
                let result = self.mesh.snapshot_paint_cavity(id);
                self.paint_result(result, || "The layer's cavity mask follows the surface as it is now.".into());
            }
            PaintAction::MaskByPickedGroup(id) => {
                if let (Some(group), Some(row)) = (self.group_picked_or_say(), self.paint_row(id)) {
                    self.paint_set_row(PaintRow { mask: MaskSource::FaceGroup(group), ..row });
                }
            }
            PaintAction::MaskByColour => self.paint_mask(false),
            PaintAction::MaskByLayer => self.paint_mask(true),
        }
    }

    /// A row's settings changed on the card. One step.
    fn paint_set_row(&mut self, row: PaintRow) {
        let Some(was) = self.paint_row(row.id) else { return };
        let text = row_words(&was, &row);
        let result = self.mesh.set_paint_row(row);
        self.paint_result(result, || text);
    }

    /// The Opacity slider of layer `id` is held at `opacity`.
    fn paint_opacity(&mut self, id: u32, opacity: u8) {
        let uid = self.mesh.uid;
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
            if row.opacity == opacity {
                return;
            }
            HELD.with(|held| *held.borrow_mut() = Some(Held { uid, was: row.clone(), opacity: row.opacity, fresh: true }));
        }
        self.mesh.preview_paint_row(PaintRow { opacity, ..row });
        HELD.with(|held| {
            if let Some(held) = held.borrow_mut().as_mut() {
                held.opacity = opacity;
            }
        });
    }

    /// The Opacity slider was let go: the drag becomes one undo step.
    pub(super) fn paint_opacity_done(&mut self) {
        let Some(held) = HELD.with(|held| held.borrow_mut().take()) else { return };
        if held.uid != self.mesh.uid {
            return;
        }
        let Some(now) = self.paint_row(held.was.id) else { return };
        self.mesh.preview_paint_row(held.was.clone());
        let percent = (now.opacity as f32 / 2.55).round();
        let name = quoted(&now);
        let result = self.mesh.set_paint_row(now);
        self.paint_result(result, || format!("{name} is at {percent}% opacity."));
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

    /// Fill: the active layer painted all over `area` with the brush's colour.
    fn paint_fill(&mut self, area: FillArea) {
        let settings = self.sculptor.settings.paint.clone();
        let colour_now = settings.colour;
        let texel = PaintSettings { slot: None, ..settings }.texel();
        let target = PaintTarget { texel, channels: ChannelSet::COLOUR, mode: Toward::Paint };
        let result = self.mesh.fill_paint(&target, area);
        if result.is_ok() {
            self.sculptor.settings.paint.remember(colour_now);
        }
        self.paint_result(result, || format!("Filled {} with the colour. Undo (Ctrl+Z) takes it back.", area_words(area)));
    }

    /// The eyedropper at pixel `at`: the colour of the paint there becomes
    /// the one to paint with.
    fn paint_pick(&mut self, at: GVec2) -> Result<String, String> {
        let on_active = self.pick(at).is_some_and(|(id, _)| id == self.active);
        let Some(hit) = self.hit(at).filter(|_| on_active) else {
            return Err("The eyedropper picks paint from the selected object, and it is not under the pointer.".into());
        };
        let m = self.active_object().placed_matrix(hit.side);
        let view = if m == glam::Mat4::IDENTITY { self.view } else { self.view.local_to(m) };
        let (origin, dir) = view.ray(at);
        let picked = crate::sculpt::paint::pick_along(&self.mesh, origin, dir);
        let Some(picked) = picked.filter(|p| p.coverage > 0.5 / 255.0) else {
            return Err("There is no paint there to pick up.".into());
        };
        let paint = &mut self.sculptor.settings.paint;
        paint.colour = picked.colour;
        paint.remember(picked.colour);
        let [r, g, b] = picked.colour;
        Ok(format!("Picked up the colour (red {r}, green {g}, blue {b}): Paint lays it now."))
    }

    /// I: the eyedropper where the pointer is.
    pub(super) fn paint_eyedropper(&mut self, at: GVec2) {
        self.status = match self.paint_pick(at) {
            Ok(text) | Err(text) => text,
        };
    }

    /// A press on the model with the Paint brush in Sample mode: the
    /// eyedropper, and no stroke. Gives back whether the press was that.
    pub(super) fn paint_samples_press(&mut self, at: GVec2, shift: bool) -> bool {
        let s = &self.sculptor.settings;
        if s.mask_mode || s.brush != Brush::Paint || s.paint.mode != PaintMode::Sample || shift {
            return false;
        }
        self.paint_eyedropper(at);
        true
    }

    /// Whether a stroke about to begin must be refused because it would
    /// paint and has nowhere to; says why if so, and says what was made for
    /// it if something was (the desktop's `Doc::paint_prepare`).
    pub(super) fn paint_refuses_stroke(&mut self, smooth_held: bool) -> bool {
        let s = &mut self.sculptor.settings;
        if s.mask_mode || s.brush != Brush::Paint {
            return false;
        }
        s.paint.slot = None;
        let mode = if smooth_held { PaintMode::Blur } else { s.paint.mode };
        if mode == PaintMode::Sample {
            return false;
        }
        if mode == PaintMode::Paint && s.paint.rgb && !s.paint.layer_mask {
            let colour = s.paint.colour;
            s.paint.remember(colour);
        }
        let layer_mask = s.paint.layer_mask;
        match self.mesh.paint_writable() {
            Ok(_) => {}
            Err(PaintRefusal::NoLayer) if matches!(mode, PaintMode::Paint) && !layer_mask => match self.mesh.add_paint_layer(None) {
                Ok((id, step)) => {
                    self.push_undo(Step::Paint(self.active, step));
                    let name = self.paint_row(id).map(|r| r.name).unwrap_or_default();
                    self.status = format!(
                        "Made the paint layer \"{name}\" to paint on (Paint Layers card, on the Mesh tab). Undo (Ctrl+Z) takes \
                         the stroke back, and then the layer."
                    );
                }
                Err(why) => {
                    self.status = why.to_string();
                    return true;
                }
            },
            Err(PaintRefusal::NoLayer) => {
                self.status = "There is no paint on this object to erase, blur or smear yet: paint some first.".into();
                return true;
            }
            Err(why) => {
                self.status = why.to_string();
                return true;
            }
        }
        if layer_mask {
            let active = self.mesh.paint_active();
            let row = self.mesh.paint_rows().into_iter().find(|r| Some(r.id) == active);
            if row.is_some_and(|r| r.mask != MaskSource::Paintable) {
                self.status = "Paint Layer Mask paints the layer's own mask, and this layer's mask is not Paintable. Set its \
                               Mask to Paintable on the Paint Layers card, or untick Paint Layer Mask."
                    .into();
                return true;
            }
        }
        false
    }

    /// The paint keys, as the desktop has them by default: P takes the Paint
    /// brush up (again, puts it down), I is the eyedropper where the pointer is.
    pub(super) fn paint_keys(&mut self, ctx: &egui::Context) {
        if self.mode != super::Mode::Sculpt || self.drag != super::Drag::None || self.sculptor.is_active() {
            return;
        }
        let (p, i) = ctx.input_mut(|i| (i.consume_key(egui::Modifiers::NONE, Key::P), i.consume_key(egui::Modifiers::NONE, Key::I)));
        if p {
            self.paint_key();
        }
        if let (true, Some(at)) = (i, self.pointer_px) {
            self.paint_eyedropper(at);
        }
    }

    /// P: the Paint brush in hand, or again, the brush that was in hand before it.
    pub(super) fn paint_key(&mut self) {
        let (library, s) = (&mut self.sculptor.library, &mut self.sculptor.settings);
        if s.brush == Brush::Paint && !s.mask_mode {
            let back = s.paint.came_from.take().and_then(|name| library.named(&name));
            let back = back.unwrap_or(Library::base(Brush::Clay));
            library.select(back, s);
            self.status = format!("{} brush.", library.entries()[back].name);
        } else {
            s.paint.came_from = Some(library.entries()[library.selected(s)].name.clone());
            library.select(Library::base(Brush::Paint), s);
            s.mask_mode = false;
            self.status = "Paint brush.".into();
        }
    }

    /// The Masking card's paint commands: a mask made from the paint (by its
    /// colour, or where the active layer has paint), as one mask step.
    fn paint_mask(&mut self, by_layer: bool) {
        let mesh = &self.mesh;
        if !mesh.has_paint() {
            self.status = "This object has no paint to mask by.".into();
            return;
        }
        let settings = &self.sculptor.settings.paint;
        let (colour, tolerance) = (settings.colour, settings.tolerance);
        let values: Vec<f32> = if by_layer {
            match layer_alpha(mesh) {
                Some(values) => values,
                None => {
                    self.status = "There is no active paint layer to mask by.".into();
                    return;
                }
            }
        } else {
            let streams = crate::mesh::paint::export_streams(mesh).unwrap_or_default();
            streams.iter().map(|r| colour_match([r[0], r[1], r[2]], r[3], colour, tolerance)).collect()
        };
        if values.len() != mesh.vertex_count() {
            return;
        }
        let delta = self.mesh.remap_mask(|_, v, _| values[v as usize]);
        let masked = values.iter().filter(|&&m| m > 0.5).count();
        let text = if by_layer { "where the active paint layer has paint" } else { "where the paint is near the Paint card's colour" };
        match delta {
            Some(delta) => {
                self.stroke_done(delta);
                self.status = if masked == 0 {
                    format!("Nothing is masked: there is nowhere {text}.")
                } else {
                    format!("Masked {text}. Masked surface is safe from brushes; Invert to work only there.")
                };
            }
            None => self.status = format!("The mask is already {text}."),
        }
    }
}

/// How far painted colour `rgb` (with coverage `cover`) is inside the Mask
/// by Colour of `colour` at `tolerance` (the desktop's `colour_match`).
fn colour_match(rgb: [u8; 3], cover: u8, colour: [u8; 3], tolerance: f32) -> f32 {
    const SOFT: f32 = 0.05;
    let d2: f32 = (0..3).map(|k| ((rgb[k] as f32 - colour[k] as f32) / 255.0).powi(2)).sum();
    let distance = (d2 / 3.0).sqrt();
    let inside = ((tolerance + SOFT - distance) / SOFT).clamp(0.0, 1.0);
    inside * cover as f32 / 255.0
}

/// How much paint the active layer holds at each vertex.
fn layer_alpha(mesh: &Mesh) -> Option<Vec<f32>> {
    let stack = mesh.paint_stack()?;
    let layer = stack.layer(stack.active?)?;
    let data = &layer.data;
    Some(
        (0..mesh.vertex_count())
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
    if was.mask != now.mask {
        return format!("{name} shows {}.", mask_words(now.mask));
    }
    format!("{name} changed.")
}

fn mask_words(mask: MaskSource) -> String {
    match mask {
        MaskSource::None => "everywhere".into(),
        MaskSource::Paintable => "where its own mask is open (paint it with Paint Layer Mask)".into(),
        MaskSource::SculptMask { invert: false } => "where the surface is masked".into(),
        MaskSource::SculptMask { invert: true } => "where the surface is not masked".into(),
        MaskSource::Cavity => "in the recesses of the surface".into(),
        MaskSource::FaceGroup(g) => format!("on face group {g}"),
    }
}

/// The paint's colour for the graphics card: the first four bytes of every
/// vertex's composite (sRGB colour and coverage), for `ranges` of vertices,
/// or all of them. Empty without paint.
pub(crate) fn colour_bytes(mesh: &Mesh, range: std::ops::Range<usize>) -> Vec<u8> {
    let stream = mesh.paint_stream();
    let end = range.end.min(stream.len());
    let start = range.start.min(end);
    stream[start..end].iter().flat_map(|r| [r[0], r[1], r[2], r[3]]).collect()
}
