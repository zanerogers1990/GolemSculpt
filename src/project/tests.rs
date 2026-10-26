use glam::{Mat4, Quat, Vec3};

use super::*;
use crate::mesh::Mesh;
use crate::mesh::primitives::{self, Primitive};

fn sphere(tris: usize) -> Mesh {
    primitives::build(Primitive::Sphere, tris, 0.5)
}

/// How far every moved point is from the origin, once moved.
fn radii(done: &Projected) -> Vec<f32> {
    done.positions.iter().map(|p| p.length()).collect()
}

#[test]
fn a_coarse_sphere_takes_the_size_of_a_fine_one_around_it() {
    let target = sphere(2_000);
    let fine = sphere(80_000);
    let r = target.positions[0].length();
    // The fine sphere a little larger, by its object's scale.
    let source = Source { mesh: &fine, to_world: Mat4::from_scale(Vec3::splat(1.04)) };
    let done = project(&target, Mat4::IDENTITY, &[source], 0.1);
    assert_eq!(done.indices.len(), target.vertex_count(), "every point found the surface");
    assert_eq!(done.missed, 0);
    for got in radii(&done) {
        assert!((got - r * 1.04).abs() < r * 0.002, "{got} against {}", r * 1.04);
    }
}

#[test]
fn both_objects_are_taken_where_they_stand() {
    let target = sphere(2_000);
    let fine = sphere(40_000);
    let place = Mat4::from_scale_rotation_translation(Vec3::ONE, Quat::from_rotation_y(0.7), Vec3::new(3.0, -1.0, 2.0));
    // The source stands in the same place and is turned the same way; the target's points come back in its own space.
    let source = Source { mesh: &fine, to_world: place * Mat4::from_scale(Vec3::splat(1.02)) };
    let done = project(&target, place, &[source], 0.1);
    let r = target.positions[0].length();
    assert_eq!(done.indices.len(), target.vertex_count());
    for got in radii(&done) {
        assert!((got - r * 1.02).abs() < r * 0.003);
    }
}

#[test]
fn a_source_or_target_placed_as_a_mirror_image_is_still_found() {
    // A live mirror copy's placement is a reflection; its normals still point out.
    let target = sphere(2_000);
    let fine = sphere(40_000);
    let mirror = Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0));
    let r = target.positions[0].length();
    let mirrored = Source { mesh: &fine, to_world: mirror * Mat4::from_scale(Vec3::splat(1.02)) };
    let done = project(&target, Mat4::IDENTITY, &[mirrored], 0.1);
    assert_eq!(done.indices.len(), target.vertex_count(), "{} points missed a mirrored source", done.missed);
    for got in radii(&done) {
        assert!((got - r * 1.02).abs() < r * 0.003);
    }
    // And a mirrored target onto a plain source.
    let plain = Source { mesh: &fine, to_world: Mat4::from_scale(Vec3::splat(1.02)) };
    let done = project(&target, mirror, &[plain], 0.1);
    assert_eq!(done.indices.len(), target.vertex_count(), "{} points of a mirrored target missed", done.missed);
}

#[test]
fn the_mask_protects_and_a_half_mask_goes_half_way() {
    let mut target = sphere(2_000);
    for (v, p) in target.positions.clone().iter().enumerate() {
        target.mask[v] = if p.y > 0.1 { 1.0 } else if p.y < -0.1 { 0.0 } else { 0.5 };
    }
    let fine = sphere(40_000);
    let source = Source { mesh: &fine, to_world: Mat4::from_scale(Vec3::splat(1.1)) };
    let done = project(&target, Mat4::IDENTITY, &[source], 0.2);
    let r = target.positions[0].length();
    for (&v, p) in done.indices.iter().zip(&done.positions) {
        let mask = target.mask[v as usize];
        assert!(mask < 1.0, "a fully masked point is not moved");
        let want = r * (1.0 + 0.1 * (1.0 - mask));
        assert!((p.length() - want).abs() < r * 0.004, "mask {mask}: {} against {want}", p.length());
    }
    assert!(done.indices.len() < target.vertex_count());
}

#[test]
fn nothing_beyond_reach_is_taken() {
    let target = sphere(2_000);
    let fine = sphere(20_000);
    let source = Source { mesh: &fine, to_world: Mat4::from_scale(Vec3::splat(1.3)) };
    let done = project(&target, Mat4::IDENTITY, &[source], 0.05);
    assert!(done.indices.is_empty());
    assert_eq!(done.missed, target.vertex_count());
}

