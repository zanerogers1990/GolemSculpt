//! Shapes and checks the tests share.

use std::collections::HashMap;

use glam::Vec3;
use sculpt_geom::TriMesh;

/// A small repeatable random source (xorshift), so tests need no crate for it.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    /// The next number in 0..1.
    pub fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }

    pub fn vec3(&mut self) -> Vec3 {
        Vec3::new(self.next(), self.next(), self.next())
    }

    pub fn below(&mut self, n: usize) -> usize {
        ((self.next() * n as f32) as usize).clamp(0, n - 1)
    }
}

/// A subdivided icosahedron: even triangles, none degenerate, and (about its
/// own centre) an exact mirror image of itself across all three coordinate
/// planes. `20 * 4^subdivisions` triangles.
pub fn icosphere(subdivisions: u32, radius: f32, centre: Vec3) -> TriMesh {
    let g = (1.0 + 5.0f32.sqrt()) * 0.5;
    let mut positions: Vec<Vec3> = [
        [-1.0, g, 0.0], [1.0, g, 0.0], [-1.0, -g, 0.0], [1.0, -g, 0.0],
        [0.0, -1.0, g], [0.0, 1.0, g], [0.0, -1.0, -g], [0.0, 1.0, -g],
        [g, 0.0, -1.0], [g, 0.0, 1.0], [-g, 0.0, -1.0], [-g, 0.0, 1.0],
    ]
    .iter()
    .map(|&p| Vec3::from(p).normalize())
    .collect();
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
    // The winding above is Unity's (clockwise from outside in its left-handed
    // frame, which is counter-clockwise from outside here only if the numbers
    // say so): settle it by the volume rather than by argument.
    let mut mesh = TriMesh::new(positions.iter().map(|&p| p * radius + centre).collect(), indices);
    if mesh.signed_volume() < 0.0 {
        mesh.flip();
    }
    mesh
}

/// A latitude and longitude sphere: long thin triangles at the poles and
/// whole fans meeting at a point, which an icosphere never has.
pub fn uv_sphere(rings: u32, segments: u32, radius: f32, centre: Vec3) -> TriMesh {
    let mut positions = vec![centre + Vec3::Y * radius];
    for r in 1..rings {
        let polar = std::f32::consts::PI * r as f32 / rings as f32;
        for s in 0..segments {
            let around = std::f32::consts::TAU * s as f32 / segments as f32;
            let dir = Vec3::new(polar.sin() * around.cos(), polar.cos(), polar.sin() * around.sin());
            positions.push(centre + dir * radius);
        }
    }
    positions.push(centre - Vec3::Y * radius);
    let ring = |r: u32, s: u32| 1 + (r - 1) * segments + s % segments;
    let bottom = positions.len() as u32 - 1;
    let mut indices = Vec::new();
    for s in 0..segments {
        indices.extend_from_slice(&[0, ring(1, s + 1), ring(1, s)]);
        for r in 1..rings - 1 {
            let (a, b, c, d) = (ring(r, s), ring(r, s + 1), ring(r + 1, s + 1), ring(r + 1, s));
            indices.extend_from_slice(&[a, b, c, a, c, d]);
        }
        indices.extend_from_slice(&[bottom, ring(rings - 1, s), ring(rings - 1, s + 1)]);
    }
    let mut mesh = TriMesh::new(positions, indices);
    if mesh.signed_volume() < 0.0 {
        mesh.flip();
    }
    mesh
}

/// An axis-aligned box of twelve triangles.
pub fn box_mesh(min: Vec3, max: Vec3) -> TriMesh {
    let positions = (0..8)
        .map(|c| {
            let pick = |axis: usize| if c >> axis & 1 != 0 { max[axis] } else { min[axis] };
            Vec3::new(pick(0), pick(1), pick(2))
        })
        .collect();
    let quads = [[0, 4, 6, 2], [1, 3, 7, 5], [0, 1, 5, 4], [2, 6, 7, 3], [0, 2, 3, 1], [4, 5, 7, 6]];
    let indices = quads.iter().flat_map(|q| [q[0], q[1], q[2], q[0], q[2], q[3]]).collect();
    TriMesh::new(positions, indices)
}

/// `mesh` without the triangles whose middle is within `radius` of `point`:
/// a hole.
pub fn without_near(mesh: &TriMesh, point: Vec3, radius: f32) -> TriMesh {
    let kept = (0..mesh.triangle_count()).filter(|&t| {
        let [a, b, c] = mesh.triangle(t);
        ((a + b + c) / 3.0).distance(point) > radius
    });
    let indices = kept.flat_map(|t| mesh.indices[t * 3..t * 3 + 3].to_vec()).collect();
    TriMesh::new(mesh.positions.clone(), indices)
}

/// The lower half of a sphere about the origin, open at the top: a bowl with
/// its rim on the plane y = 0 and no thickness.
pub fn bowl(rings: u32, segments: u32, radius: f32) -> TriMesh {
    let sphere = uv_sphere(rings, segments, radius, Vec3::ZERO);
    let below = |t: &usize| sphere.triangle(*t).iter().all(|p| p.y < radius * 1e-4);
    let indices = (0..sphere.triangle_count()).filter(below).flat_map(|t| sphere.indices[t * 3..t * 3 + 3].to_vec());
    TriMesh::new(sphere.positions.clone(), indices.collect())
}

