//! Mask Recesses and Mask Peaks, on shapes with known answers: a ball with
//! a groove round its upper half and a ridge round its lower, a plain ball,
//! a flat sheet and a cube.

use glam::Vec3;

use super::*;
use crate::mesh::primitives::{self, Primitive};

/// How far round from the equator the groove is cut and the ridge raised,
/// in radians, and how wide and how tall they are.
const GROOVE_AT: f32 = 0.5;
const RIDGE_AT: f32 = -0.5;
const WIDTH: f32 = 0.05;
const HEIGHT: f32 = 0.06;

/// How far north of the equator a point of the ball is, in radians.
fn latitude(p: Vec3) -> f32 {
    (p.y / p.length()).asin()
}

/// A ball of `radius` with a groove round it at [`GROOVE_AT`] and a ridge
/// at [`RIDGE_AT`], each a smooth bump [`WIDTH`] wide and [`HEIGHT`] of the
/// radius deep or tall.
fn grooved_ball(triangles: usize, radius: f32) -> Mesh {
    let ball = primitives::build(Primitive::Sphere, triangles, radius);
    let bump = |lat: f32, at: f32| (-((lat - at) / WIDTH).powi(2)).exp();
    let shaped = ball
        .positions
        .iter()
        .map(|&p| {
            let lat = latitude(p);
            p * (1.0 - HEIGHT * bump(lat, GROOVE_AT) + HEIGHT * bump(lat, RIDGE_AT))
        })
        .collect();
    Mesh::new(shaped, ball.indices.clone(), None)
}

/// The mean of the mask over the vertices within `within` radians of
/// latitude `at`.
fn masked_near(mesh: &Mesh, at: f32, within: f32) -> f32 {
    let there: Vec<f32> =
        (0..mesh.vertex_count()).filter(|&v| (latitude(mesh.positions[v]) - at).abs() < within).map(|v| mesh.mask[v]).collect();
    assert!(there.len() > 50, "only {} vertices near latitude {at}", there.len());
    there.iter().sum::<f32>() / there.len() as f32
}

#[test]
fn a_groove_is_masked_by_recesses_and_a_ridge_by_peaks() {
    let mut mesh = grooved_ball(120_000, 0.5);
    assert!(mask(&mut mesh, Shape::Recesses, DEFAULT_AMOUNT).is_some());
    let (groove, ridge, plain) = (masked_near(&mesh, GROOVE_AT, 0.01), masked_near(&mesh, RIDGE_AT, 0.01), masked_near(&mesh, 0.0, 0.1));
    println!("Recesses: groove {groove:.3}, ridge {ridge:.3}, plain {plain:.3}, share {:.3}", share_masked(&mesh));
    assert!(groove > 0.95, "the bottom of the groove is {groove} masked");
    assert!(ridge == 0.0, "the crest of the ridge is {ridge} masked by Recesses");
    assert!(plain == 0.0, "the plain ball between them is {plain} masked");
    assert!(share_masked(&mesh) < 0.15, "a groove is a small part of the ball: {}", share_masked(&mesh));

    // Peaks in place of that: the other way round.
    assert!(mask(&mut mesh, Shape::Peaks, DEFAULT_AMOUNT).is_some());
    let (groove, ridge, plain) = (masked_near(&mesh, GROOVE_AT, 0.01), masked_near(&mesh, RIDGE_AT, 0.01), masked_near(&mesh, 0.0, 0.1));
    println!("Peaks: groove {groove:.3}, ridge {ridge:.3}, plain {plain:.3}, share {:.3}", share_masked(&mesh));
    assert!(ridge > 0.95, "the crest of the ridge is {ridge} masked");
    assert!(groove == 0.0, "the bottom of the groove is {groove} masked by Peaks");
    assert!(plain == 0.0, "the plain ball between them is {plain} masked");
}

