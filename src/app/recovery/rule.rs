//! The rule for when a recovery copy may be removed (the top of
//! `crate::recovery`), held against runs of the app that end every way a run
//! can. No window: a [`Run`] here is the scene's history, its file and the
//! recovery [`State`], worked in a few lines each as `Running` works them, on
//! real folders with the real locks. A scene is one object, told from
//! another by its name, and a copy is a small file holding that name.
//!
//! The named tests are the cases a review found, each of which lost work:
//! Restore It and one Undo, a copy of this run's own, an answer about one of
//! two runs. The last test walks random sequences of everything a person can
//! do (answer the question any way, edit, Undo, Redo, Open, New Scene, Save,
//! wait for a copy, close, crash) over two or three runs that died, and after every
//! step compares what is on disk with what the rule allows to have gone. It
//! keeps its own account of which run each scene was restored from
//! ([`Shadow`]) and of what each dead run left ([`Dead`]), so that it does
//! not take the code's word for either.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::*;
use crate::app::file_undo::File;
use crate::app::tools::Tools;
use crate::history::{FileState, History};
use crate::mesh::Mesh;
use crate::mesh::primitives::{self, Primitive};
use crate::scene::Scene;

/// What a copy holds once it has been damaged: it no longer reads.
const DAMAGED: &str = "damaged";

/// A settings folder of its own under the temp folder, empty.
fn settings(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("sculpt_recovery_rule_tests").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a folder for the test");
    dir
}

/// Every file under the recovery folder.
fn on_disk(settings: &Path) -> BTreeSet<PathBuf> {
    let mut files = BTreeSet::new();
    let folders = std::fs::read_dir(settings.join("recovery")).into_iter().flatten().flatten();
    for folder in folders.map(|entry| entry.path()) {
        files.extend(std::fs::read_dir(&folder).into_iter().flatten().flatten().map(|entry| entry.path()));
    }
    files
}

fn is_copy(file: &Path) -> bool {
    file.extension().is_some_and(|kind| kind == "sclp")
}

/// The test's own account of a run that died and left copies.
#[derive(Clone, Debug)]
struct Dead {
    folder: PathBuf,
    /// Its copies in the order they were written: the file, the scene it
    /// holds, and whether it was made as the run stopped on a fault.
    copies: Vec<(PathBuf, String, bool)>,
}

impl Dead {
    /// The copies an answer should try, in order, as the question promises:
    /// from the newest, or (`earlier`) from the newest regular one.
    fn in_order(&self, earlier: bool) -> Vec<&(PathBuf, String, bool)> {
        let newest_first: Vec<_> = self.copies.iter().rev().collect();
        let from = if earlier { newest_first.iter().position(|copy| !copy.2).unwrap_or(0) } else { 0 };
        newest_first[from..].to_vec()
    }
}

/// The test's own account of which run the scene on screen was restored
/// from, kept through Undo and Redo without the code's help: for each step,
/// `None` if it left the scene's origin alone, or the origin of the scene on
/// its other side.
#[derive(Default)]
struct Shadow {
    here: Option<PathBuf>,
    undo: Vec<Option<Option<PathBuf>>>,
    redo: Vec<Option<Option<PathBuf>>>,
}

impl Shadow {
    fn edited(&mut self) {
        self.undo.push(None);
        self.redo.clear();
    }

    /// Another scene took this one's place; `origin` is the run it was restored from, if any.
    fn replaced(&mut self, origin: Option<PathBuf>) {
        self.undo.push(Some(std::mem::replace(&mut self.here, origin)));
        self.redo.clear();
    }

    fn stepped(&mut self, undo: bool) {
        let (from, to) = if undo { (&mut self.undo, &mut self.redo) } else { (&mut self.redo, &mut self.undo) };
        let mut step = from.pop().expect("a step to take");
        if let Some(other) = &mut step {
            std::mem::swap(other, &mut self.here);
        }
        to.push(step);
    }
}

/// What a step is allowed to have removed, by the rule.
#[derive(Default)]
struct Allowed {
    /// This run's own copies, all of them: its scene was saved (a), or it closed properly (c).
    own: bool,
    /// The oldest of this run's regular copies, past the newest three: it has just written one.
    own_oldest: bool,
    /// A dead run, whole: the scene that was restored from it has been saved (a).
    saved_from: Option<PathBuf>,
    /// A dead run, whole: the user said to discard it, and this run has saved or closed properly (b).
    discarded: Option<PathBuf>,
}

impl Allowed {
    /// The dead runs that may go, and so must.
    fn runs(&self) -> Vec<PathBuf> {
        self.saved_from.iter().chain(&self.discarded).cloned().collect()
    }

    /// This step and `next` taken as one.
    fn then(self, next: Allowed) -> Allowed {
        Allowed {
            own: self.own || next.own,
            own_oldest: self.own_oldest || next.own_oldest,
            saved_from: self.saved_from.or(next.saved_from),
            discarded: self.discarded.or(next.discarded),
        }
    }
}

