//! Sculpt layers on a mesh: recording, strength, undo, the model underneath,
//! dynamic topology.

use glam::{Mat3, Quat, Vec2};

use super::*;
use crate::camera::Camera;
use crate::history::History;
use crate::mesh::primitives::{self, Primitive};
use crate::sculpt::{Brush, PointerSample, Sculptor};

fn ball() -> Mesh {
    primitives::build(Primitive::Sphere, 6_000, 0.5)
}

fn bits(points: &[Vec3]) -> Vec<[u32; 3]> {
    points.iter().map(|p| p.to_array().map(f32::to_bits)).collect()
}

/// Everything about a mesh that layers can change: its positions, its rows
/// and every layer's offsets, to the bit.
fn state(mesh: &Mesh) -> (Vec<[u32; 3]>, Vec<LayerRow>, Vec<Vec<[u32; 3]>>) {
    let offsets = mesh.layers.iter().flat_map(|layers| &layers.list).map(|layer| bits(&layer.offsets)).collect();
    (bits(&mesh.positions), mesh.layer_rows(), offsets)
}

fn offsets(mesh: &Mesh, id: u32) -> Vec<Vec3> {
    mesh.layers.as_ref().and_then(|layers| layers.offsets(id)).expect("the layer is there").to_vec()
}

/// The furthest two shapes are apart at any vertex.
fn apart(a: &[Vec3], b: &[Vec3]) -> f32 {
    assert_eq!(a.len(), b.len());
    a.iter().zip(b).map(|(a, b)| a.distance(*b)).fold(0.0, f32::max)
}

/// Moves every vertex `shape` gives a new place, as one stroke.
fn stroke(mesh: &mut Mesh, shape: impl Fn(Vec3) -> Option<Vec3> + Sync) -> Option<StrokeDelta> {
    let leaves: Vec<u32> = (0..mesh.bvh.leaves.len() as u32).collect();
    mesh.begin_stroke();
    mesh.move_vertices(&leaves, |_, vertex| shape(vertex.pos));
    mesh.positions_changed(&leaves);
    mesh.end_stroke(false)
}

/// A bump on the front of the ball: every vertex within `radius` of the
/// point at +Z raised along its own direction from the middle.
fn bump(height: f32, radius: f32) -> impl Fn(Vec3) -> Option<Vec3> + Sync {
    move |p: Vec3| {
        let d = p.distance(Vec3::Z * 0.5) / radius;
        (d < 1.0).then(|| p + p.normalize() * height * (1.0 - d * d).powi(2))
    }
}

/// One real brush stroke across the middle of the view from +Z.
fn brushed(mesh: &mut Mesh, sculptor: &mut Sculptor, across: f32) -> Option<StrokeDelta> {
    let size = Vec2::new(1600.0, 900.0);
    let mut camera = Camera::default();
    camera.frame(mesh.bounds(), size.x / size.y, false);
    let view = camera.view(Vec2::ZERO, size, camera.distance, 1.0);
    let sample = |i: usize| PointerSample {
        pos: size * 0.5 + Vec2::new(-across + 2.0 * across * i as f32 / 40.0, 15.0),
        pressure: None,
        time: i as f64 / 240.0,
    };
    assert!(sculptor.begin(mesh, &view, sample(0), false, false), "the stroke missed the mesh");
    for i in 1..=40 {
        sculptor.sample(mesh, &view, sample(i));
        if i % 4 == 0 {
            sculptor.frame(mesh, &view, 1.0 / 60.0);
            let _ = mesh.take_dirty();
        }
    }
    let delta = sculptor.end(mesh, &view);
    let _ = mesh.take_dirty();
    delta
}

fn sculptor(brush: Brush, radius: f32, mirror: bool) -> Sculptor {
    let mut sculptor = Sculptor::default();
    sculptor.settings.brush = brush;
    sculptor.settings.symmetry = [mirror, false, false];
    sculptor.settings.radius_px = radius;
    sculptor.settings.per_brush[brush.index()].strength = 0.8;
    sculptor
}

