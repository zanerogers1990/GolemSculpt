//! The options of the Smooth, Flatten, Clay and Inflate kernels in a brush's
//! settings card: what Smooth evens out and how much volume it keeps, which
//! side of its plane Flatten works on and whether that plane is locked for
//! the stroke, how steady the plane Clay and Flatten work to is, and how far
//! Inflate evens out the way it pushes. They are the brush's own
//! (`sculpt::BrushSettings`).

use egui::Ui;

use super::theme::{self, ACCENT, ButtonStyle};
use crate::sculpt::{BrushSettings, FlattenMode, SmoothMode};

/// A row of buttons, one lit: the mode to pick from `all`.
fn mode_row<M: Copy + PartialEq>(
    ui: &mut Ui,
    picked: &mut M,
    all: &[M],
    name: impl Fn(M) -> &'static str,
    help: impl Fn(M) -> &'static str,
    tips: bool,
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let w = theme::row_width(ui, all.len(), 4.0);
        for &mode in all {
            let style = ButtonStyle { active: (*picked == mode).then_some(ACCENT()), ..Default::default() };
            if theme::tip(theme::button(ui, w, name(mode), style), tips, help(mode)).clicked() {
                *picked = mode;
            }
        }
    });
}

/// Smooth's own settings. They are Shift + drag's too, whatever brush is in
/// hand: that is this brush.
pub fn smooth(ui: &mut Ui, bs: &mut BrushSettings, tips: bool) {
    mode_row(ui, &mut bs.smooth_mode, &SmoothMode::ALL, SmoothMode::name, SmoothMode::tip, tips);
    if bs.smooth_mode == SmoothMode::All {
        theme::tip(
            theme::slider(ui, "Keep Volume", &mut bs.keep_volume, 0.0..=1.0, |v| format!("{:.0}%", v * 100.0)),
            tips,
            "Smoothing makes a form thinner every time: a finger, a fin's edge, a small ball. This gives that back. \
             At 100% the form keeps its thickness while its bumps still go; at 0% it shrinks as it always has.",
        );
    }
    theme::hint(ui, "These are also what Shift + drag does with any other brush.");
}

/// Steady Plane, for the brushes that work to a plane (Clay, Flatten).
pub fn steady_plane(ui: &mut Ui, bs: &mut BrushSettings, tips: bool) {
    theme::tip(
        theme::slider(ui, "Steady Plane", &mut bs.steady_plane, 0.0..=1.0, |v| format!("{:.0}%", v * 100.0)),
        tips,
        "Keeps the level the brush works to steady along the stroke, so it does not tip and bob over every bump          the brush passes: a smoother, more even strip. At 0% each dab finds its own level; at 100% the stroke          keeps the one it started with.",
    );
}

/// Relax Normals, for Inflate.
pub fn relax_normals(ui: &mut Ui, bs: &mut BrushSettings, tips: bool) {
    theme::tip(
        theme::slider(ui, "Relax Normals", &mut bs.relax_normals, 0.0..=1.0, |v| format!("{:.0}%", v * 100.0)),
        tips,
        "Evens out the directions Inflate pushes the surface in, so neighbouring bits swell together. Higher          keeps a swelling smooth over sharp or rough forms; at 0% every point pushes straight out its own way.",
    );
}

/// Flatten's mode and Lock Plane, under its Plane Offset.
pub fn flatten(ui: &mut Ui, bs: &mut BrushSettings, tips: bool) {
    mode_row(ui, &mut bs.flatten_mode, &FlattenMode::ALL, FlattenMode::name, FlattenMode::tip, tips);
    let label = if bs.lock_plane { "Lock Plane" } else { "Lock Plane (Off)" };
    theme::tip(
        theme::checkbox(ui, &mut bs.lock_plane, label, ACCENT()),
        tips,
        "On: the plane is fixed where the stroke begins, and the whole stroke levels to that one plane, for cutting a \
         true flat facet. It works on what lies within half the brush's size of the plane and leaves the rest \
         alone. Off: the plane follows the surface under the ring.",
    );
    if !bs.lock_plane {
        steady_plane(ui, bs, tips);
    }
}
