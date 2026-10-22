//! The Pose brush: bends, twists, resizes or stretches a limb about a joint
//! the brush finds for itself, modelled on Blender's sculpt-mode Pose brush.
//!
//! On a press the brush floods over the surface from the point under the
//! pointer as far as the brush ring, and puts a joint in the middle of where
//! the flood left the ring (`build`). What lies on the pointer's side of the
//! joint is the limb; the drag then turns it about the joint, and with more
//! than one joint a chain of them reaches for the pointer while its far end
//! stays put (`solve`). Like Move, the vertices are picked once at the press
//! and placed afresh from where they started on every frame of the drag.
//!
//! Under symmetry every copy of the brush finds a chain of its own on its
//! own side of the model, from the stroke's list of symmetry operations.
//! Two things can then happen where copies meet:
//!
//! - Copies that took hold of the same limb (a tail or a neck on the mirror
//!   plane, which both floods cover) pose it once, toward the average of
//!   where each would take it. For a mirror pair that is the pointer's motion
//!   in the mirror plane, the only pose that keeps the limb symmetric. Left
//!   to pose it one after the other, as the Unity app first did, the last
//!   copy won: dragged to the right, a tail on the centre line swung left.
//! - Copies on limbs of their own that merely touch (two legs at the crotch)
//!   share the pull where they overlap, the way Move does.

mod build;
mod solve;
#[cfg(test)]
mod tests;

use glam::{DVec3, Vec2, Vec3};
use rayon::prelude::*;

use super::falloff::Falloff;
use super::kernels::{self, SymOp};
use super::{Brush, Sculptor};
use crate::camera::View;
use crate::mesh::{Mesh, StrokeDelta};
use build::{Ask, Hold, Scratch, Skeleton};
use solve::Pose;

/// What a drag of the Pose brush does to the limb.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PoseMode {
    /// Turn it about the joint; with Ctrl, twist it about its own length
    /// (Blender's Rotate / Twist).
    #[default]
    Bend,
    /// Grow or shrink it about the joint; with Ctrl, slide it without
    /// turning (Blender's Scale / Translate).
    Resize,
    /// Lengthen or shorten it, keeping its volume (Blender's Squash &
    /// Stretch).
    Stretch,
}

impl PoseMode {
    pub const ALL: [PoseMode; 3] = [PoseMode::Bend, PoseMode::Resize, PoseMode::Stretch];

    pub fn name(self) -> &'static str {
        match self {
            PoseMode::Bend => "Bend",
            PoseMode::Resize => "Resize",
            PoseMode::Stretch => "Stretch",
        }
    }

    /// What a drag does, and what Ctrl (or the right button) changes it to.
    pub fn help(self) -> &'static str {
        match self {
            PoseMode::Bend => {
                "Drag to bend the limb at the joint. Hold Ctrl, or use the right button, to twist it instead."
            }
            PoseMode::Resize => {
                "Drag away from the joint to make that part bigger, toward the joint to make it smaller. \
                 Hold Ctrl, or use the right button, to slide it without turning."
            }
            PoseMode::Stretch => "Drag along the limb to make it longer and thinner, or shorter and fatter.",
        }
    }
}

/// The Pose brush's own settings.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PoseSettings {
    pub mode: PoseMode,
    /// How many joints a bend has (Blender's IK Segments). Resize and
    /// Stretch always work about one.
    pub joints: u32,
    /// How much further up the limb than the brush ring the first joint
    /// sits, in brush sizes (Blender's Pose Origin Offset).
    pub offset: f32,
    /// How sharp a bend is: 0 bends in a long soft curve, 1 like a hinge.
    /// Named, and turned this way round, as in the Unity app.
    pub rigidity: f32,
}

impl Default for PoseSettings {
    fn default() -> Self {
        Self { mode: PoseMode::Bend, joints: 1, offset: 0.0, rigidity: 0.6 }
    }
}

impl PoseSettings {
    pub const MAX_JOINTS: u32 = build::MAX_JOINTS as u32;
    pub const MAX_OFFSET: f32 = 2.0;

    /// The joints a press makes: what is set, within what a chain can hold.
    fn joint_count(&self) -> usize {
        match self.mode {
            PoseMode::Bend => self.joints.clamp(1, Self::MAX_JOINTS) as usize,
            PoseMode::Resize | PoseMode::Stretch => 1,
        }
    }

