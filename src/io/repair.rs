//! Points that are not numbers: finding them and putting them somewhere sensible.
//!
//! A position that is NaN or infinite spreads: the first smooth or normal pass
//! that touches it hands the damage to every neighbour. So the app never lets
//! one into a mesh it builds from a file, and never writes one to a file. Both
//! ways mend a point the same way, so a file written from a damaged model and a
//! damaged file from elsewhere come back the same:
//!
//! * a point goes to the average of the corners that share a triangle with it,
//!   counting only those that are themselves in order;
//! * a point whose neighbours are all damaged too is mended from the ones just
//!   mended, a few rounds over, so a damaged patch closes from its edge inwards;
//! * what is still out of reach (a mesh with no good points near it) goes to the
//!   middle of the good points, or to the origin if there are none.
//!
//! A mask value that is not a number becomes 0 (unmasked), as a damaged mask is
//! in a Unity sculpt.

use glam::{DVec3, Vec3};
use rayon::prelude::*;

use crate::mesh::Mesh;

/// How many times a patch of damaged points is closed in from its edge before
/// the rest are sent to the middle. A few hundred points in a row is already
/// far beyond what a stray NaN from a brush looks like.
const ROUNDS: usize = 8;

/// Below this many points the scan runs on the calling thread.
const PAR_MIN: usize = 1 << 14;

fn any_loose(positions: &[Vec3], mask: &[f32]) -> bool {
    let bad = |p: &Vec3| !p.is_finite();
    let bad_mask = |m: &f32| !m.is_finite();
    if positions.len() < PAR_MIN {
        positions.iter().any(bad) || mask.iter().any(bad_mask)
    } else {
        positions.par_iter().with_min_len(PAR_MIN).any(bad) || mask.par_iter().with_min_len(PAR_MIN).any(bad_mask)
    }
}

/// Puts every point of a mesh that is not a number in order, in place, and
/// returns how many points that was. `indices` are the triangles that say who
/// the neighbours are (a corner that names no point is passed over); `mask` is
/// empty or has a value per point.
pub fn repair_points(positions: &mut [Vec3], indices: &[u32], mask: &mut [f32]) -> usize {
    if !any_loose(positions, mask) {
        return 0;
    }
    let mut damaged: Vec<bool> = positions.iter().map(|p| !p.is_finite()).collect();
    for (m, d) in mask.iter_mut().zip(damaged.iter_mut()) {
        if !m.is_finite() {
            *m = 0.0;
            *d = true;
        }
    }
    let count = damaged.iter().filter(|&&d| d).count();
    let mut open: Vec<bool> = positions.iter().map(|p| !p.is_finite()).collect();
    if open.contains(&true) {
        for _ in 0..ROUNDS {
            if !close_in(positions, indices, &mut open) {
                break;
            }
        }
        let centre = middle(positions, &open);
        for (p, o) in positions.iter_mut().zip(&open) {
            if *o {
                *p = centre;
            }
        }
    }
    count
}

/// One round: each point still `open` that shares a triangle with a good
/// point moves to the average of those, and stops being open. The round reads
/// only points that were good when it began, so its answer does not depend on
/// the order of the triangles. Returns whether any point was mended.
fn close_in(positions: &mut [Vec3], indices: &[u32], open: &mut [bool]) -> bool {
    let n = positions.len();
    let mut sum = vec![DVec3::ZERO; n];
    let mut seen = vec![0u32; n];
    for t in indices.chunks_exact(3) {
        for k in 0..3 {
            let (a, b, c) = (t[k] as usize, t[(k + 1) % 3] as usize, t[(k + 2) % 3] as usize);
            if a >= n || !open[a] {
                continue;
            }
            for other in [b, c] {
                if other < n && !open[other] {
                    sum[a] += positions[other].as_dvec3();
                    seen[a] += 1;
                }
            }
        }
    }
    let mut mended = false;
    for v in 0..n {
        if open[v] && seen[v] > 0 {
            positions[v] = (sum[v] / seen[v] as f64).as_vec3();
            mended = true;
        }
    }
    // Only now: a point mended this round is not read until the next.
    for v in 0..n {
        if open[v] && seen[v] > 0 {
            open[v] = false;
        }
    }
    mended
}

/// The middle of the points that are in order; the origin if none are.
fn middle(positions: &[Vec3], open: &[bool]) -> Vec3 {
    let (mut sum, mut count) = (DVec3::ZERO, 0usize);
    for (p, &o) in positions.iter().zip(open) {
        if !o {
            sum += p.as_dvec3();
            count += 1;
        }
    }
    if count == 0 { Vec3::ZERO } else { (sum / count as f64).as_vec3() }
}

/// How many points of `mesh` are damaged: the number [`repair_points`] would
/// report for it.
pub fn damaged_points(mesh: &Mesh) -> usize {
    if !any_loose(&mesh.positions, &mesh.mask) {
        return 0;
    }
    let bad = |v: usize| !mesh.positions[v].is_finite() || mesh.mask.get(v).is_some_and(|m| !m.is_finite());
    (0..mesh.positions.len()).filter(|&v| bad(v)).count()
}

/// The positions and mask a mesh with damaged points should be written with,
/// or `None` if it has none. The mesh itself is not touched.
pub fn mended(mesh: &Mesh) -> Option<(Vec<Vec3>, Vec<f32>)> {
    if !any_loose(&mesh.positions, &mesh.mask) {
        return None;
    }
    let (mut positions, mut mask) = (mesh.positions.clone(), mesh.mask.clone());
    repair_points(&mut positions, &mesh.indices, &mut mask);
    Some((positions, mask))
}

