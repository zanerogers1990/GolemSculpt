//! The Project Detail card of the Sculpt panel's Mesh tab: which object the
//! detail comes from, how far a point may move, and the button.
//!
//! The card only asks: the button pushes a command the app carries out after
//! the interface pass (`app/project.rs`), as one undo step. The object to take
//! from starts as the one overlapping the selected object most, which after
//! Clone and Quad Remesh is the original; picking another holds until the
//! app closes.

use egui::Color32;

use super::theme::{self, ACCENT, ButtonStyle};
use super::tip;
use crate::app::project as projecting;
use crate::doc::{Action, Doc};

/// The card's colour on the panel.
const TINT: Color32 = Color32::from_rgb(120, 190, 235);

pub fn card(ui: &mut egui::Ui, doc: &mut Doc, actions: &mut Vec<Action>) {
    let tips = doc.opts.tooltips;
    let from = projecting::source(&doc.scene, &doc.mesh, doc.opts.project_from).map(|o| (o.id, o.name.clone()));
    let summary = from.as_ref().map_or(String::new(), |(_, name)| format!("from {name}"));
    theme::category(ui, "Project Detail", TINT, &summary, false, |ui| {
        theme::about(
            ui,
            "Puts the detail of another object onto this one. For clean topology: Clone the sculpt, Quad Remesh the \
             copy, then Subdivide (Ctrl+D) and Project, a few times over. The copy takes on the sculpt's detail and \
             keeps its even quads and levels.\n\n\
             Each point looks along its own direction, out and in, for the other surface facing its way, and goes to \
             the nearer. The mask protects as it does from a brush.",
        );
        let active = doc.scene.active().id;
        let others: Vec<(u64, String)> = doc
            .scene
            .objects
            .iter()
            .filter(|o| o.id != active && o.mesh != doc.scene.active().mesh)
            .map(|o| (o.id, o.name.clone()))
            .collect();
        if others.is_empty() {
            theme::hint(ui, "Needs a second object to take the detail from: Clone this one, then Quad Remesh the copy.");
            return;
        }
        let shown = from.as_ref().map_or("Choose an object", |(_, name)| name.as_str());
        ui.horizontal(|ui| {
            ui.label("From");
            egui::ComboBox::from_id_salt("project_from")
                .selected_text(shown)
                .width(ui.available_width() - 8.0)
                .show_ui(ui, |ui| {
                    for (id, name) in &others {
                        let picked = from.as_ref().is_some_and(|(f, _)| f == id);
                        if ui.selectable_label(picked, name).clicked() {
                            doc.opts.project_from = Some(*id);
                        }
                    }
                });
        });
        // Log slider: equal travel multiplies the reach.
        let range = crate::project::REACH_RANGE;
        let mut log = doc.opts.project_reach.clamp(*range.start(), *range.end()).ln();
        let size = projecting::reach(&doc.scene, &doc.mesh, 1.0);
        let readout = |v: f32| sculpt_geom::units::format(v.exp() * size, sculpt_geom::LengthUnit::Millimetres, true);
        let reach = theme::slider(ui, "Reach", &mut log, range.start().ln()..=range.end().ln(), readout);
        if reach.changed() {
            doc.opts.project_reach = log.exp();
        }
        tip(
            reach,
            tips,
            "How far a point may move to meet the other surface. Too little and the deepest detail is missed; too much \
             and a point may jump to a part it does not belong to, a finger to the next finger.",
        );
        let style = ButtonStyle { active: Some(ACCENT()), ..Default::default() };
        let label = from.as_ref().map_or("Project".to_owned(), |(_, name)| format!("Project from {name}"));
        let button = theme::button(ui, ui.available_width(), &label, style);
        if tip(button, tips, "Moves this object's points onto the other object's surface. One undo step (Ctrl+Z).")
            .clicked()
        {
            actions.push(Action::ProjectDetail);
        }
        if from.is_none() {
            theme::hint(ui, "No other object overlaps this one. Put the copy where the original is.");
        }
    });
}
