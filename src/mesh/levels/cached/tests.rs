//! The finest level's mesh kept for the way back up: the same mesh a fresh
//! build makes, dropped whenever it would no longer be, and given up when
//! its index has grown too loose.

use std::sync::Arc;

use glam::{Quat, Vec3};
use sculpt_multires::Split;

use super::super::{MeshLevels, stepped, subdivided, without};
use super::*;
use crate::mesh::primitives::{self, Primitive};

fn levels(mesh: &Mesh) -> &MeshLevels {
    mesh.levels.as_deref().expect("the mesh has levels")
}

/// A ball subdivided three times and sculpted on its finest level after it
/// was built, so a fresh build of it would order it differently from the
/// build that is kept. With a mask, hidden faces, groups and a layer.
fn dressed() -> Mesh {
    let mut mesh = primitives::build(Primitive::Sphere, 800, 0.5);
    for _ in 0..3 {
        mesh = subdivided(&mesh, Split::Smooth).unwrap().0;
    }
    let all = |mesh: &mut Mesh, to: &(dyn Fn(Vec3) -> Vec3 + Sync)| {
        let leaves: Vec<u32> = (0..mesh.bvh.leaves.len() as u32).collect();
        mesh.begin_stroke();
        mesh.move_vertices(&leaves, |_, vertex| Some(to(vertex.pos)));
        mesh.positions_changed(&leaves);
        let _ = mesh.end_stroke(false);
    };
    all(&mut mesh, &|p| p + p.normalize() * 0.02 * (p.x * 31.0).sin() * (p.y * 17.0).cos());
    let (layer, _) = mesh.add_layer(Some("Pores".into()));
    all(&mut mesh, &|p| p + Vec3::Z * 0.005 * (p.y * 60.0).sin());
    mesh.record_on_layer(None);
    let _ = mesh.set_layer(layer, true, 60);
    let _ = mesh.remap_mask(|mesh, v, _| (mesh.positions[v as usize].x * 4.0).clamp(0.0, 1.0));
    let hide: Vec<u32> = (0..mesh.triangle_count() as u32).filter(|t| t % 9 == 0).collect();
    let _ = mesh.hide(&hide);
    mesh.set_group_ids((0..mesh.triangle_count()).map(|t| (t % 4) as u16).collect());
    let _ = mesh.take_dirty();
    mesh
}

/// The same level change twice over: once as it is, once with what was kept
/// of the finest level thrown away, so that it builds afresh.
fn both_ways(low: impl Fn() -> Mesh, to: usize) -> (Mesh, Mesh) {
    let kept = stepped(&low(), to).unwrap().0;
    let mut fresh = low();
    fresh.levels.as_mut().unwrap().top_mesh = None;
    (kept, stepped(&fresh, to).unwrap().0)
}

/// Everything a person could tell apart, in the stack's own order: the
/// levels' print (positions, mask, hidden faces, groups), the layers', and
/// every normal to the bit. Gives back how many normals differ.
fn compare(kept: &Mesh, fresh: &Mesh, what: &str) -> usize {
    assert_eq!(levels(kept).print(kept), levels(fresh).print(fresh), "{what}");
    assert_eq!(kept.layers_print(), fresh.layers_print(), "{what}");
    let normals = |mesh: &Mesh| -> Vec<[u32; 3]> {
        levels(mesh).vertex.iter().map(|&v| mesh.normals[v as usize].to_array().map(f32::to_bits)).collect()
    };
    let (a, b) = (normals(kept), normals(fresh));
    a.iter().zip(&b).filter(|(a, b)| a != b).count()
}

#[test]
fn the_kept_finest_mesh_makes_the_mesh_a_fresh_build_makes() {
    let top = dressed();
    let finest = levels(&top).top();
    let mut differing = 0;
    for low in [0, 1, finest - 1] {
        let down = || stepped(&top, low).unwrap().0;
        assert!(levels(&down()).top_mesh.is_some(), "the way down keeps it");
        let (kept, fresh) = both_ways(down, finest);
        // Kept, the mesh has the triangles in the order the first build put them.
        assert_eq!(kept.indices, top.indices, "level {low}: the kept build was used");
        assert!(levels(&kept).top_mesh.is_none(), "on the finest level nothing is kept for it");
        differing += compare(&kept, &fresh, &format!("up from level {low}, nothing edited"));

        // An edit carried up, small and large.
        for (edit, turn) in [("a dab", 0.0f32), ("a twist", 0.8)] {
            let edited = || {
                let mut mesh = down();
                let twist = Quat::from_rotation_y(turn);
                let leaves: Vec<u32> = (0..mesh.bvh.leaves.len() as u32).collect();
                mesh.begin_stroke();
                mesh.move_vertices(&leaves, |_, vertex| {
                    let p = vertex.pos;
                    let d = p.distance(Vec3::Z * 0.5);
                    let p = if d < 0.15 { p * 1.03 } else { p };
                    Some(if p.y > 0.0 { twist * p } else { p })
                });
                mesh.positions_changed(&leaves);
                let _ = mesh.end_stroke(false);
                mesh
            };
            let (kept, fresh) = both_ways(edited, finest);
            differing += compare(&kept, &fresh, &format!("{edit} on level {low}, up"));
        }
    }
    assert_eq!(differing, 0, "normals that differ from a fresh build's");
}

