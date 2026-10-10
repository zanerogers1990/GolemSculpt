//! The curve an array's copies sit on: a smooth path through its points, open
//! or closed.
//!
//! The same family as the tube and lathe curves (cubic Hermite, tangent
//! direction from the bisector of the neighbouring chords, length from the
//! segment's own chord), so a path made from a simplified hand-drawn stroke,
//! whose points are unevenly spaced, never loops or cusps. What those do not
//! have is a closed form: here the last point can run on into the first, and
//! then every point is an interior one.
//!
//! Two points make a straight line, which is all a line array is.

use glam::Vec3;

/// Polyline points per control segment when a path is sampled.
pub const STEPS_PER_SEGMENT: usize = 24;

/// A closed path needs three points to enclose anything; with two it is just
/// the line there and back.
pub fn is_loop(point_count: usize, closed: bool) -> bool {
    closed && point_count > 2
}

pub fn segment_count(point_count: usize, closed: bool) -> usize {
    if point_count < 2 {
        0
    } else if is_loop(point_count, closed) {
        point_count
    } else {
        point_count - 1
    }
}

/// Position on segment `segment` (point `segment` to the next one, wrapping on a
/// loop) at `t`. A segment past the end counts as the last one.
pub fn evaluate(points: &[Vec3], closed: bool, segment: usize, t: f32) -> Vec3 {
    let n = points.len();
    let segments = segment_count(n, closed);
    if segments == 0 {
        return points.first().copied().unwrap_or(Vec3::ZERO);
    }
    let looped = is_loop(n, closed);
    let i0 = segment.min(segments - 1);
    let i1 = if looped { (i0 + 1) % n } else { i0 + 1 };
    let (p0, p1) = (points[i0], points[i1]);
    let chord = p1 - p0;
    let length = chord.length();
    if length < 1e-9 {
        return p0;
    }
    let chord = chord / length;

    let m0 = tangent(points, looped, i0, chord, true) * length;
    let m1 = tangent(points, looped, i1, chord, false) * length;

    let t2 = t * t;
    let t3 = t2 * t;
    p0 * (2.0 * t3 - 3.0 * t2 + 1.0) + m0 * (t3 - 2.0 * t2 + t) + p1 * (-2.0 * t3 + 3.0 * t2) + m1 * (t3 - t2)
}

/// Unit tangent at point `i`, for the segment whose direction is `chord`.
/// `at_start` says whether `i` is that segment's first point, which only matters
/// at an open end.
fn tangent(points: &[Vec3], looped: bool, i: usize, chord: Vec3, at_start: bool) -> Vec3 {
    let n = points.len();
    if n < 3 {
        return chord;
    }
    if looped || (i > 0 && i < n - 1) {
        return bisector(points, (i + n - 1) % n, i, (i + 1) % n, chord);
    }

    // A free end mirrors its neighbour's tangent across the end chord, so the
    // last span is a symmetric arc rather than one that straightens out as it
    // arrives.
    let neighbour = if at_start { i + 1 } else { i - 1 };
    let inner = bisector(points, neighbour - 1, neighbour, neighbour + 1, chord);
    let reflected = inner.project_onto_normalized(chord) * 2.0 - inner;
    let len = reflected.length();
    if len > 1e-6 { reflected / len } else { chord }
}

fn bisector(points: &[Vec3], before: usize, at: usize, after: usize, fallback: Vec3) -> Vec3 {
    let sum = direction(points[before], points[at]) + direction(points[at], points[after]);
    let len = sum.length();
    if len > 1e-6 { sum / len } else { fallback }
}

/// Unit vector from `from` to `to`; zero when they coincide.
pub(crate) fn direction(from: Vec3, to: Vec3) -> Vec3 {
    let d = to - from;
    let len = d.length();
    if len > 1e-9 { d / len } else { Vec3::ZERO }
}

/// A path as a dense polyline, with how far along it each point is.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SampledPath {
    pub points: Vec<Vec3>,
    /// Arc length from the start to each polyline point.
    pub arc: Vec<f32>,
    /// The control segment each polyline point lies on.
    pub segment_of_point: Vec<usize>,
}

impl SampledPath {
    pub fn length(&self) -> f32 {
        self.arc.last().copied().unwrap_or(0.0)
    }
}

/// The whole path as a dense polyline. A loop ends back on its first point.
pub fn sample(points: &[Vec3], closed: bool) -> SampledPath {
    let mut out = SampledPath::default();
    sample_into(points, closed, &mut out);
    out
}

/// `sample`, reusing `out`'s buffers: the tool redraws the path every frame.
pub fn sample_into(points: &[Vec3], closed: bool, out: &mut SampledPath) {
    out.points.clear();
    out.arc.clear();
    out.segment_of_point.clear();
    let Some(&first) = points.first() else { return };

    out.points.push(first);
    out.arc.push(0.0);
    out.segment_of_point.push(0);

    let mut length = 0.0;
    let mut last = first;
    for segment in 0..segment_count(points.len(), closed) {
        for step in 1..=STEPS_PER_SEGMENT {
            let p = evaluate(points, closed, segment, step as f32 / STEPS_PER_SEGMENT as f32);
            length += (p - last).length();
            last = p;
            out.points.push(p);
            out.arc.push(length);
            out.segment_of_point.push(segment);
        }
    }
}

