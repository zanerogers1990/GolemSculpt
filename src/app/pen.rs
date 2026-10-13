//! The pen as the window reports it: which touch has the pointer, and what
//! each touch event comes to for the app.
//!
//! A pen arrives as touch events (so does a finger, where the display takes
//! touch): its tip landing, moving and lifting, each with the pressure of
//! that moment, and plain moves while it only hovers. A mouse never arrives
//! this way. What the tablet does not say is left out rather than made up:
//! a pressure of nothing at all comes with no pressure, exactly as a touch
//! from a display that cannot measure pressure does.
//!
//! One touch has the pointer from when it lands to when it lifts: the
//! [`Input::pen_touch`]. Anything another touch does meanwhile is left
//! alone. Without that, the heel of the hand or a second finger landing on
//! a touch display mid-stroke moved the brush to itself, and its lifting
//! ended the stroke under the pen.

use glam::Vec2;
use winit::event::{MouseButton, Touch, TouchPhase};

use super::{Input, Running};
use crate::sculpt::PenMeter;

/// What a touch event comes to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Step {
    /// The tip has landed: a press, opening at this pressure. `None` is a
    /// touch that says nothing of pressure and no pen has yet been seen to:
    /// it draws as a mouse does, at full strength.
    Press(Option<f32>),
    /// The tip that has the pointer has moved, reporting this pressure.
    Move(Option<f32>),
    /// It has lifted.
    Lift,
    /// A pen moving over the tablet without touching it: the pointer goes
    /// with it and nothing is drawn.
    Hover,
    /// Another touch while one has the pointer, or the end of a touch that
    /// never had it: nothing to do with what is being drawn.
    Ignore,
}

/// Takes a touch event in: notes it for the pen readout, keeps track of
/// which touch has the pointer, and says what the app is to do about it.
pub(super) fn touched(input: &mut Input, meter: &mut PenMeter, touch: &Touch) -> Step {
    let force = touch.force.map(|force| force.normalized() as f32);
    let ours = input.pen_touch == Some(touch.id);
    if input.pen_touch.is_some() && !ours {
        return Step::Ignore;
    }
    input.inside = true;
    match touch.phase {
        // A start from the touch that already has the pointer is one whose
        // lift went missing: it carries on as a move.
        TouchPhase::Started if !ours => {
            input.pointer = Vec2::new(touch.location.x as f32, touch.location.y as f32);
            (input.pen_touch, input.pen_down) = (Some(touch.id), true);
            // A touch too light to measure still opens a pen's stroke.
            input.pen_pressure = meter.down(force);
            Step::Press(input.pen_pressure)
        }
        TouchPhase::Started | TouchPhase::Moved if ours => {
            input.pen_pressure = force;
            meter.moved(force);
            Step::Move(force)
        }
        TouchPhase::Started | TouchPhase::Moved => {
            input.pen_pressure = force;
            Step::Hover
        }
        TouchPhase::Ended | TouchPhase::Cancelled if ours => {
            meter.lifted();
            input.pen_touch = None;
            Step::Lift
        }
        TouchPhase::Ended | TouchPhase::Cancelled => Step::Ignore,
    }
}

