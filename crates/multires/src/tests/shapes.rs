//! What one subdivision makes: how many of everything, in what order, wound
//! which way, and what happens to meshes that are open, broken or tiny.

use glam::Vec3;
use sculpt_geom::adjacency::{MeshAdjacency, MeshEdges};
use sculpt_geom::edge_census;

use super::fixtures::*;
use crate::{LevelMesh, Levels, Polys, Split, subdivide};

fn consistently_wound(mesh: &LevelMesh) -> bool {
    let adjacency = MeshAdjacency::build(mesh.positions.len(), &mesh.triangles);
    MeshEdges::build(&adjacency, &mesh.triangles).is_consistently_wound(&mesh.triangles)
}

fn all_finite(mesh: &LevelMesh) -> bool {
    mesh.positions.iter().all(|p| p.is_finite()) && mesh.mask.iter().all(|m| m.is_finite())
}

/// Whether `p` lies in triangle `t` of `mesh`, edges included.
fn inside(mesh: &LevelMesh, t: u32, p: Vec3) -> bool {
    let [a, b, c] = [0, 1, 2].map(|k| mesh.positions[mesh.triangles[t as usize * 3 + k] as usize]);
    let normal = (b - a).cross(c - a);
    let side = |from: Vec3, to: Vec3| (to - from).cross(p - from).dot(normal) / normal.length_squared();
    (p - a).dot(normal).abs() < 1e-5 && side(a, b) > -1e-5 && side(b, c) > -1e-5 && side(c, a) > -1e-5
}

fn centroid(mesh: &LevelMesh, t: usize) -> Vec3 {
    (0..3).map(|k| mesh.positions[mesh.triangles[t * 3 + k] as usize]).sum::<Vec3>() / 3.0
}

#[test]
fn loop_makes_four_triangles_of_each_and_a_vertex_on_each_edge() {
    let mut mesh = icosahedron();
    for _ in 0..3 {
        let edges = edge_count(mesh.positions.len(), &mesh.triangles);
        let fine = subdivide(&mesh, Split::Smooth).unwrap();
        assert_eq!(fine.mesh.positions.len(), mesh.positions.len() + edges);
        assert_eq!(fine.mesh.triangles.len(), mesh.triangles.len() * 4);
        assert!(fine.mesh.polys.is_none() && fine.parent_polys.is_none());
        // A closed mesh stays closed, in one piece, the same way out.
        assert!(edge_census(fine.mesh.positions.len(), &fine.mesh.triangles).is_closed());
        assert_eq!(euler(&fine.mesh), 2);
        assert!(consistently_wound(&fine.mesh));
        assert!(signed_volume(&fine.mesh) > 0.0);
        mesh = fine.mesh;
    }
    assert_eq!((mesh.positions.len(), mesh.triangles.len() / 3), (642, 1280));
}

/// The numbering is part of what a saved file means. This pins it.
#[test]
fn loop_numbers_new_vertices_by_edge_and_new_triangles_by_corner() {
    let mesh = tetrahedron();
    let fine = subdivide(&mesh, Split::Flat).unwrap();
    assert!(same_bits(&fine.mesh.positions[..4], &mesh.positions), "kept vertices keep their numbers and places");
    // Edges in ascending order of (lower vertex, higher vertex).
    let edges = [[0, 1], [0, 2], [0, 3], [1, 2], [1, 3], [2, 3]];
    for (k, [a, b]) in edges.into_iter().enumerate() {
        assert_eq!(fine.mesh.positions[4 + k], (mesh.positions[a] + mesh.positions[b]) * 0.5, "edge {a}-{b}");
    }
    let on =
        |a: u32, b: u32| 4 + edges.iter().position(|e| *e == [a.min(b) as usize, a.max(b) as usize]).unwrap() as u32;
    for (t, c) in mesh.triangles.chunks_exact(3).enumerate() {
        let (m0, m1, m2) = (on(c[0], c[1]), on(c[1], c[2]), on(c[2], c[0]));
        let expected = [c[0], m0, m2, c[1], m1, m0, c[2], m2, m1, m0, m1, m2];
        assert_eq!(fine.mesh.triangles[t * 12..t * 12 + 12], expected, "children of triangle {t}");
        assert_eq!(fine.parent_triangles[t * 4..t * 4 + 4], [t as u32; 4]);
    }
}

