//! Keeping work through a crash, on the app's side: the recovery copy
//! written every few minutes, the question at start-up about a copy an
//! earlier run left behind, and what happens when the app has to stop on a
//! fault. Where the copies live, and the rules they are kept by, are in
//! `crate::recovery`.
//!
//! A copy is written on the interface thread, between strokes, through the
//! same code that saves a scene. That is a pause: on the machine this was
//! written on, 18 ms for a million triangles, 55 to 100 ms for four million
//! and 160 ms for twelve million, most of it the wait for the disk
//! (`timing_of_a_recovery_copy` in `crate::recovery` measures it). It is
//! taken once the hand has been off the model for a second, and a toast says
//! what it was when it was long enough to feel. Writing on another thread
//! would need a second copy of every mesh, or the scene shared between
//! threads; a tenth of a second every three minutes does not earn that.
//!
//! Stopping on a fault. A panic on the interface thread unwinds to the event
//! loop (`App::guarded` in app.rs), and only then, with nothing left running
//! that could be changing the scene, is the scene looked at. If every
//! finished step is already in a copy, nothing more is written. Otherwise
//! the scene is checked (`recovery::sound`) and written beside the regular
//! copies, marked as made on a fault: whatever was being done when the fault
//! struck may be half done in it, which is why the question at start-up says
//! so and offers the last regular copy as well. A fault the graphics card
//! reports is noted by its callback and acted on between events, when the
//! scene is whole.
//!
//! What this cannot catch: the program being killed, the power going, or
//! the system running out of memory, none of which gives the program a last
//! word. Then the newest regular copy, at most a few minutes old, is what
//! there is.
//!
//! What a copy holds is what a save holds, since it is written by the same
//! code. A change that is saved without being an undo step (a symmetry, the
//! hide keys, an array's settings) moves the scene's mark like any other, so
//! it is owed a copy and gets into one. What is in no file is in no copy
//! either: SSpheres, unmerged tubes, rulers, planned keys and a lathe outline
//! do not survive a crash (app/unsaved.rs lists them for the questions).
//!
//! When a copy is removed is the rule at the top of `crate::recovery`, and
//! [`State`] is where it is kept: [`State::saved`] and [`State::closed`] are
//! the only places a copy goes, apart from a run dropping its own oldest as
//! it writes new ones. Restoring a copy removes nothing. The restored scene
//! carries a note of the run it came from (`history::Restored`), which Undo
//! and Redo move with the scene as they move its file (`file_undo.rs`), and
//! only a save of that very scene retires that run's copies. Of several runs
//! that left copies, one is asked about at a start, the one that started
//! last; the answer restores from and removes that run's copies and no
//! other's, and the rest are asked about at later starts. `recovery/rule.rs`
//! walks all of this through runs that end every way a run can.

use std::ffi::OsString;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use glam::Vec3;
use sculpt_toolkit::{Job, JobCtx};

use super::files::Mark;
use super::{Drag, Running};
use crate::doc::{Action, Recover};
use crate::history::{FileState, Restored};
use crate::io;
use crate::prefs::{self, Home};
use crate::recovery::{self, About, Faults, Kept, Left, Outcome, Schedule, Store};
use crate::sculpt::PointerSample;
use crate::ui::question::Question;

/// The variable that makes a scripted run a check of recovery itself (see
/// [`Script`]).
const CHECK: &str = "SCULPT_TEST_RECOVERY";

/// The variable that gives a run a settings folder of its own (`prefs.rs`).
/// Without it the check variable is not heeded at all.
const OWN_SETTINGS: &str = "SCULPT_SETTINGS_DIR";

/// In the script of a check: a fault strikes while the copy is being
/// restored, when its objects have taken the scene's place and nothing else
/// has been done.
const RESTORE_FAULT: &str = "restore-fault";

/// In the script of a check: put the closing message up even though the run
/// is a scripted one.
const SHOW_MESSAGE: &str = "message";

/// In the script of a check: the file in the settings folder that `open` opens.
const TO_OPEN: &str = "to open.sclp";

/// A copy that took longer than this to write says so in a toast: the pause
/// could be felt, and should not be a mystery.
const FELT: Duration = Duration::from_millis(200);

/// How often, at most, the scene is looked at to see whether it is owed a
/// copy. The look reads every object and asks every tool what it would save
/// (18 microseconds for 200 objects, 400 with twenty arrays of 64 points
/// among them: `timing_of_the_kept_fingerprint`), and the loop comes by
/// between every batch of events, hundreds of times a second under a pen. A
/// copy is due in minutes, so a fifth of a second late costs nothing.
const LOOK_EVERY: Duration = Duration::from_millis(200);

/// Whether this run is a scripted check of recovery. Such a run opens
/// without taking the keyboard and leaves the Claude bridge's port alone,
/// like a screenshot run.
pub(super) fn checking() -> bool {
    check_spec().is_some()
}

/// What the check variable asks of this run, if anything.
fn check_spec() -> Option<String> {
    heeded(std::env::var(CHECK).ok(), std::env::var_os(OWN_SETTINGS))
}

/// The script of a check, if it is to be heeded: only in a run that was
/// given a settings folder of its own. A check answers the question about
/// the copies by itself, Discard It among the answers, and a run with
/// neither that folder nor any `--option` works in the user's own folder,
/// on the copies of their real work.
fn heeded(spec: Option<String>, own_settings: Option<OsString>) -> Option<String> {
    let own = own_settings.is_some_and(|dir| !dir.is_empty());
    spec.filter(|spec| !spec.is_empty() && own)
}

