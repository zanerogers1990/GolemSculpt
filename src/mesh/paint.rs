//! Polypaint: colour, material, roughness and metallic painted onto the
//! vertices of a mesh, in layers (ZBrush's Polypaint, Nomad's vertex paint).
//!
//! **Per vertex, not in a texture.** A sculpt has millions of vertices and no
//! UVs, and a dense mesh holds as much paint detail as it holds form, so the
//! paint lives on the vertices like the mask does. It goes wherever the
//! vertices go: undo, a save, a change of subdivision level, a remesh (by the
//! nearest point of the old surface, `transfer.rs`), a join, a copy.
//!
//! **Zero cost without paint.** `Mesh::paint` is `None` until something is
//! painted or a layer is made. With it, each layer keeps only the channels
//! that have been written (`Channels`, every list allocated on first write),
//! so a colour-only layer costs four bytes a vertex.
//!
//! **What a layer holds**, per vertex (B.2 of the design):
//!
//! - colour: sRGB red, green, blue and how much of it there is (coverage);
//! - material: which material of the scene's palette (0 for none, 1-255) and
//!   how much of it (`Mat`);
//! - roughness and metallic overrides, each with its own weight
//!   (`RoughMetal`: rough, rough weight, metal, metal weight);
//! - a mask of its own the sculptor can paint (`mask`, shown where 255), and
//!   a cavity snapshot (`cavity`) for a layer masked by the cavity as it was
//!   when the snapshot was taken.
//!
//! **The layer stack** (`PaintStack`) is listed bottom first, the order it is
//! composited in; a card shows it top first. Each layer has a colour blend
//! mode (`blend.rs`), a material blend (Normal or Add), an opacity, an eye, a
//! lock, channel toggles and a mask source. The stack also has the object's
//! local material table: the palette slots its paint uses, at most eight
//! (`MOST_MATERIALS`), which is what the renderer evaluates per fragment.
//!
//! **The composite** (`composite.rs`) folds the visible layers into one
//! interleaved 16-byte record per vertex for the renderer and exporters,
//! recomputed only for the BVH leaves that changed (`DirtyRanges::paint`).
//!
//! **Dynamic topology is not told about paint.** It appends vertices (and,
//! on undo, takes them off again) without knowing paint exists, so every
//! list here may be shorter or longer than the mesh for a while. Whatever
//! reads or writes paint first fits it to the mesh (`fit.rs`): a vertex
//! dynamic topology added takes the mean of its painted neighbours, and a list
//! longer than the mesh is cut back. Vertices that existed before keep their
//! paint to the bit. What is not exact is redo of a refining stroke: the
//! vertices it brings back are given their paint again from their
//! neighbours, which is what they were given the first time unless they were
//! painted on in between and the stroke was then undone and redone.
//!
//! **Undo.** A paint stroke is a `StrokeDelta::Paint` (the texels it changed,
//! before and after); a change to the stack (a layer made, merged, filled...)
//! is a `PaintStep` (`history::Edit::Paint`), which keeps the rows and only
//! the data of the layers the change touched, and is swapped back and forth.

use std::borrow::Cow;

use super::Mesh;

pub mod blend;
mod composite;
mod fit;
mod ops;
pub mod palette;
mod stroke;
pub mod transfer;
mod undo;

#[cfg(test)]
mod tests;

pub use blend::ColourBlend;
pub use composite::{Sample, Stream, export_streams, sample};
pub use ops::{FillArea, PaintRefusal};
pub use stroke::{PaintMode, PaintTarget, TexelEdit};
pub use undo::{PaintDelta, PaintStep};

/// The most materials one object's paint can use: the renderer evaluates
/// each of them per pixel, and eight dense weights are what fit in the 16
/// bytes a vertex the composite keeps.
pub const MOST_MATERIALS: usize = 8;

/// The most paint layers one object can have. Each can cost up to twelve
/// bytes a vertex, and a longer list no longer fits its card.
pub const MOST_PAINT_LAYERS: usize = 32;

/// Vertices handed to one rayon task.
const PER_TASK: usize = 1 << 13;

