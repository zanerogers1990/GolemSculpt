//! Project Detail on the web: the card under Subdivision on the Mesh tab, as
//! the desktop's (`ui/project.rs`, `app/project.rs`), with the same words.
//!
//! The maths is the desktop's `project.rs` (compiled in as `crate::project`):
//! each point of the active object looks along its own direction, out and in,
//! for the other object's surface facing its way, and goes to the nearer. The
//! points move through the mesh's stroke calls, so the step is an ordinary
//! stroke: one undo step, and sculpt layers and subdivision levels hear of it
//! as of any stroke.

use eframe::egui::{self, Color32, Ui};

use super::ui::theme::{self, ACCENT, ButtonStyle};
use super::ui::tip;
use super::{GolemApp, Step};
use crate::mesh::bvh::Aabb;
use crate::mesh::layers::Ride;
use crate::project::{self, Source};
use crate::scene::Object;

/// The card's colour on the panel.
const TINT: Color32 = Color32::from_rgb(120, 190, 235);

/// The card's settings: the object picked to take from, and Reach.
pub(crate) struct ProjectState {
    pub from: Option<u64>,
    pub reach: f32,
}

impl Default for ProjectState {
    fn default() -> ProjectState {
        ProjectState { from: None, reach: project::DEFAULT_REACH }
    }
}

fn shared_volume(a: Aabb, b: Aabb) -> f32 {
    let size = (a.max.min(b.max) - a.min.max(b.min)).max(glam::Vec3::ZERO);
    size.x * size.y * size.z
}

/// A length for the status line, in millimetres.
fn millimetres(length: f32) -> String {
    sculpt_geom::units::format(length, sculpt_geom::LengthUnit::Millimetres, true)
}

