//! The paint layers folded into what is drawn and exported (B.4): one
//! interleaved record of 16 bytes for every vertex.
//!
//! ```text
//! bytes 0-3   colour: sRGB red, green, blue, coverage
//! bytes 4-7   rough, metal, rough weight, metal weight
//! bytes 8-15  the weight of each local material (table entry 0 to 7);
//!             the Look's own finish has what is left (1 - the sum)
//! ```
//!
//! Every visible layer is laid on the one under it, bottom to top, each
//! scaled by its opacity and by its mask source at the vertex:
//!
//! - colour by the layer's blend mode (`blend::over`, the W3C formulas);
//! - material by Normal (`D' = D (1 - w) + w e_s`: the layer's material takes
//!   its weight's share from all the others) or Add (`D'[s] = min(1, D[s] + w)`,
//!   and where that makes more than all, the others give way in proportion);
//! - roughness and metallic each laid over by its own weight.
//!
//! Only the leaves that changed are worked out again: a stroke marks the
//! leaves of the vertices it wrote, and a change of a layer's settings marks
//! everything. Layers masked by the sculpt mask or by a face group are worked
//! out again where the mask, or anywhere the groups, changed. The record of
//! a vertex is a function of that vertex's texels alone, so a leaf worked out
//! again gives exactly what working out everything would (`tests.rs`).

use std::sync::Arc;

use rayon::prelude::*;

use super::blend::{Rgba, over};
use super::{ChannelSet, MaskSource, MaterialBlend, MOST_MATERIALS, PER_TASK, PaintLayer, PaintStack};
use crate::mesh::Mesh;
use crate::util::{BitSet, SharedMut};

/// One vertex's composite (see the module notes).
pub type Stream = [u8; 16];

/// What is worked out from the paint and kept between frames.
#[derive(Default)]
pub(crate) struct Live {
    /// The composite, one record per vertex; empty until first asked for.
    pub(crate) stream: Vec<Stream>,
    /// Leaves whose records are stale.
    pub(crate) dirty: BitSet,
    /// Every record is stale.
    pub(crate) all: bool,
    /// How many leaves the mesh had when the dirty set was last sized.
    leaves: usize,
    /// The groups' version the face-group masks were last worked out at.
    groups_seen: u64,
    /// Which vertices are on each face group a layer is masked by, worked
    /// out once per change of the groups.
    group_flags: Vec<(u16, Arc<Vec<bool>>)>,
}

impl Live {
    pub(crate) fn bytes(&self) -> usize {
        self.stream.len() * 16 + self.group_flags.iter().map(|(_, f)| f.len()).sum::<usize>()
    }
}

/// What the composite of a vertex reads besides the layers.
pub(super) struct Ctx<'a> {
    layers: Vec<&'a PaintLayer>,
    table: &'a [u8],
    mask: &'a [f32],
    groups: Vec<(u16, Arc<Vec<bool>>)>,
}

fn unit(b: u8) -> f32 {
    b as f32 / 255.0
}

fn byte(x: f32) -> u8 {
    (x * 255.0).round().clamp(0.0, 255.0) as u8
}

