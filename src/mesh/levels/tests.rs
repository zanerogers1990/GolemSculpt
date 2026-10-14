//! A mesh and its subdivision levels: what a change of level makes, what
//! follows it, and what undo puts back.

use glam::{Quat, Vec2, Vec3};

use super::*;
use crate::camera::Camera;
use crate::history::History;
use crate::mesh::primitives::{self, Primitive};
use crate::scene::Scene;
use crate::sculpt::{Brush, PointerSample, Sculptor};

/// One real brush stroke across the middle of the view, straight on from
/// +Z, through the routines a pen stroke runs. Returns how many vertices
/// it moved.
fn stroke(mesh: &mut Mesh, brush: Brush, radius_px: f32, strength: f32) -> usize {
    let size = Vec2::new(1600.0, 900.0);
    let mut camera = Camera::default();
    camera.frame(mesh.bounds(), size.x / size.y, false);
    let view = camera.view(Vec2::ZERO, size, camera.distance, 1.0);
    let mut sculptor = Sculptor::default();
    sculptor.settings.brush = brush;
    sculptor.settings.symmetry = [false; 3];
    sculptor.settings.radius_px = radius_px;
    sculptor.settings.per_brush[brush.index()].strength = strength;
    let sample = |i: usize| PointerSample {
        pos: size * 0.5 + Vec2::new(-90.0 + 3.0 * i as f32, 20.0),
        pressure: None,
        time: i as f64 / 240.0,
    };
    assert!(sculptor.begin(mesh, &view, sample(0), false, false), "the stroke missed the mesh");
    for i in 1..=60 {
        sculptor.sample(mesh, &view, sample(i));
        if i % 4 == 0 {
            sculptor.frame(mesh, &view, 1.0 / 60.0);
            let _ = mesh.take_dirty();
        }
    }
    let delta = sculptor.end(mesh, &view);
    let _ = mesh.take_dirty();
    match delta {
        Some(crate::mesh::StrokeDelta::Positions { verts, .. }) => verts.len(),
        _ => 0,
    }
}

fn ball() -> Mesh {
    primitives::build(Primitive::Sphere, 2_000, 0.5)
}

/// A cube of six quads, each cut in two for the engine as a quad remesh
/// leaves its faces.
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

fn levels(mesh: &Mesh) -> &MeshLevels {
    mesh.levels.as_deref().expect("the mesh has levels")
}

/// The mesh's positions and mask in its level's own numbering, which is the
/// same however the mesh happened to be built.
fn arrays(mesh: &Mesh) -> (Vec<Vec3>, Vec<f32>) {
    levels(mesh).level_arrays(mesh)
}

/// The mesh's hidden faces in its level's own order.
fn hidden(mesh: &Mesh) -> Vec<bool> {
    levels(mesh).level_hidden(mesh).map_or(vec![false; mesh.triangle_count()], |flags| flags.to_vec())
}

fn same_bits(a: &[Vec3], b: &[Vec3]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| a.to_array().map(f32::to_bits) == b.to_array().map(f32::to_bits))
}

/// Subdivides `count` times and hands back the finest mesh.
fn raised(mesh: &Mesh, count: usize) -> Mesh {
    let mut top = subdivided(mesh, Split::Smooth).unwrap().0;
    for _ in 1..count {
        top = subdivided(&top, Split::Smooth).unwrap().0;
    }
    top
}

/// Sculpts a mesh all over, by rule: every vertex pushed a little along a
/// direction that depends on where it is, and a mask painted in stripes.
fn sculpt(mesh: &mut Mesh) {
    for v in 0..mesh.positions.len() {
        let p = mesh.positions[v];
        mesh.positions[v] = p + p.normalize() * 0.01 * (p.x * 37.0).sin() * (p.y * 23.0).cos();
        mesh.mask[v] = if (p.z * 9.0).sin() > 0.3 { 0.75 } else { 0.0 };
    }
}

