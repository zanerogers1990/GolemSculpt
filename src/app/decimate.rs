//! Decimate, on the app's side: the Decimate and Preview buttons of the
//! Remesh card.
//!
//! What it does to a mesh is `crate::decimate` (and under it
//! `sculpt_geom::decimate`). Here is how the running app carries it out.
//!
//! - **A mirror is kept.** With a Local mirror on, the model is decimated
//!   so that its sides come out mirror images of each other, point for
//!   point (`crate::decimate::Source::mirrored`), with the Symmetry card's
//!   Match Tolerance. A World symmetry's planes cut the mesh anywhere and
//!   were never exact in it, so the model is decimated whole.
//! - **It asks first when the object has a Local radial symmetry on**,
//!   whose repeats are not kept: each is thinned in the order its own costs
//!   come. The choice was between refusing and asking. A refusal would be
//!   got past by unticking the radial symmetry, decimating, and ticking it
//!   again, which changes nothing about what comes out; the only real
//!   decision is whether losing the exact match is acceptable, so that is
//!   what is asked (Decimate Anyway, or Cancel).
//! - **It runs in the background**, on a job of its own
//!   (`sculpt_toolkit::Jobs`, the same list the tools' merges and cuts are
//!   on): its progress and a Cancel button show at the bottom of the view,
//!   and sculpting carries on. The job works on a copy of the mesh as it
//!   was when the button was pressed.
//! - **It lands as one undo step**, the old mesh kept whole, if the model is
//!   still as it was: the same mesh, and no edit recorded since. Otherwise
//!   the result is of a model that is no longer there, and is thrown away
//!   with a line that says so.
//! - **One at a time**, and not while another background job is at work.
//!
//! **The preview.** Preview works the decimation out once, in the
//! background, down to the slider's lowest count (`crate::decimate::
//! prepared`); from then on the model is drawn as it would be at whatever
//! count Target Triangles says, put together on a thread of its own in a
//! few hundredths of a second whenever the count changes, while the model
//! itself is not touched: the renderer is handed the preview in its place
//! (`State::shown_mut`). So saving, the recovery copies and every command
//! still see the model as it is. Apply (Enter, or the Decimate button)
//! puts what is shown in its place as one undo step, the very mesh Decimate
//! would have made; Cancel (Esc) closes the preview. While it is open a
//! press on the model turns the view rather than sculpting, Undo closes it,
//! and any other command that is not only about the view closes it first
//! and then runs. A model changed under it some other way (the Claude
//! bridge, say) closes it too, with a line that says so.
//!
//! A scripted run has no later frame to land in before its picture is
//! taken, so `--decimate` waits for the job (`decimate_wait`), and the
//! demo waits for the preview (`decimate_preview_wait`).

use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Instant;

use sculpt_toolkit::Job;
use winit::keyboard::KeyCode;

use super::Running;
use super::symmetry_repair::tolerance_scale;
use crate::decimate::{self, Prepared, Report, Source, TARGET_RANGE};
use crate::doc::Action;
use crate::mesh::Mesh;
use crate::scene::{Space, Symmetry};
use crate::ui::question::Question;

/// How often a frame is drawn while a decimation works, to land its result.
const POLL: std::time::Duration = std::time::Duration::from_millis(50);

/// The model a decimation is of: the scene's number for the mesh, the mesh's
/// own number, and how many edits the history had seen when the job began.
/// All three must still hold for the result to be that model's.
#[derive(Clone, Copy)]
struct Of {
    mesh: u64,
    uid: u64,
    edits: u64,
    started: Instant,
}

/// A decimation at work.
struct Pending {
    job: Job<(Mesh, Report)>,
    of: Of,
}

/// A preview of Decimate.
enum Preview {
    /// Being worked out, in the background.
    Preparing { job: Job<Prepared>, of: Of },
    Open(Box<Open>),
}

/// A preview worked out: any count shows at once.
struct Open {
    ready: Arc<Prepared>,
    of: Of,
    /// How long working it out took, for the status line.
    took: f64,
    /// What is shown, when something is.
    shown: Option<Shown>,
    /// Another count being put together, on a thread of its own.
    making: Option<(usize, JoinHandle<(Mesh, Report)>)>,
}

