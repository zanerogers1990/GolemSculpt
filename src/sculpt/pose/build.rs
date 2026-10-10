//! Finding what the Pose brush takes hold of: where the joints are, and how
//! much of each joint's move every vertex follows.
//!
//! This is the topology origin of Blender's Pose brush: flood over the
//! surface from the vertex under the pointer, stop at the brush ring, and put
//! the joint in the middle of where the flood left the ring. More joints are
//! found by growing on from there a ring of neighbours at a time. Three
//! things are done differently, because of the meshes this app sculpts
//! (millions of vertices, and uneven ones once dynamic topology has been at
//! them):
//!
//! - Blender passes over the whole mesh for every ring it grows. Here a ring
//!   is grown from the ring before it, so finding a chain costs what the
//!   chain covers and no more.
//! - Pressed half-way along a limb, Blender's flood leaves the brush ring at
//!   both ends, puts the joint between the two (under the pointer) and
//!   leaves the hand behind. Here the way out that leads to the bulk of the
//!   model is the joint, and what lies past any other way out (the hand)
//!   goes with the limb.
//! - Blender softens a bend by averaging the weights over neighbours a number
//!   of times, which is a width in edges: the same setting is a soft bend on
//!   a coarse mesh and a crease on a fine one. Here a bend is as wide as a
//!   share of the brush radius, measured along the surface.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use glam::{DVec3, Vec3};

use super::super::falloff::Falloff;
use super::super::kernels;
use crate::mesh::Mesh;

/// Most joints a chain may have.
pub const MAX_JOINTS: usize = 8;

/// While the way to the body is being found, a vertex reached by one of the
/// fronts is marked with this plus the front's number.
const FRONT: u32 = 1 << 8;

/// What to look for.
pub struct Ask {
    /// The grabbed point.
    pub centre: Vec3,
    /// A vertex of the surface that was grabbed.
    pub seed: u32,
    pub radius: f32,
    /// How much further off than the brush ring the first joint sits, in
    /// brush radii (Blender's Pose Origin Offset).
    pub offset: f32,
    pub joints: usize,
    /// Most vertices to walk beyond the brush ring, or `None` for as many as
    /// it takes. The hover preview is limited, so that resting the pointer
    /// on the middle of a very dense model never holds a frame up; a press
    /// never is.
    pub budget: Option<usize>,
    /// Whether [`Skeleton::core`] is wanted. It is a pass over everything
    /// the first joint carries, and only a twist or a stretch uses it.
    pub core: bool,
}

/// A chain as the brush found it, before anything has moved.
#[derive(Clone, Debug, PartialEq)]
pub struct Skeleton {
    /// The grabbed point.
    pub head: Vec3,
    /// The joints, from the one nearest the grabbed point back to the one
    /// that stays where it is. Never empty.
    pub joints: Vec<Vec3>,
    /// The middle of what the first joint carries. A twist or a stretch runs
    /// along the line from the first joint to here: the line to the grabbed
    /// point itself, which lies on the surface, runs askew down a limb.
    /// (The grabbed point itself where [`Ask::core`] did not ask for it.)
    pub core: Vec3,
    /// How thick the limb is at the first joint: the mean distance of the
    /// joint's ring from its middle. 0 where the brush found no ring.
    pub girth: f32,
}

/// How much of a chain's move one vertex follows: `share` of what joint
/// `joint` does, and all of what every joint after it does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hold {
    pub vert: u32,
    pub joint: u8,
    pub share: f32,
}

/// A running weighted middle. Summed in double, so that a ring and its
/// mirror image, which list their vertices in different orders, come out
/// mirror images of each other.
#[derive(Clone, Copy, Default)]
struct Middle {
    sum: DVec3,
    weight: f64,
}

impl Middle {
    fn add(&mut self, p: Vec3, weight: f32) {
        self.sum += p.as_dvec3() * weight as f64;
        self.weight += weight as f64;
    }

