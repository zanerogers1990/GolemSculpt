use std::collections::HashMap;

use glam::{Vec2, Vec3};

use crate::camera::{Camera, View};
use crate::history::History;
use crate::mesh::primitives::{self, Primitive};
use crate::mesh::{Mesh, StrokeDelta};
use crate::scene::Scene;
use crate::sculpt::{Brush, PointerSample, Sculptor};

const SIZE: Vec2 = Vec2::new(1600.0, 900.0);

fn sphere(tris: usize) -> Mesh {
    primitives::build(Primitive::Sphere, tris, 0.5)
}

fn view_of(mesh: &Mesh) -> View {
    let mut camera = Camera::default();
    camera.frame(mesh.bounds(), SIZE.x / SIZE.y, false);
    camera.view(Vec2::ZERO, SIZE, camera.distance, 1.0)
}

/// Index of each vertex's mirror image across X. Panics if the mesh is not
/// mirror-exact.
fn twins(mesh: &Mesh) -> Vec<u32> {
    let key = |p: Vec3| [(p.x + 0.0).to_bits(), (p.y + 0.0).to_bits(), (p.z + 0.0).to_bits()];
    let map: HashMap<[u32; 3], u32> =
        mesh.positions.iter().enumerate().map(|(i, &p)| (key(p), i as u32)).collect();
    mesh.positions
        .iter()
        .map(|&p| *map.get(&key(p * Vec3::new(-1.0, 1.0, 1.0))).expect("mesh is not mirror-exact"))
        .collect()
}

/// Drags the pointer along `points` (pixels), a few samples per frame.
fn run_stroke(
    mesh: &mut Mesh,
    sculptor: &mut Sculptor,
    view: &View,
    points: &[Vec2],
) -> Option<StrokeDelta> {
    let sample = |i: usize| PointerSample {
        pos: points[i],
        pressure: None,
        time: i as f64 / 240.0,
    };
    assert!(sculptor.begin(mesh, view, sample(0), false, false), "stroke missed the mesh");
    for i in 1..points.len() {
        sculptor.sample(mesh, view, sample(i));
        if i % 4 == 0 {
            sculptor.frame(mesh, view, 1.0 / 60.0);
            let _ = mesh.take_dirty();
        }
    }
    let delta = sculptor.end(mesh, view);
    let _ = mesh.take_dirty();
    delta
}

fn line(from: Vec2, to: Vec2, n: usize) -> Vec<Vec2> {
    (0..=n).map(|i| from.lerp(to, i as f32 / n as f32)).collect()
}

/// Every brush that sculpts a plain mesh. Erase Layer does nothing to one:
/// it rubs out a sculpt layer, and its strokes (what they move, undo, the
/// mirror plane) are in `sculpt/erase/tests.rs`, on a mesh that has layers.
/// The brushes that move a plain mesh: not Erase Layer (it needs a sculpt
/// layer), nor Paint (it moves nothing: `sculpt/paint/tests.rs`).
fn brushes() -> impl Iterator<Item = Brush> {
    Brush::ALL.into_iter().filter(|&brush| !matches!(brush, Brush::EraseLayer | Brush::Paint))
}

fn moved_count(delta: &Option<StrokeDelta>) -> usize {
    match delta {
        Some(StrokeDelta::Positions { verts, .. }) | Some(StrokeDelta::Mask { verts, .. }) => {
            verts.len()
        }
        // These meshes have no sculpt layers.
        Some(StrokeDelta::Layered { .. } | StrokeDelta::Paint(_) | StrokeDelta::Painted { .. }) | None => 0,
    }
}

#[test]
fn sphere_is_closed_outward_and_mirror_exact() {
    let mesh = sphere(50_000);
    assert_eq!(mesh.triangle_count(), 2 * mesh.vertex_count() - 4, "closed genus-0 surface");
    // Every edge is used exactly twice, once in each direction.
    let mut edges: HashMap<(u32, u32), i32> = HashMap::new();
    for t in mesh.indices.chunks_exact(3) {
        for k in 0..3 {
            *edges.entry((t[k], t[(k + 1) % 3])).or_default() += 1;
        }
    }
    for (&(a, b), &n) in &edges {
        assert_eq!(n, 1, "directed edge used once");
        assert_eq!(edges.get(&(b, a)), Some(&1), "edge has an opposite twin");
    }
    for (p, n) in mesh.positions.iter().zip(&mesh.normals) {
        assert!((p.length() - 0.5).abs() < 1e-5);
        assert!(n.dot(p.normalize()) > 0.99, "normals point outward");
    }
    let twins = twins(&mesh);
    for (v, &t) in twins.iter().enumerate() {
        let n = mesh.normals[v] * Vec3::new(-1.0, 1.0, 1.0);
        assert!(mesh.normals[t as usize].distance(n) < 1e-6, "normals mirror each other");
    }
    for v in 0..mesh.vertex_count() as u32 {
        assert!(mesh.neighbours(v).len() >= 3);
        assert_eq!(mesh.neighbours(v).len(), mesh.vert_tris(v).len());
    }
}