/// The model decimated to `target` triangles, shown in its place.
struct Shown {
    target: usize,
    mesh: Mesh,
    report: Report,
}

/// What the app keeps for Decimate.
#[derive(Default)]
pub(super) struct State {
    pending: Option<Pending>,
    preview: Option<Preview>,
}

impl State {
    /// The mesh a preview shows in the selected object's place, when one is
    /// shown: what the renderer is handed instead of the model.
    pub(super) fn shown_mut(&mut self) -> Option<&mut Mesh> {
        match &mut self.preview {
            Some(Preview::Open(open)) => open.shown.as_mut().map(|shown| &mut shown.mesh),
            _ => None,
        }
    }

    /// Whether a preview is open, worked out or not.
    pub(super) fn previewing(&self) -> bool {
        self.preview.is_some()
    }
}

/// What is asked before an object with a Local radial symmetry on is
/// decimated; `None` when there is nothing to ask. A mirror is kept, and a
/// World symmetry was never exact in the mesh.
pub(crate) fn symmetry_question(name: &str, symmetry: Symmetry) -> Option<String> {
    if symmetry.space != Space::Local || !symmetry.radial.on {
        return None;
    }
    let mirror = if symmetry.mirror.contains(&true) { " and its mirror image exactly" } else { "" };
    Some(format!(
        "\"{name}\" is sculpted with radial symmetry on ({}). Decimating keeps its shape{mirror}, but not its radial \
         repeats: each is thinned in its own order, so their points no longer match. Brushes still work on all of \
         them. Nothing puts the repeats back exactly afterwards. Decimate it?",
        symmetry.summary()
    ))
}

/// The count to decimate to: what the slider says, held to what the app asks
/// for and to a whole number.
pub(crate) fn target_of(setting: f32) -> usize {
    let setting = if setting.is_finite() { setting } else { *TARGET_RANGE.start() };
    setting.clamp(*TARGET_RANGE.start(), *TARGET_RANGE.end()).round() as usize
}

/// Whether an action only changes how the model is seen, so that a preview
/// stays open through it.
fn only_the_view(action: &Action) -> bool {
    matches!(
        action,
        Action::Matcap(_)
            | Action::RescanMatcaps
            | Action::Frame
            | Action::Snap(..)
            | Action::ToggleOrtho
            | Action::Lens(_)
            | Action::PanView(_)
            | Action::ZoomView(_)
            | Action::HomeView
            | Action::Turn { .. }
            | Action::Brush(_)
            | Action::RescanAlphas
    )
}

impl Running {
    /// The Decimate button: asks first if the object's radial symmetry would
    /// no longer be exact, else starts. With the preview open, keeps what it
    /// shows.
    pub(super) fn decimate_asking(&mut self) {
        if self.decimate.preview.is_some() {
            return self.decimate_apply();
        }
        let scene = &self.doc.scene;
        let (object, name) = (scene.active_id(), scene.active().name.clone());
        match symmetry_question(&name, scene.active_symmetry()) {
            Some(text) if self.decimate_refusal().is_none() => {
                self.ui.graph.ask(Question::DecimateSymmetric { object, text });
                self.redraw = true;
            }
            _ => self.decimate_start(),
        }
    }

    /// Why a decimation cannot start just now, if it cannot.
    fn decimate_refusal(&self) -> Option<String> {
        let doc = &self.doc;
        if self.decimate.pending.is_some() {
            return Some("Decimate is still working on the last one. Wait for it, or press its Cancel.".into());
        }
        if let Some(job) = doc.tools.jobs.running().first() {
            return Some(format!("{} is still working. Wait for it, or press its Cancel, then decimate.", job.label));
        }
        let (triangles, target) = (doc.mesh.live_triangle_count(), target_of(doc.opts.decimate_target));
        if triangles == 0 {
            return Some("Nothing to decimate.".into());
        }
        (target >= triangles).then(|| {
            format!(
                "\"{}\" has {} triangles already, which is no more than the {} asked for. Lower Target Triangles \
                 to decimate.",
                doc.scene.active().name,
                crate::printable::grouped(triangles),
                crate::printable::grouped(target)
            )
        })
    }

