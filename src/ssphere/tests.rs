//! Pins what the limb-based skin promises (see skin.rs): a chain of equal
//! spheres is a clean tube with no bead at any sphere, a branch leaves no
//! knob on the far side of the limb it grows from, a bent joint is exactly
//! its sphere on the outside, every sphere's radius survives into the skin,
//! and a symmetric rig skins to a closed, mirror-exact mesh. Also the rig's
//! symmetry bookkeeping, the two-bone pose solve, and the main gestures. And
//! what attaching promises (see frame.rs): no sphere moves when the rig is
//! attached or detached, the mirror plane becomes the object's own, the rig
//! goes with the object afterwards, and each of those is undone exactly.

use std::collections::{HashMap, HashSet};

use glam::{Mat4, Quat, Vec2, Vec3};

use super::rig::Rig;
use super::skin::{self, Quality, Skin, SkinSettings};
use super::{Body, Ctx, EditMode, Frame, Hit, Press, SSpheres, pose};
use crate::camera::{Camera, View};
use crate::mesh::Mesh;

fn skin_of(rig: &Rig, symmetry: bool, quality: Quality) -> Skin {
    skin::skin(rig, symmetry, &SkinSettings::default(), quality).expect("the rig skins")
}

/// Adds a chain of spheres below `from` and returns its last one.
fn chain(rig: &mut Rig, from: usize, spheres: &[(Vec3, f32)]) -> usize {
    spheres.iter().fold(from, |parent, &(p, r)| rig.add_child(parent, p, r).unwrap())
}

fn segment_distance(p: Vec3, a: Vec3, b: Vec3) -> f32 {
    let ab = b - a;
    p.distance(a + ab * ((p - a).dot(ab) / ab.dot(ab)).clamp(0.0, 1.0))
}

/// A small figure: spine, head, one arm with three fingers and one leg, the
/// other side left to symmetry.
fn figure() -> Rig {
    let v = Vec3::new;
    let mut rig = Rig::default();
    let pelvis = rig.add_root(v(0.0, 1.0, 0.0), 0.16);
    let chest = chain(&mut rig, pelvis, &[(v(0.0, 1.2, 0.02), 0.14), (v(0.0, 1.42, 0.0), 0.18)]);
    chain(&mut rig, chest, &[(v(0.0, 1.63, 0.0), 0.065), (v(0.0, 1.8, 0.02), 0.12)]);
    let arm = [(v(0.2, 1.52, 0.0), 0.08), (v(0.44, 1.33, 0.02), 0.055), (v(0.62, 1.1, 0.14), 0.045)];
    let hand = chain(&mut rig, chest, &arm);
    for finger in [-1.0, 0.0, 1.0] {
        chain(&mut rig, hand, &[(v(0.7, 1.0, 0.18 + finger * 0.035), 0.018)]);
    }
    chain(&mut rig, pelvis, &[(v(0.1, 0.93, 0.0), 0.11), (v(0.13, 0.52, 0.04), 0.07), (v(0.14, 0.1, 0.0), 0.05)]);
    rig
}

/// Asserts every edge is used as often one way as the other (no holes or
/// cracks) and returns how many are shared by more than two faces.
fn assert_closed(indices: &[u32], what: &str) -> usize {
    let mut edges: HashMap<(u32, u32), i32> = HashMap::new();
    for t in indices.chunks_exact(3) {
        for k in 0..3 {
            *edges.entry((t[k], t[(k + 1) % 3])).or_default() += 1;
        }
    }
    let open = edges.iter().filter(|&(&(a, b), &n)| edges.get(&(b, a)) != Some(&n)).count();
    assert_eq!(open, 0, "{what}: {open} of {} edges are open or mis-oriented", edges.len());
    edges.values().filter(|&&n| n > 1).count()
}

fn volume(positions: &[Vec3], indices: &[u32]) -> f64 {
    indices
        .chunks_exact(3)
        .map(|t| {
            let p = [0, 1, 2].map(|c| positions[t[c] as usize].as_dvec3());
            p[0].dot(p[1].cross(p[2])) / 6.0
        })
        .sum()
}

fn bits(p: Vec3) -> [u32; 3] {
    // "+ 0.0" folds -0 into +0, which would otherwise differ.
    [(p.x + 0.0).to_bits(), (p.y + 0.0).to_bits(), (p.z + 0.0).to_bits()]
}

fn assert_mirror_exact(positions: &[Vec3], what: &str) {
    let set: HashSet<[u32; 3]> = positions.iter().map(|&p| bits(p)).collect();
    let missing = positions.iter().filter(|p| !set.contains(&bits(Vec3::new(-p.x, p.y, p.z)))).count();
    assert_eq!(missing, 0, "{what}: vertices without an exact mirror partner");
}

#[test]
fn a_single_sphere_skins_to_a_sphere() {
    let mut rig = Rig::default();
    rig.add_root(Vec3::new(0.3, 0.2, -0.1), 0.3);
    let skin = skin_of(&rig, false, Quality::Final);
    assert_eq!(assert_closed(&skin.indices, "sphere"), 0);
    for (p, n) in skin.positions.iter().zip(&skin.normals) {
        let from_centre = *p - Vec3::new(0.3, 0.2, -0.1);
        assert!((from_centre.length() - 0.3).abs() < 0.3 * 0.01, "vertex off the sphere: {}", from_centre.length());
        assert!(n.dot(from_centre.normalize()) > 0.99, "normals point outward");
    }
    let v = volume(&skin.positions, &skin.indices);
    let truth = 4.0 / 3.0 * std::f64::consts::PI * 0.027;
    assert!(v > 0.0, "faces point outward");
    assert!((v - truth).abs() / truth < 0.05, "volume {v} against {truth}");
}

