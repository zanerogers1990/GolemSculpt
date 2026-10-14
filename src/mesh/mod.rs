//! The sculptable mesh: an indexed triangle mesh with adjacency, a spatial
//! index, a lazy stroke-start snapshot (which doubles as the undo record) and
//! dirty tracking so only the changed parts are re-lit and re-uploaded.

pub mod bvh;
pub mod dyntopo;
pub mod groups;
pub mod layers;
pub mod levels;
mod lists;
// Polypaint's data core: the painting brush, the renderer and the exporters that call the
// rest of it are being built on top of it (PHASE5-DESIGN.md, B.10 chunks 2-5).
#[allow(dead_code, unused_imports)]
pub mod paint;
pub mod primitives;
pub mod visibility;

#[cfg(test)]
mod guard_tests;

pub use visibility::VisibilityDelta;

use std::ops::Range;

use glam::{DVec3, Vec3};
use rayon::prelude::*;

use crate::util::{BitSet, SharedMut};
use bvh::{Aabb, Bvh, Hit, Leaf};
use lists::Lists;

/// Vertex ranges closer than this are uploaded as one range; a few wasted
/// kilobytes cost far less than an extra buffer write.
const UPLOAD_MERGE_GAP: u32 = 512;

/// Below this many vertices a stroke edit runs on the calling thread; waking
/// the thread pool costs more than the work.
const PAR_MIN_VERTS: usize = 4096;

/// Source of `Mesh::uid`.
static NEXT_UID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub struct Mesh {
    /// Unique per built mesh. The renderer compares it to tell a mesh that
    /// was edited in place from one that was swapped for another.
    pub uid: u64,
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    /// 0 = free, 1 = fully protected from brushes.
    pub mask: Vec<f32>,
    /// How far each vertex sits in a recess (above 0.5) or on a ridge (below),
    /// against the mesh's own average curvature. Empty until a finish that
    /// reads it asks for it with [`Self::track_curvature`].
    pub curv: Vec<f32>,
    pub indices: Vec<u32>,
    /// The quads a quad remesh left behind; `indices` are these cut in two.
    pub polys: Option<Polys>,

    curv_raw: Vec<f32>,
    curv_mean: f32,
    curv_scale: f32,
    curv_on: bool,

    /// The triangles on each vertex, and its edge-connected neighbours.
    vt: Lists,
    vv: Lists,

    pub bvh: Bvh,

    /// Faces switched off: not drawn, hit or sculpted (see visibility.rs).
    hidden: visibility::Hidden,

    // Stroke-start snapshot, filled per vertex the first time a stroke writes it.
    orig_pos: Vec<Vec3>,
    orig_nrm: Vec<Vec3>,
    orig_mask: Vec<f32>,
    stamp: Vec<u32>,
    stroke_id: u32,
    in_stroke: bool,
    touched: Vec<u32>,
    stroke_leaves: BitSet,

    pending_normals: BitSet,
    dirty_pos: BitSet,
    dirty_nrm: BitSet,
    dirty_mask: BitSet,
    dirty_curv: BitSet,

    /// What dynamic topology keeps for this mesh (see dyntopo.rs).
    topo: dyntopo::State,

    /// The mesh's subdivision levels, when it has more than one (see
    /// levels.rs, which also says why they are kept here). A mesh built by
    /// anything else starts without them.
    pub levels: Option<Box<levels::MeshLevels>>,

    /// The mesh's sculpt layers, when it has any (see layers.rs). `positions`
    /// is the shape with every layer in it.
    pub layers: Option<Box<layers::Layers>>,
    /// How the stroke now open, or the next one, carries the layers when it
    /// moves the model underneath them (see `layers::Ride`).
    pub layer_ride: layers::Ride,

    /// The mesh's face groups: which group each triangle is in (see groups.rs).
    groups: groups::Groups,

    /// The mesh's paint layers, when it has any (see paint.rs).
    pub paint: Option<Box<paint::Paint>>,
}

/// Polygon faces over a mesh's own vertices: face `f`'s corners are
/// `indices[start[f]..start[f + 1]]`. Brushes only move vertices, so the faces
/// follow every stroke for free; anything that rebuilds the triangles builds a
/// new mesh, which starts without them, and dynamic topology retires them the
/// first time it splits one.
#[derive(Clone)]
pub struct Polys {
    pub start: Vec<u32>,
    pub indices: Vec<u32>,
}

/// One vertex as a brush sees it during a stroke.
#[derive(Clone, Copy)]
pub struct StrokeVertex {
    pub pos: Vec3,
    pub normal: Vec3,
    pub mask: f32,
    /// Where the vertex was when the stroke began.
    pub start: Vec3,
}

/// The arrays a stroke writes, opened so that several threads can each work
/// on their own vertices at once.
struct StrokeArrays<'a> {
    positions: SharedMut<'a, Vec3>,
    mask: SharedMut<'a, f32>,
    stamp: SharedMut<'a, u32>,
    orig_pos: SharedMut<'a, Vec3>,
    orig_nrm: SharedMut<'a, Vec3>,
    orig_mask: SharedMut<'a, f32>,
    normals: &'a [Vec3],
    /// One flag per vertex that brushes must leave alone because every face
    /// on it is hidden; empty while nothing is hidden.
    hidden: &'a [bool],
    stroke_id: u32,
    in_stroke: bool,
}

