//! Snake Hook pulls only the surface joined to what it has hold of
//! (hook.rs): a finger beside the one being pulled stays where it is, under
//! the mirror too and on surface dynamic topology has refined, and a pull on
//! a single ball is what it always was.

use glam::{Vec2, Vec3};

use super::hook::HOLD_ALL;
use crate::camera::View;
use super::stroke_tests::{draw, sphere, twins, view_of};
use super::*;

const RADIUS: f32 = 0.1;

/// Two balls of radius 0.2 side by side with a 0.04 gap between them, one
/// each side of the X = 0 plane: two fingers. Each is the mirror image of
/// the other, to the bit.
fn two_fingers() -> Mesh {
    let ball = sphere(40_000);
    let n = ball.vertex_count() as u32;
    let mut positions = Vec::new();
    let mut indices = Vec::new();
    for (k, x) in [-0.22f32, 0.22].into_iter().enumerate() {
        positions.extend(ball.positions.iter().map(|&p| p * 0.4 + Vec3::new(x, 0.0, 0.0)));
        indices.extend(ball.indices.iter().map(|&i| i + n * k as u32));
    }
    Mesh::new(positions, indices, None)
}

/// On the left finger, close to the gap, on the side the camera sees: a ring
/// of `RADIUS` about it reaches well into the right finger.
fn by_the_gap() -> Vec3 {
    Vec3::new(-0.03, 0.0, (0.04f32 - 0.19 * 0.19).sqrt())
}

/// A sculptor holding Snake Hook as it starts, at a fixed size.
fn hook(mirror: bool) -> Sculptor {
    let mut sculptor = Sculptor::default();
    let s = &mut sculptor.settings;
    (s.brush, s.screen_space_size, s.radius_world) = (Brush::SnakeHook, false, RADIUS);
    s.symmetry = [mirror, false, false];
    sculptor
}

/// A pull on `mesh` from `from` (a point on it) by `by`, straight across the
/// screen, at full pressure.
fn pull(mesh: &mut Mesh, sculptor: &mut Sculptor, from: Vec3, by: Vec3) {
    let view = view_of(mesh);
    let (a, b) = (view.project(from).unwrap(), view.project(from + by).unwrap());
    let path: Vec<Vec2> = (0..=60).map(|i| a.lerp(b, i as f32 / 60.0)).collect();
    draw(mesh, sculptor, &view, &path, 2, Some(&|_| 1.0));
}

/// The most any vertex of the left finger has moved from where `was` has it,
/// and the most any of the right finger (x above nought there) has.
fn moved(mesh: &Mesh, was: &[Vec3]) -> (f32, f32) {
    let (mut left, mut right) = (0.0f32, 0.0f32);
    for (v, &p) in was.iter().enumerate() {
        let d = mesh.positions[v].distance(p);
        if p.x > 0.0 {
            right = right.max(d);
        } else {
            left = left.max(d);
        }
    }
    (left, right)
}

/// Hooking one finger beside another leaves the other where it is: pulled
/// straight up, and pulled across in front of it, where the tip is dragged
/// right over the other finger.
#[test]
fn a_pull_leaves_the_finger_beside_it_alone() {
    let fresh = two_fingers();
    let near = fresh.positions.iter().filter(|p| p.x > 0.0 && p.distance(by_the_gap()) < RADIUS).count();
    assert!(near > 20, "the ring should reach the other finger, or the test proves nothing ({near} vertices)");
    for (way, by) in [("up", Vec3::Y * 0.3), ("across", Vec3::X * 0.35)] {
        for hold_all in [true, false] {
            let mut mesh = two_fingers();
            HOLD_ALL.set(hold_all);
            pull(&mut mesh, &mut hook(false), by_the_gap(), by);
            HOLD_ALL.set(false);
            let (left, right) = moved(&mesh, &fresh.positions);
            let rule = if hold_all { "everything in the ring" } else { "what it holds" };
            println!("pulled {way}, {rule}: left finger moved {left:.3}, right {right:.3}");
            assert!(left > 2.0 * RADIUS, "pulled {way}: the finger under the pen moved only {left}");
            if hold_all {
                // The fault this is about, as it was.
                assert!(right > 0.02, "pulled {way}: everything in the ring moved the other finger only {right}");
            } else {
                assert_eq!(right, 0.0, "pulled {way}: the other finger moved");
            }
        }
    }
}

