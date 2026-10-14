//! The scene's palette, the materials side: what a palette entry's material
//! is, and the changes the Materials card and the painting brush make to the
//! palette (`mesh/paint/palette.rs` is the data a scene file keeps).
//!
//! An entry holds a whole copy of its material, and for a custom one the
//! shader's source and sidecar too, so a scene opens and looks the same on a
//! computer without the library it was painted from. Its `reference` is the
//! model size the material's patterns are measured against, frozen when the
//! entry was made (as picking a Look preset freezes it), so the pattern does
//! not crawl while the model is sculpted; Re-measure takes it again.
//!
//! **Which entry a library material paints as** ([`ensure_in_palette`]): the
//! entry with the same name holding exactly the same material, made the
//! first time the material is used. A library material changed later
//! ([`follow_edit`]) takes the entries that were exactly it along, so what
//! was painted with it changes with it; an entry from another scene file
//! that only shares the name is left alone and the new one gets a name of
//! its own.

use crate::mesh::paint::palette::{PaletteEntry, ScenePaint};

use super::custom::Shaders;
use super::{Material, Plain, Settings, Source, Tint, TintMode};

/// The material of a palette entry, read forgivingly: an entry whose
/// material cannot be read (from a newer version, or damaged) is a plain
/// material of its swatch colour, and is drawn so.
pub fn material_of(entry: &PaletteEntry) -> Material {
    match Material::from_json(&entry.material, &entry.name) {
        Ok(mut m) => {
            if m.name.trim().is_empty() {
                m.name = entry.name.clone();
            }
            m
        }
        Err(_) => {
            let plain = Plain { base_color: entry.swatch, ..Plain::default() };
            Material::new(&entry.name, entry.swatch, Tint { mode: TintMode::Replace, amount: 1.0 }, Settings::Plain(plain))
        }
    }
}

/// The copy of `material` an entry keeps: with a custom material's shader
/// source and sidecar as they are now.
fn kept(material: &Material, shaders: &Shaders) -> Material {
    let mut m = material.clone();
    if let Settings::Custom(c) = &m.settings {
        m.source = shaders.get(&c.id).map(|s| Source { wgsl: s.source.clone(), sidecar: s.sidecar_text.clone() }).or(m.source);
    }
    m
}

/// The JSON an entry keeps for `material`, to compare entries by.
fn json_of(material: &Material) -> serde_json::Value {
    material.to_json()
}

/// The key under which an entry remembers the library material it was
/// made from (its own name may differ, where the palette had that name).
const LIBRARY: &str = "library";

/// The name of the library material `entry` was made from.
pub fn library_name(entry: &PaletteEntry) -> &str {
    entry.extra.get(LIBRARY).and_then(serde_json::Value::as_str).unwrap_or(&entry.name)
}

fn link(entry: &mut PaletteEntry, material: &Material) {
    entry.extra.insert(LIBRARY.to_owned(), serde_json::Value::String(material.name.clone()));
}

/// Whether `entry` was made from `material` and holds exactly it (names
/// and shader source aside).
fn holds(entry: &PaletteEntry, material: &Material) -> bool {
    let mut theirs = material_of(entry);
    let mut ours = material.clone();
    (theirs.source, ours.source) = (None, None);
    theirs.name.clone_from(&ours.name);
    library_name(entry) == material.name && theirs == ours
}

/// The slot the paint names `material` by, adding it to the palette the
/// first time it is used. `size` is the model size now, which a new entry's
/// patterns are frozen against. The palette has room for 255.
pub fn ensure_in_palette(paint: &mut ScenePaint, material: &Material, size: f32, shaders: &Shaders) -> Result<u8, String> {
    if let Some(entry) = paint.palette.iter().find(|e| holds(e, material)) {
        return Ok(entry.slot);
    }
    let slot = (1..=255u8)
        .find(|s| !paint.palette.iter().any(|e| e.slot == *s))
        .ok_or("The scene's palette is full (255 materials). Remove Unused makes room.")?;
    let mut name = material.name.clone();
    let taken = |n: &str, palette: &[PaletteEntry]| palette.iter().any(|e| e.name.eq_ignore_ascii_case(n));
    if taken(&name, &paint.palette) {
        name = (2..).map(|k| format!("{} {k}", material.name)).find(|n| !taken(n, &paint.palette)).unwrap_or(name);
    }
    let copy = Material { name: name.clone(), ..kept(material, shaders) };
    let mut entry = PaletteEntry {
        slot,
        name,
        swatch: material.swatch,
        reference: if size > 0.0 && size.is_finite() { size } else { 0.0 },
        material: json_of(&copy),
        extra: Default::default(),
    };
    link(&mut entry, material);
    paint.palette.push(entry);
    Ok(slot)
}