#[test]
fn every_loop_triangle_lies_in_its_parent() {
    let mesh = ico_sphere(1);
    let fine = subdivide(&mesh, Split::Flat).unwrap();
    for (t, &parent) in fine.parent_triangles.iter().enumerate() {
        assert!(inside(&mesh, parent, centroid(&fine.mesh, t)), "triangle {t}");
    }
}

#[test]
fn catmull_clark_makes_a_quad_at_each_corner() {
    let mesh = cube();
    let fine = subdivide(&mesh, Split::Smooth).unwrap();
    // 8 kept, 12 on edges, 6 in faces; 4 quads a face, two triangles a quad.
    assert_eq!(fine.mesh.positions.len(), 8 + 12 + 6);
    let polys = fine.mesh.polys.as_ref().unwrap();
    assert_eq!(polys.start, (0..=24).map(|q| q * 4).collect::<Vec<u32>>());
    assert_eq!(fine.mesh.triangles.len(), 48 * 3);
    for (q, t) in polys.indices.chunks_exact(4).zip(fine.mesh.triangles.chunks_exact(6)) {
        assert_eq!(t, [q[0], q[1], q[2], q[0], q[2], q[3]], "a quad is drawn as its corners 0 1 2 and 0 2 3");
    }
    assert!(edge_census(26, &fine.mesh.triangles).is_closed());
    assert_eq!(euler(&fine.mesh), 2);
    assert!(consistently_wound(&fine.mesh));
    assert!(signed_volume(&fine.mesh) > 0.0);

    // After the first step every face is a quad, so the counts are regular.
    let finer = subdivide(&fine.mesh, Split::Smooth).unwrap();
    assert_eq!(finer.mesh.positions.len(), 26 + 48 + 24);
    assert_eq!(finer.mesh.polys.as_ref().unwrap().face_count(), 96);
    assert!(edge_census(98, &finer.mesh.triangles).is_closed());
    assert_eq!(euler(&finer.mesh), 2);
    assert_eq!(finer.parent_polys.unwrap(), (0..96).map(|q| q / 4).collect::<Vec<u32>>());
}

/// The numbering is part of what a saved file means. This pins it.
#[test]
fn catmull_clark_numbers_edges_then_faces_and_quads_by_corner() {
    let mesh = cube();
    let polys = mesh.polys.clone().unwrap();
    let fine = subdivide(&mesh, Split::Flat).unwrap();
    assert!(same_bits(&fine.mesh.positions[..8], &mesh.positions));
    // A cube's edges join vertices one bit apart; in ascending order of (lower, higher):
    let edges = [[0, 1], [0, 2], [0, 4], [1, 3], [1, 5], [2, 3], [2, 6], [3, 7], [4, 5], [4, 6], [5, 7], [6, 7]];
    for (k, [a, b]) in edges.into_iter().enumerate() {
        assert_eq!(fine.mesh.positions[8 + k], (mesh.positions[a] + mesh.positions[b]) * 0.5, "edge {a}-{b}");
    }
    let on =
        |a: u32, b: u32| 8 + edges.iter().position(|e| *e == [a.min(b) as usize, a.max(b) as usize]).unwrap() as u32;
    let quads = &fine.mesh.polys.as_ref().unwrap().indices;
    for (f, face) in polys.indices.chunks_exact(4).enumerate() {
        let middle = 20 + f as u32;
        assert_eq!(
            fine.mesh.positions[middle as usize],
            face.iter().map(|&v| mesh.positions[v as usize]).sum::<Vec3>() / 4.0
        );
        for k in 0..4 {
            let (before, corner, after) = (face[(k + 3) % 4], face[k], face[(k + 1) % 4]);
            let quad = f * 4 + k;
            assert_eq!(quads[quad * 4..quad * 4 + 4], [corner, on(corner, after), middle, on(before, corner)]);
            assert_eq!(fine.parent_polys.as_ref().unwrap()[quad], f as u32);
        }
    }
}

