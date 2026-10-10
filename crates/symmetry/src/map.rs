//! A one-to-one pairing of vertices across a mirror plane: which vertex on one
//! side IS the same vertex on the other, which sit ON the plane (and so are
//! their own reflection), and which have no counterpart at all.
//!
//! Deliberately NOT consulted by brush strokes. Mirrored sculpting reflects the
//! brush position and re-runs the falloff on whatever vertices it finds there.
//! That never fails and never goes stale, which a map on the hot path could not
//! promise: it would have to be rebuilt after every remesh, join and extract,
//! and a map that silently fell out of date would move the WRONG vertices.
//!
//! What the map is for is the two things positional mirroring cannot do: TELL
//! you whether an object is actually symmetric, and REPAIR it when it is not. A
//! remesh re-tessellates both halves independently and an imported mesh may
//! never have matched at all, and once the sides have drifted nothing without
//! an explicit correspondence can put them back. `repair` reads this map to do
//! exactly that.
//!
//! Building it is three passes: pair by distance (`nominate`, `claim`), grow
//! the pairing along the mesh's own edges (`Pairing::propagate`), then take
//! back the pairs whose neighbourhoods do not correspond
//! (`Pairing::reject_inconsistent_pairs`). Each pass's proposals are worked out
//! across cores and accepted in one sorted sweep, so the map is a property of
//! the geometry: neither the order vertices are stored in nor the way the work
//! was scheduled can change it.

use std::collections::HashSet;

use glam::Vec3;
use rayon::prelude::*;
use sculpt_geom::{Aabb, MeshAdjacency};

use crate::grid::VertexGrid;
use crate::{Axis, par};

/// "No counterpart" in the pairing table.
const NONE: u32 = u32::MAX;

/// How far a topological propagation step may reach, as a multiple of the
/// mesh's VERTEX SPACING (falling back to the tolerance only when there are no
/// triangles to measure spacing from).
///
/// Propagation grows the pairing along edges out of pairs already found, so
/// the distance test is no longer being asked "are these two the same vertex"
/// on its own: the topology has already said they are corresponding neighbours
/// of a known pair, and this only has to cover how far the two sides have
/// DRIFTED. Decoupled from the tolerance because the two must be free to move
/// in opposite directions. The tolerance has to stay BELOW the vertex spacing
/// or it pairs a vertex with its reflection's neighbour; drift has to be
/// allowed to reach WELL PAST it, because one sculpt stroke moves a surface
/// much further than one spacing. As a multiple of the tolerance the reach
/// collapsed along with it the moment the tolerance was correctly tightened
/// (measured in the Unity app: pairs fell from 31,905 to 16,697).
///
/// Generous on purpose. This is only a coarse "not somewhere else entirely"
/// bound; the discrimination is done by the structural residual in `propose`.
pub const PROPAGATION_REACH: f32 = 8.0;

/// The other half of that bound, as a fraction of the bounding-box diagonal,
/// and the one that governs on a dense mesh.
///
/// Drift between two halves is a WORLD-SPACE quantity: how far a stroke pushed
/// the surface, which does not change when the mesh is re-tessellated more
/// finely. Vertex spacing does. A reach expressed only in spacings silently
/// tightens as density rises (measured: 33,207 pairs at 134k triangles but only
/// 106,121 of 496,178 at 992k). Taking whichever bound is LARGER keeps a coarse
/// mesh's few-spacings reach while letting a dense one span the same world
/// distance it always could.
pub const PROPAGATION_DRIFT_FRACTION: f32 = 0.10;

/// Propagation is a fixed point (each round can only pair vertices adjacent to
/// a pair found in an earlier one), so it stops on its own. The cap is a guard
/// against a pathological mesh, not a tuning knob. One round advances the
/// pairing by one ring of vertices, so a region left unpaired by a stroke needs
/// about radius / spacing rounds: at 64 this bound silently bit on a 1.08M
/// triangle mesh (~75 rings) and the repair was refused as "too asymmetric".
/// Affordable only because a round costs O(frontier), not O(vertex count).
const MAX_PROPAGATION_ROUNDS: usize = 4096;

/// How far a propagated pair's LOCAL offset may disagree, as a fraction of the
/// vertex spacing. A correct pair disagrees only by the remesher's own
/// placement noise whatever global drift there is between the halves, because
/// the anchor's drift is subtracted first; a vertex paired with its
/// reflection's NEIGHBOUR disagrees by about a full spacing. 0.75 clears the
/// measured noise ceiling of ~0.73 while staying under that.
const STRUCTURAL_FRACTION: f32 = 0.75;

/// Half the vertex spacing. The tolerance has to separate a correct pair from
/// an off-by-one-neighbour pair, and those sit at about 0 and 1 spacing apart.
/// Not tighter than half, because a remesher does not place mirrored vertices
/// at exactly mirrored positions and the discrepancy GROWS with resolution
/// (measured mean 0.061 / max 0.73 spacings at 1.08M triangles; 0.35 refused to
/// pair most of a perfectly symmetric model).
const SPACING_FRACTION: f32 = 0.5;

/// Half a percent of the bounding-box diagonal: the original rule, kept as a
/// CEILING for the coarse meshes it was calibrated on, where it is genuinely
/// below the spacing.
const DIAGONAL_FRACTION: f32 = 0.005;

/// Support at which a propagated candidate is taken whatever its positional
/// residual: the anchor plus both vertices flanking the edge to it, the whole
/// neighbourhood the true counterpart shares and no other candidate can.
const FULL_SUPPORT: u32 = 3;

/// Seeds further than this from an exact reflection, as a fraction of the
/// vertex spacing, wait until propagation has run. An undrifted pair is exact
/// on a mirror-exact mesh and within about 1e-5 of a spacing on a remesh, so a
/// tenth separates the surface that matches from the surface a seed can only
/// guess at.
const TIGHT_SEED_FRACTION: f32 = 0.1;

/// How many validation rounds `reject_inconsistent_pairs` may run. Each drops
/// what fails and re-propagates, and a round only fails pairs that border ones
/// it just changed, so this settles in two or three; the cap is a guard.
const MAX_VALIDATION_ROUNDS: usize = 6;

