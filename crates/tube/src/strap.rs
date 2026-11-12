//! A strap: a flat band of even width and thickness laid on the surface, built as a swept mesh.
//!
//! Its centre line is a muscle's, read another way: the stations between its points are put back
//! on the surface the way a muscle's are (`muscle::settle`), so a strap drawn with two points over
//! a shoulder lies over the shoulder. What is laid there is a strap's own cross-section: a rounded
//! rectangle as wide as the strap and as thick as Thickness says, the same all along.
//!
//! Why a swept mesh and not a distance field, as tubes and muscles are: a field is meshed on a voxel
//! lattice, which rounds every edge and, at a belt's thinness, needs a lattice so fine the preview
//! could not keep up. A strap wants crisp edges and an even thickness, which rings of a profile
//! swept along the line give exactly, at a few thousand triangles.
//!
//! The strap rests on the skin rather than sinking into it. Across its width it is felt at
//! [`ACROSS`] places at every station, and its underside follows them: over a rounded form it
//! bends round it, as leather does. It never dips under any of them, and it stands Gap off them.
//! Along its length it is drawn taut by Tight: each place on its underside is pushed out until it
//! is no lower than the middle of its neighbours, which is a string pulled tight over the body.
//! Over a convex form that changes nothing; over a hollow (the small of the back) it bridges.
//!
//! A closed strap (a belt) runs from its last point back to its first; [`sample`] builds its line
//! as one loop, with no seam.

use glam::{Mat3, Mat4, Vec3};
use sculpt_geom::{SymmetryOp, TriMesh};

use crate::curve;
use crate::muscle::{self, Ground};
use crate::spine::MAX_SAMPLES;
use crate::types::{ControlPoint, Sample, Tip, TubePoint, TubeSettings};

/// Station spacing per unit of the strap's half width.
pub const SPACING_PER_RADIUS: f32 = 0.2;

/// Places the surface is felt at across the strap at each station, edge to edge.
pub const ACROSS: usize = 9;

/// Segments of each rounded corner of the cross-section.
const CORNER_STEPS: usize = 4;

/// The narrowest a pointed or rounded end gets before it is closed, as a share of the strap's
/// half width.
const MIN_TIP: f32 = 0.06;

/// Most passes of the pull that draws the strap taut, and how little a pass has to move it by to
/// stop early (as a share of the strap's thickness).
const MAX_TAUT_PASSES: usize = 3000;
const TAUT_SETTLED: f32 = 1e-4;

/// The strap as laid on the model: everything its mesh is made from, so the mesh can be made again
/// at another gap (sunk into the model for a merge) without feeling the surface again.
#[derive(Clone, Debug, Default)]
pub struct Laid {
    /// Per station, in the frame: where its centre line is on the surface, the way across it and
    /// the way up off the surface, and its half width there (narrowed at a pointed or round end).
    pub centre: Vec<Vec3>,
    pub across: Vec<Vec3>,
    pub up: Vec<Vec3>,
    pub half_width: Vec<f32>,
    /// Per station, [`ACROSS`] heights of the underside above `centre` along `up`, edge to edge,
    /// drawn as taut as the strap is: where the strap rests before Gap.
    pub bottom: Vec<[f32; ACROSS]>,
    /// Per station, the strap's own half width, before an end narrowed it: what `bottom` was felt
    /// across.
    pub full_width: Vec<f32>,
    /// How thick the strap is, how round its edges (0 to 1) and how far it stands off the skin.
    pub thickness: f32,
    pub edge: f32,
    pub gap: f32,
    pub closed: bool,
}

impl Laid {
    pub fn is_empty(&self) -> bool {
        self.centre.len() < 2
    }

