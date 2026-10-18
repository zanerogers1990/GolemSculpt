//! Turntable: spins the view around the model at a speed you set, looks
//! after the clean view's hint and resting pointer, and holds the arithmetic
//! of a 360 loop. Its card in the Scene panel is here too.
//!
//! The camera orbits; the model never moves. Turning the objects themselves
//! would write their placement (undo, instances, SSphere rigs and saved files
//! would all see it), where orbiting is only a change of view. It also looks
//! right: the lighting presets follow the camera and matcaps are fixed to the
//! view, so the model reads as turning under fixed studio lights.
//!
//! The 360 loop takes the view as it is for frame 0 and covers exactly one
//! turn at the turntable's speed: N = round(360 / speed x frames per second)
//! frames, frame i at the starting angle plus 360 x i / N. Frame N would be
//! frame 0 again, so it is left out and playback wraps round with one
//! ordinary step: a seamless loop by construction, not by trimming. In the
//! video that is N pictures at 1 / fps seconds each, so the frames add up to
//! exactly N / fps seconds and the last picture ends where the first begins
//! again. The app side of recording one is in app/recording.rs, the writing in
//! capture.rs (pictures) and capture/video.rs (the MP4).

use egui::{Align2, Color32, Id, Order, Sense, Stroke, pos2, vec2};
use serde::{Deserialize, Serialize};

use crate::capture::LoopFormat;
use crate::doc::Doc;
use crate::timelapse::mp4;
use crate::ui::theme::{self, ACCENT, ButtonStyle};

/// Degrees per second.
pub const MIN_SPEED: f32 = 5.0;
pub const MAX_SPEED: f32 = 180.0;
/// Degrees per second per second: how quickly a spin eases in, out and
/// round to the other direction. A hard start reads as a glitch; about a
/// third of a second to full speed reads as a turntable motor.
const SPIN_ACCELERATION: f32 = 120.0;

/// What the clean view says for a moment after it comes up; without it there
/// would be no visible way back out of a screen with no interface on it.
pub const HINT: &str = "Tab  show UI     O  spin     Esc  exit";
const HINT_SECONDS: f64 = 2.5;
const HINT_FADE_SECONDS: f64 = 0.5;
/// In the clean view the pointer disappears after resting this long, and
/// comes back the moment it moves.
pub const POINTER_REST_SECONDS: f32 = 1.5;

/// The picture sizes a loop is offered at: 1080 on the short side for the
/// three shapes people post turntables in (landscape video, square post,
/// vertical reel), and 4K landscape.
pub const LOOP_SIZES: [(&str, u32, u32); 4] =
    [("16:9", 1920, 1080), ("1:1", 1080, 1080), ("9:16", 1080, 1920), ("4K", 3840, 2160)];
pub const FRAME_RATES: [u32; 3] = [24, 30, 60];
/// Samples each picture of a loop can be refined to (render/refine.rs): 1
/// is drawn once, as the viewport is while it moves.
pub const LOOP_SAMPLES: [u32; 3] = [1, 16, 64];

/// What the Turntable card sets. Plain values with defaults, so a settings
/// file can keep them as they are (a field the file lacks keeps its default).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Degrees per second, always positive; `reverse` gives the direction.
    pub speed: f32,
    /// Off, the view goes round to its own right, so the model appears to
    /// turn the way a potter's wheel is usually shown.
    pub reverse: bool,
    pub loop_width: u32,
    pub loop_height: u32,
    /// Frames per second of a recorded loop.
    pub loop_fps: u32,
    /// A video, or numbered pictures.
    pub loop_format: LoopFormat,
    /// Samples each picture is refined to: one of [`LOOP_SAMPLES`].
    pub loop_samples: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            speed: 30.0,
            reverse: false,
            loop_width: 1920,
            loop_height: 1080,
            loop_fps: 30,
            loop_format: LoopFormat::Video,
            loop_samples: 1,
        }
    }
}

