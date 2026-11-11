//! Strokes under radial symmetry: a dab with its copies round the axis, and
//! the Move brush's grab under the same copies.
//!
//! Far from the axis, and from every mirror plane, the copies of a dab cannot
//! reach one another and are simply applied in turn. Where their footprints
//! meet, the order is not innocent: applied one after another against the
//! live surface, the second copy reads positions the first has just moved, so
//! the copies are handed different work, always in the same order, and the
//! difference builds up stroke after stroke into sectors that no longer match.
//!
//! Applying the copies in every order and averaging would put that right,
//! and radial symmetry cannot afford it. Near its axis every copy meets its
//! neighbours, so averaging over the orderings costs the square of the count:
//! the Unity app measured 36 applications a dab for 6 repeats, 52 ms a frame
//! for Clay at 330,000 triangles against 16 ms applied once each. Applying
//! one ordering instead, turned from dab to dab, was fast but left 1 to 8% of
//! a stroke's displacement between sectors, which Inflate then amplified
//! through the normals to 166% at the pole with 8 repeats.
//!
//! So every copy that can meet another is applied to the same starting
//! surface, and what the copies did to a vertex is shared out between them
//! (`share.rs`, which has the rule and the reasons for it, and is the same
//! for mirror images that meet). A sum does not care about order, so the
//! result is symmetric by construction, at the plain walk's cost; where
//! copies do not overlap it is exactly the plain walk. N copies landing on
//! one spot at the pole act as one dab, profile and all, rather than piling
//! up N-fold (Crease carving N times as deep there).
//!
//! Which copies can meet is a subgroup of the symmetry, and copies in
//! different cosets of it never do (`SymmetryGroup::interacting_subgroup`), so
//! each coset is added up on its own. How many dabs a coset's copies count as
//! is counted once, on the first, and used for all of them: the symmetries
//! are rigid, so it is the same number for each but for rounding, and one
//! number cannot favour a coset.
//!
//! Unlike a mirror image, a turn by a share of a circle is not exact in
//! floats for most counts, so the sectors agree to rounding rather than to
//! the bit. A stroke with no turns in its symmetry does not come through
//! here at all.

use glam::{DVec3, Vec3};
use rayon::prelude::*;
use sculpt_geom::SymmetryGroup;

use super::kernels::{self, Dab, Params, Scratch, SymOp};
use super::share::{self, Settling, Shared, Tally};
use crate::mesh::Mesh;

/// A copy of a dab closer to it than this many of the dab's own reach is
/// treated as able to see its output. 1 would be exact for dabs that only
/// read and write inside their reach; the rest covers Smooth reading one ring
/// of neighbours past its footprint, and a dab's own displacement carrying a
/// vertex over the boundary within a frame.
const INTERACTION_MARGIN: f32 = 1.5;


/// What a stroke with turns in its symmetry keeps between dabs: the symmetry
/// itself, and the buffers its dabs work in (a footprint on a dense mesh is
/// megabytes, and that costs more to get fresh for every dab than to fill).
pub struct Walk {
    group: SymmetryGroup,
    /// The copies that can meet at the dab in hand, and one copy from each
    /// set of them that cannot meet another set.
    subgroup: Vec<usize>,
    cosets: Vec<usize>,
    /// The copies of the set in hand: the ones that can meet, each applied
    /// to the surface as it stood before any of them.
    set: Vec<SymOp>,
    /// How high the dab in hand stands over the surface under it: what its
    /// copies are set down by (`Dab::copied`).
    height: Option<f32>,
    /// The leaves each copy of the set in hand reaches, one run per copy, and
    /// where each run ends.
    reached: Vec<u32>,
    ends: Vec<usize>,
    /// Every leaf the set reaches, ascending, and where each starts in the
    /// buffers below.
    leaves: Vec<u32>,
    starts: Vec<usize>,
    /// Where the vertices of `leaves` were before the dab, and what the
    /// copies have done to each between them.
    saved: Vec<Vec3>,
    moved: Vec<Shared>,
}

impl Walk {
    /// The walk for a stroke under `group`, or `None` when it has no turns
    /// in it: mirror images alone are walked by `Sculptor::apply_symmetric`,
    /// as they always were.
    pub fn new(group: SymmetryGroup) -> Option<Walk> {
        (group.radial_count() > 1).then(|| Walk {
            group,
            subgroup: Vec::new(),
            cosets: Vec::new(),
            set: Vec::new(),
            height: None,
            reached: Vec::new(),
            ends: Vec::new(),
            leaves: Vec::new(),
            starts: Vec::new(),
            saved: Vec::new(),
            moved: Vec::new(),
        })
    }

