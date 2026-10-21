//! The brush panel's Size control, in whichever unit the brush is sized in:
//! screen pixels, or the model's own units with Screen-Space Size off.

use egui::Ui;

use super::theme::{self, ACCENT};
use crate::sculpt::Settings;

/// The window's height in pixels, which the largest on-screen size follows.
fn viewport_height(ui: &Ui) -> f32 {
    ui.ctx().content_rect().height() * ui.ctx().pixels_per_point()
}

/// A size on the model, to three places with no trailing zeros: 0.125, 2.5.
fn plain(size: f32) -> String {
    let text = format!("{size:.3}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// The size as the slider shows it: "120 px", or "0.125" on the model.
pub fn shown(s: &Settings) -> String {
    if s.screen_space_size { format!("{:.0} px", s.size()) } else { plain(s.size()) }
}

pub fn slider(ui: &mut Ui, s: &mut Settings, tips: bool) {
    let height = viewport_height(ui);
    let (min, max) = s.size_range(height);
    let help = "Brush radius: on screen, or on the model with Screen-Space Size off. Hold S and drag, or scroll over the model.";
    if s.screen_space_size {
        let mut px = s.size();
        if theme::tip(theme::slider(ui, "Size", &mut px, min..=max, |v| format!("{v:.0} px")), tips, help).changed() {
            s.set_size(px, height);
        }
    } else {
        // The range runs from a hundredth of the model to eight times it, so
        // the slider is logarithmic: equal travel doubles the size, and the
        // small sizes detail is done with are not squeezed into its first
        // few pixels.
        let mut log = s.size().clamp(min, max).log2();
        let range = min.log2()..=max.log2();
        if theme::tip(theme::slider(ui, "Size", &mut log, range, |v| plain(v.exp2())), tips, help).changed() {
            s.set_size(log.exp2(), height);
        }
    }
}

pub fn toggle(ui: &mut Ui, s: &mut Settings, tips: bool) {
    theme::tip(
        theme::checkbox(ui, &mut s.screen_space_size, "Screen-Space Size", ACCENT()),
        tips,
        "On: the brush stays the same size on screen as you zoom. Off: the brush has a fixed size on the model.",
    );
}
