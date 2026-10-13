//! One tube while it is still live: its points, its settings, and what was
//! last built from them. A scaffold, not a scene object, until Merge or Keep
//! Separate turns it into geometry.

use glam::{Mat3, Mat4, Vec3};
use sculpt_geom::{SymmetryGroup, SymmetryOp};
use sculpt_toolkit::{Hit, MeshId, ObjectId, Placement, Preview, PreviewData, Ray};

use crate::field::TubeField;
use crate::merge::{CELLS_ACROSS_RADIUS, RESOLVED_TIP_FRACTION};
use crate::types::{ControlPoint, Kind, Sample, TubePoint, TubeSettings};
use crate::{muscle, spine};

/// The preview lattice: this many cells per tube radius.
const PREVIEW_CELLS_ACROSS_RADIUS: f32 = 5.0;
/// A muscle's preview lattice: this many cells across half its depth, and no finer than its tips need
/// down to this fraction of that.
const MUSCLE_CELLS_ACROSS_DEPTH: f32 = 3.5;
const MUSCLE_TIP_FRACTION: f32 = 0.6;
/// Most samples a preview lattice holds; past it the preview coarsens rather
/// than stall a drag. Merge and Keep Separate build finer.
pub const PREVIEW_MAX_SAMPLES: usize = 3_000_000;

/// A tube as a value: for undo snapshots, and for handing tubes back when a
/// merge is undone.
#[derive(Clone, Debug, PartialEq)]
pub struct TubeState {
    /// Names the tube across undo and redo.
    pub id: u64,
    /// The object the tube was drawn on. Its points live in this object's
    /// own space, so the tube moves with it, mirrors with it, and merges
    /// into it. `None` for a tube drawn with nothing to draw on, which lives
    /// in world space and can only be kept separate.
    pub anchor: Option<ObjectId>,
    pub points: Vec<TubePoint>,
    pub settings: TubeSettings,
}

/// Where a tube's anchor stands, and the spaces that gives.
///
/// The tube's frame is the anchor's position and rotation with no scale: a
/// rigid copy of world space in which the anchor's mirror planes are the
/// coordinate planes. Tubes are built there, so they stay round however the
/// anchor is scaled, and merge and mirror cleanly.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub placement: Placement,
}

impl Frame {
    pub const WORLD: Frame = Frame { placement: Placement::IDENTITY };

    pub fn frame_to_world(&self) -> Mat4 {
        Mat4::from_rotation_translation(self.placement.rotation, self.placement.position)
    }

    pub fn local_to_world(&self) -> Mat4 {
        self.placement.matrix()
    }

    pub fn local_to_frame(&self) -> Mat4 {
        Mat4::from_scale(self.placement.scale)
    }

    /// A world point and (possibly zero) world normal as a tube point,
    /// through the inverse of copy `op`, so an edit made on a mirror image
    /// lands on the tube itself.
    pub fn to_tube_point(&self, op: &SymmetryOp, world: Vec3, world_normal: Vec3, size: f32, sharp: bool) -> TubePoint {
        let local_to_world = self.local_to_world();
        let local = local_to_world.inverse().transform_point3(world);
        let mut normal = Vec3::ZERO;
        if world_normal.length_squared() > 1e-12 {
            // Normals come back through the transpose of local-to-world (the
            // inverse of the inverse-transpose they went out through).
            normal = op.apply_inverse(Mat3::from_mat4(local_to_world).transpose() * world_normal).normalize_or_zero();
        }
        TubePoint { position: op.apply_inverse_point(local), surface_normal: normal, facing: Vec3::ZERO, size, sharp }
    }

    /// A world direction as one in the tube's own space, through the inverse of copy `op`: the way a
    /// floating point of a muscle faces.
    pub fn to_tube_direction(&self, op: &SymmetryOp, world: Vec3) -> Vec3 {
        op.apply_inverse(Mat3::from_mat4(self.local_to_world()).transpose() * world).normalize_or_zero()
    }
}