    /// The strap's surface: rings of its cross-section along it, joined end to end, the ends closed
    /// (or the last ring joined back to the first, for a closed strap). `gap` overrides how far it
    /// stands off the skin; negative sinks it in.
    pub fn mesh(&self, gap: Option<f32>) -> TriMesh {
        let n = self.centre.len();
        if n < 2 {
            return TriMesh::default();
        }
        let gap = gap.unwrap_or(self.gap);
        let t = self.thickness.max(1e-7);
        let mut positions = Vec::new();
        let mut ring_size = 0;
        for k in 0..n {
            let w = self.half_width[k].max(1e-7);
            let profile = profile(w, t, self.edge);
            ring_size = profile.len();
            let (c, a, u) = (self.centre[k], self.across[k], self.up[k]);
            let bottom = &self.bottom[k];
            let base_w = self.half_width_full(k);
            for (x, y) in profile {
                let h = height_at(bottom, x / base_w) + gap;
                positions.push(c + a * x + u * (h + y));
            }
        }
        let ring = |k: usize, i: usize| (k * ring_size + i % ring_size) as u32;
        let mut indices = Vec::with_capacity(n * ring_size * 6 + ring_size * 6);
        let spans = if self.closed { n } else { n - 1 };
        for k in 0..spans {
            let next = (k + 1) % n;
            for i in 0..ring_size {
                let (a, b, c, d) = (ring(k, i), ring(k, i + 1), ring(next, i + 1), ring(next, i));
                indices.extend_from_slice(&[a, b, c, a, c, d]);
            }
        }
        if !self.closed {
            for (k, start) in [(0, true), (n - 1, false)] {
                for [p, q, r] in end_cap() {
                    let (p, q, r) = (ring(k, p), ring(k, q), ring(k, r));
                    indices.extend_from_slice(&if start { [p, r, q] } else { [p, q, r] });
                }
            }
        }
        TriMesh::new(positions, indices)
    }

    /// The half width the surface was felt across at station `k`: the strap's own there, before
    /// an end narrowed it.
    fn half_width_full(&self, k: usize) -> f32 {
        let full = self.full_width.get(k).copied().unwrap_or(self.half_width[k]);
        full.max(1e-7)
    }
}

/// The cross-section of a strap `w` half wide and `t` thick with edges `edge` round, as (across,
/// up from the underside), counterclockwise seen from where the strap is going: along the
/// underside from left to right, up the right side, back along the top and down the left side.
/// Every ring of a strap has the same number of points, whatever its width.
pub fn profile(w: f32, t: f32, edge: f32) -> Vec<(f32, f32)> {
    let r = edge.clamp(0.0, 1.0) * (0.5 * t).min(w) * 0.999;
    let mut out = Vec::with_capacity(2 * ACROSS + 4 * CORNER_STEPS);
    let inner = w - r;
    // Underside, left to right, at the places the surface was felt across.
    for j in 0..ACROSS {
        let s = j as f32 / (ACROSS - 1) as f32;
        out.push((-inner + 2.0 * inner * s, 0.0));
    }
    let arc = |out: &mut Vec<(f32, f32)>, cx: f32, cy: f32, from: f32| {
        for step in 1..CORNER_STEPS {
            let angle = from + std::f32::consts::FRAC_PI_2 * step as f32 / CORNER_STEPS as f32;
            out.push((cx + r * angle.cos(), cy + r * angle.sin()));
        }
    };
    use std::f32::consts::{FRAC_PI_2, PI};
    // Bottom right corner, round to the right side.
    arc(&mut out, inner, r, -FRAC_PI_2);
    out.push((w, r));
    out.push((w, t - r));
    arc(&mut out, inner, t - r, 0.0);
    // Top, right to left.
    for j in 0..ACROSS {
        let s = j as f32 / (ACROSS - 1) as f32;
        out.push((inner - 2.0 * inner * s, t));
    }
    arc(&mut out, -inner, t - r, FRAC_PI_2);
    out.push((-w, t - r));
    out.push((-w, r));
    arc(&mut out, -inner, r, PI);
    // A square edge has its corner points twice over; that only makes a sliver no one sees, and
    // keeps every ring the same size.
    out
}

