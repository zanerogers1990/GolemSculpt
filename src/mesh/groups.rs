//! Face groups: regions of a mesh's surface that can be hidden, masked and
//! sculpted one at a time (ZBrush's polygroups, Nomad's face groups).
//!
//! **One number per triangle**, in the order of `Mesh::indices`: the group the
//! triangle is in, or 0 for none. A group is nothing more than the triangles
//! that carry its number. It is kept per triangle, not per vertex, because
//! that is what a region of a surface is: the vertices along the border of two
//! groups belong to both, exactly as the vertices along the edge of a hidden
//! patch belong to both sides (`visibility.rs`).
//!
//! The list is empty while the mesh has no groups, which is the usual case
//! and costs nothing. With groups it is two bytes a triangle: the numbers are
//! `u16`, so a mesh can have 65,535 groups, far more than can be told apart
//! on screen (the colours are the practical limit, at a few dozen).
//!
//! **A group's colour is worked out from its number** ([`colour`]) and never
//! stored, so a group keeps its colour for as long as it keeps its number:
//! through undo, a save and a reopen, a remesh. A new group is given the
//! lowest free number whose colour is not close to that of the groups it
//! touches (`make::Picker`), which is how neighbours come out distinct without
//! any group ever changing colour to make room.
//!
//! **Quads.** The two triangles of a quad a quad remesh left are always in the
//! same group, or a quad would be drawn in two colours and half of it could
//! be hidden. [`Mesh::set_group_ids`] is the one way a list goes onto a mesh,
//! and it settles that.
//!
//! **Every change is a whole new list**, and the undo step keeps the list it
//! replaced ([`GroupsStep`]). Two bytes a triangle a step is small beside a
//! stroke's record on a mesh of any size, and a swap of lists is exact in
//! both directions with nothing to work out again.
//!
//! **Following the mesh.** The numbers mean triangles, so anything that
//! changes which triangles there are has to carry them:
//!
//! - dynamic topology: a triangle split from another is in its group, and an
//!   edge between two groups is never flipped (`dyntopo.rs`);
//! - subdivision levels: up through the parent triangles, down by majority,
//!   each level's own list remembered so a round trip is exact (`levels.rs`);
//! - a rebuild that renumbers the same triangles (a file opened, a copy
//!   made): by the numbering ([`renumbered`]);
//! - a rebuild that makes new triangles (remesh, decimate, a boolean, a cut):
//!   each new triangle takes the group of the old surface nearest its centre
//!   (`transfer.rs`).
//!
//! **Stay in Group**, the per-brush tick that keeps a stroke inside the group
//! it began on, is not here: the mesh refuses the writes, the way it refuses
//! them for hidden faces (`Mesh::stay_in_groups` in `visibility.rs`).

use rayon::prelude::*;

use super::{Mesh, StrokeDelta};

mod make;
mod transfer;

#[cfg(test)]
mod tests;

pub use make::{Made, by_angle, masked, merged, parts, visible};
pub use transfer::{Carried, Fit, Nearest, Source, carry, copies, kept_apart};
pub(crate) use transfer::nearest_points;
#[cfg(test)]
pub(crate) use transfer::tidy_specks;

/// The most groups a mesh can have: every number a `u16` can hold but 0.
pub const MOST_GROUPS: usize = u16::MAX as usize;

/// Triangles handed to one rayon task.
const PER_TASK: usize = 1 << 14;

/// A mesh's face groups: see the module notes.
#[derive(Default)]
pub(super) struct Groups {
    /// One number per triangle, or empty while the mesh has no groups.
    pub(super) ids: Vec<u16>,
    /// How many different groups there are.
    count: usize,
    /// Goes up with every change, so the renderer can tell that the colours
    /// it was sent are no longer the ones to show.
    version: u64,
}

impl Groups {
    pub(super) fn bytes(&self) -> usize {
        self.ids.len() * 2
    }

    /// Dynamic topology appended a triangle split from triangle `from`: it is
    /// in the same group.
    #[inline]
    pub(super) fn born(&mut self, from: u32) {
        if let Some(&id) = self.ids.get(from as usize) {
            self.ids.push(id);
        }
    }

    /// Whether triangles `a` and `b` are in different groups: the edge
    /// between them is a border, which dynamic topology leaves where it is.
    #[inline]
    pub(super) fn border(&self, a: u32, b: u32) -> bool {
        !self.ids.is_empty() && self.ids[a as usize] != self.ids[b as usize]
    }

