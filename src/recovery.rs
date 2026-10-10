//! Keeping work through a crash: the recovery copies of the scene, when one
//! is due, and the log a fault is written to. This is the part that needs no
//! window; `app/recovery.rs` is where it meets the running app.
//!
//! While the scene holds work that is in no file of the user's, the app
//! writes a copy of it every few minutes, as an ordinary `.sclp`, under the
//! settings folder:
//!
//! ```text
//! %APPDATA%\Sculpt\recovery\<when the run started>-<its process number>\
//!     running             held locked for as long as that run is alive
//!     copy-0007.sclp      the scene; the newest three are kept
//!     copy-0007.json      what it is a copy of, for the question at start-up
//!     stopped-0008.sclp   written as the app stopped on a fault, beside the others
//! %APPDATA%\Sculpt\log.txt   warnings, errors and what a fault said
//! ```
//!
//! A run removes its copies when its scene is saved and when it closes
//! properly. So nothing has to compare clocks or remember a clean exit: a
//! copy that is still there when the app starts, in a folder no running copy
//! of the app holds locked, is work that a crash left behind, and the app
//! offers it. Two copies of the app open at once each have a folder, and
//! neither offers the other's.
//!
//! A copy is never written over another. Each goes to a new numbered file,
//! whole or not at all (`io::replace_file`), and only then are older ones
//! dropped. The copy made as the app stops on a fault may hold a scene that
//! was caught mid-change, so it is checked first ([`sound`]), written beside
//! the regular copies without dropping any, and marked, so the question can
//! offer the last regular copy instead.

use std::fs::{File, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::io::SavedObject;
use crate::mesh::Mesh;

/// Regular copies kept in a run's folder.
pub const KEEP: usize = 3;
/// How long work may go without a copy.
pub const EVERY: Duration = Duration::from_secs(180);
/// A copy waits for this long without a stroke or a drag, so the pause it
/// costs does not land between two strokes of a run of them.
const QUIET: Duration = Duration::from_secs(1);

const FOLDER: &str = "recovery";
const RUNNING: &str = "running";
const COPY: &str = "copy";
const STOPPED: &str = "stopped";
const LOG: &str = "log.txt";
/// The log starts again once it has grown past this.
const LOG_LIMIT: u64 = 1 << 20;

// ---- what a copy is a copy of

/// The few facts kept beside a copy, so the question at start-up can say what
/// it is without reading a file that may be hundreds of megabytes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct About {
    /// Local time it was written, as `io::time_stamp` gives it.
    pub when: String,
    /// The file the scene was open from, if it had one.
    pub document: Option<PathBuf>,
    pub objects: usize,
    pub triangles: usize,
}

impl About {
    /// "dragon head.sclp", 3 objects, 2.1 million triangles.
    pub fn words(&self) -> String {
        if self.objects == 0 {
            // Nothing was noted beside the copy: say no more than is known.
            return "a scene".to_owned();
        }
        let name = match self.document.as_deref().and_then(Path::file_name) {
            Some(name) => format!("\"{}\"", name.to_string_lossy()),
            None => "a scene that was never saved".to_owned(),
        };
        let objects = if self.objects == 1 { "1 object".to_owned() } else { format!("{} objects", self.objects) };
        let triangles = match self.triangles {
            n if n >= 1_000_000 => format!("{:.1} million triangles", n as f64 / 1e6),
            n if n >= 1_000 => format!("{} thousand triangles", n / 1_000),
            n => format!("{n} triangles"),
        };
        format!("{name}, {objects}, {triangles}")
    }
}

/// A time stamp (`2026-10-10_143512`) as words for a sentence: "today at
/// 14:35", or "on 2026-10-09 at 14:35" for another day. `None` for anything
/// that is not a stamp.
pub fn when_words(stamp: &str, today: &str) -> Option<String> {
    let (date, time) = stamp.split_once('_')?;
    let digits = |text: &str, count: usize| text.len() == count && text.bytes().all(|b| b.is_ascii_digit());
    if date.len() != 10 || !digits(time, 6) {
        return None;
    }
    let clock = format!("{}:{}", &time[..2], &time[2..4]);
    let same_day = today.split_once('_').is_some_and(|(day, _)| day == date);
    Some(if same_day { format!("today at {clock}") } else { format!("on {date} at {clock}") })
}

/// A copy found on disk.
#[derive(Clone, Debug, PartialEq)]
pub struct Kept {
    pub file: PathBuf,
    /// Written as the app stopped on a fault: its last change may be half made.
    pub stopped: bool,
    pub about: About,
    modified: SystemTime,
}

// ---- the folders

/// This run's folder, and the lock that says the run is alive. The system
/// lets go of the lock when the program ends, however it ends.
struct Session {
    dir: PathBuf,
    _running: File,
}

