//! Reconstruct Subdiv on a mesh: a quad model subdivided here, brought back
//! in the way a file from elsewhere brings it (one level, numbered its own
//! way, its quads cut along either diagonal), and its levels found again.

use glam::Vec3;
use sculpt_multires::Split;

use super::*;
use crate::mesh::levels::{stepped, subdivided};
use crate::mesh::primitives::{self, Primitive};

/// A cube of six quads, each cut in two as a quad remesh leaves its faces.
fn quad_cube() -> Mesh {
    let sign = |bit: u32| if bit == 0 { -0.5 } else { 0.5 };
    let positions: Vec<Vec3> = (0..8u32).map(|i| Vec3::new(sign(i & 1), sign(i >> 1 & 1), sign(i >> 2 & 1))).collect();
    let quads: [[u32; 4]; 6] = [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];
    let triangles: Vec<u32> = quads.iter().flat_map(|q| [q[0], q[1], q[2], q[0], q[2], q[3]]).collect();
    let (mut mesh, moved) = Mesh::new_mapped(positions, triangles, None);
    let corners = quads.iter().flatten().map(|&v| moved[v as usize]).collect();
    mesh.polys = Some(Polys { start: (0..=6).map(|f| f * 4).collect(), indices: corners });
    mesh
}

/// The cube subdivided `count` times and sculpted all over by rule, with a
/// mask painted in stripes.
fn sculpted(count: usize) -> Mesh {
    let mut top = quad_cube();
    for _ in 0..count {
        top = subdivided(&top, Split::Smooth).unwrap().0;
    }
    for v in 0..top.positions.len() {
        let p = top.positions[v];
        top.positions[v] = p + p.normalize() * 0.02 * (p.x * 37.0).sin() * (p.y * 23.0).cos();
        top.mask[v] = if (p.z * 9.0).sin() > 0.3 { 0.75 } else { 0.0 };
    }
    top
}

/// `mesh`'s level as a file from elsewhere brings it in: no levels, its
/// vertices numbered the other way round, and each quad started `turn`
/// corners on, so that with an odd `turn` its triangles are cut along the
/// other diagonal from the one the levels cut it along.
fn brought_in(mesh: &Mesh, turn: usize) -> Mesh {
    let n = mesh.vertex_count();
    let rename = |v: u32| (n - 1) as u32 - v;
    let quads: Vec<[u32; 4]> = mesh
        .polys
        .as_ref()
        .unwrap()
        .indices
        .chunks_exact(4)
        .map(|q| std::array::from_fn(|k| rename(q[(k + turn) % 4])))
        .collect();
    let positions = (0..n).rev().map(|v| mesh.positions[v]).collect();
    let mask = (0..n).rev().map(|v| mesh.mask[v]).collect();
    let triangles = quads.iter().flat_map(|q| [q[0], q[1], q[2], q[0], q[2], q[3]]).collect();
    let (mut given, moved) = Mesh::new_mapped(positions, triangles, Some(mask));
    let corners = quads.iter().flatten().map(|&v| moved[v as usize]).collect();
    given.polys = Some(Polys { start: (0..=quads.len() as u32).map(|f| f * 4).collect(), indices: corners });
    given
}

/// Every vertex's position and mask to the bit, sorted: the same for the
/// same model however it is numbered.
fn points(mesh: &Mesh) -> Vec<[u32; 4]> {
    let mut points: Vec<[u32; 4]> = mesh
        .positions
        .iter()
        .zip(&mesh.mask)
        .map(|(p, m)| [p.x.to_bits(), p.y.to_bits(), p.z.to_bits(), m.to_bits()])
        .collect();
    points.sort_unstable();
    points
}

/// Every quad as its corners' positions (sorted, to the bit), the group its
/// triangles are in and whether all of them are hidden, sorted: the same for
/// the same model however it is numbered and its quads cut.
fn quads_seen(mesh: &Mesh) -> Vec<(Vec<[u32; 3]>, u16, bool)> {
    let polys = mesh.polys.as_ref().unwrap();
    let cut = quad_cut(mesh, polys).expect("every triangle in a quad");
    let hidden = mesh.hidden_triangles();
    let mut seen: Vec<_> = cut
        .iter()
        .enumerate()
        .map(|(q, two)| {
            let mut corners: Vec<[u32; 3]> = polys.indices[q * 4..q * 4 + 4]
                .iter()
                .map(|&v| mesh.positions[v as usize].to_array().map(f32::to_bits))
                .collect();
            corners.sort_unstable();
            let group = if mesh.has_groups() { mesh.group_ids()[two[0] as usize] } else { 0 };
            if mesh.has_groups() {
                assert_eq!(group, mesh.group_ids()[two[1] as usize], "a quad's two triangles are in one group");
            }
            let all_hidden = hidden.is_some_and(|flags| two.iter().all(|&t| flags[t as usize]));
            (corners, group, all_hidden)
        })
        .collect();
    seen.sort();
    seen
}

