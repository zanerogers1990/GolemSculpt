//! Checks of the Symmetry card's repairs as the app runs them: what each
//! leaves of the model, what its line says, and that one undo step gives the
//! model back. The geometry itself is checked where it lives
//! (`crates/symmetry/src/ops.rs`).

use glam::Vec3;

use super::*;
use crate::app::cut_mirror::cut_and_mirrored;
use crate::mesh::primitives::{self, Primitive};
use crate::printable;

/// A ball that is its own mirror image across X to the bit, with a row of
/// vertices on the plane: what Cut & Mirror leaves.
fn symmetric() -> Mesh {
    cut_and_mirrored(&primitives::build(Primitive::Sphere, 8_000, 0.5), Axis::X, true).unwrap().0
}

/// [`symmetric`] with a lump raised on its +X side, clear of the plane: the
/// two sides still have the same triangles and no longer the same shape.
fn lopsided() -> Mesh {
    let ball = symmetric();
    let centre = Vec3::new(0.4, 0.2, 0.2);
    let lump = |p: Vec3| 0.05 * (-(p - centre).length_squared() / (2.0 * 0.1 * 0.1)).exp();
    let positions = ball.positions.iter().map(|&p| if p.x > 0.0 { p + p.normalize() * lump(p) } else { p }).collect();
    Mesh::new(positions, ball.indices.clone(), None)
}

/// [`symmetric`] with a second, smaller ball standing off its +X side: a
/// fifth of the model has nothing across the plane to pair with.
fn with_an_odd_part() -> Mesh {
    let ball = symmetric();
    let part = primitives::build(Primitive::Sphere, 2_000, 0.2);
    let base = ball.vertex_count() as u32;
    let mut positions = ball.positions.clone();
    positions.extend(part.positions.iter().map(|&p| p + Vec3::new(1.2, 0.0, 0.0)));
    let mut indices = ball.indices.clone();
    indices.extend(part.indices.iter().map(|&i| i + base));
    Mesh::new(positions, indices, None)
}

/// [`symmetric`] as two halves that were joined and never welded: each has
/// its own copy of every vertex of the seam, a hair to its own side of the
/// plane.
fn two_halves() -> Mesh {
    let ball = symmetric();
    let (mut positions, mut indices) = (Vec::new(), Vec::new());
    for positive in [true, false] {
        let mut slot = vec![u32::MAX; ball.vertex_count()];
        for triangle in ball.indices.chunks_exact(3) {
            let across: f32 = triangle.iter().map(|&v| ball.positions[v as usize].x).sum();
            if (across > 0.0) != positive {
                continue;
            }
            for &v in triangle {
                if slot[v as usize] == u32::MAX {
                    slot[v as usize] = positions.len() as u32;
                    let p = ball.positions[v as usize];
                    let gap = if p.x != 0.0 { 0.0 } else if positive { 2e-5 } else { -2e-5 };
                    positions.push(p + Vec3::X * gap);
                }
                indices.push(slot[v as usize]);
            }
        }
    }
    Mesh::new(positions, indices, None)
}

fn furthest(a: &[Vec3], b: &[Vec3]) -> f32 {
    a.iter().zip(b).map(|(a, b)| a.distance(*b)).fold(0.0, f32::max)
}

/// How far the furthest vertex of `mesh` stands off the ball's own surface.
/// A new mesh puts its vertices in an order of its own, so two meshes are
/// not compared vertex for vertex: the ball is told by its shape.
fn off_the_ball(mesh: &Mesh) -> f32 {
    mesh.positions.iter().map(|p| (p.length() - 0.5).abs()).fold(0.0, f32::max)
}

#[test]
fn match_tolerance_stays_inside_what_the_slider_offers() {
    assert_eq!(tolerance_scale(DEFAULT_TOLERANCE), 1.0);
    assert_eq!(tolerance_scale(0.0), ops::MIN_TOLERANCE_SCALE);
    assert_eq!(tolerance_scale(-3.0), ops::MIN_TOLERANCE_SCALE);
    assert_eq!(tolerance_scale(1e9), ops::MAX_TOLERANCE_SCALE);
    assert_eq!(tolerance_scale(f32::NAN), DEFAULT_TOLERANCE);
    assert_eq!(crate::doc::Options::default().symmetry_tolerance, DEFAULT_TOLERANCE);
}

