//! Each object's own symmetry: what an object starts with, what a stroke is
//! made under, what undo leaves alone and what a file keeps.

use std::collections::HashMap;

use glam::{EulerRot, Quat, Vec2};

use super::*;
use crate::camera::{Camera, View};
use crate::history::History;
use crate::io::{self, SavedObject};
use crate::mesh::primitives::{self, Primitive};
use crate::mesh::{Mesh, StrokeDelta};
use crate::scene::Others;
use crate::sculpt::{Brush, PointerSample, Sculptor};

const SIZE: Vec2 = Vec2::new(1600.0, 900.0);

fn ball(tris: usize) -> Mesh {
    primitives::build(Primitive::Sphere, tris, 0.5)
}

fn mirror(x: bool, y: bool, z: bool) -> Symmetry {
    Symmetry { mirror: [x, y, z], ..Symmetry::default() }
}

fn radial(count: u32, axis: RadialAxis) -> Symmetry {
    Symmetry { radial: Radial { on: true, count, axis, ..Radial::default() }, ..Symmetry::default() }
}

/// The camera on everything that shows, from the front and a little above.
fn view_of(scene: &Scene, mesh: &Mesh) -> View {
    let mut camera = Camera::default();
    camera.frame(scene.bounds(mesh), SIZE.x / SIZE.y, false);
    (camera.yaw, camera.pitch) = (0.0, 10.0);
    camera.view(Vec2::ZERO, SIZE, camera.distance, 1.0)
}

/// One stroke on the active object the way the app makes one: the scene
/// hands its symmetry to the sculptor, and the pointer is dragged over the
/// object between two places given in the object's own space.
fn stroke(scene: &Scene, mesh: &mut Mesh, sculptor: &mut Sculptor, brush: Brush, from: Vec3, to: Vec3) -> Option<StrokeDelta> {
    let view = view_of(scene, mesh);
    let model = scene.active().matrix();
    let local = view.local_to(scene.brush_matrix());
    let px = |at: Vec3| view.project(model.transform_point3(at)).expect("the stroke is in front of the camera");
    let (a, b) = (px(from), px(to));
    let settings = &mut sculptor.settings;
    (settings.brush, settings.mask_mode, settings.screen_space_size, settings.radius_world) = (brush, false, false, 0.08);
    settings.per_brush[brush.index()].strength = 0.5;
    scene.hand_symmetry_to(&mut sculptor.settings);
    let steps = 32;
    let sample = |i: usize| PointerSample { pos: a.lerp(b, i as f32 / steps as f32), pressure: None, time: i as f64 / 240.0 };
    assert!(sculptor.begin(mesh, &local, sample(0), false, false), "the stroke missed the object");
    for i in 1..=steps {
        sculptor.sample(mesh, &local, sample(i));
        if i % 4 == 0 {
            sculptor.frame(mesh, &local, 1.0 / 60.0);
            let _ = mesh.take_dirty();
        }
    }
    let delta = sculptor.end(mesh, &local);
    let _ = mesh.take_dirty();
    delta
}

/// For each vertex, the vertex at its mirror image across the plane facing
/// `axis`. The ball is built mirror-exact, so that is a match to the bit.
fn twins(positions: &[Vec3], axis: usize) -> Vec<u32> {
    let key = |p: Vec3| [(p.x + 0.0).to_bits(), (p.y + 0.0).to_bits(), (p.z + 0.0).to_bits()];
    let map: HashMap<[u32; 3], u32> = positions.iter().enumerate().map(|(i, &p)| (key(p), i as u32)).collect();
    let mut sign = Vec3::ONE;
    sign[axis] = -1.0;
    positions.iter().map(|&p| *map.get(&key(p * sign)).expect("the ball is mirror-exact")).collect()
}

/// How a sculpted ball stands to the plane facing `axis`: whether every
/// vertex is still the exact mirror image of its twin, and how many vertices
/// moved without their twin moving at all.
fn across(rest: &[Vec3], now: &[Vec3], axis: usize) -> (bool, usize) {
    let twin = twins(rest, axis);
    let mut sign = Vec3::ONE;
    sign[axis] = -1.0;
    let exact = (0..now.len()).all(|v| now[twin[v] as usize] == now[v] * sign);
    let alone = (0..now.len()).filter(|&v| now[v] != rest[v] && now[twin[v] as usize] == rest[twin[v] as usize]).count();
    (exact, alone)
}

/// A stroke clear of every plane of the ball: up and to the right of its
/// middle, on the side facing the camera.
const OFF_TO_ONE_SIDE: (Vec3, Vec3) = (Vec3::new(0.2, 0.22, 0.4), Vec3::new(0.3, 0.06, 0.38));

// ---------------------------------------------------- what an object starts with