    fn get(&self) -> Option<Vec3> {
        (self.weight > 0.0).then(|| (self.sum / self.weight).as_vec3())
    }
}

/// Working memory for the search, kept between searches: a search touches a
/// small part of a mesh that may have tens of millions of vertices, and
/// clearing a note per vertex each time would cost more than the search.
/// Notes are stamped with the pass that wrote them instead.
#[derive(Default)]
pub struct Scratch {
    pass: u32,
    stamp: Vec<u32>,
    /// Valid where `stamp` is this pass: the joint whose part of the chain
    /// the vertex is in, or `FRONT` plus a front's number.
    part: Vec<u32>,
    /// The vertices each joint carries in full, the first joint's first.
    parts: Vec<Vec<u32>>,
    /// The ring of vertices each joint sits in the middle of: the last ring
    /// of its part.
    rings: Vec<Vec<u32>>,
    layer: Vec<u32>,
    /// For the bends: the pass that wrote `band` for each vertex, then the
    /// distance along the surface to the nearest joint ring and which ring.
    band_stamp: Vec<u32>,
    band: Vec<(f32, u8)>,
    reached: Vec<u32>,
    heap: BinaryHeap<Reverse<(u32, u32)>>,
}

impl Scratch {
    fn begin(&mut self, vertices: usize) {
        if self.stamp.len() != vertices {
            self.stamp = vec![0; vertices];
            self.part = vec![0; vertices];
            self.band_stamp = Vec::new();
            self.band = Vec::new();
            self.pass = 0;
        }
        self.pass = self.pass.wrapping_add(1);
        if self.pass == 0 {
            self.stamp.fill(0);
            self.band_stamp.fill(0);
            self.pass = 1;
        }
        self.parts.iter_mut().for_each(Vec::clear);
        self.rings.iter_mut().for_each(Vec::clear);
        self.parts.resize_with(MAX_JOINTS, Vec::new);
        self.rings.resize_with(MAX_JOINTS, Vec::new);
    }

    #[inline]
    fn seen(&self, v: u32) -> bool {
        self.stamp[v as usize] == self.pass
    }

    #[inline]
    fn mark(&mut self, v: u32, part: u32) {
        self.stamp[v as usize] = self.pass;
        self.part[v as usize] = part;
    }

    #[inline]
    fn forget(&mut self, v: u32) {
        self.stamp[v as usize] = 0;
    }

    /// The joint whose part of the chain just found `v` is in, if it is in
    /// one.
    #[inline]
    pub fn part_of(&self, v: u32) -> Option<usize> {
        self.seen(v).then(|| self.part[v as usize] as usize)
    }
}

/// How far apart the vertices are around `v`: the root mean square length of
/// its edges. Every edge counts, so a vertex and its mirror twin, whose
/// neighbours are listed in different orders, weigh the same.
fn spacing(mesh: &Mesh, v: u32) -> f32 {
    let ring = mesh.neighbours(v);
    if ring.is_empty() {
        return 0.0;
    }
    let p = mesh.positions[v as usize];
    let sum: f32 = ring.iter().map(|&o| mesh.positions[o as usize].distance_squared(p)).sum();
    (sum / ring.len() as f32).sqrt()
}

/// The middle of a line of vertices across the surface, each counted by how
/// much of the line it stands for.
fn middle_of(mesh: &Mesh, verts: &[u32]) -> Option<Vec3> {
    let mut middle = Middle::default();
    for &v in verts {
        middle.add(mesh.positions[v as usize], spacing(mesh, v));
    }
    middle.get()
}

/// The middle of a patch of surface, each vertex counted by the area around
/// it.
fn surface_middle(mesh: &Mesh, verts: &[u32]) -> Option<Vec3> {
    let mut middle = Middle::default();
    for &v in verts {
        let apart = spacing(mesh, v);
        middle.add(mesh.positions[v as usize], apart * apart);
    }
    middle.get()
}

