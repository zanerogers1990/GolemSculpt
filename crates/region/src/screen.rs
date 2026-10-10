//! The app's camera as `sculpt_cut` wants it: the matrices that put an
//! object's own vertices under a shape drawn on screen.
//!
//! `sculpt_cut` asks for two matrices per place an object is drawn: model to
//! clip space, which decides what a shape covers, and model to view space,
//! which gives the depth a cut face is laid out along. A tool is handed the
//! first (`View::view_proj`) but not the view matrix on its own, so that is
//! rebuilt here from the camera's position and axes, exactly as the app
//! builds it. Screen positions need no conversion: the pointer, the overlay
//! and `sculpt_cut` all use viewport pixels with (0, 0) at the top-left.

use glam::{Mat4, Vec2, Vec4};
use sculpt_cut::CutView;
use sculpt_geom::SymmetryOp;
use sculpt_toolkit::{Env, ObjectId, ObjectInfo, Scene, View};

/// World space into the camera's own: x right, y up, looking down -z, the
/// eye at the origin. The app's own view matrix (see `src/camera.rs`).
pub fn world_to_view(view: &View) -> Mat4 {
    let (right, up, forward) = (view.right, view.up, view.forward);
    Mat4::from_cols(
        Vec4::new(right.x, up.x, -forward.x, 0.0),
        Vec4::new(right.y, up.y, -forward.y, 0.0),
        Vec4::new(right.z, up.z, -forward.z, 0.0),
        Vec4::new(-right.dot(view.eye), -up.dot(view.eye), forward.dot(view.eye), 1.0),
    )
}

/// The camera as it sees an object placed by `model` (its own space into
/// world space).
pub fn cut_view(view: &View, model: Mat4) -> CutView {
    CutView::new(view.view_proj * model, world_to_view(view) * model, Vec2::ZERO, view.size)
}

/// One view per place `object`'s mesh is drawn: the object itself first, then
/// its live mirror copies and the copies another tool keeps of it (the copies
/// of an array, `ObjectInfo::live_copies`), then every other shown object that
/// shares its mesh (its instances) with theirs. A shape drawn over any of
/// these covers the vertices that place shows, which are the object's own:
/// the places of the Unity app's `MeshFrameMatrices`, in its order, which
/// walks the mirror copies, the instances and the array copies in turn.
///
/// A copy is told apart from the object by its matrix alone: the object's
/// matrix with a world reflection after it for a mirror copy, and for an array
/// copy the object turned, moved and resized by the array's layout (a
/// tapered array's copies are smaller, and the matrix carries that), seen
/// through the mirror too when the array is mirrored. For an odd number of
/// mirrored axes the determinant is negative, and seen through it the mesh
/// is inside out. Nothing the cuts do reads that sign, so the reflected
/// matrix goes in as it is, exactly as the Unity app hands its trimmer the
/// copy's `localToWorldMatrix`. A shape covers whatever lies
/// under it, front and back, so there is no facing to get the wrong way
/// round. The cut is made on the object's mesh in the object's own space, and
/// the view only says where each vertex lands on screen and how deep: a
/// cut loop is closed on the swept surface, in the chart of arc length against
/// view depth, and every cap, ring and wall takes its winding from the
/// direction the shell walks the rim it is sewn to (see `sculpt_cut::trim`).
/// So the mesh comes out wound as it went in, however the view turned it.
/// `tests/mirror.rs` and `tests/copies.rs` hold the proof: each cut is
/// checked against the same shape drawn over the copy baked into an object of
/// its own.
///
/// A copy is shown only while its object is: a hidden object's copies are
/// hidden with it, and the shape cannot be drawn over what is not there. The
/// object's own view is always first, since the tools refuse to start on a
/// hidden object and an instance's shape is drawn over a shown one.
pub fn cut_views(scene: &dyn Scene, object: ObjectId, view: &View) -> Vec<CutView> {
    let Some(info) = scene.info(object) else { return Vec::new() };
    let mut shown = vec![(info, true)];
    for other in scene.objects() {
        if other == object {
            continue;
        }
        if let Some(instance) = scene.info(other).filter(|o| o.mesh == info.mesh && o.visible) {
            shown.push((instance, false));
        }
    }
    // What each object's own settings give, then the copies tools keep: the
    // order of the Unity app's list, which walks the copies, the instances and
    // the array copies.
    let own = shown.iter().flat_map(|(info, even_hidden)| own_places(info, *even_hidden));
    let kept = shown.iter().filter(|(info, _)| info.visible).flat_map(|(info, _)| info.live_copies.iter().copied());
    own.chain(kept).map(|model| cut_view(view, model)).collect()
}

