//! Tests for the layout maths and what is built on it: the Unity app's offline
//! array suite (GeometryHarness, "array") and the parts of its ArrayRepeaterTests
//! that need no scene, then checks of this crate's own.
//!
//! Not ported from ArrayRepeaterTests, because each needs a live Unity scene
//! object (SculptableMesh, its renderers, MirrorRepeater or raycasts):
//! AMirroredObjectsArrayIsMirroredToo (the mirror's reflected views belong to the
//! symmetry crate; a reflecting copy baking inside out is covered in `bake`),
//! RaycastHitsACopyAndReportsItsFrame, and the view-handling halves of the rest
//! (a copy drawing the object's own mesh, views being added and removed, the copy
//! list and labels).

use glam::{EulerRot, Mat3, Mat4, Quat, Vec3};

use crate::layout::{axis_vector, compute, radial_arc, scale_at};
use crate::path;
use crate::testing::{Rng, angle_deg, is_finite, is_unit, uv_sphere};
use crate::{ArrayKind, ArrayPlacement, ArraySettings, ArrayState, MAX_COMBINED_VERTICES, Pose, bake_array};
use crate::{bake_normals, local_matrices};

fn near(expected: f64, got: f64, tol: f64, what: &str) {
    assert!((expected - got).abs() <= tol, "{what}: expected {expected}, got {got} (tolerance {tol})");
}

fn near_vec(expected: Vec3, got: Vec3, tol: f32, what: &str) {
    assert!((expected - got).length() <= tol, "{what}: expected {expected}, got {got} (tolerance {tol})");
}

fn check_first_is_object(p: &[ArrayPlacement], what: &str) {
    near(0.0, p[0].offset.length() as f64, 1e-6, &format!("{what}: copy 0 offset"));
    near(0.0, angle_deg(p[0].rotation), 1e-3, &format!("{what}: copy 0 rotation"));
    near(1.0, p[0].scale as f64, 1e-6, &format!("{what}: copy 0 scale"));
}

fn inverse(q: Quat) -> Quat {
    q.conjugate()
}

// ================================================================== the offline suite

#[test]
fn line() {
    let mut s = ArraySettings::new(ArrayKind::Line);
    s.count = 5;
    let end = Vec3::new(8.0, 2.0, -4.0);
    let p = compute(&s, &[Vec3::ZERO, end]);

    assert_eq!(p.len(), 5, "line: count");
    check_first_is_object(&p, "line");
    for (i, c) in p.iter().enumerate() {
        near_vec(end * (i as f32 / 4.0), c.offset, 1e-4, &format!("line: copy {i} evenly spaced"));
        near(0.0, angle_deg(c.rotation), 1e-2, &format!("line: copy {i} keeps the object's facing"));
        near(1.0, c.scale as f64, 1e-6, &format!("line: copy {i} size"));
    }

    // The path starts on the object wherever points[0] says that is.
    let shifted = Vec3::new(3.0, 3.0, 3.0);
    let q = compute(&s, &[shifted, shifted + end]);
    near_vec(end, q[4].offset, 1e-4, "line: offsets are relative to the first point");
}

