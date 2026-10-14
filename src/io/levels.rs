//! A mesh's subdivision levels in a `.sclp` file: the `LVLS` block.
//!
//! The mesh the file holds is the level that was current, written as any
//! other mesh is (`MESH`, with its `MASK`, `QUAD` and `HIDE`), so a reader
//! that knows nothing of levels opens the model as it was on screen and
//! steps over this block. What is here is the rest (see `mesh/levels.rs`):
//! the stack as plain lists, and the two lists that say which of the file
//! mesh's vertices and triangles are which of the level's.
//!
//! The body, after the mesh's number (u32) and a layout number (u32):
//! eleven counts (u64 each; twelve in layout 2), then the lists they size, in
//! this order.
//!
//!   - the coarsest level: vertex count; triangle corners (u32 each); polygon
//!     face starts (u32 each, none for a triangle mesh) and corners (u32)
//!   - how each finer level was made (a byte each: 0 smooth, 1 flat)
//!   - the level that is current (a count with no list)
//!   - the finest level's positions (three f32) and mask (f32), which the
//!     stack keeps while a coarser level is current; none on the finest
//!   - the level's vertices as numbers of the file mesh (u32 each), and for
//!     each triangle of the file mesh the level's triangle (u32 each)
//!   - how many levels have remembered hidden faces written (a count), then
//!     for each: how many flags (u64, 0 for none) and a byte per flag
//!   - layout 2 only: how many sculpt layers have a finest copy written (the
//!     twelfth count), then for each its number (u32) and an offset (three
//!     f32) for every vertex of the finest level (see `mesh/levels/layered.rs`)
//!
//! Layout 2 is written only for a mesh that has layers and is below its
//! finest level; every other mesh's block is layout 1, byte for byte what it
//! was before there were layers.
//!
//! Reading checks that the counts add up to the block's length before it
//! asks for any memory, as the mesh reader does, and everything else is
//! checked when the levels are put back on the mesh (`MeshLevels::restored`,
//! which refuses lists that are not that mesh's). A block that fails either
//! check costs the levels and nothing else: the mesh still opens.

use std::io::{Read, Write};

use glam::Vec3;
use sculpt_multires::{LevelsParts, Polys as LevelPolys, Split};

use super::{LoadedMesh, read_array, read_u32, read_u64};
use crate::mesh::groups::renumbered;
use crate::mesh::layers::Layers;
use crate::mesh::levels::{MeshLevels, SavedLevels};
use crate::mesh::{Mesh, Polys};

/// The layout without sculpt layers, and its counts at the head of the body.
const LAYOUT: u32 = 1;
const COUNTS: usize = 11;

/// The layout with the layers' finest copies after everything else, and one
/// more count for them.
const LAYOUT_LAYERS: u32 = 2;

/// The sizes of everything in the body, in the order they are written.
struct Counts {
    base_vertices: u64,
    corners: u64,
    poly_starts: u64,
    poly_corners: u64,
    splits: u64,
    current: u64,
    kept: u64,
    vertices: u64,
    triangles: u64,
    hidden_levels: u64,
    /// Every remembered hidden flag, with the count in front of each level's.
    hidden_bytes: u64,
    /// Sculpt layers with a finest copy; more than none makes it layout 2.
    layer_copies: u64,
}

impl Counts {
    fn of(saved: &SavedLevels) -> Counts {
        let parts = &saved.parts;
        let polys = parts.polys.as_ref();
        let flags: usize = saved.hidden.iter().map(|flags| flags.as_ref().map_or(0, Vec::len)).sum();
        Counts {
            base_vertices: parts.vertex_count as u64,
            corners: parts.triangles.len() as u64,
            poly_starts: polys.map_or(0, |p| p.start.len()) as u64,
            poly_corners: polys.map_or(0, |p| p.indices.len()) as u64,
            splits: parts.splits.len() as u64,
            current: parts.current as u64,
            kept: parts.positions.len() as u64,
            vertices: saved.vertex.len() as u64,
            triangles: saved.triangle.len() as u64,
            hidden_levels: saved.hidden.len() as u64,
            hidden_bytes: (saved.hidden.len() * 8 + flags) as u64,
            layer_copies: saved.finest.len() as u64,
        }
    }

    fn layout(&self) -> u32 {
        if self.layer_copies > 0 { LAYOUT_LAYERS } else { LAYOUT }
    }

    /// The counts as they are written: the twelfth only in layout 2.
    fn list(&self) -> Vec<u64> {
        let mut list = self.first().to_vec();
        list.extend((self.layer_copies > 0).then_some(self.layer_copies));
        list
    }

