//! Shapes and checks the tests share.

use std::collections::{HashMap, HashSet};

use glam::{Mat4, Vec3};
use sculpt_geom::{SymmetryOp, TriMesh};

use crate::field::TubeField;
use crate::live::Symmetry;
use crate::spine;
use crate::types::{TubePoint, TubeSettings};

/// A subdivided icosahedron: even triangles, and (about its own centre) an
/// exact mirror image of itself across all three coordinate planes.
/// `20 * 4^subdivisions` triangles.
pub fn icosphere(subdivisions: u32, radius: f32, centre: Vec3) -> TriMesh {
    let g = (1.0 + 5.0f32.sqrt()) * 0.5;
    #[rustfmt::skip]
    let corners = [
        [-1.0, g, 0.0], [1.0, g, 0.0], [-1.0, -g, 0.0], [1.0, -g, 0.0],
        [0.0, -1.0, g], [0.0, 1.0, g], [0.0, -1.0, -g], [0.0, 1.0, -g],
        [g, 0.0, -1.0], [g, 0.0, 1.0], [-g, 0.0, -1.0], [-g, 0.0, 1.0],
    ];
    let mut positions: Vec<Vec3> = corners.iter().map(|&p| Vec3::from(p).normalize()).collect();
    #[rustfmt::skip]
    let mut indices: Vec<u32> = vec![
        0, 11, 5, 0, 5, 1, 0, 1, 7, 0, 7, 10, 0, 10, 11, 1, 5, 9, 5, 11, 4, 11, 10, 2, 10, 7, 6, 7, 1, 8,
        3, 9, 4, 3, 4, 2, 3, 2, 6, 3, 6, 8, 3, 8, 9, 4, 9, 5, 2, 4, 11, 6, 2, 10, 8, 6, 7, 9, 8, 1,
    ];
    for _ in 0..subdivisions {
        let mut midpoints: HashMap<(u32, u32), u32> = HashMap::new();
        let mut midpoint = |a: u32, b: u32| {
            *midpoints.entry((a.min(b), a.max(b))).or_insert_with(|| {
                positions.push((positions[a as usize] + positions[b as usize]).normalize());
                positions.len() as u32 - 1
            })
        };
        let mut next = Vec::with_capacity(indices.len() * 4);
        for t in indices.chunks_exact(3) {
            let (a, b, c) = (t[0], t[1], t[2]);
            let (ab, bc, ca) = (midpoint(a, b), midpoint(b, c), midpoint(c, a));
            next.extend_from_slice(&[a, ab, ca, b, bc, ab, c, ca, bc, ab, bc, ca]);
        }
        indices = next;
    }
    let mut mesh = TriMesh::new(positions.iter().map(|&p| p * radius + centre).collect(), indices);
    // Settled by the volume rather than by argument.
    if mesh.signed_volume() < 0.0 {
        mesh.flip();
    }
    mesh
}

/// Open, crowded and inconsistently wound edges, and collapsed triangles.
pub fn defects(mesh: &TriMesh) -> [usize; 4] {
    let mut edges: HashMap<(u32, u32), (u32, u32)> = HashMap::new();
    let mut collapsed = 0;
    for t in mesh.indices.chunks_exact(3) {
        if t[0] == t[1] || t[1] == t[2] || t[2] == t[0] {
            collapsed += 1;
        }
        for c in 0..3 {
            let (a, b) = (t[c], t[(c + 1) % 3]);
            let uses = edges.entry((a.min(b), a.max(b))).or_default();
            if a < b { uses.0 += 1 } else { uses.1 += 1 }
        }
    }
    let (mut open, mut crowded, mut inconsistent) = (0, 0, 0);
    for &(forward, backward) in edges.values() {
        match forward + backward {
            1 => open += 1,
            2 if forward == 1 => {}
            2 => inconsistent += 1,
            _ => crowded += 1,
        }
    }
    [open, crowded, inconsistent, collapsed]
}

/// Asserts a mesh is a closed, consistently wound surface facing outwards:
/// every edge shared by exactly two triangles running opposite ways, and a
/// positive volume.
pub fn assert_closed(mesh: &TriMesh, what: &str) {
    assert!(!mesh.is_empty(), "{what}: empty");
    assert_eq!(defects(mesh), [0; 4], "{what}: open, crowded, inconsistent, collapsed");
    assert!(mesh.positions.iter().all(|p| p.is_finite()), "{what}: a vertex is not a number");
    assert!(mesh.signed_volume() > 0.0, "{what}: wound inside out");
}

/// How many separate pieces a mesh is in.
pub fn shells(mesh: &TriMesh) -> usize {
    let mut parent: Vec<u32> = (0..mesh.vertex_count() as u32).collect();
    fn find(parent: &mut [u32], mut x: u32) -> u32 {
        while parent[x as usize] != x {
            parent[x as usize] = parent[parent[x as usize] as usize];
            x = parent[x as usize];
        }
        x
    }
    let mut used = vec![false; mesh.vertex_count()];
    for t in mesh.indices.chunks_exact(3) {
        let a = find(&mut parent, t[0]);
        for &v in t {
            used[v as usize] = true;
            let b = find(&mut parent, v);
            parent[b as usize] = a;
        }
    }
    (0..mesh.vertex_count() as u32).filter(|&v| used[v as usize] && find(&mut parent, v) == v).count()
}

/// How many vertices have no exact mirror image across the plane of `axis`,
/// and how many lie on the plane.
pub fn unmatched_across(mesh: &TriMesh, axis: usize) -> (usize, usize) {
    let bits = |p: Vec3| p.to_array().map(|v| (v + 0.0).to_bits());
    let all: HashSet<[u32; 3]> = mesh.positions.iter().map(|&p| bits(p)).collect();
    let image = |p: Vec3| {
        let mut q = p;
        q[axis] = -q[axis];
        q
    };
    let unmatched = mesh.positions.iter().filter(|&&p| !all.contains(&bits(image(p)))).count();
    (unmatched, mesh.positions.iter().filter(|p| p[axis] == 0.0).count())
}

/// The field of one tube through free points, mirrored across `mirror`, in
/// the space the points are given in.
pub fn field_of(points: &[TubePoint], settings: &TubeSettings, mirror: [bool; 3]) -> TubeField {
    field_under(points, settings, &Symmetry::new(mirror, 1, Vec3::Y))
}

/// The field of one tube through free points, repeated under `symmetry`, in
/// the space the points are given in.
pub fn field_under(points: &[TubePoint], settings: &TubeSettings, symmetry: &Symmetry) -> TubeField {
    let mut field = TubeField::new();
    for op in &symmetry.group() {
        let (_, samples) = spine::sample(points, settings, &Mat4::IDENTITY, op);
        field.add_tube(&samples, settings.caps);
    }
    field
}

/// A tube's stations, unmirrored.
pub fn samples_of(points: &[TubePoint], settings: &TubeSettings) -> Vec<crate::Sample> {
    spine::sample(points, settings, &Mat4::IDENTITY, &SymmetryOp::IDENTITY).1
}

pub fn sphere_volume(radius: f64) -> f64 {
    4.0 / 3.0 * std::f64::consts::PI * radius.powi(3)
}