impl<'a> Ctx<'a> {
    pub(super) fn new(stack: &'a PaintStack, mesh: &'a Mesh, cached: &[(u16, Arc<Vec<bool>>)]) -> Ctx<'a> {
        let layers: Vec<&PaintLayer> = stack.layers.iter().filter(|l| l.row.visible && l.row.opacity > 0).collect();
        let mut groups: Vec<(u16, Arc<Vec<bool>>)> = Vec::new();
        for layer in &layers {
            if let MaskSource::FaceGroup(g) = layer.row.mask {
                if groups.iter().any(|(id, _)| *id == g) {
                    continue;
                }
                let flags = cached
                    .iter()
                    .find(|(id, f)| *id == g && f.len() == mesh.vertex_count())
                    .map(|(_, f)| f.clone())
                    .unwrap_or_else(|| Arc::new(mesh.group_vertices(g)));
                groups.push((g, flags));
            }
        }
        Ctx { layers, table: &stack.table, mask: &mesh.mask, groups }
    }

    /// How much of `layer` shows at vertex `v`: its opacity times its mask.
    fn reach(&self, layer: &PaintLayer, v: usize) -> f32 {
        let opacity = unit(layer.row.opacity);
        let masked = match layer.row.mask {
            MaskSource::None => 1.0,
            MaskSource::Paintable => layer.data.mask.as_ref().and_then(|m| m.get(v)).map_or(1.0, |m| unit(m.0)),
            MaskSource::SculptMask { invert } => {
                let m = self.mask.get(v).copied().unwrap_or(0.0).clamp(0.0, 1.0);
                if invert { 1.0 - m } else { m }
            }
            MaskSource::Cavity => layer.data.cavity.as_ref().and_then(|m| m.get(v)).map_or(1.0, |m| unit(m.0)),
            MaskSource::FaceGroup(g) => {
                let on = self.groups.iter().find(|(id, _)| *id == g).is_some_and(|(_, f)| f.get(v).copied().unwrap_or(false));
                if on { 1.0 } else { 0.0 }
            }
        };
        opacity * masked
    }

    pub(super) fn vertex(&self, v: usize) -> Stream {
        let mut colour = Rgba::CLEAR;
        let mut weights = [0.0f32; MOST_MATERIALS];
        let (mut rough, mut rough_w, mut metal, mut metal_w) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        for layer in &self.layers {
            let reach = self.reach(layer, v);
            if reach <= 0.0 {
                continue;
            }
            let on = layer.row.channels;
            let data = &layer.data;
            if let (true, Some(list)) = (on.any(ChannelSet::COLOUR), &data.colour) {
                let c = list[v].0;
                let src = Rgba { rgb: [unit(c[0]), unit(c[1]), unit(c[2])], a: unit(c[3]) * reach };
                colour = over(layer.row.colour_blend, src, colour);
            }
            if let (true, Some(list)) = (on.any(ChannelSet::MATERIAL), &data.material) {
                let [slot, w] = list[v].0;
                if let Some(k) = self.table.iter().position(|&s| s == slot).filter(|_| slot != 0 && w > 0) {
                    lay_material(&mut weights, k, unit(w) * reach, layer.row.material_blend);
                }
            }
            if let Some(list) = &data.rough_metal {
                let [r, rw, m, mw] = list[v].0;
                if on.any(ChannelSet::ROUGHNESS) {
                    (rough, rough_w) = lay_over(unit(r), unit(rw) * reach, rough, rough_w);
                }
                if on.any(ChannelSet::METALLIC) {
                    (metal, metal_w) = lay_over(unit(m), unit(mw) * reach, metal, metal_w);
                }
            }
        }
        let mut out = [0u8; 16];
        out[..3].copy_from_slice(&colour.rgb.map(byte));
        out[3] = byte(colour.a);
        out[4..8].copy_from_slice(&[byte(rough), byte(metal), byte(rough_w), byte(metal_w)]);
        for (k, w) in weights.iter().enumerate() {
            out[8 + k] = byte(*w);
        }
        out
    }
}

/// A value with weight `a` laid over one with weight `b_w`.
fn lay_over(value: f32, a: f32, under: f32, under_w: f32) -> (f32, f32) {
    let w = a + under_w * (1.0 - a);
    if w <= 0.0 { (0.0, 0.0) } else { ((value * a + under * under_w * (1.0 - a)) / w, w.min(1.0)) }
}

/// Material `k` laid on the weights with weight `a` (see the module notes).
pub(crate) fn lay_material(weights: &mut [f32; MOST_MATERIALS], k: usize, a: f32, blend: MaterialBlend) {
    let a = a.clamp(0.0, 1.0);
    match blend {
        MaterialBlend::Normal => {
            for w in weights.iter_mut() {
                *w *= 1.0 - a;
            }
            weights[k] += a;
        }
        MaterialBlend::Add => {
            weights[k] = (weights[k] + a).min(1.0);
            let total: f32 = weights.iter().sum();
            let others = total - weights[k];
            if total > 1.0 && others > 0.0 {
                let scale = (1.0 - weights[k]) / others;
                for (j, w) in weights.iter_mut().enumerate() {
                    if j != k {
                        *w *= scale;
                    }
                }
            }
        }
    }
}

/// The composite of every vertex of `mesh`, worked out afresh from its
/// paint; `None` for a mesh without paint. What exporters and bakes read.
pub fn export_streams(mesh: &Mesh) -> Option<Vec<Stream>> {
    let stack = mesh.paint_stack()?;
    let ctx = Ctx::new(&stack, mesh, &[]);
    Some((0..mesh.vertex_count()).into_par_iter().with_min_len(PER_TASK).map(|v| ctx.vertex(v)).collect())
}

/// The paint at a point of a triangle, as the composite has it.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    /// sRGB red, green, blue from 0 to 1, and coverage.
    pub colour: [f32; 4],
    /// Rough, metal, rough weight, metal weight.
    pub rough_metal: [f32; 4],
    /// Each material of the local table there, by palette slot, with its weight.
    pub materials: Vec<(u8, f32)>,
}

/// The paint at the point `bary` of triangle `tri`: the corners' composites
/// mixed by the barycentric weights. `None` without paint, or for a triangle
/// the mesh does not have. For a bake, or a colour picker.
pub fn sample(mesh: &Mesh, tri: u32, bary: [f32; 3]) -> Option<Sample> {
    let stack = mesh.paint_stack()?;
    let corners = mesh.indices.get(tri as usize * 3..tri as usize * 3 + 3)?;
    let ctx = Ctx::new(&stack, mesh, &[]);
    let records: Vec<Stream> = corners.iter().map(|&v| ctx.vertex(v as usize)).collect();
    let at = |byte: usize| records.iter().zip(bary).map(|(r, w)| unit(r[byte]) * w).sum::<f32>();
    Some(Sample {
        colour: [at(0), at(1), at(2), at(3)],
        rough_metal: [at(4), at(5), at(6), at(7)],
        materials: stack.table.iter().enumerate().map(|(k, &slot)| (slot, at(8 + k))).collect(),
    })
}

