//! Where every copy of an array goes: pure maths from the settings and the
//! handle points, with no scene in it.
//!
//! Points are offsets from the object's own position, in world axes. `points[0]`
//! is where the path starts; for a radial array `points[1]` is the centre the
//! copies go round. A placement is relative to the object too (see
//! `ArrayPlacement`), so moving, turning or resizing the object carries every
//! copy with it and never needs a new layout.
//!
//! The path normally starts on the object (`points[0]` zero). When it does not,
//! the object sits off the path by `-points[0]`, and every copy sits off it the
//! same way in its own frame: turned with the path where copies follow it, scaled
//! with the copy's size. That is what posing one copy with the gizmo edits: a
//! row of links all lifted off, or pushed out from, the curve they hang on.
//!
//! Rotations are glam's, which turn counter-clockwise looking back along the
//! axis from its positive end (the right-hand rule). The numbers are exactly the
//! Unity app's; see the crate docs for what that means for the picture.

use glam::{Quat, Vec3};

use crate::path::{self, direction};
use crate::types::{ArrayKind, ArrayPlacement, ArraySettings};

/// Size of the copy `t` of the way along the run (0 = the object, 1 = the last
/// copy): the taper from 1 to `end_scale`, times the swell of `mid_scale` that
/// peaks half way and fades to nothing at both ends. Never below a fifth of the
/// smallest size a setting can ask for, so a copy cannot collapse to a point.
pub fn scale_at(settings: &ArraySettings, t: f32) -> f32 {
    let taper = 1.0 + (settings.end_scale - 1.0) * t;
    let swell = 1.0 + (settings.mid_scale - 1.0) * (std::f32::consts::PI * t.clamp(0.0, 1.0)).sin();
    (ArraySettings::MIN_SCALE * 0.2).max(taper * swell)
}

/// The world axis a radial array turns about: 0 = X, 2 = Z, anything else Y.
pub fn axis_vector(axis: i32) -> Vec3 {
    match axis {
        0 => Vec3::X,
        2 => Vec3::Z,
        _ => Vec3::Y,
    }
}

/// One placement per copy, copy 0 being the object, in the order the copies go
/// along the run. Out-of-range settings are clamped, not trusted; kind `None`
/// has no copies. A path with no length stacks every copy on the object rather
/// than producing NaNs.
pub fn compute(raw: &ArraySettings, points: &[Vec3]) -> Vec<ArrayPlacement> {
    if raw.kind == ArrayKind::None {
        return Vec::new();
    }
    let settings = raw.clamped();
    let count = settings.count as usize;
    let sizes: Vec<f32> = (0..count).map(|i| scale_at(&settings, i as f32 / (count - 1) as f32)).collect();

    let mut placements = if settings.kind == ArrayKind::Radial {
        compute_radial(&settings, points, &sizes)
    } else {
        compute_path(&settings, points, &sizes)
    };
    // Whatever the layout did, the object is where the object is.
    placements[0] = ArrayPlacement::OBJECT;
    placements
}

/// The curve a radial array's start point sweeps out, as `steps + 1` offsets
/// from the object's position: what the tool draws as the ring.
pub fn radial_arc(settings: &ArraySettings, points: &[Vec3], steps: usize) -> Vec<Vec3> {
    let settings = settings.clamped();
    let (origin, centre) = radial_frame(points);
    let axis = axis_vector(settings.axis);
    (0..=steps)
        .map(|i| {
            let turn = Quat::from_axis_angle(axis, (settings.sweep * i as f32 / steps.max(1) as f32).to_radians());
            origin + centre - turn * centre
        })
        .collect()
}

// -------------------------------------------------------------------------- radial

/// The path's start, and the centre relative to it.
fn radial_frame(points: &[Vec3]) -> (Vec3, Vec3) {
    let origin = points.first().copied().unwrap_or(Vec3::ZERO);
    let centre = points.get(1).map_or(Vec3::ZERO, |&c| c - origin);
    (origin, centre)
}

