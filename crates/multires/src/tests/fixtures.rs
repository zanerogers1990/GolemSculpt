//! Small meshes, a seeded generator and a few measures the tests share.

use glam::Vec3;
use sculpt_geom::adjacency::{MeshAdjacency, MeshEdges};

use crate::{LevelMesh, Levels, Polys, Split, subdivide};

/// A small deterministic generator, so tests build the same shapes every run.
pub struct Rng(pub u64);

impl Rng {
    /// Uniform in [0, 1).
    pub fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Uniform in [-1, 1) on each axis.
    pub fn vector(&mut self) -> Vec3 {
        Vec3::new(self.next(), self.next(), self.next()) * 2.0 - Vec3::ONE
    }
}

/// A triangle mesh with a clear mask.
pub fn triangle_mesh(positions: Vec<Vec3>, triangles: Vec<u32>) -> LevelMesh {
    LevelMesh { mask: vec![0.0; positions.len()], positions, triangles, polys: None }
}

/// A polygon mesh with a clear mask, its faces cut the plainest way: fanned
/// from each face's first corner.
pub fn polygon_mesh(positions: Vec<Vec3>, faces: &[&[u32]]) -> LevelMesh {
    let (mut start, mut indices, mut triangles) = (vec![0u32], Vec::new(), Vec::new());
    for face in faces {
        indices.extend_from_slice(face);
        start.push(indices.len() as u32);
        for k in 1..face.len() - 1 {
            triangles.extend_from_slice(&[face[0], face[k], face[k + 1]]);
        }
    }
    LevelMesh { mask: vec![0.0; positions.len()], positions, triangles, polys: Some(Polys { start, indices }) }
}

/// The stack a mesh starts.
pub fn start(mesh: &LevelMesh) -> Levels {
    Levels::new(mesh.positions.len(), mesh.triangles.clone(), mesh.polys.clone()).expect("a sound mesh")
}

/// Four triangles, four vertices.
pub fn tetrahedron() -> LevelMesh {
    triangle_mesh(vec![Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::Z], vec![0, 2, 1, 0, 1, 3, 0, 3, 2, 1, 2, 3])
}

/// Twelve vertices on the unit sphere, twenty triangles wound outward, five at every vertex.
pub fn icosahedron() -> LevelMesh {
    let phi = (1.0 + 5.0f32.sqrt()) / 2.0;
    let positions = [
        [-1.0, phi, 0.0],
        [1.0, phi, 0.0],
        [-1.0, -phi, 0.0],
        [1.0, -phi, 0.0],
        [0.0, -1.0, phi],
        [0.0, 1.0, phi],
        [0.0, -1.0, -phi],
        [0.0, 1.0, -phi],
        [phi, 0.0, -1.0],
        [phi, 0.0, 1.0],
        [-phi, 0.0, -1.0],
        [-phi, 0.0, 1.0],
    ];
    #[rustfmt::skip]
    let triangles = vec![
        0, 11, 5, 0, 5, 1, 0, 1, 7, 0, 7, 10, 0, 10, 11, 1, 5, 9, 5, 11, 4, 11, 10, 2, 10, 7, 6, 7, 1, 8,
        3, 9, 4, 3, 4, 2, 3, 2, 6, 3, 6, 8, 3, 8, 9, 4, 9, 5, 2, 4, 11, 6, 2, 10, 8, 6, 7, 9, 8, 1,
    ];
    triangle_mesh(positions.iter().map(|&p| Vec3::from(p).normalize()).collect(), triangles)
}

/// The icosahedron split flat `splits` times with every vertex pushed out to
/// the unit sphere: `20 * 4^splits` even triangles.
pub fn ico_sphere(splits: usize) -> LevelMesh {
    let mut mesh = icosahedron();
    for _ in 0..splits {
        mesh = subdivide(&mesh, Split::Flat).unwrap().mesh;
        mesh.positions.iter_mut().for_each(|p| *p = p.normalize());
    }
    mesh
}

/// An open sheet of `nx * ny` unit cells in the XY plane, two triangles a
/// cell, facing +Z. Vertex `(i, j)` is `j * (nx + 1) + i`.
pub fn triangle_grid(nx: usize, ny: usize) -> LevelMesh {
    let quads = quad_grid(nx, ny);
    triangle_mesh(quads.positions, quads.triangles)
}

/// The same sheet as quads.
pub fn quad_grid(nx: usize, ny: usize) -> LevelMesh {
    let vertex = |i: usize, j: usize| (j * (nx + 1) + i) as u32;
    let positions = (0..=ny).flat_map(|j| (0..=nx).map(move |i| Vec3::new(i as f32, j as f32, 0.0))).collect();
    let faces: Vec<[u32; 4]> = (0..ny)
        .flat_map(|j| (0..nx).map(move |i| [vertex(i, j), vertex(i + 1, j), vertex(i + 1, j + 1), vertex(i, j + 1)]))
        .collect();
    polygon_mesh(positions, &faces.iter().map(|f| &f[..]).collect::<Vec<_>>())
}

/// The cube [-1, 1]^3 as six quads wound outward. Vertex `i` has its x, y and
/// z from bits 0, 1 and 2 of `i`.
pub fn cube() -> LevelMesh {
    let sign = |bit: u32| if bit == 0 { -1.0 } else { 1.0 };
    let positions = (0..8u32).map(|i| Vec3::new(sign(i & 1), sign(i >> 1 & 1), sign(i >> 2 & 1))).collect();
    let faces: [&[u32]; 6] = [&[0, 2, 3, 1], &[4, 5, 7, 6], &[0, 1, 5, 4], &[2, 6, 7, 3], &[0, 4, 6, 2], &[1, 3, 7, 5]];
    polygon_mesh(positions, &faces)
}

