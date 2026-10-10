//! Curve, spine, field and merge against answers that can be worked out.

use std::f64::consts::PI;

use glam::{Mat4, Vec3};
use sculpt_geom::{SymmetryOp, TriMesh};

use crate::curve;
use crate::field::{FAR, MAX_VOXEL_SAMPLES, TubeField};
use crate::merge::{self, MergePlan, Stage};
use crate::spine;
use crate::testing::{assert_closed, field_of, icosphere, samples_of, shells, sphere_volume, unmatched_across};
use crate::types::{CapStyle, ControlPoint, TubePoint, TubeSettings};

fn settings(radius: f32) -> TubeSettings {
    TubeSettings { radius, ..TubeSettings::default() }
}

fn free(points: &[[f32; 3]]) -> Vec<TubePoint> {
    points.iter().map(|&p| TubePoint::free(Vec3::from(p))).collect()
}

fn mesh_of(field: &TubeField, cell: f32, mirror: [bool; 3]) -> TriMesh {
    field.voxelize(cell, mirror, MAX_VOXEL_SAMPLES).unwrap().surface.into_mesh()
}

fn controls(points: &[([f32; 3], f32)]) -> Vec<ControlPoint> {
    points.iter().map(|&(p, r)| ControlPoint::new(Vec3::from(p), r)).collect()
}

// ---- curve -------------------------------------------------------------------

#[test]
fn the_curve_passes_through_its_points_at_any_smoothness() {
    let points = controls(&[([0.0, 0.0, 0.0], 0.1), ([1.0, 0.5, 0.0], 0.2), ([1.5, 0.5, 1.0], 0.1), ([3.0, -1.0, 0.5], 0.3)]);
    for smoothness in [0.0, 0.3, 0.6, 1.0] {
        let tension = curve::tension_for(smoothness);
        for segment in 0..curve::segment_count(&points) {
            assert!(curve::evaluate(&points, tension, segment, 0.0).distance(points[segment].position) < 1e-5);
            assert!(curve::evaluate(&points, tension, segment, 1.0).distance(points[segment + 1].position) < 1e-5);
            assert!((curve::evaluate_radius(&points, segment, 0.0) - points[segment].radius).abs() < 1e-6);
            assert!((curve::evaluate_radius(&points, segment, 1.0) - points[segment + 1].radius).abs() < 1e-6);
        }
    }
    assert_eq!(curve::tension_for(7.0), curve::MAX_TENSION);
    assert_eq!(curve::tension_for(-1.0), 0.0);
}

#[test]
fn at_zero_smoothness_the_curve_is_straight_runs() {
    let points = controls(&[([0.0, 0.0, 0.0], 0.1), ([1.0, 0.0, 0.0], 0.1), ([1.0, 1.0, 0.0], 0.1)]);
    for t in [0.25, 0.5, 0.75] {
        let p = curve::evaluate(&points, 0.0, 1, t);
        assert!((p.x - 1.0).abs() < 1e-6 && p.z.abs() < 1e-6, "{p}");
    }
    // Every point is then a span boundary, so the kink is a station.
    let samples = curve::resample(&points, 0.0, 0.25, 1e-4, 4000);
    assert!(samples.iter().any(|s| s.corner && s.center.distance(Vec3::X) < 1e-6));
}

#[test]
fn the_radius_swells_between_points_without_overshooting() {
    let points = controls(&[([0.0, 0.0, 0.0], 0.1), ([1.0, 0.0, 0.0], 0.5), ([2.0, 0.0, 0.0], 0.02), ([3.0, 0.0, 0.0], 0.3)]);
    for segment in 0..3 {
        let (low, high) = (points[segment].radius.min(points[segment + 1].radius), points[segment].radius.max(points[segment + 1].radius));
        let mut last = points[segment].radius;
        let rising = points[segment + 1].radius > last;
        for step in 0..=100 {
            let r = curve::evaluate_radius(&points, segment, step as f32 / 100.0);
            assert!(r >= low - 1e-6 && r <= high + 1e-6, "segment {segment}: {r} outside {low}..{high}");
            assert!(if rising { r >= last - 1e-6 } else { r <= last + 1e-6 }, "segment {segment} is not monotone");
            last = r;
        }
    }
}

#[test]
fn stations_are_spaced_by_the_local_radius_and_carry_their_place_on_the_curve() {
    let points = controls(&[([0.0, 0.0, 0.0], 0.2), ([2.0, 0.0, 0.0], 0.2), ([4.0, 0.0, 0.0], 0.05)]);
    let samples = curve::resample(&points, curve::tension_for(0.6), 0.25, 1e-4, 4000);
    assert!(samples.first().unwrap().center.distance(Vec3::ZERO) < 1e-6);
    assert!(samples.last().unwrap().center.distance(Vec3::new(4.0, 0.0, 0.0)) < 1e-5);
    assert!((samples.last().unwrap().distance - 4.0).abs() < 1e-3, "arc length {}", samples.last().unwrap().distance);
    for pair in samples.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        let gap = a.center.distance(b.center);
        let radius = 0.5 * (a.radius + b.radius);
        assert!(gap > 0.15 * radius && gap < 0.4 * radius, "stations {gap} apart where the tube is {radius} thick");
        assert!(b.distance > a.distance);
        assert!(b.segment >= a.segment && (0.0..=1.0).contains(&b.segment_t));
        assert!((a.tangent_out.length() - 1.0).abs() < 1e-4 && a.tangent_out.dot(Vec3::X) > 0.99);
    }
    // The thin end has more stations per unit length than the thick one.
    let thick = samples.iter().filter(|s| s.center.x < 1.0).count();
    let thin = samples.iter().filter(|s| s.center.x > 3.0).count();
    assert!(thin > thick * 2, "{thin} thin, {thick} thick");
}

