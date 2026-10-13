//! The level stack's bookkeeping: stepping about without editing, deleting
//! levels, taking steps back, and what it says it holds.

use glam::Vec3;

use super::fixtures::*;
use crate::{LevelMesh, Levels, Scheme, Split};

const SMOOTH3: [Split; 3] = [Split::Smooth; 3];

/// A sculpted stack of four levels on its finest, for each scheme.
fn sculpted() -> Vec<(Levels, LevelMesh)> {
    let mut rng = Rng(11);
    [ico_sphere(1), quad_torus(9, 6)]
        .iter()
        .map(|mesh| {
            let (levels, mut top) = raised(mesh, &SMOOTH3);
            roughen(&mut top, 0.02, &mut rng);
            (levels, top)
        })
        .collect()
}

fn same_mesh(a: &LevelMesh, b: &LevelMesh) -> bool {
    same_bits(&a.positions, &b.positions)
        && a.mask.iter().map(|m| m.to_bits()).eq(b.mask.iter().map(|m| m.to_bits()))
        && a.triangles == b.triangles
        && a.polys == b.polys
}

#[test]
fn a_stack_knows_its_levels() {
    let (levels, top) = raised(&ico_sphere(1), &[Split::Smooth, Split::Flat]);
    assert_eq!((levels.scheme(), levels.level_count(), levels.top(), levels.current()), (Scheme::Loop, 3, 2, 2));
    assert_eq!([0, 1, 2].map(|l| levels.vertex_count(l)), [42, 162, 642]);
    assert_eq!([0, 1, 2].map(|l| levels.triangle_count(l)), [80, 320, 1280]);
    assert_eq!([0, 1, 2].map(|l| levels.split(l)), [None, Some(Split::Smooth), Some(Split::Flat)]);
    assert_eq!((top.positions.len(), top.triangles.len()), (642, 3840));
    assert_eq!(levels.triangles(2), top.triangles);
    assert_eq!(levels.triangles(0), ico_sphere(1).triangles);
    assert!(levels.polys(1).is_none());

    let (levels, top) = raised(&cube(), &[Split::Smooth, Split::Smooth]);
    assert_eq!((levels.scheme(), levels.top()), (Scheme::CatmullClark, 2));
    assert_eq!([0, 1, 2].map(|l| levels.vertex_count(l)), [8, 26, 98]);
    assert_eq!([0, 1, 2].map(|l| levels.triangle_count(l)), [12, 48, 192]);
    assert_eq!(levels.polys(2), top.polys);
    assert_eq!(levels.polys(0), cube().polys);
    assert_eq!(levels.triangles(1).len(), 48 * 3);
}

#[test]
fn stepping_down_and_back_up_without_editing_gives_back_every_bit() {
    for (mut levels, top) in sculpted() {
        // Every way of wandering between the levels and coming back to the finest.
        for route in [&[0, 3][..], &[2, 1, 0, 1, 2, 3], &[1, 1, 3], &[0, 2, 0, 3], &[3, 3]] {
            let mut mesh = top.clone();
            for &level in route {
                mesh = levels.step_to(level, &mesh.positions, &mesh.mask).unwrap().mesh;
                assert_eq!(levels.current(), level);
                assert_eq!(mesh.positions.len(), levels.vertex_count(level));
                assert_eq!(mesh.triangles.len(), levels.triangle_count(level) * 3);
            }
            assert!(same_mesh(&mesh, &top), "by way of {route:?}");
        }
    }
}

#[test]
fn a_coarse_level_is_the_fine_vertices_of_the_same_numbers() {
    for (mut levels, top) in sculpted() {
        for level in [2, 1, 0] {
            let mut mesh = levels.step_to(level, &top.positions, &top.mask).unwrap().mesh;
            let n = levels.vertex_count(level);
            assert!(same_bits(&mesh.positions, &top.positions[..n]), "level {level} sits on the sculpted surface");
            assert_eq!(mesh.mask, top.mask[..n]);
            mesh = levels.step_to(3, &mesh.positions, &mesh.mask).unwrap().mesh;
            assert!(same_mesh(&mesh, &top));
        }
    }
}

