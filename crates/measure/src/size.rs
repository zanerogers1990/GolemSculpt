//! An object's real printed size: its exact extent in the world, and the
//! scale that gives it a typed dimension.
//!
//! Sizing is a change of the object's scale, the same thing the scale gizmo
//! writes, so it is instant at any density, costs the sculpt nothing, and is
//! one undo step.

use glam::{Mat3A, Mat4, Vec3, Vec3A};
use sculpt_geom::Aabb;
use sculpt_toolkit::Placement;

/// Below this many vertices one thread is quicker than starting several.
const PARALLEL_FROM: usize = 200_000;

/// The exact world-space box of `positions` placed by `matrix`: what a slicer
/// reports for the exported part.
///
/// Not the box of the mesh's own box, which for a turned object is loose by
/// up to 40%. A measurement has to be the real extent, so every vertex is
/// moved; large meshes are split across threads.
pub fn exact_bounds(positions: &[Vec3], matrix: &Mat4) -> Aabb {
    let linear = Mat3A::from_mat4(*matrix);
    let offset = Vec3A::from(matrix.w_axis.truncate());
    let fold = |points: &[Vec3]| {
        let (mut min, mut max) = (Vec3A::splat(f32::INFINITY), Vec3A::splat(f32::NEG_INFINITY));
        for &p in points {
            let q = linear * Vec3A::from(p) + offset;
            min = min.min(q);
            max = max.max(q);
        }
        (min, max)
    };
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).clamp(1, 16);
    let (min, max) = if positions.len() < PARALLEL_FROM || threads < 2 {
        fold(positions)
    } else {
        let chunk = positions.len().div_ceil(threads);
        std::thread::scope(|scope| {
            let parts: Vec<_> = positions.chunks(chunk).map(|part| scope.spawn(move || fold(part))).collect();
            parts.into_iter().filter_map(|part| part.join().ok()).fold(
                (Vec3A::splat(f32::INFINITY), Vec3A::splat(f32::NEG_INFINITY)),
                |(min, max), (lo, hi)| (min.min(lo), max.max(hi)),
            )
        })
    };
    Aabb::new(min.into(), max.into())
}

/// The object's own axis (0-2) most nearly parallel to world `axis`.
fn closest_local_axis(placement: &Placement, world_axis: usize) -> usize {
    let mut world = Vec3::ZERO;
    world[world_axis] = 1.0;
    let local = (placement.rotation.inverse() * world).abs();
    if local.x >= local.y && local.x >= local.z {
        0
    } else if local.y >= local.z {
        1
    } else {
        2
    }
}

/// The scale that makes an object measure `target` world units along world
/// `axis` (0 = X width, 1 = Y height, 2 = Z depth).
///
/// `uniform` scales all three of the object's axes together, keeping its
/// proportions; otherwise only the object's own axis that lies closest to
/// `axis` is stretched. The object scales about its own origin, as with the
/// gizmo. `Err` says why there is nothing to size.
pub fn resized_scale(
    positions: &[Vec3],
    placement: Placement,
    axis: usize,
    target: f32,
    uniform: bool,
) -> Result<Vec3, String> {
    if !(target > 1e-6) {
        return Err("The size has to be more than zero.".into());
    }
    if positions.is_empty() {
        return Err("The object has no geometry.".into());
    }
    let extent = |scale: Vec3| exact_bounds(positions, &Placement { scale, ..placement }.matrix()).size()[axis];
    let mut current = extent(placement.scale);
    if current < 1e-7 {
        return Err("The object is flat along that axis - size it along another one.".into());
    }

    let mut after = placement.scale;
    if uniform {
        after *= target / current;
    } else {
        let local = closest_local_axis(&placement, axis);
        // Exact in one step for an object that is not turned. A turned one
        // spreads each of its own axes over several world ones, so the extent
        // is not proportional to the one scale being changed, and a few
        // corrective passes close the gap. The extent grows steadily with
        // that scale, in straight stretches, so once two readings are in
        // hand the line through them lands on (or right beside) the answer;
        // scaling by the ratio is the fallback and the first step.
        let mut previous: Option<(f32, f32)> = None;
        for _ in 0..8 {
            let here = (after[local], current);
            let ratio = here.0 * (target / current);
            let secant = previous
                .filter(|&(_, extent)| (current - extent).abs() > 1e-9)
                .map(|(scale, extent)| here.0 + (target - current) * (here.0 - scale) / (current - extent));
            after[local] = secant.filter(|s| s.is_finite() && s * here.0 > 0.0).unwrap_or(ratio);
            previous = Some(here);
            current = extent(after);
            if current < 1e-7 || (current - target).abs() <= target * 1e-5 {
                break;
            }
        }
    }
    let usable = after.is_finite() && after.abs().min_element() > 1e-9;
    if usable { Ok(after) } else { Err("That size is out of range.".into()) }
}