    /// What a decimation of the selected object reads: its mesh, without the
    /// points dynamic topology's merges leave, kept mirror-symmetric when it
    /// has a Local mirror on.
    fn decimate_source(&self) -> Source {
        let doc = &self.doc;
        let tidy = crate::scene::tidied(&doc.mesh);
        let source = Source::of(tidy.as_ref().unwrap_or(&doc.mesh));
        let symmetry = doc.scene.active_symmetry();
        match symmetry.space == Space::Local && symmetry.mirror.contains(&true) {
            true => source.mirrored(symmetry.mirror, tolerance_scale(doc.opts.symmetry_tolerance)),
            false => source,
        }
    }

    /// The selected object's model as it is now, for a decimation to be of.
    fn decimate_of(&self) -> Of {
        let doc = &self.doc;
        Of { mesh: doc.scene.active_mesh(), uid: doc.mesh.uid, edits: doc.history.edits, started: Instant::now() }
    }

    /// Whether the model is still the one `of` is (it may not be the
    /// selected object's any more).
    fn decimate_still(&self, of: Of) -> bool {
        let doc = &self.doc;
        doc.scene.active_mesh() == of.mesh && doc.mesh.uid == of.uid && doc.history.edits == of.edits
    }

    /// Starts decimating the selected object to the Target Triangles of the
    /// Remesh card, in the background. (The answer Decimate Anyway comes
    /// straight here.) With the preview open, keeps what it shows.
    pub(super) fn decimate_start(&mut self) {
        if self.decimate.preview.is_some() {
            return self.decimate_apply();
        }
        if let Some(why) = self.decimate_refusal() {
            // Not an error when it is only that there is nothing to take off.
            let waiting = self.decimate.pending.is_some() || self.doc.tools.jobs.is_busy();
            return self.status(why, !waiting && self.doc.mesh.live_triangle_count() > 0);
        }
        let target = target_of(self.doc.opts.decimate_target);
        let source = self.decimate_source();
        let job = self.doc.tools.jobs.spawn("Decimate", move |cx| {
            cx.progress("0%");
            let mut progress = |done: f32| {
                cx.progress(format!("{:.0}%", done * 100.0));
                !cx.cancelled()
            };
            match decimate::decimated(&source, target, &mut progress)? {
                Some(done) => Ok(done),
                None => Err("Decimate was cancelled. The model is as it was.".into()),
            }
        });
        self.decimate.pending = Some(Pending { job, of: self.decimate_of() });
        self.redraw = true;
    }

    /// Once a frame: lands a decimation that has finished, and keeps the
    /// preview up to date.
    pub(super) fn decimate_frame(&mut self) {
        self.decimate_preview_frame();
        let Some(pending) = &mut self.decimate.pending else { return };
        let Some(result) = pending.job.poll() else {
            // Still working: look again shortly, whether or not anything else asks for a
            // frame. (The list of running jobs asks too, but stops the moment the job is
            // done, which can be just after this look.)
            self.poll_again();
            return;
        };
        let of = pending.of;
        self.decimate.pending = None;
        self.decimate_land(of, result);
    }

    /// Asks for a frame shortly, whether or not anything else does.
    fn poll_again(&mut self) {
        let at = Instant::now() + POLL;
        self.repaint_at = Some(self.repaint_at.map_or(at, |t| t.min(at)));
    }

    /// For a scripted run: waits for the decimation at work, and lands it.
    pub(super) fn decimate_wait(&mut self) {
        let Some(Pending { job, of }) = self.decimate.pending.take() else { return };
        let result = job.wait();
        self.decimate_land(of, result);
    }

