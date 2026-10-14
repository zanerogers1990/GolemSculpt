//! The deformers' maths against answers worked out by hand.

use std::f64::consts::{FRAC_PI_2, PI};

use glam::DVec3;

use super::lattice::{self, Cage};
use super::*;

/// A cylinder's surface along Y from 0 to `length`, of radius `radius`,
/// round the Y axis: rings of points, and their normals.
fn cylinder(length: f64, radius: f64) -> Vec<(DVec3, DVec3)> {
    let mut points = Vec::new();
    for ring in 0..=40 {
        let y = length * ring as f64 / 40.0;
        for k in 0..24 {
            let a = k as f64 / 24.0 * 2.0 * PI;
            let n = DVec3::new(a.sin(), 0.0, a.cos());
            points.push((DVec3::new(0.0, y, 0.0) + n * radius, n));
        }
    }
    points
}

fn bounds_of(points: &[(DVec3, DVec3)]) -> Bounds {
    let min = points.iter().fold(DVec3::splat(f64::MAX), |m, (p, _)| m.min(*p));
    let max = points.iter().fold(DVec3::splat(f64::MIN), |m, (p, _)| m.max(*p));
    Bounds::new(min, max)
}

fn params(kind: Kind) -> Params {
    Params { kind, softness: 0.0, ..Params::default() }
}

fn moved(d: &Deformer, q: DVec3) -> DVec3 {
    d.apply(q, DVec3::ZERO).unwrap_or(q)
}

#[test]
fn a_ninety_degree_bend_lays_a_cylinder_on_a_quarter_circle() {
    let (length, radius) = (2.0, 0.25);
    let points = cylinder(length, radius);
    let bounds = bounds_of(&points);
    // Along Y, toward Z (direction 0), the start fixed, no softness: Barr's bend.
    let p = Params { angle: 90.0, ..params(Kind::Bend) };
    let d = Deformer::new(&p, &bounds).unwrap();
    // The spine is bent into a circle of radius L / θ about (y, z) = (0, R).
    let big = length / FRAC_PI_2;
    for &(q, _) in &points {
        let theta = FRAC_PI_2 * q.y / length;
        let spine = DVec3::new(0.0, big * theta.sin(), big * (1.0 - theta.cos()));
        // Across the bend (Z) the point stays on the spine's normal; out of
        // the bend's plane (X) it is left as it was.
        let normal = DVec3::new(0.0, -theta.sin(), theta.cos());
        let want = spine + normal * q.z + DVec3::X * q.x;
        let got = moved(&d, q);
        assert!(got.distance(want) < 1e-9, "{q} went to {got}, not {want}");
    }
    // The end's middle lands a quarter turn round: up R and across R.
    let tip = moved(&d, DVec3::new(0.0, length, 0.0));
    assert!(tip.distance(DVec3::new(0.0, big, big)) < 1e-9, "{tip}");
    // The start is where it was, to the bit.
    assert!(d.apply(DVec3::new(0.0, 0.0, radius), DVec3::ZERO).is_none());
}

#[test]
fn a_bend_anchored_at_the_end_or_the_middle_leaves_that_part_where_it_is() {
    let points = cylinder(2.0, 0.25);
    let bounds = bounds_of(&points);
    for softness in [0.0, 0.5] {
        let end = Params { angle: 70.0, anchor: Anchor::End, softness, ..params(Kind::Bend) };
        let d = Deformer::new(&end, &bounds).unwrap();
        assert!(d.apply(DVec3::new(0.1, 2.0, 0.2), DVec3::ZERO).is_none(), "the end stays");
        assert!(moved(&d, DVec3::new(0.0, 0.0, 0.0)).distance(DVec3::ZERO) > 0.5, "the start swings");

        let middle = Params { anchor: Anchor::Middle, ..end };
        let d = Deformer::new(&middle, &bounds).unwrap();
        let at = DVec3::new(0.1, 1.0, -0.2);
        assert!(moved(&d, at).distance(at) < 1e-12, "the middle stays");
        // Both ends come up toward Z by the same amount: mirror images.
        let (a, b) = (moved(&d, DVec3::new(0.0, 0.0, 0.0)), moved(&d, DVec3::new(0.0, 2.0, 0.0)));
        assert!((a.z - b.z).abs() < 1e-9 && ((a.y - 1.0) + (b.y - 1.0)).abs() < 1e-9, "{a} {b}");
    }
}

