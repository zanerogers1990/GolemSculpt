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

/// What a live shape is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Kind {
    /// The same thickness along its length, or narrowing toward one end.
    #[default]
    Tube,
    /// Thin at both ends and full in between, wider than it is deep, and laid along the surface between
    /// its points rather than only at them.
    Muscle,
    /// A flat band of even width and thickness with square or softened edges, resting on the surface
    /// along its whole length: a belt, a strap, a band. Built as a swept mesh, not a field (see `strap`).
    Strap,
}

impl Kind {
    /// What the tool calls one of its shapes, and several, in a sentence and at the start of one.
    pub(crate) fn one(self) -> &'static str {
        match self {
            Kind::Tube => "tube",
            Kind::Muscle => "muscle",
            Kind::Strap => "strap",
        }
    }

    pub(crate) fn many(self) -> &'static str {
        match self {
            Kind::Tube => "tubes",
            Kind::Muscle => "muscles",
            Kind::Strap => "straps",
        }
    }

    pub(crate) fn title(self) -> &'static str {
        match self {
            Kind::Tube => "Tube",
            Kind::Muscle => "Muscle",
            Kind::Strap => "Strap",
        }
    }

    /// Laid along the surface between its points, not only touching it at them.
    pub fn lies_on_surface(self) -> bool {
        self != Kind::Tube
    }
}

/// How a strap's two ends are shaped, seen from above.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tip {
    /// Cut straight across.
    #[default]
    Square,
    /// A half round, as wide as the strap.
    Round,
    /// Narrowing to a point, as a belt's tip does.
    Point,
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
    /// For a point floating in space: the direction a muscle is thin in there, in the anchor's space. It
    /// is the way the view faced when the point was placed, so a muscle keeps the width it was drawn
    /// with. Zero for a round cross-section.
    pub facing: Vec3,
    /// Thickness here, as a multiple of the tube's radius.
    pub size: f32,
    /// The tube turns a sharp corner here instead of curving through.
    pub sharp: bool,
}

impl TubePoint {
    /// A point floating in space.
    pub fn free(position: Vec3) -> TubePoint {
        TubePoint { position, surface_normal: Vec3::ZERO, facing: Vec3::ZERO, size: 1.0, sharp: false }
    }

    /// A point stuck to a surface with the given normal.
    pub fn on_surface(position: Vec3, normal: Vec3) -> TubePoint {
        TubePoint { position, surface_normal: normal.normalize_or_zero(), facing: Vec3::ZERO, size: 1.0, sharp: false }
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

    /// The direction a muscle is thin in at this point: off the surface for a point on it, toward where
    /// it was drawn from for a floating one.
    pub fn up(&self) -> Vec3 {
        if self.is_on_surface() { self.surface_normal } else { self.facing }
    }
}

/// The whole-tube knobs. Every one can be changed after the tube is drawn,
/// and the tube rebuilds from its points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TubeSettings {
    /// World-space radius at size 1, before taper.
    pub radius: f32,
    /// 0 is the same thickness end to end; 1 narrows to a point at the tip. A muscle narrows toward
    /// both ends.
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
    /// multiple of the radius (for a muscle, of how far it stands off the
    /// model: see [`TubeSettings::proud`]). 0 is a hard crease.
    pub blend: f32,
    pub kind: Kind,
    /// A muscle's fullness: 0 is a lean spindle that is only wide at its middle, 1 stays wide almost to
    /// its ends.
    pub swell: f32,
    /// Where along a muscle its widest part sits: 0.5 is the middle.
    pub belly: f32,
    /// A muscle's depth as a fraction of its width: 1 is round.
    pub flatten: f32,
    /// How far a muscle on the surface is sunk into it: 0 rests on top, 0.5 is buried to its widest, 1
    /// is gone.
    pub depth: f32,
    /// A strap's thickness, in the same units as `radius` (its half width): the same however wide it
    /// is.
    pub thickness: f32,
    /// How round a strap's edges are: 0 square, 1 as round as its thickness allows.
    pub edge: f32,
    /// How far a strap stands off the skin, as a fraction of its thickness, so that it never dips
    /// into it.
    pub gap: f32,
    /// How taut a strap is drawn: 0 follows every dip of the body, 1 bridges hollows as a tight belt
    /// does across the small of the back.
    pub tight: f32,
    pub tip: Tip,
    /// A strap that goes round and joins itself: its last point runs on to its first.
    pub closed: bool,
}

