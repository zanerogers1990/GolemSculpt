//! Open, New Scene, Undo, Redo and Save against real files, with the app's
//! own history and the code that moves the scene's file across an Open
//! (`File`). The `Desk` here does in a few lines each what `Running` does
//! around them, so the cases read as a person at the app: open this, press
//! Z, press Ctrl + S, then look at what is in the files.

use std::path::{Path, PathBuf};

use glam::{Quat, Vec3};

use super::File;
use crate::app::files::Mark;
use crate::app::install;
use crate::app::new_scene::fresh_document;
use crate::app::recovery::copy_owed;
use crate::app::tools::Tools;
use crate::history::{FileState, History, Restored};
use crate::io;
use crate::mesh::Mesh;
use crate::mesh::primitives::{self, Primitive};
use crate::scene::{Scene, Symmetry};

/// A scene with one object, told from another by that object's name.
struct Desk {
    scene: Scene,
    mesh: Mesh,
    history: History,
    /// The hosted tools, which the scene's mark asks what they would save.
    tools: Tools,
    path: Option<PathBuf>,
    clean: Option<Mark>,
    /// The recovery copy the scene on screen was restored from (`recovery::State::restored`).
    restored: Option<Restored>,
    folder: PathBuf,
}

fn ball() -> Mesh {
    primitives::build(Primitive::Sphere, 80, 0.5)
}

/// Writes a sculpt of one ball called `name`, showing, with no symmetry.
fn write(path: &Path, name: &str) {
    write_kept(path, name, (true, false));
}

/// Writes a sculpt of one ball called `name`. `kept` is the part of it that
/// is saved without being an undo step: whether the ball shows (the H key),
/// and whether it is sculpted under Mirror X (the X key).
fn write_kept(path: &Path, name: &str, (visible, mirror_x): (bool, bool)) {
    let (position, rotation, scale) = (Vec3::ZERO, Quat::IDENTITY, Vec3::ONE);
    let object = io::SavedObject { name: name.into(), visible, mesh: 0, position, rotation, scale };
    let mut details = io::Details::default();
    let symmetry = Symmetry { mirror: [mirror_x, false, false], ..Symmetry::default() };
    details.set_object(0, io::slot::SYMMETRY, &symmetry);
    io::save_scene_with(&[&ball()], &[object], 0, &details, path).expect("the file writes");
}

/// The note a scene restored from the copy at `path` carries: the folder the
/// copy is in stands for the run that left it.
fn from(path: &Path) -> Restored {
    Restored { run: path.parent().expect("a copy is in a folder").to_owned(), document: None }
}

/// The name of the scene a file holds.
fn held(path: &Path) -> String {
    io::load_scene(path).expect("the file reads").objects[0].name.clone()
}

/// Whether the ball a file holds shows, and whether it has Mirror X.
fn held_kept(path: &Path) -> (bool, bool) {
    let saved = io::load_scene(path).expect("the file reads");
    let symmetry = crate::scene::saved_symmetry(&saved.details, 0).unwrap_or_default();
    (saved.objects[0].visible, symmetry.mirror[0])
}

