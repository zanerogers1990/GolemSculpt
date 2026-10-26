//! The Timelapse card of the Scene panel: start and stop, and every setting
//! the recorder has.

use egui::Color32;

use super::{FRAME_RATES, Framing, MAX_SPEED, MIN_SPEED, Phase, Request, SIZES, mp4};
use crate::doc::Doc;
use crate::ui::theme::{self, ACCENT, ButtonStyle};

/// One of several choices as a row of buttons; returns the one clicked.
fn choice(ui: &mut egui::Ui, labels: &[&str], tips: Option<&[&str]>, current: Option<usize>) -> Option<usize> {
    let mut picked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let w = theme::row_width(ui, labels.len(), 4.0);
        for (i, label) in labels.iter().enumerate() {
            let style = ButtonStyle { active: (current == Some(i)).then_some(ACCENT()), height: 24.0, ..Default::default() };
            let mut button = theme::button(ui, w, label, style);
            if let Some(tip) = tips.and_then(|t| t.get(i)) {
                button = sculpt_toolkit::translate::hover(button, tip);
            }
            if button.clicked() {
                picked = Some(i);
            }
        }
    });
    picked
}

pub fn card(ui: &mut egui::Ui, doc: &mut Doc) {
    let tips = doc.opts.tooltips;
    let tip = |r: egui::Response, text: &str| theme::tip(r, tips, text);
    let t = &mut doc.timelapse;
    let summary = match t.phase {
        Phase::Idle => format!("{:.0}x", t.options.speed),
        Phase::Recording => "Recording".to_owned(),
        Phase::Saving => "Saving".to_owned(),
    };
    if t.pinned {
        theme::open_category(ui, "Timelapse");
        ui.scroll_to_cursor_animation(Some(egui::Align::TOP), egui::style::ScrollAnimation::none());
    }
    theme::category(ui, "Timelapse", Color32::from_rgb(235, 120, 120), &summary, false, |ui| {
        if !mp4::SUPPORTED {
            theme::hint(ui, "Recording a timelapse needs Windows.");
            return;
        }
        let (label, style) = match t.phase {
            Phase::Idle => ("Start Timelapse", ButtonStyle { active: Some(ACCENT()), ..Default::default() }),
            Phase::Recording => ("Stop Timelapse", ButtonStyle { active: Some(theme::SUB), ..Default::default() }),
            Phase::Saving => ("Saving...", ButtonStyle::default()),
        };
        ui.add_enabled_ui(t.phase != Phase::Saving, |ui| {
            let record = theme::button(ui, ui.available_width(), label, style);
            if tip(record, "Records a sped-up video of your sculpting that follows your viewing angle and the part you are working on. Only the time the model is actually changing is kept: idling, and undo and redo, are left out. Stop saves it as an MP4.").clicked() {
                t.request = Some(if t.phase == Phase::Recording { Request::Stop } else { Request::Start });
            }
        });
        if !t.status.is_empty() {
            let color = match (t.phase, t.status_ok) {
                (Phase::Idle, true) => theme::OK,
                (Phase::Idle, false) => theme::ERROR,
                _ => theme::HINT(),
            };
            theme::line(ui, &t.status, color);
        }

        // On a log scale: the useful speeds are 2x, 4x, 8x and 16x, and a
        // straight 1 to 240 slider crams all of those into its first
        // centimetre.
        let o = &mut t.options;
        let mut log = o.speed.clamp(MIN_SPEED, MAX_SPEED).log2();
        let readout = |v: f32| {
            let speed = round_speed(v.exp2());
            // 7.5 s, but 8 s and not 8.0 s.
            let video = (600.0 / speed).round() / 10.0;
            format!("{speed}x   1 min = {video} s of video")
        };
        let speed = theme::slider(ui, "Speed", &mut log, MIN_SPEED.log2()..=MAX_SPEED.log2(), readout);
        if tip(speed, "Seconds of sculpting per second of video, on top of leaving idle time out. Can be changed while recording.").changed() {
            o.speed = round_speed(log.exp2());
        }

        theme::heading(ui, "Video");
        // The size and the frame rate are fixed into the file when recording starts.
        ui.add_enabled_ui(t.phase == Phase::Idle, |ui| {
            let current = SIZES.iter().position(|&(_, _, w, h)| (w, h) == (o.width, o.height));
            for (row, sizes) in SIZES.chunks(3).enumerate() {
                let labels: Vec<&str> = sizes.iter().map(|s| s.0).collect();
                let sizes_tips: Vec<&str> = sizes.iter().map(|s| s.1).collect();
                let here = current.and_then(|c| c.checked_sub(row * 3)).filter(|&c| c < 3);
                if let Some(i) = choice(ui, &labels, tips.then_some(sizes_tips.as_slice()), here) {
                    (o.width, o.height) = (sizes[i].2, sizes[i].3);
                }
            }
            let labels: Vec<String> = FRAME_RATES.iter().map(|f| format!("{f} fps")).collect();
            let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
            if let Some(i) = choice(ui, &labels, None, FRAME_RATES.iter().position(|&f| f == o.frame_rate)) {
                o.frame_rate = FRAME_RATES[i];
            }
        });

        theme::heading(ui, "Camera");
        let framing_tips = [
            "Films from your own viewing angle and leans toward where you are sculpting: smoothed, so it is a camera move and not a replay of your hand.",
            "Circles the model and ignores your camera.",
        ];
        let current = if o.framing == Framing::FollowArtist { 0 } else { 1 };
        if let Some(i) = choice(ui, &["Follow My View", "Turntable Orbit"], tips.then_some(&framing_tips[..]), Some(current)) {
            o.framing = if i == 0 { Framing::FollowArtist } else { Framing::Orbit };
        }
        let two = |v: f32| format!("{v:.2}");
        match o.framing {
            Framing::FollowArtist => {
                tip(
                    theme::slider(ui, "Focus on Work", &mut o.focus_strength, 0.0..=1.0, two),
                    "How far the shot moves from your pivot onto the spot you are actually sculpting. 0 is your framing exactly; 1 keeps the brush dead centre and lets the model swing around it.",
                );
                tip(
                    theme::slider(ui, "Zoom", &mut o.focus_zoom, 0.5..=2.0, two),
                    "A multiple of your own viewing distance. 1 shows what you see; below 1 pushes in on the work; above 1 leaves air around it.",
                );
            }
            Framing::Orbit => {
                let turn = theme::slider(ui, "Orbit Speed", &mut o.orbit_speed, 0.0..=90.0, |v| format!("{v:.0}\u{b0}/s"));
                if tip(turn, "Degrees per second of finished video, so the turn stays even however stop-start the session was.").changed() {
                    o.orbit_speed = o.orbit_speed.round();
                }
                tip(
                    theme::slider(ui, "Distance", &mut o.orbit_distance, 0.5..=4.0, two),
                    "1 fills the picture with the model; higher stands further back.",
                );
                let height = theme::slider(ui, "Height", &mut o.orbit_height, -80.0..=80.0, |v| format!("{v:.0}\u{b0}"));
                if tip(height, "How far above the model the camera looks from.").changed() {
                    o.orbit_height = o.orbit_height.round();
                }
                tip(
                    theme::checkbox(ui, &mut o.follow_subject, "Follow Subject", ACCENT()),
                    "Re-frames as the model grows or you pick another object.",
                );
            }
        }
        tip(
            theme::slider(ui, "Camera Smoothing", &mut o.camera_smoothing, 0.0..=3.0, |v| format!("{v:.1} s")),
            "How long the camera takes to settle on a new framing, in seconds of video: it looks the same however fast the app ran or how high Speed is set.",
        );
        tip(
            theme::slider(ui, "Idle Grace", &mut o.idle_grace, 0.0..=5.0, |v| format!("{v:.2} s")),
            "Keeps recording this long after the last change, so the natural gaps between strokes do not turn into hard cuts.",
        );

        let open = theme::button(ui, ui.available_width(), "Open Recordings Folder", ButtonStyle::default());
        if tip(open, "Opens the folder timelapses are saved to.").clicked() {
            t.request = Some(Request::OpenFolder);
        }
    });
}

/// Whole numbers below 10x, halves above: nobody needs 7.3183x.
fn round_speed(speed: f32) -> f32 {
    let rounded = if speed < 10.0 { speed.round() } else { (speed * 2.0).round() * 0.5 };
    rounded.clamp(MIN_SPEED, MAX_SPEED)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speeds_are_rounded_to_sensible_steps() {
        assert_eq!(round_speed(7.3183), 7.0);
        assert_eq!(round_speed(0.2), MIN_SPEED);
        assert_eq!(round_speed(12.3), 12.5);
        assert_eq!(round_speed(500.0), MAX_SPEED);
    }
}
