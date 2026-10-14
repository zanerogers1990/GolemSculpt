//! A mesh's face groups in a `.sclp` file: the `GRPS` block.
//!
//! Only the numbers are kept: a group's colour is worked out from its number
//! (`mesh/groups.rs`), so the same group is the same colour when the file is
//! opened again. A reader that knows nothing of groups steps over the block
//! and opens the model whole; a block that cannot be read costs the groups
//! and nothing else, and the status line says so (`Lost`).
//!
//! The body, after the mesh's number (u32): a layout number (u32, 1), the
//! mesh's triangle count (u64) and how many lists of other subdivision
//! levels follow (u32). Then the group of every triangle (u16 each), in the
//! order the mesh's `MESH` block has them. Then, for a mesh with subdivision
//! levels, the groups each other level was left with, so that a step down
//! and up again after a save still gives back the exact border
//! (`mesh/levels/grouped.rs`): for each level a count (u64, 0 for none) and
//! that many u16, in the level stack's own triangle order.
//!
//! Reading checks the triangle count against the mesh and that everything
//! fits in the block before it asks for any memory.

use std::io::{Read, Write};

use super::{RawMesh, read_array, read_u32, read_u64, write_face};
use crate::mesh::Mesh;
use crate::mesh::groups::{MOST_GROUPS, built_with};

/// The layout written today.
const LAYOUT: u32 = 1;

/// The body up to the first number: the mesh's number, the layout, the
/// triangle count and how many level lists there are.
const HEAD: usize = 4 + 4 + 8 + 4;

/// A mesh's face groups as a file keeps them.
pub struct SavedGroups {
    /// One number per triangle of the mesh as the file holds it.
    pub ids: Vec<u16>,
    /// The groups each subdivision level was left with; empty for a mesh
    /// without levels, or whose groups have changed since.
    pub levels: Vec<Option<Vec<u16>>>,
}

/// What a file should keep of `mesh`'s groups, if it has any.
pub(super) fn saved(mesh: &Mesh) -> Option<SavedGroups> {
    mesh.has_groups().then(|| SavedGroups {
        ids: mesh.group_ids().to_vec(),
        levels: mesh.levels.as_ref().map_or(Vec::new(), |levels| levels.saved_groups(mesh)),
    })
}

/// How long the body of a mesh's `GRPS` block is.
pub(super) fn length(saved: &SavedGroups) -> usize {
    let lists: usize = saved.levels.iter().map(|list| 8 + list.as_ref().map_or(0, Vec::len) * 2).sum();
    HEAD + saved.ids.len() * 2 + lists
}

/// Writes the body of the `GRPS` block of mesh `number`.
pub(super) fn write(w: &mut impl Write, number: u32, saved: &SavedGroups) -> std::io::Result<()> {
    w.write_all(&number.to_le_bytes())?;
    w.write_all(&LAYOUT.to_le_bytes())?;
    w.write_all(&(saved.ids.len() as u64).to_le_bytes())?;
    w.write_all(&(saved.levels.len() as u32).to_le_bytes())?;
    w.write_all(bytemuck::cast_slice(&saved.ids))?;
    for list in &saved.levels {
        let list = list.as_deref().unwrap_or(&[]);
        w.write_all(&(list.len() as u64).to_le_bytes())?;
        w.write_all(bytemuck::cast_slice(list))?;
    }
    Ok(())
}

/// The body of a `GRPS` block after the mesh's number, for a mesh of
/// `triangles` triangles; `length` is that of the whole body. `None` for one
/// whose parts do not add up, or of a layout this version does not know.
pub(super) fn read(r: &mut impl Read, length: u64, triangles: usize) -> Option<SavedGroups> {
    if read_u32(r)? != LAYOUT || read_u64(r)? != triangles as u64 {
        return None;
    }
    let lists = read_u32(r)? as u64;
    // The caller has checked that `length` bytes are really in the file, so
    // once the lists are known to fit in it no count can ask for more memory
    // than the file is big.
    let mut left = length.checked_sub(HEAD as u64)?.checked_sub((triangles as u64).checked_mul(2)?)?;
    if lists.checked_mul(8)? > left {
        return None;
    }
    let ids = read_array::<u16>(r, triangles).ok()?;
    let mut levels = Vec::with_capacity(lists as usize);
    for _ in 0..lists {
        let count = read_u64(r)?;
        left = left.checked_sub(count.checked_mul(2)?.checked_add(8)?)?;
        let list = read_array::<u16>(r, count as usize).ok()?;
        levels.push((!list.is_empty()).then_some(list));
    }
    (left == 0).then_some(SavedGroups { ids, levels })
}

