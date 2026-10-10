//! The few vector helpers glam does not already offer.
//!
//! Not here, because glam has them: `normalize_or_zero` and `normalize_or`
//! (which carry no hidden length cutoff, unlike Unity's `Vector3.normalized`,
//! so a small triangle's raw cross product still gets a normal), `min_element`
//! and `max_element` for the three-way min and max.

use glam::Vec3;

/// `v` at unit length, or `fallback` when its squared length is not above
/// `min_sqr_magnitude` or is not finite.
///
/// glam's own `normalize_or` takes any non-zero finite length, which is what
/// you want for a triangle's raw cross product (its length is twice the area, so
/// any absolute cutoff silently drops the normals of every small triangle on a
/// dense or small mesh). This is for the opposite case, a direction that is
/// meaningless when too short, with the cutoff chosen by the caller.
pub fn normalize_or(v: Vec3, fallback: Vec3, min_sqr_magnitude: f32) -> Vec3 {
    let sqr = v.length_squared();
    // Spelled so a NaN length takes the fallback too; an infinite one would scale to zero.
    if sqr.is_nan() || sqr <= min_sqr_magnitude || sqr.is_infinite() {
        return fallback;
    }
    v * (1.0 / sqr.sqrt())
}

/// How far along the ray `origin + t * dir` crosses the plane through
/// `plane_point` with normal `plane_normal`, if it does so in front of the
/// origin and is not (nearly) parallel to the plane. `t` is in units of `dir`.
pub fn ray_plane_distance(origin: Vec3, dir: Vec3, plane_point: Vec3, plane_normal: Vec3) -> Option<f32> {
    let denom = dir.dot(plane_normal);
    if denom.abs() < 1e-6 {
        return None;
    }
    let t = (plane_point - origin).dot(plane_normal) / denom;
    (t >= 0.0).then_some(t)
}

/// Where a ray crosses a plane, if it does so in front of its origin. Shared by
/// every drag that moves something over a plane: the move, pose and snake-hook
/// drags and the gizmo's axis-constrained handles.
pub fn ray_plane_intersect(origin: Vec3, dir: Vec3, plane_point: Vec3, plane_normal: Vec3) -> Option<Vec3> {
    ray_plane_distance(origin, dir, plane_point, plane_normal).map(|t| origin + dir * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_or_keeps_small_vectors_unless_told_otherwise() {
        let tiny = Vec3::new(3e-6, 0.0, 4e-6);
        assert!((normalize_or(tiny, Vec3::Y, 0.0) - Vec3::new(0.6, 0.0, 0.8)).length() < 1e-5);
        assert_eq!(normalize_or(tiny, Vec3::Y, 1e-9), Vec3::Y);
        assert_eq!(normalize_or(Vec3::ZERO, Vec3::Y, 0.0), Vec3::Y);
        assert_eq!(normalize_or(Vec3::new(f32::NAN, 0.0, 0.0), Vec3::Y, 0.0), Vec3::Y);
        assert_eq!(normalize_or(Vec3::new(f32::INFINITY, 0.0, 0.0), Vec3::Y, 0.0), Vec3::Y);
        assert!((normalize_or(Vec3::new(0.0, -7.0, 0.0), Vec3::X, 0.0) + Vec3::Y).length() < 1e-6);
    }

    #[test]
    fn ray_meets_a_plane_only_in_front_and_not_edge_on() {
        let (point, normal) = (Vec3::new(0.0, 2.0, 0.0), Vec3::Y);
        let hit = ray_plane_intersect(Vec3::new(1.0, 0.0, 1.0), Vec3::new(0.0, 4.0, 0.0), point, normal);
        assert_eq!(hit, Some(Vec3::new(1.0, 2.0, 1.0)));
        assert_eq!(ray_plane_distance(Vec3::new(1.0, 0.0, 1.0), Vec3::new(0.0, 4.0, 0.0), point, normal), Some(0.5));
        assert_eq!(ray_plane_intersect(Vec3::new(0.0, 3.0, 0.0), Vec3::Y, point, normal), None, "behind the origin");
        assert_eq!(ray_plane_intersect(Vec3::ZERO, Vec3::X, point, normal), None, "parallel");
        // From the far side of the plane the other way round.
        assert_eq!(ray_plane_intersect(Vec3::new(0.0, 5.0, 0.0), -Vec3::Y, point, normal), Some(point));
    }
}