#[test]
fn a_recorded_stroke_is_the_shape_at_full_strength_and_gone_at_none() {
    let mut mesh = ball();
    let underneath = mesh.positions.clone();
    let (layer, _) = mesh.add_layer(None);
    assert_eq!(mesh.layer_rows(), [LayerRow { id: layer, name: "Layer 1".into(), on: true, percent: 100, recording: true }]);
    let delta = stroke(&mut mesh, bump(0.05, 0.2)).expect("the stroke moved vertices");
    assert!(matches!(delta, StrokeDelta::Layered { .. }), "the stroke's record holds what the layer took");
    let sculpted = mesh.positions.clone();
    assert!(apart(&sculpted, &underneath) > 0.049);
    // Recording changes no position: at 100% the shape is the one sculpted, to the bit.
    assert_eq!(bits(&sculpted), bits(&ball().positions.iter().map(|&p| bump(0.05, 0.2)(p).unwrap_or(p)).collect::<Vec<_>>()));
    // And the layer holds exactly what moved.
    let held = offsets(&mesh, layer);
    assert!((0..held.len()).all(|v| held[v] == sculpted[v] - underneath[v]));
    assert_eq!(bits(&mesh.model_underneath()), bits(&underneath), "at 100% over a model that did not move: exact");

    // To 0%: the model underneath, to within one rounding of each coordinate.
    let one_rounding = f32::EPSILON * 0.5;
    mesh.set_layer(layer, true, 0).expect("the strength changed");
    let off = apart(&mesh.positions, &underneath);
    println!("a layer taken to 0% leaves the model underneath to within {off:e}");
    assert!(off <= one_rounding, "{off}");
    // The eye is the same thing.
    mesh.set_layer(layer, true, 100).unwrap();
    assert!(apart(&mesh.positions, &sculpted) <= one_rounding);
    mesh.set_layer(layer, false, 100).unwrap();
    assert!(apart(&mesh.positions, &underneath) <= one_rounding);
    // Halfway is halfway.
    mesh.set_layer(layer, true, 50).unwrap();
    let half: Vec<Vec3> = underneath.iter().zip(&sculpted).map(|(a, b)| (*a + *b) * 0.5).collect();
    assert!(apart(&mesh.positions, &half) <= 2.0 * one_rounding);
    // However often the slider goes up and down, nothing wanders off.
    for percent in [100, 0, 37, 100, 3, 64, 0, 100, 12, 100].repeat(5) {
        mesh.set_layer(layer, true, percent);
    }
    let drift = apart(&mesh.positions, &sculpted);
    println!("fifty changes of strength later the shape is {drift:e} from the one sculpted");
    assert!(drift < 1e-6, "{drift}");
}

#[test]
fn a_stroke_recorded_below_full_strength_shows_as_sculpted_and_is_stored_larger() {
    let mut mesh = ball();
    let underneath = mesh.positions.clone();
    let (layer, _) = mesh.add_layer(Some("Pores".into()));
    mesh.set_layer(layer, true, 25).unwrap();
    stroke(&mut mesh, bump(0.02, 0.2));
    let sculpted = mesh.positions.clone();
    // What is on screen is what was sculpted, exactly; the layer holds it four times over.
    let front = (0..sculpted.len()).max_by(|&a, &b| sculpted[a].z.total_cmp(&sculpted[b].z)).unwrap();
    assert!((sculpted[front].z - underneath[front].z - 0.02).abs() < 1e-4);
    assert!((offsets(&mesh, layer)[front].length() - 0.08).abs() < 4e-4);
    // So at 100% it stands four times as high, and at 1%, the lowest it can record at, a hundred times.
    mesh.set_layer(layer, true, 100).unwrap();
    assert!((mesh.positions[front].z - underneath[front].z - 0.08).abs() < 4e-4);
    mesh.set_layer(layer, true, 1).unwrap();
    stroke(&mut mesh, bump(0.001, 0.2));
    assert!(offsets(&mesh, layer).iter().all(|offset| offset.is_finite() && offset.length() < 0.2));

    // Off or at nothing, it cannot take a stroke, and says why; mask painting has nothing to do with it.
    assert!(mesh.layer_refusal().is_none());
    mesh.set_layer(layer, true, 0).unwrap();
    assert!(mesh.layer_refusal().unwrap().contains("\"Pores\" is recording but is at 0%"));
    mesh.set_layer(layer, false, 60).unwrap();
    assert!(mesh.layer_refusal().unwrap().contains("is switched off"));
    mesh.record_on_layer(None).unwrap();
    assert!(mesh.layer_refusal().is_none(), "with nothing recording, strokes go to the model underneath");
    assert!(ball().layer_refusal().is_none());
}

#[test]
fn layers_are_independent_and_bake_keeps_what_delete_takes_away() {
    let mut mesh = ball();
    let underneath = mesh.positions.clone();
    let (a, _) = mesh.add_layer(Some("Swelling".into()));
    stroke(&mut mesh, bump(0.05, 0.3));
    let with_a = mesh.positions.clone();
    let (b, _) = mesh.add_layer(None);
    assert_eq!(mesh.layers.as_ref().unwrap().recording().map(|row| row.id), Some(b), "a new layer is the one that records");
    stroke(&mut mesh, |p| (p.y > 0.2).then(|| p * 1.05));
    let with_both = mesh.positions.clone();
    assert!(offsets(&mesh, a).iter().zip(&with_a).zip(&underneath).all(|((o, w), u)| *o == *w - *u), "the first layer was not touched");

    // Either can go without the other.
    mesh.set_layer(a, false, 100).unwrap();
    let only_b: Vec<Vec3> = (0..underneath.len()).map(|v| underneath[v] + (with_both[v] - with_a[v])).collect();
    assert!(apart(&mesh.positions, &only_b) < 2e-7);
    mesh.set_layer(a, true, 100).unwrap();
    mesh.set_layer(b, false, 100).unwrap();
    assert!(apart(&mesh.positions, &with_a) < 2e-7);
    mesh.set_layer(b, true, 100).unwrap();

    // Delete takes the layer's part of the shape with it.
    let before = state(&mesh);
    let mut step = mesh.remove_layer(a, false).expect("there was such a layer");
    assert_eq!(mesh.layer_rows().iter().map(|row| row.id).collect::<Vec<_>>(), [b]);
    assert!(apart(&mesh.positions, &only_b) < 2e-7);
    step.swap(&mut mesh);
    assert_eq!(state(&mesh), before, "and Undo puts back every bit of it");

    // Bake keeps the shape and forgets the layer.
    let shape = mesh.positions.clone();
    mesh.remove_layer(a, true).unwrap();
    assert_eq!(bits(&mesh.positions), bits(&shape));
    mesh.set_layer(b, false, 100).unwrap();
    assert!(apart(&mesh.positions, &with_a) < 2e-7, "what was baked is part of the model underneath now");
    mesh.set_layer(b, true, 100).unwrap();
    let mut step = mesh.bake_all_layers().expect("one layer left to bake");
    assert!(mesh.layers.is_none() && bits(&mesh.positions) == bits(&shape));
    assert!(mesh.bake_all_layers().is_none() && mesh.remove_layer(b, true).is_none());
    step.swap(&mut mesh);
    assert_eq!(mesh.layer_rows().len(), 1);
}

