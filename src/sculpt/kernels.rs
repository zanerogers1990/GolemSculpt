//! What a single dab does to the mesh, one function per brush.
//!
//! The maths follows the Unity app's brushes (same constants and the same
//! stroke-start anchoring), because each of those choices fixed a visible
//! artefact there. Sums that feed a direction or a plane are accumulated in
//! double precision so mirrored dabs stay mirror images of each other.
//!
//! A dense mesh puts a million vertices under one dab, so nothing here walks
//! the footprint on one thread: sums are taken leaf by leaf and vertices are
//! moved in place through [`Mesh::move_vertices`].

use std::ops::Range;

use glam::{DVec3, Vec3};
use rayon::prelude::*;
use sculpt_geom::SymmetryOp;

use super::Brush;
use super::alpha::{Frame, Stamp};
use super::erase::{self, Erased};
use super::falloff::Falloff;
use super::modes::{FlattenMode, SmoothMode};
use super::share::{Shared, Sharing};
use crate::mesh::{Mesh, Rings, StrokeVertex};
use crate::mesh::bvh::Leaf;
use crate::util::SharedMut;

/// Below this many vertices a dab runs on the calling thread; waking the
/// thread pool costs more than the work.
pub(super) const PAR_MIN: usize = 4096;

/// How many leaves go into each partial total of a sum over a footprint.
const FOLD_RUN: usize = 64;

/// World-distance rates were tuned at this brush radius and scale with it.
const REF_RADIUS: f32 = 0.25;

/// One copy of the brush under symmetry: the original, or its image in one or
/// more of the mirror planes, turned some steps round the radial axis, or
/// both. The list a stroke keeps is built in `symmetry.rs`.
///
/// A mirror image is a per-axis sign flip and is applied as exactly that
/// multiply (see `SymmetryOp`), so mirrored halves stay bit for bit equal.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SymOp(SymmetryOp);

impl Default for SymOp {
    fn default() -> SymOp {
        SymOp::IDENTITY
    }
}

impl SymOp {
    /// The stroke as it was made: the first copy of every list.
    pub const IDENTITY: SymOp = SymOp(SymmetryOp::IDENTITY);

    /// Whether this copy lies across the object's own plane facing `axis`.
    /// Only asked of copies that are plain mirror images across those planes
    /// (see [`Self::is_own_flip`]); a turned copy is on neither side of a
    /// plane, and the planes of a symmetry fixed in the world are asked after
    /// in `symmetry::Mirrors`.
    pub fn flips(self, axis: usize) -> bool {
        self.0.sign()[axis] < 0.0
    }

    /// A mirror image across the object's own planes, through its own
    /// origin: a per-axis sign flip and nothing else.
    pub fn is_own_flip(self) -> bool {
        self.0.diagonal && !self.0.has_offset()
    }

    /// A point of the mesh. A symmetry through the object's origin maps it
    /// as it maps a direction; one fixed in the world also carries a shift,
    /// which only points take (see `SymmetryOp::offset`).
    #[inline]
    pub fn apply_point(self, p: Vec3) -> Vec3 {
        self.0.apply_point(p)
    }

    #[inline]
    pub fn apply_dir(self, d: Vec3) -> Vec3 {
        self.0.apply(d)
    }
}

impl From<SymmetryOp> for SymOp {
    fn from(op: SymmetryOp) -> SymOp {
        SymOp(op)
    }
}

#[derive(Clone, Copy)]
pub struct Params {
    pub brush: Brush,
    pub mask: bool,
    pub sign: f32,
    pub radius: f32,
    pub strength: f32,
    pub accumulate: bool,
    pub build_up: f32,
    /// The brush's focal shift, and its hand-drawn curve if it has one.
    pub falloff: Falloff,
    /// The picture the brush prints, if it has one (see `alpha.rs`).
    pub alpha: Option<Stamp>,
    pub front_facing: bool,
    pub clay_depth: f32,
    pub clay_soft: f32,
    pub crease_pinch: f32,
    pub crease_depth: f32,
    pub flatten_offset: f32,
    pub layer_height: f32,
    /// Inflate: how far the normals it pushes along are evened out with
    /// their neighbours' first (Relax Normals, 0 to 1).
    pub relax_normals: f32,
    /// Clay and Flatten: how steadily the plane they work to follows the
    /// surface from dab to dab (Steady Plane, 0 to 1; see [`Steady`]).
    pub steady_plane: f32,
    /// How much the surface under each dab is smoothed after the dab, the
    /// way that keeps its volume (Auto Smooth, 0 to 1; see [`tidy`]).
    pub auto_smooth: f32,
    /// How much the triangles under each dab are slid within the surface to
    /// even them out after it (Even Out, 0 to 1; see [`tidy`]). Move evens
    /// out once, at the release ([`even_out_grab`]).
    pub even_out: f32,
    /// Snake Hook: how the pulled clay keeps its thickness, -1 to 1: above
    /// nought it is pushed out from the line of the pull as it goes, below
    /// it is drawn in to a point (see [`snake`]).
    pub thickness: f32,
    pub mask_hardness: f32,
    /// Smooth: what it evens out, and how much of the shrinkage it gives
    /// back (Keep Volume, 0 to 1).
    pub smooth_mode: SmoothMode,
    pub keep_volume: f32,
    /// Flatten: which side of its plane it works on.
    pub flatten_mode: FlattenMode,
    /// The distance from the dab before, as a fraction of the radius: the
    /// brush's Spacing, or more when a stroke that has fallen behind is being
    /// laid coarser (`pace.rs`).
    pub spacing: f32,
    /// How much one dab does, against a dab at the spacing the brush's
    /// amounts were tuned at: 1 there, 2 at twice that spacing, where half as
    /// many dabs cover each stretch of the stroke (see `Stroke::dose`). It is
    /// what makes Spacing change how smooth a stroke is and not how much it
    /// builds.
    pub dose: f32,
    /// Auto Smooth and Even Out ([`tidy`]): how many dabs at the tuned
    /// spacing this dab tidies the surface for, nought for a dab that leaves
    /// it to a later one. A stroke tidies every other dab or so, each for
    /// the dabs since the last (`Sculptor::emit`): what they come to does
    /// not depend on how often (as Spacing does not change what a stroke
    /// builds), and it costs half as much. Its own dose, unless a stroke says.
    pub tidy: f32,
    /// Erase Layer: the sculpt layer the stroke rubs out, if there is one
    /// for it to (`erase.rs`).
    pub erase: Option<Erased>,
    /// The Paint brush: what its stroke lays (`paint.rs`). `None` for every
    /// other brush, and for mask painting.
    pub paint: Option<super::paint::PaintDab>,
}

#[derive(Clone, Copy)]
pub struct Dab {
    pub center: Vec3,
    pub normal: Vec3,
    /// Crease: smoothed travel direction (zero on the first dab).
    pub dir: Vec3,
    /// Snake hook: displacement of this step.
    pub step: Vec3,
    /// Clay: previous dab centre, for sweeping the tip between dabs.
    pub prev: Option<Vec3>,
    pub cam: Vec3,
    pub pressure: f32,
    /// Where the brush's alpha lies on the surface, if it has one.
    pub frame: Frame,
    /// Flatten with Lock Plane: the plane the whole stroke levels to (a
    /// point of it and its normal), from the stroke's first dab.
    pub plane: Option<(Vec3, Vec3)>,
    /// Which copy of the stroke's dab this is under symmetry: what a copy
    /// keeps of its own from dab to dab goes by it (Steady Plane).
    pub copy: SymOp,
    /// The way the stroke is going here, as a unit vector (zero for a dab
    /// that has come no way yet): what Even Out lines the triangles up with.
    /// Snake Hook's tip is turned from `dir` to this, so that it turns with
    /// the stroke.
    pub heading: Vec3,
}

impl Dab {
    pub fn mirrored(&self, op: SymOp) -> Dab {
        Dab {
            copy: op,
            center: op.apply_point(self.center),
            normal: op.apply_dir(self.normal),
            dir: op.apply_dir(self.dir),
            step: op.apply_dir(self.step),
            prev: self.prev.map(|p| op.apply_point(p)),
            cam: op.apply_point(self.cam),
            pressure: self.pressure,
            frame: self.frame.mirrored(op),
            plane: self.plane.map(|(origin, normal)| (op.apply_point(origin), op.apply_dir(normal))),
            heading: op.apply_dir(self.heading),
        }
    }
}

impl Params {
    pub(super) fn e(&self, pressure: f32) -> f32 {
        self.strength * pressure
    }

    fn e_off(&self, pressure: f32) -> f32 {
        self.strength * pressure * self.build_up
    }

    /// What an accumulating dab builds by. Pressure counts in full: it once
    /// counted for half here (pressure jitter compounds where every dab
    /// adds), which left the lightest touch of the pen building at more
    /// than half strength. The pen's pressure is smoothed before it gets
    /// here (`input::PenPressure`), which is the cure for jitter.
    fn e_acc(&self, pressure: f32) -> f32 {
        self.strength * pressure * self.build_up
    }

    /// Whether the stroke is one print of the brush's alpha, placed by
    /// dragging (see `alpha::Print`), and not a run of dabs.
    pub fn prints(&self) -> bool {
        // Paint lays its alpha dab by dab; a print dragged out and made again at each new size would only ever
        // add paint, never take back what a smaller print would not have covered.
        !self.mask && self.brush != Brush::Paint && self.alpha.is_some_and(|stamp| stamp.drags())
    }

    /// The weight the brush's alpha gives a vertex `rel` from the centre of
    /// dab `d` (measured where the stroke found it, so the print does not
    /// creep as the surface rises). 1 with no alpha, and a weight times 1 is
    /// that weight to the last bit.
    #[inline]
    pub(super) fn printed(&self, d: &Dab, rel: Vec3) -> f32 {
        match &self.alpha {
            Some(stamp) => stamp.at(&d.frame, rel),
            None => 1.0,
        }
    }

    /// The stroke's settings as one dab of it takes them: the dab's own
    /// radius (Pressure -> Size and Size Jitter make it smaller than the
    /// ring), with the alpha sized to match, and its own spacing and dose.
    /// With the ring's radius this is the stroke's own settings to the bit.
    pub fn for_dab(mut self, radius: f32, spacing: f32, dose: f32) -> Params {
        if radius != self.radius {
            self.radius = radius;
            self.alpha = self.alpha.map(|stamp| stamp.sized(radius));
        }
        (self.spacing, self.dose, self.tidy) = (spacing, dose, dose);
        self
    }

    /// For a brush that seeks: the share `k` of the gap to its target that a
    /// dab at the tuned spacing closes, as a dab with this one's dose closes
    /// it. Closing `k` of what is left, `dose` times over, is what the dabs
    /// it stands in for would have done between them; scaling `k` itself
    /// would overshoot at wide spacing and fall short at tight.
    #[inline]
    pub(super) fn share(&self, k: f32) -> f32 {
        if self.dose == 1.0 { k } else { 1.0 - (1.0 - k).powf(self.dose) }
    }

    /// How copies of a dab that overlap share a vertex between them (see
    /// `share.rs`). A brush seeks when each dab closes a share of the gap to
    /// a place and stops there: a height above the surface the stroke began
    /// on (Layer, and Standard, Inflate and Crease without Accumulate), a
    /// plane (Flatten), the mean of a vertex's neighbours (Smooth). The rest
    /// add a step each dab, Clay
    /// among them with or without Accumulate: its plane rises with the
    /// surface, so it goes on building until its depth for the stroke is
    /// spent.
    pub fn sharing(&self) -> Sharing {
        if self.prints() {
            return Sharing::Seeks;
        }
        match self.brush {
            // Pinch closes a share of the gap to the stroke's line each dab
            // (and spreads away from it by a share of the distance), and
            // Erase Layer a share of the gap to the surface without its layer.
            Brush::Layer | Brush::Flatten | Brush::Smooth | Brush::Pinch | Brush::EraseLayer => Sharing::Seeks,
            Brush::Standard | Brush::Inflate | Brush::Crease if !self.accumulate => Sharing::Seeks,
            // Nudge among them: every dab carries the surface a step.
            _ => Sharing::Adds,
        }
    }

    /// How far from a dab centre vertices can be affected.
    pub fn reach(&self, dab: &Dab) -> f32 {
        match (self.brush, dab.prev) {
            (Brush::Clay, Some(prev)) if !self.mask => self.radius + prev.distance(dab.center),
            _ => self.radius,
        }
    }
}

#[inline]
pub fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
pub fn quintic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * t * (t * (6.0 * t - 15.0) + 10.0)
}

/// The fraction of Layer's radius that is flat-topped at full height. The
/// rest eases down to the edge, so the layer's rim blends instead of stepping.
const LAYER_PLATEAU: f32 = 0.35;

#[inline]
pub(super) fn layer_built_in(t: f32) -> f32 {
    quintic(t / (1.0 - LAYER_PLATEAU))
}

/// Crease's own profile. Smoothstep cubed: as narrow as a cone but with zero
/// slope at both ends, so a stroke is a groove rather than a row of pokes.
#[inline]
pub(super) fn crease_built_in(t: f32) -> f32 {
    smooth(t).powi(3)
}

/// Normalised radius at which a brush's falloff reaches half weight; the
/// cursor draws its inner ring there. Half weight rather than "where the
/// falloff starts": a smoothstep is only at full weight at the very centre,
/// so that circle would be a dot whatever the setting, while the half-weight
/// radius moves with every falloff control there is.
///
/// It walks the same functions the brushes weigh vertices with, so the ring
/// cannot drift from the stroke.
pub fn half_weight_radius(brush: Brush, falloff: &Falloff, clay_soft: f32) -> f32 {
    if brush == Brush::Pose {
        // Pose turns a limb about a joint: its radius only says where the
        // joint is, so there is no half weight to draw a ring at.
        return 1.0;
    }
    let weight = |u: f32| match brush {
        Brush::Crease => falloff.at(u, crease_built_in),
        Brush::Clay => clay_weight(falloff, u, clay_soft),
        Brush::Flatten | Brush::Standard => falloff.at(u, quintic),
        Brush::Layer => falloff.at(u, layer_built_in),
        _ => falloff.at(u, smooth),
    };
    const SAMPLES: usize = 64;
    let (mut prev_u, mut prev_w) = (0.0, weight(0.0));
    if prev_w <= 0.5 {
        return 0.0;
    }
    for i in 1..=SAMPLES {
        let u = i as f32 / SAMPLES as f32;
        let w = weight(u);
        if w <= 0.5 {
            return prev_u + (u - prev_u) * ((0.5 - prev_w) / (w - prev_w)).clamp(0.0, 1.0);
        }
        (prev_u, prev_w) = (u, w);
    }
    1.0
}

