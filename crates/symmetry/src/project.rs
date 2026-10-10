//! Making part of a model symmetric WITHOUT a correspondence: each vertex is
//! dropped onto the nearest point of the other side's surface as reflected
//! through the plane.
//!
//! This is the masked repair's answer to "too asymmetric to match up". Pairing
//! vertices only works where the two sides are tessellated alike, and the areas
//! people want to repair locally are very often exactly the ones that are not
//! (measured on a 4.9M-vertex figure: 36,288 of the 345,240 unmasked vertices
//! had no counterpart, and the pairing repair rightly refused). A surface has a
//! mirror image whether or not its vertices pair up, so this always has
//! something to aim at. Topology is untouched; what it gives up is
//! vertex-exactness: the destination ends up lying ON the mirrored source
//! surface rather than being a vertex-for-vertex copy of it.

use glam::Vec3;
use rayon::prelude::*;
use sculpt_geom::{Aabb, tri};

use crate::cells::CellSet;
use crate::map::SymmetryMap;
use crate::{Axis, par};

/// How many rings of moving vertices, counted in from fully masked surface on
/// the same side, are eased in rather than moved the whole way. A hard-edged
/// mask otherwise puts a fully moved vertex next to one that did not move at
/// all, and the triangles between them take up the whole difference as a step.
const FEATHER_RINGS: u32 = 6;

/// What `smooth_stretched` calls stretched (an edge this many spacings long AND
/// this many times its length before the repair, or a triangle turned over),
/// how many rings around such a triangle it works on, and for how many passes.
const STRETCHED_SPACINGS: f32 = 2.5;
const STRETCHED_GROWTH: f32 = 2.0;
const STRETCHED_RINGS: usize = 2;
const STRETCHED_PASSES: usize = 20;

/// Passes, step size and search reach (in vertex spacings) of `relax_on_mirror`,
/// and which vertices it touches: those the projection moved by more than
/// `RELAX_MIN_MOVE` spacings, and `RELAX_RINGS` rings of moving vertices around
/// them.
const RELAX_PASSES: usize = 24;
const RELAX_RATE: f32 = 0.5;
const RELAX_REACH_SPACINGS: f32 = 6.0;
const RELAX_MIN_MOVE: f32 = 0.1;
const RELAX_RINGS: usize = 3;

/// How near its landing point a vertex must already be, in vertex spacings, to
/// count as lying on the mirrored surface and be left alone.
const RESTING_FRACTION: f32 = 1e-4;

/// The less usual inputs of `project_onto_mirror`.
#[derive(Clone, Copy, Default)]
pub struct ProjectOptions<'a> {
    /// Leave every vertex the map paired (or put on the plane) alone. For use
    /// AFTER `repair::make_symmetric`: the pairs have already been copied
    /// across exactly, and what is left unpaired is precisely where the two
    /// sides differ too much to pair, the part a local repair is usually for.
    pub unpaired_only: bool,
    /// The positions as they stood before any earlier pass moved them. The
    /// moving vertices' facing is read from it (their surroundings may just
    /// have been pulled out from around them, so the current geometry says
    /// nothing useful about which way they face), and so is the shift a vertex
    /// with nothing in reach rides. Defaults to `positions` as handed in.
    pub original: Option<&'a [Vec3]>,
    /// The whole model's positions and triangles, when the arrays being
    /// repaired are only a piece of it. The mask (and the piece) decide what
    /// moves, never what it is measured against. Defaults to the arrays being
    /// repaired.
    pub source: Option<(&'a [Vec3], &'a [u32])>,
}

