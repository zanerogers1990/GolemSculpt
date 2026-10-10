//! Where the copies of an array are drawn, through the object's live mirror,
//! and which of them is under the pointer. Ported from `ArrayRepeater.SyncViews`
//! and `ArrayController.TryResolveCopy`.
//!
//! An array's copies are the object's own mesh drawn again, one `ArrayPlacement`
//! each. When the object also has a live mirror (Nomad's Mirror, the Mirror
//! section of the Objects card, `ObjectInfo::mirror`), the array is mirrored
//! with it: after the straight copies come the same copies again, reflected
//! across each of the mirror's planes, so a strap drawn down one side is on the
//! other side too. The copy count of an array is `count - 1` besides the object
//! (Unity's `perSide`), and the views are `perSide * (1 + mirror copies)`.
//!
//! The object itself is not among them, and neither are its own reflections:
//! those are the mirror's copies, which the app draws. What this module calls a
//! *side* is a flip mask, 0 for the straight copies (X = 1, Y = 2, Z = 4 for the
//! mirror's), the same numbers `ObjectInfo::placements` uses. A copy "seen
//! through a side" is the world reflection of the straight copy, after its own
//! matrix, which is exactly how the object's own mirror copies are drawn and
//! what the Unity repeater does with a negative scale on the mirrored axes.

use glam::{Mat4, Quat, Vec3};
use sculpt_geom::mirror::{reflect_point, reflect_rotation};
use sculpt_toolkit::{MirrorCopy, MirrorRepeater, ObjectId, Placement, Ray, Scene};

use crate::types::{ArrayPlacement, Pose};

/// Whether `side` is one the mirror draws: 0, the straight copies, always, and
/// otherwise a flip mask that is a combination of the mirror's own axes.
pub fn has_side(mirror: Option<MirrorRepeater>, side: u8) -> bool {
    side == 0 || mirror.is_some_and(|m| side & !m.axis_mask == 0)
}

/// Every side there is, the straight copies first, then each of the mirror's
/// copies in flip-mask order.
pub fn sides(mirror: Option<MirrorRepeater>) -> Vec<u8> {
    let copies = mirror.into_iter().flat_map(|m| m.copies().into_iter().map(|c| c.flip_mask));
    std::iter::once(0).chain(copies).collect()
}

/// `matrix` (a copy's place in the world) as seen through `side`: itself for
/// the straight copies, else the world reflection of it. `None` for a side the
/// mirror does not have.
pub fn through(mirror: Option<MirrorRepeater>, side: u8, matrix: Mat4) -> Option<Mat4> {
    if side == 0 {
        return Some(matrix);
    }
    let mirror = mirror.filter(|m| has_side(Some(*m), side))?;
    Some(MirrorCopy::new(side).matrix(&matrix, mirror.center))
}

/// A pose (a copy's place, in the world) as seen through `side`, with a
/// rotation that is always a proper one: what a gizmo can sit on. For a mirror
/// side that is the baked copy's pose, the same as the live copy's rendering
/// (see `MirrorCopy::baked_placement`). Reflection is its own inverse, so the
/// same call turns what a gizmo reports on a copy back into the straight
/// copy's pose.
pub fn seen_as(mirror: Option<MirrorRepeater>, side: u8, pose: &Pose) -> Option<Placement> {
    if side == 0 {
        return Some(Placement { position: pose.position, rotation: pose.rotation, scale: pose.scale });
    }
    let mirror = mirror.filter(|m| has_side(Some(*m), side))?;
    let signs = MirrorCopy::new(side).signs;
    Some(Placement {
        position: reflect_point(pose.position, mirror.center, signs),
        rotation: reflect_rotation(pose.rotation, signs),
        scale: pose.scale,
    })
}

/// The same call for a bare position and rotation.
pub fn reflected(mirror: Option<MirrorRepeater>, side: u8, position: Vec3, rotation: Quat) -> Option<(Vec3, Quat)> {
    let pose = Pose::new(position, rotation, Vec3::ONE);
    seen_as(mirror, side, &pose).map(|p| (p.position, p.rotation))
}

