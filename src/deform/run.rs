//! A deformer at work on a mesh: what one deformation keeps of each mesh it
//! changes, and the pass that puts the mesh where the deformer has it.
//!
//! **One stroke.** A deformation runs as one stroke of the mesh, opened by
//! its first pass and closed when it is applied or cancelled,
//! as the gizmo's drags of a masked object are (app/masked.rs). So every pass
//! writes through the stroke calls, the stroke's snapshot keeps where each
//! vertex was when the deformation began, and every pass works each vertex
//! out again from there (`StrokeVertex::start`): a slider dragged back and
//! forth a hundred times lands where it would have landed the first time,
//! and nothing drifts. Applying is the stroke's own undo record, so sculpt
//! layers and subdivision levels are told of it as they are of any stroke.
//! Cancelling puts every vertex back where the snapshot has it, which is the
//! same bits it started from.
//!
//! The stroke is not opened when the deformation is set up (`Target::begin`,
//! which only measures the mesh), so a mesh is in the middle of a stroke only
//! while something has been done to it: a Deform card left open with nothing
//! moved holds no mesh, and anything else that wants the mesh can have it.
//!
//! **What moves.** A vertex is moved by `1 - mask` of the way the deformer
//! takes it, as the gizmo moves the unmasked part of an object: a fully
//! masked vertex never moves and a half-masked one goes half way. Hidden
//! vertices are passed over by the stroke calls themselves.
//!
//! **Cost.** A pass goes over the leaves of the mesh's spatial index on every
//! thread (`Mesh::move_vertices`). Leaves holding nothing but fully masked
//! vertices are dropped when the deformation begins. For a deformer that
//! only acts inside its range (Inflate, Lattice) the leaves wholly outside the
//! range are passed over too, unless an earlier pass moved them, which this
//! one must put back. Only the leaves a pass went over are refitted and sent
//! to the graphics card again.

use glam::{DAffine3, DMat3, DQuat, DVec3, Mat4, Vec3};
use rayon::prelude::*;

use super::{Bounds, Deformer};
use crate::mesh::layers::Ride;
use crate::mesh::{Mesh, StrokeDelta};

/// The gizmo's frame in the world: the deformers' coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub origin: DVec3,
    pub rotation: DQuat,
}

impl Frame {
    /// The map from a mesh's own space, placed in the world by `model`, into this frame.
    pub fn from_mesh(&self, model: Mat4) -> DAffine3 {
        let world = DAffine3::from_mat4(model.as_dmat4());
        DAffine3::from_quat(self.rotation.inverse()) * DAffine3::from_translation(-self.origin) * world
    }

    /// A point of this frame in the world.
    pub fn to_world(&self, q: DVec3) -> DVec3 {
        self.origin + self.rotation * q
    }

    /// A point of the world in this frame.
    pub fn from_world(&self, p: DVec3) -> DVec3 {
        self.rotation.inverse() * (p - self.origin)
    }
}

/// One mesh a deformation is changing.
pub struct Target {
    /// `Mesh::uid` of the mesh the stroke was opened on.
    pub uid: u64,
    /// The mesh's own space into the frame, and back.
    into: DAffine3,
    back: DAffine3,
    /// Normals from the mesh's own space into the frame (the inverse
    /// transpose, so a stretched object's normals stay square to its surface).
    normals_into: DMat3,
    /// The leaves with a vertex that is not fully masked, ascending, and how
    /// far each reaches along each of the frame's axes.
    leaves: Vec<u32>,
    extents: Vec<[(f64, f64); 3]>,
    /// The leaves the last pass went over: where a vertex may be off its start.
    visited: Vec<u32>,
    /// Each vertex's normal when the deformation began, for Inflate.
    normals: Option<Vec<Vec3>>,
    /// Whether the stroke has been opened: whether a pass has moved anything.
    started: bool,
    /// The box the mesh fills, in the frame.
    pub bounds: Bounds,
}

