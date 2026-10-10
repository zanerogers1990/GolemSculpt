//! Keys with answers that can be checked: closed pieces, volumes against the
//! profile's own, a peg that fits its socket, joints kept inside the face.
//! The tests that plan and build through a scaled object are the Unity
//! project's `SplitKeyTests.AutomaticKeysBuildIntoBothHalvesAndKeepThemClosed`.

use std::f64::consts::PI;
use std::sync::Arc;
use std::time::Instant;

use glam::{Mat4, Vec2, Vec3};
use sculpt_geom::TriMesh;
use sculpt_geom::units::from_millimetres as mm;
use sculpt_volume::TriangleBvh;

use crate::builder::{self, Built, NEW_VERTEX};
use crate::cutting::{split_by_bow, split_by_plane};
use crate::joint::KeyPart;
use crate::planner::{self, BuildResult, JointPlan, KeyPlan, KeySettings, KeySide, PlannedKey};
use crate::profile::{KeyProfile, KeyShape};
use crate::testing::{Halves, assert_closed, assert_near, halves, icosphere, world_mesh};
use crate::types::{KeyDimensions, KeyFeature, KeyFrame, KeyRole};
use crate::{joint_map, mating};

/// A cut plane that lines up with nothing.
const TILTED: Vec3 = Vec3::new(0.3, 0.9, 0.2);

fn shaped(shape: KeyShape) -> KeySettings {
    KeySettings { shape, ..KeySettings::default() }
}

fn plan_of(h: &Halves, settings: &KeySettings) -> KeyPlan {
    planner::plan(h.a.clone(), h.b.clone(), &h.pairs, settings, true)
}

/// Plans and builds, and checks what every build must give: both halves, no
/// key lost, closed and wound outward.
fn built(h: &Halves, settings: &KeySettings) -> (KeyPlan, BuildResult) {
    let mut plan = plan_of(h, settings);
    assert!(plan.key_count() > 0, "{:?}: no keys planned: {:?}", settings.shape, plan.joints[0].note);
    let result = planner::build(&mut plan);
    let (a, b) = result.meshes().unwrap_or_else(|| panic!("{:?}: {:?}", settings.shape, result.first_problem));
    assert_eq!(result.failed, 0, "{:?}: {:?}", settings.shape, result.first_problem);
    assert_eq!(result.built, plan.key_count());
    assert_closed(a, &format!("{:?} keyed A", settings.shape));
    assert_closed(b, &format!("{:?} keyed B", settings.shape));
    (plan, result)
}

/// The integral of a profile's area over a stretch of key where its offset
/// runs straight from `o0` to `o1`. The area of a parallel offset is a
/// quadratic in the offset, so Simpson's rule is exact.
fn slab(profile: &KeyProfile, length: f32, o0: f32, o1: f32) -> f64 {
    let area = |o: f32| profile.area(o) as f64;
    length as f64 / 6.0 * (area(o0) + 4.0 * area((o0 + o1) * 0.5) + area(o1))
}

/// The volume a peg adds, from its profile and dimensions alone.
fn peg_volume(profile: &KeyProfile, d: &KeyDimensions) -> f64 {
    let (h, slope) = (d.height, d.taper_slope());
    let c = d.tip_chamfer.clamp(0.0, h * 0.45);
    slab(profile, h - c, 0.0, -(h - c) * slope) + slab(profile, c, -(h - c) * slope, -h * slope - c)
}

/// The volume a socket takes away.
fn hole_volume(profile: &KeyProfile, d: &KeyDimensions) -> f64 {
    let (depth, slope) = (d.hole_depth(), d.taper_slope());
    let e = d.entry_chamfer.clamp(0.0, depth * 0.45);
    slab(profile, e, d.clearance + e, d.clearance - e * slope)
        + slab(profile, depth - e, d.clearance - e * slope, d.clearance - depth * slope)
}

fn volume(part: &KeyPart) -> f64 {
    world_mesh(part.mesh(), part.local_to_world()).signed_volume()
}

fn built_volume(built: &Built, part: &KeyPart) -> f64 {
    world_mesh(&built.mesh, part.local_to_world()).signed_volume()
}