/// A sheet of quads cut into triangles the way the app might: either
/// diagonal, and the triangles in no particular order.
fn sheet_cut_any_way() -> LevelMesh {
    let mut mesh = quad_grid(4, 3);
    let polys = mesh.polys.clone().unwrap();
    let mut triangles: Vec<[u32; 3]> = Vec::new();
    for (f, q) in polys.indices.chunks_exact(4).enumerate() {
        if f % 2 == 0 {
            triangles.extend([[q[0], q[1], q[2]], [q[0], q[2], q[3]]]);
        } else {
            // The other diagonal, and each triangle started from a different corner.
            triangles.extend([[q[3], q[0], q[1]], [q[2], q[3], q[1]]]);
        }
    }
    let mut rng = Rng(5);
    for k in (1..triangles.len()).rev() {
        triangles.swap(k, (rng.next() * (k + 1) as f32) as usize);
    }
    mesh.triangles = triangles.into_iter().flatten().collect();
    mesh
}

#[test]
fn every_catmull_clark_triangle_lies_in_its_parent_whichever_way_the_quads_were_cut() {
    for mesh in [cube(), sheet_cut_any_way()] {
        let fine = subdivide(&mesh, Split::Flat).unwrap();
        for (t, &parent) in fine.parent_triangles.iter().enumerate() {
            assert!(inside(&mesh, parent, centroid(&fine.mesh, t)), "triangle {t} of the first level");
        }
        // And again from a level made here, where the parents follow by rule.
        let finer = subdivide(&fine.mesh, Split::Flat).unwrap();
        for (t, &parent) in finer.parent_triangles.iter().enumerate() {
            assert!(inside(&fine.mesh, parent, centroid(&finer.mesh, t)), "triangle {t} of the second level");
        }
    }
}

/// A triangle, a quad and a pentagon round one vertex, as a quad remesh leaves here and there.
fn mixed_faces() -> LevelMesh {
    let positions = vec![
        Vec3::ZERO,
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::new(0.5, 1.0, 0.0),
        Vec3::new(-0.6, 1.0, 0.0),
        Vec3::new(-1.0, 0.0, 0.0),
        Vec3::new(-1.0, -1.0, 0.0),
        Vec3::new(0.0, -1.4, 0.0),
        Vec3::new(1.0, -1.0, 0.0),
    ];
    polygon_mesh(positions, &[&[0, 1, 2], &[0, 2, 3, 4], &[0, 4, 5, 6, 7], &[0, 7, 1]])
}

#[test]
fn faces_of_any_size_become_quads_whose_triangles_stay_in_their_own_face() {
    let mesh = mixed_faces();
    let fine = subdivide(&mesh, Split::Smooth).unwrap();
    let polys = mesh.polys.as_ref().unwrap();
    // One quad per corner of the old faces; a vertex per old vertex, edge and face.
    assert_eq!(fine.mesh.polys.as_ref().unwrap().face_count(), 3 + 4 + 5 + 3);
    assert_eq!(fine.mesh.positions.len(), 8 + edge_count(8, &mesh.triangles) - (1 + 2) + 4);
    assert!(all_finite(&fine.mesh) && consistently_wound(&fine.mesh));
    let parent_polys = fine.parent_polys.unwrap();
    for (t, &parent) in fine.parent_triangles.iter().enumerate() {
        let face = parent_polys[t / 2] as usize;
        let corners = &polys.indices[polys.start[face] as usize..polys.start[face + 1] as usize];
        let parent = &mesh.triangles[parent as usize * 3..parent as usize * 3 + 3];
        assert!(parent.iter().all(|v| corners.contains(v)), "triangle {t} came from a triangle of another face");
    }
}

/// The open edges of a mesh and the vertices on them.
fn rim(mesh: &LevelMesh) -> (usize, Vec<u32>) {
    let adjacency = MeshAdjacency::build(mesh.positions.len(), &mesh.triangles);
    let edges = MeshEdges::build(&adjacency, &mesh.triangles);
    let open = edges.boundary_edges();
    let mut on: Vec<u32> = open.iter().flat_map(|&e| edges.edge(e)).collect();
    on.sort_unstable();
    on.dedup();
    (open.len(), on)
}

