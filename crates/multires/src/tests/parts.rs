//! A stack taken apart and put back together.

use super::fixtures::*;
use crate::{LevelMesh, Levels, LevelsParts, Split};

/// A sculpted stack of three levels with the app on `current`, and the mesh
/// the app holds there.
fn stack_on(base: &LevelMesh, current: usize) -> (Levels, LevelMesh) {
    let (mut levels, mut top) = raised(base, &[Split::Smooth, Split::Flat]);
    roughen(&mut top, 0.02, &mut Rng(6));
    let mesh = levels.step_to(current, &top.positions, &top.mask).unwrap().mesh;
    (levels, mesh)
}

#[test]
fn a_stack_put_back_together_is_the_stack_it_was() {
    for base in [ico_sphere(1), cube(), quad_torus(6, 5)] {
        for current in 0..3 {
            let (mut levels, mut mesh) = stack_on(&base, current);
            let parts = levels.to_parts();
            assert_eq!(parts.splits, [Split::Smooth, Split::Flat]);
            assert_eq!((parts.vertex_count, parts.current), (base.positions.len(), current));
            assert_eq!((&parts.triangles, &parts.polys), (&base.triangles, &base.polys));
            // On the finest level the app's mesh is all there is to save; below it the stack keeps that level.
            let kept = if current == 2 { 0 } else { levels.vertex_count(2) };
            assert_eq!((parts.positions.len(), parts.mask.len()), (kept, kept));
            assert_eq!(levels.clone().into_parts(), parts);

            let mut back = Levels::from_parts(parts.clone()).unwrap();
            assert_eq!(back.to_parts(), parts);
            assert_eq!((back.current(), back.level_count(), back.scheme()), (current, 3, levels.scheme()));
            assert_eq!(back.memory_bytes(), levels.memory_bytes());

            // And it carries on exactly as the original does: an edit here, a walk through every level.
            mesh.positions.iter_mut().step_by(3).for_each(|p| *p *= 1.05);
            let (mut a, mut b) = (mesh.clone(), mesh.clone());
            for level in [2, 0, 1, 2] {
                a = levels.step_to(level, &a.positions, &a.mask).unwrap().mesh;
                b = back.step_to(level, &b.positions, &b.mask).unwrap().mesh;
                assert!(same_bits(&a.positions, &b.positions) && a == b, "level {level} from {current}");
                assert_eq!(levels.parent_triangles(2).unwrap(), back.parent_triangles(2).unwrap());
            }
        }
    }
}

#[test]
fn a_stack_whose_lower_levels_were_deleted_goes_out_and_comes_back() {
    for base in [ico_sphere(1), cube()] {
        let (mut levels, mesh) = stack_on(&base, 1);
        levels.delete_lower();
        let parts = levels.to_parts();
        assert_eq!((parts.vertex_count, parts.current, parts.splits.len()), (mesh.positions.len(), 0, 1));
        assert_eq!((&parts.triangles, &parts.polys), (&mesh.triangles, &mesh.polys));
        let mut back = Levels::from_parts(parts.clone()).unwrap();
        assert_eq!(back.to_parts(), parts);
        assert_eq!(back.parent_triangles(1).unwrap(), levels.parent_triangles(1).unwrap());
        let (a, b) = (
            levels.step_to(1, &mesh.positions, &mesh.mask).unwrap(),
            back.step_to(1, &mesh.positions, &mesh.mask).unwrap(),
        );
        assert_eq!(a, b);
    }
}

#[test]
fn parts_that_do_not_fit_together_are_refused() {
    let below = |change: &dyn Fn(&mut LevelsParts)| {
        let mut parts = stack_on(&cube(), 1).0.into_parts();
        change(&mut parts);
        Levels::from_parts(parts).map(|_| ())
    };
    assert!(below(&|_| ()).is_ok());
    assert!(below(&|p| p.current = 3).is_err(), "a level past the finest");
    assert!(below(&|p| p.positions.truncate(5)).is_err(), "a position short");
    assert!(below(&|p| p.mask.push(0.0)).is_err(), "a mask value over");
    assert!(below(&|p| p.positions.clear()).is_err(), "the kept level missing");
    assert!(below(&|p| p.current = 2).is_err(), "a kept level when the app is on the finest");
    assert!(below(&|p| p.splits.push(Split::Smooth)).is_err(), "a level more than was kept for");
    assert!(below(&|p| p.splits.truncate(1)).is_err(), "a level fewer");
    assert!(below(&|p| p.vertex_count = 7).is_err(), "faces using a vertex that is not there");
    assert!(below(&|p| p.triangles.truncate(33)).is_err(), "a triangle missing");
    assert!(below(&|p| p.triangles.swap(0, 1)).is_err(), "a triangle wound the wrong way");
    assert!(below(&|p| p.polys.as_mut().unwrap().indices[2] = 1).is_err(), "a face that repeats a corner");
    assert!(below(&|p| p.polys.as_mut().unwrap().start.truncate(6)).is_err(), "a face list cut short");

    let top = |change: &dyn Fn(&mut LevelsParts)| {
        let mut parts = stack_on(&ico_sphere(1), 2).0.into_parts();
        change(&mut parts);
        Levels::from_parts(parts).map(|_| ())
    };
    assert!(top(&|_| ()).is_ok());
    assert!(top(&|p| p.positions.push(glam::Vec3::ZERO)).is_err());
    assert!(top(&|p| p.triangles[5] = 42).is_err(), "a corner past the last vertex");
    assert!(top(&|p| p.current = 1).is_err(), "nothing kept for a coarser level to stand on");
}

#[test]
fn a_split_is_a_byte_in_a_file() {
    for split in [Split::Smooth, Split::Flat] {
        assert_eq!(Split::from_byte(split.to_byte()), Some(split));
    }
    assert_eq!(Split::from_byte(2), None);
    assert_eq!(Split::from_byte(255), None);
}
