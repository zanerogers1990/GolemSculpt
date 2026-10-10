//! Plain 2D helpers for editing a profile by hand, taken from the lathe tool's input handling
//! because they are geometry, not interface: turning a drawn stroke into a handful of profile
//! points, and finding where a new point goes on the curve.
//!
//! They work in profile space (radius, height). Anything measured in pixels - picking, snapping to
//! the axis on screen, deciding a stroke has come back to its start - stays with the caller, which
//! hands the results in.

use glam::Vec2;

use crate::curve;
use crate::profile::ProfilePoint;

/// A sketched profile is simplified to at most this many points: a handful is what can be refined
/// by hand afterwards.
pub const SKETCH_MAX_POINTS: usize = 18;

/// Distance from `p` to the segment `a`-`b`, and the position along the segment of the nearest
/// point, in [0, 1].
pub fn distance_to_segment(p: Vec2, a: Vec2, b: Vec2) -> (f32, f32) {
    let ab = b - a;
    let len2 = ab.length_squared();
    let t = if len2 > 1e-6 { ((p - a).dot(ab) / len2).clamp(0.0, 1.0) } else { 0.0 };
    ((a + ab * t - p).length(), t)
}

/// Ramer-Douglas-Peucker: the points of `points` that keep every dropped one within `tolerance` of
/// the simplified line. A loop is split at its farthest pair so both halves have fixed ends; its
/// last point must not repeat its first.
pub fn simplify(points: &[Vec2], tolerance: f32, closed_loop: bool) -> Vec<Vec2> {
    let n = points.len();
    if n < 3 {
        return points.to_vec();
    }
    let mut keep = vec![false; n];
    if !closed_loop {
        keep[0] = true;
        keep[n - 1] = true;
        simplify_range(points, 0, n - 1, tolerance, &mut keep);
    } else {
        let far = (1..n)
            .max_by(|&a, &b| {
                (points[a] - points[0]).length_squared().total_cmp(&(points[b] - points[0]).length_squared())
            })
            .unwrap_or(1);
        keep[0] = true;
        keep[far] = true;
        simplify_range(points, 0, far, tolerance, &mut keep);
        // The closing half, walked as indices far..n then back to 0.
        let tail: Vec<Vec2> = points[far..].iter().chain(std::iter::once(&points[0])).copied().collect();
        let mut tail_keep = vec![false; tail.len()];
        simplify_range(&tail, 0, tail.len() - 1, tolerance, &mut tail_keep);
        for i in 1..tail.len() - 1 {
            keep[far + i] |= tail_keep[i];
        }
    }
    points.iter().zip(&keep).filter(|&(_, &k)| k).map(|(&p, _)| p).collect()
}

fn simplify_range(points: &[Vec2], first: usize, last: usize, tolerance: f32, keep: &mut [bool]) {
    let mut stack = vec![(first, last)];
    while let Some((first, last)) = stack.pop() {
        if last <= first + 1 {
            continue;
        }
        let mut worst = None;
        let mut max_distance = tolerance;
        for i in first + 1..last {
            let (d, _) = distance_to_segment(points[i], points[first], points[last]);
            if d > max_distance {
                max_distance = d;
                worst = Some(i);
            }
        }
        if let Some(i) = worst {
            keep[i] = true;
            stack.push((first, i));
            stack.push((i, last));
        }
    }
}

/// Turns a drawn stroke (signed profile coordinates, x negative on the mirrored side) into profile
/// points: put onto one side of the axis, simplified to a handful of smooth points (the curve
/// through them reproduces the stroke closely), and with the ends passed through `snap_end` if the
/// profile is open - the caller's way of landing an end exactly on the axis when it was drawn to it.
///
/// `closed_loop` is the caller's decision that the stroke came back to where it started; it must
/// already have dropped the duplicate closing point. None when the stroke is too short or too small
/// to make a profile.
pub fn stroke_to_points(
    stroke: &[Vec2],
    closed_loop: bool,
    snap_end: impl Fn(Vec2) -> Vec2,
) -> Option<Vec<ProfilePoint>> {
    if stroke.len() < 3 {
        return None;
    }

    let side = if stroke.iter().map(|p| p.x).sum::<f32>() < 0.0 { -1.0 } else { 1.0 };
    let stroke: Vec<Vec2> = stroke.iter().map(|p| Vec2::new((p.x * side).max(0.0), p.y)).collect();

    let (min, max) = stroke.iter().fold((stroke[0], stroke[0]), |(lo, hi), &p| (lo.min(p), hi.max(p)));
    let extent = (max.x - min.x).max(max.y - min.y);
    if extent < 1e-5 {
        return None;
    }

    let mut tolerance = extent * 0.012;
    let mut simplified = simplify(&stroke, tolerance, closed_loop);
    while simplified.len() > SKETCH_MAX_POINTS {
        tolerance *= 1.4;
        simplified = simplify(&stroke, tolerance, closed_loop);
    }

    if !closed_loop {
        let last = simplified.len() - 1;
        simplified[0] = snap_end(simplified[0]);
        simplified[last] = snap_end(simplified[last]);
    }

    if simplified.len() < if closed_loop { 3 } else { 2 } {
        return None;
    }
    Some(simplified.into_iter().map(|p| ProfilePoint::new(p, false)).collect())
}

