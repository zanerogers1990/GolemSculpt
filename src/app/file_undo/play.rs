//! A check of what Undo and Redo do to the scene's file when they cross an
//! Open: worked in the real app, with real files, and printed a line per
//! check as `--demo-files play` is.
//!
//! ```text
//! sculpt.exe --screenshot out.png --demo --demo-files open-undo      # from a scene that was never saved
//! sculpt.exe --screenshot out.png --demo-files open-undo some.sclp   # from a file named on the command line
//! sculpt.exe --screenshot out.png --demo --demo-files open-undo:opened   # the File card after an Open
//! sculpt.exe --screenshot out.png --demo --demo-files open-undo:undone   # and after Undo of it
//! ```
//!
//! The file named on the command line may be a `.sculpt` from the Unity app,
//! which adds a case of its own. Every case ends by looking at what is in
//! the files: the bug this guards against was the old scene saved over the
//! file that had just been opened.
//!
//! Nothing here asks the user anything: Save is taken as far as the box it
//! would put up, and no further (`save_target`).

use std::path::{Path, PathBuf};

use glam::Vec3;

use crate::app::Running;
use crate::doc::{Action, OpenHow};
use crate::mesh::Mesh;
use crate::mesh::primitives::Primitive;
use crate::{io, prefs, recovery, unity_import};

fn say(what: &str, ok: bool) {
    println!("{} {what}", if ok { "ok  " } else { "FAIL" });
}

/// The fingerprint of the scene a file holds; 0 for a file that does not read.
fn print_of(path: &Path) -> u64 {
    let Ok(saved) = io::load_scene(path) else { return 0 };
    recovery::fingerprint(&saved.meshes.iter().collect::<Vec<&Mesh>>(), &saved.objects)
}

/// Writes the scene of `from` to `to` under another name and stretched tall:
/// a second sculpt that is plainly not the first, on screen and by its
/// fingerprint.
fn stretched(from: &Path, to: &Path) -> io::Result<()> {
    let mut saved = io::load_scene(from)?;
    for object in &mut saved.objects {
        object.name = format!("Tall {}", object.name);
        object.scale *= Vec3::new(0.7, 1.5, 0.7);
    }
    let meshes: Vec<&Mesh> = saved.meshes.iter().collect();
    io::save_scene_with(&meshes, &saved.objects, saved.active, &saved.details, to).map(|_| ())
}

/// The files of the check. `first` and `second` are put back as they were at
/// the start of every case; `third` is where Save As goes.
struct Files {
    first: PathBuf,
    second: PathBuf,
    third: PathBuf,
    model: PathBuf,
    /// The untouched originals of `first` and `second`.
    kept: (PathBuf, PathBuf),
    /// The fingerprints of the first scene and the second.
    prints: (u64, u64),
}

impl Running {
    /// The fingerprint of the scene on screen.
    fn print_now(&self) -> u64 {
        self.scene_for_file().map_or(0, |(meshes, objects, _)| recovery::fingerprint(&meshes, &objects))
    }

    /// Writes the scene to `path` without that being a save: the scene's
    /// file, and whether it has unsaved changes, stay as they are.
    fn write_aside(&self, path: &Path) -> io::Result<()> {
        let (meshes, objects, active) = self.scene_for_file()?;
        io::save_scene_with(&meshes, &objects, active, &self.details(), path).map(|_| ())
    }

    /// Save (Ctrl + S), short of the box it puts up for a scene with no file
    /// of its own: gives back the file it wrote, or `None` where it would ask.
    fn save_unasked(&mut self) -> Option<PathBuf> {
        let path = self.save_target(false)?;
        self.save_to(path.clone()).then_some(path)
    }

    fn is_file(&self, path: &Path) -> bool {
        self.doc.path.as_deref() == Some(path)
    }

    /// Something done to the scene: a cone added.
    fn edit(&mut self) {
        self.act(Action::New(Primitive::Cone));
    }