impl Settings {
    /// These settings with every number brought into the range its control
    /// allows: for values read from a file, which may hold anything.
    pub fn in_range(self) -> Settings {
        let defaults = Settings::default();
        let speed = if self.speed.is_finite() { self.speed.clamp(MIN_SPEED, MAX_SPEED) } else { defaults.speed };
        Settings {
            speed,
            reverse: self.reverse,
            loop_width: self.loop_width.clamp(64, 8192),
            loop_height: self.loop_height.clamp(64, 8192),
            loop_fps: self.loop_fps.clamp(1, 120),
            loop_format: self.loop_format,
            loop_samples: self.samples(),
        }
    }

    /// How long a recorded loop plays: its frames at its frame rate. The
    /// turn takes 360 / speed seconds; the frames are a whole number, so the
    /// loop is within half a frame of that.
    pub fn loop_seconds(&self) -> f32 {
        loop_seconds(self.loop_frames(), self.loop_fps)
    }

    /// What Record will make, in this build: the chosen format, or pictures
    /// where there is no video encoder.
    pub fn loop_output(&self) -> LoopFormat {
        self.loop_format.available()
    }

    /// Which way yaw runs: negative moves the camera to its own right.
    pub fn direction(&self) -> f32 {
        if self.reverse { 1.0 } else { -1.0 }
    }

    /// Samples each picture of a loop is refined to: the one of
    /// `LOOP_SAMPLES` nearest what was set.
    pub fn samples(&self) -> u32 {
        LOOP_SAMPLES.into_iter().min_by_key(|&n| n.abs_diff(self.loop_samples)).unwrap_or(1)
    }

    /// Frames in one turn at this speed and frame rate.
    pub fn loop_frames(&self) -> u32 {
        loop_frame_count(self.speed, self.loop_fps)
    }
}

/// Frames in one turn: N = round(360 / speed x fps), and never fewer than two.
pub fn loop_frame_count(speed: f32, fps: u32) -> u32 {
    let turn_seconds = 360.0 / speed.clamp(MIN_SPEED, MAX_SPEED);
    (turn_seconds * fps.max(1) as f32).round().max(2.0) as u32
}

/// How long `frames` pictures play at `fps`, in seconds.
pub fn loop_seconds(frames: u32, fps: u32) -> f32 {
    frames as f32 / fps.max(1) as f32
}

/// The yaw of frame `frame` in a loop of `total` frames that starts at
/// `start`. An exact fraction of a turn from the start, never steps added
/// up, so the last frame lands exactly one step short of the first however
/// many frames there are.
pub fn loop_yaw(start: f32, direction: f32, frame: u32, total: u32) -> f32 {
    start + direction * 360.0 * frame as f32 / total.max(1) as f32
}

/// The spin's motor: on or off, and how fast it is turning right now.
#[derive(Default)]
pub struct Spin {
    pub on: bool,
    /// Signed degrees per second.
    rate: f32,
}

impl Spin {
    /// Degrees to turn the view by over `dt` seconds. While `yielding` (the
    /// user is orbiting by hand, or an axis snap is running) the spin lets go
    /// of the view, and picks up from wherever it is left.
    pub fn step(&mut self, settings: &Settings, dt: f32, yielding: bool) -> f32 {
        if yielding {
            self.rate = 0.0;
            return 0.0;
        }
        let target = if self.on { settings.direction() * settings.speed.clamp(MIN_SPEED, MAX_SPEED) } else { 0.0 };
        let step = SPIN_ACCELERATION * dt.max(0.0);
        self.rate += (target - self.rate).clamp(-step, step);
        self.rate * dt
    }

    /// Whether the view is still turning (it eases out after `on` goes off).
    pub fn moving(&self) -> bool {
        self.on || self.rate != 0.0
    }

    /// Stops dead, for when something else takes the view over.
    pub fn halt(&mut self) {
        self.on = false;
        self.rate = 0.0;
    }
}

/// How far a loop recording has got, for the card and the progress bar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Progress {
    pub written: u32,
    pub total: u32,
    /// The loop is going into a video, which takes a moment to close once
    /// every picture is in it.
    pub video: bool,
}

