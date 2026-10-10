//! Pose brush tests, on limbs with answers that can be checked: where the
//! joint lands, what stays still, what stays rigid, what a chain keeps.

use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_2, TAU};

use glam::{Quat, Vec2, Vec3};
use sculpt_geom::{SymmetryGroup, SymmetryOp};

use super::*;
use crate::camera::{Camera, View};
use crate::history::History;
use crate::mesh::StrokeDelta;
use crate::mesh::primitives::{self, Primitive};
use crate::sculpt::{FalloffCurve, PointerSample, Radial, Settings};

const SIZE: Vec2 = Vec2::new(1600.0, 900.0);

/// The limb's thickness, and how far its straight part runs each way from
/// the middle.
const THICK: f32 = 0.1;
const HALF: f32 = 1.0;

/// A limb standing in for an arm: a tube of radius `THICK` along Y with
/// rounded ends, `2 * HALF` long between them, with even triangles.
fn limb() -> Mesh {
    let around = 32;
    let caps = around / 4;
    let stacks = (2.0 * HALF / (TAU * THICK / around as f32)).ceil() as usize;
    // Every ring of the tube as (height, radius), bottom to top.
    let mut rings: Vec<(f32, f32)> = Vec::new();
    for i in 1..=caps {
        let a = FRAC_PI_2 * (1.0 - i as f32 / caps as f32);
        rings.push((-HALF - THICK * a.sin(), THICK * a.cos()));
    }
    rings.extend((1..=stacks).map(|i| (-HALF + 2.0 * HALF * i as f32 / stacks as f32, THICK)));
    for i in 1..caps {
        let a = FRAC_PI_2 * i as f32 / caps as f32;
        rings.push((HALF + THICK * a.sin(), THICK * a.cos()));
    }
    let mut positions = vec![Vec3::new(0.0, -HALF - THICK, 0.0)];
    for &(y, r) in &rings {
        positions.extend((0..around).map(|k| {
            let a = TAU * k as f32 / around as f32;
            Vec3::new(r * a.cos(), y, r * a.sin())
        }));
    }
    positions.push(Vec3::new(0.0, HALF + THICK, 0.0));
    let top = positions.len() as u32 - 1;
    let at = |ring: usize, k: usize| (1 + ring * around + k % around) as u32;
    let mut indices = Vec::new();
    for k in 0..around {
        indices.extend([0, at(0, k), at(0, k + 1)]);
        for ring in 0..rings.len() - 1 {
            let (a, b, c, d) = (at(ring, k), at(ring, k + 1), at(ring + 1, k), at(ring + 1, k + 1));
            indices.extend([a, c, b, b, c, d]);
        }
        indices.extend([top, at(rings.len() - 1, k + 1), at(rings.len() - 1, k)]);
    }
    Mesh::new(positions, indices, None)
}

/// A capsule from the app's own shapes, mirror-exact in its points and its
/// triangles, stretched into a limb of the same size as [`limb`] and moved
/// `x` to the side.
fn mirror_limb(x: f32) -> (Vec<Vec3>, Vec<u32>) {
    let capsule = primitives::build(Primitive::Capsule, 12_000, 0.25);
    let scale = Vec3::new(THICK / 0.25, (HALF + THICK) / 0.5, THICK / 0.25);
    (capsule.positions.iter().map(|&p| p * scale + Vec3::new(x, 0.0, 0.0)).collect(), capsule.indices.clone())
}

/// Two such limbs, one each side of the X = 0 plane, as one mesh.
fn two_limbs() -> Mesh {
    let (mut positions, mut indices) = mirror_limb(-0.6);
    let (right, right_indices) = mirror_limb(0.6);
    let n = positions.len() as u32;
    positions.extend(right);
    indices.extend(right_indices.iter().map(|&i| i + n));
    Mesh::new(positions, indices, None)
}

/// Index of each vertex's mirror image across X. Panics if the mesh is not
/// mirror-exact.
fn twins(mesh: &Mesh) -> Vec<u32> {
    let key = |p: Vec3| [(p.x + 0.0).to_bits(), (p.y + 0.0).to_bits(), (p.z + 0.0).to_bits()];
    let map: HashMap<[u32; 3], u32> = mesh.positions.iter().enumerate().map(|(i, &p)| (key(p), i as u32)).collect();
    mesh.positions
        .iter()
        .map(|&p| *map.get(&key(p * Vec3::new(-1.0, 1.0, 1.0))).expect("the mesh is not mirror-exact"))
        .collect()
}

/// The model seen from the front, or turned `yaw` degrees.
fn view_of(mesh: &Mesh, yaw: f32) -> View {
    let mut camera = Camera::default();
    camera.frame(mesh.bounds(), SIZE.x / SIZE.y, false);
    camera.yaw = yaw;
    camera.view(Vec2::ZERO, SIZE, camera.distance, 1.0)
}

/// A sculptor holding the Pose brush at a fixed size on the model.
fn poser(radius: f32, setup: impl FnOnce(&mut Settings)) -> Sculptor {
    let mut sculptor = Sculptor::default();
    let s = &mut sculptor.settings;
    s.brush = Brush::Pose;
    s.screen_space_size = false;
    s.radius_world = radius;
    setup(s);
    sculptor
}

fn sample(view: &View, at: Vec3, step: usize) -> PointerSample {
    let pos = view.project(at).expect("the point is in front of the camera");
    PointerSample { pos, pressure: None, time: step as f64 / 240.0 }
}

/// Presses at the surface point `from` and drags the pointer to `to`, both
/// seen through `view`, leaving the stroke open so the test can look at it.
/// `other` is Ctrl held. Returns the first chain as the press found it.
fn drag(mesh: &mut Mesh, sculptor: &mut Sculptor, view: &View, from: Vec3, to: Vec3, other: bool) -> Vec<Vec3> {
    assert!(sculptor.begin(mesh, view, sample(view, from, 0), other, false), "the press missed the limb");
    let rest = chains(sculptor)[0].clone();
    for step in 1..=20 {
        sculptor.sample(mesh, view, sample(view, from.lerp(to, step as f32 / 20.0), step));
        if step % 4 == 0 {
            sculptor.frame(mesh, view, 1.0 / 60.0);
            let _ = mesh.take_dirty();
        }
    }
    rest
}

fn release(mesh: &mut Mesh, sculptor: &mut Sculptor, view: &View) -> Option<StrokeDelta> {
    let delta = sculptor.end(mesh, view);
    let _ = mesh.take_dirty();
    delta
}

/// The chains as points of their own, so the sculptor can be used again.
fn chains(sculptor: &Sculptor) -> Vec<Vec<Vec3>> {
    sculptor.pose_chains().into_iter().map(<[Vec3]>::to_vec).collect()
}