#[test]
fn radial() {
    let mut s = ArraySettings::new(ArrayKind::Radial);
    s.count = 8;
    s.axis = 1;
    let centre = Vec3::new(-3.0, 0.5, 0.0);
    let mut p = compute(&s, &[Vec3::ZERO, centre]);

    assert_eq!(p.len(), 8, "radial: count");
    check_first_is_object(&p, "radial");
    let radius = Vec3::new(centre.x, 0.0, centre.z).length();
    for (i, c) in p.iter().enumerate() {
        let from_centre = c.offset - centre;
        near(
            radius as f64,
            Vec3::new(from_centre.x, 0.0, from_centre.z).length() as f64,
            1e-4,
            &format!("radial: copy {i} on the circle"),
        );
        near(-centre.y as f64, from_centre.y as f64, 1e-4, &format!("radial: copy {i} stays at its height"));
        let mut expected = i as f64 * 45.0;
        if expected > 180.0 {
            expected = 360.0 - expected;
        }
        near(expected, angle_deg(c.rotation), 1e-2, &format!("radial: copy {i} turned by its angle"));
    }
    // Neighbours are all the same distance apart, the last and the first included.
    let gap = (p[1].offset - p[0].offset).length();
    for i in 0..p.len() {
        near(gap as f64, (p[(i + 1) % 8].offset - p[i].offset).length() as f64, 1e-4, &format!("radial: gap {i}"));
    }

    // A partial sweep ends exactly on the sweep angle.
    s.sweep = 90.0;
    s.count = 4;
    p = compute(&s, &[Vec3::ZERO, centre]);
    near(90.0, angle_deg(p[3].rotation), 1e-2, "radial: 90 degree sweep ends at 90");
    near(30.0, angle_deg(p[1].rotation), 1e-2, "radial: 90 degree sweep steps by 30");

    // Not following: placed round the circle but all facing the object's way.
    s.follow = false;
    p = compute(&s, &[Vec3::ZERO, centre]);
    near(0.0, angle_deg(p[3].rotation), 1e-3, "radial: follow off keeps the facing");
    assert!(p[3].offset.length() > 1.0, "radial: follow off still moves the copies");

    // Other axes.
    s = ArraySettings::new(ArrayKind::Radial);
    s.count = 4;
    s.axis = 0;
    p = compute(&s, &[Vec3::ZERO, Vec3::new(0.0, 2.0, 0.0)]);
    for (i, c) in p.iter().enumerate() {
        near(0.0, c.offset.x as f64, 1e-5, &format!("radial X: copy {i} stays in the YZ plane"));
    }
    near_vec(Vec3::new(0.0, 4.0, 0.0), p[2].offset, 1e-4, "radial X: opposite copy is across the centre");
}

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
fn curve() {
    let mut s = ArraySettings::new(ArrayKind::Curve);
    s.count = 9;
    let points = wave();

    let sampled = path::sample(&points, false);
    let (dense, arc, length) = (&sampled.points, &sampled.arc, sampled.length());
    assert!(length > 8.0 && length < 14.0, "curve: plausible length ({length})");

    let mut p = compute(&s, &points);
    check_first_is_object(&p, "curve");
    near_vec(points[4], p[8].offset, 1e-4, "curve: last copy on the last point");

    // Evenly spaced by arc length: consecutive chords are close to equal and never
    // longer than the arc between them.
    let step = length / 8.0;
    for i in 1..p.len() {
        let chord = (p[i].offset - p[i - 1].offset).length();
        assert!(chord <= step * 1.0005, "curve: chord {i} no longer than its arc");
        assert!(chord > step * 0.9, "curve: chord {i} close to its arc ({chord} vs {step})");
    }

    // Following: each copy's rotation carries the start tangent onto the tangent there.
    let t0 = (dense[1] - dense[0]).normalize();
    for (i, c) in p.iter().enumerate() {
        assert!(is_unit(c.rotation), "curve: copy {i} rotation is unit");
        let (k, _) = path::locate(arc, length * i as f32 / 8.0);
        let tangent = (dense[(k + 1).min(dense.len() - 1)] - dense[k]).normalize();
        let carried = c.rotation * t0;
        assert!(carried.dot(tangent) > 0.99, "curve: copy {i} faces along the path ({})", carried.dot(tangent));
    }

    // A planar curve turns its copies about the plane's normal only: no roll.
    let planar = [Vec3::ZERO, Vec3::new(2.0, 2.0, 0.0), Vec3::new(5.0, 2.0, 0.0), Vec3::new(7.0, 0.0, 0.0)];
    p = compute(&s, &planar);
    for (i, c) in p.iter().enumerate() {
        let normal = c.rotation * Vec3::Z;
        assert!(normal.z > 0.9999, "curve: planar path does not roll copy {i} ({})", normal.z);
    }

    s.follow = false;
    p = compute(&s, &points);
    for (i, c) in p.iter().enumerate() {
        near(0.0, angle_deg(c.rotation), 1e-3, &format!("curve: follow off, copy {i}"));
    }
}

#[test]
fn closed_loop() {
    let mut s = ArraySettings::new(ArrayKind::Curve);
    s.closed = true;
    s.count = 12;

    // A square of points closes into a rounded loop in the XZ plane.
    let ring = [Vec3::ZERO, Vec3::new(4.0, 0.0, 0.0), Vec3::new(4.0, 0.0, 4.0), Vec3::new(0.0, 0.0, 4.0)];
    let mut p = compute(&s, &ring);
    check_first_is_object(&p, "loop");

    let sampled = path::sample(&ring, true);
    let length = sampled.length();
    near_vec(sampled.points[0], *sampled.points.last().unwrap(), 1e-5, "loop: path ends where it starts");

    // Twelve copies leave twelve equal gaps, the one back to the object included.
    let step = length / 12.0;
    for i in 0..12 {
        let chord = (p[(i + 1) % 12].offset - p[i].offset).length();
        assert!(chord <= step * 1.0005 && chord > step * 0.85, "loop: gap {i} ({chord} vs {step})");
    }

    // Planar loop: every copy turns about Y only, and the turn adds up to a full circle.
    for (i, c) in p.iter().enumerate() {
        assert!((c.rotation * Vec3::Y).y > 0.9999, "loop: copy {i} stays upright");
    }
    near(180.0, angle_deg(p[6].rotation), 2.0, "loop: the opposite copy faces the other way");

    // A loop that leaves the plane: the frame carried once round comes back off by some
    // angle, and the correction must leave the last copy turning smoothly into the first.
    let saddle = [
        Vec3::ZERO,
        Vec3::new(3.0, 2.0, 0.0),
        Vec3::new(5.0, 0.0, 3.0),
        Vec3::new(3.0, -2.0, 6.0),
        Vec3::new(0.0, 0.0, 7.0),
        Vec3::new(-2.0, 2.0, 3.0),
    ];
    s.count = 60;
    p = compute(&s, &saddle);
    let mut worst = 0.0f64;
    for i in 0..60 {
        let delta = p[(i + 1) % 60].rotation * inverse(p[i].rotation);
        worst = worst.max(angle_deg(delta));
    }
    assert!(worst <= 25.0, "loop: no jump in facing anywhere round a twisted loop ({worst})");

    // Open and closed differ: the open path stops on the last point.
    s.closed = false;
    s.count = 4;
    p = compute(&s, &ring);
    near_vec(ring[3], p[3].offset, 1e-4, "loop: opened again, last copy on the last point");
}

