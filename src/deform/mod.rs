//! Gizmo deformers: whole-object (or masked) reshapes driven from the
//! transform gizmo, as ZBrush's Gizmo 3D deformers are. Bend, Bend Arc,
//! Twist, Taper, Flatten, Inflate, Stretch and Lattice.
//!
//! This is the maths only, and it knows nothing of meshes or the app: a
//! deformer is set up once from its settings and the box the deformed
//! objects fill ([`Deformer::new`]), then maps one point at a time
//! ([`Deformer::apply`]). Points are in the gizmo's frame: its pivot is the
//! origin and its three axes are the coordinate axes. The app carries every
//! vertex there and back (app/deform.rs).
//!
//! **Along an axis.** Every deformer works along one of the gizmo's axes
//! (`Params::axis`) over a range of it: by default the whole extent of the
//! box, set narrower with the two range handles. What happens across the
//! axis is measured from the middle of the box, not the pivot: a twist turns
//! the shape about its own middle wherever the object's origin was left.
//!
//! **The range and its soft ends** (profile.rs). Bend, Twist, Taper,
//! Flatten and Stretch build up through the range and hold what they reached
//! beyond it (Barr's global deformations): past the end of a bent range the
//! shape goes on straight, along the bent direction, as a rigid piece.
//! Inflate and Lattice act inside the range only. Softness eases each end of
//! the range so that no crease is left where the deformation starts or stops;
//! at 0 the deformations are Barr's own, linear along the axis.
//!
//! **Exact no-ops.** A point a deformer leaves where it is comes back as
//! `None`, never as a position worked out to be the same: settings that do
//! nothing (a twist of 0, a taper of 100% to 100%, an untouched lattice)
//! leave every vertex bit for bit as it was, and so does the part of the
//! shape a deformation does not reach (below the start of a bend anchored
//! at its start, outside an inflated range).
//!
//! **Doubles.** Every deformer works in `f64`, as the gizmo's own drags do:
//! a symmetric deformation of a symmetric model then lands both halves on
//! mirror images of each other to far below what Check Symmetry can see.

mod axial;
pub mod bench;
mod bend;
pub mod lattice;
mod profile;
mod run;
#[cfg(test)]
mod tests;
mod twist;

use glam::DVec3;

pub use lattice::Lattice;
use profile::Profile;
pub use run::{Frame, Target};

/// Which deformer.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Kind {
    #[default]
    Bend,
    BendArc,
    Twist,
    Taper,
    Flatten,
    Inflate,
    Stretch,
    Lattice,
}

impl Kind {
    pub const ALL: [Kind; 8] = [
        Kind::Bend,
        Kind::BendArc,
        Kind::Twist,
        Kind::Taper,
        Kind::Flatten,
        Kind::Inflate,
        Kind::Stretch,
        Kind::Lattice,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Bend => "Bend",
            Kind::BendArc => "Bend Arc",
            Kind::Twist => "Twist",
            Kind::Taper => "Taper",
            Kind::Flatten => "Flatten",
            Kind::Inflate => "Inflate",
            Kind::Stretch => "Stretch",
            Kind::Lattice => "Lattice",
        }
    }

    /// Its icon in `assets/icons`.
    pub fn icon(self) -> &'static str {
        match self {
            Kind::Bend => "deform_bend",
            Kind::BendArc => "deform_arc",
            Kind::Twist => "deform_twist",
            Kind::Taper => "deform_taper",
            Kind::Flatten => "deform_flatten",
            Kind::Inflate => "deform_inflate",
            Kind::Stretch => "deform_stretch",
            Kind::Lattice => "deform_lattice",
        }
    }

    /// What it does to the clay, for its tile's tooltip.
    pub fn tip(self) -> &'static str {
        match self {
            Kind::Bend => "Bends the shape round, along the chosen axis. One end stays where it is.",
            Kind::BendArc => {
                "Bends the shape into a smooth arc that ends where you drag the tip handle: horns, tails, tubes."
            }
            Kind::Twist => "Turns the shape about the axis, more and more along it, like wringing a cloth.",
            Kind::Taper => {
                "Makes the shape thinner or fatter along the axis: one size at the start, another at the end."
            }
            Kind::Flatten => "Squashes the shape along the axis, toward the end that stays put.",
            Kind::Inflate => "Puffs the surface out (or sucks it in) along the way it faces.",
            Kind::Stretch => {
                "Pulls the shape longer or shorter along the axis, thinning or thickening it to keep its bulk."
            }
            Kind::Lattice => "A cage of points round the shape: drag a point and the clay inside follows smoothly.",
        }
    }

    /// Whether it has an end that stays put, chosen by `Params::anchor`.
    pub fn anchors(self) -> bool {
        matches!(self, Kind::Bend | Kind::BendArc | Kind::Twist | Kind::Flatten | Kind::Stretch)
    }
}