/// Every kind of change to the layers, recorded as the app records them,
/// then stepped all the way back and all the way forward: the positions,
/// the rows and every offset are the same to the bit at each step.
#[test]
fn undo_and_redo_put_back_every_bit() {
    let mut mesh = ball();
    let mut history = History::default();
    let mut seen = vec![state(&mesh)];
    let note = |mesh: &Mesh, seen: &mut Vec<_>| seen.push(state(mesh));

    let (a, step) = mesh.add_layer(None);
    history.push_layers(step);
    note(&mesh, &mut seen);
    history.push_stroke(stroke(&mut mesh, bump(0.05, 0.25)).unwrap());
    note(&mesh, &mut seen);
    let (b, step) = mesh.add_layer(Some("Scar".into()));
    history.push_layers(step);
    note(&mesh, &mut seen);
    history.push_stroke(brushed(&mut mesh, &mut sculptor(Brush::Crease, 30.0, true), 120.0).unwrap());
    note(&mesh, &mut seen);
    // The slider, dragged through several values and let go: one step.
    let drag = mesh.drag_layer(a).unwrap();
    for percent in [90, 71, 40, 55] {
        mesh.drag_layer_to(&drag, true, percent);
    }
    history.push_layers(mesh.end_layer_drag(drag).unwrap());
    note(&mesh, &mut seen);
    history.push_layers(mesh.set_layer(b, false, 100).unwrap());
    note(&mesh, &mut seen);
    history.push_layers(mesh.rename_layer(a, "Brow").unwrap());
    note(&mesh, &mut seen);
    history.push_layers(mesh.record_on_layer(None).unwrap());
    note(&mesh, &mut seen);
    // With nothing recording: a stroke on the model underneath, which turns the layers' offsets with it.
    history.push_stroke(brushed(&mut mesh, &mut sculptor(Brush::Inflate, 90.0, false), 60.0).unwrap());
    note(&mesh, &mut seen);
    history.push_layers(mesh.record_on_layer(Some(a)).unwrap());
    note(&mesh, &mut seen);
    history.push_stroke(brushed(&mut mesh, &mut sculptor(Brush::Standard, 40.0, false), 80.0).unwrap());
    note(&mesh, &mut seen);
    history.push_layers(mesh.remove_layer(b, false).unwrap());
    note(&mesh, &mut seen);
    history.push_layers(mesh.remove_layer(a, true).unwrap());
    note(&mesh, &mut seen);
    assert!(mesh.layers.is_none());
    for at in 1..seen.len() {
        assert_ne!(seen[at], seen[at - 1], "step {at} changed something");
    }

    for at in (0..seen.len() - 1).rev() {
        assert_eq!(history.undo(&mut mesh), Some(false));
        assert!(state(&mesh) == seen[at], "undo to step {at}");
    }
    assert!(mesh.layers.is_none() && !history.can_undo());
    for at in 1..seen.len() {
        assert_eq!(history.redo(&mut mesh), Some(false));
        assert!(state(&mesh) == seen[at], "redo to step {at}");
    }
    assert!(history.bytes() > 0);
}

#[test]
fn a_change_that_is_no_change_is_no_undo_step() {
    let mut mesh = ball();
    assert!(mesh.rename_layer(1, "x").is_none() && mesh.set_layer(1, true, 5).is_none() && mesh.drag_layer(1).is_none());
    let (a, _) = mesh.add_layer(None);
    stroke(&mut mesh, bump(0.05, 0.25));
    let before = state(&mesh);
    assert!(mesh.set_layer(a, true, 100).is_none());
    assert!(mesh.rename_layer(a, "Layer 1").is_none() && mesh.rename_layer(a, "  ").is_none());
    assert!(mesh.record_on_layer(Some(a)).is_none() && mesh.record_on_layer(Some(99)).is_none());
    // A slider dragged away and back to where it was moves nothing, exactly, and records nothing.
    let drag = mesh.drag_layer(a).unwrap();
    mesh.drag_layer_to(&drag, true, 20);
    mesh.drag_layer_to(&drag, true, 100);
    assert!(mesh.end_layer_drag(drag).is_none());
    assert_eq!(state(&mesh), before);
    // Nor is a stroke that moved nothing a layered one.
    assert!(stroke(&mut mesh, |_| None).is_none());
    // A second layer takes the next name and the next number.
    let (b, _) = mesh.add_layer(None);
    assert_eq!((b, mesh.layer_rows()[1].name.as_str()), (a + 1, "Layer 2"));
    assert_eq!(mesh.layers.as_ref().unwrap().bytes(), 2 * mesh.vertex_count() * 12 + "Layer 1Layer 2".len());
    assert!(mesh.memory_bytes() > ball().memory_bytes() + 2 * mesh.vertex_count() * 12);
}