// ------------------------------------------------------------------ values

/// One kind of per-vertex value a layer keeps, with the rule that gives a
/// vertex made between others its value (B.2).
pub trait Value: Copy + PartialEq + Send + Sync + 'static {
    /// The value at a point that is `weights` of the way between `values`
    /// (weights that sum to one, or to anything above zero).
    fn weighted(values: &[Self], weights: &[f64]) -> Self;
}

/// Colour: sRGB red, green, blue and coverage. A new vertex takes the mean
/// with each colour counted by its coverage (premultiplied), so a painted
/// vertex beside a bare one gives a half-covered vertex of the painted colour
/// and not a darker one.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Rgba8(pub [u8; 4]);

/// Material: a palette slot (0 for none) and its weight. A new vertex takes
/// the slot with the most weight among its ends (the first met where two
/// tie), and the mean of that slot's weight with nothing counted where an end
/// has another slot. With the same slot at every end, that is the plain mean.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Mat8(pub [u8; 2]);

/// Roughness and metallic: rough, rough weight, metal, metal weight. A new
/// vertex takes the plain mean of each.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, bytemuck::Pod, bytemuck::Zeroable)]
pub struct RoughMetal8(pub [u8; 4]);

/// A single fraction from 0 to 255: a layer's own mask, a cavity snapshot. A
/// new vertex takes the plain mean.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Unorm8(pub u8);

fn byte(x: f64) -> u8 {
    x.round().clamp(0.0, 255.0) as u8
}

fn total(weights: &[f64]) -> f64 {
    let sum: f64 = weights.iter().sum();
    if sum > 0.0 { sum } else { 1.0 }
}

impl Value for Rgba8 {
    fn weighted(values: &[Self], weights: &[f64]) -> Self {
        let whole = total(weights);
        let (mut rgb, mut plain, mut a) = ([0.0f64; 3], [0.0f64; 3], 0.0f64);
        for (v, &w) in values.iter().zip(weights) {
            let cover = v.0[3] as f64 * w;
            a += cover;
            for k in 0..3 {
                rgb[k] += v.0[k] as f64 * cover;
                plain[k] += v.0[k] as f64 * w;
            }
        }
        // Where nothing is covered the colour is never seen; the plain mean keeps it smooth.
        let c = |k: usize| if a > 0.0 { rgb[k] / a } else { plain[k] / whole };
        Rgba8([byte(c(0)), byte(c(1)), byte(c(2)), byte(a / whole)])
    }
}

impl Value for Mat8 {
    fn weighted(values: &[Self], weights: &[f64]) -> Self {
        let whole = total(weights);
        let mut best = (0u8, 0.0f64);
        for (k, v) in values.iter().enumerate() {
            let slot = v.0[0];
            if slot == 0 || values[..k].iter().any(|o| o.0[0] == slot) {
                continue;
            }
            let sum: f64 = values.iter().zip(weights).filter(|(o, _)| o.0[0] == slot).map(|(o, w)| o.0[1] as f64 * w).sum();
            if sum > best.1 {
                best = (slot, sum);
            }
        }
        let weight = byte(best.1 / whole);
        if weight == 0 { Mat8([0, 0]) } else { Mat8([best.0, weight]) }
    }
}

impl Value for RoughMetal8 {
    fn weighted(values: &[Self], weights: &[f64]) -> Self {
        let whole = total(weights);
        let mean = |k: usize| byte(values.iter().zip(weights).map(|(v, w)| v.0[k] as f64 * w).sum::<f64>() / whole);
        RoughMetal8([mean(0), mean(1), mean(2), mean(3)])
    }
}

impl Value for Unorm8 {
    fn weighted(values: &[Self], weights: &[f64]) -> Self {
        let whole = total(weights);
        Unorm8(byte(values.iter().zip(weights).map(|(v, w)| v.0 as f64 * w).sum::<f64>() / whole))
    }
}

