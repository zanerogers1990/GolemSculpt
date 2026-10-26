//! The left panel while the tool is up: hand or foot, which side, poses and builds as buttons, every
//! slider, and Accept. Every change is an undo step of the tool's own (a slider drag is one step).

use std::ops::RangeInclusive;

use sculpt_toolkit::theme::{self, ACCENT, ButtonStyle};
use sculpt_toolkit::{Color32, Ctx};

use crate::tool::{HandTool, Kind, MAX_DETAIL, MIN_DETAIL, Side};
use crate::{FootPose, FootShape, FootStyle, HandPose, HandShape, HandStyle, MAX_DIGITS};

/// A warm flesh tone, apart from the Lathe's amber and the Tube's blue.
const ACCENT_HANDS: Color32 = Color32::from_rgb(232, 150, 120);

const SHORTCUTS: &[(&str, &str)] = &[
    ("Gizmo", "Move, turn, size"),
    ("Double-tap a slider", "Type a value"),
    ("Enter", "Accept"),
    ("Ctrl+Z / Ctrl+Shift+Z", "Undo / redo a change"),
    ("Esc", "Put away (kept for next time)"),
    ("Drag background", "Orbit"),
];

fn percent(v: f32) -> String {
    format!("{:.0}%", v * 100.0)
}

fn count(v: f32) -> String {
    format!("{:.0}", v.round())
}

/// A slider with its tooltip.
fn slide(ui: &mut egui::Ui, cx: &Ctx, label: &str, value: &mut f32, range: RangeInclusive<f32>, readout: fn(f32) -> String, help: &str) {
    let r = theme::slider(ui, label, value, range, readout);
    theme::tip(r, cx.tips, help);
}

/// A whole-number slider over a count.
fn slide_count(ui: &mut egui::Ui, cx: &Ctx, label: &str, value: &mut u32, range: RangeInclusive<u32>, help: &str) {
    let mut v = *value as f32;
    slide(ui, cx, label, &mut v, *range.start() as f32..=*range.end() as f32, count, help);
    *value = (v.round() as u32).clamp(*range.start(), *range.end());
}

/// A curl slider for every digit at once: shows their average, and sets them all.
fn slide_all(ui: &mut egui::Ui, cx: &Ctx, label: &str, curls: &mut [f32], range: RangeInclusive<f32>, help: &str) {
    let mut average = curls.iter().sum::<f32>() / curls.len().max(1) as f32;
    let before = average;
    slide(ui, cx, label, &mut average, range, percent, help);
    if average != before {
        curls.iter_mut().for_each(|c| *c = average);
    }
}

fn finger_name(i: usize, n: usize) -> String {
    match (n, i) {
        (4, _) => ["Index", "Middle", "Ring", "Little"][i].to_owned(),
        (3, _) => ["Index", "Middle", "Little"][i].to_owned(),
        _ => format!("Finger {}", i + 1),
    }
}

fn toe_name(i: usize, n: usize) -> String {
    match (n, i) {
        (5, _) => ["Big Toe", "Second Toe", "Third Toe", "Fourth Toe", "Little Toe"][i].to_owned(),
        (_, 0) if n > 1 => "Big Toe".to_owned(),
        _ => format!("Toe {}", i + 1),
    }
}

impl HandTool {
    pub(crate) fn show_panel(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        let before = *self.setup();
        // Buttons are a step each; sliders are coalesced while they move.
        let mut fresh = false;
        let mut setup = before;
        let kind = setup.kind;
        let summary = match (setup.side, kind) {
            (Side::Pair, Kind::Hand) => "Pair of Hands".to_owned(),
            (Side::Pair, Kind::Foot) => "Pair of Feet".to_owned(),
            (side, kind) => format!("{} {}", side.name(), kind.name()),
        };
        theme::category(ui, "Hand & Foot", ACCENT_HANDS, &summary, true, |ui| {
            fresh |= self.top_rows(ui, cx, &mut setup);
            fresh |= match kind {
                Kind::Hand => hand_presets(ui, cx, &mut setup.hand),
                Kind::Foot => foot_presets(ui, cx, &mut setup.foot),
            };
            self.accept_rows(ui, cx);
        });
        match kind {
            Kind::Hand => fresh |= hand_sliders(ui, cx, &mut setup.hand),
            Kind::Foot => fresh |= foot_sliders(ui, cx, &mut setup.foot),
        }
        theme::category(ui, "Hand & Foot Shortcuts", theme::KEY, "", false, |ui| theme::shortcuts(ui, SHORTCUTS));
        self.apply_panel(setup, before, cx.time, fresh);
    }

