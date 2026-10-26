//! Dynamic topology: detail added under the brush by splitting long edges,
//! and everything a mesh that grows in the middle of a stroke needs to stay
//! consistent: its adjacency, its spatial index, what the renderer is sent,
//! and the stroke's undo record. When to refine, where and how fine is the
//! brush's side of it, in sculpt/dyntopo.rs.
//!
//! SPLITS, FLIPS AND MERGES, and nothing is ever renumbered. Every vertex and
//! triangle number that existed before still names the same thing
//! afterwards, which is what keeps the rest of the app (stroke undo, the
//! mask, hidden faces, face groups) valid with no bookkeeping of its own.
//!
//! A merge (an edge collapse: one end of a short edge folded onto the other,
//! which stays where it is) does take triangles and a vertex away, so it
//! leaves them in place as tombstones. The two triangles on the edge are
//! rewritten to a point, all three corners the vertex that stayed: such a
//! triangle draws nothing, is never hit, is in no vertex's list of
//! triangles, and is dropped whenever the mesh is built again (`Mesh::new`,
//! on opening a file or after a remesh). The vertex folded away keeps its
//! number and everything kept for it, in no triangle and with no neighbours.
//! So a merge is a rewrite of the triangles round it, which the stroke's
//! undo record already keeps, and undo needs nothing new. The Unity app's
//! first attempt collapsed too and left flattened slivers; these merges are
//! refused wherever they would turn or flatten a triangle, pinch the surface
//! or move a border (see `merge_refusal`).
//!
//! Merging is Blender's "Subdivide Collapse": edges shorter than a share of
//! the detail size are merged before the long ones are split, so a big brush
//! over a finely worked area makes it coarse again, and the number of
//! triangles follows the brush both ways.
//!
//! Triangle shape is kept by two things together. Every triangle is bisected
//! only along its own longest edge (`split_or_defer`), which bounds each
//! child's angles by its parent's; and the triangles about to be split, then
//! everything the splits made, are flipped toward Delaunay, which gives the
//! bisection the best-shaped parents there are. Splitting edges in plain
//! length order instead turned a region Move or Snake Hook had stretched into
//! fans of needles lying along the stretch, which shade as streaks. The Unity
//! app measured, on a sphere stretched to three times its length, refined
//! triangles under 10 and under 20 degrees: 7.5% and 27% before, none and
//! 1.7% now.
//!
//! How the mesh takes the growth, which is where this differs from the Unity
//! app in mechanism (its mesh was arrays with spare capacity and two grids):
//!
//!   - New vertices and triangles are appended, and each refine sorts what it
//!     appended into leaves of the spatial index (bvh/grow.rs). Only the
//!     numbers it has just handed out move in that sort, never older ones.
//!   - The triangles on each vertex and its neighbours are lists that can
//!     grow in place (lists.rs), kept current split by split.
//!   - The renderer is told which triangles to send again (`take_dirty`), and
//!     the vertices follow the usual way: the new leaves are simply dirty.
//!   - Quads kept from a quad remesh describe triangles that no longer
//!     exist, so the first refine retires them, as any rebuild of the
//!     triangles does. The stroke's undo record brings them back.
//!
//! Undo is one record per stroke: growth only appends, so undoing is a cut
//! back to the old counts plus rewriting the triangles the stroke changed.
//! Undo and redo then rebuild everything derived from the triangles in one
//! pass over the mesh. That is heavier than the stroke's own bookkeeping on
//! purpose: an undo is one discrete action, and incremental bookkeeping is
//! exactly where a subtle corruption would hide.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::ops::Range;

use glam::Vec3;
use rayon::prelude::*;
use sculpt_geom::edge_key::{EdgeSet, edge_key, edge_vertices};
use sculpt_geom::tri::closest_point;

use super::bvh::{Aabb, LEAF_MAX_TRIS, Leaf, LeafRuns};
use super::{Mesh, Polys};
use crate::util::BitSet;

// Open to the rest of the crate for its checks (`assert_closed_manifold`,
// `assert_sound`): the radial symmetry tests refine under their own strokes.
#[cfg(test)]
pub(crate) mod tests;

/// A vertex masked at least this much is frozen for every brush, so detail
/// there would be triangles nothing can ever move.
const FROZEN_MASK: f32 = 0.999;

/// Normals further apart than this mean a crease: the split point stays on
/// the straight edge rather than bulging along a curve the two normals do not
/// agree on.
const CURVED_SPLIT_MIN_NORMAL_DOT: f32 = 0.25;

/// How far past 180 degrees (in radians, about 1.1 degrees) the two angles
/// facing an edge must add up to before it is flipped. The margin is what
/// stops two nearly cocircular configurations flipping back and forth.
const FLIP_DELAUNAY_MARGIN: f32 = 0.02;

/// Two faces whose normals are further apart than this (about 26 degrees)
/// meet at a crease or a sculpted ridge: flipping their shared edge would
/// re-cut the surface there rather than just re-triangulate it.
const FLIP_MIN_NORMAL_DOT: f32 = 0.9;

/// The most passes of splitting one refine makes. Each pass at least halves
/// the longest edge left, so this is never reached by a real stroke.
const MAX_PASSES: usize = 64;

/// A merge may not turn any triangle it reshapes further than this from the
/// way it faced (the cosine: 30 degrees). Our decimator allows 78. At 60, a
/// big brush run along a thin pulled shape merged it into a lumpy polygon
/// seen end on; at 30 it stays round and nearly as many merges are made
/// (5,476 triangles left against 5,454 in `--demo-dyntopo` with
/// `SCULPT_DEMO_DYNTOPO=coarse`).
const MERGE_MOST_TURN: f32 = 0.866;

/// Fewest triangles a vertex may be left with by a merge, inside the mesh.
const MERGE_FEWEST_TRIANGLES: usize = 3;

/// Two ways of folding an edge whose longest new edges are this close (as a
/// share) count as a tie, decided by where the ends are (`try_merge`).
const MERGE_TIE: f32 = 0.05;

/// Short edges are merged in order of length to this many steps of the
/// merge size, and within a step from the middle of the object outward
/// (`merge_order`).
const MERGE_ORDER_STEPS: f32 = 64.0;

/// Rewritten triangles closer together than this are sent to the renderer as
/// one run; a few untouched triangles cost less than a second write.
const UPLOAD_RUN_GAP: u32 = 64;

/// Above this many triangles under the brush, the search for the ones that
/// want splitting is shared between threads.
const PARALLEL_SCAN: usize = 16_384;

/// The last leaf takes what a refine appends while that keeps it no bigger
/// than this many times the refined region's radius.
const TAIL_REACH: f32 = 3.0;

/// What one call to [`Mesh::refine`] did.
#[derive(Clone, Copy, Default, Debug)]
pub struct RefineStats {
    pub splits: usize,
    /// Short edges merged away.
    pub merges: usize,
    pub flips: usize,
    pub vertices_added: usize,
    /// Stopped at the split budget with edges still to split: the caller
    /// should refine again next frame even if the brush has not moved.
    pub hit_budget: bool,
    /// Stopped at the triangle limit.
    pub hit_limit: bool,
}

/// An edge waiting to be split.
#[derive(Clone, Copy)]
struct Edge {
    a: u32,
    b: u32,
    length_sq: f32,
}

/// Longest first, ties by vertex pair so a refine is reproducible.
fn longest_first(x: &Edge, y: &Edge) -> Ordering {
    let pair = |e: &Edge| (e.a.min(e.b), e.a.max(e.b));
    y.length_sq.total_cmp(&x.length_sq).then_with(|| pair(x).cmp(&pair(y)))
}

/// What a pass under the brush is to make of its edges.
#[derive(Clone, Copy, Debug)]
pub struct EdgeSizes {
    /// The longest an edge may be: longer ones are split (if `split`), and
    /// no merge makes one longer.
    pub longest: f32,
    pub split: bool,
    /// Edges shorter than this are merged away; 0 merges nothing.
    pub shortest: f32,
}

/// Where a refine works and how fine: the spheres of `radius` around `sites`,
/// down to edges no longer than the square root of `max_edge_sq`, and up to
/// none shorter than the square root of `min_edge_sq`.
struct Region<'a> {
    sites: &'a [Vec3],
    radius: f32,
    radius_sq: f32,
    max_edge_sq: f32,
    min_edge_sq: f32,
}

impl Region<'_> {
    fn touches_triangle(&self, a: Vec3, b: Vec3, c: Vec3) -> bool {
        let (low, high) = (a.min(b).min(c) - self.radius, a.max(b).max(c) + self.radius);
        self.sites.iter().any(|&p| {
            p.cmpge(low).all() && p.cmple(high).all() && closest_point(p, a, b, c).distance_squared(p) <= self.radius_sq
        })
    }

    fn touches_segment(&self, a: Vec3, b: Vec3) -> bool {
        let along = b - a;
        let length_sq = along.length_squared();
        self.sites.iter().any(|&p| {
            let t = if length_sq > 0.0 { ((p - a).dot(along) / length_sq).clamp(0.0, 1.0) } else { 0.0 };
            (a + along * t).distance_squared(p) <= self.radius_sq
        })
    }
}

fn angle_between(u: Vec3, v: Vec3) -> f32 {
    u.cross(v).length().atan2(u.dot(v))
}

/// Moves the entry at `k` to place `to[k]`, for every entry.
fn permute<T: Copy>(items: &mut [T], to: &[u32]) {
    let old = items.to_vec();
    for (item, &new) in old.into_iter().zip(to) {
        items[new as usize] = item;
    }
}

/// Cuts the triangles `ids` (numbers into `centroids`) into runs of at most a
/// leaf's worth that lie together in space, reordering `ids` to match, and
/// notes where each run starts in `cuts`. `base` is where `ids` starts.
fn cut_into_leaves(ids: &mut [u32], base: u32, centroids: &[Vec3], cuts: &mut Vec<u32>) {
    if ids.len() <= LEAF_MAX_TRIS {
        cuts.push(base);
        return;
    }
    let mut spread = Aabb::EMPTY;
    for &id in ids.iter() {
        spread.grow(centroids[id as usize]);
    }
    let size = spread.size();
    let axis = if size.x >= size.y && size.x >= size.z {
        0
    } else if size.y >= size.z {
        1
    } else {
        2
    };
    let mid = ids.len() / 2;
    ids.select_nth_unstable_by(mid, |&x, &y| centroids[x as usize][axis].total_cmp(&centroids[y as usize][axis]));
    let (left, right) = ids.split_at_mut(mid);
    cut_into_leaves(left, base, centroids, cuts);
    cut_into_leaves(right, base + mid as u32, centroids, cuts);
}

/// Marks `stamps[at]` with `generation`; true if it was not marked already.
fn mark(stamps: &mut Vec<u32>, at: u32, generation: u32, room: usize) -> bool {
    let at = at as usize;
    if stamps.len() <= at {
        stamps.resize(room.max(at + 1), 0);
    }
    if stamps[at] == generation {
        return false;
    }
    stamps[at] = generation;
    true
}