    /// How far to each side of a joint its bend reaches, along the surface.
    /// Squared, so that most of the slider's travel is spent on the firm
    /// bends a joint usually wants and the last of it reaches a bend as long
    /// as the brush is wide.
    fn half_band(&self, radius: f32) -> f32 {
        let soft = 1.0 - self.rigidity.clamp(0.0, 1.0);
        radius * soft * soft
    }
}

/// Most vertices the hover preview walks beyond the brush ring for one copy
/// of the brush. Far more than a limb needs; it only binds with the pointer
/// on the trunk of a very dense model, where telling which half is the limb
/// means crossing most of the mesh.
const HOVER_BUDGET: usize = 400_000;

/// The nearest vertex that shows to `to`, no further off than `within`.
fn nearest_vertex(mesh: &Mesh, to: Vec3, within: f32) -> Option<u32> {
    let hidden = mesh.hidden_vertices();
    let mut leaves = Vec::new();
    // The point is on the surface, or its mirror image is, so a vertex is
    // nearly always right beside it: look there before looking everywhere.
    for reach in [within * 0.125, within] {
        leaves.clear();
        mesh.bvh.leaves_in_sphere(to, reach, &mut leaves);
        let mut best = (reach * reach, None);
        for &l in &leaves {
            let leaf = &mesh.bvh.leaves[l as usize];
            for v in leaf.vert_start..leaf.vert_end {
                let d2 = mesh.positions[v as usize].distance_squared(to);
                // Nor one dynamic topology merged away, which is in no triangle.
                if d2 < best.0 && !hidden.is_some_and(|h| h[v as usize]) && !mesh.neighbours(v).is_empty() {
                    best = (d2, Some(v));
                }
            }
        }
        if best.1.is_some() {
            return best.1;
        }
    }
    None
}

/// What one copy of the brush found.
struct Found {
    /// Which of the stroke's symmetry operations this copy is.
    op: usize,
    skeleton: Skeleton,
    /// The copies (by their place in the list of what was found) whose
    /// grabbed vertex this copy's chain carries.
    carries: Vec<bool>,
}

/// The chain each copy of the brush makes of a press at `at`. A copy that
/// lands on no surface makes none. `budget` and `core` are [`Ask`]'s. `each`
/// is handed every chain as it is found, while `scratch` still describes it.
#[allow(clippy::too_many_arguments)]
fn find(
    mesh: &Mesh,
    scratch: &mut Scratch,
    settings: &PoseSettings,
    ops: &[SymOp],
    at: Vec3,
    radius: f32,
    (budget, core): (Option<usize>, bool),
    mut each: impl FnMut(usize, &Skeleton, &mut Scratch),
) -> Vec<Found> {
    // A hold names its copy in a byte.
    let seeds: Vec<(usize, Vec3, u32)> = ops
        .iter()
        .enumerate()
        .take(u8::MAX as usize)
        .filter_map(|(op, copy)| {
            let centre = copy.apply_point(at);
            Some((op, centre, nearest_vertex(mesh, centre, radius)?))
        })
        .collect();
    let mut found = Vec::with_capacity(seeds.len());
    for (i, &(op, centre, seed)) in seeds.iter().enumerate() {
        let ask = Ask {
            centre,
            seed,
            radius,
            offset: settings.offset.clamp(0.0, PoseSettings::MAX_OFFSET),
            joints: settings.joint_count(),
            budget,
            core,
        };
        let skeleton = build::skeleton(mesh, scratch, &ask);
        let carries = seeds.iter().map(|&(_, _, other)| scratch.part_of(other).is_some()).collect();
        each(i, &skeleton, scratch);
        found.push(Found { op, skeleton, carries });
    }
    found
}

/// Sorts what was found into limbs: copies whose chains each carry the
/// other's grabbed vertex have hold of the same limb. Returns the limb each
/// copy belongs to, numbered from 0 in the order the copies come.
fn limbs(found: &[Found]) -> Vec<usize> {
    let mut limb: Vec<usize> = (0..found.len()).collect();
    for i in 0..found.len() {
        for j in 0..i {
            let same = found[i].carries[j]
                && found[j].carries[i]
                && found[i].skeleton.joints.len() == found[j].skeleton.joints.len();
            if same {
                let (from, to) = (limb[i], limb[j]);
                limb.iter_mut().filter(|l| **l == from).for_each(|l| *l = to);
            }
        }
    }
    // Numbered without gaps.
    let mut numbered: Vec<usize> = Vec::new();
    limb.iter()
        .map(|&l| {
            numbered.iter().position(|&seen| seen == l).unwrap_or_else(|| {
                numbered.push(l);
                numbered.len() - 1
            })
        })
        .collect()
}

