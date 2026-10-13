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
    let ups: Vec<Vec3> = points.iter().map(|p| (normal_matrix * op.apply(p.up())).normalize_or_zero()).collect();
    let last = points.len() - 1;
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
    if let Some(ground) = ground {
        // Far enough to find a surface that the run between two points has cut under or passed over.
        let reach: Vec<f32> = samples
            .iter()
            .map(|s| {
                let (a, b, _) = between(s);
                radius * 2.0 + controls[a].position.distance(controls[b].position) * 0.5
            })
            .collect();
        settle(&mut samples, &mut up, &held, &reach, radius, ground);
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
        s.center += up[k] * (lift(settings, s.radius, s.flat) * held[k]);
    }
    curve::compute_tangents(&mut samples);
    (controls, samples)
}

/// How far out from a station the surface is felt for, as a fraction of the muscle's half width there.
const FEEL: f32 = 0.5;

/// Puts the stations between points on the surface back onto it, and turns their way up to its normal.
///
/// The surface is felt at the muscle's own scale: under the station and at four places round it, half
/// way to the muscle's edge. The station rests on the highest of them, as a muscle lies over ridges
/// and not down in the gaps between them, and its way up is the tilt of the four, which is the lie of
/// the form there whatever the detail on it is doing.
fn settle(samples: &mut [Sample], up: &mut [Vec3], held: &[f32], reach: &[f32], radius: f32, ground: Ground) {
    let n = samples.len();
    let mut rise: Vec<Option<f32>> = vec![None; n];
    let mut normal: Vec<Vec3> = up.to_vec();
    for k in 0..n {
        if held[k] <= 0.0 || up[k] == Vec3::ZERO {
            continue;
        }
        let (centre, way_up) = (samples[k].center, up[k]);
        let Some((at, found)) = ground(centre, way_up, reach[k]) else { continue };
        let mut highest = (at - centre).dot(way_up);
        normal[k] = found.normalize_or(way_up);

        let along = (samples[k].tangent_out - way_up * samples[k].tangent_out.dot(way_up)).normalize_or_zero();
        let across = way_up.cross(along);
        if along == Vec3::ZERO {
            rise[k] = Some(highest);
            continue;
        }
        let step = samples[k].radius * FEEL;
        let feel = |offset: Vec3| ground(centre + offset, way_up, reach[k]).map(|(at, _)| at);
        let round = [feel(across * step), feel(-across * step), feel(along * step), feel(-along * step)];
        for at in round.iter().flatten() {
            highest = highest.max((*at - centre).dot(way_up));
        }
        if let [Some(right), Some(left), Some(ahead), Some(behind)] = round {
            let tilt = (ahead - behind).cross(right - left).normalize_or_zero();
            // Only a tilt, not a wall: a station beside a cliff keeps the way up it had.
            let tilt = if tilt.dot(way_up) < 0.0 { -tilt } else { tilt };
            if tilt.dot(way_up) > 0.5 {
                normal[k] = tilt;
            }
        }
        rise[k] = Some(highest);
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
    smooth(&mut normal, passes);
    for k in 0..n {
        samples[k].center += up[k] * (filled[k] * held[k]);
        up[k] = up[k].lerp(normal[k].normalize_or(up[k]), held[k]).normalize_or(up[k]);
    }
}
