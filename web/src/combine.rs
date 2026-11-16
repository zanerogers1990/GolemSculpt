//! The booleans and Join on the web: Subtract, Union and Intersect, and Join,
//! at the foot of the Objects card while two or more objects are selected, as
//! on the desktop (`ui/scene_graph.rs`, `app/selection.rs`), with the same
//! words.
//!
//! The active object (the bright one in the list) is the target: it keeps its
//! row, name and place and takes the result as its new mesh. The other
//! selected objects are the cutters, hidden afterwards (or deleted, with the
//! box ticked) as part of the same undo step, so one Undo brings back the
//! shape and the others. The booleans run in the target's own space through
//! the desktop's `sculpt-volume` kernel; Join is the desktop's plain
//! concatenation (`sculpt_geom::join`), remeshed afterwards if asked.
//!
//! Differences from the desktop: the Voxel Resolution goes to 512 here, not
//! 1024 (a browser tab has far less memory), and face groups are not carried
//! into the result.

use eframe::egui::{Ui, vec2};
use glam::Mat4;
use sculpt_geom::TriMesh;
use sculpt_volume::{BooleanOp, BooleanSettings};

use super::ui::theme::{self, ACCENT, ButtonStyle};
use super::ui::tip;
use super::{GolemApp, Step, group_digits};
use crate::mesh::Mesh;
use crate::printable;
use crate::remesh::{self, Notes};

/// The highest Voxel Resolution a boolean takes in a browser.
const HIGHEST_BOOLEAN: f32 = 512.0;

const NO_CUTTER: &str = "Select the target first, then Ctrl + click the other object.";
const MISS_SUBTRACT: &str = "Nothing changed - the cutter does not overlap the target - nothing to subtract.";
const MISS_INTERSECT: &str = "Nothing changed - the objects do not overlap - the intersection would be empty.";

/// The booleans' and Join's settings.
pub(crate) struct CombineState {
    pub resolution: Option<f32>,
    pub delete: bool,
    pub join_remesh: bool,
}

impl Default for CombineState {
    fn default() -> CombineState {
        CombineState { resolution: None, delete: false, join_remesh: false }
    }
}

impl GolemApp {
    /// The selected objects, the active one first, each with its mesh.
    fn selected_sources(&self) -> Vec<(&crate::scene::Object, &Mesh)> {
        let active = self.active_object();
        let others = self.selected.iter().filter(|&&id| id != self.active).filter_map(|id| self.objects.iter().find(|o| o.id == *id));
        std::iter::once(active).chain(others).map(|o| (o, self.mesh_of(o))).collect()
    }

    /// Puts `mesh` in the active object's place and hides or deletes the
    /// other selected objects, as one undo step. How many were selected.
    fn install_combined(&mut self, mesh: Mesh, delete: bool) -> usize {
        let id = self.active;
        let count = self.selected.len();
        let others: Vec<u64> = self.selected.iter().copied().filter(|&s| s != id).collect();
        let old = std::mem::replace(&mut self.mesh, mesh);
        let note = Self::rebuilt_note(&old, &self.mesh);
        let mut steps = vec![Step::Mesh(id, Box::new(old))];
        if delete {
            for other in others {
                if let Some((index, object)) = self.take_object(other) {
                    steps.push(Step::Exists { index, id: other, held: Some(object) });
                }
            }
        } else {
            let mut before = Vec::new();
            for o in self.objects.iter_mut().filter(|o| others.contains(&o.id)) {
                before.push((o.id, o.props()));
                o.visible = false;
            }
            steps.push(Step::Props(before));
        }
        self.selected = vec![id];
        self.push_undo(Step::Many(steps));
        self.upload_whole = true;
        self.sculptor.last_hit = None;
        self.mark_changed();
        self.shaping.note = note;
        count
    }

