//! Softening and hardening a mask's edge: what a tap and an Alt tap on the
//! model do while mask painting (app/mask_gestures.rs).

use rayon::prelude::*;

use super::{Mesh, StrokeDelta};

/// How much one hardening steepens the mask's edge.
const HARDEN: f32 = 1.6;

/// One tap's softening: the mask is blurred over its edge, as a few
/// hundredths of the model's width, whatever the mesh's density. Only the
/// band where the mask changes is worked, so a dense mesh with a short edge
/// is quick. One undo step.
impl Mesh {
    pub fn soften_mask(&mut self) -> Option<StrokeDelta> {
        let mesh = &*self;
    let count = mesh.mask.len();
    // Blur reaches about the square root of its passes in rings, and a mesh
    // is about the square root of its points across: passes that grow with
    // the point count keep the reach a like share of the model.
    let passes = (count / 20_000).clamp(3, 250);
    let start = mesh.mask.clone();
    let mut mask = start.clone();
    let differs = |mask: &[f32], v: u32| {
        let own = mask[v as usize];
        mesh.neighbours(v).iter().any(|&o| (mask[o as usize] - own).abs() > 1e-5)
    };
    let mut active: Vec<u32> = (0..count as u32).into_par_iter().filter(|&v| differs(&mask, v)).collect();
    if active.is_empty() {
        return None;
    }
    let mut in_band = vec![false; count];
    for &v in &active {
        in_band[v as usize] = true;
    }
    for _ in 0..passes {
        let new: Vec<f32> = active
            .par_iter()
            .map(|&v| {
                let ring = mesh.neighbours(v);
                let own = mask[v as usize];
                if ring.is_empty() {
                    return own;
                }
                let mean = ring.iter().map(|&o| mask[o as usize]).sum::<f32>() / ring.len() as f32;
                0.5 * (own + mean)
            })
            .collect();
        let mut grown = Vec::new();
        for (&v, &m) in active.iter().zip(&new) {
            let moved = (m - mask[v as usize]).abs() > 1e-4;
            mask[v as usize] = m;
            // Points next to one that moved are worked from the next pass on;
            // far tails that move by less than a ten-thousandth are left.
            if moved {
                for &o in mesh.neighbours(v) {
                    if !in_band[o as usize] {
                        in_band[o as usize] = true;
                        grown.push(o);
                    }
                }
            }
        }
        active.extend(grown);
    }
    let verts: Vec<u32> = (0..count as u32).filter(|&v| mask[v as usize] != start[v as usize]).collect();
    if verts.is_empty() {
        return None;
    }
    let before = verts.iter().map(|&v| start[v as usize]).collect();
    let after = verts.iter().map(|&v| mask[v as usize]).collect();
    self.mask = mask;
    self.dirty_mask.set_all(self.bvh.leaves.len());
    Some(StrokeDelta::Mask { verts, before, after })
}

/// One Alt tap's hardening: the mask's edge steepens about its middle, so the
/// band a softening widened narrows again. Fully masked and free stay so.
pub fn harden_mask(&mut self) -> Option<StrokeDelta> {
    self.remap_mask(|_, _, m| (m - 0.5) * HARDEN + 0.5)
}
}

#[cfg(test)]
mod tests {
    use super::super::primitives::{build, Primitive};

    /// Points partly masked: the soft band between masked and free.
    fn soft_count(mask: &[f32]) -> usize {
        mask.iter().filter(|&&m| m > 0.01 && m < 0.99).count()
    }

    #[test]
    fn a_tap_widens_the_edge_and_an_alt_tap_narrows_it() {
        let mut mesh = build(Primitive::Sphere, 40_000, 1.0);
        let top: Vec<bool> = mesh.positions.iter().map(|p| p.y > 0.0).collect();
        for (m, &t) in mesh.mask.iter_mut().zip(&top) {
            *m = if t { 1.0 } else { 0.0 };
        }
        let hard = soft_count(&mesh.mask);
        assert!(mesh.soften_mask().is_some());
        let soft = soft_count(&mesh.mask);
        assert!(soft > hard + 100, "softening widened the edge: {hard} -> {soft}");
        // Far from the edge nothing changed.
        for (p, &m) in mesh.positions.iter().zip(&mesh.mask) {
            if p.y > 0.6 {
                assert_eq!(m, 1.0);
            }
            if p.y < -0.6 {
                assert_eq!(m, 0.0);
            }
        }
        let softer = { mesh.soften_mask(); soft_count(&mesh.mask) };
        assert!(softer > soft, "a second tap softens more: {soft} -> {softer}");
        for _ in 0..4 {
            mesh.harden_mask();
        }
        assert!(soft_count(&mesh.mask) < softer, "hardening narrowed it");
    }
}
