//! The deformers on real meshes: what moves and what stays, undo and cancel
//! to the bit (with sculpt layers too), symmetry, dynamic topology,
//! subdivision levels, steps of several meshes taken as one, and the
//! bridge's arguments.

use glam::{DQuat, DVec3, Mat4, Quat, Vec3};
use serde_json::json;

use super::bridge::params_from;
use super::keeps_mirror;
use crate::deform::{Anchor, Deformer, Frame, Kind, Params, Target};
use crate::history::{History, Next};
use crate::mesh::layers::Ride;
use crate::mesh::primitives::{self, Primitive};
use crate::mesh::{Mesh, StrokeDelta};

const AT_REST: Frame = Frame { origin: DVec3::ZERO, rotation: DQuat::IDENTITY };

fn bits(points: &[Vec3]) -> Vec<[u32; 3]> {
    points.iter().map(|p| p.to_array().map(f32::to_bits)).collect()
}

/// An upright cylinder from y = -1 to 1, radius 0.5.
fn cylinder(triangles: usize) -> Mesh {
    primitives::build(Primitive::Cylinder, triangles, 0.5)
}

fn bend(angle: f32) -> Params {
    Params { kind: Kind::Bend, angle, softness: 0.0, ..Params::default() }
}

/// Opens a deformation of `mesh`, shows each of `settings` in turn on it, and
/// gives back what the deformation keeps of it.
fn shown(mesh: &mut Mesh, model: Mat4, frame: &Frame, settings: &[Params]) -> Target {
    let mut target = Target::begin(mesh, model, frame);
    for p in settings {
        let deformer = Deformer::new(p, &target.bounds);
        target.apply(mesh, deformer.as_ref(), p.axis);
    }
    target
}

/// `shown`, then applied: the undo record.
fn applied(mesh: &mut Mesh, settings: &[Params]) -> Option<StrokeDelta> {
    let target = shown(mesh, Mat4::IDENTITY, &AT_REST, settings);
    target.end(mesh, Ride::Turned)
}

#[test]
fn a_ninety_degree_bend_of_a_turned_and_stretched_cylinder_follows_the_arc_in_its_own_axes() {
    let mut mesh = cylinder(20_000);
    let rest = mesh.positions.clone();
    // As the gizmo sets it up for one object: on its origin, along its own axes.
    let (position, rotation, scale) = (Vec3::new(0.3, -0.2, 0.5), Quat::from_rotation_z(0.4), Vec3::new(1.0, 1.5, 0.8));
    let model = Mat4::from_scale_rotation_translation(scale, rotation, position);
    let frame = Frame { origin: position.as_dvec3(), rotation: rotation.as_dquat() };
    let target = shown(&mut mesh, model, &frame, &[bend(90.0)]);
    assert!(target.end(&mut mesh, Ride::Turned).is_some());
    // In the object's own axes the cylinder runs from y = -1.5 to 1.5 (scaled), and the
    // bend lays it on a quarter circle of radius 3 / (π/2) about (y, z) = (-1.5, R).
    let (length, big) = (3.0f64, 3.0 / std::f64::consts::FRAC_PI_2);
    for (was, now) in rest.iter().zip(&mesh.positions) {
        let q = (scale * *was).as_dvec3();
        let theta = std::f64::consts::FRAC_PI_2 * (q.y + 1.5) / length;
        let want = DVec3::new(q.x, -1.5 + big * theta.sin() - q.z * theta.sin(), big * (1.0 - theta.cos()) + q.z * theta.cos());
        let got = frame.from_world(model.transform_point3(*now).as_dvec3());
        assert!(got.distance(want) < 2e-5, "{was} went to {got}, not {want}");
    }
}

#[test]
fn a_twist_of_nothing_and_a_taper_of_one_to_one_leave_every_vertex_to_the_bit() {
    let mut mesh = cylinder(8_000);
    let rest = bits(&mesh.positions);
    let twist = Params { kind: Kind::Twist, angle: 0.0, ..Params::default() };
    let taper = Params { kind: Kind::Taper, scales: [1.0, 1.0], ..Params::default() };
    assert!(applied(&mut mesh, &[twist, taper]).is_none(), "nothing to undo");
    assert_eq!(bits(&mesh.positions), rest);
    // And a twist that was shown and then taken back to nothing.
    let mut target = shown(&mut mesh, Mat4::IDENTITY, &AT_REST, &[Params { kind: Kind::Twist, angle: 50.0, ..Params::default() }]);
    assert_ne!(bits(&mesh.positions), rest);
    target.apply(&mut mesh, None, 1);
    assert!(target.end(&mut mesh, Ride::Turned).is_none());
    assert_eq!(bits(&mesh.positions), rest);
}