#[test]
fn subdivide_makes_a_finer_mesh_and_leaves_the_old_one_alone() {
    let base = ball();
    let before = base.positions.clone();
    let (fine, note) = subdivided(&base, Split::Smooth).unwrap();
    assert_eq!(note, Subdivided::default());
    assert!(
        base.levels.is_none() && same_bits(&base.positions, &before),
        "the mesh subdivided is what undo goes back to"
    );
    assert_eq!((base.level_count(), fine.level_count()), (1, 2));
    assert_eq!(fine.triangle_count(), base.triangle_count() * 4);
    assert_eq!(fine.triangle_count(), triangles_after(&base));
    let l = levels(&fine);
    assert_eq!((l.count(), l.current(), l.top(), l.scheme()), (2, 1, 1, Scheme::Loop));
    assert_eq!((l.triangle_count(0), l.triangle_count(1)), (base.triangle_count(), fine.triangle_count()));
    assert!(!l.is_flat(1));
    // A mesh like any other: closed, and every vertex used.
    assert_eq!(fine.triangle_count(), 2 * fine.vertex_count() - 4);
    assert!(fine.polys.is_none());

    let (flat, _) = subdivided(&base, Split::Flat).unwrap();
    assert!(levels(&flat).is_flat(1));
    assert!(same_bits(&arrays(&flat).0[..base.vertex_count()], &base.positions), "a flat split moves nothing");
}

#[test]
fn stepping_down_and_back_up_gives_back_the_sculpt_bit_for_bit() {
    for base in [ball(), quad_cube()] {
        let mut top = raised(&base, 3);
        sculpt(&mut top);
        top.hide(&(0..top.triangle_count() as u32).filter(|t| t % 7 < 3).collect::<Vec<_>>());
        let (positions, mask) = arrays(&top);
        let flags = hidden(&top);
        for route in [&[0, 3][..], &[2, 1, 0, 1, 2, 3], &[1, 3], &[0, 2, 0, 3]] {
            let mut mesh = stepped(&top, route[0]).unwrap().0;
            for &level in &route[1..] {
                assert_eq!(mesh.triangle_count(), levels(&mesh).triangle_count(levels(&mesh).current()));
                mesh = stepped(&mesh, level).unwrap().0;
            }
            let (now, now_mask) = arrays(&mesh);
            assert!(same_bits(&now, &positions), "positions by way of {route:?}");
            assert_eq!(
                now_mask.iter().map(|m| m.to_bits()).collect::<Vec<_>>(),
                mask.iter().map(|m| m.to_bits()).collect::<Vec<_>>()
            );
            assert_eq!(hidden(&mesh), flags, "hidden faces by way of {route:?}");
            assert_eq!(mesh.polys.is_some(), top.polys.is_some());
        }
        // A coarse level sits on the sculpt: its vertices are the fine vertices of the same numbers.
        let coarse = stepped(&top, 1).unwrap().0;
        let (low, low_mask) = arrays(&coarse);
        assert!(same_bits(&low, &positions[..low.len()]));
        assert_eq!(low_mask, mask[..low.len()]);
    }
}

#[test]
fn a_coarse_level_turned_takes_the_fine_sculpt_round_with_it() {
    let mut top = raised(&ball(), 2);
    sculpt(&mut top);
    let (before, _) = arrays(&top);
    let mut coarse = stepped(&top, 0).unwrap().0;
    let turn = Quat::from_axis_angle(Vec3::new(0.3, 1.0, -0.2).normalize(), 0.8);
    coarse.positions.iter_mut().for_each(|p| *p = turn * *p);
    let (after, _) = arrays(&stepped(&coarse, 2).unwrap().0);
    let off = before.iter().zip(&after).map(|(was, now)| (turn * *was).distance(*now)).fold(0.0, f32::max);
    assert!(off < 1e-5, "{off}");
}