#[test]
fn taper() {
    let mut s = ArraySettings::new(ArrayKind::Line);
    s.count = 6;
    s.end_scale = 0.25;
    s.space_by_size = false;
    let end = Vec3::new(10.0, 0.0, 0.0);
    let mut p = compute(&s, &[Vec3::ZERO, end]);

    near(0.25, p[5].scale as f64, 1e-5, "taper: last copy is End Size");
    for i in 1..6 {
        assert!(p[i].scale < p[i - 1].scale, "taper: copy {i} smaller than the one before");
    }
    near(2.0, p[1].offset.x as f64, 1e-4, "taper: even spacing when not spacing by size");

    // Spaced by size: every gap is the same multiple of the two sizes either side.
    s.space_by_size = true;
    p = compute(&s, &[Vec3::ZERO, end]);
    near(10.0, p[5].offset.x as f64, 1e-4, "taper: still ends on the end point");
    let ratio = (p[1].offset.x - p[0].offset.x) / (p[0].scale + p[1].scale);
    for i in 1..5 {
        let r = (p[i + 1].offset.x - p[i].offset.x) / (p[i].scale + p[i + 1].scale);
        near(ratio as f64, r as f64, 1e-3, &format!("taper: gap {i} in proportion to its copies"));
    }

    // Growing instead.
    s.end_scale = 3.0;
    p = compute(&s, &[Vec3::ZERO, end]);
    near(3.0, p[5].scale as f64, 1e-5, "expand: last copy is End Size");
    assert!(p[3].scale > p[2].scale, "expand: grows along the run");

    // Middle swell: biggest in the middle, both ends untouched.
    s.end_scale = 1.0;
    s.mid_scale = 2.0;
    s.count = 7;
    p = compute(&s, &[Vec3::ZERO, end]);
    near(1.0, p[0].scale as f64, 1e-5, "swell: first copy unchanged");
    near(1.0, p[6].scale as f64, 1e-4, "swell: last copy unchanged");
    near(2.0, p[3].scale as f64, 1e-4, "swell: middle copy is Middle Size");
    assert!(p[1].scale > 1.0 && p[1].scale < p[2].scale, "swell: rises toward the middle");
}

/// points[0] is where the path starts relative to the object. When that is not zero the
/// object sits off the path, and every copy must sit off it the same way in its own frame
/// (turned with the path, scaled with the copy): what posing one copy with the gizmo edits.
#[test]
fn off_the_path() {
    let start = Vec3::new(0.4, -0.7, 0.25);
    let shift = -start;

    // Curve, following and tapering: each copy is its place on the path plus the shift
    // carried into that copy's frame and size.
    let mut s = ArraySettings::new(ArrayKind::Curve);
    s.count = 9;
    s.end_scale = 0.5;
    let rel = wave();
    let moved: Vec<Vec3> = rel.iter().map(|&p| p + start).collect();

    let on = compute(&s, &rel);
    let off = compute(&s, &moved);
    check_first_is_object(&off, "off path");
    for i in 0..on.len() {
        let expected = start + on[i].offset + on[i].rotation * (shift * on[i].scale);
        near_vec(expected, off[i].offset, 1e-4, &format!("off path: copy {i} carries the shift in its own frame"));
        // Loose: the angle comes from an acos next to 1, where float rounding alone reads as hundredths of a degree.
        near(
            0.0,
            angle_deg(off[i].rotation * inverse(on[i].rotation)),
            0.1,
            &format!("off path: copy {i} faces as before"),
        );
        near(on[i].scale as f64, off[i].scale as f64, 1e-6, &format!("off path: copy {i} size as before"));
        // Always the same distance off the path, for its size.
        let distance = (off[i].offset - (start + on[i].offset)).length();
        near(
            (shift.length() * on[i].scale) as f64,
            distance as f64,
            1e-4,
            &format!("off path: copy {i} is the shift's length off the path"),
        );
    }

    // Not following and all one size: the whole run just moves as one, so relative to the
    // object nothing changes at all.
    s.follow = false;
    s.end_scale = 1.0;
    let on = compute(&s, &rel);
    let off = compute(&s, &moved);
    for i in 0..on.len() {
        near_vec(on[i].offset, off[i].offset, 1e-4, &format!("off path: follow off, copy {i} unchanged"));
    }

    // Circle: pushing the object out from the path pushes every copy out along its own
    // spoke, so they are all still one distance from the axis.
    let mut r = ArraySettings::new(ArrayKind::Radial);
    r.count = 6;
    r.axis = 1;
    let centre = Vec3::new(-3.0, 0.0, 0.0);
    let outward = Vec3::new(-0.5, 0.2, 0.0);
    let ring = compute(&r, &[outward, outward + centre]);
    check_first_is_object(&ring, "off circle");
    let axis_point = outward + centre;
    let radius = Vec3::new(axis_point.x, 0.0, axis_point.z).length();
    near(3.5, radius as f64, 1e-5, "off circle: the object is further from the axis");
    for (i, c) in ring.iter().enumerate() {
        let from_axis = c.offset - axis_point;
        near(
            radius as f64,
            Vec3::new(from_axis.x, 0.0, from_axis.z).length() as f64,
            1e-4,
            &format!("off circle: copy {i} at the object's radius"),
        );
        near(-axis_point.y as f64, from_axis.y as f64, 1e-4, &format!("off circle: copy {i} at the object's height"));
    }
}