/// Clay's own profile: full weight through the middle of the tip, easing to
/// nothing across the outer `soft` of the radius. Quintic rather than a
/// smoothstep: its second derivative is zero at both ends too, and a
/// smoothstep's is not, which reads as a faint crease where the taper meets
/// the flat top, most of all where two dabs' edges overlap.
#[inline]
pub(super) fn clay_falloff(t: f32, soft: f32) -> f32 {
    if t <= 0.0 {
        0.0
    } else if t >= soft {
        1.0
    } else {
        quintic(t / soft.max(0.001))
    }
}

/// Clay's weight at normalised distance `u` from the tip's centre. The edge
/// is tested before the focal shift: past it the shift stops being a bias
/// and would hand back weight.
#[inline]
pub(super) fn clay_weight(falloff: &Falloff, u: f32, soft: f32) -> f32 {
    if u >= 1.0 {
        return 0.0;
    }
    falloff.at(u, |t| clay_falloff(t, soft))
}

/// Soft gate on vertices facing away from the camera. A hard cut-off leaves
/// jagged edges on thin forms, so it ramps over the last 0.2 of the cosine.
#[inline]
pub(super) fn facing(enabled: bool, n: Vec3, p: Vec3, cam: Vec3) -> f32 {
    if !enabled {
        return 1.0;
    }
    let to_cam = cam - p;
    let c = n.dot(to_cam);
    if c <= 0.0 {
        return 0.0;
    }
    let cos = c / to_cam.length().max(1e-12);
    if cos >= 0.2 { 1.0 } else { smooth(cos / 0.2) }
}

fn leaf_vertex_total(mesh: &Mesh, leaves: &[u32]) -> usize {
    leaves
        .iter()
        .map(|&l| {
            let leaf = &mesh.bvh.leaves[l as usize];
            (leaf.vert_end - leaf.vert_start) as usize
        })
        .sum()
}

// Hidden parts of a mesh are out of a brush's reach altogether. The mesh keeps
// brushes from writing to hidden vertices; the passes below, which only read
// (the brush direction, the plane a dab levels to, the weights, what Move
// grabs), leave them out as well, or a hidden bulge beside the brush would
// still tilt the plane it works to. Each asks `Mesh::hidden_vertices` once and
// picks its loop from the answer, so with nothing hidden, which is nearly
// always, the loop is the plain one and the test is never made per vertex.

/// Calls `f` with every vertex `leaf` owns that brushes may read: all of
/// them, or with `hidden` flags, the ones that still show.
#[inline]
fn each_shown(leaf: &Leaf, hidden: Option<&[bool]>, mut f: impl FnMut(u32)) {
    match hidden {
        None => (leaf.vert_start..leaf.vert_end).for_each(f),
        Some(hidden) => {
            for v in leaf.vert_start..leaf.vert_end {
                if !hidden[v as usize] {
                    f(v);
                }
            }
        }
    }
}

/// Runs `f` over every vertex owned by `leaves` that is not hidden, keeping
/// the `Some` results.
pub(super) fn collect<T: Send>(
    mesh: &Mesh,
    leaves: &[u32],
    f: impl Fn(u32) -> Option<T> + Sync + Send,
) -> Vec<T> {
    let hidden = mesh.hidden_vertices();
    if leaf_vertex_total(mesh, leaves) < PAR_MIN {
        let mut out = Vec::new();
        for &l in leaves {
            each_shown(&mesh.bvh.leaves[l as usize], hidden, |v| out.extend(f(v)));
        }
        return out;
    }
    let owned = |l: u32| {
        let leaf = &mesh.bvh.leaves[l as usize];
        leaf.vert_start..leaf.vert_end
    };
    let leaves = leaves.par_iter().with_min_len(8);
    match hidden {
        None => leaves.flat_map_iter(|&l| owned(l).filter_map(&f)).collect(),
        Some(hidden) => leaves
            .flat_map_iter(|&l| owned(l).filter(|&v| !hidden[v as usize]).filter_map(&f))
            .collect(),
    }
}

// Several passes work on a buffer holding one entry per vertex of a list of
// leaves, leaf after leaf. The leaves are always ascending with none repeated
// (which is also the order `Mesh::move_vertices` counts vertices in), so both
// such a buffer and the mesh's own arrays fall into one stretch per leaf, and
// those stretches can be handed to different threads.

pub(super) fn owned(leaf: &Leaf) -> Range<usize> {
    leaf.vert_start as usize..leaf.vert_end as usize
}

/// Where each leaf's vertices start in such a buffer, into `starts` (kept by
/// the caller between dabs), and the buffer's length.
pub(super) fn layout(mesh: &Mesh, leaves: &[u32], starts: &mut Vec<usize>) -> usize {
    debug_assert!(leaves.windows(2).all(|w| w[0] < w[1]));
    starts.clear();
    let mut total = 0;
    for &l in leaves {
        starts.push(total);
        total += owned(&mesh.bvh.leaves[l as usize]).len();
    }
    total
}

/// Runs `f` on each leaf's stretch of `buf`, the leaf at `leaves[k]` from
/// `starts[k]` to the next start (or the end), on all threads once there is
/// enough to share out. The starts must ascend and lie within `buf`.
pub(super) fn each_stretch<T: Send>(buf: &mut [T], starts: &[usize], f: impl Fn(usize, &mut [T]) + Sync) {
    let total = buf.len();
    debug_assert!(starts.windows(2).all(|w| w[0] <= w[1]) && starts.last().is_none_or(|&s| s <= total));
    let shared = SharedMut::new(buf);
    let run = |k: usize| {
        let (start, end) = (starts[k], starts.get(k + 1).copied().unwrap_or(total));
        // SAFETY: the stretches do not overlap, so each is one task's alone.
        f(k, unsafe { shared.slice(start..end) });
    };
    if total < PAR_MIN {
        (0..starts.len()).for_each(run);
    } else {
        (0..starts.len()).into_par_iter().with_min_len(64).for_each(run);
    }
}

/// Fills `out` with `f(place in the buffer, vertex)` for every vertex owned
/// by `leaves`, laid out from `starts` (see [`layout`]), and returns its
/// length. A hidden vertex keeps its place in the buffer and gets the
/// default value (no weight, no target) without `f` being asked.
pub(super) fn fill<T: Copy + Default + Send>(
    mesh: &Mesh,
    leaves: &[u32],
    starts: &mut Vec<usize>,
    out: &mut Vec<T>,
    f: impl Fn(usize, u32) -> T + Sync,
) -> usize {
    let total = layout(mesh, leaves, starts);
    let starts = &*starts;
    out.resize(total, T::default());
    let hidden = mesh.hidden_vertices();
    each_stretch(&mut out[..total], starts, |k, part| {
        let verts = owned(&mesh.bvh.leaves[leaves[k] as usize]);
        let slots = part.iter_mut().zip(verts).enumerate();
        match hidden {
            None => slots.for_each(|(n, (slot, v))| *slot = f(starts[k] + n, v as u32)),
            Some(hidden) => slots.for_each(|(n, (slot, v))| {
                *slot = if hidden[v] { T::default() } else { f(starts[k] + n, v as u32) };
            }),
        }
    });
    total
}

/// Working buffers a stroke keeps between dabs. A footprint on a dense mesh
/// is megabytes, and memory that size costs more to get fresh from the system
/// for every dab than it does to fill.
#[derive(Default)]
pub struct Scratch {
    weights: Vec<f32>,
    pub(super) targets: Vec<Vec3>,
    /// Where a Smooth dab found its vertices, for Keep Volume.
    before: Vec<Vec3>,
    /// Where each leaf of the dab in hand starts in those (see [`layout`]).
    pub(super) starts: Vec<usize>,
    /// The leaves under the dabs of a frame, for Clay's finishing pass.
    frame_leaves: Vec<u32>,
    /// For the walk over mirror images that can meet
    /// (`Sculptor::apply_symmetric`): what they have done to each vertex
    /// between them.
    pub(super) shared: Vec<Shared>,
    /// What a dab of the Paint brush fills (`paint.rs`).
    pub(super) paint: super::paint::Buffers,
    /// Each vertex's normal as the stroke found the surface, by vertex, once
    /// a dab has needed it (zero until then): what Layer and Inflate raise
    /// along, and what Standard's and Flatten's directions are taken from.
    start_normals: Vec<[f32; 3]>,
    /// Inflate without Accumulate: those normals evened out with their
    /// neighbours' by Relax Normals, kept the same way.
    relaxed_normals: Vec<[f32; 3]>,
    /// Which leaves have all their vertices in those two (see
    /// [`prepare_start_normals`]).
    prepared: Vec<(u32, u32)>,
    relaxed_prepared: Vec<(u32, u32)>,
    /// Steady Plane: each copy of the brush's planes so far (see [`Steady`]).
    steady: Vec<(SymOp, Steady)>,
    /// What a pass of the smoothing that keeps volume, or of Even Out, has
    /// worked out for each vertex of the dab's leaves (see [`relax_step`]).
    tidy: Vec<Tidy>,
    /// That smoothing's first half, by vertex: how far each vertex's
    /// neighbours' middle is from where it is held to (Vollmer's `b`). Zero
    /// outside the pass in hand; each pass clears what it wrote.
    held_off: Vec<[f32; 3]>,
}

impl Scratch {
    /// The stroke's start normals, for a plane the stroke takes from its
    /// first dab (Lock Plane).
    pub(super) fn start_normals(&mut self) -> &mut Vec<[f32; 3]> {
        &mut self.start_normals
    }
}

/// Makes `kept`, a slot per vertex with zero for "not worked out yet", long
/// enough for every vertex of `mesh`, those dynamic topology has added
/// since too. Zeroed by the system the first time, so a big mesh pays only
/// for the pages a stroke reaches.
fn per_vertex(kept: &mut Vec<[f32; 3]>, mesh: &Mesh) {
    let n = mesh.vertex_count();
    if kept.is_empty() {
        *kept = vec![[0.0; 3]; n];
    } else if kept.len() < n {
        kept.resize(n, [0.0; 3]);
    }
}

/// Vertex `v`'s normal as the stroke found the surface
/// ([`Mesh::stroke_start_normal`]): out of `kept`, or worked out and kept
/// there. It cannot change while the stroke lasts, so it is worked out once
/// a stroke, the first time a dab needs it.
///
/// # Safety
/// No other thread may read or write `v`'s slot of `kept` while this runs:
/// the caller works on the vertices of its own leaves only.
#[inline]
unsafe fn start_normal(mesh: &Mesh, kept: &SharedMut<'_, [f32; 3]>, v: u32) -> Vec3 {
    // SAFETY: as the caller promises.
    let known = Vec3::from_array(unsafe { kept.read(v as usize) });
    if known != Vec3::ZERO {
        return known;
    }
    let n = mesh.stroke_start_normal(v);
    unsafe { kept.write(v as usize, n.to_array()) };
    n
}

/// Totals `add` over every vertex owned by `leaves` that is not hidden. The
/// leaves are totalled in fixed runs and the runs merged in order, so the
/// answer does not depend on how the work was shared between threads.
fn fold_leaves<T: Default + Send>(
    mesh: &Mesh,
    leaves: &[u32],
    add: impl Fn(&mut T, u32) + Sync,
    merge: impl Fn(&mut T, T),
) -> T {
    let hidden = mesh.hidden_vertices();
    let run_total = |run: &[u32]| {
        let mut total = T::default();
        for &l in run {
            each_shown(&mesh.bvh.leaves[l as usize], hidden, |v| add(&mut total, v));
        }
        total
    };
    let mut out = T::default();
    if leaf_vertex_total(mesh, leaves) < PAR_MIN {
        for run in leaves.chunks(FOLD_RUN) {
            merge(&mut out, run_total(run));
        }
    } else {
        let totals: Vec<T> = leaves.par_chunks(FOLD_RUN).map(run_total).collect();
        for total in totals {
            merge(&mut out, total);
        }
    }
    out
}

/// Weighted average normal under the brush (linear weights). Used as the
/// brush direction where a single facet normal would be too jumpy.
pub fn footprint_normal(mesh: &Mesh, center: Vec3, radius: f32, from_start: bool) -> Option<Vec3> {
    let mut leaves = Vec::new();
    mesh.bvh.leaves_in_sphere(center, radius, &mut leaves);
    let r2 = radius * radius;
    let sum = fold_leaves(
        mesh,
        &leaves,
        |sum: &mut DVec3, v| {
            let (p, n) = if from_start {
                (mesh.start_pos(v), mesh.start_nrm(v))
            } else {
                (mesh.positions[v as usize], mesh.normals[v as usize])
            };
            let d2 = p.distance_squared(center);
            if d2 < r2 {
                *sum += n.as_dvec3() * (1.0 - d2.sqrt() / radius) as f64;
            }
        },
        |sum, part| *sum += part,
    );
    (sum.length_squared() > 1e-24).then(|| sum.normalize().as_vec3())
}

/// The way the surface faced under a dab as the stroke found it: the normals
/// of the vertices whose stroke-start places are within `radius` of
/// `center`, each as the stroke found it ([`Mesh::stroke_start_normal`],
/// kept in `kept`), by linear weights from those places. Measured on what
/// the stroke started from, it does not lean as the surface under it rises,
/// and a vertex and its mirror twin count alike however late the stroke
/// reached either.
fn start_footprint_normal(mesh: &Mesh, center: Vec3, radius: f32, kept: &mut Vec<[f32; 3]>) -> Option<Vec3> {
    let mut leaves = Vec::new();
    mesh.bvh.leaves_in_sphere(center, radius, &mut leaves);
    leaves.sort_unstable();
    leaves.dedup();
    per_vertex(kept, mesh);
    let kept = SharedMut::new(kept);
    let r2 = radius * radius;
    let sum = fold_leaves(
        mesh,
        &leaves,
        |sum: &mut DVec3, v| {
            let d2 = mesh.start_pos(v).distance_squared(center);
            if d2 < r2 {
                // SAFETY: each leaf is totalled by one task, and owns its vertices.
                let n = unsafe { start_normal(mesh, &kept, v) };
                *sum += n.as_dvec3() * (1.0 - d2.sqrt() / radius) as f64;
            }
        },
        |sum, part| *sum += part,
    );
    (sum.length_squared() > 1e-24).then(|| sum.normalize().as_vec3())
}