    /// The start of a case: the two files as they first were, and the first
    /// of them open, saved and unchanged.
    fn first_open(&mut self, files: &Files) {
        let copied = std::fs::copy(&files.kept.0, &files.first).and(std::fs::copy(&files.kept.1, &files.second));
        let _ = std::fs::remove_file(&files.third);
        self.load(files.first.clone(), true);
        let open = self.is_file(&files.first) && !self.unsaved() && self.print_now() == files.prints.0;
        if copied.is_err() || !open {
            say("(the case could not be set up)", false);
        }
    }

    /// Whether the two files hold these scenes.
    fn files_hold(files: &Files, first: u64, second: u64) -> bool {
        print_of(&files.first) == first && print_of(&files.second) == second
    }

    /// The whole check, or (`open-undo:opened`, `open-undo:undone`) only as
    /// far as an Open, or an Open and its Undo, for a picture of the File card.
    pub(in crate::app) fn open_undo_play(&mut self, folder: &Path, what: &str) {
        let named = |name: &str| folder.join(name);
        let kept = (named("first kept.sclp"), named("second kept.sclp"));
        // The scene the run started with is the first sculpt; the second is made from it.
        let start = (self.doc.path.clone(), self.unsaved(), self.print_now());
        let made = self.write_aside(&kept.0).and_then(|()| stretched(&kept.0, &kept.1));
        let files = Files {
            first: named("first sculpt.sclp"),
            second: named("second sculpt.sclp"),
            third: named("saved as.sclp"),
            model: named("model.obj"),
            prints: (print_of(&kept.0), print_of(&kept.1)),
            kept,
        };
        let (first, second) = files.prints;
        say("two different sculpts are on disk", made.is_ok() && first == start.2 && second != first && second != 0);
        if what != "open-undo" {
            // The Objects card shut, so the File card under it is in view.
            let shut = std::collections::BTreeMap::from([("Objects".to_owned(), false)]);
            prefs::cards::restore(&self.egui_ctx, &shut);
            self.first_open(&files);
            self.load(files.second.clone(), true);
            if what == "open-undo:undone" {
                self.act(Action::Undo);
            }
            return;
        }
        self.from_the_start(&files, &start);
        self.from_a_saved_scene(&files);
        self.from_unsaved_changes(&files);
        self.with_edits_about(&files);
        self.with_a_save_as_between(&files);
        self.adding_leaves_the_file_alone(&files);
        if let Some(sculpt) = start.0.clone().filter(|path| unity_import::is_unity(path)) {
            self.over_a_unity_sculpt(&files, &sculpt);
        }
    }

    /// The scene the run started with: never saved (with `--demo`, its
    /// strokes unsaved), or a file named on the command line.
    fn from_the_start(&mut self, files: &Files, start: &(Option<PathBuf>, bool, u64)) {
        let (file, unsaved, scene) = start;
        match file {
            Some(file) => println!("     the run started with {}, unsaved {unsaved}", file.display()),
            None => println!("     the run started with a scene that has no file, unsaved {unsaved}"),
        }
        let copied = std::fs::copy(&files.kept.1, &files.second);
        self.load(files.second.clone(), true);
        let opened = self.is_file(&files.second) && !self.unsaved() && self.print_now() == files.prints.1;
        say("an opened file is the scene's file, with nothing unsaved", copied.is_ok() && opened);
        self.act(Action::Undo);
        say("Undo of the Open brings the scene before it back", self.print_now() == *scene);
        say("with the file it had, or none", self.doc.path == *file);
        say("and unsaved exactly if it was", self.unsaved() == *unsaved);
        let target = self.save_target(false);
        let own = file.clone().filter(|path| !unity_import::is_unity(path));
        say("Save would write to its own file, or ask where; never to the opened file", target == own);
        self.act(Action::Redo);
        let again = self.is_file(&files.second) && !self.unsaved() && self.print_now() == files.prints.1;
        say("Redo: the opened file is the scene's file again, with nothing unsaved", again);
        self.act(Action::Undo);
        say("and Undo once more gives the same as before", self.doc.path == *file && self.unsaved() == *unsaved);
    }