#[test]
fn straight_chain_of_equal_spheres_has_no_beads() {
    let mut rig = Rig::default();
    let mut prev = rig.add_root(Vec3::ZERO, 0.2);
    for i in 1..6 {
        prev = rig.add_child(prev, Vec3::new(0.0, i as f32 * 0.3, 0.0), 0.2).unwrap();
    }
    let skin = skin_of(&rig, false, Quality::Final);
    let (mut lo, mut hi) = (f32::MAX, 0.0f32);
    for p in skin.positions.iter().filter(|p| p.y > 0.25 && p.y < 1.25) {
        let r = Vec2::new(p.x, p.z).length();
        lo = lo.min(r);
        hi = hi.max(r);
    }
    // Smooth-joining every piece swells 17% at every sphere.
    assert!(hi < 0.2 * 1.02, "chain bulges at its spheres: {hi}");
    assert!(lo > 0.2 * 0.98, "chain pinches between its spheres: {lo}");
}

#[test]
fn branch_leaves_no_knob_opposite_it() {
    let mut rig = Rig::default();
    let mut prev = rig.add_root(Vec3::ZERO, 0.2);
    let mut mid = prev;
    for i in 1..5 {
        prev = rig.add_child(prev, Vec3::new(0.0, i as f32 * 0.3, 0.0), 0.2).unwrap();
        if i == 2 {
            mid = prev;
        }
    }
    chain(&mut rig, mid, &[(Vec3::new(0.35, 0.6, 0.0), 0.13), (Vec3::new(0.7, 0.62, 0.0), 0.11)]);

    let skin = skin_of(&rig, false, Quality::Final);
    let hi = skin
        .positions
        .iter()
        .filter(|p| p.x < -0.02 && p.y > 0.35 && p.y < 0.85)
        .map(|p| Vec2::new(p.x, p.z).length())
        .fold(0.0f32, f32::max);
    // An unfaded fillet pushes a 22% knob out of the trunk's far side.
    assert!(hi < 0.2 * 1.02, "knob opposite the branch: {hi}");
}

#[test]
fn bent_joint_is_its_sphere_on_the_outside() {
    let v = Vec3::new;
    let mut rig = Rig::default();
    let a = rig.add_root(Vec3::ZERO, 0.15);
    chain(&mut rig, a, &[(v(0.0, 0.6, 0.0), 0.15), (v(0.6, 0.6, 0.0), 0.15), (v(0.95, 0.75, 0.0), 0.12)]);

    let skin = skin_of(&rig, false, Quality::Final);
    let joint = v(0.0, 0.6, 0.0);
    let outward = v(-1.0, 1.0, 0.0).normalize();
    let hi = skin
        .positions
        .iter()
        .filter(|&&p| (p - joint).dot(outward) > 0.05 && p.x < 0.3 && p.y > 0.3)
        .map(|&p| segment_distance(p, Vec3::ZERO, joint).min(segment_distance(p, joint, v(0.6, 0.6, 0.0))))
        .fold(0.0f32, f32::max);
    // A fillet all round the joint grows an 18% knob on the outside of the bend.
    assert!(hi < 0.15 * 1.02, "knob outside the bend: {hi}");
}

#[test]
fn every_sphere_keeps_its_radius() {
    let v = Vec3::new;
    let radii = [0.12, 0.22, 0.1, 0.17, 0.05];
    let centres = [v(0.0, 0.0, 0.0), v(0.1, 0.35, 0.0), v(0.05, 0.75, 0.0), v(-0.1, 1.1, 0.0), v(-0.15, 1.45, 0.0)];
    let mut rig = Rig::default();
    let mut prev = rig.add_root(centres[0], radii[0]);
    for i in 1..radii.len() {
        prev = rig.add_child(prev, centres[i], radii[i]).unwrap();
    }
    let skin = skin_of(&rig, false, Quality::Final);
    // The smooth radius profile must arrive at each sphere's radius (the
    // swellings between thin ones included). Waists may read a little
    // thicker where a fat neighbour's hull overlaps them; nothing may come
    // out thinner.
    for i in 1..radii.len() - 1 {
        let hi = skin
            .positions
            .iter()
            .filter(|p| (p.y - centres[i].y).abs() < 0.01)
            .map(|p| Vec2::new(p.x - centres[i].x, p.z - centres[i].z).length())
            .fold(0.0f32, f32::max);
        assert!(hi > radii[i] * 0.97, "sphere {i} thinner than placed: {hi}");
        assert!(hi < radii[i] * 1.15, "sphere {i} inflated: {hi}");
    }
}

#[test]
fn symmetric_figure_is_closed_and_mirror_exact() {
    let rig = figure();
    for quality in [Quality::Draft, Quality::Preview, Quality::Final] {
        let skin = skin_of(&rig, true, quality);
        let what = format!("{quality:?}");
        let pinched = assert_closed(&skin.indices, &what);
        println!(
            "{what}: {} tris at resolution {} in {:.1} ms ({} limbs, {} pieces, {pinched} pinched edges)",
            skin.stats.triangles, skin.stats.resolution, skin.stats.ms, skin.stats.limbs, skin.stats.pieces
        );
        // Where two sheets of surface pass through one voxel (between
        // fingers) they share an edge; that must stay a rarity.
        assert!(pinched <= skin.stats.triangles / 500, "{what}: {pinched} pinched edges");
        assert_mirror_exact(&skin.positions, &what);
        assert!(volume(&skin.positions, &skin.indices) > 0.0, "{what}: faces point outward");
        assert!(skin.positions.iter().all(|p| p.is_finite()));
        assert!(skin.normals.iter().all(|n| (n.length() - 1.0).abs() < 1e-3), "{what}: unit normals");
    }
}