pub(super) struct State {
    store: Store,
    schedule: Schedule,
    /// The scene's mark at which a copy on disk holds every step up to it:
    /// where this run's newest copy was written, or where a copy of an
    /// earlier run's was restored.
    held: Option<Mark>,
    /// Whether the scene held unsaved work that is in no copy, when that was
    /// last looked at (`copy_owed`), and when that was.
    owed: bool,
    looked: Option<Instant>,
    /// The run whose copies the question at start-up was about: of the runs
    /// that left any, the one that started last. This run restores from no
    /// other run's copies, and an answer removes no other run's.
    offered: Option<Left>,
    /// The user answered Discard It about that run's copies. They go when
    /// this run saves or closes properly; until then they are still there
    /// to be opened by hand, if the answer was a slip.
    discarded: bool,
    /// The copy the scene on screen was restored from, until that scene is
    /// saved; `None` for any other scene. It follows the scene through Undo
    /// and Redo (`file_undo.rs`), and only a save of that scene retires the
    /// copies it names.
    pub(super) restored: Option<Restored>,
    /// Where the graphics device leaves word of a fault.
    pub(super) faults: Faults,
    script: Script,
}

/// What the question at start-up has to say.
#[derive(Debug, PartialEq)]
pub(super) struct Offer {
    /// The copy Restore It brings back: the newest of the run asked about.
    newest: Kept,
    /// The last regular copy before it, where it was made on a fault.
    earlier: Option<Kept>,
    /// How many other runs left copies. They are asked about at later starts.
    others: usize,
}

/// What came of putting one copy in place of the scene.
pub(super) enum Tried {
    /// It is the scene now, with this many objects.
    Restored(usize),
    /// It could not be read. Nothing has been touched.
    Unreadable(String),
    /// A fault struck after the scene had begun to change.
    Broke,
}

/// What came of an answer to restore.
#[derive(Debug, PartialEq)]
pub(super) enum Restoring<'a> {
    /// This copy is the scene now; `passed_over` newer ones could not be read.
    Done { kept: &'a Kept, objects: usize, passed_over: usize },
    /// None could be read. The scene is as it was.
    NoneRead,
    /// A fault part of the way through one: what is on screen is neither the
    /// scene from before nor the copy.
    Broke,
}

/// The status line after a copy was restored. `when` is when it was kept,
/// `passed_over` how many newer copies could not be read, and `damage` what
/// an Open would say of damage found in the file as it was read (points
/// mended, triangles left out: `Running::opened_damage`), which a restored
/// copy is owed as much as an opened file.
fn restored_words(when: Option<&str>, objects: usize, passed_over: usize, damage: Option<&str>) -> String {
    let when = when.map_or(String::new(), |when| format!(" kept {when}"));
    let note = if passed_over > 0 { " A newer copy could not be read." } else { "" };
    let objects = if objects == 1 { "1 object".to_owned() } else { format!("{objects} objects") };
    let damage = damage.map_or(String::new(), |damage| format!(" {damage}"));
    format!(
        "Restored the copy{when} ({objects}).{note} It is not saved yet: \
         Save (Ctrl + S) asks where to keep it, and the copy stays until it is saved.{damage}"
    )
}