impl StrokeArrays<'_> {
    /// Offers vertex `v` to `f` and writes back the position and mask it
    /// returns, recording the vertex in the stroke snapshot first. Returns
    /// whether this was the stroke's first write to the vertex.
    ///
    /// # Safety
    /// No other thread may touch vertex `v` while this call runs.
    #[inline]
    unsafe fn edit(&self, v: u32, f: impl FnOnce(&StrokeVertex) -> Option<(Vec3, f32)>) -> bool {
        let i = v as usize;
        // Bounds-checked, and every array here has this length.
        let normal = self.normals[i];
        // SAFETY: `i` is in range, and the caller keeps other threads off it.
        unsafe {
            let pos = self.positions.read(i);
            let mask = self.mask.read(i);
            let fresh = self.stamp.read(i) != self.stroke_id;
            let start = if fresh || !self.in_stroke { pos } else { self.orig_pos.read(i) };
            let Some((new_pos, new_mask)) = f(&StrokeVertex { pos, normal, mask, start }) else {
                return false;
            };
            let moved = new_pos.is_finite() && new_pos != pos;
            let painted = new_mask != mask;
            if !moved && !painted {
                return false;
            }
            if fresh {
                self.stamp.write(i, self.stroke_id);
                self.orig_pos.write(i, pos);
                self.orig_nrm.write(i, normal);
                self.orig_mask.write(i, mask);
            }
            if moved {
                self.positions.write(i, new_pos);
            }
            if painted {
                self.mask.write(i, new_mask);
            }
            fresh
        }
    }
}

/// What a finished stroke changed; enough to undo and redo it.
pub enum StrokeDelta {
    /// Vertices moved. `topology` is there when dynamic topology also added
    /// triangles and vertices during the stroke: `verts` then lists only
    /// vertices from before it, and the rest is in there (see dyntopo.rs).
    Positions {
        verts: Vec<u32>,
        before: Vec<Vec3>,
        after: Vec<Vec3>,
        topology: Option<Box<dyntopo::TopologyDelta>>,
    },
    Mask { verts: Vec<u32>, before: Vec<f32>, after: Vec<f32> },
    /// A `Positions` stroke on a mesh with sculpt layers, with what it did
    /// to them: what a layer recorded of it, or how the layers' offsets were
    /// carried when it moved the model underneath (see layers.rs). A stroke
    /// that changed nothing of the layers is a plain `Positions`.
    Layered { stroke: Box<StrokeDelta>, layers: Box<layers::StrokeLayers> },
    /// A paint stroke: the texels it changed on one paint layer (paint.rs).
    Paint(Box<paint::PaintDelta>),
    /// A stroke that painted and also moved or masked the surface (paint
    /// with a sculpting brush along, Zadd): both, undone together.
    Painted { paint: Box<paint::PaintDelta>, shape: Box<StrokeDelta> },
}

/// What kind of stroke is ending, for [`Mesh::end_stroke_kind`].
#[allow(dead_code)] // for the painting brush
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StrokeKind {
    /// It moved the surface.
    Shape,
    /// It painted the sculpt mask.
    Mask,
    /// It painted on a paint layer (and may have moved the surface too).
    Paint,
}

impl StrokeDelta {
    pub fn bytes(&self) -> usize {
        match self {
            StrokeDelta::Positions { verts, topology, .. } => {
                verts.len() * 28 + topology.as_ref().map_or(0, |t| t.bytes())
            }
            StrokeDelta::Mask { verts, .. } => verts.len() * 12,
            StrokeDelta::Layered { stroke, layers } => stroke.bytes() + layers.bytes(),
            StrokeDelta::Paint(paint) => paint.bytes(),
            StrokeDelta::Painted { paint, shape } => paint.bytes() + shape.bytes(),
        }
    }
}

#[derive(Default)]
pub struct DirtyRanges {
    pub positions: Vec<Range<u32>>,
    pub normals: Vec<Range<u32>>,
    pub mask: Vec<Range<u32>>,
    pub curv: Vec<Range<u32>>,
    /// Triangles (not corners) whose corners changed or that are new, when
    /// `Mesh::topology_version` has moved since the last call.
    pub triangles: Vec<Range<u32>>,
    /// Vertices whose paint composite changed (`Mesh::paint_stream`).
    #[allow(dead_code)] // for the renderer's paint stream
    pub paint: Vec<Range<u32>>,
}

/// Whether an input triangle is kept when a mesh is built: three different
/// corners, all of them real vertices.
#[inline]
fn sound(t: &[u32], vertex_count: u32) -> bool {
    t[0] != t[1] && t[1] != t[2] && t[0] != t[2] && t.iter().all(|&i| i < vertex_count)
}

impl Mesh {
    /// Builds a sculptable mesh from raw triangles. Vertices are reordered for
    /// spatial coherence, so indices into the input are not preserved.
    pub fn new(positions: Vec<Vec3>, indices: Vec<u32>, mask: Option<Vec<f32>>) -> Mesh {
        Self::new_mapped(positions, indices, mask).0
    }

    /// [`Self::new`], also returning where each input vertex went (u32::MAX
    /// for one no triangle uses).
    pub fn new_mapped(positions: Vec<Vec3>, indices: Vec<u32>, mask: Option<Vec<f32>>) -> (Mesh, Vec<u32>) {
        let (mesh, vert_remap, _) = Self::new_ordered(positions, indices, mask);
        (mesh, vert_remap)
    }

