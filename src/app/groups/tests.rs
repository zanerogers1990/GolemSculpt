//! Face groups through the routines the app calls: Stay in Group asked as a
//! stroke begins, and the groups following the model through the real
//! rebuilders. The same through the buttons themselves, status lines
//! included, is the scripted run `--demo-groups play` (it needs the window).

use std::collections::{HashMap, HashSet};

use glam::{Vec2, Vec3};
use sculpt_volume::{BooleanOp, BooleanSettings};

use super::{began, follow, stay};
use crate::app::{cut_mirror, selection};
use crate::camera::{Camera, View};
use crate::history::History;
use crate::mesh::groups::Fit;
use crate::mesh::primitives::{self, Primitive};
use crate::mesh::{Mesh, StrokeDelta};
use crate::scene::{Others, Scene};
use crate::sculpt::{Brush, PointerSample, Sculptor};
use crate::{decimate, quadremesh, remesh};

const SIZE: Vec2 = Vec2::new(1600.0, 900.0);

fn centre(mesh: &Mesh, t: usize) -> Vec3 {
    let c = &mesh.indices[t * 3..t * 3 + 3];
    (mesh.positions[c[0] as usize] + mesh.positions[c[1] as usize] + mesh.positions[c[2] as usize]) / 3.0
}

/// A ball with every face in the group `of` gives its centre.
fn ball(tris: usize, of: impl Fn(Vec3) -> u16) -> Mesh {
    let mut mesh = primitives::build(Primitive::Sphere, tris, 0.5);
    let ids = (0..mesh.triangle_count()).map(|t| of(centre(&mesh, t))).collect();
    mesh.set_group_ids(ids);
    mesh
}

fn view_of(mesh: &Mesh) -> View {
    let mut camera = Camera::default();
    camera.frame(mesh.bounds(), SIZE.x / SIZE.y, false);
    camera.view(Vec2::ZERO, SIZE, camera.distance, 1.0)
}

/// A stroke from `from` to `to` (pixels from the middle of the view), begun
/// the way `Doc::begin_stroke` begins one. Returns the vertices it moved.
fn stroke(mesh: &mut Mesh, sculptor: &mut Sculptor, from: Vec2, to: Vec2, smooth: bool) -> Vec<u32> {
    let view = view_of(mesh);
    let sample = |i: usize| PointerSample {
        pos: SIZE * 0.5 + from.lerp(to, i as f32 / 40.0),
        pressure: None,
        time: i as f64 / 240.0,
    };
    stay(mesh, sculptor, &view, sample(0).pos, smooth);
    let begun = sculptor.begin(mesh, &view, sample(0), false, smooth);
    assert!(began(mesh, begun), "the stroke missed the mesh");
    for i in 1..=40 {
        sculptor.sample(mesh, &view, sample(i));
        if i % 4 == 0 {
            sculptor.frame(mesh, &view, 1.0 / 60.0);
            let _ = mesh.take_dirty();
        }
    }
    let delta = sculptor.end(mesh, &view);
    let _ = mesh.take_dirty();
    assert!(!mesh.stays_in_groups(), "the stroke is over, and so is its hold on the groups");
    // A stroke a sculpt layer recorded is the same stroke with the layer's side of it attached.
    fn moved(delta: StrokeDelta) -> Vec<u32> {
        match delta {
            StrokeDelta::Positions { verts, .. } => verts,
            StrokeDelta::Layered { stroke, .. } => moved(*stroke),
            StrokeDelta::Painted { shape, .. } => moved(*shape),
            StrokeDelta::Mask { .. } | StrokeDelta::Paint(_) => Vec::new(),
        }
    }
    delta.map_or(Vec::new(), moved)
}

fn sculptor(brush: Brush, mirror: bool, stay_in_group: bool) -> Sculptor {
    let mut sculptor = Sculptor::default();
    sculptor.settings.brush = brush;
    sculptor.settings.symmetry = [mirror, false, false];
    sculptor.settings.radius_px = 60.0;
    sculptor.settings.per_brush[brush.index()].strength = 0.8;
    sculptor.settings.per_brush[brush.index()].stay_in_group = stay_in_group;
    sculptor
}

/// How far from the plane y = 0 the border between groups 1 and 2 runs on
/// `mesh`, at most, and the longest edge along it.
fn border(mesh: &Mesh) -> (f32, f32) {
    let mut by_edge: HashMap<(u32, u32), Vec<u16>> = HashMap::new();
    for (t, c) in mesh.indices.chunks_exact(3).enumerate() {
        for (a, b) in [(c[0], c[1]), (c[1], c[2]), (c[2], c[0])] {
            by_edge.entry((a.min(b), a.max(b))).or_default().push(mesh.group_of(t as u32));
        }
    }
    let (mut worst, mut longest, mut edges) = (0f32, 0f32, 0);
    for (&(a, b), on) in &by_edge {
        if on.len() == 2 && on.contains(&1) && on.contains(&2) {
            let (p, q) = (mesh.positions[a as usize], mesh.positions[b as usize]);
            worst = worst.max(p.y.abs()).max(q.y.abs());
            longest = longest.max(p.distance(q));
            edges += 1;
        }
    }
    assert!(edges > 10, "groups 1 and 2 share a border ({edges} edges)");
    (worst, longest)
}