#[test]
fn bend_arc_puts_the_end_of_the_range_on_the_tip_handle() {
    let points = cylinder(2.0, 0.25);
    let bounds = bounds_of(&points);
    for softness in [0.0, 0.3, 1.0] {
        for tip in [[0.4, -0.3, 0.6], [-0.2, -0.5, 0.0], [0.0, 0.5, 0.0], [0.0, -0.6, -1.1]] {
            let p = Params { tip, softness, ..params(Kind::BendArc) };
            let handle = tip_point(&p, &bounds);
            let d = Deformer::new(&p, &bounds).unwrap();
            let end = moved(&d, DVec3::new(0.0, 2.0, 0.0));
            assert!(end.distance(handle) < 1e-9, "{softness} {tip:?}: {end} is not on {handle}");
            assert_eq!(tip_for(&p, &bounds, handle), tip);
            assert!(d.apply(DVec3::new(0.0, 0.0, 0.25), DVec3::ZERO).is_none(), "the start stays");
        }
    }
}

#[test]
fn twist_turns_each_slice_by_its_share_of_the_angle() {
    let points = cylinder(2.0, 0.25);
    let bounds = bounds_of(&points);
    let p = Params { angle: 180.0, ..params(Kind::Twist) };
    let d = Deformer::new(&p, &bounds).unwrap();
    for &(q, _) in &points {
        let turn = PI * q.y / 2.0;
        // About the axis line (Y), from Z toward X.
        let (sin, cos) = turn.sin_cos();
        let want = DVec3::new(q.z * sin + q.x * cos, q.y, q.z * cos - q.x * sin);
        assert!(moved(&d, q).distance(want) < 1e-12, "{q}: {} {want}", moved(&d, q));
    }
}

#[test]
fn twist_of_nothing_and_taper_of_one_to_one_do_nothing_at_all() {
    let points = cylinder(2.0, 0.25);
    let bounds = bounds_of(&points);
    for p in [params(Kind::Twist), Params { scales: [1.0, 1.0], ..params(Kind::Taper) }] {
        assert!(p.is_identity());
        assert!(Deformer::new(&p, &bounds).is_none());
    }
    // A taper from one size to itself also leaves the range alone, to the bit.
    let p = Params { scales: [1.0, 0.5], range: [0.5, 1.0], ..params(Kind::Taper) };
    let d = Deformer::new(&p, &bounds).unwrap();
    assert!(d.apply(DVec3::new(0.25, 0.4, 0.0), DVec3::ZERO).is_none());
    let tip = moved(&d, DVec3::new(0.25, 2.0, 0.0));
    assert!(tip.distance(DVec3::new(0.125, 2.0, 0.0)) < 1e-12, "{tip}");
}

#[test]
fn stretch_keeps_the_volume_and_flatten_presses_toward_the_anchor() {
    let points = cylinder(2.0, 0.25);
    let bounds = bounds_of(&points);
    let p = Params { stretch: 2.0, ..params(Kind::Stretch) };
    let d = Deformer::new(&p, &bounds).unwrap();
    let side = moved(&d, DVec3::new(0.25, 1.0, 0.0));
    assert!(side.distance(DVec3::new(0.25 / 2f64.sqrt(), 2.0, 0.0)) < 1e-12, "{side}");
    let p = Params { keep_volume: false, ..p };
    let side = moved(&Deformer::new(&p, &bounds).unwrap(), DVec3::new(0.25, 1.0, 0.0));
    assert!(side.distance(DVec3::new(0.25, 2.0, 0.0)) < 1e-12, "{side}");

    let p = Params { flatten: 0.75, anchor: Anchor::End, ..params(Kind::Flatten) };
    let d = Deformer::new(&p, &bounds).unwrap();
    assert!(d.apply(DVec3::new(0.25, 2.0, 0.0), DVec3::ZERO).is_none(), "the end stays");
    let start = moved(&d, DVec3::new(0.25, 0.0, 0.0));
    assert!(start.distance(DVec3::new(0.25, 1.5, 0.0)) < 1e-12, "a quarter of the length is left: {start}");
}

#[test]
fn inflate_pushes_along_the_normal_inside_the_range_only() {
    let points = cylinder(2.0, 0.25);
    let bounds = bounds_of(&points);
    let p = Params { inflate: 0.125, range: [0.25, 0.75], softness: 0.0, ..params(Kind::Inflate) };
    let d = Deformer::new(&p, &bounds).unwrap();
    assert_eq!(d.reach(), Some((0.5, 1.5)));
    for &(q, n) in &points {
        match d.apply(q, n) {
            Some(to) => assert!((to - q).distance(n * 0.25) < 1e-12 && (0.5..=1.5).contains(&q.y), "{q} {to}"),
            None => assert!(!(0.5..=1.5).contains(&q.y)),
        }
    }
}

