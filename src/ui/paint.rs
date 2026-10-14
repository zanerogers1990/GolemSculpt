//! Painting's cards: the Paint Layers card of the Sculpt panel's Mesh tab,
//! the Paint brush's own settings in the Brush Settings card, and the paint
//! commands of the Masking card.
//!
//! The Paint Layers card mirrors the Layers card (`layers.rs`). A row is one
//! layer, top of the stack first: a grip to drag it up or down the stack,
//! the dot that marks the layer strokes go to (click to paint on another),
//! its name (double-click to rename), the eye, the lock, the bin, and its
//! Opacity slider. Under the list are the settings of the layer being
//! painted on, which a row has no room for at 75% size: its colour blend,
//! material blend, the channels it shows, and its mask. The card only asks:
//! every press pushes a `PaintAction` the app carries out after the
//! interface pass (`app/paint.rs`), each one undo step; the slider says
//! where it is held every frame it is held and that it was let go, so a
//! drag is one step too.

use egui::{Align2, Color32, Id, Key, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

use super::assets::Assets;
use super::look;
use super::theme::{self, ACCENT, ButtonStyle};
use super::tip;
use crate::doc::{Action, Doc, PaintAction};
use crate::mesh::Mesh;
use crate::mesh::paint::palette::ScenePaint;
use crate::mesh::paint::{
    ChannelSet, ColourBlend, FillArea, MOST_MATERIALS, MOST_PAINT_LAYERS, MaskSource, MaterialBlend, PaintRow,
};
use crate::sculpt::paint::{PaintMode, PaintSettings, ZADD_BRUSHES};

/// The card's colour on the panel.
const TINT: Color32 = Color32::from_rgb(236, 128, 186);
/// The dot of the layer strokes go to.
const ACTIVE: Color32 = Color32::from_rgb(236, 128, 186);

const LINE: f32 = 24.0;
const PAD: f32 = 6.0;
const GRIP: f32 = 14.0;

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

/// A row being dragged up or down the stack: its layer, and where the
/// pointer was over the rows last frame.
#[derive(Clone, Copy)]
struct Dragging {
    id: u32,
}

fn drag_id() -> Id {
    Id::new("paint_layer_drag")
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

fn swatch32(rgb: [f32; 3]) -> Color32 {
    let [r, g, b] = rgb.map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as u8);
    Color32::from_rgb(r, g, b)
}

// ------------------------------------------------------------ the layers card

pub fn card(ui: &mut Ui, doc: &mut Doc, assets: &Assets, actions: &mut Vec<Action>) {
    let tips = doc.opts.tooltips;
    let mesh = &doc.mesh;
    let rows = mesh.paint_rows();
    let active = mesh.paint_active();
    theme::category(ui, "Paint Layers", TINT, &summary(&rows, active), false, |ui| {
        theme::hint(
            ui,
            "Colour and materials painted on the model, in layers like a painting program's. The lit dot marks the \
             layer the Paint brush paints on (P takes the brush up). Drag a row by its grip to move it up or down.",
        );
        commands(ui, &rows, active, tips, actions);
        if rows.is_empty() {
            theme::hint(ui, "This object has no paint yet. The first Paint stroke makes a layer for it.");
            return;
        }
        ui.add_space(4.0);
        let mut plates = Vec::with_capacity(rows.len());
        let dragging: Option<Dragging> = ui.data(|d| d.get_temp(drag_id()));
        // Top of the stack first.
        for row in rows.iter().rev() {
            let plate = layer(ui, mesh.uid, row, active == Some(row.id), dragging.map(|d| d.id), assets, tips, actions);
            plates.push((row.id, plate));
            ui.add_space(4.0);
        }
        drop_target(ui, &plates, rows.len(), dragging, actions);
        if let Some(row) = rows.iter().find(|r| Some(r.id) == active) {
            ui.add_space(2.0);
            active_settings(ui, mesh, row, doc.opts.groups.picked.map(|p| p.group), tips, actions);
            fill(ui, row, &doc.sculptor.settings.paint, &doc.opts.paint, tips, actions);
        }
        cost(ui, mesh, rows.len());
    });
}

/// New, Duplicate, Merge Down, Flatten All.
fn commands(ui: &mut Ui, rows: &[PaintRow], active: Option<u32>, tips: bool, actions: &mut Vec<Action>) {
    let full = rows.len() >= MOST_PAINT_LAYERS;
    let below = active.and_then(|id| rows.iter().position(|r| r.id == id)).is_some_and(|at| at > 0);
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, 2, 6.0);
        ui.add_enabled_ui(!full, |ui| {
            let style = ButtonStyle { active: Some(ACCENT()), ..Default::default() };
            let help = "Adds an empty paint layer above the one you paint on, and paints on it from now on. Undo (Z) \
                        removes it.";
            if tip(theme::button(ui, w, "New Layer", style), tips, help).clicked() {
                actions.push(Action::Paint(PaintAction::New));
            }
        });
        ui.add_enabled_ui(active.is_some() && !full, |ui| {
            let help = "Copies the layer you paint on, just above it, and paints on the copy.";
            if tip(theme::button(ui, w, "Duplicate", ButtonStyle::default()), tips, help).clicked() {
                if let Some(id) = active {
                    actions.push(Action::Paint(PaintAction::Duplicate(id)));
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
            let help = "Folds the layer you paint on into the one under it, as the two show together. Undo (Z) splits \
                        them again.";
            if tip(theme::button(ui, w, "Merge Down", ButtonStyle::default()), tips, help).clicked() {
                if let Some(id) = active {
                    actions.push(Action::Paint(PaintAction::MergeDown(id)));
                }
            }
        });
        ui.add_enabled_ui(rows.len() > 1, |ui| {
            let help = "Folds every layer that shows into one, as they show; hidden layers are dropped. Undo (Z) \
                        brings them back.";
            if tip(theme::button(ui, w, "Flatten All", ButtonStyle::default()), tips, help).clicked() {
                actions.push(Action::Paint(PaintAction::FlattenAll));
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
    assets: &Assets,
    tips: bool,
    actions: &mut Vec<Action>,
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
        actions.push(Action::Paint(PaintAction::Row(PaintRow { visible: !row.visible, ..row.clone() })));
    }
    icon(ui, eye, assets, if row.visible { "eye" } else { "eye_off" }, if row.visible { 235 } else { 90 }, hovered);

    let response = ui.interact(lock, Id::new(("paint_lock", row.id)), Sense::click());
    let hovered = response.hovered();
    let help = "Locks this layer: no stroke, fill or clear changes it until it is unlocked.";
    if tip(response, tips, help).clicked() {
        actions.push(Action::Paint(PaintAction::Row(PaintRow { locked: !row.locked, ..row.clone() })));
    }
    padlock(ui, lock, row.locked, hovered);

    let response = ui.interact(bin, Id::new(("paint_bin", row.id)), Sense::click());
    let hovered = response.hovered();
    if tip(response, tips, "Deletes this layer and its paint. Undo (Z) brings it back.").clicked() {
        actions.push(Action::Paint(PaintAction::Delete(row.id)));
    }
    ui.painter().rect_filled(bin, 5.0, theme::CONTROL());
    icon(ui, bin, assets, "trash", 210, hovered);

    opacity(ui, slider, row, tips, actions);
    plate
}

fn icon(ui: &Ui, rect: Rect, assets: &Assets, name: &str, alpha: u8, hovered: bool) {
    if hovered {
        ui.painter().rect_filled(rect, 5.0, Color32::from_white_alpha(20));
    }
    if let Some(icon) = assets.icon(name) {
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
fn drop_target(ui: &mut Ui, plates: &[(u32, Rect)], count: usize, dragging: Option<Dragging>, actions: &mut Vec<Action>) {
    let Some(drag) = dragging else { return };
    let Some(y) = ui.ctx().pointer_latest_pos().map(|p| p.y) else { return };
    // How many rows (from the top) lie above where the pointer is.
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
        // Its own slot counts once: dropped below itself it moves one fewer.
        let to_top = if above > from_top { above - 1 } else { above };
        if to_top != from_top {
            let to_bottom = count - 1 - to_top.min(count - 1);
            actions.push(Action::Paint(PaintAction::Move(drag.id, to_bottom)));
        }
    }
}

/// The dot: lit on the layer strokes go to. A click paints on this one.
fn active_dot(ui: &mut Ui, rect: Rect, row: &PaintRow, active: bool, tips: bool, actions: &mut Vec<Action>) {
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
        actions.push(Action::Paint(PaintAction::Select(row.id)));
    }
}

fn layer_name(ui: &mut Ui, rect: Rect, mesh: u64, row: &PaintRow, tips: bool, actions: &mut Vec<Action>) {
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
                actions.push(Action::Paint(PaintAction::Rename(row.id, text.to_owned())));
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
        actions.push(Action::Paint(PaintAction::Select(row.id)));
    }
    tip(response, tips, "Click to paint on this layer; double-click to rename it.");
}

/// The Opacity slider: one undo step per drag.
fn opacity(ui: &mut Ui, rect: Rect, row: &PaintRow, tips: bool, actions: &mut Vec<Action>) {
    let response = ui.interact(rect, Id::new(("paint_opacity", row.id)), Sense::click_and_drag());
    let down = response.is_pointer_button_down_on();
    let mut percent = (row.opacity as f32 / 2.55).round();
    let response = theme::slider_in(ui, rect, response, "Opacity", &mut percent, 0.0..=100.0, |v| format!("{v:.0}%"));
    let was_down_id = Id::new(("paint_opacity_down", row.id));
    let was_down: bool = ui.data(|data| data.get_temp(was_down_id)).unwrap_or(false);
    if down {
        let byte = (percent.clamp(0.0, 100.0) * 2.55).round() as u8;
        actions.push(Action::Paint(PaintAction::Opacity(row.id, byte)));
    } else if was_down {
        actions.push(Action::Paint(PaintAction::OpacityDone));
    }
    ui.data_mut(|data| data.insert_temp(was_down_id, down));
    tip(response, tips, "How much of this layer shows: 100% as painted, 0% none of it. One drag is one undo step.");
}

/// The settings of the layer being painted on.
fn active_settings(ui: &mut Ui, mesh: &Mesh, row: &PaintRow, picked: Option<u16>, tips: bool, actions: &mut Vec<Action>) {
    theme::heading(ui, &format!("{} Settings", shortened(&row.name, 18)));
    let set = |actions: &mut Vec<Action>, row: PaintRow| actions.push(Action::Paint(PaintAction::Row(row)));
    // Colour blend: every mode a painting program has, by the names it has them under.
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Blend").font(theme::font(12.0)).color(theme::TEXT()));
        egui::ComboBox::from_id_salt(("paint_blend", row.id))
            .selected_text(row.colour_blend.name())
            .width(ui.available_width() - 8.0)
            .show_ui(ui, |ui| {
                for blend in ColourBlend::ALL {
                    if ui.selectable_label(blend == row.colour_blend, blend.name()).clicked() && blend != row.colour_blend {
                        set(actions, PaintRow { colour_blend: blend, ..row.clone() });
                    }
                }
            });
    });
    let help = "Normal: its material takes the place of what is under it. Add: it is added to it.";
    let at = usize::from(row.material_blend == MaterialBlend::Add);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Material").font(theme::font(12.0)).color(theme::TEXT()));
        if let Some(k) = look::choice(ui, &["Normal", "Add"], at) {
            let material_blend = if k == 1 { MaterialBlend::Add } else { MaterialBlend::Normal };
            set(actions, PaintRow { material_blend, ..row.clone() });
        }
    });
    if tips {
        theme::hint(ui, help);
    }
    // The channels the layer shows.
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let w = theme::row_width(ui, 4, 4.0);
        for (label, bit, help) in [
            ("Colour", ChannelSet::COLOUR, "Show this layer's colour."),
            ("Material", ChannelSet::MATERIAL, "Show this layer's material."),
            ("Rough", ChannelSet::ROUGHNESS, "Show this layer's roughness."),
            ("Metal", ChannelSet::METALLIC, "Show this layer's metallic."),
        ] {
            let on = row.channels.has(bit);
            let style = ButtonStyle { active: on.then_some(ACCENT()), height: 24.0, ..Default::default() };
            if tip(theme::button(ui, w, label, style), tips, help).clicked() {
                let channels = if on { row.channels.without(bit) } else { row.channels.with(bit) };
                set(actions, PaintRow { channels, ..row.clone() });
            }
        }
    });
    mask_source(ui, mesh, row, picked, tips, actions);
}