#[test]
fn a_surface_facing_the_other_way_is_never_taken() {
    let target = sphere(2_000);
    // The same sphere a little larger, turned inside out: its outside faces in.
    let mut inside_out = sphere(20_000);
    let mut indices = inside_out.indices.clone();
    indices.chunks_exact_mut(3).for_each(|t| t.swap(1, 2));
    let positions = inside_out.positions.clone();
    inside_out = Mesh::new(positions, indices, None);
    let source = Source { mesh: &inside_out, to_world: Mat4::from_scale(Vec3::splat(1.02)) };
    let done = project(&target, Mat4::IDENTITY, &[source], 0.1);
    assert!(done.indices.is_empty(), "{} points took the wrong side", done.indices.len());
}

#[test]
fn a_bump_on_the_source_comes_through_where_it_is() {
    let target = sphere(8_000);
    let mut bumped = sphere(80_000);
    let r = bumped.positions[0].length();
    for p in &mut bumped.positions {
        // A smooth swelling on top, 5% of the radius high.
        let rise = (p.y / r - 0.8).max(0.0) / 0.2 * 0.05 * r;
        *p += p.normalize() * rise;
    }
    let positions = bumped.positions.clone();
    let indices = bumped.indices.clone();
    let bumped = Mesh::new(positions, indices, None);
    let source = Source { mesh: &bumped, to_world: Mat4::IDENTITY };
    let done = project(&target, Mat4::IDENTITY, &[source], 0.1);
    let top = done.indices.iter().position(|&v| target.positions[v as usize].y > r * 0.995).expect("a point at the top");
    assert!((done.positions[top].length() - r * 1.05).abs() < r * 0.01, "the top rose by the bump");
    let side = done.indices.iter().position(|&v| target.positions[v as usize].y.abs() < r * 0.05).expect("a point at the side");
    assert!((done.positions[side].length() - r).abs() < r * 0.003, "the side stayed");
}

/// The way it is meant to be used: a lumpy sculpt, a quad remesh of it
/// (which loses the lumps' detail), Subdivide twice with a Project after
/// each. The copy ends close to the sculpt everywhere.
#[test]
fn quad_remesh_then_subdivide_and_project_gives_the_sculpt_back() {
    let ball = sphere(120_000);
    let r = ball.positions[0].length();
    let lumps = |p: Vec3| {
        let wave = (p.x * 25.0).sin() * (p.y * 25.0).sin() * (p.z * 25.0).sin();
        p.normalize() * (r + 0.01 * r * wave)
    };
    let positions = ball.positions.iter().map(|&p| lumps(p)).collect();
    let sculpt = Mesh::new(positions, ball.indices.clone(), None);
    let (mut copy, _) = crate::quadremesh::quad_remesh_mesh(&sculpt, 1_500).unwrap();
    let source = || Source { mesh: &sculpt, to_world: Mat4::IDENTITY };
    let worst = |mesh: &Mesh| {
        mesh.positions.iter().map(|&p| (p.length() - lumps(p).length()).abs()).fold(0.0f32, f32::max)
    };
    let before = {
        let (once, _) = crate::mesh::levels::subdivided(&copy, sculpt_multires::Split::Smooth).unwrap();
        let (twice, _) = crate::mesh::levels::subdivided(&once, sculpt_multires::Split::Smooth).unwrap();
        worst(&twice)
    };
    for _ in 0..2 {
        copy = crate::mesh::levels::subdivided(&copy, sculpt_multires::Split::Smooth).unwrap().0;
        let done = project(&copy, Mat4::IDENTITY, &[source()], r * 0.05);
        let mut moved = copy.positions.clone();
        for (&v, &p) in done.indices.iter().zip(&done.positions) {
            moved[v as usize] = p;
        }
        copy.positions = moved;
    }
    let after = worst(&copy);
    assert!(after < before * 0.25, "projected {after}, against {before} without");
    assert!(after < r * 0.002, "within a fifth of a percent of the sculpt: {after}");
}

/// How long a projection of a big model takes:
/// `cargo test --release project_timing -- --ignored --nocapture`.
#[test]
#[ignore]
fn project_timing() {
    let target = sphere(10_000_000);
    let fine = sphere(10_000_000);
    let source = Source { mesh: &fine, to_world: Mat4::from_scale(Vec3::splat(1.01)) };
    let started = std::time::Instant::now();
    let done = project(&target, Mat4::IDENTITY, &[source], 0.05);
    println!("{} points, {} moved, {:?}", target.vertex_count(), done.indices.len(), started.elapsed());
}