#[test]
fn a_sharp_point_is_a_corner_station_with_two_tangents() {
    let mut points = controls(&[([0.0, 0.0, 0.0], 0.1), ([1.0, 0.0, 0.0], 0.1), ([1.0, 1.0, 0.0], 0.1)]);
    points[1].sharp = true;
    let samples = curve::resample(&points, curve::tension_for(0.6), 0.25, 1e-4, 4000);
    let corners: Vec<_> = samples.iter().filter(|s| s.corner).collect();
    assert_eq!(corners.len(), 1);
    let corner = corners[0];
    assert!(corner.center.distance(Vec3::X) < 1e-6);
    assert!(corner.tangent_in.dot(Vec3::X) > 0.999 && corner.tangent_out.dot(Vec3::Y) > 0.999, "{corner:?}");
    // The runs either side are straight: a sharp neighbour gives a free end
    // nothing to curve toward.
    assert!(samples.iter().all(|s| s.center.y.abs() < 1e-6 || (s.center.x - 1.0).abs() < 1e-6));
}

#[test]
fn the_fewest_points_there_can_be() {
    let one = controls(&[([0.0, 0.0, 0.0], 0.1)]);
    assert!(curve::resample(&one, 1.0, 0.25, 1e-4, 4000).is_empty());
    assert!(curve::resample(&[], 1.0, 0.25, 1e-4, 4000).is_empty());
    assert_eq!(curve::segment_count(&[]), 0);
    assert_eq!(curve::segment_count(&one), 0);
    // Points all in one place have no direction between them.
    let stacked = controls(&[([1.0, 1.0, 1.0], 0.1), ([1.0, 1.0, 1.0], 0.3), ([1.0, 1.0, 1.0], 0.2)]);
    assert!(curve::resample(&stacked, 1.0, 0.25, 1e-4, 4000).is_empty());
    // A repeated point is dropped, and the stations still name the
    // artist's own points.
    let repeated = controls(&[([0.0, 0.0, 0.0], 0.1), ([0.0, 0.0, 0.0], 0.1), ([1.0, 0.0, 0.0], 0.1)]);
    let samples = curve::resample(&repeated, 1.0, 0.25, 1e-4, 4000);
    assert!(samples.len() > 10 && samples.iter().all(|s| s.segment == 0 && s.center.is_finite()));
    // Two points and the smallest budgets there are: a tube is never fewer
    // than its two ends, and a budget of n pieces is n + 1 stations.
    let two = controls(&[([0.0, 0.0, 0.0], 0.1), ([1.0, 0.0, 0.0], 0.1)]);
    for (budget, stations) in [(0, 2), (1, 2), (2, 3), (3, 4)] {
        let samples = curve::resample(&two, 1.0, 0.25, 1e-4, budget);
        assert_eq!(samples.len(), stations, "budget {budget}");
        assert!(samples[0].center == Vec3::ZERO && samples.last().unwrap().center == Vec3::X);
    }
}

#[test]
fn the_sample_budget_is_kept() {
    let long = controls(&[([0.0, 0.0, 0.0], 0.001), ([50.0, 0.0, 0.0], 0.001), ([50.0, 50.0, 0.0], 0.001)]);
    let samples = curve::resample(&long, curve::tension_for(0.6), 0.25, 1e-5, 4000);
    assert!(samples.len() <= 4002 && samples.len() > 3900, "{} stations", samples.len());
}

#[test]
fn simplify_keeps_the_ends_and_the_corners() {
    let mut stroke: Vec<Vec3> = (0..=50).map(|i| Vec3::new(i as f32 * 0.02, 0.0, 0.0)).collect();
    stroke.extend((1..=50).map(|i| Vec3::new(1.0, i as f32 * 0.02, 0.0)));
    // A wobble well under the tolerance.
    for (i, p) in stroke.iter_mut().enumerate() {
        p.z = if i % 2 == 0 { 0.001 } else { -0.001 };
    }
    assert_eq!(curve::simplify(&stroke, 0.01), [0, 50, 100]);
    assert_eq!(curve::simplify(&stroke, 0.0).len(), stroke.len());
    assert_eq!(curve::simplify(&[], 0.1), Vec::<usize>::new());
    assert_eq!(curve::simplify(&stroke[..1], 0.1), [0]);
    assert_eq!(curve::simplify(&stroke[..2], 0.1), [0, 1]);
    assert!((curve::distance_to_segment(Vec3::new(0.5, 2.0, 0.0), Vec3::ZERO, Vec3::X) - 2.0).abs() < 1e-6);
    assert!((curve::distance_to_segment(Vec3::new(4.0, 0.0, 0.0), Vec3::ZERO, Vec3::X) - 3.0).abs() < 1e-6);
}

// ---- spine -------------------------------------------------------------------

