//! Meshes and measurements the tests share.
//!
//! Both spheres are symmetric TO THE BIT across all three coordinate planes, so
//! every vertex's partner is known exactly and no tolerance is needed to find
//! it. Negation commutes with every operation used to build them.

use std::collections::HashMap;

use glam::{EulerRot, Mat4, Quat, Vec3};
use sculpt_geom::{MeshAdjacency, MeshEdges, TriMesh};

use crate::Axis;

/// A small deterministic random source, so a failing test fails the same way
/// every run.
pub struct Lcg(u64);

impl Lcg {
    pub fn new(seed: u64) -> Lcg {
        Lcg(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03)
    }

    /// Uniform in [0, 1).
    pub fn unit(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Uniform in the cube from -1 to 1.
    pub fn in_cube(&mut self) -> Vec3 {
        Vec3::new(self.unit(), self.unit(), self.unit()) * 2.0 - Vec3::ONE
    }
}

/// A repeatable pseudo-random point in the cube from -1 to 1 for index `i`,
/// for scattering the vertices of a mesh too large to walk one generator along.
pub fn scatter(i: u32) -> Vec3 {
    let unit = |salt: u32| {
        let mut h = i.wrapping_mul(0x9E37_79B1) ^ salt.wrapping_mul(0x85EB_CA77);
        h ^= h >> 16;
        h = h.wrapping_mul(0x85EB_CA6B);
        h ^= h >> 13;
        h = h.wrapping_mul(0xC2B2_AE35);
        ((h ^ (h >> 16)) >> 8) as f32 / (1u32 << 24) as f32
    };
    Vec3::new(unit(1), unit(2), unit(3)) * 2.0 - Vec3::ONE
}

/// An octahedron with every face cut into `n * n` triangles, pushed out to a
/// sphere: `4 n^2 + 2` vertices and `8 n^2` triangles, built directly (no
/// midpoint table), which is what makes a multi-million vertex one affordable.
///
/// Vertices are the integer points with `|a| + |b| + |c| = n`, normalised.
pub fn octa_sphere(n: u32, radius: f32) -> TriMesh {
    let n = n as i32;
    let mut positions = Vec::with_capacity((4 * n * n + 2) as usize);
    for b in -n..=n {
        let r = n - b.abs();
        for k in 0..ring_len(r) {
            let (a, c) = ring_point(r, k);
            positions.push(Vec3::new(a as f32, b as f32, c as f32).normalize() * radius);
        }
    }

    let mut indices = Vec::with_capacity((24 * n * n) as usize);
    for b in -n..n {
        let (lower, upper) = (n - b.abs(), n - (b + 1).abs());
        let (lower_start, upper_start) = (ring_start(n, b), ring_start(n, b + 1));
        let low = |k: i32| lower_start + (k.rem_euclid(ring_len(lower))) as u32;
        let up = |k: i32| upper_start + (k.rem_euclid(ring_len(upper))) as u32;
        for q in 0..4 {
            let (l, u) = (q * lower, q * upper);
            if upper < lower {
                for t in 0..lower {
                    indices.extend([low(l + t), up(u + t), low(l + t + 1)]);
                }
                for t in 0..upper {
                    indices.extend([low(l + t + 1), up(u + t), up(u + t + 1)]);
                }
            } else {
                for t in 0..upper {
                    indices.extend([up(u + t), up(u + t + 1), low(l + t)]);
                }
                for t in 0..lower {
                    indices.extend([up(u + t + 1), low(l + t + 1), low(l + t)]);
                }
            }
        }
    }
    TriMesh::new(positions, indices)
}

fn ring_len(r: i32) -> i32 {
    if r == 0 { 1 } else { 4 * r }
}

/// Index of the first vertex of the ring at height `b`.
fn ring_start(n: i32, b: i32) -> u32 {
    // Vertices in the rings of radius 0..=m: the apex plus 4 * (1 + ... + m).
    let up_to = |m: i32| if m < 0 { 0 } else { 1 + 2 * m * (m + 1) };
    let count = if b <= 0 { up_to(n + b - 1) } else { up_to(n) + 2 * ((n - 1) * n - (n - b) * (n - b + 1)) };
    count as u32
}

/// The `k`-th point of the ring `|a| + |c| = r`, going from +X towards +Z.
fn ring_point(r: i32, k: i32) -> (i32, i32) {
    if r == 0 {
        return (0, 0);
    }
    let (q, t) = (k / r, k % r);
    match q {
        0 => (r - t, t),
        1 => (-t, r - t),
        2 => (t - r, -t),
        _ => (t, t - r),
    }
}

/// The vertex of `octa_sphere(n, _)` at integer point `(a, b, c)`.
pub fn octa_index(n: u32, a: i32, b: i32, c: i32) -> u32 {
    let n = n as i32;
    let r = n - b.abs();
    debug_assert_eq!(a.abs() + c.abs(), r);
    let k = if r == 0 {
        0
    } else if a > 0 && c >= 0 {
        c
    } else if c > 0 {
        r - a
    } else if a < 0 {
        2 * r - c
    } else {
        3 * r + a
    };
    ring_start(n, b) + k as u32
}

/// For every vertex of `octa_sphere(n, _)`, the vertex that is exactly its
/// reflection across `axis`: itself when it lies on the plane.
pub fn octa_partners(n: u32, axis: Axis) -> Vec<u32> {
    let m = n as i32;
    let mut partners = Vec::with_capacity((4 * m * m + 2) as usize);
    for b in -m..=m {
        let r = m - b.abs();
        for k in 0..ring_len(r) {
            let (a, c) = ring_point(r, k);
            let (a, b, c) = match axis {
                Axis::X => (-a, b, c),
                Axis::Y => (a, -b, c),
                Axis::Z => (a, b, -c),
            };
            partners.push(octa_index(n, a, b, c));
        }
    }
    partners
}

pub const SPHERE_RADIUS: f32 = 0.5;

/// A cube with every face cut into `n * n` squares, pushed out to a sphere of
/// radius `SPHERE_RADIUS`: `6 n^2 + 2` vertices, `12 n^2` triangles.
///
/// With `n` odd no vertex lies on a coordinate plane: each plane runs BETWEEN
/// two rows of vertices half a spacing either side of it, which is where a
/// voxel remesh puts them. The positions are mirror-exact and so is the way
/// the squares are split, except for the one row of squares that straddles
/// each plane, which no single diagonal can split symmetrically.
pub fn cube_sphere(n: u32) -> TriMesh {
    cube_sphere_with(n, true)
}

/// `cube_sphere`, optionally with every square split the same way round
/// instead of mirror-symmetrically: the positions are still mirror-exact, but
/// on four of the six faces the two halves' diagonals then run opposite ways.
pub fn cube_sphere_with(n: u32, mirrored_diagonals: bool) -> TriMesh {
    let n = n as i32;
    let mut slots: HashMap<[i32; 3], u32> = HashMap::new();
    let mut positions = Vec::new();
    let mut indices = Vec::new();
    // Lattice points run 0..=n; centred and doubled they are exact integers
    // that negate exactly.
    let centred = |c: [i32; 3]| Vec3::new((2 * c[0] - n) as f32, (2 * c[1] - n) as f32, (2 * c[2] - n) as f32);
    let mut vertex = |c: [i32; 3]| {
        *slots.entry(c).or_insert_with(|| {
            positions.push(centred(c).normalize() * SPHERE_RADIUS);
            positions.len() as u32 - 1
        })
    };
    for face in 0..6usize {
        let (axis, side) = (face / 2, (face % 2) as i32 * n);
        let (u_axis, v_axis) = ((axis + 1) % 3, (axis + 2) % 3);
        for u in 0..n {
            for v in 0..n {
                let corner = |du: i32, dv: i32| {
                    let mut c = [0; 3];
                    c[axis] = side;
                    c[u_axis] = u + du;
                    c[v_axis] = v + dv;
                    c
                };
                let mut quad = [corner(0, 0), corner(1, 0), corner(1, 1), corner(0, 1)];
                // u cross v points along +axis, so that order is outward on the far face only.
                if side == 0 {
                    quad.swap(1, 3);
                }
                // Split along the diagonal that starts at the corner nearest the middle of the
                // face: the same one in every mirror image.
                let nearest = (0..4).min_by_key(|&k| centred(quad[k]).length_squared() as i64).unwrap();
                let q = quad.map(&mut vertex);
                if nearest % 2 == 0 || !mirrored_diagonals {
                    indices.extend([q[0], q[1], q[2], q[0], q[2], q[3]]);
                } else {
                    indices.extend([q[0], q[1], q[3], q[1], q[2], q[3]]);
                }
            }
        }
    }
    TriMesh::new(positions, indices)
}

/// The bumpy surface the icosphere is pushed out to. Reads only `|x|`, `|y|`,
/// `|z|`, so it is the same on every side of every plane.
pub fn surface_radius(dir: Vec3) -> f32 {
    let (x, y, z) = (dir.x.abs(), dir.y.abs(), dir.z.abs());
    SPHERE_RADIUS
        * (1.0
            + 0.04 * (x * 23.0 + 0.4).sin() * (y * 19.0 + 1.3).sin() * (z * 17.0 + 0.7).sin()
            + 0.015 * (x * 71.0 + z * 53.0 + y * 11.0).sin())
}

/// A subdivided icosahedron pushed out to a bumpy, slightly irregular sphere
/// (the Unity tests' `SymmetricTestMesh`). `partners[axis][i]` is the vertex
/// that is exactly vertex `i`'s reflection across that axis's plane: `i` itself
/// for a vertex on the plane.
pub fn icosphere(subdivisions: u32) -> (TriMesh, [Vec<u32>; 3]) {
    let t = (1.0 + 5.0f32.sqrt()) * 0.5;
    let mut verts: Vec<Vec3> = [
        [-1.0, t, 0.0], [1.0, t, 0.0], [-1.0, -t, 0.0], [1.0, -t, 0.0],
        [0.0, -1.0, t], [0.0, 1.0, t], [0.0, -1.0, -t], [0.0, 1.0, -t],
        [t, 0.0, -1.0], [t, 0.0, 1.0], [-t, 0.0, -1.0], [-t, 0.0, 1.0],
    ]
    .iter()
    .map(|&v| Vec3::from(v).normalize())
    .collect();
    #[rustfmt::skip]
    let mut tris: Vec<u32> = vec![
        0, 11, 5, 0, 5, 1, 0, 1, 7, 0, 7, 10, 0, 10, 11, 1, 5, 9, 5, 11, 4, 11, 10, 2, 10, 7, 6, 7, 1, 8,
        3, 9, 4, 3, 4, 2, 3, 2, 6, 3, 6, 8, 3, 8, 9, 4, 9, 5, 2, 4, 11, 6, 2, 10, 8, 6, 7, 9, 8, 1,
    ];

    for _ in 0..subdivisions {
        let mut midpoints: HashMap<(u32, u32), u32> = HashMap::with_capacity(tris.len());
        let mut next = Vec::with_capacity(tris.len() * 4);
        let mut midpoint = |i: u32, j: u32, verts: &mut Vec<Vec3>| {
            *midpoints.entry((i.min(j), i.max(j))).or_insert_with(|| {
                verts.push((verts[i as usize] + verts[j as usize]).normalize());
                verts.len() as u32 - 1
            })
        };
        for f in tris.chunks_exact(3) {
            let (a, b, c) = (f[0], f[1], f[2]);
            let (ab, bc, ca) = (midpoint(a, b, &mut verts), midpoint(b, c, &mut verts), midpoint(c, a, &mut verts));
            next.extend([a, ab, ca, b, bc, ab, c, ca, bc, ab, bc, ca]);
        }
        tris = next;
    }

    // Partners from the unit-sphere positions, before any noise: exact by construction.
    let partners = Axis::ALL.map(|axis| exact_partners(&verts, axis).expect("the icosphere is mirror-exact"));

    let edge = 1.0515 * SPHERE_RADIUS / (1u32 << subdivisions) as f32;
    for v in &mut verts {
        let u = *v;
        let hash = (u.x.abs() * 12.9898 + u.y.abs() * 78.233 + u.z.abs() * 37.719).sin() * 43758.547;
        let jitter = ((hash - hash.floor()) * 2.0 - 1.0) * 0.2 * edge;
        *v = u * (surface_radius(u) + jitter);
    }
    (TriMesh::new(verts, tris), partners)
}

/// A closed, spiky ball that no coordinate plane cuts cleanly: an octahedral
/// sphere with its vertices knocked out of their rows and each pushed in or out
/// by an amount of its own, up to three vertex spacings either way, then moved
/// off centre. A plane through it runs across the sides of the spikes, where
/// the surface crosses it, turns back and crosses again: the cut has ragged
/// outlines, islands, and necks a single edge wide.
///
/// Every vertex stays on its own ray from the centre and at least half a unit
/// along it, so the surface never passes through itself and the ball (moved
/// less than that) is astride all three planes.
pub fn bumpy_sphere(seed: u64, n: u32) -> TriMesh {
    let mut rng = Lcg::new(seed);
    let mut mesh = octa_sphere(n, 1.0);
    let spacing = 1.5 / n as f32;
    let rough = (3.0 * spacing).min(0.5);
    let offset = rng.in_cube() * 0.45;
    for p in &mut mesh.positions {
        let dir = (*p + rng.in_cube() * (0.2 * spacing)).normalize();
        *p = dir * (1.0 + (rng.unit() * 2.0 - 1.0) * rough) + offset;
    }
    mesh
}

/// A sphere with a deep pit pushed into it, the pit lying across all three
/// coordinate planes, and the whole turned a little so no vertex sits on one:
/// each plane's outline of it is concave.
pub fn dented_sphere(n: u32) -> TriMesh {
    let mut mesh = octa_sphere(n, 1.0);
    let pit = Vec3::new(0.5, 0.6, 0.62).normalize();
    for p in &mut mesh.positions {
        let near = (1.0 - p.distance(pit) / 1.1).max(0.0);
        *p *= 1.0 - 0.8 * near * near;
    }
    mesh.transform(&Mat4::from_quat(Quat::from_euler(EulerRot::XYZ, 0.07, -0.11, 0.05)));
    mesh
}

/// A ring (a torus about Y), `around` by `across` squares, wound outward. Stood
/// across a plane it is the simplest shape a cut goes through twice.
pub fn torus(major: f32, minor: f32, around: u32, across: u32) -> TriMesh {
    let mut positions = Vec::with_capacity((around * across) as usize);
    let mut indices = Vec::with_capacity((around * across * 6) as usize);
    for i in 0..around {
        let (sin_u, cos_u) = (i as f32 / around as f32 * std::f32::consts::TAU).sin_cos();
        for j in 0..across {
            let (sin_v, cos_v) = (j as f32 / across as f32 * std::f32::consts::TAU).sin_cos();
            let out = major + minor * cos_v;
            positions.push(Vec3::new(out * cos_u, minor * sin_v, out * sin_u));
            let at = |di: u32, dj: u32| ((i + di) % around) * across + (j + dj) % across;
            indices.extend([at(0, 0), at(0, 1), at(1, 1), at(0, 0), at(1, 1), at(1, 0)]);
        }
    }
    let mut mesh = TriMesh::new(positions, indices);
    if mesh.signed_volume() < 0.0 {
        mesh.flip();
    }
    mesh
}

/// What stops a mesh being one sound closed surface, counted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Faults {
    /// Edges with one triangle: a hole.
    pub open_edges: usize,
    /// Edges with three or more triangles: sheets meeting along a line.
    pub crowded_edges: usize,
    /// Edges whose two triangles face opposite ways.
    pub miswound: bool,
    /// Vertices where two separate fans of triangles touch at a point.
    pub pinched_vertices: usize,
    /// Triangles with no area at all.
    pub flat_triangles: usize,
    /// Vertices at exactly the position of an earlier one.
    pub duplicate_vertices: usize,
}

