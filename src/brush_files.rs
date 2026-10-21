//! Brushes as files: the user's own brushes, one small JSON file each in a
//! `brushes` folder beside the settings file, and the same file as what
//! Export Brush writes and Import Brush reads.
//!
//! A brush file is plain JSON with its fields by name, so it can be read,
//! edited and passed to a friend:
//!
//! ```json
//! { "name": "Blocking In", "kernel": "Clay",
//!   "brush": { "strength": 0.5, "spacing": 0.3, "alpha": { "name": "Square" }, "clay_depth": 0.3 },
//!   "made":  { ... } }
//! ```
//!
//! `brush` is the brush as it is now and `made` as it was when it was made,
//! which is what Reset goes back to. Reading is as forgiving as the settings
//! file (`prefs.rs`): a field the file lacks keeps the value of the kernel's
//! base brush, one this version does not know is ignored, one of the wrong
//! kind is passed over on its own, and numbers are brought into the range
//! their sliders allow. A file that is not a brush at all (not JSON, or
//! naming no kernel) is skipped and said so, and is never written over.
//!
//! What goes where:
//!  - the fourteen base brushes stay in `settings.json`, exactly as before
//!    there was a library, so a settings file of any age still means what
//!    it meant ([`Kept`] is the little the library adds to it);
//!  - a brush of the user's own has a file here;
//!  - a shipped brush has one only while it differs from how it shipped.
//!
//! The [`Shelf`] is the folder as the app last left it. It writes a file
//! when its brush has changed and at no other time, so a file edited by
//! hand, or holding fields from a newer version, is left as it is until the
//! brush itself is changed in the app.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::prefs::{AlphaKept, BrushTuning};
use crate::sculpt::library::{Library, Origin, Record};
use crate::sculpt::{Brush, BrushSettings, FalloffCurve, PoseMode, Settings};

/// The folder's name, beside the settings file.
pub const FOLDER: &str = "brushes";
/// Where a deleted brush's file is put, inside that folder: Delete in the
/// panel has no question to answer, so it must not be the end of a brush.
const DELETED: &str = "deleted";
const EXTENSION: &str = "json";
/// No brush file comes near this. Anything bigger is some other file, and
/// is not read into memory to find that out.
const MAX_BYTES: u64 = 1 << 20;

/// What the settings file remembers of the library besides the base
/// brushes' own settings: each brush by its name.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Kept {
    /// The brush in hand. Empty in a file from before there was a library:
    /// the kernel it names (`BrushPrefs::brush`) is then the base brush.
    /// With no settings file at all it is the brush a fresh app starts with
    /// (`Prefs::fresh`).
    pub selected: String,
    /// The brush Shift smooths with.
    pub shift: String,
    /// The brushes on the hold-Space wheel. `None` in a file from before
    /// there was a library, which leaves the pins as they start.
    pub pinned: Option<Vec<String>>,
    /// Shipped brushes the user has deleted.
    pub removed: Vec<String>,
}

impl Kept {
    pub fn of(library: &Library, s: &Settings) -> Kept {
        let entries = library.entries();
        Kept {
            selected: entries[library.selected(s)].name.clone(),
            shift: entries[library.shift()].name.clone(),
            pinned: Some(entries.iter().filter(|e| e.pinned).map(|e| e.name.clone()).collect()),
            removed: library.removed().to_vec(),
        }
    }

    /// Start-up, after the settings file's brush settings are in place and
    /// the brush folder has been read.
    pub fn apply(&self, library: &mut Library, s: &mut Settings) {
        library.restore(&self.removed, self.pinned.as_deref(), &self.shift, &self.selected, s);
    }
}