/// The triangles that close an end of a strap, as places in its ring ([`profile`]), wound the way
/// the ring runs. Not a fan from the middle: a strap bent round a narrow form is curved across, and
/// its middle can be outside its own cross-section. A strip from the underside to the top instead,
/// each place below joined to the one above it, and a fan over each rounded side from its corner.
fn end_cap() -> Vec<[usize; 3]> {
    let (a, c) = (ACROSS, CORNER_STEPS - 1);
    let right = a; // The right side's first place: after the underside.
    let top = a + 2 * c + 2; // The top's first place, at the right.
    let left = top + a; // The left side's first place: after the top.
    let size = left + 2 * c + 2;
    let above = |j: usize| top + (a - 1 - j);
    let mut out = Vec::with_capacity(2 * a + 4 * c + 4);
    for j in 0..a - 1 {
        out.push([j, j + 1, above(j + 1)]);
        out.push([j, above(j + 1), above(j)]);
    }
    // The right side, from the underside's last place round to the top's first.
    for i in right..top {
        out.push([a - 1, i, i + 1]);
    }
    // The left side, from the top's last place round to the underside's first.
    for i in left..size - 1 {
        out.push([left - 1, i, i + 1]);
    }
    out.push([left - 1, size - 1, 0]);
    out
}

/// The underside's height at `s` across the strap, from -1 (one edge) to 1 (the other), between
/// the places it was felt at.
fn height_at(bottom: &[f32; ACROSS], s: f32) -> f32 {
    let f = ((s.clamp(-1.0, 1.0) + 1.0) * 0.5) * (ACROSS - 1) as f32;
    let j = (f.floor() as usize).min(ACROSS - 2);
    let t = f - j as f32;
    bottom[j] + (bottom[j + 1] - bottom[j]) * t
}

/// How wide a strap is at `d` half widths from its nearer end, as a share of its full width.
fn tip_share(tip: Tip, d: f32) -> f32 {
    let share = match tip {
        Tip::Square => 1.0,
        Tip::Round => {
            let x = (1.0 - d).max(0.0);
            (1.0 - x * x).max(0.0).sqrt()
        }
        Tip::Point => (d / 1.5).min(1.0),
    };
    share.clamp(MIN_TIP, 1.0)
}

/// The control points of a closed strap with its neighbours either side carried round, so that
/// the curve flows through its first point with no seam: the last point first, then all of them,
/// then the first two again. Segment `k` of the result is segment `k - 1` of the strap; the strap
/// itself is segments 1 to `n`.
fn carried_round(controls: &[ControlPoint]) -> Vec<ControlPoint> {
    let n = controls.len();
    let mut out = Vec::with_capacity(n + 3);
    out.push(controls[n - 1]);
    out.extend_from_slice(controls);
    out.push(controls[0]);
    out.push(controls[1 % n]);
    out
}

/// The tangents at the two ends of a closed strap's stations, which run on into each other.
fn close_tangents(samples: &mut [Sample]) {
    let last = samples.len() - 1;
    if last < 2 {
        return;
    }
    let first = (samples[1].center - samples[last].center).normalize_or(samples[0].tangent_out);
    let end = (samples[0].center - samples[last - 1].center).normalize_or(samples[last].tangent_in);
    (samples[0].tangent_in, samples[0].tangent_out) = (first, first);
    (samples[last].tangent_in, samples[last].tangent_out) = (end, end);
}

/// Repeated 1-2-1 averaging round a loop, `passes` times.
fn smooth_round(values: &mut [f32], passes: usize) {
    let n = values.len();
    if n < 3 {
        return;
    }
    let mut next = values.to_vec();
    for _ in 0..passes {
        for i in 0..n {
            next[i] = (values[(i + n - 1) % n] + values[i] * 2.0 + values[(i + 1) % n]) * 0.25;
        }
        values.copy_from_slice(&next);
    }
}

/// Whether a strap of `settings` with `count` points is closed: it has to have three points to go
/// round anything.
pub fn is_closed(settings: &TubeSettings, count: usize) -> bool {
    settings.closed && count >= 3
}