impl Desk {
    /// The app as it starts: a scene with no file and nothing unsaved.
    fn new(test: &str) -> Desk {
        let folder = std::env::temp_dir().join("sculpt_file_undo_tests").join(test);
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).expect("a folder for the test");
        let (scene, mesh, history) = (Scene::new("start"), ball(), History::default());
        let tools = Tools::default();
        let mut desk = Desk { scene, mesh, history, tools, path: None, clean: None, restored: None, folder };
        // As `Running::mark_saved` at start-up.
        desk.clean = Some(desk.mark());
        desk
    }

    /// The app with `name` open from a file of that name, saved.
    fn with_open(test: &str, name: &str) -> Desk {
        let mut desk = Desk::new(test);
        let path = desk.file_of(name);
        desk.open(&path);
        desk
    }

    /// A sculpt on disk holding the scene `name`, and where it is.
    fn file_of(&self, name: &str) -> PathBuf {
        let path = self.at(name);
        write(&path, name);
        path
    }

    fn at(&self, name: &str) -> PathBuf {
        self.folder.join(format!("{name}.sclp"))
    }

    /// As `Running::history_mark`.
    fn mark(&self) -> Mark {
        Mark::of(&self.history, &self.scene, &self.tools)
    }

    /// Whether the ball on screen shows, and whether it has Mirror X: what
    /// `held_kept` reads of a file.
    fn kept(&self) -> (bool, bool) {
        let ball = &self.scene.objects[0];
        (ball.visible, self.scene.symmetry(ball.id).mirror[0])
    }

    /// The H key, or Alt + H if the ball is hidden: not an undo step.
    fn hide_key(&mut self) {
        if self.scene.objects[0].visible {
            self.scene.hide_selected(&mut self.mesh);
        } else {
            self.scene.unhide_all();
        }
    }

    /// The X key: Mirror X of the ball on or off. Not an undo step.
    fn mirror_key(&mut self) {
        let ball = self.scene.active_id();
        self.scene.edit_symmetry(ball, |symmetry| symmetry.mirror[0] = !symmetry.mirror[0]);
    }

    fn file(&mut self) -> File<'_> {
        File { path: &mut self.path, clean: &mut self.clean, restored: &mut self.restored, history: &mut self.history }
    }

    fn unsaved(&mut self) -> bool {
        let now = self.mark();
        self.file().unsaved(now)
    }

    /// The name of the scene on screen.
    fn shown(&self) -> &str {
        &self.scene.objects[0].name
    }

    /// The scene on screen, its file and whether anything is unsaved.
    fn all(&mut self) -> (String, Option<String>, bool) {
        let (file, unsaved) = self.stands();
        (self.shown().to_owned(), file, unsaved)
    }

    /// The scene's file by its name alone, and whether anything is unsaved.
    fn stands(&mut self) -> (Option<String>, bool) {
        let name = self.path.as_ref().map(|path| path.file_stem().unwrap().to_string_lossy().into_owned());
        (name, self.unsaved())
    }

    /// An edit: the object renamed, as one undo step.
    fn edit(&mut self, name: &str) {
        let before = self.scene.state();
        self.scene.objects[0].name = name.into();
        self.history.push_scene(before);
    }

    /// As `Running::replace_scene` (an Open: `document`), or as
    /// `Running::restore` (a recovery copy restored, whose run's folder is
    /// taken to be the one the file is in).
    fn replace(&mut self, path: &Path, document: bool) {
        let saved = io::load_scene(path).expect("the file reads");
        let file = if document { FileState::opened(path.to_owned()) } else { FileState::restored(from(path)) };
        self.put_in_place(saved, file);
    }

    /// As `Running::new_scene`: the scene the app starts with, which is
    /// called "Sphere", in place of this one.
    fn new_scene(&mut self) {
        self.put_in_place(fresh_document(80.0, 1.6), FileState::new_scene());
    }

    /// As `Running::put_in_place`.
    fn put_in_place(&mut self, saved: io::SavedScene, file: FileState) {
        let now = self.mark();
        let before = self.file().state(now);
        let state = self.scene.state();
        let details = install(&mut self.scene, &mut self.mesh, saved);
        self.history.push_scene(state);
        self.history.keep_file(before);
        // As `Running::apply_details`: each object's symmetry comes with it.
        self.scene.restore_symmetry(&details);
        let now = self.mark();
        self.file().became(file, now);
    }

    /// Whether `Running::recovery_tick` would count the scene as owed a
    /// recovery copy, none having been written yet.
    fn owed_a_copy(&mut self) -> bool {
        let (unsaved, now) = (self.unsaved(), self.mark());
        copy_owed(true, unsaved, None, now)
    }

    /// Open, in place of the scene.
    fn open(&mut self, path: &Path) {
        self.replace(path, true);
    }

    /// A recovery copy restored.
    fn restore(&mut self, path: &Path) {
        self.replace(path, false);
    }

    /// As `Running::save_scene` and the line after it in `save_to`. Gives
    /// back the recovery copy the saved scene had been restored from, which
    /// is what the app retires that run's copies by.
    fn save_as(&mut self, path: &Path) -> Option<Restored> {
        write_kept(path, &self.shown().to_owned(), self.kept());
        let now = self.mark();
        let restored = self.file().written(path, now);
        self.path = Some(path.to_owned());
        restored
    }

    /// Save (Ctrl + S): writes to the scene's own file and gives it back, or
    /// gives `None` where the app would ask for one.
    fn save(&mut self) -> Option<PathBuf> {
        let path = self.path.clone()?;
        self.save_as(&path);
        Some(path)
    }

    /// As Undo and Redo in `Running::act`.
    fn step(&mut self, undo: bool) {
        let was_saved = !self.unsaved();
        assert!(self.history.step_scene(undo, &mut self.scene, &mut self.mesh), "nothing to step");
        let now = self.mark();
        let (before, crossed) = (self.path.clone(), self.file().stepped(undo, was_saved, now));
        assert!(crossed || self.path == before, "a step that is not an Open changed the scene's file");
    }

    fn undo(&mut self) {
        self.step(true);
    }

    fn redo(&mut self) {
        self.step(false);
    }
}

fn named(name: &str) -> Option<String> {
    Some(name.to_owned())
}

/// What `Desk::all` gives for this scene on screen, this file and this much unsaved.
fn all(shown: &str, file: Option<String>, unsaved: bool) -> (String, Option<String>, bool) {
    (shown.to_owned(), file, unsaved)
}

