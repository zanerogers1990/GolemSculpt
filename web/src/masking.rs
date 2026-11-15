//! Masking in the web version: the Masking card's commands (invert, clear,
//! blur, sharpen, grow, shrink, and masks made from the surface's shape),
//! each one undo step, and the desktop's quick mask gestures while mask
//! painting (`app/mask_gestures.rs`):
//!
//! - **A double tap on empty space** inverts the mask. With nothing masked,
//!   or with any other brush, it frames the model.
//! - **A Shift tap on empty space** clears the mask.
//!
//! The commands are the desktop's own (`app.rs`, `Action::Mask`, and
//! `sculpt/cavity.rs`), so a mask blurred here is blurred the same there.

use eframe::egui::{Id, Ui};
use glam::Vec2;

use super::GolemApp;
use super::ui::theme::{self, ButtonStyle};
use super::ui::tip;
use crate::sculpt::cavity::{self, DEFAULT_AMOUNT, Shape};

/// Two presses closer than this in time and place are a double tap, as on
/// the desktop (seconds, physical pixels).
const DOUBLE_TAP: f64 = 0.5;
const DOUBLE_TAP_PX: f32 = 20.0;

/// The mask commands, as the desktop's Masking card has them.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum MaskOp {
    Invert,
    Clear,
    Blur,
    Sharpen,
    Grow,
    Shrink,
}

/// What the gestures remember between presses.
#[derive(Default)]
pub struct Gestures {
    /// The last press on empty space that could be the first of a double tap.
    last_tap: Option<(f64, Vec2)>,
    /// A Shift press on empty space while mask painting, where it landed:
    /// lifted without going anywhere, it clears the mask.
    shift_tap: Option<Vec2>,
}

impl GolemApp {
    /// Runs a mask command as one undo step, and says what it did.
    pub(super) fn mask_op(&mut self, op: MaskOp) {
        let delta = match op {
            MaskOp::Invert => self.mesh.remap_mask(|_, _, m| 1.0 - m),
            MaskOp::Clear => self.mesh.remap_mask(|_, _, _| 0.0),
            MaskOp::Blur => self.mesh.remap_mask(|mesh, v, m| 0.5 * (m + mesh.mask_ring_mean(v))),
            MaskOp::Sharpen => self
                .mesh
                .remap_mask(|mesh, v, m| ((2.0 * m - mesh.mask_ring_mean(v)).clamp(0.0, 1.0) - 0.5) * 1.25 + 0.5),
            MaskOp::Grow => self.mesh.remap_mask(|mesh, v, _| mesh.mask_ring_extreme(v, true)),
            MaskOp::Shrink => self.mesh.remap_mask(|mesh, v, _| mesh.mask_ring_extreme(v, false)),
        };
        let what = match op {
            MaskOp::Invert => "Inverted the mask.",
            MaskOp::Clear => "Cleared the mask.",
            MaskOp::Blur => "Blurred the mask's edge.",
            MaskOp::Sharpen => "Sharpened the mask's edge.",
            MaskOp::Grow => "Grew the mask by one ring of points.",
            MaskOp::Shrink => "Shrank the mask by one ring of points.",
        };
        match delta {
            Some(delta) => {
                self.stroke_done(delta);
                self.status = what.into();
            }
            None => self.status = "The mask is as it was: nothing to change.".into(),
        }
    }

    /// Masks the recesses or the peaks, in place of the mask there was, as
    /// one undo step, and says how much of the surface that came to (the
    /// desktop's `app/mask_shape.rs`).
    fn mask_shape(&mut self, shape: Shape, amount: f32) {
        let delta = cavity::mask(&mut self.mesh, shape, amount);
        let changed = delta.is_some();
        if let Some(delta) = delta {
            self.stroke_done(delta);
        }
        let share = cavity::share_masked(&self.mesh);
        let what = shape.name();
        self.status = if share < 0.0005 {
            let more = if shape == Shape::Recesses { "shallower hollows" } else { "gentler rises" };
            format!("Nothing was masked: the surface has no {what} sharp enough at this Amount. Raise Amount to take in {more}.")
        } else if !changed {
            format!("The {what} are masked already at this Amount.")
        } else {
            let percent = (share * 100.0).round().max(1.0);
            format!("Masked the {what}: about {percent:.0}% of the surface. Invert the mask to work only on them.")
        };
    }

