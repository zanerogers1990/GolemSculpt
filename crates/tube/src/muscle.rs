//! A muscle's centre line and cross-sections: a tube's points read another way.
//!
//! Three things make a muscle of a tube. Its width is a profile along its whole length (thin at both
//! ends, full at the belly) rather than a ramp between its points. Its cross-section is wider than it is
//! deep, thin along the way up off the surface. And between two points on the surface the centre line is
//! put back on the surface at every station, where a tube only touches it at its points: a muscle drawn
//! with two points over a shoulder lies over the shoulder.
//!
//! The surface under a station is found by the caller (`Ground`), so this stays plain geometry.

use std::f32::consts::PI;

use glam::{Mat3, Mat4, Vec3};
use sculpt_geom::SymmetryOp;

use crate::curve;
use crate::spine::{MAX_SAMPLES, MIN_TAPER_FRACTION};
use crate::types::{ControlPoint, Sample, TubePoint, TubeSettings};

/// Station spacing per unit of the muscle's radius: finer than a tube's, because the width changes all
/// the way along and the stations are what follow the surface.
pub const SPACING_PER_RADIUS: f32 = 0.15;

/// The surface is read through a window about this many radii long, so the muscle lies over the form
/// under it and not over every pore and wrinkle of it.
const SMOOTH_RADII: f32 = 0.6;

/// The surface under a place: given a point, the way up there and how far to look either way, the point
/// of the surface on that line and its normal. All in the frame the muscle is built in.
pub type Ground<'a> = &'a dyn Fn(Vec3, Vec3, f32) -> Option<(Vec3, Vec3)>;

/// Width along a muscle, as a fraction of its radius, at `t` from 0 (one end) to 1 (the other).
pub fn profile(settings: &TubeSettings, t: f32) -> f32 {
    let end = (1.0 - settings.taper.clamp(0.0, 1.0)).max(MIN_TAPER_FRACTION);
    let belly = settings.belly.clamp(0.1, 0.9);
    let t = t.clamp(0.0, 1.0);
    // The belly is where the half-wave peaks.
    let u = if t < belly { 0.5 * t / belly } else { 0.5 + 0.5 * (t - belly) / (1.0 - belly) };
    let power = 1.6 + (0.3 - 1.6) * settings.swell.clamp(0.0, 1.0);
    end + (1.0 - end) * (PI * u).sin().max(0.0).powf(power)
}

/// How far a station's centre stands off the surface: its half depth when it rests on top, nothing
/// when it is sunk to its widest.
pub fn lift(settings: &TubeSettings, radius: f32, flat: f32) -> f32 {
    radius * flat * (1.0 - 2.0 * settings.depth.clamp(0.0, TubeSettings::MAX_DEPTH))
}

/// Repeated 1-2-1 averaging, `passes` times. The two ends stay exactly where they are: a muscle's ends
/// are its points, and they are on the surface.
fn smooth<T>(values: &mut [T], passes: usize)
where
    T: Copy + std::ops::Add<Output = T> + std::ops::Mul<f32, Output = T>,
{
    let n = values.len();
    if n < 3 {
        return;
    }
    let mut next = values.to_vec();
    for _ in 0..passes {
        for i in 1..n - 1 {
            next[i] = (values[i - 1] + values[i] * 2.0 + values[i + 1]) * 0.25;
        }
        values.copy_from_slice(&next);
    }
}

