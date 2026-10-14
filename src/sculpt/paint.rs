//! The Paint brush (Polypaint, B.6 of the design): colour, a material,
//! roughness and metallic laid on the active paint layer under the ring.
//!
//! **A stroke like any other.** The Paint brush is a kernel of its own
//! (`Brush::Paint`), so its strokes come down the same road as a sculpting
//! brush's: the pen's pressure, Lazy Mouse, Spacing, Scatter and Size Jitter,
//! the falloff and the alpha, the mirror axes and the turns round a radial
//! axis, Front Facing, the sculpt mask (masked surface takes no paint, as it
//! takes no clay), hidden faces and Stay in Group. One stroke is one undo
//! step (`StrokeDelta::Paint`). It never moves a vertex unless Zadd is on,
//! and it never adds any: dynamic topology refines only for the brushes it
//! names (`sculpt/dyntopo.rs`), and this is not one of them.
//!
//! **What a dab writes** is the paint data's own business
//! (`mesh/paint/stroke.rs`): each vertex is taken toward the colour by the
//! brush's weight there, its stroke alpha building toward the Opacity and
//! never past it, worked out again from the stroke-start texel every dab so
//! nothing drifts. The brush's Strength slider is the Flow: how much of the
//! way to the Opacity one dab goes.
//!
//! **Symmetric copies** of a Paint or Erase dab are laid in turn, after
//! `Mesh::paint_begin_dab`: a vertex two copies reach gets what one copy
//! alone gives it, whichever goes first, so a mirrored stroke is mirrored to
//! the bit and nothing doubles on the plane. Blur and Smear read their
//! neighbours, so their copies are laid as one: every vertex any copy
//! reaches takes the strongest copy's weight, every read is done before any
//! write (Jacobi), and the result cannot depend on which copy came first.
//!
//! **The modes** (`PaintMode`):
//!
//! - Paint lays the colour (and the material, roughness, metallic: whichever
//!   channels are ticked);
//! - Erase takes this layer's paint away toward nothing, colour coverage,
//!   material weight and the roughness and metallic weights alike;
//! - Blur evens the layer out toward the mean of each vertex's neighbours;
//! - Smear picks up what it passes over and drags it along: each copy of
//!   the brush carries a texel, laid down by its weight and refreshed from
//!   the paint under it every dab;
//! - Sample is the eyedropper: a click picks the colour and material of the
//!   paint under it (the app does it; no stroke is made).
//!
//! Ctrl (or the right button) swaps Paint and Erase for one stroke, as it
//! swaps adding and removing for a sculpting brush. Shift blurs the paint
//! while the Paint brush is in hand, instead of smoothing the clay: a stroke
//! of a painting brush must never move the surface unasked.
//!
//! **Zadd** ("colour and sculpt") runs a sculpting kernel in the same dab as
//! the paint, with the Paint brush's ring, falloff and alpha and that
//! kernel's own Strength and options, so a stroke raises the paint it lays.
//! The kernel is a choice of the Paint card (`ZADD_BRUSHES`, Standard to
//! begin with), not the brush that was in hand before: that would change
//! under the sculptor's hand every time another brush was picked. The stroke
//! is one undo step that holds both (`StrokeDelta::Painted`).

use glam::Vec3;
use rayon::prelude::*;

use super::Brush;
use super::kernels::{self, Dab, Params, Scratch, SymOp};
use crate::mesh::Mesh;
use crate::mesh::paint::{
    ChannelSet, Mat8, PaintMode as Toward, PaintTarget, Rgba8, RoughMetal8, Texel, Unorm8, Value,
};

mod kept;
#[cfg(test)]
mod tests;

pub use kept::PaintKept;

/// What the Paint brush does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PaintMode {
    #[default]
    Paint,
    Erase,
    Blur,
    Smear,
    Sample,
}

impl PaintMode {
    pub const ALL: [PaintMode; 5] = [PaintMode::Paint, PaintMode::Erase, PaintMode::Blur, PaintMode::Smear, PaintMode::Sample];

