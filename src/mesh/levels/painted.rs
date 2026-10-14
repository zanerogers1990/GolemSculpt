//! A mesh's paint (`mesh/paint.rs`) through a change of subdivision level.
//!
//! Paint is a value per vertex, as the mask is, so it is carried the way the
//! level stack carries the mask (`sculpt_multires::Channel`, implemented here
//! for each kind of paint value with the rules of `paint::Value`):
//!
//! - **Subdivide** gives each new vertex the value of a vertex made between
//!   the vertices it lies between (`Levels::values_up`): no smoothing, and
//!   nothing leaves the range of the paint around it.
//! - **Down** a level, a vertex keeps its value (a coarse vertex is the fine
//!   vertex of the same number), and the finest level's lists wait beside the
//!   layers (`PaintStack::finest`, in the level stack's order) so that the
//!   detail painted there comes back to the bit.
//! - **Up** again, paint done on the coarser level is carried up onto the
//!   finest lists (`Levels::values_up_over`): where nothing was painted on
//!   the coarser level the finest values come back to the bit; where
//!   something was, the change is spread to the finer vertices around it and
//!   added to what they had.
//!
//! A layer made on a coarser level has no finest copy yet, and is carried up
//! from what it holds there. The finest copies are saved in the file with
//! the paint (`io/paint.rs`).

use sculpt_multires::{Channel, Levels};

use super::MeshLevels;
use crate::mesh::Mesh;
use crate::mesh::paint::{Channels, FinestCopy, Mat8, PaintStack, Rgba8, RoughMetal8, Scatter, Unorm8, Value, mean_of};

fn shift(value: u8, from: u8, to: u8) -> u8 {
    (value as i32 + to as i32 - from as i32).clamp(0, 255) as u8
}

impl Channel for Rgba8 {
    fn mean(values: &[Self], of: &[u32]) -> Self {
        mean_of(values, of)
    }
    fn shifted(self, from: Self, to: Self) -> Self {
        Rgba8([0, 1, 2, 3].map(|k| shift(self.0[k], from.0[k], to.0[k])))
    }
    fn same(self, other: Self) -> bool {
        self == other
    }
}

impl Channel for RoughMetal8 {
    fn mean(values: &[Self], of: &[u32]) -> Self {
        mean_of(values, of)
    }
    fn shifted(self, from: Self, to: Self) -> Self {
        RoughMetal8([0, 1, 2, 3].map(|k| shift(self.0[k], from.0[k], to.0[k])))
    }
    fn same(self, other: Self) -> bool {
        self == other
    }
}

impl Channel for Unorm8 {
    fn mean(values: &[Self], of: &[u32]) -> Self {
        mean_of(values, of)
    }
    fn shifted(self, from: Self, to: Self) -> Self {
        Unorm8(shift(self.0, from.0, to.0))
    }
    fn same(self, other: Self) -> bool {
        self == other
    }
}

impl Channel for Mat8 {
    fn mean(values: &[Self], of: &[u32]) -> Self {
        mean_of(values, of)
    }
    /// A material is moved by an edit of the same material by as much as its
    /// weight was; painted over with another, it takes the new one.
    fn shifted(self, from: Self, to: Self) -> Self {
        if from == to {
            return self;
        }
        if self.0[0] == from.0[0] && from.0[0] == to.0[0] {
            let w = shift(self.0[1], from.0[1], to.0[1]);
            return if w == 0 { Mat8([0, 0]) } else { Mat8([self.0[0], w]) };
        }
        to
    }
    fn same(self, other: Self) -> bool {
        self == other
    }
}

/// A list carried up from one level to a finer one: over the finer level's
/// own values where it has some, from the coarser values alone where not.
fn up<T: Value + Channel>(
    stack: &Levels,
    from: usize,
    to: usize,
    here: Option<&Vec<T>>,
    fine: Option<&Vec<T>>,
    missing: T,
) -> Option<Vec<T>> {
    let (coarse, finer) = (stack.vertex_count(from), stack.vertex_count(to));
    let fine = fine.filter(|f| f.len() == finer);
    let here_or_bare = |here: Option<&Vec<T>>| here.cloned().unwrap_or_else(|| vec![missing; coarse]);
    match (here, fine) {
        (None, None) => None,
        (_, Some(fine)) => stack.values_up_over(from, to, &here_or_bare(here), fine).ok(),
        (Some(here), None) => stack.values_up(from, to, here).ok(),
    }
}

