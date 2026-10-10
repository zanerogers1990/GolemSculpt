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
use super::falloff::Falloff;
use crate::mesh::Mesh;
use crate::mesh::bvh::Leaf;

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

    /// Whether this copy lies across the plane facing `axis`. Only asked of
    /// the copies of a stroke with no radial symmetry, which are all plain
    /// mirror images; a turned copy is on neither side of a plane.
    pub fn flips(self, axis: usize) -> bool {
        self.0.sign()[axis] < 0.0
    }

    /// Symmetries here go through the object's origin, so a point and a
    /// direction are mapped alike.
    #[inline]
    pub fn apply_point(self, p: Vec3) -> Vec3 {
        self.0.apply(p)
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
    pub front_facing: bool,
    pub clay_depth: f32,
    pub clay_soft: f32,
    pub crease_pinch: f32,
    pub crease_depth: f32,
    pub flatten_offset: f32,
    pub layer_height: f32,
    pub mask_hardness: f32,
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
}

impl Dab {
    pub fn mirrored(&self, op: SymOp) -> Dab {
        Dab {
            center: op.apply_point(self.center),
            normal: op.apply_dir(self.normal),
            dir: op.apply_dir(self.dir),
            step: op.apply_dir(self.step),
            prev: self.prev.map(|p| op.apply_point(p)),
            cam: op.apply_point(self.cam),
            pressure: self.pressure,
        }
    }
}

impl Params {
    fn e(&self, pressure: f32) -> f32 {
        self.strength * pressure
    }

    fn e_off(&self, pressure: f32) -> f32 {
        self.strength * pressure * self.build_up
    }

    /// Accumulating brushes compound pressure jitter, so pressure only gets
    /// half its influence there.
    fn e_acc(&self, pressure: f32) -> f32 {
        self.strength * (1.0 + (pressure - 1.0) * 0.5) * self.build_up
    }

    /// Distance between dabs, as a fraction of the radius. Mask painting has
    /// its own pace whichever brush is selected.
    pub fn spacing(&self) -> f32 {
        if self.mask { 0.2 } else { self.brush.spacing() }
    }

    /// Cost ceiling for a frame in which the pointer jumped a long way.
    pub fn max_dabs_per_frame(&self) -> usize {
        if self.mask { 24 } else { self.brush.max_dabs_per_frame() }
    }

    /// Whether the brush raises the surface to a set height in one dab and
    /// never past it (Layer). Copies of such a dab that overlap must not be
    /// added up beyond what any one of them asked for; see `radial.rs`.
    pub fn holds_a_height(&self) -> bool {
        !self.mask && self.brush == Brush::Layer
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

/// Where each leaf's vertices start in such a buffer, and the buffer's length.
pub(super) fn layout(mesh: &Mesh, leaves: &[u32]) -> (Vec<usize>, usize) {
    debug_assert!(leaves.windows(2).all(|w| w[0] < w[1]));
    let mut total = 0;
    let starts = leaves
        .iter()
        .map(|&l| {
            let start = total;
            total += owned(&mesh.bvh.leaves[l as usize]).len();
            start
        })
        .collect();
    (starts, total)
}

/// The stretch of the buffer each leaf has, from [`layout`]'s answer.
pub(super) fn spans(starts: &[usize], total: usize) -> impl Iterator<Item = Range<usize>> + '_ {
    let ends = starts.iter().skip(1).copied().chain([total]);
    starts.iter().copied().zip(ends).map(|(start, end)| start..end)
}

/// Cuts `buf` into one stretch per range. The ranges must ascend without
/// overlapping.
pub(super) fn stretches<'a, T>(
    buf: &'a mut [T],
    ranges: impl Iterator<Item = Range<usize>>,
) -> Vec<&'a mut [T]> {
    let mut rest = buf;
    let mut at = 0;
    ranges
        .map(|range| {
            let (_, tail) = std::mem::take(&mut rest).split_at_mut(range.start - at);
            let (stretch, tail) = tail.split_at_mut(range.len());
            rest = tail;
            at = range.end;
            stretch
        })
        .collect()
}

/// Runs `f` on each leaf's stretch, on all threads once there is enough to
/// share out.
pub(super) fn each_stretch<T: Send>(
    stretches: Vec<&mut [T]>,
    total: usize,
    f: impl Fn(usize, &mut [T]) + Sync,
) {
    if total < PAR_MIN {
        for (k, stretch) in stretches.into_iter().enumerate() {
            f(k, stretch);
        }
    } else {
        stretches.into_par_iter().enumerate().with_min_len(64).for_each(|(k, stretch)| f(k, stretch));
    }
}

