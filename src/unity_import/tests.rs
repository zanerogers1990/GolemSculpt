//! Tests of the Unity sculpt reader, on files written by `fixture` (which
//! follows the Unity app's writer byte for byte) and, on request, on real
//! sculpts (`samples`).
//!
//! What the Unity app showed is worked out here from the fixture's own
//! numbers with the Unity app's arithmetic (its transform rule, its camera,
//! its OBJ and STL exporters), never with `axes`, so a wrong mapping cannot
//! agree with itself.

mod fixture;
mod samples;

use std::path::Path;

use glam::{Mat4, Vec2};

use super::*;
use crate::camera::Camera;
use crate::mesh::primitives::{self, Primitive};
use fixture::{Array, Era, Instance, Object, Sculpt};

/// Where an object's local point is in this app's world.
fn placed(o: &SavedObject, local: Vec3) -> Vec3 {
    Mat4::from_scale_rotation_translation(o.scale, o.rotation, o.position).transform_point3(local)
}

/// Every vertex of row `row` of an opened scene, in the world.
fn world_points(scene: &SavedScene, row: usize) -> Vec<Vec3> {
    let object = &scene.objects[row];
    scene.meshes[object.mesh].positions.iter().map(|&p| placed(object, p)).collect()
}

/// Where the Unity app drew every vertex of a fixture object placed by
/// `place`, carried into this app by the one rule the two apps share: what
/// Unity wrote to an OBJ as (-x, y, z), this app reads as (-x, y, z).
fn unity_points(object: &Object, place: impl Fn(Vec3) -> Vec3) -> Vec<Vec3> {
    object.vertices.iter().map(|&v| place(v)).map(|w| Vec3::new(-w.x, w.y, w.z)).collect()
}

/// Whether two sets of points are the same set, to within `tolerance`.
/// (Building a mesh reorders its vertices, so they cannot be compared in order.)
fn same_points(a: &[Vec3], b: &[Vec3], tolerance: f32) -> bool {
    a.len() == b.len()
        && a.iter().all(|p| b.iter().any(|q| p.distance(*q) <= tolerance))
        && b.iter().all(|q| a.iter().any(|p| p.distance(*q) <= tolerance))
}

/// Six times the volume a mesh encloses: positive when its faces look outward.
fn six_volumes(mesh: &Mesh) -> f64 {
    mesh.indices
        .chunks_exact(3)
        .map(|t| {
            let p = |i: u32| mesh.positions[i as usize].as_dvec3();
            p(t[0]).dot(p(t[1]).cross(p(t[2])))
        })
        .sum()
}

/// Edges with a triangle on one side only. None on a closed surface.
fn boundary_edges(indices: &[u32]) -> usize {
    let mut edges: Vec<(u32, u32)> = indices
        .chunks_exact(3)
        .flat_map(|t| [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])])
        .map(|(a, b)| (a.min(b), a.max(b)))
        .collect();
    edges.sort_unstable();
    edges.chunk_by(|a, b| a == b).filter(|run| run.len() == 1).count()
}

/// A mesh as something two builds of it can be compared by. Building a mesh
/// puts its vertices and triangles in an order of its own, so they are told
/// apart by where they are: each vertex as its position and mask, each
/// triangle as its corners' positions (turned so the least comes first,
/// which keeps the side it faces) and whether it is hidden; both sorted.
#[derive(PartialEq)]
struct Shape {
    vertices: Vec<([u32; 3], u32)>,
    triangles: Vec<([[u32; 3]; 3], bool)>,
}

fn shape(mesh: &Mesh) -> Shape {
    let bits = |v: u32| mesh.positions[v as usize].to_array().map(|c| (c + 0.0).to_bits());
    let mut vertices: Vec<([u32; 3], u32)> =
        (0..mesh.vertex_count() as u32).map(|v| (bits(v), mesh.mask[v as usize].to_bits())).collect();
    let mut triangles: Vec<([[u32; 3]; 3], bool)> = mesh
        .indices
        .par_chunks_exact(3)
        .enumerate()
        .map(|(t, corners)| {
            let mut corners = [bits(corners[0]), bits(corners[1]), bits(corners[2])];
            let least = (0..3).min_by_key(|&k| corners[k]).unwrap();
            corners.rotate_left(least);
            (corners, mesh.is_triangle_hidden(t as u32))
        })
        .collect();
    vertices.par_sort_unstable();
    triangles.par_sort_unstable();
    Shape { vertices, triangles }
}

/// The settings a file's details hold, as the app takes them out: surface
/// and background, the look (as text, having no equality of its own), the
/// view, and the symmetry; then each row's live mirror.
type Settings = (Shading, String, Option<SavedView>, Mirror, Vec<Option<sculpt_symmetry::MirrorRepeater>>);

fn settings(details: &Details, rows: usize) -> Settings {
    let look: Look = details.get_over(io::slot::LOOK, &Look::default()).expect("a look");
    (
        details.get_over(io::slot::SHADING, &Shading::default()).expect("shading"),
        serde_json::to_string(&look).unwrap(),
        details.get(io::slot::CAMERA),
        details.get_over(io::slot::MIRROR, &Mirror::default()).expect("symmetry"),
        (0..rows).map(|row| crate::scene::saved_mirror(details, row)).collect(),
    )
}

