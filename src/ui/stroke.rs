//! The stroke's controls on the Brush tab, in two places on purpose.
//!
//! How the brush in hand lays its dabs and answers the pen is that brush's
//! own (`sculpt::BrushSettings`), and sits in its settings card with the
//! rest of what is its own: [`brush_section`]. What holds whichever brush
//! is in hand, the hand's side of things, is the Stroke card: [`card`], with
//! the readout that shows whether the pen's pressure is arriving at all.

use egui::{Align2, Sense, Ui, vec2};

use super::theme::{self, ACCENT};
use crate::sculpt::{BrushSettings, PenMeter, Sculptor, Settings};

fn percent(v: f32) -> String {
    format!("{:.0}%", v * 100.0)
}

/// Pressure -> Strength for the brush in hand. Mask painting shows it too:
/// a mask is painted at that brush's Strength, pressure and all.
pub fn pressure_strength(ui: &mut Ui, bs: &mut BrushSettings, tips: bool) {
    theme::tip(
        theme::slider(ui, "Pressure \u{2192} Strength", &mut bs.pressure_strength, BrushSettings::PRESSURE, percent),
        tips,
        "How much weaker the brush gets as you press more lightly. At 100% a feather touch does next to nothing; \
         at 0% pressure does not change the strength at all. Full pressure is always the Strength set above, and \
         so is a mouse. (A brush with Accumulate on takes half as much notice of pressure, which keeps its \
         build-up even.)",
    );
}

/// The part of the brush's own settings that shapes its stroke: how far
/// apart its dabs are, how they are scattered, and what the pen's pressure
/// does to it. Not shown for the brushes that lay no dabs.
pub fn brush_section(ui: &mut Ui, s: &mut Settings, tips: bool) {
    if s.mask_mode || !s.brush.lays_dabs() {
        return;
    }
    let bs = s.current_mut();
    theme::heading(ui, "Dabs");
    // A slider of proportions: the same travel doubles the spacing anywhere
    // along it, so the tight end, where a few percent matter, has room.
    let range = BrushSettings::SPACING;
    let mut log = bs.spacing.clamp(*range.start(), *range.end()).ln();
    let slider = theme::slider(ui, "Spacing", &mut log, range.start().ln()..=range.end().ln(), |v| percent(v.exp()));
    let help = "How far apart the brush lays its dabs, as a share of its size. Low is a smooth stroke; higher shows \
                the dabs; from about 100% they stand apart, which is how to lay an alpha as separate prints. It \
                changes how smooth the stroke is, not how much it builds.";
    if theme::tip(slider, tips, help).changed() {
        // Whole percents, as the slider shows them.
        bs.spacing = ((log.exp() * 100.0).round() / 100.0).clamp(*range.start(), *range.end());
    }
    theme::tip(
        theme::slider(ui, "Scatter", &mut bs.scatter, BrushSettings::SCATTER, percent),
        tips,
        "Throws each dab off the line of the stroke, any way along the surface, by a random distance up to this \
         share of the brush's size. 0 is off.",
    );
    theme::tip(
        theme::slider(ui, "Size Jitter", &mut bs.size_jitter, BrushSettings::SIZE_JITTER, percent),
        tips,
        "Makes each dab smaller than the brush by a random amount, up to this much. 0 is off.",
    );
    theme::heading(ui, "Pen Pressure");
    theme::tip(
        theme::slider(ui, "Pressure \u{2192} Size", &mut bs.pressure_size, BrushSettings::PRESSURE, percent),
        tips,
        "How much smaller the brush gets as you press more lightly, so a stroke can swell and taper. At 100% the \
         lightest touch is a fine point; full pressure is always the size of the ring. 0 is off: pressure does not \
         change the size. A mouse is always full size.",
    );
    pressure_strength(ui, bs, tips);
}

/// The Stroke card: what holds for every brush.
pub fn card(ui: &mut Ui, sculptor: &mut Sculptor, tips: bool) {
    let s = &mut sculptor.settings;
    theme::tip(
        theme::checkbox(ui, &mut s.build_up_on_hold, "Build Up on Hold", ACCENT()),
        tips,
        "Keep adding while the pointer rests in one place.",
    );
    theme::tip(
        theme::checkbox(ui, &mut s.lazy_mouse, "Lazy Mouse", ACCENT()),
        tips,
        "The brush trails the pointer on a short rope, steadying long curves.",
    );
    if s.lazy_mouse {
        theme::slider(ui, "Lazy Radius", &mut s.lazy_radius, 1.0..=150.0, |v| format!("{v:.0} px"));
        theme::slider(ui, "Lazy Smoothing", &mut s.lazy_strength, 0.05..=1.0, |v| format!("{v:.2}"));
    }
    theme::heading(ui, "Pen");
    theme::tip(
        theme::slider(ui, "Pressure Curve", &mut s.pressure_curve, 0.5..=3.0, |v| format!("{v:.2}")),
        tips,
        "How the pen answers your hand, for every brush. Higher needs a firmer press before pressure counts, \
         which steadies light strokes; lower answers to the lightest touch. What pressure then changes (size, \
         strength, and by how much) is set for each brush, in its own settings.",
    );
    pen_readout(ui, &sculptor.pen_meter);
}

/// Whether the pen's pressure is reaching the app, and what it is now.
fn pen_readout(ui: &mut Ui, pen: &PenMeter) {
    if !pen.seen {
        return theme::hint(
            ui,
            "No pen pressure has arrived yet since GolemSculpt opened. Press the pen on the tablet: this line turns \
             into a live bar. If it stays, tick \"Enable Windows Ink\" in the Huion tablet driver, or the pen works \
             as a mouse: full strength, full size.",
        );
    }
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 4.0, theme::FIELD());
    let text = match pen.now {
        Some(pressure) => {
            let mut filled = rect;
            filled.set_width(rect.width() * pressure.clamp(0.0, 1.0));
            p.rect_filled(filled, 4.0, ACCENT());
            format!("Pen pressure now: {}", percent(pressure))
        }
        None => "Pen pressure: the pen is up".to_owned(),
    };
    p.text(rect.center(), Align2::CENTER_CENTER, text, theme::font(11.0), theme::TEXT());
}
