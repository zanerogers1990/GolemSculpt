//! The material library: every material there is to paint with. The
//! built-in ones first (`builtin.rs`), then one for each custom shader in the
//! shaders folder (`custom.rs`), then the user's own (`files.rs`), each a
//! named [`Material`].
//!
//! Built-in and custom-shader materials are as they come: to change one,
//! Duplicate it and change the copy, which is the user's own. A user's
//! material can be renamed, deleted (its file is set aside, see `files.rs`),
//! and its tint and finish settings changed in the Materials card, where the
//! change sticks as it is made.

use super::custom::Shaders;
use super::{Custom, MAX_NAME, Material, Settings, Tint, TintMode, builtin};

/// Where a material came from, which says what may be done with it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Origin {
    /// Made from a Look preset at start-up.
    BuiltIn,
    /// One of the custom shaders in the shaders folder, as its sidecar has it.
    Shader,
    /// The user's own: a duplicate, a saved Look, an import.
    User,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub material: Material,
    pub origin: Origin,
}

/// Something the Materials card asks to have done. It is the app that does
/// it (`app/materials.rs`): the folder, the file boxes, the scene's palette
/// and the status line are the app's.
#[derive(Clone, Debug, PartialEq)]
pub enum Ask {
    /// Paint with this material of the library from now on.
    Select(usize),
    /// Paint with this material of the scene's palette from now on.
    SelectSlot(u8),
    Duplicate(usize),
    Rename(usize, String),
    Delete(usize),
    /// A user material's settings, changed in the card (sent while a slider
    /// is held). Painted areas using it as it was change with it.
    Set(usize, Material),
    /// Pick a material file and bring it in. Opens a file box.
    Import,
    /// Pick where to write this material. Opens a file box.
    Export(usize),
    /// Make a material of the Look's surface finish as it is now.
    SaveLook,
    /// Make a material of the library of this palette entry.
    SaveSlot(u8),
    /// Take out of the palette the entries no object's paint uses.
    RemoveUnused,
    /// Put this library material in place of a palette entry: everything
    /// painted with the entry takes the new material.
    Replace(u8, usize),
    /// Measure the model again for a palette entry's pattern size.
    Remeasure(u8),
    /// Look through the shaders folder again.
    ReloadShaders,
}

pub struct Library {
    entries: Vec<Entry>,
    /// Moves with every change, so what is worked out from the library (the
    /// thumbnails' names) is worked out again only then.
    generation: u64,
}

impl Default for Library {
    fn default() -> Library {
        let entries = builtin::all().into_iter().map(|material| Entry { material, origin: Origin::BuiltIn }).collect();
        Library { entries, generation: 0 }
    }
}

impl Library {
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The material called `name`, whatever the case of its letters.
    pub fn named(&self, name: &str) -> Option<usize> {
        self.entries.iter().position(|e| e.material.name.eq_ignore_ascii_case(name.trim()))
    }

    /// Why `name` cannot be a material's name, if it cannot. `but` is the
    /// material being named, which may keep its own.
    pub fn name_refused(&self, name: &str, but: Option<usize>) -> Option<String> {
        let name = name.trim();
        if name.is_empty() {
            return Some("A material needs a name.".to_owned());
        }
        if name.chars().count() > MAX_NAME {
            return Some(format!("A material's name can be {MAX_NAME} letters at most."));
        }
        let taken = self.entries.iter().enumerate().any(|(i, e)| Some(i) != but && e.material.name.eq_ignore_ascii_case(name));
        taken.then(|| format!("There is already a material called {name}."))
    }

    /// A name no material has, starting from `wanted`.
    pub fn free_name(&self, wanted: &str) -> String {
        let wanted: String = wanted.trim().chars().take(MAX_NAME - 3).collect();
        let wanted = if wanted.is_empty() { "Material".to_owned() } else { wanted };
        if self.name_refused(&wanted, None).is_none() {
            return wanted;
        }
        (2..).map(|n| format!("{wanted} {n}")).find(|name| self.name_refused(name, None).is_none()).unwrap_or(wanted)
    }

    /// Adds a material of the user's own under a name no other has, and
    /// gives back where it is.
    pub fn add(&mut self, mut material: Material) -> usize {
        material.name = self.free_name(&material.name);
        material.source = None;
        self.entries.push(Entry { material: material.in_range(), origin: Origin::User });
        self.generation += 1;
        self.entries.len() - 1
    }

    /// A copy of material `i`, as it is now, as one of the user's own.
    pub fn duplicate(&mut self, i: usize) -> Option<usize> {
        let mut material = self.entries.get(i)?.material.clone();
        material.name = format!("{} Copy", material.name);
        Some(self.add(material))
    }

    /// Renames one of the user's materials.
    pub fn rename(&mut self, i: usize, name: &str) -> Result<(), String> {
        let entry = self.entries.get(i).ok_or("There is no such material.")?;
        if entry.origin != Origin::User {
            return Err(format!("{} comes with the app and keeps its name. Duplicate it and name the copy.", entry.material.name));
        }
        if let Some(why) = self.name_refused(name, Some(i)) {
            return Err(why);
        }
        self.entries[i].material.name = name.trim().to_owned();
        self.generation += 1;
        Ok(())
    }

    /// Deletes one of the user's materials.
    pub fn delete(&mut self, i: usize) -> Result<Material, String> {
        let entry = self.entries.get(i).ok_or("There is no such material.")?;
        if entry.origin != Origin::User {
            return Err(format!("{} comes with the app, and stays.", entry.material.name));
        }
        self.generation += 1;
        Ok(self.entries.remove(i).material)
    }

    /// Changes one of the user's materials (its name stays its own). Gives
    /// back what it was.
    pub fn set(&mut self, i: usize, material: Material) -> Result<Material, String> {
        let entry = self.entries.get_mut(i).ok_or("There is no such material.")?;
        if entry.origin != Origin::User {
            return Err(format!("{} comes with the app. Duplicate it to change it.", entry.material.name));
        }
        let name = entry.material.name.clone();
        let was = std::mem::replace(&mut entry.material, Material { name, source: None, ..material }.in_range());
        self.generation += 1;
        Ok(was)
    }

    /// The custom shaders' materials as `shaders` has them now: one per
    /// shader, after the built-in ones, at its sidecar's defaults.
    pub fn set_shaders(&mut self, shaders: &Shaders) {
        self.entries.retain(|e| e.origin != Origin::Shader);
        let at = self.entries.iter().position(|e| e.origin == Origin::User).unwrap_or(self.entries.len());
        let made: Vec<Entry> = shaders
            .list()
            .iter()
            .map(|shader| {
                let swatch = shader.sidecar.swatch.unwrap_or([0.6, 0.6, 0.6]).map(|c| c.clamp(0.0, 1.0));
                let settings = Settings::Custom(Custom { id: shader.id.clone(), values: shader.sidecar.defaults() });
                let tint = Tint { mode: TintMode::Off, amount: 1.0 };
                Entry { material: Material::new(&shader.name(), swatch, tint, settings), origin: Origin::Shader }
            })
            .collect();
        // A user's material named like a new shader keeps its name; the
        // shader's material gives way.
        let mut made = made;
        for entry in &mut made {
            if self.named(&entry.material.name).is_some() {
                entry.material.name = format!("{} (shader)", entry.material.name);
            }
        }
        self.entries.splice(at..at, made);
        self.generation += 1;
    }
}