    /// [`Self::new_mapped`], also returning where each triangle of the mesh
    /// came from: its place among the input's sound triangles (see [`sound`]).
    fn new_ordered(positions: Vec<Vec3>, indices: Vec<u32>, mask: Option<Vec<f32>>) -> (Mesh, Vec<u32>, Vec<u32>) {
        let n_in = positions.len() as u32;
        let clean: Vec<u32> =
            indices.chunks_exact(3).filter(|t| sound(t, n_in)).flatten().copied().collect();

        let built = Bvh::build(&positions, &clean);
        let n = built.positions.len();
        let mut new_mask = vec![0.0f32; n];
        if let Some(mask) = mask {
            for (old, &new) in built.vert_remap.iter().enumerate() {
                if new != u32::MAX {
                    new_mask[new as usize] = mask.get(old).copied().unwrap_or(0.0);
                }
            }
        }

        let leaf_count = built.bvh.leaves.len();
        let mut mesh = Mesh {
            uid: NEXT_UID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            positions: built.positions,
            normals: vec![Vec3::Y; n],
            mask: new_mask,
            curv: Vec::new(),
            indices: built.indices,
            polys: None,
            curv_raw: Vec::new(),
            curv_mean: 0.0,
            curv_scale: 1.0,
            curv_on: false,
            vt: Lists::default(),
            vv: Lists::default(),
            bvh: built.bvh,
            hidden: Default::default(),
            orig_pos: vec![Vec3::ZERO; n],
            orig_nrm: vec![Vec3::ZERO; n],
            orig_mask: vec![0.0; n],
            stamp: vec![0; n],
            stroke_id: 0,
            in_stroke: false,
            touched: Vec::new(),
            stroke_leaves: BitSet::with_len(leaf_count),
            pending_normals: BitSet::with_len(leaf_count),
            dirty_pos: BitSet::with_len(leaf_count),
            dirty_nrm: BitSet::with_len(leaf_count),
            dirty_mask: BitSet::with_len(leaf_count),
            dirty_curv: BitSet::with_len(leaf_count),
            topo: dyntopo::State::default(),
            levels: None,
            layers: None,
            layer_ride: layers::Ride::default(),
            groups: groups::Groups::default(),
            paint: None,
        };
        mesh.build_adjacency();
        mesh.recompute_all_normals();
        (mesh, built.vert_remap, built.tri_order)
    }

    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    pub fn bounds(&self) -> Aabb {
        self.bvh.bounds()
    }

    /// Triangles incident to a vertex.
    #[cfg(test)]
    pub fn vert_tris(&self, v: u32) -> &[u32] {
        self.vt.row(v as usize)
    }

    /// Edge-connected neighbours of a vertex.
    #[inline]
    pub fn neighbours(&self, v: u32) -> &[u32] {
        self.vv.row(v as usize)
    }

    fn build_adjacency(&mut self) {
        let n = self.positions.len();
        let mut off = vec![0u32; n + 1];
        for &v in &self.indices {
            off[v as usize + 1] += 1;
        }
        for i in 0..n {
            off[i + 1] += off[i];
        }
        let mut cursor = off.clone();
        let mut fans = vec![0u32; self.indices.len()];
        for (i, &v) in self.indices.iter().enumerate() {
            let c = &mut cursor[v as usize];
            fans[*c as usize] = (i / 3) as u32;
            *c += 1;
        }

        let indices = &self.indices;
        let (vt_off, vt) = (&off, &fans);
        let ring = |v: usize, buf: &mut Vec<u32>| {
            buf.clear();
            for &t in &vt[vt_off[v] as usize..vt_off[v + 1] as usize] {
                for &o in &indices[t as usize * 3..t as usize * 3 + 3] {
                    if o as usize != v {
                        buf.push(o);
                    }
                }
            }
            buf.sort_unstable();
            buf.dedup();
        };

        let counts: Vec<u32> = (0..n)
            .into_par_iter()
            .map_init(Vec::new, |buf, v| {
                ring(v, buf);
                buf.len() as u32
            })
            .collect();
        let mut vv_off = vec![0u32; n + 1];
        for i in 0..n {
            vv_off[i + 1] = vv_off[i] + counts[i];
        }
        let mut vv = vec![0u32; vv_off[n] as usize];
        {
            let out = SharedMut::new(&mut vv);
            let vv_off = &vv_off;
            (0..n).into_par_iter().for_each_init(Vec::new, |buf, v| {
                ring(v, buf);
                let base = vv_off[v] as usize;
                for (k, &o) in buf.iter().enumerate() {
                    // SAFETY: each vertex writes only its own CSR range.
                    unsafe { out.write(base + k, o) };
                }
            });
        }
        self.vt = Lists::from_offsets(&off, fans);
        self.vv = Lists::from_offsets(&vv_off, vv);
    }

    #[inline]
    fn vertex_normal(positions: &[Vec3], indices: &[u32], tris: &[u32], fallback: Vec3) -> Vec3 {
        // Computed in double so a vertex and its mirror twin get matching
        // normals even though their triangle fans are listed in different
        // orders; float sums here seed symmetry drift over a long session.
        let mut sum = DVec3::ZERO;
        for &t in tris {
            let i = t as usize * 3;
            let p0 = positions[indices[i] as usize].as_dvec3();
            let p1 = positions[indices[i + 1] as usize].as_dvec3();
            let p2 = positions[indices[i + 2] as usize].as_dvec3();
            // Cross product length is twice the area, so this is area weighted.
            sum += (p1 - p0).cross(p2 - p0);
        }
        let len = sum.length();
        if len > 1e-30 { (sum / len).as_vec3() } else { fallback }
    }