/// One run of the app, as far as its scene's file, its history and its copies go.
struct Run {
    settings: PathBuf,
    /// The name of this run's folder of copies.
    name: String,
    state: State,
    scene: Scene,
    mesh: Mesh,
    history: History,
    tools: Tools,
    path: Option<PathBuf>,
    clean: Option<Mark>,
    /// What the question at start-up had to say.
    offer: Option<Offer>,
    /// The copies this run has written and not cleared, oldest first.
    written: Vec<(PathBuf, String, bool)>,
    shadow: Shadow,
    /// The run the user said Discard It about.
    discarded: Option<PathBuf>,
    /// The files as they were after the last step that was checked.
    before: BTreeSet<PathBuf>,
}

impl Run {
    /// The app starts: the scene it starts with, nothing unsaved, and a look
    /// for what earlier runs left. `number` names this run's folder.
    fn start(settings: &Path, number: usize) -> Run {
        let name = format!("run-{number}");
        let store = Store::at(Some(settings), true).named(&name);
        let mut state = State::with(store, Script::default());
        let offer = state.look();
        let mesh = primitives::build(Primitive::Sphere, 80, 0.5);
        let mut run = Run {
            settings: settings.to_owned(),
            name,
            state,
            scene: Scene::new("start"),
            mesh,
            history: History::default(),
            tools: Tools::default(),
            path: None,
            clean: None,
            offer,
            written: Vec::new(),
            shadow: Shadow::default(),
            discarded: None,
            before: on_disk(settings),
        };
        // As `Running::mark_saved` at start-up.
        run.clean = Some(run.mark());
        run
    }

    fn mark(&self) -> Mark {
        Mark::of(&self.history, &self.scene, &self.tools)
    }