#[test]
fn a_skin_becomes_a_sculptable_mirror_exact_mesh() {
    let skin = skin_of(&figure(), true, Quality::Preview);
    let triangles = skin.stats.triangles;
    let mesh = Mesh::new(skin.positions, skin.indices, None);
    assert_eq!(mesh.triangle_count(), triangles, "no triangle was dropped as degenerate");
    assert_mirror_exact(&mesh.positions, "mesh");
    // The figure stands on the mirror plane, so it has vertices exactly on it.
    assert!(mesh.positions.iter().any(|p| p.x == 0.0));
    let bounds = mesh.bounds();
    assert!(bounds.min.y < 0.06 && bounds.max.y > 1.9, "head to toe: {bounds:?}");
}

#[test]
fn more_detail_gives_more_triangles() {
    let rig = figure();
    let count = |density: f32| {
        let settings = SkinSettings { density, ..SkinSettings::default() };
        skin::skin(&rig, true, &settings, Quality::Final).unwrap().stats.triangles
    };
    let (coarse, normal, fine) = (count(0.5), count(1.0), count(2.0));
    assert!(coarse < normal && normal < fine, "{coarse} < {normal} < {fine}");
    // Triangles follow area: doubling the detail is about four times as many.
    assert!(fine > normal * 3, "{normal} -> {fine}");
}

#[test]
fn baking_symmetry_and_folding_it_back_is_a_round_trip() {
    let mut rig = figure();
    let before = rig.alive_count();
    let positions: Vec<Vec3> = rig.nodes().iter().map(|n| n.position).collect();
    let (spheres, links) = rig.geometry(true);

    let baked = rig.bake_symmetry();
    assert_eq!(baked, 9, "the arm, its fingers and the leg gain a twin each");
    // With the reflection baked in, the rig shows the same thing unmirrored.
    let (real_spheres, real_links) = rig.geometry(false);
    assert_eq!((real_spheres.len(), real_links.len()), (spheres.len(), links.len()));

    assert_eq!(rig.merge_mirror_pairs(), baked);
    assert_eq!(rig.alive_count(), before);
    for (node, &p) in rig.nodes().iter().zip(&positions) {
        assert_eq!(node.position, p, "the surviving half is the one that was there");
    }

    // A limb moved while symmetry was off is a deliberate edit: it stays.
    rig.bake_symmetry();
    let twin = (0..rig.nodes().len()).rev().find(|&i| rig.is_alive(i)).unwrap();
    rig.set_position(twin, rig.nodes()[twin].position + Vec3::new(0.0, 0.3, 0.0));
    assert_eq!(rig.merge_mirror_pairs(), baked - 1);
}

#[test]
fn inserting_and_removing_keep_the_tree_whole() {
    let mut rig = Rig::default();
    let root = rig.add_root(Vec3::ZERO, 0.2);
    let tip = chain(&mut rig, root, &[(Vec3::Y, 0.2), (Vec3::Y * 2.0, 0.2)]);
    let elbow = rig.parent_of(tip).unwrap();
    let mid = rig.insert_between(tip, Vec3::Y * 1.5, 0.1).unwrap();
    assert_eq!(rig.parent_of(tip), Some(mid));
    assert_eq!(rig.parent_of(mid), Some(elbow));
    assert_eq!(rig.nodes()[elbow].children, vec![mid], "the limb lengthened rather than forked");

    // Parents come before children even though `mid` was added last.
    let mut order = Vec::new();
    rig.collect_hierarchy(&mut order);
    assert_eq!(order, vec![root, elbow, mid, tip]);

    rig.remove(elbow);
    assert_eq!(rig.alive_count(), 1);
    assert!(rig.nodes()[root].children.is_empty());
    assert_eq!(rig.geometry(true).1.len(), 0);
}

#[test]
fn two_bone_solve_keeps_lengths_and_reaches_target() {
    // A small deterministic generator in -1..1.
    let mut state = 3u32;
    let mut r = || {
        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        (state >> 8) as f32 / (1 << 23) as f32 - 1.0
    };
    let mut v = || Vec3::new(r(), r(), r());
    for _ in 0..2000 {
        let root = v();
        let joint = root + v() * 0.6;
        let end = joint + v() * 0.6;
        let target = root + v();
        let (a, b) = (joint.distance(root), end.distance(joint));
        if a < 0.05 || b < 0.05 {
            continue;
        }
        let normal = pose::bend_normal(root, joint, end, Vec3::Z);
        let side = pose::bend_side(root, joint, end, normal);
        let target = root + (target - root) * (a + b) * 0.8;
        let (j, e) = pose::solve_two_bone(root, a, b, target, normal, side, end - root);
        assert!((j.distance(root) - a).abs() < a * 1e-3);
        assert!((e.distance(j) - b).abs() < b * 1e-3);
        let d = target.distance(root);
        if d < a + b - 1e-3 && d > (a - b).abs() + 1e-3 {
            assert!(e.distance(target) < (a + b) * 1e-3);
        }
    }
}

#[test]
fn straight_chain_bends_in_the_view_plane() {
    let (root, joint, end) = (Vec3::ZERO, Vec3::new(0.5, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0));
    let normal = pose::bend_normal(root, joint, end, Vec3::Z);
    let side = pose::bend_side(root, joint, end, normal);
    let (j, _) = pose::solve_two_bone(root, 0.5, 0.5, Vec3::new(0.6, 0.2, 0.0), normal, side, end - root);
    assert!(j.z.abs() < 1e-4, "bent toward the camera, where the bend cannot be seen");
}