    /// A saved scene, an Open, Undo: its own file, nothing to save.
    fn from_a_saved_scene(&mut self, files: &Files) {
        let (first, second) = files.prints;
        self.first_open(files);
        self.load(files.second.clone(), true);
        self.act(Action::Undo);
        say("a saved scene, Open, Undo: the first scene is back", self.print_now() == first);
        say("its file is its own again", self.is_file(&files.first));
        say("and it has nothing unsaved", !self.unsaved());
        let saved = self.save_unasked();
        say("Save writes it to its own file", saved.as_deref() == Some(files.first.as_path()));
        say("and each file holds its own scene", Self::files_hold(files, first, second));
        self.act(Action::Redo);
        let again = self.is_file(&files.second) && !self.unsaved() && self.print_now() == second;
        say("Redo: the opened file again, with nothing unsaved", again);

        // A recent file's tile goes the same way: it asks, and the answer opens.
        self.first_open(files);
        self.open_recent(files.second.clone());
        let asked = self.ui.graph.asking();
        self.ui.graph.answer(false);
        self.open_as(files.second.clone(), OpenHow::Replace);
        self.act(Action::Undo);
        say("a recent file's tile, Undo: the same", asked && self.is_file(&files.first) && !self.unsaved());
    }

    /// Unsaved changes, Open Without Saving, Undo: its own file, still unsaved.
    fn from_unsaved_changes(&mut self, files: &Files) {
        let second = files.prints.1;
        self.first_open(files);
        self.edit();
        let edited = self.print_now();
        self.load(files.second.clone(), true);
        say("Open Without Saving: the opened file has nothing unsaved", self.is_file(&files.second) && !self.unsaved());
        self.act(Action::Undo);
        say("unsaved changes, Open, Undo: the edited scene is back", self.print_now() == edited);
        say("its file is its own again", self.is_file(&files.first));
        say("and its changes are still unsaved", self.unsaved());
        let saved = self.save_unasked();
        say("Save writes them to its own file", saved.as_deref() == Some(files.first.as_path()));
        say("and the opened file is as it was", Self::files_hold(files, edited, second) && !self.unsaved());
    }

    /// Edits on either side of the Open and its Undo.
    fn with_edits_about(&mut self, files: &Files) {
        let (first, second) = files.prints;
        self.first_open(files);
        self.load(files.second.clone(), true);
        self.edit();
        self.act(Action::Undo);
        self.act(Action::Undo);
        let back = self.print_now() == first && self.is_file(&files.first) && !self.unsaved();
        say("Open, an edit, Undo twice: the first scene, its own file, nothing unsaved", back);
        let saved = self.save_unasked();
        let right = saved.as_deref() == Some(files.first.as_path()) && Self::files_hold(files, first, second);
        say("Save writes to its own file and the opened file is as it was", right);
        self.act(Action::Redo);
        let again = self.print_now() == second && self.is_file(&files.second);
        say("Redo: the opened file is the scene's file again", again);

        self.first_open(files);
        self.load(files.second.clone(), true);
        self.act(Action::Undo);
        self.edit();
        let edited = self.print_now();
        let back = self.is_file(&files.first) && self.unsaved() && !self.doc.history.can_redo();
        say("Open, Undo, an edit: the first scene's file, unsaved, and the Open is gone for good", back);
        let saved = self.save_unasked();
        let right = saved.as_deref() == Some(files.first.as_path()) && Self::files_hold(files, edited, second);
        say("Save writes to its own file and the opened file is as it was", right);
    }