#[test]
fn a_brush_without_the_tick_and_a_mesh_without_groups_are_left_as_they_were() {
    let mut plain = primitives::build(Primitive::Sphere, 4_000, 0.5);
    let view = view_of(&plain);
    assert!(!stay(&mut plain, &sculptor(Brush::Clay, false, true), &view, SIZE * 0.5, false));
    assert!(!plain.stays_in_groups() && plain.hidden_vertices().is_none());

    let mut mesh = ball(4_000, |p| if p.x > 0.0 { 1 } else { 2 });
    assert!(!stay(&mut mesh, &sculptor(Brush::Clay, false, false), &view, SIZE * 0.5, false));
    assert!(!mesh.stays_in_groups());
    // Off the model there is no group to stay in, and nothing is held.
    assert!(!stay(&mut mesh, &sculptor(Brush::Clay, false, true), &view, Vec2::new(5.0, 5.0), false));
    assert!(!mesh.stays_in_groups());
    // The tick on, over the model: held, until the stroke that did not begin lets go.
    assert!(stay(&mut mesh, &sculptor(Brush::Clay, false, true), &view, SIZE * 0.5 + Vec2::new(40.0, 0.0), false));
    assert!(mesh.stays_in_groups());
    assert!(!began(&mut mesh, false) && !mesh.stays_in_groups());
}

#[test]
fn shift_smooths_with_the_smooth_brushs_own_tick() {
    // Clay has the tick; the Smooth brush Shift brings up does not, and it is the one doing the work.
    let mut mesh = ball(40_000, |p| if p.x > 0.0 { 1 } else { 2 });
    for v in 0..mesh.vertex_count() {
        let p = mesh.positions[v];
        mesh.positions[v] = p * (1.0 + 0.02 * (p.y * 60.0).sin());
    }
    let outside: Vec<bool> = mesh.group_vertices(1).iter().map(|&on| !on).collect();
    let mut clay = sculptor(Brush::Clay, false, true);
    let free = stroke(&mut mesh, &mut clay, Vec2::new(60.0, 0.0), Vec2::new(-120.0, 0.0), true);
    assert!(free.iter().filter(|&&v| outside[v as usize]).count() > 20, "Shift's smoothing is not held to the group");
    clay.settings.per_brush[Brush::Smooth.index()].stay_in_group = true;
    let kept = stroke(&mut mesh, &mut clay, Vec2::new(60.0, 40.0), Vec2::new(-120.0, 40.0), true);
    assert!(kept.len() > 20 && kept.iter().all(|&v| !outside[v as usize]), "with the Smooth brush's own tick it is");
}

#[test]
fn under_symmetry_each_side_stays_in_the_group_its_own_side_begins_on() {
    for brush in [Brush::Clay, Brush::Move, Brush::Smooth] {
        // Right and left on top, and a third group below that neither side begins on.
        let make = || {
            let mut mesh = ball(60_000, |p| {
                if p.y < -0.05 {
                    3
                } else if p.x > 0.0 {
                    1
                } else {
                    2
                }
            });
            // Something for Smooth to smooth, the same on both sides.
            for v in 0..mesh.vertex_count() {
                let p = mesh.positions[v];
                mesh.positions[v] = p * (1.0 + 0.02 * (p.y * 60.0).sin() * (p.x.abs() * 40.0).cos());
            }
            mesh.recompute_all_normals();
            mesh
        };
        let mut mesh = make();
        let rest = mesh.positions.clone();
        let (one, two) = (mesh.group_vertices(1), mesh.group_vertices(2));
        let third: Vec<bool> = one.iter().zip(&two).map(|(one, two)| !one && !two).collect();
        // Down the right side, from the upper group well into the lower one; Move grabs what is under
        // its ring where it starts, so it starts just above the border.
        let (from, to) = match brush {
            Brush::Move => (Vec2::new(90.0, 5.0), Vec2::new(150.0, 60.0)),
            _ => (Vec2::new(90.0, -130.0), Vec2::new(110.0, 140.0)),
        };

        let free = stroke(&mut make(), &mut sculptor(brush, true, false), from, to, false);
        let reached = free.iter().filter(|&&v| third[v as usize]).count();
        assert!(reached > 50, "{}: the free stroke only reached {reached} vertices of the third group", brush.name());

        let kept = stroke(&mut mesh, &mut sculptor(brush, true, true), from, to, false);
        assert!(kept.iter().all(|&v| !third[v as usize]), "{}: a vertex of the third group moved", brush.name());
        let (right, left) = (
            kept.iter().filter(|&&v| rest[v as usize].x > 0.02).count(),
            kept.iter().filter(|&&v| rest[v as usize].x < -0.02).count(),
        );
        assert!(right > 100 && left == right, "{}: {right} moved on the right, {left} on the left", brush.name());
        // And the two sides are still mirror images of each other, to the bit.
        let places: HashSet<[u32; 3]> =
            mesh.positions.iter().map(|p| (*p + Vec3::ZERO).to_array().map(f32::to_bits)).collect();
        let image = |p: &Vec3| (Vec3::new(-p.x, p.y, p.z) + Vec3::ZERO).to_array().map(f32::to_bits);
        assert!(
            mesh.positions.iter().all(|p| places.contains(&image(p))),
            "{}: the sides no longer match",
            brush.name()
        );
    }
}

