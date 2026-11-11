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
//!
//! All three run in the background, the way Remesh does (app/remesh_job.rs),
//! and for the same reason: those seconds spent in the window's own thread
//! froze it, with nothing to say what it was doing and no way to stop it.
//! So each one:
//!
//! - **works on a copy** of the mesh's arrays as they were when the button
//!   was pressed ([`Copied`]), on a job of its own, whose progress and Cancel
//!   button show at the bottom of the view while the window goes on drawing.
//!   What the job hands back is what the repair found ([`Found`]): the
//!   vertices to move, or the new mesh, already built.
//! - **lands as one undo step**, named as the button is, if the model is
//!   still as it was: the same mesh, no edit recorded since, and no step
//!   undone or redone since. Otherwise what it found is of a model that is
//!   no longer there, and is thrown away with a line that says so. An object
//!   deselected meanwhile is selected again for it, as Remesh does; one
//!   deleted meanwhile is no longer there, and the result goes.
//! - **runs one at a time**, and not while another background job is at work.
//!
//! Small models go the same way, as they do for Remesh: their job is over
//! before the next frame or two, and one path is one path to keep right.
//!
//! The direct calls below ([`symmetry_of`], [`make_symmetric`],
//! [`clean_up`]) are the same two halves run one after the other: what the
//! job works out, then what landing it does.

use std::time::{Duration, Instant};

use glam::Vec3;
use sculpt_geom::EdgeCensus;
use sculpt_symmetry::ops::{self, CleanupChange};
use sculpt_symmetry::{Axis, Moved};
use sculpt_toolkit::{Job, JobCtx};

use super::Running;
use crate::history::History;
use crate::mesh::layers::Ride;
use crate::mesh::{Mesh, StrokeDelta};
use crate::printable::{self, Cure};

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
    symmetry_across(&mesh.positions, &mesh.indices, name, axis, tolerance_scale)
}

