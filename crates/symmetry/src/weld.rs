//! Repairs that change topology: replacing one half of a model with a mirror of
//! the other, and merging the duplicated vertices of a seam.
//!
//! Both hand back a whole new mesh, together with which vertex of the input
//! each new vertex was made from, so anything stored per vertex (the mask
//! above all) can be carried across with `carry`.

use std::sync::atomic::{AtomicU8, Ordering};

use glam::Vec3;
use rayon::prelude::*;
use sculpt_geom::TriMesh;

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
    /// pinned onto the plane, or reflected across it.
    pub source: Vec<u32>,
    /// Triangles of the input on the side that was kept.
    pub kept_triangles: usize,
    /// Triangles of the input that were thrown away and rebuilt.
    pub discarded_triangles: usize,
}

impl MirrorWelded {
    /// A per-vertex array of the input (a mask), carried onto the new mesh. A
    /// reflected vertex takes the value of the vertex it reflects, so the mask
    /// comes out mirrored along with the surface.
    pub fn carry<T: Copy + Send + Sync>(&self, values: &[T]) -> Vec<T> {
        carry(&self.source, values)
    }
}

/// Replaces one whole side of the model with a reflection of the other,
/// cutting the mesh at the plane and rebuilding the discarded half from
/// scratch. Unlike `repair::make_symmetric` it needs NO vertex correspondence,
/// so it works on exactly the models that has to refuse: two arms modelled or
/// joined separately, one side remeshed, an imported asset whose halves never
/// matched. It always produces an exactly symmetric result, at the cost of
/// rebuilding the mesh.
///
/// The cut is decided per TRIANGLE by its centroid, not per vertex, so every
/// triangle goes wholly one way or the other and none is left with a dangling
/// corner. Kept triangles that reach the cut then have those corners pinned
/// onto the plane, which makes the cut edge exactly planar instead of a ragged
/// fringe. Because the boundary loop ends up exactly on the plane, the mirrored
/// half SHARES those vertices rather than duplicating them, and the two halves
/// come out already joined: watertight, one shared edge loop, nothing left to
/// weld.
///
/// `seam_tolerance` is the band around the plane whose vertices are pinned
/// onto it before anything is cut, so a centre vertex sitting a hair off the
/// plane cannot decide a triangle's side by accident.
///
/// `None` when there is nothing to work with: no geometry, or no triangle at
/// all on the source side.
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
    let seam = seam_tolerance.max(1e-6);
    let sign = if source_is_positive { 1.0 } else { -1.0 };

    let in_band = |p: Vec3| axis.coord(p).abs() <= seam;
    let mut work: Vec<Vec3> =
        positions.par_iter().with_min_len(8192).map(|&p| if in_band(p) { axis.pin(p) } else { p }).collect();

    // Pass 1: which triangles survive the cut. By centroid, so a triangle
    // spanning the plane goes whichever way most of it lies rather than being
    // split: this is a rebuild, not a boolean, and the surviving corners are
    // pinned onto the plane straight afterwards anyway. Strictly greater than
    // zero: a triangle lying exactly IN the plane would otherwise be kept and
    // then mirrored onto itself, giving two coincident faces.
    let keep: Vec<bool> = indices
        .par_chunks_exact(3)
        .with_min_len(4096)
        .map(|t| t.iter().map(|&i| axis.coord(work[i as usize])).sum::<f32>() / 3.0 * sign > 0.0)
        .collect();
    let kept_triangles = par::count(triangle_count, |t| keep[t]);
    if kept_triangles == 0 {
        return None;
    }

    // Pass 2: flatten the cut edge onto the plane.
    //
    // TWO kinds of vertex need this, and getting only the first is what left
    // the result torn open. The obvious one is a corner that reaches across
    // the plane. The one that was missed is the RIM itself: a vertex shared by
    // a kept and a discarded triangle, sitting on the correct side and so
    // pinned by nothing. Whether the rim happened to land inside the seam band
    // was pure luck of the tessellation, and where it did not it was given a
    // reflected TWIN instead of being shared, so the halves came out as two
    // shells with a gap between them (measured on a watertight 145k-triangle
    // sculpt: 1560 boundary edges down the centreline).
    //
    // Pinning the rim makes the cut planar BY CONSTRUCTION at any tolerance.
    // The rim is at most one triangle from the plane to begin with, so this
    // moves nothing further than the cut itself already did.
    const USED: u8 = 1;
    const RIM: u8 = 2;
    let flags: Vec<AtomicU8> = (0..work.len()).into_par_iter().with_min_len(8192).map(|_| AtomicU8::new(0)).collect();
    indices.par_chunks_exact(3).with_min_len(4096).zip(&keep).for_each(|(t, &kept)| {
        for &i in t {
            flags[i as usize].fetch_or(if kept { USED } else { RIM }, Ordering::Relaxed);
        }
    });
    let flags: Vec<u8> = flags.into_iter().map(AtomicU8::into_inner).collect();
    work.par_iter_mut().with_min_len(8192).zip(&flags).for_each(|(p, &flag)| {
        if flag & USED != 0 && (flag & RIM != 0 || axis.coord(*p) * sign < 0.0) {
            *p = axis.pin(*p);
        }
    });

    // Pass 3: compact the kept vertices, giving each off-plane one a reflected
    // twin in the slot after its own. On-plane vertices get no twin: they ARE
    // the join, and duplicating them is exactly what would leave the result as
    // two shells touching rather than one closed surface.
    let mut first = vec![NONE; work.len()];
    let mut source = Vec::with_capacity(kept_triangles * 2);
    for i in 0..work.len() {
        if flags[i] & USED == 0 {
            continue;
        }
        first[i] = source.len() as u32;
        source.push(i as u32);
        if axis.coord(work[i]) != 0.0 {
            source.push(i as u32);
        }
    }
    let twin = |i: u32| first[i as usize] + (axis.coord(work[i as usize]) != 0.0) as u32;

    let out_indices: Vec<u32> = par::gather(triangle_count, |t, out| {
        if !keep[t] {
            return;
        }
        let [i0, i1, i2] = [indices[t * 3], indices[t * 3 + 1], indices[t * 3 + 2]];
        // Pinning can collapse a triangle that had two corners across the
        // plane down onto a line. Dropped rather than kept: a zero-area face
        // has no normal, and both halves would carry one.
        let [p0, p1, p2] = [i0, i1, i2].map(|i| work[i as usize]);
        if i0 == i1 || i1 == i2 || i0 == i2 || (p1 - p0).cross(p2 - p0).length_squared() <= 1e-20 {
            return;
        }
        // A triangle now lying wholly IN the plane is its own mirror. Pinning
        // the rim can flatten a thin protrusion into the plane, and emitting
        // the pair would put two coincident faces on the same three vertices.
        let [a0, a1, a2] = [i0, i1, i2].map(|i| first[i as usize]);
        let [m0, m1, m2] = [i0, i1, i2].map(twin);
        if m0 == a0 && m1 == a1 && m2 == a2 {
            return;
        }
        // Reflection flips handedness, so the mirrored copy is wound the other
        // way round or the whole new half would face inward. The swap is of the
        // LAST two corners, keeping the first in place, so a normal pass that
        // takes a face's edges from that corner sums exactly negated terms on
        // the two halves.
        out.extend([a0, a1, a2, m0, m2, m1]);
    });
    if out_indices.is_empty() {
        return None;
    }

    let out_positions: Vec<Vec3> = source
        .par_iter()
        .with_min_len(8192)
        .enumerate()
        .map(|(k, &i)| if first[i as usize] == k as u32 { work[i as usize] } else { axis.reflect(work[i as usize]) })
        .collect();

    Some(MirrorWelded {
        mesh: TriMesh::new(out_positions, out_indices),
        source,
        kept_triangles,
        discarded_triangles: triangle_count - kept_triangles,
    })
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
    use crate::fixtures::{duplicate_positions, edge_census, exact_partners, octa_sphere};
    use glam::{Mat4, Quat};

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

    /// A mask painted on the kept side comes out mirrored on the rebuilt side.
    #[test]
    fn a_mask_survives_and_is_mirrored() {
        let mut mesh = octa_sphere(9, 1.0);
        mesh.transform(&Mat4::from_rotation_y(0.2));
        let mask: Vec<f32> = mesh.positions.iter().map(|p| (p.y * 3.0).sin() * 0.5 + 0.5).collect();
        let welded = mirror_and_weld(&mesh.positions, &mesh.indices, Axis::X, true, 1e-4).unwrap();
        let carried = welded.carry(&mask);
        assert_eq!(carried.len(), welded.mesh.vertex_count());
        assert_eq!(carried, carry(&welded.source, &mask));

        let partners = exact_partners(&welded.mesh.positions, Axis::X).unwrap();
        for (k, &twin) in partners.iter().enumerate() {
            assert_eq!(carried[k], carried[twin as usize], "vertex {k} and its mirror image disagree");
            assert_eq!(carried[k], mask[welded.source[k] as usize]);
            // Every new vertex is its source vertex, moved at most onto the plane or across it.
            let from = mesh.positions[welded.source[k] as usize];
            let p = welded.mesh.positions[k];
            assert!((p.y, p.z) == (from.y, from.z) && p.x.abs() <= from.x.abs());
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
        let mut mesh = octa_sphere(354, 1.0);
        mesh.transform(&Mat4::from_quat(Quat::from_euler(glam::EulerRot::XYZ, 0.31, 0.47, 0.83)));
        let mask: Vec<f32> = mesh.positions.iter().map(|p| p.y).collect();
        let clock = std::time::Instant::now();
        let welded = mirror_and_weld(&mesh.positions, &mesh.indices, Axis::X, true, 1e-4).unwrap();
        let carried = welded.carry(&mask);
        let took = clock.elapsed();
        println!(
            "mirror_and_weld: {} triangles, {} vertices -> {} triangles, {} vertices in {:.1} ms (mask carried)",
            mesh.triangle_count(),
            mesh.vertex_count(),
            welded.mesh.triangle_count(),
            welded.mesh.vertex_count(),
            took.as_secs_f64() * 1e3
        );
        assert_eq!(carried.len(), welded.mesh.vertex_count());
        assert_closed_and_symmetric(&welded.mesh, Axis::X, "1M");

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