#[test]
fn a_surface_point_is_lifted_half_its_radius_off_the_surface() {
    let points = [TubePoint::on_surface(Vec3::ZERO, Vec3::Y), TubePoint::on_surface(Vec3::X, Vec3::Y).sized(2.0), TubePoint::free(Vec3::X * 2.0)];
    let built = spine::build(&points, &settings(0.1), &Mat4::IDENTITY, &SymmetryOp::IDENTITY);
    assert!(built[0].position.distance(Vec3::new(0.0, 0.05, 0.0)) < 1e-6);
    assert!(built[1].position.distance(Vec3::new(1.0, 0.1, 0.0)) < 1e-6 && (built[1].radius - 0.2).abs() < 1e-6);
    assert!(built[2].position.distance(Vec3::new(2.0, 0.0, 0.0)) < 1e-6, "a free point is the centre line itself");
    // On a stretched anchor the tube keeps its radius and the lift follows
    // the visible surface.
    let stretched = Mat4::from_scale(Vec3::new(3.0, 1.0, 1.0));
    let built = spine::build(&points, &settings(0.1), &stretched, &SymmetryOp::IDENTITY);
    assert!(built[1].position.distance(Vec3::new(3.0, 0.1, 0.0)) < 1e-6);
    // Through a mirror the lift goes the mirrored way.
    let mirror = SymmetryOp::from_sign(Vec3::new(1.0, -1.0, 1.0));
    let built = spine::build(&points, &settings(0.1), &Mat4::IDENTITY, &mirror);
    assert!(built[0].position.distance(Vec3::new(0.0, -0.05, 0.0)) < 1e-6);
}

#[test]
fn taper_narrows_toward_the_tip_and_flips() {
    let points = free(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [4.0, 0.0, 0.0]]);
    let mut s = TubeSettings { taper: 0.8, ..settings(0.1) };
    let radii = |s: &TubeSettings| -> Vec<f32> {
        spine::build(&points, s, &Mat4::IDENTITY, &SymmetryOp::IDENTITY).iter().map(|c| c.radius).collect()
    };
    let r = radii(&s);
    assert!((r[0] - 0.1).abs() < 1e-6 && (r[1] - 0.08).abs() < 1e-6 && (r[2] - 0.02).abs() < 1e-6, "{r:?}");
    s.flip_taper = true;
    let r = radii(&s);
    assert!((r[0] - 0.02).abs() < 1e-6 && (r[2] - 0.1).abs() < 1e-6, "{r:?}");
    // A full taper still leaves a tip there is a surface for.
    s.taper = 1.0;
    assert!((radii(&s)[0] - 0.1 * spine::MIN_TAPER_FRACTION).abs() < 1e-7);
    // Sizes and radii outside their ranges are brought into them.
    let wild = [TubePoint::free(Vec3::ZERO).sized(100.0), TubePoint::free(Vec3::X).sized(0.0)];
    let built = spine::build(&wild, &settings(99.0), &Mat4::IDENTITY, &SymmetryOp::IDENTITY);
    assert_eq!(built[0].radius, TubeSettings::MAX_RADIUS * TubeSettings::MAX_SIZE);
    assert_eq!(built[1].radius, TubeSettings::MAX_RADIUS * TubeSettings::MIN_SIZE);
}

// ---- field -------------------------------------------------------------------

/// Distance to a sphere swept along a segment while its radius changes
/// evenly: the round cone, the slow way.
fn swept_sphere(p: Vec3, a: Vec3, b: Vec3, ra: f32, rb: f32) -> f32 {
    (0..=4000)
        .map(|i| {
            let t = i as f32 / 4000.0;
            p.distance(a.lerp(b, t)) - (ra + (rb - ra) * t)
        })
        .fold(f32::INFINITY, f32::min)
}

#[test]
fn the_field_of_two_stations_is_the_hull_of_their_spheres() {
    let cases = [
        (Vec3::new(0.1, 0.2, -0.1), Vec3::new(1.3, 0.5, 0.4), 0.3, 0.1),
        (Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), 0.1, 0.4),
        (Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0), 0.25, 0.25),
        // One sphere inside the other: just the larger sphere.
        (Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.1, 0.0, 0.0), 0.5, 0.1),
    ];
    for (a, b, ra, rb) in cases {
        let samples = [
            crate::Sample { center: a, radius: ra, ..Default::default() },
            crate::Sample { center: b, radius: rb, ..Default::default() },
        ];
        let mut field = TubeField::new();
        field.add_tube(&samples, CapStyle::Round);
        let mut seed = 7u32;
        let mut next = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as f32 / (1u32 << 24) as f32
        };
        for _ in 0..400 {
            let p = Vec3::new(next(), next(), next()) * 3.0 - Vec3::splat(1.0);
            let (got, want) = (field.distance(p), swept_sphere(p, a, b, ra, rb));
            // Exact outside; inside it only has to be inside.
            if want > 0.0 {
                assert!((got - want).abs() < 2e-3, "at {p}: {got} vs {want}");
            } else {
                assert!(got <= 2e-3, "at {p}: {got} should be inside");
            }
        }
    }
}

#[test]
fn the_lookup_agrees_with_testing_every_cone() {
    let points = free(&[[0.0, 0.0, 0.0], [0.5, 0.4, 0.1], [1.0, 0.0, 0.3], [1.2, -0.5, 0.0]]);
    for caps in [CapStyle::Round, CapStyle::Flat] {
        let field = field_of(&points, &TubeSettings { caps, taper: 0.5, ..settings(0.08) }, [true, false, false]);
        let band = 0.1;
        let lookup = field.lookup(band);
        let (mut near, mut far) = (0, 0);
        for i in 0..40 {
            for j in 0..30 {
                for k in 0..12 {
                    let p = Vec3::new(-1.5 + i as f32 * 0.075, -0.8 + j as f32 * 0.05, -0.2 + k as f32 * 0.05);
                    let (quick, slow) = (lookup.evaluate(p), field.distance(p));
                    if slow < band {
                        assert_eq!(quick, slow, "at {p}");
                        near += 1;
                    } else {
                        assert!(quick >= band, "at {p}: {quick} but the tubes are {slow} away");
                        far += (quick >= FAR) as usize;
                    }
                }
            }
        }
        assert!(near > 500 && far > 1000, "{near} near, {far} far");
    }
    let empty = TubeField::new();
    assert_eq!(empty.lookup(1.0).evaluate(Vec3::ZERO), FAR);
    assert_eq!(empty.distance(Vec3::ZERO), FAR);
    assert!(empty.voxelize(0.1, [false; 3], 1000).is_err());
}