/// The case the review found: open a file, Undo, Save.
#[test]
fn save_after_undo_of_an_open_writes_the_old_scene_to_its_own_file_not_the_opened_one() {
    let mut desk = Desk::with_open("save_after_undo", "first");
    let second = desk.file_of("second");
    desk.open(&second);
    assert_eq!(desk.all(), all("second", named("second"), false));

    desk.undo();
    assert_eq!(desk.shown(), "first");
    assert_eq!(desk.stands(), (named("first"), false), "the first scene is back: its own file, nothing unsaved");
    let wrote = desk.save();
    assert_eq!(wrote, Some(desk.at("first")), "Save writes to the scene's own file");
    assert_eq!((held(&desk.at("first")), held(&second)), ("first".to_owned(), "second".to_owned()));

    desk.redo();
    assert_eq!(desk.all(), all("second", named("second"), false));
    desk.undo();
    assert_eq!(desk.all(), all("first", named("first"), false));
}

#[test]
fn unsaved_changes_are_still_unsaved_after_an_open_and_its_undo() {
    let mut desk = Desk::with_open("unsaved_changes", "first");
    let second = desk.file_of("second");
    desk.edit("first, edited");
    // Open Without Saving.
    desk.open(&second);
    assert_eq!(desk.stands(), (named("second"), false));
    desk.undo();
    assert_eq!(desk.all(), all("first, edited", named("first"), true));
    assert_eq!(desk.save(), Some(desk.at("first")));
    assert_eq!((held(&desk.at("first")), held(&second)), ("first, edited".to_owned(), "second".to_owned()));
    assert!(!desk.unsaved());
    // The opened scene was left as its file holds it, and is that again.
    desk.redo();
    assert_eq!(desk.all(), all("second", named("second"), false));
}

#[test]
fn a_scene_with_no_file_has_none_again_after_undo_so_save_asks_where() {
    // The untouched scene the app starts with.
    let mut desk = Desk::new("no_file");
    let second = desk.file_of("second");
    desk.open(&second);
    desk.undo();
    assert_eq!(desk.all(), all("start", None, false));
    assert_eq!(desk.save(), None, "Save has no file to write to: it asks");
    assert_eq!(held(&second), "second");
    desk.redo();
    assert_eq!(desk.stands(), (named("second"), false));

    // And one that has been worked on but never saved.
    let mut desk = Desk::new("no_file_edited");
    let second = desk.file_of("second");
    desk.edit("start, edited");
    desk.open(&second);
    desk.undo();
    assert_eq!(desk.all(), all("start, edited", None, true));
    assert_eq!(desk.save(), None);
    assert_eq!(held(&second), "second");
}

#[test]
fn edits_either_side_of_the_open_never_send_a_save_to_the_opened_file() {
    // Open, an edit, Undo twice (past the Open), Save.
    let mut desk = Desk::with_open("edits_about", "first");
    let second = desk.file_of("second");
    desk.open(&second);
    desk.edit("second, edited");
    desk.undo();
    desk.undo();
    assert_eq!(desk.all(), all("first", named("first"), false));
    assert_eq!(desk.save(), Some(desk.at("first")));
    assert_eq!((held(&desk.at("first")), held(&second)), ("first".to_owned(), "second".to_owned()));
    // Forward again: the opened scene with its own file. It was left with an
    // edit undone, which counts as unsaved (see `Running::unsaved`).
    desk.redo();
    assert_eq!(desk.all(), all("second", named("second"), true));
    desk.redo();
    assert_eq!(desk.all(), all("second, edited", named("second"), true));

    // Open, Undo, an edit (Redo of the Open is gone), Save.
    let mut desk = Desk::with_open("edits_about_after", "first");
    let second = desk.file_of("second");
    desk.open(&second);
    desk.undo();
    desk.edit("first, edited");
    assert!(!desk.history.can_redo());
    assert_eq!(desk.stands(), (named("first"), true));
    assert_eq!(desk.save(), Some(desk.at("first")));
    assert_eq!((held(&desk.at("first")), held(&second)), ("first, edited".to_owned(), "second".to_owned()));
}

/// Open, Save As, Undo past the Open. The new name is the opened scene's:
/// the scene from before keeps the file it had, and Redo goes back to the
/// file the opened scene was saved as.
#[test]
fn a_save_as_after_an_open_belongs_to_the_opened_scene() {
    let mut desk = Desk::with_open("save_as_between", "first");
    let second = desk.file_of("second");
    let third = desk.at("third");
    desk.open(&second);
    desk.save_as(&third);
    assert_eq!(desk.stands(), (named("third"), false));
    desk.undo();
    assert_eq!(desk.all(), all("first", named("first"), false));
    assert_eq!(desk.save(), Some(desk.at("first")));
    let files = [held(&desk.at("first")), held(&second), held(&third)];
    assert_eq!(files, ["first", "second", "second"], "each file holds what it was given");
    desk.redo();
    assert_eq!(desk.all(), all("second", named("third"), false));
}

