//! The tool's panel: the knobs with no natural gesture, placement, and
//! finishing the tubes. Drawing and point editing happen in the viewport
//! (see `input`).

use egui::Color32;
use sculpt_geom::units;
use sculpt_toolkit::theme::{self, ACCENT, ButtonStyle};
use sculpt_toolkit::{Ctx, Env};

use crate::tool::TubeTool;
use crate::types::{CapStyle, TubeSettings};

const ACCENT_TUBE: Color32 = Color32::from_rgb(255, 163, 56);

/// The radius slider works on a log scale: tubes run from threads to trunks.
const MIN_LOG_RADIUS: f32 = -7.0;
const MAX_LOG_RADIUS: f32 = 1.1;

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

/// What the brushes' symmetry is, as the Symmetry card says it ("X + Z + Radial 8"); `None` while it is off.
fn symmetry_summary(env: &Env) -> Option<String> {
    let mut parts: Vec<String> =
        ["X", "Y", "Z"].into_iter().zip(env.mirror).filter(|&(_, on)| on).map(|(name, _)| name.to_owned()).collect();
    if env.radial_count > 1 {
        parts.push(format!("Radial {}", env.radial_count));
    }
    (!parts.is_empty()).then(|| parts.join(" + "))
}

#[cfg(test)]
mod tests {
    use glam::Vec3;
    use sculpt_toolkit::testing::Bench;

    use super::symmetry_summary;

    #[test]
    fn the_panel_says_what_symmetry_the_tubes_are_made_under() {
        let mut env = Bench::new().env;
        assert_eq!(symmetry_summary(&env), None);
        env.mirror = [true, false, true];
        assert_eq!(symmetry_summary(&env).as_deref(), Some("X + Z"));
        (env.radial_count, env.radial_axis) = (6, Vec3::X);
        assert_eq!(symmetry_summary(&env).as_deref(), Some("X + Z + Radial 6"));
        env.mirror = [false; 3];
        assert_eq!(symmetry_summary(&env).as_deref(), Some("Radial 6"));
    }
}

/// Two buttons side by side, the one that is on lit. Returns which was
/// clicked.
fn pair(ui: &mut egui::Ui, cx: &Ctx, first_on: bool, labels: [&str; 2], tips: [&str; 2]) -> Option<usize> {
    let mut clicked = None;
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, 2, 6.0);
        for (i, (label, tip)) in labels.into_iter().zip(tips).enumerate() {
            let on = (i == 0) == first_on;
            let style = ButtonStyle { active: on.then_some(ACCENT), height: 24.0, ..Default::default() };
            if theme::tip(theme::button(ui, w, label, style), cx.tips, tip).clicked() {
                clicked = Some(i);
            }
        }
    });
    clicked
}

impl TubeTool {
    pub(crate) fn show_panel(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        self.now = cx.time;
        let count = self.tube_count();
        let summary = if count == 0 { String::new() } else { format!("{count} live") };
        theme::category(ui, "Tube", ACCENT_TUBE, &summary, true, |ui| {
            self.finish_rows(ui, cx);
            self.shape_controls(ui, cx);
            self.point_controls(ui, cx);
            self.edit_rows(ui, cx);
            let style = ButtonStyle { badge: "T", ..Default::default() };
            let r = theme::button(ui, ui.available_width(), "Back to Sculpt", style);
            if theme::tip(r, cx.tips, "Put the tube tool away (T). Live tubes are still there next time.").clicked() {
                cx.finish();
            }
        });
        theme::category(ui, "Tube Shortcuts", Color32::from_rgb(230, 204, 102), "", false, |ui| {
            theme::shortcuts(
                ui,
                &[
                    ("Drag from the model", "Draw a tube"),
                    ("Ctrl + drag", "Draw in mid-air"),
                    ("Drag a point", "Move it"),
                    ("Shift + drag an end", "Extend the tube"),
                    ("Click the tube", "Add a point"),
                    ("Double-click a point", "Sharp / smooth"),
                    ("Right-click a point", "Delete it"),
                    ("Drag the square", "Thicker / thinner there"),
                    ("Wheel over a point", "Thicker / thinner"),
                    ("S + move sideways", "Size point, else tube"),
                    ("Delete", "Delete point, else tube"),
                    ("Enter", "Merge"),
                    ("Z / Shift + Z", "Undo / redo the tubes"),
                    ("Esc", "Cancel, deselect, leave"),
                    ("Drag on background", "Orbit"),
                ],
            );
        });
        self.flush(cx);
    }

