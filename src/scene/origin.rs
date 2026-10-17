//! Set Origin to Mesh: moves an object's centre onto its mesh without moving
//! the object.
//!
//! An object's centre is the origin of its mesh's own space: the point its
//! position names, the pivot it turns and resizes about, where the gizmo
//! sits, and where its Local mirror planes and radial axis pass. A piece cut
//! off a figure keeps the figure's centre, which can be a long way from the
//! piece (see "Far from its own mirror plane" in the README). This puts the
//! centre in the middle of the mesh's bounding box.
//!
//! Nothing moves in the world. The mesh's vertices are all shifted by the
//! same amount in the mesh's own space, and each object that shows the mesh
//! has its position moved the other way through its own turn and size, so
//! the two cancel: for a vertex `p`, a shift `c` and an object placed by
//! `position + rotation * (scale * p)`,
//!
//! ```text
//! (position + rotation * (scale * c)) + rotation * (scale * (p - c))
//! ```
//!
//! is where it was, to the rounding of the sums.
//!
//! What follows from how the rest of the scene is kept:
//!
//! - **Instances** share the mesh, so the shift is theirs too, and each gets
//!   its own correction: they have their own turns and sizes.
//! - **Live mirror copies** are the object's matrix reflected across the
//!   world's planes (`scene/mirror.rs`). The matrix places every vertex where
//!   it did, so the copies do not move, and the mirror's own centre is a
//!   point of the world's that is left alone.
//! - **A World symmetry** is carried into the mesh's space through where the
//!   object stands (`Symmetry::world`), so its planes stay where they are in
//!   the world. **A Local symmetry** goes through the object's centre by
//!   definition, so its planes and its radial axis move with the centre: that
//!   is the point of the command for a piece far from its mirror plane, and a
//!   thing to know before pressing it on a model that is symmetric about the
//!   centre it has.
//! - **Arrays** are the Array tool's, laid out from the object's position;
//!   the app tells the tool, which carries the path the other way
//!   (`Tool::origin_moved`).
//!
//! The shifted mesh is a new mesh under a new number, as after a join: the
//! undo step is the object list, which names the old mesh, and the old mesh
//! stays parked, to the bit, until no step shows it. So one Undo puts back
//! every vertex and every position exactly, and nothing is subtracted twice.
//! The mask, the hidden faces, the quads of a quad remesh and the shape Reset
//! Shape goes back to all come across.

use glam::{DVec3, Vec3};
use sculpt_symmetry::Placement;

use super::{Baseline, Scene, duplicate_placed};
use crate::mesh::Mesh;

/// A centre this close to the middle of its mesh, as a fraction of the
/// mesh's longest side, is on it: moving it would change nothing that can be
/// seen and cost an undo step.
const ALREADY_THERE: f64 = 1e-6;

/// What [`Scene::move_origin_to_mesh`] did.
#[derive(Clone, Debug, PartialEq)]
pub struct OriginMoved {
    /// How far the centre went, in the mesh's own space: every vertex was
    /// moved by the opposite of this.
    pub shift: Vec3,
    /// Every object that shows the mesh, with where it stood and where it
    /// stands now. The active object is first.
    pub objects: Vec<(u64, Placement, Placement)>,
}

/// The middle of the box round `positions`, worked out in double so that a
/// mesh far from its centre is not measured with its own rounding. `None`
/// for no points, or points that are not places.
fn middle_of(positions: &[Vec3]) -> Option<(DVec3, f64)> {
    let (mut low, mut high) = (DVec3::INFINITY, DVec3::NEG_INFINITY);
    for p in positions {
        let p = p.as_dvec3();
        (low, high) = (low.min(p), high.max(p));
    }
    let middle = (low + high) * 0.5;
    (!positions.is_empty() && middle.is_finite()).then(|| (middle, (high - low).max_element()))
}