/// The chain a press at `at` would make: the grabbed point, then the joints.
fn hover(mesh: &Mesh, sculptor: &mut Sculptor, at: Vec3) -> Vec<Vec3> {
    sculptor.pose_hover(mesh, Some((at, sculptor.settings.radius_world)), false);
    let found = chains(sculptor);
    assert_eq!(found.len(), 1, "one chain for one copy of the brush");
    found[0].clone()
}

/// Where the brush ring of radius `r`, centred on the front of the limb at
/// height `y`, cuts the limb below it: the height of the middle of the cut.
/// A point of the limb's skin at angle `a` round it is on the ring where its
/// height differs from `y` by sqrt(r^2 - 2 THICK^2 (1 - sin a)).
fn ring_cut_below(y: f32, r: f32) -> f32 {
    let n = 3600;
    let at = |i: usize| (r * r - 2.0 * THICK * THICK * (1.0 - (TAU * i as f32 / n as f32).sin())).sqrt();
    let reach: f32 = (0..n).map(at).sum();
    y - reach / n as f32
}

// ------------------------------------------------------------- the joint

#[test]
fn the_joint_sits_in_the_middle_of_the_limb_where_the_brush_ring_cuts_it() {
    let mesh = limb();
    let edge = TAU * THICK / 32.0;
    for (y, radius) in [(0.6, 0.5), (0.9, 0.3), (0.2, 0.9)] {
        let grabbed = Vec3::new(0.0, y, THICK);
        let mut sculptor = poser(radius, |_| {});
        let chain = hover(&mesh, &mut sculptor, grabbed);
        assert_eq!(chain.len(), 2, "the grabbed point and one joint");
        assert_eq!(chain[0], grabbed);
        // The joint's ring is the first row of vertices past the cut, so it
        // sits up to an edge further on, and on the limb's centre line.
        let cut = ring_cut_below(y, radius);
        let joint = chain[1];
        assert!(joint.y < cut + 1e-3 && joint.y > cut - 1.5 * edge, "radius {radius}: joint at {joint}, cut at {cut}");
        let off_centre = Vec2::new(joint.x, joint.z).length();
        assert!(off_centre < 0.2 * THICK, "radius {radius}: joint {joint} is off the centre line");
    }
}

#[test]
fn the_offset_moves_the_joint_on_by_that_many_brush_sizes() {
    let mesh = limb();
    let grabbed = Vec3::new(0.0, 0.8, THICK);
    let plain = hover(&mesh, &mut poser(0.4, |_| {}), grabbed);
    for offset in [0.25, 0.5, 1.0, 2.0] {
        let moved = hover(&mesh, &mut poser(0.4, |s| s.pose.offset = offset), grabbed);
        // That far on from the grabbed point through the plain joint, and
        // there in the middle of the limb. (Blender's joint is the pushed
        // point itself, which for a limb grabbed by its side as here is off
        // the centre line: at 2 it has left the limb.)
        let pushed = plain[1] + (plain[1] - grabbed).normalize() * (0.4 * offset);
        let joint = moved[1];
        assert!((joint.y - pushed.y).abs() < 0.03, "offset {offset}: joint at {joint}, pushed to {pushed}");
        let off_centre = Vec2::new(joint.x, joint.z).length();
        assert!(off_centre < 0.2 * THICK, "offset {offset}: joint {joint} is off the centre line");
    }
}

/// A press and the hover before it agree: what the preview shows is what the
/// press takes hold of.
#[test]
fn the_preview_shows_the_chain_the_press_then_makes() {
    let mut mesh = limb();
    let view = view_of(&mesh, 0.0);
    let grabbed = Vec3::new(0.0, 0.9, THICK);
    let mut sculptor = poser(0.3, |s| s.pose.joints = 3);
    let shown = hover(&mesh, &mut sculptor, grabbed);
    assert_eq!(shown.len(), 4);
    assert!(sculptor.begin(&mut mesh, &view, sample(&view, grabbed, 0), false, false));
    let held = chains(&sculptor);
    assert_eq!(held.len(), 1);
    for (a, b) in shown.iter().zip(&held[0]) {
        // The press lands where the ray through the pixel meets the surface,
        // which is the grabbed point to within the pixel.
        assert!(a.distance(*b) < 2e-3, "{a} / {b}");
    }
    assert!(release(&mut mesh, &mut sculptor, &view).is_none(), "a press that does not move changes nothing");
}

#[test]
fn the_number_of_joints_is_kept_to_what_a_chain_can_hold() {
    let mesh = limb();
    let grabbed = Vec3::new(0.0, 1.0, THICK);
    let joints = |wanted: u32| hover(&mesh, &mut poser(0.2, |s| s.pose.joints = wanted), grabbed).len() - 1;
    // The lowest input first: house rule 9.
    assert_eq!(joints(0), 1);
    assert_eq!(joints(1), 1);
    assert_eq!(joints(2), 2);
    assert_eq!(joints(8), 8);
    assert_eq!(joints(u32::MAX), 8);
    // A chain stops where the limb does.
    let short = hover(&mesh, &mut poser(0.45, |s| s.pose.joints = 8), grabbed);
    assert!(short.len() - 1 == 4, "{} joints", short.len() - 1);
    // Resize and Stretch work about one joint whatever is set.
    for mode in [PoseMode::Resize, PoseMode::Stretch] {
        let chain = hover(&mesh, &mut poser(0.2, |s| (s.pose.joints, s.pose.mode) = (5, mode)), grabbed);
        assert_eq!(chain.len(), 2, "{mode:?}");
    }
}

// ---------------------------------------------------------------- a bend

/// A limb bent with one joint from the front, by a drag from `from` to `to`.
struct Bent {
    mesh: Mesh,
    before: Vec<Vec3>,
    joint: Vec3,
    /// The turn that takes the grabbed point from where it was to where the
    /// drag left it.
    turn: Quat,
}

/// The shortest turn about `joint` that takes the direction to `from` onto
/// the direction to `to`.
fn turn_about(joint: Vec3, from: Vec3, to: Vec3) -> Quat {
    Quat::from_rotation_arc((from - joint).normalize(), (to - joint).normalize())
}

fn bend(mut sculptor: Sculptor, from: Vec3, to: Vec3, mask: impl Fn(Vec3) -> f32) -> Bent {
    let mut mesh = limb();
    for v in 0..mesh.vertex_count() {
        mesh.mask[v] = mask(mesh.positions[v]);
    }
    let before = mesh.positions.clone();
    let view = view_of(&mesh, 0.0);
    let rest = drag(&mut mesh, &mut sculptor, &view, from, to, false);
    let chain = chains(&sculptor)[0].clone();
    let joint = chain[1];
    assert_eq!(joint, rest[1], "the one joint of a bend stays where it is");
    assert!(rest[0].distance(from) < 1e-3, "the press landed at {}", rest[0]);
    assert!(release(&mut mesh, &mut sculptor, &view).is_some());
    // The limb points at the pointer.
    assert!((chain[0] - joint).normalize().distance((to - joint).normalize()) < 1e-3);
    Bent { mesh, before, joint, turn: turn_about(joint, rest[0], chain[0]) }
}