/// Where recovery copies are kept, for this run and for the runs before it.
pub struct Store {
    /// The `recovery` folder; `None` where this run has nothing to do with copies.
    root: Option<PathBuf>,
    /// Whether this run may write and remove copies, or only look.
    writable: bool,
    session: Option<Session>,
    /// The number the last copy of this run was written under.
    number: u32,
}

/// The number in `copy-0007.sclp`, and whether it is a `stopped-` one.
fn parse_name(file: &Path) -> Option<(bool, u32)> {
    if file.extension()? != "sclp" {
        return None;
    }
    let (kind, number) = file.file_stem()?.to_str()?.split_once('-')?;
    let stopped = match kind {
        COPY => false,
        STOPPED => true,
        _ => return None,
    };
    Some((stopped, number.parse().ok()?))
}

/// The copies in one run's folder.
fn copies_in(dir: &Path) -> Vec<Kept> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut copies = Vec::new();
    for file in entries.flatten().map(|entry| entry.path()) {
        let Some((stopped, _)) = parse_name(&file) else { continue };
        let Ok(modified) = std::fs::metadata(&file).and_then(|m| m.modified()) else { continue };
        let about = std::fs::read_to_string(file.with_extension("json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        copies.push(Kept { file, stopped, about, modified });
    }
    copies
}

/// Whether a run still holds this folder. One that cannot be told either way
/// counts as running: its copies are then left alone rather than offered.
fn is_running(dir: &Path) -> bool {
    match File::open(dir.join(RUNNING)) {
        Ok(lock) => matches!(lock.try_lock(), Err(TryLockError::WouldBlock)),
        Err(e) => e.kind() != std::io::ErrorKind::NotFound,
    }
}

/// Removes a copy with what was written beside it.
fn remove_copy(file: &Path) {
    let _ = std::fs::remove_file(file);
    let _ = std::fs::remove_file(file.with_extension("json"));
    let _ = std::fs::remove_file(file.with_extension("sclp.tmp"));
}

/// Removes a run's folder once nothing of ours is left in it. Anything else
/// someone put there stays, and then so does the folder.
fn remove_folder(dir: &Path) {
    if copies_in(dir).is_empty() {
        let _ = std::fs::remove_file(dir.join(RUNNING));
        let _ = std::fs::remove_dir(dir);
    }
}

impl Store {
    /// Copies under `settings` (the folder the settings file is in).
    /// `writable` is whether this run may write there.
    pub fn at(settings: Option<&Path>, writable: bool) -> Store {
        Store { root: settings.map(|dir| dir.join(FOLDER)), writable, session: None, number: 0 }
    }

    /// Keeps nothing and finds nothing.
    pub fn nowhere() -> Store {
        Store::at(None, false)
    }

    /// Whether this run writes copies at all.
    pub fn keeps(&self) -> bool {
        self.root.is_some() && self.writable
    }

    /// This run's folder, made and locked the first time it is needed.
    fn session(&mut self) -> Result<&Path, String> {
        if self.session.is_none() {
            let root = self.root.as_ref().filter(|_| self.writable).ok_or("This run keeps no recovery copies.")?;
            let dir = root.join(format!("{}-{}", crate::io::time_stamp(), std::process::id()));
            let made = std::fs::create_dir_all(&dir).and_then(|()| File::create(dir.join(RUNNING)));
            let running = made.map_err(|e| format!("Could not make {}: {e}", dir.display()))?;
            // Nothing else has reason to hold a lock on a file made this instant.
            if let Err(e) = running.try_lock() {
                log::warn!("could not lock {}: {e}", dir.display());
            }
            self.session = Some(Session { dir, _running: running });
        }
        Ok(self.session.as_ref().map_or(Path::new(""), |session| session.dir.as_path()))
    }

    /// The file the next copy is to be written to: a new one, never one that
    /// is there. `stopped` is the copy made as the app stops on a fault.
    pub fn next_file(&mut self, stopped: bool) -> Result<PathBuf, String> {
        let dir = self.session()?.to_owned();
        self.number += 1;
        Ok(dir.join(format!("{}-{:04}.sclp", if stopped { STOPPED } else { COPY }, self.number)))
    }

    /// A copy has been written, whole, at `file`: notes what it is a copy of
    /// beside it, and drops the regular copies older than the newest
    /// [`KEEP`]. A `stopped` copy drops nothing: it is the one that may be
    /// damaged, so every regular copy stays beside it.
    pub fn wrote(&mut self, file: &Path, about: &About) {
        if let Ok(text) = serde_json::to_string_pretty(about) {
            // Only words for the question: the copy is good without them.
            let _ = crate::io::replace_file(&file.with_extension("json"), |w| w.write_all(text.as_bytes()));
        }
        let (Some((false, _)), Some(dir)) = (parse_name(file), file.parent()) else { return };
        let mut regular: Vec<(u32, PathBuf)> = copies_in(dir)
            .into_iter()
            .filter_map(|copy| parse_name(&copy.file).filter(|(stopped, _)| !stopped).map(|(_, n)| (n, copy.file)))
            .collect();
        regular.sort();
        let extra = regular.len().saturating_sub(KEEP);
        for (_, old) in regular.into_iter().take(extra) {
            remove_copy(&old);
        }
    }

    /// The copies left by runs that are no longer running, newest first.
    pub fn left_behind(&self) -> Vec<Kept> {
        let Some(root) = &self.root else { return Vec::new() };
        let Ok(entries) = std::fs::read_dir(root) else { return Vec::new() };
        let own = self.session.as_ref().map(|session| session.dir.as_path());
        let mut found = Vec::new();
        for dir in entries.flatten().map(|entry| entry.path()).filter(|dir| dir.is_dir()) {
            if Some(dir.as_path()) == own || is_running(&dir) {
                continue;
            }
            let copies = copies_in(&dir);
            if copies.is_empty() && self.writable {
                // A run that ended before it had anything to keep.
                remove_folder(&dir);
            }
            found.extend(copies);
        }
        found.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| b.file.cmp(&a.file)));
        found
    }

    /// Removes copies that [`Self::left_behind`] found, and their folders.
    /// Does nothing in a run that may only look.
    pub fn remove(&self, copies: &[Kept]) {
        if !self.writable {
            return;
        }
        for copy in copies {
            remove_copy(&copy.file);
        }
        for dir in copies.iter().filter_map(|copy| copy.file.parent()) {
            if !is_running(dir) {
                remove_folder(dir);
            }
        }
    }

    /// Removes this run's own copies and its folder: the scene is in a file
    /// of the user's now, or the app is closing properly. The next copy, if
    /// there is one, starts a new folder.
    pub fn clear(&mut self) {
        let Some(session) = self.session.take() else { return };
        for copy in copies_in(&session.dir) {
            remove_copy(&copy.file);
        }
        let dir = session.dir.clone();
        // The lock goes first: a locked file cannot be removed.
        drop(session);
        remove_folder(&dir);
    }
}