/// The opened scene saved over the first scene's own file. Undo brings the
/// first scene back with that file, which no longer holds it: unsaved work.
#[test]
fn a_scene_whose_file_was_written_over_is_unsaved_when_undo_brings_it_back() {
    let mut desk = Desk::with_open("saved_over", "first");
    let (first, second) = (desk.at("first"), desk.file_of("second"));
    desk.open(&second);
    desk.save_as(&first);
    assert_eq!(held(&first), "second");
    desk.undo();
    assert_eq!(desk.all(), all("first", named("first"), true));
    assert_eq!(desk.save(), Some(first.clone()));
    assert_eq!(held(&first), "first");
    // And now it is the opened scene that is in no file under that name.
    desk.redo();
    assert_eq!(desk.all(), all("second", named("first"), true));

    // The same when the file is named in other letters, as Windows allows.
    if cfg!(windows) {
        let mut desk = Desk::with_open("saved_over_in_capitals", "first");
        let second = desk.file_of("second");
        desk.open(&second);
        let capitals = desk.folder.join("FIRST.SCLP");
        desk.save_as(&capitals);
        desk.undo();
        assert!(desk.unsaved(), "FIRST.SCLP is first.sclp");
    }
}

#[test]
fn a_restored_recovery_copy_has_no_file_and_undo_gives_the_scene_before_it_its_file_back() {
    let mut desk = Desk::with_open("restored", "first");
    let copy = desk.file_of("copy");
    desk.restore(&copy);
    assert_eq!(desk.all(), all("copy", None, true));
    assert_eq!(desk.save(), None, "a restored copy is never written anywhere unasked");
    desk.undo();
    assert_eq!(desk.all(), all("first", named("first"), false));
    desk.redo();
    assert_eq!(desk.all(), all("copy", None, true));
    assert_eq!(held(&copy), "copy");
}

/// A review point about restoring: a fault after the copy had begun to take
/// the scene's place left it under the name of the file the app was started
/// with, and Ctrl + S wrote it over that file unasked.
#[test]
fn a_restore_that_broke_off_half_way_leaves_a_scene_with_no_file_that_counts_as_unsaved() {
    for half in [false, true] {
        let mut desk = Desk::with_open(if half { "broke_early" } else { "broke_late" }, "first");
        let copy = desk.file_of("copy");
        if half {
            // The fault struck with the objects changed and nothing else done.
            desk.scene.objects[0].name = "half a copy".into();
        } else {
            // Or with everything done, in what follows.
            desk.restore(&copy);
            assert!(desk.restored.is_some());
        }
        desk.file().broke();
        assert_eq!(desk.stands(), (None, true), "no file, and unsaved, so closing asks");
        assert_eq!(desk.save(), None, "Save has no file to write to: it asks where");
        assert_eq!(held(&desk.at("first")), "first", "the file the scene before it came from is as it was");
        assert_eq!(desk.restored, None, "and it is nobody's restored copy: saving it retires none");
    }
}

/// A scene restored from a recovery copy carries where it came from, and
/// Undo and Redo move that with the scene, as they move its file.
#[test]
fn where_a_scene_was_restored_from_crosses_undo_and_redo_with_it_and_a_save_takes_it() {
    let mut desk = Desk::with_open("restored_from", "first");
    let (copy, second) = (desk.file_of("copy"), desk.file_of("second"));
    let from = Some(Restored { run: desk.folder.clone(), document: None });
    desk.restore(&copy);
    assert_eq!(desk.restored, from);
    desk.undo();
    assert_eq!((desk.shown(), &desk.restored), ("first", &None), "the scene from before is not the restored copy");
    let now = desk.mark();
    let path = desk.at("first");
    assert_eq!(desk.file().written(&path, now), None, "saving it is not a save of the restored scene");
    desk.redo();
    assert_eq!((desk.shown(), &desk.restored), ("copy", &from));
    // An Open over the restored scene puts it aside, note and all.
    desk.open(&second);
    assert_eq!(desk.restored, None);
    desk.undo();
    assert_eq!(desk.restored, from);
    let (now, kept) = (desk.mark(), desk.at("kept"));
    assert_eq!(desk.file().written(&kept, now), from, "the save of that scene gives the note up");
    assert_eq!(desk.restored, None, "once: the copies it names are retired by that save");
}

#[test]
fn each_open_in_a_row_keeps_its_own_file() {
    let mut desk = Desk::with_open("in_a_row", "first");
    let (second, third) = (desk.file_of("second"), desk.file_of("third"));
    desk.open(&second);
    desk.open(&third);
    for name in ["second", "first"] {
        desk.undo();
        assert_eq!(desk.all(), all(name, named(name), false));
    }
    // Back to the scene the app started with, which has no file.
    desk.undo();
    assert_eq!(desk.all(), all("start", None, false));
    for name in ["first", "second", "third"] {
        desk.redo();
        assert_eq!(desk.all(), all(name, named(name), false));
    }
}