/// Under the mirror each copy pulls its own finger: the left copy does not
/// reach across the gap into the right one, nor the right into the left. The
/// result is a mirror image, and the left finger comes out as a pull alone
/// leaves it.
#[test]
fn mirrored_pulls_each_keep_to_their_own_finger() {
    let fresh = two_fingers();
    let flip = Vec3::new(-1.0, 1.0, 1.0);
    let twins = twins(&fresh);
    let mut alone = two_fingers();
    pull(&mut alone, &mut hook(false), by_the_gap(), Vec3::Y * 0.3);
    let mut mirrored = two_fingers();
    pull(&mut mirrored, &mut hook(true), by_the_gap(), Vec3::Y * 0.3);
    let (mut mirror_error, mut off_alone, mut biggest) = (0.0f32, 0.0f32, 0.0f32);
    for (v, &t) in twins.iter().enumerate() {
        mirror_error = mirror_error.max(mirrored.positions[t as usize].distance(mirrored.positions[v] * flip));
        biggest = biggest.max(mirrored.positions[v].distance(fresh.positions[v]));
        if fresh.positions[v].x < 0.0 {
            off_alone = off_alone.max(mirrored.positions[v].distance(alone.positions[v]));
        }
    }
    println!("mirrored: moved {biggest:.3}, mirror error {mirror_error:.2e}, left finger off the pull alone by {off_alone:.2e}");
    assert!(biggest > 2.0 * RADIUS, "the pull moved only {biggest}");
    assert!(mirror_error <= biggest * 1e-5, "mirror error {mirror_error} of {biggest}");
    assert!(off_alone <= biggest * 1e-4, "each copy should pull its own finger as a pull alone does: off by {off_alone}");
}

/// A pull does not refine as it goes (dynamic topology refines under the
/// brushes that build), but it pulls on surface that dynamic topology has
/// refined: vertices added after the rest, and vertices merged away, left in
/// place with no neighbours. Over two fingers a Clay stroke with dynamic
/// topology has worked across the gap, a pull still leaves the other finger
/// where it is.
#[test]
fn a_pull_on_refined_surface_leaves_the_finger_beside_it_alone() {
    let mut mesh = two_fingers();
    let view = view_of(&mesh);
    let mut clay = Sculptor::default();
    let s = &mut clay.settings;
    (s.brush, s.screen_space_size, s.radius_world) = (Brush::Clay, false, RADIUS);
    clay.dyntopo.enabled = true;
    clay.dyntopo.detail = super::dyntopo::MIN_DETAIL;
    let (a, b) = (view.project(by_the_gap()).unwrap(), view.project(by_the_gap() * Vec3::new(-1.0, 1.0, 1.0)).unwrap());
    let path: Vec<Vec2> = (0..=40).map(|i| a.lerp(b, i as f32 / 40.0)).collect();
    let built = mesh.vertex_count();
    draw(&mut mesh, &mut clay, &view, &path, 2, Some(&|_| 1.0));
    let refined = mesh.positions.clone();
    let merged = (0..mesh.vertex_count() as u32).filter(|&v| mesh.neighbours(v).is_empty()).count();
    assert!(mesh.vertex_count() > built, "dynamic topology added nothing, so the test proves nothing");
    let added_right = refined[built..].iter().filter(|p| p.x > 0.0).count();
    assert!(added_right > 0, "dynamic topology refined nothing of the other finger");
    pull(&mut mesh, &mut hook(false), by_the_gap(), Vec3::Y * 0.3);
    let (left, right) = moved(&mesh, &refined);
    println!(
        "on refined surface: {} vertices from {built} ({added_right} added on the right finger, {merged} merged away), \
         left moved {left:.3}, right {right:.3}",
        mesh.vertex_count()
    );
    assert!(left > 2.0 * RADIUS, "the finger under the pen moved only {left}");
    assert_eq!(right, 0.0, "the other finger moved");
}

