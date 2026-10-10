//! Tubes under radial symmetry: a tube for each repeat round the axis (twice
//! as many with a mirror), each the exact turn of the first, fused into the
//! model as one surface that maps onto itself under a turn of one repeat.

use std::f32::consts::TAU;

use glam::{Quat, Vec2, Vec3};
use sculpt_geom::TriMesh;
use sculpt_toolkit::{Key, ObjectId, Scene};
use sculpt_volume::TriangleBvh;

use sculpt_toolkit::testing::Bench;

use super::tool::{bench, draw_horn, handle_px, on_sphere, preview_mesh, status, stroke, wait_for_merge};
use crate::TubeTool;
use crate::live::Symmetry;
use crate::merge::density_cell;
use crate::testing::{assert_closed, shells, unmatched_across};

/// `p` turned `step` of `count` repeats round `axis`: what radial symmetry
/// makes of it.
fn turned(p: Vec3, axis: Vec3, step: usize, count: usize) -> Vec3 {
    Quat::from_axis_angle(axis.normalize(), TAU * step as f32 / count as f32) * p
}

fn flipped_x(p: Vec3) -> Vec3 {
    Vec3::new(-p.x, p.y, p.z)
}

/// The handles of copy `copy` of tube 0, point by point.
fn handles(tool: &TubeTool, copy: usize) -> Vec<Vec3> {
    (0..tool.point_count(0)).map(|point| tool.handle_world(0, point, copy).unwrap()).collect()
}

fn same(a: &[Vec3], b: &[Vec3]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| a.distance(*b) < 1e-5)
}

/// How far the mesh is from itself turned by one repeat: the furthest any
/// vertex of the turned mesh lies from the original surface.
fn off_when_turned(mesh: &TriMesh, axis: Vec3, count: usize) -> f32 {
    let bvh = TriangleBvh::new(mesh);
    mesh.positions.iter().map(|&p| bvh.distance(turned(p, axis, 1, count))).fold(0.0, f32::max)
}

/// A horn that keeps well clear of the mirror plane and of the planes half
/// way between the turns of four, so that no two of a set of eight touch.
fn draw_side_horn(bench: &mut Bench, tool: &mut TubeTool) {
    let start = on_sphere(0.45, 0.05);
    let z = start.z;
    stroke(bench, tool, &[start, Vec3::new(0.8, 0.4, z), Vec3::new(1.1, 0.45, z)]);
}

fn set_radial(bench: &mut Bench, count: u32, axis: Vec3) {
    (bench.env.radial_count, bench.env.radial_axis) = (count, axis);
}

/// Enter, and frames until the merge has landed; the model as it is then.
fn merge(bench: &mut Bench, tool: &mut TubeTool, body: ObjectId) -> TriMesh {
    assert!(bench.key(tool, Key::Enter));
    wait_for_merge(bench, tool);
    assert_eq!(tool.tube_count(), 0, "{}", status(bench));
    bench.doc.mesh(body).unwrap().to_tri_mesh()
}

#[test]
fn radial_symmetry_makes_a_tube_for_each_repeat_and_each_is_the_exact_turn_of_the_first() {
    let tilted = Vec3::new(0.3, 1.0, -0.2).normalize();
    for (count, axis) in [(2, Vec3::Y), (5, Vec3::Y), (6, Vec3::X), (7, tilted)] {
        let (mut bench, mut tool, _) = bench();
        set_radial(&mut bench, count, axis);
        draw_horn(&mut bench, &mut tool);
        let n = count as usize;
        assert_eq!((tool.tube_count(), tool.copy_count(0)), (1, n), "{count} about {axis}");
        let first = handles(&tool, 0);
        for copy in 1..n {
            let want: Vec<Vec3> = first.iter().map(|&p| turned(p, axis, copy, n)).collect();
            assert!(same(&handles(&tool, copy), &want), "copy {copy} of {count} about {axis}");
        }

        // The tube is drawn as all of them: separate, closed pieces.
        let mesh = preview_mesh(&bench.frame(&mut tool), 0);
        assert_closed(&mesh, "the tubes round the axis");
        assert_eq!(shells(&mesh), n, "{count} about {axis}");
        assert_eq!(bench.doc.undo_steps(), 0, "nothing has touched the scene");

        // Turning it off again is one tube again, with no further edit.
        set_radial(&mut bench, 1, axis);
        let mesh = preview_mesh(&bench.frame(&mut tool), 0);
        assert_eq!((tool.copy_count(0), shells(&mesh)), (1, 1));
    }
}