/// Applies one dab of the stroke's brush to the vertices in `leaves`.
///
/// `fit` is `leaves` or more: the leaves that the sums a dab takes over its
/// footprint (the plane Clay and Flatten work to, the cap Nudge slides along)
/// are taken over. Those are totalled in runs of leaves (`fold_leaves`), so
/// the list they go over decides how the double-precision additions are
/// grouped. The copies of a dab near a mirror plane each move only the leaves
/// they reach, and take their sums over every leaf the copies reach between
/// them, as they always did, so that no sum comes out a rounding different.
pub fn apply(mesh: &mut Mesh, p: &Params, d: &Dab, leaves: &[u32], fit: &[u32], scratch: &mut Scratch) {
    if p.mask {
        return mask_paint(mesh, p, d, leaves);
    }
    if p.brush == Brush::Paint {
        // One copy of a Paint or Erase dab; it moves nothing. Blur and Smear lay every copy at once (paint.rs).
        return super::paint::lay(mesh, p, d, leaves, scratch);
    }
    if p.brush == Brush::EraseLayer {
        // Before the print below, which raises the surface: this brush's own print takes away.
        return erase::dab(mesh, p, d, leaves, scratch);
    }
    if p.prints() {
        return print(mesh, p, d, leaves);
    }
    match p.brush {
        Brush::Inflate => inflate(mesh, p, d, leaves, scratch),
        Brush::Standard => standard(mesh, p, d, leaves, scratch),
        Brush::Layer => layer(mesh, p, d, leaves, scratch),
        Brush::Flatten => flatten(mesh, p, d, leaves, fit, scratch),
        Brush::Clay => clay(mesh, p, d, leaves, fit, scratch),
        Brush::Crease => crease(mesh, p, d, leaves),
        Brush::Smooth => smooth_dab(mesh, p, d, leaves, scratch),
        Brush::SnakeHook => snake(mesh, p, d, leaves),
        Brush::Pinch => pinch(mesh, p, d, leaves),
        Brush::Nudge => nudge(mesh, p, d, leaves, fit),
        // Move and Pose take hold once at the press and are dragged, not
        // dabbed. Erase Layer and Paint were seen to above.
        Brush::Move | Brush::Pose | Brush::EraseLayer | Brush::Paint => return,
    }
    // Smooth is smoothing already. A brush with neither at nought is left
    // exactly as it was: nothing more is read or written.
    if p.brush != Brush::Smooth && p.tidy > 0.0 && (p.auto_smooth > 0.0 || p.even_out > 0.0) {
        tidy(mesh, p, d, leaves, scratch);
    }
}

/// How tall a stroke of Standard without Accumulate stands at full Strength
/// and full pressure, as a share of the brush's radius. One pass comes to
/// about nine tenths of it ([`SEEK_RATE`]): a third of the radius, a tenth
/// at half Strength, as Accumulate built in one pass at those Strengths.
const STANDARD_HEIGHT: f32 = 0.35;

/// The same for Inflate: a pass at full Strength swells the surface by a
/// little over a quarter of the radius, as Accumulate did.
const INFLATE_HEIGHT: f32 = 0.33;

/// For Standard and Inflate without Accumulate: the share of the way to
/// what it asks for that a dab at the tuned spacing takes a vertex, at
/// Build-Up 1. About five dabs cover each point of a stroke at the brush's
/// own spacing, so one pass takes the middle of the stroke about nine
/// tenths of the way, and a second most of the rest.
const SEEK_RATE: f32 = 0.35;

/// Inflate brings the normals within this many radii of the dab up to date
/// before it pushes along them: the ring, and the neighbours just outside it
/// that Relax Normals reads.
const REFRESH_REACH: f32 = 1.25;

/// Where a vertex at `pos` goes under one dab of a brush that raises the
/// surface to a height for the stroke and no further (Standard and Inflate
/// without Accumulate): along `n`, `height` being the full height (signed),
/// `cap` the share of it this dab asks for there (its weight there times
/// the strength the pen gives it) and `k` the share of the way it goes.
///
/// How far it has risen is read back off the vertex, from `start`, where the
/// stroke found it (as Layer reads its share). A dab takes it `k` of the way
/// to what the dab asks for, aiming a little past it so the last of the way
/// does not take for ever (Blender's 1.05), and never beyond it: what one
/// dab asks for is the most the stroke gets there. So a stroke's
/// cross-section is the brush's own falloff, a rounded bead, however often
/// the stroke crosses itself; a second pass in the same stroke closes what
/// is left of the way and stops; and a new stroke starts from the surface
/// this one left and builds on top of it.
#[inline]
fn raised(pos: Vec3, start: Vec3, n: Vec3, height: f32, cap: f32, k: f32) -> Option<Vec3> {
    let risen = (pos - start).dot(n) / height;
    if risen >= cap {
        return None;
    }
    let next = (risen + k * (LAYER_OVERSHOOT * cap - risen)).min(cap);
    Some(pos + n * (height * (next - risen)))
}

/// The share of its full height a dab of `kernel` asks for under the middle
/// of the ring: its Strength as the slider maps it (`strength.rs`), times the
/// pen's pressure, out of what the slider's top gives.
#[inline]
fn seek_full(p: &Params, d: &Dab, kernel: Brush) -> f32 {
    let top = super::strength::gain(kernel).unwrap_or(1.0);
    (p.e(d.pressure) / top).clamp(0.0, 1.0)
}

/// The share of the way a dab of Standard or Inflate without Accumulate
/// takes a vertex: Build-Up is how quickly the stroke comes to its height.
#[inline]
fn seek_rate(p: &Params) -> f32 {
    p.share((SEEK_RATE * p.build_up).clamp(0.0, 1.0))
}

/// Vertex `v`'s normal, as `normal` gives each vertex's, evened out with its
/// neighbours' by `relax`: that share of the way from its own to their
/// mean. Summed in double (see the module docs).
#[inline]
fn relaxed(mesh: &Mesh, v: u32, relax: f32, normal: impl Fn(u32) -> Vec3) -> Vec3 {
    let own = normal(v);
    let ring = mesh.neighbours(v);
    if relax <= 0.0 || ring.is_empty() {
        return own;
    }
    let mut sum = DVec3::ZERO;
    for &o in ring {
        sum += normal(o).as_dvec3();
    }
    let mean = (sum / ring.len() as f64).as_vec3();
    own.lerp(mean, relax).normalize_or(own)
}

/// Inflate's weight for a vertex: the falloff from where the stroke found
/// it, the mask, Front Facing and the alpha.
#[inline]
fn inflate_weight(p: &Params, d: &Dab, vt: &StrokeVertex) -> f32 {
    let r = p.radius;
    let d2 = vt.start.distance_squared(d.center);
    if d2 >= r * r {
        return 0.0;
    }
    p.falloff.at(d2.sqrt() / r, smooth)
        * (1.0 - vt.mask)
        * facing(p.front_facing, vt.normal, vt.pos, d.cam)
        * p.printed(d, vt.start - d.center)
}

/// Inflate: swells the surface out along its own normals.
///
/// Without Accumulate (as it starts) each vertex rises along its own normal
/// as the stroke found it, evened out with its neighbours' by Relax Normals,
/// to a height for the stroke and no further, as Standard does
/// ([`raised`]). (It once drew every vertex toward one point above the dab,
/// which gathered the surface in under the brush.)
///
/// With Accumulate every dab pushes each vertex a step along its normal as
/// the surface is now. The normals under the dab are brought up to date
/// first ([`Mesh::refresh_normals_within`]): left for the frame to do, a
/// push went the way the surface faced several dabs before, and a strong
/// stroke split the surface into spikes. They are evened out by Relax
/// Normals too (Nomad's Relax (Normal)), so neighbours do not push apart.
///
/// Either way a vertex is weighed by where the stroke found it, so the
/// footprint stays where it was laid as the surface swells.
fn inflate(mesh: &mut Mesh, p: &Params, d: &Dab, leaves: &[u32], scratch: &mut Scratch) {
    let r = p.radius;
    let relax = p.relax_normals.clamp(0.0, 1.0);
    if p.accumulate {
        mesh.refresh_normals_within(leaves, d.center, r * REFRESH_REACH);
        let rate = p.sign * p.e_acc(d.pressure) * 1.25 * 0.1 * (r / REF_RADIUS) * p.dose;
        if relax <= 0.0 {
            return mesh.move_vertices(leaves, |_, vt| {
                let w = inflate_weight(p, d, vt);
                (w > 0.0).then(|| vt.pos + vt.normal * (w * rate))
            });
        }
        {
            let mesh = &*mesh;
            let r2 = r * r;
            fill(mesh, leaves, &mut scratch.starts, &mut scratch.targets, |_, v| {
                if mesh.start_pos(v).distance_squared(d.center) >= r2 {
                    return Vec3::ZERO;
                }
                relaxed(mesh, v, relax, |o| mesh.normals[o as usize])
            });
        }
        let normals = &scratch.targets;
        return mesh.move_vertices(leaves, |at, vt| {
            let w = inflate_weight(p, d, vt);
            (w > 0.0).then(|| vt.pos + normals[at] * (w * rate))
        });
    }
    // Each vertex's normal as the stroke found the surface, evened out with
    // its neighbours' by Relax Normals: worked out once a stroke, leaf by
    // leaf the first time a dab reaches the leaf, and kept.
    let Scratch { start_normals, prepared, relaxed_normals, relaxed_prepared, .. } = scratch;
    prepare_start_normals(mesh, leaves, start_normals, prepared);
    let normals = if relax > 0.0 {
        prepare_relaxed_normals(mesh, leaves, relax, start_normals, relaxed_normals, relaxed_prepared);
        &*relaxed_normals
    } else {
        &*start_normals
    };
    let height = p.sign * r * INFLATE_HEIGHT;
    let (full, k) = (seek_full(p, d, Brush::Inflate), seek_rate(p));
    // No vertex is further from where the stroke found it than the height.
    let near = r + height.abs();
    mesh.move_within(leaves, d.center, near * near, |_, vt| {
        let w = inflate_weight(p, d, vt);
        if w <= 0.0 {
            return None;
        }
        raised(vt.pos, vt.start, Vec3::from_array(normals[vt.index as usize]), height, w * full, k)
    });
}

/// Which leaves have had every vertex's start normal (or relaxed start
/// normal) worked out this stroke: the run of vertices each had then, by
/// leaf. A leaf whose run has changed since (dynamic topology has given it
/// vertices) is gone over again, and only what is missing is worked out.
fn unprepared(mesh: &Mesh, leaves: &[u32], prepared: &mut Vec<(u32, u32)>) -> Vec<u32> {
    if prepared.len() < mesh.bvh.leaves.len() {
        prepared.resize(mesh.bvh.leaves.len(), (0, 0));
    }
    leaves
        .iter()
        .copied()
        .filter(|&l| {
            let leaf = &mesh.bvh.leaves[l as usize];
            let run = (leaf.vert_start, leaf.vert_end);
            run.0 != run.1 && std::mem::replace(&mut prepared[l as usize], run) != run
        })
        .collect()
}

/// Runs `f` on every leaf of `leaves`, on all threads once there are enough.
fn each_leaf(mesh: &Mesh, leaves: &[u32], f: impl Fn(&Leaf) + Sync) {
    if leaf_vertex_total(mesh, leaves) < PAR_MIN {
        leaves.iter().for_each(|&l| f(&mesh.bvh.leaves[l as usize]));
    } else {
        leaves.par_iter().with_min_len(4).for_each(|&l| f(&mesh.bvh.leaves[l as usize]));
    }
}

/// Works out the stroke-start normal of every vertex of `leaves` that does
/// not have one in `kept` yet ([`start_normal`]), a leaf at a time, each
/// leaf once a stroke (`prepared`). A brush that raises along these then
/// reads them by vertex, with no pass of its own over the footprint each dab.
fn prepare_start_normals(mesh: &Mesh, leaves: &[u32], kept: &mut Vec<[f32; 3]>, prepared: &mut Vec<(u32, u32)>) {
    let fresh = unprepared(mesh, leaves, prepared);
    if fresh.is_empty() {
        return;
    }
    per_vertex(kept, mesh);
    let kept = SharedMut::new(kept);
    each_leaf(mesh, &fresh, |leaf| {
        for v in leaf.vert_start..leaf.vert_end {
            // SAFETY: each leaf is one task's, and owns its vertices.
            unsafe { start_normal(mesh, &kept, v) };
        }
    });
}

/// [`prepare_start_normals`] for the start normals evened out by Relax
/// Normals (`relax`), from `kept`, into `evened`. A neighbour in a leaf not
/// prepared has its start normal worked out where it is needed, and not
/// kept: its own leaf is another task's.
fn prepare_relaxed_normals(
    mesh: &Mesh,
    leaves: &[u32],
    relax: f32,
    kept: &[[f32; 3]],
    evened: &mut Vec<[f32; 3]>,
    prepared: &mut Vec<(u32, u32)>,
) {
    let fresh = unprepared(mesh, leaves, prepared);
    if fresh.is_empty() {
        return;
    }
    per_vertex(evened, mesh);
    let evened = SharedMut::new(evened);
    let start = |o: u32| {
        let known = kept.get(o as usize).map_or(Vec3::ZERO, |&n| Vec3::from_array(n));
        if known != Vec3::ZERO { known } else { mesh.stroke_start_normal(o) }
    };
    each_leaf(mesh, &fresh, |leaf| {
        for v in leaf.vert_start..leaf.vert_end {
            // SAFETY: as in `prepare_start_normals`.
            if Vec3::from_array(unsafe { evened.read(v as usize) }) == Vec3::ZERO {
                unsafe { evened.write(v as usize, relaxed(mesh, v, relax, start).to_array()) };
            }
        }
    });
}
/// Standard: raises a soft, rounded stroke.
///
/// Every vertex is weighed by where the stroke found it, and each dab has
/// one direction: the normals under it as the stroke found the surface,
/// averaged ([`start_footprint_normal`]). Weighed where they are now, the
/// vertices a dab raised left its ring as they rose, the next dab found the
/// bump's flank instead, and passes over the same place peaked into a
/// ridge.
///
/// Without Accumulate (as it starts) a stroke raises the surface to a height
/// and no further ([`raised`]): Strength and the pen's pressure say how
/// tall, Build-Up how quickly the stroke gets there. With it every dab adds
/// a step, as Standard always did.
fn standard(mesh: &mut Mesh, p: &Params, d: &Dab, leaves: &[u32], scratch: &mut Scratch) {
    let r = p.radius;
    let r2 = r * r;
    let dir = start_footprint_normal(mesh, d.center, r, &mut scratch.start_normals).unwrap_or(d.normal);
    let weight = |vt: &StrokeVertex| {
        let d2 = vt.start.distance_squared(d.center);
        if d2 >= r2 {
            return 0.0;
        }
        p.falloff.at(d2.sqrt() / r, quintic)
            * (1.0 - vt.mask)
            * facing(p.front_facing, vt.normal, vt.pos, d.cam)
            * p.printed(d, vt.start - d.center)
    };
    if p.accumulate {
        let rate = p.sign * p.e_acc(d.pressure) * 1.25 * 0.1 * (r / REF_RADIUS) * p.dose;
        return mesh.move_vertices(leaves, |_, vt| {
            let w = weight(vt);
            (w > 0.0).then(|| vt.pos + dir * (rate * w))
        });
    }
    let height = p.sign * r * STANDARD_HEIGHT;
    let (full, k) = (seek_full(p, d, Brush::Standard), seek_rate(p));
    // No vertex is further from where the stroke found it than the height.
    let near = r + height.abs();
    mesh.move_within(leaves, d.center, near * near, |_, vt| {
        let w = weight(vt);
        if w <= 0.0 {
            return None;
        }
        raised(vt.pos, vt.start, dir, height, w * full, k)
    });
}
/// A print of the brush's alpha placed by dragging: every vertex under it
/// is set to its full height above where the stroke found it, in one dab,
/// whichever brush is in hand. The dab is made afresh, on the surface as the
/// stroke found it, each time the drag resizes the print (`alpha::Print`).
fn print(mesh: &mut Mesh, p: &Params, d: &Dab, leaves: &[u32]) {
    let r = p.radius;
    let r2 = r * r;
    let dir = footprint_normal(mesh, d.center, r, true).unwrap_or(d.normal);
    // At full Strength a print stands half as tall as its radius; taller
    // than that is a spike, not a print.
    let height = p.sign * r * p.strength * 0.5;
    mesh.move_vertices(leaves, |_, vt| {
        let start = vt.start;
        let d2 = start.distance_squared(d.center);
        if d2 >= r2 {
            return None;
        }
        let w = p.falloff.at(d2.sqrt() / r, quintic)
            * (1.0 - vt.mask)
            * facing(p.front_facing, vt.normal, start, d.cam)
            * p.printed(d, start - d.center);
        (w > 0.0).then(|| start + dir * (height * w))
    });
}