#[test]
fn raycast_hits_the_front_of_the_sphere() {
    let mesh = sphere(50_000);
    let view = view_of(&mesh);
    let (o, d) = view.ray(SIZE * 0.5);
    let hit = mesh.raycast(o, d).expect("centre ray hits");
    assert!((hit.pos - Vec3::new(0.0, 0.0, 0.5)).length() < 1e-3, "{:?}", hit.pos);
    assert!(hit.normal.dot(Vec3::Z) > 0.99);
    assert!(mesh.raycast(view.ray(Vec2::new(5.0, 5.0)).0, view.ray(Vec2::new(5.0, 5.0)).1).is_none());
    // Projecting the hit lands back on the pixel it came from.
    assert!(view.project(hit.pos).unwrap().distance(SIZE * 0.5) < 0.01);
}

#[test]
fn every_brush_moves_the_surface_and_undo_restores_it_exactly() {
    for brush in brushes() {
        let mut mesh = sphere(80_000);
        let view = view_of(&mesh);
        let before = mesh.positions.clone();
        let normals_before = mesh.normals.clone();
        if brush == Brush::Smooth || brush == Brush::Flatten {
            // These need something to work on: roughen the surface first.
            let mut s = Sculptor::default();
            s.settings.brush = Brush::Inflate;
            s.settings.symmetry = [false; 3];
            s.settings.radius_px = 30.0;
            s.settings.per_brush[Brush::Inflate.index()].strength = 1.0;
            run_stroke(&mut mesh, &mut s, &view, &line(SIZE * 0.5 - 60.0, SIZE * 0.5 + 60.0, 40));
        }
        let rough = mesh.positions.clone();

        let mut sculptor = Sculptor::default();
        sculptor.settings.brush = brush;
        sculptor.settings.symmetry = [false; 3];
        sculptor.settings.radius_px = 50.0;
        sculptor.settings.per_brush[brush.index()].strength = 0.5;
        let path = line(SIZE * 0.5 + Vec2::new(-80.0, 10.0), SIZE * 0.5 + Vec2::new(80.0, -20.0), 60);
        let delta = run_stroke(&mut mesh, &mut sculptor, &view, &path);
        let moved = moved_count(&delta);
        assert!(moved > 200, "{brush:?} moved only {moved} vertices");

        let max_move = mesh
            .positions
            .iter()
            .zip(&rough)
            .map(|(a, b)| a.distance(*b))
            .fold(0.0f32, f32::max);
        assert!(max_move > 1e-4, "{brush:?} max move {max_move}");
        assert!(max_move < 0.6, "{brush:?} exploded: {max_move}");
        assert!(mesh.positions.iter().all(|p| p.is_finite()), "{brush:?} produced NaN");
        assert!(mesh.normals.iter().all(|n| (n.length() - 1.0).abs() < 1e-3), "{brush:?} normals");

        // The back of the sphere is out of reach of a brush on the front.
        for (p, b) in mesh.positions.iter().zip(&rough) {
            if b.z < -0.2 {
                assert_eq!(p, b, "{brush:?} touched the far side");
            }
        }

        // Spatial index still encloses the surface, so picking keeps working.
        let bounds = mesh.bounds();
        for p in &mesh.positions {
            assert!(p.cmpge(bounds.min).all() && p.cmple(bounds.max).all());
        }
        let (o, d) = view.ray(SIZE * 0.5);
        assert!(mesh.raycast(o, d).is_some());

        let mut history = History::default();
        history.push_stroke(delta.unwrap());
        assert_eq!(history.undo(&mut mesh), Some(false));
        assert!(mesh.positions == rough, "{brush:?} undo did not restore positions");
        if brush != Brush::Smooth && brush != Brush::Flatten {
            assert!(mesh.positions == before);
            let _ = mesh.take_dirty();
            assert!(mesh.normals == normals_before, "{brush:?} undo did not restore normals");
        }
        assert_eq!(history.redo(&mut mesh), Some(false));
        assert!(mesh.positions != rough);
    }
}

