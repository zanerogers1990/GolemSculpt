//! The tessellated profile: the resampled curve plus the caps and the axis rules, oriented so the
//! revolved surface faces outward, with the normal and self-intersection checks the builder needs.

use glam::Vec2;
use rayon::prelude::*;

use crate::curve::Sample;

/// A profile with this many segments or more checks itself for crossings in parallel; below it the
/// threads cost more than the quadratic scan.
const PARALLEL_CROSSING_SEGMENTS: usize = 384;

/// What the axis rules had to do to the curve.
pub(crate) struct AxisRules {
    /// Part of the curve swung across the axis and was held just off it.
    pub touches_axis: bool,
}

/// Writes the profile to revolve into `out`: the resampled curve, an end that is close to the axis
/// closed onto it, and (with `cap_ends`) a flat disc closing an end that stops short of it.
/// `min_interior` is the radius interior samples are held above.
pub(crate) fn tessellate(
    samples: &[Sample],
    closed_loop: bool,
    cap_ends: bool,
    spacing: f32,
    min_interior: f32,
    out: &mut Vec<Vec2>,
) -> AxisRules {
    out.clear();
    let mut rules = AxisRules { touches_axis: false };
    let n = samples.len();

    // Held off the axis: at least the profile's own interior minimum, and a quarter of the spacing
    // so the thinnest ring still has room for its four vertices.
    let min_interior = min_interior.max(spacing * 0.25);
    // An end this close to the axis closes onto it. Leaving it would put a ring of a few vertices a
    // hair from the axis - a pinhole with a fan of slivers around it.
    let pole_snap = spacing * 0.5;

    let mut first = samples[0].position;
    let mut last = samples[n - 1].position;
    if !closed_loop && first.x < pole_snap {
        first.x = 0.0;
    }
    if !closed_loop && last.x < pole_snap {
        last.x = 0.0;
    }

    if !closed_loop && cap_ends && first.x > 0.0 {
        add_cap(out, first, spacing, true);
    }

    for (i, sample) in samples.iter().enumerate() {
        let mut p = sample.position;
        if !closed_loop && i == 0 {
            p = first;
        } else if !closed_loop && i == n - 1 {
            p = last;
        } else if p.x < min_interior {
            p.x = min_interior;
            rules.touches_axis = true;
        }
        add_unique(out, p, spacing);
    }

    if !closed_loop && cap_ends && last.x > 0.0 {
        add_cap(out, last, spacing, false);
    }

    // A loop's closing duplicate, if resampling rounding left one.
    let eps = spacing * 1e-3;
    if closed_loop && out.len() > 2 && (out[0] - out[out.len() - 1]).length_squared() < eps * eps {
        out.pop();
    }
    rules
}

/// A flat disc from the rim point to the axis, at the rim's height, sampled at the same spacing as
/// the wall so the cap's faces match the wall's.
fn add_cap(out: &mut Vec<Vec2>, rim: Vec2, spacing: f32, from_axis: bool) {
    let pieces = ((rim.x / spacing).round_ties_even() as usize).max(1);
    let at = |k: usize| Vec2::new(rim.x * k as f32 / pieces as f32, rim.y);
    if from_axis {
        // Axis up to (not including) the rim, which the wall's first sample supplies.
        for k in 0..pieces {
            add_unique(out, at(k), spacing);
        }
    } else {
        for k in (0..pieces).rev() {
            add_unique(out, at(k), spacing);
        }
    }
}

fn add_unique(out: &mut Vec<Vec2>, p: Vec2, spacing: f32) {
    let eps = spacing * 1e-3;
    if out.last().is_some_and(|&last| (last - p).length_squared() < eps * eps) {
        return;
    }
    out.push(p);
}

