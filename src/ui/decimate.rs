//! Decimate, at the foot of the Remesh card: the count to come down to, a
//! row of fractions of what the object has now that set it, and the buttons.
//!
//! **Preview** shows the model decimated before anything is kept: the app
//! works the decimation out once, down to the slider's lowest count, and
//! from then on the model is shown at whatever count the slider is dragged
//! to, at once (app/decimate.rs). Apply (Enter) keeps what is shown as one
//! undo step; Cancel (Esc) leaves the model as it was. **Decimate** does it
//! in one go, for the count the slider says.

use egui::Ui;

use super::theme::{self, ACCENT, ButtonStyle};
use super::{fmt_count, rebuild_notes, tip};
use crate::doc::{Action, Doc};

/// What the card shows of a preview. The preview itself is the app's
/// (app/decimate.rs), which sets this every frame. Not kept between runs.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Preview {
    /// A preview is open: being worked out, or showing.
    pub open: bool,
    /// Worked out: any count shows at once.
    pub ready: bool,
    /// The triangles of what is shown, when something is.
    pub shown: Option<usize>,
    /// The fewest triangles it can show.
    pub fewest: usize,
}

/// Two significant figures: 5,200 rather than 5,187.
fn rounded_to_two_figures(value: f32) -> f32 {
    if !(value > 0.0 && value.is_finite()) {
        return 0.0;
    }
    let step = 10f32.powf(value.log10().floor() - 1.0);
    (value / step).round() * step
}

pub fn card(ui: &mut Ui, doc: &mut Doc, tips: bool, actions: &mut Vec<Action>) {
    let range = crate::decimate::TARGET_RANGE;
    let preview = doc.opts.decimate_preview;
    let now = doc.mesh.live_triangle_count();
    let mut log = doc.opts.decimate_target.clamp(*range.start(), *range.end()).log2();
    let percent = |target: f32| if now > 0 { format!(" ({:.0}%)", (target / now as f32 * 100.0).min(999.0)) } else { String::new() };
    let slider = theme::slider(ui, "Target Triangles", &mut log, range.start().log2()..=range.end().log2(), |v| {
        format!("{}{}", fmt_count(v.exp2() as usize), percent(v.exp2()))
    });
    let help = "How many triangles Decimate brings the selected object down to. The number in brackets is what share of the triangles it has now. With the preview open, the model shows it as the slider moves.";
    if tip(slider, tips, help).changed() {
        doc.opts.decimate_target = rounded_to_two_figures(log.exp2()).clamp(*range.start(), *range.end());
    }
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, 4, 4.0);
        ui.spacing_mut().item_spacing.x = 4.0;
        for share in [0.75f32, 0.5, 0.25, 0.1] {
            let label = format!("{:.0}%", share * 100.0);
            let help = format!("Sets Target Triangles to {label} of the triangles the selected object has now.");
            if tip(theme::button(ui, w, &label, ButtonStyle::default()), tips, &help).clicked() {
                doc.opts.decimate_target = rounded_to_two_figures(now as f32 * share).clamp(*range.start(), *range.end());
            }
        }
    });
    if preview.open {
        return previewing(ui, doc, preview, tips, actions);
    }
    let target = doc.opts.decimate_target.round() as usize;
    ui.add_enabled_ui(now > target, |ui| {
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, 2, 6.0);
            let help = "Shows the model decimated before anything is changed. Drag Target Triangles and it follows at once; Apply (Enter) keeps what is shown, Cancel (Esc) leaves the model as it was. On a large model it takes a few seconds to work out first.";
            if tip(theme::button(ui, w, "Preview", ButtonStyle::default()), tips, help).clicked() {
                actions.push(Action::DecimatePreview);
            }
            let style = ButtonStyle { active: Some(ACCENT()), ..Default::default() };
            let help = "Takes triangles off and keeps the shape, for a game model that has to fit a budget (200k to 500k triangles, say). Brings the selected object down to about Target Triangles, taking them from where the surface is flat and keeping them where it bends. A closed model stays closed. With a mirror on, the sides come out mirror images of each other, point for point. Fully masked surface is left as it is, and a softer mask keeps more of its detail the stronger it is; hidden parts are decimated too and stay hidden. It works in the background on a large model, with its progress and a Cancel at the bottom of the view. Undo (Ctrl+Z) brings the triangles back.";
            if tip(theme::button(ui, w, "Decimate", style), tips, help).clicked() {
                actions.push(Action::Decimate);
            }
        });
    });
    if now > 0 && now <= target {
        theme::hint(ui, "The selected object is at or under the target already.");
    } else {
        rebuild_notes::hint(ui, doc, rebuild_notes::Rebuild::Decimate, false);
    }
}

/// The card's foot while a preview is open: what is shown, Apply and Cancel.
fn previewing(ui: &mut Ui, doc: &mut Doc, preview: Preview, tips: bool, actions: &mut Vec<Action>) {
    let target = doc.opts.decimate_target.round() as usize;
    let said = match (preview.ready, preview.shown) {
        (false, _) => "Working out the preview. Its progress is at the bottom of the view.".to_owned(),
        (true, None) => "Showing it in a moment.".to_owned(),
        (true, Some(shown)) if target < preview.fewest => format!(
            "Showing {} triangles: as few as it can go without tearing the surface or touching what is fully masked.",
            fmt_count(shown)
        ),
        (true, Some(shown)) => format!("Showing {} triangles. Drag Target Triangles to see other counts.", fmt_count(shown)),
    };
    theme::hint(ui, &said);
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, 2, 6.0);
        ui.add_enabled_ui(preview.shown.is_some(), |ui| {
            let style = ButtonStyle { active: Some(ACCENT()), badge: "Enter", ..Default::default() };
            if tip(theme::button(ui, w, "Apply", style), tips, "Keep the model as shown. Undo (Ctrl+Z) brings the triangles back.").clicked() {
                actions.push(Action::DecimateApply);
            }
        });
        let style = ButtonStyle { badge: "Esc", ..Default::default() };
        if tip(theme::button(ui, w, "Cancel", style), tips, "Close the preview and leave the model as it was.").clicked() {
            actions.push(Action::DecimateCancel);
        }
    });
}
