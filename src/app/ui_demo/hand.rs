//! A stroke drawn by hand, scripted and measured: `--demo-ui hand=...`.
//!
//! Scripted strokes everywhere else are fed to the sculptor all at once and
//! laid without a budget. A hand does neither: its pointer events arrive as
//! the pen moves, a frame's worth at a time, and the stroke is laid at the
//! live pace (`sculpt/pace.rs`), so on a dense model with a big ring it
//! falls behind the pen and catches up. This drives exactly that, through
//! the routines the window's own pointer events go to, and writes down
//! what happened: how long the frames took, how far behind the brush got,
//! how long it took to catch up and to let go, whether the dabs were
//! spaced wider to keep up, and how tall the stroke stands along its path.
//!
//! The word is `hand=<brush>@<thousands of triangles>@<ring radius in
//! pixels>@<seconds>`, then if wanted `@<Spacing in percent>` (the brush's
//! own otherwise). After it, `:x4` has the pen go across and back that many
//! times without lifting (`<seconds>` is one crossing), and `:lift` lets go
//! the moment the pen reaches the end of the path (the default holds the
//! pen there, still down, until the stroke has caught up). It sets up a ball of that many
//! triangles and waits. The stroke begins when the Claude bridge is told
//! `command hand_stroke`, so that whatever is driving the run can take a
//! picture first, and `command hand_report` gives the report once the pen
//! is up (before that the bridge is busy, as with any stroke).
//!
//! The pen crosses the middle of the ball from left to right at an even
//! speed, sampled 240 times a second, at full pressure.

use std::time::{Duration, Instant};

use glam::Vec2;
use serde_json::json;
use winit::event::MouseButton;
use winit::keyboard::ModifiersState;

use crate::app::Running;

/// How many times a second the pen reports.
const PEN_RATE: f64 = 240.0;
/// The share of the ball's width the stroke crosses.
const PATH: f32 = 0.6;
/// Places along the path the stroke's height is read at.
const READINGS: usize = 240;

/// What the run was asked for.
#[derive(Clone, Debug)]
struct Asked {
    brush: String,
    triangles: usize,
    radius_px: f32,
    seconds: f64,
    /// The Spacing to give the brush for this run, as a share of its radius.
    spacing: Option<f32>,
    /// How many times the pen crosses, there and back, without lifting.
    crossings: usize,
    /// Let go at the end of the path instead of waiting to catch up.
    lift: bool,
}

impl Asked {
    fn parse(spec: &str) -> Option<Asked> {
        let mut words = spec.split(':').map(str::trim);
        let spec = words.next()?;
        let (mut lift, mut crossings) = (false, 1);
        for word in words {
            match word.strip_prefix('x') {
                Some(times) => crossings = times.parse::<usize>().ok().filter(|&times| times >= 1)?,
                None if word == "lift" => lift = true,
                None => return None,
            }
        }
        let mut parts = spec.split('@').map(str::trim);
        let brush = parts.next().filter(|name| !name.is_empty())?.to_owned();
        let thousands: f32 = parts.next()?.parse().ok()?;
        let radius_px: f32 = parts.next()?.parse().ok()?;
        let seconds: f64 = parts.next()?.parse().ok()?;
        let spacing = match parts.next() {
            Some(percent) => Some(percent.parse::<f32>().ok()? / 100.0),
            None => None,
        };
        let sane = thousands >= 1.0 && radius_px >= 2.0 && seconds > 0.0;
        sane.then(|| Asked { brush, triangles: (thousands * 1000.0) as usize, radius_px, seconds, spacing, crossings, lift })
    }
}

/// One frame of the stroke, as it began.
#[derive(Clone, Copy)]
struct Frame {
    /// Since the frame before, in milliseconds.
    took: f32,
    /// How far the brush is behind the pen on screen, in ring radii.
    behind: f32,
    /// Dabs still to lay, and how much wider than asked they are being laid.
    waiting: f32,
    widen: f32,
    /// Milliseconds a dab has been taking.
    per_dab: f32,
}