const SIZE: Vec2 = Vec2::new(1600.0, 900.0);

/// A front view of the origin, three units back.
fn front_view() -> View {
    let camera = Camera::default();
    camera.view(Vec2::ZERO, SIZE, camera.distance, 1.0)
}

fn no_surface(_: Vec3, _: Vec3) -> Option<Hit> {
    None
}

/// An object standing square at the centre of the scene.
fn plain_body(id: u64) -> Body {
    Body { id, position: Vec3::ZERO, rotation: Quat::IDENTITY, scale: Vec3::ONE }
}

/// An object that has been moved, turned and stretched unevenly.
fn placed_body(id: u64) -> Body {
    Body {
        id,
        position: Vec3::new(0.6, -0.3, 0.4),
        rotation: Quat::from_rotation_z(0.7) * Quat::from_rotation_y(-0.4),
        scale: Vec3::new(1.5, 0.8, 1.2),
    }
}

/// The tool holding the figure, standing in the world's own space.
fn figure_tool() -> SSpheres {
    let mut tool = SSpheres::default();
    tool.rig = figure();
    tool
}

/// Where each node is in the world, in node order.
fn world_nodes(tool: &SSpheres) -> Vec<Vec3> {
    tool.rig.nodes().iter().map(|n| tool.frame().to_world(n.position)).collect()
}

/// Every sphere the rig shows, reflections included, in the world: the node
/// it belongs to, whether it is the reflection, its centre and its radius.
fn world_spheres(tool: &SSpheres) -> Vec<(usize, bool, Vec3, f32)> {
    let frame = tool.frame();
    let spheres = tool.rig.geometry(tool.symmetry()).0;
    spheres.iter().map(|s| (s.node, s.mirrored, frame.to_world(s.centre), s.radius * frame.scale)).collect()
}

fn assert_same_points(a: &[Vec3], b: &[Vec3], what: &str) {
    assert_eq!(a.len(), b.len(), "{what}");
    for (i, (p, q)) in a.iter().zip(b).enumerate() {
        assert!(p.distance(*q) < 1e-4, "{what}: sphere {i} went from {p} to {q}");
    }
}

/// Asserts every reflection is its node mirrored in the plane through
/// `body` across its own x axis.
fn assert_mirrors_across(tool: &SSpheres, body: &Body) {
    let nodes = world_nodes(tool);
    let normal = body.rotation * Vec3::X;
    let mut reflections = 0;
    for (node, mirrored, centre, _) in world_spheres(tool) {
        if mirrored {
            let real = nodes[node];
            let reflected = real - 2.0 * (real - body.position).dot(normal) * normal;
            assert!(centre.distance(reflected) < 1e-4, "sphere {node} mirrors to {centre}, not {reflected}");
            reflections += 1;
        }
    }
    assert!(reflections > 0, "nothing was mirrored");
}

/// Presses at `from`, drags to `to` in steps and lets go.
fn drag(tool: &mut SSpheres, cx: &Ctx, from: Vec2, to: Vec2) -> Press {
    let press = tool.press(cx, from, false, false);
    for i in 1..=10 {
        tool.drag_to(cx, from.lerp(to, i as f32 / 10.0));
    }
    tool.release(cx);
    press
}

#[test]
fn gestures_build_a_rig_and_undo_takes_it_apart_again() {
    let view = front_view();
    let cx = Ctx { view: &view, pivot: Vec3::ZERO, surface: &no_surface, body: None };
    let mut tool = SSpheres::default();
    let centre = SIZE * 0.5;

    // A click in the empty viewport places the first sphere, on the mirror
    // plane because it was clicked there.
    assert_eq!(drag(&mut tool, &cx, centre, centre), Press::Drag);
    assert_eq!(tool.rig.alive_count(), 1);
    let root = tool.selected.expect("the new sphere is selected");
    assert_eq!(tool.rig.nodes()[root].position, Vec3::ZERO);
    let radius = tool.rig.nodes()[root].radius;

    // Dragging off it pulls a new sphere out, which rests on its parent
    // until the cursor leaves it and then follows the cursor.
    assert_eq!(drag(&mut tool, &cx, centre, centre + Vec2::new(200.0, 0.0)), Press::Drag);
    assert_eq!(tool.rig.alive_count(), 2);
    let child = tool.selected.unwrap();
    assert_ne!(child, root);
    let node = &tool.rig.nodes()[child];
    assert_eq!(node.parent, Some(root));
    assert_eq!(node.radius, radius, "new spheres match the last one");
    assert!(node.position.x > 2.0 * radius, "it left its parent: {}", node.position);
    assert!(view.project(node.position).unwrap().distance(centre + Vec2::new(200.0, 0.0)) < 1.0);
    // Off the plane it has a reflection; the root on the plane does not.
    assert_eq!(tool.rig.geometry(tool.symmetry()).0.len(), 3);

    // A click on empty space extends the chain from the selection.
    let beside = centre + Vec2::new(380.0, -60.0);
    assert_eq!(tool.press(&cx, beside, false, false), Press::Empty);
    tool.release(&cx);
    assert_eq!(tool.rig.alive_count(), 3);
    assert_eq!(tool.rig.parent_of(tool.selected.unwrap()), Some(child));

    // The same press moved away is the camera's drag, and places nothing.
    assert_eq!(tool.press(&cx, beside + Vec2::new(0.0, 200.0), false, false), Press::Empty);
    assert!(!tool.drag_to(&cx, beside + Vec2::new(40.0, 240.0)), "the camera has the drag");
    tool.release(&cx);
    assert_eq!(tool.rig.alive_count(), 3);

    // Grabbing the reflection moves the real sphere, mirrored.
    tool.set_mode(EditMode::Move);
    let before = tool.rig.nodes()[child].position;
    let on_twin = view.project(Vec3::new(-before.x, before.y, before.z)).unwrap();
    assert_eq!(drag(&mut tool, &cx, on_twin, on_twin + Vec2::new(-50.0, 0.0)), Press::Drag);
    let after = tool.rig.nodes()[child].position;
    assert!(after.x > before.x + 0.05, "dragging the twin left moves the sphere right: {before} -> {after}");

    // Right-click deletes a sphere and everything below it.
    let on_child = view.project(after).unwrap();
    assert_eq!(tool.press(&cx, on_child, false, true), Press::Done);
    assert_eq!(tool.rig.alive_count(), 1);

    // Every gesture is one undo step.
    for alive in [3, 3, 2, 1, 0] {
        assert!(tool.undo());
        assert_eq!(tool.rig.alive_count(), alive);
    }
    assert!(!tool.undo(), "nothing left");
    assert!(tool.redo());
    assert_eq!(tool.rig.alive_count(), 1);
}