/// What `Running::unsaved` says of itself, for steps that are not an Open.
#[test]
fn an_ordinary_step_leaves_the_file_alone_and_errs_on_the_side_of_unsaved() {
    let mut desk = Desk::with_open("ordinary", "first");
    desk.edit("first, edited");
    assert_eq!(desk.stands(), (named("first"), true));
    assert_eq!(desk.save(), Some(desk.at("first")));
    // Undo then Redo back to the save: nothing unsaved.
    desk.undo();
    assert_eq!(desk.stands(), (named("first"), true));
    desk.redo();
    assert_eq!(desk.stands(), (named("first"), false));
    // An edit after the save, undone: the scene is what the file holds, and
    // still counts as unsaved.
    desk.edit("first, edited twice");
    desk.undo();
    assert_eq!(desk.all(), all("first, edited", named("first"), true));
}

#[test]
fn once_the_open_is_too_old_to_undo_the_file_stays_the_opened_one() {
    let mut desk = Desk::with_open("too_old", "first");
    let second = desk.file_of("second");
    desk.history.max_steps = 2;
    desk.open(&second);
    desk.edit("second, edited");
    desk.edit("second, edited twice");
    while desk.history.can_undo() {
        desk.undo();
    }
    assert_eq!(desk.all(), all("second", named("second"), true));
    assert_eq!(desk.save(), Some(second.clone()));
    assert_eq!((held(&desk.at("first")), held(&second)), ("first".to_owned(), "second".to_owned()));
}

/// New Scene on a saved sculpt: one undo step, a scene with no file, and the
/// sculpt's file safe from it.
#[test]
fn a_new_scene_is_one_undo_step_and_undo_gives_the_scene_before_it_its_file_back() {
    let mut desk = Desk::with_open("new_scene", "first");
    let steps = desk.history.steps();
    desk.new_scene();
    assert_eq!(desk.history.steps(), (steps.0 + 1, 0), "one undo step");
    assert_eq!(desk.all(), all("Sphere", None, false), "the new scene: no file, nothing unsaved");
    assert_eq!(desk.scene.objects.len(), 1);

    desk.undo();
    assert_eq!(desk.all(), all("first", named("first"), false), "the sculpt is back: its own file, nothing unsaved");
    assert_eq!(desk.history.steps(), (steps.0, 1));
    desk.redo();
    assert_eq!(desk.all(), all("Sphere", None, false));
    desk.undo();
    assert_eq!(desk.all(), all("first", named("first"), false));
    assert_eq!(held(&desk.at("first")), "first", "and nothing was ever written to its file");
}

#[test]
fn save_in_a_new_scene_asks_where_and_never_goes_to_the_file_of_the_scene_it_replaced() {
    let mut desk = Desk::with_open("new_scene_save", "first");
    desk.new_scene();
    assert_eq!(desk.save(), None, "Save has no file to write to: it asks");
    desk.edit("a new piece");
    assert_eq!(desk.stands(), (None, true));
    assert_eq!(desk.save(), None, "nor once the new scene has work in it");
    // Not after the H and X keys either, which change what a save would write.
    desk.mirror_key();
    desk.hide_key();
    assert_eq!(desk.save(), None);
    assert_eq!(held(&desk.at("first")), "first", "the sculpt it replaced is as it was on disk");
    assert_eq!(held_kept(&desk.at("first")), (true, false));

    // Saved under a name of its own, that name is the new scene's and no one else's.
    let second = desk.at("second");
    desk.save_as(&second);
    assert_eq!(desk.stands(), (named("second"), false));
    desk.undo();
    desk.undo();
    assert_eq!(desk.all(), all("first", named("first"), false));
    assert_eq!(desk.save(), Some(desk.at("first")));
    assert_eq!((held(&desk.at("first")), held(&second)), ("first".to_owned(), "a new piece".to_owned()));
}

/// Closing straight after New Scene asks nothing, and no recovery copy is
/// written for a scene nothing has been done to.
#[test]
fn an_untouched_new_scene_has_nothing_unsaved_and_is_owed_no_recovery_copy() {
    let mut desk = Desk::with_open("new_scene_untouched", "first");
    desk.edit("first, edited");
    assert!(desk.unsaved() && desk.owed_a_copy(), "the sculpt has unsaved work, which is owed a copy");
    // New Scene Without Saving.
    desk.new_scene();
    assert!(!desk.unsaved(), "nothing in the new scene is unsaved");
    assert!(!desk.owed_a_copy(), "and it is owed no recovery copy");
    // The X key and back again: still nothing.
    desk.mirror_key();
    assert!(desk.unsaved() && desk.owed_a_copy(), "Mirror X is something to save, in a new scene as in any");
    desk.mirror_key();
    assert!(!desk.unsaved() && !desk.owed_a_copy());
    // The work that was put aside is unsaved again the moment Undo brings it back.
    desk.undo();
    assert_eq!(desk.all(), all("first, edited", named("first"), true));
    assert!(desk.owed_a_copy());
}