#[test]
fn every_profile_gives_two_closed_outward_pieces() {
    let h = halves(5, 2.0, TILTED, 0.1, 1.0);
    for shape in KeyShape::ALL {
        let (plan, result) = built(&h, &shaped(shape));
        let (a, b) = (result.a.as_ref().unwrap(), result.b.as_ref().unwrap());
        // Every vertex the part came with is where it was; the rest are new.
        for (built, part) in [(a, &h.a), (b, &h.b)] {
            assert_eq!(built.source.len(), built.mesh.vertex_count());
            let mut added = 0;
            for (p, &source) in built.mesh.positions.iter().zip(&built.source) {
                if source == NEW_VERTEX {
                    added += 1;
                } else {
                    assert_eq!(*p, part.mesh().positions[source as usize]);
                }
            }
            assert!(added > 50, "{shape:?}: only {added} new vertices");
            assert!(built.errors.iter().all(Option::is_none));
        }
        // A keyed shape needs one key to stop the halves twisting; a round
        // one needs two.
        let expected = if shape == KeyShape::Circle { 2 } else { 1 };
        assert_eq!(plan.key_count(), expected, "{shape:?}");
    }
}

#[test]
fn a_peg_adds_and_a_socket_removes_the_volume_of_its_profile() {
    let h = halves(5, 2.0, TILTED, 0.1, 1.0);
    for (shape, taper) in KeyShape::ALL.into_iter().zip([0.0, 0.0, 2.0, 0.0, 0.0, 3.0, 0.0, 0.0, 0.0, 1.0]) {
        let settings = KeySettings { taper_degrees: taper, ..shaped(shape) };
        let (plan, result) = built(&h, &settings);
        let jp = &plan.joints[0];
        let profile = jp.valid_profile().unwrap();
        let keys = result.built as f64;
        let (added, removed) = (peg_volume(profile, &jp.dimensions) * keys, hole_volume(profile, &jp.dimensions) * keys);
        let grew_a = built_volume(result.a.as_ref().unwrap(), &h.a) - volume(&h.a);
        let grew_b = built_volume(result.b.as_ref().unwrap(), &h.b) - volume(&h.b);
        let (peg_side, hole_side) = if jp.peg_on_a { (grew_a, grew_b) } else { (grew_b, grew_a) };
        assert_near(peg_side, added, 2e-3, &format!("{shape:?} peg volume"));
        assert_near(-hole_side, removed, 2e-3, &format!("{shape:?} socket volume"));
        assert!(removed > added, "{shape:?}: the socket is the roomier of the two");
    }
}

#[test]
fn a_round_key_measures_what_its_dimensions_say() {
    let h = halves(5, 2.0, TILTED, 0.1, 1.0);
    let settings = KeySettings { size: mm(6.0), height: mm(5.0), count: 1, ..KeySettings::default() };
    let (plan, result) = built(&h, &settings);
    let jp = &plan.joints[0];
    assert_eq!(result.built, 1);
    let (r, hgt, c) = (mm(3.0) as f64, mm(5.0) as f64, mm(0.4) as f64);
    let frustum = |from: f64, to: f64, length: f64| PI * length / 3.0 * (from * from + from * to + to * to);
    let peg = PI * r * r * (hgt - c) + frustum(r, r - c, c);
    let (wide, e, depth) = (r + mm(0.15) as f64, mm(0.3) as f64, mm(5.4) as f64);
    let socket = frustum(wide + e, wide, e) + PI * wide * wide * (depth - e);

    let grew_a = built_volume(result.a.as_ref().unwrap(), &h.a) - volume(&h.a);
    let grew_b = built_volume(result.b.as_ref().unwrap(), &h.b) - volume(&h.b);
    let (peg_side, hole_side) = if jp.peg_on_a { (grew_a, -grew_b) } else { (grew_b, -grew_a) };
    // The key is a many-sided prism inside its true circle: a little under.
    assert!(peg_side < peg && hole_side < socket);
    assert_near(peg_side, peg, 0.015, "peg against a cylinder with a chamfered tip");
    assert_near(hole_side, socket, 0.015, "socket against a cylinder with a chamfered mouth");
}

