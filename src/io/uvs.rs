//! A mesh's UVs in files (see `mesh/uvs.rs` for what they are): the `UVCO`
//! block of a `.sclp`, and the `vt` lines of an OBJ, both ways. FBX keeps
//! them in `fbx/uvs.rs` and `fbx_read/uvs.rs`, glTF in `gltf.rs`; every
//! reader hands them to `polygons.rs` with the faces, one for each corner.
//!
//! **`UVCO`**, after its mesh, if it has UVs: the mesh's number (u32), how
//! many triangle corners there are (u64), then a UV for each (U then V, an
//! f32 each), in the order the mesh's `MESH` block lists the corners. A
//! reader that knows nothing of UVs steps over the block and opens the model
//! without them. A block that cannot be read (its count is not the mesh's,
//! or the count and the block's length do not agree) costs the UVs and
//! nothing else, and the status line says so (`Lost`).
//!
//! **OBJ.** UVs are `vt u v` lines, and each corner of a face names one
//! (`f v/vt` or `f v/vt/vn`), so a seam is a vertex whose corners name two.
//! An export writes each different UV once and every face with its corners'
//! UVs; quads stay quads where their triangles agree on the UV of each
//! corner (`Mesh::faces_with_uvs`). The numbers are written in full, so a
//! model that goes out and comes back has the same UVs to the bit.

use std::collections::HashMap;
use std::io::{Read, Write};

use glam::Vec2;

use super::{read_array, read_u64};
use crate::mesh::Mesh;
use crate::mesh::uvs::UvFaces;

/// The body up to the first UV: the mesh's number and the corner count.
const HEAD: usize = 4 + 8;

// ---------------------------------------------------------------- .sclp

/// How long the body of a mesh's `UVCO` block is.
pub(super) fn length(mesh: &Mesh) -> usize {
    HEAD + mesh.uvs().len() * 8
}

/// Writes the body of the `UVCO` block of mesh `number`.
pub(super) fn write(w: &mut impl Write, number: u32, mesh: &Mesh) -> std::io::Result<()> {
    w.write_all(&number.to_le_bytes())?;
    w.write_all(&(mesh.uvs().len() as u64).to_le_bytes())?;
    w.write_all(bytemuck::cast_slice(mesh.uvs()))
}

/// The body of a `UVCO` block after the mesh's number, for a mesh of
/// `corners` triangle corners; `length` is that of the whole body. `None`
/// for one whose count is not the mesh's or does not fill the block.
pub(super) fn read(r: &mut impl Read, length: u64, corners: usize) -> Option<Vec<Vec2>> {
    let count = read_u64(r)?;
    // The caller has checked that `length` bytes are really in the file, so
    // once the count fills the block exactly it cannot ask for more memory
    // than the file is big.
    if count != corners as u64 || count.checked_mul(8)?.checked_add(HEAD as u64)? != length {
        return None;
    }
    read_array::<Vec2>(r, corners).ok()
}

// ---------------------------------------------------------------- OBJ reading

/// The `vt` lines of an OBJ file as it is read, and the UVs of the face
/// being read.
#[derive(Default)]
pub(super) struct ObjUvs {
    points: Vec<Vec2>,
    face: Vec<Vec2>,
}

impl ObjUvs {
    /// A `vt u v` line (a third number, which some programs write, is
    /// passed over).
    pub(super) fn vt<'a>(&mut self, mut parts: impl Iterator<Item = &'a str>) {
        let mut number = || parts.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.0);
        let u = number();
        self.points.push(Vec2::new(u, number()));
    }

    /// A face begins.
    pub(super) fn begin(&mut self) {
        self.face.clear();
    }

    /// One corner of the face: `field` is what its token has after the first
    /// slash, the number of its `vt` line (counted from the end when
    /// negative). A corner that names none is given none (`Vec2::NAN`).
    pub(super) fn corner(&mut self, field: Option<&str>) {
        let named = field.and_then(|f| f.parse::<i64>().ok()).and_then(|i| {
            let at = if i < 0 { self.points.len() as i64 + i } else { i - 1 };
            usize::try_from(at).ok().and_then(|at| self.points.get(at).copied())
        });
        self.face.push(named.unwrap_or(Vec2::NAN));
    }

    /// The UV of each corner of the face, or `None` while the file has had
    /// no `vt` lines (most OBJ files from sculpting programs).
    pub(super) fn face(&self) -> Option<&[Vec2]> {
        (!self.points.is_empty()).then_some(&self.face[..])
    }
}