/// How much past the full height a Layer vertex's share of it aims each dab
/// (Blender's 1.05): aiming at exactly the height, the last of the way
/// would take for ever.
const LAYER_OVERSHOOT: f32 = 1.05;

/// Layer: raises an even step of the Layer Height above the surface the
/// stroke began on, easing up to it (Blender's Layer brush).
///
/// Each vertex has its share of the height, 0 where the stroke found it and
/// 1 at the full height, measured along its own normal as the stroke found
/// it. Every dab adds to that share by the dab's weight there times the
/// brush's strength, less the nearer it already is to the full height, and
/// sets the vertex at that much of the height. (Blender also moves it only
/// the weight's share of the way there, keeping the share apart; read back
/// off the vertex, that would count the weight twice.) So a
/// light pass raises a low step and a firm one or a second pass raises it
/// to the full height and no further, however often the stroke crosses
/// itself. A mask holds the share down as well as the weight: a vertex
/// masked half way rises half way.
///
/// The share is read back off where the vertex is, not kept apart: what
/// the mirror copies of a dab do to a vertex is then shared out as for any
/// brush that seeks a height (`share.rs`), and a vertex dynamic topology
/// has just made starts from where it was made.
fn layer(mesh: &mut Mesh, p: &Params, d: &Dab, leaves: &[u32], scratch: &mut Scratch) {
    let r = p.radius;
    let r2 = r * r;
    let height = r * p.layer_height;
    let sign = p.sign;
    let grow = p.e_off(d.pressure).max(0.0);
    // Each vertex's normal as the stroke found the surface: worked out once
    // a stroke (it cannot change while the stroke lasts), for a leaf's
    // vertices the first time a dab reaches the leaf, and read by vertex. A
    // vertex dynamic topology adds comes after every one there was, and is
    // worked out when its leaf is next reached.
    let Scratch { start_normals, prepared, .. } = scratch;
    prepare_start_normals(mesh, leaves, start_normals, prepared);
    let normals = &*start_normals;
    // A vertex is never further from where the stroke found it than the
    // height: what is further than that from the dab is out of its reach,
    // and is passed over before the rest of it is read.
    let near = r + height;
    mesh.move_within(leaves, d.center, near * near, |_, vt| {
        let start = vt.start;
        // Weighed where the stroke found the surface: the footprint stays
        // where it was laid as the surface rises.
        let d2 = start.distance_squared(d.center);
        if d2 >= r2 {
            return None;
        }
        // Flat plateau in the middle so overlapping dabs build one even step.
        let free = 1.0 - vt.mask;
        let w = p.falloff.at(d2.sqrt() / r, layer_built_in)
            * free
            * facing(p.front_facing, vt.normal, vt.pos, d.cam)
            * p.printed(d, start - d.center);
        if w <= 0.0 {
            return None;
        }
        let n = Vec3::from_array(normals[vt.index as usize]);
        let share = (vt.pos - start).dot(n) * sign / height;
        if share >= free {
            return None;
        }
        let share = share.max(-free);
        // A dab at another spacing closes the gap as the dabs it stands in
        // for would have between them (`Params::share`).
        let next = (share + p.share((w * grow).clamp(0.0, 1.0)) * (LAYER_OVERSHOOT - share.abs())).min(free);
        Some(start + n * (sign * height * next))
    });
}

/// Steady Plane: the plane Clay and Flatten work to, steadied along the
/// stroke for one copy of the brush, so that it does not rock with every
/// bump the ring passes over (after Blender's `calc_stabilized_plane`).
///
/// Each dab's own plane is drawn toward the one the dab before worked to,
/// by Steady Plane's share: its normal turned that far toward that one's,
/// and its height over the dab's centre moved that far toward that one's.
/// So the plane worked to is a running average of the planes the stroke has
/// found, the nearer ones counting for most.
///
/// Two things differ from Blender's, each for what `--brush-feel` showed.
/// Blender also averages the last so many dabs' planes, which a dab further
/// apart makes a longer stretch of the stroke: at half and twice the
/// spacing Flatten then did a quarter more and a quarter less. Here a dab
/// at another spacing is drawn by the share the dabs it stands in for would
/// have been in turn, and there is nothing else to it. And Blender steadies
/// the plane's place in space, drawing each centre on to the plane before,
/// which on a rounded form holds the plane up off a surface that curves away
/// from it: Fill raised a ball that had nothing to fill. Here it is how high
/// the plane stands over the dab's own centre that is steadied, which does
/// not change as a stroke goes over a smooth form and does over a bump.
///
/// A plane that follows the surface with any lag tilts away from a rounded
/// form, and Flatten cuts or fills a little more for it there (Trim a
/// seventh deeper across a ball five times the brush's size, at the 50% it
/// starts with): the price of not rocking.
///
/// Every copy of the brush under symmetry keeps its own: each works on its
/// own part of the model, and a copy and its mirror image make the same sums
/// of mirror images, so their planes stay mirror images.
pub(super) struct Steady {
    /// The plane the dab before worked to: its normal, and how high it stood
    /// over that dab's centre.
    last: (Vec3, f32),
}

impl Steady {
    /// The normal and height of the plane to work to, for a dab whose own
    /// plane has normal `n` and stands `height` over the dab's centre.
    /// `amount` is Steady Plane, and `dose` the dab's.
    fn plane(&mut self, amount: f32, dose: f32, n: Vec3, height: f32) -> (Vec3, f32) {
        let pull = amount.powf(dose.max(1e-3));
        let (last_n, last_h) = self.last;
        self.last = (n.lerp(last_n, pull).normalize_or(n), height + (last_h - height) * pull);
        self.last
    }
}
/// The plane dab `d` works to, its own being (`origin`, `n`): steadied by
/// the brush's Steady Plane, if it has any, over the planes that copy of
/// the brush worked to before ([`Steady`]).
fn steadied(scratch: &mut Scratch, p: &Params, d: &Dab, origin: Vec3, n: Vec3) -> (Vec3, Vec3) {
    if p.steady_plane <= 0.0 {
        return (origin, n);
    }
    let amount = p.steady_plane.min(1.0);
    let height = (origin - d.center).dot(n);
    let (normal, height) = match scratch.steady.iter_mut().find(|(copy, _)| *copy == d.copy) {
        Some((_, steady)) => steady.plane(amount, p.dose, n, height),
        None => {
            scratch.steady.push((d.copy, Steady { last: (n, height) }));
            (n, height)
        }
    };
    (d.center + normal * height, normal)
}
/// Running totals for a weighted plane through a footprint.
#[derive(Default)]
struct PlaneSum {
    origin: DVec3,
    normal: DVec3,
    weight: f64,
}

impl PlaneSum {
    #[inline]
    fn add(&mut self, pos: Vec3, normal: Vec3, w: f32) {
        let w = w as f64;
        self.origin += pos.as_dvec3() * w;
        self.normal += normal.as_dvec3() * w;
        self.weight += w;
    }

    fn merge(&mut self, other: PlaneSum) {
        self.origin += other.origin;
        self.normal += other.normal;
        self.weight += other.weight;
    }

    /// The plane as (origin, normal), or `None` if nothing carried weight.
    fn plane(&self, fallback_normal: Vec3) -> Option<(Vec3, Vec3)> {
        if self.weight <= 1e-12 {
            return None;
        }
        let normal = if self.normal.length_squared() > 1e-24 {
            self.normal.normalize().as_vec3()
        } else {
            fallback_normal
        };
        Some(((self.origin / self.weight).as_vec3(), normal))
    }
}

/// Flatten's weight for a vertex at `pos`, before the mask: the brush's
/// falloff round the dab and Front Facing.
#[inline]
fn flatten_weight(p: &Params, d: &Dab, pos: Vec3, normal: Vec3) -> f32 {
    let r = p.radius;
    let d2 = pos.distance_squared(d.center);
    if d2 >= r * r {
        return 0.0;
    }
    p.falloff.at(d2.sqrt() / r, quintic) * facing(p.front_facing, normal, pos, d.cam)
}

/// The plane a Flatten dab levels to: fitted to the surface under the ring
/// and moved along its normal by Plane Offset. A point of it and its normal,
/// or `None` where nothing under the ring carries weight. `kept` is the
/// stroke's start normals (`Scratch`).
///
/// It is fitted with mask-free weights: a mask edge would otherwise tilt it
/// and raise a ridge along the mask boundary. Its origin comes from the
/// stroke-start positions: with a live origin a non-zero offset drills or
/// balloons without limit. Its normal comes from the normals as the stroke
/// found the surface too ([`Mesh::stroke_start_normal`]), so that the two
/// agree: with the live normals the plane tilted toward wherever the stroke
/// had already levelled.
pub fn flatten_plane(mesh: &Mesh, p: &Params, d: &Dab, leaves: &[u32], kept: &mut Vec<[f32; 3]>) -> Option<(Vec3, Vec3)> {
    let (origin, n) = flatten_fit(mesh, p, d, leaves, kept)?;
    Some((origin + n * (p.radius * p.flatten_offset), n))
}

/// [`flatten_plane`] before Plane Offset.
fn flatten_fit(mesh: &Mesh, p: &Params, d: &Dab, leaves: &[u32], kept: &mut Vec<[f32; 3]>) -> Option<(Vec3, Vec3)> {
    per_vertex(kept, mesh);
    let kept = SharedMut::new(kept);
    let fit = fold_leaves(
        mesh,
        leaves,
        |sum: &mut PlaneSum, v| {
            let i = v as usize;
            let w = flatten_weight(p, d, mesh.positions[i], mesh.normals[i]);
            if w > 0.0 {
                // SAFETY: each leaf is totalled by one task, and owns its vertices.
                sum.add(mesh.start_pos(v), unsafe { start_normal(mesh, &kept, v) }, w);
            }
        },
        PlaneSum::merge,
    );
    fit.plane(d.normal)
}

/// How much of a dab's levelling a vertex `along` from a LOCKED plane gets,
/// for a brush of radius `r`: all of it within a quarter of a radius of the
/// plane, none half a radius away. A stroke that wanders off the plane
/// (round the side of the form it is cutting a facet on, say) then stops
/// working where the plane has left the surface behind, instead of hauling
/// the surface the whole way to it. Half a radius is as far as a stroke of
/// inverted Flatten may move a vertex, and so as far as this can: nothing
/// further from the plane is touched.
#[inline]
fn locked_reach(along: f32, r: f32) -> f32 {
    smooth(2.0 - 4.0 * along.abs() / r.max(1e-12))
}

fn flatten(mesh: &mut Mesh, p: &Params, d: &Dab, leaves: &[u32], fit: &[u32], scratch: &mut Scratch) {
    let r = p.radius;
    // The stroke's one plane with Lock Plane, else this dab's own, steadied
    // by Steady Plane before it is offset.
    let plane = match d.plane {
        Some(plane) => Some(plane),
        None => flatten_fit(mesh, p, d, fit, &mut scratch.start_normals).map(|(origin, n)| {
            let (origin, n) = steadied(scratch, p, d, origin, n);
            (origin + n * (r * p.flatten_offset), n)
        }),
    };
    let Some((origin, n)) = plane else { return };
    let locked = d.plane.is_some();
    let mode = p.flatten_mode.inverted(p.sign < 0.0);
    let e = p.e(d.pressure) * 4.0 * 0.1;
    // `flatten_weight` turns down the same vertices first.
    mesh.move_within(leaves, d.center, r * r, |_, vt| {
        let pos = vt.pos;
        let fit_w = flatten_weight(p, d, pos, vt.normal);
        if fit_w <= 0.0 {
            return None;
        }
        let w = fit_w * (1.0 - vt.mask);
        let along = (pos - origin).dot(n);
        let mut k = p.share((w * e).clamp(0.0, 1.0));
        if locked {
            k *= locked_reach(along, r);
        }
        match mode {
            // One side of the plane only: what is on the other is left alone.
            FlattenMode::Scrape if along <= 0.0 => None,
            FlattenMode::Fill if along >= 0.0 => None,
            FlattenMode::Scrape | FlattenMode::Fill => Some(pos - n * (along * k)),
            FlattenMode::Both if p.sign >= 0.0 => Some(pos - n * (along * k)),
            FlattenMode::Both => {
                // Inverted: push away from the plane (contrast), bounded per stroke.
                let new = pos + n * (along * k);
                let h = (new - vt.start).dot(n);
                Some(new - n * (h - h.clamp(-0.5 * r, 0.5 * r)))
            }
        }
    });
}

