//! A file dragged from Explorer onto the window. A sculpt (`.sclp`, or a
//! `.sculpt` from the Unity app) is opened exactly as Open opens one: in place
//! of the scene, asking first only when that would put unsaved work aside
//! (`open_chosen`). A model (`.obj`, `.stl`) is added to the scene as Import
//! adds it. Either is one undo step. Anything else is turned away with a line
//! saying what can be dropped.
//!
//! The drop arrives as `Action::DropFile`, so it waits, as every command does,
//! while a stroke or a gizmo drag holds the scene. While the file is still
//! being dragged over the window a toast says what letting go will do.

use std::path::{Path, PathBuf};

use super::Running;
use crate::unity_import;

/// What a dropped file is taken for, by its extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Dropped {
    /// Opened in place of the scene.
    Sculpt,
    /// Added to the scene.
    Model,
    /// Not a file the app reads.
    Other,
}

pub(super) fn kind(path: &Path) -> Dropped {
    let extension = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    match extension.as_str() {
        "sclp" => Dropped::Sculpt,
        e if e == unity_import::EXTENSION => Dropped::Sculpt,
        "obj" | "stl" => Dropped::Model,
        _ => Dropped::Other,
    }
}

fn name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

impl Running {
    /// A file is being dragged over the window: say what letting go will do.
    pub(super) fn file_hovered(&mut self, path: &Path) {
        self.toast(match kind(path) {
            Dropped::Sculpt => "Drop to Open",
            Dropped::Model => "Drop to Import",
            Dropped::Other => "Not a Sculpt or Model",
        });
    }

    /// A file was let go over the window (`Action::DropFile`).
    pub(super) fn file_dropped(&mut self, path: PathBuf) {
        if self.ui.graph.asking() {
            // Another dropped file's question, or any other, is waiting.
            return self.status(format!("{} was not opened: answer the question first.", name(&path)), false);
        }
        if !path.is_file() {
            return self.status(format!("{} is not a file that can be opened.", name(&path)), false);
        }
        match kind(&path) {
            Dropped::Sculpt => self.open_chosen(path),
            Dropped::Model => self.load(path, false),
            Dropped::Other => self.status(
                format!("{} is not a file GolemSculpt opens. Drop a sculpt (.sclp, .sculpt) or a model (.obj, .stl).", name(&path)),
                false,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dropped_file_is_taken_by_its_extension_whatever_its_case() {
        assert_eq!(kind(Path::new("D:/Sculpts/dragon head.sclp")), Dropped::Sculpt);
        assert_eq!(kind(Path::new("old.SCULPT")), Dropped::Sculpt);
        assert_eq!(kind(Path::new("print.STL")), Dropped::Model);
        assert_eq!(kind(Path::new("scan.obj")), Dropped::Model);
        assert_eq!(kind(Path::new("photo.png")), Dropped::Other);
        assert_eq!(kind(Path::new("no extension")), Dropped::Other);
    }
}
