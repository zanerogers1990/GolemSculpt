//! A step up worked from what the edit changed (`crate::dirty`) against the
//! same step worked over every vertex: the same to the bit, for no edit, one
//! vertex, a patch, a scattering and everything, on both schemes, both
//! splits, closed, open and broken meshes. And the two things the dirty way
//! leans on: that fans are symmetric, and that `touched_by` names every fine
//! vertex a coarse one can move.

use glam::{Quat, Vec3};

use super::fixtures::*;
use crate::dirty::Way;
use crate::{LevelMesh, Levels, Split, subdivide};

/// A grid with a fin: a third triangle on one of its inner edges, so three
/// sheets meet there, and a vertex no face uses.
fn finned_grid() -> LevelMesh {
    let mut mesh = triangle_grid(4, 3);
    let above = mesh.positions.len() as u32;
    mesh.positions.push(Vec3::new(1.5, 1.0, 1.0));
    mesh.positions.push(Vec3::new(9.0, 9.0, 9.0));
    mesh.mask.extend([0.0, 0.0]);
    mesh.triangles.extend([6, 7, above]);
    mesh
}

/// Faces of three, four and five corners in one open sheet.
fn mixed_polygons() -> LevelMesh {
    let positions = vec![
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(1.0, 0.0, 0.1),
        Vec3::new(2.0, 0.0, 0.0),
        Vec3::new(0.0, 1.0, 0.2),
        Vec3::new(1.0, 1.0, 0.0),
        Vec3::new(2.0, 1.0, 0.1),
        Vec3::new(1.5, 1.8, 0.0),
        Vec3::new(0.5, 1.8, 0.3),
    ];
    polygon_mesh(positions, &[&[0, 1, 4, 3], &[1, 2, 5, 4], &[3, 4, 6, 7], &[4, 5, 6]])
}

fn bases() -> Vec<(&'static str, LevelMesh)> {
    vec![
        ("ico sphere", ico_sphere(1)),
        ("open grid", triangle_grid(5, 4)),
        ("finned grid", finned_grid()),
        ("cube", cube()),
        ("quad torus", quad_torus(7, 5)),
        ("open quads", quad_grid(4, 3)),
        ("mixed polygons", mixed_polygons()),
    ]
}

fn bits(points: &[Vec3]) -> Vec<[u32; 3]> {
    points.iter().map(|p| p.to_array().map(f32::to_bits)).collect()
}

fn mask_bits(values: &[f32]) -> Vec<u32> {
    values.iter().map(|v| v.to_bits()).collect()
}

#[test]
fn every_fan_is_named_back() {
    for (name, base) in bases() {
        for split in [Split::Smooth, Split::Flat] {
            let (levels, _) = raised(&base, &[split, split]);
            for level in 0..2 {
                let (fans, _) = levels.fans_and_touches(level);
                for (v, fan) in fans.iter().enumerate() {
                    for &u in fan {
                        assert!(fans[u as usize].contains(&(v as u32)), "{name}, level {level}: {v} names {u}, not back");
                    }
                }
            }
        }
    }
}

/// Moves each coarse vertex in turn, subdivides, and checks that every fine
/// vertex that moved (or whose mask moved) is one `touched_by` names.
#[test]
fn touched_by_names_every_fine_vertex_a_coarse_one_moves() {
    for (name, base) in bases() {
        let (levels, _) = raised(&base, &[Split::Smooth]);
        let (_, touches) = levels.fans_and_touches(0);
        let mut rng = Rng(5);
        for split in [Split::Smooth, Split::Flat] {
            let before = subdivide(&base, split).unwrap().mesh;
            for c in 0..base.positions.len() {
                let mut moved = base.clone();
                moved.positions[c] += rng.vector() * 0.3 + Vec3::splat(0.05);
                moved.mask[c] = 1.0;
                let after = subdivide(&moved, split).unwrap().mesh;
                for i in 0..after.positions.len() {
                    let differs = after.positions[i] != before.positions[i] || after.mask[i] != before.mask[i];
                    assert!(
                        !differs || i == c || touches[c].contains(&(i as u32)),
                        "{name} {split:?}: moving {c} moves {i}, which touched_by leaves out"
                    );
                }
            }
        }
    }
}

