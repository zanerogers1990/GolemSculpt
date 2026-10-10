//! Timelapse: a sped-up video of a sculpting session, filmed by a camera of
//! its own, that keeps only the time the model is actually changing.
//!
//! Two things make it short. The activity gate (activity.rs) drops idle time
//! outright, which is what stops an hour at the desk making an hour of
//! video; undo and redo count as idle. The speed then squeezes what is left,
//! which is what stops the twenty minutes of real sculpting making twenty
//! minutes of video. [`Schedule`] is both rules in one place.
//!
//! The camera is in rig.rs, the writing of the file in recorder.rs and
//! mp4.rs, the card in panel.rs. The app's side (drawing each frame and
//! handing it over) is in app/recording.rs.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub mod activity;
pub mod mp4;
mod panel;
pub mod recorder;
pub mod rig;

pub use panel::card;

/// Seconds of sculpting per second of video.
pub const MIN_SPEED: f32 = 1.0;
pub const MAX_SPEED: f32 = 240.0;

/// The video sizes offered: label, tooltip, width, height.
pub const SIZES: [(&str, &str, u32, u32); 6] = [
    ("720p", "1280 x 720", 1280, 720),
    ("1080p", "1920 x 1080", 1920, 1080),
    ("1440p", "2560 x 1440", 2560, 1440),
    ("4K", "3840 x 2160", 3840, 2160),
    ("Square", "1080 x 1080", 1080, 1080),
    ("Vertical", "1080 x 1920", 1080, 1920),
];
pub const FRAME_RATES: [u32; 3] = [24, 30, 60];

/// How the timelapse camera decides where to point.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Framing {
    /// Films the model from the artist's own angle, leaning toward whatever
    /// they are working on. The default: see `rig::Rig::follow_artist`.
    FollowArtist,
    /// A steady turntable orbit around the subject that ignores the artist's
    /// camera.
    Orbit,
}

/// Everything the Timelapse card sets. Plain values with defaults, so a
/// settings file can keep them as they are (a field the file lacks keeps its
/// default). The size and the frame rate are fixed into the video when
/// recording starts; the rest is read each frame and can be changed while
/// recording.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    pub width: u32,
    pub height: u32,
    /// Frames per second of the finished video.
    pub frame_rate: u32,
    /// Seconds of sculpting per second of video. 1 is real time; 8 plays an
    /// eight second stroke back in one.
    pub speed: f32,
    /// How long the camera takes to settle on a new framing, in seconds of
    /// video. 0 snaps; about a second reads as a deliberate camera move.
    pub camera_smoothing: f32,
    pub framing: Framing,
    /// Follow only: how far the shot is pulled from the artist's pivot toward
    /// the spot where the model last changed. 0 is their framing exactly; 1
    /// keeps the brush dead centre and lets the model swing around it.
    pub focus_strength: f32,
    /// Follow only: a multiple of the artist's own viewing distance. Below 1
    /// pushes in on the work; above 1 leaves room for the recentring.
    pub focus_zoom: f32,
    /// Orbit only: degrees per second of video.
    pub orbit_speed: f32,
    /// Orbit only: the camera's height above the subject, in degrees.
    pub orbit_height: f32,
    /// Orbit only: a multiple of the distance at which the subject just fills
    /// the picture. 1 is a tight fit; the default leaves a little air.
    pub orbit_distance: f32,
    /// Orbit only: keep re-framing as the model grows or another object is
    /// picked, instead of holding what the first frame set up.
    pub follow_subject: bool,
    /// How long to keep recording after the last change, in seconds. Long
    /// enough to bridge the gaps between dabs, short enough that walking away
    /// costs less than a second of footage.
    pub idle_grace: f32,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 720,
            frame_rate: 30,
            speed: 8.0,
            camera_smoothing: 0.8,
            framing: Framing::FollowArtist,
            focus_strength: 0.5,
            focus_zoom: 1.0,
            orbit_speed: 9.0,
            orbit_height: 15.0,
            orbit_distance: 1.25,
            follow_subject: true,
            idle_grace: 0.75,
        }
    }
}

impl Options {
    /// These options with every number brought into the range its control
    /// allows: for values read from a file, which may hold anything.
    pub fn in_range(self) -> Options {
        let d = Options::default();
        let fit = |value: f32, low: f32, high: f32, fallback: f32| {
            if value.is_finite() { value.clamp(low, high) } else { fallback }
        };
        Options {
            width: self.width.clamp(320, 8192) & !1,
            height: self.height.clamp(240, 8192) & !1,
            frame_rate: self.frame_rate.clamp(1, 120),
            speed: fit(self.speed, MIN_SPEED, MAX_SPEED, d.speed),
            camera_smoothing: fit(self.camera_smoothing, 0.0, 3.0, d.camera_smoothing),
            framing: self.framing,
            focus_strength: fit(self.focus_strength, 0.0, 1.0, d.focus_strength),
            focus_zoom: fit(self.focus_zoom, 0.5, 2.0, d.focus_zoom),
            orbit_speed: fit(self.orbit_speed, 0.0, 90.0, d.orbit_speed),
            orbit_height: fit(self.orbit_height, -80.0, 80.0, d.orbit_height),
            orbit_distance: fit(self.orbit_distance, 0.5, 4.0, d.orbit_distance),
            follow_subject: self.follow_subject,
            idle_grace: fit(self.idle_grace, 0.0, 5.0, d.idle_grace),
        }
    }