    /// Open, Save As, then Undo past the Open. The name the opened scene was
    /// saved under belongs to that scene, not to the one Undo brings back.
    fn with_a_save_as_between(&mut self, files: &Files) {
        let (first, second) = files.prints;
        self.first_open(files);
        self.load(files.second.clone(), true);
        let saved_as = self.save_to(files.third.clone());
        self.act(Action::Undo);
        let back = saved_as && self.print_now() == first && self.is_file(&files.first) && !self.unsaved();
        say("Open, Save As, Undo: the first scene, its own file, nothing unsaved", back);
        let saved = self.save_unasked();
        let right = saved.as_deref() == Some(files.first.as_path())
            && Self::files_hold(files, first, second)
            && print_of(&files.third) == second;
        say("Save writes to its own file; the opened file and the Save As are as they were", right);
        self.act(Action::Redo);
        let again = self.print_now() == second && self.is_file(&files.third) && !self.unsaved();
        say("Redo: the file it was saved as, with nothing unsaved", again);

        // Saved over the first scene's own file: that file no longer holds
        // the first scene, so when Undo brings it back it is unsaved work.
        self.first_open(files);
        self.load(files.second.clone(), true);
        let saved_over = self.save_to(files.first.clone());
        self.act(Action::Undo);
        let back = saved_over && self.print_now() == first && self.is_file(&files.first);
        say("Open, Save As over the first scene's file, Undo: the first scene and its file", back);
        let overwritten = self.unsaved() && print_of(&files.first) == second;
        say("which now holds the other scene, so the first counts as unsaved", overwritten);
        let saved = self.save_unasked();
        say("and Save puts it back in its file", saved.is_some() && print_of(&files.first) == first && !self.unsaved());
    }

    /// Import adds to the scene: the file stays the scene's own, both ways.
    fn adding_leaves_the_file_alone(&mut self, files: &Files) {
        self.first_open(files);
        self.load(files.second.clone(), false);
        say("adding a sculpt's objects leaves the scene's file alone", self.is_file(&files.first) && self.unsaved());
        self.act(Action::Undo);
        say("and so does Undo of it", self.is_file(&files.first) && self.print_now() == files.prints.0);
        self.act(Action::Redo);
        say("and Redo", self.is_file(&files.first));
        let tetrahedron = "v 0 0 0\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 1 3 2\nf 1 2 4\nf 2 3 4\nf 1 4 3\n";
        let written = std::fs::write(&files.model, tetrahedron);
        let before = self.doc.scene.objects.len();
        self.load(files.model.clone(), false);
        let added = written.is_ok() && self.doc.scene.objects.len() == before + 1;
        say("importing a model leaves it alone too", added && self.is_file(&files.first));
        self.act(Action::Undo);
        say("and Undo of that", self.is_file(&files.first));
    }

    /// A sculpt from the Unity app opened over a saved scene. It is the
    /// scene's file while it is open (Save asks for a `.sclp`), and Undo
    /// gives the scene before it its own file back.
    fn over_a_unity_sculpt(&mut self, files: &Files, sculpt: &Path) {
        let stamp = |path: &Path| std::fs::metadata(path).ok().map(|m| (m.len(), m.modified().ok()));
        let untouched = stamp(sculpt);
        self.first_open(files);
        self.load(sculpt.to_owned(), true);
        let opened = self.is_file(sculpt) && !self.unsaved() && self.save_target(false).is_none();
        say("a Unity sculpt opened over a saved scene: Save asks where its .sclp should go", opened);
        self.act(Action::Undo);
        let back = self.is_file(&files.first) && !self.unsaved() && self.print_now() == files.prints.0;
        say("Undo: the first scene, its own file, nothing unsaved", back);
        say("and Save would write there", self.save_target(false).as_deref() == Some(files.first.as_path()));
        self.act(Action::Redo);
        let again = self.is_file(sculpt) && !self.unsaved() && self.save_target(false).is_none();
        say("Redo: the Unity sculpt again, and Save asks again", again);
        say("the Unity file was never written to", stamp(sculpt) == untouched);
    }
}
