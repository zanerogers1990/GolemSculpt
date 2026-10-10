//! What a new array starts with: the defaults the Line and Circle buttons and
//! the end of a Draw Curve stroke work out. Ported from the Unity controller
//! (`MakeLine`, `MakeCircle`, `FinishDraw`), as pure functions of the object's
//! size, the camera's axes and the stroke, so the numbers can be tested
//! without a scene.
//!
//! Every one of them keeps what the object's array already had (count, sizes,
//! whether copies turn) when it is only changing shape, and falls back on the
//! defaults otherwise. A path that has been posed off the object keeps its
//! offset (`points[0]`), so a new shape does not undo the posing.

use glam::Vec3;

use crate::layout::axis_vector;
use crate::simplify::simplify;
use crate::state::ArrayState;
use crate::types::{ArrayKind, ArraySettings};

/// Gap left between neighbouring copies, as a multiple of the object's size.
const BREATHING_ROOM: f32 = 1.1;
/// A drawn path keeps at most this many points: few enough to reshape by hand.
pub const MAX_STROKE_POINTS: usize = 24;
/// Copies a fresh drawn curve gets, however long or short the stroke.
const DRAWN_COUNT: (i32, i32) = (3, 40);

/// The world axis (signed) that `direction` points most nearly along, leaving
/// out axis `skip`. A tie goes to the lower axis.
pub fn nearest_axis(direction: Vec3, skip: Option<usize>) -> Vec3 {
    let mut best = None;
    let mut best_dot = -1.0;
    for axis in (0..3).filter(|&a| Some(a) != skip) {
        let d = direction[axis].abs();
        if d > best_dot {
            best_dot = d;
            best = Some(axis);
        }
    }
    let axis = best.unwrap_or(1);
    let unit = axis_vector(axis as i32);
    if direction[axis] < 0.0 { -unit } else { unit }
}

/// 0, 1 or 2 for a world axis vector, as `ArraySettings::axis` stores it.
pub fn axis_index(axis: Vec3) -> i32 {
    if axis.x.abs() > 0.5 {
        0
    } else if axis.z.abs() > 0.5 {
        2
    } else {
        1
    }
}

/// Roughly how big the object is: the mean of the sides of its box.
pub fn object_size(bounds_size: Vec3) -> f32 {
    ((bounds_size.x + bounds_size.y + bounds_size.z) / 3.0).max(1e-4)
}

/// Where the object's path starts relative to the object: zero unless its
/// copies have been moved off the path.
pub fn path_start(existing: &ArrayState) -> Vec3 {
    if existing.is_none() { Vec3::ZERO } else { existing.points.first().copied().unwrap_or(Vec3::ZERO) }
}

/// The settings a new array of `kind` takes: the existing array's when there
/// is one, else the defaults.
fn settings_for(existing: &ArrayState, kind: ArrayKind) -> ArraySettings {
    let mut settings = if existing.is_none() { ArraySettings::new(kind) } else { existing.settings };
    settings.kind = kind;
    settings.closed = false;
    settings
}

/// A straight row of copies, along the world axis that runs most nearly
/// left to right on screen (`camera_right`), spaced so they sit side by side.
/// `size` is the object's world box.
pub fn line(existing: &ArrayState, size: Vec3, camera_right: Vec3) -> ArrayState {
    let settings = settings_for(existing, ArrayKind::Line);
    let direction = nearest_axis(camera_right, None);
    let width = size.dot(direction.abs()).abs();
    let step = width.max(object_size(size) * 0.25) * BREATHING_ROOM;
    let start = path_start(existing);
    ArrayState { settings, points: vec![start, start + direction * (step * (settings.count - 1) as f32)] }
}

/// A ring of copies round a centre beside the object, facing the view, sized
/// so they sit side by side. The axis is the world axis the camera looks most
/// nearly along; the centre goes to the left on screen, so the object starts
/// at the ring's right and the copies run counter-clockwise from it.
pub fn circle(existing: &ArrayState, size: Vec3, camera_right: Vec3, camera_forward: Vec3) -> ArrayState {
    let mut settings = settings_for(existing, ArrayKind::Radial);
    let axis = nearest_axis(camera_forward, None);
    settings.axis = axis_index(axis);
    let to_centre = -nearest_axis(camera_right, Some(settings.axis as usize));
    let size = object_size(size);
    let radius = (settings.count as f32 * size * BREATHING_ROOM / std::f32::consts::TAU).max(size * 1.2);
    let start = path_start(existing);
    ArrayState { settings, points: vec![start, start + to_centre * radius] }
}