#[test]
fn a_straight_tube_has_the_volume_of_a_cylinder_with_its_caps() {
    let (r, length) = (0.1f64, 1.0f64);
    // Off the lattice in every direction.
    let ends = free(&[[-0.487, 0.021, -0.017], [0.513, 0.021, -0.017]]);
    let capsule = PI * r * r * length + sphere_volume(r);
    let cylinder = PI * r * r * length;
    for (caps, want, name) in [(CapStyle::Round, capsule, "round"), (CapStyle::Flat, cylinder, "flat")] {
        let field = field_of(&ends, &TubeSettings { caps, ..settings(r as f32) }, [false; 3]);
        for (cells, tolerance) in [(6.0, 0.02), (12.0, 0.006)] {
            let mesh = mesh_of(&field, r as f32 / cells, [false; 3]);
            assert_closed(&mesh, name);
            assert_eq!(shells(&mesh), 1);
            let ratio = mesh.signed_volume() / want;
            assert!((ratio - 1.0).abs() < tolerance, "{name} ends at {cells} cells a radius: volume ratio {ratio}");
        }
    }
    // The flat end is flat: nothing pokes past the end plane.
    let field = field_of(&ends, &TubeSettings { caps: CapStyle::Flat, ..settings(0.1) }, [false; 3]);
    let mesh = mesh_of(&field, 0.1 / 8.0, [false; 3]);
    let reach = mesh.positions.iter().map(|p| p.x).fold(f32::MIN, f32::max);
    assert!((reach - 0.513).abs() < 0.004, "the flat end reaches x = {reach}");
}

#[test]
fn awkward_tubes_still_come_out_closed_and_wound_outward() {
    let tight_bend = free(&[[0.0, 0.0, 0.0], [0.5, 0.0, 0.0], [0.5, 0.05, 0.0], [0.0, 0.05, 0.02]]);
    let crossing = free(&[[-0.5, 0.0, 0.0], [0.5, 0.0, 0.0], [0.5, 0.5, 0.0], [0.0, 0.5, 0.0], [0.0, -0.5, 0.0]]);
    let mut cornered = free(&[[0.0, 0.0, 0.0], [0.6, 0.0, 0.0], [0.6, 0.6, 0.0]]);
    cornered[1].sharp = true;
    let mut ball = free(&[[0.0, 0.0, 0.0], [0.2, 0.0, 0.0], [0.4, 0.0, 0.0]]);
    ball[1].size = 5.0;
    let cases = [
        (tight_bend, settings(0.1), "a bend tighter than the tube is thick"),
        (crossing, settings(0.06), "a tube crossing itself"),
        (cornered, TubeSettings { caps: CapStyle::Flat, ..settings(0.08) }, "a sharp corner with flat ends"),
        (ball, settings(0.05), "a point far thicker than its neighbours"),
        (free(&[[0.0, 0.0, 0.0], [1.0, 0.3, 0.2]]), TubeSettings { taper: 1.0, ..settings(0.1) }, "tapered to a point"),
    ];
    for (points, settings, name) in cases {
        let field = field_of(&points, &settings, [false; 3]);
        let floor = field.min_radius().max(settings.radius * merge::RESOLVED_TIP_FRACTION);
        let cell = (settings.radius / 6.0).min(floor / merge::CELLS_ACROSS_RADIUS);
        let mesh = mesh_of(&field, cell, [false; 3]);
        assert_closed(&mesh, name);
        assert_eq!(shells(&mesh), 1, "{name}");
        // Every vertex is on the surface of the field (as thick as a cell
        // where the tube is thinner), to a fraction of a cell.
        let meshed = field.thickened(cell);
        let off = mesh.positions.iter().map(|&p| meshed.distance(p).abs()).fold(0.0, f32::max);
        assert!(off < cell * 0.6, "{name}: a vertex {} cells off the surface", off / cell);
    }
}

/// A tip thinner than the lattice is meshed a cell thick rather than left to
/// break up, whichever way the tube runs through the lattice.
#[test]
fn a_tip_too_thin_for_the_lattice_rounds_off_in_one_piece() {
    let directions = [[1.0, 0.0, 0.0], [1.0, 1.0, 1.0], [0.3, 1.0, 0.1], [0.7, -0.2, 0.55], [0.0, 0.31, 0.95]];
    for direction in directions {
        let end = Vec3::from(direction).normalize() * 1.3 + Vec3::new(0.011, 0.017, -0.013);
        let points = [TubePoint::free(Vec3::new(0.011, 0.017, -0.013)), TubePoint::free(end)];
        let field = field_of(&points, &TubeSettings { taper: 1.0, ..settings(0.1) }, [false; 3]);
        assert!(field.min_radius() < 0.0035);
        for cell in [0.012, 0.02, 0.031] {
            let mesh = mesh_of(&field, cell, [false; 3]);
            assert_closed(&mesh, "tapered tube");
            assert_eq!(shells(&mesh), 1, "toward {direction:?} on a lattice of {cell}");
            // It still reaches its last point.
            let reach = mesh.positions.iter().map(|p| p.distance(end)).fold(f32::MAX, f32::min);
            assert!(reach < cell * 1.5, "toward {direction:?}: the tip stops {reach} short");
        }
    }
    let thick = field_of(&free(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]]), &settings(0.1), [false; 3]);
    assert_eq!(thick.thickened(0.05).min_radius(), thick.min_radius(), "nothing to fatten");
    let fattened = thick.thickened(0.2);
    assert_eq!(fattened.min_radius(), 0.2);
    assert!((fattened.distance(Vec3::new(0.5, 0.5, 0.0)) - 0.3).abs() < 1e-6);
    assert!((fattened.bounds().max.y - 0.2).abs() < 1e-6);
}