    /// Puts a finished decimation in the model's place as one undo step, if
    /// the model is still the one it was made from.
    fn decimate_land(&mut self, pending: Of, result: Result<(Mesh, Report), String>) {
        self.redraw = true;
        let (mesh, report) = match result {
            Ok(done) => done,
            // A cancel is the user's own doing, and no bad news.
            Err(why) => return self.status(&why, why.contains("cancelled")),
        };
        // A stroke or a gizmo drag has the mesh while it runs: it ends first.
        if self.doc.sculptor.is_active() || self.transform_dragging() || self.deform_holds_mesh() {
            return self.status("Decimate finished in the middle of a stroke, so its result was not used. Press Decimate again.", false);
        }
        let doc = &mut self.doc;
        doc.scene.focus_mesh(pending.mesh, &mut doc.mesh);
        let same = self.decimate_still(pending);
        self.scene_changed();
        if !same {
            let text = "The model was changed while Decimate was working, so its result was not used. Press Decimate again.";
            return self.status(text, false);
        }
        let name = self.doc.scene.active().name.clone();
        self.replace_mesh(mesh, true);
        self.toast("Decimate");
        let (text, ok) = decimate::line(&name, &report, pending.started.elapsed().as_secs_f64());
        self.status(text, ok);
    }

    // ---- the preview ---------------------------------------------------------

    /// The Preview button: works the decimation out in the background, then
    /// shows the model at the slider's count.
    pub(super) fn decimate_preview_start(&mut self) {
        if self.decimate.preview.is_some() {
            return;
        }
        if let Some(why) = self.decimate_refusal() {
            let waiting = self.decimate.pending.is_some() || self.doc.tools.jobs.is_busy();
            return self.status(why, !waiting && self.doc.mesh.live_triangle_count() > 0);
        }
        let source = self.decimate_source();
        let job = self.doc.tools.jobs.spawn("Decimate Preview", move |cx| {
            cx.progress("0%");
            let mut progress = |done: f32| {
                cx.progress(format!("{:.0}%", done * 100.0));
                !cx.cancelled()
            };
            match decimate::prepared(source, &mut progress)? {
                Some(ready) => Ok(ready),
                None => Err("The Decimate preview was cancelled. The model is as it was.".into()),
            }
        });
        self.decimate.preview = Some(Preview::Preparing { job, of: self.decimate_of() });
        self.redraw = true;
    }

    /// Once a frame: a preview worked out is opened, a count asked for is put
    /// together, one put together is shown, and one of a model that has
    /// changed is closed. Tells the card what there is to tell.
    fn decimate_preview_frame(&mut self) {
        let of = match &self.decimate.preview {
            None => {
                self.doc.opts.decimate_preview = Default::default();
                return;
            }
            Some(Preview::Preparing { of, .. }) => *of,
            Some(Preview::Open(open)) => open.of,
        };
        if !self.decimate_still(of) {
            return self.decimate_close("The model was changed, so the Decimate preview was closed. The model is as it was.", false);
        }
        let polled = match &mut self.decimate.preview {
            Some(Preview::Preparing { job, of }) => Some((job.poll(), *of)),
            _ => None,
        };
        match polled {
            Some((None, _)) => self.poll_again(),
            Some((Some(Err(why)), _)) => {
                self.decimate.preview = None;
                self.redraw = true;
                self.status(&why, why.contains("cancelled"));
            }
            Some((Some(Ok(ready)), of)) => self.decimate_opened(of, ready),
            None => {}
        }
        let target = target_of(self.doc.opts.decimate_target);
        let mut card = crate::ui::decimate::Preview { open: self.decimate.preview.is_some(), ..Default::default() };
        let mut again = false;
        if let Some(Preview::Open(open)) = &mut self.decimate.preview {
            // A count put together is shown.
            if open.making.as_ref().is_some_and(|(_, thread)| thread.is_finished()) {
                let (count, thread) = open.making.take().expect("just looked");
                if let Ok((mesh, report)) = thread.join() {
                    open.shown = Some(Shown { target: count, mesh, report });
                    self.redraw = true;
                }
            }
            // And the count asked for is put together, if it is not the one shown.
            if open.making.is_none() && open.shown.as_ref().is_none_or(|shown| shown.target != target) {
                let ready = Arc::clone(&open.ready);
                open.making = Some((target, std::thread::spawn(move || ready.at(target))));
            }
            card = crate::ui::decimate::Preview {
                open: true,
                ready: true,
                shown: open.shown.as_ref().map(|shown| shown.report.after),
                fewest: open.ready.fewest(),
            };
            again = open.making.is_some();
        }
        if again {
            self.poll_again();
        }
        self.doc.opts.decimate_preview = card;
    }

