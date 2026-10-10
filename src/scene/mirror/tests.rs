//! The live mirror on the scene's side: Unity's `MirrorRepeaterTests` for the
//! parts that are the app's (the arithmetic itself is tested in
//! `sculpt_symmetry::mirror`), and the edits the interface makes.

use glam::{EulerRot, Quat, Vec2};

use super::*;
use crate::camera::Camera;
use crate::history::History;
use crate::mesh::primitives::{self, Primitive};
use crate::sculpt::{Brush, PointerSample, Sculptor};

const SIZE: Vec2 = Vec2::new(1600.0, 900.0);

fn ball(tris: usize) -> Mesh {
    primitives::build(Primitive::Sphere, tris, 0.5)
}

/// The Unity tests' object: off to one side, turned and scaled.
fn placed(tris: usize) -> (Scene, Mesh) {
    let mesh = ball(tris);
    let mut scene = Scene::new("Sphere");
    let object = scene.active_mut();
    object.position = Vec3::new(0.7, 0.2, -0.3);
    object.rotation = Quat::from_euler(EulerRot::YXZ, 30f32.to_radians(), 10f32.to_radians(), -20f32.to_radians());
    object.scale = Vec3::splat(0.8);
    (scene, mesh)
}

fn signed_volume(mesh: &Mesh) -> f64 {
    mesh.indices
        .chunks_exact(3)
        .map(|t| {
            let p = [0, 1, 2].map(|c| mesh.positions[t[c] as usize].as_dvec3());
            p[0].dot(p[1].cross(p[2])) / 6.0
        })
        .sum()
}

/// One undo step for an edit of the object list, as `Running::edit_scene` makes it.
fn edit(scene: &mut Scene, mesh: &mut Mesh, history: &mut History, change: impl FnOnce(&mut Scene, &mut Mesh) -> bool) {
    let before = scene.state();
    if change(scene, mesh) {
        history.push_scene(before);
    }
    scene.collect(|id| history.keeps_mesh(id));
}

fn undo(scene: &mut Scene, mesh: &mut Mesh, history: &mut History, undo: bool) {
    assert!(history.step_scene(undo, scene, mesh), "nothing to step");
    scene.collect(|id| history.keeps_mesh(id));
}

#[test]
fn copies_are_the_world_reflection_of_the_object() {
    let (mut scene, mesh) = placed(2_000);
    let id = scene.active_id();
    assert_eq!(scene.active().placements().count(), 1, "no mirror, no copies");

    assert!(scene.set_mirror(id, 1 | 4, Vec3::ZERO));
    assert!(!scene.set_mirror(id, 1 | 4, Vec3::ZERO), "the same mirror again is not a change");
    let object = scene.active().clone();
    let copies: Vec<(u8, Mat4)> = object.placements().collect();
    assert_eq!(copies.iter().map(|c| c.0).collect::<Vec<_>>(), [0, 1, 4, 5], "the object, then X, Z and XZ");
    assert_eq!(copies[0].1, object.matrix());

    let world = object.matrix();
    for &(flip, model) in &copies[1..] {
        let copy = MirrorCopy::new(flip);
        // A copy that is mirror-reversed is drawn with a mirroring matrix.
        assert_eq!(model.determinant() < 0.0, copy.reflects(), "copy {}", copy.axis_name());
        assert_eq!(object.placed_matrix(flip), model);
        for &v in mesh.positions.iter().step_by(13) {
            let expected = world.transform_point3(v) * copy.signs;
            assert!(expected.distance(model.transform_point3(v)) < 1e-5, "copy {} vertex {v}", copy.axis_name());
        }
    }
    // Only the copies it has.
    assert!([1, 4, 5].into_iter().all(|c| object.has_copy(c)));
    assert!([0, 2, 3, 6, 7].into_iter().all(|c| !object.has_copy(c)));
    assert_eq!(object.placed_matrix(2), world, "a copy it does not have is the object itself");

    // Remove takes them all away.
    assert!(scene.set_mirror(id, 0, Vec3::ZERO));
    assert_eq!(scene.active().placements().count(), 1);
    assert!(!scene.set_mirror(id, 0, Vec3::ZERO));
}

