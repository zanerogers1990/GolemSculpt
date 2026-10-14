//! Paint carried onto a mesh built from others (B.2, item 4).
//!
//! - **A rebuild** (remesh, decimate, a boolean, a cut, a mesh back from
//!   Blender) says nothing about which new vertex came from which old one.
//!   Each new vertex takes the paint of the nearest point of the old surface,
//!   worked out from the corners of the old triangle there by the same rules
//!   a vertex made between others gets (`Value::weighted`, barycentric
//!   shares). The search is the one face groups use (`groups::nearest_points`)
//!   and the same rules decide what counts as on the old surface (`Fit`): a
//!   whole rebuild that is mostly not on the old surface is another shape and
//!   starts without paint; for a piece, new surface (a cap, a copy standing
//!   elsewhere) starts bare.
//! - **Copies** (an array made real, a join of plain shells, a copy of an
//!   object) know exactly which vertex is which, and carry each vertex's
//!   texels as they are.
//!
//! **Several sources** (a join, a boolean) make one stack: the first
//! source's layers (the active object's), then each other source's layers
//! matched to those by name, and a layer whose name is new added on top.
//! Material slots are the scene's, so they need no renumbering; the local
//! tables are merged (to at most `MOST_MATERIALS`: a material past that shows
//! as the Look's finish until the table is tidied).

use rayon::prelude::*;

use super::{Channels, MOST_MATERIALS, MOST_PAINT_LAYERS, PER_TASK, PaintLayer, PaintStack, Scatter, Value};
use crate::mesh::Mesh;
use crate::mesh::groups::{Fit, Nearest, Source, nearest_points};

/// What became of the paint when a mesh took another's place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Carried {
    /// No source had any.
    Nothing,
    /// The new mesh has it.
    Kept,
    /// The new mesh is another shape, and starts without paint.
    Dropped,
}

/// One stack made of several: the rows (with no lists) and, for each source,
/// which of its layers each layer of the result is (see the module notes).
fn merged(stacks: &[Option<&PaintStack>]) -> (PaintStack, Vec<Vec<Option<usize>>>) {
    let first = stacks.iter().flatten().next();
    let mut out = first.map_or_else(|| PaintStack::new(0), |s| s.header());
    out.finest = Vec::new();
    out.layers = Vec::new();
    let mut maps: Vec<Vec<Option<usize>>> = Vec::with_capacity(stacks.len());
    let mut next_id = stacks.iter().flatten().map(|s| s.next_id).max().unwrap_or(1);
    let mut first_done = false;
    for stack in stacks {
        let Some(stack) = stack else {
            maps.push(Vec::new());
            continue;
        };
        let mut map: Vec<Option<usize>> = vec![None; out.layers.len()];
        for (k, layer) in stack.layers.iter().enumerate() {
            let matched = if first_done { out.layers.iter().position(|l| l.row.name == layer.row.name) } else { None };
            match matched {
                Some(j) if map[j].is_none() => map[j] = Some(k),
                _ if out.layers.len() < MOST_PAINT_LAYERS => {
                    let mut row = layer.row.clone();
                    if first_done {
                        // Its own number could be one of the first stack's.
                        row.id = next_id;
                        next_id += 1;
                    }
                    out.layers.push(PaintLayer { row, data: Channels::default() });
                    map.push(Some(k));
                }
                _ => {}
            }
        }
        for &slot in &stack.table {
            if !out.table.contains(&slot) && out.table.len() < MOST_MATERIALS {
                out.table.push(slot);
            }
        }
        first_done = true;
        maps.push(map);
    }
    out.next_id = next_id.max(out.layers.iter().map(|l| l.row.id + 1).max().unwrap_or(1));
    (out, maps)
}

/// Where each vertex's paint comes from: a source, the corners of one of its
/// triangles and their shares. `None`: bare.
type Sampled = Vec<Option<(u32, [u32; 3], [f64; 3])>>;

/// The lists of one layer of the result, for one kind of list (`pick`),
/// each vertex's value worked out from its sample.
struct Build<'a> {
    samples: &'a Sampled,
    /// For each source, its list of this kind for the layer, if it has one.
    lists: Vec<Option<&'a dyn std::any::Any>>,
}

impl Build<'_> {
    fn list<T: Value>(&self, missing: T) -> Option<Vec<T>> {
        let lists: Vec<Option<&Vec<T>>> = self.lists.iter().map(|l| l.and_then(|l| l.downcast_ref::<Vec<T>>())).collect();
        if lists.iter().all(Option::is_none) {
            return None;
        }
        Some(
            self.samples
                .par_iter()
                .with_min_len(PER_TASK)
                .map(|sample| {
                    let Some((k, corners, shares)) = sample else { return missing };
                    let Some(list) = lists[*k as usize] else { return missing };
                    let values = corners.map(|c| list.get(c as usize).copied().unwrap_or(missing));
                    T::weighted(&values, shares)
                })
                .collect(),
        )
    }
}