#[test]
fn masked_vertices_stay_put_half_masked_go_half_way_and_hidden_ones_stay_too() {
    let mut mesh = cylinder(20_000);
    for v in 0..mesh.vertex_count() {
        let y = mesh.positions[v].y;
        mesh.mask[v] = if y < -0.5 { 1.0 } else if y < 0.0 { 0.5 } else { 0.0 };
    }
    // Hide the faces on the far side, x > 0.25.
    let far: Vec<u32> = (0..mesh.triangle_count() as u32)
        .filter(|&t| (0..3).all(|c| mesh.positions[mesh.indices[t as usize * 3 + c] as usize].x > 0.25))
        .collect();
    mesh.hide(&far);
    let rest = mesh.positions.clone();
    let stretch = Params { kind: Kind::Stretch, stretch: 1.5, keep_volume: false, softness: 0.0, ..Params::default() };
    let delta = applied(&mut mesh, &[stretch.clone()]).expect("it moved");
    // Where a free vertex goes, for comparing with the half-masked ones.
    let free = |p: Vec3| p + Vec3::Y * (p.y + 1.0) * 0.5;
    let (mut pinned, mut halves, mut wholes, mut hidden) = (0, 0, 0, 0);
    for v in 0..rest.len() {
        let (was, now) = (rest[v], mesh.positions[v]);
        if mesh.is_vertex_hidden(v as u32) {
            assert_eq!(now, was, "a hidden vertex moved");
            hidden += 1;
        } else if mesh.mask[v] >= 1.0 {
            assert_eq!(now, was, "a fully masked vertex moved");
            pinned += 1;
        } else if mesh.mask[v] > 0.0 {
            assert!(now.distance(was.lerp(free(was), 0.5)) < 1e-5, "{was} {now}");
            halves += 1;
        } else {
            assert!(now.distance(free(was)) < 1e-5);
            wholes += 1;
        }
    }
    assert!(pinned > 0 && halves > 0 && wholes > 0 && hidden > 0, "{pinned} {halves} {wholes} {hidden}");
    let StrokeDelta::Positions { verts, .. } = &delta else { panic!("a plain stroke") };
    assert_eq!(verts.len(), halves + wholes, "only what moved is in the undo step");
}

#[test]
fn undo_puts_every_vertex_back_to_the_bit_with_sculpt_layers_and_without() {
    for layered in [false, true] {
        for recording in [false, true] {
            if recording && !layered {
                continue;
            }
            let mut mesh = cylinder(12_000);
            let mut history = History::default();
            if layered {
                // A layer with a bump in it, then (unless recording) back to the model underneath.
                let (id, _) = mesh.add_layer(None);
                let leaves: Vec<u32> = (0..mesh.bvh.leaves.len() as u32).collect();
                mesh.begin_stroke();
                mesh.move_vertices(&leaves, |_, v| (v.pos.z > 0.4).then(|| v.pos + Vec3::Z * 0.05));
                mesh.positions_changed(&leaves);
                history.push_stroke(mesh.end_stroke(false).unwrap());
                if !recording {
                    mesh.record_on_layer(None);
                }
                assert!(mesh.layers.as_ref().unwrap().offsets(id).unwrap().iter().any(|o| *o != Vec3::ZERO));
            }
            let offsets = |mesh: &Mesh| mesh.layer_rows().iter().map(|r| bits(mesh.layers.as_ref().unwrap().offsets(r.id).unwrap())).collect::<Vec<_>>();
            let (rest, rest_offsets) = (bits(&mesh.positions), offsets(&mesh));
            let twist = Params { kind: Kind::Twist, angle: 120.0, anchor: Anchor::Middle, ..Params::default() };
            let delta = applied(&mut mesh, &[bend(40.0), twist]).expect("it moved");
            assert_eq!(matches!(delta, StrokeDelta::Layered { .. }), layered, "layers told of it: {layered} {recording}");
            history.push_stroke(delta);
            let (after, after_offsets) = (bits(&mesh.positions), offsets(&mesh));
            assert_eq!(history.undo(&mut mesh), Some(false));
            assert_eq!(bits(&mesh.positions), rest, "{layered} {recording}");
            assert_eq!(offsets(&mesh), rest_offsets);
            assert_eq!(history.redo(&mut mesh), Some(false));
            assert_eq!((bits(&mesh.positions), offsets(&mesh)), (after, after_offsets));
        }
    }
}

