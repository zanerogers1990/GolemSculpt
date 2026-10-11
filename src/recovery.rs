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
//! When a copy is removed. Never, unless one of these holds:
//!
//! - (a) the scene it holds has been saved, by this run, to a file of the
//!   user's: a run's own copies when its scene is saved, and an earlier
//!   run's when the scene that was restored from them is;
//! - (b) the user answered Discard It about that run's copies, and this run
//!   has since saved or closed properly;
//! - (c) the run that made it is closing properly, with nothing unsaved or
//!   with the question about closing answered.
//!
//! (A run also drops its own regular copies beyond the newest three as it
//! writes new ones.) Nothing else removes a copy: not restoring it, not this
//! run writing a copy of its own, not Not Now, not Undo, and not an answer
//! about another run's copies. Where it cannot be told, the copy stays and
//! the next start asks again. This file only has the means ([`Store::clear`]
//! for a run's own, [`Store::retire`] for a dead run's); `app/recovery.rs`
//! decides when, and its tests walk the rule through runs that crash.
//!
//! So nothing has to compare clocks or remember a clean exit: a copy that is
//! still there when the app starts, in a folder no running copy of the app
//! holds locked, is work that a crash left behind, and the app offers it.
//! Two copies of the app open at once each have a folder, and neither offers
//! the other's. If several runs left copies, each is asked about by itself,
//! the run that started last first; an answer touches that run's copies only.
//! Which copy is the newest goes by the names (the run's folder, the copy's
//! number), never by the dates of the files, which a backup or a plain copy
//! of the folder changes.
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
use std::time::{Duration, Instant};

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
/// How many times, and how far apart, a run tries to take hold of the folder
/// it has just made. Only another copy of the app looking at that folder in
/// the same instant can be in the way, and it lets go at once.
const HOLD_TRIES: u32 = 20;
const HOLD_PAUSE: Duration = Duration::from_millis(10);
/// How many folder names a run tries before it gives up keeping a copy for now.
const NAMES: u32 = 5;

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
    /// The number in its name. A run numbers its copies as it writes them.
    pub number: u32,
    pub about: About,
}

/// What one run that is no longer running left behind.
#[derive(Clone, Debug, PartialEq)]
pub struct Left {
    /// That run's folder.
    pub folder: PathBuf,
    /// Its copies, newest first; there is at least one.
    pub copies: Vec<Kept>,
}

impl Left {
    /// The newest copy: the one Restore It brings back.
    pub fn newest(&self) -> Option<&Kept> {
        self.copies.first()
    }

    /// Where the newest copy was made as the app stopped on a fault: the
    /// last regular copy before it, to offer instead.
    pub fn earlier(&self) -> Option<&Kept> {
        self.newest().filter(|newest| newest.stopped)?;
        self.copies.iter().find(|kept| !kept.stopped)
    }

