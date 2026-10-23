//! Repairs that change topology: replacing one half of a model with a mirror of
//! the other, and merging the duplicated vertices of a seam.
//!
//! Both hand back a whole new mesh, together with which vertex of the input
//! each new vertex was made from, so anything stored per vertex (the mask
//! above all) can be carried across with `carry`. The mirror also makes
//! vertices that were not in the input, where it cuts an edge at the plane;
//! `MirrorWelded::cuts` says which edge each is on and how far along.

use std::sync::atomic::{AtomicBool, Ordering};

use glam::Vec3;
use rayon::prelude::*;
use sculpt_geom::TriMesh;
use sculpt_geom::adjacency::is_degenerate;

use crate::grid::VertexGrid;
use crate::{Axis, par};

const NONE: u32 = u32::MAX;

/// Per-vertex values of the input mesh, re-laid out for the output mesh:
/// `source[k]` is the input vertex output vertex `k` was made from.
pub fn carry<T: Copy + Send + Sync>(source: &[u32], values: &[T]) -> Vec<T> {
    source.par_iter().with_min_len(8192).map(|&i| values[i as usize]).collect()
}

/// What `mirror_and_weld` built.
#[derive(Clone, Debug, PartialEq)]
pub struct MirrorWelded {
    pub mesh: TriMesh,
    /// For each vertex of `mesh`, the input vertex it is: kept where it was,
    /// moved onto the plane (or stood just off it), or reflected across it. A
    /// vertex made where an edge crossed the plane (see `cuts`) names the kept
    /// end of that edge.
    pub source: Vec<u32>,
    /// The vertices made where an edge of the input crossed the plane. They
    /// are the last `cuts.len()` vertices of `mesh`, in this order.
    pub cuts: Vec<CutVertex>,
    /// For each triangle of `mesh`, the input triangle it is (or is a piece
    /// of, where the cut went through it). The triangles come in pairs: one
    /// on the kept side, then its mirror image, both naming the same one.
    pub triangle_source: Vec<u32>,
    /// Triangles of the input kept, whole or in part.
    pub kept_triangles: usize,
    /// The rest of the input's triangles: thrown away and rebuilt.
    pub discarded_triangles: usize,
}

/// A vertex made where an edge of the input crossed the plane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CutVertex {
    /// The end of the edge on the side that was kept.
    pub kept: u32,
    /// The end on the side that was thrown away.
    pub cut_off: u32,
    /// How far along the edge from `kept` to `cut_off` the plane crossed it,
    /// from 0 to 1.
    pub along: f32,
}

impl MirrorWelded {
    /// A per-vertex array of the input, carried onto the new mesh. A reflected
    /// vertex takes the value of the vertex it reflects, and a vertex made on
    /// the plane the value at the kept end of its edge.
    pub fn carry<T: Copy + Send + Sync>(&self, values: &[T]) -> Vec<T> {
        carry(&self.source, values)
    }

    /// `carry` for values that can be mixed (a mask): a vertex made on the
    /// plane takes what the two ends of its edge had, in proportion, which is
    /// what the surface had at that point. So a mask comes out where it was,
    /// mirrored along with the surface, with its edge not moved to the nearest
    /// old vertex along the seam.
    pub fn carry_blended(&self, values: &[f32]) -> Vec<f32> {
        let mut carried = carry(&self.source, values);
        let first_cut = carried.len() - self.cuts.len();
        for (value, cut) in carried[first_cut..].iter_mut().zip(&self.cuts) {
            let (kept, cut_off) = (values[cut.kept as usize], values[cut.cut_off as usize]);
            *value = kept + (cut_off - kept) * cut.along;
        }
        carried
    }
}

/// Where a vertex is once the seam band has been pinned: on the side being
/// thrown away, on the plane itself, or on the side being kept.
const CUT_OFF: u8 = 0;
const ON_PLANE: u8 = 1;
const KEPT: u8 = 2;

/// The plane a model is cut at, and which side of it is kept.
#[derive(Clone, Copy)]
struct Cutter {
    axis: Axis,
    /// +1 when the positive side is kept, -1 when the negative side is.
    sign: f32,
    /// Half the thickness of the band of vertices counted as on the plane.
    seam: f32,
}

impl Cutter {
    /// How far `p` is from the plane, measured into the kept side.
    #[inline]
    fn height(self, p: Vec3) -> f32 {
        self.axis.coord(p) * self.sign
    }

    #[inline]
    fn side(self, p: Vec3) -> u8 {
        let height = self.height(p);
        if height > 0.0 {
            KEPT
        } else if height < 0.0 {
            CUT_OFF
        } else {
            ON_PLANE
        }
    }

    /// `p` stood on the kept side, at the edge of the seam band: clear of the
    /// plane by as much as anything else that is not on it, so the vertex and
    /// its mirror twin are a whole band apart and nothing later (Symmetry
    /// Cleanup above all) takes the two for one seam vertex stored twice.
    fn lifted(self, mut p: Vec3) -> Vec3 {
        p[self.axis.index()] = self.sign * self.seam;
        p
    }
}

/// An edge from a kept vertex to a cut-off one, as one sortable number.
#[inline]
fn crossing_key(kept: u32, cut_off: u32) -> u64 {
    (kept as u64) << 32 | cut_off as u64
}

/// The edge of a triangle from corner `a` to corner `b`, if it crosses the plane.
#[inline]
fn crossing(a: u32, b: u32, side: &[u8]) -> Option<u64> {
    match (side[a as usize], side[b as usize]) {
        (KEPT, CUT_OFF) => Some(crossing_key(a, b)),
        (CUT_OFF, KEPT) => Some(crossing_key(b, a)),
        _ => None,
    }
}

#[inline]
fn corners(indices: &[u32], t: usize) -> [u32; 3] {
    [indices[t * 3], indices[t * 3 + 1], indices[t * 3 + 2]]
}

/// Whether a triangle has a corner on the given side. A triangle with a corner
/// repeated has none anywhere: it covers nothing (a local collapse retires a
/// triangle that way, see `sculpt_geom::adjacency`), so it takes no part in
/// the cut and is left out of the result.
#[inline]
fn reaches(corners: [u32; 3], side: &[u8], which: u8) -> bool {
    !is_degenerate(&corners) && corners.iter().any(|&i| side[i as usize] == which)
}

/// Replaces one whole side of the model with a reflection of the other,
/// cutting the mesh at the plane and rebuilding the discarded half from
/// scratch. Unlike `repair::make_symmetric` it needs NO vertex correspondence,
/// so it works on exactly the models that has to refuse: two arms modelled or
/// joined separately, one side remeshed, an imported asset whose halves never
/// matched. It always produces an exactly symmetric result, at the cost of
/// rebuilding the mesh.
///
/// The cut goes THROUGH the triangles that cross the plane. Each edge from a
/// kept vertex to a cut-off one gets one new vertex exactly on the plane, used
/// by both triangles on that edge, and what is left of each crossing triangle
/// on the kept side (a triangle or a four-sided piece) is kept. The cut edge
/// is therefore planar by construction, the mirrored half SHARES its vertices
/// rather than duplicating them, and the two halves come out already joined:
/// watertight, one shared edge loop, nothing left to weld.
///
/// (This used to keep or drop whole triangles by their centroid and then pin
/// every corner along the ragged edge onto the plane. That moved the surface by
/// up to a triangle, and it went wrong wherever the kept side narrowed to one
/// edge between two pinned corners: the edge then lay in the plane with a kept
/// triangle on each side, the mirror image added its own two, and four
/// triangles shared it. Measured on 40 spiky spheres, each cut six ways with
/// a seam band half a triangle wide: 30 of them came out with such an edge.)
///
/// `seam_tolerance` is the band around the plane whose vertices are pinned
/// onto it before anything is cut. Those are the join where the model already
/// has a centreline, and without the band a vertex a hair off the plane would
/// have a sliver of a triangle cut from every face round it. A pinned vertex
/// the kept side only TOUCHES the plane at is stood back off it, at the edge of
/// the band: see `unpin_where_the_halves_would_only_touch`.
///
/// A model with holes in it comes out open only where it was: along what is
/// left of its own rim, and the mirror image of that. The cut adds no open
/// edge, and where a rim meets the plane the two halves are joined up to it.
///
/// `None` when there is nothing to work with: no geometry, or nothing on the
/// source side of the plane.
pub fn mirror_and_weld(
    positions: &[Vec3],
    indices: &[u32],
    axis: Axis,
    source_is_positive: bool,
    seam_tolerance: f32,
) -> Option<MirrorWelded> {
    let triangle_count = indices.len() / 3;
    if positions.is_empty() || triangle_count == 0 {
        return None;
    }
    let indices = &indices[..triangle_count * 3];
    let cutter = Cutter { axis, sign: if source_is_positive { 1.0 } else { -1.0 }, seam: seam_tolerance.max(1e-6) };

    let in_band = |p: Vec3| axis.coord(p).abs() <= cutter.seam;
    let mut work: Vec<Vec3> =
        positions.par_iter().with_min_len(8192).map(|&p| if in_band(p) { axis.pin(p) } else { p }).collect();
    let mut side: Vec<u8> = work.par_iter().with_min_len(8192).map(|&p| cutter.side(p)).collect();
    unpin_where_the_halves_would_only_touch(cutter, indices, &mut work, &mut side);

    // Every triangle with a corner on the kept side leaves something there.
    let kept = |t: usize| reaches(corners(indices, t), &side, KEPT);
    let kept_triangles = par::count(triangle_count, kept);
    if kept_triangles == 0 {
        return None;
    }

    // One new vertex per crossing edge, whichever of its triangles asks: an
    // edge's place in the list is its vertex.
    let crossings = crossing_edges(indices, &side);
    let cuts: Vec<CutVertex> = crossings
        .par_iter()
        .with_min_len(4096)
        .map(|&key| {
            let (kept, cut_off) = ((key >> 32) as u32, key as u32);
            let (from, to) = (cutter.height(work[kept as usize]), cutter.height(work[cut_off as usize]));
            CutVertex { kept, cut_off, along: from / (from - to) }
        })
        .collect();
    let cut_positions: Vec<Vec3> = cuts
        .par_iter()
        .with_min_len(4096)
        .map(|cut| {
            let (from, to) = (work[cut.kept as usize], work[cut.cut_off as usize]);
            axis.pin(from + (to - from) * cut.along)
        })
        .collect();

    // The old vertices that survive, each off-plane one with a reflected twin
    // in the slot after its own; the new vertices on the plane follow, in the
    // order of their edges.
    let (first, mut source) = surviving_vertices(indices, &side);
    let first_cut = source.len();
    assert!(first_cut + cuts.len() < NONE as usize, "the mirrored mesh would have too many vertices to number");
    source.extend(cuts.iter().map(|cut| cut.kept));

    // A corner of the output: the vertex on the kept side, the one that takes
    // its place in the mirror image (itself, on the plane), and where it is.
    type Corner = (u32, u32, Vec3);
    let old_corner = |i: u32| -> Corner {
        let slot = first[i as usize];
        (slot, slot + (side[i as usize] == KEPT) as u32, work[i as usize])
    };
    let cut_corner = |key: u64| -> Corner {
        let rank = crossings.binary_search(&key).expect("every crossing edge was listed");
        let slot = (first_cut + rank) as u32;
        (slot, slot, cut_positions[rank])
    };
    // Reflection flips handedness, so the mirrored copy is wound the other way
    // round or the whole new half would face inward. The swap is of the LAST
    // two corners, keeping the first in place, so a normal pass that takes a
    // face's edges from that corner sums exactly negated terms on the two
    // halves.
    // Each with the input triangle it comes from, fourth.
    let emit_from = |t: usize| {
        move |out: &mut Vec<[u32; 4]>, a: Corner, b: Corner, c: Corner| {
            out.extend([[a.0, b.0, c.0, t as u32], [a.1, c.1, b.1, t as u32]])
        }
    };

    let out_triangles: Vec<[u32; 4]> = par::gather(triangle_count, |t, out| {
        if !kept(t) {
            // Wholly cut off, or lying IN the plane: such a triangle is its own
            // mirror image and would come out as two coincident faces.
            return;
        }
        let emit = emit_from(t);
        let corners = corners(indices, t);
        let sides = corners.map(|i| side[i as usize]);
        if !sides.contains(&CUT_OFF) {
            let [a, b, c] = corners.map(old_corner);
            emit(out, a, b, c);
            return;
        }
        // What is left of a crossing triangle on the kept side: going round
        // it, each corner not cut off, and a new vertex wherever an edge
        // crosses the plane. Three corners or four.
        let mut piece = [(0, 0, Vec3::ZERO); 4];
        let mut len = 0;
        for k in 0..3 {
            let (a, b) = (corners[k], corners[(k + 1) % 3]);
            if sides[k] != CUT_OFF {
                piece[len] = old_corner(a);
                len += 1;
            }
            if let Some(key) = crossing(a, b, &side) {
                piece[len] = cut_corner(key);
                len += 1;
            }
        }
        let [a, b, c, d] = piece;
        if len == 3 {
            emit(out, a, b, c);
        } else if a.2.distance_squared(c.2) <= b.2.distance_squared(d.2) {
            // Four corners: split along the shorter diagonal, for the squarer pair of triangles.
            emit(out, a, b, c);
            emit(out, a, c, d);
        } else {
            emit(out, a, b, d);
            emit(out, b, c, d);
        }
    });

    let mut out_positions: Vec<Vec3> = Vec::with_capacity(source.len());
    out_positions.par_extend(source[..first_cut].par_iter().with_min_len(8192).enumerate().map(|(k, &i)| {
        if first[i as usize] == k as u32 { work[i as usize] } else { axis.reflect(work[i as usize]) }
    }));
    out_positions.extend(cut_positions);
    let out_indices: Vec<u32> = out_triangles.par_iter().with_min_len(8192).flat_map_iter(|t| [t[0], t[1], t[2]]).collect();
    let triangle_source: Vec<u32> = out_triangles.par_iter().with_min_len(8192).map(|t| t[3]).collect();

    Some(MirrorWelded {
        mesh: TriMesh::new(out_positions, out_indices),
        source,
        cuts,
        triangle_source,
        kept_triangles,
        discarded_triangles: triangle_count - kept_triangles,
    })
}