    pub fn name(self) -> &'static str {
        match self {
            PaintMode::Paint => "Paint",
            PaintMode::Erase => "Erase",
            PaintMode::Blur => "Blur",
            PaintMode::Smear => "Smear",
            PaintMode::Sample => "Sample",
        }
    }

    pub fn about(self) -> &'static str {
        match self {
            PaintMode::Paint => "Lays the colour (and the material, roughness and metallic that are ticked) on the active paint layer.",
            PaintMode::Erase => "Takes the active layer's paint away under the brush. Ctrl with Paint does the same.",
            PaintMode::Blur => "Softens the active layer's paint, as Smooth softens clay. Shift with the Paint brush does the same.",
            PaintMode::Smear => "Picks up the paint it passes over and drags it along the stroke, like a finger in wet paint.",
            PaintMode::Sample => "Click the model to pick up the colour and material there (the eyedropper; I does it anywhere).",
        }
    }

    pub fn named(name: &str) -> Option<PaintMode> {
        Self::ALL.into_iter().find(|m| m.name().eq_ignore_ascii_case(name.trim()))
    }
}

/// The sculpting kernels Zadd can run with the paint: the ones that raise or
/// cut a stroke of their own along the path.
pub const ZADD_BRUSHES: [Brush; 5] = [Brush::Standard, Brush::Clay, Brush::Layer, Brush::Inflate, Brush::Crease];

/// How many colours the swatch row remembers.
pub const RECENT: usize = 12;

/// The Paint brush's settings: the Paint card. Kept between runs (`prefs.rs`),
/// all but `material`, which is the scene's (`ScenePaint::active_material`)
/// and is handed in as a stroke begins.
#[derive(Clone, Debug, PartialEq)]
pub struct PaintSettings {
    /// sRGB.
    pub colour: [u8; 3],
    /// Colours painted with lately, newest first.
    pub recent: Vec<[u8; 3]>,
    /// The most one stroke lays at a vertex, 0 to 1. The brush's Strength is
    /// its Flow: how far toward this each dab goes.
    pub opacity: f32,
    pub mode: PaintMode,
    /// The channels the brush writes.
    pub rgb: bool,
    pub material: bool,
    pub roughness: bool,
    pub metallic: bool,
    /// The roughness and metallic laid where those channels are ticked, 0 to 1.
    pub roughness_value: f32,
    pub metallic_value: f32,
    /// Colour and sculpt: `zadd_brush` runs in the same dab.
    pub zadd: bool,
    pub zadd_brush: Brush,
    /// Write the active layer's own mask (its Paintable mask source)
    /// instead of its paint: Paint hides the layer there, Erase shows it.
    pub layer_mask: bool,
    /// Mask by Colour: how far from the colour still counts, 0 to 1.
    pub tolerance: f32,
    /// The palette slot paint lays as its material, from the scene, for the
    /// stroke about to begin. Not kept between runs.
    pub slot: Option<u8>,
    /// The brush in hand before P took up the Paint brush, by name, which P
    /// goes back to. Not kept between runs.
    pub came_from: Option<String>,
}

impl Default for PaintSettings {
    fn default() -> Self {
        PaintSettings {
            colour: [196, 62, 46],
            recent: Vec::new(),
            opacity: 1.0,
            mode: PaintMode::Paint,
            rgb: true,
            material: true,
            roughness: false,
            metallic: false,
            roughness_value: 0.5,
            metallic_value: 0.0,
            zadd: false,
            zadd_brush: Brush::Standard,
            layer_mask: false,
            tolerance: 0.15,
            slot: None,
            came_from: None,
        }
    }
}

impl PaintSettings {
    /// Puts `colour` at the front of the swatches.
    pub fn remember(&mut self, colour: [u8; 3]) {
        self.recent.retain(|&c| c != colour);
        self.recent.insert(0, colour);
        self.recent.truncate(RECENT);
    }

