//! The copies tools keep of an object (an array's), on the scene's side.
//!
//! A copy of an array is not an object and not the object's own setting: the
//! Array tool keeps the layout, and says every frame where the copies are
//! (`Draw::live_copies`). The app hands that on to the scene (`set_tool_places`,
//! from `app/tools.rs`), and from then on each copy is one more place the
//! object's mesh is drawn, like a live mirror copy or an instance:
//!
//! - a ray meets it (`Scene::pick`), so a click on a copy picks the object;
//! - a stroke that begins on it is mapped through its matrix
//!   (`Scene::aim_brush`), so it sculpts exactly the vertices the copy shows
//!   there and every other copy follows, as in the Unity app, where an array's
//!   views are among the copies `RaycastAnyCopy` walks;
//! - the bounds the camera frames include it;
//! - tools read it as `ObjectInfo::live_copies`, so the region tools cut
//!   through it.
//!
//! The scene keeps them with no undo and no save: they are the tool's, and
//! the tool says them again every frame, so a copy that goes (the array is
//! made real, or taken off, or undone) is gone from here by the next frame.
//! A place is named by a code, which is also how a stroke is aimed: 0 for the
//! object, 1 to 7 for a live mirror copy (its flip mask), and `TOOL_COPY + i`
//! for the i-th copy a tool keeps.

use std::collections::HashMap;

use glam::Mat4;

use super::{Object, Scene};

/// The code of the first copy a tool keeps. Below it a code is a flip mask.
pub const TOOL_COPY: u16 = 8;

/// The most copies of one object the scene takes from tools; an array is
/// limited to a hundred, on each side of up to three mirrors.
const MOST_PER_OBJECT: usize = 4096;

impl Scene {
    /// Takes in the copies tools kept of objects in their last frame (object
    /// id, and each copy's matrix into world space), in place of whatever was
    /// said before.
    pub fn set_tool_places(&mut self, mut places: HashMap<u64, Vec<Mat4>>) {
        for copies in places.values_mut() {
            copies.truncate(MOST_PER_OBJECT);
        }
        self.tool_places = places;
    }

    /// The copies of an object that tools keep, as they last said.
    pub fn tool_places(&self, id: u64) -> &[Mat4] {
        self.tool_places.get(&id).map_or(&[], Vec::as_slice)
    }

    /// Whether `code` names a place `object` has now.
    pub fn has_place(&self, object: &Object, code: u16) -> bool {
        match code.checked_sub(TOOL_COPY) {
            Some(i) => usize::from(i) < self.tool_places(object.id).len(),
            None => code == 0 || object.has_copy(code as u8),
        }
    }

    /// The matrix of one place of `object`: where its mesh is drawn there.
    /// The object's own for a code it has no place for.
    pub fn place_matrix(&self, object: &Object, code: u16) -> Mat4 {
        match code.checked_sub(TOOL_COPY) {
            Some(i) => self.tool_places(object.id).get(usize::from(i)).copied().unwrap_or_else(|| object.matrix()),
            None => object.placed_matrix(code as u8),
        }
    }

    /// Every place an object's mesh is drawn, with its code: the object, its
    /// live mirror copies, then the copies tools keep.
    pub fn places_of<'a>(&'a self, object: &'a Object) -> impl Iterator<Item = (u16, Mat4)> + 'a {
        let own = object.placements().map(|(copy, model)| (u16::from(copy), model));
        let kept = self.tool_places(object.id).iter().enumerate().map(|(i, model)| (TOOL_COPY + i as u16, *model));
        own.chain(kept)
    }
}

#[cfg(test)]
mod tests {
    use glam::{Quat, Vec2, Vec3};

    use super::*;
    use crate::mesh::Mesh;
    use crate::mesh::primitives::{self, Primitive};

    fn cube() -> Mesh {
        primitives::build(Primitive::Cube, 600, 0.5)
    }

    fn at(x: f32) -> Mat4 {
        Mat4::from_translation(Vec3::new(x, 0.0, 0.0))
    }