impl Scene {
    /// How far the active object's centre is from the middle of its mesh, in
    /// the mesh's own space. `None` when it is there already (or there is no
    /// mesh to speak of): there is then nothing for Set Origin to Mesh to do.
    pub fn origin_off_mesh(&self, slot: &Mesh) -> Option<Vec3> {
        let (middle, longest) = middle_of(&slot.positions)?;
        let shift = middle.as_vec3();
        (middle.abs().max_element() > longest * ALREADY_THERE && shift != Vec3::ZERO).then_some(shift)
    }

    /// Moves the active object's centre to the middle of its mesh and leaves
    /// the object, and every instance of it, where it is in the world (see
    /// the top of this file). The caller records the object list from before
    /// as the undo step. `None`, and nothing changes, when the centre is
    /// there already.
    pub fn move_origin_to_mesh(&mut self, slot: &mut Mesh) -> Option<OriginMoved> {
        let shift = self.origin_off_mesh(slot)?;
        Some(self.move_origin_by(shift, slot))
    }

    /// Moves the active object's centre by `shift`, measured in its mesh's
    /// own space, and leaves the object and its instances where they are in
    /// the world: the mesh goes by `-shift` inside each. What
    /// [`Self::move_origin_to_mesh`] does once it has measured how far; the
    /// scripted check also stands a centre far from its mesh with it.
    pub(crate) fn move_origin_by(&mut self, shift: Vec3, slot: &mut Mesh) -> OriginMoved {
        let positions: Vec<Vec3> = slot.positions.iter().map(|&p| p - shift).collect();
        let (moved, places) = duplicate_placed(slot, positions);

        let (old, new) = (self.active_mesh(), self.fresh_id());
        // The shape Reset Shape goes back to, for the new mesh's vertices.
        let rest = self.baseline(slot).and_then(|rest| {
            let rest = slot.rest_shape(&rest.positions)?;
            let mut positions = moved.positions.clone();
            for (from, &to) in places.iter().enumerate().filter(|(_, to)| **to != u32::MAX) {
                positions[to as usize] = rest[from] - shift;
            }
            Some(Baseline { uid: moved.uid, positions })
        });
        if let Some(rest) = rest {
            self.baselines.insert(new, rest);
        }

        let active = self.active;
        let mut objects = Vec::new();
        for object in self.objects.iter_mut().filter(|o| o.mesh == old) {
            let from = object.placement();
            object.mesh = new;
            object.position += object.rotation * (object.scale * shift);
            let entry = (object.id, from, object.placement());
            // The active object first: it is the one the status line names.
            if object.id == active { objects.insert(0, entry) } else { objects.push(entry) }
        }
        self.parked.insert(new, moved);
        self.swap_in(old, new, slot);
        (self.brush, self.stroked) = (None, None);
        OriginMoved { shift, objects }
    }
}

#[cfg(test)]
mod tests {
    use glam::{Mat4, Quat};

    use super::*;
    use crate::history::History;
    use crate::mesh::Polys;
    use crate::mesh::primitives::{self, Primitive};
    use crate::remesh::voxel_remesh;
    use crate::scene::{Space, Symmetry};

    /// A cube whose own centre is `off` from its middle.
    fn cube_off(off: Vec3) -> Mesh {
        let cube = primitives::build(Primitive::Cube, 600, 0.5);
        Mesh::new(cube.positions.iter().map(|&p| p + off).collect(), cube.indices.clone(), None)
    }

    /// Where every vertex of an object's mesh is in the world, drawn through `model`.
    fn world_points(scene: &Scene, slot: &Mesh, id: u64, model: Mat4) -> Vec<Vec3> {
        let mesh = scene.mesh(scene.get(id).unwrap().mesh, slot).unwrap();
        mesh.positions.iter().map(|&p| model.transform_point3(p)).collect()
    }

    /// Whether two sets of points are the same places to within `near`,
    /// however they are numbered: as many of them, and each of one with one
    /// of the other on it.
    fn same_places(a: &[Vec3], b: &[Vec3], near: f32) -> bool {
        let on = |from: &[Vec3], to: &[Vec3]| from.iter().all(|p| to.iter().any(|q| p.distance(*q) <= near));
        a.len() == b.len() && on(a, b) && on(b, a)
    }

