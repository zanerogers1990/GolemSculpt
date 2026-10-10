//! The scene's file through Undo and Redo.
//!
//! Opening a file in place of the scene is one undo step, and so is restoring
//! a recovery copy. Either changes which file the scene belongs to, and Undo
//! of it brings the objects from before back. Their file has to come back
//! with them, and whether they had been saved: if the name of the opened file
//! stayed behind, Save after Undo would write the old scene over the file
//! that was just opened.
//!
//! So such a step keeps the file of the scene on its other side
//! (`history::FileState`), and each time Undo or Redo crosses it the two
//! change places: the scene arrived at gets its file back, and the step
//! keeps the file of the scene just left. A scene's file is its own whichever
//! way it is reached: saving the opened scene under a new name, then undoing
//! the Open, leaves the scene from before with the file it always had.
//!
//! Whether a scene has unsaved changes crosses with it. A scene that was what
//! its file holds is that again when Undo or Redo comes back to it, unless
//! its file has been written since with another scene: then it is on screen
//! and in no file, which is unsaved work.

use std::path::{Path, PathBuf};

use super::files::Mark;
use crate::history::{FileState, History};
use crate::recent;

mod play;
#[cfg(test)]
mod tests;

/// The scene's file as the app keeps it, gathered from where its parts live
/// so that what happens to it can be said, and tested, in one place.
pub(super) struct File<'a> {
    /// The file Save writes to; `None` for a scene that has none yet.
    pub(super) path: &'a mut Option<PathBuf>,
    /// The scene's mark at which it was what its file holds: where it was
    /// last saved or opened, or where Undo or Redo arrived at a scene that
    /// was. `None` if no step short of one that changes the file leads to
    /// such a point.
    pub(super) clean: &'a mut Option<Mark>,
    pub(super) history: &'a mut History,
}

impl File<'_> {
    /// Whether the scene, standing at `now`, differs from what its file holds.
    pub(super) fn unsaved(&self, now: Mark) -> bool {
        *self.clean != Some(now)
    }

    /// The file as it is, for a step that is about to replace the scene to keep.
    pub(super) fn state(&self, now: Mark) -> FileState {
        FileState { path: self.path.clone(), saved: !self.unsaved(now) }
    }

    /// Another file's scene has taken the scene's place. `document` is that
    /// file if the scene now belongs to it (an Open): the scene is what it
    /// holds. With none (a recovery copy) the scene has no file and counts
    /// as unsaved.
    pub(super) fn became(&mut self, document: Option<PathBuf>, now: Mark) {
        *self.clean = document.is_some().then_some(now);
        *self.path = document;
    }

    /// Undo (`undo`) or Redo has just taken a step; `was_saved` is whether
    /// the scene was what its file holds before it, and `now` its mark after.
    /// A step that changed the scene's file gives the scene now on screen its
    /// own file back, saved if it was, and keeps the other for the way back.
    /// Returns whether it was such a step.
    pub(super) fn stepped(&mut self, undo: bool, was_saved: bool, now: Mark) -> bool {
        let (path, history) = (&mut *self.path, &mut *self.history);
        let here = || FileState { path: path.take(), saved: was_saved };
        let Some(there) = history.cross_file(undo, here) else { return false };
        *self.clean = there.saved.then_some(now);
        *self.path = there.path;
        true
    }

    /// The scene has just been written to `path`. It is what that file holds,
    /// and a scene across an Open that had `path` for its file is not any
    /// more: when Undo or Redo brings it back it is unsaved work.
    pub(super) fn written(&mut self, path: &Path, now: Mark) {
        *self.clean = Some(now);
        for kept in self.history.kept_files() {
            if kept.path.as_deref().is_some_and(|other| recent::same_file(other, path)) {
                kept.saved = false;
            }
        }
    }
}