#[test]
fn copies_follow_a_moved_turned_and_stretched_object_and_a_moved_centre() {
    let (mut scene, mesh) = placed(2_000);
    let id = scene.active_id();
    let center = Vec3::new(0.4, -1.0, 2.0);
    scene.set_mirror(id, 1, center);

    let object = scene.active_mut();
    object.position = Vec3::new(1.5, -0.4, 0.2);
    object.rotation = Quat::from_euler(EulerRot::YXZ, 1.2, 0.0, 0.09);
    object.scale = Vec3::new(0.5, 1.0, 2.0);
    let object = scene.active().clone();

    let (_, model) = object.placements().nth(1).unwrap();
    for &v in &mesh.positions {
        let world = object.matrix().transform_point3(v);
        let expected = center + (world - center) * Vec3::new(-1.0, 1.0, 1.0);
        assert!(expected.distance(model.transform_point3(v)) < 1e-5);
    }
}

#[test]
fn a_ray_meets_the_copy_and_maps_back_to_the_same_vertices() {
    let (mut scene, mesh) = placed(20_000);
    let id = scene.active_id();
    scene.set_mirror(id, 1, Vec3::ZERO);
    let object = scene.active().clone();
    let reflect = Vec3::new(-1.0, 1.0, 1.0);

    // Straight at the copy's middle from its outer side.
    let copy_centre = object.position * reflect;
    let (origin, dir) = (copy_centre + Vec3::new(-5.0, 0.0, 0.0), Vec3::X);
    let hit = scene.pick(origin, dir, &mesh).expect("the copy is in the way");
    assert_eq!((hit.id, hit.copy), (id, 1), "the copy is what is met");
    assert!(hit.normal.x < 0.0, "its surface faces back along the ray");

    // The point it met, seen through the copy's matrix, is a point of the mesh; through
    // the object's own, it is the reflection.
    let local = object.placed_matrix(1).inverse().transform_point3(hit.pos);
    assert!((local.length() - 0.5).abs() < 2e-3, "{local} is not on the ball");
    assert!((object.matrix().transform_point3(local) * reflect).distance(hit.pos) < 1e-4);

    // The object still wins where it is nearer, and is met as itself.
    let (origin, dir) = (object.position + Vec3::new(5.0, 0.0, 0.0), Vec3::NEG_X);
    let own = scene.pick(origin, dir, &mesh).unwrap();
    assert_eq!((own.id, own.copy), (id, 0));

    // A hidden object has no copies on screen either.
    scene.get_mut(id).unwrap().visible = false;
    assert!(scene.pick(copy_centre + Vec3::new(-5.0, 0.0, 0.0), Vec3::X, &mesh).is_none());
    scene.get_mut(id).unwrap().visible = true;

    // And with the mirror removed there is no copy to be met: the ray goes on to the object.
    scene.set_mirror(id, 0, Vec3::ZERO);
    let through = scene.pick(copy_centre + Vec3::new(-5.0, 0.0, 0.0), Vec3::X, &mesh).unwrap();
    assert_eq!((through.id, through.copy), (id, 0));
    assert!(through.pos.x > 0.0);
}

#[test]
fn bounds_and_framing_include_the_copies_but_the_object_alone_does_not() {
    let (mut scene, mesh) = placed(2_000);
    let id = scene.active_id();
    let alone = scene.bounds(&mesh);
    scene.set_mirror(id, 1, Vec3::ZERO);

    let object = scene.active().clone();
    let all = scene.bounds(&mesh);
    assert!(all.min.x < -alone.max.x + 0.01 && (all.max.x - alone.max.x).abs() < 1e-5, "{all:?} against {alone:?}");
    assert_eq!(scene.selection_bounds(&mesh).min, all.min);
    assert_eq!(scene.shown_bounds_of(&object, &mesh).min, all.min);
    // The gizmo is sized to the object, not to the pair.
    assert_eq!(scene.bounds_of(&object, &mesh).min, alone.min);
}