/// Lays the destination side's free vertices onto the source side's mirrored
/// surface. Returns how many vertices moved.
///
/// A vertex takes the nearest point outright when that surface faces the way
/// the vertex does. When it does not (a vertex between two thin sheets, whose
/// nearest neighbour is the back of the sheet next door), a second, short
/// search looks only at surface facing its way. `max_distance` bounds the
/// first search; a vertex with nothing in reach, or nothing facing it nearby,
/// rides its neighbours' shift instead, the rule `repair::carry_unmatched`
/// uses.
///
/// Landing on the surface is not the whole job. The vertices are then evened
/// out along it (`relax_on_mirror`), the few patches that could not be laid
/// down cleanly are bridged (`smooth_stretched`), and under a mask the move is
/// eased in over the last few rings before fully masked surface, so the
/// repaired patch neither streaks nor steps where it meets what was left alone.
///
/// `indices` must be the triangles `map` was built with, and the map must have
/// topology (0 is returned otherwise). A fully masked vertex never moves.
///
/// Cost is what the first version of this got wrong: it bucketed every source
/// triangle inside the moving vertices' bounding box. Now only triangles within
/// reach of a moving vertex are kept, they go in a bounding-volume tree, the
/// near ones are tried before the far ones, and the queries run across cores.
pub fn project_onto_mirror(
    positions: &mut [Vec3],
    indices: &[u32],
    map: &SymmetryMap,
    source_is_positive: bool,
    mask: Option<&[f32]>,
    max_distance: f32,
    options: ProjectOptions<'_>,
) -> usize {
    let n = positions.len();
    assert_eq!(map.vertex_count(), n, "the map was built for a different mesh");
    let Some(adjacency) = map.adjacency() else { return 0 };
    let axis = map.axis();
    let sign = if source_is_positive { 1.0 } else { -1.0 };
    let reach = max_distance.max(1e-6);
    let spacing = map.mean_spacing();

    // Which vertices move.
    let moves: Vec<bool> = {
        let read = &*positions;
        (0..n)
            .into_par_iter()
            .with_min_len(8192)
            .map(|i| {
                !mask.is_some_and(|m| m[i] >= 1.0)
                    && axis.coord(read[i]) * sign < 0.0
                    && !(options.unpaired_only && !map.is_unmatched(i as u32))
            })
            .collect()
    };
    let move_list = par::indices_where(n, |i| moves[i]);
    if move_list.is_empty() {
        return 0;
    }

    let before = positions.to_vec();
    let original = options.original.unwrap_or(&before);
    assert_eq!(original.len(), n, "`original` describes a different mesh");
    let source = options.source.unwrap_or((&before, indices));

    // Their normals (area-weighted, not normalised: only the direction is used).
    let normals: Vec<Vec3> = (0..n)
        .into_par_iter()
        .with_min_len(4096)
        .map(|i| {
            if !moves[i] {
                return Vec3::ZERO;
            }
            let face = |&t: &u32| {
                let [a, b, c] = corners(original, indices, t as usize * 3);
                tri::area_normal(a, b, c)
            };
            adjacency.triangles_of(i).iter().map(face).sum()
        })
        .collect();

    // Near first, far only for what near did not settle. Most vertices have
    // their mirrored surface within a few spacings, and the triangles within a
    // SHORT reach of the moving vertices are a small set; the full reach then
    // only has to gather triangles around the handful still looking. Searching
    // everything at the full reach in one go meant a tree over nearly every
    // source triangle of the model.
    let mut reaches = vec![reach];
    while reaches.len() < 3 && reaches[0] * 0.25 >= spacing * 6.0 {
        reaches.insert(0, reaches[0] * 0.25);
    }
    let resting = spacing * RESTING_FRACTION;
    let mut looking = move_list.clone();
    for &level_reach in &reaches {
        if looking.is_empty() {
            break;
        }
        let Some(tree) = MirrorTree::around(source, axis, sign, &before, &looking, level_reach) else { continue };
        let landed: Vec<Option<Vec3>> = looking
            .par_iter()
            .with_min_len(256)
            .map(|&i| tree.land(before[i as usize], normals[i as usize], level_reach, spacing))
            .collect();
        let mut still = Vec::new();
        for (&i, landed) in looking.iter().zip(&landed) {
            match landed {
                // A vertex already lying on the mirrored surface is left
                // exactly where it is. Its "nearest point" differs from it by
                // rounding alone, and writing that back made every press
                // report vertices moved and leave an undo step that undid
                // nothing anyone could see.
                Some(p) if p.distance_squared(before[i as usize]) > resting * resting => positions[i as usize] = *p,
                Some(_) => {}
                None => still.push(i),
            }
        }
        looking = still;
    }

    let mut known = vec![true; n];
    looking.iter().for_each(|&i| known[i as usize] = false);
    relax_on_mirror(positions, &before, map, &moves, &known, &normals, source, sign, spacing);

    // Whatever found no surface goes where its surroundings went, ring by ring.
    while !looking.is_empty() {
        let read = &*positions;
        let fills: Vec<(u32, Vec3)> = par::gather(looking.len(), |k, out| {
            let i = looking[k];
            let (mut sum, mut count) = (Vec3::ZERO, 0);
            for &nb in map.neighbours(i) {
                if known[nb as usize] {
                    sum += read[nb as usize] - original[nb as usize];
                    count += 1;
                }
            }
            if count > 0 {
                out.push((i, original[i as usize] + sum / count as f32));
            }
        });
        if fills.is_empty() {
            break;
        }
        for &(i, p) in &fills {
            positions[i as usize] = p;
            known[i as usize] = true;
        }
        looking.retain(|&i| !known[i as usize]);
    }
    smooth_stretched(positions, original, indices, map, &moves, spacing);

    // Everything above worked out where each vertex WOULD go; how far it
    // actually goes is the mask's call, plus a fade over the last few rings
    // before fully masked surface.
    if let Some(mask) = mask {
        let ring = feather_rings(map, &moves, &move_list, mask, &before, sign);
        positions.par_iter_mut().with_min_len(8192).enumerate().for_each(|(i, p)| {
            if !moves[i] {
                return;
            }
            let mut weight = 1.0 - mask[i].clamp(0.0, 1.0);
            if ring[i] > 0 {
                let x = ring[i] as f32 / (FEATHER_RINGS + 1) as f32;
                weight *= x * x * (3.0 - 2.0 * x);
            }
            if weight < 1.0 {
                *p = before[i] + (*p - before[i]) * weight;
            }
        });
    }

    move_list.iter().filter(|&&i| positions[i as usize] != before[i as usize]).count()
}