    pub fn recompute_all_normals(&mut self) {
        let positions = &self.positions;
        let indices = &self.indices;
        let vt = &self.vt;
        self.normals.par_iter_mut().enumerate().with_min_len(4096).for_each(|(v, n)| {
            *n = Self::vertex_normal(positions, indices, vt.row(v), *n);
        });
        self.pending_normals.clear();
        let leaves = self.bvh.leaves.len();
        self.dirty_pos.set_all(leaves);
        self.dirty_nrm.set_all(leaves);
        self.dirty_mask.set_all(leaves);
        if self.curv_on {
            self.recompute_all_curvature();
        }
    }

    // ---- curvature ---------------------------------------------------------

    /// Turns per-vertex curvature on or off. The finishes that read it (wash
    /// pooled in recesses, wear on ridges) ask for it; nothing else pays.
    pub fn track_curvature(&mut self, on: bool) {
        if on == self.curv_on {
            return;
        }
        if on {
            // Before the flag is set: the flush would otherwise try to patch
            // curvature that does not exist yet.
            self.flush_normals();
        }
        self.curv_on = on;
        if on {
            self.recompute_all_curvature();
        } else {
            self.curv = Vec::new();
            self.curv_raw = Vec::new();
        }
    }

    /// Mean curvature at a vertex relative to the object's size: 0 on a flat
    /// surface, positive in a valley, negative on a ridge. The offsets to the
    /// neighbours are summed before dividing by the summed squared lengths,
    /// so the same shape reads the same at any density and one stray short
    /// edge barely moves it.
    fn raw_curvature(positions: &[Vec3], normals: &[Vec3], ring: &[u32], v: usize, scale: f32) -> f32 {
        let p = positions[v];
        let mut offset = DVec3::ZERO;
        let mut sqr_len = 0.0f64;
        for &q in ring {
            let d = positions[q as usize] - p;
            let l = d.length_squared();
            if l < 1e-18 {
                continue;
            }
            offset += d.as_dvec3();
            sqr_len += l as f64;
        }
        if sqr_len <= 0.0 {
            return 0.0;
        }
        (offset.dot(normals[v].as_dvec3()) / sqr_len * scale as f64) as f32
    }

    /// The stored value: the one-ring average of the raw curvature, less the
    /// mesh's mean (so a plain ball is neutral and only departures from the
    /// overall form count), eased toward its limits rather than clipped so a
    /// shallow recess and a deep groove stay distinguishable.
    fn encode_curvature(raw: &[f32], ring: &[u32], v: usize, mean: f32) -> f32 {
        // A detail whose radius of curvature is 2.5% of the object's
        // half-size lands halfway to saturation.
        const SENSITIVITY: f32 = 1.0 / 40.0;
        let sum: f32 = raw[v] + ring.iter().map(|&q| raw[q as usize]).sum::<f32>();
        let soft = (sum / (ring.len() + 1) as f32 - mean) * SENSITIVITY;
        0.5 + soft / (1.0 + soft.abs()) * 0.5
    }

    fn recompute_all_curvature(&mut self) {
        let n = self.positions.len();
        let size = self.bounds();
        let half = if size.is_empty() { Vec3::ZERO } else { (size.max - size.min) * 0.5 };
        self.curv_scale = half.max_element();
        if self.curv_scale < 1e-6 {
            self.curv_scale = 1.0;
        }
        let (positions, normals, vv, scale) = (&self.positions, &self.normals, &self.vv, self.curv_scale);
        let ring = |v: usize| vv.row(v);
        self.curv_raw.resize(n, 0.0);
        self.curv_raw.par_iter_mut().enumerate().with_min_len(4096).for_each(|(v, raw)| {
            *raw = Self::raw_curvature(positions, normals, ring(v), v, scale);
        });
        // Taken only here, so a stroke never shifts the whole mesh's tone
        // out from under itself.
        let sum: f64 = self.curv_raw.iter().map(|&r| r as f64).sum();
        self.curv_mean = if n > 0 { (sum / n as f64) as f32 } else { 0.0 };
        let (raw, mean) = (&self.curv_raw, self.curv_mean);
        self.curv.resize(n, 0.5);
        self.curv.par_iter_mut().enumerate().with_min_len(4096).for_each(|(v, c)| {
            *c = Self::encode_curvature(raw, ring(v), v, mean);
        });
        self.dirty_curv.set_all(self.bvh.leaves.len());
    }

    /// Brings the curvature of the vertices in `leaves` up to date after they
    /// (or their neighbours) moved.
    fn update_curvature(&mut self, leaves: &[u32]) {
        // The leaves' vertex ranges index both arrays through `SharedMut`;
        // dynamic topology grows them with the mesh, and they must stay as long.
        assert!(
            self.curv_raw.len() == self.positions.len() && self.curv.len() == self.positions.len(),
            "the curvature arrays ({}, {}) are not as long as the mesh ({} vertices)",
            self.curv_raw.len(),
            self.curv.len(),
            self.positions.len()
        );
        let (positions, normals, vv, bvh) = (&self.positions, &self.normals, &self.vv, &self.bvh);
        let (scale, mean) = (self.curv_scale, self.curv_mean);
        let ring = |v: usize| vv.row(v);
        // Two passes: the stored value blurs across neighbours, so every raw
        // value has to exist before any of them is read.
        {
            let out = SharedMut::new(&mut self.curv_raw);
            leaves.par_iter().with_min_len(32).for_each(|&l| {
                let leaf = &bvh.leaves[l as usize];
                for v in leaf.vert_start as usize..leaf.vert_end as usize {
                    // SAFETY: every leaf owns a disjoint vertex range.
                    unsafe { out.write(v, Self::raw_curvature(positions, normals, ring(v), v, scale)) };
                }
            });
        }
        let raw = &self.curv_raw;
        let out = SharedMut::new(&mut self.curv);
        leaves.par_iter().with_min_len(32).for_each(|&l| {
            let leaf = &bvh.leaves[l as usize];
            for v in leaf.vert_start as usize..leaf.vert_end as usize {
                // SAFETY: every leaf owns a disjoint vertex range.
                unsafe { out.write(v, Self::encode_curvature(raw, ring(v), v, mean)) };
            }
        });
        for &l in leaves {
            self.dirty_curv.set(l as usize);
        }
    }