/// A tolerance proportional to the model rather than an absolute number,
/// because this is used on everything from a unit sphere to an imported
/// multi-metre scan, and a fixed epsilon would pair everything on one and
/// nothing on the other.
///
/// Bounded by BOTH the model's overall size and its actual vertex spacing. The
/// property every consumer relies on is that the tolerance stays comfortably
/// BELOW the vertex spacing: a tolerance approaching it starts pairing each
/// vertex with its reflection's NEIGHBOUR, and makes the "on the plane" band
/// (this same value, as a half-thickness) wide enough to swallow a strip of
/// real geometry either side of the centreline. Bounding-box diagonal alone
/// holds at 500 vertices and fails as the mesh gets denser, which was the root
/// cause of a repaired half coming back jagged.
///
/// Pass an empty `indices` for a point cloud; the diagonal rule then stands
/// alone.
pub fn default_tolerance(positions: &[Vec3], indices: &[u32]) -> f32 {
    if positions.is_empty() {
        return 0.001;
    }
    tolerance_for(diagonal(positions), mean_edge_length(positions, indices))
}

/// The same rule from measurements the caller already has: the model's
/// bounding diagonal and its vertex spacing (0 when there are no triangles).
pub fn tolerance_for(diagonal: f32, spacing: f32) -> f32 {
    let mut tolerance = diagonal * DIAGONAL_FRACTION;
    if spacing > 0.0 {
        tolerance = tolerance.min(spacing * SPACING_FRACTION);
    }
    tolerance.max(1e-6)
}

/// Mean length of one edge per triangle: a cheap, robust stand-in for vertex
/// spacing. One edge rather than all three because this only needs a scale, and
/// the three edges of a triangle are the same length to within the mesh's own
/// regularity. 0 when there are no triangles.
pub fn mean_edge_length(positions: &[Vec3], indices: &[u32]) -> f32 {
    let triangles = indices.len() / 3;
    if triangles == 0 {
        return 0.0;
    }
    let total = par::sum(triangles, |t| {
        let (a, b) = (positions[indices[t * 3] as usize], positions[indices[t * 3 + 1] as usize]);
        a.distance(b) as f64
    });
    (total / triangles as f64) as f32
}

/// Length of the bounding box's diagonal.
pub fn diagonal(positions: &[Vec3]) -> f32 {
    let bounds = positions
        .par_iter()
        .with_min_len(8192)
        .fold(|| Aabb::EMPTY, |mut b, &p| {
            b.grow(p);
            b
        })
        .reduce(|| Aabb::EMPTY, Aabb::union);
    if bounds.is_empty() { 0.0 } else { (bounds.max - bounds.min).length() }
}

pub struct SymmetryMap {
    axis: Axis,
    tolerance: f32,
    /// Each vertex's reflection: another vertex, itself when it is on the
    /// plane, `NONE` when it has no counterpart.
    mate: Vec<u32>,
    /// The mesh's topology, kept for the repairs that walk it. Its neighbour
    /// lists are ascending and without repeats, which `support` and
    /// `corresponds` rely on to test membership.
    adjacency: Option<MeshAdjacency>,
    mean_spacing: f32,
    diagonal: f32,
    pair_count: usize,
    on_plane_count: usize,
    unmatched_count: usize,
    propagated_pair_count: usize,
    rejected_pair_count: usize,
}

impl SymmetryMap {
    /// Builds the pairing across the plane through the origin perpendicular to
    /// `axis`.
    ///
    /// `tolerance` is how far apart two positions may be and still count as
    /// reflections of each other by distance alone, and the half-thickness of
    /// the "on the plane" band; see `default_tolerance` for the scale it wants.
    ///
    /// Always pass the triangles where they are to hand. Distance alone can
    /// only pair vertices still within `tolerance` of their reflection, a
    /// judgement about DRIFT made with no way to tell drift from a different
    /// vertex nearby; with the topology the two questions separate and the
    /// pairing stops depending on how far the model has been pushed out of
    /// shape. An empty `indices` builds a distance-only map, which cannot
    /// answer neighbour queries (`has_topology` is false).
    pub fn build(positions: &[Vec3], indices: &[u32], axis: Axis, tolerance: f32) -> SymmetryMap {
        let n = positions.len();
        let tolerance = tolerance.max(1e-6);
        let mut map = SymmetryMap {
            axis,
            tolerance,
            mate: vec![NONE; n],
            adjacency: None,
            mean_spacing: 0.0,
            diagonal: 0.0,
            pair_count: 0,
            on_plane_count: 0,
            unmatched_count: 0,
            propagated_pair_count: 0,
            rejected_pair_count: 0,
        };
        if n == 0 {
            return map;
        }

        claim(&mut map.mate, &nominate(positions, axis, tolerance));

        // Everything above is a distance test, and a distance test can only
        // ever pair the parts of the model that have not drifted far, which on
        // a model that needs repairing is precisely the parts that did not
        // need it.
        if indices.len() >= 3 {
            map.mean_spacing = mean_edge_length(positions, indices);
            map.diagonal = diagonal(positions);
            let adjacency = MeshAdjacency::build(n, indices);
            let mate = std::mem::take(&mut map.mate);
            let mut pairing = Pairing::new(&map, positions, &adjacency, mate);

            // Only the seeds that are near-exact reflections go first; the
            // rest wait until propagation has had its chance at their
            // vertices. A loose seed is a distance judgement made with no way
            // to tell drift from a different vertex nearby: where one half has
            // slid or been pushed through the other's mirrored surface it is
            // often the wrong vertex, and propagation then grows a whole patch
            // of pairs out of it, all consistently offset. Reached by
            // propagation from undrifted surface instead, the same vertices
            // pair by topology.
            let loose = pairing.withhold_loose_seeds();
            pairing.propagate(None, None);
            let readmitted = pairing.readmit_seeds(loose);
            if !readmitted.is_empty() {
                pairing.propagate(Some(&readmitted), None);
            }
            pairing.reject_inconsistent_pairs();

            map.propagated_pair_count = pairing.propagated;
            map.rejected_pair_count = pairing.rejected_pairs;
            map.mate = pairing.mate;
            map.adjacency = Some(adjacency);
        }

        let mate = &map.mate;
        map.on_plane_count = par::count(n, |i| mate[i] == i as u32);
        map.unmatched_count = par::count(n, |i| mate[i] == NONE);
        map.pair_count = (n - map.on_plane_count - map.unmatched_count) / 2;
        map
    }

    pub fn axis(&self) -> Axis {
        self.axis
    }

    /// The tolerance the map was built with.
    pub fn tolerance(&self) -> f32 {
        self.tolerance
    }

    pub fn vertex_count(&self) -> usize {
        self.mate.len()
    }