#[test]
fn degenerate() {
    assert!(compute(&ArraySettings::default(), &[Vec3::ZERO]).is_empty(), "none: no placements");

    // A line with no length, a single point, no points at all: copies stack on the object
    // rather than producing NaNs.
    let mut s = ArraySettings::new(ArrayKind::Line);
    let stacks: [&[Vec3]; 3] = [&[Vec3::ZERO, Vec3::ZERO], &[Vec3::ZERO], &[]];
    for pts in stacks {
        let p = compute(&s, pts);
        assert_eq!(p.len(), 5, "degenerate: count kept");
        for a in &p {
            assert!(is_finite(a) && a.offset.length() < 1e-6, "degenerate: stacked on the object");
        }
    }

    // Out-of-range settings are clamped, not trusted.
    s.count = 100_000;
    s.end_scale = -5.0;
    let q = compute(&s, &[Vec3::ZERO, Vec3::X]);
    assert_eq!(q.len(), ArraySettings::MAX_COUNT as usize, "clamp: count");
    assert!(q[q.len() - 1].scale > 0.0, "clamp: size stays positive");

    // A path that doubles straight back on itself still gives usable rotations.
    let mut c = ArraySettings::new(ArrayKind::Curve);
    c.count = 20;
    for a in compute(&c, &[Vec3::ZERO, Vec3::new(3.0, 0.0, 0.0), Vec3::ZERO]) {
        assert!(is_finite(&a) && is_unit(a.rotation), "doubled back: finite, unit rotation");
    }

    // Repeated points.
    let repeated =
        [Vec3::ZERO, Vec3::ZERO, Vec3::new(2.0, 0.0, 0.0), Vec3::new(2.0, 0.0, 0.0), Vec3::new(4.0, 1.0, 0.0)];
    for a in compute(&c, &repeated) {
        assert!(is_finite(&a) && is_unit(a.rotation), "repeated points: finite, unit rotation");
    }
}

#[test]
fn fuzz() {
    let mut rng = Rng::new(1234);
    let mut bad = 0;
    for _ in 0..2000 {
        let kind = ArrayKind::from_id(rng.int(1, 4));
        let mut s = ArraySettings::new(kind);
        s.count = rng.int(2, 60);
        s.end_scale = rng.float(0.05, 4.0);
        s.mid_scale = rng.float(0.05, 4.0);
        s.closed = rng.coin();
        s.follow = rng.coin();
        s.space_by_size = rng.coin();
        s.axis = rng.int(0, 3);
        s.sweep = rng.float(1.0, 360.0);

        let n = if kind == ArrayKind::Curve { rng.int(2, 9) } else { 2 };
        let mut points = vec![Vec3::ZERO; n as usize];
        for p in points.iter_mut().skip(1) {
            *p = Vec3::new(rng.float(-5.0, 5.0), rng.float(-5.0, 5.0), rng.float(-5.0, 5.0));
        }

        let p = compute(&s, &points);
        if p.len() != s.count as usize {
            bad += 1;
            continue;
        }
        if p[0].offset.length() > 1e-6 || p[0].scale != 1.0 {
            bad += 1;
        }
        if p.iter().any(|a| !is_finite(a) || !is_unit(a.rotation) || a.scale <= 0.0 || a.offset.length() > 200.0) {
            bad += 1;
        }
    }
    assert_eq!(bad, 0, "fuzz: 2000 random arrays all finite, unit-rotated and anchored on the object");
}

// ============================================= ArrayRepeaterTests, the parts without a scene

const POSITION: Vec3 = Vec3::new(0.7, 0.2, -0.3);
const SCALE: f32 = 0.8;

/// Unity's `Quaternion.Euler(10, 30, -20)`: Z first, then X, then Y.
fn rotation() -> Quat {
    Quat::from_euler(EulerRot::YXZ, 30f32.to_radians(), 10f32.to_radians(), (-20f32).to_radians())
}

fn object() -> Pose {
    Pose::new(POSITION, rotation(), Vec3::splat(SCALE))
}

fn line_state(count: i32, end: Vec3, end_scale: f32) -> ArrayState {
    let mut settings = ArraySettings::new(ArrayKind::Line);
    settings.count = count;
    settings.end_scale = end_scale;
    settings.space_by_size = false;
    ArrayState { settings, points: vec![Vec3::ZERO, end] }
}

fn layout_of(state: &ArrayState) -> Vec<ArrayPlacement> {
    compute(&state.settings, &state.points)
}

#[test]
fn no_array_means_no_copies() {
    assert!(ArrayState::none().is_none());
    assert!(layout_of(&ArrayState::none()).is_empty());
}

#[test]
fn copies_lie_along_the_line_in_the_objects_own_facing_and_size() {
    let end = Vec3::new(4.0, 0.0, 2.0);
    let p = layout_of(&line_state(5, end, 1.0));
    assert_eq!(p.len(), 5, "one placement per copy, the object included");

    for (i, placement) in p.iter().enumerate().skip(1) {
        let copy = placement.apply(&object());
        near_vec(POSITION + end * (i as f32 / 4.0), copy.position, 1e-4, &format!("copy {i} position"));
        near(0.0, angle_deg(copy.rotation * inverse(rotation())), 0.01, &format!("copy {i} rotation"));
        near(SCALE as f64, copy.scale.x as f64, 1e-5, &format!("copy {i} scale"));
    }
}

