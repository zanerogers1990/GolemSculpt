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
//! A mirrored pair is put right by applying it in both orders and averaging
//! (`Sculptor::apply_symmetric`). Radial symmetry cannot afford that. Near its
//! axis every copy meets its neighbours, so averaging over the orderings costs
//! the square of the count: the Unity app measured 36 applications a dab for 6
//! repeats, 52 ms a frame for Clay at 330,000 triangles against 16 ms applied
//! once each. Applying one ordering instead, turned from dab to dab, was fast
//! but left 1 to 8% of a stroke's displacement between sectors, which Inflate
//! then amplified through the normals to 166% at the pole with 8 repeats.
//!
//! So every copy that can meet another is applied to the same starting
//! surface and their displacements are added. A sum does not care about
//! order, so the result is symmetric by construction, at the plain walk's
//! cost; where copies do not overlap it is exactly the plain walk. Where they
//! do, a sum would stack them (Crease carving N times as deep at the pole), so
//! it is divided by how many copies sit on top of each other (see
//! [`multiplicity`]): N copies landing on one spot at the pole act as one
//! dab, profile and all, rather than piling up N-fold. That is one number
//! for the whole set of copies, so it cannot favour one of them. (Dividing
//! each vertex by the number of copies covering it was tried first in the
//! Unity app. It turned N coincident dabs into a flat-topped mesa with a
//! cliff where the cover fell below one, and Inflate amplified float noise on
//! that cliff to 4% of the stroke.)
//!
//! A brush that holds a height needs one thing more. Layer raises the surface
//! to a set height in a single dab and never past it. A dab a little off the
//! axis has every copy covering the middle while the set counts as fewer
//! (they only partly overlap), so the shared-out sum carried the middle half
//! as high again as the brush allows, and Layer never takes anything back
//! down. (The Unity app's Layer does exactly that within a radius of the
//! axis.) For such a brush the shared-out move is also held to the furthest
//! any one copy moved the vertex: what the copies would have left applied one
//! after another, each only topping up the last, but with no order to it.
//!
//! Which copies can meet is a subgroup of the symmetry, and copies in
//! different cosets of it never do (`SymmetryGroup::interacting_subgroup`), so
//! each coset is added up on its own.
//!
//! Unlike a mirror image, a turn by a share of a circle is not exact in
//! floats for most counts, so the sectors agree to rounding rather than to
//! the bit. A stroke with no turns in its symmetry does not come through
//! here at all.

use glam::{DVec3, Vec3};
use rayon::prelude::*;
use sculpt_geom::SymmetryGroup;

use super::kernels::{self, Dab, Params, Scratch, SymOp};
use crate::mesh::Mesh;

/// A copy of a dab closer to it than this many of the dab's own reach is
/// treated as able to see its output. 1 would be exact for dabs that only
/// read and write inside their reach; the rest covers Smooth reading one ring
/// of neighbours past its footprint, and a dab's own displacement carrying a
/// vertex over the boundary within a frame.
const INTERACTION_MARGIN: f32 = 1.5;

/// What the copies of one set have done to a vertex between them.
#[derive(Clone, Copy, Default)]
struct Moved {
    /// Their moves, added up.
    sum: DVec3,
    /// The square of the furthest any one of them moved it.
    furthest: f32,
}