// ---------------------------------------------------------------- OBJ writing

/// What an OBJ export writes of a mesh with UVs: its faces with their UVs,
/// and those UVs as `vt` lines.
pub(super) struct ObjFaces<'a> {
    faces: UvFaces<'a>,
    /// Each different UV once, in the order the faces first use them: the
    /// `vt` lines.
    table: Vec<Vec2>,
    /// The place in `table` of each corner's UV, as `faces.uvs`.
    index: Vec<u32>,
}

impl<'a> ObjFaces<'a> {
    /// `None` for a mesh without UVs, which is written as it always was.
    pub(super) fn of(mesh: &'a Mesh) -> Option<ObjFaces<'a>> {
        let faces = mesh.faces_with_uvs()?;
        let mut seen: HashMap<[u32; 2], u32> = HashMap::new();
        let mut table = Vec::new();
        let index = faces
            .uvs
            .iter()
            .map(|uv| {
                // +0.0 so that -0.0 and 0.0 are one line.
                let key = [(uv.x + 0.0).to_bits(), (uv.y + 0.0).to_bits()];
                *seen.entry(key).or_insert_with(|| {
                    table.push(*uv);
                    table.len() as u32 - 1
                })
            })
            .collect();
        Some(ObjFaces { faces, table, index })
    }

    /// How many `vt` lines each place of the mesh writes.
    pub(super) fn count(&self) -> u32 {
        self.table.len() as u32
    }

    /// Writes the `vt` lines, numbered from `uv_base`, then every face, its
    /// vertices numbered from `base`, turned round when `mirrored`. A mesh
    /// with face groups has each group's faces under a `g` line of its own,
    /// as `groups::write_obj_faces` writes them.
    pub(super) fn write(
        &self,
        w: &mut impl Write,
        mesh: &Mesh,
        mirrored: bool,
        with_normals: bool,
        base: u32,
        uv_base: u32,
    ) -> std::io::Result<()> {
        for uv in &self.table {
            writeln!(w, "vt {} {}", uv.x, uv.y)?;
        }
        let faces = &self.faces.faces;
        let mut first = Vec::with_capacity(faces.len());
        let mut corners = 0;
        for face in faces {
            first.push(corners);
            corners += face.len();
        }
        let group = |f: usize| mesh.group_of(self.faces.triangle[f]);
        let mut order: Vec<usize> = (0..faces.len()).collect();
        if mesh.has_groups() {
            order.sort_by_key(|&f| group(f));
        }
        let mut current = None;
        for f in order {
            if mesh.has_groups() && current != Some(group(f)) {
                writeln!(w, "g {}", super::groups::obj_name(group(f)))?;
                current = Some(group(f));
            }
            let uvs = &self.index[first[f]..first[f] + faces[f].len()];
            write_face(w, faces[f], uvs, mirrored, with_normals, (base, uv_base))?;
        }
        Ok(())
    }
}

/// One `f` line: each corner as `v/vt/vn` (or `v/vt` without normals),
/// numbered from `bases` (vertices, then UVs), in turn or turned round.
fn write_face(
    w: &mut impl Write,
    corners: &[u32],
    uvs: &[u32],
    reversed: bool,
    with_normals: bool,
    (base, uv_base): (u32, u32),
) -> std::io::Result<()> {
    write!(w, "f")?;
    for k in 0..corners.len() {
        let k = if reversed { corners.len() - 1 - k } else { k };
        let (v, vt) = (corners[k] + base + 1, uvs[k] + uv_base + 1);
        if with_normals {
            write!(w, " {v}/{vt}/{v}")?;
        } else {
            write!(w, " {v}/{vt}")?;
        }
    }
    writeln!(w)
}

#[cfg(test)]
mod tests;