    fn top_rows(&mut self, ui: &mut egui::Ui, cx: &mut Ctx, setup: &mut crate::tool::Setup) -> bool {
        let mut fresh = false;
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, 2, 6.0);
            for (kind, help) in [
                (Kind::Hand, "Shape a hand: fingers, thumb, palm and wrist."),
                (Kind::Foot, "Shape a foot: toes, arch, heel and ankle."),
            ] {
                let style = ButtonStyle { active: (setup.kind == kind).then(ACCENT), icon: cx.icon("tool_hands"), ..Default::default() };
                if theme::tip(theme::button(ui, w, kind.name(), style), cx.tips, help).clicked() && setup.kind != kind {
                    setup.kind = kind;
                    fresh = true;
                }
            }
        });
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, 3, 6.0);
            for (side, help) in [
                (Side::Right, "A right one."),
                (Side::Left, "A left one: the mirror image of the right."),
                (Side::Pair, "A right one where the gizmo is, and a left one mirrored across the middle of the world, so both sides of a figure come at once."),
            ] {
                let style = ButtonStyle { active: (setup.side == side).then(ACCENT), ..Default::default() };
                if theme::tip(theme::button(ui, w, side.name(), style), cx.tips, help).clicked() && setup.side != side {
                    setup.side = side;
                    fresh = true;
                }
            }
        });
        fresh
    }

    fn accept_rows(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, 2, 6.0);
            ui.add_enabled_ui(!self.is_making(), |ui| {
                let style = ButtonStyle { icon: cx.icon("check"), active: Some(ACCENT()), badge: "Enter", ..Default::default() };
                let help = "Makes it a real object to sculpt (Enter), and goes back to sculpting. The settings are kept for the next one.";
                if theme::tip(theme::button(ui, w, "Accept", style), cx.tips, help).clicked() {
                    self.accept(cx);
                }
            });
            let r = theme::button(ui, w, "Done", ButtonStyle { badge: "Esc", ..Default::default() });
            if theme::tip(r, cx.tips, "Puts the tool away without making anything. The settings are kept.").clicked() {
                cx.finish();
            }
        });
        let mut detail = self.detail();
        let help = "How finely Accept builds the mesh: more is smoother and holds finer sculpting, and is heavier. The preview is always lighter.";
        slide(ui, cx, "Detail", &mut detail, MIN_DETAIL..=MAX_DETAIL, count, help);
        self.set_detail(detail);
        let each = if self.setup().side == Side::Pair { " each" } else { "" };
        let text = format!("Accept makes about {} triangles{each}.", theme::count(self.estimate_triangles()));
        theme::line(ui, &text, theme::HINT());
        theme::hint(ui, "Move, turn and size it with the gizmo. Double-tap a slider to type a value.");
    }

    fn apply_panel(&mut self, setup: crate::tool::Setup, before: crate::tool::Setup, now: f64, fresh: bool) {
        self.set_setup(setup);
        self.changed(before, now, fresh);
    }
}

fn reset_button(ui: &mut egui::Ui, cx: &Ctx, what: &str) -> bool {
    let r = theme::button(ui, ui.available_width(), &format!("Reset {what}"), ButtonStyle::default());
    theme::tip(r, cx.tips, "Puts every slider back to an ordinary adult's, straight and at rest. Undoable (Ctrl+Z).").clicked()
}

