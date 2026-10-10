//! The symmetry a stroke is repeated under: the mirror axes that are ticked,
//! times the turns of radial symmetry.
//!
//! The settings become one list of copies (`sculpt_geom::SymmetryGroup`), and
//! a stroke keeps that list from press to release (`Stroke::ops`). The first
//! copy is always the stroke as it was made. A copy maps a point with
//! `SymOp::apply_point` and a direction or a normal with `apply_dir`, and
//! that is all a brush needs to know about it, whether the copy is a mirror
//! image, a turn, or both.
//!
//! Mirror-only lists come out in the order mirrored sculpting has always
//! walked them (each ticked axis doubling the list: X, then Y, then Z), every
//! copy a per-axis sign flip, so a stroke with no radial symmetry is to the
//! bit what it was before there was any.
//!
//! How the two combine is the group's business. A mirror plane through the
//! radial axis gives each turn and its reflection. A copy two settings would
//! both make (mirror Z on top of mirror X is the half turn an even count
//! about Y already has) is listed once, so no stroke is made twice in one
//! place. A custom axis tilted against a mirror plane makes a list that is
//! not a closed group, and its copies are then applied one after another.
//!
//! All of it is in the sculpted object's own space, through its origin, as
//! the mirror planes are. (The Unity app keeps these settings per object and
//! can fix them in the world instead; here they are the scene's, like the
//! mirror axes they sit beside.)

use glam::Vec3;
use sculpt_geom::SymmetryGroup;
use serde::{Deserialize, Serialize};

use super::Settings;
use super::kernels::SymOp;

/// The axis radial symmetry repeats round: one of the object's own, or a
/// direction of the user's choosing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RadialAxis {
    X,
    /// Where it starts: the axis a lathed object is turned round, and the
    /// usual "up" of a bust or a vase.
    #[default]
    Y,
    Z,
    Custom,
}

impl RadialAxis {
    pub const ALL: [RadialAxis; 4] = [RadialAxis::X, RadialAxis::Y, RadialAxis::Z, RadialAxis::Custom];

    pub fn name(self) -> &'static str {
        match self {
            RadialAxis::X => "X",
            RadialAxis::Y => "Y",
            RadialAxis::Z => "Z",
            RadialAxis::Custom => "Custom",
        }
    }
}

/// Radial symmetry: every stroke repeated `count` times, evenly spaced, round
/// an axis through the object's origin. It combines with the mirror axes.
///
/// Kept in `.sclp` files with the mirror axes (`io::Mirror`), so a field
/// added here needs a value to start from.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Radial {
    pub on: bool,
    /// How many times a stroke is made, the stroke itself included.
    pub count: u32,
    pub axis: RadialAxis,
    /// The direction used when `axis` is `Custom`. Kept as given; see
    /// [`Self::axis_vector`] for what a zero one falls back to.
    #[serde(with = "crate::io::vec3")]
    pub custom_axis: Vec3,
}

impl Default for Radial {
    fn default() -> Radial {
        Radial { on: false, count: 8, axis: RadialAxis::Y, custom_axis: Vec3::Y }
    }
}

impl Radial {
    pub const MIN_COUNT: u32 = SymmetryGroup::MIN_RADIAL_COUNT;
    pub const MAX_COUNT: u32 = SymmetryGroup::MAX_RADIAL_COUNT;

    /// How many times round the axis a stroke is made: 1 with radial symmetry
    /// off. (A count below 2 is off as well, and one above the most there can
    /// be is that most; `SymmetryGroup::build` sees to both.)
    pub fn repeats(&self) -> u32 {
        if self.on { self.count } else { 1 }
    }

    /// The axis as a unit direction in the object's own space.
    pub fn axis_vector(&self) -> Vec3 {
        match self.axis {
            RadialAxis::X => Vec3::X,
            RadialAxis::Y => Vec3::Y,
            RadialAxis::Z => Vec3::Z,
            RadialAxis::Custom => unit_or_y(self.custom_axis),
        }
    }

    /// Points the custom axis along `direction` and switches to it. A
    /// direction that is not one changes nothing.
    pub fn set_custom_axis(&mut self, direction: Vec3) {
        if direction.is_finite() && direction.length_squared() > 1e-12 {
            self.custom_axis = direction.normalize();
            self.axis = RadialAxis::Custom;
        }
    }

    /// What the viewport needs to draw the guide: the axis with how many
    /// spokes go round it, and the way the first spoke points with which
    /// colour it is in (0, 1, 2 for the X, Y, Z axis colours, 3 for a custom
    /// axis), each as the four numbers the shader takes. `None` with radial
    /// symmetry off.
    ///
    /// The spokes are where the Unity app draws them: the first along X
    /// about a Y axis, and carried with the axis by the shortest turn from Y
    /// about any other.
    pub fn guide(&self) -> Option<([f32; 4], [f32; 4])> {
        let repeats = self.repeats().clamp(1, Self::MAX_COUNT);
        if repeats < Self::MIN_COUNT {
            return None;
        }
        let axis = self.axis_vector();
        let spoke = glam::Quat::from_rotation_arc(Vec3::Y, axis) * Vec3::X;
        let color = RadialAxis::ALL.iter().position(|&a| a == self.axis).unwrap_or(1);
        Some((axis.extend(repeats as f32).to_array(), spoke.extend(color as f32).to_array()))
    }

    /// These settings with everything in range: what a file or a caller's
    /// numbers are passed through before they are used.
    pub fn sanitized(self) -> Radial {
        Radial {
            count: self.count.clamp(Self::MIN_COUNT, Self::MAX_COUNT),
            custom_axis: unit_or_y(self.custom_axis),
            ..self
        }
    }
}

/// A symmetry as the list of copies a stroke keeps (`Stroke::ops`), in the
/// symmetry's own order.
pub(super) fn copies(group: &SymmetryGroup) -> Vec<SymOp> {
    group.iter().map(|&op| SymOp::from(op)).collect()
}

/// `v` as a unit vector, or Y if it has no direction.
fn unit_or_y(v: Vec3) -> Vec3 {
    if v.is_finite() && v.length_squared() > 1e-12 { v.normalize() } else { Vec3::Y }
}

impl Settings {
    /// Every copy a stroke is repeated as under the settings as they stand,
    /// the stroke itself first.
    pub fn symmetry_group(&self) -> SymmetryGroup {
        let [x, y, z] = self.symmetry;
        SymmetryGroup::build(x, y, z, self.radial.repeats(), self.radial.axis_vector())
    }

    /// What the Symmetry card says in its header: "X + Z + Radial 8", or "Off".
    pub fn symmetry_summary(&self) -> String {
        let mut parts: Vec<String> =
            (0..3).filter(|&axis| self.symmetry[axis]).map(|axis| ["X", "Y", "Z"][axis].to_owned()).collect();
        if self.radial.on {
            parts.push(format!("Radial {}", self.radial.count));
        }
        if parts.is_empty() { "Off".to_owned() } else { parts.join(" + ") }
    }
}
