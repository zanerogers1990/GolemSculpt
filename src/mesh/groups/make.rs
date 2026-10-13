//! The commands that make and change face groups. Each works out a whole new
//! group list from the mesh as it is and hands it back; putting it on the
//! mesh and recording the undo step is the caller's (`Mesh::set_group_ids`).
//!
//! **Only what shows changes.** A hidden face keeps the group it has through
//! every one of these, as it keeps its shape under every brush: hiding part
//! of a model is how a sculptor says "not this".
//!
//! **Numbers and colours.** A new group takes the lowest free number whose
//! colour is not close to the colours of the groups it touches ([`Picker`]).
//! Lowest, so that the numbers stay small and a model made the same way gets
//! the same colours; not close, so that two groups side by side can be told
//! apart. Only a handful of free numbers are tried: consecutive numbers are
//! spread evenly round the colour wheel, so one of the first few is always
//! clear of four or five neighbours, and a group hemmed in by more than that
//! takes the best of them.

use std::sync::atomic::{AtomicBool, Ordering};

use glam::Vec3;
use rayon::prelude::*;

use super::{MOST_GROUPS, PER_TASK, faces_of_triangles, hue_gap};
use crate::mesh::Mesh;

/// How far apart on the colour wheel the colours of two groups that touch
/// should be: a tenth of the way round, the step from orange to yellow.
const APART: f32 = 0.1;

/// How many free numbers are looked at for a new group.
const CANDIDATES: usize = 24;

/// What a command made: the mesh's new group list, and what to say of it.
#[derive(Debug)]
pub struct Made {
    pub ids: Vec<u16>,
    /// The group that was made or grew; with [`Made::groups`] more than one,
    /// the first of them.
    pub group: u16,
    /// How many groups the command made.
    pub groups: usize,
    /// How many triangles changed group.
    pub triangles: usize,
}

/// Hands out group numbers that are free, keeping clear of given colours.
pub(super) struct Picker {
    used: Vec<bool>,
    /// The lowest number that is free.
    next: usize,
}

impl Picker {
    /// With every number in `taken` spoken for.
    pub(super) fn new(taken: impl Iterator<Item = u16>) -> Picker {
        let mut used = vec![false; MOST_GROUPS + 1];
        used[0] = true;
        for id in taken {
            used[id as usize] = true;
        }
        let next = used.iter().position(|&used| !used).unwrap_or(used.len());
        Picker { used, next }
    }

    /// A free number for a group that touches the groups `near`, or `None`
    /// if every number is taken.
    pub(super) fn pick(&mut self, near: &[u16]) -> Option<u16> {
        let mut best: Option<(usize, f32)> = None;
        let free = (self.next..self.used.len()).filter(|&id| !self.used[id]).take(CANDIDATES);
        for id in free {
            let gap = near.iter().map(|&other| hue_gap(id as u16, other)).fold(1.0, f32::min);
            if best.is_none_or(|(_, widest)| gap > widest) {
                best = Some((id, gap));
            }
            if gap >= APART {
                break;
            }
        }
        let (id, _) = best?;
        self.used[id] = true;
        while self.next < self.used.len() && self.used[self.next] {
            self.next += 1;
        }
        Some(id as u16)
    }
}

/// The mesh's group list, or a list of zeros if it has none yet.
fn ids_of(mesh: &Mesh) -> Vec<u16> {
    if mesh.has_groups() { mesh.group_ids().to_vec() } else { vec![0; mesh.triangle_count()] }
}

/// Whether triangle `t` shows.
fn shows(mesh: &Mesh, t: usize) -> bool {
    !mesh.is_triangle_hidden(t as u32)
}

/// The groups of the triangles outside `inside` that share a vertex with a
/// triangle in it: the groups a group made of `inside` would touch.
fn touching(mesh: &Mesh, ids: &[u16], inside: &[bool]) -> Vec<u16> {
    let seen: Vec<AtomicBool> = (0..=MOST_GROUPS).map(|_| AtomicBool::new(false)).collect();
    // Threads only ever store `true`, so their order cannot matter.
    mesh.indices.par_chunks_exact(3).enumerate().with_min_len(PER_TASK).filter(|(t, _)| inside[*t]).for_each(
        |(_, t)| {
            for &v in t {
                for &other in mesh.vt.row(v as usize) {
                    if !inside[other as usize] {
                        seen[ids[other as usize] as usize].store(true, Ordering::Relaxed);
                    }
                }
            }
        },
    );
    (1..=MOST_GROUPS).filter(|&id| seen[id].load(Ordering::Relaxed)).map(|id| id as u16).collect()
}