/// Puts the first of `copies` that can be read in place of the scene, by
/// `put`. A copy that cannot be read is passed over for the one before it.
/// After a fault part of the way through one, no other is tried: the scene
/// is in no state to have another laid over it, and what is said about it
/// has to be about what happened.
pub(super) fn restore_first<'a>(copies: &'a [Kept], mut put: impl FnMut(&'a Kept) -> Tried) -> Restoring<'a> {
    for (passed_over, kept) in copies.iter().enumerate() {
        match put(kept) {
            Tried::Restored(objects) => return Restoring::Done { kept, objects, passed_over },
            Tried::Unreadable(e) => log::error!("{e}"),
            Tried::Broke => return Restoring::Broke,
        }
    }
    Restoring::NoneRead
}

impl State {
    pub(super) fn new(screenshot: bool) -> State {
        let script = Script::from_env();
        let home = Home::for_run(screenshot);
        // A scripted run keeps no copies and offers none, even with a
        // settings folder of its own (visual checks share those, and a
        // question over the picture would spoil every one of them), unless
        // it is a check of this very thing.
        let store =
            if prefs::scripted() && !script.on { Store::nowhere() } else { Store::at(home.folder(), home.writable()) };
        State::with(store, script)
    }

    fn with(store: Store, script: Script) -> State {
        State {
            store,
            schedule: Schedule::new(script.every.unwrap_or(recovery::EVERY)),
            held: None,
            owed: false,
            looked: None,
            offered: None,
            discarded: false,
            restored: None,
            faults: Faults::default(),
            script,
        }
    }

    /// Whether any copy of this scene's work is on disk.
    fn has_copy(&self) -> bool {
        self.held.is_some() || self.restored.is_some()
    }

    /// Start-up: looks for copies that runs before this one left behind, and
    /// picks the run to ask about: the one that started last.
    fn look(&mut self) -> Option<Offer> {
        let mut left = self.store.left_behind();
        let others = left.len().saturating_sub(1);
        self.offered = (!left.is_empty()).then(|| left.remove(0));
        let run = self.offered.as_ref()?;
        Some(Offer { newest: run.newest()?.clone(), earlier: run.earlier().cloned(), others })
    }

    /// The answer Discard It: the copies asked about are to go, once this
    /// run has saved or closed properly. Gives back the folder they are in.
    fn discard(&mut self) -> Option<&Path> {
        self.discarded = self.offered.is_some();
        self.offered.as_ref().map(|run| run.folder.as_path())
    }

    /// The copies to try for Restore It, or (`earlier`) for Restore the
    /// Earlier Copy Instead, in order: the offered run's and no other's.
    fn to_restore(&self, earlier: bool) -> Vec<Kept> {
        self.offered.as_ref().map_or(Vec::new(), |run| run.to_try(earlier).to_vec())
    }

    /// The note a scene restored from `kept` carries with it.
    fn note(kept: &Kept) -> Restored {
        let run = kept.file.parent().unwrap_or(Path::new("")).to_owned();
        Restored { run, document: kept.about.document.clone() }
    }

    /// A copy has been put in place as the scene, which now stands at
    /// `mark`. That copy holds the scene as it is, so none is owed until the
    /// scene changes. Nothing is removed: the copy is still the only place
    /// the work is.
    fn restored_at(&mut self, mark: Mark) {
        self.held = Some(mark);
    }

    /// This run has written a copy of the scene as it stands at `mark`.
    /// Nothing of an earlier run's goes for that: its copies hold work the
    /// user may yet want as it was (the earlier copy beside one made on a
    /// fault, most of all), and they have not said otherwise.
    fn copied(&mut self, mark: Mark) {
        self.held = Some(mark);
    }

    /// The scene has been saved to a file of the user's; `restored` is the
    /// copy that scene had been restored from, if it had. Rule (a): this
    /// run's own copies go, and that earlier run's. Rule (b): so do the
    /// copies the user said to discard. No others.
    fn saved(&mut self, restored: Option<Restored>) {
        self.store.clear();
        self.held = None;
        self.schedule.restart();
        if let Some(from) = restored {
            self.store.retire(&from.run);
        }
        self.retire_discarded();
    }

    fn retire_discarded(&mut self) {
        if !std::mem::take(&mut self.discarded) {
            return;
        }
        if let Some(run) = self.offered.take() {
            self.store.retire(&run.folder);
        }
    }

    /// The app is closing. `settled` is whether it is closing properly: the
    /// question about closing was answered, or there was nothing unsaved to
    /// ask about. Then (rule (c)) this run's own copies go, and (rule (b))
    /// those the user said to discard. Otherwise everything stays, and the
    /// next start asks.
    fn closed(&mut self, settled: bool) {
        if !settled {
            return;
        }
        self.store.clear();
        self.retire_discarded();
    }
}

impl Running {
    // ---- start-up: a copy an earlier run left behind

    /// Start-up: if a run before this one ended without closing properly and
    /// left copies of its work, asks whether to bring the newest back. Of
    /// several such runs, this start asks about the one that started last.
    pub(super) fn recovery_start(&mut self) {
        let Some(offer) = self.recovery.look() else { return };
        let today = io::time_stamp();
        let when = |kept: &Kept| recovery::when_words(&kept.about.when, &today);
        let earlier = offer.earlier.as_ref().map(|kept| when(kept).unwrap_or_else(|| "before that".to_owned()));
        let question = Question::Recover {
            when: when(&offer.newest),
            what: offer.newest.about.words(),
            stopped: offer.newest.stopped,
            earlier,
            others: offer.others,
        };
        self.ui.graph.ask(question);
        self.redraw = true;
        if let Some(answer) = self.recovery.script.answer.take() {
            self.ui.graph.answer(false);
            if let Some(answer) = answer {
                self.recover(answer);
            }
        }
    }

    /// The answer to that question. (Not Now does not come here: it leaves
    /// the copies as they are, to be asked about next time.) No answer
    /// removes a copy by itself; see the rule in `crate::recovery`.
    pub(super) fn recover(&mut self, answer: Recover) {
        if answer == Recover::Discard {
            let folder = self.recovery.discard().map_or(String::new(), |dir| dir.display().to_string());
            let text = format!("Not restored. The copy stays in {folder} until you save the scene or close the app.");
            return self.status(text, true);
        }
        let copies = self.recovery.to_restore(answer == Recover::Earlier);
        match restore_first(&copies, |kept| self.restore(kept)) {
            Restoring::Done { kept, objects, passed_over } => {
                let when = recovery::when_words(&kept.about.when, &io::time_stamp());
                let text = restored_words(when.as_deref(), objects, passed_over, self.opened_damage().as_deref());
                self.status(text, passed_over == 0);
            }
            // Nothing was restored, so nothing is settled: the copies stay,
            // and the next start asks again.
            Restoring::NoneRead => {
                self.status("The recovery copy could not be read, and stays where it is. The scene is as it was.", false);
            }
            Restoring::Broke => {
                let text = "A fault stopped the recovery copy part of the way in, so the scene on screen may be \
                            incomplete. It has no file now: Save (Ctrl + S) asks where to keep it, and writes over \
                            nothing of yours. The copy itself is untouched, and the next start offers it again.";
                recovery::log(text);
                self.status(text, false);
            }
        }
    }

    /// Puts a recovery copy in place of the scene, as one undo step. The
    /// scene has no file after it and counts as unsaved, so Save asks where
    /// it should go and closing asks first: the copy is never written over
    /// the user's own file without their say. Undo of it gives the scene
    /// from before its own file back, as Undo of an Open does.
    ///
    /// A copy that brings a fault of its own with it must not take the app
    /// down: the next start would offer it again and fall the same way. So
    /// a panic is caught here, in two steps that are told apart. Reading
    /// the copy touches nothing, and a fault there is a copy that cannot be
    /// read. A fault after that has left the scene part the old one and
    /// part the copy, still under the old scene's file name, and that is not
    /// passed off as either (`restore_broke`).
    fn restore(&mut self, kept: &Kept) -> Tried {
        let saved = match catch_unwind(AssertUnwindSafe(|| io::load_scene(&kept.file))) {
            Ok(Ok(saved)) => saved,
            Ok(Err(e)) => return Tried::Unreadable(e),
            Err(_) => return Tried::Unreadable(format!("{} could not be opened.", kept.file.display())),
        };
        let file = FileState::restored(State::note(kept));
        let placed = catch_unwind(AssertUnwindSafe(|| self.put_in_place(saved, file)));
        match placed {
            Ok((objects, _)) => {
                let mark = self.history_mark();
                self.recovery.restored_at(mark);
                Tried::Restored(objects)
            }
            Err(_) => {
                self.restore_broke();
                Tried::Broke
            }
        }
    }

    /// A fault struck while a copy was taking the scene's place. Whatever is
    /// on screen now is no file's scene: it gets no file and counts as
    /// unsaved, so that Save cannot write it over the file the scene before
    /// it came from, and closing asks first. It is nobody's restored copy
    /// either, so saving it retires nothing.
    fn restore_broke(&mut self) {
        self.file().broke();
        self.recovery.held = None;
        self.doc.mesh_replaced = true;
        // What the frame draws from, brought up to whatever the scene now
        // holds. If that faults as well there is nothing more to be done here.
        let _ = catch_unwind(AssertUnwindSafe(|| {
            self.scene_changed();
            self.sync_view();
        }));
        self.redraw = true;
    }

    /// For a scripted check (`restore-fault`): a fault at the worst moment of
    /// a restore, with the copy's objects in place of the scene's and the
    /// scene's file still the one from before.
    pub(super) fn restore_fault_point(&self) {
        if self.recovery.script.restore_fault {
            panic!("a fault made on purpose while a copy was restored ({CHECK})");
        }
    }

    /// The file a restored scene was a copy of, for the Save dialog to start
    /// from. Only while that scene is the one on screen.
    pub(super) fn recovered_document(&self) -> Option<PathBuf> {
        self.recovery.restored.as_ref().and_then(|from| from.document.clone())
    }

    // ---- while the app runs: a copy every few minutes

    /// Between events: writes a copy if one is due. Gives back when to look
    /// again if nothing happens before then, for the event loop to wake at.
    /// This is asked from the loop, not from the frame: a window that is
    /// minimised draws no frames, and its work is owed a copy all the same.
    pub(super) fn recovery_tick(&mut self) -> Option<Instant> {
        let now = Instant::now();
        let busy = self.doc.sculptor.is_active() || self.input.drag != Drag::None;
        // While a stroke or a drag is under way no copy is written whatever
        // the scene holds, and a look taken a moment ago still stands
        // (`LOOK_EVERY`): the last answer serves until then.
        let lately = self.recovery.looked.filter(|&at| now.saturating_duration_since(at) < LOOK_EVERY);
        if !busy && lately.is_none() {
            let mark = self.history_mark();
            let (keeps, unsaved) = (self.recovery.store.keeps(), self.unsaved_at(mark));
            self.recovery.owed = copy_owed(keeps, unsaved, self.recovery.held, mark);
            self.recovery.looked = Some(now);
        }
        let changed = self.recovery.owed;
        if self.recovery.schedule.due(now, changed, busy) {
            self.recovery.schedule.restart();
            if let Err(e) = self.keep_copy(false) {
                log::warn!("no recovery copy was written: {e}");
                // Said once here; it is tried again when the next copy is due.
                self.status(format!("No recovery copy could be kept. {e}"), false);
            }
            self.redraw = true;
        }
        // A look that was put off is still owed if nothing else happens: the
        // change it would have seen may have come with the very last event.
        let look = lately.map(|at| at + LOOK_EVERY);
        match (self.recovery.schedule.wake(), look) {
            (Some(copy), Some(look)) => Some(copy.min(look)),
            (copy, look) => copy.or(look),
        }
    }

    /// Another scene is about to take this one's place (an Open, a New
    /// Scene, a restored copy). If this one holds unsaved work that is in no
    /// copy yet, the copy it is owed is written now and not when three
    /// minutes are up: once the scene is put aside its work is one Undo away
    /// in memory and nowhere else, and the timed copy looks only at the
    /// scene on screen, so nothing would write one until Undo came back to
    /// it. Without this, "Open Without Saving" or "New Scene Without Saving"
    /// over two minutes of strokes, then a crash, lost them although Undo
    /// would have brought them back.
    ///
    /// It is a timed copy taken early, by the same routine, and nothing
    /// more: this run's own, kept and removed by the same rule (the top of
    /// `crate::recovery`), and not written at all where copies are not being
    /// kept (a scripted run that is not a check of recovery) or where one
    /// already holds the scene as it stands.
    pub(super) fn recovery_before_replacing(&mut self) {
        let mark = self.history_mark();
        if !copy_owed(self.recovery.store.keeps(), self.unsaved_at(mark), self.recovery.held, mark) {
            return;
        }
        self.recovery.schedule.restart();
        if let Err(e) = self.keep_copy(false) {
            // The scene is replaced all the same: the user has asked for
            // that, and has been asked about the work. It is in the log.
            log::warn!("no recovery copy was written of the scene that is being put aside: {e}");
        }
    }

    /// Writes the scene to a new recovery copy, through the code that saves
    /// a scene (whole or not at all, never over another file). `stopped`
    /// marks the copy made as the app stops on a fault: the scene is checked
    /// first, and no older copy is dropped for it.
    fn keep_copy(&mut self, stopped: bool) -> io::Result<()> {
        let started = Instant::now();
        let mark = self.history_mark();
        let file = self.recovery.store.next_file(stopped)?;
        let about = {
            let (meshes, objects, active) = self.scene_for_file()?;
            if stopped {
                recovery::sound(&meshes).map_err(|e| format!("The scene was not whole ({e})."))?;
            }
            io::save_scene_with(&meshes, &objects, active, &self.details(), &file)?;
            About {
                when: io::time_stamp(),
                document: self.doc.path.clone(),
                objects: objects.len(),
                triangles: meshes.iter().map(|mesh| mesh.triangle_count()).sum(),
            }
        };
        self.recovery.store.wrote(&file, &about);
        if !stopped {
            self.recovery.copied(mark);
        }
        let took = started.elapsed();
        if self.recovery.script.on {
            let name = file.file_name().unwrap_or_default().to_string_lossy();
            println!("copy {name} {} triangles {:.0} ms", about.triangles, took.as_secs_f64() * 1000.0);
        }
        if took > FELT && !stopped {
            self.toast("Recovery copy kept");
        }
        Ok(())
    }

    /// The scene has been saved to a file of the user's: its copies are
    /// removed, so a crash after this offers nothing older than that file.
    /// `restored` is the copy the saved scene had been restored from, if it
    /// had (`File::written`).
    pub(super) fn recovery_saved(&mut self, restored: Option<Restored>) {
        self.recovery.saved(restored);
    }

    /// The app is closing. If it is closing properly (the user answered the
    /// question about closing, or there was nothing unsaved to ask about),
    /// this run's copies go, and those the user said to discard. Closed any
    /// other way over unsaved work, they stay, and the next start offers
    /// them.
    pub(super) fn recovery_exit(&mut self) {
        let settled = self.close_settled();
        self.recovery.closed(settled);
    }

    // ---- stopping on a fault

    /// The app is about to stop: makes sure of a copy of the unsaved work,
    /// and says what became of it. `whole` is whether the scene can be
    /// trusted as it stands (a fault of the graphics card's, found between
    /// events) or may have been caught mid-change (a panic).
    fn last_copy(&mut self, whole: bool) -> Outcome {
        if !self.unsaved() {
            return Outcome::NothingUnsaved;
        }
        if !self.recovery.store.keeps() {
            return Outcome::Lost;
        }
        // Every finished step is in a copy already. What was under way when
        // the fault struck is not worth risking a damaged copy for.
        if self.recovery.held == Some(self.history_mark()) {
            return Outcome::Kept;
        }
        match self.keep_copy(!whole) {
            Ok(()) => Outcome::Kept,
            Err(e) => {
                recovery::log(&format!("The scene could not be kept as the app stopped: {e}"));
                if self.recovery.has_copy() { Outcome::EarlierOnly } else { Outcome::Lost }
            }
        }
    }

    /// Between events: if the graphics card has reported a fault, keeps a
    /// copy of the scene, which is whole at this point, and closes the app.
    pub(super) fn stop_on_graphics_fault(&mut self) {
        let Some(fault) = self.recovery.faults.take() else { return };
        let outcome = self.last_copy(true);
        close(&format!("{fault}, and GolemSculpt has to close."), outcome)
    }

    // ---- scripted checks

    /// Whether the script still has steps to run: it needs frames without input.
    pub(super) fn recovery_script_running(&self) -> bool {
        self.recovery.script.running()
    }

    /// Once a frame: runs the steps a scripted check has for it.
    pub(super) fn recovery_frame(&mut self) {
        if !self.recovery.script.on {
            return;
        }
        self.recovery.script.frame += 1;
        let frame = self.recovery.script.frame;
        if let Some(result) = self.recovery.script.job.as_mut().and_then(|job| job.poll()) {
            println!("job came back: {result:?}");
            self.recovery.script.job = None;
        }
        let steps = &self.recovery.script.steps;
        let now: Vec<Step> = steps.iter().filter(|(at, _)| *at == frame).map(|&(_, step)| step).collect();
        for step in now {
            self.recovery_check_step(step);
        }
    }

    fn recovery_check_step(&mut self, step: Step) {
        // Where the scripted strokes go: a row lower each time.
        let row = 0.25 - 0.12 * self.recovery.script.strokes as f32;
        match step {
            Step::Stroke => {
                self.recovery.script.strokes += 1;
                self.symmetry_demo_stroke(Vec3::new(-0.15, row, 0.0), Vec3::new(0.2, row, 0.0));
                println!("stroke: {} steps to undo, unsaved {}", self.doc.history.steps().0, self.unsaved());
            }
            Step::Scene => match self.scene_for_file() {
                Ok((meshes, objects, _)) => {
                    let triangles: usize = meshes.iter().map(|mesh| mesh.triangle_count()).sum();
                    let print = recovery::fingerprint(&meshes, &objects);
                    let (objects, unsaved) = (objects.len(), self.unsaved());
                    println!("scene {print:016x} {objects} objects {triangles} triangles unsaved {unsaved}");
                }
                Err(e) => println!("scene: {e}"),
            },
            Step::Panic => panic!("a fault made on purpose ({CHECK})"),
            Step::PanicMidStroke => {
                // A stroke begun and half drawn: its change is in the mesh but in no undo step.
                self.sync_view();
                let from = self.view.project(Vec3::new(-0.15, row, 0.0)).unwrap_or(self.view.size * 0.5);
                let at = |i: usize| PointerSample {
                    pos: from + glam::Vec2::new(4.0 * i as f32, 0.0),
                    pressure: None,
                    time: i as f64 / 240.0,
                };
                let doc = &mut self.doc;
                let began = doc.begin_stroke(&self.local, at(0), false, false);
                for i in 1..=20 {
                    doc.sculptor.sample(&mut doc.mesh, &self.local, at(i));
                }
                doc.sculptor.frame(&mut doc.mesh, &self.local, 1.0 / 60.0);
                println!("mid-stroke: began {began}, stroke under way {}", self.doc.sculptor.is_active());
                panic!("a fault made on purpose in the middle of a stroke ({CHECK})");
            }
            Step::JobPanic => {
                let work = |_: &JobCtx| -> Result<(), String> { panic!("a fault made on purpose in a job") };
                self.recovery.script.job = Some(self.doc.tools.jobs.spawn("The test job", work));
            }
            Step::Save => {
                let file = Home::for_run(false).folder().map(|dir| dir.join("saved by the check.sclp"));
                let saved = match file {
                    Some(file) => self.save_scene(&file).map(|()| self.doc.path = Some(file)),
                    None => Err("no settings folder to save in".to_owned()),
                };
                println!("save: {saved:?}, unsaved {}", self.unsaved());
            }
            Step::Open => match Home::for_run(false).folder().map(|dir| dir.join(TO_OPEN)) {
                Some(file) => {
                    self.load(file, true);
                    println!("open: {}", self.ui.status.0);
                }
                None => println!("open: no settings folder to open from"),
            },
            Step::NewScene => {
                // "New Scene Without Saving", as the answer to the question runs it.
                self.act(Action::NewSceneAs { save: false });
                println!("new: {} (unsaved {})", self.ui.status.0, self.unsaved());
            }
            Step::Undo | Step::Redo => {
                self.act(if step == Step::Undo { Action::Undo } else { Action::Redo });
                let (undo, redo) = self.doc.history.steps();
                println!("{}: {undo} steps to undo, {redo} to redo", if step == Step::Undo { "undo" } else { "redo" });
            }
            Step::File => {
                let file = self.doc.path.as_ref().and_then(|path| path.file_name());
                let file = file.map_or("none".into(), |name| name.to_string_lossy());
                println!("file: {file}, unsaved {}", self.unsaved());
            }
            Step::Status => println!("status: {}", self.ui.status.0),
            Step::Kept(what) => {
                self.kept_change(what);
                println!("{what}: unsaved {}", self.unsaved());
            }
            Step::KeptPrint => println!("kept {:016x}", self.history_mark().kept()),
            Step::Close => match self.question_before_closing() {
                Some(question) => println!("close: asks {question:?}"),
                None => println!("close: closes without asking"),
            },
            Step::GraphicsError => {
                // A texture no wider than nothing: the device refuses it.
                let _ = self.gpu.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("a fault made on purpose"),
                    size: wgpu::Extent3d { width: 0, height: 0, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: self.gpu.format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                });
            }
            Step::DeviceLost => {
                // A device that is destroyed says it is lost the next time it
                // is asked how its work is going. (One the driver loses says
                // so by itself, there and then.)
                self.gpu.device.destroy();
                let _ = self.gpu.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
            }
            // As if the power had gone: no last word, no tidying.
            Step::Kill => std::process::exit(9),
            Step::Quit => self.quit(false),
        }
    }
}