#[test]
fn mirrored_strokes_stay_mirror_exact_across_the_plane() {
    for brush in brushes() {
        let mut mesh = sphere(80_000);
        let twins = twins(&mesh);
        let view = view_of(&mesh);
        let mut sculptor = Sculptor::default();
        sculptor.settings.brush = brush;
        sculptor.settings.symmetry = [true, false, false];
        sculptor.settings.radius_px = 45.0;
        sculptor.settings.per_brush[brush.index()].strength = 0.6;
        // Starts well to one side, crosses the mirror plane at an angle and
        // runs along it: the hard case for symmetry.
        let c = SIZE * 0.5;
        let mut path = line(c + Vec2::new(-110.0, 60.0), c + Vec2::new(6.0, 10.0), 40);
        path.extend(line(c + Vec2::new(6.0, 10.0), c + Vec2::new(14.0, -90.0), 40));
        let delta = run_stroke(&mut mesh, &mut sculptor, &view, &path);
        assert!(moved_count(&delta) > 200, "{brush:?}");

        let flip = Vec3::new(-1.0, 1.0, 1.0);
        let mut worst = 0.0f32;
        let mut biggest = 0.0f32;
        for (v, &t) in twins.iter().enumerate() {
            worst = worst.max(mesh.positions[t as usize].distance(mesh.positions[v] * flip));
            biggest = biggest.max((mesh.positions[v].length() - 0.5).abs());
        }
        assert!(biggest > 1e-4, "{brush:?} did nothing");
        // Relative to how far the stroke moved the surface.
        let drift = worst / biggest;
        println!("{brush:?}: displacement {biggest:.5}, mirror error {worst:.2e} ({:.3}%)", drift * 100.0);
        assert!(drift < 0.01, "{brush:?} drifted {:.2}% off symmetry", drift * 100.0);
    }
}

#[test]
fn mask_protects_and_mask_strokes_undo() {
    let mut mesh = sphere(80_000);
    let view = view_of(&mesh);
    let mut sculptor = Sculptor::default();
    sculptor.settings.symmetry = [false; 3];
    sculptor.settings.radius_px = 60.0;
    sculptor.settings.mask_mode = true;
    sculptor.settings.per_brush[sculptor.settings.brush.index()].strength = 1.0;
    let path = line(SIZE * 0.5 - 40.0, SIZE * 0.5 + 40.0, 60);
    let delta = run_stroke(&mut mesh, &mut sculptor, &view, &path);
    assert!(matches!(delta, Some(StrokeDelta::Mask { .. })));
    let masked = mesh.mask.iter().filter(|&&m| m > 0.99).count();
    assert!(masked > 200, "only {masked} fully masked");

    sculptor.settings.mask_mode = false;
    sculptor.settings.brush = Brush::Inflate;
    sculptor.settings.per_brush[Brush::Inflate.index()].strength = 1.0;
    let before = mesh.positions.clone();
    run_stroke(&mut mesh, &mut sculptor, &view, &path);
    let mut free_moved = 0;
    for v in 0..mesh.vertex_count() {
        if mesh.mask[v] > 0.999 {
            assert_eq!(mesh.positions[v], before[v], "fully masked vertex moved");
        } else if mesh.positions[v] != before[v] {
            free_moved += 1;
        }
    }
    assert!(free_moved > 50);

    let mut history = History::default();
    history.push_stroke(delta.unwrap());
    history.undo(&mut mesh);
    assert!(mesh.mask.iter().all(|&m| m == 0.0));
}

/// Asserts the surface is closed and consistently oriented: every edge is
/// used as often in one direction as the other, so there are no holes or
/// cracks. Returns how many edges are shared by more than two faces (where
/// two sheets of surface pinch together inside one voxel).
fn assert_closed(mesh: &Mesh, what: &str) -> usize {
    let mut edges: HashMap<(u32, u32), i32> = HashMap::new();
    for t in mesh.indices.chunks_exact(3) {
        for k in 0..3 {
            *edges.entry((t[k], t[(k + 1) % 3])).or_default() += 1;
        }
    }
    let mut open = 0;
    let mut pinched = 0;
    for (&(a, b), &n) in &edges {
        if edges.get(&(b, a)) != Some(&n) {
            open += 1;
        } else if n > 1 {
            pinched += 1;
        }
    }
    assert_eq!(open, 0, "{what}: {open} of {} edges are open or mis-oriented", edges.len());
    pinched
}