impl Target {
    /// Sets a deformation up on `mesh`, placed in the world by `model`: it is
    /// measured in `frame`. The stroke opens with the first pass that moves it.
    pub fn begin(mesh: &Mesh, model: Mat4, frame: &Frame) -> Target {
        let into = frame.from_mesh(model);
        let leaves = &mesh.bvh.leaves;
        let (positions, mask) = (&mesh.positions, &mesh.mask);
        let measured: Vec<(u32, [(f64, f64); 3], bool)> = (0..leaves.len() as u32)
            .into_par_iter()
            .with_min_len(64)
            .filter_map(|l| {
                let leaf = &leaves[l as usize];
                let range = leaf.vert_start as usize..leaf.vert_end as usize;
                if range.is_empty() {
                    return None;
                }
                let mut reach = [(f64::MAX, f64::MIN); 3];
                let mut free = false;
                for v in range {
                    let q = into.transform_point3(positions[v].as_dvec3());
                    for (c, r) in reach.iter_mut().enumerate() {
                        *r = (r.0.min(q[c]), r.1.max(q[c]));
                    }
                    free |= mask[v] < 1.0;
                }
                Some((l, reach, free))
            })
            .collect();
        let (min, max) = measured.iter().fold((DVec3::MAX, DVec3::MIN), |(min, max), (_, reach, _)| {
            let lo = DVec3::new(reach[0].0, reach[1].0, reach[2].0);
            let hi = DVec3::new(reach[0].1, reach[1].1, reach[2].1);
            (min.min(lo), max.max(hi))
        });
        let free: Vec<&(u32, [(f64, f64); 3], bool)> = measured.iter().filter(|m| m.2).collect();
        Target {
            uid: mesh.uid,
            into,
            back: into.inverse(),
            normals_into: into.matrix3.inverse().transpose(),
            leaves: free.iter().map(|m| m.0).collect(),
            extents: free.iter().map(|m| m.1).collect(),
            visited: Vec::new(),
            normals: None,
            started: false,
            bounds: if min.cmple(max).all() { Bounds::new(min, max) } else { Bounds::new(DVec3::ZERO, DVec3::ZERO) },
        }
    }

    /// Whether a pass has moved the mesh, which is then in the middle of a
    /// stroke until the deformation is applied or cancelled.
    pub fn holds(&self) -> bool {
        self.started
    }

    /// Whether every vertex of the mesh is fully masked: nothing to deform.
    pub fn is_pinned(&self) -> bool {
        self.leaves.is_empty()
    }

    /// Puts the mesh where `deformer` has it, every vertex worked out from
    /// where it was when the deformation began. `None` puts every vertex
    /// back there. `axis` is the deformer's, for passing over the leaves its
    /// range cannot reach.
    pub fn apply(&mut self, mesh: &mut Mesh, deformer: Option<&Deformer>, axis: usize) {
        if mesh.uid != self.uid {
            return;
        }
        let reach = deformer.and_then(Deformer::reach);
        let mut visit: Vec<u32> = match deformer {
            None => Vec::new(),
            Some(_) => self
                .leaves
                .iter()
                .zip(&self.extents)
                .filter(|(_, extent)| reach.is_none_or(|(from, to)| extent[axis].1 >= from && extent[axis].0 <= to))
                .map(|(&l, _)| l)
                .collect(),
        };
        // What the last pass moved and this one would not reach goes back.
        if visit.len() != self.leaves.len() {
            visit.extend_from_slice(&self.visited);
            visit.sort_unstable();
            visit.dedup();
        }
        if visit.is_empty() || (!self.started && deformer.is_none()) {
            return;
        }
        if !self.started {
            mesh.begin_stroke();
            self.started = true;
        }
        if deformer.is_some_and(Deformer::needs_normals) && self.normals.is_none() {
            mesh.flush_normals();
            self.normals = Some(mesh.normals.clone());
        }
        let (into, back, normals_into) = (self.into, self.back, self.normals_into);
        let normals = self.normals.as_deref();
        let runs = if normals.is_some() { pass_runs(&visit, &mesh.bvh.leaves) } else { Vec::new() };
        mesh.move_vertices(&visit, |at, vertex| {
            if vertex.mask >= 1.0 {
                return None;
            }
            let Some(deformer) = deformer else { return Some(vertex.start) };
            let q = into.transform_point3(vertex.start.as_dvec3());
            let normal = match normals {
                // The pass gives a place, not a vertex: Inflate alone needs the number.
                Some(normals) => (normals_into * normals[vertex_at(&runs, at)].as_dvec3()).normalize_or_zero(),
                None => DVec3::ZERO,
            };
            let Some(to) = deformer.apply(q, normal) else { return Some(vertex.start) };
            let moved = back.transform_point3(to).as_vec3();
            // A half-masked vertex goes part of the way, written as a step
            // from the start so that a free vertex lands exactly on `moved`.
            let weight = 1.0 - vertex.mask;
            Some(if weight >= 1.0 { moved } else { vertex.start + (moved - vertex.start) * weight })
        });
        let changed = with_neighbours(mesh, &visit);
        mesh.positions_changed(&changed);
        self.visited = if deformer.is_some() { visit } else { Vec::new() };
    }