    /// The Masking card of the Mesh tab, laid out as the desktop's.
    pub(super) fn masking_card(&mut self, ui: &mut Ui) {
        let tips = self.ui.tips;
        let mut run = None;
        let mut shape = None;
        theme::category(ui, "Masking", theme::MASK, "", true, |ui| {
            theme::hint(ui, "Masked areas are protected from every brush.");
            let ops = [
                [
                    ("Invert", MaskOp::Invert, "What is masked becomes free and what is free becomes masked. One undo step."),
                    ("Clear", MaskOp::Clear, "Takes the whole mask away. One undo step."),
                ],
                [
                    ("Blur", MaskOp::Blur, "Softens the mask's edge; press again for softer. One undo step."),
                    ("Sharpen", MaskOp::Sharpen, "Makes a soft mask edge crisper; press again for crisper. One undo step."),
                ],
                [
                    ("Grow", MaskOp::Grow, "Spreads the mask out by one ring of points. One undo step."),
                    ("Shrink", MaskOp::Shrink, "Draws the mask in by one ring of points. One undo step."),
                ],
            ];
            for row in ops {
                ui.horizontal(|ui| {
                    let w = theme::row_width(ui, 2, 6.0);
                    for (label, op, help) in row {
                        if tip(theme::button(ui, w, label, ButtonStyle::default()), tips, help).clicked() {
                            run = Some(op);
                        }
                    }
                });
            }

            // Masks made from the surface's shape (the desktop's `ui/masking.rs`).
            // Amount is the card's own to keep between frames: how much the
            // next press takes, not a setting of the model.
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
                for (label, which, help) in buttons {
                    if tip(theme::button(ui, w, label, ButtonStyle::default()), tips, help).clicked() {
                        shape = Some((which, amount));
                    }
                }
            });
            let slider = theme::slider(ui, "Amount", &mut amount, 0.0..=1.0, |v| format!("{:.0}%", v * 100.0));
            let help = "How much of the surface the two buttons take: low for only the sharpest creases or ridges, high for \
                        anything that lies low or stands up at all. Press a button again to see the new amount.";
            if tip(slider, tips, help).changed() {
                ui.data_mut(|d| d.insert_temp(id, amount));
            }
            theme::hint(
                ui,
                "Paint a mask with Mask (M) on the Brush tab. While it is on: double tap empty space to invert the mask, \
                 Shift + tap empty space to clear it.",
            );
        });
        if let Some(op) = run {
            self.mask_op(op);
        }
        if let Some((which, amount)) = shape {
            self.mask_shape(which, amount);
        }
    }

    /// A left press on empty space (no stroke began). A second one soon
    /// after and near the first is a double tap: it inverts the mask while
    /// mask painting with something masked, and otherwise frames the model,
    /// as on the desktop. With Shift held while mask painting, the press is
    /// noted, and lifted where it landed it clears the mask.
    pub(super) fn empty_press(&mut self, px: Vec2, time: f64, shift: bool) {
        if shift {
            self.gestures.last_tap = None;
            self.gestures.shift_tap = self.sculptor.settings.mask_mode.then_some(px);
            return;
        }
        let double = self.gestures.last_tap.is_some_and(|(t, at)| time - t < DOUBLE_TAP && at.distance(px) < DOUBLE_TAP_PX);
        if !double {
            self.gestures.last_tap = Some((time, px));
            return;
        }
        self.gestures.last_tap = None;
        let masked = self.mesh.mask.iter().any(|&m| m > 0.0);
        if self.sculptor.settings.mask_mode && masked {
            self.mask_op(MaskOp::Invert);
            self.status = "Inverted the mask. Double tap empty space again to swap it back.".into();
        } else {
            self.frame_model(true);
        }
    }

    /// The button came up at `px`, after a press that did not sculpt. A
    /// Shift tap that stayed put clears the mask; a drag that went somewhere
    /// is not the first tap of a double tap.
    pub(super) fn empty_release(&mut self, px: Option<Vec2>) {
        let stayed = |at: Vec2| px.is_none_or(|p| p.distance(at) < DOUBLE_TAP_PX);
        if let Some(at) = self.gestures.shift_tap.take() {
            if stayed(at) && self.sculptor.settings.mask_mode {
                self.mask_op(MaskOp::Clear);
            }
        }
        if self.gestures.last_tap.is_some_and(|(_, at)| !stayed(at)) {
            self.gestures.last_tap = None;
        }
    }
}
