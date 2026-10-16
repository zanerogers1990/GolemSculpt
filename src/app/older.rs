//! Older versions, on the app's side: keeping the file a save writes over
//! (`crate::backups`), the File tab's Older Versions window, which opens
//! those earlier saves and the recovery copies kept for a week
//! (`crate::recovery::kept`), and the word a save gives when it leaves out
//! an object that was deleted since the last one.
//!
//! On 2026-10-15 one click on a row's bin deleted the Door, a save at 00:11
//! wrote the scene without it over the only file that had it, and closing
//! the app took the undo history with it. Each of those three is answered
//! here or nearby: the bin asks first (`ui/scene_graph.rs`), the save keeps
//! the file it replaces and says what it leaves out, and what closing used to
//! remove stays for a week.
//!
//! Opening an older version never makes it the scene's file. Added, its
//! objects join the scene like an Import of a `.sclp`, and the scene's file
//! stays as it was: the way to get one lost object back into today's work.
//! Opened in place of the scene, it has no file at all, so Save asks where
//! to keep it (offering the file it is an older version of), and nothing is
//! ever saved into the backups folder by accident, where pruning would take
//! it. That is offered only while the scene has nothing unsaved and nothing
//! that is in no file; otherwise Add, which loses nothing, is the one way in.

use std::path::{Path, PathBuf};

use sculpt_toolkit::Loss;

use super::Running;
use crate::backups;
use crate::history::{FileState, Restored};
use crate::io;
use crate::recovery;
use crate::ui::older::{Listing, Row};
use crate::ui::question::listed;

/// An object deleted since the scene was last saved.
struct Deleted {
    id: u64,
    name: String,
    /// The scene's file when it was deleted. A save of another scene (after
    /// an Open or a New Scene) says nothing about it.
    document: Option<PathBuf>,
}

#[derive(Default)]
pub(super) struct State {
    deleted: Vec<Deleted>,
    /// The earlier save the save under way has kept, if it kept one.
    backup: Option<PathBuf>,
    /// What the last save has to say about objects it left out.
    note: String,
    /// The files the window lists, with the scene file each is a version of.
    listed: Vec<(PathBuf, Option<PathBuf>)>,
}

/// A file's name without its folders.
fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// "154.9 MB".
fn size_words(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1e6)
}

/// What a save says of the objects deleted since the one before, which it
/// leaves out of the file. `kept` is whether the file it wrote over was kept
/// as an earlier save. Empty when nothing was deleted.
fn left_out_words(names: &[String], kept: bool) -> String {
    if names.is_empty() {
        return String::new();
    }
    let quoted: Vec<String> = names.iter().map(|name| format!("\"{name}\"")).collect();
    let (were, them) = if names.len() == 1 { ("was", "it") } else { ("were", "them") };
    let back = if kept {
        format!("The save before, which has {them}, is kept: File tab, Older Versions, Add Objects.")
    } else {
        format!("Undo (Z) brings {them} back, but only until GolemSculpt is closed.")
    };
    format!(". {} {were} deleted and {were} not saved. {back}", listed(&quoted))
}

impl Running {
    // ---- saving

    /// Before a save writes `path`: keeps the file that is there as an
    /// earlier save. Gives back that backup, if one was made. If none could
    /// be, the save goes ahead all the same, and the status line says so.
    pub(super) fn older_before_save(&mut self, path: &Path) -> Option<PathBuf> {
        self.older.note.clear();
        self.older.backup = None;
        if !is_native_file(path) {
            return None;
        }
        match backups::keep(path) {
            Ok(backup) => backup,
            Err(e) => {
                log::warn!("{e}");
                self.toast("No backup of the last save could be kept");
                None
            }
        }
    }

    /// After that save: if it was written, the earlier saves of `path` are
    /// thinned out (`backups::prune`), and what it left out is noted for the
    /// status line; if it was not, the backup made for it goes again.
    pub(super) fn older_after_save(&mut self, path: &Path, backup: Option<PathBuf>, written: bool) {
        if !written {
            if let Some(backup) = backup {
                backups::unkeep(&backup);
            }
            return;
        }
        backups::prune(path, &io::time_stamp());
        // Deleted from this scene, and still not back: the save left them out.
        let document = self.doc.path.clone();
        let same = |of: &Option<PathBuf>| match (of, &document) {
            (Some(a), Some(b)) => crate::recent::same_file(a, b),
            (None, None) => true,
            _ => false,
        };
        let scene = &self.doc.scene;
        let names: Vec<String> = self
            .older
            .deleted
            .iter()
            .filter(|deleted| same(&deleted.document) && scene.get(deleted.id).is_none())
            .map(|deleted| deleted.name.clone())
            .collect();
        self.older.note = left_out_words(&names, backup.is_some());
        self.older.backup = backup;
        self.older.deleted.clear();
    }

