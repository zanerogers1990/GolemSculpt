//! Paint strokes (B.6): what the painting brush writes, and how.
//!
//! A paint stroke is an ordinary stroke of the mesh (`Mesh::begin_stroke`,
//! then `end_stroke`), so everything a sculpt stroke has comes with it: one
//! undo step, symmetry, Stay in Group, hidden faces left alone. Its writes go
//! to the active layer, through [`Mesh::paint_edit`], which keeps a
//! stroke-start copy of each texel it writes, stamped with the stroke like
//! the mesh's own `orig_pos`, and gives back what the stroke changed when it
//! ends (`StrokeDelta::Paint`).
//!
//! **No drift.** Each touched vertex also keeps a stroke alpha, a float that
//! says how far the stroke has taken it toward the brush's colour. A texel
//! is always worked out again from its stroke-start value and that alpha
//! ([`PaintTarget::applied`]), never from the bytes the last dab left, so
//! a hundred light dabs land where one heavy one would and the rounding to
//! bytes happens once, not once a dab.
//!
//! **Opacity is a ceiling.** Flow builds the alpha up dab by dab toward the
//! brush's opacity (`cap`) and never past it: scrubbing over the same spot
//! at 40% gives 40%, as a painting program does.
//!
//! **Mirrored writes are idempotent.** Within one dab (`paint_begin_dab`)
//! every write to a vertex starts from the alpha the vertex had when the dab
//! began, and the vertex keeps the largest result. So when two symmetric
//! copies of a dab reach the same vertex (on the mirror plane, or where the
//! copies overlap) the vertex gets what one copy alone would give it, in
//! whatever order the copies are applied: painting is not doubled where they
//! meet.

use rayon::prelude::*;

use super::{ChannelSet, Mat8, Paint, PaintDelta, PaintRefusal, Rgba8, RoughMetal8, Texel, Unorm8};
use crate::mesh::{Mesh, PAR_MIN_VERTS};
use crate::util::SharedMut;

/// What a paint stroke keeps per vertex while it runs (see the module notes).
/// Allocated the first time a mesh is painted, and kept: no stroke allocates.
#[derive(Default)]
pub(crate) struct Scratch {
    /// The mesh's stroke number the copies below are of; 0 for none.
    stroke: u32,
    /// The layer the stroke writes.
    layer: u32,
    /// The channels it has written.
    written: ChannelSet,
    /// The lists it found missing and made, which its undo takes away again.
    opened: ChannelSet,
    stamp: Vec<u32>,
    start: Vec<Texel>,
    alpha: Vec<f32>,
    dab: u32,
    dab_stamp: Vec<u32>,
    dab_alpha: Vec<f32>,
    touched: Vec<u32>,
}

impl Scratch {
    pub(crate) fn bytes(&self) -> usize {
        self.stamp.len() * (4 + 12 + 4 + 4 + 4) + self.touched.len() * 4
    }

    /// Keeps the lists as long as the mesh, once there are lists.
    pub(crate) fn fit(&mut self, n: usize) {
        if self.stamp.is_empty() || self.stamp.len() == n {
            return;
        }
        self.stamp.resize(n, 0);
        self.start.resize(n, Texel::BARE);
        self.alpha.resize(n, 0.0);
        self.dab_stamp.resize(n, 0);
        self.dab_alpha.resize(n, 0.0);
        self.touched.retain(|&v| (v as usize) < n);
    }

    fn open(&mut self, n: usize) {
        if self.stamp.len() != n {
            self.stamp = vec![0; n];
            self.start = vec![Texel::BARE; n];
            self.alpha = vec![0.0; n];
            self.dab_stamp = vec![0; n];
            self.dab_alpha = vec![0.0; n];
        }
    }
}

/// One texel as a paint write sees it, and what the write changes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TexelEdit {
    /// The texel when the stroke began.
    pub start: Texel,
    /// The texel now; the write sets it to what it should be.
    pub texel: Texel,
    /// How far the stroke has taken the vertex so far (0 to 1); the write
    /// sets it to how far it has taken it now.
    pub alpha: f32,
    /// What `alpha` was when the current dab began.
    pub dab_alpha: f32,
}