/// The matrices of every copy to draw for an array whose object is posed by
/// `pose`: the straight copies from copy `first` on (1 leaves the object's own
/// place to the app, 0 draws the object too, for when the tool is showing it
/// somewhere the scene does not have it yet), then for each side of the mirror
/// the same copies again, in the order the Unity repeater lays its views out.
pub fn drawn(layout: &[ArrayPlacement], pose: &Pose, mirror: Option<MirrorRepeater>, first: usize) -> Vec<Mat4> {
    let straight: Vec<Mat4> = layout.iter().skip(first).map(|p| p.world_matrix(pose)).collect();
    let mut all = straight.clone();
    for side in sides(mirror).into_iter().skip(1) {
        all.extend(straight.iter().filter_map(|&m| through(mirror, side, m)));
    }
    all
}

/// One place an array's mesh is drawn: which copy of the array, seen through
/// which side of the mirror, and the matrix that puts the mesh there. Copy 0 is
/// the object, so copy 0 on side 0 is the object itself and on any other side
/// one of its own mirror copies.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    pub copy: usize,
    pub side: u8,
    pub matrix: Mat4,
}

/// Every place the mesh is drawn for an array, the object and its mirror
/// copies included, copy by copy.
pub fn places(layout: &[ArrayPlacement], pose: &Pose, mirror: Option<MirrorRepeater>) -> Vec<Place> {
    let sides = sides(mirror);
    let mut places = Vec::with_capacity(layout.len() * sides.len());
    for (copy, placement) in layout.iter().enumerate() {
        let matrix = placement.world_matrix(pose);
        for &side in &sides {
            if let Some(matrix) = through(mirror, side, matrix) {
                places.push(Place { copy, side, matrix });
            }
        }
    }
    places
}

/// The nearest of `places` a world ray meets, with how far along the ray: the
/// object's mesh is cast against through each place's own matrix, which is how
/// the scene says which copy a click landed on.
pub fn pick(scene: &dyn Scene, object: ObjectId, places: &[Place], ray: &Ray) -> Option<(f32, Place)> {
    places
        .iter()
        .filter_map(|place| scene.raycast_placed(object, &place.matrix, ray).map(|hit| (hit.distance, *place)))
        .min_by(|a, b| a.0.total_cmp(&b.0))
}

#[cfg(test)]
mod tests {
    use glam::Quat;

    use super::*;
    use crate::layout::compute;
    use crate::types::{ArrayKind, ArraySettings};

    fn line(count: i32, end: Vec3) -> Vec<ArrayPlacement> {
        let mut settings = ArraySettings::new(ArrayKind::Line);
        settings.count = count;
        settings.space_by_size = false;
        compute(&settings, &[Vec3::ZERO, end])
    }

    /// The Unity tests' object: off to one side, turned and scaled.
    fn pose() -> Pose {
        Pose::new(Vec3::new(0.7, 0.2, -0.3), Quat::from_euler(glam::EulerRot::XYZ, 0.17, 0.52, -0.35), Vec3::splat(0.8))
    }

    fn mirror(axes: u8) -> Option<MirrorRepeater> {
        MirrorRepeater::new(axes, Vec3::new(0.1, 0.0, 0.0))
    }

    #[test]
    fn the_sides_are_the_straight_copies_then_each_of_the_mirrors() {
        assert_eq!(sides(None), [0]);
        assert_eq!(sides(mirror(1)), [0, 1]);
        assert_eq!(sides(mirror(1 | 4)), [0, 1, 4, 5]);
        assert_eq!(sides(mirror(7)).len(), 8);
        assert!(has_side(mirror(1 | 4), 5) && !has_side(mirror(1 | 4), 2) && has_side(None, 0) && !has_side(None, 1));
        assert!(!has_side(mirror(7), 8), "a flip mask is three bits");
    }

    #[test]
    fn an_unmirrored_arrays_copies_are_the_layout_from_the_first_asked_for() {
        let layout = line(5, Vec3::new(4.0, 0.0, 2.0));
        let pose = pose();
        let all = drawn(&layout, &pose, None, 0);
        assert_eq!(all.len(), 5);
        assert_eq!(all[0], pose.matrix(), "the object itself, when it is asked for");
        let rest = drawn(&layout, &pose, None, 1);
        assert_eq!(rest, all[1..], "and left to the app otherwise");
        for (i, m) in rest.iter().enumerate() {
            let expected = pose.position + Vec3::new(4.0, 0.0, 2.0) * ((i + 1) as f32 / 4.0);
            assert!(m.transform_point3(Vec3::ZERO).distance(expected) < 1e-4, "copy {}", i + 1);
        }
    }