    /// Whether the map was built with triangles, and so can answer neighbour
    /// queries and has grown its pairing along the surface.
    pub fn has_topology(&self) -> bool {
        self.adjacency.is_some()
    }

    /// The vertices sharing an edge with `vertex`, ascending. Empty for a map
    /// built without triangles.
    #[inline]
    pub fn neighbours(&self, vertex: u32) -> &[u32] {
        self.adjacency.as_ref().map_or(&[], |a| a.neighbours_of(vertex as usize))
    }

    /// The topology the map was built over, for repairs that need a vertex's
    /// triangles as well as its neighbours. `None` without triangles.
    pub fn adjacency(&self) -> Option<&MeshAdjacency> {
        self.adjacency.as_ref()
    }

    /// The mesh's own vertex spacing (0 without triangles).
    pub fn mean_spacing(&self) -> f32 {
        self.mean_spacing
    }

    /// The model's bounding-box diagonal: the scale drift is measured in, as
    /// against `mean_spacing`, the scale a vertex's identity is measured in.
    pub fn diagonal(&self) -> f32 {
        self.diagonal
    }

    /// Vertex pairs found (counted once per pair, not once per vertex).
    pub fn pair_count(&self) -> usize {
        self.pair_count
    }

    /// How many pairs came from propagating along the surface rather than from
    /// the distance test alone.
    pub fn propagated_pair_count(&self) -> usize {
        self.propagated_pair_count
    }

    /// Pairs made and then taken back because the two vertices' neighbourhoods
    /// did not correspond.
    pub fn rejected_pair_count(&self) -> usize {
        self.rejected_pair_count
    }

    /// Vertices sitting on the mirror plane: the centreline of a symmetric
    /// model.
    pub fn on_plane_count(&self) -> usize {
        self.on_plane_count
    }

    /// Vertices that are neither on the plane nor paired with anything. These
    /// are precisely the places the model is NOT symmetric, and the count is
    /// the honest answer to "is this object symmetric" that a user otherwise
    /// has to judge by eye.
    pub fn unmatched_count(&self) -> usize {
        self.unmatched_count
    }

    pub fn is_symmetric(&self) -> bool {
        self.unmatched_count == 0
    }

    /// The vertex across the plane that `vertex` is paired with. `None` both
    /// for a vertex with no counterpart and for one on the plane, which has no
    /// SEPARATE partner, being its own reflection (`is_on_plane` tells the two
    /// apart).
    #[inline]
    pub fn partner_of(&self, vertex: u32) -> Option<u32> {
        let mate = self.mate[vertex as usize];
        (mate != NONE && mate != vertex).then_some(mate)
    }

    #[inline]
    pub fn is_on_plane(&self, vertex: u32) -> bool {
        self.mate[vertex as usize] == vertex
    }

    /// `vertex`'s reflection as a vertex: its partner, or itself on the plane.
    #[inline]
    pub fn counterpart(&self, vertex: u32) -> Option<u32> {
        let mate = self.mate[vertex as usize];
        (mate != NONE).then_some(mate)
    }

    /// Neither paired nor on the plane.
    #[inline]
    pub fn is_unmatched(&self, vertex: u32) -> bool {
        self.mate[vertex as usize] == NONE
    }

    /// One-line report for the UI. The whole value of a correspondence map is
    /// that it can answer "is this symmetric, and where isn't it" with numbers
    /// instead of leaving the user to rotate the model and squint at it.
    pub fn summary(&self) -> String {
        let axis = self.axis.name();
        if self.mate.is_empty() {
            return format!("{axis}: no geometry");
        }
        if self.is_symmetric() {
            return format!("{axis}: symmetric - {} pairs, {} on centre", self.pair_count, self.on_plane_count);
        }
        format!(
            "{axis}: {} unmatched of {} ({} pairs, {} on centre)",
            self.unmatched_count,
            self.mate.len(),
            self.pair_count,
            self.on_plane_count
        )
    }
}

/// A candidate pairing from the distance pass. Ordered by squared distance,
/// then by the two vertices, so sorting gives closest-first with ties broken
/// the same way every time. The distance is held as its bit pattern: it is
/// never negative, and for such floats bit order is value order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Seed {
    sqr: u32,
    a: u32,
    b: u32,
}

/// Pass 1: every off-plane vertex nominates the vertices across the plane
/// within reach of its reflection.
///
/// Several candidates rather than just the nearest, because meshes routinely
/// carry COINCIDENT vertices: a UV seam splits the vertices along one meridian
/// and poles are split once per surrounding triangle. Those sit at the SAME
/// position, so which one a vertex would nominate first is arbitrary, and any
/// rule that needs two vertices to independently agree on an arbitrary choice
/// will keep disagreeing (nearest-only left 115 of a 515-vertex primitive
/// sphere "unmatched").
fn nominate(positions: &[Vec3], axis: Axis, tolerance: f32) -> Vec<Seed> {
    // A vertex in the centreline band is matched out to its own distance from
    // its reflection, even past the tolerance: any twin nearer than that is a
    // better account of it than "on the plane". Never more than twice the
    // tolerance, the band's own width.
    let reach = |side: f32| if side.abs() <= tolerance { tolerance.max(2.0 * side.abs()) } else { tolerance };
    let grid = VertexGrid::build(positions, tolerance * 2.0);

    let mut seeds = par::gather(positions.len(), |i, out| {
        let p = positions[i];
        let side = axis.coord(p);
        let i = i as u32;

        // A vertex in the band nominates ITSELF too, at its distance from its
        // own reflection, and competes in the greedy pass with every twin
        // across the plane. It is filed as on the plane only if nothing sits
        // closer to its reflection than it does.
        //
        // Being inside the band used to settle it outright, and the band is
        // half a vertex spacing wide, which on a remesh is exactly where the
        // first row of vertices either side of the plane lands. Measured on a
        // remeshed symmetric sphere: 873 of 1224 "centreline" vertices were in
        // fact one half of a mirrored pair 0.8 spacings apart, and snapping
        // them to the plane collapsed the edge between them.
        if side.abs() <= tolerance {
            out.push(Seed { sqr: (4.0 * side * side).to_bits(), a: i, b: i });
        }
        // Exactly on the plane: its own reflection, and nothing else can be.
        if side == 0.0 {
            return;
        }

        grid.for_each_within(axis.reflect(p), reach(side), |j, q, d| {
            // Must genuinely be across the plane, strictly: two vertices on
            // one side are not a reflection however close they land, and a
            // vertex exactly on the plane is its own twin, never anyone else's.
            let other = axis.coord(q);
            if j == i || other * side >= 0.0 {
                return;
            }
            // Both ends find each other unless only this one reaches that far,
            // so the pair is listed once: by its lower end, or by the only end
            // that can see it.
            let back = reach(other);
            if i < j || d > back * back {
                out.push(Seed { sqr: d.to_bits(), a: i, b: j });
            }
        });
    });
    par::sort(&mut seeds);
    seeds
}