#[test]
fn a_two_group_ball_keeps_its_groups_through_the_real_rebuilders() {
    let old = ball(30_000, |p| if p.y > 0.0 { 1 } else { 2 });
    let half = |mesh: &Mesh| mesh.group_triangles(1, true).len() as f32 / mesh.triangle_count() as f32;

    // Voxel remesh, coarse and fine.
    for resolution in [32, 96] {
        let mut new = remesh::voxel_remesh(&old, resolution, [false; 3]).unwrap();
        assert!(!new.has_groups());
        follow(&old, &mut new, Fit::Whole);
        let (worst, longest) = border(&new);
        assert_eq!(new.group_count(), 2, "resolution {resolution}");
        assert!(
            worst <= longest,
            "resolution {resolution}: the border is {worst} from where it was, a triangle there {longest} across"
        );
        assert!((half(&new) - 0.5).abs() < 0.03);
        assert!(
            (0..new.triangle_count())
                .all(|t| centre(&new, t).y.abs() <= longest
                    || (new.group_of(t as u32) == 1) == (centre(&new, t).y > 0.0))
        );
    }

    // Quad remesh: quads, each whole in its group.
    let (mut quads, _) = quadremesh::quad_remesh_mesh(&old, 600).unwrap();
    follow(&old, &mut quads, Fit::Whole);
    let (worst, longest) = border(&quads);
    assert!(quads.polys.is_some() && quads.group_count() == 2 && worst <= longest, "{worst} against {longest}");
    let of_face = quads.face_groups().unwrap();
    assert!(of_face.iter().filter(|&&id| id == 1).count().abs_diff(of_face.len() / 2) < of_face.len() / 10);

    // Decimate: far fewer, larger triangles, the border still where it was to within one of them.
    let source = decimate::Source::of(&old);
    let (mut fewer, _) = decimate::decimated(&source, 3_000, &mut |_| true).unwrap().expect("not cancelled");
    follow(&old, &mut fewer, Fit::Whole);
    let (worst, longest) = border(&fewer);
    assert!(fewer.group_count() == 2 && worst <= longest, "{worst} against {longest}");
    assert!((half(&fewer) - 0.5).abs() < 0.05);

    // Cut & Mirror: the side kept keeps its groups, and the other side is their image.
    let lopsided = ball(30_000, |p| {
        if p.x <= 0.0 {
            9
        } else if p.y > 0.0 {
            1
        } else {
            2
        }
    });
    let (cut, _) = cut_mirror::cut_and_mirrored(&lopsided, sculpt_symmetry::Axis::X, true).unwrap();
    assert_eq!(cut.group_count(), 2, "the side that was thrown away took its group with it");
    let (worst, longest) = border(&cut);
    assert!(worst <= longest);

    // A mesh that came with its own groups is left with them.
    let mut own = ball(5_000, |p| if p.z > 0.0 { 5 } else { 6 });
    let print = own.groups_print();
    follow(&old, &mut own, Fit::Whole);
    assert_eq!(own.groups_print(), print);
}

/// The scene, the mesh in hand and the history, as `Doc` holds them: a ball
/// in two groups with a cube beside it, both selected, the ball active.
fn two_objects(cube_groups: bool) -> (Scene, Mesh, History) {
    let mut mesh = ball(12_000, |p| if p.y > 0.0 { 1 } else { 2 });
    let mut scene = Scene::new("Ball");
    let ball = scene.active_id();
    let mut block = primitives::build(Primitive::Cube, 2_400, 0.3);
    if cube_groups {
        let ids = (0..block.triangle_count()).map(|t| if centre(&block, t).z > 0.0 { 1 } else { 2 }).collect();
        block.set_group_ids(ids);
    }
    let cube = scene.add("Block", block, &mut mesh);
    scene.active_mut().position = Vec3::new(0.45, 0.0, 0.0);
    scene.select(ball, &mut mesh);
    scene.select_also(cube);
    let mut history = History::default();
    history.target = scene.active_mesh();
    (scene, mesh, history)
}