fn hand_presets(ui: &mut egui::Ui, cx: &Ctx, hand: &mut HandShape) -> bool {
    let mut fresh = false;
    theme::heading(ui, "Pose");
    for row in HandPose::ALL.chunks(3) {
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, 3, 6.0);
            for &pose in row {
                if theme::tip(theme::button(ui, w, pose.name(), ButtonStyle::default()), cx.tips, pose.tip()).clicked() {
                    pose.apply(hand);
                    fresh = true;
                }
            }
        });
    }
    theme::heading(ui, "Build");
    for row in HandStyle::ALL.chunks(3) {
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, 3, 6.0);
            for &style in row {
                if theme::tip(theme::button(ui, w, style.name(), ButtonStyle::default()), cx.tips, style.tip()).clicked() {
                    style.apply(hand);
                    fresh = true;
                }
            }
        });
    }
    if reset_button(ui, cx, "Hand") {
        *hand = HandShape::default();
        fresh = true;
    }
    fresh
}

fn foot_presets(ui: &mut egui::Ui, cx: &Ctx, foot: &mut FootShape) -> bool {
    let mut fresh = false;
    theme::heading(ui, "Pose");
    for row in FootPose::ALL.chunks(3) {
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, 3, 6.0);
            for &pose in row {
                if theme::tip(theme::button(ui, w, pose.name(), ButtonStyle::default()), cx.tips, pose.tip()).clicked() {
                    pose.apply(foot);
                    fresh = true;
                }
            }
        });
    }
    theme::heading(ui, "Build");
    for row in FootStyle::ALL.chunks(3) {
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, 3, 6.0);
            for &style in row {
                if theme::tip(theme::button(ui, w, style.name(), ButtonStyle::default()), cx.tips, style.tip()).clicked() {
                    style.apply(foot);
                    fresh = true;
                }
            }
        });
    }
    if reset_button(ui, cx, "Foot") {
        *foot = FootShape::default();
        fresh = true;
    }
    fresh
}

fn hand_sliders(ui: &mut egui::Ui, cx: &Ctx, h: &mut HandShape) -> bool {
    let mut fresh = false;
    let n = h.fingers.clamp(1, MAX_DIGITS as u32) as usize;
    theme::category(ui, "Fingers", ACCENT_HANDS, &format!("{n}"), true, |ui| {
        slide_count(ui, cx, "Fingers", &mut h.fingers, 1..=MAX_DIGITS as u32, "How many fingers, not counting the thumb.");
        let mut thumb = h.thumb;
        let r = theme::checkbox(ui, &mut thumb, "Thumb", ACCENT_HANDS);
        if theme::tip(r, cx.tips, "Whether the hand has a thumb.").changed() {
            h.thumb = thumb;
            fresh = true;
        }
        slide(ui, cx, "Length", &mut h.finger_length, 0.4..=1.8, percent, "Finger length against an ordinary hand's.");
        slide(ui, cx, "Thickness", &mut h.finger_thickness, 0.5..=2.0, percent, "How thick the fingers and thumb are.");
        slide(ui, cx, "Taper", &mut h.taper, 0.0..=1.0, percent, "How much thinner the fingertips are than the bases.");
        slide(ui, cx, "Spread", &mut h.spread, 0.0..=1.0, percent, "How far the fingers fan apart.");
        let curls = &mut h.curl[..n];
        slide_all(ui, cx, "Curl", curls, 0.0..=1.0, "How far every finger curls toward the palm: 0% straight, 100% a fist. Set fingers one by one under Each Finger.");
        slide(ui, cx, "Knuckles", &mut h.knuckles, 0.0..=1.0, percent, "How much the knuckles stand out.");
        slide(ui, cx, "Claws", &mut h.claws, 0.0..=1.0, percent, "Claws growing from the tips. 0% is none.");
    });
    theme::category(ui, "Thumb & Palm", ACCENT_HANDS, "", true, |ui| {
        ui.add_enabled_ui(h.thumb, |ui| {
            slide(ui, cx, "Thumb Length", &mut h.thumb_length, 0.5..=1.6, percent, "Thumb length against an ordinary hand's.");
            slide(ui, cx, "Thumb Angle", &mut h.thumb_angle, 0.0..=1.0, percent, "How far the thumb swings out from the hand.");
            slide(ui, cx, "Thumb Curl", &mut h.thumb_curl, 0.0..=1.0, percent, "How far the thumb folds across the palm.");
        });
        slide(ui, cx, "Palm Width", &mut h.palm_width, 0.6..=1.6, percent, "How wide the palm is.");
        slide(ui, cx, "Palm Length", &mut h.palm_length, 0.6..=1.5, percent, "How long the palm is, wrist to knuckles.");
        slide(ui, cx, "Palm Thickness", &mut h.palm_thickness, 0.5..=1.8, percent, "How thick the palm and wrist are.");
        slide(ui, cx, "Wrist Length", &mut h.wrist, 0.0..=1.0, percent, "How much forearm comes with the hand, to join an arm to.");
        slide(ui, cx, "Softness", &mut h.softness, 0.0..=1.0, percent, "How round the joins are where the fingers and pads meet the palm.");
    });
    theme::category(ui, "Each Finger", ACCENT_HANDS, "", false, |ui| {
        theme::hint(ui, "How far each one curls: 0% straight, 100% closed.");
        for i in 0..n {
            let label = finger_name(i, n);
            slide(ui, cx, &label, &mut h.curl[i], 0.0..=1.0, percent, "How far this finger curls toward the palm.");
        }
    });
    fresh
}