    /// A preview worked out: it is open, and the line says how to use it.
    fn decimate_opened(&mut self, of: Of, ready: Prepared) {
        let took = of.started.elapsed().as_secs_f64();
        let unmirrored = ready.unmirrored();
        let open = Open { ready: Arc::new(ready), of, took, shown: None, making: None };
        self.decimate.preview = Some(Preview::Open(Box::new(open)));
        self.redraw = true;
        let name = self.doc.scene.active().name.clone();
        let radial = symmetry_question(&name, self.doc.scene.active_symmetry()).is_some();
        let mut text = format!(
            "Previewing Decimate of \"{name}\": drag Target Triangles to see other counts. Apply (Enter) keeps what is \
             shown; Cancel (Esc) leaves the model as it was."
        );
        if radial {
            text += " Its radial repeats will not match exactly.";
        }
        if unmirrored {
            text += " Its sides are not mirror images of each other, so it is decimated as it is and they will not pair up.";
        }
        self.status(text, !radial && !unmirrored);
    }

    /// Apply (Enter): what the preview shows takes the model's place, as one
    /// undo step: the very mesh Decimate makes for the count.
    pub(super) fn decimate_apply(&mut self) {
        match &self.decimate.preview {
            Some(Preview::Open(_)) => {}
            Some(Preview::Preparing { .. }) => return self.status("The preview is still being worked out. Apply when it shows.", true),
            None => return,
        }
        let Some(Preview::Open(open)) = self.decimate.preview.take() else { unreachable!("just looked") };
        self.redraw = true;
        let open = *open;
        if !self.decimate_still(open.of) {
            return self.status("The model was changed, so the Decimate preview was closed. The model is as it was.", false);
        }
        let target = target_of(self.doc.opts.decimate_target);
        let (mesh, report) = match open.shown {
            Some(shown) if shown.target == target => (shown.mesh, shown.report),
            _ => open.ready.at(target),
        };
        let name = self.doc.scene.active().name.clone();
        self.replace_mesh(mesh, true);
        self.toast("Decimate");
        let (text, ok) = decimate::line(&name, &report, open.took);
        self.status(text, ok);
    }

    /// Closes the preview, and says `why` on the status line (`ok`: as no
    /// bad news). A preview still being worked out is stopped.
    fn decimate_close(&mut self, why: &str, ok: bool) {
        if let Some(Preview::Preparing { job, .. }) = self.decimate.preview.take() {
            job.cancel();
        }
        self.doc.opts.decimate_preview = Default::default();
        self.redraw = true;
        self.status(why, ok);
    }

    /// Cancel (Esc): the preview is closed, the model as it was.
    pub(super) fn decimate_cancel(&mut self) {
        if self.decimate.preview.is_some() {
            self.decimate_close("Decimate preview closed. The model is as it was.", true);
        }
    }

    /// Before any command: with the preview open, Undo and Redo only close
    /// it, a command about the view leaves it open, and any other closes it
    /// first and then runs. True if the command was taken.
    pub(super) fn decimate_before(&mut self, action: &Action) -> bool {
        if self.decimate.preview.is_none() || only_the_view(action) {
            return false;
        }
        match action {
            Action::Decimate | Action::DecimateAnyway | Action::DecimatePreview | Action::DecimateApply | Action::DecimateCancel => false,
            Action::Undo | Action::Redo | Action::UndoSteps(_) | Action::RedoSteps(_) => {
                self.decimate_cancel();
                true
            }
            _ => {
                self.decimate_cancel();
                false
            }
        }
    }

    /// Enter and Esc while the preview is open. True if the key was taken.
    pub(super) fn decimate_key(&mut self, code: KeyCode) -> bool {
        if self.decimate.preview.is_none() || self.input.drag != super::Drag::None {
            return false;
        }
        match code {
            KeyCode::Enter | KeyCode::NumpadEnter => self.act(Action::DecimateApply),
            KeyCode::Escape => self.act(Action::DecimateCancel),
            _ => return false,
        }
        true
    }