fn clay(mesh: &mut Mesh, p: &Params, d: &Dab, leaves: &[u32], fit: &[u32], scratch: &mut Scratch) {
    let r = p.radius;
    let pressure = d.pressure;
    // Pressure narrows and softens the tip a little, like pressing harder
    // with a real tool; the build height keeps using the full radius.
    let eff_r = r * (0.7 + 0.3 * pressure);
    let soft = (p.clay_soft * (1.5 - 0.5 * pressure)).clamp(0.05, 1.0);
    let height = p.sign * r * p.clay_depth;

    // The tip is measured on the stroke-start surface. Measured on the live
    // surface, a firm stroke breaks into a chain of separate round blobs.
    let reach2 = (eff_r + d.prev.map_or(0.0, |q| q.distance(d.center))).powi(2);
    let tip_w = |start: Vec3, normal: Vec3, pos: Vec3| {
        clay_weight(&p.falloff, start.distance(d.center) / eff_r, soft)
            * facing(p.front_facing, normal, pos, d.cam)
    };
    // Live origin (a stroke-start origin makes dabs fight and corrugates the
    // surface), stroke-start normal (the live footprint at a stroke head is a
    // ramp that leans the push forward and shears the mesh).
    let fit = {
        let mesh = &*mesh;
        fold_leaves(
            mesh,
            fit,
            |sum: &mut PlaneSum, v| {
                let start = mesh.start_pos(v);
                if start.distance_squared(d.center) < reach2 {
                    let i = v as usize;
                    let pos = mesh.positions[i];
                    sum.add(pos, mesh.start_nrm(v), tip_w(start, mesh.normals[i], pos));
                }
            },
            PlaneSum::merge,
        )
    };
    let Some((origin, n)) = fit.plane(d.normal) else { return };
    // Steady Plane (none on Clay Strips, which is left exactly as it was).
    let (origin, n) = steadied(scratch, p, d, origin, n);

    let per_dab = {
        let base = p.e(pressure) * 4.0 * 0.05;
        p.dose * if p.accumulate { base.max(p.e_acc(pressure) * 4.0 * 0.05 / 0.25) } else { base }
    };
    // Strong dabs are swept from the previous centre instead of stamped;
    // whole stamps print their rims as a row of crescents. The steps of the
    // sweep are no further apart than a third of the tip's soft edge.
    let steps = match d.prev {
        Some(_) => ((per_dab / 0.15).ceil().max((p.spacing * 3.0 / soft - 1e-3).ceil()) as usize).clamp(1, 8),
        None => 1,
    };
    let prev = d.prev.unwrap_or(d.center);
    let lerp_scale = p.e(pressure) * 4.0 * 0.05 * p.dose / steps as f32;
    let rate = p.sign * p.clay_depth * p.e_acc(pressure) * 4.0 * 0.05 * (r / REF_RADIUS) * p.dose
        / steps as f32;
    let max_along = height.abs() * if p.accumulate { 3.0 } else { 1.5 };
    let band = 0.75 * height.abs();
    let dir_sign = p.sign;

    mesh.move_vertices(leaves, |_, vt| {
        let start = vt.start;
        if start.distance_squared(d.center) >= reach2 {
            return None;
        }
        let mut pos = vt.pos;
        let guard = (1.0 - vt.mask) * facing(p.front_facing, vt.normal, pos, d.cam);
        for s in 1..=steps {
            // The alpha shapes what is laid down, not the plane it is laid
            // on: `fit` above is taken without it.
            let w = if steps == 1 {
                tip_w(start, vt.normal, vt.pos) * (1.0 - vt.mask) * p.printed(d, start - d.center)
            } else {
                let centre = prev.lerp(d.center, s as f32 / steps as f32);
                clay_weight(&p.falloff, start.distance(centre) / eff_r, soft) * guard * p.printed(d, start - centre)
            };
            if w <= 0.0 {
                continue;
            }
            let along = (pos - origin).dot(n);
            // Target height is scaled by the weight; unscaled, the settled
            // shape is a flat-topped cylinder.
            let mut step = (height * w - along) * (w * lerp_scale).clamp(0.0, 1.0);
            if p.accumulate {
                step += rate * w;
            }
            // Per-stroke depth cap, eased so it does not leave a shelf.
            let built = (pos - start).dot(n) * dir_sign;
            if step * dir_sign > 0.0 {
                let remaining = max_along - built;
                if remaining <= 0.0 {
                    continue;
                }
                if remaining < band {
                    let k = (remaining / band).clamp(0.0, 1.0);
                    step *= k * k;
                }
                if step * dir_sign > remaining {
                    step = remaining * dir_sign;
                }
            }
            pos += n * step;
        }
        Some(pos)
    });
}

fn crease(mesh: &mut Mesh, p: &Params, d: &Dab, leaves: &[u32]) {
    let r = p.radius;
    let r2 = r * r;
    let n = d.normal;
    let dir = d.dir;
    let e_off = (p.e_off(d.pressure) * 4.0 * 0.2).max(0.0);
    let e_acc = p.e_acc(d.pressure) * 4.0 * 0.2 * p.dose;
    mesh.move_within(leaves, d.center, r2, |_, vt| {
        let pos = vt.pos;
        let d2 = pos.distance_squared(d.center);
        if d2 >= r2 {
            return None;
        }
        let w = p.falloff.at(d2.sqrt() / r, crease_built_in)
            * (1.0 - vt.mask)
            * facing(p.front_facing, vt.normal, pos, d.cam);
        if w <= 0.0 {
            return None;
        }
        // Pinch only across the travel direction; pinching toward the dab
        // centre drags material along the stroke into hooks and beads.
        let live = pos - d.center;
        let live_tang = live - n * live.dot(n);
        let live_across = live_tang - dir * live_tang.dot(dir);
        if p.accumulate {
            let carve = n * (p.sign * p.crease_depth * e_acc * (r / REF_RADIUS) * w);
            let pinch = live_across * (w * p.crease_pinch * e_acc).clamp(0.0, 1.0);
            return Some(pos + carve - pinch);
        }
        let start = vt.start;
        let off = start - d.center;
        let start_n = off.dot(n);
        let tang = off - n * start_n;
        let start_along = tang.dot(dir);
        let start_across = tang - dir * start_along;
        // Deepest dab wins: never push a vertex back toward the surface.
        let mut carve = p.sign * r * p.crease_depth * w;
        let achieved = (pos - start).dot(n);
        if p.sign * achieved > p.sign * carve {
            carve = achieved;
        }
        let mut pinched = start_across * (1.0 - p.crease_pinch * w);
        if live_across.length_squared() < pinched.length_squared() {
            pinched = live_across;
        }
        let target = d.center + n * (start_n + carve) + dir * start_along + pinched;
        Some(pos + (target - pos) * p.share((w * e_off).clamp(0.0, 1.0)))
    });
}

/// Which of a vertex's neighbours smoothing averages it with (every mode
/// of Smooth, Auto Smooth and Clay's finishing pass, as in Blender).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Ring {
    /// None: the vertex stays where it is. One with two neighbours or fewer
    /// is the tip of a spike or a stray corner, which averaging only drags
    /// about; and so is a rim vertex with no rim on either side of it.
    Held,
    /// All of them: a vertex inside the surface.
    All,
    /// Only the two along the open edge it is on: a vertex on the rim of an
    /// open surface. Averaged with the rest, it was drawn in off the rim and
    /// every opening shrank as it was smoothed; with these it smooths along
    /// the rim, and the rim keeps its place.
    Rim,
}

impl Ring {
    #[inline]
    pub(super) fn of(mesh: &Mesh, v: u32) -> Ring {
        Ring::in_rings(mesh.rings(), v)
    }

    #[inline]
    fn in_rings(rings: Rings<'_>, v: u32) -> Ring {
        let ring = rings.neighbours(v);
        if ring.len() <= 2 {
            return Ring::Held;
        }
        // Round a vertex inside a surface there are as many triangles as
        // neighbours; round one on a rim, fewer.
        if rings.vert_tris(v).len() >= ring.len() {
            return Ring::All;
        }
        if ring.iter().filter(|&&o| on_rim(rings, v, o)).count() >= 2 { Ring::Rim } else { Ring::Held }
    }

    /// Whether neighbour `o` of `v` is one this vertex is averaged with.
    #[inline]
    fn counts(self, rings: Rings<'_>, v: u32, o: u32) -> bool {
        match self {
            Ring::All => true,
            Ring::Rim => on_rim(rings, v, o),
            Ring::Held => false,
        }
    }
}

/// Whether the edge from `v` to `o` is on an open rim: one triangle has it.
fn on_rim(rings: Rings<'_>, v: u32, o: u32) -> bool {
    let tris = rings.vert_tris(v).iter();
    tris.filter(|&&t| rings.indices()[t as usize * 3..t as usize * 3 + 3].contains(&o)).count() == 1
}

/// The mean of `value` over the neighbours of `v` that `ring` counts,
/// summed in double (see module docs); `None` if it counts none.
#[inline]
fn ring_mean(rings: Rings<'_>, v: u32, ring: Ring, value: impl Fn(u32) -> Vec3) -> Option<Vec3> {
    let (mut sum, mut count) = (DVec3::ZERO, 0u32);
    for &o in rings.neighbours(v) {
        if ring.counts(rings, v, o) {
            sum += value(o).as_dvec3();
            count += 1;
        }
    }
    (count > 0).then(|| (sum / count as f64).as_vec3())
}

/// The middle of the neighbours a vertex is smoothed toward ([`Ring`]), or
/// `None` for a vertex smoothing leaves where it is. Inside a surface it is
/// the plain mean of every neighbour, to the bit as it always was.
#[inline]
fn neighbour_mean(mesh: &Mesh, v: u32) -> Option<Vec3> {
    ring_mean(mesh.rings(), v, Ring::of(mesh, v), |o| mesh.positions[o as usize])
}

/// Jacobi relaxation passes over the vertices of `leaves` that
/// `scratch.weights` gives a weight above zero: every target is computed from
/// the pass's starting positions, then all are written. (Updating in place is
/// order dependent and breaks mirror symmetry.) Each vertex goes all the way
/// toward the mean of its neighbours by its weight.
fn relax_passes(mesh: &mut Mesh, leaves: &[u32], scratch: &mut Scratch, amount: f32, min_move2: f32) {
    stepped_passes(mesh, leaves, scratch, amount, min_move2, |_, _, pos, mean, k| pos + (mean - pos) * k);
}

/// [`relax_passes`] with the step a vertex takes left to `step`: it is given
/// the mesh as the pass found it, the vertex, where it is, the mean of its
/// neighbours and its weight for this pass (0 to 1), and says where it goes.
fn stepped_passes(
    mesh: &mut Mesh,
    leaves: &[u32],
    scratch: &mut Scratch,
    amount: f32,
    min_move2: f32,
    step: impl Fn(&Mesh, u32, Vec3, Vec3, f32) -> Vec3 + Sync,
) {
    let full = amount.floor() as usize;
    let frac = amount - full as f32;
    let passes = full + usize::from(frac > 0.001);
    let Scratch { weights, targets, starts, .. } = scratch;
    let weights = &*weights;
    for pass in 0..passes {
        let factor = if pass < full { 1.0 } else { frac };
        {
            let mesh = &*mesh;
            fill(mesh, leaves, starts, targets, |at, v| {
                let w = weights[at];
                if w <= 0.0 {
                    return Vec3::ZERO;
                }
                let pos = mesh.positions[v as usize];
                match neighbour_mean(mesh, v) {
                    Some(avg) => step(mesh, v, pos, avg, (w * factor).clamp(0.0, 1.0)),
                    None => pos,
                }
            });
        }
        let targets = &*targets;
        mesh.move_vertices(leaves, |at, vt| {
            if weights[at] <= 0.0 {
                return None;
            }
            let new = targets[at];
            (new.distance_squared(vt.pos) > min_move2).then_some(new)
        });
    }
}

/// The most passes one Smooth dab makes (Blender's `iteration_strengths`).
pub(super) const SMOOTH_MOST_PASSES: f32 = 4.0;

/// How many passes a Smooth dab makes at strength `e` (in the old units,
/// times pressure and dose), each moving a vertex all the way to the middle
/// of its neighbours by its weight, the last a fraction of a pass. Four at
/// full strength and no more: a dab of ten passes, as full strength once
/// made, cost ten walks over the footprint and smoothed little more than
/// four, and the brush's weight on its edge then reached far past where it
/// seemed to. The old passes went 40% of the way; four of these do what
/// ten of those did, and so does each of the slider's other values once it
/// is mapped (`strength.rs`), for the form if not for the finest noise,
/// which these take out faster.
#[inline]
pub(super) fn smooth_passes(e: f32) -> f32 {
    (e * SMOOTH_MOST_PASSES).clamp(0.0, SMOOTH_MOST_PASSES)
}

/// How many times as many passes Smooth makes in Surface mode as in
/// Classic at the same Strength (up to its four). A pass that keeps volume
/// takes out less of a fine roughness than a plain one, since it holds
/// every vertex half way to where it was: with twice as many, a stroke at
/// the Strength Smooth starts with takes as much roughness off a rough ball
/// as the old way did (`--brush-feel`: 0.647 of it left against 0.649) and
/// none of its volume. Past about half Strength it takes off less than the
/// old way could: it holds the surface.
const SURFACE_GAIN: f32 = 2.0;

fn smooth_dab(mesh: &mut Mesh, p: &Params, d: &Dab, leaves: &[u32], scratch: &mut Scratch) {
    let r = p.radius;
    let r2 = r * r;
    {
        let mesh = &*mesh;
        fill(mesh, leaves, &mut scratch.starts, &mut scratch.weights, |_, v| {
            let i = v as usize;
            let pos = mesh.positions[i];
            let d2 = pos.distance_squared(d.center);
            if d2 >= r2 {
                return 0.0;
            }
            let w = p.falloff.at(d2.sqrt() / r, smooth)
                * (1.0 - mesh.mask[i])
                * facing(p.front_facing, mesh.normals[i], pos, d.cam);
            w.max(0.0)
        });
    }
    let amount = smooth_passes(p.e(d.pressure) * p.dose);
    match p.smooth_mode {
        // Smoothing that keeps the form's volume, holding each vertex to
        // where the stroke found it.
        SmoothMode::Surface => {
            let amount = smooth_passes(p.e(d.pressure) * p.dose * SURFACE_GAIN);
            let weights = std::mem::take(&mut scratch.weights);
            let full = amount.floor() as usize;
            let frac = amount - full as f32;
            for pass in 0..full + usize::from(frac > 0.001) {
                let factor = if pass < full { 1.0 } else { frac };
                relax_step(mesh, leaves, scratch, Anchor::Start, Vec3::ZERO, true, |_, at, _| (weights[at] * factor, 0.0));
            }
            scratch.weights = weights;
        }
        SmoothMode::All if p.keep_volume > 0.0 => {
            fill(mesh, leaves, &mut scratch.starts, &mut scratch.before, |_, v| mesh.positions[v as usize]);
            relax_passes(mesh, leaves, scratch, amount, 0.0);
            give_back(mesh, leaves, scratch, p.keep_volume.min(1.0));
        }
        SmoothMode::All => relax_passes(mesh, leaves, scratch, amount, 0.0),
        // Along the normal only, and one way only. What a vertex would have
        // slid sideways is left out, so nothing jumps where the surface
        // passes from standing up to lying low.
        SmoothMode::Peaks => stepped_passes(mesh, leaves, scratch, amount, 0.0, |mesh, v, pos, mean, k| {
            let n = mesh.normals[v as usize];
            pos + n * ((mean - pos).dot(n).min(0.0) * k)
        }),
        SmoothMode::Valleys => stepped_passes(mesh, leaves, scratch, amount, 0.0, |mesh, v, pos, mean, k| {
            let n = mesh.normals[v as usize];
            pos + n * ((mean - pos).dot(n).max(0.0) * k)
        }),
        // Within the surface only: the part of the way to the mean that lies
        // across the normal, and then back down on to the surface.
        SmoothMode::Relax => stepped_passes(mesh, leaves, scratch, amount, 0.0, |mesh, v, pos, mean, k| {
            let n = mesh.normals[v as usize];
            let toward = mean - pos;
            slid(mesh, v, pos, n, (toward - n * toward.dot(n)) * k)
        }),
    }
}