#[inline]
fn corners(positions: &[Vec3], indices: &[u32], at: usize) -> [Vec3; 3] {
    [indices[at], indices[at + 1], indices[at + 2]].map(|i| positions[i as usize])
}

/// For each moving vertex within `FEATHER_RINGS` of a fully masked vertex on
/// the DESTINATION side, which ring it is in (1 = touching one); 0 everywhere
/// else. Masked vertices on the source side do not count: the source never
/// moves, masked or not, and the surface the destination is laid onto already
/// meets it cleanly at the plane.
fn feather_rings(
    map: &SymmetryMap,
    moves: &[bool],
    move_list: &[u32],
    mask: &[f32],
    positions: &[Vec3],
    sign: f32,
) -> Vec<u32> {
    let axis = map.axis();
    let mut ring = vec![0u32; moves.len()];
    let touches_masked = |i: u32| {
        map.neighbours(i).iter().any(|&nb| mask[nb as usize] >= 1.0 && axis.coord(positions[nb as usize]) * sign < 0.0)
    };
    let mut frontier: Vec<u32> =
        move_list.par_iter().with_min_len(4096).copied().filter(|&i| touches_masked(i)).collect();
    frontier.iter().for_each(|&i| ring[i as usize] = 1);

    for depth in 2..=FEATHER_RINGS {
        if frontier.is_empty() {
            break;
        }
        let mut next = Vec::new();
        for &i in &frontier {
            for &nb in map.neighbours(i) {
                if moves[nb as usize] && ring[nb as usize] == 0 {
                    ring[nb as usize] = depth;
                    next.push(nb);
                }
            }
        }
        frontier = next;
    }
    ring
}

/// `seeds` and the moving vertices within `rings` edges of them, seeds first.
/// `inside` must be all false and holds the result's membership on return.
fn grow_rings(
    map: &SymmetryMap,
    mut set: Vec<u32>,
    rings: usize,
    inside: &mut [bool],
    allowed: impl Fn(u32) -> bool,
) -> Vec<u32> {
    set.iter().for_each(|&i| inside[i as usize] = true);
    let mut from = 0;
    for _ in 0..rings {
        let to = set.len();
        for f in from..to {
            for &nb in map.neighbours(set[f]) {
                if allowed(nb) && !std::mem::replace(&mut inside[nb as usize], true) {
                    set.push(nb);
                }
            }
        }
        from = to;
    }
    set
}

/// Evens out the vertices the projection has just placed, without taking them
/// off the mirrored surface: each steps toward the middle of its neighbours and
/// is dropped back onto the surface, a few times over.
///
/// "Nearest point on the surface" says where each vertex lands and nothing
/// about how they are spread once there. Vertices either side of a ridge or
/// groove all find the same crest nearest, so they pile up along it and the
/// wall beside it is left to a few long thin triangles, which shade as streaks
/// fanning across the repaired area. The surface is right; its sampling is
/// not, and sliding the vertices along it fixes the sampling without touching
/// the shape. Vertices that did not move are the anchors, so the relaxed patch
/// also meets its surroundings without a seam of stretched triangles.
///
/// Only where the projection actually moved something (and a few rings
/// around it). A part of the area that already lay on the mirrored surface has
/// nothing to even out, and sliding its vertices anyway would make every press
/// change the mesh: a repair that never reports "already matches".
#[allow(clippy::too_many_arguments)]
fn relax_on_mirror(
    positions: &mut [Vec3],
    before: &[Vec3],
    map: &SymmetryMap,
    moves: &[bool],
    known: &[bool],
    normals: &[Vec3],
    source: (&[Vec3], &[u32]),
    sign: f32,
    spacing: f32,
) {
    if spacing <= 0.0 {
        return;
    }
    let candidate = |i: u32| moves[i as usize] && known[i as usize];
    let min_move = spacing * RELAX_MIN_MOVE;
    let read = &*positions;
    let moved = par::indices_where(read.len(), |i| {
        candidate(i as u32) && read[i].distance_squared(before[i]) > min_move * min_move
    });
    let mut active = vec![false; positions.len()];
    let settled = grow_rings(map, moved, RELAX_RINGS, &mut active, candidate);
    if settled.is_empty() {
        return;
    }

    let reach = spacing * RELAX_REACH_SPACINGS;
    let Some(tree) = MirrorTree::around(source, map.axis(), sign, positions, &settled, reach) else { return };

    let mut relaxed = Vec::with_capacity(settled.len());
    for _ in 0..RELAX_PASSES {
        // Jacobi: every vertex reads the positions the pass started with, so
        // the result does not depend on the order (or the thread) they are
        // visited in.
        let read = &*positions;
        settled
            .par_iter()
            .with_min_len(256)
            .map(|&v| {
                let here = read[v as usize];
                let around = map.neighbours(v);
                if around.is_empty() {
                    return here;
                }
                let middle = around.iter().map(|&nb| read[nb as usize]).sum::<Vec3>() / around.len() as f32;
                let target = here + (middle - here) * RELAX_RATE;
                tree.land(target, normals[v as usize], reach, spacing).unwrap_or(here)
            })
            .collect_into_vec(&mut relaxed);
        for (&v, &p) in settled.iter().zip(&relaxed) {
            positions[v as usize] = p;
        }
    }
}