/// What a stroke that grew the mesh has changed so far, for its undo record.
struct StrokeRecord {
    /// How many vertices, triangles and leaves there were before it.
    vertices: u32,
    triangles: u32,
    leaves: u32,
    /// The last leaf's runs before it: a refine may have extended it.
    tail: LeafRuns,
    /// The triangles from before the stroke that it rewrote, and their
    /// corners as they were.
    slots: Vec<u32>,
    before: Vec<[u32; 3]>,
    /// The quads the stroke retired.
    polys: Option<Polys>,
    /// The UVs the stroke retired, as the quads (see `uvs.rs`); empty for a
    /// mesh that had none.
    uvs: Vec<glam::Vec2>,
    /// The vertices from before the stroke that its splits took back (see
    /// `take_vertex`), each with what it was until then.
    reused: Vec<(u32, VertexKept)>,
    /// The triangle places from before the stroke its splits took back, and
    /// the face group each was in; none for a mesh without groups.
    reused_groups: Vec<(u32, u16)>,
}

/// What undo keeps of a vertex a split took back from a merge: what it was,
/// besides its triangles, which the stroke's rewritten places keep.
#[derive(Clone, Copy, PartialEq)]
struct VertexKept {
    position: Vec3,
    mask: f32,
    /// The two vertices it was last made between, if it was made by a split.
    parents: Option<[u32; 2]>,
}

/// A vertex a stroke's splits took back, as it was before and after.
struct Reused {
    vertex: u32,
    before: VertexKept,
    after: VertexKept,
}

/// What an undo step keeps of a stroke that grew the mesh, beside the
/// positions an ordinary stroke keeps: enough to cut the mesh back to how it
/// was, and to grow it again exactly as the stroke left it.
pub struct TopologyDelta {
    vertices: u32,
    triangles: u32,
    slots: Vec<u32>,
    slots_before: Vec<[u32; 3]>,
    slots_after: Vec<[u32; 3]>,
    new_positions: Vec<Vec3>,
    new_mask: Vec<f32>,
    new_parents: Vec<[u32; 2]>,
    new_indices: Vec<u32>,
    /// The face group of each triangle the stroke added; none for a mesh
    /// that had no groups.
    new_groups: Vec<u16>,
    /// How many leaves there were before, the last one's runs then, and the
    /// runs of that leaf and every later one after the stroke.
    leaves: u32,
    tail_before: LeafRuns,
    leaves_after: Vec<LeafRuns>,
    polys: Option<Polys>,
    /// The UVs the stroke retired, which undo puts back; empty for none.
    uvs: Vec<glam::Vec2>,
    /// The vertices from before the stroke its splits took back.
    reused: Vec<Reused>,
    /// The places from before the stroke its splits took back: each one's
    /// face group before and after.
    reused_groups: Vec<(u32, u16, u16)>,
}

impl TopologyDelta {
    /// How many vertices the mesh had before the stroke; those it added are
    /// numbered from here.
    pub fn vertices_before(&self) -> u32 {
        self.vertices
    }

    pub fn bytes(&self) -> usize {
        self.slots.len() * 28
            + self.new_positions.len() * 24
            + self.new_indices.len() * 4
            + self.new_groups.len() * 2
            + self.leaves_after.len() * 16
            + self.polys.as_ref().map_or(0, |p| (p.start.len() + p.indices.len()) * 4)
            + self.uvs.len() * 8
            + self.reused.len() * std::mem::size_of::<Reused>()
            + self.reused_groups.len() * 8
    }
}

/// What dynamic topology keeps for a mesh. Empty, and costing nothing, on a
/// mesh it has never refined.
#[derive(Default)]
pub(super) struct State {
    /// Goes up whenever triangles or vertices are added, rewritten or taken
    /// away, so the renderer can tell its copy of the faces is out of date.
    version: u64,

    // What one refine works with. Kept between calls: a stroke refines every
    // frame, and fresh buffers each time cost more than the work.
    first_vertex: u32,
    first_triangle: u32,
    edges: Vec<Edge>,
    next: Vec<Edge>,
    seen: EdgeSet,
    flip_queue: Vec<u64>,
    flip_queued: EdgeSet,
    /// Triangles from before this refine that it rewrote, and every vertex
    /// whose triangles or neighbours it changed; the stamps say which are
    /// listed already.
    changed: Vec<u32>,
    touched: Vec<u32>,
    slot_stamp: Vec<u32>,
    vertex_stamp: Vec<u32>,
    generation: u32,

    /// The undo record of the stroke under way, once it has changed anything.
    stroke: Option<StrokeRecord>,
    stroke_stamp: Vec<u32>,
    stroke_generation: u32,

    /// Rewritten triangles the renderer has not been sent, the first appended
    /// one it has not been sent (if any), and whether it needs them all.
    unsent: Vec<u32>,
    unsent_from: Option<u32>,
    resend: bool,

    /// The two ends of the edge each vertex made by a split came from. The
    /// vertices numbered from `born_from` on are the ones with an entry.
    born_from: u32,
    parents: Vec<[u32; 2]>,

    /// How many triangles merges have left as points, and how many vertices
    /// in no triangle (see the module remarks).
    dead: usize,
    orphans: usize,
    /// What one merge works with.
    fan: Vec<u32>,
    ring: Vec<u32>,
    /// The vertices this refine's merges folded away and parked.
    parked: Vec<u32>,

    /// Places merges left near the brush that this refine's splits may take
    /// (`free_places`): triangles left as points, and vertices in none. The
    /// splits take from the end.
    free_slots: Vec<u32>,
    free_vertices: Vec<u32>,
    /// The vertices this refine's splits took back, and whether the split
    /// under way took its vertex back (its triangles may then go in places).
    taken: Vec<u32>,
    into_places: bool,
    /// Where a vertex numbered before `born_from` was made from, when a
    /// split took it back from a merge (for `rest_shape`): the vertices
    /// from `born_from` on have theirs in `parents`.
    reborn: std::collections::HashMap<u32, [u32; 2]>,
}

impl State {
    pub(super) fn bytes(&self) -> usize {
        (self.slot_stamp.len() + self.vertex_stamp.len() + self.stroke_stamp.len()) * 4
            + self.parents.len() * 8
            + self.reborn.len() * 12
    }

    /// The triangles to send to the renderer again, as runs, out of
    /// `triangles` in all. Forgets them.
    pub(super) fn take_unsent(&mut self, triangles: u32) -> Vec<Range<u32>> {
        let from = self.unsent_from.take().unwrap_or(triangles).min(triangles);
        if std::mem::take(&mut self.resend) {
            self.unsent.clear();
            return if triangles > 0 { vec![0..triangles] } else { Vec::new() };
        }
        let mut runs: Vec<Range<u32>> = Vec::new();
        self.unsent.sort_unstable();
        for &t in self.unsent.iter().filter(|&&t| t < from) {
            match runs.last_mut() {
                Some(run) if t <= run.end + UPLOAD_RUN_GAP => run.end = run.end.max(t + 1),
                _ => runs.push(t..t + 1),
            }
        }
        self.unsent.clear();
        if from < triangles {
            match runs.last_mut() {
                Some(run) if from <= run.end + UPLOAD_RUN_GAP => run.end = triangles,
                _ => runs.push(from..triangles),
            }
        }
        runs
    }
}

impl Mesh {
    /// Changes whenever dynamic topology (or the undo of it) adds, rewrites
    /// or takes away triangles. The renderer keeps the value it last drew
    /// with and sends the faces again when it differs.
    pub fn topology_version(&self) -> u64 {
        self.topo.version
    }

    /// Triangles that are part of the surface: the count less those merges
    /// have left as points. What the model is said to have, and what the
    /// triangle limit counts. (`triangle_count` is the length of the lists.)
    pub fn live_triangle_count(&self) -> usize {
        self.triangle_count() - self.topo.dead
    }

    /// Whether merges have left points among the triangles, and vertices in
    /// none of them: a mesh to build again before it leaves the app.
    pub fn has_tombstones(&self) -> bool {
        self.topo.dead > 0 || self.topo.orphans > 0
    }

    /// The two vertices whose edge was split to make vertex `v`, or `None`
    /// for a vertex the mesh was built with.
    #[cfg(test)]
    pub fn split_parents(&self, v: u32) -> Option<[u32; 2]> {
        let born = v.checked_sub(self.topo.born_from)?;
        self.topo.parents.get(born as usize).copied()
    }

