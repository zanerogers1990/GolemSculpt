//! How much of a stroke one frame lays down, so that a fast stroke with a big
//! brush on a dense mesh keeps the app answering.
//!
//! A dab on such a mesh can take many milliseconds, and a pen crosses the
//! model faster than the dabs along its path can be laid. A frame used to lay
//! a fixed number of them and DROP the rest of the travel, which left a fast
//! stroke as a row of separate dabs with gaps. Now a frame works on the
//! stroke for a set time ([`Pace`]) and what is left waits its turn: the
//! pointer samples a frame could not afford are kept, in order, and worked
//! off over the frames that follow and at the release. The stroke falls
//! behind the pen and catches up, as it does in ZBrush and Nomad, and comes
//! out the same as if the machine had kept up.
//!
//! The old note against keeping the travel ("banking it would release as a
//! burst of dabs next frame") was about keeping it as a distance: the dabs
//! owed would then all have been laid wherever the brush was a frame later,
//! a burst in one place. What is kept here is the path itself, so every dab
//! lands where it belongs on it, however late.
//!
//! That leaves one case: a stroke the machine cannot keep up with at all.
//! Left alone its backlog grows for as long as the pen moves and the release
//! freezes until it is done. So when there is more waiting than about a
//! second can lay ([`Meter::per_second`]), the dabs of the backlog are spaced
//! wider, each doing as much more as it stands in for (the same correction a
//! wider Spacing setting gets, `Params::dose`): the stroke gets coarser, not
//! shallower, and not later. How much is waiting is a guess from how far
//! apart the waiting samples are on screen (`Stroke::dabs_waiting`): nothing
//! is known of where they land until they are worked through, so a trip off
//! the model and back counts as path to lay, and can coarsen a stroke that is
//! already behind a little sooner than it need be.
//!
//! What a frame does once (bringing the normals up to date, Clay's finishing
//! pass, dynamic topology's share of splits) it goes on doing once a frame
//! while the stroke catches up, the release included, which finishes the
//! stroke a frame's worth at a time. So the tail of a stroke that fell
//! behind is made the way its body was.

use std::time::{Duration, Instant};

/// How a stroke is paced.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Pace {
    /// Every pointer sample is worked through as it arrives, however long
    /// that takes. For strokes nobody is watching land: scripted ones (the
    /// Claude bridge, the demos) and tests, which must come out the same
    /// whatever else the machine is doing.
    #[default]
    Unpaced,
    /// A frame lays this many dabs. A pace with no clock in it, for tests.
    #[cfg_attr(not(test), allow(dead_code))]
    Dabs(usize),
    /// A frame works on the stroke for this long.
    Time(Duration),
}

impl Pace {
    /// A stroke drawn by hand. The window is drawn in step with the screen,
    /// so a frame takes one of its refreshes or two: 20 ms of dabs and the
    /// drawing of a dense model fit in two of a 60 Hz screen's, which keeps
    /// the ring moving thirty times a second while the stroke is behind. (A
    /// budget that fitted in one refresh would leave no more than a few
    /// milliseconds for dabs once the drawing was paid for.)
    pub const LIVE: Pace = Pace::Time(Duration::from_millis(20));
}

/// The share of a second that goes on dabs while a stroke drawn by hand is
/// catching up, until it has been measured: [`Pace::LIVE`]'s 20 ms out of
/// each 33 ms frame. The rest draws the frames.
///
/// It is measured as the stroke goes ([`Meter::new_frame`]), because how long
/// the rest takes is the model's doing. A stroke drawn by hand on a ball of
/// eight million triangles had frames of 70 ms, 27 of them dabs: a share of
/// 0.4, not 0.6. Counting on 0.6, the backlog that was to be a second's work
/// took two seconds to clear after the pen stopped, and a lift with that
/// much waiting held the app for 1.6.
const WORKING_SHARE: f32 = 0.6;

/// The least and most the measured share is taken for. Under a twentieth it
/// is a hiccup and not a pace.
const SHARE_RANGE: (f32, f32) = (0.05, 1.0);

/// Frames in a second, for a pace counted in dabs.
const FRAMES_PER_SECOND: f32 = 60.0;

/// What a stroke has spent of the frame under way, and what its dabs cost.
#[derive(Default)]
pub(super) struct Meter {
    /// Dabs laid since the frame began.
    dabs: usize,
    /// Time spent on the stroke since the frame began, up to the start of
    /// the work under way.
    spent: Duration,
    /// When the work under way began.
    since: Option<Instant>,
    /// Seconds a dab has been taking lately; 0 before the first.
    per_dab: f32,
    /// When the frame under way began, once one has.
    frame_began: Option<Instant>,
    /// The share of a frame that has been going on the stroke lately, in the
    /// frames that gave it all they could; 0 before the first such.
    share: f32,
}

impl Meter {
    /// A frame has ended: the next has its whole share.
    ///
    /// If the frame that ended spent its whole budget on the stroke, how
    /// much of the frame that came to is noted: it is what says how fast a
    /// backlog is really being worked off ([`Self::per_second`]). A frame
    /// that had less to do than it could afford says nothing about that.
    pub fn new_frame(&mut self, pace: Pace) {
        let now = Instant::now();
        if let (Pace::Time(budget), Some(began)) = (pace, self.frame_began) {
            if self.dabs > 0 && self.spent >= budget {
                self.note_share(self.spent, now - began);
            }
        }
        self.frame_began = Some(now);
        (self.dabs, self.spent, self.since) = (0, Duration::ZERO, None);
    }