#[test]
fn subdividing_is_from_the_finest_level_only() {
    let (mut levels, top) = raised(&ico_sphere(1), &[Split::Smooth]);
    let coarse = levels.step_to(0, &top.positions, &top.mask).unwrap().mesh;
    assert!(levels.subdivide(&coarse.positions, &coarse.mask, Split::Smooth).is_err());
    assert_eq!((levels.top(), levels.current()), (1, 0), "a refusal changes nothing");
    let top = levels.step_to(1, &coarse.positions, &coarse.mask).unwrap().mesh;
    let finer = levels.subdivide(&top.positions, &top.mask, Split::Smooth).unwrap();
    assert_eq!((levels.top(), levels.current(), finer.positions.len()), (2, 2, 642));
}

#[test]
fn a_subdivision_can_be_taken_back() {
    for (mut levels, top) in sculpted() {
        let before = levels.to_parts();
        let finer = levels.subdivide(&top.positions, &top.mask, Split::Smooth).unwrap();
        assert_eq!((levels.top(), levels.current()), (4, 4));
        assert!(finer.positions.len() > top.positions.len());
        levels.undo_subdivide().unwrap();
        assert_eq!(levels.to_parts(), before);
        // The app puts its own copy of the level back, and work goes on as if nothing had happened.
        let coarse = levels.step_to(0, &top.positions, &top.mask).unwrap().mesh;
        let again = levels.step_to(3, &coarse.positions, &coarse.mask).unwrap().mesh;
        assert!(same_mesh(&again, &top));
    }
}

/// Edits level `level` of a stack that is on its finest level: steps down,
/// pushes and masks some vertices, and steps back up.
fn edit_at(levels: &mut Levels, level: usize, top: &LevelMesh) -> LevelMesh {
    let mut mesh = levels.step_to(level, &top.positions, &top.mask).unwrap().mesh;
    for v in (0..mesh.positions.len()).step_by(3) {
        mesh.positions[v] *= 1.0 + 0.1 * (v % 7) as f32 / 7.0;
        mesh.mask[v] = 1.0 - mesh.mask[v];
    }
    levels.step_to(levels.top(), &mesh.positions, &mesh.mask).unwrap().mesh
}

#[test]
fn deleting_the_lower_levels_leaves_the_rest_working_as_before() {
    for (levels, top) in sculpted() {
        // The same edit made with the lower levels there and with them gone.
        let (mut whole, mut cut) = (levels.clone(), levels);
        let expected = edit_at(&mut whole, 1, &top);

        let mesh = cut.step_to(1, &top.positions, &top.mask).unwrap().mesh;
        cut.delete_lower();
        assert_eq!((cut.level_count(), cut.current(), cut.top()), (3, 0, 2));
        assert_eq!(cut.vertex_count(0), mesh.positions.len());
        assert_eq!(
            (cut.triangles(0), cut.polys(0)),
            (mesh.triangles.clone(), mesh.polys.clone()),
            "the mesh is untouched"
        );
        assert_eq!(cut.split(0), None);
        assert_eq!(cut.parent_triangles(1).unwrap(), whole.parent_triangles(2).unwrap());
        let back = cut.step_to(2, &mesh.positions, &mesh.mask).unwrap().mesh;
        assert!(same_mesh(&back, &top));
        assert!(same_mesh(&edit_at(&mut cut, 0, &top), &expected));
    }
}

#[test]
fn deleting_the_lower_levels_from_the_finest_leaves_one_level() {
    for (mut levels, top) in sculpted() {
        levels.delete_lower();
        assert_eq!((levels.level_count(), levels.current()), (1, 0));
        assert_eq!((levels.triangles(0), levels.polys(0)), (top.triangles.clone(), top.polys.clone()));
        assert_eq!(levels.kept_bytes(), 0);
        // And it is a mesh like any other: it subdivides.
        let finer = levels.subdivide(&top.positions, &top.mask, Split::Flat).unwrap();
        assert!(same_bits(&finer.positions[..top.positions.len()], &top.positions));
    }
}

