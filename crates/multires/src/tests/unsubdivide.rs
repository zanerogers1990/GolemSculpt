//! Reconstruct Subdiv: meshes subdivided here, renumbered and reordered as a
//! file from elsewhere would have them, and their levels found again; and
//! meshes that were not subdivided, which must not be given levels.

use std::time::Instant;

use glam::Vec3;

use super::fixtures::*;
use crate::{LevelMesh, Polys, Split, reconstruct, unsubdivide};

/// A whole number below `n`.
fn pick(rng: &mut Rng, n: usize) -> usize {
    ((rng.next() * n as f32) as usize).min(n - 1)
}

/// The faces of `polys` with every vertex renumbered at random, the faces in
/// a random order and each started at a random corner (its winding kept), and
/// for each new vertex number the old one.
fn shuffled(polys: &Polys, vertex_count: usize, rng: &mut Rng) -> (Polys, Vec<u32>) {
    let mut old_of_new: Vec<u32> = (0..vertex_count as u32).collect();
    for i in (1..vertex_count).rev() {
        old_of_new.swap(i, pick(rng, i + 1));
    }
    let mut new_of_old = vec![0u32; vertex_count];
    for (new, &old) in old_of_new.iter().enumerate() {
        new_of_old[old as usize] = new as u32;
    }
    let mut order: Vec<usize> = (0..polys.face_count()).collect();
    for i in (1..order.len()).rev() {
        order.swap(i, pick(rng, i + 1));
    }
    let mut out = Polys { start: vec![0], indices: Vec::new() };
    for f in order {
        let face = &polys.indices[polys.start[f] as usize..polys.start[f + 1] as usize];
        let from = pick(rng, face.len());
        out.indices.extend((0..face.len()).map(|k| new_of_old[face[(k + from) % face.len()] as usize]));
        out.start.push(out.indices.len() as u32);
    }
    (out, old_of_new)
}

/// Every face, its corners named through `name` and started at the lowest
/// (so its winding still shows), in sorted order: two meshes with the same
/// faces give the same list however their faces are ordered.
fn face_set(polys: &Polys, name: impl Fn(u32) -> u32) -> Vec<Vec<u32>> {
    let mut faces: Vec<Vec<u32>> = polys
        .start
        .windows(2)
        .map(|w| {
            let face: Vec<u32> = polys.indices[w[0] as usize..w[1] as usize].iter().map(|&v| name(v)).collect();
            let low = (0..face.len()).min_by_key(|&k| face[k]).unwrap();
            (0..face.len()).map(|k| face[(k + low) % face.len()]).collect()
        })
        .collect();
    faces.sort();
    faces
}

/// A polygon mesh over `positions` with the faces of `polys`.
fn with_faces(positions: Vec<Vec3>, polys: &Polys) -> LevelMesh {
    let faces: Vec<&[u32]> = polys.start.windows(2).map(|w| &polys.indices[w[0] as usize..w[1] as usize]).collect();
    polygon_mesh(positions, &faces)
}

/// The cube with one corner cut off: three pentagons, three quads and a
/// triangle, wound outward.
fn cut_cube() -> LevelMesh {
    let mut positions = cube().positions;
    // Vertex 7, the corner (1, 1, 1), becomes the cut's point towards vertex 3; 8 and 9 are towards 5 and 6.
    positions[7] = Vec3::new(1.0, 1.0, 0.5);
    positions.extend([Vec3::new(1.0, 0.5, 1.0), Vec3::new(0.5, 1.0, 1.0)]);
    let faces: [&[u32]; 7] =
        [&[0, 2, 3, 1], &[4, 5, 8, 9, 6], &[0, 1, 5, 4], &[2, 6, 9, 7, 3], &[0, 4, 6, 2], &[1, 3, 7, 8, 5], &[9, 8, 7]];
    let mesh = polygon_mesh(positions, &faces);
    assert!(signed_volume(&mesh) > 0.0, "the cut cube is wound outward");
    mesh
}

/// Two meshes as one, the second moved along by `shift` and its vertices
/// numbered after the first's.
fn joined(a: &LevelMesh, b: &LevelMesh, shift: Vec3) -> LevelMesh {
    let (pa, pb) = (a.polys.as_ref().unwrap(), b.polys.as_ref().unwrap());
    let offset = a.positions.len() as u32;
    let mut polys = pa.clone();
    let corners = pa.indices.len() as u32;
    polys.start.extend(pb.start[1..].iter().map(|&s| s + corners));
    polys.indices.extend(pb.indices.iter().map(|&v| v + offset));
    let positions = a.positions.iter().copied().chain(b.positions.iter().map(|&p| p + shift)).collect();
    with_faces(positions, &polys)
}