/// Where the vertex `v`, at `pos` with normal `n`, comes to when it is slid
/// `across` its own tangent plane: that far across, and back on to the
/// surface.
///
/// A step in the tangent plane leaves a curved surface behind: by the square
/// of the step times half the curvature that way. How the surface curves
/// that way is read off the vertex's neighbours, each of which says how far
/// the surface has fallen away (or risen) at its own distance and in its own
/// direction, and counts by how nearly it lies along the step. So a step
/// along a cylinder is not corrected at all and a step round it is, which
/// the mean curvature alone would get half wrong each way. Summed in double
/// (see the module docs).
#[inline]
pub(super) fn slid(mesh: &Mesh, v: u32, pos: Vec3, n: Vec3, across: Vec3) -> Vec3 {
    let step2 = across.length_squared();
    if step2 <= 0.0 {
        return pos;
    }
    let (mut fall, mut count) = (0.0f64, 0.0f64);
    for &o in mesh.neighbours(v) {
        let to = mesh.positions[o as usize] - pos;
        let height = to.dot(n);
        let flat = to - n * height;
        let reach2 = flat.length_squared();
        if reach2 < 1e-18 {
            continue;
        }
        // The square of the step's length along this neighbour's direction.
        let along2 = flat.dot(across).powi(2) / reach2;
        fall += (along2 * height / reach2) as f64;
        count += along2 as f64;
    }
    let lift = if count > 0.0 { (fall / count) as f32 * step2 } else { 0.0 };
    // On any surface worth the name the way back is a small part of the
    // step. Where the mesh is crumpled (a neighbour all but straight above
    // the vertex reads as a hairpin bend) it is held to half the step, so a
    // bad patch is slid over and not flung away.
    let most = 0.5 * step2.sqrt();
    pos + across + n * lift.clamp(-most, most)
}

/// Keep Volume: gives back `keep` of what a Smooth dab took from the form
/// under it.
///
/// Smoothing a curved surface draws it in: every vertex moves toward the
/// middle of its neighbours, which on a ball, a finger or a fin's edge is
/// inward, so each stroke leaves the form thinner. What the dab did to the
/// form's bulk is how far it moved its vertices along their normals, added
/// up; that much is put back, spread over the same vertices by the brush's
/// own weights. The bumps a dab flattened stay flattened (pushing every
/// vertex out a little does not bring one back), and on a flat surface,
/// where smoothing moves as much up as down, nothing is put back at all.
///
/// One number for the whole dab, so it cannot ring or roughen the surface
/// the way undoing the shrinkage vertex by vertex does (Taubin's inflating
/// pass sharpens what the smoothing pass has just blunted, and on a form
/// only a few vertices round it reads the whole form as a detail and lets
/// it shrink regardless).
fn give_back(mesh: &mut Mesh, leaves: &[u32], scratch: &Scratch, keep: f32) {
    let Scratch { weights, before, starts, .. } = scratch;
    // How far the dab moved the surface along its normals, and the weights,
    // leaf by leaf and then in the leaves' order: the same total whatever
    // threads did the adding.
    let total = {
        let mesh = &*mesh;
        let hidden = mesh.hidden_vertices();
        let of_leaf = |(k, &l): (usize, &u32)| {
            let (mut moved, mut weight) = (0.0f64, 0.0f64);
            for (n, v) in owned(&mesh.bvh.leaves[l as usize]).enumerate() {
                let w = weights[starts[k] + n];
                if w > 0.0 && !hidden.is_some_and(|hidden| hidden[v]) {
                    moved += (mesh.positions[v] - before[starts[k] + n]).dot(mesh.normals[v]) as f64;
                    weight += w as f64;
                }
            }
            (moved, weight)
        };
        let sums: Vec<(f64, f64)> = if leaf_vertex_total(mesh, leaves) < PAR_MIN {
            leaves.iter().enumerate().map(of_leaf).collect()
        } else {
            leaves.par_iter().enumerate().with_min_len(8).map(of_leaf).collect()
        };
        sums.into_iter().fold((0.0, 0.0), |(moved, weight), (m, w)| (moved + m, weight + w))
    };
    let (moved, weight) = total;
    if weight <= 1e-12 || moved == 0.0 {
        return;
    }
    let back = (-(keep as f64) * moved / weight) as f32;
    mesh.move_vertices(leaves, |at, vt| {
        let w = weights[at];
        (w > 0.0).then(|| vt.pos + vt.normal * (w * back))
    });
}

/// The smoothing that keeps volume: Vollmer, Kreylos and Hamann's "HC"
/// Laplacian, which Blender's Surface Smooth is. Each pass moves a vertex to
/// the middle of its neighbours, as plain smoothing does, and then back by
/// how far that middle is from where the vertex is held to (`HC_ALPHA` of
/// the way to its anchor, the rest where it was): partly by its own such
/// distance, `HC_BETA`, and partly by its neighbours'. On a ball every
/// vertex goes in and comes straight back out, so the form keeps its size,
/// while a bump, which its neighbours do not share, still goes down. Both at
/// a half, where Blender starts them.
const HC_ALPHA: f32 = 0.5;
const HC_BETA: f32 = 0.5;

/// What one pass of [`relax_step`] has worked out for a vertex.
#[derive(Clone, Copy, Default, PartialEq)]
pub(super) struct Tidy {
    /// How far the vertex goes, but for the smoothing's second half.
    moved: Vec3,
    /// Its weight in the smoothing (nought: it is not smoothed).
    smooth: f32,
}

/// Where the smoothing that keeps volume holds a vertex to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Anchor {
    /// Where it is, as each pass finds it: Auto Smooth after a dab, which
    /// keeps what the dab did. A pass of it is a step of the smoothing of
    /// a smoothing (bi-Laplacian), which takes ripples off and leaves forms.
    Here,
    /// Where the stroke found it: the Smooth brush in Surface mode.
    Start,
}

/// One pass of the smoothing that keeps volume and of Even Out, over the
/// vertices `leaves` own. `weigh` gives each (the mesh, its place in the
/// leaves' order, the vertex) its weight in the smoothing and its share of
/// Even Out, 0 to 1 each; `smoothing` is false when every weight in the
/// smoothing is nought, which saves its second half.
///
/// Jacobi, as every smoothing here is: the pass reads the surface as it
/// found it and moves every vertex at once, so the order the work is shared
/// out in cannot show and mirror twins stay twins. In two walks over the
/// leaves, each on all threads: the first works out each vertex's move and
/// the smoothing's first half (written by vertex, for its neighbours to
/// read), and the second the second half as it moves the vertices.
fn relax_step(
    mesh: &mut Mesh,
    leaves: &[u32],
    scratch: &mut Scratch,
    anchor: Anchor,
    heading: Vec3,
    smoothing: bool,
    weigh: impl Fn(&Mesh, usize, u32) -> (f32, f32) + Sync,
) {
    let Scratch { starts, tidy, held_off, .. } = scratch;
    {
        let mesh = &*mesh;
        if smoothing {
            per_vertex(held_off, mesh);
        }
        let off = SharedMut::new(held_off);
        fill(mesh, leaves, starts, tidy, |at, v| {
            let (smooth, even) = weigh(mesh, at, v);
            if smooth <= 0.0 && even <= 0.0 {
                return Tidy::default();
            }
            let ring = Ring::of(mesh, v);
            if ring == Ring::Held {
                return Tidy::default();
            }
            let q = mesh.positions[v as usize];
            let mut tidied = Tidy::default();
            if smooth > 0.0 {
                if let Some(mean) = ring_mean(mesh.rings(), v, ring, |o| mesh.positions[o as usize]) {
                    let o = match anchor {
                        Anchor::Here => q,
                        Anchor::Start => mesh.start_pos(v),
                    };
                    let held = mean - (o * HC_ALPHA + q * (1.0 - HC_ALPHA));
                    // SAFETY: `fill` gives each leaf's vertices to one task.
                    unsafe { off.write(v as usize, held.to_array()) };
                    let smooth = smooth.min(MOST_PER_PASS);
                    tidied = Tidy { moved: (mean - q) * smooth - held * (smooth * HC_BETA), smooth };
                }
            }
            // Not on a rim: slid within the surface it would leave it.
            if even > 0.0 && ring == Ring::All {
                tidied.moved += evened(mesh, v, q, heading, even.min(1.0)) - q;
            }
            tidied
        });
    }
    let tidy = &*tidy;
    if !smoothing {
        return mesh.move_vertices(leaves, |at, vt| {
            let moved = tidy[at].moved;
            (moved != Vec3::ZERO).then(|| vt.pos + moved)
        });
    }
    {
        let off = &*held_off;
        mesh.move_vertices_ringed(leaves, |at, vt, rings| {
            let t = tidy[at];
            if t == Tidy::default() {
                return None;
            }
            let mut pos = vt.pos + t.moved;
            if t.smooth > 0.0 {
                let ring = Ring::in_rings(rings, vt.index);
                let theirs = ring_mean(rings, vt.index, ring, |o| Vec3::from_array(off[o as usize])).unwrap_or(Vec3::ZERO);
                pos -= theirs * (t.smooth * (1.0 - HC_BETA));
            }
            Some(pos)
        });
    }
    // Nought again for the next pass: every vertex written is one of these
    // leaves', and a leaf's vertices are one run.
    let off = SharedMut::new(held_off);
    each_leaf(mesh, leaves, |leaf| {
        // SAFETY: each leaf is cleared by one task, and owns its run.
        unsafe { off.slice(owned(leaf)) }.fill([0.0; 3]);
    });
}

/// Even Out at full setting does this many times as much a dab, at the
/// tuned spacing and full pressure, as its slider says (to nineteen
/// twentieths of the way at most): a fifth evens a vertex under the middle
/// of the ring four fifths of the way, and five dabs cover each point of a
/// stroke.
const EVEN_RATE: f32 = 4.0;

/// Where Even Out takes vertex `v`, at `q`: `k` of the way to the middle of
/// its neighbours within the surface, and back on to it ([`slid`]). It does
/// not change the form, only how the triangles lie on it.
///
/// With a `heading`, the neighbours that lie along it or straight across it
/// count for most and those at a slant for least (Blender's topology rake,
/// `bmesh_four_neighbor_average`, whose weights are 1/4 along or across and
/// nought at a slant): what a swelling has stretched out of line is drawn
/// back into rows along and across the stroke. Every neighbour also counts
/// for a little whatever way it lies ([`RAKE_FLOOR`]), so that the weights
/// can never all but vanish, and the place a vertex goes changes smoothly
/// with the way the stroke goes. Without a heading every neighbour counts
/// the same.
fn evened(mesh: &Mesh, v: u32, q: Vec3, heading: Vec3, k: f32) -> Vec3 {
    let n = mesh.normals[v as usize];
    let along = (heading - n * heading.dot(n)).normalize_or_zero();
    let (mut sum, mut weight) = (DVec3::ZERO, 0.0f64);
    for &o in mesh.neighbours(v) {
        let p = mesh.positions[o as usize];
        let w = if along == Vec3::ZERO {
            1.0
        } else {
            let to = p - q;
            let flat = to - n * to.dot(n);
            let len2 = flat.length_squared();
            let cos2 = if len2 > 1e-24 { flat.dot(along).powi(2) / len2 } else { 0.5 };
            ((cos2 - 0.5) * (cos2 - 0.5) + RAKE_FLOOR) as f64
        };
        sum += p.as_dvec3() * w;
        weight += w;
    }
    if weight <= 0.0 {
        return q;
    }
    let toward = (sum / weight).as_vec3() - q;
    slid(mesh, v, q, n, (toward - n * toward.dot(n)) * k)
}

/// What every neighbour counts for in Even Out along a heading, besides how
/// nearly it lies along or across it (at most a quarter): a fifth of that.
const RAKE_FLOOR: f32 = 0.05;

/// Whether a brush weighs a vertex by where the stroke found it (the
/// footprint stays where it was laid as the surface rises), rather than by
/// where it is: what its Auto Smooth and Even Out are weighed by too.
fn weighs_by_start(brush: Brush) -> bool {
    matches!(brush, Brush::Standard | Brush::Inflate | Brush::Layer | Brush::Pinch | Brush::Clay)
}

/// Auto Smooth and Even Out: what a brush does to the surface under a dab
/// after the dab, by the same falloff over the same footprint, a vertex at
/// a time for what pressure gave the dab (as Blender's Auto Smooth and
/// Topology Rake).
///
/// Auto Smooth smooths the way that keeps volume ([`relax_step`]), each
/// vertex held to where the dab left it: a brush that swells or pulls the
/// surface leaves it as round as it found it, and no thinner. At full it is
/// as many passes a dab as Smooth makes at full Strength. Even Out slides
/// the triangles within the surface so they stay even ([`evened`]), lined
/// up with the way the stroke is going.
///
/// Measured where the brush measures its own weights, from the dab's centre.
fn tidy(mesh: &mut Mesh, p: &Params, d: &Dab, leaves: &[u32], scratch: &mut Scratch) {
    // For the dabs since the last that tidied (`Params::tidy`).
    let p = &Params { dose: p.tidy, ..*p };
    let passes = (p.auto_smooth * d.pressure * p.dose * SMOOTH_MOST_PASSES).clamp(0.0, SMOOTH_MOST_PASSES);
    // Snake Hook evens out once, at the release ([`even_out_moved`]):
    // slid at every step, the surface it was pulling slipped back out of
    // its grip and a long pull came out half as long.
    let even = if p.brush == Brush::SnakeHook { 0.0 } else { p.even_out * EVEN_RATE * d.pressure };
    if passes <= 0.0 && even <= 0.0 {
        return;
    }
    // About the dab's own centre, even for Snake Hook, whose tip has moved
    // on by its step: the leaves a copy of a dab is given are the ones it
    // reaches from there, and only those are shared out between the copies
    // that overlap at a mirror plane (`share::bank`).
    let (r, centre) = (p.radius, d.center);
    let r2 = r * r;
    let by_start = weighs_by_start(p.brush);
    let weight = |mesh: &Mesh, v: u32| {
        let i = v as usize;
        let pos = mesh.positions[i];
        let d2 = if by_start { mesh.start_pos(v) } else { pos }.distance_squared(centre);
        if d2 >= r2 {
            return 0.0;
        }
        p.falloff.at(d2.sqrt() / r, smooth) * (1.0 - mesh.mask[i]) * facing(p.front_facing, mesh.normals[i], pos, d.cam)
    };
    // In as few passes as can take it: a pass may go up to half as far
    // again as to the middle of a vertex's neighbours (the smoothing still
    // settles, every way the surface can ripple), which a dab that tidies
    // for two at a fifth or so of Auto Smooth (Inflate's) needs. Each pass
    // is a walk over the footprint, and a pass is what tidying costs.
    let count = ((passes / MOST_PER_PASS).ceil() as usize).max(1);
    let per_pass = passes / count as f32;
    for pass in 0..count {
        let smooth = per_pass;
        let even = if pass == 0 { even } else { 0.0 };
        relax_step(mesh, leaves, scratch, Anchor::Here, d.heading, smooth > 0.0, |mesh, _, v| {
            let w = weight(mesh, v);
            // Even Out takes a vertex a share of the way, which a dab at
            // another spacing closes as the dabs it stands in for would have
            // between them (`Params::share`).
            (w * smooth, if even > 0.0 { p.share((w * even).clamp(0.0, 0.95)) } else { 0.0 })
        });
    }
}

