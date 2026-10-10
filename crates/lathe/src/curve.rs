//! Turns a profile's control points into a smooth curve, and resamples that curve at an even
//! arc-length spacing for the mesh builder.
//!
//! Cubic Hermite through every point, with each tangent's DIRECTION taken from the bisector of the
//! two neighbouring chords and its LENGTH from the segment's own chord. Two properties follow that
//! plain Catmull-Rom does not have: unevenly spaced points never produce loops or cusps (a short
//! segment next to a long one only gets a short tangent), and the overshoot past a point is bounded
//! by the chord, so a curve that hugs the axis cannot swing far across it.
//!
//! The curve is split into SPANS at every sharp point (and at the ends of an open profile). Each
//! span is resampled on its own with a spacing adjusted to divide it exactly, so a corner always
//! lands on a sample - otherwise the resampler would round it off, and a flat base would come out
//! bevelled.

use glam::Vec2;

use crate::profile::ProfilePoint;

/// Evaluations per control segment for arc-length measurement. Resampling reads positions off this
/// polyline, so it has to be fine enough that the chords are indistinguishable from the curve at
/// any spacing the mesh could ask for.
const DENSE_STEPS_PER_SEGMENT: usize = 48;

/// One resampled point of the curve.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub position: Vec2,
    /// Sits on a sharp control point or an open end, where the curve's direction jumps.
    pub corner: bool,
}

// ------------------------------------------------------------------------- evaluation

/// How many control segments the curve has: one fewer than the points, or as many as the points
/// for a loop. 0 below two points.
pub fn segment_count(points: &[ProfilePoint], closed_loop: bool) -> usize {
    let n = points.len();
    if n < 2 {
        0
    } else if closed_loop {
        n
    } else {
        n - 1
    }
}

/// A point on control segment `segment` (from point `segment` to point `segment + 1`, wrapping for
/// a loop) at parameter `t` in [0, 1].
pub fn evaluate(points: &[ProfilePoint], closed_loop: bool, segment: usize, t: f32) -> Vec2 {
    Hermite::new(points, closed_loop, segment).at(t)
}

/// Chord length of the control polygon - a cheap upper-ish estimate of the curve's length, used to
/// cap the sample count before resampling.
pub fn control_polygon_length(points: &[ProfilePoint], closed_loop: bool) -> f32 {
    let n = points.len();
    let mut sum: f32 = points.windows(2).map(|w| (w[1].position - w[0].position).length()).sum();
    if closed_loop && n > 2 {
        sum += (points[0].position - points[n - 1].position).length();
    }
    sum
}

/// One control segment as a cubic Hermite, its tangents worked out once so the dense loop in
/// [`ResampleScratch::resample`] does not redo them at every step.
struct Hermite {
    p0: Vec2,
    p1: Vec2,
    m0: Vec2,
    m1: Vec2,
    degenerate: bool,
}

impl Hermite {
    fn new(points: &[ProfilePoint], closed_loop: bool, segment: usize) -> Hermite {
        let n = points.len();
        let a = segment;
        let b = if closed_loop { (segment + 1) % n } else { segment + 1 };

        let p0 = points[a].position;
        let p1 = points[b].position;
        let chord = (p1 - p0).length();
        if chord < 1e-9 {
            return Hermite { p0, p1, m0: Vec2::ZERO, m1: Vec2::ZERO, degenerate: true };
        }
        Hermite {
            p0,
            p1,
            m0: outgoing_tangent(points, closed_loop, a) * chord,
            m1: incoming_tangent(points, closed_loop, b) * chord,
            degenerate: false,
        }
    }

    fn at(&self, t: f32) -> Vec2 {
        if self.degenerate {
            return self.p0;
        }
        let t2 = t * t;
        let t3 = t2 * t;
        (2.0 * t3 - 3.0 * t2 + 1.0) * self.p0
            + (t3 - 2.0 * t2 + t) * self.m0
            + (-2.0 * t3 + 3.0 * t2) * self.p1
            + (t3 - t2) * self.m1
    }
}

/// Unit direction the curve LEAVES point `i` in.
fn outgoing_tangent(points: &[ProfilePoint], closed_loop: bool, i: usize) -> Vec2 {
    let n = points.len();
    let has_prev = closed_loop || i > 0;
    let next = if closed_loop { (i + 1) % n } else { i + 1 };
    let chord_out = direction(points[i].position, points[next].position);

    if points[i].sharp {
        chord_out
    } else if !has_prev {
        open_end_tangent(points, i, next, chord_out, true)
    } else {
        smooth_tangent(points, closed_loop, i, chord_out)
    }
}