/// Which part of the range stays where it is.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Anchor {
    /// The start of the range, and everything before it.
    #[default]
    Start,
    /// The middle: both halves move, mirror images of each other.
    Middle,
    /// The end of the range, and everything after it.
    End,
}

impl Anchor {
    pub const ALL: [Anchor; 3] = [Anchor::Start, Anchor::Middle, Anchor::End];

    pub fn name(self) -> &'static str {
        match self {
            Anchor::Start => "Start",
            Anchor::Middle => "Middle",
            Anchor::End => "End",
        }
    }
}

/// Everything a deformer is set by. One set of values for all of them, so
/// that going from one deformer to another and back keeps what each was
/// left at. Angles are in degrees and sizes are fractions, so the numbers
/// mean the same on a model of any size.
#[derive(Clone, Debug, PartialEq)]
pub struct Params {
    pub kind: Kind,
    /// The gizmo axis the deformer works along: 0, 1, 2 for X, Y, Z.
    pub axis: usize,
    /// Where the range starts and ends, as fractions of the box along the axis.
    pub range: [f32; 2],
    /// How gently the range's ends ease in, 0 (a hard start) to 1.
    pub softness: f32,
    pub anchor: Anchor,
    /// Bend and Twist: degrees over the range.
    pub angle: f32,
    /// Bend: which way round the axis the shape bends, in degrees. 0 is
    /// toward the next axis (Y bends toward Z, Z toward X, X toward Y).
    pub direction: f32,
    /// Taper: the size across at the start and at the end of the range (1 is as it is).
    pub scales: [f32; 2],
    /// Flatten: how far the range is pressed, 0 to 1 (flat).
    pub flatten: f32,
    /// Inflate: how far the surface is pushed, as a fraction of the box's longest side.
    pub inflate: f32,
    /// Stretch: how many times longer the range becomes.
    pub stretch: f32,
    /// Stretch: thin as it lengthens (and thicken as it shortens), keeping its bulk.
    pub keep_volume: bool,
    /// Bend Arc: where the tip handle has been dragged from the end of the
    /// range, in the gizmo's axes, as fractions of the range's length.
    pub tip: [f32; 3],
    pub lattice: Lattice,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            kind: Kind::Bend,
            axis: 1,
            range: [0.0, 1.0],
            softness: 0.3,
            anchor: Anchor::Start,
            angle: 0.0,
            direction: 0.0,
            scales: [1.0, 1.0],
            flatten: 0.0,
            inflate: 0.0,
            stretch: 1.0,
            keep_volume: true,
            tip: [0.0; 3],
            lattice: Lattice::default(),
        }
    }
}

impl Params {
    /// These settings with every number in the range the card offers, so
    /// that nothing a script or a typed number puts in can blow a model up.
    pub fn sanitized(&self) -> Params {
        let finite = |v: f32, or: f32| if v.is_finite() { v } else { or };
        let mut p = self.clone();
        p.axis = p.axis.min(2);
        let (a, b) = (finite(p.range[0], 0.0).clamp(0.0, 1.0), finite(p.range[1], 1.0).clamp(0.0, 1.0));
        p.range = [a.min(b), a.max(b)];
        p.softness = finite(p.softness, 0.3).clamp(0.0, 1.0);
        p.angle = finite(p.angle, 0.0).clamp(-720.0, 720.0);
        p.direction = finite(p.direction, 0.0).clamp(-360.0, 360.0);
        p.scales = p.scales.map(|s| finite(s, 1.0).clamp(0.0, 10.0));
        p.flatten = finite(p.flatten, 0.0).clamp(0.0, 1.0);
        p.inflate = finite(p.inflate, 0.0).clamp(-1.0, 1.0);
        p.stretch = finite(p.stretch, 1.0).clamp(0.05, 20.0);
        p.tip = p.tip.map(|t| finite(t, 0.0).clamp(-20.0, 20.0));
        p.lattice = p.lattice.sanitized();
        p
    }

