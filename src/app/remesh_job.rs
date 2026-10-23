//! Remesh, on the app's side: the Remesh button of the Remesh card, and R.
//!
//! What it does to a mesh is `crate::remesh` (and under it `sculpt_volume`).
//! Here is how the running app carries it out, which is the way Decimate
//! does (app/decimate.rs):
//!
//! - **It runs in the background**, on a job of its own: its progress and a
//!   Cancel button show at the bottom of the view, and the window goes on
//!   drawing. Since the Resolution slider goes to 4096, a remesh can make 80
//!   million triangles, and building those takes seconds; done in the
//!   window's own thread, that froze it until it was over. The job works on
//!   a copy of the mesh as it was when the button was pressed, and does all
//!   of it there: the rebuild, the sculpting structures of the result, and
//!   the check of whether it is closed.
//! - **It lands as one undo step**, the old mesh kept whole, if the model is
//!   still as it was: the same mesh, and no edit recorded since. Otherwise
//!   the result is of a model that is no longer there, and is thrown away
//!   with a line that says so.
//! - **One at a time**, and not while another background job is at work.
//!
//! A run that has no frames to land it in (a scripted play, the Claude
//! bridge) waits for it: `remesh_wait`.

use std::time::Instant;

use sculpt_geom::EdgeCensus;
use sculpt_toolkit::Job;

use super::Running;
use crate::mesh::Mesh;
use crate::printable::{self, Cure};
use crate::remesh::{self, Copied, Notes};

/// How often a frame is drawn while a remesh works, to land its result.
const POLL: std::time::Duration = std::time::Duration::from_millis(50);

/// What a finished remesh hands back: the new mesh, what it did about a
/// model that was not closed, and how closed the result is.
type Done = (Mesh, Notes, EdgeCensus);

/// The model a remesh is of: the scene's number for the mesh, the mesh's own
/// number, and how many edits the history had seen when the job began. All
/// three must still hold for the result to be that model's.
#[derive(Clone, Copy)]
struct Of {
    mesh: u64,
    uid: u64,
    edits: u64,
    started: Instant,
}

/// A remesh at work.
struct Pending {
    job: Job<Done>,
    of: Of,
}

/// What the app keeps for Remesh.
#[derive(Default)]
pub(super) struct State {
    pending: Option<Pending>,
}

impl Running {
    /// Why a remesh cannot start just now, if it cannot.
    fn remesh_refusal(&self) -> Option<String> {
        if self.remesh.pending.is_some() {
            return Some("Remesh is still working on the last one. Wait for it, or press its Cancel.".into());
        }
        if let Some(job) = self.doc.tools.jobs.running().first() {
            return Some(format!("{} is still working. Wait for it, or press its Cancel, then remesh.", job.label));
        }
        (self.doc.mesh.triangle_count() == 0).then(|| "Nothing to remesh.".into())
    }

    /// Starts remeshing the selected object at the Remesh card's Resolution
    /// and Smooth, in the background, the whole model as it is, whatever the
    /// brush symmetry.
    pub(super) fn remesh_start(&mut self) {
        if let Some(why) = self.remesh_refusal() {
            return self.status(why, false);
        }
        let doc = &self.doc;
        let resolution = doc.opts.remesh_resolution as u32;
        let smooth = doc.opts.remesh_smooth as u32;
        let copied = Copied::of(&doc.mesh);
        let job = doc.tools.jobs.spawn("Remesh", move |cx| {
            cx.progress("rebuilding the surface");
            let (mesh, notes) = copied.remesh(resolution, remesh::AS_IT_IS, smooth)?;
            drop(copied);
            if cx.cancelled() {
                return Err(CANCELLED.into());
            }
            cx.progress("checking it is closed");
            let census = printable::census(&mesh);
            Ok((mesh, notes, census))
        });
        let of = Of { mesh: doc.scene.active_mesh(), uid: doc.mesh.uid, edits: doc.history.edits, started: Instant::now() };
        self.remesh.pending = Some(Pending { job, of });
        self.redraw = true;
    }

    /// Once a frame: lands a remesh that has finished.
    pub(super) fn remesh_frame(&mut self) {
        let Some(pending) = &mut self.remesh.pending else { return };
        let Some(result) = pending.job.poll() else {
            // Still working: look again shortly, whether or not anything else asks for a
            // frame (as Decimate does).
            let at = Instant::now() + POLL;
            self.repaint_at = Some(self.repaint_at.map_or(at, |t| t.min(at)));
            return;
        };
        let of = pending.of;
        self.remesh.pending = None;
        self.remesh_land(of, result);
    }

    /// For a run with no frames to come: waits for the remesh at work, and lands it.
    pub(super) fn remesh_wait(&mut self) {
        let Some(Pending { job, of }) = self.remesh.pending.take() else { return };
        let result = job.wait();
        self.remesh_land(of, result);
    }

    /// Puts a finished remesh in the model's place as one undo step, if the
    /// model is still the one it was made from.
    fn remesh_land(&mut self, pending: Of, result: Result<Done, String>) {
        self.redraw = true;
        let (mesh, notes, census) = match result {
            Ok(done) => done,
            // A cancel is the user's own doing, and no bad news. The kernel's
            // refusals say "The model"; the line says which.
            Err(why) => {
                let (ok, name) = (why == CANCELLED, self.doc.scene.active().name.clone());
                return self.status(remesh::named(why, &name), ok);
            }
        };
        // A stroke or a gizmo drag has the mesh while it runs: it ends first.
        if self.doc.sculptor.is_active() || self.transform_dragging() || self.deform_holds_mesh() {
            return self.status("Remesh finished in the middle of a stroke, so its result was not used. Press Remesh again.", false);
        }
        let doc = &mut self.doc;
        doc.scene.focus_mesh(pending.mesh, &mut doc.mesh);
        let same = doc.scene.active_mesh() == pending.mesh && doc.mesh.uid == pending.uid && doc.history.edits == pending.edits;
        self.scene_changed();
        if !same {
            let text = "The model was changed while Remesh was working, so its result was not used. Press Remesh again.";
            return self.status(text, false);
        }
        let name = self.doc.scene.active().name.clone();
        let tris = mesh.triangle_count();
        self.replace_mesh(mesh, true);
        self.toast("Remesh");
        let ms = pending.started.elapsed().as_secs_f64() * 1000.0;
        // What it did about holes the model had, then whether the result is closed.
        let text = format!("Remeshed to {tris} triangles in {ms:.0} ms, both sides as they were.{}", notes.after_remesh());
        match printable::warning(&[(&name, census)], Cure::OtherResolution) {
            Some(warning) => self.status(format!("{text} {warning}"), false),
            None => self.status(text, true),
        }
    }
}

/// What a cancelled remesh says.
const CANCELLED: &str = "Remesh was cancelled. The model is as it was.";
