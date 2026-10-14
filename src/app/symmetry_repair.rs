//! The Symmetry card's repairs other than Cut & Mirror, on the app's side:
//! Check Symmetry, Make Symmetric and Symmetry Cleanup, and the Match
//! Tolerance all four buttons work at.
//!
//! What they compute is `sculpt_symmetry::ops`, which reads the mesh's arrays
//! and hands back either the vertices to move or a whole new mesh. Here is
//! what the app adds: how each of those becomes one undo step, and what the
//! status line says.
//!
//! - Vertices to move (`sculpt_symmetry::Moved`: Make Symmetric, and a Cleanup
//!   that had nothing to merge) are written through the mesh's stroke calls,
//!   the way the gizmo reshapes the unmasked part of an object (app/masked.rs).
//!   So the step is an ordinary sparse stroke in the history, the renderer
//!   re-sends only what moved, and sculpt layers and subdivision levels are
//!   told of it as they are of any stroke. Nothing here writes positions any
//!   other way: a second path would be a second place for layers to go stale.
//! - A new mesh (a Cleanup that merged vertices) takes the old one's place as
//!   Cut & Mirror's does (`Running::replace_mesh`), the old mesh kept whole
//!   as the undo step.
//!
//! A stroke passes over hidden vertices, and so do these: what is hidden is
//! put away. The line says how many were left where they were, since the
//! counts before it are of the whole model.
//!
//! The plane is always the object's own, as for Cut & Mirror and for the
//! same reason (app/cut_mirror.rs).
//!
//! Match Tolerance is the whole app's and no object's (`Options`), and is not
//! kept between runs. It answers "how far apart did these two halves drift",
//! which belongs to the repair in hand: it is widened to get one stubborn
//! model to pair and wants to be back where it started for the next. Kept
//! with an object it would go into every saved file for the sake of a button
//! pressed once.
//!
//! Check Symmetry is worked out when its button is pressed and at no other
//! time. Pairing a mesh up takes seconds on a large one, and every stroke
//! makes the answer stale, so a figure kept on the card would either cost
//! that after each stroke or describe a model that is no longer there.

use sculpt_symmetry::ops::{self, CleanupChange};
use sculpt_symmetry::{Axis, Moved};

use super::Running;
use crate::history::History;
use crate::mesh::layers::Ride;
use crate::mesh::{Mesh, StrokeDelta};
use crate::printable::Cure;

/// Match Tolerance where the slider starts: the tolerance the mesh itself
/// suggests, about half a triangle's width.
pub(crate) const DEFAULT_TOLERANCE: f32 = 1.0;

/// The Match Tolerance setting as the operations take it: inside the range
/// the slider offers, whatever a script or a bad number put in the options.
pub(crate) fn tolerance_scale(set: f32) -> f32 {
    if set.is_finite() { set.clamp(ops::MIN_TOLERANCE_SCALE, ops::MAX_TOLERANCE_SCALE) } else { DEFAULT_TOLERANCE }
}

/// `text` as a sentence of the status line. Some of the operations' lines
/// end in advice with its own full stop, and most end in a count.
fn sentence(text: String) -> String {
    if text.ends_with('.') { text } else { text + "." }
}

/// How far apart the two ends of a pair may be from mirror images of each
/// other and still be called so by Check Symmetry, as a fraction of the
/// model's size. Strokes drawn with the mirror on leave pairs a rounding
/// error apart, which nobody can see and no repair is wanted for.
const OUT_OF_TRUE: f32 = 1e-5;

