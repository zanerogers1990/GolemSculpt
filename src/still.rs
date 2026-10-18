//! Render Picture: the view as it is, saved as one finished PNG at a size of
//! its own, refined over as many samples as asked (render/refine.rs), with
//! the backdrop or without it. Its card in the File panel is here; the
//! drawing and saving are in app/still.rs.
//!
//! The picture is drawn as a recording's are: into a target of its own
//! (`capture::Target`), from the window's camera as it was when Render was
//! pressed, never a grab of the window. A few samples are drawn each frame,
//! so the window keeps answering and the card can show how far it has got.

use egui::Color32;

use crate::doc::Doc;
use crate::ui::theme::{self, ACCENT, ButtonStyle};

/// How many times the window's size a render can be.
pub const SCALES: [u32; 4] = [1, 2, 3, 4];
/// How many samples a render can be refined to.
pub const SAMPLES: [u32; 4] = [16, 64, 256, 1024];
/// The longest side a render is drawn at. Past this one picture's working
/// textures take several gigabytes, and an 8K picture is already more than
/// any store listing or client preview asks for.
pub const MAX_SIDE: u32 = 8192;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// Times the window's size.
    pub scale: u32,
    pub samples: u32,
    /// Leave the backdrop out: the PNG is see-through wherever the model is not.
    pub transparent: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { scale: 2, samples: 256, transparent: false }
    }
}

/// The size of a render `scale` times `window`, brought down (keeping its
/// shape) until its longest side is no more than `MAX_SIDE` and `limit`.
pub fn size_for(window: (u32, u32), scale: u32, limit: u32) -> (u32, u32) {
    let (w, h) = (window.0.max(1) as f32 * scale.max(1) as f32, window.1.max(1) as f32 * scale.max(1) as f32);
    let longest = MAX_SIDE.min(limit).max(64) as f32;
    let shrink = (longest / w.max(h)).min(1.0);
    (((w * shrink).round() as u32).max(1), ((h * shrink).round() as u32).max(1))
}

/// How far a render has got.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Progress {
    pub samples: u32,
    pub total: u32,
    /// Every sample is in and the PNG is being written.
    pub saving: bool,
}

/// What the card's buttons ask the app to do; it picks these up each frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Request {
    Render,
    Cancel,
    OpenFolder,
}

#[derive(Default)]
pub struct Still {
    pub settings: Settings,
    /// Set while a picture is being rendered.
    pub progress: Option<Progress>,
    /// One line for the card: where the last render went, or why it failed.
    pub status: String,
    pub status_ok: bool,
    pub request: Option<Request>,
    /// The window's size and the largest picture side this graphics card
    /// takes, which the app sets each frame, for the card to say what a
    /// render will come to.
    pub window: (u32, u32),
    pub limit: u32,
}

impl Still {
    /// The size a render would be now.
    pub fn size(&self) -> (u32, u32) {
        size_for(self.window, self.settings.scale, self.limit.max(64))
    }
}

/// The Render Picture card of the File panel.
pub fn card(ui: &mut egui::Ui, doc: &mut Doc) {
    let tips = doc.opts.tooltips;
    let tip = |r: egui::Response, text: &str| theme::tip(r, tips, text);
    let still = &mut doc.still;
    let summary = if still.progress.is_some() { "Rendering".to_owned() } else { String::new() };
    theme::category(ui, "Render Picture", Color32::from_rgb(235, 170, 120), &summary, false, |ui| {
        let busy = still.progress.is_some();
        let s = &mut still.settings;
        ui.add_enabled_ui(!busy, |ui| {
            theme::heading(ui, "Size");
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let w = theme::row_width(ui, SCALES.len(), 4.0);
                for scale in SCALES {
                    let style = ButtonStyle { active: (s.scale == scale).then_some(ACCENT()), height: 24.0, ..Default::default() };
                    let button = theme::button(ui, w, &format!("{scale}x"), style);
                    if tip(button, "Times the size of the viewport: the picture has the viewport's shape.").clicked() {
                        s.scale = scale;
                    }
                }
            });
            theme::heading(ui, "Samples");
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let w = theme::row_width(ui, SAMPLES.len(), 4.0);
                for samples in SAMPLES {
                    let style = ButtonStyle { active: (s.samples == samples).then_some(ACCENT()), height: 24.0, ..Default::default() };
                    let button = theme::button(ui, w, &samples.to_string(), style);
                    if tip(button, "How many times the picture is drawn and averaged: more is smoother edges, cleaner soft shadows and glossier gloss, and takes longer.").clicked() {
                        s.samples = samples;
                    }
                }
            });
            tip(
                theme::checkbox(ui, &mut s.transparent, "Transparent Background", ACCENT()),
                "Leave the backdrop out: the PNG is see-through wherever the model is not, ready to put on any background.",
            );
        });
        let (width, height) = still.size();
        theme::hint(ui, &format!("{width} x {height} PNG, {} samples, saved to Pictures\\Sculpt", still.settings.samples));

        let (label, style) = match still.progress {
            Some(_) => ("Cancel", ButtonStyle { active: Some(theme::SUB), ..Default::default() }),
            None => ("Render Picture", ButtonStyle { active: Some(ACCENT()), ..Default::default() }),
        };
        let render = theme::button(ui, ui.available_width(), label, style);
        let what = "Draws the view as it is now at the size above, refined over that many samples, and saves it as a PNG. Lit shading only: a matcap is rendered as it shows.";
        if tip(render, what).clicked() {
            still.request = Some(if busy { Request::Cancel } else { Request::Render });
        }
        if let Some(p) = still.progress {
            let line = if p.saving { "Saving the picture...".to_owned() } else { format!("Rendering {} / {}", p.samples, p.total) };
            theme::line(ui, &line, theme::HINT());
        } else if !still.status.is_empty() {
            let color = if still.status_ok { theme::OK } else { theme::ERROR };
            theme::line(ui, &still.status, color);
        }
        let open = theme::button(ui, ui.available_width(), "Open Pictures Folder", ButtonStyle::default());
        if tip(open, "Opens the folder rendered pictures are saved to.").clicked() {
            still.request = Some(Request::OpenFolder);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_render_keeps_the_window_shape_and_no_side_passes_the_limit() {
        assert_eq!(size_for((1600, 900), 2, 16384), (3200, 1800));
        assert_eq!(size_for((1600, 900), 1, 16384), (1600, 900));
        // 4x a 2560 x 1440 window would be 10240 wide: brought down to 8192.
        let (w, h) = size_for((2560, 1440), 4, 16384);
        assert_eq!(w, MAX_SIDE);
        assert!((w as f32 / h as f32 - 2560.0 / 1440.0).abs() < 0.01, "{w} x {h}");
        // A card that takes less than that.
        assert_eq!(size_for((1600, 900), 4, 4096).0, 4096);
        // Nonsense in, something drawable out.
        let (w, h) = size_for((0, 0), 0, 0);
        assert!(w >= 1 && h >= 1);
    }
}