#[test]
fn a_bend_turns_the_limb_about_the_joint_and_leaves_the_rest_exactly_still() {
    let (from, to) = (Vec3::new(0.0, 0.6, THICK), Vec3::new(0.45, 0.45, THICK));
    // With the joint where the brush ring cuts the limb, and with it pushed
    // on half a brush size: the bend goes with the joint.
    for offset in [0.0, 0.5] {
        let bent = bend(poser(0.5, |s| s.pose.offset = offset), from, to, |_| 0.0);
        assert!((bent.joint.y - (ring_cut_below(0.6, 0.5) - 0.5 * offset)).abs() < 0.03, "joint at {}", bent.joint);
        // The bend at rigidity 0.6 reaches 0.16 of the brush size each side
        // of the joint's ring, which is tilted by the limb's thickness.
        let band = 0.16 * 0.5 + THICK;
        let (mut still, mut rigid, mut worst) = (0, 0, 0.0f32);
        for (v, &was) in bent.before.iter().enumerate() {
            let now = bent.mesh.positions[v];
            if was.y < bent.joint.y - band {
                assert_eq!(now, was, "offset {offset}: vertex {v} below the joint moved");
                still += 1;
            } else if was.y > bent.joint.y + band {
                worst = worst.max(now.distance(bent.joint + bent.turn * (was - bent.joint)));
                rigid += 1;
            }
        }
        assert!(still > 500 && rigid > 500, "offset {offset}: {still} still, {rigid} carried");
        assert!(worst < 1e-5, "offset {offset}: the carried end is {worst} from where a rigid turn puts it");
    }
}

/// Blender's flood leaves the brush ring at both ends of a limb pressed
/// half-way along, and puts the joint under the pointer. Here the joint is
/// the cut on the body's side, and the far end goes with the limb.
#[test]
fn a_press_half_way_along_a_limb_takes_the_hand_with_it() {
    let (from, to) = (Vec3::new(0.0, 0.3, THICK), Vec3::new(0.3, 0.2, THICK));
    let bent = bend(poser(0.3, |_| {}), from, to, |_| 0.0);
    let cut = ring_cut_below(0.3, 0.3);
    assert!((bent.joint.y - cut).abs() < 0.03, "the joint is at {}, the lower cut at {cut}", bent.joint);
    let mut carried = 0;
    for (v, &was) in bent.before.iter().enumerate() {
        // From above the bend right out to the tip, past the upper cut.
        if was.y > bent.joint.y + 0.15 {
            let rigid = bent.joint + bent.turn * (was - bent.joint);
            assert!(bent.mesh.positions[v].distance(rigid) < 1e-5, "vertex {v} at {was} was left behind");
            carried += 1;
        } else if was.y < bent.joint.y - 0.15 {
            assert_eq!(bent.mesh.positions[v], was);
        }
    }
    assert!(carried > 1000);
}

#[test]
fn rigidity_sets_how_long_the_bend_is() {
    let (from, to) = (Vec3::new(0.0, 0.6, THICK), Vec3::new(0.45, 0.45, THICK));
    // How much of the limb neither stays still nor is carried whole.
    let bending = |rigidity: f32| {
        let bent = bend(poser(0.5, |s| s.pose.rigidity = rigidity), from, to, |_| 0.0);
        let partly = |v: usize| {
            let (was, now) = (bent.before[v], bent.mesh.positions[v]);
            now != was && now.distance(bent.joint + bent.turn * (was - bent.joint)) > 1e-4
        };
        let heights: Vec<f32> = (0..bent.before.len()).filter(|&v| partly(v)).map(|v| bent.before[v].y).collect();
        let (low, high) = heights.iter().fold((f32::MAX, f32::MIN), |(low, high), &y| (low.min(y), high.max(y)));
        (high - low).max(0.0)
    };
    let (soft, firm, hinge) = (bending(0.0), bending(0.6), bending(1.0));
    // A hinge bends between one row of vertices and the next; at 0 the bend
    // runs a whole brush size each side of the joint.
    assert!(hinge < 0.05, "a hinge bends over {hinge}");
    assert!((firm - 0.16).abs() < 0.08, "rigidity 0.6 bends over {firm}");
    assert!((soft - 1.0).abs() < 0.15, "rigidity 0 bends over {soft}");
}

/// A turn of part of the angle, not a mix of turned and unturned positions:
/// the skin in the bend keeps its distance from the joint, where a mix would
/// pinch it in.
#[test]
fn the_skin_in_a_bend_keeps_its_distance_from_the_joint() {
    let (from, to) = (Vec3::new(0.0, 0.6, THICK), Vec3::new(0.48, 0.15, THICK));
    let bent = bend(poser(0.5, |s| s.pose.rigidity = 0.3), from, to, |_| 0.0);
    let mut worst = 0.0f32;
    for (v, &was) in bent.before.iter().enumerate() {
        let now = bent.mesh.positions[v];
        if now != was {
            worst = worst.max((now.distance(bent.joint) - was.distance(bent.joint)).abs());
        }
    }
    assert!(worst < 1e-5, "the bend moved skin {worst} nearer to or further from the joint");
}

