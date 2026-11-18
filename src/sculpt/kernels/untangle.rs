//! Smooth takes out dimples: the little dark stars a sculpt gathers where a
//! few vertices have bunched up on one another and their tiny triangles
//! have turned to face every which way (seen on Zane's sculpts, 2026-11-16:
//! knots of three or four vertices, triangles a hundredth the size of their
//! neighbours, tipped 50 to 70 degrees off the surface, some folded over).
//!
//! Smoothing proper cannot take one out. It moves each vertex to the middle
//! of its neighbours, and a knot's vertices are mostly each other's
//! neighbours: the knot drifts about as a whole, as crumpled as before, and
//! a stroke of Smooth passes over it and leaves the star where it was.
//!
//! So before a Smooth dab does what it always has, a vertex the dab reaches
//! that is in such a knot ([`crumpled`]) is put in the middle of its
//! neighbours by itself, outright, a few times over, which lays the knot
//! down flat in the surface round it. Only those: on a mesh without knots
//! nothing is moved here at all, and Smooth is to the bit what it was. A
//! groove's sharp floor is not a knot (its triangles are the size of the
//! rest), nor is a pit or a bump (Smooth takes those out already), so
//! neither is touched here.

use glam::Vec3;
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};

use super::{Dab, Params, Ring, Scratch, facing, fill};
use crate::mesh::Mesh;

/// How many times a dab lays the knots under it down, at most. One more
/// pass only happens while a pass still found a knot, so a clean mesh costs
/// one look.
const PASSES: usize = 4;

/// A triangle turned this far from the surface round its vertex (the cosine)
/// is crumpled, if it is also tiny or a sliver.
const TURNED: f32 = 0.5;

/// Tiny: this share of the area a triangle of the vertex's own edge length
/// would have.
const TINY: f32 = 0.1;

/// A sliver: its sharpest corner under this many degrees.
const SLIVER_DEGREES: f32 = 12.0;

/// Whether `v` is in a knot: one of its triangles is folded over against
/// the surface round it, or is tiny or a sliver and turned well away from
/// it. Only a vertex inside the surface: a rim or a held vertex keeps its
/// place as smoothing always leaves it.
#[cfg(test)]
pub(in crate::sculpt) fn crumpled(mesh: &Mesh, v: u32) -> bool {
    knot(mesh, v).is_some()
}

/// [`crumpled`], with the way the surface round a knotted vertex faces.
///
/// That, and not the vertex's own normal, is what says whether the brush
/// can reach it: in a knot the vertex normal points anywhere, often away
/// from the camera, and a brush with Front Facing on then left the very
/// vertices that make the dimple alone. (That more than anything is why
/// Smooth passed over dimples.)
fn knot(mesh: &Mesh, v: u32) -> Option<Vec3> {
    if Ring::of(mesh, v) != Ring::All {
        return None;
    }
    let rings = mesh.rings();
    let tris = rings.vert_tris(v);
    let ring = rings.neighbours(v);
    let at = mesh.positions[v as usize];
    // The surface round the vertex: its triangles' normals, by their area.
    let mut around = Vec3::ZERO;
    let mut each = [(Vec3::ZERO, [Vec3::ZERO; 3]); 24];
    let count = tris.len().min(each.len());
    for (k, &t) in tris.iter().take(count).enumerate() {
        let i = t as usize * 3;
        let c = [0, 1, 2].map(|j| mesh.positions[rings.indices()[i + j] as usize]);
        let n = (c[1] - c[0]).cross(c[2] - c[0]);
        around += n;
        each[k] = (n, c);
    }
    let Some(around) = around.try_normalize() else {
        // Triangles that cancel out: as crumpled as can be. Its neighbours
        // say which way the surface there faces.
        let theirs = ring.iter().map(|&o| mesh.normals[o as usize]).sum::<Vec3>();
        return (!tris.is_empty()).then(|| theirs.normalize_or_zero());
    };
    let edge2 = ring.iter().map(|&o| mesh.positions[o as usize].distance_squared(at)).sum::<f32>() / ring.len().max(1) as f32;
    // A triangle with sides of the mean edge length: sqrt(3)/4 of its square.
    let tiny = TINY * 0.433 * edge2;
    let sliver = SLIVER_DEGREES.to_radians().cos();
    let knotted = each[..count].iter().any(|&(n, c)| {
        let len = n.length();
        if len <= 0.0 {
            return true;
        }
        let facing = n.dot(around) / len;
        if facing < 0.0 {
            return true;
        }
        if facing >= TURNED {
            return false;
        }
        if 0.5 * len < tiny {
            return true;
        }
        // Sharpest corner: the largest cosine of the three.
        (0..3).any(|j| {
            let (p, q, r) = (c[j], c[(j + 1) % 3], c[(j + 2) % 3]);
            (q - p).normalize_or_zero().dot((r - p).normalize_or_zero()) > sliver
        })
    });
    knotted.then_some(around)
}

/// Tests switch the untangling off, to compare with Smooth as it was.
#[cfg(test)]
pub(in crate::sculpt) static OFF: AtomicBool = AtomicBool::new(false);

/// Lays down the knots inside the dab's ring: not masked, not hidden, and
/// (with Front Facing on) on the side of the surface the camera sees, going
/// by the surface round each knot.
pub(super) fn untangle(mesh: &mut Mesh, p: &Params, d: &Dab, leaves: &[u32], scratch: &mut Scratch) {
    #[cfg(test)]
    if OFF.load(Ordering::SeqCst) {
        return;
    }
    let r2 = p.radius * p.radius;
    let Scratch { knots, starts, .. } = scratch;
    for _ in 0..PASSES {
        // None for a vertex left alone (and a hidden one, which `fill` skips).
        let any = {
            let mesh = &*mesh;
            fill(mesh, leaves, starts, knots, |_, v| {
                let i = v as usize;
                let pos = mesh.positions[i];
                if pos.distance_squared(d.center) >= r2 || mesh.mask[i] >= 1.0 {
                    return None;
                }
                let around = knot(mesh, v)?;
                if facing(p.front_facing, d.sided(around), pos, d.cam) <= 0.0 {
                    return None;
                }
                let ring = mesh.neighbours(v);
                Some(ring.iter().map(|&o| mesh.positions[o as usize]).sum::<Vec3>() / ring.len() as f32)
            });
            knots.iter().any(Option::is_some)
        };
        if !any {
            return;
        }
        let knots = &*knots;
        mesh.move_vertices(leaves, |at, _| knots[at]);
    }
}

#[cfg(test)]
mod tests;