/// The control points, stations and laid strap of the copy of a strap that `op` maps it to. See
/// `spine::build` for the spaces. Without `ground` (a strap drawn in space) it lies flat, facing
/// the way its points do.
pub fn sample(
    points: &[TubePoint],
    settings: &TubeSettings,
    local_to_frame: &Mat4,
    op: &SymmetryOp,
    ground: Option<Ground>,
) -> (Vec<ControlPoint>, Vec<Sample>, Laid) {
    let normal_matrix = Mat3::from_mat4(local_to_frame.inverse()).transpose();
    let radius = settings.radius.clamp(TubeSettings::MIN_RADIUS, TubeSettings::MAX_RADIUS);
    let mut controls: Vec<ControlPoint> = points
        .iter()
        .map(|p| ControlPoint {
            position: local_to_frame.transform_point3(op.apply_point(p.position)),
            radius: radius * p.size.clamp(TubeSettings::MIN_SIZE, TubeSettings::MAX_SIZE),
            sharp: p.sharp,
        })
        .collect();
    let n = controls.len();
    let closed = is_closed(settings, n);
    let tension = curve::tension_for(settings.smoothness);
    let spacing = SPACING_PER_RADIUS;
    let mut samples = if closed {
        let round = carried_round(&controls);
        let all = curve::resample(&round, tension, spacing, radius * 0.02, MAX_SAMPLES);
        // The strap itself, segment 1 to n of the carried curve; the rest is only there to bend it.
        let mut own: Vec<Sample> = all.into_iter().filter(|s| (1..=n).contains(&s.segment) && !(s.segment == n && s.segment_t >= 0.999)).collect();
        for s in &mut own {
            s.segment -= 1;
            s.corner = false;
        }
        own
    } else {
        curve::resample(&controls, tension, spacing, radius * 0.02, MAX_SAMPLES)
    };
    if samples.len() < 2 {
        return (controls, Vec::new(), Laid::default());
    }
    let mut distance = 0.0;
    for k in 0..samples.len() {
        if k > 0 {
            distance += samples[k].center.distance(samples[k - 1].center);
        }
        samples[k].distance = distance;
    }
    curve::compute_tangents(&mut samples);
    if closed {
        close_tangents(&mut samples);
    }

    // Each station's way up, from the points either side, as a muscle's.
    let mut ups: Vec<Vec3> = points.iter().map(|p| (normal_matrix * op.apply(p.up())).normalize_or_zero()).collect();
    let next_of = |i: usize| if closed { (i + 1) % n } else { (i + 1).min(n - 1) };
    if let Some(ground) = ground.filter(|_| n > 1) {
        for i in (0..n).filter(|&i| points[i].is_on_surface()) {
            let toward = if i + 1 < n || closed {
                controls[next_of(i)].position - controls[i].position
            } else {
                controls[i].position - controls[i - 1].position
            };
            let step = controls[i].radius * muscle::POINT_FEEL;
            ups[i] = muscle::lie_of_form(controls[i].position, ups[i], toward, step, ground);
        }
    }
    // A point floating with no way up of its own takes the one the strap would face from the view.
    let fallback = ups.iter().copied().find(|u| *u != Vec3::ZERO).unwrap_or(Vec3::Y);
    for u in &mut ups {
        if *u == Vec3::ZERO {
            *u = fallback;
        }
    }
    let between = |s: &Sample| {
        let a = s.segment.min(n - 1);
        (a, next_of(a), s.segment_t.clamp(0.0, 1.0))
    };
    let mut up: Vec<Vec3> = samples
        .iter()
        .map(|s| {
            let (a, b, t) = between(s);
            ups[a].lerp(ups[b], t).normalize_or(fallback)
        })
        .collect();
    let held: Vec<f32> = samples
        .iter()
        .map(|s| {
            let (a, b, t) = between(s);
            let on = |i: usize| if points[i].is_on_surface() { 1.0 } else { 0.0 };
            on(a) + (on(b) - on(a)) * t
        })
        .collect();

    if let Some(ground) = ground {
        let reach: Vec<f32> = samples
            .iter()
            .map(|s| {
                let (a, b, _) = between(s);
                radius * 2.0 + controls[a].position.distance(controls[b].position) * 0.5
            })
            .collect();
        muscle::settle(&mut samples, &mut up, &held, &reach, radius, ground);
        let last = n - 1;
        for (i, control) in controls.iter_mut().enumerate().filter(|(i, _)| points[*i].is_on_surface()) {
            let station = samples.iter().find(|s| s.segment == i).or(if i == last && !closed { samples.last() } else { None });
            if let Some(station) = station.filter(|s| (i == last && !closed) || s.segment_t < 0.02) {
                control.position = station.center;
            }
        }
    }
    curve::compute_tangents(&mut samples);
    if closed {
        close_tangents(&mut samples);
    }

    let laid = lay(&mut samples, &up, &held, settings, closed, ground);
    // A point's ring shows how wide the strap is there.
    for (i, control) in controls.iter_mut().enumerate() {
        let station = samples.iter().find(|s| s.segment == i).or(if i == n - 1 { samples.last() } else { None });
        if let Some(station) = station {
            control.radius = station.radius;
        }
    }
    (controls, samples, laid)
}