    /// The rounding of a sum or two at the sizes these scenes have.
    const NEAR: f32 = 1e-5;

    fn placed(scene: &mut Scene, id: u64, position: Vec3, rotation: Quat, scale: Vec3) {
        let object = scene.get_mut(id).unwrap();
        (object.position, object.rotation, object.scale) = (position, rotation, scale);
    }

    #[test]
    fn the_centre_goes_to_the_middle_of_the_mesh_and_nothing_moves_in_the_world() {
        let off = Vec3::new(3.0, -1.5, 0.25);
        let mut mesh = cube_off(off);
        let mut scene = Scene::new("Piece");
        let id = scene.active_id();
        placed(&mut scene, id, Vec3::new(0.4, 2.0, -1.0), Quat::from_euler(glam::EulerRot::XYZ, 0.3, -0.8, 1.1), Vec3::new(2.0, 0.5, 1.25));
        let before = world_points(&scene, &mesh, id, scene.active().matrix());
        let was = scene.active().clone();

        assert_eq!(scene.origin_off_mesh(&mesh), Some(off));
        let moved = scene.move_origin_to_mesh(&mut mesh).expect("the centre was off the mesh");
        assert_eq!(moved.shift, off);
        assert_eq!(moved.objects, [(id, was.placement(), scene.active().placement())]);

        // The mesh's own box is about its own centre now.
        let bounds = mesh.bounds();
        assert!(bounds.center().abs().max_element() < 1e-6, "{:?}", bounds.center());
        assert!((bounds.size() - Vec3::ONE).abs().max_element() < 1e-5);
        // The object's position is the middle of the piece, in the world.
        let object = scene.active();
        assert!(object.position.distance(was.matrix().transform_point3(off)) < 1e-5);
        assert_eq!((object.rotation, object.scale), (was.rotation, was.scale), "only the position changes");
        // And every vertex is where it was.
        assert!(same_places(&world_points(&scene, &mesh, id, scene.active().matrix()), &before, NEAR));
        // A second press has nothing to do.
        assert_eq!(scene.origin_off_mesh(&mesh), None);
        assert!(scene.move_origin_to_mesh(&mut mesh).is_none());
    }

    #[test]
    fn instances_each_stay_put_through_their_own_turn_and_size() {
        let mut mesh = cube_off(Vec3::new(-2.0, 0.5, 4.0));
        let mut scene = Scene::new("Piece");
        let first = scene.active_id();
        placed(&mut scene, first, Vec3::new(1.0, 0.0, 0.0), Quat::from_rotation_z(0.4), Vec3::splat(1.5));
        let twin = scene.instance_active();
        placed(&mut scene, twin, Vec3::new(-6.0, 2.0, 1.0), Quat::from_rotation_x(-1.2), Vec3::new(0.5, 3.0, 1.0));
        // An object with a mesh of its own is no part of it.
        let other = scene.add("Other", cube_off(Vec3::X * 9.0), &mut mesh);
        let other_was = scene.get(other).unwrap().clone();
        scene.select(twin, &mut mesh);

        let world = |scene: &Scene, mesh: &Mesh, id: u64| world_points(scene, mesh, id, scene.get(id).unwrap().matrix());
        let before = [first, twin, other].map(|id| world(&scene, &mesh, id));
        let moved = scene.move_origin_to_mesh(&mut mesh).unwrap();
        assert_eq!(moved.objects.iter().map(|o| o.0).collect::<Vec<_>>(), [twin, first], "the selected one first");
        for (id, before) in [first, twin, other].into_iter().zip(&before) {
            assert!(same_places(&world(&scene, &mesh, id), before, 4.0 * NEAR), "object {id} moved");
        }
        // Still one mesh between the two, and a new one.
        assert_eq!(scene.get(first).unwrap().mesh, scene.get(twin).unwrap().mesh);
        assert_eq!(scene.users(scene.active_mesh()), 2);
        assert_eq!(*scene.get(other).unwrap(), other_was);
        assert_eq!(scene.active_id(), twin);
    }