/// Unit direction the curve ARRIVES at point `i` in.
fn incoming_tangent(points: &[ProfilePoint], closed_loop: bool, i: usize) -> Vec2 {
    let n = points.len();
    let has_next = closed_loop || i < n - 1;
    let prev = if closed_loop { (i + n - 1) % n } else { i - 1 };
    let chord_in = direction(points[prev].position, points[i].position);

    if points[i].sharp {
        chord_in
    } else if !has_next {
        open_end_tangent(points, i, prev, chord_in, false)
    } else {
        smooth_tangent(points, closed_loop, i, chord_in)
    }
}

/// Bisector of the unit chords either side - the direction a smooth curve through the three points
/// is heading in at the middle one.
fn smooth_tangent(points: &[ProfilePoint], closed_loop: bool, i: usize, fallback: Vec2) -> Vec2 {
    let n = points.len();
    let prev = if closed_loop { (i + n - 1) % n } else { i - 1 };
    let next = if closed_loop { (i + 1) % n } else { i + 1 };
    let sum =
        direction(points[prev].position, points[i].position) + direction(points[i].position, points[next].position);
    let len = sum.length();
    // A hairpin (the curve doubling straight back) has no bisector; the chord is as good an answer
    // as any, and this only ever happens mid-drag.
    if len > 1e-6 { sum / len } else { fallback }
}

/// The free end of an open profile. On the axis it leaves horizontally, so the pole closes in a
/// rounded dome instead of a cone point (mark the point sharp for a point). Off the axis the end
/// mirrors its neighbour's tangent across the chord, so the last segment is a symmetric arc rather
/// than a curve that straightens out as it reaches the end.
fn open_end_tangent(points: &[ProfilePoint], end: usize, neighbour: usize, chord: Vec2, leaving: bool) -> Vec2 {
    if points[end].position.x <= 0.0 {
        // Travel direction at the end: away from the axis when leaving it, toward it when arriving.
        return Vec2::new(if leaving { 1.0 } else { -1.0 }, 0.0);
    }

    let n = points.len();
    if n < 3 || points[neighbour].sharp {
        return chord;
    }

    // The neighbour's own smooth tangent, reflected across this segment's chord.
    let neighbour_tangent = if leaving {
        let after = neighbour + 1;
        if after >= n {
            return chord;
        }
        direction(points[end].position, points[neighbour].position)
            + direction(points[neighbour].position, points[after].position)
    } else {
        if neighbour == 0 {
            return chord;
        }
        let before = neighbour - 1;
        direction(points[before].position, points[neighbour].position)
            + direction(points[neighbour].position, points[end].position)
    };

    let len = neighbour_tangent.length();
    if len < 1e-6 {
        return chord;
    }
    let neighbour_tangent = neighbour_tangent / len;
    let reflected = 2.0 * chord.dot(neighbour_tangent) * chord - neighbour_tangent;
    let reflected_len = reflected.length();
    if reflected_len > 1e-6 { reflected / reflected_len } else { chord }
}

fn direction(from: Vec2, to: Vec2) -> Vec2 {
    let d = to - from;
    let len = d.length();
    if len > 1e-9 { d / len } else { Vec2::ZERO }
}

// --------------------------------------------------------------------------- resampling

/// Working storage for [`ResampleScratch::resample`], kept between calls so a live drag resamples
/// without allocating.
#[derive(Default)]
pub struct ResampleScratch {
    boundaries: Vec<usize>,
    dense: Vec<Vec2>,
    cumulative: Vec<f32>,
}

/// [`ResampleScratch::resample`] with throwaway scratch, returning the samples.
pub fn resample(points: &[ProfilePoint], closed_loop: bool, spacing: f32) -> Vec<Sample> {
    let mut output = Vec::new();
    ResampleScratch::default().resample(points, closed_loop, spacing, &mut output);
    output
}

impl ResampleScratch {
    /// Resamples the whole curve at (close to) `spacing`, appending to `output`. An open profile
    /// yields its first and last point exactly; a loop yields no duplicate closing sample (the mesh
    /// builder wraps it). Nothing is appended for fewer than two points or a spacing that is not
    /// positive.
    pub fn resample(&mut self, points: &[ProfilePoint], closed_loop: bool, spacing: f32, output: &mut Vec<Sample>) {
        let n = points.len();
        if n < 2 || spacing.is_nan() || spacing <= 0.0 {
            return;
        }
        let segments = segment_count(points, closed_loop);

        // Span boundaries: the control points where the curve may turn a corner. An open profile's
        // ends always are; a loop with no sharp points is one span that starts and ends at point 0.
        self.boundaries.clear();
        for (i, p) in points.iter().enumerate() {
            let end = !closed_loop && (i == 0 || i == n - 1);
            if end || p.sharp {
                self.boundaries.push(i);
            }
        }
        let seamless = closed_loop && self.boundaries.is_empty();
        if seamless {
            self.boundaries.push(0);
        }

        let span_count = if closed_loop { self.boundaries.len() } else { self.boundaries.len() - 1 };
        for s in 0..span_count {
            let start_point = self.boundaries[s];
            let end_point =
                if closed_loop { self.boundaries[(s + 1) % self.boundaries.len()] } else { self.boundaries[s + 1] };
            let span_segments = if closed_loop {
                match (end_point + n - start_point) % n {
                    0 => n,
                    d => d,
                }
            } else {
                end_point - start_point
            };
            if span_segments == 0 {
                continue;
            }

            self.measure_span(points, closed_loop, start_point, span_segments, segments);
            self.emit_span(s, span_count, closed_loop, seamless, spacing, output);
        }
    }