#[test]
fn a_new_object_starts_with_no_symmetry_whatever_the_selected_one_has() {
    let mut mesh = ball(2_000);
    let mut scene = Scene::new("Sphere");
    let first = scene.active_id();
    assert_eq!(scene.symmetry(first), Symmetry::default());
    assert!(scene.symmetry(first).is_off() && scene.symmetry(first).space == Space::Local);
    assert_eq!(scene.symmetry(first).radial, Radial::default(), "eight round Y, switched off, as in the Unity app");
    scene.set_symmetry(first, Symmetry { space: Space::World, ..mirror(true, false, true) });

    // A new shape, an imported model, a skin, an extract: all arrive through `add`.
    let second = scene.add("Cube", ball(2_000), &mut mesh);
    assert_eq!(scene.symmetry(second), Symmetry::default());
    assert_eq!(scene.active_symmetry(), Symmetry::default(), "and it is the selected one now");
    assert_eq!(scene.symmetry(first).mirror, [true, false, true], "the first keeps its own");
    // An object that is not there has none, and setting one does nothing.
    scene.set_symmetry(999, mirror(true, true, true));
    assert_eq!(scene.symmetry(999), Symmetry::default());
}

#[test]
fn a_clone_starts_with_its_sources_symmetry_and_is_then_its_own_object() {
    let mut mesh = ball(2_000);
    let mut scene = Scene::new("Sphere");
    let source = scene.active_id();
    let own = Symmetry { space: Space::World, ..radial(5, RadialAxis::Z) };
    scene.set_symmetry(source, Symmetry { mirror: [true, false, false], ..own });
    let copy = scene.clone_active(&mut mesh);
    assert_eq!(scene.symmetry(copy), scene.symmetry(source));
    // Independent from then on: it has a mesh of its own.
    scene.set_symmetry(copy, mirror(false, true, false));
    assert_eq!(scene.symmetry(source).mirror, [true, false, false]);
    assert_eq!(scene.symmetry(copy), mirror(false, true, false));
}

/// In the Unity app an instance had no settings of its own: it was a second
/// placement of its source, sculpted under the source's symmetry. The objects
/// that share a mesh here are equals, so they share the setting.
#[test]
fn instances_share_one_symmetry_until_one_is_made_unique() {
    let mut mesh = ball(2_000);
    let mut scene = Scene::new("Sphere");
    let source = scene.active_id();
    scene.set_symmetry(source, mirror(true, false, false));
    let twin = scene.instance_active();
    assert_eq!(scene.symmetry(twin), mirror(true, false, false), "an instance starts with its source's");

    // Changed with either selected, it changes for both.
    scene.set_symmetry(twin, radial(6, RadialAxis::Y));
    assert_eq!(scene.symmetry(source), radial(6, RadialAxis::Y));
    scene.select(source, &mut mesh);
    scene.edit_symmetry(source, |symmetry| symmetry.mirror[2] = true);
    assert_eq!(scene.symmetry(twin).mirror, [false, false, true]);
    assert_eq!(scene.symmetry(twin), scene.symmetry(source));

    // Made unique, it keeps what it had and goes its own way.
    scene.select(twin, &mut mesh);
    assert!(scene.make_unique(&mut mesh));
    assert_eq!(scene.symmetry(twin), scene.symmetry(source), "as in the Unity app, the copy takes the settings along");
    scene.set_symmetry(twin, Symmetry::default());
    assert!(scene.symmetry(twin).is_off() && !scene.symmetry(source).is_off());
}

/// A live mirror made into real objects: each is the object reflected, so the
/// same axes and the same X, Y or Z radial axis describe the same symmetry,
/// and a custom axis, being a direction, is reflected with the geometry
/// (the Unity app's `CopySettingsFrom`).
#[test]
fn objects_made_from_live_mirror_copies_take_the_symmetry_reflected() {
    let mut mesh = ball(2_000);
    let mut scene = Scene::new("Sphere");
    let id = scene.active_id();
    scene.active_mut().position = Vec3::new(0.8, 0.0, 0.3);
    let custom = Vec3::new(0.6, 0.0, 0.8);
    let own = Symmetry {
        mirror: [true, false, true],
        radial: Radial { on: true, count: 7, axis: RadialAxis::Custom, custom_axis: custom },
        space: Space::Local,
    };
    scene.set_symmetry(id, own);
    assert!(scene.set_mirror(id, 1 | 4, Vec3::ZERO));
    let made = scene.bake_mirror(id, &mut mesh).expect("it is mirrored");
    assert_eq!(made.len(), 3, "X, Z and XZ");
    assert_eq!(scene.symmetry(id), own, "the object keeps its own");
    let signs = [Vec3::new(-1.0, 1.0, 1.0), Vec3::new(1.0, 1.0, -1.0), Vec3::new(-1.0, 1.0, -1.0)];
    for (&copy, sign) in made.iter().zip(signs) {
        let got = scene.symmetry(copy);
        assert_eq!((got.mirror, got.radial.on, got.radial.count, got.space), ([true, false, true], true, 7, Space::Local));
        assert_eq!(got.radial.custom_axis, custom * sign, "{}", scene.get(copy).unwrap().name);
    }
    // An axis that is one of the object's own is the same line either way.
    scene.set_symmetry(id, radial(4, RadialAxis::X));
    assert_eq!(scene.symmetry(id).reflected(Vec3::new(-1.0, 1.0, 1.0)), radial(4, RadialAxis::X));
}