/// The symmetry a tube is repeated under: that of the object it was drawn
/// on, which is the one brushes on that object are under (the Unity app took
/// the anchor's as well, whichever object was selected). The mirror axes and
/// the radial axis are that object's own, through its origin, unless its
/// symmetry is fixed in the world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Symmetry {
    pub mirror: [bool; 3],
    /// How many times round the axis the tube is made, itself included: 1
    /// when radial symmetry is off.
    pub radial_count: u32,
    /// With radial symmetry off this is always Y, so turning the axis of a
    /// symmetry that is off does not look like a change.
    pub radial_axis: Vec3,
    /// Where the anchor stands, when its symmetry is the world's and not its
    /// own: the copies then go through the world's planes and axis, and move
    /// through the tube as the anchor is moved.
    pub world: Option<Placement>,
}

impl Symmetry {
    pub const NONE: Symmetry = Symmetry { mirror: [false; 3], radial_count: 1, radial_axis: Vec3::Y, world: None };

    /// A symmetry of the anchor's own, through its origin.
    pub fn new(mirror: [bool; 3], radial_count: u32, radial_axis: Vec3) -> Symmetry {
        // The count is limited with a chain rather than `if n < MIN { 1 } else { n.min(MAX) }`:
        // rustc 1.99.0 miscompiles that form for unsigned integers, turning 0 into 0.
        let radial_count = if radial_count < SymmetryGroup::MIN_RADIAL_COUNT {
            1
        } else if radial_count > SymmetryGroup::MAX_RADIAL_COUNT {
            SymmetryGroup::MAX_RADIAL_COUNT
        } else {
            radial_count
        };
        let axis = radial_axis.normalize_or_zero();
        let radial_axis = if radial_count > 1 && axis != Vec3::ZERO { axis } else { Vec3::Y };
        Symmetry { mirror, radial_count, radial_axis, world: None }
    }

    /// An object's symmetry as the scene says it, for a tube drawn on that
    /// object.
    pub fn of(object: sculpt_toolkit::Symmetry) -> Symmetry {
        let own = Symmetry::new(object.mirror, object.radial_count, object.radial_axis);
        // No copies, no planes to place: where the anchor stands is then not a change.
        let world = object.world.filter(|_| own.mirror.contains(&true) || own.radial_count > 1);
        Symmetry { world, ..own }
    }

    /// Every copy a tube is made as, the tube itself first, in the anchor's
    /// own space.
    pub fn group(&self) -> SymmetryGroup {
        let Symmetry { mirror, radial_count, radial_axis, world } = *self;
        sculpt_toolkit::Symmetry { mirror, radial_count, radial_axis, world }.group()
    }

    /// The coordinate planes the copies are exactly symmetric across, which
    /// is where the lattice they are meshed on can be laid out to make that
    /// true to the bit (see `TubeField::voxelize`): the mirror planes that are
    /// on. Not when turns come with them and the copies do not close up under
    /// the mirrors, as with a custom radial axis tilted against a mirror
    /// plane: no plane maps those copies onto each other, and forcing one
    /// would add each side's tubes to the other. And not for a symmetry
    /// fixed in the world, whose planes are not the frame's coordinate planes
    /// once the anchor has moved: those copies are as symmetric as rounding
    /// leaves them, as in the Unity app.
    pub fn exact_planes(&self) -> [bool; 3] {
        let turned = self.radial_count > 1;
        let open = turned && self.mirror.contains(&true) && !self.group().is_group();
        if open || self.world.is_some() { [false; 3] } else { self.mirror }
    }
}

/// One symmetric copy of a live tube, as last built.
#[derive(Clone, Debug)]
pub struct TubeCopy {
    pub op: SymmetryOp,
    /// Control points and stations in the tube's frame.
    pub controls: Vec<ControlPoint>,
    pub samples: Vec<Sample>,
    /// World positions of the handles (one per point) and of the stations,
    /// for drawing and picking.
    pub handle_world: Vec<Vec3>,
    pub curve_world: Vec<Vec3>,
}