/// The value of a vertex made between `of` (equal shares).
pub fn mean_of<T: Value>(list: &[T], of: &[u32]) -> T {
    let values: Vec<T> = of.iter().map(|&v| list[v as usize]).collect();
    let weights = vec![1.0; values.len()];
    T::weighted(&values, &weights)
}

// ---------------------------------------------------------------- channels

/// Which channels: of a layer that are switched on, of a stroke that are
/// written. Bits, so a set is one byte in a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ChannelSet(pub u8);

impl ChannelSet {
    pub const COLOUR: ChannelSet = ChannelSet(1);
    pub const MATERIAL: ChannelSet = ChannelSet(2);
    pub const ROUGHNESS: ChannelSet = ChannelSet(4);
    pub const METALLIC: ChannelSet = ChannelSet(8);
    /// The layer's own paintable mask.
    pub const MASK: ChannelSet = ChannelSet(16);
    /// What a layer shows: everything but its mask.
    pub const SURFACE: ChannelSet = ChannelSet(15);
    pub const NONE: ChannelSet = ChannelSet(0);
    pub const ALL: ChannelSet = ChannelSet(31);

    pub fn has(self, other: ChannelSet) -> bool {
        self.0 & other.0 == other.0 && other.0 != 0
    }

    pub fn any(self, other: ChannelSet) -> bool {
        self.0 & other.0 != 0
    }

    pub fn with(self, other: ChannelSet) -> ChannelSet {
        ChannelSet(self.0 | other.0)
    }

    pub fn without(self, other: ChannelSet) -> ChannelSet {
        ChannelSet(self.0 & !other.0)
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl Default for ChannelSet {
    fn default() -> Self {
        ChannelSet::SURFACE
    }
}

/// Everything a layer keeps about one vertex, as a brush reads and writes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Texel {
    pub colour: Rgba8,
    pub material: Mat8,
    pub rough_metal: RoughMetal8,
    pub mask: Unorm8,
}

impl Texel {
    /// A vertex nothing has been painted on: clear, no material, no
    /// override, and the layer's own mask open.
    pub const BARE: Texel =
        Texel { colour: Rgba8([0; 4]), material: Mat8([0; 2]), rough_metal: RoughMetal8([0; 4]), mask: Unorm8(255) };

    /// This texel with the channels of `which` taken from `other`.
    pub fn with(self, other: Texel, which: ChannelSet) -> Texel {
        let mut out = self;
        if which.any(ChannelSet::COLOUR) {
            out.colour = other.colour;
        }
        if which.any(ChannelSet::MATERIAL) {
            out.material = other.material;
        }
        if which.any(ChannelSet::ROUGHNESS) {
            out.rough_metal.0[0] = other.rough_metal.0[0];
            out.rough_metal.0[1] = other.rough_metal.0[1];
        }
        if which.any(ChannelSet::METALLIC) {
            out.rough_metal.0[2] = other.rough_metal.0[2];
            out.rough_metal.0[3] = other.rough_metal.0[3];
        }
        if which.any(ChannelSet::MASK) {
            out.mask = other.mask;
        }
        out
    }
}

impl Default for Texel {
    fn default() -> Self {
        Texel::BARE
    }
}

/// A layer's per-vertex lists, each there only once something wrote it. A
/// list that is there is one entry per vertex of the mesh (once fitted, see
/// `fit.rs`).
#[derive(Clone, Default, PartialEq)]
pub struct Channels {
    pub colour: Option<Vec<Rgba8>>,
    pub material: Option<Vec<Mat8>>,
    pub rough_metal: Option<Vec<RoughMetal8>>,
    pub mask: Option<Vec<Unorm8>>,
    pub cavity: Option<Vec<Unorm8>>,
}

/// What an absent list reads as, list by list.
const BARE_MASK: Unorm8 = Unorm8(255);

/// Something done to every list of a layer, whatever its element type.
pub(crate) trait ListMap {
    fn map<T: Value>(&mut self, list: &[T], missing: T) -> Vec<T>;
}

/// Something done in place to every list of a layer.
pub(crate) trait ListEdit {
    fn edit<T: Value>(&mut self, list: &mut Vec<T>, missing: T);
}

impl Channels {
    /// The texel of vertex `v`, with whatever is not there as bare.
    pub fn texel(&self, v: usize) -> Texel {
        Texel {
            colour: self.colour.as_ref().and_then(|l| l.get(v).copied()).unwrap_or_default(),
            material: self.material.as_ref().and_then(|l| l.get(v).copied()).unwrap_or_default(),
            rough_metal: self.rough_metal.as_ref().and_then(|l| l.get(v).copied()).unwrap_or_default(),
            mask: self.mask.as_ref().and_then(|l| l.get(v).copied()).unwrap_or(BARE_MASK),
        }
    }