// ---------------------------------------------------------------- OBJ files
//
// An OBJ file says which group a face is in with a `g` line before it, which
// is how ZBrush writes its polygroups and how most modellers write theirs.
// So an export writes each group's faces after a `g` line of its own, and an
// import reads `g` lines back as groups.
//
// The names written are `Group_<number>`, and a name of that form read back
// is that number again, so a model that goes out and comes back keeps its
// groups' numbers and with them their colours. Any other program's names
// become new groups in the order they are met. Faces written before any `g`
// line, or under `Ungrouped` or OBJ's own `default`, are in no group; and a
// file whose faces are all in one and the same group says nothing about
// groups at all (every exporter writes at least one `g` line), so it is
// read as having none.

/// The name a group is written under in an OBJ file.
fn obj_name(group: u16) -> String {
    if group == 0 { "Ungrouped".to_owned() } else { format!("Group_{group}") }
}

/// Writes the faces of a mesh that has groups, each group's after a `g` line
/// of its own, quads as quads; `false`, writing nothing, for a mesh without
/// groups. Within a group the faces keep the order they have in the mesh.
pub(super) fn write_obj_faces(
    w: &mut impl Write,
    mesh: &Mesh,
    mirrored: bool,
    with_normals: bool,
    base: u32,
) -> std::io::Result<bool> {
    if !mesh.has_groups() {
        return Ok(false);
    }
    let faces: Vec<(u16, &[u32])> = match (&mesh.polys, mesh.face_groups()) {
        (Some(polys), Some(of_face)) => {
            let face = |(f, w): (usize, &[u32])| (of_face[f], &polys.indices[w[0] as usize..w[1] as usize]);
            polys.start.windows(2).enumerate().map(face).collect()
        }
        _ => mesh.group_ids().iter().copied().zip(mesh.indices.chunks_exact(3)).collect(),
    };
    let mut order: Vec<usize> = (0..faces.len()).collect();
    order.sort_by_key(|&f| faces[f].0);
    let mut current = None;
    for f in order {
        let (group, corners) = faces[f];
        if current != Some(group) {
            writeln!(w, "g {}", obj_name(group))?;
            current = Some(group);
        }
        write_face(w, corners, mirrored, with_normals, base)?;
    }
    Ok(true)
}

/// The `g` lines of an OBJ file as it is read: which name each triangle was
/// under.
#[derive(Default)]
pub(super) struct ObjGroups {
    names: Vec<String>,
    /// The name in force: its place in `names`.
    current: Option<usize>,
    /// For each triangle read, the name it was under.
    of_triangle: Vec<Option<usize>>,
}

impl ObjGroups {
    /// A `g` line: the faces after it are in the group it names. Several
    /// names on one line (OBJ allows it) are taken together as one name.
    pub(super) fn named(&mut self, line: &str) {
        let name = line.split_ascii_whitespace().skip(1).collect::<Vec<_>>().join(" ");
        let at = self.names.iter().position(|known| *known == name).unwrap_or_else(|| {
            self.names.push(name);
            self.names.len() - 1
        });
        self.current = Some(at);
    }

    /// One more triangle was read, under the name in force.
    pub(super) fn triangle(&mut self) {
        self.of_triangle.push(self.current);
    }

    /// The group of each triangle `kept` says the mesh keeps, in order;
    /// empty when the file says nothing about groups (see the notes above).
    pub(super) fn ids(&self, kept: impl Fn(usize) -> bool) -> Vec<u16> {
        // A name of our own is its number; any other takes the lowest number free.
        let own = |name: &str| name.strip_prefix("Group_")?.parse::<u16>().ok().filter(|&id| id != 0);
        let none = |name: &str| name.is_empty() || name.eq_ignore_ascii_case("Ungrouped") || name == "default";
        let mut used = vec![false; MOST_GROUPS + 1];
        used[0] = true;
        let mut number: Vec<u16> = self.names.iter().map(|name| own(name).unwrap_or(0)).collect();
        for &id in &number {
            used[id as usize] = true;
        }
        let mut next = 1;
        for (id, name) in number.iter_mut().zip(&self.names) {
            if *id != 0 || none(name) {
                continue;
            }
            while next < used.len() && used[next] {
                next += 1;
            }
            // Past the most groups there can be, the rest are in none.
            if next < used.len() {
                (*id, used[next]) = (next as u16, true);
            }
        }
        let ids: Vec<u16> = (0..self.of_triangle.len())
            .filter(|&t| kept(t))
            .map(|t| self.of_triangle[t].map_or(0, |name| number[name]))
            .collect();
        let first = ids.first().copied().unwrap_or(0);
        if ids.iter().all(|&id| id == first) { Vec::new() } else { ids }
    }
}

/// The mesh of a model read from a file, with the groups the file gave it,
/// and its colours as a paint layer if it gave colours (`io/painted.rs`).
pub fn imported_mesh(raw: RawMesh) -> Mesh {
    if !raw.colours.is_empty() && raw.colours.len() == raw.positions.len() {
        let stack = super::painted::imported_stack(raw.colours);
        return crate::mesh::paint::built_with(raw.positions, raw.indices, None, &raw.groups, Some(&stack));
    }
    built_with(raw.positions, raw.indices, None, &raw.groups)
}

#[cfg(test)]
mod tests;