/// Reverses the profile if needed so the revolved surface faces OUT of the solid. The artist can
/// draw the curve top-down or bottom-up; either way the result is the same.
///
/// Decided by the signed volume of revolution, pi * (closed integral of r^2 dh), not by the
/// profile's signed area. The two agree for a simple curve, but a curve that dips across the axis
/// twice gets both dips clamped onto the same thin radius and folds over itself - its area can then
/// be positive while the solid it sweeps is inside out. Volume is also what the faces have to agree
/// with, and it needs no closing segment for an open profile: the axis has r = 0, so it contributes
/// nothing.
pub(crate) fn orient_outward(profile: &mut [Vec2], closed_loop: bool) {
    let n = profile.len();
    let segments = if closed_loop { n } else { n - 1 };
    let mut volume = 0.0f64;
    for i in 0..segments {
        let (a, b) = (profile[i], profile[(i + 1) % n]);
        // Exact integral of r^2 dh along the straight segment a -> b.
        let (ax, bx) = (a.x as f64, b.x as f64);
        volume += (b.y as f64 - a.y as f64) * (ax * ax + ax * bx + bx * bx) / 3.0;
    }
    // A counter-clockwise (r right, h up) boundary encloses positive volume, and on it the
    // right-hand normal is the outward one - see `normal`.
    if volume < 0.0 {
        profile.reverse();
    }
}

/// Outward normal of the profile at sample `i`, in (radius, height) space. Corners get the bisector
/// of the two sides too, which is what an averaged vertex normal would come to.
pub(crate) fn normal(profile: &[Vec2], i: usize, closed_loop: bool) -> Vec2 {
    let n = profile.len();
    let has_prev = closed_loop || i > 0;
    let has_next = closed_loop || i + 1 < n;
    let p = profile[i];
    let dir_in = if has_prev { (p - profile[(i + n - 1) % n]).normalize_or_zero() } else { Vec2::ZERO };
    let dir_out = if has_next { (profile[(i + 1) % n] - p).normalize_or_zero() } else { Vec2::ZERO };

    let mut t = dir_in + dir_out;
    if t.length_squared() < 1e-12 {
        t = if dir_out.length_squared() > 0.0 { dir_out } else { dir_in };
    }
    let t = t.normalize_or_zero();
    // Right-hand normal of a counter-clockwise boundary points outward.
    Vec2::new(t.y, -t.x)
}

/// Whether the profile crosses itself, so the revolved surface would pass through itself.
pub(crate) fn crosses_itself(profile: &[Vec2], closed_loop: bool) -> bool {
    let n = profile.len();
    let segments = if closed_loop { n } else { n.saturating_sub(1) };
    if segments >= PARALLEL_CROSSING_SEGMENTS {
        (0..segments).into_par_iter().any(|i| segment_crosses_later(profile, closed_loop, segments, i))
    } else {
        (0..segments).any(|i| segment_crosses_later(profile, closed_loop, segments, i))
    }
}

/// Whether segment `i` crosses any segment after its neighbour.
fn segment_crosses_later(profile: &[Vec2], closed_loop: bool, segments: usize, i: usize) -> bool {
    let n = profile.len();
    let (a0, a1) = (profile[i], profile[(i + 1) % n]);
    let (min, max) = (a0.min(a1), a0.max(a1));
    for j in i + 2..segments {
        // Neighbours share an endpoint - including the loop's last and first segments.
        if closed_loop && i == 0 && j == segments - 1 {
            continue;
        }
        let (b0, b1) = (profile[j], profile[(j + 1) % n]);
        if b0.max(b1).cmplt(min).any() || b0.min(b1).cmpgt(max).any() {
            continue;
        }
        if segments_cross(a0, a1, b0, b1) {
            return true;
        }
    }
    false
}