/// The paint of `stacks` (one per source, `None` for a source without
/// paint) for a mesh whose every vertex is `samples` of them.
fn sampled_stack(stacks: &[Option<&PaintStack>], samples: &Sampled) -> PaintStack {
    let (mut out, maps) = merged(stacks);
    for (j, layer) in out.layers.iter_mut().enumerate() {
        let source_layer = |k: usize| -> Option<&Channels> {
            let index = maps.get(k)?.get(j).copied().flatten()?;
            Some(&stacks[k]?.layers[index].data)
        };
        let pick = |f: &dyn Fn(&Channels) -> Option<&dyn std::any::Any>| -> Build {
            Build { samples, lists: (0..stacks.len()).map(|k| source_layer(k).and_then(f)).collect() }
        };
        layer.data = Channels {
            colour: pick(&|c| c.colour.as_ref().map(|l| l as &dyn std::any::Any)).list(Default::default()),
            material: pick(&|c| c.material.as_ref().map(|l| l as &dyn std::any::Any)).list(Default::default()),
            rough_metal: pick(&|c| c.rough_metal.as_ref().map(|l| l as &dyn std::any::Any)).list(Default::default()),
            mask: pick(&|c| c.mask.as_ref().map(|l| l as &dyn std::any::Any)).list(super::BARE_MASK),
            cavity: pick(&|c| c.cavity.as_ref().map(|l| l as &dyn std::any::Any)).list(super::BARE_MASK),
        };
    }
    out.len = samples.len();
    out
}

/// Gives `new` the paint of the surfaces it was made from, by the nearest
/// point (see the module notes). `new` is changed only if a source has paint
/// and `new` lies on it. A new mesh that has paint already (the routine that
/// built it carried it) is left as it is.
pub fn carry(new: &mut Mesh, sources: &[Source], fit: Fit) -> Carried {
    if new.has_paint() {
        return Carried::Kept;
    }
    let stacks: Vec<Option<std::borrow::Cow<PaintStack>>> = sources.iter().map(|s| s.mesh.paint_stack()).collect();
    if stacks.iter().all(Option::is_none) || new.vertex_count() == 0 {
        return Carried::Nothing;
    }
    let usable: Vec<&Source> = sources.iter().filter(|s| s.mesh.triangle_count() > 0).collect();
    let index_of: Vec<usize> =
        sources.iter().enumerate().filter(|(_, s)| s.mesh.triangle_count() > 0).map(|(k, _)| k).collect();
    let found = nearest_points(&new.positions, &usable);
    let within = (new.bounds().size().length() * fit.on_surface()).powi(2);
    let on = found.iter().filter(|n| n.apart <= within).count();
    if fit == Fit::Whole && on * 2 < new.vertex_count() {
        return Carried::Dropped;
    }
    if on == 0 {
        // A piece that lies on none of the old surface takes nothing from it.
        return Carried::Nothing;
    }
    let samples: Sampled = found
        .par_iter()
        .with_min_len(PER_TASK)
        .map(|n: &Nearest| {
            if n.source == Nearest::NONE || (fit == Fit::Part && n.apart > within) {
                return None;
            }
            let k = index_of[n.source as usize];
            let mesh = sources[k].mesh;
            let i = n.tri as usize * 3;
            let corners = [mesh.indices[i], mesh.indices[i + 1], mesh.indices[i + 2]];
            Some((k as u32, corners, n.bary(mesh).map(|w| w as f64)))
        })
        .collect();
    let refs: Vec<Option<&PaintStack>> = stacks.iter().map(|s| s.as_deref()).collect();
    new.set_paint(Some(sampled_stack(&refs, &samples)));
    Carried::Kept
}

/// The paint of `new` if it is `old` copied out several times over, as an
/// array made real is (`groups::copies` says how that is told): vertex
/// `c * V + j` of what `new` was built from is a copy of `old`'s vertex `j`,
/// and `vertex` says where each of those went in `new`. `false`, and `new`
/// untouched, if `new` is not that or `old` has no paint.
pub fn copies(old: &Mesh, new: &mut Mesh, vertex: &[u32]) -> bool {
    let (v0, all) = (old.vertex_count(), vertex.len());
    if v0 == 0 || all <= v0 || all % v0 != 0 {
        return false;
    }
    let Some(stack) = old.paint_stack() else { return false };
    // The first copy is the mesh as it was, to the bit.
    let first = (0..v0).into_par_iter().all(|j| {
        let at = vertex[j] as usize;
        at < new.vertex_count() && new.positions[at] == old.positions[j]
    });
    if !first {
        return false;
    }
    let repeated: Vec<u32> = (0..all as u32).map(|i| i % v0 as u32).collect();
    let mut out = stack.gathered(&repeated);
    let mut scatter = Scatter(vertex, new.vertex_count());
    for layer in &mut out.layers {
        layer.data = layer.data.mapped(&mut scatter);
    }
    out.len = new.vertex_count();
    new.set_paint(Some(out));
    true
}

/// The paint of a join of plain shells: `parts` one after another, their
/// vertices in order, put in the order `vertex` says (where each vertex of
/// the parts went in the joined mesh of `vertices` vertices).
pub fn joined(parts: &[&Mesh], vertex: &[u32], vertices: usize) -> Option<PaintStack> {
    let stacks: Vec<Option<std::borrow::Cow<PaintStack>>> = parts.iter().map(|m| m.paint_stack()).collect();
    if stacks.iter().all(Option::is_none) {
        return None;
    }
    let mut samples: Sampled = Vec::with_capacity(vertex.len());
    for (k, part) in parts.iter().enumerate() {
        // A vertex is its own corner, whole.
        samples.extend((0..part.vertex_count() as u32).map(|v| Some((k as u32, [v; 3], [1.0, 0.0, 0.0]))));
    }
    let refs: Vec<Option<&PaintStack>> = stacks.iter().map(|s| s.as_deref()).collect();
    let mut out = sampled_stack(&refs, &samples);
    let mut scatter = Scatter(vertex, vertices);
    for layer in &mut out.layers {
        layer.data = layer.data.mapped(&mut scatter);
    }
    out.len = vertices;
    Some(out)
}