#[test]
fn escape_puts_a_drag_back_and_the_first_sphere_rests_on_the_model() {
    let view = front_view();
    // A wall facing the camera at z = 0.5.
    let wall = |origin: Vec3, dir: Vec3| {
        let t = (0.5 - origin.z) / dir.z;
        (t > 0.0).then(|| Hit { point: origin + dir * t, normal: Vec3::Z, body: plain_body(1) })
    };
    let cx = Ctx { view: &view, pivot: Vec3::ZERO, surface: &wall, body: None };
    let mut tool = SSpheres::default();
    let centre = SIZE * 0.5;

    drag(&mut tool, &cx, centre, centre);
    let node = &tool.rig.nodes()[tool.selected.unwrap()];
    assert!((node.position.z - (0.5 + node.radius)).abs() < 1e-4, "touching the wall, not in it: {}", node.position);
    let radius = node.radius;

    tool.set_mode(EditMode::Scale);
    assert_eq!(tool.press(&cx, centre, false, false), Press::Drag);
    tool.drag_to(&cx, centre + Vec2::new(120.0, 0.0));
    assert!(tool.rig.nodes()[0].radius > radius * 1.5);
    assert!(tool.escape());
    assert_eq!(tool.rig.nodes()[0].radius, radius, "Esc restores the press-time rig");
    tool.release(&cx);
    assert!(tool.undo() && !tool.undo(), "the cancelled drag left no undo step");
}

#[test]
fn symmetry_toggle_never_loses_or_doubles_spheres() {
    let mut tool = SSpheres::default();
    tool.rig = figure();
    let shown = |tool: &SSpheres| tool.rig.geometry(tool.symmetry()).0.len();
    let before = shown(&tool);
    tool.set_symmetry(false);
    assert_eq!(shown(&tool), before, "the mirrored half became real spheres");
    assert_eq!(tool.rig.alive_count(), before);
    tool.set_symmetry(true);
    assert_eq!(shown(&tool), before);
    assert_eq!(tool.rig.alive_count(), 14);
    assert!(tool.undo() && tool.undo());
    assert!(tool.symmetry() && tool.rig.alive_count() == 14);
}

#[test]
fn make_skin_keeps_or_clears_the_spheres() {
    let mut tool = SSpheres::default();
    tool.rig = figure();
    let made = tool.make_skin().unwrap();
    assert!(made.skin.stats.triangles > 10_000);
    assert_eq!(tool.rig.alive_count(), 14, "kept by default, to change and skin again");

    tool.keep_rig = false;
    tool.make_skin().unwrap();
    assert!(tool.rig.is_empty());
    assert!(tool.undo(), "and Undo brings them back");
    assert_eq!(tool.rig.alive_count(), 14);
    assert!(tool.make_skin().is_ok());
}

#[test]
fn make_skin_puts_the_pivot_in_the_middle_of_the_skin() {
    let mut tool = figure_tool();
    let rig_space = skin_of(&tool.rig, true, Quality::Final);
    let middle = |positions: &[Vec3]| {
        let lo = positions.iter().fold(Vec3::MAX, |a, &p| a.min(p));
        let hi = positions.iter().fold(Vec3::MIN, |a, &p| a.max(p));
        (lo + hi) * 0.5
    };

    let made = tool.make_skin().unwrap();
    assert!(middle(&made.skin.positions).length() < 1e-4, "the skin is measured from its own middle");
    // Under symmetry the pivot stays on the mirror plane, so the new object
    // mirrors across the plane the rig was built about.
    assert_eq!(made.position.x, 0.0);
    assert!(made.position.distance(middle(&rig_space.positions)) < 1e-4, "pivot at {}", made.position);
    assert!(made.position.y > 0.9 && made.position.y < 1.1, "the figure's middle, not its feet");
    assert_eq!((made.rotation, made.scale), (Quat::IDENTITY, Vec3::ONE));
    assert_mirror_exact(&made.skin.positions, "skin about its pivot");
    // No vertex moved in the world.
    for (p, q) in made.skin.positions.iter().zip(&rig_space.positions) {
        assert!((made.position + *p).distance(*q) < 1e-5);
    }

    // Attached to an object that is then moved, turned and made twice the
    // size, the skin object takes the rig's place, turn and size, and still
    // lies exactly over the spheres.
    let was = plain_body(7);
    tool.attach(was);
    let now = Body {
        position: Vec3::new(1.0, -0.4, 0.3),
        rotation: Quat::from_rotation_y(0.8) * Quat::from_rotation_x(-0.3),
        scale: Vec3::splat(2.0),
        ..was
    };
    tool.follow(|_| Some(now));
    let frame = tool.frame();
    let made = tool.make_skin().unwrap();
    // A rig seated on its object takes the object's turn exactly.
    assert_eq!(made.rotation, now.rotation);
    assert!((made.scale - Vec3::splat(2.0)).length() < 1e-4);
    let model = Mat4::from_scale_rotation_translation(made.scale, made.rotation, made.position);
    for (p, q) in made.skin.positions.iter().zip(&rig_space.positions) {
        assert!(model.transform_point3(*p).distance(frame.to_world(*q)) < 1e-4);
    }
}