impl Faults {
    pub const NONE: Faults = Faults {
        open_edges: 0,
        crowded_edges: 0,
        miswound: false,
        pinched_vertices: 0,
        flat_triangles: 0,
        duplicate_vertices: 0,
    };

    pub fn of(mesh: &TriMesh) -> Faults {
        let adjacency = MeshAdjacency::of(mesh);
        let edges = MeshEdges::of(mesh, &adjacency);
        let flat = |t: &[u32]| {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| mesh.positions[i as usize]);
            (b - a).cross(c - a) == Vec3::ZERO
        };
        Faults {
            open_edges: edges.boundary_edges().len(),
            crowded_edges: edges.non_manifold_edges().len(),
            miswound: !edges.is_consistently_wound(&mesh.indices),
            pinched_vertices: pinched_among(mesh, &adjacency).len(),
            flat_triangles: mesh.indices.chunks_exact(3).filter(|t| flat(t)).count(),
            duplicate_vertices: duplicate_positions(&mesh.positions),
        }
    }
}

/// The vertices where two or more separate fans of triangles touch at a point,
/// ascending.
pub fn pinched_vertices(mesh: &TriMesh) -> Vec<u32> {
    pinched_among(mesh, &MeshAdjacency::of(mesh))
}

fn pinched_among(mesh: &TriMesh, adjacency: &MeshAdjacency) -> Vec<u32> {
    (0..mesh.vertex_count()).filter(|&v| fans_at(mesh, adjacency, v) > 1).map(|v| v as u32).collect()
}