/// [`symmetry_of`] the mesh whose vertices are `positions` and whose
/// triangles are `indices`: what a job has of it.
fn symmetry_across(positions: &[Vec3], indices: &[u32], name: &str, axis: Axis, tolerance_scale: f32) -> (String, bool) {
    // The map `ops::status` builds, kept for its counts and its pairs:
    // building it twice would double the wait on a large model.
    let Some(map) = ops::build_map(positions, indices, axis, tolerance_scale) else {
        return (format!("\"{name}\": {}.", ops::status(positions, indices, axis, tolerance_scale)), false);
    };
    let mut text = format!("\"{name}\" across {}.", map.summary());
    if map.is_symmetric() {
        // Each pair once, from its lower end.
        let apart = (0..positions.len() as u32).filter_map(|v| {
            let partner = map.partner_of(v).filter(|&partner| partner > v)?;
            Some(positions[v as usize].distance(axis.reflect(positions[partner as usize])))
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
    let (text, moved) = symmetrized(&mesh.positions, &mesh.indices, &mesh.mask, axis, tolerance_scale, from_positive)?;
    Ok(text + &commit(mesh, history, &moved))
}

/// The half of [`make_symmetric`] that reads the mesh and changes nothing:
/// the line, and the vertices to move.
fn symmetrized(
    positions: &[Vec3],
    indices: &[u32],
    mask: &[f32],
    axis: Axis,
    tolerance_scale: f32,
    from_positive: bool,
) -> Result<(String, Moved), String> {
    let done = ops::make_symmetric(positions, indices, Some(mask), axis, tolerance_scale, from_positive)?;
    Ok((sentence(done.message()), done.moved))
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

/// What Symmetry Cleanup found to do, before any of it is done: the half of
/// [`clean_up`] that a job can carry out.
pub(crate) enum Cleaning {
    Nothing,
    /// Vertices to put on the plane.
    Moved(Moved),
    /// The mesh with its doubled vertices merged, built already.
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
    let (cleaning, text) = cleaned(&mesh.positions, &mesh.indices, &mesh.mask, axis, tolerance_scale)?;
    Ok(apply_cleaning(mesh, history, cleaning, text))
}

/// The half of [`clean_up`] that reads the mesh and changes nothing. The
/// slow part of a weld, building the new mesh's sculpting structures, is in
/// this half, so that a job does it.
fn cleaned(
    positions: &[Vec3],
    indices: &[u32],
    mask: &[f32],
    axis: Axis,
    tolerance_scale: f32,
) -> Result<(Cleaning, String), String> {
    let cleaned = ops::cleanup(positions, indices, Some(mask), axis, tolerance_scale)?;
    let text = sentence(cleaned.message());
    let cleaning = match cleaned.change {
        CleanupChange::Nothing => Cleaning::Nothing,
        CleanupChange::Moved(moved) => Cleaning::Moved(moved),
        CleanupChange::Rebuilt { mesh: built, mask, .. } => Cleaning::Rebuilt(Mesh::new(built.positions, built.indices, mask)),
    };
    Ok((cleaning, text))
}

/// The other half of [`clean_up`]: does to `mesh` what [`cleaned`] found
/// for it, but for putting a rebuilt mesh in its place, which is the
/// caller's.
fn apply_cleaning(mesh: &mut Mesh, history: &mut History, cleaning: Cleaning, mut text: String) -> (Cleanup, String) {
    let change = match cleaning {
        Cleaning::Nothing => Cleanup::Nothing,
        Cleaning::Moved(moved) => {
            text += &commit(mesh, history, &moved);
            Cleanup::Moved
        }
        Cleaning::Rebuilt(built) => {
            if mesh.any_hidden() {
                text += " Its hidden parts were cleaned up with the rest, and everything is showing again.";
            }
            Cleanup::Rebuilt(built)
        }
    };
    (change, text)
}

/// One of the three buttons, as a job is told what to do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Repair {
    Check(Axis),
    Make { axis: Axis, from_positive: bool },
    Cleanup(Axis),
}

impl Repair {
    /// The button's name, which is also the job's and the undo step's.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Repair::Check(_) => "Check Symmetry",
            Repair::Make { .. } => "Make Symmetric",
            Repair::Cleanup(_) => "Symmetry Cleanup",
        }
    }
}

/// What a repair reads of a mesh, copied out of it, so that the repair can
/// be worked out on a thread of its own while the mesh stays where it is.
pub(crate) struct Copied {
    positions: Vec<Vec3>,
    indices: Vec<u32>,
    mask: Vec<f32>,
    /// Whether the mesh has what dynamic topology's merges leave behind,
    /// which Check Symmetry reads past (see [`work`]).
    tombstones: bool,
}

impl Copied {
    pub(crate) fn of(mesh: &Mesh) -> Copied {
        Copied {
            positions: mesh.positions.clone(),
            indices: mesh.indices.clone(),
            mask: mesh.mask.clone(),
            tombstones: mesh.has_tombstones(),
        }
    }
}

/// What a repair found, for landing on the mesh it was copied from.
pub(crate) enum Found {
    /// Check Symmetry's line, and whether it is good news.
    Checked(String, bool),
    /// Make Symmetric's line, and the vertices to move.
    Symmetrized(String, Moved),
    /// Symmetry Cleanup's line, and what to do.
    Cleaned(String, Cleaning),
}

/// Works out `repair` on `copied`, the mesh of the object called `name`, and
/// changes nothing. The whole of the slow part of each repair is here.
pub(crate) fn work(repair: Repair, copied: Copied, name: &str, tolerance_scale: f32) -> Result<Found, String> {
    let Copied { positions, indices, mask, tombstones } = copied;
    Ok(match repair {
        Repair::Check(axis) => {
            // Without the vertices dynamic topology's merges left in no
            // triangle, which would have no partner across the plane. A
            // build leaves them out, as the tidied copy the other two are
            // given does (`crate::scene::tidied`); it is made here so that
            // the window does not wait for it.
            let (positions, indices) = if tombstones {
                let mut tidy = Mesh::new(positions, indices, None);
                (std::mem::take(&mut tidy.positions), std::mem::take(&mut tidy.indices))
            } else {
                (positions, indices)
            };
            let (text, ok) = symmetry_across(&positions, &indices, name, axis, tolerance_scale);
            Found::Checked(text, ok)
        }
        Repair::Make { axis, from_positive } => {
            let (text, moved) = symmetrized(&positions, &indices, &mask, axis, tolerance_scale, from_positive)?;
            Found::Symmetrized(text, moved)
        }
        Repair::Cleanup(axis) => {
            let (cleaning, text) = cleaned(&positions, &indices, &mask, axis, tolerance_scale)?;
            Found::Cleaned(text, cleaning)
        }
    })
}

/// How often a frame is drawn while a repair works, to land its result.
const POLL: Duration = Duration::from_millis(50);

/// What a finished job hands back: what the repair found, and how closed a
/// rebuilt mesh is.
type Done = (Found, Option<EdgeCensus>);

/// The model a repair is of: the scene's number for the mesh, the mesh's own
/// number, how many edits the history had seen, and how many steps it had to
/// undo and redo, when the job began. All of them must still hold for the
/// result to be that model's.
///
/// The count of steps is one more than Remesh asks of its model: an Undo
/// records no edit, and Make Symmetric hands back where each vertex goes,
/// which put on the model an Undo left would put back part of what it took
/// out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Of {
    repair: Repair,
    mesh: u64,
    uid: u64,
    edits: u64,
    steps: (usize, usize),
}