impl TubeCopy {
    /// Has a curve (two or more distinct points).
    pub fn is_valid(&self) -> bool {
        !self.samples.is_empty()
    }
}

/// What a tube's copies were built from. Blend and placement do not change
/// the shape, so they are not here.
#[derive(Clone, Debug, PartialEq)]
struct Built {
    points: Vec<TubePoint>,
    shape: TubeSettings,
    frame: Frame,
    symmetry: Symmetry,
    ground: Option<Ground>,
}

/// Which mesh a muscle was laid on: its anchor's, and how many vertices it had. A mesh put in the
/// anchor's place (a merge, a remesh) is another one; one that was only reshaped is the same one, and
/// [`LiveTube::lay_again`] is for that.
pub type Ground = (MeshId, usize);

/// The settings with the ones that do not change the shape put aside.
fn shape_of(settings: &TubeSettings) -> TubeSettings {
    TubeSettings { blend: 0.0, on_surface: false, ..*settings }
}

impl Built {
    fn matches(&self, state: &TubeState, frame: Frame, symmetry: Symmetry, ground: Option<Ground>) -> bool {
        self.frame == frame
            && self.symmetry == symmetry
            && self.ground == ground
            && self.shape == shape_of(&state.settings)
            && self.points == state.points
    }
}

/// Where a world ray meets the object a tube was drawn on.
pub type Cast<'a> = &'a dyn Fn(&Ray) -> Option<Hit>;

pub struct LiveTube {
    pub state: TubeState,
    /// One per symmetry operation at the last build; copy 0 is the tube
    /// itself.
    pub copies: Vec<TubeCopy>,
    pub frame: Frame,
    pub symmetry: Symmetry,
    built: Option<Built>,
    /// The tube's surface in world space, as drawn while it is live.
    pub preview: Preview,
    /// The preview shows the copies as they are now.
    preview_current: bool,
}

impl LiveTube {
    pub fn new(id: u64, anchor: Option<ObjectId>, settings: TubeSettings) -> LiveTube {
        LiveTube::from_state(TubeState { id, anchor, points: Vec::new(), settings })
    }

    pub fn from_state(state: TubeState) -> LiveTube {
        LiveTube {
            state,
            copies: Vec::new(),
            frame: Frame::WORLD,
            symmetry: Symmetry::NONE,
            built: None,
            preview: Preview::new(),
            preview_current: false,
        }
    }

    /// Builds every symmetric copy of the tube (in its frame) with their
    /// world-space handles and curves, unless what they were last built from
    /// is unchanged. `symmetry` is the anchor's own: its mirror axes and
    /// radial repeats.
    ///
    /// A muscle is laid on its anchor between its points, which takes `cast` to find the anchor's
    /// surface, and `ground` to say which mesh that surface is. It is laid when it is built, so it
    /// follows its points, its anchor's placement and a new mesh in its anchor's place; after the
    /// anchor has been reshaped it has to be asked ([`Self::lay_again`]).
    pub fn build(&mut self, frame: Frame, symmetry: Symmetry, cast: Option<Cast>, ground: Option<Ground>) {
        let ground = ground.filter(|_| self.state.settings.kind == Kind::Muscle);
        if self.built.as_ref().is_some_and(|b| b.matches(&self.state, frame, symmetry, ground)) {
            return;
        }
        let laid_on = ground;
        let group = symmetry.group();
        let local_to_frame = frame.local_to_frame();
        let frame_to_world = frame.frame_to_world();
        let world_to_frame = frame_to_world.inverse();
        let surface = |at: Vec3, up: Vec3, reach: f32| {
            let ray = Ray::new(frame_to_world.transform_point3(at + up * reach), frame_to_world.transform_vector3(-up));
            let hit = cast.and_then(|cast| cast(&ray)).filter(|hit| hit.distance <= 2.0 * reach)?;
            Some((world_to_frame.transform_point3(hit.position), world_to_frame.transform_vector3(hit.normal)))
        };
        let muscle = self.state.settings.kind == Kind::Muscle;
        self.copies = group
            .iter()
            .map(|op| {
                let (points, settings) = (&self.state.points, &self.state.settings);
                let (controls, samples) = if muscle {
                    let surface: Option<muscle::Ground> = if cast.is_some() { Some(&surface) } else { None };
                    muscle::sample(points, settings, &local_to_frame, op, surface)
                } else {
                    spine::sample(points, settings, &local_to_frame, op)
                };
                let handle_world = controls.iter().map(|c| frame_to_world.transform_point3(c.position)).collect();
                let curve_world = samples.iter().map(|s| frame_to_world.transform_point3(s.center)).collect();
                TubeCopy { op: *op, controls, samples, handle_world, curve_world }
            })
            .collect();
        self.built = Some(Built {
            points: self.state.points.clone(),
            shape: shape_of(&self.state.settings),
            frame,
            symmetry,
            ground: laid_on,
        });
        self.frame = frame;
        self.symmetry = symmetry;
        self.preview_current = false;
    }