#[test]
fn hidden_faces_follow_a_change_of_level() {
    let top = raised(&ball(), 2);
    // Up: every face made from a hidden face is hidden.
    let mut coarse = stepped(&top, 0).unwrap().0;
    coarse.hide(&[3, 4, 50]);
    let was = hidden(&coarse);
    let fine = stepped(&coarse, 2).unwrap().0;
    assert_eq!(fine.hidden_triangle_count(), 3 * 16);
    let parents = |mesh: &Mesh, level: usize| levels(mesh).stack.parent_triangles(level).unwrap();
    let (first, second) = (parents(&fine, 1), parents(&fine, 2));
    for (t, &flag) in hidden(&fine).iter().enumerate() {
        assert_eq!(flag, was[first[second[t] as usize] as usize], "triangle {t}");
    }
    // Down: a face is hidden only when all it was made into is. Show one
    // fine face of a hidden coarse face again, and that coarse face shows.
    let mut fine = fine;
    let shown = hidden(&fine).iter().position(|&flag| flag).unwrap();
    let on_mesh = levels(&fine).triangle.iter().position(|&t| t as usize == shown).unwrap();
    fine.show(&[on_mesh as u32]);
    let coarse = stepped(&fine, 0).unwrap().0;
    assert_eq!(coarse.hidden_triangle_count(), 2);
    let now = hidden(&coarse);
    let parent = first[second[shown] as usize] as usize;
    assert!(was[parent] && !now[parent]);
    assert_eq!((0..was.len()).filter(|&t| was[t] != now[t]).count(), 1);
    // And back up, the patch is as it was left on the fine level, not as coarse as the level in between.
    assert_eq!(stepped(&coarse, 2).unwrap().0.hidden_triangle_count(), 3 * 16 - 1);
    // Unless what is hidden was changed down here: then that is what is carried up.
    let mut coarse = coarse;
    coarse.show_all();
    assert_eq!(stepped(&coarse, 2).unwrap().0.hidden_triangle_count(), 0);
}

#[test]
fn a_mesh_of_quads_keeps_quads_on_every_level() {
    let cube = quad_cube();
    assert_eq!(cube.level_count(), 1);
    let mut top = raised(&cube, 2);
    assert_eq!(levels(&top).scheme(), Scheme::CatmullClark);
    for level in [2, 1, 0, 2] {
        top = stepped(&top, level).unwrap().0;
        assert_eq!(top.level_count(), 3);
        let polys = top.polys.as_ref().expect("quads at every level");
        let faces = polys.start.len() - 1;
        assert_eq!(faces, 6 * 4usize.pow(level as u32), "level {level}");
        assert_eq!(top.triangle_count(), faces * 2);
        assert!(polys.start.windows(2).all(|w| w[1] - w[0] == 4));
        assert!(polys.indices.iter().all(|&v| (v as usize) < top.vertex_count()));
        // Every triangle the engine draws lies in one of the quads.
        let key = |t: &[u32]| {
            let mut corners = [t[0], t[1], t[2]];
            corners.sort_unstable();
            corners
        };
        let halves: std::collections::HashSet<[u32; 3]> = polys
            .indices
            .chunks_exact(4)
            .flat_map(|q| {
                [key(&[q[0], q[1], q[2]]), key(&[q[0], q[2], q[3]]), key(&[q[0], q[1], q[3]]), key(&[q[1], q[2], q[3]])]
            })
            .collect();
        assert!(top.indices.chunks_exact(3).all(|t| halves.contains(&key(t))), "level {level}");
    }
}

#[test]
fn quads_that_are_not_the_triangles_are_set_aside_and_the_triangles_subdivided() {
    let mut cube = quad_cube();
    // Faces that name the same corners in another order: no longer the triangles cut up.
    cube.polys.as_mut().unwrap().indices.swap(0, 2);
    let (fine, note) = subdivided(&cube, Split::Smooth).unwrap();
    assert!(note.quads_set_aside);
    assert_eq!(levels(&fine).scheme(), Scheme::Loop);
    assert!(fine.polys.is_none());
    assert_eq!(fine.triangle_count(), 48);
    // Back at the bottom the mesh is the triangles it was, and the stack never knew of the quads.
    assert!(stepped(&fine, 0).unwrap().0.polys.is_none());
}