/// The most of the way to the middle of its neighbours one pass of Auto
/// Smooth takes a vertex (see [`tidy`]).
const MOST_PER_PASS: f32 = 1.5;

/// How many passes of Even Out Move and Snake Hook make at full setting,
/// once, as the stroke is let go: at the fifth they start with, six.
const RELEASE_EVEN_PASSES: f32 = 30.0;

/// Even Out for Move and Snake Hook: once the stroke is let go, slides the
/// triangles it stretched back toward even within the surface ([`evened`]),
/// each vertex `verts` lists (ascending) by its share in `weights`, every
/// neighbour counting the same. (Lined up with the way each vertex was
/// moved, it evened out no better, `--brush-feel`'s Move drag, and where a
/// vertex had hardly moved that way was rounding.) It changes the dragged
/// form little: the triangles on it are evened out. Part of the stroke, so
/// one undo takes back both. `leaves` are the leaves that hold `verts` and
/// their neighbours (`Mesh::leaves_of`).
///
/// Not at every step: what a drag or a pull holds on to is the surface under
/// the pen, and slid from under it at every step a Snake Hook pull lost its
/// grip and came out half as long.
pub fn even_out_moved(mesh: &mut Mesh, verts: &[u32], weights: &[f32], leaves: &[u32], amount: f32) {
    let passes = (amount * RELEASE_EVEN_PASSES).max(0.0);
    let full = passes.floor() as usize;
    let frac = passes - full as f32;
    let count = full + usize::from(frac > 0.001);
    let mut targets = vec![Vec3::ZERO; verts.len()];
    for pass in 0..count {
        let factor = if pass < full { 1.0 } else { frac };
        // Slid within the surface as it is now.
        mesh.flush_normals();
        {
            let mesh = &*mesh;
            let place = |(target, (&v, &w)): (&mut Vec3, (&u32, &f32))| {
                let q = mesh.positions[v as usize];
                let k = (w * factor).clamp(0.0, 1.0);
                *target = if k > 0.0 && Ring::of(mesh, v) == Ring::All {
                    evened(mesh, v, q, Vec3::ZERO, k)
                } else {
                    q
                };
            };
            if verts.len() < PAR_MIN {
                targets.iter_mut().zip(verts.iter().zip(weights)).for_each(place);
            } else {
                targets.par_iter_mut().zip(verts.par_iter().zip(weights)).with_min_len(1024).for_each(place);
            }
        }
        mesh.move_listed(verts, |slot, vt| (targets[slot] != vt.pos).then_some(targets[slot]));
        mesh.positions_changed(leaves);
    }
}

/// How much of the way to the stroke's line one Pinch dab at full strength
/// draws the surface under the middle of the brush. Five dabs cover each
/// point of a stroke at the brush's own spacing, so a full-strength stroke
/// draws it about three quarters of the way.
const PINCH_RATE: f32 = 0.25;

/// Pinch: draws the surface in toward the stroke's line, or with the brush
/// inverted spreads it away from the line.
///
/// Each vertex moves across the way the stroke is going and within the
/// plane the dab touches the surface in, never along the stroke: drawn
/// toward the dab's centre instead, the surface is dragged along the stroke
/// and bunches into beads (the same reason Crease pinches across its travel
/// only). Nothing moves in or out along the brush's direction, so there is
/// no carving: the flanks of a ridge are drawn in under it and it sharpens.
///
/// A dab with no direction (a press that never set off) pinches toward its
/// centre: there is no line yet, only a point.
///
/// A vertex is weighed by where the stroke found it, so the footprint stays
/// where it was laid: weighed where it is now, a vertex drawn in toward the
/// line counted for more with every dab that drew it.
fn pinch(mesh: &mut Mesh, p: &Params, d: &Dab, leaves: &[u32]) {
    let r = p.radius;
    let r2 = r * r;
    let (n, dir) = (d.normal, d.dir);
    let rate = p.e(d.pressure) * PINCH_RATE;
    mesh.move_vertices(leaves, |_, vt| {
        let pos = vt.pos;
        let d2 = vt.start.distance_squared(d.center);
        if d2 >= r2 {
            return None;
        }
        let w = p.falloff.at(d2.sqrt() / r, smooth)
            * (1.0 - vt.mask)
            * facing(p.front_facing, vt.normal, pos, d.cam);
        if w <= 0.0 {
            return None;
        }
        let from_centre = pos - d.center;
        let flat = from_centre - n * from_centre.dot(n);
        let across = flat - dir * flat.dot(dir);
        let k = p.share((w * rate).clamp(0.0, 1.0));
        Some(if p.sign >= 0.0 { pos - across * k } else { pos + across * k })
    });
}

/// The surface under a dab as one smooth cap: its height over the plane the
/// dab touches it in, to the second order (a tilt, and how it curves each
/// way), fitted to every vertex under the ring by the brush's own soft
/// weight.
///
/// It is what Nudge slides the surface along. Read off the neighbours of
/// each vertex instead ([`slid`], which Relax uses), the surface is only as
/// good as the mesh just there, and a hard nudge crumples the mesh under
/// itself: the crumpled patch then read as a tight bend, vertices were set
/// down off the surface, and a stroke that was meant to push clay sideways
/// left a cusp where it stopped. A fit over the whole footprint does not
/// see the crumpling.
struct Cap {
    /// Across, up and out: the plane the dab touches the surface in, and
    /// the way it faces.
    across: Vec3,
    up: Vec3,
    out: Vec3,
    /// One over the brush's radius: the fit is made in radii.
    per_radius: f32,
    /// The height at (x, y) radii from the centre is
    /// `c[0] + c[1] x + c[2] y + c[3] x x + c[4] x y + c[5] y y`.
    c: [f32; 6],
}

/// The sums a [`Cap`] is fitted from: for the six terms of its height, each
/// times each (the upper half, row by row) and each times the height.
#[derive(Default)]
struct CapSum {
    products: [f64; 21],
    heights: [f64; 6],
}

impl Cap {
    fn terms(x: f64, y: f64) -> [f64; 6] {
        [1.0, x, y, x * x, x * y, y * y]
    }

    /// The cap under dab `d`, or `None` where too little of the surface is
    /// under the ring to say how it curves.
    fn fit(mesh: &Mesh, p: &Params, d: &Dab, leaves: &[u32]) -> Option<Cap> {
        let out = d.normal.try_normalize()?;
        let (across, up) = out.any_orthonormal_pair();
        let (r, r2) = (p.radius, p.radius * p.radius);
        let per_radius = 1.0 / r.max(1e-12);
        let sums = fold_leaves(
            mesh,
            leaves,
            |sum: &mut CapSum, v| {
                let i = v as usize;
                let from = mesh.positions[i] - d.center;
                let d2 = from.length_squared();
                // The sheet the brush is on, not the far side of a thin form.
                if d2 >= r2 || mesh.normals[i].dot(out) <= 0.0 {
                    return;
                }
                let w = smooth(1.0 - d2.sqrt() * per_radius) as f64;
                let terms = Cap::terms((from.dot(across) * per_radius) as f64, (from.dot(up) * per_radius) as f64);
                let height = from.dot(out) as f64;
                let mut k = 0;
                for row in 0..6 {
                    for col in row..6 {
                        sum.products[k] += w * terms[row] * terms[col];
                        k += 1;
                    }
                    sum.heights[row] += w * terms[row] * height;
                }
            },
            |sum, part| {
                sum.products.iter_mut().zip(part.products).for_each(|(a, b)| *a += b);
                sum.heights.iter_mut().zip(part.heights).for_each(|(a, b)| *a += b);
            },
        );
        let c = sums.solve()?;
        Some(Cap { across, up, out, per_radius, c: c.map(|c| c as f32) })
    }

    /// The cap's height over the point (x, y), in radii from the centre.
    #[inline]
    fn height(&self, x: f32, y: f32) -> f32 {
        let c = &self.c;
        c[0] + c[1] * x + c[2] * y + c[3] * x * x + c[4] * x * y + c[5] * y * y
    }

    /// Where a point at `pos` comes to when it is slid `by` along the
    /// surface: that far across the cap's plane, keeping the height it has
    /// above (or below) the cap. So a bump is carried along as a bump.
    #[inline]
    fn slide(&self, center: Vec3, pos: Vec3, by: Vec3) -> Vec3 {
        let flat = by - self.out * by.dot(self.out);
        let at = |p: Vec3| ((p - center).dot(self.across) * self.per_radius, (p - center).dot(self.up) * self.per_radius);
        let ((x0, y0), (x1, y1)) = (at(pos), at(pos + flat));
        pos + flat + self.out * (self.height(x1, y1) - self.height(x0, y0))
    }
}

impl CapSum {
    /// The six numbers of the cap that fits the sums best (least squares),
    /// by elimination with the largest pivot first. `None` if the sums do
    /// not pin all six down: next to no vertices, or all of them in a line.
    fn solve(&self) -> Option<[f64; 6]> {
        let mut m = [[0.0f64; 7]; 6];
        let mut k = 0;
        for row in 0..6 {
            for col in row..6 {
                (m[row][col], m[col][row]) = (self.products[k], self.products[k]);
                k += 1;
            }
            m[row][6] = self.heights[row];
        }
        let scale = m[0][0];
        if !(scale > 0.0) {
            return None;
        }
        for col in 0..6 {
            let pivot = (col..6).max_by(|&a, &b| m[a][col].abs().total_cmp(&m[b][col].abs()))?;
            if m[pivot][col].abs() <= 1e-9 * scale {
                return None;
            }
            m.swap(col, pivot);
            for row in 0..6 {
                if row != col {
                    let f = m[row][col] / m[col][col];
                    for j in col..7 {
                        m[row][j] -= f * m[col][j];
                    }
                }
            }
        }
        let c: [f64; 6] = std::array::from_fn(|i| m[i][6] / m[i][i]);
        c.iter().all(|c| c.is_finite()).then_some(c)
    }
}

/// Nudge: slides the surface under the ring along with the stroke, within
/// the surface.
///
/// Each dab carries what is under it now a share of the way the stroke has
/// come since the dab before (`Dab::step`): all of that way under the middle
/// of the brush at full Strength, where the surface stays under the pen, and
/// less toward the rim by the falloff, so the surface is left behind as the
/// brush moves on. It goes along the surface as the dab finds it ([`Cap`]),
/// so a nudge pushes clay sideways and does not raise or sink it. A dab that
/// has come no way (the first of a stroke, a pen held still) moves nothing.
///
/// The steps of a stroke add up to the way it has come whatever its
/// Spacing, so there is no dose to apply.
fn nudge(mesh: &mut Mesh, p: &Params, d: &Dab, leaves: &[u32], fit: &[u32]) {
    if d.step == Vec3::ZERO {
        return;
    }
    let r = p.radius;
    let r2 = r * r;
    let follow = p.e(d.pressure).clamp(0.0, 1.0);
    // With no cap to go by (next to nothing under the ring) nothing is
    // moved: there is no surface to slide along.
    let Some(cap) = Cap::fit(mesh, p, d, fit) else { return };
    mesh.move_within(leaves, d.center, r2, |_, vt| {
        let pos = vt.pos;
        let d2 = pos.distance_squared(d.center);
        if d2 >= r2 {
            return None;
        }
        let w = p.falloff.at(d2.sqrt() / r, smooth)
            * (1.0 - vt.mask)
            * facing(p.front_facing, vt.normal, pos, d.cam);
        (w > 0.0).then(|| cap.slide(d.center, pos, d.step * (w * follow)))
    });
}

/// Snake Hook: carries the surface under the tip the step the tip took, by
/// the falloff's weight times Strength (all of it at 100%, where it starts)
/// times the pen's pressure, as Blender's does (not squared: its Strength
/// is how much of the way the surface follows).
///
/// Two more things, after Blender's. Thickness: the surface being carried
/// is pushed out from the line of the pull, or drawn in to it, by the step's
/// length over the radius (Blender's pinch, `2 (0.5 - pinch) |step| / r`
/// with the sign turned: Thickness 1 is pinch 0). Drawn after the tip by the
/// falloff alone, a pull thins with every step, the middle of the ring going
/// further than its edge; pushed out a little at each step, a long pull
/// keeps its thickness. Drawn in (below nought), it tapers to a point, more
/// gently the nearer the axis. And it turns with the stroke: the surface
/// under the tip is turned about it by the turn of the way the stroke is
/// going since the step before (from `dir` to `heading`), by its weight, so
/// a pull that curves keeps its cross-section square to its path.
fn snake(mesh: &mut Mesh, p: &Params, d: &Dab, leaves: &[u32]) {
    let r = p.radius;
    let r2 = r * r;
    let follow = (p.strength * d.pressure).clamp(0.0, 1.0);
    let along = d.step.normalize_or_zero();
    let pinch = -p.thickness.clamp(-1.0, 1.0) * d.step.length() / r.max(1e-12);
    // Each vertex goes its weight's share of the way to where the whole turn
    // takes it: as good as turning it that share of the angle for the small
    // turns of a step, and no sine or cosine for every vertex.
    let turn = (d.dir != Vec3::ZERO && d.heading != Vec3::ZERO && d.dir != d.heading)
        .then(|| glam::Quat::from_rotation_arc(d.dir, d.heading));
    mesh.move_within(leaves, d.center, r2, |_, vt| {
        let pos = vt.pos;
        let d2 = pos.distance_squared(d.center);
        if d2 >= r2 {
            return None;
        }
        let w = p.falloff.at(d2.sqrt() / r, smooth)
            * (1.0 - vt.mask)
            * facing(p.front_facing, vt.normal, pos, d.cam)
            * follow;
        if w <= 0.0 {
            return None;
        }
        let mut moved = d.step * w;
        if pinch != 0.0 && along != Vec3::ZERO {
            // Measured from the line the pull is going along, through
            // where the tip has got to (Blender's, which leaves out the
            // falloff here).
            let off = pos - d.center - d.step;
            let off = off - along * off.dot(along);
            // Drawn in, it eases off toward the axis, or it would pinch
            // the middle of the pull into nothing.
            let ease = if pinch > 0.0 { (off.length() / r).min(1.0).powi(2) } else { 1.0 };
            moved -= off * (pinch * w * ease);
        }
        if let Some(turn) = turn {
            let from = pos - d.center;
            moved += (turn * from - from) * w;
        }
        Some(pos + moved)
    });
}