    /// Makes sure every list `which` names is there, `len` long.
    pub fn open(&mut self, which: ChannelSet, len: usize) {
        if which.any(ChannelSet::COLOUR) {
            self.colour.get_or_insert_with(|| vec![Rgba8::default(); len]);
        }
        if which.any(ChannelSet::MATERIAL) {
            self.material.get_or_insert_with(|| vec![Mat8::default(); len]);
        }
        if which.any(ChannelSet::ROUGHNESS.with(ChannelSet::METALLIC)) {
            self.rough_metal.get_or_insert_with(|| vec![RoughMetal8::default(); len]);
        }
        if which.any(ChannelSet::MASK) {
            self.mask.get_or_insert_with(|| vec![BARE_MASK; len]);
        }
    }

    /// Writes the channels of `which` of vertex `v`; the lists must be open.
    #[inline]
    pub fn put(&mut self, v: usize, t: Texel, which: ChannelSet) {
        let now = self.texel(v).with(t, which);
        if let Some(l) = &mut self.colour {
            l[v] = now.colour;
        }
        if let Some(l) = &mut self.material {
            l[v] = now.material;
        }
        if let Some(l) = &mut self.rough_metal {
            l[v] = now.rough_metal;
        }
        if let Some(l) = &mut self.mask {
            l[v] = now.mask;
        }
    }

    /// Which channels are there.
    pub fn present(&self) -> ChannelSet {
        let mut set = ChannelSet::NONE;
        if self.colour.is_some() {
            set = set.with(ChannelSet::COLOUR);
        }
        if self.material.is_some() {
            set = set.with(ChannelSet::MATERIAL);
        }
        if self.rough_metal.is_some() {
            set = set.with(ChannelSet::ROUGHNESS).with(ChannelSet::METALLIC);
        }
        if self.mask.is_some() {
            set = set.with(ChannelSet::MASK);
        }
        set
    }

    pub fn is_empty(&self) -> bool {
        self.colour.is_none() && self.material.is_none() && self.rough_metal.is_none() && self.mask.is_none() && self.cavity.is_none()
    }

    pub fn bytes(&self) -> usize {
        self.colour.as_ref().map_or(0, |l| l.len() * 4)
            + self.material.as_ref().map_or(0, |l| l.len() * 2)
            + self.rough_metal.as_ref().map_or(0, |l| l.len() * 4)
            + self.mask.as_ref().map_or(0, Vec::len)
            + self.cavity.as_ref().map_or(0, Vec::len)
    }

    /// Every list put through `f`, as a new set of lists.
    pub(crate) fn mapped(&self, f: &mut impl ListMap) -> Channels {
        Channels {
            colour: self.colour.as_ref().map(|l| f.map(l, Rgba8::default())),
            material: self.material.as_ref().map(|l| f.map(l, Mat8::default())),
            rough_metal: self.rough_metal.as_ref().map(|l| f.map(l, RoughMetal8::default())),
            mask: self.mask.as_ref().map(|l| f.map(l, BARE_MASK)),
            cavity: self.cavity.as_ref().map(|l| f.map(l, BARE_MASK)),
        }
    }