#[test]
fn a_varying_radius_is_honoured_along_the_curve() {
    let r = 0.08;
    let points = [TubePoint::free(Vec3::new(-0.6, 0.0, 0.0)), TubePoint::free(Vec3::ZERO).sized(2.5), TubePoint::free(Vec3::new(0.6, 0.0, 0.0))];
    let field = field_of(&points, &settings(r), [false; 3]);
    let cell = r / 8.0;
    let mesh = mesh_of(&field, cell, [false; 3]);
    assert_closed(&mesh, "swelling tube");
    let radial = |p: &Vec3| (p.y * p.y + p.z * p.z).sqrt();
    let around = |x: f32| -> (f32, f32) {
        let ring: Vec<f32> = mesh.positions.iter().filter(|p| (p.x - x).abs() < cell).map(radial).collect();
        assert!(ring.len() > 8, "no vertices near x = {x}");
        (ring.iter().copied().fold(f32::MAX, f32::min), ring.iter().copied().fold(f32::MIN, f32::max))
    };
    // 2.5 radii thick at the fat point, one radius at the ends, and in
    // between somewhere between the two, growing toward the middle.
    let (low, high) = around(0.0);
    assert!((low - 2.5 * r).abs() < cell && (high - 2.5 * r).abs() < cell, "at the fat point: {low}..{high}");
    let mut last = 0.0;
    for x in [-0.55, -0.4, -0.25, -0.1] {
        let (low, high) = around(x);
        assert!(low > r - cell && high < 2.5 * r + cell, "at {x}: {low}..{high}");
        assert!(low > last, "the tube should thicken toward the fat point: {low} at {x} after {last}");
        last = low;
    }
    let tip = mesh.positions.iter().map(|p| p.x).fold(f32::MIN, f32::max);
    assert!((tip - (0.6 + r)).abs() < cell * 0.5, "the round end reaches {tip}");
    assert!(field.min_radius() > r * 0.99 && field.min_radius() < r * 1.01);
}

#[test]
fn a_mirrored_tube_is_exactly_symmetric() {
    let apart = free(&[[0.3, 0.1, 0.0], [0.8, 0.4, 0.2], [1.1, 0.2, 0.5]]);
    let astride = free(&[[-0.15, 0.1, 0.0], [0.4, 0.4, 0.2], [0.9, 0.2, 0.5]]);
    for (points, mirror, name) in [
        (&apart, [true, false, false], "a pair either side of X"),
        (&astride, [true, false, false], "a pair crossing the X plane"),
        (&apart, [true, true, true], "eight copies"),
        (&astride, [false, false, true], "a pair across Z"),
    ] {
        let field = field_of(points, &TubeSettings { taper: 0.4, ..settings(0.07) }, mirror);
        let built = field.voxelize(0.07 / 5.0, mirror, MAX_VOXEL_SAMPLES).unwrap();
        assert_eq!(built.surface.stats.mirror_skipped, 0, "{name}");
        let mesh = built.surface.into_mesh();
        assert_closed(&mesh, name);
        for axis in (0..3).filter(|&a| mirror[a]) {
            let (unmatched, on_plane) = unmatched_across(&mesh, axis);
            assert_eq!(unmatched, 0, "{name}: of {} vertices, across axis {axis}", mesh.vertex_count());
            if std::ptr::eq(points, &astride) {
                assert!(on_plane > 0, "{name}: nothing on the plane");
            }
        }
        // The tube starts on the Z plane, so its image across Z joins it.
        if std::ptr::eq(points, &apart) {
            assert_eq!(shells(&mesh), if mirror[2] { 4 } else { 2 }, "{name}");
        }
    }
}

#[test]
fn a_lattice_too_big_is_coarsened_and_a_tube_too_thin_is_an_error() {
    let field = field_of(&free(&[[0.0, 0.0, 0.0], [3.0, 1.0, 0.5]]), &settings(0.05), [true, false, false]);
    for budget in [50_000, 400_000] {
        let built = field.voxelize(0.001, [true, false, false], budget).unwrap();
        assert!(built.grid.cell > 0.001 && built.grid.sample_count() <= budget, "{:?}", built.grid);
        assert!(built.grid.sample_count() > budget / 2, "coarsened further than it had to be: {:?}", built.grid);
        assert_closed(&built.surface.into_mesh(), "coarsened tube");
    }
    // No budget at all still gives a lattice, of a few thousand samples.
    let least = field.grid_for(0.001, [false; 3], 0).unwrap();
    assert!(least.sample_count() <= 4096 && least.sample_count() > 1000, "{least:?}");
    // A thread on a lattice far coarser than itself is meshed as thick as
    // the lattice can hold, in one piece.
    let thread = field_of(&free(&[[0.012, 0.013, 0.011], [2.4, 0.9, 0.3]]), &settings(0.001), [false; 3]);
    let mesh = mesh_of(&thread, 0.1, [false; 3]);
    assert_closed(&mesh, "thread");
    assert_eq!(shells(&mesh), 1);
    let fattest = mesh.positions.iter().map(|&p| thread.distance(p)).fold(0.0, f32::max);
    assert!(fattest > 0.05 && fattest < 0.16, "the thread came out {fattest} thick");
    // A tube far from the origin is still built.
    let far = field_of(&free(&[[5000.0, 0.0, 0.0], [5000.5, 0.2, 0.0]]), &settings(0.05), [false; 3]);
    assert_closed(&mesh_of(&far, 0.01, [false; 3]), "far tube");
}