/// The array a finished Draw Curve stroke makes. `stroke` is the pen's samples
/// on the plane facing the camera through the object, in world space; the path
/// is thinned to a handful of points, moved to start on the object, and closed
/// into a loop if `closed` (the stroke ended where it began). `None` when the
/// stroke has too few samples or no length.
///
/// A fresh array gets about as many copies as fit along the stroke without
/// overlapping; one that already exists keeps its count and settings.
pub fn curve_from_stroke(existing: &ArrayState, stroke: &[Vec3], closed: bool, size: Vec3) -> Option<ArrayState> {
    if stroke.len() < 3 {
        return None;
    }
    let length: f32 = stroke.windows(2).map(|w| (w[1] - w[0]).length()).sum();
    if length < 1e-9 {
        return None;
    }

    let mut tolerance = length * 0.02;
    let mut kept = simplify(stroke, tolerance);
    while kept.len() > MAX_STROKE_POINTS {
        tolerance *= 1.4;
        kept = simplify(stroke, tolerance);
    }

    // The path starts on the object (or wherever the one it replaces started).
    let shift = stroke[0] - path_start(existing);
    let mut points: Vec<Vec3> = kept.iter().map(|&i| stroke[i] - shift).collect();
    // A loop's last point is its first again; the path closes onto it by itself.
    if closed && points.len() > 3 {
        points.pop();
    }
    let closed = closed && points.len() >= 3;

    let mut settings = if existing.is_none() { ArraySettings::new(ArrayKind::Curve) } else { existing.settings };
    if existing.is_none() {
        let fit = (length / (object_size(size) * BREATHING_ROOM)).round() as i32 + 1;
        settings.count = fit.clamp(DRAWN_COUNT.0, DRAWN_COUNT.1);
    }
    settings.kind = ArrayKind::Curve;
    settings.closed = closed;
    Some(ArrayState { settings, points })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::compute;

    /// A unit cube seen from the front: the camera looks down -Z, +X is right.
    const SIZE: Vec3 = Vec3::ONE;
    const RIGHT: Vec3 = Vec3::X;
    const FORWARD: Vec3 = Vec3::NEG_Z;

    #[test]
    fn the_nearest_axis_is_signed_and_can_leave_one_out() {
        assert_eq!(nearest_axis(Vec3::new(0.2, -0.9, 0.3), None), Vec3::NEG_Y);
        assert_eq!(nearest_axis(Vec3::new(0.2, -0.9, 0.3), Some(1)), Vec3::Z);
        assert_eq!(nearest_axis(Vec3::new(-1.0, 1.0, 1.0), None), Vec3::NEG_X, "a tie goes to the lower axis");
        assert_eq!(nearest_axis(Vec3::ZERO, None), Vec3::X);
    }

    #[test]
    fn axes_come_back_as_their_stored_numbers() {
        assert_eq!(
            [Vec3::X, Vec3::NEG_X, Vec3::Y, Vec3::NEG_Y, Vec3::Z, Vec3::NEG_Z].map(axis_index),
            [0, 0, 1, 1, 2, 2]
        );
    }

    #[test]
    fn a_line_runs_along_the_screens_horizontal_one_object_wide_per_step() {
        let state = line(&ArrayState::none(), SIZE, RIGHT);
        assert_eq!(state.settings.kind, ArrayKind::Line);
        assert_eq!(state.settings.count, 5);
        // One unit wide plus a tenth, four steps to the fifth copy.
        assert_eq!(state.points[0], Vec3::ZERO);
        assert!(state.points[1].distance(Vec3::new(4.4, 0.0, 0.0)) < 1e-5, "{:?}", state.points[1]);

        // Seen from the right instead, the screen's horizontal is -Z.
        let side = line(&ArrayState::none(), Vec3::new(0.5, 1.0, 2.0), Vec3::NEG_Z);
        assert!(side.points[1].distance(Vec3::new(0.0, 0.0, -8.8)) < 1e-4, "{:?}", side.points[1]);
    }

    #[test]
    fn a_thin_object_still_gets_a_quarter_of_its_size_between_copies() {
        let flat = line(&ArrayState::none(), Vec3::new(0.0, 4.0, 4.0), RIGHT);
        // mean size 8/3, a quarter of it, plus a tenth.
        let expected = (8.0 / 3.0) * 0.25 * 1.1 * 4.0;
        assert!((flat.points[1].x - expected).abs() < 1e-4, "{:?}", flat.points[1]);
    }

    #[test]
    fn a_circle_has_its_centre_left_of_the_object_and_the_copies_run_counter_clockwise() {
        let state = circle(&ArrayState::none(), SIZE, RIGHT, FORWARD);
        assert_eq!(state.settings.kind, ArrayKind::Radial);
        assert_eq!((state.settings.count, state.settings.axis), (8, 2));
        // Eight unit copies with a tenth between them, round a circle.
        let radius = 8.0 * 1.1 / std::f32::consts::TAU;
        assert!(state.points[1].distance(Vec3::new(-radius, 0.0, 0.0)) < 1e-5, "{:?}", state.points[1]);

        // The second copy is above the object on screen: counter-clockwise from the front.
        let placements = compute(&state.settings, &state.points);
        assert_eq!(placements.len(), 8);
        assert!(placements[1].offset.y > 0.0, "{:?}", placements[1].offset);
        // A quarter turn on is as far up as the centre is left of the object.
        assert!((placements[2].offset - Vec3::new(-radius, radius, 0.0)).length() < 1e-4, "{:?}", placements[2].offset);
    }

    #[test]
    fn a_small_ring_is_pushed_out_so_the_object_does_not_sit_on_the_centre() {
        let mut few = ArrayState::none();
        few.settings = ArraySettings::new(ArrayKind::Radial);
        few.settings.count = 2;
        let state = circle(&few, SIZE, RIGHT, FORWARD);
        assert!((state.points[1].length() - 1.2).abs() < 1e-5, "{:?}", state.points[1]);
    }

    #[test]
    fn the_circle_goes_round_whichever_axis_the_camera_looks_along() {
        // From above: looking down -Y, the screen's right is +X.
        let top = circle(&ArrayState::none(), SIZE, Vec3::X, Vec3::NEG_Y);
        assert_eq!(top.settings.axis, 1);
        assert!(top.points[1].x < 0.0 && top.points[1].y == 0.0 && top.points[1].z == 0.0);
        // From the side the centre is on the -Z end when right is +Z.
        let side = circle(&ArrayState::none(), SIZE, Vec3::Z, Vec3::NEG_X);
        assert_eq!(side.settings.axis, 0);
        assert!(side.points[1].z < 0.0);
    }

    #[test]
    fn changing_shape_keeps_what_the_array_already_had() {
        let mut existing = ArrayState { settings: ArraySettings::new(ArrayKind::Curve), points: vec![Vec3::ZERO; 3] };
        existing.settings.count = 11;
        existing.settings.end_scale = 0.4;
        existing.settings.closed = true;
        let ring = circle(&existing, SIZE, RIGHT, FORWARD);
        assert_eq!((ring.settings.count, ring.settings.end_scale, ring.settings.closed), (11, 0.4, false));
        let row = line(&existing, SIZE, RIGHT);
        assert_eq!((row.settings.count, row.settings.end_scale), (11, 0.4));
    }

    #[test]
    fn a_path_posed_off_the_object_keeps_its_offset() {
        let existing = ArrayState {
            settings: ArraySettings::new(ArrayKind::Line),
            points: vec![Vec3::new(0.0, 0.5, 0.0), Vec3::new(3.0, 0.5, 0.0)],
        };
        assert_eq!(path_start(&existing), Vec3::new(0.0, 0.5, 0.0));
        let row = line(&existing, SIZE, RIGHT);
        assert_eq!(row.points[0], Vec3::new(0.0, 0.5, 0.0));
        assert!((row.points[1].y - 0.5).abs() < 1e-6 && row.points[1].x > 1.0);
        assert_eq!(path_start(&ArrayState::none()), Vec3::ZERO);
    }

    fn wave(samples: usize) -> Vec<Vec3> {
        (0..samples)
            .map(|i| {
                let t = i as f32 / (samples - 1) as f32;
                Vec3::new(10.0 + 8.0 * t, 3.0 + 2.0 * (t * std::f32::consts::TAU).sin(), 7.0)
            })
            .collect()
    }

    #[test]
    fn a_drawn_curve_starts_on_the_object_and_has_few_points() {
        let stroke = wave(300);
        let state = curve_from_stroke(&ArrayState::none(), &stroke, false, SIZE).unwrap();
        assert_eq!(state.settings.kind, ArrayKind::Curve);
        assert!(!state.settings.closed);
        assert_eq!(state.points[0], Vec3::ZERO, "the object is where the path starts");
        assert!(state.points.len() >= 4 && state.points.len() <= MAX_STROKE_POINTS, "{}", state.points.len());
        let end = *state.points.last().unwrap();
        assert!(end.distance(stroke[299] - stroke[0]) < 1e-4, "the far end is where the stroke ended");
    }

    #[test]
    fn a_fresh_curve_gets_about_as_many_copies_as_fit_along_it() {
        let stroke = wave(300);
        let length: f32 = stroke.windows(2).map(|w| (w[1] - w[0]).length()).sum();
        let state = curve_from_stroke(&ArrayState::none(), &stroke, false, SIZE).unwrap();
        assert_eq!(state.settings.count, (length / 1.1).round() as i32 + 1);
        // A tiny object on a long stroke is capped; a huge one is floored.
        assert_eq!(
            curve_from_stroke(&ArrayState::none(), &stroke, false, Vec3::splat(0.01)).unwrap().settings.count,
            40
        );
        assert_eq!(
            curve_from_stroke(&ArrayState::none(), &stroke, false, Vec3::splat(100.0)).unwrap().settings.count,
            3
        );
    }

    #[test]
    fn redrawing_keeps_the_count_and_the_offset_of_the_array_it_replaces() {
        let mut existing = ArrayState { settings: ArraySettings::new(ArrayKind::Line), points: vec![Vec3::Y, Vec3::X] };
        existing.settings.count = 9;
        existing.settings.mid_scale = 2.0;
        let state = curve_from_stroke(&existing, &wave(100), false, SIZE).unwrap();
        assert_eq!((state.settings.count, state.settings.mid_scale), (9, 2.0));
        assert_eq!(state.points[0], Vec3::Y, "still posed the way it was");
    }

    #[test]
    fn a_stroke_that_ends_where_it_began_makes_a_loop_without_repeating_its_first_point() {
        let ring: Vec<Vec3> = (0..=120)
            .map(|i| {
                let a = i as f32 / 120.0 * std::f32::consts::TAU;
                Vec3::new(a.cos() * 5.0, a.sin() * 5.0, 0.0)
            })
            .collect();
        let state = curve_from_stroke(&ArrayState::none(), &ring, true, SIZE).unwrap();
        assert!(state.settings.closed);
        assert!(state.points.len() >= 3);
        let first = state.points[0];
        assert!(state.points[1..].iter().all(|p| p.distance(first) > 0.5), "the closing point was dropped");

        // Two points is not a loop, however it was drawn.
        let line_ = [Vec3::ZERO, Vec3::X, Vec3::X * 2.0];
        assert!(!curve_from_stroke(&ArrayState::none(), &line_, true, SIZE).unwrap().settings.closed);
    }

    #[test]
    fn a_stroke_with_nothing_in_it_makes_no_array() {
        assert!(curve_from_stroke(&ArrayState::none(), &[], false, SIZE).is_none());
        assert!(curve_from_stroke(&ArrayState::none(), &[Vec3::ONE, Vec3::ONE], false, SIZE).is_none());
        assert!(curve_from_stroke(&ArrayState::none(), &[Vec3::ONE; 10], false, SIZE).is_none(), "no length");
    }
}