fn volume(mesh: &Mesh) -> f64 {
    mesh.indices
        .chunks_exact(3)
        .map(|t| {
            let p = [0, 1, 2].map(|c| mesh.positions[t[c] as usize].as_dvec3());
            p[0].dot(p[1].cross(p[2])) / 6.0
        })
        .sum()
}

#[test]
fn voxel_remesh_of_a_sphere_is_watertight_accurate_and_symmetric() {
    let source = sphere(60_000);
    let true_volume = 4.0 / 3.0 * std::f64::consts::PI * 0.125;
    for resolution in [16, 48, 128] {
        let mesh = crate::remesh::voxel_remesh(&source, resolution, [true, false, false]).unwrap();
        assert_eq!(assert_closed(&mesh, &format!("remesh {resolution}")), 0, "a sphere has no pinches");
        let h = 1.0 / resolution as f32;
        for p in &mesh.positions {
            assert!((p.length() - 0.5).abs() < h * 0.75, "vertex off the surface: {}", p.length());
        }
        let v = volume(&mesh);
        assert!(v > 0.0, "faces point outward");
        let error = (v - true_volume).abs() / true_volume;
        println!(
            "remesh {resolution}: {} tris, volume error {:.2}%",
            mesh.triangle_count(),
            error * 100.0
        );
        assert!(error < 0.6 / resolution as f64 + 0.01, "volume error {error}");
        // Mirror-exact, so symmetric sculpting can carry straight on.
        let _ = twins(&mesh);
    }
}

#[test]
fn voxel_remesh_keeps_sculpted_detail_and_fuses_overlaps() {
    let mut mesh = sphere(120_000);
    let view = view_of(&mesh);
    let mut sculptor = Sculptor::default();
    sculptor.settings.brush = Brush::SnakeHook;
    sculptor.settings.symmetry = [true, false, false];
    sculptor.settings.radius_px = 70.0;
    // Pull a horn out past the silhouette, with its mirror twin.
    let c = SIZE * 0.5;
    run_stroke(&mut mesh, &mut sculptor, &view, &line(c + Vec2::new(-90.0, -60.0), c + Vec2::new(-330.0, -260.0), 80));
    let before = volume(&mesh);
    let extent = mesh.bounds().size();
    assert!(extent.x > 1.2, "the horns widened the model: {extent}");

    let remeshed = crate::remesh::voxel_remesh(&mesh, 160, [true, false, false]).unwrap();
    let pinched = assert_closed(&remeshed, "remeshed horns");
    assert!(pinched < remeshed.triangle_count() / 10_000, "{pinched} pinched edges");
    let after = volume(&remeshed);
    assert!((after - before).abs() / before < 0.02, "volume {before} -> {after}");
    assert!((remeshed.bounds().size() - extent).length() < 0.03);
    let _ = twins(&remeshed);

    // And the result is a normal sculptable mesh.
    let mut mesh = remeshed;
    let view = view_of(&mesh);
    let mut sculptor = Sculptor::default();
    let delta = run_stroke(&mut mesh, &mut sculptor, &view, &line(c - 30.0, c + 30.0, 30));
    assert!(moved_count(&delta) > 50);
}

#[test]
fn obj_and_stl_round_trip() {
    let mesh = sphere(5_000);
    let dir = std::env::temp_dir().join("sculpt_io_test");
    std::fs::create_dir_all(&dir).unwrap();
    for name in ["a.obj", "a.stl"] {
        let path = dir.join(name);
        if name.ends_with("obj") {
            crate::io::export_obj(&mesh, glam::Mat4::IDENTITY, &path).unwrap();
        } else {
            crate::io::export_stl(&mesh, glam::Mat4::IDENTITY, &path).unwrap();
        }
        let raw = crate::io::import(&path).unwrap();
        assert_eq!(raw.positions.len(), mesh.vertex_count(), "{name} welds back to the same count");
        assert_eq!(raw.indices.len(), mesh.indices.len());
        let back = Mesh::new(raw.positions, raw.indices, None);
        assert_eq!(back.triangle_count(), mesh.triangle_count());
        assert!((back.bounds().size() - mesh.bounds().size()).length() < 1e-4);
    }
}