    #[test]
    fn live_mirror_copies_stay_where_they_are() {
        let mut mesh = cube_off(Vec3::new(2.0, 1.0, -3.0));
        let mut scene = Scene::new("Piece");
        let id = scene.active_id();
        placed(&mut scene, id, Vec3::new(1.5, 0.2, 0.0), Quat::from_rotation_y(0.7), Vec3::new(1.0, 2.0, 0.5));
        let centre = Vec3::new(0.25, 0.0, -0.5);
        assert!(scene.set_mirror(id, 1 | 4, centre));
        let copies = |scene: &Scene, mesh: &Mesh| -> Vec<(u8, Vec<Vec3>)> {
            scene.active().placements().map(|(copy, model)| (copy, world_points(scene, mesh, id, model))).collect()
        };
        let before = copies(&scene, &mesh);
        assert_eq!(before.len(), 4, "the object and three reflections");
        let frame = scene.bounds(&mesh);

        scene.move_origin_to_mesh(&mut mesh).unwrap();
        for ((copy, now), (was_copy, was)) in copies(&scene, &mesh).iter().zip(&before) {
            assert_eq!(copy, was_copy);
            assert!(same_places(now, was, 2.0 * NEAR), "mirror copy {copy} moved");
        }
        assert_eq!(scene.active().mirror.map(|m| (m.axis_mask, m.center)), Some((5, centre)), "the mirror is as it was");
        let now = scene.bounds(&mesh);
        assert!((now.min - frame.min).abs().max_element() < 1e-4 && (now.max - frame.max).abs().max_element() < 1e-4);
    }

    /// A World symmetry's plane, in the mesh's own space, is where it was on
    /// the model; a Local one has come with the centre.
    #[test]
    fn a_world_symmetry_stays_in_the_world_and_a_local_one_comes_with_the_centre() {
        let off = Vec3::new(5.0, 0.0, 0.0);
        let mut mesh = cube_off(off);
        let mut scene = Scene::new("Piece");
        let id = scene.active_id();
        // Standing so that the world's X plane cuts the cube a quarter of the way in.
        placed(&mut scene, id, Vec3::new(-5.25, 0.0, 0.0), Quat::IDENTITY, Vec3::ONE);
        scene.set_symmetry(id, Symmetry { mirror: [true, false, false], space: Space::World, ..Symmetry::default() });
        // The plane's place on the model: the point of the mesh the world's centre is at.
        let on_model = |scene: &Scene| scene.active().matrix().inverse().transform_point3(Vec3::ZERO);
        let was = on_model(&scene) - off;
        scene.move_origin_to_mesh(&mut mesh).unwrap();
        assert!(scene.symmetry(id).world(scene.active()).is_some(), "still the world's");
        assert!((on_model(&scene) - was).length() < 1e-5, "the plane is where it was on the model: {}", on_model(&scene));
        assert!((on_model(&scene).x - 0.25).abs() < 1e-5, "a quarter of the way in from its middle");
        // A Local plane is x = 0 of the mesh's own space, which is the middle of the cube now.
        assert!(mesh.bounds().min.x < -0.49 && mesh.bounds().max.x > 0.49);
    }