/// The one chain that stands for several copies' chains on the same limb:
/// their average, joint for joint. Summed in double, so that a mirror pair's
/// average lies on the mirror plane.
fn shared(skeletons: &[&Skeleton]) -> Skeleton {
    if let [only] = skeletons {
        return (*only).clone();
    }
    let mean = |pick: &dyn Fn(&Skeleton) -> Vec3| {
        let sum: DVec3 = skeletons.iter().map(|s| pick(s).as_dvec3()).sum();
        (sum / skeletons.len() as f64).as_vec3()
    };
    Skeleton {
        head: mean(&|s| s.head),
        joints: (0..skeletons[0].joints.len()).map(|k| mean(&|s| s.joints[k])).collect(),
        core: mean(&|s| s.core),
        girth: skeletons.iter().map(|s| s.girth).sum::<f32>() / skeletons.len() as f32,
    }
}

/// One copy of the brush with hold of a limb.
struct Member {
    op: usize,
    /// Twist: the way across the screen the surface facing the camera goes
    /// for a positive turn about the limb, and how far the pointer travels
    /// for one radian.
    roll: Vec3,
    roll_radius: f32,
    /// 1, or -1 if this copy's limb runs the other way to the shared chain.
    along: f32,
}

/// A limb during a drag: its chain, and the copies of the brush on it.
struct Limb {
    skeleton: Skeleton,
    members: Vec<Member>,
    /// The copies on it leave it no way to turn (see [`pinned`]).
    pinned: bool,
    pose: Pose,
}

/// Whether the copies of the brush on one limb, between them, leave it
/// nowhere to bend.
///
/// Copies with hold of the same limb are symmetries of that limb, and it can
/// only be posed in a way they all agree on. A mirror pair agree on the
/// mirror plane, and the limb bends within it. The turns of radial symmetry
/// about a limb that stands on the axis (the stem of a vase, the spike on a
/// helmet) agree on nothing but the axis itself, and so do two mirror planes
/// that cross along the limb: the place the copies would take it to between
/// them is always back on its own line. Pointing the limb at that place is
/// no bend at all, until the pointer passes the joint and it is the limb
/// turned end for end, about whichever way rounding picks. Such a limb is
/// left where it is by a bend, and is resized without being turned; it can
/// still be twisted about the axis (unless a mirror is on it too, which
/// undoes the twist), slid along it and stretched.
///
/// What the copies agree on is what their maps all leave where it is, and
/// the trace of the average of those maps (each taken back through the
/// first, so that a limb which is some other copy's limb is judged as itself)
/// is how many directions that is: 3 for one copy, 2 for a mirror pair, 1 for
/// a line.
fn pinned(ops: &[SymOp], members: &[Member]) -> bool {
    let first = &ops[members[0].op];
    let trace: f32 = members
        .iter()
        .flat_map(|m| Vec3::AXES.map(|axis| ops[m.op].apply_dir(axis).dot(first.apply_dir(axis))))
        .sum();
    trace / (members.len() as f32) < 1.5
}

/// `goal` brought onto the limb's own line, on the grabbed point's side of
/// the first joint: where a limb that cannot turn is resized toward.
fn along_the_limb(skeleton: &Skeleton, goal: Vec3) -> Vec3 {
    let joint = skeleton.joints[0];
    let reach = skeleton.head - joint;
    let Some(line) = reach.try_normalize() else { return skeleton.head };
    // Never back through the joint, which would be the limb inside out.
    joint + line * (goal - joint).dot(line).max(1e-3 * reach.length())
}

/// One vertex's place in one limb's chain: it follows `share` of what joint
/// `joint` does, and all of what the joints after it do.
#[derive(Clone, Copy)]
struct Grip {
    limb: u8,
    joint: u8,
    share: f32,
}

/// A Pose drag, press to release.
struct Drag {
    mode: PoseMode,
    /// Ctrl or the right button was down at the press: twist, or slide.
    other: bool,
    falloff: Falloff,
    limbs: Vec<Limb>,
    /// Every vertex some limb holds, in ascending order (which lets the drag
    /// move them on all threads), and where each one's grips are.
    verts: Vec<u32>,
    first: Vec<u32>,
    grips: Vec<Grip>,
    leaves: Vec<u32>,
    /// The grabbed point: the drag plane faces the camera through it.
    origin: Vec3,
    pressed_at: Vec2,
    dirty: bool,
}