// ---- when a copy is due

/// Decides, once a frame, whether a copy should be written now.
pub struct Schedule {
    every: Duration,
    quiet: Duration,
    /// When work that no copy holds was first seen.
    waiting_since: Option<Instant>,
    /// The last frame in which a stroke or a drag was under way.
    last_busy: Option<Instant>,
    /// When to look again, if no input brings a frame before then.
    wake: Option<Instant>,
}

impl Schedule {
    pub fn new(every: Duration) -> Schedule {
        Schedule { every, quiet: QUIET.min(every / 2), waiting_since: None, last_busy: None, wake: None }
    }

    /// `changed`: the scene holds unsaved work that is in no copy yet.
    /// `busy`: a stroke or a drag is under way. True when a copy should be
    /// written in this frame: the work has waited its time and the hand is
    /// off the model, and has been for a moment. Work that has waited twice
    /// its time is copied in the first gap between strokes, however short.
    pub fn due(&mut self, now: Instant, changed: bool, busy: bool) -> bool {
        self.wake = None;
        if busy {
            self.last_busy = Some(now);
        }
        if !changed {
            self.waiting_since = None;
            return false;
        }
        let since = *self.waiting_since.get_or_insert(now);
        let waited = now.saturating_duration_since(since);
        if waited < self.every {
            self.wake = Some(since + self.every);
            return false;
        }
        if busy {
            // The end of the drag brings a frame of its own.
            return false;
        }
        let quiet_at = self.last_busy.map(|at| at + self.quiet).filter(|&at| at > now);
        match quiet_at {
            Some(at) if waited < self.every * 2 => {
                self.wake = Some(at);
                false
            }
            _ => true,
        }
    }

    /// A copy was written, or could not be: either way the wait starts again
    /// with the next look, so a disk that is full is not tried every frame.
    pub fn restart(&mut self) {
        self.waiting_since = None;
    }

    /// When [`Self::due`] should next be asked, if nothing happens before.
    pub fn wake(&self) -> Option<Instant> {
        self.wake
    }
}

// ---- a scene that can be trusted to a file

/// Whether these meshes can be written as a file that will open again: every
/// triangle whole and naming vertices that exist, every vertex somewhere.
/// After a fault the scene may have been caught mid-change; one that fails
/// here is not written, and the regular copies stand.
pub fn sound(meshes: &[&Mesh]) -> Result<(), String> {
    for (number, mesh) in meshes.iter().enumerate() {
        let verts = mesh.positions.len();
        if mesh.indices.is_empty() || mesh.indices.len() % 3 != 0 {
            return Err(format!("mesh {number} has {} triangle corners", mesh.indices.len()));
        }
        if mesh.indices.par_iter().any(|&corner| corner as usize >= verts) {
            return Err(format!("mesh {number} has a triangle on a vertex it does not have"));
        }
        if mesh.positions.par_iter().any(|position| !position.is_finite()) {
            return Err(format!("mesh {number} has a vertex that is nowhere"));
        }
    }
    Ok(())
}

