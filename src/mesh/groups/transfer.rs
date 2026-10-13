//! Face groups carried onto a mesh that was built from another: a remesh, a
//! decimate, a boolean, a cut, a mesh back from Blender. None of those says
//! which new triangle came from which old one, and most could not: a voxel
//! remesh throws every old triangle away. What they all keep is the shape.
//! So each new triangle takes the group of the old surface nearest its
//! centre, which needs nothing from the routine that built the mesh and
//! puts a border back where it was to within a triangle.
//!
//! Afterwards the specks are tidied: where the old border ran through the
//! middle of a new triangle's neighbours, a lone triangle can end up in one
//! group with all three of its neighbours in another, and it joins them.
//!
//! **A shape that is not the old shape** (Blender sent back something else
//! altogether) has no groups to take: the new mesh keeps the old groups only
//! if at least half of it lies on the old surface, and otherwise starts
//! without any. Half, because a model that came back with a new arm is still
//! the same model, and one that came back as something else lies on the old
//! surface only where the two happen to cross.
//!
//! **A piece is not a rebuild** ([`Fit::Part`]). What a cut leaves, a piece
//! split off, a copy an array made: there the new mesh is the old surface in
//! part, and new surface for the rest (the cap over a cut, a copy standing
//! somewhere else). A triangle that lies on the old surface is that surface
//! and keeps its group; one that does not is new, and is in no group, rather
//! than being handed the group of whatever happened to be nearest.
//!
//! The search is the mesh's own spatial index (`bvh/nearest.rs`), on all
//! threads. New triangles are taken in their own order, which is by place, so
//! each search starts from the answer to the last and opens a leaf or two.

use std::borrow::Cow;

use glam::{Affine3A, Vec3};
use rayon::prelude::*;

use super::make::edge_neighbours;
use crate::mesh::Mesh;
use crate::mesh::bvh::nearest::closest_on_triangle;

/// New triangles searched by one task, each from the last one's answer.
const RUN: usize = 512;

/// How the new mesh came from the old ones: see the module notes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    /// The whole shape built again with other triangles.
    Whole,
    /// Part of the old surface, with new surface beside it.
    Part,
}

impl Fit {
    /// How near the old surface a new triangle's centre has to be to count
    /// as on it, as a share of the new mesh's size. A rebuild drifts off the
    /// surface by about the size of its own triangles; a piece's triangles
    /// are the old ones, or cut from them, and lie in them exactly.
    fn on_surface(self) -> f32 {
        match self {
            Fit::Whole => 0.02,
            Fit::Part => 0.002,
        }
    }
}

/// A mesh whose groups a new mesh may take.
pub struct Source<'a> {
    pub mesh: &'a Mesh,
    /// The mesh's group of each of its triangles, as they are to be called
    /// on the new mesh (a join keeps two objects' groups apart by renaming
    /// one's); empty for a mesh with no groups.
    pub ids: Cow<'a, [u16]>,
    /// Takes a point of the new mesh's own space into this mesh's.
    pub place: Affine3A,
    /// For a mesh that was cut at one of its own planes and mirrored: the
    /// axis the plane faces and whether the positive side was kept. A point
    /// on the other side is looked up at its mirror image, so the rebuilt
    /// side takes the groups of the side it is the image of, not of the side
    /// that was thrown away.
    pub fold: Option<(usize, bool)>,
}

impl<'a> Source<'a> {
    /// `mesh` as the source for a mesh built in its own space.
    pub fn of(mesh: &'a Mesh) -> Source<'a> {
        Source { mesh, ids: Cow::Borrowed(mesh.group_ids()), place: Affine3A::IDENTITY, fold: None }
    }

    /// A point of the new mesh, where this mesh has it.
    fn placed(&self, p: Vec3) -> Vec3 {
        let mut q = self.place.transform_point3(p);
        if let Some((axis, positive)) = self.fold {
            if (q[axis] > 0.0) != positive {
                q[axis] = -q[axis];
            }
        }
        q
    }

    /// The point of triangle `t` nearest `q`, both in this mesh's space.
    fn nearest_on(&self, t: u32, q: Vec3) -> Vec3 {
        let (p, i) = (&self.mesh.positions, t as usize * 3);
        let [a, b, c] = [0, 1, 2].map(|k| p[self.mesh.indices[i + k] as usize]);
        closest_on_triangle(q, a, b, c)
    }
}

/// What became of the groups when a mesh took another's place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Carried {
    /// The old mesh had none.
    Nothing,
    /// The new mesh has them: this many groups.
    Kept(usize),
    /// The new mesh is another shape, and starts without groups.
    Dropped,
}