/// Smooths the few patches the projection could not lay down cleanly.
///
/// Where the mirrored surface folds differently from the one being repaired
/// (an undercut on one side and not the other, two tubes where there was one),
/// neighbouring vertices find different folds nearest, and the triangles
/// between them are pulled across the gap: long, thin, sometimes turned over.
/// `relax_on_mirror` cannot help, because each vertex is already evenly placed
/// on ITS fold. There is no position on the mirrored surface that is right for
/// those triangles, so the honest result is a smooth bridge: the vertices of
/// each such triangle, and a couple of rings around it, settle to the average
/// of their neighbours, anchored by everything around them. Only moving
/// vertices are touched, and only near a triangle that actually came out
/// stretched; the rest of the repair stays exactly on the mirrored surface.
fn smooth_stretched(
    positions: &mut [Vec3],
    original: &[Vec3],
    indices: &[u32],
    map: &SymmetryMap,
    moves: &[bool],
    spacing: f32,
) {
    if spacing <= 0.0 {
        return;
    }
    let min_sqr = (spacing * STRETCHED_SPACINGS) * (spacing * STRETCHED_SPACINGS);
    let growth_sqr = STRETCHED_GROWTH * STRETCHED_GROWTH;

    let read = &*positions;
    let bad: Vec<u32> = par::gather(indices.len() / 3, |t, out| {
        let i = [indices[t * 3], indices[t * 3 + 1], indices[t * 3 + 2]];
        if !i.iter().any(|&v| moves[v as usize]) {
            return;
        }
        let stretched = (0..3).any(|e| {
            let (a, b) = (i[e] as usize, i[(e + 1) % 3] as usize);
            let now = read[a].distance_squared(read[b]);
            now > min_sqr && now > growth_sqr * original[a].distance_squared(original[b])
        });
        let turned = || {
            let [a, b, c] = corners(original, indices, t * 3);
            let [p, q, r] = corners(read, indices, t * 3);
            tri::area_normal(a, b, c).dot(tri::area_normal(p, q, r)) < 0.0
        };
        if stretched || turned() {
            out.extend(i.into_iter().filter(|&v| moves[v as usize]));
        }
    });
    if bad.is_empty() {
        return;
    }

    let mut inside = vec![false; positions.len()];
    let mut seeds = Vec::with_capacity(bad.len());
    for v in bad {
        if !std::mem::replace(&mut inside[v as usize], true) {
            seeds.push(v);
        }
    }
    seeds.iter().for_each(|&v| inside[v as usize] = false);
    let set = grow_rings(map, seeds, STRETCHED_RINGS, &mut inside, |v| moves[v as usize]);

    // Jacobi, as everywhere else here: a pass reads only what the pass before
    // it left.
    let mut next = Vec::with_capacity(set.len());
    for _ in 0..STRETCHED_PASSES {
        let read = &*positions;
        set.par_iter()
            .with_min_len(1024)
            .map(|&v| {
                let here = read[v as usize];
                let around = map.neighbours(v);
                if around.is_empty() {
                    return here;
                }
                let middle = around.iter().map(|&nb| read[nb as usize]).sum::<Vec3>() / around.len() as f32;
                here + (middle - here) * 0.5
            })
            .collect_into_vec(&mut next);
        for (&v, &p) in set.iter().zip(&next) {
            positions[v as usize] = p;
        }
    }
}

/// A nearest point on the mirrored surface.
#[derive(Clone, Copy)]
struct Hit {
    point: Vec3,
    sqr: f32,
    /// Whether the triangle it lies on faces along the query's normal.
    facing: bool,
}

#[derive(Clone, Copy)]
struct Node {
    bounds: Aabb,
    /// A leaf's first triangle slot; a branch's right child (its left child is
    /// the node straight after it).
    link: u32,
    /// Triangles in a leaf; 0 for a branch.
    count: u32,
}

