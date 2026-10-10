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
//! The settings here are the sculptor's working copy. They belong to the
//! object being sculpted (`scene::Symmetry`), and the scene hands them over
//! before a stroke (`Scene::hand_symmetry_to`), so nothing in this module
//! knows which object it is working on.
//!
//! The copies are always in the sculpted mesh's own space. A symmetry of the
//! object's own (Local) goes through its origin along its axes, and a mirror
//! image is then an exact sign flip. One fixed in the world (World,
//! `Settings::symmetry_world`) is the same list built about the world's
//! origin and axes and carried into the mesh's space through where the object
//! stands (`SymmetryGroup::in_object_frame`): each copy then also shifts
//! points, and on an object that is turned its mirror images are general
//! reflections, equal to rounding rather than to the bit.

use glam::Vec3;
use sculpt_geom::SymmetryGroup;
use sculpt_symmetry::Placement;
use serde::{Deserialize, Serialize};

use super::Settings;
use super::kernels::SymOp;

/// The axis radial symmetry repeats round: X, Y or Z of the space the
/// symmetry sits in (the object's own, or the world's), or a direction of the
/// user's choosing in that space.
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
/// an axis through the origin of the space the symmetry sits in. It combines
/// with the mirror axes.
///
/// Kept in `.sclp` files with the mirror axes, under each object
/// (`scene::Symmetry`) and in what older files had for the whole scene
/// (`io::Mirror`), so a field added here needs a value to start from.
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

    /// The axis as a unit direction in the space the symmetry sits in.
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

/// The mirror planes of a stroke with no turns in its symmetry, as the walk
/// over mirrored copies needs them (`Sculptor::apply_symmetric`): which
/// planes each copy lies across, and how far a dab is from each plane.
///
/// For the object's own planes that is read off the copies, which are sign
/// flips, and the distance is a coordinate of the dab. The planes of a
/// symmetry fixed in the world lie anywhere in the mesh's space once the
/// object has been moved or turned, so there the copies' order says which
/// plane is which (carrying a symmetry into an object's space keeps it), and
/// the distance to a plane is half the way to the dab's image across it.
#[derive(Default)]
pub(super) struct Mirrors {
    /// For each copy, in the order of `Stroke::ops`, the planes it lies
    /// across from the stroke itself: X = 1, Y = 2, Z = 4.
    across: Vec<u8>,
    /// The copy across each plane alone, where the planes are not the
    /// object's own.
    placed: Option<[Option<SymOp>; 3]>,
}

impl Mirrors {
    /// The planes behind `ops`, the copies of a stroke under `settings`.
    /// Empty when there are turns among the copies: `radial::Walk` has those.
    pub fn of(settings: &Settings, ops: &[SymOp]) -> Mirrors {
        if settings.radial.repeats() >= Radial::MIN_COUNT {
            return Mirrors::default();
        }
        let mask = |flips: [bool; 3]| flips[0] as u8 | (flips[1] as u8) << 1 | (flips[2] as u8) << 2;
        if ops.iter().all(|op| op.is_own_flip()) {
            let across = ops.iter().map(|op| mask([0, 1, 2].map(|axis| op.flips(axis)))).collect();
            return Mirrors { across, placed: None };
        }
        // The same planes where they were built, about the world's origin:
        // sign flips, in the order the copies in the mesh's space kept.
        let [x, y, z] = settings.symmetry;
        let world = SymmetryGroup::build(x, y, z, 1, Vec3::Y);
        let across: Vec<u8> = world.iter().map(|op| mask([0, 1, 2].map(|axis| op.sign()[axis] < 0.0))).collect();
        debug_assert_eq!(across.len(), ops.len());
        let plane = |axis: usize| across.iter().position(|&m| m == 1 << axis).and_then(|k| ops.get(k).copied());
        Mirrors { placed: Some([0, 1, 2].map(plane)), across }
    }

    /// The planes copy `k` lies across from the stroke itself.
    #[inline]
    pub fn across(&self, k: usize) -> u8 {
        self.across[k]
    }

    /// The planes a dab at `centre` is closer to than `within`: its copies
    /// across those can reach its own footprint.
    pub fn near(&self, centre: Vec3, within: f32) -> u8 {
        let mut near = 0;
        for axis in 0..3 {
            let bit = 1 << axis;
            let from_plane = match &self.placed {
                None if self.across.iter().any(|&m| m & bit != 0) => centre[axis].abs(),
                Some(planes) => match planes[axis] {
                    Some(op) => 0.5 * centre.distance(op.apply_point(centre)),
                    None => continue,
                },
                None => continue,
            };
            if from_plane < within {
                near |= bit;
            }
        }
        near
    }
}

/// `v` as a unit vector, or Y if it has no direction.
fn unit_or_y(v: Vec3) -> Vec3 {
    if v.is_finite() && v.length_squared() > 1e-12 { v.normalize() } else { Vec3::Y }
}

impl Settings {
    /// Every copy a stroke is repeated as under the settings as they stand,
    /// the stroke itself first, in the sculpted mesh's own space.
    pub fn symmetry_group(&self) -> SymmetryGroup {
        let [x, y, z] = self.symmetry;
        let group = SymmetryGroup::build(x, y, z, self.radial.repeats(), self.radial.axis_vector());
        match self.symmetry_world {
            // One copy is the stroke itself, wherever the planes are.
            Some(at) if group.len() > 1 => group.in_object_frame(at.rotation, at.position, &at.matrix().inverse()),
            _ => group,
        }
    }

    /// Takes on an object's symmetry: its mirror axes and radial symmetry,
    /// and with `world` the place the object stands when those are the
    /// world's rather than its own.
    pub fn set_symmetry(&mut self, mirror: [bool; 3], radial: Radial, world: Option<Placement>) {
        (self.symmetry, self.radial, self.symmetry_world) = (mirror, radial, world);
    }

    /// The same as the list a stroke keeps: what a stroke begun now would be
    /// made as (the Pose brush's preview asks before there is a stroke).
    pub(super) fn ops(&self) -> Vec<SymOp> {
        copies(&self.symmetry_group())
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