/// Grows what has been reached by one ring of neighbours from `front`,
/// marking them as belonging to `part`. The new ring is left in `out`.
/// Returns roughly where it is, or `None` once there is no surface left to
/// grow into: the plain mean of its vertices, which is enough to tell how
/// far the rings have got and costs a fraction of [`middle_of`]. A chain on
/// a dense model grows through a great many rings to find each joint.
fn grow(mesh: &Mesh, scratch: &mut Scratch, front: &[u32], part: u32, out: &mut Vec<u32>) -> Option<Vec3> {
    let hidden = mesh.hidden_vertices();
    out.clear();
    let mut sum = DVec3::ZERO;
    for &v in front {
        for &u in mesh.neighbours(v) {
            if scratch.seen(u) || hidden.is_some_and(|h| h[u as usize]) {
                continue;
            }
            scratch.mark(u, part);
            out.push(u);
            sum += mesh.positions[u as usize].as_dvec3();
        }
    }
    (!out.is_empty()).then(|| (sum / out.len() as f64).as_vec3())
}

/// Which of the ways the flood left the brush ring leads to the body.
///
/// `ring` is every vertex just outside the brush ring that the flood touched.
/// Along one limb those make a single loop. Pressed mid-limb they make two,
/// and on a hand one for the wrist and one for each finger that pokes out of
/// the ring. A front is walked out from all of them at the same pace; fronts
/// that meet are one way out; a front that runs out of surface was a dead
/// end (the hand, a fingertip). The piece with the most surface is the body:
/// usually that is the last front still going, and the walk stops there
/// without having to cross the rest of the model.
///
/// Every dead end is marked as carried by the first joint and added to
/// `carried`. Returns the ring vertices on the body's side, and whether the
/// walk finished inside its budget.
fn find_body(
    mesh: &Mesh,
    scratch: &mut Scratch,
    ring: &[u32],
    carried: &mut Vec<u32>,
    budget: Option<usize>,
) -> (Vec<u32>, bool) {
    let hidden = mesh.hidden_vertices();
    let pos = &mesh.positions;
    let n = ring.len();
    // Fronts that have met share a root here.
    let mut root: Vec<u32> = (0..n as u32).collect();
    fn find(root: &mut [u32], mut a: u32) -> u32 {
        while root[a as usize] != a {
            root[a as usize] = root[root[a as usize] as usize];
            a = root[a as usize];
        }
        a
    }
    // Per root: vertices reached but not yet walked on from, and the surface
    // walked so far (the area round each vertex walked on from).
    let mut waiting = vec![1u32; n];
    let mut area = vec![0.0f64; n];
    let mut going = n;
    let mut largest_done = 0.0f64;
    let mut reached = std::mem::take(&mut scratch.reached);
    reached.clear();
    reached.extend_from_slice(ring);
    let mut next = 0;

    let settled = loop {
        let Some(&v) = reached.get(next) else { break true };
        let mut a = find(&mut root, scratch.part[v as usize] - FRONT);
        if going <= 1 && area[a as usize] >= largest_done {
            break true;
        }
        if budget.is_some_and(|most| next >= most.max(n)) {
            break false;
        }
        next += 1;
        let pv = pos[v as usize];
        let (mut around, mut edges) = (0.0f32, 0u32);
        for &u in mesh.neighbours(v) {
            if hidden.is_some_and(|h| h[u as usize]) {
                continue;
            }
            around += pv.distance_squared(pos[u as usize]);
            edges += 1;
            if scratch.seen(u) {
                let part = scratch.part[u as usize];
                if part >= FRONT {
                    let b = find(&mut root, part - FRONT);
                    if b != a {
                        // Two fronts have met. `a` is still going (`v` was
                        // waiting in it), so two going fronts become one.
                        if waiting[b as usize] > 0 {
                            going -= 1;
                        }
                        let (keep, drop) = (a.min(b), a.max(b));
                        root[drop as usize] = keep;
                        waiting[keep as usize] += waiting[drop as usize];
                        area[keep as usize] += area[drop as usize];
                        a = keep;
                    }
                }
                continue;
            }
            scratch.mark(u, FRONT + a);
            reached.push(u);
            waiting[a as usize] += 1;
        }
        area[a as usize] += (around / edges.max(1) as f32) as f64;
        waiting[a as usize] -= 1;
        if waiting[a as usize] == 0 {
            going -= 1;
            largest_done = largest_done.max(area[a as usize]);
        }
    };

    // The body: the front the walk stopped on, or with everything walked the
    // piece with the most surface. Out of budget, every front still going
    // may be it, which is where Blender leaves the question.
    let stopped_on = reached.get(next).map(|&v| find(&mut root, scratch.part[v as usize] - FRONT));
    let largest = (0..n as u32)
        .filter(|&a| root[a as usize] == a)
        .max_by(|&a, &b| area[a as usize].total_cmp(&area[b as usize]));
    let mut body_ring = Vec::new();
    for (i, &v) in reached.iter().enumerate() {
        let a = find(&mut root, scratch.part[v as usize] - FRONT);
        let body = if !settled { waiting[a as usize] > 0 } else { Some(a) == stopped_on.or(largest) };
        if !body {
            scratch.mark(v, 0);
            carried.push(v);
        } else if i < n {
            scratch.mark(v, 0);
            body_ring.push(v);
        } else {
            // Body surface the walk crossed: free again, for the chain to
            // grow over.
            scratch.forget(v);
        }
    }
    scratch.reached = reached;
    (body_ring, settled)
}

