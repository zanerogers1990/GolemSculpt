//! Brush alphas in the web version: the Alpha part of the brush settings (a
//! grid of the pictures there are to print with, and how the one picked is
//! laid under the brush) and the picture shown faintly inside the ring, so a
//! print can be aimed before it is made.
//!
//! The alphas and what a stroke does with one are the engine's own
//! (`sculpt/alpha.rs`); the section follows the desktop's `ui/alpha.rs` and
//! the faint picture is the desktop's `ui/alpha_preview.rs`, compiled here as
//! it is. The web version has the alphas that come with the app and nothing
//! from disk, so the desktop's Import Alpha and Rescan Folder are left out.

use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui::{self, Align2, Color32, ColorImage, Id, Pos2, Rect, Sense, TextureHandle, TextureOptions, Ui, pos2, vec2};

use super::GolemApp;
use super::ui::theme::{self, ACCENT};
use super::ui::tip;
use crate::sculpt::Settings;
use crate::sculpt::alpha::{self, AlphaSettings, Thumb};

#[path = "../../src/ui/alpha_preview.rs"]
mod alpha_preview;

const PER_ROW: usize = 5;
const GAP: f32 = 4.0;
/// Rows of tiles shown before the grid scrolls.
const ROWS_SHOWN: f32 = 3.0;

/// The tiles' pictures as the graphics card holds them, by alpha name.
#[derive(Clone, Default)]
struct Tiles(HashMap<String, (Arc<Thumb>, TextureHandle)>);

fn texture(ui: &Ui, name: &str, thumb: &Arc<Thumb>) -> egui::TextureId {
    let id = Id::new("alpha_tiles");
    let kept = ui.data(|d| d.get_temp::<Tiles>(id)).unwrap_or_default();
    if let Some((from, handle)) = kept.0.get(name) {
        if Arc::ptr_eq(from, thumb) {
            return handle.id();
        }
    }
    let image = ColorImage::from_gray([thumb.side, thumb.side], &thumb.pixels);
    let handle = ui.ctx().load_texture(format!("alpha/{name}"), image, TextureOptions::LINEAR);
    let texture = handle.id();
    ui.data_mut(|d| d.get_temp_mut_or_default::<Tiles>(id).0.insert(name.to_owned(), (thumb.clone(), handle)));
    texture
}

/// One tile of the grid: a picture, or with none the word for no alpha.
fn tile(ui: &mut Ui, side: f32, picture: Option<egui::TextureId>, active: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(side, side), Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let p = ui.painter();
    p.rect_filled(rect, 5.0, if active { ACCENT() } else { theme::CONTROL() });
    let inner = rect.shrink(3.0);
    match picture {
        Some(picture) => {
            p.rect_filled(inner, 3.0, Color32::BLACK);
            let tint = if active || response.hovered() { Color32::WHITE } else { Color32::from_white_alpha(215) };
            p.image(picture, inner, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), tint);
        }
        None => {
            p.text(rect.center(), Align2::CENTER_CENTER, "None", theme::font(10.0), theme::TEXT());
        }
    }
    response
}

