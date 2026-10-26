//! The Reference Images card of the Scene panel's Objects tab: the scene's
//! reference pictures (see references.rs) in a list, and the settings of the
//! one selected.
//!
//! The card changes a picture's settings itself, straight on the scene, as
//! the Symmetry card does an object's symmetry: they are not undo steps, and
//! the scene's fingerprint (app/unsaved.rs) sees every change, so Save is
//! asked about it. Adding a picture needs the file box and deleting one the
//! app, so those are asked of the app (`Ask`, carried out in
//! app/references.rs). Delete asks first, in the card itself: the image is
//! kept in the scene and nowhere else, and Undo does not bring it back.

use egui::{Align2, Color32, Id, Rect, Sense, Ui, pos2, vec2};
use sculpt_geom::LengthUnit;

use super::assets::Assets;
use super::theme::{self, ACCENT, ButtonStyle};
use super::tip;
use crate::doc::{Action, Doc};
use crate::references::{Ask, Facing, Picture, Placement, References};

/// The card's colour on the panel.
const TINT: Color32 = Color32::from_rgb(214, 170, 120);
/// A row of the list, and the room its small picture has.
const ROW: f32 = 34.0;
const THUMB: [f32; 2] = [40.0, 26.0];
/// The longest side a list picture is made at, in pixels.
const THUMB_PIXELS: u32 = 96;

/// A length as the cards show one.
fn millimetres(length: f32) -> String {
    sculpt_geom::units::format(length, LengthUnit::Millimetres, true)
}

pub fn card(ui: &mut Ui, doc: &mut Doc, assets: &Assets, actions: &mut Vec<Action>) {
    let tips = doc.opts.tooltips;
    let count = doc.scene.references.pictures.len();
    let summary = match count {
        0 => String::new(),
        1 => "1 picture".to_owned(),
        n => format!("{n} pictures"),
    };
    theme::category(ui, "Reference Images", TINT, &summary, false, |ui| {
        theme::about(
            ui,
            "Photos or drawings of the subject, standing in the scene to sculpt against: match the model's outline to \
             the front photo in the front view, to the side photo in the side view.\n\n\
             A new picture faces the view you are in (turn to Front, Right or Top with the view gizmo first, else it \
             faces the front), as tall as the model and just behind it. It shows only from the side it faces, so it \
             never stands between you and the model as you orbit round.\n\n\
             Pictures are saved in the scene, image and all, so it opens with them anywhere. They are not objects: \
             brushes, Frame, Remesh, exports and renders leave them out.",
        );
        let style = ButtonStyle { active: Some(ACCENT()), ..Default::default() };
        let add = theme::button(ui, ui.available_width(), "Add Picture...", style);
        if tip(add, tips, "Choose a photo or drawing (.png or .jpg) to stand in the scene.").clicked() {
            actions.push(Action::References(Ask::Add));
        }
        if count == 0 {
            theme::hint(ui, "No pictures yet. A front photo and a side photo of the subject are a good start.");
            return;
        }
        rows(ui, &mut doc.scene.references, assets, tips);
        // The scene's size, for how far the sliders reach and for Fit to Model.
        let bounds = doc.scene.bounds(&doc.mesh);
        settings(ui, &mut doc.scene.references, bounds, tips, actions);
    });
}

