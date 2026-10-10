//! Reflecting a mesh: the maths shared by live mirror copies and by baking them
//! into real objects, and a plain clone, which is the same call with every sign
//! +1.
//!
//! A baked copy is an exact mirror in the mesh's LOCAL space: each vertex and
//! normal is reflected through the object's own origin (or any centre), and the
//! transform that places the copy is reflected to match (`reflect_rotation`).
//! Normals are carried across rather than recalculated: a mesh with duplicated
//! seam vertices, as primitive spheres have along their seam and at the poles,
//! averages each duplicate over only its own triangles when recalculated, so
//! every seam of the copy would shade as a hard crease the original does not
//! have.
//!
//! Winding: a reflection reverses handedness, so an odd number of mirrored axes
//! also reverses every triangle, and the outside stays outside. That holds
//! whichever way the app winds its faces (clockwise in Unity, counter-clockwise
//! here); only the matching swap of two corners matters, and it is the LAST two
//! so each triangle keeps its first corner. A vertex normal pass that takes a
//! face's edges from that corner then sums exactly negated terms on both halves,
//! and the halves' normals stay exact mirror images through every later stroke.

use glam::{Quat, Vec3};
use rayon::prelude::*;

use crate::mesh::TriMesh;

/// Below this many vertices a reflection is not worth splitting across threads.
const PARALLEL_MIN: usize = 1 << 14;

/// -1 on each mirrored axis and +1 on the rest. Scaling a local point or normal
/// by this reflects it through the origin on exactly those axes.
pub fn axis_signs(x: bool, y: bool, z: bool) -> Vec3 {
    let flip = |on: bool| if on { -1.0 } else { 1.0 };
    Vec3::new(flip(x), flip(y), flip(z))
}

/// Whether reflecting by `signs` reverses handedness, and with it every
/// triangle's winding: an odd number of mirrored axes.
pub fn flips_winding(signs: Vec3) -> bool {
    signs.x * signs.y * signs.z <= 0.0
}

pub fn reflect_point(point: Vec3, center: Vec3, signs: Vec3) -> Vec3 {
    center + (point - center) * signs
}

/// The rotation that makes a copy whose LOCAL geometry is reflected by `signs`
/// render as the world-space reflection of the original. With F the reflection,
/// that rotation is F R F: conjugating by F carries the rotation axis u to F u,
/// and reverses the turn when F is improper (an odd number of axes), which
/// together fold into the quaternion as `det(F) * F * xyz`. The result is always
/// a proper rotation, so no negative scale ever reaches a transform, and scale
/// needs no change at all, because a diagonal scale commutes with F.
pub fn reflect_rotation(rotation: Quat, signs: Vec3) -> Quat {
    let det = signs.x * signs.y * signs.z;
    Quat::from_xyzw(det * signs.x * rotation.x, det * signs.y * rotation.y, det * signs.z * rotation.z, rotation.w)
}

/// What a reflection produces.
#[derive(Clone, Debug, PartialEq)]
pub struct Reflected {
    pub positions: Vec<Vec3>,
    /// `None` when none were given, or when they did not match the vertices in
    /// number, for the caller to recalculate.
    pub normals: Option<Vec<Vec3>>,
    pub indices: Vec<u32>,
}

/// Reflects arrays through the origin by scaling with `signs` (each +1 or -1),
/// into fresh ones, reversing every triangle's winding when the reflection is
/// improper (see the module notes). With all signs +1 this is a plain clone.
pub fn reflect_geometry(positions: &[Vec3], normals: Option<&[Vec3]>, indices: &[u32], signs: Vec3) -> Reflected {
    let scale = |v: &[Vec3]| -> Vec<Vec3> {
        if v.len() < PARALLEL_MIN {
            v.iter().map(|&p| p * signs).collect()
        } else {
            v.par_iter().map(|&p| p * signs).collect()
        }
    };
    let normals = normals.filter(|n| n.len() == positions.len()).map(scale);
    let mut indices = indices.to_vec();
    if flips_winding(signs) {
        flip_winding(&mut indices);
    }
    Reflected { positions: scale(positions), normals, indices }
}

