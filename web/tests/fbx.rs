//! FBX in and out of the web version (src/fbx.rs: the desktop's own reader
//! and writer). Here rather than in the crate: their own tests would pull in
//! the rest of the desktop's file module.

use glam::{Mat4, Quat, Vec3};
use golem_web::fbx;
use golem_web::mesh::Mesh;
use golem_web::mesh::primitives::{self, Primitive};

#[test]
fn a_saved_fbx_opens_again_with_every_object_where_it_stood() {
    let ball = primitives::build(Primitive::Sphere, 2_000, 1.0);
    let block = primitives::build(Primitive::Cube, 600, 1.0);
    let moved = Mat4::from_scale_rotation_translation(Vec3::splat(2.0), Quat::from_rotation_y(0.5), Vec3::new(5.0, 1.0, -2.0));
    let ball_places = [("Ball", Mat4::IDENTITY), ("Ball_MirrorX", Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0)))];
    let block_places = [("Big_Block", moved)];
    let (bytes, mended) = fbx::write(&[(&ball, &ball_places[..]), (&block, &block_places[..])]).unwrap();
    assert_eq!(mended, 0);
    assert!(bytes.starts_with(b"Kaydara FBX Binary  \x00"), "a binary FBX");

    let (objects, at) = fbx::read(&bytes, None).unwrap();
    let names: Vec<&str> = objects.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(names, ["Ball", "Ball_MirrorX", "Big_Block"]);
    assert_eq!(objects[0].mesh.positions.len(), ball.positions.len());
    assert_eq!(objects[0].mesh.indices.len(), ball.indices.len());
    // Each where it stood: the block's points are its own moved by its place.
    let back = &objects[0].mesh.positions;
    let want = ball.positions.iter().all(|a| back.iter().any(|b| a.distance(*b) < 1e-4));
    assert!(want, "the ball comes back where it was");
    let corner = moved.transform_point3(block.positions[0]);
    assert!(objects[2].mesh.positions.iter().any(|p| p.distance(corner) < 1e-3), "the block comes back moved, turned and sized");
    // The middle of them all goes in the middle of the scene.
    assert!(at.is_finite());
}

#[test]
fn quads_go_out_and_come_back_as_quads() {
    let positions = vec![
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::new(1.0, 1.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(2.0, 0.0, 0.0),
        Vec3::new(2.0, 1.0, 0.0),
    ];
    let indices = vec![0, 1, 2, 0, 2, 3, 1, 4, 5, 1, 5, 2];
    let mut mesh = Mesh::new(positions, indices, None);
    mesh.polys = Some(golem_web::mesh::Polys { start: vec![0, 4, 8], indices: vec![0, 1, 2, 3, 1, 4, 5, 2] });
    let (bytes, _) = fbx::write(&[(&mesh, &[("Strip", Mat4::IDENTITY)][..])]).unwrap();
    let (objects, _) = fbx::read(&bytes, None).unwrap();
    let back = &objects[0].mesh;
    assert_eq!(back.indices.len(), 12);
    assert_eq!(back.polys.as_ref().map(|p| p.start.len()), Some(3), "two quads");
}

#[test]
fn a_bad_file_says_so_rather_than_crashing() {
    assert!(fbx::read(b"not an fbx at all", None).is_err());
    let (bytes, _) = fbx::write(&[(&primitives::build(Primitive::Cube, 200, 1.0), &[("C", Mat4::IDENTITY)][..])]).unwrap();
    for cut in [30, 100, bytes.len() / 2, bytes.len() - 200] {
        assert!(fbx::read(&bytes[..cut], None).is_err(), "cut at {cut}");
    }
    let empty = Mesh::new(Vec::new(), Vec::new(), None);
    assert!(fbx::write(&[(&empty, &[("E", Mat4::IDENTITY)][..])]).is_err());
}
