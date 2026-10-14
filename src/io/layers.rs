//! A mesh's sculpt layers in a `.sclp` file: the `LAYR` block.
//!
//! The mesh the file holds is the finished shape, every layer in it at its
//! strength, written as any other mesh is. So a reader that knows nothing of
//! layers opens the model as it was on screen and steps over this block; and
//! a block that cannot be read costs the layers and not the shape: what they
//! added is part of the model from then on, as if they had been baked, and
//! the status line says so (`Lost`).
//!
//! The body, after the mesh's number (u32): a layout number (u32, 1), how
//! many layers (u32), the number of the layer that records (u32, 0 for
//! none), the number the next layer made will be given (u32) and the mesh's
//! vertex count (u64). Then each layer in the order of the list: its number
//! (u32), whether it is on (a byte), its strength in percent (a byte), the
//! length of its name in bytes (u16), the name (UTF-8), and an offset (three
//! f32) for every vertex, in the order the mesh's `MESH` block has them.
//!
//! A mesh below its finest subdivision level also has each layer's offsets
//! for the finest level, which the level stack keeps: those are in the
//! `LVLS` block (`io/levels.rs`).
//!
//! Reading checks the vertex count against the mesh and that the layers fit
//! in the block before it asks for any memory. Whether the layers are that
//! mesh's is settled when they are put back on it (`Layers::restored`).

use std::io::{Read, Write};

use glam::Vec3;

use super::{read_array, read_u32, read_u64};
use crate::mesh::layers::{SavedLayer, SavedLayers};

/// The layout written today.
const LAYOUT: u32 = 1;

/// The body up to the first layer: the mesh's number, the layout, three
/// numbers about the list and the vertex count.
const HEAD: usize = 4 + 4 + 3 * 4 + 8;

/// A layer's record without its name and offsets.
const ROW: usize = 4 + 1 + 1 + 2;

/// What a file held for its meshes that could not be put back on them: how
/// many meshes lost their subdivision levels, how many their layers, how
/// many their face groups and how many their paint.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Lost {
    pub levels: usize,
    pub layers: usize,
    pub groups: usize,
    pub paint: usize,
}

impl Lost {
    /// What to tell the sculptor, as a piece for a status line; `None` when
    /// nothing was lost. Neither loss changes the shape that was saved, and
    /// each sentence says so, because that is the first thing to wonder.
    pub fn note(self) -> Option<String> {
        let objects = |count: usize| if count == 1 { "1 object".to_owned() } else { format!("{count} objects") };
        let levels = (self.levels > 0).then(|| {
            format!(
                "The subdivision levels saved with {} could not be read (the file is damaged there): the model opened \
                 without its levels, as the level it was saved on.",
                objects(self.levels)
            )
        });
        let layers = (self.layers > 0).then(|| {
            format!(
                "The sculpt layers saved with {} could not be read (the file is damaged there): the shape is as it was \
                 saved, with what the layers added now part of the model.",
                objects(self.layers)
            )
        });
        let groups = (self.groups > 0).then(|| {
            format!(
                "The face groups saved with {} could not be read (the file is damaged there): the model is as it was \
                 saved, without its groups.",
                objects(self.groups)
            )
        });
        let paint = (self.paint > 0).then(|| {
            format!(
                "The paint saved with {} could not be read (the file is damaged there): the model is as it was                  saved, without its paint.",
                objects(self.paint)
            )
        });
        let notes: Vec<String> = levels.into_iter().chain(layers).chain(groups).chain(paint).collect();
        (!notes.is_empty()).then(|| notes.join(" "))
    }
}

/// A layer's name as it is written: all of it, unless it is longer than the
/// two bytes of its length can say.
fn name(layer: &SavedLayer) -> &[u8] {
    let mut end = layer.name.len().min(u16::MAX as usize);
    while !layer.name.is_char_boundary(end) {
        end -= 1;
    }
    &layer.name.as_bytes()[..end]
}

/// How long the body of a mesh's `LAYR` block is.
pub(super) fn length(saved: &SavedLayers) -> usize {
    HEAD + saved.layers.iter().map(|layer| ROW + name(layer).len() + layer.offsets.len() * 12).sum::<usize>()
}

/// Writes the body of the `LAYR` block of mesh `number`.
pub(super) fn write(w: &mut impl Write, number: u32, saved: &SavedLayers) -> std::io::Result<()> {
    let vertices = saved.layers.first().map_or(0, |layer| layer.offsets.len());
    let head = [number, LAYOUT, saved.layers.len() as u32, saved.recording.unwrap_or(0), saved.next_id];
    for value in head {
        w.write_all(&value.to_le_bytes())?;
    }
    w.write_all(&(vertices as u64).to_le_bytes())?;
    for layer in &saved.layers {
        let name = name(layer);
        w.write_all(&layer.id.to_le_bytes())?;
        w.write_all(&[layer.on as u8, layer.percent])?;
        w.write_all(&(name.len() as u16).to_le_bytes())?;
        w.write_all(name)?;
        if layer.offsets.iter().all(|offset| offset.is_finite()) {
            w.write_all(bytemuck::cast_slice(&layer.offsets))?;
        } else {
            // An offset that is not a number (damage from an earlier edit)
            // would cost the whole list when the file is read. The point it
            // belongs to is written mended, and the layer moves it nowhere.
            let sound: Vec<Vec3> = layer.offsets.iter().map(|o| if o.is_finite() { *o } else { Vec3::ZERO }).collect();
            w.write_all(bytemuck::cast_slice(&sound))?;
        }
    }
    Ok(())
}

/// The body of a `LAYR` block after the mesh's number, for a mesh of
/// `vertices` vertices; `length` is that of the whole body. `None` for one
/// whose parts do not add up, or of a layout this version does not know.
pub(super) fn read(r: &mut impl Read, length: u64, vertices: usize) -> Option<SavedLayers> {
    if read_u32(r)? != LAYOUT {
        return None;
    }
    let (count, recording, next_id) = (read_u32(r)? as u64, read_u32(r)?, read_u32(r)?);
    if read_u64(r)? != vertices as u64 {
        return None;
    }
    // The caller has checked that `length` bytes are really in the file, so
    // once the layers are known to fit in it no count can ask for more memory
    // than the file is big.
    let offsets = (vertices as u64).checked_mul(12)?;
    let mut left = length.checked_sub(HEAD as u64)?;
    if count == 0 || count.checked_mul(offsets.checked_add(ROW as u64)?)? > left {
        return None;
    }
    let mut layers = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let id = read_u32(r)?;
        let row = read_array::<u8>(r, 4).ok()?;
        let name_length = u16::from_le_bytes([row[2], row[3]]) as u64;
        left = left.checked_sub(ROW as u64 + name_length + offsets)?;
        let name = String::from_utf8(read_array::<u8>(r, name_length as usize).ok()?).ok()?;
        let offsets = read_array::<Vec3>(r, vertices).ok()?;
        layers.push(SavedLayer { id, name, on: row[0] != 0, percent: row[1], offsets });
    }
    (left == 0).then_some(SavedLayers { layers, recording: (recording != 0).then_some(recording), next_id })
}

#[cfg(test)]
mod tests;
