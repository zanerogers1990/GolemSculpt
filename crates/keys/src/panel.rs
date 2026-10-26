//! The tool's panel: which pieces it has, the joint buttons, and the key's
//! shape, size and printing tolerances. Placing happens in the viewport
//! (`input.rs`); this is for the numbers.
//!
//! Lengths here are always millimetres, whatever unit the rest of the app is
//! showing: they are printer tolerances, and a slicer speaks millimetres.

use std::time::Duration;

use egui::Ui;
use sculpt_geom::units::{from_millimetres as mm, to_millimetres as to_mm};
use sculpt_toolkit::Ctx;
use sculpt_toolkit::theme::{self, ACCENT, ButtonStyle};

use crate::input::ViewSide;
use crate::pair::Showing;
use crate::planner::{KeySettings, KeySide};
use crate::preview::PEG;
use crate::profile::KeyShape;
use crate::tool::Keys;

const GAP: f32 = 6.0;

fn lit(on: bool) -> ButtonStyle<'static> {
    ButtonStyle { active: on.then_some(ACCENT()), ..Default::default() }
}

/// A slider over a length kept in world units and shown in millimetres.
/// Returns whether it was moved.
fn mm_slider(ui: &mut Ui, cx: &Ctx, label: &str, world: &mut f32, range: std::ops::RangeInclusive<f32>, decimals: usize, help: &str) -> bool {
    let mut value = to_mm(*world);
    let response = theme::slider(ui, label, &mut value, range, |v| format!("{v:.decimals$} mm"));
    let changed = theme::tip(response, cx.tips, help).changed();
    if changed {
        *world = mm(value);
    }
    changed
}

pub(crate) fn panel(keys: &mut Keys, ui: &mut Ui, cx: &mut Ctx) {
    let count = keys.key_count();
    let summary = match count {
        0 => String::new(),
        1 => "1 joint".to_string(),
        n => format!("{n} joints"),
    };
    theme::category(ui, "Keys", PEG, &summary, true, |ui| pieces(keys, ui, cx));

    let before = keys.settings;
    theme::category(ui, "Key Shape & Size", PEG, keys.settings.shape.name(), true, |ui| shape_and_size(keys, ui, cx));
    let clearance = format!("{:.2} mm", to_mm(keys.settings.clearance));
    theme::category(ui, "Fit (Printer Tolerances)", PEG, &clearance, false, |ui| fit(keys, ui, cx));
    if keys.settings != before {
        keys.settings_changed();
    }

    theme::category(ui, "Keys Shortcuts", theme::KEY, "", false, |ui| {
        theme::shortcuts(
            ui,
            &[
                ("Click the cut face", "Add a joint"),
                ("Drag a joint", "Move it"),
                ("Right-click / Delete", "Remove it"),
                ("Wheel over a joint", "Turn it (Shift: fine)"),
                ("R over a joint", "Turn it 90 degrees"),
                ("V", "Show pegs / sockets / both"),
                ("Enter", "Build the keys"),
                ("Ctrl + Z / Ctrl + Shift + Z", "Undo / redo a joint edit"),
                ("Esc", "Let go, stop a build, leave"),
                ("J", "Keys on / off"),
                ("Drag on background", "Orbit"),
            ],
        );
    });
}