/// A stroke on the model underneath that turns the surface turns the
/// layers' offsets with it: afterwards a layer switched off leaves the model
/// underneath moved as the stroke moved it, with none of the layer's detail
/// left behind pointing the old way.
#[test]
fn a_stroke_underneath_turns_the_layers_with_the_surface() {
    let mut mesh = ball();
    let underneath = mesh.positions.clone();
    let (layer, _) = mesh.add_layer(None);
    stroke(&mut mesh, |p| Some(p + p.normalize() * 0.02 * (p.x * 40.0).sin() * (p.y * 31.0).cos()));
    let held = offsets(&mesh, layer);
    mesh.record_on_layer(None);

    // The whole model turned and shifted, as the Pose brush turns a limb.
    let turn = Quat::from_axis_angle(Vec3::new(0.3, 1.0, 0.2).normalize(), 1.2);
    let moved = |p: Vec3| turn * p + Vec3::new(0.1, -0.05, 0.02);
    let sculpted: Vec<Vec3> = mesh.positions.iter().map(|&p| moved(p)).collect();
    let delta = stroke(&mut mesh, |p| Some(moved(p))).unwrap();
    assert!(matches!(delta, StrokeDelta::Layered { .. }));
    assert_eq!(bits(&mesh.positions), bits(&sculpted), "carrying the layers along moves nothing on screen");
    let now = offsets(&mesh, layer);
    let off = (0..held.len()).map(|v| now[v].distance(turn * held[v])).fold(0.0, f32::max);
    assert!(off < 1e-6, "every offset turned with the surface ({off})");
    mesh.set_layer(layer, false, 100).unwrap();
    let expected: Vec<Vec3> = underneath.iter().map(|&p| moved(p)).collect();
    let ghost = apart(&mesh.positions, &expected);
    println!("after a turn of 1.2 rad underneath, the layer switched off leaves {ghost:e} of itself behind");
    assert!(ghost < 2e-6, "{ghost}");

    // With a layer recording, the same move is that layer's and nothing is turned.
    let mut mesh = ball();
    let (first, _) = mesh.add_layer(None);
    stroke(&mut mesh, bump(0.05, 0.25));
    let held = offsets(&mesh, first);
    let (pose, _) = mesh.add_layer(None);
    stroke(&mut mesh, |p| Some(moved(p)));
    assert_eq!(bits(&offsets(&mesh, first)), bits(&held));
    mesh.set_layer(pose, false, 100).unwrap();
    let back: Vec<Vec3> = ball().positions.iter().map(|&p| bump(0.05, 0.25)(p).unwrap_or(p)).collect();
    assert!(apart(&mesh.positions, &back) < 2e-7, "switch the pose off and the model is where it was");
}

/// The gizmo, with part of the model masked, moves what is not masked by a
/// matrix. The layers' offsets go by the same matrix, as far as each vertex
/// went: a limb made twice the size has detail twice the size.
#[test]
fn a_move_by_a_matrix_carries_the_offsets_by_that_matrix() {
    let mut mesh = ball();
    let (layer, _) = mesh.add_layer(None);
    stroke(&mut mesh, |p| Some(p + p.normalize() * 0.02 * (p.x * 40.0).sin()));
    mesh.record_on_layer(None);
    let held = offsets(&mesh, layer);
    for v in 0..mesh.mask.len() {
        mesh.mask[v] = (0.5 - mesh.positions[v].y * 2.0).clamp(0.0, 1.0);
    }
    let matrix = Mat3::from_quat(Quat::from_rotation_z(0.7)) * Mat3::from_diagonal(Vec3::new(2.0, 1.5, 1.0));
    let all: Vec<u32> = (0..mesh.vertex_count() as u32).collect();
    let leaves = mesh.leaves_of(&all);
    mesh.begin_stroke();
    mesh.move_listed(&all, |_, vertex| Some(vertex.start + (matrix * vertex.start - vertex.start) * (1.0 - vertex.mask)));
    mesh.positions_changed(&leaves);
    let delta = mesh.end_stroke_moved(matrix).unwrap();
    let now = offsets(&mesh, layer);
    for v in 0..held.len() {
        let free = 1.0 - mesh.mask[v];
        let expected = held[v] + (matrix * held[v] - held[v]) * free;
        assert!(now[v].distance(expected) < 1e-6, "vertex {v}, {free} free");
    }
    // Undo is exact here too.
    let after = state(&mesh);
    mesh.apply_delta(&delta, true);
    assert_eq!(bits(&offsets(&mesh, layer)), bits(&held));
    mesh.apply_delta(&delta, false);
    assert_eq!(state(&mesh), after);
}