    /// One cube at the origin, a row of copies kept for it to the right.
    fn row() -> (Scene, Mesh, u64) {
        let mesh = cube();
        let mut scene = Scene::new("Cube");
        let id = scene.active_id();
        scene.set_tool_places(HashMap::from([(id, vec![at(2.0), at(4.0), at(6.0)])]));
        (scene, mesh, id)
    }

    #[test]
    fn a_ray_meets_each_copy_a_tool_keeps_and_names_it() {
        let (scene, mesh, id) = row();
        for (i, x) in [0.0, 2.0, 4.0, 6.0].into_iter().enumerate() {
            let hit = scene.pick(Vec3::new(x, 0.0, 5.0), -Vec3::Z, &mesh).expect("a cube is there");
            assert_eq!(hit.id, id);
            assert_eq!(usize::from(hit.copy), if i == 0 { 0 } else { usize::from(TOOL_COPY) + i - 1 }, "x = {x}");
            assert!((hit.pos - Vec3::new(x, 0.0, 0.5)).length() < 1e-4);
        }
        assert!(scene.pick(Vec3::new(1.0, 0.0, 5.0), -Vec3::Z, &mesh).is_none(), "between the copies");
        assert!(scene.pick(Vec3::new(8.0, 0.0, 5.0), -Vec3::Z, &mesh).is_none(), "past the last");
    }

    #[test]
    fn what_a_tool_says_next_replaces_what_it_said_and_a_hidden_object_has_no_copies_to_hit() {
        let (mut scene, mesh, id) = row();
        scene.set_tool_places(HashMap::from([(id, vec![at(4.0)])]));
        assert!(scene.pick(Vec3::new(2.0, 0.0, 5.0), -Vec3::Z, &mesh).is_none());
        assert!(scene.pick(Vec3::new(4.0, 0.0, 5.0), -Vec3::Z, &mesh).is_some());
        scene.set_tool_places(HashMap::new());
        assert!(scene.pick(Vec3::new(4.0, 0.0, 5.0), -Vec3::Z, &mesh).is_none());
        assert!(scene.tool_places(id).is_empty());

        scene.set_tool_places(HashMap::from([(id, vec![at(4.0)])]));
        scene.get_mut(id).unwrap().visible = false;
        assert!(scene.pick(Vec3::new(4.0, 0.0, 5.0), -Vec3::Z, &mesh).is_none());
    }

    #[test]
    fn a_stroke_aimed_at_a_copy_is_mapped_through_the_copys_matrix() {
        let (mut scene, _mesh, id) = row();
        let second = TOOL_COPY + 1;
        scene.aim_brush(id, second);
        assert_eq!(scene.brush_matrix(), at(4.0));
        assert_eq!((scene.brush_object().id, scene.brush_copy()), (id, second));
        assert_eq!(scene.stroke_matrix(), at(4.0));
        // A copy the tool no longer keeps is the object's own.
        scene.set_tool_places(HashMap::from([(id, vec![at(2.0)])]));
        assert_eq!(scene.brush_copy(), 0);
        assert_eq!(scene.brush_matrix(), scene.active().matrix());
        assert!(scene.release_brush());
    }