/// Check Symmetry: how much of `mesh` pairs up across its own `axis` plane,
/// and whether it is symmetric: every vertex paired, and every pair the
/// mirror image of each other. Where it is not, the line names the button
/// that can put it right.
///
/// The operations' own report (`ops::status`) counts pairs, and calls a
/// model symmetric when every vertex has one. That is a statement about the
/// triangles: a model with a lump on one side pairs up perfectly, which is
/// what lets Make Symmetric repair it. Read on the status line it would say
/// "symmetric" of a model that plainly is not, so the line goes on to say
/// how many pairs are out of true and by how much.
pub(crate) fn symmetry_of(mesh: &Mesh, name: &str, axis: Axis, tolerance_scale: f32) -> (String, bool) {
    // The map `ops::status` builds, kept for its counts and its pairs:
    // building it twice would double the wait on a large model.
    let Some(map) = ops::build_map(&mesh.positions, &mesh.indices, axis, tolerance_scale) else {
        return (format!("\"{name}\": {}.", ops::status(&mesh.positions, &mesh.indices, axis, tolerance_scale)), false);
    };
    let mut text = format!("\"{name}\" across {}.", map.summary());
    if map.is_symmetric() {
        // Each pair once, from its lower end.
        let apart = (0..mesh.positions.len() as u32).filter_map(|v| {
            let partner = map.partner_of(v).filter(|&partner| partner > v)?;
            Some(mesh.positions[v as usize].distance(axis.reflect(mesh.positions[partner as usize])))
        });
        let allowed = map.diagonal() * OUT_OF_TRUE;
        let (out, furthest) = apart.filter(|&d| d > allowed).fold((0, 0.0f32), |(n, far), d| (n + 1, far.max(d)));
        if out == 0 {
            text += " Every pair is the mirror image of the other.";
            return (text, true);
        }
        let share = 100.0 * furthest / map.diagonal().max(f32::MIN_POSITIVE);
        text += &format!(
            " The vertices all pair up, but {out} of the pairs are not mirror images of each other, the furthest by \
             {share:.2}% of the model's size. Make Symmetric lines them up."
        );
        return (text, false);
    }
    // The rule Make Symmetric refuses by, so the advice is never a button that will say no.
    let off_plane = map.vertex_count() - map.on_plane_count();
    let too_many = map.unmatched_count() as f32 > off_plane as f32 * ops::MAX_UNMATCHED_FRACTION;
    text += if too_many {
        " Too many to match up vertex for vertex: raise Match Tolerance, or use Cut & Mirror, which rebuilds one \
         side from the other."
    } else {
        " Make Symmetric can match the two sides up."
    };
    (text, false)
}

/// Moves the vertices `moved` lists to where it has them, as one stroke, and
/// returns the stroke's undo record: `None` if nothing moved. Hidden vertices
/// are passed over, as by any stroke.
fn move_as_stroke(mesh: &mut Mesh, moved: &Moved) -> Option<StrokeDelta> {
    if moved.is_empty() {
        return None;
    }
    // A repair pushes the surface along; it does not swing it round a joint (mesh/layers.rs).
    mesh.layer_ride = Ride::Still;
    let leaves = mesh.leaves_of(&moved.indices);
    mesh.begin_stroke();
    mesh.move_listed(&moved.indices, |k, _| Some(moved.positions[k]));
    mesh.positions_changed(&leaves);
    mesh.end_stroke(false)
}

/// Commits a vertex-only repair: the stroke, and its step in `history` if it
/// moved anything. Returns what the status line adds about hidden vertices,
/// which is nothing when none of the vertices to move was hidden.
fn commit(mesh: &mut Mesh, history: &mut History, moved: &Moved) -> String {
    let hidden = moved.indices.iter().filter(|&&v| mesh.is_vertex_hidden(v)).count();
    if let Some(delta) = move_as_stroke(mesh, moved) {
        history.push_stroke(delta);
    }
    match hidden {
        0 => String::new(),
        1 => " 1 of those vertices is hidden and was left where it was.".to_owned(),
        n => format!(" {n} of those vertices are hidden and were left where they were."),
    }
}

/// Make Symmetric: moves one side of `mesh` onto the mirror image of the
/// other across its own `axis` plane, the positive side being the one copied
/// when `from_positive`, as one stroke recorded in `history`. With anything
/// masked only the unmasked part moves. Returns the line that says what was
/// done; or, with nothing changed, why it would not: the two sides are too
/// unlike to pair, and the line names Cut & Mirror.
pub(crate) fn make_symmetric(
    mesh: &mut Mesh,
    history: &mut History,
    axis: Axis,
    tolerance_scale: f32,
    from_positive: bool,
) -> Result<String, String> {
    let done =
        ops::make_symmetric(&mesh.positions, &mesh.indices, Some(&mesh.mask), axis, tolerance_scale, from_positive)?;
    let mut text = sentence(done.message());
    text += &commit(mesh, history, &done.moved);
    Ok(text)
}