#[test]
fn what_cannot_be_subdivided_is_refused_in_plain_words() {
    let top = raised(&ball(), 1);
    let coarse = stepped(&top, 0).unwrap().0;
    let why = subdivided(&coarse, Split::Smooth).err().expect("not from a lower level");
    assert!(why.contains("finest level"), "{why}");
    assert!(subdivide_refusal(&top).is_none());
    let nothing = Mesh::new(Vec::new(), Vec::new(), None);
    assert!(subdivide_refusal(&nothing).unwrap().contains("nothing"));
    assert!(stepped(&ball(), 0).is_err(), "a mesh without levels has no level to step to");
    assert!(stepped(&top, 2).is_err(), "no such level");
    assert_eq!(MOST_TRIANGLES % 4, 0);
}

#[test]
fn dynamic_topology_leaves_a_mesh_with_levels_alone() {
    let mut plain = ball();
    let sites = [plain.positions[0]];
    let mut with_levels = raised(&plain, 1);
    let mut coarse = stepped(&with_levels, 0).unwrap().0;
    // The same call that splits the plain ball's triangles under a brush...
    assert!(plain.refine(&sites, 0.2, 0.005, 1000, usize::MAX).splits > 0);
    // ...splits nothing on either level of the ball that has levels.
    for mesh in [&mut with_levels, &mut coarse] {
        let triangles = mesh.triangle_count();
        let stats = mesh.refine(&sites, 0.2, 0.001, 1000, usize::MAX);
        assert_eq!((stats.splits, mesh.triangle_count()), (0, triangles));
    }
}

#[test]
fn deleting_levels_keeps_the_mesh_and_one_level_is_no_levels() {
    let mut top = raised(&ball(), 3);
    sculpt(&mut top);
    let middle = stepped(&top, 1).unwrap().0;
    let (positions, _) = arrays(&top);

    let mut cut = stepped(&top, 1).unwrap().0;
    cut.levels = without(&cut, true);
    assert_eq!((levels(&cut).count(), levels(&cut).current()), (3, 0));
    assert!(same_bits(&arrays(&stepped(&cut, 2).unwrap().0).0, &positions), "the finer levels are untouched");

    let mut cut = stepped(&top, 1).unwrap().0;
    cut.levels = without(&cut, false);
    assert_eq!((levels(&cut).count(), levels(&cut).current(), levels(&cut).top()), (2, 1, 1));
    assert!(subdivide_refusal(&cut).is_none(), "the level kept is the finest now");

    // Nothing to delete leaves things as they are; deleting everything else leaves a plain mesh.
    assert_eq!(without(&top, false).map(|l| l.count()), Some(4));
    assert!(without(&top, true).is_none());
    assert!(without(&stepped(&top, 0).unwrap().0, false).is_none());
    assert!(without(&ball(), true).is_none());
    assert_eq!(levels(&middle).count(), 4, "the mesh the levels were read from still has its own");
}