#[test]
fn a_path_starting_off_the_object_holds_every_copy_off_it_the_same_way() {
    // What posing one copy with the gizmo edits: the object sits off the path's start, and
    // every copy off its own place on the path.
    let mut settings = ArraySettings::new(ArrayKind::Radial);
    settings.count = 4;
    settings.axis = 1;
    let centre = Vec3::new(-2.0, 0.0, 0.0);
    let first = compute(&settings, &[Vec3::ZERO, centre]);
    let opposite = first[2].apply(&object()).position;

    // The object pushed 0.5 further from the centre, the centre left where it was.
    let push = Vec3::new(0.5, 0.0, 0.0);
    let pushed = Pose { position: POSITION + push, ..object() };
    let second = compute(&settings, &[-push, centre - push]);

    near_vec(
        opposite - push,
        second[2].apply(&pushed).position,
        1e-4,
        "the copy across the ring moved outward too, the other way",
    );
    let axis_point = POSITION + centre;
    for (i, placement) in second.iter().enumerate() {
        let d = (placement.apply(&pushed).position - axis_point).length();
        near(2.5, d as f64, 1e-4, &format!("copy {i} radius"));
    }
}

#[test]
fn changing_the_count_changes_the_copies() {
    let right2 = Vec3::X * 2.0;
    assert_eq!(layout_of(&line_state(3, right2, 1.0)).len(), 3);
    assert_eq!(layout_of(&line_state(8, right2, 1.0)).len(), 8);
    let two = layout_of(&line_state(2, right2, 1.0));
    assert_eq!(two.len(), 2);
    near_vec(POSITION + right2, two[1].apply(&object()).position, 1e-4, "the only other copy is at the end");
}

#[test]
fn moving_the_object_carries_the_copies() {
    let end = Vec3::new(0.0, 3.0, 0.0);
    let p = layout_of(&line_state(4, end, 0.5));

    let moved = Pose { position: Vec3::new(-2.0, 1.0, 5.0), scale: Vec3::splat(2.0), ..object() };
    near_vec(moved.position + end, p[3].apply(&moved).position, 1e-4, "last copy");
    near(1.0, p[3].apply(&moved).scale.x as f64, 1e-5, "tapered copy scales with the object");
    let first = p[1].apply(&moved).scale.x;
    assert!(first < 2.0 && first > 1.0, "{first}");
}

#[test]
fn circle_turns_each_copy_about_the_axis() {
    let mut settings = ArraySettings::new(ArrayKind::Radial);
    settings.count = 4;
    settings.axis = 1;
    let centre = Vec3::new(-2.0, 0.0, 0.0);
    let p = compute(&settings, &[Vec3::ZERO, centre]);

    assert_eq!(p.len(), 4);
    // Half way round: straight across the centre, turned 180 degrees about Y.
    let opposite = p[2].apply(&object());
    near_vec(POSITION + centre * 2.0, opposite.position, 1e-4, "across the centre");
    let expected = Quat::from_axis_angle(Vec3::Y, std::f32::consts::PI) * rotation();
    near(0.0, angle_deg(opposite.rotation * inverse(expected)), 0.01, "turned half way round");
}

#[test]
fn state_round_trips_and_no_array_removes_it() {
    let mut settings = ArraySettings::new(ArrayKind::Curve);
    settings.count = 7;
    settings.end_scale = 0.4;
    settings.mid_scale = 1.5;
    settings.closed = true;
    let state = ArrayState {
        settings,
        points: vec![Vec3::ZERO, Vec3::new(2.0, 1.0, 0.0), Vec3::new(3.0, -1.0, 2.0), Vec3::new(0.0, 0.0, 3.0)],
    };
    // Apply stores the sanitized state and Capture hands it back: what went in is what comes out.
    assert_eq!(state.sanitized(), state, "what was applied is what is captured");
    assert_eq!(ArrayState::none().sanitized(), ArrayState::none());
}

#[test]
fn combined_geometry_is_every_copy_where_it_was_drawn() {
    // Non-uniform, the hard case.
    let pose = Pose::new(POSITION, rotation(), Vec3::new(0.8, 1.3, 0.6));
    let mut state = line_state(4, Vec3::new(3.0, 1.0, -2.0), 0.5);
    state.settings.follow = true;
    let placements = layout_of(&state);

    let sphere = uv_sphere(24, 48, 1.0);
    let normals: Vec<Vec3> = sphere.positions.iter().map(|p| p.normalize()).collect();
    let (v, t) = (sphere.vertex_count(), sphere.indices.len());

    let baked = bake_array(&sphere, &placements, &pose).unwrap();
    assert_eq!(baked.vertex_count(), v * 4);
    assert_eq!(baked.indices.len(), t * 4);
    let matrices = local_matrices(&placements, &pose);
    let baked_normals = bake_normals(&normals, &matrices).unwrap();

    let to_world = pose.matrix();
    let object_normals = Mat3::from_mat4(to_world).inverse().transpose();
    let bounds = baked.bounds();
    for (c, placement) in placements.iter().enumerate() {
        let frame = placement.world_matrix(&pose);
        let frame_normals = Mat3::from_mat4(frame).inverse().transpose();
        for i in (0..v).step_by(23) {
            // The combined vertex, through the object's transform, is where the copy drew it.
            let local = baked.positions[c * v + i];
            near_vec(
                frame.transform_point3(sphere.positions[i]),
                to_world.transform_point3(local),
                1e-4,
                &format!("copy {c} vertex {i}"),
            );
            assert!(bounds.contains(local), "bounds hold every vertex");

            let world_normal = (object_normals * baked_normals[c * v + i]).normalize();
            let expected_normal = (frame_normals * normals[i]).normalize();
            assert!(world_normal.angle_between(expected_normal).to_degrees() < 0.1, "copy {c} normal {i}");
        }
        for i in (0..t).step_by(31) {
            assert_eq!(baked.indices[c * t + i], (c * v) as u32 + sphere.indices[i]);
        }
    }

    // Copy 0 is the object's own geometry, untouched.
    assert_eq!(baked.positions[..v], sphere.positions[..]);
    assert_eq!(baked.indices[..t], sphere.indices[..]);
}