    /// Subtract, Union or Intersect on the selection.
    fn boolean_selected(&mut self, op: BooleanOp, resolution: u32, delete: bool) {
        self.deform_commit();
        let started = web_time::Instant::now();
        let sources = self.selected_sources();
        let Some(((target, mesh), others)) = sources.split_first().filter(|(_, others)| !others.is_empty()) else {
            self.status = NO_CUTTER.into();
            return;
        };
        let into = target.matrix().inverse();
        let mut cutters: Vec<TriMesh> = others
            .iter()
            .map(|(o, m)| {
                let mut solid = TriMesh::new(m.positions.clone(), m.indices.clone());
                solid.transform(&(into * o.matrix()));
                solid
            })
            .filter(|c| !c.is_empty())
            .collect();
        let target_solid = TriMesh::new(mesh.positions.clone(), mesh.indices.clone());
        let name = target.name.clone();
        drop(sources);
        if cutters.is_empty() {
            self.status = NO_CUTTER.into();
            return;
        }
        let bounds = target_solid.bounds();
        if op != BooleanOp::Union && !cutters.iter().any(|cutter| cutter.bounds().overlaps(&bounds)) {
            self.status = if op == BooleanOp::Subtract { MISS_SUBTRACT } else { MISS_INTERSECT }.into();
            return;
        }
        let most = sculpt_volume::boolean::MAX_SOLIDS - 1;
        if op != BooleanOp::Intersect && cutters.len() > most {
            for extra in cutters.split_off(most) {
                cutters[most - 1].append(&extra);
            }
        }
        let solids: Vec<&TriMesh> = cutters.iter().collect();
        let settings = BooleanSettings { mirror: remesh::AS_IT_IS, ..BooleanSettings::new(resolution) };
        let built = match sculpt_volume::boolean_with(&target_solid, &solids, op, &settings) {
            Ok(built) => built,
            Err(e) => {
                self.status = format!("Nothing changed. {}", remesh::named(e, &name));
                return;
            }
        };
        let notes = Notes::of(&built);
        let made = Mesh::new(built.mesh.positions, built.mesh.indices, None);
        let triangles = made.triangle_count();
        let count = self.install_combined(made, delete) - 1;
        let (verb, plural) = (
            match op {
                BooleanOp::Subtract => "Subtracted",
                BooleanOp::Union => "United",
                BooleanOp::Intersect => "Intersected",
            },
            if count == 1 { "" } else { "s" },
        );
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        let text = if delete {
            format!(
                "{verb} {count} object{plural}, deleted. {} triangles in {ms:.0} ms. Undo (Ctrl+Z) brings back the shape and what was deleted.",
                group_digits(triangles)
            )
        } else {
            format!(
                "{verb} {count} object{plural}, hidden. {} triangles in {ms:.0} ms. The eye in the list shows a hidden one again; undo (Ctrl+Z) brings back the shape and shows them.",
                group_digits(triangles)
            )
        };
        self.status = text + &notes.after_boolean() + &self.closed_note() + &std::mem::take(&mut self.shaping.note);
    }

    /// What to add when the result is not closed, as the desktop's status line does.
    fn closed_note(&self) -> String {
        let census = printable::census(&self.mesh);
        let name = self.active_object().name.as_str();
        printable::warning(&[(name, census)], printable::Cure::OtherResolution).map_or(String::new(), |w| format!(" {}", w.trim_end()))
    }

    /// Join: merges the selected objects into the active one.
    fn join_selected(&mut self, rebuild: bool) {
        self.deform_commit();
        let started = web_time::Instant::now();
        let sources = self.selected_sources();
        if sources.len() < 2 {
            self.status = "Join needs two or more objects selected. Shift + click objects in the list to add them.".into();
            return;
        }
        let survivor = sources[0].0;
        let into = survivor.matrix().inverse();
        let name = survivor.name.clone();
        let shells: Vec<TriMesh> = sources.iter().map(|(_, m)| TriMesh::new(m.positions.clone(), m.indices.clone())).collect();
        let place = |o: &crate::scene::Object| if o.id == survivor.id { Mat4::IDENTITY } else { into * o.matrix() };
        let parts: Vec<(&TriMesh, Mat4)> = shells.iter().zip(&sources).map(|(shell, (o, _))| (shell, place(o))).collect();
        let joined = sculpt_geom::join::join(&parts);
        let mut mask = Vec::with_capacity(joined.mesh.positions.len());
        for (_, m) in &sources {
            mask.extend_from_slice(&m.mask);
        }
        mask.resize(joined.mesh.positions.len(), 0.0);
        let (mut mesh, vertex) = Mesh::new_mapped(joined.mesh.positions, joined.mesh.indices, Some(mask));
        let meshes: Vec<&Mesh> = sources.iter().map(|(_, m)| *m).collect();
        let count = mesh.vertex_count();
        mesh.set_paint(crate::mesh::paint::transfer::joined(&meshes, &vertex, count));
        drop(sources);
        let (mut remesh_failed, mut notes) = (None, Notes::default());
        if rebuild {
            match remesh::voxel_remesh_noted(&mesh, self.remesh_resolution, remesh::AS_IT_IS) {
                Ok((rebuilt, noted)) => (mesh, notes) = (rebuilt, noted),
                Err(e) => remesh_failed = Some(e),
            }
        }
        let triangles = mesh.triangle_count();
        let count = self.install_combined(mesh, true);
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        let mut text = format!("Joined {count} objects into \"{name}\" ({triangles} triangles) in {ms:.0} ms.");
        text += &match (rebuild, remesh_failed) {
            (_, Some(why)) => format!(" The remesh did not run ({}), so the parts are as they were.", remesh::named(why, &name)),
            (true, None) => format!(" It was remeshed into one surface.{}", notes.after_remesh()),
            (false, None) => String::new(),
        };
        text += " Undo (Ctrl+Z) brings the separate objects back.";
        self.status = text + &std::mem::take(&mut self.shaping.note);
    }