#[test]
fn a_new_scene_after_a_new_scene_is_a_step_of_its_own() {
    let mut desk = Desk::with_open("new_scene_twice", "first");
    desk.new_scene();
    desk.new_scene();
    assert_eq!(desk.history.steps().0, 3, "the Open and two new scenes");
    assert_eq!(desk.all(), all("Sphere", None, false));
    desk.edit("second new scene, edited");
    desk.new_scene();
    assert_eq!(desk.all(), all("Sphere", None, false), "a third, over unsaved work that is now one Undo away");
    desk.undo();
    assert_eq!(desk.all(), all("second new scene, edited", None, true));
    desk.undo();
    desk.undo();
    assert_eq!(desk.all(), all("Sphere", None, false), "the first new scene, untouched");
    assert_eq!(desk.save(), None);
    desk.undo();
    assert_eq!(desk.all(), all("first", named("first"), false));
    for _ in 0..4 {
        desk.redo();
    }
    assert_eq!(desk.all(), all("Sphere", None, false), "the third again, as it was left");
    assert_eq!(held(&desk.at("first")), "first");
}

/// New Scene, an edit, Undo twice (past the New Scene), Save.
#[test]
fn new_scene_then_an_edit_then_undo_twice_and_save_writes_the_old_scene_to_its_own_file() {
    let mut desk = Desk::with_open("new_scene_edit_undo", "first");
    desk.edit("first, edited");
    // New Scene Without Saving, then work in the new scene.
    desk.new_scene();
    desk.edit("a new piece");
    desk.undo();
    desk.undo();
    assert_eq!(desk.all(), all("first, edited", named("first"), true));
    assert_eq!(desk.save(), Some(desk.at("first")), "Save writes to the old scene's own file");
    assert_eq!(held(&desk.at("first")), "first, edited", "and what it wrote is the old scene, not the new one");
    assert!(!desk.unsaved());
    // Forward again: the new scene, with no file whatever was saved meanwhile.
    desk.redo();
    assert_eq!(desk.stands().0, None);
    assert_eq!(desk.save(), None);
    desk.redo();
    assert_eq!(desk.all(), all("a new piece", None, true));
    assert_eq!(desk.save(), None);
    assert_eq!(held(&desk.at("first")), "first, edited");
}

/// A scene that never had a file, a new scene, and back.
#[test]
fn a_new_scene_over_a_scene_with_no_file_leaves_both_without_one() {
    let mut desk = Desk::new("new_scene_no_file");
    desk.edit("start, edited");
    desk.new_scene();
    assert_eq!(desk.all(), all("Sphere", None, false));
    desk.undo();
    assert_eq!(desk.all(), all("start, edited", None, true));
    assert_eq!(desk.save(), None);
    // And over a restored recovery copy, which is unsaved work with no file.
    let copy = desk.file_of("copy");
    desk.restore(&copy);
    desk.new_scene();
    assert_eq!(desk.all(), all("Sphere", None, false));
    desk.undo();
    assert_eq!(desk.all(), all("copy", None, true), "the restored copy is still unsaved when Undo brings it back");
}

/// Where the two meet: New Scene over a scene that was restored from a
/// recovery copy and has not been saved. The new scene is nobody's restored
/// copy, so saving it retires no crashed run's copies; the restored scene is
/// one Undo away with its note, and only its own save gives the note up.
#[test]
fn a_new_scene_over_a_restored_scene_carries_no_note_and_undo_gives_the_note_back() {
    let mut desk = Desk::with_open("new_scene_over_restored", "first");
    let copy = desk.file_of("copy");
    let note = Some(from(&copy));
    desk.restore(&copy);
    assert_eq!((desk.all(), &desk.restored), (all("copy", None, true), &note));

    // New Scene Without Saving.
    desk.new_scene();
    assert_eq!(desk.all(), all("Sphere", None, false), "clean, and in no file");
    assert_eq!(desk.restored, None, "the new scene is not the crashed work");
    assert_eq!(desk.save(), None, "Save asks where: it has no file");
    let elsewhere = desk.at("elsewhere");
    assert_eq!(desk.save_as(&elsewhere), None, "and saving it gives up no note: the crashed run's copies stay");
    assert_eq!(desk.all(), all("Sphere", named("elsewhere"), false));

    // Undo: the restored scene, with no file, unsaved, and its note.
    desk.undo();
    assert_eq!((desk.all(), &desk.restored), (all("copy", None, true), &note));
    desk.redo();
    assert_eq!((desk.all(), &desk.restored), (all("Sphere", named("elsewhere"), false), &None));
    desk.undo();
    // Past the restore as well, and forward again: the note is on the restored scene alone.
    desk.undo();
    assert_eq!((desk.all(), &desk.restored), (all("first", named("first"), false), &None));
    desk.redo();
    assert_eq!((desk.shown(), &desk.restored), ("copy", &note));
    let kept = desk.at("kept");
    assert_eq!(desk.save_as(&kept), note, "the save of the restored scene is the one that gives the note up");
    assert_eq!(desk.restored, None);
    // Saved, it is an ordinary scene: a new scene over it and back brings no note with it.
    desk.new_scene();
    desk.undo();
    assert_eq!((desk.all(), &desk.restored), (all("copy", named("kept"), false), &None));
}