/// Every edge that runs from a kept vertex to a cut-off one, once each, in
/// order (see `crossing_key`). There are only as many as the cut is long.
fn crossing_edges(indices: &[u32], side: &[u8]) -> Vec<u64> {
    let mut crossings: Vec<u64> = par::gather(indices.len() / 3, |t, out| {
        let [a, b, c] = corners(indices, t);
        if !is_degenerate(&[a, b, c]) {
            out.extend([crossing(a, b, side), crossing(b, c, side), crossing(c, a, side)].into_iter().flatten());
        }
    });
    par::sort(&mut crossings);
    crossings.dedup();
    crossings
}

/// The input's vertices that are in the result: every kept one a triangle
/// uses, and those on the plane that a kept triangle reaches. Returns each
/// one's slot in the result (`NONE` for the rest) and, slot by slot, the input
/// vertex it holds.
///
/// A kept vertex has TWO slots, its own and then its reflected twin's. A vertex
/// on the plane has one: it IS the join, and duplicating it is exactly what
/// would leave the result as two shells touching rather than one closed
/// surface.
fn surviving_vertices(indices: &[u32], side: &[u8]) -> (Vec<u32>, Vec<u32>) {
    let used: Vec<AtomicBool> =
        (0..side.len()).into_par_iter().with_min_len(8192).map(|_| AtomicBool::new(false)).collect();
    (0..indices.len() / 3).into_par_iter().with_min_len(4096).for_each(|t| {
        let corners = corners(indices, t);
        if reaches(corners, side, KEPT) {
            for i in corners.into_iter().filter(|&i| side[i as usize] != CUT_OFF) {
                used[i as usize].store(true, Ordering::Relaxed);
            }
        }
    });

    let mut first = vec![NONE; side.len()];
    let mut source = Vec::with_capacity(side.len());
    for (i, flag) in used.into_iter().enumerate() {
        if !flag.into_inner() {
            continue;
        }
        first[i] = source.len() as u32;
        source.push(i as u32);
        if side[i] == KEPT {
            source.push(i as u32);
        }
    }
    (first, source)
}

/// Takes back off the plane the pinned vertices that the kept side and its
/// mirror image must not share, standing each at the edge of the seam band.
///
/// A vertex on the plane is where the two halves are joined, and the join is
/// only sound where the kept side ENDS at the plane: round each such vertex the
/// kept triangles must be one fan, running from one edge of the cut to the
/// next (or, at the rim of a model with a hole in it, from an edge of the cut
/// to the rim). Where the kept side merely touches the plane the mirror image
/// touches it at the same place, and sharing the vertex would stick the two
/// together there:
///
/// * a ridge lying along the plane, an edge with both ends pinned and a kept
///   triangle on each side: shared, that edge would carry four triangles;
/// * a spike with only its tip on the plane, or two parts of the cut meeting at
///   one pinned vertex: every edge would have its two triangles, but the
///   surface would be pinched to a point there;
/// * on an open model, a rim vertex the kept side comes to from both sides of a
///   notch in the rim: two fans, each ending at the cut, pinched the same way.
///
/// Stood off the plane, such a vertex gets a mirror twin like any other kept
/// vertex and the halves pass close by each other without touching. Taking a
/// vertex off the plane changes what its neighbours on the plane see, so this
/// goes round until nothing more turns up. It is usually done in one look, and
/// it only ever looks at the triangles that have a corner on the plane.
fn unpin_where_the_halves_would_only_touch(cutter: Cutter, indices: &[u32], work: &mut [Vec3], side: &mut [u8]) {
    let near = par::indices_where(indices.len() / 3, |t| reaches(corners(indices, t), side, ON_PLANE));
    loop {
        let touching = pinned_vertices_only_touched(indices, &near, side);
        if touching.is_empty() {
            return;
        }
        for &v in &touching {
            work[v as usize] = cutter.lifted(work[v as usize]);
            side[v as usize] = KEPT;
        }
    }
}

/// The vertices on the plane where the kept side does not simply end: both
/// ends of every on-plane edge with a kept triangle on each side, and every
/// vertex whose kept triangles are anything other than one fan with an end on
/// the cut. Ascending, each once.
///
/// Round an on-plane vertex the kept triangles fall into fans, each a run of
/// triangles joined across the edges from that vertex to a kept one. A fan has
/// two ends, and an end is one of two things:
///
/// * An edge of the cut, where the fan's mirror image is joined to it. A kept
///   triangle has one at an on-plane corner in two ways: an edge to another
///   on-plane corner, or, with its third corner cut off, the new edge from that
///   corner to where the plane crosses the opposite side.
/// * An edge to a kept vertex with no triangle on its other side: the rim of a
///   model that was open there to begin with.
///
/// The vertex may stay on the plane when that comes to ONE fan, joined to its
/// mirror image: two ends in all, at least one of them on the cut. A closed
/// model has no rim, so there this is a count of the edges of the cut alone:
/// two are one fan, none is the tip of a spike, four or more are parts of the
/// cut meeting. On an open model that count is not enough, and the rim ends
/// have to be counted with it: two edges of the cut can as well be two fans,
/// each with its other end on the rim (the kept side coming to a rim vertex
/// from either side of a notch), and each would be joined to its own mirror
/// image and to the other pair at that one point only.
fn pinned_vertices_only_touched(indices: &[u32], near: &[u32], side: &[u8]) -> Vec<u32> {
    // The two edges each kept triangle has at each of its on-plane corners, as (that vertex,
    // the other end): one to a kept vertex names it, and one that ends the fan at the cut names none.
    let mut fan_edges: Vec<u64> = par::gather(near.len(), |k, out| {
        let corners = corners(indices, near[k] as usize);
        let sides = corners.map(|i| side[i as usize]);
        if !sides.contains(&KEPT) {
            return;
        }
        for at in (0..3).filter(|&at| sides[at] == ON_PLANE) {
            for other in [(at + 1) % 3, (at + 2) % 3] {
                let kept_end = if sides[other] == KEPT { corners[other] } else { NONE };
                out.push((corners[at] as u64) << 32 | kept_end as u64);
            }
        }
    });
    let mut on_plane_edges: Vec<u64> = par::gather(near.len(), |k, out| {
        let corners = corners(indices, near[k] as usize);
        let sides = corners.map(|i| side[i as usize]);
        let on_plane = |c: usize| sides[c % 3] == ON_PLANE;
        if let Some(c) = (0..3).find(|&c| sides[c] == KEPT && on_plane(c + 1) && on_plane(c + 2)) {
            let (a, b) = (corners[(c + 1) % 3], corners[(c + 2) % 3]);
            out.push((a.min(b) as u64) << 32 | a.max(b) as u64);
        }
    });
    par::sort(&mut fan_edges);
    par::sort(&mut on_plane_edges);

    let mut touching: Vec<u32> = on_plane_edges
        .windows(2)
        .filter(|pair| pair[0] == pair[1])
        .flat_map(|pair| [(pair[0] >> 32) as u32, pair[0] as u32])
        .collect();
    for at_one_vertex in fan_edges.chunk_by(|a, b| a >> 32 == b >> 32) {
        // Sorted, so the edges of the cut come last; before them, an edge listed once has a
        // triangle on one side only.
        let on_the_cut = at_one_vertex.iter().rev().take_while(|&&edge| edge as u32 == NONE).count();
        let to_kept = &at_one_vertex[..at_one_vertex.len() - on_the_cut];
        let on_the_rim = to_kept.chunk_by(|a, b| a == b).filter(|same| same.len() == 1).count();
        if on_the_cut == 0 || on_the_cut + on_the_rim != 2 {
            touching.push((at_one_vertex[0] >> 32) as u32);
        }
    }
    touching.sort_unstable();
    touching.dedup();
    touching
}

/// How many rounds `weld` settles across cores before finishing in order.
const WELD_ROUNDS: usize = 6;

/// What `weld` built.
#[derive(Clone, Debug, PartialEq)]
pub struct Welded {
    pub mesh: TriMesh,
    /// For each vertex of `mesh`, the input vertex that survived as it.
    pub source: Vec<u32>,
    /// For each vertex of the input, the vertex of `mesh` it became: its own,
    /// or the one it was merged into.
    pub target: Vec<u32>,
}

impl Welded {
    /// A per-vertex array of the input (a mask), carried onto the new mesh.
    /// Where vertices were merged the survivor's value is the one kept.
    pub fn carry<T: Copy + Send + Sync>(&self, values: &[T]) -> Vec<T> {
        carry(&self.source, values)
    }

    /// How many vertices were merged away.
    pub fn merged(&self) -> usize {
        self.target.len() - self.source.len()
    }
}