/// The object a join or a boolean leaves is the one it started with, so it
/// keeps its own symmetry; the others' go with them, and come back with them.
#[test]
fn what_a_join_or_a_boolean_leaves_keeps_its_own_symmetry() {
    for others in [Others::Deleted, Others::Hidden] {
        let mut mesh = ball(2_000);
        let mut scene = Scene::new("Body");
        let mut history = History::default();
        let body = scene.active_id();
        scene.set_symmetry(body, mirror(false, false, true));
        let arm = scene.add("Arm", ball(2_000), &mut mesh);
        scene.set_symmetry(arm, radial(3, RadialAxis::X));
        scene.select(body, &mut mesh);
        scene.select_also(arm);

        let before = scene.state();
        let joined = scene.joined_mesh(&mesh).expect("two are selected");
        scene.install_combined(joined, others, &mut mesh);
        history.push_scene(before);
        assert_eq!(scene.active_id(), body);
        assert_eq!(scene.symmetry(body), mirror(false, false, true), "{others:?}");

        // Its symmetry is changed, and then the join is undone: the change stays
        // (no undo step holds a symmetry), and the arm is back with its own.
        scene.set_symmetry(body, mirror(true, false, true));
        assert!(history.step_scene(true, &mut scene, &mut mesh));
        assert_eq!(scene.symmetry(body), mirror(true, false, true));
        assert_eq!(scene.symmetry(arm), radial(3, RadialAxis::X));
    }
}

// ------------------------------------------------------------ undo

/// Ticking an axis is not an undo step, and no undo step puts an older
/// symmetry back: as in the Unity app, where the settings were never on the
/// history. An object that undo brings back has its settings as it left them.
#[test]
fn undo_never_changes_an_objects_symmetry() {
    let mut mesh = ball(2_000);
    let mut scene = Scene::new("Sphere");
    let mut history = History::default();
    let first = scene.active_id();

    // A rename, then a change of symmetry, then the rename undone and redone.
    let before = scene.state();
    scene.active_mut().name = "Head".into();
    history.push_scene(before);
    let steps = history.steps();
    scene.set_symmetry(first, mirror(true, false, false));
    assert_eq!(history.steps(), steps, "a change of symmetry records nothing");
    assert!(history.step_scene(true, &mut scene, &mut mesh));
    assert_eq!((scene.active().name.as_str(), scene.symmetry(first)), ("Sphere", mirror(true, false, false)));
    assert!(history.step_scene(false, &mut scene, &mut mesh));
    assert_eq!(scene.symmetry(first), mirror(true, false, false));

    // A second object is made, given a symmetry, deleted, and brought back.
    let before = scene.state();
    let second = scene.add("Cube", ball(2_000), &mut mesh);
    history.push_scene(before);
    scene.set_symmetry(second, radial(12, RadialAxis::Z));
    let before = scene.state();
    assert!(scene.remove(second, &mut mesh));
    history.push_scene(before);
    scene.collect(|id| history.keeps_mesh(id));
    assert_eq!(scene.active_symmetry(), mirror(true, false, false), "the first is selected again, with its own");
    assert!(history.step_scene(true, &mut scene, &mut mesh));
    assert_eq!(scene.active_id(), second);
    assert_eq!(scene.active_symmetry(), radial(12, RadialAxis::Z));
}

// ------------------------------------------------------------ strokes