/// The H and X keys change what a save writes without being undo steps. A
/// scene they have changed is unsaved until it is saved, or until the change
/// is taken back by the same key.
#[test]
fn a_change_that_is_saved_but_is_no_undo_step_is_unsaved_until_saved_or_put_back() {
    for (name, key) in [("hide", Desk::hide_key as fn(&mut Desk)), ("mirror", Desk::mirror_key)] {
        let mut desk = Desk::with_open(&format!("kept_{name}"), "first");
        let first = desk.at("first");
        assert_eq!(desk.stands(), (named("first"), false));
        key(&mut desk);
        assert!(!desk.history.can_undo() || desk.history.steps().0 == 1, "{name}: no undo step was recorded");
        assert_eq!(desk.stands(), (named("first"), true), "{name}: the file does not hold it yet");
        assert_ne!(held_kept(&first), desk.kept());
        // Put back by the same key: the scene is what the file holds again.
        key(&mut desk);
        assert_eq!(desk.stands(), (named("first"), false), "{name}: taken back, nothing is unsaved");
        // Changed and saved: the file holds it.
        key(&mut desk);
        assert_eq!(desk.save(), Some(first.clone()));
        assert_eq!(desk.stands(), (named("first"), false), "{name}: Save makes it clean");
        assert_eq!(held_kept(&first), desk.kept());
    }
}

/// The same across an Open: the scene that Undo brings back is unsaved
/// exactly if the H or X key had changed it since it was saved.
#[test]
fn such_a_change_is_still_unsaved_after_an_open_and_its_undo() {
    let mut desk = Desk::with_open("kept_across_an_open", "first");
    let second = desk.file_of("second");
    desk.mirror_key();
    // Open Without Saving.
    desk.open(&second);
    assert_eq!(desk.all(), all("second", named("second"), false));
    assert_eq!(desk.kept(), (true, false), "the opened ball has the symmetry its file kept: none");
    desk.undo();
    assert_eq!(desk.all(), all("first", named("first"), true));
    assert_eq!(desk.kept(), (true, true), "the first ball is back with Mirror X, which is in no file");
    assert_eq!(desk.save(), Some(desk.at("first")));
    assert_eq!(held_kept(&desk.at("first")), (true, true));
    assert!(!desk.unsaved());

    // The opened scene changed the same way and left by Undo: Redo finds it unsaved.
    desk.redo();
    desk.hide_key();
    desk.undo();
    assert_eq!(desk.all(), all("first", named("first"), false));
    desk.redo();
    assert_eq!(desk.all(), all("second", named("second"), true));
    assert_eq!(desk.kept(), (false, false), "hidden, as it was left");
}