/// The layer's mask: where on the surface the layer shows.
fn mask_source(ui: &mut Ui, mesh: &Mesh, row: &PaintRow, picked: Option<u16>, tips: bool, actions: &mut Vec<Action>) {
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
                        actions.push(Action::Paint(PaintAction::Row(PaintRow { mask, ..row.clone() })));
                    }
                }
                let on_group = matches!(row.mask, MaskSource::FaceGroup(_));
                if ui.selectable_label(on_group, "Face Group (the one picked)").clicked() {
                    actions.push(Action::Paint(PaintAction::MaskByPickedGroup(row.id)));
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
                actions.push(Action::Paint(PaintAction::RecomputeCavity(row.id)));
            }
        }
        MaskSource::FaceGroup(g) => {
            theme::hint(ui, &format!("The layer shows on face group {g} only."));
            if picked.is_some_and(|p| p != g) {
                let help = "Shows the layer on the face group picked on the model (Ctrl + Shift + click) instead.";
                if tip(theme::button(ui, ui.available_width(), "Use Picked Group", ButtonStyle::default()), tips, help).clicked() {
                    actions.push(Action::Paint(PaintAction::MaskByPickedGroup(row.id)));
                }
            }
        }
    }
    let _ = mesh;
}

/// Fill and Clear.
fn fill(ui: &mut Ui, row: &PaintRow, paint: &PaintSettings, scene: &ScenePaint, tips: bool, actions: &mut Vec<Action>) {
    let id = Id::new("paint_fill_with");
    let (mut colour, mut material): (bool, bool) = ui.data(|d| d.get_temp(id)).unwrap_or((true, true));
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Fill with").font(theme::font(12.0)).color(theme::TEXT()));
        let (r, g, b) = (paint.colour[0], paint.colour[1], paint.colour[2]);
        let (rect, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
        ui.painter().rect_filled(rect, 3.0, Color32::from_rgb(r, g, b));
        theme::checkbox(ui, &mut colour, "Colour", ACCENT());
        let named = scene.active_material.and_then(|slot| scene.palette.iter().find(|e| e.slot == slot));
        ui.add_enabled_ui(named.is_some(), |ui| {
            theme::checkbox(ui, &mut material, "Material", ACCENT());
        });
    });
    ui.data_mut(|d| d.insert_temp(id, (colour, material)));
    let locked = row.locked;
    ui.add_enabled_ui(!locked, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let w = theme::row_width(ui, 3, 4.0);
            let fills = [
                ("Fill All", FillArea::All, "Paints the whole layer with the Paint brush's colour (and the material picked)."),
                ("Fill Masked", FillArea::SculptMask { invert: false }, "Paints the layer as far as the surface is masked."),
                ("Fill Unmasked", FillArea::SculptMask { invert: true }, "Paints the layer where the surface is not masked."),
            ];
            for (label, area, help) in fills {
                if tip(theme::button(ui, w, label, ButtonStyle { height: 24.0, ..Default::default() }), tips, help).clicked() {
                    actions.push(Action::Paint(PaintAction::Fill { area, colour, material }));
                }
            }
        });
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, 2, 6.0);
            let help = "Paints the face group picked on the model (Ctrl + Shift + click it, or Pick on the Face Groups card).";
            if tip(theme::button(ui, w, "Fill Picked Group", ButtonStyle::default()), tips, help).clicked() {
                actions.push(Action::Paint(PaintAction::FillPickedGroup { colour, material }));
            }
            let help = "Empties this layer of its paint. Undo (Z) brings it back.";
            if tip(theme::button(ui, w, "Clear Layer", ButtonStyle::default()), tips, help).clicked() {
                actions.push(Action::Paint(PaintAction::Clear(row.id)));
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
        &format!(
            "Paint takes 4 bytes for each vertex a layer's colour covers, and as much again for its materials: {} for \
             {layers}. One object can use {MOST_MATERIALS} materials.",
            super::levels::megabytes(bytes)
        ),
    );
}

