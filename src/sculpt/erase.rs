//! Erase Layer: rubs a sculpt layer out under the ring (ZBrush's Morph brush
//! used against a layer, Nomad's layer eraser). Wrinkles taken off an eyelid
//! and kept everywhere else, without touching the model underneath or any
//! other layer.
//!
//! A sculpt layer is an offset for every vertex, and the shape is the model
//! underneath plus each layer's offset times its strength (`mesh/layers.rs`).
//! The layer this brush works on is the one that is RECORDING, the same one
//! every other brush would be adding to.
//!
//! **It moves the surface and writes no offset.** Each dab eases the vertices
//! under it toward where they would stand without the layer: where the stroke
//! found them, less the layer's strength times its offset there. When the
//! stroke ends it is recorded into the layer like any other stroke
//! (`Mesh::settle_layers`): what it moved, divided by the layer's strength,
//! is added to the offsets. A vertex moved by minus the strength times its
//! offset has that offset come to nothing. So the layer forgets what was
//! rubbed out by the path that already keeps layers and positions in step,
//! and undo, redo, files and the subdivision levels have nothing new to
//! learn. Writing the offsets here as well was the other way, and it would
//! have been a second place for the shape and the layers to disagree in.
//!
//! **The place a vertex is eased to is fixed for the whole stroke.** The
//! offsets change only when a stroke ends, and the place is measured from
//! where the stroke found the vertex, not from where the last dab left it.
//! So however many dabs pass over a vertex, and in whatever order, each
//! closes a share of what is left of one gap and none goes past it
//! (`Sharing::Seeks`): a second pass finishes what the first began, and a
//! stroke down a mirror plane is as strong as the same stroke beside it.
//!
//! **Rounding.** A position is a float, so "without the layer" is the
//! nearest float to it, and what the layer keeps of a vertex rubbed right
//! out is that rounding divided by the layer's strength: at most half a unit
//! in the last place of the position at 100%, a hundred times that at 1%,
//! which on a model a metre across is a few thousandths of a millimetre if
//! the slider is then raised to 100%. `tests.rs` measures it.
//!
//! **No polarity.** Ctrl and the right button erase as the left does. The
//! opposite of erasing is putting back, and what was rubbed out is in no
//! offset any more: Undo has it. (Making the layer stronger under the ring
//! instead has no place to stop at, so it would be another brush, one that
//! adds, and was left out.)
//!
//! **Nothing to erase.** With no layer recording, or the one that records
//! hidden or at 0%, the stroke has no layer ([`Erased::of`]) and every dab
//! leaves the mesh alone, so no undo step is made. The app does not let it
//! get that far: it refuses the press and says what to do ([`refusal`]).

use glam::Vec3;

use super::kernels::{self, Dab, Params, Scratch};
use crate::mesh::Mesh;

#[cfg(test)]
mod tests;

/// The layer a stroke of Erase Layer rubs out: the one that was recording
/// when the stroke began, and how much of its offsets is in the shape.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Erased {
    layer: u32,
    /// As `mesh/layers.rs` works it out: whole percent over a hundred.
    strength: f64,
}

impl Erased {
    /// The layer a stroke beginning on `mesh` now would rub out: the one
    /// that records, if it shows. `None` leaves the brush nothing to do.
    pub fn of(mesh: &Mesh) -> Option<Erased> {
        let row = mesh.layers.as_ref()?.recording()?;
        (row.on && row.percent > 0).then(|| Erased { layer: row.id, strength: row.percent as f64 / 100.0 })
    }

    /// The layer's offsets, one for every vertex of `mesh`.
    fn offsets<'a>(&self, mesh: &'a Mesh) -> Option<&'a [Vec3]> {
        mesh.layers.as_ref()?.offsets(self.layer).filter(|offsets| offsets.len() == mesh.positions.len())
    }

    /// Where a vertex the stroke found at `start`, with the layer's `offset`
    /// there, would stand without the layer. Worked out in double and
    /// rounded once; a vertex the layer does not move is where it was, to
    /// the bit.
    #[inline]
    fn without(&self, start: Vec3, offset: Vec3) -> Vec3 {
        if offset == Vec3::ZERO { start } else { (start.as_dvec3() - offset.as_dvec3() * self.strength).as_vec3() }
    }
}

/// Why a stroke of Erase Layer begun on `mesh` now would do nothing, in
/// words for the status line: there is no layer for it to rub out.
pub fn refusal(mesh: &Mesh) -> Option<String> {
    if Erased::of(mesh).is_some() {
        return None;
    }
    let recording = mesh.layers.as_ref().and_then(|layers| layers.recording());
    Some(match recording {
        Some(row) => format!(
            "Erase Layer rubs out the layer that is recording, and \"{}\" is {}. Show it and raise its Strength first.",
            row.name,
            if row.on { "at 0%" } else { "hidden" }
        ),
        None if mesh.layer_count() == 0 => "Erase Layer rubs a sculpt layer out under the brush, and this object has no \
                                            layers. They are made on the Mesh tab, in the Layers card."
            .to_owned(),
        None => "Erase Layer rubs out the layer that is recording, and none is. Pick the layer to erase: click its dot \
                 in the Layers card (Mesh tab)."
            .to_owned(),
    })
}

/// `from`, a share `k` of the way to `target`, and on it exactly when the
/// share is all of the way: a layer rubbed right out must not keep the last
/// bit of a sum.
#[inline]
fn eased(from: Vec3, target: Vec3, k: f32) -> Vec3 {
    if k >= 1.0 { target } else { from + (target - from) * k }
}

/// One dab: every vertex under it goes a share of what is left of the way to
/// where it would stand without the stroke's layer, by the brush's falloff,
/// its Strength and the pen, and by its alpha if it has one (the layer is
/// then rubbed out through the picture).
///
/// With Drag to Place the stroke is one print, made afresh on the surface
/// as the stroke found it each time the drag resizes it (`alpha::Print`): it
/// is measured from there, and takes the print's share in one go.
pub(super) fn dab(mesh: &mut Mesh, p: &Params, d: &Dab, leaves: &[u32], scratch: &mut Scratch) {
    let Some(erased) = p.erase else { return };
    {
        // The offsets are the mesh's, and the mesh is about to be written:
        // where each vertex is going is read off first.
        let mesh = &*mesh;
        let Some(offsets) = erased.offsets(mesh) else { return };
        kernels::fill(mesh, leaves, &mut scratch.starts, &mut scratch.targets, |_, v| {
            erased.without(mesh.start_pos(v), offsets[v as usize])
        });
    }
    let (r, targets, placed) = (p.radius, &scratch.targets, p.prints());
    let r2 = r * r;
    let amount = if placed { p.strength } else { p.e(d.pressure) };
    mesh.move_vertices(leaves, |at, vt| {
        let from = if placed { vt.start } else { vt.pos };
        let d2 = from.distance_squared(d.center);
        if d2 >= r2 {
            return None;
        }
        let w = p.falloff.at(d2.sqrt() / r, kernels::smooth)
            * (1.0 - vt.mask)
            * kernels::facing(p.front_facing, vt.normal, from, d.cam)
            * p.printed(d, vt.start - d.center);
        if w <= 0.0 {
            return None;
        }
        let k = (w * amount).clamp(0.0, 1.0);
        Some(eased(from, targets[at], if placed { k } else { p.share(k) }))
    });
}