/// What the Pose brush keeps on the sculptor between frames.
#[derive(Default)]
pub struct State {
    scratch: Scratch,
    /// The chains a press would make where the pointer rests: the grabbed
    /// point, then the joints.
    hover: Vec<Vec<Vec3>>,
    drag: Option<Drag>,
}

/// Whether mirroring through `op` swaps left for right, which turns a twist
/// the other way.
fn handedness(op: &SymOp) -> f32 {
    let det = op.apply_dir(Vec3::X).cross(op.apply_dir(Vec3::Y)).dot(op.apply_dir(Vec3::Z));
    if det < 0.0 { -1.0 } else { 1.0 }
}

/// How far along a chain of `joints` joints a hold is: 0 at the grabbed end,
/// a whole number at each joint ring's far edge, `joints` where the chain
/// lets go.
fn depth(joint: u8, share: f32) -> f32 {
    joint as f32 + (1.0 - share)
}

impl Drag {
    #[allow(clippy::too_many_arguments)]
    fn begin(
        mesh: &Mesh,
        scratch: &mut Scratch,
        settings: &PoseSettings,
        falloff: Falloff,
        ops: &[SymOp],
        at: Vec3,
        radius: f32,
        other: bool,
        view: &View,
        pressed_at: Vec2,
    ) -> Option<Drag> {
        let half_band = settings.half_band(radius);
        // Every copy's hold on every vertex it holds: (vertex, copy, depth).
        let mut held: Vec<(u32, u8, f32)> = Vec::new();
        let mut holds: Vec<Hold> = Vec::new();
        // The limb's own line is what a twist turns about and a stretch
        // runs along; a bend or a resize does not need it found.
        let lined = matches!((settings.mode, other), (PoseMode::Bend, true) | (PoseMode::Stretch, _));
        let found = find(mesh, scratch, settings, ops, at, radius, (None, lined), |copy, skeleton, scratch| {
            build::holds(mesh, scratch, skeleton.joints.len(), half_band, &falloff, &mut holds);
            held.extend(holds.iter().map(|h| (h.vert, copy as u8, depth(h.joint, h.share))));
        });
        if held.is_empty() {
            return None;
        }

        let limb_of = limbs(&found);
        let count = limb_of.iter().max().map_or(0, |&last| last + 1);
        let limbs: Vec<Limb> = (0..count)
            .map(|limb| {
                let copies: Vec<&Found> =
                    found.iter().zip(&limb_of).filter(|&(_, &l)| l == limb).map(|(f, _)| f).collect();
                let skeletons: Vec<&Skeleton> = copies.iter().map(|f| &f.skeleton).collect();
                let skeleton = shared(&skeletons);
                let line = solve::grain(&skeleton);
                let members = copies
                    .iter()
                    .map(|f| {
                        // The surface that faces the camera follows the
                        // pointer round the limb: for a positive turn about
                        // the limb's line it goes the way `roll` points. A
                        // limb seen end-on has no such way, and turns with a
                        // drag to the right instead, as Blender's does.
                        let own = solve::grain(&f.skeleton);
                        let across = own.cross(ops[f.op].apply_dir(-view.forward));
                        let roll = if across.length() > 0.25 {
                            across.normalize()
                        } else {
                            ops[f.op].apply_dir(view.right) * handedness(&ops[f.op])
                        };
                        // Grabbed on its side, the limb turns so that the
                        // grabbed skin stays under the pointer. Grabbed on
                        // its tip there is no skin to follow, and the limb's
                        // own thickness sets the pace.
                        let spoke = f.skeleton.head - f.skeleton.joints[0];
                        let slowest = if f.skeleton.girth > 0.0 { 0.5 * f.skeleton.girth } else { 0.25 * radius };
                        let roll_radius = (spoke - own * spoke.dot(own)).length().max(slowest);
                        Member { op: f.op, roll, roll_radius, along: if own.dot(line) < 0.0 { -1.0 } else { 1.0 } }
                    })
                    .collect::<Vec<Member>>();
                let pose = Pose::rest(&skeleton);
                let pinned = pinned(ops, &members);
                Limb { skeleton, members, pinned, pose }
            })
            .collect();

        // A limb's hold on a vertex is the average of its copies' holds,
        // a copy that does not hold the vertex counting as letting go.
        let key = |&(v, copy, _): &(u32, u8, f32)| (v, limb_of[copy as usize]);
        if held.len() < kernels::PAR_MIN {
            held.sort_unstable_by_key(key);
        } else {
            held.par_sort_unstable_by_key(key);
        }
        let mut verts = Vec::with_capacity(held.len());
        let mut first = Vec::with_capacity(held.len() + 1);
        let mut grips = Vec::with_capacity(held.len());
        for run in held.chunk_by(|a, b| key(a) == key(b)) {
            let (v, limb) = key(&run[0]);
            let (joints, copies) = (limbs[limb].skeleton.joints.len(), limbs[limb].members.len());
            let total: f32 = run.iter().map(|&(_, _, depth)| depth).sum();
            let depth = (total + (copies - run.len()) as f32 * joints as f32) / copies as f32;
            if depth >= joints as f32 {
                continue;
            }
            let joint = depth.floor().clamp(0.0, (joints - 1) as f32);
            if verts.last() != Some(&v) {
                verts.push(v);
                first.push(grips.len() as u32);
            }
            grips.push(Grip { limb: limb as u8, joint: joint as u8, share: (1.0 - (depth - joint)).clamp(0.0, 1.0) });
        }
        first.push(grips.len() as u32);
        let leaves = mesh.leaves_of(&verts);
        let (mode, origin) = (settings.mode, at);
        Some(Drag { mode, other, falloff, limbs, verts, first, grips, leaves, origin, pressed_at, dirty: false })
    }