    // ---- strokes -----------------------------------------------------------

    pub fn begin_stroke(&mut self) {
        debug_assert!(!self.in_stroke);
        self.stroke_id = self.stroke_id.wrapping_add(1);
        if self.stroke_id == 0 {
            self.stamp.fill(0);
            self.stroke_id = 1;
        }
        self.touched.clear();
        self.stroke_leaves.clear();
        self.in_stroke = true;
    }

    #[inline]
    pub fn start_pos(&self, v: u32) -> Vec3 {
        let i = v as usize;
        if self.stamp[i] == self.stroke_id && self.in_stroke {
            self.orig_pos[i]
        } else {
            self.positions[i]
        }
    }

    #[inline]
    pub fn start_nrm(&self, v: u32) -> Vec3 {
        let i = v as usize;
        if self.stamp[i] == self.stroke_id && self.in_stroke {
            self.orig_nrm[i]
        } else {
            self.normals[i]
        }
    }

    fn stroke_arrays(&mut self) -> (StrokeArrays<'_>, &[Leaf], &mut Vec<u32>) {
        // `StrokeArrays::edit` checks a vertex against `normals` only.
        let n = self.normals.len();
        assert!(
            [self.positions.len(), self.orig_pos.len(), self.orig_nrm.len()] == [n; 3]
                && [self.mask.len(), self.orig_mask.len(), self.stamp.len()] == [n; 3]
        );
        let arrays = StrokeArrays {
            positions: SharedMut::new(&mut self.positions),
            mask: SharedMut::new(&mut self.mask),
            stamp: SharedMut::new(&mut self.stamp),
            orig_pos: SharedMut::new(&mut self.orig_pos),
            orig_nrm: SharedMut::new(&mut self.orig_nrm),
            orig_mask: SharedMut::new(&mut self.orig_mask),
            normals: &self.normals,
            hidden: self.hidden.kept_off(),
            stroke_id: self.stroke_id,
            in_stroke: self.in_stroke,
        };
        (arrays, &self.bvh.leaves, &mut self.touched)
    }

    /// Runs `f` over every vertex owned by `leaves`, on all threads, and
    /// writes back the position and mask it returns. `f` also gets the
    /// vertex's place in the leaves' combined vertex order.
    ///
    /// A vertex whose faces are all hidden is passed over without being
    /// offered to `f`, which is what keeps every brush off hidden parts. The
    /// loop is built twice so that the test is only in the copy that runs
    /// while something is hidden; the usual copy is the loop as it was.
    fn edit_leaves(
        &mut self,
        leaves: &[u32],
        f: impl Fn(usize, &StrokeVertex) -> Option<(Vec3, f32)> + Sync,
    ) {
        if self.hidden.keeps_off() {
            self.edit_leaves_as::<true>(leaves, f);
        } else {
            self.edit_leaves_as::<false>(leaves, f);
        }
    }

    fn edit_leaves_as<const HIDING: bool>(
        &mut self,
        leaves: &[u32],
        f: impl Fn(usize, &StrokeVertex) -> Option<(Vec3, f32)> + Sync,
    ) {
        // Threads may only share the work if no leaf is listed twice.
        let sorted;
        let leaves = if leaves.windows(2).all(|w| w[0] < w[1]) {
            leaves
        } else {
            let mut copy = leaves.to_vec();
            copy.sort_unstable();
            copy.dedup();
            sorted = copy;
            &sorted[..]
        };
        let (arrays, ranges, touched) = self.stroke_arrays();
        let mut starts = Vec::with_capacity(leaves.len());
        let mut total = 0usize;
        for &l in leaves {
            let leaf = &ranges[l as usize];
            starts.push(total);
            total += (leaf.vert_end - leaf.vert_start) as usize;
        }
        let run = |k: usize, first: &mut Vec<u32>| {
            let leaf = &ranges[leaves[k] as usize];
            for (n, v) in (leaf.vert_start..leaf.vert_end).enumerate() {
                if HIDING && arrays.hidden[v as usize] {
                    continue;
                }
                // SAFETY: the leaves are distinct and each owns its own run
                // of vertices, so no two tasks ever reach the same vertex.
                if unsafe { arrays.edit(v, |vertex| f(starts[k] + n, vertex)) } {
                    first.push(v);
                }
            }
        };
        if total < PAR_MIN_VERTS {
            for k in 0..leaves.len() {
                run(k, touched);
            }
        } else {
            let firsts: Vec<Vec<u32>> = (0..leaves.len())
                .into_par_iter()
                .with_min_len(8)
                .fold(Vec::new, |mut first, k| {
                    run(k, &mut first);
                    first
                })
                .collect();
            for first in firsts {
                touched.extend(first);
            }
        }
    }