    /// Seconds of sculpting between recorded frames. At speed 1 and 30
    /// frames a second that is every 33 ms of activity; at speed 8, every
    /// 267 ms. Held above a floor so a silly speed cannot ask for a frame on
    /// every frame for ever.
    pub fn capture_interval(&self) -> f32 {
        (self.speed.max(0.01) / self.frame_rate.max(1) as f32).max(1.0 / 240.0)
    }
}

/// Decides, once a frame, whether this frame goes in the video.
#[derive(Default)]
pub struct Schedule {
    /// Seconds spent sculpting since recording began: how much of the
    /// session got through the activity gate.
    sculpting: f32,
    next_at: f32,
}

impl Schedule {
    /// `dt` seconds have passed; `editing` says whether the model is
    /// changing (see `activity`). True when a frame is due.
    pub fn tick(&mut self, dt: f32, editing: bool, interval: f32) -> bool {
        if !editing {
            return false;
        }
        self.sculpting += dt.max(0.0);
        if self.sculpting < self.next_at {
            return false;
        }
        // On from the previous target, not from now. Counting from now adds
        // however long this frame took to every interval, which at low
        // speeds is most of the interval: the video would play too fast.
        self.next_at += interval;
        // But a long stall (a heavy remesh) must not leave a backlog behind
        // it. Picking up at most one interval back means what follows is the
        // next frame or two, not a frame on every frame for as long as the
        // stall lasted.
        self.next_at = self.next_at.max(self.sculpting - interval);
        true
    }

    #[cfg(test)]
    pub fn sculpting_seconds(&self) -> f32 {
        self.sculpting
    }
}

/// What the recorder is doing, for the card.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Idle,
    Recording,
    /// Stop was pressed and the end of the video is still being written.
    Saving,
}

/// What the card's buttons ask the app to do; it picks these up each frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    Start,
    Stop,
    OpenFolder,
}

/// The timelapse as the interface sees it.
#[derive(Default)]
pub struct Timelapse {
    pub options: Options,
    pub phase: Phase,
    /// One line for the card: counts while recording, what came of it after.
    pub status: String,
    pub status_ok: bool,
    pub request: Option<Request>,
    /// The last finished video.
    pub last_output: Option<PathBuf>,
    /// For visual checks: keep the card open and scrolled into view.
    pub pinned: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_spaced_by_the_speed() {
        let slow = Options { speed: 1.0, frame_rate: 30, ..Default::default() };
        assert!((slow.capture_interval() - 1.0 / 30.0).abs() < 1e-6);
        let usual = Options::default();
        assert!((usual.capture_interval() - 8.0 / 30.0).abs() < 1e-6);
        let silly = Options { speed: 0.0, frame_rate: 60, ..Default::default() };
        assert!((silly.capture_interval() - 1.0 / 240.0).abs() < 1e-6);
    }

    #[test]
    fn options_from_a_file_are_brought_into_range() {
        let wild = Options { width: 99_999, height: 7, frame_rate: 0, speed: -4.0, idle_grace: f32::INFINITY, ..Default::default() };
        let fitted = wild.in_range();
        assert_eq!((fitted.width, fitted.height, fitted.frame_rate), (8192, 240, 1));
        assert_eq!((fitted.speed, fitted.idle_grace), (MIN_SPEED, Options::default().idle_grace));
        assert_eq!(Options::default().in_range(), Options::default());
        // A file that only mentions some of them keeps the rest as built in.
        let partial: Options = serde_json::from_str("{ \"speed\": 16.0, \"framing\": \"orbit\" }").unwrap();
        assert_eq!(partial, Options { speed: 16.0, framing: Framing::Orbit, ..Default::default() });
    }

    /// Runs `seconds` of a 60 frames-a-second session and counts the frames
    /// that would be recorded.
    fn run(schedule: &mut Schedule, seconds: f32, editing: bool, interval: f32) -> u32 {
        let frames = (seconds * 60.0).round() as u32;
        (0..frames).filter(|_| schedule.tick(1.0 / 60.0, editing, interval)).count() as u32
    }

    #[test]
    fn only_time_spent_editing_is_recorded_and_it_is_sped_up() {
        let interval = Options::default().capture_interval();
        let mut schedule = Schedule::default();
        // A minute away from the desk: nothing.
        assert_eq!(run(&mut schedule, 60.0, false, interval), 0);
        // Eight seconds of sculpting at 8x is one second of video: 30 frames,
        // give or take the one at the very start.
        let frames = run(&mut schedule, 8.0, true, interval);
        assert!((30..=31).contains(&frames), "{frames} frames");
        assert!((schedule.sculpting_seconds() - 8.0).abs() < 0.01);
        // Idle again, then more work: the count carries on evenly.
        assert_eq!(run(&mut schedule, 5.0, false, interval), 0);
        let more = run(&mut schedule, 8.0, true, interval);
        assert!((29..=31).contains(&more), "{more} more frames");
    }

    #[test]
    fn real_time_keeps_pace_with_a_fast_app() {
        // Speed 1 at 30 frames a second, in an app drawing 60: every other
        // frame, not every frame and not every third.
        let mut schedule = Schedule::default();
        let frames = run(&mut schedule, 10.0, true, 1.0 / 30.0);
        assert!((299..=301).contains(&frames), "{frames} frames");
    }

    #[test]
    fn a_stall_does_not_leave_a_burst_behind_it() {
        let interval = 8.0 / 30.0;
        let mut schedule = Schedule::default();
        assert!(schedule.tick(1.0 / 60.0, true, interval));
        // A remesh holds the app for five seconds.
        assert!(schedule.tick(5.0, true, interval));
        // What follows is a frame or two, not the eighteen the stall "owed".
        let after = run(&mut schedule, 1.0, true, interval);
        assert!((3..=5).contains(&after), "{after} frames in the second after");
    }
}