/// Index of each vertex's mirror image across X.
fn twins(mesh: &Mesh) -> Vec<usize> {
    let key = |p: Vec3| [(p.x + 0.0).to_bits(), (p.y + 0.0).to_bits(), (p.z + 0.0).to_bits()];
    let at: std::collections::HashMap<[u32; 3], usize> = mesh.positions.iter().enumerate().map(|(v, &p)| (key(p), v)).collect();
    mesh.positions.iter().map(|&p| at[&key(p * Vec3::new(-1.0, 1.0, 1.0))]).collect()
}

#[test]
fn mirrored_strokes_leave_mirrored_layers() {
    let mut mesh = ball();
    let twin = twins(&mesh);
    let mirrored = |p: Vec3| p * Vec3::new(-1.0, 1.0, 1.0);
    let lopsided = |list: &[Vec3]| (0..list.len()).map(|v| list[v].distance(mirrored(list[twin[v]]))).fold(0.0, f32::max);

    let (layer, _) = mesh.add_layer(None);
    brushed(&mut mesh, &mut sculptor(Brush::Clay, 40.0, true), 110.0);
    brushed(&mut mesh, &mut sculptor(Brush::Move, 70.0, true), 60.0);
    let recorded = lopsided(&offsets(&mesh, layer));
    assert!(lopsided(&mesh.positions) < 1e-6 && recorded < 1e-6, "what a mirrored stroke records is mirrored ({recorded})");
    assert!(offsets(&mesh, layer).iter().filter(|offset| **offset != Vec3::ZERO).count() > 60);

    // And a mirrored stroke on the model underneath turns the two sides' offsets alike.
    mesh.record_on_layer(None);
    brushed(&mut mesh, &mut sculptor(Brush::Move, 110.0, true), 90.0);
    brushed(&mut mesh, &mut sculptor(Brush::Inflate, 80.0, true), 100.0);
    let turned = lopsided(&offsets(&mesh, layer));
    println!("mirrored strokes: offsets recorded {recorded:e} and turned {turned:e} from mirror images of each other");
    assert!(turned < 1e-6, "{turned}");
    mesh.set_layer(layer, false, 100);
    assert!(lopsided(&mesh.positions) < 2e-6, "so the model underneath is mirrored too");
}

/// The longest edge of the mesh.
fn longest_edge(mesh: &Mesh) -> f32 {
    let side = |a: u32, b: u32| mesh.positions[a as usize].distance(mesh.positions[b as usize]);
    mesh.indices.chunks_exact(3).map(|t| side(t[0], t[1]).max(side(t[1], t[2])).max(side(t[2], t[0]))).fold(0.0, f32::max)
}

#[test]
fn a_split_gives_the_new_vertex_its_share_of_every_layer() {
    let mut mesh = primitives::build(Primitive::Sphere, 1_500, 0.5);
    let underneath = mesh.positions.clone();
    let (a, _) = mesh.add_layer(None);
    stroke(&mut mesh, |p| Some(p * (1.0 + 0.2 * p.y)));
    let (b, _) = mesh.add_layer(None);
    stroke(&mut mesh, bump(0.08, 0.4));
    mesh.set_layer(b, true, 50).unwrap();
    let (old, coarse) = (mesh.vertex_count(), longest_edge(&mesh));
    let before = state(&mesh);

    // Refine the front with nothing moved: the shape must not jump, and the layers must cover the new vertices.
    let mut history = History::default();
    mesh.begin_stroke();
    let stats = mesh.refine(&[Vec3::Z * 0.5], 0.3, coarse * 0.3, 10_000, usize::MAX);
    let delta = mesh.end_stroke(false).expect("the refine is the stroke's record");
    assert!(stats.vertices_added > 50 && matches!(delta, StrokeDelta::Layered { .. }));
    assert_eq!(bits(&mesh.positions[..old]), before.0, "no vertex that was there moved");
    for layer in [a, b] {
        let offsets = offsets(&mesh, layer);
        assert_eq!(offsets.len(), mesh.vertex_count());
        assert_eq!(bits(&offsets[..old]), bits(&offsets_of(&before, layer)));
        for v in old..mesh.vertex_count() {
            let [p, q] = mesh.split_parents(v as u32).unwrap();
            assert_eq!(offsets[v], (offsets[p as usize] + offsets[q as usize]) * 0.5, "vertex {v} of layer {layer}");
        }
    }
    // Both layers off: the model underneath, with each new vertex near the
    // middle of the edge it split there (as near as the split itself put it
    // on the shape that was showing), and not a spike anywhere.
    let showing = mesh.positions.clone();
    mesh.set_layer(a, false, 100);
    mesh.set_layer(b, false, 50);
    assert!(apart(&mesh.positions[..old], &underneath) < 2e-7);
    let fine = longest_edge(&mesh);
    for v in old..mesh.vertex_count() {
        let [p, q] = mesh.split_parents(v as u32).unwrap();
        let middle = |list: &[Vec3]| (list[p as usize] + list[q as usize]) * 0.5;
        let (under, shown) = (mesh.positions[v] - middle(&mesh.positions), showing[v] - middle(&showing));
        assert!(under.distance(shown) < 1e-6, "vertex {v} sits off its edge underneath as it does on the shape");
        assert!((mesh.positions[v].length() - 0.5).abs() < 0.01, "vertex {v} is on the ball underneath");
    }
    assert!(fine < coarse, "{fine} against {coarse}");
    mesh.set_layer(b, true, 50);
    mesh.set_layer(a, true, 100);

    // Undo takes the new vertices' offsets away with the vertices; redo brings both back.
    let after = state(&mesh);
    history.push_stroke(delta);
    history.undo(&mut mesh).unwrap();
    assert!(mesh.vertex_count() == old && offsets(&mesh, a).len() == old && offsets(&mesh, b).len() == old);
    assert_eq!(state(&mesh).2, before.2);
    history.redo(&mut mesh).unwrap();
    assert!(apart(&mesh.positions, &showing) < 4e-7 && state(&mesh).2 == after.2);
}