#[test]
fn check_symmetry_says_what_pairs_and_names_the_button_that_helps() {
    let ball = symmetric();
    let (text, ok) = symmetry_of(&ball, "Ball", Axis::X, 1.0);
    // The line is the one the operations give, with the object's name before it and the pairs' fit after.
    let said = ops::status(&ball.positions, &ball.indices, Axis::X, 1.0);
    assert!(said.starts_with("X: symmetric - "), "{said}");
    assert_eq!(text, format!("\"Ball\" across {said}. Every pair is the mirror image of the other."));
    assert!(ok);

    // A lump on one side: every vertex still has its partner, and the model is not symmetric for that.
    let lump = lopsided();
    let (text, ok) = symmetry_of(&lump, "Ball", Axis::X, 1.0);
    assert!(text.starts_with("\"Ball\" across X: symmetric - "), "{text}");
    assert!(!ok && text.contains(" of the pairs are not mirror images of each other"), "{text}");
    assert!(text.ends_with("% of the model's size. Make Symmetric lines them up."), "{text}");
    // Rounding error is not a lump.
    let mut nudged = symmetric();
    let v = (0..nudged.vertex_count()).find(|&v| nudged.positions[v].x > 0.2).unwrap();
    nudged.positions[v].y += 1e-7;
    assert!(symmetry_of(&nudged, "Ball", Axis::X, 1.0).1);

    let (text, ok) = symmetry_of(&with_an_odd_part(), "Ball", Axis::X, 1.0);
    assert!(!ok && text.contains(" unmatched of ") && text.contains("use Cut & Mirror"), "{text}");
    assert!(make_symmetric(&mut with_an_odd_part(), &mut History::default(), Axis::X, 1.0, true).is_err());
    let empty = Mesh::new(Vec::new(), Vec::new(), None);
    assert_eq!(symmetry_of(&empty, "Nothing", Axis::Z, 1.0), ("\"Nothing\": No geometry.".to_owned(), false));
}

#[test]
fn a_symmetric_model_is_left_as_it_is_by_all_three() {
    let mut ball = symmetric();
    let (before, uid) = (ball.positions.clone(), ball.uid);
    let mut history = History::default();

    assert!(symmetry_of(&ball, "Ball", Axis::X, 1.0).1);
    let text = make_symmetric(&mut ball, &mut history, Axis::X, 1.0, true).unwrap();
    assert!(text.starts_with("Already symmetric across X - ") && text.ends_with(" pairs match."), "{text}");
    let (change, text) = clean_up(&mut ball, &mut history, Axis::X, 1.0).unwrap();
    assert!(matches!(change, Cleanup::Nothing), "{text}");
    assert_eq!(text, "Already clean across X - nothing to do.");

    // Nothing moved, and there is nothing to undo: no step that undoes nothing.
    assert_eq!((ball.uid, &ball.positions), (uid, &before));
    assert_eq!(history.steps(), (0, 0));
}

#[test]
fn make_symmetric_puts_a_lopsided_model_right_in_one_undo_step() {
    let ball = symmetric();
    // The cut's own vertices sit on chords of the ball, a hair inside it; the lump stands well clear.
    let round = off_the_ball(&ball);
    assert!(round < 1e-3 && off_the_ball(&lopsided()) > 0.04);
    for from_positive in [false, true] {
        let mut mesh = lopsided();
        let before = mesh.positions.clone();
        let (uid, triangles) = (mesh.uid, mesh.indices.clone());
        let mut history = History::default();

        let text = make_symmetric(&mut mesh, &mut history, Axis::X, 1.0, from_positive).unwrap();
        let (from, to) = if from_positive { ("+X", "-X") } else { ("-X", "+X") };
        assert!(text.starts_with(&format!("Mirrored {from} onto {to}: ")) && text.ends_with('.'), "{text}");
        assert!(symmetry_of(&mesh, "Ball", Axis::X, 1.0).1, "{text}");
        // It is the same mesh with some of its vertices moved: no triangle changed.
        assert_eq!((mesh.uid, &mesh.indices), (uid, &triangles));
        if from_positive {
            // The lump is on both sides now, and the side copied is as it was.
            let moved = mesh.positions.iter().zip(&before).filter(|(a, b)| a != b);
            assert!(moved.clone().count() > 20 && moved.clone().all(|(_, was)| was.x <= 0.0), "{text}");
            assert!(mesh.positions.iter().any(|p| p.x < 0.0 && p.length() > 0.54), "{text}");
        } else {
            // The lump is gone: the ball is back.
            assert!(off_the_ball(&mesh) < round + 1e-5, "{text}");
        }

        assert_eq!(history.steps(), (1, 0));
        let repaired = mesh.positions.clone();
        assert_eq!(history.undo(&mut mesh), Some(false));
        assert_eq!(mesh.positions, before);
        assert!(!history.can_undo());
        assert_eq!(history.redo(&mut mesh), Some(false));
        assert_eq!(mesh.positions, repaired);

        // Pressed again it has nothing to do, and records nothing.
        let again = make_symmetric(&mut mesh, &mut history, Axis::X, 1.0, from_positive).unwrap();
        assert!(again.starts_with("Already symmetric across X"), "{again}");
        assert_eq!(history.steps(), (1, 0));
    }
}