    /// Where `deformer` takes the point `p` of the mesh's own space, whose
    /// surface faces `normal` there, with no mask: the deformation itself,
    /// for checking it against the mesh's mirror planes.
    pub fn map(&self, deformer: &Deformer, p: DVec3, normal: DVec3) -> DVec3 {
        let normal = (self.normals_into * normal).normalize_or_zero();
        deformer.apply(self.into.transform_point3(p), normal).map_or(p, |q| self.back.transform_point3(q))
    }

    /// Closes the stroke after an Apply and gives back its undo record:
    /// `None` if nothing ended up anywhere new. `ride` says how sculpt layers'
    /// offsets are carried by a deformation of the model underneath them.
    pub fn end(self, mesh: &mut Mesh, ride: Ride) -> Option<StrokeDelta> {
        if mesh.uid != self.uid || !self.started {
            return None;
        }
        mesh.layer_ride = ride;
        match mesh.end_stroke(false) {
            Some(StrokeDelta::Positions { before, after, .. }) if before == after => None,
            delta => delta,
        }
    }

    /// Puts every vertex back where it was when the deformation began and
    /// closes the stroke, leaving nothing to undo.
    pub fn cancel(mut self, mesh: &mut Mesh) {
        if mesh.uid != self.uid || !self.started {
            return;
        }
        self.apply(mesh, None, 0);
        // Nothing moved, so the layers have nothing to be told.
        mesh.layer_ride = Ride::Still;
        let _ = mesh.end_stroke(false);
    }
}

/// Where each leaf's vertices start in a pass over `visit` (leaves
/// ascending, each counted once, as `Mesh::move_vertices` numbers them), and
/// the leaf's first vertex: what turns a place in the pass back into a vertex.
fn pass_runs(visit: &[u32], leaves: &[crate::mesh::bvh::Leaf]) -> Vec<(usize, u32)> {
    let mut at = 0usize;
    visit
        .iter()
        .map(|&l| {
            let leaf = &leaves[l as usize];
            let run = (at, leaf.vert_start);
            at += (leaf.vert_end - leaf.vert_start) as usize;
            run
        })
        .collect()
}

/// The vertex at place `at` of a pass laid out by `runs`.
#[inline]
fn vertex_at(runs: &[(usize, u32)], at: usize) -> usize {
    let k = runs.partition_point(|run| run.0 <= at) - 1;
    runs[k].1 as usize + (at - runs[k].0)
}

/// `leaves` and every leaf that shares a vertex with one of them: what a
/// change to the vertices of `leaves` reaches (`Mesh::positions_changed`).
fn with_neighbours(mesh: &Mesh, leaves: &[u32]) -> Vec<u32> {
    let mut set = vec![false; mesh.bvh.leaves.len()];
    for &l in leaves {
        set[l as usize] = true;
        for &o in mesh.bvh.leaf_neighbours(l) {
            set[o as usize] = true;
        }
    }
    (0..set.len() as u32).filter(|&l| set[l as usize]).collect()
}
