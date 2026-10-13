//! A mesh's sculpt layers (`mesh/layers.rs`) through a change of
//! subdivision level.
//!
//! A layer is an offset for every vertex of the mesh, so it has to be an
//! offset for every vertex of every level. It is kept the way the level
//! stack keeps the model itself: **the finest level's offsets are the layer**,
//! and every coarser level's are their first entries, because a coarse vertex
//! is the fine vertex of the same number. While the mesh is on a coarser
//! level the finest copy of each layer waits here beside the stack
//! (`MeshLevels::finest`), and a round trip with nothing edited hands it back
//! to the bit.
//!
//! What can happen to a layer on a coarser level, and what is done about it
//! when the level is left:
//!
//! - **The model underneath was changed** (strokes with no layer recording, a
//!   gizmo drag). The stack carries that up into the finest level, turning
//!   the finer detail with the surface. The layers' finest offsets are carried
//!   up by the same rule and the same turns (`Levels::riders_up`). On the
//!   level that was edited each offset is what the strokes made there left it
//!   (a push leaves it, a pose turns it: `layers::Ride`), which the mesh's
//!   own layers have kept track of since the level was arrived at
//!   (`Layers::frames`); going up, each layer's finer detail turns as the
//!   positions' did. So a wrinkle layer goes round a bend made on level 1
//!   with the wrinkles themselves, and switching it off afterwards takes
//!   exactly the wrinkles away. (Turning each finest offset by how the
//!   finest surface turned at its own vertex sounds the same and is not:
//!   after a twist it left three times as much of a layer of fine detail
//!   behind. `tests.rs` measures what this leaves.)
//! - **A layer recorded strokes.** What it recorded is in the positions the
//!   stack carried up, mixed with everything else. Its share of the finest
//!   level is found by carrying the level up a second time without it: the
//!   difference between the two results, divided by the layer's strength, is
//!   what the layer added, as the finest level has it (softened and spread by
//!   the levels in between exactly as the positions were). That is added to
//!   the layer's finest offsets. The model underneath then comes out as the
//!   level carried up without the layer, which is what "underneath" means.
//!   (Carrying the offsets up by plain interpolation instead would leave the
//!   difference between that and how the positions were carried in the model
//!   underneath: a dent under every stroke, showing when the layer is turned
//!   down.)
//!
//! Strength, the eye and Delete would have to change the finest positions
//! the stack keeps, by offsets it knows nothing about, so on a coarser level
//! the mesh refuses them (`Mesh::drag_layer`) and the app says to step up.
//! Recording, a new layer, a rename and Bake are fine on any level.
//!
//! The shape after a step is always exactly what the stack returned: nothing
//! here writes a position. Whatever differs between how positions and
//! offsets are carried lands in the model underneath, which is nowhere
//! stored.

use std::borrow::Cow;
use std::sync::Arc;

use glam::{Mat3, Vec3};
use rayon::prelude::*;
use sculpt_multires::{Levels, Rider};

use super::MeshLevels;
use crate::mesh::Mesh;
use crate::mesh::layers::SavedLayers;

/// Vertices handed to one rayon task.
const PER_TASK: usize = 1 << 12;

/// The finest level's copy of each layer's offsets, by the layer's number,
/// in the stack's vertex order. Empty while the mesh is on the finest level.
pub(super) type Finest = Vec<(u32, Arc<[Vec3]>)>;

/// A mesh's layers for the level it is arriving on, each layer's offsets in
/// the stack's order, with the finest copies to keep beside the stack.
pub(super) struct Riding {
    pub layers: SavedLayers,
    pub finest: Finest,
}

fn same(a: Vec3, b: Vec3) -> bool {
    a.to_array().map(f32::to_bits) == b.to_array().map(f32::to_bits)
}

fn all_same(a: &[Vec3], b: &[Vec3]) -> bool {
    a.len() == b.len() && a.par_iter().zip(b).with_min_len(PER_TASK).all(|(a, b)| same(*a, *b))
}

/// The layers of `mesh` on the level Subdivide has just added above its
/// finest. Each layer's offsets go through the same split the positions
/// went through: a split places every vertex at a weighted mean of the old
/// ones, so the split of model-plus-layers is the split of the model plus
/// the split of each layer, and that second part is what the layer adds to
/// the new level. (Carried up as a mask is, without the smoothing, a layer
/// of fine detail would leave the difference in the model underneath: a faint
/// print of itself that stays when it is switched off.)
pub(super) fn subdivided(mesh: &Mesh, levels: &MeshLevels, grown: &Levels) -> Option<Riding> {
    let mut layers = mesh.layers.as_ref()?.saved(Some(&levels.vertex));
    let top = levels.top();
    for layer in &mut layers.layers {
        layer.offsets = grown.points_up(top, &layer.offsets).ok()?;
    }
    Some(Riding { layers, finest: Vec::new() })
}

