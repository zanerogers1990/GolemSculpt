//! The left panel while the tool is up: starting shapes, the knobs that have no natural gesture
//! (detail, closure, overall size), and Create. The shaping itself happens in the viewport.

use sculpt_toolkit::theme::{self, ACCENT, ButtonStyle};
use sculpt_toolkit::{Color32, Ctx};

use super::Lathe;
use crate::plane::{MAX_SCALE, MIN_SCALE};
use crate::{LathePreset, MAX_SEGMENTS, MIN_SEGMENTS, PREVIEW_MAX_SEGMENTS, round_segments};

/// The warm accent of the Blockout group the Unity panel sat in.
const ACCENT_LATHE: Color32 = Color32::from_rgb(242, 173, 77);
/// An open surface: not wrong, but not what sculpting wants.
const WARN: Color32 = Color32::from_rgb(250, 184, 89);

const RADIUS_RANGE: std::ops::RangeInclusive<f32> = 0.02..=3.0;
const HEIGHT_RANGE: std::ops::RangeInclusive<f32> = 0.02..=5.0;

const SHORTCUTS: &[(&str, &str)] = &[
    ("Drag a handle", "Reshape"),
    ("Shift + drag", "Straight"),
    ("Drag the curve", "Add a point"),
    ("Click beside it", "Extend the outline"),
    ("Ctrl + drag", "Sketch an outline"),
    ("Double-click", "Corner / smooth"),
    ("Right-click", "Delete a point"),
    ("Drag the axis", "Move the lathe"),
    ("Delete", "Delete the selected"),
    ("Enter", "Create the mesh"),
    ("Z / Shift+Z", "Undo / redo"),
    ("Esc", "Cancel, deselect, leave"),
    ("Drag background", "Orbit"),
];

