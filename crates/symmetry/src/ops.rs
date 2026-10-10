//! What each symmetry command computes, from the mesh's arrays to a result the
//! app can commit: Symmetry Status, Make Symmetric, Cut & Mirror and Symmetry
//! Cleanup.
//!
//! Nothing here touches a live mesh. Every function reads slices and returns
//! either the vertices that moved (`Moved`, for an ordinary sparse undo step)
//! or a whole new mesh with where each vertex came from, and nothing at all
//! when the command would change nothing, so the app never records an undo
//! step that undoes nothing. Each result has a `message()` for the status line;
//! a command that cannot run says why as an `Err` in the same plain language.
//!
//! The one rule worth stating up front: a result with a new mesh changes
//! topology, so everything derived from the old one (adjacency, the brush
//! index, normals, the GPU buffers) has to be rebuilt, not patched.

use glam::Vec3;
use rayon::prelude::*;
use sculpt_geom::TriMesh;

use crate::cells::CellSet;
use crate::map::{self, SymmetryMap};
use crate::project::{ProjectOptions, project_onto_mirror};
use crate::repair::{self, Moved};
use crate::{Axis, par, weld};

/// Multiplier range offered in the UI, around `map::default_tolerance`. Wide
/// enough to pair a coarsely remeshed model (whose two halves land further
/// apart) without being so wide that it starts pairing a vertex with its
/// reflection's neighbour.
pub const MIN_TOLERANCE_SCALE: f32 = 0.25;
pub const MAX_TOLERANCE_SCALE: f32 = 8.0;

/// The most of a model that may be left UNPAIRED before Make Symmetric refuses
/// to run, as a fraction of the off-centreline vertices.
///
/// A partial mirror is not a partial repair; it is a tear. Copying one side
/// onto the other only moves vertices that HAVE a counterpart. Where paired and
/// unpaired vertices are interleaved across the same surface, half of them jump
/// to their mirrored positions and their immediate neighbours do not, and the
/// surface between them is pulled into ribbons.
///
/// Calibrated against a body with two arms joined in, one of them rotated:
/// 71,427 vertices, 23,673 of them unmatched (35% of the off-plane vertices),
/// every pair the map made an exact reflection. 5% leaves room for the drift
/// this tool is FOR while catching that case. Measured after propagation, so
/// what it counts is the two halves not being tessellated alike, which is
/// exactly what mirroring through a correspondence cannot repair.
pub const MAX_UNMATCHED_FRACTION: f32 = 0.05;

/// How close two points must be, as a fraction of the default tolerance, to
/// count as literally the same point during a cleanup weld. Small enough that
/// only vertices a mesh format split apart on purpose (UV seams, poles) land
/// inside it. Nothing a person sculpts ends up this close by accident.
pub const COINCIDENT_FRACTION: f32 = 0.02;

/// How close the two halves of a matched pair must be, as a fraction of the
/// vertex spacing, for Cleanup to treat them as one duplicated seam vertex
/// rather than two real ones. A joined seam's copies are essentially
/// coincident, while a genuine pair straddling the plane sits most of a spacing
/// apart.
pub const SEAM_DUPLICATE_FRACTION: f32 = 0.1;

/// How far the masked repair may move a vertex onto the mirrored surface, as a
/// fraction of the model's bounding diagonal. Generous: the areas this is for
/// are the visibly lopsided ones.
pub const PROJECT_REACH_FRACTION: f32 = 0.03;

/// How far past the unmasked area (and its mirror image) the masked repair's
/// working region extends, in vertex spacings. Wide enough that pairing has
/// intact surroundings to grow in from.
pub const MASKED_REGION_MARGIN_SPACINGS: f32 = 8.0;

/// A mask value above this counts as "something is masked".
const MASK_PRESENT: f32 = 0.001;

/// The matching tolerance for a mesh at a Match Tolerance slider setting.
/// Bounded by the mesh's own vertex spacing, not its bounding box alone; see
/// `map::default_tolerance` for why the box-only rule becomes too loose as a
/// mesh gets denser.
pub fn tolerance(positions: &[Vec3], indices: &[u32], tolerance_scale: f32) -> f32 {
    map::default_tolerance(positions, indices) * tolerance_scale.max(0.01)
}

/// The correspondence map every command here starts from, or `None` for a mesh
/// with no vertices.
pub fn build_map(positions: &[Vec3], indices: &[u32], axis: Axis, tolerance_scale: f32) -> Option<SymmetryMap> {
    if positions.is_empty() {
        return None;
    }
    check_triangles(positions, indices);
    Some(SymmetryMap::build(positions, indices, axis, tolerance(positions, indices, tolerance_scale)))
}

/// One-line symmetry report for the UI. Worked out on demand rather than
/// cached: the map is invalidated by every brush stroke, and a figure that
/// silently described the mesh as it was several strokes ago would be worse
/// than no figure at all.
pub fn status(positions: &[Vec3], indices: &[u32], axis: Axis, tolerance_scale: f32) -> String {
    build_map(positions, indices, axis, tolerance_scale).map_or_else(|| "No geometry".to_string(), |map| map.summary())
}

/// A triangle list naming vertices the mesh does not have is a bug in the
/// caller, not something a user did.
fn check_triangles(positions: &[Vec3], indices: &[u32]) {
    let n = positions.len() as u64;
    let in_range = indices.par_iter().with_min_len(1 << 16).all(|&i| (i as u64) < n);
    assert!(in_range, "a triangle names a vertex the mesh does not have");
}

fn check_mask(positions: &[Vec3], mask: Option<&[f32]>) {
    assert!(mask.is_none_or(|m| m.len() == positions.len()), "the mask has a different number of vertices than the mesh");
}

/// The side being copied from and the side being written, as the UI names them.
fn sides(axis: Axis, source_is_positive: bool) -> (String, String) {
    let (plus, minus) = (format!("+{}", axis.name()), format!("-{}", axis.name()));
    if source_is_positive { (plus, minus) } else { (minus, plus) }
}

/// What Make Symmetric did.
#[derive(Clone, Debug, PartialEq)]
pub struct Symmetrized {
    /// The vertices to move, empty when the model already matched.
    pub moved: Moved,
    pub axis: Axis,
    pub source_is_positive: bool,
    /// Vertices copied across through a pair.
    pub changed: usize,
    /// Pairs the map found; under a mask, those whose destination end is free
    /// to move.
    pub pairs: usize,
    /// Vertices with no counterpart; under a mask, those in the unmasked area.
    pub unmatched: usize,
    /// Unmatched vertices carried along with the surface around them.
    pub carried: usize,
    /// Whether a mask confined the repair to the unmasked area.
    pub masked: bool,
    /// Vertices laid onto the mirrored surface rather than copied through a
    /// pair (masked repairs only).
    pub projected: usize,
}