    fn first(&self) -> [u64; COUNTS] {
        [
            self.base_vertices,
            self.corners,
            self.poly_starts,
            self.poly_corners,
            self.splits,
            self.current,
            self.kept,
            self.vertices,
            self.triangles,
            self.hidden_levels,
            self.hidden_bytes,
        ]
    }

    /// The length of the whole body, or `None` if the counts are too large
    /// to mean anything.
    fn length(&self) -> Option<u64> {
        let words = [self.corners, self.poly_starts, self.poly_corners, self.vertices, self.triangles];
        let mut total = 8 + self.list().len() as u64 * 8;
        for words in words {
            total = total.checked_add(words.checked_mul(4)?)?;
        }
        total = total.checked_add(self.splits)?;
        total = total.checked_add(self.kept.checked_mul(16)?)?;
        total = total.checked_add(self.hidden_bytes)?;
        // Each layer's copy: its number, and an offset for every finest vertex.
        total.checked_add(self.layer_copies.checked_mul(self.kept.checked_mul(12)?.checked_add(4)?)?)
    }
}

/// How long the body of a mesh's `LVLS` block is.
pub(super) fn length(saved: &SavedLevels) -> usize {
    Counts::of(saved).length().unwrap_or(0) as usize
}

/// Writes the body of the `LVLS` block of mesh `number`.
pub(super) fn write(w: &mut impl Write, number: u32, saved: &SavedLevels) -> std::io::Result<()> {
    let parts = &saved.parts;
    let counts = Counts::of(saved);
    w.write_all(&number.to_le_bytes())?;
    w.write_all(&counts.layout().to_le_bytes())?;
    for count in counts.list() {
        w.write_all(&count.to_le_bytes())?;
    }
    w.write_all(bytemuck::cast_slice(&parts.triangles))?;
    if let Some(polys) = &parts.polys {
        w.write_all(bytemuck::cast_slice(&polys.start))?;
        w.write_all(bytemuck::cast_slice(&polys.indices))?;
    }
    let splits: Vec<u8> = parts.splits.iter().map(|split| split.to_byte()).collect();
    w.write_all(&splits)?;
    w.write_all(bytemuck::cast_slice(&parts.positions))?;
    w.write_all(bytemuck::cast_slice(&parts.mask))?;
    w.write_all(bytemuck::cast_slice(&saved.vertex))?;
    w.write_all(bytemuck::cast_slice(&saved.triangle))?;
    for flags in &saved.hidden {
        let flags = flags.as_deref().unwrap_or(&[]);
        w.write_all(&(flags.len() as u64).to_le_bytes())?;
        w.write_all(&flags.iter().map(|&hidden| hidden as u8).collect::<Vec<u8>>())?;
    }
    for (id, offsets) in &saved.finest {
        w.write_all(&id.to_le_bytes())?;
        w.write_all(bytemuck::cast_slice(offsets))?;
    }
    Ok(())
}

/// The body of a `LVLS` block after the mesh's number; `length` is that of
/// the whole body. `None` for one whose parts do not add up, or of a layout
/// this version does not know: the caller steps over it, and the mesh opens
/// without levels.
pub(super) fn read(r: &mut impl Read, length: u64) -> Option<SavedLevels> {
    let layout = read_u32(r)?;
    if layout != LAYOUT && layout != LAYOUT_LAYERS {
        return None;
    }
    let mut list = [0u64; COUNTS];
    for count in &mut list {
        *count = read_u64(r)?;
    }
    let layer_copies = if layout == LAYOUT_LAYERS { read_u64(r)? } else { 0 };
    let [
        base_vertices,
        corners,
        poly_starts,
        poly_corners,
        splits,
        current,
        kept,
        vertices,
        triangles,
        hidden_levels,
        hidden_bytes,
    ] = list;
    let counts = Counts {
        base_vertices,
        corners,
        poly_starts,
        poly_corners,
        splits,
        current,
        kept,
        vertices,
        triangles,
        hidden_levels,
        hidden_bytes,
        layer_copies,
    };
    // The caller has checked that `length` bytes are really in the file, so
    // no count can ask for more memory than the file is big.
    let layout_fits = (layout == LAYOUT_LAYERS) == (layer_copies > 0);
    if counts.length() != Some(length) || hidden_levels.checked_mul(8)? > hidden_bytes || !layout_fits {
        return None;
    }
    let triangle_corners = read_array::<u32>(r, corners as usize).ok()?;
    let polys = if poly_starts > 0 {
        let start = read_array::<u32>(r, poly_starts as usize).ok()?;
        Some(LevelPolys { start, indices: read_array::<u32>(r, poly_corners as usize).ok()? })
    } else {
        None
    };
    let split_bytes = read_array::<u8>(r, splits as usize).ok()?;
    let splits = split_bytes.iter().map(|&byte| Split::from_byte(byte)).collect::<Option<Vec<Split>>>()?;
    let positions = read_array::<Vec3>(r, kept as usize).ok()?;
    let mask = read_array::<f32>(r, kept as usize).ok()?;
    // A point that is not a number would be carried into every finer level
    // the first time a level changed.
    if !positions.iter().all(|p| p.is_finite()) || !mask.iter().all(|m| m.is_finite()) {
        return None;
    }
    let vertex = read_array::<u32>(r, vertices as usize).ok()?;
    let triangle = read_array::<u32>(r, triangles as usize).ok()?;
    let mut hidden = Vec::new();
    let mut left = hidden_bytes - hidden_levels * 8;
    for _ in 0..hidden_levels {
        let flags = read_u64(r)?;
        left = left.checked_sub(flags)?;
        let flags = read_array::<u8>(r, flags as usize).ok()?;
        hidden.push((!flags.is_empty()).then(|| flags.iter().map(|&flag| flag != 0).collect()));
    }
    if left != 0 {
        return None;
    }
    let mut finest = Vec::new();
    for _ in 0..layer_copies {
        finest.push((read_u32(r)?, read_array::<Vec3>(r, kept as usize).ok()?));
    }
    let parts = LevelsParts {
        vertex_count: base_vertices as usize,
        triangles: triangle_corners,
        polys,
        splits,
        current: current as usize,
        positions,
        mask,
    };
    Some(SavedLevels { parts, vertex, triangle, hidden, finest, groups: Vec::new() })
}

