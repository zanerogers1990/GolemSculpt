//! Project Detail on the app's side: which object the detail comes from, the
//! reach in the scene's units, and the projection made one undo step.
//!
//! What moves where is `project.rs`. The points are moved through the mesh's
//! stroke calls, as Make Symmetric moves them (app/symmetry_repair.rs), so the
//! step is an ordinary sparse stroke: the renderer re-sends only what moved,
//! and sculpt layers and subdivision levels hear of it as of any stroke. On a
//! level below the finest, the projection is carried up when the object steps
//! up, as a stroke made there would be.
//!
//! The source is the object the card names (`Options::project_from`), or, when
//! it names none or one that has gone, the object whose box overlaps the
//! active one's most: after Clone and Quad Remesh that is the original, sitting
//! exactly where the copy is. An object that shares the active one's mesh (an
//! instance) cannot be a source: it is the same surface.

use std::time::Instant;

use super::Running;
use crate::history::History;
use crate::mesh::Mesh;
use crate::mesh::bvh::Aabb;
use crate::mesh::layers::Ride;
use crate::project::{self, Projected, Source};
use crate::scene::{Object, Scene};

/// How much two boxes overlap, as the volume they share.
fn shared_volume(a: Aabb, b: Aabb) -> f32 {
    let size = (a.max.min(b.max) - a.min.max(b.min)).max(glam::Vec3::ZERO);
    size.x * size.y * size.z
}

/// The object Project takes detail from: `wanted` if it is another object
/// with a mesh of its own, else the one whose box overlaps the active one's
/// most. `None` when no other object overlaps it at all.
pub(crate) fn source<'a>(scene: &'a Scene, slot: &Mesh, wanted: Option<u64>) -> Option<&'a Object> {
    let active = scene.active();
    let usable = |o: &&Object| o.id != active.id && o.mesh != active.mesh && scene.mesh(o.mesh, slot).is_some();
    if let Some(chosen) = scene.objects.iter().filter(usable).find(|o| Some(o.id) == wanted) {
        return Some(chosen);
    }
    let mine = scene.bounds_of(active, slot);
    scene
        .objects
        .iter()
        .filter(usable)
        .map(|o| (o, shared_volume(mine, scene.bounds_of(o, slot))))
        .filter(|(_, shared)| *shared > 0.0)
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(o, _)| o)
}

/// The reach `share` gives on the active object, in the scene's units: that
/// share of the longest side of its box in the scene.
pub(crate) fn reach(scene: &Scene, slot: &Mesh, share: f32) -> f32 {
    let size = scene.bounds_of(scene.active(), slot).size().max_element();
    let share = if share.is_finite() { share.clamp(*project::REACH_RANGE.start(), *project::REACH_RANGE.end()) } else { project::DEFAULT_REACH };
    size.max(0.0) * share
}

/// Moves the points `done` lists as one stroke, and records it in `history`
/// if anything moved.
fn commit(mesh: &mut Mesh, history: &mut History, done: &Projected) {
    // A projection pushes the surface along; it does not swing it round a joint (mesh/layers.rs).
    mesh.layer_ride = Ride::Still;
    let leaves = mesh.leaves_of(&done.indices);
    mesh.begin_stroke();
    mesh.move_listed(&done.indices, |k, _| Some(done.positions[k]));
    mesh.positions_changed(&leaves);
    if let Some(delta) = mesh.end_stroke(false) {
        history.push_stroke(delta);
    }
}

/// A length for the status line, in millimetres.
fn millimetres(length: f32) -> String {
    sculpt_geom::units::format(length, sculpt_geom::LengthUnit::Millimetres, true)
}

impl Running {
    /// The Project button: the source's detail onto the active object, as one
    /// undo step, or none if nothing was within reach.
    pub(super) fn project_detail(&mut self) {
        self.tidy_merged();
        let doc = &mut self.doc;
        let Some(from) = source(&doc.scene, &doc.mesh, doc.opts.project_from) else {
            let why = if doc.scene.objects.len() < 2 {
                "Project needs a second object to take the detail from: Clone this one, Quad Remesh the copy, \
                 Subdivide it, then Project."
            } else {
                "No other object overlaps this one, so there is no detail to take. Put the copy where the original is."
            };
            return self.status(why.to_owned(), false);
        };
        let (from_name, from_mesh, from_place) = (from.name.clone(), from.mesh, from.matrix());
        let target_name = doc.scene.active().name.clone();
        let target_place = doc.scene.active().matrix();
        let reach = reach(&doc.scene, &doc.mesh, doc.opts.project_reach);
        let started = Instant::now();
        let Some(source_mesh) = doc.scene.mesh(from_mesh, &doc.mesh) else { return };
        let done = project::project(&doc.mesh, target_place, &[Source { mesh: source_mesh, to_world: from_place }], reach);
        let took = started.elapsed().as_millis();
        if done.indices.is_empty() {
            return self.status(
                format!(
                    "Nothing of \"{from_name}\" was within reach of \"{target_name}\" ({}). Raise Reach, or check the \
                     two are in the same place.",
                    millimetres(reach)
                ),
                false,
            );
        }
        // Reset Shape goes back to the shape before the first thing that moved the mesh (scene.rs).
        doc.scene.keep_baseline(&doc.mesh);
        commit(&mut doc.mesh, &mut doc.history, &done);
        // The surface has moved out from under the last brush dab.
        doc.sculptor.last_hit = None;
        let moved = done.indices.len();
        let missed = match done.missed {
            0 => String::new(),
            1 => format!(" 1 point found nothing within {} and stayed.", millimetres(reach)),
            n => format!(" {n} points found nothing within {} and stayed.", millimetres(reach)),
        };
        self.toast("Project");
        self.status(
            format!("Projected \"{from_name}\" onto \"{target_name}\": {moved} points moved in {took} ms.{missed}"),
            true,
        );
    }
}

#[cfg(test)]
mod tests;