#[test]
fn a_brush_on_the_copy_sculpts_the_vertices_the_copy_shows() {
    let (mut scene, mut mesh) = placed(60_000);
    let id = scene.active_id();
    scene.set_mirror(id, 1, Vec3::ZERO);
    let object = scene.active().clone();
    let (copy_model, own_model) = (object.placed_matrix(1), object.matrix());

    let mut camera = Camera::default();
    camera.frame(scene.bounds(&mesh), SIZE.x / SIZE.y, false);
    (camera.yaw, camera.pitch) = (10.0, 12.0);
    let view = camera.view(Vec2::ZERO, SIZE, camera.distance, 1.0);

    // The pointer is over the copy.
    let centre = view.project(copy_model.transform_point3(Vec3::ZERO)).unwrap();
    let (origin, dir) = view.ray(centre);
    let seen = scene.pick(origin, dir, &mesh).expect("the copy is under the pointer");
    assert_eq!((seen.id, seen.copy), (id, 1));
    scene.aim_brush(seen.id, seen.copy);
    assert_eq!(scene.brush_matrix(), copy_model);
    assert_eq!(scene.stroke_matrix(), copy_model);
    assert_eq!(scene.brush_object().id, id);
    let local = view.local_to(scene.brush_matrix());

    let before = mesh.positions.clone();
    let mut sculptor = Sculptor::default();
    sculptor.settings.brush = Brush::Inflate;
    sculptor.settings.symmetry = [false; 3];
    sculptor.settings.radius_px = 40.0;
    let at = |i: usize| PointerSample {
        pos: centre + Vec2::new(i as f32 * 2.0 - 20.0, 0.0),
        pressure: None,
        time: i as f64 / 60.0,
    };
    assert!(sculptor.begin(&mut mesh, &local, at(0), false, false), "the stroke missed the copy");
    for i in 1..=20 {
        sculptor.sample(&mut mesh, &local, at(i));
        sculptor.frame(&mut mesh, &local, 1.0 / 60.0);
    }
    sculptor.end(&mut mesh, &local);

    let moved: Vec<usize> = (0..before.len()).filter(|&v| mesh.positions[v] != before[v]).collect();
    assert!(!moved.is_empty(), "the stroke changed nothing");
    let stroke_end = centre + Vec2::new(20.0, 0.0);
    let copy_centre = copy_model.transform_point3(Vec3::ZERO);
    let mut through_object = 0;
    for &v in &moved {
        // What moved is what the copy shows under the stroke...
        let px = view.project(copy_model.transform_point3(before[v])).unwrap();
        assert!(
            px.distance(centre) < 80.0 || px.distance(stroke_end) < 80.0,
            "moved a vertex far from the stroke: {px}"
        );
        // ...and an inflate pushes it out of the copy, not into it.
        let (was, now) = (copy_model.transform_point3(before[v]), copy_model.transform_point3(mesh.positions[v]));
        assert!(now.distance(copy_centre) > was.distance(copy_centre) - 1e-6, "pushed into the copy");
        // The same vertices through the object's own matrix are on the other side of the picture.
        if view.project(own_model.transform_point3(before[v])).is_some_and(|p| p.distance(centre) > 200.0) {
            through_object += 1;
        }
    }
    assert!(through_object * 2 > moved.len(), "the stroke went through the object instead of the copy");

    // A stroke over the object itself goes through the object.
    scene.release_brush();
    assert_eq!(scene.brush_matrix(), own_model);
}