/// What a paint dab does to a texel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PaintMode {
    /// Toward the target.
    #[default]
    Paint,
    /// Toward nothing on this layer: colour coverage, material weight and
    /// the override weights go to zero, and the layer's own mask opens.
    Erase,
}

/// What a dab paints: a texel, the channels of it to write, and how.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaintTarget {
    pub texel: Texel,
    pub channels: ChannelSet,
    pub mode: PaintMode,
}

fn mix(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t).round().clamp(0.0, 255.0) as u8
}

fn unit(b: u8) -> f32 {
    b as f32 / 255.0
}

fn byte(x: f32) -> u8 {
    (x * 255.0).round().clamp(0.0, 255.0) as u8
}

/// `value` with weight `a` laid over `under` with weight `under_w`: a
/// colour over another, a roughness over another.
fn over(value: [u8; 3], a: f32, under: [u8; 3], under_w: f32) -> ([u8; 3], f32) {
    let w = a + under_w * (1.0 - a);
    if w <= 0.0 {
        return (under, 0.0);
    }
    let c = |k: usize| byte((unit(value[k]) * a + unit(under[k]) * under_w * (1.0 - a)) / w);
    ([c(0), c(1), c(2)], w)
}

impl PaintTarget {
    /// The texel a vertex whose stroke-start texel was `start` has when the
    /// stroke has taken it `alpha` of the way (see the module notes).
    pub fn applied(&self, start: Texel, alpha: f32) -> Texel {
        let a = alpha.clamp(0.0, 1.0);
        let (want, which) = (self.texel, self.channels);
        let mut out = start;
        match self.mode {
            PaintMode::Paint => {
                if which.any(ChannelSet::COLOUR) {
                    let s = start.colour.0;
                    let t = want.colour.0;
                    let (rgb, w) = over([t[0], t[1], t[2]], a * unit(t[3]), [s[0], s[1], s[2]], unit(s[3]));
                    out.colour = Rgba8([rgb[0], rgb[1], rgb[2], byte(w)]);
                }
                if which.any(ChannelSet::MATERIAL) {
                    out.material = material_toward(start.material, want.material, a);
                }
                let (s, t) = (start.rough_metal.0, want.rough_metal.0);
                let mut rm = s;
                if which.any(ChannelSet::ROUGHNESS) {
                    let (v, w) = over([t[0], 0, 0], a * unit(t[1]), [s[0], 0, 0], unit(s[1]));
                    (rm[0], rm[1]) = (v[0], byte(w));
                }
                if which.any(ChannelSet::METALLIC) {
                    let (v, w) = over([t[2], 0, 0], a * unit(t[3]), [s[2], 0, 0], unit(s[3]));
                    (rm[2], rm[3]) = (v[0], byte(w));
                }
                out.rough_metal = RoughMetal8(rm);
                if which.any(ChannelSet::MASK) {
                    out.mask = Unorm8(mix(start.mask.0, want.mask.0, a));
                }
            }
            PaintMode::Erase => {
                let fade = |b: u8| mix(b, 0, a);
                if which.any(ChannelSet::COLOUR) {
                    let s = start.colour.0;
                    out.colour = Rgba8([s[0], s[1], s[2], fade(s[3])]);
                }
                if which.any(ChannelSet::MATERIAL) {
                    let w = fade(start.material.0[1]);
                    out.material = if w == 0 { Mat8([0, 0]) } else { Mat8([start.material.0[0], w]) };
                }
                let mut rm = start.rough_metal.0;
                if which.any(ChannelSet::ROUGHNESS) {
                    rm[1] = fade(rm[1]);
                }
                if which.any(ChannelSet::METALLIC) {
                    rm[3] = fade(rm[3]);
                }
                out.rough_metal = RoughMetal8(rm);
                if which.any(ChannelSet::MASK) {
                    out.mask = Unorm8(mix(start.mask.0, 255, a));
                }
            }
        }
        out
    }
}

