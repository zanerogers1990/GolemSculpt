//! Hidden faces: part of a mesh switched off, so that it is not drawn, not
//! hit by the pointer and not touched by brushes until it is shown again.
//! This is what the box and lasso hide tools change ("hide the arm so I can
//! reach the ribs behind it").
//!
//! Hiding is kept per triangle, because that is what hiding part of a surface
//! means: a vertex on the edge of the hidden area also belongs to triangles
//! that still show. The per-vertex answer is worked out from it: a vertex is
//! hidden only when every triangle on it is, so the cut edge stays ordinary,
//! sculptable surface rather than a frozen wall.
//!
//! Only what is drawn and what is hit shrinks. `Mesh::indices`, the adjacency
//! and every per-vertex array keep the whole mesh, so vertex and triangle
//! numbers (undo records, the mask, mirror twins) mean the same before and
//! after a hide, and the normals along a cut edge do not change. It also
//! means that everything which reads `indices` (remesh, join, export, save)
//! works on the whole model, hidden part included, as the Unity app does.
//!
//! Nothing hidden is the normal case and costs nothing: both flag lists are
//! empty, and the ray test and the stroke loops are each built twice, once in
//! a form that never looks at them (`Bvh::raycast`, `Mesh::edit_leaves`).

use std::borrow::Cow;

use glam::Vec3;
use rayon::prelude::*;
use sculpt_cut::select;

use super::Mesh;

/// Below this many triangles a parallel split costs more than it saves.
const MIN_PARALLEL: usize = 4096;

/// Which faces of a mesh are switched off. Both lists are empty while nothing
/// is hidden.
#[derive(Default)]
pub(super) struct Hidden {
    /// One flag per triangle, in the order of `Mesh::indices`.
    tris: Vec<bool>,
    /// One flag per vertex, set when every triangle on the vertex is hidden.
    /// This is the list the stroke loops read.
    pub(super) verts: Vec<bool>,
    /// How many triangles are hidden.
    count: usize,
    /// Goes up with every change, so the renderer can tell that the faces it
    /// was sent are no longer the ones that show.
    version: u64,
}

impl Hidden {
    pub(super) fn bytes(&self) -> usize {
        self.tris.len() + self.verts.len()
    }
}

/// What a hide, show or invert changed; enough to undo and redo it.
pub enum VisibilityDelta {
    /// These triangles (ascending, each once) became hidden or, with `hidden`
    /// false, were shown again. Only triangles that really changed are listed,
    /// so hiding the same area twice leaves no second step that undoes
    /// nothing.
    Set { triangles: Vec<u32>, hidden: bool },
    /// Hidden and showing swapped over the whole mesh. A swap is its own
    /// opposite, so there is nothing to keep, however large the mesh.
    Invert,
}

impl VisibilityDelta {
    pub fn bytes(&self) -> usize {
        match self {
            VisibilityDelta::Set { triangles, .. } => triangles.len() * 4,
            VisibilityDelta::Invert => 0,
        }
    }
}

impl Mesh {
    /// [`Mesh::new`] for a mesh that was saved with part of it hidden.
    /// `hidden` holds one flag per triangle of `indices` as given here;
    /// building a mesh reorders its triangles, and the flags follow them.
    /// A list of the wrong length is ignored, since flags for another
    /// topology would hide the wrong faces.
    pub fn new_with_hidden(
        positions: Vec<Vec3>,
        indices: Vec<u32>,
        mask: Option<Vec<f32>>,
        hidden: &[bool],
    ) -> Mesh {
        Self::new_mapped_with_hidden(positions, indices, mask, hidden).0
    }

    /// [`Mesh::new_with_hidden`], also returning where each input vertex
    /// went, as [`Mesh::new_mapped`] does.
    pub fn new_mapped_with_hidden(
        positions: Vec<Vec3>,
        indices: Vec<u32>,
        mask: Option<Vec<f32>>,
        hidden: &[bool],
    ) -> (Mesh, Vec<u32>) {
        if hidden.len() != indices.len() / 3 || !hidden.contains(&true) {
            return Mesh::new_mapped(positions, indices, mask);
        }
        // The flags of the triangles a build keeps, in the order it keeps them.
        let vertex_count = positions.len() as u32;
        let kept: Vec<bool> = indices
            .chunks_exact(3)
            .zip(hidden)
            .filter(|(t, _)| super::sound(t, vertex_count))
            .map(|(_, &flag)| flag)
            .collect();
        let (mut mesh, vertex_went, came_from) = Mesh::new_ordered(positions, indices, mask);
        let flags: Vec<bool> = came_from.iter().map(|&t| kept[t as usize]).collect();
        mesh.restore_hidden_triangles(&flags);
        (mesh, vertex_went)
    }

    /// Whether any face is hidden. One comparison, so it can be asked every
    /// frame and before every dab.
    #[inline]
    pub fn any_hidden(&self) -> bool {
        self.hidden.count > 0
    }

