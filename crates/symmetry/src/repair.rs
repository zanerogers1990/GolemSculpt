//! Repairs that only move vertices: the operations a correspondence map makes
//! possible and that mirroring strokes by position cannot do on its own.
//!
//! Each works in place on a copy of the mesh's positions and says how many
//! vertices it moved. Topology is never touched, so none of them can punch a
//! hole in a model, and the caller can record the outcome as an ordinary sparse
//! undo step (`Moved::between`). `ops` strings them together the way the menu
//! does.
//!
//! A `mask`, where one is taken, has one value per vertex and 1 means
//! protected: a vertex moves `1 - mask` of the way it otherwise would.

use glam::Vec3;
use rayon::prelude::*;

use crate::map::SymmetryMap;
use crate::par;

/// The vertices an operation moved and where they ended up: what the app needs
/// to record the change as a sparse undo step and re-upload the affected part
/// of the mesh.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Moved {
    /// Ascending.
    pub indices: Vec<u32>,
    /// The new position of each, parallel to `indices`.
    pub positions: Vec<Vec3>,
}

impl Moved {
    /// Every vertex whose position differs between the two arrays.
    pub fn between(before: &[Vec3], after: &[Vec3]) -> Moved {
        assert_eq!(before.len(), after.len(), "the two arrays describe different meshes");
        let indices = par::indices_where(before.len(), |i| before[i] != after[i]);
        let positions = indices.par_iter().with_min_len(8192).map(|&i| after[i as usize]).collect();
        Moved { indices, positions }
    }

    pub fn len(&self) -> usize {
        self.indices.len()
    }

    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// Writes the moved positions into `positions`.
    pub fn apply(&self, positions: &mut [Vec3]) {
        for (&i, &p) in self.indices.iter().zip(&self.positions) {
            positions[i as usize] = p;
        }
    }
}

/// How much of its move vertex `i` is free to make.
#[inline]
pub(crate) fn freedom(mask: Option<&[f32]>, i: usize) -> f32 {
    mask.map_or(1.0, |m| 1.0 - m[i].clamp(0.0, 1.0))
}

/// Whether a signed distance from the plane is on the side being copied FROM.
#[inline]
pub(crate) fn on_source_side(side: f32, source_is_positive: bool) -> bool {
    if source_is_positive { side > 0.0 } else { side < 0.0 }
}

/// Copies the source side of the model onto the other side, using the map's
/// pairing so each vertex lands on its OWN counterpart rather than merely
/// somewhere plausible.
///
/// This is the operation positional mirroring cannot express. After a remesh
/// the two sides no longer share a vertex layout, and every later stroke leaves
/// them a little further apart. Given a correspondence, the fix is exact and
/// instant: write each source vertex's reflection into its partner.
///
/// Unmatched vertices are deliberately left untouched rather than approximated
/// HERE: the map found no counterpart for them, so any position written by
/// this pass would be a guess. Leaving them is not the finished answer either
/// (they end up standing proud of a surface that moved out from under them),
/// which is what `carry_unmatched` is for; run it after this.
///
/// With a mask, each destination moves only `1 - mask` of the way to its
/// reflection, so a soft mask edge blends the repaired area into the untouched
/// one. Only the DESTINATION's mask matters: the source is read, never written.
///
/// Returns how many vertices genuinely MOVED, not how many were written:
/// running this twice in a row writes the same values the second time, and
/// reporting them again would leave an undo step that undoes nothing.
pub fn make_symmetric(
    positions: &mut [Vec3],
    map: &SymmetryMap,
    source_is_positive: bool,
    mask: Option<&[f32]>,
) -> usize {
    assert_eq!(map.vertex_count(), positions.len(), "the map was built for a different mesh");
    let axis = map.axis();
    let read = &*positions;
    let changes: Vec<(u32, Vec3)> = par::gather(read.len(), |i, out| {
        // Each pair once, from its lower end.
        let Some(partner) = map.partner_of(i as u32).filter(|&p| p as usize > i) else { return };
        let partner = partner as usize;

        // The member further toward the source side drives, by comparing the
        // two coordinates rather than testing each one's sign, so a pair whose
        // destination end has slid a hair across the plane still repairs from
        // the right end. Only the destination is written, and every vertex is
        // in one pair at most, so nothing read here is written by another pair.
        let (ci, cp) = (axis.coord(read[i]), axis.coord(read[partner]));
        let i_is_source = if source_is_positive { ci > cp } else { ci < cp };
        let (source, destination) = if i_is_source { (i, partner) } else { (partner, i) };

        let mut reflected = axis.reflect(read[source]);
        let free = freedom(mask, destination);
        if free <= 0.0 {
            return;
        }
        if free < 1.0 {
            reflected = read[destination] + (reflected - read[destination]) * free;
        }
        // Exact comparison, not a tolerance: the previous pass stored
        // precisely this value, so an unchanged vertex compares equal, while a
        // source that has since moved at all should be re-mirrored.
        if read[destination].distance_squared(reflected) > 0.0 {
            out.push((destination as u32, reflected));
        }
    });
    for &(destination, p) in &changes {
        positions[destination as usize] = p;
    }
    changes.len()
}