/// How many separate fans the triangles round vertex `v` fall into, two
/// triangles being in one fan when they share an edge at `v`.
fn fans_at(mesh: &TriMesh, adjacency: &MeshAdjacency, v: usize) -> usize {
    let triangles = adjacency.triangles_of(v);
    let others = |t: u32| {
        let corners = &mesh.indices[t as usize * 3..t as usize * 3 + 3];
        let at = corners.iter().position(|&c| c as usize == v).unwrap();
        [corners[(at + 1) % 3], corners[(at + 2) % 3]]
    };
    let mut fan: Vec<usize> = (0..triangles.len()).collect();
    for a in 0..triangles.len() {
        for b in 0..a {
            if others(triangles[a]).iter().any(|w| others(triangles[b]).contains(w)) {
                let (from, to) = (fan[a], fan[b]);
                fan.iter_mut().filter(|f| **f == from).for_each(|f| *f = to);
            }
        }
    }
    fan.sort_unstable();
    fan.dedup();
    fan.len()
}

/// For every vertex, the vertex at exactly its reflection across `axis`, or
/// `None` if some vertex has no exact mirror image.
pub fn exact_partners(positions: &[Vec3], axis: Axis) -> Option<Vec<u32>> {
    // -0 and +0 compare equal but have different bits; adding +0 folds them together.
    let key = |p: Vec3| (p + Vec3::ZERO).to_array().map(f32::to_bits);
    let at: HashMap<[u32; 3], u32> = positions.iter().enumerate().map(|(i, &p)| (key(p), i as u32)).collect();
    positions.iter().map(|&p| at.get(&key(axis.reflect(p))).copied()).collect()
}