fn mask_paint(mesh: &mut Mesh, p: &Params, d: &Dab, leaves: &[u32]) {
    let r = p.radius;
    let r2 = r * r;
    let h = p.mask_hardness.clamp(0.0, 0.999);
    let amount = p.sign * p.e(d.pressure) * (4.0 + 36.0 * h) * 0.1;
    mesh.paint_mask_within(leaves, d.center, r2, |vt| {
        let pos = vt.pos;
        let d2 = pos.distance_squared(d.center);
        if d2 >= r2 {
            return None;
        }
        let u = d2.sqrt() / r;
        // No Front Facing gate here: that setting belongs to whichever brush
        // happens to be selected, and a mask is painted where the ring is.
        let w = if u <= h { 1.0 } else { smooth(1.0 - (u - h) / (1.0 - h)) };
        Some((vt.mask + amount * w).clamp(0.0, 1.0))
    });
}

/// What Clay's finishing pass measures a vertex's curvature against: the
/// mesh's own average, and the size that makes curvature a pure shape measure.
#[derive(Clone, Copy, Default)]
pub struct CurvatureBase {
    pub(super) mean: f32,
    /// Half the largest side of the bounds.
    pub(super) scale: f32,
}

/// Mean curvature at a vertex relative to the object's size: 0 on a flat
/// surface, positive in a valley, negative on a ridge, about -0.5 all over a
/// ball. The offsets to the neighbours are summed before dividing by the
/// summed squared lengths, so the same shape reads the same at any density
/// and one stray short edge (a voxel remesh is full of them) barely moves it.
/// Summed in double: the offsets very nearly cancel on a smooth surface, so
/// the order they are added in would otherwise show, and a vertex and its
/// mirror twin list their neighbours in different orders.
#[inline]
pub(super) fn raw_curvature(mesh: &Mesh, v: u32, scale: f32) -> f32 {
    let pos = mesh.positions[v as usize];
    let mut offset = DVec3::ZERO;
    let mut sqr_len = 0.0f64;
    for &o in mesh.neighbours(v) {
        let d = mesh.positions[o as usize] - pos;
        let l = d.length_squared();
        // Welded or collapsed geometry does occur; skip it rather than
        // divide by nothing.
        if l < 1e-18 {
            continue;
        }
        offset += d.as_dvec3();
        sqr_len += l as f64;
    }
    if sqr_len <= 0.0 {
        return 0.0;
    }
    (offset.dot(mesh.normals[v as usize].as_dvec3()) / sqr_len * scale as f64) as f32
}

/// Measures a mesh's [`CurvatureBase`]. A pass over every vertex, so it is
/// taken once per mesh and kept (see `Sculptor::curvature_base`): a brush
/// changes the average curvature of a whole object next to nothing.
pub fn curvature_base(mesh: &Mesh) -> CurvatureBase {
    let bounds = mesh.bounds();
    let half = if bounds.is_empty() { 0.0 } else { bounds.size().max_element() * 0.5 };
    let scale = if half < 1e-6 { 1.0 } else { half };
    let n = mesh.vertex_count();
    if n == 0 {
        return CurvatureBase { mean: 0.0, scale };
    }
    // Totalled in fixed runs merged in order, so the answer does not depend
    // on how the work was shared between threads.
    const RUN: usize = 1 << 16;
    let totals: Vec<f64> = (0..n.div_ceil(RUN))
        .into_par_iter()
        .map(|run| {
            let verts = run * RUN..((run + 1) * RUN).clamp(0, n);
            verts.map(|v| raw_curvature(mesh, v as u32, scale) as f64).sum()
        })
        .collect();
    CurvatureBase { mean: (totals.iter().sum::<f64>() / n as f64) as f32, scale }
}

// How far a vertex's curvature has to depart from the mesh's own average
// before Clay's finishing pass ramps up to full strength. It keeps the pass
// mostly out of the way of ordinary curved shaping (a lobe's rounded tip, a
// deliberate broad bump) and concentrated on real creases and pinches. The
// numbers come from the Unity app's measurements on a 78,000-vertex sculpt:
// a rounded lobe tip read about 0.07, nine vertices in ten were under 2.9,
// ninety-nine in a hundred under 12.5, and a pinch the user reported read 21.
// The floor leaves a little everywhere, enough to tidy stray triangles.
const RELAX_CURVATURE_FLOOR: f32 = 0.08;
const RELAX_CURVATURE_START: f32 = 3.0;
const RELAX_CURVATURE_FULL: f32 = 15.0;

/// How much of the finishing pass a vertex gets, from how far its curvature
/// departs from the mesh's average.
#[inline]
pub(super) fn relax_gate(departure: f32) -> f32 {
    let ramp = (departure - RELAX_CURVATURE_START) / (RELAX_CURVATURE_FULL - RELAX_CURVATURE_START);
    RELAX_CURVATURE_FLOOR + (1.0 - RELAX_CURVATURE_FLOOR) * smooth(ramp)
}

/// One dab of a frame, for Clay's finishing pass: where it was laid, the
/// camera as that copy of the brush sees it, and the dab's radius.
#[derive(Clone, Copy)]
pub struct Centre {
    pub at: Vec3,
    pub cam: Vec3,
    pub radius: f32,
}

/// A centre further from a leaf's box than this many of its own radii is
/// left out of the leaf's weighing in Clay's finishing pass. Past one radius
/// it cannot be the dab a vertex of the leaf is deepest in, unless that
/// vertex is in none of them, and then it has no weight whichever dab is
/// found; the margin keeps rounding from ever deciding between a centre
/// that was left out and one that was not (see [`relax_weight`]).
const CULL_MARGIN: f32 = 1.01;

thread_local! {
    /// The centres near the leaf a thread is weighing, kept between leaves.
    static NEAR: std::cell::RefCell<Vec<Centre>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Clay's finishing pass, run once per frame over that frame's dabs. It only
/// works the rim of the footprint and only where the surface is sharply
/// curved, which removes the ridges dabs leave at their edges without erasing
/// what the stroke just built.
///
/// A frame can hold dozens of dabs (every copy of each under symmetry). The
/// leaves under them are found in one walk of the tree, and each vertex is
/// weighed against only the centres near its own leaf, not all of them.
#[allow(clippy::too_many_arguments)]
pub fn clay_relax(
    mesh: &mut Mesh,
    p: &Params,
    centres: &[Centre],
    dab_count: usize,
    relax: f32,
    object_size: f32,
    base: CurvatureBase,
    scratch: &mut Scratch,
) {
    if centres.is_empty() || relax <= 0.0 {
        return;
    }
    let mut leaves = std::mem::take(&mut scratch.frame_leaves);
    let Scratch { weights, starts, .. } = scratch;
    leaves.clear();
    mesh.bvh.leaves_in_spheres(centres.iter().map(|c| (c.at, c.radius)), &mut leaves);
    leaves.sort_unstable();
    let total = layout(mesh, &leaves, starts);
    weights.resize(total, 0.0);
    {
        let (mesh, leaves) = (&*mesh, &leaves[..]);
        let hidden = mesh.hidden_vertices();
        // The first dab's size, which the search for the deepest starts from
        // whichever centres a leaf keeps (see `relax_weight`).
        let first = centres[0].radius;
        each_stretch(&mut weights[..total], starts, |k, part| {
            let leaf = &mesh.bvh.leaves[leaves[k] as usize];
            let bounds = mesh.bvh.leaf_box(leaves[k]);
            NEAR.with_borrow_mut(|near| {
                near.clear();
                near.extend(centres.iter().filter(|c| bounds.dist_sq(c.at) <= (c.radius * CULL_MARGIN).powi(2)));
                for (slot, v) in part.iter_mut().zip(leaf.vert_start..leaf.vert_end) {
                    *slot = if hidden.is_some_and(|hidden| hidden[v as usize]) {
                        0.0
                    } else {
                        relax_weight(mesh, p.front_facing, near, first, base, v)
                    };
                }
            });
        });
    }
    let amount = (relax * 4.0 * dab_count as f32).min(16.0);
    let min_move = object_size / 20_000.0;
    relax_passes(mesh, &leaves, scratch, amount, min_move * min_move);
    mesh.positions_changed(&leaves);
    scratch.frame_leaves = leaves;
}

/// How much of Clay's finishing pass vertex `v` gets from the dabs `centres`
/// (those near its leaf), `first` being the radius of the frame's first.
///
/// The centres are searched in the frame's order for the one the vertex is
/// deepest inside, and the search starts from no dab at all of size `first`,
/// as it did when every vertex was weighed against every centre. A centre
/// left out ([`CULL_MARGIN`]) is a hundredth of its radius beyond the vertex
/// at the least: had it been searched, either it lost to the dab the search
/// ends on, or that dab is outside its own radius too and the vertex has no
/// weight either way. Comparisons that close to a tie are decided by
/// rounding, which is why the margin is there.
#[inline]
fn relax_weight(mesh: &Mesh, front_facing: bool, centres: &[Centre], first: f32, base: CurvatureBase, v: u32) -> f32 {
    let i = v as usize;
    let start = mesh.start_pos(v);
    // The dab the vertex is deepest inside: the nearest, or where the
    // dabs are of different sizes, the nearest for its size.
    let mut best = f32::INFINITY;
    let (mut r, mut cam) = (first, Vec3::ZERO);
    for c in centres {
        let d2 = start.distance_squared(c.at);
        let deeper = if c.radius == r { d2 < best } else { d2 * r * r < best * c.radius * c.radius };
        if deeper {
            (best, r, cam) = (d2, c.radius, c.cam);
        }
    }
    if best >= r * r {
        return 0.0;
    }
    let u = (best.sqrt() - 0.65 * r) / (0.35 * r);
    let band = if u <= 0.0 {
        0.05
    } else {
        0.05 * (1.0 - u) + 0.95 * 16.0 * u * u * (1.0 - u) * (1.0 - u)
    };
    // Curvature gate: how far this vertex's curvature departs from the
    // mesh's own average, in either direction (a recess or a ridge).
    let pos = mesh.positions[i];
    if mesh.neighbours(v).is_empty() {
        return 0.0;
    }
    let gate = relax_gate((raw_curvature(mesh, v, base.scale) - base.mean).abs());
    let w = band
        * gate
        * (1.0 - mesh.mask[i])
        * facing(front_facing, mesh.normals[i], pos, cam);
    if w > 1e-4 { w } else { 0.0 }
}

#[cfg(test)]
mod relax_cull_tests {
    use glam::Vec3;

    use super::{CULL_MARGIN, Centre, curvature_base, relax_weight};
    use crate::mesh::primitives::{self, Primitive};

    /// Weighed against only the centres near its leaf, every vertex gets
    /// the weight it gets against all of the frame's centres, to the bit:
    /// dabs of two sizes, many of them, some overlapping and some apart.
    #[test]
    fn a_vertex_weighs_the_same_against_the_centres_near_its_leaf() {
        let mesh = primitives::build(Primitive::Sphere, 40_000, 0.5);
        let base = curvature_base(&mesh);
        let centres: Vec<Centre> = (0..40)
            .map(|i| {
                let a = i as f32 * 0.7;
                let at = Vec3::new(a.cos() * 0.4, (i as f32 * 0.37).sin() * 0.3, a.sin().abs() * 0.4).normalize() * 0.5;
                Centre { at, cam: Vec3::new(0.0, 0.0, 3.0), radius: if i % 3 == 0 { 0.06 } else { 0.11 } }
            })
            .collect();
        let first = centres[0].radius;
        let mut weighted = 0;
        for l in 0..mesh.bvh.leaves.len() as u32 {
            let bounds = mesh.bvh.leaf_box(l);
            let near: Vec<Centre> =
                centres.iter().filter(|c| bounds.dist_sq(c.at) <= (c.radius * CULL_MARGIN).powi(2)).copied().collect();
            let leaf = mesh.bvh.leaves[l as usize];
            for v in leaf.vert_start..leaf.vert_end {
                let all = relax_weight(&mesh, true, &centres, first, base, v);
                let culled = relax_weight(&mesh, true, &near, first, base, v);
                assert_eq!(all.to_bits(), culled.to_bits(), "vertex {v} of leaf {l}");
                weighted += usize::from(all > 0.0);
            }
        }
        assert!(weighted > 1000, "only {weighted} vertices had weight");
    }
}

#[cfg(test)]
mod relax_cull_timing {
    use std::time::Instant;

    use glam::Vec3;

    use super::{CULL_MARGIN, Centre, curvature_base, relax_weight};
    use crate::mesh::primitives::{self, Primitive};

    /// Weighing Clay's finishing pass against every centre of a busy frame,
    /// against weighing each leaf's vertices against the centres near it.
    #[test]
    #[ignore]
    fn timing_of_weighing_against_a_frames_centres() {
        let mesh = primitives::build(Primitive::Sphere, 5_000_000, 0.5);
        let base = curvature_base(&mesh);
        for count in [2usize, 8, 24] {
            // A frame's dabs along a stroke, each with its mirror image.
            let centres: Vec<Centre> = (0..count)
                .map(|i| {
                    let at = Vec3::new(0.05 + 0.01 * (i / 2) as f32, 0.0, 0.5).normalize() * 0.5;
                    let at = if i % 2 == 0 { at } else { at * Vec3::new(-1.0, 1.0, 1.0) };
                    Centre { at, cam: Vec3::new(0.0, 0.0, 3.0), radius: 0.1 }
                })
                .collect();
            let mut leaves = Vec::new();
            mesh.bvh.leaves_in_spheres(centres.iter().map(|c| (c.at, c.radius)), &mut leaves);
            let (mut all, mut near) = (f64::MAX, f64::MAX);
            let mut sum = 0.0f32;
            for _ in 0..3 {
                let t = Instant::now();
                for &l in &leaves {
                    let leaf = mesh.bvh.leaves[l as usize];
                    for v in leaf.vert_start..leaf.vert_end {
                        sum += relax_weight(&mesh, true, &centres, 0.1, base, v);
                    }
                }
                all = all.min(t.elapsed().as_secs_f64() * 1000.0);
                let t = Instant::now();
                let mut kept = Vec::new();
                for &l in &leaves {
                    let bounds = mesh.bvh.leaf_box(l);
                    kept.clear();
                    kept.extend(centres.iter().filter(|c| bounds.dist_sq(c.at) <= (c.radius * CULL_MARGIN).powi(2)));
                    let leaf = mesh.bvh.leaves[l as usize];
                    for v in leaf.vert_start..leaf.vert_end {
                        sum += relax_weight(&mesh, true, &kept, 0.1, base, v);
                    }
                }
                near = near.min(t.elapsed().as_secs_f64() * 1000.0);
            }
            println!("{count} centres, {} leaves: all {all:.2} ms, near its leaf {near:.2} ms ({sum})", leaves.len());
        }
    }
}