/// What the card's buttons ask the app to do; it picks these up each frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Request {
    RecordLoop,
    CancelLoop,
    OpenFolder,
}

#[derive(Default)]
pub struct Turntable {
    pub settings: Settings,
    pub spin: Spin,
    /// Set while a loop is being written.
    pub recording: Option<Progress>,
    /// One line for the card: what the last recording came to.
    pub status: String,
    pub status_ok: bool,
    pub request: Option<Request>,
    /// For visual checks: keep the card open and scrolled into view.
    pub pinned: bool,
    /// The clean view was just entered: the hint starts on the next frame.
    hint_pending: bool,
    hint_since: Option<f64>,
}

impl Turntable {
    /// How strongly to show the clean view's hint at `time`: 1 at first,
    /// fading out over its last half second, 0 once it is gone.
    pub fn hint_alpha(&mut self, time: f64) -> f32 {
        if std::mem::take(&mut self.hint_pending) {
            self.hint_since = Some(time);
        }
        let Some(since) = self.hint_since else { return 0.0 };
        let left = HINT_SECONDS - (time - since);
        if left <= 0.0 || self.recording.is_some() {
            self.hint_since = None;
            return 0.0;
        }
        (left / HINT_FADE_SECONDS).min(1.0) as f32
    }

    fn show_hint(&mut self) {
        self.hint_pending = true;
    }

    fn hide_hint(&mut self) {
        self.hint_pending = false;
        self.hint_since = None;
    }
}

/// Puts the clean view up or takes it down. Up, the model is shown with
/// nothing but its lighting and background: no panels, gizmo, guides or brush
/// ring, and no strokes or bare-key shortcuts either.
pub fn set_clean_view(doc: &mut Doc, on: bool) {
    // A loop being recorded owns the clean view until it finishes.
    if doc.turntable.recording.is_some() && !on {
        return;
    }
    if doc.opts.clean_view == on {
        return;
    }
    doc.opts.clean_view = on;
    if on {
        doc.turntable.show_hint();
    } else {
        doc.turntable.hide_hint();
    }
}

