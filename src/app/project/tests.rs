//! Checks of Project Detail as the app runs it: which object it takes from,
//! the reach in the scene, and that the projection is one undo step. The
//! geometry itself is checked where it lives (`project.rs`).

use glam::{Mat4, Vec3};

use super::*;
use crate::mesh::primitives::{self, Primitive};

fn sphere(tris: usize, size: f32) -> Mesh {
    primitives::build(Primitive::Sphere, tris, size)
}

/// A scene of three: the active "Copy", an "Original" where it stands, and a
/// "Far" object off to one side. Gives the scene, the slot holding the
/// active mesh, and the ids of the three.
fn three() -> (Scene, Mesh, [u64; 3]) {
    let mut slot = sphere(2_000, 1.0);
    let mut scene = Scene::new("Copy");
    let copy = scene.active().id;
    let original = scene.add("Original", sphere(20_000, 1.05), &mut slot);
    let far = scene.add("Far", sphere(2_000, 1.0), &mut slot);
    scene.active_mut().position = Vec3::new(10.0, 0.0, 0.0);
    scene.select(copy, &mut slot);
    (scene, slot, [copy, original, far])
}

#[test]
fn the_source_is_the_object_overlapping_the_active_one_most_unless_one_is_named() {
    let (scene, slot, [_, original, far]) = three();
    assert_eq!(source(&scene, &slot, None).map(|o| o.id), Some(original));
    assert_eq!(source(&scene, &slot, Some(far)).map(|o| o.id), Some(far), "a named object is taken");
    assert_eq!(source(&scene, &slot, Some(9_999)).map(|o| o.id), Some(original), "one that has gone is not");
}

#[test]
fn neither_the_object_itself_nor_an_instance_of_it_is_a_source() {
    let (mut scene, mut slot, [copy, original, _]) = three();
    assert!(source(&scene, &slot, Some(copy)).is_some_and(|o| o.id != copy), "the object itself is never taken");
    // Without the original, only the far object is beside the copy, and it does not overlap.
    assert!(scene.remove(original, &mut slot));
    scene.select(copy, &mut slot);
    assert!(source(&scene, &slot, None).is_none(), "nothing overlaps");
    // An instance shares the copy's mesh: the same surface, never a source.
    let instance = scene.instance_active();
    scene.select(copy, &mut slot);
    assert!(source(&scene, &slot, Some(instance)).is_none());
}

#[test]
fn the_reach_is_a_share_of_the_objects_size_in_the_scene() {
    let (mut scene, slot, _) = three();
    let side = scene.bounds_of(scene.active(), &slot).size().max_element();
    assert!((reach(&scene, &slot, 0.02) - side * 0.02).abs() < 1e-6);
    scene.active_mut().scale = Vec3::splat(3.0);
    assert!((reach(&scene, &slot, 0.02) - side * 3.0 * 0.02).abs() < 1e-5, "scaling the object scales the reach");
    assert!((reach(&scene, &slot, f32::NAN) - side * 3.0 * project::DEFAULT_REACH).abs() < 1e-5);
}

#[test]
fn a_projection_is_one_undo_step_that_gives_the_model_back() {
    let mut mesh = sphere(2_000, 1.0);
    let before = mesh.positions.clone();
    let original = sphere(20_000, 1.05);
    let done = project::project(&mesh, Mat4::IDENTITY, &[Source { mesh: &original, to_world: Mat4::IDENTITY }], 0.1);
    assert!(!done.indices.is_empty());
    let mut history = History::default();
    commit(&mut mesh, &mut history, &done);
    assert_eq!(history.steps(), (1, 0));
    assert!(mesh.positions.iter().zip(&before).any(|(a, b)| a != b), "the model moved");
    assert_eq!(history.undo(&mut mesh), Some(false));
    assert_eq!(mesh.positions, before, "and comes back to the bit");
}