/// Bounding-volume tree over a set of triangles as REFLECTED through a mirror
/// plane, for nearest-point queries. Built once, then read from many threads.
///
/// The triangles are sorted along a space-filling curve and the tree is simply
/// that order cut in half again and again, so it is balanced whatever the
/// triangles look like (which bounds the depth, and with it the query stack),
/// and both the sort and the tree are built across cores.
struct MirrorTree {
    /// Reflected corners of each triangle, in tree order.
    corners: Vec<[Vec3; 3]>,
    /// Depth-first: a branch is followed by its whole left subtree.
    nodes: Vec<Node>,
}

const LEAF_SIZE: usize = 8;
/// Deeper than a balanced tree over 2^32 triangles can be.
const STACK_SIZE: usize = 64;

impl MirrorTree {
    /// The tree over every source-side triangle of `source` whose reflection
    /// lies within `reach` of one of the vertices `around` lists (positions
    /// read from `points`), or `None` when there is none. "Within reach" is
    /// judged by coarse cells a reach wide.
    fn around(
        source: (&[Vec3], &[u32]),
        axis: Axis,
        sign: f32,
        points: &[Vec3],
        around: &[u32],
        reach: f32,
    ) -> Option<MirrorTree> {
        let (positions, indices) = source;
        let near = CellSet::around(around.par_iter().map(|&i| points[i as usize]), reach);
        if near.is_empty() {
            return None;
        }
        let bounds = around
            .par_iter()
            .with_min_len(8192)
            .fold(|| Aabb::EMPTY, |mut b, &i| {
                b.grow(points[i as usize]);
                b
            })
            .reduce(|| Aabb::EMPTY, Aabb::union)
            .expanded(reach);

        // Every triangle of the model is looked at here, across cores.
        let mut prims: Vec<(u64, [Vec3; 3])> = par::gather(indices.len() / 3, |t, out| {
            let abc = corners(positions, indices, t * 3);
            // Source side: any corner on it. A triangle straddling the plane
            // counts, so the reflected surface reaches a little past the plane
            // instead of stopping short.
            if abc.iter().all(|&p| axis.coord(p) * sign <= 0.0) {
                return;
            }
            let reflected = abc.map(|p| axis.reflect(p));
            let centroid = (reflected[0] + reflected[1] + reflected[2]) / 3.0;
            if bounds.contains(centroid) && near.contains(centroid) {
                out.push((curve_position(centroid, &bounds), reflected));
            }
        });
        if prims.is_empty() {
            return None;
        }
        if prims.len() > 8192 {
            prims.par_sort_unstable_by_key(|p| p.0);
        } else {
            prims.sort_unstable_by_key(|p| p.0);
        }
        let corners: Vec<[Vec3; 3]> = prims.into_iter().map(|p| p.1).collect();

        let mut nodes = vec![Node { bounds: Aabb::EMPTY, link: 0, count: 0 }; node_count(corners.len())];
        build_nodes(&mut nodes, 0, &corners, 0);
        Some(MirrorTree { corners, nodes })
    }

    /// Nearest point to `p` on the reflected triangles, closer than
    /// `sqrt(max_sqr)`. With `require_facing` only triangles facing along
    /// `normal` are considered at all.
    fn closest(&self, p: Vec3, normal: Vec3, require_facing: bool, max_sqr: f32) -> Option<Hit> {
        let mut best = Hit { point: p, sqr: max_sqr, facing: false };
        let mut found = false;
        let mut stack = [0u32; STACK_SIZE];
        let mut top = 1;
        while top > 0 {
            top -= 1;
            let index = stack[top] as usize;
            let node = &self.nodes[index];
            if distance_sqr(&node.bounds, p) >= best.sqr {
                continue;
            }
            if node.count > 0 {
                for &[a, b, c] in &self.corners[node.link as usize..(node.link + node.count) as usize] {
                    // Reflection reverses the winding, so the facing is the other cross.
                    let faces = (c - a).cross(b - a).dot(normal) > 0.0;
                    if require_facing && !faces {
                        continue;
                    }
                    let q = tri::closest_point(p, a, b, c);
                    let d = q.distance_squared(p);
                    if d < best.sqr {
                        best = Hit { point: q, sqr: d, facing: faces };
                        found = true;
                    }
                }
                continue;
            }

            // Nearer child last, so it is popped first and tightens the bound early.
            let (left, right) = (index as u32 + 1, node.link);
            let d_left = distance_sqr(&self.nodes[left as usize].bounds, p);
            let d_right = distance_sqr(&self.nodes[right as usize].bounds, p);
            let (near, d_near, far, d_far) =
                if d_left < d_right { (left, d_left, right, d_right) } else { (right, d_right, left, d_left) };
            if top + 2 > STACK_SIZE {
                continue; // cannot happen at the depths `build_nodes` allows
            }
            if d_far < best.sqr {
                stack[top] = far;
                top += 1;
            }
            if d_near < best.sqr {
                stack[top] = near;
                top += 1;
            }
        }
        found.then_some(best)
    }

