//! Drawing only the parts of a mesh that are in view.
//!
//! A mesh used to go to the graphics card as one draw of every triangle, so a
//! model half off the screen (zoomed in on a face, say) cost as much as the
//! whole of it. Its triangles are already in the order of its spatial index
//! (mesh/bvh.rs): every leaf of the tree owns one contiguous run, and the
//! leaves are in triangle order. So a run of consecutive leaves is a run of
//! consecutive triangles, and, for the leaves the mesh was built with, a
//! compact piece of the surface. [`Chunks`] cuts the mesh into such runs of
//! about [`CHUNK_TRIS`] triangles, keeps a box around each, and each frame
//! [`Chunks::visible`] keeps the pieces whose box is in the view (or, for the
//! shadow, in the light's view) and joins the ones that follow each other
//! into single draws.
//!
//! **Why leaves and not subtrees.** A subtree of the tree is a contiguous run
//! of triangles only until dynamic topology changes the mesh: its new
//! triangles become leaves of their own at the end of the triangle list,
//! hung anywhere in the tree, and the tree is laid out afresh over the same
//! leaves when a stroke ends, which renumbers its nodes without a change of
//! topology anyone is told of. Leaves keep their triangles and their numbers
//! through all of that, so the pieces are made of leaves, and each piece's
//! box is the union of its leaves' boxes, read from the tree whenever the
//! vertices under it have moved. Pieces of leaves added by dynamic topology
//! may be spread out and their boxes large: they are drawn more often than
//! they need be, never less. Triangles no leaf holds (none should be) go in
//! pieces that are always drawn.
//!
//! **Hidden faces.** While faces are hidden the index buffer holds only the
//! triangles that show, so a piece's place in it is the number of triangles
//! that show before it, counted when the buffer is built.

use std::ops::Range;

use glam::{Mat4, Vec3, Vec4};
use rayon::prelude::*;

use crate::mesh::Mesh;
use crate::mesh::bvh::Aabb;

/// About how many triangles a piece holds: enough that a ten-million
/// triangle mesh is a few hundred pieces, few enough that a close-up leaves
/// most of them out.
pub const CHUNK_TRIS: u32 = 32_768;

/// One piece: its leaves, and where its triangles are in the index buffer.
#[derive(Clone, Debug, PartialEq)]
struct Chunk {
    /// The leaves it is made of; empty for triangles no leaf holds, which
    /// are always drawn.
    leaves: Range<u32>,
    /// Its corners in the index buffer.
    corners: Range<u32>,
}

/// A mesh cut into pieces for drawing, and a box around each.
#[derive(Default)]
pub struct Chunks {
    list: Vec<Chunk>,
    bounds: Vec<Aabb>,
    /// Every corner in the index buffer, for a draw of all of it.
    total: u32,
}

/// Cuts triangles `0..triangles` into pieces along `leaves` (each leaf's
/// triangle run, in triangle order), with `hidden` (a flag per triangle)
/// saying which triangles the index buffer leaves out. `None` if the leaves
/// are out of order or overlap: the caller then draws everything.
fn plan(leaves: &[Range<u32>], triangles: u32, hidden: Option<&[bool]>) -> Option<Vec<Chunk>> {
    // Triangles that show in tris[..t], counted as the walk goes.
    let mut shown_before = 0u32;
    let mut counted_to = 0u32;
    let mut place = |t: u32| -> u32 {
        if let Some(hidden) = hidden {
            shown_before += hidden[counted_to as usize..t as usize].iter().filter(|h| !**h).count() as u32;
        } else {
            shown_before += t - counted_to;
        }
        counted_to = t;
        shown_before * 3
    };

    let mut out = Vec::new();
    let mut at = 0u32;
    let mut i = 0usize;
    while i < leaves.len() {
        let start = leaves[i].start;
        if start < at || leaves[i].end < start || leaves[i].end > triangles {
            return None;
        }
        if start > at {
            // A gap no leaf holds: drawn always.
            let corners = place(at)..place(start);
            out.push(Chunk { leaves: 0..0, corners });
            at = start;
        }
        // Consecutive leaves up to about a piece's worth of triangles.
        let first = i;
        let begin = place(at);
        while i < leaves.len() && leaves[i].start == at && (i == first || leaves[i].end - start <= CHUNK_TRIS) {
            at = leaves[i].end;
            i += 1;
        }
        if i == first {
            // The next leaf does not carry on from here: out of order.
            return None;
        }
        out.push(Chunk { leaves: first as u32..i as u32, corners: begin..place(at) });
    }
    if at < triangles {
        let corners = place(at)..place(triangles);
        out.push(Chunk { leaves: 0..0, corners });
    }
    Some(out)
}