/// Every vertex of a peg's wall and tip, once the halves are closed up, is
/// in the socket's cavity and at least the clearance from its wall.
#[test]
fn the_peg_fits_its_socket_with_the_clearance_all_round() {
    let h = halves(5, 2.0, TILTED, 0.1, 1.0);
    for settings in [
        shaped(KeyShape::Circle),
        shaped(KeyShape::Star),
        shaped(KeyShape::PuzzleTab),
        KeySettings { taper_degrees: 3.0, clearance: mm(0.25), ..shaped(KeyShape::Hexagon) },
    ] {
        let (plan, result) = built(&h, &settings);
        let jp = &plan.joints[0];
        let (a, b) = (result.a.as_ref().unwrap(), result.b.as_ref().unwrap());
        let (peg, hole) = if jp.peg_on_a { (a, b) } else { (b, a) };
        // From the peg's part into the socket's.
        let axis = if jp.peg_on_a { h.normal } else { -h.normal };
        let socket = TriangleBvh::new(&hole.mesh);

        let clearance = jp.dimensions.clearance;
        let (mut checked, mut nearest) = (0, f32::MAX);
        for (p, &source) in peg.mesh.positions.iter().zip(&peg.source) {
            if source != NEW_VERTEX || (*p - h.point).dot(axis) < mm(0.02) {
                continue;
            }
            checked += 1;
            assert!(!socket.contains(*p), "{:?}: a peg vertex is inside the other half's material", settings.shape);
            let gap = socket.distance(*p);
            assert!(gap >= clearance * 0.98, "{:?}: {gap} of gap where {clearance} was asked for", settings.shape);
            nearest = nearest.min(gap);
        }
        assert!(checked > 100, "{:?}: only {checked} peg vertices found", settings.shape);
        assert!(nearest < clearance * 1.02, "{:?}: the fit is looser than asked: {nearest}", settings.shape);
    }
}

#[test]
fn the_preview_is_the_solid_that_gets_built() {
    let d = KeyDimensions::from_millimetres(5.0, 0.2, 0.4, 0.5, 0.3, 1.0);
    for shape in [KeyShape::Circle, KeyShape::Cross, KeyShape::Dovetail] {
        let mut d = d;
        let (profile, _) = builder::fit_profile(shape, mm(6.0), 1.6, mm(0.6), &mut d, mm(0.4));
        let profile = Arc::new(profile);
        let frame = KeyFrame::new(Vec3::new(1.0, 2.0, 3.0), Vec3::new(0.2, 1.0, -0.4), Vec3::X, 30.0);
        for role in [KeyRole::Peg, KeyRole::Hole] {
            let feature = KeyFeature { role, profile: profile.clone(), dimensions: d, frame, patch: 0 };
            let mut solid = TriMesh::default();
            builder::preview(&feature, mm(0.4), &mut solid);
            // Open at the cut face: close it there with a fan and it is a
            // solid, facing out for a peg and in for a socket.
            let n = profile.point_count() as u32;
            for i in 1..n - 1 {
                solid.indices.extend_from_slice(&if role == KeyRole::Peg { [0, i + 1, i] } else { [0, i, i + 1] });
            }
            let expected = if role == KeyRole::Peg { peg_volume(&profile, &d) } else { -hole_volume(&profile, &d) };
            assert_near(solid.signed_volume(), expected, 2e-3, &format!("{shape:?} {role:?} preview"));
            if role == KeyRole::Hole {
                solid.flip();
            }
            // A fan across a shape with inside corners overlaps itself, but
            // every edge is still shared by two triangles.
            assert_closed(&solid, &format!("{shape:?} {role:?} preview"));
        }
    }
}

/// How far a plane point of a joint is from the middle of the round cut face.
fn from_centre(h: &Halves, jp: &JointPlan, plane: Vec2) -> f32 {
    jp.joint.from_plane(plane, 0.0).distance(h.point)
}