    /// Moves the vertices owned by `leaves` during a stroke, on all threads.
    /// `f` is called once per vertex with its place in the leaves' combined
    /// vertex order (leaves ascending, each counted once) and its current
    /// state, and returns its new position, or `None` to leave it alone. It
    /// sees nothing of the other vertices, so the result does not depend on
    /// the order the work is done in. Every vertex that moves is recorded in
    /// the stroke snapshot first; call [`Self::positions_changed`] afterwards.
    pub fn move_vertices(
        &mut self,
        leaves: &[u32],
        f: impl Fn(usize, &StrokeVertex) -> Option<Vec3> + Sync,
    ) {
        self.edit_leaves(leaves, |at, vertex| f(at, vertex).map(|pos| (pos, vertex.mask)));
    }

    /// As [`Self::move_vertices`], for the mask: `f` returns a vertex's new
    /// mask value. Call [`Self::mask_changed`] afterwards.
    pub fn paint_mask(&mut self, leaves: &[u32], f: impl Fn(&StrokeVertex) -> Option<f32> + Sync) {
        self.edit_leaves(leaves, |_, vertex| f(vertex).map(|mask| (vertex.pos, mask)));
    }

    /// As [`Self::move_vertices`], for a list of vertices: `f` gets each
    /// one's place in `verts`. The work is shared between threads when
    /// `verts` is in ascending order with no repeats. Hidden vertices in the
    /// list are passed over, as in [`Self::edit_leaves`].
    pub fn move_listed(
        &mut self,
        verts: &[u32],
        f: impl Fn(usize, &StrokeVertex) -> Option<Vec3> + Sync,
    ) {
        if self.hidden.keeps_off() {
            self.move_listed_as::<true>(verts, f);
        } else {
            self.move_listed_as::<false>(verts, f);
        }
    }

    fn move_listed_as<const HIDING: bool>(
        &mut self,
        verts: &[u32],
        f: impl Fn(usize, &StrokeVertex) -> Option<Vec3> + Sync,
    ) {
        let distinct = verts.len() >= PAR_MIN_VERTS
            && verts.par_windows(2).with_min_len(4096).all(|w| w[0] < w[1]);
        let (arrays, _, touched) = self.stroke_arrays();
        let run = |k: usize, v: u32, first: &mut Vec<u32>| {
            if HIDING && arrays.hidden[v as usize] {
                return;
            }
            // SAFETY: either this is the only thread, or every vertex is
            // listed once and so reached by one task only.
            let fresh =
                unsafe { arrays.edit(v, |vertex| f(k, vertex).map(|pos| (pos, vertex.mask))) };
            if fresh {
                first.push(v);
            }
        };
        if !distinct {
            for (k, &v) in verts.iter().enumerate() {
                run(k, v, touched);
            }
        } else {
            let firsts: Vec<Vec<u32>> = verts
                .par_iter()
                .enumerate()
                .with_min_len(1024)
                .fold(Vec::new, |mut first, (k, &v)| {
                    run(k, v, &mut first);
                    first
                })
                .collect();
            for first in firsts {
                touched.extend(first);
            }
        }
    }

    /// Ends the stroke, tightening the spatial index and returning the undo
    /// record (or `None` if nothing changed).
    pub fn end_stroke(&mut self, mask_stroke: bool) -> Option<StrokeDelta> {
        self.end_stroke_as(mask_stroke, None)
    }

    #[allow(dead_code)] // for the painting brush
    /// [`Self::end_stroke`] by kind. Whatever the kind, paint the stroke
    /// wrote is in the record too (see paint.rs).
    pub fn end_stroke_kind(&mut self, kind: StrokeKind) -> Option<StrokeDelta> {
        self.end_stroke_as(kind == StrokeKind::Mask, None)
    }

    /// [`Self::end_stroke`] for a stroke that moved the surface by a matrix
    /// (the gizmo, reshaping what is not masked): the matrix's turn and
    /// stretch, in the mesh's own space, so that sculpt layers can be carried
    /// by exactly that (see layers.rs).
    pub fn end_stroke_moved(&mut self, linear: glam::Mat3) -> Option<StrokeDelta> {
        self.end_stroke_as(false, Some(linear))
    }

    fn end_stroke_as(&mut self, mask_stroke: bool, linear: Option<glam::Mat3>) -> Option<StrokeDelta> {
        debug_assert!(self.in_stroke);
        self.in_stroke = false;
        // A stroke kept to its group (Stay in Group) is over: see visibility.rs.
        self.hidden.lift_fence();

        // Bounds only grew during the stroke; make them exact again.
        let mut leaves = Vec::new();
        self.stroke_leaves.for_each(|l| leaves.push(l as u32));
        self.stroke_leaves.clear();
        self.bvh.refit_leaves(&leaves, &self.positions, &self.indices, false);

        // What dynamic topology added during the stroke, if it ran.
        let topology = self.end_stroke_topology();
        // What the stroke painted, if it painted (paint.rs).
        let painted = self.paint_stroke_record().map(Box::new);
        if self.touched.is_empty() && topology.is_none() {
            return painted.map(StrokeDelta::Paint);
        }
        let shape = self.end_shape_record(mask_stroke, linear, topology);
        Some(match painted {
            Some(paint) => StrokeDelta::Painted { paint, shape: Box::new(shape) },
            None => shape,
        })
    }

