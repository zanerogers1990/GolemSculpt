//! Paint following the model through the routines the app calls to rebuild
//! it, the same ones face groups go through (`app/groups.rs`).

use glam::Vec3;
use sculpt_volume::{BooleanOp, BooleanSettings};

use super::follow;
use crate::app::{cut_mirror, selection};
use crate::history::History;
use crate::mesh::Mesh;
use crate::mesh::groups::Fit;
use crate::mesh::paint::{Channels, Mat8, PaintLayer, PaintRow, PaintStack, Rgba8, Texel};
use crate::mesh::primitives::{self, Primitive};
use crate::scene::{Others, Scene};
use crate::{decimate, remesh};

/// A smooth colour over space, so a carried value can be checked against where it lands.
fn colour_at(p: Vec3) -> [u8; 3] {
    let b = |x: f32| (128.0 + 150.0 * x).round().clamp(0.0, 255.0) as u8;
    [b(p.x), b(p.y), b(p.z)]
}

/// `mesh` with one layer: the colour of where each vertex is (in `place`'s
/// space), and material 4 above the middle, 9 below.
fn paint(mut mesh: Mesh, name: &str, place: impl Fn(Vec3) -> Vec3) -> Mesh {
    let p: Vec<Vec3> = mesh.positions.iter().map(|&p| place(p)).collect();
    let data = Channels {
        colour: Some(p.iter().map(|&p| {
            let c = colour_at(p);
            Rgba8([c[0], c[1], c[2], 255])
        }).collect()),
        material: Some(p.iter().map(|p| if p.y > 0.0 { Mat8([4, 255]) } else { Mat8([9, 255]) }).collect()),
        ..Channels::default()
    };
    let stack = PaintStack {
        layers: vec![PaintLayer { row: PaintRow::new(1, name.into()), data }],
        active: Some(1),
        next_id: 2,
        table: vec![4, 9],
        finest: Vec::new(),
        len: mesh.vertex_count(),
    };
    mesh.set_paint(Some(stack));
    mesh
}

fn ball(tris: usize) -> Mesh {
    paint(primitives::build(Primitive::Sphere, tris, 0.5), "Colour", |p| p)
}

/// The texel of layer `k` at every vertex.
fn texels(mesh: &Mesh, k: usize) -> Vec<Texel> {
    let stack = mesh.paint_stack().expect("the mesh has paint");
    (0..mesh.vertex_count()).map(|v| stack.layers[k].data.texel(v)).collect()
}

/// The mean and worst difference between each vertex's colour and the
/// colour of where it is, over the vertices `which` picks.
fn error(mesh: &Mesh, place: impl Fn(Vec3) -> Vec3, which: impl Fn(Vec3) -> bool) -> (f32, u8, usize) {
    let all = texels(mesh, 0);
    let (mut sum, mut worst, mut count) = (0.0, 0u8, 0);
    for (v, t) in all.iter().enumerate() {
        let p = mesh.positions[v];
        if !which(p) {
            continue;
        }
        let want = colour_at(place(p));
        for k in 0..3 {
            let d = t.colour.0[k].abs_diff(want[k]);
            sum += d as f32;
            worst = worst.max(d);
        }
        count += 1;
    }
    (sum / (3 * count.max(1)) as f32, worst, count)
}

#[test]
fn paint_follows_a_remesh_a_decimate_and_a_cut_and_mirror() {
    let old = ball(30_000);
    for resolution in [48, 128] {
        let mut new = remesh::voxel_remesh(&old, resolution, [false; 3]).unwrap();
        assert!(!new.has_paint());
        follow(&old, &mut new, Fit::Whole);
        let (mean, worst, _) = error(&new, |p| p, |_| true);
        assert!(mean < 1.5 && worst <= 6, "resolution {resolution}: {mean} {worst}");
        assert_eq!(new.paint_rows(), old.paint_rows());
    }
    let source = decimate::Source::of(&old);
    let (mut fewer, _) = decimate::decimated(&source, 3_000, &mut |_| true).unwrap().expect("not cancelled");
    follow(&old, &mut fewer, Fit::Whole);
    let (mean, worst, _) = error(&fewer, |p| p, |_| true);
    assert!(mean < 1.5 && worst <= 6, "decimate: {mean} {worst}");

    // Cut & Mirror from +X: the far side is the image of the side kept, colour and all.
    let (cut, _) = cut_mirror::cut_and_mirrored(&old, sculpt_symmetry::Axis::X, true).unwrap();
    let (mean, worst, count) = error(&cut, |p| Vec3::new(p.x.abs(), p.y, p.z), |_| true);
    assert!(count == cut.vertex_count() && mean < 1.0 && worst <= 4, "cut & mirror: {mean} {worst}");

    // A mesh that came with its own paint is left with it.
    let mut own = paint(primitives::build(Primitive::Sphere, 3_000, 0.5), "Own", |p| p * 0.1);
    let before = texels(&own, 0);
    follow(&old, &mut own, Fit::Whole);
    assert_eq!(texels(&own, 0), before);
    assert_eq!(own.paint_rows()[0].name, "Own");
}