/// Pass 2: greedy one-to-one matching, closest pairs first. Each vertex can be
/// claimed once, so the result is a genuine involution, exactly what a repair
/// needs to copy one side onto the other without a vertex being written twice.
///
/// Closest-first is what makes greedy the right rule rather than merely a
/// cheap one: a true reflection sits at distance ~0, so real pairs are matched
/// long before any looser almost-match gets a chance to steal either end.
/// Where several candidates are exactly coincident the choice between them is
/// arbitrary AND harmless: they occupy the same point.
fn claim(mate: &mut [u32], seeds: &[Seed]) {
    for seed in seeds {
        let (a, b) = (seed.a as usize, seed.b as usize);
        if mate[a] != NONE || mate[b] != NONE {
            continue;
        }
        // A vertex paired with itself: its own reflection was the closest
        // thing to it, so it is on the plane.
        mate[a] = seed.b;
        mate[b] = seed.a;
    }
}

/// A candidate pairing from propagation. Sorting puts the best supported
/// first, then the smallest structural residual, then a fixed order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Candidate {
    /// `u32::MAX - support`, so more support sorts first.
    rank: u32,
    /// Squared structural residual, as bits (see `Seed`).
    sqr: u32,
    a: u32,
    b: u32,
    /// The paired vertex that proposed this candidate.
    anchor: u32,
}

/// The pairing while it is being grown along the surface.
struct Pairing<'a> {
    positions: &'a [Vec3],
    adjacency: &'a MeshAdjacency,
    axis: Axis,
    mate: Vec<u32>,
    /// "Already in the next frontier" flags, cleared again after every use.
    queued: Vec<bool>,
    limit_sqr: f32,
    structural_sqr: f32,
    tight_sqr: f32,
    /// Pairs validation has taken back TWICE, which propagation may not make
    /// again, so re-propagation cannot keep recreating a pair that keeps
    /// failing. Not on the first failure: a correct pair fails too when its
    /// neighbours were wrongly paired, and once those are corrected it has to
    /// be free to come back.
    rejected: HashSet<u64>,
    rejected_once: HashSet<u64>,
    propagated: usize,
    rejected_pairs: usize,
}

