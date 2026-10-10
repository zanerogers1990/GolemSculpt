//! The small types every part of the key geometry shares: which half of a
//! joint a feature is, a key's dimensions, where it sits, and one feature to
//! build.

use std::sync::Arc;

use glam::{Vec2, Vec3};
use sculpt_geom::units;

use crate::profile::KeyProfile;

/// Which half of a joint a key feature is built into: the solid peg standing
/// proud of one cut face, or the matching socket sunk into the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyRole {
    Peg,
    Hole,
}

/// A key's dimensions, in world units (see `sculpt_geom::units` for
/// millimetres). Everything a printed fit depends on is here, so a peg and
/// its hole are always built from one copy.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct KeyDimensions {
    /// How far the peg stands out of its cut face.
    pub height: f32,
    /// Gap between peg and hole wall, per side. The hole is the peg's outline
    /// grown by this much, as a true parallel offset (see `KeyProfile`),
    /// corners included.
    pub clearance: f32,
    /// How much deeper the hole is than the peg is tall, so the peg bottoms
    /// out on nothing and the two cut faces close fully.
    pub extra_depth: f32,
    /// A 45-degree bevel round the peg's tip, so it finds the hole.
    pub tip_chamfer: f32,
    /// A 45-degree bevel round the hole's mouth: lead-in for the peg, and
    /// room for the flared first layer ("elephant's foot") of a peg printed
    /// standing on its cut face.
    pub entry_chamfer: f32,
    /// Draft, in degrees: the peg narrows toward its tip and the hole toward
    /// its floor by the same angle, so the clearance is the same at every
    /// depth.
    pub taper_degrees: f32,
}

impl KeyDimensions {
    pub fn hole_depth(&self) -> f32 {
        self.height + self.extra_depth
    }

    pub fn taper_slope(&self) -> f32 {
        self.taper_degrees.clamp(0.0, 20.0).to_radians().tan()
    }

    /// The offset of the hole's outline at the cut face, relative to the
    /// peg's.
    pub fn mouth_offset(&self) -> f32 {
        self.clearance + self.entry_chamfer
    }

    /// The most the peg's outline shrinks anywhere (at its tip), as a
    /// negative offset.
    pub fn tip_offset(&self) -> f32 {
        -(self.height * self.taper_slope() + self.tip_chamfer)
    }

    /// Dimensions given in millimetres.
    pub fn from_millimetres(
        height: f32,
        clearance: f32,
        extra_depth: f32,
        tip_chamfer: f32,
        entry_chamfer: f32,
        taper_degrees: f32,
    ) -> KeyDimensions {
        KeyDimensions {
            height: units::from_millimetres(height),
            clearance: units::from_millimetres(clearance),
            extra_depth: units::from_millimetres(extra_depth),
            tip_chamfer: units::from_millimetres(tip_chamfer),
            entry_chamfer: units::from_millimetres(entry_chamfer),
            taper_degrees,
        }
    }
}

/// Where a key sits, in world space. `origin` is on the cut face; `axis` is
/// the direction the two parts separate in, pointing from the peg's part into
/// the hole's part (so the peg grows along it and the hole is sunk along it);
/// `u` and `v` are the profile's own x and y, perpendicular to the axis and
/// to each other, with `u x v = axis`: an outline that runs counter-clockwise
/// in the profile's plane runs counter-clockwise seen from the tip of the
/// axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeyFrame {
    pub origin: Vec3,
    pub axis: Vec3,
    pub u: Vec3,
    pub v: Vec3,
}

impl KeyFrame {
    /// A frame at `origin` along `axis`, with the profile's x as near
    /// `reference` as the axis allows, then turned `angle_degrees` about the
    /// axis.
    pub fn new(origin: Vec3, axis: Vec3, reference: Vec3, angle_degrees: f32) -> KeyFrame {
        let axis = axis.normalize_or(Vec3::Y);
        let mut u = reference - axis * reference.dot(axis);
        if u.length_squared() < 1e-12 {
            u = any_perpendicular(axis);
        }
        let u = u.normalize();
        let v = axis.cross(u).normalize();
        let (s, c) = angle_degrees.to_radians().sin_cos();
        KeyFrame { origin, axis, u: u * c + v * s, v: v * c - u * s }
    }

    /// A point of the profile's plane, `height` along the axis.
    #[inline]
    pub fn point(&self, profile: Vec2, height: f32) -> Vec3 {
        self.origin + self.u * profile.x + self.v * profile.y + self.axis * height
    }
}

/// Some unit vector at right angles to `axis`.
pub fn any_perpendicular(axis: Vec3) -> Vec3 {
    let other = if axis.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
    axis.cross(other).normalize()
}

/// One key feature to build into one part: a peg or a hole, with its
/// profile, dimensions and frame, on one of that part's cut faces.
#[derive(Clone, Debug)]
pub struct KeyFeature {
    pub role: KeyRole,
    pub profile: Arc<KeyProfile>,
    pub dimensions: KeyDimensions,
    pub frame: KeyFrame,
    /// Index into the part's cut faces (`KeyPart::cut_faces`).
    pub patch: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_is_orthonormal_and_right_handed_about_its_axis() {
        for (axis, reference, angle) in [
            (Vec3::Z, Vec3::X, 0.0),
            (Vec3::new(0.3, -0.8, 0.5), Vec3::new(1.0, 0.2, 0.0), 35.0),
            (Vec3::NEG_Y, Vec3::NEG_Y, 270.0),
        ] {
            let f = KeyFrame::new(Vec3::ONE, axis, reference, angle);
            for (a, b) in [(f.u, f.v), (f.v, f.axis), (f.axis, f.u)] {
                assert!(a.dot(b).abs() < 1e-5);
            }
            assert!((f.u.length() - 1.0).abs() < 1e-5 && (f.v.length() - 1.0).abs() < 1e-5);
            assert!(f.u.cross(f.v).distance(f.axis) < 1e-5, "u x v is the axis");
        }
        // Turning by a quarter sends the profile's x to where its y was.
        let flat = KeyFrame::new(Vec3::ZERO, Vec3::Z, Vec3::X, 0.0);
        let turned = KeyFrame::new(Vec3::ZERO, Vec3::Z, Vec3::X, 90.0);
        assert!(turned.u.distance(flat.v) < 1e-5);
        assert!(flat.point(Vec2::new(2.0, 3.0), 4.0).distance(Vec3::new(2.0, 3.0, 4.0)) < 1e-5);
    }

    #[test]
    fn dimensions_derive_the_offsets_the_fit_is_built_from() {
        let d = KeyDimensions::from_millimetres(5.0, 0.2, 0.4, 0.5, 0.3, 0.0);
        assert!((units::to_millimetres(d.hole_depth()) - 5.4).abs() < 1e-4);
        assert!((units::to_millimetres(d.mouth_offset()) - 0.5).abs() < 1e-4);
        assert!((units::to_millimetres(d.tip_offset()) + 0.5).abs() < 1e-4);
        let drafted = KeyDimensions { taper_degrees: 45.0, ..d };
        assert!((drafted.taper_slope() - 20f32.to_radians().tan()).abs() < 1e-6, "draft is capped at 20 degrees");
    }
}
