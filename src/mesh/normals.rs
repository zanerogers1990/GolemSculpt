//! Bringing vertex normals up to date after vertices moved.
//!
//! A vertex's normal is the sum of the cross products of the triangles round
//! it, in double (see `Mesh::vertex_normal`). Each triangle is round three
//! vertices, and worked out vertex by vertex every cross product was taken
//! three times. Here a leaf's own triangles are crossed once into a small
//! buffer and each vertex adds up its fan from there, in the fan's own order:
//! the same sums, so the same normals to the bit. A fan triangle in another
//! leaf (a vertex at the leaf's edge) is crossed where it is needed.
//!
//! The leaves next to a leaf whose vertices moved used to have every normal
//! worked out again, though only the vertices along the shared edge can have
//! changed. Those leaves are now marked apart (`Mesh::pending_rims`), and only
//! their vertices with a neighbour in a leaf that moved are worked out. The
//! rest keep normals that were worked out from the same positions, so the
//! answer is the one a full pass would give.

use std::cell::RefCell;

use glam::{DVec3, Vec3};
use rayon::prelude::*;

use super::Mesh;
use crate::util::SharedMut;

thread_local! {
    /// The cross products of the triangles of the leaf a thread is on.
    static FACES: RefCell<Vec<DVec3>> = const { RefCell::new(Vec::new()) };
}

/// The cross product of triangle `t`, twice its area along its normal.
#[inline]
pub(super) fn face(positions: &[Vec3], indices: &[u32], t: u32) -> DVec3 {
    let i = t as usize * 3;
    let p0 = positions[indices[i] as usize].as_dvec3();
    let p1 = positions[indices[i + 1] as usize].as_dvec3();
    let p2 = positions[indices[i + 2] as usize].as_dvec3();
    (p1 - p0).cross(p2 - p0)
}

impl Mesh {
    /// Recomputes normals for everything that moved since the last call.
    pub fn flush_normals(&mut self) {
        if !self.pending_normals.any() && !self.pending_rims.any() {
            return;
        }
        let mut leaves = std::mem::take(&mut self.flush_leaves);
        leaves.clear();
        self.pending_normals.for_each(|l| leaves.push(l as u32));
        // Rim leaves come after the leaves that moved, each once.
        let moved = leaves.len();
        let full = &self.pending_normals;
        self.pending_rims.for_each(|l| {
            if !full.get(l) {
                leaves.push(l as u32);
            }
        });
        {
            let (positions, indices, vt, vv, bvh) = (&self.positions, &self.indices, &self.vt, &self.vv, &self.bvh);
            let out = SharedMut::new(&mut self.normals);
            let fan = |v: usize, faces: &[DVec3], tris: std::ops::Range<u32>| {
                Self::vertex_normal_by(vt.row(v), |t| {
                    if tris.contains(&t) { faces[(t - tris.start) as usize] } else { face(positions, indices, t) }
                })
            };
            leaves.par_iter().enumerate().with_min_len(32).for_each(|(k, &l)| {
                let leaf = &bvh.leaves[l as usize];
                let verts = leaf.vert_start as usize..leaf.vert_end as usize;
                if k < moved {
                    FACES.with_borrow_mut(|faces| {
                        faces.clear();
                        faces.extend((leaf.tri_start..leaf.tri_end).map(|t| face(positions, indices, t)));
                        for v in verts {
                            // SAFETY: every leaf owns a disjoint vertex range, so
                            // each normal is read and written by exactly one task.
                            unsafe {
                                let old = out.read(v);
                                out.write(v, fan(v, faces, leaf.tri_start..leaf.tri_end).unwrap_or(old));
                            }
                        }
                    });
                } else {
                    for v in verts {
                        // Only a vertex beside one that may have moved.
                        if !vv.row(v).iter().any(|&o| full.get(bvh.vert_leaf[o as usize] as usize)) {
                            continue;
                        }
                        // SAFETY: as above.
                        unsafe {
                            let old = out.read(v);
                            out.write(v, fan(v, &[], 0..0).unwrap_or(old));
                        }
                    }
                }
            });
        }
        self.pending_normals.clear();
        self.pending_rims.clear();
        for &l in &leaves {
            self.dirty_nrm.set(l as usize);
        }
        if self.curv_on {
            self.update_curvature(&leaves);
        }
        self.flush_leaves = leaves;
    }

    /// The unit sum of the cross products `face` gives for the triangles
    /// `tris`, added in their order, or `None` where they come to nothing.
    #[inline]
    fn vertex_normal_by(tris: &[u32], face: impl Fn(u32) -> DVec3) -> Option<Vec3> {
        let mut sum = DVec3::ZERO;
        for &t in tris {
            sum += face(t);
        }
        let len = sum.length();
        (len > 1e-30).then(|| (sum / len).as_vec3())
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec3;

    use crate::mesh::primitives::{self, Primitive};

    /// After vertices in some leaves move, the flush leaves every normal
    /// exactly as working them all out afresh does: the leaves that moved,
    /// the vertices along their edges in the leaves next to them, and none
    /// left stale.
    #[test]
    fn a_flush_gives_the_normals_a_full_pass_gives_to_the_bit() {
        let mut mesh = primitives::build(Primitive::Sphere, 30_000, 0.5);
        for round in 0..3u32 {
            // Whole leaves of vertices moved, and as `positions_changed` asks,
            // every leaf with a triangle on one of them passed with them.
            let mut moved = Vec::new();
            for l in (round..mesh.bvh.leaves.len() as u32).step_by(5) {
                let leaf = mesh.bvh.leaves[l as usize];
                for v in leaf.vert_start..leaf.vert_end {
                    let p = mesh.positions[v as usize];
                    mesh.positions[v as usize] = p * (1.0 + 0.05 * ((v as f32 * 0.37 + round as f32).sin()));
                    moved.push(v);
                }
            }
            let leaves = mesh.leaves_of(&moved);
            mesh.positions_changed(&leaves);
            mesh.flush_normals();
            let flushed = mesh.normals.clone();
            mesh.recompute_all_normals();
            let bits = |n: &[Vec3]| n.iter().map(|n| n.to_array().map(f32::to_bits)).collect::<Vec<_>>();
            assert!(bits(&flushed) == bits(&mesh.normals), "round {round}: a normal differs from a full pass");
        }
    }
}