/// A brush's settings as a file keeps them: `BrushSettings` field by field,
/// the falloff curve's points, the alpha by its picture's name, and the
/// numbers of the brush's own kernel (the others are not in the file).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct RecordKept {
    #[serde(flatten)]
    tuning: BrushTuning,
    #[serde(skip_serializing_if = "Option::is_none")]
    falloff: Option<Vec<[f32; 2]>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    alpha: Option<AlphaKept>,
    #[serde(skip_serializing_if = "Option::is_none")]
    clay_depth: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    clay_softness: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    clay_relax: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    crease_pinch: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    crease_depth: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    flatten_offset: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    layer_height: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    move_connected: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pose_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pose_joints: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pose_offset: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pose_rigidity: Option<f32>,
}

impl RecordKept {
    fn of(record: &Record, kernel: Brush) -> RecordKept {
        let n = &record.numbers;
        let own = |of: Brush| kernel == of;
        RecordKept {
            tuning: BrushTuning::of(&record.settings),
            falloff: record.falloff.as_ref().map(|curve| curve.points.iter().map(|p| p.to_array()).collect()),
            alpha: record.alpha.image.is_some().then(|| AlphaKept::of(&record.alpha)),
            clay_depth: own(Brush::Clay).then_some(n.clay_depth),
            clay_softness: own(Brush::Clay).then_some(n.clay_softness),
            clay_relax: own(Brush::Clay).then_some(n.clay_relax),
            crease_pinch: own(Brush::Crease).then_some(n.crease_pinch),
            crease_depth: own(Brush::Crease).then_some(n.crease_depth),
            flatten_offset: own(Brush::Flatten).then_some(n.flatten_offset),
            layer_height: own(Brush::Layer).then_some(n.layer_height),
            move_connected: own(Brush::Move).then_some(n.move_connected),
            pose_mode: own(Brush::Pose).then(|| n.pose.mode.name().to_owned()),
            pose_joints: own(Brush::Pose).then_some(n.pose.joints),
            pose_offset: own(Brush::Pose).then_some(n.pose.offset),
            pose_rigidity: own(Brush::Pose).then_some(n.pose.rigidity),
        }
    }

    /// The record this stands for, over `base` (the kernel's base brush) for
    /// what it does not say, with every number in its slider's range. Also
    /// the name of the alpha it asks for, if that picture is not there.
    fn restored(&self, kernel: Brush, base: &Record) -> (Record, Option<String>) {
        let t = &self.tuning;
        let settings = BrushSettings {
            strength: t.strength,
            accumulate: t.accumulate,
            build_up: t.build_up,
            positive: t.positive,
            focal_shift: t.focal_shift,
            front_facing: t.front_facing,
            stay_in_group: t.stay_in_group,
            spacing: t.spacing,
            scatter: t.scatter,
            size_jitter: t.size_jitter,
            pressure_size: t.pressure_size,
            pressure_strength: t.pressure_strength,
            smooth_mode: t.smooth_mode,
            keep_volume: t.keep_volume,
            flatten_mode: t.flatten_mode,
            lock_plane: t.lock_plane,
            relax_normals: t.relax_normals,
            steady_plane: t.steady_plane,
        };
        // A curve needs two points that are numbers (`Record::in_range` then
        // puts them in order, as after any edit).
        let falloff = self.falloff.as_ref().and_then(|points| {
            let points: Vec<glam::Vec2> = points.iter().map(|&p| glam::Vec2::from_array(p)).filter(|p| p.is_finite()).collect();
            (points.len() >= 2).then_some(FalloffCurve { points })
        });
        let alpha = self.alpha.as_ref().map(AlphaKept::restored).unwrap_or_default();
        let wanted = self.alpha.as_ref().map(|kept| &kept.name).filter(|name| !name.is_empty());
        let missing = wanted.filter(|_| alpha.image.is_none()).cloned();
        let mut theirs = base.numbers;
        let set = |slot: &mut f32, value: Option<f32>| *slot = value.unwrap_or(*slot);
        set(&mut theirs.clay_depth, self.clay_depth);
        set(&mut theirs.clay_softness, self.clay_softness);
        set(&mut theirs.clay_relax, self.clay_relax);
        set(&mut theirs.crease_pinch, self.crease_pinch);
        set(&mut theirs.crease_depth, self.crease_depth);
        set(&mut theirs.flatten_offset, self.flatten_offset);
        set(&mut theirs.layer_height, self.layer_height);
        set(&mut theirs.pose.offset, self.pose_offset);
        set(&mut theirs.pose.rigidity, self.pose_rigidity);
        theirs.move_connected = self.move_connected.unwrap_or(theirs.move_connected);
        theirs.pose.joints = self.pose_joints.unwrap_or(theirs.pose.joints);
        let mode = self.pose_mode.as_deref().and_then(|name| PoseMode::ALL.into_iter().find(|mode| mode.name() == name));
        theirs.pose.mode = mode.unwrap_or(theirs.pose.mode);
        // Only its own kernel's numbers are the brush's to say.
        let numbers = base.numbers.with_own(kernel, &theirs);
        (Record { settings, falloff, alpha, numbers }.in_range(), missing)
    }
}

