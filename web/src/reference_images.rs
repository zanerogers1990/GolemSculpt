//! Reference pictures in the web version: photos or drawings of the subject
//! standing in the scene to sculpt against, as on the desktop. The pictures
//! themselves are the desktop's (`src/references.rs`, compiled in as it is:
//! where one stands, which view it faces, reading its image); this is the
//! web app's side of them, ported from the desktop's `app/references.rs`
//! and its card, `ui/references.rs`:
//!
//! - **Add Picture...** (the browser's file picker, offering .png and .jpg),
//!   or a photo dropped on the page, stands a picture facing the axis view
//!   the camera is in (the front, if none), as tall as the scene and just
//!   behind it.
//! - The **Reference Images** card, last on the Scene tab as on the
//!   desktop's Objects tab: the list, and the selected picture's facing,
//!   size, place, opacity, Flip, In Front and Show, Fit to Model, Delete.
//! - They are kept with the scene, in the browser and in a Save Scene file,
//!   image and all (`picture_bytes`, after the scene's other blocks). They
//!   are not objects: brushes, Frame, Remesh, saved models and Save Picture
//!   leave them out (picture_pass.rs draws them in the viewport only).

use std::cell::RefCell;
use std::sync::Arc;

use eframe::egui::{self, Align2, Color32, Id, Rect, Sense, Ui, pos2, vec2};
use sculpt_geom::LengthUnit;

use super::GolemApp;
use super::ui::theme::{self, ACCENT, ButtonStyle};
use super::ui::tip;
use crate::browser::Later;
use crate::references::{Facing, Picture, Placement, References};
use crate::render::picture_pass::Draw;

/// The card's colour on the panel.
const TINT: Color32 = Color32::from_rgb(214, 170, 120);
const ROW: f32 = 34.0;
const THUMB: [f32; 2] = [40.0, 26.0];
const THUMB_PIXELS: u32 = 96;
/// Where the pictures begin in the scene the browser keeps.
const TAG: &[u8; 4] = b"REFS";

#[derive(Default)]
struct State {
    references: References,
    /// A picture on its way from the file picker.
    picked: Option<Later<(String, Vec<u8>)>>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

fn millimetres(length: f32) -> String {
    sculpt_geom::units::format(length, LengthUnit::Millimetres, true)
}

/// Whether `name` is a picture (a photo dropped on the page is a reference picture).
pub fn is_picture(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    crate::references::EXTENSIONS.iter().any(|e| lower.ends_with(&format!(".{e}")))
}

/// A fingerprint of what is kept of the pictures, to know a change.
fn kept_print(references: &References) -> u64 {
    use std::hash::Hasher;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    references.hash_kept(&mut hasher);
    hasher.finish()
}

impl GolemApp {
    /// A picture picked or dropped: stood in the scene, selected (the
    /// desktop's `add_reference`).
    pub(super) fn add_reference(&mut self, name: &str, file: Vec<u8>) {
        if file.len() > 200 << 20 {
            self.status = format!("{name} is {} MB, too large to keep in a scene. Save a smaller copy of it.", file.len() >> 20);
            return;
        }
        let shared = STATE.with(|s| s.borrow().references.same_image(&file));
        let (file, image) = match shared {
            Some(shared) => shared,
            None => match crate::references::decode(&file) {
                Ok(image) => (Arc::from(file), Arc::new(image)),
                Err(e) => {
                    self.status = format!("{name}: {e}");
                    return;
                }
            },
        };
        let view = self.camera.saved_view();
        let facing = Facing::of_view(view.yaw, view.pitch).unwrap_or_default();
        let place = Placement::fitted(facing, self.scene_bounds());
        let (w, h) = image.dimensions();
        STATE.with(|s| s.borrow_mut().references.add(Picture::new(name.to_owned(), file, image, place)));
        self.mark_changed();
        self.status = format!(
            "Added {name} ({w} x {h}) as a reference picture facing the {} view. It is in the Reference Images card \
             (Scene tab).",
            facing.name().to_lowercase()
        );
    }