#[test]
fn every_starting_shape_is_closed_outward_and_the_right_size() {
    let pi = std::f64::consts::PI;
    let expected = [
        (Primitive::Cube, 1.0),
        (Primitive::Sphere, pi / 6.0),
        // Unity's proportions: 1 across and 2 tall, the capsule's domes included.
        (Primitive::Cylinder, pi * 0.25 * 2.0),
        (Primitive::Capsule, pi * 0.25 * 1.0 + pi * 0.125 * 4.0 / 3.0),
        (Primitive::Cone, pi / 12.0),
        (Primitive::Wedge, 0.5),
    ];
    for (kind, want) in expected {
        let mesh = primitives::build(kind, 20_000, 0.5);
        assert_eq!(assert_closed(&mesh, kind.name()), 0, "{} pinches", kind.name());
        // Bending the lattice must not collapse any triangle.
        let full = primitives::triangle_count(kind, 20_000);
        assert_eq!(mesh.triangle_count(), full, "{} lost triangles", kind.name());
        let got = volume(&mesh);
        assert!((got - want).abs() < want * 0.01, "{}: volume {got}, expected {want}", kind.name());
        let thinnest = mesh
            .indices
            .chunks_exact(3)
            .map(|t| {
                let p = [0, 1, 2].map(|c| mesh.positions[t[c] as usize]);
                (p[1] - p[0]).cross(p[2] - p[0]).length()
            })
            .fold(f32::INFINITY, f32::min);
        assert!(thinnest > 1e-7, "{} has a sliver of area {thinnest}", kind.name());
    }
}

#[test]
fn scene_objects_clone_instance_reorder_and_undo() {
    use crate::history::Next;
    use crate::scene::Scene;

    let mut mesh = sphere(2_000);
    let sphere_verts = mesh.vertex_count();
    let mut scene = Scene::new("Sphere");
    let mut history = History::default();
    history.target = scene.active_mesh();
    let first = scene.active_id();

    // Adding selects the new object and brings its mesh to hand.
    let before = scene.state();
    let cube = scene.add("Cube", primitives::build(Primitive::Cube, 500, 0.5), &mut mesh);
    history.push_scene(before);
    assert_eq!(scene.active_id(), cube);
    assert_ne!(mesh.vertex_count(), sphere_verts);
    let cube_verts = mesh.vertex_count();

    // An instance shares the mesh; a clone does not.
    let instance = scene.instance_active();
    assert_eq!(scene.get(instance).unwrap().mesh, scene.get(cube).unwrap().mesh);
    assert_eq!(scene.get(instance).unwrap().name, "Cube Instance");
    assert_eq!(scene.users(scene.active_mesh()), 2);
    let copy = scene.clone_active(&mut mesh);
    assert_eq!(scene.get(copy).unwrap().name, "Cube Instance Copy");
    assert_eq!(scene.users(scene.active_mesh()), 1);
    let order = |s: &Scene| s.objects.iter().map(|o| o.id).collect::<Vec<_>>();
    assert_eq!(order(&scene), [first, cube, instance, copy]);

    // Names stay unique.
    assert_eq!(scene.unique_name("Cube"), "Cube (2)");

    // Reordering.
    assert!(scene.move_to(copy, 0));
    assert_eq!(order(&scene), [copy, first, cube, instance]);
    assert!(!scene.move_to(copy, 0));

    // Selecting swaps the meshes over.
    scene.select(first, &mut mesh);
    assert_eq!(mesh.vertex_count(), sphere_verts);
    scene.select(instance, &mut mesh);
    assert_eq!(mesh.vertex_count(), cube_verts);

    // Make Unique splits an instance off.
    let shared = scene.active_mesh();
    assert!(scene.make_unique(&mut mesh));
    assert_ne!(scene.active_mesh(), shared);
    assert_eq!(scene.users(shared), 1);
    assert!(!scene.make_unique(&mut mesh));

    // Deleting the active object selects a neighbour; the last one stays.
    let before = scene.state();
    assert!(scene.remove(instance, &mut mesh));
    history.push_scene(before);
    assert_ne!(scene.active_id(), instance);
    assert_eq!(scene.objects.len(), 3);

    // Undo brings the deleted object back, selected, with its mesh.
    assert_eq!(history.next_undo(), Some(Next::Scene));
    assert!(history.step_scene(true, &mut scene, &mut mesh));
    assert_eq!(scene.active_id(), instance);
    assert_eq!(mesh.vertex_count(), cube_verts);
    // The redo step holds the list without it, so nothing is collected yet.
    scene.collect(|id| history.keeps_mesh(id));
    assert!(history.step_scene(true, &mut scene, &mut mesh));
    assert_eq!(order(&scene), [first]);
    assert_eq!(mesh.vertex_count(), sphere_verts);
    assert!(history.step_scene(false, &mut scene, &mut mesh));
    assert_eq!(scene.active_id(), instance);
    assert_eq!(mesh.vertex_count(), cube_verts);

    // A ray picks the nearest shown object, in world space.
    scene.select(first, &mut mesh);
    scene.get_mut(cube).unwrap().position = Vec3::new(3.0, 0.0, 0.0);
    let hit = scene.pick(Vec3::new(3.0, 0.0, 5.0), Vec3::NEG_Z, &mesh).unwrap();
    assert_eq!(hit.id, cube);
    assert!((hit.pos - Vec3::new(3.0, 0.0, 0.5)).length() < 1e-4);
    scene.get_mut(cube).unwrap().visible = false;
    assert!(scene.pick(Vec3::new(3.0, 0.0, 5.0), Vec3::NEG_Z, &mesh).is_none());
    // The clones still sit at the centre, around the sphere.
    for id in [copy, instance] {
        scene.get_mut(id).unwrap().visible = false;
    }
    assert_eq!(scene.pick(Vec3::new(0.0, 0.0, 5.0), Vec3::NEG_Z, &mesh).unwrap().id, first);
}