#[test]
fn two_objects_each_sculpt_under_their_own_symmetry() {
    let mut mesh = ball(20_000);
    let rest = mesh.positions.clone();
    let mut scene = Scene::new("Face");
    let mut sculptor = Sculptor::default();
    let face = scene.active_id();
    let body = scene.add("Body", ball(20_000), &mut mesh);
    scene.get_mut(body).unwrap().position = Vec3::new(1.6, 0.0, 0.0);
    // As in the figure this was done for: the face across X, the body across Z.
    scene.set_symmetry(face, mirror(true, false, false));
    scene.set_symmetry(body, mirror(false, false, true));
    let (from, to) = OFF_TO_ONE_SIDE;

    // The body is the selected one: a stroke on it goes across its Z plane only.
    assert_eq!(scene.active_id(), body);
    assert!(stroke(&scene, &mut mesh, &mut sculptor, Brush::Inflate, from, to).is_some());
    let (exact_z, alone_z) = across(&rest, &mesh.positions, 2);
    let (exact_x, alone_x) = across(&rest, &mesh.positions, 0);
    assert!(exact_z && alone_z == 0, "the body's stroke is mirrored across Z, to the bit");
    assert!(!exact_x && alone_x > 50, "and not across X: {alone_x} vertices moved alone");
    let body_after = mesh.positions.clone();

    // The face is selected: the same stroke goes across its X plane only.
    scene.select(face, &mut mesh);
    assert_eq!(mesh.positions, rest, "the face's own mesh is in hand, untouched");
    assert!(stroke(&scene, &mut mesh, &mut sculptor, Brush::Inflate, from, to).is_some());
    let (exact_x, alone_x) = across(&rest, &mesh.positions, 0);
    let (exact_z, alone_z) = across(&rest, &mesh.positions, 2);
    assert!(exact_x && alone_x == 0, "the face's stroke is mirrored across X, to the bit");
    assert!(!exact_z && alone_z > 50, "and not across Z: {alone_z} vertices moved alone");

    // And back: nothing of the face's symmetry came along.
    scene.select(body, &mut mesh);
    assert_eq!(mesh.positions, body_after);
    assert!(stroke(&scene, &mut mesh, &mut sculptor, Brush::Inflate, from * Vec3::new(1.0, -1.0, 1.0), to * Vec3::new(1.0, -1.0, 1.0)).is_some());
    let (exact_z, _) = across(&rest, &mesh.positions, 2);
    let (exact_x, alone_x) = across(&rest, &mesh.positions, 0);
    assert!(exact_z && !exact_x && alone_x > 100, "still across Z alone");
}

/// However the selection changes (a click on a row, a new object, an undo
/// that brings an object back), the next stroke is under the symmetry of the
/// object it lands on and not of the one that was selected before.
#[test]
fn switching_selection_never_carries_one_objects_symmetry_onto_another() {
    let mut mesh = ball(20_000);
    let rest = mesh.positions.clone();
    let mut scene = Scene::new("Mirrored");
    let mut history = History::default();
    let mut sculptor = Sculptor::default();
    let mirrored = scene.active_id();
    scene.set_symmetry(mirrored, Symmetry { mirror: [true, true, false], ..radial(6, RadialAxis::Y) });
    let (from, to) = OFF_TO_ONE_SIDE;
    assert!(stroke(&scene, &mut mesh, &mut sculptor, Brush::Clay, from, to).is_some());
    assert!(sculptor.settings.radial.on && sculptor.settings.symmetry == [true, true, false]);
    let many = mesh.positions.iter().zip(&rest).filter(|(a, b)| a != b).count();

    // A new object: no symmetry, so one stroke in one place.
    let before = scene.state();
    let plain = scene.add("Plain", ball(20_000), &mut mesh);
    history.push_scene(before);
    assert!(stroke(&scene, &mut mesh, &mut sculptor, Brush::Clay, from, to).is_some());
    let one = mesh.positions.iter().zip(&rest).filter(|(a, b)| a != b).count();
    assert!(one > 30 && many > 10 * one, "{one} vertices under no symmetry, {many} under twenty-four copies");
    let (_, alone_x) = across(&rest, &mesh.positions, 0);
    assert_eq!(alone_x, one, "nothing was mirrored: every vertex that moved moved alone");
    assert_eq!(sculptor.settings.symmetry_group().len(), 1);

    // Back by a click, then away again by an undo that takes the new object out.
    scene.select(mirrored, &mut mesh);
    scene.hand_symmetry_to(&mut sculptor.settings);
    assert_eq!(sculptor.settings.symmetry_group().len(), 24);
    scene.select(plain, &mut mesh);
    scene.hand_symmetry_to(&mut sculptor.settings);
    assert_eq!(sculptor.settings.symmetry_group().len(), 1);
    assert!(history.step_scene(true, &mut scene, &mut mesh));
    assert_eq!(scene.active_id(), mirrored);
    scene.hand_symmetry_to(&mut sculptor.settings);
    assert_eq!(sculptor.settings.symmetry_group().len(), 24, "the object undo left selected has its own");
}

/// A stroke that lands on an instance of the selected object is made under
/// the selected object's symmetry, in the mesh's own space, as one on a
/// source's instance was in the Unity app.
#[test]
fn a_stroke_on_an_instance_is_under_the_selected_objects_symmetry() {
    let mut mesh = ball(20_000);
    let rest = mesh.positions.clone();
    let mut scene = Scene::new("Sphere");
    let mut sculptor = Sculptor::default();
    let source = scene.active_id();
    scene.set_symmetry(source, mirror(true, false, false));
    let twin = scene.instance_active();
    scene.get_mut(twin).unwrap().position = Vec3::new(1.5, 0.3, 0.0);
    scene.get_mut(twin).unwrap().rotation = Quat::from_rotation_y(0.6);
    scene.select(source, &mut mesh);

    // Through the instance's placement, as a press on it does.
    scene.aim_brush(twin, 0);
    let view = view_of(&scene, &mesh);
    let local = view.local_to(scene.brush_matrix());
    let model = scene.get(twin).unwrap().matrix();
    let (from, to) = OFF_TO_ONE_SIDE;
    let px = |at: Vec3| view.project(model.transform_point3(at)).unwrap();
    scene.hand_symmetry_to(&mut sculptor.settings);
    let settings = &mut sculptor.settings;
    (settings.brush, settings.screen_space_size, settings.radius_world) = (Brush::Inflate, false, 0.08);
    let sample = |i: usize| PointerSample { pos: px(from).lerp(px(to), i as f32 / 20.0), pressure: None, time: i as f64 / 240.0 };
    assert!(sculptor.begin(&mut mesh, &local, sample(0), false, false), "the stroke missed the instance");
    for i in 1..=20 {
        sculptor.sample(&mut mesh, &local, sample(i));
    }
    assert!(sculptor.end(&mut mesh, &local).is_some());
    scene.release_brush();
    let (exact, alone) = across(&rest, &mesh.positions, 0);
    assert!(exact && alone == 0 && mesh.positions != rest, "mirrored across the mesh's own X plane, to the bit");
}

