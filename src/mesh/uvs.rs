//! UVs: where each corner of each triangle lies on a flat picture, so that
//! maps baked from the sculpt (`bake.rs`), or painted in another program,
//! have somewhere to go.
//!
//! **One point per triangle corner**, in the order of `Mesh::indices`: the UV
//! of corner `c` of triangle `t` is `uvs[t * 3 + c]`, as its vertex is
//! `indices[t * 3 + c]`. They are kept per corner and not per vertex because
//! a seam is a vertex with two UVs: the sculpt stays one connected surface
//! while the picture is cut apart, which is also how the files keep them
//! (an OBJ's `vt` numbers on each face corner, an FBX's `ByPolygonVertex`).
//! U runs to the right and V upward, as in OBJ and FBX; glTF's V runs down,
//! and is turned over as it is read (`io/gltf.rs`).
//!
//! The list is empty while the mesh has no UVs, which is the usual case and
//! costs nothing. With UVs it is 24 bytes a triangle.
//!
//! **Following the mesh.** UVs belong to the triangles, so:
//!
//! - a stroke moves vertices and leaves the UVs as they are;
//! - a rebuild that renumbers the same triangles (a file opened, a copy made,
//!   Set Origin) carries them by the numbering ([`renumbered`]). Building a
//!   mesh puts its triangles in another order but never turns one round:
//!   corner `c` of a triangle of the new mesh is corner `c` of the triangle
//!   it was (`Bvh::build` keeps each triangle's corners in their order), so a
//!   triangle's three UVs go with it as they are;
//! - dynamic topology retires them the first time it changes a triangle, as
//!   it retires quads, and the stroke's undo brings them back (`dyntopo.rs`);
//! - anything that makes new triangles (remesh, decimate, a boolean, a cut,
//!   a step between subdivision levels) builds a new mesh, which starts
//!   without them.

use glam::Vec2;
use rayon::prelude::*;

use super::Mesh;

/// Triangles handed to one rayon task.
const PER_TASK: usize = 1 << 14;

/// The UVs of a mesh rebuilt from the one `uvs` are of, the same triangles in
/// another order: `triangle` says, for each triangle of the new mesh, which
/// of the old it is (`levels::Numbering::triangle`). Empty if the old mesh
/// had none, or a number falls outside its list.
pub fn renumbered(uvs: &[Vec2], triangle: &[u32]) -> Vec<Vec2> {
    let old = uvs.len() / 3;
    if uvs.is_empty() || !triangle.par_iter().with_min_len(PER_TASK).all(|&t| (t as usize) < old) {
        return Vec::new();
    }
    let three = |&t: &u32| {
        let at = t as usize * 3;
        [uvs[at], uvs[at + 1], uvs[at + 2]]
    };
    triangle.par_iter().with_min_len(PER_TASK).flat_map_iter(three).collect()
}

/// A mesh's faces as an export writes them, with the UV of every corner of
/// every face (see [`Mesh::faces_with_uvs`]).
pub struct UvFaces<'a> {
    /// Each face's corners, as the mesh numbers its vertices: a quad's own
    /// list, or a triangle's.
    pub faces: Vec<&'a [u32]>,
    /// The UV of each corner of each face, the faces one after another.
    pub uvs: Vec<Vec2>,
    /// One triangle of each face (its lowest numbered), which says what face
    /// group it is in.
    pub triangle: Vec<u32>,
}

impl Mesh {
    /// Whether the mesh has UVs.
    #[inline]
    pub fn has_uvs(&self) -> bool {
        !self.uvs.is_empty()
    }

    /// The UV of every triangle corner, in the order of `indices`: corner `c`
    /// of triangle `t` is at `t * 3 + c`. Empty while the mesh has none.
    pub fn uvs(&self) -> &[Vec2] {
        &self.uvs
    }

