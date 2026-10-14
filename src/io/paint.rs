//! A mesh's paint in a `.sclp` file: the `PNTL` block (see `mesh/paint.rs`).
//!
//! A reader that knows nothing of paint steps over the block and opens the
//! model whole, unpainted. A block that cannot be read costs the paint and
//! nothing else, and the status line says so (`Lost::paint`).
//!
//! The body, after the mesh's number (u32): a layout number (u32, 1), the
//! mesh's vertex count (u64), how many layers (u32), the number the next
//! layer made will be given (u32), the active layer's number (u32, 0 for
//! none), how many local materials (u32, at most 8) and their palette slots
//! (a byte each). Then each layer, bottom first:
//!
//! ```text
//! number                    u32
//! flags                     u8   1 visible, 2 locked
//! opacity                   u8
//! colour blend              u8   paint::ColourBlend::to_byte
//! material blend            u8   0 Normal, 1 Add
//! mask source               u8   0 none, 1 paintable, 2 sculpt mask, 3 sculpt
//!                                mask inverted, 4 cavity, 5 face group
//! channels switched on      u8   paint::ChannelSet
//! lists present             u8   1 colour, 2 material, 4 rough/metal,
//!                                8 mask, 16 cavity
//! (nothing)                 u8
//! face group of the mask    u16
//! name length               u16, then the name (UTF-8)
//! the lists present, in that order, one entry per vertex in the order of
//! the mesh's MESH block: colour 4 bytes, material 2, rough/metal 4, mask 1,
//! cavity 1
//! ```
//!
//! Last, the finest subdivision level's copies of the layers, kept while the
//! mesh is below its finest level (`mesh/levels/painted.rs`): how many (u32),
//! then for each the layer's number (u32), the finest level's vertex count
//! (u64), the lists present (u8) and those lists, in the level stack's own
//! vertex order.
//!
//! **Versions.** The block has its own layout number, as every block does;
//! a later layout is stepped over by this reader (the paint is lost and the
//! status line says so) rather than misread. The file's magic is not changed
//! for paint (that would make every older app refuse a file with paint in
//! it, where now it opens the model unpainted); the JSON section records the
//! format the file was written in (`FORMAT`, in `io.rs`).
//!
//! Reading checks every count against the block's length before it asks for
//! any memory.

use std::io::{Read, Write};

use super::{read_array, read_u32, read_u64};
use crate::mesh::paint::{
    ChannelSet, Channels, ColourBlend, FinestCopy, MOST_MATERIALS, MOST_PAINT_LAYERS, MaskSource, Mat8, MaterialBlend,
    PaintLayer, PaintRow, PaintStack, RoughMetal8, Rgba8, Unorm8,
};

/// The layout written today.
const LAYOUT: u32 = 1;

/// The bytes of a layer's record before its name.
const ROW: u64 = 4 + 8 + 2 + 2;

const COLOUR: u8 = 1;
const MATERIAL: u8 = 2;
const ROUGH_METAL: u8 = 4;
const MASK: u8 = 8;
const CAVITY: u8 = 16;

/// Which lists `data` has, as the file says it.
fn present(data: &Channels) -> u8 {
    [
        (data.colour.is_some(), COLOUR),
        (data.material.is_some(), MATERIAL),
        (data.rough_metal.is_some(), ROUGH_METAL),
        (data.mask.is_some(), MASK),
        (data.cavity.is_some(), CAVITY),
    ]
    .iter()
    .filter(|(on, _)| *on)
    .fold(0, |bits, (_, bit)| bits | bit)
}

/// Bytes a vertex of the lists `bits` names.
fn per_vertex(bits: u8) -> u64 {
    [(COLOUR, 4), (MATERIAL, 2), (ROUGH_METAL, 4), (MASK, 1), (CAVITY, 1)]
        .iter()
        .filter(|(bit, _)| bits & bit != 0)
        .map(|(_, size)| size)
        .sum()
}

fn name(row: &PaintRow) -> &[u8] {
    let mut end = row.name.len().min(u16::MAX as usize);
    while !row.name.is_char_boundary(end) {
        end -= 1;
    }
    &row.name.as_bytes()[..end]
}