impl Running {
    /// A touch event from the window: the pen, or a finger.
    pub(super) fn on_touch(&mut self, touch: &Touch) {
        let pos = Vec2::new(touch.location.x as f32, touch.location.y as f32);
        match touched(&mut self.input, &mut self.doc.sculptor.pen_meter, touch) {
            Step::Press(pressure) => self.pointer_pressed(MouseButton::Left, pressure),
            Step::Move(pressure) => self.pointer_moved(pos, pressure),
            Step::Hover => self.pointer_moved(pos, None),
            Step::Lift => {
                self.pointer_moved(pos, None);
                self.pointer_released(MouseButton::Left);
                self.input.pen_down = false;
            }
            Step::Ignore => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use winit::dpi::PhysicalPosition;
    use winit::event::{DeviceId, Force};

    use super::*;

    const PEN: u64 = 7;
    const PALM: u64 = 9;

    fn touch(phase: TouchPhase, id: u64, at: (f64, f64), force: Option<f64>) -> Touch {
        Touch {
            device_id: DeviceId::dummy(),
            phase,
            location: PhysicalPosition::new(at.0, at.1),
            force: force.map(Force::Normalized),
            id,
        }
    }

    /// The events of one pen stroke as the window sends them, taken in, with
    /// what each came to.
    fn feed(input: &mut Input, meter: &mut PenMeter, events: &[Touch]) -> Vec<Step> {
        events.iter().map(|event| touched(input, meter, event)).collect()
    }

    #[test]
    fn a_touch_that_says_nothing_of_pressure_draws_as_a_mouse_until_a_pen_has_been_seen() {
        let (mut input, mut meter) = (Input::default(), PenMeter::default());
        // No pen seen this session: a finger on a touch display, or a pen's
        // first touch so light the tablet measured nothing.
        let steps = feed(&mut input, &mut meter, &[touch(TouchPhase::Started, PEN, (400.0, 300.0), None)]);
        assert_eq!(steps, [Step::Press(None)], "it opens as a mouse would: full strength, full size");
        assert_eq!((input.pointer, input.pen_down, input.pen_touch), (Vec2::new(400.0, 300.0), true, Some(PEN)));
        assert_eq!(meter, PenMeter { seen: false, now: None });

        // Pressure arrives mid-stroke: it is a pen, and is from here on.
        let steps = feed(&mut input, &mut meter, &[touch(TouchPhase::Moved, PEN, (410.0, 300.0), Some(0.4))]);
        assert_eq!(steps, [Step::Move(Some(0.4))]);
        assert_eq!((meter.seen, meter.now, input.pen_pressure), (true, Some(0.4), Some(0.4)));

        // The lift.
        let steps = feed(&mut input, &mut meter, &[touch(TouchPhase::Ended, PEN, (420.0, 300.0), None)]);
        assert_eq!(steps, [Step::Lift]);
        assert_eq!((meter.now, input.pen_touch), (None, None), "the readout shows the tip up, and the pointer is free");

        // Now that a pen has been seen, a touch with no pressure is a pen
        // pressing too lightly to measure, and opens at none: not the blob
        // at full strength a mouse's press would be.
        let steps = feed(&mut input, &mut meter, &[touch(TouchPhase::Started, PEN, (500.0, 300.0), None)]);
        assert_eq!(steps, [Step::Press(Some(0.0))]);
        assert_eq!(input.pen_pressure, Some(0.0));
        // One with pressure opens at its own.
        feed(&mut input, &mut meter, &[touch(TouchPhase::Cancelled, PEN, (500.0, 300.0), None)]);
        let steps = feed(&mut input, &mut meter, &[touch(TouchPhase::Started, PEN, (500.0, 300.0), Some(0.25))]);
        assert_eq!(steps, [Step::Press(Some(0.25))]);
    }

    #[test]
    fn a_second_touch_mid_stroke_neither_takes_the_stroke_nor_ends_it() {
        let (mut input, mut meter) = (Input::default(), PenMeter::default());
        feed(&mut input, &mut meter, &[touch(TouchPhase::Started, PEN, (400.0, 300.0), Some(0.5))]);
        let palm = [
            touch(TouchPhase::Started, PALM, (900.0, 700.0), Some(1.0)),
            touch(TouchPhase::Moved, PALM, (905.0, 702.0), Some(1.0)),
            touch(TouchPhase::Ended, PALM, (905.0, 702.0), None),
        ];
        let steps = feed(&mut input, &mut meter, &palm);
        assert_eq!(steps, [Step::Ignore; 3]);
        assert_eq!(input.pointer, Vec2::new(400.0, 300.0), "the brush did not jump to the other touch");
        assert_eq!((input.pen_touch, input.pen_down), (Some(PEN), true), "and the pen still has the stroke");
        assert_eq!((meter.now, input.pen_pressure), (Some(0.5), Some(0.5)), "at the pen's own pressure, not the palm's");
        // The pen carries on and ends its own stroke.
        let steps = feed(
            &mut input,
            &mut meter,
            &[touch(TouchPhase::Moved, PEN, (430.0, 310.0), Some(0.6)), touch(TouchPhase::Ended, PEN, (430.0, 310.0), None)],
        );
        assert_eq!(steps, [Step::Move(Some(0.6)), Step::Lift]);
        // A touch still down from before lifts later: it has nothing to end.
        assert_eq!(feed(&mut input, &mut meter, &[touch(TouchPhase::Ended, PALM, (0.0, 0.0), None)]), [Step::Ignore]);
    }

    #[test]
    fn a_hovering_pen_moves_the_pointer_and_presses_nothing() {
        let (mut input, mut meter) = (Input::default(), PenMeter::default());
        let hover = [touch(TouchPhase::Moved, PEN, (300.0, 200.0), None), touch(TouchPhase::Moved, PEN, (310.0, 205.0), None)];
        assert_eq!(feed(&mut input, &mut meter, &hover), [Step::Hover; 2]);
        assert!(input.inside && !input.pen_down && input.pen_touch.is_none());
        assert_eq!(meter, PenMeter::default(), "hovering is not pressure: the readout still waits for a touch");
        // Then it lands, as the same touch.
        assert_eq!(feed(&mut input, &mut meter, &[touch(TouchPhase::Started, PEN, (310.0, 205.0), Some(0.3))]), [Step::Press(Some(0.3))]);
    }

    #[test]
    fn a_start_whose_lift_went_missing_carries_the_stroke_on() {
        let (mut input, mut meter) = (Input::default(), PenMeter::default());
        let events = [
            touch(TouchPhase::Started, PEN, (400.0, 300.0), Some(0.5)),
            touch(TouchPhase::Started, PEN, (410.0, 300.0), Some(0.6)),
        ];
        assert_eq!(feed(&mut input, &mut meter, &events), [Step::Press(Some(0.5)), Step::Move(Some(0.6))]);
        assert_eq!(input.pointer, Vec2::new(400.0, 300.0), "where the pointer goes is the move's to say, as for any move");
    }
}