/// Snaps every on-plane vertex exactly onto the mirror plane, zeroing the axis
/// component the tolerance band allowed to drift.
///
/// Worth doing as its own step because "nearly on the centreline" is what
/// actually breaks a symmetric model: a centre vertex sitting a hair off the
/// plane is mirrored onto a partner a hair off the OTHER way, so the seam
/// develops a zig-zag that gets worse each time symmetry is enforced. Pinning
/// them to zero first makes the centreline a fixed point of every later
/// operation.
///
/// With a mask each vertex moves only `1 - mask` of the way onto the plane.
/// Returns how many vertices moved.
pub fn snap_to_plane(positions: &mut [Vec3], map: &SymmetryMap, mask: Option<&[f32]>) -> usize {
    assert_eq!(map.vertex_count(), positions.len(), "the map was built for a different mesh");
    let axis = map.axis().index();
    positions
        .par_iter_mut()
        .with_min_len(8192)
        .enumerate()
        .filter_map(|(i, p)| {
            if !map.is_on_plane(i as u32) || p[axis] == 0.0 {
                return None;
            }
            // What is left of the off-plane distance: none of it unmasked, all
            // of it masked.
            let keep = 1.0 - freedom(mask, i);
            if keep >= 1.0 {
                return None;
            }
            p[axis] *= keep;
            Some(())
        })
        .count()
}

/// Pulls both copies of a DUPLICATED centreline vertex onto the plane: a pair
/// the map matched across it, inside the centreline band, whose two ends sit
/// within `max_separation` of each other. That is what a seam joined from two
/// mirrored halves looks like (one vertex, twice, a hair either side), and a
/// weld can only merge the copies once they are exactly on the plane. A genuine
/// mirrored pair straddling the plane is left alone: its ends are a good
/// fraction of a vertex spacing apart, and pinning it would collapse the edge
/// between them. Returns how many vertices moved.
pub fn snap_seam_duplicates(positions: &mut [Vec3], map: &SymmetryMap, max_separation: f32) -> usize {
    assert_eq!(map.vertex_count(), positions.len(), "the map was built for a different mesh");
    let axis = map.axis();
    let max_sqr = max_separation * max_separation;
    let read = &*positions;
    let pinned = par::indices_where(read.len(), |i| {
        let Some(partner) = map.partner_of(i as u32) else { return false };
        let coord = axis.coord(read[i]);
        coord != 0.0 && coord.abs() <= map.tolerance() && read[i].distance_squared(read[partner as usize]) <= max_sqr
    });
    for &i in &pinned {
        positions[i as usize] = axis.pin(positions[i as usize]);
    }
    pinned.len()
}