#[test]
fn cancel_puts_every_vertex_back_to_the_bit_and_leaves_nothing_to_undo() {
    let mut mesh = cylinder(12_000);
    let (id, _) = mesh.add_layer(None);
    mesh.record_on_layer(None);
    mesh.flush_normals();
    let (rest, normals, offsets) = (bits(&mesh.positions), bits(&mesh.normals), bits(mesh.layers.as_ref().unwrap().offsets(id).unwrap()));
    let inflate = Params { kind: Kind::Inflate, inflate: 0.05, ..Params::default() };
    let target = shown(&mut mesh, Mat4::IDENTITY, &AT_REST, &[bend(30.0), bend(80.0), inflate.clone()]);
    mesh.flush_normals();
    assert_ne!(bits(&mesh.positions), rest);
    target.cancel(&mut mesh);
    mesh.flush_normals();
    assert_eq!(bits(&mesh.positions), rest);
    assert_eq!(bits(&mesh.normals), normals);
    assert_eq!(bits(mesh.layers.as_ref().unwrap().offsets(id).unwrap()), offsets);
    // The mesh is out of its stroke: another can begin.
    mesh.begin_stroke();
    assert!(mesh.end_stroke(false).is_none());
}

#[test]
fn a_preview_is_always_worked_out_from_the_start_so_nothing_drifts() {
    let mut mesh = cylinder(12_000);
    for kind in [Kind::Bend, Kind::Inflate, Kind::Lattice, Kind::Stretch] {
        let mut a = crate::deform::bench::dragged(kind, 0);
        a.range = [0.1, 0.9];
        let b = crate::deform::bench::dragged(kind, 7);
        let mut target = shown(&mut mesh, Mat4::IDENTITY, &AT_REST, &[a.clone()]);
        let first = bits(&mesh.positions);
        for p in [&b, &a, &b, &b, &a] {
            let deformer = Deformer::new(p, &target.bounds);
            target.apply(&mut mesh, deformer.as_ref(), p.axis);
            // Inflate reads the normals, which the renderer brings up to date between frames.
            mesh.flush_normals();
        }
        assert_eq!(bits(&mesh.positions), first, "{kind:?}");
        target.cancel(&mut mesh);
    }
}

#[test]
fn a_bend_in_the_mirror_plane_of_a_symmetric_model_passes_check_symmetry() {
    use sculpt_symmetry::Axis;
    let mut mesh = primitives::build(Primitive::Capsule, 30_000, 0.5);
    let check = |mesh: &Mesh| crate::app::symmetry_repair::symmetry_of(mesh, "Capsule", Axis::X, 1.0);
    assert!(check(&mesh).1, "the capsule starts symmetric: {}", check(&mesh).0);
    // Along Y toward Z: the bend's plane is the X mirror plane.
    let target = shown(&mut mesh, Mat4::IDENTITY, &AT_REST, &[Params { angle: 70.0, softness: 0.5, ..bend(0.0) }]);
    let deformer = Deformer::new(&Params { angle: 70.0, softness: 0.5, ..bend(0.0) }, &target.bounds).unwrap();
    assert!(keeps_mirror(&mesh, &target, &deformer, Mat4::IDENTITY, 0, false));
    // Toward X it is not, and nor is a twist about Y.
    let sideways = Deformer::new(&Params { angle: 70.0, direction: 90.0, ..bend(0.0) }, &target.bounds).unwrap();
    assert!(!keeps_mirror(&mesh, &target, &sideways, Mat4::IDENTITY, 0, false));
    let twist = Deformer::new(&Params { kind: Kind::Twist, angle: 70.0, ..Params::default() }, &target.bounds).unwrap();
    assert!(!keeps_mirror(&mesh, &target, &twist, Mat4::IDENTITY, 0, false));
    assert!(target.end(&mut mesh, Ride::Turned).is_some());
    let (text, symmetric) = check(&mesh);
    assert!(symmetric, "{text}");
}

#[test]
fn a_mesh_dynamic_topology_refined_is_deformed_whole_and_undone_whole() {
    let mut mesh = primitives::build(Primitive::Sphere, 4_000, 0.5);
    let before = mesh.vertex_count();
    mesh.refine(&[Vec3::new(0.0, 0.0, 0.5)], 0.2, 0.01, 1_000_000, 10_000_000);
    assert!(mesh.vertex_count() > before, "dynamic topology added vertices");
    let rest = bits(&mesh.positions);
    let stretch = Params { kind: Kind::Stretch, stretch: 2.0, softness: 0.0, ..Params::default() };
    let delta = applied(&mut mesh, &[stretch]).unwrap();
    // Every vertex but the ones on the anchor's own plane moved, the new ones too.
    let moved = mesh.positions.iter().zip(&rest).filter(|(p, r)| p.to_array().map(f32::to_bits) != **r).count();
    assert!(moved as f64 > mesh.vertex_count() as f64 * 0.99, "{moved} of {}", mesh.vertex_count());
    assert!(mesh.positions[before..].iter().all(|p| p.z > 0.0 && p.y.abs() < 1.1));
    let mut history = History::default();
    history.push_stroke(delta);
    history.undo(&mut mesh);
    assert_eq!(bits(&mesh.positions), rest);
}