    /// Every list put through `f` in place.
    pub(crate) fn edit(&mut self, f: &mut impl ListEdit) {
        if let Some(l) = &mut self.colour {
            f.edit(l, Rgba8::default());
        }
        if let Some(l) = &mut self.material {
            f.edit(l, Mat8::default());
        }
        if let Some(l) = &mut self.rough_metal {
            f.edit(l, RoughMetal8::default());
        }
        if let Some(l) = &mut self.mask {
            f.edit(l, BARE_MASK);
        }
        if let Some(l) = &mut self.cavity {
            f.edit(l, BARE_MASK);
        }
    }
}

/// The list `place` puts in order: entry `i` of the result is `list[place[i]]`
/// (bare where `place` names nothing in it).
pub(crate) struct Gather<'a>(pub &'a [u32]);

impl ListMap for Gather<'_> {
    fn map<T: Value>(&mut self, list: &[T], missing: T) -> Vec<T> {
        use rayon::prelude::*;
        self.0.par_iter().with_min_len(PER_TASK).map(|&v| list.get(v as usize).copied().unwrap_or(missing)).collect()
    }
}

/// The list scattered by `place`: entry `place[i]` of the result, `len` long,
/// is `list[i]` (bare where nothing lands).
pub(crate) struct Scatter<'a>(pub &'a [u32], pub usize);

impl ListMap for Scatter<'_> {
    fn map<T: Value>(&mut self, list: &[T], missing: T) -> Vec<T> {
        let mut out = vec![missing; self.1];
        for (&value, &to) in list.iter().zip(self.0) {
            if let Some(slot) = out.get_mut(to as usize) {
                *slot = value;
            }
        }
        out
    }
}

// ------------------------------------------------------------------ layers

/// What a layer's mask is: where on the surface the layer shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum MaskSource {
    /// Everywhere.
    #[default]
    None,
    /// The layer's own mask, painted (`Channels::mask`).
    Paintable,
    /// The sculpt mask: where it is masked, or with `invert` where it is not.
    SculptMask { invert: bool },
    /// The cavity of the surface when the snapshot was taken
    /// (`Channels::cavity`): the layer shows in the recesses.
    Cavity,
    /// The faces of one face group.
    FaceGroup(u16),
}

/// The material blend of a layer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum MaterialBlend {
    /// The layer's material replaces what is under it, by its weight.
    #[default]
    Normal,
    /// The layer's material is added to what is under it; where that comes to
    /// more than all, the others give way.
    Add,
}

/// A layer's settings: one row of the Paint Layers card.
#[derive(Clone, Debug, PartialEq)]
pub struct PaintRow {
    /// Never handed out twice on one mesh, so an undo record naming a layer
    /// cannot mean another.
    pub id: u32,
    pub name: String,
    pub visible: bool,
    /// A locked layer takes no strokes, fills or clears.
    pub locked: bool,
    /// 0 to 255.
    pub opacity: u8,
    pub colour_blend: ColourBlend,
    pub material_blend: MaterialBlend,
    pub mask: MaskSource,
    /// The channels the layer adds to the composite (the card's toggles).
    pub channels: ChannelSet,
}

impl PaintRow {
    pub fn new(id: u32, name: String) -> PaintRow {
        PaintRow {
            id,
            name,
            visible: true,
            locked: false,
            opacity: 255,
            colour_blend: ColourBlend::Normal,
            material_blend: MaterialBlend::Normal,
            mask: MaskSource::None,
            channels: ChannelSet::SURFACE,
        }
    }
}

/// One layer: its row and its lists.
#[derive(Clone, PartialEq)]
pub struct PaintLayer {
    pub row: PaintRow,
    pub data: Channels,
}

/// The finest subdivision level's copy of one layer's lists, in the level
/// stack's own vertex order, while the mesh is on a coarser level (see
/// `levels/painted.rs`).
#[derive(Clone, PartialEq)]
pub struct FinestCopy {
    pub id: u32,
    pub data: Channels,
}

/// A mesh's paint as data: what undo, a file and a rebuild carry.
#[derive(Clone, PartialEq)]
pub struct PaintStack {
    /// Bottom first.
    pub layers: Vec<PaintLayer>,
    /// The layer strokes and fills go to.
    pub active: Option<u32>,
    pub next_id: u32,
    /// The palette slots the paint uses, at most `MOST_MATERIALS`, in the
    /// order the composite's weights are: weight `k` is of slot `table[k]`.
    pub table: Vec<u8>,
    /// The finest level's copies of the layers (see `FinestCopy`).
    pub finest: Vec<FinestCopy>,
    /// How many vertices the lists are for.
    pub len: usize,
}