    /// Whether these settings leave every point where it is, whatever the
    /// range: nothing to apply.
    pub fn is_identity(&self) -> bool {
        match self.kind {
            Kind::Bend | Kind::Twist => self.angle == 0.0,
            Kind::BendArc => self.tip == [0.0; 3],
            Kind::Taper => self.scales == [1.0, 1.0],
            Kind::Flatten => self.flatten == 0.0,
            Kind::Inflate => self.inflate == 0.0,
            Kind::Stretch => self.stretch == 1.0,
            Kind::Lattice => self.lattice.is_rest(),
        }
    }

    /// The same settings with the chosen deformer's own values put back to
    /// doing nothing, the axis, range and the rest kept: what is left on the
    /// card once a deformation has been applied or cancelled.
    pub fn reset_values(&self) -> Params {
        let rest = Params::default();
        let mut p = self.clone();
        match p.kind {
            Kind::Bend | Kind::Twist => p.angle = 0.0,
            Kind::BendArc => p.tip = rest.tip,
            Kind::Taper => p.scales = rest.scales,
            Kind::Flatten => p.flatten = 0.0,
            Kind::Inflate => p.inflate = 0.0,
            Kind::Stretch => p.stretch = 1.0,
            Kind::Lattice => p.lattice.rest(),
        }
        p
    }
}

/// The box the deformed objects fill, in the gizmo's frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub min: DVec3,
    pub max: DVec3,
}

impl Bounds {
    pub fn new(min: DVec3, max: DVec3) -> Bounds {
        Bounds { min, max }
    }

    pub fn size(&self) -> DVec3 {
        (self.max - self.min).max(DVec3::ZERO)
    }

    /// The longest side: what Inflate's push is a fraction of.
    pub fn longest(&self) -> f64 {
        self.size().max_element()
    }

    pub fn centre(&self) -> DVec3 {
        (self.min + self.max) * 0.5
    }

    /// The range along `axis` that `range` (fractions) picks out: its start and end.
    pub fn span(&self, axis: usize, range: [f32; 2]) -> (f64, f64) {
        let (lo, hi) = (self.min[axis], self.max[axis]);
        // Written so that 0 and 1 give the box's own ends to the bit.
        let at = |f: f32| lo * (1.0 - f as f64) + hi * f as f64;
        (at(range[0]), at(range[1]))
    }
}

/// Coordinates along a deformer's axis: `s` along it, and the two across
/// it measured from the middle of the box, in the order the axes follow it
/// (Y is followed by Z then X).
#[derive(Clone, Copy, Debug)]
struct Axes {
    along: usize,
    across: [usize; 2],
    /// The middle of the box across the axis: the line twists and tapers turn about.
    centre: [f64; 2],
}

impl Axes {
    fn new(axis: usize, bounds: &Bounds) -> Axes {
        let across = [(axis + 1) % 3, (axis + 2) % 3];
        let c = bounds.centre();
        Axes { along: axis, across, centre: [c[across[0]], c[across[1]]] }
    }

    /// `(s, u, v)`: along, then across from the middle.
    fn split(&self, q: DVec3) -> (f64, f64, f64) {
        (q[self.along], q[self.across[0]] - self.centre[0], q[self.across[1]] - self.centre[1])
    }

    fn join(&self, s: f64, u: f64, v: f64) -> DVec3 {
        let mut q = DVec3::ZERO;
        q[self.along] = s;
        q[self.across[0]] = u + self.centre[0];
        q[self.across[1]] = v + self.centre[1];
        q
    }
}

/// What a deformer does once it is set up.
enum Shape {
    Bend(bend::Bend),
    Twist(twist::Twist),
    Taper(twist::Taper),
    Axial(axial::Axial),
    Inflate { push: f64 },
    Lattice(lattice::Cage),
}

/// One deformer, set up from its settings and the box, ready to map points.
/// Cheap to make: the app makes one every time a setting or a handle moves.
pub struct Deformer {
    axes: Axes,
    profile: Profile,
    shape: Shape,
}

