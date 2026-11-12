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

use winit::event::MouseButton;

use super::{Drag, Running};
use crate::doc::{Action, MaskOp};

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
}