/// The list: each picture small, its name, and its eye. A tap selects it;
/// the eye shows or hides it.
fn rows(ui: &mut Ui, references: &mut References, assets: &Assets, tips: bool) {
    let selected = references.selected().map(|p| p.id);
    let (mut pick, mut toggle) = (None, None);
    for picture in &references.pictures {
        let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
        let eye = Rect::from_min_max(pos2(rect.right() - 28.0, rect.top()), rect.max);
        let name = Rect::from_min_max(rect.min, pos2(eye.left() - 4.0, rect.bottom()));
        let on_eye = |at: Option<egui::Pos2>| at.is_some_and(|at| eye.contains(at));
        if response.clicked() {
            if on_eye(response.interact_pointer_pos()) {
                toggle = Some(picture.id);
            } else {
                pick = Some(picture.id);
            }
        }
        let hover = response.hover_pos();
        let response = tip(response, tips, if on_eye(hover) { "Show or hide this picture." } else { picture.name.as_str() });
        if !ui.is_rect_visible(rect) {
            continue;
        }
        let picked = selected == Some(picture.id);
        let p = ui.painter();
        p.rect_filled(name, 5.0, if picked { ACCENT() } else { theme::CONTROL() });
        if response.hovered() && !on_eye(hover) {
            p.rect_filled(name, 5.0, Color32::from_white_alpha(20));
        }
        let mut ink = if picked { Color32::WHITE } else { theme::TEXT() };
        if !picture.place.shown {
            ink = ink.gamma_multiply(0.55);
        }
        let thumb = thumbnail(ui, picture);
        let size = thumb.size_vec2();
        let scale = (THUMB[0] / size.x.max(1.0)).min(THUMB[1] / size.y.max(1.0));
        let at = Rect::from_center_size(pos2(name.left() + 6.0 + THUMB[0] * 0.5, name.center().y), size * scale);
        let full = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        let p = ui.painter();
        p.image(thumb.id(), at, full, if picture.place.shown { Color32::WHITE } else { Color32::from_white_alpha(110) });
        let text = pos2(name.left() + THUMB[0] + 14.0, name.center().y);
        let clip = Rect::from_min_max(name.min, pos2(name.right() - 4.0, name.bottom()));
        p.with_clip_rect(clip.intersect(p.clip_rect())).text(text, Align2::LEFT_CENTER, &picture.name, theme::font(12.0), ink);
        if on_eye(hover) {
            p.rect_filled(eye, 5.0, Color32::from_white_alpha(20));
        }
        let (icon, alpha) = if picture.place.shown { ("eye", 235) } else { ("eye_off", 80) };
        if let Some(icon) = assets.icon(icon) {
            p.image(icon, Rect::from_center_size(eye.center(), egui::Vec2::splat(16.0)), full, Color32::from_white_alpha(alpha));
        }
    }
    if let Some(id) = pick {
        references.selected = Some(id);
    }
    if let Some(picture) = toggle.and_then(|id| references.get_mut(id)) {
        picture.place.shown = !picture.place.shown;
    }
}

/// The list's small copy of a picture, made the first time the list shows it
/// and kept with the interface's own data for as long as the app runs.
fn thumbnail(ui: &Ui, picture: &Picture) -> egui::TextureHandle {
    let id = Id::new(("reference picture thumbnail", picture.id));
    if let Some(handle) = ui.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return handle;
    }
    let (w, h) = picture.image.dimensions();
    let scale = THUMB_PIXELS as f32 / w.max(h).max(1) as f32;
    let (tw, th) = (((w as f32 * scale).round() as u32).clamp(1, THUMB_PIXELS), ((h as f32 * scale).round() as u32).clamp(1, THUMB_PIXELS));
    let small = image::imageops::thumbnail(&*picture.image, tw, th);
    let colours = egui::ColorImage::from_rgba_unmultiplied([tw as usize, th as usize], small.as_raw());
    let handle = ui.ctx().load_texture(format!("reference picture {}", picture.id), colours, egui::TextureOptions::LINEAR);
    ui.data_mut(|d| d.insert_temp(id, handle.clone()));
    handle
}

