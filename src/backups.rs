//! Earlier saves of a scene, kept beside it. Saving puts the new file in
//! place of the old one in a single rename (`io::replace_file`), and before
//! this nothing of the old one was left: one stray click (deleting an object,
//! say) and one save, and hours of work were gone once the app had closed and
//! taken its undo history with it. That is what happened to the Door on
//! 2026-10-15.
//!
//! So before a save writes over a scene file, the file that is there is kept
//! in a folder beside it:
//!
//! ```text
//! D:\work\dragon head.sclp
//! D:\work\GolemSculpt backups\dragon head 2026-10-15_231105.sclp
//! ```
//!
//! named for when that version was saved (the old file's own date, read as
//! it is kept; the name is what is read back, never the date of the backup,
//! which a copy of the folder changes). It is kept as a second name for the
//! same file on the disk (a hard link), which costs no time and no room until
//! the new save takes the old name: a scene of hundreds of megabytes is not
//! copied on every Ctrl + S. A drive that cannot do that (FAT, exFAT, some
//! network shares) gets a plain copy instead.
//!
//! Which are kept ([`chosen`]): the last [`NEWEST`] saves, and, so that a
//! loss found the next day can still be undone after a morning of saves,
//! one more for each of [`SPREAD`]: the newest save that is at least that old
//! and not kept already. At most nine per scene, then, spread over a week and
//! more. The rest are removed as each save is kept.

use std::path::{Path, PathBuf};

/// The folder beside a scene that its earlier saves are kept in.
pub const FOLDER: &str = "GolemSculpt backups";

/// The newest saves, all kept whatever their age.
pub const NEWEST: usize = 3;

/// Ages, in seconds, at each of which one older save is kept as well: an
/// hour, three hours, eight hours, a day, three days and a week.
pub const SPREAD: [i64; 6] = [3_600, 3 * 3_600, 8 * 3_600, 86_400, 3 * 86_400, 7 * 86_400];

/// One earlier save found on disk.
#[derive(Clone, Debug, PartialEq)]
pub struct Backup {
    pub file: PathBuf,
    /// When that version was saved, as `io::time_stamp` writes it.
    pub stamp: String,
    pub bytes: u64,
}

/// The folder `scene`'s earlier saves go in.
pub fn folder_of(scene: &Path) -> PathBuf {
    scene.parent().unwrap_or(Path::new("")).join(FOLDER)
}

/// The scene's name without `.sclp`.
fn stem_of(scene: &Path) -> String {
    scene.file_stem().map_or_else(String::new, |stem| stem.to_string_lossy().into_owned())
}

/// Whether `text` is a time stamp as `io::time_stamp` writes it:
/// `2026-10-15_231105`.
fn is_stamp(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 17
        && bytes.iter().enumerate().all(|(i, &b)| match i {
            4 | 7 => b == b'-',
            10 => b == b'_',
            _ => b.is_ascii_digit(),
        })
}

/// The stamp in the name of an earlier save of the scene called `stem`, or
/// `None` if `name` is not one. ("dragon head 2026-10-15_231105.sclp" is one
/// of "dragon head"; it is not one of "dragon", whose would end in a stamp
/// right after "dragon ".)
fn stamp_in(stem: &str, name: &str) -> Option<String> {
    let lower = name.to_lowercase();
    let rest = lower.strip_suffix(".sclp")?.strip_prefix(&format!("{} ", stem.to_lowercase()))?;
    is_stamp(rest).then(|| rest.to_owned())
}

/// A stamp as seconds from some fixed day: for telling how far apart two
/// are, not for a clock. `None` for anything that is not a stamp.
pub fn seconds_of(stamp: &str) -> Option<i64> {
    if !is_stamp(stamp) {
        return None;
    }
    let number = |range: std::ops::Range<usize>| stamp[range].parse::<i64>().ok();
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(13..15)?, number(15..17)?);
    // Days from 1970-01-01 (Howard Hinnant's days_from_civil).
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// The local time a file was last written, as a stamp. That is when the
/// version in it was saved.
fn stamp_of_file(file: &Path) -> Option<String> {
    let written = std::fs::metadata(file).and_then(|meta| meta.modified()).ok()?;
    local_stamp(written)
}

