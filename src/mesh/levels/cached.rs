//! The finest level's mesh, kept for the way back up.
//!
//! Most of a step up to a finest level of ten million triangles was building
//! the sculpt mesh: the spatial index alone took 0.9 s and the adjacency
//! another 0.25 s, of 1.2 s in all. Yet the finest level's faces cannot
//! change while the app is below it: dynamic topology does nothing to a mesh
//! with levels, and everything that rebuilds a mesh leaves the levels behind
//! with the old one. Only where its vertices are changes, when an edit made
//! lower down is carried up. So the step down from the finest level keeps
//! what the build made of its faces (the triangles in the index's order,
//! each vertex's triangles and neighbours, the index itself, and how the
//! level's numbering maps onto the mesh's), and the step back up puts the
//! level's positions and mask into that order, fits the index's boxes round
//! them, and works out the normals.
//!
//! **The index can only be as good as the shape it was built round.** Fitted
//! again round a surface that has moved, its boxes still group the triangles
//! they grouped before, and those may now lie far apart (a limb bent right
//! round), which slows every dab. Strokes on the finest level have always
//! left the index like that; a step down and up used to rebuild it. So the
//! refitted index is measured ([`spread`]: the area of all its boxes for the
//! area of the whole) against what it measured when it was last built, and a
//! step up whose index has grown looser than [`LOOSEST`] times that builds
//! the mesh afresh, as it always did.
//!
//! **What comes out is the same.** Positions, mask, hidden faces, groups and
//! layers are put in the same places by the same numbering lists. A normal is
//! summed over the vertex's triangles in double precision, in the order of
//! the mesh's own triangles, and rounded once; another order changes the
//! sum's last bits and almost never the rounded normal (the tests count how
//! many differ from a fresh build's: none, on the shapes they try).
//!
//! **What it costs** is memory: about 60 bytes a triangle while the mesh is
//! below its finest level, shared by every mesh kept for undo since the step
//! down, which are charged their share of it (`MeshLevels::bytes`).

use glam::Vec3;
use rayon::prelude::*;

use super::{MeshLevels, Numbering, PER_TASK};
use crate::mesh::Mesh;
use crate::mesh::bvh::Bvh;
use crate::mesh::lists::Lists;
use crate::util::SharedMut;

/// How much looser than when it was built the refitted index may be before
/// a step up builds the mesh afresh. A ball bent over as far as the levels
/// demo bends it measures 1.07 (`cached/tests.rs`). 1.5 is a judgment, not
/// a measurement: wide enough that changes of form leave the kept build in
/// use, narrow enough that a model pulled far out of its old shape gets the
/// fresh index it always used to get.
pub const LOOSEST: f64 = 1.5;

/// What a build made of the finest level's faces: see the module notes.
pub(super) struct TopMesh {
    indices: Vec<u32>,
    vt: Lists,
    vv: Lists,
    bvh: Bvh,
    /// For each vertex of the level, in the stack's numbering, the mesh's vertex.
    vertex: Vec<u32>,
    /// For each triangle of the mesh, the stack's triangle.
    triangle: Vec<u32>,
    /// For each vertex of the mesh, the stack's: `vertex` the other way
    /// round, so that the way up can gather (writing each mesh vertex in
    /// turn) rather than scatter, which is several times quicker.
    stack_of: Vec<u32>,
    /// [`spread`] of the index when it was last built afresh.
    built_spread: f64,
}

/// The area of every box of `bvh`'s tree over the area of its root: how many
/// times over, near enough, a ray through the model is tested against a box.
/// About the depth of the tree for a tight one, more where boxes overlap.
pub fn spread(bvh: &Bvh) -> f64 {
    let area = |node: &crate::mesh::bvh::Node| {
        let size = node.bounds.size().as_dvec3().max(glam::DVec3::ZERO);
        2.0 * (size.x * size.y + size.y * size.z + size.z * size.x)
    };
    let Some(root) = bvh.nodes.first().map(area).filter(|&root| root > 0.0) else { return 1.0 };
    bvh.nodes.par_iter().with_min_len(PER_TASK).map(area).sum::<f64>() / root
}

impl TopMesh {
    /// What `mesh`, the finest level of `levels`, was built into.
    pub(super) fn of(mesh: &Mesh, levels: &MeshLevels) -> TopMesh {
        // Copies the size of the mesh's faces: side by side, as there is
        // more memory bandwidth than one thread uses.
        let ((indices, bvh), (vt, vv)) = rayon::join(
            || rayon::join(|| mesh.indices.clone(), || mesh.bvh.clone()),
            || rayon::join(|| mesh.vt.clone(), || mesh.vv.clone()),
        );
        let (vertex, triangle) = rayon::join(|| levels.vertex.clone(), || levels.triangle.clone());
        let mut stack_of = vec![0u32; vertex.len()];
        {
            let out = SharedMut::new(&mut stack_of);
            vertex.par_iter().enumerate().with_min_len(PER_TASK).for_each(|(at, &v)| {
                // SAFETY: the numbering is one to one, so each mesh vertex is written once.
                unsafe { out.write(v as usize, at as u32) };
            });
        }
        let built_spread = if levels.spread > 0.0 { levels.spread } else { spread(&mesh.bvh) };
        TopMesh { indices, vt, vv, bvh, vertex, triangle, stack_of, built_spread }
    }

    /// What is kept, in bytes.
    pub(super) fn bytes(&self) -> usize {
        (self.indices.len() + self.vertex.len() + self.triangle.len() + self.stack_of.len()) * 4
            + self.vt.bytes()
            + self.vv.bytes()
            + self.bvh.bytes()
    }

    /// The finest level's mesh with `positions`, `mask` and `hidden` (one
    /// flag a triangle, or none), all in the stack's order, with how it was
    /// numbered and the [`spread`] its index was built with. `None` when
    /// the index refitted round these positions is too loose to keep.
    pub(super) fn mesh(&self, positions: &[Vec3], mask: &[f32], hidden: &[bool]) -> Option<(Mesh, Numbering, f64)> {
        let count = self.vertex.len();
        if positions.len() != count || mask.len() != count {
            return None;
        }
        let gather = || self.stack_of.par_iter().with_min_len(PER_TASK);
        let (placed, masked) = rayon::join(
            || gather().map(|&at| positions[at as usize]).collect::<Vec<_>>(),
            || gather().map(|&at| mask[at as usize]).collect::<Vec<_>>(),
        );
        let ((indices, bvh), (vt, vv)) = rayon::join(
            || rayon::join(|| self.indices.clone(), || self.bvh.clone()),
            || rayon::join(|| self.vt.clone(), || self.vv.clone()),
        );
        let mut mesh = Mesh::assembled(placed, masked, indices, bvh);
        let leaves: Vec<u32> = (0..mesh.bvh.leaves.len() as u32).collect();
        mesh.bvh.refit_leaves(&leaves, &mesh.positions, &mesh.indices, false);
        if spread(&mesh.bvh) > self.built_spread * LOOSEST {
            return None;
        }
        (mesh.vt, mesh.vv) = (vt, vv);
        mesh.recompute_all_normals();
        if hidden.len() == self.triangle.len() && hidden.contains(&true) {
            let flags: Vec<bool> = self.triangle.par_iter().with_min_len(PER_TASK).map(|&t| hidden[t as usize]).collect();
            mesh.restore_hidden_triangles(&flags);
        }
        let numbering = Numbering { vertex: self.vertex.clone(), triangle: self.triangle.clone() };
        Some((mesh, numbering, self.built_spread))
    }
}

#[cfg(test)]
mod tests;
