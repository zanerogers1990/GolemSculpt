//! Quad Remesh, Unwrap UVs, Project Detail, Open and Import, run in the
//! background.
//!
//! Each of them works something out (a new shape for the selected object, or
//! the objects of a file) and then puts it in place as one undo step. The
//! working out took seconds on a big model and was done on the window's own
//! thread, which froze the window until it was over. Now it is done the way
//! Remesh does it (app/remesh_job.rs):
//!
//! - **On a job of its own**, on copies of what it reads, taken when the
//!   button was pressed. Its progress and a Cancel button show at the bottom
//!   of the view, and the window goes on drawing.
//! - **A new shape lands only on the model it was made from** ([`Stamp`]):
//!   the same object's mesh, the same mesh, and no edit recorded since.
//!   Otherwise the result is of a model that is no longer there, and is
//!   thrown away with a line that says so. A file read for Open or Import
//!   lands whatever was done meanwhile: it is one undo step over it.
//! - **Never in the middle of a stroke or a gizmo drag**: a result that is
//!   ready then waits for it to end.
//! - **One at a time**, and not while another background job is at work.
//! - **A scripted run waits for it**, as if it were done there and then: a
//!   run with options has no person in front of it, and its checks read the
//!   result on the next line. So does the Claude bridge (`behind_wait`).
//!
//! What lands is a closure the job hands back, which runs on the window's
//! thread with the app in hand: each action says there what it says.

use std::time::{Duration, Instant};

use sculpt_toolkit::{Job, JobCtx};

use super::Running;

/// How often a frame is drawn while a job works, to land its result.
const POLL: Duration = Duration::from_millis(50);

/// What a finished job hands back to be done on the window's thread.
pub(super) type Land = Box<dyn FnOnce(&mut Running) + Send>;

/// The model a background job is of: the scene's number for the active
/// object's mesh, the mesh's own number, and how many edits the history had
/// recorded when the job began. All three must still hold for the result to
/// be that model's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Stamp {
    mesh: u64,
    uid: u64,
    edits: u64,
}

struct Pending {
    label: &'static str,
    job: Job<Land>,
    /// The model a new shape is for; `None` for a file read (Open, Import).
    of: Option<Stamp>,
    started: Instant,
}

/// What the app keeps for the jobs here.
#[derive(Default)]
pub(super) struct State {
    pending: Option<Pending>,
}

/// What a cancelled job says.
fn cancelled(label: &str) -> String {
    format!("{label} was cancelled. Nothing was changed.")
}

impl Running {
    /// The model as it is now, for a job to be checked against when it lands.
    pub(super) fn model_stamp(&self) -> Stamp {
        Stamp { mesh: self.doc.scene.active_mesh(), uid: self.doc.mesh.uid, edits: self.doc.history.edits }
    }

    /// Whether the model is still the one `of` was taken of, with the
    /// object it belongs to selected again if another was selected meanwhile.
    pub(super) fn still_the_model(&mut self, of: Stamp) -> bool {
        let doc = &mut self.doc;
        doc.scene.focus_mesh(of.mesh, &mut doc.mesh);
        let same = self.model_stamp() == of;
        self.scene_changed();
        same
    }

    /// Why a background job cannot start just now, if it cannot. `on_model`:
    /// it works on the selected object's model, which must have something in it.
    fn behind_refusal(&self, label: &str, on_model: bool) -> Option<String> {
        if let Some(pending) = &self.behind.pending {
            return Some(format!("{} is still working. Wait for it, or press its Cancel, then {label}.", pending.label));
        }
        if let Some(job) = self.doc.tools.jobs.running().first() {
            return Some(format!("{} is still working. Wait for it, or press its Cancel, then {label}.", job.label));
        }
        (on_model && self.doc.mesh.triangle_count() == 0).then(|| format!("There is no model to {label}."))
    }

    /// Starts `work` in the background as `label`, for the selected object
    /// as it is now. What it hands back is run when it lands, if the model
    /// is still the one it was made from.
    pub(super) fn run_behind(
        &mut self,
        label: &'static str,
        work: impl FnOnce(&JobCtx) -> Result<Land, String> + Send + 'static,
    ) {
        self.start_behind(label, true, work);
    }