#[cfg(windows)]
fn local_stamp(time: std::time::SystemTime) -> Option<String> {
    #[repr(C)]
    #[derive(Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn FileTimeToSystemTime(file_time: *const FileTime, system_time: *mut SystemTime) -> i32;
        fn SystemTimeToTzSpecificLocalTime(zone: *const u8, universal: *const SystemTime, local: *mut SystemTime) -> i32;
    }
    // Windows counts hundreds of nanoseconds from 1601.
    let since = time.duration_since(std::time::UNIX_EPOCH).ok()?;
    let ticks = since.as_nanos() as u64 / 100 + 116_444_736_000_000_000;
    let file_time = FileTime { low: ticks as u32, high: (ticks >> 32) as u32 };
    let (mut universal, mut local) = (SystemTime::default(), SystemTime::default());
    // SAFETY: both calls only read the first struct they are given and fill in
    // the last, which have the layouts above; a null zone is the system's own.
    let done = unsafe {
        FileTimeToSystemTime(&file_time, &mut universal) != 0
            && SystemTimeToTzSpecificLocalTime(std::ptr::null(), &universal, &mut local) != 0
    };
    let t = &local;
    done.then(|| format!("{:04}-{:02}-{:02}_{:02}{:02}{:02}", t.year, t.month, t.day, t.hour, t.minute, t.second))
}

/// Everywhere else, UTC, as `io::time_stamp` gives it there.
#[cfg(not(windows))]
fn local_stamp(time: std::time::SystemTime) -> Option<String> {
    let seconds = time.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    Some(crate::io::stamp_from_unix(seconds))
}

/// The earlier saves of `scene`, newest first.
pub fn list(scene: &Path) -> Vec<Backup> {
    let stem = stem_of(scene);
    let Ok(entries) = std::fs::read_dir(folder_of(scene)) else { return Vec::new() };
    let mut found: Vec<Backup> = entries
        .flatten()
        .filter_map(|entry| {
            let stamp = stamp_in(&stem, &entry.file_name().to_string_lossy())?;
            let meta = entry.metadata().ok().filter(|meta| meta.is_file())?;
            Some(Backup { file: entry.path(), stamp, bytes: meta.len() })
        })
        .collect();
    found.sort_by(|a, b| b.stamp.cmp(&a.stamp).then_with(|| b.file.cmp(&a.file)));
    found
}

/// Which of the saves made at `stamps` (seconds, newest first) to keep, `now`
/// being the time it is: the newest [`NEWEST`], then for each age of
/// [`SPREAD`] the newest one at least that old that is not kept already.
pub fn chosen(stamps: &[i64], now: i64) -> Vec<bool> {
    let mut keep: Vec<bool> = (0..stamps.len()).map(|i| i < NEWEST).collect();
    for age in SPREAD {
        if let Some(i) = (0..stamps.len()).find(|&i| !keep[i] && now - stamps[i] >= age) {
            keep[i] = true;
        }
    }
    keep
}

/// Keeps the file that is at `scene` now, before a save writes over it.
/// Gives back the backup, or `None` when there was nothing to keep (no file
/// there yet) or that very version is kept already. An error means none
/// could be made; the save should go ahead all the same, and say so.
pub fn keep(scene: &Path) -> Result<Option<PathBuf>, String> {
    if !scene.is_file() {
        return Ok(None);
    }
    let stamp = stamp_of_file(scene).unwrap_or_else(crate::io::time_stamp);
    let folder = folder_of(scene);
    let file = folder.join(format!("{} {stamp}.sclp", stem_of(scene)));
    if file.exists() {
        return Ok(None);
    }
    let failed = |e: std::io::Error| format!("No backup of the last save could be kept in {}: {e}", folder.display());
    std::fs::create_dir_all(&folder).map_err(failed)?;
    if std::fs::hard_link(scene, &file).is_ok() {
        return Ok(Some(file));
    }
    // No hard links on this drive: a copy, whole or not at all.
    let mut part = file.clone().into_os_string();
    part.push(".tmp");
    let part = PathBuf::from(part);
    let copied = std::fs::copy(scene, &part).and_then(|_| std::fs::rename(&part, &file));
    if let Err(e) = copied {
        let _ = std::fs::remove_file(&part);
        return Err(failed(e));
    }
    Ok(Some(file))
}