/// Merges vertices that occupy the same point into single shared ones,
/// remapping the triangles onto the survivors and dropping the triangles that
/// collapse to nothing.
///
/// Runs at TWO different radii, which is the whole design:
///
/// * Between two vertices ON the mirror plane, generously (`seam_tolerance`).
///   `snap_to_plane` has just pulled a band of vertices onto the plane, and the
///   pair that used to straddle it now needs to become one. This is the case
///   joining two mirrored halves creates: the centreline carries two coincident
///   vertices for every one it should have, and the model only LOOKS closed. It
///   is two shells touching, which splits apart under smoothing and remeshes
///   as a doubled surface.
///
/// * Everywhere else only at `coincident_epsilon`, small enough to catch
///   vertices that are literally the same point and nothing else: a UV seam, a
///   pole split once per surrounding triangle.
///
/// The two radii are what keeps this safe. A whole-mesh weld at the seam
/// tolerance would also collapse fine sculpted detail that happens to sit
/// closer together than that, far from the plane, which a symmetry tool has no
/// business doing. And the generous radius applies only when BOTH ends are on
/// the plane: decided from one end alone it let an on-seam vertex swallow an
/// off-seam one up to a whole seam width away, dragging the centreline
/// sideways into whatever geometry sat near it.
///
/// A vertex is merged into the NEAREST earlier survivor in reach (the lower
/// index on a tie), so the result does not depend on how anything was
/// bucketed.
///
/// `None` when nothing needed merging, so the caller can skip a rebuild it
/// does not need.
pub fn weld(
    positions: &[Vec3],
    indices: &[u32],
    axis: Axis,
    seam_tolerance: f32,
    coincident_epsilon: f32,
) -> Option<Welded> {
    weld_in_rounds(positions, indices, axis, seam_tolerance, coincident_epsilon, WELD_ROUNDS)
}

/// Merges every group of vertices closer together than `distance` (at least a
/// millionth of a unit), wherever on the mesh they are: `weld` without a
/// mirror plane, for stitching parts that were laid edge to edge. `None` when
/// nothing needed merging.
pub fn weld_coincident(positions: &[Vec3], indices: &[u32], distance: f32) -> Option<Welded> {
    // With the two radii equal, which vertices count as on the seam no longer matters.
    weld(positions, indices, Axis::X, distance, distance)
}