/// Finds the chain for a press at `ask.centre`. Leaves in `scratch` which
/// vertices each joint carries, for [`holds`] to read.
pub fn skeleton(mesh: &Mesh, scratch: &mut Scratch, ask: &Ask) -> Skeleton {
    scratch.begin(mesh.vertex_count());
    let hidden = mesh.hidden_vertices();
    let pos = &mesh.positions;
    let r2 = ask.radius * ask.radius;

    // Everything joined to the grabbed vertex without leaving the brush
    // ring, and the vertices just outside it.
    let mut inside = std::mem::take(&mut scratch.parts[0]);
    let mut ring = Vec::new();
    scratch.mark(ask.seed, 0);
    inside.push(ask.seed);
    let mut far = (0.0f32, ask.seed);
    let mut next = 0;
    while let Some(&v) = inside.get(next) {
        next += 1;
        for &u in mesh.neighbours(v) {
            if scratch.seen(u) || hidden.is_some_and(|h| h[u as usize]) {
                continue;
            }
            let d2 = pos[u as usize].distance_squared(ask.centre);
            if d2 < r2 {
                scratch.mark(u, 0);
                inside.push(u);
                if d2 > far.0 {
                    far = (d2, u);
                }
            } else {
                scratch.mark(u, FRONT + ring.len() as u32);
                ring.push(u);
            }
        }
    }

    let reach = |from: Vec3, to: Vec3| (to - from).normalize_or_zero() * (ask.radius * ask.offset);
    let core = |carried: &[u32]| ask.core.then(|| surface_middle(mesh, carried)).flatten().unwrap_or(ask.centre);
    if ring.is_empty() {
        // The whole piece is inside the brush ring. It turns about its far
        // end, as in Blender.
        let far = pos[far.1 as usize];
        let core = core(&inside);
        scratch.parts[0] = inside;
        return Skeleton { head: ask.centre, joints: vec![far + reach(ask.centre, far)], core, girth: 0.0 };
    }

    let (mut front, settled) = find_body(mesh, scratch, &ring, &mut inside, ask.budget);
    inside.extend_from_slice(&front);
    let first = middle_of(mesh, &front).unwrap_or(ask.centre);
    let girth = front.iter().map(|&v| pos[v as usize].distance(first)).sum::<f32>() / front.len().max(1) as f32;
    let mut first = first;
    let mut layer = std::mem::take(&mut scratch.layer);
    if ask.offset > 0.0 {
        // The joint is to sit further on than the brush ring. Blender pushes
        // it straight on from the grabbed point through the middle of the
        // ring, then grows what it carries after it, a ring at a time, for as
        // long as the rings are still coming closer to it. But the grabbed
        // point is on the skin, so that line runs askew: pushed a brush size
        // on, the joint of a limb grabbed by its side has left the limb by
        // the far side. Here the push only says how far to grow, and the
        // joint is the middle of the ring that came nearest.
        let pushed = first + reach(ask.centre, first);
        let mut nearest = f32::INFINITY;
        while let Some(middle) = grow(mesh, scratch, &front, 0, &mut layer) {
            let apart = middle.distance(pushed);
            if apart >= nearest {
                layer.iter().for_each(|&v| scratch.forget(v));
                break;
            }
            nearest = apart;
            inside.extend_from_slice(&layer);
            std::mem::swap(&mut front, &mut layer);
        }
        first = middle_of(mesh, &front).unwrap_or(first);
    }
    let core = core(&inside);
    scratch.parts[0] = inside;

    // The joints after the first: each where a ring's middle first lies a
    // segment's length from the joint before.
    let mut joints = vec![first];
    let mut walked = 0;
    let length = ask.radius * (1.0 + ask.offset);
    let wanted = ask.joints.clamp(1, MAX_JOINTS);
    while settled && joints.len() < wanted {
        let from = joints[joints.len() - 1];
        let part = joints.len();
        let mut members = std::mem::take(&mut scratch.parts[part]);
        let mut ring = front.clone();
        let found = loop {
            let Some(rough) = grow(mesh, scratch, &ring, part as u32, &mut layer) else { break None };
            members.extend_from_slice(&layer);
            walked += layer.len();
            std::mem::swap(&mut ring, &mut layer);
            if rough.distance(from) >= length {
                break Some(middle_of(mesh, &ring).unwrap_or(rough));
            }
            if ask.budget.is_some_and(|most| walked > most) {
                break None;
            }
        };
        let Some(joint) = found else {
            // The surface (or the budget) ran out before another joint: what
            // was grown over stays where it is, with the rest of the model.
            members.iter().for_each(|&v| scratch.forget(v));
            members.clear();
            scratch.parts[part] = members;
            break;
        };
        scratch.rings[part - 1] = std::mem::replace(&mut front, ring);
        scratch.parts[part] = members;
        joints.push(joint);
    }
    scratch.rings[joints.len() - 1] = front;
    scratch.layer = layer;
    Skeleton { head: ask.centre, joints, core, girth }
}