/// Each edit of a coarse level, as the vertices it changes and where to.
fn edits(coarse: &LevelMesh, rng: &mut Rng) -> Vec<(&'static str, Vec<Vec3>, Vec<f32>)> {
    let n = coarse.positions.len();
    let centre = coarse.positions[n / 3];
    let turn = Quat::from_axis_angle(Vec3::new(0.3, 1.0, -0.2).normalize(), 0.5);
    let mut list = Vec::new();
    list.push(("nothing", coarse.positions.clone(), coarse.mask.clone()));
    let mut one = (coarse.positions.clone(), coarse.mask.clone());
    one.0[n / 2] += Vec3::new(0.05, -0.02, 0.07);
    one.1[n / 2] = 0.75;
    list.push(("one vertex", one.0, one.1));
    let near = |p: Vec3| p.distance(centre) < 0.6;
    let patch: (Vec<Vec3>, Vec<f32>) = coarse
        .positions
        .iter()
        .zip(&coarse.mask)
        .map(|(&p, &m)| if near(p) { (p + Vec3::Y * 0.1, (m + 0.5).min(1.0)) } else { (p, m) })
        .unzip();
    list.push(("a patch", patch.0, patch.1));
    let scattered: (Vec<Vec3>, Vec<f32>) = coarse
        .positions
        .iter()
        .zip(&coarse.mask)
        .map(|(&p, &m)| if rng.next() < 0.2 { (p + rng.vector() * 0.05, rng.next()) } else { (p, m) })
        .unzip();
    list.push(("a scattering", scattered.0, scattered.1));
    let all: Vec<Vec3> = coarse.positions.iter().map(|&p| turn * p * 1.1 + Vec3::X).collect();
    list.push(("everything", all, coarse.mask.iter().map(|m| 1.0 - m).collect()));
    list
}

fn changed<T: crate::Channel>(now: &[T], was: &[T]) -> Vec<u32> {
    (0..now.len() as u32).filter(|&i| !now[i as usize].same(was[i as usize])).collect()
}

/// Every edit, from every level below the top, carried up the dirty way, the
/// whole-level way and the quicker of the two: all three the same to the bit,
/// and the same as what `step_to` keeps.
#[test]
fn the_dirty_way_and_the_whole_way_give_the_same_bits() {
    let settle = |m: f32| m.clamp(0.0, 1.0);
    for (name, base) in bases() {
        for split in [Split::Smooth, Split::Flat] {
            let (mut levels, mut top) = raised(&base, &[split, Split::Smooth, split]);
            roughen(&mut top, 0.01, &mut Rng(3));
            let mut rng = Rng(11);
            for from in 0..3 {
                let coarse = levels.step_to(from, &top.positions, &top.mask).unwrap().mesh;
                let kept_positions = levels.finest_positions().unwrap().to_vec();
                let kept_mask = levels.finest_mask().unwrap().to_vec();
                for (edit, positions, mask) in edits(&coarse, &mut rng) {
                    let what = format!("{name}, {split:?}, {edit} on level {from}");
                    let moved = changed(&positions, &kept_positions);
                    let painted = changed(&mask, &kept_mask);
                    let ways = [Way::Dirty, Way::Whole, Way::Quicker].map(|way| {
                        let p = levels.raise_positions(from, &positions, &moved, &kept_positions, way);
                        let m = levels.raise_edit(from, 3, &mask, &painted, &kept_mask, &settle, way);
                        (p, m)
                    });
                    let (whole_positions, whole_mask) = &ways[1];
                    for ((p, m), way) in ways.iter().zip(["dirty", "whole", "quicker"]) {
                        assert_eq!(bits(&p.0), bits(&whole_positions.0), "{what}, {way}: positions");
                        assert_eq!(p.1, whole_positions.1, "{what}, {way}: where the positions changed");
                        assert_eq!(mask_bits(&m.0), mask_bits(&whole_mask.0), "{what}, {way}: mask");
                        assert_eq!(m.1, whole_mask.1, "{what}, {way}: where the mask changed");
                    }
                    // What it reports changed is exactly what did.
                    assert_eq!(ways[0].0.1, changed(&ways[0].0.0, &kept_positions), "{what}");
                    assert_eq!(ways[0].1.1, changed(&ways[0].1.0, &kept_mask), "{what}");
                    if edit == "nothing" {
                        assert!(ways[0].0.1.is_empty() && ways[0].1.1.is_empty(), "{what}");
                    }
                    // And a real step lands on it.
                    let mut stack = levels.clone();
                    let up = stack.step_to(3, &positions, &mask).unwrap();
                    assert_eq!(bits(&up.mesh.positions), bits(&ways[2].0.0), "{what}: step_to");
                    assert_eq!(mask_bits(&up.mesh.mask), mask_bits(&ways[2].1.0), "{what}: step_to");
                }
                levels.step_to(3, &coarse.positions, &coarse.mask).unwrap();
            }
        }
    }
}