/// Every change to a mesh's levels as the app records it, stepped back and
/// forward: the stack, the mesh, the mask, the hidden faces and the quads
/// come back exactly, because the mesh from the other side of the step is
/// kept whole and its levels with it.
#[test]
fn undo_and_redo_put_back_the_mesh_and_its_levels_exactly() {
    for base in [ball(), quad_cube()] {
        let mut history = History::default();
        let mut mesh = base;
        let mut seen: Vec<(u64, usize, usize)> = vec![(mesh.uid, mesh.level_count(), mesh.triangle_count())];
        let replace = |mesh: &mut Mesh, history: &mut History, new: Mesh| {
            history.push_replace(std::mem::replace(mesh, new));
        };
        // Subdivide twice, sculpt, hide, step down, delete the level above the bottom two... every kind of step.
        for _ in 0..2 {
            let fine = subdivided(&mesh, Split::Smooth).unwrap().0;
            replace(&mut mesh, &mut history, fine);
            seen.push((mesh.uid, mesh.level_count(), mesh.triangle_count()));
        }
        sculpt(&mut mesh);
        mesh.hide(&[0, 1, 2, 3]);
        let top = (arrays(&mesh), hidden(&mesh), mesh.polys.clone().map(|p| p.indices));
        let down = stepped(&mesh, 1).unwrap().0;
        replace(&mut mesh, &mut history, down);
        seen.push((mesh.uid, mesh.level_count(), mesh.triangle_count()));
        let fewer = without(&mesh, false);
        history.push_levels(std::mem::replace(&mut mesh.levels, fewer));
        seen.push((mesh.uid, mesh.level_count(), mesh.triangle_count()));
        // Something that knows nothing of levels puts a new mesh in its place.
        let rebuilt = Mesh::new(mesh.positions.clone(), mesh.indices.clone(), None);
        replace(&mut mesh, &mut history, rebuilt);
        seen.push((mesh.uid, mesh.level_count(), mesh.triangle_count()));
        assert_eq!(seen.iter().map(|s| s.1).collect::<Vec<_>>(), [1, 2, 3, 3, 2, 1]);

        // All the way back, and all the way forward again.
        for step in (0..seen.len() - 1).rev() {
            let replaced = history.undo(&mut mesh).unwrap();
            assert_eq!((mesh.uid, mesh.level_count(), mesh.triangle_count()), seen[step], "undo to step {step}");
            assert_eq!(replaced, seen[step].0 != seen[step + 1].0, "only a new mesh is news to the renderer");
        }
        assert!(mesh.levels.is_none());
        for step in 1..seen.len() {
            history.redo(&mut mesh).unwrap();
            assert_eq!((mesh.uid, mesh.level_count(), mesh.triangle_count()), seen[step], "redo to step {step}");
        }
        // Back to the sculpted finest level: Undo of the rebuild, the delete and the step down.
        for _ in 0..3 {
            history.undo(&mut mesh).unwrap();
        }
        let now = (arrays(&mesh), hidden(&mesh), mesh.polys.clone().map(|p| p.indices));
        assert!(same_bits(&now.0.0, &top.0.0) && now.0.1 == top.0.1 && now.1 == top.1 && now.2 == top.2);
        assert_eq!(mesh.hidden_triangle_count(), 4);
        // And it is a working stack, not a picture of one.
        let low = stepped(&mesh, 0).unwrap().0;
        assert!(same_bits(&arrays(&stepped(&low, 2).unwrap().0).0, &top.0.0));
    }
}

#[test]
fn instances_share_levels_and_a_copy_takes_its_own_with_it() {
    let mut slot = raised(&ball(), 2);
    sculpt(&mut slot);
    slot.hide(&[5, 6]);
    let mut scene = Scene::new("Ball");
    let original = scene.active_id();
    let twin = scene.instance_active();
    assert_eq!(scene.get(twin).unwrap().mesh, scene.get(original).unwrap().mesh, "one mesh, so one set of levels");
    assert_eq!(scene.mesh(scene.get(twin).unwrap().mesh, &slot).unwrap().level_count(), 3);

    let (positions, mask) = arrays(&slot);
    let flags = hidden(&slot);
    let down = arrays(&stepped(&slot, 0).unwrap().0).0;
    let copy = scene.clone_active(&mut slot);
    assert_ne!(scene.get(copy).unwrap().mesh, scene.get(original).unwrap().mesh);
    // `slot` is the copy's mesh now: a mesh of its own, numbered its own way, with the same levels.
    assert_eq!(slot.level_count(), 3);
    assert!(same_bits(&arrays(&slot).0, &positions) && arrays(&slot).1 == mask && hidden(&slot) == flags);
    assert!(same_bits(&arrays(&stepped(&slot, 0).unwrap().0).0, &down));
    // Sculpting the copy leaves the original's levels alone.
    slot.positions.iter_mut().for_each(|p| *p *= 1.5);
    scene.select(original, &mut slot);
    assert!(same_bits(&arrays(&slot).0, &positions));
}

