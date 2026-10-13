//! How a surface turned at one of its points between two shapes, read off
//! the faces round the point: what carries a vector that belongs to the
//! surface (sculpted detail, a layer's offset) along when the surface under
//! it is bent.
//!
//! The turn is in two parts, both from the point's own fan of faces:
//!
//! 1. Its normal, area weighted over the fan. The least rotation that takes
//!    the old normal to the new one leans the vector over with the surface.
//! 2. Its spin about that normal: the one angle that best lines the old fan's
//!    edges, leaned over, up with the new fan's (the least-squares rotation
//!    of one set of in-plane vectors onto another, which needs only two sums).
//!
//! The spin is why this is not written as "a normal and one tangent edge",
//! the usual tangent frame. One edge would do for a rigid move, where every
//! edge turns alike. But under a stretch or a shear each edge turns by its own
//! amount, and a vertex and its mirror twin, whose edges are listed in
//! different orders, would pick differently and turn their vectors
//! differently: a symmetric model edited symmetrically would come out
//! lopsided. Every edge of the fan counted alike has no first edge to pick.
//! It is still a frame tied to the mesh and not to the world: turn the whole
//! model and every vector turns with it exactly.
//!
//! A turn is a rotation and nothing else. What it carries keeps its length:
//! scale the surface up and a pore stays a pore on a larger head.
//!
//! (The other thing that can be read off a fan is everything that happened
//! to it, stretch and shear as well as turn: the matrix that best takes the
//! old edges to the new. That would be the right carrier for a vector an
//! edit moved both ends of, such as a sculpt layer's offset under a brush
//! stroke on the model beneath it, and it was tried twice. A fan lies nearly
//! in a plane, so what the fit says about a vector standing out of it comes
//! from the little the fan is not flat by, and the bend of an edit across one
//! edge is as large as that. Trusted freely, it carried a fine layer a third
//! wrong under a twist the turn carries to a tenth. Trusted only where the
//! fan is rougher than its edges are long, it left the same faint print of a
//! layer of deep creases under a hard lean as the turn does, and Reset Shape,
//! which can only turn a layer back, no longer brought it home.)

use glam::{DMat3, DVec3};

/// The faces round one point of a surface, before and after.
pub trait Fan {
    /// Calls `wedge(was, now)` once for every corner of a face at the point:
    /// the two edges of that corner as vectors from the point, wound as the
    /// face is, as they were and as they are now. It is called twice for one
    /// turn and must give the same corners both times.
    fn wedges(&self, wedge: impl FnMut([DVec3; 2], [DVec3; 2]));
}

/// How the surface turned at a point: see the module notes.
#[derive(Clone, Copy, Debug)]
pub struct Turn {
    /// `None` where there is no turn to speak of: the fan has no area to
    /// take a normal from (a stray point, a collapsed face).
    lean: Option<Lean>,
    normal: DVec3,
    cos: f64,
    sin: f64,
}

impl Turn {
    /// No turn at all: what it is given comes back as it was.
    pub const NONE: Turn = Turn { lean: None, normal: DVec3::Z, cos: 1.0, sin: 0.0 };

    /// The turn of the surface at the point `fan` is of.
    pub fn of(fan: &impl Fan) -> Turn {
        let (mut normal_was, mut normal_now) = (DVec3::ZERO, DVec3::ZERO);
        fan.wedges(|was, now| {
            normal_was += was[0].cross(was[1]);
            normal_now += now[0].cross(now[1]);
        });
        let (Some(normal_was), Some(normal_now)) = (normal_was.try_normalize(), normal_now.try_normalize()) else {
            return Turn::NONE;
        };
        let lean = Lean::new(normal_was, normal_now);

        // The spin: with every old edge leaned over into the new tangent
        // plane, the angle whose cosine and sine these two sums are in
        // proportion to.
        let (mut along, mut across) = (0.0f64, 0.0f64);
        fan.wedges(|was, now| {
            for (was, now) in was.into_iter().zip(now) {
                let old = lean.apply(flattened(was, normal_was));
                let new = flattened(now, normal_now);
                along += old.dot(new);
                across += old.cross(new).dot(normal_now);
            }
        });
        let length = along.hypot(across);
        let (cos, sin) = if length > 0.0 && length.is_finite() { (along / length, across / length) } else { (1.0, 0.0) };
        Turn { lean: Some(lean), normal: normal_now, cos, sin }
    }

    /// `v` turned as the surface turned.
    #[inline]
    pub fn apply(&self, v: DVec3) -> DVec3 {
        let Some(lean) = &self.lean else { return v };
        let leaned = lean.apply(v);
        let n = self.normal;
        leaned * self.cos + n.cross(leaned) * self.sin + n * (n.dot(leaned) * (1.0 - self.cos))
    }

    /// The turn as a matrix, for keeping many of them or applying one to
    /// many vectors.
    pub fn matrix(&self) -> DMat3 {
        DMat3::from_cols(self.apply(DVec3::X), self.apply(DVec3::Y), self.apply(DVec3::Z))
    }