#[test]
fn a_join_and_a_boolean_carry_the_groups_of_every_object_they_take_in() {
    // Join: both objects' groups, the second object's renamed where they clash.
    let (mut scene, mut mesh, mut history) = two_objects(true);
    assert!(selection::join(&mut scene, &mut mesh, &mut history, None).is_some());
    assert_eq!(mesh.group_count(), 4);
    for t in 0..mesh.triangle_count() {
        let (p, id) = (centre(&mesh, t), mesh.group_of(t as u32));
        // The ball is 0.5 across from its centre; the block stands from 0.15 to 0.75.
        let on_ball = p.length() < 0.501 && (p.length() - 0.5).abs() < 0.01;
        if on_ball && p.x < 0.1 {
            assert_eq!(id, if p.y > 0.0 { 1 } else { 2 }, "the ball's face at {p}");
        } else if p.x > 0.6 {
            assert!(id == 3 || id == 4, "the block's face at {p} is in group {id}");
            assert_eq!(id == 3, p.z > 0.0, "the block's face at {p}");
        }
    }
    // A group of a single face comes through a join as it is: nothing is tidied away.
    let (mut scene, mut mesh, mut history) = two_objects(true);
    let lone = (0..mesh.triangle_count()).find(|&t| centre(&mesh, t).y > 0.3 && centre(&mesh, t).x < 0.0).unwrap();
    let at = centre(&mesh, lone);
    let mut ids = mesh.group_ids().to_vec();
    ids[lone] = 77;
    mesh.set_group_ids(ids);
    selection::join(&mut scene, &mut mesh, &mut history, None).unwrap();
    let kept: Vec<usize> = (0..mesh.triangle_count()).filter(|&t| mesh.group_of(t as u32) == 77).collect();
    assert!(kept.len() == 1 && centre(&mesh, kept[0]).distance(at) < 1e-6, "{kept:?}");
    assert!(history.step_scene(true, &mut scene, &mut mesh));
    // Undo is the scene step the join made: the ball has its groups as before.
    assert_eq!(mesh.group_count(), 3);

    // A block without groups joins as surface in no group.
    let (mut scene, mut mesh, mut history) = two_objects(false);
    selection::join(&mut scene, &mut mesh, &mut history, None).unwrap();
    assert_eq!(mesh.group_count(), 2);
    assert!((0..mesh.triangle_count()).filter(|&t| centre(&mesh, t).x > 0.6).all(|t| mesh.group_of(t as u32) == 0));

    // Subtract: what is left of the ball keeps its groups, and the cut's own faces are the block's.
    let (mut scene, mut mesh, mut history) = two_objects(true);
    let settings = BooleanSettings::new(64);
    selection::boolean(&mut scene, &mut mesh, &mut history, BooleanOp::Subtract, &settings, Others::Hidden).unwrap();
    let ids: HashSet<u16> = mesh.group_ids().iter().copied().collect();
    assert!(ids.contains(&1) && ids.contains(&2), "{ids:?}");
    let skin = (0..mesh.triangle_count()).filter(|&t| centre(&mesh, t).x < 0.0);
    assert!(skin.clone().count() > 500);
    for t in skin {
        let p = centre(&mesh, t);
        assert!(p.y.abs() < 0.05 || mesh.group_of(t as u32) == if p.y > 0.0 { 1 } else { 2 }, "the ball's face at {p}");
    }
}

#[test]
fn a_piece_placed_somewhere_else_finds_the_surface_it_was_cut_from() {
    let mesh = ball(8_000, |p| if p.y > 0.0 { 1 } else { 2 });
    let mut scene = Scene::new("Ball");
    let ball = scene.active_id();
    scene.active_mut().position = Vec3::new(0.3, 0.2, 0.0);
    scene.active_mut().scale = Vec3::splat(2.0);
    // The top of the ball as an object of its own, standing where the ball's top is, under another placement.
    let top: Vec<u32> = (0..mesh.triangle_count())
        .filter(|&t| centre(&mesh, t).y > 0.1)
        .flat_map(|t| mesh.indices[t * 3..t * 3 + 3].to_vec())
        .collect();
    let model = glam::Mat4::from_translation(Vec3::new(0.3, 0.2, 0.0)) * glam::Mat4::from_scale(Vec3::splat(0.5));
    // In the piece's own space the ball's points are four times as far out.
    let positions: Vec<Vec3> = mesh.positions.iter().map(|&p| p * 4.0).collect();
    let mut piece = Mesh::new(positions, top, None);
    super::piece_of(&scene, &mesh, ball, model, &mut piece);
    assert_eq!(piece.group_count(), 1);
    assert!(piece.group_ids().iter().all(|&id| id == 1));
    // A whole copy of the ball standing somewhere else is the ball's own mesh in its own space.
    let far = glam::Mat4::from_translation(Vec3::new(5.0, 0.0, 0.0));
    let mut copy = Mesh::new(mesh.positions.clone(), mesh.indices.clone(), None);
    super::piece_of(&scene, &mesh, ball, far, &mut copy);
    assert_eq!(copy.group_count(), 2);
    assert!((0..copy.triangle_count()).all(|t| (copy.group_of(t as u32) == 1) == (centre(&copy, t).y > 0.0)));
    // An object the scene does not have, or one without groups, gives none.
    let mut other = Mesh::new(piece.positions.clone(), piece.indices.clone(), None);
    super::piece_of(&scene, &mesh, 999, model, &mut other);
    assert!(!other.has_groups());
}