    /// The pictures shown, for the renderer, as they stand this frame.
    pub(super) fn picture_draws(&mut self) -> Vec<Draw> {
        // A picture the file picker has handed over.
        let picked = STATE.with(|s| {
            let mut s = s.borrow_mut();
            let got = s.picked.as_ref().and_then(|slot| slot.borrow_mut().take());
            if got.is_some() {
                s.picked = None;
            }
            got
        });
        if let Some((name, bytes)) = picked {
            self.add_reference(&name, bytes);
        }
        STATE.with(|s| {
            s.borrow()
                .references
                .pictures
                .iter()
                .filter(|p| p.place.shown)
                .map(|p| Draw {
                    id: p.id,
                    image: p.image.clone(),
                    corners: p.corners(),
                    uvs: p.place.image_corners(),
                    opacity: p.place.opacity,
                    in_front: p.place.in_front,
                })
                .collect()
        })
    }

    /// The pictures as the scene the browser keeps holds them: the tag, then
    /// each picture's name, its placement (JSON, as the desktop's file keeps
    /// it) and its image file. Nothing when there are none.
    pub(super) fn picture_bytes(&self) -> Vec<u8> {
        STATE.with(|s| {
            let s = s.borrow();
            let mut out = Vec::new();
            if s.references.pictures.is_empty() {
                return out;
            }
            out.extend_from_slice(TAG);
            out.extend_from_slice(&(s.references.pictures.len() as u32).to_le_bytes());
            for p in &s.references.pictures {
                let place = serde_json::to_vec(&p.place).unwrap_or_default();
                for part in [p.name.as_bytes(), &place, &p.file] {
                    out.extend_from_slice(&(part.len() as u32).to_le_bytes());
                    out.extend_from_slice(part);
                }
            }
            out
        })
    }

    /// The pictures a kept scene holds (what [`Self::picture_bytes`] wrote,
    /// at the start of `bytes`), in place of the scene's; none for a scene
    /// without.
    pub(super) fn pictures_restore(&mut self, bytes: &[u8]) {
        let mut pictures = Vec::new();
        let mut unread = 0;
        if let Some(rest) = bytes.strip_prefix(TAG) {
            let mut at = 4usize;
            let mut part = || -> Option<&[u8]> {
                let n = u32::from_le_bytes(rest.get(at..at + 4)?.try_into().ok()?) as usize;
                let bytes = rest.get(at + 4..at + 4 + n)?;
                at += 4 + n;
                Some(bytes)
            };
            for _ in 0..part_count(rest) {
                let (Some(name), Some(place), Some(file)) = (part(), part(), part()) else { break };
                let name = String::from_utf8_lossy(name).into_owned();
                let place: Placement = serde_json::from_slice(place).unwrap_or_default();
                let file: Arc<[u8]> = Arc::from(file);
                let shared = pictures.iter().find(|p: &&Picture| *p.file == *file).map(|p| p.image.clone());
                match shared.map(Ok).unwrap_or_else(|| crate::references::decode(&file).map(Arc::new)) {
                    Ok(image) => pictures.push(Picture::new(name, file, image, place)),
                    Err(_) => unread += 1,
                }
            }
        }
        STATE.with(|s| {
            let r = &mut s.borrow_mut().references;
            r.selected = pictures.first().map(|p| p.id);
            r.pictures = pictures;
        });
        if unread > 0 {
            let was = if unread == 1 { "1 reference picture was" } else { "Some reference pictures were" };
            self.status += &format!(" {was} not readable and left out.");
        }
    }