#[test]
fn the_maximum_count_still_combines() {
    let state = line_state(ArraySettings::MAX_COUNT, Vec3::X, 1.0);
    let placements = layout_of(&state);
    assert_eq!(placements.len(), ArraySettings::MAX_COUNT as usize);
    let sphere = uv_sphere(24, 48, 1.0);
    let baked = bake_array(&sphere, &placements, &object()).unwrap();
    assert_eq!(baked.vertex_count(), sphere.vertex_count() * ArraySettings::MAX_COUNT as usize);
    assert!((baked.vertex_count() as u64) < MAX_COMBINED_VERTICES);
}

// ======================================================================= this crate's own

/// Angle, in degrees (-180 to 180), that `to` is turned from `from` about `axis` (right-hand rule).
fn turn_about(axis: Vec3, from: Vec3, to: Vec3) -> f64 {
    let (a, b) = (from - axis * from.dot(axis), to - axis * to.dot(axis));
    axis.dot(a.cross(b)).atan2(a.dot(b)).to_degrees() as f64
}

/// `near` for angles in degrees, where 359.99 is close to 0.
fn near_angle(expected: f64, got: f64, tol: f64, what: &str) {
    let d = (expected - got).rem_euclid(360.0);
    near(0.0, d.min(360.0 - d), tol, what);
}

#[test]
fn n_copies_on_a_circle_are_evenly_spaced_and_at_the_right_radius() {
    let object = Pose::new(Vec3::new(1.5, -2.0, 0.5), rotation(), Vec3::ONE);
    for axis in 0..3 {
        let axis_dir = axis_vector(axis);
        for count in [2, 3, 5, 8, 13, 100] {
            // A centre that is not on the axis and not in the object's own plane.
            let centre = Vec3::new(-2.5, 1.25, 1.5) + axis_dir * 0.7;
            let mut s = ArraySettings::new(ArrayKind::Radial);
            s.count = count;
            s.axis = axis;
            let p = compute(&s, &[Vec3::ZERO, centre]);
            assert_eq!(p.len(), count as usize);

            let centre_world = object.position + centre;
            let spoke = object.position - centre_world;
            let radius = (spoke - axis_dir * spoke.dot(axis_dir)).length();
            let step = 360.0 / count as f64;
            for i in 0..p.len() {
                let here = p[i].apply(&object).position;
                let from_axis = here - centre_world;
                near(
                    radius as f64,
                    (from_axis - axis_dir * from_axis.dot(axis_dir)).length() as f64,
                    2e-4,
                    &format!("axis {axis}, {count} copies: copy {i} radius"),
                );
                near(
                    spoke.dot(axis_dir) as f64,
                    from_axis.dot(axis_dir) as f64,
                    2e-4,
                    &format!("copy {i} height along the axis"),
                );

                let next = p[(i + 1) % p.len()].apply(&object).position - centre_world;
                near_angle(
                    step,
                    turn_about(axis_dir, from_axis, next),
                    1e-2,
                    &format!("axis {axis}, {count} copies: gap {i}"),
                );
            }
        }
    }
}

#[test]
fn a_partial_circle_ends_on_its_sweep() {
    let mut s = ArraySettings::new(ArrayKind::Radial);
    s.count = 7;
    s.sweep = 210.0;
    s.axis = 2;
    let centre = Vec3::new(0.0, -4.0, 0.0);
    let p = compute(&s, &[Vec3::ZERO, centre]);
    let spoke = |c: &ArrayPlacement| c.offset - centre;
    for (i, c) in p.iter().enumerate() {
        near_angle(35.0 * i as f64, turn_about(Vec3::Z, spoke(&p[0]), spoke(c)), 1e-2, &format!("copy {i}"));
    }
    // Sweeps of 360 spread evenly all the way round, so the last copy stops one gap short of the first.
    s.sweep = 360.0;
    let full = compute(&s, &[Vec3::ZERO, centre]);
    near_angle(360.0 / 7.0 * 6.0, turn_about(Vec3::Z, spoke(&full[0]), spoke(&full[6])), 1e-2, "full sweep");
}

