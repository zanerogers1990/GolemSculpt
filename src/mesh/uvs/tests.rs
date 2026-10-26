//! UVs on the mesh: kept by strokes and by rebuilds of the same triangles,
//! retired by dynamic topology and brought back by its undo, and gone from
//! anything that makes new triangles.

use glam::{Vec2, Vec3};
use sculpt_multires::Split;

use super::*;
use crate::mesh::Polys;
use crate::mesh::levels::{stepped, subdivided};
use crate::mesh::primitives::{self, Primitive};

fn ball() -> Mesh {
    primitives::build(Primitive::Sphere, 2_000, 0.5)
}

/// A UV for every corner of a list of `corners` corners that says which
/// triangle and which corner it is, so that one carried to the wrong place
/// shows.
fn named(corners: usize) -> Vec<Vec2> {
    (0..corners).map(|k| Vec2::new((k / 3) as f32, (k % 3) as f32)).collect()
}

/// `mesh` with [`named`] UVs.
fn with_uvs(mut mesh: Mesh) -> Mesh {
    mesh.set_uvs(named(mesh.indices.len())).unwrap();
    mesh
}

fn every_leaf(mesh: &Mesh) -> Vec<u32> {
    (0..mesh.bvh.leaves.len() as u32).collect()
}

#[test]
fn uvs_go_on_and_come_off_whole_and_a_list_of_the_wrong_length_is_refused() {
    let mut mesh = ball();
    assert!(!mesh.has_uvs() && mesh.uvs().is_empty());
    let uvs = named(mesh.indices.len());
    assert_eq!(mesh.set_uvs(uvs.clone()), Ok(Vec::new()), "there were none before");
    assert!(mesh.has_uvs());
    assert_eq!(mesh.uvs()[15..18], [Vec2::new(5.0, 0.0), Vec2::new(5.0, 1.0), Vec2::new(5.0, 2.0)]);

    let refused = mesh.set_uvs(vec![Vec2::ZERO; 5]);
    assert!(refused.is_err_and(|why| why.contains("one for each corner")));
    assert_eq!(mesh.uvs(), &uvs[..], "a refused list leaves the mesh as it was");
    assert!(mesh.memory_bytes() >= uvs.len() * 8);

    assert_eq!(mesh.set_uvs(Vec::new()), Ok(uvs.clone()), "an empty list is none, and gives back what was there");
    assert!(!mesh.has_uvs());
    mesh.set_uvs(uvs.clone()).unwrap();
    assert_eq!(mesh.clear_uvs(), uvs);
    assert!(!mesh.has_uvs());
}

#[test]
fn a_stroke_moves_the_vertices_and_leaves_the_uvs_alone() {
    let mut mesh = with_uvs(ball());
    let (uvs, before) = (mesh.uvs().to_vec(), mesh.positions.clone());
    let leaves = every_leaf(&mesh);
    mesh.begin_stroke();
    mesh.move_vertices(&leaves, |_, v| (v.pos.y > 0.0).then(|| v.pos + v.normal * 0.02));
    mesh.positions_changed(&leaves);
    let delta = mesh.end_stroke(false).expect("the stroke moved the top half");
    assert_ne!(mesh.positions, before);
    assert_eq!(mesh.uvs(), &uvs[..]);
    mesh.apply_delta(&delta, true);
    assert_eq!((&mesh.positions, mesh.uvs()), (&before, &uvs[..]), "and undo neither");
}

#[test]
fn a_rebuild_keeps_each_triangles_uvs_on_its_own_corners() {
    let mesh = ball();
    let count = mesh.triangle_count();
    // The triangles handed in last first, each starting from its second
    // corner, after one that no build keeps (two corners on one vertex).
    let mut indices = vec![0, 0, 1];
    for t in (0..count).rev() {
        let c = &mesh.indices[t * 3..t * 3 + 3];
        indices.extend_from_slice(&[c[1], c[2], c[0]]);
    }
    let uvs = named(indices.len());
    let (mut built, numbering) = Mesh::new_numbered(mesh.positions.clone(), indices.clone(), None, &[]);
    assert_eq!(built.triangle_count(), count, "all but the one no build keeps");
    assert!(numbering.triangle.iter().enumerate().any(|(t, &was)| was as usize != t), "the build put them in another order");
    assert!(!numbering.triangle.contains(&0), "the one no build keeps is in no place");
    built.set_uvs(renumbered(&uvs, &numbering.triangle)).unwrap();

    // Corner c of each triangle is corner c of the triangle it was, and has
    // that corner's UV: the build turned no triangle round.
    for t in 0..count {
        let was = numbering.triangle[t] as usize;
        for c in 0..3 {
            assert_eq!(built.indices[t * 3 + c], numbering.vertex[indices[was * 3 + c] as usize], "triangle {t} corner {c}");
            assert_eq!(built.uvs()[t * 3 + c], uvs[was * 3 + c], "triangle {t} corner {c}");
        }
    }
    // A list that is not one for the old mesh carries nothing.
    assert!(renumbered(&uvs[..30], &numbering.triangle).is_empty());
    assert!(renumbered(&[], &numbering.triangle).is_empty());
}