    /// A frame that lasted `frame` spent `spent` of it on the stroke.
    fn note_share(&mut self, spent: Duration, frame: Duration) {
        if frame.is_zero() {
            return;
        }
        let share = (spent.as_secs_f32() / frame.as_secs_f32()).clamp(SHARE_RANGE.0, SHARE_RANGE.1);
        // Mostly the last few, as with what a dab costs.
        self.share = if self.share > 0.0 { self.share * 0.8 + share * 0.2 } else { share };
    }

    /// Work on the stroke begins; [`Self::stop`] ends it.
    pub fn start(&mut self) {
        self.since = Some(Instant::now());
    }

    pub fn stop(&mut self) {
        if let Some(since) = self.since.take() {
            self.spent += since.elapsed();
        }
    }

    /// Whether the frame can afford another dab. The first of a frame always
    /// can, so a stroke moves on however long a dab takes.
    pub fn has_room(&self, pace: Pace) -> bool {
        match pace {
            Pace::Unpaced => true,
            Pace::Dabs(most) => self.dabs < most.clamp(1, usize::MAX),
            Pace::Time(most) => self.dabs == 0 || self.spent + self.since.map_or(Duration::ZERO, |s| s.elapsed()) < most,
        }
    }

    /// A dab was laid, and took this long.
    pub fn dab(&mut self, took: Duration) {
        self.dabs += 1;
        let took = took.as_secs_f32();
        // Mostly the last few: the cost follows the brush on to denser or
        // sparser parts of the mesh.
        self.per_dab = if self.per_dab > 0.0 { self.per_dab * 0.8 + took * 0.2 } else { took };
    }

    /// Seconds a dab has been taking lately; 0 before the first.
    pub fn per_dab(&self) -> f32 {
        self.per_dab
    }

    /// How many dabs about a second of catching up lays: the most a stroke
    /// may have waiting before its backlog is spaced wider.
    pub fn per_second(&self, pace: Pace) -> f32 {
        match pace {
            Pace::Unpaced => f32::INFINITY,
            Pace::Dabs(most) => most.clamp(1, usize::MAX) as f32 * FRAMES_PER_SECOND,
            Pace::Time(_) if self.per_dab <= 0.0 => f32::INFINITY,
            Pace::Time(_) => self.working_share() / self.per_dab,
        }
    }

    /// The share of a second that goes on dabs while the stroke catches up:
    /// as measured, or [`WORKING_SHARE`] until it has been.
    fn working_share(&self) -> f32 {
        if self.share > 0.0 { self.share } else { WORKING_SHARE }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How fast a backlog is worked off is measured, not assumed: on a model
    /// whose frames take long to draw, less of each second goes on dabs, and
    /// "a second's work" is fewer of them.
    #[test]
    fn the_working_share_is_what_the_frames_show() {
        let ms = Duration::from_millis;
        let mut meter = Meter::default();
        meter.dab(ms(10));
        let assumed = meter.per_second(Pace::LIVE);
        assert!((assumed - 60.0).abs() < 0.01, "0.6 of a second at 10 ms a dab, until a frame has been measured: {assumed}");
        // Frames of 70 ms with 28 ms of dabs in them: 0.4.
        for _ in 0..40 {
            meter.note_share(ms(28), ms(70));
        }
        assert!((meter.per_second(Pace::LIVE) - 40.0).abs() < 0.5, "{}", meter.per_second(Pace::LIVE));
        // It follows the model: lighter frames, a bigger share.
        for _ in 0..40 {
            meter.note_share(ms(20), ms(25));
        }
        assert!((meter.per_second(Pace::LIVE) - 80.0).abs() < 1.0, "{}", meter.per_second(Pace::LIVE));
        // One frame that hung does not make the stroke crawl, and a pace
        // with no clock in it is not touched.
        meter.note_share(ms(20), ms(60_000));
        assert!(meter.per_second(Pace::LIVE) > 60.0);
        assert_eq!(meter.per_second(Pace::Dabs(2)), 120.0);
    }

    /// Only a frame that spent its whole budget on the stroke is measured: a
    /// stroke that is keeping up lays a dab or two and waits for the screen,
    /// which says nothing of how fast it could go.
    #[test]
    fn a_frame_with_time_to_spare_is_not_measured() {
        let mut meter = Meter::default();
        meter.new_frame(Pace::LIVE);
        meter.dab(Duration::from_millis(2));
        meter.spent = Duration::from_millis(2);
        meter.new_frame(Pace::LIVE);
        assert_eq!(meter.share, 0.0, "2 ms of a 20 ms budget");
        meter.dab(Duration::from_millis(12));
        meter.spent = Duration::from_millis(24);
        std::thread::sleep(Duration::from_millis(5));
        meter.new_frame(Pace::LIVE);
        assert_eq!(meter.share, 1.0, "24 ms spent in a frame of about 5: all of it, and no more than all");
        // Unpaced and counted paces never measure.
        let mut plain = Meter::default();
        for pace in [Pace::Unpaced, Pace::Dabs(2)] {
            plain.new_frame(pace);
            plain.dab(Duration::from_millis(30));
            plain.spent = Duration::from_millis(30);
            plain.new_frame(pace);
            assert_eq!(plain.share, 0.0, "{pace:?}");
        }
    }
}