    /// As `Running::file`.
    fn file(&mut self) -> File<'_> {
        let (path, clean, restored) = (&mut self.path, &mut self.clean, &mut self.state.restored);
        File { path, clean, restored, history: &mut self.history }
    }

    fn unsaved(&self) -> bool {
        self.clean != Some(self.mark())
    }

    /// The name of the scene on screen.
    fn shown(&self) -> String {
        self.scene.objects[0].name.clone()
    }

    /// This run's folder of copies, if it has one.
    fn folder(&self) -> Option<PathBuf> {
        self.written.last().and_then(|(file, ..)| file.parent()).map(Path::to_owned)
    }

    /// The run that the scene on screen was restored from, by the code's own note.
    fn origin(&self) -> Option<PathBuf> {
        self.state.restored.as_ref().map(|from| from.run.clone())
    }

    /// An edit: the object renamed, as one undo step.
    fn edit(&mut self, name: &str) {
        let before = self.scene.state();
        self.scene.objects[0].name = name.into();
        self.history.push_scene(before);
        self.shadow.edited();
    }

    /// As `Running::put_in_place`: the scene `name` takes this one's place,
    /// as one undo step that keeps this scene's file. `file` is what it
    /// brings: its file (an Open), the copy it was restored from, or neither
    /// (a new scene). Unsaved work in the scene that is put aside gets the
    /// copy it is owed first (`Running::recovery_before_replacing`); gives
    /// back whether one was written.
    fn replace(&mut self, name: &str, file: FileState) -> bool {
        let copied = self.timed_copy();
        let now = self.mark();
        let before = self.file().state(now);
        let state = self.scene.state();
        self.scene.objects[0].name = name.into();
        self.history.push_scene(state);
        self.history.keep_file(before);
        self.shadow.replaced(file.restored.as_ref().map(|from| from.run.clone()));
        let now = self.mark();
        self.file().became(file, now);
        copied
    }

    /// Open, with "Open Without Saving" if anything is unsaved. Gives back
    /// whether a copy of the scene that was put aside was written first.
    fn open(&mut self, file: &Path) -> bool {
        let name = std::fs::read_to_string(file).expect("the file reads");
        self.replace(&name, FileState::opened(file.to_owned()))
    }

    /// New Scene (`Running::new_scene`), with "New Scene Without Saving" if
    /// anything is unsaved: a scene called `name`, in no file, with nothing
    /// in it to save. Gives back whether a copy of the scene that was put
    /// aside was written first.
    fn new_scene(&mut self, name: &str) -> bool {
        self.replace(name, FileState::new_scene())
    }

    /// As `Running::restore`: one copy put in place of the scene.
    fn put(&mut self, kept: &Kept) -> Tried {
        let name = match std::fs::read_to_string(&kept.file) {
            Ok(name) if name != DAMAGED => name,
            _ => return Tried::Unreadable(format!("{} is damaged", kept.file.display())),
        };
        self.replace(&name, FileState::restored(State::note(kept)));
        let mark = self.mark();
        self.state.restored_at(mark);
        Tried::Restored(1)
    }

    /// As `Running::recover` for Restore It, or (`earlier`) Restore the
    /// Earlier Copy Instead. Gives back the copy that was restored and how
    /// many were passed over for it.
    fn restore(&mut self, earlier: bool) -> Option<(PathBuf, usize)> {
        let copies = self.state.to_restore(earlier);
        match restore_first(&copies, |kept| self.put(kept)) {
            Restoring::Done { kept, passed_over, .. } => Some((kept.file.clone(), passed_over)),
            Restoring::NoneRead => None,
            Restoring::Broke => unreachable!("nothing here faults"),
        }
    }

    /// The answer Discard It.
    fn discard(&mut self) {
        self.discarded = self.state.discard().map(Path::to_owned);
    }

    /// As Undo and Redo in `Running::act`.
    fn step(&mut self, undo: bool) {
        let was_saved = !self.unsaved();
        assert!(self.history.step_scene(undo, &mut self.scene, &mut self.mesh), "nothing to step");
        let now = self.mark();
        self.file().stepped(undo, was_saved, now);
        self.shadow.stepped(undo);
    }

    /// Writes a copy as `Running::keep_copy` does.
    fn write_copy(&mut self, stopped: bool) {
        let mark = self.mark();
        let file = self.state.store.next_file(stopped).expect("a file for the copy");
        std::fs::write(&file, self.shown()).expect("the copy writes");
        self.state.store.wrote(&file, &About { objects: 1, document: self.path.clone(), ..About::default() });
        if !stopped {
            self.state.copied(mark);
        }
        self.written.push((file, self.shown(), stopped));
    }

    /// Three minutes pass with the hand off the model (`Running::recovery_tick`):
    /// a copy is written if one is owed. Returns whether one was.
    fn timed_copy(&mut self) -> bool {
        let owed = copy_owed(self.state.store.keeps(), self.unsaved(), self.state.held, self.mark());
        if owed {
            self.write_copy(false);
        }
        owed
    }

    /// As `Running::save_scene` and `save_to`: to the scene's own file, or
    /// to `new` where the app would ask for one. Gives back what the rule
    /// lets this save remove.
    fn save(&mut self, new: &Path) -> Allowed {
        let path = self.path.clone().unwrap_or_else(|| new.to_owned());
        std::fs::write(&path, self.shown()).expect("the save writes");
        let now = self.mark();
        let restored = self.file().written(&path, now);
        self.state.saved(restored);
        self.path = Some(path);
        self.written.clear();
        // (a) for the run this very scene was restored from, (b) for one that was discarded.
        Allowed { own: true, own_oldest: false, saved_from: self.shadow.here.take(), discarded: self.discarded.take() }
    }

    /// The app closes (`Running::recovery_exit`). `answered`: the question
    /// about closing was asked and answered with a way to close. Gives back
    /// what the rule lets this remove.
    fn close(&mut self, answered: bool) -> Allowed {
        let settled = answered || !self.unsaved();
        self.state.closed(settled);
        if !settled {
            return Allowed::default();
        }
        self.written.clear();
        Allowed { own: true, discarded: self.discarded.take(), ..Allowed::default() }
    }

    /// The app stops on a fault (`Running::last_copy` after a panic): one
    /// more copy if the newest does not hold the scene as it stands.
    fn fault(&mut self) {
        if self.unsaved() && self.state.store.keeps() && self.state.held != Some(self.mark()) {
            self.write_copy(true);
        }
    }

    /// The program is gone, however it went. What it left is the next run's
    /// to find; gives back the test's account of it.
    fn end(mut self) -> Option<Dead> {
        let folder = self.folder();
        self.state.store.crash();
        let copies: Vec<_> = self.written.into_iter().filter(|(file, ..)| file.is_file()).collect();
        folder.filter(|_| !copies.is_empty()).map(|folder| Dead { folder, copies })
    }

    /// Whether `folder` is one this run made for its own copies.
    fn is_own(&self, folder: &Path) -> bool {
        let name = folder.file_name().unwrap_or_default().to_string_lossy();
        name == self.name.as_str() || name.starts_with(&format!("{}-", self.name))
    }

    /// Holds what is on disk now against what was there after the last
    /// check: nothing has gone but what `allowed` names, all of that has,
    /// and the code's note of where the scene came from agrees with the
    /// test's own.
    fn check(&mut self, what: &str, allowed: &Allowed) {
        let now = on_disk(&self.settings);
        let runs = allowed.runs();
        for gone in self.before.difference(&now) {
            let folder = gone.parent().expect("a file is in a folder");
            if self.is_own(folder) {
                let oldest = allowed.own_oldest && !gone.to_string_lossy().contains("stopped");
                assert!(allowed.own || oldest, "{what}: {} of this run's own went", gone.display());
            } else {
                assert!(runs.iter().any(|run| run == folder), "{what}: {} went, outside the rule", gone.display());
            }
        }
        for run in &runs {
            assert!(!run.exists(), "{what}: {} should have gone, folder and all", run.display());
        }
        if allowed.own {
            let left = now.iter().find(|file| file.parent().is_some_and(|folder| self.is_own(folder)));
            assert_eq!(left, None, "{what}: this run's own copies should have gone");
        }
        // Of its regular copies, the newest three it wrote are the ones that are there.
        let regular: Vec<&PathBuf> = self.written.iter().filter(|copy| !copy.2).map(|(file, ..)| file).collect();
        let newest = regular.len().saturating_sub(recovery::KEEP);
        for (i, file) in regular.iter().enumerate() {
            assert_eq!(file.is_file(), i >= newest, "{what}: {} (the newest {} stay)", file.display(), recovery::KEEP);
        }
        assert_eq!(self.origin(), self.shadow.here, "{what}: where the scene on screen was restored from");
        self.before = now;
    }
}