    /// Puts UVs on the mesh, one for each triangle corner in the order of
    /// `indices` (an empty list is none), and gives back the ones they
    /// replace, which is all an undo step needs to keep. A list of any other
    /// length is refused: the mesh keeps what it had, and the error says why.
    pub fn set_uvs(&mut self, uvs: Vec<Vec2>) -> Result<Vec<Vec2>, String> {
        debug_assert!(!self.in_stroke);
        if !uvs.is_empty() && uvs.len() != self.indices.len() {
            return Err(format!(
                "{} UVs were given for a mesh of {} triangle corners: there has to be one for each corner",
                uvs.len(),
                self.indices.len()
            ));
        }
        Ok(std::mem::replace(&mut self.uvs, uvs))
    }

    /// Takes the mesh's UVs off and gives them back.
    pub fn clear_uvs(&mut self) -> Vec<Vec2> {
        std::mem::take(&mut self.uvs)
    }

    /// The faces an export writes of a mesh with UVs, each with the UVs of
    /// its corners; `None` for a mesh without. A triangle mesh's faces are
    /// its triangles. A mesh with quads (`polys`) has them written as quads,
    /// as every export does, wherever a quad's triangles agree on the UV of
    /// each of its corners. A quad whose triangles do not (a seam drawn down
    /// its diagonal) cannot be one face with one UV to a corner, so it is
    /// written as its triangles instead; every other quad stays a quad.
    pub fn faces_with_uvs(&self) -> Option<UvFaces<'_>> {
        if !self.has_uvs() {
            return None;
        }
        let Some(polys) = &self.polys else {
            return Some(UvFaces {
                faces: self.indices.chunks_exact(3).collect(),
                uvs: self.uvs.clone(),
                triangle: (0..self.triangle_count() as u32).collect(),
            });
        };
        // Each face's triangles, ascending: the triangle-to-face list turned round.
        let face_of = super::groups::faces_of_triangles(self)?;
        let count = polys.start.len().saturating_sub(1);
        let mut start = vec![0u32; count + 1];
        for &f in face_of.iter().filter(|&&f| (f as usize) < count) {
            start[f as usize + 1] += 1;
        }
        for f in 0..count {
            start[f + 1] += start[f];
        }
        let mut next = start.clone();
        let mut owned = vec![0u32; start[count] as usize];
        for (t, &f) in face_of.iter().enumerate().filter(|(_, f)| (**f as usize) < count) {
            owned[next[f as usize] as usize] = t as u32;
            next[f as usize] += 1;
        }

        let mut out = UvFaces {
            faces: Vec::with_capacity(count),
            uvs: Vec::with_capacity(polys.indices.len()),
            triangle: Vec::with_capacity(count),
        };
        let mut corner_uvs = Vec::new();
        for f in 0..count {
            let corners = &polys.indices[polys.start[f] as usize..polys.start[f + 1] as usize];
            let triangles = &owned[start[f] as usize..start[f + 1] as usize];
            corner_uvs.clear();
            let agreed = corners.iter().all(|&v| match self.agreed_uv(triangles, v) {
                Some(uv) => {
                    corner_uvs.push(uv);
                    true
                }
                None => false,
            });
            if agreed && !triangles.is_empty() {
                out.faces.push(corners);
                out.uvs.extend_from_slice(&corner_uvs);
                out.triangle.push(triangles[0]);
                continue;
            }
            for &t in triangles {
                let at = t as usize * 3;
                out.faces.push(&self.indices[at..at + 3]);
                out.uvs.extend_from_slice(&self.uvs[at..at + 3]);
                out.triangle.push(t);
            }
        }
        Some(out)
    }

    /// The UV vertex `v` has in every one of `triangles` that has it as a
    /// corner; `None` if two of them give it different ones, or none has it.
    fn agreed_uv(&self, triangles: &[u32], v: u32) -> Option<Vec2> {
        let mut found = None;
        for &t in triangles {
            for c in 0..3 {
                let at = t as usize * 3 + c;
                if self.indices[at] != v {
                    continue;
                }
                match found {
                    None => found = Some(self.uvs[at]),
                    Some(uv) if uv != self.uvs[at] => return None,
                    Some(_) => {}
                }
            }
        }
        found
    }
}

#[cfg(test)]
mod tests;