#[test]
fn make_symmetric_refuses_a_model_it_would_tear_and_changes_nothing() {
    let mut mesh = with_an_odd_part();
    let before = mesh.positions.clone();
    let mut history = History::default();
    let why = make_symmetric(&mut mesh, &mut history, Axis::X, 1.0, true).unwrap_err();
    assert!(why.starts_with("Too asymmetric to match up: "), "{why}");
    // The way forward is a button of the same card, by the name on it.
    assert!(why.contains("use Cut & Mirror +X to -X instead"), "{why}");
    assert_eq!(mesh.positions, before);
    assert_eq!(history.steps(), (0, 0));

    let mut empty = Mesh::new(Vec::new(), Vec::new(), None);
    assert_eq!(make_symmetric(&mut empty, &mut history, Axis::X, 1.0, true).unwrap_err(), "No geometry to mirror");
    assert_eq!(clean_up(&mut empty, &mut history, Axis::X, 1.0).err().unwrap(), "No geometry to clean up");
}

#[test]
fn make_symmetric_leaves_the_masked_part_and_the_hidden_part_alone() {
    // Masked all over but the lump: only the lump moves.
    let mut mesh = lopsided();
    let before = mesh.positions.clone();
    let centre = Vec3::new(0.4, 0.2, 0.2);
    for v in 0..mesh.vertex_count() {
        mesh.mask[v] = if before[v].distance(centre) < 0.3 { 0.0 } else { 1.0 };
    }
    let mask = mesh.mask.clone();
    let mut history = History::default();
    let text = make_symmetric(&mut mesh, &mut history, Axis::X, 1.0, false).unwrap();
    assert!(text.starts_with("Mirrored -X onto +X inside the unmasked area: "), "{text}");
    let moved: Vec<usize> = (0..before.len()).filter(|&v| mesh.positions[v] != before[v]).collect();
    assert!(moved.len() > 20 && moved.iter().all(|&v| mask[v] < 1.0), "{text}");
    assert!(off_the_ball(&mesh) < 2e-3, "{text}");
    assert_eq!(mesh.mask, mask, "the mask is not touched");
    assert_eq!(history.steps(), (1, 0));

    // With the lump's own faces hidden, a stroke cannot reach its vertices: they stay, and the line says so.
    let mut mesh = lopsided();
    let before = mesh.positions.clone();
    let on_the_lump: Vec<u32> = (0..mesh.triangle_count() as u32)
        .filter(|&t| mesh.indices[t as usize * 3..][..3].iter().all(|&v| before[v as usize].distance(centre) < 0.2))
        .collect();
    assert!(mesh.hide(&on_the_lump).is_some());
    let mut history = History::default();
    let text = make_symmetric(&mut mesh, &mut history, Axis::X, 1.0, false).unwrap();
    assert!(text.ends_with(" of those vertices are hidden and were left where they were."), "{text}");
    for v in 0..before.len() {
        assert!(!mesh.is_vertex_hidden(v as u32) || mesh.positions[v] == before[v], "hidden vertex {v} moved");
    }
    assert!(mesh.any_hidden(), "what was hidden is hidden still");
}

