//! Posing maths for the rig: the two-bone solve behind dragging a bone in
//! Move mode. Kept apart from the gestures so it can be tested on its own.

use glam::Vec3;

fn perpendicular(dir: Vec3) -> Vec3 {
    let reference = if dir.y.abs() < 0.9 * dir.length() { Vec3::Y } else { Vec3::X };
    dir.cross(reference)
}

/// The plane a two-bone chain bends in, as its normal, fixed at the start of
/// a drag. Keeping it fixed is what makes the pose stable: the elbow stays on
/// the side it started on however the hand is dragged around. A chain that
/// starts dead straight has no plane of its own, so it bends in the one
/// facing the camera, where the bend can be seen.
pub fn bend_normal(root: Vec3, joint: Vec3, end: Vec3, view_direction: Vec3) -> Vec3 {
    let n = (joint - root).cross(end - joint);
    let scale = (joint - root).length() * (end - joint).length();
    if scale > 0.0 && n.length() > scale * 0.02 {
        return n.normalize();
    }
    // The plane facing the camera has the view direction as its normal, made
    // perpendicular to the chain, which has to lie in the plane.
    let along = end - root;
    let along2 = along.length_squared();
    let n = if along2 > 0.0 {
        view_direction - along * (view_direction.dot(along) / along2)
    } else {
        view_direction
    };
    if n.length() > view_direction.length() * 1e-3 {
        return n.normalize();
    }
    // Looking straight down the chain: any perpendicular will do.
    perpendicular(along).try_normalize().unwrap_or(Vec3::Y)
}

/// Which side of the root-to-end line the joint sits on, in the bend plane:
/// +1 or -1.
pub fn bend_side(root: Vec3, joint: Vec3, end: Vec3, bend_normal: Vec3) -> f32 {
    let bend = bend_normal.cross(end - root);
    if bend.dot(joint - root) >= 0.0 { 1.0 } else { -1.0 }
}

/// Places a two-bone chain root -> joint -> end so the end reaches `target`
/// (or as far towards it as the bones allow), with both bone lengths kept and
/// the joint in the fixed bend plane on the fixed side. Returns (joint, end).
///
/// Analytic (law of cosines) rather than iterative, so it gives the same pose
/// for the same cursor position every time.
pub fn solve_two_bone(
    root: Vec3,
    upper: f32,
    lower: f32,
    target: Vec3,
    bend_normal: Vec3,
    bend_side: f32,
    fallback_direction: Vec3,
) -> (Vec3, Vec3) {
    let to_target = target - root;
    let mut distance = to_target.length();
    let dir = if distance > 1e-9 { to_target / distance } else { fallback_direction.normalize_or(Vec3::Y) };

    // Never quite straight and never folded flat: both are singular, and a
    // reach clamped a hair short of either reads identically.
    let reach = upper + lower;
    let min_reach = (upper - lower).abs();
    let slack = reach * 1e-4;
    distance = distance.clamp(min_reach + slack, (min_reach + slack).max(reach - slack));

    let mut bend = bend_normal.cross(dir);
    // Keep the bend perpendicular to the reach even if the normal no longer is.
    bend -= dir * bend.dot(dir);
    if bend.length() < 1e-6 {
        bend = perpendicular(dir);
    }
    let bend = bend.normalize() * bend_side;

    let cos_a = if upper > 0.0 && distance > 0.0 {
        ((upper * upper + distance * distance - lower * lower) / (2.0 * upper * distance)).clamp(-1.0, 1.0)
    } else {
        1.0
    };
    let sin_a = (1.0 - cos_a * cos_a).max(0.0).sqrt();
    (root + (dir * cos_a + bend * sin_a) * upper, root + dir * distance)
}