/// Every list of a layer carried up (see [`up`]).
fn layer_up(stack: &Levels, from: usize, to: usize, here: &Channels, fine: Option<&Channels>) -> Channels {
    let bare = crate::mesh::paint::Texel::BARE;
    Channels {
        colour: up(stack, from, to, here.colour.as_ref(), fine.and_then(|f| f.colour.as_ref()), Rgba8::default()),
        material: up(stack, from, to, here.material.as_ref(), fine.and_then(|f| f.material.as_ref()), Mat8::default()),
        rough_metal: up(stack, from, to, here.rough_metal.as_ref(), fine.and_then(|f| f.rough_metal.as_ref()), RoughMetal8::default()),
        mask: up(stack, from, to, here.mask.as_ref(), fine.and_then(|f| f.mask.as_ref()), bare.mask),
        cavity: up(stack, from, to, here.cavity.as_ref(), fine.and_then(|f| f.cavity.as_ref()), bare.mask),
    }
}

/// The first `count` entries of a list: a level's list out of a finer one's.
fn cut<T: Clone>(list: &[T], count: usize) -> Vec<T> {
    list[..count.min(list.len())].to_vec()
}

/// [`cut`] for every list of a layer.
fn first(data: &Channels, count: usize) -> Channels {
    Channels {
        colour: data.colour.as_deref().map(|l| cut(l, count)),
        material: data.material.as_deref().map(|l| cut(l, count)),
        rough_metal: data.rough_metal.as_deref().map(|l| cut(l, count)),
        mask: data.mask.as_deref().map(|l| cut(l, count)),
        cavity: data.cavity.as_deref().map(|l| cut(l, count)),
    }
}

/// The paint of `mesh`, in the stack's vertex order, for the level being left.
fn here(mesh: &Mesh, levels: &MeshLevels) -> Option<PaintStack> {
    Some(mesh.paint_stack()?.gathered(&levels.vertex))
}

/// The paint of `mesh` on the level Subdivide has just added above its finest
/// (`grown` is the stack with it), in the stack's order.
pub(super) fn subdivided(mesh: &Mesh, levels: &MeshLevels, grown: &Levels) -> Option<PaintStack> {
    let mut stack = here(mesh, levels)?;
    let top = levels.top();
    for layer in &mut stack.layers {
        layer.data = layer_up(grown, top, top + 1, &layer.data, None);
    }
    stack.finest = Vec::new();
    stack.len = grown.vertex_count(top + 1);
    Some(stack)
}

/// The paint of `mesh` on level `to`, which it is stepping to, in the stack's order.
pub(super) fn stepped(mesh: &Mesh, levels: &MeshLevels, to: usize) -> Option<PaintStack> {
    let mut stack = here(mesh, levels)?;
    let (from, top) = (levels.current(), levels.top());
    let old = &levels.stack;
    let finest: Vec<Channels> = if from == top {
        stack.layers.iter().map(|l| l.data.clone()).collect()
    } else {
        stack
            .layers
            .iter()
            .map(|layer| {
                let kept = stack.finest.iter().find(|f| f.id == layer.row.id).map(|f| &f.data);
                layer_up(old, from, top, &layer.data, kept)
            })
            .collect()
    };
    let count = old.vertex_count(to);
    stack.finest = if to == top {
        Vec::new()
    } else {
        stack.layers.iter().zip(&finest).map(|(l, data)| FinestCopy { id: l.row.id, data: data.clone() }).collect()
    };
    for (layer, data) in stack.layers.iter_mut().zip(&finest) {
        layer.data = first(data, count);
    }
    stack.len = count;
    Some(stack)
}

/// Puts the paint worked out for a level on the mesh just built for it: the
/// lists from the stack's order into the mesh's (`MeshLevels::vertex`).
pub(super) fn arrive(mesh: &mut Mesh, stack: Option<PaintStack>) {
    let Some(mut stack) = stack else { return };
    let Some(vertex) = mesh.levels.as_ref().map(|levels| levels.vertex.clone()) else { return };
    let n = mesh.vertex_count();
    let mut scatter = Scatter(&vertex, n);
    for layer in &mut stack.layers {
        layer.data = layer.data.mapped(&mut scatter);
    }
    stack.len = n;
    mesh.set_paint(Some(stack));
}
