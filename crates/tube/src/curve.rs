//! The tube's centre line: a smooth curve through its control points,
//! resampled into stations for the tube's distance field.
//!
//! Cubic Hermite, with the tangent direction at a point taken from the
//! bisector of the neighbouring chords and its length from the segment's own
//! chord. Unevenly spaced points, which is what simplifying a hand-drawn
//! stroke produces, then never make loops or cusps. The tangent length is
//! also scaled by a tension (the Smoothness setting), so the artist can go
//! from straight runs to loose curves without the curve ever leaving its
//! points, and the radius is interpolated along with the position.
//!
//! Sharp points split the curve into spans, each resampled on its own so the
//! corner lands exactly on a station.

use glam::Vec3;

use crate::types::{ControlPoint, Sample};

/// Tension at Smoothness 1. Past about 1.6 a sparse point set starts to bulge
/// out between points.
pub const MAX_TENSION: f32 = 1.6;

/// Below this tension the curve is effectively straight between points, and
/// every point becomes a span boundary so each kink lands on a station.
const STRAIGHT_TENSION: f32 = 0.05;

const DENSE_STEPS: usize = 32;

pub fn tension_for(smoothness: f32) -> f32 {
    smoothness.clamp(0.0, 1.0) * MAX_TENSION
}

pub fn segment_count(points: &[ControlPoint]) -> usize {
    points.len().saturating_sub(1)
}

fn hermite(t: f32) -> [f32; 4] {
    let t2 = t * t;
    let t3 = t2 * t;
    [2.0 * t3 - 3.0 * t2 + 1.0, t3 - 2.0 * t2 + t, -2.0 * t3 + 3.0 * t2, t3 - t2]
}

/// Centre-line position on control segment `segment` (point `segment` to
/// `segment + 1`) at `t`.
pub fn evaluate(points: &[ControlPoint], tension: f32, segment: usize, t: f32) -> Vec3 {
    let p0 = points[segment].position;
    let p1 = points[segment + 1].position;
    let chord = p0.distance(p1);
    if chord < 1e-9 {
        return p0;
    }
    let m0 = outgoing_tangent(points, segment) * (chord * tension);
    let m1 = incoming_tangent(points, segment + 1) * (chord * tension);
    let [h0, h1, h2, h3] = hermite(t);
    h0 * p0 + h1 * m0 + h2 * p1 + h3 * m1
}

/// Radius on control segment `segment` at `t`: a monotone cubic through the
/// points' radii, so a thick point swells smoothly into its neighbours
/// without the radius ever overshooting past either of them (a plain spline
/// would dip below zero next to a thin point).
pub fn evaluate_radius(points: &[ControlPoint], segment: usize, t: f32) -> f32 {
    let r0 = points[segment].radius;
    let r1 = points[segment + 1].radius;
    let [h0, h1, h2, h3] = hermite(t);
    let r = h0 * r0 + h1 * radius_slope(points, segment) + h2 * r1 + h3 * radius_slope(points, segment + 1);
    r.max(r0.min(r1) * 0.5)
}

fn radius_slope(points: &[ControlPoint], i: usize) -> f32 {
    let n = points.len();
    if n < 2 {
        return 0.0;
    }
    if i == 0 {
        return points[1].radius - points[0].radius;
    }
    if i == n - 1 {
        return points[n - 1].radius - points[n - 2].radius;
    }
    let before = points[i].radius - points[i - 1].radius;
    let after = points[i + 1].radius - points[i].radius;
    if before * after <= 0.0 {
        return 0.0;
    }
    2.0 * before * after / (before + after)
}

fn direction(from: Vec3, to: Vec3) -> Vec3 {
    let d = to - from;
    let length = d.length();
    if length > 1e-9 { d / length } else { Vec3::ZERO }
}

fn outgoing_tangent(points: &[ControlPoint], i: usize) -> Vec3 {
    let chord = direction(points[i].position, points[i + 1].position);
    if points[i].sharp {
        chord
    } else if i == 0 {
        open_end_tangent(points, 0, 1, chord, true)
    } else {
        smooth_tangent(points, i, chord)
    }
}

fn incoming_tangent(points: &[ControlPoint], i: usize) -> Vec3 {
    let chord = direction(points[i - 1].position, points[i].position);
    if points[i].sharp {
        chord
    } else if i == points.len() - 1 {
        open_end_tangent(points, i, i - 1, chord, false)
    } else {
        smooth_tangent(points, i, chord)
    }
}