#[test]
fn joints_keep_the_margins_the_planner_promises() {
    let radius = 2.0;
    for (shape, offset) in [(KeyShape::Circle, 0.0), (KeyShape::Circle, 0.6), (KeyShape::Square, -0.4), (KeyShape::Star, 0.0)] {
        let h = halves(5, radius, TILTED, offset, 1.0);
        let settings = shaped(shape);
        let plan = plan_of(&h, &settings);
        let jp = &plan.joints[0];
        assert!(jp.joint.axis.distance(h.normal) < 1e-3, "the joint pulls apart along the cut's normal");
        assert!((jp.joint.area - std::f32::consts::PI * h.face_radius * h.face_radius).abs() < 0.02 * jp.joint.area);
        assert!(jp.joint.spread < 1.0 && jp.map.folded_fraction() == 0.0);
        assert!(plan.key_count() >= 1 && plan.invalid_count() == 0, "{shape:?}: {:?}", jp.note);

        // The sockets go into the half with more behind the cut: the cap the
        // plane was moved away from.
        if offset != 0.0 {
            assert_eq!(jp.peg_on_a, offset < 0.0, "{shape:?} at {offset}");
        }
        // Automatic sizes stay inside the limits the panel states.
        assert!(jp.size >= mm(1.5) - 1e-6 && jp.size <= mm(10.0) + 1e-6);
        assert!(jp.height >= mm(1.5) - 1e-6 && jp.height <= mm(12.0) + 1e-6);

        let slack = jp.map.cell() * 1.5;
        let need = jp.dimensions.hole_depth() + settings.min_wall;
        // What is behind the face in the half the sockets go into, at a
        // distance `rho` from the middle of the face.
        let behind = |rho: f32| {
            let reach = (radius * radius - rho * rho).max(0.0).sqrt();
            if jp.peg_on_a { reach - offset } else { reach + offset }
        };
        for key in &jp.keys {
            for p in jp.mouth(key) {
                let rho = from_centre(&h, jp, p);
                assert!(rho <= h.face_radius - settings.min_wall + slack, "{shape:?}: a mouth {rho} out on a face of {}", h.face_radius);
                assert!(behind(rho + settings.min_wall) >= need - slack, "{shape:?}: a socket too near the far side");
            }
        }
        let reach = jp.valid_profile().unwrap().circumradius(jp.dimensions.mouth_offset());
        for (i, key) in jp.keys.iter().enumerate() {
            for other in &jp.keys[i + 1..] {
                assert!(key.position.distance(other.position) >= 2.0 * reach + settings.min_wall - slack);
            }
        }
    }
}

#[test]
fn a_face_too_small_gets_no_joint() {
    // A 4 mm ball cut through the middle, and a cap off the side of a 10 mm
    // one: a face 4.4 mm across, where the smallest key and its walls want
    // 4.8.
    for h in [halves(4, 0.2, TILTED, 0.0, 1.0), halves(4, 0.5, TILTED, 0.45, 1.0)] {
        for shape in [KeyShape::Circle, KeyShape::Hexagon] {
            let mut plan = plan_of(&h, &shaped(shape));
            assert_eq!(plan.key_count(), 0);
            let note = plan.joints[0].note.clone().unwrap_or_default();
            assert!(note.starts_with("no room for a key"), "{note}");
            // Building nothing gives the halves back as they were.
            let result = planner::build(&mut plan);
            assert_eq!(result.built, 0);
            assert_eq!(result.a.unwrap().mesh.triangle_count(), h.a.triangle_count());
        }
    }
}