fn shortened(name: &str, most: usize) -> String {
    if name.chars().count() <= most { name.to_owned() } else { format!("{}...", name.chars().take(most - 3).collect::<String>()) }
}

// --------------------------------------------------------- the brush settings

/// The Paint brush's own settings, in the Brush Settings card.
#[allow(clippy::too_many_arguments)]
pub fn brush_settings(
    ui: &mut Ui,
    paint: &mut PaintSettings,
    scene: &ScenePaint,
    mesh: &Mesh,
    tabs: &mut super::tabs::Showing,
    tips: bool,
    actions: &mut Vec<Action>,
) {
    // What the brush does.
    let modes: Vec<&str> = PaintMode::ALL.iter().map(|m| m.name()).collect();
    let at = PaintMode::ALL.iter().position(|m| *m == paint.mode).unwrap_or(0);
    if let Some(k) = look::choice(ui, &modes, at) {
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

    // The channels the brush writes.
    theme::heading(ui, "Paints");
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        theme::checkbox(ui, &mut paint.rgb, "Colour", ACCENT());
        theme::checkbox(ui, &mut paint.material, "Material", ACCENT());
    });
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        theme::checkbox(ui, &mut paint.roughness, "Roughness", ACCENT());
        theme::checkbox(ui, &mut paint.metallic, "Metallic", ACCENT());
    });
    if paint.roughness {
        theme::slider(ui, "Roughness", &mut paint.roughness_value, 0.0..=1.0, look::percent);
    }
    if paint.metallic {
        theme::slider(ui, "Metallic", &mut paint.metallic_value, 0.0..=1.0, look::percent);
    }
    material_line(ui, paint, scene, tabs, tips, actions);
    layer_line(ui, paint, mesh, tips);

    // Zadd: colour and sculpt in one stroke.
    let help = "Colour and sculpt: each dab also runs the brush below, with this ring, falloff and alpha and that \
                brush's own Strength, so the paint stands up from the surface. Ctrl paints out and digs in.";
    tip(theme::checkbox(ui, &mut paint.zadd, "Zadd (Colour + Sculpt)", ACCENT()), tips, help);
    if paint.zadd {
        let names: Vec<&str> = ZADD_BRUSHES.iter().map(|b| b.name()).collect();
        let at = ZADD_BRUSHES.iter().position(|b| *b == paint.zadd_brush).unwrap_or(0);
        if let Some(k) = look::choice(ui, &names, at) {
            paint.zadd_brush = ZADD_BRUSHES[k];
        }
    }
    theme::hint(ui, "Ctrl erases, Shift blurs the paint, I picks up the colour under the pointer, P puts the brush down.");
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