    /// The booleans and Join, at the foot of the Objects card.
    pub(crate) fn combine_section(&mut self, ui: &mut Ui) {
        let tips = self.ui.tips;
        let count = self.selected.len();
        if count < 2 {
            theme::hint(ui, "Shift + click two or more objects in the list to cut one with the others, weld them or join them.");
            return;
        }
        let name = self.active_object().name.clone();
        theme::hint(
            ui,
            &format!("\"{name}\", the bright one, is kept, with its centre and symmetry plane. Whatever else is selected cuts it or goes into it."),
        );
        let icons = ["bool_subtract", "bool_union", "bool_intersect", "join"].map(|n| self.shaping.icons.get(ui.ctx(), n));
        let remesh_resolution = self.remesh_resolution;
        let st = &mut self.shaping.combine;
        let resolution = st.resolution.unwrap_or((remesh_resolution as f32).min(HIGHEST_BOOLEAN));
        let after = if st.delete { "deleted" } else { "hidden afterwards, not deleted: the eye in the list shows them again" };
        let tiles = [
            (BooleanOp::Subtract, icons[0], "Subtract", "Cuts the other selected objects out of the bright one."),
            (BooleanOp::Union, icons[1], "Union", "Welds the selected objects into the bright one as one solid. Unlike Join, this leaves no geometry inside the overlap."),
            (BooleanOp::Intersect, icons[2], "Intersect", "Keeps only the volume the bright object shares with every other selected object."),
        ];
        let mut boolean = None;
        let mut join = None;
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, tiles.len(), 4.0);
            ui.spacing_mut().item_spacing.x = 4.0;
            for (op, icon, label, what) in tiles {
                let r = theme::tile(ui, vec2(w, 46.0), icon, label, "", None);
                let help = format!("{what} The others are {after}. It clears the mask, as a remesh does. Undo (Ctrl+Z) restores the shape and the others.");
                if tip(r, tips, &help).clicked() {
                    boolean = Some(op);
                }
            }
        });
        let mut log = resolution.log2();
        let slider = theme::slider(ui, "Voxel Resolution (across the target)", &mut log, 4f32.log2()..=HIGHEST_BOOLEAN.log2(), |v| {
            format!("{:.0}", v.exp2())
        });
        let help = "Voxel density the boolean is computed at, counted along the bright object's longest side: higher captures finer cutter detail but is slower.";
        if tip(slider, tips, help).changed() {
            st.resolution = Some(log.exp2().round());
        }
        let help = "Deletes the other selected objects instead of just hiding them (hidden ones can be shown again from the list). Undo (Ctrl+Z) brings them back either way.";
        tip(theme::checkbox(ui, &mut st.delete, "Delete the others instead of hiding", ACCENT()), tips, help);
        ui.add_space(6.0);
        let help = "Rebuilds the joined result on a clean voxel grid, welding away the seams where the objects met. Like any remesh, it clears the mask.";
        tip(theme::checkbox(ui, &mut st.join_remesh, "Remesh after Join", ACCENT()), tips, help);
        let style = ButtonStyle { icon: icons[3], ..Default::default() };
        let help = "Merges every selected object into the bright one, as one mesh. Each shell is kept as it is, masks included. Undo (Ctrl+Z) brings the separate objects back.";
        if tip(theme::button(ui, ui.available_width(), "Join", style), tips, help).clicked() {
            join = Some(st.join_remesh);
        }
        if st.join_remesh {
            theme::hint(ui, "The remesh uses the Remesh card's Resolution (Mesh tab).");
        }
        let delete = st.delete;
        if let Some(op) = boolean {
            self.boolean_selected(op, resolution as u32, delete);
        }
        if let Some(rebuild) = join {
            self.join_selected(rebuild);
        }
    }
}
