//! The checks that stand between a bad table and a write past the end of an array.

use super::*;
use crate::mesh::primitives::{self, Primitive};

fn ball_with_curvature() -> Mesh {
    let mut mesh = primitives::build(Primitive::Sphere, 2_000, 0.5);
    mesh.track_curvature(true);
    mesh
}

#[test]
fn curvature_updates_when_its_arrays_match_the_mesh() {
    let mut mesh = ball_with_curvature();
    let leaves: Vec<u32> = (0..mesh.bvh.leaves.len() as u32).collect();
    mesh.update_curvature(&leaves);
    assert_eq!((mesh.curv.len(), mesh.curv_raw.len()), (mesh.positions.len(), mesh.positions.len()));
}

#[test]
#[should_panic(expected = "curvature arrays")]
fn curvature_refuses_to_write_into_arrays_shorter_than_the_mesh() {
    let mut mesh = ball_with_curvature();
    // Spare room, so that a write wrongly let through stays inside memory the vector owns.
    mesh.curv.reserve(64);
    mesh.curv_raw.reserve(64);
    mesh.curv.truncate(mesh.positions.len() - 5);
    let leaves: Vec<u32> = (0..mesh.bvh.leaves.len() as u32).collect();
    mesh.update_curvature(&leaves);
}