impl PaintStack {
    pub fn new(len: usize) -> PaintStack {
        PaintStack { layers: Vec::new(), active: None, next_id: 1, table: Vec::new(), finest: Vec::new(), len }
    }

    pub fn index_of(&self, id: u32) -> Option<usize> {
        self.layers.iter().position(|l| l.row.id == id)
    }

    pub fn layer(&self, id: u32) -> Option<&PaintLayer> {
        self.layers.iter().find(|l| l.row.id == id)
    }

    pub fn layer_mut(&mut self, id: u32) -> Option<&mut PaintLayer> {
        self.layers.iter_mut().find(|l| l.row.id == id)
    }

    pub fn rows(&self) -> Vec<PaintRow> {
        self.layers.iter().map(|l| l.row.clone()).collect()
    }

    pub fn bytes(&self) -> usize {
        let layers: usize = self.layers.iter().map(|l| l.data.bytes() + l.row.name.len() + 32).sum();
        layers + self.finest.iter().map(|f| f.data.bytes()).sum::<usize>()
    }

    /// The table entry of palette slot `slot`, if the table has it.
    pub fn table_index(&self, slot: u8) -> Option<usize> {
        if slot == 0 { None } else { self.table.iter().position(|&s| s == slot) }
    }

    /// A name for a new layer that no layer has: `base`, or `base 2`...
    pub fn free_name(&self, base: &str) -> String {
        let taken = |name: &str| self.layers.iter().any(|l| l.row.name == name);
        if !taken(base) {
            return base.to_owned();
        }
        (2..).map(|n| format!("{base} {n}")).find(|name| !taken(name)).expect("some number is free")
    }

    /// Every list of every layer put in a new vertex order (see `Gather`),
    /// for a mesh of `place.len()` vertices. The finest copies are in the
    /// level stack's order and are not touched.
    pub(crate) fn gathered(&self, place: &[u32]) -> PaintStack {
        let mut gather = Gather(place);
        let layers =
            self.layers.iter().map(|l| PaintLayer { row: l.row.clone(), data: l.data.mapped(&mut gather) }).collect();
        PaintStack { layers, len: place.len(), ..self.header() }
    }

    /// The stack without its layers' lists: rows only (each with no data).
    pub(crate) fn header(&self) -> PaintStack {
        PaintStack {
            layers: Vec::new(),
            active: self.active,
            next_id: self.next_id,
            table: self.table.clone(),
            finest: self.finest.clone(),
            len: self.len,
        }
    }

    /// Whether any layer reads the sculpt mask, or face groups, through its
    /// mask source: those layers' composite changes when they do.
    pub(crate) fn reads(&self, sculpt_mask: bool) -> bool {
        self.layers.iter().any(|l| match l.row.mask {
            MaskSource::SculptMask { .. } => sculpt_mask,
            MaskSource::FaceGroup(_) => !sculpt_mask,
            _ => false,
        })
    }
}

/// A mesh's paint: the data (`stack`) and what is worked out from it.
pub struct Paint {
    pub(crate) stack: PaintStack,
    pub(crate) live: composite::Live,
    pub(crate) scratch: stroke::Scratch,
}

impl Paint {
    pub fn new(stack: PaintStack) -> Paint {
        Paint { stack, live: composite::Live::default(), scratch: stroke::Scratch::default() }
    }

    pub fn stack(&self) -> &PaintStack {
        &self.stack
    }

    pub fn bytes(&self) -> usize {
        self.stack.bytes() + self.live.bytes() + self.scratch.bytes()
    }
}

impl Mesh {
    /// Whether the mesh has paint (any layer, painted or not).
    pub fn has_paint(&self) -> bool {
        self.paint.is_some()
    }