    /// What the last save has to say about the objects it left out, for its
    /// status line; empty if nothing. Said once.
    pub(super) fn older_saved_note(&mut self) -> String {
        std::mem::take(&mut self.older.note)
    }

    /// An object was deleted: the next save of this scene says so.
    pub(super) fn older_deleted(&mut self, id: u64, name: &str) {
        let document = self.doc.path.clone();
        self.older.deleted.push(Deleted { id, name: name.to_owned(), document });
    }

    // ---- the window

    /// The File tab's Older Versions button: lists the earlier saves of the
    /// scene's file and the recovery copies kept for a week.
    pub(super) fn older_versions_action(&mut self) {
        let today = io::time_stamp();
        let when = |stamp: &str| recovery::when_words(stamp, &today).unwrap_or_else(|| "at a time not noted".to_owned());
        // A sculpt from the Unity app is never saved over, so it has none.
        let document = self.doc.path.clone().filter(|path| is_sclp(path));
        let mut listed = Vec::new();
        let saves: Vec<Row> = document.as_deref().map_or(Vec::new(), backups::list).into_iter().map(|backup| {
            listed.push((backup.file.clone(), document.clone()));
            Row { file: backup.file, when: format!("Saved {}", when(&backup.stamp)), what: size_words(backup.bytes) }
        }).collect();
        let copies: Vec<Row> = self.recovery_kept().into_iter().map(|copy| {
            listed.push((copy.file.clone(), copy.about.document.clone()));
            Row { file: copy.file.clone(), when: format!("Kept {}", when(&copy.stamp())), what: copy.about.words() }
        }).collect();
        self.older.listed = listed;
        let unsaved = self.unsaved() || !self.scaffolds_at_risk(Loss::SceneReplaced).is_empty();
        let copies_folder = self.recovery_kept_folder().filter(|folder| folder.is_dir());
        self.ui.older.open = Some(Listing {
            saves_folder: document.as_deref().map(backups::folder_of),
            document,
            saves,
            copies,
            copies_folder,
            can_replace: !unsaved,
        });
        self.redraw = true;
    }

    /// One of them, chosen in the window: its objects added to the scene
    /// (`add`), or the whole of it in place of the scene, with no file.
    pub(super) fn open_older(&mut self, file: PathBuf, add: bool) {
        self.ui.older.open = None;
        let document = self.older.listed.iter().find(|(listed, _)| *listed == file).and_then(|(_, of)| of.clone());
        if add {
            // As Import does it: the scene and its file stay as they are.
            return self.load(file, false);
        }
        if self.unsaved() || !self.scaffolds_at_risk(Loss::SceneReplaced).is_empty() {
            return self.status("Save the scene first, or use Add Objects, which keeps it as it is.", false);
        }
        let saved = match io::load_scene(&file) {
            Ok(saved) => saved,
            Err(e) => return self.status(e, false),
        };
        // No file of its own: Save asks where, offering the one it is a version of.
        let note = Restored { run: PathBuf::new(), document };
        let (count, _) = self.put_in_place(saved, FileState::restored(note));
        let objects = if count == 1 { "1 object".to_owned() } else { format!("{count} objects") };
        self.status(
            format!(
                "Opened {} ({objects}) in place of the scene. It has no file of its own: Save (Ctrl + S) asks \
                 where to keep it. Undo (Z) brings the scene from before back.",
                file_name(&file)
            ),
            true,
        );
    }

    /// Show Folder in that window.
    pub(super) fn show_folder(&mut self, folder: &Path) {
        if let Err(e) = crate::capture::open_folder(folder) {
            self.status(e, false);
        }
    }
}

/// Whether `path` is named as this app's scene files are.
fn is_sclp(path: &Path) -> bool {
    path.extension().is_some_and(|kind| kind.eq_ignore_ascii_case("sclp"))
}

/// Whether `path` is a scene file that is there to be kept.
fn is_native_file(path: &Path) -> bool {
    is_sclp(path) && path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_save_names_what_it_left_out_and_where_to_find_it() {
        assert_eq!(left_out_words(&[], true), "");
        let door = left_out_words(&["Door".to_owned()], true);
        assert_eq!(
            door,
            ". \"Door\" was deleted and was not saved. The save before, which has it, is kept: \
             File tab, Older Versions, Add Objects."
        );
        let two = left_out_words(&["Door".to_owned(), "Hinge".to_owned()], false);
        assert_eq!(
            two,
            ". \"Door\" and \"Hinge\" were deleted and were not saved. \
             Undo (Z) brings them back, but only until GolemSculpt is closed."
        );
    }
}
