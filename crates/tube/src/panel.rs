//! The tool's panel: the knobs with no natural gesture, placement, and
//! finishing the tubes. Drawing and point editing happen in the viewport
//! (see `input`).

use egui::Color32;
use sculpt_geom::units;
use sculpt_toolkit::theme::{self, ACCENT, ButtonStyle};
use sculpt_toolkit::{Command, Ctx, Env, ObjectId};

use crate::tool::TubeTool;
use crate::types::{CapStyle, Kind, TubeSettings};

const ACCENT_TUBE: Color32 = Color32::from_rgb(255, 163, 56);
const ACCENT_MUSCLE: Color32 = Color32::from_rgb(226, 104, 92);

/// The radius slider works on a log scale: tubes run from threads to trunks.
const MIN_LOG_RADIUS: f32 = -7.0;
const MAX_LOG_RADIUS: f32 = 1.1;

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

/// What the selected object's symmetry is, as the Symmetry card says it ("X + Z + Radial 8", with "(World)"
/// after it when it is fixed in the world); `None` while it is off.
fn symmetry_summary(env: &Env) -> Option<String> {
    let mut parts: Vec<String> =
        ["X", "Y", "Z"].into_iter().zip(env.mirror).filter(|&(_, on)| on).map(|(name, _)| name.to_owned()).collect();
    if env.radial_count > 1 {
        parts.push(format!("Radial {}", env.radial_count));
    }
    let space = if env.symmetry_world.is_some() { " (World)" } else { "" };
    (!parts.is_empty()).then(|| parts.join(" + ") + space)
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
            let style = ButtonStyle { active: on.then_some(ACCENT()), height: 24.0, ..Default::default() };
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
        let kind = self.kind();
        let muscle = kind == Kind::Muscle;
        let (one, title) = (kind.one(), kind.title());
        let key = if muscle { "B" } else { "T" };
        let count = self.tube_count();
        let summary = if count == 0 { String::new() } else { format!("{count} live") };
        theme::category(ui, title, if muscle { ACCENT_MUSCLE } else { ACCENT_TUBE }, &summary, true, |ui| {
            self.finish_rows(ui, cx);
            self.mirror_row(ui, cx);
            if muscle {
                self.muscle_controls(ui, cx);
            } else {
                self.shape_controls(ui, cx);
            }
            self.point_controls(ui, cx);
            self.edit_rows(ui, cx);
            let style = ButtonStyle { badge: key, ..Default::default() };
            let r = theme::button(ui, ui.available_width(), "Back to Sculpt", style);
            let help = format!("Put the {one} tool away ({key}). Live {} are still there next time.", kind.many());
            if theme::tip(r, cx.tips, &help).clicked() {
                cx.finish();
            }
        });
        theme::category(ui, &format!("{title} Shortcuts"), Color32::from_rgb(230, 204, 102), "", false, |ui| {
            let draw = format!("Draw a {one}");
            let extend = format!("Extend the {one}");
            let add = format!("Click the {one}");
            let size = format!("Size point, else {one}");
            let delete = format!("Delete point, else {one}");
            let history = format!("Undo / redo the {}", kind.many());
            let (from, mid_air) = if muscle {
                ("Drag across the model", "Draw floating, anywhere")
            } else {
                ("Drag from the model", "Draw in mid-air")
            };
            let squares: &[(&str, &str)] = if muscle {
                &[("Drag a square", "Move that edge, not the other"), ("Shift + drag a square", "Wider / narrower evenly")]
            } else {
                &[("Drag the square", "Thicker / thinner there")]
            };
            let mut keys = vec![
                (from, draw.as_str()),
                ("Ctrl + drag", mid_air),
                ("Drag a point", "Move it"),
                ("Shift + drag an end", extend.as_str()),
                (add.as_str(), "Add a point"),
                ("Double-click a point", "Sharp / smooth"),
                ("Right-click a point", "Delete it"),
            ];
            keys.extend_from_slice(squares);
            keys.extend([
                ("Wheel over a point", "Thicker / thinner"),
                ("S + move sideways", size.as_str()),
                ("Delete", delete.as_str()),
                ("Enter", "Merge"),
                ("Ctrl + Z / Ctrl + Shift + Z", history.as_str()),
                ("Esc", "Cancel, deselect, leave"),
                ("Drag on background", "Orbit"),
            ]);
            theme::shortcuts(ui, &keys);
        });
        self.flush(cx);
    }

    /// Merge and Keep Separate, then what they and everything else last
    /// said.
    fn finish_rows(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        let kind = self.kind();
        let (one, many) = (kind.one(), kind.many());
        let (count, mergeable) = (self.tube_count(), self.mergeable_count());
        let merging = self.is_merging();
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, 2, 6.0);
            ui.add_enabled_ui(mergeable > 0 && !merging, |ui| {
                let style = ButtonStyle { active: Some(ACCENT()), badge: "Enter", ..Default::default() };
                let help = format!(
                    "Fuses every live {one} into the object it was drawn on: one solid surface with a smooth join. The \
                     object is rebuilt at its current density. Undo (Ctrl+Z) gives the {many} back."
                );
                if theme::tip(theme::button(ui, w, "Merge", style), cx.tips, &help).clicked() {
                    self.merge_all(cx);
                }
            });
            ui.add_enabled_ui(count > 0 && !merging, |ui| {
                let help = format!(
                    "Makes each live {one} its own sculptable object, placed with the object it was drawn on. Undo (Ctrl+Z) \
                     gives the {many} back."
                );
                if theme::tip(theme::button(ui, w, "Keep Separate", ButtonStyle::default()), cx.tips, &help).clicked() {
                    self.keep_separate(cx);
                }
            });
        });
        if let Some(progress) = self.finish.progress() {
            theme::line(ui, &format!("Merging: {progress}"), theme::OK);
            let r = theme::button(ui, ui.available_width(), "Cancel Merge", ButtonStyle { badge: "Esc", ..Default::default() });
            if theme::tip(r, cx.tips, &format!("Stops the merge. The {many} stay live.")).clicked() {
                self.cancel_merge();
            }
        } else if !self.status.is_empty() {
            theme::line(ui, &self.status, theme::OK);
        } else {
            theme::hint(ui, self.invitation());
        }
        let line = match count {
            0 => format!("No live {many}."),
            _ if mergeable < count => {
                format!("{count} live {one}{}, {} in empty space (Keep Separate only).", plural(count), count - mergeable)
            }
            _ => format!("{count} live {one}{}.", plural(count)),
        };
        theme::line(ui, &line, theme::HINT());
    }

    /// The object whose mirror axes the Mirror buttons switch: the one the
    /// selected tube was drawn on, else the selected object, which is what
    /// the next tube is drawn on when it starts off the model.
    pub(crate) fn mirror_target(&self, cx: &Ctx) -> ObjectId {
        let anchor = self.active_tube().and_then(|tube| self.tubes[tube].state.anchor);
        anchor.filter(|&a| cx.scene().exists(a)).unwrap_or_else(|| cx.scene().active())
    }

    /// Switches mirroring across one axis (0 X, 1 Y, 2 Z) on the object the
    /// tubes are made on. It is the Symmetry card's setting, so the brushes
    /// on that object follow it too, and like those ticks it is not an undo
    /// step. That object is selected first if it was not.
    pub(crate) fn toggle_mirror(&mut self, cx: &mut Ctx, axis: usize) {
        let target = self.mirror_target(cx);
        if target != cx.scene().active() {
            cx.commit("Select", vec![Command::Select { object: target }]);
        }
        let mut mirror = cx.mirror;
        mirror[axis] = !mirror[axis];
        cx.set_mirror(mirror);
        let name = cx.scene().info(target).map(|info| info.name.to_owned()).unwrap_or_default();
        let (on, many, axis) = (mirror[axis], self.kind().many(), ["X", "Y", "Z"][axis]);
        self.say(if on {
            format!("Mirror {axis} on for {name}: its {many} are made mirrored across {axis}.")
        } else if mirror.contains(&true) {
            format!("Mirror {axis} off for {name}.")
        } else {
            format!("Mirror off for {name}: its {many} are made once.")
        });
    }

    /// Mirror X / Y / Z for the object the tubes are made on, and what
    /// symmetry that is.
    fn mirror_row(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        let kind = self.kind();
        let (one, many) = (kind.one(), kind.many());
        theme::heading(ui, "Mirror");
        let target = self.mirror_target(cx);
        let mirror = cx.symmetry_of(target).mirror;
        let mut clicked = None;
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, 3, 6.0);
            // Whole sentences, each its own literal: a tooltip is translated by its whole English text.
            let tips = [
                "Mirror across X: what you draw is also made on the other side, left to right. X, Y and Z can be on \
                 together. It is the drawn-on object's symmetry, so its brushes mirror the same way.",
                "Mirror across Y: what you draw is also made on the other side, top to bottom. X, Y and Z can be on \
                 together. It is the drawn-on object's symmetry, so its brushes mirror the same way.",
                "Mirror across Z: what you draw is also made on the other side, front to back. X, Y and Z can be on \
                 together. It is the drawn-on object's symmetry, so its brushes mirror the same way.",
            ];
            for (axis, (label, tip)) in ["X", "Y", "Z"].into_iter().zip(tips).enumerate() {
                let style = ButtonStyle { active: mirror[axis].then_some(ACCENT()), height: 24.0, ..Default::default() };
                if theme::tip(theme::button(ui, w, label, style), cx.tips, tip).clicked() {
                    clicked = Some(axis);
                }
            }
        });
        if let Some(axis) = clicked {
            self.toggle_mirror(cx, axis);
        }
        let elsewhere = (target != cx.scene().active()).then(|| cx.scene().info(target).map(|info| info.name.to_owned()));
        let text = match (elsewhere.flatten(), symmetry_summary(cx)) {
            (Some(name), _) => format!("For {name}, the object the selected {one} is on. Clicking selects it."),
            (None, Some(symmetry)) => format!(
                "Symmetry of the selected object: {symmetry}. The {many} drawn on it are made under it (the Symmetry \
                 card on the Sculpt panel's Mesh tab shows the same)."
            ),
            (None, None) => format!("Mirror is off for the selected object: each {one} is made once."),
        };
        theme::hint(ui, &text);
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
        let r = theme::checkbox(ui, &mut flip, "Thick End at the Tip", ACCENT());
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

    /// A muscle's placement and shape. They edit the selected muscle, and always the next one drawn.
    fn muscle_controls(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        let settings = self.settings();
        let percent = |v: f32| format!("{:.0}%", v * 100.0);

        theme::heading(ui, "Placement");
        let tips = [
            "The muscle lies on the model along its whole length, sunk in by Depth. A stroke that starts just off the \
             model is pulled onto it. Dragged points slide over the surface.",
            "The muscle floats on its own, drawn on a plane facing you: in front of the model where the stroke starts \
             on it, or anywhere with Ctrl. A dragged point lifts off the surface and floats too.",
        ];
        if let Some(i) = pair(ui, cx, self.on_surface(), ["On Surface", "Free"], tips) {
            self.set_on_surface(i == 0);
        }

        theme::heading(ui, "Shape");
        let unit = cx.unit;
        let label = if self.active_tube().is_some() { "Width" } else { "Width (next muscle)" };
        let mut log_radius = settings.radius.max(TubeSettings::MIN_RADIUS).ln();
        let r = theme::slider(ui, label, &mut log_radius, MIN_LOG_RADIUS..=MAX_LOG_RADIUS, |v| {
            units::format(2.0 * v.exp(), unit, true)
        });
        if r.changed() {
            self.set_radius(log_radius.exp());
        }
        let help = "How wide the selected muscle is at its belly (and the next one you draw). Hold S and move sideways in \
                    the viewport for the same.";
        theme::tip(r, cx.tips, help);

        let mut swell = settings.swell;
        let r = theme::slider(ui, "Swell", &mut swell, 0.0..=1.0, percent);
        if r.changed() {
            self.set_swell(swell);
        }
        theme::tip(r, cx.tips, "How full the muscle is: low is a lean spindle, high stays wide almost to its ends.");

        let mut belly = settings.belly;
        let r = theme::slider(ui, "Belly", &mut belly, 0.1..=0.9, percent);
        if r.changed() {
            self.set_belly(belly);
        }
        theme::tip(r, cx.tips, "Where along the muscle its widest part sits: 50% is the middle.");

        let mut taper = settings.taper;
        let r = theme::slider(ui, "Taper", &mut taper, 0.0..=1.0, percent);
        if r.changed() {
            self.set_taper(taper);
        }
        theme::tip(r, cx.tips, "How thin the two ends are: 0 is as wide as the belly, 100% comes to a point.");

        let mut flatten = settings.flatten;
        let r = theme::slider(ui, "Thickness", &mut flatten, TubeSettings::MIN_FLATTEN..=1.0, |v| {
            if v >= 0.995 { "round".to_owned() } else { format!("{:.0}% of width", v * 100.0) }
        });
        if r.changed() {
            self.set_flatten(flatten);
        }
        theme::tip(r, cx.tips, "How deep the muscle is for its width: low is a flat strap, 100% is round.");

        let mut depth = settings.depth;
        let r = theme::slider(ui, "Depth", &mut depth, 0.0..=TubeSettings::MAX_DEPTH, |v| {
            if v <= 0.02 { "on top".to_owned() } else { format!("{:.0}% sunk", v * 100.0) }
        });
        if r.changed() {
            self.set_depth(depth);
        }
        let help = "How far the muscle is sunk into the model: from resting on top to grown out of it. Only where it lies \
                    on the surface.";
        theme::tip(r, cx.tips, help);

        let mut smoothness = settings.smoothness;
        let r = theme::slider(ui, "Smoothness", &mut smoothness, 0.0..=1.0, percent);
        if r.changed() {
            self.set_smoothness(smoothness);
        }
        let help = "How the muscle flows between its points: low is straight runs with a kink at each point, high is \
                    loose, round curves. Never moves a point.";
        theme::tip(r, cx.tips, help);

        let mut blend = settings.blend;
        let readout = |v: f32| if v <= 0.0 { "crisp seam".to_owned() } else { format!("{v:.2}x height") };
        let r = theme::slider(ui, "Blend", &mut blend, 0.0..=TubeSettings::MAX_BLEND, readout);
        if r.changed() {
            self.set_blend(blend);
        }
        let help = "How soft the join is when merged, against how far the muscle stands off the model: 0 is a crisp seam. \
                    Keep it low on a finely detailed model, where a soft join drags the detail up the muscle.";
        theme::tip(r, cx.tips, help);
    }

    fn point_controls(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        let one = self.kind().one();
        theme::heading(ui, "Show Points");
        let tips = [
            self.label("Shows the points of every live tube.", "Shows the points of every live muscle."),
            self.label(
                "Shows only the selected tube's points, so the others are out of the way and cannot be grabbed by \
                 mistake. Click a tube to select it and bring up its points.",
                "Shows only the selected muscle's points, so the others are out of the way and cannot be grabbed by \
                 mistake. Click a muscle to select it and bring up its points.",
            ),
        ];
        if let Some(i) = pair(ui, cx, !self.only_selected_points(), ["All", "Selected Only"], tips) {
            self.set_only_selected_points(i == 1);
        }

        theme::heading(ui, "Selected Point");
        let selected = self.selected_point().is_some();
        let sharp = self.selected_is_sharp();
        let line = match (selected, sharp) {
            (false, _) => "Click a point in the viewport.".to_owned(),
            (true, false) => format!("Size {:.2}x", self.selected_point_size()),
            (true, true) => format!("Size {:.2}x, sharp", self.selected_point_size()),
        };
        theme::line(ui, &line, theme::HINT());
        ui.add_enabled_ui(selected, |ui| {
            let mut log_size = self.selected_point_size().ln();
            let range = TubeSettings::MIN_SIZE.ln()..=TubeSettings::MAX_SIZE.ln();
            let r = theme::slider(ui, "Point Size", &mut log_size, range, |v| format!("{:.2}x", v.exp()));
            if r.changed() {
                self.set_selected_point_size(log_size.exp());
            }
            let help = if self.kind() == Kind::Muscle {
                "Width at the selected point, as a multiple of the muscle's own there, on both sides evenly. The wheel \
                 over the point, or Shift + dragging one of its squares, does the same. Dragging a square on its own \
                 moves that edge and leaves the other where it is."
                    .to_owned()
            } else {
                format!(
                    "Thickness at the selected point, as a multiple of the {one}'s own there. Dragging the point's square \
                     handle, or the wheel over the point, does the same."
                )
            };
            theme::tip(r, cx.tips, &help);
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
        let kind = self.kind();
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, 3, 6.0);
            let small = || ButtonStyle { height: 24.0, ..Default::default() };
            ui.add_enabled_ui(self.active_tube().is_some(), |ui| {
                let (label, help) = (self.label("Delete Tube", "Delete"), format!("Removes the selected {}.", kind.one()));
                if theme::tip(theme::button(ui, w, label, small()), cx.tips, &help).clicked() {
                    self.delete_active_tube();
                }
            });
            ui.add_enabled_ui(self.can_undo(), |ui| {
                let help = format!("Undoes the last {} edit (Ctrl+Z).", kind.one());
                if theme::tip(theme::button(ui, w, "Undo", small()), cx.tips, &help).clicked() {
                    self.undo();
                }
            });
            ui.add_enabled_ui(self.can_redo(), |ui| {
                if theme::tip(theme::button(ui, w, "Redo", small()), cx.tips, "Redoes it (Ctrl + Shift + Z).").clicked() {
                    self.redo();
                }
            });
        });
        match self.next_undo_label() {
            Some(label) => theme::hint(ui, &format!("Undo (Ctrl+Z): {label}")),
            None => theme::hint(ui, &format!("{} history empty.", kind.title())),
        }
    }
}