/// A run that makes `copies` regular copies of scenes called `"<name> 1"`,
/// `"<name> 2"`..., one more as it stops on a fault if `stopped`, and dies.
fn dies(settings: &Path, number: usize, name: &str, copies: usize, stopped: bool) -> Dead {
    let mut run = Run::start(settings, number);
    for i in 1..=copies {
        run.edit(&format!("{name} {i}"));
        assert!(run.timed_copy());
    }
    if stopped {
        run.edit(&format!("{name}, caught mid-change"));
        run.fault();
    }
    run.end().expect("it left copies")
}

fn copies_of(dead: &Dead) -> Vec<bool> {
    dead.copies.iter().map(|(file, ..)| file.is_file()).collect()
}

/// Review point 1: crash, start, Restore It, a stray Z, close the window.
/// Undo left a scene with nothing unsaved, so closing asked nothing, and the
/// clean exit then removed the copies as "answered about".
#[test]
fn restore_then_one_undo_then_closing_removes_no_copy_and_the_next_start_asks_again() {
    let dir = settings("restore_undo_close");
    let crashed = dies(&dir, 1, "dragon", 2, false);
    let mut run = Run::start(&dir, 2);
    assert_eq!(run.offer.as_ref().map(|offer| offer.newest.file.clone()), Some(crashed.copies[1].0.clone()));
    assert!(run.restore(false).is_some());
    assert_eq!(run.shown(), "dragon 2");
    assert!(run.unsaved() && run.path.is_none(), "restored: unsaved, with no file");
    assert_eq!(run.origin(), Some(crashed.folder.clone()));

    run.step(true);
    assert_eq!(run.shown(), "start");
    assert!(!run.unsaved(), "the scene from before the restore: closing asks nothing");
    assert_eq!(run.origin(), None, "and it is not the restored copy: Save has no crashed file's name to offer");
    let allowed = run.close(false);
    run.check("closing after Undo of the restore", &allowed);
    assert_eq!(copies_of(&crashed), [true, true], "the recovered work is still on disk");
    assert!(run.end().is_none());

    // And it is asked about again. Redo of the restore brings the note back with the scene.
    let mut run = Run::start(&dir, 3);
    assert_eq!(run.offer.as_ref().map(|offer| offer.newest.file.clone()), Some(crashed.copies[1].0.clone()));
    run.restore(false);
    run.step(true);
    run.step(false);
    assert_eq!((run.shown().as_str(), run.origin()), ("dragon 2", Some(crashed.folder.clone())));
    // Saving the restored scene is what retires the copies.
    let allowed = run.save(&dir.join("dragon.sclp"));
    run.check("saving the restored scene", &allowed);
    assert_eq!(copies_of(&crashed), [false, false]);
    assert!(!crashed.folder.exists());
}

/// The same from the other side: after Undo of the restore it is the scene
/// from before that is saved, and that settles nothing about the copies.
#[test]
fn saving_the_scene_from_before_the_restore_does_not_retire_the_copies() {
    let dir = settings("save_other_scene");
    let crashed = dies(&dir, 1, "dragon", 1, false);
    let mut run = Run::start(&dir, 2);
    run.edit("a new idea");
    run.restore(false);
    run.step(true);
    assert_eq!(run.shown(), "a new idea");
    let allowed = run.save(&dir.join("idea.sclp"));
    assert!(allowed.runs().is_empty());
    run.check("saving the scene from before the restore", &allowed);
    assert_eq!(copies_of(&crashed), [true]);
    // Opening another file over the restored scene, and saving that, is the same.
    run.step(false);
    assert_eq!(run.shown(), "dragon 1");
    std::fs::write(dir.join("other.sclp"), "other").unwrap();
    run.open(&dir.join("other.sclp"));
    run.edit("other, edited");
    let allowed = run.save(&dir.join("unused.sclp"));
    run.check("saving a file opened over the restored scene", &allowed);
    assert_eq!(copies_of(&crashed), [true], "the restored scene is put aside, not saved");
    let allowed = run.close(false);
    run.check("closing", &allowed);
    assert_eq!(copies_of(&crashed), [true]);
}