#[test]
fn a_live_mirror_made_real_a_copy_and_an_instance_keep_every_face_in_its_group() {
    let of = |p: Vec3| {
        if p.y > 0.0 {
            1
        } else if p.z > 0.0 {
            2
        } else {
            0
        }
    };
    let mut mesh = ball(6_000, of);
    let census = |mesh: &Mesh| [0u16, 1, 2].map(|group| mesh.group_triangles(group, true).len());
    let was = census(&mesh);
    let mut scene = Scene::new("Ball");
    let ball = scene.active_id();
    scene.active_mut().position = Vec3::new(0.8, 0.0, 0.0);

    // The mirror copy across X made real: the same faces, reflected, in the same groups.
    assert!(scene.set_mirror(ball, 1, Vec3::ZERO));
    let made = scene.bake_mirror(ball, &mut mesh).expect("the ball has a mirror");
    let copy = scene.mesh(scene.get(made[0]).unwrap().mesh, &mesh).expect("the copy has a mesh of its own");
    assert_eq!(census(copy), was);
    // Its own space is the ball's reflected in X, which leaves y and z as they were.
    assert!((0..copy.triangle_count()).all(|t| copy.group_of(t as u32) == of(centre(copy, t))));

    // A clone is the same faces in the same groups; an instance shares them, and made unique has its own.
    scene.select(ball, &mut mesh);
    scene.clone_active(&mut mesh);
    assert!(scene.active_id() != ball && census(&mesh) == was);
    assert!((0..mesh.triangle_count()).all(|t| mesh.group_of(t as u32) == of(centre(&mesh, t))));
    scene.select(ball, &mut mesh);
    scene.instance_active();
    assert_eq!(census(&mesh), was);
    assert!(scene.make_unique(&mut mesh));
    assert_eq!(census(&mesh), was);
    mesh.set_group_ids(Vec::new());
    scene.select(ball, &mut mesh);
    assert_eq!(census(&mesh), was, "the original's groups are its own");
}

// ------------------------------------------------- strokes as the app begins them

/// A document holding `mesh`, and a view onto it from the front.
fn doc_of(mesh: Mesh) -> (crate::doc::Doc, View) {
    let view = view_of(&mesh);
    let mut doc = crate::doc::Doc {
        mesh,
        scene: Scene::new("Ball"),
        mesh_replaced: true,
        history: Default::default(),
        sculptor: Default::default(),
        camera: Camera::default(),
        opts: Default::default(),
        path: None,
        stats: Default::default(),
        tool: crate::doc::Tool::Sculpt,
        spheres: Default::default(),
        tools: Default::default(),
        turntable: Default::default(),
        timelapse: Default::default(),
        still: Default::default(),
    };
    doc.history.target = doc.scene.active_mesh();
    doc.sculptor.settings.symmetry = [false; 3];
    (doc, view)
}

/// One stroke across the front of the model through `Doc::begin_stroke`,
/// which is how the pointer, the bridge and every scripted run begin one.
fn doc_stroke(doc: &mut crate::doc::Doc, view: &View, brush: Brush, from: Vec2, to: Vec2) -> bool {
    doc.sculptor.settings.brush = brush;
    doc.sculptor.settings.radius_px = 60.0;
    doc.sculptor.settings.per_brush[brush.index()].strength = 0.8;
    let at = |i: usize| SIZE * 0.5 + from.lerp(to, i as f32 / 40.0);
    let sample = |i: usize| PointerSample { pos: at(i), pressure: None, time: i as f64 / 240.0 };
    if !doc.begin_stroke(view, sample(0), false, false) {
        return false;
    }
    for i in 1..=40 {
        doc.sculptor.sample(&mut doc.mesh, view, sample(i));
        if i % 4 == 0 {
            doc.sculptor.frame(&mut doc.mesh, view, 1.0 / 60.0);
            let _ = doc.mesh.take_dirty();
        }
    }
    let delta = doc.sculptor.end(&mut doc.mesh, view);
    let _ = doc.mesh.take_dirty();
    delta.map(|delta| doc.history.push_stroke(delta)).is_some()
}