/// Carries the vertices `make_symmetric` could not touch along with the ones
/// it moved, so a repair leaves a continuous surface instead of a torn one.
///
/// A vertex with no partner is not a vertex that should stay put; it is a
/// vertex the map has nothing to say about. Leaving it where it was while
/// every vertex around it jumps onto its mirror is what makes a repair read as
/// "nearly aligned": a scatter of points sticking out of the surface by the
/// whole distance their neighbours just moved, exactly where the two sides are
/// not tessellated alike.
///
/// The rule is the one thing that can be said about such a vertex without
/// guessing at geometry: it should move the way its surroundings moved. Each
/// unmatched vertex takes the average of its known neighbours' displacements,
/// which then becomes known itself, so a cluster of them fills inward from its
/// rim. Rounds are batched (every vertex in a round reads only what was known
/// when the round started), so the result does not depend on vertex order.
///
/// The source side is never carried, only used as an anchor: a repair that
/// quietly reshaped the half it was copying FROM would be a different
/// operation than the one the button says it is.
///
/// `before` is the positions as they stood before the repair; `positions` the
/// array being repaired. Needs a map built with triangles. With a mask each
/// carried vertex's shift is scaled by `1 - mask`. Returns how many vertices
/// were carried.
pub fn carry_unmatched(
    positions: &mut [Vec3],
    before: &[Vec3],
    map: &SymmetryMap,
    source_is_positive: bool,
    mask: Option<&[f32]>,
) -> usize {
    assert_eq!(map.vertex_count(), positions.len(), "the map was built for a different mesh");
    assert_eq!(before.len(), positions.len(), "the two arrays describe different meshes");
    if !map.has_topology() {
        return 0;
    }
    let axis = map.axis();

    // Known: anything paired or on the plane, and the whole source side. Its
    // shift is zero wherever the repair did not move it, which is exactly what
    // makes an untouched region hold its unmatched vertices still.
    let mut known: Vec<bool> = (0..positions.len())
        .into_par_iter()
        .with_min_len(8192)
        .map(|i| !map.is_unmatched(i as u32) || on_source_side(axis.coord(before[i]), source_is_positive))
        .collect();
    let mut waiting = par::indices_where(known.len(), |i| !known[i]);

    let mut carried = 0;
    while !waiting.is_empty() {
        let read = &*positions;
        let fills: Vec<(u32, Vec3)> = par::gather(waiting.len(), |k, out| {
            let i = waiting[k];
            let (mut sum, mut count) = (Vec3::ZERO, 0);
            for &nb in map.neighbours(i) {
                if known[nb as usize] {
                    sum += read[nb as usize] - before[nb as usize];
                    count += 1;
                }
            }
            if count > 0 {
                out.push((i, sum / count as f32 * freedom(mask, i as usize)));
            }
        });
        if fills.is_empty() {
            break;
        }
        for &(i, shift) in &fills {
            known[i as usize] = true;
            if shift.length_squared() > 0.0 {
                positions[i as usize] = before[i as usize] + shift;
                carried += 1;
            }
        }
        waiting.retain(|&i| !known[i as usize]);
    }
    carried
}

/// How much of the way to its neighbours' average an unmatched vertex is moved
/// per pass, and how many passes. Deliberately gentle and few: this is seating
/// a scatter of isolated points back into a surface, not smoothing the surface.
const RESEAT_RATE: f32 = 0.5;
const RESEAT_PASSES: usize = 3;