    /// Puts the surface where the drag has it with the pointer at `pointer`.
    /// Returns where the grabbed point is now.
    fn apply(&mut self, mesh: &mut Mesh, ops: &[SymOp], view: &View, pointer: Vec2) -> Option<Vec3> {
        // The pointer's place on the plane that faces the camera through
        // the grabbed point, as for Move.
        let (from, dir) = view.ray(pointer);
        let denom = dir.dot(view.forward);
        if denom.abs() <= 1e-6 {
            return None;
        }
        let target = from + dir * ((self.origin - from).dot(view.forward) / denom);
        let dragged = target - self.origin;
        // Back on the pixel of the press is back at rest, exactly: the ray
        // through it does not land on the grabbed point to the last bit.
        let rest = pointer == self.pressed_at;
        let (mode, other, falloff) = (self.mode, self.other, self.falloff);
        let full = falloff.at(0.0, kernels::smooth).max(1e-6);
        for limb in &mut self.limbs {
            let (s, copies) = (&limb.skeleton, limb.members.len() as f32);
            // Where the copies between them would take the limb.
            let mean = |of: &dyn Fn(&Member) -> Vec3| limb.members.iter().map(of).sum::<Vec3>() / copies;
            limb.pose = match (mode, other) {
                _ if rest => Pose::rest(s),
                (PoseMode::Bend, false) if limb.pinned => Pose::rest(s),
                (PoseMode::Resize, false) if limb.pinned => {
                    solve::resize(s, along_the_limb(s, mean(&|m| ops[m.op].apply_point(target))))
                }
                (PoseMode::Bend, false) => solve::bend(s, mean(&|m| ops[m.op].apply_point(target))),
                (PoseMode::Bend, true) => {
                    let turn = |m: &Member| ops[m.op].apply_dir(dragged).dot(m.roll) / m.roll_radius * m.along;
                    let angle = limb.members.iter().map(turn).sum::<f32>() / copies;
                    // The brush's falloff, read along the chain: its centre
                    // is the grabbed end, its edge the anchor.
                    solve::twist(s, angle, |along| (falloff.at(along, kernels::smooth) / full).clamp(0.0, 1.0))
                }
                (PoseMode::Resize, false) => solve::resize(s, mean(&|m| ops[m.op].apply_point(target))),
                (PoseMode::Resize, true) => solve::slide(s, mean(&|m| ops[m.op].apply_dir(dragged))),
                (PoseMode::Stretch, _) => solve::stretch(s, mean(&|m| ops[m.op].apply_point(target))),
            };
        }

        let (limbs, first, grips) = (&self.limbs, &self.first, &self.grips);
        mesh.move_listed(&self.verts, |slot, vt| {
            let free = 1.0 - vt.mask;
            if free <= 0.0 {
                return None;
            }
            let grips = &grips[first[slot] as usize..first[slot + 1] as usize];
            if let ([grip], true) = (grips, free >= 1.0) {
                // The usual case, kept to the one transform: a limb that is
                // carried whole comes out exactly rigid.
                return Some(limbs[grip.limb as usize].pose.place(grip.joint as usize, grip.share, vt.start));
            }
            let mut moved = Vec3::ZERO;
            let mut held = 0.0;
            for grip in grips {
                let limb = &limbs[grip.limb as usize];
                moved += limb.pose.place(grip.joint as usize, grip.share, vt.start) - vt.start;
                let last = grip.joint as usize + 1 == limb.skeleton.joints.len();
                held += if last { grip.share } else { 1.0 };
            }
            // Where limbs overlap, share the pull, as Move does.
            if held > 1.0 {
                moved /= held;
            }
            Some(vt.start + moved * free)
        });
        mesh.positions_changed(&self.leaves);
        self.limbs.first().map(|limb| limb.pose.points[0])
    }
}