fn same_bits(a: &[Vec3], b: &[Vec3]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| a.to_array().map(f32::to_bits) == b.to_array().map(f32::to_bits))
}

#[test]
fn a_subdivided_model_brought_in_gets_its_levels_back_and_nothing_moves() {
    let made = sculpted(3);
    let made_low = stepped(&made, 0).unwrap().0;
    for turn in [0, 1] {
        let given = brought_in(&made, turn);
        assert_eq!(given.level_count(), 1);
        let (mesh, found) = reconstructed(&given).unwrap();
        assert_eq!(found, Reconstructed { below: 3, coarsest_faces: 6 }, "cut from corner {turn}");
        let levels = mesh.levels.as_deref().unwrap();
        assert_eq!((levels.count(), levels.current()), (4, 3));
        assert_eq!(points(&mesh), points(&given), "every vertex and its mask where it was");
        assert_eq!(face_count(mesh.polys.as_ref().unwrap()), 6 * 64);

        // Down to the cube: its corners where the sculpt has them, as the
        // levels that made the model step down to them.
        let low = stepped(&mesh, 0).unwrap().0;
        assert_eq!(face_count(low.polys.as_ref().unwrap()), 6);
        assert_eq!(points(&low), points(&made_low));

        // And back up: the model as it was brought in, to the bit.
        let up = stepped(&low, 3).unwrap().0;
        let order = |m: &Mesh| m.levels.as_deref().unwrap().ordered_positions(m);
        assert!(same_bits(&order(&up), &order(&mesh)));
        assert_eq!(points(&up), points(&given));
    }
}

#[test]
fn groups_and_hidden_faces_stay_on_the_quads_they_were_on() {
    let made = sculpted(2);
    for turn in [0, 1] {
        let mut given = brought_in(&made, turn);
        let cut = quad_cut(&given, given.polys.as_ref().unwrap()).unwrap();
        let mut ids = vec![0u16; given.triangle_count()];
        let mut hide = Vec::new();
        for (q, two) in cut.iter().enumerate() {
            for &t in two {
                ids[t as usize] = (q % 5) as u16 + 1;
                if q % 7 == 0 {
                    hide.push(t);
                }
            }
        }
        given.set_group_ids(ids);
        given.hide(&hide);
        let (mesh, _) = reconstructed(&given).unwrap();
        assert_eq!(mesh.hidden_triangle_count(), hide.len(), "cut from corner {turn}");
        assert_eq!(quads_seen(&mesh), quads_seen(&given), "cut from corner {turn}");
        // Down a level and back, they are still where they were.
        let back = stepped(&stepped(&mesh, 0).unwrap().0, 2).unwrap().0;
        assert_eq!(quads_seen(&back), quads_seen(&given));
    }
}

#[test]
fn what_cannot_be_reconstructed_is_refused_in_plain_words() {
    let ball = primitives::build(Primitive::Sphere, 2_000, 0.5);
    assert!(reconstructed(&ball).err().unwrap().contains("This one is triangles"));
    let cube = quad_cube();
    assert!(reconstructed(&cube).err().unwrap().contains("isn't a subdivided quad mesh"));
    let made = sculpted(1);
    assert!(reconstructed(&made).err().unwrap().contains("levels already"));
    // A quad model with one quad given as two triangles.
    let mut mixed = brought_in(&made, 0);
    let polys = mixed.polys.as_mut().unwrap();
    let q: Vec<u32> = polys.indices[..4].to_vec();
    polys.indices.splice(..4, [q[0], q[1], q[2], q[0], q[2], q[3]]);
    polys.start = [0, 3].into_iter().chain((0..polys.start.len() as u32 - 1).map(|f| f * 4 + 6)).collect();
    assert!(reconstructed(&mixed).err().unwrap().contains("all quads"));
}
