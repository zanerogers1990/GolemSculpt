//! Saves and recovery copies, put on the disk by a thread of their own.
//!
//! Writing a scene of millions of triangles, and then waiting for the disk to
//! say it really has it (`sync_all`), took the window's own thread for up to
//! a second or more, and a recovery copy did that by itself every few
//! minutes, in the middle of work. Now the scene is made into the file's
//! bytes on the window's thread, which only reads and copies it and is quick,
//! and a thread puts them on the disk, the same way as before: beside the
//! old file, and in its place only once whole (`io::SceneBytes`).
//!
//! - **Which writes**: Save and Save As from the button and Ctrl + S, and the
//!   timed recovery copy. A save that something waits on (Save before
//!   closing, before Open or New Scene, from the Claude bridge) and the copy
//!   taken before the scene is put aside or as the app stops on a fault are
//!   written as before, at once: [`Running::writes_finish`] lands a write
//!   still at work first, so they never cross.
//! - **One at a time.** A Save while one is still writing waits for it.
//!   A recovery copy that falls due waits until no write is at work.
//! - **What happens after the write** (the file is the scene's own, the
//!   older versions are thinned out, the recovery copies go, the status line
//!   says Saved) happens when it lands, which `recovery_tick` looks for
//!   between frames. The scene's mark is taken when the save starts, so work
//!   done while it writes is still unsaved after it.
//! - **Closing the app** waits for a write at work (`Drop`), so the file is
//!   whole on the disk before the program ends.

use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::super::Running;
use super::Mark;
use crate::io::{self, SceneBytes};
use crate::recovery::About;

/// How often a write at work is looked at.
pub(in crate::app) const POLL: Duration = Duration::from_millis(50);

/// What is still to be done once a write has landed.
pub(in crate::app) enum After {
    /// A Save: its file, the scene's mark when it began, the earlier save kept
    /// for it (`older_before_save`), and how many damaged points it mended.
    Save { path: PathBuf, mark: Mark, backup: Option<PathBuf>, mended: usize },
    /// A recovery copy: its file, what it holds, the scene's mark when it
    /// began, when it began, and how long the window's thread spent on it.
    Copy { file: PathBuf, about: About, mark: Mark, started: Instant, paused: Duration },
}

/// A write at work.
struct Writing {
    after: After,
    thread: JoinHandle<io::Result<()>>,
}

/// The write at work, if any.
#[derive(Default)]
pub(in crate::app) struct Writes {
    now: Option<Writing>,
}

impl Drop for Writes {
    /// The app is closing: the file is finished first. What would have been
    /// said about it has nobody to read it.
    fn drop(&mut self) {
        if let Some(writing) = self.now.take() {
            let _ = writing.thread.join();
        }
    }
}

/// What a write that stopped on a bug says.
fn stopped(path: &Path) -> String {
    format!("Could not write {}: the write stopped part-way. The file that was there is as it was.", path.display())
}

impl Running {
    /// Whether a save or a recovery copy is being written.
    pub(in crate::app) fn writing(&self) -> bool {
        self.files.writes.now.is_some()
    }

    /// Puts `bytes` on the disk at `to` on a thread, and `after` is done
    /// once they are there (or are not).
    pub(in crate::app) fn write_behind(&mut self, bytes: SceneBytes, to: PathBuf, after: After) {
        self.writes_finish();
        let target = to.clone();
        let spawned = std::thread::Builder::new().name("writing a scene".into()).spawn(move || bytes.write(&target));
        match spawned {
            Ok(thread) => self.files.writes.now = Some(Writing { after, thread }),
            // No thread to be had: the write is done here, as it always was.
            Err(e) => {
                log::warn!("no thread for writing {}: {e}", to.display());
                self.writes_land(after, Err(format!("Could not write {}: {e}", to.display())));
            }
        }
        self.redraw = true;
    }

    /// Between frames: lands a write that has finished. Returns when to look
    /// again while one is still at work.
    pub(in crate::app) fn writes_poll(&mut self) -> Option<Instant> {
        let finished = self.files.writes.now.as_ref()?.thread.is_finished();
        if !finished {
            return Some(Instant::now() + POLL);
        }
        self.writes_finish();
        None
    }

    /// Waits for the write at work, if any, and lands it: before anything
    /// that must find it finished (another save, a copy written at once, an
    /// Open or a New Scene).
    pub(in crate::app) fn writes_finish(&mut self) {
        let Some(Writing { after, thread }) = self.files.writes.now.take() else { return };
        let path = match &after {
            After::Save { path, .. } => path.clone(),
            After::Copy { file, .. } => file.clone(),
        };
        let written = thread.join().unwrap_or_else(|_| Err(stopped(&path)));
        self.writes_land(after, written);
    }

    fn writes_land(&mut self, after: After, written: io::Result<()>) {
        self.redraw = true;
        match after {
            After::Save { path, mark, backup, mended } => self.save_landed(path, mark, backup, mended, written),
            After::Copy { file, about, mark, started, paused } => {
                self.recovery_copy_landed(file, about, mark, started, paused, written)
            }
        }
    }

    /// Save and Save As from the button or Ctrl + S: asks where if the
    /// scene has no file yet, then writes it in the background.
    pub(in crate::app) fn save_action_behind(&mut self, save_as: bool) {
        // A Save still writing gives the scene its file first, so this one knows it.
        self.writes_finish();
        let Some(path) = self.save_target(save_as).or_else(|| self.ask_where_to_save()) else { return };
        let backup = self.older_before_save(&path);
        let made = self
            .scene_for_file()
            .and_then(|(meshes, objects, active)| SceneBytes::of(&meshes, &objects, active, &self.details(), &path));
        let bytes = match made {
            Ok(bytes) => bytes,
            Err(e) => {
                self.older_after_save(&path, backup, false);
                return self.status(e, false);
            }
        };
        let (mark, mended) = (self.history_mark(), bytes.mended);
        self.status(format!("Saving {}...", path.display()), true);
        self.write_behind(bytes, path.clone(), After::Save { path, mark, backup, mended });
    }

    /// A save written in the background has landed: what `save_to` does
    /// after its write.
    fn save_landed(&mut self, path: PathBuf, mark: Mark, backup: Option<PathBuf>, mended: usize, written: io::Result<()>) {
        self.older_after_save(&path, backup, written.is_ok());
        if let Err(e) = written {
            return self.status(e, false);
        }
        self.files.mended_on_save = mended;
        let restored = self.file().written(&path, mark);
        self.note_recent(&path, true);
        self.recovery_saved(restored);
        let size = std::fs::metadata(&path).map_or(String::new(), |m| format!(" ({:.1} MB)", m.len() as f64 / 1e6));
        let note = self.saved_note() + &self.older_saved_note();
        self.status(format!("Saved {}{size}{note}", path.display()), true);
        self.toast("Saved");
        self.doc.path = Some(path);
    }
}
