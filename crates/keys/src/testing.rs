//! Shapes and checks the tests of this crate share. Compiled for tests only.

use std::collections::HashMap;
use std::sync::Arc;

use glam::{Mat4, Vec3};
use sculpt_geom::{MeshAdjacency, MeshEdges, TriMesh};

use crate::cutting::split_by_plane;
use crate::joint::KeyPart;

/// A sphere of radius `radius` from an icosahedron split `level` times:
/// 20 x 4^level triangles of nearly equal size, wound outward.
pub fn icosphere(level: u32, radius: f32) -> TriMesh {
    let phi = (1.0 + 5f32.sqrt()) * 0.5;
    let corners = [
        (-1.0, phi, 0.0),
        (1.0, phi, 0.0),
        (-1.0, -phi, 0.0),
        (1.0, -phi, 0.0),
        (0.0, -1.0, phi),
        (0.0, 1.0, phi),
        (0.0, -1.0, -phi),
        (0.0, 1.0, -phi),
        (phi, 0.0, -1.0),
        (phi, 0.0, 1.0),
        (-phi, 0.0, -1.0),
        (-phi, 0.0, 1.0),
    ];
    let mut positions: Vec<Vec3> = corners.iter().map(|&(x, y, z)| Vec3::new(x, y, z).normalize()).collect();
    let mut indices: Vec<u32> = vec![
        0, 11, 5, 0, 5, 1, 0, 1, 7, 0, 7, 10, 0, 10, 11, 1, 5, 9, 5, 11, 4, 11, 10, 2, 10, 7, 6, 7, 1, 8, 3, 9, 4, 3,
        4, 2, 3, 2, 6, 3, 6, 8, 3, 8, 9, 4, 9, 5, 2, 4, 11, 6, 2, 10, 8, 6, 7, 9, 8, 1,
    ];
    for _ in 0..level {
        let mut midpoints: HashMap<(u32, u32), u32> = HashMap::with_capacity(indices.len());
        let mut next = Vec::with_capacity(indices.len() * 4);
        for t in indices.chunks_exact(3) {
            let mut mid = [0u32; 3];
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                mid[k] = *midpoints.entry((a.min(b), a.max(b))).or_insert_with(|| {
                    positions.push(((positions[a as usize] + positions[b as usize]) * 0.5).normalize());
                    positions.len() as u32 - 1
                });
            }
            next.extend_from_slice(&[t[0], mid[0], mid[2], t[1], mid[1], mid[0], t[2], mid[2], mid[1]]);
            next.extend_from_slice(&mid);
        }
        indices = next;
    }
    for t in indices.chunks_exact_mut(3) {
        let [a, b, c] = [t[0], t[1], t[2]].map(|i| positions[i as usize]);
        if (b - a).cross(c - a).dot(a + b + c) < 0.0 {
            t.swap(1, 2);
        }
    }
    for p in &mut positions {
        *p *= radius;
    }
    TriMesh::new(positions, indices)
}

/// Why a mesh is not a closed surface wound outward, or `None` when it is:
/// every edge between exactly two triangles that walk it opposite ways, every
/// vertex used, and a positive volume.
pub fn closed_error(mesh: &TriMesh) -> Option<String> {
    if mesh.indices.chunks_exact(3).any(|t| t[0] == t[1] || t[1] == t[2] || t[2] == t[0]) {
        return Some("a triangle uses one vertex twice".into());
    }
    let adjacency = MeshAdjacency::of(mesh);
    let edges = MeshEdges::of(mesh, &adjacency);
    let (open, crowded) = (edges.boundary_edges().len(), edges.non_manifold_edges().len());
    if open > 0 || crowded > 0 {
        return Some(format!("{open} open edges and {crowded} shared by more than two triangles"));
    }
    if !edges.is_consistently_wound(&mesh.indices) {
        return Some("neighbouring triangles are wound against each other".into());
    }
    let unused = (0..mesh.vertex_count()).filter(|&v| adjacency.triangles_of(v).is_empty()).count();
    if unused > 0 {
        return Some(format!("{unused} vertices no triangle uses"));
    }
    (mesh.signed_volume() <= 0.0).then(|| "wound inside out".into())
}

#[track_caller]
pub fn assert_closed(mesh: &TriMesh, what: &str) {
    if let Some(problem) = closed_error(mesh) {
        panic!("{what}: {problem}");
    }
}

#[track_caller]
pub fn assert_near(actual: f64, expected: f64, relative: f64, what: &str) {
    assert!(
        (actual - expected).abs() <= expected.abs() * relative,
        "{what}: {actual} is not within {}% of {expected}",
        relative * 100.0
    );
}

/// A sphere cut in two by a plane, as the key code takes it: the piece behind
/// the plane's normal (A), the piece the normal points into (B), and their
/// mating faces as the splitter reports them.
pub struct Halves {
    pub a: Arc<KeyPart>,
    pub b: Arc<KeyPart>,
    pub pairs: Vec<(usize, usize)>,
    /// The cut plane, in world space: a point on it and its unit normal,
    /// pointing out of A into B.
    pub point: Vec3,
    pub normal: Vec3,
    /// The radius of the round cut face, in world units.
    pub face_radius: f32,
}

/// A sphere of `radius` world units about the origin, cut by the plane
/// `offset` along `normal` from its centre. `scale` is the size of the
/// objects holding the halves: the mesh is made that much smaller, so the
/// world size is the same.
pub fn halves(level: u32, radius: f32, normal: Vec3, offset: f32, scale: f32) -> Halves {
    let normal = normal.normalize();
    let sphere = icosphere(level, radius / scale);
    let cut = split_by_plane(&sphere, normal * offset / scale, normal).expect("the plane crosses the sphere");
    let to_world = Mat4::from_scale(Vec3::splat(scale));
    Halves {
        a: Arc::new(KeyPart::new(cut.outside.mesh, to_world, cut.outside.cut_faces)),
        b: Arc::new(KeyPart::new(cut.inside.mesh, to_world, cut.inside.cut_faces)),
        pairs: cut.face_pairs,
        point: normal * offset,
        normal,
        face_radius: (radius * radius - offset * offset).sqrt(),
    }
}

/// A part's mesh in world space.
pub fn world_mesh(mesh: &TriMesh, to_world: Mat4) -> TriMesh {
    let mut mesh = mesh.clone();
    mesh.transform(&to_world);
    mesh
}
