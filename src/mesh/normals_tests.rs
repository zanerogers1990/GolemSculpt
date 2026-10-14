//! Bringing normals up to date after a stroke: what the flush leaves, and
//! what it costs.

use std::time::Instant;

use glam::Vec3;

use crate::mesh::Mesh;
use crate::mesh::primitives::{self, Primitive};

/// Moves every vertex of every fifth leaf (starting at `first`) and tells the
/// mesh, as `positions_changed` asks: every leaf with a triangle on one of
/// them passed with them.
fn move_some(mesh: &mut Mesh, first: u32, by: f32) -> Vec<u32> {
    let mut moved = Vec::new();
    for l in (first..mesh.bvh.leaves.len() as u32).step_by(5) {
        let leaf = mesh.bvh.leaves[l as usize];
        for v in leaf.vert_start..leaf.vert_end {
            let p = mesh.positions[v as usize];
            mesh.positions[v as usize] = p * (1.0 + by * (v as f32 * 0.37 + first as f32).sin());
            moved.push(v);
        }
    }
    let leaves = mesh.leaves_of(&moved);
    mesh.positions_changed(&leaves);
    leaves
}

/// After vertices in some leaves move, the flush leaves every normal exactly
/// as working them all out afresh does: none left stale, none different.
#[test]
fn a_flush_gives_the_normals_a_full_pass_gives_to_the_bit() {
    let mut mesh = primitives::build(Primitive::Sphere, 30_000, 0.5);
    for round in 0..3u32 {
        move_some(&mut mesh, round, 0.05);
        mesh.flush_normals();
        let flushed = mesh.normals.clone();
        mesh.recompute_all_normals();
        let bits = |n: &[Vec3]| n.iter().map(|n| n.to_array().map(f32::to_bits)).collect::<Vec<_>>();
        assert!(bits(&flushed) == bits(&mesh.normals), "round {round}: a normal differs from a full pass");
    }
}

/// What a flush costs after a big dab on five million triangles.
///
/// Kept because two ways of making it cheaper were tried and measured here
/// and neither paid (2026-10-14, one thread: 0.39 ms as it is, 0.41 ms
/// tried): crossing each triangle of a moved leaf once into a buffer for
/// its vertices to add up, instead of once per corner; and working out only
/// the vertices of a neighbouring leaf that sit beside one that moved. The
/// flush is bound by reading the fans and positions, not by the arithmetic.
#[test]
#[ignore]
fn timing_of_a_normals_flush() {
    let mut mesh = primitives::build(Primitive::Sphere, 5_000_000, 0.5);
    for radius in [0.02f32, 0.08] {
        let mut leaves = Vec::new();
        mesh.bvh.leaves_in_sphere(Vec3::new(0.0, 0.0, 0.5), radius, &mut leaves);
        let mut best = f64::MAX;
        for _ in 0..10 {
            mesh.positions_changed(&leaves);
            let t = Instant::now();
            mesh.flush_normals();
            best = best.min(t.elapsed().as_secs_f64() * 1000.0);
        }
        println!("radius {radius}: {} leaves moved, flush {best:.3} ms", leaves.len());
    }
}