#[test]
fn deleting_the_higher_levels_makes_the_current_one_the_finest() {
    for (mut levels, top) in sculpted() {
        let mesh = levels.step_to(1, &top.positions, &top.mask).unwrap().mesh;
        assert!(levels.kept_bytes() > 0);
        levels.delete_higher();
        assert_eq!((levels.level_count(), levels.current(), levels.top()), (2, 1, 1));
        assert_eq!(levels.kept_bytes(), 0, "the detail went with them");
        assert_eq!(levels.triangles(1), mesh.triangles);
        let coarse = levels.step_to(0, &mesh.positions, &mesh.mask).unwrap().mesh;
        let back = levels.step_to(1, &coarse.positions, &coarse.mask).unwrap().mesh;
        assert!(same_mesh(&back, &mesh));
        let finer = levels.subdivide(&back.positions, &back.mask, Split::Smooth).unwrap();
        assert_eq!(finer.positions.len(), levels.vertex_count(2));

        // Down to a single level, which is then just a mesh.
        let coarse = levels.step_to(0, &finer.positions, &finer.mask).unwrap().mesh;
        levels.delete_higher();
        assert_eq!((levels.level_count(), levels.current()), (1, 0));
        assert!(levels.step_to(0, &coarse.positions, &coarse.mask).is_ok());
    }
}

#[test]
fn a_step_can_be_taken_back_exactly_and_taken_again() {
    for (levels, top) in sculpted() {
        // From the finest, between coarser levels, to the same level, and up to the finest.
        for (from, to) in [(3, 1), (1, 0), (0, 2), (1, 1), (2, 3), (3, 3), (0, 3)] {
            let mut levels = levels.clone();
            let mut live = levels.step_to(from, &top.positions, &top.mask).unwrap().mesh;
            // Sculpt the level about to be left, so the step has an edit to carry.
            for v in (0..live.positions.len()).step_by(5) {
                live.positions[v] *= 1.07;
                live.mask[v] = 0.25;
            }
            let before = levels.to_parts();
            let step = levels.step_to(to, &live.positions, &live.mask).unwrap();
            assert_eq!((step.undo.from(), step.undo.to()), (from, to));
            assert_eq!(step.undo.carried_an_edit(), from < 3, "{from} to {to}");
            assert_eq!(step.undo.bytes() > 0, from < 3);
            let after = levels.to_parts();

            let back = levels.undo_step(&step.undo, &step.mesh.positions, &step.mesh.mask).unwrap();
            assert!(same_mesh(&back, &live), "{from} to {to}: the level left comes back as it was handed in");
            assert_eq!(levels.to_parts(), before, "{from} to {to}: and the stack is as it was");

            let again = levels.step_to(to, &back.positions, &back.mask).unwrap();
            assert!(same_mesh(&again.mesh, &step.mesh) && again.undo == step.undo, "{from} to {to}: redone");
            assert_eq!(levels.to_parts(), after);
        }
    }
}

#[test]
fn an_undo_record_from_another_time_is_refused() {
    let (mut levels, top) = raised(&ico_sphere(1), &SMOOTH3);
    let step = levels.step_to(1, &top.positions, &top.mask).unwrap();
    let elsewhere = levels.step_to(2, &step.mesh.positions, &step.mesh.mask).unwrap();
    assert!(
        levels.undo_step(&step.undo, &elsewhere.mesh.positions, &elsewhere.mesh.mask).is_err(),
        "not where it left us"
    );
    let back = levels.undo_step(&elsewhere.undo, &elsewhere.mesh.positions, &elsewhere.mesh.mask).unwrap();
    assert!(levels.undo_step(&step.undo, &back.positions[1..], &back.mask).is_err(), "not that level's mesh");
    levels.delete_higher();
    assert!(levels.undo_step(&step.undo, &back.positions, &back.mask).is_err(), "the levels have changed since");
}