#[test]
fn with_a_mirror_as_well_it_is_twice_as_many_each_the_turn_of_the_first_or_of_its_image() {
    let (mut bench, mut tool, _) = bench();
    bench.env.mirror = [true, false, false];
    set_radial(&mut bench, 4, Vec3::Y);
    draw_side_horn(&mut bench, &mut tool);
    assert_eq!(tool.copy_count(0), 8);
    let first = handles(&tool, 0);
    let (mut turns, mut images) = ([false; 4], [false; 4]);
    for copy in 0..8 {
        let got = handles(&tool, copy);
        for step in 0..4 {
            let turn: Vec<Vec3> = first.iter().map(|&p| turned(p, Vec3::Y, step, 4)).collect();
            let image: Vec<Vec3> = turn.iter().map(|&p| flipped_x(p)).collect();
            turns[step] |= same(&got, &turn);
            images[step] |= same(&got, &image);
        }
    }
    assert_eq!((turns, images), ([true; 4], [true; 4]), "each turn, and each turn of the mirror image");

    let mesh = preview_mesh(&bench.frame(&mut tool), 0);
    assert_closed(&mesh, "the dihedral set");
    assert_eq!(shells(&mesh), 8);
    assert_eq!(unmatched_across(&mesh, 0).0, 0, "it is exactly symmetric across the mirror plane");
}

#[test]
fn a_point_of_a_turned_copy_edits_the_tube_through_the_turn() {
    let (mut bench, mut tool, _) = bench();
    set_radial(&mut bench, 4, Vec3::Y);
    draw_horn(&mut bench, &mut tool);
    let last = tool.point_count(0) - 1;
    let before = tool.points(0)[last].position;
    // Copy 2 is the half turn: the horn on the far side of the model.
    let grabbed = handle_px(&bench, &tool, 0, last, 2);
    bench.drag(&mut tool, grabbed, grabbed + Vec2::new(30.0, -45.0));
    let moved = tool.points(0)[last].position;
    assert!(moved.distance(before) > 0.05, "the tube's own end moved: {before} to {moved}");
    // The copies went with it, each still the exact turn of the first.
    let first = handles(&tool, 0);
    for copy in 1..4 {
        let want: Vec<Vec3> = first.iter().map(|&p| turned(p, Vec3::Y, copy, 4)).collect();
        assert!(same(&handles(&tool, copy), &want), "copy {copy}");
    }
    // The end of the copy that was grabbed went where the pointer took it,
    // which is the other way about for the tube itself (a half turn).
    let at = tool.handle_world(0, last, 2).unwrap();
    assert!(handle_px(&bench, &tool, 0, last, 2).distance(grabbed + Vec2::new(30.0, -45.0)) < 2.0, "{at}");
    assert!(Vec3::new(-at.x, at.y, -at.z).distance(tool.handle_world(0, last, 0).unwrap()) < 1e-4);
}

#[test]
fn a_merge_of_tubes_round_an_axis_is_closed_and_maps_onto_itself_under_a_turn_of_one_repeat() {
    let tilted = Vec3::new(0.3, 1.0, 0.2).normalize();
    for (count, axis) in [(4, Vec3::Y), (3, tilted), (5, Vec3::Z)] {
        let (mut bench, mut tool, body) = bench();
        set_radial(&mut bench, count, axis);
        draw_horn(&mut bench, &mut tool);
        let n = count as usize;
        let last = tool.point_count(0) - 1;
        let tips: Vec<Vec3> = (0..n).map(|copy| tool.handle_world(0, last, copy).unwrap()).collect();
        let sphere = bench.doc.mesh(body).unwrap().to_tri_mesh();

        let merged = merge(&mut bench, &mut tool, body);
        let name = format!("{count} about {axis}");
        assert_closed(&merged, &name);
        assert_eq!(shells(&merged), 1, "{name}: every tube is fused into the model");
        assert!(merged.signed_volume() > sphere.signed_volume() * 1.02, "{name}");
        for (copy, tip) in tips.iter().enumerate() {
            let short = merged.positions.iter().map(|p| p.distance(*tip)).fold(f32::MAX, f32::min);
            assert!(short < 0.1, "{name}: the tube of copy {copy} stops {short} short of its tip");
        }
        // The remesh lattice is not turned with the model, so the surface
        // can only come out the same to within a share of one cell.
        let off = off_when_turned(&merged, axis, n);
        let cell = density_cell(&sphere.positions, &sphere.indices);
        assert!(off < 0.5 * cell, "{name}: turned by one repeat the model is {off} from itself, a cell is {cell}");
        assert_eq!(bench.doc.undo_steps(), 1);
    }
}

