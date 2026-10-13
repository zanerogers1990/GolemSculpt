//! Everything that puts a new mesh in an object's place, walked one by one
//! at the layer the commands call (the scene, the mesh in hand and the
//! history, as `Doc` holds them): each must leave the object without levels,
//! and Undo of it must bring them back exactly. And the things that do not
//! rebuild the mesh must leave them alone.
//!
//! The same walk through the buttons themselves, status lines included, is
//! the scripted run `--demo-levels play` (it needs the real app's window).

use glam::Vec3;
use sculpt_multires::Split;
use sculpt_symmetry::Axis;
use sculpt_volume::{BooleanOp, BooleanSettings};

use crate::app::{cut_mirror, origin, selection};
use crate::history::{History, Next};
use crate::mesh::levels::{stepped, subdivided};
use crate::mesh::primitives::{self, Primitive};
use crate::mesh::{Mesh, StrokeDelta};
use crate::scene::{Others, Scene};
use crate::{decimate, quadremesh, remesh};

/// The scene, the mesh in hand and the history, as `Doc` holds them.
struct Bench {
    scene: Scene,
    mesh: Mesh,
    history: History,
    /// The object with the levels.
    ball: u64,
}

impl Bench {
    /// One object, a ball of three levels sculpted on the finest.
    fn new() -> Bench {
        let ball = primitives::build(Primitive::Sphere, 2_000, 0.5);
        let mut mesh = subdivided(&subdivided(&ball, Split::Smooth).unwrap().0, Split::Smooth).unwrap().0;
        for v in 0..mesh.positions.len() {
            let p = mesh.positions[v];
            mesh.positions[v] = p * (1.0 + 0.03 * (p.x * 29.0).sin() * (p.y * 19.0).cos());
            mesh.mask[v] = if p.z > 0.3 { 1.0 } else { 0.0 };
        }
        mesh.hide(&[1, 2, 3, 40, 41]);
        let scene = Scene::new("Ball");
        let mut history = History::default();
        history.target = scene.active_mesh();
        Bench { ball: scene.active_id(), scene, mesh, history }
    }

    /// The ball's mesh, wherever it is: in hand or parked.
    fn ball_mesh(&self) -> &Mesh {
        let id = self.scene.get(self.ball).expect("the ball is in the scene").mesh;
        self.scene.mesh(id, &self.mesh).expect("its mesh is in the scene")
    }

    fn print(&self) -> u64 {
        let mesh = self.ball_mesh();
        mesh.levels.as_ref().map_or(0, |levels| levels.print(mesh))
    }

    /// As `Running::replace_mesh`: the new mesh in hand, the old one the undo step.
    fn replace(&mut self, mesh: Mesh) {
        self.history.push_replace(std::mem::replace(&mut self.mesh, mesh));
    }

    /// A cube beside the ball, selected along with it, the ball the active one.
    fn with_a_block(&mut self) {
        let before = self.scene.state();
        let block = self.scene.add("Block", primitives::build(Primitive::Cube, 600, 0.3), &mut self.mesh);
        self.scene.active_mut().position = Vec3::new(0.3, 0.0, 0.0);
        self.history.push_scene(before);
        self.scene.select(self.ball, &mut self.mesh);
        self.scene.select_also(block);
        self.history.target = self.scene.active_mesh();
    }

    /// Undo, as `Running::act` runs it, until `steps` are left.
    fn undo_to(&mut self, steps: usize) {
        while self.history.steps().0 > steps {
            match self.history.next_undo() {
                Some(Next::Scene) => assert!(self.history.step_scene(true, &mut self.scene, &mut self.mesh)),
                Some(Next::Mesh(id)) => {
                    self.scene.focus_mesh(id, &mut self.mesh);
                    assert!(self.history.undo(&mut self.mesh).is_some());
                }
                None => panic!("nothing left to undo"),
            }
            self.history.target = self.scene.active_mesh();
        }
        self.scene.select(self.ball, &mut self.mesh);
    }
}

type Command = fn(&mut Bench);