#[test]
fn scene_file_round_trip_keeps_objects_and_shared_meshes() {
    use crate::io::{SavedObject, load_scene, save_scene};

    let (ball, block) = (sphere(2_000), primitives::build(Primitive::Cube, 500, 0.5));
    let object = |name: &str, mesh, x| SavedObject {
        name: name.into(),
        visible: x != 1.0,
        mesh,
        position: Vec3::new(x, 0.0, 0.0),
        rotation: glam::Quat::from_rotation_y(x),
        scale: Vec3::new(1.0, 1.0 + x, 1.0),
    };
    let objects = [object("Ball", 0, 0.0), object("Block", 1, 1.0), object("Ball Instance", 0, 2.0)];
    let dir = std::env::temp_dir().join("sculpt_io_test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("scene.sclp");
    save_scene(&[&ball, &block], &objects, 2, &path).unwrap();
    let back = load_scene(&path).unwrap();
    assert_eq!(back.meshes.len(), 2);
    for (a, b) in back.meshes.iter().zip([&ball, &block]) {
        assert_eq!((a.vertex_count(), a.triangle_count()), (b.vertex_count(), b.triangle_count()));
        assert!((volume(a) - volume(b)).abs() < 1e-9);
    }
    assert_eq!(back.active, 2);
    for (a, b) in back.objects.iter().zip(&objects) {
        assert_eq!((&a.name, a.visible, a.mesh, a.position), (&b.name, b.visible, b.mesh, b.position));
        assert!(a.rotation.angle_between(b.rotation) < 1e-4 && a.scale == b.scale);
    }

    // A file cut short is refused, not half-loaded.
    let bytes = std::fs::read(&path).unwrap();
    std::fs::write(&path, &bytes[..bytes.len() - 5]).unwrap();
    assert!(load_scene(&path).is_err());

    // Files from before scenes existed still open, as one object.
    let old = dir.join("old.sclp");
    crate::io::save_native(&ball, &old).unwrap();
    let back = load_scene(&old).unwrap();
    assert_eq!((back.meshes.len(), back.objects.len()), (1, 1));
    assert_eq!(back.objects[0].name, "old");
}

#[test]
fn turned_and_scaled_objects_pick_draw_and_sculpt_where_they_are_seen() {
    use glam::Quat;

    let mut mesh = primitives::build(Primitive::Cube, 2_000, 0.5);
    let mut scene = Scene::new("Cube");
    let object = scene.active_mut();
    object.position = Vec3::new(0.4, -0.2, 0.3);
    object.rotation = Quat::from_rotation_z(0.6) * Quat::from_rotation_y(0.5);
    object.scale = Vec3::new(1.0, 1.6, 0.5);
    let model = scene.active().matrix();

    let mut camera = Camera::default();
    camera.frame(scene.bounds(&mesh), SIZE.x / SIZE.y, false);
    (camera.yaw, camera.pitch) = (25.0, 15.0);
    for ortho in [false, true] {
        camera.ortho = ortho;
        let view = camera.view(Vec2::ZERO, SIZE, camera.distance, 1.0);
        let local = view.local_to(model);
        let px = SIZE * 0.5 + Vec2::new(30.0, -20.0);

        // The world ray and the object-space ray meet the surface at the
        // same place, and that place is under the pixel.
        let (origin, dir) = view.ray(px);
        let picked = scene.pick(origin, dir, &mesh).expect("the cube is under the pixel");
        let (o, d) = local.ray(px);
        let hit = mesh.raycast(o, d).expect("and under the same pixel in its own space");
        assert!((model.transform_point3(hit.pos) - picked.pos).length() < 1e-4);
        assert!(view.project(picked.pos).unwrap().distance(px) < 0.5);
        assert!(local.project(hit.pos).unwrap().distance(px) < 0.5);
        assert!(picked.normal.dot(dir) < 0.0, "the picked normal faces the eye");
    }

    // A brush dab lands under the pointer on the turned object.
    camera.ortho = false;
    let view = camera.view(Vec2::ZERO, SIZE, camera.distance, 1.0);
    let local = view.local_to(model);
    let before = mesh.positions.clone();
    let mut sculptor = Sculptor::default();
    sculptor.settings.radius_px = 40.0;
    let at = |i: usize| PointerSample { pos: SIZE * 0.5 + Vec2::new(i as f32 * 2.0, 0.0), pressure: None, time: i as f64 / 60.0 };
    assert!(sculptor.begin(&mut mesh, &local, at(0), false, false));
    for i in 1..20 {
        sculptor.sample(&mut mesh, &local, at(i));
    }
    sculptor.end(&mut mesh, &local);
    let moved: Vec<Vec3> = (0..before.len()).filter(|&v| mesh.positions[v] != before[v]).map(|v| before[v]).collect();
    assert!(!moved.is_empty());
    for p in moved {
        let px = view.project(model.transform_point3(p)).unwrap();
        assert!(px.distance(SIZE * 0.5 + Vec2::new(19.0, 0.0)) < 120.0, "moved a vertex far from the stroke: {px}");
    }
}

#[test]
fn gizmo_handles_move_turn_and_scale_the_object() {
    use crate::gizmo::{Drag, Frame, Handle, Placement};
    use glam::Quat;

    let mesh = sphere(2_000);
    let mut scene = Scene::new("Ball");
    scene.active_mut().rotation = Quat::from_rotation_y(0.3);
    let mut camera = Camera::default();
    camera.frame(scene.bounds(&mesh), SIZE.x / SIZE.y, false);
    (camera.yaw, camera.pitch) = (25.0, 15.0);
    let view = camera.view(Vec2::ZERO, SIZE, camera.distance, 1.0);
    let object = scene.active().clone();
    let frame = Frame::of(&object, 0.87, &view, 1.0).unwrap();
    let at = |p: Vec3| view.project(p).unwrap();
    let axis = |i: usize| frame.axis(i);

    // Each handle is picked where it is drawn.
    for i in 0..3 {
        assert_eq!(frame.pick(&view, at(frame.pivot + axis(i) * frame.arm * 1.1), 1.0), Some(Handle::Move(i)));
        assert_eq!(frame.pick(&view, at(frame.pivot + axis(i) * frame.arm * 0.82), 1.0), Some(Handle::Scale(i)));
        let on_ring = (axis((i + 1) % 3) + axis((i + 2) % 3)).normalize() * frame.arm * 0.63;
        assert_eq!(frame.pick(&view, at(frame.pivot + on_ring), 1.0), Some(Handle::Rotate(i)));
    }
    assert_eq!(frame.pick(&view, at(frame.pivot), 1.0), Some(Handle::Uniform));
    assert_eq!(frame.pick(&view, Vec2::new(5.0, 5.0), 1.0), None);

    // Move: the grabbed point follows the pointer along the axis.
    let grab = frame.pivot + axis(0) * frame.arm;
    let mut drag = Drag::begin(Handle::Move(0), frame, &object, &view, at(grab));
    let moved = drag.update(&view, at(grab + axis(0) * 0.4));
    assert!((moved.position - (object.position + axis(0) * 0.4)).length() < 1e-3);
    assert_eq!((moved.rotation, moved.scale), (object.rotation, object.scale));
    // ...and dragging back returns exactly to the start.
    assert_eq!(drag.update(&view, at(grab)).position, object.position);

    // Rotate: dragging a quarter of the way round the ring turns a quarter.
    for i in 0..3 {
        let (u, v) = (axis((i + 1) % 3) * frame.arm * 0.63, axis((i + 2) % 3) * frame.arm * 0.63);
        let mut drag = Drag::begin(Handle::Rotate(i), frame, &object, &view, at(frame.pivot + u));
        let mut turned = Placement::of(&object);
        for k in 1..=30 {
            let a = k as f32 / 30.0 * std::f32::consts::FRAC_PI_2;
            turned = drag.update(&view, at(frame.pivot + u * a.cos() + v * a.sin()));
        }
        let want = Quat::from_axis_angle(axis(i), std::f32::consts::FRAC_PI_2) * object.rotation;
        assert!(turned.rotation.angle_between(want) < 0.02, "ring {i} turned to {:?}", turned.rotation);
        assert_eq!(turned.position, object.position);
    }

    // Scale: pulling the cube to twice its distance doubles that axis only.
    let grab = frame.pivot + axis(1) * frame.arm * 0.82;
    let mut drag = Drag::begin(Handle::Scale(1), frame, &object, &view, at(grab));
    let scaled = drag.update(&view, at(frame.pivot + axis(1) * frame.arm * 1.64));
    assert!((scaled.scale - Vec3::new(1.0, 2.0, 1.0)).length() < 0.01, "{:?}", scaled.scale);

    // The centre cube: up grows every axis alike, down shrinks.
    let centre = at(frame.pivot);
    let mut drag = Drag::begin(Handle::Uniform, frame, &object, &view, centre);
    let bigger = drag.update(&view, centre - Vec2::new(0.0, 50.0)).scale;
    assert!(bigger.x > 1.1 && bigger.x == bigger.y && bigger.y == bigger.z);
    assert!(drag.update(&view, centre + Vec2::new(0.0, 200.0)).scale.x < 1.0);

    // A ring seen edge-on still turns, by dragging across it.
    camera.yaw = 0.0;
    camera.pitch = 0.0;
    let view = camera.view(Vec2::ZERO, SIZE, camera.distance, 1.0);
    let mut flat = object.clone();
    flat.rotation = Quat::IDENTITY;
    let frame = Frame::of(&flat, 0.87, &view, 1.0).unwrap();
    let near = view.project(frame.pivot + Vec3::Z * frame.arm * 0.63).unwrap();
    let mut drag = Drag::begin(Handle::Rotate(1), frame, &flat, &view, near);
    let turned = drag.update(&view, near + Vec2::new(60.0, 0.0));
    let (turn_axis, angle) = turned.rotation.to_axis_angle();
    assert!(angle > 0.1 && turn_axis.y > 0.99, "{turn_axis} {angle}");
}

/// A ray must never slip between two triangles. If it did, picking would
/// return the far side of the model, and a stroke would start there with the
/// wrong brush size.
#[test]
fn raycast_never_leaks_through_the_surface() {
    let mesh = sphere(1_000_000);
    let view = view_of(&mesh);
    let centre = SIZE * 0.5;
    let mid_depth = view.depth_of(Vec3::ZERO);
    let mut leaks = 0;
    let mut cast = |px: Vec2| {
        let (o, d) = view.ray(px);
        // Every pixel tested is well inside the silhouette, so the ray must
        // hit, and must hit the half of the sphere facing the camera.
        if !mesh.raycast(o, d).is_some_and(|h| view.depth_of(h.pos) < mid_depth) {
            leaks += 1;
        }
    };
    // Along the mesh's own edge lines: the sphere has edges exactly on the
    // coordinate planes, which these rows of pixels look straight down.
    for i in 0..=4000 {
        let t = i as f32 / 4000.0 * 360.0 - 180.0;
        cast(centre + Vec2::new(t, 0.0));
        cast(centre + Vec2::new(0.0, t));
    }
    // And a dense scatter of ordinary rays.
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut random = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 40) as f32 / (1u64 << 24) as f32 - 0.5
    };
    for _ in 0..400_000 {
        cast(centre + Vec2::new(random(), random()) * 260.0);
    }
    assert_eq!(leaks, 0, "{leaks} rays passed through the front of the sphere");
}

#[test]
fn the_monitor_to_open_on_is_read_with_its_number_and_the_file_after_it_still_opens() {
    let read = |args: &[&str]| crate::window_options(&args.iter().map(|a| a.to_string()).collect::<Vec<_>>());
    assert_eq!(read(&["--monitor", "2"]).monitor, Some(2));
    let both = read(&["--monitor", "2", "D:/Sculpts/head.sclp"]);
    assert_eq!((both.monitor, both.open), (Some(2), Some(std::path::PathBuf::from("D:/Sculpts/head.sclp"))));
    assert_eq!(read(&["--monitor", "two"]).monitor, None, "a monitor that is not a number is none");
    assert_eq!(read(&[]).monitor, None);
}