/// A 6 by 6 sheet with the middle 2 by 2 cells cut out, as triangles or as quads.
fn sheet_with_a_hole(quads: bool) -> LevelMesh {
    let sheet = quad_grid(6, 6);
    let polys = sheet.polys.unwrap();
    let keep = |f: usize| !((2..4).contains(&(f % 6)) && (2..4).contains(&(f / 6)));
    let faces: Vec<&[u32]> =
        polys.indices.chunks_exact(4).enumerate().filter(|(f, _)| keep(*f)).map(|(_, q)| q).collect();
    let mesh = polygon_mesh(sheet.positions, &faces);
    if quads { mesh } else { triangle_mesh(mesh.positions, mesh.triangles) }
}

#[test]
fn an_open_rim_neither_shrinks_away_nor_spikes() {
    for quads in [false, true] {
        let mut mesh = sheet_with_a_hole(quads);
        let (mut open, _) = rim(&mesh);
        assert_eq!(open, 24 + 8);
        for level in 1..=4 {
            mesh = subdivide(&mesh, Split::Smooth).unwrap().mesh;
            let (now_open, on_rim) = rim(&mesh);
            assert_eq!(now_open, open * 2, "each rim edge is cut in two");
            open = now_open;
            assert_eq!(edge_census(mesh.positions.len(), &mesh.triangles).holes, Some(2));
            assert!(mesh.positions.iter().all(|p| p.z == 0.0), "a flat sheet stays flat");

            // The outline still reaches the sheet's sides and nothing has left it.
            let (low, high) = mesh.positions.iter().fold((Vec3::MAX, Vec3::MIN), |(l, h), &p| (l.min(p), h.max(p)));
            assert_eq!((low.truncate(), high.truncate()), (glam::Vec2::ZERO, glam::Vec2::splat(6.0)), "level {level}");

            // The hole's rim still reaches the sides of the hole that was cut,
            // and is nowhere outside it (its corners round off inward).
            let outline = |p: Vec3| p.x < 1.0 || p.x > 5.0 || p.y < 1.0 || p.y > 5.0;
            let hole: Vec<Vec3> = on_rim.iter().map(|&v| mesh.positions[v as usize]).filter(|&p| !outline(p)).collect();
            let (low, high) = hole.iter().fold((Vec3::MAX, Vec3::MIN), |(l, h), &p| (l.min(p), h.max(p)));
            assert_eq!((low.truncate(), high.truncate()), (glam::Vec2::splat(2.0), glam::Vec2::splat(4.0)));
            // Nothing of the surface has been pulled into the hole. (Its
            // corners end a sixth of a cell in from where they were cut, and no
            // further. The vertex left in the middle by the cut is not surface.)
            let in_hole = |p: Vec3| p.x > 2.2 && p.x < 3.8 && p.y > 2.2 && p.y < 3.8;
            assert!(!mesh.triangles.iter().any(|&v| in_hole(mesh.positions[v as usize])));
            let corner = hole.iter().map(|p| p.distance(Vec3::new(2.0, 2.0, 0.0))).fold(f32::MAX, f32::min);
            assert!(corner > 0.17 && corner < 0.24, "{corner}");
        }
    }
}