    /// The record of what a stroke moved or masked: the rest of [`Self::end_stroke_as`].
    fn end_shape_record(
        &mut self,
        mask_stroke: bool,
        linear: Option<glam::Mat3>,
        topology: Option<Box<dyntopo::TopologyDelta>>,
    ) -> StrokeDelta {
        let mut verts = std::mem::take(&mut self.touched);
        if let Some(topology) = &topology {
            // The vertices the stroke made go when it is undone; only those
            // from before it have somewhere to go back to.
            verts.retain(|&v| v < topology.vertices_before());
        }
        if mask_stroke {
            debug_assert!(topology.is_none(), "mask painting never refines");
            StrokeDelta::Mask {
                before: verts.iter().map(|&v| self.orig_mask[v as usize]).collect(),
                after: verts.iter().map(|&v| self.mask[v as usize]).collect(),
                verts,
            }
        } else {
            let delta = StrokeDelta::Positions {
                before: verts.iter().map(|&v| self.orig_pos[v as usize]).collect(),
                after: verts.iter().map(|&v| self.positions[v as usize]).collect(),
                verts,
                topology,
            };
            // What the stroke means for the sculpt layers, if there are any.
            self.settle_layers(delta, linear)
        }
    }

    /// Tells the mesh that vertices inside these leaves moved. Leaves must
    /// cover every triangle that references a moved vertex (the leaves a brush
    /// sphere intersects always do).
    pub fn positions_changed(&mut self, leaves: &[u32]) {
        self.bvh.refit_leaves(leaves, &self.positions, &self.indices, self.in_stroke);
        for &l in leaves {
            let l = l as usize;
            self.dirty_pos.set(l);
            self.pending_normals.set(l);
            if self.in_stroke {
                self.stroke_leaves.set(l);
            }
        }
        // A moved vertex also changes the normals of neighbours owned by
        // adjacent leaves.
        for &l in leaves {
            for &o in self.bvh.leaf_neighbours(l) {
                self.pending_normals.set(o as usize);
            }
        }
    }

    pub fn mask_changed(&mut self, leaves: &[u32]) {
        for &l in leaves {
            self.dirty_mask.set(l as usize);
        }
    }

    /// Leaves owning `verts`, plus their neighbours: a safe argument for
    /// [`Self::positions_changed`] when the change is given as a vertex list.
    pub fn leaves_of(&self, verts: &[u32]) -> Vec<u32> {
        let mut set = BitSet::with_len(self.bvh.leaves.len());
        // Kept apart from `set`: a leaf already in there as some other
        // leaf's neighbour still has neighbours of its own to bring.
        let mut owners = BitSet::with_len(self.bvh.leaves.len());
        for &v in verts {
            let l = self.bvh.vert_leaf[v as usize];
            if !owners.get(l as usize) {
                owners.set(l as usize);
                set.set(l as usize);
                for &o in self.bvh.leaf_neighbours(l) {
                    set.set(o as usize);
                }
            }
        }
        let mut out = Vec::new();
        set.for_each(|l| out.push(l as u32));
        out
    }

    /// Recomputes normals for everything that moved since the last call.
    pub fn flush_normals(&mut self) {
        if !self.pending_normals.any() {
            return;
        }
        let mut leaves = Vec::new();
        self.pending_normals.for_each(|l| leaves.push(l as u32));
        self.pending_normals.clear();
        {
            let positions = &self.positions;
            let indices = &self.indices;
            let vt = &self.vt;
            let bvh = &self.bvh;
            let out = SharedMut::new(&mut self.normals);
            leaves.par_iter().with_min_len(32).for_each(|&l| {
                let leaf = &bvh.leaves[l as usize];
                for v in leaf.vert_start as usize..leaf.vert_end as usize {
                    let tris = vt.row(v);
                    // SAFETY: every leaf owns a disjoint vertex range, so each
                    // normal is read and written by exactly one task.
                    unsafe {
                        let old = out.read(v);
                        out.write(v, Self::vertex_normal(positions, indices, tris, old));
                    }
                }
            });
        }
        for &l in &leaves {
            self.dirty_nrm.set(l as usize);
        }
        if self.curv_on {
            self.update_curvature(&leaves);
        }
    }

    fn ranges(&self, set: &BitSet) -> Vec<Range<u32>> {
        let mut out: Vec<Range<u32>> = Vec::new();
        set.for_each(|l| {
            let leaf = &self.bvh.leaves[l];
            if leaf.vert_start == leaf.vert_end {
                return;
            }
            match out.last_mut() {
                Some(last) if leaf.vert_start <= last.end + UPLOAD_MERGE_GAP => {
                    last.end = last.end.max(leaf.vert_end);
                }
                _ => out.push(leaf.vert_start..leaf.vert_end),
            }
        });
        out
    }

    /// Whether vertices have moved since the last [`Self::take_dirty`]: a
    /// change the renderer has not been sent yet.
    pub fn moved_since_upload(&self) -> bool {
        self.dirty_pos.any()
    }

    /// Vertex ranges whose GPU copies are stale. Clears the dirty state.
    pub fn take_dirty(&mut self) -> DirtyRanges {
        self.flush_normals();
        // Before the mask's dirty leaves are cleared: layers masked by it read them.
        let paint = if self.paint.is_some() { self.paint_take_dirty(&self.dirty_mask.clone()) } else { Vec::new() };
        let out = DirtyRanges {
            positions: self.ranges(&self.dirty_pos),
            normals: self.ranges(&self.dirty_nrm),
            mask: self.ranges(&self.dirty_mask),
            curv: self.ranges(&self.dirty_curv),
            triangles: self.topo.take_unsent((self.indices.len() / 3) as u32),
            paint,
        };
        self.dirty_curv.clear();
        self.dirty_pos.clear();
        self.dirty_nrm.clear();
        self.dirty_mask.clear();
        out
    }

    // ---- queries -----------------------------------------------------------