impl Symmetrized {
    /// The status line for this outcome. "Nothing visibly happened" and "the
    /// map could not pair anything" look identical in the viewport, so the
    /// counts ride along.
    pub fn message(&self) -> String {
        let axis = self.axis.name();
        let (from, to) = sides(self.axis, self.source_is_positive);
        if self.masked {
            if self.changed == 0 && self.projected == 0 {
                return format!(
                    "Unmasked area already mirrors {from} across {axis} - {} pairs, {} unpaired. \
                     Check the Symmetry Plane axis is the one you mean.",
                    self.pairs, self.unmatched
                );
            }
            let laid = if self.projected > 0 {
                format!(", {} unpaired vertices laid onto the mirrored {from} surface", self.projected)
            } else {
                String::new()
            };
            let (changed, pairs) = (self.changed, self.pairs);
            return format!("Mirrored {from} onto {to} inside the unmasked area: {changed} of {pairs} pairs{laid}");
        }
        if self.changed == 0 {
            return format!("Already symmetric across {axis} - {} pairs match", self.pairs);
        }
        // The unmatched count rides along on success too: it is the part of
        // the model that has no counterpart to be mirrored onto, and leaving it
        // out is what let a partial mirror look like a complete one.
        let leftover = match (self.unmatched, self.carried) {
            (0, _) => String::new(),
            (unmatched, 0) => format!(", {unmatched} unmatched"),
            (unmatched, carried) => format!(", {carried} of {unmatched} unmatched carried along"),
        };
        format!("Mirrored {from} onto {to}: {} of {} pairs{leftover}", self.changed, self.pairs)
    }
}

/// Make Symmetric: copies one side of the model onto the other through the
/// correspondence map, pinning the centreline first so the seam cannot drift.
/// Vertex-only, so it can never punch a hole in the model, which is what makes
/// it safe to offer as a one-click repair.
///
/// Refuses, changing nothing, when mirroring would tear the model rather than
/// repair it (`MAX_UNMATCHED_FRACTION`); the message names Cut & Mirror
/// (`mirror_and_weld`), because "too asymmetric" with no way forward is what
/// makes a refusal read as the tool being broken.
///
/// With anything masked, only the unmasked area is repaired (soft mask edges
/// blend), and it never refuses: where the area does not pair up it is laid
/// onto the other side's mirrored surface instead. See
/// `make_symmetric_masked`.
pub fn make_symmetric(
    positions: &[Vec3],
    indices: &[u32],
    mask: Option<&[f32]>,
    axis: Axis,
    tolerance_scale: f32,
    source_is_positive: bool,
) -> Result<Symmetrized, String> {
    check_mask(positions, mask);
    if positions.is_empty() {
        return Err("No geometry to mirror".to_string());
    }
    if let Some(mask) = mask.filter(|m| m.par_iter().with_min_len(1 << 16).any(|&v| v > MASK_PRESENT)) {
        check_triangles(positions, indices);
        return make_symmetric_masked(positions, indices, mask, axis, tolerance_scale, source_is_positive);
    }

    let map = build_map(positions, indices, axis, tolerance_scale).ok_or("No geometry to mirror")?;
    let (pairs, unmatched) = (map.pair_count(), map.unmatched_count());
    let name = axis.name();
    let (from, to) = sides(axis, source_is_positive);

    // Refuse rather than half-apply. Measured against off-plane vertices only:
    // the centreline is symmetric by definition and would otherwise dilute the
    // fraction on a model with a dense seam, making a torn result look
    // acceptable.
    let off_plane = map.vertex_count() - map.on_plane_count();
    if off_plane > 0 && unmatched as f32 > off_plane as f32 * MAX_UNMATCHED_FRACTION {
        return Err(format!(
            "Too asymmetric to match up: {unmatched} vertices have no counterpart across {name} ({pairs} pairs do). \
             Nudging vertices would tear those apart - use Cut & Mirror {from} to {to} instead, which rebuilds that \
             side outright."
        ));
    }
    if pairs == 0 {
        return Err(format!("Nothing paired across {name} - raise Match Tolerance"));
    }

    let mut working = positions.to_vec();
    repair::snap_to_plane(&mut working, &map, None);
    let changed = repair::make_symmetric(&mut working, &map, source_is_positive, None);

    // Whatever is left unpaired after propagation is not pairable at all (the
    // two sides are not tessellated alike there), so it is carried along by
    // the deformation around it rather than left standing where its
    // neighbours used to be. That last fraction of a percent is the whole
    // visible difference between "symmetric" and "nearly symmetric".
    let carried = repair::carry_unmatched(&mut working, positions, &map, source_is_positive, None);

    // Carrying keeps those vertices attached to the surface; seating them
    // makes them lie IN it. Only when this press actually moved something:
    // reseating is a relaxation, so running it on a press that mirrored
    // nothing kept nudging the unmatched vertices on every repeat, which reads
    // as a repair that never settles.
    if changed > 0 || carried > 0 {
        repair::reseat_unmatched(&mut working, positions, &map, source_is_positive, None);
    }

    Ok(Symmetrized {
        moved: Moved::between(positions, &working),
        axis,
        source_is_positive,
        changed,
        pairs,
        unmatched,
        carried,
        masked: false,
        projected: 0,
    })
}