/// A reflected view turns right and left over in the object's own space. The brushes that pull
/// the surface along with the pointer must still carry it where the pointer goes, on the copy.
#[test]
fn a_brush_that_drags_the_surface_follows_the_pointer_on_the_copy_as_on_the_object() {
    for brush in [Brush::Move, Brush::SnakeHook] {
        let mut carried = Vec::new();
        for copy in [0u8, 1] {
            let (mut scene, mut mesh) = placed(60_000);
            let id = scene.active_id();
            scene.set_mirror(id, 1, Vec3::ZERO);
            let model = scene.active().placed_matrix(copy);

            let mut camera = Camera::default();
            camera.frame(scene.bounds(&mesh), SIZE.x / SIZE.y, false);
            (camera.yaw, camera.pitch) = (10.0, 12.0);
            let view = camera.view(Vec2::ZERO, SIZE, camera.distance, 1.0);
            let centre = view.project(model.transform_point3(Vec3::ZERO)).unwrap();
            let local = view.local_to(model);

            let before = mesh.positions.clone();
            let mut sculptor = Sculptor::default();
            sculptor.settings.brush = brush;
            sculptor.settings.symmetry = [false; 3];
            sculptor.settings.radius_px = 50.0;
            // Along the pointer's way: 80 pixels right and 30 up.
            let to = Vec2::new(80.0, -30.0);
            let at = |i: usize| PointerSample { pos: centre + to * (i as f32 / 20.0), pressure: None, time: i as f64 / 60.0 };
            assert!(sculptor.begin(&mut mesh, &local, at(0), false, false), "{brush:?} missed copy {copy}");
            for i in 1..=20 {
                sculptor.sample(&mut mesh, &local, at(i));
                sculptor.frame(&mut mesh, &local, 1.0 / 60.0);
            }
            sculptor.end(&mut mesh, &local);

            // How far, and which way, what moved has gone across the picture.
            let (mut sum, mut count) = (Vec2::ZERO, 0);
            for v in (0..before.len()).filter(|&v| mesh.positions[v] != before[v]) {
                let (was, now) = (model.transform_point3(before[v]), model.transform_point3(mesh.positions[v]));
                sum += view.project(now).unwrap() - view.project(was).unwrap();
                count += 1;
            }
            assert!(count > 0, "{brush:?} changed nothing on copy {copy}");
            carried.push(sum / count as f32);
        }
        let (own, copy) = (carried[0], carried[1]);
        assert!(copy.x > 5.0 && copy.y < -1.0, "{brush:?} on the copy went {copy}, not with the pointer");
        assert!(own.distance(copy) < 0.1 * own.length() + 1.0, "{brush:?}: the copy went {copy}, the object {own}");
    }
}