/// Turns the edge between quad `f`'s first two corners: the two quads either
/// side of it are taken as the hexagon they make and cut along the next
/// diagonal round. Every face stays a quad, but the mesh is no longer one
/// that subdividing makes there.
fn turn_edge(polys: &mut Polys, f: usize) {
    let quad = |polys: &Polys, g: usize| -> [u32; 4] { std::array::from_fn(|k| polys.indices[g * 4 + k]) };
    let a = quad(polys, f);
    let (x, y) = (a[0], a[1]);
    let (g, from) = (0..polys.face_count())
        .flat_map(|g| (0..4).map(move |k| (g, k)))
        .find(|&(g, k)| quad(polys, g)[k] == y && quad(polys, g)[(k + 1) % 4] == x)
        .expect("an inside edge");
    let b: [u32; 4] = std::array::from_fn(|k| quad(polys, g)[(k + from) % 4]);
    // Round the hexagon: x, then b's far side, then y, then a's far side.
    let h = [x, b[2], b[3], y, a[2], a[3]];
    polys.indices[f * 4..f * 4 + 4].copy_from_slice(&[h[1], h[2], h[3], h[4]]);
    polys.indices[g * 4..g * 4 + 4].copy_from_slice(&[h[4], h[5], h[0], h[1]]);
}

/// Raises `base` `splits` times, renumbers and reorders the finest level
/// (unless `rng` is `None`), and checks that what is reconstructed from it
/// has every level the stack that made it had, face for face, and `extra`
/// more below those.
fn round_trip(name: &str, base: &LevelMesh, splits: usize, extra: usize, rng: Option<&mut Rng>) {
    let (made, top) = raised(base, &vec![Split::Smooth; splits]);
    let (given, old_of_new) = match rng {
        Some(rng) => shuffled(top.polys.as_ref().unwrap(), top.positions.len(), rng),
        None => (top.polys.clone().unwrap(), (0..top.positions.len() as u32).collect()),
    };
    let rebuilt = reconstruct(top.positions.len(), &given).unwrap_or_else(|| panic!("{name} raised {splits}: none"));
    assert_eq!(rebuilt.levels.level_count(), splits + 1 + extra, "{name} raised {splits}");
    assert_eq!(rebuilt.levels.current(), rebuilt.levels.top(), "{name}: handed back on the finest level");
    assert_eq!(
        face_set(&rebuilt.polys, |v| rebuilt.vertex[v as usize]),
        face_set(&given, |v| v),
        "{name} raised {splits}: the finest level is the mesh given"
    );
    let made_vertex = |v: u32| old_of_new[rebuilt.vertex[v as usize] as usize];
    for level in 0..=splits {
        assert_eq!(
            face_set(&rebuilt.levels.polys(level + extra).unwrap(), made_vertex),
            face_set(&made.polys(level).unwrap(), |v| v),
            "{name} raised {splits}: level {level} is the level it was made from"
        );
    }
}

#[test]
fn a_subdivided_cube_comes_back_a_cube_and_not_an_octahedron() {
    let (_, top) = raised(&cube(), &[Split::Smooth]);
    let coarser = unsubdivide(top.positions.len(), top.polys.as_ref().unwrap()).expect("a subdivided cube");
    // Its corners kept, numbered as they were; six faces of four.
    assert_eq!(coarser.kept, (0..8).collect::<Vec<u32>>());
    assert_eq!(face_set(&coarser.polys, |v| v), face_set(cube().polys.as_ref().unwrap(), |v| v));
}

#[test]
fn renumbered_and_reordered_subdivided_meshes_give_back_every_level() {
    let mut rng = Rng(7);
    let shapes = [
        ("a cube", cube()),
        ("a cube with a corner cut off", cut_cube()),
        ("an open sheet", quad_grid(3, 2)),
        ("two pieces", joined(&cube(), &cut_cube(), Vec3::new(4.0, 0.0, 0.0))),
    ];
    for (name, base) in &shapes {
        for splits in 1..=3 {
            round_trip(name, base, splits, 0, Some(&mut rng));
        }
    }
}

#[test]
fn a_torus_goes_down_as_far_as_its_grid_halves_evenly() {
    // 8 by 6 is itself 4 by 3 subdivided, and 3 rows do not halve: one level
    // more than the stack had, and no more. Numbered as subdividing numbers
    // it, the levels are the ones it was made from.
    round_trip("a torus", &quad_torus(8, 6), 2, 1, None);
    // Renumbered, every level is the grid shifted one way or another, all as
    // good: what is checked is that the stack's finest level is the mesh.
    let (_, top) = raised(&quad_torus(8, 6), &[Split::Smooth; 2]);
    let (given, _) = shuffled(top.polys.as_ref().unwrap(), top.positions.len(), &mut Rng(3));
    let rebuilt = reconstruct(top.positions.len(), &given).unwrap();
    assert_eq!(rebuilt.levels.level_count(), 4);
    assert_eq!(face_set(&rebuilt.polys, |v| rebuilt.vertex[v as usize]), face_set(&given, |v| v));
    assert_eq!(rebuilt.levels.polys(0).unwrap().face_count(), 12);
}