#[test]
fn attaching_moves_the_mirror_plane_but_no_sphere() {
    let mut tool = figure_tool();
    let before = world_nodes(&tool);
    let reflections_before = world_spheres(&tool);
    let body = placed_body(7);

    assert!(tool.attach(body), "the mirror plane moved onto the object");
    assert_eq!(tool.attached_to(), Some(7));
    assert_same_points(&before, &world_nodes(&tool), "attach");
    // The rig keeps its own size however the object is stretched.
    assert_eq!(tool.frame().scale, 1.0);
    for (node, original) in tool.rig.nodes().iter().zip(figure().nodes()) {
        assert_eq!(node.radius, original.radius);
    }

    // The reflections are now in the object's plane, not the world's.
    assert_mirrors_across(&tool, &body);
    let (_, _, twin, _) = *world_spheres(&tool).iter().find(|s| s.1).unwrap();
    let nearest = reflections_before.iter().map(|s| s.2.distance(twin)).fold(f32::MAX, f32::min);
    assert!(nearest > 0.1, "a reflection is still where the world's plane put it");
}

#[test]
fn attach_then_detach_leaves_everything_where_it_was() {
    let mut tool = figure_tool();
    let before = world_nodes(&tool);
    let body = placed_body(7);

    tool.attach(body);
    assert!(tool.detach());
    assert_eq!(tool.attached_to(), None);
    assert!(!tool.detach(), "nothing left to let go of");
    assert_same_points(&before, &world_nodes(&tool), "attach then detach");
    // The mirror plane stays where attaching put it.
    assert_mirrors_across(&tool, &body);

    // Let go of, the rig no longer goes with the object.
    let frame = tool.frame();
    tool.follow(|_| Some(Body { position: Vec3::ONE, ..body }));
    assert_eq!(tool.frame(), frame);

    // Both were steps in the rig's history, and stepping back over them
    // puts the rig back exactly, plane and all.
    assert!(tool.undo() && tool.undo());
    assert_eq!(tool.frame(), Frame::IDENTITY);
    for (node, original) in tool.rig.nodes().iter().zip(figure().nodes()) {
        assert_eq!(node.position, original.position);
    }
    assert!(!tool.undo(), "attach and detach were one step each");
}

#[test]
fn attach_is_one_undo_step_and_redo_brings_it_back() {
    let mut tool = figure_tool();
    let before = world_nodes(&tool);
    let body = placed_body(7);

    tool.attach(body);
    let seated = tool.frame();
    assert_eq!(tool.undo_summary(), Some(("Attach Spheres", 1)));

    assert!(tool.undo());
    assert_eq!((tool.frame(), tool.attached_to()), (Frame::IDENTITY, None));
    for (node, original) in tool.rig.nodes().iter().zip(figure().nodes()) {
        assert_eq!(node.position, original.position, "exactly as it was, not merely close");
    }
    assert!(!tool.can_undo() && tool.can_redo());

    assert!(tool.redo());
    assert_eq!((tool.frame(), tool.attached_to()), (seated, Some(7)));
    assert_same_points(&before, &world_nodes(&tool), "redo of attach");
    assert_mirrors_across(&tool, &body);

    // Attaching again to the same object changes nothing, so it is no step.
    assert!(!tool.attach(body));
    assert_eq!(tool.undo_summary(), Some(("Attach Spheres", 1)));

    // An empty rig can be attached too, ready for its first sphere, and that
    // is a step as well.
    let mut empty = SSpheres::default();
    empty.attach(body);
    assert_eq!((empty.frame(), empty.attached_to()), (Frame::IDENTITY.seated_on(&body), Some(7)));
    assert!(empty.undo());
    assert_eq!(empty.attached_to(), None);
}

#[test]
fn an_attached_rig_goes_with_its_object() {
    let mut tool = figure_tool();
    let was = placed_body(7);
    tool.attach(was);
    // Where every sphere sits as the object sees it, leaving its size out.
    let seen_from = |tool: &SSpheres, body: &Body| -> Vec<Vec3> {
        world_spheres(tool).iter().map(|s| body.rotation.inverse() * (s.2 - body.position)).collect()
    };
    let before = seen_from(&tool, &was);
    let radii: Vec<f32> = world_spheres(&tool).iter().map(|s| s.3).collect();
    let history = tool.undo_summary();

    // Moved, turned and made half as big again, evenly.
    let now = Body {
        position: was.position + Vec3::new(1.0, 2.0, -0.5),
        rotation: Quat::from_rotation_x(1.1) * was.rotation,
        scale: was.scale * 1.5,
        ..was
    };
    tool.follow(|id| (id == 7).then_some(now));
    let grown: Vec<Vec3> = before.iter().map(|&p| p * 1.5).collect();
    assert_same_points(&grown, &seen_from(&tool, &now), "following");
    for (s, r) in world_spheres(&tool).iter().zip(&radii) {
        assert!((s.3 - r * 1.5).abs() < 1e-5, "spheres grow with the object");
    }
    assert_mirrors_across(&tool, &now);
    assert_eq!(tool.undo_summary(), history, "following is not an edit");

    // Spheres cannot stretch, so a stretch along one axis is not followed.
    let frame = tool.frame();
    let stretched = Body { scale: now.scale * Vec3::new(1.0, 2.0, 1.0), ..now };
    tool.follow(|_| Some(stretched));
    assert_eq!(tool.frame(), frame);

    // The object is deleted: the rig lets go and stays put, and says so.
    tool.follow(|_| None);
    assert_eq!((tool.frame(), tool.attached_to()), (frame, None));
    assert!(tool.note().is_some());
}