#[test]
fn dynamic_topology_retires_the_uvs_and_its_undo_brings_them_back() {
    let mut mesh = with_uvs(ball());
    let uvs = mesh.uvs().to_vec();
    let centre = mesh.positions[0];
    mesh.begin_stroke();
    assert!(mesh.refine(&[centre], 0.2, 0.01, 1_000_000, 10_000_000).splits > 0);
    let delta = mesh.end_stroke(false).expect("the stroke grew the mesh");
    assert!(!mesh.has_uvs(), "the UVs were of triangles that are not there any more");
    assert!(delta.bytes() >= uvs.len() * 8, "the undo record keeps them");
    mesh.apply_delta(&delta, true);
    assert_eq!(mesh.uvs(), &uvs[..], "undo puts them back");
    mesh.apply_delta(&delta, false);
    assert!(!mesh.has_uvs(), "redo takes them off again");
    mesh.apply_delta(&delta, true);
    assert_eq!(mesh.uvs(), &uvs[..]);
}

#[test]
fn anything_that_makes_new_triangles_starts_without_uvs() {
    let mesh = with_uvs(ball());
    let finer = subdivided(&mesh, Split::Smooth).unwrap().0;
    assert!(!finer.has_uvs(), "subdivided");
    assert!(!stepped(&finer, 0).unwrap().0.has_uvs(), "and the level it came from, built again");
    assert!(!crate::remesh::voxel_remesh(&mesh, 64, [false; 3]).unwrap().has_uvs(), "remeshed");
}

#[test]
fn a_quad_whose_triangles_disagree_on_a_uv_is_written_as_its_triangles() {
    // Two quads side by side: 0 1 2 3 on the left, 1 4 5 2 on the right.
    let points = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0), (2.0, 0.0), (2.0, 1.0)];
    let points: Vec<Vec3> = points.iter().map(|&(x, y)| Vec3::new(x, y, 0.0)).collect();
    let quads = [[0u32, 1, 2, 3], [1, 4, 5, 2]];
    let indices = quads.iter().flat_map(|q| [q[0], q[1], q[2], q[0], q[2], q[3]]).collect();
    let (mut mesh, to_new) = Mesh::new_mapped(points, indices, None);
    let corners = quads.iter().flatten().map(|&v| to_new[v as usize]).collect();
    mesh.polys = Some(Polys { start: vec![0, 4, 8], indices: corners });

    // Each corner's UV is where it is; but the right quad's upper triangle,
    // the one without the corner at (2, 0), is moved along by 10 in U, so
    // the quad has a seam down its diagonal.
    let place = |mesh: &Mesh, k: usize| mesh.positions[mesh.indices[k] as usize];
    let seamed = |mesh: &Mesh, k: usize| {
        let t = k / 3;
        let triangle = [0, 1, 2].map(|c| place(mesh, t * 3 + c));
        let upper_right = triangle.iter().all(|p| p.x >= 1.0) && !triangle.contains(&Vec3::new(2.0, 0.0, 0.0));
        let p = place(mesh, k);
        Vec2::new(p.x + if upper_right { 10.0 } else { 0.0 }, p.y)
    };
    let uvs: Vec<Vec2> = (0..mesh.indices.len()).map(|k| seamed(&mesh, k)).collect();
    mesh.set_uvs(uvs).unwrap();
    let faces = mesh.faces_with_uvs().unwrap();
    let sizes: Vec<usize> = faces.faces.iter().map(|f| f.len()).collect();
    assert_eq!(sizes, [4, 3, 3], "the left quad whole, the right one as its two triangles");
    assert_eq!(faces.uvs.len(), 10);
    assert_eq!(faces.faces[0], &mesh.polys.as_ref().unwrap().indices[..4]);
    for (corner, uv) in faces.faces[0].iter().zip(&faces.uvs[..4]) {
        let p = mesh.positions[*corner as usize];
        assert_eq!(*uv, Vec2::new(p.x, p.y));
    }
    for f in 1..3 {
        let t = faces.triangle[f] as usize;
        assert_eq!(faces.faces[f], &mesh.indices[t * 3..t * 3 + 3]);
        assert_eq!(faces.uvs[4 + (f - 1) * 3..4 + f * 3], mesh.uvs()[t * 3..t * 3 + 3]);
    }

    // Without the seam both stay quads.
    let plain: Vec<Vec2> = (0..mesh.indices.len()).map(|k| place(&mesh, k).truncate()).collect();
    mesh.set_uvs(plain).unwrap();
    let sizes: Vec<usize> = mesh.faces_with_uvs().unwrap().faces.iter().map(|f| f.len()).collect();
    assert_eq!(sizes, [4, 4]);
    // A mesh of triangles has its triangles; one without UVs has none.
    mesh.polys = None;
    let faces = mesh.faces_with_uvs().unwrap();
    assert_eq!((faces.faces.len(), &faces.uvs[..]), (4, mesh.uvs()));
    mesh.clear_uvs();
    assert!(mesh.faces_with_uvs().is_none());
}