fn smooth_tangent(points: &[ControlPoint], i: usize, fallback: Vec3) -> Vec3 {
    let sum = direction(points[i - 1].position, points[i].position) + direction(points[i].position, points[i + 1].position);
    let length = sum.length();
    if length > 1e-6 { sum / length } else { fallback }
}

/// The free end mirrors its neighbour's tangent across the end chord, so the
/// last span is a symmetric arc rather than a curve that straightens out as
/// it reaches the end.
fn open_end_tangent(points: &[ControlPoint], end: usize, neighbour: usize, chord: Vec3, leaving: bool) -> Vec3 {
    if points.len() < 3 || points[neighbour].sharp {
        return chord;
    }
    let at = |i: usize| points[i].position;
    let neighbour_tangent = if leaving {
        direction(at(end), at(neighbour)) + direction(at(neighbour), at(neighbour + 1))
    } else {
        direction(at(neighbour - 1), at(neighbour)) + direction(at(neighbour), at(end))
    };
    let length = neighbour_tangent.length();
    if length < 1e-6 {
        return chord;
    }
    let neighbour_tangent = neighbour_tangent / length;
    let reflected = 2.0 * chord.dot(neighbour_tangent) * chord - neighbour_tangent;
    let reflected_length = reflected.length();
    if reflected_length > 1e-6 { reflected / reflected_length } else { chord }
}

/// One point of the dense polyline the stations are picked from.
struct Dense {
    position: Vec3,
    radius: f32,
    /// Arc length from the start of the tube.
    arc: f32,
    /// The "spacing budget" used so far in this span: the integral of
    /// `ds / spacing`.
    u: f32,
    t: f32,
    segment: usize,
}

/// Resamples the curve into stations. They are spaced about
/// `spacing_per_radius` times the local radius apart, so a thick part and a
/// thin tip both get stations roughly as far apart as they are wide, but
/// never closer than `min_spacing`, and never more than about `max_samples`
/// in all. Empty when there is no curve (fewer than two distinct points).
pub fn resample(
    points: &[ControlPoint],
    tension: f32,
    spacing_per_radius: f32,
    min_spacing: f32,
    max_samples: usize,
) -> Vec<Sample> {
    // Coincident neighbours have no direction between them; drop the repeats,
    // keeping the index of the point each kept one came from so `segment`
    // still names the artist's points.
    let mut clean: Vec<ControlPoint> = Vec::with_capacity(points.len());
    let mut source: Vec<usize> = Vec::with_capacity(points.len());
    for (i, point) in points.iter().enumerate() {
        if let Some(last) = clean.last_mut() {
            if point.position.distance_squared(last.position) < 1e-12 {
                // Keep the sharper and fatter of the two.
                last.sharp |= point.sharp;
                last.radius = last.radius.max(point.radius);
                continue;
            }
        }
        clean.push(*point);
        source.push(i);
    }
    let n = clean.len();
    if n < 2 {
        return Vec::new();
    }

    let straight = tension < STRAIGHT_TENSION;
    let mut boundaries = vec![0];
    boundaries.extend((1..n - 1).filter(|&i| straight || clean[i].sharp));
    boundaries.push(n - 1);

    // A dense polyline over the whole curve, with arc length and the spacing
    // budget accumulated per span.
    let min_spacing = min_spacing.max(1e-6);
    let mut dense: Vec<Dense> = Vec::with_capacity((n - 1) * DENSE_STEPS + boundaries.len());
    let mut span_start = Vec::with_capacity(boundaries.len());
    let mut u_total = 0.0;
    for span in boundaries.windows(2) {
        let first = dense.len();
        span_start.push(first);
        for segment in span[0]..span[1] {
            let first_step = if segment == span[0] { 0 } else { 1 };
            for step in first_step..=DENSE_STEPS {
                let t = step as f32 / DENSE_STEPS as f32;
                let position = evaluate(&clean, tension, segment, t);
                let radius = evaluate_radius(&clean, segment, t);
                let (mut arc, mut u) = (0.0, 0.0);
                if let Some(previous) = dense.last() {
                    if dense.len() > first {
                        let ds = position.distance(previous.position);
                        let spacing = min_spacing.max(spacing_per_radius * 0.5 * (radius + previous.radius));
                        arc = previous.arc + ds;
                        u = previous.u + ds / spacing;
                    } else {
                        // First dense point of a later span: the same place as
                        // the last one of the span before, but its budget
                        // restarts (each span is divided on its own).
                        arc = previous.arc;
                    }
                }
                dense.push(Dense { position, radius, arc, u, t, segment });
            }
        }
        u_total += dense[dense.len() - 1].u;
    }
    span_start.push(dense.len());

    let budget_scale = if u_total > max_samples as f32 { max_samples as f32 / u_total } else { 1.0 };
    let spans = span_start.len() - 1;
    let mut output = Vec::new();
    for s in 0..spans {
        let first = span_start[s];
        let last = span_start[s + 1] - 1;
        let span_u = dense[last].u;
        let pieces = (span_u * budget_scale).round().max(1.0) as usize;
        let mut cursor = first;
        // Every span after the first starts on the station the previous one
        // ended on.
        for k in (if s == 0 { 0 } else { 1 })..=pieces {
            let (a, b, f) = if k == pieces {
                // The span's end, exactly: a control point (and a corner,
                // unless it is the tube's last point).
                (last, last, 0.0)
            } else {
                let target = span_u * k as f32 / pieces as f32;
                while cursor + 1 < last && dense[cursor + 1].u < target {
                    cursor += 1;
                }
                let du = dense[cursor + 1].u - dense[cursor].u;
                let f = if du > 1e-12 { ((target - dense[cursor].u) / du).clamp(0.0, 1.0) } else { 0.0 };
                (cursor, cursor + 1, f)
            };
            let (da, db) = (&dense[a], &dense[b]);
            let end_t = if db.segment == da.segment { db.t } else { 1.0 };
            output.push(Sample {
                center: da.position.lerp(db.position, f),
                radius: da.radius + (db.radius - da.radius) * f,
                distance: da.arc + (db.arc - da.arc) * f,
                segment: source[da.segment],
                segment_t: da.t + (end_t - da.t) * f,
                corner: k == pieces && s + 1 < spans,
                ..Sample::default()
            });
        }
    }
    compute_tangents(&mut output);
    if output.len() >= 2 { output } else { Vec::new() }
}