/// The interface thread panicked and the panic has unwound to the event
/// loop: makes sure of a copy of the scene if there is one to make, tells
/// the user, and ends the program. `running` is `None` if the fault came
/// before the app was up.
pub(super) fn stop_after_panic(running: Option<&mut Running>) -> ! {
    let outcome = match running {
        Some(running) => {
            let without = if running.recovery.has_copy() { Outcome::EarlierOnly } else { Outcome::Lost };
            // A second fault while saving what the first left must end here too.
            catch_unwind(AssertUnwindSafe(|| running.last_copy(false))).unwrap_or(without)
        }
        None => Outcome::NothingUnsaved,
    };
    close("GolemSculpt ran into a fault and has to close.", outcome)
}

/// Tells the user why the app is closing and what became of their work,
/// then ends the program at once: after a fault, tidying up could do harm.
fn close(cause: &str, outcome: Outcome) -> ! {
    let words = recovery::farewell(cause, outcome, recovery::log_file());
    recovery::log(&words.replace("\n\n", " "));
    // A scripted run has nobody to press OK, unless the check is of the
    // message itself (`message` in the script).
    let shown = check_spec().is_some_and(|spec| spec.split(',').any(|part| part.trim() == SHOW_MESSAGE));
    if prefs::scripted() {
        println!("stopped: {outcome:?}");
        eprintln!("{words}");
    }
    if shown || !prefs::scripted() {
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title("GolemSculpt")
            .set_description(words)
            .show();
    }
    std::process::exit(101)
}