    /// Merge and Keep Separate, then what they and everything else last
    /// said.
    fn finish_rows(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        let (count, mergeable) = (self.tube_count(), self.mergeable_count());
        let merging = self.is_merging();
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, 2, 6.0);
            ui.add_enabled_ui(mergeable > 0 && !merging, |ui| {
                let style = ButtonStyle { active: Some(ACCENT), badge: "Enter", ..Default::default() };
                let help = "Fuses every live tube into the object it was drawn on: one solid surface with a smooth join. \
                            The object is rebuilt at its current density. Undo (Z) gives the tubes back.";
                if theme::tip(theme::button(ui, w, "Merge", style), cx.tips, help).clicked() {
                    self.merge_all(cx);
                }
            });
            ui.add_enabled_ui(count > 0 && !merging, |ui| {
                let help = "Makes each live tube its own sculptable object, placed with the object it was drawn on. \
                            Undo (Z) gives the tubes back.";
                if theme::tip(theme::button(ui, w, "Keep Separate", ButtonStyle::default()), cx.tips, help).clicked() {
                    self.keep_separate(cx);
                }
            });
        });
        if let Some(progress) = self.finish.progress() {
            theme::line(ui, &format!("Merging: {progress}"), theme::OK);
            let r = theme::button(ui, ui.available_width(), "Cancel Merge", ButtonStyle { badge: "Esc", ..Default::default() });
            if theme::tip(r, cx.tips, "Stops the merge. The tubes stay live.").clicked() {
                self.cancel_merge();
            }
        } else if !self.status.is_empty() {
            theme::line(ui, &self.status, theme::OK);
        } else {
            theme::hint(ui, "Drag from the model to grow a tube out of it.");
        }
        let line = match count {
            0 => "No live tubes.".to_owned(),
            _ if mergeable < count => {
                format!("{count} live tube{}, {} in empty space (Keep Separate only).", plural(count), count - mergeable)
            }
            _ => format!("{count} live tube{}.", plural(count)),
        };
        theme::line(ui, &line, theme::HINT);
        if let Some(symmetry) = symmetry_summary(cx) {
            let text = format!("Symmetry: {symmetry}. Each tube is made under it (the Symmetry card of the Sculpt panel).");
            theme::hint(ui, &text);
        }
    }

    /// Placement, ends and the whole-tube sliders. They edit the selected
    /// tube, and always the next one drawn.
    fn shape_controls(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        let settings = self.settings();

        theme::heading(ui, "Placement");
        let tips = [
            "The stroke sticks to the model, half sunk into it: veins, ridges, tails draped along a body. \
             Dragged points slide over the surface.",
            "The stroke is drawn on a plane facing you. Start it on the model and the tube grows out of that spot: \
             horns, arms, legs.",
        ];
        if let Some(i) = pair(ui, cx, self.on_surface(), ["On Surface", "Free"], tips) {
            self.set_on_surface(i == 0);
        }

        theme::heading(ui, "Ends");
        let tips = ["Soft domed ends.", "Ends cut square across the tube."];
        if let Some(i) = pair(ui, cx, settings.caps == CapStyle::Round, ["Round", "Flat"], tips) {
            self.set_caps(if i == 0 { CapStyle::Round } else { CapStyle::Flat });
        }

        let unit = cx.unit;
        let label = if self.active_tube().is_some() { "Radius" } else { "Radius (next tube)" };
        let mut log_radius = settings.radius.max(TubeSettings::MIN_RADIUS).ln();
        let r = theme::slider(ui, label, &mut log_radius, MIN_LOG_RADIUS..=MAX_LOG_RADIUS, |v| units::format(v.exp(), unit, true));
        if r.changed() {
            self.set_radius(log_radius.exp());
        }
        let help = "Thickness of the selected tube (and of the next one you draw). Hold S and move sideways in the viewport \
                    for the same.";
        theme::tip(r, cx.tips, help);

        let mut taper = settings.taper;
        let r = theme::slider(ui, "Taper", &mut taper, 0.0..=1.0, |v| format!("{:.0}%", v * 100.0));
        if r.changed() {
            self.set_taper(taper);
        }
        theme::tip(r, cx.tips, "Narrows the tube toward its end: 0 is even thickness, 1 comes to a point, for a horn or a claw.");

        let mut flip = settings.flip_taper;
        let r = theme::checkbox(ui, &mut flip, "Thick End at the Tip", ACCENT);
        if r.changed() {
            self.set_flip_taper(flip);
        }
        theme::tip(r, cx.tips, "Tapers toward where the stroke started instead of where it ended.");

        let mut smoothness = settings.smoothness;
        let r = theme::slider(ui, "Smoothness", &mut smoothness, 0.0..=1.0, |v| format!("{:.0}%", v * 100.0));
        if r.changed() {
            self.set_smoothness(smoothness);
        }
        let help = "How the tube flows between its points: low is straight runs with a kink at each point, high is loose, \
                    round curves. Never moves a point.";
        theme::tip(r, cx.tips, help);

        let mut blend = settings.blend;
        let readout = |v: f32| if v <= 0.0 { "crisp seam".to_owned() } else { format!("{v:.2}x radius") };
        let r = theme::slider(ui, "Blend", &mut blend, 0.0..=TubeSettings::MAX_BLEND, readout);
        if r.changed() {
            self.set_blend(blend);
        }
        let help = "How soft the join is when merged: 0 is a crisp seam, higher grows a smooth fillet where the tube meets \
                    the model.";
        theme::tip(r, cx.tips, help);
    }

    fn point_controls(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        theme::heading(ui, "Selected Point");
        let selected = self.selected_point().is_some();
        let sharp = self.selected_is_sharp();
        let line = match (selected, sharp) {
            (false, _) => "Click a point in the viewport.".to_owned(),
            (true, false) => format!("Size {:.2}x", self.selected_point_size()),
            (true, true) => format!("Size {:.2}x, sharp", self.selected_point_size()),
        };
        theme::line(ui, &line, theme::HINT);
        ui.add_enabled_ui(selected, |ui| {
            let mut log_size = self.selected_point_size().ln();
            let range = TubeSettings::MIN_SIZE.ln()..=TubeSettings::MAX_SIZE.ln();
            let r = theme::slider(ui, "Point Size", &mut log_size, range, |v| format!("{:.2}x", v.exp()));
            if r.changed() {
                self.set_selected_point_size(log_size.exp());
            }
            let help = "Thickness at the selected point, as a multiple of the tube's radius. Dragging the point's square \
                        handle, or the wheel over the point, does the same.";
            theme::tip(r, cx.tips, help);
            ui.horizontal(|ui| {
                let w = theme::row_width(ui, 2, 6.0);
                let label = if sharp { "Make Smooth" } else { "Make Sharp" };
                let r = theme::button(ui, w, label, ButtonStyle { height: 24.0, ..Default::default() });
                let help = "Switches the selected point between a smooth curve and a sharp corner (double-click a point \
                            does the same).";
                if theme::tip(r, cx.tips, help).clicked() {
                    self.toggle_selected_sharp();
                }
                let style = ButtonStyle { badge: "Del", height: 24.0, ..Default::default() };
                let r = theme::button(ui, w, "Delete Point", style);
                if theme::tip(r, cx.tips, "Removes the selected point (Delete, or right-click a point).").clicked() {
                    self.delete_selected_point();
                }
            });
        });
    }

    fn edit_rows(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, 3, 6.0);
            let small = || ButtonStyle { height: 24.0, ..Default::default() };
            ui.add_enabled_ui(self.active_tube().is_some(), |ui| {
                if theme::tip(theme::button(ui, w, "Delete Tube", small()), cx.tips, "Removes the selected tube.").clicked() {
                    self.delete_active_tube();
                }
            });
            ui.add_enabled_ui(self.can_undo(), |ui| {
                if theme::tip(theme::button(ui, w, "Undo", small()), cx.tips, "Undoes the last tube edit (Z).").clicked() {
                    self.undo();
                }
            });
            ui.add_enabled_ui(self.can_redo(), |ui| {
                if theme::tip(theme::button(ui, w, "Redo", small()), cx.tips, "Redoes it (Shift + Z).").clicked() {
                    self.redo();
                }
            });
        });
        match self.next_undo_label() {
            Some(label) => theme::hint(ui, &format!("Undo (Z): {label}")),
            None => theme::hint(ui, "Tube history empty."),
        }
    }
}