    /// The surface under the muscle may have changed shape: lay it on it afresh at the next build.
    /// Nothing to a tube, which only touches the surface at its points.
    pub fn lay_again(&mut self) {
        if self.state.settings.kind == Kind::Muscle && self.state.anchor.is_some() {
            self.built = None;
        }
    }

    /// Every copy's round cones, in the tube's frame.
    pub fn field(&self) -> TubeField {
        let mut field = TubeField::new();
        self.add_to(&mut field);
        field
    }

    pub fn add_to(&self, field: &mut TubeField) {
        for copy in self.copies.iter().filter(|c| c.is_valid()) {
            field.add_tube(&copy.samples, self.state.settings.caps);
        }
    }

    /// Rebuilds the preview if the copies changed since it was made.
    ///
    /// The preview is the tube's field, voxelized: the same surface Merge
    /// and Keep Separate make, only on a coarser lattice, so there is no gap
    /// between what is edited and what comes out (a thick point next to
    /// close neighbours is a ball, a tube crossing itself is one solid). A
    /// tube's own volume is all it costs, a few milliseconds, so the same
    /// lattice serves while a point is being dragged and at rest; and it is
    /// kept per tube, so dragging one point rebuilds only that tube.
    pub fn build_preview(&mut self) {
        if self.preview_current {
            return;
        }
        self.preview_current = true;
        let field = self.field();
        if field.is_empty() {
            self.preview.clear();
            return;
        }
        let settings = &self.state.settings;
        let r = settings.radius;
        let cell = if settings.kind == Kind::Muscle {
            // A muscle is as thin as its depth, not its width: the lattice follows that, so a flat
            // strap is still a few cells through.
            let thin = settings.thin_radius();
            let floor = field.min_radius().max(thin * MUSCLE_TIP_FRACTION);
            (thin / MUSCLE_CELLS_ACROSS_DEPTH).min(floor / CELLS_ACROSS_RADIUS)
        } else {
            let floor = field.min_radius().max(r * RESOLVED_TIP_FRACTION);
            (r / PREVIEW_CELLS_ACROSS_RADIUS).min(floor / CELLS_ACROSS_RADIUS)
        };
        let Ok(built) = field.voxelize(cell, self.symmetry.exact_planes(), PREVIEW_MAX_SAMPLES) else {
            self.preview.clear();
            return;
        };
        let frame_to_world = self.frame.frame_to_world();
        let mut surface = built.surface;
        for (p, n) in surface.positions.iter_mut().zip(&mut surface.normals) {
            *p = frame_to_world.transform_point3(*p);
            *n = frame_to_world.transform_vector3(*n);
        }
        self.preview.set_parts(PreviewData { positions: surface.positions, normals: surface.normals, indices: surface.indices });
    }
}