/// Where recovery meets New Scene: a new scene started over a restored scene
/// that has not been saved puts the crashed work one Undo away. Neither that,
/// nor saving the new scene, nor closing, is a save of the restored scene or
/// an answer to discard it, so the crashed run's copies stay.
#[test]
fn a_new_scene_over_a_restored_scene_that_is_not_saved_retires_none_of_that_runs_copies() {
    let dir = settings("new_scene_over_restored");
    let crashed = dies(&dir, 1, "dragon", 2, false);
    let mut run = Run::start(&dir, 2);
    assert!(run.restore(false).is_some());
    assert_eq!(run.origin(), Some(crashed.folder.clone()));

    // New Scene Without Saving. The restored scene stands where its copy holds it, so none is owed.
    assert!(!run.new_scene("a fresh ball"), "the copy it was restored from already holds it");
    assert!(!run.unsaved() && run.path.is_none(), "the new scene: in no file, nothing unsaved");
    assert_eq!(run.origin(), None, "and it is not the restored copy");
    run.check("New Scene over the restored scene", &Allowed::default());
    assert!(!run.timed_copy(), "an untouched new scene is owed no copy");
    assert_eq!(copies_of(&crashed), [true, true]);

    // Work in the new scene, saved: (a) is for this run's own copies, of which there are none yet.
    run.edit("a fresh ball, worked on");
    let allowed = run.save(&dir.join("ball.sclp"));
    assert!(allowed.runs().is_empty(), "the save of the new scene retires no crashed run");
    run.check("saving the new scene", &allowed);
    assert_eq!(copies_of(&crashed), [true, true], "the crashed work is in no file of the user's yet");

    // Closed with nothing unsaved, and nothing asked: the next start offers the copies again.
    let allowed = run.close(false);
    run.check("closing from the saved new scene", &allowed);
    assert_eq!(copies_of(&crashed), [true, true]);
    assert!(run.end().is_none());
    let mut run = Run::start(&dir, 3);
    assert_eq!(run.offer.as_ref().map(|offer| offer.newest.file.clone()), Some(crashed.copies[1].0.clone()));

    // This time: restore, New Scene, Undo back to the restored scene, and save that.
    run.restore(false);
    run.new_scene("another ball");
    run.step(true);
    assert_eq!((run.shown().as_str(), run.origin()), ("dragon 2", Some(crashed.folder.clone())));
    assert!(run.unsaved(), "Undo of the new scene gives the restored scene back unsaved");
    let allowed = run.save(&dir.join("dragon.sclp"));
    assert_eq!(allowed.runs(), [crashed.folder.clone()]);
    run.check("saving the restored scene after Undo of a New Scene", &allowed);
    assert_eq!(copies_of(&crashed), [false, false]);
}

/// Where the copies meet "Open Without Saving" and "New Scene Without
/// Saving": work under three minutes old had no copy yet, and once it was put
/// aside nothing wrote one until Undo came back to it, so a crash in between
/// lost work that Undo would have returned.
#[test]
fn unsaved_work_put_aside_by_a_new_scene_or_an_open_has_a_copy_first_and_a_crash_offers_it() {
    for open in [false, true] {
        let dir = settings(if open { "aside_by_open" } else { "aside_by_new_scene" });
        let library = dir.join("library.sclp");
        std::fs::write(&library, "library").unwrap();
        let mut run = Run::start(&dir, 1);
        run.edit("two minutes of strokes");
        assert!(run.written.is_empty(), "three minutes are not up: no timed copy yet");
        let copied = if open { run.open(&library) } else { run.new_scene("a fresh ball") };
        assert!(copied, "the copy that was owed is written before the scene is put aside");
        run.check("the scene put aside", &Allowed { own_oldest: true, ..Allowed::default() });
        assert_eq!(run.written.last().map(|copy| copy.1.as_str()), Some("two minutes of strokes"));
        assert!(!run.unsaved() && !run.timed_copy(), "the scene that took its place is owed none");
        // The power goes.
        let left = run.end().expect("the copy is there for the next start");
        let mut run = Run::start(&dir, 2);
        assert_eq!(run.restore(false), Some((left.copies[0].0.clone(), 0)));
        assert_eq!(run.shown(), "two minutes of strokes");
    }
    // Nothing is written where nothing is owed: a scene with nothing unsaved,
    // and one that a copy already holds as it stands.
    let dir = settings("aside_nothing_owed");
    let mut run = Run::start(&dir, 1);
    assert!(!run.new_scene("a fresh ball"), "the untouched starting scene");
    run.edit("a fresh ball, worked on");
    assert!(run.timed_copy());
    assert!(!run.new_scene("another"), "the timed copy holds it already");
    assert_eq!(run.written.len(), 1);
    // And the copy follows the rule like any of this run's own: a proper close removes it.
    run.edit("another, worked on");
    assert!(run.new_scene("a third"));
    let allowed = run.close(false);
    assert!(allowed.own, "closing with nothing unsaved is a proper close");
    run.check("closing", &allowed);
    assert!(run.end().is_none());
}