#[test]
fn an_untouched_lattice_is_the_identity() {
    let points = cylinder(2.0, 0.25);
    let bounds = bounds_of(&points);
    for count in [[2, 2, 2], [3, 4, 5], [6, 6, 6]] {
        let p = Params { lattice: Lattice::default().with_count(count), ..params(Kind::Lattice) };
        // As the app runs it: nothing to set up, so every vertex stays to the bit.
        assert!(Deformer::new(&p, &bounds).is_none());
        // And the textbook sum over every control point gives each point back
        // to within a few units in the last place of the box's size.
        let span = bounds.span(1, [0.0, 1.0]);
        for &(q, _) in &points {
            let sum = Cage::full_sum(&p.lattice, &bounds, 1, span, q);
            assert!(sum.distance(q) < 1e-12, "{count:?} {q}: {sum}");
        }
    }
}

#[test]
fn a_dragged_lattice_point_carries_the_clay_nearest_it_furthest() {
    let points = cylinder(2.0, 0.25);
    let bounds = bounds_of(&points);
    let mut cage = Lattice::default();
    // The top middle point, half the cage's height up.
    let top = cage.index([1, 2, 1]);
    cage.moved[top] = [0.0, 0.5, 0.0];
    let p = Params { lattice: cage.clone(), ..params(Kind::Lattice) };
    let d = Deformer::new(&p, &bounds).unwrap();
    let span = bounds.span(1, [0.0, 1.0]);
    let tip = moved(&d, DVec3::new(0.0, 2.0, 0.0));
    // At the top middle the weight of that point is Bᵢ(½)·B₂(1)·Bₖ(½) = ½·1·½.
    assert!(tip.distance(DVec3::new(0.0, 2.0 + 0.25 * 2.0 * 0.5, 0.0)) < 1e-9, "{tip}");
    assert!(d.apply(DVec3::new(0.0, 0.0, 0.0), DVec3::ZERO).unwrap().distance(DVec3::ZERO) < 1e-12);
    // A drag of the handle to a point puts the setting that moves it there.
    let to = lattice::point(&cage, top, &bounds, 1, span) + DVec3::X * 0.3;
    let back = lattice::moved_for(&cage, top, &bounds, 1, span, to);
    let mut again = cage.clone();
    again.moved[top] = back;
    assert!(lattice::point(&again, top, &bounds, 1, span).distance(to) < 1e-6);
}

#[test]
fn a_bend_in_the_mirror_plane_keeps_mirror_images_mirror_images_to_the_bit() {
    // A model symmetric across X = 0, bent along Y toward Z: the bend's plane
    // is the mirror plane, so a point and its mirror image land on mirror images.
    let points = cylinder(2.0, 0.25);
    let bounds = bounds_of(&points);
    let flip = |q: DVec3| DVec3::new(-q.x, q.y, q.z);
    for kind in [Kind::Bend, Kind::Taper, Kind::Stretch, Kind::Flatten] {
        let p = Params { angle: 75.0, scales: [1.0, 0.3], stretch: 1.6, flatten: 0.4, softness: 0.4, ..params(kind) };
        let d = Deformer::new(&p, &bounds).unwrap();
        for &(q, _) in &points {
            let (a, b) = (moved(&d, q), moved(&d, flip(q)));
            assert_eq!(flip(a).to_array(), b.to_array(), "{kind:?} at {q}");
        }
    }
    // A twist about an axis in the mirror plane cannot be: its mirror image turns the other way.
    let d = Deformer::new(&Params { angle: 75.0, ..params(Kind::Twist) }, &bounds).unwrap();
    let q = DVec3::new(0.25, 1.0, 0.0);
    assert!(flip(moved(&d, q)).distance(moved(&d, flip(q))) > 0.1);
}

#[test]
fn settings_out_of_range_are_brought_back_into_it() {
    let wild = Params {
        axis: 7,
        range: [f32::NAN, -3.0],
        softness: 9.0,
        angle: f32::INFINITY,
        stretch: 0.0,
        lattice: Lattice { count: [1, 9, 4], moved: vec![], picked: 99 },
        ..Params::default()
    };
    let p = wild.sanitized();
    assert_eq!((p.axis, p.range, p.softness, p.angle, p.stretch), (2, [0.0, 0.0], 1.0, 0.0, 0.05));
    assert_eq!((p.lattice.count, p.lattice.len(), p.lattice.moved.len()), ([2, 6, 4], 48, 48));
    assert!(p.lattice.picked < 48);
    // A range of no length does nothing rather than divide by it.
    let bounds = Bounds::new(DVec3::ZERO, DVec3::ONE);
    assert!(Deformer::new(&Params { kind: Kind::Stretch, stretch: 2.0, range: [0.4, 0.4], ..p }, &bounds).is_none());
}