/// A brush file, whole.
#[derive(Serialize)]
struct BrushFile<'a> {
    name: &'a str,
    kernel: &'a str,
    brush: RecordKept,
    made: RecordKept,
    /// What its numbers mean (`prefs::FEEL_VERSION`). A file without it is
    /// from before 2026-10-21, and its Strength is brought over; one older
    /// than 2 has no Relax Normals or Steady Plane, and keeps both at nought
    /// (`parse`).
    feel_version: u32,
}

/// A brush as a file gave it.
pub struct Read {
    pub name: String,
    pub kernel: Brush,
    pub record: Record,
    pub made: Record,
    /// The alpha the brush asks for, if that picture is not on this
    /// computer: the brush has its plain round tip instead.
    pub missing_alpha: Option<String>,
}

impl Read {
    /// What to tell the user about the missing alpha.
    pub fn note(&self) -> Option<String> {
        let alpha = self.missing_alpha.as_ref()?;
        Some(format!("{} uses an alpha called {alpha}, which is not on this computer, so it has no alpha.", self.name))
    }
}

/// The text of the file for a brush.
pub fn text(name: &str, kernel: Brush, record: &Record, made: &Record) -> String {
    let file = BrushFile {
        name,
        kernel: kernel.name(),
        brush: RecordKept::of(record, kernel),
        made: RecordKept::of(made, kernel),
        feel_version: crate::prefs::FEEL_VERSION,
    };
    // Plain values and string keys: there is nothing in it that cannot be written.
    serde_json::to_string_pretty(&file).unwrap_or_default()
}