fn compute_radial(settings: &ArraySettings, points: &[Vec3], sizes: &[f32]) -> Vec<ArrayPlacement> {
    let count = sizes.len();
    let (origin, centre) = radial_frame(points);
    let axis = axis_vector(settings.axis);
    // How far the object sits off the path's start: see the module remarks.
    let shift = -origin;
    // Negative goes the other way round (see `ArraySettings::sweep`).
    let direction = if settings.sweep < 0.0 { -1.0 } else { 1.0 };
    let size = settings.sweep.abs();
    let step = direction * if size >= 359.99 { 360.0 / count as f32 } else { size / (count - 1) as f32 };

    (0..count)
        .map(|i| {
            let turn = Quat::from_axis_angle(axis, (step * i as f32).to_radians());
            let rotation = if settings.follow { turn } else { Quat::IDENTITY };
            // The path's start swung about the axis through the centre.
            let mut offset = centre - turn * centre;
            if shift != Vec3::ZERO {
                offset += rotation * (shift * sizes[i]) - shift;
            }
            ArrayPlacement { offset, rotation, scale: sizes[i] }
        })
        .collect()
}

// ---------------------------------------------------------------------------- path

fn compute_path(settings: &ArraySettings, points: &[Vec3], sizes: &[f32]) -> Vec<ArrayPlacement> {
    let count = sizes.len();
    let closed = settings.kind == ArrayKind::Curve && settings.closed;
    let looped = path::is_loop(points.len(), closed);
    let sampled = path::sample(points, closed);
    let length = sampled.length();

    if length < 1e-9 {
        return sizes.iter().map(|&scale| ArrayPlacement { scale, ..ArrayPlacement::OBJECT }).collect();
    }

    let dense = &sampled.points;
    let along = distribute_along(sizes, looped, settings.space_by_size);
    let (frames, twist) = build_frames(dense, looped);
    let start_tangent = direction(dense[0], dense[1]);
    let origin = dense[0];
    // How far the object sits off the path's start: see the module remarks.
    let shift = -points[0];

    (0..count)
        .map(|i| {
            let s = along[i] * length;
            let (k, f) = path::locate(&sampled.arc, s);
            let mut rotation = Quat::IDENTITY;
            if settings.follow {
                rotation = nlerp(frames[k], frames[k + 1], f);
                // A loop's transported frame comes back turned about the tangent by
                // `twist`; spread the correction round the loop so no one gap
                // carries all of it.
                if looped && twist.abs() > 1e-5 {
                    rotation *= Quat::from_axis_angle(start_tangent, -twist * (s / length));
                }
            }
            let mut offset = dense[k] + (dense[k + 1] - dense[k]) * f - origin;
            if shift != Vec3::ZERO {
                offset += rotation * (shift * sizes[i]) - shift;
            }
            ArrayPlacement { offset, rotation, scale: sizes[i] }
        })
        .collect()
}

/// Each copy's place on the path as a fraction of its length. An open path runs
/// from its start to its end; a loop leaves one more gap, from the last copy
/// back round to the first. With `by_size` a gap is as wide as the two copies
/// either side of it are big.
fn distribute_along(sizes: &[f32], looped: bool, by_size: bool) -> Vec<f32> {
    let gap = |a: f32, b: f32| if by_size { 0.5 * (a + b) } else { 1.0 };
    let mut along = Vec::with_capacity(sizes.len());
    let mut total = 0.0;
    along.push(0.0);
    for pair in sizes.windows(2) {
        total += gap(pair[0], pair[1]);
        along.push(total);
    }
    if looped {
        total += gap(sizes[sizes.len() - 1], sizes[0]);
    }
    if total < 1e-9 {
        total = 1.0;
    }
    along.iter_mut().for_each(|a| *a /= total);
    along
}

/// One rotation per polyline point: how a frame carried along the path from its
/// start, turning only as much as the path does (parallel transport), has turned
/// by then. So copies follow the curve without rolling about it. Also returns,
/// for a loop, the angle in radians about the tangent that the frame is off by
/// when it gets back to the start.
fn build_frames(dense: &[Vec3], looped: bool) -> (Vec<Quat>, f32) {
    let n = dense.len();
    let first = vertex_tangent(dense, 0, looped);
    let mut previous = first;
    let mut frame = Quat::IDENTITY;
    let mut frames = Vec::with_capacity(n);
    frames.push(frame);
    for k in 1..n {
        let tangent = vertex_tangent(dense, k, looped);
        // Renormalised each step: hundreds of products would otherwise drift.
        frame = (from_to(previous, tangent) * frame).normalize();
        frames.push(frame);
        previous = tangent;
    }
    if !looped {
        return (frames, 0.0);
    }

    let side = perpendicular(first);
    let carried = frames[n - 1] * side;
    let twist = side.cross(carried).dot(first).atan2(side.dot(carried));
    (frames, twist)
}