/// A long walk of opens, restored recovery copies, new scenes, edits, saves,
/// undos and redos, and of the H and X keys (which change what a save writes
/// without being undo steps), checked at every step against things that must
/// never fail: where the app says there is nothing unsaved, the scene's file
/// really holds the scene on screen, name, visibility and symmetry; the
/// scene's file is the one it was opened from or last saved as, worked out
/// here a second way (a list of files beside the steps); the recovery copy
/// the scene was restored from is its own and no other scene's, worked out
/// the same second way, and a save gives up that note and no other; a scene
/// with no file that counts as saved is a starting scene nothing has been
/// done to; and Save writes to the scene's own file or to none, leaving
/// every other file as it was.
#[test]
fn a_long_walk_never_calls_a_scene_saved_that_is_not_in_its_file() {
    let mut desk = Desk::new("long_walk");
    let names = ["a", "b", "c"];
    for name in names {
        desk.file_of(name);
    }
    // For every step undo and redo hold, whether it changed the scene's file;
    // and the file of each stretch of the history between two such steps.
    let (mut undo, mut redo): (Vec<bool>, Vec<bool>) = (Vec::new(), Vec::new());
    let mut files: Vec<Option<PathBuf>> = vec![None];
    // And beside each of those, the recovery copy that stretch's scene was
    // restored from, until a save of it gives the note up.
    let mut notes: Vec<Option<Restored>> = vec![None];
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = |below: u64| {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (seed >> 33) % below
    };
    let (mut crossed, mut keys, mut clean_after_a_key, mut new_scenes, mut untouched) = (0, 0, 0, 0, 0);
    let (mut restores, mut noted, mut given_up, mut new_over_restored) = (0, 0, 0, 0);
    for turn in 0..900 {
        let stretch = undo.iter().filter(|&&open| open).count();
        let file = desk.at(names[next(3) as usize]);
        let key = matches!(next(4), 0).then(|| next(2));
        if let Some(which) = key {
            // Neither is an undo step: the history, and the files beside it, stand as they are.
            if which == 0 { desk.hide_key() } else { desk.mirror_key() }
            keys += 1;
            clean_after_a_key += !desk.unsaved() as usize;
        }
        match next(10) {
            _ if key.is_some() => {}
            0 | 1 => {
                // Redo ends here, and the files of everything it led to with it.
                desk.edit(&format!("edit {turn}"));
                undo.push(false);
                redo.clear();
                files.truncate(stretch + 1);
                notes.truncate(stretch + 1);
            }
            2 => {
                // An Open, a restored recovery copy or a new scene: the last two have no file.
                let kind = next(5);
                new_over_restored += (kind == 1 && desk.restored.is_some()) as usize;
                match kind {
                    0 => desk.restore(&file),
                    1 => desk.new_scene(),
                    _ => desk.open(&file),
                }
                new_scenes += (kind == 1) as usize;
                restores += (kind == 0) as usize;
                assert!(kind != 1 || !desk.unsaved(), "turn {turn}: a new scene with something unsaved in it");
                assert!(kind != 0 || desk.unsaved(), "turn {turn}: a restored copy that counts as saved");
                undo.push(true);
                redo.clear();
                files.truncate(stretch + 1);
                notes.truncate(stretch + 1);
                notes.push((kind == 0).then(|| from(&file)));
                files.push((kind > 1).then_some(file));
            }
            3 => {
                // Save writes to the scene's own file or asks; either way no other file changes.
                let before = names.map(|name| held(&desk.at(name)));
                let wrote = desk.save();
                assert_eq!(wrote, files[stretch], "turn {turn}: Save went somewhere else than the scene's file");
                // A restored scene has no file until Save As gives it one, which takes its note.
                assert!(wrote.is_none() || notes[stretch].is_none(), "turn {turn}: a restored scene had a file");
                for (name, was) in names.into_iter().zip(before) {
                    let path = desk.at(name);
                    let written = wrote.as_deref() == Some(path.as_path());
                    let now = held(&path);
                    assert!(written || now == was, "turn {turn}: Save wrote to {name}.sclp, which is not its file");
                    assert!(!written || now == desk.shown(), "turn {turn}: {name}.sclp does not hold what was saved");
                }
            }
            4 => {
                // Save As: the save of this scene, which gives up its note and no other scene's.
                let gave = desk.save_as(&file);
                assert_eq!(gave, notes[stretch].take(), "turn {turn}: the save gave up another note than its scene's");
                given_up += gave.is_some() as usize;
                files[stretch] = Some(file);
            }
            5..=7 if desk.history.can_undo() => {
                desk.undo();
                let open = undo.pop().expect("a step to undo");
                redo.push(open);
                crossed += open as usize;
            }
            _ if desk.history.can_redo() => {
                desk.redo();
                let open = redo.pop().expect("a step to redo");
                undo.push(open);
                crossed += open as usize;
            }
            _ => {}
        }
        let stretch = undo.iter().filter(|&&open| open).count();
        assert_eq!(desk.path, files[stretch], "turn {turn}: the scene's file is not its own");
        assert_eq!(desk.restored, notes[stretch], "turn {turn}: where the scene was restored from is not its own");
        noted += desk.restored.is_some() as usize;
        if let Some(path) = desk.path.clone().filter(|_| !desk.unsaved()) {
            assert_eq!(held(&path), desk.shown(), "turn {turn}: nothing unsaved, but the file holds another scene");
            let (in_file, on_screen) = (held_kept(&path), desk.kept());
            assert_eq!(in_file, on_screen, "turn {turn}: nothing unsaved, but the file shows and mirrors otherwise");
        }
        // With no file, only a scene nothing has been done to has nothing to save.
        if desk.path.is_none() && !desk.unsaved() {
            let starting = ["start", "Sphere"].contains(&desk.shown()) && desk.kept() == (true, false);
            assert!(starting, "turn {turn}: \"{}\" has no file and counts as saved", desk.shown());
            assert_eq!(desk.save(), None, "turn {turn}: Save of a scene with no file wrote somewhere");
            untouched += 1;
        }
    }
    assert!(new_scenes > 10, "the walk started a new scene only {new_scenes} times");
    assert!(restores > 10, "the walk restored a copy only {restores} times");
    assert!(noted > 30, "the walk stood on a restored scene that was not saved only {noted} times");
    assert!(given_up > 3, "a save gave up a restored scene's note only {given_up} times");
    assert!(new_over_restored > 0, "the walk never started a new scene over a restored one");
    assert!(untouched > 10, "the walk stood on an untouched scene with no file only {untouched} times");
    assert!(crossed > 20, "the walk crossed an Open only {crossed} times");
    assert!(keys > 100, "the walk pressed H or X only {keys} times");
    assert!(clean_after_a_key > 5, "a key put a saved scene back as its file holds it only {clean_after_a_key} times");
}