/// Reads a brush from the text of a file. `fallback_name` (the file's own
/// name) is the brush's if the file does not give one.
pub fn parse(text: &str, fallback_name: &str) -> Result<Read, String> {
    let file: Value = serde_json::from_str(text).map_err(|_| "it is not a brush file (it is not JSON)".to_owned())?;
    let kernel = file.get("kernel").and_then(Value::as_str).ok_or("it is not a brush file (it names no kernel)")?;
    let kernel = Brush::ALL
        .into_iter()
        .find(|b| b.name().eq_ignore_ascii_case(kernel.trim()))
        .ok_or_else(|| format!("its kernel, {kernel}, is not one this version has"))?;
    // A name is one line of plain text, whatever the file says.
    let name = file.get("name").and_then(Value::as_str).map(|name| name.replace(char::is_control, " "));
    let name = name.as_deref().map(str::trim).filter(|name| !name.is_empty());
    let base = Record::base(kernel);
    // A file from before the Strength slider was mapped for some kernels
    // (`sculpt::strength`): the Strength it gives is the one that does what
    // its number did then. A dragged print's Strength is its height, which
    // was not mapped. The rest of what it says is the user's and stays.
    let version = file.get("feel_version").and_then(Value::as_u64).unwrap_or(0);
    let read = |key: &str, over: &Record| {
        let start = RecordKept::of(over, kernel);
        let given = file.get(key);
        let kept = given.and_then(|value| crate::io::read_over(&start, value)).unwrap_or(start);
        let (mut record, missing) = kept.restored(kernel, &base);
        let prints = record.alpha.image.is_some() && record.alpha.drag;
        let has = |field: &str| given.is_some_and(|value| value.get(field).is_some());
        let strength = given.and_then(|value| value.get("strength")).and_then(Value::as_f64).map(|s| s as f32);
        if version < 1 && !prints && strength.is_some() {
            record.settings.strength = crate::sculpt::strength::from_old(kernel, record.settings.strength);
        }
        if version < 2 {
            // Older than Relax Normals and Steady Plane: the brush did
            // without them, and still does (a missing number is otherwise
            // the base brush's starting value, which would change it).
            if !has("relax_normals") {
                record.settings.relax_normals = 0.0;
            }
            if !has("steady_plane") {
                record.settings.steady_plane = 0.0;
            }
            // Accumulate started on for these then: a file that does not
            // say keeps it so.
            if !has("accumulate") && matches!(kernel, Brush::Standard | Brush::Inflate | Brush::Clay) {
                record.settings.accumulate = true;
            }
            // Standard without Accumulate builds each stroke to a height
            // now: its Strength is the one that builds a pass as tall.
            if kernel == Brush::Standard && !record.settings.accumulate && !prints {
                let old = match (version, strength) {
                    (0, Some(old)) => Some(old),
                    (_, Some(_)) => Some(crate::sculpt::strength::effective(kernel, record.settings.strength)),
                    (_, None) => None,
                };
                if let Some(old) = old.filter(|old| old.is_finite()) {
                    record.settings.strength = crate::sculpt::strength::from_old_seeking(old);
                }
            }
        }
        (record, missing)
    };
    let (record, missing_alpha) = read("brush", &base);
    // A file with no `made` was written by hand: Reset goes back to the
    // brush as the file has it.
    let made = if file.get("made").is_some_and(Value::is_object) { read("made", &base).0 } else { record.clone() };
    Ok(Read { name: name.unwrap_or(fallback_name).to_owned(), kernel, record, made, missing_alpha })
}

/// Reads the brush file at `path`: what Import Brush does, under its file box.
pub fn read(path: &Path) -> Result<Read, String> {
    let shown = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
    let size = std::fs::metadata(path).map_err(|e| format!("Could not read {shown}: {e}"))?.len();
    if size > MAX_BYTES {
        return Err(format!("{shown} is not a brush file (it is far too big to be one)."));
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("Could not read {shown}: {e}"))?;
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    parse(&text, &stem).map_err(|why| format!("{shown} was not imported: {why}."))
}

/// Writes a brush to `path`, whole or not at all: what Export Brush does,
/// under its file box.
pub fn write(path: &Path, name: &str, kernel: Brush, record: &Record, made: &Record) -> Result<(), String> {
    let text = text(name, kernel, record, made);
    crate::io::replace_file(path, |w| w.write_all(text.as_bytes()))
}

/// A file name for the brush called `name`: its letters, digits, spaces,
/// dashes and underscores, and nothing Windows would refuse.
pub fn file_stem(name: &str) -> String {
    let kept: String = name.trim().chars().map(|c| if c.is_alphanumeric() || " -_".contains(c) { c } else { '_' }).collect();
    let kept = kept.trim().to_owned();
    // CON, NUL, COM1 and their kind are devices whatever follows the dot.
    let device = crate::bridge_guard::off_limits(&format!("C:\\{kept}.{EXTENSION}")).is_some();
    if kept.is_empty() || device { format!("brush {kept}").trim().to_owned() } else { kept }
}

/// One brush's file in the folder.
struct OnDisk {
    path: PathBuf,
    /// What the app would write for the brush as it was when the file was
    /// last read or written: the file is written again when that changes.
    text: String,
}