    /// The Reference Images card (the desktop's), last on the Scene tab.
    pub(super) fn references_card(&mut self, ui: &mut Ui) {
        let tips = self.ui.tips;
        let bounds = self.scene_bounds();
        let eyes = (self.ui.pictures.icon("eye"), self.ui.pictures.icon("eye_off"));
        let mut add = false;
        let changed = STATE.with(|s| {
            let mut s = s.borrow_mut();
            let before = kept_print(&s.references);
            let references = &mut s.references;
            let count = references.pictures.len();
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
                     Pictures are kept with the scene, image and all, in this browser and in a Save Scene file. They are \
                     not objects: brushes, Frame, Remesh, saved models and pictures leave them out.",
                );
                let style = ButtonStyle { active: Some(ACCENT()), ..Default::default() };
                let button = theme::button(ui, ui.available_width(), "Add Picture...", style);
                if tip(button, tips, "Choose a photo or drawing (.png or .jpg) to stand in the scene.").clicked() {
                    add = true;
                }
                if count == 0 {
                    theme::hint(ui, "No pictures yet. A front photo and a side photo of the subject are a good start. Or drop one on the page.");
                    return;
                }
                rows(ui, references, eyes, tips);
                settings(ui, references, bounds, tips);
            });
            kept_print(&s.references) != before
        });
        if add {
            STATE.with(|s| s.borrow_mut().picked = Some(crate::browser::pick_file_of(".png,.jpg,.jpeg")));
        }
        if STATE.with(|s| s.borrow().picked.is_some()) {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
        if changed {
            self.mark_changed();
        }
        if let Some(said) = ui.data_mut(|d| d.remove_temp::<String>(Id::new("reference picture said"))) {
            self.status = said;
        }
    }
}

/// How many pictures the block after the tag says it holds.
fn part_count(rest: &[u8]) -> usize {
    rest.get(..4).map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()) as usize).min(10_000)
}

fn rows(ui: &mut Ui, references: &mut References, eyes: (Option<egui::TextureId>, Option<egui::TextureId>), tips: bool) {
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
        let (icon, alpha) = if picture.place.shown { (eyes.0, 235) } else { (eyes.1, 80) };
        if let Some(icon) = icon {
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

fn settings(ui: &mut Ui, references: &mut References, bounds: crate::mesh::bvh::Aabb, tips: bool) {
    let Some(id) = references.selected().map(|p| p.id) else { return };
    let mut delete_now = false;
    {
        let Some(picture) = references.get_mut(id) else { return };
        let aspect = picture.aspect();
        let place = &mut picture.place;
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
        if place.in_front && !was_in_front && place.opacity > 0.95 {
            place.opacity = 0.5;
        }
        let shown = theme::checkbox(ui, &mut place.shown, "Show", ACCENT());
        tip(shown, tips, "Show or hide the picture. A hidden one stays in the scene and in its file.");
    }

    // Delete, asking first in the card: Delete, then Delete Picture or Keep.
    let asked_id = Id::new("reference picture delete asked");
    let asked = ui.data(|d| d.get_temp::<u64>(asked_id)) == Some(id);
    ui.add_space(4.0);
    if !asked {
        let delete = theme::button(ui, ui.available_width(), "Delete", ButtonStyle { height: 24.0, ..Default::default() });
        if tip(delete, tips, "Take this picture out of the scene. It asks first.").clicked() {
            ui.data_mut(|d| d.insert_temp(asked_id, id));
        }
    } else {
        theme::hint(ui, "Delete this picture? Its image is kept in this scene only, and Undo does not bring it back.");
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let w = theme::row_width(ui, 2, 4.0);
            let style = ButtonStyle { active: Some(theme::ERROR), height: 24.0, ..Default::default() };
            if theme::button(ui, w, "Delete Picture", style).clicked() {
                delete_now = true;
                ui.data_mut(|d| d.remove::<u64>(asked_id));
            }
            if theme::button(ui, w, "Keep", ButtonStyle { height: 24.0, ..Default::default() }).clicked() {
                ui.data_mut(|d| d.remove::<u64>(asked_id));
            }
        });
    }
    if delete_now {
        if let Some(gone) = references.remove(id) {
            ui.data_mut(|d| d.insert_temp(Id::new("reference picture said"), format!("Deleted the reference picture {}.", gone.name)));
        }
    }
}