    /// The channels a stroke writes: the ticked ones, the material only with
    /// a material to lay (painting "no material" would rub out the ones
    /// under it), or the layer's own mask alone.
    pub fn channels(&self, mode: PaintMode) -> ChannelSet {
        if self.layer_mask {
            return ChannelSet::MASK;
        }
        let mut set = ChannelSet::NONE;
        if self.rgb {
            set = set.with(ChannelSet::COLOUR);
        }
        if self.material && (self.slot.is_some() || mode != PaintMode::Paint) {
            set = set.with(ChannelSet::MATERIAL);
        }
        if self.roughness {
            set = set.with(ChannelSet::ROUGHNESS);
        }
        if self.metallic {
            set = set.with(ChannelSet::METALLIC);
        }
        set
    }

    /// The texel Paint lays.
    pub fn texel(&self) -> Texel {
        let [r, g, b] = self.colour;
        let unit = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
        Texel {
            colour: Rgba8([r, g, b, 255]),
            material: Mat8(match self.slot {
                Some(slot) if slot != 0 => [slot, 255],
                _ => [0, 0],
            }),
            rough_metal: RoughMetal8([unit(self.roughness_value), 255, unit(self.metallic_value), 255]),
            // The layer's own mask painted shut: the layer is hidden there.
            mask: Unorm8(0),
        }
    }

    /// What a stroke begun now in `mode` does, with `invert` (Ctrl or the
    /// right button) swapping Paint and Erase. `None` for Sample, which is
    /// no stroke.
    pub fn stroke(&self, mode: PaintMode, invert: bool) -> Option<PaintDab> {
        let mode = match (mode, invert) {
            (PaintMode::Paint, true) => PaintMode::Erase,
            (PaintMode::Erase, true) => PaintMode::Paint,
            (mode, _) => mode,
        };
        let channels = self.channels(mode);
        let target = |toward| PaintTarget { texel: self.texel(), channels, mode: toward };
        let how = match mode {
            PaintMode::Paint => How::Lay(target(Toward::Paint)),
            PaintMode::Erase => How::Lay(target(Toward::Erase)),
            PaintMode::Blur => How::Blur(channels),
            PaintMode::Smear => How::Smear(channels),
            PaintMode::Sample => return None,
        };
        Some(PaintDab { how, cap: self.opacity.clamp(0.0, 1.0) })
    }
}

/// What one stroke of the Paint brush does, as its dabs read it: part of the
/// stroke's settings (`Params::paint`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaintDab {
    pub how: How,
    /// The Opacity: the most the stroke lays.
    pub cap: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum How {
    /// Toward a texel (Paint), or toward nothing (Erase).
    Lay(PaintTarget),
    Blur(ChannelSet),
    Smear(ChannelSet),
}

/// Buffers a Paint dab fills, kept with the stroke's other scratch so that
/// no dab allocates.
#[derive(Default)]
pub struct Buffers {
    verts: Vec<u32>,
    weights: Vec<f32>,
    starts: Vec<usize>,
    leaves: Vec<u32>,
    /// Blur and Smear: which copy weighs most at each vertex, and what each
    /// vertex is taken toward.
    best: Vec<u16>,
    toward: Vec<Texel>,
    copies: Vec<Dab>,
}

/// What a stroke of the Paint brush keeps from dab to dab: Smear's carried
/// paint, one per copy of the brush.
#[derive(Default)]
pub struct Painting {
    carried: Vec<Option<Texel>>,
}

/// How much of what Smear passes over it takes up each dab.
const PICKUP: f32 = 0.35;