/// How much of the chain's move every vertex follows, for the chain
/// [`skeleton`] has just found (`scratch` still describes it).
///
/// A vertex in a joint's part follows that joint in full, except near a
/// joint ring: within `half_band` of one, measured along the surface, the
/// share falls from all to none across the ring, shaped by the brush's
/// falloff (its centre is the limb's side of the bend, its edge the far
/// side). Where two joints are closer together than their bends are wide,
/// each bend is narrowed to half the way to the other.
pub fn holds(mesh: &Mesh, scratch: &mut Scratch, joints: usize, half_band: f32, falloff: &Falloff, out: &mut Vec<Hold>) {
    out.clear();
    // The brush's weight at its centre is the share a vertex well inside the
    // limb gets: nothing to move if a hand-drawn curve starts at nothing.
    let full = falloff.at(0.0, kernels::smooth);
    if full <= 0.0 {
        return;
    }
    let in_full = |out: &mut Vec<Hold>, scratch: &Scratch| {
        for (joint, part) in scratch.parts.iter().enumerate().take(joints) {
            out.extend(part.iter().map(|&vert| Hold { vert, joint: joint as u8, share: 1.0 }));
        }
    };
    // No bend to soften: a hinge, or a piece with no joint ring at all.
    let bends = half_band > 0.0 && scratch.rings.iter().take(joints).any(|ring| !ring.is_empty());
    if !bends {
        return in_full(out, scratch);
    }

    // The distance of everything near a joint ring from the nearest one.
    let vertices = mesh.vertex_count();
    if scratch.band.len() != vertices {
        scratch.band_stamp = vec![0; vertices];
        scratch.band = vec![(0.0, 0); vertices];
    }
    let pass = scratch.pass;
    let hidden = mesh.hidden_vertices();
    let pos = &mesh.positions;
    let mut reached = std::mem::take(&mut scratch.reached);
    reached.clear();
    scratch.heap.clear();
    for (joint, ring) in scratch.rings.iter().enumerate().take(joints) {
        for &v in ring {
            scratch.band_stamp[v as usize] = pass;
            scratch.band[v as usize] = (0.0, joint as u8);
            scratch.heap.push(Reverse((0, v)));
            reached.push(v);
        }
    }
    // Distances are never negative, and such floats order as their bits do.
    while let Some(Reverse((bits, v))) = scratch.heap.pop() {
        let (at, joint) = scratch.band[v as usize];
        if f32::from_bits(bits) > at {
            continue;
        }
        for &u in mesh.neighbours(v) {
            if hidden.is_some_and(|h| h[u as usize]) {
                continue;
            }
            let through = at + pos[u as usize].distance(pos[v as usize]);
            if through > half_band {
                continue;
            }
            let fresh = scratch.band_stamp[u as usize] != pass;
            if fresh || through < scratch.band[u as usize].0 {
                if fresh {
                    scratch.band_stamp[u as usize] = pass;
                    reached.push(u);
                }
                scratch.band[u as usize] = (through, joint);
                scratch.heap.push(Reverse((through.to_bits(), u)));
            }
        }
    }

    // Half the way to the nearest other ring, where that is nearer than the
    // bend is wide.
    let mut half = [half_band; MAX_JOINTS];
    for &v in &reached {
        let (at, joint) = scratch.band[v as usize];
        for &u in mesh.neighbours(v) {
            if scratch.band_stamp[u as usize] != pass {
                continue;
            }
            let (other_at, other) = scratch.band[u as usize];
            if other != joint {
                let apart = at + pos[u as usize].distance(pos[v as usize]) + other_at;
                half[joint as usize] = half[joint as usize].min(0.5 * apart);
            }
        }
    }

    let share = |along: f32, half: f32| {
        // 0 well on the limb's side of the ring, 1 well on the far side.
        let u = (0.5 - 0.5 * along / half).clamp(0.0, 1.0);
        (falloff.at(u, kernels::smooth) / full).clamp(0.0, 1.0)
    };
    let near = |scratch: &Scratch, v: u32| {
        let (at, joint) = scratch.band[v as usize];
        (scratch.band_stamp[v as usize] == pass && at < half[joint as usize]).then_some((at, joint as usize))
    };
    for (part, verts) in scratch.parts.iter().enumerate().take(joints) {
        for &vert in verts {
            let hold = match near(scratch, vert) {
                // On the limb's side of its own joint's ring.
                Some((at, joint)) if joint == part => Hold { vert, joint: joint as u8, share: share(at, half[joint]) },
                // On the far side of the ring of the joint before.
                Some((at, joint)) if joint + 1 == part => {
                    Hold { vert, joint: joint as u8, share: share(-at, half[joint]) }
                }
                _ => Hold { vert, joint: part as u8, share: 1.0 },
            };
            out.push(hold);
        }
    }
    // Past the last joint nothing is carried, but the near side of the bend
    // there still gives a little.
    let last = joints - 1;
    for &vert in &reached {
        if scratch.part_of(vert).is_some_and(|part| part < joints) {
            continue;
        }
        if let Some((at, joint)) = near(scratch, vert).filter(|&(_, joint)| joint == last) {
            let share = share(-at, half[joint]);
            if share > 0.0 {
                out.push(Hold { vert, joint: joint as u8, share });
            }
        }
    }
    scratch.reached = reached;
}