#[test]
fn a_step_that_carried_nothing_records_nothing() {
    let (mut levels, top) = raised(&ico_sphere(1), &SMOOTH3);
    let mut mesh = top.clone();
    for level in [0, 2, 1, 3] {
        let step = levels.step_to(level, &mesh.positions, &mesh.mask).unwrap();
        assert!(!step.undo.carried_an_edit() && step.undo.bytes() == 0);
        mesh = step.mesh;
    }
}

#[test]
fn settling_an_edit_twice_changes_nothing_the_second_time() {
    for (mut levels, top) in sculpted() {
        let mut mesh = levels.step_to(0, &top.positions, &top.mask).unwrap().mesh;
        mesh.positions[4] *= 1.3;
        // Stepping to the level we are on carries the edit up and hands the level back as it now stands.
        let settled = levels.step_to(0, &mesh.positions, &mesh.mask).unwrap();
        assert!(settled.undo.carried_an_edit());
        let again = levels.step_to(0, &settled.mesh.positions, &settled.mesh.mask).unwrap();
        assert!(!again.undo.carried_an_edit() && same_mesh(&again.mesh, &settled.mesh));
        // And going up afterwards is the same as going up straight away would have been.
        let up = levels.step_to(3, &again.mesh.positions, &again.mesh.mask).unwrap().mesh;
        let mut twin = raised_like(&top);
        let coarse = twin.step_to(0, &top.positions, &top.mask).unwrap().mesh;
        assert!(same_bits(&coarse.positions, &top.positions[..coarse.positions.len()]));
        let direct = twin.step_to(3, &mesh.positions, &mesh.mask).unwrap().mesh;
        assert!(same_mesh(&up, &direct));
    }
}

/// A fresh stack with the levels `top` is the finest of: three smooth levels
/// over the sphere or the torus.
fn raised_like(top: &LevelMesh) -> Levels {
    let base = if top.polys.is_none() { ico_sphere(1) } else { quad_torus(9, 6) };
    raised(&base, &SMOOTH3).0
}

#[test]
fn values_that_are_not_numbers_are_taken_as_not_edited() {
    let (mut levels, top) = raised(&ico_sphere(1), &SMOOTH3);
    let mut mesh = levels.step_to(1, &top.positions, &top.mask).unwrap().mesh;
    let sound = mesh.clone();
    mesh.positions[7].y = f32::NAN;
    mesh.positions[9] = glam::Vec3::splat(f32::INFINITY);
    mesh.mask[5] = f32::NAN;
    let step = levels.step_to(1, &mesh.positions, &mesh.mask).unwrap();
    assert!(!step.undo.carried_an_edit());
    assert!(same_mesh(&step.mesh, &sound));
}

#[test]
fn the_stack_says_what_it_holds() {
    for (mut levels, top) in sculpted() {
        // On the finest level nothing is kept for it at all: the app's mesh is that level.
        assert_eq!(levels.kept_bytes(), 0);
        assert_eq!(levels.level_bytes(3), 0);
        assert!((0..3).all(|level| levels.level_bytes(level) > 0));
        let tables = levels.memory_bytes();
        assert_eq!(tables, (0..3).map(|level| levels.level_bytes(level)).sum::<usize>());
        // Below it, the finest level's positions and mask are: 16 bytes a vertex, and no more for any number of levels.
        for level in [2, 0, 1] {
            levels.step_to(level, &top.positions, &top.mask).unwrap();
            assert_eq!(levels.kept_bytes(), top.positions.len() * 16);
            assert_eq!(levels.memory_bytes(), tables + top.positions.len() * 16);
            levels
                .step_to(3, &top.positions[..levels.vertex_count(level)], &top.mask[..levels.vertex_count(level)])
                .unwrap();
        }
    }
}