/// What Symmetry Cleanup came to.
pub(crate) enum Cleanup {
    /// The model was clean already.
    Nothing,
    /// Vertices were put on the plane: done, and recorded as a stroke.
    Moved,
    /// Vertices were merged, which is another mesh: this one, with the mask
    /// carried across, for the caller to put in the old one's place.
    Rebuilt(Mesh),
}

/// Symmetry Cleanup: puts the vertices of `mesh` that sit near its own `axis`
/// plane exactly on it, and merges the doubled ones that leaves, so that two
/// halves joined along the plane become one surface. Returns what it came to
/// and the line that says so. A change that only moved vertices has been made
/// and recorded in `history` by the time this returns; a rebuilt mesh has not
/// (see [`Cleanup`]).
pub(crate) fn clean_up(
    mesh: &mut Mesh,
    history: &mut History,
    axis: Axis,
    tolerance_scale: f32,
) -> Result<(Cleanup, String), String> {
    let cleaned = ops::cleanup(&mesh.positions, &mesh.indices, Some(&mesh.mask), axis, tolerance_scale)?;
    let mut text = sentence(cleaned.message());
    let change = match cleaned.change {
        CleanupChange::Nothing => Cleanup::Nothing,
        CleanupChange::Moved(moved) => {
            text += &commit(mesh, history, &moved);
            Cleanup::Moved
        }
        CleanupChange::Rebuilt { mesh: built, mask, .. } => {
            if mesh.any_hidden() {
                text += " Its hidden parts were cleaned up with the rest, and everything is showing again.";
            }
            Cleanup::Rebuilt(Mesh::new(built.positions, built.indices, mask))
        }
    };
    Ok((change, text))
}

impl Running {
    fn match_tolerance(&self) -> f32 {
        tolerance_scale(self.doc.opts.symmetry_tolerance)
    }

    /// The Check Symmetry button: says how much of the selected object pairs
    /// up across its own `axis` plane. Changes nothing.
    pub(super) fn check_symmetry(&mut self, axis: Axis) {
        let name = self.doc.scene.active().name.clone();
        let (text, ok) = symmetry_of(&self.doc.mesh, &name, axis, self.match_tolerance());
        self.toast("Check Symmetry");
        self.status(text, ok);
    }

    /// The Make Symmetric buttons. One undo step, or none if the object
    /// matched already or the repair was refused.
    pub(super) fn make_symmetric(&mut self, axis: Axis, from_positive: bool) {
        let tolerance = self.match_tolerance();
        let doc = &mut self.doc;
        // Reset Shape goes back to the shape before the first thing that moved the mesh (scene.rs).
        doc.scene.keep_baseline(&doc.mesh);
        match make_symmetric(&mut doc.mesh, &mut doc.history, axis, tolerance, from_positive) {
            Ok(text) => {
                // The surface may have moved out from under the last brush dab.
                doc.sculptor.last_hit = None;
                self.toast("Make Symmetric");
                self.status(text, true);
            }
            Err(why) => self.status(why, false),
        }
    }

    /// The Symmetry Cleanup button. One undo step, or none if the object was
    /// clean already.
    pub(super) fn symmetry_cleanup(&mut self, axis: Axis) {
        let tolerance = self.match_tolerance();
        let doc = &mut self.doc;
        doc.scene.keep_baseline(&doc.mesh);
        match clean_up(&mut doc.mesh, &mut doc.history, axis, tolerance) {
            Ok((Cleanup::Rebuilt(mesh), text)) => {
                self.replace_mesh(mesh, true);
                self.toast("Symmetry Cleanup");
                // A weld closes a seam; say if the model has other holes still.
                self.status_checked(text, Cure::Remesh);
            }
            Ok((_, text)) => {
                self.doc.sculptor.last_hit = None;
                self.toast("Symmetry Cleanup");
                self.status(text, true);
            }
            Err(why) => self.status(why, false),
        }
    }
}

mod demo;
#[cfg(test)]
mod tests;