/// Finite-difference tangents: central inside a span, one-sided at the ends
/// and on either side of a corner. A zero-length difference (never expected,
/// but a degenerate drag can produce one) inherits its neighbour's direction.
fn compute_tangents(samples: &mut [Sample]) {
    let n = samples.len();
    let mut last_good = Vec3::Z;
    for i in 0..n {
        let here = samples[i].center;
        let back = if i > 0 { direction(samples[i - 1].center, here) } else { Vec3::ZERO };
        let ahead = if i + 1 < n { direction(here, samples[i + 1].center) } else { Vec3::ZERO };

        let (mut tangent_in, mut tangent_out) = if samples[i].corner {
            (back, ahead)
        } else {
            let back_is_corner = i > 0 && samples[i - 1].corner;
            let ahead_is_corner = i + 1 < n && samples[i + 1].corner;
            let central = if i == 0 {
                ahead
            } else if i == n - 1 {
                back
            } else if back_is_corner && !ahead_is_corner {
                ahead
            } else if ahead_is_corner && !back_is_corner {
                back
            } else {
                direction(samples[i - 1].center, samples[i + 1].center)
            };
            (central, central)
        };

        if tangent_in.length_squared() < 0.5 {
            tangent_in = if tangent_out.length_squared() > 0.5 { tangent_out } else { last_good };
        }
        if tangent_out.length_squared() < 0.5 {
            tangent_out = tangent_in;
        }
        last_good = tangent_out;
        samples[i].tangent_in = tangent_in;
        samples[i].tangent_out = tangent_out;
    }
}

/// Ramer-Douglas-Peucker in 3D: the indices of the points worth keeping so a
/// polyline through them stays within `tolerance` of every dropped point.
/// The ends are always kept.
pub fn simplify(points: &[Vec3], tolerance: f32) -> Vec<usize> {
    let n = points.len();
    if n < 2 {
        return (0..n).collect();
    }
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    let mut stack = vec![(0, n - 1)];
    while let Some((first, last)) = stack.pop() {
        if last <= first + 1 {
            continue;
        }
        let mut farthest = None;
        let mut max_distance = tolerance;
        for i in first + 1..last {
            let d = distance_to_segment(points[i], points[first], points[last]);
            if d > max_distance {
                max_distance = d;
                farthest = Some(i);
            }
        }
        if let Some(index) = farthest {
            keep[index] = true;
            stack.push((first, index));
            stack.push((index, last));
        }
    }
    (0..n).filter(|&i| keep[i]).collect()
}

pub fn distance_to_segment(p: Vec3, a: Vec3, b: Vec3) -> f32 {
    let ab = b - a;
    let length_squared = ab.length_squared();
    let t = if length_squared > 1e-12 { ((p - a).dot(ab) / length_squared).clamp(0.0, 1.0) } else { 0.0 };
    (a + ab * t).distance(p)
}