/// The layers of `mesh` on level `to`, which it is stepping to. `positions`
/// and `mask` are the level it is leaving, in the stack's order; `after` is
/// the stack once the step is taken, and `arrived` the positions it gave for
/// level `to`.
pub(super) fn stepped(
    mesh: &Mesh,
    levels: &MeshLevels,
    positions: &[Vec3],
    mask: &[f32],
    to: usize,
    after: &Levels,
    arrived: &[Vec3],
) -> Option<Riding> {
    let own = mesh.layers.as_ref()?;
    let mut layers = own.saved(Some(&levels.vertex));
    let (from, top) = (levels.current(), levels.top());
    let finest: Vec<Arc<[Vec3]>> = if from == top {
        // Leaving the finest level: what the layers hold now is their finest copy.
        layers.layers.iter_mut().map(|layer| Arc::from(std::mem::take(&mut layer.offsets))).collect()
    } else {
        let vertices = levels.stack.vertex_count(top);
        let kept = |id: u32| levels.finest.iter().find(|(layer, _)| *layer == id).map(|(_, finest)| finest.clone());
        // A layer made on this level has nothing finer yet.
        let kept: Vec<Arc<[Vec3]>> =
            layers.layers.iter().map(|l| kept(l.id).unwrap_or_else(|| vec![Vec3::ZERO; vertices].into())).collect();
        let was = levels.stack.finest_positions()?;
        let now = if to == top { arrived } else { after.finest_positions()? };
        let frames = own.frames(&levels.vertex);
        Carry { old: &levels.stack, from, positions, mask, was, now, frames: frames.as_deref() }.finest(&layers, kept)
    };
    let count = after.vertex_count(to);
    for (layer, finest) in layers.layers.iter_mut().zip(&finest) {
        layer.offsets = finest[..count].to_vec();
    }
    let ids: Vec<u32> = layers.layers.iter().map(|layer| layer.id).collect();
    let finest = if to == top { Vec::new() } else { ids.into_iter().zip(finest).collect() };
    Some(Riding { layers, finest })
}

/// `position` less `share`, a layer's part of it: the model underneath at a
/// vertex the layer recorded on. Where that comes to `was`, the vertex as the
/// level came down, but for the rounding of putting the share on and taking
/// it off again (a few units in the last place of the larger of the two), it
/// is `was` itself. So recording on a layer leaves the model underneath as
/// it was to the bit, and nothing else is carried for a change that is not
/// one.
fn settled(position: Vec3, share: Vec3, was: Vec3) -> Vec3 {
    let under = (position.as_dvec3() - share.as_dvec3()).as_vec3();
    let slack = 4.0 * f32::EPSILON * position.abs().max(share.abs()).max_element();
    if (under - was).abs().max_element() <= slack { was } else { under }
}

/// One step away from a level below the finest: everything the layers'
/// finest copies are carried by.
struct Carry<'a> {
    /// The stack as it was before the step.
    old: &'a Levels,
    from: usize,
    /// The level being left, as it is now, in the stack's order.
    positions: &'a [Vec3],
    mask: &'a [f32],
    /// The finest level's positions before the step and after it.
    was: &'a [Vec3],
    now: &'a [Vec3],
    /// What each vertex's offsets have been carried by on the level being
    /// left, by moves of the model underneath made there (`Layers::frames`),
    /// in the stack's order. `None` if nothing carried them.
    frames: Option<&'a [Mat3]>,
}

