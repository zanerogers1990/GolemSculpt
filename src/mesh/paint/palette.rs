//! The scene's material palette (B.3), as a file keeps it: the materials
//! paint can name by slot, 1 to 255, shared by every object of the scene.
//!
//! This is the part the paint data needs: the slot numbers a layer's
//! material list holds mean entries of this list, and the list is read from
//! and written to the `.sclp` file's JSON under `io::slot::PAINT`. What a
//! material is (its finish and settings) belongs to the materials library,
//! which fills `PaletteEntry::material` in; until then an entry is a name and
//! a swatch colour. Every field is optional on reading, so a file from any
//! version reads, and a field this version does not know is kept as it was
//! (`extra`).

use serde::{Deserialize, Serialize};

/// The scene's palette and the two display toggles of painted surfaces.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScenePaint {
    pub palette: Vec<PaletteEntry>,
    /// Show the painted colour.
    pub show_polypaint: bool,
    /// Show the painted materials.
    pub show_materials: bool,
    /// The palette slot paint strokes lay as their material, picked in the
    /// Materials card; `None` paints colour only. Kept with the scene: a slot
    /// means something only in its own palette.
    pub active_material: Option<u8>,
}

impl Default for ScenePaint {
    fn default() -> Self {
        ScenePaint { palette: Vec::new(), show_polypaint: true, show_materials: true, active_material: None }
    }
}

/// One material of the palette.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PaletteEntry {
    /// 1 to 255: what a layer's material list holds for it.
    pub slot: u8,
    pub name: String,
    /// The colour the palette row and a plain render show, sRGB 0 to 1.
    pub swatch: [f32; 3],
    /// The size the finish's patterns are measured against, frozen when the
    /// entry was added; 0 to measure each frame.
    pub reference: f32,
    /// The material itself, as the materials library writes it.
    pub material: serde_json::Value,
    /// Anything else a later version wrote, kept as it was.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl ScenePaint {
    /// The palette as a file kept it, or as it was with nothing kept: an
    /// entry with a slot of 0 or a slot used twice is left out.
    pub fn read(details: &crate::io::Details) -> Option<ScenePaint> {
        let mut paint: ScenePaint = details.get_over(crate::io::slot::PAINT, &ScenePaint::default())?;
        let mut seen = [false; 256];
        paint.palette.retain(|e| {
            let fresh = e.slot != 0 && !seen[e.slot as usize];
            seen[e.slot as usize] = true;
            fresh
        });
        // A material to paint with that the palette does not have is none.
        if paint.active_material.is_some_and(|slot| !seen[slot as usize] || slot == 0) {
            paint.active_material = None;
        }
        Some(paint)
    }

    /// Keeps the palette in a file's JSON; nothing for a scene that has
    /// never had one, so files of unpainted scenes are as they were.
    pub fn keep(&self, details: &mut crate::io::Details) {
        if *self != ScenePaint::default() {
            details.set(crate::io::slot::PAINT, self);
        }
    }
}