// ------------------------------------------------------------ world space

/// The object the Unity tests place: off to one side, turned and scaled.
fn placed() -> (Vec3, Quat, Vec3) {
    let turn = Quat::from_euler(EulerRot::YXZ, 30f32.to_radians(), 10f32.to_radians(), -20f32.to_radians());
    (Vec3::new(0.25, 0.1, -0.15), turn, Vec3::splat(0.8))
}

/// A ball that is mirror-exact across the WORLD's X plane while the object
/// that shows it is moved, turned and scaled: its vertices are the plain
/// ball's, taken into the object's own space. Returns the scene, the mesh,
/// and for each vertex of the plain ball where the mesh keeps it and which
/// plain vertex is its twin across X.
fn ball_in_the_world(tris: usize) -> (Scene, Mesh, Vec<u32>, Vec<u32>) {
    let plain = ball(tris);
    let twin = twins(&plain.positions, 0);
    let mut scene = Scene::new("Sphere");
    let object = scene.active_mut();
    (object.position, object.rotation, object.scale) = placed();
    let to_own = scene.active().matrix().inverse();
    let own: Vec<Vec3> = plain.positions.iter().map(|&p| to_own.transform_point3(p)).collect();
    let (mesh, kept_at) = Mesh::new_mapped(own, plain.indices.clone(), None);
    (scene, mesh, kept_at, twin)
}

/// How far the sculpted ball is from being its own mirror image across the
/// world's X plane (the furthest any vertex is from its twin's image), and
/// the furthest any vertex is from where it started, both in world units.
fn world_mirror_error(scene: &Scene, mesh: &Mesh, kept_at: &[u32], twin: &[u32], rest: &[Vec3]) -> (f32, f32) {
    let to_world = scene.active().matrix();
    let world = |plain: usize| to_world.transform_point3(mesh.positions[kept_at[plain] as usize]);
    let (mut apart, mut moved) = (0.0f32, 0.0f32);
    for plain in 0..kept_at.len() {
        let image = world(twin[plain] as usize) * Vec3::new(-1.0, 1.0, 1.0);
        apart = apart.max(world(plain).distance(image));
        moved = moved.max(world(plain).distance(to_world.transform_point3(rest[kept_at[plain] as usize])));
    }
    (apart, moved)
}

/// World space on an object that has been moved, turned and scaled: a stroke
/// is mirrored across the world's X plane, wherever that cuts the object, and
/// the two sides stay mirror images to rounding. A turned object's mirror
/// image is a general reflection, so "to the bit" is not on offer (the Unity
/// app's was the same); what is held here is a ten-thousandth of the stroke.
#[test]
fn world_space_on_a_moved_and_turned_object_mirrors_across_the_worlds_plane_and_stays_exact() {
    let (mut scene, mut mesh, kept_at, twin) = ball_in_the_world(20_000);
    let rest = mesh.positions.clone();
    let id = scene.active_id();
    let (floor, _) = world_mirror_error(&scene, &mesh, &kept_at, &twin, &rest);
    assert!(floor < 2e-7, "the ball itself is the world's mirror image to rounding: {floor:.2e}");

    scene.set_symmetry(id, Symmetry { space: Space::World, ..mirror(true, false, false) });
    let mut sculptor = Sculptor::default();
    let to_own = scene.active().matrix().inverse();
    let own = |world: Vec3| to_own.transform_point3(world);
    // Clear of the plane, then along it and across it, where the two copies of
    // every dab overlap; with the brushes that follow the surface they are on.
    let strokes = [
        (Brush::Clay, Vec3::new(0.25, 0.2, 0.4), Vec3::new(0.3, 0.02, 0.38)),
        (Brush::Inflate, Vec3::new(0.03, 0.3, 0.4), Vec3::new(0.05, -0.1, 0.48)),
        (Brush::Crease, Vec3::new(-0.2, -0.2, 0.4), Vec3::new(0.15, -0.3, 0.38)),
        (Brush::Standard, Vec3::new(0.3, -0.05, 0.4), Vec3::new(0.02, 0.0, 0.5)),
        (Brush::Smooth, Vec3::new(0.2, 0.2, 0.4), Vec3::new(-0.05, 0.25, 0.42)),
    ];
    for (brush, from, to) in strokes {
        assert!(stroke(&scene, &mut mesh, &mut sculptor, brush, own(from), own(to)).is_some(), "{brush:?}");
        let (apart, moved) = world_mirror_error(&scene, &mesh, &kept_at, &twin, &rest);
        println!("{brush:?}: the sides are {apart:.2e} apart after strokes that moved the ball {moved:.3e}");
        assert!(moved > 2e-3, "{brush:?} barely moved the ball");
        assert!(apart <= 1e-4 * moved + 2e-6, "{brush:?}: the sides are {apart:.2e} apart, moved {moved:.3e}");
    }
    assert_eq!(sculptor.settings.symmetry_world, Some(scene.active().placement()));

    // Both sides of the world's plane were sculpted, each by as many vertices.
    let to_world = scene.active().matrix();
    let moved = (0..rest.len()).filter(|&v| mesh.positions[v] != rest[v]).map(|v| to_world.transform_point3(rest[v]).x);
    let (left, right) = moved.fold((0usize, 0usize), |(l, r), x| (l + (x < -1e-3) as usize, r + (x > 1e-3) as usize));
    assert!(left > 300 && left.abs_diff(right) <= left / 50, "{left} vertices moved on one side of the world's plane, {right} on the other");
}