/// The Turntable card of the Scene panel. Everything on it is also on a key
/// (O spins, Tab hides the panels, Esc leaves the clean view or cancels a
/// recording): the card is for setting it up, the keys for using it.
pub fn card(ui: &mut egui::Ui, doc: &mut Doc) {
    let tips = doc.opts.tooltips;
    let tip = |r: egui::Response, text: &str| theme::tip(r, tips, text);
    let summary = if doc.turntable.recording.is_some() {
        "Recording".to_owned()
    } else if doc.turntable.spin.on {
        "Spinning".to_owned()
    } else {
        format!("{:.0}\u{b0}/s", doc.turntable.settings.speed)
    };
    if doc.turntable.pinned {
        theme::open_category(ui, "Turntable");
        ui.scroll_to_cursor_animation(Some(egui::Align::TOP), egui::style::ScrollAnimation::none());
    }
    theme::category(ui, "Turntable", Color32::from_rgb(120, 205, 190), &summary, false, |ui| {
        let recording = doc.turntable.recording;
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, 2, 6.0);
            let on = doc.turntable.spin.on;
            let style = ButtonStyle { active: on.then_some(ACCENT()), badge: "O", ..Default::default() };
            let spin = theme::button(ui, w, if on { "Stop Spin" } else { "Spin" }, style);
            if tip(spin, "Turns the view around the model, like a turntable. Alt + drag still orbits while it spins, and the spin carries on from wherever you leave it.").clicked()
                && recording.is_none()
            {
                doc.turntable.spin.on = !on;
            }
            let style = ButtonStyle { badge: "Tab", ..Default::default() };
            let hide = theme::button(ui, w, "Hide UI", style);
            if tip(hide, "Hides every panel, gizmo and guide so the model is shown with just its lighting and background. Tab or Esc brings the panels back. Sculpting is paused while they are hidden.").clicked() {
                set_clean_view(doc, true);
            }
        });

        let s = &mut doc.turntable.settings;
        let speed = theme::slider(ui, "Speed", &mut s.speed, MIN_SPEED..=MAX_SPEED, |v| {
            format!("{v:.0}\u{b0}/s   {:.1} s a turn", 360.0 / v.max(MIN_SPEED))
        });
        if tip(speed, "Degrees per second. Also sets how long a recorded 360 loop is.").changed() {
            s.speed = s.speed.round();
        }
        tip(theme::checkbox(ui, &mut s.reverse, "Reverse Direction", ACCENT()), "Spins the other way.");

        theme::heading(ui, "360 Loop");
        ui.add_enabled_ui(recording.is_none(), |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let w = theme::row_width(ui, LOOP_SIZES.len(), 4.0);
                for (label, width, height) in LOOP_SIZES {
                    let chosen = (s.loop_width, s.loop_height) == (width, height);
                    let style = ButtonStyle { active: chosen.then_some(ACCENT()), height: 24.0, ..Default::default() };
                    if tip(theme::button(ui, w, label, style), &format!("{width} x {height}")).clicked() {
                        (s.loop_width, s.loop_height) = (width, height);
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let w = theme::row_width(ui, FRAME_RATES.len(), 4.0);
                for fps in FRAME_RATES {
                    let style = ButtonStyle { active: (s.loop_fps == fps).then_some(ACCENT()), height: 24.0, ..Default::default() };
                    let button = theme::button(ui, w, &format!("{fps} fps"), style);
                    if tip(button, &format!("{fps} frames per second in the finished loop.")).clicked() {
                        s.loop_fps = fps;
                    }
                }
            });
            // Samples per picture: each refined as the viewport is when still.
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let w = theme::row_width(ui, LOOP_SAMPLES.len(), 4.0);
                for samples in LOOP_SAMPLES {
                    let active = (s.samples() == samples).then_some(ACCENT());
                    let style = ButtonStyle { active, height: 24.0, ..Default::default() };
                    let label = if samples == 1 { "1 sample".to_owned() } else { format!("{samples} samples") };
                    let what = "Samples per picture: more draws each one several times and averages them, for smooth edges and clean soft shadows. Slower to record.";
                    if tip(theme::button(ui, w, &label, style), what).clicked() {
                        s.loop_samples = samples;
                    }
                }
            });
            // Without the video encoder there is only one thing to record.
            if mp4::SUPPORTED {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let w = theme::row_width(ui, 2, 4.0);
                    let formats = [
                        (LoopFormat::Video, "MP4", "One H.264 video file, ready to post or to play round and round."),
                        (
                            LoopFormat::Pictures,
                            "PNG frames",
                            "A folder of numbered PNG pictures, for compositing, editing or a GIF tool.",
                        ),
                    ];
                    for (format, label, what) in formats {
                        let active = (s.loop_format == format).then_some(ACCENT());
                        let style = ButtonStyle { active, height: 24.0, ..Default::default() };
                        if tip(theme::button(ui, w, label, style), what).clicked() {
                            s.loop_format = format;
                        }
                    }
                });
            }
        });
        // What Record will make, so the length and the number of frames are
        // known before committing to it.
        let output = match s.loop_output() {
            LoopFormat::Video => "MP4 video",
            LoopFormat::Pictures => "PNG pictures",
        };
        theme::hint(
            ui,
            &format!("{} x {}, {:.1} s, {} frames, {output}", s.loop_width, s.loop_height, s.loop_seconds(), s.loop_frames()),
        );

        let (label, style) = match recording {
            Some(_) => ("Cancel Recording", ButtonStyle { active: Some(theme::SUB), badge: "Esc", ..Default::default() }),
            None => ("Record 360\u{b0} Loop", ButtonStyle { active: Some(ACCENT()), ..Default::default() }),
        };
        let record = theme::button(ui, ui.available_width(), label, style);
        if tip(record, "Saves exactly one turn, starting from the view as it is now and at the speed above, as an MP4 video or numbered PNG pictures, as chosen. The last picture flows straight back into the first: a seamless loop. The panels hide while it records. Esc cancels.").clicked() {
            doc.turntable.request = Some(if recording.is_some() { Request::CancelLoop } else { Request::RecordLoop });
        }
        if let Some(p) = recording {
            theme::line(ui, &format!("Recording {} / {}", p.written, p.total), theme::HINT());
        } else if !doc.turntable.status.is_empty() {
            let color = if doc.turntable.status_ok { theme::OK } else { theme::ERROR };
            theme::line(ui, &doc.turntable.status, color);
        }
        let open = theme::button(ui, ui.available_width(), "Open Recordings Folder", ButtonStyle::default());
        if tip(open, "Opens the folder turntable loops and timelapses are saved to.").clicked() {
            doc.turntable.request = Some(Request::OpenFolder);
        }
    });
}