/// The Alpha section of the Brush Settings card, for the brushes that take
/// one and not while mask painting. Each brush keeps its own.
pub fn section(ui: &mut Ui, s: &mut Settings, tips: bool) {
    if s.mask_mode || !s.brush.takes_alpha() {
        return;
    }
    theme::heading(ui, "Alpha");
    let slot = s.brush.index();
    let listing = alpha::listing();
    let picked = s.alpha[slot].image.map(|image| image.name.clone());

    let side = theme::row_width(ui, PER_ROW, GAP);
    let rows = (listing.len() + 1).div_ceil(PER_ROW);
    let grid = |ui: &mut Ui, s: &mut Settings| {
        ui.spacing_mut().item_spacing = vec2(GAP, GAP);
        // The first tile is no alpha at all.
        let mut tiles = std::iter::once(None).chain(listing.iter().map(Some));
        for _ in 0..rows {
            ui.horizontal(|ui| {
                for listed in tiles.by_ref().take(PER_ROW) {
                    let name = listed.map(|l| l.name.as_str());
                    let active = name == picked.as_deref();
                    let picture = listed.map(|l| texture(ui, &l.name, &l.thumb));
                    let text = match name {
                        Some(name) => format!("{name}\nWhite prints, black does not."),
                        None => "No alpha: the brush's plain round tip.".to_owned(),
                    };
                    if tip(tile(ui, side, picture, active), tips, &text).clicked() {
                        s.alpha[slot].image = name.and_then(alpha::find);
                    }
                }
            });
        }
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
    };
    if rows as f32 > ROWS_SHOWN {
        egui::ScrollArea::vertical()
            .id_salt("alpha_grid")
            .max_height(ROWS_SHOWN * (side + GAP) + side * 0.5)
            .show(ui, |ui| grid(ui, s));
    } else {
        grid(ui, s);
    }

    let a = &mut s.alpha[slot];
    if a.image.is_none() {
        return;
    }
    let label = if a.drag { "Drag to Place" } else { "Drag to Place (Off)" };
    tip(
        theme::checkbox(ui, &mut a.drag, label, ACCENT()),
        tips,
        "On: one stroke is one print. Press where its middle goes and drag outward to size it; lift to leave it. Strength sets how tall it stands for its size. Off: the picture is laid down all along the stroke.",
    );
    tip(
        theme::slider(ui, "Alpha Size", &mut a.scale, AlphaSettings::SCALE, |v| format!("{:.0}%", v * 100.0)),
        tips,
        "How big the picture is against the brush ring. At 100% it fills the square the ring sits in.",
    );
    theme::slider(ui, "Alpha Rotation", &mut a.rotation, 0.0..=360.0, |v| format!("{v:.0}\u{b0}"));
    tip(
        theme::slider(ui, "Random Rotation", &mut a.random_rotation, AlphaSettings::RANDOM_ROTATION, |v| format!("\u{b1}{v:.0}\u{b0}")),
        tips,
        "Turns each print by a random angle, up to this far either way, so a row of prints does not look stamped \
         by a machine. At 180\u{b0} a print may face any way at all. Raise the brush's Spacing to see the prints apart. 0 is off.",
    );
    let label = if a.follow { "Turn With Stroke" } else { "Turn With Stroke (Off)" };
    tip(
        theme::checkbox(ui, &mut a.follow, label, ACCENT()),
        tips,
        "On: the top of the picture points the way the stroke is going, so a rake's lines follow your hand (or, with Drag to Place, the way you drag). Off: it points up the screen.",
    );
    let label = if a.tile { "Repeat" } else { "Repeat (Off)" };
    tip(
        theme::checkbox(ui, &mut a.tile, label, ACCENT()),
        tips,
        "Repeat the picture past its edges. With a small Alpha Size this covers the brush in a fine pattern.",
    );
    let label = if a.invert { "Invert" } else { "Invert (Off)" };
    tip(theme::checkbox(ui, &mut a.invert, label, ACCENT()), tips, "Swap black and white: bumps become pits.");
    theme::about(
        ui,
        "The brush's soft edge still fades the picture toward the ring: slide Focal Shift toward Hard to print more of it. A stroke smears a picture along itself; Drag to Place prints it once, whole.",
    );
}

impl GolemApp {
    /// Draws the picture the brush in hand will print, faintly inside the
    /// ring, when it has one and the pointer is over the model (or a stroke
    /// is under way that is not a print being dragged out, which is on the
    /// surface to be seen). As the desktop's `alpha_under_cursor`.
    pub(super) fn paint_alpha_preview(&self, painter: &egui::Painter, ppp: f32) {
        let sculptor = &self.sculptor;
        let s = &sculptor.settings;
        if s.mask_mode || !s.brush.takes_alpha() || self.mode != super::Mode::Sculpt {
            return;
        }
        let a = &s.alpha[s.brush.index()];
        let Some(image) = a.image else { return };
        let stroking = sculptor.is_active();
        let centre: Pos2 = if stroking {
            if a.drag {
                return;
            }
            let Some((px, _, _)) = sculptor.stroke_cursor() else { return };
            pos2(px.x / ppp, px.y / ppp)
        } else {
            let Some(hit) = self.hover else { return };
            let Some(px) = self.view.project(hit.pos) else { return };
            pos2(px.x / ppp, px.y / ppp)
        };
        // On the screen x goes across and y down; the view's own directions
        // are the model's, as the stroke's heading is.
        let on_screen = |v: glam::Vec3| vec2(v.dot(self.view.right), -v.dot(self.view.up));
        let heading = sculptor.stroke_heading().filter(|_| a.follow).map(on_screen).filter(|h| h.length() > 1e-6);
        let top = heading.map_or(vec2(0.0, -1.0), |h| h.normalized());
        // Right of a top that points up the screen is across it.
        let across = vec2(-top.y, top.x);
        let (sin, cos) = a.rotation.to_radians().sin_cos();
        let preview = alpha_preview::Preview {
            image,
            scale: a.scale.clamp(*AlphaSettings::SCALE.start(), *AlphaSettings::SCALE.end()),
            right: across * cos + top * sin,
            up: top * cos - across * sin,
            invert: a.invert,
        };
        let radius = (s.radius_px / ppp).max(7.0);
        alpha_preview::draw(painter, centre, radius, Color32::from_rgb(235, 235, 235), &preview);
    }
}
