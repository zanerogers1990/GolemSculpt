//! The Remesh card's Smooth, as the app runs it (`smoothed`).

use std::f32::consts::PI;

use glam::Vec3;

use super::*;
use crate::mesh::primitives::{self, Primitive};

/// A lumpy ball with a groove round it, as a remesh at 48 leaves it: what a
/// step up in resolution starts from.
fn coarse_model() -> Mesh {
    let ball = primitives::build(Primitive::Sphere, 200_000, 0.5);
    let across = Vec3::new(0.3, 1.0, 0.2).normalize();
    let positions = ball
        .positions
        .iter()
        .map(|p| {
            let d = p.normalize();
            let mut r = 0.5 + 0.03 * (5.0 * d.x + 0.3).sin() * (4.0 * d.y + 1.1).sin() * (3.0 * d.z + 2.0).sin();
            let s = d.dot(across).clamp(-1.0, 1.0).asin();
            if s.abs() < 0.07 {
                r -= 0.018 * 0.5 * (1.0 + (PI * s / 0.07).cos());
            }
            d * r
        })
        .collect();
    voxel_remesh(&Mesh::new(positions, ball.indices.clone(), None), 48, AS_IT_IS).unwrap()
}

/// How far, on average, each vertex normal turns from the mean of its
/// neighbours', in degrees: the shading noise a step up leaves.
fn roughness(mesh: &Mesh) -> f32 {
    let mut sums = vec![Vec3::ZERO; mesh.positions.len()];
    for t in mesh.indices.chunks_exact(3) {
        for e in 0..3 {
            sums[t[e] as usize] += mesh.normals[t[(e + 1) % 3] as usize];
        }
    }
    let angles: f32 = sums.iter().zip(&mesh.normals).map(|(sum, n)| n.angle_between(sum.normalize()).to_degrees().powi(2)).sum();
    (angles / mesh.positions.len() as f32).sqrt()
}

fn volume(mesh: &Mesh) -> f32 {
    let p = |i: u32| mesh.positions[i as usize];
    mesh.indices.chunks_exact(3).map(|t| p(t[0]).dot(p(t[1]).cross(p(t[2]))) / 6.0).sum()
}

/// What the measurements in `sculpt_volume::smooth` found, in the app: a
/// step up with Smooth is clearly less rough, the same size and shape, and
/// about as many triangles.
#[test]
fn a_step_up_with_smooth_is_smoother_and_the_same_shape() {
    let coarse = Copied::of(&coarse_model());
    let (sharp, _) = coarse.remesh(128, AS_IT_IS, 0).unwrap();
    let (smooth, _) = coarse.remesh(128, AS_IT_IS, DEFAULT_SMOOTH as u32).unwrap();
    let (rough_sharp, rough_smooth) = (roughness(&sharp), roughness(&smooth));
    assert!(rough_smooth < rough_sharp * 0.75, "roughness {rough_smooth} with Smooth, {rough_sharp} without");
    let ratio = volume(&smooth) / volume(&sharp);
    assert!((ratio - 1.0).abs() < 0.01, "volume ratio {ratio}");
    let (a, b) = (sharp.triangle_count() as f32, smooth.triangle_count() as f32);
    assert!((a / b - 1.0).abs() < 0.02, "{a} triangles without Smooth, {b} with");
    let cell = sharp.bounds().size().max_element() / 128.0;
    let grown = (smooth.bounds().size() - sharp.bounds().size()).abs().max_element();
    assert!(grown < 0.5 * cell, "the bounds moved {} cells", grown / cell);
}

#[test]
fn smooth_off_is_the_remesh_as_it_always_was() {
    let model = coarse_model();
    let (off, _) = Copied::of(&model).remesh(96, AS_IT_IS, 0).unwrap();
    let plain = voxel_remesh(&model, 96, AS_IT_IS).unwrap();
    assert_eq!((off.positions, off.indices), (plain.positions, plain.indices));
}

/// A mirrored rebuild comes out exactly symmetric, which smoothing each side
/// on its own would spoil, so it is not smoothed.
#[test]
fn a_mirrored_rebuild_is_never_smoothed() {
    let model = Copied::of(&coarse_model());
    let mirror = [true, false, false];
    let (smoothed, _) = model.remesh(96, mirror, 5).unwrap();
    let (plain, _) = model.remesh(96, mirror, 0).unwrap();
    assert_eq!((smoothed.positions, smoothed.indices), (plain.positions, plain.indices));
}