/// A material texel taken `a` of the way from `start` toward `want`. One
/// texel holds one material, so where the two differ the old one's weight
/// falls and the new one's rises through zero, crossing where they are
/// equal: continuous, with no jump in what shows.
fn material_toward(start: Mat8, want: Mat8, a: f32) -> Mat8 {
    let ([s_slot, s_w], [t_slot, t_w]) = (start.0, want.0);
    if s_slot == t_slot || s_w == 0 {
        let from = if s_slot == t_slot { s_w } else { 0 };
        let w = mix(from, t_w, a);
        return if w == 0 || t_slot == 0 { Mat8([0, 0]) } else { Mat8([t_slot, w]) };
    }
    let x = a * t_w as f32 - (1.0 - a) * s_w as f32;
    let w = x.abs().round().clamp(0.0, 255.0) as u8;
    match (x > 0.0, w) {
        (_, 0) => Mat8([0, 0]),
        (true, w) if t_slot != 0 => Mat8([t_slot, w]),
        (true, _) => Mat8([0, 0]),
        (false, w) => Mat8([s_slot, w]),
    }
}

/// The lists of the layer a stroke writes, opened so that several threads
/// can each work on their own vertices.
struct Lists<'a> {
    colour: Option<SharedMut<'a, Rgba8>>,
    material: Option<SharedMut<'a, Mat8>>,
    rough_metal: Option<SharedMut<'a, RoughMetal8>>,
    mask: Option<SharedMut<'a, Unorm8>>,
}

impl Lists<'_> {
    /// # Safety
    /// No other thread may touch vertex `v` while this runs.
    unsafe fn texel(&self, v: usize) -> Texel {
        // SAFETY: as the caller promises.
        unsafe {
            Texel {
                colour: self.colour.as_ref().map_or_else(Rgba8::default, |l| l.read(v)),
                material: self.material.as_ref().map_or_else(Mat8::default, |l| l.read(v)),
                rough_metal: self.rough_metal.as_ref().map_or_else(RoughMetal8::default, |l| l.read(v)),
                mask: self.mask.as_ref().map_or(Unorm8(255), |l| l.read(v)),
            }
        }
    }

    /// # Safety
    /// No other thread may touch vertex `v` while this runs.
    unsafe fn put(&self, v: usize, t: Texel) {
        // SAFETY: as the caller promises.
        unsafe {
            if let Some(l) = &self.colour {
                l.write(v, t.colour);
            }
            if let Some(l) = &self.material {
                l.write(v, t.material);
            }
            if let Some(l) = &self.rough_metal {
                l.write(v, t.rough_metal);
            }
            if let Some(l) = &self.mask {
                l.write(v, t.mask);
            }
        }
    }
}

/// The scratch lists, opened the same way.
struct Shared<'a> {
    stamp: SharedMut<'a, u32>,
    start: SharedMut<'a, Texel>,
    alpha: SharedMut<'a, f32>,
    dab_stamp: SharedMut<'a, u32>,
    dab_alpha: SharedMut<'a, f32>,
    stroke: u32,
    dab: u32,
}

impl Mesh {
    /// The layer a paint stroke or fill would go to, or why there is none.
    pub fn paint_writable(&self) -> Result<u32, PaintRefusal> {
        let paint = self.paint.as_ref().ok_or(PaintRefusal::NoLayer)?;
        let id = paint.stack.active.ok_or(PaintRefusal::NoLayer)?;
        let layer = paint.stack.layer(id).ok_or(PaintRefusal::NoLayer)?;
        if layer.row.locked {
            return Err(PaintRefusal::Locked(layer.row.name.clone()));
        }
        Ok(id)
    }

    /// Starts a new dab of the paint stroke now open: writes from here on
    /// start from the alpha each vertex had at this point (see the module
    /// notes on mirrored writes). Call once per dab, before its symmetric
    /// copies are applied.
    pub fn paint_begin_dab(&mut self) {
        if let Some(paint) = &mut self.paint {
            let s = &mut paint.scratch;
            s.dab = s.dab.wrapping_add(1);
            if s.dab == 0 {
                s.dab_stamp.fill(0);
                s.dab = 1;
            }
        }
    }