#[test]
fn cleanup_puts_a_knocked_centreline_back_on_the_plane_in_one_undo_step() {
    let ball = symmetric();
    let seam = ball.positions.iter().filter(|p| p.x == 0.0).count();
    assert!(seam > 20);
    // Each a little to one side or the other, as a smoothing stroke with the mirror off leaves them.
    let mut knocked = ball.positions.clone();
    for (k, p) in knocked.iter_mut().filter(|p| p.x == 0.0).enumerate() {
        p.x = if k % 2 == 0 { 4e-4 } else { -3e-4 };
    }
    let mut mesh = Mesh::new(knocked, ball.indices.clone(), None);
    let knocked = mesh.positions.clone();
    let on_plane: Vec<usize> = (0..knocked.len()).filter(|&v| knocked[v].x.abs() < 1e-3).collect();
    assert_eq!(on_plane.len(), seam);
    let (uid, triangles) = (mesh.uid, mesh.indices.clone());
    let mut history = History::default();

    let (change, text) = clean_up(&mut mesh, &mut history, Axis::X, 1.0).unwrap();
    assert!(matches!(change, Cleanup::Moved), "{text}");
    assert_eq!(text, format!("Snapped {} vertices onto X - no duplicates found.", on_plane.len()));
    assert!(on_plane.iter().all(|&v| mesh.positions[v].x == 0.0));
    assert_eq!((mesh.uid, &mesh.indices), (uid, &triangles));
    assert!(symmetry_of(&mesh, "Ball", Axis::X, 1.0).1);

    assert_eq!(history.steps(), (1, 0));
    assert_eq!(history.undo(&mut mesh), Some(false));
    assert_eq!(mesh.positions, knocked);
    assert_eq!(history.redo(&mut mesh), Some(false));
    assert!(on_plane.iter().all(|&v| mesh.positions[v].x == 0.0));
    // And it is clean now.
    assert!(matches!(clean_up(&mut mesh, &mut history, Axis::X, 1.0).unwrap().0, Cleanup::Nothing));
    assert_eq!(history.steps(), (1, 0));
}

/// As the app records a rebuilt mesh (`Running::replace_mesh`): the old mesh
/// is the undo step.
#[test]
fn cleanup_welds_two_joined_halves_into_one_closed_surface_in_one_undo_step() {
    let whole = symmetric();
    let mut mesh = two_halves();
    let seam = whole.positions.iter().filter(|p| p.x == 0.0).count();
    assert_eq!(mesh.vertex_count(), whole.vertex_count() + seam);
    assert!(!printable::census(&mesh).is_closed(), "the seam is open before");
    for v in 0..mesh.vertex_count() {
        mesh.mask[v] = if mesh.positions[v].y > 0.1 { 1.0 } else { 0.0 };
    }
    assert!(mesh.hide(&[3, 4, 5]).is_some());
    let (before, uid) = (mesh.positions.clone(), mesh.uid);
    let mut history = History::default();

    let (change, text) = clean_up(&mut mesh, &mut history, Axis::X, 1.0).unwrap();
    let Cleanup::Rebuilt(welded) = change else { panic!("{text}") };
    let said = format!("Snapped {} onto X, welded {seam} duplicate vertices.", 2 * seam);
    assert_eq!(text, format!("{said} Its hidden parts were cleaned up with the rest, and everything is showing again."));
    assert_eq!(history.steps(), (0, 0), "the caller records a rebuilt mesh");
    assert_eq!((welded.vertex_count(), welded.triangle_count()), (whole.vertex_count(), whole.triangle_count()));
    assert!(printable::census(&welded).is_closed() && !welded.any_hidden());
    assert!(symmetry_of(&welded, "Ball", Axis::X, 1.0).1);
    // The mask came across with the vertices.
    for (p, &m) in welded.positions.iter().zip(&welded.mask) {
        assert_eq!(m, if p.y > 0.1 { 1.0 } else { 0.0 }, "mask at {p}");
    }

    history.push_replace(std::mem::replace(&mut mesh, welded));
    assert_eq!(history.steps(), (1, 0));
    assert_eq!(history.undo(&mut mesh), Some(true));
    assert_eq!((mesh.uid, &mesh.positions, mesh.hidden_triangle_count()), (uid, &before, 3));
    assert_eq!(history.redo(&mut mesh), Some(true));
    assert_eq!(mesh.vertex_count(), whole.vertex_count());
}

/// A sculpt layer that is recording takes the repair, as it takes a stroke:
/// switched off, the model is lopsided again, and Undo takes the repair out
/// of the layer.
#[test]
fn a_recording_layer_takes_the_repair_like_any_stroke() {
    let mut mesh = lopsided();
    let before = mesh.positions.clone();
    let (layer, _) = mesh.add_layer(None);
    let mut history = History::default();
    make_symmetric(&mut mesh, &mut history, Axis::X, 1.0, false).unwrap();
    let repaired = mesh.positions.clone();
    assert!(off_the_ball(&mesh) < 1e-3);

    mesh.set_layer(layer, false, 100).unwrap();
    assert!(furthest(&mesh.positions, &before) < 1e-5, "without the layer the lump is back");
    mesh.set_layer(layer, true, 100).unwrap();
    assert!(furthest(&mesh.positions, &repaired) < 1e-5);

    assert_eq!(history.undo(&mut mesh), Some(false));
    assert!(furthest(&mesh.positions, &before) < 1e-5);
}