/// The brush's own falloff shapes a bend: Focal Shift moves where in the bend
/// the skin goes half-way, and a hand-drawn curve replaces its shape.
#[test]
fn the_falloff_shapes_the_bend() {
    let (from, to) = (Vec3::new(0.0, 0.6, THICK), Vec3::new(0.45, 0.45, THICK));
    // How far the skin in the bend goes, as a share of the whole turn.
    let followed = |setup: &dyn Fn(&mut Settings)| {
        let sculptor = poser(0.5, |s| {
            s.pose.rigidity = 0.3;
            setup(s);
        });
        let bent = bend(sculptor, from, to, |_| 0.0);
        let (mut gone, mut whole) = (0.0f64, 0.0f64);
        for (v, &was) in bent.before.iter().enumerate() {
            if (was.y - bent.joint.y).abs() < 0.2 {
                gone += bent.mesh.positions[v].distance(was) as f64;
                whole += (bent.joint + bent.turn * (was - bent.joint)).distance(was) as f64;
            }
        }
        gone / whole
    };
    let slot = Brush::Pose.index();
    let plain = followed(&|_| {});
    let hard = followed(&|s| s.per_brush[slot].focal_shift = -0.8);
    let soft = followed(&|s| s.per_brush[slot].focal_shift = 0.8);
    assert!(hard > plain * 1.1 && plain > soft * 1.1, "hard {hard}, plain {plain}, soft {soft}");
    // A curve that stays at full weight to its edge carries the whole bend.
    let flat = |weight: f32| FalloffCurve { points: vec![Vec2::new(0.0, weight), Vec2::new(1.0, weight)] };
    let all = followed(&|s| s.falloff[slot] = Some(flat(1.0)));
    assert!(all > hard, "a flat curve carries {all} of the bend");

    // And one of no weight moves nothing, as for every other brush.
    let mut mesh = limb();
    let before = mesh.positions.clone();
    let view = view_of(&mesh, 0.0);
    let mut sculptor = poser(0.5, |s| s.falloff[slot] = Some(flat(0.0)));
    assert!(sculptor.begin(&mut mesh, &view, sample(&view, from, 0), false, false));
    assert!(sculptor.pose_chains().is_empty(), "nothing is held, so there is no chain to show");
    sculptor.sample(&mut mesh, &view, sample(&view, to, 1));
    assert!(release(&mut mesh, &mut sculptor, &view).is_none());
    assert!(mesh.positions == before);
}

/// Out of budget before it can tell the limb from the body, the search gives
/// Blender's answer (every way out of the brush ring is the joint's, so the
/// joint lies between them) and stops at one joint.
#[test]
fn a_search_that_runs_out_of_budget_still_gives_a_joint() {
    let mesh = limb();
    let centre = Vec3::new(0.0, 0.3, THICK);
    let seed = nearest_vertex(&mesh, centre, 0.3).unwrap();
    let ask = |budget| Ask { centre, seed, radius: 0.3, offset: 0.0, joints: 3, budget, core: false };
    let mut scratch = Scratch::default();
    let whole = build::skeleton(&mesh, &mut scratch, &ask(None));
    let cut = ring_cut_below(0.3, 0.3);
    assert_eq!(whole.joints.len(), 3);
    assert!((whole.joints[0].y - cut).abs() < 0.03, "the joint is at {}", whole.joints[0]);

    let short = build::skeleton(&mesh, &mut scratch, &ask(Some(40)));
    assert_eq!(short.joints.len(), 1);
    assert!(short.joints[0].distance(Vec3::new(0.0, 0.3, 0.0)) < 0.03, "the joint is at {}", short.joints[0]);
    // Enough budget to finish, and the answer is the unhurried one.
    assert_eq!(build::skeleton(&mesh, &mut scratch, &ask(Some(100_000))), whole);
}

#[test]
fn masked_vertices_stay_put_and_half_masked_ones_go_half_way() {
    let (from, to) = (Vec3::new(0.0, 0.6, THICK), Vec3::new(0.45, 0.45, THICK));
    // A fully masked band across the carried end, and a half masked one.
    let mask = |p: Vec3| if (0.7..0.8).contains(&p.y) { 1.0 } else if (0.9..1.0).contains(&p.y) { 0.5 } else { 0.0 };
    let bent = bend(poser(0.5, |_| {}), from, to, mask);
    let (mut held, mut half) = (0, 0);
    for (v, &was) in bent.before.iter().enumerate() {
        let now = bent.mesh.positions[v];
        let rigid = bent.joint + bent.turn * (was - bent.joint);
        if mask(was) == 1.0 {
            assert_eq!(now, was, "masked vertex {v} moved");
            held += 1;
        } else if mask(was) == 0.5 {
            assert!(now.distance((was + rigid) * 0.5) < 1e-5, "half-masked vertex {v}");
            half += 1;
        }
    }
    assert!(held > 100 && half > 100, "{held} masked, {half} half masked");
}

#[test]
fn one_drag_is_one_undo_step_and_undo_puts_the_limb_back_exactly() {
    let mut mesh = limb();
    let before = mesh.positions.clone();
    let view = view_of(&mesh, 0.0);
    let mut sculptor = poser(0.5, |s| s.pose.joints = 2);
    let mut history = History::default();
    let (from, to) = (Vec3::new(0.0, 0.8, THICK), Vec3::new(0.4, 0.5, THICK));
    drag(&mut mesh, &mut sculptor, &view, from, to, false);
    history.push_stroke(release(&mut mesh, &mut sculptor, &view).expect("the drag moved the limb"));
    assert_eq!(history.steps(), (1, 0));
    let bent = mesh.positions.clone();
    assert!(bent != before);
    assert_eq!(history.undo(&mut mesh), Some(false));
    assert!(mesh.positions == before, "undo did not restore the limb");
    assert_eq!(history.redo(&mut mesh), Some(false));
    assert!(mesh.positions == bent);
    history.undo(&mut mesh);

    // Dragged out and back to where the press was, the limb is as it was,
    // and there is nothing to undo.
    drag(&mut mesh, &mut sculptor, &view, from, to, false);
    sculptor.sample(&mut mesh, &view, sample(&view, from, 21));
    assert!(release(&mut mesh, &mut sculptor, &view).is_none());
    assert!(mesh.positions == before);
}

// --------------------------------------------------------------- a chain

#[test]
fn a_chain_keeps_its_lengths_reaches_the_pointer_and_holds_its_anchor() {
    let mut mesh = limb();
    let before = mesh.positions.clone();
    let view = view_of(&mesh, 0.0);
    let grabbed = Vec3::new(0.0, 0.9, THICK);
    let mut sculptor = poser(0.3, |s| s.pose.joints = 3);
    let lengths = |chain: &[Vec3]| chain.windows(2).map(|pair| pair[0].distance(pair[1])).collect::<Vec<f32>>();

    // Within reach, out of reach, and folded back on itself.
    for to in [Vec3::new(0.5, 0.5, THICK), Vec3::new(1.4, 0.9, THICK), Vec3::new(0.15, 0.1, THICK)] {
        let rest = drag(&mut mesh, &mut sculptor, &view, grabbed, to, false);
        assert_eq!(rest.len(), 4);
        let total: f32 = lengths(&rest).iter().sum();
        let anchor = rest[3];
        let chain = chains(&sculptor)[0].clone();
        for (now, was) in lengths(&chain).iter().zip(lengths(&rest)) {
            assert!((now - was).abs() < 1e-5, "a segment went from {was} to {now} long");
        }
        assert_eq!(chain[3], anchor, "the anchor moved");
        let within_reach = to.distance(chain[3]) < total;
        if within_reach {
            assert!(chain[0].distance(to) < 1e-3, "the grabbed point is {} from the pointer", chain[0].distance(to));
        } else {
            // Out of reach the chain lies straight, pointing at the pointer.
            let straight = chain[3] + (to - chain[3]).normalize() * total;
            assert!(chain[0].distance(straight) < 1e-3, "{} / {straight}", chain[0]);
        }
        // Below the anchor's bend nothing has moved at all.
        for (v, &was) in before.iter().enumerate() {
            if was.y < anchor.y - 0.15 {
                assert_eq!(mesh.positions[v], was);
            }
        }
        // And the skin of every segment still sits as far from its own
        // joint as it did: carried, not stretched.
        let mut carried = 0;
        for (v, &was) in before.iter().enumerate() {
            for k in 0..3 {
                let (low, high) = (rest[k + 1].y + 0.1, rest[k].y.min(rest[k + 1].y + 0.2));
                if was.y > low && was.y < high {
                    let (now, joint) = (mesh.positions[v], chain[k + 1]);
                    assert!((now.distance(joint) - was.distance(rest[k + 1])).abs() < 1e-4, "segment {k} was stretched");
                    carried += 1;
                }
            }
        }
        assert!(carried > 300);
        let delta = release(&mut mesh, &mut sculptor, &view).expect("the chain moved");
        mesh.apply_delta(&delta, true);
        let _ = mesh.take_dirty();
        assert!(mesh.positions == before);
    }
}

