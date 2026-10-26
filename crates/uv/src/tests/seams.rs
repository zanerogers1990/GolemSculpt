//! Where the seams go: along polygon edges, never through a quad, and along
//! face-group borders, never across one.

use super::checks::NO_ISLAND;
use super::meshes::{cube, icosphere, torus};
use super::shapes::unwrap_and_check;

/// Every corner a polygon's triangles share has one UV in all of them, so
/// no polygon is cut by a seam.
fn assert_polygons_whole(name: &str, triangles: &[u32], start: &[u32], corners: &[u32], uvs: &[glam::Vec2]) {
    let mut t = 0;
    for (f, span) in start.windows(2).enumerate() {
        let face = &corners[span[0] as usize..span[1] as usize];
        let pieces = face.len() - 2;
        for &v in face {
            let at: Vec<glam::Vec2> = (t..t + pieces)
                .flat_map(|tt| (0..3).map(move |c| tt * 3 + c))
                .filter(|&c| triangles[c] == v)
                .map(|c| uvs[c])
                .collect();
            assert!(at.windows(2).all(|w| w[0] == w[1]), "{name}: face {f} is split at point {v}");
        }
        t += pieces;
    }
}

#[test]
fn a_quad_cube_keeps_every_quad_whole() {
    let model = cube(6, true);
    let (out, q) = unwrap_and_check("quad cube", &model, None);
    let (start, corners) = model.polys().unwrap();
    assert_polygons_whole("quad cube", &model.triangles, start, corners, &out.uvs);
    assert_eq!(q.charts, 6);
}

#[test]
fn a_bumpy_quad_torus_keeps_every_quad_whole() {
    let model = torus(80, 40, (0.2, 9.0, 6.0), true);
    let (out, _) = unwrap_and_check("bumpy quad torus, quads", &model, None);
    let (start, corners) = model.polys().unwrap();
    assert_polygons_whole("bumpy quad torus", &model.triangles, start, corners, &out.uvs);
}

#[test]
fn two_face_groups_on_a_sphere_put_a_seam_along_their_border() {
    let model = icosphere(4);
    // Top and bottom halves, by each triangle's middle.
    let groups: Vec<u16> = model
        .triangles
        .chunks_exact(3)
        .map(|t| {
            let z: f32 = t.iter().map(|&v| model.positions[v as usize].z).sum();
            (z > 0.0) as u16
        })
        .collect();
    let (out, q) = unwrap_and_check("two-group sphere", &model, Some(&groups));
    // No chart holds both groups.
    let mut group_of_island = vec![None; q.islands];
    for (t, &island) in q.island_of.iter().enumerate().filter(|(_, i)| **i != NO_ISLAND) {
        let seen = group_of_island[island as usize].get_or_insert(groups[t]);
        assert_eq!(*seen, groups[t], "a chart crosses the group border");
    }
    // So a point on the border has no UV in common between the two sides.
    let mut uvs_by_side = vec![[Vec::new(), Vec::new()]; model.positions.len()];
    for (c, &v) in model.triangles.iter().enumerate() {
        uvs_by_side[v as usize][groups[c / 3] as usize].push(out.uvs[c]);
    }
    let border: Vec<_> = uvs_by_side.iter().filter(|s| !s[0].is_empty() && !s[1].is_empty()).collect();
    assert!(border.len() > 10, "{} points on the border", border.len());
    for [below, above] in border {
        assert!(below.iter().all(|p| !above.contains(p)), "a border point shares a UV across it");
    }
}

#[test]
fn groups_on_a_quad_mesh_follow_whole_quads() {
    // Groups painted per triangle, with some quads' two triangles in
    // different groups: the quad stays whole and goes with its first
    // triangle's group, and still no chart crosses a border.
    let model = torus(48, 24, (0.0, 0.0, 0.0), true);
    let groups: Vec<u16> = (0..model.triangle_count())
        .map(|t| {
            let quad = t / 2;
            let ring = quad / 24;
            let odd_triangle_moved = t % 2 == 1 && quad % 5 == 0;
            ((ring < 24) ^ odd_triangle_moved) as u16
        })
        .collect();
    let (out, q) = unwrap_and_check("grouped quad torus", &model, Some(&groups));
    let (start, corners) = model.polys().unwrap();
    assert_polygons_whole("grouped quad torus", &model.triangles, start, corners, &out.uvs);
    let quad_group = |t: usize| groups[t - t % 2];
    let mut group_of_island = vec![None; q.islands];
    for (t, &island) in q.island_of.iter().enumerate().filter(|(_, i)| **i != NO_ISLAND) {
        let seen = group_of_island[island as usize].get_or_insert(quad_group(t));
        assert_eq!(*seen, quad_group(t), "a chart crosses the group border");
    }
    assert!(q.charts >= 2);
}