impl Deformer {
    /// `None` when the settings do nothing (`Params::is_identity`), or the
    /// box has no length along the axis to work over.
    pub fn new(params: &Params, bounds: &Bounds) -> Option<Deformer> {
        let p = params.sanitized();
        if p.is_identity() {
            return None;
        }
        let axes = Axes::new(p.axis, bounds);
        let (lo, hi) = bounds.span(p.axis, p.range);
        if !(hi - lo > bounds.longest() * 1e-9) {
            return None;
        }
        let profile = Profile::new(lo, hi, p.softness as f64);
        let shape = match p.kind {
            Kind::Bend => Shape::Bend(bend::Bend::turned(&p, &profile)),
            Kind::BendArc => Shape::Bend(bend::Bend::to_tip(&p, &axes, &profile)?),
            Kind::Twist => Shape::Twist(twist::Twist::new(&p, &profile)),
            Kind::Taper => Shape::Taper(twist::Taper::new(&p)),
            Kind::Flatten | Kind::Stretch => Shape::Axial(axial::Axial::new(&p, &profile)),
            Kind::Inflate => Shape::Inflate { push: p.inflate as f64 * bounds.longest() },
            Kind::Lattice => Shape::Lattice(lattice::Cage::new(&p.lattice, bounds, p.axis, (lo, hi))?),
        };
        Some(Deformer { axes, profile, shape })
    }

    /// Whether `apply` reads the normal it is given (Inflate alone does):
    /// the app then keeps the normals as they were at the start.
    pub fn needs_normals(&self) -> bool {
        matches!(self.shape, Shape::Inflate { .. })
    }

    /// Where the point `q` (in the gizmo's frame) goes; `None` if it stays
    /// exactly where it is. `normal` is the surface's at `q`, in the same
    /// frame, and is only read by Inflate.
    #[inline]
    pub fn apply(&self, q: DVec3, normal: DVec3) -> Option<DVec3> {
        let axes = &self.axes;
        let (s, u, v) = axes.split(q);
        let (s2, u2, v2) = match &self.shape {
            Shape::Bend(bend) => bend.apply(&self.profile, s, u, v)?,
            Shape::Twist(twist) => twist.apply(&self.profile, s, u, v)?,
            Shape::Taper(taper) => taper.apply(&self.profile, s, u, v)?,
            Shape::Axial(axial) => axial.apply(&self.profile, s, u, v)?,
            Shape::Inflate { push } => {
                let w = self.profile.window(s);
                if w == 0.0 {
                    return None;
                }
                return Some(q + normal * (push * w));
            }
            Shape::Lattice(cage) => return cage.apply(q),
        };
        Some(axes.join(s2, u2, v2))
    }

    /// The stretch of the axis, `[from, to]`, outside which this deformer
    /// leaves a point alone, or `None` if it can reach past both ends: the
    /// app passes over parts of the mesh that lie wholly outside it.
    pub fn reach(&self) -> Option<(f64, f64)> {
        match &self.shape {
            Shape::Inflate { .. } | Shape::Lattice(_) => Some((self.profile.lo, self.profile.hi)),
            _ => None,
        }
    }
}

/// Where the deformer's handles are, in the gizmo's frame: the axis line at
/// the start and end of the range, through the middle of the box.
pub fn range_ends(params: &Params, bounds: &Bounds) -> [DVec3; 2] {
    let p = params.sanitized();
    let axes = Axes::new(p.axis, bounds);
    let (lo, hi) = bounds.span(p.axis, p.range);
    [axes.join(lo, 0.0, 0.0), axes.join(hi, 0.0, 0.0)]
}

/// Where Bend Arc's tip handle is, in the gizmo's frame: the end of the
/// range that moves (the far end from the anchor), plus how far it has been
/// dragged. A point dragged there is what `tip_for` turns back into the
/// setting.
pub fn tip_point(params: &Params, bounds: &Bounds) -> DVec3 {
    let p = params.sanitized();
    let (lo, hi) = bounds.span(p.axis, p.range);
    let [start, end] = range_ends(&p, bounds);
    let rest = if p.anchor == Anchor::End { start } else { end };
    rest + DVec3::from_array(p.tip.map(f64::from)) * (hi - lo)
}

/// The `Params::tip` that puts Bend Arc's tip handle at `point` (in the
/// gizmo's frame).
pub fn tip_for(params: &Params, bounds: &Bounds, point: DVec3) -> [f32; 3] {
    let p = params.sanitized();
    let (lo, hi) = bounds.span(p.axis, p.range);
    let [start, end] = range_ends(&p, bounds);
    let rest = if p.anchor == Anchor::End { start } else { end };
    ((point - rest) / (hi - lo).max(1e-12)).as_vec3().to_array()
}