/// The scene, the mesh in hand and the history: a painted ball with a
/// painted cube beside it, both selected, the ball active.
fn two_objects() -> (Scene, Mesh, History) {
    let mut mesh = ball(12_000);
    let mut scene = Scene::new("Ball");
    let ball = scene.active_id();
    let at = Vec3::new(0.45, 0.0, 0.0);
    // The cube's colour is of where it stands in the scene, beside the ball.
    let block = paint(primitives::build(Primitive::Cube, 2_400, 0.3), "Colour", move |p| p + at);
    let cube = scene.add("Block", block, &mut mesh);
    scene.get_mut(cube).unwrap().position = at;
    scene.select(ball, &mut mesh);
    scene.select_also(cube);
    let mut history = History::default();
    history.target = scene.active_mesh();
    (scene, mesh, history)
}

#[test]
fn a_join_and_a_boolean_carry_the_paint_of_every_object() {
    let (mut scene, mut mesh, mut history) = two_objects();
    let count = (mesh.vertex_count(), scene.mesh(scene.get(scene.selected()[1]).unwrap().mesh, &mesh).unwrap().vertex_count());
    selection::join(&mut scene, &mut mesh, &mut history, None).unwrap();
    assert_eq!(mesh.vertex_count(), count.0 + count.1);
    let (mean, worst, _) = error(&mesh, |p| p, |_| true);
    assert!(mean == 0.0 && worst == 0, "a plain join carries every vertex's paint as it was: {mean} {worst}");
    assert_eq!(mesh.paint_rows().len(), 1, "both objects' layers are called Colour: one layer");
    // Undo is the scene step: the ball's own paint as before.
    assert!(history.step_scene(true, &mut scene, &mut mesh));
    assert_eq!(mesh.vertex_count(), count.0);

    // Joined and remeshed: by the nearest point of either object.
    let (mut scene, mut mesh, mut history) = two_objects();
    selection::join(&mut scene, &mut mesh, &mut history, Some((96, [false; 3]))).unwrap();
    let (mean, worst, _) = error(&mesh, |p| p, |_| true);
    assert!(mean < 2.0 && worst <= 12, "{mean} {worst}");

    // Subtract: what is left of the ball keeps its paint; the cut's own faces take the block's.
    let (mut scene, mut mesh, mut history) = two_objects();
    let settings = BooleanSettings::new(64);
    selection::boolean(&mut scene, &mut mesh, &mut history, BooleanOp::Subtract, &settings, Others::Hidden).unwrap();
    let (mean, worst, count) = error(&mesh, |p| p, |_| true);
    assert!(count > 500 && mean < 2.0 && worst <= 12, "{mean} {worst}");
}

#[test]
fn a_copy_an_instance_and_a_live_mirror_made_real_keep_every_vertexs_paint() {
    let mut mesh = ball(6_000);
    let mut scene = Scene::new("Ball");
    let ball = scene.active_id();
    scene.active_mut().position = Vec3::new(0.8, 0.0, 0.0);
    // The mirror copy across X made real: the same vertices, reflected, with the same paint.
    assert!(scene.set_mirror(ball, 1, Vec3::ZERO));
    let made = scene.bake_mirror(ball, &mut mesh).expect("the ball has a mirror");
    let copy = scene.mesh(scene.get(made[0]).unwrap().mesh, &mesh).unwrap();
    // Its own space is the ball's reflected in X.
    let (mean, worst, count) = error(copy, |p| Vec3::new(-p.x, p.y, p.z), |_| true);
    assert!(count == mesh.vertex_count() && mean == 0.0 && worst == 0, "{mean} {worst}");

    // A clone: the same paint, by place.
    scene.select(ball, &mut mesh);
    let print = mesh.paint_print_by_place();
    scene.clone_active(&mut mesh);
    assert!(scene.active_id() != ball);
    assert_eq!(mesh.paint_print_by_place(), print);
    // An instance made unique has its own copy.
    scene.select(ball, &mut mesh);
    scene.instance_active();
    assert!(scene.make_unique(&mut mesh));
    assert_eq!(mesh.paint_print_by_place(), print);
}

#[test]
fn a_piece_placed_somewhere_else_takes_the_paint_of_the_surface_it_was_cut_from() {
    let mesh = ball(8_000);
    let scene = Scene::new("Ball");
    let ball = scene.active_id();
    // A whole copy of the ball standing somewhere else is the ball's own mesh in its own space.
    let far = glam::Mat4::from_translation(Vec3::new(5.0, 0.0, 0.0));
    let mut copy = Mesh::new(mesh.positions.clone(), mesh.indices.clone(), None);
    super::piece_of(&scene, &mesh, ball, far, &mut copy);
    let (mean, worst, _) = error(&copy, |p| p, |_| true);
    assert!(mean < 0.5 && worst <= 2, "{mean} {worst}");
}