/// Makes one new group of the showing triangles `chosen` says yes to.
/// `nothing` is what to say when there are none.
fn one_group(mesh: &Mesh, chosen: impl Fn(usize, &[u32]) -> bool + Sync, nothing: &str) -> Result<Made, String> {
    let mut inside: Vec<bool> = mesh
        .indices
        .par_chunks_exact(3)
        .enumerate()
        .with_min_len(PER_TASK)
        .map(|(t, corners)| shows(mesh, t) && chosen(t, corners))
        .collect();
    // A quad goes in whole or not at all, as its first triangle says.
    if let Some(face_of) = faces_of_triangles(mesh) {
        let mut first: Vec<Option<bool>> = vec![None; mesh.polys.as_ref().map_or(0, |polys| polys.start.len())];
        for (flag, &face) in inside.iter_mut().zip(&face_of) {
            if let Some(of_face) = first.get_mut(face as usize) {
                *flag = *of_face.get_or_insert(*flag);
            }
        }
    }
    let triangles = inside.iter().filter(|&&flag| flag).count();
    if triangles == 0 {
        return Err(nothing.into());
    }
    let mut ids = ids_of(mesh);
    // The numbers still in use once these triangles have left their groups.
    let kept = ids.iter().zip(&inside).filter(|(_, inside)| !**inside).map(|(&id, _)| id);
    let group = Picker::new(kept).pick(&touching(mesh, &ids, &inside)).ok_or_else(full)?;
    ids.par_iter_mut().zip(&inside).with_min_len(PER_TASK).for_each(|(id, &inside)| {
        if inside {
            *id = group;
        }
    });
    Ok(Made { ids, group, groups: 1, triangles })
}

fn full() -> String {
    format!(
        "This object already has {MOST_GROUPS} groups, which is the most there can be. Merge some, or Clear Groups."
    )
}

/// Group Masked: the masked surface that shows becomes a new group. A
/// triangle counts as masked when its corners are masked by half or more on
/// average, so the group's edge runs where the mask's soft edge is halfway.
pub fn masked(mesh: &Mesh) -> Result<Made, String> {
    let mask = &mesh.mask;
    one_group(
        mesh,
        |_, t| mask[t[0] as usize] + mask[t[1] as usize] + mask[t[2] as usize] >= 1.5,
        "Nothing that shows is masked. Mask the surface the group should cover, then press Group Masked.",
    )
}

/// Group Visible: everything that shows becomes a new group. With nothing
/// hidden that is the whole object as one group.
pub fn visible(mesh: &Mesh) -> Result<Made, String> {
    one_group(mesh, |_, _| true, "Nothing of this object shows. Show All first.")
}

/// Disjoint sets over `0..count`, with the paths halved as they are walked.
struct Sets {
    parent: Vec<u32>,
}

impl Sets {
    fn new(count: usize) -> Sets {
        Sets { parent: (0..count as u32).collect() }
    }

    fn find(&mut self, mut x: u32) -> u32 {
        while self.parent[x as usize] != x {
            let up = self.parent[self.parent[x as usize] as usize];
            self.parent[x as usize] = up;
            x = up;
        }
        x
    }

    /// Joins the sets of `a` and `b`. The lower numbered root stays the
    /// root, so a set is always named by its first member.
    fn join(&mut self, a: u32, b: u32) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.parent[a.max(b) as usize] = a.min(b);
        }
    }
}

/// Auto Groups: each connected part of what shows becomes a group of its
/// own. Parts are connected through shared vertices, so two shells that
/// merely touch or pass through each other are two groups.
pub fn parts(mesh: &Mesh) -> Result<Made, String> {
    let triangles = mesh.triangle_count();
    let mut sets = Sets::new(mesh.vertex_count());
    for (t, corners) in mesh.indices.chunks_exact(3).enumerate() {
        if shows(mesh, t) {
            sets.join(corners[0], corners[1]);
            sets.join(corners[0], corners[2]);
        }
    }
    let part: Vec<u32> = (0..triangles).map(|t| sets.find(mesh.indices[t * 3])).collect();
    regroup(mesh, &part, &[], "Nothing of this object shows. Show All first.")
}

/// Unit normals of the triangles; zero for one with no area.
pub(super) fn face_normals(mesh: &Mesh) -> Vec<Vec3> {
    let p = &mesh.positions;
    mesh.indices
        .par_chunks_exact(3)
        .with_min_len(PER_TASK)
        .map(|t| (p[t[1] as usize] - p[t[0] as usize]).cross(p[t[2] as usize] - p[t[0] as usize]).normalize_or_zero())
        .collect()
}

/// For each triangle, the triangle across each of its three edges (the edge
/// leaving corner 0, 1 and 2 in turn); `u32::MAX` across an open edge.
pub(super) fn edge_neighbours(mesh: &Mesh) -> Vec<[u32; 3]> {
    let indices = &mesh.indices;
    let across = |t: u32, a: u32, b: u32| {
        let on_a = mesh.vt.row(a as usize).iter();
        on_a.copied().find(|&o| o != t && indices[o as usize * 3..o as usize * 3 + 3].contains(&b)).unwrap_or(u32::MAX)
    };
    indices
        .par_chunks_exact(3)
        .enumerate()
        .with_min_len(PER_TASK)
        .map(|(t, c)| [across(t as u32, c[0], c[1]), across(t as u32, c[1], c[2]), across(t as u32, c[2], c[0])])
        .collect()
}

