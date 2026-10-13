//! The Masking card's part for masks made from the surface's shape: Mask
//! Recesses, Mask Peaks and their Amount (`sculpt/cavity.rs`).

use egui::{Id, Ui};

use super::theme::{self, ButtonStyle};
use crate::doc::Action;
use crate::sculpt::cavity::{DEFAULT_AMOUNT, Shape};

/// The two buttons and the slider. Amount is the card's own to keep between
/// frames: it is how much the next press takes, not a setting of the model.
pub fn by_shape(ui: &mut Ui, tips: bool, actions: &mut Vec<Action>) {
    theme::heading(ui, "From the Surface");
    let id = Id::new("mask_by_shape_amount");
    let mut amount = ui.data(|d| d.get_temp::<f32>(id)).unwrap_or(DEFAULT_AMOUNT);
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, 2, 6.0);
        let buttons = [
            (
                "Mask Recesses",
                Shape::Recesses,
                "Mask what lies in creases and pits, in place of the mask there is. Then Invert, and the brushes \
                 work only in the wrinkles.",
            ),
            (
                "Mask Peaks",
                Shape::Peaks,
                "Mask what stands proud: ridges, bumps and edges, in place of the mask there is. Then Invert, and \
                 the brushes work only on the high spots.",
            ),
        ];
        for (label, shape, help) in buttons {
            if theme::tip(theme::button(ui, w, label, ButtonStyle::default()), tips, help).clicked() {
                actions.push(Action::MaskShape(shape, amount));
            }
        }
    });
    let slider = theme::slider(ui, "Amount", &mut amount, 0.0..=1.0, |v| format!("{:.0}%", v * 100.0));
    let help = "How much of the surface the two buttons take: low for only the sharpest creases or ridges, high for \
                anything that lies low or stands up at all. Press a button again to see the new amount.";
    if theme::tip(slider, tips, help).changed() {
        ui.data_mut(|d| d.insert_temp(id, amount));
    }
}