#[test]
fn a_key_placed_by_hand_is_checked_where_it_stands() {
    let h = halves(5, 2.0, TILTED, 0.0, 1.0);
    let settings = KeySettings { size: mm(5.0), height: mm(4.0), ..shaped(KeyShape::Square) };
    let mut plan = planner::plan(h.a.clone(), h.b.clone(), &h.pairs, &settings, false);
    let jp = &mut plan.joints[0];
    assert!(jp.keys.is_empty() && jp.valid_profile().is_some(), "sized, with nothing placed");
    let centre = jp.joint.to_plane(h.point);
    let toward = |distance: f32| centre + Vec2::new(distance, 0.0);

    jp.keys.push(PlannedKey::new(toward(0.0), 0.0));
    planner::validate(jp, 0, &settings);
    assert!(jp.keys[0].valid && jp.keys[0].problem.is_none());
    assert_eq!(jp.key_at(toward(0.1)), Some(0));
    assert_eq!(jp.key_at(toward(1.0)), None);

    // Hard against the rim, half off the face, and off it altogether.
    for distance in [1.75, 2.0, 2.6] {
        jp.keys[0].position = toward(distance);
        planner::validate(jp, 0, &settings);
        assert!(!jp.keys[0].valid);
        assert_eq!(jp.keys[0].problem.as_deref(), Some(joint_map::TOO_CLOSE), "at {distance}");
    }

    // Where the face is wide enough but the ball behind it is not deep
    // enough for a tall peg's socket.
    let deep = KeySettings { height: mm(14.0), ..settings };
    planner::resize(jp, &deep);
    jp.keys[0].position = toward(1.2);
    planner::validate(jp, 0, &deep);
    let problem = jp.keys[0].problem.clone().unwrap();
    assert!(problem.contains("of material behind the face") || problem == joint_map::TOO_CLOSE, "{problem}");
    planner::resize(jp, &settings);

    // Two keys shoulder to shoulder crowd each other; apart they are fine.
    jp.keys[0].position = toward(-0.3);
    jp.keys.push(PlannedKey::new(toward(0.3), 45.0));
    planner::validate_all(jp, &settings);
    assert!(jp.keys.iter().all(|k| k.problem.as_deref() == Some("too close to another key")));
    jp.keys[1].position = toward(0.9);
    jp.keys[0].position = toward(-0.9);
    planner::validate_all(jp, &settings);
    assert!(jp.keys.iter().all(|k| k.valid));

    // A key forced through where it cannot be built is dropped from both
    // halves, and the rest still build.
    jp.keys.push(PlannedKey::new(toward(5.0), 0.0));
    let result = planner::build(&mut plan);
    assert_eq!((result.built, result.failed), (2, 1));
    assert_eq!(plan.joints[0].keys[2].problem.as_deref(), Some("off the cut face"));
    let (a, b) = result.meshes().unwrap();
    assert_closed(a, "A with two keys");
    assert_closed(b, "B with two keys");
}

#[test]
fn keys_that_overlap_are_not_built_through_each_other() {
    let h = halves(5, 2.0, TILTED, 0.0, 1.0);
    let settings = KeySettings { size: mm(5.0), ..shaped(KeyShape::Circle) };
    let mut plan = planner::plan(h.a.clone(), h.b.clone(), &h.pairs, &settings, false);
    let centre = plan.joints[0].joint.to_plane(h.point);
    // Marked valid by hand: the builder has to catch it by itself.
    plan.joints[0].keys = vec![PlannedKey::new(centre, 0.0), PlannedKey::new(centre + Vec2::new(0.3, 0.0), 0.0)];
    let result = planner::build(&mut plan);
    assert_eq!((result.built, result.failed), (1, 1));
    assert_eq!(result.first_problem.as_deref(), Some("overlaps another key"));
    let (a, b) = result.meshes().unwrap();
    assert_closed(a, "A");
    assert_closed(b, "B");
}

#[test]
fn mating_faces_are_found_from_the_geometry_alone() {
    let h = halves(5, 2.0, TILTED, 0.3, 1.0);
    let face_area = (std::f32::consts::PI * h.face_radius * h.face_radius) as f64;
    let (mut a, mut b) = ((*h.a).clone(), (*h.b).clone());
    let (known_a, known_b) = (a.cut_faces.clone(), b.cut_faces.clone());
    let pairs = mating::find(&mut a, &mut b, 0.0);
    assert_eq!(pairs, vec![(0, 0)]);
    assert_near(mating::mating_area(&a, &pairs) as f64, face_area, 0.01, "mating area");
    // The same triangles the splitter said were the cut faces.
    let sorted = |face: &[u32]| {
        let mut face = face.to_vec();
        face.sort_unstable();
        face
    };
    assert_eq!(sorted(&a.cut_faces[0]), sorted(&known_a[0]));
    assert_eq!(sorted(&b.cut_faces[0]), sorted(&known_b[0]));

    // Keys planned from the found faces are the keys planned from the known
    // ones.
    let settings = KeySettings::default();
    let found = planner::plan(Arc::new(a), Arc::new(b), &pairs, &settings, true);
    let known = plan_of(&h, &settings);
    assert_eq!(found.key_count(), known.key_count());
    for (x, y) in found.joints[0].keys.iter().zip(&known.joints[0].keys) {
        assert!(x.position.distance(y.position) < 1e-4);
    }

    // Pulled apart, or turned so the faces no longer meet, nothing mates.
    let apart = Mat4::from_translation(h.normal * 0.05);
    let mut moved = KeyPart::new(h.b.mesh().clone(), apart, Vec::new());
    let mut still = (*h.a).clone();
    assert!(mating::find(&mut still, &mut moved, 0.0).is_empty());
    assert!(still.cut_faces.is_empty() && moved.cut_faces.is_empty());
    let mut twin = (*h.a).clone();
    let mut same = (*h.a).clone();
    assert!(mating::find(&mut same, &mut twin, 0.0).is_empty(), "a part does not mate with a copy of itself");
}