    /// The mesh's paint layers' rows, bottom first; none without paint.
    pub fn paint_rows(&self) -> Vec<PaintRow> {
        self.paint.as_ref().map_or_else(Vec::new, |p| p.stack.rows())
    }

    /// The layer strokes and fills go to.
    pub fn paint_active(&self) -> Option<u32> {
        self.paint.as_ref()?.stack.active
    }

    /// The local material table: the palette slots the paint uses.
    pub fn paint_table(&self) -> &[u8] {
        self.paint.as_ref().map_or(&[], |p| &p.stack.table)
    }

    /// The paint as data, fitted to the mesh as it is now (see `fit.rs`):
    /// every list exactly one entry per vertex. Borrowed when it fits
    /// already, which is always but after dynamic topology changed the mesh
    /// and before anything else touched the paint.
    pub fn paint_stack(&self) -> Option<Cow<'_, PaintStack>> {
        let paint = self.paint.as_ref()?;
        if paint.stack.len == self.vertex_count() {
            return Some(Cow::Borrowed(&paint.stack));
        }
        let mut stack = paint.stack.clone();
        fit::fit_stack(&mut stack, self);
        Some(Cow::Owned(stack))
    }

    /// The paint for a copy of this mesh whose vertices were renumbered:
    /// vertex `v` of this mesh is vertex `place[v]` of the copy, which has
    /// `vertices` vertices.
    pub fn paint_renumbered(&self, place: &[u32], vertices: usize) -> Option<Box<Paint>> {
        let stack = self.paint_stack()?;
        let mut scatter = Scatter(place, vertices);
        let layers = stack
            .layers
            .iter()
            .map(|l| PaintLayer { row: l.row.clone(), data: l.data.mapped(&mut scatter) })
            .collect();
        Some(Box::new(Paint::new(PaintStack { layers, len: vertices, ..stack.header() })))
    }

    /// Puts `stack` on the mesh as its paint (`None` takes the paint off),
    /// fitted to it, with the composite to be worked out afresh.
    pub fn set_paint(&mut self, stack: Option<PaintStack>) {
        self.paint = stack.map(|mut stack| {
            fit::fit_stack(&mut stack, self);
            Box::new(Paint::new(stack))
        });
    }

    /// Whether the paint is fit to be written to a file and read back: every
    /// list one entry per vertex, every layer's number its own, the table no
    /// longer than it may be. For the recovery copy's check before it is
    /// trusted; says what is wrong if anything is.
    pub fn paint_sound(&self) -> Result<(), String> {
        let Some(stack) = self.paint_stack() else { return Ok(()) };
        let n = self.vertex_count();
        let fits = |l: Option<usize>| l.is_none_or(|l| l == n);
        for (k, layer) in stack.layers.iter().enumerate() {
            let d = &layer.data;
            let lens = [
                d.colour.as_ref().map(Vec::len),
                d.material.as_ref().map(Vec::len),
                d.rough_metal.as_ref().map(Vec::len),
                d.mask.as_ref().map(Vec::len),
                d.cavity.as_ref().map(Vec::len),
            ];
            if !lens.into_iter().all(fits) {
                return Err(format!("layer {} is not one entry per vertex", layer.row.name));
            }
            if layer.row.id == 0 || stack.layers[..k].iter().any(|o| o.row.id == layer.row.id) {
                return Err(format!("layer {} has a number another layer has", layer.row.name));
            }
        }
        if stack.table.len() > MOST_MATERIALS {
            return Err("uses more materials than an object can".into());
        }
        Ok(())
    }

    /// A number that is the same for the same paint on the same surface and,
    /// in practice, different for any other, whatever order the vertices are
    /// in: each vertex's position with every layer's texel there, and the
    /// rows. 0 for a mesh without paint. For checks across a save and an
    /// open, and the recovery fingerprint.
    pub fn paint_print_by_place(&self) -> u64 {
        use rayon::prelude::*;
        let Some(stack) = self.paint_stack() else { return 0 };
        let vertex = |v: usize| {
            let p = self.positions[v].to_array().map(|x| (x + 0.0).to_bits());
            let place = p.iter().fold(0u64, |h, &b| mix(h ^ b as u64));
            stack.layers.iter().fold(place, |h, l| {
                let t = l.data.texel(v);
                let bits = u64::from_le_bytes([
                    t.colour.0[0], t.colour.0[1], t.colour.0[2], t.colour.0[3], t.material.0[0], t.material.0[1], t.mask.0, 0,
                ]);
                mix(mix(h ^ bits) ^ u32::from_le_bytes(t.rough_metal.0) as u64)
            })
        };
        let all = (0..self.vertex_count()).into_par_iter().with_min_len(PER_TASK).map(vertex).reduce(|| 0, u64::wrapping_add);
        let rows = stack.layers.iter().fold(mix(all), |h, l| {
            let r = &l.row;
            let flags = (r.visible as u64) | (r.locked as u64) << 1 | (r.opacity as u64) << 8 | (r.colour_blend.to_byte() as u64) << 16;
            r.name.bytes().fold(mix(h ^ flags ^ (r.id as u64) << 32), |h, b| mix(h ^ b as u64))
        });
        stack.table.iter().fold(mix(rows ^ 1), |h, &s| mix(h ^ s as u64))
    }
}