enum Phase {
    /// Set up and waiting for `command hand_stroke`.
    Ready,
    /// The pen is crossing: when it landed, and how many samples it has sent.
    Crossing { began: Instant, sent: usize },
    /// The pen is at the end of the path, still down, since then.
    Holding { since: Instant },
    Done,
}

pub(in crate::app) struct Hand {
    asked: Asked,
    phase: Phase,
    /// The path on screen: from, to.
    path: (Vec2, Vec2),
    /// How far from the ball's middle the surface was along the path before
    /// the stroke.
    before: Vec<Option<f32>>,
    last_frame: Instant,
    frames: Vec<Frame>,
    dabs_before: u64,
    /// Seconds from the pen stopping to the stroke having caught up, and
    /// how long letting go took.
    caught_up: Option<f64>,
    release: f64,
    report: Option<String>,
}

fn millis(d: Duration) -> f32 {
    d.as_secs_f32() * 1000.0
}

/// The value a `share` of the way up `sorted`.
fn at_share(sorted: &[f32], share: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f32 * share).round() as usize]
}

impl Running {
    /// Sets the run up from the `hand=` word: the ball, the brush, the view.
    pub(in crate::app) fn hand_demo(&mut self, spec: &str) {
        let Some(asked) = Asked::parse(spec) else {
            return eprintln!("--demo-ui: hand wants brush@thousands of triangles@radius@seconds, as in hand=Clay@2000@140@0.6");
        };
        let old: Vec<u64> = self.doc.scene.objects.iter().map(|o| o.id).collect();
        let size = self.view.size;
        let made = [
            self.call("add_shape", &json!({ "shape": "Sphere", "name": "Dense", "triangles": asked.triangles, "position": [0, 0, 0], "size": 0.5 })),
            self.call("set_options", &json!({ "symmetry_guides": false })),
            self.call("set_camera", &json!({ "view": "front", "frame": "active" })),
        ];
        for id in old {
            let _ = self.call("delete_object", &json!({ "object": id }));
        }
        let radius = asked.radius_px / size.y.max(1.0);
        let mut brush = json!({ "brush": asked.brush, "radius": radius, "mirror_x": false, "mirror_y": false, "mirror_z": false });
        if let Some(spacing) = asked.spacing {
            brush["spacing"] = json!(spacing);
        }
        let brush = self.call("set_brush", &brush);
        for result in made.into_iter().chain([brush]) {
            if let Err(why) = result {
                eprintln!("--demo-ui hand: {why}");
            }
        }
        let now = Instant::now();
        self.ui_demo.hand = Some(Box::new(Hand {
            asked,
            phase: Phase::Ready,
            path: (Vec2::ZERO, Vec2::ZERO),
            before: Vec::new(),
            last_frame: now,
            frames: Vec::new(),
            dabs_before: 0,
            caught_up: None,
            release: 0.0,
            report: None,
        }));
    }

    /// The bridge's `command hand_stroke` and `command hand_report`, in a
    /// run started with the `hand=` word. `None` for any other command, or
    /// in any other run.
    pub(in crate::app) fn hand_demo_command(&mut self, name: &str) -> Option<Result<String, String>> {
        let hand = self.ui_demo.hand.as_mut()?;
        match name {
            "handstroke" => Some(match hand.phase {
                Phase::Ready => {
                    self.hand_begin();
                    Ok("The pen is down.".into())
                }
                _ => Err("That stroke has been drawn.".into()),
            }),
            "handreport" => Some(hand.report.clone().ok_or_else(|| "The stroke has not been drawn yet.".into())),
            _ => None,
        }
    }

    /// The ball's middle and its radius on screen, in the window's pixels.
    fn hand_ball(&self) -> (Vec2, f32) {
        let middle = self.view.origin + self.view.size * 0.5;
        // The ball is framed in the middle of the view: its edge is where a
        // ray from the pointer stops hitting it.
        let hits = |reach: f32| {
            let (origin, dir) = self.local.ray(middle + Vec2::new(reach, 0.0));
            self.doc.mesh.raycast(origin, dir).is_some()
        };
        let (mut inside, mut outside) = (0.0, self.view.size.x * 0.5);
        for _ in 0..20 {
            let reach = (inside + outside) * 0.5;
            if hits(reach) { inside = reach } else { outside = reach }
        }
        (middle, inside)
    }