#[test]
fn the_gizmo_on_a_copy_moves_the_object_mirrored() {
    let (mut scene, mut mesh) = placed(2_000);
    let id = scene.active_id();
    let center = Vec3::new(0.1, 0.0, 0.0);
    scene.set_mirror(id, 1, center);
    let repeater = scene.active().mirror.unwrap();
    let copy = repeater.copies()[0];

    // Nothing chosen: the gizmo is on the object.
    assert_eq!(scene.selected_side(), 0);
    assert_eq!(scene.shown_placement(&scene.active().clone()), scene.active().placement());
    // A copy the object does not have cannot be chosen; another object's cannot either.
    scene.select_side(id, 2);
    assert_eq!(scene.selected_side(), 0);

    scene.select_side(id, 1);
    assert_eq!(scene.selected_side(), 1);
    assert_eq!(scene.shown_matrix(), scene.active().placed_matrix(1));
    let object = scene.active().clone();
    let shown = scene.shown_placement(&object);
    let drawn = copy.placement(&object.placement(), center);
    assert!(shown.position.distance(drawn.position) < 1e-6);
    assert!(shown.scale.x > 0.0, "reported as the object's positive scale");

    // Pressing a handle and not moving it changes nothing, to the last bit.
    let before = scene.active().clone();
    scene.place_shown(id, shown);
    assert_eq!(scene.active(), &before);

    // Moving the copy left moves the object right.
    let start = object.position;
    scene.place_shown(id, Placement { position: shown.position + Vec3::new(-0.5, 0.1, 0.0), ..shown });
    assert!(scene.active().position.distance(start + Vec3::new(0.5, 0.1, 0.0)) < 1e-5);

    // Turning the copy turns the object so that the copy shows the turn asked for.
    let object = scene.active().clone();
    let shown = scene.shown_placement(&object);
    let spun = Quat::from_axis_angle(Vec3::Y, 25f32.to_radians()) * shown.rotation;
    scene.place_shown(id, Placement { rotation: spun, ..shown });
    let now = copy.baked_placement(&scene.active().placement(), center).rotation;
    assert!(now.angle_between(spun) < 1e-3);

    // Stretching it stretches the object along the same axis of its own.
    let shown = scene.shown_placement(&scene.active().clone());
    scene.place_shown(id, Placement { scale: shown.scale * Vec3::new(1.0, 2.0, 1.0), ..shown });
    assert!((scene.active().scale.y - 1.6).abs() < 1e-5 && scene.active().scale.x == 0.8);

    // The side belongs to the object it was chosen for, and to the copy being there.
    scene.set_mirror(id, 0, center);
    assert_eq!(scene.selected_side(), 0, "no copy left to be on");
    scene.set_mirror(id, 1, center);
    assert_eq!(scene.selected_side(), 0, "a new mirror starts on the object");
    scene.select_side(id, 1);
    scene.set_mirror(id, 1 | 2, center);
    assert_eq!(scene.selected_side(), 1, "changing the axes leaves a copy that is still there selected");
    let other = scene.add("Cube", primitives::build(Primitive::Cube, 300, 0.5), &mut mesh);
    assert_eq!(scene.selected_side(), 0);
    scene.select(id, &mut mesh);
    assert_eq!(scene.selected_side(), 0, "a plain selection is the object");
    scene.select_side(other, 1);
    assert_eq!(scene.selected_side(), 0);
}

#[test]
fn turning_the_mirror_on_and_off_is_one_undo_step_each() {
    let (mut scene, mut mesh) = placed(2_000);
    let mut history = History::default();
    let id = scene.active_id();

    edit(&mut scene, &mut mesh, &mut history, |s, _| s.set_mirror(id, 1, Vec3::ZERO));
    edit(&mut scene, &mut mesh, &mut history, |s, _| s.set_mirror(id, 1, Vec3::ZERO));
    assert_eq!(history.steps(), (1, 0), "asking for what is there leaves no step");
    edit(&mut scene, &mut mesh, &mut history, |s, _| s.set_mirror(id, 1 | 2, Vec3::ZERO));
    edit(&mut scene, &mut mesh, &mut history, |s, _| s.set_mirror(id, 0, Vec3::ZERO));
    assert_eq!(history.steps(), (3, 0));
    assert!(scene.active().mirror.is_none());

    // Off, undone: it is back across both axes. Then the change of axes. Then on.
    undo(&mut scene, &mut mesh, &mut history, true);
    assert_eq!(scene.active().mirror.map(|m| m.axis_mask), Some(3));
    assert_eq!(scene.active().placements().count(), 4);
    undo(&mut scene, &mut mesh, &mut history, true);
    assert_eq!(scene.active().mirror.map(|m| m.axis_mask), Some(1));
    undo(&mut scene, &mut mesh, &mut history, true);
    assert!(scene.active().mirror.is_none());
    assert!(!history.can_undo());

    // And redo does each again.
    for want in [Some(1), Some(3), None] {
        undo(&mut scene, &mut mesh, &mut history, false);
        assert_eq!(scene.active().mirror.map(|m| m.axis_mask), want);
    }
}

#[test]
fn an_instance_or_a_clone_of_a_mirrored_object_is_not_mirrored() {
    let (mut scene, mut mesh) = placed(2_000);
    let id = scene.active_id();
    scene.set_mirror(id, 1, Vec3::ZERO);
    let twin = scene.instance_active();
    assert!(scene.get(twin).unwrap().mirror.is_none());
    scene.select(id, &mut mesh);
    let copy = scene.clone_active(&mut mesh);
    assert!(scene.get(copy).unwrap().mirror.is_none());
    assert!(scene.get(id).unwrap().mirror.is_some());
}