/// A layer's offsets out of a kept state, for the vertices that were there.
fn offsets_of(kept: &(Vec<[u32; 3]>, Vec<LayerRow>, Vec<Vec<[u32; 3]>>), id: u32) -> Vec<Vec3> {
    let at = kept.1.iter().position(|row| row.id == id).unwrap();
    kept.2[at].iter().map(|bits| Vec3::from_array(bits.map(f32::from_bits))).collect()
}

#[test]
fn a_stroke_with_dynamic_topology_on_is_recorded_new_vertices_and_all() {
    let mut mesh = primitives::build(Primitive::Sphere, 1_500, 0.5);
    let underneath = mesh.positions.clone();
    let old = mesh.vertex_count();
    let (layer, _) = mesh.add_layer(None);
    let mut history = History::default();
    let mut brush = sculptor(Brush::Clay, 60.0, false);
    brush.dyntopo.enabled = true;
    brush.dyntopo.detail = crate::sculpt::dyntopo::MIN_DETAIL;
    let before = state(&mesh);
    history.push_stroke(brushed(&mut mesh, &mut brush, 120.0).expect("the stroke changed the mesh"));
    assert!(mesh.vertex_count() > old + 100, "the stroke added detail");
    let sculpted = mesh.positions.clone();
    assert!(apart(&sculpted[..old], &underneath) > 0.005);

    // The layer off: every vertex that was there is back where it was, and
    // the new ones lie on the ball between them, with no spike or dent.
    mesh.set_layer(layer, false, 100).unwrap();
    assert!(apart(&mesh.positions[..old], &underneath) < 2e-7);
    let off = mesh.positions[old..].iter().map(|p| (p.length() - 0.5).abs()).fold(0.0, f32::max);
    println!("layer off after a stroke with dynamic topology on: the new vertices are within {off:e} of the ball");
    assert!(off < 0.004, "{off}");
    mesh.set_layer(layer, true, 100).unwrap();
    assert!(apart(&mesh.positions, &sculpted) < 2e-7);

    // Undo of the stroke: the mesh and the layer exactly as before it.
    let after = state(&mesh);
    history.undo(&mut mesh).unwrap();
    assert!(state(&mesh).1 == before.1 && state(&mesh).2 == before.2 && bits(&mesh.positions) == before.0);
    history.redo(&mut mesh).unwrap();
    assert!(state(&mesh).2 == after.2 && apart(&mesh.positions, &sculpted) < 4e-7);
}

#[test]
fn a_copy_of_the_mesh_takes_the_layers_under_its_own_numbering() {
    let mut mesh = ball();
    let (a, _) = mesh.add_layer(Some("Wrinkles".into()));
    stroke(&mut mesh, bump(0.05, 0.3));
    let (b, _) = mesh.add_layer(None);
    stroke(&mut mesh, |p| (p.x > 0.1).then(|| p * 1.1));
    mesh.set_layer(a, true, 40).unwrap();
    // Built again from its arrays in another order, as a copy or an opened file is.
    let shuffled: Vec<u32> = mesh.indices.chunks_exact(3).rev().flatten().copied().collect();
    let (mut copy, numbering) = Mesh::new_numbered(mesh.positions.clone(), shuffled, None, &[]);
    copy.layers = mesh.layers_renumbered(&numbering.vertex);
    assert_eq!(copy.layer_rows(), mesh.layer_rows());
    for layer in [a, b] {
        let (theirs, mine) = (offsets(&copy, layer), offsets(&mesh, layer));
        assert!((0..mine.len()).all(|v| theirs[numbering.vertex[v] as usize] == mine[v]));
    }
    // The copy's layers are its own.
    copy.set_layer(b, false, 100).unwrap();
    assert!(mesh.layer_rows()[1].on);
    // Lists that are not this mesh's are refused.
    let saved = || mesh.layers.as_ref().unwrap().saved(None);
    assert!(Layers::restored(saved(), mesh.vertex_count() + 1, None).is_none());
    assert!(Layers::restored(saved(), mesh.vertex_count(), Some(&numbering.vertex[1..])).is_none());
    let mut bad = saved();
    bad.layers[0].offsets[3].y = f32::NAN;
    assert!(Layers::restored(bad, mesh.vertex_count(), None).is_none());
    assert!(Layers::restored(saved(), mesh.vertex_count(), None).is_some());
}