impl Of {
    fn now(repair: Repair, mesh: u64, uid: u64, history: &History) -> Of {
        Of { repair, mesh, uid, edits: history.edits, steps: history.steps() }
    }

    /// Whether the scene's mesh `mesh`, which is `slot`, recorded in
    /// `history`, is still the model this repair was started on.
    fn holds(&self, mesh: u64, slot: &Mesh, history: &History) -> bool {
        *self == Of::now(self.repair, mesh, slot.uid, history)
    }
}

/// A repair at work.
struct Pending {
    job: Job<Done>,
    of: Of,
}

/// What the app keeps for the three repairs.
#[derive(Default)]
pub(crate) struct State {
    pending: Option<Pending>,
}

/// The whole of a repair's job: [`work`], and then for a rebuilt mesh the
/// check of whether it is closed, which the status line reports.
fn in_background(cx: &JobCtx, repair: Repair, copied: Copied, name: &str, tolerance_scale: f32) -> Result<Done, String> {
    cx.progress("pairing the two sides");
    let found = work(repair, copied, name, tolerance_scale)?;
    // The pairing cannot be stopped part of the way through; what it found
    // is thrown away instead.
    if cx.cancelled() {
        return Err(cancelled(repair));
    }
    let census = match &found {
        Found::Cleaned(_, Cleaning::Rebuilt(mesh)) => {
            cx.progress("checking it is closed");
            Some(printable::census(mesh))
        }
        _ => None,
    };
    Ok((found, census))
}

/// What a cancelled repair says.
fn cancelled(repair: Repair) -> String {
    match repair {
        Repair::Check(_) => "Check Symmetry was cancelled.".to_owned(),
        _ => format!("{} was cancelled. The model is as it was.", repair.label()),
    }
}

impl Running {
    fn match_tolerance(&self) -> f32 {
        tolerance_scale(self.doc.opts.symmetry_tolerance)
    }

    /// The Check Symmetry button: says how much of the selected object pairs
    /// up across its own `axis` plane. Changes nothing.
    pub(super) fn check_symmetry(&mut self, axis: Axis) {
        self.symmetry_repair_start(Repair::Check(axis));
    }

    /// The Make Symmetric buttons. One undo step, or none if the object
    /// matched already or the repair was refused.
    pub(super) fn make_symmetric(&mut self, axis: Axis, from_positive: bool) {
        self.symmetry_repair_start(Repair::Make { axis, from_positive });
    }

    /// The Symmetry Cleanup button. One undo step, or none if the object was
    /// clean already.
    pub(super) fn symmetry_cleanup(&mut self, axis: Axis) {
        self.symmetry_repair_start(Repair::Cleanup(axis));
    }

    /// Why a repair cannot start just now, if it cannot.
    fn symmetry_repair_refusal(&self) -> Option<String> {
        if let Some(pending) = &self.symmetry_repair.pending {
            let label = pending.of.repair.label();
            return Some(format!("{label} is still working on the last one. Wait for it, or press its Cancel."));
        }
        let job = self.doc.tools.jobs.running().into_iter().next()?;
        Some(format!("{} is still working. Wait for it, or press its Cancel, then try again.", job.label))
    }