    /// The copies to try for an answer, in order: from the newest, or
    /// (`earlier`) from the newest regular one. One that cannot be read is
    /// passed over for the one before it.
    pub fn to_try(&self, earlier: bool) -> &[Kept] {
        let first = if earlier { self.copies.iter().position(|kept| !kept.stopped).unwrap_or(0) } else { 0 };
        &self.copies[first.min(self.copies.len())..]
    }
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
    /// The name for this run's folder where a test has given it one.
    name: Option<String>,
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

/// The copies in one run's folder, newest first. The newest is the one with
/// the highest number, and a copy made as the app stopped comes after every
/// regular copy of its run: it was the last thing the run did. The dates of
/// the files say nothing here; copying the folder gives them all new ones.
fn copies_in(dir: &Path) -> Vec<Kept> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut copies = Vec::new();
    for file in entries.flatten().map(|entry| entry.path()).filter(|file| file.is_file()) {
        let Some((stopped, number)) = parse_name(&file) else { continue };
        let about = std::fs::read_to_string(file.with_extension("json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        copies.push(Kept { file, stopped, number, about });
    }
    copies.sort_by(|a, b| (b.stopped, b.number).cmp(&(a.stopped, a.number)).then_with(|| b.file.cmp(&a.file)));
    copies
}

/// A piece of a folder's name, for putting runs in order.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum Part {
    Number(u64),
    Text(String),
}

/// A run's folder as something to order runs by: its name (when the run
/// started, its process number, and a count where that name was taken) with
/// each run of digits read as a number, so that `-10` comes after `-9`.
fn run_order(dir: &Path) -> Vec<Part> {
    let name = dir.file_name().unwrap_or_default().to_string_lossy();
    let mut parts: Vec<Part> = Vec::new();
    for c in name.chars() {
        match (c.to_digit(10), parts.last_mut()) {
            (Some(digit), Some(Part::Number(n))) => *n = n.saturating_mul(10).saturating_add(digit as u64),
            (Some(digit), _) => parts.push(Part::Number(digit as u64)),
            (None, Some(Part::Text(text))) => text.push(c),
            (None, _) => parts.push(Part::Text(c.to_string())),
        }
    }
    parts
}

/// Whether the answer to "may I lock this run's file?" leaves it possible
/// that the run is alive. Only a lock that was given says it is not. One
/// that was refused says it is, and an error of any other kind says nothing,
/// which has to count as alive: copies that may be a running app's are
/// neither offered nor removed.
fn may_be_running(tried: &Result<(), TryLockError>) -> bool {
    tried.is_err()
}

/// What a look at a run's folder found.
enum Probe {
    /// A run holds it, or it cannot be told.
    Running,
    /// No run holds it. The lock is this look's own for as long as it is
    /// kept (`None` where the folder has no `running` file to lock).
    Dead(Option<File>),
}

fn probe(dir: &Path) -> Probe {
    match File::open(dir.join(RUNNING)) {
        Ok(lock) if may_be_running(&lock.try_lock()) => Probe::Running,
        Ok(lock) => Probe::Dead(Some(lock)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Probe::Dead(None),
        Err(_) => Probe::Running,
    }
}

/// Removes a copy with what was written beside it.
fn remove_copy(file: &Path) {
    let _ = std::fs::remove_file(file);
    let _ = std::fs::remove_file(file.with_extension("json"));
}

/// Whether `file` is something of ours that is not a copy and is of no use
/// without one: what a write that was cut short left behind
/// (`copy-0007.sclp.tmp`; a run killed in the middle of writing never comes
/// back to finish or remove it), or the notes of a copy that has gone.
fn is_leftover(file: &Path) -> bool {
    let ours = |file: &Path| parse_name(&file.with_extension("sclp")).is_some();
    let kind = |file: &Path| file.extension().and_then(|kind| kind.to_str()).map(str::to_owned);
    match kind(file).as_deref() {
        Some("tmp") => {
            let whole = file.with_extension("");
            matches!(kind(&whole).as_deref(), Some("sclp" | "json")) && ours(&whole)
        }
        Some("json") => ours(file) && !file.with_extension("sclp").exists(),
        _ => false,
    }
}

/// Removes a run's folder once no copy is left in it, with what a cut-short
/// write left there. Anything else someone put there stays, and then so does
/// the folder. `lock` is the lock on its `running` file, if the caller has it.
fn remove_folder(dir: &Path, lock: Option<File>) {
    if !copies_in(dir).is_empty() {
        return;
    }
    let files = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|entry| entry.path());
    for file in files.filter(|file| is_leftover(file)) {
        let _ = std::fs::remove_file(file);
    }
    // While the lock is still held: a run that made this folder a moment ago
    // and has not taken hold of it yet then finds its `running` file gone,
    // and takes another folder (`claim`).
    let _ = std::fs::remove_file(dir.join(RUNNING));
    drop(lock);
    // And once more, for a system that will not remove a locked file.
    let _ = std::fs::remove_file(dir.join(RUNNING));
    let _ = std::fs::remove_dir(dir);
}

/// Takes the lock on a `running` file this run has just made, trying for a
/// moment if another copy of the app is looking at it. False if it could
/// not be had.
fn hold(running: &File) -> bool {
    (0..HOLD_TRIES).any(|round| {
        if round > 0 {
            std::thread::sleep(HOLD_PAUSE);
        }
        running.try_lock().is_ok()
    })
}

/// Makes `dir`, which must not be there yet, and takes hold of it for this
/// run. A folder this run does not hold is not one to write copies into:
/// another copy of the app would take them for work a crash left behind. So
/// one that cannot be held is given up, and the caller tries another name.
fn claim(dir: &Path, hold: impl Fn(&File) -> bool) -> Result<Session, String> {
    let made = std::fs::create_dir(dir).and_then(|()| File::create(dir.join(RUNNING)));
    let running = made.map_err(|e| format!("Could not make {}: {e}", dir.display()))?;
    // Whoever was looking may have taken the folder for an empty one that a
    // run left behind, and removed it: the file must still be there.
    if hold(&running) && dir.join(RUNNING).is_file() {
        return Ok(Session { dir: dir.to_owned(), _running: running });
    }
    drop(running);
    let _ = std::fs::remove_file(dir.join(RUNNING));
    let _ = std::fs::remove_dir(dir);
    Err(format!("Could not take hold of {} for this run's recovery copies.", dir.display()))
}

impl Store {
    /// Copies under `settings` (the folder the settings file is in).
    /// `writable` is whether this run may write there.
    pub fn at(settings: Option<&Path>, writable: bool) -> Store {
        Store { root: settings.map(|dir| dir.join(FOLDER)), writable, session: None, number: 0, name: None }
    }