impl TubeSettings {
    pub const MIN_FLATTEN: f32 = 0.2;
    pub const MAX_DEPTH: f32 = 0.9;
    /// A strap is never thinner than this share of its half width, nor thicker than this many half
    /// widths.
    pub const MIN_THICKNESS_SHARE: f32 = 0.01;
    pub const MAX_THICKNESS_SHARE: f32 = 2.0;
    pub const MAX_GAP: f32 = 1.0;

    /// What a muscle starts as, at the radius the tool has.
    pub fn muscle(self) -> TubeSettings {
        TubeSettings { kind: Kind::Muscle, taper: 0.8, on_surface: true, blend: 0.2, ..self }
    }

    /// What a strap starts as, at the radius the tool has: a belt about a sixth as thick as it is wide,
    /// edges softened a little, drawn fairly taut.
    pub fn strap(self) -> TubeSettings {
        TubeSettings {
            kind: Kind::Strap,
            taper: 0.0,
            on_surface: true,
            blend: 0.0,
            smoothness: 0.7,
            thickness: self.radius * 0.3,
            ..self
        }
    }

    /// A strap's thickness within its limits for its width.
    pub fn strap_thickness(&self) -> f32 {
        let r = self.radius.clamp(TubeSettings::MIN_RADIUS, TubeSettings::MAX_RADIUS);
        self.thickness.clamp(r * TubeSettings::MIN_THICKNESS_SHARE, r * TubeSettings::MAX_THICKNESS_SHARE)
    }

    /// A muscle's depth as a fraction of its width, within the limits; a strap's thickness as one of
    /// its width; 1 for a tube.
    pub fn flat(&self) -> f32 {
        match self.kind {
            Kind::Muscle => self.flatten.clamp(TubeSettings::MIN_FLATTEN, 1.0),
            Kind::Strap => 0.5 * self.strap_thickness() / self.radius.max(TubeSettings::MIN_RADIUS),
            Kind::Tube => 1.0,
        }
    }

    /// Half the least thickness of the shape at its fullest: what a lattice has to hold.
    pub fn thin_radius(&self) -> f32 {
        self.radius * self.flat()
    }

    /// How far the shape stands proud of the model it lies on, at its fullest: what the fillet of a
    /// merge is sized by. A tube's is its radius. A muscle is wide and low, and a fillet sized by its
    /// width would swallow it: its own is its height off the surface. A strap's is its thickness.
    pub fn proud(&self) -> f32 {
        match self.kind {
            Kind::Muscle => self.thin_radius() * (2.0 - 2.0 * self.depth.clamp(0.0, TubeSettings::MAX_DEPTH)),
            Kind::Strap => self.strap_thickness(),
            Kind::Tube => self.radius,
        }
    }

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
            kind: Kind::Tube,
            swell: 0.7,
            belly: 0.5,
            flatten: 0.55,
            depth: 0.35,
            thickness: 0.024,
            edge: 0.35,
            gap: 0.1,
            tight: 0.7,
            tip: Tip::Square,
            closed: false,
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
    /// The direction the cross-section is thin in, and its depth as a fraction of its width. Zero and
    /// anything for a round one.
    pub up: Vec3,
    pub flat: f32,
    /// A muscle's underside filled down to the model: from `rest` below the centre (along `up`), where
    /// the cross-section meets the surface it rests on, to `under` below it, as wide as the
    /// cross-section is there. Nothing when `under` is not past `rest`. See `muscle::sample`.
    pub rest: f32,
    pub under: f32,
}
