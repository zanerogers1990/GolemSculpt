//! Checks of UVs & Maps as the app runs them. The unwrapping itself is
//! checked where it lives (crates/uv), and the baking in src/bake.

use super::*;
use crate::mesh::primitives::{self, Primitive};

#[test]
fn unwrapping_gives_a_copy_with_uvs_in_the_square_and_leaves_the_model_alone() {
    let ball = primitives::build(Primitive::Sphere, 4_000, 0.5);
    let (copy, charts, seams) = unwrapped(&ball, true).unwrap();
    assert!(!ball.has_uvs(), "the model itself is not touched");
    assert!(copy.has_uvs());
    assert_eq!(copy.uvs().len(), copy.indices.len(), "one for every corner");
    assert!(copy.uvs().iter().all(|uv| (0.0..=1.0).contains(&uv.x) && (0.0..=1.0).contains(&uv.y)));
    assert!(charts >= 1);
    assert!(!seams, "a ball with no face groups has none to follow");
    // The same surface: the copy is numbered afresh, but every point is still there.
    let sorted = |m: &Mesh| {
        let mut p: Vec<[i64; 3]> = m.positions.iter().map(|p| p.to_array().map(|c| (c * 1e5).round() as i64)).collect();
        p.sort_unstable();
        p
    };
    assert_eq!(sorted(&copy), sorted(&ball));
}

#[test]
fn a_quad_remesh_unwraps_with_every_quad_whole_whatever_order_its_triangles_are_in() {
    let ball = primitives::build(Primitive::Sphere, 20_000, 0.5);
    let (quads, _) = crate::quadremesh::quad_remesh_mesh(&ball, 800).unwrap();
    assert!(quads.polys.is_some());
    let (copy, charts, _) = unwrapped(&quads, false).unwrap();
    assert!(charts >= 1);
    let polys = copy.polys.as_ref().unwrap();
    // Each quad's two triangles agree on the UV at every corner they share: no seam runs through a quad.
    let mut at: std::collections::HashMap<(u32, u32), glam::Vec2> = std::collections::HashMap::new();
    for (t, c) in copy.indices.chunks_exact(3).enumerate() {
        // Find the polygon this triangle belongs to by its corners.
        let face = polys.start.windows(2).position(|w| {
            let corners = &polys.indices[w[0] as usize..w[1] as usize];
            c.iter().all(|v| corners.contains(v))
        });
        let Some(face) = face else { panic!("triangle {t} is in no polygon") };
        for k in 0..3 {
            let uv = copy.uvs()[t * 3 + k];
            if let Some(seen) = at.insert((face as u32, c[k]), uv) {
                assert!(seen.distance(uv) < 1e-6, "a seam through quad {face}");
            }
        }
    }
}

#[test]
fn seams_follow_the_face_groups_when_asked() {
    let mut ball = primitives::build(Primitive::Sphere, 4_000, 0.5);
    let ids: Vec<u16> = ball
        .indices
        .chunks_exact(3)
        .map(|t| if ball.positions[t[0] as usize].y > 0.0 { 1 } else { 2 })
        .collect();
    ball.set_group_ids(ids);
    let (_, with, seams) = unwrapped(&ball, true).unwrap();
    assert!(seams);
    let (_, without, _) = unwrapped(&ball, false).unwrap();
    assert!(with >= 2 && with >= without, "the border is a seam: {with} pieces against {without}");
}