/// A number that is the same for the same scene and, in practice, different
/// for a different one: every object's name, place and mesh, and every mesh's
/// vertices, masks and triangles. Opening a file puts a mesh's vertices and
/// triangles in a new order, so this does not depend on their order. For the
/// scripted checks, which compare a scene before a crash with the one
/// restored after it.
pub fn fingerprint(meshes: &[&Mesh], objects: &[SavedObject]) -> u64 {
    fn mix(mut x: u64) -> u64 {
        x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        x ^ (x >> 31)
    }
    let fold = |seed: u64, values: &[u32]| values.iter().fold(seed, |h, &v| mix(h ^ v as u64));
    let point = |p: glam::Vec3| fold(1, &[p.x.to_bits(), p.y.to_bits(), p.z.to_bits()]);
    let mut total = 0u64;
    for mesh in meshes {
        let verts: u64 = mesh
            .positions
            .par_iter()
            .zip(mesh.mask.par_iter())
            .map(|(&p, &m)| mix(point(p) ^ m.to_bits() as u64))
            .reduce(|| 0, u64::wrapping_add);
        let corner = |i: u32| mesh.positions.get(i as usize).map_or(0, |&p| point(p));
        let tris: u64 = mesh
            .indices
            .par_chunks_exact(3)
            .map(|t| {
                // The same triangle whichever corner it is listed from.
                let (a, b, c) = (corner(t[0]), corner(t[1]), corner(t[2]));
                let first = a.min(b).min(c);
                let turned = if first == a { [a, b, c] } else if first == b { [b, c, a] } else { [c, a, b] };
                mix(mix(mix(turned[0]) ^ turned[1]) ^ turned[2])
            })
            .reduce(|| 0, u64::wrapping_add);
        total = total.wrapping_add(mix(verts ^ mix(tris)));
    }
    for (row, o) in objects.iter().enumerate() {
        let (position, rotation, scale) = (o.position.to_array(), o.rotation.to_array(), o.scale.to_array());
        let place: Vec<u32> = [position.as_slice(), rotation.as_slice(), scale.as_slice()]
            .concat()
            .iter()
            .map(|v| v.to_bits())
            .collect();
        let name = o.name.bytes().fold(row as u64, |h, b| mix(h ^ b as u64));
        total = mix(total ^ fold(name ^ o.visible as u64, &place));
    }
    total
}

// ---- faults the graphics card reports

/// Where the graphics device's error callbacks leave word of a fault. They
/// may be called from any thread and can do nothing about the scene, so they
/// only note it; the interface thread finds the note between events, when
/// the scene is whole, keeps a copy and closes the app.
#[derive(Clone, Default)]
pub struct Faults(Arc<Mutex<Option<String>>>);

impl Faults {
    /// Notes a fault: `what` in words for the user, `details` as the device
    /// gave them, for the log. The first fault is the one that is kept: the
    /// rest follow from it.
    pub fn report(&self, what: &str, details: &str) {
        log::error!("{what}: {}", details.trim());
        self.0.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(|| what.to_owned());
    }

    /// The fault that was reported, once.
    pub fn take(&self) -> Option<String> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

// ---- stopping on a fault

/// What became of unsaved work when the app had to stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// There was none: the scene was as its file has it.
    NothingUnsaved,
    /// A copy of the scene as it stood was kept.
    Kept,
    /// The scene as it stood could not be kept, but an earlier copy was.
    EarlierOnly,
    /// Nothing could be kept.
    Lost,
}

/// What the user is told as the app closes on a fault. `cause` is a sentence
/// saying what happened.
pub fn farewell(cause: &str, outcome: Outcome, log: Option<&Path>) -> String {
    let work = match outcome {
        Outcome::NothingUnsaved => "Nothing had changed since the scene was last saved, so no work is lost.",
        Outcome::Kept => {
            "A copy of your work was kept. Start GolemSculpt again and it will offer to bring it back."
        }
        Outcome::EarlierOnly => {
            "The last few minutes of work could not be kept, but an earlier copy was. \
             Start GolemSculpt again and it will offer to bring it back."
        }
        Outcome::Lost => "Your unsaved work could not be kept.",
    };
    let details = log.map_or(String::new(), |log| format!("\n\nWhat went wrong is written in {}", log.display()));
    format!("{cause}\n\n{work}{details}")
}

// ---- the log

static LOG_FILE: OnceLock<Option<PathBuf>> = OnceLock::new();

/// The file warnings, errors and faults are written to, if this run has one.
pub fn log_file() -> Option<&'static Path> {
    LOG_FILE.get().and_then(|file| file.as_deref())
}

/// Adds lines to a log file under a line saying when. A log that has grown
/// past [`LOG_LIMIT`] starts again. Never fails: there is nowhere to report it.
pub fn write_log(file: &Path, text: &str) {
    if let Some(folder) = file.parent() {
        let _ = std::fs::create_dir_all(folder);
    }
    let grown = std::fs::metadata(file).is_ok_and(|m| m.len() > LOG_LIMIT);
    let opened = std::fs::OpenOptions::new().create(true).append(!grown).write(true).truncate(grown).open(file);
    if let Ok(mut log) = opened {
        let _ = writeln!(log, "[{}] {text}", crate::io::time_stamp());
    }
}