    /// For a scripted run: waits until the preview shows the slider's count.
    pub(super) fn decimate_preview_wait(&mut self) {
        if let Some(Preview::Preparing { .. }) = &self.decimate.preview {
            let Some(Preview::Preparing { job, of }) = self.decimate.preview.take() else { unreachable!("just looked") };
            match job.wait() {
                Ok(ready) => self.decimate_opened(of, ready),
                Err(why) => return self.status(&why, false),
            }
        }
        // Once to start putting the count together, once to show it.
        for _ in 0..2 {
            self.decimate_preview_frame();
            if let Some(Preview::Open(open)) = &mut self.decimate.preview {
                if let Some((count, thread)) = open.making.take() {
                    if let Ok((mesh, report)) = thread.join() {
                        open.shown = Some(Shown { target: count, mesh, report });
                    }
                }
            }
        }
        self.decimate_preview_frame();
    }

    // ---- for visual checks ---------------------------------------------------

    /// `--demo-decimate`: Decimate worked through the routines its buttons and
    /// its question run, on the scene the other options left (the scripted
    /// strokes of `--demo`, say, which are made with Mirror X on). Prints the
    /// status line each step leaves and a line per check (ok / FAIL); the
    /// picture is the model at the end, decimated, in wireframe.
    ///
    /// With a mirror on, nothing is asked and the result is exactly
    /// symmetric; with a radial symmetry on, the button asks and starts
    /// nothing, and the answer Decimate Anyway starts the job, which shows
    /// in the list of running jobs; it lands as one undo step with the count
    /// asked for and the surface still closed; Undo and Redo step across it.
    /// A second one is cancelled and leaves the model alone. A third
    /// finishes after the model was edited under it, and is not used. A
    /// target the model is already under is said to be so. Then the preview:
    /// it opens, follows the slider, leaves the model untouched, and Apply
    /// lands what it shows as one undo step, the very mesh Decimate makes;
    /// Esc and an edit close it.
    pub(super) fn decimate_demo(&mut self) {
        use crate::doc::MaskOp;
        let mut checks: Vec<(&str, bool)> = Vec::new();
        let asking = |app: &Running| matches!(app.ui.graph.question(), Some(Question::DecimateSymmetric { .. }));
        let triangles = |app: &Running| app.doc.mesh.triangle_count();
        let closed = |app: &Running| crate::printable::census(&app.doc.mesh).is_closed();
        let good = |app: &Running| app.ui.status.1 == crate::ui::theme::OK;

        self.doc.edit_symmetry(|symmetry| {
            symmetry.mirror = [false; 3];
            symmetry.radial.on = true;
        });
        let (whole, target) = (triangles(self), (triangles(self) / 4).max(1_000));
        self.doc.opts.decimate_target = target as f32;
        // `SCULPT_DEMO_DECIMATE=ask`: only the question, left up for the picture.
        if std::env::var("SCULPT_DEMO_DECIMATE").is_ok_and(|what| what == "ask") {
            return self.act(Action::Decimate);
        }
        let steps = self.doc.history.steps().0;
        let was_closed = closed(self);

        self.act(Action::Decimate);
        checks.push(("with radial symmetry on, the button asks first and starts nothing", asking(self) && self.decimate.pending.is_none()));
        let answer = self.ui.graph.answer(true);
        checks.push(("Enter is Decimate Anyway", answer == Some(Action::DecimateAnyway)));
        self.act(Action::DecimateAnyway);
        let listed = self.doc.tools.jobs.running().iter().any(|job| job.label == "Decimate");
        checks.push(("the work is a background job, in the list with a Cancel", self.decimate.pending.is_some() && listed));
        checks.push(("the model is untouched while it works", triangles(self) == whole));
        self.decimate_wait();
        println!("{}", self.ui.status.0);
        let landed = triangles(self) <= target && triangles(self) + 2 > target;
        checks.push(("it lands with the count asked for", landed));
        checks.push(("as one undo step", self.doc.history.steps().0 == steps + 1));
        checks.push(("a closed model is still closed, and the line says so", was_closed && closed(self) && good(self)));
        checks.push(("the line says it was closed and still is", self.ui.status.0.contains("It was a closed surface and still is.")));
        self.act(Action::Undo);
        checks.push(("one Undo brings every triangle back", triangles(self) == whole && self.doc.history.steps().0 == steps));
        self.act(Action::Redo);
        checks.push(("and Redo takes them off again", triangles(self) <= target));
        self.act(Action::Undo);

        // Cancelled: the model is as it was, and nothing is recorded.
        self.act(Action::DecimateAnyway);
        self.doc.tools.jobs.cancel(0);
        self.decimate_wait();
        println!("{}", self.ui.status.0);
        let untouched = triangles(self) == whole && self.doc.history.steps().0 == steps;
        checks.push(("a cancelled one leaves the model alone", untouched && self.ui.status.0.contains("cancelled")));

        // Edited while it worked: the result is of a model that is gone.
        self.act(Action::DecimateAnyway);
        self.act(Action::Mask(MaskOp::Invert));
        self.decimate_wait();
        println!("{}", self.ui.status.0);
        let refused = triangles(self) == whole && !good(self) && self.ui.status.0.contains("was changed while Decimate was working");
        checks.push(("one that finishes after the model was edited is not used", refused && self.doc.history.steps().0 == steps + 1));
        self.act(Action::Undo);

        // A second press while one is working is turned away.
        self.act(Action::DecimateAnyway);
        self.act(Action::DecimateAnyway);
        checks.push(("a second press while one works is turned away", self.ui.status.0.starts_with("Decimate is still working")));
        self.doc.tools.jobs.cancel(0);
        self.decimate_wait();

        // A mirror: nothing to ask, and the sides come out mirror images.
        self.doc.edit_symmetry(|symmetry| {
            symmetry.radial.on = false;
            symmetry.mirror = [true, false, false];
        });
        self.act(Action::Decimate);
        checks.push(("with Mirror X on it starts without asking", !asking(self) && self.decimate.pending.is_some()));
        self.decimate_wait();
        println!("{}", self.ui.status.0);
        checks.push(("and lands closed", triangles(self) <= target && closed(self)));
        checks.push(("its sides mirror each other, and the line says so", self.ui.status.0.contains("Its sides mirror each other across X")));
        let mesh = &self.doc.mesh;
        let key = |p: glam::Vec3| (p + glam::Vec3::ZERO).to_array().map(f32::to_bits);
        let points: std::collections::HashSet<[u32; 3]> = mesh.positions.iter().map(|&p| key(p)).collect();
        let paired = mesh.positions.iter().filter(|p| points.contains(&key(glam::Vec3::new(-p.x, p.y, p.z)))).count();
        println!("{paired} of {} points have their mirror image", mesh.positions.len());
        checks.push(("nearly every point has its mirror image to the bit", paired * 100 >= mesh.positions.len() * 95));
        let mirrored = triangles(self);
        let steps = self.doc.history.steps().0;
        self.act(Action::Decimate);
        println!("{}", self.ui.status.0);
        let nothing = self.decimate.pending.is_none() && self.doc.history.steps().0 == steps;
        checks.push(("a model at or under the target is told so, and left", nothing && self.ui.status.0.contains("triangles already")));
        self.act(Action::Undo);

        // The preview: opened, it works out in the background and touches nothing.
        let steps = self.doc.history.steps().0;
        self.act(Action::DecimatePreview);
        let listed = self.doc.tools.jobs.running().iter().any(|job| job.label == "Decimate Preview");
        checks.push(("Preview works it out in the background, in the list with a Cancel", self.decimate.previewing() && listed));
        self.decimate_preview_wait();
        println!("{}", self.ui.status.0);
        let shown = |app: &mut Running| app.decimate.shown_mut().map(|mesh| mesh.triangle_count());
        checks.push(("it shows the count the slider says", shown(self).is_some_and(|n| n <= target && n + 4 > target)));
        checks.push(("and leaves the model as it is", triangles(self) == whole && self.doc.history.steps().0 == steps));
        // The slider moved: the other count shows.
        let fewer = (target / 2).max(1_000);
        self.doc.opts.decimate_target = fewer as f32;
        self.decimate_preview_wait();
        checks.push(("moving the slider shows the other count", shown(self).is_some_and(|n| n <= fewer && n + 4 > fewer)));
        // A view command leaves it open; Apply lands what it shows, as Decimate would.
        self.act(Action::ToggleOrtho);
        self.act(Action::ToggleOrtho);
        checks.push(("a command about the view leaves it open", self.decimate.previewing()));
        let showing: Vec<u32> = self.decimate.shown_mut().map(|mesh| mesh.indices.clone()).unwrap_or_default();
        self.act(Action::DecimateApply);
        println!("{}", self.ui.status.0);
        checks.push(("Apply lands what is shown, as one undo step", self.doc.mesh.indices == showing && self.doc.history.steps().0 == steps + 1));
        checks.push(("and closes the preview", !self.decimate.previewing() && self.ui.status.0.starts_with("Decimated")));
        self.act(Action::Undo);
        self.act(Action::Decimate);
        self.decimate_wait();
        checks.push(("the very mesh Decimate makes for that count", self.doc.mesh.indices == showing));
        self.act(Action::Undo);
        checks.push(("Undo brings the model back", triangles(self) == whole));
        // Esc closes it, and so does an edit.
        self.act(Action::DecimatePreview);
        self.decimate_preview_wait();
        let esc = self.decimate_key(KeyCode::Escape);
        checks.push(("Esc closes it and leaves the model", esc && !self.decimate.previewing() && triangles(self) == whole));
        self.act(Action::DecimatePreview);
        self.decimate_preview_wait();
        self.act(Action::Mask(MaskOp::Invert));
        checks.push(("an edit closes it first, and is made", !self.decimate.previewing() && self.doc.history.steps().0 == steps + 1));
        self.act(Action::Undo);
        self.act(Action::DecimatePreview);
        self.decimate_preview_wait();
        self.act(Action::Undo);
        checks.push(("Undo only closes it", !self.decimate.previewing() && self.doc.history.steps().0 == steps));
        // For the picture: the preview open on the mirrored model.
        self.doc.opts.decimate_target = mirrored as f32;
        self.act(Action::DecimatePreview);
        self.decimate_preview_wait();

        for (what, ok) in checks {
            println!("{} {what}", if ok { "ok  " } else { "FAIL" });
        }
        self.doc.opts.wireframe = true;
        self.input.inside = false;
        self.sync_view();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sculpt::Radial;

    #[test]
    fn the_question_is_asked_of_a_local_radial_symmetry_only() {
        let off = Symmetry::default();
        assert_eq!(symmetry_question("Head", off), None);
        // A mirror is kept: nothing to ask.
        let mirror = Symmetry { mirror: [true, false, false], ..off };
        assert_eq!(symmetry_question("Head", mirror), None);
        let radial = Symmetry { radial: Radial { on: true, count: 6, ..Radial::default() }, ..off };
        assert_eq!(
            symmetry_question("Vase", radial).unwrap(),
            "\"Vase\" is sculpted with radial symmetry on (Radial 6). Decimating keeps its shape, but not its radial \
             repeats: each is thinned in its own order, so their points no longer match. Brushes still work on all of \
             them. Nothing puts the repeats back exactly afterwards. Decimate it?"
        );
        let both = Symmetry { mirror: [true, false, true], ..radial };
        let said = symmetry_question("Vase", both).unwrap();
        assert!(said.contains("Decimating keeps its shape and its mirror image exactly, but not its radial repeats"), "{said}");
        // The world's planes cut the mesh anywhere: there was no exact match to lose.
        assert_eq!(symmetry_question("Vase", Symmetry { space: Space::World, ..radial }), None);
    }

    #[test]
    fn the_target_is_held_to_what_the_app_asks_for() {
        assert_eq!(target_of(200_000.4), 200_000);
        assert_eq!(target_of(0.0), 1_000);
        assert_eq!(target_of(-5.0), 1_000);
        assert_eq!(target_of(1.0e12), 4_000_000);
        assert_eq!(target_of(f32::NAN), 1_000);
        assert_eq!(target_of(f32::INFINITY), 1_000);
    }

    #[test]
    fn only_commands_about_the_view_leave_a_preview_open() {
        assert!(only_the_view(&Action::ToggleOrtho) && only_the_view(&Action::Frame) && only_the_view(&Action::Matcap(2)));
        assert!(!only_the_view(&Action::Remesh) && !only_the_view(&Action::Save) && !only_the_view(&Action::Undo));
        assert!(!only_the_view(&Action::Mask(crate::doc::MaskOp::Invert)) && !only_the_view(&Action::SelectObject(3)));
    }
}
