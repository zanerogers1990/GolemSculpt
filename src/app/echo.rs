//! A second tap that lands where an interface control was a moment ago.
//!
//! Some controls go away, or move, when they are tapped: the tab that folds
//! a panel jumps to the screen's edge, the button that puts a floating card
//! back takes the card's window with it, a tile of the brush wheel closes
//! the wheel. A pen often taps twice (a bounce, or a double tap from habit),
//! and the second press would land on the model that is now under it as a
//! dab of the brush: an undo step nobody asked for. So a press on the view
//! is ignored when it comes straight after a tap on the interface, in the
//! same place.
//!
//! The limits are the double-click's, in time and in distance, and the time
//! runs from when the pen came up. Nobody starts a real stroke a third of a
//! second after tapping a button, on the very spot of the tap; a stroke
//! anywhere else, or any later, is untouched. Only a tap counts: a slider
//! dragged out over the view and let go there has no echo, so the stroke
//! that follows it is never taken for one.

use std::time::{Duration, Instant};

use glam::Vec2;

/// How soon after the tap a press still counts as its echo.
const WITHIN: Duration = Duration::from_millis(350);
/// And how near to it, in the interface's points.
const REACH: f32 = 12.0;

/// A tap on an interface control: when the pen came up, and where, in the
/// window's pixels.
#[derive(Clone, Copy, Debug)]
pub(super) struct Tap {
    pub at: Instant,
    pub pos: Vec2,
}

impl Tap {
    /// The tap that a press on a control at `pressed` makes when the pen
    /// comes up at `released`, now; nothing if it travelled, which makes it
    /// a drag. Both are in the window's pixels.
    pub(super) fn ending(pressed: Vec2, released: Vec2, per_point: f32) -> Option<Tap> {
        (pressed.distance(released) < REACH * per_point).then(|| Tap { at: Instant::now(), pos: released })
    }

    /// Whether a press at `pos` at `now` is this tap's echo. `per_point` is
    /// how many pixels one of the interface's points is. A position that is
    /// not a number is never an echo: a doubt lets the stroke through.
    pub(super) fn echoed_by(&self, now: Instant, pos: Vec2, per_point: f32) -> bool {
        now.saturating_duration_since(self.at) < WITHIN && self.pos.distance(pos) < REACH * per_point
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_press_is_a_taps_echo_only_at_once_and_on_the_same_spot() {
        let at = Instant::now();
        let tap = Tap { at, pos: Vec2::new(287.0, 240.0) };
        let after = |ms: u64| at + Duration::from_millis(ms);
        assert!(tap.echoed_by(after(0), tap.pos, 1.0));
        assert!(tap.echoed_by(after(120), tap.pos + Vec2::new(3.0, -4.0), 1.0), "a pen does not land twice on one pixel");
        assert!(tap.echoed_by(after(349), tap.pos, 1.0));
        assert!(!tap.echoed_by(after(350), tap.pos, 1.0), "a third of a second on, it is a press of its own");
        assert!(!tap.echoed_by(after(5_000), tap.pos, 1.0));
        assert!(tap.echoed_by(after(100), tap.pos + Vec2::new(11.9, 0.0), 1.0));
        assert!(!tap.echoed_by(after(100), tap.pos + Vec2::new(12.0, 0.0), 1.0), "a stroke begun beside it is a stroke");
        assert!(!tap.echoed_by(after(100), tap.pos + Vec2::new(0.0, -300.0), 1.0));
        // The reach is in the interface's points: at 200% it is twice the pixels.
        assert!(tap.echoed_by(after(100), tap.pos + Vec2::new(23.0, 0.0), 2.0));
        assert!(!tap.echoed_by(after(100), tap.pos + Vec2::new(24.0, 0.0), 2.0));
        // Nonsense is never an echo, and a clock read out of order is no reason to panic.
        assert!(!tap.echoed_by(after(100), Vec2::new(f32::NAN, 240.0), 1.0));
        assert!(!tap.echoed_by(after(100), tap.pos, 0.0));
        assert!(!tap.echoed_by(after(100), tap.pos, f32::NAN));
        let later = Tap { at: after(1_000), pos: tap.pos };
        assert!(later.echoed_by(at, tap.pos, 1.0));
    }

    #[test]
    fn only_a_press_let_go_where_it_began_is_a_tap() {
        let pressed = Vec2::new(900.0, 400.0);
        let tap = Tap::ending(pressed, pressed + Vec2::new(2.0, 1.0), 1.0).expect("a tap");
        assert_eq!(tap.pos, pressed + Vec2::new(2.0, 1.0), "where the pen came up");
        // A slider dragged along, or a card dragged by its title, is no tap.
        assert!(Tap::ending(pressed, pressed + Vec2::new(60.0, 0.0), 1.0).is_none());
        assert!(Tap::ending(pressed, pressed + Vec2::new(12.0, 0.0), 1.0).is_none());
        assert!(Tap::ending(pressed, pressed + Vec2::new(20.0, 0.0), 2.0).is_some(), "in points, as the reach is");
        assert!(Tap::ending(pressed, Vec2::new(f32::NAN, 0.0), 1.0).is_none());
        assert!(Tap::ending(pressed, pressed, 0.0).is_none());
    }
}
