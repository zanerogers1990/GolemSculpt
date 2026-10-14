//! Undo of paint: a stroke's texels ([`PaintDelta`], inside a
//! `StrokeDelta`), and a change to the layer stack ([`PaintStep`], a
//! `history::Edit::Paint`).

use super::{ChannelSet, Channels, Paint, PaintLayer, PaintStack, Texel};

use crate::mesh::Mesh;

/// What a paint stroke changed: the texels of one layer, before and after,
/// for the channels it wrote.
pub struct PaintDelta {
    pub layer: u32,
    pub channels: ChannelSet,
    /// The lists the stroke made on the layer: undo takes them away again,
    /// so that the layer is as it was, list for list.
    pub opened: ChannelSet,
    pub verts: Vec<u32>,
    pub before: Vec<Texel>,
    pub after: Vec<Texel>,
}

impl PaintDelta {
    pub fn bytes(&self) -> usize {
        self.verts.len() * (4 + 2 * std::mem::size_of::<Texel>())
    }
}

impl Mesh {
    /// Undo (`undo`) or redo of a paint stroke. A vertex the record names
    /// that the mesh no longer has (dynamic topology took it off since) is
    /// passed over, as is a layer that is gone.
    pub(crate) fn apply_paint_delta(&mut self, delta: &PaintDelta, undo: bool) {
        self.paint_fit();
        let n = self.vertex_count();
        let Some(paint) = self.paint.as_mut() else { return };
        let Some(layer) = paint.stack.layer_mut(delta.layer) else { return };
        layer.data.open(delta.channels, n);
        let values = if undo { &delta.before } else { &delta.after };
        paint.live.dirty.grow(self.bvh.leaves.len());
        for (&v, &t) in delta.verts.iter().zip(values) {
            if (v as usize) < n {
                layer.data.put(v as usize, t, delta.channels);
                paint.live.dirty.set(self.bvh.vert_leaf[v as usize] as usize);
            }
        }
        if undo {
            let d = &mut layer.data;
            let gone = |set: ChannelSet| delta.opened.any(set);
            if gone(ChannelSet::COLOUR) {
                d.colour = None;
            }
            if gone(ChannelSet::MATERIAL) {
                d.material = None;
            }
            if gone(ChannelSet::ROUGHNESS.with(ChannelSet::METALLIC)) {
                d.rough_metal = None;
            }
            if gone(ChannelSet::MASK) {
                d.mask = None;
            }
        }
    }
}

/// The paint on the other side of an undo step that changed the stack: the
/// rows, and the lists of the layers whose lists differ between the two
/// sides (one that was filled, cleared, merged, deleted...). Stepping across
/// it swaps both with what the mesh has, so it serves undo and redo alike,
/// as `LayersStep` does for sculpt layers.
pub struct PaintStep {
    /// The other side's stack, with lists only for the layers in `owned`.
    /// `None`: the other side has no paint at all.
    other: Option<PaintStack>,
    /// The layers whose lists are this step's own (the rest are the mesh's,
    /// the same on both sides).
    owned: Vec<u32>,
}

impl PaintStep {
    /// The step of a change about to be made to `paint` that changes the
    /// lists of the layers `changing` (and may change any row).
    pub(crate) fn before(paint: Option<&PaintStack>, changing: &[u32]) -> PaintStep {
        let Some(stack) = paint else { return PaintStep { other: None, owned: Vec::new() } };
        let mut other = stack.header();
        let mut owned = Vec::new();
        for layer in &stack.layers {
            let mine = changing.contains(&layer.row.id);
            if mine {
                owned.push(layer.row.id);
            }
            let data = if mine { layer.data.clone() } else { Channels::default() };
            other.layers.push(PaintLayer { row: layer.row.clone(), data });
        }
        PaintStep { other: Some(other), owned }
    }

    pub fn bytes(&self) -> usize {
        self.other.as_ref().map_or(0, |s| s.bytes())
    }

    /// Steps the change back, or forward again: the same swap either way.
    pub fn swap(&mut self, mesh: &mut Mesh) {
        mesh.paint_fit();
        let mine = mesh.paint.take().map(|p| p.stack);
        let owned = std::mem::take(&mut self.owned);
        let mut theirs = self.other.take();
        if let Some(stack) = &mut theirs {
            // Its own lists may be for the mesh as it was before dynamic topology changed it.
            super::fit::fit_stack(stack, mesh);
        }
        let mut mine_layers = Vec::new();
        let mine_header = mine.map(|stack| {
            let header = stack.header();
            mine_layers = stack.layers;
            header
        });
        // Each list the two sides share moves across; `moved` notes which.
        let mut moved = vec![false; mine_layers.len()];
        if let Some(stack) = &mut theirs {
            for layer in &mut stack.layers {
                if owned.contains(&layer.row.id) {
                    continue;
                }
                if let Some(at) = mine_layers.iter().position(|m| m.row.id == layer.row.id) {
                    layer.data = std::mem::take(&mut mine_layers[at].data);
                    moved[at] = true;
                }
            }
        }
        // The way back: the mesh's side, owning every list that did not move.
        self.owned =
            mine_layers.iter().zip(&moved).filter(|(_, moved)| !**moved).map(|(layer, _)| layer.row.id).collect();
        self.other = mine_header.map(|mut header| {
            header.layers = mine_layers;
            header
        });
        mesh.paint = theirs.map(Paint::of);
    }
}

impl Paint {
    /// A fresh paint for `stack`, with the composite to be worked out.
    pub(crate) fn of(stack: PaintStack) -> Box<Paint> {
        Box::new(Paint::new(stack))
    }
}