/// A hard pull with the Move brush on the model underneath a layer of deep
/// creases. The layer is left standing as it stood, which is how the app
/// carries layers under every brush but Pose; had it been turned with the
/// surface it would have left several times as much of itself behind.
#[test]
fn a_push_underneath_leaves_the_layers_standing_as_they_stood() {
    // The same pull on the bare ball: what the model underneath should come to.
    let mut bare = ball();
    brushed(&mut bare, &mut sculptor(Brush::Move, 120.0, false), 90.0);
    let left_behind = |ride: Ride| {
        let mut mesh = ball();
        let (layer, _) = mesh.add_layer(None);
        brushed(&mut mesh, &mut sculptor(Brush::Crease, 12.0, false), 150.0);
        let held = offsets(&mesh, layer);
        let deepest = held.iter().map(|o| o.length()).fold(0.0, f32::max);
        mesh.record_on_layer(None);
        mesh.layer_ride = ride;
        brushed(&mut mesh, &mut sculptor(Brush::Move, 120.0, false), 90.0);
        let untouched = bits(&offsets(&mesh, layer)) == bits(&held);
        mesh.set_layer(layer, false, 100);
        // How far the model is off the bare stroke's surface, with the layer hidden.
        let off = |v: usize| (mesh.positions[v] - bare.positions[v]).dot(bare.normals[v]).abs();
        (deepest, (0..bare.positions.len()).map(off).fold(0.0, f32::max), untouched)
    };
    let (deepest, still, untouched) = left_behind(Ride::Still);
    let (_, turned, _) = left_behind(Ride::Turned);
    println!(
        "a hard pull under a layer {deepest:.4} deep, layer hidden: {still:.4} off the bare pull's surface with the \
         layer left standing, {turned:.4} with it turned"
    );
    assert!(untouched, "a push leaves every offset exactly as it was");
    assert!(still < deepest * 0.35 && still < turned * 0.4, "{still} {turned} {deepest}");
}

/// The model underneath posed, pushed and moved by the gizmo, a layer
/// sculpted on in between, and then Reset Shape: the model underneath is
/// where it was built, and the layers are carried back by exactly what
/// carried them, however rough the turns that were read off the surface.
#[test]
fn reset_carries_the_layers_back_by_exactly_what_carried_them() {
    let mut mesh = ball();
    let rest = mesh.positions.clone();
    let (first, _) = mesh.add_layer(None);
    stroke(&mut mesh, |p| Some(p + p.normalize() * 0.02 * (p.x * 40.0).sin() * (p.y * 31.0).cos()));
    mesh.record_on_layer(None);
    let first_held = offsets(&mesh, first);

    // A pose that swings every part a different way and swells one side.
    let pose = |p: Vec3| {
        let up = ((p.y + 0.1) / 0.6).clamp(0.0, 1.0);
        let swell = 0.2 * (-(p - Vec3::new(-0.45, -0.1, 0.1)).length_squared() / 0.08).exp();
        Quat::from_rotation_z(-0.9 * up * up) * (p + p.normalize() * swell) + Vec3::X * 0.25 * up
    };
    mesh.layer_ride = Ride::Turned;
    stroke(&mut mesh, |p| Some(pose(p)));
    assert!(mesh.layers.as_ref().unwrap().frame.is_some(), "what the pose did to the layers is kept");
    // A second layer, sculpted on the posed model.
    let (second, _) = mesh.add_layer(None);
    stroke(&mut mesh, bump(0.04, 0.3));
    mesh.record_on_layer(None);
    // A push, which leaves the layers as they stand.
    mesh.layer_ride = Ride::Still;
    let held = (offsets(&mesh, first), offsets(&mesh, second));
    stroke(&mut mesh, |p| Some(p + Vec3::new(0.2 * p.y * p.y, 0.0, 0.1 * p.x)));
    assert!(bits(&offsets(&mesh, first)) == bits(&held.0) && bits(&offsets(&mesh, second)) == bits(&held.1));
    // The gizmo turning and stretching the whole of it (nothing is masked).
    let matrix = Mat3::from_quat(Quat::from_rotation_x(0.7)) * Mat3::from_diagonal(Vec3::new(1.3, 0.8, 1.0));
    let leaves: Vec<u32> = (0..mesh.bvh.leaves.len() as u32).collect();
    mesh.begin_stroke();
    mesh.move_vertices(&leaves, |_, vertex| Some(matrix * vertex.pos));
    mesh.positions_changed(&leaves);
    mesh.end_stroke_moved(matrix).expect("the gizmo moved the surface");

    let before = state(&mesh);
    let reset = mesh.rest_underneath(&rest).expect("the model underneath was moved");
    let under = apart(&mesh.model_underneath(), &rest);
    let home = (0..rest.len()).map(|v| offsets(&mesh, first)[v].distance(first_held[v])).fold(0.0, f32::max);
    println!("Reset Shape after a pose, a push and a gizmo move: the model underneath {under:e} from rest, the first layer {home:e} from how it stood");
    assert!(under < 1e-6 && home < 2e-6, "{under} {home}");
    assert!(mesh.rest_underneath(&rest).is_none(), "at rest, there is nothing more to reset");
    // The second layer was sculpted on the posed model: it comes back by the pose as well,
    // so it stands on the resting ball as it stood on the posed one. And Undo of the reset
    // is every bit, the frame included: resetting again does exactly the same.
    let after = state(&mesh);
    mesh.apply_delta(&reset, true);
    assert!(state(&mesh) == before, "Undo of Reset Shape puts back every position and offset");
    let again = mesh.rest_underneath(&rest).expect("it is moved again");
    assert!(state(&mesh) == after, "and the reset can be made again, the same");
    mesh.apply_delta(&again, true);
    mesh.apply_delta(&again, false);
    assert!(state(&mesh) == after, "as can its redo");
}