/// Whether the scene is owed a recovery copy: copies are being kept, the
/// scene has unsaved work, and it does not stand where the newest copy was
/// written (`held`). `now` is the scene's mark, which every change that a
/// save would keep moves, undo step or not.
pub(super) fn copy_owed(keeps: bool, unsaved: bool, held: Option<Mark>, now: Mark) -> bool {
    keeps && unsaved && held != Some(now)
}

// ---- the script of a scripted check

/// One thing a scripted check does in a frame.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Step {
    /// One Clay stroke, as an undo step.
    Stroke,
    /// Prints the scene's fingerprint.
    Scene,
    /// Panics on the interface thread.
    Panic,
    /// Begins a stroke, draws half of it and panics.
    PanicMidStroke,
    /// Starts a background job that panics.
    JobPanic,
    /// Saves the scene, as Save does, to a file in the settings folder.
    Save,
    /// Opens `to open.sclp` from the settings folder in place of the scene,
    /// as Open does when told not to save first.
    Open,
    /// New Scene, answered "New Scene Without Saving" if it would ask.
    NewScene,
    /// Undo, as the Z key.
    Undo,
    /// Redo.
    Redo,
    /// Prints the name of the scene's file and whether anything is unsaved.
    File,
    /// Prints the status line.
    Status,
    /// A change that a file keeps and no undo step records, by the key that
    /// makes it (`Running::kept_change`): `mirror` (X), `hide` (H), `show`
    /// (Alt + H) or `local` (/).
    Kept(&'static str),
    /// Prints a fingerprint of what the file would keep that is not an undo
    /// step (symmetry, visibility, arrays): the same in a scene restored from
    /// a copy as in the scene the copy was made of.
    KeptPrint,
    /// Prints what the close button would ask as things stand.
    Close,
    /// Asks the graphics device for something it refuses.
    GraphicsError,
    /// Destroys the graphics device, which then reports itself lost.
    DeviceLost,
    /// Ends the program on the spot.
    Kill,
    /// Closes the app properly, without saving.
    Quit,
}