/// Gives `new` the groups of the surfaces it was made from: see the module
/// notes. `new` is changed only if a source has groups and it lies on them.
pub fn carry(new: &mut Mesh, sources: &[Source], fit: Fit) -> Carried {
    let sources: Vec<&Source> = sources.iter().filter(|s| s.mesh.triangle_count() > 0).collect();
    if new.triangle_count() == 0 || sources.iter().all(|s| s.ids.iter().all(|&id| id == 0)) {
        return Carried::Nothing;
    }
    let (mut ids, on_surface) = nearest_groups(new, &sources, fit);
    if fit == Fit::Whole {
        if on_surface * 2 < new.triangle_count() {
            return Carried::Dropped;
        }
        for _ in 0..2 {
            tidy_specks(new, &mut ids);
        }
    }
    new.set_group_ids(ids);
    if new.has_groups() { Carried::Kept(new.group_count()) } else { Carried::Nothing }
}

/// For each triangle of `new`, the group of the source surface nearest its
/// centre (for a part: of the source surface it lies on, or none); and how
/// many of the centres lie on a source's surface.
fn nearest_groups(new: &Mesh, sources: &[&Source], fit: Fit) -> (Vec<u16>, usize) {
    let within = (new.bounds().size().length() * fit.on_surface()).powi(2);
    let back: Vec<Affine3A> = sources.iter().map(|s| s.place.inverse()).collect();
    let p = &new.positions;
    let runs: Vec<(Vec<u16>, usize)> = new
        .indices
        .par_chunks(RUN * 3)
        .map(|run| {
            let mut last: Vec<Option<u32>> = vec![None; sources.len()];
            let (mut ids, mut near) = (Vec::with_capacity(run.len() / 3), 0);
            for t in run.chunks_exact(3) {
                let centre = (p[t[0] as usize] + p[t[1] as usize] + p[t[2] as usize]) / 3.0;
                // The nearest of every source, compared in the new mesh's own space.
                let mut best = (f32::INFINITY, 0u16);
                for (k, source) in sources.iter().enumerate() {
                    let q = source.placed(centre);
                    // No further than the last answer is from here, with room for rounding.
                    let reach =
                        last[k].map_or(f32::INFINITY, |t| source.nearest_on(t, q).distance_squared(q) * 1.001 + 1e-12);
                    let mesh = source.mesh;
                    let Some((found, _)) =
                        mesh.bvh.nearest_triangle(&mesh.positions, &mesh.indices, q, reach, |_| false)
                    else {
                        continue;
                    };
                    last[k] = Some(found);
                    let (mut on, mut here) = (source.nearest_on(found, q), q);
                    if source.place != Affine3A::IDENTITY {
                        // A source may be scaled: measure where the new mesh is.
                        (on, here) = (back[k].transform_point3(on), back[k].transform_point3(here));
                    }
                    let apart = on.distance_squared(here);
                    if apart < best.0 {
                        best = (apart, source.ids.get(found as usize).copied().unwrap_or(0));
                    }
                }
                let on = best.0 <= within;
                near += usize::from(on);
                ids.push(if on || fit == Fit::Whole { best.1 } else { 0 });
            }
            (ids, near)
        })
        .collect();
    let on_surface = runs.iter().map(|(_, near)| near).sum();
    (runs.into_iter().flat_map(|(ids, _)| ids).collect(), on_surface)
}