/// How the mesh's edges are shared: `(boundary, misoriented)`. A boundary edge
/// belongs to one triangle only; a misoriented one is walked the same way by
/// two triangles, or shared by more than two. Both zero means a closed surface
/// wound consistently.
pub fn edge_census(mesh: &TriMesh) -> (usize, usize) {
    let mut directed: HashMap<(u32, u32), u32> = HashMap::with_capacity(mesh.indices.len());
    for t in mesh.indices.chunks_exact(3) {
        for k in 0..3 {
            *directed.entry((t[k], t[(k + 1) % 3])).or_insert(0) += 1;
        }
    }
    let (mut boundary, mut misoriented) = (0, 0);
    for (&(a, b), &count) in &directed {
        if count > 1 {
            misoriented += 1;
        }
        if !directed.contains_key(&(b, a)) {
            boundary += 1;
        }
    }
    (boundary, misoriented)
}

/// Vertices that share their exact position with an earlier one.
pub fn duplicate_positions(positions: &[Vec3]) -> usize {
    let mut seen = std::collections::HashSet::with_capacity(positions.len());
    positions.iter().filter(|p| !seen.insert((**p + Vec3::ZERO).to_array().map(f32::to_bits))).count()
}

/// Triangles with (nearly) no area: under a ten-thousandth of a normal one's
/// for a mesh whose edges are about `spacing` long.
pub fn degenerate_triangles(positions: &[Vec3], indices: &[u32], spacing: f32) -> usize {
    let threshold = 1e-4 * spacing * spacing;
    indices
        .chunks_exact(3)
        .filter(|t| {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| positions[i as usize]);
            (b - a).cross(c - a).length_squared() <= threshold * threshold
        })
        .count()
}

