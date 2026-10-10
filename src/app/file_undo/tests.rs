//! Open, Undo, Redo and Save against real files, with the app's own history
//! and the code that moves the scene's file across an Open (`File`). The
//! `Desk` here does in a few lines each what `Running` does around them, so
//! the cases read as a person at the app: open this, press Z, press Ctrl + S,
//! then look at what is in the files.

use std::path::{Path, PathBuf};

use glam::{Quat, Vec3};

use super::File;
use crate::app::files::Mark;
use crate::app::install;
use crate::history::History;
use crate::io;
use crate::mesh::Mesh;
use crate::mesh::primitives::{self, Primitive};
use crate::scene::Scene;

/// A scene with one object, told from another by that object's name.
struct Desk {
    scene: Scene,
    mesh: Mesh,
    history: History,
    path: Option<PathBuf>,
    clean: Option<Mark>,
    folder: PathBuf,
}

fn ball() -> Mesh {
    primitives::build(Primitive::Sphere, 80, 0.5)
}

/// Writes a sculpt of one ball called `name`.
fn write(path: &Path, name: &str) {
    let (position, rotation, scale) = (Vec3::ZERO, Quat::IDENTITY, Vec3::ONE);
    let object = io::SavedObject { name: name.into(), visible: true, mesh: 0, position, rotation, scale };
    io::save_scene_with(&[&ball()], &[object], 0, &io::Details::default(), path).expect("the file writes");
}

/// The name of the scene a file holds.
fn held(path: &Path) -> String {
    io::load_scene(path).expect("the file reads").objects[0].name.clone()
}

impl Desk {
    /// The app as it starts: a scene with no file and nothing unsaved.
    fn new(test: &str) -> Desk {
        let folder = std::env::temp_dir().join("sculpt_file_undo_tests").join(test);
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).expect("a folder for the test");
        let (scene, mesh, history) = (Scene::new("start"), ball(), History::default());
        Desk { scene, mesh, history, path: None, clean: Some((0, 0, 0)), folder }
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
        let (undo, redo) = self.history.steps();
        (self.history.edits, undo, redo)
    }

    fn file(&mut self) -> File<'_> {
        File { path: &mut self.path, clean: &mut self.clean, history: &mut self.history }
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

    /// As `Running::replace_scene`.
    fn replace(&mut self, path: &Path, document: bool) {
        let saved = io::load_scene(path).expect("the file reads");
        let now = self.mark();
        let before = self.file().state(now);
        let state = self.scene.state();
        install(&mut self.scene, &mut self.mesh, saved);
        self.history.push_scene(state);
        self.history.keep_file(before);
        let now = self.mark();
        self.file().became(document.then(|| path.to_owned()), now);
    }

    /// Open, in place of the scene.
    fn open(&mut self, path: &Path) {
        self.replace(path, true);
    }

    /// A recovery copy restored.
    fn restore(&mut self, path: &Path) {
        self.replace(path, false);
    }

    /// As `Running::save_scene` and the line after it in `save_to`.
    fn save_as(&mut self, path: &Path) {
        write(path, &self.shown().to_owned());
        let now = self.mark();
        self.file().written(path, now);
        self.path = Some(path.to_owned());
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

/// A long walk of opens, edits, saves, undos and redos, checked at every
/// step against two things that must never fail: where the app says there is
/// nothing unsaved, the scene's file really holds the scene on screen; and
/// the scene's file is the one it was opened from or last saved as, worked
/// out here a second way (a list of files beside the steps).
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
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = |below: u64| {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (seed >> 33) % below
    };
    let mut crossed = 0;
    for turn in 0..600 {
        let stretch = undo.iter().filter(|&&open| open).count();
        let file = desk.at(names[next(3) as usize]);
        match next(10) {
            0 | 1 => {
                // Redo ends here, and the files of everything it led to with it.
                desk.edit(&format!("edit {turn}"));
                undo.push(false);
                redo.clear();
                files.truncate(stretch + 1);
            }
            2 => {
                let recovery = next(4) == 0;
                desk.replace(&file, !recovery);
                undo.push(true);
                redo.clear();
                files.truncate(stretch + 1);
                files.push((!recovery).then_some(file));
            }
            3 => {
                desk.save();
            }
            4 => {
                desk.save_as(&file);
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
        if let Some(path) = desk.path.clone().filter(|_| !desk.unsaved()) {
            assert_eq!(held(&path), desk.shown(), "turn {turn}: nothing unsaved, but the file holds another scene");
        }
    }
    assert!(crossed > 20, "the walk crossed an Open only {crossed} times");
}