/// The picture `Design/Array/array_circle.png`: the object at the right, the centre to its left,
/// seen from the front. The copies go round counter-clockwise on screen from the object (the
/// next one is above it), and tapering shrinks them that way round. Seen from +Z, as the front
/// view of this app is, a turn about +Z is counter-clockwise, so the layout needs no sign change.
#[test]
fn the_circle_runs_counter_clockwise_as_the_design_picture_does() {
    let mut s = ArraySettings::new(ArrayKind::Radial);
    s.count = 8;
    s.axis = 2;
    s.end_scale = 0.5;
    let p = compute(&s, &[Vec3::ZERO, Vec3::new(-3.0, 0.0, 0.0)]);

    // Offsets from the object on screen (x right, y up); the centre is 3 to its left.
    let at = |i: usize| p[i].offset;
    assert!(at(1).y > 0.0 && at(1).x < 0.0, "copy 1 is up and to the left of the object: {}", at(1));
    assert!(at(2).y > at(1).y && at(2).x < at(1).x, "copy 2 further round the top");
    near_vec(Vec3::new(-3.0, 3.0, 0.0), at(2), 1e-4, "a quarter of the way round, straight above the centre");
    assert!(p[7].offset.y < 0.0, "the last copy comes up from below to the object");
    for i in 1..8 {
        assert!(p[i].scale < p[i - 1].scale, "sizes shrink the way round, as in the picture");
    }
    assert!(turn_about(Vec3::Z, Vec3::X, p[1].rotation * Vec3::X) > 0.0, "each copy turns counter-clockwise too");
}

#[test]
fn a_line_arrays_last_copy_is_where_the_settings_say() {
    let mut rng = Rng::new(7);
    for _ in 0..300 {
        let start = Vec3::new(rng.float(-3.0, 3.0), rng.float(-3.0, 3.0), rng.float(-3.0, 3.0));
        let end = start + Vec3::new(rng.float(-9.0, 9.0), rng.float(-9.0, 9.0), rng.float(-9.0, 9.0));
        let mut s = ArraySettings::new(ArrayKind::Line);
        s.count = rng.int(2, 101);
        s.end_scale = rng.float(0.1, 4.0);
        s.mid_scale = rng.float(0.1, 4.0);
        s.space_by_size = rng.coin();
        s.follow = rng.coin();

        let p = compute(&s, &[start, end]);
        let last = p.last().unwrap();
        // The path starts `start` away from the object, so every copy sits that far off it, scaled
        // with its size; a straight path turns nothing, so the shift is not rotated.
        let expected = (end - start) + start * (1.0 - last.scale);
        near_vec(expected, last.offset, 2e-3, "last copy at the end point");
        near(s.end_scale as f64, last.scale as f64, 1e-5, "last copy at the end size");
    }
}

#[test]
fn a_line_arrays_last_copy_is_exactly_the_end_when_the_path_starts_on_the_object() {
    let end = Vec3::new(5.0, -3.0, 2.0);
    for (count, end_scale, by_size) in [(2, 1.0, true), (9, 0.3, true), (9, 3.0, false), (100, 0.05, true)] {
        let mut s = ArraySettings::new(ArrayKind::Line);
        s.count = count;
        s.end_scale = end_scale;
        s.space_by_size = by_size;
        let p = compute(&s, &[Vec3::ZERO, end]);
        near_vec(
            end,
            p[count as usize - 1].offset,
            1e-4,
            &format!("{count} copies, end {end_scale}, by size {by_size}"),
        );
        for pair in p.windows(2) {
            assert!((pair[1].offset - pair[0].offset).dot(end) > 0.0, "copies advance toward the end");
        }
    }
}

#[test]
fn a_curve_arrays_last_copy_is_on_its_last_point_even_when_it_swells() {
    let mut s = ArraySettings::new(ArrayKind::Curve);
    s.count = 31;
    s.end_scale = 0.2;
    s.mid_scale = 2.5;
    let points = wave();
    let p = compute(&s, &points);
    near_vec(points[4], p[30].offset, 1e-4, "last copy");
    check_first_is_object(&p, "swelling curve");
}

#[test]
fn the_ring_drawn_for_a_circle_goes_through_the_object_and_round_the_axis() {
    let mut s = ArraySettings::new(ArrayKind::Radial);
    s.axis = 1;
    s.sweep = 270.0;
    let points = [Vec3::new(0.0, 0.5, 0.0), Vec3::new(-2.0, 0.5, 0.0)];
    let arc = radial_arc(&s, &points, 96);
    assert_eq!(arc.len(), 97);
    near_vec(points[0], arc[0], 1e-5, "starts at the path's first point");
    let centre = points[1];
    for (i, p) in arc.iter().enumerate() {
        near(2.0, (*p - centre).length() as f64, 1e-4, &format!("arc point {i} on the circle"));
        near(0.0, (p.y - centre.y) as f64, 1e-5, &format!("arc point {i} in the plane"));
    }
    near_angle(270.0, turn_about(Vec3::Y, arc[0] - centre, arc[96] - centre), 1e-2, "ends at the sweep");

    // The copies sit on this ring when the sweep is partial.
    s.count = 4;
    let p = compute(&s, &points);
    for (i, c) in p.iter().enumerate() {
        let on_ring = arc.iter().any(|a| (*a - (points[0] + c.offset)).length() < 1e-3);
        assert!(on_ring, "copy {i} lies on the ring");
    }
}