    /// Applies a dab and every copy of it. `reach` is the widest radius one
    /// copy reads or writes vertices within, which for Clay's swept tip is
    /// more than the brush's radius.
    /// more than the brush's radius; `height` is how high the dab stands
    /// over the surface under it (`kernels::height_over`).
    pub fn apply(
        &mut self,
        mesh: &mut Mesh,
        p: &Params,
        dab: &Dab,
        reach: f32,
        height: Option<f32>,
        scratch: &mut Scratch,
    ) {
        self.height = height;
        // Two copies can meet when they land within the margin of each
        // other. That is measured by the brush's radius, as is how much
        // they overlap: a sweep makes a copy look further for its
        // vertices, it does not put it any nearer its neighbours.
        let within = 2.0 * p.radius * INTERACTION_MARGIN;
        if !self.group.is_group() {
            return self.in_clusters(mesh, p, dab, reach, within, scratch);
        }
        self.group.interacting_subgroup(dab.center, within, &mut self.subgroup);
        if self.subgroup.len() == 1 {
            return self.in_turn(mesh, p, dab, reach, scratch);
        }
        self.group.coset_representatives(&self.subgroup, &mut self.cosets);
        let mut settling = None;
        for coset in 0..self.cosets.len() {
            // The coset's copies: its representative times each of the
            // copies that can meet. More than one copy can meet only in a
            // real group (see `interacting_subgroup`), where every product
            // is another copy.
            let (group, representative) = (&self.group, self.cosets[coset]);
            self.set.clear();
            let copy = |&h: &usize| SymOp::from(group[group.product(representative, h).unwrap_or(representative)]);
            self.set.extend(self.subgroup.iter().map(copy));
            if p.mask {
                // The copies that can meet lay one dab's worth of mask
                // between them (`kernels::mask_paint_together`), not one each.
                self.mask_together(mesh, p, dab, reach);
            } else {
                settling = self.together(mesh, p, dab, reach, settling, scratch);
            }
        }
    }

    /// The walk for a list of copies that is not a group: a radial axis
    /// tilted against a mirror plane, whose images of images never close.
    /// There is no subgroup to go by, so the copies that can meet are found
    /// by where they land: any two within `within` of each other are of one
    /// set, and so are two that a third joins. Each set is then shared out
    /// as a coset is. (They were once applied in turn, each on the surface
    /// the one before left: twice the strength down the centre line, as many
    /// times as there were copies at the axis, and a result that leaned
    /// toward whichever copy came last.) Every set works out its own
    /// sharing: with no symmetry between the sets, one's is not another's.
    fn in_clusters(&mut self, mesh: &mut Mesh, p: &Params, dab: &Dab, reach: f32, within: f32, scratch: &mut Scratch) {
        let ops: Vec<SymOp> = self.group.iter().map(|&op| SymOp::from(op)).collect();
        let centres: Vec<Vec3> = ops.iter().map(|op| op.apply_point(dab.center)).collect();
        // Which set each copy is of: the lowest copy it can reach, by way
        // of others or not.
        let mut of: Vec<usize> = (0..ops.len()).collect();
        let mut changed = true;
        while changed {
            changed = false;
            for a in 0..ops.len() {
                for b in a + 1..ops.len() {
                    if of[a] != of[b] && centres[a].distance_squared(centres[b]) < within * within {
                        let low = of[a].min(of[b]);
                        (of[a], of[b]) = (low, low);
                        changed = true;
                    }
                }
            }
        }
        for first in 0..ops.len() {
            if of[first] != first {
                continue;
            }
            self.set.clear();
            self.set.extend((first..ops.len()).filter(|&k| of[k] == first).map(|k| ops[k]));
            if p.mask {
                self.mask_together(mesh, p, dab, reach);
            } else {
                self.together(mesh, p, dab, reach, None, scratch);
            }
        }
    }

