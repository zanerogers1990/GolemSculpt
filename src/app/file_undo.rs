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
//!
//! So does where a scene was restored from, if it came from a recovery copy
//! (`history::Restored`). A save retires the copies of the scene it saves and
//! no others, so Undo of a restore must leave that note behind with the
//! restored scene: the scene from before it is not the crashed work, and
//! saving or closing it settles nothing about the copies.

use std::path::{Path, PathBuf};

use super::files::Mark;
use crate::history::{FileState, History, Restored};
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
    /// The recovery copy the scene on screen was restored from, until that
    /// scene is saved; `None` for every other scene.
    pub(super) restored: &'a mut Option<Restored>,
    pub(super) history: &'a mut History,
}

impl File<'_> {
    /// Whether the scene, standing at `now`, differs from what its file holds.
    pub(super) fn unsaved(&self, now: Mark) -> bool {
        *self.clean != Some(now)
    }

    /// The file as it is, for a step that is about to replace the scene to keep.
    pub(super) fn state(&self, now: Mark) -> FileState {
        FileState { path: self.path.clone(), saved: !self.unsaved(now), restored: self.restored.clone() }
    }

    /// Another file's scene has taken the scene's place. `document` is that
    /// file if the scene now belongs to it (an Open): the scene is what it
    /// holds. With none (a recovery copy) the scene has no file and counts
    /// as unsaved; `restored` then says which copy it was.
    pub(super) fn became(&mut self, document: Option<PathBuf>, restored: Option<Restored>, now: Mark) {
        *self.clean = document.is_some().then_some(now);
        *self.path = document;
        *self.restored = restored;
    }

    /// Putting another scene in place of this one broke off half way (a
    /// fault while a recovery copy was being restored): what is on screen is
    /// no file's scene and nobody's restored copy. It has no file, so Save
    /// asks where it should go and writes over nothing, and it counts as
    /// unsaved, so closing asks first.
    pub(super) fn broke(&mut self) {
        (*self.path, *self.clean, *self.restored) = (None, None, None);
    }

    /// Undo (`undo`) or Redo has just taken a step; `was_saved` is whether
    /// the scene was what its file holds before it, and `now` its mark after.
    /// A step that changed the scene's file gives the scene now on screen its
    /// own file back, saved if it was, and keeps the other for the way back.
    /// Where a scene was restored from crosses with it. Returns whether it
    /// was such a step.
    pub(super) fn stepped(&mut self, undo: bool, was_saved: bool, now: Mark) -> bool {
        let (path, restored, history) = (&mut *self.path, &mut *self.restored, &mut *self.history);
        let here = || FileState { path: path.take(), saved: was_saved, restored: restored.take() };
        let Some(there) = history.cross_file(undo, here) else { return false };
        *self.clean = there.saved.then_some(now);
        *self.path = there.path;
        *self.restored = there.restored;
        true
    }

    /// The scene has just been written to `path`. It is what that file holds,
    /// and a scene across an Open that had `path` for its file is not any
    /// more: when Undo or Redo brings it back it is unsaved work. Gives back
    /// the recovery copy this scene was restored from, if it was: the work is
    /// in a file of the user's now, and that copy has had its day.
    pub(super) fn written(&mut self, path: &Path, now: Mark) -> Option<Restored> {
        *self.clean = Some(now);
        for kept in self.history.kept_files() {
            if kept.path.as_deref().is_some_and(|other| recent::same_file(other, path)) {
                kept.saved = false;
            }
        }
        self.restored.take()
    }
}