/// A library material changed from `before` to `after`: every entry that
/// held exactly `before` now holds `after` (keeping its slot, its name and
/// its reference), so what was painted with it follows the change. Gives
/// back how many entries changed.
pub fn follow_edit(paint: &mut ScenePaint, before: &Material, after: &Material, shaders: &Shaders) -> usize {
    let mut changed = 0;
    for entry in paint.palette.iter_mut().filter(|e| holds(e, before)) {
        let copy = Material { name: entry.name.clone(), ..kept(after, shaders) };
        entry.material = json_of(&copy);
        entry.swatch = after.swatch;
        link(entry, after);
        changed += 1;
    }
    changed
}

/// A library material was renamed: the entries made from it remember it by
/// its new name.
pub fn follow_rename(paint: &mut ScenePaint, from: &str, to: &str) {
    let to = serde_json::Value::String(to.to_owned());
    for entry in paint.palette.iter_mut().filter(|e| library_name(e) == from) {
        entry.extra.insert(LIBRARY.to_owned(), to.clone());
    }
}

/// The custom shaders `ids` changed on disk: the entries using them take
/// the new source, so the scene file keeps what is on screen.
pub fn refresh_sources(paint: &mut ScenePaint, ids: &[String], shaders: &Shaders) {
    for entry in &mut paint.palette {
        let material = material_of(entry);
        let Settings::Custom(c) = &material.settings else { continue };
        if !ids.contains(&c.id) {
            continue;
        }
        let Some(shader) = shaders.list().iter().find(|s| s.id == c.id) else { continue };
        let source = Some(Source { wgsl: shader.source.clone(), sidecar: shader.sidecar_text.clone() });
        if material.source != source {
            entry.material = json_of(&Material { source, ..material });
        }
    }
}

/// Lets `shaders` know the custom shaders the palette carries, for the ones
/// the shaders folder does not have.
pub fn carry_sources(paint: &ScenePaint, shaders: &mut Shaders) {
    for entry in &paint.palette {
        let material = material_of(entry);
        if let (Settings::Custom(c), Some(source)) = (&material.settings, &material.source) {
            shaders.carry(&c.id, source);
        }
    }
}

/// Takes out every entry whose slot is not in `used`. Gives back how many went.
pub fn remove_unused(paint: &mut ScenePaint, used: &[u8]) -> usize {
    let before = paint.palette.len();
    paint.palette.retain(|e| used.contains(&e.slot));
    before - paint.palette.len()
}

/// Puts `material` in the place of entry `slot`, keeping the slot (so what
/// was painted with the entry takes the new material) and its reference.
pub fn replace(paint: &mut ScenePaint, slot: u8, material: &Material, shaders: &Shaders) -> Result<String, String> {
    let entry = paint.palette.iter_mut().find(|e| e.slot == slot).ok_or("There is no such palette entry.")?;
    let was = entry.name.clone();
    entry.name = material.name.clone();
    entry.swatch = material.swatch;
    entry.material = json_of(&kept(material, shaders));
    link(entry, material);
    Ok(format!("What was painted with {was} is now {}.", material.name))
}

/// Measures the model again for entry `slot`'s patterns.
pub fn remeasure(paint: &mut ScenePaint, slot: u8, size: f32) -> Result<String, String> {
    let entry = paint.palette.iter_mut().find(|e| e.slot == slot).ok_or("There is no such palette entry.")?;
    entry.reference = size;
    Ok(format!("{}'s pattern is sized to the model as it is now.", entry.name))
}