/// Seats the vertices that had no counterpart back into the repaired surface.
///
/// `carry_unmatched` moves them by their neighbours' average SHIFT, the right
/// rule while the repair is happening, but it preserves whatever offset each
/// had from the surface it used to sit in, and after a repair that surface is a
/// mirror of the other half rather than the one those offsets were measured
/// against. Measured on a 134k-triangle repair: the destination half came back
/// at 0.216 mean roughness against the 0.064 of the half it was copied from, a
/// scatter of a few hundred points standing proud of an exactly mirrored
/// surface, which is what reads as the repaired side being jagged.
///
/// An unmatched vertex has no correct position by definition. "Sit smoothly
/// among the vertices that DO have correct positions" is the best answer
/// available.
///
/// Only unmatched vertices off the source side move, so this cannot degrade
/// the mirror itself. Jacobi passes: every vertex reads the previous pass's
/// positions, so the result cannot depend on order. Needs a map built with
/// triangles. A mask leaves fully masked vertices out and slows partly masked
/// ones by `1 - mask`. Returns how many vertices ended up moved from `before`.
pub fn reseat_unmatched(
    positions: &mut [Vec3],
    before: &[Vec3],
    map: &SymmetryMap,
    source_is_positive: bool,
    mask: Option<&[f32]>,
) -> usize {
    assert_eq!(map.vertex_count(), positions.len(), "the map was built for a different mesh");
    assert_eq!(before.len(), positions.len(), "the two arrays describe different meshes");
    if !map.has_topology() {
        return 0;
    }
    let axis = map.axis();

    // Sides are read from `before`: a vertex's ROLE in the repair is a fact
    // about where it started, not about where it ended up.
    let loose = par::indices_where(positions.len(), |i| {
        map.is_unmatched(i as u32)
            && !on_source_side(axis.coord(before[i]), source_is_positive)
            && freedom(mask, i) > 0.0
    });
    if loose.is_empty() {
        return 0;
    }

    for _ in 0..RESEAT_PASSES {
        let read = &*positions;
        let next: Vec<Vec3> = loose
            .par_iter()
            .with_min_len(2048)
            .map(|&i| {
                let around = map.neighbours(i);
                let here = read[i as usize];
                if around.is_empty() {
                    return here;
                }
                let sum: Vec3 = around.iter().map(|&nb| read[nb as usize]).sum();
                here + (sum / around.len() as f32 - here) * (RESEAT_RATE * freedom(mask, i as usize))
            })
            .collect();
        for (&i, &p) in loose.iter().zip(&next) {
            positions[i as usize] = p;
        }
    }
    loose.iter().filter(|&&i| positions[i as usize] != before[i as usize]).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Axis;
    use crate::fixtures::{Lcg, octa_partners, octa_sphere};
    use crate::map::{default_tolerance, mean_edge_length};
    use sculpt_geom::TriMesh;

    fn map_of(mesh: &TriMesh, positions: &[Vec3], axis: Axis) -> SymmetryMap {
        SymmetryMap::build(positions, &mesh.indices, axis, default_tolerance(positions, &mesh.indices))
    }

    fn jitter(positions: &mut [Vec3], amount: f32, seed: u64, mut pick: impl FnMut(Vec3) -> bool) {
        let mut rng = Lcg::new(seed);
        for p in positions {
            let offset = rng.in_cube() * amount;
            if pick(*p) {
                *p += offset;
            }
        }
    }

    /// Jitter one half; the repair puts back the symmetric mesh exactly.
    #[test]
    fn a_jittered_half_is_restored_exactly() {
        let n = 16;
        let mesh = octa_sphere(n, 1.0);
        let spacing = mean_edge_length(&mesh.positions, &mesh.indices);
        let mut positions = mesh.positions.clone();
        jitter(&mut positions, spacing * 0.08, 1, |p| p.x < -1e-3);

        let map = map_of(&mesh, &positions, Axis::X);
        assert!(map.is_symmetric(), "{}", map.summary());
        let moved = make_symmetric(&mut positions, &map, true, None);
        assert_eq!(moved, map.pair_count());
        assert_eq!(positions, mesh.positions, "the untouched +X half was copied over the jittered one");
        assert_eq!(make_symmetric(&mut positions, &map, true, None), 0, "a second pass has nothing to move");
    }

    /// Jitter everything, the centreline included; after the repair every pair is a bitwise
    /// reflection, the centreline is exactly on the plane, and nothing went further than the
    /// jitter itself.
    #[test]
    fn a_jittered_sphere_is_made_symmetric_within_the_jitter() {
        let n = 16;
        let mesh = octa_sphere(n, 1.0);
        let partners = octa_partners(n, Axis::X);
        let spacing = mean_edge_length(&mesh.positions, &mesh.indices);
        let amount = spacing * 0.04;
        for source_is_positive in [true, false] {
            let mut positions = mesh.positions.clone();
            jitter(&mut positions, amount, 2, |_| true);
            let map = map_of(&mesh, &positions, Axis::X);
            assert!(map.is_symmetric(), "{}", map.summary());

            let snapped = snap_to_plane(&mut positions, &map, None);
            assert_eq!(snapped, map.on_plane_count());
            make_symmetric(&mut positions, &map, source_is_positive, None);
            for (i, &j) in partners.iter().enumerate() {
                assert_eq!(positions[j as usize], Axis::X.reflect(positions[i]) + Vec3::ZERO, "vertex {i}");
                assert!(positions[i].distance(mesh.positions[i]) <= amount * 1.7321 + 1e-6);
            }
            // The source half is where the jitter left it.
            let mut jittered = mesh.positions.clone();
            jitter(&mut jittered, amount, 2, |_| true);
            for i in 0..positions.len() {
                if on_source_side(jittered[i].x, source_is_positive) && !map.is_on_plane(i as u32) {
                    assert_eq!(positions[i], jittered[i]);
                }
            }
        }
    }

    #[test]
    fn a_mask_holds_the_destination_back() {
        let mesh = octa_sphere(8, 1.0);
        let mut positions = mesh.positions.clone();
        for p in &mut positions {
            if p.x < -1e-3 {
                *p *= 1.01;
            }
        }
        let start = positions.clone();
        let map = map_of(&mesh, &positions, Axis::X);
        assert!(map.is_symmetric());

        // Fully masked where y > 0, half masked below.
        let mask: Vec<f32> = start.iter().map(|p| if p.y > 0.0 { 1.0 } else { 0.5 }).collect();
        let moved = make_symmetric(&mut positions, &map, true, Some(&mask));
        assert!(moved > 0);
        for (i, p) in positions.iter().enumerate() {
            if start[i].x >= -1e-3 || start[i].y > 0.0 {
                assert_eq!(*p, start[i], "vertex {i} is on the source side or fully masked");
            } else {
                let halfway = (start[i] + mesh.positions[i]) * 0.5;
                assert!(p.distance(halfway) < 1e-6, "vertex {i} should be half way back");
            }
        }
    }

    #[test]
    fn snap_to_plane_pins_the_centreline_and_respects_the_mask() {
        let mesh = octa_sphere(8, 1.0);
        let mut positions = mesh.positions.clone();
        let map = map_of(&mesh, &positions, Axis::Z);
        let on_plane: Vec<usize> = (0..positions.len()).filter(|&i| map.is_on_plane(i as u32)).collect();
        for &i in &on_plane {
            positions[i].z = 0.004;
        }
        let mut mask = vec![0.0; positions.len()];
        mask[on_plane[0]] = 1.0;
        mask[on_plane[1]] = 0.25;

        let snapped = snap_to_plane(&mut positions, &map, Some(&mask));
        assert_eq!(snapped, on_plane.len() - 1);
        assert_eq!(positions[on_plane[0]].z, 0.004);
        assert!((positions[on_plane[1]].z - 0.001).abs() < 1e-9);
        assert!(on_plane[2..].iter().all(|&i| positions[i].z == 0.0));
        assert_eq!(snap_to_plane(&mut positions, &map, None), 2);
        assert_eq!(snap_to_plane(&mut positions, &map, None), 0);
    }

    /// The seam of two joined halves: every centreline vertex twice, a hair either side.
    #[test]
    fn seam_duplicates_are_pinned_but_real_pairs_are_not() {
        let tolerance = 0.01;
        let mut positions = vec![
            Vec3::new(0.0004, 0.0, 0.0),
            Vec3::new(-0.0004, 0.0, 0.0),
            Vec3::new(0.006, 1.0, 0.0),
            Vec3::new(-0.006, 1.0, 0.0),
            Vec3::new(0.5, 2.0, 0.0),
            Vec3::new(-0.5, 2.0, 0.0),
        ];
        let map = SymmetryMap::build(&positions, &[], Axis::X, tolerance);
        assert_eq!(map.pair_count(), 3);
        assert_eq!(snap_seam_duplicates(&mut positions, &map, 0.002), 2);
        assert_eq!(positions[0], Vec3::ZERO);
        assert_eq!(positions[1], Vec3::ZERO);
        assert_eq!(positions[2].x, 0.006, "a straddling pair most of a spacing apart is real geometry");
        assert_eq!(positions[4].x, 0.5);
    }

    /// One side has a vertex the other does not. The repair moves everything around it; the
    /// orphan has to go along instead of being left standing where the surface used to be.
    #[test]
    fn an_unmatched_vertex_is_carried_with_its_surroundings() {
        let n = 10;
        let mut mesh = octa_sphere(n, 1.0);
        let t = (0..mesh.triangle_count())
            .find(|&t| mesh.triangle(t).iter().all(|p| p.x < -0.3 && p.y > 0.3 && p.z > 0.3))
            .unwrap();
        let [a, b, c] = [mesh.indices[t * 3], mesh.indices[t * 3 + 1], mesh.indices[t * 3 + 2]];
        let orphan = mesh.vertex_count() as u32;
        // The orphan stands a little proud of the surface it sits in.
        let centroid = mesh.triangle(t).iter().sum::<Vec3>() / 3.0;
        mesh.positions.push(centroid * 1.08);
        mesh.indices[t * 3 + 2] = orphan;
        mesh.indices.extend([b, c, orphan, c, a, orphan]);

        // The destination (-X) half sits 5% too far out.
        let mut positions = mesh.positions.clone();
        for p in &mut positions {
            if p.x < -1e-3 {
                *p *= 1.05;
            }
        }
        let before = positions.clone();
        let map = map_of(&mesh, &positions, Axis::X);
        assert_eq!(map.unmatched_count(), 1, "{}", map.summary());

        assert!(make_symmetric(&mut positions, &map, true, None) > 0);
        assert_eq!(positions[orphan as usize], before[orphan as usize], "the pair pass cannot move it");
        assert_eq!(carry_unmatched(&mut positions, &before, &map, true, None), 1);
        let around = |positions: &[Vec3]| [a, b, c].iter().map(|&v| positions[v as usize]).sum::<Vec3>() / 3.0;
        let expected = around(&positions) - around(&before);
        assert!((positions[orphan as usize] - before[orphan as usize]).distance(expected) < 1e-6);

        // Carried, it still stands as proud as it did; seating it takes that down by half a pass
        // three times over.
        let proud = positions[orphan as usize].distance(around(&positions));
        assert!((proud - 0.08 * 1.05 * centroid.length()).abs() < 1e-4);
        assert_eq!(reseat_unmatched(&mut positions, &before, &map, true, None), 1);
        assert!((positions[orphan as usize].distance(around(&positions)) - proud / 8.0).abs() < 1e-5);

        // Copying the other way, the orphan is on the SOURCE side: an anchor, never carried.
        let mut other = before.clone();
        make_symmetric(&mut other, &map, false, None);
        assert_eq!(carry_unmatched(&mut other, &before, &map, false, None), 0);
        assert_eq!(reseat_unmatched(&mut other, &before, &map, false, None), 0);
    }

    #[test]
    fn moved_lists_exactly_what_changed() {
        let before = vec![Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::Z];
        let mut after = before.clone();
        after[1] = Vec3::new(1.0, 0.5, 0.0);
        after[3] = Vec3::splat(2.0);
        let moved = Moved::between(&before, &after);
        assert_eq!(moved.indices, [1, 3]);
        assert_eq!(moved.positions, [after[1], after[3]]);
        assert_eq!(moved.len(), 2);
        let mut replay = before.clone();
        moved.apply(&mut replay);
        assert_eq!(replay, after);
        assert!(Moved::between(&before, &before).is_empty());
    }
}
