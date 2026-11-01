//! Apply Scale: puts an object's uneven Scale into its mesh, so the object
//! looks the same and stands at a Scale of 1.
//!
//! A brush works in its mesh's own space. It is round there, at a size worked
//! out from the object's average scale, so on an object stretched three times
//! as far one way as another a dab lands stretched the same way, and whatever
//! the ring on screen shows. Remesh's voxels are cubes in that same space, so
//! its triangles come out stretched too. Nothing a brush can do to such an
//! object lands as it should; with the stretch in the mesh, all of it does.
//!
//! Nothing moves in the world. For a vertex `p`, an object placed by
//! `position + rotation * (scale * p)` and a stretch `k`,
//!
//! ```text
//! position + rotation * ((scale / k) * (k * p))
//! ```
//!
//! is where it was, to the rounding of the sums. `k` is the active object's
//! own size along each axis, so its Scale comes out 1 (or -1, for an axis it
//! is flipped along: the triangles keep their winding).
//!
//! As for Set Origin to Mesh (`scene/origin.rs`): instances share the mesh, so
//! each gets its own correction; live mirror copies and arrays are drawn
//! through the object's placement, so they stay as they look; and the result
//! is a new mesh under a new number, the old one parked to the bit, so one Undo
//! of the object list puts everything back. The planes of a symmetry on the
//! object's own axes stay where they are, since `k` leaves those planes alone.

use glam::Vec3;

use super::{Baseline, Scene, duplicate_placed};
use crate::mesh::Mesh;

/// A Scale whose longest side is more than this fraction longer than its
/// shortest counts as uneven, as for turning a pivot (`scene/origin.rs`).
const UNEVEN: f32 = 1e-4;

/// A Scale this small on any axis has flattened the object, and there is no
/// shape left in that axis to bring back.
const FLATTENED: f32 = 1e-6;

impl Scene {
    /// How many times longer the active object's longest side is stretched
    /// than its shortest: 1 for an object scaled the same every way.
    pub fn stretch(&self) -> f32 {
        let scale = self.active().scale.abs();
        scale.max_element() / scale.min_element().max(FLATTENED)
    }

    /// Whether Apply Scale has anything to do on the active object.
    pub fn scale_is_uneven(&self) -> bool {
        self.stretch() > 1.0 + UNEVEN && self.active().scale.abs().min_element() > FLATTENED
    }

    /// Puts the active object's Scale into its mesh and leaves every object
    /// that shows the mesh looking as it did. The caller records the object
    /// list from before as the undo step. `None`, and nothing changes, when
    /// the Scale is even. Otherwise how many instances came with it.
    pub fn apply_scale(&mut self, slot: &mut Mesh) -> Option<usize> {
        if !self.scale_is_uneven() {
            return None;
        }
        let by = self.active().scale.abs();
        let (moved, places) = duplicate_placed(slot, slot.positions.iter().map(|&p| p * by).collect());

        let (old, new) = (self.active_mesh(), self.fresh_id());
        // The shape Reset Shape goes back to, for the new mesh's vertices.
        let rest = self.baseline(slot).and_then(|rest| {
            let rest = slot.rest_shape(&rest.positions)?;
            let mut positions = moved.positions.clone();
            for (from, &to) in places.iter().enumerate().filter(|(_, to)| **to != u32::MAX) {
                positions[to as usize] = rest[from] * by;
            }
            Some(Baseline { uid: moved.uid, positions })
        });
        if let Some(rest) = rest {
            self.baselines.insert(new, rest);
        }

        let mut count = 0;
        for object in self.objects.iter_mut().filter(|o| o.mesh == old) {
            object.mesh = new;
            object.scale /= by;
            count += 1;
        }
        // The active object's comes out exact, not to the rounding of a division.
        self.active_mut().scale = Vec3::select(self.active().scale.cmplt(Vec3::ZERO), Vec3::NEG_ONE, Vec3::ONE);
        self.parked.insert(new, moved);
        self.swap_in(old, new, slot);
        (self.brush, self.stroked) = (None, None);
        Some(count - 1)
    }
}

#[cfg(test)]
mod tests {
    use glam::{Mat4, Quat, Vec2};