    /// Paint's version of `move_listed`: offers each of `verts` to `f` with
    /// its texel on the active layer (and its stroke-start texel and stroke
    /// alpha), and writes back the channels `which` of whatever `f` leaves in
    /// the texel when it returns true. Every vertex written is recorded in the
    /// stroke-start copy first. Hidden vertices, and with Stay in Group the
    /// vertices of other groups, are passed over. The work is shared between
    /// threads when `verts` is ascending with no repeats. Gives back how many
    /// vertices were written.
    ///
    /// Only inside a stroke (`Mesh::begin_stroke`), and only on a layer that
    /// takes paint ([`Mesh::paint_writable`]).
    pub fn paint_edit(
        &mut self,
        verts: &[u32],
        which: ChannelSet,
        f: impl Fn(usize, u32, &mut TexelEdit) -> bool + Sync,
    ) -> Result<usize, PaintRefusal> {
        if !self.in_stroke {
            return Err(PaintRefusal::NotInStroke);
        }
        self.paint_fit();
        let layer_id = self.paint_writable()?;
        let n = self.vertex_count();
        let stroke = self.stroke_id;
        let Some(mut paint) = self.paint.take() else { return Err(PaintRefusal::NoLayer) };
        let written = self.paint_write(&mut paint, layer_id, stroke, n, verts, which, &f);
        // Marked here, on one thread: the composite of these leaves is stale.
        for &v in &written {
            paint.live.dirty.set(self.bvh.vert_leaf[v as usize] as usize);
        }
        let count = written.len();
        self.paint = Some(paint);
        Ok(count)
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_write(
        &self,
        paint: &mut Paint,
        layer_id: u32,
        stroke: u32,
        n: usize,
        verts: &[u32],
        which: ChannelSet,
        f: &(impl Fn(usize, u32, &mut TexelEdit) -> bool + Sync),
    ) -> Vec<u32> {
        let Paint { stack, scratch, live } = paint;
        live.dirty.grow(self.bvh.leaves.len());
        if scratch.stroke != stroke {
            // The first write of this stroke: a fresh record.
            if stroke == 1 {
                scratch.stamp.fill(0);
            }
            (scratch.stroke, scratch.layer, scratch.written) = (stroke, layer_id, ChannelSet::NONE);
            scratch.opened = ChannelSet::NONE;
            scratch.touched.clear();
            // A dab of its own, whether or not the brush says where dabs begin.
            scratch.dab = scratch.dab.wrapping_add(1).max(1);
        }
        scratch.open(n);
        scratch.written = scratch.written.with(which);
        let index = stack.index_of(scratch.layer).unwrap_or(0);
        let data = &mut stack.layers[index].data;
        let had = data.present();
        data.open(which, n);
        scratch.opened = scratch.opened.with(data.present().without(had));
        let lists = Lists {
            colour: data.colour.as_deref_mut().map(SharedMut::new),
            material: data.material.as_deref_mut().map(SharedMut::new),
            rough_metal: data.rough_metal.as_deref_mut().map(SharedMut::new),
            mask: data.mask.as_deref_mut().map(SharedMut::new),
        };
        let shared = Shared {
            stamp: SharedMut::new(&mut scratch.stamp),
            start: SharedMut::new(&mut scratch.start),
            alpha: SharedMut::new(&mut scratch.alpha),
            dab_stamp: SharedMut::new(&mut scratch.dab_stamp),
            dab_alpha: SharedMut::new(&mut scratch.dab_alpha),
            stroke,
            dab: scratch.dab,
        };
        let hidden: &[bool] = if self.hidden.keeps_off() { self.hidden.kept_off() } else { &[] };
        // Returns whether the vertex was written, and whether for the first time this stroke.
        let run = |k: usize, v: u32| -> (bool, bool) {
            let i = v as usize;
            if i >= n || hidden.get(i).copied().unwrap_or(false) {
                return (false, false);
            }
            // SAFETY: either one thread runs, or each vertex is listed once.
            unsafe {
                let now = lists.texel(i);
                let fresh = shared.stamp.read(i) != shared.stroke;
                let (start, alpha) = if fresh { (now, 0.0) } else { (shared.start.read(i), shared.alpha.read(i)) };
                let this_dab = !fresh && shared.dab_stamp.read(i) == shared.dab;
                let dab_alpha = if this_dab { shared.dab_alpha.read(i) } else { alpha };
                let mut edit = TexelEdit { start, texel: now, alpha, dab_alpha };
                if !f(k, v, &mut edit) {
                    return (false, false);
                }
                if fresh {
                    shared.stamp.write(i, shared.stroke);
                    shared.start.write(i, now);
                }
                if !this_dab {
                    shared.dab_stamp.write(i, shared.dab);
                    shared.dab_alpha.write(i, alpha);
                }
                shared.alpha.write(i, edit.alpha);
                lists.put(i, now.with(edit.texel, which));
                (true, fresh)
            }
        };
        let distinct =
            verts.len() >= PAR_MIN_VERTS && verts.par_windows(2).with_min_len(4096).all(|w| w[0] < w[1]);
        let mut written = Vec::new();
        if distinct {
            let parts: Vec<(Vec<u32>, Vec<u32>)> = verts
                .par_iter()
                .enumerate()
                .with_min_len(1024)
                .fold(
                    || (Vec::new(), Vec::new()),
                    |(mut wrote, mut first), (k, &v)| {
                        let (w, fresh) = run(k, v);
                        if w {
                            wrote.push(v);
                        }
                        if fresh {
                            first.push(v);
                        }
                        (wrote, first)
                    },
                )
                .collect();
            for (wrote, first) in parts {
                written.extend(wrote);
                scratch.touched.extend(first);
            }
        } else {
            for (k, &v) in verts.iter().enumerate() {
                let (w, fresh) = run(k, v);
                if w {
                    written.push(v);
                }
                if fresh {
                    scratch.touched.push(v);
                }
            }
        }
        written
    }

    /// One dab of paint: each of `verts` is taken toward `target` by
    /// `strength(k, v)` (0 to 1: the brush's falloff, flow and pressure at
    /// the vertex), with the stroke's alpha built up toward `cap` (the
    /// brush's opacity) and never past it. See the module notes for why this
    /// neither drifts nor doubles where symmetric copies meet. Gives back
    /// how many vertices were written.
    pub fn paint_dab(
        &mut self,
        verts: &[u32],
        target: &PaintTarget,
        cap: f32,
        strength: impl Fn(usize, u32) -> f32 + Sync,
    ) -> Result<usize, PaintRefusal> {
        let cap = cap.clamp(0.0, 1.0);
        self.paint_edit(verts, target.channels, |k, v, edit| {
            let s = strength(k, v).clamp(0.0, 1.0);
            if s <= 0.0 || !s.is_finite() {
                return false;
            }
            let from = edit.dab_alpha.min(cap);
            let reached = (from + s * (cap - from)).min(cap).max(edit.alpha);
            if reached <= edit.alpha {
                return false;
            }
            edit.alpha = reached;
            edit.texel = target.applied(edit.start, reached);
            true
        })
    }

    /// The record of the paint the stroke now ending wrote, if it wrote any
    /// that changed a texel. Called as a stroke ends (`end_stroke`).
    pub(crate) fn paint_stroke_record(&mut self) -> Option<PaintDelta> {
        let stroke = self.stroke_id;
        let paint = self.paint.as_mut()?;
        let s = &mut paint.scratch;
        if s.stroke != stroke {
            return None;
        }
        s.stroke = 0;
        let mut verts = std::mem::take(&mut s.touched);
        verts.sort_unstable();
        verts.dedup();
        let data = &paint.stack.layer(s.layer)?.data;
        let which = s.written;
        let (mut kept, mut before, mut after) = (Vec::new(), Vec::new(), Vec::new());
        for v in verts {
            let (was, now) = (s.start[v as usize], data.texel(v as usize));
            if was.with(now, which) != was {
                kept.push(v);
                before.push(was);
                after.push(now);
            }
        }
        let opened = s.opened;
        (!kept.is_empty()).then(|| PaintDelta { layer: s.layer, channels: which, opened, verts: kept, before, after })
    }
}