#[cfg(test)]
mod tests {
    use glam::Quat;
    use sculpt_toolkit::testing::cube;

    use super::*;

    fn turned() -> Placement {
        Placement {
            position: Vec3::new(1.0, 2.0, 3.0),
            rotation: Quat::from_rotation_z(0.6) * Quat::from_rotation_x(0.3),
            scale: Vec3::new(1.0, 2.0, 0.5),
        }
    }

    #[test]
    fn the_exact_box_of_a_turned_cube_is_tighter_than_the_box_of_its_box() {
        let mesh = cube(0.5);
        let placement = Placement { rotation: Quat::from_rotation_y(std::f32::consts::FRAC_PI_4), ..Placement::IDENTITY };
        let size = exact_bounds(&mesh.positions, &placement.matrix()).size();
        // A unit cube turned 45 degrees about Y is root-two wide and deep.
        assert!((size.x - 2f32.sqrt()).abs() < 1e-5 && (size.z - 2f32.sqrt()).abs() < 1e-5);
        assert!((size.y - 1.0).abs() < 1e-6);
    }

    #[test]
    fn many_threads_give_the_same_box_as_one() {
        // Enough points to take the threaded path, in no particular order.
        let positions: Vec<Vec3> = (0..PARALLEL_FROM * 2 + 7)
            .map(|i| {
                let t = i as f32 * 0.37;
                Vec3::new(t.sin() * 3.0, (t * 1.3).cos() * 2.0, (t * 0.7).sin() * (i % 11) as f32)
            })
            .collect();
        let matrix = turned().matrix();
        let got = exact_bounds(&positions, &matrix);
        let want = positions.iter().fold(Aabb::EMPTY, |mut b, p| {
            b.grow(matrix.transform_point3(*p));
            b
        });
        assert!(got.min.distance(want.min) < 1e-4 && got.max.distance(want.max) < 1e-4);
    }

    #[test]
    fn keeping_proportions_scales_every_axis_alike() {
        let mesh = cube(0.5);
        let placement = Placement { scale: Vec3::new(1.0, 2.0, 4.0), ..Placement::IDENTITY };
        let scale = resized_scale(&mesh.positions, placement, 1, 5.0, true).unwrap();
        assert!(scale.distance(Vec3::new(2.5, 5.0, 10.0)) < 1e-5);
    }

    #[test]
    fn one_axis_of_an_unturned_object_is_exact_in_one_step() {
        let mesh = cube(0.5);
        let scale = resized_scale(&mesh.positions, Placement::IDENTITY, 0, 3.0, false).unwrap();
        assert!(scale.distance(Vec3::new(3.0, 1.0, 1.0)) < 1e-6);
    }

    #[test]
    fn one_axis_of_a_turned_object_converges_on_the_typed_size() {
        let mesh = cube(0.5);
        let placement = turned();
        for axis in 0..3 {
            let scale = resized_scale(&mesh.positions, placement, axis, 4.0, false).unwrap();
            let size = exact_bounds(&mesh.positions, &Placement { scale, ..placement }.matrix()).size();
            assert!((size[axis] - 4.0).abs() < 4.0 * 1e-4, "axis {axis}: {}", size[axis]);
            // Only one of the object's own axes was touched.
            let changed = (0..3).filter(|&i| (scale[i] - placement.scale[i]).abs() > 1e-6).count();
            assert_eq!(changed, 1);
        }
    }

    #[test]
    fn nothing_to_size_is_said_in_words() {
        let mesh = cube(0.5);
        let flat = Placement { scale: Vec3::new(1.0, 0.0, 1.0), ..Placement::IDENTITY };
        assert!(resized_scale(&mesh.positions, Placement::IDENTITY, 0, 0.0, true).unwrap_err().contains("more than zero"));
        assert!(resized_scale(&mesh.positions, Placement::IDENTITY, 0, f32::NAN, true).is_err());
        assert!(resized_scale(&[], Placement::IDENTITY, 0, 1.0, true).unwrap_err().contains("no geometry"));
        assert!(resized_scale(&mesh.positions, flat, 1, 1.0, true).unwrap_err().contains("flat"));
        assert!(resized_scale(&mesh.positions, Placement::IDENTITY, 0, f32::INFINITY, true).unwrap_err().contains("range"));
    }
}