/// What a stroke with turns in its symmetry keeps between dabs: the symmetry
/// itself, and the buffers its dabs work in (a footprint on a dense mesh is
/// megabytes, and that costs more to get fresh for every dab than to fill).
pub struct Walk {
    group: SymmetryGroup,
    /// The copies that can meet at the dab in hand, and one copy from each
    /// set of them that cannot meet another set.
    subgroup: Vec<usize>,
    cosets: Vec<usize>,
    /// The leaves each copy of the set in hand reaches, one run per copy, and
    /// where each run ends.
    reached: Vec<u32>,
    ends: Vec<usize>,
    /// Every leaf the set reaches, ascending.
    leaves: Vec<u32>,
    /// Where the vertices of `leaves` were before the dab, and what the
    /// copies have done to each between them.
    saved: Vec<Vec3>,
    moved: Vec<Moved>,
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
            reached: Vec::new(),
            ends: Vec::new(),
            leaves: Vec::new(),
            saved: Vec::new(),
            moved: Vec::new(),
        })
    }

    /// Applies a dab and every copy of it. `reach` is the widest radius one
    /// copy reads or writes vertices within, which for Clay's swept tip is
    /// more than the brush's radius.
    pub fn apply(&mut self, mesh: &mut Mesh, p: &Params, dab: &Dab, reach: f32, scratch: &mut Scratch) {
        // Mask amounts all have the same sign and saturate, so order does not
        // matter there.
        if !p.mask {
            // Two copies can meet when they land within the margin of each
            // other. That is measured by the brush's radius, as is how much
            // they overlap: a sweep makes a copy look further for its
            // vertices, it does not put it any nearer its neighbours.
            let within = 2.0 * p.radius * INTERACTION_MARGIN;
            self.group.interacting_subgroup(dab.center, within, &mut self.subgroup);
        }
        if p.mask || self.subgroup.len() == 1 {
            return self.in_turn(mesh, p, dab, reach, scratch);
        }
        self.group.coset_representatives(&self.subgroup, &mut self.cosets);
        let scale = 1.0 / multiplicity(&self.group, &self.subgroup, dab.center, p.radius);
        for coset in 0..self.cosets.len() {
            self.together(mesh, p, dab, reach, self.cosets[coset], scale, scratch);
        }
    }

    /// Every copy once, in the symmetry's own order, each on the surface the
    /// one before left.
    fn in_turn(&mut self, mesh: &mut Mesh, p: &Params, dab: &Dab, reach: f32, scratch: &mut Scratch) {
        for &op in self.group.iter() {
            let copy = dab.mirrored(SymOp::from(op));
            self.leaves.clear();
            mesh.bvh.leaves_in_sphere(copy.center, reach, &mut self.leaves);
            // Ascending, which is what lets a dab's vertices be shared out
            // between threads.
            self.leaves.sort_unstable();
            if self.leaves.is_empty() {
                continue;
            }
            kernels::apply(mesh, p, &copy, &self.leaves, scratch);
            if p.mask {
                mesh.mask_changed(&self.leaves);
            } else {
                mesh.positions_changed(&self.leaves);
            }
        }
    }

    /// The copies of one coset (`representative` times each of the copies
    /// that can meet), each applied to the surface as it stood before any of
    /// them, and the sum of what they did shared out by `scale`.
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
        representative: usize,
        scale: f32,
        scratch: &mut Scratch,
    ) {
        let Walk { group, subgroup, reached, ends, leaves, saved, moved, .. } = self;
        // More than one copy can meet only in a real group (see
        // `interacting_subgroup`), where every product is another copy.
        let copy = |h: usize| SymOp::from(group[group.product(representative, h).unwrap_or(representative)]);

        reached.clear();
        ends.clear();
        for &h in subgroup.iter() {
            let from = reached.len();
            mesh.bvh.leaves_in_sphere(copy(h).apply_point(dab.center), reach, reached);
            reached[from..].sort_unstable();
            ends.push(reached.len());
        }
        leaves.clear();
        leaves.extend_from_slice(reached);
        leaves.sort_unstable();
        leaves.dedup();
        if leaves.is_empty() {
            return;
        }

        let (starts, total) = kernels::layout(mesh, leaves);
        super::save(mesh, leaves, saved);
        moved.clear();
        moved.resize(total, Moved::default());
        let mut from = 0;
        for (&h, &end) in subgroup.iter().zip(ends.iter()) {
            let own = &reached[from..end];
            from = end;
            if own.is_empty() {
                continue;
            }
            kernels::apply(mesh, p, &dab.mirrored(copy(h)), own, scratch);
            bank(mesh, own, leaves, &starts, saved, moved);
        }
        // Moves rather than positions, so a vertex no copy touched is not
        // written at all and keeps its position to the bit.
        let (saved, moved, holds) = (&*saved, &*moved, p.holds_a_height());
        mesh.move_vertices(leaves, |at, _| {
            let Moved { sum, furthest } = moved[at];
            if sum == DVec3::ZERO {
                return None;
            }
            let mut step = sum.as_vec3() * scale;
            // See the module's notes on a brush that holds a height.
            let length = step.length_squared();
            if holds && length > furthest {
                step *= (furthest / length).sqrt();
            }
            Some(saved[at] + step)
        });
        mesh.positions_changed(leaves);
    }
}

/// How many copies of a dab at `centre` sit on top of each other, of the
/// copies `subgroup` of `group` that can meet it. Each counts by how much of
/// its footprint it shares with the dab (the overlap of two discs of the
/// brush's radius), so the answer is the number of copies when they all land
/// on one spot (the dab on the axis), 1 when none overlap, and moves smoothly
/// between the two as the dab travels off the axis. A copy is as far from the
/// first one whichever coset it is in (the symmetries are rigid), so every
/// copy of every coset gets the same number.
fn multiplicity(group: &SymmetryGroup, subgroup: &[usize], centre: Vec3, radius: f32) -> f32 {
    let copies: f64 = subgroup
        .iter()
        .map(|&h| {
            let apart = centre.distance(group[h].apply_point(centre));
            shared_disc((apart / (2.0 * radius)) as f64)
        })
        .sum();
    copies.max(1.0) as f32
}

/// The area two equal discs share when their centres are `u` diameters
/// apart, as a fraction of one disc: all of it at 0, none from 1.
fn shared_disc(u: f64) -> f64 {
    if u >= 1.0 {
        0.0
    } else if u <= 0.0 {
        1.0
    } else {
        2.0 / std::f64::consts::PI * (u.acos() - u * (1.0 - u * u).sqrt())
    }
}