/// Every vertex of `mesh` through `model`.
fn world_points(mesh: &Mesh, model: &Mat4) -> Vec<Vec3> {
    mesh.positions.iter().map(|&p| model.transform_point3(p)).collect()
}

#[test]
fn validate_makes_real_objects_exactly_where_the_copies_were_and_undoes_in_one_step() {
    let (mut scene, mut mesh) = placed(1_200);
    let mut history = History::default();
    let id = scene.active_id();
    // A mask that tells vertices apart, so it can be followed onto the copies.
    for v in 0..mesh.vertex_count() {
        mesh.mask[v] = (mesh.positions[v].y + 0.5).clamp(0.0, 1.0);
    }
    edit(&mut scene, &mut mesh, &mut history, |s, _| s.set_mirror(id, 1 | 4, Vec3::ZERO));

    let source = scene.active().clone();
    let repeater = source.mirror.unwrap();
    let original = (mesh.positions.clone(), mesh.normals.clone(), mesh.indices.clone(), mesh.mask.clone());
    let volume = signed_volume(&mesh);
    let live: Vec<Mat4> = source.placements().skip(1).map(|(_, m)| m).collect();
    let steps = history.steps().0;

    let mut made = Vec::new();
    edit(&mut scene, &mut mesh, &mut history, |s, slot| {
        made = s.bake_mirror(id, slot).expect("it has a mirror");
        true
    });
    assert_eq!(history.steps().0, steps + 1, "one undo step");
    assert_eq!(made.len(), 3);
    assert!(scene.get(id).unwrap().mirror.is_none(), "the live mirror is gone");
    assert_eq!((scene.active_id(), scene.selected()), (id, &[id][..]), "the object is what is selected");
    let names: Vec<&str> = scene.objects.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(names, ["Sphere", "Sphere Mirror X", "Sphere Mirror Z", "Sphere Mirror XZ"]);

    // What the geometry crate says a baked copy is.
    let baked = repeater.bake(&original.0, Some(&original.1), &original.2, &source.placement());
    for ((&new, bake), live) in made.iter().zip(&baked).zip(&live) {
        let object = scene.get(new).unwrap();
        assert_eq!(object.placement(), bake.placement, "copy {}", bake.copy.axis_name());
        assert_eq!(object.scale, source.scale, "the original's positive scale");
        assert!(object.mirror.is_none() && object.visible);
        let own = scene.mesh(object.mesh, &mesh).expect("a mesh of its own");
        assert_ne!(object.mesh, source.mesh);
        assert_eq!(own.vertex_count(), baked[0].geometry.positions.len());
        assert_eq!(own.triangle_count(), original.2.len() / 3);

        // Outward, and as large as the original: reflecting turned the faces over with it.
        assert!((signed_volume(own) - volume).abs() < 1e-9 * volume.abs().max(1.0) + 1e-9, "inside out");
        // The reflected local geometry, vertex for vertex (the order is the mesh's own).
        let mut asked: Vec<Vec3> = bake.geometry.positions.clone();
        asked.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)).then(a.z.total_cmp(&b.z)));
        let mut got = own.positions.clone();
        got.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)).then(a.z.total_cmp(&b.z)));
        assert_eq!(got, asked);

        // Placed, it is on top of the live copy it replaces.
        let now = world_points(own, &object.matrix());
        let was = world_points(&mesh_from(&original), live);
        assert_eq!(now.len(), was.len());
        assert!(now.iter().all(|a| was.iter().any(|b| a.distance(*b) < 1e-4)), "not where the live copy was");

        // The mask goes with each vertex.
        for (p, &m) in own.positions.iter().zip(&own.mask) {
            assert!(((p.y + 0.5).clamp(0.0, 1.0) - m).abs() < 1e-6, "mask {m} at {p}");
        }
    }

    // One undo: the one object, mirrored as it was.
    undo(&mut scene, &mut mesh, &mut history, true);
    assert_eq!(scene.objects.len(), 1);
    assert_eq!(scene.active().mirror, Some(repeater));
    assert_eq!(mesh.positions, original.0);
    // Redo makes them again, with their meshes.
    undo(&mut scene, &mut mesh, &mut history, false);
    assert_eq!(scene.objects.len(), 4);
    assert!(scene.get(id).unwrap().mirror.is_none());
    for &new in &made {
        let object = scene.get(new).unwrap();
        let own = scene.mesh(object.mesh, &mesh).expect("its mesh is back");
        assert!((signed_volume(own) - volume).abs() < 1e-6);
    }
    // Nothing to validate on an object with no mirror.
    assert!(scene.bake_mirror(id, &mut mesh).is_none());
}