/// What `SCULPT_TEST_RECOVERY` asks for, so that keeping copies, the
/// question at start-up and stopping on a fault can be checked end to end
/// without a person or a real crash. A list separated by commas:
///
/// - `every=0.3`: a copy is due after this many seconds, not three minutes;
/// - `answer=restore | earlier | discard | later`: answers the question at start-up;
/// - `message`: the message the app closes with on a fault is put up, as it
///   is for a person, and the run waits for it to be closed;
/// - `restore-fault`: a fault strikes while the copy is being restored;
/// - `stroke@30`, `scene@40`, `save@45`, `open@46`, `new@46`, `undo@47`, `redo@48`,
///   `file@49`, `status@49`, `mirror@50`, `hide@50`, `show@50`, `local@50`, `kept@50`,
///   `close@50`, `panic@50`, `stroke-panic@50`, `job-panic@50`,
///   `graphics-error@50`, `device-lost@50`, `kill@50`, `quit@50`: does that
///   in the frame of that number (see [`Step`]).
///
/// Setting it at all (`on` will do) switches recovery on for a scripted run,
/// which otherwise keeps no copies. It needs `SCULPT_SETTINGS_DIR` to say
/// where, and does nothing at all without it ([`heeded`]).
#[derive(Default)]
struct Script {
    on: bool,
    every: Option<Duration>,
    /// A fault is to strike while a copy is being restored.
    restore_fault: bool,
    /// The answer to give at start-up; `Some(None)` is Not Now.
    answer: Option<Option<Recover>>,
    steps: Vec<(u32, Step)>,
    frame: u32,
    strokes: u32,
    /// The job `job-panic` started, until it comes back.
    job: Option<Job<()>>,
}

