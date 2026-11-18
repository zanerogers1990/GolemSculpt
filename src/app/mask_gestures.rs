//! Two quick mask gestures while mask painting (M), so the Mask tool need
//! not be brought up for the commonest things done with a mask:
//!
//! - **Shift + drag** draws a box instead of a stroke and masks what it
//!   covers, front and back, as the Mask tool's box does; with the right
//!   button (the pen's barrel button) it unmasks. Ctrl + Shift is not used:
//!   that click is the Face Groups card's. A box over empty space, or a
//!   Shift-tap there, clears the mask.
//!   The box itself is `sculpt_region::QuickBox`.
//! - **A double tap on empty space** inverts the mask. With nothing masked it
//!   frames the model, as it does with any other brush.
//! - **A tap on the model** softens the mask's edge, so a bend through it
//!   eases in instead of kinking; each tap softens more. **An Alt tap** on the
//!   model hardens it again. Only with something masked: with nothing masked a
//!   tap lays a dab as before. The dab a tap's press laid is put back.

use std::time::{Duration, Instant};

use glam::Vec2;
use rayon::prelude::*;
use winit::event::MouseButton;

use super::{Drag, Running};
use crate::doc::{Action, MaskOp};

/// Longest press, and furthest the pointer may wander, that is still a tap.
const TAP_TIME: Duration = Duration::from_millis(350);
const TAP_PX: f32 = 10.0;

/// A press while mask painting that may turn out to be a tap.
pub(super) struct MaskTap {
    at: Instant,
    pos: Vec2,
    /// An Alt tap: hardens rather than softens.
    harden: bool,
    /// Something was masked before the press laid its dab.
    masked_before: bool,
}

impl Running {
    /// Whether this press is a mask box: Shift and either button while mask
    /// painting. Alt with Shift is the camera's and Ctrl with Shift the Face
    /// Groups card's; both are matched first.
    pub(super) fn mask_box_takes_press(&self, button: MouseButton) -> bool {
        let either = matches!(button, MouseButton::Left | MouseButton::Right);
        either && self.input.mods.shift_key() && self.doc.sculptor.settings.mask_mode
    }

    pub(super) fn mask_box_pressed(&mut self, button: MouseButton) -> Drag {
        self.sync_view();
        let at = self.input.pointer - self.view.origin;
        self.input.mask_box = Some(sculpt_region::QuickBox::begin(at, button == MouseButton::Right));
        Drag::Ui
    }

    /// The pointer moved. True when the move was the mask box's.
    pub(super) fn mask_box_moved(&mut self) -> bool {
        let at = self.input.pointer - self.view.origin;
        match &mut self.input.mask_box {
            Some(quick) => {
                quick.drag_to(at);
                true
            }
            None => false,
        }
    }

    /// The button came up: the box lands, as one undo step.
    pub(super) fn mask_box_released(&mut self) {
        let Some(quick) = self.input.mask_box.take() else { return };
        // A context over the scene is had through a tool; the Mask tool's
        // is the natural one, though it is not brought up.
        let Some(index) = self.doc.tools.info_index("mask") else { return };
        self.sync_view();
        let at = self.input.pointer - self.view.origin;
        match self.tools_call(index, None, |_, cx| quick.finish(cx, at)) {
            Some(Ok(text)) => self.status(text, true),
            Some(Err(text)) => self.status(text, false),
            None => {}
        }
        self.redraw = true;
    }

    /// Adds the box being dragged, if one is, to what is drawn over the view.
    pub(super) fn mask_box_draw(&self, view: &sculpt_toolkit::View, draw: &mut sculpt_toolkit::Draw) {
        if let Some(quick) = &self.input.mask_box {
            quick.draw(view, draw);
        }
    }

    /// A double tap on empty space while mask painting with something
    /// masked: the mask is inverted. False when that is not the case, and the
    /// double tap frames the model as usual.
    pub(super) fn mask_double_tap(&mut self) -> bool {
        if !self.doc.sculptor.settings.mask_mode || !self.doc.mesh.mask.iter().any(|&m| m > 0.0) {
            return false;
        }
        self.act(Action::Mask(MaskOp::Invert));
        self.status("Inverted the mask. Double tap empty space again to swap it back.", true);
        true
    }

    /// Notes a press that may be a mask tap: a plain left press that began a
    /// mask stroke, or an Alt press over the model (which began a turn of the
    /// view that will go nowhere).
    pub(super) fn mask_tap_pressed(&mut self, button: MouseButton, pos: Vec2, masked_before: bool) {
        let mods = self.input.mods;
        let plain = !mods.control_key() && !mods.shift_key() && !mods.alt_key();
        let alt_only = mods.alt_key() && !mods.control_key() && !mods.shift_key();
        let qualifies = button == MouseButton::Left
            && self.doc.sculptor.settings.mask_mode
            && match self.input.drag {
                Drag::Sculpt => plain,
                Drag::Orbit => alt_only && self.input.hover.is_some(),
                _ => false,
            };
        self.input.mask_tap = qualifies.then(|| MaskTap { at: Instant::now(), pos, harden: alt_only, masked_before });
    }

    /// Whether anything is masked, asked before a press that may be a mask
    /// tap goes to the brush (a dab would be in the mask after). False, and
    /// not asked, for presses that cannot be one.
    pub(super) fn mask_tap_masked_before(&self, button: MouseButton) -> bool {
        let mods = self.input.mods;
        button == MouseButton::Left
            && self.doc.sculptor.settings.mask_mode
            && !mods.control_key()
            && !mods.shift_key()
            && self.doc.mesh.mask.par_iter().any(|&m| m > 0.0)
    }

    /// The button came up. If the press was a quick tap that stayed put and
    /// something is masked, the mask is softened (or hardened) as one undo
    /// step and the dab the press laid is put back. True if so, and then the
    /// stroke must not be finished.
    pub(super) fn mask_tap_released(&mut self) -> bool {
        let Some(tap) = self.input.mask_tap.take() else { return false };
        if tap.at.elapsed() > TAP_TIME || tap.pos.distance(self.input.pointer) > TAP_PX {
            return false;
        }
        // With nothing masked before the press, a tap is a dab as ever.
        if !tap.masked_before {
            return false;
        }
        if self.input.drag == Drag::Sculpt {
            self.drop_stroke();
        }
        if tap.harden {
            self.act(Action::Mask(MaskOp::Harden));
            self.status("Hardened the mask's edge. Alt+tap again for harder, tap to soften.", true);
        } else {
            self.act(Action::Mask(MaskOp::Soften));
            self.status("Softened the mask's edge. Tap again for softer, Alt+tap to harden.", true);
        }
        self.input.drag = Drag::Ui;
        self.redraw = true;
        true
    }
}