/// Fills `out` with `f(place in the buffer, vertex)` for every vertex owned
/// by `leaves`. A hidden vertex keeps its place in the buffer and gets the
/// default value (no weight, no target) without `f` being asked.
fn fill<T: Copy + Default + Send>(
    mesh: &Mesh,
    leaves: &[u32],
    out: &mut Vec<T>,
    f: impl Fn(usize, u32) -> T + Sync,
) {
    let (starts, total) = layout(mesh, leaves);
    out.resize(total, T::default());
    let parts = stretches(out, spans(&starts, total));
    let hidden = mesh.hidden_vertices();
    each_stretch(parts, total, |k, part| {
        let verts = owned(&mesh.bvh.leaves[leaves[k] as usize]);
        let slots = part.iter_mut().zip(verts).enumerate();
        match hidden {
            None => slots.for_each(|(n, (slot, v))| *slot = f(starts[k] + n, v as u32)),
            Some(hidden) => slots.for_each(|(n, (slot, v))| {
                *slot = if hidden[v] { T::default() } else { f(starts[k] + n, v as u32) };
            }),
        }
    });
}

/// Working buffers a stroke keeps between dabs. A footprint on a dense mesh
/// is megabytes, and memory that size costs more to get fresh from the system
/// for every dab than it does to fill.
#[derive(Default)]
pub struct Scratch {
    weights: Vec<f32>,
    targets: Vec<Vec3>,
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

/// Applies one dab of the stroke's brush to the vertices in `leaves`.
pub fn apply(mesh: &mut Mesh, p: &Params, d: &Dab, leaves: &[u32], scratch: &mut Scratch) {
    if p.mask {
        return mask_paint(mesh, p, d, leaves);
    }
    match p.brush {
        Brush::Inflate => inflate(mesh, p, d, leaves),
        Brush::Standard => standard(mesh, p, d, leaves),
        Brush::Layer => layer(mesh, p, d, leaves),
        Brush::Flatten => flatten(mesh, p, d, leaves),
        Brush::Clay => clay(mesh, p, d, leaves),
        Brush::Crease => crease(mesh, p, d, leaves),
        Brush::Smooth => smooth_dab(mesh, p, d, leaves, scratch),
        Brush::SnakeHook => snake(mesh, p, d, leaves),
        // Move and Pose take hold once at the press and are dragged, not dabbed.
        Brush::Move | Brush::Pose => {}
    }
}

fn inflate(mesh: &mut Mesh, p: &Params, d: &Dab, leaves: &[u32]) {
    let r = p.radius;
    let r2 = r * r;
    let rate = p.sign * p.e_acc(d.pressure) * 1.25 * 0.1 * (r / REF_RADIUS);
    let ease = p.e_off(d.pressure) * 4.0 * 0.1;
    let target = d.center + d.normal * (0.6 * r * p.sign);
    mesh.move_vertices(leaves, |_, vt| {
        let pos = vt.pos;
        let d2 = pos.distance_squared(d.center);
        if d2 >= r2 {
            return None;
        }
        let n = vt.normal;
        let w = p.falloff.at(d2.sqrt() / r, smooth)
            * (1.0 - vt.mask)
            * facing(p.front_facing, n, pos, d.cam);
        if w <= 0.0 {
            return None;
        }
        Some(if p.accumulate { pos + n * (w * rate) } else { pos + (target - pos) * (w * ease).clamp(0.0, 1.0) })
    });
}

fn standard(mesh: &mut Mesh, p: &Params, d: &Dab, leaves: &[u32]) {
    let r = p.radius;
    let r2 = r * r;
    // Direction comes from the stroke-start surface: on the live surface the
    // brush runs away up its own ridge.
    let dir = footprint_normal(mesh, d.center, r, true).unwrap_or(d.normal);
    let rate = p.sign * p.e_acc(d.pressure) * 1.25 * 0.1 * (r / REF_RADIUS);
    let height = p.sign * 0.3 * r;
    let ease = (p.e_off(d.pressure) * 4.0 * 0.1).clamp(0.0, 1.0);
    mesh.move_vertices(leaves, |_, vt| {
        let pos = vt.pos;
        let d2 = pos.distance_squared(d.center);
        if d2 >= r2 {
            return None;
        }
        let w = p.falloff.at(d2.sqrt() / r, quintic)
            * (1.0 - vt.mask)
            * facing(p.front_facing, vt.normal, pos, d.cam);
        if w <= 0.0 {
            return None;
        }
        if p.accumulate {
            return Some(pos + dir * (rate * w));
        }
        let remaining = height * w - (pos - vt.start).dot(dir);
        (remaining * height > 0.0).then(|| pos + dir * (remaining * ease))
    });
}

fn layer(mesh: &mut Mesh, p: &Params, d: &Dab, leaves: &[u32]) {
    let r = p.radius;
    let r2 = r * r;
    let dir = footprint_normal(mesh, d.center, r, false).unwrap_or(d.normal);
    let height = p.sign * r * p.layer_height;
    let ease = (p.e_off(d.pressure) * 40.0 * 0.1).clamp(0.0, 1.0);
    mesh.move_vertices(leaves, |_, vt| {
        let pos = vt.pos;
        let d2 = pos.distance_squared(d.center);
        if d2 >= r2 {
            return None;
        }
        // Flat plateau in the middle so overlapping dabs build one even step.
        let w = p.falloff.at(d2.sqrt() / r, layer_built_in)
            * (1.0 - vt.mask)
            * facing(p.front_facing, vt.normal, pos, d.cam);
        if w <= 0.0 {
            return None;
        }
        // Height is measured from the stroke-start surface, so crossing your
        // own stroke does not stack a second layer.
        let remaining = height * w - (pos - vt.start).dot(dir);
        (remaining * height > 0.0).then(|| pos + dir * (remaining * ease))
    });
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

fn flatten(mesh: &mut Mesh, p: &Params, d: &Dab, leaves: &[u32]) {
    let r = p.radius;
    let r2 = r * r;
    // The plane is fitted with mask-free weights: a mask edge would otherwise
    // tilt it and raise a ridge along the mask boundary.
    let plane_w = |pos: Vec3, normal: Vec3| {
        let d2 = pos.distance_squared(d.center);
        if d2 >= r2 {
            return 0.0;
        }
        p.falloff.at(d2.sqrt() / r, quintic) * facing(p.front_facing, normal, pos, d.cam)
    };
    // Origin from stroke-start positions: with a live origin a non-zero
    // offset drills or balloons without limit.
    let fit = {
        let mesh = &*mesh;
        fold_leaves(
            mesh,
            leaves,
            |sum: &mut PlaneSum, v| {
                let i = v as usize;
                let w = plane_w(mesh.positions[i], mesh.normals[i]);
                if w > 0.0 {
                    sum.add(mesh.start_pos(v), mesh.normals[i], w);
                }
            },
            PlaneSum::merge,
        )
    };
    let Some((origin, n)) = fit.plane(d.normal) else { return };
    let origin = origin + n * (r * p.flatten_offset);
    let e = p.e(d.pressure) * 4.0 * 0.1;
    mesh.move_vertices(leaves, |_, vt| {
        let pos = vt.pos;
        let fit_w = plane_w(pos, vt.normal);
        if fit_w <= 0.0 {
            return None;
        }
        let w = fit_w * (1.0 - vt.mask);
        let along = (pos - origin).dot(n);
        let k = (w * e).clamp(0.0, 1.0);
        Some(if p.sign >= 0.0 {
            pos - n * (along * k)
        } else {
            // Inverted: push away from the plane (contrast), bounded per stroke.
            let new = pos + n * (along * k);
            let h = (new - vt.start).dot(n);
            new - n * (h - h.clamp(-0.5 * r, 0.5 * r))
        })
    });
}

fn clay(mesh: &mut Mesh, p: &Params, d: &Dab, leaves: &[u32]) {
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
            leaves,
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

    let per_dab = {
        let base = p.e(pressure) * 4.0 * 0.05;
        if p.accumulate { base.max(p.e_acc(pressure) * 4.0 * 0.05 / 0.25) } else { base }
    };
    // Strong dabs are swept from the previous centre instead of stamped;
    // whole stamps print their rims as a row of crescents.
    let steps = match d.prev {
        Some(_) => ((per_dab / 0.15).ceil().max((0.2 * 3.0 / soft - 1e-3).ceil()) as usize).clamp(1, 8),
        None => 1,
    };
    let prev = d.prev.unwrap_or(d.center);
    let lerp_scale = p.e(pressure) * 4.0 * 0.05 / steps as f32;
    let rate = p.sign * p.clay_depth * p.e_acc(pressure) * 4.0 * 0.05 * (r / REF_RADIUS)
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
            let w = if steps == 1 {
                tip_w(start, vt.normal, vt.pos) * (1.0 - vt.mask)
            } else {
                let centre = prev.lerp(d.center, s as f32 / steps as f32);
                clay_weight(&p.falloff, start.distance(centre) / eff_r, soft) * guard
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
    let e_acc = p.e_acc(d.pressure) * 4.0 * 0.2;
    mesh.move_vertices(leaves, |_, vt| {
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
        Some(pos + (target - pos) * (w * e_off).clamp(0.0, 1.0))
    });
}

/// Mean of a vertex's edge neighbours, summed in double (see module docs).
#[inline]
fn neighbour_mean(mesh: &Mesh, v: u32) -> Option<Vec3> {
    let ring = mesh.neighbours(v);
    if ring.is_empty() {
        return None;
    }
    let mut sum = DVec3::ZERO;
    for &o in ring {
        sum += mesh.positions[o as usize].as_dvec3();
    }
    Some((sum / ring.len() as f64).as_vec3())
}

/// Jacobi relaxation passes over the vertices of `leaves` that
/// `scratch.weights` gives a weight above zero: every target is computed from
/// the pass's starting positions, then all are written. (Updating in place is
/// order dependent and breaks mirror symmetry.)
fn relax_passes(mesh: &mut Mesh, leaves: &[u32], scratch: &mut Scratch, amount: f32, min_move2: f32) {
    let full = amount.floor() as usize;
    let frac = amount - full as f32;
    let passes = full + usize::from(frac > 0.001);
    let Scratch { weights, targets } = scratch;
    let weights = &*weights;
    for pass in 0..passes {
        let factor = if pass < full { 1.0 } else { frac };
        {
            let mesh = &*mesh;
            fill(mesh, leaves, targets, |at, v| {
                let w = weights[at];
                if w <= 0.0 {
                    return Vec3::ZERO;
                }
                let pos = mesh.positions[v as usize];
                match neighbour_mean(mesh, v) {
                    Some(avg) => pos + (avg - pos) * (w * factor).clamp(0.0, 1.0),
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

fn smooth_dab(mesh: &mut Mesh, p: &Params, d: &Dab, leaves: &[u32], scratch: &mut Scratch) {
    let r = p.radius;
    let r2 = r * r;
    {
        let mesh = &*mesh;
        fill(mesh, leaves, &mut scratch.weights, |_, v| {
            let i = v as usize;
            let pos = mesh.positions[i];
            let d2 = pos.distance_squared(d.center);
            if d2 >= r2 {
                return 0.0;
            }
            let w = p.falloff.at(d2.sqrt() / r, smooth)
                * (1.0 - mesh.mask[i])
                * facing(p.front_facing, mesh.normals[i], pos, d.cam);
            if w > 0.0 { w * 4.0 * 0.1 } else { 0.0 }
        });
    }
    relax_passes(mesh, leaves, scratch, p.e(d.pressure) * 10.0, 0.0);
}

fn snake(mesh: &mut Mesh, p: &Params, d: &Dab, leaves: &[u32]) {
    let r = p.radius;
    let r2 = r * r;
    mesh.move_vertices(leaves, |_, vt| {
        let pos = vt.pos;
        let d2 = pos.distance_squared(d.center);
        if d2 >= r2 {
            return None;
        }
        let w = p.falloff.at(d2.sqrt() / r, smooth)
            * (1.0 - vt.mask)
            * facing(p.front_facing, vt.normal, pos, d.cam);
        (w > 0.0).then(|| pos + d.step * w)
    });
}

fn mask_paint(mesh: &mut Mesh, p: &Params, d: &Dab, leaves: &[u32]) {
    let r = p.radius;
    let r2 = r * r;
    let h = p.mask_hardness.clamp(0.0, 0.999);
    let amount = p.sign * p.e(d.pressure) * (4.0 + 36.0 * h) * 0.1;
    mesh.paint_mask(leaves, |vt| {
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

/// Clay's finishing pass, run once per frame over that frame's dabs. It only
/// works the rim of the footprint and only where the surface is sharply
/// curved, which removes the ridges dabs leave at their edges without erasing
/// what the stroke just built.
#[allow(clippy::too_many_arguments)]
pub fn clay_relax(
    mesh: &mut Mesh,
    p: &Params,
    centres: &[(Vec3, Vec3)],
    dab_count: usize,
    relax: f32,
    object_size: f32,
    base: CurvatureBase,
    scratch: &mut Scratch,
) {
    if centres.is_empty() || relax <= 0.0 {
        return;
    }
    let r = p.radius;
    let r2 = r * r;
    let mut leaves = Vec::new();
    for &(c, _) in centres {
        mesh.bvh.leaves_in_sphere(c, r, &mut leaves);
    }
    leaves.sort_unstable();
    leaves.dedup();

    let weigh = |mesh: &Mesh, v: u32| {
        let i = v as usize;
        let start = mesh.start_pos(v);
        let mut best = f32::INFINITY;
        let mut cam = Vec3::ZERO;
        for &(c, cm) in centres {
            let d2 = start.distance_squared(c);
            if d2 < best {
                best = d2;
                cam = cm;
            }
        }
        if best >= r2 {
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
            * facing(p.front_facing, mesh.normals[i], pos, cam);
        if w > 1e-4 { w } else { 0.0 }
    };
    {
        let mesh = &*mesh;
        fill(mesh, &leaves, &mut scratch.weights, |_, v| weigh(mesh, v));
    }
    let amount = (relax * 4.0 * dab_count as f32).min(16.0);
    let min_move = object_size / 20_000.0;
    relax_passes(mesh, &leaves, scratch, amount, min_move * min_move);
    mesh.positions_changed(&leaves);
}
