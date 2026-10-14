//! The user's own materials as files: one small JSON file each in a
//! `materials` folder beside the settings file, and the same file as what
//! Export Material writes and Import Material reads. Built the way the
//! brushes are kept (`brush_files.rs`):
//!
//! ```json
//! { "name": "Green Bronze", "swatch": [0.3, 0.5, 0.4],
//!   "tint": { "mode": "Multiply", "amount": 0.6 },
//!   "settings": { "finish": "Metal", "preset": "patina", "exposure": 1.2, ... } }
//! ```
//!
//! Reading is as forgiving as the settings file: a field the file lacks
//! keeps the finish's starting value, one this version does not know is
//! ignored, and numbers are brought into their sliders' range
//! (`Material::from_json`). A file that is not a material at all is skipped,
//! said so, and never written over.
//!
//! The [`Shelf`] is the folder as the app last left it. It writes a file when
//! its material changed and at no other time, so a file edited by hand is
//! left as it is until the material is changed in the app. Delete moves the
//! file into `deleted/`, where Import Material can find it again.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::library::{Library, Origin};
use super::Material;

/// The folder's name, beside the settings file.
pub const FOLDER: &str = "materials";
const DELETED: &str = "deleted";
const EXTENSION: &str = "json";
/// No material file comes near this; anything bigger is not read.
const MAX_BYTES: u64 = 1 << 20;

/// The text of the file for a material.
pub fn text(material: &Material) -> String {
    let mut kept = material.clone();
    // A palette's copy of a shader stays in the scene; the folder has the shader.
    kept.source = None;
    serde_json::to_string_pretty(&kept).unwrap_or_default()
}

/// Reads a material from the text of a file; `fallback_name` (the file's
/// own name) is its name if the file gives none.
pub fn parse(text: &str, fallback_name: &str) -> Result<Material, String> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|_| "it is not a material file (it is not JSON)".to_owned())?;
    let mut material = Material::from_json(&value, fallback_name)?;
    material.source = None;
    Ok(material)
}

/// Reads the material file at `path`: what Import Material does, under its file box.
pub fn read(path: &Path) -> Result<Material, String> {
    let shown = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
    let size = std::fs::metadata(path).map_err(|e| format!("Could not read {shown}: {e}"))?.len();
    if size > MAX_BYTES {
        return Err(format!("{shown} is not a material file (it is far too big to be one)."));
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("Could not read {shown}: {e}"))?;
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    parse(&text, &stem).map_err(|why| format!("{shown} was not imported: {why}."))
}

/// Writes a material to `path`, whole or not at all.
pub fn write(path: &Path, material: &Material) -> Result<(), String> {
    let text = text(material);
    crate::io::replace_file(path, |w| w.write_all(text.as_bytes()))
}

/// A file name for the material called `name`: its letters, digits, spaces,
/// dashes and underscores, and nothing Windows would refuse.
pub fn file_stem(name: &str) -> String {
    let kept: String = name.trim().chars().map(|c| if c.is_alphanumeric() || " -_".contains(c) { c } else { '_' }).collect();
    let kept = kept.trim().to_owned();
    // CON, NUL, COM1 and their kind are devices whatever follows the dot.
    let device = crate::bridge_guard::off_limits(&format!("C:\\{kept}.{EXTENSION}")).is_some();
    if kept.is_empty() || device { format!("material {kept}").trim().to_owned() } else { kept }
}

struct OnDisk {
    path: PathBuf,
    /// What the app would write for the material as it was when the file
    /// was last read or written: the file is written again when that changes.
    text: String,
}

/// The materials folder as the app last left it (see the top of this file).
#[derive(Default)]
pub struct Shelf {
    dir: Option<PathBuf>,
    writable: bool,
    /// The user's materials with a file, by name.
    files: BTreeMap<String, OnDisk>,
    /// Files in the folder that are not materials this version can read.
    foreign: Vec<PathBuf>,
    loaded: bool,
}

impl Shelf {
    /// The shelf of the settings folder `home`, or one that keeps nothing.
    pub fn new(home: Option<&Path>, writable: bool) -> Shelf {
        Shelf { dir: home.map(|home| home.join(FOLDER)), writable, ..Shelf::default() }
    }

    /// The folder, for telling the user where their materials are.
    pub fn folder(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// Whether this run may write there.
    pub fn writable(&self) -> bool {
        self.writable
    }

    /// Start-up: reads every material file in the folder into `library` as
    /// one of the user's own. Gives back what there is to say. Asked again,
    /// it does nothing.
    pub fn load(&mut self, library: &mut Library) -> Vec<String> {
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
                Ok(material) => {
                    let i = library.add(material);
                    let entry = &library.entries()[i];
                    self.files.insert(entry.material.name.clone(), OnDisk { path, text: text(&entry.material) });
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
            let (files, they) = if one { ("a file", "it is not a material") } else { ("files", "they are not materials") };
            notes.push(format!("Skipped {files} in the materials folder ({}): {they} this version can read.", skipped.join(", ")));
        }
        notes
    }

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
    /// every material of the user's that changed since its file was written,
    /// and takes away the file of one renamed (its file goes under the new
    /// name). Cheap when nothing changed, which is nearly always.
    pub fn sync(&mut self, library: &Library) {
        let Some(dir) = self.dir.clone().filter(|_| self.writable) else { return };
        let wanted: BTreeMap<String, String> = library
            .entries()
            .iter()
            .filter(|e| e.origin == Origin::User)
            .map(|e| (e.material.name.clone(), text(&e.material)))
            .collect();
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
                log::warn!("could not keep the material {name}: {e}");
            }
            self.files.insert(name, OnDisk { path, text });
        }
        for name in gone {
            if let Some(file) = self.files.remove(&name) {
                let _ = std::fs::remove_file(&file.path);
            }
        }
    }

    /// Material `i` is about to be deleted: its file, as the material is
    /// now, goes into the folder's `deleted` folder, from where Import
    /// Material can bring it back. Gives back where it went.
    pub fn set_aside(&mut self, library: &Library, i: usize) -> Option<PathBuf> {
        let dir = self.dir.clone().filter(|_| self.writable)?;
        let entry = library.entries().get(i).filter(|e| e.origin == Origin::User)?;
        let had = self.files.remove(&entry.material.name);
        let aside = dir.join(DELETED);
        let path = aside.join(format!("{}.{EXTENSION}", file_stem(&entry.material.name)));
        let text = text(&entry.material);
        let wrote = std::fs::create_dir_all(&aside)
            .map_err(|e| e.to_string())
            .and_then(|()| crate::io::replace_file(&path, |w| w.write_all(text.as_bytes())));
        if let Some(file) = had {
            if wrote.is_ok() {
                let _ = std::fs::remove_file(&file.path);
            } else {
                self.foreign.push(file.path);
            }
        }
        wrote.ok().map(|()| path)
    }
}

/// Brings a material from a file into the library as one of the user's
/// own, under a name no material has. Gives back its place and what to say.
pub fn import(path: &Path, library: &mut Library) -> Result<(usize, String), String> {
    let material = read(path)?;
    let i = library.add(material);
    let name = &library.entries()[i].material.name;
    Ok((i, format!("Imported the material {name}.")))
}

/// Writes material `i` of the library to `path` as it is now.
pub fn export(path: &Path, library: &Library, i: usize) -> Result<String, String> {
    let entry = library.entries().get(i).ok_or("There is no such material.")?;
    write(path, &entry.material)?;
    Ok(format!("Exported the material {} to {}.", entry.material.name, path.display()))
}