/// A copy of `mesh` reflected through the origin by `signs`.
pub fn reflect_mesh(mesh: &TriMesh, signs: Vec3) -> TriMesh {
    let r = reflect_geometry(&mesh.positions, None, &mesh.indices, signs);
    TriMesh::new(r.positions, r.indices)
}

/// A copy of `mesh` reflected across the plane through `point` with normal
/// `normal` (need not be unit length; a zero normal gives a plain copy), for a
/// mirror plane that is not a coordinate plane or does not pass through the
/// origin. Always winds the copy the other way round.
pub fn reflect_across_plane(mesh: &TriMesh, point: Vec3, normal: Vec3) -> TriMesh {
    let n = normal.normalize_or_zero();
    if n == Vec3::ZERO {
        return mesh.clone();
    }
    let reflect = |p: Vec3| p - n * (2.0 * (p - point).dot(n));
    let positions = if mesh.positions.len() < PARALLEL_MIN {
        mesh.positions.iter().map(|&p| reflect(p)).collect()
    } else {
        mesh.positions.par_iter().map(|&p| reflect(p)).collect()
    };
    let mut indices = mesh.indices.clone();
    flip_winding(&mut indices);
    TriMesh::new(positions, indices)
}

/// Swaps the last two corners of every triangle.
fn flip_winding(indices: &mut [u32]) {
    for t in indices.chunks_exact_mut(3) {
        t.swap(1, 2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adjacency::{MeshAdjacency, MeshEdges};
    use crate::testing::{self, Rng};

    fn assert_outward_and_closed(mesh: &TriMesh) {
        let edges = MeshEdges::of(mesh, &MeshAdjacency::of(mesh));
        assert!(edges.is_closed() && edges.is_consistently_wound(&mesh.indices));
        assert!(mesh.signed_volume() > 0.0, "turned inside out: {}", mesh.signed_volume());
    }

    #[test]
    fn signs_name_the_mirrored_axes() {
        assert_eq!(axis_signs(true, false, true), Vec3::new(-1.0, 1.0, -1.0));
        assert!(!flips_winding(Vec3::ONE));
        assert!(flips_winding(axis_signs(true, false, false)));
        assert!(!flips_winding(axis_signs(true, true, false)));
        assert!(flips_winding(axis_signs(true, true, true)));
        assert_eq!(reflect_point(Vec3::new(3.0, 1.0, 2.0), Vec3::new(1.0, 1.0, 1.0), axis_signs(true, false, false)),
            Vec3::new(-1.0, 1.0, 2.0));
    }

    #[test]
    fn every_axis_combination_keeps_a_closed_mesh_closed_and_outward() {
        let cube = testing::cube();
        let sphere = testing::uv_sphere(8, 12);
        for mask in 0..8u32 {
            let signs = axis_signs(mask & 1 != 0, mask & 2 != 0, mask & 4 != 0);
            for mesh in [&cube, &sphere] {
                let mirrored = reflect_mesh(mesh, signs);
                assert_outward_and_closed(&mirrored);
                assert!((mirrored.signed_volume() - mesh.signed_volume()).abs() < 1e-5);
                for (a, b) in mirrored.positions.iter().zip(&mesh.positions) {
                    assert_eq!(*a, *b * signs);
                }
                // Reflecting back restores the original exactly.
                assert_eq!(reflect_mesh(&mirrored, signs), *mesh);
            }
        }
    }

    #[test]
    fn plain_clone_changes_nothing_and_normals_follow_the_signs() {
        let sphere = testing::uv_sphere(6, 8);
        let normals: Vec<Vec3> = sphere.positions.iter().map(|p| p.normalize()).collect();
        let clone = reflect_geometry(&sphere.positions, Some(&normals), &sphere.indices, Vec3::ONE);
        assert_eq!(clone.positions, sphere.positions);
        assert_eq!(clone.indices, sphere.indices);
        assert_eq!(clone.normals.as_deref(), Some(&normals[..]));

        let signs = axis_signs(true, false, true);
        let mirrored = reflect_geometry(&sphere.positions, Some(&normals), &sphere.indices, signs);
        let got = mirrored.normals.unwrap();
        assert!(got.iter().zip(&normals).all(|(g, n)| *g == *n * signs));
        // A normals array of the wrong length is dropped, not guessed at.
        assert!(reflect_geometry(&sphere.positions, Some(&normals[1..]), &sphere.indices, signs).normals.is_none());
        assert!(reflect_geometry(&sphere.positions, None, &sphere.indices, signs).normals.is_none());
    }

    #[test]
    fn a_triangle_keeps_its_first_corner_when_flipped() {
        let r = reflect_geometry(&[Vec3::ZERO, Vec3::X, Vec3::Y], None, &[0, 1, 2, 2, 1, 0], axis_signs(true, false, false));
        assert_eq!(r.indices, [0, 2, 1, 2, 0, 1]);
    }

    #[test]
    fn large_meshes_reflect_the_same_through_the_parallel_path() {
        let big = testing::uv_sphere(200, 200);
        assert!(big.vertex_count() >= PARALLEL_MIN);
        let signs = axis_signs(false, true, true);
        let r = reflect_mesh(&big, signs);
        assert!(r.positions.iter().zip(&big.positions).all(|(a, b)| *a == *b * signs));
        assert!(r.signed_volume() > 0.0);
    }

    /// The reflected rotation must turn the reflected object exactly as the original is turned:
    /// F (R p) = R' (F p).
    #[test]
    fn reflected_rotation_turns_the_mirrored_copy_the_mirrored_way() {
        let mut rng = Rng(5);
        for mask in 0..8u32 {
            let signs = axis_signs(mask & 1 != 0, mask & 2 != 0, mask & 4 != 0);
            for _ in 0..50 {
                let axis = Vec3::new(rng.next() as f32 - 0.5, rng.next() as f32 - 0.5, rng.next() as f32 - 0.5).normalize();
                let rotation = Quat::from_axis_angle(axis, rng.next() as f32 * 6.0);
                let p = Vec3::new(rng.next() as f32, rng.next() as f32, rng.next() as f32);
                let reflected = reflect_rotation(rotation, signs);
                assert!((reflected.length() - 1.0).abs() < 1e-5, "stays a proper rotation");
                let want = (rotation * p) * signs;
                let got = reflected * (p * signs);
                assert!(want.distance(got) < 1e-5, "mask {mask}: {want} vs {got}");
            }
        }
    }

    #[test]
    fn reflecting_across_a_plane_matches_the_axis_form_and_stays_outward() {
        let sphere = testing::uv_sphere(10, 14);
        // The plane x = 0.5 is the YZ plane moved along X.
        let moved = reflect_across_plane(&sphere, Vec3::new(0.5, 3.0, -2.0), Vec3::new(4.0, 0.0, 0.0));
        for (a, b) in moved.positions.iter().zip(&sphere.positions) {
            assert!(a.distance(Vec3::new(1.0 - b.x, b.y, b.z)) < 1e-6);
        }
        assert_outward_and_closed(&moved);

        let tilted = reflect_across_plane(&sphere, Vec3::new(0.3, 0.1, 0.2), Vec3::new(1.0, 2.0, -1.0));
        assert_outward_and_closed(&tilted);
        let twice = reflect_across_plane(&tilted, Vec3::new(0.3, 0.1, 0.2), Vec3::new(1.0, 2.0, -1.0));
        assert!(twice.positions.iter().zip(&sphere.positions).all(|(a, b)| a.distance(*b) < 1e-5));
        assert_eq!(reflect_across_plane(&sphere, Vec3::ZERO, Vec3::ZERO), sphere);
    }

    /// A million-triangle sphere reflected; run with `--release --ignored --nocapture`.
    #[test]
    #[ignore]
    fn timing_on_a_million_triangles() {
        let sphere = testing::uv_sphere(700, 715);
        let t = std::time::Instant::now();
        let by_axis = reflect_mesh(&sphere, axis_signs(true, false, false));
        println!("{} triangles: axis {:?}", by_axis.triangle_count(), t.elapsed());
        let t = std::time::Instant::now();
        let by_plane = reflect_across_plane(&sphere, Vec3::new(0.1, 0.0, 0.0), Vec3::new(1.0, 1.0, 0.0));
        println!("plane {:?}", t.elapsed());
        assert_eq!(by_plane.triangle_count(), by_axis.triangle_count());
    }
}