/// While a loop records the panels are hidden, so its progress and a way to
/// stop it are shown over the viewport. Only the window gets this; the saved
/// pictures are drawn separately and never have it in them.
pub fn recording_overlay(ctx: &egui::Context, doc: &mut Doc) {
    let Some(progress) = doc.turntable.recording else { return };
    let screen = ctx.content_rect();
    let size = vec2(320.0, 74.0);
    egui::Area::new(Id::new("loop_recording"))
        .order(Order::Foreground)
        .fade_in(false)
        .fixed_pos(pos2(screen.center().x - size.x * 0.5, screen.bottom() - 24.0 - size.y))
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
            let p = ui.painter();
            p.rect_filled(rect, 8.0, Color32::from_black_alpha(190));
            p.rect_stroke(rect, 8.0, theme::hairline(), egui::StrokeKind::Inside);
            let done = progress.written >= progress.total;
            let text = if done {
                (if progress.video { "Finishing the video..." } else { "Saving the last pictures..." }).to_owned()
            } else {
                format!("Recording 360\u{b0} loop   {} / {}", progress.written, progress.total)
            };
            p.text(rect.center_top() + vec2(0.0, 16.0), Align2::CENTER_CENTER, text, theme::bold(13.0), theme::TEXT());
            let bar = egui::Rect::from_min_size(rect.left_top() + vec2(14.0, 30.0), vec2(size.x - 28.0, 5.0));
            p.rect_filled(bar, 2.5, theme::FIELD());
            let fraction = progress.written as f32 / progress.total.max(1) as f32;
            let filled = egui::Rect::from_min_size(bar.min, vec2(bar.width() * fraction.clamp(0.0, 1.0), bar.height()));
            p.rect_filled(filled, 2.5, theme::SUB);
            p.circle_filled(rect.left_top() + vec2(20.0, 16.0), 4.0, theme::SUB);
            p.circle_stroke(rect.left_top() + vec2(20.0, 16.0), 6.5, Stroke::new(1.0, theme::SUB.gamma_multiply(0.5)));

            let button = egui::Rect::from_min_size(rect.left_bottom() + vec2(14.0, -30.0), vec2(size.x - 28.0, 22.0));
            let style = ButtonStyle { height: 22.0, badge: "Esc", ..Default::default() };
            let cancel = ui.put(button, |ui: &mut egui::Ui| theme::button(ui, button.width(), "Cancel", style));
            if cancel.clicked() {
                doc.turntable.request = Some(Request::CancelLoop);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_loop_is_one_turn_at_the_spin_speed() {
        // 30 degrees a second is a 12 second turn.
        assert_eq!(loop_frame_count(30.0, 30), 360);
        assert_eq!(loop_frame_count(30.0, 24), 288);
        assert_eq!(loop_frame_count(180.0, 60), 120);
        // Rounded, not cut short: 360 / 7 x 30 is 1542.86.
        assert_eq!(loop_frame_count(7.0, 30), 1543);
        // Speeds outside the range are held to it, so the count stays sane.
        assert_eq!(loop_frame_count(0.0, 30), loop_frame_count(MIN_SPEED, 30));
        assert_eq!(loop_frame_count(10_000.0, 1), 2);
        assert_eq!(Settings::default().loop_frames(), 360);
    }

    /// Two angles as the same direction, whatever multiple of a turn apart.
    fn same_angle(a: f32, b: f32) -> bool {
        let d = (a - b).rem_euclid(360.0);
        d.min(360.0 - d) < 1e-3
    }

    #[test]
    fn the_last_frame_plus_one_step_is_the_first() {
        for (total, direction, start) in [(360u32, -1.0f32, 25.0f32), (288, 1.0, -170.0), (1543, -1.0, 0.0), (2, 1.0, 359.5)] {
            let step = direction * 360.0 / total as f32;
            let first = loop_yaw(start, direction, 0, total);
            let last = loop_yaw(start, direction, total - 1, total);
            assert_eq!(first, start);
            assert!(same_angle(last + step, first), "{total} frames: {last} + {step} is not {first}");
            // Every step is the same size, the wrap included.
            let second = loop_yaw(start, direction, 1, total);
            assert!(same_angle(second - first, step));
        }
    }

    /// When each picture of a loop video starts, as the video's writer stamps
    /// it (100 ns units), with the end of the video last.
    fn video_times(frames: u32, fps: u32) -> Vec<i64> {
        let rate = mp4::rate_fraction(fps as f32);
        (0..=frames).map(|i| mp4::frame_time(i as u64, rate)).collect()
    }

    #[test]
    fn a_loop_video_is_one_turn_with_no_frame_twice() {
        for speed in [MIN_SPEED, 7.0, 30.0, 45.0, 90.0, MAX_SPEED] {
            for fps in FRAME_RATES {
                let frames = loop_frame_count(speed, fps);
                let what = format!("{speed} degrees/s at {fps} fps, {frames} frames");
                // Frame `frames` is the end of the video, not a picture in it.
                let times = video_times(frames, fps);
                assert_eq!(times.len(), frames as usize + 1);
                assert_eq!(times[0], 0, "{what}: the first picture is at the start");

                // Every picture lasts one frame, back to back with the next.
                let frame = 1e7 / fps as f64;
                for pair in times.windows(2) {
                    assert!(((pair[1] - pair[0]) as f64 - frame).abs() < 1.0, "{what}: steps of {}", pair[1] - pair[0]);
                }
                // So the frames as stamped add up to exactly frames / fps, with
                // nothing lost from the closing picture ...
                let length = times[frames as usize] as f64 / 1e7;
                assert!((length - frames as f64 / fps as f64).abs() < 1e-7, "{what}: {length} s");
                assert!((length - loop_seconds(frames, fps) as f64).abs() < 1e-3, "{what}");
                // ... and that is one turn at the speed asked for, to within
                // half a frame of rounding.
                assert!((length - 360.0 / speed as f64).abs() <= 0.5 / fps as f64 + 1e-9, "{what}: {length} s");

                // The view is at the same angle however it is worked out: from
                // the picture's number or from its time in the video, so a
                // player that plays round and round turns at a steady speed
                // across the join.
                let start = 25.0f32;
                for i in 0..frames {
                    let by_time = 360.0 * times[i as usize] as f64 / times[frames as usize] as f64;
                    let by_number = loop_yaw(start, 1.0, i, frames) as f64 - start as f64;
                    assert!((by_time - by_number).abs() < 1e-3, "{what}: picture {i} is at {by_number} or {by_time}");
                }
                // The last picture is one step short of the first, and the
                // first is not there twice.
                let step = 360.0 / frames as f32;
                let last = loop_yaw(start, 1.0, frames - 1, frames);
                assert!(((last - start) - (360.0 - step)).abs() < 1e-2, "{what}");
                let angles: Vec<f32> = (0..frames).map(|i| loop_yaw(start, 1.0, i, frames).rem_euclid(360.0)).collect();
                let first = angles[0];
                let again = angles[1..].iter().any(|a| (a - first).abs() <= step * 0.5);
                assert!(!again, "{what}: the first picture comes round again");
            }
        }
        // The default loop: 12 seconds, 360 pictures.
        let default = Settings::default();
        assert_eq!((default.loop_frames(), default.loop_seconds()), (360, 12.0));
    }

    #[test]
    fn the_spin_eases_in_and_out() {
        let settings = Settings { speed: 60.0, ..Default::default() };
        let mut spin = Spin { on: true, ..Default::default() };
        let dt = 1.0 / 60.0;
        // The first frame barely moves: no hard start.
        let first = spin.step(&settings, dt, false);
        assert!(first < 0.0 && first.abs() < 60.0 * dt * 0.1, "first step {first}");
        let mut last = first;
        for _ in 0..29 {
            let step = spin.step(&settings, dt, false);
            assert!(step.abs() >= last.abs(), "the spin slowed while speeding up");
            last = step;
        }
        // Half a second at 120 degrees/s/s reaches 60 degrees/s, and holds it.
        assert!((last + 60.0 * dt).abs() < 1e-3, "full speed step {last}");
        assert!((spin.step(&settings, dt, false) + 60.0 * dt).abs() < 1e-3);

        spin.on = false;
        let slowing = spin.step(&settings, dt, false);
        assert!(slowing < 0.0 && slowing.abs() < 60.0 * dt, "no hard stop");
        assert!(spin.moving());
        for _ in 0..40 {
            spin.step(&settings, dt, false);
        }
        assert!(!spin.moving());
        assert_eq!(spin.step(&settings, dt, false), 0.0);
    }

    #[test]
    fn the_spin_reverses_and_yields() {
        let mut settings = Settings { speed: 30.0, ..Default::default() };
        let mut spin = Spin { on: true, ..Default::default() };
        let dt = 1.0 / 60.0;
        for _ in 0..60 {
            spin.step(&settings, dt, false);
        }
        assert!(spin.step(&settings, dt, false) < 0.0);

        // Reversed, it slows through a stop and comes up to speed the other way.
        settings.reverse = true;
        let mut steps = Vec::new();
        for _ in 0..60 {
            steps.push(spin.step(&settings, dt, false));
        }
        assert!(steps[0] < 0.0, "it does not flip in one frame");
        assert!(steps.windows(2).all(|w| w[1] >= w[0]), "it turns round smoothly");
        assert!((steps[59] - 30.0 * dt).abs() < 1e-3, "it ends at full speed the other way");

        // An orbit by hand takes the view; afterwards the spin eases back in.
        assert_eq!(spin.step(&settings, dt, true), 0.0);
        let resumed = spin.step(&settings, dt, false);
        assert!(resumed > 0.0 && resumed < 30.0 * dt * 0.2);
    }

    #[test]
    fn settings_from_a_file_are_brought_into_range() {
        let wild = Settings {
            speed: 9000.0,
            reverse: true,
            loop_width: 3,
            loop_height: 100_000,
            loop_fps: 0,
            loop_format: LoopFormat::Pictures,
            loop_samples: 50,
        };
        let fitted = wild.in_range();
        let expected = Settings {
            speed: MAX_SPEED,
            reverse: true,
            loop_width: 64,
            loop_height: 8192,
            loop_fps: 1,
            loop_format: LoopFormat::Pictures,
            // The nearest count there is.
            loop_samples: 64,
        };
        assert_eq!(fitted, expected);
        assert_eq!(Settings { speed: f32::NAN, ..Default::default() }.in_range(), Settings::default());
        assert_eq!(Settings::default().in_range(), Settings::default());
        // A file that only mentions the speed keeps the rest as built in.
        let partial: Settings = serde_json::from_str("{ \"speed\": 45.0 }").unwrap();
        assert_eq!(partial, Settings { speed: 45.0, ..Default::default() });
    }

    #[test]
    fn the_hint_shows_then_fades() {
        let mut t = Turntable::default();
        assert_eq!(t.hint_alpha(10.0), 0.0);
        t.show_hint();
        assert_eq!(t.hint_alpha(10.0), 1.0);
        assert_eq!(t.hint_alpha(11.9), 1.0);
        let fading = t.hint_alpha(12.25);
        assert!((fading - 0.5).abs() < 1e-3);
        assert_eq!(t.hint_alpha(12.6), 0.0);
        // Gone for good until the clean view is entered again.
        assert_eq!(t.hint_alpha(10.5), 0.0);
    }
}