/// Feels the surface across the strap at every station and draws the strap over it as taut as
/// Tight says.
fn lay(samples: &mut [Sample], up: &[Vec3], held: &[f32], settings: &TubeSettings, closed: bool, ground: Option<Ground>) -> Laid {
    let count = samples.len();
    let thickness = settings.strap_thickness();
    let total = samples[count - 1].distance.max(1e-9);
    let mut laid = Laid {
        thickness,
        edge: settings.edge.clamp(0.0, 1.0),
        gap: settings.gap.clamp(0.0, TubeSettings::MAX_GAP) * thickness,
        closed,
        ..Laid::default()
    };
    let mut full_width = Vec::with_capacity(count);
    for (k, s) in samples.iter_mut().enumerate() {
        let along = (s.tangent_out - up[k] * s.tangent_out.dot(up[k])).normalize_or_zero();
        let across = if along == Vec3::ZERO { up[k].any_orthonormal_vector() } else { up[k].cross(along).normalize() };
        let w = s.radius;
        let shrink = if closed {
            1.0
        } else {
            let d = s.distance.min(total - s.distance) / w.max(1e-9);
            tip_share(settings.tip, d)
        };
        laid.centre.push(s.center);
        laid.across.push(across);
        laid.up.push(up[k]);
        laid.half_width.push(w * shrink);
        full_width.push(w);
        s.up = up[k];
        s.flat = 0.5 * thickness / w.max(1e-9);
        s.rest = 0.0;
        s.under = 0.0;
    }

    // The surface across the strap. A station off the model (or with no model to feel) rests flat
    // on its own centre line.
    let mut bottom: Vec<[f32; ACROSS]> = vec![[0.0; ACROSS]; count];
    if let Some(ground) = ground {
        let radius = settings.radius.clamp(TubeSettings::MIN_RADIUS, TubeSettings::MAX_RADIUS);
        for k in 0..count {
            if held[k] <= 0.0 {
                continue;
            }
            let (c, a, u, w) = (laid.centre[k], laid.across[k], laid.up[k], full_width[k]);
            let reach = radius * 2.0 + thickness;
            let mut felt = [None; ACROSS];
            for (j, slot) in felt.iter_mut().enumerate() {
                let s = -1.0 + 2.0 * j as f32 / (ACROSS - 1) as f32;
                *slot = ground(c + a * (s * w), u, reach).map(|(p, _)| (p - c).dot(u));
            }
            // A place that found nothing (past the edge of the model) takes its nearest neighbour's.
            for j in 0..ACROSS {
                bottom[k][j] = (0..ACROSS)
                    .filter_map(|i| felt[i].map(|h| (i.abs_diff(j), h)))
                    .min_by_key(|(d, _)| *d)
                    .map_or(0.0, |(_, h)| h * held[k]);
            }
        }
    }

    // Leather does not follow the pores of a sculpt: a light smoothing along the strap, never
    // letting the underside go below what it rests on.
    let raw = bottom.clone();
    for j in 0..ACROSS {
        let mut column: Vec<f32> = bottom.iter().map(|b| b[j]).collect();
        if closed {
            smooth_round(&mut column, 2);
        } else {
            muscle::smooth(&mut column, 2);
        }
        for k in 0..count {
            bottom[k][j] = column[k].max(raw[k][j]);
        }
    }

    // Stiff across: the underside bridges a groove under it from edge to edge.
    for b in &mut bottom {
        let positions: Vec<f32> = (0..ACROSS).map(|j| -1.0 + 2.0 * j as f32 / (ACROSS - 1) as f32).collect();
        taut_line(b, &positions);
    }

    // Taut along: each place is pushed out until it is no lower than the middle of its neighbours.
    let tight = settings.tight.clamp(0.0, 1.0);
    if tight > 0.0 && count >= 3 {
        let loose = bottom.clone();
        let mut place: Vec<[Vec3; ACROSS]> = (0..count)
            .map(|k| {
                let mut row = [Vec3::ZERO; ACROSS];
                for (j, p) in row.iter_mut().enumerate() {
                    let s = -1.0 + 2.0 * j as f32 / (ACROSS - 1) as f32;
                    *p = laid.centre[k] + laid.across[k] * (s * full_width[k]) + laid.up[k] * bottom[k][j];
                }
                row
            })
            .collect();
        let settled = TAUT_SETTLED * thickness;
        let inner = if closed { 0..count } else { 1..count - 1 };
        for _ in 0..MAX_TAUT_PASSES {
            let mut most = 0.0f32;
            for k in inner.clone() {
                let (before, after) = ((k + count - 1) % count, (k + 1) % count);
                for j in 0..ACROSS {
                    let middle = (place[before][j] + place[after][j]) * 0.5;
                    let out = (middle - place[k][j]).dot(laid.up[k]);
                    if out > 0.0 {
                        place[k][j] += laid.up[k] * out;
                        bottom[k][j] += out;
                        most = most.max(out);
                    }
                }
            }
            if most < settled {
                break;
            }
        }
        for k in 0..count {
            for j in 0..ACROSS {
                bottom[k][j] = loose[k][j] + (bottom[k][j] - loose[k][j]) * tight;
            }
        }
    }
    laid.bottom = bottom;
    laid.full_width = full_width;
    laid
}