    /// Whether this is no turn at all (see [`Turn::NONE`]).
    pub fn is_none(&self) -> bool {
        self.lean.is_none()
    }
}

/// `v` without its part along the unit vector `normal`.
#[inline]
fn flattened(v: DVec3, normal: DVec3) -> DVec3 {
    v - normal * normal.dot(v)
}

/// The least rotation that takes one unit vector to another, as two half
/// turns: about the first vector, then about the one halfway between. (Two
/// half turns about axes an angle apart make a turn of twice that angle about
/// the line square to both.) Unlike the textbook form with `1 + cos` under a
/// division, this is a true rotation however near opposite the two vectors
/// are, and never divides by nearly nothing. When they are opposite (or
/// within about 1e-8 of it, where the halfway vector is all rounding) any half
/// turn square to them will do: it lands that close to the target, and the
/// spin that follows settles which way round.
#[derive(Clone, Copy, Debug)]
struct Lean {
    from: DVec3,
    halfway: DVec3,
}

impl Lean {
    fn new(from: DVec3, to: DVec3) -> Lean {
        let sum = from + to;
        let halfway = if sum.length_squared() > 1e-16 { sum.normalize() } else { from.any_orthonormal_vector() };
        Lean { from, halfway }
    }

    #[inline]
    fn apply(&self, v: DVec3) -> DVec3 {
        half_turn(self.halfway, half_turn(self.from, v))
    }
}

/// `v` turned half way round the unit vector `axis`.
#[inline]
fn half_turn(axis: DVec3, v: DVec3) -> DVec3 {
    axis * (2.0 * axis.dot(v)) - v
}

#[cfg(test)]
mod tests {
    use glam::DQuat;

    use super::*;

    #[test]
    fn a_lean_takes_one_vector_to_the_other_and_turns_nothing_else() {
        let from = DVec3::new(0.3, -0.5, 0.8).normalize();
        for to in [DVec3::new(-0.2, 0.9, 0.1).normalize(), from, -from, (-from + DVec3::X * 1e-9).normalize()] {
            let lean = Lean::new(from, to);
            // Aimed to far better than single precision even when the two are all but opposite.
            assert!((lean.apply(from) - to).length() < 1e-7, "{to}");
            // A rotation: lengths and angles are kept.
            let (a, b) = (DVec3::new(1.0, 2.0, 3.0), DVec3::new(-2.0, 0.5, 1.0));
            assert!((lean.apply(a).dot(lean.apply(b)) - a.dot(b)).abs() < 1e-12);
            assert!((lean.apply(a).cross(lean.apply(b)) - lean.apply(a.cross(b))).length() < 1e-12);
        }
        // The least one: what is square to both vectors stays put.
        let to = DVec3::new(-0.2, 0.9, 0.1).normalize();
        let axis = from.cross(to).normalize();
        assert!((Lean::new(from, to).apply(axis) - axis).length() < 1e-12);
        assert!((Lean::new(from, to).apply(from) - to).length() < 1e-12);
    }

    /// Six spokes round a point, before and after some change to them.
    struct Spokes(Vec<DVec3>, Vec<DVec3>);

    impl Fan for Spokes {
        fn wedges(&self, mut wedge: impl FnMut([DVec3; 2], [DVec3; 2])) {
            let n = self.0.len();
            (0..n).for_each(|k| wedge([self.0[k], self.0[(k + 1) % n]], [self.1[k], self.1[(k + 1) % n]]));
        }
    }

    fn fan() -> Vec<DVec3> {
        let lift = [0.1, -0.05, 0.2, 0.0, -0.15, 0.07];
        (0..6).map(|k| DVec3::new((k as f64).cos(), (k as f64).sin() * 1.3, lift[k])).collect()
    }

    #[test]
    fn a_fan_turned_rigidly_gives_back_that_turn_and_a_fan_only_stretched_gives_none() {
        let was = fan();
        let detail = DVec3::new(0.2, -0.4, 0.9);
        for turn in [DQuat::from_axis_angle(DVec3::new(1.0, 2.0, -0.5).normalize(), 2.4), DQuat::from_rotation_x(std::f64::consts::PI), DQuat::IDENTITY] {
            let now: Vec<DVec3> = was.iter().map(|&s| turn * s * 2.5).collect();
            let found = Turn::of(&Spokes(was.clone(), now));
            assert!((found.apply(detail) - turn * detail).length() < 1e-9, "the turn, and none of the scale");
        }
        // No area to take a normal from: no turn.
        let flat = vec![DVec3::X, DVec3::X * 2.0, DVec3::X * 3.0];
        assert_eq!(Turn::of(&Spokes(flat.clone(), was[..3].to_vec())).apply(detail), detail);
        assert_eq!(Turn::NONE.apply(detail), detail);
    }
}