fn mask_bytes(mask: MaskSource) -> (u8, u16) {
    match mask {
        MaskSource::None => (0, 0),
        MaskSource::Paintable => (1, 0),
        MaskSource::SculptMask { invert: false } => (2, 0),
        MaskSource::SculptMask { invert: true } => (3, 0),
        MaskSource::Cavity => (4, 0),
        MaskSource::FaceGroup(g) => (5, g),
    }
}

fn mask_of(kind: u8, group: u16) -> MaskSource {
    match kind {
        1 => MaskSource::Paintable,
        2 => MaskSource::SculptMask { invert: false },
        3 => MaskSource::SculptMask { invert: true },
        4 => MaskSource::Cavity,
        5 => MaskSource::FaceGroup(group),
        _ => MaskSource::None,
    }
}

fn data_length(data: &Channels, vertices: usize) -> u64 {
    per_vertex(present(data)) * vertices as u64
}

/// How long the body of a mesh's `PNTL` block is.
pub(super) fn length(stack: &PaintStack) -> usize {
    let head = 4 + 4 + 8 + 4 * 4 + stack.table.len() as u64;
    let layers: u64 = stack.layers.iter().map(|l| ROW + name(&l.row).len() as u64 + data_length(&l.data, stack.len)).sum();
    let finest: u64 = stack
        .finest
        .iter()
        .map(|f| 4 + 8 + 1 + data_length(&f.data, finest_len(&f.data)))
        .sum();
    (head + layers + 4 + finest) as usize
}

/// How many vertices a finest copy's lists are for.
fn finest_len(data: &Channels) -> usize {
    data.colour
        .as_ref()
        .map(Vec::len)
        .or(data.material.as_ref().map(Vec::len))
        .or(data.rough_metal.as_ref().map(Vec::len))
        .or(data.mask.as_ref().map(Vec::len))
        .or(data.cavity.as_ref().map(Vec::len))
        .unwrap_or(0)
}

fn write_lists(w: &mut impl Write, data: &Channels) -> std::io::Result<()> {
    if let Some(l) = &data.colour {
        w.write_all(bytemuck::cast_slice(l))?;
    }
    if let Some(l) = &data.material {
        w.write_all(bytemuck::cast_slice(l))?;
    }
    if let Some(l) = &data.rough_metal {
        w.write_all(bytemuck::cast_slice(l))?;
    }
    if let Some(l) = &data.mask {
        w.write_all(bytemuck::cast_slice(l))?;
    }
    if let Some(l) = &data.cavity {
        w.write_all(bytemuck::cast_slice(l))?;
    }
    Ok(())
}

/// Writes the body of the `PNTL` block of mesh `number`. `stack` must be
/// fitted to the mesh (`Mesh::paint_stack`).
pub(super) fn write(w: &mut impl Write, number: u32, stack: &PaintStack) -> std::io::Result<()> {
    w.write_all(&number.to_le_bytes())?;
    w.write_all(&LAYOUT.to_le_bytes())?;
    w.write_all(&(stack.len as u64).to_le_bytes())?;
    let head = [stack.layers.len() as u32, stack.next_id, stack.active.unwrap_or(0), stack.table.len() as u32];
    for value in head {
        w.write_all(&value.to_le_bytes())?;
    }
    w.write_all(&stack.table)?;
    for layer in &stack.layers {
        let r = &layer.row;
        let (kind, group) = mask_bytes(r.mask);
        let material_blend = (r.material_blend == MaterialBlend::Add) as u8;
        let flags = r.visible as u8 | (r.locked as u8) << 1;
        w.write_all(&r.id.to_le_bytes())?;
        w.write_all(&[flags, r.opacity, r.colour_blend.to_byte(), material_blend, kind, r.channels.0, present(&layer.data), 0])?;
        w.write_all(&group.to_le_bytes())?;
        let name = name(r);
        w.write_all(&(name.len() as u16).to_le_bytes())?;
        w.write_all(name)?;
        write_lists(w, &layer.data)?;
    }
    w.write_all(&(stack.finest.len() as u32).to_le_bytes())?;
    for copy in &stack.finest {
        w.write_all(&copy.id.to_le_bytes())?;
        w.write_all(&(finest_len(&copy.data) as u64).to_le_bytes())?;
        w.write_all(&[present(&copy.data)])?;
        write_lists(w, &copy.data)?;
    }
    Ok(())
}