    #[test]
    fn the_mask_the_hidden_faces_the_quads_and_the_resting_shape_come_across() {
        let off = Vec3::new(1.0, 2.0, 3.0);
        let mut mesh = cube_off(off);
        let mut scene = Scene::new("Piece");
        // Masked above its middle, hidden below a quarter of the way down, and its first two
        // triangles kept as a "quad" (any polygon over its vertices will do).
        for v in 0..mesh.vertex_count() {
            mesh.mask[v] = if mesh.positions[v].y > off.y { 1.0 } else { 0.25 };
        }
        let low: Vec<u32> = (0..mesh.triangle_count() as u32)
            .filter(|&t| mesh.indices[t as usize * 3..t as usize * 3 + 3].iter().all(|&v| mesh.positions[v as usize].y < off.y - 0.25))
            .collect();
        assert!(mesh.hide(&low).is_some());
        let corners: Vec<u32> = mesh.indices[..3].to_vec();
        let corner_places: Vec<Vec3> = corners.iter().map(|&v| mesh.positions[v as usize]).collect();
        mesh.polys = Some(Polys { start: vec![0, 3], indices: corners });
        // Sculpted since it was built: one vertex of the top pushed in (the box round the mesh
        // is as it was), the resting shape kept first.
        scene.keep_baseline(&mesh);
        let inner = |p: &Vec3| p.y > off.y + 0.49 && (p.x - off.x).abs() < 0.25 && (p.z - off.z).abs() < 0.25;
        let pushed = mesh.positions.iter().position(inner).expect("a vertex in the middle of the top");
        let rest_of_pushed = mesh.positions[pushed];
        mesh.positions[pushed] -= Vec3::Y * 0.3;
        assert!(scene.can_reset(&mesh));

        scene.move_origin_to_mesh(&mut mesh).unwrap();
        for (p, &m) in mesh.positions.iter().zip(&mesh.mask) {
            assert_eq!(m, if p.y > 0.0 { 1.0 } else { 0.25 }, "mask at {p}");
        }
        assert_eq!(mesh.hidden_triangle_count(), low.len());
        for t in 0..mesh.triangle_count() as u32 {
            let below = mesh.indices[t as usize * 3..t as usize * 3 + 3].iter().all(|&v| mesh.positions[v as usize].y < -0.25);
            assert_eq!(mesh.is_triangle_hidden(t), below, "triangle {t}");
        }
        let polys = mesh.polys.as_ref().expect("the quads came across");
        let now: Vec<Vec3> = polys.indices.iter().map(|&v| mesh.positions[v as usize]).collect();
        for (now, was) in now.iter().zip(&corner_places) {
            assert!(now.distance(*was - off) < 1e-6);
        }
        // Reset Shape still goes back to the shape before the push, where the model is now.
        assert!(scene.can_reset(&mesh));
        let nearest = |mesh: &Mesh, to: Vec3| mesh.positions.iter().map(|p| p.distance(to)).fold(f32::MAX, f32::min);
        let rest = rest_of_pushed - off;
        assert!(nearest(&mesh, rest - Vec3::Y * 0.3) < 1e-6 && nearest(&mesh, rest) > 1e-3, "the dent is still in it");
        assert!(scene.reset_shape(&mut mesh).is_some());
        assert!(nearest(&mesh, rest) < 1e-6 && nearest(&mesh, rest - Vec3::Y * 0.3) > 0.1, "and Reset Shape takes it out");
    }

    /// As the app records it (`Running::edit_scene`): the object list from
    /// before is the step, and the old mesh waits in the scene.
    #[test]
    fn one_undo_step_puts_back_every_vertex_and_every_place_to_the_bit() {
        let mut mesh = cube_off(Vec3::new(0.3, 7.0, -0.1));
        let mut scene = Scene::new("Piece");
        let mut history = History::default();
        let first = scene.active_id();
        placed(&mut scene, first, Vec3::new(0.1, 0.2, 0.3), Quat::from_rotation_y(1.0), Vec3::new(1.0, 2.0, 3.0));
        let twin = scene.instance_active();
        scene.get_mut(twin).unwrap().position = Vec3::new(4.0, 0.0, 0.0);
        scene.select(first, &mut mesh);
        let (uid, positions, objects) = (mesh.uid, mesh.positions.clone(), scene.objects.clone());

        let before = scene.state();
        let moved = scene.move_origin_to_mesh(&mut mesh).unwrap();
        history.push_scene(before);
        history.settle(&mut scene);
        assert_eq!(history.steps(), (1, 0));
        let (new_uid, after) = (mesh.uid, scene.objects.clone());
        assert_ne!(new_uid, uid);

        assert!(history.step_scene(true, &mut scene, &mut mesh));
        history.settle(&mut scene);
        assert_eq!((mesh.uid, &mesh.positions, &scene.objects), (uid, &positions, &objects));
        assert!(!history.can_undo());
        assert!(history.step_scene(false, &mut scene, &mut mesh));
        history.settle(&mut scene);
        assert_eq!((mesh.uid, &scene.objects), (new_uid, &after));
        assert_eq!(scene.get(first).unwrap().placement(), moved.objects[0].2);
    }