/// Groups by Angle: what shows is split into groups along its sharp edges,
/// those where the surface turns by more than `degrees`. Good for a hard
/// model (each flat side its own group); on a soft sculpt it follows the
/// creases, and a low angle makes a great many small groups.
pub fn by_angle(mesh: &Mesh, degrees: f32) -> Result<Made, String> {
    let normals = face_normals(mesh);
    let neighbours = edge_neighbours(mesh);
    let least = degrees.clamp(0.0, 180.0).to_radians().cos();
    let mut sets = Sets::new(mesh.triangle_count());
    for (t, across) in neighbours.iter().enumerate() {
        for &other in across.iter().filter(|&&other| other != u32::MAX && (other as usize) > t) {
            let smooth = normals[t].dot(normals[other as usize]) >= least;
            if smooth && shows(mesh, t) && shows(mesh, other as usize) {
                sets.join(t as u32, other);
            }
        }
    }
    // A quad is one face however its diagonal folds.
    if let Some(face_of) = faces_of_triangles(mesh) {
        let mut first = vec![u32::MAX; mesh.polys.as_ref().map_or(0, |polys| polys.start.len())];
        for (t, &face) in face_of.iter().enumerate() {
            if let Some(of_face) = first.get_mut(face as usize) {
                if *of_face == u32::MAX {
                    *of_face = t as u32;
                } else {
                    sets.join(*of_face, t as u32);
                }
            }
        }
    }
    let part: Vec<u32> = (0..mesh.triangle_count() as u32).map(|t| sets.find(t)).collect();
    regroup(mesh, &part, &neighbours, "Nothing of this object shows. Show All first.")
}

/// Every showing triangle put in a new group by the part it is in (`part`
/// names each triangle's part by any number, the same for the same part).
/// The parts are numbered in the order of their first triangles, each kept
/// clear of the colours of the parts it shares an edge with (`neighbours`,
/// or empty when no two parts touch).
fn regroup(mesh: &Mesh, part: &[u32], neighbours: &[[u32; 3]], nothing: &str) -> Result<Made, String> {
    let mut ids = ids_of(mesh);
    // Parts in the order they are first met, by the name `part` gives them.
    let mut number = std::collections::HashMap::new();
    let mut of: Vec<u32> = vec![u32::MAX; part.len()];
    for t in (0..part.len()).filter(|&t| shows(mesh, t)) {
        let next = number.len() as u32;
        of[t] = *number.entry(part[t]).or_insert(next);
    }
    let count = number.len();
    if count == 0 {
        return Err(nothing.into());
    }
    let hidden_ids = ids.iter().enumerate().filter(|(t, _)| !shows(mesh, *t)).map(|(_, &id)| id);
    let mut picker = Picker::new(hidden_ids);
    let free = picker.used.iter().filter(|&&used| !used).count();
    if count > free {
        return Err(format!(
            "That would make {count} groups, and an object can have {MOST_GROUPS}. {}",
            if neighbours.is_empty() { "Join or remesh the loose pieces first." } else { "Try a larger angle." }
        ));
    }
    // Which parts share an edge, each pair once from either side.
    let of_part = &of;
    let mut pairs: Vec<(u32, u32)> = neighbours
        .par_iter()
        .enumerate()
        .with_min_len(PER_TASK)
        .flat_map_iter(|(t, across)| {
            across.iter().filter(|&&o| o != u32::MAX).map(move |&o| (of_part[t], of_part[o as usize]))
        })
        .filter(|&(a, b)| a != b && a != u32::MAX && b != u32::MAX)
        .collect();
    pairs.par_sort_unstable();
    pairs.dedup();
    let mut given = vec![0u16; count];
    let (mut at, mut near) = (0, Vec::new());
    for part in 0..count as u32 {
        near.clear();
        while at < pairs.len() && pairs[at].0 == part {
            // A part numbered later has no colour yet to keep clear of.
            near.extend(Some(given[pairs[at].1 as usize]).filter(|&id| id != 0));
            at += 1;
        }
        given[part as usize] = picker.pick(&near).ok_or_else(full)?;
    }
    let triangles = of.iter().filter(|&&part| part != u32::MAX).count();
    ids.par_iter_mut().zip(&of).with_min_len(PER_TASK).for_each(|(id, &part)| {
        if part != u32::MAX {
            *id = given[part as usize];
        }
    });
    Ok(Made { ids, group: given[0], groups: count, triangles })
}

/// Merge: every face of group `from` joins group `into`, hidden or not (a
/// group is merged whole, or the part of it that was hidden would be left
/// behind under the old number).
pub fn merged(mesh: &Mesh, from: u16, into: u16) -> Result<Made, String> {
    if from == 0 || into == 0 {
        return Err("That surface is in no group, so there is nothing to merge.".into());
    }
    if from == into {
        return Err("That is the same group.".into());
    }
    let mut ids = ids_of(mesh);
    let triangles = ids.par_iter().with_min_len(PER_TASK).filter(|&&id| id == from).count();
    if triangles == 0 {
        return Err("That group has no faces left.".into());
    }
    ids.par_iter_mut().with_min_len(PER_TASK).for_each(|id| {
        if *id == from {
            *id = into;
        }
    });
    Ok(Made { ids, group: into, groups: 1, triangles })
}