impl Carry<'_> {
    /// A layer's offsets on the level being left as it came down with them
    /// (the first entries of its finest copy), carried as moves of the model
    /// underneath made on this level have carried them since.
    fn came<'k>(&self, finest: &'k [Vec3], count: usize) -> Cow<'k, [Vec3]> {
        let Some(frames) = self.frames else { return Cow::Borrowed(&finest[..count]) };
        let carried = |(offset, frame): (&Vec3, &Mat3)| {
            if *frame == Mat3::IDENTITY || *offset == Vec3::ZERO { *offset } else { *frame * *offset }
        };
        Cow::Owned(finest[..count].iter().zip(frames).map(carried).collect())
    }

    /// Whether a layer's offset `now` is the offset it `came` down with:
    /// to the bit, or where moves of the model underneath have carried the
    /// offsets, to within the rounding of working that out a second time.
    fn unchanged(&self, now: Vec3, came: Vec3) -> bool {
        let slack = if self.frames.is_some() { 8.0 * f32::EPSILON * now.abs().max(came.abs()).max_element() } else { 0.0 };
        same(now, came) || (now - came).abs().max_element() <= slack
    }

    /// Each layer's finest offsets after the step, given what they were.
    fn finest(&self, layers: &SavedLayers, kept: Vec<Arc<[Vec3]>>) -> Vec<Arc<[Vec3]>> {
        let count = self.positions.len();
        let came: Vec<Cow<[Vec3]>> = kept.iter().map(|kept| self.came(kept, count)).collect();
        // What each layer recorded on this level: where its offsets differ
        // from what it came down with.
        let recorded: Vec<Option<Vec<Vec3>>> = layers
            .layers
            .iter()
            .zip(&came)
            .map(|(layer, came)| {
                let change = |(now, came): (&Vec3, &Vec3)| if self.unchanged(*now, *came) { Vec3::ZERO } else { *now - *came };
                let recorded: Vec<Vec3> = layer.offsets.iter().zip(came.iter()).map(change).collect();
                recorded.iter().any(|change| *change != Vec3::ZERO).then_some(recorded)
            })
            .collect();
        if recorded.iter().all(Option::is_none) && self.frames.is_none() && all_same(self.was, self.now) {
            return kept;
        }

        // Each recording layer's share of the finest level: what carrying
        // the level up with it gives over carrying it up without. `level`
        // ends as the level with no layer's recording in it: the model
        // underneath's own change, which is what every layer rides.
        let mut level = self.positions.to_vec();
        let mut with = self.now.to_vec();
        let mut shares: Vec<Option<Vec<Vec3>>> = Vec::new();
        for (layer, recorded) in layers.layers.iter().zip(&recorded) {
            let strength = if layer.on { layer.percent as f64 / 100.0 } else { 0.0 };
            let Some(recorded) = recorded else {
                shares.push(None);
                continue;
            };
            if strength <= 0.0 {
                // Not in the shape at all (it cannot have recorded like
                // this; kept for safety): carried up as the mask would be.
                shares.push(self.old.values_up(self.from, self.old.top(), recorded).ok());
                continue;
            }
            for ((p, add), was) in level.iter_mut().zip(recorded).zip(self.was) {
                if *add != Vec3::ZERO {
                    *p = settled(*p, (add.as_dvec3() * strength).as_vec3(), *was);
                }
            }
            let Ok(step) = self.old.clone().step_to(self.old.top(), &level, self.mask) else {
                shares.push(None);
                continue;
            };
            let without = step.mesh.positions;
            let share = |(with, without): (&Vec3, &Vec3)| {
                if same(*with, *without) { Vec3::ZERO } else { ((with.as_dvec3() - without.as_dvec3()) / strength).as_vec3() }
            };
            shares.push(Some(with.par_iter().zip(&without).with_min_len(PER_TASK).map(share).collect()));
            with = without;
        }

        let add = |(finest, share): (Arc<[Vec3]>, Option<Vec<Vec3>>)| -> Arc<[Vec3]> {
            let Some(share) = share else { return finest };
            finest.iter().zip(&share).map(|(offset, add)| *offset + *add).collect()
        };
        self.ridden(&level, &kept, &came).into_iter().zip(shares).map(add).collect()
    }

    /// The layers' finest offsets after the model underneath went from what
    /// the stack holds to `level` on the level being left: carried up with
    /// the positions (`Levels::riders_up`), from what each layer's offsets
    /// are on this level now (`came`). A layer the change does not reach is
    /// handed back as it came, the same list.
    fn ridden(&self, level: &[Vec3], kept: &[Arc<[Vec3]>], came: &[Cow<[Vec3]>]) -> Vec<Arc<[Vec3]>> {
        if self.frames.is_none() && all_same(level, &self.was[..level.len()]) {
            return kept.to_vec();
        }
        // An empty layer has nothing to carry.
        let riding: Vec<usize> = (0..kept.len()).filter(|&k| kept[k].iter().any(|o| *o != Vec3::ZERO)).collect();
        let riders: Vec<Rider> = riding.iter().map(|&k| Rider { finest: &kept[k], level: &came[k] }).collect();
        let Ok(carried) = self.old.riders_up(level, &riders) else { return kept.to_vec() };
        let mut after = kept.to_vec();
        for (k, carried) in riding.into_iter().zip(carried) {
            if !all_same(&carried, &kept[k]) {
                after[k] = carried.into();
            }
        }
        after
    }
}

#[cfg(test)]
mod tests;