    #[test]
    fn the_fewest_and_the_most_copies_a_tool_may_keep() {
        let (mut scene, mesh, id) = row();
        // None, and one: the codes start at TOOL_COPY and nothing else is a copy.
        scene.set_tool_places(HashMap::from([(id, Vec::new())]));
        let object = scene.active().clone();
        assert!(scene.tool_places(id).is_empty() && !scene.has_place(&object, TOOL_COPY));
        scene.set_tool_places(HashMap::from([(id, vec![at(2.0)])]));
        assert!(scene.has_place(&object, TOOL_COPY) && !scene.has_place(&object, TOOL_COPY + 1));
        assert_eq!(scene.pick(Vec3::new(2.0, 0.0, 5.0), -Vec3::Z, &mesh).map(|p| p.copy), Some(TOOL_COPY));

        // More than the scene takes are cut off at the limit, and the last one it keeps still works.
        let many: Vec<Mat4> = (0..5000).map(|i| at(2.0 + 2.0 * i as f32)).collect();
        scene.set_tool_places(HashMap::from([(id, many)]));
        let kept = scene.tool_places(id).len();
        assert_eq!(kept, 4096);
        let last = TOOL_COPY + (kept - 1) as u16;
        assert!(scene.has_place(&object, last) && !scene.has_place(&object, last + 1));
        let x = 2.0 + 2.0 * (kept - 1) as f32;
        assert_eq!(scene.pick(Vec3::new(x, 0.0, 5.0), -Vec3::Z, &mesh).map(|p| p.copy), Some(last));
    }

    #[test]
    fn the_object_and_a_mirror_copy_keep_their_codes_and_a_tools_copies_come_after() {
        let (mut scene, _mesh, id) = row();
        scene.set_mirror(id, 1, Vec3::ZERO);
        let object = scene.active().clone();
        let codes: Vec<u16> = scene.places_of(&object).map(|(code, _)| code).collect();
        assert_eq!(codes, [0, 1, TOOL_COPY, TOOL_COPY + 1, TOOL_COPY + 2]);
        assert!(scene.has_place(&object, 1) && !scene.has_place(&object, 2) && scene.has_place(&object, TOOL_COPY + 2));
        assert!(!scene.has_place(&object, TOOL_COPY + 3));
        assert_eq!(scene.place_matrix(&object, 1), object.placed_matrix(1));
        assert_eq!(scene.place_matrix(&object, TOOL_COPY + 2), at(6.0));
        assert_eq!(scene.place_matrix(&object, TOOL_COPY + 9), object.matrix(), "no such copy: the object's own");
    }

    /// The copies of a real array: a ring of six round a centre to the right of the object, turning
    /// as they go round and shrinking toward the last, with the object's mirror across X as well, so
    /// that the far side has the same ring reflected.
    fn ring_of_copies(scene: &Scene) -> Vec<Mat4> {
        use sculpt_array::{ArrayKind, ArraySettings, compute_layout, drawn_copies};
        let object = scene.active();
        let mut settings = ArraySettings::new(ArrayKind::Radial);
        (settings.count, settings.axis, settings.end_scale) = (6, 2, 0.5);
        let layout = compute_layout(&settings, &[Vec3::ZERO, Vec3::new(1.8, 0.0, 0.0)]);
        let pose = sculpt_array::Pose::new(object.position, object.rotation, object.scale);
        drawn_copies(&layout, &pose, object.mirror, 1)
    }

