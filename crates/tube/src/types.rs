//! What a tube is made of: the points the artist edits, the whole-tube
//! settings, and the two forms the curve code turns them into.

use glam::Vec3;

/// How a tube's two ends are closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CapStyle {
    /// A dome: the tube ends in half a sphere of its end radius.
    #[default]
    Round,
    /// Cut square across the tube's direction at the end.
    Flat,
}

/// One control point of a tube as the artist edits it, in the space of the
/// object the tube was drawn on (its anchor), so the tube rides along when
/// that object is moved.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TubePoint {
    /// Where the point sits. For a point placed on the surface this is the
    /// surface itself: the centre line is lifted off it along
    /// `surface_normal` by a fraction of the radius when the tube is built
    /// (see `spine`), so changing the radius never floats the tube off the
    /// model or buries it.
    pub position: Vec3,
    /// Unit surface normal for a point placed on the surface; zero for a
    /// point floating in space, whose position is the centre line itself.
    pub surface_normal: Vec3,
    /// Thickness here, as a multiple of the tube's radius.
    pub size: f32,
    /// The tube turns a sharp corner here instead of curving through.
    pub sharp: bool,
}

impl TubePoint {
    /// A point floating in space.
    pub fn free(position: Vec3) -> TubePoint {
        TubePoint { position, surface_normal: Vec3::ZERO, size: 1.0, sharp: false }
    }

    /// A point stuck to a surface with the given normal.
    pub fn on_surface(position: Vec3, normal: Vec3) -> TubePoint {
        TubePoint { position, surface_normal: normal.normalize_or_zero(), size: 1.0, sharp: false }
    }

    pub fn sized(mut self, size: f32) -> TubePoint {
        self.size = size;
        self
    }

    pub fn sharp(mut self) -> TubePoint {
        self.sharp = true;
        self
    }

    pub fn is_on_surface(&self) -> bool {
        self.surface_normal.length_squared() > 0.25
    }
}

/// The whole-tube knobs. Every one can be changed after the tube is drawn,
/// and the tube rebuilds from its points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TubeSettings {
    /// World-space radius at size 1, before taper.
    pub radius: f32,
    /// 0 is the same thickness end to end; 1 narrows to a point at the tip.
    pub taper: f32,
    /// Taper toward the start of the stroke instead of its end.
    pub flip_taper: bool,
    pub caps: CapStyle,
    /// How loosely the tube flows between its points: 0 is straight runs
    /// with a kink at every point, 1 the loosest, roundest curve. Never
    /// moves a point.
    pub smoothness: f32,
    /// Placement for new points: stuck to the surface, or floating on a
    /// plane facing the view.
    pub on_surface: bool,
    /// Size of the fillet where the tube meets the model when merged, as a
    /// multiple of the radius. 0 is a hard crease.
    pub blend: f32,
}

impl TubeSettings {
    pub const MIN_RADIUS: f32 = 0.001;
    pub const MAX_RADIUS: f32 = 10.0;
    pub const MIN_SIZE: f32 = 0.1;
    pub const MAX_SIZE: f32 = 5.0;
    pub const MAX_BLEND: f32 = 1.5;
}

impl Default for TubeSettings {
    fn default() -> TubeSettings {
        TubeSettings {
            radius: 0.08,
            taper: 0.0,
            flip_taper: false,
            caps: CapStyle::Round,
            smoothness: 0.6,
            on_surface: false,
            blend: 0.5,
        }
    }
}

/// A control point ready for the curve: surface lift, taper, symmetry and the
/// anchor's scale already applied (see `spine`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ControlPoint {
    pub position: Vec3,
    pub radius: f32,
    pub sharp: bool,
}

impl ControlPoint {
    pub fn new(position: Vec3, radius: f32) -> ControlPoint {
        ControlPoint { position, radius, sharp: false }
    }
}

/// One station along the resampled centre line.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Sample {
    pub center: Vec3,
    pub radius: f32,
    /// Direction the centre line arrives in and leaves in. Equal except at a
    /// corner.
    pub tangent_in: Vec3,
    pub tangent_out: Vec3,
    /// A sharp control point: the tube turns here with a ball joint.
    pub corner: bool,
    /// Arc length from the start of the tube.
    pub distance: f32,
    /// Which control segment (point `segment` to point `segment + 1`) this
    /// station lies on, and how far along it, so a click on the tube can
    /// insert a point in the right place.
    pub segment: usize,
    pub segment_t: f32,
}