/// Review point 2: the question warns that the copy made on a fault may be
/// incomplete and offers the earlier one. The user takes Restore It; three
/// minutes later this run's first copy removed every copy the crash left,
/// the earlier one with them. So did Exit Without Saving, at once.
#[test]
fn a_copy_of_this_runs_own_and_exit_without_saving_leave_the_earlier_copy_to_be_had() {
    let dir = settings("earlier_stays");
    let crashed = dies(&dir, 1, "dragon", 2, true);
    assert_eq!(crashed.copies.len(), 3);
    let mut run = Run::start(&dir, 2);
    let offer = run.offer.as_ref().expect("the copies are offered");
    assert!(offer.newest.stopped, "the newest is the one made as the app stopped");
    assert_eq!(offer.earlier.as_ref().map(|kept| kept.file.clone()), Some(crashed.copies[1].0.clone()));
    assert_eq!(run.restore(false), Some((crashed.copies[2].0.clone(), 0)));
    assert_eq!(run.shown(), "dragon, caught mid-change");
    assert!(!run.timed_copy(), "the copy it was restored from holds the scene as it stands: none is owed");
    run.edit("dragon, looked at");
    assert!(run.timed_copy(), "after a change, this run writes a copy of its own");
    run.check("this run's own copy", &Allowed { own_oldest: true, ..Allowed::default() });
    assert_eq!(copies_of(&crashed), [true, true, true], "which removes nothing the crash left");

    // It is no good. Exit Without Saving, to start again and take the earlier one.
    let allowed = run.close(true);
    run.check("Exit Without Saving", &allowed);
    assert_eq!(copies_of(&crashed), [true, true, true]);
    assert!(run.end().is_none(), "this run closed properly: its own copy has gone");

    let mut run = Run::start(&dir, 3);
    assert!(run.offer.as_ref().is_some_and(|offer| offer.earlier.is_some()));
    assert_eq!(run.restore(true), Some((crashed.copies[1].0.clone(), 0)));
    assert_eq!(run.shown(), "dragon 2", "the earlier copy, as it was");
}

/// Review point 3: copies from several crashed runs were one list. The
/// question was about the newest copy of all, the "earlier copy" could be
/// another run's scene, and one answer removed every run's copies.
#[test]
fn an_answer_about_one_run_touches_no_other_runs_copies_and_they_are_asked_about_next() {
    for discard in [false, true] {
        let dir = settings(if discard { "two_runs_discard" } else { "two_runs_save" });
        // Two windows open when the power went; the second made its last copy on a fault.
        let first = dies(&dir, 1, "dragon", 2, false);
        let second = dies(&dir, 2, "lure", 0, true);
        let mut run = Run::start(&dir, 3);
        let offer = run.offer.as_ref().expect("the copies are offered");
        assert_eq!((offer.newest.file.clone(), offer.others), (second.copies[0].0.clone(), 1));
        assert_eq!(offer.earlier, None, "the other run's regular copy is not this one's earlier copy");
        let allowed = if discard {
            run.discard();
            run.check("Discard It", &Allowed::default());
            assert_eq!(copies_of(&second), [true], "not yet: until this run saves or closes");
            run.close(false)
        } else {
            assert_eq!(run.restore(false), Some((second.copies[0].0.clone(), 0)));
            run.save(&dir.join("lure.sclp"))
        };
        assert_eq!(allowed.runs(), [second.folder.clone()]);
        run.check("the answer carried out", &allowed);
        assert_eq!((copies_of(&second), copies_of(&first)), (vec![false], vec![true, true]));
        let _ = run.close(true);
        assert!(run.end().is_none());

        let run = Run::start(&dir, 4);
        let offer = run.offer.as_ref().expect("the other run is asked about now");
        assert_eq!((offer.newest.file.clone(), offer.others), (first.copies[1].0.clone(), 0));
    }
}

