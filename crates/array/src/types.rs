//! What an array is made of: its kind, its settings, and where one copy goes.
//!
//! Everything about an array is relative to the object it repeats, so moving,
//! turning or resizing the object carries every copy with it and never needs a
//! new layout. Nothing is baked until the array is applied (see `bake`).

use glam::{Mat4, Quat, Vec3};

/// What an array's copies are laid out along. The numbers are the ones the
/// save file stores (0 none, 1 line, 2 curve, 3 circle).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ArrayKind {
    #[default]
    None = 0,
    /// A straight run from the object to one end point.
    Line = 1,
    /// A smooth curve through any number of points, open or closed into a loop.
    Curve = 2,
    /// Around an axis through a centre point.
    Radial = 3,
}

impl ArrayKind {
    /// The value written to a save file.
    pub fn id(self) -> i32 {
        self as i32
    }

    /// The kind a saved value stands for; anything unknown reads as no array,
    /// which is also what a file from before arrays existed holds.
    pub fn from_id(id: i32) -> ArrayKind {
        match id {
            1 => ArrayKind::Line,
            2 => ArrayKind::Curve,
            3 => ArrayKind::Radial,
            _ => ArrayKind::None,
        }
    }
}

/// The whole-array knobs. Any of them can change at any time and the copies
/// simply move.
///
/// `count` and `axis` stay `i32`, like the file stores them, so a value read
/// from a damaged file reaches `clamped` as it was written instead of failing
/// to convert first.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArraySettings {
    pub kind: ArrayKind,
    /// How many times the object is drawn, the object itself included.
    pub count: i32,
    /// Size of the last copy as a multiple of the object's own: below 1 tapers
    /// toward the end, above 1 grows toward it, 1 keeps every copy alike.
    pub end_scale: f32,
    /// Extra size at the middle of the run, fading to none at both ends: above
    /// 1 swells the middle (a spine thickest at the ribs), below 1 pinches it.
    pub mid_scale: f32,
    /// Curve only: the path runs on from its last point back to the first and
    /// the copies are spread round the whole loop, as on a bracelet.
    pub closed: bool,
    /// Copies turn to follow the path (or swing round the axis) instead of all
    /// facing the way the object does.
    pub follow: bool,
    /// Line and curve: gaps between copies in proportion to their sizes, so a
    /// tapering run stays evenly packed instead of the small end spreading out.
    pub space_by_size: bool,
    /// Radial: the world axis the copies go round (0 = X, 1 = Y, 2 = Z).
    pub axis: i32,
    /// Radial: how much of the circle is covered, in degrees. 360 spreads the
    /// copies evenly all the way round; less puts the last copy at the arc's end.
    /// Negative goes the other way round, by the same amount: a circle the Unity
    /// app turned about its Y or Z axis runs the other way once its X is negated,
    /// and this is how it keeps the copies where they were drawn.
    pub sweep: f32,
}

impl ArraySettings {
    pub const MIN_COUNT: i32 = 2;
    pub const MAX_COUNT: i32 = 100;
    pub const MIN_SCALE: f32 = 0.05;
    pub const MAX_SCALE: f32 = 4.0;

    /// The settings a new array of `kind` starts with.
    pub fn new(kind: ArrayKind) -> ArraySettings {
        ArraySettings {
            kind,
            count: if kind == ArrayKind::Radial { 8 } else { 5 },
            end_scale: 1.0,
            mid_scale: 1.0,
            closed: false,
            follow: true,
            space_by_size: true,
            axis: 1,
            sweep: 360.0,
        }
    }

    /// True for the two kinds that lie along a path of points.
    pub fn is_path(&self) -> bool {
        matches!(self.kind, ArrayKind::Line | ArrayKind::Curve)
    }

    /// The same settings with every value pulled into its legal range: what a
    /// loaded file or a script may not have respected. A value that is not a
    /// number falls back to its default rather than spreading through the layout.
    pub fn clamped(&self) -> ArraySettings {
        let scale = |v: f32| finite_or(v, 1.0).clamp(Self::MIN_SCALE, Self::MAX_SCALE);
        ArraySettings {
            count: self.count.clamp(Self::MIN_COUNT, Self::MAX_COUNT),
            end_scale: scale(self.end_scale),
            mid_scale: scale(self.mid_scale),
            axis: self.axis.clamp(0, 2),
            sweep: clamp_sweep(self.sweep),
            closed: self.closed && self.kind == ArrayKind::Curve,
            ..*self
        }
    }
}

impl Default for ArraySettings {
    fn default() -> ArraySettings {
        ArraySettings::new(ArrayKind::None)
    }
}

fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() { value } else { fallback }
}

/// A sweep with its size in range (1 to 360 degrees) and its direction kept.
fn clamp_sweep(sweep: f32) -> f32 {
    let sweep = finite_or(sweep, 360.0);
    let size = sweep.abs().clamp(1.0, 360.0);
    if sweep < 0.0 { -size } else { size }
}

/// Where something sits in the world: position, rotation, and a scale that may
/// differ per axis. Scale is applied first, in the object's own axes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Pose {
    pub const IDENTITY: Pose = Pose { position: Vec3::ZERO, rotation: Quat::IDENTITY, scale: Vec3::ONE };

    pub fn new(position: Vec3, rotation: Quat, scale: Vec3) -> Pose {
        Pose { position, rotation, scale }
    }

    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.position)
    }
}

/// Where one copy goes, relative to the object itself: the copy is the object
/// moved by `offset` (world axes, from the object's own position), turned by
/// `rotation` (applied after the object's own rotation, so in world axes, about
/// the object's pivot) and resized by `scale`. Copy 0 is always the object:
/// zero, identity, one. `rotation` is also the copy's frame on the path, which
/// is the identity when copies do not follow it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArrayPlacement {
    pub offset: Vec3,
    pub rotation: Quat,
    pub scale: f32,
}