/// The places one object's own settings give its mesh: itself (unless it is
/// hidden and `even_hidden` is off) and, while it is shown, each of its live
/// mirror copies.
fn own_places(info: &ObjectInfo<'_>, even_hidden: bool) -> Vec<Mat4> {
    let wanted = |copy: u8| info.visible || (copy == 0 && even_hidden);
    info.placements().into_iter().filter(|&(copy, _)| wanted(copy)).map(|(_, model)| model).collect()
}

/// The brushes' symmetry as the list of operations a gesture repeats under:
/// the identity first, then every other copy a stroke would be made as, the
/// mirror images and the turns round the radial axis alike. They are in the
/// object's own space, so they repeat the same way under every view of it:
/// the object, its instances, its live mirror copies and the copies tools keep.
pub fn symmetry(env: &Env) -> Vec<SymmetryOp> {
    env.symmetry().ops().to_vec()
}

#[cfg(test)]
mod tests {
    use glam::{Quat, Vec3};

    use super::*;

    fn views() -> [View; 2] {
        let size = Vec2::new(1600.0, 960.0);
        let eye = Vec3::new(1.5, 1.0, 4.0);
        [View::perspective(eye, Vec3::ZERO, 60.0, size), View::orthographic(eye, Vec3::new(0.2, -0.1, 0.0), 2.0, size)]
    }

    fn model() -> Mat4 {
        Mat4::from_scale_rotation_translation(
            Vec3::new(1.4, 0.7, 1.1),
            Quat::from_euler(glam::EulerRot::YXZ, 0.7, -0.4, 0.2),
            Vec3::new(0.6, -0.3, 0.4),
        )
    }

    #[test]
    fn an_objects_point_lands_on_the_pixel_the_app_draws_it_at() {
        let model = model();
        for view in views() {
            let cut = cut_view(&view, model);
            for local in [Vec3::ZERO, Vec3::new(0.3, -0.4, 0.2), Vec3::new(-0.8, 0.5, -0.6)] {
                let world = model.transform_point3(local);
                let on_screen = view.project(world).unwrap();
                let through_cut = cut.projection.project(local).unwrap();
                assert!(through_cut.distance(on_screen) < 1e-2, "{through_cut} is not {on_screen}");
                // View space looks down -z: depth in front of the eye is -z.
                let depth = -cut.model_to_view.transform_point3(local).z;
                assert!((depth - view.depth_of(world)).abs() < 1e-4, "{depth} is not {}", view.depth_of(world));
            }
        }
    }

    /// What `sculpt_cut` relies on to lay a cut face out: with the view
    /// matrix taken off, what is left of the projection gives a clip w that
    /// depends on depth alone.
    #[test]
    fn the_rebuilt_view_matrix_leaves_a_plain_projection() {
        for view in views() {
            let projection = view.view_proj * world_to_view(&view).inverse();
            assert!(projection.x_axis.w.abs() < 1e-5 && projection.y_axis.w.abs() < 1e-5, "{projection:?}");
            assert!(projection.x_axis.y.abs() < 1e-5 && projection.y_axis.x.abs() < 1e-5);
        }
    }

    #[test]
    fn mirror_axes_become_the_flips_to_repeat_under() {
        let mut env = Env::at_rest(views()[0]);
        assert_eq!(symmetry(&env), vec![SymmetryOp::IDENTITY]);
        env.mirror = [true, false, false];
        let x = symmetry(&env);
        assert_eq!(x.len(), 2);
        assert!(x[0].is_identity());
        assert_eq!(x[1].sign(), Vec3::new(-1.0, 1.0, 1.0));
        env.mirror = [true, false, true];
        assert_eq!(symmetry(&env).len(), 4);
    }

    #[test]
    fn radial_symmetry_adds_the_turns_to_repeat_under() {
        let mut env = Env::at_rest(views()[0]);
        (env.radial_count, env.radial_axis) = (6, Vec3::Z);
        let turns = symmetry(&env);
        assert_eq!(turns.len(), 6);
        assert!(turns[0].is_identity());
        let p = Vec3::new(0.4, 0.1, 0.3);
        for (k, turn) in turns.iter().enumerate() {
            let angle = std::f32::consts::TAU * k as f32 / 6.0;
            let turned = Vec3::new(p.x * angle.cos() - p.y * angle.sin(), p.x * angle.sin() + p.y * angle.cos(), p.z);
            assert!(turn.apply_point(p).distance(turned) < 1e-6, "turn {k}");
        }
        // With a mirror plane through the axis: each turn and its reflection.
        env.mirror = [true, false, false];
        assert_eq!(symmetry(&env).len(), 12);
        // A count below 2 is no radial symmetry at all.
        for count in [0, 1] {
            env.radial_count = count;
            assert_eq!(symmetry(&env).len(), 2);
        }
    }
}
