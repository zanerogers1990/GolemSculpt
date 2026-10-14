//! The relaxation passes Smooth and Clay's finishing pass make, run over a
//! list of the vertices that have weight rather than every vertex of the
//! dab's leaves.
//!
//! Each pass is Jacobi: every vertex's new place is worked out from where the
//! pass found them all, and only then is any moved (updating in place depends
//! on the order and breaks mirror symmetry). The passes used to read the mesh,
//! write a target for every vertex of the leaves, weight or none, and then put
//! the targets back through `Mesh::move_vertices`, which also reads each
//! vertex's mask, normal and stroke record, sixteen times over for Clay. Here
//! the vertices with weight are listed once, the passes move copies of their
//! positions (a vertex's neighbours are read from the copy where it has one),
//! and the mesh is written once, at the end.
//!
//! What a vertex comes to is the same, to the bit: the same sums in the same
//! order, and a vertex moves in a pass exactly when it would have been written
//! then (further than the pass's least move, to a finite place, and not where
//! it already is).

use glam::{DVec3, Vec3};
use rayon::prelude::*;

use super::kernels::PAR_MIN;
use crate::mesh::Mesh;
use crate::mesh::bvh::Leaf;
use crate::util::SharedMut;

/// No place in the buffer: the leaf is not among those being relaxed.
const NONE: u32 = u32::MAX;

/// What the passes keep between dabs. A footprint on a dense mesh is
/// megabytes, and that costs more to get fresh for every dab than to fill.
#[derive(Default)]
pub(super) struct Relaxing {
    /// The vertices that have weight, leaf after leaf, ascending, and the
    /// place each has in the leaves' buffer (`kernels::layout`).
    verts: Vec<u32>,
    ats: Vec<u32>,
    /// Where each leaf's run of `verts` starts, and one past the last.
    runs: Vec<usize>,
    /// The leaves' vertices where the pass in hand finds them, in the
    /// leaves' buffer order, and where the listed ones go in it.
    now: Vec<Vec3>,
    next: Vec<Vec3>,
    /// For every leaf of the mesh, where its vertices start in `now`, or
    /// `NONE` for a leaf that is not being relaxed.
    leaf_at: Vec<u32>,
    /// Keep Volume's two totals for each leaf.
    sums: Vec<(f64, f64)>,
}

/// Where a pass finds a vertex: in the pass's own copy if the vertex is in
/// one of the leaves being relaxed, otherwise on the mesh, where it stays.
pub(super) struct Live<'a> {
    now: &'a [Vec3],
    positions: &'a [Vec3],
    leaves: &'a [Leaf],
    vert_leaf: &'a [u32],
    leaf_at: &'a [u32],
    /// The leaf whose vertices are being worked out, which nearly every
    /// neighbour is in: its vertex range and where it starts in `now`.
    lo: u32,
    hi: u32,
    base: usize,
}

impl Live<'_> {
    #[inline]
    pub(super) fn pos(&self, v: u32) -> Vec3 {
        if v >= self.lo && v < self.hi {
            return self.now[self.base + (v - self.lo) as usize];
        }
        let leaf = self.vert_leaf[v as usize];
        match self.leaf_at[leaf as usize] {
            NONE => self.positions[v as usize],
            at => self.now[at as usize + (v - self.leaves[leaf as usize].vert_start) as usize],
        }
    }

    /// Mean of a vertex's edge neighbours, summed in double (see the
    /// module docs of `kernels`), or `None` for a vertex with none.
    #[inline]
    fn neighbour_mean(&self, mesh: &Mesh, v: u32) -> Option<Vec3> {
        let ring = mesh.neighbours(v);
        if ring.is_empty() {
            return None;
        }
        let mut sum = DVec3::ZERO;
        for &o in ring {
            sum += self.pos(o).as_dvec3();
        }
        Some((sum / ring.len() as f64).as_vec3())
    }
}