    /// The Unity test `AMirroredObjectsArrayIsMirroredToo`.
    #[test]
    fn a_mirrored_objects_array_is_mirrored_too() {
        let layout = line(4, Vec3::new(0.0, 3.0, 1.0));
        let pose = pose();
        let center = Vec3::new(0.1, 0.0, 0.0);
        let one = drawn(&layout, &pose, mirror(1), 1);
        assert_eq!(one.len(), 6, "three copies, and their three reflections");
        let signs = Vec3::new(-1.0, 1.0, 1.0);
        for i in 0..3 {
            let (straight, reflected) = (one[i], one[3 + i]);
            let local = Vec3::new(0.3, -0.2, 0.4);
            // Any point of the object's shape lands on its mirror image.
            let want = reflect_point(straight.transform_point3(local), center, signs);
            assert!(reflected.transform_point3(local).distance(want) < 1e-4, "copy {i} is the mirror image");
            assert!(straight.determinant() > 0.0 && reflected.determinant() < 0.0, "copy {i} is drawn reflected");
            // Where the copy sits is where its partner's reflection sits.
            let place = reflect_point(straight.transform_point3(Vec3::ZERO), center, signs);
            assert!(reflected.transform_point3(Vec3::ZERO).distance(place) < 1e-4);
        }

        // Two axes: three reflections of each copy, and the X and Z one is a rotation.
        let two = drawn(&layout, &pose, mirror(1 | 4), 1);
        assert_eq!(two.len(), 12);
        let dets: Vec<bool> = two.iter().map(|m| m.determinant() < 0.0).collect();
        assert_eq!(dets[0..3], [false; 3]);
        assert_eq!(dets[3..6], [true; 3], "X");
        assert_eq!(dets[6..9], [true; 3], "Z");
        assert_eq!(dets[9..12], [false; 3], "X and Z together is a rotation, not a reflection");

        assert_eq!(drawn(&layout, &pose, mirror(7), 1).len(), 3 * 8);
        assert_eq!(drawn(&layout, &pose, None, 1).len(), 3, "mirror off: only the straight copies are left");
    }

    #[test]
    fn the_places_are_every_copy_on_every_side_and_the_object_is_the_first() {
        let layout = line(3, Vec3::X * 2.0);
        let pose = pose();
        let all = places(&layout, &pose, mirror(1));
        assert_eq!(all.len(), 6);
        assert_eq!((all[0].copy, all[0].side, all[0].matrix), (0, 0, pose.matrix()));
        assert_eq!((all[1].copy, all[1].side), (0, 1), "the object's own mirror copy");
        assert_eq!((all[2].copy, all[2].side), (1, 0));
        assert_eq!((all[5].copy, all[5].side), (2, 1));
        assert_eq!(places(&layout, &pose, None).len(), 3);
    }

    #[test]
    fn a_copy_seen_through_a_side_is_the_proper_pose_of_its_reflection() {
        let pose = pose();
        let m = mirror(1);
        let seen = seen_as(m, 1, &pose).unwrap();
        let center = Vec3::new(0.1, 0.0, 0.0);
        assert!(seen.position.distance(reflect_point(pose.position, center, Vec3::new(-1.0, 1.0, 1.0))) < 1e-6);
        assert!(Placement { scale: Vec3::ONE, ..seen }.matrix().determinant() > 0.0, "a gizmo can sit on it");
        // And it is where the live copy is drawn: the baked copy's matrix is the live one's, mesh reflected.
        let live = through(m, 1, pose.matrix()).unwrap();
        let baked = Mat4::from_scale_rotation_translation(seen.scale, seen.rotation, seen.position)
            * Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0));
        for local in [Vec3::new(0.3, -0.2, 0.4), Vec3::new(-1.0, 0.5, 0.25)] {
            assert!(live.transform_point3(local).distance(baked.transform_point3(local)) < 1e-5);
        }
        // Reflection is its own inverse.
        let (back_at, back_by) = reflected(m, 1, seen.position, seen.rotation).unwrap();
        assert!(back_at.distance(pose.position) < 1e-6 && back_by.abs_diff_eq(pose.rotation, 1e-6));
        assert!(seen_as(m, 2, &pose).is_none() && seen_as(None, 1, &pose).is_none());
        assert_eq!(seen_as(None, 0, &pose).unwrap().position, pose.position);
    }
}