/// Reads the lists `bits` names, `count` entries each, having checked that
/// they fit in what is `left` of the block.
fn read_lists(r: &mut impl Read, bits: u8, count: usize, left: &mut u64) -> Option<Channels> {
    *left = left.checked_sub(per_vertex(bits).checked_mul(count as u64)?)?;
    let take = |bit: u8| bits & bit != 0;
    let mut data = Channels::default();
    if take(COLOUR) {
        data.colour = Some(read_array::<Rgba8>(r, count).ok()?);
    }
    if take(MATERIAL) {
        data.material = Some(read_array::<Mat8>(r, count).ok()?);
    }
    if take(ROUGH_METAL) {
        data.rough_metal = Some(read_array::<RoughMetal8>(r, count).ok()?);
    }
    if take(MASK) {
        data.mask = Some(read_array::<Unorm8>(r, count).ok()?);
    }
    if take(CAVITY) {
        data.cavity = Some(read_array::<Unorm8>(r, count).ok()?);
    }
    Some(data)
}

/// The body of a `PNTL` block after the mesh's number, for a mesh of
/// `vertices` vertices; `length` is that of the whole body. `None` for one
/// whose parts do not add up, or of a layout this version does not know.
pub(super) fn read(r: &mut impl Read, length: u64, vertices: usize) -> Option<PaintStack> {
    if read_u32(r)? != LAYOUT || read_u64(r)? != vertices as u64 {
        return None;
    }
    let (count, next_id, active, materials) = (read_u32(r)?, read_u32(r)?, read_u32(r)?, read_u32(r)? as usize);
    if count as usize > MOST_PAINT_LAYERS || materials > MOST_MATERIALS {
        return None;
    }
    let mut left = length.checked_sub(4 + 4 + 8 + 16 + materials as u64)?;
    let table = read_array::<u8>(r, materials).ok()?;
    let mut stack = PaintStack::new(vertices);
    stack.table = table;
    for _ in 0..count {
        left = left.checked_sub(ROW)?;
        let id = read_u32(r)?;
        let b = read_array::<u8>(r, 8).ok()?;
        let group = u16::from_le_bytes(read_array::<u8>(r, 2).ok()?.try_into().ok()?);
        let name_length = u16::from_le_bytes(read_array::<u8>(r, 2).ok()?.try_into().ok()?) as u64;
        left = left.checked_sub(name_length)?;
        let name = String::from_utf8(read_array::<u8>(r, name_length as usize).ok()?).ok()?;
        let data = read_lists(r, b[6], vertices, &mut left)?;
        if id == 0 || stack.index_of(id).is_some() {
            return None;
        }
        let row = PaintRow {
            id,
            name,
            visible: b[0] & 1 != 0,
            locked: b[0] & 2 != 0,
            opacity: b[1],
            colour_blend: ColourBlend::from_byte(b[2]),
            material_blend: if b[3] == 1 { MaterialBlend::Add } else { MaterialBlend::Normal },
            mask: mask_of(b[4], group),
            channels: ChannelSet(b[5] & ChannelSet::ALL.0),
        };
        stack.layers.push(PaintLayer { row, data });
    }
    left = left.checked_sub(4)?;
    let copies = read_u32(r)?;
    for _ in 0..copies {
        left = left.checked_sub(4 + 8 + 1)?;
        let id = read_u32(r)?;
        let finest = read_u64(r)?;
        let bits = read_array::<u8>(r, 1).ok()?[0];
        // A count larger than what is left could never be read.
        if finest > left {
            return None;
        }
        let data = read_lists(r, bits, finest as usize, &mut left)?;
        stack.finest.push(FinestCopy { id, data });
    }
    if left != 0 {
        return None;
    }
    let most = stack.layers.iter().map(|l| l.row.id).max().unwrap_or(0);
    stack.next_id = next_id.max(most + 1);
    stack.active = (active != 0 && stack.index_of(active).is_some()).then_some(active);
    Some(stack)
}

#[cfg(test)]
mod tests;