    #[test]
    fn a_brush_stroke_on_a_copy_of_an_array_sculpts_the_vertices_that_copy_shows() {
        use crate::camera::Camera;
        use crate::sculpt::{Brush, PointerSample, Sculptor};

        let mesh = primitives::build(Primitive::Sphere, 60_000, 0.5);
        let mut scene = Scene::new("Sphere");
        scene.active_mut().position = Vec3::new(1.2, 0.3, 0.0);
        scene.active_mut().scale = Vec3::new(0.8, 0.7, 0.75);
        let id = scene.active_id();
        scene.set_mirror(id, 1, Vec3::ZERO);
        let copies = ring_of_copies(&scene);
        assert_eq!(copies.len(), 10, "five copies, and their reflections");
        scene.set_tool_places(HashMap::from([(id, copies.clone())]));

        let mut camera = Camera::default();
        camera.frame(scene.bounds(&mesh), 1600.0 / 900.0, false);
        let view = camera.view(Vec2::ZERO, Vec2::new(1600.0, 900.0), camera.distance, 1.0);

        // The ring turns the copies, shrinks them, and the mirror reflects half: every kind of matrix.
        let reflected = copies.iter().filter(|m| m.determinant() < 0.0).count();
        assert_eq!(reflected, 5);
        let sizes: Vec<f32> = copies.iter().map(|m| m.determinant().abs().cbrt()).collect();
        assert!(sizes[3] < sizes[0] * 0.8, "the copies shrink: {sizes:?}");

        for (k, copy) in copies.iter().enumerate().filter(|(k, _)| k % 4 == 1) {
            let centre = view.project(copy.transform_point3(Vec3::ZERO)).expect("the copy is in view");
            let (origin, dir) = view.ray(centre);
            let seen = scene.pick(origin, dir, &mesh).expect("a copy is under the pointer");
            assert_eq!((seen.id, usize::from(seen.copy)), (id, usize::from(TOOL_COPY) + k), "copy {k}");
            scene.aim_brush(seen.id, seen.copy);
            assert_eq!(scene.brush_matrix(), *copy);
            let local = view.local_to(scene.brush_matrix());

            // Each stroke on a fresh ball: a stroke leaves the sculpting structures as it likes.
            let mut mesh = primitives::build(Primitive::Sphere, 60_000, 0.5);
            let before = mesh.positions.clone();
            let mut sculptor = Sculptor::default();
            sculptor.settings.brush = Brush::Inflate;
            sculptor.settings.symmetry = [false; 3];
            sculptor.settings.radius_px = 30.0;
            let at = |i: usize| PointerSample {
                pos: centre + Vec2::new(i as f32 * 2.0 - 10.0, 0.0),
                pressure: None,
                time: i as f64 / 60.0,
            };
            assert!(sculptor.begin(&mut mesh, &local, at(0), false, false), "copy {k}: the stroke missed it");
            for i in 1..=10 {
                sculptor.sample(&mut mesh, &local, at(i));
                sculptor.frame(&mut mesh, &local, 1.0 / 60.0);
            }
            sculptor.end(&mut mesh, &local);

            let moved: Vec<usize> = (0..before.len()).filter(|&v| mesh.positions[v] != before[v]).collect();
            assert!(!moved.is_empty(), "copy {k}: the stroke changed nothing");
            // What moved is what that copy shows under the stroke, and it was pushed out of the copy.
            let middle = copy.transform_point3(Vec3::ZERO);
            for &v in &moved {
                let px = view.project(copy.transform_point3(before[v])).unwrap();
                assert!(px.distance(centre) < 60.0, "copy {k}: moved a vertex {px} far from the stroke");
                let (was, now) = (copy.transform_point3(before[v]), copy.transform_point3(mesh.positions[v]));
                assert!(now.distance(middle) > was.distance(middle) - 1e-6, "copy {k}: pushed into the copy");
            }
            // Through the object's own matrix those vertices are somewhere else in the picture.
            let own = scene.active().matrix();
            let far = |&&v: &&usize| view.project(own.transform_point3(before[v])).unwrap().distance(centre) > 100.0;
            let away = moved.iter().filter(far).count();
            assert!(away * 2 > moved.len(), "copy {k}: the stroke went through the object instead");
            scene.release_brush();
            assert_eq!(scene.brush_matrix(), scene.active().matrix());
        }
    }

    #[test]
    fn the_bounds_the_camera_frames_take_in_every_copy() {
        let (mut scene, mesh, id) = row();
        let alone = scene.bounds(&mesh).size().x;
        assert!((alone - 7.0).abs() < 1e-4, "{alone}: the cube at 0 and the last copy at 6, each a unit wide");
        scene.set_tool_places(HashMap::new());
        assert!((scene.bounds(&mesh).size().x - 1.0).abs() < 1e-4);
        // A turned and moved copy is boxed as a mirror copy is.
        let turned = Mat4::from_rotation_translation(Quat::from_rotation_z(0.5), Vec3::new(0.0, 5.0, 0.0));
        scene.set_tool_places(HashMap::from([(id, vec![turned])]));
        assert!(scene.bounds(&mesh).max.y > 5.5);
    }
}