impl<'a> Pairing<'a> {
    fn new(map: &SymmetryMap, positions: &'a [Vec3], adjacency: &'a MeshAdjacency, mate: Vec<u32>) -> Pairing<'a> {
        // Whichever bound is larger: a few vertex spacings, or a fixed
        // fraction of the model. The first governs a coarse mesh, the second a
        // dense one.
        let spacing = map.mean_spacing;
        let spacing_reach = (if spacing > 0.0 { spacing } else { map.tolerance }) * PROPAGATION_REACH;
        let limit = spacing_reach.max(map.diagonal * PROPAGATION_DRIFT_FRACTION);
        // No spacing to measure against means no structural test either.
        let structural = spacing * STRUCTURAL_FRACTION;
        let tight = spacing * TIGHT_SEED_FRACTION;
        Pairing {
            positions,
            adjacency,
            axis: map.axis,
            queued: vec![false; mate.len()],
            mate,
            limit_sqr: limit * limit,
            structural_sqr: if spacing > 0.0 { structural * structural } else { f32::MAX },
            tight_sqr: if spacing > 0.0 { tight * tight } else { f32::MAX },
            rejected: HashSet::new(),
            rejected_once: HashSet::new(),
            propagated: 0,
            rejected_pairs: 0,
        }
    }

    /// Unpairs every seed further than `TIGHT_SEED_FRACTION` from an exact
    /// reflection and returns them, closest first, for `readmit_seeds`.
    fn withhold_loose_seeds(&mut self) -> Vec<Seed> {
        let (mate, positions, axis, tight_sqr) = (&self.mate, self.positions, self.axis, self.tight_sqr);
        let mut loose = par::gather(mate.len(), |i, out| {
            let j = mate[i];
            if j == NONE || j as usize <= i {
                return;
            }
            let d = positions[j as usize].distance_squared(axis.reflect(positions[i]));
            if d > tight_sqr {
                out.push(Seed { sqr: d.to_bits(), a: i as u32, b: j });
            }
        });
        for seed in &loose {
            self.mate[seed.a as usize] = NONE;
            self.mate[seed.b as usize] = NONE;
        }
        par::sort(&mut loose);
        loose
    }

    /// Puts back the withheld seeds whose vertices propagation did not reach
    /// (a part of the model with no near-exact surface to grow from, like a
    /// separate shell), closest first, as the seeding pass itself does.
    /// Returns the vertices re-paired.
    fn readmit_seeds(&mut self, loose: Vec<Seed>) -> Vec<u32> {
        let mut readmitted = Vec::new();
        for seed in loose {
            if self.mate[seed.a as usize] != NONE || self.mate[seed.b as usize] != NONE {
                continue;
            }
            self.mate[seed.a as usize] = seed.b;
            self.mate[seed.b as usize] = seed.a;
            readmitted.extend([seed.a, seed.b]);
        }
        readmitted
    }

    /// Grows the pairing outward along the surface: where `i` and `j` are
    /// already partners, a neighbour of `i` should pair with a neighbour of
    /// `j`, and the only question left is which one. That is asked of a
    /// handful of candidates the topology has already vouched for, instead of
    /// every vertex in a tolerance ball, so it keeps working long after the two
    /// sides have drifted past any distance the seeding pass could safely use.
    ///
    /// This is what decides whether a repair is exact. Distance-only pairing
    /// left 540 of a 17,650-vertex sculpted torso unpaired, and a repair cannot
    /// move a vertex it has no counterpart for.
    ///
    /// Rounds are batched and each round's candidates accepted best-first, for
    /// the same reason the seeding pass is greedy: the order vertices happen to
    /// be visited in must not decide which of two near-equal candidates wins.
    ///
    /// `start` is the vertices to grow from (everything paired when `None`);
    /// vertices that gain a partner are appended to `paired`.
    fn propagate(&mut self, start: Option<&[u32]>, mut paired: Option<&mut Vec<u32>>) {
        // Only vertices that gained a pairing in the PREVIOUS round can offer
        // anything new this round, so each round walks that frontier instead of
        // rescanning the whole mesh. Sound because a candidate only becomes
        // MORE acceptable when one of its neighbours gets paired (its support
        // grows), and that neighbour, being newly paired itself, is in the next
        // frontier and proposes it again.
        let mut frontier = match start {
            None => {
                let mate = &self.mate;
                par::indices_where(mate.len(), |i| mate[i] != NONE)
            }
            Some(list) => {
                let mut unique = Vec::with_capacity(list.len());
                for &i in list {
                    if self.mate[i as usize] != NONE && !self.queued[i as usize] {
                        self.queued[i as usize] = true;
                        unique.push(i);
                    }
                }
                unique.iter().for_each(|&i| self.queued[i as usize] = false);
                unique
            }
        };
        let mut next = Vec::new();

        for _ in 0..MAX_PROPAGATION_ROUNDS {
            if frontier.is_empty() {
                return;
            }
            let (full, held) = self.propose(&frontier);

            // While any candidate anywhere has full support, only those are
            // taken. A weaker one is a tie between two of j's neighbours that
            // position alone breaks, wrong wherever the half has slid, and one
            // wrong pair then grows a whole patch of consistently offset ones
            // with full support of their own. Deferred, it is usually settled
            // by full support from another side a round or two later; its
            // anchor stays in the frontier so it is proposed again.
            //
            // On a mesh that has drifted a little everywhere the weaker ones
            // are nearly all of the candidates, round after round, which is why
            // they are kept apart: in such a round they are only looked through
            // for anchors to keep, never gathered into one list or sorted.
            let (mut taken, held) = if full.is_empty() { (held.concat(), Vec::new()) } else { (full, held) };
            if taken.is_empty() {
                return;
            }
            par::sort(&mut taken);

            next.clear();
            let mut accepted = 0;
            for c in &taken {
                if self.mate[c.a as usize] != NONE || self.mate[c.b as usize] != NONE {
                    continue;
                }
                self.mate[c.a as usize] = c.b;
                self.mate[c.b as usize] = c.a;
                if let Some(list) = paired.as_mut() {
                    list.extend([c.a, c.b]);
                }
                // Both ends become anchors for the next round: they are the
                // only vertices that can vouch for anything new.
                self.enqueue(c.a, &mut next);
                self.enqueue(c.b, &mut next);
                accepted += 1;
            }
            for anchor in self.anchors_still_waiting(&held) {
                self.enqueue(anchor, &mut next);
            }
            next.iter().for_each(|&i| self.queued[i as usize] = false);

            if accepted == 0 {
                return;
            }
            self.propagated += accepted;
            std::mem::swap(&mut frontier, &mut next);
        }
    }

    #[inline]
    fn around(&self, vertex: u32) -> &'a [u32] {
        self.adjacency.neighbours_of(vertex as usize)
    }

    fn enqueue(&mut self, vertex: u32, list: &mut Vec<u32>) {
        if !self.queued[vertex as usize] {
            self.queued[vertex as usize] = true;
            list.push(vertex);
        }
    }

    /// Every pairing the frontier's anchors can vouch for.
    ///
    /// "Best" is measured STRUCTURALLY: how well `b` sits relative to its
    /// anchor `j` compared to where `a` sits relative to its anchor `i`, rather
    /// than by raw distance from `a`'s reflection. The two agree when the
    /// halves have not drifted and diverge exactly when they have. Raw distance
    /// has to be given a wide radius to tolerate drift at all, wide enough on a
    /// dense mesh for the closest candidate to be the reflection's NEIGHBOUR;
    /// those wrong pairs are what a repair then writes mismatched positions
    /// through (a jagged half, flipped triangles). The structural residual
    /// cancels whatever drift `i` and `j` already carry.
    ///
    /// That still fails where the drift is not uniform (one half slid along its
    /// surface by a varying amount, which is what Smooth and Clay's relax do),
    /// because each step's residual error rides on the last and a chain walks
    /// off by a vertex at a time. So candidates are ranked first by TOPOLOGICAL
    /// support: how many of `a`'s neighbours already have a counterpart that
    /// neighbours `b`. The true counterpart shares its whole paired
    /// neighbourhood; a neighbour of it in `j`'s ring shares at most two.
    /// Position only breaks ties, and a candidate with full support is taken
    /// even past the residual gate, since sliding moves positions but not
    /// connectivity.
    ///
    /// Returns the candidates with full support as one list, and the rest as
    /// they came off each core (see `propagate` for why they are kept apart).
    fn propose(&self, frontier: &[u32]) -> (Vec<Candidate>, Vec<Vec<Candidate>>) {
        // Each anchor weighs a few dozen candidates, and a round's frontier is
        // often only a few thousand anchors: small blocks keep those rounds
        // spread across cores too.
        let block = (frontier.len() / 256).clamp(512, 8192);
        let blocks: Vec<(Vec<Candidate>, Vec<Candidate>)> = frontier
            .par_chunks(block)
            .map(|anchors| {
                let (mut full, mut rest) = (Vec::new(), Vec::new());
                anchors.iter().for_each(|&i| self.propose_from(i, &mut full, &mut rest));
                (full, rest)
            })
            .collect();

        let mut full = Vec::with_capacity(blocks.iter().map(|b| b.0.len()).sum());
        let mut held = Vec::with_capacity(blocks.len());
        for (block_full, block_rest) in blocks {
            full.extend(block_full);
            held.push(block_rest);
        }
        (full, held)
    }

    /// The pairings anchor `i` can vouch for: those with full support into
    /// `full`, the others into `rest`.
    fn propose_from(&self, i: u32, full: &mut Vec<Candidate>, rest: &mut Vec<Candidate>) {
        let (v, axis) = (self.positions, self.axis);
        // An on-plane vertex is its own reflection, so it seeds propagation
        // into BOTH sides at once, which is what carries the pairing off the
        // centreline on a model whose halves only meet there.
        let j = self.mate[i as usize];
        if j == NONE {
            return;
        }
        // Where the anchor pair's own reflection lands. The residual is
        // measured against THIS, so any drift already between i and j cancels
        // out instead of being charged to every candidate.
        let anchor_drift = v[j as usize] - axis.reflect(v[i as usize]);

        for &a in self.around(i) {
            if self.mate[a as usize] != NONE {
                continue;
            }
            let target = axis.reflect(v[a as usize]);
            let expected = target + anchor_drift;
            let side_a = axis.coord(v[a as usize]);
            // No candidate can have more support than `a` has paired
            // neighbours, which early on is too few to override a gate: then a
            // candidate that fails one is dropped without its support ever
            // being counted.
            let ceiling = self.around(a).iter().filter(|&&c| self.mate[c as usize] != NONE).count() as u32;

            for &b in self.around(j) {
                if b == a || self.mate[b as usize] != NONE {
                    continue;
                }
                // Both gates must pass. The absolute one keeps the pairing
                // anchored to real geometry; the structural one is what
                // discriminates between the reflection and its neighbour.
                let pb = v[b as usize];
                if pb.distance_squared(target) > self.limit_sqr {
                    continue;
                }
                if !self.rejected.is_empty() && self.rejected.contains(&pair_key(a, b)) {
                    continue;
                }
                let residual = pb.distance_squared(expected);
                // A reflection lives across the plane, unless the topology
                // says otherwise outright: a half that slid along its surface
                // near the centreline can carry a vertex a hair across the
                // plane, and its counterpart is then on the same side. Refusing
                // it left a strip of the centreline unpaired and torn by the
                // repair.
                let gated = residual > self.structural_sqr || axis.coord(pb) * side_a > 0.0;
                if gated && ceiling < FULL_SUPPORT {
                    continue;
                }
                let support = self.support(a, b);
                if gated && support < FULL_SUPPORT {
                    continue;
                }
                let candidate = Candidate { rank: u32::MAX - support, sqr: residual.to_bits(), a, b, anchor: i };
                if support >= FULL_SUPPORT { full.push(candidate) } else { rest.push(candidate) }
            }
        }
    }

    /// The anchors of held-over candidates whose two vertices are both still
    /// unpaired: the ones worth proposing again next round. An anchor may be
    /// listed more than once.
    fn anchors_still_waiting(&self, held: &[Vec<Candidate>]) -> Vec<u32> {
        let waiting = |c: &&Candidate| self.mate[c.a as usize] == NONE && self.mate[c.b as usize] == NONE;
        let blocks: Vec<Vec<u32>> = held
            .par_iter()
            .map(|block| {
                // An anchor's candidates come one after another, so dropping
                // immediate repeats drops nearly all of them.
                let mut anchors: Vec<u32> = block.iter().filter(waiting).map(|c| c.anchor).collect();
                anchors.dedup();
                anchors
            })
            .collect();
        blocks.concat()
    }

    /// How many of `a`'s neighbours have a counterpart among `b`'s neighbours.
    fn support(&self, a: u32, b: u32) -> u32 {
        let around_b = self.around(b);
        self.around(a)
            .iter()
            .filter(|&&c| {
                let counterpart = self.mate[c as usize];
                counterpart != NONE && around_b.binary_search(&counterpart).is_ok()
            })
            .count() as u32
    }

    /// Whether most of `i`'s paired neighbours are counterparts of `j`'s
    /// neighbours. Too few paired neighbours to judge by (fewer than two)
    /// counts as agreeing.
    fn corresponds(&self, i: u32, j: u32) -> bool {
        let around_j = self.around(j);
        let (mut judged, mut agreed) = (0, 0);
        for &a in self.around(i) {
            let counterpart = self.mate[a as usize];
            if counterpart == NONE {
                continue;
            }
            judged += 1;
            if counterpart != j && around_j.binary_search(&counterpart).is_ok() {
                agreed += 1;
            }
        }
        judged < 2 || agreed * 2 >= judged
    }

    /// Judged from both ends.
    fn pair_holds(&self, i: u32, j: u32) -> bool {
        self.corresponds(i, j) && self.corresponds(j, i)
    }

    /// Takes back every pair whose two vertices' neighbourhoods do not
    /// correspond, then lets propagation re-pair them from the pairs that do.
    ///
    /// Seeding and propagation both decide by POSITION, and neither can tell
    /// the model's own shape from drift. Where one half has slid along its
    /// surface or been pushed out through the other half's mirrored surface,
    /// the closest candidate is often not the counterpart at all, and a repair
    /// then writes the source side's positions through those pairs: the "weird
    /// shearing" on the repaired side (measured on a remeshed sphere after
    /// one-sided Smooth and Clay strokes: 2389 triangles flipped, edges
    /// stretched to 13.6 spacings).
    ///
    /// Topology can tell. If `i` and `j` are counterparts, `i`'s neighbours are
    /// `j`'s neighbours' counterparts; a wrong pair's neighbours map somewhere
    /// else entirely. So a pair is kept only if at least half of `i`'s paired
    /// neighbours map onto neighbours of `j` (not all: two halves triangulated
    /// along opposite diagonals disagree on a couple of neighbours for a
    /// correct pair). What stays unpaired is left to `repair::carry_unmatched`,
    /// which moves it with its surroundings instead of through a pair that
    /// would tear it.
    fn reject_inconsistent_pairs(&mut self) {
        let n = self.mate.len();
        let mut mark = vec![false; n];
        // After the first round only pairs near a change can have changed
        // their verdict.
        let mut suspects: Option<Vec<u32>> = None;

        for _ in 0..MAX_VALIDATION_ROUNDS {
            let failed: Vec<u32> = match &suspects {
                None => {
                    // Each pair once, from its lower end.
                    let this = &*self;
                    par::indices_where(n, |i| {
                        let j = this.mate[i];
                        j != NONE && j as usize > i && !this.pair_holds(i as u32, j)
                    })
                }
                Some(list) => {
                    let mut failed = Vec::new();
                    for &v in list {
                        let j = self.mate[v as usize];
                        if j == NONE || j == v {
                            continue;
                        }
                        let i = v.min(j);
                        if std::mem::replace(&mut mark[i as usize], true) {
                            continue;
                        }
                        if !self.pair_holds(i, v.max(j)) {
                            failed.push(i);
                        }
                    }
                    for &v in list {
                        let j = self.mate[v as usize];
                        mark[v as usize] = false;
                        if j != NONE {
                            mark[j as usize] = false;
                        }
                    }
                    failed
                }
            };
            if failed.is_empty() {
                return;
            }

            let mut changed = Vec::with_capacity(failed.len() * 2);
            for &i in &failed {
                let j = self.mate[i as usize];
                let key = pair_key(i, j);
                if !self.rejected_once.insert(key) {
                    self.rejected.insert(key);
                }
                self.mate[i as usize] = NONE;
                self.mate[j as usize] = NONE;
                changed.extend([i, j]);
            }
            self.rejected_pairs += failed.len();

            // Re-propagate from the anchors around what was dropped.
            let anchors = self.ring(&changed, &mut mark);
            self.propagate(Some(&anchors), Some(&mut changed));

            // Next round's suspects: everything dropped or re-paired, and
            // their neighbours.
            let mut next = self.ring(&changed, &mut mark);
            next.extend(&changed);
            suspects = Some(next);
        }
    }

    /// The neighbours of `vertices`, each once (`mark` must be all false and is
    /// left so).
    fn ring(&self, vertices: &[u32], mark: &mut [bool]) -> Vec<u32> {
        let mut ring = Vec::new();
        for &v in vertices {
            for &a in self.around(v) {
                if !std::mem::replace(&mut mark[a as usize], true) {
                    ring.push(a);
                }
            }
        }
        ring.iter().for_each(|&a| mark[a as usize] = false);
        ring
    }
}