/// A closed ring of `nu * nv` quads: a torus of ring radius 1 and tube
/// radius 0.4 about the Y axis, wound outward.
pub fn quad_torus(nu: usize, nv: usize) -> LevelMesh {
    let vertex = |i: usize, j: usize| ((j % nv) * nu + i % nu) as u32;
    let mut positions = Vec::new();
    for j in 0..nv {
        let v = std::f32::consts::TAU * j as f32 / nv as f32;
        for i in 0..nu {
            let u = std::f32::consts::TAU * i as f32 / nu as f32;
            let ring = 1.0 + 0.4 * v.cos();
            positions.push(Vec3::new(ring * u.cos(), 0.4 * v.sin(), ring * u.sin()));
        }
    }
    let faces: Vec<[u32; 4]> = (0..nv)
        .flat_map(|j| (0..nu).map(move |i| [vertex(i, j), vertex(i, j + 1), vertex(i + 1, j + 1), vertex(i + 1, j)]))
        .collect();
    let mesh = polygon_mesh(positions, &faces.iter().map(|f| &f[..]).collect::<Vec<_>>());
    assert!(signed_volume(&mesh) > 0.0, "the torus is wound outward");
    mesh
}

/// A stack started from `mesh` and subdivided once for each of `splits`,
/// with the finest level's mesh.
pub fn raised(mesh: &LevelMesh, splits: &[Split]) -> (Levels, LevelMesh) {
    let mut levels = start(mesh);
    let mut top = mesh.clone();
    for &split in splits {
        top = levels.subdivide(&top.positions, &top.mask, split).unwrap();
    }
    (levels, top)
}

/// Sculpts a mesh all over: every vertex nudged by up to `amount` each way
/// and the mask painted at random.
pub fn roughen(mesh: &mut LevelMesh, amount: f32, rng: &mut Rng) {
    for (p, m) in mesh.positions.iter_mut().zip(&mut mesh.mask) {
        *p += rng.vector() * amount;
        *m = rng.next();
    }
}

/// A smooth bump of `height` raised along `out` on every vertex within
/// `radius` of `centre`.
pub fn raise_bump(positions: &mut [Vec3], centre: Vec3, radius: f32, height: f32, out: impl Fn(Vec3) -> Vec3) {
    for p in positions {
        let d = p.distance(centre) / radius;
        if d < 1.0 {
            *p += out(*p) * height * (1.0 - d * d).powi(2);
        }
    }
}

/// Area-weighted vertex normals.
pub fn vertex_normals(positions: &[Vec3], triangles: &[u32]) -> Vec<Vec3> {
    let mut normals = vec![Vec3::ZERO; positions.len()];
    for t in triangles.chunks_exact(3) {
        let [a, b, c] = [t[0], t[1], t[2]].map(|v| positions[v as usize]);
        let n = (b - a).cross(c - a);
        t.iter().for_each(|&v| normals[v as usize] += n);
    }
    normals.iter().map(|n| n.normalize_or_zero()).collect()
}

/// The longest edge of a triangle mesh.
pub fn longest_edge(positions: &[Vec3], triangles: &[u32]) -> f32 {
    let side = |a: u32, b: u32| positions[a as usize].distance(positions[b as usize]);
    triangles.chunks_exact(3).map(|t| side(t[0], t[1]).max(side(t[1], t[2])).max(side(t[2], t[0]))).fold(0.0, f32::max)
}

/// Distinct edges of a triangle list.
pub fn edge_count(vertices: usize, triangles: &[u32]) -> usize {
    MeshEdges::build(&MeshAdjacency::build(vertices, triangles), triangles).len()
}

/// Vertices less edges plus faces of a triangle mesh: 2 for a closed surface like a ball.
pub fn euler(mesh: &LevelMesh) -> i64 {
    let edges = edge_count(mesh.positions.len(), &mesh.triangles);
    mesh.positions.len() as i64 - edges as i64 + (mesh.triangles.len() / 3) as i64
}

/// Volume enclosed, positive for a closed mesh wound outward.
pub fn signed_volume(mesh: &LevelMesh) -> f64 {
    let p = |i: u32| mesh.positions[i as usize].as_dvec3();
    mesh.triangles.chunks_exact(3).map(|t| p(t[0]).dot(p(t[1]).cross(p(t[2])))).sum::<f64>() / 6.0
}

/// Whether two lists of positions are the same to the bit.
pub fn same_bits(a: &[Vec3], b: &[Vec3]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| a.to_array().map(f32::to_bits) == b.to_array().map(f32::to_bits))
}

/// The largest distance between matching entries.
pub fn furthest(a: &[Vec3], b: &[Vec3]) -> f32 {
    assert_eq!(a.len(), b.len());
    a.iter().zip(b).map(|(a, b)| a.distance(*b)).fold(0.0, f32::max)
}

#[test]
fn the_shared_meshes_are_what_they_say() {
    assert_eq!(euler(&icosahedron()), 2);
    assert!((signed_volume(&icosahedron()) - 2.536).abs() < 1e-3);
    assert!((signed_volume(&cube()) - 8.0).abs() < 1e-6);
    assert!((signed_volume(&tetrahedron()) - 1.0 / 6.0).abs() < 1e-7);
    let sphere = ico_sphere(3);
    assert_eq!((sphere.positions.len(), sphere.triangles.len() / 3), (642, 1280));
    // A little under a ball's 4.189: the corners are on the sphere and the faces inside it.
    assert!((signed_volume(&sphere) - 4.16).abs() < 0.02, "{}", signed_volume(&sphere));
    assert_eq!(quad_torus(8, 6).polys.unwrap().face_count(), 48);
    assert_eq!(triangle_grid(3, 2).triangles.len(), 36);
}