/// Puts paint read from a file on the mesh built from it: `stack`'s lists
/// are in the file's vertex order, and file vertex `i` is mesh vertex
/// `place[i]`. The finest copies are kept only if the mesh has the
/// subdivision levels they were made for (they were put back first). Gives
/// back whether the paint fitted the mesh; if not, the mesh is left without.
pub fn restore(mesh: &mut Mesh, stack: PaintStack, place: &[u32]) -> bool {
    let n = mesh.vertex_count();
    let lists_fit = |data: &Channels, len: usize| {
        let ok = |l: Option<usize>| l.is_none_or(|l| l == len);
        ok(data.colour.as_ref().map(Vec::len))
            && ok(data.material.as_ref().map(Vec::len))
            && ok(data.rough_metal.as_ref().map(Vec::len))
            && ok(data.mask.as_ref().map(Vec::len))
            && ok(data.cavity.as_ref().map(Vec::len))
    };
    // A vertex no triangle uses is left out of the mesh (`u32::MAX`), and its paint with it.
    let whole = stack.len == place.len() && place.iter().all(|&v| v == u32::MAX || (v as usize) < n);
    if !whole || !stack.layers.iter().all(|l| lists_fit(&l.data, stack.len)) {
        return false;
    }
    let finest_len = mesh.levels.as_ref().filter(|l| l.current() < l.top()).map(|l| l.vertex_count(l.top()));
    let mut scatter = Scatter(place, n);
    let layers = stack.layers.iter().map(|l| PaintLayer { row: l.row.clone(), data: l.data.mapped(&mut scatter) }).collect();
    let finest = match finest_len {
        Some(len) => stack.finest.iter().filter(|f| lists_fit(&f.data, len)).cloned().collect(),
        None => Vec::new(),
    };
    mesh.set_paint(Some(PaintStack { layers, finest, len: n, ..stack.header() }));
    true
}

/// `groups::built_with` for a mesh whose vertices are also in the same order
/// as those `stack` is of (a mirror copy baked): the new mesh has the same
/// paint on the same vertices.
pub fn built_with(
    positions: Vec<glam::Vec3>,
    indices: Vec<u32>,
    mask: Option<Vec<f32>>,
    ids: &[u16],
    stack: Option<&PaintStack>,
) -> Mesh {
    let Some(stack) = stack.filter(|s| s.len == positions.len()) else {
        return super::groups::built_with(positions, indices, mask, ids);
    };
    let triangles = indices.len() / 3;
    let (mut mesh, numbering) = Mesh::new_numbered(positions, indices, mask, &[]);
    if ids.len() == triangles && !ids.is_empty() {
        mesh.set_group_ids(super::groups::renumbered(ids, &numbering.triangle));
    }
    restore(&mut mesh, stack.clone(), &numbering.vertex);
    mesh
}

/// A well-mixed number from `x`, for the prints.
fn mix(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}