/// A mesh with these arrays, for `world_points`.
fn mesh_from(arrays: &(Vec<Vec3>, Vec<Vec3>, Vec<u32>, Vec<f32>)) -> Mesh {
    Mesh::new(arrays.0.clone(), arrays.2.clone(), Some(arrays.3.clone()))
}

#[test]
fn validating_a_hidden_object_makes_hidden_copies_and_in_list_order() {
    let (mut scene, mut mesh) = placed(600);
    let first = scene.active_id();
    let last = scene.add("Cube", primitives::build(Primitive::Cube, 300, 0.5), &mut mesh);
    scene.select(first, &mut mesh);
    scene.set_mirror(first, 1, Vec3::ZERO);
    scene.get_mut(first).unwrap().visible = false;
    let made = scene.bake_mirror(first, &mut mesh).unwrap();
    assert!(!scene.get(made[0]).unwrap().visible);
    let order: Vec<u64> = scene.objects.iter().map(|o| o.id).collect();
    assert_eq!(order, [first, made[0], last], "right under the object, not at the bottom");
    // The name is taken again the next time.
    scene.set_mirror(first, 1, Vec3::ZERO);
    let again = scene.bake_mirror(first, &mut mesh).unwrap();
    assert_eq!(scene.get(again[0]).unwrap().name, "Sphere Mirror X (2)");
}

#[test]
fn what_a_file_keeps_of_the_mirror_reads_back_and_ignores_what_it_cannot() {
    let (mut scene, mut mesh) = placed(600);
    let first = scene.active_id();
    let plain = scene.add("Cube", primitives::build(Primitive::Cube, 300, 0.5), &mut mesh);
    scene.set_mirror(first, 2 | 4, Vec3::new(0.0, 0.5, -1.0));

    let mut details = Details::default();
    scene.keep_mirrors(&mut details);
    assert!(details.get_object::<SavedMirror>(1, slot::MIRROR_COPIES).is_none(), "an object with none keeps none");

    scene.set_mirror(first, 0, Vec3::ZERO);
    scene.restore_mirrors(&details);
    assert_eq!(scene.get(first).unwrap().mirror, MirrorRepeater::new(6, Vec3::new(0.0, 0.5, -1.0)));
    assert!(scene.get(plain).unwrap().mirror.is_none());

    // An entry with no axes is no mirror; a centre that is not a number is the origin.
    assert_eq!(SavedMirror { axes: 0, center: [1.0; 3] }.repeater(), None);
    assert_eq!(SavedMirror { axes: 8, center: [1.0; 3] }.repeater(), None, "only three bits name axes");
    let odd = SavedMirror { axes: 1, center: [f32::NAN, 0.0, 0.0] }.repeater().unwrap();
    assert_eq!(odd.center, Vec3::ZERO);
    // Fields it has not heard of, or that are missing, read as what they were.
    let json = serde_json::json!({ "axes": 3, "later": "thing" });
    let read: SavedMirror = serde_json::from_value(json).unwrap();
    assert_eq!(read, SavedMirror { axes: 3, center: [0.0; 3] });
}