/// The brush's weight at vertex `v` for dab `d`: its falloff, the sculpt mask
/// (masked surface takes no paint), Front Facing, the alpha, the Flow and the
/// pen, and the dose that keeps Spacing from changing how fast a stroke
/// builds. 0 outside the ring.
#[inline]
fn weight(mesh: &Mesh, p: &Params, d: &Dab, v: u32) -> f32 {
    let i = v as usize;
    let pos = mesh.positions[i];
    let r = p.radius;
    let d2 = pos.distance_squared(d.center);
    if !(d2 < r * r) {
        return 0.0;
    }
    let w = p.falloff.at(d2.sqrt() / r, kernels::smooth)
        * (1.0 - mesh.mask[i])
        * kernels::facing(p.front_facing, mesh.normals[i], pos, d.cam)
        * p.printed(d, mesh.start_pos(v) - d.center);
    if w <= 0.0 { 0.0 } else { p.share((w * p.e(d.pressure)).clamp(0.0, 1.0)) }
}

/// Lays out every vertex `leaves` own into `b.verts`, ascending leaf by leaf
/// (so the paint data can share the writes out between threads), and gives
/// back how many.
fn lay_out(mesh: &Mesh, leaves: &[u32], b: &mut Buffers) -> usize {
    let total = kernels::layout(mesh, leaves, &mut b.starts);
    b.verts.resize(total, 0);
    kernels::each_stretch(&mut b.verts[..total], &b.starts, |k, part| {
        let range = kernels::owned(&mesh.bvh.leaves[leaves[k] as usize]);
        for (slot, v) in part.iter_mut().zip(range) {
            *slot = v as u32;
        }
    });
    total
}

/// One copy of a Paint or Erase dab, on the vertices `leaves` own: what
/// `kernels::apply` does for the Paint brush.
pub(super) fn lay(mesh: &mut Mesh, p: &Params, d: &Dab, leaves: &[u32], scratch: &mut Scratch) {
    let Some(PaintDab { how: How::Lay(target), cap }) = p.paint else { return };
    let b = &mut scratch.paint;
    let total = lay_out(mesh, leaves, b);
    kernels::fill(mesh, leaves, &mut b.starts, &mut b.weights, |_, v| weight(mesh, p, d, v));
    let weights = &b.weights[..total];
    // Refused only with no layer to take it, which the app sees to before the stroke.
    let _ = mesh.paint_dab(&b.verts[..total], &target, cap, |k, _| weights[k]);
}

/// One dab of the Paint brush and all its symmetric copies (see the module
/// notes). `reach` is how far from a copy's centre it can touch.
pub(super) fn dab(mesh: &mut Mesh, p: &Params, d: &Dab, ops: &[SymOp], reach: f32, state: &mut Painting, scratch: &mut Scratch) {
    let Some(paint) = p.paint else { return };
    match paint.how {
        How::Lay(_) => {
            mesh.paint_begin_dab();
            let mut leaves = std::mem::take(&mut scratch.paint.leaves);
            for &op in ops {
                let copy = d.mirrored(op);
                leaves.clear();
                mesh.bvh.leaves_in_sphere(copy.center, reach, &mut leaves);
                leaves.sort_unstable();
                leaves.dedup();
                if !leaves.is_empty() {
                    kernels::apply(mesh, p, &copy, &leaves, &leaves, scratch);
                }
            }
            scratch.paint.leaves = leaves;
        }
        How::Blur(which) | How::Smear(which) => spread(mesh, p, d, ops, reach, which, state, &mut scratch.paint),
    }
}