#[test]
fn reset_shape_goes_back_to_the_shape_before_the_first_stroke_however_the_stroke_was_begun() {
    use crate::mesh::levels::{stepped, subdivided};
    use sculpt_multires::Split;
    for kind in ["plain", "groups", "levels", "a layer"] {
        let mut mesh = ball(4_000, |p| {
            if kind != "groups" {
                0
            } else if p.x > 0.0 {
                1
            } else {
                2
            }
        });
        if kind == "levels" {
            mesh = subdivided(&mesh, Split::Smooth).unwrap().0;
            mesh.set_group_ids((0..mesh.triangle_count()).map(|t| 1 + (t % 2) as u16).collect());
        }
        if kind == "a layer" {
            // A layer that is not recording: the stroke is on the model underneath it.
            mesh.add_layer(None);
            mesh.record_on_layer(None);
        }
        let (mut doc, view) = doc_of(mesh);
        let made = doc.mesh.positions.clone();
        let prints = |mesh: &Mesh| {
            let levels = mesh.levels.as_ref().map(|levels| levels.print(mesh));
            (mesh.groups_print(), mesh.level_count(), mesh.layer_count(), levels)
        };
        let was = prints(&doc.mesh);
        assert!(!doc.scene.can_reset(&doc.mesh), "{kind}: nothing sculpted yet");

        // Two strokes, begun as the bridge and the scripted runs begin them: no pointer press before them.
        assert!(doc_stroke(&mut doc, &view, Brush::Clay, Vec2::new(-120.0, -40.0), Vec2::new(120.0, -20.0)), "{kind}");
        assert!(doc_stroke(&mut doc, &view, Brush::Inflate, Vec2::new(-100.0, 60.0), Vec2::new(100.0, 40.0)), "{kind}");
        let sculpted = doc.mesh.positions.clone();
        assert!(sculpted != made && doc.scene.can_reset(&doc.mesh), "{kind}: the strokes left something to put back");

        let delta = doc.scene.reset_shape(&mut doc.mesh);
        doc.history.push_stroke(delta.unwrap_or_else(|| panic!("{kind}: Reset Shape had nothing to go back to")));
        // To the bit; under a sculpt layer, to the last digit a position keeps (the model underneath
        // is put back and the layer's offset added to it again, which rounds once).
        let off = doc.mesh.positions.iter().zip(&made).map(|(a, b)| a.distance(*b)).fold(0.0, f32::max);
        let most = if kind == "a layer" { 2e-7 } else { 0.0 };
        assert!(off <= most, "{kind}: Reset Shape leaves the model {off:e} from the shape as made");
        assert_eq!(prints(&doc.mesh), was, "{kind}: and the groups, levels and layers are as they were");
        assert!(doc.scene.reset_shape(&mut doc.mesh).is_none(), "{kind}: already at rest");
        assert_eq!(doc.history.undo(&mut doc.mesh), Some(false));
        assert!(doc.mesh.positions == sculpted, "{kind}: Undo of the reset is the sculpt again");
    }

    // Painting a mask moves nothing, and is not what Reset Shape goes back to.
    let (mut doc, view) = doc_of(ball(4_000, |_| 0));
    doc.sculptor.settings.mask_mode = true;
    assert!(doc_stroke(&mut doc, &view, Brush::Standard, Vec2::new(-80.0, 0.0), Vec2::new(80.0, 0.0)));
    assert!(!doc.scene.can_reset(&doc.mesh));

    // A change of level is a new mesh, which rests where it was built: Reset Shape then goes
    // back no further than that, as after a remesh.
    let mesh = subdivided(&ball(4_000, |_| 0), Split::Smooth).unwrap().0;
    let (mut doc, view) = doc_of(mesh);
    assert!(doc_stroke(&mut doc, &view, Brush::Clay, Vec2::new(-120.0, 0.0), Vec2::new(120.0, 0.0)));
    assert!(doc.scene.can_reset(&doc.mesh));
    doc.mesh = stepped(&stepped(&doc.mesh, 0).unwrap().0, 1).unwrap().0;
    assert!(!doc.scene.can_reset(&doc.mesh));
    let arrived = doc.mesh.positions.clone();
    assert!(doc_stroke(&mut doc, &view, Brush::Clay, Vec2::new(-120.0, 60.0), Vec2::new(120.0, 60.0)));
    assert!(doc.scene.reset_shape(&mut doc.mesh).is_some() && doc.mesh.positions == arrived);
}