    /// How far from the ball's middle the surface is under each of the
    /// places along the path the stroke's height is read at.
    fn hand_readings(&self, (from, to): (Vec2, Vec2)) -> Vec<Option<f32>> {
        (0..READINGS)
            .map(|i| {
                let (origin, dir) = self.local.ray(from.lerp(to, (i as f32 + 0.5) / READINGS as f32));
                self.doc.mesh.raycast(origin, dir).map(|hit| hit.pos.length())
            })
            .collect()
    }

    /// The pen lands at the start of the path.
    fn hand_begin(&mut self) {
        self.sync_view();
        let (middle, ball) = self.hand_ball();
        let path = (middle - Vec2::new(ball * PATH, 0.0), middle + Vec2::new(ball * PATH, 0.0));
        let before = self.hand_readings(path);
        self.input.inside = true;
        self.input.mods = ModifiersState::empty();
        self.pointer_moved(path.0, None);
        self.pointer_pressed(MouseButton::Left, Some(1.0));
        let dabs_before = self.doc.sculptor.dabs_applied;
        let Some(hand) = self.ui_demo.hand.as_mut() else { return };
        let now = Instant::now();
        (hand.path, hand.before, hand.dabs_before, hand.last_frame) = (path, before, dabs_before, now);
        hand.phase = Phase::Crossing { began: now, sent: 0 };
    }

    /// Once a frame, before the stroke's samples are taken in: the pen
    /// moves on as far as it has come by now, as the window's events would
    /// have said it had.
    pub(in crate::app) fn hand_demo_frame(&mut self) {
        let Some(mut hand) = self.ui_demo.hand.take() else { return };
        let now = Instant::now();
        match hand.phase {
            Phase::Ready | Phase::Done => {}
            Phase::Crossing { began, sent } => {
                self.hand_note(&mut hand, now);
                let (from, to) = hand.path;
                let crossing = (hand.asked.seconds * PEN_RATE).ceil() as usize;
                let total = crossing * hand.asked.crossings;
                let due = (((now - began).as_secs_f64() * PEN_RATE) as usize).min(total);
                for sample in sent + 1..=due {
                    // There on the odd crossings and back on the even.
                    let (done, along) = (sample / crossing, (sample % crossing) as f32 / crossing as f32);
                    let t = if done % 2 == 0 { along } else { 1.0 - along };
                    self.pointer_moved(from.lerp(to, t), Some(1.0));
                }
                hand.phase = Phase::Crossing { began, sent: due };
                if due == total {
                    hand.phase = Phase::Holding { since: now };
                    if hand.asked.lift {
                        self.hand_lift(&mut hand);
                    }
                }
            }
            Phase::Holding { since } => {
                self.hand_note(&mut hand, now);
                // A frame after the stroke stops being behind, so that the
                // frame that caught it up is in the count.
                if !self.doc.sculptor.lag().is_some_and(|lag| lag.behind) {
                    hand.caught_up = Some((now - since).as_secs_f64());
                    self.hand_lift(&mut hand);
                }
            }
        }
        self.redraw = true;
        self.ui_demo.hand = Some(hand);
    }

    /// Notes the frame that has just ended.
    fn hand_note(&mut self, hand: &mut Hand, now: Instant) {
        let took = millis(now - hand.last_frame);
        hand.last_frame = now;
        let Some(lag) = self.doc.sculptor.lag() else { return };
        let behind = self.input.pointer.distance(lag.at) / hand.asked.radius_px.max(1.0);
        hand.frames.push(Frame { took, behind, waiting: lag.dabs, widen: lag.widen, per_dab: lag.per_dab * 1000.0 });
    }

    /// The pen lifts, and the report is written.
    fn hand_lift(&mut self, hand: &mut Hand) {
        let began = Instant::now();
        self.pointer_released(MouseButton::Left);
        hand.release = began.elapsed().as_secs_f64();
        self.input.inside = false;
        hand.phase = Phase::Done;
        let report = self.hand_report(hand);
        println!("{report}");
        hand.report = Some(report);
    }