impl Script {
    fn from_env() -> Script {
        if check_spec().is_none() && std::env::var_os(CHECK).is_some_and(|spec| !spec.is_empty()) {
            eprintln!("{CHECK} is not heeded without {OWN_SETTINGS}: it would work on your own copies.");
        }
        check_spec().map_or_else(Script::default, |spec| Script::parse(&spec))
    }

    fn parse(spec: &str) -> Script {
        let mut script = Script { on: true, ..Script::default() };
        let seconds = |text: &str| text.parse::<f32>().ok().filter(|seconds| seconds.is_finite());
        let step_at = |(name, at): (&str, &str)| Some((at.parse().ok()?, step_named(name)?));
        let words = ["on", SHOW_MESSAGE];
        for part in spec.split(',').map(str::trim).filter(|part| !part.is_empty() && !words.contains(part)) {
            if part == RESTORE_FAULT {
                script.restore_fault = true;
            } else if let Some(seconds) = part.strip_prefix("every=").and_then(seconds) {
                script.every = Some(Duration::from_secs_f32(seconds.clamp(0.01, 86_400.0)));
            } else if let Some(answer) = part.strip_prefix("answer=").and_then(answer_named) {
                script.answer = Some(answer);
            } else if let Some(step) = part.split_once('@').and_then(step_at) {
                script.steps.push(step);
            } else {
                eprintln!("{CHECK}: no such step as {part:?}");
            }
        }
        script
    }

    fn running(&self) -> bool {
        self.on && (self.job.is_some() || self.steps.iter().any(|(at, _)| *at > self.frame))
    }
}

fn answer_named(name: &str) -> Option<Option<Recover>> {
    Some(match name {
        "restore" => Some(Recover::Newest),
        "earlier" => Some(Recover::Earlier),
        "discard" => Some(Recover::Discard),
        "later" => None,
        _ => return None,
    })
}

fn step_named(name: &str) -> Option<Step> {
    Some(match name {
        "stroke" => Step::Stroke,
        "scene" => Step::Scene,
        "panic" => Step::Panic,
        "stroke-panic" => Step::PanicMidStroke,
        "job-panic" => Step::JobPanic,
        "save" => Step::Save,
        "open" => Step::Open,
        "new" => Step::NewScene,
        "undo" => Step::Undo,
        "redo" => Step::Redo,
        "file" => Step::File,
        "status" => Step::Status,
        "mirror" => Step::Kept("mirror"),
        "hide" => Step::Kept("hide"),
        "show" => Step::Kept("show"),
        "local" => Step::Kept("local"),
        "kept" => Step::KeptPrint,
        "close" => Step::Close,
        "graphics-error" => Step::GraphicsError,
        "device-lost" => Step::DeviceLost,
        "kill" => Step::Kill,
        "quit" => Step::Quit,
        _ => return None,
    })
}

#[cfg(test)]
mod rule;

#[cfg(test)]
mod tests {
    use super::*;

    /// Review point 8: the variable alone, in a run with no `--option`, put
    /// a check to work in the user's own settings folder.
    #[test]
    fn a_check_is_not_heeded_without_a_settings_folder_of_its_own() {
        let spec = || Some("answer=discard,quit@8".to_owned());
        let folder = |name: &str| Some(OsString::from(name));
        assert_eq!(heeded(spec(), None), None, "it would discard the copies of real work");
        assert_eq!(heeded(spec(), folder("")), None, "an empty name is no folder");
        assert_eq!(heeded(spec(), folder("D:/some/folder")), spec());
        assert_eq!(heeded(Some(String::new()), folder("D:/some/folder")), None, "nothing asked");
        assert_eq!(heeded(None, folder("D:/some/folder")), None);
    }

