//! Thinning a hand-drawn stroke to a few points: Ramer-Douglas-Peucker.
//!
//! A pen stroke arrives as hundreds of samples, far too many to reshape by
//! hand. This keeps the first and last and, between them, every sample that
//! sticks out of the straight line through its neighbours by more than a
//! tolerance, so what is left is the corners of the stroke. The curve through
//! those points (see `path`) follows the stroke closely.
//!
//! It is the same rule as the tube tool's `TubeCurve.Simplify` in the Unity
//! app, and private to this crate for the same reason the path maths is: the
//! array tool is its only user.

use glam::Vec3;

/// The indices, in order, of the points of `points` worth keeping at
/// `tolerance`: the ends always, and any point further than `tolerance` from
/// the line between the kept points either side of it.
///
/// Worked with an explicit stack, not recursion, so a long stroke cannot
/// overflow it.
pub fn simplify(points: &[Vec3], tolerance: f32) -> Vec<usize> {
    let n = points.len();
    if n < 2 {
        return (0..n).collect();
    }
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;

    let mut spans = vec![(0, n - 1)];
    while let Some((first, last)) = spans.pop() {
        if last <= first + 1 {
            continue;
        }
        let mut worst = None;
        let mut worst_distance = tolerance;
        for i in first + 1..last {
            let d = distance_to_segment(points[i], points[first], points[last]);
            if d > worst_distance {
                worst_distance = d;
                worst = Some(i);
            }
        }
        if let Some(i) = worst {
            keep[i] = true;
            spans.push((first, i));
            spans.push((i, last));
        }
    }
    (0..n).filter(|&i| keep[i]).collect()
}

/// How far `p` is from the segment `a` to `b` (an end point, not the line's
/// extension, when `p` is past it).
pub fn distance_to_segment(p: Vec3, a: Vec3, b: Vec3) -> f32 {
    let ab = b - a;
    let len2 = ab.length_squared();
    let t = if len2 > 1e-12 { ((p - a).dot(ab) / len2).clamp(0.0, 1.0) } else { 0.0 };
    (a + ab * t - p).length()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f32, y: f32) -> Vec3 {
        Vec3::new(x, y, 0.0)
    }

    #[test]
    fn a_straight_run_keeps_only_its_ends() {
        let line: Vec<Vec3> = (0..50).map(|i| v(i as f32, 2.0 * i as f32)).collect();
        assert_eq!(simplify(&line, 0.01), vec![0, 49]);
    }

    #[test]
    fn a_corner_is_kept_and_wobble_below_the_tolerance_is_not() {
        // Out along x with a little noise, then up along y.
        let mut path: Vec<Vec3> = (0..=10).map(|i| v(i as f32, if i % 2 == 0 { 0.0 } else { 0.05 })).collect();
        path.extend((1..=10).map(|i| v(10.0, i as f32)));
        let kept = simplify(&path, 0.2);
        assert_eq!(kept, vec![0, 10, 20], "the two ends and the corner");
        // Tighten the tolerance below the wobble and the wobble comes back.
        assert!(simplify(&path, 0.01).len() > 3);
    }

    #[test]
    fn a_tolerance_larger_than_the_stroke_leaves_the_ends() {
        let arc: Vec<Vec3> = (0..=20).map(|i| (i as f32 * 0.15).sin() * Vec3::Y + Vec3::X * i as f32).collect();
        assert_eq!(simplify(&arc, 100.0), vec![0, 20]);
    }

    #[test]
    fn every_dropped_point_is_within_tolerance_of_the_polyline_through_the_kept_ones() {
        let tolerance = 0.3;
        let curve: Vec<Vec3> = (0..200)
            .map(|i| {
                let t = i as f32 * 0.05;
                Vec3::new(t * 3.0, (t * 1.3).sin() * 2.0, (t * 0.7).cos())
            })
            .collect();
        let kept = simplify(&curve, tolerance);
        assert!(kept.len() > 4 && kept.len() < 100, "{} kept", kept.len());
        assert!(kept.windows(2).all(|w| w[0] < w[1]), "in order, no repeats");
        for pair in kept.windows(2) {
            for i in pair[0]..=pair[1] {
                let d = distance_to_segment(curve[i], curve[pair[0]], curve[pair[1]]);
                assert!(d <= tolerance + 1e-5, "point {i} is {d} from its span");
            }
        }
    }

    #[test]
    fn short_inputs_come_back_as_they_are() {
        assert!(simplify(&[], 1.0).is_empty());
        assert_eq!(simplify(&[Vec3::ONE], 1.0), vec![0]);
        assert_eq!(simplify(&[Vec3::ONE, Vec3::ZERO], 1.0), vec![0, 1]);
        // A stroke that came back to where it began has coincident ends.
        let loop_: Vec<Vec3> = [v(0.0, 0.0), v(2.0, 0.0), v(2.0, 2.0), v(0.0, 0.0)].to_vec();
        assert_eq!(simplify(&loop_, 0.1), vec![0, 1, 2, 3]);
    }

    #[test]
    fn a_hundred_thousand_samples_do_not_overflow_anything() {
        let long: Vec<Vec3> = (0..100_000).map(|i| v(i as f32 * 0.01, (i as f32 * 0.002).sin())).collect();
        let kept = simplify(&long, 0.01);
        assert!(kept.len() > 10 && kept.len() < long.len());
    }

    #[test]
    fn distance_stops_at_the_ends_of_the_segment() {
        assert!((distance_to_segment(v(5.0, 1.0), v(0.0, 0.0), v(2.0, 0.0)) - (9.0f32 + 1.0).sqrt()).abs() < 1e-6);
        assert!((distance_to_segment(v(1.0, 3.0), v(0.0, 0.0), v(2.0, 0.0)) - 3.0).abs() < 1e-6);
        assert!(
            (distance_to_segment(v(1.0, 1.0), v(0.0, 0.0), v(0.0, 0.0)) - 2.0f32.sqrt()).abs() < 1e-6,
            "a zero-length segment is a point"
        );
    }
}