/// On a single ball, where everything inside the ring is joined to what the
/// pull holds, a pull is what it was when everything inside the ring went:
/// the same to the bit, straight out of the outline and curving across the
/// front, alone and under the mirror, with Auto Smooth on.
#[test]
fn a_pull_on_a_single_ball_is_what_it_was() {
    let out = |view: &View| {
        let from = Vec3::new(0.48, 0.0, (0.25f32 - 0.48 * 0.48).sqrt());
        let (a, b) = (view.project(from).unwrap(), view.project(from + Vec3::X * 3.0 * RADIUS).unwrap());
        (0..=60).map(|i| a.lerp(b, i as f32 / 60.0)).collect::<Vec<Vec2>>()
    };
    let curve = |view: &View| {
        let c = view.project(Vec3::new(0.0, 0.0, 0.5)).unwrap();
        (0..=60).map(|i| c + Vec2::new(-90.0 + 3.0 * i as f32, 60.0 * ((i as f32) * 0.1).sin())).collect::<Vec<Vec2>>()
    };
    let paths: [(&str, &dyn Fn(&View) -> Vec<Vec2>); 2] = [("out of the outline", &out), ("curving", &curve)];
    for (name, path) in paths {
        for mirror in [false, true] {
            let run = |hold_all: bool| {
                let mut mesh = sphere(60_000);
                let view = view_of(&mesh);
                let mut sculptor = hook(mirror);
                sculptor.settings.current_mut().auto_smooth = 0.1;
                HOLD_ALL.set(hold_all);
                draw(&mut mesh, &mut sculptor, &view, &path(&view), 2, Some(&|_| 1.0));
                HOLD_ALL.set(false);
                mesh.positions
            };
            let (was, now) = (run(true), run(false));
            let fresh = sphere(60_000);
            let biggest = was.iter().zip(&fresh.positions).map(|(a, b)| a.distance(*b)).fold(0.0f32, f32::max);
            let differ = was.iter().zip(&now).filter(|(a, b)| a != b).count();
            println!("{name}, mirror {mirror}: moved {biggest:.3}, {differ} vertices differ");
            assert!(biggest > RADIUS, "{name}: the pull moved only {biggest}");
            assert_eq!(differ, 0, "{name}, mirror {mirror}: the pull on a single ball changed");
        }
    }
}

/// What the hold costs: `--bench`'s Snake Hook strokes, each timed with the
/// hold and with everything in the ring taken (as before it), in turn, the
/// fastest of several runs of each, in one process so that the two meet the
/// same load on a shared machine:
/// `cargo test --release timing_of_the_hold -- --ignored --nocapture`.
#[test]
#[ignore]
fn timing_of_the_hold() {
    const SCREEN: Vec2 = Vec2::new(1920.0, 1080.0);
    let across = |t: f32| Vec2::new(SCREEN.x * 0.5 - 180.0 + 360.0 * t, SCREEN.y * 0.5 + (t * 9.0).sin() * 60.0);
    println!("fastest mean ms per frame of 4 pointer samples, everything in the ring / what the pull holds");
    for tris in [1_000_000, 5_000_000] {
        let mut mesh = crate::mesh::primitives::build(crate::mesh::primitives::Primitive::Sphere, tris, 0.5);
        let mut camera = crate::camera::Camera::default();
        camera.frame(mesh.bounds(), SCREEN.x / SCREEN.y, false);
        let mut line = format!("{:>9} tris", mesh.triangle_count());
        for radius_px in [40.0, 150.0] {
            for mirror in [false, true] {
                let mut best = [f64::INFINITY; 2];
                for _ in 0..5 {
                    for (k, hold_all) in [true, false].into_iter().enumerate() {
                        let mut sculptor = Sculptor::default();
                        sculptor.settings.brush = Brush::SnakeHook;
                        sculptor.settings.radius_px = radius_px;
                        sculptor.settings.symmetry = [mirror, false, false];
                        sculptor.settings.per_brush[Brush::SnakeHook.index()].strength = 0.5;
                        HOLD_ALL.set(hold_all);
                        let (mean, _, _) = crate::bench::stroke(&mut mesh, &mut sculptor, &camera, across, 40, 4);
                        HOLD_ALL.set(false);
                        best[k] = best[k].min(mean);
                    }
                }
                line += &format!("  r{radius_px}{} {:.2}/{:.2}", if mirror { "+m" } else { "" }, best[0], best[1]);
            }
        }
        println!("{line}");
    }
}