/// The selected picture's settings.
fn settings(ui: &mut Ui, references: &mut References, bounds: crate::mesh::bvh::Aabb, tips: bool, actions: &mut Vec<Action>) {
    let Some(id) = references.selected().map(|p| p.id) else { return };
    let Some(picture) = references.get_mut(id) else { return };
    let aspect = picture.aspect();
    let place = &mut picture.place;
    // How far the sliders reach: a good way past the scene and the picture.
    let scene = (bounds.max - bounds.min).max_element();
    let scene = if scene.is_finite() && scene > 1e-6 { scene } else { 2.0 };
    let span = 2.0 * scene.max(place.size * aspect.max(1.0));

    ui.add_space(4.0);
    theme::heading(ui, "Facing");
    for row in Facing::ALL.chunks(3) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let w = theme::row_width(ui, row.len(), 4.0);
            for &facing in row {
                let style = ButtonStyle { active: (place.facing == facing).then(ACCENT), height: 24.0, ..Default::default() };
                let button = theme::button(ui, w, facing.name(), style);
                let help = format!("Face the {} view: it sees the picture straight on.", facing.name().to_lowercase());
                if tip(button, tips, &help).clicked() {
                    place.facing = facing;
                }
            }
        });
    }
    let fit = theme::button(ui, ui.available_width(), "Fit to Model", ButtonStyle { height: 24.0, ..Default::default() });
    if tip(fit, tips, "Make it as tall as the model, its middle on the model's and just behind it, as seen from the view it faces.")
        .clicked()
    {
        let Placement { opacity, flip, in_front, shown, .. } = *place;
        *place = Placement { opacity, flip, in_front, shown, ..Placement::fitted(place.facing, bounds) };
    }

    // Size on a log scale: equal travel multiplies it.
    let (small, large) = (scene * 0.02, scene * 20.0);
    let mut log = place.size.max(1e-6).ln();
    let size = theme::slider(ui, "Size", &mut log, small.ln()..=large.ln(), |v| millimetres(v.exp()));
    if tip(size, tips, "How tall the picture stands. Its width follows from the image.").changed() {
        place.size = log.exp();
    }
    let lengths: [(&str, &mut f32, &str); 3] = [
        ("Left / Right", &mut place.across, "Slide the picture to the left or the right, as its view sees it."),
        ("Up / Down", &mut place.up, "Slide the picture up or down."),
        (
            "Depth",
            &mut place.depth,
            "How far behind the middle of the scene it stands, seen from its view. Less than nothing brings it in front.",
        ),
    ];
    for (label, value, help) in lengths {
        let slider = theme::slider(ui, label, value, -span..=span, millimetres);
        tip(slider, tips, help);
    }
    let opacity = theme::slider(ui, "Opacity", &mut place.opacity, 0.0..=1.0, |v| format!("{:.0}%", v * 100.0));
    tip(opacity, tips, "How solid the picture is. Lower it to see the model through a picture In Front.");
    let flip = theme::checkbox(ui, &mut place.flip, "Flip", ACCENT());
    tip(flip, tips, "Mirror the image left to right: one photo of a side serves for both sides.");
    let was_in_front = place.in_front;
    let in_front = theme::checkbox(ui, &mut place.in_front, "In Front", ACCENT());
    tip(in_front, tips, "Draw the picture over the model, see-through, to trace its outline. Otherwise the model hides it.");
    // A solid picture over the model would hide it altogether.
    if place.in_front && !was_in_front && place.opacity > 0.95 {
        place.opacity = 0.5;
    }
    let shown = theme::checkbox(ui, &mut place.shown, "Show", ACCENT());
    tip(shown, tips, "Show or hide the picture. A hidden one stays in the scene and in its file.");

    delete(ui, id, tips, actions);
}

/// Delete, asking first in the card: Delete, then Delete Picture or Keep.
fn delete(ui: &mut Ui, id: u64, tips: bool, actions: &mut Vec<Action>) {
    let asked_id = Id::new("reference picture delete asked");
    let asked = ui.data(|d| d.get_temp::<u64>(asked_id)) == Some(id);
    ui.add_space(4.0);
    if !asked {
        let delete = theme::button(ui, ui.available_width(), "Delete", ButtonStyle { height: 24.0, ..Default::default() });
        if tip(delete, tips, "Take this picture out of the scene. It asks first.").clicked() {
            ui.data_mut(|d| d.insert_temp(asked_id, id));
        }
        return;
    }
    theme::hint(ui, "Delete this picture? Its image is kept in this scene only, and Undo does not bring it back.");
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let w = theme::row_width(ui, 2, 4.0);
        let style = ButtonStyle { active: Some(theme::ERROR), height: 24.0, ..Default::default() };
        if theme::button(ui, w, "Delete Picture", style).clicked() {
            actions.push(Action::References(Ask::Delete(id)));
            ui.data_mut(|d| d.remove::<u64>(asked_id));
        }
        if theme::button(ui, w, "Keep", ButtonStyle { height: 24.0, ..Default::default() }).clicked() {
            ui.data_mut(|d| d.remove::<u64>(asked_id));
        }
    });
}