/// Lists the vertices of `leaves` that `weights` gives a weight above zero,
/// leaf after leaf: `verts` with their places `ats`, and `runs[k]..runs[k +
/// 1]` the run of leaf `leaves[k]`. `weights` holds one entry per vertex of
/// the leaves, leaf after leaf from `starts[k]` (`kernels::layout`), and a
/// hidden vertex's is zero.
pub(super) fn list_weighted(
    mesh: &Mesh,
    leaves: &[u32],
    starts: &[usize],
    weights: &[f32],
    verts: &mut Vec<u32>,
    ats: &mut Vec<u32>,
    runs: &mut Vec<usize>,
) {
    let span = |k: usize| starts[k]..starts.get(k + 1).copied().unwrap_or(weights.len());
    runs.clear();
    runs.resize(leaves.len() + 1, 0);
    let count = |(k, run): (usize, &mut usize)| *run = weights[span(k)].iter().filter(|&&w| w > 0.0).count();
    if weights.len() < PAR_MIN {
        runs[1..].iter_mut().enumerate().for_each(count);
    } else {
        runs[1..].par_iter_mut().enumerate().with_min_len(64).for_each(count);
    }
    for k in 0..leaves.len() {
        runs[k + 1] += runs[k];
    }
    let total = runs[leaves.len()];
    verts.clear();
    verts.resize(total, 0);
    ats.clear();
    ats.resize(total, 0);
    let (out_v, out_at, runs) = (SharedMut::new(verts), SharedMut::new(ats), &*runs);
    let fill = |k: usize| {
        let first = mesh.bvh.leaves[leaves[k] as usize].vert_start;
        let mut i = runs[k];
        for (n, &w) in weights[span(k)].iter().enumerate() {
            if w > 0.0 {
                // SAFETY: leaf k writes only its own run of the lists.
                unsafe {
                    out_v.write(i, first + n as u32);
                    out_at.write(i, (starts[k] + n) as u32);
                }
                i += 1;
            }
        }
    };
    if weights.len() < PAR_MIN {
        (0..leaves.len()).for_each(fill);
    } else {
        (0..leaves.len()).into_par_iter().with_min_len(64).for_each(fill);
    }
}

/// Runs `k` over every leaf of a dab, on all threads once the work is worth
/// sharing out.
fn each_leaf(count: usize, work: usize, k: impl Fn(usize) + Sync + Send) {
    if work < PAR_MIN {
        (0..count).for_each(k);
    } else {
        (0..count).into_par_iter().with_min_len(16).for_each(k);
    }
}

/// How a relaxation goes: how many passes and how far, and what a vertex
/// does in one ([`Relaxing::run`]).
pub(super) struct Passes {
    /// Whole passes and the fraction of one more, as `Smooth` counts them.
    pub amount: f32,
    /// A vertex moves in a pass only if it would go further than this
    /// (squared).
    pub min_move2: f32,
    /// Keep Volume: how much of what the passes took from the form along
    /// the normals is given back, 0 for none (see [`Relaxing::given_back`]).
    pub keep: f32,
}