#[test]
fn a_frame_beside_its_object_is_carried_as_if_fixed_to_it() {
    // Not seated on the object: off to one side of it and turned its own way.
    let frame = Frame { position: Vec3::new(1.0, 2.0, 3.0), rotation: Quat::from_rotation_x(0.3), scale: 1.2 };
    let was = placed_body(1);
    let now = Body {
        position: Vec3::new(-0.5, 0.2, 2.0),
        rotation: Quat::from_rotation_z(-1.3) * was.rotation,
        scale: was.scale * 0.5,
        ..was
    };
    let carried = frame.carried(&was, &now);
    // The object was halved, so everything sits at half the distance from it.
    for p in [Vec3::ZERO, Vec3::X, Vec3::new(-0.3, 0.8, 0.1)] {
        let before = was.rotation.inverse() * (frame.to_world(p) - was.position);
        let after = now.rotation.inverse() * (carried.to_world(p) - now.position);
        assert!((before * 0.5).distance(after) < 1e-5, "{before} -> {after}");
        assert!(carried.to_rig(carried.to_world(p)).distance(p) < 1e-5, "there and back");
    }
    assert!((carried.scale - 0.6).abs() < 1e-6);
}

#[test]
fn undo_after_the_object_moved_lands_the_rig_beside_it() {
    let mut tool = figure_tool();
    let was = placed_body(7);
    tool.attach(was);
    let seen_from = |tool: &SSpheres, body: &Body| -> Vec<Vec3> {
        world_nodes(tool).iter().map(|&p| body.rotation.inverse() * (p - body.position)).collect()
    };
    let before = seen_from(&tool, &was);

    // An edit, then the object is moved and turned, then the edit is undone.
    // The step holds the rig's place from before the move, which must not
    // be where the rig goes back to.
    tool.delete_node(4);
    let now = Body {
        position: Vec3::new(-2.0, 0.5, 1.0),
        rotation: Quat::from_rotation_y(2.0) * was.rotation,
        ..was
    };
    tool.follow(|_| Some(now));
    assert!(tool.undo());
    assert_eq!(tool.rig.alive_count(), 14);
    tool.follow(|_| Some(now));
    assert_same_points(&before, &seen_from(&tool, &now), "undo after the object moved");
}

#[test]
fn gestures_follow_the_cursor_on_a_rig_standing_on_a_turned_object() {
    let view = front_view();
    let home = Body {
        id: 3,
        position: Vec3::new(0.3, 0.2, -0.1),
        rotation: Quat::from_rotation_z(0.6) * Quat::from_rotation_y(0.5),
        scale: Vec3::new(1.0, 1.3, 0.7),
    };
    let cx = Ctx { view: &view, pivot: Vec3::ZERO, surface: &no_surface, body: Some(home) };
    let mut tool = SSpheres::default();
    tool.set_next_radius(0.1);
    let world = |tool: &SSpheres, node: usize| tool.frame().to_world(tool.rig.nodes()[node].position);
    let on_screen = |p: Vec3| view.project(p).unwrap();

    // Placed in the open, the first sphere lands under the cursor, where the
    // camera is looking, and the rig stands on the selected object (sharing
    // its mirror plane) without being tied to it.
    let at = SIZE * 0.5 + Vec2::new(-120.0, 40.0);
    assert_eq!(drag(&mut tool, &cx, at, at), Press::Drag);
    let root = tool.selected.unwrap();
    assert_eq!(tool.frame(), Frame::IDENTITY.seated_on(&home));
    assert_eq!(tool.attached_to(), None);
    assert!(on_screen(world(&tool, root)).distance(at) < 1.0);
    assert!((view.depth_of(world(&tool, root)) - view.depth_of(Vec3::ZERO)).abs() < 1e-4);

    // A sphere pulled out of it follows the cursor.
    let to = at + Vec2::new(220.0, -30.0);
    assert_eq!(drag(&mut tool, &cx, at, to), Press::Drag);
    let child = tool.selected.unwrap();
    assert_eq!(tool.rig.parent_of(child), Some(root));
    assert!(on_screen(world(&tool, child)).distance(to) < 1.0, "{}", on_screen(world(&tool, child)));
    assert_mirrors_across(&tool, &home);

    // Its reflection can be grabbed where it is drawn, and stays under the
    // cursor while the real sphere moves on the other side of the plane.
    let twin = |tool: &SSpheres| world_spheres(tool).iter().find(|s| s.0 == child && s.1).unwrap().2;
    tool.set_mode(EditMode::Move);
    let grab = on_screen(twin(&tool));
    let moved_from = world(&tool, child);
    assert_eq!(drag(&mut tool, &cx, grab, grab + Vec2::new(0.0, -60.0)), Press::Drag);
    assert_eq!(tool.selected, Some(child));
    assert!(on_screen(twin(&tool)).distance(grab + Vec2::new(0.0, -60.0)) < 1.0);
    assert!(world(&tool, child).distance(moved_from) > 0.05);
    assert_mirrors_across(&tool, &home);

    // The wheel and Scale mode never look at the frame; a sphere is still
    // picked where it is drawn.
    tool.hover(&cx, on_screen(world(&tool, root)), false);
    assert_eq!(tool.hovered, Some(root));
}