#[test]
fn one_triangle_one_quad_and_no_mesh_at_all() {
    let triangle = triangle_mesh(vec![Vec3::ZERO, Vec3::X * 4.0, Vec3::Y * 4.0], vec![0, 1, 2]);
    let fine = subdivide(&triangle, Split::Smooth).unwrap().mesh;
    assert_eq!((fine.positions.len(), fine.triangles.len()), (6, 12));
    // All rim: corners follow the curve through their two neighbours, edges are cut in the middle.
    assert_eq!(
        fine.positions,
        [
            Vec3::new(0.5, 0.5, 0.0),
            Vec3::new(3.0, 0.5, 0.0),
            Vec3::new(0.5, 3.0, 0.0),
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(0.0, 2.0, 0.0),
            Vec3::new(2.0, 2.0, 0.0),
        ]
    );

    let quad = polygon_mesh(vec![Vec3::ZERO, Vec3::X * 4.0, Vec3::new(4.0, 4.0, 0.0), Vec3::Y * 4.0], &[&[0, 1, 2, 3]]);
    let fine = subdivide(&quad, Split::Smooth).unwrap().mesh;
    assert_eq!((fine.positions.len(), fine.triangles.len(), fine.polys.unwrap().face_count()), (9, 24, 4));
    assert_eq!(fine.positions[0], Vec3::new(0.5, 0.5, 0.0));
    assert_eq!(fine.positions[4], Vec3::new(2.0, 0.0, 0.0), "the first edge, 0-1");
    assert_eq!(fine.positions[8], Vec3::new(2.0, 2.0, 0.0), "the face");

    for polys in [None, Some(Polys { start: vec![0], indices: Vec::new() })] {
        let nothing = LevelMesh { polys, ..LevelMesh::default() };
        let fine = subdivide(&nothing, Split::Smooth).unwrap();
        assert!(fine.mesh.positions.is_empty() && fine.mesh.triangles.is_empty() && fine.parent_triangles.is_empty());
        let fine = subdivide(&fine.mesh, Split::Flat).unwrap();
        assert!(fine.mesh.positions.is_empty() && fine.mesh.mask.is_empty());
    }
}

#[test]
fn stray_vertices_and_collapsed_triangles_are_carried_along() {
    // A triangle, a vertex nothing uses, and a triangle with two equal corners.
    let stray = Vec3::new(7.0, 8.0, 9.0);
    let mesh = triangle_mesh(vec![Vec3::ZERO, Vec3::X, Vec3::Y, stray], vec![0, 1, 2, 1, 1, 2]);
    let fine = subdivide(&mesh, Split::Smooth).unwrap();
    assert_eq!(fine.mesh.positions.len(), 4 + 3);
    assert_eq!(fine.mesh.positions[3], stray, "a vertex with nothing to be smoothed against stays put");
    assert_eq!(fine.mesh.triangles.len(), 24);
    assert_eq!(fine.mesh.triangles[12..], [1, 1, 2].repeat(4), "four copies that still draw nothing");
    assert!(all_finite(&fine.mesh));
    let finer = subdivide(&fine.mesh, Split::Smooth).unwrap();
    assert!(all_finite(&finer.mesh));
    assert_eq!(finer.mesh.positions[3], stray);
}

#[test]
fn an_edge_with_three_faces_is_a_crease_not_a_crash() {
    // Three triangles fanned round the edge 0-1.
    let positions = vec![Vec3::ZERO, Vec3::Z, Vec3::X, Vec3::Y, Vec3::new(-1.0, -1.0, 0.0)];
    let mesh = triangle_mesh(positions.clone(), vec![0, 1, 2, 0, 1, 3, 0, 1, 4]);
    let fine = subdivide(&mesh, Split::Smooth).unwrap().mesh;
    assert_eq!((fine.positions.len(), fine.triangles.len()), (5 + 7, 36));
    assert_eq!(fine.positions[5], Vec3::new(0.0, 0.0, 0.5), "the shared edge is cut in the middle");
    assert_eq!(fine.positions[..2], positions[..2], "its ends, where four creases meet, stay put");
    let finer = subdivide(&fine, Split::Smooth).unwrap().mesh;
    assert!(all_finite(&fine) && all_finite(&finer));
    // The line the sheets share is still a straight line shared by all three.
    assert_eq!(edge_census(finer.positions.len(), &finer.triangles).crowded, 4);

    // The same with quads: three leaves of a book.
    let mut positions = vec![Vec3::ZERO, Vec3::Z];
    for leaf in [Vec3::X, Vec3::Y, Vec3::new(-1.0, -1.0, 0.0)] {
        positions.extend([leaf + Vec3::Z, leaf]);
    }
    let book = polygon_mesh(positions, &[&[0, 1, 2, 3], &[0, 1, 4, 5], &[0, 1, 6, 7]]);
    let fine = subdivide(&book, Split::Smooth).unwrap().mesh;
    assert_eq!(fine.positions.len(), 8 + 10 + 3);
    assert_eq!(fine.positions[8], Vec3::new(0.0, 0.0, 0.5));
    let finer = subdivide(&fine, Split::Smooth).unwrap().mesh;
    assert!(all_finite(&fine) && all_finite(&finer));
}