#[test]
fn under_radial_symmetry_every_repeat_stays_in_the_group_it_begins_on() {
    use std::f32::consts::{PI, TAU};
    // Six sectors round the Y axis above, and a seventh group below that no repeat begins on.
    let sector = |p: Vec3| 1 + ((p.z.atan2(p.x) + PI) / (TAU / 6.0)).floor().clamp(0.0, 5.0) as u16;
    let of = move |p: Vec3| if p.y < -0.05 { 7 } else { sector(p) };
    for brush in [Brush::Clay, Brush::Move, Brush::EraseLayer] {
        let make = || {
            let mut mesh = ball(60_000, of);
            if brush == Brush::EraseLayer {
                // Something for it to rub out: a layer, recording, with ripples all over it.
                mesh.add_layer(None);
                let leaves: Vec<u32> = (0..mesh.bvh.leaves.len() as u32).collect();
                mesh.begin_stroke();
                mesh.move_vertices(&leaves, |_, v| Some(v.pos * (1.0 + 0.03 * (v.pos.y * 40.0).sin())));
                mesh.positions_changed(&leaves);
                mesh.end_stroke(false).unwrap();
                let _ = mesh.take_dirty();
            }
            mesh
        };
        let radial = |stay_in_group: bool| {
            let mut sculptor = sculptor(brush, false, stay_in_group);
            sculptor.settings.radial = crate::sculpt::Radial { on: true, count: 6, ..Default::default() };
            sculptor
        };
        let mut mesh = make();
        let rest = mesh.positions.clone();
        // The vertices with no face in any of the six upper groups.
        let upper: Vec<Vec<bool>> = (1..=6).map(|group| mesh.group_vertices(group)).collect();
        let only_lower: Vec<bool> = (0..rest.len()).map(|v| upper.iter().all(|on| !on[v])).collect();
        // Down the front from the upper groups well into the lower one; Move starts just above the border.
        let (from, to) = match brush {
            Brush::Move => (Vec2::new(20.0, 5.0), Vec2::new(40.0, 70.0)),
            _ => (Vec2::new(20.0, -120.0), Vec2::new(30.0, 140.0)),
        };
        let free = stroke(&mut make(), &mut radial(false), from, to, false);
        let reached = free.iter().filter(|&&v| only_lower[v as usize]).count();
        assert!(reached > 50, "{}: the free stroke only reached {reached} vertices of the lower group", brush.name());

        let kept = stroke(&mut mesh, &mut radial(true), from, to, false);
        assert!(kept.iter().all(|&v| !only_lower[v as usize]), "{}: a vertex of the lower group moved", brush.name());
        // Every one of the six repeats did its share: each sector has moved vertices of its own.
        for group in 1..=6u16 {
            let own = kept.iter().filter(|&&v| sector(rest[v as usize]) == group).count();
            assert!(own > 20, "{}: sector {group} has only {own} moved vertices", brush.name());
        }
    }
}

#[test]
fn mask_recesses_and_peaks_are_one_undo_step_and_leave_levels_layers_and_groups_alone() {
    use crate::mesh::levels::{stepped, subdivided};
    use crate::sculpt::cavity::{self, Shape};
    use sculpt_multires::Split;
    // Three levels, with a crease and a ridge on the finest, in two groups, with a layer of
    // ripples and part of it hidden.
    let base = ball(3_000, |_| 0);
    let mut mesh = subdivided(&subdivided(&base, Split::Smooth).unwrap().0, Split::Smooth).unwrap().0;
    let leaves: Vec<u32> = (0..mesh.bvh.leaves.len() as u32).collect();
    let bump = |y: f32, at: f32| (-((y - at) * 30.0).powi(2)).exp();
    mesh.begin_stroke();
    mesh.move_vertices(&leaves, |_, v| Some(v.pos * (1.0 - 0.06 * bump(v.pos.y, 0.0) + 0.05 * bump(v.pos.y, 0.25))));
    mesh.positions_changed(&leaves);
    mesh.end_stroke(false).unwrap();
    let ids = (0..mesh.triangle_count()).map(|t| if centre(&mesh, t).x > 0.0 { 1 } else { 2 }).collect();
    mesh.set_group_ids(ids);
    mesh.add_layer(Some("Ripples".into()));
    mesh.begin_stroke();
    mesh.move_vertices(&leaves, |_, v| Some(v.pos * (1.0 + 0.004 * (v.pos.x * 60.0).sin())));
    mesh.positions_changed(&leaves);
    mesh.end_stroke(false).unwrap();
    let back: Vec<u32> = (0..mesh.triangle_count() as u32).filter(|&t| centre(&mesh, t as usize).z < -0.3).collect();
    mesh.hide(&back);
    let hidden = mesh.hidden_vertices().unwrap().to_vec();

    let mut history = History::default();
    let kept = |mesh: &Mesh| {
        let levels = mesh.levels.as_ref().unwrap();
        let counts = (levels.count(), levels.current(), mesh.hidden_triangle_count());
        (mesh.positions.clone(), mesh.layers_print(), mesh.groups_print(), counts)
    };
    let (was, no_mask) = (kept(&mesh), mesh.mask.clone());
    for shape in [Shape::Recesses, Shape::Peaks] {
        let name = shape.name();
        let delta = cavity::mask(&mut mesh, shape, 0.7).unwrap_or_else(|| panic!("{name}: nothing was masked"));
        assert!(matches!(delta, StrokeDelta::Mask { .. }), "{name}: a mask step and nothing else");
        history.push_stroke(delta);
        assert!(kept(&mesh) == was, "{name}: the shape, layers, groups, levels and hidden faces are untouched");
        let share = cavity::share_masked(&mesh);
        assert!(share > 0.01 && share < 0.6, "{name}: {share} of the surface is masked");
        assert!(
            (0..mesh.vertex_count()).all(|v| !hidden[v] || mesh.mask[v] == 0.0),
            "{name}: hidden parts keep their mask"
        );
        // The crease is a recess and the ridge a peak, and each command takes its own.
        let at = |y: f32| {
            let near = (0..mesh.vertex_count()).filter(|&v| !hidden[v] && (mesh.positions[v].y - y).abs() < 0.01);
            let (sum, count) = near.fold((0.0, 0), |(sum, count), v| (sum + mesh.mask[v], count + 1));
            sum / count.max(1) as f32
        };
        let (crease, ridge) = (at(0.0), at(0.25));
        assert_eq!(crease > ridge, shape == Shape::Recesses, "{name}: crease {crease}, ridge {ridge}");
        // Curvature is put back as it was found: off.
        assert!(mesh.curv.is_empty());
    }
    assert_eq!(history.steps(), (2, 0), "each is one undo step");
    history.undo(&mut mesh).unwrap();
    history.undo(&mut mesh).unwrap();
    assert!(mesh.mask == no_mask && kept(&mesh) == was);
    history.redo(&mut mesh).unwrap();

    // The mask goes down and up the levels with the model, and the model comes back as it was, to the bit.
    let ordered = |mesh: &Mesh| mesh.levels.as_ref().unwrap().ordered_positions(mesh);
    let fine = ordered(&mesh);
    let mut low = stepped(&mesh, 0).unwrap().0;
    assert!(low.mask.iter().any(|&m| m > 0.5), "the mask came down with the model");
    let (layers, groups) = (low.layer_count(), low.group_count());
    let delta = cavity::mask(&mut low, Shape::Peaks, 0.9);
    assert!(delta.is_some() && low.layer_count() == layers && low.group_count() == groups && low.level_count() == 3);
    let up = stepped(&low, 2).unwrap().0;
    assert!(ordered(&up) == fine, "a mask made on a low level moves nothing on the way back up");
    assert_eq!((up.layer_count(), up.group_count()), (1, 2));
}