fn pair_key(a: u32, b: u32) -> u64 {
    ((a.min(b) as u64) << 32) | a.max(b) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{Lcg, exact_partners, icosphere, octa_partners, octa_sphere};
    use sculpt_geom::TriMesh;

    fn build(mesh: &TriMesh, axis: Axis) -> SymmetryMap {
        let tolerance = default_tolerance(&mesh.positions, &mesh.indices);
        SymmetryMap::build(&mesh.positions, &mesh.indices, axis, tolerance)
    }

    /// Asserts the map is an involution and names exactly `partners`.
    fn assert_matches(map: &SymmetryMap, partners: &[u32], label: &str) {
        for (i, &want) in partners.iter().enumerate() {
            let i = i as u32;
            assert_eq!(map.counterpart(i), Some(want), "{label}: vertex {i}");
            assert_eq!(map.is_on_plane(i), want == i, "{label}: vertex {i}");
            if let Some(j) = map.partner_of(i) {
                assert_eq!(map.partner_of(j), Some(i), "{label}: not an involution at {i}");
            }
        }
    }

    #[test]
    fn symmetric_sphere_maps_every_vertex_to_its_exact_partner() {
        let mesh = octa_sphere(20, 1.0);
        for axis in Axis::ALL {
            let partners = octa_partners(20, axis);
            let map = build(&mesh, axis);
            assert_matches(&map, &partners, axis.name());
            assert!(map.is_symmetric());
            assert_eq!(map.on_plane_count(), 4 * 20);
            assert_eq!(map.pair_count(), (mesh.vertex_count() - 80) / 2);
            assert_eq!(map.rejected_pair_count(), 0);
            assert_eq!(map.summary(), format!("{}: symmetric - 761 pairs, 80 on centre", axis.name()));

            // By distance alone too: every reflection is exact.
            let cloud = SymmetryMap::build(&mesh.positions, &[], axis, map.tolerance());
            assert_matches(&cloud, &partners, "no topology");
            assert!(!cloud.has_topology() && cloud.neighbours(0).is_empty());
        }
    }

    /// The Unity fixture: bumpy, irregular, and mirror-exact on all three planes.
    #[test]
    fn fixture_is_exactly_symmetric_and_the_map_finds_it() {
        let (mesh, partners) = icosphere(5);
        for axis in Axis::ALL {
            let map = build(&mesh, axis);
            assert_matches(&map, &partners[axis.index()], axis.name());
            assert!(map.is_symmetric(), "{}", map.summary());
        }
    }

    #[test]
    fn neighbours_are_sorted_distinct_and_mutual() {
        let mesh = octa_sphere(6, 1.0);
        let map = build(&mesh, Axis::X);
        assert!(map.has_topology());
        let mut edges = 0;
        for v in 0..mesh.vertex_count() as u32 {
            let list = map.neighbours(v);
            assert!(list.windows(2).all(|w| w[0] < w[1]), "vertex {v}: {list:?}");
            assert!(list.iter().all(|&a| map.neighbours(a).contains(&v)));
            edges += list.len();
        }
        // A closed triangle mesh has three edges for every two triangles.
        assert_eq!(edges / 2, mesh.triangle_count() * 3 / 2);
    }

    #[test]
    fn tolerance_stays_below_the_vertex_spacing() {
        let coarse = octa_sphere(3, 1.0);
        let fine = octa_sphere(80, 1.0);
        for mesh in [&coarse, &fine] {
            let spacing = mean_edge_length(&mesh.positions, &mesh.indices);
            let tolerance = default_tolerance(&mesh.positions, &mesh.indices);
            assert!(tolerance <= spacing * 0.5 + 1e-9 && tolerance > 0.0);
        }
        // Coarse: the diagonal rule is the tighter one. Fine: the spacing takes over.
        let coarse_tolerance = default_tolerance(&coarse.positions, &coarse.indices);
        assert!((coarse_tolerance - diagonal(&coarse.positions) * 0.005).abs() < 1e-7);
        let fine_spacing = mean_edge_length(&fine.positions, &fine.indices);
        assert!((default_tolerance(&fine.positions, &fine.indices) - fine_spacing * 0.5).abs() < 1e-7);
        assert_eq!(default_tolerance(&[], &[]), 0.001);
        assert_eq!(tolerance_for(0.0, 0.0), 1e-6);
    }

    #[test]
    fn empty_map_reports_no_geometry() {
        let map = SymmetryMap::build(&[], &[], Axis::Z, 0.1);
        assert_eq!(map.vertex_count(), 0);
        assert_eq!(map.summary(), "Z: no geometry");
        assert!(map.is_symmetric());
    }

    /// Coincident vertices (a UV seam, a split pole) must not leave anything unmatched: a
    /// symmetric mesh with every vertex doubled still pairs completely.
    #[test]
    fn duplicated_vertices_still_pair_completely() {
        let mesh = octa_sphere(8, 1.0);
        let mut positions = mesh.positions.clone();
        positions.extend_from_slice(&mesh.positions);
        let tolerance = default_tolerance(&mesh.positions, &mesh.indices);
        let map = SymmetryMap::build(&positions, &[], Axis::X, tolerance);
        assert!(map.is_symmetric(), "{}", map.summary());
        assert_eq!(map.on_plane_count(), 2 * 4 * 8);
        for i in 0..positions.len() as u32 {
            let j = map.counterpart(i).unwrap();
            assert_eq!(positions[j as usize], Axis::X.reflect(positions[i as usize]));
        }
    }

    /// A vertex a hair off the plane with a twin a hair off the other side is half of a PAIR,
    /// not a centreline vertex, even though both sit inside the band.
    #[test]
    fn a_straddling_pair_inside_the_band_is_a_pair() {
        let tolerance = 0.01;
        let positions = vec![
            Vec3::new(0.004, 0.0, 0.0),
            Vec3::new(-0.004, 0.0005, 0.0),
            Vec3::new(0.003, 1.0, 0.0),
            Vec3::new(0.0, 2.0, 0.0),
            Vec3::new(0.5, 0.0, 0.0),
        ];
        let map = SymmetryMap::build(&positions, &[], Axis::X, tolerance);
        assert_eq!(map.partner_of(0), Some(1));
        assert_eq!(map.partner_of(1), Some(0));
        assert!(map.is_on_plane(2), "nothing across the plane: it is its own reflection");
        assert!(map.is_on_plane(3));
        assert!(map.is_unmatched(4));
        assert_eq!((map.pair_count(), map.on_plane_count(), map.unmatched_count()), (1, 2, 1));
        assert_eq!(map.summary(), "X: 1 unmatched of 5 (1 pairs, 2 on centre)");
    }

    /// Two vertices on the same side never pair, however close one is to the other's reflection.
    #[test]
    fn same_side_vertices_do_not_pair() {
        let positions = vec![Vec3::new(0.001, 0.0, 0.0), Vec3::new(0.002, 0.0, 0.0)];
        let map = SymmetryMap::build(&positions, &[], Axis::X, 0.01);
        assert!(map.is_on_plane(0) && map.is_on_plane(1));
    }

    /// One half pushed well out of shape: distance alone loses the pushed region, propagation
    /// along the surface gets it back, and every pair is still the exact partner.
    #[test]
    fn propagation_pairs_what_has_drifted_past_the_tolerance() {
        let n = 24;
        let mesh = octa_sphere(n, 1.0);
        let partners = octa_partners(n, Axis::X);
        let spacing = mean_edge_length(&mesh.positions, &mesh.indices);
        let mut drifted = mesh.positions.clone();
        let centre = Vec3::new(0.6, 0.5, 0.6).normalize();
        for p in &mut drifted {
            if p.x > 0.0 {
                let bump = (-(*p - centre).length_squared() / (2.0 * 0.25 * 0.25)).exp();
                *p += p.normalize() * (spacing * 3.0 * bump);
            }
        }
        let tolerance = default_tolerance(&drifted, &mesh.indices);

        let by_distance = SymmetryMap::build(&drifted, &[], Axis::X, tolerance);
        assert!(by_distance.unmatched_count() > 100, "{}", by_distance.summary());

        let map = SymmetryMap::build(&drifted, &mesh.indices, Axis::X, tolerance);
        assert!(map.is_symmetric(), "{}", map.summary());
        assert!(map.propagated_pair_count() > 50);
        assert_matches(&map, &partners, "drifted");
    }

    /// A vertex only one side has (the halves are not tessellated alike there) stays unmatched;
    /// everything else still pairs exactly.
    #[test]
    fn a_vertex_with_no_counterpart_stays_unmatched() {
        let n = 10;
        let mut mesh = octa_sphere(n, 1.0);
        // Split one triangle well inside the +X half into three around a new vertex.
        let t = (0..mesh.triangle_count())
            .find(|&t| mesh.triangle(t).iter().all(|p| p.x > 0.3 && p.y > 0.3 && p.z > 0.3))
            .unwrap();
        let [a, b, c] = [mesh.indices[t * 3], mesh.indices[t * 3 + 1], mesh.indices[t * 3 + 2]];
        let centre = mesh.triangle(t).iter().sum::<Vec3>() / 3.0;
        let extra = mesh.vertex_count() as u32;
        mesh.positions.push(centre);
        mesh.indices[t * 3 + 2] = extra;
        mesh.indices.extend([b, c, extra, c, a, extra]);

        let map = build(&mesh, Axis::X);
        assert_eq!(map.unmatched_count(), 1, "{}", map.summary());
        assert!(map.is_unmatched(extra));
        assert!(!map.is_symmetric());
        let partners = octa_partners(n, Axis::X);
        for (i, &want) in partners.iter().enumerate() {
            assert_eq!(map.counterpart(i as u32), Some(want));
        }
    }

    /// The map must not depend on the order vertices are stored in: shuffle them and every
    /// vertex is still paired with the same position.
    #[test]
    fn the_map_is_a_property_of_the_geometry_not_the_vertex_order() {
        let (mesh, _) = icosphere(4);
        let n = mesh.vertex_count();
        let mut order: Vec<u32> = (0..n as u32).collect();
        let mut rng = Lcg::new(42);
        for i in (1..n).rev() {
            order.swap(i, ((rng.unit() * (i + 1) as f32) as usize).min(i));
        }
        // order[new] = old
        let mut new_of_old = vec![0u32; n];
        order.iter().enumerate().for_each(|(new, &old)| new_of_old[old as usize] = new as u32);
        let positions: Vec<Vec3> = order.iter().map(|&old| mesh.positions[old as usize]).collect();
        let indices: Vec<u32> = mesh.indices.iter().map(|&i| new_of_old[i as usize]).collect();

        let tolerance = default_tolerance(&positions, &indices);
        let map = SymmetryMap::build(&positions, &indices, Axis::Y, tolerance);
        let partners = exact_partners(&positions, Axis::Y).unwrap();
        assert_matches(&map, &partners, "shuffled");
    }

    /// Run with `--release -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn timing_build_map_4m() {
        let mesh = octa_sphere(1000, 1.0);
        let partners = octa_partners(1000, Axis::X);
        let clock = std::time::Instant::now();
        let tolerance = default_tolerance(&mesh.positions, &mesh.indices);
        let cloud = SymmetryMap::build(&mesh.positions, &[], Axis::X, tolerance);
        let by_distance = clock.elapsed();
        let clock = std::time::Instant::now();
        let map = build(&mesh, Axis::X);
        println!(
            "SymmetryMap: {} vertices, {} triangles: distance only {:.0} ms, with topology {:.0} ms - {}",
            mesh.vertex_count(),
            mesh.triangle_count(),
            by_distance.as_secs_f64() * 1e3,
            clock.elapsed().as_secs_f64() * 1e3,
            map.summary()
        );
        assert!(cloud.is_symmetric());
        assert_matches(&map, &partners, "4M");
    }
}