    #[test]
    fn a_script_can_ask_for_a_fault_while_a_copy_is_restored() {
        let script = Script::parse("answer=restore,restore-fault,file@5,status@6");
        assert!(script.restore_fault && script.steps == [(5, Step::File), (6, Step::Status)]);
        assert!(!Script::parse("answer=restore").restore_fault);
    }

    fn copy(name: &str) -> Kept {
        Kept { file: PathBuf::from("run").join(name), stopped: false, number: 1, about: About::default() }
    }

    /// Review point 5: a fault part of the way through a copy was taken for
    /// a copy that could not be read, and the next one was laid over it.
    #[test]
    fn after_a_fault_part_of_the_way_through_a_copy_no_other_copy_is_tried() {
        let copies = [copy("stopped-0003.sclp"), copy("copy-0002.sclp"), copy("copy-0001.sclp")];
        let mut tried = Vec::new();
        let came = restore_first(&copies, |kept| {
            tried.push(kept.file.clone());
            if tried.len() == 1 { Tried::Unreadable("damaged".into()) } else { Tried::Broke }
        });
        assert_eq!(came, Restoring::Broke);
        assert_eq!(tried, [copies[0].file.clone(), copies[1].file.clone()], "the third is not laid over the wreck");
    }

    #[test]
    fn a_copy_that_cannot_be_read_is_passed_over_for_the_one_before_it() {
        let copies = [copy("copy-0003.sclp"), copy("copy-0002.sclp"), copy("copy-0001.sclp")];
        let unread = |kept: &Kept| Tried::Unreadable(format!("{} is damaged", kept.file.display()));
        let came = restore_first(&copies, |kept| if kept == &copies[0] { unread(kept) } else { Tried::Restored(4) });
        assert_eq!(came, Restoring::Done { kept: &copies[1], objects: 4, passed_over: 1 });
        assert_eq!(restore_first(&copies, unread), Restoring::NoneRead);
        assert_eq!(restore_first(&[], unread), Restoring::NoneRead, "nothing to try");
    }

    #[test]
    fn a_script_reads_its_interval_its_answer_and_its_steps() {
        let script = Script::parse("every=0.25, answer=earlier,stroke@30,scene@40,stroke-panic@41,nonsense,quit@x");
        assert!(script.on);
        assert_eq!(script.every, Some(Duration::from_millis(250)));
        assert_eq!(script.answer, Some(Some(Recover::Earlier)));
        assert_eq!(script.steps, [(30, Step::Stroke), (40, Step::Scene), (41, Step::PanicMidStroke)]);
        assert_eq!(Script::parse("answer=later").answer, Some(None), "Not Now is an answer too");
    }

    #[test]
    fn a_script_can_open_a_file_and_step_through_undo() {
        let script = Script::parse("open@3,undo@4,file@5,redo@6");
        assert_eq!(script.steps, [(3, Step::Open), (4, Step::Undo), (5, Step::File), (6, Step::Redo)]);
    }

    #[test]
    fn a_script_can_start_a_new_scene() {
        let script = Script::parse("stroke@3,new@4,scene@5,kill@6");
        assert_eq!(script.steps, [(3, Step::Stroke), (4, Step::NewScene), (5, Step::Scene), (6, Step::Kill)]);
    }

    /// A restored copy said nothing of damage found in it, which an Open of
    /// the same file does say.
    #[test]
    fn restoring_a_copy_says_what_an_open_says_of_damage_found_in_it() {
        let plain = restored_words(Some("today at 14:35"), 3, 0, None);
        assert_eq!(
            plain,
            "Restored the copy kept today at 14:35 (3 objects). It is not saved yet: \
             Save (Ctrl + S) asks where to keep it, and the copy stays until it is saved."
        );
        let damage = io::notes_opened(2, 1).expect("two points and a triangle");
        let said = restored_words(None, 1, 1, Some(&damage));
        assert!(said.starts_with("Restored the copy (1 object). A newer copy could not be read. It is not saved yet"));
        assert!(said.contains("until it is saved. 2 damaged points in the file had no position"), "{said}");
        assert!(said.ends_with("a Remesh (R) closes them."), "and the triangles that were left out: {said}");
        assert!(!said.contains(".."), "{said}");
    }

    #[test]
    fn a_script_can_make_the_changes_that_are_saved_without_an_undo_step() {
        let script = Script::parse("mirror@3,hide@4,show@5,local@6,close@7,kept@8");
        let kept = ["mirror", "hide", "show", "local"].map(Step::Kept);
        let steps = [kept[0], kept[1], kept[2], kept[3], Step::Close, Step::KeptPrint];
        assert_eq!(script.steps.iter().map(|&(_, step)| step).collect::<Vec<_>>(), steps);
    }

    #[test]
    fn a_script_keeps_frames_coming_until_its_last_step() {
        let mut script = Script::parse("on");
        assert!(script.on && !script.running(), "nothing to do: frames come on input, as ever");
        script = Script::parse("scene@3");
        assert!(script.running());
        script.frame = 3;
        assert!(!script.running(), "its last step has run");
        assert!(!Script::default().running() && !Script::default().on, "without the variable there is no script");
    }

    #[test]
    fn the_shortest_interval_a_script_can_ask_for_is_not_nothing() {
        // Rule 9 of crates/README.md: a limit is tested at its lowest input.
        assert_eq!(Script::parse("every=0").every, Some(Duration::from_millis(10)));
        assert_eq!(Script::parse("every=-5").every, Some(Duration::from_millis(10)));
        assert_eq!(Script::parse("every=NaN").every, None, "not a time at all");
    }
}