#[test]
fn a_flat_or_evenly_curved_surface_is_left_alone() {
    for amount in [0.0, DEFAULT_AMOUNT, 1.0] {
        for shape in [Shape::Recesses, Shape::Peaks] {
            let mut ball = primitives::build(Primitive::Sphere, 60_000, 0.5);
            assert!(mask(&mut ball, shape, amount).is_none(), "{shape:?} at {amount} masked part of a plain ball");
            assert!(ball.mask.iter().all(|&m| m == 0.0));
        }
    }
    // A flat sheet, edges and all.
    let n = 80;
    let positions: Vec<Vec3> = (0..=n).flat_map(|j| (0..=n).map(move |i| Vec3::new(i as f32, 0.0, j as f32) / n as f32)).collect();
    let at = |i: u32, j: u32| j * (n + 1) + i;
    let indices: Vec<u32> =
        (0..n).flat_map(|j| (0..n).flat_map(move |i| [at(i, j), at(i, j + 1), at(i + 1, j), at(i + 1, j), at(i, j + 1), at(i + 1, j + 1)])).collect();
    for shape in [Shape::Recesses, Shape::Peaks] {
        let mut sheet = Mesh::new(positions.clone(), indices.clone(), None);
        assert!(mask(&mut sheet, shape, 1.0).is_none(), "{shape:?} masked part of a flat sheet");
    }
    // A cube: its edges stand proud and its faces are neither.
    let mut cube = primitives::build(Primitive::Cube, 60_000, 0.5);
    let on_a_face = |p: Vec3| {
        let a = p.abs();
        let mut sorted = [a.x, a.y, a.z];
        sorted.sort_by(f32::total_cmp);
        sorted[1] < 0.3
    };
    let faces = |cube: &Mesh| (0..cube.vertex_count()).filter(|&v| on_a_face(cube.positions[v])).map(|v| cube.mask[v]).fold(0.0, f32::max);
    assert!(mask(&mut cube, Shape::Peaks, DEFAULT_AMOUNT).is_some(), "a cube's edges are peaks");
    assert_eq!(faces(&cube), 0.0, "its faces are not");
    // Not even at the most Amount: the edges pull the mesh's average their
    // way, which leaves the faces leaning a little, and too little to count.
    for amount in [DEFAULT_AMOUNT, 1.0] {
        let _ = mask(&mut cube, Shape::Recesses, amount);
        assert_eq!(faces(&cube), 0.0, "nor are they recesses, at Amount {amount}");
    }
}

#[test]
fn the_same_model_at_twice_the_size_masks_the_same_places() {
    let (mut small, mut big) = (grooved_ball(60_000, 0.5), grooved_ball(60_000, 1.0));
    for shape in [Shape::Recesses, Shape::Peaks] {
        mask(&mut small, shape, 0.4);
        mask(&mut big, shape, 0.4);
        let apart = small.mask.iter().zip(&big.mask).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(share_masked(&small) > 0.01, "{shape:?} masked something");
        assert!(apart < 2e-3, "{shape:?}: a vertex is masked {apart} differently at twice the size");
    }
}

#[test]
fn the_same_shape_masks_the_same_places_coarse_or_fine() {
    for shape in [Shape::Recesses, Shape::Peaks] {
        let (at, other) = if shape == Shape::Recesses { (GROOVE_AT, RIDGE_AT) } else { (RIDGE_AT, GROOVE_AT) };
        let mut shares = Vec::new();
        for triangles in [60_000, 250_000, 1_000_000] {
            let mut mesh = grooved_ball(triangles, 0.5);
            mask(&mut mesh, shape, DEFAULT_AMOUNT);
            let (here, there, share) = (masked_near(&mesh, at, 0.01), masked_near(&mesh, other, 0.01), share_masked(&mesh));
            println!("{shape:?} at {triangles} triangles: {here:.3} where asked, {there:.3} at the other, share {share:.4}");
            assert!(here > 0.95 && there == 0.0, "{shape:?} at {triangles} triangles: {here} and {there}");
            shares.push(share);
        }
        // Four times the triangles, and sixteen times: the same part of
        // the surface, near enough. (At sixty thousand a wrinkle this fine
        // is two triangles across, and the mesh itself has it shallower.)
        let (coarse, fine, finest) = (shares[0], shares[1], shares[2]);
        assert!((fine / finest - 1.0).abs() < 0.12, "{shape:?} takes {fine} of the surface at 250k and {finest} at a million");
        assert!((coarse / finest - 1.0).abs() < 0.4, "{shape:?} takes {coarse} of the surface at 60k and {finest} at a million");
    }
}