impl ArrayPlacement {
    /// Copy 0: the object itself.
    pub const OBJECT: ArrayPlacement = ArrayPlacement { offset: Vec3::ZERO, rotation: Quat::IDENTITY, scale: 1.0 };

    /// The copy's pose in the world, given the object's.
    ///
    /// The scale is the object's own times `scale`, per axis, because a copy is
    /// the object's mesh drawn through its own transform and the object may be
    /// stretched.
    pub fn apply(&self, object: &Pose) -> Pose {
        Pose {
            position: object.position + self.offset,
            rotation: self.rotation * object.rotation,
            scale: object.scale * self.scale,
        }
    }

    /// The copy's matrix in the world.
    pub fn world_matrix(&self, object: &Pose) -> Mat4 {
        self.apply(object).matrix()
    }

    /// The copy's matrix in the object's own local space: what takes the
    /// object's mesh to this copy's place, as baking an array wants it.
    /// Exactly the identity for a copy that is the object itself, so copy 0
    /// comes through baking bit for bit.
    pub fn local_matrix(&self, object: &Pose) -> Mat4 {
        self.local_matrix_with(&object.matrix().inverse(), object)
    }

    /// `local_matrix` with the object's inverse matrix already in hand, for a
    /// loop over every copy.
    pub(crate) fn local_matrix_with(&self, object_inverse: &Mat4, object: &Pose) -> Mat4 {
        if *self == ArrayPlacement::OBJECT {
            return Mat4::IDENTITY;
        }
        *object_inverse * self.world_matrix(object)
    }

    /// The rotation to give the object so that this copy faces `world`, and
    /// with it every other copy in its own place. A copy's facing is the object's
    /// own rotation seen through the copy's frame on the path, so turning a copy
    /// is turning the object.
    pub fn object_rotation_for(&self, world: Quat) -> Quat {
        self.rotation.inverse() * world
    }

    /// How far to carry the object, along its own place on the path (see
    /// `ArrayState::shift_object`), so that this copy moves by `world_delta`
    /// and the path stays where it is in the world. Every other copy sits off
    /// the path by the same amount in its own frame and size.
    pub fn object_shift_for(&self, world_delta: Vec3) -> Vec3 {
        self.rotation.inverse() * world_delta / self.scale.max(1e-4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_unity_ones() {
        let s = ArraySettings::new(ArrayKind::Line);
        assert_eq!((s.count, s.end_scale, s.mid_scale, s.axis, s.sweep), (5, 1.0, 1.0, 1, 360.0));
        assert!(s.follow && s.space_by_size && !s.closed);
        assert_eq!(ArraySettings::new(ArrayKind::Radial).count, 8);
        assert_eq!(ArraySettings::new(ArrayKind::Curve).count, 5);
    }

    #[test]
    fn kinds_round_trip_through_their_saved_numbers() {
        for kind in [ArrayKind::None, ArrayKind::Line, ArrayKind::Curve, ArrayKind::Radial] {
            assert_eq!(ArrayKind::from_id(kind.id()), kind);
        }
        assert_eq!(ArrayKind::from_id(9), ArrayKind::None);
        assert_eq!(ArrayKind::from_id(-1), ArrayKind::None);
    }

    #[test]
    fn clamping_pulls_everything_into_range() {
        let mut s = ArraySettings::new(ArrayKind::Line);
        s.count = 100_000;
        s.end_scale = -5.0;
        s.mid_scale = 99.0;
        s.axis = 7;
        s.sweep = 1000.0;
        s.closed = true;
        let c = s.clamped();
        assert_eq!(c.count, ArraySettings::MAX_COUNT);
        assert_eq!(c.end_scale, ArraySettings::MIN_SCALE);
        assert_eq!(c.mid_scale, ArraySettings::MAX_SCALE);
        assert_eq!((c.axis, c.sweep), (2, 360.0));
        assert!(!c.closed, "only a curve can close");

        s.count = -3;
        s.sweep = 0.0;
        s.end_scale = f32::NAN;
        let c = s.clamped();
        assert_eq!((c.count, c.sweep, c.end_scale), (ArraySettings::MIN_COUNT, 1.0, 1.0));

        s.kind = ArrayKind::Curve;
        assert!(s.clamped().closed);
    }

    #[test]
    fn a_sweep_keeps_its_direction_and_is_limited_in_size() {
        let mut s = ArraySettings::new(ArrayKind::Radial);
        let cases = [(-90.0, -90.0), (-1000.0, -360.0), (-0.2, -1.0), (0.2, 1.0), (-0.0, 1.0), (f32::NEG_INFINITY, 360.0)];
        for (given, kept) in cases {
            s.sweep = given;
            assert_eq!(s.clamped().sweep, kept, "{given}");
        }
    }

    #[test]
    fn the_object_placement_is_an_exact_identity_in_its_own_space() {
        let object = Pose::new(Vec3::new(1.0, 2.0, 3.0), Quat::from_rotation_y(0.4), Vec3::new(0.8, 1.3, 0.6));
        assert_eq!(ArrayPlacement::OBJECT.local_matrix(&object), Mat4::IDENTITY);
        assert_eq!(ArrayPlacement::OBJECT.apply(&object), object);
    }

    #[test]
    fn a_copys_scale_multiplies_the_objects_per_axis() {
        let object = Pose::new(Vec3::ZERO, Quat::IDENTITY, Vec3::new(2.0, 3.0, 4.0));
        let p = ArrayPlacement { offset: Vec3::X, rotation: Quat::IDENTITY, scale: 0.5 };
        assert_eq!(p.apply(&object).scale, Vec3::new(1.0, 1.5, 2.0));
    }
}