/// The material the brush lays: the scene's pick on the Materials card.
fn material_line(
    ui: &mut Ui,
    paint: &PaintSettings,
    scene: &ScenePaint,
    tabs: &mut super::tabs::Showing,
    tips: bool,
    actions: &mut Vec<Action>,
) {
    let entry = scene.active_material.and_then(|slot| scene.palette.iter().find(|e| e.slot == slot));
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
        match entry {
            Some(e) => {
                ui.painter().circle_filled(rect.center(), 7.0, swatch32(e.swatch));
                ui.painter().circle_stroke(rect.center(), 7.0, Stroke::new(1.0, Color32::from_black_alpha(110)));
            }
            None => {
                ui.painter().circle_stroke(rect.center(), 6.5, Stroke::new(1.2, theme::HINT()));
            }
        }
        let name = entry.map_or("None (colour only)".to_owned(), |e| e.name.clone());
        let ink = if paint.material { theme::TEXT() } else { theme::HINT() };
        ui.label(egui::RichText::new(format!("Material: {name}")).font(theme::font(12.0)).color(ink));
    });
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, 2, 6.0);
        ui.add_enabled_ui(entry.is_some(), |ui| {
            let help = "Paint colour only, with no material.";
            if tip(theme::button(ui, w, "None", ButtonStyle::default()), tips, help).clicked() {
                actions.push(Action::Material(crate::materials::Ask::ColourOnly));
            }
        });
        let help = "Opens the Materials card (Scene panel, Look tab) to pick the material to paint with.";
        if tip(theme::button(ui, w, "Materials...", ButtonStyle::default()), tips, help).clicked() {
            tabs.scene = super::tabs::Scene::Look;
            crate::prefs::cards::open(ui.ctx(), "Materials");
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

/// The Masking card's paint commands, for an object with paint.
pub fn masking(ui: &mut Ui, doc: &mut Doc, tips: bool, actions: &mut Vec<Action>) {
    if !doc.mesh.has_paint() {
        return;
    }
    theme::heading(ui, "Mask by Paint");
    let paint = &mut doc.sculptor.settings.paint;
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
        ui.painter().rect_filled(rect, 3.0, colour32(paint.colour));
        ui.label(egui::RichText::new("The Paint brush's colour").font(theme::font(11.0)).color(theme::HINT()));
    });
    tip(
        theme::slider(ui, "Colour Tolerance", &mut paint.tolerance, 0.0..=1.0, look::percent),
        tips,
        "How far from the colour still counts for Mask by Colour: 0% only that colour, more takes in its neighbours.",
    );
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let w = theme::row_width(ui, 3, 4.0);
        let buttons = [
            ("By Colour", PaintAction::MaskByColour, "Masks where the paint is the Paint brush's colour, within the tolerance."),
            ("By Material", PaintAction::MaskByMaterial, "Masks where the material picked on the Materials card is painted."),
            ("By Layer", PaintAction::MaskByLayer, "Masks where the paint layer being painted on has paint."),
        ];
        for (label, action, help) in buttons {
            if tip(theme::button(ui, w, label, ButtonStyle { height: 24.0, ..Default::default() }), tips, help).clicked() {
                actions.push(Action::Paint(action));
            }
        }
    });
}
