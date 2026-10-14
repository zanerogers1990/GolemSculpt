//! Decimate, on the app's side: the Decimate button of the Remesh card.
//!
//! What it does to a mesh is `crate::decimate` (and under it
//! `sculpt_geom::decimate`). Here is how the running app carries it out.
//!
//! - **It asks first when the object has a Local symmetry on.** Decimating
//!   does not keep the two sides of a mirror, or the repeats of a radial
//!   symmetry, exactly alike: each is thinned in the order its own costs
//!   come. The choice was between refusing and asking. A refusal would be
//!   got past by unticking the mirror, decimating, and ticking it again,
//!   which changes nothing about what comes out; the only real decision is
//!   whether losing the exact match is acceptable, so that is what is asked
//!   (Decimate Anyway, or Cancel), with what puts a mirror right afterwards
//!   (Cut & Mirror). A World symmetry is not exact in the mesh to begin
//!   with, and is not asked about.
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
//! A scripted run has no later frame to land in before its picture is
//! taken, so `--decimate` waits for the job (`decimate_wait`).

use std::time::Instant;

use sculpt_toolkit::Job;

use super::Running;
use crate::decimate::{self, Report, Source, TARGET_RANGE};
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

/// What the app keeps for Decimate.
#[derive(Default)]
pub(super) struct State {
    pending: Option<Pending>,
}

/// What is asked before an object with a Local symmetry on is decimated;
/// `None` when there is nothing to ask.
pub(crate) fn symmetry_question(name: &str, symmetry: Symmetry) -> Option<String> {
    if symmetry.space != Space::Local || symmetry.is_off() {
        return None;
    }
    let mirrored = symmetry.mirror.contains(&true);
    let repair = match (mirrored, symmetry.radial.on) {
        (true, false) => " Afterwards, Cut & Mirror in the Symmetry card makes the two sides exact again.",
        (true, true) => {
            " Afterwards, Cut & Mirror in the Symmetry card makes the two sides of a mirror exact again; nothing \
             does that for the radial repeats."
        }
        (false, _) => " Nothing puts the radial repeats back exactly afterwards.",
    };
    Some(format!(
        "\"{name}\" is sculpted with symmetry on ({}). Decimating keeps its shape, but not its exact symmetry: each \
         side is thinned in its own order, so the points of one no longer mirror the points of the other. Brushes \
         still work on both sides.{repair} Decimate it?",
        symmetry.summary()
    ))
}

/// The count to decimate to: what the slider says, held to what the app asks
/// for and to a whole number.
pub(crate) fn target_of(setting: f32) -> usize {
    let setting = if setting.is_finite() { setting } else { *TARGET_RANGE.start() };
    setting.clamp(*TARGET_RANGE.start(), *TARGET_RANGE.end()).round() as usize
}