    /// Where every vertex sits in the mesh's resting shape, given the resting
    /// positions `base` of the vertices it was built with (Reset Shape keeps
    /// those). A vertex made by a split rests halfway between the two it was
    /// made from, so a refined mesh goes back to the surface it was refined
    /// from (give or take the corner an edge flipped across two faces cuts
    /// between them). `None` if `base` is not this mesh's.
    pub fn rest_shape<'a>(&self, base: &'a [Vec3]) -> Option<Cow<'a, [Vec3]>> {
        let count = self.positions.len();
        let reborn = &self.topo.reborn;
        if base.len() == count && reborn.is_empty() {
            return Some(Cow::Borrowed(base));
        }
        let born = if self.topo.parents.is_empty() { count } else { self.topo.born_from as usize };
        if base.len() != born || self.topo.parents.len() != count - born {
            return None;
        }
        // Worked out from their parents: every vertex a split made, and every
        // one from before that a split took back (`take_vertex`). The rest
        // rest where `base` has them.
        const TO_WORK_OUT: u8 = 0;
        const WORKING: u8 = 1;
        const KNOWN: u8 = 2;
        let mut rest = base.to_vec();
        rest.extend_from_slice(&self.positions[born..]);
        let mut state = vec![KNOWN; count];
        state[born..].fill(TO_WORK_OUT);
        for &v in reborn.keys().filter(|&&v| (v as usize) < born) {
            state[v as usize] = TO_WORK_OUT;
        }
        // A vertex can be numbered before the ones it was made from (a refine
        // sorts what it added, and a split takes back a vertex of any number),
        // so each is worked out when first asked for.
        let mut waiting = Vec::new();
        for v in 0..count {
            if state[v] != TO_WORK_OUT {
                continue;
            }
            waiting.push(v);
            while let Some(&top) = waiting.last() {
                if state[top] == KNOWN {
                    waiting.pop();
                    continue;
                }
                let Some([a, b]) = self.parents_of(top as u32).map(|p| p.map(|p| p as usize)) else {
                    state[top] = KNOWN;
                    waiting.pop();
                    continue;
                };
                state[top] = WORKING;
                match [a, b].into_iter().find(|&p| state[p] == TO_WORK_OUT) {
                    Some(parent) => waiting.push(parent),
                    None => {
                        // Both known; or one is still being worked out further
                        // down, a loop (a vertex taken back and made from one
                        // that was made from it), let be as it stands.
                        rest[top] = (rest[a] + rest[b]) * 0.5;
                        state[top] = KNOWN;
                        waiting.pop();
                    }
                }
            }
        }
        Some(Cow::Owned(rest))
    }

    // ---- refine ------------------------------------------------------------

    /// Splits every edge longer than `max_edge` in the region the spheres of
    /// `radius` around `sites` cover, longest first, until none is left or
    /// `split_budget` splits have been made, never taking the mesh past
    /// `triangle_limit`. Triangles a sphere only passes through (a small
    /// brush in the middle of a big face) count too: their longest edge is
    /// split until the pieces near the brush are small enough. Edges under
    /// the brush are flipped toward Delaunay before and after (see the module
    /// remarks).
    ///
    /// Inside a stroke (between `begin_stroke` and `end_stroke`) the growth
    /// is part of the stroke's undo record. Outside one it is not recorded.
    #[cfg(test)]
    pub fn refine(
        &mut self,
        sites: &[Vec3],
        radius: f32,
        max_edge: f32,
        split_budget: usize,
        triangle_limit: usize,
    ) -> RefineStats {
        let sizes = EdgeSizes { longest: max_edge, split: true, shortest: 0.0 };
        self.retopologize(sites, radius, sizes, split_budget, triangle_limit)
    }

    /// [`Self::refine`], with merges: first every edge under the brush
    /// shorter than `sizes.shortest` is merged away, shortest first, then
    /// (if `sizes.split`) every one longer than `sizes.longest` is split.
    /// Merges and splits together make no more than `budget` changes. At the
    /// triangle limit merges still go on (they bring the count down), and
    /// splits do not.
    pub fn retopologize(
        &mut self,
        sites: &[Vec3],
        radius: f32,
        sizes: EdgeSizes,
        budget: usize,
        triangle_limit: usize,
    ) -> RefineStats {
        let mut stats = RefineStats::default();
        let max_edge = sizes.longest;
        if sites.is_empty() || !(radius > 0.0) || !(max_edge > 0.0) || self.indices.is_empty() {
            return stats;
        }
        // A mesh with subdivision levels keeps its triangles as they are:
        // a split here would leave its levels numbering another mesh (levels.rs).
        if self.levels.is_some() {
            return stats;
        }
        let min_edge = sizes.shortest.min(max_edge);
        let region = Region {
            sites,
            radius,
            radius_sq: radius * radius,
            max_edge_sq: max_edge * max_edge,
            min_edge_sq: if min_edge > 0.0 { min_edge * min_edge } else { 0.0 },
        };

        // 0. Every edge under the brush too short to keep.
        let short = if region.min_edge_sq > 0.0 { self.short_edges(&region) } else { Vec::new() };
        // 1. Every triangle the footprint touches that is too coarse, and
        // their edges. Nearly every call finds none of either and ends here.
        let split = sizes.split && self.live_triangle_count() + 2 <= triangle_limit;
        stats.hit_limit = sizes.split && !split;
        let wanted = if split { self.triangles_to_split(&region) } else { Vec::new() };
        if short.is_empty() && wanted.is_empty() {
            return stats;
        }
        self.begin_refine();
        if !short.is_empty() {
            (stats.merges, stats.hit_budget) = self.merge_short_edges(&region, short, budget);
            if stats.hit_budget {
                return self.finish_refine(&region, stats);
            }
        }
        let split_budget = budget - stats.merges;
        // The places merges left near the brush, for the splits to take.
        if !wanted.is_empty() {
            self.free_places(&region);
        }
        for &t in &wanted {
            if self.gather_edges(&region, t, false) {
                self.queue_triangle_for_flip(t);
            }
        }

        // 2. The triangles about to be split are made Delaunay first.
        // Bisection keeps each child's angles bounded by its parent's, so the
        // best-shaped parents give the best children: on a region stretched
        // to three times its length this halved the refined triangles under
        // 20 degrees, where the flips afterwards alone could not reach them.
        stats.flips = self.process_flip_queue(&region, true);
        if stats.flips > 0 {
            self.topo.edges.clear();
            self.topo.seen.clear();
            for &t in &wanted {
                self.gather_edges(&region, t, false);
            }
            // A flip can reshape a neighbour that was not on the list.
            for k in 0..self.topo.changed.len() {
                self.gather_edges(&region, self.topo.changed[k], false);
            }
        }

        // 3. Longest first, pass after pass: each pass splits what the last
        // one queued, and queues whichever of the new edges still qualify,
        // plus every edge that had to wait for a longer one (see
        // `split_or_defer`). A pass that split nothing had only edges waiting
        // on longer ones that cannot split (a hidden or non-manifold
        // neighbour): the next pass splits them regardless.
        let mut stalled = false;
        for _ in 0..MAX_PASSES {
            if self.topo.edges.is_empty() {
                break;
            }
            let mut edges = std::mem::take(&mut self.topo.edges);
            edges.sort_unstable_by(longest_first);
            self.topo.next.clear();
            self.topo.seen.clear();
            let before = stats.splits;
            for &edge in &edges {
                if stats.splits >= split_budget {
                    stats.hit_budget = true;
                    break;
                }
                if self.live_triangle_count() + 4 > triangle_limit {
                    stats.hit_limit = true;
                    break;
                }
                // An earlier split this pass can have consumed it.
                if !self.vv.contains(edge.a as usize, edge.b) {
                    continue;
                }
                let split =
                    if stalled { self.try_split(&region, edge.a, edge.b) } else { self.split_or_defer(&region, edge) };
                stats.splits += usize::from(split);
            }
            // The list just worked through becomes the next pass's spare.
            edges.clear();
            self.topo.edges = std::mem::replace(&mut self.topo.next, edges);
            if stats.hit_budget || stats.hit_limit {
                break;
            }
            stalled = stats.splits == before;
        }
        self.topo.edges.clear();
        self.finish_refine(&region, stats)
    }

    /// The end of every refine that began: what the merges and splits made,
    /// Delaunay again, and all of it handed on.
    fn finish_refine(&mut self, region: &Region, mut stats: RefineStats) -> RefineStats {
        stats.vertices_added = self.positions.len() - self.topo.first_vertex as usize;
        let grew = stats.vertices_added > 0 || self.triangle_count() > self.topo.first_triangle as usize;
        if !self.topo.changed.is_empty() || grew || !self.topo.taken.is_empty() {
            // 4. And what the splits made, Delaunay again.
            stats.flips += self.flip_toward_delaunay(region);
            self.publish(region);
        }
        stats
    }

    fn begin_refine(&mut self) {
        let (vertices, triangles) = (self.positions.len() as u32, self.triangle_count() as u32);
        let topo = &mut self.topo;
        (topo.first_vertex, topo.first_triangle) = (vertices, triangles);
        topo.edges.clear();
        topo.next.clear();
        topo.seen.clear();
        topo.flip_queue.clear();
        topo.flip_queued.clear();
        topo.changed.clear();
        topo.touched.clear();
        topo.parked.clear();
        topo.free_slots.clear();
        topo.free_vertices.clear();
        topo.taken.clear();
        topo.generation = topo.generation.wrapping_add(1);
        if topo.generation == 0 {
            topo.slot_stamp.fill(0);
            topo.vertex_stamp.fill(0);
            topo.generation = 1;
        }
    }

    #[inline]
    fn corners(&self, t: u32) -> [u32; 3] {
        let i = t as usize * 3;
        [self.indices[i], self.indices[i + 1], self.indices[i + 2]]
    }

    #[inline]
    fn frozen(&self, a: u32, b: u32) -> bool {
        self.mask[a as usize] >= FROZEN_MASK && self.mask[b as usize] >= FROZEN_MASK
    }

    /// Whether triangle `t` is one a refine of `region` wants to split:
    /// showing, sound, with an edge longer than the detail size, and under
    /// the brush.
    fn wants_split(&self, region: &Region, t: u32) -> bool {
        if self.hidden.hides(t) {
            return false;
        }
        let [ia, ib, ic] = self.corners(t);
        if ia == ib || ib == ic || ia == ic {
            return false;
        }
        let (pa, pb, pc) = (self.positions[ia as usize], self.positions[ib as usize], self.positions[ic as usize]);
        let longest = pa.distance_squared(pb).max(pb.distance_squared(pc)).max(pc.distance_squared(pa));
        longest > region.max_edge_sq && region.touches_triangle(pa, pb, pc)
    }

    /// The triangles under the brush that want splitting, in ascending order.
    fn triangles_to_split(&self, region: &Region) -> Vec<u32> {
        let mut leaves = Vec::new();
        for &site in region.sites {
            self.bvh.leaves_in_sphere(site, region.radius, &mut leaves);
        }
        leaves.sort_unstable();
        leaves.dedup();
        let mesh = &*self;
        let triangles = move |&leaf: &u32| {
            let leaf = &mesh.bvh.leaves[leaf as usize];
            (leaf.tri_start..leaf.tri_end).filter(move |&t| mesh.wants_split(region, t))
        };
        let under: usize = leaves.iter().map(|&l| self.bvh.leaves[l as usize].tri_count()).sum();
        if under < PARALLEL_SCAN {
            leaves.iter().flat_map(triangles).collect()
        } else {
            leaves.par_iter().with_min_len(8).flat_map_iter(triangles).collect()
        }
    }

    /// Queues triangle `t`'s qualifying edges, for this pass or (`later`) the
    /// next: longer than the detail size, and either passing within the brush
    /// radius or being the longest edge of a triangle the brush touches
    /// (which is what lets a small brush inside one big triangle refine it).
    /// True if the triangle is one the refine wants to split.
    fn gather_edges(&mut self, region: &Region, t: u32, later: bool) -> bool {
        if self.hidden.hides(t) {
            return false;
        }
        let [ia, ib, ic] = self.corners(t);
        if ia == ib || ib == ic || ia == ic {
            return false;
        }
        let (pa, pb, pc) = (self.positions[ia as usize], self.positions[ib as usize], self.positions[ic as usize]);
        let (ab, bc, ca) = (pa.distance_squared(pb), pb.distance_squared(pc), pc.distance_squared(pa));
        let longest = ab.max(bc).max(ca);
        if longest <= region.max_edge_sq || !region.touches_triangle(pa, pb, pc) {
            return false;
        }
        for (a, b, from, to, length_sq) in [(ia, ib, pa, pb, ab), (ib, ic, pb, pc, bc), (ic, ia, pc, pa, ca)] {
            if length_sq <= region.max_edge_sq || self.frozen(a, b) {
                continue;
            }
            if length_sq != longest && !region.touches_segment(from, to) {
                continue;
            }
            if self.topo.seen.insert(edge_key(a, b)) {
                let list = if later { &mut self.topo.next } else { &mut self.topo.edges };
                list.push(Edge { a, b, length_sq });
            }
        }
        true
    }

    /// Longest-edge bisection proper: an edge is split only once it is the
    /// longest edge of every triangle on it. Otherwise the longer edge of the
    /// neighbour goes first (queued for the next pass, where the length sort
    /// puts it ahead) and this one waits behind it.
    ///
    /// Splitting an edge also bisects the triangle on its far side, and when
    /// that edge was not the far triangle's longest, the cut runs from its
    /// long side into its short one. Done over and over along a stretched
    /// region, that fanned needles out of every original vertex along the
    /// stretch. Waiting for the longer edge is what keeps every triangle's
    /// angles bounded by its parent's; it costs a few extra splits just
    /// outside the brush, which is also what grades the fine region into the
    /// coarse one instead of fanning it.
    fn split_or_defer(&mut self, region: &Region, edge: Edge) -> bool {
        // Nothing `try_split` could split either: hidden, or more than two
        // triangles.
        let Some((first, second)) = self.edge_triangles(edge.a, edge.b) else { return false };
        let mut deferred = self.queue_longer_edge(first, edge.length_sq);
        if let Some(second) = second {
            deferred |= self.queue_longer_edge(second, edge.length_sq);
        }
        if !deferred {
            return self.try_split(region, edge.a, edge.b);
        }
        if self.topo.seen.insert(edge_key(edge.a, edge.b)) {
            self.topo.next.push(edge);
        }
        false
    }

    /// Queues triangle `t`'s longest edge if it is longer than `length_sq`
    /// and can be split; true if it was (the caller's edge then has to wait
    /// for it).
    fn queue_longer_edge(&mut self, t: u32, length_sq: f32) -> bool {
        let [ia, ib, ic] = self.corners(t);
        let (pa, pb, pc) = (self.positions[ia as usize], self.positions[ib as usize], self.positions[ic as usize]);
        let (mut u, mut v, mut longest) = (ia, ib, pa.distance_squared(pb));
        for (a, b, d) in [(ib, ic, pb.distance_squared(pc)), (ic, ia, pc.distance_squared(pa))] {
            if d > longest {
                (u, v, longest) = (a, b, d);
            }
        }
        // Equal lengths (to rounding) make this edge one of t's longest
        // already, and keep every wait strictly shorter than what it waits
        // for, so a chain of them always ends.
        if longest <= length_sq * (1.0 + 1e-5) {
            return false;
        }
        // A longer edge that can never split must not hold this one up forever.
        if self.frozen(u, v) {
            return false;
        }
        if self.topo.seen.insert(edge_key(u, v)) {
            self.topo.next.push(Edge { a: u, b: v, length_sq: longest });
        }
        true
    }

    /// Splits edge a-b at its (curved) midpoint: the one or two triangles on
    /// it become two or four. False, touching nothing, for an edge this
    /// cannot split safely: more than two triangles on it, a hidden side,
    /// inconsistent winding, or a fold.
    fn try_split(&mut self, region: &Region, a: u32, b: u32) -> bool {
        match self.edge_triangles(a, b) {
            Some((first, Some(second))) => self.split_interior(region, first, second, a, b),
            Some((only, None)) => self.split_boundary(region, only, a, b),
            None => false,
        }
    }

    /// The one or two triangles on edge a-b. `None` if there are none or
    /// more than two, or if one of them is hidden.
    fn edge_triangles(&self, a: u32, b: u32) -> Option<(u32, Option<u32>)> {
        let mut on_edge = self.vt.row(a as usize).iter().copied().filter(|&t| self.corners(t).contains(&b));
        let (first, second) = (on_edge.next()?, on_edge.next());
        if on_edge.next().is_some() || self.hidden.hides(first) || second.is_some_and(|t| self.hidden.hides(t)) {
            return None;
        }
        Some((first, second))
    }

    /// Whether triangle `t` runs from `u` straight on to `v` in its winding.
    fn runs(&self, t: u32, u: u32, v: u32) -> bool {
        let [x, y, z] = self.corners(t);
        (x == u && y == v) || (y == u && z == v) || (z == u && x == v)
    }

    /// The corner of triangle `t` that is neither `u` nor `v`.
    fn third_corner(&self, t: u32, u: u32, v: u32) -> Option<u32> {
        self.corners(t).into_iter().find(|&w| w != u && w != v)
    }

    fn split_interior(&mut self, region: &Region, mut t1: u32, mut t2: u32, a: u32, b: u32) -> bool {
        // Orient so t1 runs a to b and t2 b to a; anything else is
        // inconsistent winding.
        if !self.runs(t1, a, b) {
            std::mem::swap(&mut t1, &mut t2);
        }
        if !self.runs(t1, a, b) || !self.runs(t2, b, a) {
            return false;
        }
        let (Some(c), Some(d)) = (self.third_corner(t1, a, b), self.third_corner(t2, b, a)) else { return false };
        if c == d {
            return false;
        }

        let m = self.add_split_vertex(a, b, self.curved_split_point(a, b));
        let t3 = self.add_triangle([m, b, c], t1);
        self.set_triangle(t1, [a, m, c]);
        let t4 = self.add_triangle([m, a, d], t2);
        self.set_triangle(t2, [b, m, d]);

        self.vv.replace(a as usize, b, m);
        self.vv.replace(b as usize, a, m);
        self.vv.add(c as usize, m);
        self.vv.add(d as usize, m);

        self.vt.replace(a as usize, t2, t4);
        self.vt.replace(b as usize, t1, t3);
        self.vt.add(c as usize, t3);
        self.vt.add(d as usize, t4);
        self.new_rows(m, &[a, c, b, d], &[t1, t3, t2, t4]);

        for v in [a, b, c, d, m] {
            self.touch(v);
        }
        for t in [t1, t3, t2, t4] {
            self.gather_edges(region, t, true);
        }
        true
    }

    /// A genuinely open edge: only one triangle to split, and the new vertex
    /// stays on the boundary, on the straight edge.
    fn split_boundary(&mut self, region: &Region, t1: u32, mut a: u32, mut b: u32) -> bool {
        if !self.runs(t1, a, b) {
            std::mem::swap(&mut a, &mut b);
        }
        if !self.runs(t1, a, b) {
            return false;
        }
        let Some(c) = self.third_corner(t1, a, b) else { return false };

        let middle = (self.positions[a as usize] + self.positions[b as usize]) * 0.5;
        let m = self.add_split_vertex(a, b, middle);
        let t3 = self.add_triangle([m, b, c], t1);
        self.set_triangle(t1, [a, m, c]);

        self.vv.replace(a as usize, b, m);
        self.vv.replace(b as usize, a, m);
        self.vv.add(c as usize, m);

        self.vt.replace(b as usize, t1, t3);
        self.vt.add(c as usize, t3);
        self.new_rows(m, &[a, c, b], &[t1, t3]);

        for v in [a, b, c, m] {
            self.touch(v);
        }
        self.gather_edges(region, t1, true);
        self.gather_edges(region, t3, true);
        true
    }

    /// The point splitting a-b, bowed out along the curve the two vertex
    /// normals describe (the PN-triangle edge midpoint). On a sphere that
    /// lands within a fourth-order term of the true surface, so refining a
    /// coarse curved area rounds it out instead of leaving the facets in
    /// place. Clamped, and straight across a crease, where the normals
    /// disagree about which curve that is.
    fn curved_split_point(&self, a: u32, b: u32) -> Vec3 {
        let (pa, pb) = (self.positions[a as usize], self.positions[b as usize]);
        let (na, nb) = (self.normals[a as usize], self.normals[b as usize]);
        let middle = (pa + pb) * 0.5;
        if na.dot(nb) < CURVED_SPLIT_MIN_NORMAL_DOT {
            return middle;
        }
        let along = pb - pa;
        let mut offset = (na * along.dot(na) - nb * along.dot(nb)) * -0.125;
        let most_sq = along.length_squared() * (0.25 * 0.25);
        let offset_sq = offset.length_squared();
        if offset_sq > most_sq {
            offset *= (most_sq / offset_sq).sqrt();
        }
        middle + offset
    }

    fn add_split_vertex(&mut self, a: u32, b: u32, position: Vec3) -> u32 {
        let m = self.positions.len() as u32;
        let (ia, ib) = (a as usize, b as usize);
        let normal = (self.normals[ia] + self.normals[ib]).normalize_or(self.normals[ia]);
        let mask = (0.5 * (self.mask[ia] + self.mask[ib])).clamp(0.0, 1.0);

        // A vertex born mid-stroke between vertices the stroke already moved
        // gets a stroke-start record of its own, interpolated from theirs.
        // Otherwise it would read its current, already raised position as
        // where the stroke found it, and every brush that measures from the
        // stroke-start surface (Clay's depth cap, Standard, Layer) would let
        // it rise a second time and leave a spike.
        let moved = self.in_stroke && (self.stamp[ia] == self.stroke_id || self.stamp[ib] == self.stroke_id);
        let (start, start_normal, stamp) = if moved {
            let between = (self.start_pos(a) + self.start_pos(b)) * 0.5;
            (between, (self.start_nrm(a) + self.start_nrm(b)).normalize_or(normal), self.stroke_id)
        } else {
            (position, normal, 0)
        };

        // A vertex a merge left in no triangle, near here, if there is one
        // to take (`free_places`): the mesh's lists stay the length they are.
        let taken = self.topo.free_vertices.pop();
        // Its triangles go in places merges left only with it: a vertex
        // appended is housed in the spatial index with the triangles
        // appended with it (`house_appended`), and needs some.
        self.topo.into_places = taken.is_some();
        if let Some(m) = taken {
            self.take_vertex(m, [a, b], position, normal, mask, [start, start_normal], stamp);
            return m;
        }
        self.positions.push(position);
        self.normals.push(normal);
        self.mask.push(mask);
        self.orig_pos.push(start);
        self.orig_nrm.push(start_normal);
        self.orig_mask.push(mask);
        self.stamp.push(stamp);
        if self.curv_on {
            self.curv.push(0.5 * (self.curv[ia] + self.curv[ib]));
            self.curv_raw.push(0.5 * (self.curv_raw[ia] + self.curv_raw[ib]));
        }
        if self.topo.parents.is_empty() {
            self.topo.born_from = m;
        }
        self.topo.parents.push([a, b]);
        // Each sculpt layer's offset there, as the mask above: between the two ends (layers.rs).
        if let Some(layers) = &mut self.layers {
            layers.split(a, b);
        }
        m
    }

    /// The neighbours and triangles of `m`, a vertex a split has just made:
    /// lists of its own after the last, or, for one a split took back, the
    /// empty lists it had.
    fn new_rows(&mut self, m: u32, neighbours: &[u32], triangles: &[u32]) {
        if m as usize == self.vv.len() {
            self.vv.push_row(neighbours);
            self.vt.push_row(triangles);
            return;
        }
        for &w in neighbours {
            self.vv.add(m as usize, w);
        }
        for &t in triangles {
            self.vt.add(m as usize, t);
        }
    }

    /// [`Self::add_split_vertex`] into vertex `m`, which a merge left in no
    /// triangle: everything kept for it is made over as for a new vertex
    /// between `parents`, and inside a stroke what it was goes in the
    /// stroke's undo record first. (Only a mesh with no sculpt layers, no
    /// paint and nothing hidden gives vertices back: those keep more per
    /// vertex, in records of their own; see `free_places`.)
    #[allow(clippy::too_many_arguments)]
    fn take_vertex(
        &mut self,
        m: u32,
        parents: [u32; 2],
        position: Vec3,
        normal: Vec3,
        mask: f32,
        [start, start_normal]: [Vec3; 2],
        stamp: u32,
    ) {
        let i = m as usize;
        if self.in_stroke {
            let kept = self.vertex_kept(m);
            self.open_stroke_record();
            let record = self.topo.stroke.as_mut().expect("the stroke's record is open");
            if m < record.vertices {
                record.reused.push((m, kept));
            }
        }
        let [ia, ib] = parents.map(|p| p as usize);
        self.positions[i] = position;
        self.normals[i] = normal;
        self.mask[i] = mask;
        self.orig_pos[i] = start;
        self.orig_nrm[i] = start_normal;
        self.orig_mask[i] = mask;
        self.stamp[i] = stamp;
        if self.curv_on {
            self.curv[i] = 0.5 * (self.curv[ia] + self.curv[ib]);
            self.curv_raw[i] = 0.5 * (self.curv_raw[ia] + self.curv_raw[ib]);
        }
        self.set_parents(m, Some(parents));
        self.topo.orphans -= 1;
        self.topo.taken.push(m);
    }

    /// Appends a triangle split from triangle `from`, whose face group it is
    /// in; or puts it in a place a merge left, if there is one near to take.
    fn add_triangle(&mut self, corners: [u32; 3], from: u32) -> u32 {
        if let Some(t) = self.topo.into_places.then(|| self.topo.free_slots.pop()).flatten() {
            self.take_slot(t, corners, from);
            return t;
        }
        self.groups.born(from);
        self.indices.extend_from_slice(&corners);
        (self.indices.len() / 3 - 1) as u32
    }

    /// [`Self::add_triangle`] into place `t`, which a merge left as a point.
    /// The rewrite goes in the undo record as any rewrite does; the face
    /// group the place had goes in with it.
    fn take_slot(&mut self, t: u32, corners: [u32; 3], from: u32) {
        let group = self.groups.ids.get(from as usize).copied();
        if let (Some(group), Some(&was)) = (group, self.groups.ids.get(t as usize)) {
            if self.in_stroke {
                self.open_stroke_record();
                let record = self.topo.stroke.as_mut().expect("the stroke's record is open");
                if t < record.triangles {
                    record.reused_groups.push((t, was));
                }
            }
            self.groups.ids[t as usize] = group;
        }
        self.set_triangle(t, corners);
        self.topo.dead -= 1;
    }

    /// What undo keeps of vertex `v` beside its triangles.
    fn vertex_kept(&self, v: u32) -> VertexKept {
        VertexKept { position: self.positions[v as usize], mask: self.mask[v as usize], parents: self.parents_of(v) }
    }

    /// The two vertices vertex `v` was last made between by a split, if it was.
    fn parents_of(&self, v: u32) -> Option<[u32; 2]> {
        match v.checked_sub(self.topo.born_from).map(|k| k as usize) {
            Some(k) if k < self.topo.parents.len() => Some(self.topo.parents[k]),
            _ => self.topo.reborn.get(&v).copied(),
        }
    }

    /// Records what vertex `v` was made between (or that it was not).
    fn set_parents(&mut self, v: u32, parents: Option<[u32; 2]>) {
        match (v.checked_sub(self.topo.born_from).map(|k| k as usize), parents) {
            (Some(k), Some(parents)) if k < self.topo.parents.len() => self.topo.parents[k] = parents,
            (_, Some(parents)) => {
                self.topo.reborn.insert(v, parents);
            }
            (_, None) => {
                self.topo.reborn.remove(&v);
            }
        }
    }

    /// Lays out the places merges left under the brush for this refine's
    /// splits to take: triangles left as points that are not hidden, and
    /// vertices in no triangle. None where something else is kept per vertex
    /// that a record of its own would have to carry (sculpt layers, paint),
    /// or where faces are hidden: a split takes triangle places only with a
    /// vertex (`add_split_vertex`). Only places from before this stroke: one the stroke has
    /// already rewritten or moved is in its undo record already, and taken
    /// again it would be recorded twice over. Taken lowest first.
    fn free_places(&mut self, region: &Region) {
        self.topo.free_slots.clear();
        self.topo.free_vertices.clear();
        let kept_elsewhere = self.layers.is_some() || self.paint.is_some() || self.hidden.keeps_off();
        if self.topo.dead == 0 || self.topo.orphans == 0 || kept_elsewhere {
            return;
        }
        let mut leaves = Vec::new();
        for &site in region.sites {
            self.bvh.leaves_in_sphere(site, region.radius, &mut leaves);
        }
        leaves.sort_unstable();
        leaves.dedup();
        let topo = &self.topo;
        let recorded = |t: u32| {
            self.in_stroke && topo.stroke.is_some() && topo.stroke_stamp.get(t as usize) == Some(&topo.stroke_generation)
        };
        let (mut slots, mut vertices) = (Vec::new(), Vec::new());
        for &leaf in &leaves {
            let leaf = &self.bvh.leaves[leaf as usize];
            for t in (leaf.tri_start..leaf.tri_end).filter(|&t| t < topo.first_triangle) {
                let [a, b, c] = self.corners(t);
                if a == b && b == c && !self.hidden.hides(t) && !recorded(t) {
                    slots.push(t);
                }
            }
            {
                let moved = |v: u32| self.in_stroke && self.stamp[v as usize] == self.stroke_id;
                for v in (leaf.vert_start..leaf.vert_end).filter(|&v| v < topo.first_vertex) {
                    if self.vt.row(v as usize).is_empty() && !moved(v) {
                        vertices.push(v);
                    }
                }
            }
        }
        slots.reverse();
        vertices.reverse();
        (self.topo.free_slots, self.topo.free_vertices) = (slots, vertices);
    }

    fn set_triangle(&mut self, t: u32, corners: [u32; 3]) {
        self.record_before_change(t);
        self.indices[t as usize * 3..t as usize * 3 + 3].copy_from_slice(&corners);
    }

    /// Notes triangle `t`'s corners before a refine rewrites it: once per
    /// refine for the index and the renderer, and once per stroke for undo.
    /// A triangle the refine itself appended needs neither.
    fn record_before_change(&mut self, t: u32) {
        if t >= self.topo.first_triangle {
            return;
        }
        let room = self.indices.len() / 3;
        if mark(&mut self.topo.slot_stamp, t, self.topo.generation, room) {
            self.topo.changed.push(t);
        }
        if !self.in_stroke {
            return;
        }
        let before = self.corners(t);
        self.open_stroke_record();
        let topo = &mut self.topo;
        let record = topo.stroke.as_mut().expect("the stroke's record is open");
        // Only triangles from before the stroke need their old corners kept
        // (the cut back to the old counts removes the rest), and only once.
        if t < record.triangles && mark(&mut topo.stroke_stamp, t, topo.stroke_generation, room) {
            record.slots.push(t);
            record.before.push(before);
        }
    }

    /// Opens the undo record of the stroke under way, if this is the first
    /// change the stroke makes. The counts it opens with are those of the
    /// mesh before the refine now running, which is how the stroke found it.
    fn open_stroke_record(&mut self) {
        if self.topo.stroke.is_some() {
            return;
        }
        let topo = &mut self.topo;
        topo.stroke_generation = topo.stroke_generation.wrapping_add(1);
        if topo.stroke_generation == 0 {
            topo.stroke_stamp.fill(0);
            topo.stroke_generation = 1;
        }
        topo.stroke = Some(StrokeRecord {
            vertices: topo.first_vertex,
            triangles: topo.first_triangle,
            leaves: self.bvh.leaves.len() as u32,
            tail: self.bvh.leaves.last().map_or([0; 4], Leaf::runs),
            slots: Vec::new(),
            before: Vec::new(),
            polys: None,
            uvs: Vec::new(),
            reused: Vec::new(),
            reused_groups: Vec::new(),
        });
    }

    /// Adds `v` to this refine's touched set: its normal is worked out again
    /// afterwards, and its lists are renumbered if the refine sorts.
    fn touch(&mut self, v: u32) {
        let room = self.positions.len();
        if mark(&mut self.topo.vertex_stamp, v, self.topo.generation, room) {
            self.topo.touched.push(v);
        }
    }

    // ---- merges ------------------------------------------------------------

    /// Every edge of a showing triangle under the brush shorter than the
    /// region's shortest, shortest first, each once. An edge frozen by the
    /// mask at both ends is left out.
    fn short_edges(&self, region: &Region) -> Vec<Edge> {
        let mut leaves = Vec::new();
        for &site in region.sites {
            self.bvh.leaves_in_sphere(site, region.radius, &mut leaves);
        }
        leaves.sort_unstable();
        leaves.dedup();
        let mesh = self;
        let edges = move |&leaf: &u32| {
            let leaf = &mesh.bvh.leaves[leaf as usize];
            (leaf.tri_start..leaf.tri_end).flat_map(move |t| mesh.short_edges_of(region, t)).flatten()
        };
        let under: usize = leaves.iter().map(|&l| self.bvh.leaves[l as usize].tri_count()).sum();
        let mut edges: Vec<Edge> = if under < PARALLEL_SCAN {
            leaves.iter().flat_map(edges).collect()
        } else {
            leaves.par_iter().with_min_len(8).flat_map_iter(edges).collect()
        };
        self.merge_order(region, &mut edges);
        edges
    }

    /// Puts short edges in the order they are merged, each once: shortest
    /// first by steps of the merge size, and within a step from the object's
    /// middle outward, then by number. Sorted on the exact length, the last
    /// digits decided between an edge and its mirror twin, and the two sides
    /// of a mirrored stroke merged in different orders; the steps and the
    /// distance from the middle are the same for both.
    fn merge_order(&self, region: &Region, edges: &mut Vec<Edge>) {
        edges.sort_unstable_by_key(|e| edge_key(e.a, e.b));
        edges.dedup_by_key(|e| edge_key(e.a, e.b));
        let key = |e: &Edge| {
            let step = (e.length_sq / region.min_edge_sq * MERGE_ORDER_STEPS) as u32;
            let middle = (self.positions[e.a as usize] + self.positions[e.b as usize]).length_squared();
            (step, middle, edge_key(e.a, e.b))
        };
        edges.sort_unstable_by(|x, y| {
            let ((sx, mx, kx), (sy, my, ky)) = (key(x), key(y));
            sx.cmp(&sy).then(mx.total_cmp(&my)).then(kx.cmp(&ky))
        });
    }

    /// The edges of triangle `t` a merge under the brush wants.
    fn short_edges_of(&self, region: &Region, t: u32) -> [Option<Edge>; 3] {
        let [ia, ib, ic] = self.corners(t);
        if self.hidden.hides(t) || ia == ib || ib == ic || ia == ic {
            return [None; 3];
        }
        [(ia, ib), (ib, ic), (ic, ia)].map(|(a, b)| self.short_edge(region, a, b))
    }

    /// The short edges round vertex `v`, for the next pass of merges.
    fn short_edges_round(&self, region: &Region, v: u32, out: &mut Vec<Edge>) {
        out.extend(self.vv.row(v as usize).iter().filter_map(|&w| self.short_edge(region, v, w)));
    }

    /// Edge a-b, if a merge under the brush wants it: shorter than the
    /// region's shortest, under the brush, not frozen by the mask, and on
    /// surface the stroke under way has not yet moved.
    ///
    /// That last is what keeps a stroke's own shape steady. Clay, Standard
    /// and the other brushes that build from the surface as the stroke found
    /// it measure from each vertex's place and its triangles' then; a merge
    /// in ground the stroke has already shaped changes that reference under
    /// its feet, and differently in each copy of a mirrored or radial stroke
    /// (one vertex came out 3.9% of a Clay stroke off its five partners,
    /// against 0.7% with no merges). The merges run just ahead of each dab,
    /// so the ground a stroke reaches is merged before it is moved; what the
    /// stroke itself made short waits for the next stroke.
    fn short_edge(&self, region: &Region, a: u32, b: u32) -> Option<Edge> {
        let moved = |v: u32| self.in_stroke && self.stamp[v as usize] == self.stroke_id;
        let (pa, pb) = (self.positions[a as usize], self.positions[b as usize]);
        let length_sq = pa.distance_squared(pb);
        let wanted = length_sq < region.min_edge_sq
            && !self.frozen(a, b)
            && !moved(a)
            && !moved(b)
            && region.touches_segment(pa, pb);
        wanted.then_some(Edge { a, b, length_sq })
    }

    /// Merges `edges` (shortest first) and, pass after pass, the short edges
    /// that each merge leaves round the vertex that stayed, until none is
    /// left or `budget` merges have been made. Returns how many were, and
    /// whether the budget is what stopped them.
    fn merge_short_edges(&mut self, region: &Region, mut edges: Vec<Edge>, budget: usize) -> (usize, bool) {
        let mut merges = 0;
        for _ in 0..MAX_PASSES {
            if edges.is_empty() {
                break;
            }
            let mut next = Vec::new();
            for &edge in &edges {
                if merges >= budget {
                    return (merges, true);
                }
                // An earlier merge can have taken it, or made it longer.
                if !self.vv.contains(edge.a as usize, edge.b) {
                    continue;
                }
                let (pa, pb) = (self.positions[edge.a as usize], self.positions[edge.b as usize]);
                if pa.distance_squared(pb) >= region.min_edge_sq {
                    continue;
                }
                if let Some(kept) = self.try_merge(region, edge.a, edge.b) {
                    merges += 1;
                    self.short_edges_round(region, kept, &mut next);
                }
            }
            self.merge_order(region, &mut next);
            edges = next;
        }
        (merges, false)
    }

    /// Merges edge a-b if one end can be folded onto the other without harm
    /// (see [`Self::merge_cost`]), folding away whichever leaves the shorter
    /// longest edge. Returns the vertex that stays. Only an edge inside the
    /// surface, with a triangle on each side: a border keeps its vertices.
    fn try_merge(&mut self, region: &Region, a: u32, b: u32) -> Option<u32> {
        let (mut t1, mut t2) = match self.edge_triangles(a, b)? {
            (first, Some(second)) => (first, second),
            (_, None) => return None,
        };
        if !self.runs(t1, a, b) {
            std::mem::swap(&mut t1, &mut t2);
        }
        if !self.runs(t1, a, b) || !self.runs(t2, b, a) {
            return None;
        }
        let (c, d) = (self.third_corner(t1, a, b)?, self.third_corner(t2, b, a)?);
        if c == d {
            return None;
        }
        // Where the mask is much stronger at one end, that end stays, and
        // with it the edge of the mask.
        let may_fold = |keep: u32, gone: u32| self.mask[gone as usize] <= self.mask[keep as usize] + 0.5;
        let folds = [(a, b), (b, a)]
            .map(|(keep, gone)| if may_fold(keep, gone) { self.merge_cost(region, keep, gone, [c, d]) } else { None });
        // Nearly the same either way, the end further from the object's
        // middle stays: a choice its mirror image and its radial repeats make
        // alike, where the last digits of two lengths would not, and one
        // that keeps the outermost point of a curve.
        let outer = |v: u32| self.positions[v as usize].length_squared();
        let (keep, gone) = match folds {
            [Some(x), Some(y)] if (x - y).abs() <= MERGE_TIE * x.max(y) => {
                if outer(b) > outer(a) { (b, a) } else { (a, b) }
            }
            [Some(x), Some(y)] if y < x => (b, a),
            [Some(_), _] => (a, b),
            [None, Some(_)] => (b, a),
            [None, None] => return None,
        };
        self.merge(keep, gone, [t1, t2], [c, d]);
        Some(keep)
    }

    /// The longest edge (squared) folding `gone` onto `keep` would leave
    /// round `keep`, or `None` if the fold would harm the surface:
    ///
    ///   - `gone` frozen by the mask, on a hidden face, or on a border (of
    ///     the mesh, or between two face groups): it stays, and so does the
    ///     border's shape.
    ///   - The link condition: the two ends share no neighbour but the
    ///     vertices across their edge, `across`. A third would be a ring of
    ///     three edges with no triangle in it, a neck, which the fold would
    ///     pinch into two triangles back to back.
    ///   - Every vertex keeps at least three triangles.
    ///   - No triangle turns over, or further than [`MERGE_MOST_TURN`], or
    ///     flattens to nothing.
    ///   - No edge longer than the detail size: a merge that made one, the
    ///     next split would undo.
    fn merge_cost(&self, region: &Region, keep: u32, gone: u32, across: [u32; 2]) -> Option<f32> {
        let fan = self.vt.row(gone as usize);
        if self.mask[gone as usize] >= FROZEN_MASK || fan.iter().any(|&t| self.hidden.hides(t)) || self.on_border(gone) {
            return None;
        }
        let ring = self.vv.row(gone as usize);
        let common = ring.iter().filter(|&&w| w != keep && self.vv.contains(keep as usize, w)).count();
        if common != 2 {
            return None;
        }
        if across.iter().any(|&w| self.vt.row(w as usize).len() <= MERGE_FEWEST_TRIANGLES) {
            return None;
        }
        if self.vt.row(keep as usize).len() + fan.len() < MERGE_FEWEST_TRIANGLES + 4 {
            return None;
        }
        let at = self.positions[keep as usize];
        for &t in fan {
            let corners = self.corners(t);
            if corners.contains(&keep) {
                continue;
            }
            let was = corners.map(|w| self.positions[w as usize]);
            let now = corners.map(|w| if w == gone { at } else { self.positions[w as usize] });
            let (before, after) = ((was[1] - was[0]).cross(was[2] - was[0]), (now[1] - now[0]).cross(now[2] - now[0]));
            let (size_before, size_after) = (before.length(), after.length());
            // A triangle with no area to begin with has no way to turn.
            if size_before > 0.0 && !(before.dot(after) > MERGE_MOST_TURN * size_before * size_after) {
                return None;
            }
        }
        let longest = ring
            .iter()
            .filter(|&&w| w != keep)
            .map(|&w| at.distance_squared(self.positions[w as usize]))
            .fold(0.0, f32::max);
        (longest <= region.max_edge_sq).then_some(longest)
    }

    /// Whether vertex `v` is on a border: an edge of it with a triangle on
    /// one side only (or more than two), or two of its triangles in
    /// different face groups. A vertex in no triangle counts as one.
    fn on_border(&self, v: u32) -> bool {
        let fan = self.vt.row(v as usize);
        let Some(&first) = fan.first() else { return true };
        if fan.iter().any(|&t| self.groups.border(first, t)) {
            return true;
        }
        let shared = |w: u32| fan.iter().filter(|&&t| self.corners(t).contains(&w)).count();
        self.vv.row(v as usize).iter().any(|&w| shared(w) != 2)
    }

    /// Folds `gone` onto `keep`, which stays where it is. The two triangles
    /// on their edge, `edge`, become points (`across` holds the vertex
    /// across the edge in each), and the rest of `gone`'s become `keep`'s.
    /// `gone` is left in no triangle, with every value kept for it.
    fn merge(&mut self, keep: u32, gone: u32, edge: [u32; 2], across: [u32; 2]) {
        let mut fan = std::mem::take(&mut self.topo.fan);
        fan.clear();
        fan.extend(self.vt.row(gone as usize).iter().copied().filter(|t| !edge.contains(t)));
        for t in edge {
            self.set_triangle(t, [keep; 3]);
        }
        self.topo.dead += 2;
        self.topo.orphans += 1;
        for &t in &fan {
            let corners = self.corners(t).map(|w| if w == gone { keep } else { w });
            self.set_triangle(t, corners);
            self.vt.add(keep as usize, t);
        }
        for (t, w) in edge.into_iter().zip(across) {
            self.vt.remove(keep as usize, t);
            self.vt.remove(w as usize, t);
        }
        self.vt.clear(gone as usize);

        let mut ring = std::mem::take(&mut self.topo.ring);
        ring.clear();
        ring.extend_from_slice(self.vv.row(gone as usize));
        for &w in &ring {
            if w == keep {
                continue;
            }
            if across.contains(&w) {
                self.vv.remove(w as usize, gone);
            } else {
                self.vv.replace(w as usize, gone, keep);
                self.vv.add(keep as usize, w);
            }
            self.touch(w);
        }
        self.vv.remove(keep as usize, gone);
        self.vv.clear(gone as usize);
        self.touch(keep);
        (self.topo.fan, self.topo.ring) = (fan, ring);
        self.park(gone, keep);
    }

    /// Puts vertex `v`, which a merge has left in no triangle, where vertex
    /// `on` is: on the surface, so that nothing that measures, frames or
    /// centres the model by its points finds one standing off it. Inside a
    /// stroke the move is the stroke's to undo, as any brush's is.
    fn park(&mut self, v: u32, on: u32) {
        let i = v as usize;
        if self.in_stroke && self.stamp[i] != self.stroke_id {
            self.stamp[i] = self.stroke_id;
            self.orig_pos[i] = self.positions[i];
            self.orig_nrm[i] = self.normals[i];
            self.orig_mask[i] = self.mask[i];
            self.touched.push(v);
        }
        self.positions[i] = self.positions[on as usize];
        // In no triangle, it faces nowhere (`Mesh::vertex_normal`).
        self.normals[i] = Vec3::ZERO;
        self.topo.parked.push(v);
    }

    // ---- flips -------------------------------------------------------------

    /// Flips every edge of what this refine created or rewrote that is not
    /// locally Delaunay (the two angles facing it add up to more than 180
    /// degrees) and then the edges around each flip in turn, until none is
    /// left. On a flat patch that maximizes the smallest angle, which
    /// bisection alone never improves. Returns the number of flips.
    fn flip_toward_delaunay(&mut self, region: &Region) -> usize {
        self.topo.flip_queue.clear();
        self.topo.flip_queued.clear();
        for t in self.topo.first_triangle..self.triangle_count() as u32 {
            self.queue_triangle_for_flip(t);
        }
        // The count as it is now: the flips add to this list.
        for k in 0..self.topo.changed.len() {
            self.queue_triangle_for_flip(self.topo.changed[k]);
        }
        self.process_flip_queue(region, false)
    }

    /// Works through the flip queue, which each flip extends with the four
    /// edges around it, and leaves it empty.
    fn process_flip_queue(&mut self, region: &Region, before_splits: bool) -> usize {
        // Delaunay flipping ends by itself on a flat patch; a curved one
        // gets a ceiling.
        let budget = self.topo.flip_queue.len() * 4 + 64;
        let mut flips = 0;
        let mut at = 0;
        while at < self.topo.flip_queue.len() && flips < budget {
            let key = self.topo.flip_queue[at];
            at += 1;
            self.topo.flip_queued.remove(&key);
            let (a, b) = edge_vertices(key);
            flips += usize::from(self.try_flip(region, a, b, before_splits));
        }
        self.topo.flip_queue.clear();
        self.topo.flip_queued.clear();
        flips
    }

    fn queue_triangle_for_flip(&mut self, t: u32) {
        if self.hidden.hides(t) {
            return;
        }
        let [a, b, c] = self.corners(t);
        if a == b || b == c || a == c {
            return;
        }
        for (u, v) in [(a, b), (b, c), (c, a)] {
            self.queue_edge_for_flip(u, v);
        }
    }

    fn queue_edge_for_flip(&mut self, a: u32, b: u32) {
        let key = edge_key(a, b);
        if self.topo.flip_queued.insert(key) {
            self.topo.flip_queue.push(key);
        }
    }

    /// Replaces edge a-b, shared by t1 (a, b, c) and t2 (b, a, d), with c-d:
    /// t1 becomes (a, d, c) and t2 (d, b, c). Both keep the quad's outer
    /// edges in the direction they already ran, so winding is preserved by
    /// construction. False, touching nothing, for an edge that is already
    /// Delaunay or that cannot be flipped without harm.
    fn try_flip(&mut self, region: &Region, a: u32, b: u32, before_splits: bool) -> bool {
        // An earlier flip may have removed it.
        if !self.vv.contains(a as usize, b) {
            return false;
        }
        let Some((mut t1, Some(mut t2))) = self.edge_triangles(a, b) else { return false };
        if !self.runs(t1, a, b) {
            std::mem::swap(&mut t1, &mut t2);
        }
        if !self.runs(t1, a, b) || !self.runs(t2, b, a) {
            return false;
        }
        let (Some(c), Some(d)) = (self.third_corner(t1, a, b), self.third_corner(t2, b, a)) else { return false };
        // c-d already an edge: the flip would give it a third and fourth triangle.
        if c == d || self.vv.contains(c as usize, d) {
            return false;
        }
        // Frozen by the mask, the same rule the splits follow.
        if self.frozen(a, b) {
            return false;
        }
        // The border of two face groups stays where it is: a flip would hand
        // a sliver of each to the other.
        if self.groups.border(t1, t2) {
            return false;
        }

        let [pa, pb, pc, pd] = [a, b, c, d].map(|v| self.positions[v as usize]);
        if angle_between(pa - pc, pb - pc) + angle_between(pa - pd, pb - pd)
            <= std::f32::consts::PI + FLIP_DELAUNAY_MARGIN
        {
            return false;
        }
        // Under the brush only. Each flip queues the edges around it, and on
        // a surface that was never Delaunay (anything Move has stretched)
        // that chain would otherwise run on across parts of the model the
        // stroke never went near.
        if !region.touches_segment(pa, pb) {
            return false;
        }

        // Only across a (nearly) flat pair, and only if neither new triangle
        // folds over.
        let (n1, n2) = ((pb - pa).cross(pc - pa), (pa - pb).cross(pd - pb));
        let (l1, l2) = (n1.length(), n2.length());
        if l1 > 0.0 && l2 > 0.0 && n1.dot(n2) < FLIP_MIN_NORMAL_DOT * l1 * l2 {
            return false;
        }
        let reference = n1.normalize_or_zero() + n2.normalize_or_zero();
        let (nx, ny) = ((pd - pa).cross(pc - pa), (pb - pd).cross(pc - pd));
        let (lx, ly) = (nx.length(), ny.length());
        if !(lx > 0.0) || !(ly > 0.0) {
            return false;
        }
        if nx.dot(reference) <= 0.0 || ny.dot(reference) <= 0.0 || nx.dot(ny) < FLIP_MIN_NORMAL_DOT * lx * ly {
            return false;
        }

        // Never lengthen past the detail size. Before the splits, an edge no
        // longer than the one it replaces is fine (they will split it
        // anyway); after them, a new edge the refine would have split has to
        // be outside the brush too, or the refine is left unconverged.
        let across = pc.distance_squared(pd);
        if across > region.max_edge_sq
            && (across > pa.distance_squared(pb) || (!before_splits && region.touches_segment(pc, pd)))
        {
            return false;
        }

        self.set_triangle(t1, [a, d, c]);
        self.set_triangle(t2, [d, b, c]);

        self.vv.remove(a as usize, b);
        self.vv.remove(b as usize, a);
        self.vv.add(c as usize, d);
        self.vv.add(d as usize, c);
        self.vt.remove(b as usize, t1);
        self.vt.remove(a as usize, t2);
        self.vt.add(d as usize, t1);
        self.vt.add(c as usize, t2);

        for v in [a, b, c, d] {
            self.touch(v);
        }
        for (u, v) in [(a, d), (d, b), (b, c), (c, a)] {
            self.queue_edge_for_flip(u, v);
        }
        true
    }

    // ---- publishing --------------------------------------------------------

    /// Pushes a finished refine into everything derived from the triangles,
    /// at the cost of the footprint rather than of the mesh. The mask and the
    /// hidden faces are left as they are: every number that existed still
    /// names the same vertex and triangle.
    fn publish(&mut self, region: &Region) {
        self.topo.version += 1;
        if self.in_stroke {
            self.open_stroke_record();
        }
        // Quad faces describe triangles that no longer exist. The stroke's
        // undo record keeps them, so undo brings them back.
        if let Some(polys) = self.polys.take() {
            if let Some(record) = &mut self.topo.stroke {
                record.polys = Some(polys);
            }
        }
        // So do UVs (uvs.rs), kept the same way.
        if self.has_uvs() {
            let uvs = self.clear_uvs();
            if let Some(record) = &mut self.topo.stroke {
                record.uvs = uvs;
            }
        }

        self.bvh.loosen();
        let housed = self.house_appended(region.radius);

        // Triangles from before the refine that it rewrote stay in their
        // leaves, which now reach as far as their new corners do and have
        // the leaves of those corners for neighbours.
        let mut widened = Vec::with_capacity(self.topo.changed.len());
        for k in 0..self.topo.changed.len() {
            let t = self.topo.changed[k];
            let leaf = self.bvh.leaf_of_triangle(t);
            let mut around = Aabb::EMPTY;
            for v in self.corners(t) {
                self.reach_of(v, &mut around);
                let owner = self.bvh.vert_leaf[v as usize];
                self.bvh.link(leaf, owner);
            }
            self.bvh.enclose(leaf, around);
            widened.push(leaf);
        }
        // A vertex merges parked, or a split took back, has moved: its leaf
        // holds it, and the renderer is sent where it went and what it is.
        for k in 0..self.topo.parked.len() + self.topo.taken.len() {
            let v = if k < self.topo.parked.len() { self.topo.parked[k] } else { self.topo.taken[k - self.topo.parked.len()] };
            let leaf = self.bvh.vert_leaf[v as usize];
            let mut around = Aabb::EMPTY;
            self.reach_of(v, &mut around);
            self.bvh.enclose(leaf, around);
            self.dirty_pos.set(leaf as usize);
            self.dirty_nrm.set(leaf as usize);
            self.dirty_mask.set(leaf as usize);
            widened.push(leaf);
        }
        for leaf in housed.clone() {
            let triangles = self.bvh.leaves[leaf as usize].tri_range();
            for i in triangles.start * 3..triangles.end * 3 {
                let owner = self.bvh.vert_leaf[self.indices[i] as usize];
                self.bvh.link(leaf, owner);
            }
        }

        // Everything kept per leaf has to know the new leaves.
        let leaves = self.bvh.leaves.len();
        for set in [
            &mut self.stroke_leaves,
            &mut self.pending_normals,
            &mut self.dirty_pos,
            &mut self.dirty_nrm,
            &mut self.dirty_mask,
            &mut self.dirty_curv,
        ] {
            set.grow(leaves);
        }
        // The new vertices have never been sent to the renderer.
        for leaf in housed.start as usize..housed.end as usize {
            self.dirty_pos.set(leaf);
            self.dirty_nrm.set(leaf);
            self.dirty_mask.set(leaf);
            self.dirty_curv.set(leaf);
            self.pending_normals.set(leaf);
            widened.push(leaf as u32);
        }
        // Bounds grown here are made exact again when the stroke ends.
        if self.in_stroke {
            for &leaf in &widened {
                self.stroke_leaves.set(leaf as usize);
            }
        }
        // Every vertex whose triangles changed needs its normal again.
        for k in 0..self.topo.touched.len() {
            let leaf = self.bvh.vert_leaf[self.topo.touched[k] as usize];
            if self.pending_normals.get(leaf as usize) {
                continue;
            }
            self.pending_normals.set(leaf as usize);
            // Curvature is blurred across neighbours, so theirs follows.
            if self.curv_on {
                for &other in self.bvh.leaf_neighbours(leaf) {
                    self.pending_normals.set(other as usize);
                }
            }
        }

        // What was added shows, like the faces it was split from.
        self.hidden.resize(self.indices.len() / 3, self.positions.len());

        let topo = &mut self.topo;
        if self.indices.len() as u32 / 3 > topo.first_triangle {
            topo.unsent_from = Some(topo.unsent_from.map_or(topo.first_triangle, |from| from.min(topo.first_triangle)));
        }
        topo.unsent.extend_from_slice(&topo.changed);
        topo.changed.clear();
        topo.touched.clear();
    }

    /// Widens `around` to take in vertex `v`, both where it is and where it
    /// was when the stroke began: during a stroke the spatial index has to
    /// enclose both surfaces.
    fn reach_of(&self, v: u32, around: &mut Aabb) {
        around.grow(self.positions[v as usize]);
        around.grow(self.start_pos(v));
    }

    /// The box around a leaf-to-be: its triangles' corners and its vertices.
    fn runs_bounds(&self, [tri_start, tri_end, vert_start, vert_end]: LeafRuns) -> Aabb {
        let mut around = Aabb::EMPTY;
        for &v in &self.indices[tri_start as usize * 3..tri_end as usize * 3] {
            self.reach_of(v, &mut around);
        }
        for v in vert_start..vert_end {
            self.reach_of(v, &mut around);
        }
        around
    }

    /// Gives the triangles and vertices this refine appended their place in
    /// the spatial index, and returns the leaves that hold them. A handful go
    /// into the last leaf while it has room and they are near it, so a
    /// stroke that adds a few triangles a frame does not leave a trail of
    /// tiny leaves. Otherwise they become leaves of their own, sorted first
    /// if there is more than one leaf's worth.
    fn house_appended(&mut self, reach: f32) -> Range<u32> {
        let (first_vertex, first_triangle) = (self.topo.first_vertex, self.topo.first_triangle);
        let (vertices, triangles) = (self.positions.len() as u32, (self.indices.len() / 3) as u32);
        let added = (triangles - first_triangle) as usize;
        if added == 0 {
            return 0..0;
        }
        let whole = [first_triangle, triangles, first_vertex, vertices];
        if let Some(tail) = self.bvh.tail_leaf(first_triangle, first_vertex) {
            let around = self.runs_bounds(whole);
            let joined = Aabb::union(self.bvh.leaf_box(tail), around);
            let held = self.bvh.leaves[tail as usize].tri_count();
            if held + added <= LEAF_MAX_TRIS && joined.size().max_element() <= TAIL_REACH * reach {
                self.bvh.extend_tail(triangles, vertices, around);
                return tail..tail + 1;
            }
        }
        let runs = if added <= LEAF_MAX_TRIS { vec![whole] } else { self.sort_appended() };
        let bounds: Vec<Aabb> = runs.iter().map(|&run| self.runs_bounds(run)).collect();
        let first = self.bvh.add_leaves(&runs, &bounds);
        first..first + runs.len() as u32
    }

    /// Puts the triangles this refine appended in leaf order (those that lie
    /// together, together) and renumbers the vertices it appended to match,
    /// so that each leaf-to-be has one run of each. Returns the runs.
    ///
    /// Only numbers this refine handed out change, and nothing outside it
    /// has seen them yet: they are in the appended triangles, in the older
    /// triangles the refine rewrote, and in the lists of the vertices it
    /// touched, and all three are renamed here.
    fn sort_appended(&mut self) -> Vec<LeafRuns> {
        let (v0, t0) = (self.topo.first_vertex as usize, self.topo.first_triangle as usize);
        let (added_vertices, added) = (self.positions.len() - v0, self.indices.len() / 3 - t0);
        let centroids: Vec<Vec3> = self.indices[t0 * 3..]
            .chunks_exact(3)
            .map(|t| {
                (self.positions[t[0] as usize] + self.positions[t[1] as usize] + self.positions[t[2] as usize]) / 3.0
            })
            .collect();
        let mut order: Vec<u32> = (0..added as u32).collect();
        let mut cuts = Vec::new();
        cut_into_leaves(&mut order, 0, &centroids, &mut cuts);
        cuts.push(added as u32);

        // Vertices are numbered by first use, which gives each leaf one run.
        const UNSET: u32 = u32::MAX;
        let mut vertex_to = vec![UNSET; added_vertices];
        let mut triangle_to = vec![0u32; added];
        let mut corners = Vec::with_capacity(added * 3);
        let mut runs: Vec<LeafRuns> = Vec::with_capacity(cuts.len() - 1);
        let mut next = 0u32;
        for leaf in cuts.windows(2) {
            let vert_start = next;
            for at in leaf[0]..leaf[1] {
                let old = order[at as usize] as usize;
                triangle_to[old] = at;
                for &v in &self.indices[(t0 + old) * 3..(t0 + old) * 3 + 3] {
                    corners.push(if (v as usize) < v0 {
                        v
                    } else {
                        let to = &mut vertex_to[v as usize - v0];
                        if *to == UNSET {
                            *to = next;
                            next += 1;
                        }
                        v0 as u32 + *to
                    });
                }
            }
            runs.push([t0 as u32 + leaf[0], t0 as u32 + leaf[1], v0 as u32 + vert_start, v0 as u32 + next]);
        }
        // A vertex none of the appended triangles still uses (flips can
        // leave one so) goes to the last leaf.
        for to in vertex_to.iter_mut().filter(|to| **to == UNSET) {
            *to = next;
            next += 1;
        }
        runs.last_mut().expect("at least one leaf")[3] = v0 as u32 + next;

        let vertex = |v: u32| if (v as usize) < v0 { v } else { v0 as u32 + vertex_to[v as usize - v0] };
        let triangle = |t: u32| if (t as usize) < t0 { t } else { t0 as u32 + triangle_to[t as usize - t0] };
        self.indices[t0 * 3..].copy_from_slice(&corners);
        self.groups.reorder(t0, &triangle_to);
        for &t in &self.topo.changed {
            for corner in &mut self.indices[t as usize * 3..t as usize * 3 + 3] {
                *corner = vertex(*corner);
            }
        }

        permute(&mut self.positions[v0..], &vertex_to);
        permute(&mut self.normals[v0..], &vertex_to);
        permute(&mut self.mask[v0..], &vertex_to);
        permute(&mut self.orig_pos[v0..], &vertex_to);
        permute(&mut self.orig_nrm[v0..], &vertex_to);
        permute(&mut self.orig_mask[v0..], &vertex_to);
        permute(&mut self.stamp[v0..], &vertex_to);
        if let Some(layers) = &mut self.layers {
            layers.reorder(|offsets| permute(&mut offsets[v0..], &vertex_to));
        }
        if self.curv_on {
            permute(&mut self.curv[v0..], &vertex_to);
            permute(&mut self.curv_raw[v0..], &vertex_to);
        }
        let born = self.topo.parents.len() - added_vertices;
        permute(&mut self.topo.parents[born..], &vertex_to);
        for parents in &mut self.topo.parents[born..] {
            *parents = parents.map(vertex);
        }

        self.vt.renumber(v0, &vertex_to);
        self.vv.renumber(v0, &vertex_to);
        for k in 0..self.topo.touched.len() {
            let v = vertex(self.topo.touched[k]);
            self.topo.touched[k] = v;
            for neighbour in self.vv.row_mut(v as usize) {
                *neighbour = vertex(*neighbour);
            }
            for t in self.vt.row_mut(v as usize) {
                *t = triangle(*t);
            }
        }
        runs
    }

    // ---- the stroke's undo record ------------------------------------------

    /// Closes the record of a stroke that grew the mesh, if this one did.
    /// Called as the stroke ends, once its leaves' bounds are exact again.
    pub(super) fn end_stroke_topology(&mut self) -> Option<Box<TopologyDelta>> {
        let record = self.topo.stroke.take()?;
        // The stroke hung its leaves on the tree one refine at a time; laid
        // out afresh the tree is balanced again, however long it was.
        self.bvh.rebuild_tree();
        let (v0, t0) = (record.vertices as usize, record.triangles as usize);
        let born = self.topo.parents.len() - (self.positions.len() - v0);
        Some(Box::new(TopologyDelta {
            vertices: record.vertices,
            triangles: record.triangles,
            slots_after: record.slots.iter().map(|&t| self.corners(t)).collect(),
            slots: record.slots,
            slots_before: record.before,
            new_positions: self.positions[v0..].to_vec(),
            new_mask: self.mask[v0..].to_vec(),
            new_parents: self.topo.parents[born..].to_vec(),
            new_indices: self.indices[t0 * 3..].to_vec(),
            new_groups: self.groups.from(t0),
            leaves: record.leaves,
            tail_before: record.tail,
            leaves_after: self.bvh.leaves[record.leaves as usize - 1..].iter().map(Leaf::runs).collect(),
            polys: record.polys,
            uvs: record.uvs,
            reused: record
                .reused
                .iter()
                .map(|&(vertex, before)| Reused { vertex, before, after: self.vertex_kept(vertex) })
                .collect(),
            reused_groups: record
                .reused_groups
                .iter()
                .map(|&(t, before)| (t, before, self.groups.ids.get(t as usize).copied().unwrap_or(before)))
                .collect(),
        }))
    }

    /// Undo or redo of a stroke that grew the mesh: the same "put the kept
    /// state back" step in both directions. `verts` and `positions` are the
    /// stroke's ordinary record, the vertices from before it that it moved
    /// and where they go. Counts first, then positions, then the triangles,
    /// and last a rebuild of everything derived, so that it sees the final
    /// shape.
    pub(super) fn apply_topology(&mut self, delta: &TopologyDelta, verts: &[u32], positions: &[Vec3], undo: bool) {
        debug_assert!(!self.in_stroke);
        let (v0, t0) = (delta.vertices as usize, delta.triangles as usize);
        // Back to the counts before the stroke either way.
        let born = if self.topo.parents.is_empty() { v0 } else { self.topo.born_from as usize };
        self.topo.parents.truncate(v0.saturating_sub(born));
        self.positions.truncate(v0);
        self.mask.truncate(v0);
        self.indices.truncate(t0 * 3);
        let slots = if undo { &delta.slots_before } else { &delta.slots_after };
        if undo {
            self.polys = delta.polys.clone();
            self.uvs = delta.uvs.clone();
        } else {
            self.positions.extend_from_slice(&delta.new_positions);
            self.mask.extend_from_slice(&delta.new_mask);
            self.indices.extend_from_slice(&delta.new_indices);
            if self.topo.parents.is_empty() {
                self.topo.born_from = delta.vertices;
            }
            self.topo.parents.extend_from_slice(&delta.new_parents);
            self.polys = None;
            self.uvs = Vec::new();
        }
        for (&t, corners) in delta.slots.iter().zip(slots) {
            self.indices[t as usize * 3..t as usize * 3 + 3].copy_from_slice(corners);
        }
        let added: &[u16] = if undo { &[] } else { &delta.new_groups };
        self.groups.resize(t0, added, self.indices.len() / 3);
        for &(t, before, after) in &delta.reused_groups {
            if let Some(id) = self.groups.ids.get_mut(t as usize) {
                *id = if undo { before } else { after };
            }
        }
        for (&v, &p) in verts.iter().zip(positions) {
            self.positions[v as usize] = p;
        }
        // The vertices the stroke's splits took back, after the moves: a
        // brush may have moved one after it was taken.
        for reused in &delta.reused {
            let kept = if undo { reused.before } else { reused.after };
            self.positions[reused.vertex as usize] = kept.position;
            self.mask[reused.vertex as usize] = kept.mask;
            self.set_parents(reused.vertex, kept.parents);
        }

        // Everything kept per vertex follows the count. Normals are worked
        // out below; the stroke snapshot starts empty.
        let count = self.positions.len();
        self.normals.resize(count, Vec3::Y);
        self.orig_pos.resize(count, Vec3::ZERO);
        self.orig_nrm.resize(count, Vec3::ZERO);
        self.orig_mask.resize(count, 0.0);
        self.stamp.resize(count, 0);
        self.touched.clear();

        let (tail, more) =
            if undo { (delta.tail_before, &[][..]) } else { (delta.leaves_after[0], &delta.leaves_after[1..]) };
        self.bvh.loosen();
        self.bvh.restore_leaves(delta.leaves as usize, tail, more, &self.positions, &self.indices);
        let leaves = self.bvh.leaves.len();
        for set in [
            &mut self.stroke_leaves,
            &mut self.pending_normals,
            &mut self.dirty_pos,
            &mut self.dirty_nrm,
            &mut self.dirty_mask,
            &mut self.dirty_curv,
        ] {
            *set = BitSet::with_len(leaves);
        }
        self.hidden.resize(self.indices.len() / 3, count);
        self.build_adjacency();
        // Marks every leaf for the renderer, and takes curvature with it.
        self.recompute_all_normals();

        let topo = &mut self.topo;
        // The points merges left, and the vertices in no triangle, as many as there are now.
        topo.dead = self.indices.par_chunks_exact(3).filter(|t| t[0] == t[1] && t[1] == t[2]).count();
        topo.orphans = (0..count).into_par_iter().filter(|&v| self.vt.row(v).is_empty()).count();
        topo.version += 1;
        topo.resend = true;
        topo.unsent.clear();
        topo.unsent_from = None;
    }
}
