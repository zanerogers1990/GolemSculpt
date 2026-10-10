//! Shapes and checks the tests share. Compiled for tests only.

use std::collections::HashMap;

use glam::Vec3;
use sculpt_geom::TriMesh;

/// A sphere about the origin from an octahedron split `level` times:
/// 8 x 4^level triangles of nearly equal size, with a vertex ring on each of
/// the three coordinate planes.
pub fn sphere(level: u32, radius: f32) -> TriMesh {
    let mut positions = vec![Vec3::X, Vec3::NEG_X, Vec3::Y, Vec3::NEG_Y, Vec3::Z, Vec3::NEG_Z];
    let mut indices: Vec<u32> = vec![0, 2, 4, 2, 1, 4, 1, 3, 4, 3, 0, 4, 2, 0, 5, 1, 2, 5, 3, 1, 5, 0, 3, 5];
    for _ in 0..level {
        let mut midpoints: HashMap<(u32, u32), u32> = HashMap::new();
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
    for p in &mut positions {
        *p *= radius;
    }
    let mut mesh = TriMesh::new(positions, indices);
    if mesh.signed_volume() < 0.0 {
        mesh.flip();
    }
    mesh
}

/// Why a mesh is not a closed, consistently wound surface, or `None` when it
/// is: every edge walked exactly once each way.
pub fn closed_error(positions: usize, indices: &[u32]) -> Option<String> {
    if let Some(&i) = indices.iter().find(|&&i| i as usize >= positions) {
        return Some(format!("index {i} is past the last vertex"));
    }
    let mut directed: Vec<u64> =
        indices.chunks_exact(3).flat_map(|t| (0..3).map(move |k| ((t[k] as u64) << 32) | t[(k + 1) % 3] as u64)).collect();
    directed.sort_unstable();
    if directed.windows(2).any(|w| w[0] == w[1]) {
        return Some("an edge is used twice the same way".to_owned());
    }
    let open = directed.iter().filter(|&&e| directed.binary_search(&e.rotate_left(32)).is_err()).count();
    (open > 0).then(|| format!("{open} open edges"))
}

/// The volume a closed mesh encloses, positive when wound outward.
pub fn volume(positions: &[Vec3], indices: &[u32]) -> f64 {
    TriMesh::new(positions.to_vec(), indices.to_vec()).signed_volume()
}

#[test]
fn the_test_sphere_is_closed_and_round() {
    let ball = sphere(4, 1.0);
    assert_eq!(ball.triangle_count(), 8 * 256);
    assert_eq!(closed_error(ball.vertex_count(), &ball.indices), None);
    let full = 4.0 / 3.0 * std::f64::consts::PI;
    assert!((ball.signed_volume() - full).abs() < full * 0.02, "{}", ball.signed_volume());
    assert!(closed_error(ball.vertex_count(), &ball.indices[3..]).unwrap().contains("open"));
}
