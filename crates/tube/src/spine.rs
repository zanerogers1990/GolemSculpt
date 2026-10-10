//! Turns a tube's editable points (in its anchor's space, surface-relative,
//! with per-point sizes) into the control points the curve is built from,
//! for one symmetric copy of the tube.
//!
//! Everything the curve and the field must not have to know about happens
//! here: the symmetry operation, the anchor's scale, lifting surface points
//! off the surface, and taper.

use glam::{Mat3, Mat4, Vec3};
use sculpt_geom::SymmetryOp;

use crate::curve;
use crate::types::{ControlPoint, Sample, TubePoint, TubeSettings};

/// A point placed on the surface holds the centre line this many radii off
/// it, so half the tube is sunk into the model: the overlap a merge needs to
/// fuse without a gap, and what reads as "attached" rather than "resting on"
/// before it is merged.
pub const SURFACE_LIFT: f32 = 0.5;

/// Taper never narrows a tube below this fraction of its radius: a true
/// zero-radius tip has no surface to build, and the dome that closes it
/// would vanish.
pub const MIN_TAPER_FRACTION: f32 = 0.03;

/// Station spacing along the tube, per unit of local radius. Each pair of
/// stations is one round cone of the field, so this only has to be fine
/// enough that the chain of straight cones follows the curve closely.
pub const SPACING_PER_RADIUS: f32 = 0.25;

/// Most stations per tube copy, whatever its length.
pub const MAX_SAMPLES: usize = 4000;

/// The control points of the copy of the tube that `op` maps it to.
///
/// `op` works in the anchor's own space, like every symmetry operation; pass
/// `SymmetryOp::IDENTITY` for the tube itself. `local_to_frame` takes the
/// anchor's space to the frame the tube is built in (the anchor's position
/// and rotation without its scale, see `live`).
pub fn build(points: &[TubePoint], settings: &TubeSettings, local_to_frame: &Mat4, op: &SymmetryOp) -> Vec<ControlPoint> {
    // Normals go through the inverse transpose, so a non-uniformly scaled
    // anchor still lifts its points straight off its visible surface.
    let normal_matrix = Mat3::from_mat4(local_to_frame.inverse()).transpose();
    let placed: Vec<(Vec3, Vec3)> = points
        .iter()
        .map(|p| {
            let position = local_to_frame.transform_point3(op.apply_point(p.position));
            let normal =
                if p.is_on_surface() { (normal_matrix * op.apply(p.surface_normal)).normalize_or_zero() } else { Vec3::ZERO };
            (position, normal)
        })
        .collect();

    // Taper runs along the control polygon's length: close enough to the
    // curve's own for a thickness ramp, and it keeps the lift below exact for
    // surface points (the lift has to know the tapered radius, and the curve
    // does not exist yet).
    let total: f32 = placed.windows(2).map(|w| w[0].0.distance(w[1].0)).sum();
    let radius = settings.radius.clamp(TubeSettings::MIN_RADIUS, TubeSettings::MAX_RADIUS);
    let taper = settings.taper.clamp(0.0, 1.0);
    let mut run = 0.0;
    let mut output = Vec::with_capacity(points.len());
    for (i, point) in points.iter().enumerate() {
        if i > 0 {
            run += placed[i].0.distance(placed[i - 1].0);
        }
        let mut t = if total > 1e-9 { run / total } else { 0.0 };
        if settings.flip_taper {
            t = 1.0 - t;
        }
        let factor = (1.0 - taper * t).max(MIN_TAPER_FRACTION);
        let size = point.size.clamp(TubeSettings::MIN_SIZE, TubeSettings::MAX_SIZE);
        let r = radius * size * factor;
        let (position, normal) = placed[i];
        output.push(ControlPoint { position: position + normal * (r * SURFACE_LIFT), radius: r, sharp: point.sharp });
    }
    output
}

/// Builds and resamples one copy in a single call: what the preview, the
/// picking and the merge all want. The stations are empty when the copy has
/// no curve yet (fewer than two distinct points).
pub fn sample(
    points: &[TubePoint],
    settings: &TubeSettings,
    local_to_frame: &Mat4,
    op: &SymmetryOp,
) -> (Vec<ControlPoint>, Vec<Sample>) {
    let controls = build(points, settings, local_to_frame, op);
    let min_spacing = (settings.radius * MIN_TAPER_FRACTION).max(1e-5);
    let tension = curve::tension_for(settings.smoothness);
    let samples = curve::resample(&controls, tension, SPACING_PER_RADIUS, min_spacing, MAX_SAMPLES);
    (controls, samples)
}