#[test]
fn a_soup_of_random_triangles_comes_through_finite() {
    let mut rng = Rng(21);
    let positions: Vec<Vec3> = (0..60).map(|_| rng.vector()).collect();
    let triangles: Vec<u32> = (0..600).map(|_| (rng.next() * 60.0) as u32).collect();
    let mut mesh = triangle_mesh(positions, triangles);
    for _ in 0..3 {
        mesh = subdivide(&mesh, Split::Smooth).unwrap().mesh;
        assert!(all_finite(&mesh));
        // Smoothing only ever averages: nothing leaves the box the soup was in.
        assert!(mesh.positions.iter().all(|p| p.abs().max_element() <= 1.0));
    }
}

#[test]
fn data_that_does_not_fit_together_is_refused() {
    let refused =
        |vertices: usize, triangles: Vec<u32>, polys: Option<Polys>| Levels::new(vertices, triangles, polys).is_err();
    assert!(refused(3, vec![0, 1, 3], None), "a corner past the last vertex");
    assert!(refused(3, vec![0, 1], None), "not whole triangles");

    let quad = |start: Vec<u32>, indices: Vec<u32>| Some(Polys { start, indices });
    let triangles = vec![0, 1, 2, 0, 2, 3];
    assert!(!refused(4, triangles.clone(), quad(vec![0, 4], vec![0, 1, 2, 3])));
    assert!(!refused(4, vec![3, 0, 1, 1, 2, 3], quad(vec![0, 4], vec![0, 1, 2, 3])), "the other diagonal is as good");
    assert!(refused(4, triangles.clone(), quad(vec![0, 4], vec![0, 1, 2, 9])), "a corner past the last vertex");
    assert!(refused(4, triangles.clone(), quad(vec![0, 3], vec![0, 1, 2, 3])), "the face list stops short");
    assert!(refused(4, triangles.clone(), quad(vec![1, 4], vec![0, 1, 2, 3])), "the face list starts late");
    assert!(refused(4, triangles.clone(), quad(vec![0, 2, 4], vec![0, 1, 2, 3])), "faces of two corners");
    assert!(refused(4, vec![0, 1, 0, 0, 0, 3], quad(vec![0, 4], vec![0, 1, 0, 3])), "a face that repeats a corner");
    assert!(refused(4, vec![0, 1, 2], quad(vec![0, 4], vec![0, 1, 2, 3])), "half the quad has no triangle");
    assert!(refused(4, vec![0, 2, 1, 0, 3, 2], quad(vec![0, 4], vec![0, 1, 2, 3])), "triangles wound the other way");
    assert!(refused(4, vec![0, 1, 2, 0, 1, 2], quad(vec![0, 4], vec![0, 1, 2, 3])), "the same triangle twice");
    assert!(refused(5, vec![0, 1, 2, 0, 2, 3, 0, 3, 4], quad(vec![0, 4], vec![0, 1, 2, 3])), "a triangle in no face");
    assert!(refused(4, triangles, quad(vec![0, 3, 6], vec![0, 1, 2, 0, 2, 1])), "faces that are other triangles");

    let mut levels = start(&icosahedron());
    let ball = icosahedron();
    assert!(levels.subdivide(&ball.positions[..11], &ball.mask, Split::Smooth).is_err(), "a vertex short");
    assert!(levels.subdivide(&ball.positions, &ball.mask[..11], Split::Smooth).is_err(), "a mask value short");
    assert!(levels.step_to(1, &ball.positions, &ball.mask).is_err(), "no such level");
    assert!(levels.undo_subdivide().is_err(), "nothing to take back");
    assert_eq!((levels.level_count(), levels.current()), (1, 0), "a refusal changes nothing");
}