#[test]
fn a_merge_with_a_mirror_as_well_stays_exactly_symmetric_across_it() {
    let (mut bench, mut tool, body) = bench();
    bench.env.mirror = [true, false, false];
    set_radial(&mut bench, 4, Vec3::Y);
    draw_side_horn(&mut bench, &mut tool);
    let last = tool.point_count(0) - 1;
    let tips: Vec<Vec3> = (0..8).map(|copy| tool.handle_world(0, last, copy).unwrap()).collect();
    let sphere = bench.doc.mesh(body).unwrap().to_tri_mesh();

    let merged = merge(&mut bench, &mut tool, body);
    assert_closed(&merged, "the dihedral merge");
    assert_eq!(shells(&merged), 1);
    let (unmatched, on_plane) = unmatched_across(&merged, 0);
    assert_eq!(unmatched, 0, "of {} vertices", merged.vertex_count());
    assert!(on_plane > 50);
    for tip in &tips {
        let short = merged.positions.iter().map(|p| p.distance(*tip)).fold(f32::MAX, f32::min);
        assert!(short < 0.1, "a tube stops {short} short of its tip");
    }
    let off = off_when_turned(&merged, Vec3::Y, 4);
    let cell = density_cell(&sphere.positions, &sphere.indices);
    assert!(off < 0.5 * cell, "turned by one repeat the model is {off} from itself, a cell is {cell}");
}

#[test]
fn kept_separate_every_repeat_is_a_piece_of_the_one_object_and_undo_hands_the_tube_back() {
    let (mut bench, mut tool, body) = bench();
    set_radial(&mut bench, 5, Vec3::Y);
    draw_horn(&mut bench, &mut tool);
    let drawn = tool.points(0).to_vec();
    bench.call(|cx| tool.keep_separate(cx));
    assert_eq!((tool.tube_count(), bench.doc.objects().len(), bench.doc.undo_steps()), (0, 2, 1));
    let kept = bench.doc.active_mesh().unwrap().to_tri_mesh();
    assert_closed(&kept, "the kept tubes");
    assert_eq!(shells(&kept), 5);
    assert!(off_when_turned(&kept, Vec3::Y, 5) < 0.01);

    assert!(bench.undo(&mut tool));
    assert_eq!(bench.doc.objects(), vec![body]);
    bench.frame(&mut tool);
    assert_eq!((tool.tube_count(), tool.copy_count(0)), (1, 5));
    assert_eq!(tool.points(0), drawn.as_slice());
}

#[test]
fn the_repeat_count_is_limited_without_losing_its_lowest_values() {
    // The shape of limit that rustc 1.99.0 gets wrong for unsigned integers,
    // so every value either side of each bound is tried.
    for (asked, kept) in [(0, 1), (1, 1), (2, 2), (3, 3), (31, 31), (32, 32), (33, 32), (u32::MAX, 32)] {
        let symmetry = Symmetry::new([false; 3], asked, Vec3::Y);
        assert_eq!(symmetry.radial_count, kept, "asked for {asked}");
        assert_eq!(symmetry.group().len(), kept as usize, "asked for {asked}");
    }
    assert_eq!(Symmetry::new([true, false, false], 2, Vec3::Y).group().len(), 4);
}

#[test]
fn an_axis_that_does_not_matter_is_always_y_and_one_that_does_is_a_unit_direction() {
    let off = Symmetry::new([false; 3], 1, Vec3::X);
    assert_eq!(off, Symmetry::NONE, "turning the axis of a symmetry that is off is no change");
    assert_eq!(Symmetry::new([false; 3], 4, Vec3::ZERO).radial_axis, Vec3::Y, "no direction is no axis");
    assert_eq!(Symmetry::new([false; 3], 4, Vec3::new(0.0, 3.0, 0.0)).radial_axis, Vec3::Y);
    assert_ne!(Symmetry::new([false; 3], 4, Vec3::X), Symmetry::new([false; 3], 4, Vec3::Y));
}

#[test]
fn only_planes_the_copies_close_up_under_are_made_exact() {
    let tilted = Vec3::new(0.3, 1.0, 0.2);
    let planes = |mirror, count, axis| Symmetry::new(mirror, count, axis).exact_planes();
    // A mirror alone, as ever.
    assert_eq!(planes([true, false, true], 1, Vec3::Y), [true, false, true]);
    // Turns about a coordinate axis close up under any mirror.
    assert_eq!(planes([true, false, false], 4, Vec3::Y), [true, false, false]);
    assert_eq!(planes([true, false, true], 3, Vec3::Y), [true, false, true]);
    assert_eq!(planes([false, true, false], 5, Vec3::X), [false, true, false]);
    // A tilted axis against a mirror plane does not: no plane is exact.
    assert_eq!(planes([true, false, false], 5, tilted), [false; 3]);
    // Turns alone have no plane to make exact.
    assert_eq!(planes([false; 3], 5, tilted), [false; 3]);
}