impl Relaxing {
    /// Relaxes the vertices of `leaves` that `weights` gives weight, as the
    /// passes say. `step` is given the mesh, a vertex, where it is, the mean
    /// of its neighbours, its weight for the pass (0 to 1) and where the pass
    /// finds every other vertex, and says where the vertex goes.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn run(
        &mut self,
        mesh: &mut Mesh,
        leaves: &[u32],
        starts: &[usize],
        total: usize,
        weights: &[f32],
        passes: &Passes,
        step: impl Fn(&Mesh, u32, Vec3, Vec3, f32, &Live) -> Vec3 + Sync,
    ) {
        let full = passes.amount.floor() as usize;
        let frac = passes.amount - full as f32;
        let count = full + usize::from(frac > 0.001);
        if count == 0 {
            return;
        }
        debug_assert_eq!(weights.len(), total);
        list_weighted(mesh, leaves, starts, weights, &mut self.verts, &mut self.ats, &mut self.runs);
        if self.verts.is_empty() {
            return;
        }
        self.copy_in(mesh, leaves, starts, total);
        let min_move2 = passes.min_move2;
        for pass in 0..count {
            let factor = if pass < full { 1.0 } else { frac };
            self.pass(mesh, leaves, weights, factor, min_move2, &step);
        }
        let back = if passes.keep > 0.0 { self.given_back(mesh, leaves, weights, passes.keep) } else { None };
        let (now, ats) = (&self.now, &self.ats);
        mesh.move_listed(&self.verts, |i, vt| {
            let at = ats[i] as usize;
            Some(match back {
                Some(back) => now[at] + vt.normal * (weights[at] * back),
                None => now[at],
            })
        });
        for &leaf in leaves {
            self.leaf_at[leaf as usize] = NONE;
        }
    }

    /// Copies where the leaves' vertices are into `now`, and notes where each
    /// leaf starts in it.
    fn copy_in(&mut self, mesh: &Mesh, leaves: &[u32], starts: &[usize], total: usize) {
        self.leaf_at.resize(mesh.bvh.leaves.len(), NONE);
        for (k, &leaf) in leaves.iter().enumerate() {
            self.leaf_at[leaf as usize] = starts[k] as u32;
        }
        self.now.resize(total, Vec3::ZERO);
        self.next.resize(self.verts.len(), Vec3::ZERO);
        let now = SharedMut::new(&mut self.now);
        each_leaf(leaves.len(), total, |k| {
            let leaf = &mesh.bvh.leaves[leaves[k] as usize];
            for (n, v) in (leaf.vert_start..leaf.vert_end).enumerate() {
                // SAFETY: each leaf has its own stretch of the buffer.
                unsafe { now.write(starts[k] + n, mesh.positions[v as usize]) };
            }
        });
    }

    /// One Jacobi pass: where every listed vertex goes, from where the pass
    /// found them all, and then all of them moved there in `now`.
    fn pass(
        &mut self,
        mesh: &Mesh,
        leaves: &[u32],
        weights: &[f32],
        factor: f32,
        min_move2: f32,
        step: &(impl Fn(&Mesh, u32, Vec3, Vec3, f32, &Live) -> Vec3 + Sync),
    ) {
        let (verts, ats, runs, leaf_at) = (&self.verts, &self.ats, &self.runs, &self.leaf_at);
        let work = verts.len();
        {
            let now = &self.now;
            let next = SharedMut::new(&mut self.next);
            each_leaf(leaves.len(), work, |k| {
                let leaf = &mesh.bvh.leaves[leaves[k] as usize];
                let live = Live {
                    now,
                    positions: &mesh.positions,
                    leaves: &mesh.bvh.leaves,
                    vert_leaf: &mesh.bvh.vert_leaf,
                    leaf_at,
                    lo: leaf.vert_start,
                    hi: leaf.vert_end,
                    base: leaf_at[leaves[k] as usize] as usize,
                };
                for i in runs[k]..runs[k + 1] {
                    let (v, at) = (verts[i], ats[i] as usize);
                    let pos = now[at];
                    let to = match live.neighbour_mean(mesh, v) {
                        Some(mean) => step(mesh, v, pos, mean, (weights[at] * factor).clamp(0.0, 1.0), &live),
                        None => pos,
                    };
                    // Moved as `Mesh::move_vertices` would have moved it.
                    let moved = to.distance_squared(pos) > min_move2 && to.is_finite() && to != pos;
                    // SAFETY: leaf k writes only its own run of the list.
                    unsafe { next.write(i, if moved { to } else { pos }) };
                }
            });
        }
        let next = &self.next;
        let now = SharedMut::new(&mut self.now);
        each_leaf(leaves.len(), work, |k| {
            for i in runs[k]..runs[k + 1] {
                // SAFETY: every listed vertex has its own place in the buffer.
                unsafe { now.write(ats[i] as usize, next[i]) };
            }
        });
    }

    /// Keep Volume: gives back `keep` of what a Smooth dab took from the form
    /// under it. Says how far along its normal each vertex the passes moved
    /// goes back out per unit of its weight, or `None` where nothing is to be
    /// given back; it is added as the vertices are written.
    ///
    /// Smoothing a curved surface draws it in: every vertex moves toward the
    /// middle of its neighbours, which on a ball, a finger or a fin's edge is
    /// inward, so each stroke leaves the form thinner. What the dab did to the
    /// form's bulk is how far it moved its vertices along their normals, added
    /// up; that much is put back, spread over the same vertices by the brush's
    /// own weights. The bumps a dab flattened stay flattened (pushing every
    /// vertex out a little does not bring one back), and on a flat surface,
    /// where smoothing moves as much up as down, nothing is put back at all.
    ///
    /// One number for the whole dab, so it cannot ring or roughen the surface
    /// the way undoing the shrinkage vertex by vertex does (Taubin's inflating
    /// pass sharpens what the smoothing pass has just blunted, and on a form
    /// only a few vertices round it reads the whole form as a detail and lets
    /// it shrink regardless).
    ///
    /// Totalled leaf by leaf and the leaves added up in order, as it was when
    /// it went over every vertex of the leaves, so it comes to the same
    /// number to the last bit, whatever threads did the adding.
    fn given_back(&mut self, mesh: &Mesh, leaves: &[u32], weights: &[f32], keep: f32) -> Option<f32> {
        let (verts, ats, runs, now) = (&self.verts, &self.ats, &self.runs, &self.now);
        self.sums.clear();
        self.sums.resize(leaves.len(), (0.0, 0.0));
        let of_leaf = |(k, sum): (usize, &mut (f64, f64))| {
            let (mut moved, mut weight) = (0.0f64, 0.0f64);
            for i in runs[k]..runs[k + 1] {
                let (v, at) = (verts[i] as usize, ats[i] as usize);
                moved += (now[at] - mesh.positions[v]).dot(mesh.normals[v]) as f64;
                weight += weights[at] as f64;
            }
            *sum = (moved, weight);
        };
        if verts.len() < PAR_MIN {
            self.sums.iter_mut().enumerate().for_each(of_leaf);
        } else {
            self.sums.par_iter_mut().enumerate().with_min_len(8).for_each(of_leaf);
        }
        let (moved, weight) = self.sums.iter().fold((0.0, 0.0), |(moved, weight), &(m, w)| (moved + m, weight + w));
        if weight <= 1e-12 || moved == 0.0 {
            return None;
        }
        Some((-(keep as f64) * moved / weight) as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::primitives::{self, Primitive};

    /// The list holds every vertex with weight, in order, each with its own
    /// place in the leaves' buffer, in runs that follow the leaves; a leaf
    /// with nothing weighted has an empty run.
    #[test]
    fn the_list_holds_the_weighted_vertices_leaf_by_leaf() {
        let mesh = primitives::build(Primitive::Sphere, 20_000, 0.5);
        for (pick, small) in [(3usize, true), (2, false)] {
            let leaves: Vec<u32> = (0..mesh.bvh.leaves.len() as u32).step_by(if small { 7 } else { 1 }).collect();
            let (mut starts, mut total) = (Vec::new(), 0);
            for &l in &leaves {
                starts.push(total);
                let leaf = &mesh.bvh.leaves[l as usize];
                total += (leaf.vert_end - leaf.vert_start) as usize;
            }
            // Every `pick`th place weighted, and the whole of leaf 1 left out.
            let mut weights: Vec<f32> = (0..total).map(|at| if at % pick == 0 { 0.5 } else { 0.0 }).collect();
            weights[starts[1]..starts[2]].fill(0.0);
            weights[0] = f32::NAN;
            let (mut verts, mut ats, mut runs) = (Vec::new(), Vec::new(), Vec::new());
            list_weighted(&mesh, &leaves, &starts, &weights, &mut verts, &mut ats, &mut runs);
            let mut expected = Vec::new();
            for (k, &l) in leaves.iter().enumerate() {
                let leaf = &mesh.bvh.leaves[l as usize];
                for (n, v) in (leaf.vert_start..leaf.vert_end).enumerate() {
                    if weights[starts[k] + n] > 0.0 {
                        expected.push((v, (starts[k] + n) as u32, k));
                    }
                }
            }
            assert_eq!(verts, expected.iter().map(|e| e.0).collect::<Vec<_>>());
            assert_eq!(ats, expected.iter().map(|e| e.1).collect::<Vec<_>>());
            assert_eq!((runs.len(), runs[0], runs[leaves.len()]), (leaves.len() + 1, 0, verts.len()));
            assert_eq!(runs[1], runs[2], "leaf 1 has nothing weighted");
            for k in 0..leaves.len() {
                assert!(expected[runs[k]..runs[k + 1]].iter().all(|e| e.2 == k), "run {k}");
            }
            assert!(verts.windows(2).all(|w| w[0] < w[1]), "ascending");
        }
    }
}