// ---- merge -------------------------------------------------------------------

/// Volume of a unit-centred sphere of radius `big` joined with a tube of
/// radius `r` along x from `from` to `to` with round ends: both are solids
/// of revolution about x, so the union's cross-section is the larger disc.
fn union_volume(big: f64, r: f64, from: f64, to: f64) -> f64 {
    let steps = 400_000;
    let (lo, hi) = (-big, (to + r).max(big));
    let dx = (hi - lo) / steps as f64;
    (0..steps)
        .map(|i| {
            let x = lo + (i as f64 + 0.5) * dx;
            let sphere = (big * big - x * x).max(0.0);
            let along = (x - x.clamp(from, to)).abs();
            let tube = (r * r - along * along).max(0.0);
            PI * sphere.max(tube) * dx
        })
        .sum()
}

fn capsule_distance(p: Vec3, from: f32, to: f32, r: f32) -> f32 {
    p.distance(Vec3::new(p.x.clamp(from, to), 0.0, 0.0)) - r
}

/// A tube of radius 0.15 growing out of a unit sphere along x.
fn horn_on_a_sphere(blend: f32, mirror: [bool; 3]) -> (TriMesh, TriMesh, MergePlan) {
    let sphere = icosphere(5, 1.0, Vec3::ZERO);
    let field = field_of(&free(&[[0.9, 0.0, 0.0], [1.8, 0.0, 0.0]]), &settings(0.15), mirror);
    let plan = merge::plan(&sphere.positions, &sphere.indices, &Mat4::IDENTITY, &field, 0.15, 0.15);
    let merged = merge::merge(&sphere, &Mat4::IDENTITY, &field, &plan, blend, mirror, &|_| true).unwrap();
    (sphere, merged, plan)
}

#[test]
fn a_tube_fused_onto_a_sphere_is_one_closed_shell_with_the_unions_volume() {
    let (sphere, merged, plan) = horn_on_a_sphere(0.0, [false; 3]);
    assert!(!plan.too_thin && plan.resolution > 60 && plan.resolution < 100, "{plan:?}");
    assert_closed(&merged, "sphere with a horn");
    assert_eq!(shells(&merged), 1, "the wall inside the join should be gone");
    // The model keeps its density.
    let (before, after) = (sphere.triangle_count() as f32, merged.triangle_count() as f32);
    assert!(after > before * 0.8 && after < before * 1.6, "{before} triangles became {after}");

    let want = union_volume(1.0, 0.15, 0.9, 1.8);
    let got = merged.signed_volume();
    assert!((got / want - 1.0).abs() < 0.004, "volume {got}, the union's is {want}");
    // The horn itself, which is a sixtieth of that volume: as thick as it
    // was drawn along its length, and as long.
    let cell = plan.remesh_cell;
    let along: Vec<f32> =
        merged.positions.iter().filter(|p| p.x > 1.2 && p.x < 1.75).map(|p| (p.y * p.y + p.z * p.z).sqrt()).collect();
    assert!(along.len() > 100, "{} vertices along the horn", along.len());
    let (thinnest, thickest) = along.iter().fold((f32::MAX, f32::MIN), |(l, h), &r| (l.min(r), h.max(r)));
    assert!(thinnest > 0.15 - 0.25 * cell && thickest < 0.15 + 0.1 * cell, "the horn is {thinnest}..{thickest} thick");
    let tip = merged.positions.iter().map(|p| p.x).fold(f32::MIN, f32::max);
    assert!((tip - 1.95).abs() < 0.25 * cell, "the horn reaches x = {tip}");
    // Everything else is the sphere.
    let off = merged.positions.iter().filter(|p| p.x < 0.9).map(|p| (p.length() - 1.0).abs()).fold(0.0, f32::max);
    assert!(off < 0.15 * cell, "a vertex {off} off the sphere");
}

#[test]
fn the_blend_is_a_fillet_at_the_join_and_nothing_anywhere_else() {
    let (_, crisp, _) = horn_on_a_sphere(0.0, [false; 3]);
    let k = 0.15;
    let (_, blended, plan) = horn_on_a_sphere(k, [false; 3]);
    assert_closed(&blended, "blended horn");
    assert_eq!(shells(&blended), 1);
    assert_eq!(crisp.indices, blended.indices, "the blend moves vertices; it does not rebuild");

    // Signed distances to the two solids: the smaller is how far outside
    // their union a point is.
    let outside = |p: Vec3| (p.length() - 1.0).min(capsule_distance(p, 0.9, 1.8, 0.15));
    let cell = plan.cell;
    let (mut moved, mut highest) = (0, 0.0f32);
    for (&a, &b) in crisp.positions.iter().zip(&blended.positions) {
        let apart = (p_apart(a), outside(b));
        if a != b {
            moved += 1;
            // Only near the seam circle, where the tube leaves the sphere.
            assert!(apart.0 < k * 4.0, "a vertex {} from the seam was moved", apart.0);
        }
        // A fillet fills the crease: it never cuts into either solid, and
        // rises by a quarter of its size at most.
        assert!(apart.1 > -0.5 * cell && apart.1 < k * 0.25 + 0.5 * cell, "a vertex {} outside the union", apart.1);
        highest = highest.max(apart.1);
    }
    assert!(moved > 100, "{moved} vertices moved");
    assert!(highest > k * 0.15, "the fillet only rises {highest}");
    assert!(blended.signed_volume() > crisp.signed_volume());

    // How far a point is from the circle the seam runs along.
    fn p_apart(p: Vec3) -> f32 {
        let seam_x = (1.0f32 - 0.15 * 0.15).sqrt();
        let radial = (p.y * p.y + p.z * p.z).sqrt();
        ((p.x - seam_x).powi(2) + (radial - 0.15).powi(2)).sqrt()
    }
}