impl Chunks {
    /// Cuts `mesh` as its index buffer has it now (`Mesh::visible_indices`).
    pub fn build(mesh: &Mesh) -> Chunks {
        let triangles = mesh.triangle_count() as u32;
        let leaves: Vec<Range<u32>> = mesh.bvh.leaves.iter().map(|l| l.tri_start..l.tri_end).collect();
        let hidden = mesh.hidden_triangles();
        let total = mesh.visible_triangle_count() as u32 * 3;
        let list = plan(&leaves, triangles, hidden).unwrap_or_else(|| {
            log::warn!("the mesh's leaves are not in triangle order; drawing it whole");
            vec![Chunk { leaves: 0..0, corners: 0..total }]
        });
        debug_assert_eq!(list.last().map_or(0, |c| c.corners.end), total);
        let mut chunks = Chunks { bounds: vec![Aabb::EMPTY; list.len()], list, total };
        chunks.refresh(mesh);
        chunks
    }

    /// Takes each piece's box from the tree again: after vertices moved.
    pub fn refresh(&mut self, mesh: &Mesh) {
        let bvh = &mesh.bvh;
        if bvh.leaves.is_empty() {
            return;
        }
        self.list
            .par_iter()
            .zip(self.bounds.par_iter_mut())
            .with_min_len(8)
            .for_each(|(chunk, bounds)| {
                *bounds = if chunk.leaves.is_empty() {
                    Aabb { min: Vec3::splat(f32::NEG_INFINITY), max: Vec3::splat(f32::INFINITY) }
                } else {
                    chunk.leaves.clone().fold(Aabb::EMPTY, |b, l| Aabb::union(b, bvh.leaf_box(l)))
                };
            });
    }

    /// The runs of corners to draw for an object whose placement and view
    /// together are `clip` (the view's projection times the object's
    /// matrix): the pieces whose box is at least partly in view, the ones
    /// that follow each other joined.
    pub fn visible(&self, clip: &Mat4, out: &mut Vec<Range<u32>>) {
        out.clear();
        let planes = Frustum::of(clip);
        for (chunk, bounds) in self.list.iter().zip(&self.bounds) {
            if chunk.corners.is_empty() || !planes.touches(bounds) {
                continue;
            }
            match out.last_mut() {
                Some(last) if last.end == chunk.corners.start => last.end = chunk.corners.end,
                _ => out.push(chunk.corners.clone()),
            }
        }
    }

    /// All of the index buffer, as one run.
    pub fn all(&self) -> Range<u32> {
        0..self.total
    }
}

/// The four sides of a view: what is on the screen lies on the inner side of
/// all of them. (The near and far planes are left out: the sides already
/// keep out everything behind a perspective camera, and an orthographic
/// view has its eye parked outside the scene.)
struct Frustum {
    planes: [Vec4; 4],
}

impl Frustum {
    /// From a matrix taking a space to clip space, the planes in that space.
    fn of(clip: &Mat4) -> Frustum {
        let row = |i: usize| clip.row(i);
        let w = row(3);
        Frustum { planes: [w + row(0), w - row(0), w + row(1), w - row(1)] }
    }