/// `weld`, settling for at most `rounds` rounds across cores before finishing
/// in vertex order. The answer is the same for any number of rounds.
fn weld_in_rounds(
    positions: &[Vec3],
    indices: &[u32],
    axis: Axis,
    seam_tolerance: f32,
    coincident_epsilon: f32,
    rounds: usize,
) -> Option<Welded> {
    let n = positions.len();
    let seam = seam_tolerance.max(1e-6);
    let coincident = coincident_epsilon.clamp(1e-7, seam);
    let (seam_sqr, coincident_sqr) = (seam * seam, coincident * coincident);

    // "On the seam" means ON the plane: the seam is snapped before it is
    // welded. Testing the band instead also handed the generous radius to real
    // mirrored pairs straddling the plane half a spacing out, which on a remesh
    // is the whole first row either side.
    let on_seam = |p: Vec3| axis.coord(p) == 0.0;
    let reach = |p: Vec3| if on_seam(p) { seam } else { coincident };
    let within = |p: Vec3, q: Vec3, d: f32| d <= if on_seam(p) && on_seam(q) { seam_sqr } else { coincident_sqr };
    let grid = VertexGrid::build(positions, seam);

    // Nearly every vertex of a real mesh has nothing in reach at all. Those
    // can neither be merged nor be merged into, and finding them is spread
    // across cores; what is left is usually small.
    let mut undecided = par::indices_where(n, |i| {
        let p = positions[i];
        let mut any = false;
        grid.for_each_within(p, reach(p), |j, q, d| any |= j as usize != i && within(p, q, d));
        any
    });

    // The rule is the plain one: in vertex order, each vertex joins the
    // nearest survivor before it, or survives. `into[i]` is where vertex `i`
    // ends up: itself for a survivor.
    let mut into: Vec<u32> = (0..n as u32).collect();
    undecided.iter().for_each(|&i| into[i as usize] = NONE);

    // What that rule gives vertex `i`, if it can be told yet. It can once
    // every earlier vertex in reach is settled, or once a survivor is in reach
    // that nothing still unsettled could beat (nearer, or as near with a lower
    // index). The second case is what settles a pile of coincident copies in
    // one go instead of one copy at a time.
    let settle = |i: u32, into: &[u32]| -> Option<u32> {
        let p = positions[i as usize];
        // Nearest so far as (squared distance, vertex), which also orders ties by index.
        let (mut survivor, mut pending) = (None::<(f32, u32)>, None::<(f32, u32)>);
        grid.for_each_within(p, reach(p), |j, q, d| {
            if j >= i || !within(p, q, d) {
                return;
            }
            let fate = into[j as usize];
            let best = if fate == NONE { &mut pending } else { &mut survivor };
            if (fate == NONE || fate == j) && best.is_none_or(|b| (d, j) < b) {
                *best = Some((d, j));
            }
        });
        match (survivor, pending) {
            (survivor, None) => Some(survivor.map_or(i, |(_, j)| j)),
            (Some(survivor), Some(pending)) if survivor < pending => Some(survivor.1),
            _ => None,
        }
    };

    // A mesh stored as loose triangles has every vertex several times over,
    // so "what is left" can be all of it. A few rounds across cores settle
    // nearly everything then; each round's verdicts depend only on vertices
    // settled before it, so they are the same ones the plain sweep would reach.
    for _ in 0..rounds {
        if undecided.len() < 4096 {
            break;
        }
        let settled: Vec<(u32, u32)> = par::gather(undecided.len(), |k, out| {
            if let Some(target) = settle(undecided[k], &into) {
                out.push((undecided[k], target));
            }
        });
        settled.iter().for_each(|&(i, target)| into[i as usize] = target);
        undecided.retain(|&i| into[i as usize] == NONE);
    }
    // The rest in order: by the time a vertex comes up, everything before it is settled.
    for &i in &undecided {
        into[i as usize] = settle(i, &into).expect("every earlier vertex is settled");
    }
    let merged = par::count(n, |i| into[i] != i as u32);
    if merged == 0 {
        return None;
    }

    // Compact: survivors keep their order, merged-away vertices disappear.
    let mut slot = vec![NONE; n];
    let mut source = Vec::with_capacity(n - merged);
    for i in 0..n {
        if into[i] == i as u32 {
            slot[i] = source.len() as u32;
            source.push(i as u32);
        }
    }
    let target: Vec<u32> = into.par_iter().with_min_len(8192).map(|&j| slot[j as usize]).collect();

    // Two corners merged into one leaves a triangle with no area; those go.
    let out_indices = par::gather(indices.len() / 3, |t, out| {
        let [a, b, c] = [indices[t * 3], indices[t * 3 + 1], indices[t * 3 + 2]].map(|i| target[i as usize]);
        if a != b && b != c && a != c {
            out.extend([a, b, c]);
        }
    });
    let out_positions = carry(&source, positions);
    Some(Welded { mesh: TriMesh::new(out_positions, out_indices), source, target })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{
        Faults, Lcg, bumpy_sphere, dented_sphere, duplicate_positions, edge_census, exact_partners, octa_sphere,
        pinched_vertices, torus,
    };
    use glam::{DVec3, Mat4, Quat};
    use sculpt_geom::{MeshAdjacency, MeshEdges};
    use std::collections::{HashMap, HashSet};

    /// The triangles whose centroid is on the chosen side of the X plane, with
    /// unused vertices left in (as a trimmed mesh would have them).
    fn half(mesh: &TriMesh, positive: bool) -> TriMesh {
        let indices: Vec<u32> = (0..mesh.triangle_count())
            .filter(|&t| (mesh.triangle(t).iter().map(|p| p.x).sum::<f32>() > 0.0) == positive)
            .flat_map(|t| mesh.indices[t * 3..t * 3 + 3].to_vec())
            .collect();
        TriMesh::new(mesh.positions.clone(), indices)
    }

    fn assert_closed_and_symmetric(mesh: &TriMesh, axis: Axis, label: &str) {
        assert_eq!(edge_census(mesh), (0, 0), "{label}: open or inconsistently wound");
        assert!(mesh.signed_volume() > 0.0, "{label}: wound inward");
        assert_eq!(duplicate_positions(&mesh.positions), 0, "{label}: duplicate vertices");
        assert!(exact_partners(&mesh.positions, axis).is_some(), "{label}: not mirror-exact");
    }

    #[test]
    fn half_a_sphere_becomes_a_whole_closed_one() {
        let n = 12;
        let full = octa_sphere(n, 1.0);
        for positive in [true, false] {
            let open = half(&full, positive);
            assert!(edge_census(&open).0 > 0, "the half is open along the plane");
            let welded = mirror_and_weld(&open.positions, &open.indices, Axis::X, positive, 1e-4).unwrap();

            assert_closed_and_symmetric(&welded.mesh, Axis::X, "half sphere");
            assert_eq!(welded.mesh.vertex_count(), full.vertex_count(), "one vertex per seam point, not two");
            assert_eq!(welded.mesh.triangle_count(), full.triangle_count());
            assert_eq!(welded.kept_triangles, full.triangle_count() / 2);
            assert_eq!(welded.discarded_triangles, 0);
            assert!((welded.mesh.signed_volume() - full.signed_volume()).abs() < 1e-9 * full.signed_volume());
        }
    }

    #[test]
    fn each_triangle_names_the_one_it_was_cut_from_and_its_mirror_image_the_same() {
        let full = octa_sphere(10, 1.0);
        // Turned a little, so that the plane goes through triangles and not along their edges.
        let turned: Vec<Vec3> = full.positions.iter().map(|p| Vec3::new(p.x * 0.96 + p.y * 0.28, p.y * 0.96 - p.x * 0.28, p.z)).collect();
        let welded = mirror_and_weld(&turned, &full.indices, Axis::X, true, 1e-4).unwrap();
        let mesh = &welded.mesh;
        assert_eq!(welded.triangle_source.len(), mesh.triangle_count());
        for (t, pair) in welded.triangle_source.chunks_exact(2).enumerate() {
            assert_eq!(pair[0], pair[1], "a triangle and its mirror image come from one");
            // The kept one lies within the triangle it came from (a piece of it, or the whole).
            let from: Vec<Vec3> = full.indices[pair[0] as usize * 3..pair[0] as usize * 3 + 3].iter().map(|&v| turned[v as usize]).collect();
            let normal = (from[1] - from[0]).cross(from[2] - from[0]).normalize();
            for p in mesh.triangle(t * 2) {
                assert!((p - from[0]).dot(normal).abs() < 1e-5, "triangle {t} is off the plane of the one it names");
            }
        }
        let sources: HashSet<u32> = welded.triangle_source.iter().copied().collect();
        assert_eq!(sources.len(), welded.kept_triangles);
    }

    #[test]
    fn a_whole_sphere_keeps_one_side_and_rebuilds_the_other() {
        let full = octa_sphere(10, 1.0);
        // Dent the -X side; mirroring +X over it must erase the dent.
        let mut dented = full.clone();
        for p in &mut dented.positions {
            if p.x < -0.5 {
                *p *= 0.8;
            }
        }
        let welded = mirror_and_weld(&dented.positions, &dented.indices, Axis::X, true, 1e-4).unwrap();
        assert_closed_and_symmetric(&welded.mesh, Axis::X, "dented sphere");
        assert_eq!(welded.kept_triangles, full.triangle_count() / 2);
        assert_eq!(welded.discarded_triangles, full.triangle_count() / 2);
        assert!((welded.mesh.signed_volume() - full.signed_volume()).abs() < 1e-9);

        // The other way round keeps the dent and mirrors it.
        let other = mirror_and_weld(&dented.positions, &dented.indices, Axis::X, false, 1e-4).unwrap();
        assert_closed_and_symmetric(&other.mesh, Axis::X, "dent mirrored");
        assert!(other.mesh.signed_volume() < full.signed_volume() * 0.95);
    }

    /// No vertex on the plane and every triangle near it straddling: the cut still comes out
    /// planar and closed at ANY tolerance, because the rim is pinned by construction.
    #[test]
    fn a_tilted_mesh_is_cut_planar_and_closed_at_any_tolerance() {
        let mut mesh = octa_sphere(14, 1.0);
        mesh.transform(&Mat4::from_quat(Quat::from_euler(glam::EulerRot::XYZ, 0.31, 0.47, 0.83)));
        for axis in Axis::ALL {
            for tolerance in [1e-6, 1e-3, 0.02] {
                for positive in [true, false] {
                    let label = format!("{axis:?} tol {tolerance} positive {positive}");
                    let welded = mirror_and_weld(&mesh.positions, &mesh.indices, axis, positive, tolerance).unwrap();
                    assert_closed_and_symmetric(&welded.mesh, axis, &label);
                    assert_eq!(welded.kept_triangles + welded.discarded_triangles, mesh.triangle_count());
                    // The cut moves the rim by at most a triangle, so the volume stays close.
                    assert!((welded.mesh.signed_volume() / mesh.signed_volume() - 1.0).abs() < 0.03, "{label}");
                    // Seam vertices are exactly on the plane and appear once.
                    let seam = welded.mesh.positions.iter().filter(|p| axis.coord(**p) == 0.0).count();
                    assert!(seam > 20, "{label}");
                }
            }
        }
    }

    /// A mask painted on the kept side comes out mirrored on the rebuilt side, and along the cut
    /// it is what the surface had at each new vertex.
    #[test]
    fn a_mask_survives_and_is_mirrored() {
        let mut mesh = octa_sphere(9, 1.0);
        mesh.transform(&Mat4::from_rotation_y(0.2));
        let mask: Vec<f32> = mesh.positions.iter().map(|p| (p.y * 3.0).sin() * 0.5 + 0.5).collect();
        let welded = mirror_and_weld(&mesh.positions, &mesh.indices, Axis::X, true, 1e-4).unwrap();
        let carried = welded.carry_blended(&mask);
        assert_eq!(carried.len(), welded.mesh.vertex_count());
        let first_cut = welded.mesh.vertex_count() - welded.cuts.len();
        assert!(welded.cuts.len() > 20, "the plane runs between this mesh's rows of vertices");
        assert_eq!(carried[..first_cut], welded.carry(&mask)[..first_cut]);

        let partners = exact_partners(&welded.mesh.positions, Axis::X).unwrap();
        for (k, &twin) in partners.iter().enumerate() {
            assert_eq!(carried[k], carried[twin as usize], "vertex {k} and its mirror image disagree");
            let source = welded.source[k] as usize;
            let (p, from) = (welded.mesh.positions[k], mesh.positions[source]);
            if k < first_cut {
                // An old vertex: where it was, or moved straight onto the plane or across it.
                assert_eq!(carried[k], mask[source]);
                assert!((p.y, p.z) == (from.y, from.z) && p.x.abs() <= from.x.abs());
                continue;
            }
            // A new one: on the plane, on the edge between a kept vertex and a cut-off one.
            let cut = welded.cuts[k - first_cut];
            let (to, far) = (mesh.positions[cut.cut_off as usize], mask[cut.cut_off as usize]);
            assert_eq!((cut.kept as usize, twin as usize, p.x), (source, k, 0.0));
            assert!(from.x > 0.0 && to.x < 0.0 && cut.along > 0.0 && cut.along < 1.0);
            assert!(p.distance(from.lerp(to, cut.along)) < 1e-6);
            assert!((carried[k] - (mask[source] + (far - mask[source]) * cut.along)).abs() < 1e-6);
        }
    }

    /// The plainest cut there is: a tetrahedron with one corner on one side of the plane and
    /// three on the other, every crossing edge cut at its middle.
    #[test]
    fn a_tetrahedron_is_cut_through_its_edges() {
        let corner = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
        let far = [corner(-1.0, 1.0, 0.0), corner(-1.0, -1.0, 1.0), corner(-1.0, -1.0, -1.0)];
        let positions = vec![corner(1.0, 0.0, 0.0), far[0], far[1], far[2]];
        let mut tetra = TriMesh::new(positions, vec![0, 1, 2, 0, 2, 3, 0, 3, 1, 1, 3, 2]);
        if tetra.signed_volume() < 0.0 {
            tetra.flip();
        }
        let volume = tetra.signed_volume();

        // Keeping the tip: three faces lose their far ends and the fourth goes. Half the size
        // is an eighth of the volume, twice over.
        let tip = mirror_and_weld(&tetra.positions, &tetra.indices, Axis::X, true, 1e-4).unwrap();
        assert_eq!(Faults::of(&tip.mesh), Faults::NONE);
        assert_closed_and_symmetric(&tip.mesh, Axis::X, "tip");
        assert_eq!((tip.mesh.vertex_count(), tip.mesh.triangle_count()), (5, 6));
        assert_eq!((tip.kept_triangles, tip.discarded_triangles), (3, 1));
        let halfway = |cut_off| CutVertex { kept: 0, cut_off, along: 0.5 };
        assert_eq!(tip.cuts, [halfway(1), halfway(2), halfway(3)]);
        assert_eq!(tip.source, [0, 0, 0, 0, 0]);
        assert!((tip.mesh.signed_volume() - volume / 4.0).abs() < 1e-6);
        // The tip, its mirror image, then the three new vertices: each halfway to its far end.
        assert_eq!(tip.carry_blended(&[0.0, 1.0, 0.5, 0.25]), [0.0, 0.0, 0.5, 0.25, 0.125]);
        assert_eq!(tip.carry(&[7, 1, 2, 3]), [7; 5]);

        // Keeping the base: it stays whole, and each of the other three faces is left four-sided.
        let base = mirror_and_weld(&tetra.positions, &tetra.indices, Axis::X, false, 1e-4).unwrap();
        assert_eq!(Faults::of(&base.mesh), Faults::NONE);
        assert_closed_and_symmetric(&base.mesh, Axis::X, "base");
        assert_eq!((base.mesh.vertex_count(), base.mesh.triangle_count()), (9, 14));
        assert_eq!((base.kept_triangles, base.discarded_triangles, base.cuts.len()), (4, 0, 3));
        assert!((base.mesh.signed_volume() - volume * 7.0 / 4.0).abs() < 1e-6);
    }

    /// A model that does not reach the plane has nothing to be cut: kept, it comes out twice,
    /// as two separate closed shells; asked for from the empty side, it is refused.
    #[test]
    fn a_model_wholly_on_one_side_is_doubled_or_refused() {
        for (reach, what) in [(2.5, "clear of the plane"), (1.0005, "reaching into the seam band")] {
            let mut ball = octa_sphere(6, 1.0);
            ball.transform(&Mat4::from_translation(Vec3::new(reach, 0.3, 0.0)));
            let welded = mirror_and_weld(&ball.positions, &ball.indices, Axis::X, true, 1e-3).unwrap();
            assert_eq!(Faults::of(&welded.mesh), Faults::NONE, "{what}");
            assert_closed_and_symmetric(&welded.mesh, Axis::X, what);
            assert_eq!(welded.mesh.vertex_count(), 2 * ball.vertex_count(), "{what}");
            assert_eq!(welded.mesh.triangle_count(), 2 * ball.triangle_count(), "{what}");
            assert_eq!((welded.kept_triangles, welded.discarded_triangles), (ball.triangle_count(), 0), "{what}");
            assert!(welded.cuts.is_empty(), "{what}");
            assert!(mirror_and_weld(&ball.positions, &ball.indices, Axis::X, false, 1e-3).is_none(), "{what}");
        }
    }

    #[test]
    fn nothing_on_the_source_side_is_refused() {
        let full = octa_sphere(4, 1.0);
        let negative = half(&full, false);
        assert!(mirror_and_weld(&negative.positions, &negative.indices, Axis::X, true, 1e-4).is_none());
        assert!(mirror_and_weld(&negative.positions, &negative.indices, Axis::X, false, 1e-4).is_some());
        assert!(mirror_and_weld(&[], &[], Axis::X, true, 1e-4).is_none());
        assert!(mirror_and_weld(&full.positions, &[], Axis::X, true, 1e-4).is_none());
    }

    #[test]
    fn a_triangle_lying_in_the_plane_is_not_doubled() {
        // A tetrahedron with one face in the X = 0 plane.
        let positions = vec![Vec3::ZERO, Vec3::Y, Vec3::Z, Vec3::new(1.0, 0.3, 0.3)];
        let indices = vec![0, 2, 1, 0, 1, 3, 1, 2, 3, 2, 0, 3];
        let tetra = TriMesh::new(positions, indices);
        assert!(tetra.signed_volume() > 0.0);
        let welded = mirror_and_weld(&tetra.positions, &tetra.indices, Axis::X, true, 1e-4).unwrap();
        assert_eq!(welded.mesh.vertex_count(), 5);
        assert_eq!(welded.mesh.triangle_count(), 6, "the in-plane face is gone, the other three are doubled");
        assert_closed_and_symmetric(&welded.mesh, Axis::X, "double tetrahedron");
        assert!((welded.mesh.signed_volume() - 2.0 * tetra.signed_volume()).abs() < 1e-9);
    }

    /// The volume of the part of a closed model on one side of a plane, worked out separately
    /// from the cut under test: each triangle clipped to that side in double precision and its
    /// share of the enclosed volume summed. (The flat face the plane would add contributes
    /// nothing, measured from the plane as this is.)
    fn kept_volume(mesh: &TriMesh, axis: Axis, positive: bool) -> f64 {
        let sign = if positive { 1.0 } else { -1.0 };
        let height = |p: DVec3| p[axis.index()] * sign;
        let mut volume = 0.0;
        for t in 0..mesh.triangle_count() {
            let corners = mesh.triangle(t).map(|p| p.as_dvec3());
            let mut kept: Vec<DVec3> = Vec::with_capacity(4);
            for k in 0..3 {
                let (a, b) = (corners[k], corners[(k + 1) % 3]);
                if height(a) > 0.0 {
                    kept.push(a);
                }
                if (height(a) > 0.0) != (height(b) > 0.0) {
                    kept.push(a + (b - a) * (height(a) / (height(a) - height(b))));
                }
            }
            for k in 2..kept.len() {
                let (a, b, c) = (kept[0], kept[k - 1], kept[k]);
                volume += height((b - a).cross(c - a)) * (height(a) + height(b) + height(c)) / 6.0;
            }
        }
        volume
    }

    /// What cutting one model every way got wrong.
    #[derive(Default)]
    struct Wrong {
        /// Cuts that are not one sound closed surface, or not exactly symmetric.
        broken: Vec<String>,
        /// Sound cuts that do not enclose twice what the kept side enclosed.
        misshapen: Vec<String>,
    }

    /// Cuts `mesh` on every plane, keeping each side in turn, and lists the cuts that are not a
    /// sound closed surface, exactly symmetric, enclosing twice what the kept side enclosed (to
    /// within what the seam band may move).
    fn wrong_cuts(mesh: &TriMesh, tolerance: f32, label: &str) -> Wrong {
        let area: f64 = (0..mesh.triangle_count())
            .map(|t| mesh.triangle(t))
            .map(|[a, b, c]| (b - a).cross(c - a).length() as f64 * 0.5)
            .sum();
        let mut wrong = Wrong::default();
        for axis in Axis::ALL {
            for positive in [true, false] {
                let label = format!("{label}, {}{} kept", if positive { '+' } else { '-' }, axis.name());
                let Some(welded) = mirror_and_weld(&mesh.positions, &mesh.indices, axis, positive, tolerance) else {
                    wrong.broken.push(format!("{label}: refused"));
                    continue;
                };
                let faults = Faults::of(&welded.mesh);
                let (volume, expected) = (welded.mesh.signed_volume(), 2.0 * kept_volume(mesh, axis, positive));
                if faults != Faults::NONE {
                    wrong.broken.push(format!("{label}: {faults:?}"));
                } else if exact_partners(&welded.mesh.positions, axis).is_none() {
                    wrong.broken.push(format!("{label}: not mirror-exact"));
                } else if (volume - expected).abs() > 2.0 * tolerance as f64 * area + 1e-4 * expected {
                    wrong.misshapen.push(format!("{label}: encloses {volume}, twice the kept side is {expected}"));
                }
            }
        }
        wrong
    }

    /// A seam band far below the triangle size, a usual one, and one about half a triangle wide
    /// (what the Match Tolerance slider gives on a dense model).
    const SEAM_BANDS: [f32; 3] = [1e-6, 1e-4, 0.08];

    /// Lumpy, rough models, which the plane crosses at a slant and more than once. Cutting whole
    /// triangles and pinning their corners left edges here with four triangles on them: an edge
    /// with both ends pinned onto the plane and a kept triangle either side was doubled by its
    /// own mirror image.
    #[test]
    fn bumpy_spheres_cut_to_one_closed_surface_with_two_triangles_on_every_edge() {
        let mut report = Vec::new();
        for tolerance in SEAM_BANDS {
            let (mut broken, mut misshapen, mut spheres) = (Vec::new(), 0, 0);
            for seed in 0..40 {
                let wrong = wrong_cuts(&bumpy_sphere(seed, 9), tolerance, &format!("seed {seed}"));
                spheres += !wrong.broken.is_empty() as usize;
                misshapen += wrong.misshapen.len();
                broken.extend(wrong.broken);
            }
            if broken.len() + misshapen > 0 {
                report.push(format!(
                    "seam band {tolerance}: {} of 240 cuts are not a sound closed surface (on {spheres} of 40 spheres), \
                     {misshapen} more enclose the wrong volume\n  {}",
                    broken.len(),
                    broken.join("\n  ")
                ));
            }
        }
        assert!(report.is_empty(), "{}", report.join("\n"));
    }

    /// Hollow shapes: a pit lying across the plane, and a ring the plane goes through twice.
    #[test]
    fn dented_and_ring_shapes_cut_to_closed_surfaces() {
        let upright = Mat4::from_quat(Quat::from_euler(glam::EulerRot::XYZ, 1.1, 0.3, 0.6));
        let grazing = Mat4::from_translation(Vec3::new(0.55, 0.0, 0.31)) * Mat4::from_rotation_z(0.4);
        let mut shapes = vec![("dented sphere", dented_sphere(12))];
        for (name, placed) in [("ring", upright), ("ring cut near its hole", grazing)] {
            let mut ring = torus(1.0, 0.4, 28, 14);
            ring.transform(&placed);
            shapes.push((name, ring));
        }
        let mut report = Vec::new();
        for (name, mesh) in &shapes {
            assert_eq!(Faults::of(mesh), Faults::NONE, "{name} is sound before it is cut");
            for tolerance in SEAM_BANDS {
                let wrong = wrong_cuts(mesh, tolerance, &format!("{name}, seam band {tolerance}"));
                report.extend(wrong.broken);
                report.extend(wrong.misshapen);
            }
        }
        assert!(report.is_empty(), "{} cuts wrong:\n{}", report.len(), report.join("\n"));
    }

    /// A ridge that touches the plane along one edge, from the kept side. Left sharing that edge
    /// with its mirror image, the edge would carry four triangles; the two are stood a seam band
    /// apart instead.
    #[test]
    fn a_ridge_touching_the_plane_is_not_joined_to_its_mirror_image_along_it() {
        let positions = vec![Vec3::ZERO, Vec3::Y, Vec3::new(1.0, 0.5, 1.0), Vec3::new(1.0, 0.5, -1.0)];
        let mut wedge = TriMesh::new(positions, vec![0, 1, 2, 1, 0, 3, 0, 2, 3, 1, 3, 2]);
        if wedge.signed_volume() < 0.0 {
            wedge.flip();
        }
        assert!(wedge.signed_volume() > 0.0 && Faults::of(&wedge) == Faults::NONE);
        let welded = mirror_and_weld(&wedge.positions, &wedge.indices, Axis::X, true, 1e-3).unwrap();
        assert_eq!(Faults::of(&welded.mesh), Faults::NONE);
        assert_closed_and_symmetric(&welded.mesh, Axis::X, "two wedges");
        assert_eq!((welded.mesh.vertex_count(), welded.mesh.triangle_count()), (8, 8), "two whole wedges");
        let nearest = welded.mesh.positions.iter().map(|p| p.x.abs()).fold(f32::MAX, f32::min);
        assert_eq!(nearest, 1e-3, "the ridge stands a seam band off the plane");
        assert!(mirror_and_weld(&wedge.positions, &wedge.indices, Axis::X, false, 1e-3).is_none());
    }

    /// The same for a point: a spike whose tip alone is on the plane would be joined to its
    /// mirror image at that one vertex.
    #[test]
    fn a_spike_touching_the_plane_is_not_joined_to_its_mirror_image_at_its_tip() {
        let positions = vec![Vec3::ZERO, Vec3::new(1.0, 1.0, 0.0), Vec3::new(1.0, -0.5, 1.0), Vec3::new(1.0, -0.5, -1.0)];
        let mut spike = TriMesh::new(positions, vec![0, 2, 1, 0, 1, 3, 0, 3, 2, 1, 2, 3]);
        if spike.signed_volume() < 0.0 {
            spike.flip();
        }
        assert!(spike.signed_volume() > 0.0 && Faults::of(&spike) == Faults::NONE);
        let welded = mirror_and_weld(&spike.positions, &spike.indices, Axis::X, true, 1e-3).unwrap();
        assert_eq!(Faults::of(&welded.mesh), Faults::NONE);
        assert_eq!((welded.mesh.vertex_count(), welded.mesh.triangle_count()), (8, 8), "two whole spikes");
        assert_closed_and_symmetric(&welded.mesh, Axis::X, "two spikes");
    }

    /// Triangles retired by repeating a corner, as a local collapse leaves them, are no part of
    /// the surface: the cut is the same with them as without, wherever they lie.
    #[test]
    fn triangles_with_a_repeated_corner_are_left_out() {
        let straight = octa_sphere(8, 1.0);
        let mut tilted = straight.clone();
        tilted.transform(&Mat4::from_rotation_y(0.2));
        for mesh in [straight, tilted] {
            let clean = mirror_and_weld(&mesh.positions, &mesh.indices, Axis::X, true, 1e-4).unwrap();
            let find = |wanted: &dyn Fn(f32) -> bool| -> [u32; 2] {
                let mut found = (0..mesh.vertex_count() as u32).filter(|&i| wanted(mesh.positions[i as usize].x));
                [found.next().unwrap(), found.next().unwrap()]
            };
            let ([on, on_too], [kept, kept_too], [cut_off, _]) =
                (find(&|x| x.abs() < 1e-6), find(&|x| x > 0.5), find(&|x| x < -0.5));
            // Along the centreline (twice over), across the plane, on the kept side, and to it from the plane.
            let retired = [on, on, on_too, on_too, on, on, kept, kept, cut_off, kept, kept_too, kept, on, kept, kept];
            let mut indices = mesh.indices.clone();
            indices.extend(retired);
            let littered = mirror_and_weld(&mesh.positions, &indices, Axis::X, true, 1e-4).unwrap();
            assert_eq!((&littered.mesh, &littered.source, &littered.cuts), (&clean.mesh, &clean.source, &clean.cuts));
            assert_eq!(littered.kept_triangles, clean.kept_triangles);
            assert_eq!(littered.discarded_triangles, clean.discarded_triangles + retired.len() / 3);
        }
        // And a mesh of nothing else has nothing on either side.
        assert!(mirror_and_weld(&[Vec3::X, Vec3::Y], &[0, 0, 1], Axis::X, true, 1e-4).is_none());
    }

    /// Two parts of the cut meeting at one vertex on the plane: the surface there is a saddle,
    /// up to the kept side, down, up and down again going round it. Shared with the mirror
    /// image it would pinch the surface to a point, so it is stood off the plane and cut round.
    #[test]
    fn a_saddle_on_the_plane_is_not_pinched() {
        let corner = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
        let ring = [corner(1.0, 1.0, 0.0), corner(-1.0, 0.0, 1.0), corner(1.0, -1.0, 0.0), corner(-1.0, 0.0, -1.0)];
        let mut positions = vec![Vec3::ZERO, corner(0.5, 0.0, 6.0)];
        positions.extend(ring);
        // The saddle (vertex 0) and a far corner (vertex 1), each joined to all four of the ring.
        let mut indices = Vec::new();
        for k in 0..4u32 {
            let (a, b) = (2 + k, 2 + (k + 1) % 4);
            indices.extend([0, a, b, 1, b, a]);
        }
        let mut saddle = TriMesh::new(positions, indices);
        if saddle.signed_volume() < 0.0 {
            saddle.flip();
        }
        assert_eq!(Faults::of(&saddle), Faults::NONE);
        for positive in [true, false] {
            let welded = mirror_and_weld(&saddle.positions, &saddle.indices, Axis::X, positive, 1e-3).unwrap();
            assert_eq!(Faults::of(&welded.mesh), Faults::NONE, "positive {positive}");
            assert!(exact_partners(&welded.mesh.positions, Axis::X).is_some());
            let at_saddle = |p: &&Vec3| p.x.abs() == 1e-3 && p.y == 0.0 && p.z == 0.0;
            let stood_off = welded.mesh.positions.iter().filter(at_saddle).count();
            assert_eq!(stood_off, 2, "the saddle and its mirror image, a seam band either side of the plane");
        }
    }

    /// A model with a hole in it, the hole lying across the plane: it is still open there
    /// afterwards and nowhere else, and the centreline vertices at the rim of the hole, where the
    /// cut simply stops, are left on the plane.
    #[test]
    fn a_hole_across_the_plane_is_still_one_hole() {
        let full = octa_sphere(8, 1.0);
        let in_hole = |t: usize| full.triangle(t).iter().sum::<Vec3>().normalize().distance(Vec3::Z) < 0.4;
        let indices: Vec<u32> = (0..full.triangle_count())
            .filter(|&t| !in_hole(t))
            .flat_map(|t| full.indices[t * 3..t * 3 + 3].to_vec())
            .collect();
        let holed = TriMesh::new(full.positions.clone(), indices);
        let rim = |mesh: &TriMesh| {
            let adjacency = MeshAdjacency::of(mesh);
            let edges = MeshEdges::of(mesh, &adjacency);
            (edges.boundary_loops(&mesh.indices), edges.boundary_edges().len(), edges.non_manifold_edges().len())
        };
        let (loops, open, _) = rim(&holed);
        assert!(loops.len() == 1 && open > 8);

        for positive in [true, false] {
            let welded = mirror_and_weld(&holed.positions, &holed.indices, Axis::X, positive, 1e-4).unwrap();
            let (loops_after, open_after, crowded) = rim(&welded.mesh);
            assert_eq!((loops_after.len(), open_after, crowded), (1, open, 0), "the same hole, mirror-exact");
            let faults = Faults::of(&welded.mesh);
            assert_eq!(faults, Faults { open_edges: open, ..Faults::NONE });
            assert!(exact_partners(&welded.mesh.positions, Axis::X).is_some());
            assert_eq!(welded.mesh.triangle_count(), holed.triangle_count());
            // Nothing was stood off the plane or made new: the mesh already had its centreline.
            let used = holed.indices.iter().collect::<std::collections::HashSet<_>>().len();
            assert_eq!((welded.mesh.vertex_count(), welded.cuts.len()), (used, 0));
        }
    }

    /// How many triangles each edge has, by its two ends in ascending order.
    fn triangles_on_edges(indices: &[u32]) -> HashMap<(u32, u32), u32> {
        let mut count = HashMap::with_capacity(indices.len());
        for t in indices.chunks_exact(3).filter(|t| !is_degenerate(t)) {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                *count.entry((a.min(b), a.max(b))).or_insert(0) += 1;
            }
        }
        count
    }

    /// The open edges of a cut that are not a kept piece of one of the input's own open edges, or
    /// the mirror image of one: where the cut itself left the surface open. Worked out from where
    /// each vertex came from, not from where it is.
    fn open_edges_the_cut_made(input: &TriMesh, welded: &MirrorWelded, axis: Axis) -> Vec<String> {
        let mesh = &welded.mesh;
        let first_cut = mesh.vertex_count() - welded.cuts.len();
        let before = triangles_on_edges(&input.indices);
        let was_open = |a: u32, b: u32| before.get(&(a.min(b), a.max(b))) == Some(&1);
        let height = |k: u32| axis.coord(mesh.positions[k as usize]);
        let made_at_the_cut = |k: u32| (k as usize >= first_cut).then(|| welded.cuts[k as usize - first_cut]);
        let source = |k: u32| welded.source[k as usize];

        let mut made = Vec::new();
        let mut seen = HashSet::new();
        for (&(p, q), _) in triangles_on_edges(&mesh.indices).iter().filter(|(_, count)| **count == 1) {
            // The edge of the input this one is (part of), if it is one at all.
            let from = match (made_at_the_cut(p), made_at_the_cut(q)) {
                (None, None) if height(p) != 0.0 || height(q) != 0.0 => Some((source(p), source(q))),
                (Some(cut), None) if source(q) == cut.kept => Some((cut.kept, cut.cut_off)),
                (None, Some(cut)) if source(p) == cut.kept => Some((cut.kept, cut.cut_off)),
                _ => None,
            };
            let on_the_positive_side = height(p) + height(q) > 0.0;
            let from_the_rim =
                from.is_some_and(|(a, b)| was_open(a, b) && seen.insert((a.min(b), a.max(b), on_the_positive_side)));
            if !from_the_rim {
                made.push(format!("{} - {}", mesh.positions[p as usize], mesh.positions[q as usize]));
            }
        }
        made.sort();
        made
    }

    /// A triangle's corners turned round, without changing their order, to start at the lowest.
    fn lowest_first(t: [u32; 3]) -> [u32; 3] {
        let at = (0..3).min_by_key(|&k| t[k]).unwrap();
        [t[at], t[(at + 1) % 3], t[(at + 2) % 3]]
    }

    /// What is wrong with a cut of a model that had holes in it. It has to be as sound as a
    /// closed one, except that it is open where the input was: no edge with more than two
    /// triangles, one winding, no flat triangles or doubled vertices, no vertex pinched that was
    /// not pinched to begin with, every vertex AND every triangle with its exact mirror image,
    /// and no open edge that is not a piece of the input's own rim.
    fn open_cut_problems(input: &TriMesh, welded: &MirrorWelded, axis: Axis) -> Vec<String> {
        let mesh = &welded.mesh;
        let mut problems = Vec::new();
        let (before, after) = (Faults::of(input), Faults::of(mesh));
        let allowed = Faults { open_edges: after.open_edges, pinched_vertices: after.pinched_vertices, ..Faults::NONE };
        if after != allowed {
            problems.push(format!("{after:?}"));
        }

        let first_cut = mesh.vertex_count() - welded.cuts.len();
        let pinched_before = pinched_vertices(input);
        let was_pinched = |k: u32| (k as usize) < first_cut && pinched_before.contains(&welded.source[k as usize]);
        let new_pinches: Vec<u32> = pinched_vertices(mesh).into_iter().filter(|&k| !was_pinched(k)).collect();
        if let Some(&first) = new_pinches.first() {
            problems.push(format!(
                "pinched vertices: {} in, {} out, {} of them not pinched before (the first at {})",
                before.pinched_vertices,
                after.pinched_vertices,
                new_pinches.len(),
                mesh.positions[first as usize]
            ));
        }

        match exact_partners(&mesh.positions, axis) {
            None => problems.push("not mirror-exact".to_string()),
            Some(partners) => {
                let triangles: HashSet<[u32; 3]> =
                    mesh.indices.chunks_exact(3).map(|t| lowest_first([t[0], t[1], t[2]])).collect();
                let mirrored = |t: &[u32; 3]| {
                    let [a, b, c] = t.map(|k| partners[k as usize]);
                    lowest_first([a, c, b])
                };
                let alone = triangles.iter().filter(|t| !triangles.contains(&mirrored(t))).count();
                if alone > 0 {
                    problems.push(format!("{alone} triangles have no mirror image"));
                }
            }
        }

        let made = open_edges_the_cut_made(input, welded, axis);
        if let Some(first) = made.first() {
            problems.push(format!(
                "open edges: {} in, {} out, {} of them not on the model's own rim (the first {first})",
                before.open_edges,
                after.open_edges,
                made.len()
            ));
        }
        problems
    }

    /// Cuts an open model on the given planes, keeping each side in turn, at each seam band, and
    /// lists the cuts that are not sound (`open_cut_problems`). A refusal is only right when the
    /// model has nothing beyond the seam band on the side asked for.
    fn wrong_open_cuts(input: &TriMesh, axes: &[Axis], label: &str) -> Vec<String> {
        let mut wrong = Vec::new();
        for &axis in axes {
            for positive in [true, false] {
                for tolerance in SEAM_BANDS {
                    let side = if positive { '+' } else { '-' };
                    let label = format!("{label}, {side}{} kept, seam band {tolerance}", axis.name());
                    let Some(welded) = mirror_and_weld(&input.positions, &input.indices, axis, positive, tolerance)
                    else {
                        let sign = if positive { 1.0 } else { -1.0 };
                        if input.indices.iter().any(|&i| axis.coord(input.positions[i as usize]) * sign > tolerance) {
                            wrong.push(format!("{label}: refused"));
                        }
                        continue;
                    };
                    let problems = open_cut_problems(input, &welded, axis);
                    if !problems.is_empty() {
                        wrong.push(format!("{label}: {}", problems.join("; ")));
                    }
                }
            }
        }
        wrong
    }

    fn assert_open_cuts_are_sound(input: &TriMesh, axes: &[Axis], label: &str) {
        let wrong = wrong_open_cuts(input, axes, label);
        assert!(wrong.is_empty(), "{} cuts wrong:\n{}", wrong.len(), wrong.join("\n"));
    }

    /// The checks above see what they are there to see: a cut left open, and one half unlike the
    /// other.
    #[test]
    fn a_cut_left_open_or_lopsided_is_seen() {
        let full = octa_sphere(6, 1.0);
        let mut bowl = without(&full, |t| full.triangle(t).iter().map(|p| p.y).sum::<f32>() < 0.0);
        bowl.transform(&Mat4::from_rotation_y(0.3));
        let welded = mirror_and_weld(&bowl.positions, &bowl.indices, Axis::X, true, 1e-4).unwrap();
        assert_eq!(open_cut_problems(&bowl, &welded, Axis::X), Vec::<String>::new());

        // One triangle at the cut taken out of one half.
        let first_cut = (welded.mesh.vertex_count() - welded.cuts.len()) as u32;
        let at_the_cut = |t: usize| welded.mesh.indices[t * 3..t * 3 + 3].iter().any(|&k| k >= first_cut);
        let taken = (0..welded.mesh.triangle_count()).find(|&t| at_the_cut(t)).unwrap();
        let torn = MirrorWelded { mesh: without(&welded.mesh, |t| t == taken), ..welded.clone() };
        let problems = open_cut_problems(&bowl, &torn, Axis::X).join("; ");
        assert!(problems.contains("1 triangles have no mirror image"), "{problems}");
        assert!(problems.contains("not on the model's own rim"), "{problems}");

        // And one vertex of the rim moved: no longer its mirror image's mirror image.
        let mut lopsided = welded.clone();
        lopsided.mesh.positions[0].y += 0.01;
        assert_eq!(open_cut_problems(&bowl, &lopsided, Axis::X), ["not mirror-exact"]);
    }

    /// `mesh` without the triangles `gone` picks out. Their vertices are left in.
    fn without(mesh: &TriMesh, gone: impl Fn(usize) -> bool) -> TriMesh {
        let indices = (0..mesh.triangle_count())
            .filter(|&t| !gone(t))
            .flat_map(|t| mesh.indices[t * 3..t * 3 + 3].to_vec())
            .collect();
        TriMesh::new(mesh.positions.clone(), indices)
    }

    /// A sheet of squares, each split in two from its lowest corner to its highest, with the
    /// vertex in column `i` and row `j` (each from `-reach` to `reach`) at `place(i, j)`.
    fn sheet(reach: i32, place: impl Fn(i32, i32) -> Vec3) -> TriMesh {
        let across = 2 * reach + 1;
        let at = |i: i32, j: i32| ((j + reach) * across + i + reach) as u32;
        let positions = (-reach..=reach).flat_map(|j| (-reach..=reach).map(move |i| (i, j))).map(|(i, j)| place(i, j));
        let mut indices = Vec::new();
        for j in -reach..reach {
            for i in -reach..reach {
                indices.extend([at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
            }
        }
        TriMesh::new(positions.collect(), indices)
    }

    /// The smallest open model the cut can pinch: two triangles side by side, the edge between
    /// them running from a rim vertex ON the plane back into the side thrown away. Each leaves a
    /// piece on the kept side, and the two pieces meet only at that vertex. Left on the plane it
    /// would be one vertex shared by two separate fans, each a piece and its mirror image.
    #[test]
    fn two_triangles_meeting_at_a_rim_vertex_on_the_plane_are_not_pinched_there() {
        let (rim_vertex, cut_off) = (Vec3::ZERO, Vec3::new(-1.0, 0.0, 0.0));
        let positions = vec![rim_vertex, Vec3::new(1.0, 1.0, 0.0), cut_off, Vec3::new(1.0, -1.0, 0.0)];
        let pair = TriMesh::new(positions, vec![0, 1, 2, 0, 2, 3]);
        assert_eq!(Faults::of(&pair), Faults { open_edges: 4, ..Faults::NONE });
        assert_open_cuts_are_sound(&pair, &[Axis::X, Axis::Y], "two triangles");

        // Keeping the side with the two far corners: the rim vertex is stood off the plane, so
        // the edge it shares with the cut-off corner is cut too and the two pieces share that.
        let welded = mirror_and_weld(&pair.positions, &pair.indices, Axis::X, true, 1e-3).unwrap();
        let faults = Faults::of(&welded.mesh);
        assert_eq!(faults, Faults { open_edges: 8, ..Faults::NONE }, "the four edges round the two pieces, twice");
        assert_eq!((welded.mesh.vertex_count(), welded.mesh.triangle_count(), welded.cuts.len()), (9, 8, 3));
        let stood_off = welded.mesh.positions.iter().filter(|p| p.x.abs() == 1e-3 && p.y == 0.0).count();
        assert_eq!(stood_off, 2, "the rim vertex and its mirror image, a seam band either side of the plane");

        // Keeping the other side there is one piece, ending at the plane: nothing to stand off.
        let welded = mirror_and_weld(&pair.positions, &pair.indices, Axis::X, false, 1e-3).unwrap();
        assert_eq!(Faults::of(&welded.mesh), Faults { open_edges: 4, ..Faults::NONE });
        assert_eq!((welded.mesh.vertex_count(), welded.mesh.triangle_count(), welded.cuts.len()), (5, 4, 2));
        assert_eq!(welded.mesh.positions[0], Vec3::ZERO);
    }

    /// A strip along the plane whose rim on the kept side runs in and out, touching the plane at
    /// every other vertex: the kept side is a row of teeth, each joined to the next only at such
    /// a vertex.
    #[test]
    fn a_strip_whose_rim_touches_the_plane_is_not_pinched_between_its_teeth() {
        let teeth = 5;
        let mut positions = Vec::new();
        for k in 0..=2 * teeth {
            positions.push(Vec3::new(-1.0, k as f32, 0.0));
            positions.push(Vec3::new(if k % 2 == 0 { 1.0 } else { 0.0 }, k as f32, 0.2));
        }
        let mut indices = Vec::new();
        for k in 0..2 * teeth as u32 {
            let (low, high, next_low, next_high) = (2 * k, 2 * k + 1, 2 * k + 2, 2 * k + 3);
            indices.extend([low, high, next_high, low, next_high, next_low]);
        }
        let strip = TriMesh::new(positions, indices);
        assert_eq!(Faults::of(&strip), Faults { open_edges: 4 * teeth + 2, ..Faults::NONE });
        assert_open_cuts_are_sound(&strip, &Axis::ALL, "toothed strip");
    }

    /// A strip straight across the plane, with a row of vertices on it: its two rim vertices on
    /// the plane are where the cut simply stops, and are left there.
    #[test]
    fn a_strip_across_the_plane_keeps_its_rim_vertices_on_the_plane() {
        let strip = sheet(2, |i, j| Vec3::new(i as f32, j as f32, 0.1 * (i * j) as f32));
        let open = Faults::of(&strip).open_edges;
        assert_eq!(open, 16);
        assert_open_cuts_are_sound(&strip, &Axis::ALL, "strip");
        for positive in [true, false] {
            let welded = mirror_and_weld(&strip.positions, &strip.indices, Axis::X, positive, 1e-4).unwrap();
            assert_eq!(Faults::of(&welded.mesh), Faults { open_edges: open, ..Faults::NONE });
            assert_eq!((welded.mesh.vertex_count(), welded.cuts.len()), (strip.vertex_count(), 0));
            assert_eq!(welded.mesh.positions.iter().filter(|p| p.x == 0.0).count(), 5, "nothing stood off the plane");
        }
    }

    /// A sheet across the plane with a hole in it, the hole on one side and its rim touching the
    /// plane at one vertex. Keeping the side the hole is on, the kept surface comes to that vertex
    /// from both sides of the hole and the two parts meet nowhere else.
    #[test]
    fn a_hole_whose_rim_touches_the_plane_does_not_pinch_the_sheet() {
        // Bowed, so that of the middle column only the middle vertex is on the plane.
        let place = |i: i32, j: i32| Vec3::new(i as f32 + 0.3 * (j * j) as f32, j as f32, 0.05 * i as f32);
        let whole = sheet(3, place);
        let centre_of_hole = whole.positions.iter().position(|&p| p == place(1, 0)).unwrap() as u32;
        let holed = without(&whole, |t| whole.indices[t * 3..t * 3 + 3].contains(&centre_of_hole));
        assert_eq!(Faults::of(&holed), Faults { open_edges: 24 + 6, ..Faults::NONE });
        assert_eq!(holed.positions.iter().filter(|p| p.x == 0.0).count(), 1);
        assert_open_cuts_are_sound(&holed, &Axis::ALL, "holed sheet");

        // And a disc with its hole in the middle, the plane through both: four rim vertices on
        // the plane, each the end of the cut, each left where it is.
        let ring = |radius: f32, k: u32| {
            let (sin, cos) = (k as f32 / 12.0 * std::f32::consts::TAU).sin_cos();
            // Exactly on the planes where it should be, which the sine and cosine alone are not.
            let exact = |c: f32| if c.abs() < 1e-6 { 0.0 } else { c };
            Vec3::new(radius * exact(sin), radius * exact(cos), 0.1 * radius)
        };
        let positions: Vec<Vec3> = (0..12).map(|k| ring(1.0, k)).chain((0..12).map(|k| ring(2.0, k))).collect();
        let indices = (0..12u32).flat_map(|k| [k, 12 + k, 12 + (k + 1) % 12, k, 12 + (k + 1) % 12, (k + 1) % 12]);
        let disc = TriMesh::new(positions, indices.collect());
        assert_eq!(Faults::of(&disc), Faults { open_edges: 24, ..Faults::NONE });
        assert_open_cuts_are_sound(&disc, &Axis::ALL, "disc with a hole");
        for axis in [Axis::X, Axis::Y] {
            let welded = mirror_and_weld(&disc.positions, &disc.indices, axis, true, 1e-4).unwrap();
            assert_eq!(Faults::of(&welded.mesh), Faults { open_edges: 24, ..Faults::NONE }, "{axis:?}");
            assert_eq!((welded.mesh.vertex_count(), welded.cuts.len()), (24, 0), "{axis:?}: nothing stood off or cut");
        }
    }

    /// Half a ball, its open rim crossing the plane: at a rim vertex on the plane, between two
    /// rim vertices, and (with a rim that wanders) in and out of the seam band.
    #[test]
    fn a_hemisphere_whose_rim_crosses_the_plane_is_open_only_along_its_rim() {
        let full = octa_sphere(8, 1.0);
        let upper = without(&full, |t| full.triangle(t).iter().map(|p| p.y).sum::<f32>() < 0.0);
        let open = Faults::of(&upper).open_edges;
        assert_eq!(open, 32);
        assert_open_cuts_are_sound(&upper, &Axis::ALL, "hemisphere");
        for axis in [Axis::X, Axis::Z] {
            let welded = mirror_and_weld(&upper.positions, &upper.indices, axis, true, 1e-4).unwrap();
            assert_eq!(Faults::of(&welded.mesh), Faults { open_edges: open, ..Faults::NONE }, "{axis:?}");
            assert_eq!(welded.mesh.triangle_count(), upper.triangle_count(), "{axis:?}: the same hemisphere");
        }

        let mut turned = upper.clone();
        turned.transform(&Mat4::from_quat(Quat::from_euler(glam::EulerRot::XYZ, 0.31, 0.47, 0.83)));
        assert_open_cuts_are_sound(&turned, &Axis::ALL, "hemisphere, turned");

        for seed in 0..12 {
            let ball = bumpy_sphere(seed, 9);
            let wander = |p: Vec3| p.y + 0.1 * (7.0 * p.x).sin();
            let ragged = without(&ball, |t| ball.triangle(t).iter().map(|&p| wander(p)).sum::<f32>() < 0.0);
            assert_open_cuts_are_sound(&ragged, &Axis::ALL, &format!("ragged half of ball {seed}"));
        }
    }

    /// A sheet wholly on one side that touches the plane at one vertex of its rim: it is not
    /// joined to its mirror image at that point. And one across the plane with every vertex of
    /// its middle column on it, rim included: one sheet, as it was.
    #[test]
    fn a_sheet_with_a_rim_vertex_on_the_plane_is_joined_only_where_it_crosses() {
        // A square stood on its corner, the corner on the plane.
        let diamond = sheet(2, |i, j| Vec3::new((i - j + 4) as f32, (i + j) as f32, 0.1 * i as f32));
        let open = Faults::of(&diamond).open_edges;
        assert_eq!(diamond.positions.iter().filter(|p| p.x == 0.0).count(), 1);
        assert_open_cuts_are_sound(&diamond, &Axis::ALL, "sheet on its corner");
        let welded = mirror_and_weld(&diamond.positions, &diamond.indices, Axis::X, true, 1e-3).unwrap();
        assert_eq!(Faults::of(&welded.mesh), Faults { open_edges: 2 * open, ..Faults::NONE });
        assert_eq!(welded.mesh.vertex_count(), 2 * diamond.vertex_count(), "two whole sheets");
        assert!(mirror_and_weld(&diamond.positions, &diamond.indices, Axis::X, false, 1e-3).is_none());

        // The same sheet pushed across, so the plane runs corner to corner along the edges that
        // split its squares.
        let across = sheet(2, |i, j| Vec3::new((i - j) as f32, (i + j) as f32, 0.1 * i as f32));
        assert_open_cuts_are_sound(&across, &Axis::ALL, "sheet cut corner to corner");
        let welded = mirror_and_weld(&across.positions, &across.indices, Axis::X, true, 1e-3).unwrap();
        assert_eq!(Faults::of(&welded.mesh), Faults { open_edges: open, ..Faults::NONE });
        assert_eq!((welded.mesh.vertex_count(), welded.cuts.len()), (across.vertex_count(), 0));
    }

    /// A closed model with patches torn out of it, many of them on or across a coordinate plane,
    /// and with vertices near the planes set exactly onto them: an open model with every kind of
    /// rim a plane can meet.
    fn torn(seed: u64) -> TriMesh {
        let mut rng = Lcg::new(seed ^ 0x7A11);
        let pick = |rng: &mut Lcg, n: usize| ((rng.unit() * n as f32) as usize).min(n - 1);
        let mut mesh = match seed % 4 {
            0 | 1 => bumpy_sphere(seed, 5 + (seed % 5) as u32),
            2 => {
                let mut ring = torus(1.0, 0.4, 10 + (seed % 9) as u32, 6 + (seed % 5) as u32);
                let turn = Quat::from_euler(glam::EulerRot::XYZ, rng.unit() * 3.0, rng.unit() * 3.0, rng.unit() * 3.0);
                ring.transform(&(Mat4::from_translation(rng.in_cube() * 0.5) * Mat4::from_quat(turn)));
                ring
            }
            _ => dented_sphere(5 + (seed % 6) as u32),
        };

        // Each vertex near one of the planes (which one is drawn first) may be set exactly on it.
        let (band, how_often) = (rng.unit() * 0.3, rng.unit());
        for p in &mut mesh.positions {
            let axis = pick(&mut rng, 3);
            if p[axis].abs() < band && rng.unit() < how_often {
                p[axis] = 0.0;
            }
        }

        let mut gone = vec![false; mesh.triangle_count()];
        for _ in 0..1 + pick(&mut rng, 5) {
            let mut centre = mesh.positions[pick(&mut rng, mesh.vertex_count())];
            // Two patches in three are centred ON a plane, so they lie across it.
            let axis = pick(&mut rng, 3);
            if rng.unit() < 0.67 {
                centre[axis] = 0.0;
            }
            let (reach, by_any_corner) = (0.08 + rng.unit() * 0.5, rng.unit() < 0.5);
            for (t, gone) in gone.iter_mut().enumerate() {
                let near = |p: &Vec3| p.distance(centre) < reach;
                let corners = mesh.triangle(t);
                *gone |= if by_any_corner { corners.iter().any(near) } else { corners.iter().all(near) };
            }
        }
        // And odd single triangles, which leave the model touching itself across their corners.
        for _ in 0..pick(&mut rng, 6) {
            gone[pick(&mut rng, mesh.triangle_count())] = true;
        }
        without(&mesh, |t| gone[t])
    }

    /// Open models of every sort, cut every way: the cut is sound wherever the plane meets a rim.
    #[test]
    fn torn_models_are_cut_soundly_wherever_the_plane_meets_a_rim() {
        let (mut cuts, mut wrong) = (0, Vec::new());
        // How many of the models have a rim vertex exactly on a plane, and a rim across one.
        let (mut rim_on_a_plane, mut rim_across_a_plane) = (0, 0);
        for seed in 0..300 {
            let model = torn(seed);
            let on_edges = triangles_on_edges(&model.indices);
            let rim: Vec<(u32, u32)> = on_edges.into_iter().filter(|&(_, count)| count == 1).map(|(e, _)| e).collect();
            if rim.is_empty() {
                continue;
            }
            // Torn, but not otherwise broken: whatever else is wrong afterwards, the cut did.
            let faults = Faults::of(&model);
            let broken = (faults.crowded_edges, faults.miswound, faults.flat_triangles, faults.duplicate_vertices);
            assert_eq!(broken, (0, false, 0, 0), "seed {seed}");
            let ends = |e: &(u32, u32)| [model.positions[e.0 as usize], model.positions[e.1 as usize]];
            let on_a_plane = |p: &Vec3| p.x == 0.0 || p.y == 0.0 || p.z == 0.0;
            rim_on_a_plane += rim.iter().any(|e| ends(e).iter().any(on_a_plane)) as usize;
            rim_across_a_plane += rim.iter().any(|e| (ends(e)[0] * ends(e)[1]).min_element() < 0.0) as usize;
            cuts += 18;
            wrong.extend(wrong_open_cuts(&model, &Axis::ALL, &format!("torn model {seed}")));
        }
        assert!(cuts >= 4000 && rim_on_a_plane > 150 && rim_across_a_plane > 200, "{cuts} cuts is too tame a sweep");
        let shown = wrong.iter().take(12).cloned().collect::<Vec<_>>().join("\n  ");
        assert!(wrong.is_empty(), "{} of {cuts} cuts of open models are wrong:\n  {shown}", wrong.len());
    }

    /// Two mirrored halves joined without welding: the seam is every centreline vertex twice.
    #[test]
    fn weld_turns_two_touching_shells_into_one_surface() {
        let full = octa_sphere(10, 1.0);
        let mut joined = half(&full, true);
        let negative = half(&full, false);
        joined.append(&negative);
        // Drop the vertices neither half uses, as a join of two real objects would.
        let joined = compact(&joined);
        let seam = 4 * 10;
        assert_eq!(joined.vertex_count(), full.vertex_count() + seam);
        assert!(edge_census(&joined).0 > 0, "two shells touching, not one closed surface");

        let welded = weld(&joined.positions, &joined.indices, Axis::X, 0.01, 1e-5).unwrap();
        assert_eq!(welded.merged(), seam);
        assert_eq!(welded.mesh.vertex_count(), full.vertex_count());
        assert_eq!(welded.mesh.triangle_count(), full.triangle_count());
        assert_closed_and_symmetric(&welded.mesh, Axis::X, "welded");
        assert!((welded.mesh.signed_volume() - full.signed_volume()).abs() < 1e-9);
        for (i, &k) in welded.target.iter().enumerate() {
            assert_eq!(welded.mesh.positions[k as usize], joined.positions[i]);
        }
        assert!(weld(&welded.mesh.positions, &welded.mesh.indices, Axis::X, 0.01, 1e-5).is_none(), "already clean");
    }

    fn compact(mesh: &TriMesh) -> TriMesh {
        let mut slot = vec![NONE; mesh.vertex_count()];
        let mut positions = Vec::new();
        let indices = mesh
            .indices
            .iter()
            .map(|&i| {
                if slot[i as usize] == NONE {
                    slot[i as usize] = positions.len() as u32;
                    positions.push(mesh.positions[i as usize]);
                }
                slot[i as usize]
            })
            .collect();
        TriMesh::new(positions, indices)
    }

    #[test]
    fn the_generous_radius_is_for_the_seam_only() {
        let positions = vec![
            // Two on the plane, well inside the seam radius of each other: one vertex.
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.004, 0.0),
            // Off the plane but just as close to the first: real geometry, left alone.
            Vec3::new(0.004, 0.0, 0.0),
            // Far from the plane, closer together than the seam radius: sculpted detail, left alone.
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 0.004, 0.0),
            // Far from the plane and literally the same point: a split vertex, merged.
            Vec3::new(2.0, 1.0, 0.0),
            Vec3::new(2.0, 1.0, 0.0),
        ];
        let indices = vec![0, 1, 3, 1, 2, 4, 3, 5, 6, 2, 4, 5];
        let welded = weld(&positions, &indices, Axis::X, 0.01, 1e-5).unwrap();
        assert_eq!(welded.target, [0, 0, 1, 2, 3, 4, 4]);
        assert_eq!(welded.source, [0, 2, 3, 4, 5]);
        assert_eq!(welded.merged(), 2);
        // (0,1,3) lost a corner and (3,5,6) collapsed; the other two survive, renumbered.
        assert_eq!(welded.mesh.indices, [0, 1, 3, 1, 3, 4]);
        assert_eq!(welded.carry(&[10, 11, 12, 13, 14, 15, 16]), [10, 12, 13, 14, 15]);
    }

    #[test]
    fn weld_joins_each_vertex_to_its_nearest_survivor() {
        // Three in a row on the plane, each within reach of the next but not the ends of each
        // other: the middle joins the first, and the last, with the middle gone, stays.
        let positions = vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.008, 0.0), Vec3::new(0.0, 0.016, 0.0)];
        let welded = weld(&positions, &[0, 1, 2], Axis::X, 0.01, 1e-5).unwrap();
        assert_eq!(welded.target, [0, 0, 1]);
        assert!(welded.mesh.indices.is_empty());
        assert!(weld(&[Vec3::ZERO, Vec3::X], &[], Axis::X, 0.01, 1e-5).is_none());
        assert!(weld(&[], &[], Axis::X, 0.01, 1e-5).is_none());
    }

    #[test]
    fn weld_coincident_ignores_the_plane() {
        // The same pair of near-coincident points on the plane and off it: both merge, or neither.
        let positions =
            vec![Vec3::ZERO, Vec3::new(0.0, 0.004, 0.0), Vec3::new(1.0, 0.0, 0.0), Vec3::new(1.0, 0.004, 0.0), Vec3::Z];
        let indices = vec![0, 2, 4, 1, 3, 4];
        let welded = weld_coincident(&positions, &indices, 0.01).unwrap();
        assert_eq!(welded.target, [0, 0, 1, 1, 2]);
        assert_eq!(welded.mesh.indices, [0, 1, 2, 0, 1, 2]);
        assert!(weld_coincident(&positions, &indices, 0.001).is_none());
        assert!(weld_coincident(&positions, &indices, 0.0).is_none());
    }

    /// A mesh stored as loose triangles (every corner its own vertex), with a strip of
    /// near-coincident seam vertices thrown in: settling across cores must give exactly what the
    /// plain in-order sweep gives.
    #[test]
    fn settling_across_cores_matches_the_in_order_sweep() {
        let full = octa_sphere(20, 1.0);
        let mut positions: Vec<Vec3> = full.indices.iter().map(|&i| full.positions[i as usize]).collect();
        let indices: Vec<u32> = (0..positions.len() as u32).collect();
        // A chain along the plane, each link within the seam radius of the next.
        positions.extend((0..300).map(|k| Vec3::new(0.0, 2.0 + k as f32 * 0.004, 0.0)));

        let in_order = weld_in_rounds(&positions, &indices, Axis::X, 0.01, 1e-5, 0).unwrap();
        let across = weld_in_rounds(&positions, &indices, Axis::X, 0.01, 1e-5, WELD_ROUNDS).unwrap();
        assert_eq!(across, in_order);
        assert_eq!(across, weld(&positions, &indices, Axis::X, 0.01, 1e-5).unwrap());

        // The sphere is whole again: one vertex per position, closed, outward.
        let sphere_vertices = across.source.iter().filter(|&&i| (i as usize) < indices.len()).count();
        assert_eq!(sphere_vertices, full.vertex_count());
        assert_eq!(across.mesh.triangle_count(), full.triangle_count());
        assert_eq!(edge_census(&across.mesh), (0, 0));
        assert!((across.mesh.signed_volume() - full.signed_volume()).abs() < 1e-9);
        // The chain: links 0.004 apart with a 0.01 reach, so each survivor swallows the two after it.
        assert_eq!(across.mesh.vertex_count() - sphere_vertices, 100);
    }

    /// Run with `--release -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn timing_mirror_and_weld_1m() {
        // A smooth ball the plane crosses once, at a slant to every row of triangles, and a
        // spiky one it crosses over and over.
        let mut smooth = octa_sphere(354, 1.0);
        smooth.transform(&Mat4::from_quat(Quat::from_euler(glam::EulerRot::XYZ, 0.31, 0.47, 0.83)));
        let spiky = bumpy_sphere(7, 354);
        for (name, mesh) in [("tilted sphere", &smooth), ("spiky ball", &spiky)] {
            let spacing = crate::map::mean_edge_length(&mesh.positions, &mesh.indices);
            let mask: Vec<f32> = mesh.positions.iter().map(|p| p.y).collect();
            // A band far thinner than a triangle, and the half a triangle the app asks for.
            for (band, seam) in [("thin seam band", 1e-4), ("seam band half a triangle wide", 0.5 * spacing)] {
                let mut best = f64::MAX;
                let mut built = None;
                for _ in 0..5 {
                    let clock = std::time::Instant::now();
                    let welded = mirror_and_weld(&mesh.positions, &mesh.indices, Axis::X, true, seam).unwrap();
                    let carried = welded.carry_blended(&mask);
                    best = best.min(clock.elapsed().as_secs_f64() * 1e3);
                    assert_eq!(carried.len(), welded.mesh.vertex_count());
                    built = Some(welded);
                }
                let welded = built.unwrap();
                println!(
                    "mirror_and_weld, {name}, {band}: {} triangles, {} vertices -> {} triangles, {} vertices \
                     ({} new on the plane) in {best:.1} ms (best of 5, mask carried)",
                    mesh.triangle_count(),
                    mesh.vertex_count(),
                    welded.mesh.triangle_count(),
                    welded.mesh.vertex_count(),
                    welded.cuts.len(),
                );
                assert_closed_and_symmetric(&welded.mesh, Axis::X, name);
            }
        }

        // And the weld of the same model as two unjoined halves.
        let full = octa_sphere(354, 1.0);
        let mut joined = half(&full, true);
        joined.append(&half(&full, false));
        let clock = std::time::Instant::now();
        let welded = weld(&joined.positions, &joined.indices, Axis::X, 1e-3, 1e-6).unwrap();
        println!(
            "weld: {} triangles, {} vertices -> {} vertices in {:.1} ms",
            joined.triangle_count(),
            joined.vertex_count(),
            welded.mesh.vertex_count(),
            clock.elapsed().as_secs_f64() * 1e3
        );
    }
}