/// The brush folder as the app last left it (see the top of this file).
#[derive(Default)]
pub struct Shelf {
    /// The folder, if this run keeps one, and whether it may write there.
    dir: Option<PathBuf>,
    writable: bool,
    /// The brushes with a file, by name.
    files: BTreeMap<String, OnDisk>,
    /// Files in the folder that are not brushes this version can read:
    /// left exactly as they are, and never written over.
    foreign: Vec<PathBuf>,
    /// The folder has been read into the library: once is all it is read.
    loaded: bool,
}

impl Shelf {
    /// The shelf of the settings folder `home`, or one that keeps nothing.
    pub fn new(home: Option<&Path>, writable: bool) -> Shelf {
        Shelf { dir: home.map(|home| home.join(FOLDER)), writable, ..Shelf::default() }
    }

    /// The folder, for telling the user where their brushes are.
    pub fn folder(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// Start-up: reads every brush file in the folder into `library`. A
    /// file named for a shipped brush is that brush as the user left it;
    /// any other is a brush of the user's own. Gives back what there is to
    /// say: the files that were skipped, and the alphas that were missing.
    /// Asked again, it does nothing: the brushes are already in the library.
    pub fn load(&mut self, library: &mut Library, s: &mut Settings) -> Vec<String> {
        if std::mem::replace(&mut self.loaded, true) {
            return Vec::new();
        }
        let Some(dir) = self.dir.clone() else { return Vec::new() };
        let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
            .unwrap_or_default();
        paths.retain(|path| path.is_file() && path.extension().is_some_and(|e| e.eq_ignore_ascii_case(EXTENSION)));
        paths.sort();
        let (mut notes, mut skipped) = (Vec::new(), Vec::new());
        for path in paths {
            match read(&path) {
                Ok(brush) => {
                    notes.extend(brush.note());
                    let i = place(library, s, brush);
                    let entry = &library.entries()[i];
                    let text = text(&entry.name, entry.kernel, &library.record(i, s), entry.made());
                    self.files.insert(entry.name.clone(), OnDisk { path, text });
                }
                Err(why) => {
                    log::warn!("{why}");
                    skipped.push(path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
                    self.foreign.push(path);
                }
            }
        }
        if !skipped.is_empty() {
            let one = skipped.len() == 1;
            let (files, they) = if one { ("a file", "it is not a brush") } else { ("files", "they are not brushes") };
            let names = skipped.join(", ");
            notes.push(format!("Skipped {files} in the brushes folder ({names}): {they} this version can read."));
        }
        notes
    }

    /// A path in the folder for the brush called `name` that no file on
    /// the shelf, and no file that is not ours, already has.
    fn free_path(&self, dir: &Path, name: &str) -> PathBuf {
        let stem = file_stem(name);
        let taken = |path: &Path| {
            let same = |other: &Path| other.as_os_str().eq_ignore_ascii_case(path.as_os_str());
            self.files.values().any(|file| same(&file.path)) || self.foreign.iter().any(|other| same(other)) || path.exists()
        };
        let numbered = |n: u32| dir.join(if n < 2 { format!("{stem}.{EXTENSION}") } else { format!("{stem} {n}.{EXTENSION}") });
        (1..).map(numbered).find(|path| !taken(path)).unwrap_or_else(|| numbered(1))
    }

    /// Brings the folder up to date with the library: writes the file of
    /// every brush that has changed since its file was written, and takes
    /// away the file of a brush that needs none any more (one renamed, whose
    /// file goes under its new name, or a shipped brush put back as it
    /// shipped). Cheap when nothing has changed, which is nearly always.
    pub fn sync(&mut self, library: &Library, s: &Settings) {
        let Some(dir) = self.dir.clone().filter(|_| self.writable) else { return };
        let mut wanted = BTreeMap::new();
        for (i, entry) in library.entries().iter().enumerate() {
            let record = library.record(i, s);
            if entry.origin == Origin::User || (entry.origin == Origin::Shipped && record != *entry.made()) {
                wanted.insert(entry.name.clone(), text(&entry.name, entry.kernel, &record, entry.made()));
            }
        }
        let gone: Vec<String> = self.files.keys().filter(|name| !wanted.contains_key(*name)).cloned().collect();
        for (name, text) in wanted {
            if self.files.get(&name).is_some_and(|file| file.text == text) {
                continue;
            }
            let path = self.files.get(&name).map_or_else(|| self.free_path(&dir, &name), |file| file.path.clone());
            let wrote = std::fs::create_dir_all(&dir)
                .map_err(|e| e.to_string())
                .and_then(|()| crate::io::replace_file(&path, |w| w.write_all(text.as_bytes())));
            if let Err(e) = wrote {
                log::warn!("could not keep the brush {name}: {e}");
            }
            // Even if that failed: trying again every second would not help.
            self.files.insert(name, OnDisk { path, text });
        }
        // Last, so that a renamed brush is under its new name before the
        // file under its old one goes.
        for name in gone {
            if let Some(file) = self.files.remove(&name) {
                let _ = std::fs::remove_file(&file.path);
            }
        }
    }

    /// A brush is about to be deleted: its file, as the brush is now, goes
    /// into the folder's `deleted` folder, from where Import Brush can bring
    /// it back. Gives back where it went, if it went anywhere.
    pub fn set_aside(&mut self, library: &Library, i: usize, s: &Settings) -> Option<PathBuf> {
        let dir = self.dir.clone().filter(|_| self.writable)?;
        let entry = library.entries().get(i)?;
        let had = self.files.remove(&entry.name);
        // A shipped brush nobody changed is in the app itself; there is
        // nothing of the user's to keep.
        let record = library.record(i, s);
        if entry.origin != Origin::User && record == *entry.made() {
            return None;
        }
        let aside = dir.join(DELETED);
        let path = aside.join(format!("{}.{EXTENSION}", file_stem(&entry.name)));
        let text = text(&entry.name, entry.kernel, &record, entry.made());
        let wrote = std::fs::create_dir_all(&aside)
            .map_err(|e| e.to_string())
            .and_then(|()| crate::io::replace_file(&path, |w| w.write_all(text.as_bytes())));
        if let Some(file) = had {
            // Only once the copy is safely written.
            if wrote.is_ok() {
                let _ = std::fs::remove_file(&file.path);
            } else {
                self.foreign.push(file.path);
            }
        }
        wrote.ok().map(|()| path)
    }
}

/// Puts a brush read from the folder into the library: over the shipped
/// brush of its name, or as a new brush of the user's own. Gives back its
/// place.
fn place(library: &mut Library, s: &mut Settings, brush: Read) -> usize {
    let shipped = library.named(&brush.name).filter(|&i| {
        let entry = &library.entries()[i];
        entry.origin == Origin::Shipped && entry.kernel == brush.kernel
    });
    match shipped {
        Some(i) => {
            library.set_record(i, brush.record, s);
            i
        }
        None => library.add(&brush.name, brush.kernel, brush.record, brush.made),
    }
}

/// Brings a brush from a file into the library as one of the user's own,
/// under a name no brush has, and takes it in hand. Gives back its place
/// and what there is to say about it.
pub fn import(path: &Path, library: &mut Library, s: &mut Settings) -> Result<(usize, String), String> {
    let brush = read(path)?;
    let note = brush.note();
    let i = library.add(&brush.name, brush.kernel, brush.record, brush.made);
    library.select(i, s);
    let name = &library.entries()[i].name;
    Ok((i, note.unwrap_or_else(|| format!("Imported the brush {name}."))))
}

/// Writes brush `i` of the library to `path` as it is now.
pub fn export(path: &Path, library: &Library, i: usize, s: &Settings) -> Result<String, String> {
    let entry = library.entries().get(i).ok_or("There is no such brush.")?;
    write(path, &entry.name, entry.kernel, &library.record(i, s), entry.made())?;
    Ok(format!("Exported the brush {} to {}.", entry.name, path.display()))
}

#[cfg(test)]
mod tests;