/// Proper crossing only: segments that merely touch at an end do not count.
fn segments_cross(p1: Vec2, p2: Vec2, q1: Vec2, q2: Vec2) -> bool {
    let d1 = (q2 - q1).perp_dot(p1 - q1);
    let d2 = (q2 - q1).perp_dot(p2 - q1);
    let d3 = (p2 - p1).perp_dot(q1 - p1);
    let d4 = (p2 - p1).perp_dot(q2 - p1);
    ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0)) && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f32, y: f32) -> Vec2 {
        Vec2::new(x, y)
    }

    #[test]
    fn a_crossing_is_found_and_a_touch_is_not() {
        let bow_tie = [v(0.0, 0.0), v(1.0, 1.0), v(1.0, 0.0), v(0.0, 1.0)];
        assert!(crosses_itself(&bow_tie, false));
        let zigzag = [v(0.0, 0.0), v(1.0, 0.5), v(0.0, 1.0), v(1.0, 1.5)];
        assert!(!crosses_itself(&zigzag, false));
        let square = [v(0.0, 0.0), v(1.0, 0.0), v(1.0, 1.0), v(0.0, 1.0)];
        assert!(!crosses_itself(&square, true), "a loop's last and first segments are neighbours");
    }

    #[test]
    fn long_profiles_are_checked_in_parallel_with_the_same_answer() {
        let n = PARALLEL_CROSSING_SEGMENTS * 2;
        let mut line: Vec<Vec2> = (0..n).map(|i| v(0.5 + 0.1 * (i as f32).sin(), i as f32 * 0.01)).collect();
        assert!(!crosses_itself(&line, false));
        // Fold the tail back down through the start of the line.
        line[n - 3] = v(0.5, 0.5);
        line[n - 2] = v(0.5, 0.0);
        line[n - 1] = v(0.6, 0.0);
        assert!(crosses_itself(&line, false));
    }

    #[test]
    fn a_profile_drawn_downward_is_turned_to_run_upward() {
        let up = [v(0.0, 0.0), v(1.0, 0.0), v(1.0, 1.0), v(0.0, 1.0)];
        let mut a = up;
        orient_outward(&mut a, false);
        assert_eq!(a, up);
        let mut b = [up[3], up[2], up[1], up[0]];
        orient_outward(&mut b, false);
        assert_eq!(b, up);
    }

    #[test]
    fn normals_point_away_from_the_axis_on_a_wall_and_down_on_a_base() {
        let profile = [v(0.0, 0.0), v(1.0, 0.0), v(1.0, 1.0), v(0.0, 1.0)];
        assert!((normal(&profile, 1, false) - v(1.0, -1.0).normalize()).length() < 1e-6, "corner bisector");
        let base = [v(0.0, 0.0), v(0.5, 0.0), v(1.0, 0.0), v(1.0, 1.0)];
        assert!((normal(&base, 1, false) - v(0.0, -1.0)).length() < 1e-6);
        let wall = [v(1.0, 0.0), v(1.0, 0.5), v(1.0, 1.0)];
        assert!((normal(&wall, 1, false) - v(1.0, 0.0)).length() < 1e-6);
    }

    #[test]
    fn caps_close_an_open_end_down_to_the_axis() {
        let samples = [Sample { position: v(1.0, 0.0), corner: true }, Sample { position: v(1.0, 1.0), corner: true }];
        let mut out = Vec::new();
        tessellate(&samples, false, true, 0.25, 0.01, &mut out);
        assert_eq!(out[0], v(0.0, 0.0));
        assert_eq!(out[out.len() - 1], v(0.0, 1.0));
        assert!(out.windows(2).all(|w| w[0] != w[1]));
        let mut open = Vec::new();
        tessellate(&samples, false, false, 0.25, 0.01, &mut open);
        assert_eq!(open, vec![v(1.0, 0.0), v(1.0, 1.0)]);
    }

    #[test]
    fn interior_samples_that_dip_to_the_axis_are_held_off_it_and_reported() {
        let s = |x, y| Sample { position: v(x, y), corner: false };
        let samples = [s(1.0, 0.0), s(0.0, 1.0), s(1.0, 2.0)];
        let mut out = Vec::new();
        let rules = tessellate(&samples, false, false, 0.2, 0.01, &mut out);
        assert!(rules.touches_axis);
        assert_eq!(out[1].x, 0.05, "a quarter of the spacing beats the profile minimum");
    }
}