impl State {
    /// Takes hold for a drag that starts at `at`, a point of the surface.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn begin(
        &mut self,
        mesh: &Mesh,
        settings: &PoseSettings,
        falloff: Falloff,
        ops: &[SymOp],
        at: Vec3,
        radius: f32,
        other: bool,
        view: &View,
        pointer: Vec2,
    ) {
        self.hover.clear();
        self.drag = Drag::begin(mesh, &mut self.scratch, settings, falloff, ops, at, radius, other, view, pointer);
    }

    /// The pointer has moved: the surface follows on the next frame.
    pub(super) fn touch(&mut self) {
        if let Some(drag) = &mut self.drag {
            drag.dirty = true;
        }
    }

    /// One frame of the drag. Returns where the grabbed point is, if the
    /// surface moved.
    pub(super) fn frame(&mut self, mesh: &mut Mesh, ops: &[SymOp], view: &View, pointer: Vec2) -> Option<Vec3> {
        let drag = self.drag.as_mut().filter(|drag| drag.dirty)?;
        drag.dirty = false;
        drag.apply(mesh, ops, view, pointer)
    }

    /// Lets go. `delta` is the stroke's undo record, which is handed back
    /// unless the drag ended where it began: the mesh notes every vertex a
    /// drag has moved, and a limb dragged out and back again has moved and
    /// is exactly where it was, which is nothing to undo.
    pub(super) fn end(&mut self, delta: Option<StrokeDelta>) -> Option<StrokeDelta> {
        if self.drag.take().is_none() {
            return delta;
        }
        match delta {
            Some(StrokeDelta::Positions { before, after, .. }) if before == after => None,
            delta => delta,
        }
    }

    /// The chains to draw: the drag's as they now stand, else the ones a
    /// press would make. Each is the grabbed point followed by its joints,
    /// the last of which is the one that stays still.
    pub fn chains(&self) -> Vec<&[Vec3]> {
        match &self.drag {
            Some(drag) => drag.limbs.iter().map(|limb| &limb.pose.points[..]).collect(),
            None => self.hover.iter().map(|points| &points[..]).collect(),
        }
    }
}

impl Sculptor {
    /// Keeps the Pose brush's preview current. Call once a frame with the
    /// point of the model the brush is over and the brush's radius there, or
    /// `None` when the pointer is off the model or busy with something else.
    /// Costs nothing while another brush is in hand.
    pub fn pose_hover(&mut self, mesh: &Mesh, under: Option<(Vec3, f32)>, smooth_held: bool) {
        self.pose.hover.clear();
        if self.stroke.is_some() {
            // A stroke shows its own chains, if it is a Pose drag, and
            // otherwise none: not what the pointer rested on before it.
            return;
        }
        if self.settings.mask_mode || self.settings.brush != Brush::Pose {
            // The working memory is a few numbers per vertex of the mesh:
            // not worth keeping while another brush is in hand.
            self.pose.scratch = Scratch::default();
            return;
        }
        let Some((at, radius)) = under.filter(|_| !smooth_held) else { return };
        let ops = self.settings.ops();
        let settings = self.settings.pose;
        let within = (Some(HOVER_BUDGET), false);
        let found = find(mesh, &mut self.pose.scratch, &settings, &ops, at, radius, within, |_, _, _| {});
        let limb_of = limbs(&found);
        for limb in 0..limb_of.iter().max().map_or(0, |&last| last + 1) {
            let skeletons: Vec<&Skeleton> =
                found.iter().zip(&limb_of).filter(|&(_, &l)| l == limb).map(|(f, _)| &f.skeleton).collect();
            let skeleton = shared(&skeletons);
            let mut points = vec![skeleton.head];
            points.extend(skeleton.joints);
            self.pose.hover.push(points);
        }
    }

    /// The Pose brush's chains for the preview (see [`State::chains`]).
    /// Empty while there is nothing to show.
    pub fn pose_chains(&self) -> Vec<&[Vec3]> {
        self.pose.chains()
    }
}