/// The control points and stations of the copy of a muscle that `op` maps it to. See `spine::build`
/// for the spaces.
pub fn sample(
    points: &[TubePoint],
    settings: &TubeSettings,
    local_to_frame: &Mat4,
    op: &SymmetryOp,
    ground: Option<Ground>,
) -> (Vec<ControlPoint>, Vec<Sample>) {
    let normal_matrix = Mat3::from_mat4(local_to_frame.inverse()).transpose();
    let radius = settings.radius.clamp(TubeSettings::MIN_RADIUS, TubeSettings::MAX_RADIUS);
    // The points themselves: a muscle's handles are on the surface, and the lift is the stations'.
    let mut controls: Vec<ControlPoint> = points
        .iter()
        .map(|p| ControlPoint {
            position: local_to_frame.transform_point3(op.apply_point(p.position)),
            radius: radius * p.size.clamp(TubeSettings::MIN_SIZE, TubeSettings::MAX_SIZE),
            sharp: p.sharp,
        })
        .collect();
    let tension = curve::tension_for(settings.smoothness);
    let mut samples = curve::resample(&controls, tension, SPACING_PER_RADIUS, radius * 0.02, MAX_SAMPLES);
    if samples.is_empty() {
        return (controls, samples);
    }

    // Each station's way up and how firmly it is held to the surface, from the points either side.
    let mut ups: Vec<Vec3> = points.iter().map(|p| (normal_matrix * op.apply(p.up())).normalize_or_zero()).collect();
    let last = points.len() - 1;
    // A point on the surface faces the way the form under it does, at the muscle's own scale: the face
    // it was put down on is one of a dense sculpt's millions, tilted every which way by the detail, and
    // the stations either side are felt for along this. Dragged across the faces, a point would
    // otherwise swing its whole muscle about with every face it crossed.
    if let Some(ground) = ground.filter(|_| last > 0) {
        for i in (0..=last).filter(|&i| points[i].is_on_surface()) {
            let toward = if i < last {
                controls[i + 1].position - controls[i].position
            } else {
                controls[i].position - controls[i - 1].position
            };
            let step = controls[i].radius * POINT_FEEL;
            ups[i] = lie_of_form(controls[i].position, ups[i], toward, step, ground);
        }
    }
    let between = |s: &Sample| (s.segment.min(last), (s.segment + 1).min(last), s.segment_t.clamp(0.0, 1.0));
    let mut up: Vec<Vec3> = samples
        .iter()
        .map(|s| {
            let (a, b, t) = between(s);
            ups[a].lerp(ups[b], t).normalize_or_zero()
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

    let total = samples[samples.len() - 1].distance.max(1e-9);
    for s in &mut samples {
        s.radius *= profile(settings, s.distance / total);
    }
    let mut deep: Vec<Option<f32>> = vec![None; samples.len()];
    if let Some(ground) = ground {
        // Far enough to find a surface that the run between two points has cut under or passed over.
        let reach: Vec<f32> = samples
            .iter()
            .map(|s| {
                let (a, b, _) = between(s);
                radius * 2.0 + controls[a].position.distance(controls[b].position) * 0.5
            })
            .collect();
        deep = settle(&mut samples, &mut up, &held, &reach, radius, ground);
        // A point on the surface is shown where the surface is now. The model may have been sculpted
        // since the point was put down: the muscle has followed it, and so does its handle.
        for (i, control) in controls.iter_mut().enumerate().filter(|(i, _)| points[*i].is_on_surface()) {
            let station = if i == last { samples.last() } else { samples.iter().find(|s| s.segment == i) };
            if let Some(station) = station.filter(|s| i == last || s.segment_t < 0.02) {
                control.position = station.center;
            }
        }
    }

    // A point's ring shows how wide the muscle is there, which is its own size narrowed by the profile.
    for (i, control) in controls.iter_mut().enumerate() {
        let station = if i == last { samples.last() } else { samples.iter().find(|s| s.segment == i) };
        if let Some(station) = station.filter(|s| i == last || s.segment_t < 0.02) {
            control.radius = station.radius;
        }
    }

    let flatten = settings.flatten.clamp(TubeSettings::MIN_FLATTEN, 1.0);
    for (k, s) in samples.iter_mut().enumerate() {
        s.flat = if up[k] == Vec3::ZERO { 1.0 } else { flatten };
        s.up = up[k];
        let rest = lift(settings, s.radius, s.flat) * held[k];
        s.center += up[k] * rest;
        // Filled under down into the model wherever the model was found under it: over a groove or a
        // hollow it bridges, past the sides of a rounded form it lies along, under a point lifted off
        // the surface. Never more than a few radii, and a part floating farther off than that is left
        // floating.
        let most = MAX_FILL_RADII * radius;
        let fill_to = deep[k].filter(|_| s.up != Vec3::ZERO).and_then(|d| match held[k] > 0.0 {
            true => Some(d.min(most)),
            false => (d <= most).then_some(d),
        });
        s.rest = rest;
        s.under = fill_to.map_or(0.0, |d| rest + d + FILL_MARGIN_RADII * radius);
    }
    curve::compute_tangents(&mut samples);
    (controls, samples)
}

/// How far out from a station the surface is felt for, as a fraction of the muscle's half width there.
const FEEL: f32 = 0.5;

/// How far out the surface is felt for the fill under a muscle, as a fraction of its half width: next
/// to its edge, where a rounded form under it has dropped away most.
const FILL_FEEL: f32 = 0.95;

/// How far a muscle is filled under, in radii: past the deepest the model was found under it by this
/// much, so the fill goes well into the model however the merge's lattice falls; and down to the model
/// from at most this far off it.
const FILL_MARGIN_RADII: f32 = 0.3;
const MAX_FILL_RADII: f32 = 2.5;

/// How far out from a point on the surface the form under it is felt for, as a fraction of the
/// muscle's half width there: at its edge, so the way up at a point is the lie of what the muscle
/// covers, not of the wrinkle the point is on.
const POINT_FEEL: f32 = 1.0;

/// How far the five heights a station feels have to differ before it rests on the highest of them, as
/// a fraction of the muscle's radius. Detail finer than this is averaged; a ridge taller than this is
/// lain over.
const SOFT_RADII: f32 = 0.08;

/// What a station rests on, of the heights it felt: a smooth maximum. Over a ridge or a bump much
/// taller than `soft` it is the highest of them, so a muscle lies over ridges and not down in the gaps
/// between them; where they differ by less (the pores and wrinkles of a sculpt) it is about their mean.
/// A plain maximum picks whichever little peak the five places happen to land on, and as the stations
/// slide along under a moving point it picks another every frame.
fn rest_on(heights: &[f32], soft: f32) -> f32 {
    let highest = heights.iter().copied().fold(f32::MIN, f32::max);
    if !(soft > 0.0) {
        return highest;
    }
    let mean = heights.iter().map(|h| ((h - highest) / soft).exp()).sum::<f32>() / heights.len() as f32;
    highest + soft * mean.ln()
}

/// The way up at a point on the surface: the tilt of the surface felt at four places `step` round it,
/// along the muscle (`toward`) and across it. Felt twice, the second time along the first answer, so
/// the face the point was put down on only picks the directions the first four are felt in.
fn lie_of_form(at: Vec3, up: Vec3, toward: Vec3, step: f32, ground: Ground) -> Vec3 {
    let mut normal = up;
    if step <= 0.0 || up == Vec3::ZERO {
        return up;
    }
    for _ in 0..2 {
        let along = (toward - normal * toward.dot(normal)).normalize_or_zero();
        if along == Vec3::ZERO {
            break;
        }
        let across = normal.cross(along);
        let feel = |offset: Vec3| ground(at + offset, normal, step * 2.0).map(|(p, _)| p);
        let (Some(right), Some(left), Some(ahead), Some(behind)) =
            (feel(across * step), feel(-across * step), feel(along * step), feel(-along * step))
        else {
            break;
        };
        let tilt = (ahead - behind).cross(right - left).normalize_or_zero();
        // Only a tilt, not a wall: a point at an edge keeps the way up it had.
        if tilt.dot(up) < 0.5 {
            break;
        }
        normal = tilt;
    }
    normal
}

/// Repeated 1-2-1 averaging, `passes` times, with the ends free: each end is averaged with its one
/// neighbour as though the run went on past it the same way. For the way up, which is not pinned at a
/// muscle's points the way its height is.
fn smooth_free<T>(values: &mut [T], passes: usize)
where
    T: Copy + std::ops::Add<Output = T> + std::ops::Mul<f32, Output = T>,
{
    let n = values.len();
    if n < 2 {
        return;
    }
    let mut next = values.to_vec();
    for _ in 0..passes {
        for i in 0..n {
            let before = values[i.saturating_sub(1)];
            let after = values[(i + 1).min(n - 1)];
            let (before, after) = if i == 0 { (after, after) } else if i == n - 1 { (before, before) } else { (before, after) };
            next[i] = (before + values[i] * 2.0 + after) * 0.25;
        }
        values.copy_from_slice(&next);
    }
}

/// Puts the stations between points on the surface back onto it, and turns their way up to its normal.
///
/// The surface is felt at the muscle's own scale: under the station and at four places round it, half
/// way to the muscle's edge. The station rests on a smooth maximum of them (see [`rest_on`]), as a
/// muscle lies over ridges and not down in the gaps between them, and its way up is the tilt of the
/// four, which is the lie of the form there whatever the detail on it is doing.
///
/// Returns, for each station the model was found under, how far below where it rests (along its way
/// up as it was) the model is at the deepest under its footprint, near enough: what the muscle is
/// filled under by. A station of a muscle that is laid on the model somewhere but is itself off it (a
/// point lifted, or between two such) only looks straight down.
fn settle(samples: &mut [Sample], up: &mut [Vec3], held: &[f32], reach: &[f32], radius: f32, ground: Ground) -> Vec<Option<f32>> {
    let n = samples.len();
    let mut rise: Vec<Option<f32>> = vec![None; n];
    let mut lowest: Vec<Option<f32>> = vec![None; n];
    let mut normal: Vec<Vec3> = up.to_vec();
    let soft = SOFT_RADII * radius;
    let laid = held.iter().any(|&h| h > 0.0);
    for k in 0..n {
        if up[k] == Vec3::ZERO {
            continue;
        }
        let (centre, way_up) = (samples[k].center, up[k]);
        if held[k] <= 0.0 {
            if laid {
                lowest[k] = ground(centre, way_up, reach[k]).map(|(at, _)| (at - centre).dot(way_up));
            }
            continue;
        }
        let Some((at, found)) = ground(centre, way_up, reach[k]) else { continue };
        let mut heights = vec![(at - centre).dot(way_up)];
        normal[k] = found.normalize_or(way_up);

        let along = (samples[k].tangent_out - way_up * samples[k].tangent_out.dot(way_up)).normalize_or_zero();
        let across = way_up.cross(along);
        if along == Vec3::ZERO {
            rise[k] = Some(heights[0]);
            continue;
        }
        let step = samples[k].radius * FEEL;
        let feel = |offset: Vec3| ground(centre + offset, way_up, reach[k]).map(|(at, _)| at);
        let round = [feel(across * step), feel(-across * step), feel(along * step), feel(-along * step)];
        heights.extend(round.iter().flatten().map(|at| (*at - centre).dot(way_up)));
        let edge = samples[k].radius * FILL_FEEL;
        let edges = [feel(across * edge), feel(-across * edge)];
        let below = heights.iter().copied().chain(edges.iter().flatten().map(|at| (*at - centre).dot(way_up)));
        lowest[k] = below.reduce(f32::min);
        if let [Some(right), Some(left), Some(ahead), Some(behind)] = round {
            let tilt = (ahead - behind).cross(right - left).normalize_or_zero();
            // Only a tilt, not a wall: a station beside a cliff keeps the way up it had.
            let tilt = if tilt.dot(way_up) < 0.0 { -tilt } else { tilt };
            if tilt.dot(way_up) > 0.5 {
                normal[k] = tilt;
            }
        }
        rise[k] = Some(rest_on(&heights, soft));
    }
    // A station that found nothing takes its neighbour's answer.
    let mut filled: Vec<f32> = Vec::with_capacity(n);
    let mut carry = rise.iter().flatten().next().copied().unwrap_or(0.0);
    for r in &rise {
        carry = r.unwrap_or(carry);
        filled.push(carry);
    }
    let spacing = (radius * SPACING_PER_RADIUS).max(1e-9);
    let sigma = SMOOTH_RADII * radius / spacing;
    let passes = (2.0 * sigma * sigma).round().clamp(1.0, 200.0) as usize;
    smooth(&mut filled, passes);
    // The two ends are where the muscle is thinnest, and felt over the smallest patch: the way up there
    // is the run's, not theirs alone.
    smooth_free(&mut normal, passes);
    for k in 0..n {
        samples[k].center += up[k] * (filled[k] * held[k]);
        up[k] = up[k].lerp(normal[k].normalize_or(up[k]), held[k]).normalize_or(up[k]);
    }
    // How deep under where each station now rests, and the deepest within a radius along the muscle
    // either way: the fill between two stations is straight, and a dip between them is as deep.
    let deep: Vec<Option<f32>> = (0..n).map(|k| lowest[k].map(|low| (filled[k] * held[k] - low).max(0.0))).collect();
    let reach_along = (1.0 / SPACING_PER_RADIUS).ceil() as usize;
    (0..n)
        .map(|k| {
            let window = k.saturating_sub(reach_along)..(k + reach_along + 1).min(n);
            deep[k].map(|own| deep[window].iter().flatten().copied().fold(own, f32::max))
        })
        .collect()
}