    /// Where a vertex at `p` facing along `normal` lands on the mirrored
    /// surface: the nearest point within `reach`, unless that surface faces
    /// the other way, when it is the nearest point a little further out that
    /// does not. `None` when there is nothing suitable.
    fn land(&self, p: Vec3, normal: Vec3, reach: f32, spacing: f32) -> Option<Vec3> {
        let nearest = self.closest(p, normal, false, reach * reach)?;
        if nearest.facing {
            return Some(nearest.point);
        }
        let r = (3.0 * nearest.sqr.sqrt() + 2.0 * spacing).min(reach);
        self.closest(p, normal, true, r * r).map(|hit| hit.point)
    }
}

/// Nodes in the tree over `triangles` triangles.
fn node_count(triangles: usize) -> usize {
    if triangles <= LEAF_SIZE { 1 } else { 1 + node_count(triangles / 2) + node_count(triangles - triangles / 2) }
}

/// Fills `nodes` (whose first entry is node number `base` of the whole tree)
/// with the subtree over `corners`, which start at triangle slot `first`.
/// Returns the subtree's bounds.
fn build_nodes(nodes: &mut [Node], base: usize, corners: &[[Vec3; 3]], first: usize) -> Aabb {
    if corners.len() <= LEAF_SIZE {
        let bounds = corners.iter().flatten().fold(Aabb::EMPTY, |mut b, &p| {
            b.grow(p);
            b
        });
        nodes[0] = Node { bounds, link: first as u32, count: corners.len() as u32 };
        return bounds;
    }
    let half = corners.len() / 2;
    let left_nodes = node_count(half);
    let (head, rest) = nodes.split_first_mut().expect("a subtree has at least its root");
    let (left, right) = rest.split_at_mut(left_nodes);
    let (right_base, right_first) = (base + 1 + left_nodes, first + half);
    let (a, b) = if corners.len() > 4096 {
        rayon::join(
            || build_nodes(left, base + 1, &corners[..half], first),
            || build_nodes(right, right_base, &corners[half..], right_first),
        )
    } else {
        let a = build_nodes(left, base + 1, &corners[..half], first);
        (a, build_nodes(right, right_base, &corners[half..], right_first))
    };
    let bounds = Aabb::union(a, b);
    *head = Node { bounds, link: right_base as u32, count: 0 };
    bounds
}

#[inline]
fn distance_sqr(bounds: &Aabb, p: Vec3) -> f32 {
    (bounds.min - p).max(p - bounds.max).max(Vec3::ZERO).length_squared()
}

/// Where `p` falls along a Z-order curve through `bounds`: points near each
/// other in space are near each other in this order.
fn curve_position(p: Vec3, bounds: &Aabb) -> u64 {
    let size = (bounds.max - bounds.min).max(Vec3::splat(1e-30));
    let unit = ((p - bounds.min) / size).clamp(Vec3::ZERO, Vec3::ONE);
    let [x, y, z] = (unit * 2_097_151.0).to_array().map(|v| spread_bits(v as u64));
    x | (y << 1) | (z << 2)
}