fn foot_sliders(ui: &mut egui::Ui, cx: &Ctx, f: &mut FootShape) -> bool {
    let mut fresh = false;
    let n = f.toes.clamp(1, MAX_DIGITS as u32) as usize;
    theme::category(ui, "Toes", ACCENT_HANDS, &format!("{n}"), true, |ui| {
        slide_count(ui, cx, "Toes", &mut f.toes, 1..=MAX_DIGITS as u32, "How many toes.");
        slide(ui, cx, "Length", &mut f.toe_length, 0.4..=2.0, percent, "Toe length against an ordinary foot's.");
        slide(ui, cx, "Thickness", &mut f.toe_thickness, 0.5..=2.0, percent, "How thick the toes are.");
        slide(ui, cx, "Big Toe", &mut f.big_toe, 0.6..=1.6, percent, "How big the big toe is against the others.");
        slide(ui, cx, "Taper", &mut f.taper, 0.0..=1.0, percent, "How much thinner the tips are than the bases.");
        slide(ui, cx, "Spread", &mut f.spread, 0.0..=1.0, percent, "How far the toes fan apart.");
        let curls = &mut f.curl[..n];
        slide_all(ui, cx, "Curl", curls, -0.6..=1.0, "How far every toe curls under; below 0% lifts them. Set toes one by one under Each Toe.");
        slide(ui, cx, "Knuckles", &mut f.knuckles, 0.0..=1.0, percent, "How much the toe joints and ankle bones stand out.");
        slide(ui, cx, "Claws", &mut f.claws, 0.0..=1.0, percent, "Claws growing from the tips. 0% is none.");
    });
    theme::category(ui, "Foot & Ankle", ACCENT_HANDS, "", true, |ui| {
        slide(ui, cx, "Width", &mut f.width, 0.6..=1.6, percent, "How wide the foot is across the ball.");
        slide(ui, cx, "Heel", &mut f.heel, 0.6..=1.5, percent, "How big the heel is.");
        slide(ui, cx, "Arch", &mut f.arch, 0.0..=1.0, percent, "How high the inside of the sole lifts off the ground: 0% is a flat foot.");
        slide(ui, cx, "Ankle Length", &mut f.ankle, 0.0..=1.0, percent, "How much leg comes with the foot, to join a leg to.");
        slide(ui, cx, "Ankle Bend", &mut f.ankle_bend, -1.0..=1.0, |v| format!("{:.0}°", v * 34.4), "Leans the leg forward, or back to point the foot.");
        slide(ui, cx, "Softness", &mut f.softness, 0.0..=1.0, percent, "How round the joins are.");
        let mut flat = f.flat_sole;
        let r = theme::checkbox(ui, &mut flat, "Flat Sole", ACCENT_HANDS);
        if theme::tip(r, cx.tips, "Cuts the sole level, so a figure can stand on it.").changed() {
            f.flat_sole = flat;
            fresh = true;
        }
    });
    theme::category(ui, "Each Toe", ACCENT_HANDS, "", false, |ui| {
        theme::hint(ui, "How far each one curls under; below 0% lifts it.");
        for i in 0..n {
            let label = toe_name(i, n);
            slide(ui, cx, &label, &mut f.curl[i], -0.6..=1.0, percent, "How far this toe curls under.");
        }
    });
    fresh
}