/// Everything that puts a new mesh in the ball's place.
fn rebuilders() -> [(&'static str, Command); 11] {
    [
        ("voxel remesh", |b| {
            let rebuilt = remesh::voxel_remesh(&b.mesh, 40, [false; 3]).unwrap();
            b.replace(rebuilt);
        }),
        ("quad remesh", |b| {
            let rebuilt = quadremesh::quad_remesh_mesh(&b.mesh, 300).unwrap().0;
            b.replace(rebuilt);
        }),
        ("decimate", |b| {
            let source = decimate::Source::of(&b.mesh);
            let (rebuilt, _) = decimate::decimated(&source, 3_000, &mut |_| true).unwrap().expect("not cancelled");
            b.replace(rebuilt);
        }),
        ("cut and mirror", |b| {
            let rebuilt = cut_mirror::cut_and_mirrored(&b.mesh, Axis::X, true).unwrap().0;
            b.replace(rebuilt);
        }),
        ("join", |b| {
            b.with_a_block();
            assert!(selection::join(&mut b.scene, &mut b.mesh, &mut b.history, None).is_some());
        }),
        ("join with a remesh", |b| {
            b.with_a_block();
            assert!(selection::join(&mut b.scene, &mut b.mesh, &mut b.history, Some((40, [false; 3]))).is_some());
        }),
        ("boolean", |b| {
            b.with_a_block();
            let settings = BooleanSettings::new(40);
            let made = selection::boolean(
                &mut b.scene,
                &mut b.mesh,
                &mut b.history,
                BooleanOp::Subtract,
                &settings,
                Others::Hidden,
            );
            assert!(made.is_ok());
        }),
        ("a mesh back from Blender", |b| {
            let before = b.scene.state();
            let changed = primitives::build(Primitive::Cone, 900, 0.5);
            assert!(b.scene.replace_mesh_of(b.ball, changed, &mut b.mesh));
            b.history.push_scene(before);
        }),
        ("a tool that hands the object a new mesh (trim, split, a tube fused, keys)", |b| {
            // What `app/tools.rs` does with a mesh a tool sends back: it
            // arrives on an object of its own, and the object takes it over.
            let before = b.scene.state();
            let old = b.scene.get(b.ball).unwrap().mesh;
            let carrier = b.scene.add("mesh", primitives::build(Primitive::Capsule, 800, 0.4), &mut b.mesh);
            let new = b.scene.get(carrier).unwrap().mesh;
            b.scene.objects.iter_mut().filter(|o| o.mesh == old).for_each(|o| o.mesh = new);
            b.scene.select(b.ball, &mut b.mesh);
            assert!(b.scene.remove(carrier, &mut b.mesh));
            b.history.push_scene(before);
        }),
        ("a model imported in its place", |b| {
            let imported = Mesh::new(b.mesh.positions.clone(), b.mesh.indices.clone(), Some(b.mesh.mask.clone()));
            b.replace(imported);
        }),
        ("a rebuild from a coarser level", |b| {
            // On level 1 of 3, as when Remesh is pressed there: the fine levels go with the rest.
            let low = stepped(&b.mesh, 0).unwrap().0;
            b.replace(low);
            let rebuilt = remesh::voxel_remesh(&b.mesh, 40, [false; 3]).unwrap();
            b.replace(rebuilt);
        }),
    ]
}

#[test]
fn whatever_rebuilds_the_mesh_drops_the_levels_and_undo_brings_them_back() {
    let commands = rebuilders();
    let mut bench = Bench::new();
    let sculpted = bench.print();
    assert!(sculpted != 0 && bench.ball_mesh().level_count() == 3);
    for (name, command) in commands {
        let steps = bench.history.steps().0;
        command(&mut bench);
        bench.history.target = bench.scene.active_mesh();
        let now = bench.ball_mesh();
        assert!(now.levels.is_none() && now.triangle_count() > 0, "{name}: the new mesh has no levels");
        bench.undo_to(steps);
        let back = bench.ball_mesh();
        assert_eq!(back.level_count(), 3, "{name}: Undo brings the levels back");
        assert_eq!(bench.print(), sculpted, "{name}: and every vertex, mask value and hidden face with them");
        assert_eq!(back.hidden_triangle_count(), 5, "{name}");
        assert_eq!(bench.scene.objects.len(), 1, "{name}: the scene is as it was");
    }
    // After all of it the levels still work: down and up is the sculpt.
    let low = stepped(&bench.mesh, 0).unwrap().0;
    let up = stepped(&low, 2).unwrap().0;
    assert_eq!(up.levels.as_ref().unwrap().print(&up), sculpted);
}

#[test]
fn what_does_not_rebuild_the_mesh_leaves_the_levels_alone() {
    let mut bench = Bench::new();
    let sculpted = bench.print();

    // Moving every vertex (a stroke, Reset Shape), the mask, hiding and showing.
    let verts: Vec<u32> = (0..bench.mesh.vertex_count() as u32).collect();
    let before = bench.mesh.positions.clone();
    let after = before.iter().map(|&p| p * 1.1).collect();
    let stroke = StrokeDelta::Positions { verts, before, after, topology: None };
    bench.mesh.apply_delta(&stroke, false);
    bench.history.push_stroke(stroke);
    let mask = bench.mesh.remap_mask(|_, _, mask| 1.0 - mask).expect("the mask changed");
    bench.history.push_stroke(mask);
    let shown = bench.mesh.show_all().expect("something was hidden");
    bench.history.push_visibility(shown);
    assert_eq!(bench.mesh.level_count(), 3);
    assert_ne!(bench.print(), sculpted);
    bench.undo_to(0);
    assert_eq!(bench.print(), sculpted);

    // Set Origin to Mesh rebuilds the mesh with every vertex shifted; the levels come across.
    let uid = bench.mesh.uid;
    bench.mesh.positions.iter_mut().for_each(|p| p.x += 0.25);
    let shifted = bench.print();
    assert!(origin::move_origin(&mut bench.scene, &mut bench.mesh, &mut bench.history).is_some());
    assert!(bench.mesh.uid != uid && bench.mesh.level_count() == 3, "a new mesh, with the levels");
    let low = stepped(&bench.mesh, 0).unwrap().0;
    assert_eq!(stepped(&low, 2).unwrap().0.level_count(), 3);
    bench.undo_to(0);
    assert_eq!(bench.print(), shifted);
    bench.mesh.positions.iter_mut().for_each(|p| p.x -= 0.25);

    // A copy keeps the levels as its own; an instance shares the original's.
    let print = bench.print();
    let copy = bench.scene.clone_active(&mut bench.mesh);
    assert_ne!(bench.scene.get(copy).unwrap().mesh, bench.scene.get(bench.ball).unwrap().mesh);
    assert_eq!(bench.mesh.levels.as_ref().map(|l| l.print(&bench.mesh)), Some(print), "the copy, in hand");
    let lower = stepped(&bench.mesh, 0).unwrap().0;
    bench.replace(lower);
    assert_eq!(bench.print(), print, "stepping the copy down leaves the original on its level");
    bench.scene.select(bench.ball, &mut bench.mesh);
    let twin = bench.scene.instance_active();
    assert_eq!(bench.scene.get(twin).unwrap().mesh, bench.scene.get(bench.ball).unwrap().mesh);
    assert!(bench.scene.make_unique(&mut bench.mesh));
    assert_ne!(bench.scene.get(twin).unwrap().mesh, bench.scene.get(bench.ball).unwrap().mesh);
    assert_eq!(
        bench.mesh.levels.as_ref().map(|l| l.print(&bench.mesh)),
        Some(print),
        "made unique, it has the same levels"
    );
}

/// Moves every vertex `to` gives a new place, as one stroke through the
/// mesh's own stroke routines.
fn stroke(bench: &mut Bench, to: impl Fn(Vec3) -> Option<Vec3> + Sync) {
    let leaves: Vec<u32> = (0..bench.mesh.bvh.leaves.len() as u32).collect();
    bench.scene.keep_baseline(&bench.mesh);
    bench.mesh.begin_stroke();
    bench.mesh.move_vertices(&leaves, |_, vertex| to(vertex.pos));
    bench.mesh.positions_changed(&leaves);
    let delta = bench.mesh.end_stroke(false).expect("the stroke moved vertices");
    bench.history.push_stroke(delta);
}

/// The bench's ball with two sculpt layers on its finest level: fine detail
/// at 60%, and a bump that is hidden. Gives back their numbers.
fn layered(bench: &mut Bench) -> (u32, u32) {
    bench.scene.keep_baseline(&bench.mesh);
    let (detail, step) = bench.mesh.add_layer(Some("Detail".into()));
    bench.history.push_layers(step);
    stroke(bench, |p| Some(p + p.normalize() * 0.01 * (p.x * 40.0).sin() * (p.y * 31.0).cos()));
    let step = bench.mesh.set_layer(detail, true, 60).unwrap();
    bench.history.push_layers(step);
    let (bump, step) = bench.mesh.add_layer(Some("Bump".into()));
    bench.history.push_layers(step);
    stroke(bench, |p| (p.z > 0.4).then(|| p * 1.04));
    let step = bench.mesh.set_layer(bump, false, 100).unwrap();
    bench.history.push_layers(step);
    (detail, bump)
}

/// The shape as a list that does not depend on how the mesh is numbered.
fn sorted(points: &[Vec3]) -> Vec<[u32; 3]> {
    let mut bits: Vec<[u32; 3]> = points.iter().map(|p| p.to_array().map(f32::to_bits)).collect();
    bits.sort_unstable();
    bits
}

#[test]
fn whatever_rebuilds_the_mesh_merges_the_layers_into_it_and_undo_brings_them_back() {
    let mut bench = Bench::new();
    layered(&mut bench);
    let before = bench.ball_mesh().layers_print();
    let shape = sorted(&bench.ball_mesh().positions);
    for (name, command) in rebuilders() {
        let steps = bench.history.steps().0;
        command(&mut bench);
        bench.history.target = bench.scene.active_mesh();
        let now = bench.ball_mesh();
        assert!(now.layers.is_none() && now.triangle_count() > 0, "{name}: the new mesh has no layers");
        if name == "a model imported in its place" {
            // The one rebuild that keeps every vertex: they are where the layers had them.
            assert!(sorted(&now.positions) == shape, "{name}: the shape has the layers in it");
        }
        bench.undo_to(steps);
        let back = bench.ball_mesh();
        assert_eq!(back.layer_count(), 2, "{name}: Undo brings the layers back");
        assert_eq!(back.layers_print(), before, "{name}: with every offset, and the shape, to the bit");
        assert_eq!(bench.scene.objects.len(), 1, "{name}: the scene is as it was");
    }
    // After all of it the layers still work, through a change of level and back.
    let low = stepped(&bench.mesh, 0).unwrap().0;
    assert_eq!(stepped(&low, 2).unwrap().0.layers_print(), before);
}

#[test]
fn what_does_not_rebuild_the_mesh_keeps_the_layers() {
    let apart = |a: &[Vec3], b: &[Vec3]| a.iter().zip(b).map(|(a, b)| a.distance(*b)).fold(0.0, f32::max);
    let mut bench = Bench::new();
    let rest = bench.mesh.positions.clone();
    let (detail, bump) = layered(&mut bench);
    let (print, steps) = (bench.mesh.layers_print(), bench.history.steps().0);
    let shape = bench.mesh.positions.clone();

    // The mask, hiding and showing.
    let mask = bench.mesh.remap_mask(|_, _, mask| 1.0 - mask).expect("the mask changed");
    bench.history.push_stroke(mask);
    let shown = bench.mesh.show_all().expect("something was hidden");
    bench.history.push_visibility(shown);
    assert!(bench.mesh.layer_count() == 2 && bench.mesh.positions == shape, "they move nothing and lose nothing");
    bench.undo_to(steps);

    // A stroke on the model underneath (the whole of it turned, as a pose
    // turns a limb), then Reset Shape: the model underneath goes back to
    // where it was built, and the layers stay on it as they were.
    bench.history.push_layers(bench.mesh.record_on_layer(None).expect("the bump was recording"));
    let turn = glam::Quat::from_axis_angle(Vec3::new(0.3, 1.0, 0.2).normalize(), 1.2);
    stroke(&mut bench, |p| Some(turn * p + Vec3::new(0.1, -0.05, 0.02)));
    let turned: Vec<Vec3> = rest.iter().map(|&p| turn * p + Vec3::new(0.1, -0.05, 0.02)).collect();
    assert!(apart(&bench.mesh.model_underneath(), &turned) < 2e-6, "the stroke moved the model underneath");
    let reset = bench.scene.reset_shape(&mut bench.mesh).expect("there was something to reset");
    bench.history.push_stroke(reset);
    assert_eq!(bench.mesh.layer_count(), 2, "Reset Shape keeps the layers");
    assert!(apart(&bench.mesh.model_underneath(), &rest) < 1e-6, "the model underneath is back at rest");
    assert!(apart(&bench.mesh.positions, &shape) < 2e-6, "and the layers on it are as they were");
    assert!(bench.scene.reset_shape(&mut bench.mesh).is_none(), "at rest, there is nothing more to reset");
    bench.undo_to(steps);
    assert_eq!(bench.mesh.layers_print(), print, "Undo of the reset, the stroke and the dot: every bit");

    // Set Origin to Mesh rebuilds the mesh with every vertex shifted; the layers come across.
    let uid = bench.mesh.uid;
    bench.mesh.positions.iter_mut().for_each(|p| p.x += 0.25);
    let shifted = bench.mesh.layers_print();
    assert!(origin::move_origin(&mut bench.scene, &mut bench.mesh, &mut bench.history).is_some());
    assert!(bench.mesh.uid != uid && bench.mesh.layer_count() == 2, "a new mesh, with the layers");
    let with = bench.mesh.positions.clone();
    bench.mesh.set_layer(bump, true, 100).expect("the layers work on the new mesh");
    assert!(apart(&bench.mesh.positions, &with) > 0.01);
    bench.mesh.set_layer(bump, false, 100).unwrap();
    bench.undo_to(steps);
    assert_eq!(bench.mesh.layers_print(), shifted, "Undo is the mesh it was made from, layers and all");
    bench.mesh.positions.clone_from(&shape);
    assert_eq!(bench.mesh.layers_print(), print);

    // A copy keeps the layers as its own; an instance shares the original's.
    let copy = bench.scene.clone_active(&mut bench.mesh);
    assert_ne!(bench.scene.get(copy).unwrap().mesh, bench.scene.get(bench.ball).unwrap().mesh);
    assert_eq!(bench.mesh.layers_print(), print, "the copy, in hand");
    bench.mesh.set_layer(detail, false, 60).unwrap();
    assert_eq!(bench.ball_mesh().layers_print(), print, "hiding the copy's layer leaves the original's showing");
    bench.scene.select(bench.ball, &mut bench.mesh);
    let twin = bench.scene.instance_active();
    assert_eq!(bench.scene.get(twin).unwrap().mesh, bench.scene.get(bench.ball).unwrap().mesh);
    assert!(bench.scene.make_unique(&mut bench.mesh));
    assert_eq!(bench.mesh.layers_print(), print, "made unique, it has the same layers");
}

#[test]
fn the_flat_tick_is_remembered_and_older_settings_still_read() {
    use crate::prefs::Prefs;
    assert!(!Prefs::default().subdivide_flat, "Subdivide rounds the model unless told otherwise");
    let kept = Prefs { subdivide_flat: true, ..Prefs::default() };
    let text = serde_json::to_string(&kept).unwrap();
    assert!(Prefs::parse(&text).subdivide_flat);
    // A settings file from before the tick was kept: what it holds is read, and the tick is off.
    let mut older: serde_json::Value = serde_json::from_str(&text).unwrap();
    older.as_object_mut().unwrap().remove("subdivide_flat");
    older["remesh_resolution"] = 77.0.into();
    let read = Prefs::parse(&older.to_string());
    assert!(!read.subdivide_flat && read.remesh_resolution == 77.0);
}
