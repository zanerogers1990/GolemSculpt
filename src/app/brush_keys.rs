//! The [ and ] keys: the brush a step smaller or bigger, and with Shift a step
//! weaker or stronger. Held down, they keep stepping (the key's own repeat),
//! and the toast says the number each step lands on.

use winit::keyboard::KeyCode;

use super::{Drag, Running};
use crate::sculpt::Brush;

/// A step of [ or ] changes the size by this factor: about what a notch of
/// the wheel over the model does, and the same up as down, so ] then [ comes
/// back to where it was.
const SIZE_STEP: f32 = 1.1;

/// A step of Shift + [ or ] goes to the next multiple of this, 5%.
const STRENGTH_STEP: f32 = 0.05;

/// The strength one step from `strength`, up (`up`) or down: the next
/// multiple of [`STRENGTH_STEP`] that way, so an odd 47% steps to 50% or 45%.
/// Never under the slider's 1% or over 100%.
fn stepped(strength: f32, up: bool) -> f32 {
    let steps = strength / STRENGTH_STEP;
    // A hair of slack, so 45% read back as 44.99999% still steps to 50%.
    let next = if up { (steps + 1e-3).floor() + 1.0 } else { (steps - 1e-3).ceil() - 1.0 };
    (next * STRENGTH_STEP).clamp(0.01, 1.0)
}

impl Running {
    /// A press of [ or ], or a repeat of one held down. True if the key was
    /// one of the two, whether or not it could step now: not in the middle of
    /// a stroke or any other drag, which has the size it began with.
    pub(super) fn brush_step_key(&mut self, code: KeyCode) -> bool {
        let up = match code {
            KeyCode::BracketLeft => false,
            KeyCode::BracketRight => true,
            _ => return false,
        };
        // A held key's repeats come straight here, past the question and the
        // clean view that take the first press: they are refused the same.
        let busy = self.input.drag != Drag::None || self.ui.graph.asking() || self.doc.opts.clean_view;
        if busy || self.input.mods.control_key() {
            return true;
        }
        let paint = self.doc.sculptor.effective_brush(false) == Brush::Paint;
        let s = &mut self.doc.sculptor.settings;
        let text = if self.input.mods.shift_key() {
            let bs = s.current_mut();
            bs.strength = stepped(bs.strength, up);
            // The Paint brush's Strength is its Flow, as the panel calls it.
            let name = if paint && !s.mask_mode { "Flow" } else { "Strength" };
            format!("{name} {:.0}%", s.current().strength * 100.0)
        } else {
            let factor = if up { SIZE_STEP } else { 1.0 / SIZE_STEP };
            s.set_size(s.size() * factor, self.view.size.y);
            format!("Size {}", crate::ui::brush_size_shown(s))
        };
        self.toast(text);
        self.redraw = true;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::stepped;

    #[test]
    fn strength_steps_land_on_multiples_of_five_percent() {
        let percent = |v: f32| (v * 100.0).round() as i32;
        assert_eq!(percent(stepped(0.47, true)), 50);
        assert_eq!(percent(stepped(0.47, false)), 45);
        assert_eq!(percent(stepped(0.45, true)), 50, "from a multiple, a whole step");
        assert_eq!(percent(stepped(0.45, false)), 40);
        assert_eq!(percent(stepped(0.01, true)), 5);
        assert_eq!(percent(stepped(0.05, false)), 1, "never under the slider's 1%");
        assert_eq!(percent(stepped(1.0, true)), 100);
        // Twenty steps up from the bottom reach the top, one at a time.
        let mut v = 0.01;
        for _ in 0..20 {
            v = stepped(v, true);
        }
        assert_eq!(percent(v), 100);
    }
}