    /// One flag per triangle (true = hidden) in the order of `indices`, or
    /// `None` while nothing is hidden. This is what a saved file should keep
    /// and what `sculpt_cut::select::plan_hide` asks for.
    pub fn hidden_triangles(&self) -> Option<&[bool]> {
        self.any_hidden().then_some(&self.hidden.tris[..])
    }

    /// One flag per vertex, or `None` while nothing is hidden. A vertex is
    /// hidden only when every triangle on it is, so the vertices along the
    /// edge of a hidden area are not.
    #[allow(dead_code)] // for the region mask tool
    pub fn hidden_vertices(&self) -> Option<&[bool]> {
        self.any_hidden().then_some(&self.hidden.verts[..])
    }

    #[inline]
    #[allow(dead_code)] // for callers that ask about one face
    pub fn is_triangle_hidden(&self, triangle: u32) -> bool {
        self.any_hidden() && self.hidden.tris.get(triangle as usize).copied().unwrap_or(false)
    }

    #[inline]
    #[allow(dead_code)] // for callers that ask about one vertex
    pub fn is_vertex_hidden(&self, vertex: u32) -> bool {
        self.any_hidden() && self.hidden.verts.get(vertex as usize).copied().unwrap_or(false)
    }

    pub fn hidden_triangle_count(&self) -> usize {
        self.hidden.count
    }

    pub fn visible_triangle_count(&self) -> usize {
        self.triangle_count() - self.hidden.count
    }

    /// Changes whenever faces are hidden or shown. The renderer keeps the
    /// value it last drew with and sends the faces again when it differs.
    pub fn visibility_version(&self) -> u64 {
        self.hidden.version
    }