#[test]
fn the_pose_preview_shows_the_chain_a_press_kept_to_its_group_will_make() {
    // A limb standing up, its upper half one group and its lower half another.
    let part = primitives::build(Primitive::Capsule, 20_000, 0.5);
    let positions: Vec<Vec3> = part.positions.iter().map(|&p| p * Vec3::new(0.4, 1.0, 0.4)).collect();
    let mut mesh = Mesh::new(positions, part.indices.clone(), None);
    let ids = (0..mesh.triangle_count()).map(|t| if centre(&mesh, t).y > 0.0 { 1 } else { 2 }).collect();
    mesh.set_group_ids(ids);
    let view = view_of(&mesh);
    let poser = |stay_in_group: bool| {
        let mut sculptor = sculptor(Brush::Pose, false, stay_in_group);
        sculptor.settings.screen_space_size = false;
        sculptor.settings.radius_world = 0.5;
        sculptor
    };
    let at = mesh.raycast(Vec3::new(0.0, 0.3, 3.0), Vec3::NEG_Z).expect("the limb is in front of the camera").pos;
    let chains = |sculptor: &Sculptor| sculptor.pose_chains().iter().map(|chain| chain.to_vec()).collect::<Vec<_>>();
    // The preview as `Running::pose_frame` makes it.
    let preview = |mesh: &mut Mesh, sculptor: &mut Sculptor| {
        let fenced = super::stay_at(mesh, sculptor, at, false);
        sculptor.pose_hover(mesh, Some((at, sculptor.settings.radius_world)), false);
        if fenced {
            mesh.leave_groups();
        }
        assert!(!mesh.stays_in_groups(), "the preview lets go of the groups when it has looked");
        chains(sculptor)
    };
    // And the chain of the press itself, as `Doc::begin_stroke` begins it.
    let pressed = |mesh: &mut Mesh, sculptor: &mut Sculptor| {
        let pos = view.project(at).unwrap();
        stay(mesh, sculptor, &view, pos, false);
        let begun = sculptor.begin(mesh, &view, PointerSample { pos, pressure: None, time: 0.0 }, false, false);
        assert!(began(mesh, begun), "the press missed the limb");
        let made = chains(sculptor);
        sculptor.end(mesh, &view);
        made
    };
    let apart = |a: &[Vec<Vec3>], b: &[Vec<Vec3>]| {
        assert_eq!(a.len(), b.len());
        let pairs = a.iter().zip(b).flat_map(|(a, b)| a.iter().zip(b));
        pairs.map(|(p, q)| p.distance(*q)).fold(0.0, f32::max)
    };

    let (free, kept) = (preview(&mut mesh, &mut poser(false)), preview(&mut mesh, &mut poser(true)));
    assert!(!free.is_empty() && !kept.is_empty(), "there is a chain to show either way");
    let differ = apart(&free, &kept);
    assert!(differ > 0.05, "kept to the upper group the limb bends somewhere else ({differ})");
    // Nothing of the kept chain reaches into the lower group.
    assert!(kept.iter().flatten().all(|p| p.y > -0.05), "{kept:?}");
    // Each preview is the chain its own press makes.
    for stay_in_group in [false, true] {
        let shown = preview(&mut mesh, &mut poser(stay_in_group));
        let made = pressed(&mut mesh, &mut poser(stay_in_group));
        let off = apart(&shown, &made);
        assert!(off < 0.02, "tick {stay_in_group}: the preview is {off} from the chain the press makes");
    }
}