// ----------------------------------------------------- twist and the rest

#[test]
fn ctrl_twists_the_limb_about_its_own_length() {
    let mut mesh = limb();
    let before = mesh.positions.clone();
    let view = view_of(&mesh, 0.0);
    let grabbed = Vec3::new(0.0, 0.6, THICK);
    // Across the limb, by a quarter of the way round it: the grabbed skin
    // goes with the pointer.
    let across = FRAC_PI_2 * THICK;
    let mut sculptor = poser(0.5, |_| {});
    let rest = drag(&mut mesh, &mut sculptor, &view, grabbed, grabbed + Vec3::new(across, 0.0, 0.0), true);
    let joint = chains(&sculptor)[0][1];
    assert_eq!(joint, rest[1], "a twist leaves the joint where it is");
    assert!(release(&mut mesh, &mut sculptor, &view).is_some());
    let (mut turned, mut still) = (0, 0);
    for (v, &was) in before.iter().enumerate() {
        let now = mesh.positions[v];
        if was.y < joint.y - 0.2 {
            assert_eq!(now, was);
            still += 1;
        } else if was.y > joint.y + 0.2 && was.y < HALF {
            // A quarter turn about the limb's centre line: the front goes to
            // the right, as the pointer did. (The limb's line and thickness
            // are found from its skin, so to within a little.)
            let expect = Vec3::new(was.z, was.y, -was.x);
            assert!(now.distance(expect) < 0.15 * THICK, "vertex {v}: {was} went to {now}");
            turned += 1;
        }
    }
    assert!(turned > 500 && still > 500);
}

#[test]
fn resize_puts_the_grabbed_point_on_the_pointer_and_slide_moves_without_turning() {
    let grabbed = Vec3::new(0.0, 0.6, THICK);
    let view_and_limb = || {
        let mesh = limb();
        (view_of(&mesh, 0.0), mesh.positions.clone(), mesh)
    };

    // Resize: bigger about the joint, and turned to follow.
    let (view, before, mut mesh) = view_and_limb();
    let to = Vec3::new(0.3, 0.95, THICK);
    let mut sculptor = poser(0.5, |s| s.pose.mode = PoseMode::Resize);
    let rest = drag(&mut mesh, &mut sculptor, &view, grabbed, to, false);
    let chain = chains(&sculptor)[0].clone();
    let joint = chain[1];
    assert!(chain[0].distance(to) < 1e-3, "the grabbed point is at {}", chain[0]);
    release(&mut mesh, &mut sculptor, &view);
    let size = chain[0].distance(joint) / rest[0].distance(joint);
    let turn = turn_about(joint, rest[0], chain[0]);
    assert!(size > 1.4);
    for (v, &was) in before.iter().enumerate() {
        if was.y > joint.y + 0.2 {
            assert!(mesh.positions[v].distance(joint + turn * (was - joint) * size) < 1e-4, "vertex {v}");
        } else if was.y < joint.y - 0.2 {
            assert_eq!(mesh.positions[v], was);
        }
    }

    // Slide (Ctrl): the same shift for everything carried.
    let (view, before, mut mesh) = view_and_limb();
    let wanted = Vec3::new(0.25, -0.1, 0.0);
    let rest = drag(&mut mesh, &mut sculptor, &view, grabbed, grabbed + wanted, true);
    let chain = chains(&sculptor)[0].clone();
    let (joint, by) = (chain[1], chain[0] - rest[0]);
    assert!(by.distance(wanted) < 1e-3, "slid by {by}");
    release(&mut mesh, &mut sculptor, &view);
    for (v, &was) in before.iter().enumerate() {
        if was.y > joint.y + 0.2 {
            assert!(mesh.positions[v].distance(was + by) < 1e-5, "vertex {v}");
        } else if was.y < joint.y - 0.2 {
            assert_eq!(mesh.positions[v], was);
        }
    }
}

#[test]
fn stretch_lengthens_the_limb_and_thins_it_to_keep_its_volume() {
    let mut mesh = limb();
    let before = mesh.positions.clone();
    let view = view_of(&mesh, 0.0);
    let grabbed = Vec3::new(0.0, 0.6, THICK);
    let mut sculptor = poser(0.5, |s| s.pose.mode = PoseMode::Stretch);
    // Up the limb by half the distance from the joint, and a little to the
    // side, which a stretch takes no notice of.
    let joint = hover(&mesh, &mut sculptor, grabbed)[1];
    let reach = grabbed.y - joint.y;
    drag(&mut mesh, &mut sculptor, &view, grabbed, grabbed + Vec3::new(0.05, 0.5 * reach, 0.0), false);
    release(&mut mesh, &mut sculptor, &view);
    let (long, thin) = (1.5f32, 1.5f32.sqrt().recip());
    let mut checked = 0;
    for (v, &was) in before.iter().enumerate() {
        let now = mesh.positions[v];
        if was.y > joint.y + 0.2 {
            let expect = Vec3::new(was.x * thin, joint.y + (was.y - joint.y) * long, was.z * thin);
            // The limb's own line is found from its skin, so it is the
            // centre line to within a little.
            assert!(now.distance(expect) < 0.02, "vertex {v}: {was} went to {now}, not {expect}");
            checked += 1;
        } else if was.y < joint.y - 0.2 {
            assert_eq!(now, was);
        }
    }
    assert!(checked > 500);
}

// --------------------------------------------------------------- symmetry