/// Spreads the low 21 bits of `v` so there are two zero bits between each.
fn spread_bits(v: u64) -> u64 {
    let mut v = v & 0x1F_FFFF;
    v = (v | (v << 32)) & 0x1F_0000_0000_FFFF;
    v = (v | (v << 16)) & 0x1F_0000_FF00_00FF;
    v = (v | (v << 8)) & 0x100F_00F0_0F00_F00F;
    v = (v | (v << 4)) & 0x10C3_0C30_C30C_30C3;
    (v | (v << 2)) & 0x1249_2492_4924_9249
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{Lcg, octa_sphere};
    use crate::map::default_tolerance;
    use sculpt_geom::TriMesh;

    fn whole_tree(mesh: &TriMesh, axis: Axis, sign: f32) -> MirrorTree {
        let all: Vec<u32> = (0..mesh.vertex_count() as u32).collect();
        MirrorTree::around((&mesh.positions, &mesh.indices), axis, sign, &mesh.positions, &all, 10.0).unwrap()
    }

    /// The nearest point by trying every source-side triangle.
    fn brute_closest(mesh: &TriMesh, axis: Axis, sign: f32, p: Vec3) -> f32 {
        (0..mesh.triangle_count())
            .filter(|&t| mesh.triangle(t).iter().any(|&q| axis.coord(q) * sign > 0.0))
            .map(|t| {
                let [a, b, c] = mesh.triangle(t).map(|q| axis.reflect(q));
                tri::closest_point(p, a, b, c).distance_squared(p)
            })
            .fold(f32::MAX, f32::min)
    }

    #[test]
    fn tree_layout_matches_its_node_count_at_every_size() {
        for triangles in [1usize, 2, 7, 8, 9, 16, 17, 100, 1000, 5000] {
            let mut rng = Lcg::new(triangles as u64);
            let corners: Vec<[Vec3; 3]> = (0..triangles)
                .map(|_| {
                    let at = rng.in_cube();
                    [at, at + rng.in_cube() * 0.05, at + rng.in_cube() * 0.05]
                })
                .collect();
            let mut nodes = vec![Node { bounds: Aabb::EMPTY, link: 0, count: 0 }; node_count(triangles)];
            build_nodes(&mut nodes, 0, &corners, 0);
            // Every triangle is in exactly one leaf, and every branch holds its children.
            let mut covered = vec![0; triangles];
            for (index, node) in nodes.iter().enumerate() {
                if node.count > 0 {
                    (node.link..node.link + node.count).for_each(|slot| covered[slot as usize] += 1);
                    assert!(node.count as usize <= LEAF_SIZE);
                } else {
                    for child in [index + 1, node.link as usize] {
                        let inner = nodes[child].bounds;
                        assert!(node.bounds.contains(inner.min) && node.bounds.contains(inner.max));
                    }
                }
            }
            assert!(covered.iter().all(|&c| c == 1), "{triangles} triangles");
        }
    }

    #[test]
    fn closest_matches_trying_every_triangle() {
        let mesh = octa_sphere(12, 1.0);
        let tree = whole_tree(&mesh, Axis::X, 1.0);
        let mut rng = Lcg::new(4);
        for _ in 0..400 {
            let p = rng.in_cube() * 1.3;
            let want = brute_closest(&mesh, Axis::X, 1.0, p);
            let hit = tree.closest(p, Vec3::X, false, 100.0).unwrap();
            assert!((hit.sqr - want).abs() <= 1e-6 * want.max(1.0), "{p}: {} vs {want}", hit.sqr);
            assert!((hit.point.distance_squared(p) - hit.sqr).abs() < 1e-6);
            // A bound tighter than the answer finds nothing.
            assert!(tree.closest(p, Vec3::X, false, want * 0.99).is_none() || want == 0.0);
        }
    }

    /// The reflected +X half of a sphere is its -X half, facing outward: a vertex there looking
    /// outward sees it face on, one looking inward only finds it by asking for the back.
    #[test]
    fn facing_is_judged_against_the_reflected_winding() {
        let mesh = octa_sphere(12, 1.0);
        let tree = whole_tree(&mesh, Axis::X, 1.0);
        let p = Vec3::new(-0.9, 0.3, 0.2).normalize() * 1.02;
        let outward = p.normalize();
        assert!(tree.closest(p, outward, false, 1.0).unwrap().facing);
        assert!(!tree.closest(p, -outward, false, 1.0).unwrap().facing);
        assert!(tree.closest(p, outward, true, 1.0).is_some());
        // Looking inward, the only surface facing that way is the far side of the sphere.
        assert!(tree.closest(p, -outward, true, 0.5).is_none());
        assert!(tree.land(p, outward, 1.0, 0.1).unwrap().distance(outward) < 0.02);
        assert!(tree.land(p, -outward, 0.5, 0.1).is_none());
    }

    #[test]
    fn only_triangles_within_reach_of_the_asking_vertices_are_kept() {
        let mesh = octa_sphere(24, 1.0);
        // One vertex near (-1, 0, 0) asking with a short reach.
        let asker = (0..mesh.vertex_count() as u32)
            .min_by(|&a, &b| mesh.positions[a as usize].x.total_cmp(&mesh.positions[b as usize].x))
            .unwrap();
        let source = (&mesh.positions[..], &mesh.indices[..]);
        let tree = MirrorTree::around(source, Axis::X, 1.0, &mesh.positions, &[asker], 0.15).unwrap();
        assert!(tree.corners.len() < mesh.triangle_count() / 8, "{} of {}", tree.corners.len(), mesh.triangle_count());
        assert!(tree.corners.iter().flatten().all(|p| p.distance(mesh.positions[asker as usize]) < 0.6));
        // Nothing on the source side is anywhere near a vertex asking from far outside.
        let far = [Vec3::new(-9.0, 0.0, 0.0)];
        assert!(MirrorTree::around(source, Axis::X, 1.0, &far, &[0], 0.15).is_none());
        // And with the source on the other side there is nothing to reflect onto +X... from -X.
        let tree = whole_tree(&mesh, Axis::X, -1.0);
        assert!(tree.corners.iter().flatten().all(|p| p.x >= 0.0));
    }

    /// A lump raised on the destination half and not on the source half: projecting lays it
    /// back down onto the mirrored sphere.
    #[test]
    fn a_one_sided_lump_is_laid_onto_the_mirrored_surface() {
        let mesh = octa_sphere(32, 1.0);
        let spacing = crate::map::mean_edge_length(&mesh.positions, &mesh.indices);
        let centre = Vec3::new(-0.6, 0.5, 0.6).normalize();
        let mut positions = mesh.positions.clone();
        for p in &mut positions {
            let lump = (-(*p - centre).length_squared() / (2.0 * 0.15 * 0.15)).exp();
            if p.x < 0.0 {
                *p += p.normalize() * (spacing * 4.0 * lump);
            }
        }
        let start = positions.clone();
        let tolerance = default_tolerance(&positions, &mesh.indices);
        let map = SymmetryMap::build(&positions, &mesh.indices, Axis::X, tolerance);

        let moved = project_onto_mirror(&mut positions, &mesh.indices, &map, true, None, 0.5, ProjectOptions::default());
        assert!(moved > 50);
        let mut worst_before = 0.0f32;
        let mut worst_after = 0.0f32;
        for (i, p) in positions.iter().enumerate() {
            if start[i].x >= 0.0 {
                assert_eq!(*p, start[i], "the source side never moves");
            } else {
                // The mirrored surface is the sphere itself (as a polyhedron, so a hair inside 1).
                worst_before = worst_before.max((start[i].length() - 1.0).abs());
                worst_after = worst_after.max((p.length() - 1.0).abs());
            }
        }
        assert!(worst_before > 3.0 * spacing);
        assert!(worst_after < 0.05 * spacing, "left {worst_after} off the surface, spacing {spacing}");
        // The vertices were slid along the surface, not piled up: no triangle turned over or collapsed.
        let flipped = (0..mesh.triangle_count())
            .filter(|&t| {
                let [a, b, c] = corners(&positions, &mesh.indices, t * 3);
                tri::area_normal(a, b, c).dot(a + b + c) <= 0.0
            })
            .count();
        assert_eq!(flipped, 0);
        // Pressing again finds everything already in place.
        let again = project_onto_mirror(&mut positions, &mesh.indices, &map, true, None, 0.5, ProjectOptions::default());
        assert_eq!(again, 0, "second press moved {again} of {moved}");
    }

    #[test]
    fn a_fully_masked_vertex_never_moves_and_the_edge_is_eased_in() {
        let mesh = octa_sphere(32, 1.0);
        let mut positions = mesh.positions.clone();
        for p in &mut positions {
            if p.x < 0.0 {
                *p *= 1.0 + 0.05 * (-p.x).min(0.4);
            }
        }
        let start = positions.clone();
        let map = SymmetryMap::build(&positions, &mesh.indices, Axis::X, default_tolerance(&positions, &mesh.indices));
        // Free only where y > 0.
        let mask: Vec<f32> = start.iter().map(|p| if p.y > 0.0 { 0.0 } else { 1.0 }).collect();
        let options = ProjectOptions::default();
        let moved = project_onto_mirror(&mut positions, &mesh.indices, &map, true, Some(&mask), 0.5, options);
        assert!(moved > 0);
        let mut eased = 0;
        for (i, p) in positions.iter().enumerate() {
            if mask[i] >= 1.0 || start[i].x >= 0.0 {
                assert_eq!(*p, start[i], "vertex {i} is masked or on the source side");
            } else if start[i].x < -0.3 && start[i].y > 0.5 {
                assert!((p.length() - 1.0).abs() < 2e-3, "deep in the free area the lump is gone");
            } else if start[i].x < -0.3 && start[i].y < 0.05 {
                // Touching the mask: moved only a small part of the way.
                let (was, now) = (start[i].length() - 1.0, p.length() - 1.0);
                assert!(now > 0.5 * was, "vertex {i} next to the mask jumped from {was} to {now}");
                eased += 1;
            }
        }
        assert!(eased > 5);
    }

    #[test]
    fn a_map_without_topology_or_nothing_to_move_is_a_no_op() {
        let mesh = octa_sphere(6, 1.0);
        let mut positions = mesh.positions.clone();
        let cloud = SymmetryMap::build(&positions, &[], Axis::X, 0.01);
        let plain = ProjectOptions::default();
        assert_eq!(project_onto_mirror(&mut positions, &mesh.indices, &cloud, true, None, 0.5, plain), 0);

        let map = SymmetryMap::build(&positions, &mesh.indices, Axis::X, 0.01);
        let everything = vec![1.0; positions.len()];
        assert_eq!(project_onto_mirror(&mut positions, &mesh.indices, &map, true, Some(&everything), 0.5, plain), 0);
        // Symmetric and fully paired: with `unpaired_only` there is nothing left to lay down.
        let options = ProjectOptions { unpaired_only: true, ..Default::default() };
        assert_eq!(project_onto_mirror(&mut positions, &mesh.indices, &map, true, None, 0.5, options), 0);
        assert_eq!(positions, mesh.positions);
    }
}