#[test]
fn a_mesh_is_charged_for_its_share_of_its_levels() {
    let plain = ball();
    let top = raised(&plain, 2);
    let alone = levels(&top).bytes();
    assert!(alone > 0 && top.memory_bytes() > alone);
    assert_eq!(levels(&top).stack.kept_bytes(), 0, "on the finest level the mesh is the finest level");
    // A second mesh of the same stack (a level step kept for undo) shares the
    // tables: the two are charged about what one was, plus what is new.
    let low = stepped(&top, 0).unwrap().0;
    let kept = top.vertex_count() * 16;
    assert_eq!(levels(&low).stack.kept_bytes(), kept);
    let own_lists = |mesh: &Mesh| (mesh.vertex_count() + mesh.triangle_count()) * 4;
    let shared = alone - own_lists(&top);
    let both = levels(&top).bytes() + levels(&low).bytes();
    // And the low mesh keeps what the finest level's mesh was built into, for the way back up.
    let top_mesh = levels(&low).top_mesh.as_ref().map_or(0, |top| top.bytes());
    assert!(top_mesh > top.triangle_count() * 40, "{top_mesh}");
    let expected = shared + kept + own_lists(&top) + own_lists(&low) + top_mesh;
    assert!(both.abs_diff(expected) < 64, "{both} against {expected}");
    assert!(levels(&low).whole_bytes() >= shared + kept);
}

/// The whole point of levels, with a real brush: a bump sculpted on the
/// finest level, the form under it reshaped by a wide stroke on the
/// coarsest, and back up the bump is still there, the height it was,
/// standing out of the surface where the surface now is.
#[test]
fn a_wide_stroke_on_the_coarsest_level_carries_a_fine_bump_with_it() {
    let base = primitives::build(Primitive::Sphere, 3_000, 0.5);
    let plain = raised(&base, 3);
    // The bump goes in the middle of a coarse face at the front, small
    // enough to miss that face's corners: the coarsest level knows nothing of it.
    let (fine, _) = arrays(&plain);
    let coarse = &fine[..base.vertex_count()];
    let face = levels(&plain).stack.triangles(0);
    let middle = |t: &[u32]| (coarse[t[0] as usize] + coarse[t[1] as usize] + coarse[t[2] as usize]) / 3.0;
    let front =
        face.chunks_exact(3).map(middle).min_by(|a, b| a.distance(Vec3::Z * 0.5).total_cmp(&b.distance(Vec3::Z * 0.5)));
    let centre = front.unwrap();
    let (radius, height) = (0.02, 0.004);
    assert!(coarse.iter().all(|p| p.distance(centre) > radius), "the bump misses every coarse vertex");
    let apex = (0..fine.len()).min_by(|&a, &b| fine[a].distance(centre).total_cmp(&fine[b].distance(centre))).unwrap();
    let mut bumped = raised(&base, 3);
    let lifted = {
        let map = levels(&bumped).vertex.clone();
        let on_bump: Vec<usize> = (0..fine.len()).filter(|&v| fine[v].distance(fine[apex]) < radius).collect();
        for &v in &on_bump {
            let fall = 1.0 - (fine[v].distance(fine[apex]) / radius).powi(2);
            bumped.positions[map[v] as usize] += fine[apex].normalize() * height * fall * fall;
        }
        on_bump.len()
    };
    assert!(lifted >= 7, "the bump is several vertices of the finest level ({lifted})");

    // The same wide Inflate stroke on the coarsest level of each.
    let reshaped = |top: &Mesh| {
        let mut low = stepped(top, 0).unwrap().0;
        let moved = stroke(&mut low, Brush::Inflate, 120.0, 1.0);
        assert!(moved > 50, "the stroke moved {moved} coarse vertices");
        stepped(&low, 3).unwrap().0
    };
    let (plain_after, bumped_after) = (reshaped(&plain), reshaped(&bumped));
    let (surface, with_bump) = (arrays(&plain_after).0, arrays(&bumped_after).0);
    let swelled = surface[apex].distance(fine[apex]);
    assert!(swelled > height * 5.0, "the form under the bump moved {swelled}");

    // What the bump adds now, against the surface as it now lies.
    let normal = plain_after.normals[levels(&plain_after).vertex[apex] as usize];
    let adds = with_bump[apex] - surface[apex];
    let (out, aside) = (adds.dot(normal), (adds - normal * adds.dot(normal)).length());
    println!("a {height} bump after the form under it moved {swelled:.3}: {out:.5} out, {aside:.5} aside");
    assert!((out - height).abs() < height * 0.05, "{out} out of the surface, for {height}");
    assert!(aside < height * 0.1, "{aside} aside");
    // And away from the stroke nothing changed at all.
    let (back, back_after) = (arrays(&plain).0, &surface);
    let untouched =
        (0..back.len()).filter(|&v| back[v].z < -0.2 && same_bits(&back[v..=v], &back_after[v..=v])).count();
    assert_eq!(untouched, back.iter().filter(|p| p.z < -0.2).count(), "the far side is bit for bit as it was");
}