impl Mesh {
    /// Fits the paint to the mesh as it is now (see `fit.rs`) and sizes what
    /// is kept per vertex and per leaf with it. Called by everything that
    /// reads or writes paint on a mesh that may have changed.
    pub(crate) fn paint_fit(&mut self) {
        let Some(mut paint) = self.paint.take() else { return };
        let (from, n) = (paint.stack.len, self.vertex_count());
        let leaves = self.bvh.leaves.len();
        if from != n {
            super::fit::fit_stack(&mut paint.stack, self);
        }
        if leaves != paint.live.leaves {
            // Dynamic topology adds leaves for the vertices it adds, and
            // leaves the old vertices where they were; fewer leaves is a
            // mesh laid out again.
            paint.live.dirty.grow(leaves);
            paint.live.all |= leaves < paint.live.leaves;
            paint.live.leaves = leaves;
        }
        if from < n {
            // Grown by dynamic topology: only the new vertices need working out.
            for v in from..n {
                paint.live.dirty.set(self.bvh.vert_leaf[v] as usize);
            }
        } else if from > n {
            paint.live.all = true;
        }
        if paint.live.stream.len() != n && !paint.live.stream.is_empty() {
            paint.live.stream.resize(n, [0; 16]);
        }
        paint.scratch.fit(n);
        self.paint = Some(paint);
    }

    /// Marks every vertex's composite stale: after a change to a layer's
    /// settings, or to the stack.
    pub fn paint_changed_all(&mut self) {
        if let Some(paint) = &mut self.paint {
            paint.live.all = true;
        }
    }

    /// Marks the composite of the vertices owned by `leaves` stale.
    pub fn paint_changed(&mut self, leaves: &[u32]) {
        if let Some(paint) = &mut self.paint {
            for &l in leaves {
                if (l as usize) < paint.live.leaves {
                    paint.live.dirty.set(l as usize);
                }
            }
        }
    }

    /// Brings the composite up to date and gives back the vertex ranges
    /// whose records changed (empty without paint). `mask_dirty` is the
    /// leaves whose sculpt mask changed since the last call, for layers
    /// masked by it. Called from `take_dirty`.
    pub(crate) fn paint_take_dirty(&mut self, mask_dirty: &BitSet) -> Vec<std::ops::Range<u32>> {
        if self.paint.is_none() {
            return Vec::new();
        }
        self.paint_fit();
        let groups_version = self.groups_version();
        let Some(mut paint) = self.paint.take() else { return Vec::new() };
        let live = &mut paint.live;
        if paint.stack.reads(true) && mask_dirty.any() {
            mask_dirty.for_each(|l| {
                if l < live.leaves {
                    live.dirty.set(l);
                }
            });
        }
        if live.groups_seen != groups_version {
            live.groups_seen = groups_version;
            live.group_flags.clear();
            if paint.stack.reads(false) {
                live.all = true;
            }
        }
        let n = self.vertex_count();
        if live.stream.len() != n {
            live.stream = vec![[0; 16]; n];
            live.all = true;
        }
        let ctx = Ctx::new(&paint.stack, self, &live.group_flags);
        // Kept for the next call: worked out once per change of the groups.
        for (g, flags) in &ctx.groups {
            if !live.group_flags.iter().any(|(id, _)| id == g) {
                live.group_flags.push((*g, flags.clone()));
            }
        }
        let ranges = if live.all {
            live.stream.par_iter_mut().enumerate().with_min_len(PER_TASK).for_each(|(v, out)| *out = ctx.vertex(v));
            vec![0..n as u32]
        } else if live.dirty.any() {
            let mut leaves = Vec::new();
            live.dirty.for_each(|l| leaves.push(l as u32));
            let out = SharedMut::new(&mut live.stream);
            let bvh = &self.bvh;
            leaves.par_iter().with_min_len(16).for_each(|&l| {
                let leaf = &bvh.leaves[l as usize];
                for v in leaf.vert_start as usize..(leaf.vert_end as usize).min(n) {
                    // SAFETY: every leaf owns a disjoint vertex range.
                    unsafe { out.write(v, ctx.vertex(v)) };
                }
            });
            self.ranges(&live.dirty)
        } else {
            Vec::new()
        };
        drop(ctx);
        paint.live.all = false;
        paint.live.dirty.clear();
        self.paint = Some(paint);
        ranges
    }

    /// The composite as the last `take_dirty` left it: one record per
    /// vertex, for the renderer. Empty without paint.
    pub fn paint_stream(&self) -> &[Stream] {
        self.paint.as_ref().map_or(&[], |p| &p.live.stream)
    }
}
