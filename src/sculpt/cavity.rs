//! A mask made from the surface's own shape: Mask Recesses takes what lies
//! in creases and pits, Mask Peaks what stands proud (ZBrush's Mask By
//! Cavity). With the mask inverted, the brushes then work only in the
//! wrinkles, or only on the high spots.
//!
//! What says which is which is the curvature the mesh already measures for
//! the finishes that pool in recesses and wear on ridges (`Mesh::curv`):
//! the bend of the surface at each vertex against the object's size, less
//! the mesh's own average, so that a plain ball, a flat face and any evenly
//! curved surface are neither, and the same model at twice the size reads
//! the same. It is turned on for the length of this command if nothing else
//! has it on.
//!
//! That measure is taken over a vertex's own ring of neighbours, which on a
//! dense mesh is a tiny patch: the same wrinkle reads sharper the finer the
//! triangles, and every pore reads as a pit. So it is first evened out over
//! a set distance on the surface ([`REACH`], a share of the object's size,
//! however many triangles that is). The same shape then masks the same
//! places whether it has sixty thousand triangles or a million.
//!
//! One control, Amount, says how much of the surface is taken: from only
//! the sharpest creases to anything that lies low at all.

use rayon::prelude::*;

use crate::mesh::{Mesh, StrokeDelta};

/// Which way the surface must lean to be masked.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape {
    /// What lies in creases and pits.
    Recesses,
    /// What stands proud: ridges, bumps, edges.
    Peaks,
}

impl Shape {
    pub fn name(self) -> &'static str {
        match self {
            Shape::Recesses => "recesses",
            Shape::Peaks => "peaks",
        }
    }
}

/// How far over the surface the curvature is evened out before it is read,
/// as a share of the object's half-size: about the width of the finest
/// wrinkle worth masking. Finer than this is taken as texture of the mesh.
const REACH: f32 = 0.012;

/// The most passes the evening-out takes. Each reaches one ring further, and
/// on a mesh so dense that [`REACH`] is more rings than this the result is
/// a little sharper than on a coarser one, which costs less than making a
/// command on ten million vertices wait.
const MAX_PASSES: usize = 64;

/// How far the surface must lean (0 is neutral, 0.5 the most `Mesh::curv`
/// can say) to be half masked: at Amount 0, where only the sharpest creases
/// are taken, and at Amount 1, where anything that leans at all is.
///
/// For scale: a wrinkle a twentieth of the object's half-size wide and
/// three fifths as deep as it is wide leans 0.08 to 0.09 at its bottom, on
/// any mesh fine enough to hold it. The faces of a cube lean 0.003 (its
/// edges pull the mesh's average their way), and at Amount 1 they are still
/// left alone.
const LEAN_AT: (f32, f32) = (0.15, 0.008);

/// Where Amount starts. On a ball worked over with the brushes (wrinkles
/// cut with Dam Standard, a groove, bumps, a rolled ridge) this took the
/// wrinkles, the groove and the foot of every bump and ridge, and for Peaks
/// their tops: what "the recesses" and "the peaks" of a sculpt are taken to
/// mean. At half, only the bottoms of the sharpest wrinkles were masked.
pub const DEFAULT_AMOUNT: f32 = 0.7;

/// The lean at which a vertex is half masked at `amount`. Each equal step
/// of Amount takes in leans as many times shallower: a slider that went
/// evenly from one to the other would spend nearly all its travel on
/// creases nobody could tell apart.
fn half_masked_at(amount: f32) -> f32 {
    let (sharpest, slightest) = LEAN_AT;
    sharpest * (slightest / sharpest).powf(amount.clamp(0.0, 1.0))
}