    /// The README's piece split off a figure: it keeps the figure's centre
    /// and its mirrors, a long way off, and a mirrored rebuild of it is not
    /// attempted. With its centre moved onto it, the same rebuild runs at the
    /// resolution the app starts with, and what comes out is its own mirror
    /// image where it stands.
    #[test]
    fn a_piece_far_from_its_mirror_plane_is_recentred_and_then_mirrored_at_the_default_resolution() {
        let resolution = crate::doc::Options::default().remesh_resolution as u32;
        let mirror = [true, true, false];
        // A rivet from the figure's hand: a lopsided block, 300 of its own lengths out.
        let far = Vec3::new(300.0, 300.0, 2.0);
        let block = primitives::build(Primitive::Cube, 2_400, 0.5);
        let lopsided = |p: Vec3| p * Vec3::new(1.0 + 0.3 * p.y, 0.6, 0.8) + far;
        let mut mesh = Mesh::new(block.positions.iter().map(|&p| lopsided(p)).collect(), block.indices.clone(), None);
        let mut scene = Scene::new("Rivet");
        let id = scene.active_id();
        placed(&mut scene, id, Vec3::new(-2.0, 1.0, 0.0), Quat::from_rotation_z(0.5), Vec3::splat(0.01));
        scene.set_symmetry(id, Symmetry { mirror, ..Symmetry::default() });

        let refused = voxel_remesh(&mesh, resolution, mirror).err().expect("too far off");
        assert!(refused.contains("times its own size away from its X and Y mirror planes"), "{refused}");
        assert!(refused.contains("Set Origin to Mesh"), "the line says what would let it through: {refused}");

        let world = world_points(&scene, &mesh, id, scene.active().matrix());
        scene.move_origin_to_mesh(&mut mesh).expect("its centre is far from it");
        // The rivet is a hundredth of a unit across: within a thousandth of that.
        let now = world_points(&scene, &mesh, id, scene.active().matrix());
        assert!(same_places(&now, &world, 1e-5), "the rivet has not moved");
        assert_eq!(scene.active_symmetry().mirror, mirror, "its mirrors are still on");

        let rebuilt = voxel_remesh(&mesh, resolution, mirror).expect("it fits now");
        assert!(crate::printable::census(&rebuilt).is_closed());
        let places: std::collections::HashSet<[u32; 3]> =
            rebuilt.positions.iter().map(|&p| (p + Vec3::ZERO).to_array().map(f32::to_bits)).collect();
        for flip in [Vec3::new(-1.0, 1.0, 1.0), Vec3::new(1.0, -1.0, 1.0)] {
            let mirrored = |p: &Vec3| places.contains(&(*p * flip + Vec3::ZERO).to_array().map(f32::to_bits));
            assert!(rebuilt.positions.iter().all(mirrored), "exactly its own mirror image across {flip}");
        }
        // The size it was, about the centre it now has.
        let (was, now) = (mesh.bounds().size(), rebuilt.bounds());
        assert!((now.size() - was).abs().max_element() < 0.05 * was.max_element(), "{:?} against {was}", now.size());
        assert!(now.center().abs().max_element() < 1e-3);
    }

    #[test]
    fn nothing_to_measure_is_nothing_to_do() {
        let mut empty = Mesh::new(Vec::new(), Vec::new(), None);
        let mut scene = Scene::new("Nothing");
        assert_eq!(scene.origin_off_mesh(&empty), None);
        assert!(scene.move_origin_to_mesh(&mut empty).is_none());
        // A centre a hair off the middle is on it: a sphere as the app makes one.
        let mut ball = primitives::build(Primitive::Sphere, 2_000, 0.5);
        let mut scene = Scene::new("Sphere");
        assert!(scene.move_origin_to_mesh(&mut ball).is_none());
    }
}