/// What the status line says after a file with damaged points was opened or
/// saved, in plain words; `None` when there were none.
pub fn note_opened(count: usize) -> Option<String> {
    (count > 0).then(|| {
        let points = if count == 1 { "1 damaged point".to_owned() } else { format!("{count} damaged points") };
        format!("{points} in the file had no position and {} moved to where the neighbours are.", verb(count))
    })
}

/// [`note_opened`] for a save, which repairs the copy it writes and not the model.
pub fn note_saved(count: usize) -> Option<String> {
    (count > 0).then(|| {
        let points =
            if count == 1 { "1 point of the model has".to_owned() } else { format!("{count} points of the model have") };
        format!(
            "{points} no position (damage from an earlier edit), so the file holds {} where the neighbours are. \
             The model on screen is unchanged.",
            if count == 1 { "it" } else { "them" }
        )
    })
}

fn verb(count: usize) -> &'static str {
    if count == 1 { "was" } else { "were" }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat grid of `side` x `side` points in the XZ plane, two triangles a cell.
    fn grid(side: u32) -> (Vec<Vec3>, Vec<u32>) {
        let positions = (0..side * side).map(|i| Vec3::new((i % side) as f32, 0.0, (i / side) as f32)).collect();
        let mut indices = Vec::new();
        for z in 0..side - 1 {
            for x in 0..side - 1 {
                let a = z * side + x;
                indices.extend_from_slice(&[a, a + side, a + 1, a + 1, a + side, a + side + 1]);
            }
        }
        (positions, indices)
    }

    #[test]
    fn a_point_that_is_not_a_number_goes_between_its_neighbours() {
        let (mut positions, indices) = grid(5);
        let middle = 2 * 5 + 2;
        positions[middle] = Vec3::NAN;
        let repaired = repair_points(&mut positions, &indices, &mut []);
        assert_eq!(repaired, 1);
        assert!(positions[middle].is_finite());
        assert!(positions[middle].distance(Vec3::new(2.0, 0.0, 2.0)) < 0.5, "{:?}", positions[middle]);
        // Nothing else moved.
        let (clean, _) = grid(5);
        for (i, (a, b)) in positions.iter().zip(&clean).enumerate() {
            assert!(i == middle || a == b);
        }
    }

    #[test]
    fn an_infinity_and_a_patch_of_damage_close_in_from_the_edge() {
        let (mut positions, indices) = grid(7);
        for (z, x) in [(3, 3), (3, 4), (4, 3), (4, 4)] {
            positions[z * 7 + x] = Vec3::new(f32::INFINITY, 1.0, f32::NEG_INFINITY);
        }
        let repaired = repair_points(&mut positions, &indices, &mut []);
        assert_eq!(repaired, 4);
        assert!(positions.iter().all(|p| p.is_finite()));
        for (z, x) in [(3, 3), (3, 4), (4, 3), (4, 4)] {
            let want = Vec3::new(x as f32, 0.0, z as f32);
            assert!(positions[z * 7 + x].distance(want) < 1.5, "{:?} for {want:?}", positions[z * 7 + x]);
        }
    }

    #[test]
    fn points_with_no_good_neighbour_go_to_the_middle_of_the_rest() {
        // Two triangles apart: the second has only damaged corners.
        let mut positions = vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(0.0, 2.0, 0.0),
            Vec3::NAN,
            Vec3::NAN,
            Vec3::NAN,
        ];
        let indices = [0, 1, 2, 3, 4, 5];
        let repaired = repair_points(&mut positions, &indices, &mut []);
        assert_eq!(repaired, 3);
        let centre = Vec3::new(2.0 / 3.0, 2.0 / 3.0, 0.0);
        for p in &positions[3..] {
            assert!(p.distance(centre) < 1e-5, "{p:?}");
        }
        // And with nothing good anywhere: the origin.
        let mut all = vec![Vec3::NAN; 3];
        assert_eq!(repair_points(&mut all, &[0, 1, 2], &mut []), 3);
        assert_eq!(all, vec![Vec3::ZERO; 3]);
    }

    #[test]
    fn a_mask_that_is_not_a_number_is_cleared_and_counted() {
        let (mut positions, indices) = grid(4);
        let mut mask = vec![0.5f32; positions.len()];
        mask[3] = f32::NAN;
        positions[6] = Vec3::NAN;
        mask[6] = f32::NAN;
        let repaired = repair_points(&mut positions, &indices, &mut mask);
        assert_eq!(repaired, 2, "a point damaged twice is one point");
        assert_eq!((mask[3], mask[6], mask[0]), (0.0, 0.0, 0.5));
        assert!(positions[6].is_finite());
    }

    #[test]
    fn a_sound_mesh_is_left_alone() {
        let (mut positions, indices) = grid(5);
        let before = positions.clone();
        assert_eq!(repair_points(&mut positions, &indices, &mut vec![0.25; 25]), 0);
        assert_eq!(positions, before);
        assert!(note_opened(0).is_none() && note_saved(0).is_none());
        assert!(note_opened(1).unwrap().contains("1 damaged point in the file"));
        assert!(note_opened(3).unwrap().contains("3 damaged points"));
    }
}