    /// Every copy once, in the symmetry's own order, each on the surface the
    /// one before left.
    fn in_turn(&mut self, mesh: &mut Mesh, p: &Params, dab: &Dab, reach: f32, scratch: &mut Scratch) {
        for &op in self.group.iter() {
            let copy = dab.copied(mesh, p, SymOp::from(op), self.height);
            self.leaves.clear();
            mesh.bvh.leaves_in_sphere(copy.center, reach, &mut self.leaves);
            // Ascending, which is what lets a dab's vertices be shared out
            // between threads.
            self.leaves.sort_unstable();
            if self.leaves.is_empty() {
                continue;
            }
            kernels::apply(mesh, p, &copy, &self.leaves, &self.leaves, scratch);
            if p.mask {
                mesh.mask_changed(&self.leaves);
            } else {
                mesh.positions_changed(&self.leaves);
            }
        }
    }

    /// Mask painting for the copies of the set in hand: one dab's worth
    /// between them.
    fn mask_together(&mut self, mesh: &mut Mesh, p: &Params, dab: &Dab, reach: f32) {
        let Walk { set, leaves, height, .. } = self;
        let copies: Vec<Dab> = set.iter().map(|&op| dab.copied(mesh, p, op, *height)).collect();
        leaves.clear();
        for copy in &copies {
            mesh.bvh.leaves_in_sphere(copy.center, reach, leaves);
        }
        leaves.sort_unstable();
        leaves.dedup();
        if leaves.is_empty() {
            return;
        }
        kernels::mask_paint_together(mesh, p, &copies, leaves);
        mesh.mask_changed(leaves);
    }

    /// The copies of the set in hand (a coset: its representative times
    /// each of the copies that can meet), each applied to the surface as it
    /// stood before any of them, and what they did shared out. `settling` is
    /// how, from the coset before (for a brush that adds it holds how many
    /// dabs the copies count as); the first coset works it out, and every
    /// coset gives it back.
    ///
    /// A copy works on its own leaves only, and only those are put back
    /// after it. Near the axis that is the same handful for every copy; a
    /// ring of 32 copies a few radii out is one set too (each meets its
    /// neighbour), and there, sweeping every copy over the whole ring would
    /// cost the square of the count.
    #[allow(clippy::too_many_arguments)]
    fn together(
        &mut self,
        mesh: &mut Mesh,
        p: &Params,
        dab: &Dab,
        reach: f32,
        settling: Option<Settling>,
        scratch: &mut Scratch,
    ) -> Option<Settling> {
        let Walk { group, set, reached, ends, leaves, starts, saved, moved, height, .. } = self;
        let copies: Vec<Dab> = set.iter().map(|&op| dab.copied(mesh, p, op, *height)).collect();

        reached.clear();
        ends.clear();
        for copy in &copies {
            let from = reached.len();
            mesh.bvh.leaves_in_sphere(copy.center, reach, reached);
            reached[from..].sort_unstable();
            ends.push(reached.len());
        }
        leaves.clear();
        leaves.extend_from_slice(reached);
        leaves.sort_unstable();
        leaves.dedup();
        if leaves.is_empty() {
            return settling;
        }

        let sharing = p.sharing();
        let total = kernels::layout(mesh, leaves, starts);
        super::save(mesh, leaves, starts, total, saved);
        moved.clear();
        moved.resize(total, Shared::default());
        let mut tally = Tally::default();
        let mut from = 0;
        for (copy, &end) in copies.iter().zip(ends.iter()) {
            let own = &reached[from..end];
            from = end;
            if own.is_empty() {
                continue;
            }
            kernels::apply(mesh, p, copy, own, own, scratch);
            tally.add(share::bank(mesh, own, leaves, starts, saved, moved, sharing));
        }
        // Moves rather than positions, so a vertex no copy touched is not
        // written at all and keeps its position to the bit.
        let (saved, moved) = (&*saved, &*moved);
        let settling = settling.unwrap_or_else(|| Settling::of(sharing, &tally));
        // A vertex the stroke found on the axis, or on a mirror plane among
        // the copies, stays on it (`share::held_on`).
        // Any copy but the dab as drawn whose plane or axis passes within
        // the reach of this set's own dab: a vertex under the dab that such
        // a copy leaves where it is lies no further from the dab's centre
        // than its image does, so the centre's image is within twice the
        // reach.
        let centre = set[0].apply_point(dab.center);
        let images: Vec<SymOp> = group
            .iter()
            .filter(|op| !op.is_identity() && op.apply_point(centre).distance(centre) <= 2.02 * reach)
            .map(|&op| SymOp::from(op))
            .collect();
        let on_plane = share::on_plane(p.radius);
        mesh.move_vertices(leaves, |at, vt| {
            let step = settling.step(&moved[at])?;
            Some(share::held_on(&images, vt.start, saved[at] + step, on_plane))
        });
        mesh.positions_changed(leaves);
        Some(settling)
    }
}