#[test]
fn a_mirrored_drag_gives_a_mirrored_result() {
    for (joints, other) in [(1, false), (3, false), (1, true)] {
        let mut mesh = two_limbs();
        let twins = twins(&mesh);
        let before = mesh.positions.clone();
        let view = view_of(&mesh, 0.0);
        let mut sculptor = poser(0.4, |s| {
            s.symmetry = [true, false, false];
            s.pose.joints = joints;
        });
        let grabbed = Vec3::new(0.6, 0.8, THICK);
        assert_eq!(chains_at(&mesh, &mut sculptor, grabbed), 2, "a chain for each limb");
        drag(&mut mesh, &mut sculptor, &view, grabbed, Vec3::new(0.95, 0.55, THICK), other);
        assert!(release(&mut mesh, &mut sculptor, &view).is_some());
        let flip = Vec3::new(-1.0, 1.0, 1.0);
        let (mut worst, mut furthest) = (0.0f32, 0.0f32);
        for (v, &t) in twins.iter().enumerate() {
            worst = worst.max(mesh.positions[t as usize].distance(mesh.positions[v] * flip));
            furthest = furthest.max(mesh.positions[v].distance(before[v]));
        }
        assert!(furthest > 0.1, "{joints} joints, ctrl {other}: the drag moved the limbs {furthest}");
        assert!(worst < 2e-6, "{joints} joints, ctrl {other}: the two sides differ by {worst}");
    }
}

fn chains_at(mesh: &Mesh, sculptor: &mut Sculptor, at: Vec3) -> usize {
    sculptor.pose_hover(mesh, Some((at, sculptor.settings.radius_world)), false);
    sculptor.pose_chains().len()
}

/// A limb on the mirror plane is held by both copies of the brush. It is
/// posed once: it follows the pointer in the plane exactly as it would with
/// the mirror off, and cannot be dragged off to one side.
#[test]
fn a_limb_on_the_mirror_plane_bends_in_the_plane_and_stays_rigid() {
    let (positions, indices) = mirror_limb(0.0);
    let build = || Mesh::new(positions.clone(), indices.clone(), None);
    let twins = twins(&build());
    // Grabbed on its side, seen from that side: the screen is the mirror
    // plane.
    let side = view_of(&build(), -90.0).pos.x.signum() * THICK;
    let grabbed = Vec3::new(side, 0.7, 0.0);
    let posed = |from: Vec3, to: Vec3, yaw: f32| {
        let mut mesh = build();
        let view = view_of(&mesh, yaw);
        let mut sculptor = poser(0.4, |s| s.symmetry = [true, false, false]);
        assert_eq!(chains_at(&mesh, &mut sculptor, from), 1, "one chain for the one limb");
        let rest = drag(&mut mesh, &mut sculptor, &view, from, to, false);
        let chain = chains(&sculptor)[0].clone();
        release(&mut mesh, &mut sculptor, &view);
        (mesh, rest, chain)
    };
    let before = build().positions;
    let flip = Vec3::new(-1.0, 1.0, 1.0);
    let mirrored = |mesh: &Mesh| {
        let apart = |(v, &t): (usize, &u32)| mesh.positions[t as usize].distance(mesh.positions[v] * flip);
        twins.iter().enumerate().map(apart).fold(0.0, f32::max)
    };

    // Forward, in the plane.
    let to = Vec3::new(side, 0.5, 0.35);
    let (mesh, rest, chain) = posed(grabbed, to, -90.0);
    assert!(mirrored(&mesh) < 2e-6, "the two sides differ by {}", mirrored(&mesh));
    assert!(chain[1].x.abs() < 1e-6 && chain[0].x.abs() < 1e-6, "the chain lies in the plane: {chain:?}");
    let joint = chain[1];
    // The grabbed point and the pointer, as the two copies see them between
    // them: straight in from where each is, onto the plane.
    let in_plane = Vec3::new(0.0, 1.0, 1.0);
    // (To within the flats of this coarser limb, which the press lands on.)
    assert!(rest[0].distance(grabbed * in_plane) < 5e-3, "grabbed at {}", rest[0]);
    assert!((chain[0] - joint).normalize().distance((to * in_plane - joint).normalize()) < 5e-3, "{chain:?}");
    let turn = turn_about(joint, rest[0], chain[0]);
    let mut carried = 0;
    for (v, &was) in before.iter().enumerate() {
        if was.y > joint.y + 0.25 {
            let turned = joint + turn * (was - joint);
            assert!(mesh.positions[v].distance(turned) < 1e-5, "vertex {v} is not where a turn in the plane puts it");
            carried += 1;
        }
    }
    assert!(carried > 500);

    // To one side, seen from the front: the pointer's motion out of the
    // plane is what the two copies disagree on, and nothing of it is left.
    let from_front = Vec3::new(0.0, 0.7, THICK);
    let (mesh, _, _) = posed(from_front, from_front + Vec3::new(0.4, 0.0, 0.0), 0.0);
    assert!(mirrored(&mesh) < 2e-6);
    let moved = mesh.positions.iter().zip(&before).map(|(a, b)| a.distance(*b)).fold(0.0, f32::max);
    assert!(moved < 1e-5, "a drag across the mirror plane moved the limb {moved}");
}

// -------------------------------------------------------- radial symmetry

/// Eight repeats round Y, the line the test limb stands on.
fn eightfold(settings: &mut Settings) {
    settings.radial = Radial { on: true, count: 8, ..Radial::default() };
}

/// For every vertex, the vertex an eighth of a turn round Y from it. The
/// limb has 32 points to a ring, so there always is one.
fn eighth_turn_partners(positions: &[Vec3]) -> (SymmetryOp, Vec<u32>) {
    let turn = SymmetryOp::rotation(Vec3::Y, 1, 8);
    let key = |p: Vec3| (p * 2e4).round().as_ivec3().to_array();
    let map: HashMap<[i32; 3], u32> = positions.iter().enumerate().map(|(i, &p)| (key(p), i as u32)).collect();
    let partner = |&p: &Vec3| *map.get(&key(turn.apply(p))).expect("the limb is not the same an eighth of a turn round");
    (turn, positions.iter().map(partner).collect())
}

/// How far the mesh is from being the same an eighth of a turn round.
fn turned_apart(mesh: &Mesh, (turn, partners): &(SymmetryOp, Vec<u32>)) -> f32 {
    let apart = |(v, &t): (usize, &u32)| mesh.positions[t as usize].distance(turn.apply(mesh.positions[v]));
    partners.iter().enumerate().map(apart).fold(0.0, f32::max)
}