#[test]
fn a_mirrored_pair_fused_onto_a_symmetric_model_is_exactly_symmetric() {
    let sphere = icosphere(5, 1.0, Vec3::ZERO);
    assert_eq!(unmatched_across(&sphere, 0).0, 0, "the test sphere is its own mirror image");
    let mirror = [true, false, false];
    let field = field_of(&free(&[[0.5, 0.6, 0.0], [0.9, 1.3, 0.2]]), &TubeSettings { taper: 0.5, ..settings(0.12) }, mirror);
    let plan = merge::plan(&sphere.positions, &sphere.indices, &Mat4::IDENTITY, &field, 0.12, 0.12);
    for blend in [0.0, 0.12] {
        let stages = std::cell::RefCell::new(Vec::new());
        let note = |stage: Stage| {
            stages.borrow_mut().push(stage);
            true
        };
        let merged = merge::merge(&sphere, &Mat4::IDENTITY, &field, &plan, blend, mirror, &note).unwrap();
        assert_closed(&merged, "sphere with a pair of horns");
        assert_eq!(shells(&merged), 1);
        let (unmatched, on_plane) = unmatched_across(&merged, 0);
        assert_eq!(unmatched, 0, "blend {blend}: of {} vertices", merged.vertex_count());
        assert!(on_plane > 50);
        // Both horns are there, out to their tips.
        for side in [-1.0, 1.0] {
            let tip = Vec3::new(0.9 * side, 1.3, 0.2);
            let reach = merged.positions.iter().map(|p| p.distance(tip)).fold(f32::MAX, f32::min);
            assert!(reach < 0.1, "the horn at x = {} stops {reach} short of its tip", tip.x);
        }
        assert!(merged.signed_volume() > sphere.signed_volume() * 1.002);
        let expected: &[Stage] = if blend > 0.0 { &[Stage::Tubes, Stage::Union, Stage::Blend] } else { &[Stage::Tubes, Stage::Union] };
        assert_eq!(stages.borrow().as_slice(), expected);
    }
}

#[test]
fn a_merge_runs_in_the_models_own_space_whatever_its_scale() {
    // The model is a unit sphere shown at three times the size, mirrored
    // (a negative scale); the tube was built in the shown size.
    let sphere = icosphere(4, 1.0, Vec3::ZERO);
    let to_field = Mat4::from_scale(Vec3::new(-3.0, 3.0, 3.0));
    let field = field_of(&free(&[[2.7, 0.0, 0.0], [5.4, 0.0, 0.0]]), &settings(0.45), [false; 3]);
    let plan = merge::plan(&sphere.positions, &sphere.indices, &to_field, &field, 0.45, 0.45);
    assert!(!plan.too_thin);
    let merged = merge::merge(&sphere, &to_field, &field, &plan, 0.45, [false; 3], &|_| true).unwrap();
    assert_closed(&merged, "scaled model with a horn");
    assert_eq!(shells(&merged), 1);
    // In the model's own space the horn is a third the size, and on -x.
    let tip = merged.positions.iter().map(|p| p.x).fold(f32::MAX, f32::min);
    assert!((tip + 1.95).abs() < 0.03, "the horn reaches x = {tip}");
    let want = union_volume(1.0, 0.15, 0.9, 1.8);
    assert!(merged.signed_volume() > want * 0.99 && merged.signed_volume() < want * 1.03, "{}", merged.signed_volume());
}

#[test]
fn a_merge_that_cannot_be_done_says_why() {
    let sphere = icosphere(3, 1.0, Vec3::ZERO);
    let field = field_of(&free(&[[0.9, 0.0, 0.0], [1.8, 0.0, 0.0]]), &settings(0.15), [false; 3]);
    let plan = merge::plan(&sphere.positions, &sphere.indices, &Mat4::IDENTITY, &field, 0.15, 0.15);
    let run = |target: &TriMesh, field: &TubeField, go: &dyn Fn(Stage) -> bool| {
        merge::merge(target, &Mat4::IDENTITY, field, &plan, 0.1, [false; 3], go)
    };
    assert!(run(&TriMesh::default(), &field, &|_| true).unwrap_err().contains("no geometry"));
    assert!(run(&sphere, &TubeField::new(), &|_| true).unwrap_err().contains("no tube"));
    assert_eq!(run(&sphere, &field, &|_| false).unwrap_err(), "Cancelled.");
    assert_eq!(run(&sphere, &field, &|stage| stage != Stage::Blend).unwrap_err(), "Cancelled.");
    // A hair on a coarse model is flagged rather than refused.
    let hair = field_of(&free(&[[0.9, 0.0, 0.0], [1.8, 0.0, 0.0]]), &settings(0.01), [false; 3]);
    assert!(merge::plan(&sphere.positions, &sphere.indices, &Mat4::IDENTITY, &hair, 0.01, 0.01).too_thin);
}