/// The planes stay put in the world while the object moves through them: the
/// same stroke, after the object has been moved, is mirrored across the same
/// world plane, which now cuts the object somewhere else.
#[test]
fn a_world_symmetry_stays_in_the_world_when_the_object_is_moved() {
    let mut scene = Scene::new("Sphere");
    let id = scene.active_id();
    let world_x = Symmetry { space: Space::World, ..mirror(true, false, false) };

    // Square on the world's origin, world space is the object's own, to the bit.
    let (from, to) = OFF_TO_ONE_SIDE;
    let mut sculpted = [world_x, mirror(true, false, false)].map(|symmetry| {
        scene.set_symmetry(id, symmetry);
        let mut mesh = ball(20_000);
        assert!(stroke(&scene, &mut mesh, &mut Sculptor::default(), Brush::Clay, from, to).is_some());
        mesh.positions
    });
    assert!(sculpted[0] == sculpted[1], "at the origin a world symmetry is the object's own, to the bit");
    let rest = ball(20_000).positions;
    let (exact, alone) = across(&rest, &sculpted[0], 0);
    assert!(exact && alone == 0 && std::mem::take(&mut sculpted[0]) != rest);

    // Moved a quarter of its width along X, with no turn: the world's plane is
    // the object's own x = -0.25, and the copy is across that.
    let mut mesh = ball(20_000);
    let mut sculptor = Sculptor::default();
    scene.set_symmetry(id, world_x);
    scene.active_mut().position = Vec3::new(0.25, 0.0, 0.0);
    // Down the ball a little to the stroke's side of that plane, where the
    // copy across it still reaches the surface.
    let (from, to) = (Vec3::new(-0.17, 0.15, 0.45), Vec3::new(-0.19, -0.1, 0.45));
    assert!(stroke(&scene, &mut mesh, &mut sculptor, Brush::Inflate, from, to).is_some());
    let moved: Vec<Vec3> = (0..rest.len()).filter(|&v| mesh.positions[v] != rest[v]).map(|v| rest[v]).collect();
    let near = moved.iter().filter(|p| p.x > -0.24).count();
    let far = moved.iter().filter(|p| p.x < -0.26).count();
    assert!(near > 50 && far > 20, "{near} vertices moved on the stroke's side of the world's plane, {far} beyond it");
    // Mirrored across the object's own middle, the copy would be at x = +0.17 to +0.19.
    let furthest = moved.iter().map(|p| p.x).fold(f32::MIN, f32::max);
    assert!(furthest < -0.05, "something moved across the object's own plane, at x = {furthest}");
}

/// Radial symmetry in world space: the repeats go round the world's axis,
/// not the object's, on an object standing off it.
#[test]
fn world_radial_symmetry_goes_round_the_worlds_axis() {
    let (mut scene, mut mesh, _, _) = ball_in_the_world(20_000);
    let rest = mesh.positions.clone();
    let id = scene.active_id();
    scene.set_symmetry(id, Symmetry { space: Space::World, ..radial(4, RadialAxis::Y) });
    let mut sculptor = Sculptor::default();
    let to_own = scene.active().matrix().inverse();
    let own = |world: Vec3| to_own.transform_point3(world);
    assert!(stroke(&scene, &mut mesh, &mut sculptor, Brush::Inflate, own(Vec3::new(0.2, 0.15, 0.42)), own(Vec3::new(0.3, 0.05, 0.38))).is_some());
    // The same number of vertices moved in each quarter turn about the world's Y.
    let to_world = scene.active().matrix();
    let mut quarters = [0usize; 4];
    for v in (0..rest.len()).filter(|&v| mesh.positions[v] != rest[v]) {
        let p = to_world.transform_point3(rest[v]);
        quarters[(p.x > 0.0) as usize | ((p.z > 0.0) as usize) << 1] += 1;
    }
    let (fewest, most) = (*quarters.iter().min().unwrap(), *quarters.iter().max().unwrap());
    assert!(fewest > 30 && most - fewest <= most / 5, "vertices moved by quarter of the world: {quarters:?}");
}