/// Blur and Smear: every copy at once (see the module notes).
#[allow(clippy::too_many_arguments)]
fn spread(mesh: &mut Mesh, p: &Params, d: &Dab, ops: &[SymOp], reach: f32, which: ChannelSet, state: &mut Painting, b: &mut Buffers) {
    let Some(layer) = mesh.paint_writable().ok() else { return };
    mesh.paint_fit();
    // Only what the layer has: blurring a channel it has none of would make one of nothing.
    let present = mesh.paint.as_ref().and_then(|paint| paint.stack.layer(layer)).map_or(ChannelSet::NONE, |l| l.data.present());
    let which = ChannelSet(which.0 & present.0);
    if which.is_empty() {
        return;
    }
    b.copies.clear();
    b.copies.extend(ops.iter().map(|&op| d.mirrored(op)));
    b.leaves.clear();
    mesh.bvh.leaves_in_spheres(b.copies.iter().map(|c| (c.center, reach)), &mut b.leaves);
    b.leaves.sort_unstable();
    b.leaves.dedup();
    if b.leaves.is_empty() {
        return;
    }
    let leaves = std::mem::take(&mut b.leaves);
    let total = lay_out(mesh, &leaves, b);
    b.leaves = leaves;
    let copies = &b.copies;
    // Hidden surface is neither written nor read: it is out of the brush's reach altogether.
    let hidden = mesh.hidden_vertices();
    let weight = |copy: &Dab, v: u32| if hidden.is_some_and(|h| h[v as usize]) { 0.0 } else { weight(mesh, p, copy, v) };
    // The strongest copy at each vertex. Exactly equal weights (a vertex on
    // a mirror plane) are told apart below by what they carry, which does
    // not depend on the copies' order.
    let mut best = std::mem::take(&mut b.best);
    best.resize(total, 0);
    b.weights.resize(total, 0.0);
    {
        let (verts, weights) = (&b.verts[..total], &mut b.weights[..total]);
        weights.par_iter_mut().zip(best[..total].par_iter_mut()).zip(verts.par_iter()).with_min_len(1024).for_each(
            |((w, k), &v)| {
                let mut top = (0.0f32, 0u16);
                for (c, copy) in copies.iter().enumerate() {
                    let here = weight(copy, v);
                    if here > top.0 {
                        top = (here, c as u16);
                    }
                }
                (*w, *k) = top;
            },
        );
    }
    let data = mesh.paint.as_ref().and_then(|paint| paint.stack.layer(layer)).map(|l| &l.data);
    let Some(data) = data else { return };
    let texel = |v: u32| data.texel(v as usize);
    b.toward.resize(total, Texel::BARE);
    match p.paint.map(|paint| paint.how) {
        Some(How::Smear(_)) => {
            // What each copy has under it, then what it carries this dab.
            let means: Vec<Option<Texel>> =
                copies.iter().map(|copy| footprint(&b.verts[..total], |v| weight(copy, v), &texel)).collect();
            state.carried.resize(copies.len(), None);
            for (carried, mean) in state.carried.iter_mut().zip(means) {
                *carried = match (*carried, mean) {
                    (Some(c), Some(m)) => Some(mixed(c, m, PICKUP)),
                    (c, m) => m.or(c),
                };
            }
            let carried = &state.carried;
            let verts = &b.verts[..total];
            let (weights, best) = (&b.weights[..total], &best[..total]);
            b.toward[..total].par_iter_mut().enumerate().with_min_len(1024).for_each(|(k, out)| {
                if weights[k] <= 0.0 {
                    return;
                }
                // Of the copies that weigh exactly as much, the one carrying the "largest" texel.
                let v = verts[k];
                let mut pick: Option<Texel> = None;
                for (c, copy) in copies.iter().enumerate() {
                    let here = if c == best[k] as usize { weights[k] } else { weight(copy, v) };
                    if here == weights[k] {
                        if let Some(t) = carried[c] {
                            pick = Some(pick.map_or(t, |q| if key(t) > key(q) { t } else { q }));
                        }
                    }
                }
                *out = pick.unwrap_or_else(|| texel(v));
            });
        }
        _ => {
            let verts = &b.verts[..total];
            let weights = &b.weights[..total];
            b.toward[..total].par_iter_mut().enumerate().with_min_len(1024).for_each(|(k, out)| {
                if weights[k] > 0.0 {
                    *out = ring_mean(mesh, data, verts[k]);
                }
            });
        }
    }
    b.best = best;
    let (weights, toward) = (&b.weights[..total], &b.toward[..total]);
    let verts = std::mem::take(&mut b.verts);
    let _ = mesh.paint_edit(&verts[..total], which, |k, _, edit| {
        let w = weights[k];
        if w <= 0.0 {
            return false;
        }
        edit.texel = mixed(edit.texel, toward[k], w);
        true
    });
    b.verts = verts;
}