#[test]
fn what_is_kept_goes_when_the_finest_level_changes() {
    let top = dressed();
    let low = stepped(&top, 1).unwrap().0;
    // Delete Higher on level 2 of 4: level 2 is the finest now, and its mesh is this one.
    let mut cut = stepped(&top, 1).unwrap().0;
    cut.levels = without(&cut, false);
    assert!(levels(&cut).top_mesh.is_none());
    // Subdividing from there makes a new finest level, built afresh; down again keeps that one.
    let again = subdivided(&cut, Split::Flat).unwrap().0;
    assert!(levels(&again).top_mesh.is_none());
    let down = stepped(&again, 0).unwrap().0;
    let (kept, fresh) = both_ways(|| stepped(&again, 0).unwrap().0, 2);
    assert_eq!(kept.indices, again.indices);
    assert_eq!(compare(&kept, &fresh, "after Delete Higher and Subdivide"), 0);
    assert!(levels(&down).top_mesh.is_some());

    // Delete Lower leaves the finest level as it was, so what was kept still serves.
    let mut fewer = stepped(&top, 1).unwrap().0;
    fewer.levels = without(&fewer, true);
    assert!(levels(&fewer).top_mesh.is_some());
    let (kept, fresh) = both_ways(
        || {
            let mut mesh = stepped(&top, 1).unwrap().0;
            mesh.levels = without(&mesh, true);
            mesh
        },
        2,
    );
    assert_eq!(kept.indices, top.indices);
    assert_eq!(compare(&kept, &fresh, "after Delete Lower"), 0);

    // A copy of the object can step up with it too.
    let (copy, numbering) = Mesh::new_numbered(low.positions.clone(), low.indices.clone(), Some(low.mask.clone()), &[]);
    let copied = levels(&low).renumbered(&copy, &numbering).unwrap();
    assert!(copied.top_mesh.is_some());
    drop(copied);

    // What it is charged: its share, once between the meshes that hold it.
    let other = stepped(&low, 0).unwrap().0;
    let top_bytes = levels(&low).top_mesh.as_ref().unwrap().bytes();
    let share = |mesh: &Mesh| levels(mesh).top_mesh.as_ref().map_or(0, |t| t.bytes() / Arc::strong_count(t));
    assert!(share(&low) + share(&other) <= top_bytes && share(&low) + share(&other) + 2 >= top_bytes,
        "{} + {} against {top_bytes}",
        share(&low),
        share(&other));
}

#[test]
fn an_index_grown_too_loose_is_built_afresh() {
    let top = dressed();
    let low = stepped(&top, 0).unwrap().0;
    let cache = levels(&low).top_mesh.clone().unwrap();
    let stack = &levels(&low).stack;
    let finest = stack.top();
    let mut climb = stack.clone();
    let (positions, mask, _) =
        climb.step_lists(finest, &levels(&low).ordered_positions(&low), &vec![0.0; low.vertex_count()]).unwrap();
    assert!(cache.mesh(&positions, &mask, &[]).is_some());
    // The same points shuffled: every box of the old tree now spans the whole model.
    let shuffled: Vec<Vec3> = (0..positions.len()).map(|i| positions[(i * 7919) % positions.len()]).collect();
    assert!(cache.mesh(&shuffled, &mask, &[]).is_none());

    // How loose a big change of form made low down leaves it: the levels demo's bend.
    let bent = |p: Vec3| {
        let up = ((p.y + 0.1) / 0.6).clamp(0.0, 1.0);
        let lean = Quat::from_rotation_z(-0.9 * up * up);
        let swell = 1.0 + 0.5 * (-((p - Vec3::new(-0.45, -0.1, 0.1)).length_squared()) / 0.08).exp();
        lean * (p * swell) + Vec3::new(0.25 * up, 0.0, 0.0)
    };
    let mut low = low;
    let leaves: Vec<u32> = (0..low.bvh.leaves.len() as u32).collect();
    low.begin_stroke();
    low.move_vertices(&leaves, |_, vertex| Some(bent(vertex.pos)));
    low.positions_changed(&leaves);
    let _ = low.end_stroke(false);
    let up = stepped(&low, finest).unwrap().0;
    let grown = spread(&up.bvh) / cache.built_spread;
    println!("the bend leaves the index {grown:.3} times as loose");
    assert!(grown < LOOSEST && up.indices == top.indices, "a bend keeps the kept build ({grown})");
}