/// Central-difference tangent at polyline point `k` (one-sided at an open end;
/// wrapping on a loop, whose last point is its first again).
fn vertex_tangent(dense: &[Vec3], k: usize, looped: bool) -> Vec3 {
    let n = dense.len();
    let (before, after) = if looped {
        (if k == 0 { n - 2 } else { k - 1 }, if k + 1 > n - 1 { 1 } else { k + 1 })
    } else {
        (k.saturating_sub(1), (k + 1).min(n - 1))
    };
    let t = direction(dense[before], dense[after]);
    if t.length_squared() > 0.5 {
        return t;
    }
    // Doubled-back or coincident neighbours: fall back on either side alone.
    let t = direction(dense[k], dense[(k + 1).min(n - 1)]);
    if t.length_squared() > 0.5 {
        return t;
    }
    let t = direction(dense[k.saturating_sub(1)], dense[k]);
    if t.length_squared() > 0.5 { t } else { Vec3::Z }
}

// ----------------------------------------------------------------------- rotations
//
// Written out rather than taken from glam's `from_rotation_arc` and `lerp` so the
// degenerate cases (opposite vectors, a nearly zero blend) resolve exactly as in
// the Unity app, whose tests the maths here is checked against.

/// The shortest rotation taking unit vector `a` onto unit vector `b`.
pub fn from_to(a: Vec3, b: Vec3) -> Quat {
    let dot = a.dot(b);
    if dot > 0.999_999_9 {
        return Quat::IDENTITY;
    }
    if dot < -0.999_999 {
        return Quat::from_axis_angle(perpendicular(a), std::f32::consts::PI);
    }
    let c = a.cross(b);
    normalized(Quat::from_xyzw(c.x, c.y, c.z, 1.0 + dot))
}

fn perpendicular(v: Vec3) -> Vec3 {
    let other = if v.x.abs() < 0.7 { Vec3::X } else { Vec3::Y };
    let p = v.cross(other);
    let len = p.length();
    if len > 1e-9 { p / len } else { Vec3::Y }
}

fn nlerp(a: Quat, b: Quat, t: f32) -> Quat {
    let sign = if a.dot(b) < 0.0 { -1.0 } else { 1.0 };
    normalized(a * (1.0 - t) + b * (t * sign))
}

fn normalized(q: Quat) -> Quat {
    let len = q.length();
    if len > 1e-12 { q / len } else { Quat::IDENTITY }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_to_carries_one_direction_onto_another() {
        let diagonal = Vec3::new(1.0, 1.0, 1.0).normalize();
        let nearly_opposite = Vec3::new(-1.0, 0.0005, 0.0).normalize();
        for (a, b) in [
            (Vec3::X, Vec3::Y),
            (Vec3::X, Vec3::X),
            (Vec3::X, -Vec3::X),
            (Vec3::Y, -Vec3::Y),
            (Vec3::Z, diagonal),
            (Vec3::X, nearly_opposite),
            (diagonal, -diagonal),
        ] {
            let q = from_to(a, b);
            assert!((q.length() - 1.0).abs() < 1e-5, "{a} to {b} is not a unit rotation");
            assert!((q * a - b).length() < 2e-3, "{a} to {b}: got {}", q * a);
        }
    }

    #[test]
    fn blending_two_rotations_takes_the_short_way_round() {
        let q = Quat::from_axis_angle(Vec3::Y, 0.4);
        let r = Quat::from_axis_angle(Vec3::Y, 0.8);
        let direct = nlerp(q, r, 0.5);
        let via_negative = nlerp(q, -r, 0.5);
        assert!(direct.dot(via_negative).abs() > 0.999_999, "-r is the same rotation as r");
        assert!(direct.dot(via_negative) > 0.0, "and the blend does not pass through the far side");
        assert!((direct.dot(Quat::from_axis_angle(Vec3::Y, 0.6))).abs() > 0.9999);
    }

    #[test]
    fn axes_are_x_y_z_and_anything_else_is_y() {
        assert_eq!(axis_vector(0), Vec3::X);
        assert_eq!(axis_vector(1), Vec3::Y);
        assert_eq!(axis_vector(2), Vec3::Z);
        assert_eq!(axis_vector(-4), Vec3::Y);
        assert_eq!(axis_vector(7), Vec3::Y);
    }
}
