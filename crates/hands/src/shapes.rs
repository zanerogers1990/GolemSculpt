//! The pieces a hand or a foot is built from, each a signed distance (negative inside): round cones for
//! bones with flesh round them, ellipsoids for pads and muscle, rounded boxes for the flat of a palm.
//!
//! Distances only have to be right near the surface, where extraction reads them, and to have the right
//! sign everywhere else. The ellipsoid is the usual bound rather than the exact distance, which is all
//! that needs.

use glam::{Mat3, Vec3};
use sculpt_geom::Aabb;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    /// The hull of a sphere of radius `ra` at `a` and one of radius `rb` at `b`.
    Cone { a: Vec3, b: Vec3, ra: f32, rb: f32 },
    /// `axes` holds the ellipsoid's own x, y and z as columns, orthonormal.
    Ellipsoid { center: Vec3, axes: Mat3, radii: Vec3 },
    /// A box of outer half size `half` whose edges and corners are rounded to `round`.
    Box { center: Vec3, axes: Mat3, half: Vec3, round: f32 },
}

fn sign(v: f32) -> f32 {
    if v > 0.0 {
        1.0
    } else if v < 0.0 {
        -1.0
    } else {
        0.0
    }
}

impl Shape {
    pub fn sphere(center: Vec3, radius: f32) -> Shape {
        Shape::Cone { a: center, b: center, ra: radius, rb: radius }
    }

    pub fn cone(a: Vec3, b: Vec3, ra: f32, rb: f32) -> Shape {
        Shape::Cone { a, b, ra, rb }
    }

    pub fn ellipsoid(center: Vec3, radii: Vec3) -> Shape {
        Shape::Ellipsoid { center, axes: Mat3::IDENTITY, radii }
    }

    pub fn distance(&self, p: Vec3) -> f32 {
        match *self {
            Shape::Cone { a, b, ra, rb } => round_cone(p, a, b, ra, rb),
            Shape::Ellipsoid { center, axes, radii } => ellipsoid(axes.transpose() * (p - center), radii),
            Shape::Box { center, axes, half, round } => {
                let q = (axes.transpose() * (p - center)).abs() - (half - Vec3::splat(round)).max(Vec3::ZERO);
                q.max(Vec3::ZERO).length() + q.max_element().min(0.0) - round
            }
        }
    }

    pub fn bounds(&self) -> Aabb {
        match *self {
            Shape::Cone { a, b, ra, rb } => {
                Aabb::union(Aabb::new(a - Vec3::splat(ra), a + Vec3::splat(ra)), Aabb::new(b - Vec3::splat(rb), b + Vec3::splat(rb)))
            }
            Shape::Ellipsoid { center, radii, .. } => {
                let r = Vec3::splat(radii.max_element());
                Aabb::new(center - r, center + r)
            }
            Shape::Box { center, half, .. } => {
                let r = Vec3::splat(half.length());
                Aabb::new(center - r, center + r)
            }
        }
    }

    /// The same shape reflected across the plane x = 0: a left hand from a right one.
    pub fn mirrored(&self) -> Shape {
        let flip = |v: Vec3| Vec3::new(-v.x, v.y, v.z);
        let flip_axes = |m: Mat3| Mat3::from_cols(flip(m.x_axis), flip(m.y_axis), flip(m.z_axis));
        match *self {
            Shape::Cone { a, b, ra, rb } => Shape::Cone { a: flip(a), b: flip(b), ra, rb },
            Shape::Ellipsoid { center, axes, radii } => Shape::Ellipsoid { center: flip(center), axes: flip_axes(axes), radii },
            Shape::Box { center, axes, half, round } => Shape::Box { center: flip(center), axes: flip_axes(axes), half, round },
        }
    }
}

/// Exact distance to a round cone (after Inigo Quilez). Where one sphere swallows the other the hull is
/// that sphere, and the formula has no answer, so the larger sphere stands in.
fn round_cone(p: Vec3, a: Vec3, b: Vec3, ra: f32, rb: f32) -> f32 {
    let ba = b - a;
    let l2 = ba.length_squared();
    let rr = ra - rb;
    let a2 = l2 - rr * rr;
    if a2 <= 1e-12 {
        return if ra >= rb { p.distance(a) - ra } else { p.distance(b) - rb };
    }
    let il2 = 1.0 / l2;
    let pa = p - a;
    let y = pa.dot(ba);
    let z = y - l2;
    let x2 = (pa * l2 - ba * y).length_squared();
    let y2 = y * y * l2;
    let z2 = z * z * l2;
    let k = sign(rr) * rr * rr * x2;
    if sign(z) * a2 * z2 > k {
        return (x2 + z2).sqrt() * il2 - rb;
    }
    if sign(y) * a2 * y2 < k {
        return (x2 + y2).sqrt() * il2 - ra;
    }
    ((x2 * a2 * il2).sqrt() + y * rr) * il2 - ra
}