/// A triangle whose three neighbours are all in one group that is not its
/// own joins them. Worked out from the list as it stands, so the answer does
/// not depend on the order the triangles are taken in.
pub(super) fn tidy_specks(mesh: &Mesh, ids: &mut Vec<u16>) {
    let neighbours = edge_neighbours(mesh);
    let before = &*ids;
    let after: Vec<u16> = neighbours
        .par_iter()
        .enumerate()
        .with_min_len(super::PER_TASK)
        .map(|(t, across)| {
            if across.contains(&u32::MAX) {
                return before[t];
            }
            let around = across.map(|other| before[other as usize]);
            if around[0] == around[1] && around[1] == around[2] { around[0] } else { before[t] }
        })
        .collect();
    *ids = after;
}

/// The groups of `new` if it is `old` copied out several times over, as an
/// array made real is: `old` itself, face for face, then each further copy
/// with the same faces in the same order. `triangle` says, for each triangle
/// of `new`, where it stood in the list `new` was built from
/// (`levels::Numbering::triangle`). Every copy's faces are then in the groups
/// of the faces they are copies of. `None` if `new` is not that.
///
/// The layout is the array tool's own (`sculpt_array::bake`: copy `c` of
/// face `j` is face `c * faces + j`, and the first copy is the mesh as it
/// is), so nothing has to be searched for and a copy standing anywhere,
/// turned, scaled or mirrored, gets exactly its own. What is checked before
/// believing it: the counts are a whole number of copies, and the first copy
/// is `old` to the last bit of every corner. A mesh that merely happens to
/// have twice the triangles does not pass that.
pub fn copies(old: &Mesh, new: &Mesh, triangle: &[u32]) -> Option<Vec<u16>> {
    let (faces, all) = (old.triangle_count(), new.triangle_count());
    if faces == 0 || all <= faces || all % faces != 0 || triangle.len() != all {
        return None;
    }
    if new.vertex_count() != old.vertex_count() * (all / faces) {
        return None;
    }
    let corners = |mesh: &Mesh, t: usize| [0, 1, 2].map(|k| mesh.positions[mesh.indices[t * 3 + k] as usize]);
    let first_is_old = (0..all).into_par_iter().with_min_len(super::PER_TASK).all(|t| {
        let from = triangle[t] as usize;
        from >= faces || corners(new, t) == corners(old, from)
    });
    let in_range = triangle.par_iter().all(|&from| (from as usize) < all);
    (in_range && first_is_old).then(|| triangle.iter().map(|&from| old.group_of(from % faces as u32)).collect())
}

/// The groups of several meshes renamed so that no two meshes share a
/// number: the first keeps its own, and a later one keeps each of its
/// numbers that is still free and takes the lowest free number for each that
/// is not. What a join asks for, so that the head's groups and the body's
/// stay different groups in the joined object. A mesh without groups gets an
/// empty list.
pub fn kept_apart(meshes: &[&Mesh]) -> Vec<Vec<u16>> {
    let mut used = vec![false; super::MOST_GROUPS + 1];
    used[0] = true;
    let mut next = 1;
    meshes
        .iter()
        .map(|mesh| {
            let mut own = vec![false; super::MOST_GROUPS + 1];
            for &id in mesh.group_ids() {
                own[id as usize] = true;
            }
            // This mesh's numbers that an earlier mesh has, each with its new one.
            let mut renamed: Vec<u16> = (0..=super::MOST_GROUPS as u32).map(|id| id as u16).collect();
            let clash: Vec<usize> = (1..own.len()).filter(|&id| own[id] && used[id]).collect();
            for id in 1..own.len() {
                used[id] |= own[id];
            }
            for id in clash {
                while next < used.len() && used[next] {
                    next += 1;
                }
                // With every number taken the two groups have to share one.
                if next < used.len() {
                    used[next] = true;
                    renamed[id] = next as u16;
                }
            }
            mesh.group_ids().par_iter().with_min_len(super::PER_TASK).map(|&id| renamed[id as usize]).collect()
        })
        .collect()
}