    /// Keeps nothing and finds nothing.
    pub fn nowhere() -> Store {
        Store::at(None, false)
    }

    /// For tests: this run's folder is to be called `name`. (A real run's is
    /// named for the second it started in and its process number, which
    /// would give every run in a test the same name.)
    #[cfg(test)]
    pub fn named(mut self, name: &str) -> Store {
        self.name = Some(name.to_owned());
        self
    }

    /// For tests: the program ends without a word. The system lets go of the
    /// lock, and nothing is tidied.
    #[cfg(test)]
    pub fn crash(&mut self) {
        drop(self.session.take());
    }

    /// Whether this run writes copies at all.
    pub fn keeps(&self) -> bool {
        self.root.is_some() && self.writable
    }

    /// This run's folder, made and locked the first time it is needed: a new
    /// one, never a folder that is there already (an earlier run's copies
    /// are numbered from one as well).
    fn session(&mut self) -> Result<&Path, String> {
        if self.session.is_none() {
            let root = self.root.as_ref().filter(|_| self.writable).ok_or("This run keeps no recovery copies.")?;
            std::fs::create_dir_all(root).map_err(|e| format!("Could not make {}: {e}", root.display()))?;
            let stamped = || format!("{}-{}", crate::io::time_stamp(), std::process::id());
            let name = self.name.clone().unwrap_or_else(stamped);
            let mut failed = String::new();
            let session = (1..=NAMES).find_map(|round| {
                let dir = root.join(if round == 1 { name.clone() } else { format!("{name}-{round}") });
                claim(&dir, hold).map_err(|e| failed = e).ok()
            });
            if !failed.is_empty() {
                log::warn!("{failed}");
            }
            self.session = Some(session.ok_or(failed)?);
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
        // Newest first, so the ones past the first `KEEP` are the oldest.
        let regular = copies_in(dir).into_iter().filter(|copy| !copy.stopped);
        for old in regular.skip(KEEP) {
            remove_copy(&old.file);
        }
    }

    /// What the runs that are no longer running left behind: one entry for
    /// each run that left a copy, the run that started last first.
    pub fn left_behind(&self) -> Vec<Left> {
        let Some(root) = &self.root else { return Vec::new() };
        let Ok(entries) = std::fs::read_dir(root) else { return Vec::new() };
        let own = self.session.as_ref().map(|session| session.dir.as_path());
        let mut found = Vec::new();
        for folder in entries.flatten().map(|entry| entry.path()).filter(|dir| dir.is_dir()) {
            if Some(folder.as_path()) == own {
                continue;
            }
            let Probe::Dead(lock) = probe(&folder) else { continue };
            let copies = copies_in(&folder);
            if !copies.is_empty() {
                found.push(Left { folder, copies });
            } else if self.writable {
                // A run that ended before it had a whole copy to keep.
                remove_folder(&folder, lock);
            }
        }
        found.sort_by(|a, b| run_order(&b.folder).cmp(&run_order(&a.folder)));
        found
    }

    /// Removes everything a run that is no longer running left: its copies,
    /// what a cut-short write left beside them, and its folder. When that
    /// may be done is the rule at the top of this file, and the caller's to
    /// keep. Does nothing in a run that may only look, and nothing to a
    /// folder that is not a run's of this store, that is this run's own, or
    /// that a run may still hold.
    pub fn retire(&self, folder: &Path) {
        let a_run_here = self.root.as_deref().is_some_and(|root| folder.parent() == Some(root));
        let own = self.session.as_ref().is_some_and(|session| session.dir == folder);
        if !self.writable || !a_run_here || own {
            return;
        }
        let Probe::Dead(lock) = probe(folder) else { return };
        for copy in copies_in(folder) {
            remove_copy(&copy.file);
        }
        remove_folder(folder, lock);
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
        // The lock goes first: nobody else has reason to take it, and the
        // folder cannot go while its file is held open.
        drop(session);
        remove_folder(&dir, None);
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

/// Whether these meshes can be written as a file at all: every triangle
/// whole, and naming vertices that exist. After a fault the scene may have
/// been caught mid-change, and a mesh whose triangle list was cut short or
/// names a vertex that is not there would be written as garbage, or bring
/// the app down when it is opened; such a scene is not written, and the
/// regular copies stand.
///
/// This is the last chance to keep the work, so it refuses nothing that an
/// ordinary Save would write. A vertex that is not a number (a brush can
/// make one) is not a reason: Save mends those as it writes, and the same
/// code writes this copy. Nor is a mesh with no triangles at all: nothing in
/// it can be wrong, a save writes it and it opens again
/// (`an_empty_mesh_is_saved_and_opened_like_any_other`), and refusing the
/// copy for one empty object would lose every other object with it.
pub fn sound(meshes: &[&Mesh]) -> Result<(), String> {
    for (number, mesh) in meshes.iter().enumerate() {
        let (verts, corners) = (mesh.positions.len(), mesh.indices.len());
        if corners % 3 != 0 {
            return Err(format!("mesh {number} has {corners} triangle corners, which is not a whole number of triangles"));
        }
        if mesh.indices.par_iter().any(|&corner| corner as usize >= verts) {
            return Err(format!("mesh {number} has a triangle on a vertex it does not have"));
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

    /// A run called `name` that wrote `copies` regular copies and died.
    fn dead_run(dir: &Path, name: &str, copies: usize) -> PathBuf {
        let mut run = Store::at(Some(dir), true).named(name);
        for i in 0..copies {
            keep(&mut run, false, &format!("{name} scene {i}"));
        }
        let folder = run.session.as_ref().unwrap().dir.clone();
        run.crash();
        folder
    }

    /// The folders of the runs that left copies, as `left_behind` orders them.
    fn runs(left: &[Left]) -> Vec<String> {
        left.iter().map(|run| run.folder.file_name().unwrap().to_string_lossy().into_owned()).collect()
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
        let copy = keep(&mut running, false, "scene");
        // A second copy of the app, started while the first is open.
        let second = Store::at(Some(&dir), true);
        assert!(second.left_behind().is_empty(), "the first run holds its folder");
        assert!(running.left_behind().is_empty(), "and a run does not offer itself its own");
        second.retire(copy.parent().unwrap());
        assert!(copy.is_file(), "nor can another run remove what a running one holds");
        // The first run ends without a word; the system lets go of its lock.
        running.crash();
        let left = second.left_behind();
        assert_eq!(left.len(), 1, "now it is work a crash left behind");
        assert_eq!(names(&left[0].copies), ["copy-0001.sclp"]);
    }

    #[test]
    fn copies_of_a_run_that_died_are_offered_newest_first() {
        let dir = settings("died");
        let mut died = Store::at(Some(&dir), true);
        let first = keep(&mut died, false, "scene 0");
        let last = keep(&mut died, true, "scene 1");
        // The program ending is what lets go of the lock; nothing is tidied.
        died.crash();
        let next = Store::at(Some(&dir), true);
        let left = next.left_behind();
        let found = &left[0].copies;
        assert_eq!(names(found), ["stopped-0002.sclp", "copy-0001.sclp"]);
        assert!(found[0].stopped && !found[1].stopped);
        assert_eq!(found[0].about.objects, 1, "what it is a copy of is read from beside it");
        assert_eq!((found[0].file.as_path(), found[1].file.as_path()), (last.as_path(), first.as_path()));
        assert_eq!(left[0].newest(), Some(&found[0]));
        assert_eq!(left[0].earlier(), Some(&found[1]), "the regular copy is the one to offer instead");
        assert_eq!(names(left[0].to_try(false)), ["stopped-0002.sclp", "copy-0001.sclp"]);
        assert_eq!(names(left[0].to_try(true)), ["copy-0001.sclp"]);
    }

    /// Review point 7: the dates of the files decided which copy was the
    /// newest, and a plain copy of the folder gives them all new dates.
    #[test]
    fn the_newest_copy_goes_by_its_number_and_not_by_the_date_of_its_file() {
        let dir = settings("dates");
        let mut died = Store::at(Some(&dir), true);
        let files: Vec<PathBuf> = (0..3).map(|i| keep(&mut died, false, &format!("scene {i}"))).collect();
        let stopped = keep(&mut died, true, "scene 3");
        died.crash();
        // As a copy of the folder, or a backup put back, might leave them: the
        // oldest copy with the latest date, the newest with the earliest.
        let day = Duration::from_secs(86_400);
        let now = std::time::SystemTime::now();
        let dated = |file: &Path, at| File::options().write(true).open(file).unwrap().set_modified(at).unwrap();
        dated(&files[0], now);
        dated(&files[1], now - day);
        dated(&files[2], now - day * 2);
        dated(&stopped, now - day * 3);
        let left = Store::at(Some(&dir), true).left_behind();
        let order = ["stopped-0004.sclp", "copy-0003.sclp", "copy-0002.sclp", "copy-0001.sclp"];
        assert_eq!(names(&left[0].copies), order, "by number, the copy made as the app stopped last of all");
        assert_eq!(left[0].earlier().map(|kept| kept.number), Some(3));
    }

    /// Review point 3: every dead run's copies were one list.
    #[test]
    fn each_run_that_died_is_found_by_itself_and_the_last_to_start_comes_first() {
        let dir = settings("several_runs");
        // Started in this order; the names are as a real run's, with the
        // count that a name already taken is given.
        let second = "2026-10-10_101500-41";
        let (ninth, tenth) = (format!("{second}-9"), format!("{second}-10"));
        let order = ["2026-10-09_230000-777", second, ninth.as_str(), tenth.as_str()];
        for (i, name) in order.iter().enumerate() {
            dead_run(&dir, name, i + 1);
        }
        let left = Store::at(Some(&dir), true).left_behind();
        let newest_first: Vec<&str> = order.iter().rev().copied().collect();
        assert_eq!(runs(&left), newest_first);
        for (run, copies) in left.iter().zip([4, 3, 2, 1]) {
            assert_eq!(run.copies.len(), copies.min(KEEP), "{}: its own copies and no other run's", run.folder.display());
            assert!(run.copies.iter().all(|kept| kept.file.parent() == Some(run.folder.as_path())));
        }
    }

    #[test]
    fn retiring_one_run_leaves_every_other_run_as_it_was() {
        let dir = settings("retire_one");
        let folders: Vec<PathBuf> = ["run-1", "run-2", "run-3"].iter().map(|name| dead_run(&dir, name, 2)).collect();
        let next = Store::at(Some(&dir), true);
        next.retire(&folders[1]);
        assert!(!folders[1].exists(), "the run that was answered about has gone, folder and all");
        assert_eq!(runs(&next.left_behind()), ["run-3", "run-1"]);
        let files = ["copy-0001.sclp", "copy-0002.sclp", "copy-0001.json", RUNNING];
        let whole = |folder: &Path| files.iter().all(|file| folder.join(file).is_file());
        assert!(whole(&folders[0]) && whole(&folders[2]), "nothing of another run is touched");
        // A folder that is not a run's of this store is not this store's to remove.
        let elsewhere = dir.join("somewhere else");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("copy-0001.sclp"), "not ours").unwrap();
        next.retire(&elsewhere);
        assert!(elsewhere.join("copy-0001.sclp").is_file());
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
    fn a_run_that_was_answered_about_can_be_retired_and_a_run_that_only_looks_removes_nothing() {
        let dir = settings("answered");
        let folder = dead_run(&dir, "died", 1);
        let copy = folder.join("copy-0001.sclp");

        let looking = Store::at(Some(&dir), false);
        assert!(!looking.keeps());
        assert_eq!(looking.left_behind().len(), 1);
        looking.retire(&folder);
        assert!(copy.is_file(), "a run that may not write leaves the copy where it is");

        let next = Store::at(Some(&dir), true);
        next.retire(&folder);
        assert!(!copy.exists() && next.left_behind().is_empty());
        assert!(!folder.exists(), "the dead run's folder goes with its last copy");
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
        let theirs = [folder.join("holiday.sclp"), folder.join("holiday.sclp.tmp"), folder.join("copy.json")];
        theirs.iter().for_each(|file| std::fs::write(file, "keep me too").unwrap());
        died.crash();
        let next = Store::at(Some(&dir), true);
        let left = next.left_behind();
        assert_eq!(names(&left[0].copies), ["copy-0001.sclp"], "a file still being written is not a copy");
        next.retire(&folder);
        assert!(mine.is_file() && folder.is_dir(), "someone else's file, and so the folder, stay");
        assert!(theirs.iter().all(|file| file.is_file()), "and whatever else is not named as a copy of ours is");
        assert!(!copy.exists() && !half.exists(), "the copy has gone, and the half-written one with it");
    }

    /// Review point 7: a write cut short by the program being killed left a
    /// `.tmp` that nothing removed, and its folder stayed for ever.
    #[test]
    fn what_a_write_that_was_cut_short_left_goes_with_its_run() {
        let dir = settings("cut_short");
        let folder = dead_run(&dir, "killed mid-write", 2);
        let halves = ["copy-0003.sclp.tmp", "copy-0003.json.tmp", "stopped-0004.sclp.tmp"].map(|name| folder.join(name));
        halves.iter().for_each(|file| std::fs::write(file, "half").unwrap());
        // A run killed while it wrote its very first copy: nothing whole to offer.
        let first = dir.join(FOLDER).join("killed at once");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::write(first.join(RUNNING), "").unwrap();
        std::fs::write(first.join("copy-0001.sclp.tmp"), "half").unwrap();

        let next = Store::at(Some(&dir), true);
        let left = next.left_behind();
        assert_eq!(runs(&left), ["killed mid-write"], "only the run with whole copies is offered");
        assert!(!first.exists(), "the folder with nothing but a half-written file has gone");
        assert!(halves.iter().all(|file| file.is_file()), "while its copies stay, nothing in a run's folder is touched");
        next.retire(&folder);
        assert!(!folder.exists(), "and when they are retired the folder goes, half-written files and all");
    }

    /// Review point 7: an error from asking for the lock, other than "it is
    /// held", counted as "nobody holds it".
    #[test]
    fn a_lock_that_cannot_be_asked_about_counts_as_a_run_that_may_be_alive() {
        assert!(!may_be_running(&Ok(())), "the lock was given: nobody holds the folder");
        assert!(may_be_running(&Err(TryLockError::WouldBlock)), "a run holds it");
        let unknown = TryLockError::Error(std::io::Error::other("the disk did not answer"));
        assert!(may_be_running(&Err(unknown)), "not known: its copies are neither offered nor removed");
    }

    /// Review point 7: a run that could not lock its new folder went on to
    /// write copies into it all the same.
    #[test]
    fn a_run_takes_a_folder_of_its_own_and_never_one_that_is_there() {
        let dir = settings("folder_taken");
        // Another run's folder under the name this run would use, with a copy in it.
        let theirs = dead_run(&dir, "same name", 1);
        let their_copy = theirs.join("copy-0001.sclp");
        let mut run = Store::at(Some(&dir), true).named("same name");
        let mine = keep(&mut run, false, "mine");
        assert_eq!(mine.parent().unwrap().file_name().unwrap(), "same name-2", "a folder of its own");
        assert_eq!(std::fs::read_to_string(&their_copy).unwrap(), "same name scene 0", "theirs is not written over");
        let left = run.left_behind();
        assert_eq!(runs(&left), ["same name"], "and its own is held: only the other is a dead run's");
        assert!(Store::at(Some(&dir), true).left_behind().iter().all(|left| left.folder != mine.parent().unwrap()));
    }

    #[test]
    fn taking_hold_of_a_folder_waits_out_a_passing_look_and_gives_up_on_one_that_is_held() {
        let dir = settings("hold");
        let file = dir.join(RUNNING);
        let running = File::create(&file).unwrap();
        // Another copy of the app looks at the folder for a moment (`probe`).
        let look = File::open(&file).unwrap();
        look.try_lock().unwrap();
        assert!(running.try_lock().is_err(), "the first try is refused");
        let passing = std::thread::spawn(move || {
            std::thread::sleep(HOLD_PAUSE * 3);
            drop(look);
        });
        assert!(hold(&running), "it is tried again until the look has passed");
        passing.join().unwrap();
        drop(running);

        // One that never lets go: given up, with nothing left behind to mistake for a run.
        let folder = dir.join("never mine");
        let refused = claim(&folder, |_| false);
        assert!(refused.is_err_and(|e| e.contains("Could not take hold of")));
        assert!(!folder.exists(), "the folder it could not hold is not left lying");
        // And the store then keeps its copies under another name.
        let held = claim(&folder, hold).unwrap();
        assert!(matches!(probe(&folder), Probe::Running), "a folder that is held reads as a run that is alive");
        drop(held);
        assert!(matches!(probe(&folder), Probe::Dead(Some(_))));
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

        let mut torn = primitives::build(Primitive::Sphere, 2_000, 0.5);
        torn.indices.pop();
        assert!(sound(&[&torn]).is_err(), "a triangle with two corners");
    }

    /// Review point 4: the copy made as the app stops was refused for a
    /// vertex that is not a number, and for a mesh with no triangles, though
    /// an ordinary Save writes both.
    #[test]
    fn the_last_copy_is_not_refused_for_what_an_ordinary_save_writes() {
        let dir = settings("sound_as_save");
        let mut nowhere = primitives::build(Primitive::Sphere, 2_000, 0.5);
        nowhere.positions[3].y = f32::NAN;
        nowhere.positions[9] = glam::Vec3::splat(f32::INFINITY);
        assert_eq!(sound(&[&nowhere]), Ok(()), "a vertex that is not a number");
        crate::io::save_scene(&[&nowhere], &[object("Ball", 0)], 0, &dir.join("nowhere.sclp")).expect("Save writes it");

        let mut empty = primitives::build(Primitive::Sphere, 2_000, 0.5);
        empty.indices.clear();
        let whole = primitives::build(Primitive::Sphere, 2_000, 0.5);
        assert_eq!(sound(&[&whole, &empty]), Ok(()), "no triangles at all");
    }

    #[test]
    fn an_empty_mesh_is_saved_and_opened_like_any_other() {
        let dir = settings("empty_mesh");
        let empty = Mesh::new(Vec::new(), Vec::new(), None);
        let whole = primitives::build(Primitive::Sphere, 2_000, 0.5);
        let objects = [object("Ball", 0), object("Nothing", 1)];
        let file = dir.join("with an empty object.sclp");
        crate::io::save_scene(&[&whole, &empty], &objects, 0, &file).expect("Save writes it");
        let opened = crate::io::load_scene(&file).expect("and it opens again");
        assert_eq!(opened.objects.len(), 2);
        assert_eq!((opened.meshes[0].triangle_count(), opened.meshes[1].triangle_count()), (whole.triangle_count(), 0));
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