/// What a change of level costs on a large model as the app pays for it:
/// the stack's own work and the new mesh's spatial index, adjacency and
/// normals. Run with `--ignored --nocapture`.
#[test]
#[ignore]
fn timing_of_level_changes_on_a_large_mesh() {
    use std::time::Instant;
    let timed = |what: &str, work: &mut dyn FnMut() -> Mesh| {
        let start = Instant::now();
        let mesh = work();
        println!("{what}: {:.0} ms ({} triangles)", start.elapsed().as_secs_f64() * 1000.0, mesh.triangle_count());
        mesh
    };
    println!("{} threads", rayon::current_num_threads());
    let base = primitives::build(Primitive::Sphere, 80_000, 0.5);
    let mut top = timed("subdivide 80k", &mut || subdivided(&base, Split::Smooth).unwrap().0);
    top = timed("subdivide 320k", &mut || subdivided(&top, Split::Smooth).unwrap().0);
    let start = Instant::now();
    let rebuilt = Mesh::new(top.positions.clone(), top.indices.clone(), Some(top.mask.clone()));
    println!("(building a 1.28M mesh from its arrays alone: {:.0} ms)", start.elapsed().as_secs_f64() * 1000.0);
    drop(rebuilt);
    top = timed("subdivide 1.28M", &mut || subdivided(&top, Split::Smooth).unwrap().0);
    let start = Instant::now();
    let rebuilt = Mesh::new(top.positions.clone(), top.indices.clone(), Some(top.mask.clone()));
    println!("(building a 5.12M mesh from its arrays alone: {:.0} ms)", start.elapsed().as_secs_f64() * 1000.0);
    drop(rebuilt);
    sculpt(&mut top);
    let mut low = timed("finest (5.12M) down to the coarsest", &mut || stepped(&top, 0).unwrap().0);
    let _ = timed("back up, nothing edited", &mut || stepped(&low, 3).unwrap().0);
    low.positions.iter_mut().filter(|p| p.x > 0.0).for_each(|p| *p = Quat::from_rotation_y(0.4) * *p);
    let up = timed("back up, half the coarsest level turned", &mut || stepped(&low, 3).unwrap().0);
    let _ = timed("finest down one level", &mut || stepped(&up, 2).unwrap().0);
    let levels = levels(&up);
    println!(
        "the 5.12M mesh holds {:.0} MB; its levels {:.0} MB of that",
        up.memory_bytes() as f64 / (1 << 20) as f64,
        levels.bytes() as f64 / (1 << 20) as f64
    );
}