/// Writes to the log this run has, if it has one.
pub fn log(text: &str) {
    if let Some(file) = log_file() {
        write_log(file, text);
    }
}

/// What a panic said, where, and on which thread.
fn panic_report(info: &std::panic::PanicHookInfo) -> String {
    let payload = info.payload();
    let said = payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(no message)".to_owned());
    let place = info.location().map_or(String::new(), |at| format!(" at {}:{}", at.file(), at.line()));
    let thread = std::thread::current();
    format!("Fault in thread \"{}\"{place}: {said}", thread.name().unwrap_or("unnamed"))
}

/// Starts this run's log at `settings/log.txt` (nowhere for `None`) and has
/// every panic written to it, with the calls that led there, before the
/// panic goes on its way. The release build has no console, so without this
/// a fault leaves no trace at all.
///
/// The hook only writes. It runs on whichever thread panicked, while that
/// thread is still in the middle of what it was doing to the scene, so the
/// scene is not its to read: the copy of the scene is made afterwards, by
/// the interface thread, once the panic has unwound out of whatever was
/// changing it (`app/recovery.rs`).
pub fn start_log(settings: Option<&Path>) {
    let file = LOG_FILE.get_or_init(|| settings.map(|dir| dir.join(LOG))).clone();
    let before = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(file) = &file {
            let calls = std::backtrace::Backtrace::force_capture();
            write_log(file, &format!("{}\n{calls}", panic_report(info)));
        }
        before(info);
    }));
}

/// The app's logger: everything goes to the console as before (a release
/// build has none unless it was started from one), and warnings and errors
/// also go to the log file.
pub struct Logger {
    console: env_logger::Logger,
}

impl Logger {
    /// Makes this the logger for `log::warn!` and the rest.
    pub fn install() {
        let console = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).build();
        log::set_max_level(console.filter());
        let _ = log::set_boxed_logger(Box::new(Logger { console }));
    }
}

impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        self.console.enabled(metadata)
    }

    fn log(&self, record: &log::Record) {
        self.console.log(record);
        if record.level() <= log::Level::Warn && self.console.matches(record) {
            log(&format!("{} {}: {}", record.level(), record.target(), record.args()));
        }
    }

    fn flush(&self) {
        self.console.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::primitives::{self, Primitive};

    /// A settings folder of its own under the temp folder, empty.
    fn settings(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("sculpt_recovery_tests").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Writes a stand-in for a copy and tells the store, as the app does.
    fn keep(store: &mut Store, stopped: bool, text: &str) -> PathBuf {
        let file = store.next_file(stopped).unwrap();
        std::fs::write(&file, text).unwrap();
        store.wrote(&file, &About { objects: 1, ..About::default() });
        file
    }

    fn names(copies: &[Kept]) -> Vec<String> {
        copies.iter().map(|copy| copy.file.file_name().unwrap().to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn a_copy_goes_to_a_new_file_each_time_and_the_last_three_are_kept() {
        let dir = settings("keep_three");
        let mut store = Store::at(Some(&dir), true);
        let files: Vec<PathBuf> = (0..5).map(|i| keep(&mut store, false, &format!("scene {i}"))).collect();
        let mut distinct = files.clone();
        distinct.dedup();
        assert_eq!(distinct.len(), 5, "every copy has a file of its own");
        let there: Vec<bool> = files.iter().map(|file| file.is_file()).collect();
        assert_eq!(there, [false, false, true, true, true], "the newest {KEEP} stay");
        assert!(!files[0].with_extension("json").exists(), "a dropped copy takes its notes with it");
        assert_eq!(std::fs::read_to_string(&files[4]).unwrap(), "scene 4");
        assert_eq!(std::fs::read_to_string(&files[2]).unwrap(), "scene 2", "an older copy is never written over");
    }

    #[test]
    fn the_copy_made_on_a_fault_drops_no_regular_copy() {
        let dir = settings("stopped_beside");
        let mut store = Store::at(Some(&dir), true);
        let regular: Vec<PathBuf> = (0..3).map(|i| keep(&mut store, false, &format!("scene {i}"))).collect();
        let stopped = keep(&mut store, true, "half a stroke");
        assert!(regular.iter().all(|file| file.is_file()), "all {KEEP} regular copies are still there");
        assert!(stopped.is_file() && parse_name(&stopped) == Some((true, 4)));
        // And a regular copy after it still only counts regular ones.
        let next = keep(&mut store, false, "scene 3");
        assert!(stopped.is_file() && next.is_file() && !regular[0].exists() && regular[1].is_file());
    }

    #[test]
    fn copies_of_a_run_that_is_still_running_are_not_offered() {
        let dir = settings("still_running");
        let mut running = Store::at(Some(&dir), true);
        keep(&mut running, false, "scene");
        // A second copy of the app, started while the first is open.
        let second = Store::at(Some(&dir), true);
        assert!(second.left_behind().is_empty(), "the first run holds its folder");
        assert!(running.left_behind().is_empty(), "and a run does not offer itself its own");
        // The first run ends without a word; the system lets go of its lock.
        drop(running.session.take());
        assert_eq!(names(&second.left_behind()), ["copy-0001.sclp"], "now it is work a crash left behind");
    }

    #[test]
    fn copies_of_a_run_that_died_are_offered_newest_first() {
        let dir = settings("died");
        let mut died = Store::at(Some(&dir), true);
        let first = keep(&mut died, false, "scene 0");
        std::thread::sleep(Duration::from_millis(20));
        let last = keep(&mut died, true, "scene 1");
        // The program ending is what lets go of the lock; nothing is tidied.
        drop(died.session.take());
        let next = Store::at(Some(&dir), true);
        let found = next.left_behind();
        assert_eq!(names(&found), ["stopped-0002.sclp", "copy-0001.sclp"]);
        assert!(found[0].stopped && !found[1].stopped);
        assert_eq!(found[0].about.objects, 1, "what it is a copy of is read from beside it");
        assert_eq!((found[0].file.as_path(), found[1].file.as_path()), (last.as_path(), first.as_path()));
    }

    #[test]
    fn a_clean_end_leaves_nothing_to_offer() {
        let dir = settings("clean_end");
        let mut store = Store::at(Some(&dir), true);
        keep(&mut store, false, "scene");
        keep(&mut store, true, "scene");
        store.clear();
        assert!(Store::at(Some(&dir), true).left_behind().is_empty());
        let left: Vec<_> = std::fs::read_dir(dir.join(FOLDER)).unwrap().flatten().collect();
        assert!(left.is_empty(), "its folder has gone too: {left:?}");
        // The run goes on: its next copy starts afresh.
        assert!(keep(&mut store, false, "scene").is_file());
    }

    #[test]
    fn copies_that_were_answered_can_be_removed_and_a_run_that_only_looks_removes_nothing() {
        let dir = settings("answered");
        let mut died = Store::at(Some(&dir), true);
        keep(&mut died, false, "scene");
        drop(died.session.take());

        let looking = Store::at(Some(&dir), false);
        assert!(!looking.keeps());
        let found = looking.left_behind();
        assert_eq!(found.len(), 1);
        looking.remove(&found);
        assert!(found[0].file.is_file(), "a run that may not write leaves the copy where it is");

        let next = Store::at(Some(&dir), true);
        next.remove(&found);
        assert!(!found[0].file.exists() && next.left_behind().is_empty());
        assert!(!found[0].file.parent().unwrap().exists(), "the dead run's folder goes with its last copy");
    }

    #[test]
    fn a_run_with_no_settings_folder_keeps_and_finds_nothing() {
        let mut store = Store::nowhere();
        assert!(!store.keeps() && store.left_behind().is_empty());
        assert!(store.next_file(false).is_err());
        let dir = settings("read_only");
        let mut looking = Store::at(Some(&dir), false);
        assert!(looking.next_file(false).is_err(), "a run that may only look writes no copy");
        assert!(!dir.join(FOLDER).exists());
    }

    #[test]
    fn files_that_are_not_copies_are_neither_offered_nor_removed() {
        let dir = settings("strangers");
        let mut died = Store::at(Some(&dir), true);
        let copy = keep(&mut died, false, "scene");
        let folder = copy.parent().unwrap().to_owned();
        let (half, mine) = (folder.join("copy-0009.sclp.tmp"), folder.join("my own notes.txt"));
        std::fs::write(&half, "half written").unwrap();
        std::fs::write(&mine, "keep me").unwrap();
        drop(died.session.take());
        let next = Store::at(Some(&dir), true);
        let found = next.left_behind();
        assert_eq!(names(&found), ["copy-0001.sclp"], "a file still being written is not a copy");
        next.remove(&found);
        assert!(mine.is_file() && folder.is_dir(), "someone else's file, and so the folder, stay");
    }

    #[test]
    fn a_copy_waits_its_time_and_for_the_hand_to_be_off_the_model() {
        let every = Duration::from_secs(180);
        let mut schedule = Schedule::new(every);
        let start = Instant::now();
        let at = |seconds: f32| start + Duration::from_secs_f32(seconds);
        assert!(!schedule.due(at(0.0), false, false), "nothing has changed");
        assert_eq!(schedule.wake(), None);
        assert!(!schedule.due(at(10.0), true, false), "work has only just been seen");
        assert_eq!(schedule.wake(), Some(at(190.0)), "a frame is asked for when it falls due");
        assert!(!schedule.due(at(189.0), true, false));
        // Due, but a stroke is under way: not now, and not right as it ends.
        assert!(!schedule.due(at(190.0), true, true));
        assert_eq!(schedule.wake(), None, "the end of the stroke brings its own frame");
        assert!(!schedule.due(at(190.5), true, false), "the stroke ended half a second ago");
        assert_eq!(schedule.wake(), Some(at(191.0)));
        assert!(schedule.due(at(191.0), true, false), "a second after the stroke, it is written");
        schedule.restart();
        assert!(!schedule.due(at(192.0), true, false), "and the wait starts again");
        assert_eq!(schedule.wake(), Some(at(372.0)));
    }

    #[test]
    fn work_that_is_saved_in_the_meantime_is_not_copied() {
        let mut schedule = Schedule::new(Duration::from_secs(180));
        let start = Instant::now();
        let at = |seconds: u64| start + Duration::from_secs(seconds);
        assert!(!schedule.due(at(0), true, false));
        assert!(!schedule.due(at(100), false, false), "the user saved");
        assert!(!schedule.due(at(200), true, false), "new work waits its own three minutes");
        assert!(schedule.due(at(380), true, false));
    }

    #[test]
    fn strokes_one_after_another_cannot_put_a_copy_off_for_ever() {
        let mut schedule = Schedule::new(Duration::from_secs(180));
        let start = Instant::now();
        let at = |seconds: f32| start + Duration::from_secs_f32(seconds);
        assert!(!schedule.due(at(0.0), true, false));
        // A stroke every half second, with a gap of a tenth between them.
        let mut written = None;
        let mut time = 1.0;
        while time < 400.0 && written.is_none() {
            let _ = schedule.due(at(time), true, true);
            if schedule.due(at(time + 0.4), true, false) {
                written = Some(time + 0.4);
            }
            time += 0.5;
        }
        let written = written.expect("a copy is written in the end");
        assert!((360.0..361.0).contains(&written), "in the first gap once it is twice overdue: {written}");
    }

    #[test]
    fn the_shortest_interval_still_waits() {
        // The quiet time is part of the interval, never longer than it.
        let mut schedule = Schedule::new(Duration::from_millis(100));
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        assert!(!schedule.due(at(0), true, false));
        assert!(!schedule.due(at(100), true, true), "due, but a drag is under way");
        assert!(!schedule.due(at(120), true, false), "the drag ended this instant");
        assert_eq!(schedule.wake(), Some(at(150)), "half the interval is the longest it waits for quiet");
        assert!(schedule.due(at(150), true, false));
    }

    #[test]
    fn a_whole_scene_is_sound_and_a_damaged_one_is_not() {
        let whole = primitives::build(Primitive::Sphere, 2_000, 0.5);
        assert_eq!(sound(&[&whole]), Ok(()));

        let mut stray = primitives::build(Primitive::Sphere, 2_000, 0.5);
        stray.indices[7] = stray.positions.len() as u32;
        assert!(sound(&[&whole, &stray]).unwrap_err().contains("mesh 1"), "a corner past the last vertex");

        let mut nowhere = primitives::build(Primitive::Sphere, 2_000, 0.5);
        nowhere.positions[3].y = f32::NAN;
        assert!(sound(&[&nowhere]).is_err(), "a vertex that is not a number");

        let mut torn = primitives::build(Primitive::Sphere, 2_000, 0.5);
        torn.indices.pop();
        assert!(sound(&[&torn]).is_err(), "a triangle with two corners");
        torn.indices.clear();
        assert!(sound(&[&torn]).is_err(), "no triangles at all");
    }

    fn object(name: &str, mesh: usize) -> SavedObject {
        SavedObject {
            name: name.to_owned(),
            visible: true,
            mesh,
            position: glam::Vec3::ZERO,
            rotation: glam::Quat::IDENTITY,
            scale: glam::Vec3::ONE,
        }
    }

    #[test]
    fn a_fingerprint_follows_the_scene_through_a_file_and_tells_a_change() {
        let dir = settings("fingerprint");
        let mut mesh = primitives::build(Primitive::Sphere, 5_000, 0.5);
        // Moved about as sculpting moves them, so opening orders them anew.
        for (i, p) in mesh.positions.iter_mut().enumerate() {
            *p *= 1.0 + 0.2 * ((i * 7919) % 13) as f32 / 13.0;
        }
        mesh.mask[5] = 0.5;
        let objects = [object("Ball", 0)];
        let before = fingerprint(&[&mesh], &objects);
        let file = dir.join("scene.sclp");
        crate::io::save_scene(&[&mesh], &objects, 0, &file).unwrap();
        let opened = crate::io::load_scene(&file).unwrap();
        let meshes: Vec<&Mesh> = opened.meshes.iter().collect();
        assert_eq!(fingerprint(&meshes, &opened.objects), before, "the same scene after a save and an open");

        mesh.positions[11].x += 1e-4;
        assert_ne!(fingerprint(&[&mesh], &objects), before, "one vertex moved a hair");
        mesh.positions[11].x -= 1e-4;
        mesh.mask[5] = 0.25;
        assert_ne!(fingerprint(&[&mesh], &objects), before, "one vertex's mask");
        mesh.mask[5] = 0.5;
        assert_ne!(fingerprint(&[&mesh], &[object("Ball 2", 0)]), before, "a name");
        mesh.indices.swap(0, 1);
        assert_ne!(fingerprint(&[&mesh], &objects), before, "a triangle turned inside out");
    }

    /// What a recovery copy costs the interface thread: the check that the
    /// scene is whole (only made for the copy written on a fault) and the
    /// write itself, which is the save every copy goes through.
    /// `cargo test --release timing_of_a_recovery_copy -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn timing_of_a_recovery_copy() {
        let dir = settings("timing");
        for triangles in [1_000_000, 4_000_000, 12_000_000] {
            let mesh = primitives::build(Primitive::Sphere, triangles, 0.5);
            let objects = [object("Ball", 0)];
            let ms = |from: Instant| from.elapsed().as_secs_f64() * 1000.0;
            let start = Instant::now();
            sound(&[&mesh]).unwrap();
            let checked = ms(start);
            let mut writes = Vec::new();
            for round in 0..3 {
                let file = dir.join(format!("copy-{round}.sclp"));
                let start = Instant::now();
                crate::io::save_scene(&[&mesh], &objects, 0, &file).unwrap();
                writes.push(ms(start));
            }
            let size = std::fs::metadata(dir.join("copy-0.sclp")).unwrap().len() as f64 / 1e6;
            println!(
                "{:>9} triangles: check {checked:5.1} ms, write {:6.0} / {:6.0} / {:6.0} ms ({size:.0} MB)",
                mesh.triangle_count(),
                writes[0],
                writes[1],
                writes[2]
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_time_stamp_reads_as_words() {
        let today = "2026-10-10_160000";
        assert_eq!(when_words("2026-10-10_143512", today).as_deref(), Some("today at 14:35"));
        assert_eq!(when_words("2026-10-09_090501", today).as_deref(), Some("on 2026-10-09 at 09:05"));
        assert_eq!(when_words("", today), None);
        assert_eq!(when_words("yesterday_evening", today), None);
    }

    #[test]
    fn what_a_copy_holds_reads_as_words() {
        let mut about = About { objects: 3, triangles: 2_140_000, ..About::default() };
        assert_eq!(about.words(), "a scene that was never saved, 3 objects, 2.1 million triangles");
        about.document = Some(PathBuf::from("C:/sculpts/dragon head.sclp"));
        (about.objects, about.triangles) = (1, 20_480);
        assert_eq!(about.words(), "\"dragon head.sclp\", 1 object, 20 thousand triangles");
        about.triangles = 12;
        assert!(about.words().ends_with("12 triangles"));
        assert_eq!(About::default().words(), "a scene", "a copy with nothing noted beside it");
    }

    #[test]
    fn the_first_fault_reported_is_the_one_kept() {
        let faults = Faults::default();
        assert_eq!(faults.take(), None);
        let seen_elsewhere = faults.clone();
        let first = move || seen_elsewhere.report("The graphics card ran out of memory", "Not enough memory left");
        std::thread::spawn(first).join().unwrap();
        faults.report("The graphics card refused a drawing command", "and so did everything after it");
        assert_eq!(faults.take().as_deref(), Some("The graphics card ran out of memory"));
        assert_eq!(faults.take(), None, "it is acted on once");
    }

    #[test]
    fn the_farewell_says_what_became_of_the_work() {
        let log = PathBuf::from("C:/Users/me/AppData/Roaming/Sculpt/log.txt");
        let kept = farewell("GolemSculpt ran into a fault and has to close.", Outcome::Kept, Some(&log));
        assert!(kept.starts_with("GolemSculpt ran into a fault and has to close.\n\nA copy of your work was kept."));
        assert!(kept.contains("offer to bring it back") && kept.contains("log.txt"));
        assert!(farewell("It", Outcome::NothingUnsaved, None).contains("no work is lost"));
        assert!(farewell("It", Outcome::EarlierOnly, None).contains("an earlier copy was"));
        let lost = farewell("It", Outcome::Lost, None);
        assert!(lost.contains("could not be kept") && !lost.contains("written in"));
    }

    #[test]
    fn the_log_gathers_lines_and_starts_again_when_it_has_grown() {
        let file = settings("log").join("deeper").join(LOG);
        write_log(&file, "first");
        write_log(&file, "second\nwith the calls that led there");
        let text = std::fs::read_to_string(&file).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with('[') && lines[0].ends_with("] first") && lines[1].ends_with("] second"));

        std::fs::write(&file, vec![b'x'; LOG_LIMIT as usize + 1]).unwrap();
        write_log(&file, "after");
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.ends_with("] after\n") && text.len() < 100, "the old megabyte is gone");
    }
}
