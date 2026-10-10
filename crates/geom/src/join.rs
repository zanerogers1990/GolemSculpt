//! Joining several meshes into one: plain concatenation, each part moved by its
//! own matrix first. Not a boolean and not welded; anyone who wants watertight
//! topology out of overlapping parts remeshes afterwards.
//!
//! The matrix of a part is whatever takes its vertices into the frame of the
//! result. Joining objects into the first of them, that is the survivor's
//! world-to-local matrix times the part's local-to-world matrix.

use glam::{Mat4, Vec3};
use rayon::prelude::*;

use crate::mesh::TriMesh;

/// The result of `join`, and where each part went.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Joined {
    pub mesh: TriMesh,
    /// Part `i`'s vertices are `vertex_starts[i]..vertex_starts[i + 1]` of the
    /// result, in their original order, so per-vertex data (a mask, say) carries
    /// across by concatenation. A part with no vertices has an empty range.
    pub vertex_starts: Vec<u32>,
}

/// Concatenates `parts`, moving each by its matrix. A matrix that mirrors
/// (negative determinant) also reverses that part's winding, so its outside stays
/// outside. Each part's indices keep their order; a trailing partial triangle is dropped.
pub fn join(parts: &[(&TriMesh, Mat4)]) -> Joined {
    let triangle_indices = |m: &TriMesh| m.indices.len() - m.indices.len() % 3;
    let vertex_total: usize = parts.iter().map(|(m, _)| m.positions.len()).sum();
    let index_total: usize = parts.iter().map(|(m, _)| triangle_indices(m)).sum();
    assert!(vertex_total <= u32::MAX as usize, "too many vertices to join into one mesh");

    let mut positions = vec![Vec3::ZERO; vertex_total];
    let mut indices = vec![0u32; index_total];
    let mut vertex_starts = Vec::with_capacity(parts.len() + 1);

    // One pair of disjoint output slices per part, so the parts can be filled at once.
    let mut jobs = Vec::with_capacity(parts.len());
    let (mut position_rest, mut index_rest) = (&mut positions[..], &mut indices[..]);
    let mut base = 0u32;
    for (mesh, matrix) in parts {
        vertex_starts.push(base);
        let (out_positions, tail) = position_rest.split_at_mut(mesh.positions.len());
        position_rest = tail;
        let (out_indices, tail) = index_rest.split_at_mut(triangle_indices(mesh));
        index_rest = tail;
        jobs.push((*mesh, matrix, base, out_positions, out_indices));
        base += mesh.positions.len() as u32;
    }
    vertex_starts.push(base);

    jobs.into_par_iter().for_each(|(mesh, matrix, base, out_positions, out_indices)| {
        out_positions.par_iter_mut().zip(&mesh.positions).for_each(|(out, &p)| *out = matrix.transform_point3(p));
        let mirrored = matrix.determinant() < 0.0;
        out_indices.par_chunks_exact_mut(3).zip(mesh.indices.par_chunks_exact(3)).for_each(|(out, t)| {
            let (second, third) = if mirrored { (t[2], t[1]) } else { (t[1], t[2]) };
            out[0] = t[0] + base;
            out[1] = second + base;
            out[2] = third + base;
        });
    });

    Joined { mesh: TriMesh::new(positions, indices), vertex_starts }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adjacency::{MeshAdjacency, MeshEdges, connected_components};
    use crate::testing;

    #[test]
    fn parts_land_where_their_matrices_put_them() {
        let (cube, tetra) = (testing::cube(), testing::tetrahedron());
        let move_cube = Mat4::from_translation(Vec3::new(10.0, 0.0, 0.0));
        let scale_tetra = Mat4::from_scale(Vec3::splat(3.0));
        let joined = join(&[(&cube, move_cube), (&tetra, scale_tetra)]);

        assert_eq!(joined.vertex_starts, [0, 8, 12]);
        assert_eq!(joined.mesh.vertex_count(), 12);
        assert_eq!(joined.mesh.triangle_count(), 16);
        assert_eq!(joined.mesh.positions[0], cube.positions[0] + Vec3::new(10.0, 0.0, 0.0));
        assert_eq!(joined.mesh.positions[9], tetra.positions[1] * 3.0);
        assert!((joined.mesh.signed_volume() - (8.0 + 27.0 / 6.0)).abs() < 1e-4);
        // The second part's indices were moved up past the first part's vertices.
        assert!(joined.mesh.indices[36..].iter().all(|&i| (8..12).contains(&i)));
        assert_eq!(connected_components(12, &joined.mesh.indices).count, 2);
    }

    #[test]
    fn a_mirroring_matrix_keeps_its_part_outward() {
        let tetra = testing::tetrahedron();
        let mirror = Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0));
        let joined = join(&[(&tetra, Mat4::IDENTITY), (&tetra, mirror * Mat4::from_translation(Vec3::new(3.0, 0.0, 0.0)))]);
        assert!((joined.mesh.signed_volume() - 2.0 / 6.0).abs() < 1e-6);
        let edges = MeshEdges::of(&joined.mesh, &MeshAdjacency::of(&joined.mesh));
        assert!(edges.is_closed() && edges.is_consistently_wound(&joined.mesh.indices));
    }

    #[test]
    fn empty_parts_and_no_parts_are_fine() {
        let tetra = testing::tetrahedron();
        let empty = TriMesh::default();
        let joined = join(&[(&empty, Mat4::IDENTITY), (&tetra, Mat4::IDENTITY), (&empty, Mat4::IDENTITY)]);
        assert_eq!(joined.vertex_starts, [0, 0, 4, 4]);
        assert_eq!(joined.mesh, tetra);
        assert_eq!(join(&[]), Joined { mesh: TriMesh::default(), vertex_starts: vec![0] });
    }

    #[test]
    fn many_large_parts_join_in_order() {
        let sphere = testing::uv_sphere(100, 120);
        let moves: Vec<Mat4> = (0..6).map(|k| Mat4::from_translation(Vec3::new(3.0 * k as f32, 0.0, 0.0))).collect();
        let parts: Vec<(&TriMesh, Mat4)> = moves.iter().map(|&m| (&sphere, m)).collect();
        let joined = join(&parts);
        assert_eq!(joined.mesh.triangle_count(), 6 * sphere.triangle_count());
        for k in 0..6 {
            let range = joined.vertex_starts[k] as usize..joined.vertex_starts[k + 1] as usize;
            assert_eq!(range.len(), sphere.vertex_count());
            assert_eq!(joined.mesh.positions[range.start], sphere.positions[0] + Vec3::new(3.0 * k as f32, 0.0, 0.0));
        }
        assert_eq!(connected_components(joined.mesh.vertex_count(), &joined.mesh.indices).count, 6);
    }

    /// Six million-triangle spheres joined; run with `--release --ignored --nocapture`.
    #[test]
    #[ignore]
    fn timing_on_six_million_triangles() {
        let sphere = testing::uv_sphere(700, 715);
        let moves: Vec<Mat4> = (0..6).map(|k| Mat4::from_translation(Vec3::new(3.0 * k as f32, 0.0, 0.0))).collect();
        let parts: Vec<(&TriMesh, Mat4)> = moves.iter().map(|&m| (&sphere, m)).collect();
        let t = std::time::Instant::now();
        let joined = join(&parts);
        println!("{} triangles joined in {:?}", joined.mesh.triangle_count(), t.elapsed());
    }
}