    /// Dynamic topology put the triangles from `first` on in another order:
    /// the one at `first + k` went to `first + to[k]`.
    pub(super) fn reorder(&mut self, first: usize, to: &[u32]) {
        if self.ids.is_empty() {
            return;
        }
        let old = self.ids[first..].to_vec();
        for (id, &at) in old.into_iter().zip(to) {
            self.ids[first + at as usize] = id;
        }
    }

    /// The numbers of the triangles from `first` on, for the undo record of
    /// the stroke that added them. Empty for a mesh without groups.
    pub(super) fn from(&self, first: usize) -> Vec<u16> {
        self.ids.get(first..).map_or(Vec::new(), <[u16]>::to_vec)
    }

    /// Undo or redo of a stroke that added triangles: back to `first`
    /// triangles, then `more` after them (none on undo), `triangles` in all.
    pub(super) fn resize(&mut self, first: usize, more: &[u16], triangles: usize) {
        if self.ids.is_empty() {
            return;
        }
        self.ids.truncate(first);
        self.ids.extend_from_slice(more);
        // A record made before the mesh had groups has none for its triangles.
        self.ids.resize(triangles, 0);
        self.version += 1;
    }
}

/// A golden-ratio step around the colour wheel: the hues of consecutive
/// numbers are as far apart as they can be, however many there are. Whole
/// number arithmetic, so the same number is the same hue on every machine.
fn spread(id: u16, step: u32) -> f32 {
    ((id as u32).wrapping_mul(step) >> 8) as f32 / (1u32 << 24) as f32
}

/// Where group `id`'s colour is on the colour wheel, 0 to 1.
pub fn hue(id: u16) -> f32 {
    spread(id, 2_654_435_769)
}

/// How far apart two groups' colours are on the wheel: 0 the same hue, 0.5
/// opposite.
pub fn hue_gap(a: u16, b: u16) -> f32 {
    let d = (hue(a) - hue(b)).abs();
    d.min(1.0 - d)
}