/// A texel as a number to order texels by, for picking one of several
/// whatever order they came in.
fn key(t: Texel) -> u64 {
    u64::from_le_bytes([t.colour.0[0], t.colour.0[1], t.colour.0[2], t.colour.0[3], t.material.0[0], t.material.0[1], t.mask.0, 0])
        ^ ((u32::from_le_bytes(t.rough_metal.0) as u64) << 32).rotate_left(7)
}

/// `a` taken `t` of the way to `b`, by the rules a new vertex between two
/// painted ones gets its paint by (`mesh/paint.rs`): colour by coverage, the
/// stronger material, the rest plainly.
pub fn mixed(a: Texel, b: Texel, t: f32) -> Texel {
    let t = t.clamp(0.0, 1.0) as f64;
    let w = [1.0 - t, t];
    Texel {
        colour: Rgba8::weighted(&[a.colour, b.colour], &w),
        material: Mat8::weighted(&[a.material, b.material], &w),
        rough_metal: RoughMetal8::weighted(&[a.rough_metal, b.rough_metal], &w),
        mask: Unorm8::weighted(&[a.mask, b.mask], &w),
    }
}

/// The most neighbours of a vertex Blur counts: as many as a mesh ever
/// gives one but in freak spots, which then blur by the first of them.
const MOST_NEIGHBOURS: usize = 32;

/// The mean of the paint of `v`'s neighbours, by the same rules.
fn ring_mean(mesh: &Mesh, data: &crate::mesh::paint::Channels, v: u32) -> Texel {
    let ring = mesh.neighbours(v);
    let n = ring.len().min(MOST_NEIGHBOURS);
    if n == 0 {
        return data.texel(v as usize);
    }
    let ones = [1.0f64; MOST_NEIGHBOURS];
    let mut colour = [Rgba8::default(); MOST_NEIGHBOURS];
    let mut material = [Mat8::default(); MOST_NEIGHBOURS];
    let mut rough_metal = [RoughMetal8::default(); MOST_NEIGHBOURS];
    let mut mask = [Unorm8::default(); MOST_NEIGHBOURS];
    for (k, &o) in ring.iter().take(n).enumerate() {
        let t = data.texel(o as usize);
        (colour[k], material[k], rough_metal[k], mask[k]) = (t.colour, t.material, t.rough_metal, t.mask);
    }
    Texel {
        colour: Rgba8::weighted(&colour[..n], &ones[..n]),
        material: Mat8::weighted(&material[..n], &ones[..n]),
        rough_metal: RoughMetal8::weighted(&rough_metal[..n], &ones[..n]),
        mask: Unorm8::weighted(&mask[..n], &ones[..n]),
    }
}

/// Running totals for a weighted mean of texels over a footprint.
struct Totals {
    weight: f64,
    cover: f64,
    covered: [f64; 3],
    plain: [f64; 3],
    rough_metal: [f64; 4],
    mask: f64,
    /// Each palette slot's weight, summed.
    slots: Box<[f64; 256]>,
}

impl Default for Totals {
    fn default() -> Self {
        Totals {
            weight: 0.0,
            cover: 0.0,
            covered: [0.0; 3],
            plain: [0.0; 3],
            rough_metal: [0.0; 4],
            mask: 0.0,
            slots: Box::new([0.0; 256]),
        }
    }
}

impl Totals {
    fn add(&mut self, t: Texel, w: f64) {
        self.weight += w;
        let cover = t.colour.0[3] as f64 * w;
        self.cover += cover;
        for k in 0..3 {
            self.covered[k] += t.colour.0[k] as f64 * cover;
            self.plain[k] += t.colour.0[k] as f64 * w;
        }
        for k in 0..4 {
            self.rough_metal[k] += t.rough_metal.0[k] as f64 * w;
        }
        self.mask += t.mask.0 as f64 * w;
        self.slots[t.material.0[0] as usize] += t.material.0[1] as f64 * w;
    }