/// A bound on the distance to an ellipsoid centred at the origin, exact on its surface.
fn ellipsoid(p: Vec3, radii: Vec3) -> f32 {
    let k0 = (p / radii).length();
    let k1 = (p / (radii * radii)).length();
    if k1 < 1e-12 {
        return -radii.min_element();
    }
    k0 * (k0 - 1.0) / k1
}

/// Smooth union: the lower of two distances, with a fillet of size `k` where they are close.
#[inline]
pub fn smooth_min(a: f32, b: f32, k: f32) -> f32 {
    if k <= 0.0 {
        return a.min(b);
    }
    let h = (k - (a - b).abs()).max(0.0) / k;
    a.min(b) - h * h * k * 0.25
}

/// Smooth intersection: the higher of two distances, rounded over a band of size `k`.
#[inline]
pub fn smooth_max(a: f32, b: f32, k: f32) -> f32 {
    -smooth_min(-a, -b, k)
}

/// How far a point is from a box: zero inside. A lower bound on the distance to anything in the box.
#[inline]
pub fn box_distance(bounds: &Aabb, p: Vec3) -> f32 {
    (p - p.clamp(bounds.min, bounds.max)).length()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_round_cone_is_its_two_spheres_at_the_ends_and_their_hull_between() {
        let cone = Shape::cone(Vec3::ZERO, Vec3::Y, 0.3, 0.1);
        assert!((cone.distance(Vec3::new(0.0, -1.0, 0.0)) - 0.7).abs() < 1e-5);
        assert!((cone.distance(Vec3::new(0.0, 2.0, 0.0)) - 0.9).abs() < 1e-5);
        assert!(cone.distance(Vec3::new(0.0, 0.5, 0.0)) < 0.0);
        // A sphere written as a cone of no length.
        let ball = Shape::sphere(Vec3::X, 0.5);
        assert!((ball.distance(Vec3::new(2.0, 0.0, 0.0)) - 0.5).abs() < 1e-5);
    }

    #[test]
    fn an_ellipsoid_and_a_rounded_box_are_zero_on_their_surfaces() {
        let e = Shape::ellipsoid(Vec3::ZERO, Vec3::new(0.5, 0.2, 0.1));
        assert!(e.distance(Vec3::new(0.5, 0.0, 0.0)).abs() < 1e-5);
        assert!(e.distance(Vec3::new(0.0, 0.2, 0.0)).abs() < 1e-5);
        assert!(e.distance(Vec3::ZERO) < 0.0);
        let b = Shape::Box { center: Vec3::ZERO, axes: Mat3::IDENTITY, half: Vec3::new(0.5, 0.3, 0.2), round: 0.05 };
        assert!(b.distance(Vec3::new(0.5, 0.0, 0.0)).abs() < 1e-5);
        assert!(b.distance(Vec3::new(0.0, 0.0, 0.3)) > 0.09);
    }

    #[test]
    fn mirroring_reflects_every_shape_across_x() {
        let shapes = [
            Shape::cone(Vec3::new(0.2, 0.1, 0.0), Vec3::new(0.5, 0.4, 0.1), 0.1, 0.05),
            Shape::Ellipsoid {
                center: Vec3::new(0.3, 0.0, 0.0),
                axes: Mat3::from_rotation_z(0.4),
                radii: Vec3::new(0.3, 0.1, 0.2),
            },
            Shape::Box { center: Vec3::X, axes: Mat3::from_rotation_y(0.7), half: Vec3::splat(0.2), round: 0.05 },
        ];
        for shape in shapes {
            let mirror = shape.mirrored();
            for p in [Vec3::new(0.3, 0.2, 0.1), Vec3::new(0.9, -0.1, 0.2), Vec3::new(0.1, 0.4, -0.3)] {
                let q = Vec3::new(-p.x, p.y, p.z);
                assert!((shape.distance(p) - mirror.distance(q)).abs() < 1e-5);
            }
        }
    }

    #[test]
    fn a_smooth_union_only_differs_from_min_where_the_two_are_close() {
        assert_eq!(smooth_min(0.0, 1.0, 0.2), 0.0);
        assert!(smooth_min(0.1, 0.1, 0.2) < 0.1);
        assert_eq!(smooth_min(0.3, 0.1, 0.0), 0.1);
        assert!(smooth_max(0.1, 0.1, 0.2) > 0.1);
    }
}