    /// [`Self::run_behind`] for reading a file, whose result lands on
    /// whatever the scene is by then (Open, Import).
    pub(super) fn read_behind(
        &mut self,
        label: &'static str,
        work: impl FnOnce(&JobCtx) -> Result<Land, String> + Send + 'static,
    ) {
        self.start_behind(label, false, work);
    }

    fn start_behind(
        &mut self,
        label: &'static str,
        on_model: bool,
        work: impl FnOnce(&JobCtx) -> Result<Land, String> + Send + 'static,
    ) {
        if let Some(why) = self.behind_refusal(label, on_model) {
            return self.status(why, false);
        }
        let job = self.doc.tools.jobs.spawn(label, move |cx| {
            let land = work(cx)?;
            if cx.cancelled() {
                return Err(cancelled(label));
            }
            Ok(land)
        });
        let of = on_model.then(|| self.model_stamp());
        let pending = Pending { label, job, of, started: Instant::now() };
        self.behind.pending = Some(pending);
        self.redraw = true;
        if crate::prefs::scripted() {
            self.behind_wait();
        }
    }

    /// Once a frame: lands a job that has finished.
    pub(super) fn behind_frame(&mut self) {
        // A stroke or a gizmo drag has the mesh while it runs: a result waits for it to end.
        let busy = self.mesh_held();
        let Some(pending) = &mut self.behind.pending else { return };
        let ready = if busy { None } else { pending.job.poll() };
        let Some(result) = ready else {
            let at = Instant::now() + POLL;
            self.repaint_at = Some(self.repaint_at.map_or(at, |t| t.min(at)));
            return;
        };
        let Some(Pending { label, of, started, .. }) = self.behind.pending.take() else { return };
        self.behind_land(label, of, started, result);
    }

    /// For a run with no frames to come: waits for the job at work, and lands it.
    pub(super) fn behind_wait(&mut self) {
        let Some(Pending { label, job, of, started }) = self.behind.pending.take() else { return };
        let result = job.wait();
        self.behind_land(label, of, started, result);
    }

    /// Whether a stroke or a gizmo drag has the mesh just now.
    fn mesh_held(&self) -> bool {
        self.doc.sculptor.is_active() || self.transform_dragging() || self.deform_holds_mesh()
    }

    fn behind_land(&mut self, label: &str, of: Option<Stamp>, started: Instant, result: Result<Land, String>) {
        self.redraw = true;
        let land = match result {
            Ok(land) => land,
            // A cancel is the user's own doing, and no bad news.
            Err(why) => {
                let ok = why == cancelled(label);
                return self.status(why, ok);
            }
        };
        // Only a run that waits for the job gets here mid-stroke (`behind_wait`).
        if self.mesh_held() {
            let text = format!("{label} finished in the middle of a stroke, so its result was not used. Press {label} again.");
            return self.status(text, false);
        }
        if of.is_some_and(|of| !self.still_the_model(of)) {
            let text = format!("The model was changed while {label} was working, so its result was not used. Press {label} again.");
            return self.status(text, false);
        }
        log::info!("{label} landed after {:.0} ms", started.elapsed().as_secs_f64() * 1000.0);
        // Its undo step is called what the button was (app/undo_names.rs).
        self.doc.history.name_next(Some(label.to_owned()));
        land(self);
        self.doc.history.name_next(None);
    }
}

impl Running {
    /// The Quad Remesh button: the selected object remeshed into about the
    /// Quad Remesh card's number of quads, in the background.
    pub(super) fn quad_remesh_start(&mut self) {
        let target = self.doc.opts.quad_target as usize;
        let (positions, indices) = (self.doc.mesh.positions.clone(), self.doc.mesh.indices.clone());
        self.run_behind("Quad Remesh", move |cx| {
            // A quad remesh keeps the holes a model has, and can leave new ones.
            cx.progress("looking for holes");
            let was_closed = sculpt_geom::edge_census(positions.len(), &indices).is_closed();
            cx.progress("laying out the quads");
            let (mesh, report) = crate::quadremesh::quad_remesh_of(&positions, &indices, target)?;
            Ok(Box::new(move |app: &mut Running| {
                app.replace_mesh(mesh, true);
                // The quads are the point: show them.
                app.doc.opts.wireframe = true;
                app.toast("Quad Remesh");
                let closed = report.census().is_closed();
                let text = format!("Quad remesh: {}.{}", report.summary(), report.advice(was_closed));
                app.status(text, closed);
            }) as Land)
        });
    }
}