// ------------------------------------------------------------ guides

#[test]
fn the_guides_are_the_selected_objects_in_the_space_its_symmetry_sits_in() {
    let mut mesh = ball(2_000);
    let mut scene = Scene::new("Sphere");
    let first = scene.active_id();
    (scene.active_mut().position, scene.active_mut().rotation, scene.active_mut().scale) = placed();
    scene.set_symmetry(first, mirror(true, false, false));
    let guides = scene.guides();
    assert_eq!((guides.mirror, guides.radial, guides.space), ([true, false, false], None, glam::Mat4::IDENTITY));
    // Fixed in the world, the planes are the world's: the surface goes there by the object's placement.
    scene.edit_symmetry(first, |symmetry| symmetry.space = Space::World);
    assert_eq!(scene.guides().space, scene.active().matrix());

    // Another object selected: its own guides, and none of the first's.
    let second = scene.add("Vase", ball(2_000), &mut mesh);
    assert_eq!(scene.guides().mirror, [false; 3]);
    scene.set_symmetry(second, radial(5, RadialAxis::Y));
    let guides = scene.guides();
    assert_eq!(guides.radial.map(|(axis, _)| axis), Some([0.0, 1.0, 0.0, 5.0]));
    assert_eq!(guides.space, glam::Mat4::IDENTITY);
    scene.select(first, &mut mesh);
    assert_eq!((scene.guides().mirror, scene.guides().radial), ([true, false, false], None));
}

#[test]
fn the_card_header_says_where_a_symmetry_sits() {
    assert_eq!(Symmetry::default().summary(), "Off");
    assert_eq!(mirror(true, false, true).summary(), "X + Z");
    let world = Symmetry { space: Space::World, mirror: [true, false, false], ..radial(6, RadialAxis::Z) };
    assert_eq!(world.summary(), "X + Radial 6 (World)");
    assert_eq!(Symmetry { space: Space::World, ..Symmetry::default() }.summary(), "Off", "nothing is on to be anywhere");
    // Only a symmetry of the object's own can be made exact by a remesh.
    assert_eq!(mirror(true, false, true).exact_mirrors(), [true, false, true]);
    assert_eq!(world.exact_mirrors(), [false; 3]);
}

// ------------------------------------------------------------ files

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("sculpt_symmetry_tests");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// Opens a file's objects in place of a scene's, as the app does.
fn open(path: &std::path::Path) -> (Scene, Mesh, io::Details) {
    let saved = io::load_scene(path).expect("the file reads");
    let mut slot = ball(200);
    let mut scene = Scene::new("Sphere");
    let objects = saved
        .objects
        .into_iter()
        .map(|o| Object {
            id: 0,
            name: o.name,
            visible: o.visible,
            mesh: o.mesh as u64,
            position: o.position,
            rotation: o.rotation,
            scale: o.scale,
            mirror: None,
        })
        .collect();
    scene.load(saved.meshes, objects, saved.active, &mut slot);
    scene.restore_symmetry(&saved.details);
    (scene, slot, saved.details)
}

fn rows(scene: &Scene) -> Vec<SavedObject> {
    // Each object on a mesh of its own, but for the instance, which shares the row before it.
    let mut meshes: Vec<u64> = Vec::new();
    scene
        .objects
        .iter()
        .map(|o| {
            let mesh = meshes.iter().position(|&m| m == o.mesh).unwrap_or_else(|| {
                meshes.push(o.mesh);
                meshes.len() - 1
            });
            SavedObject { name: o.name.clone(), visible: o.visible, mesh, position: o.position, rotation: o.rotation, scale: o.scale }
        })
        .collect()
}