/// Which pieces, how the joints stand, and the buttons that act on them all.
fn pieces(keys: &mut Keys, ui: &mut Ui, cx: &mut Ctx) {
    match keys.pair_line(cx) {
        Some(line) => {
            let unfit = keys.invalid_count();
            let unfit = if unfit > 0 { format!(", {unfit} will not fit") } else { String::new() };
            theme::line(ui, &format!("{line}: {} joint{}{unfit}", keys.key_count(), if keys.key_count() == 1 { "" } else { "s" }), theme::TEXT());
        }
        None => theme::line(ui, "No pieces yet. Split a model in two, select one half and press J.", theme::HINT()),
    }
    let notes = keys.notes();
    if !notes.is_empty() {
        theme::line(ui, &format!("{notes}."), theme::HINT());
    }

    let r = theme::checkbox(ui, &mut keys.auto_plan, "Plan joints automatically", PEG);
    theme::tip(
        r,
        cx.tips,
        "When the tool is given two pieces it places their joints itself: pegs on one half, matching sockets in the other, where the cut face is widest, with a minimum wall all round. Off, the face starts empty and every joint is placed by hand.",
    );

    if let Some(job) = &keys.job {
        theme::line(ui, &format!("{}...", job.progress()), theme::OK);
        let r = theme::button(ui, ui.available_width(), "Cancel", ButtonStyle { badge: "Esc", ..Default::default() });
        if theme::tip(r, cx.tips, "Stops the build. Nothing is changed.").clicked() {
            job.cancel();
        }
        return;
    }

    let editable = keys.editable().is_some();
    ui.add_enabled_ui(editable, |ui| {
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, 3, GAP);
            let r = theme::button(ui, w, "Auto Place", ButtonStyle::default());
            if theme::tip(r, cx.tips, "Throws the current joints away and places them afresh from the settings below.").clicked() {
                keys.auto_place(cx);
            }
            let r = theme::button(ui, w, "Clear", ButtonStyle::default());
            if theme::tip(r, cx.tips, "Removes every joint (Ctrl+Z brings them back).").clicked() {
                keys.clear_keys(cx);
            }
            let r = theme::button(ui, w, "Swap Sides", ButtonStyle::default());
            if theme::tip(r, cx.tips, "Puts the pegs on the other half and the sockets in this one.").clicked() {
                keys.flip_sides(cx);
            }
        });

        theme::heading(ui, "Show");
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, 3, GAP);
            let views = [
                (ViewSide::Pegs, "Show the half with the pegs (V cycles)."),
                (ViewSide::Holes, "Show the half with the sockets (V cycles)."),
                (ViewSide::Both, "Show both halves; the joints stay drawn over them (V cycles)."),
            ];
            for (view, help) in views {
                if theme::tip(theme::button(ui, w, view.name(), lit(keys.view == view)), cx.tips, help).clicked() {
                    keys.set_view(cx, view);
                }
            }
        });

        let ready = (keys.key_count() > 0).then_some(ACCENT());
        let style = ButtonStyle { icon: cx.icon("tool_keys"), active: ready, badge: "Enter", ..Default::default() };
        let r = theme::button(ui, ui.available_width(), "Build Keys", style);
        if theme::tip(r, cx.tips, "Builds the joints into both pieces: one undo step. Undo gives the planned joints back.").clicked() {
            keys.apply(cx, Duration::ZERO);
        }
    });
    if keys.pair.as_ref().is_some_and(|pair| pair.showing == Showing::Keyed) {
        theme::hint(ui, "These pieces have their keys. Undo (Ctrl+Z) takes them off and gives the planned joints back.");
    } else {
        theme::hint(ui, "Red joints will not fit where they are, and are left out of the build.");
    }
    let r = theme::button(ui, ui.available_width(), "Back to Sculpt", ButtonStyle { badge: "J", ..Default::default() });
    if theme::tip(r, cx.tips, "Puts the tool away (J). The planned joints are kept for next time.").clicked() {
        cx.finish();
    }
}

fn shape_and_size(keys: &mut Keys, ui: &mut Ui, cx: &mut Ctx) {
    theme::heading(ui, "Shape");
    let help = "The key's cross-section. A circle is simplest; every other shape is keyed - it only fits one way round, so one is enough to stop the halves twisting (two circles do the same).";
    for row in KeyShape::ALL.chunks(2) {
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, 2, GAP);
            for &shape in row {
                let style = ButtonStyle { height: 24.0, ..lit(keys.settings.shape == shape) };
                if theme::tip(theme::button(ui, w, shape.name(), style), cx.tips, help).clicked() {
                    keys.settings.shape = shape;
                }
            }
        });
    }

    // What automatic sizing chose, to show beside "auto".
    let planned = keys.pair.as_ref().and_then(|pair| pair.plan.main_joint()).map(|jp| (jp.size, jp.height));
    let s = &mut keys.settings;

    let mut auto = s.size <= 0.0;
    let r = theme::checkbox(ui, &mut auto, "Size: auto", PEG);
    if theme::tip(r, cx.tips, "Sized from the cut face: about half its narrowest width, 2-10 mm.").changed() {
        s.size = if auto { 0.0 } else { mm(keys.slider_size) };
    }
    let chosen = planned.filter(|_| auto).map(|(size, _)| to_mm(size));
    let readout = |v: f32| chosen.map_or(format!("{v:.1} mm"), |mm| format!("auto ({mm:.1} mm)"));
    let r = theme::slider(ui, "Size", &mut keys.slider_size, 1.5..=25.0, readout);
    if theme::tip(r, cx.tips, "The key's width in millimetres (a circle's diameter; a rectangle's short side).").changed() {
        s.size = mm(keys.slider_size);
    }

    let r = theme::slider(ui, "Rectangle length", &mut s.aspect, 1.1..=4.0, |v| format!("{v:.1}x width"));
    theme::tip(r, cx.tips, "A rectangle key's length, as a multiple of its width.");

    let mut auto = s.height <= 0.0;
    let r = theme::checkbox(ui, &mut auto, "Height: auto", PEG);
    let help = "As tall as the key is wide, less wherever the half behind the cut is too thin for the socket.";
    if theme::tip(r, cx.tips, help).changed() {
        s.height = if auto { 0.0 } else { mm(keys.slider_height) };
    }
    let chosen = planned.filter(|_| auto).map(|(_, height)| to_mm(height));
    let readout = |v: f32| chosen.map_or(format!("{v:.1} mm"), |mm| format!("auto ({mm:.1} mm)"));
    let r = theme::slider(ui, "Height", &mut keys.slider_height, 1.0..=20.0, readout);
    let help = "How far the peg stands out of its face, in millimetres. The socket is this plus the extra depth.";
    if theme::tip(r, cx.tips, help).changed() {
        s.height = mm(keys.slider_height);
    }

    theme::heading(ui, "Keys per cut");
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, 7, 3.0);
        ui.spacing_mut().item_spacing.x = 3.0;
        for count in 0..=6 {
            let label = if count == 0 { "Auto".to_string() } else { count.to_string() };
            let style = ButtonStyle { height: 24.0, ..lit(s.count.min(6) == count) };
            let help = "Auto puts two round keys (one can be a pivot), or one keyed shape - two on a long, narrow face.";
            if theme::tip(theme::button(ui, w, &label, style), cx.tips, help).clicked() {
                s.count = count;
            }
        }
    });

    let r = theme::slider(ui, "Turn", &mut s.rotation_degrees, 0.0..=180.0, |v| format!("{v:.0} deg"));
    theme::tip(r, cx.tips, "Turns the keys from the orientation placement picks (along the cut face's length).");

    theme::heading(ui, "Pegs on");
    let names = keys.pair.as_ref().map(|pair| pair.names(cx.scene()));
    let sides = [
        (KeySide::Auto, "Auto (sockets in the thicker half)".to_string()),
        (KeySide::PegOnFirst, names.as_ref().map_or("The selected piece".to_string(), |[a, _]| a.clone())),
        (KeySide::PegOnSecond, names.as_ref().map_or("The other piece".to_string(), |[_, b]| b.clone())),
    ];
    let help = "Which half gets the pegs. The sockets need depth behind them, so Auto sinks them into the thicker half.";
    for (side, label) in sides {
        let style = ButtonStyle { height: 24.0, ..lit(s.side == side) };
        if theme::tip(theme::button(ui, ui.available_width(), &label, style), cx.tips, help).clicked() {
            s.side = side;
        }
    }
}