    fn hand_report(&self, hand: &Hand) -> String {
        let asked = &hand.asked;
        let mut took: Vec<f32> = hand.frames.iter().map(|frame| frame.took).collect();
        took.sort_by(f32::total_cmp);
        let most = |value: fn(&Frame) -> f32| hand.frames.iter().map(value).fold(0.0, f32::max);
        let dabs = self.doc.sculptor.dabs_applied - hand.dabs_before;
        // How much the stroke raised the surface along the middle of its
        // path, clear of the ends, where it tapers.
        let after = self.hand_readings(hand.path);
        let raised: Vec<f32> = hand.before.iter().zip(&after).filter_map(|(b, a)| Some((*a)? - (*b)?)).collect();
        let middle = &raised[raised.len() / 8..raised.len() - raised.len() / 8];
        let mean = middle.iter().sum::<f32>() / middle.len().max(1) as f32;
        let (least, tallest) = middle.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &h| (lo.min(h), hi.max(h)));
        let gaps = middle.iter().filter(|&&h| h < mean * 0.5).count();
        let caught_up = match hand.caught_up {
            Some(seconds) => format!("{seconds:.2} s after the pen stopped"),
            None => "not waited for: the pen lifted at the end of the path".to_owned(),
        };
        format!(
            "hand: {} on {} triangles, ring {} px, across in {} s, {} time(s)\n\
             hand: frames {}, median {:.0} ms, nine in ten under {:.0} ms, longest {:.0} ms\n\
             hand: furthest behind the pen {:.1} ring radii, most dabs waiting {:.0}, widest spacing {:.2}x, a dab took up to {:.1} ms\n\
             hand: caught up {caught_up}; letting go took {:.3} s\n\
             hand: {dabs} dabs; raised along the path {:.5} on average, least {:.5}, most {:.5}, {gaps} of {} readings under half the average",
            asked.brush,
            self.doc.mesh.triangle_count(),
            asked.radius_px,
            asked.seconds,
            asked.crossings,
            took.len(),
            at_share(&took, 0.5),
            at_share(&took, 0.9),
            at_share(&took, 1.0),
            most(|frame| frame.behind),
            most(|frame| frame.waiting),
            most(|frame| frame.widen),
            most(|frame| frame.per_dab),
            hand.release,
            mean,
            least,
            tallest,
            middle.len(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hand_word_reads_its_four_parts_and_lift() {
        let asked = Asked::parse("Clay Strips@2000@140@0.6").unwrap();
        assert_eq!((asked.brush.as_str(), asked.triangles, asked.radius_px, asked.seconds, asked.lift), ("Clay Strips", 2_000_000, 140.0, 0.6, false));
        assert_eq!((asked.spacing, asked.crossings), (None, 1));
        let tight = Asked::parse("Smooth@500@80@1.5@5:x4:lift").unwrap();
        assert!(tight.lift && tight.spacing == Some(0.05) && tight.crossings == 4);
        assert!(Asked::parse("Smooth@500@80@1.5:lift").unwrap().lift);
        let wrong = ["", "Clay", "Clay@2000@140", "Clay@0@140@0.6", "Clay@2000@140@0", "@2000@140@0.6", "Clay@many@140@0.6"];
        for wrong in wrong.into_iter().chain(["Clay@2000@140@0.6@tight", "Clay@2000@140@0.6:x0", "Clay@2000@140@0.6:fast"]) {
            assert!(Asked::parse(wrong).is_none(), "{wrong:?} was taken for a stroke");
        }
    }

    #[test]
    fn a_share_of_the_way_up_a_sorted_list() {
        let sorted = [10.0, 20.0, 30.0, 40.0, 50.0];
        assert_eq!((at_share(&sorted, 0.0), at_share(&sorted, 0.5), at_share(&sorted, 1.0)), (10.0, 30.0, 50.0));
        assert_eq!(at_share(&[], 0.5), 0.0);
    }
}
