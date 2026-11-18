//! Smooth takes out dimples, and leaves a mesh without any as it was.

use glam::{Vec2, Vec3};

use std::sync::Mutex;
use std::sync::atomic::Ordering;

use super::{OFF, crumpled};
use crate::mesh::Mesh;
use crate::sculpt::stroke_tests::{across, draw, fingerprint, rough_ball, sculptor_with, sphere, view_of};
use crate::sculpt::{Brush, strength};

/// The tests here one at a time, since one switches the untangling off.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn knots(mesh: &Mesh) -> usize {
    (0..mesh.positions.len() as u32).filter(|&v| crumpled(mesh, v)).count()
}

/// A ball with a dimple at its front, made as they are found on sculpts: the
/// triangle there split round three new vertices bunched up in its middle,
/// a twentieth of an edge apart, folded over, one standing up and one sunk
/// in, so their tiny triangle and the slivers round it face every which way.
fn dimpled_ball() -> Mesh {
    let ball = sphere(20_000);
    let front = Vec3::new(0.0, 0.0, 0.5);
    let tri = (0..ball.indices.len() / 3)
        .min_by(|&a, &b| {
            let mid = |t: usize| (0..3).map(|k| ball.positions[ball.indices[t * 3 + k] as usize]).sum::<Vec3>() / 3.0;
            mid(a).distance(front).total_cmp(&mid(b).distance(front))
        })
        .unwrap();
    let [a, b, c] = [0, 1, 2].map(|k| ball.indices[tri * 3 + k]);
    let [pa, pb, pc] = [a, b, c].map(|v| ball.positions[v as usize]);
    let mid = (pa + pb + pc) / 3.0;
    let edge = pa.distance(pb);
    let out = mid.normalize();
    let near = |to: Vec3, lift: f32| mid + (to - mid) * 0.05 + out * (lift * edge);
    let mut positions = ball.positions.clone();
    let p = positions.len() as u32;
    // Each new vertex sits toward the corner it is not joined to first, so
    // their little triangle is turned over and the slivers round it fold:
    // the knot's normals point every way, some away from the camera.
    positions.extend([near(pb, 0.3), near(pa, -0.3), near(pc, 0.0)]);
    let (q, r) = (p + 1, p + 2);
    let mut indices: Vec<u32> = ball.indices.iter().enumerate().filter(|(i, _)| i / 3 != tri).map(|(_, &v)| v).collect();
    indices.extend([a, b, q, a, q, p, b, c, r, b, r, q, c, a, p, c, p, r, p, q, r]);
    Mesh::new(positions, indices, None)
}

/// A light stroke of Smooth over a dimple takes it out.
#[test]
fn a_light_smooth_stroke_takes_out_a_dimple() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let mut mesh = dimpled_ball();
    assert!(knots(&mesh) >= 3, "the dimple as made: {} knotted vertices", knots(&mesh));
    let view = view_of(&mesh);
    let path = across(&view, -0.08, 0.08, 16);
    let mut sculptor = sculptor_with(Brush::Smooth, 0.06, strength::from_old(Brush::Smooth, 0.05));
    draw(&mut mesh, &mut sculptor, &view, &path, 4, None);
    assert_eq!(knots(&mesh), 0, "the dimple is still there after a stroke over it");
}

/// On a mesh with no dimples Smooth does what it always did, to the bit: no
/// vertex of a rough ball, a smooth one or one with a sharp groove cut in it
/// counts as knotted, so nothing is moved before the smoothing proper.
#[test]
fn a_mesh_without_dimples_has_nothing_to_untangle() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(knots(&sphere(20_000)), 0);
    assert_eq!(knots(&rough_ball(20_000)), 0);
    let mut grooved = sphere(80_000);
    let view = view_of(&grooved);
    let mut crease = sculptor_with(Brush::Crease, 0.05, 1.0);
    draw(&mut grooved, &mut crease, &view, &across(&view, -0.5, 0.5, 40), 4, None);
    assert_eq!(knots(&grooved), 0, "a Crease groove is not a dimple");
    // And a stroke of Smooth over it comes out to the bit as it did before
    // the untangling was there.
    let path: Vec<Vec2> = across(&view, -0.2, 0.2, 20);
    let stroke = |off: bool| {
        let mut mesh = Mesh::new(grooved.positions.clone(), grooved.indices.clone(), None);
        let mut smooth = sculptor_with(Brush::Smooth, 0.05, strength::from_old(Brush::Smooth, 0.1));
        OFF.store(off, Ordering::SeqCst);
        draw(&mut mesh, &mut smooth, &view, &path, 4, None);
        OFF.store(false, Ordering::SeqCst);
        fingerprint(&mesh)
    };
    let (was, now) = (stroke(true), stroke(false));
    assert_ne!(was, fingerprint(&grooved), "the stroke smoothed nothing: the test proves nothing");
    assert_eq!(now, was, "Smooth changed on a mesh without dimples");
}

/// A Smooth stroke over a dimple does what it did before everywhere the
/// dimple is not: the change stays at the dimple.
#[test]
fn only_the_dimple_is_changed() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let made = dimpled_ball();
    let view = view_of(&made);
    let path = across(&view, -0.08, 0.08, 16);
    let stroke = |off: bool| {
        let mut mesh = Mesh::new(made.positions.clone(), made.indices.clone(), None);
        let mut smooth = sculptor_with(Brush::Smooth, 0.06, strength::from_old(Brush::Smooth, 0.05));
        OFF.store(off, Ordering::SeqCst);
        draw(&mut mesh, &mut smooth, &view, &path, 4, None);
        OFF.store(false, Ordering::SeqCst);
        mesh
    };
    let (was, now) = (stroke(true), stroke(false));
    assert!(was.positions != now.positions, "the untangling did nothing: the test proves nothing");
    let middle = Vec3::new(0.0, 0.0, 0.5);
    let far = was.positions.iter().zip(&now.positions).filter(|(a, _)| a.distance(middle) > 0.06).map(|(a, b)| a.distance(*b)).fold(0.0f32, f32::max);
    assert!(far < 1e-4, "Smooth moved the surface away from the dimple differently, by up to {far}");
}