/// A limb that stands on the radial axis is held by every copy of the brush
/// at once, and is posed once. No bend is the same from every side, so a
/// bend leaves it where it is, however far the pointer goes (and does not
/// turn it end for end when the pointer passes the joint). Twisting it about
/// the axis, resizing it, sliding it along the axis and stretching it are
/// the same from every side, and work.
#[test]
fn a_limb_on_the_radial_axis_does_not_bend_but_twists_resizes_slides_and_stretches() {
    let before = limb().positions;
    let partners = eighth_turn_partners(&before);
    let grabbed = Vec3::new(0.0, 0.6, THICK);
    let posed = |setup: &dyn Fn(&mut Settings), to: Vec3, other: bool| {
        let mut mesh = limb();
        let view = view_of(&mesh, 0.0);
        let mut sculptor = poser(0.5, |s| {
            eightfold(s);
            setup(s);
        });
        assert_eq!(chains_at(&mesh, &mut sculptor, grabbed), 1, "one chain for the one limb");
        let rest = drag(&mut mesh, &mut sculptor, &view, grabbed, to, other);
        let chain = chains(&sculptor)[0].clone();
        release(&mut mesh, &mut sculptor, &view);
        assert!(mesh.positions.iter().all(|p| p.is_finite()));
        let apart = turned_apart(&mesh, &partners);
        assert!(apart < 5e-6, "the limb is {apart} from the same all the way round");
        (mesh, rest, chain)
    };
    let moved = |mesh: &Mesh| mesh.positions.iter().zip(&before).map(|(a, b)| a.distance(*b)).fold(0.0, f32::max);

    // The chain the copies share is the limb's own centre line.
    let (_, rest, _) = posed(&|_| {}, grabbed, false);
    assert!(rest.iter().all(|p| p.x.abs() < 1e-5 && p.z.abs() < 1e-5), "the chain is off the axis: {rest:?}");
    let joint = rest[1];

    // Bend: to one side, and right round past the joint.
    for to in [grabbed + Vec3::new(0.5, 0.1, 0.0), Vec3::new(0.3, joint.y - 0.6, THICK)] {
        for joints in [1, 3] {
            let (mesh, _, _) = posed(&|s| s.pose.joints = joints, to, false);
            assert!(moved(&mesh) < 1e-5, "a bend to {to} with {joints} joints moved the limb {}", moved(&mesh));
        }
    }

    // Twist: a quarter of the way round, as with no symmetry.
    let (mesh, _, _) = posed(&|_| {}, grabbed + Vec3::new(FRAC_PI_2 * THICK, 0.0, 0.0), true);
    let mut turned = 0;
    for (v, &was) in before.iter().enumerate() {
        if was.y < joint.y - 0.2 {
            assert_eq!(mesh.positions[v], was);
        } else if was.y > joint.y + 0.2 && was.y < HALF {
            let expect = Vec3::new(was.z, was.y, -was.x);
            assert!(mesh.positions[v].distance(expect) < 0.15 * THICK, "vertex {v}: {was} went to {}", mesh.positions[v]);
            turned += 1;
        }
    }
    assert!(turned > 500);

    // Resize: bigger about the joint by as much as the pointer went up the
    // axis, and not turned toward it.
    let resize = |s: &mut Settings| s.pose.mode = PoseMode::Resize;
    let (mesh, rest, chain) = posed(&resize, Vec3::new(0.3, 0.95, THICK), false);
    let size = (chain[0].y - joint.y) / (rest[0].y - joint.y);
    assert!(size > 1.4 && chain[0].x.abs() < 1e-5 && chain[0].z.abs() < 1e-5, "{chain:?}");
    assert!((chain[0].y - 0.95).abs() < 1e-3, "the grabbed point went up the axis to {}", chain[0]);
    for (v, &was) in before.iter().enumerate() {
        if was.y > joint.y + 0.2 {
            assert!(mesh.positions[v].distance(joint + (was - joint) * size) < 1e-4, "vertex {v}");
        } else if was.y < joint.y - 0.2 {
            assert_eq!(mesh.positions[v], was);
        }
    }
    // Dragged down through the joint, it is as small as it gets, not inside out.
    let (mesh, _, chain) = posed(&resize, Vec3::new(0.1, joint.y - 0.5, THICK), false);
    assert!(chain[0].y > joint.y && moved(&mesh) > 0.1, "{chain:?}");

    // Slide: only the part of the drag that runs along the axis.
    let (mesh, rest, chain) = posed(&resize, grabbed + Vec3::new(0.25, -0.1, 0.0), true);
    assert!((chain[0] - rest[0]).distance(Vec3::new(0.0, -0.1, 0.0)) < 1e-3, "slid by {}", chain[0] - rest[0]);
    assert!(moved(&mesh) > 0.09 && moved(&mesh) < 0.11);

    // Stretch: half as long again, and thinner, as with no symmetry.
    let stretch = |s: &mut Settings| s.pose.mode = PoseMode::Stretch;
    let (mesh, _, _) = posed(&stretch, grabbed + Vec3::new(0.05, 0.5 * (grabbed.y - joint.y), 0.0), false);
    let (long, thin) = (1.5f32, 1.5f32.sqrt().recip());
    for (v, &was) in before.iter().enumerate() {
        if was.y > joint.y + 0.2 {
            let expect = Vec3::new(was.x * thin, joint.y + (was.y - joint.y) * long, was.z * thin);
            assert!(mesh.positions[v].distance(expect) < 0.02, "vertex {v}: {was} went to {}", mesh.positions[v]);
        }
    }

    // With a mirror through the axis as well, the mirror undoes the twist.
    let mirrored = |s: &mut Settings| s.symmetry = [true, false, false];
    let (mesh, _, _) = posed(&mirrored, grabbed + Vec3::new(FRAC_PI_2 * THICK, 0.0, 0.0), true);
    assert!(moved(&mesh) < 1e-5, "a twist moved a limb that is mirrored as well: {}", moved(&mesh));
}