    /// Whether `b` is at least partly on the inner side of every plane.
    /// Leans toward yes: a box kept by mistake costs a draw, one dropped by
    /// mistake leaves a hole in the model.
    fn touches(&self, b: &Aabb) -> bool {
        if b.is_empty() {
            return false;
        }
        if !(b.min.is_finite() && b.max.is_finite()) {
            return true;
        }
        let centre = (b.min + b.max) * 0.5;
        let half = (b.max - b.min) * 0.5;
        self.planes.iter().all(|p| {
            let n = p.truncate();
            let reach = n.abs().dot(half);
            let d = n.dot(centre) + p.w;
            // A margin in proportion to the numbers involved, against rounding.
            let slack = 1e-4 * (n.abs().dot(centre.abs() + half) + p.w.abs());
            d + reach >= -slack
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::primitives::{self, Primitive};

    fn runs(ranges: &[(u32, u32)]) -> Vec<Range<u32>> {
        ranges.iter().map(|&(a, b)| a..b).collect()
    }

    #[test]
    fn pieces_cover_every_corner_once_in_order() {
        let leaves = runs(&[(0, 20_000), (20_000, 40_000), (40_000, 41_000), (41_000, 90_000)]);
        let list = plan(&leaves, 100_000, None).unwrap();
        let corners: Vec<Range<u32>> = list.iter().map(|c| c.corners.clone()).collect();
        // 20k fits a piece; adding the next would pass the limit, so it starts
        // another, which takes the 1k leaf too; the 49k leaf is a piece of its
        // own however big; the tail no leaf holds is always drawn.
        assert_eq!(corners, runs(&[(0, 60_000), (60_000, 123_000), (123_000, 270_000), (270_000, 300_000)]));
        assert_eq!(list[0].leaves, 0..1);
        assert_eq!(list[1].leaves, 1..3);
        assert_eq!(list[2].leaves, 3..4);
        assert!(list[3].leaves.is_empty());
    }

    #[test]
    fn a_gap_between_leaves_is_a_piece_that_is_always_drawn() {
        let list = plan(&runs(&[(0, 10), (15, 30)]), 30, None).unwrap();
        let corners: Vec<Range<u32>> = list.iter().map(|c| c.corners.clone()).collect();
        assert_eq!(corners, runs(&[(0, 30), (30, 45), (45, 90)]));
        assert!(list[1].leaves.is_empty());
        // Leaves out of order, or overlapping, are refused.
        assert!(plan(&runs(&[(10, 20), (0, 10)]), 20, None).is_none());
        assert!(plan(&runs(&[(0, 12), (10, 20)]), 20, None).is_none());
        assert!(plan(&runs(&[(0, 30)]), 20, None).is_none());
        // Nothing at all.
        assert_eq!(plan(&[], 0, None).unwrap(), vec![]);
    }

    #[test]
    fn with_faces_hidden_a_piece_starts_where_its_first_shown_triangle_is() {
        // Every third triangle hidden.
        let hidden: Vec<bool> = (0..90).map(|t| t % 3 == 0).collect();
        let list = plan(&runs(&[(0, 30), (30, 60), (60, 90)]), 90, Some(&hidden)).unwrap();
        // One piece (90 triangles fit): 60 that show.
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].corners, 0..180);
        // Pieces split by size: each starts at the count shown before it.
        let hidden: Vec<bool> = (0..(3 * CHUNK_TRIS)).map(|t| t < CHUNK_TRIS / 2).collect();
        let leaves = runs(&[(0, CHUNK_TRIS), (CHUNK_TRIS, 2 * CHUNK_TRIS), (2 * CHUNK_TRIS, 3 * CHUNK_TRIS)]);
        let list = plan(&leaves, 3 * CHUNK_TRIS, Some(&hidden)).unwrap();
        let half = CHUNK_TRIS / 2 * 3;
        let corners: Vec<Range<u32>> = list.iter().map(|c| c.corners.clone()).collect();
        assert_eq!(corners, vec![0..half, half..half + CHUNK_TRIS * 3, half + CHUNK_TRIS * 3..half + CHUNK_TRIS * 6]);
    }

    /// Checks that each piece's corners are the triangles of its leaves that
    /// show, in the order `visible_indices` has them, and that its box holds
    /// every one of those triangles.
    fn check(mesh: &Mesh) {
        let chunks = Chunks::build(mesh);
        let shown = mesh.visible_indices();
        assert_eq!(chunks.all(), 0..shown.len() as u32);
        let mut next = 0u32;
        for (chunk, bounds) in chunks.list.iter().zip(&chunks.bounds) {
            assert_eq!(chunk.corners.start, next, "pieces follow each other");
            next = chunk.corners.end;
            for corner in chunk.corners.clone() {
                let p = mesh.positions[shown[corner as usize] as usize];
                assert!(p.cmpge(bounds.min).all() && p.cmple(bounds.max).all(), "a corner outside its piece's box");
            }
            if !chunk.leaves.is_empty() {
                let leaves = &mesh.bvh.leaves[chunk.leaves.start as usize..chunk.leaves.end as usize];
                let hidden = mesh.hidden_triangles();
                let want: Vec<u32> = (leaves[0].tri_start..leaves.last().unwrap().tri_end)
                    .filter(|&t| hidden.is_none_or(|h| !h[t as usize]))
                    .flat_map(|t| mesh.indices[t as usize * 3..t as usize * 3 + 3].to_vec())
                    .collect();
                assert_eq!(&shown[chunk.corners.start as usize..chunk.corners.end as usize], &want[..]);
            }
        }
        assert_eq!(next, shown.len() as u32);
    }

    #[test]
    fn pieces_of_a_real_mesh_hold_their_triangles_hidden_faces_and_dynamic_topology_included() {
        let mut mesh = primitives::build(Primitive::Sphere, 200_000, 1.0);
        check(&mesh);
        assert!(Chunks::build(&mesh).list.len() > 4);
        let hide: Vec<u32> = (0..mesh.triangle_count() as u32).filter(|t| t % 7 < 3).collect();
        mesh.hide(&hide);
        check(&mesh);

        // Dynamic topology appends triangles and leaves, rewrites some in
        // place, and a stroke's end lays the tree out afresh.
        let mut mesh = primitives::build(Primitive::Sphere, 20_000, 1.0);
        mesh.begin_stroke();
        let stats = mesh.refine(&[Vec3::Z], 0.4, 0.01, 1_000_000, 10_000_000);
        assert!(stats.splits > 1000);
        check(&mesh);
        let _ = mesh.end_stroke(false);
        check(&mesh);
        let hide: Vec<u32> = (0..mesh.triangle_count() as u32).filter(|t| t % 5 == 0).collect();
        mesh.hide(&hide);
        check(&mesh);
    }

    /// Looking down -Z from z = 5 at the origin, a square view 90 degrees
    /// across (or, orthographic, 2 * `distance` across).
    fn view(distance: f32, ortho: bool) -> Mat4 {
        let mut camera = crate::camera::Camera::default();
        (camera.distance, camera.fov_y, camera.ortho) = (distance, 90.0, ortho);
        camera.view(glam::Vec2::ZERO, glam::Vec2::splat(100.0), distance, 1.0).view_proj
    }

    fn camera() -> Mat4 {
        view(5.0, false)
    }

    fn cube(at: Vec3) -> Aabb {
        Aabb { min: at - 0.5, max: at + 0.5 }
    }

    #[test]
    fn boxes_in_view_are_kept_and_those_off_screen_or_behind_are_not() {
        let view = Frustum::of(&camera());
        assert!(view.touches(&cube(Vec3::ZERO)));
        // Straddling the left edge (at x = -5 for z = 0, -5.5 at the box's far side).
        assert!(view.touches(&cube(Vec3::new(-5.2, 0.0, 0.0))));
        assert!(view.touches(&cube(Vec3::new(-5.9, 0.0, 0.0))));
        assert!(!view.touches(&cube(Vec3::new(-6.2, 0.0, 0.0))));
        assert!(!view.touches(&cube(Vec3::new(0.0, 7.0, 0.0))));
        // Behind the camera.
        assert!(!view.touches(&cube(Vec3::new(0.0, 0.0, 9.0))));
        assert!(!view.touches(&Aabb::EMPTY));
        // Far away in front, however far.
        assert!(view.touches(&cube(Vec3::new(0.0, 0.0, -1e5))));
        // An object's own space: a mirrored placement moves the box across.
        let mirror = Mat4::from_translation(Vec3::new(-7.0, 0.0, 0.0)) * Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0));
        let placed = Frustum::of(&(camera() * mirror));
        // (-7 - x): the box at x = -7 lands at 0, in view; at x = 0 it lands at -7, out.
        assert!(placed.touches(&cube(Vec3::new(-7.0, 0.0, 0.0))));
        assert!(!placed.touches(&cube(Vec3::ZERO)));
    }

    #[test]
    fn pieces_in_view_are_joined_into_as_few_draws_as_possible() {
        let boxes = [Vec3::ZERO, Vec3::new(0.5, 0.0, 0.0), Vec3::new(30.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0)];
        let chunks = Chunks {
            list: (0..4u32).map(|i| Chunk { leaves: i..i + 1, corners: i * 30..i * 30 + 30 }).collect(),
            bounds: boxes.iter().map(|&p| cube(p)).collect(),
            total: 120,
        };
        let mut out = Vec::new();
        chunks.visible(&camera(), &mut out);
        assert_eq!(out, runs(&[(0, 60), (90, 120)]));
        // A view that takes in all of it: one draw.
        chunks.visible(&view(100.0, true), &mut out);
        assert_eq!(out, vec![chunks.all()]);
    }
}