impl Running {
    /// The Decimate button: asks first if the object's symmetry would no
    /// longer be exact, else starts.
    pub(super) fn decimate_asking(&mut self) {
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
        let (triangles, target) = (doc.mesh.triangle_count(), target_of(doc.opts.decimate_target));
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

    /// Starts decimating the selected object to the Target Triangles of the
    /// Remesh card, in the background. (The answer Decimate Anyway comes
    /// straight here.)
    pub(super) fn decimate_start(&mut self) {
        if let Some(why) = self.decimate_refusal() {
            // Not an error when it is only that there is nothing to take off.
            let waiting = self.decimate.pending.is_some() || self.doc.tools.jobs.is_busy();
            return self.status(why, !waiting && self.doc.mesh.triangle_count() > 0);
        }
        let doc = &self.doc;
        let target = target_of(doc.opts.decimate_target);
        let source = Source::of(&doc.mesh);
        let job = doc.tools.jobs.spawn("Decimate", move |cx| {
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
        let of = Of { mesh: doc.scene.active_mesh(), uid: doc.mesh.uid, edits: doc.history.edits, started: Instant::now() };
        self.decimate.pending = Some(Pending { job, of });
        self.redraw = true;
    }

    /// Once a frame: lands a decimation that has finished.
    pub(super) fn decimate_frame(&mut self) {
        let Some(pending) = &mut self.decimate.pending else { return };
        let Some(result) = pending.job.poll() else {
            // Still working: look again shortly, whether or not anything else asks for a
            // frame. (The list of running jobs asks too, but stops the moment the job is
            // done, which can be just after this look.)
            let at = Instant::now() + POLL;
            self.repaint_at = Some(self.repaint_at.map_or(at, |t| t.min(at)));
            return;
        };
        let of = pending.of;
        self.decimate.pending = None;
        self.decimate_land(of, result);
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
        let same = doc.scene.active_mesh() == pending.mesh && doc.mesh.uid == pending.uid && doc.history.edits == pending.edits;
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

    // ---- for visual checks ---------------------------------------------------

    /// `--demo-decimate`: Decimate worked through the routines its button and
    /// its question run, on the scene the other options left (the scripted
    /// strokes of `--demo`, say, which are made with Mirror X on). Prints the
    /// status line each step leaves and a line per check (ok / FAIL); the
    /// picture is the model at the end, decimated, in wireframe.
    ///
    /// With a mirror on, the button asks and starts nothing; the answer
    /// Decimate Anyway starts the job, which shows in the list of running
    /// jobs; it lands as one undo step with the count asked for and the
    /// surface still closed; Undo and Redo step across it. A second one is
    /// cancelled and leaves the model alone. A third finishes after the
    /// model was edited under it, and is not used. With the mirror off
    /// nothing is asked, and a target the model is already under is said to
    /// be so.
    pub(super) fn decimate_demo(&mut self) {
        use crate::doc::{Action, MaskOp};
        let mut checks: Vec<(&str, bool)> = Vec::new();
        let asking = |app: &Running| matches!(app.ui.graph.question(), Some(Question::DecimateSymmetric { .. }));
        let triangles = |app: &Running| app.doc.mesh.triangle_count();
        let closed = |app: &Running| crate::printable::census(&app.doc.mesh).is_closed();
        let good = |app: &Running| app.ui.status.1 == crate::ui::theme::OK;

        self.doc.edit_symmetry(|symmetry| symmetry.mirror = [true, false, false]);
        let (whole, target) = (triangles(self), (triangles(self) / 4).max(1_000));
        self.doc.opts.decimate_target = target as f32;
        // `SCULPT_DEMO_DECIMATE=ask`: only the question, left up for the picture.
        if std::env::var("SCULPT_DEMO_DECIMATE").is_ok_and(|what| what == "ask") {
            return self.act(Action::Decimate);
        }
        let steps = self.doc.history.steps().0;
        let was_closed = closed(self);

        self.act(Action::Decimate);
        checks.push(("with Mirror X on, the button asks first and starts nothing", asking(self) && self.decimate.pending.is_none()));
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

        // No symmetry: nothing to ask. And nothing to do at or under the target.
        self.doc.edit_symmetry(|symmetry| symmetry.mirror = [false; 3]);
        self.act(Action::Decimate);
        checks.push(("with no symmetry on it starts without asking", !asking(self) && self.decimate.pending.is_some()));
        self.decimate_wait();
        println!("{}", self.ui.status.0);
        checks.push(("and lands", triangles(self) <= target && closed(self)));
        let steps = self.doc.history.steps().0;
        self.act(Action::Decimate);
        println!("{}", self.ui.status.0);
        let nothing = self.decimate.pending.is_none() && self.doc.history.steps().0 == steps;
        checks.push(("a model at or under the target is told so, and left", nothing && self.ui.status.0.contains("triangles already")));

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
    fn the_question_is_asked_of_a_local_symmetry_and_names_what_repairs_it() {
        let off = Symmetry::default();
        assert_eq!(symmetry_question("Head", off), None);
        let mirror = Symmetry { mirror: [true, false, false], ..off };
        assert_eq!(
            symmetry_question("Head", mirror).unwrap(),
            "\"Head\" is sculpted with symmetry on (X). Decimating keeps its shape, but not its exact symmetry: each \
             side is thinned in its own order, so the points of one no longer mirror the points of the other. \
             Brushes still work on both sides. Afterwards, Cut & Mirror in the Symmetry card makes the two sides \
             exact again. Decimate it?"
        );
        let radial = Symmetry { radial: Radial { on: true, count: 6, ..Radial::default() }, ..off };
        let said = symmetry_question("Vase", radial).unwrap();
        assert!(said.contains("(Radial 6)") && said.contains(" Nothing puts the radial repeats back exactly afterwards. Decimate it?"), "{said}");
        let both = Symmetry { mirror: [true, false, true], ..radial };
        let said = symmetry_question("Vase", both).unwrap();
        assert!(said.contains("exact again; nothing does that for the radial repeats. Decimate it?"), "{said}");
        // The world's planes cut the mesh anywhere: there was no exact match to lose.
        assert_eq!(symmetry_question("Head", Symmetry { space: Space::World, ..mirror }), None);
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
}