    /// Nearest point of the surface along a ray. Hidden faces let the ray
    /// through to whatever is behind them.
    pub fn raycast(&self, origin: Vec3, dir: Vec3) -> Option<Hit> {
        let pos = |v: u32| self.positions[v as usize];
        match self.hidden_triangles() {
            None => self.bvh.raycast(&self.indices, origin, dir, false, pos),
            Some(hidden) => {
                self.bvh.raycast_skipping(&self.indices, origin, dir, false, pos, |t| hidden[t as usize])
            }
        }
    }

    /// Raycast against the surface as it was when the current stroke began.
    pub fn raycast_start(&self, origin: Vec3, dir: Vec3) -> Option<Hit> {
        let pos = |v: u32| self.start_pos(v);
        match self.hidden_triangles() {
            None => self.bvh.raycast(&self.indices, origin, dir, false, pos),
            Some(hidden) => {
                self.bvh.raycast_skipping(&self.indices, origin, dir, false, pos, |t| hidden[t as usize])
            }
        }
    }

    /// Smooth normal at a hit, interpolated from the vertex normals.
    pub fn hit_normal(&self, hit: &Hit, from_start: bool) -> Vec3 {
        let i = hit.tri as usize * 3;
        let mut n = Vec3::ZERO;
        for c in 0..3 {
            let v = self.indices[i + c];
            let vn = if from_start { self.start_nrm(v) } else { self.normals[v as usize] };
            n += vn * hit.bary[c];
        }
        let n = n.normalize_or_zero();
        if n == Vec3::ZERO { hit.normal } else { n }
    }

    // ---- undo --------------------------------------------------------------

    pub fn apply_delta(&mut self, delta: &StrokeDelta, undo: bool) {
        debug_assert!(!self.in_stroke);
        match delta {
            StrokeDelta::Positions { verts, before, after, topology } => {
                let src = if undo { before } else { after };
                if let Some(topology) = topology {
                    // The stroke also grew the mesh: counts and triangles go
                    // back or forward with the positions (see dyntopo.rs).
                    return self.apply_topology(topology, verts, src, undo);
                }
                for (&v, &p) in verts.iter().zip(src) {
                    self.positions[v as usize] = p;
                }
                let leaves = self.leaves_of(verts);
                self.positions_changed(&leaves);
            }
            StrokeDelta::Mask { verts, before, after } => {
                let src = if undo { before } else { after };
                for (&v, &m) in verts.iter().zip(src) {
                    self.mask[v as usize] = m;
                }
                let leaves = self.leaves_of(verts);
                self.mask_changed(&leaves);
            }
            StrokeDelta::Layered { stroke, layers } => {
                self.apply_delta(stroke, undo);
                self.apply_stroke_layers(layers, undo);
            }
            StrokeDelta::Paint(paint) => self.apply_paint_delta(paint, undo),
            StrokeDelta::Painted { paint, shape } => {
                self.apply_delta(shape, undo);
                self.apply_paint_delta(paint, undo);
            }
        }
    }

    /// Rewrites the whole mask as `f(vertex, current value)` and returns the
    /// undo record, or `None` if nothing changed. `f` may read neighbours'
    /// current values: all results are computed before any is written.
    pub fn remap_mask(&mut self, f: impl Fn(&Mesh, u32, f32) -> f32 + Sync) -> Option<StrokeDelta> {
        let this = &*self;
        let new: Vec<f32> = (0..this.mask.len() as u32)
            .into_par_iter()
            .with_min_len(4096)
            .map(|v| f(this, v, this.mask[v as usize]).clamp(0.0, 1.0))
            .collect();
        let verts: Vec<u32> =
            (0..new.len() as u32).filter(|&v| new[v as usize] != self.mask[v as usize]).collect();
        if verts.is_empty() {
            return None;
        }
        let before = verts.iter().map(|&v| self.mask[v as usize]).collect();
        let after = verts.iter().map(|&v| new[v as usize]).collect();
        self.mask = new;
        self.dirty_mask.set_all(self.bvh.leaves.len());
        Some(StrokeDelta::Mask { verts, before, after })
    }

    /// Mean mask value of a vertex's neighbours.
    pub fn mask_ring_mean(&self, v: u32) -> f32 {
        let ring = self.neighbours(v);
        if ring.is_empty() {
            return self.mask[v as usize];
        }
        ring.iter().map(|&o| self.mask[o as usize]).sum::<f32>() / ring.len() as f32
    }

    pub fn mask_ring_extreme(&self, v: u32, max: bool) -> f32 {
        let own = self.mask[v as usize];
        self.neighbours(v).iter().map(|&o| self.mask[o as usize]).fold(own, |a, b| {
            if max { a.max(b) } else { a.min(b) }
        })
    }

    /// What the mesh holds in memory, near enough: every list that grows with
    /// the mesh. Undo is charged this for each mesh it keeps (history.rs).
    pub fn memory_bytes(&self) -> usize {
        // Positions, normals and the stroke-start copies of both; then the
        // mask, its stroke-start copy and the stroke stamp.
        self.positions.len() * (12 * 4 + 4 * 3)
            + self.indices.len() * 4
            + (self.curv.len() + self.curv_raw.len() + self.touched.len()) * 4
            + self.polys.as_ref().map_or(0, |p| (p.start.len() + p.indices.len()) * 4)
            + self.vt.bytes()
            + self.vv.bytes()
            + self.bvh.bytes()
            + self.hidden.bytes()
            + self.topo.bytes()
            + self.levels.as_ref().map_or(0, |levels| levels.bytes())
            + self.layers.as_ref().map_or(0, |layers| layers.bytes())
            + self.groups.bytes()
            + self.paint.as_ref().map_or(0, |paint| paint.bytes())
    }
}