/// The masked repair: only the unmasked area moves, and it never refuses.
///
/// Works on a REGION of the model rather than all of it (the unmasked
/// vertices, the place their mirror images fall, and a margin around both),
/// because the correspondence map is the expensive part and only that region's
/// pairs are ever used.
///
/// Inside the region the pairs are copied across exactly as in the whole-model
/// repair, and every unpaired vertex in the unmasked area is then laid onto
/// the source side's mirrored SURFACE instead of being carried. That matters
/// more here than anywhere: where two sides differ enough to want a local
/// repair they differ too much to pair, so the unpaired vertices ARE the
/// asymmetry, and a pairs-only pass reports "already matches" and leaves it
/// standing. The surface they are laid onto is the whole model's, not the
/// region's: the mask (and the region) decide what moves, never what it is
/// measured against.
///
/// Where the area mostly does not pair (`MAX_UNMATCHED_FRACTION`), its two
/// sides are not tessellated alike and the pairs that do exist are near-misses
/// rather than true counterparts; copying through them shears the surface. So
/// the pair pass is skipped there and the whole area is projected.
fn make_symmetric_masked(
    positions: &[Vec3],
    indices: &[u32],
    mask: &[f32],
    axis: Axis,
    tolerance_scale: f32,
    source_is_positive: bool,
) -> Result<Symmetrized, String> {
    const NONE: u32 = u32::MAX;
    let n = positions.len();
    let spacing = map::mean_edge_length(positions, indices);
    let diagonal = map::diagonal(positions);
    let tolerance = map::tolerance_for(diagonal, spacing) * tolerance_scale.max(0.01);
    let (from, to) = sides(axis, source_is_positive);
    let nothing_free = || {
        format!(
            "Nothing unmasked on the {to} side to change - if the area you left unmasked is on {from}, use {to} to {from}"
        )
    };

    // The region, as coarse cells: wherever an unmasked vertex or its mirror
    // image is, plus the ring of cells around each.
    let cell = (spacing * MASKED_REGION_MARGIN_SPACINGS).max(diagonal * 1e-4).max(1e-6);
    let free = par::indices_where(n, |i| mask[i] < 1.0);
    let cells = CellSet::around(
        free.par_iter().flat_map_iter(|&i| [positions[i as usize], axis.reflect(positions[i as usize])]),
        cell,
    );
    if cells.is_empty() {
        return Err(nothing_free());
    }
    let region = par::indices_where(n, |i| mask[i] < 1.0 || cells.contains(positions[i]));
    let mut slot = vec![NONE; n];
    region.iter().enumerate().for_each(|(k, &i)| slot[i as usize] = k as u32);

    let region_positions = weld::carry(&region, positions);
    let region_mask = weld::carry(&region, mask);
    let region_indices: Vec<u32> = par::gather(indices.len() / 3, |t, out| {
        let [a, b, c] = [indices[t * 3], indices[t * 3 + 1], indices[t * 3 + 2]].map(|i| slot[i as usize]);
        if a != NONE && b != NONE && c != NONE {
            out.extend([a, b, c]);
        }
    });

    let map = SymmetryMap::build(&region_positions, &region_indices, axis, tolerance);

    // Counted over the unmasked, off-plane part of the region; a pair counts
    // when its destination end is free (the same source/destination rule as
    // `repair::make_symmetric`).
    let count = region.len();
    let free_off_plane = |k: usize| region_mask[k] < 1.0 && !map.is_on_plane(k as u32);
    let off_plane = par::count(count, free_off_plane);
    let unmatched = par::count(count, |k| free_off_plane(k) && map.is_unmatched(k as u32));
    let pairs = par::count(count, |k| {
        free_off_plane(k)
            && map.partner_of(k as u32).is_some_and(|partner| {
                let (own, other) = (axis.coord(region_positions[k]), axis.coord(region_positions[partner as usize]));
                if source_is_positive { own < other } else { own > other }
            })
    });
    let use_pairs = !(off_plane > 0 && unmatched as f32 > off_plane as f32 * MAX_UNMATCHED_FRACTION);

    let mut working = region_positions.clone();
    let mut changed = 0;
    if use_pairs {
        repair::snap_to_plane(&mut working, &map, Some(&region_mask));
        changed = repair::make_symmetric(&mut working, &map, source_is_positive, Some(&region_mask));
    }
    let options = ProjectOptions {
        unpaired_only: use_pairs,
        original: Some(&region_positions),
        source: Some((positions, indices)),
    };
    let reach = diagonal * PROJECT_REACH_FRACTION;
    let projected =
        project_onto_mirror(&mut working, &region_indices, &map, source_is_positive, Some(&region_mask), reach, options);

    // Only unmasked vertices are ever written back, whatever the passes above did.
    let moved_slots =
        par::indices_where(count, |k| region_mask[k] < 1.0 && working[k] != region_positions[k]);
    let moved = Moved {
        indices: moved_slots.iter().map(|&k| region[k as usize]).collect(),
        positions: weld::carry(&moved_slots, &working),
    };
    // The one extra way to get nothing: the area sits on the side being copied
    // FROM, which is never written.
    if moved.is_empty() && changed == 0 && projected == 0 && pairs == 0 {
        return Err(nothing_free());
    }
    Ok(Symmetrized { moved, axis, source_is_positive, changed, pairs, unmatched, carried: 0, masked: true, projected })
}

/// What Cut & Mirror built.
#[derive(Clone, Debug, PartialEq)]
pub struct CutAndMirrored {
    pub mesh: TriMesh,
    /// For each vertex of `mesh`, the vertex of the old mesh it was made from.
    /// A vertex made where an edge crossed the plane names the kept end of
    /// that edge.
    pub source: Vec<u32>,
    /// The old mesh's mask carried onto `mesh` (mirrored along with the
    /// surface), when one was given. Along the cut it is what the surface had
    /// at each new vertex, between the two ends of the edge it was made on.
    pub mask: Option<Vec<f32>>,
    pub axis: Axis,
    pub source_is_positive: bool,
    /// Triangles of the old mesh kept, whole or in part.
    pub kept_triangles: usize,
    pub discarded_triangles: usize,
}

impl CutAndMirrored {
    /// Reports what was THROWN AWAY as well as what was built, because that is
    /// the part this operation cannot undo by pressing the other direction: a
    /// user who meant the opposite direction should see that immediately.
    pub fn message(&self) -> String {
        let (from, to) = sides(self.axis, self.source_is_positive);
        format!(
            "Cut & mirrored {from} onto {to}: kept {} triangles, replaced {}, now {} vertices",
            self.kept_triangles,
            self.discarded_triangles,
            self.mesh.vertex_count()
        )
    }
}

/// Cut & Mirror: cuts the model at the mirror plane, through the triangles that
/// cross it, and rebuilds the discarded side as a reflection of the kept one
/// (`weld::mirror_and_weld`). A closed model comes out closed, with the two
/// halves sharing one loop of vertices on the plane.
///
/// This is the answer for everything Make Symmetric refuses. Nothing about a
/// correspondence map can fix a model whose halves were built separately: the
/// correspondence genuinely is not there. Throwing one side away and
/// reflecting the other always can, because it never needs to know which
/// vertex was which.
///
/// Uses the same seam band the pairing uses, so the Match Tolerance slider
/// means one consistent thing across the panel: how far off the centreline a
/// vertex may sit and still count as being on it.
pub fn mirror_and_weld(
    positions: &[Vec3],
    indices: &[u32],
    mask: Option<&[f32]>,
    axis: Axis,
    tolerance_scale: f32,
    source_is_positive: bool,
) -> Result<CutAndMirrored, String> {
    check_mask(positions, mask);
    check_triangles(positions, indices);
    let seam = tolerance(positions, indices, tolerance_scale);
    let built = weld::mirror_and_weld(positions, indices, axis, source_is_positive, seam)
        .ok_or_else(|| format!("Nothing on the {} side to mirror", sides(axis, source_is_positive).0))?;
    let mask = mask.map(|m| built.carry_blended(m));
    Ok(CutAndMirrored {
        mesh: built.mesh,
        source: built.source,
        mask,
        axis,
        source_is_positive,
        kept_triangles: built.kept_triangles,
        discarded_triangles: built.discarded_triangles,
    })
}

/// How Symmetry Cleanup changes the mesh.
#[derive(Clone, Debug, PartialEq)]
pub enum CleanupChange {
    /// The model was already clean.
    Nothing,
    /// Vertices were snapped onto the plane and nothing needed merging: same
    /// topology, some positions moved.
    Moved(Moved),
    /// Vertices were merged: a new mesh.
    Rebuilt {
        mesh: TriMesh,
        /// For each vertex of `mesh`, the vertex of the old mesh that survived as it.
        source: Vec<u32>,
        /// The old mesh's mask carried onto `mesh`, when one was given.
        mask: Option<Vec<f32>>,
    },
}