#[test]
fn a_cut_through_two_legs_makes_a_joint_for_each() {
    // Two balls side by side in one mesh, cut by one plane.
    let mut legs = icosphere(4, 1.0);
    legs.transform(&Mat4::from_translation(Vec3::new(-1.5, 0.0, 0.0)));
    let mut other = icosphere(4, 0.8);
    other.transform(&Mat4::from_translation(Vec3::new(1.5, 0.0, 0.0)));
    legs.append(&other);
    let cut = split_by_plane(&legs, Vec3::new(0.0, 0.1, 0.0), Vec3::Y).unwrap();
    assert_eq!(cut.face_pairs.len(), 2);
    let a = Arc::new(KeyPart::new(cut.outside.mesh, Mat4::IDENTITY, cut.outside.cut_faces));
    let b = Arc::new(KeyPart::new(cut.inside.mesh, Mat4::IDENTITY, cut.inside.cut_faces));
    let mut plan = planner::plan(a, b, &cut.face_pairs, &shaped(KeyShape::Hexagon), true);
    assert_eq!(plan.joints.len(), 2);
    assert_eq!(plan.key_count(), 2, "one keyed shape on each leg");
    let result = planner::build(&mut plan);
    assert_eq!((result.built, result.failed), (2, 0));
    let (a, b) = result.meshes().unwrap();
    assert_closed(a, "A");
    assert_closed(b, "B");
}

/// A lasso cut is a curved sheet, not a plane: the keys stand on it all the
/// same, each on a flat ring let into the face, and the two faces are still
/// found to mate though they are triangulated differently.
#[test]
fn a_gently_curved_cut_takes_keys_too() {
    // The coarser the mesh, the further the two faces' own triangles stray
    // from the sheet they were cut along, and from each other.
    for (level, sag, shape) in [(5, 0.12, KeyShape::Circle), (4, 0.25, KeyShape::Square), (3, -0.2, KeyShape::PuzzleTab)] {
        let sphere = icosphere(level, 2.0);
        let cut = split_by_bow(&sphere, Vec3::ZERO, TILTED.normalize(), sag, 2.0).unwrap();
        assert_eq!(cut.face_pairs, vec![(0, 0)]);
        let a = KeyPart::new(cut.outside.mesh, Mat4::IDENTITY, cut.outside.cut_faces);
        let b = KeyPart::new(cut.inside.mesh, Mat4::IDENTITY, cut.inside.cut_faces);

        // From the geometry alone, the same faces the splitter reported.
        let (mut found_a, mut found_b) = (a.clone(), b.clone());
        let pairs = mating::find(&mut found_a, &mut found_b, 0.0);
        assert_eq!(pairs, vec![(0, 0)], "sag {sag}: the curved faces were not found to mate");
        let share = found_a.cut_faces[0].len() as f32 / a.cut_faces[0].len() as f32;
        assert!(share > 0.97, "sag {sag}: only {share} of the face was found");

        for (a, b) in [(a, b), (found_a, found_b)] {
            let h = Halves { a: Arc::new(a), b: Arc::new(b), pairs: vec![(0, 0)], point: Vec3::ZERO, normal: TILTED.normalize(), face_radius: 2.0 };
            let (plan, result) = built(&h, &shaped(shape));
            let jp = &plan.joints[0];
            assert!(jp.joint.spread > 2.0 && jp.joint.spread < joint_map::MAX_FACE_TILT_DEGREES, "spread {}", jp.joint.spread);
            // Volumes still add up: a key stands on a flat let into the face,
            // so a little more or less than its own volume, never much.
            let profile = jp.valid_profile().unwrap();
            let keys = result.built as f64;
            let grew_a = built_volume(result.a.as_ref().unwrap(), &h.a) - volume(&h.a);
            let grew_b = built_volume(result.b.as_ref().unwrap(), &h.b) - volume(&h.b);
            let (peg_side, hole_side) = if jp.peg_on_a { (grew_a, -grew_b) } else { (grew_b, -grew_a) };
            assert_near(peg_side, peg_volume(profile, &jp.dimensions) * keys, 0.15, "peg volume on a curved face");
            assert_near(hole_side, hole_volume(profile, &jp.dimensions) * keys, 0.15, "socket volume on a curved face");
        }
    }
}