#[test]
fn a_mesh_that_was_not_subdivided_has_no_levels_below_it() {
    let sheet = quad_grid(5, 5);
    assert!(reconstruct(sheet.positions.len(), sheet.polys.as_ref().unwrap()).is_none(), "an odd grid");
    let cube = cube();
    assert!(reconstruct(8, cube.polys.as_ref().unwrap()).is_none(), "a cube is as coarse as it goes");
    // A subdivided mesh with one edge turned on its finest level.
    let (_, top) = raised(&cut_cube(), &[Split::Smooth; 2]);
    let mut polys = top.polys.clone().unwrap();
    turn_edge(&mut polys, 17);
    assert!(reconstruct(top.positions.len(), &polys).is_none(), "an edge turned");
}

#[test]
fn an_edge_turned_partway_up_stops_the_levels_there() {
    let (_, once) = raised(&cube(), &[Split::Smooth]);
    let mut polys = once.polys.clone().unwrap();
    turn_edge(&mut polys, 5);
    let turned = with_faces(once.positions.clone(), &polys);
    round_trip("a cube with an edge turned", &turned, 2, 0, Some(&mut Rng(5)));
}

#[test]
fn triangles_among_the_quads_are_refused() {
    let cut = cut_cube();
    assert!(unsubdivide(cut.positions.len(), cut.polys.as_ref().unwrap()).is_none());
    // A subdivided cube with one quad given as two triangles.
    let (_, top) = raised(&cube(), &[Split::Smooth; 2]);
    let polys = top.polys.unwrap();
    let mut faces: Vec<Vec<u32>> =
        polys.start.windows(2).map(|w| polys.indices[w[0] as usize..w[1] as usize].to_vec()).collect();
    let q = faces.remove(10);
    faces.extend([vec![q[0], q[1], q[2]], vec![q[0], q[2], q[3]]]);
    let mixed = polygon_mesh(top.positions, &faces.iter().map(|f| &f[..]).collect::<Vec<_>>());
    assert!(reconstruct(mixed.positions.len(), mixed.polys.as_ref().unwrap()).is_none());
}

#[test]
fn the_levels_found_step_down_to_the_kept_vertices_and_back_up_exactly() {
    let mut rng = Rng(9);
    let (mut made, mut top) = raised(&cut_cube(), &[Split::Smooth; 3]);
    roughen(&mut top, 0.01, &mut rng);
    let (given, old_of_new) = shuffled(top.polys.as_ref().unwrap(), top.positions.len(), &mut rng);
    let rebuilt = reconstruct(top.positions.len(), &given).unwrap();
    // The finest level's positions and mask in the stack's numbering.
    let made_vertex = |v: u32| old_of_new[rebuilt.vertex[v as usize] as usize] as usize;
    let positions: Vec<Vec3> = (0..rebuilt.vertex.len() as u32).map(|v| top.positions[made_vertex(v)]).collect();
    let mask: Vec<f32> = (0..rebuilt.vertex.len() as u32).map(|v| top.mask[made_vertex(v)]).collect();
    let mut levels = rebuilt.levels;
    let down = levels.step_to(0, &positions, &mask).unwrap().mesh;
    let made_down = made.step_to(0, &top.positions, &top.mask).unwrap().mesh;
    assert_eq!(down.positions.len(), 10);
    for (v, p) in down.positions.iter().enumerate() {
        assert!(same_bits(&[*p], &[made_down.positions[made_vertex(v as u32)]]), "corner {v} is where it was");
    }
    let up = levels.step_to(3, &down.positions, &down.mask).unwrap().mesh;
    assert!(same_bits(&up.positions, &positions), "back up, the finest level is as it was given, to the bit");
    assert_eq!(up.mask, mask);
}

/// `cargo test -p sculpt-multires --release -- --ignored --nocapture reconstruct_timing`
#[test]
#[ignore]
fn reconstruct_timing() {
    // A cube subdivided ten times: 6.3 million quads, poles at its corners.
    let (_, top) = raised(&cube(), &[Split::Smooth; 10]);
    let polys = top.polys.unwrap();
    let (shuffled, _) = shuffled(&polys, top.positions.len(), &mut Rng(1));
    println!(
        "{} threads, {} quads, {} vertices",
        rayon::current_num_threads(),
        polys.face_count(),
        top.positions.len()
    );
    // As subdividing numbers it, near vertices have near numbers, as in the
    // app's own meshes; renumbered at random, nearly every look is a cache miss.
    for (name, given) in [("as numbered", &polys), ("renumbered at random", &shuffled)] {
        let start = Instant::now();
        let one = unsubdivide(top.positions.len(), given).unwrap();
        let step = start.elapsed().as_secs_f64() * 1000.0;
        drop(one);
        let start = Instant::now();
        let rebuilt = reconstruct(top.positions.len(), given).unwrap();
        let all = start.elapsed().as_secs_f64() * 1000.0;
        println!("  {name}: one step down {step:.0} ms; every level and the check {all:.0} ms");
        assert_eq!(rebuilt.levels.level_count(), 11);
    }
}