#[test]
fn a_deformation_on_a_coarser_level_is_carried_up_to_the_finer_one() {
    let base = cylinder(2_000);
    let (fine, _) = crate::mesh::levels::subdivided(&base, sculpt_multires::Split::Flat).unwrap();
    let (mut coarse, _) = crate::mesh::levels::stepped(&fine, 0).unwrap();
    let height = |mesh: &Mesh| mesh.bounds().size().y;
    let stretch = Params { kind: Kind::Stretch, stretch: 1.5, keep_volume: false, softness: 0.0, ..Params::default() };
    applied(&mut coarse, &[stretch]).unwrap();
    assert!((height(&coarse) - 3.0).abs() < 1e-4);
    let (up, _) = crate::mesh::levels::stepped(&coarse, 1).unwrap();
    assert_eq!(up.triangle_count(), fine.triangle_count());
    assert!((height(&up) - 3.0).abs() < 1e-3, "the finer level was stretched too: {}", height(&up));
}

#[test]
fn steps_of_several_meshes_made_together_are_undone_and_redone_together() {
    let (mut a, mut b) = (cylinder(2_000), cylinder(2_000));
    let mut history = History::default();
    let (rest_a, rest_b) = (bits(&a.positions), bits(&b.positions));
    // An unrelated step before, which must stay.
    history.target = 1;
    history.push_stroke(applied(&mut a, &[bend(10.0)]).unwrap());
    let earlier = bits(&a.positions);
    history.push_stroke(applied(&mut a, &[bend(30.0)]).unwrap());
    history.target = 2;
    history.push_stroke(applied(&mut b, &[bend(30.0)]).unwrap());
    history.join_last();
    history.target = 1;
    assert_eq!(history.steps(), (3, 0));
    // One press of Undo, as the app runs it: step, and step on while joined.
    assert_eq!(history.next_undo(), Some(Next::Mesh(2)));
    history.undo(&mut b);
    assert!(history.joined(true));
    assert_eq!(history.next_undo(), Some(Next::Mesh(1)));
    history.undo(&mut a);
    assert!(!history.joined(true), "the earlier step is not part of it");
    assert_eq!((bits(&a.positions), bits(&b.positions)), (earlier, rest_b.clone()));
    // Redo takes both again.
    history.redo(&mut a);
    assert!(history.joined(false));
    history.redo(&mut b);
    assert!(!history.joined(false));
    assert_ne!(bits(&b.positions), rest_b);
    assert_ne!(bits(&a.positions), rest_a);
}

#[test]
fn the_bridge_reads_its_numbers_as_the_card_shows_them() {
    let p = params_from(&json!({
        "kind": "bend_arc", "axis": "z", "tip": [10, -20, 30], "range": [25, 75], "softness": 0, "anchor": "end"
    }))
    .unwrap();
    assert_eq!((p.kind, p.axis, p.tip, p.range, p.softness, p.anchor), (Kind::BendArc, 2, [0.1, -0.2, 0.3], [0.25, 0.75], 0.0, Anchor::End));
    let p = params_from(&json!({ "kind": "Inflate", "amount": -5 })).unwrap();
    assert_eq!(p.inflate, -0.05);
    let p = params_from(&json!({ "kind": "lattice", "lattice_points": [2, 4, 2], "lattice_moves": [{ "point": [1, 4, 2], "by": [0, 50, 0] }] })).unwrap();
    assert_eq!(p.lattice.count, [2, 4, 2]);
    assert_eq!(p.lattice.moved[p.lattice.index([0, 3, 1])], [0.0, 0.5, 0.0]);
    assert!(params_from(&json!({ "kind": "melt" })).unwrap_err().contains("bend, bend_arc"));
    assert!(params_from(&json!({ "kind": "bend", "axis": "w" })).is_err());
    assert!(params_from(&json!({ "kind": "twist", "amount": 3 })).is_err());
    assert!(params_from(&json!({ "kind": "lattice", "lattice_moves": [{ "point": [9, 1, 1], "by": [0, 0, 0] }] })).is_err());
}