/// The side of a cylinder along x, about the origin, open at both ends.
pub fn open_tube(segments: u32, rings: u32, radius: f32, length: f32) -> TriMesh {
    let mut positions = Vec::new();
    for r in 0..=rings {
        let x = length * (r as f32 / rings as f32 - 0.5);
        for s in 0..segments {
            let around = std::f32::consts::TAU * s as f32 / segments as f32;
            positions.push(Vec3::new(x, radius * around.cos(), radius * around.sin()));
        }
    }
    let at = |r: u32, s: u32| r * segments + s % segments;
    let mut indices = Vec::new();
    for r in 0..rings {
        for s in 0..segments {
            let (a, b, c, d) = (at(r, s), at(r + 1, s), at(r + 1, s + 1), at(r, s + 1));
            indices.extend_from_slice(&[a, b, c, a, c, d]);
        }
    }
    TriMesh::new(positions, indices)
}

/// The turn about the origin by the given angles (degrees) about x, then y,
/// then z.
pub fn turn(degrees: Vec3) -> glam::Quat {
    let r = degrees * (std::f32::consts::PI / 180.0);
    glam::Quat::from_euler(glam::EulerRot::ZYX, r.z, r.y, r.x)
}

/// `mesh` turned about the origin by [`turn`]: a way off the lattice for
/// shapes with flat faces.
pub fn rotated(mesh: &TriMesh, degrees: Vec3) -> TriMesh {
    let turn = turn(degrees);
    TriMesh::new(mesh.positions.iter().map(|&p| turn * p).collect(), mesh.indices.clone())
}

/// What is wrong with a mesh as a closed surface, if anything.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Defects {
    /// Edges used by one triangle only.
    pub open: usize,
    /// Edges used by more than two triangles.
    pub crowded: usize,
    /// Edges whose two triangles run along them the same way round.
    pub inconsistent: usize,
    /// Triangles naming the same vertex twice.
    pub collapsed: usize,
}

/// Checks that every edge is shared by exactly two triangles that traverse it
/// in opposite directions.
pub fn defects(mesh: &TriMesh) -> Defects {
    let mut edges: HashMap<(u32, u32), (u32, u32)> = HashMap::new();
    let mut out = Defects::default();
    for t in mesh.indices.chunks_exact(3) {
        if t[0] == t[1] || t[1] == t[2] || t[2] == t[0] {
            out.collapsed += 1;
        }
        for c in 0..3 {
            let (a, b) = (t[c], t[(c + 1) % 3]);
            let uses = edges.entry((a.min(b), a.max(b))).or_default();
            if a < b { uses.0 += 1 } else { uses.1 += 1 }
        }
    }
    for &(forward, backward) in edges.values() {
        match forward + backward {
            1 => out.open += 1,
            2 if forward == 1 => {}
            2 => out.inconsistent += 1,
            _ => out.crowded += 1,
        }
    }
    out
}

/// Asserts a mesh is a closed, consistently wound surface facing outwards.
pub fn assert_closed(mesh: &TriMesh, what: &str) {
    assert!(!mesh.is_empty(), "{what}: empty");
    assert_eq!(defects(mesh), Defects::default(), "{what}");
    assert!(mesh.positions.iter().all(|p| p.is_finite()), "{what}: a vertex is not a number");
    assert!(mesh.signed_volume() > 0.0, "{what}: wound inside out");
}

/// How many separate pieces a mesh is in (vertices no triangle uses are not
/// counted).
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

/// How far the worst vertex is from the sphere it should lie on.
pub fn worst_radius_error(mesh: &TriMesh, radius: f32, centre: Vec3) -> f32 {
    mesh.positions.iter().map(|p| (p.distance(centre) - radius).abs()).fold(0.0, f32::max)
}

pub fn sphere_volume(radius: f64) -> f64 {
    4.0 / 3.0 * std::f64::consts::PI * radius.powi(3)
}

/// Volume shared by two spheres of radius `r` whose centres are `d` apart.
pub fn lens_volume(r: f64, d: f64) -> f64 {
    std::f64::consts::PI * (4.0 * r + d) * (2.0 * r - d).powi(2) / 12.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shapes_are_closed_and_face_outwards() {
        for (mesh, volume) in [
            (icosphere(3, 2.0, Vec3::new(1.0, 0.0, -1.0)), sphere_volume(2.0)),
            (uv_sphere(24, 48, 2.0, Vec3::ZERO), sphere_volume(2.0)),
            (box_mesh(Vec3::new(-1.0, 0.0, 2.0), Vec3::new(1.0, 3.0, 2.5)), 3.0),
        ] {
            assert_closed(&mesh, "test shape");
            assert_eq!(shells(&mesh), 1);
            assert!((mesh.signed_volume() / volume - 1.0).abs() < 0.03, "{} vs {volume}", mesh.signed_volume());
        }
        assert_eq!(icosphere(2, 1.0, Vec3::ZERO).triangle_count(), 320);
    }

    #[test]
    fn the_icosphere_is_its_own_mirror_image() {
        let mesh = icosphere(3, 0.5, Vec3::ZERO);
        let bits = |p: Vec3| p.to_array().map(|v| (v + 0.0).to_bits());
        let all: std::collections::HashSet<_> = mesh.positions.iter().map(|&p| bits(p)).collect();
        for &p in &mesh.positions {
            assert!(all.contains(&bits(Vec3::new(-p.x, p.y, p.z))));
            assert!(all.contains(&bits(Vec3::new(p.x, -p.y, p.z))));
        }
    }

    #[test]
    fn defects_are_noticed() {
        let mut mesh = box_mesh(Vec3::ZERO, Vec3::ONE);
        mesh.indices.truncate(33);
        assert_eq!(defects(&mesh).open, 3);
        let mut two = box_mesh(Vec3::ZERO, Vec3::ONE);
        two.append(&box_mesh(Vec3::splat(2.0), Vec3::splat(3.0)));
        assert_eq!(shells(&two), 2);
    }
}