    /// The corners of the triangles that show, in their own order: what the
    /// renderer draws. The whole of `indices`, without a copy, while nothing
    /// is hidden.
    pub fn visible_indices(&self) -> Cow<'_, [u32]> {
        match self.hidden_triangles() {
            None => Cow::Borrowed(&self.indices[..]),
            Some(hidden) => Cow::Owned(select::visible_indices(&self.indices, hidden)),
        }
    }

    /// Hides the given triangles. Returns the undo record, or `None` if every
    /// one of them was hidden already. Numbers past the end are ignored.
    #[allow(dead_code)] // the short form of `set_triangles_hidden`
    pub fn hide(&mut self, triangles: &[u32]) -> Option<VisibilityDelta> {
        self.set_triangles_hidden(triangles, true)
    }

    /// Shows the given triangles again; the opposite of [`Self::hide`].
    #[allow(dead_code)] // the short form of `set_triangles_hidden`
    pub fn show(&mut self, triangles: &[u32]) -> Option<VisibilityDelta> {
        self.set_triangles_hidden(triangles, false)
    }

    /// Hides (or with `hidden` false shows) the given triangles: the one call
    /// behind a hide or show gesture. Returns the undo record, or `None` if
    /// nothing changed.
    pub fn set_triangles_hidden(&mut self, triangles: &[u32], hidden: bool) -> Option<VisibilityDelta> {
        debug_assert!(!self.in_stroke);
        if !hidden && !self.any_hidden() {
            return None;
        }
        self.open_flags();
        let flags = &mut self.hidden.tris;
        // A triangle listed twice only changes, and is only recorded, once.
        let mut changed = Vec::new();
        for &t in triangles {
            if let Some(flag) = flags.get_mut(t as usize) {
                if *flag != hidden {
                    *flag = hidden;
                    changed.push(t);
                }
            }
        }
        if changed.is_empty() {
            self.close_flags();
            return None;
        }
        changed.sort_unstable();
        self.refresh_hidden();
        Some(VisibilityDelta::Set { triangles: changed, hidden })
    }

    /// Shows every hidden face again. `None` if nothing was hidden.
    pub fn show_all(&mut self) -> Option<VisibilityDelta> {
        debug_assert!(!self.in_stroke);
        let hidden = self.hidden_triangles()?;
        let triangles: Vec<u32> = (0..hidden.len() as u32)
            .into_par_iter()
            .with_min_len(MIN_PARALLEL)
            .filter(|&t| hidden[t as usize])
            .collect();
        self.hidden.tris = Vec::new();
        self.refresh_hidden();
        Some(VisibilityDelta::Set { triangles, hidden: false })
    }

    /// Swaps hidden and showing over the whole mesh. `None`, and nothing
    /// happens, when nothing is hidden: the swap would hide the entire
    /// object, which looks exactly like having deleted it.
    pub fn invert_visible(&mut self) -> Option<VisibilityDelta> {
        debug_assert!(!self.in_stroke);
        if !self.any_hidden() {
            return None;
        }
        self.flip_all();
        Some(VisibilityDelta::Invert)
    }

    /// Steps a hide, show or invert back (`undo`) or forward again.
    pub fn apply_visibility(&mut self, delta: &VisibilityDelta, undo: bool) {
        debug_assert!(!self.in_stroke);
        match delta {
            VisibilityDelta::Set { triangles, hidden } => {
                let value = *hidden != undo;
                self.open_flags();
                let flags = &mut self.hidden.tris;
                for &t in triangles {
                    if let Some(flag) = flags.get_mut(t as usize) {
                        *flag = value;
                    }
                }
                self.refresh_hidden();
            }
            VisibilityDelta::Invert => self.flip_all(),
        }
    }

    /// Puts back flags read earlier with [`Self::hidden_triangles`] from this
    /// same mesh (or `&[]` to show everything). Not an undo step: it is for
    /// restoring a state, as loading does. Returns false, changing nothing,
    /// if the list is not one flag per triangle.
    ///
    /// Flags saved before the mesh was rebuilt belong to the old triangle
    /// order; [`Mesh::new_with_hidden`] is the way in for those.
    pub fn restore_hidden_triangles(&mut self, hidden: &[bool]) -> bool {
        debug_assert!(!self.in_stroke);
        if !hidden.is_empty() && hidden.len() != self.triangle_count() {
            return false;
        }
        if hidden.is_empty() && !self.any_hidden() {
            return true;
        }
        self.hidden.tris = hidden.to_vec();
        self.refresh_hidden();
        true
    }

    /// The wireframe's vertex pairs while part of the mesh is hidden: one
    /// pair per edge of a face that shows (of the quads a quad remesh left,
    /// else of the triangles). `None` while nothing is hidden, when the
    /// overlay's own quicker way of listing every edge is right.
    pub fn visible_wire_lines(&self) -> Option<Vec<u32>> {
        let hidden = self.hidden_triangles()?;
        let indices = &self.indices;
        // Whether a showing triangle on `a` also has `b`: next after `a` in
        // its winding, or with `either_way` on either side of it.
        let shows = |a: u32, b: u32, either_way: bool| {
            let fan = &self.vt[self.vt_off[a as usize] as usize..self.vt_off[a as usize + 1] as usize];
            fan.iter().any(|&t| {
                let t = t as usize;
                if hidden[t] {
                    return false;
                }
                let c = &indices[t * 3..t * 3 + 3];
                (0..3).any(|k| c[k] == a && (c[(k + 1) % 3] == b || (either_way && c[(k + 2) % 3] == b)))
            })
        };
        if let Some(polys) = &self.polys {
            // A quad's sides are triangle edges too (only its diagonal is
            // not), so a side shows exactly when a showing triangle has it.
            let mut edges: Vec<(u32, u32)> = Vec::with_capacity(polys.indices.len());
            for f in polys.start.windows(2) {
                let face = &polys.indices[f[0] as usize..f[1] as usize];
                for (k, &a) in face.iter().enumerate() {
                    let b = face[(k + 1) % face.len()];
                    edges.push((a.min(b), a.max(b)));
                }
            }
            edges.par_sort_unstable();
            edges.dedup();
            return Some(
                edges
                    .into_par_iter()
                    .with_min_len(MIN_PARALLEL)
                    .filter(|&(a, b)| shows(a, b, true))
                    .flat_map_iter(|(a, b)| [a, b])
                    .collect(),
            );
        }
        // Two triangles run a shared edge in opposite directions, so the side
        // running low -> high takes each edge once. Along the cut that twin
        // may be the hidden one; then the high -> low side has to draw it.
        Some(
            indices
                .par_chunks_exact(3)
                .zip(hidden.par_iter())
                .with_min_len(MIN_PARALLEL)
                .filter(|&(_, &off)| !off)
                .flat_map_iter(|(t, _)| {
                    [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])]
                        .into_iter()
                        .filter(|&(a, b)| a < b || !shows(b, a, false))
                        .flat_map(|(a, b)| [a, b])
                })
                .collect(),
        )
    }

    /// Makes room for one flag per triangle (all showing) if there is none.
    fn open_flags(&mut self) {
        if self.hidden.tris.len() != self.triangle_count() {
            self.hidden.tris = vec![false; self.triangle_count()];
        }
    }

    /// Lets the flags go again when none of them is set.
    fn close_flags(&mut self) {
        if self.hidden.count == 0 {
            self.hidden.tris = Vec::new();
            self.hidden.verts = Vec::new();
        }
    }

    fn flip_all(&mut self) {
        self.open_flags();
        self.hidden.tris.par_iter_mut().with_min_len(MIN_PARALLEL).for_each(|flag| *flag = !*flag);
        self.refresh_hidden();
    }

    /// Brings the count, the per-vertex flags and the version up to date
    /// after the per-triangle flags changed. One pass over the mesh, paid per
    /// hide or show, never per frame.
    fn refresh_hidden(&mut self) {
        self.hidden.version += 1;
        self.hidden.count =
            self.hidden.tris.par_iter().with_min_len(MIN_PARALLEL).filter(|&&flag| flag).count();
        if self.hidden.count == 0 {
            self.close_flags();
            return;
        }
        self.hidden.verts = select::hidden_vertices(self.positions.len(), &self.indices, &self.hidden.tris);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use glam::Vec2;

    use super::*;
    use crate::camera::{Camera, View};
    use crate::history::History;
    use crate::mesh::primitives::{self, Primitive};
    use crate::mesh::{Polys, StrokeDelta};
    use crate::sculpt::{Brush, PointerSample, Sculptor};

    const SIZE: Vec2 = Vec2::new(1600.0, 900.0);

    fn sphere(tris: usize) -> Mesh {
        primitives::build(Primitive::Sphere, tris, 0.5)
    }

    fn view_of(mesh: &Mesh) -> View {
        let mut camera = Camera::default();
        camera.frame(mesh.bounds(), SIZE.x / SIZE.y, false);
        camera.view(Vec2::ZERO, SIZE, camera.distance, 1.0)
    }

    /// The triangles with all three corners where `inside` says, as the box
    /// and lasso tools pick them.
    fn triangles_where(mesh: &Mesh, inside: impl Fn(Vec3) -> bool) -> Vec<u32> {
        let flags: Vec<bool> = mesh.positions.iter().map(|&p| inside(p)).collect();
        select::triangles_inside(&mesh.indices, &flags)
    }

    fn flags_of(mesh: &Mesh) -> Vec<bool> {
        mesh.hidden_triangles().map_or(vec![false; mesh.triangle_count()], <[bool]>::to_vec)
    }

    /// Drags the pointer along `points` (pixels), a few samples per frame.
    fn run_stroke(mesh: &mut Mesh, sculptor: &mut Sculptor, view: &View, points: &[Vec2]) -> Option<StrokeDelta> {
        let sample = |i: usize| PointerSample { pos: points[i], pressure: None, time: i as f64 / 240.0 };
        assert!(sculptor.begin(mesh, view, sample(0), false, false), "stroke missed the mesh");
        for i in 1..points.len() {
            sculptor.sample(mesh, view, sample(i));
            if i % 4 == 0 {
                sculptor.frame(mesh, view, 1.0 / 60.0);
                let _ = mesh.take_dirty();
            }
        }
        let delta = sculptor.end(mesh, view);
        let _ = mesh.take_dirty();
        delta
    }

    fn changed(delta: &Option<StrokeDelta>) -> &[u32] {
        match delta {
            Some(StrokeDelta::Positions { verts, .. }) | Some(StrokeDelta::Mask { verts, .. }) => verts,
            None => &[],
        }
    }

    #[test]
    fn a_ray_passes_through_a_hidden_patch_and_hits_what_is_behind() {
        let mut mesh = sphere(20_000);
        let (origin, dir) = (Vec3::new(0.0, 0.0, 3.0), Vec3::NEG_Z);
        let front = mesh.raycast(origin, dir).expect("hits the front");
        assert!((front.pos.z - 0.5).abs() < 1e-3);

        // Hide the cap facing the ray.
        let cap = triangles_where(&mesh, |p| p.z > 0.25);
        assert!(cap.len() > 100 && cap.len() < mesh.triangle_count() / 2);
        assert!(mesh.hide(&cap).is_some());
        let through = mesh.raycast(origin, dir).expect("hits the far wall instead");
        assert!((through.pos.z + 0.5).abs() < 1e-3, "{:?}", through.pos);
        assert!(!mesh.is_triangle_hidden(through.tri));
        // The inside of the far wall, turned to face the ray.
        assert!(through.normal.dot(Vec3::Z) > 0.99);
        let start = mesh.raycast_start(origin, dir).expect("the stroke-start test agrees");
        assert_eq!(start.tri, through.tri);

        // No ray anywhere lands on a hidden face, and a ray that never met
        // the hidden cap is not disturbed.
        for i in 0..2000 {
            let a = i as f32 * 0.37;
            let from = Vec3::new(a.cos() * (i % 7) as f32 * 0.07, a.sin() * (i % 5) as f32 * 0.09, 3.0);
            if let Some(hit) = mesh.raycast(from, dir) {
                assert!(!mesh.is_triangle_hidden(hit.tri), "ray {i} stopped on a hidden face");
            }
        }
        let side = mesh.raycast(Vec3::new(3.0, 0.0, 0.0), Vec3::NEG_X).expect("side ray");
        assert!((side.pos.x - 0.5).abs() < 1e-3);

        assert!(mesh.show_all().is_some());
        let again = mesh.raycast(origin, dir).expect("front is back");
        assert_eq!(again.tri, front.tri);
    }

    #[test]
    fn rays_do_not_leak_between_triangles_while_part_is_hidden() {
        // With the back hidden, a ray slipping through a seam of the front
        // would hit nothing at all, so a single leak shows up as a miss.
        let mut mesh = sphere(300_000);
        let back = triangles_where(&mesh, |p| p.z < -0.05);
        assert!(mesh.hide(&back).is_some());
        let view = view_of(&mesh);
        let centre = view.project(Vec3::ZERO).unwrap();
        let rim = view.project(Vec3::new(0.5, 0.0, 0.0)).unwrap().distance(centre);
        let mut rays = 0;
        for y in 0..160 {
            for x in 0..160 {
                let offset = Vec2::new(x as f32 - 79.5, y as f32 - 79.5) * (rim / 80.0) * 0.9;
                if offset.length() > rim * 0.9 {
                    continue;
                }
                let (o, d) = view.ray(centre + offset);
                let hit = mesh.raycast(o, d).unwrap_or_else(|| panic!("ray at {offset} leaked through the front"));
                assert!(hit.pos.z > -0.05 - 1e-3);
                rays += 1;
            }
        }
        assert!(rays > 15_000);
    }

    #[test]
    fn stroke_loops_pass_over_hidden_vertices_and_keep_their_numbering() {
        let mut mesh = sphere(20_000);
        let leaves: Vec<u32> = (0..mesh.bvh.leaves.len() as u32).collect();
        let all: Vec<u32> = (0..mesh.vertex_count() as u32).collect();
        let rest = mesh.positions.clone();

        // Each vertex's place in the leaves' order, with nothing hidden.
        mesh.begin_stroke();
        mesh.move_vertices(&leaves, |at, _| Some(Vec3::new(at as f32, 0.0, 0.0)));
        let place: Vec<f32> = mesh.positions.iter().map(|p| p.x).collect();
        let delta = mesh.end_stroke(false).unwrap();
        mesh.apply_delta(&delta, true);
        assert_eq!(mesh.positions, rest);

        // Everything up to the plane x = 0, whose own vertices are the seam.
        assert!(mesh.hide(&triangles_where(&mesh, |p| p.x <= 0.0)).is_some());
        let hidden = mesh.hidden_vertices().unwrap().to_vec();
        let hidden_count = hidden.iter().filter(|h| **h).count();
        assert!(hidden_count > 1000 && hidden_count < mesh.vertex_count() / 2);
        for (p, h) in rest.iter().zip(&hidden) {
            // The seam has showing faces on it, so it stays live.
            assert_eq!(*h, p.x < 0.0);
        }

        mesh.begin_stroke();
        mesh.move_vertices(&leaves, |at, _| Some(Vec3::new(at as f32, 1.0, 0.0)));
        for v in 0..mesh.vertex_count() {
            if hidden[v] {
                assert_eq!(mesh.positions[v], rest[v], "hidden vertex {v} moved");
            } else {
                assert_eq!(mesh.positions[v], Vec3::new(place[v], 1.0, 0.0), "vertex {v} lost its place");
            }
        }
        mesh.move_listed(&all, |slot, _| Some(Vec3::new(slot as f32, 2.0, 0.0)));
        for v in 0..mesh.vertex_count() {
            let want = if hidden[v] { rest[v] } else { Vec3::new(v as f32, 2.0, 0.0) };
            assert_eq!(mesh.positions[v], want);
        }
        // A short list runs on one thread; the same rule holds there.
        mesh.move_listed(&all[..64], |_, _| Some(Vec3::splat(9.0)));
        assert!((0..64).all(|v| (mesh.positions[v] == rest[v]) == hidden[v]));
        let moved = mesh.end_stroke(false);
        assert_eq!(changed(&moved).len(), mesh.vertex_count() - hidden_count);

        mesh.begin_stroke();
        mesh.paint_mask(&leaves, |_| Some(1.0));
        assert!(mesh.mask.iter().zip(&hidden).all(|(m, h)| (*m == 0.0) == *h));
        mesh.end_stroke(true);
    }

    #[test]
    fn a_brush_dab_over_a_half_hidden_area_moves_only_visible_vertices() {
        // The stroke runs down the front just right of the cut, with a brush
        // wide enough to reach well over it.
        let centre = SIZE * 0.5;
        let points: Vec<Vec2> =
            (0..=40).map(|i| centre + Vec2::new(14.0, -90.0 + 180.0 * i as f32 / 40.0)).collect();
        for (brush, mask_mode) in Brush::ALL.into_iter().map(|b| (b, false)).chain([(Brush::Standard, true)]) {
            let mut mesh = sphere(80_000);
            let view = view_of(&mesh);
            let left = triangles_where(&mesh, |p| p.x <= 0.0);
            let what = if mask_mode { "Mask" } else { brush.name() };
            let mut sculptor = Sculptor::default();
            sculptor.settings.brush = brush;
            sculptor.settings.mask_mode = mask_mode;
            sculptor.settings.radius_px = 70.0;
            sculptor.settings.per_brush[brush.index()].strength = 0.8;

            // First with everything showing: the stroke does reach the part
            // about to be hidden, so the second half of the test means
            // something.
            assert!(mesh.hide(&left).is_some());
            let hidden = mesh.hidden_vertices().unwrap().to_vec();
            assert!(mesh.show_all().is_some());
            let free = run_stroke(&mut mesh, &mut sculptor, &view, &points);
            let reached = changed(&free).iter().filter(|&&v| hidden[v as usize]).count();
            assert!(reached > 20, "{what}: the open stroke only reached {reached} vertices left of the cut");
            mesh.apply_delta(free.as_ref().unwrap(), true);
            let _ = mesh.take_dirty();

            let (rest, rest_mask) = (mesh.positions.clone(), mesh.mask.clone());
            assert!(mesh.hide(&left).is_some());
            let delta = run_stroke(&mut mesh, &mut sculptor, &view, &points);
            let verts = changed(&delta);
            assert!(verts.len() > 50, "{what}: the stroke still works on what shows ({})", verts.len());
            assert!(verts.iter().all(|&v| !hidden[v as usize]), "{what}: a hidden vertex is in the undo record");
            for v in 0..mesh.vertex_count() {
                if hidden[v] {
                    assert_eq!(mesh.positions[v], rest[v], "{what}: hidden vertex {v} moved");
                    assert_eq!(mesh.mask[v], rest_mask[v], "{what}: hidden vertex {v} was masked");
                }
            }
        }
    }

    #[test]
    fn hide_then_undo_then_redo() {
        let mut mesh = sphere(20_000);
        let mut history = History::default();
        let all_faces = mesh.indices.clone();
        let patch = triangles_where(&mesh, |p| p.y > 0.1);

        history.push_visibility(mesh.hide(&patch).unwrap());
        let version = mesh.visibility_version();
        let hidden = flags_of(&mesh);
        assert_eq!(mesh.hidden_triangle_count(), patch.len());
        assert!(patch.iter().all(|&t| mesh.is_triangle_hidden(t)));
        let shown = mesh.visible_indices().into_owned();
        assert_eq!(shown.len(), all_faces.len() - patch.len() * 3);

        // Undo: every face back, and the renderer is told to send them.
        assert_eq!(history.undo(&mut mesh), Some(false), "the mesh itself is not swapped");
        assert!(!mesh.any_hidden() && mesh.hidden_triangles().is_none() && mesh.hidden_vertices().is_none());
        assert_eq!(&mesh.visible_indices()[..], &all_faces[..]);
        assert!(mesh.visibility_version() > version);
        let version = mesh.visibility_version();

        // Redo: the same faces hidden again.
        assert_eq!(history.redo(&mut mesh), Some(false));
        assert_eq!(flags_of(&mesh), hidden);
        assert_eq!(&mesh.visible_indices()[..], &shown[..]);
        assert!(mesh.visibility_version() > version);
        assert_eq!(history.steps(), (1, 0));

        // The mesh's own triangles were never touched by any of it.
        assert_eq!(mesh.indices, all_faces);
    }

    #[test]
    fn invert_twice_is_a_round_trip() {
        let mut mesh = sphere(20_000);
        let mut history = History::default();
        // Nothing hidden: inverting would hide the whole object, so it does
        // nothing.
        assert!(mesh.invert_visible().is_none());
        assert_eq!(mesh.visibility_version(), 0);

        history.push_visibility(mesh.hide(&triangles_where(&mesh, |p| p.x > 0.2)).unwrap());
        let once = flags_of(&mesh);
        let hidden_vertices = mesh.hidden_vertices().unwrap().to_vec();

        history.push_visibility(mesh.invert_visible().unwrap());
        let flipped = flags_of(&mesh);
        assert!(flipped.iter().zip(&once).all(|(a, b)| a != b));
        assert_eq!(mesh.hidden_triangle_count(), mesh.triangle_count() - once.iter().filter(|h| **h).count());
        // The seam belongs to both sides, so it is hidden on neither.
        let other_side = mesh.hidden_vertices().unwrap();
        assert!(hidden_vertices.iter().zip(other_side).all(|(a, b)| !(*a && *b)));
        assert!(hidden_vertices.iter().zip(other_side).any(|(a, b)| !*a && !*b));

        history.push_visibility(mesh.invert_visible().unwrap());
        assert_eq!(flags_of(&mesh), once);
        assert_eq!(mesh.hidden_vertices().unwrap(), &hidden_vertices[..]);

        // And each invert is its own undo step.
        assert_eq!(history.undo(&mut mesh), Some(false));
        assert_eq!(flags_of(&mesh), flipped);
        assert_eq!(history.undo(&mut mesh), Some(false));
        assert_eq!(flags_of(&mesh), once);
        assert_eq!(history.redo(&mut mesh), Some(false));
        assert_eq!(flags_of(&mesh), flipped);
    }

    #[test]
    fn show_all_after_several_hides() {
        let mut mesh = sphere(20_000);
        let mut history = History::default();
        let top = triangles_where(&mesh, |p| p.y > 0.3);
        let right = triangles_where(&mesh, |p| p.x > 0.3);
        let front = triangles_where(&mesh, |p| p.z > 0.3);

        let mut states = vec![flags_of(&mesh)];
        for patch in [&top, &right, &front] {
            history.push_visibility(mesh.hide(patch).unwrap());
            states.push(flags_of(&mesh));
        }
        // Hiding what is already hidden is not a step, and neither is
        // showing what already shows.
        assert!(mesh.hide(&top).is_none());
        let untouched = triangles_where(&mesh, |p| p.x < -0.35 && p.y < 0.2 && p.z < 0.2);
        assert!(untouched.len() > 100);
        assert!(mesh.show(&untouched).is_none());
        // A triangle named twice, or one that does not exist, is harmless.
        assert!(mesh.hide(&[top[0], top[0], u32::MAX]).is_none());

        let union: HashSet<u32> = top.iter().chain(&right).chain(&front).copied().collect();
        assert!(union.len() < top.len() + right.len() + front.len(), "the patches overlap");
        assert_eq!(mesh.hidden_triangle_count(), union.len());

        let shown = mesh.show_all().unwrap();
        match &shown {
            VisibilityDelta::Set { triangles, hidden: false } => {
                assert!(triangles.windows(2).all(|w| w[0] < w[1]));
                assert_eq!(triangles.iter().copied().collect::<HashSet<u32>>(), union);
            }
            _ => panic!("show all records the faces it brought back"),
        }
        history.push_visibility(shown);
        assert!(!mesh.any_hidden());
        assert_eq!(mesh.visible_triangle_count(), mesh.triangle_count());
        assert!(mesh.show_all().is_none(), "nothing left to show");

        // Undo walks back through all four steps, one state at a time.
        for state in states.iter().rev() {
            assert_eq!(history.undo(&mut mesh), Some(false));
            assert_eq!(&flags_of(&mesh), state);
        }
        assert_eq!(history.undo(&mut mesh), None);
        // Showing part of a hidden area is a step of its own.
        mesh.hide(&top).unwrap();
        let half: Vec<u32> = top.iter().copied().step_by(2).collect();
        assert!(matches!(mesh.show(&half), Some(VisibilityDelta::Set { hidden: false, .. })));
        assert_eq!(mesh.hidden_triangle_count(), top.len() - half.len());
    }

    #[test]
    fn the_uploaded_index_count_equals_the_visible_triangle_count() {
        let mut mesh = sphere(20_000);
        // Nothing hidden: the mesh's own triangles, without a copy.
        assert!(matches!(mesh.visible_indices(), Cow::Borrowed(_)));
        assert_eq!(mesh.visible_indices().len(), mesh.triangle_count() * 3);

        let patch = triangles_where(&mesh, |p| p.z < 0.1);
        mesh.hide(&patch).unwrap();
        // `visible_indices` is exactly what the renderer puts in its index
        // buffer (`Renderer::upload_mesh` and `update_mesh`).
        let uploaded = mesh.visible_indices();
        assert_eq!(uploaded.len(), mesh.visible_triangle_count() * 3);
        assert_eq!(mesh.visible_triangle_count(), mesh.triangle_count() - patch.len());
        // They are the showing triangles, whole and in the mesh's own order.
        let want: Vec<u32> = mesh
            .indices
            .chunks_exact(3)
            .enumerate()
            .filter(|(t, _)| !mesh.is_triangle_hidden(*t as u32))
            .flat_map(|(_, c)| c.iter().copied())
            .collect();
        assert_eq!(&uploaded[..], &want[..]);
        assert!(uploaded.chunks_exact(3).all(|c| c.iter().any(|&v| mesh.positions[v as usize].z >= 0.1)));
        // Every vertex is still there for them to point at.
        assert!(uploaded.iter().all(|&v| (v as usize) < mesh.vertex_count()));
        drop(uploaded);

        mesh.invert_visible().unwrap();
        assert_eq!(mesh.visible_indices().len(), patch.len() * 3);
        mesh.show_all().unwrap();
        assert!(matches!(mesh.visible_indices(), Cow::Borrowed(_)));
    }

    #[test]
    fn hiding_leaves_the_shape_and_its_normals_alone() {
        let mut mesh = sphere(20_000);
        let (positions, normals, bounds) = (mesh.positions.clone(), mesh.normals.clone(), mesh.bounds());
        mesh.hide(&triangles_where(&mesh, |p| p.x < 0.0)).unwrap();
        mesh.recompute_all_normals();
        // The cut edge is lit as if its hidden neighbours were still there.
        assert_eq!(mesh.normals, normals);
        assert_eq!(mesh.positions, positions);
        assert_eq!(mesh.bounds().min, bounds.min);
        assert_eq!(mesh.bounds().max, bounds.max);
    }

    #[test]
    fn saved_flags_follow_the_triangles_through_a_rebuild() {
        let mut mesh = sphere(20_000);
        mesh.hide(&triangles_where(&mesh, |p| p.y < -0.1 || p.x > 0.35)).unwrap();
        // A triangle as the three places its corners are at, whatever its
        // number and whatever its vertices are numbered.
        let corners = |m: &Mesh, t: usize| {
            let mut c: Vec<[u32; 3]> = m.indices[t * 3..t * 3 + 3]
                .iter()
                .map(|&v| m.positions[v as usize].to_array().map(f32::to_bits))
                .collect();
            c.sort_unstable();
            c
        };
        let hidden_faces = |m: &Mesh| -> HashSet<Vec<[u32; 3]>> {
            (0..m.triangle_count()).filter(|&t| m.is_triangle_hidden(t as u32)).map(|t| corners(m, t)).collect()
        };
        let before = hidden_faces(&mesh);

        // As a file holds it: the triangles in the saved order (here turned
        // back to front, with a broken one in the way), one flag for each.
        let mut indices: Vec<u32> = mesh.indices.chunks_exact(3).rev().flatten().copied().collect();
        let mut flags: Vec<bool> = flags_of(&mesh).into_iter().rev().collect();
        indices.splice(0..0, [7, 7, 8]);
        flags.insert(0, true);
        let loaded = Mesh::new_with_hidden(mesh.positions.clone(), indices.clone(), Some(mesh.mask.clone()), &flags);
        assert_eq!(loaded.triangle_count(), mesh.triangle_count());
        assert_eq!(loaded.hidden_triangle_count(), mesh.hidden_triangle_count());
        assert_eq!(hidden_faces(&loaded), before);

        // The same with the vertex map, for a caller that has quads to carry.
        let (mapped, went) =
            Mesh::new_mapped_with_hidden(mesh.positions.clone(), indices.clone(), None, &flags);
        assert_eq!(hidden_faces(&mapped), before);
        assert!(went.iter().enumerate().all(|(v, &to)| mapped.positions[to as usize] == mesh.positions[v]));

        // Flags for some other mesh are dropped rather than guessed at.
        let wrong = Mesh::new_with_hidden(mesh.positions.clone(), indices, None, &flags[1..]);
        assert!(!wrong.any_hidden());

        // Within one mesh the flags can be read and put straight back.
        let kept = flags_of(&mesh);
        mesh.show_all().unwrap();
        assert!(mesh.restore_hidden_triangles(&kept));
        assert_eq!(hidden_faces(&mesh), before);
        assert!(!mesh.restore_hidden_triangles(&kept[1..]), "wrong length is refused");
        assert_eq!(flags_of(&mesh), kept);
        assert!(mesh.restore_hidden_triangles(&[]));
        assert!(!mesh.any_hidden());
    }

    /// Every edge of a showing triangle, low vertex first.
    fn showing_edges(mesh: &Mesh) -> HashSet<(u32, u32)> {
        mesh.visible_indices()
            .chunks_exact(3)
            .flat_map(|t| [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])])
            .map(|(a, b)| (a.min(b), a.max(b)))
            .collect()
    }

    #[test]
    fn the_wireframe_draws_only_the_faces_that_show() {
        let mut mesh = sphere(20_000);
        assert!(mesh.visible_wire_lines().is_none(), "nothing hidden: the overlay lists the edges itself");
        mesh.hide(&triangles_where(&mesh, |p| p.x < 0.05)).unwrap();
        let lines = mesh.visible_wire_lines().unwrap();
        let drawn: Vec<(u32, u32)> = lines.chunks_exact(2).map(|l| (l[0].min(l[1]), l[0].max(l[1]))).collect();
        let unique: HashSet<(u32, u32)> = drawn.iter().copied().collect();
        assert_eq!(unique.len(), drawn.len(), "no edge is drawn twice");
        // The cut edge included, and nothing of the hidden side.
        assert_eq!(unique, showing_edges(&mesh));
        assert!(lines.iter().all(|&v| !mesh.is_vertex_hidden(v)));
    }

    #[test]
    fn the_wireframe_of_a_quad_mesh_keeps_its_quads_while_part_is_hidden() {
        // A flat sheet of quads, each cut in two for the engine.
        let n = 12usize;
        let at = |i: usize, j: usize| (j * (n + 1) + i) as u32;
        let positions: Vec<Vec3> = (0..=n)
            .flat_map(|j| (0..=n).map(move |i| Vec3::new(i as f32 - 6.0, j as f32 - 6.0, 0.0)))
            .collect();
        let (mut triangles, mut quads) = (Vec::new(), Vec::new());
        for j in 0..n {
            for i in 0..n {
                let q = [at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1)];
                triangles.extend_from_slice(&[q[0], q[1], q[2], q[0], q[2], q[3]]);
                quads.push(q);
            }
        }
        let (mut mesh, to_new) = Mesh::new_mapped(positions, triangles, None);
        let corners: Vec<u32> = quads.iter().flatten().map(|&v| to_new[v as usize]).collect();
        mesh.polys =
            Some(Polys { start: (0..=quads.len() as u32).map(|f| f * 4).collect(), indices: corners.clone() });

        mesh.hide(&triangles_where(&mesh, |p| p.x <= 0.0)).unwrap();
        let lines = mesh.visible_wire_lines().unwrap();
        let drawn: HashSet<(u32, u32)> = lines.chunks_exact(2).map(|l| (l[0].min(l[1]), l[0].max(l[1]))).collect();
        assert_eq!(drawn.len() * 2, lines.len(), "no edge is drawn twice");
        // The sides of the quads that show: those with a corner right of the
        // cut.
        let mut want = HashSet::new();
        for q in corners.chunks_exact(4) {
            if q.iter().any(|&v| mesh.positions[v as usize].x > 0.0) {
                for k in 0..4 {
                    want.insert((q[k].min(q[(k + 1) % 4]), q[k].max(q[(k + 1) % 4])));
                }
            }
        }
        assert_eq!(drawn, want);
        // In particular no diagonal, and nothing left of the cut.
        assert!(drawn.iter().all(|&(a, b)| {
            let (p, q) = (mesh.positions[a as usize], mesh.positions[b as usize]);
            p.distance(q) < 1.01 && p.x >= 0.0 && q.x >= 0.0
        }));
    }
}