/// Adds what one copy did to every vertex of `own` to `moved`, measured from
/// `saved`, and puts the vertex back. `own` is some of `leaves`, whose
/// vertices `saved` and `moved` hold leaf after leaf, the leaf at `leaves[k]`
/// from `starts[k]`.
///
/// Added up in double. A vertex and its twin in another sector get the same
/// moves in a different order, and float addition is not associative: with
/// four terms or more the twins come out an ulp apart, a seed every brush
/// that follows the normals then amplifies. A handful of floats adds up
/// exactly in double, so the order stops mattering.
fn bank(mesh: &mut Mesh, own: &[u32], leaves: &[u32], starts: &[usize], saved: &[Vec3], moved: &mut [Moved]) {
    let ranges = &mesh.bvh.leaves;
    let owned = |leaf: u32| kernels::owned(&ranges[leaf as usize]);
    let place = |leaf: u32| leaves.binary_search(&leaf).map_or(0, |k| starts[k]);
    let total: usize = own.iter().map(|&leaf| owned(leaf).len()).sum();
    let live = kernels::stretches(&mut mesh.positions, own.iter().map(|&leaf| owned(leaf)));
    let sums = kernels::stretches(moved, own.iter().map(|&leaf| place(leaf)..place(leaf) + owned(leaf).len()));
    let work: Vec<(&mut [Vec3], &mut [Moved], usize)> =
        live.into_iter().zip(sums).zip(own).map(|((live, sums), &leaf)| (live, sums, place(leaf))).collect();
    let settle = |(live, sums, at): (&mut [Vec3], &mut [Moved], usize)| {
        for ((pos, moved), &before) in live.iter_mut().zip(sums.iter_mut()).zip(&saved[at..]) {
            let step = *pos - before;
            moved.sum += step.as_dvec3();
            moved.furthest = moved.furthest.max(step.length_squared());
            *pos = before;
        }
    };
    if total < kernels::PAR_MIN {
        work.into_iter().for_each(settle);
    } else {
        work.into_par_iter().with_min_len(64).for_each(settle);
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
            super::keep_connected(mesh, &[copy.0], &mut verts, &mut weights);
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
    /// A vertex only one copy reaches follows that copy's pull. Where copies
    /// overlap, near the axis or a mirror plane, their pulls are added (in
    /// double, so a vertex and its twin get the same total whatever order
    /// their copies come in) and shared out by the total weight once that
    /// passes 1: the weighted average, the way overlapping dabs are shared
    /// out by their multiplicity. A vertex on the axis then follows the
    /// pointer's motion along the axis one to one and stays on it.
    #[inline]
    pub fn pull(&self, slot: usize, pulls: &[Vec3]) -> Vec3 {
        let (mut pull, mut total) = (DVec3::ZERO, 0.0f64);
        for hold in self.start[slot] as usize..self.start[slot + 1] as usize {
            let w = self.weight[hold];
            pull += (pulls[self.copy[hold] as usize] * w).as_dvec3();
            total += w as f64;
        }
        if total > 1.0 {
            pull /= total;
        }
        pull.as_vec3()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discs_share_all_of_themselves_at_no_distance_and_nothing_a_diameter_apart() {
        assert_eq!(shared_disc(0.0), 1.0);
        assert_eq!(shared_disc(1.0), 0.0);
        assert_eq!(shared_disc(7.0), 0.0);
        // Half a diameter apart: 2/3 - sqrt(3) / (2 pi) of a disc.
        assert!((shared_disc(0.5) - (2.0 / 3.0 - 3f64.sqrt() / std::f64::consts::TAU)).abs() < 1e-12);
        let mut last = 1.0;
        for step in 1..=100 {
            let shared = shared_disc(step as f64 / 100.0);
            assert!(shared < last && shared >= 0.0, "it only ever falls with distance");
            last = shared;
        }
    }

    /// The number the copies' sum is divided by: the count of copies when
    /// they land on one spot, 1 when none touch, and in between by how much
    /// of the brush's own disc they share, whatever the dab's reach was.
    #[test]
    fn multiplicity_counts_copies_by_how_much_they_overlap() {
        let radius = 0.1;
        let off_axis = |from_axis: f32| Vec3::new(from_axis, 0.3, 0.0);
        for repeats in [2u32, 3, 8, 32] {
            let group = SymmetryGroup::build(false, false, false, repeats, Vec3::Y);
            let every: Vec<usize> = (0..group.len()).collect();
            assert_eq!(multiplicity(&group, &every, off_axis(0.0), radius), repeats as f32, "all on one spot");
            assert_eq!(multiplicity(&group, &every, off_axis(40.0 * radius), radius), 1.0, "none touching");
            let near = multiplicity(&group, &every, off_axis(0.3 * radius), radius);
            let further = multiplicity(&group, &every, off_axis(0.6 * radius), radius);
            assert!(near < repeats as f32 && further < near && further > 1.0, "x{repeats}: {near} then {further}");
        }
        // Two copies half a radius either side of the axis are half a
        // diameter apart: the dab itself, and 0.391 of its copy.
        let pair = SymmetryGroup::build(false, false, false, 2, Vec3::Y);
        let both = multiplicity(&pair, &[0, 1], off_axis(0.5 * radius), radius);
        assert!((both - 1.391).abs() < 1e-3, "{both}");
        // Copies that do not meet are not asked about: the dab alone is 1.
        assert_eq!(multiplicity(&pair, &[0], off_axis(0.5 * radius), radius), 1.0);
    }
}