/// A step that carries nothing shares the kept finest level with the stack
/// it was cloned from; Undo of one that did puts the shared copy back as it
/// was without touching the other.
#[test]
fn a_step_that_carries_nothing_shares_what_it_keeps() {
    let (mut levels, mut top) = raised(&ico_sphere(1), &[Split::Smooth; 2]);
    roughen(&mut top, 0.01, &mut Rng(9));
    let coarse = levels.step_to(0, &top.positions, &top.mask).unwrap().mesh;
    let kept = levels.kept_bytes();
    let alone = levels.owned_bytes();
    let mut other: Levels = levels.clone();
    assert!(levels.owned_bytes() < alone - kept / 3, "the clone shares the finest level");
    let mid = other.step_to(1, &coarse.positions, &coarse.mask).unwrap();
    assert!(!mid.undo.carried_an_edit());
    assert!(std::ptr::eq(other.finest_positions().unwrap(), levels.finest_positions().unwrap()), "passed on, not copied");

    // An edit carried up gives the stepping stack its own copy.
    let mut bent = mid.mesh.clone();
    bent.positions.iter_mut().for_each(|p| *p += Vec3::Y * 0.1);
    let step = other.step_to(0, &bent.positions, &bent.mask).unwrap();
    assert!(step.undo.carried_an_edit());
    assert!(!std::ptr::eq(other.finest_positions().unwrap(), levels.finest_positions().unwrap()));
    assert_eq!(bits(levels.finest_positions().unwrap()), bits(&top.positions), "the original keeps its own");
    let back = other.undo_step(&step.undo, &step.mesh.positions, &step.mesh.mask).unwrap();
    assert_eq!(bits(&back.positions), bits(&bent.positions));
    assert_eq!(bits(other.finest_positions().unwrap()), bits(&top.positions), "and Undo puts the edit back out");
}

/// How the two ways compare as an edit grows, on a stack of 1.3M triangles,
/// for choosing where the quicker way changes over. Run with
/// `cargo test -p sculpt-multires --release -- --ignored --nocapture timing_of_the_two_ways`.
#[test]
#[ignore]
fn timing_of_the_two_ways() {
    let (mut levels, mut top) = raised(&ico_sphere(4), &[Split::Smooth; 4]);
    roughen(&mut top, 0.001, &mut Rng(4));
    println!("{} triangles on the finest level", levels.triangle_count(4));
    let coarse = levels.step_to(0, &top.positions, &top.mask).unwrap().mesh;
    let kept = levels.finest_positions().unwrap().to_vec();
    for share in [0.001, 0.01, 0.03, 0.1, 0.3, 1.0] {
        // A cap of the sphere about `share` of its area, moved out.
        let reach = 1.0 - 2.0 * share as f32;
        let positions: Vec<Vec3> = coarse.positions.iter().map(|&p| if p.z >= reach { p * 1.02 } else { p }).collect();
        let moved = changed(&positions, &kept);
        let mut line = format!("{:>6.1}% of level 0 edited ({} vertices):", share * 100.0, moved.len());
        for way in [Way::Dirty, Way::Whole] {
            let mut best = f64::MAX;
            for _ in 0..3 {
                let clock = std::time::Instant::now();
                let raised = levels.raise_positions(0, &positions, &moved, &kept, way);
                best = best.min(clock.elapsed().as_secs_f64() * 1000.0);
                std::hint::black_box(raised);
            }
            line += &format!("  {way:?} {best:.0} ms");
        }
        println!("{line}");
    }
}