/// How much a vertex leaning `lean` is masked, with half at `half`: nothing
/// below three fifths of it and all of it from seven fifths up, eased
/// between so the mask has a soft edge.
fn masked(lean: f32, half: f32) -> f32 {
    let t = ((lean - 0.6 * half) / (0.8 * half)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The mean length of the mesh's edges, each counted from both ends.
fn mean_edge(mesh: &Mesh) -> f32 {
    let (sum, count) = (0..mesh.vertex_count() as u32)
        .into_par_iter()
        .with_min_len(4096)
        .map(|v| {
            let p = mesh.positions[v as usize];
            let ring = mesh.neighbours(v);
            (ring.iter().map(|&q| mesh.positions[q as usize].distance(p) as f64).sum::<f64>(), ring.len())
        })
        .reduce(|| (0.0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
    if count == 0 { 0.0 } else { (sum / count as f64) as f32 }
}

/// How many passes of ring averaging even a value out over [`REACH`] on
/// this mesh. A pass spreads a value about two thirds of an edge's length
/// (seven values averaged, six of them an edge away), and passes add up as
/// steps of a random walk do: by the square.
fn passes_for(mesh: &Mesh) -> usize {
    let bounds = mesh.bounds();
    let edge = mean_edge(mesh);
    if bounds.is_empty() || edge <= 0.0 {
        return 0;
    }
    let half = ((bounds.max - bounds.min) * 0.5).max_element();
    let rings = REACH * half / edge;
    ((2.3 * rings * rings).round() as usize).clamp(0, MAX_PASSES)
}

/// `field` with every value replaced by the mean of itself and its ring,
/// `passes` times over. Jacobi: each pass reads the one before whole.
fn evened(mesh: &Mesh, mut field: Vec<f32>, passes: usize) -> Vec<f32> {
    let mut next = vec![0.0; field.len()];
    for _ in 0..passes {
        next.par_iter_mut().enumerate().with_min_len(4096).for_each(|(v, out)| {
            let ring = mesh.neighbours(v as u32);
            let sum: f32 = field[v] + ring.iter().map(|&q| field[q as usize]).sum::<f32>();
            *out = sum / (ring.len() + 1) as f32;
        });
        std::mem::swap(&mut field, &mut next);
    }
    field
}

/// How far every vertex leans the way `shape` asks, 0 for neutral or the
/// other way: the mesh's curvature, evened out over [`REACH`].
fn leans(mesh: &mut Mesh, shape: Shape) -> Vec<f32> {
    // Nothing else pays for curvature it does not read: put it back as found.
    let was_on = !mesh.curv.is_empty();
    mesh.track_curvature(true);
    let field = evened(mesh, mesh.curv.clone(), passes_for(mesh));
    if !was_on {
        mesh.track_curvature(false);
    }
    // `curv` is 0.5 where the surface is neither, above it in a valley.
    let sign = if shape == Shape::Recesses { 1.0 } else { -1.0 };
    field.into_iter().map(|c| ((c - 0.5) * sign).max(0.0)).collect()
}

/// Masks what `shape` asks for, in place of the mask there was, and gives
/// back the undo record: `None` if that changed nothing. Hidden parts keep
/// the mask they had, as they do under every mask command and brush.
pub fn mask(mesh: &mut Mesh, shape: Shape, amount: f32) -> Option<StrokeDelta> {
    if mesh.vertex_count() == 0 {
        return None;
    }
    let leans = leans(mesh, shape);
    let half = half_masked_at(amount);
    mesh.remap_mask(|mesh, v, was| if mesh.is_vertex_hidden(v) { was } else { masked(leans[v as usize], half) })
}

/// The share of the visible surface the mask covers, counting a half-masked
/// vertex as half: what the status line says a mask command took.
pub fn share_masked(mesh: &Mesh) -> f32 {
    let (sum, count) = (0..mesh.vertex_count() as u32)
        .filter(|&v| !mesh.is_vertex_hidden(v))
        .fold((0.0f64, 0usize), |(sum, count), v| (sum + mesh.mask[v as usize] as f64, count + 1));
    if count == 0 { 0.0 } else { (sum / count as f64) as f32 }
}

#[cfg(test)]
mod tests;