    fn merge(mut self, o: Totals) -> Totals {
        self.weight += o.weight;
        self.cover += o.cover;
        for k in 0..3 {
            self.covered[k] += o.covered[k];
            self.plain[k] += o.plain[k];
        }
        for k in 0..4 {
            self.rough_metal[k] += o.rough_metal[k];
        }
        self.mask += o.mask;
        for (a, b) in self.slots.iter_mut().zip(o.slots.iter()) {
            *a += b;
        }
        self
    }

    fn mean(&self) -> Option<Texel> {
        if self.weight <= 0.0 {
            return None;
        }
        let byte = |x: f64| x.round().clamp(0.0, 255.0) as u8;
        let w = self.weight;
        let c = |k: usize| if self.cover > 0.0 { self.covered[k] / self.cover } else { self.plain[k] / w };
        let (mut slot, mut most) = (0u8, 0.0f64);
        for (s, &sum) in self.slots.iter().enumerate().skip(1) {
            if sum > most {
                (slot, most) = (s as u8, sum);
            }
        }
        let mat = byte(most / w);
        Some(Texel {
            colour: Rgba8([byte(c(0)), byte(c(1)), byte(c(2)), byte(self.cover / w)]),
            material: Mat8(if mat == 0 { [0, 0] } else { [slot, mat] }),
            rough_metal: RoughMetal8(std::array::from_fn(|k| byte(self.rough_metal[k] / w))),
            mask: Unorm8(byte(self.mask / w)),
        })
    }
}

/// The paint under one copy of the brush: the mean of `verts`' texels, each
/// by the copy's weight there. Summed in fixed runs and merged in order, so
/// it does not depend on how the work was shared between threads.
fn footprint(verts: &[u32], weight: impl Fn(u32) -> f32 + Sync, texel: &(impl Fn(u32) -> Texel + Sync)) -> Option<Texel> {
    let run = |part: &[u32]| {
        let mut totals = Totals::default();
        for &v in part {
            let w = weight(v);
            if w > 0.0 {
                totals.add(texel(v), w as f64);
            }
        }
        totals
    };
    let parts: Vec<Totals> = verts.par_chunks(4096).map(run).collect();
    parts.into_iter().fold(Totals::default(), Totals::merge).mean()
}

/// What the eyedropper picks at a point of a triangle: the colour of the
/// paint there and its strongest material, if any is painted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Picked {
    /// sRGB, and how much of the surface it covers, 0 to 1.
    pub colour: [u8; 3],
    pub coverage: f32,
    /// The palette slot that weighs most there, if one weighs anything.
    pub material: Option<u8>,
}

/// The paint at `bary` of triangle `tri`, as the eyedropper sees it.
pub fn pick(mesh: &Mesh, tri: u32, bary: [f32; 3]) -> Option<Picked> {
    let sample = crate::mesh::paint::sample(mesh, tri, bary)?;
    let byte = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    let material = sample
        .materials
        .iter()
        .filter(|(_, w)| *w > 0.5 / 255.0)
        .fold(None::<(u8, f32)>, |best, &(slot, w)| match best {
            Some((_, b)) if b >= w => best,
            _ => Some((slot, w)),
        })
        .map(|(slot, _)| slot);
    let [r, g, b, a] = sample.colour;
    Some(Picked { colour: [byte(r), byte(g), byte(b)], coverage: a, material })
}

/// Where a ray from `origin` along `dir` meets the mesh, and the paint there.
pub fn pick_along(mesh: &Mesh, origin: Vec3, dir: Vec3) -> Option<Picked> {
    let hit = mesh.raycast(origin, dir)?;
    pick(mesh, hit.tri, hit.bary)
}