#[test]
fn not_now_and_a_run_that_only_restores_remove_nothing() {
    let dir = settings("not_now");
    let crashed = dies(&dir, 1, "dragon", 3, true);
    // Not Now, some work of this run's own, saved, closed.
    let mut run = Run::start(&dir, 2);
    run.edit("something else");
    run.timed_copy();
    let allowed = run.save(&dir.join("else.sclp"));
    run.check("saving after Not Now", &allowed);
    let allowed = run.close(false);
    run.check("closing after Not Now", &allowed);
    assert_eq!(copies_of(&crashed), [true; 4]);
    assert!(run.end().is_none());
    // Restore It, then the program is killed before anything else.
    let mut run = Run::start(&dir, 3);
    assert!(run.restore(false).is_some());
    run.check("Restore It", &Allowed::default());
    assert!(run.end().is_none(), "it wrote no copy of its own");
    assert_eq!(copies_of(&crashed), [true; 4]);
    assert_eq!(Run::start(&dir, 4).offer.map(|offer| offer.newest.file), Some(crashed.copies[3].0.clone()));
}

/// Rule (c) has a condition. Something that closes the app over unsaved work
/// without the question (the Claude bridge's quit does) is not the user
/// giving the work up.
#[test]
fn closing_over_unsaved_work_that_nobody_was_asked_about_keeps_this_runs_copies() {
    let dir = settings("closed_unasked");
    let mut run = Run::start(&dir, 1);
    run.edit("an hour of work");
    assert!(run.timed_copy());
    let allowed = run.close(false);
    assert!(!allowed.own, "unsaved, and nobody answered");
    run.check("closed without the question", &allowed);
    let left = run.end().expect("its copy is still there");
    let mut run = Run::start(&dir, 2);
    assert_eq!(run.restore(false), Some((left.copies[0].0.clone(), 0)), "and the next start offers it");
    assert_eq!(run.shown(), "an hour of work");
}

/// A small random number generator, the same every time.
struct Dice(u64);

impl Dice {
    fn below(&mut self, below: u64) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) % below
    }

    fn one_in(&mut self, n: u64) -> bool {
        self.below(n) == 0
    }
}

/// How often each thing happened in the walk, to be sure it walked.
#[derive(Default, Debug)]
struct Seen {
    restored: usize,
    earlier: usize,
    passed_over: usize,
    none_read: usize,
    discarded: usize,
    retired_saved: usize,
    retired_discarded: usize,
    crossed_back: usize,
    several_left: usize,
    closed_unasked: usize,
    /// New Scene, and New Scene over a restored scene that had not been saved.
    new_scenes: usize,
    new_over_restored: usize,
    /// An Open or a New Scene over unsaved work that had no copy yet, which wrote one first.
    copied_aside: usize,
}

impl Seen {
    fn count(&mut self, allowed: &Allowed) {
        self.retired_saved += allowed.saved_from.is_some() as usize;
        self.retired_discarded += allowed.discarded.is_some() as usize;
    }
}

/// The answer to the question at start-up, taken at random, with what the
/// question promised checked against the test's own account of the runs.
fn answer(run: &mut Run, dead: &[Dead], dice: &mut Dice, seen: &mut Seen) {
    let Some(asked) = dead.last() else {
        assert!(run.offer.is_none(), "nothing was left, and something is offered");
        return;
    };
    let newest = asked.copies.last().expect("a dead run has copies");
    let regular = asked.copies.iter().rev().find(|copy| !copy.2).filter(|_| newest.2);
    let offer = run.offer.as_ref().expect("copies were left, and none is offered");
    assert_eq!(offer.newest.file, newest.0, "the question is about the newest copy of the run that started last");
    assert_eq!((offer.newest.stopped, offer.others), (newest.2, dead.len() - 1));
    let earlier = offer.earlier.as_ref().map(|kept| &kept.file);
    assert_eq!(earlier, regular.map(|copy| &copy.0), "the earlier copy is its own run's");
    seen.several_left += (dead.len() > 1) as usize;
    let earlier = earlier.is_some() && dice.one_in(3);
    match dice.below(6) {
        // Not Now.
        0 => {}
        1 => {
            run.discard();
            seen.discarded += 1;
        }
        _ => {
            let before = run.shown();
            let tried = asked.in_order(earlier);
            let expected = tried.iter().position(|copy| copy.1 != DAMAGED);
            let restored = run.restore(earlier);
            let newest_that_reads = expected.map(|at| (tried[at].0.clone(), at));
            assert_eq!(restored, newest_that_reads, "Restore It restores the newest copy that reads");
            match expected {
                Some(at) => assert_eq!(run.shown(), tried[at].1, "and the scene is the one that copy holds"),
                None => assert_eq!(run.shown(), before, "none reads: the scene is as it was"),
            }
            seen.restored += expected.is_some() as usize;
            seen.earlier += (earlier && expected.is_some()) as usize;
            seen.passed_over += expected.is_some_and(|at| at > 0) as usize;
            seen.none_read += expected.is_none() as usize;
        }
    }
    run.check("the answer", &Allowed::default());
}