impl GolemApp {
    /// The object Project takes detail from: the one picked if it is still
    /// there, else the one whose box overlaps the active one's most.
    fn project_source(&self) -> Option<&Object> {
        let usable = |o: &&Object| o.id != self.active;
        let wanted = self.shaping.project.from;
        if let Some(chosen) = self.objects.iter().filter(usable).find(|o| Some(o.id) == wanted) {
            return Some(chosen);
        }
        let mine = self.active_object().world_bounds(&self.mesh);
        self.objects
            .iter()
            .filter(usable)
            .map(|o| (o, shared_volume(mine, o.world_bounds(self.mesh_of(o)))))
            .filter(|(_, shared)| *shared > 0.0)
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(o, _)| o)
    }

    /// The reach `share` gives on the active object, in the scene's units.
    fn project_reach(&self, share: f32) -> f32 {
        let size = self.active_object().world_bounds(&self.mesh).size().max_element();
        let range = project::REACH_RANGE;
        let share = if share.is_finite() { share.clamp(*range.start(), *range.end()) } else { project::DEFAULT_REACH };
        size.max(0.0) * share
    }

    /// The Project button: the source's detail onto the active object, as one undo step.
    fn project_detail(&mut self) {
        self.deform_commit();
        let Some(from) = self.project_source() else {
            let why = if self.objects.len() < 2 {
                "Project needs a second object to take the detail from: Clone this one, Quad Remesh the copy, \
                 Subdivide it, then Project."
            } else {
                "No other object overlaps this one, so there is no detail to take. Put the copy where the original is."
            };
            self.status = why.to_owned();
            return;
        };
        let (from_id, from_name, from_place) = (from.id, from.name.clone(), from.matrix());
        let target_name = self.active_object().name.clone();
        let target_place = self.model();
        let reach = self.project_reach(self.shaping.project.reach);
        let started = web_time::Instant::now();
        let Some(source_mesh) = self.objects.iter().find(|o| o.id == from_id).and_then(|o| o.mesh.as_ref()) else { return };
        let done = project::project(&self.mesh, target_place, &[Source { mesh: source_mesh, to_world: from_place }], reach);
        let took = started.elapsed().as_millis();
        if done.indices.is_empty() {
            self.status = format!(
                "Nothing of \"{from_name}\" was within reach of \"{target_name}\" ({}). Raise Reach, or check the \
                 two are in the same place.",
                millimetres(reach)
            );
            return;
        }
        let mesh = &mut self.mesh;
        mesh.layer_ride = Ride::Still;
        let leaves = mesh.leaves_of(&done.indices);
        mesh.begin_stroke();
        mesh.move_listed(&done.indices, |k, _| Some(done.positions[k]));
        mesh.positions_changed(&leaves);
        if let Some(delta) = mesh.end_stroke(false) {
            self.push_undo(Step::Stroke(self.active, delta));
            self.mark_changed();
        }
        self.sculptor.last_hit = None;
        let moved = done.indices.len();
        let missed = match done.missed {
            0 => String::new(),
            1 => format!(" 1 point found nothing within {} and stayed.", millimetres(reach)),
            n => format!(" {n} points found nothing within {} and stayed.", millimetres(reach)),
        };
        self.status = format!("Projected \"{from_name}\" onto \"{target_name}\": {moved} points moved in {took} ms.{missed}");
    }

    /// The Project Detail card (Mesh tab, under Subdivision).
    pub(crate) fn project_card(&mut self, ui: &mut Ui) {
        let tips = self.ui.tips;
        let from = self.project_source().map(|o| (o.id, o.name.clone()));
        let summary = from.as_ref().map_or(String::new(), |(_, name)| format!("from {name}"));
        let others: Vec<(u64, String)> =
            self.objects.iter().filter(|o| o.id != self.active).map(|o| (o.id, o.name.clone())).collect();
        // The object's size itself (the desktop's readout goes through `reach`, which caps the share at 25%).
        let size = self.active_object().world_bounds(&self.mesh).size().max_element().max(0.0);
        let mut pressed = false;
        let state = &mut self.shaping.project;
        theme::category(ui, "Project Detail", TINT, &summary, false, |ui| {
            theme::about(
                ui,
                "Puts the detail of another object onto this one. For clean topology: Copy the sculpt, Quad Remesh the \
                 copy, then Subdivide (Ctrl+D) and Project, a few times over. The copy takes on the sculpt's detail and \
                 keeps its even quads and levels.\n\n\
                 Each point looks along its own direction, out and in, for the other surface facing its way, and goes to \
                 the nearer. The mask protects as it does from a brush.",
            );
            if others.is_empty() {
                theme::hint(ui, "Needs a second object to take the detail from: Copy this one, then Quad Remesh the copy.");
                return;
            }
            let shown = from.as_ref().map_or("Choose an object", |(_, name)| name.as_str());
            ui.horizontal(|ui| {
                ui.label("From");
                egui::ComboBox::from_id_salt("project_from").selected_text(shown).width(ui.available_width() - 8.0).show_ui(
                    ui,
                    |ui| {
                        for (id, name) in &others {
                            let picked = from.as_ref().is_some_and(|(f, _)| f == id);
                            if ui.selectable_label(picked, name).clicked() {
                                state.from = Some(*id);
                            }
                        }
                    },
                );
            });
            let range = project::REACH_RANGE;
            let mut log = state.reach.clamp(*range.start(), *range.end()).ln();
            let readout = |v: f32| sculpt_geom::units::format(v.exp() * size, sculpt_geom::LengthUnit::Millimetres, true);
            let reach = theme::slider(ui, "Reach", &mut log, range.start().ln()..=range.end().ln(), readout);
            if reach.changed() {
                state.reach = log.exp();
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
            if tip(button, tips, "Moves this object's points onto the other object's surface. One undo step (Ctrl+Z).").clicked() {
                pressed = true;
            }
            if from.is_none() {
                theme::hint(ui, "No other object overlaps this one. Put the copy where the original is.");
            }
        });
        if pressed {
            self.project_detail();
        }
    }
}