/// The frame is a list for every vertex like the layers' own, and has to
/// grow and shrink with them when dynamic topology adds vertices.
#[test]
fn what_carried_the_layers_keeps_step_with_dynamic_topology() {
    let mut mesh = primitives::build(Primitive::Sphere, 1_500, 0.5);
    let rest = mesh.positions.clone();
    let mut history = History::default();
    let (layer, step) = mesh.add_layer(None);
    history.push_layers(step);
    history.push_layers(mesh.record_on_layer(None).unwrap());
    let turn = Quat::from_axis_angle(Vec3::new(0.3, 1.0, 0.2).normalize(), 0.6);
    mesh.layer_ride = Ride::Turned;
    history.push_stroke(stroke(&mut mesh, |p| Some(turn * p)).unwrap());
    let frame_len = |mesh: &Mesh| mesh.layers.as_ref().unwrap().frame.as_ref().map(|frame| [frame[0].len(), frame[2].len()]);
    assert_eq!(frame_len(&mesh), Some([rest.len(); 2]));
    history.push_layers(mesh.record_on_layer(Some(layer)).unwrap());
    let before = state(&mesh);

    let mut brush = sculptor(Brush::Clay, 60.0, false);
    brush.dyntopo.enabled = true;
    brush.dyntopo.detail = crate::sculpt::dyntopo::MIN_DETAIL;
    history.push_stroke(brushed(&mut mesh, &mut brush, 120.0).expect("the stroke changed the mesh"));
    let grown = mesh.vertex_count();
    assert!(grown > rest.len() + 100 && frame_len(&mesh) == Some([grown; 2]), "a split vertex has its share of the frame");
    history.undo(&mut mesh).unwrap();
    assert!(frame_len(&mesh) == Some([rest.len(); 2]) && state(&mesh) == before);
    history.redo(&mut mesh).unwrap();
    assert_eq!(frame_len(&mesh), Some([grown; 2]));

    // Reset Shape on the grown mesh: the turn is undone for every vertex, new ones included.
    let grown_rest = mesh.rest_shape(&rest).expect("the baseline is this mesh's").into_owned();
    mesh.rest_underneath(&grown_rest).expect("the model underneath was turned");
    assert!(apart(&mesh.model_underneath(), &grown_rest) < 1e-6);
    let frame = mesh.layers.as_ref().unwrap();
    assert!((0..grown).all(|v| frame.frame_at(v) == Mat3::IDENTITY), "nothing is left to carry back");
}

/// The gizmo under mirror symmetry moves each half by its own matrix, the
/// mirror image of the other's: each half's offsets go by its own.
#[test]
fn a_move_by_a_matrix_each_side_its_own_carries_each_sides_offsets_by_its_own() {
    let mut mesh = ball();
    let (layer, _) = mesh.add_layer(None);
    stroke(&mut mesh, |p| Some(p + p.normalize() * 0.02 * (p.y * 40.0).sin()));
    mesh.record_on_layer(None);
    let held = offsets(&mesh, layer);
    let plus = Mat3::from_quat(Quat::from_rotation_z(0.7)) * Mat3::from_diagonal(Vec3::new(2.0, 1.5, 1.0));
    let flip = Mat3::from_diagonal(Vec3::new(-1.0, 1.0, 1.0));
    let minus = flip * plus * flip;
    let by = |start: Vec3| if start.x < 0.0 { minus } else { plus };
    let all: Vec<u32> = (0..mesh.vertex_count() as u32).collect();
    let leaves = mesh.leaves_of(&all);
    let start = mesh.positions.clone();
    mesh.begin_stroke();
    mesh.move_listed(&all, |_, vertex| Some(by(vertex.start) * vertex.start));
    mesh.positions_changed(&leaves);
    mesh.end_stroke_carried(&by).unwrap();
    let now = offsets(&mesh, layer);
    for v in 0..held.len() {
        assert!(now[v].distance(by(start[v]) * held[v]) < 1e-6, "vertex {v}");
    }
    assert!(start.iter().any(|p| p.x < 0.0) && start.iter().any(|p| p.x > 0.0));
}