fn fit(keys: &mut Keys, ui: &mut Ui, cx: &mut Ctx) {
    let s = &mut keys.settings;
    mm_slider(
        ui,
        cx,
        "Clearance per side",
        &mut s.clearance,
        0.0..=0.6,
        2,
        "Gap between peg and socket wall, per side. 0.1-0.2 mm is a snug fit on most FDM printers; resin wants less, a loose slip fit more. The socket is the peg's exact outline grown by this much.",
    );
    mm_slider(
        ui,
        cx,
        "Extra socket depth",
        &mut s.extra_depth,
        0.0..=2.0,
        2,
        "How much deeper the socket is than the peg is tall, so the peg never bottoms out and the faces close fully.",
    );
    mm_slider(ui, cx, "Peg tip chamfer", &mut s.tip_chamfer, 0.0..=1.5, 2, "A 45-degree bevel round the peg's tip, so it finds the socket.");
    mm_slider(
        ui,
        cx,
        "Socket entry chamfer",
        &mut s.entry_chamfer,
        0.0..=1.0,
        2,
        "A 45-degree bevel round the socket's mouth: a lead-in, and room for the flared first layer (elephant's foot) of a peg printed standing on its face.",
    );
    let draft = |v: f32| if v > 0.01 { format!("{v:.1} deg") } else { "none (straight walls)".to_string() };
    let r = theme::slider(ui, "Draft", &mut s.taper_degrees, 0.0..=5.0, draft);
    theme::tip(
        r,
        cx.tips,
        "Draft: peg and socket both narrow with depth by this angle, keeping the same clearance all the way down. Helps parts that must come apart again.",
    );
    mm_slider(
        ui,
        cx,
        "Minimum wall",
        &mut s.min_wall,
        0.4..=5.0,
        1,
        "The least material a socket may leave round itself, sideways and under its floor. Placement will not put a key anywhere it would break through.",
    );
    mm_slider(
        ui,
        cx,
        "Corner rounding",
        &mut s.corner_radius,
        0.2..=2.0,
        2,
        "Rounding on the corners of square, hex, star and the other angular shapes. No corner is ever sharper than the chamfers need, whatever this says.",
    );
    let r = theme::button(ui, ui.available_width(), "Reset to Defaults", ButtonStyle::default());
    let help = "0.15 mm clearance, 0.4 mm extra depth, 0.4 / 0.3 mm chamfers, 1.2 mm walls - FDM defaults for a 0.4 mm nozzle.";
    if theme::tip(r, cx.tips, help).clicked() {
        *s = KeySettings::default();
    }
}