/// Lifts the inside of a line of heights at `positions` until none is below the straight line
/// between its neighbours: a ruler laid across them. The two ends stay.
fn taut_line(heights: &mut [f32; ACROSS], positions: &[f32]) {
    // The upper hull of the points, then each point raised onto it.
    let mut hull: Vec<usize> = Vec::with_capacity(ACROSS);
    for j in 0..ACROSS {
        while hull.len() >= 2 {
            let (a, b) = (hull[hull.len() - 2], hull[hull.len() - 1]);
            let cross = (positions[b] - positions[a]) * (heights[j] - heights[a]) - (heights[b] - heights[a]) * (positions[j] - positions[a]);
            if cross >= 0.0 {
                hull.pop();
            } else {
                break;
            }
        }
        hull.push(j);
    }
    for pair in hull.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        for j in a + 1..b {
            let t = (positions[j] - positions[a]) / (positions[b] - positions[a]);
            heights[j] = heights[j].max(heights[a] + (heights[b] - heights[a]) * t);
        }
    }
}

/// Whether two copies of a strap are the same strap: a belt round a body that is mirrored across
/// its middle is its own mirror image, and two of it would be one inside the other. Every station
/// of `copy` is within a sliver of `of`'s line.
pub fn same_strap(copy: &[Sample], of: &[Sample], radius: f32) -> bool {
    if copy.is_empty() || of.is_empty() {
        return false;
    }
    let near = (radius * 0.05).powi(2);
    let step = (copy.len() / 64).max(1);
    copy.iter().step_by(step).all(|s| {
        of.windows(2).any(|w| {
            let (a, b) = (w[0].center, w[1].center);
            let ab = b - a;
            let t = if ab.length_squared() > 1e-12 { ((s.center - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
            (a + ab * t).distance_squared(s.center) <= near
        })
    })
}