/// What Symmetry Cleanup did.
#[derive(Clone, Debug, PartialEq)]
pub struct Cleaned {
    pub change: CleanupChange,
    pub axis: Axis,
    /// Vertices pulled onto the plane.
    pub snapped: usize,
    /// Vertices then merged away.
    pub welded: usize,
}

impl Cleaned {
    pub fn message(&self) -> String {
        let axis = self.axis.name();
        match (self.snapped, self.welded) {
            (0, 0) => format!("Already clean across {axis} - nothing to do"),
            (snapped, 0) => format!("Snapped {snapped} vertices onto {axis} - no duplicates found"),
            (0, welded) => format!("Welded {welded} duplicate vertices"),
            (snapped, welded) => format!("Snapped {snapped} onto {axis}, welded {welded} duplicate vertices"),
        }
    }
}

/// Symmetry Cleanup: snaps near-centreline vertices exactly onto the plane,
/// then welds the duplicates that leaves behind, so a seam that was two
/// coincident shells becomes one shared edge loop. The repair for a model
/// joined from two mirrored halves.
///
/// The mask, if given, is only carried onto a rebuilt mesh; cleanup is not
/// confined by it.
pub fn cleanup(
    positions: &[Vec3],
    indices: &[u32],
    mask: Option<&[f32]>,
    axis: Axis,
    tolerance_scale: f32,
) -> Result<Cleaned, String> {
    check_mask(positions, mask);
    let map = build_map(positions, indices, axis, tolerance_scale).ok_or("No geometry to clean up")?;

    let mut working = positions.to_vec();
    let mut snapped = repair::snap_to_plane(&mut working, &map, None);
    // The map files a seam's duplicated copies as a PAIR (they are each
    // other's reflection), so `snap_to_plane` alone does not pin them; this
    // does, and only them.
    snapped += repair::snap_seam_duplicates(&mut working, &map, map.mean_spacing() * SEAM_DUPLICATE_FRACTION);

    // Deliberately derived from the mesh alone rather than from the map's
    // tolerance: the Match Tolerance slider widens what counts as a PAIR, a
    // judgement about how far apart two halves may have drifted and still mean
    // the same thing. How close two points must be to be the SAME point is not
    // that judgement, and letting the slider scale it would turn a loose
    // pairing setting into a licence to collapse real geometry.
    let coincident = map::default_tolerance(&working, indices) * COINCIDENT_FRACTION;

    let change = match weld::weld(&working, indices, axis, map.tolerance(), coincident) {
        Some(welded) => {
            let mask = mask.map(|m| welded.carry(m));
            let merged = welded.merged();
            return Ok(Cleaned {
                change: CleanupChange::Rebuilt { mesh: welded.mesh, source: welded.source, mask },
                axis,
                snapped,
                welded: merged,
            });
        }
        // Snapping alone moved geometry, so there is still something to commit.
        None if snapped > 0 => CleanupChange::Moved(Moved::between(positions, &working)),
        None => CleanupChange::Nothing,
    };
    Ok(Cleaned { change, axis, snapped, welded: 0 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{
        Faults, Lcg, bumpy_sphere, cube_sphere, cube_sphere_with, degenerate_triangles, duplicate_positions, edge_census,
        icosphere, max_delta, octa_sphere, scatter,
    };
    use crate::grid::VertexGrid;

    /// The kind of mesh these commands actually get used on: a symmetric shape whose two halves
    /// are tessellated alike but with every vertex a little out of true, as a remesh leaves it,
    /// and with the mirror plane running BETWEEN two rows of vertices rather than through one.
    /// (Stands in for the Unity tests' remeshed icosphere; the remesher lives in another crate.)
    struct Fixture {
        mesh: TriMesh,
        spacing: f32,
    }

    impl Fixture {
        fn new(n: u32) -> Fixture {
            let mut mesh = cube_sphere(n);
            let spacing = map::mean_edge_length(&mesh.positions, &mesh.indices);
            // Mostly a few percent of a spacing, now and then a fifth of one: pairs end up as far
            // out of true as a voxel remesh leaves them (up to about 0.45 spacings).
            let mut rng = Lcg::new(77);
            for p in &mut mesh.positions {
                let amount = 0.01 + 0.12 * rng.unit().powi(8);
                *p += rng.in_cube() * (spacing * amount);
            }
            Fixture { mesh, spacing }
        }

        /// A broad bump on the +X half away from the plane, and a second one that runs right up
        /// to it, cut off sharply AT the plane, which is what drift near a centreline looks like.
        fn drifted(&self) -> Vec<Vec3> {
            let (far, near) = (Vec3::new(0.25, 0.1, -0.35), Vec3::new(0.03, 0.3, -0.25));
            let mut positions = self.mesh.positions.clone();
            for p in &mut positions {
                if p.x <= 0.0 {
                    continue;
                }
                let far_bump = (-(*p - far).length_squared() / (2.0 * 0.08 * 0.08)).exp();
                let near_bump = (-(*p - near).length_squared() / (2.0 * 0.06 * 0.06)).exp();
                *p += p.normalize() * (self.spacing * (4.0 * far_bump + 2.0 * near_bump));
            }
            positions
        }

        /// How far each vertex's reflection is from the nearest vertex: `(mean, max, count
        /// beyond one spacing)`, each distance capped at a spacing and a half.
        fn mirror_residual(&self, positions: &[Vec3]) -> (f32, f32, usize) {
            let grid = VertexGrid::build(positions, self.spacing);
            let (mut sum, mut max, mut beyond) = (0.0f64, 0.0f32, 0);
            for &p in positions {
                let nearest = grid.nearest(Axis::X.reflect(p), 1.5 * self.spacing);
                let d = nearest.map_or(1.5 * self.spacing, |(_, sqr)| sqr.sqrt());
                beyond += (d > self.spacing) as usize;
                sum += d as f64;
                max = max.max(d);
            }
            ((sum / positions.len() as f64) as f32, max, beyond)
        }
    }

    fn apply(positions: &[Vec3], moved: &Moved) -> Vec<Vec3> {
        let mut after = positions.to_vec();
        moved.apply(&mut after);
        after
    }

    /// A symmetric shape must come back reported as one: every vertex near the centreline either
    /// IS on it or has a twin across it, and no pair that clearly has a twin gets filed as a
    /// centreline vertex instead.
    #[test]
    fn noisy_symmetric_shape_classifies_centreline_correctly() {
        let fixture = Fixture::new(61);
        let v = &fixture.mesh.positions;
        let map = build_map(v, &fixture.mesh.indices, Axis::X, 1.0).unwrap();
        assert!(map.is_symmetric(), "{}", map.summary());

        // On-plane vertices (by the map) with some unmatched vertex across the plane closer to
        // their reflection than they are themselves: a pair the map genuinely missed.
        let grid = VertexGrid::build(v, fixture.spacing);
        let mut missed = 0;
        for i in 0..v.len() as u32 {
            let off = v[i as usize].x.abs();
            if !map.is_on_plane(i) || off <= 0.05 * map.tolerance() {
                continue;
            }
            let side = v[i as usize].x;
            let twin = grid.nearest_where(Axis::X.reflect(v[i as usize]), 2.0 * off, |j, q| j != i && q.x * side < 0.0);
            missed += twin.is_some_and(|(j, sqr)| sqr.sqrt() < 2.0 * off && map.is_unmatched(j)) as usize;
        }
        assert_eq!(missed, 0, "{}", map.summary());
        // With the plane between two rows there is no centreline at all here: the rows either
        // side, half a spacing out and inside the band, are pairs.
        assert_eq!(map.on_plane_count(), 0, "{}", map.summary());
        assert_eq!(status(v, &fixture.mesh.indices, Axis::X, 1.0), map.summary());
    }

    /// On a model that is already symmetric the repair must be close to a no-op: no vertex
    /// dragged anywhere, and above all no edge collapsed into a degenerate triangle.
    #[test]
    fn make_symmetric_on_symmetric_shape_does_no_damage() {
        let fixture = Fixture::new(61);
        let (v, indices) = (&fixture.mesh.positions, &fixture.mesh.indices);
        let degenerate_before = degenerate_triangles(v, indices, fixture.spacing);
        for source_is_positive in [true, false] {
            let result = make_symmetric(v, indices, None, Axis::X, 1.0, source_is_positive).unwrap();
            let after = apply(v, &result.moved);
            assert!(degenerate_triangles(&after, indices, fixture.spacing) <= degenerate_before, "{}", result.message());
            // Not near zero: the pairs sit a good fraction of a spacing out of true, and lining
            // those up is the job. A vertex dragged well past that went somewhere it had no
            // business going.
            assert!(max_delta(v, &after) <= 1.5 * fixture.spacing, "{}", result.message());
            assert!(result.changed > 0 && !result.masked);
            let (mean, _, _) = fixture.mirror_residual(&after);
            assert!(mean < 1e-3 * fixture.spacing, "residual {mean}");
        }
    }

    /// The job it exists for: one side pushed out of shape, including right up against the
    /// centreline. The repair must line the halves up, leave no collapsed geometry behind, and
    /// be idempotent: pressing it again must not keep moving things.
    #[test]
    fn make_symmetric_repairs_drift_and_is_idempotent() {
        let fixture = Fixture::new(61);
        let indices = &fixture.mesh.indices;
        let drifted = fixture.drifted();
        let degenerate_before = degenerate_triangles(&drifted, indices, fixture.spacing);
        let (drift_mean, drift_max, _) = fixture.mirror_residual(&drifted);
        assert!(drift_max > fixture.spacing && drift_mean > 0.05 * fixture.spacing);

        for source_is_positive in [true, false] {
            let first = make_symmetric(&drifted, indices, None, Axis::X, 1.0, source_is_positive).unwrap();
            let after_first = apply(&drifted, &first.moved);
            let (mean, _, _) = fixture.mirror_residual(&after_first);
            let (unmatched, carried) = (first.unmatched, first.carried);
            let report = format!("{} (unmatched {unmatched}, carried {carried}, residual {mean})", first.message());

            assert!(first.changed > 0, "{report}");
            assert!(!first.moved.is_empty(), "{report}");
            assert!(degenerate_triangles(&after_first, indices, fixture.spacing) <= degenerate_before, "{report}");
            assert!(mean <= 0.05 * fixture.spacing, "{report}");
            // The side copied FROM is exactly as it was.
            let map = build_map(&drifted, indices, Axis::X, 1.0).unwrap();
            for (i, p) in after_first.iter().enumerate() {
                if repair::on_source_side(drifted[i].x, source_is_positive) && !map.is_on_plane(i as u32) {
                    assert_eq!(*p, drifted[i], "source vertex {i} moved");
                }
            }

            // A second press may still pair a handful of vertices the first had to carry. What
            // it must not do is keep reworking the model.
            let second = make_symmetric(&after_first, indices, None, Axis::X, 1.0, source_is_positive).unwrap();
            let after_second = apply(&after_first, &second.moved);
            assert!(second.changed <= (drifted.len() / 10000).max(1), "second press: {}", second.message());
            assert!(max_delta(&after_first, &after_second) <= 1.5 * fixture.spacing);
            if second.changed == 0 {
                assert!(second.message().starts_with("Already symmetric across X"), "{}", second.message());
            }
        }
    }

    /// The harder mesh: the two halves split their squares along OPPOSITE diagonals, as halves
    /// remeshed separately do, so no vertex has quite the neighbours its twin has, and a vertex
    /// reached along a diagonal has no true counterpart among its anchor's neighbours at all.
    /// Under strong drift the pairing then goes wrong in places: measured here, 160 of 22,328
    /// vertices left unmatched and about 0.2% of the triangles turned over where a small patch
    /// paired one vertex off (the shearing `Pairing::reject_inconsistent_pairs` exists to limit).
    /// That is the pairing's known limit, and why a remesh should be mirror-exact. What must
    /// still hold: the repair goes through, the halves line up, and the damage stays that small.
    #[test]
    fn make_symmetric_on_halves_split_along_opposite_diagonals_stays_contained() {
        let mut fixture = Fixture::new(61);
        fixture.mesh.indices = cube_sphere_with(61, false).indices;
        let indices = &fixture.mesh.indices;
        let drifted = fixture.drifted();

        let result = make_symmetric(&drifted, indices, None, Axis::X, 1.0, true).unwrap();
        let after = apply(&drifted, &result.moved);
        let (mean, _, _) = fixture.mirror_residual(&after);
        let dragged = drifted.iter().zip(&after).filter(|(a, b)| a.distance(**b) > 3.5 * fixture.spacing).count();
        let turned = (0..indices.len() / 3)
            .filter(|&t| {
                let [a, b, c] = [indices[t * 3], indices[t * 3 + 1], indices[t * 3 + 2]].map(|i| after[i as usize]);
                (b - a).cross(c - a).dot(a + b + c) <= 0.0
            })
            .count();
        let report = format!("{}: residual {mean}, {dragged} dragged, {turned} turned over", result.message());
        assert!(result.changed > 10_000, "{report}");
        assert!(result.unmatched * 50 < drifted.len(), "{report}");
        assert!(result.carried > 0, "{report}");
        assert!(mean <= 0.05 * fixture.spacing, "{report}");
        assert!(dragged * 100 < drifted.len() && turned * 100 < indices.len() / 3, "{report}");
    }

    /// Pressing the same button on the same mesh must move the same vertices by the same amounts,
    /// however the work was shared out: one core and all of them give identical results, through
    /// the plain repair, the hard pairing case and the masked repair alike.
    #[test]
    fn results_do_not_depend_on_how_many_cores_did_the_work() {
        let fixture = Fixture::new(41);
        let drifted = fixture.drifted();
        let far = Vec3::new(0.25, 0.1, -0.35);
        let mask: Vec<f32> = drifted.iter().map(|p| ((p.distance(far) - 0.1) * 10.0).clamp(0.0, 1.0)).collect();
        let hard = cube_sphere_with(41, false).indices;
        let run = || {
            (
                make_symmetric(&drifted, &fixture.mesh.indices, None, Axis::X, 1.0, true),
                make_symmetric(&drifted, &hard, None, Axis::X, 1.0, false),
                make_symmetric(&drifted, &fixture.mesh.indices, Some(&mask), Axis::X, 1.0, false),
                mirror_and_weld(&drifted, &hard, Some(&mask), Axis::X, 1.0, true),
                cleanup(&drifted, &fixture.mesh.indices, None, Axis::Y, 4.0),
            )
        };
        let one_core = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap().install(run);
        let all_cores = run();
        assert!(one_core.0.is_ok() && one_core.1.is_ok() && one_core.2.is_ok());
        assert!(one_core.3.is_ok() && one_core.4.is_ok());
        assert!(!one_core.0.as_ref().unwrap().moved.is_empty() && !one_core.2.as_ref().unwrap().moved.is_empty());
        assert_eq!(one_core, all_cores);
        assert_eq!(run(), all_cores, "and the same again on a second run");
    }

    /// With a mask painted, the repair is confined to the unmasked area: the far bump is left
    /// unmasked and must come back off, the near one is masked and must not move by a bit, nor
    /// may anything else that is masked, on either side.
    #[test]
    fn make_symmetric_with_mask_only_changes_the_unmasked_area() {
        let fixture = Fixture::new(61);
        let (start, indices, spacing) = (&fixture.mesh.positions, &fixture.mesh.indices, fixture.spacing);
        let drifted = fixture.drifted();
        let far = Vec3::new(0.25, 0.1, -0.35);
        let mask: Vec<f32> = start.iter().map(|p| if p.x > 0.0 && p.distance(far) < 0.2 { 0.0 } else { 1.0 }).collect();

        let result = make_symmetric(&drifted, indices, Some(&mask), Axis::X, 1.0, false).unwrap();
        let after = apply(&drifted, &result.moved);
        assert!(result.masked, "{}", result.message());
        assert!(result.changed > 0, "{}", result.message());
        assert!(result.message().starts_with("Mirrored -X onto +X inside the unmasked area"), "{}", result.message());
        assert!(result.moved.indices.iter().all(|&i| mask[i as usize] < 1.0), "a masked vertex is in the moved list");

        let (mut free_before, mut free_after, mut masked_standing) = (0.0f32, 0.0f32, 0.0f32);
        for i in 0..start.len() {
            if mask[i] >= 1.0 {
                assert_eq!(after[i], drifted[i], "masked vertex {i} moved");
                masked_standing = masked_standing.max(after[i].distance(start[i]));
            } else {
                free_before = free_before.max(drifted[i].distance(start[i]));
                free_after = free_after.max(after[i].distance(start[i]));
            }
        }
        assert!(free_before > 3.0 * spacing);
        // Same allowance as the unmasked tests: the pairs themselves sit a little out of true.
        assert!(free_after <= 1.5 * spacing, "unmasked worst offset {} spacings", free_after / spacing);
        // The masked bump (about a third of a spacing tall where it meets this surface) is still there.
        assert!(masked_standing > 0.25 * spacing);

        // The same mask with the copy going the other way: the unmasked area is on the side
        // being copied FROM, so there is nothing to change, and the message says which way to go.
        let wrong_way = make_symmetric(&drifted, indices, Some(&mask), Axis::X, 1.0, true).unwrap_err();
        assert!(wrong_way.starts_with("Nothing unmasked on the -X side to change"), "{wrong_way}");
    }

    /// Where the unmasked area does not pair up at all (one side re-tessellated), the masked
    /// repair lays it onto the mirrored surface instead of refusing.
    #[test]
    fn masked_repair_projects_where_the_sides_do_not_pair() {
        // +X half from one tessellation, -X half from a finer, unrelated one.
        let coarse = octa_sphere(24, 0.5);
        let fine = cube_sphere(41);
        let mut positions = Vec::new();
        let mut indices = Vec::new();
        for (mesh, positive) in [(&coarse, true), (&fine, false)] {
            let mut slot = vec![u32::MAX; mesh.vertex_count()];
            for t in 0..mesh.triangle_count() {
                if (mesh.triangle(t).iter().map(|p| p.x).sum::<f32>() > 0.0) != positive {
                    continue;
                }
                for &i in &mesh.indices[t * 3..t * 3 + 3] {
                    if slot[i as usize] == u32::MAX {
                        slot[i as usize] = positions.len() as u32;
                        positions.push(mesh.positions[i as usize]);
                    }
                    indices.push(slot[i as usize]);
                }
            }
        }
        // A dent in the -X side, which is the unmasked area.
        let centre = Vec3::new(-0.35, 0.25, 0.25);
        let free = |p: Vec3| p.x < -0.05 && p.distance(centre) < 0.2;
        let start = positions.clone();
        for p in &mut positions {
            if free(*p) {
                *p *= 1.0 - 0.08 * (1.0 - p.distance(centre) / 0.2);
            }
        }
        let mask: Vec<f32> = start.iter().map(|&p| if free(p) { 0.0 } else { 1.0 }).collect();
        let whole = make_symmetric(&positions, &indices, None, Axis::X, 1.0, true).unwrap_err();
        assert!(whole.starts_with("Too asymmetric to match up"), "{whole}");
        assert!(whole.contains("use Cut & Mirror +X to -X instead"), "{whole}");

        let result = make_symmetric(&positions, &indices, Some(&mask), Axis::X, 1.0, true).unwrap();
        assert!(result.masked && result.projected > 20, "{}", result.message());
        assert!(result.message().contains("laid onto the mirrored +X surface"), "{}", result.message());
        let after = apply(&positions, &result.moved);
        let dent = |v: &[Vec3]| (0..v.len()).filter(|&i| mask[i] < 1.0).map(|i| 0.5 - v[i].length()).fold(0.0, f32::max);
        assert!(dent(&positions) > 0.03);
        assert!(dent(&after) < 0.012, "dent left {} deep", dent(&after));
        for i in 0..after.len() {
            assert!(mask[i] < 1.0 || after[i] == positions[i]);
        }
    }

    #[test]
    fn nothing_to_mirror_and_nothing_paired_say_so() {
        assert_eq!(make_symmetric(&[], &[], None, Axis::X, 1.0, true).unwrap_err(), "No geometry to mirror");
        assert_eq!(status(&[], &[], Axis::Y, 1.0), "No geometry");
        assert!(build_map(&[], &[], Axis::Y, 1.0).is_none());
        // Everything on the centreline: nothing to copy across.
        let flat = TriMesh::new(vec![Vec3::ZERO, Vec3::Y, Vec3::Z], vec![0, 1, 2]);
        let error = make_symmetric(&flat.positions, &flat.indices, None, Axis::X, 1.0, true).unwrap_err();
        assert_eq!(error, "Nothing paired across X - raise Match Tolerance");
        // An exactly symmetric model: nothing to move, and it says so.
        let (sphere, _) = icosphere(3);
        let same = make_symmetric(&sphere.positions, &sphere.indices, None, Axis::Z, 1.0, false).unwrap();
        assert!(same.moved.is_empty());
        assert_eq!(same.message(), format!("Already symmetric across Z - {} pairs match", same.pairs));
        // The slider's lowest setting still gives a usable tolerance.
        let at = |scale: f32| tolerance(&sphere.positions, &sphere.indices, scale);
        assert!(at(0.0) > 0.0 && at(MIN_TOLERANCE_SCALE) < at(MAX_TOLERANCE_SCALE));
    }

    #[test]
    fn cut_and_mirror_rebuilds_a_lopsided_model_and_carries_the_mask() {
        let fixture = Fixture::new(41);
        let drifted = fixture.drifted();
        let indices = &fixture.mesh.indices;
        let mask: Vec<f32> = drifted.iter().map(|p| (p.y + 0.5).clamp(0.0, 1.0)).collect();

        let built = mirror_and_weld(&drifted, indices, Some(&mask), Axis::X, 1.0, true).unwrap();
        assert_eq!(edge_census(&built.mesh), (0, 0));
        assert_eq!(Faults::of(&built.mesh), Faults::NONE);
        assert!(built.mesh.signed_volume() > 0.0);
        assert_eq!(duplicate_positions(&built.mesh.positions), 0);
        assert_eq!(degenerate_triangles(&built.mesh.positions, &built.mesh.indices, fixture.spacing), 0);
        assert_eq!(built.kept_triangles + built.discarded_triangles, fixture.mesh.triangle_count());
        // The mask is where it was: every vertex has what the surface had at that point (0.5
        // on the plane's own vertices, which are on y's ramp like the rest), on both sides.
        let carried = built.mask.as_deref().unwrap();
        assert_eq!(carried.len(), built.mesh.vertex_count());
        for (k, &p) in built.mesh.positions.iter().enumerate() {
            let from = drifted[built.source[k] as usize];
            let moved = p.distance(if p.x < 0.0 { Axis::X.reflect(from) } else { from });
            assert!((carried[k] - (p.y + 0.5).clamp(0.0, 1.0)).abs() <= moved + 1e-5, "vertex {k}: {}", carried[k]);
        }
        assert_eq!(
            built.message(),
            format!(
                "Cut & mirrored +X onto -X: kept {} triangles, replaced {}, now {} vertices",
                built.kept_triangles,
                built.discarded_triangles,
                built.mesh.vertex_count()
            )
        );
        // The result is exactly symmetric, so Make Symmetric has nothing left to do.
        let map = build_map(&built.mesh.positions, &built.mesh.indices, Axis::X, 1.0).unwrap();
        assert!(map.is_symmetric(), "{}", map.summary());
        let again = make_symmetric(&built.mesh.positions, &built.mesh.indices, None, Axis::X, 1.0, true).unwrap();
        assert!(again.moved.is_empty(), "{}", again.message());

        // Nothing on the side being kept.
        let one_side: Vec<u32> = (0..fixture.mesh.triangle_count())
            .filter(|&t| fixture.mesh.triangle(t).iter().all(|p| p.x < 0.0))
            .flat_map(|t| indices[t * 3..t * 3 + 3].to_vec())
            .collect();
        let error = mirror_and_weld(&drifted, &one_side, None, Axis::X, 1.0, true).unwrap_err();
        assert_eq!(error, "Nothing on the +X side to mirror");
        assert_eq!(mirror_and_weld(&[], &[], None, Axis::Y, 1.0, false).unwrap_err(), "Nothing on the -Y side to mirror");
    }

    /// Cut & Mirror at every setting of the Match Tolerance slider, on spiky models the plane
    /// crosses many times over: always one sound closed surface, exactly symmetric, with no
    /// slivers along the cut, and the same whichever cores did the work.
    #[test]
    fn cut_and_mirror_on_bumpy_models_is_closed_at_every_tolerance() {
        for seed in 0..12 {
            let ball = bumpy_sphere(seed, 30);
            let spacing = map::mean_edge_length(&ball.positions, &ball.indices);
            let mask: Vec<f32> = ball.positions.iter().map(|p| (p.z * 4.0).sin() * 0.5 + 0.5).collect();
            for scale in [MIN_TOLERANCE_SCALE, 1.0, MAX_TOLERANCE_SCALE] {
                let (axis, positive) = (Axis::ALL[seed as usize % 3], seed % 2 == 0);
                let label = format!("seed {seed}, {axis:?}, tolerance x{scale}");
                let built = mirror_and_weld(&ball.positions, &ball.indices, Some(&mask), axis, scale, positive).unwrap();
                assert_eq!(Faults::of(&built.mesh), Faults::NONE, "{label}");
                assert!(built.mesh.signed_volume() > 0.0, "{label}");
                assert_eq!(degenerate_triangles(&built.mesh.positions, &built.mesh.indices, spacing), 0, "{label}");
                let map = build_map(&built.mesh.positions, &built.mesh.indices, axis, 1.0).unwrap();
                assert!(map.is_symmetric(), "{label}: {}", map.summary());
                let mask_after = built.mask.as_deref().unwrap();
                assert_eq!(mask_after.len(), built.mesh.vertex_count(), "{label}");
                assert!(mask_after.iter().all(|m| (0.0..=1.0).contains(m)), "{label}");
            }
        }
        // One large enough to be shared out among the cores.
        let ball = bumpy_sphere(3, 40);
        let mask: Vec<f32> = ball.positions.iter().map(|p| p.y.abs().min(1.0)).collect();
        let run = || mirror_and_weld(&ball.positions, &ball.indices, Some(&mask), Axis::Y, MIN_TOLERANCE_SCALE, false);
        let one_core = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap().install(run).unwrap();
        assert!(one_core.mesh.triangle_count() > 8192 && one_core.mesh.positions.iter().any(|p| p.y == 0.0));
        assert_eq!(Ok(one_core), run());
    }

    /// Cleanup is for welding a seam that is two coincident shells. A mesh with no such seam
    /// must come through untouched: no degenerate faces, no real mirrored pairs straddling the
    /// plane collapsed into each other.
    #[test]
    fn cleanup_on_a_noisy_symmetric_shape_stays_on_the_seam() {
        let fixture = Fixture::new(61);
        let (v, indices) = (&fixture.mesh.positions, &fixture.mesh.indices);
        let cleaned = cleanup(v, indices, None, Axis::X, 1.0).unwrap();
        assert_eq!(cleaned.change, CleanupChange::Nothing, "{}", cleaned.message());
        assert_eq!(cleaned.message(), "Already clean across X - nothing to do");

        // One with a real centreline, knocked a little off the plane.
        let (mut sphere, _) = icosphere(5);
        let spacing = map::mean_edge_length(&sphere.positions, &sphere.indices);
        let mut rng = Lcg::new(5);
        for p in &mut sphere.positions {
            *p += rng.in_cube() * (spacing * 0.03);
        }
        let map = build_map(&sphere.positions, &sphere.indices, Axis::X, 1.0).unwrap();
        let cleaned = cleanup(&sphere.positions, &sphere.indices, None, Axis::X, 1.0).unwrap();
        assert_eq!(cleaned.snapped, map.on_plane_count());
        assert_eq!(cleaned.welded, 0);
        let CleanupChange::Moved(moved) = &cleaned.change else { panic!("{}", cleaned.message()) };
        assert_eq!(moved.len(), map.on_plane_count());
        assert!(moved.positions.iter().all(|p| p.x == 0.0));
        assert_eq!(cleaned.message(), format!("Snapped {} vertices onto X - no duplicates found", cleaned.snapped));
        let after = apply(&sphere.positions, moved);
        assert_eq!(degenerate_triangles(&after, &sphere.indices, spacing), 0);
        assert_eq!(cleanup(&after, &sphere.indices, None, Axis::X, 1.0).unwrap().change, CleanupChange::Nothing);
        assert_eq!(cleanup(&[], &[], None, Axis::X, 1.0).unwrap_err(), "No geometry to clean up");
    }

    /// Two mirrored halves joined without welding, the seam a hair open: cleanup snaps the two
    /// copies of every seam vertex onto the plane and merges them, and the mask comes along.
    #[test]
    fn cleanup_welds_the_seam_of_two_joined_halves() {
        let full = octa_sphere(16, 1.0);
        let mut positions = Vec::new();
        let mut indices = Vec::new();
        for positive in [true, false] {
            let mut slot = vec![u32::MAX; full.vertex_count()];
            for t in 0..full.triangle_count() {
                if (full.triangle(t).iter().map(|p| p.x).sum::<f32>() > 0.0) != positive {
                    continue;
                }
                for &i in &full.indices[t * 3..t * 3 + 3] {
                    if slot[i as usize] == u32::MAX {
                        slot[i as usize] = positions.len() as u32;
                        // Each half's rim sits a hair on its own side of the plane.
                        let on_rim = full.positions[i as usize].x == 0.0;
                        let gap = if !on_rim { 0.0 } else if positive { 2e-5 } else { -2e-5 };
                        positions.push(full.positions[i as usize] + Vec3::X * gap);
                    }
                    indices.push(slot[i as usize]);
                }
            }
        }
        let seam = 4 * 16;
        assert_eq!(positions.len(), full.vertex_count() + seam);
        let mask: Vec<f32> = positions.iter().map(|p| p.z.abs()).collect();

        let cleaned = cleanup(&positions, &indices, Some(&mask), Axis::X, 1.0).unwrap();
        assert_eq!((cleaned.snapped, cleaned.welded), (2 * seam, seam), "{}", cleaned.message());
        assert_eq!(cleaned.message(), format!("Snapped {} onto X, welded {seam} duplicate vertices", 2 * seam));
        let CleanupChange::Rebuilt { mesh, source, mask: carried } = &cleaned.change else {
            panic!("{}", cleaned.message())
        };
        assert_eq!(mesh.vertex_count(), full.vertex_count());
        assert_eq!(mesh.triangle_count(), full.triangle_count());
        assert_eq!(edge_census(mesh), (0, 0));
        assert!((mesh.signed_volume() - full.signed_volume()).abs() < 1e-6);
        assert_eq!(carried.as_deref(), Some(&weld::carry(source, &mask)[..]));
        assert!(build_map(&mesh.positions, &mesh.indices, Axis::X, 1.0).unwrap().is_symmetric());
    }

    /// Run with `--release -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn timing_symmetrize_4m() {
        let mesh = octa_sphere(1000, 1.0);
        let indices = &mesh.indices;
        let spacing = map::mean_edge_length(&mesh.positions, indices);
        let ms = |clock: std::time::Instant| clock.elapsed().as_secs_f64() * 1e3;
        println!("symmetrize: {} vertices, {} triangles", mesh.vertex_count(), mesh.triangle_count());

        let clock = std::time::Instant::now();
        let text = status(&mesh.positions, indices, Axis::X, 1.0);
        println!("  status (build the map) on the symmetric mesh: {:.0} ms - {text}", ms(clock));

        // Every -X vertex knocked out of true. A few hundredths of a spacing is what a long
        // mirrored session leaves, and every pair is still found by distance alone; a quarter of
        // a spacing is past what a seed is trusted at, so most pairs are grown along the surface.
        for (label, amount) in [("0.05", 0.03), ("0.25", 0.15)] {
            let mut jittered = mesh.positions.clone();
            jittered.par_iter_mut().enumerate().for_each(|(i, p)| {
                if p.x < 0.0 {
                    *p += scatter(i as u32) * (spacing * amount);
                }
            });
            let clock = std::time::Instant::now();
            let result = make_symmetric(&jittered, indices, None, Axis::X, 1.0, true).unwrap();
            let (took, moved) = (ms(clock), result.moved.len());
            println!("  make_symmetric, half jittered by up to {label} spacings: {took:.0} ms, {moved} moved");
            let after = apply(&jittered, &result.moved);
            assert_eq!(max_delta(&after, &mesh.positions), 0.0, "the sphere is back, exactly");
        }

        // Heavy drift: the whole -X half pushed out, most of it far past the tolerance, so nearly
        // every pair has to be found by walking the surface out from the centreline.
        let mut drifted = mesh.positions.clone();
        let centre = Vec3::new(-0.6, 0.5, 0.6).normalize();
        drifted.par_iter_mut().for_each(|p| {
            if p.x < 0.0 {
                let bump = (-(*p - centre).length_squared() / (2.0 * 0.3 * 0.3)).exp();
                *p += p.normalize() * (spacing * (0.2 + 6.0 * bump)) * (-p.x * 50.0).min(1.0);
            }
        });
        let clock = std::time::Instant::now();
        let map = build_map(&drifted, indices, Axis::X, 1.0).unwrap();
        let build = ms(clock);
        let clock = std::time::Instant::now();
        let result = make_symmetric(&drifted, indices, None, Axis::X, 1.0, true).unwrap();
        let (took, walked, moved) = (ms(clock), map.propagated_pair_count(), result.moved.len());
        println!("  make_symmetric, half pushed up to 6 spacings out: {took:.0} ms (its map alone {build:.0} ms,");
        println!("    {walked} pairs found along the surface), {moved} moved - {}", result.message());
        assert_eq!(max_delta(&apply(&drifted, &result.moved), &mesh.positions), 0.0, "the sphere is back, exactly");

        // A masked repair of one patch of that model.
        let mask: Vec<f32> = drifted.par_iter().map(|p| if p.distance(centre) < 0.25 { 0.0 } else { 1.0 }).collect();
        let free = mask.iter().filter(|&&m| m < 1.0).count();
        let clock = std::time::Instant::now();
        let masked = make_symmetric(&drifted, indices, Some(&mask), Axis::X, 1.0, true).unwrap();
        println!("  masked make_symmetric, {free} unmasked vertices: {:.0} ms - {}", ms(clock), masked.message());
    }
}
