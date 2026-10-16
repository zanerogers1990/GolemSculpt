//! Recovery copies kept for a week after their run has finished with them.
//!
//! A run's copies used to go as soon as the scene was saved or the app closed
//! properly (rules (a) and (c) at the top of `crate::recovery`). "Exit
//! Without Saving" over an afternoon's work then left nothing anywhere. So
//! before a run removes its copies, the newest of them is moved here, under
//! the run's name, in place of the one that run kept here before:
//!
//! ```text
//! %APPDATA%\Sculpt\kept recovery copies\
//!     2026-10-15_201203-89660.sclp    the newest copy that run made
//!     2026-10-15_201203-89660.json    what it is a copy of
//! ```
//!
//! One file per run of the app, then, never more, and each is kept for
//! [`DAYS`] days from when it was written; past [`MOST`] the oldest go
//! sooner. They are not crash copies: nothing asks about them at start-up.
//! The File tab's Older Versions opens them, or adds their objects to the
//! scene. The folder is beside the `recovery` one, not in it, so the rule
//! that offers what a crash left behind never looks at it.

use std::path::{Path, PathBuf};

use super::{About, Kept};

/// The folder beside the settings file the copies are kept in.
pub const FOLDER: &str = "kept recovery copies";

/// How long a kept copy stays.
pub const DAYS: i64 = 7;

/// At most this many are kept, the newest.
pub const MOST: usize = 20;

/// One kept copy.
#[derive(Clone, Debug, PartialEq)]
pub struct Copy {
    pub file: PathBuf,
    pub about: About,
}

impl Copy {
    /// When it was written: the stamp noted beside it, or else the start of
    /// its run, which its name begins with.
    pub fn stamp(&self) -> String {
        if crate::backups::seconds_of(&self.about.when).is_some() {
            return self.about.when.clone();
        }
        let name = self.file.file_stem().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        name.get(..17).unwrap_or_default().to_owned()
    }
}

/// The folder, for the `recovery` folder `root`.
pub fn folder(root: &Path) -> PathBuf {
    root.parent().unwrap_or(Path::new("")).join(FOLDER)
}

/// Moves `newest`, a copy of the run called `run`, here, in place of what
/// that run kept here before, and drops what has been kept long enough.
/// `now` is the time it is, as a stamp. Nothing is moved if anything fails:
/// the copy is then removed with the rest, as it always was.
pub fn keep(root: &Path, run: &str, newest: &Kept, now: &str) {
    let dir = folder(root);
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let file = dir.join(format!("{run}.sclp"));
    match std::fs::rename(&newest.file, &file) {
        Ok(()) => {
            let _ = std::fs::rename(newest.file.with_extension("json"), file.with_extension("json"));
        }
        Err(e) => log::warn!("The last recovery copy could not be kept in {}: {e}", dir.display()),
    }
    prune(root, now);
}

/// The kept copies, newest first.
pub fn list(root: &Path) -> Vec<Copy> {
    let Ok(entries) = std::fs::read_dir(folder(root)) else { return Vec::new() };
    let mut found: Vec<Copy> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|file| file.is_file() && file.extension().is_some_and(|kind| kind == "sclp"))
        .map(|file| {
            let about = std::fs::read_to_string(file.with_extension("json"))
                .ok()
                .and_then(|text| serde_json::from_str(&text).ok())
                .unwrap_or_default();
            Copy { file, about }
        })
        .collect();
    found.sort_by(|a, b| b.stamp().cmp(&a.stamp()).then_with(|| b.file.cmp(&a.file)));
    found
}

/// Removes the copies older than [`DAYS`], and past the newest [`MOST`].
/// A copy that says nothing of when it was written stays until the count
/// pushes it out.
pub fn prune(root: &Path, now: &str) {
    let Some(now) = crate::backups::seconds_of(now) else { return };
    for (i, copy) in list(root).iter().enumerate() {
        let age = crate::backups::seconds_of(&copy.stamp()).map(|at| now - at);
        if i >= MOST || age.is_some_and(|age| age > DAYS * 86_400) {
            let _ = std::fs::remove_file(&copy.file);
            let _ = std::fs::remove_file(copy.file.with_extension("json"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(name: &str) -> PathBuf {
        let settings = std::env::temp_dir().join("sculpt_kept_copies_tests").join(name);
        let _ = std::fs::remove_dir_all(&settings);
        let root = settings.join("recovery");
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    /// A copy as a run writes one, with its notes beside it.
    fn copy(dir: &Path, number: u32, when: &str, scene: &str) -> Kept {
        std::fs::create_dir_all(dir).unwrap();
        let file = dir.join(format!("copy-{number:04}.sclp"));
        std::fs::write(&file, scene).unwrap();
        let about = About { when: when.to_owned(), objects: 1, triangles: 10, document: None };
        std::fs::write(file.with_extension("json"), serde_json::to_string(&about).unwrap()).unwrap();
        Kept { file, stopped: false, number, about }
    }

    #[test]
    fn a_run_keeps_one_copy_its_newest() {
        let root = root("one");
        let run = root.join("2026-10-16_100000-1");
        let first = copy(&run, 1, "2026-10-16_100500", "after the first save");
        keep(&root, "2026-10-16_100000-1", &first, "2026-10-16_100600");
        let second = copy(&run, 2, "2026-10-16_110500", "after Exit Without Saving");
        keep(&root, "2026-10-16_100000-1", &second, "2026-10-16_110600");
        let kept = list(&root);
        assert_eq!(kept.len(), 1, "one per run");
        assert_eq!(std::fs::read_to_string(&kept[0].file).unwrap(), "after Exit Without Saving");
        assert_eq!(kept[0].about.when, "2026-10-16_110500", "its notes came with it");
        assert!(!second.file.exists(), "moved, not copied");
        assert_eq!(kept[0].file.parent(), Some(folder(&root).as_path()));
        assert_ne!(folder(&root).parent(), Some(root.as_path()), "beside the recovery folder, not in it");
    }

    #[test]
    fn copies_stay_a_week_and_no_more_than_the_most() {
        let root = root("week");
        let old = copy(&root.join("a"), 1, "2026-10-01_120000", "two weeks old");
        keep(&root, "2026-10-01_110000-7", &old, "2026-10-01_130000");
        let recent = copy(&root.join("b"), 1, "2026-10-12_120000", "four days old");
        keep(&root, "2026-10-12_110000-8", &recent, "2026-10-16_120000");
        let kept = list(&root);
        assert_eq!(kept.len(), 1);
        assert_eq!(std::fs::read_to_string(&kept[0].file).unwrap(), "four days old");

        for i in 0..MOST + 5 {
            let run = format!("2026-10-16_{:02}0000-{i}", i % 24);
            let made = copy(&root.join(&run), 1, &format!("2026-10-16_{:02}{:02}00", i % 24, i / 24), "a run");
            keep(&root, &run, &made, "2026-10-16_235900");
        }
        assert_eq!(list(&root).len(), MOST);
    }
}
