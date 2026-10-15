//! The Alpha part of the brush settings: a grid of the pictures there are to
//! print with, and how the one picked is laid under the brush. Each brush
//! keeps its own. Shown only for the brushes that take one, and not while
//! mask painting.

use std::collections::HashMap;
use std::sync::Arc;

use egui::{Align2, Color32, ColorImage, Id, Rect, Sense, TextureHandle, TextureOptions, Ui, pos2, vec2};

use super::theme::{self, ACCENT, ButtonStyle};
use crate::doc::Action;
use crate::sculpt::alpha::{self, AlphaSettings, Thumb};
use crate::sculpt::Settings;

const PER_ROW: usize = 5;
const GAP: f32 = 4.0;
/// Rows of tiles shown before the grid scrolls.
const ROWS_SHOWN: f32 = 3.0;

/// The tiles' pictures as the graphics card holds them, by alpha name, each
/// with the picture it was made from: a rescan can put a new picture under
/// an old name.
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

/// The Alpha section. Picking, importing and rescanning are the only things
/// that reach past the settings: the last two go to the app as actions,
/// because one of them opens a file box.
pub fn section(ui: &mut Ui, s: &mut Settings, tips: bool, actions: &mut Vec<Action>) {
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
                    if theme::tip(tile(ui, side, picture, active), tips, &text).clicked() {
                        s.alpha[slot].image = name.and_then(alpha::find);
                    }
                }
            });
        }
    };
    if rows as f32 > ROWS_SHOWN {
        egui::ScrollArea::vertical()
            .id_salt("alpha_grid")
            .max_height(ROWS_SHOWN * (side + GAP) + side * 0.5)
            .show(ui, |ui| grid(ui, s));
    } else {
        grid(ui, s);
    }

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let w = theme::row_width(ui, 2, 6.0);
        let import = theme::button(ui, w, "Import Alpha...", ButtonStyle::default());
        let text = "Add a greyscale picture (.png or .jpg) of your own. It is copied into the alpha folder.";
        if theme::tip(import, tips, text).clicked() {
            actions.push(Action::ImportAlpha);
        }
        let rescan = theme::button(ui, w, "Rescan Folder", ButtonStyle::default());
        if theme::tip(rescan, tips, "Look through the alpha folder again for pictures you have put there.").clicked() {
            actions.push(Action::RescanAlphas);
        }
    });

    let a = &mut s.alpha[slot];
    if a.image.is_none() {
        return;
    }
    let label = if a.drag { "Drag to Place" } else { "Drag to Place (Off)" };
    theme::tip(
        theme::checkbox(ui, &mut a.drag, label, ACCENT()),
        tips,
        "On: one stroke is one print. Press where its middle goes and drag outward to size it; lift to leave it. Strength sets how tall it stands for its size. Off: the picture is laid down all along the stroke.",
    );
    let scale = AlphaSettings::SCALE;
    theme::tip(
        theme::slider(ui, "Alpha Size", &mut a.scale, scale, |v| format!("{:.0}%", v * 100.0)),
        tips,
        "How big the picture is against the brush ring. At 100% it fills the square the ring sits in.",
    );
    theme::slider(ui, "Alpha Rotation", &mut a.rotation, 0.0..=360.0, |v| format!("{v:.0}\u{b0}"));
    theme::tip(
        theme::slider(ui, "Random Rotation", &mut a.random_rotation, AlphaSettings::RANDOM_ROTATION, |v| format!("\u{b1}{v:.0}\u{b0}")),
        tips,
        "Turns each print by a random angle, up to this far either way, so a row of prints does not look stamped \
         by a machine. At 180\u{b0} a print may face any way at all. Raise the brush's Spacing to see the prints apart. 0 is off.",
    );
    let label = if a.follow { "Turn With Stroke" } else { "Turn With Stroke (Off)" };
    theme::tip(
        theme::checkbox(ui, &mut a.follow, label, ACCENT()),
        tips,
        "On: the top of the picture points the way the stroke is going, so a rake's lines follow your hand (or, with Drag to Place, the way you drag). Off: it points up the screen.",
    );
    let label = if a.tile { "Repeat" } else { "Repeat (Off)" };
    theme::tip(
        theme::checkbox(ui, &mut a.tile, label, ACCENT()),
        tips,
        "Repeat the picture past its edges. With a small Alpha Size this covers the brush in a fine pattern.",
    );
    let label = if a.invert { "Invert" } else { "Invert (Off)" };
    theme::tip(
        theme::checkbox(ui, &mut a.invert, label, ACCENT()),
        tips,
        "Swap black and white: bumps become pits.",
    );
    theme::about(
        ui,
        "The brush's soft edge still fades the picture toward the ring: slide Focal Shift toward Hard to print more of it. A stroke smears a picture along itself; Drag to Place prints it once, whole.",
    );
}