/// The display colour of group `id`, red green blue from 0 to 1. Clear,
/// fairly light colours that differ in hue first and a little in strength, so
/// that shading still reads through them and two groups of nearly the same
/// hue are still not the same colour.
pub fn colour(id: u16) -> [f32; 3] {
    let (h, s, v) = (hue(id) * 6.0, 0.50 + 0.28 * spread(id, 3_242_174_889), 1.0 - 0.14 * spread(id, 2_447_445_413));
    let (sector, f) = (h.floor(), h - h.floor());
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    match sector as u32 % 6 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

/// How many different numbers other than 0 are in `ids`.
fn count_of(ids: &[u16]) -> usize {
    let mut seen = vec![false; MOST_GROUPS + 1];
    for &id in ids {
        seen[id as usize] = true;
    }
    seen[1..].iter().filter(|&&used| used).count()
}

/// For each triangle of a mesh with polygon faces, the face it is part of
/// (`u32::MAX` for one that is part of none); `None` for a triangle mesh.
fn faces_of_triangles(mesh: &Mesh) -> Option<Vec<u32>> {
    use std::sync::atomic::{AtomicU32, Ordering};
    let polys = mesh.polys.as_ref()?;
    let face_of: Vec<AtomicU32> = (0..mesh.triangle_count()).map(|_| AtomicU32::new(u32::MAX)).collect();
    // A face's triangles are the ones made of its corners alone, found on
    // those corners. Each triangle is one face's, so no two tasks disagree.
    polys.start.par_windows(2).enumerate().with_min_len(1024).for_each(|(f, w)| {
        let corners = &polys.indices[w[0] as usize..w[1] as usize];
        for &v in corners {
            for &t in mesh.vt.row(v as usize) {
                let of = &mesh.indices[t as usize * 3..t as usize * 3 + 3];
                if of.iter().all(|c| corners.contains(c)) {
                    face_of[t as usize].store(f as u32, Ordering::Relaxed);
                }
            }
        }
    });
    Some(face_of.into_iter().map(AtomicU32::into_inner).collect())
}

/// Gives every triangle of a polygon face the number of the face's first
/// (lowest numbered) triangle, so a quad is never in two groups.
fn settle_faces(mesh: &Mesh, ids: &mut [u16]) {
    let Some(face_of) = faces_of_triangles(mesh) else { return };
    let faces = mesh.polys.as_ref().map_or(0, |polys| polys.start.len().saturating_sub(1));
    let mut first: Vec<Option<u16>> = vec![None; faces];
    for (id, &face) in ids.iter_mut().zip(&face_of) {
        if let Some(of_face) = first.get_mut(face as usize) {
            *id = *of_face.get_or_insert(*id);
        }
    }
}

/// [`Mesh::new`] for the triangles of a mesh whose group list is `ids`, in
/// the same order (a copy reflected, a file read): the new mesh has the same
/// faces in the same groups.
pub fn built_with(positions: Vec<glam::Vec3>, indices: Vec<u32>, mask: Option<Vec<f32>>, ids: &[u16]) -> Mesh {
    if ids.len() != indices.len() / 3 || ids.is_empty() {
        return Mesh::new(positions, indices, mask);
    }
    let (mut mesh, numbering) = Mesh::new_numbered(positions, indices, mask, &[]);
    mesh.set_group_ids(renumbered(ids, &numbering.triangle));
    mesh
}

/// A group list for a mesh rebuilt from the one `ids` is of, the same
/// triangles in another order: `triangle` says, for each triangle of the new
/// mesh, which of the old it is (`levels::Numbering::triangle`). Empty if
/// the old mesh had no groups or a number falls outside the list.
pub fn renumbered(ids: &[u16], triangle: &[u32]) -> Vec<u16> {
    if ids.is_empty() {
        return Vec::new();
    }
    let carried: Option<Vec<u16>> = triangle.iter().map(|&t| ids.get(t as usize).copied()).collect();
    carried.unwrap_or_default()
}

impl Mesh {
    /// Whether any face of the mesh is in a group.
    #[inline]
    pub fn has_groups(&self) -> bool {
        !self.groups.ids.is_empty()
    }

    /// One number per triangle, in the order of `indices` (0 = in no group);
    /// empty while the mesh has no groups.
    pub fn group_ids(&self) -> &[u16] {
        &self.groups.ids
    }

    /// The group triangle `triangle` is in; 0 for none.
    #[inline]
    pub fn group_of(&self, triangle: u32) -> u16 {
        self.groups.ids.get(triangle as usize).copied().unwrap_or(0)
    }

    /// How many groups the mesh has.
    pub fn group_count(&self) -> usize {
        self.groups.count
    }

    /// Changes whenever the groups do. The renderer keeps the value it last
    /// coloured the faces by.
    pub fn groups_version(&self) -> u64 {
        self.groups.version
    }

    /// Puts a group list on the mesh and gives back the one it replaces: the
    /// only way groups change apart from dynamic topology, and the whole of
    /// their undo (see [`GroupsStep`]). An empty list, or one of nothing but
    /// zeros, is no groups. A quad's triangles are made to agree.
    pub fn set_group_ids(&mut self, mut ids: Vec<u16>) -> Vec<u16> {
        debug_assert!(!self.in_stroke);
        debug_assert!(ids.is_empty() || ids.len() == self.triangle_count(), "a group list is one number per triangle");
        if !ids.is_empty() {
            ids.resize(self.triangle_count(), 0);
            settle_faces(self, &mut ids);
        }
        if ids.par_iter().with_min_len(PER_TASK).all(|&id| id == 0) {
            ids = Vec::new();
        }
        self.groups.count = count_of(&ids);
        self.groups.version += 1;
        std::mem::replace(&mut self.groups.ids, ids)
    }

    /// The triangles of group `group` (with `inside` false: every other
    /// triangle), ascending.
    pub fn group_triangles(&self, group: u16, inside: bool) -> Vec<u32> {
        (0..self.triangle_count() as u32)
            .into_par_iter()
            .with_min_len(PER_TASK)
            .filter(|&t| (self.group_of(t) == group) == inside)
            .collect()
    }

    /// One flag per vertex: whether it is on a face of group `group`. The
    /// vertices along the group's border are, as they are on both sides.
    pub fn group_vertices(&self, group: u16) -> Vec<bool> {
        use std::sync::atomic::{AtomicBool, Ordering};
        let on: Vec<AtomicBool> = (0..self.vertex_count()).map(|_| AtomicBool::new(false)).collect();
        // Threads only ever store `true`, so their order cannot matter.
        self.indices.par_chunks_exact(3).zip(self.groups.ids.par_iter()).with_min_len(PER_TASK).for_each(|(t, &id)| {
            if id == group {
                for &v in t {
                    on[v as usize].store(true, Ordering::Relaxed);
                }
            }
        });
        on.into_iter().map(AtomicBool::into_inner).collect()
    }

    /// The group of each polygon face of a quad-remeshed mesh, in the order
    /// of `polys`; `None` for a triangle mesh.
    pub fn face_groups(&self) -> Option<Vec<u16>> {
        let face_of = faces_of_triangles(self)?;
        let mut of_face = vec![0u16; self.polys.as_ref().map_or(0, |polys| polys.start.len().saturating_sub(1))];
        for (t, &face) in face_of.iter().enumerate() {
            if let Some(group) = of_face.get_mut(face as usize) {
                *group = self.group_of(t as u32);
            }
        }
        Some(of_face)
    }

    /// A number that is the same for the same groups on the same triangles
    /// and, in practice, different for any others: for checks that compare
    /// the groups before and after something that should have left them be.
    pub fn groups_print(&self) -> u64 {
        self.groups
            .ids
            .iter()
            .enumerate()
            .fold(mix(self.groups.ids.len() as u64), |print, (t, &id)| mix(print ^ ((t as u64) << 16 | id as u64)))
    }

    /// [`Self::groups_print`] for comparing two meshes that are the same
    /// surface numbered differently (one saved, the other opened from the
    /// file): every face is named by where its corners are, not by its
    /// number, so the print is the same whatever order the faces are in.
    pub fn groups_print_by_place(&self) -> u64 {
        let corner = |v: u32| self.positions[v as usize].to_array().map(|x| (x + 0.0).to_bits());
        let face = |(t, c): (usize, &[u32])| {
            let mut corners = [corner(c[0]), corner(c[1]), corner(c[2])];
            corners.sort_unstable();
            let place = corners.iter().flatten().fold(0u64, |print, &bits| mix(print ^ bits as u64));
            mix(place ^ (self.group_of(t as u32) as u64) << 40)
        };
        // A sum, so the order the faces are taken in does not matter.
        let faces = self.indices.par_chunks_exact(3).enumerate().with_min_len(PER_TASK);
        faces.map(face).reduce(|| 0, u64::wrapping_add) ^ self.group_count() as u64
    }
}

/// A well-mixed number from `x`, for the prints above.
fn mix(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// The undo step of a change to a mesh's groups: what was there before it.
/// A command that also clears the mask (Group Masked) or changes what is
/// hidden (Isolate) keeps that in the same step, so one Undo takes back all
/// of it.
pub struct GroupsStep {
    /// The group list on the other side of the step, when the step changed
    /// the groups.
    ids: Option<Vec<u16>>,
    /// What the step did to the mask.
    mask: Option<StrokeDelta>,
    /// The hidden faces on the other side of the step (one flag per
    /// triangle, or empty for nothing hidden), when the step changed them.
    hidden: Option<Vec<bool>>,
    /// Whether the step has been undone, which is the direction the mask's
    /// record is applied in next.
    undone: bool,
}

impl GroupsStep {
    /// The step of a command that replaced the group list `before` and, if
    /// it changed the mask, did so by `mask`.
    pub fn groups(before: Vec<u16>, mask: Option<StrokeDelta>) -> GroupsStep {
        GroupsStep { ids: Some(before), mask, hidden: None, undone: false }
    }

    /// The step of a command that changed which faces are hidden and nothing
    /// else; `before` is what was hidden, as `Mesh::hidden_triangles` gave it.
    pub fn hidden(before: Option<&[bool]>) -> GroupsStep {
        GroupsStep { ids: None, mask: None, hidden: Some(before.map_or(Vec::new(), <[bool]>::to_vec)), undone: false }
    }

    pub fn bytes(&self) -> usize {
        self.ids.as_ref().map_or(0, |ids| ids.len() * 2)
            + self.mask.as_ref().map_or(0, StrokeDelta::bytes)
            + self.hidden.as_ref().map_or(0, Vec::len)
    }

    /// Steps the change back, or forward again: the same swap either way.
    pub fn swap(&mut self, mesh: &mut Mesh) {
        if let Some(ids) = &mut self.ids {
            *ids = mesh.set_group_ids(std::mem::take(ids));
        }
        if let Some(mask) = &self.mask {
            mesh.apply_delta(mask, !self.undone);
        }
        if let Some(hidden) = &mut self.hidden {
            let now = mesh.hidden_triangles().map_or(Vec::new(), <[bool]>::to_vec);
            mesh.restore_hidden_triangles(hidden);
            *hidden = now;
        }
        self.undone = !self.undone;
    }
}