/// Builds a mesh that was saved with levels, sculpt layers or face groups,
/// and puts them back on it. The build renumbers the mesh, and the levels'
/// lists, the layers' offsets, the groups, the quads' corners and the hidden
/// flags all follow the same renumbering. Levels, layers or groups that turn
/// out not to be this mesh's are left off, and the second value says which
/// were: the shape is whole without any of them.
pub(super) fn build(loaded: LoadedMesh) -> (Mesh, [bool; 4]) {
    let LoadedMesh { positions, indices, mask, polys, hidden, levels, layers, groups, paint, lost } = loaded;
    let verts = positions.len();
    let (mut mesh, numbering) = Mesh::new_numbered(positions, indices, mask, &hidden.unwrap_or_default());
    let went = |v: u32| numbering.vertex.get(v as usize).copied().unwrap_or(u32::MAX);
    let sound = |p: &Polys| {
        p.start.first() == Some(&0)
            && p.start.last().map(|&end| end as usize) == Some(p.indices.len())
            && p.start.windows(2).all(|w| w[0] <= w[1])
            && p.indices.iter().all(|&i| (i as usize) < verts)
    };
    if let Some(polys) = polys.filter(sound) {
        let corners: Vec<u32> = polys.indices.iter().map(|&v| went(v)).collect();
        if !corners.contains(&u32::MAX) {
            mesh.polys = Some(Polys { start: polys.start, indices: corners });
        }
    }
    let [mut lost_levels, mut lost_layers, lost_groups, mut lost_paint] = lost;
    // The groups go on first: the levels keep the other levels' groups, and
    // those are good only for the groups this mesh has.
    let mut level_groups = Vec::new();
    if let Some(saved) = groups {
        mesh.set_group_ids(renumbered(&saved.ids, &numbering.triangle));
        level_groups = saved.levels;
    }
    if let Some(mut saved) = levels {
        saved.groups = level_groups;
        mesh.levels = MeshLevels::restored(saved, &mesh, &numbering);
        lost_levels |= mesh.levels.is_none();
    }
    if let Some(saved) = layers {
        mesh.layers = Layers::restored(saved, mesh.vertex_count(), Some(&numbering.vertex));
        lost_layers |= mesh.layers.is_none();
    }
    if let Some(stack) = paint {
        // After the levels: the finest copies are good only with the levels they were made on.
        lost_paint |= !crate::mesh::paint::restore(&mut mesh, stack, &numbering.vertex);
    }
    if lost_paint {
        log::warn!("the paint saved with a mesh could not be put back on it, and it opened without it");
    }
    if lost_levels {
        log::warn!("the subdivision levels saved with a mesh could not be put back on it, and were left off");
    }
    if lost_layers {
        log::warn!("the sculpt layers saved with a mesh could not be put back on it, and are part of its shape now");
    }
    if lost_groups {
        log::warn!("the face groups saved with a mesh could not be read, and it opened without them");
    }
    (mesh, [lost_levels, lost_layers, lost_groups, lost_paint])
}

#[cfg(test)]
mod tests;