#[test]
fn amount_is_how_much_of_the_surface_is_taken() {
    let mut last = 0.0;
    for amount in [0.0, 0.25, 0.5, 0.75, 1.0] {
        let mut mesh = grooved_ball(120_000, 0.5);
        mask(&mut mesh, Shape::Recesses, amount);
        let share = share_masked(&mesh);
        println!("Recesses at Amount {amount}: {share:.4} of the surface");
        assert!(share >= last, "more Amount took less: {share} after {last}");
        last = share;
    }
    assert!(last > 0.05, "at its most it takes in the whole groove and its sides: {last}");
    // The slider's travel is even: each step shallower by the same factor.
    assert!((half_masked_at(0.0) - LEAN_AT.0).abs() < 1e-6 && (half_masked_at(1.0) - LEAN_AT.1).abs() < 1e-6);
    let steps: Vec<f32> = [0.0, 0.5, 1.0].map(half_masked_at).windows(2).map(|pair| pair[1] / pair[0]).collect();
    assert!((steps[0] - steps[1]).abs() < 1e-5);
    assert_eq!((masked(0.0, 0.1), masked(0.2, 0.1)), (0.0, 1.0));
    assert!((masked(0.1, 0.1) - 0.5).abs() < 1e-5 && masked(0.07, 0.1) < 0.1 && masked(0.13, 0.1) > 0.9);
}

#[test]
fn hidden_parts_keep_their_mask_and_the_command_is_one_undo_step() {
    let mut mesh = grooved_ball(120_000, 0.5);
    // A mask from before, everywhere; then the upper half (with the groove) hidden.
    mesh.remap_mask(|_, _, _| 0.3);
    let upper: Vec<u32> = (0..mesh.indices.len() as u32 / 3)
        .filter(|&t| (0..3).all(|k| mesh.positions[mesh.indices[t as usize * 3 + k] as usize].y > 0.02))
        .collect();
    assert!(mesh.hide(&upper).is_some());
    let before = mesh.mask.clone();
    assert!(mesh.curv.is_empty(), "nothing has asked for curvature");

    let delta = mask(&mut mesh, Shape::Peaks, DEFAULT_AMOUNT).expect("the ridge is in the half that shows");
    assert!(mesh.curv.is_empty(), "and the command leaves it off, as it found it");
    let hidden = |v: usize| mesh.is_vertex_hidden(v as u32);
    assert!((0..mesh.vertex_count()).filter(|&v| hidden(v)).count() > 10_000);
    assert!((0..mesh.vertex_count()).filter(|&v| hidden(v)).all(|v| mesh.mask[v] == 0.3), "a hidden vertex's mask was rewritten");
    assert!(masked_near(&mesh, RIDGE_AT, 0.01) > 0.95, "the ridge, which shows, is masked");
    assert!(masked_near(&mesh, -0.2, 0.05) == 0.0, "and the old mask on the plain part that shows is replaced");

    mesh.apply_delta(&delta, true);
    assert!(mesh.mask == before, "undo puts the mask back as it was");
    mesh.apply_delta(&delta, false);
    assert!(masked_near(&mesh, RIDGE_AT, 0.01) > 0.95, "and redo masks the ridge again");

    // A mesh something else has curvature on for keeps it on.
    let mut watched = grooved_ball(60_000, 0.5);
    watched.track_curvature(true);
    mask(&mut watched, Shape::Recesses, DEFAULT_AMOUNT);
    assert_eq!(watched.curv.len(), watched.vertex_count());
}

#[test]
fn the_evening_out_reaches_the_same_distance_whatever_the_triangles() {
    let coarse = primitives::build(Primitive::Sphere, 60_000, 0.5);
    let fine = primitives::build(Primitive::Sphere, 1_000_000, 0.5);
    let (few, many) = (passes_for(&coarse), passes_for(&fine));
    println!("{few} passes at 60k triangles, {many} at a million");
    assert!(many > few * 8, "sixteen times the triangles is four times the rings, and the passes go by the square");
    assert!(many <= MAX_PASSES);
    // And twice the size is the same number of rings.
    assert_eq!(passes_for(&primitives::build(Primitive::Sphere, 60_000, 1.0)), few);
    // A value on one vertex spreads and is not lost.
    let mut spike = vec![0.0; coarse.vertex_count()];
    spike[100] = 1.0;
    let spread = evened(&coarse, spike, 3);
    assert!(spread[100] < 0.2 && spread.iter().filter(|&&v| v > 0.0).count() > 15);
    assert!((spread.iter().sum::<f32>() - 1.0).abs() < 0.1, "about as much as there was: {}", spread.iter().sum::<f32>());
}