    /// The span as a dense polyline with running arc length, in `dense` / `cumulative`.
    fn measure_span(
        &mut self,
        points: &[ProfilePoint],
        closed_loop: bool,
        start_point: usize,
        span_segments: usize,
        segments: usize,
    ) {
        let n = points.len();
        self.dense.clear();
        self.cumulative.clear();
        self.dense.push(points[start_point].position);
        self.cumulative.push(0.0);
        for k in 0..span_segments {
            let seg = if closed_loop { (start_point + k) % n } else { start_point + k };
            if seg >= segments {
                break;
            }
            let hermite = Hermite::new(points, closed_loop, seg);
            for step in 1..=DENSE_STEPS_PER_SEGMENT {
                let p = hermite.at(step as f32 / DENSE_STEPS_PER_SEGMENT as f32);
                let length =
                    self.cumulative[self.cumulative.len() - 1] + (p - self.dense[self.dense.len() - 1]).length();
                self.cumulative.push(length);
                self.dense.push(p);
            }
        }
    }

    /// Samples the measured span at equal arc length, `pieces` of them chosen so the spacing
    /// divides the span exactly.
    fn emit_span(
        &self,
        s: usize,
        span_count: usize,
        closed_loop: bool,
        seamless: bool,
        spacing: f32,
        output: &mut Vec<Sample>,
    ) {
        let length = self.cumulative[self.cumulative.len() - 1];
        // A loop needs three samples at the very least to enclose anything.
        let min_pieces = if closed_loop { 3usize.div_ceil(span_count) } else { 1 };
        let pieces = min_pieces.max((length / spacing).round_ties_even() as usize);
        let corner = !seamless;

        // Every span starts where the previous one ended, so only the very first span emits its
        // start point.
        if s == 0 {
            output.push(Sample { position: self.dense[0], corner });
        }

        let mut cursor = 0;
        for k in 1..=pieces {
            // Closing sample of a loop's last span is the first sample again - skipped.
            if closed_loop && s == span_count - 1 && k == pieces {
                break;
            }
            if k == pieces {
                // The span's end, exactly - it is a control point, and a corner.
                output.push(Sample { position: self.dense[self.dense.len() - 1], corner });
                break;
            }

            let target = length * k as f32 / pieces as f32;
            while cursor < self.cumulative.len() - 2 && self.cumulative[cursor + 1] < target {
                cursor += 1;
            }
            let seg_len = self.cumulative[cursor + 1] - self.cumulative[cursor];
            let t = if seg_len > 1e-12 { ((target - self.cumulative[cursor]) / seg_len).clamp(0.0, 1.0) } else { 0.0 };
            let (a, b) = (self.dense[cursor], self.dense[cursor + 1]);
            output.push(Sample { position: a + (b - a) * t, corner: false });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn smooth(points: &[(f32, f32)]) -> Vec<ProfilePoint> {
        points.iter().map(|&(r, h)| ProfilePoint::smooth(r, h)).collect()
    }

    #[test]
    fn the_curve_passes_through_every_control_point() {
        let pts = smooth(&[(0.0, 0.0), (0.4, 0.3), (0.5, 0.9), (0.2, 1.5), (0.3, 2.0)]);
        for seg in 0..segment_count(&pts, false) {
            assert!((evaluate(&pts, false, seg, 0.0) - pts[seg].position).length() < 1e-6);
            assert!((evaluate(&pts, false, seg, 1.0) - pts[seg + 1].position).length() < 1e-6);
        }
    }

    #[test]
    fn a_loop_wraps_its_last_segment_to_the_first_point() {
        let pts = smooth(&[(1.0, 0.0), (2.0, 1.0), (1.0, 2.0), (0.5, 1.0)]);
        assert_eq!(segment_count(&pts, true), 4);
        assert!((evaluate(&pts, true, 3, 1.0) - pts[0].position).length() < 1e-6);
    }

    #[test]
    fn an_axis_end_leaves_horizontally_so_the_pole_is_a_dome() {
        let pts = smooth(&[(0.0, 0.0), (0.5, 0.5), (0.5, 1.0)]);
        let d = evaluate(&pts, false, 0, 0.01) - evaluate(&pts, false, 0, 0.0);
        assert!(d.x > 0.0 && d.y.abs() < 0.1 * d.x, "{d:?}");
    }

    #[test]
    fn sharp_points_give_straight_runs_with_the_chord_as_tangent() {
        let pts = vec![ProfilePoint::corner(1.0, 0.0), ProfilePoint::corner(1.0, 2.0)];
        for step in 0..=10 {
            let p = evaluate(&pts, false, 0, step as f32 / 10.0);
            assert!((p.x - 1.0).abs() < 1e-6 && (p.y - 0.2 * step as f32).abs() < 1e-5);
        }
    }

    #[test]
    fn uneven_points_do_not_loop_back_on_themselves() {
        let pts = smooth(&[(0.3, 0.0), (0.3, 0.01), (0.3, 2.0)]);
        let mut last = f32::MIN;
        for seg in 0..2 {
            for step in 0..=20 {
                let y = evaluate(&pts, false, seg, step as f32 / 20.0).y;
                assert!(y >= last - 1e-5, "height went backwards: {y} after {last}");
                last = y;
            }
        }
    }

    #[test]
    fn resampling_an_open_profile_keeps_both_ends_exactly_and_spaces_evenly() {
        let pts = smooth(&[(0.2, 0.0), (0.5, 0.5), (0.3, 1.0), (0.4, 1.6)]);
        let samples = resample(&pts, false, 0.05);
        assert_eq!(samples.first().unwrap().position, pts[0].position);
        assert_eq!(samples.last().unwrap().position, pts[3].position);
        assert!(samples.first().unwrap().corner && samples.last().unwrap().corner);
        let gaps: Vec<f32> = samples.windows(2).map(|w| (w[1].position - w[0].position).length()).collect();
        let (lo, hi) = gaps.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &g| (lo.min(g), hi.max(g)));
        assert!(lo > 0.04 && hi < 0.055, "gaps {lo}..{hi}");
    }

    #[test]
    fn corners_land_exactly_on_a_sample() {
        let mut pts = smooth(&[(0.0, 0.0), (0.7, 0.0), (0.7, 1.0), (0.0, 1.0)]);
        pts[1].sharp = true;
        pts[2].sharp = true;
        let samples = resample(&pts, false, 0.0731);
        for corner in [pts[1].position, pts[2].position] {
            assert!(samples.iter().any(|s| s.corner && s.position == corner), "no sample on {corner:?}");
        }
    }

    #[test]
    fn a_loop_has_no_duplicate_closing_sample_and_encloses_something() {
        let n = 8;
        let pts: Vec<ProfilePoint> = (0..n)
            .map(|i| {
                let a = std::f32::consts::TAU * i as f32 / n as f32;
                ProfilePoint::smooth(2.0 + a.cos() * 0.5, a.sin() * 0.5)
            })
            .collect();
        let samples = resample(&pts, true, 0.1);
        assert!(samples.len() >= 3);
        assert!((samples[0].position - samples[samples.len() - 1].position).length() > 0.05);
        assert!(samples.iter().all(|s| !s.corner), "a loop with no sharp points has no corners");
        // Sample count is the circumference over the spacing (about pi / 0.1).
        assert!((28..=36).contains(&samples.len()), "{} samples", samples.len());
    }

    #[test]
    fn a_loop_with_one_corner_closes_on_it() {
        let mut pts = smooth(&[(1.0, 0.0), (2.0, 0.0), (2.0, 1.0), (1.0, 1.0)]);
        pts[0].sharp = true;
        let samples = resample(&pts, true, 0.2);
        assert_eq!(samples[0].position, pts[0].position);
        assert!(samples[0].corner);
        assert!((samples[samples.len() - 1].position - pts[0].position).length() > 0.05);
    }

    #[test]
    fn nothing_comes_from_too_few_points_or_a_bad_spacing() {
        let one = smooth(&[(1.0, 1.0)]);
        assert!(resample(&one, false, 0.1).is_empty());
        let two = smooth(&[(1.0, 0.0), (1.0, 1.0)]);
        assert!(resample(&two, false, 0.0).is_empty());
        assert!(resample(&two, false, f32::NAN).is_empty());
        assert_eq!(resample(&two, false, 0.25).len(), 5);
    }

    #[test]
    fn control_polygon_length_counts_the_closing_chord_of_a_loop() {
        let pts = smooth(&[(0.0, 0.0), (3.0, 0.0), (3.0, 4.0)]);
        assert!((control_polygon_length(&pts, false) - 7.0).abs() < 1e-6);
        assert!((control_polygon_length(&pts, true) - 12.0).abs() < 1e-6);
    }
}
