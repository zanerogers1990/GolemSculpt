//! The Scene panel's Settings tab: what belongs to the app and not to the
//! sculpt. The interface's colours, its size, the keyboard shortcuts, and
//! how hard the machine is working.

use egui::{Align2, Color32, Rect, Sense, Stroke, StrokeKind, pos2, vec2};

use super::theme::{self, ACCENT, ButtonStyle, Palette};
use super::tip;
use crate::doc::{Doc, Options};

/// Themes to a row in the Theme card.
const PER_ROW: usize = 2;

/// Puts the theme the options name up, and has a backdrop nobody chose
/// follow it. Called every frame; it does nothing while nothing has changed.
///
/// A sculpt keeps its backdrop in its file, and a new scene starts with the
/// built-in theme's. Either is one of the themes' own backdrops unless the
/// user picked a colour of their own (under Look), so a backdrop that is one
/// of those goes with the theme that is up, and any other is left alone.
pub fn apply_theme(ctx: &egui::Context, o: &mut Options) {
    theme::set_theme(ctx, o.theme);
    let now = theme::palette();
    let follows = sculpt_toolkit::palette::stock_backdrop(o.background, o.background_top);
    if follows && (o.background, o.background_top) != (now.backdrop, now.backdrop_top) {
        (o.background, o.background_top) = (now.backdrop, now.backdrop_top);
        ctx.request_repaint();
    }
}

/// The tab's cards.
pub fn page(ui: &mut egui::Ui, doc: &mut Doc, capture: &mut Option<String>) {
    let tips = doc.opts.tooltips;
    theme_card(ui, &mut doc.opts, tips);
    interface(ui, &mut doc.opts, tips);
    super::shortcuts::card(ui, doc, capture);
    performance(ui, doc);
}

fn theme_card(ui: &mut egui::Ui, o: &mut Options, tips: bool) {
    let themes = &theme::THEMES;
    let showing = o.theme.min(themes.len() - 1);
    theme::category(ui, "Theme", Color32::from_rgb(214, 140, 255), themes[showing].name, true, |ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        let w = theme::row_width(ui, PER_ROW, 6.0);
        for (row, pair) in themes.chunks(PER_ROW).enumerate() {
            ui.horizontal(|ui| {
                for (column, palette) in pair.iter().enumerate() {
                    let index = row * PER_ROW + column;
                    let help = format!("{}: the interface in these colours.", palette.name);
                    if tip(swatch(ui, w, palette, index == showing), tips, &help).clicked() {
                        o.theme = index;
                    }
                }
            });
        }
        theme::hint(ui, "The backdrop behind the model goes with the theme, until you pick a colour of your own for it under Look.");
    });
}

/// One theme's tile, painted in that theme's own colours: its name on its
/// card, and a button and its accent beside it.
fn swatch(ui: &mut egui::Ui, width: f32, palette: &Palette, on: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width, 40.0), Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let p = ui.painter();
    p.rect_filled(rect, 6.0, palette.card);
    let chip = |from_right: f32, fill: Color32| {
        let centre = pos2(rect.right() - from_right, rect.center().y);
        p.rect_filled(Rect::from_center_size(centre, vec2(12.0, 20.0)), 3.0, fill);
    };
    chip(32.0, palette.control);
    chip(16.0, palette.accent);
    let face = if on { theme::bold(12.0) } else { theme::font(12.0) };
    p.text(rect.left_center() + vec2(10.0, -1.0), Align2::LEFT_CENTER, palette.name, face, palette.text);
    if on {
        p.rect_stroke(rect, 6.0, Stroke::new(2.0, palette.accent), StrokeKind::Inside);
    } else {
        // Its edge, for a theme whose card is close to the one it lies on.
        p.rect_stroke(rect, 6.0, Stroke::new(1.0, Color32::from_white_alpha(28)), StrokeKind::Inside);
        if response.hovered() {
            p.rect_filled(rect, 6.0, Color32::from_white_alpha(16));
        }
    }
    response
}

/// How large the interface is drawn, and whether it explains itself.
fn interface(ui: &mut egui::Ui, o: &mut Options, tips: bool) {
    theme::category(ui, "Interface", Color32::from_rgb(242, 158, 89), &format!("{:.0}%", o.ui_scale * 100.0), true, |ui| {
        ui.horizontal(|ui| {
            let sizes = [0.75f32, 1.0, 1.25, 1.5, 2.0];
            let w = theme::row_width(ui, sizes.len(), 4.0);
            ui.spacing_mut().item_spacing.x = 4.0;
            for scale in sizes {
                let style = ButtonStyle { active: ((o.ui_scale - scale).abs() < 0.01).then_some(ACCENT()), ..Default::default() };
                if theme::button(ui, w, &format!("{:.0}%", scale * 100.0), style).clicked() {
                    o.ui_scale = scale;
                }
            }
        });
        theme::hint(ui, "Ctrl + = and Ctrl + - also work.");
        let label = if o.tooltips { "Tooltips: On" } else { "Tooltips: Off" };
        tip(theme::checkbox(ui, &mut o.tooltips, label, ACCENT()), tips, "Say what a button does when the pointer rests on it.");
    });
}

fn performance(ui: &mut egui::Ui, doc: &Doc) {
    theme::category(ui, "Performance", Color32::from_rgb(158, 158, 173), &format!("{:.1} ms", doc.stats.frame_ms), false, |ui| {
        let (undo, redo) = doc.history.steps();
        let lines = [
            format!("Frame: {:.2} ms", doc.stats.frame_ms),
            format!("Sculpting: {:.2} ms per frame", doc.stats.sculpt_ms),
            format!("Sent to GPU: {:.0} KB per update", doc.stats.upload_kb),
            format!("History: {undo} undo, {redo} redo, {:.0} MB", doc.history.bytes() as f64 / 1e6),
            doc.stats.adapter.clone(),
        ];
        for line in lines {
            theme::line(ui, &line, theme::HINT());
        }
    });
}