/// Random sequences of everything a person can do, over two or three runs
/// that died, with what is on disk held against the rule after every step.
#[test]
fn a_random_walk_never_removes_a_copy_outside_the_rule() {
    let mut seen = Seen::default();
    for world in 0..36u64 {
        let dir = settings(&format!("walk_{world}"));
        let mut dice = Dice(0x9e37_79b9_7f4a_7c15 ^ (world << 20));
        let library = dir.join("library.sclp");
        std::fs::write(&library, "library").unwrap();
        // Two or three runs that died, each leaving what it would.
        let mut dead: Vec<Dead> = Vec::new();
        let mut started = 0;
        for _ in 0..2 + dice.below(2) {
            started += 1;
            let (copies, stopped) = (dice.below(4) as usize, dice.one_in(2));
            dead.push(dies(&dir, started, &format!("crashed {started}"), copies.max(!stopped as usize), stopped));
        }
        for _ in 0..7 {
            // Now and then the newest copy a crash left is damaged on disk.
            let newest = dead.last_mut().and_then(|run| run.copies.last_mut());
            if let Some((file, name, _)) = newest.filter(|_| dice.one_in(6)) {
                std::fs::write(&*file, DAMAGED).unwrap();
                *name = DAMAGED.to_owned();
            }
            started += 1;
            let mut run = Run::start(&dir, started);
            let folders: BTreeSet<&Path> = run.before.iter().filter_map(|file| file.parent()).collect();
            let known: BTreeSet<&Path> = dead.iter().map(|run| run.folder.as_path()).collect();
            assert_eq!(folders, known, "world {world}: what is on disk at a start is what the dead runs left");
            answer(&mut run, &dead, &mut dice, &mut seen);
            for turn in 0..dice.below(14) {
                let mut allowed = Allowed::default();
                match dice.below(12) {
                    0..=2 => run.edit(&format!("run {started} edit {turn}")),
                    3..=5 => allowed.own_oldest = run.timed_copy(),
                    6 | 7 if run.history.can_undo() => {
                        run.step(true);
                        seen.crossed_back += run.shadow.redo.last().is_some_and(|step| step.is_some()) as usize;
                    }
                    8 if run.history.can_redo() => run.step(false),
                    9 => {
                        allowed.own_oldest = run.open(&library);
                        seen.copied_aside += allowed.own_oldest as usize;
                    }
                    10 => allowed = run.save(&dir.join(format!("saved by run {started}.sclp"))),
                    11 if dice.one_in(2) => {
                        seen.new_scenes += 1;
                        seen.new_over_restored += run.shadow.here.is_some() as usize;
                        allowed.own_oldest = run.new_scene(&format!("run {started} new scene {turn}"));
                        seen.copied_aside += allowed.own_oldest as usize;
                        assert!(!run.unsaved(), "world {world}: a new scene with something unsaved in it");
                    }
                    _ => {}
                }
                run.check(&format!("world {world}, run {started}, turn {turn}"), &allowed);
                seen.count(&allowed);
                dead.retain(|run| !allowed.runs().contains(&run.folder));
            }
            // The run ends: closed properly, closed unasked, killed, or stopped on a fault.
            let allowed = match dice.below(8) {
                0..=2 => {
                    // The question, if there is one: Save and Exit, or Exit Without Saving.
                    let save_first = run.unsaved() && dice.one_in(2);
                    let new = dir.join("saved on the way out.sclp");
                    let saved = if save_first { run.save(&new) } else { Allowed::default() };
                    saved.then(run.close(true))
                }
                3 => {
                    seen.closed_unasked += run.unsaved() as usize;
                    run.close(false)
                }
                4 | 5 => {
                    run.fault();
                    Allowed::default()
                }
                _ => Allowed::default(),
            };
            let what = format!("world {world}, the end of run {started}");
            run.check(&what, &allowed);
            seen.count(&allowed);
            dead.retain(|run| !allowed.runs().contains(&run.folder));
            dead.extend(run.end());
            let now: BTreeSet<PathBuf> = on_disk(&dir).into_iter().filter(|file| is_copy(file)).collect();
            let copies = dead.iter().flat_map(|run| &run.copies);
            let left: BTreeSet<PathBuf> = copies.map(|copy| copy.0.clone()).collect();
            assert_eq!(now, left, "{what}: the copies on disk are the ones the dead runs left, no more and no fewer");
        }
    }
    // The walk went everywhere it was meant to.
    println!("{seen:?}");
    let counts = [seen.restored, seen.earlier, seen.passed_over, seen.discarded, seen.retired_saved];
    assert!(counts.iter().all(|&count| count >= 3), "{seen:?}");
    let counts = [seen.retired_discarded, seen.crossed_back, seen.several_left, seen.closed_unasked, seen.none_read];
    assert!(counts.iter().all(|&count| count >= 3), "{seen:?}");
    assert!(seen.new_scenes >= 10 && seen.new_over_restored >= 3 && seen.copied_aside >= 10, "{seen:?}");
}