/// The polyline span holding arc length `s`: the index of its first point, and
/// how far along the span `s` lies (0 to 1). `s` is clamped to the path.
pub fn locate(arc: &[f32], s: f32) -> (usize, f32) {
    let Some(last) = arc.len().checked_sub(1).filter(|&l| l >= 1) else { return (0, 0.0) };
    if s <= 0.0 {
        return (0, 0.0);
    }
    if s >= arc[last] {
        return (last - 1, 1.0);
    }

    let (mut lo, mut hi) = (0, last);
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if arc[mid] <= s {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let span = arc[lo + 1] - arc[lo];
    (lo, if span > 1e-12 { (s - arc[lo]) / span } else { 0.0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wave() -> Vec<Vec3> {
        vec![
            Vec3::ZERO,
            Vec3::new(2.0, 1.5, 0.0),
            Vec3::new(4.0, 0.0, 1.0),
            Vec3::new(6.0, -1.0, 0.0),
            Vec3::new(8.0, 0.5, -1.0),
        ]
    }

    #[test]
    fn two_points_make_a_straight_line() {
        let pts = [Vec3::ZERO, Vec3::new(3.0, 4.0, 0.0)];
        for step in 0..=4 {
            let t = step as f32 / 4.0;
            assert!((evaluate(&pts, false, 0, t) - pts[1] * t).length() < 1e-5);
        }
        assert!((sample(&pts, false).length() - 5.0).abs() < 1e-4);
    }

    #[test]
    fn the_path_goes_through_every_control_point() {
        let pts = wave();
        for i in 0..pts.len() - 1 {
            assert!((evaluate(&pts, false, i, 0.0) - pts[i]).length() < 1e-5, "start of segment {i}");
            assert!((evaluate(&pts, false, i, 1.0) - pts[i + 1]).length() < 1e-5, "end of segment {i}");
        }
    }

    #[test]
    fn a_loop_comes_back_to_where_it_started() {
        let ring = [Vec3::ZERO, Vec3::new(4.0, 0.0, 0.0), Vec3::new(4.0, 0.0, 4.0), Vec3::new(0.0, 0.0, 4.0)];
        assert_eq!(segment_count(4, true), 4);
        assert_eq!(segment_count(4, false), 3);
        let s = sample(&ring, true);
        assert_eq!(s.points.len(), 4 * STEPS_PER_SEGMENT + 1);
        assert!((s.points[s.points.len() - 1] - s.points[0]).length() < 1e-5);
        // Closing two points is not a loop: it stays the single line.
        assert!(!is_loop(2, true));
        assert_eq!(segment_count(2, true), 1);
    }

    #[test]
    fn locate_finds_the_span_and_clamps_to_the_path() {
        let arc = [0.0, 1.0, 3.0, 4.0];
        assert_eq!(locate(&arc, -1.0), (0, 0.0));
        assert_eq!(locate(&arc, 0.0), (0, 0.0));
        assert_eq!(locate(&arc, 2.0), (1, 0.5));
        assert_eq!(locate(&arc, 4.0), (2, 1.0));
        assert_eq!(locate(&arc, 9.0), (2, 1.0));
        assert_eq!(locate(&[0.0], 1.0), (0, 0.0));
        assert_eq!(locate(&[], 1.0), (0, 0.0));
    }

    #[test]
    fn sampling_tracks_arc_length_and_segments() {
        let pts = wave();
        let s = sample(&pts, false);
        assert_eq!(s.points.len(), 4 * STEPS_PER_SEGMENT + 1);
        assert!(s.arc.windows(2).all(|w| w[1] >= w[0]));
        assert_eq!(s.segment_of_point[0], 0);
        assert_eq!(s.segment_of_point[STEPS_PER_SEGMENT], 0);
        assert_eq!(s.segment_of_point[STEPS_PER_SEGMENT + 1], 1);
        assert_eq!(*s.segment_of_point.last().unwrap(), 3);
        let l = s.length();
        assert!(l > 8.0 && l < 14.0, "plausible length {l}");

        let mut reused = SampledPath::default();
        sample_into(&pts, false, &mut reused);
        sample_into(&pts[..3], false, &mut reused);
        assert_eq!(reused, sample(&pts[..3], false));
    }

    #[test]
    fn short_inputs_do_not_panic() {
        assert_eq!(sample(&[], false), SampledPath::default());
        let one = sample(&[Vec3::ONE], false);
        assert_eq!((one.points.len(), one.length()), (1, 0.0));
        assert_eq!(evaluate(&[], false, 0, 0.5), Vec3::ZERO);
        assert_eq!(evaluate(&[Vec3::ONE], false, 3, 0.5), Vec3::ONE);
        // A segment index past the end is the last segment.
        let pts = wave();
        assert!((evaluate(&pts, false, 99, 1.0) - pts[4]).length() < 1e-5);
    }
}