#[test]
fn a_file_keeps_every_objects_own_symmetry() {
    let mut mesh = ball(600);
    let mut scene = Scene::new("Face");
    let face = scene.active_id();
    let body = scene.add("Body", ball(600), &mut mesh);
    let vase = scene.add("Vase", ball(600), &mut mesh);
    let plain = scene.add("Plain", ball(600), &mut mesh);
    scene.set_symmetry(face, mirror(true, false, false));
    scene.set_symmetry(body, Symmetry { space: Space::World, ..mirror(false, false, true) });
    let slanted = Radial { on: true, count: 7, axis: RadialAxis::Custom, custom_axis: Vec3::new(0.0, 0.6, 0.8) };
    scene.set_symmetry(vase, Symmetry { mirror: [false, true, false], radial: slanted, space: Space::Local });
    scene.select(body, &mut mesh);
    let twin = scene.instance_active();
    let kept: Vec<Symmetry> = scene.objects.iter().map(|o| scene.symmetry(o.id)).collect();
    assert_eq!(scene.symmetry(twin), scene.symmetry(body));

    let mut details = io::Details::default();
    scene.keep_symmetry(&mut details);
    let objects = rows(&scene);
    let all: Vec<&Mesh> = [face, body, vase, plain].iter().map(|&id| scene.mesh(scene.get(id).unwrap().mesh, &mesh).unwrap()).collect();
    let path = temp("each_its_own.sclp");
    io::save_scene_with(&all, &objects, 1, &details, &path).expect("the file writes");

    let (opened, _, _) = open(&path);
    let names: Vec<&str> = opened.objects.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(names, ["Face", "Body", "Body Instance", "Vase", "Plain"]);
    let read: Vec<Symmetry> = opened.objects.iter().map(|o| opened.symmetry(o.id)).collect();
    assert_eq!(read, kept, "every object's mirror axes, radial symmetry and space");
    assert_eq!(read[4], Symmetry::default(), "one with none set comes back with none");
    let _ = std::fs::remove_file(&path);
}

/// A file from before symmetry belonged to objects holds one setting for the
/// scene. That was every object's in turn (it applied to whichever was
/// selected, in its own space), so every object of the file gets it.
#[test]
fn a_file_from_before_gives_its_one_setting_to_every_object() {
    let mesh = ball(600);
    let objects: Vec<SavedObject> = ["Head", "Hat", "Hand"]
        .into_iter()
        .map(|name| SavedObject {
            name: name.into(),
            visible: true,
            mesh: 0,
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        })
        .collect();
    let mut details = io::Details::default();
    let six_round_z = Radial { on: true, count: 6, axis: RadialAxis::Z, custom_axis: Vec3::Y };
    details.set(io::slot::MIRROR, &Mirror { x: true, y: false, z: true, guides: false, radial: six_round_z });
    let path = temp("one_setting.sclp");
    io::save_scene_with(&[&mesh], &objects, 2, &details, &path).expect("the file writes");

    let (opened, _, details) = open(&path);
    assert_eq!(opened.objects.len(), 3);
    for o in &opened.objects {
        let own = opened.symmetry(o.id);
        assert_eq!((own.mirror, own.radial, own.space), ([true, false, true], six_round_z, Space::Local), "{}", o.name);
    }
    assert!(saved_symmetry(&details, 7).is_some(), "the scene's one setting, whatever the row");

    // And one from before any of it was kept gives none: its objects start with no symmetry.
    io::save_scene_with(&[&mesh], &objects, 0, &io::Details::default(), &path).expect("the file writes");
    let (opened, _, details) = open(&path);
    assert!(opened.objects.iter().all(|o| opened.symmetry(o.id) == Symmetry::default()));
    assert_eq!(saved_symmetry(&details, 0), None);
    let _ = std::fs::remove_file(&path);
}

/// What a version of the app from before this one finds in a new file: the
/// scene-wide slot it has always read is still there, so the file opens, with
/// the selected object's axes. And what this version makes of a slot written
/// by a later one: what it can read of it, with the rest as an object starts.
#[test]
fn an_objects_symmetry_reads_as_far_as_it_can() {
    let mut details = io::Details::default();
    details.set_object(0, io::slot::SYMMETRY, &serde_json::json!({ "mirror": [false, true, false] }));
    details.set_object(1, io::slot::SYMMETRY, &serde_json::json!({ "space": "screen", "mirror": [true, false, false], "radial": { "on": true, "count": 900, "axis": "x" } }));
    details.set_object(2, io::slot::SYMMETRY, &serde_json::json!({ "space": "world" }));
    let first = saved_symmetry(&details, 0).unwrap();
    assert_eq!(first, mirror(false, true, false), "what is missing is as an object starts");
    let second = saved_symmetry(&details, 1).unwrap();
    assert_eq!((second.mirror, second.space), ([true, false, false], Space::Local), "a space this version has not heard of");
    assert_eq!((second.radial.on, second.radial.count, second.radial.axis), (true, Radial::MAX_COUNT, RadialAxis::X));
    assert_eq!(saved_symmetry(&details, 2).unwrap().space, Space::World);
    assert_eq!(saved_symmetry(&details, 3), None, "no slot and no setting for the scene");

    // As the file has it, for a person reading it.
    let json = serde_json::to_value(Symmetry { space: Space::World, ..mirror(true, false, false) }).unwrap();
    assert_eq!(json["space"], "world");
    assert_eq!(json["mirror"], serde_json::json!([true, false, false]));
    assert_eq!(json["radial"]["axis"], "y");
}