#[test]
fn the_plan_stays_in_range_at_its_extremes() {
    let field = field_of(&free(&[[0.9, 0.0, 0.0], [1.8, 0.0, 0.0]]), &settings(0.15), [false; 3]);
    // Twenty huge triangles: the coarsest lattice a merge uses.
    let coarse = icosphere(0, 1.0, Vec3::ZERO);
    let plan = merge::plan(&coarse.positions, &coarse.indices, &Mat4::IDENTITY, &field, 0.15, 0.15);
    assert_eq!(plan.resolution, 16);
    assert!(plan.too_thin && plan.tube_cell > 0.0 && plan.tube_cell <= 0.15 / merge::CELLS_ACROSS_RADIUS);
    // Triangles with no size ask for an endlessly fine lattice.
    let speck = TriMesh::new(vec![Vec3::ZERO; 3], vec![0, 1, 2]);
    let plan = merge::plan(&speck.positions, &speck.indices, &Mat4::IDENTITY, &field, 0.15, 0.15);
    assert_eq!(plan.resolution, sculpt_volume::remesh::MAX_RESOLUTION);
    // Nothing at all.
    let plan = merge::plan(&[], &[], &Mat4::IDENTITY, &TubeField::new(), 0.15, 0.15);
    assert!(plan.resolution >= 16 && plan.resolution <= sculpt_volume::remesh::MAX_RESOLUTION);
    assert_eq!(merge::density_cell(&[], &[]), 0.0);
    // One triangle of area a half: three triangles to a cell squared.
    assert!((merge::density_cell(&[Vec3::ZERO, Vec3::X, Vec3::Y], &[0, 1, 2]) - 1.5f32.sqrt()).abs() < 1e-6);
    assert_eq!(merge::density_cell(&[Vec3::ZERO], &[0, 5, 9]), 0.0, "a damaged list is no reason to panic");

    // A kept tube: a sixth of its radius with no anchor, the anchor's own
    // edge length within a twelfth to a third of it, and never coarser than
    // its thinnest part worth keeping can stand.
    let near = |got: f32, want: f32| assert!((got - want).abs() < 1e-6, "{got} for {want}");
    near(merge::separate_cell(0.12, None, &field), 0.02);
    near(merge::separate_cell(0.12, Some(1.0), &field), 0.04);
    near(merge::separate_cell(0.12, Some(0.03), &field), 0.03);
    near(merge::separate_cell(0.12, Some(0.0), &field), 0.01);
    near(merge::separate_cell(0.6, None, &field), 0.6 * merge::RESOLVED_TIP_FRACTION / merge::CELLS_ACROSS_RADIUS);
}

#[test]
fn stations_feed_the_field_in_whatever_space_they_are_in() {
    let samples = samples_of(&free(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]]), &settings(0.1));
    let mut field = TubeField::new();
    field.add_tube(&samples, CapStyle::Round);
    assert_eq!(field.cone_count(), samples.len() - 1);
    assert!(field.bounds().min.distance(Vec3::new(-0.1, -0.1, -0.1)) < 1e-5);
    assert!(field.bounds().max.distance(Vec3::new(1.1, 0.1, 0.1)) < 1e-5);
    assert!((field.distance(Vec3::new(0.5, 0.3, 0.0)) - 0.2).abs() < 1e-5);
    assert!((field.distance(Vec3::new(1.3, 0.0, 0.0)) - 0.2).abs() < 1e-5);
    // One station is not a tube.
    let mut none = TubeField::new();
    none.add_tube(&samples[..1], CapStyle::Flat);
    assert!(none.is_empty());
}

#[test]
#[ignore]
fn probe_leaning_fillet() {
    let sphere = icosphere(6, 0.5, Vec3::ZERO);
    let start = Vec3::new(-0.15, 0.2, (0.25f32 - 0.15 * 0.15 - 0.2 * 0.2).sqrt());
    let points = free(&[
        start.to_array(),
        [-0.275, 0.475, start.z],
        [-0.5, 0.6, start.z],
        [-0.725, 0.475, start.z],
    ]);
    let field = field_of(&points, &settings(0.06), [false; 3]);
    let plan = merge::plan(&sphere.positions, &sphere.indices, &Mat4::IDENTITY, &field, 0.06, 0.06);
    println!("{plan:?}");
    for k in [0.0, 0.03, 0.06, 0.12] {
        let merged = merge::merge(&sphere, &Mat4::IDENTITY, &field, &plan, k, [false; 3], &|_| true).unwrap();
        // Dihedral angles and folded triangles.
        let normal = |t: usize| {
            let [a, b, c] = merged.triangle(t);
            (b - a).cross(c - a)
        };
        let mut edges: std::collections::HashMap<(u32, u32), Vec<usize>> = std::collections::HashMap::new();
        for t in 0..merged.triangle_count() {
            for c in 0..3 {
                let (a, b) = (merged.indices[t * 3 + c], merged.indices[t * 3 + (c + 1) % 3]);
                edges.entry((a.min(b), a.max(b))).or_default().push(t);
            }
        }
        let mut worst = 1.0f32;
        let mut sharp = 0;
        let mut worst_at = Vec3::ZERO;
        for (edge, tris) in &edges {
            if tris.len() != 2 {
                continue;
            }
            let (n0, n1) = (normal(tris[0]).normalize_or_zero(), normal(tris[1]).normalize_or_zero());
            let cos = n0.dot(n1);
            if cos < -0.2 {
                sharp += 1;
            }
            if cos < worst {
                worst = cos;
                worst_at = merged.positions[edge.0 as usize];
            }
        }
        let smallest = (0..merged.triangle_count()).map(|t| normal(t).length()).fold(f32::MAX, f32::min);
        let cell = plan.remesh_cell;
        println!(
            "k {k}: worst dihedral cos {worst:.3} at {worst_at} (a {:.4} b {:.4}), {sharp} edges past 100 degrees, smallest triangle {:.2e} cell^2, defects {:?}",
            worst_at.length() - 0.5,
            field.distance(worst_at),
            smallest / (cell * cell),
            crate::testing::defects(&merged)
        );
    }
}