    /// Starts `repair` on the selected object, in the background.
    fn symmetry_repair_start(&mut self, repair: Repair) {
        if let Some(why) = self.symmetry_repair_refusal() {
            return self.status(why, false);
        }
        // The two that change the mesh read every vertex and triangle of it
        // and write to it where it is, so what dynamic topology left behind
        // goes first, as a step of its own (as it did before they ran in the
        // background). Check Symmetry changes nothing, and reads past it.
        if !matches!(repair, Repair::Check(_)) {
            self.tidy_merged();
        }
        let tolerance = self.match_tolerance();
        let doc = &self.doc;
        let name = doc.scene.active().name.clone();
        let copied = Copied::of(&doc.mesh);
        let job = doc.tools.jobs.spawn(repair.label(), move |cx| in_background(cx, repair, copied, &name, tolerance));
        let of = Of::now(repair, doc.scene.active_mesh(), doc.mesh.uid, &doc.history);
        self.symmetry_repair.pending = Some(Pending { job, of });
        self.redraw = true;
    }

    /// Once a frame: lands a repair that has finished.
    pub(super) fn symmetry_repair_frame(&mut self) {
        let Some(pending) = &mut self.symmetry_repair.pending else { return };
        let Some(result) = pending.job.poll() else {
            // Still working: look again shortly, whether or not anything else asks for a frame.
            let at = Instant::now() + POLL;
            self.repaint_at = Some(self.repaint_at.map_or(at, |t| t.min(at)));
            return;
        };
        let of = pending.of;
        self.symmetry_repair.pending = None;
        self.symmetry_repair_land(of, result);
    }

    /// For a run with no frames to come: waits for the repair at work, and lands it.
    pub(super) fn symmetry_repair_wait(&mut self) {
        let Some(Pending { job, of }) = self.symmetry_repair.pending.take() else { return };
        let result = job.wait();
        self.symmetry_repair_land(of, result);
    }

    /// Does to the model what a finished repair found, as one undo step, if
    /// the model is still the one it was worked out on.
    fn symmetry_repair_land(&mut self, of: Of, result: Result<Done, String>) {
        self.redraw = true;
        let repair = of.repair;
        let label = repair.label();
        let (found, census) = match result {
            Ok(done) => done,
            // A cancel is the user's own doing, and no bad news.
            Err(why) => {
                let ok = why == cancelled(repair);
                return self.status(why, ok);
            }
        };
        // A stroke or a gizmo drag has the mesh while it runs: it ends first.
        if self.doc.sculptor.is_active() || self.transform_dragging() || self.deform_holds_mesh() {
            let text = format!("{label} finished in the middle of a stroke, so its result was not used. Press it again.");
            return self.status(text, false);
        }
        let doc = &mut self.doc;
        doc.scene.focus_mesh(of.mesh, &mut doc.mesh);
        let same = of.holds(doc.scene.active_mesh(), &doc.mesh, &doc.history);
        self.scene_changed();
        if !same {
            let text = format!("The model was changed while {label} was working, so its result was not used. Press it again.");
            return self.status(text, false);
        }
        let doc = &mut self.doc;
        // The undo step is called by the button, as it was when the button did the work itself.
        doc.history.name_next(Some(label.to_owned()));
        match found {
            Found::Checked(text, ok) => {
                self.toast(label);
                self.status(text, ok);
            }
            Found::Symmetrized(text, moved) => {
                // Reset Shape goes back to the shape before the first thing that moved the mesh (scene.rs).
                doc.scene.keep_baseline(&doc.mesh);
                let text = text + &commit(&mut doc.mesh, &mut doc.history, &moved);
                // The surface may have moved out from under the last brush dab.
                doc.sculptor.last_hit = None;
                self.toast(label);
                self.status(text, true);
            }
            Found::Cleaned(text, cleaning) => {
                doc.scene.keep_baseline(&doc.mesh);
                match apply_cleaning(&mut doc.mesh, &mut doc.history, cleaning, text) {
                    (Cleanup::Rebuilt(mesh), text) => {
                        self.replace_mesh(mesh, true);
                        self.toast(label);
                        // A weld closes a seam; say if the model has other holes still.
                        let name = self.doc.scene.active().name.clone();
                        let census = census.unwrap_or_else(|| printable::census(&self.doc.mesh));
                        match printable::warning(&[(&name, census)], Cure::Remesh) {
                            Some(warning) => self.status(format!("{text} {warning}"), false),
                            None => self.status(text, true),
                        }
                    }
                    (_, text) => {
                        self.doc.sculptor.last_hit = None;
                        self.toast(label);
                        self.status(text, true);
                    }
                }
            }
        }
        self.doc.history.name_next(None);
    }
}

mod demo;
#[cfg(test)]
mod tests;
