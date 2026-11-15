//! The web version's file readers and writers (src/files.rs).
//! Here rather than in the crate: its own tests would pull in the desktop's.

use glam::Vec3;
use golem_web::files::*;


#[test]
fn obj_and_stl_round_trip() {
    let positions = vec![Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::Z];
    let indices = vec![0, 1, 2, 0, 2, 3, 0, 3, 1, 1, 3, 2];
    let obj = read_obj(&write_obj(&positions, &indices)).unwrap();
    assert_eq!(obj.positions, positions);
    assert_eq!(obj.indices, indices);
    let stl = read_stl(&write_stl(&positions, &indices)).unwrap();
    assert_eq!(stl.positions.len(), 4);
    assert_eq!(stl.indices.len(), 12);
}

#[test]
fn obj_quads_become_two_triangles() {
    let obj = read_obj(b"v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nf 1/1/1 2/2/2 3/3/3 4/4/4\n").unwrap();
    assert_eq!(obj.indices, vec![0, 1, 2, 0, 2, 3]);
}

#[test]
fn ascii_ply_reads() {
    let ply = b"ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nproperty float y\nproperty float z\n\
                element face 1\nproperty list uchar int vertex_indices\nend_header\n0 0 0\n1 0 0\n0 1 0\n3 0 1 2\n";
    let t = read_ply(ply).unwrap();
    assert_eq!(t.positions.len(), 3);
    assert_eq!(t.indices, vec![0, 1, 2]);
}