/// Limbs round the axis, one to a copy of the brush (the arms of a starfish):
/// each gets a joint of its own and they all bend alike. With a mirror
/// through the axis as well, each arm lies on a mirror plane and bends
/// within it.
#[test]
fn limbs_round_the_radial_axis_all_bend_alike() {
    let quarter_turns = SymmetryGroup::build(false, false, false, 4, Vec3::Y);
    let (arm, arm_indices) = mirror_limb(0.6);
    let n = arm.len() as u32;
    let (mut positions, mut indices) = (Vec::new(), Vec::new());
    for (k, turn) in quarter_turns.iter().enumerate() {
        positions.extend(arm.iter().map(|&p| turn.apply(p)));
        indices.extend(arm_indices.iter().map(|&i| i + n * k as u32));
    }
    let arms = || Mesh::new_mapped(positions.clone(), indices.clone(), None);
    // Each vertex's partner a quarter turn on is the same vertex of the next arm.
    let in_mesh = arms().1;
    let mut next = vec![0u32; positions.len()];
    for i in 0..positions.len() {
        next[in_mesh[i] as usize] = in_mesh[(i + arm.len()) % positions.len()];
    }
    let partners = (quarter_turns[1], next);

    let grabbed = Vec3::new(0.6, 0.8, THICK);
    for mirror in [false, true] {
        for (joints, other) in [(1, false), (3, false), (1, true)] {
            let (mut mesh, _) = arms();
            let before = mesh.positions.clone();
            let view = view_of(&mesh, 0.0);
            let mut sculptor = poser(0.4, |s| {
                s.radial = Radial { on: true, count: 4, ..Radial::default() };
                s.symmetry = [mirror, false, false];
                s.pose.joints = joints;
            });
            assert_eq!(chains_at(&mesh, &mut sculptor, grabbed), 4, "a chain for each arm");
            drag(&mut mesh, &mut sculptor, &view, grabbed, Vec3::new(0.95, 0.55, THICK), other);
            let twisting_a_mirrored_arm = mirror && other;
            assert_eq!(release(&mut mesh, &mut sculptor, &view).is_some(), !twisting_a_mirrored_arm);
            let apart = turned_apart(&mesh, &partners);
            let furthest = mesh.positions.iter().zip(&before).map(|(a, b)| a.distance(*b)).fold(0.0, f32::max);
            let what = format!("mirror {mirror}, {joints} joints, ctrl {other}");
            assert!(apart < 5e-6, "{what}: the arms differ by {apart}");
            assert_eq!(furthest > 0.1, !twisting_a_mirrored_arm, "{what}: the drag moved the arms {furthest}");
            if mirror {
                // The arm that was grabbed lies on the Z = 0 mirror plane,
                // and is still its own mirror image across it.
                let across = |p: Vec3| Vec3::new(p.x, p.y, -p.z);
                let key = |p: Vec3| (p * 2e4).round().as_ivec3().to_array();
                let at: HashMap<[i32; 3], usize> = before.iter().enumerate().map(|(i, &p)| (key(p), i)).collect();
                let worst = before
                    .iter()
                    .enumerate()
                    .map(|(v, &p)| mesh.positions[at[&key(across(p))]].distance(across(mesh.positions[v])))
                    .fold(0.0, f32::max);
                assert!(worst < 5e-6, "{what}: the arms are {worst} from mirror images of themselves");
            }
        }
    }
}

// ----------------------------------------------------------------- hidden

#[test]
fn hidden_surface_is_neither_moved_nor_crossed() {
    let mut mesh = limb();
    // A band of the limb hidden, between the grabbed end and the joint the
    // brush would otherwise find.
    let band: Vec<u32> = (0..mesh.triangle_count() as u32)
        .filter(|&t| (0..3).all(|c| (0.3..0.5).contains(&mesh.positions[mesh.indices[t as usize * 3 + c] as usize].y)))
        .collect();
    assert!(mesh.hide(&band).is_some());
    let hidden = mesh.hidden_vertices().unwrap().to_vec();
    assert!(hidden.iter().filter(|&&h| h).count() > 50);
    let before = mesh.positions.clone();
    let view = view_of(&mesh, 0.0);
    let grabbed = Vec3::new(0.0, 0.8, THICK);
    let mut sculptor = poser(0.6, |_| {});
    drag(&mut mesh, &mut sculptor, &view, grabbed, Vec3::new(0.4, 0.6, THICK), false);
    assert!(release(&mut mesh, &mut sculptor, &view).is_some());
    let mut moved = 0;
    for (v, &was) in before.iter().enumerate() {
        if hidden[v] {
            assert_eq!(mesh.positions[v], was, "a hidden vertex moved");
        } else if was.y < 0.3 {
            assert_eq!(mesh.positions[v], was, "the brush reached across the hidden band");
        } else if mesh.positions[v] != was {
            moved += 1;
        }
    }
    assert!(moved > 500);
}

// ----------------------------------------------------------------- timing

/// How long the preview, a press and a frame of the drag take on dense
/// models: with the brush on most of the model (it takes hold of nine tenths
/// of it, far more than posing a figure does) and on a tenth of it. The
/// search runs on one thread, the drag on all of them. Each is the best of
/// several goes: the machine is shared, and one go can be twice another.
#[test]
#[ignore = "timing run: cargo test --release pose_timing -- --ignored --nocapture"]
fn pose_timing() {
    let best = |times: &mut dyn FnMut() -> f64| (0..5).map(|_| times()).fold(f64::MAX, f64::min);
    let ms = |from: std::time::Instant| from.elapsed().as_secs_f64() * 1e3;
    let runs = [(1_000_000, 0.8, "most of it"), (4_000_000, 0.8, "most of it"), (4_000_000, 0.2, "a tenth")];
    for (tris, radius, what) in runs {
        let capsule = primitives::build(Primitive::Capsule, tris, 0.25);
        let positions = capsule.positions.iter().map(|&p| p * Vec3::new(1.0, 4.0, 1.0)).collect();
        let mut mesh = Mesh::new(positions, capsule.indices.clone(), None);
        let view = view_of(&mesh, 0.0);
        let grabbed = Vec3::new(0.0, 1.8, 0.25);
        let mut sculptor = poser(radius, |s| s.pose.joints = 3);
        sculptor.pose_hover(&mesh, Some((grabbed, radius)), false);
        let hover_ms = best(&mut || {
            let t = std::time::Instant::now();
            sculptor.pose_hover(&mesh, Some((grabbed, radius)), false);
            ms(t)
        });
        let (mut press_ms, mut frame_ms, mut moved) = (f64::MAX, f64::MAX, 0);
        for _ in 0..5 {
            let t = std::time::Instant::now();
            assert!(sculptor.begin(&mut mesh, &view, sample(&view, grabbed, 0), false, false));
            press_ms = press_ms.min(ms(t));
            for step in 1..=20 {
                let to = grabbed + Vec3::new(0.03 * step as f32, 0.0, 0.0);
                let t = std::time::Instant::now();
                sculptor.sample(&mut mesh, &view, sample(&view, to, step));
                sculptor.frame(&mut mesh, &view, 1.0 / 60.0);
                let _ = mesh.take_dirty();
                frame_ms = frame_ms.min(ms(t));
            }
            let delta = release(&mut mesh, &mut sculptor, &view).expect("the drag moved the limb");
            if let StrokeDelta::Positions { verts, .. } = &delta {
                moved = verts.len();
            }
            mesh.apply_delta(&delta, true);
            let _ = mesh.take_dirty();
        }
        println!(
            "{} tris, {what}: preview {hover_ms:.1} ms, press {press_ms:.1} ms, {frame_ms:.1} ms a frame, \
             {moved} vertices moved",
            mesh.triangle_count()
        );
    }
}