/// The app keeps a clone of the stack with every mesh it holds for undo. The
/// clones share every level's faces and tables, and each is charged its
/// share: together, what one stack holds and no more.
#[test]
fn clones_share_the_tables_and_are_charged_for_them_once_between_them() {
    for (mut levels, top) in sculpted() {
        let whole = levels.memory_bytes();
        assert_eq!(levels.owned_bytes(), whole, "a stack on its own owns all it holds");
        let clones: Vec<Levels> = (0..5).map(|_| levels.clone()).collect();
        let charged: usize = clones.iter().chain([&levels]).map(Levels::owned_bytes).sum();
        // (To within the rounding of each share.)
        assert!(charged <= whole && whole - charged < 6 * 8, "{charged} of {whole}");
        // What a clone keeps of its own is its own: the finest level, while it is below it.
        levels.step_to(0, &top.positions, &top.mask).unwrap();
        let kept = top.positions.len() * 16;
        assert_eq!(levels.kept_bytes(), kept);
        assert!(levels.owned_bytes() >= kept && levels.owned_bytes() < kept + whole / 5);
        assert_eq!(clones[0].kept_bytes(), 0);
        drop(clones);
        assert_eq!(levels.owned_bytes(), whole + kept, "and when the clones go it owns everything again");
        // A clone is a stack in its own right: stepping one leaves the other where it was.
        let mut other = levels.clone();
        let step =
            other.step_to(3, &top.positions[..other.vertex_count(0)], &top.mask[..other.vertex_count(0)]).unwrap();
        assert!(same_mesh(&step.mesh, &top) && (other.current(), levels.current()) == (3, 0));
    }
}

/// Meshes with everything wrong that the checks let through: edges shared by
/// many faces, faces sharing corners any old way, collapsed triangles, vertices
/// nothing uses. Whatever is done to them, no panic and no value that is not
/// a number.
#[test]
fn broken_meshes_survive_levels_edits_and_undo() {
    let mut rng = Rng(23);
    let pick = |rng: &mut Rng, n: usize| ((rng.next() * n as f32) as usize).min(n - 1) as u32;
    let positions: Vec<Vec3> = (0..50).map(|_| rng.vector()).collect();
    let triangles: Vec<u32> = (0..300).map(|_| pick(&mut rng, 50)).collect();
    let soup = triangle_mesh(positions.clone(), triangles);
    // Quads of four different corners picked at random.
    let quads: Vec<[u32; 4]> = (0..60)
        .map(|_| {
            let first = pick(&mut rng, 50);
            [first, (first + 1 + pick(&mut rng, 10)) % 50, (first + 12 + pick(&mut rng, 10)) % 50, (first + 30) % 50]
        })
        .collect();
    let patchwork = polygon_mesh(positions, &quads.iter().map(|q| &q[..]).collect::<Vec<_>>());

    for base in [soup, patchwork] {
        let (mut levels, mut top) = raised(&base, &[Split::Smooth, Split::Flat, Split::Smooth]);
        roughen(&mut top, 0.05, &mut rng);
        let finite =
            |mesh: &LevelMesh| mesh.positions.iter().all(|p| p.is_finite()) && mesh.mask.iter().all(|m| m.is_finite());
        assert!(finite(&top));
        for level in [0, 2, 1] {
            let mut mesh = levels.step_to(level, &top.positions, &top.mask).unwrap().mesh;
            for (v, p) in mesh.positions.iter_mut().enumerate() {
                *p = *p * 1.5 + Vec3::new(0.1 * (v % 5) as f32, -p.z, 0.3);
            }
            mesh.mask.iter_mut().step_by(2).for_each(|m| *m = 1.0 - *m);
            let before = levels.to_parts();
            let step = levels.step_to(3, &mesh.positions, &mesh.mask).unwrap();
            assert!(finite(&step.mesh), "after editing level {level}");
            assert!(step.mesh.mask.iter().all(|m| (0.0..=1.0).contains(m)));
            let back = levels.undo_step(&step.undo, &step.mesh.positions, &step.mesh.mask).unwrap();
            assert!(same_mesh(&back, &mesh) && levels.to_parts() == before);
            // Leave the stack on its finest level, unedited, for the next round.
            let n = levels.vertex_count(level);
            levels.step_to(3, &top.positions[..n], &top.mask[..n]).unwrap();
        }
        let parts = levels.to_parts();
        assert!(Levels::from_parts(parts.clone()).unwrap().to_parts() == parts);
    }
}