/// The Unity project's `AutomaticKeysBuildIntoBothHalvesAndKeepThemClosed`:
/// through an object four times life size, so the millimetre-sized keys go
/// through the object's placement.
#[test]
fn automatic_keys_build_into_both_halves_of_a_scaled_object() {
    for shape in [KeyShape::Circle, KeyShape::Hexagon, KeyShape::PuzzleTab, KeyShape::Dovetail] {
        // A ball one unit across in an object scaled four times: a 40 mm part.
        let h = halves(5, 2.0, Vec3::new(0.57, 0.82, 0.05), 0.1, 4.0);
        assert!(h.a.mesh().bounds().size().max_element() < 1.01, "the mesh itself is unit size");
        let (plan, result) = built(&h, &shaped(shape));
        let (a, b) = result.meshes().unwrap();
        assert!(a.triangle_count() > h.a.triangle_count() && b.triangle_count() > h.b.triangle_count());
        // The keys came out in world millimetres: the peg stands its height
        // out of the cut face, measured in the world.
        let jp = &plan.joints[0];
        let (peg, axis) = if jp.peg_on_a { (a, h.normal) } else { (b, -h.normal) };
        let proud = peg.positions.iter().map(|p| (*p * 4.0 - h.point).dot(axis)).fold(0.0, f32::max);
        assert!((proud - jp.height).abs() < mm(0.01), "{shape:?}: the peg stands {proud} proud, not {}", jp.height);
    }
}

#[test]
fn settings_default_to_a_snug_fit_for_a_small_nozzle() {
    let s = KeySettings::default();
    assert_eq!((s.shape, s.count, s.side), (KeyShape::Circle, 0, KeySide::Auto));
    assert_eq!((s.size, s.height), (0.0, 0.0), "sized from the face");
    assert!((s.clearance - mm(0.15)).abs() < 1e-7 && (s.min_wall - mm(1.2)).abs() < 1e-7);
    assert_eq!(planner::show_mm(mm(5.0), 1), "5");
    assert_eq!(planner::show_mm(mm(2.75), 1), "2.8");
    assert_eq!(planner::show_mm(mm(0.15), 2), "0.15");
}

/// `cargo test -p sculpt-keys --release -- --ignored --nocapture timing`
#[test]
#[ignore = "timing: run with --release"]
fn timing_keys_on_a_million_triangle_model() {
    let sphere = icosphere(8, 5.0);
    println!("model: {} triangles, {} vertices", sphere.triangle_count(), sphere.vertex_count());
    let start = Instant::now();
    let cut = split_by_plane(&sphere, TILTED.normalize() * 0.3, TILTED).unwrap();
    println!("split (sculpt-cut): {:?}", start.elapsed());

    let start = Instant::now();
    let mut a = KeyPart::new(cut.outside.mesh, Mat4::IDENTITY, Vec::new());
    let mut b = KeyPart::new(cut.inside.mesh, Mat4::IDENTITY, Vec::new());
    println!("parts into world space: {:?}", start.elapsed());

    let start = Instant::now();
    let pairs = mating::find(&mut a, &mut b, 0.0);
    println!("mating faces: {:?} ({} + {} face triangles)", start.elapsed(), a.cut_faces[0].len(), b.cut_faces[0].len());
    assert_eq!(pairs, vec![(0, 0)]);

    let (a, b) = (Arc::new(a), Arc::new(b));
    for shape in [KeyShape::Circle, KeyShape::PuzzleTab] {
        let start = Instant::now();
        let mut plan = planner::plan(a.clone(), b.clone(), &pairs, &shaped(shape), true);
        let planned = start.elapsed();
        let start = Instant::now();
        let result = planner::build(&mut plan);
        println!("{shape:?}: plan {planned:?}, build {:?} ({} keys)", start.elapsed(), result.built);
        assert!(result.built > 0 && result.failed == 0);
        let (keyed_a, keyed_b) = result.meshes().unwrap();
        assert_closed(keyed_a, "keyed A");
        assert_closed(keyed_b, "keyed B");
    }
}