    use super::*;
    use crate::camera::Camera;
    use crate::history::History;
    use crate::mesh::primitives::{self, Primitive};
    use crate::remesh::voxel_remesh;
    use crate::sculpt::{Brush, PointerSample, Sculptor};

    const SIZE: Vec2 = Vec2::new(1600.0, 900.0);

    fn ball() -> Mesh {
        primitives::build(Primitive::Sphere, 200_000, 0.5)
    }

    fn stretched(scale: Vec3) -> Scene {
        let mut scene = Scene::new("Torso");
        let object = scene.active_mut();
        (object.position, object.rotation, object.scale) = (Vec3::new(0.3, -2.0, 0.1), Quat::from_rotation_x(0.18), scale);
        scene
    }

    fn world(mesh: &Mesh, model: Mat4) -> Vec<Vec3> {
        mesh.positions.iter().map(|&p| model.transform_point3(p)).collect()
    }

    /// How wide and how tall a single dab lands on the front of the object, in
    /// the world, with a round brush on screen.
    fn footprint(scene: &Scene, mesh: &mut Mesh) -> (f32, f32) {
        let mut camera = Camera::default();
        camera.frame(scene.bounds(mesh), SIZE.x / SIZE.y, false);
        (camera.yaw, camera.pitch) = (0.0, 0.0);
        let view = camera.view(Vec2::ZERO, SIZE, camera.distance, 1.0);
        let model = scene.active().matrix();
        let local = view.local_to(scene.brush_matrix());

        let mut sculptor = Sculptor::default();
        let settings = &mut sculptor.settings;
        (settings.brush, settings.mask_mode, settings.screen_space_size, settings.radius_px) = (Brush::Standard, false, true, 40.0);
        settings.per_brush[Brush::Standard.index()].strength = 0.5;
        scene.hand_symmetry_to(&mut sculptor.settings);

        let at = SIZE * 0.5;
        let before = world(mesh, model);
        let sample = |i: usize| PointerSample { pos: at + Vec2::new(i as f32 * 0.01, 0.0), pressure: None, time: i as f64 / 240.0 };
        assert!(sculptor.begin(mesh, &local, sample(0), false, false), "the dab missed the object");
        sculptor.sample(mesh, &local, sample(1));
        sculptor.frame(mesh, &local, 1.0 / 60.0);
        let _ = sculptor.end(mesh, &local);
        let _ = mesh.take_dirty();

        let after = world(mesh, model);
        let moved = before.iter().zip(&after).filter(|(a, b)| a.distance(**b) > 1e-6).map(|(a, _)| *a);
        let (low, high) = moved.fold((Vec3::MAX, Vec3::MIN), |(low, high), p| (low.min(p), high.max(p)));
        assert!(high.x > low.x, "the dab moved nothing");
        (high.x - low.x, high.y - low.y)
    }

    #[test]
    fn a_dab_lands_stretched_on_a_stretched_object_and_round_once_the_scale_is_applied() {
        let scale = Vec3::new(1.2, 3.0, 1.0);
        let (mut scene, mut mesh) = (stretched(scale), ball());

        let (wide, tall) = footprint(&scene, &mut mesh);
        assert!(tall / wide > 2.0, "the fault this is for: {wide} wide, {tall} tall");

        let (mut scene_after, mut mesh_after) = (stretched(scale), ball());
        assert_eq!(scene_after.apply_scale(&mut mesh_after), Some(0));
        let (wide, tall) = footprint(&scene_after, &mut mesh_after);
        assert!((tall / wide - 1.0).abs() < 0.15, "now round: {wide} wide, {tall} tall");

        scene.active_mut().scale = Vec3::ONE;
        let (wide, tall) = footprint(&scene, &mut mesh);
        assert!((tall / wide - 1.0).abs() < 0.15, "an object never stretched: {wide} wide, {tall} tall");
    }