#[test]
fn a_first_sphere_placed_on_a_model_attaches_to_it() {
    let view = front_view();
    let model = Body { id: 9, position: Vec3::new(0.2, 0.1, 0.0), rotation: Quat::from_rotation_z(0.4), scale: Vec3::ONE };
    // A wall facing the camera at z = 0.5 that belongs to the model.
    let wall = |origin: Vec3, dir: Vec3| {
        let t = (0.5 - origin.z) / dir.z;
        (t > 0.0).then(|| Hit { point: origin + dir * t, normal: Vec3::Z, body: model })
    };
    let cx = Ctx { view: &view, pivot: Vec3::ZERO, surface: &wall, body: Some(plain_body(2)) };
    let mut tool = SSpheres::default();
    tool.set_next_radius(0.1);
    let at = SIZE * 0.5 + Vec2::new(150.0, -80.0);

    drag(&mut tool, &cx, at, at);
    assert_eq!(tool.attached_to(), Some(9), "tied to the model it rests on, not the selected object");
    assert_eq!(tool.frame(), Frame::IDENTITY.seated_on(&model));
    let node = &tool.rig.nodes()[tool.selected.unwrap()];
    let centre = tool.frame().to_world(node.position);
    assert!((centre.z - (0.5 + node.radius)).abs() < 1e-4, "touching the wall, not in it: {centre}");
    assert!(view.project(centre - Vec3::Z * node.radius).unwrap().distance(at) < 1.0, "where it was clicked");

    // It goes with the model from then on.
    let moved = Body { position: model.position + Vec3::new(0.0, 1.0, 0.0), ..model };
    tool.follow(|_| Some(moved));
    let carried = tool.frame().to_world(tool.rig.nodes()[0].position);
    assert!(carried.distance(centre + Vec3::new(0.0, 1.0, 0.0)) < 1e-4);

    // Undo takes the sphere and the attachment away together.
    assert!(tool.undo());
    assert!(tool.rig.is_empty() && tool.attached_to().is_none());

    // With resting on the model switched off, it is placed in the open and
    // stands on the selected object instead.
    tool.snap_to_surface = false;
    drag(&mut tool, &cx, at, at);
    assert_eq!((tool.frame(), tool.attached_to()), (Frame::IDENTITY, None));
}

#[test]
fn new_rig_drops_one_sphere_in_view_and_never_clears_a_rig() {
    let view = front_view();
    // The camera is looking at the selected object, which has been moved,
    // turned and stretched.
    let home = placed_body(3);
    let cx = Ctx { view: &view, pivot: home.position, surface: &no_surface, body: Some(home) };
    let mut tool = SSpheres::default();
    tool.set_next_radius(0.1);
    tool.set_mode(EditMode::Scale);
    let world = |tool: &SSpheres, node: usize| tool.frame().to_world(tool.rig.nodes()[node].position);

    assert!(tool.start_new_rig(&cx));
    assert_eq!(tool.mode(), EditMode::Draw, "ready to pull the next sphere out");
    assert_eq!(tool.rig.alive_count(), 1);
    let root = tool.selected.expect("the new sphere is selected");
    assert_eq!(tool.frame(), Frame::IDENTITY.seated_on(&home), "standing on the selected object");
    assert_eq!(tool.attached_to(), None);
    assert!(world(&tool, root).distance(cx.pivot) < 1e-4, "where the camera looks");
    assert_eq!(tool.rig.geometry(true).0.len(), 1, "on the mirror plane: one sphere, not a pair");
    assert!(!tool.is_dragging(), "a button leaves no drag behind");

    assert!(!tool.start_new_rig(&cx), "a rig is already up");
    assert_eq!(tool.rig.alive_count(), 1);
    assert_eq!(tool.undo_summary(), Some(("New SSphere Rig", 1)));
    assert!(tool.undo());
    assert!(tool.rig.is_empty());

    // Looking somewhere off the object's mirror plane, the sphere still goes
    // on the plane, level with where the camera is looking.
    let aside = Ctx { pivot: Vec3::new(-0.4, 0.5, 0.0), ..cx };
    assert!(tool.start_new_rig(&aside));
    let root = tool.selected.unwrap();
    assert_eq!(tool.rig.nodes()[root].position.x, 0.0);
    assert!((view.depth_of(world(&tool, root)) - view.depth_of(aside.pivot)).abs() < 1e-4);
    assert!(view.contains(view.project(world(&tool, root)).unwrap()), "and in view");
    assert!(tool.undo());

    // With the selected object's surface between the camera and where it is
    // looking, the sphere rests on that surface, where it can be seen, and
    // attaches to the object as a first click on it would.
    let upright = Body { id: 9, position: Vec3::new(0.2, 0.1, 0.0), rotation: Quat::IDENTITY, scale: Vec3::ONE };
    let wall = |origin: Vec3, dir: Vec3| {
        let t = (0.5 - origin.z) / dir.z;
        (t > 0.0).then(|| Hit { point: origin + dir * t, normal: Vec3::Z, body: upright })
    };
    let behind_wall = Ctx { surface: &wall, pivot: Vec3::new(0.3, 0.2, 0.0), body: Some(upright), ..cx };
    assert!(tool.start_new_rig(&behind_wall));
    let node = &tool.rig.nodes()[tool.selected.unwrap()];
    let centre = tool.frame().to_world(node.position);
    assert_eq!(node.position.x, 0.0, "on the object's mirror plane");
    assert!((centre.z - (0.5 + node.radius)).abs() < 1e-4, "on the wall, not behind it: {centre}");
    assert!(view.contains(view.project(centre).unwrap()));
    assert_eq!(tool.attached_to(), Some(9));
}