pub fn max_delta(a: &[Vec3], b: &[Vec3]) -> f32 {
    a.iter().zip(b).map(|(p, q)| p.distance(*q)).fold(0.0, f32::max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sphere_volume(radius: f64) -> f64 {
        4.0 / 3.0 * std::f64::consts::PI * radius.powi(3)
    }

    #[test]
    fn octa_sphere_is_closed_outward_and_mirror_exact() {
        for n in [1u32, 2, 5, 16] {
            let mesh = octa_sphere(n, 2.0);
            assert_eq!(mesh.vertex_count(), (4 * n * n + 2) as usize);
            assert_eq!(mesh.triangle_count(), (8 * n * n) as usize);
            assert_eq!(edge_census(&mesh), (0, 0), "n = {n}");
            assert!(mesh.signed_volume() > 0.0, "n = {n} is wound inward");
            for axis in Axis::ALL {
                let partners = octa_partners(n, axis);
                assert_eq!(Some(partners.clone()), exact_partners(&mesh.positions, axis), "n = {n} {axis:?}");
                for (i, &j) in partners.iter().enumerate() {
                    assert_eq!(mesh.positions[j as usize], axis.reflect(mesh.positions[i]));
                }
            }
        }
        let fine = octa_sphere(64, 1.0);
        assert!((fine.signed_volume() / sphere_volume(1.0) - 1.0).abs() < 2e-3);
    }

    #[test]
    fn cube_sphere_is_closed_outward_and_mirror_exact_with_no_centreline() {
        for n in [1u32, 2, 5, 8] {
            let mesh = cube_sphere(n);
            assert_eq!(mesh.vertex_count(), (6 * n * n + 2) as usize);
            assert_eq!(mesh.triangle_count(), (12 * n * n) as usize);
            assert_eq!(edge_census(&mesh), (0, 0), "n = {n}");
            assert!(mesh.signed_volume() > 0.0, "n = {n} is wound inward");
            for axis in Axis::ALL {
                assert!(exact_partners(&mesh.positions, axis).is_some(), "n = {n} {axis:?}");
                let on_plane = mesh.positions.iter().filter(|p| axis.coord(**p) == 0.0).count();
                assert_eq!(on_plane == 0, n % 2 == 1, "n = {n} {axis:?}");
            }
        }
        let fine = cube_sphere(65);
        assert!((fine.signed_volume() / sphere_volume(SPHERE_RADIUS as f64) - 1.0).abs() < 3e-3);
    }

    #[test]
    fn the_shapes_for_cutting_are_sound_closed_surfaces() {
        for seed in 0..40 {
            let ball = bumpy_sphere(seed, 9);
            assert_eq!(Faults::of(&ball), Faults::NONE, "seed {seed}");
            assert!(ball.signed_volume() > 0.0, "seed {seed} is wound inward");
            // Astride every plane, so a cut always has something on both sides.
            let bounds = ball.bounds();
            assert!(bounds.min.max_element() < -0.05 && bounds.max.min_element() > 0.05, "seed {seed}");
        }
        assert_ne!(bumpy_sphere(1, 9).positions, bumpy_sphere(2, 9).positions);
        assert_eq!(bumpy_sphere(1, 9).positions, bumpy_sphere(1, 9).positions);
        for mesh in [dented_sphere(12), torus(1.0, 0.4, 28, 14)] {
            assert_eq!(Faults::of(&mesh), Faults::NONE);
            assert!(mesh.signed_volume() > 0.0);
        }
        let ring = torus(1.0, 0.4, 64, 32).signed_volume();
        assert!((ring / (2.0 * std::f64::consts::PI.powi(2) * 0.16) - 1.0).abs() < 0.01);
    }

    /// `Faults` sees each thing it is there to see.
    #[test]
    fn faults_are_counted() {
        // Two tetrahedra sharing one vertex: every edge has two triangles, but the vertex is pinched.
        let mut pinched = TriMesh::new(
            vec![Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::Z, -Vec3::X, -Vec3::Y, -Vec3::Z],
            vec![0, 2, 1, 0, 1, 3, 0, 3, 2, 1, 2, 3, 0, 5, 4, 0, 4, 6, 0, 6, 5, 4, 5, 6],
        );
        assert_eq!(Faults::of(&pinched), Faults { pinched_vertices: 1, ..Faults::NONE });
        // Three sheets on the edge 0-1, each with open edges of its own.
        let fan = TriMesh::new(vec![Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::Z, -Vec3::Y], vec![0, 1, 2, 0, 1, 3, 0, 1, 4]);
        let faults = Faults::of(&fan);
        assert_eq!((faults.crowded_edges, faults.open_edges), (1, 6));
        // A corner repeated in place, and a triangle turned over.
        pinched.positions[6] = pinched.positions[5];
        pinched.indices.swap(0, 1);
        let faults = Faults::of(&pinched);
        assert!(faults.miswound && faults.duplicate_vertices == 1 && faults.flat_triangles == 2, "{faults:?}");
    }

    #[test]
    fn icosphere_is_closed_outward_and_mirror_exact() {
        let (mesh, partners) = icosphere(3);
        assert_eq!(mesh.vertex_count(), 10 * 4usize.pow(3) + 2);
        assert_eq!(edge_census(&mesh), (0, 0));
        assert!(mesh.signed_volume() > 0.0);
        for axis in Axis::ALL {
            for (i, &j) in partners[axis.index()].iter().enumerate() {
                let mirrored = axis.reflect(mesh.positions[i]) + Vec3::ZERO;
                assert_eq!(mesh.positions[j as usize], mirrored, "{axis:?} vertex {i}");
            }
        }
    }
}