    #[test]
    fn nothing_moves_in_the_world_and_the_scale_comes_out_one() {
        let (mut scene, mut mesh) = (stretched(Vec3::new(1.2, -3.0, 1.0)), ball());
        let was_box = {
            let points = world(&mesh, scene.active().matrix());
            points.iter().fold((Vec3::MAX, Vec3::MIN), |(l, h), &p| (l.min(p), h.max(p)))
        };
        let before = scene.active().clone();
        assert_eq!(scene.apply_scale(&mut mesh), Some(0));

        let object = scene.active();
        assert_eq!(object.scale, Vec3::new(1.0, -1.0, 1.0));
        assert_eq!((object.position, object.rotation), (before.position, before.rotation));
        let points = world(&mesh, object.matrix());
        let now_box = points.iter().fold((Vec3::MAX, Vec3::MIN), |(l, h), &p| (l.min(p), h.max(p)));
        assert!(now_box.0.distance(was_box.0) < 1e-4 && now_box.1.distance(was_box.1) < 1e-4, "{now_box:?} against {was_box:?}");
        assert!(!scene.scale_is_uneven());
        assert_eq!(scene.apply_scale(&mut mesh), None, "a second press has nothing to do");
    }

    #[test]
    fn an_instance_looks_as_it_did_and_undo_puts_everything_back() {
        let (mut scene, mut mesh) = (stretched(Vec3::new(1.0, 3.0, 1.0)), ball());
        let mut history = History::default();
        let first = scene.active_id();
        let twin = scene.instance_active();
        scene.get_mut(twin).unwrap().scale = Vec3::splat(2.0);
        scene.select(first, &mut mesh);

        let box_of = |scene: &Scene, mesh: &Mesh| {
            let points = world(mesh, scene.get(twin).unwrap().matrix());
            points.iter().fold((Vec3::MAX, Vec3::MIN), |(l, h), &p| (l.min(p), h.max(p)))
        };
        let (uid, positions, objects) = (mesh.uid, mesh.positions.clone(), scene.objects.clone());
        let twin_was = box_of(&scene, &mesh);
        let before = scene.state();
        assert_eq!(scene.apply_scale(&mut mesh), Some(1));
        history.push_scene(before);
        history.settle(&mut scene);

        let twin_now = box_of(&scene, &mesh);
        assert!(twin_now.0.distance(twin_was.0) < 1e-4 && twin_now.1.distance(twin_was.1) < 1e-4, "the instance moved");
        assert_eq!(scene.get(twin).unwrap().scale, Vec3::new(2.0, 2.0 / 3.0, 2.0));

        assert!(history.step_scene(true, &mut scene, &mut mesh));
        history.settle(&mut scene);
        assert_eq!((mesh.uid, &mesh.positions, &scene.objects), (uid, &positions, &objects), "undo gives it all back to the bit");
    }

    #[test]
    fn a_remesh_of_the_applied_mesh_makes_triangles_even_in_the_world() {
        // The longest edge of a triangle against its shortest, in the world,
        // on average: an even mesh is near 1.
        let skew = |mesh: &Mesh, model: Mat4| {
            let points = world(mesh, model);
            let total: f32 = mesh.indices.chunks_exact(3).map(|t| {
                let side = |a: u32, b: u32| points[a as usize].distance(points[b as usize]);
                let sides = [side(t[0], t[1]), side(t[1], t[2]), side(t[2], t[0])];
                sides.iter().copied().fold(0.0, f32::max) / sides.iter().copied().fold(f32::MAX, f32::min).max(1e-9)
            }).sum();
            total / (mesh.indices.len() / 3) as f32
        };
        let scale = Vec3::new(1.0, 3.0, 1.0);

        let (scene, mesh) = (stretched(scale), ball());
        let plain = voxel_remesh(&mesh, 64, [false; 3]).unwrap();
        let stretched_skew = skew(&plain, scene.active().matrix());

        let (mut scene, mut mesh) = (stretched(scale), ball());
        scene.apply_scale(&mut mesh).unwrap();
        let even = voxel_remesh(&mesh, 64, [false; 3]).unwrap();
        let even_skew = skew(&even, scene.active().matrix());

        // What the remesher makes of the same ball never stretched at all.
        let plain_ball = voxel_remesh(&ball(), 64, [false; 3]).unwrap();
        let usual = skew(&plain_ball, Mat4::IDENTITY);
        assert!(stretched_skew > usual * 1.3, "left in its own space it comes out stretched: {stretched_skew} against {usual}");
        assert!(even_skew < usual * 1.15, "applied, it comes out as even as any: {even_skew} against {usual}");
    }
}