#[test]
fn posing_a_copy_moves_it_where_asked_and_leaves_the_path_where_it_was() {
    let mut s = ArraySettings::new(ArrayKind::Curve);
    s.count = 9;
    s.end_scale = 0.5;
    let pose = object();
    let state = ArrayState { settings: s, points: wave() };
    let before = layout_of(&state);

    for copy in 0..before.len() {
        let delta = Vec3::new(0.3, -0.2, 0.5);
        let mut edited = state.clone();
        let local = before[copy].object_shift_for(delta);
        edited.shift_object(local);
        let moved = Pose { position: pose.position + local, ..pose };
        let after = layout_of(&edited);

        let was = before[copy].apply(&pose).position;
        near_vec(was + delta, after[copy].apply(&moved).position, 1e-4, &format!("copy {copy} moved by the pose"));
        for (i, (a, b)) in edited.points.iter().zip(&state.points).enumerate() {
            near_vec(pose.position + *b, moved.position + *a, 1e-5, &format!("copy {copy}: path point {i} stays put"));
        }
        // Every copy keeps its own facing and size.
        for i in 0..before.len() {
            assert_eq!(before[i].scale, after[i].scale);
            near(0.0, angle_deg(before[i].rotation * inverse(after[i].rotation)), 0.1, "facing unchanged");
        }
        assert!(edited.has_copy_offset() || local == Vec3::ZERO);
    }
}

#[test]
fn turning_a_copy_turns_the_object_and_every_copy_follows_in_its_own_frame() {
    let pose = object();
    let turn = Quat::from_axis_angle(Vec3::new(1.0, 2.0, 3.0).normalize(), 0.6);

    // Following a curve: the posed copy faces as asked, and each other copy takes the object's change
    // through its own frame on the path.
    let mut s = ArraySettings::new(ArrayKind::Curve);
    s.count = 6;
    let p = compute(&s, &wave());
    for copy in 0..p.len() {
        let wanted = turn * p[copy].apply(&pose).rotation;
        let new_object = Pose { rotation: p[copy].object_rotation_for(wanted), ..pose };
        near(
            0.0,
            angle_deg(p[copy].apply(&new_object).rotation * inverse(wanted)),
            0.01,
            &format!("copy {copy} faces as asked"),
        );

        let change = inverse(p[copy].rotation) * turn * p[copy].rotation;
        for (other, c) in p.iter().enumerate() {
            let expected = c.rotation * change * pose.rotation;
            near(
                0.0,
                angle_deg(c.apply(&new_object).rotation * inverse(expected)),
                0.01,
                &format!("copy {other} after posing {copy}"),
            );
        }
    }

    // Not following: every copy faces the object's way, so turning any one turns them all by that same turn.
    s.follow = false;
    let p = compute(&s, &wave());
    let new_object = Pose { rotation: p[3].object_rotation_for(turn * p[3].apply(&pose).rotation), ..pose };
    for (i, c) in p.iter().enumerate() {
        let expected = turn * c.apply(&pose).rotation;
        near(
            0.0,
            angle_deg(c.apply(&new_object).rotation * inverse(expected)),
            0.01,
            &format!("copy {i} turned alike"),
        );
    }
}

#[test]
fn a_copys_local_matrix_is_its_world_matrix_seen_from_the_object() {
    let pose = Pose::new(Vec3::new(1.0, 2.0, 3.0), rotation(), Vec3::new(0.8, 1.3, 0.6));
    let mut s = ArraySettings::new(ArrayKind::Curve);
    s.count = 6;
    let p = compute(&s, &wave());
    let local = local_matrices(&p, &pose);
    assert_eq!(local[0], Mat4::IDENTITY);
    for (i, c) in p.iter().enumerate() {
        let composed = pose.matrix() * local[i];
        let world = c.world_matrix(&pose);
        for (a, b) in composed.to_cols_array().iter().zip(world.to_cols_array()) {
            assert!((a - b).abs() < 1e-4, "copy {i}: {composed:?} vs {world:?}");
        }
    }
}

#[test]
fn scale_follows_the_taper_and_the_swell() {
    let mut s = ArraySettings::new(ArrayKind::Line);
    s.end_scale = 0.25;
    near(1.0, scale_at(&s, 0.0) as f64, 1e-6, "first");
    near(0.25, scale_at(&s, 1.0) as f64, 1e-6, "last");
    near(0.625, scale_at(&s, 0.5) as f64, 1e-6, "half way");
    s.end_scale = 1.0;
    s.mid_scale = 3.0;
    near(3.0, scale_at(&s, 0.5) as f64, 1e-5, "swollen middle");
    near(1.0, scale_at(&s, 1.0) as f64, 1e-5, "swell fades at the end");
    s.mid_scale = 0.0;
    assert!(scale_at(&s, 0.5) > 0.0, "never collapses");
}

/// `cargo test -p sculpt-array --release --target-dir target/w/array -- --ignored --nocapture`
#[test]
#[ignore]
fn timing_of_a_layout() {
    let mut s = ArraySettings::new(ArrayKind::Curve);
    s.count = 100;
    s.closed = true;
    s.end_scale = 0.4;
    let points: Vec<Vec3> = (0..24)
        .map(|i| {
            let a = i as f32 / 24.0 * std::f32::consts::TAU;
            Vec3::new(a.cos() * 5.0, (a * 3.0).sin(), a.sin() * 5.0)
        })
        .collect();

    let runs = 2000;
    let start = std::time::Instant::now();
    let mut checksum = 0.0;
    for _ in 0..runs {
        checksum += compute(&s, &points)[50].offset.x;
    }
    println!("100 copies on a closed 24-point curve: {:?} per layout (checksum {checksum})", start.elapsed() / runs);
}