/// A save did not go ahead after all: the backup `keep` made for it goes
/// again, since the file it is a second name for is still in place.
pub fn unkeep(backup: &Path) {
    let _ = std::fs::remove_file(backup);
}

/// Removes the earlier saves of `scene` that [`chosen`] does not keep, `now`
/// being the stamp of the time it is.
pub fn prune(scene: &Path, now: &str) {
    let Some(now) = seconds_of(now) else { return };
    let backups = list(scene);
    let stamps: Vec<i64> = backups.iter().map(|b| seconds_of(&b.stamp).unwrap_or(now)).collect();
    for (backup, keep) in backups.iter().zip(chosen(&stamps, now)) {
        if !keep {
            let _ = std::fs::remove_file(&backup.file);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder of its own under the temp folder, empty.
    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("sculpt_backups_tests").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const HOUR: i64 = 3_600;
    const DAY: i64 = 86_400;

    #[test]
    fn a_stamp_reads_as_seconds_and_anything_else_does_not() {
        let a = seconds_of("2026-10-15_231105").unwrap();
        let b = seconds_of("2026-10-16_001105").unwrap();
        assert_eq!(b - a, HOUR, "across midnight");
        let new_year = seconds_of("2027-01-01_000000").unwrap() - seconds_of("2026-12-31_000000").unwrap();
        assert_eq!(new_year, DAY);
        let leap = seconds_of("2028-03-01_000000").unwrap() - seconds_of("2028-02-28_000000").unwrap();
        assert_eq!(leap, 2 * DAY, "2028 has a 29th of February");
        assert_eq!(seconds_of("1970-01-01_000001"), Some(1));
        for not in ["", "2026-10-15", "2026-10-15 231105", "2026-10-15_23110x", "copy-0007"] {
            assert_eq!(seconds_of(not), None, "{not}");
        }
    }

    #[test]
    fn a_name_is_one_of_this_scene_only() {
        assert_eq!(stamp_in("dragon head", "dragon head 2026-10-15_231105.sclp").as_deref(), Some("2026-10-15_231105"));
        assert_eq!(stamp_in("Dragon Head", "dragon head 2026-10-15_231105.SCLP").as_deref(), Some("2026-10-15_231105"));
        assert_eq!(stamp_in("dragon", "dragon head 2026-10-15_231105.sclp"), None);
        assert_eq!(stamp_in("dragon head", "dragon head 2026-10-15_231105.sclp.tmp"), None);
        assert_eq!(stamp_in("dragon head", "dragon head.sclp"), None);
    }

    #[test]
    fn the_newest_three_are_kept_and_one_more_for_each_age() {
        let now = 100 * DAY;
        // Saves every ten minutes for a little over three hours: the newest
        // three, the newest an hour old, and the newest three hours old.
        let morning: Vec<i64> = (0..20).map(|i| now - 60 - i * 600).collect();
        let keep = chosen(&morning, now);
        let kept: Vec<usize> = (0..keep.len()).filter(|&i| keep[i]).collect();
        assert_eq!(kept, [0, 1, 2, 6, 18]);

        // The night of the Door: a save at 23:20 with it, the stray click,
        // the save at 00:11 without it, then a morning of saves the next day.
        // The save from before the click is still there at lunchtime.
        let midnight = now - 12 * HOUR;
        let mut saves: Vec<i64> = (0..8).map(|i| now - 30 * 60 - i * 15 * 60).collect();
        saves.push(midnight + 11 * 60);
        let with_door = midnight - 40 * 60;
        saves.push(with_door);
        saves.extend((1..5).map(|i| with_door - i * 20 * 60));
        let keep = chosen(&saves, now);
        assert!(keep[saves.iter().position(|&s| s == with_door).unwrap()], "the save before the click is kept");
        assert!(keep.iter().filter(|&&k| k).count() <= NEWEST + SPREAD.len());
    }

    #[test]
    fn never_more_than_nine_and_old_ones_go_but_one() {
        let now = 1_000 * DAY;
        let years: Vec<i64> = (0..400).map(|i| now - i * 6 * HOUR).collect();
        let keep = chosen(&years, now);
        assert_eq!(keep.iter().filter(|&&k| k).count(), NEWEST + SPREAD.len());
        let oldest_kept = (0..keep.len()).rev().find(|&i| keep[i]).unwrap();
        assert!(now - years[oldest_kept] >= 7 * DAY, "one a week old or more stays");
        assert!(now - years[oldest_kept] < 8 * DAY, "and nothing much older");
        // Fewer saves than that: all of them stay.
        assert!(chosen(&[now, now - HOUR, now - DAY], now).iter().all(|&k| k));
    }

    #[test]
    fn keeping_a_save_leaves_the_old_version_beside_the_new_one() {
        let dir = folder("keep");
        let scene = dir.join("dragon head.sclp");
        assert_eq!(keep(&scene), Ok(None), "nothing there yet: nothing to keep");
        std::fs::write(&scene, b"with the door").unwrap();
        let backup = keep(&scene).unwrap().expect("the old version is kept");
        assert_eq!(backup.parent(), Some(folder_of(&scene).as_path()));
        assert!(backup.file_name().unwrap().to_string_lossy().starts_with("dragon head 20"));
        assert_eq!(keep(&scene), Ok(None), "the same version is not kept twice");
        // The save itself: a new file renamed over the scene's name, as
        // `io::replace_file` does it.
        let temp = dir.join("dragon head.sclp.tmp");
        std::fs::write(&temp, b"without it").unwrap();
        std::fs::rename(&temp, &scene).unwrap();
        assert_eq!(std::fs::read(&scene).unwrap(), b"without it");
        assert_eq!(std::fs::read(&backup).unwrap(), b"with the door", "the backup is the old file, untouched");
        let listed = list(&scene);
        assert_eq!(listed.len(), 1);
        assert_eq!((listed[0].file.as_path(), listed[0].bytes), (backup.as_path(), 13));
        assert!(list(&dir.join("dragon.sclp")).is_empty(), "another scene has none");
    }

    #[test]
    fn a_save_that_does_not_go_ahead_takes_its_backup_with_it() {
        let dir = folder("unkeep");
        let scene = dir.join("bust.sclp");
        std::fs::write(&scene, b"as it was").unwrap();
        let backup = keep(&scene).unwrap().unwrap();
        unkeep(&backup);
        assert!(!backup.exists());
        assert_eq!(std::fs::read(&scene).unwrap(), b"as it was", "the scene is untouched");
    }

    #[test]
    fn pruning_removes_what_is_not_chosen_and_nothing_of_another_scene() {
        let dir = folder("prune");
        let scene = dir.join("bust.sclp");
        let backups = folder_of(&scene);
        std::fs::create_dir_all(&backups).unwrap();
        let stamps = ["2026-10-16_120000", "2026-10-16_115000", "2026-10-16_114000", "2026-10-16_113000"];
        for stamp in stamps {
            std::fs::write(backups.join(format!("bust {stamp}.sclp")), stamp).unwrap();
        }
        let other = backups.join("bust two 2026-10-16_100000.sclp");
        std::fs::write(&other, b"another scene's").unwrap();
        let mine = backups.join("my notes.txt");
        std::fs::write(&mine, b"the user's").unwrap();
        prune(&scene, "2026-10-16_120500");
        let left: Vec<String> = list(&scene).into_iter().map(|b| b.stamp).collect();
        assert_eq!(left, &stamps[..3], "the newest three; the fourth is not an hour old");
        assert!(other.exists() && mine.exists());
    }
}