fn open(sculpt: &Sculpt, name: &str) -> Imported {
    load(&sculpt.write(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn sphere(name: &str, triangles: usize) -> Object {
    let mesh = primitives::build(Primitive::Sphere, triangles, 0.5);
    Object::new(name, mesh.positions.clone(), mesh.indices.iter().map(|&i| i as i32).collect())
}

fn marker(name: &str, at: Vec3) -> Object {
    let (vertices, triangles) = fixture::marker(at);
    Object::new(name, vertices, triangles)
}

fn cuboid(name: &str, lo: Vec3, hi: Vec3) -> Object {
    let (vertices, triangles) = fixture::cuboid(lo, hi);
    Object::new(name, vertices, triangles)
}

// ---------------------------------------------------------------- both versions

#[test]
fn a_version_1_file_opens_whole() {
    let ball = sphere("Ball", 2_000).placed(Vec3::new(0.0, 1.0, 0.0), Quat::IDENTITY, Vec3::ONE);
    let sculpt = Sculpt::new(vec![ball.clone()]);
    assert_eq!(sculpt.version(), 1);
    let opened = open(&sculpt, "v1.sculpt");
    let scene = &opened.scene;
    assert_eq!(scene.objects.len(), 1);
    assert_eq!(scene.objects[0].name, "Ball");
    assert!(scene.objects[0].visible);
    assert_eq!(scene.active, 0);
    let mesh = &scene.meshes[0];
    assert_eq!(mesh.vertex_count(), ball.vertices.len());
    assert_eq!(mesh.triangle_count(), ball.triangles.len() / 3);
    assert!(!mesh.any_hidden() && mesh.mask.iter().all(|&m| m == 0.0) && mesh.polys.is_none());
    assert_eq!(boundary_edges(&mesh.indices), 0, "a closed ball stays closed");
    assert!(same_points(&world_points(scene, 0), &unity_points(&ball, |v| ball.world(v)), 1e-6));
    // And through the app's own door, which finds it by how the file begins.
    let through = io::load_scene(&sculpt.write("v1 by any other name.sclp")).unwrap();
    assert_eq!(through.objects[0].name, "Ball");
}

#[test]
fn a_file_from_the_first_builds_opens_too() {
    // Fewer fields per object, a retired linked-mirror pair, no view worth having.
    let mut sculpt = Sculpt::new(vec![sphere("SculptSphere", 800).placed(Vec3::Y, Quat::IDENTITY, Vec3::ONE)]);
    sculpt.era = Era::BeforeHiding;
    sculpt.camera.remove("orthographic");
    sculpt.environment.remove("lightingPreset").set("studioLightingEnabled", "true").set("lightingMode", "2");
    sculpt.brush.remove("dynamicTopology").remove("dynamicTopologyDetail");
    assert!(!sculpt.json().contains("hasHidden") && sculpt.json().contains("mirrorLinkTwin"));
    let opened = open(&sculpt, "first builds.sculpt");
    assert_eq!(opened.scene.objects[0].name, "SculptSphere");
    assert_eq!(opened.scene.objects[0].position, Vec3::Y);
    let mirror: Mirror = opened.scene.details.get(io::slot::MIRROR).unwrap();
    assert_eq!(mirror.radial, Radial::default(), "a file from before radial symmetry leaves it as the app starts it");
    assert!(opened.extras.objects[0].array.is_none());
    let look: Look = opened.scene.details.get(io::slot::LOOK).unwrap();
    assert_eq!(look.lighting, 0, "no preset named: the default rig");
    let view: SavedView = opened.scene.details.get(io::slot::CAMERA).unwrap();
    assert!(!view.ortho);
    // No dynamic topology in the file reads as Unity's default: off, at 0.15.
    let kept: SavedDynamicTopology = opened.scene.details.get(slot::DYNAMIC_TOPOLOGY).unwrap();
    assert_eq!(kept, SavedDynamicTopology { enabled: false, detail: dyntopo::DEFAULT_DETAIL });
}

#[test]
fn a_version_2_file_keeps_its_hidden_faces() {
    let mut ball = sphere("Ball", 3_000);
    // Hide the cap toward Unity's +X, which is this app's -X.
    let hidden: Vec<bool> =
        ball.triangles.chunks_exact(3).map(|t| t.iter().all(|&i| ball.vertices[i as usize].x > 0.2)).collect();
    let hidden_count = hidden.iter().filter(|h| **h).count();
    assert!(hidden_count > 50 && hidden_count < hidden.len() / 2);
    ball.hidden = Some(hidden);
    let sculpt = Sculpt::new(vec![ball]);
    assert_eq!(sculpt.version(), 2, "a hidden block makes the file version 2");
    let opened = open(&sculpt, "v2.sculpt");
    let mesh = &opened.scene.meshes[0];
    assert_eq!(mesh.hidden_triangle_count(), hidden_count);
    for (t, corners) in mesh.indices.chunks_exact(3).enumerate() {
        let in_cap = corners.iter().all(|&i| mesh.positions[i as usize].x < -0.2);
        assert_eq!(mesh.is_triangle_hidden(t as u32), in_cap, "triangle {t}");
    }
}

// ---------------------------------------------------------------- each kind of content

#[test]
fn a_mask_stays_on_the_vertices_it_was_painted_on() {
    let mut ball = sphere("Ball", 3_000);
    // Soft from Unity's +X side to its -X side, so every vertex has its own value.
    ball.mask = Some(ball.vertices.iter().map(|v| (v.x + 0.5).clamp(0.0, 1.0)).collect());
    let opened = open(&Sculpt::new(vec![ball.clone()]), "mask.sculpt");
    let mesh = &opened.scene.meshes[0];
    for (p, &m) in mesh.positions.iter().zip(&mesh.mask) {
        assert!((m - (0.5 - p.x).clamp(0.0, 1.0)).abs() < 1e-6, "mask {m} at {p}");
    }
    let total = |mask: &[f32]| mask.iter().map(|&m| m as f64).sum::<f64>();
    assert!((total(&mesh.mask) - total(ball.mask.as_ref().unwrap())).abs() < 1e-3);

    // A mask with nothing painted is not written at all, and reads as none.
    let mut bare = sphere("Bare", 500);
    bare.mask = Some(vec![0.0; bare.vertices.len()]);
    let sculpt = Sculpt::new(vec![bare]);
    assert!(sculpt.json().contains("\"hasMask\":false"));
    assert!(open(&sculpt, "no mask.sculpt").scene.meshes[0].mask.iter().all(|&m| m == 0.0));
}

#[test]
fn a_quad_remeshed_object_arrives_as_its_triangles() {
    // The Unity app never wrote its quads to a `.sculpt` (it kept them only
    // while it ran, for the wireframe and the OBJ export), so there are none
    // to bring: a flat quad sheet, cut in two per quad as Unity held it,
    // arrives as exactly those triangles, with no polygons claimed.
    let n = 6usize;
    let vertices: Vec<Vec3> = (0..=n).flat_map(|j| (0..=n).map(move |i| Vec3::new(i as f32, j as f32, 0.0))).collect();
    let at = |i: usize, j: usize| (j * (n + 1) + i) as i32;
    let mut triangles = Vec::new();
    for j in 0..n {
        for i in 0..n {
            let q = [at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1)];
            triangles.extend_from_slice(&[q[0], q[1], q[2], q[0], q[2], q[3]]);
        }
    }
    let opened = open(&Sculpt::new(vec![Object::new("Sheet", vertices, triangles)]), "quads.sculpt");
    let mesh = &opened.scene.meshes[0];
    assert!(mesh.polys.is_none());
    assert_eq!((mesh.vertex_count(), mesh.triangle_count()), ((n + 1) * (n + 1), n * n * 2));
    assert_eq!(boundary_edges(&mesh.indices), 4 * n, "the sheet's rim, and nothing torn inside it");
}

#[test]
fn instances_stay_linked_and_sit_where_they_were() {
    let mut tooth = marker("Tooth", Vec3::ZERO).placed(
        Vec3::new(1.0, 0.5, -2.0),
        Quat::from_rotation_y(0.7),
        Vec3::new(1.0, 2.0, 0.5),
    );
    tooth.instances = vec![
        Instance {
            name: "Tooth Instance".into(),
            position: Vec3::new(-3.0, 0.0, 1.0),
            rotation: Quat::from_euler(glam::EulerRot::YXZ, 1.1, -0.4, 0.3),
            scale: Vec3::new(0.5, 0.5, 2.0),
            visible: true,
        },
        Instance {
            name: String::new(),
            position: Vec3::new(4.0, 4.0, 4.0),
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
            visible: false,
        },
    ];
    let other = cuboid("Jaw", Vec3::splat(-1.0), Vec3::ONE);
    let opened = open(&Sculpt::new(vec![tooth.clone(), other]), "instances.sculpt");
    let scene = &opened.scene;
    let names: Vec<&str> = scene.objects.iter().map(|o| o.name.as_str()).collect();
    // Each instance directly under its object, a nameless one named after it.
    assert_eq!(names, ["Tooth", "Tooth Instance", "Tooth Instance (2)", "Jaw"]);
    assert_eq!(scene.meshes.len(), 2, "one mesh per object, however many places it is drawn in");
    assert_eq!(scene.objects.iter().map(|o| o.mesh).collect::<Vec<_>>(), [0, 0, 0, 1]);
    assert_eq!(scene.objects.iter().map(|o| o.visible).collect::<Vec<_>>(), [true, true, false, true]);
    for (row, instance) in tooth.instances.iter().enumerate() {
        let drawn = |v: Vec3| instance.position + instance.rotation * (instance.scale * v);
        assert!(same_points(&world_points(scene, row + 1), &unity_points(&tooth, drawn), 1e-5), "instance {row}");
    }
    assert!(same_points(&world_points(scene, 0), &unity_points(&tooth, |v| tooth.world(v)), 1e-5));
}

#[test]
fn several_objects_keep_their_order_names_visibility_and_selection() {
    let mut hidden = cuboid("Base", Vec3::splat(-1.0), Vec3::ONE);
    hidden.visible = false;
    let objects = vec![
        sphere("Head", 600).placed(Vec3::new(0.0, 2.0, 0.0), Quat::IDENTITY, Vec3::splat(2.0)),
        hidden,
        marker("Horn", Vec3::new(0.5, 0.0, 0.0)),
        // Two objects of one name, as the Unity app allowed.
        marker("Horn", Vec3::new(-0.5, 0.0, 0.0)),
    ];
    let mut sculpt = Sculpt::new(objects);
    sculpt.primary = 2;
    let opened = open(&sculpt, "several.sculpt");
    let scene = &opened.scene;
    let names: Vec<&str> = scene.objects.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(names, ["Head", "Base", "Horn", "Horn (2)"]);
    assert_eq!(scene.objects.iter().map(|o| o.visible).collect::<Vec<_>>(), [true, false, true, true]);
    assert_eq!(scene.active, 2);
    assert_eq!(scene.objects[0].scale, Vec3::splat(2.0));
    assert_eq!(scene.meshes.len(), 4);

    // No selection, or one past the end, is brought into the list as Unity brought it.
    for (saved, active) in [(-1, 0), (99, 3)] {
        sculpt.primary = saved;
        assert_eq!(open(&sculpt, "several unselected.sculpt").scene.active, active);
    }
}

// ---------------------------------------------------------------- the axis mapping

/// A scene that is nothing like its mirror image: a body at the centre and a
/// marker well off to Unity's +X, which is the viewer's LEFT from where the
/// Unity app's Z view looks.
fn lopsided() -> (Sculpt, Object, Object) {
    let body = cuboid("Body", Vec3::new(-1.0, -1.0, -1.0), Vec3::new(1.0, 1.5, 1.0));
    let side = marker("Marker", Vec3::ZERO).placed(
        Vec3::new(4.0, 0.5, 1.0),
        Quat::from_euler(glam::EulerRot::YXZ, 0.9, 0.35, -0.6),
        Vec3::new(1.0, 1.5, 0.75),
    );
    (Sculpt::new(vec![body.clone(), side.clone()]), body, side)
}

#[test]
fn a_marker_off_to_one_side_lands_on_the_mirrored_side_facing_outward() {
    let (sculpt, body, side) = lopsided();
    assert!(fixture::six_volumes(&side.vertices, &side.triangles) > 0.0, "the fixture faces outward by Unity's rule");
    let opened = open(&sculpt, "lopsided.sculpt");
    let scene = &opened.scene;
    // X is the axis that turns over, and only X.
    assert_eq!(scene.objects[1].position, Vec3::new(-4.0, 0.5, 1.0));
    assert!(same_points(&world_points(scene, 1), &unity_points(&side, |v| side.world(v)), 1e-5));
    assert!(same_points(&world_points(scene, 0), &unity_points(&body, |v| body.world(v)), 1e-5));
    for (mesh, source) in scene.meshes.iter().zip([&body, &side]) {
        // Turned over with the axis, so still the same solid seen from outside.
        let (now, then) = (six_volumes(mesh), fixture::six_volumes(&source.vertices, &source.triangles));
        assert!(now > 0.0 && (now - then).abs() < 1e-4 * then, "volume {now} against {then}");
        assert_eq!(boundary_edges(&mesh.indices), 0);
    }
    // The marker's arms: Unity's local +X arm now runs along -X, the others as they did.
    let m = &scene.meshes[1];
    let reach = |axis: usize| {
        m.positions.iter().map(|p| p[axis]).fold(0.0f32, |far, v| if v.abs() > far.abs() { v } else { far })
    };
    assert_eq!([reach(0), reach(1), reach(2)], [-1.0, 2.0, 3.0]);
}

/// Where the Unity app's camera drew a world point of its own, in
/// normalised screen coordinates (x right, y up, the picture's edges at 1),
/// by `CameraOrbitController.UpdateTransform`: the camera turned by
/// Euler(pitch, yaw, 0), which is yaw about Y after pitch about X, and set
/// back from the pivot along its own -Z; x to its right, y up, z ahead.
fn unity_screen(yaw: f32, pitch: f32, distance: f32, pivot: Vec3, ortho: bool, aspect: f32, point: Vec3) -> Vec2 {
    let turn = Quat::from_rotation_y(yaw.to_radians()) * Quat::from_rotation_x(pitch.to_radians());
    let tan = (60.0f32.to_radians() * 0.5).tan();
    // Orthographic parks the camera further back; only depth changes.
    let eye = pivot + turn * Vec3::new(0.0, 0.0, -distance);
    let seen = turn.inverse() * (point - eye);
    let half = if ortho { distance * tan } else { seen.z * tan };
    Vec2::new(seen.x / (half * aspect), seen.y / half)
}

/// The same for this app's camera, once it has taken a saved view.
fn app_screen(view: &SavedView, aspect: f32, point: Vec3) -> Vec2 {
    let mut camera = Camera::default();
    assert!(camera.restore_view(view));
    let size = Vec2::new(900.0 * aspect, 900.0);
    let pixel = camera.view(Vec2::ZERO, size, camera.distance, 10.0).project(point).expect("in front of the camera");
    Vec2::new(pixel.x / size.x * 2.0 - 1.0, 1.0 - pixel.y / size.y * 2.0)
}

#[test]
fn the_picture_from_the_saved_camera_is_the_unity_picture_not_its_mirror_image() {
    let (mut sculpt, body, side) = lopsided();
    let aspect = 16.0 / 10.0;
    let pivot = Vec3::new(1.0, 0.25, -0.5);
    // Views all round, some of them wound up many turns as Unity's yaw gets.
    for (yaw, pitch, ortho) in [
        (0.0, 0.0, false),
        (180.0, 0.0, false),
        (35.0, 20.0, false),
        (-8560.25, -19.5, true),
        (250.0, -60.0, false),
        (90.0, 45.0, true),
        (-90.0, 0.0, true),
    ] {
        sculpt.camera = fixture::view(yaw, pitch, 14.0, pivot, ortho);
        let opened = open(&sculpt, "picture.sculpt");
        let view: SavedView = opened.scene.details.get(io::slot::CAMERA).expect("the view came across");
        assert_eq!(view.ortho, ortho);
        for (row, source) in [(0, &body), (1, &side)] {
            // Every vertex, where each app draws it: the Unity app from the
            // file's own numbers, this app from what it opened.
            let unity: Vec<Vec2> = source
                .vertices
                .iter()
                .map(|&v| unity_screen(yaw, pitch, 14.0, pivot, ortho, aspect, source.world(v)))
                .collect();
            let here: Vec<Vec2> =
                world_points(&opened.scene, row).iter().map(|&p| app_screen(&view, aspect, p)).collect();
            let same = |a: &[Vec2], b: &[Vec2]| a.iter().all(|p| b.iter().any(|q| p.distance(*q) < 2e-4));
            assert!(
                same(&unity, &here) && same(&here, &unity),
                "yaw {yaw} pitch {pitch}, {}: Unity drew {unity:?}, this app {here:?}",
                source.name
            );
            // And the test can tell: the mirror image of Unity's picture is a different picture.
            let mirrored: Vec<Vec2> = unity.iter().map(|p| Vec2::new(-p.x, p.y)).collect();
            assert!(!same(&mirrored, &here), "yaw {yaw} pitch {pitch}: {} looks the same mirrored", source.name);
        }
    }
}

#[test]
fn the_view_gizmos_agree_on_front_top_and_the_bubble_on_the_right() {
    // Unity's view gizmo (ViewGizmoUIBuilder.AxisYaw / AxisPitch) against
    // this app's (ui/overlay.rs): the yaw and pitch each snaps to.
    let saved = |yaw: f32, pitch: f32| {
        view_of(&View { valid: true, yaw, pitch, distance: 5.0, pivot: Xyz::default(), orthographic: true }).unwrap()
    };
    // Unity's Z bubble puts the camera on +Z at yaw 180: this app's Front view, yaw 0.
    assert_eq!((saved(180.0, 0.0).yaw, saved(180.0, 0.0).pitch), (0.0, 0.0));
    // Seen from there, the bubble on the right of Unity's gizmo is its -X
    // one (yaw 90). It gives this app's Right view, yaw -90.
    assert_eq!(saved(90.0, 0.0).yaw, 270.0);
    // And the one on the left, Unity's +X (yaw -90), this app's Left view, yaw 90.
    assert_eq!(saved(-90.0, 0.0).yaw, 90.0);
    // Top from the front: Unity keeps the yaw and pitches to 90 (held just short of it, as here).
    assert_eq!((saved(180.0, 90.0).yaw, saved(180.0, 90.0).pitch), (0.0, 89.0));
    // Where Unity's camera starts (yaw 0, on -Z) is this app's Back view.
    assert_eq!(saved(0.0, 0.0).yaw, 180.0);
    // A file from before views were kept has none; the app frames the model.
    assert!(view_of(&View::default()).is_none());
    assert!(view_of(&View { valid: true, distance: f32::NAN, ..View::default() }).is_none());
}

/// What the Unity app's `ObjExporter.AppendObject` writes for an object:
/// every vertex through its transform with X negated, to six places, and
/// every face with its corners in reverse.
fn unity_obj(object: &Object) -> String {
    let mut text = String::from("# Exported from Sculpting Application\no SculptMesh\n");
    for &v in &object.vertices {
        let w = object.world(v);
        text += &format!("v {:.6} {:.6} {:.6}\n", -w.x, w.y, w.z);
    }
    for t in object.triangles.chunks_exact(3) {
        text += &format!("f {0}//{0} {1}//{1} {2}//{2}\n", t[2] + 1, t[1] + 1, t[0] + 1);
    }
    text
}

/// A triangle soup's faces as (centre, unit normal) pairs, which say where
/// every face is and which way it looks whatever the vertices are numbered.
fn faces(positions: &[Vec3], indices: &[u32], place: impl Fn(Vec3) -> Vec3) -> Vec<(Vec3, Vec3)> {
    indices
        .chunks_exact(3)
        .map(|t| {
            let [a, b, c] = [0, 1, 2].map(|k| place(positions[t[k] as usize]));
            ((a + b + c) / 3.0, (b - a).cross(c - a).normalize())
        })
        .collect()
}

fn same_faces(a: &[(Vec3, Vec3)], b: &[(Vec3, Vec3)], tolerance: f32) -> bool {
    let covers = |a: &[(Vec3, Vec3)], b: &[(Vec3, Vec3)]| {
        a.iter()
            .all(|(centre, normal)| b.iter().any(|(c, n)| centre.distance(*c) <= tolerance && normal.dot(*n) > 0.999))
    };
    a.len() == b.len() && covers(a, b) && covers(b, a)
}

#[test]
fn a_sculpt_lands_exactly_where_its_unity_obj_export_does() {
    let (sculpt, body, side) = lopsided();
    let opened = open(&sculpt, "obj route.sculpt");
    for (row, source) in [(0, &body), (1, &side)] {
        // The same object by the other road: out of Unity as an OBJ, into this app as an OBJ.
        let obj = fixture::write(&format!("obj route {row}.obj"), unity_obj(source).as_bytes());
        let through_obj = io::import(&obj).unwrap();
        let object = &opened.scene.objects[row];
        let mesh = &opened.scene.meshes[object.mesh];
        assert!(same_points(&world_points(&opened.scene, row), &through_obj.positions, 2e-6));
        let from_sculpt = faces(&mesh.positions, &mesh.indices, |p| placed(object, p));
        let from_obj = faces(&through_obj.positions, &through_obj.indices, |p| p);
        assert!(same_faces(&from_sculpt, &from_obj, 1e-5), "{}: the faces differ", source.name);
    }
}

#[test]
fn a_sculpt_prints_exactly_as_its_unity_stl_export_does() {
    let (sculpt, _, side) = lopsided();
    let opened = open(&sculpt, "stl route.sculpt");
    let object = &opened.scene.objects[1];
    let stl = fixture::write("stl route.stl", &[]);
    io::export_stl(
        &opened.scene.meshes[1],
        Mat4::from_scale_rotation_translation(object.scale, object.rotation, object.position),
        &stl,
    )
    .unwrap();
    let bytes = std::fs::read(&stl).unwrap();
    let float = |at: usize| f32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    let corner = |at: usize| Vec3::new(float(at), float(at + 4), float(at + 8));
    let written: Vec<Vec3> =
        (0..side.triangles.len() / 3).flat_map(|t| [12, 24, 36].map(|c| corner(84 + t * 50 + c))).collect();
    let soup: Vec<u32> = (0..written.len() as u32).collect();
    // `StlExporter`: a world point (x, y, z) goes out as (-x, -z, y) in
    // millimetres, ten to the unit, each triangle with its last two corners swapped.
    let unity: Vec<Vec3> = side
        .triangles
        .chunks_exact(3)
        .flat_map(|t| [t[0], t[2], t[1]])
        .map(|i| side.world(side.vertices[i as usize]))
        .map(|w| Vec3::new(-w.x, -w.z, w.y) * 10.0)
        .collect();
    assert!(same_faces(&faces(&written, &soup, |p| p), &faces(&unity, &soup, |p| p), 1e-4));
}

#[test]
fn rotations_go_through_the_mirror() {
    // A quarter turn about each axis, by what it does to the axes.
    let turned = |q: Quat, v: Vec3| axes::rotation(q) * axes::point(v);
    for q in [
        Quat::from_rotation_x(1.0),
        Quat::from_rotation_y(-0.7),
        Quat::from_rotation_z(2.2),
        Quat::from_euler(glam::EulerRot::ZXY, 0.3, 1.9, -1.2),
    ] {
        for v in [Vec3::X, Vec3::Y, Vec3::Z, Vec3::new(0.3, -2.0, 1.1)] {
            // Mirroring after turning is turning the mirrored point by the mirrored turn.
            assert!((axes::point(q * v) - turned(q, v)).length() < 1e-5);
        }
    }
    // About X the turn is unchanged; about Y and Z it runs the other way.
    assert_eq!(axes::rotation(Quat::from_rotation_x(0.5)), Quat::from_rotation_x(0.5));
    assert!(axes::rotation(Quat::from_rotation_y(0.5)).abs_diff_eq(Quat::from_rotation_y(-0.5), 1e-6));
    assert!(axes::rotation(Quat::from_rotation_z(0.5)).abs_diff_eq(Quat::from_rotation_z(-0.5), 1e-6));
    // A placement that makes no sense falls back, part by part.
    let (p, r, s) = placement(
        Xyz { x: f32::NAN, y: 0.0, z: 0.0 },
        Xyzw { x: 0.0, y: 0.0, z: 0.0, w: 0.0 },
        Xyz { x: 0.0, y: -2.0, z: 3.0 },
    );
    assert_eq!((p, r, s), (Vec3::ZERO, Quat::IDENTITY, Vec3::new(1.0, 2.0, 3.0)));
}

// ---------------------------------------------------------------- mirrors, symmetry, arrays

#[test]
fn a_live_mirror_stays_live_through_the_mirrored_centre() {
    let mut wing = marker("Wing", Vec3::ZERO).placed(Vec3::new(2.0, 0.0, 0.0), Quat::IDENTITY, Vec3::ONE);
    (wing.repeater_axes, wing.repeater_center) = (1 | 4, Vec3::new(0.5, 1.0, -2.0));
    let plain = cuboid("Plain", Vec3::ZERO, Vec3::ONE);
    let opened = open(&Sculpt::new(vec![wing, plain]), "mirror.sculpt");
    let details = &opened.scene.details;
    // Read back the way the app reads a `.sclp`'s.
    let mirror = crate::scene::saved_mirror(details, 0).expect("the wing is mirrored");
    assert_eq!((mirror.axis_mask, mirror.center), (5, Vec3::new(-0.5, 1.0, -2.0)));
    assert!(crate::scene::saved_mirror(details, 1).is_none());
}

#[test]
fn the_scene_takes_the_selected_objects_mirror_axes_and_radial_symmetry() {
    let mut face = sphere("Face", 300);
    (face.mirror, face.show_planes) = ([true, false, false], false);
    let mut body = sphere("Body", 300).placed(Vec3::new(1.0, 0.0, 0.0), Quat::IDENTITY, Vec3::ONE);
    (body.mirror, body.symmetry_space) = ([false, false, true], 1);
    // Six times round a slanted axis of the body's own.
    (body.radial, body.radial_count, body.radial_axis, body.radial_custom_axis) =
        (true, 6, 3, Vec3::new(3.0, 4.0, 0.0));
    let mut sculpt = Sculpt::new(vec![face, body]);
    sculpt.primary = 1;
    let opened = open(&sculpt, "symmetry.sculpt");
    // As the app reads a `.sclp`'s.
    let mirror: Mirror = opened.scene.details.get_over(io::slot::MIRROR, &Mirror::default()).unwrap();
    assert_eq!((mirror.x, mirror.y, mirror.z, mirror.guides), (false, false, true, true), "the selected object's");
    // A custom axis is a direction in the object's own space, so it goes
    // through the mirror with the mesh; how many times round does not change.
    let slanted = Vec3::new(-0.6, 0.8, 0.0);
    assert_eq!((mirror.radial.on, mirror.radial.count, mirror.radial.axis), (true, 6, RadialAxis::Custom));
    assert!(
        (mirror.radial.custom_axis - slanted).length() < 1e-6
            && (mirror.radial.axis_vector() - slanted).length() < 1e-6
    );

    // Every object's own is still there for the asking.
    let [face, body] = [opened.extras.objects[0].symmetry, opened.extras.objects[1].symmetry];
    assert_eq!((face.mirror, face.show_planes, face.space), ([true, false, false], false, SymmetrySpace::Local));
    assert_eq!(face.radial, Radial::default());
    assert_eq!((body.mirror, body.space, body.radial), ([false, false, true], SymmetrySpace::World, mirror.radial));
    let left = &opened.left_behind;
    assert_eq!(
        left.settings[..2],
        [
            "the world-space symmetry of \"Body\" (symmetry here goes with the object)".to_owned(),
            "each object's own symmetry (this app has one setting for the scene, taken from \"Body\")".to_owned(),
        ]
    );
    assert!(left.objects.is_empty(), "symmetry is the scene's, so nothing of an object's is lost: {left:?}");

    // The axes by number, and a count past the most there can be.
    for (number, axis) in [(0, RadialAxis::X), (1, RadialAxis::Y), (2, RadialAxis::Z), (7, RadialAxis::Y)] {
        (sculpt.objects[1].radial_axis, sculpt.objects[1].radial_count) = (number, 500);
        let mirror: Mirror = open(&sculpt, "symmetry axes.sculpt").scene.details.get(io::slot::MIRROR).unwrap();
        assert_eq!(
            (mirror.radial.axis, mirror.radial.count, mirror.radial.on),
            (axis, Radial::MAX_COUNT, true),
            "axis {number}"
        );
    }

    // Objects that are alike leave nothing to report, whatever a
    // switched-off radial symmetry would have done; nor does world space
    // on an object that sits square at the origin.
    let mut twin = sphere("Twin", 300);
    (twin.mirror, twin.radial_count, twin.radial_axis, twin.symmetry_space) = ([true, false, false], 12, 2, 1);
    let mut other = sphere("Other", 300);
    other.mirror = [true, false, false];
    let alike = open(&Sculpt::new(vec![twin, other]), "symmetry alike.sculpt");
    assert_eq!(alike.left_behind.settings, ["the Unity app's brush settings"]);
}

#[test]
fn the_copies_of_a_live_array_come_in_as_linked_copies_where_unity_drew_them() {
    // A run of four along a slanted line, each copy half as big again as the last.
    let mut link = marker("Link", Vec3::ZERO).placed(
        Vec3::new(1.0, 0.0, 0.5),
        Quat::from_rotation_z(0.4),
        Vec3::new(1.0, 0.5, 2.0),
    );
    let mut line = Array::line(4, Vec3::new(6.0, 3.0, 0.0));
    line.end_scale = 2.5;
    link.array = Some(line);
    (link.repeater_axes, link.repeater_center) = (1, Vec3::ZERO);
    // And three a third of a turn apart about Y, which goes the other way round in the mirror.
    let mut petal = marker("Petal", Vec3::ZERO).placed(Vec3::new(3.0, 0.0, 0.0), Quat::IDENTITY, Vec3::ONE);
    petal.array = Some(Array {
        kind: 3,
        count: 3,
        sweep: 240.0,
        points: vec![Vec3::ZERO, Vec3::new(-3.0, 0.0, 0.0)],
        ..Array::line(0, Vec3::ZERO)
    });
    let sculpt = Sculpt::new(vec![link.clone(), petal.clone()]);
    let opened = open(&sculpt, "arrays.sculpt");
    let scene = &opened.scene;
    let names: Vec<&str> = scene.objects.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(
        names,
        ["Link", "Link (Array 2)", "Link (Array 3)", "Link (Array 4)", "Petal", "Petal (Array 2)", "Petal (Array 3)"]
    );
    assert_eq!(scene.meshes.len(), 2, "the copies share their object's mesh");
    assert_eq!(scene.objects.iter().map(|o| o.mesh).collect::<Vec<_>>(), [0, 0, 0, 0, 1, 1, 1]);

    for (first_row, source) in [(0, &link), (4, &petal)] {
        // Where the Unity app drew copy k (ArrayRepeater.SyncViews): the
        // object's position plus the copy's offset, the copy's turn after the
        // object's, the object's scale times the copy's.
        let array = source.array.as_ref().unwrap();
        let settings = ArraySettings {
            kind: ArrayKind::from_id(array.kind),
            count: array.count,
            end_scale: array.end_scale,
            mid_scale: array.mid_scale,
            closed: array.closed,
            follow: array.follow,
            space_by_size: array.space_by_size,
            axis: array.axis,
            sweep: array.sweep,
        };
        let layout = sculpt_array::compute_layout(&settings, &array.points);
        assert_eq!(layout.len(), array.count as usize);
        for (k, copy) in layout.iter().enumerate() {
            let drawn = |v: Vec3| {
                source.position + copy.offset + (copy.rotation * source.rotation) * (source.scale * copy.scale * v)
            };
            assert!(
                same_points(&world_points(scene, first_row + k), &unity_points(source, drawn), 1e-4),
                "{} copy {k} is not where Unity drew it",
                source.name
            );
        }
    }
    // The petals really do go round: the second is nowhere near the first.
    assert!(scene.objects[5].position.distance(scene.objects[4].position) > 3.0);

    // The link's copies are mirrored with it, as Unity mirrored an array.
    for row in 0..4 {
        assert_eq!(crate::scene::saved_mirror(&scene.details, row).map(|m| m.axis_mask), Some(1), "row {row}");
    }
    assert!(crate::scene::saved_mirror(&scene.details, 4).is_none());

    // The arrays themselves are kept for the day a file here can hold one.
    let kept = opened.extras.objects[0].array.as_ref().unwrap();
    assert_eq!((kept.settings.kind, kept.settings.count, kept.rows.as_slice()), (ArrayKind::Line, 4, &[1, 2, 3][..]));
    assert_eq!(kept.points, [Vec3::ZERO, Vec3::new(-6.0, 3.0, 0.0)]);
    assert_eq!(opened.extras.objects[1].row, 4);
    let left = &opened.left_behind.objects;
    assert!(
        left.iter().any(|line| line.contains("\"Link\"") && line.contains("its 3 copies came in as linked copies")),
        "{left:?}"
    );

    // An array with too few points, or of a kind from a newer Unity build, is left off, as Unity left it.
    let mut odd = sculpt.clone();
    odd.objects[0].array.as_mut().unwrap().points.truncate(1);
    odd.objects[1].array.as_mut().unwrap().kind = 9;
    let opened = open(&odd, "odd arrays.sculpt");
    assert_eq!(opened.scene.objects.len(), 2);
    assert!(opened.left_behind.objects.iter().any(|line| line.contains("\"Petal\"")));
}

// ---------------------------------------------------------------- the look, the settings

#[test]
fn material_finish_lighting_and_effects_go_to_the_same_settings() {
    let mut sculpt = Sculpt::new(vec![sphere("Ball", 300)]);
    sculpt
        .material
        .set("baseColor", fixture::rgba([0.8, 0.3, 0.1, 1.0]))
        .set("metallic", fixture::float(0.25))
        .set("smoothness", fixture::float(0.7))
        .set("flatShading", "true")
        .set("screenCavityEnabled", "false")
        .set("cavityRidge", fixture::float(1.5))
        .set("matcapEnabled", "true")
        .set("matcapName", fixture::text("MatCap_BS6"))
        .set("matcapIntensity", fixture::float(1.75))
        .set("metalEnabled", "true")
        .set("metalPresetId", fixture::text("gothic_pewter"))
        .set("metalExposure", fixture::float(1.2753973))
        .set("metalPatternSeed", fixture::float(0.326))
        .set("metalReferenceSize", fixture::float(2.1012940))
        .set("clayPresetId", fixture::text("warm_grey"))
        .set("woodPresetId", fixture::text("oak"))
        .set("woodGrainAxis", "2")
        .set("glowEnabled", "true")
        .set("glowPresetId", fixture::text("veins"))
        .set("glowHueShift", fixture::float(0.4))
        .set("lurePresetId", fixture::text("a preset this app has never heard of"));
    sculpt
        .environment
        .set("lightingPreset", fixture::text("moody"))
        .set("lightingFivePoint", "true")
        .set("lightingBrightness", fixture::float(1.4))
        .set("lightingRotation", fixture::float(-35.0))
        .set("lightingFollowCamera", "false")
        .set("lightingWorldYaw", fixture::float(90.0))
        .set("lightingShadows", "false")
        .set("backgroundMode", "1")
        .set("backgroundColorA", fixture::rgba([0.2, 0.1, 0.0, 1.0]))
        .set("backgroundColorB", fixture::rgba([0.9, 0.8, 0.7, 1.0]))
        .set("gradientBias", fixture::float(2.0))
        .set("hdriEnabled", "true")
        .set("hdriPath", fixture::text("C:\\hdri\\studio.hdr"))
        .set("hdriRotation", fixture::float(120.0))
        .set("hdriExposure", fixture::float(1.5))
        .set("bloomEnabled", "false")
        .set("bloomIntensity", fixture::float(0.6))
        .set("dofEnabled", "true")
        .set("dofFocusDistance", fixture::float(3.5))
        .set("colorAdjustmentsEnabled", "true")
        .set("saturation", fixture::float(-20.0));
    let opened = open(&sculpt, "look.sculpt");
    let details = &opened.scene.details;

    let shading: Shading = details.get(io::slot::SHADING).unwrap();
    assert_eq!((shading.base_color, shading.smoothness), ([0.8, 0.3, 0.1], 0.7));
    assert!(shading.flat_shading && !shading.cavity && shading.cavity_ridge == 1.5);
    assert_eq!((shading.matcap_on, shading.matcap.as_str()), (true, "MatCap_BS6"));
    assert!(shading.gradient_background);
    assert_eq!((shading.background, shading.background_top), ([0.2, 0.1, 0.0], [0.9, 0.8, 0.7]));

    // As the app reads it out of a `.sclp`.
    let look: Look = details.get_over(io::slot::LOOK, &Look::default()).unwrap();
    assert_eq!((look.metallic, look.matcap_intensity), (0.25, 1.75));
    assert_eq!(look.finish, Finish::Metal);
    assert_eq!(look::METALS[look.metal.preset].id, "gothic_pewter");
    assert_eq!((look.metal.exposure, look.metal.seed, look.metal.reference), (1.2753973, 0.326, 2.1012940));
    assert_eq!(look::CLAYS[look.clay.preset].id, "warm_grey");
    assert_eq!((look::WOODS[look.wood.preset].id, look.wood.axis), ("oak", 2));
    assert!(look.glow.on && look::GLOWS[look.glow.preset].id == "veins" && look.glow.hue_shift == 0.4);
    assert_eq!(look.lure.preset, Look::default().lure.preset, "an unknown preset keeps the default");
    assert_eq!(look::LIGHTING[look.lighting].id, "moody");
    assert!(look.five_point && !look.shadows && !look.follow_camera);
    assert_eq!((look.brightness, look.rotation), (1.4, -35.0));
    // Unity's yaw 90 faces its +X, which is -X here.
    assert!((look.world_forward - Vec3::NEG_X).length() < 1e-6, "{}", look.world_forward);
    assert!(look.hdri.enabled && look.hdri.path.as_deref() == Some(Path::new("C:\\hdri\\studio.hdr")));
    assert_eq!(
        (look.hdri.rotation, look.hdri.exposure, look.hdri_background, look.gradient_bias),
        (120.0, 1.5, false, 2.0)
    );
    assert!(
        !look.bloom
            && look.bloom_intensity == 0.6
            && look.vignette
            && look.depth_of_field
            && look.focus_distance == 3.5
    );
    assert!(look.grading && look.saturation == -20.0);

    // The HDRI as the background; a matcap switched on with none named stays off, as in Unity.
    sculpt.environment.set("backgroundMode", "2");
    sculpt.material.set("matcapName", fixture::text(""));
    // A scene saved without effects wrote zeros for them: they are not taken.
    sculpt
        .environment
        .set("postAvailable", "false")
        .set("bloomIntensity", fixture::float(0.0))
        .set("vignetteEnabled", "false");
    let details = open(&sculpt, "look 2.sculpt").scene.details;
    let look: Look = details.get_over(io::slot::LOOK, &Look::default()).unwrap();
    let shading: Shading = details.get(io::slot::SHADING).unwrap();
    assert!(look.hdri_background && shading.gradient_background && !shading.matcap_on);
    assert!(look.bloom && look.bloom_intensity == 0.25 && look.vignette && !look.depth_of_field);
}

#[test]
fn every_light_falls_on_the_same_side_of_the_model() {
    // Where the Unity app put each light of a rig
    // (`LightingPresetController.UpdateOrientation`): a frame that is the
    // camera's rotation while the rig follows it, or a level one at the yaw
    // it was unlocked at, turned by Rotation about its own up; and in that
    // frame a light at an azimuth and elevation is Euler(elevation, -azimuth, 0)
    // applied to "back".
    let unity_light = |frame: Quat, rotation: f32, light: &look::PresetLight| {
        let turned = frame * Quat::from_rotation_y(rotation.to_radians());
        let aimed =
            Quat::from_rotation_y((-light.azimuth).to_radians()) * Quat::from_rotation_x(light.elevation.to_radians());
        turned * (aimed * Vec3::NEG_Z)
    };
    for (yaw, pitch, rotation, follow, world_yaw, preset) in [
        (0.0f32, 0.0f32, 0.0f32, true, 0.0f32, "studio"),
        (130.0, 25.0, 0.0, true, 0.0, "moody"),
        (-8560.25, -19.5, 70.0, true, 33.0, "dramatic"),
        (200.0, 40.0, 0.0, false, 0.0, "studio"),
        (20.0, -30.0, -110.0, false, 245.0, "golden"),
        (75.0, 10.0, 45.0, false, -90.0, "cinematic"),
    ] {
        let mut sculpt = Sculpt::new(vec![sphere("Ball", 300)]);
        sculpt.camera = fixture::view(yaw, pitch, 5.0, Vec3::ZERO, false);
        sculpt
            .environment
            .set("lightingPreset", fixture::text(preset))
            .set("lightingFivePoint", "true")
            .set("lightingRotation", fixture::float(rotation))
            .set("lightingFollowCamera", follow.to_string())
            .set("lightingWorldYaw", fixture::float(world_yaw));
        let details = open(&sculpt, "lights.sculpt").scene.details;
        let look: Look = details.get_over(io::slot::LOOK, &Look::default()).unwrap();
        let view: SavedView = details.get(io::slot::CAMERA).unwrap();
        // This app's camera with the opened view, as the renderer is told of it.
        let mut camera = Camera::default();
        assert!(camera.restore_view(&view));
        let seen = camera.view(Vec2::ZERO, Vec2::new(1600.0, 1000.0), camera.distance, 10.0);
        let lit = look.uniform(&look::Frame {
            cam_right: seen.right,
            cam_up: seen.up,
            cam_forward: seen.forward,
            ortho_half_height: None,
            eye_offset: 0.0,
            scene_min: Vec3::splat(-1.0),
            scene_max: Vec3::ONE,
            model_size: 1.0,
            time: 0.0,
            gradient_background: false,
        });
        let unity_camera = Quat::from_rotation_y(yaw.to_radians()) * Quat::from_rotation_x(pitch.to_radians());
        let frame = if follow { unity_camera } else { Quat::from_rotation_y(world_yaw.to_radians()) };
        for (i, light) in look.preset().lights.iter().enumerate() {
            let unity = unity_light(frame, rotation, light);
            // Through the same door as every point of the model.
            let wanted = Vec3::new(-unity.x, unity.y, unity.z);
            let here = Vec3::new(lit.light_dir[i][0], lit.light_dir[i][1], lit.light_dir[i][2]);
            assert!(
                (here - wanted).length() < 2e-4,
                "{preset}, light {i}, yaw {yaw} rotation {rotation} world yaw {world_yaw}: {here} here, {wanted} in Unity"
            );
        }
    }
}

#[test]
fn dynamic_topology_is_restored_as_unity_restored_it() {
    let mut sculpt = Sculpt::new(vec![sphere("Ball", 300)]);
    sculpt.brush.set("dynamicTopology", "true").set("dynamicTopologyDetail", fixture::float(0.16317758));
    let opened = open(&sculpt, "dyntopo.sculpt");
    let mut live = DynTopo::default();
    assert!(!live.enabled);
    restore_dynamic_topology(&opened.scene.details, &mut live);
    assert!(live.enabled && live.detail == 0.16317758);
    // The rest of the brush block is read, and kept for whoever wants it.
    let brush = &opened.extras.brush;
    assert_eq!((brush.current_brush, brush.remesh_resolution, brush.quad_remesh_target), (1, 128, 5000));
    assert_eq!((brush.brush_screen_radius, brush.per_brush_strength.len(), brush.pressure_curve), (60.0, 10, 1.6));

    // Out of range (a file edited by hand) comes back onto the slider.
    sculpt.brush.set("dynamicTopologyDetail", fixture::float(7.0));
    restore_dynamic_topology(&open(&sculpt, "dyntopo.sculpt").scene.details, &mut live);
    assert_eq!(live.detail, dyntopo::MAX_DETAIL);
    // A `.sclp`'s details have nothing to say, and leave the user's setting alone.
    live.detail = 0.2;
    restore_dynamic_topology(&Details::default(), &mut live);
    assert!(live.enabled && live.detail == 0.2);
}

#[test]
fn settings_that_do_not_read_cost_only_themselves() {
    let mut sculpt = Sculpt::new(vec![sphere("Ball", 300)]);
    sculpt.material.set("smoothness", fixture::text("very")).set("metallic", fixture::float(0.5));
    sculpt.environment.set("lightingPreset", "17").set("lightingBrightness", fixture::float(1.5));
    sculpt.camera.set("pivot", "null");
    sculpt.brush = fixture::Block::new(&[("perBrushStrength", "\"none\"".into())]);
    let opened = open(&sculpt, "odd settings.sculpt");
    let shading: Shading = opened.scene.details.get(io::slot::SHADING).unwrap();
    let look: Look = opened.scene.details.get(io::slot::LOOK).unwrap();
    assert_eq!((shading.smoothness, look.metallic, look.lighting, look.brightness), (0.4, 0.5, 0, 1.5));
    assert!(opened.scene.details.get::<SavedView>(io::slot::CAMERA).is_some());
    assert!(opened.extras.brush.per_brush_strength.is_empty());

    // A file with no settings at all (only objects) still opens, on the Unity app's own starting values.
    let json = format!(
        "{{\"objects\":[{}]}}",
        sculpt.json().split_once("\"objects\":[").unwrap().1.rsplit_once("],\"primary").unwrap().0
    );
    let bare = load(&fixture::write("bare.sculpt", &sculpt.bytes_with(1, &json))).unwrap();
    assert_eq!(bare.scene.objects.len(), 1);
    assert!(bare.scene.details.get::<SavedView>(io::slot::CAMERA).is_none(), "no view: the app frames the model");
}

// ---------------------------------------------------------------- the user is told, the file is safe

#[test]
fn the_status_line_says_what_it_is_and_what_was_left_behind() {
    let mut link = marker("Link", Vec3::ZERO);
    link.array = Some(Array::line(2, Vec3::X * 4.0));
    let opened = open(&Sculpt::new(vec![link]), "status.sculpt");
    let replaced = status_note(&opened.scene.details, true).unwrap();
    assert_eq!(
        replaced,
        "It is a sculpt from the Unity app: saving makes a .sclp and leaves that file as it is. Left behind: the live array \
         on \"Link\" (its copy came in as a linked copy, without the array's handles); the Unity app's brush settings."
    );
    // Added to a scene, only what its objects lost matters: no settings were ever going to be taken.
    let added = status_note(&opened.scene.details, false).unwrap();
    assert_eq!(
        added,
        "Left behind: the live array on \"Link\" (its copy came in as a linked copy, without the array's handles)."
    );
    let plain = open(&Sculpt::new(vec![marker("Plain", Vec3::ZERO)]), "status plain.sculpt");
    assert!(status_note(&plain.scene.details, false).is_none());
    assert!(status_note(&Details::default(), true).is_none(), "nothing to say about a .sclp");
}

#[test]
fn a_save_never_lands_on_a_unity_sculpt() {
    let sculpt = Sculpt::new(vec![marker("Keep", Vec3::ZERO)]);
    let path = sculpt.write("precious.sculpt");
    let before = std::fs::read(&path).unwrap();
    let opened = load(&path).unwrap();
    let meshes: Vec<&Mesh> = opened.scene.meshes.iter().collect();
    for target in [path.clone(), path.with_extension("SCULPT")] {
        let refused =
            io::save_scene_with(&meshes, &opened.scene.objects, 0, &opened.scene.details, &target).unwrap_err();
        assert!(refused.contains("Unity") && refused.contains(".sclp"), "{refused}");
    }
    assert_eq!(std::fs::read(&path).unwrap(), before, "the Unity file is untouched");
    assert!(!path.with_extension("sculpt.tmp").exists());
    // What a save offers instead: the same name, as this app's own file.
    assert_eq!(save_name(&path), "precious.sclp");
    assert!(
        is_unity(Path::new("C:\\work\\Head.SCULPT"))
            && !is_unity(Path::new("head.sclp"))
            && !is_unity(Path::new("sculpt"))
    );
}

#[test]
fn opened_then_saved_as_sclp_then_reopened_is_the_same_scene() {
    let mut ball =
        sphere("Ball", 1_500).placed(Vec3::new(1.0, 2.0, 3.0), Quat::from_rotation_y(0.5), Vec3::new(1.0, 2.0, 0.5));
    ball.mask = Some(ball.vertices.iter().map(|v| (v.y * 2.0).clamp(0.0, 1.0)).collect());
    ball.hidden =
        Some(ball.triangles.chunks_exact(3).map(|t| t.iter().all(|&i| ball.vertices[i as usize].z > 0.1)).collect());
    (ball.repeater_axes, ball.repeater_center) = (1, Vec3::new(0.25, 0.0, 0.0));
    ball.instances = vec![Instance {
        name: "Ball Instance".into(),
        position: Vec3::X * 5.0,
        rotation: Quat::from_rotation_x(1.0),
        scale: Vec3::splat(0.5),
        visible: false,
    }];
    let mut link = marker("Link", Vec3::ZERO);
    link.array = Some(Array::line(3, Vec3::Y * 4.0));
    let mut sculpt = Sculpt::new(vec![ball, link]);
    sculpt.primary = 1;
    sculpt.camera = fixture::view(-400.0, 25.0, 7.5, Vec3::new(0.5, 1.0, -1.0), true);
    sculpt.material.set("clayEnabled", "true").set("clayPresetId", fixture::text("red_oil"));
    sculpt.environment.set("lightingPreset", fixture::text("golden")).set("backgroundMode", "1");
    let opened = open(&sculpt, "round trip.sculpt").scene;

    let saved_to = fixture::write("round trip.sclp", &[]);
    let meshes: Vec<&Mesh> = opened.meshes.iter().collect();
    io::save_scene_with(&meshes, &opened.objects, opened.active, &opened.details, &saved_to).unwrap();
    let again = io::load_scene(&saved_to).unwrap();

    assert_eq!(again.active, opened.active);
    assert_eq!(again.objects.len(), opened.objects.len());
    for (a, b) in again.objects.iter().zip(&opened.objects) {
        assert_eq!(
            (&a.name, a.visible, a.mesh, a.position, a.rotation, a.scale),
            (&b.name, b.visible, b.mesh, b.position, b.rotation, b.scale)
        );
    }
    assert_eq!(again.meshes.len(), opened.meshes.len());
    for (a, b) in again.meshes.iter().zip(&opened.meshes) {
        // To the bit: every vertex with its mask, every triangle with the
        // side it faces and whether it is hidden.
        assert!(shape(a) == shape(b), "a mesh changed on the way through the .sclp");
        assert_eq!(
            (a.vertex_count(), a.triangle_count(), a.hidden_triangle_count()),
            (b.vertex_count(), b.triangle_count(), b.hidden_triangle_count())
        );
    }
    assert!(again.meshes[0].any_hidden() && again.meshes[0].mask.iter().any(|&m| m > 0.0));
    let rows = opened.objects.len();
    let (kept, wanted) = (settings(&again.details, rows), settings(&opened.details, rows));
    assert_eq!(kept, wanted);
    // And they are the file's, not the app's own starting ones.
    let (shading, look, view, mirror, mirrors) = kept;
    assert!(
        shading.gradient_background
            && look.contains("\"finish\":\"Clay\"")
            && look.contains("red_oil")
            && look.contains("golden")
    );
    assert_eq!(view.map(|v| (v.yaw, v.pitch, v.distance, v.ortho)), Some((140.0, 25.0, 7.5, true)));
    assert_eq!((mirror.x, mirror.radial.on), (false, false));
    assert!(mirrors[0].is_some() && mirrors[1].is_none() && mirrors[2].is_none());
}

// ---------------------------------------------------------------- failures

/// Loading must fail, with a message a person can read that names the file.
fn refused(bytes: &[u8], name: &str) -> String {
    let path = fixture::write(name, bytes);
    let through_the_app = io::load_scene(&path).err().unwrap_or_else(|| panic!("{name} opened"));
    if bytes.starts_with(MAGIC) {
        assert_eq!(load(&path).err().as_deref(), Some(through_the_app.as_str()));
    }
    assert!(through_the_app.contains(name), "{through_the_app}");
    through_the_app
}

fn whole() -> Sculpt {
    let mut ball = sphere("Ball", 400);
    ball.mask = Some(ball.vertices.iter().map(|v| (v.x + 0.5).clamp(0.0, 1.0)).collect());
    ball.hidden = Some(ball.triangles.chunks_exact(3).enumerate().map(|(t, _)| t % 5 == 0).collect());
    Sculpt::new(vec![ball, marker("Marker", Vec3::X)])
}

#[test]
fn a_file_that_is_not_a_unity_sculpt_is_refused() {
    let mut bytes = whole().bytes();
    bytes[..8].copy_from_slice(b"SCLPTSX\0");
    let path = fixture::write("wrong magic.sculpt", &bytes);
    assert_eq!(load(&path).err().unwrap(), format!("{} is not a sculpt from the Unity app.", path.display()));
    assert!(io::load_scene(&path).is_err());
    for (name, bytes) in [
        ("empty.sculpt", &b""[..]),
        ("short.sculpt", &b"SCLP"[..]),
        ("text.sculpt", &b"not a sculpt at all, just some words"[..]),
    ] {
        let path = fixture::write(name, bytes);
        assert!(load(&path).is_err() && io::load_scene(&path).is_err(), "{name}");
    }
    assert!(load(Path::new("no such folder/no such file.sculpt")).err().unwrap().starts_with("Could not read"));
}

#[test]
fn a_file_cut_short_anywhere_is_refused_whole() {
    let sculpt = whole();
    let bytes = sculpt.bytes();
    let json = sculpt.json().len();
    let (verts, corners) = (sculpt.objects[0].vertices.len(), sculpt.objects[0].triangles.len());
    let first = 16 + json;
    // In the header, in the JSON, and in every array of the first object; then the second object's.
    let cuts = [
        9,
        12,
        15,
        16,
        16 + json / 2,
        first,
        first + 7,
        first + verts * 12,
        first + verts * 12 + 5,
        first + verts * 24,
        first + verts * 24 + corners * 2,
        first + verts * 24 + corners * 4,
        first + verts * 28 + corners * 4 - 1,
        first + verts * 28 + corners * 4,
        first + verts * 28 + corners * 4 + corners / 3,
        bytes.len() - 13,
        bytes.len() - 1,
    ];
    for cut in cuts {
        assert!(cut < bytes.len());
        let message = refused(&bytes[..cut], &format!("cut at {cut}.sculpt"));
        assert!(message.contains("is damaged"), "cut at {cut}: {message}");
    }
    // Whole, it opens; and bytes after the last array do no harm, as in Unity.
    assert!(load(&sculpt.write("whole.sculpt")).is_ok());
    let mut longer = bytes.clone();
    longer.extend_from_slice(&[7u8; 100]);
    assert!(load(&fixture::write("longer.sculpt", &longer)).is_ok());
}

#[test]
fn a_file_from_a_newer_unity_format_is_refused_by_name() {
    let sculpt = whole();
    for version in [3, 4, 1000] {
        let message = refused(&sculpt.bytes_with(version, &sculpt.json()), &format!("version {version}.sculpt"));
        assert!(
            message.contains("newer version of the Unity app") && message.contains(&format!("file format {version}")),
            "{message}"
        );
    }
    // Both versions it knows open, and a version no file ever had is damage.
    for version in [1, 2] {
        // (A version 1 stamp on this file would be a lie: it has a hidden block.)
        let mut plain = sculpt.clone();
        plain.objects[0].hidden = None;
        let file = if version == 1 { plain } else { sculpt.clone() };
        assert_eq!(file.version(), version);
        assert!(load(&file.write("known version.sculpt")).is_ok());
    }
    for version in [0, -1] {
        assert!(
            refused(&sculpt.bytes_with(version, &sculpt.json()), &format!("version {version}.sculpt"))
                .contains("is damaged")
        );
    }
}

#[test]
fn a_file_whose_parts_do_not_add_up_is_refused() {
    let sculpt = whole();
    let good = sculpt.json();
    let damaged = |json: &str, name: &str| {
        let message = refused(&sculpt.bytes_with(sculpt.version(), json), name);
        assert!(message.contains("is damaged"), "{name}: {message}");
    };
    let verts = sculpt.objects[0].vertices.len();
    let count = format!("\"vertexCount\":{verts}");
    assert!(good.contains(&count));
    // Counts that ask for more than the file has, or for nothing sensible.
    damaged(&good.replacen(&count, "\"vertexCount\":2000000000", 1), "huge count.sculpt");
    damaged(&good.replacen(&count, "\"vertexCount\":9000000000000000000", 1), "absurd count.sculpt");
    damaged(&good.replacen(&count, "\"vertexCount\":-5", 1), "negative count.sculpt");
    damaged(&good.replacen(&count, "\"vertexCount\":\"many\"", 1), "worded count.sculpt");
    damaged(&good.replacen(&count, "\"vertices\":12", 1), "missing count.sculpt");
    let corners = format!("\"triangleIndexCount\":{}", sculpt.objects[0].triangles.len());
    damaged(
        &good.replacen(&corners, &format!("\"triangleIndexCount\":{}", sculpt.objects[0].triangles.len() - 1), 1),
        "ragged triangles.sculpt",
    );
    damaged(&good.replacen("\"hasMask\":true", "\"hasMask\":7", 1), "odd flag.sculpt");
    // A description that is not one.
    damaged("{\"objects\":", "half json.sculpt");
    damaged("[1,2,3]", "list json.sculpt");
    damaged("{\"objects\":7}", "objects not a list.sculpt");
    damaged("{\"objects\":[3]}", "object not an object.sculpt");

    // A JSON length that points past the end, or nowhere.
    let mut bytes = sculpt.bytes();
    for length in [i32::MAX, -1, 0, bytes.len() as i32] {
        bytes[12..16].copy_from_slice(&length.to_le_bytes());
        assert!(refused(&bytes, "json length.sculpt").contains("is damaged"));
    }

    // A corner that names no vertex, and a coordinate that is not a number: Unity refused both.
    let mut wild = sculpt.clone();
    wild.objects[1].triangles[4] = 40;
    assert!(refused(&wild.bytes(), "wild corner.sculpt").contains("is damaged"));
    wild.objects[1].triangles[4] = -1;
    assert!(refused(&wild.bytes(), "negative corner.sculpt").contains("is damaged"));
    let mut hole = sculpt.clone();
    hole.objects[1].vertices[2].y = f32::NAN;
    assert!(refused(&hole.bytes(), "not a number.sculpt").contains("is damaged"));

    // Nothing in it at all.
    let empty = Sculpt::new(Vec::new());
    assert!(refused(&empty.bytes(), "no objects.sculpt").contains("has no objects"));
}