/// Which copies of a Move grab hold each grabbed vertex, and how firmly: for
/// vertex `u` of the grab, `copy[start[u]..start[u + 1]]` with a weight each.
///
/// Only the copies that reach a vertex are listed. A weight for every copy
/// on every vertex, which is how a mirrored grab is kept, would be 64 numbers
/// a vertex at 32 repeats with a mirror, nearly all of them zero.
pub struct Held {
    start: Vec<u32>,
    copy: Vec<u16>,
    weight: Vec<f32>,
}

/// Captures the vertices under every copy of the ring for a Move drag:
/// `copies` are each copy's centre and the camera as that copy sees it,
/// `weight` what a copy's pull on a vertex is. Returns the vertices, in
/// ascending order, and which copies hold each.
///
/// Each copy picks from its own footprint, and with `connected` keeps only
/// the piece of surface under its own centre (see `keep_connected`).
pub fn grab(
    mesh: &Mesh,
    copies: &[(Vec3, Vec3)],
    radius: f32,
    connected: bool,
    weight: impl Fn(u32, (Vec3, Vec3)) -> f32 + Sync + Send,
) -> (Vec<u32>, Held) {
    let mut holds: Vec<(u32, u16, f32)> = Vec::new();
    let mut leaves = Vec::new();
    let mut walk = super::Connected::default();
    for (k, &copy) in copies.iter().enumerate() {
        leaves.clear();
        mesh.bvh.leaves_in_sphere(copy.0, radius, &mut leaves);
        leaves.sort_unstable();
        let (mut verts, mut weights): (Vec<u32>, Vec<f32>) = kernels::collect(mesh, &leaves, |v| {
            let w = weight(v, copy);
            (w > 0.0).then_some((v, w))
        })
        .into_iter()
        .unzip();
        if connected && verts.len() > 1 {
            super::keep_connected(mesh, &[copy.0], &mut verts, &mut weights, &mut walk);
        }
        holds.extend(verts.iter().zip(&weights).map(|(&v, &w)| (v, k as u16, w)));
    }
    // By vertex, and for one vertex in the copies' own order.
    holds.par_sort_unstable_by_key(|&(v, k, _)| (v, k));

    let mut verts = Vec::new();
    let mut held = Held { start: Vec::new(), copy: Vec::with_capacity(holds.len()), weight: Vec::with_capacity(holds.len()) };
    for &(v, k, w) in &holds {
        if verts.last() != Some(&v) {
            verts.push(v);
            held.start.push(held.copy.len() as u32);
        }
        held.copy.push(k);
        held.weight.push(w);
    }
    held.start.push(held.copy.len() as u32);
    (verts, held)
}

impl Held {
    /// How far vertex `slot` of the grab has been dragged from where the
    /// stroke found it. `pulls` is how far the pointer has carried the
    /// grabbed point since the drag began, as each of the stroke's copies
    /// makes that motion.
    ///
    /// The pulls of the copies that hold the vertex, each times its weight,
    /// added up (in double, so a vertex and its twin get the same total
    /// whatever order their copies come in). What that comes to where copies
    /// overlap, near the axis or a mirror plane, is `share::Hold`'s to say:
    /// they are shared out by how many grabs the copies count as, the way
    /// overlapping dabs are. A vertex on the axis then follows the pointer's
    /// motion along the axis and stays on it.
    #[inline]
    pub fn pull(&self, slot: usize, pulls: &[Vec3]) -> DVec3 {
        let mut pull = DVec3::ZERO;
        for hold in self.start[slot] as usize..self.start[slot + 1] as usize {
            pull += (pulls[self.copy[hold] as usize] * self.weight[hold]).as_dvec3();
        }
        pull
    }

    /// How firmly the copy under the pointer, the first, holds vertex `slot`
    /// of the grab: 0 where only its images do.
    pub fn own(&self, slot: usize) -> f32 {
        let first = self.start[slot] as usize;
        // A vertex's copies are in their own order, so the first is first.
        if self.copy[first] == 0 { self.weight[first] } else { 0.0 }
    }

    /// Every grabbed vertex's weights, one for each copy that holds it.
    pub fn weights(&self) -> impl Iterator<Item = &[f32]> {
        self.start.windows(2).map(|ends| &self.weight[ends[0] as usize..ends[1] as usize])
    }
}