/// 60480 as "60,480".
pub(super) fn grouped(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

impl Lathe {
    pub(super) fn show_panel(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        let summary = if self.built.has_mesh { format!("{} tris", theme::count(self.built.triangles)) } else { String::new() };
        theme::category(ui, "Lathe", ACCENT_LATHE, &summary, true, |ui| {
            self.top_row(ui, cx);
            match &self.job {
                Some(job) => theme::line(ui, &format!("Making: {}", job.progress()), theme::OK),
                None => theme::line(ui, "Shaping - drag the handles on the outline in the viewport.", theme::HINT),
            }
            self.presets(ui, cx);
            self.history_row(ui, cx);
            self.options(ui, cx);
            self.sliders(ui, cx);
            self.point_row(ui, cx);
            let r = theme::button(ui, ui.available_width(), "Move Axis to View Centre", ButtonStyle::default());
            let help = "Moves the lathe's axis to the middle of the view, keeping the shape - to build it where it will be used.";
            if theme::tip(r, cx.tips, help).clicked() {
                self.move_axis_to_view_centre(cx);
            }
            self.stats(ui);
        });
        theme::category(ui, "Lathe Shortcuts", theme::KEY, "", false, |ui| theme::shortcuts(ui, SHORTCUTS));
    }

    fn top_row(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, 2, 6.0);
            ui.add_enabled_ui(self.job.is_none() && self.built.has_mesh, |ui| {
                let style = ButtonStyle { icon: cx.icon("check"), active: Some(ACCENT), ..Default::default() };
                let r = theme::button(ui, w, "Create Mesh", style);
                let help = "Turns the shape into a real sculptable object (Enter). The profile is kept, so you can make variations.";
                if theme::tip(r, cx.tips, help).clicked() {
                    self.create(cx);
                }
            });
            let r = theme::button(ui, w, "Done", ButtonStyle { badge: "Esc", ..Default::default() });
            if theme::tip(r, cx.tips, "Puts the lathe away and goes back to sculpting. The profile is kept for next time.").clicked() {
                cx.finish();
            }
        });
    }

    fn presets(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        theme::heading(ui, "Start From");
        for row in LathePreset::ALL.chunks(3) {
            ui.horizontal(|ui| {
                let w = theme::row_width(ui, 3, 6.0);
                for &preset in row {
                    let r = theme::button(ui, w, preset.name(), ButtonStyle::default());
                    let help = format!(
                        "Replaces the profile with a {} to shape from. Undoable (Z).",
                        preset.name().to_lowercase()
                    );
                    if theme::tip(r, cx.tips, &help).clicked() {
                        self.apply_preset(cx, preset);
                    }
                }
            });
        }
    }

    fn history_row(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, 3, 6.0);
            let r = theme::button(ui, w, "Draw New", ButtonStyle::default());
            let help = "Empties the profile so you can draw your own: click beside the dashed axis to place points from bottom to top, or Ctrl-drag to sketch the outline in one stroke.";
            if theme::tip(r, cx.tips, help).clicked() {
                self.clear_profile(cx);
            }
            ui.add_enabled_ui(self.can_step(false), |ui| {
                let r = theme::button(ui, w, "Undo", ButtonStyle::default());
                if theme::tip(r, cx.tips, "Undoes the last profile edit (Z).").clicked() {
                    self.step(cx, false);
                }
            });
            ui.add_enabled_ui(self.can_step(true), |ui| {
                let r = theme::button(ui, w, "Redo", ButtonStyle::default());
                if theme::tip(r, cx.tips, "Redoes it (Shift+Z).").clicked() {
                    self.step(cx, true);
                }
            });
        });
        let text = match self.history.next_undo(&self.profile, &self.plane) {
            Some(label) => format!("Undo (Z): {label}"),
            None => "Profile history empty.".to_owned(),
        };
        theme::hint(ui, &text);
    }

    fn can_step(&self, redo: bool) -> bool {
        if redo { self.history.can_redo() } else { self.history.can_undo(&self.profile, &self.plane) }
    }

    fn options(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        let mut closed = self.profile.closed_loop();
        let r = theme::checkbox(ui, &mut closed, "Closed Loop (ring)", ACCENT_LATHE);
        let help = "Joins the last point back to the first. A loop that stays clear of the axis revolves into a ring or torus.";
        if theme::tip(r, cx.tips, help).changed() {
            self.set_closed_loop(cx, closed);
        }

        let mut caps = self.settings.cap_ends;
        let r = theme::checkbox(ui, &mut caps, "Cap Open Ends", ACCENT_LATHE);
        let help = "Closes an end that stops short of the axis with a flat disc, so the result is a watertight solid - what sculpting and Boolean want. Off leaves it open, like a tube.";
        if theme::tip(r, cx.tips, help).changed() {
            self.set_cap_ends(caps);
        }

        let mut even = self.settings.even_triangles;
        let r = theme::checkbox(ui, &mut even, "Even Triangles", ACCENT_LATHE);
        let help = "Uses fewer vertices on rings near the axis so every triangle is about the same size - no slivers at the poles, which brushes handle much better.";
        if theme::tip(r, cx.tips, help).changed() {
            self.set_even_triangles(even);
        }

        let mut radial = self.radial_on_create;
        let r = theme::checkbox(ui, &mut radial, "Radial Symmetry on Create", ACCENT_LATHE);
        let help = "Turns on radial symmetry round the lathe's axis when the object is created, so every stroke on it is repeated all the way round. Set how many repeats under Symmetry in the sculpting panel. Undoing the Create puts the symmetry back as it was.";
        if theme::tip(r, cx.tips, help).changed() {
            self.set_radial_on_create(radial);
        }
    }

    fn sliders(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        let mut segments = self.settings.radial_segments as f32;
        let readout = |v: f32| {
            let n = round_segments(v as usize);
            if n > PREVIEW_MAX_SEGMENTS { format!("{n} (preview {PREVIEW_MAX_SEGMENTS})") } else { n.to_string() }
        };
        let r = theme::slider(ui, "Density", &mut segments, MIN_SEGMENTS as f32..=MAX_SEGMENTS as f32, readout);
        let help = format!(
            "How finely Create divides the shape: vertices around the widest part, with the profile divided to match so faces stay square. Higher comes out smoother, ready to sculpt without smoothing. The preview stops at {PREVIEW_MAX_SEGMENTS} to stay responsive; Create always uses the full Density."
        );
        if theme::tip(r, cx.tips, &help).changed() {
            self.set_segments(segments.round() as usize);
        }

        let mut radius = self.radius().max(*RADIUS_RANGE.start());
        let r = theme::slider(ui, "Radius", &mut radius, RADIUS_RANGE, |v| format!("{v:.3}"));
        let help = "Radius of the widest point. Stretches the outline sideways, keeping its shape.";
        if theme::tip(r, cx.tips, help).changed() {
            self.set_radius(cx, radius);
        }

        let mut height = self.height().max(*HEIGHT_RANGE.start());
        let r = theme::slider(ui, "Height", &mut height, HEIGHT_RANGE, |v| format!("{v:.3}"));
        let help = "Height of the shape. Stretches the outline up from its base.";
        if theme::tip(r, cx.tips, help).changed() {
            self.set_height(cx, height);
        }

        let mut scale = self.plane.scale;
        let r = theme::slider(ui, "Scale", &mut scale, MIN_SCALE..=MAX_SCALE, |v| format!("{v:.2}x"));
        if theme::tip(r, cx.tips, "Overall size - scales everything together.").changed() {
            self.set_scale(cx, scale);
        }
    }

    fn point_row(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        theme::heading(ui, "Selected Point");
        let selected = self.selected.filter(|&i| i < self.profile.len());
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, 2, 6.0);
            ui.add_enabled_ui(selected.is_some(), |ui| {
                let sharp = selected.is_some_and(|i| self.profile[i].sharp);
                let r = theme::button(ui, w, if sharp { "Make Smooth" } else { "Make Corner" }, ButtonStyle::default());
                let help = "Switches the selected point between smooth and a sharp corner (double-click a handle does the same).";
                if theme::tip(r, cx.tips, help).clicked() {
                    self.toggle_selected_sharp(cx);
                }
                let style = ButtonStyle { badge: "Del", ..Default::default() };
                let r = theme::button(ui, w, "Delete Point", style);
                if theme::tip(r, cx.tips, "Removes the selected point (Delete, or right-click a handle).").clicked() {
                    self.delete_selected(cx);
                }
            });
        });
    }

    /// What Create will make, and what is wrong with the outline if anything is.
    fn stats(&self, ui: &mut egui::Ui) {
        if !self.built.has_mesh {
            theme::line(ui, self.built.error.as_deref().unwrap_or("No shape yet."), theme::HINT);
            return;
        }
        let watertight = self.build.watertight;
        let closure = if watertight { "closed solid" } else { "open surface - ends not closed" };
        let mut text = format!("Create: {} tris | {closure}", grouped(self.built.triangles));
        if self.built.draft {
            text.push_str(" (draft while dragging)");
        }
        let mut color = if watertight { theme::OK } else { WARN };
        if self.build.self_intersecting {
            text.push_str("\nThe outline crosses itself, so the surface passes through itself.");
            color = theme::ERROR;
        } else if self.build.touches_axis {
            text.push_str("\nThe curve swings across the axis between points - held just off it (a thin neck).");
            color = WARN;
        }
        theme::line(ui, &text, color);
    }
}

#[cfg(test)]
mod tests {
    use super::grouped;

    #[test]
    fn thousands_are_grouped_by_three() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1_000), "1,000");
        assert_eq!(grouped(60_480), "60,480");
        assert_eq!(grouped(1_234_567), "1,234,567");
    }
}