/// The index at which a new point should be inserted to land on the curve nearest `position`: the
/// point list index after the nearest control segment. How a click on a closed loop picks where to
/// add to it.
pub fn nearest_insert_index(points: &[ProfilePoint], closed_loop: bool, position: Vec2) -> usize {
    const STEPS: usize = 12;
    let mut best = points.len();
    let mut best_distance = f32::MAX;
    for seg in 0..curve::segment_count(points, closed_loop) {
        let mut previous = points[seg].position;
        for step in 1..=STEPS {
            let current = curve::evaluate(points, closed_loop, seg, step as f32 / STEPS as f32);
            let (d, _) = distance_to_segment(position, previous, current);
            if d < best_distance {
                best_distance = d;
                best = seg + 1;
            }
            previous = current;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f32, y: f32) -> Vec2 {
        Vec2::new(x, y)
    }

    #[test]
    fn distance_to_a_segment_clamps_to_its_ends() {
        let (d, t) = distance_to_segment(v(0.5, 1.0), v(0.0, 0.0), v(1.0, 0.0));
        assert!((d - 1.0).abs() < 1e-6 && (t - 0.5).abs() < 1e-6);
        let (d, t) = distance_to_segment(v(3.0, 0.0), v(0.0, 0.0), v(1.0, 0.0));
        assert!((d - 2.0).abs() < 1e-6 && t == 1.0);
        let (d, t) = distance_to_segment(v(3.0, 4.0), v(0.0, 0.0), v(0.0, 0.0));
        assert!((d - 5.0).abs() < 1e-6 && t == 0.0);
    }

    #[test]
    fn a_straight_stroke_simplifies_to_its_ends() {
        let line: Vec<Vec2> = (0..50).map(|i| v(0.5, i as f32 * 0.1)).collect();
        assert_eq!(simplify(&line, 0.01, false), vec![line[0], line[49]]);
    }

    #[test]
    fn a_circle_simplifies_to_a_polygon_that_stays_within_tolerance() {
        let circle: Vec<Vec2> = (0..200)
            .map(|i| {
                let a = std::f32::consts::TAU * i as f32 / 200.0;
                v(3.0 + a.cos(), a.sin())
            })
            .collect();
        let simple = simplify(&circle, 0.02, true);
        assert!((8..=40).contains(&simple.len()), "{} points", simple.len());
        for p in &circle {
            let d = (0..simple.len())
                .map(|i| distance_to_segment(*p, simple[i], simple[(i + 1) % simple.len()]).0)
                .fold(f32::MAX, f32::min);
            assert!(d <= 0.021, "dropped point {d} from the polygon");
        }
    }

    #[test]
    fn a_stroke_on_the_mirrored_side_lands_on_the_real_one_and_ends_snap() {
        let stroke: Vec<Vec2> = (0..40)
            .map(|i| {
                let a = std::f32::consts::PI * i as f32 / 39.0;
                v(-a.sin(), -a.cos())
            })
            .collect();
        let snap = |p: Vec2| if p.x < 0.05 { v(0.0, p.y) } else { p };
        let points = stroke_to_points(&stroke, false, snap).expect("a half circle makes a profile");
        assert!(points.len() >= 4 && points.len() <= SKETCH_MAX_POINTS);
        assert!(points.iter().all(|p| p.position.x >= 0.0 && !p.sharp));
        assert_eq!(points[0].position.x, 0.0);
        assert_eq!(points[points.len() - 1].position.x, 0.0);
    }

    #[test]
    fn strokes_too_short_or_too_small_make_nothing() {
        assert!(stroke_to_points(&[v(0.0, 0.0), v(1.0, 1.0)], false, |p| p).is_none());
        assert!(stroke_to_points(&[v(1.0, 1.0); 10], false, |p| p).is_none());
    }

    #[test]
    fn a_click_on_a_loop_goes_into_the_nearest_segment() {
        let pts: Vec<ProfilePoint> =
            [(1.0, 0.0), (2.0, 0.0), (2.0, 1.0), (1.0, 1.0)].iter().map(|&(r, h)| ProfilePoint::smooth(r, h)).collect();
        // Beside the segment from point 1 to point 2, on the right-hand wall.
        assert_eq!(nearest_insert_index(&pts, true, v(2.05, 0.5)), 2);
        // Beside the closing segment, from point 3 back to point 0.
        assert_eq!(nearest_insert_index(&pts, true, v(0.95, 0.5)), 4);
    }
}
