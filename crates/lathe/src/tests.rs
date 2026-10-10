//! The lathe builder's promises, pinned: every build is 2-manifold with consistent outward winding,
//! closed builds are watertight with the right Euler characteristic, the mesh is exactly
//! mirror-symmetric across x = 0 and z = 0 in positions and topology alike, and the counts and
//! volumes are the ones the geometry says they must be.
//!
//! The first six tests are the Unity `LatheMeshBuilderTests`; the rest are new.

use std::collections::{HashMap, HashSet};
use std::f64::consts::PI;
use std::time::Instant;

use glam::Vec3;
use sculpt_geom::TriMesh;

use crate::{
    LatheBuild, LatheBuilder, LathePreset, LatheProfile, LatheSettings, MAX_VERTICES, ProfilePoint, build_mesh,
    round_segments,
};

fn preset_profile(preset: LathePreset) -> LatheProfile {
    preset.profile(1.3)
}

fn settings(segments: usize, even: bool, caps: bool) -> LatheSettings {
    LatheSettings { radial_segments: segments, even_triangles: even, cap_ends: caps }
}

fn build(profile: &LatheProfile, segments: usize, even: bool, caps: bool) -> LatheBuild {
    let mut out = LatheBuild::default();
    LatheBuilder::new().build(profile, &settings(segments, even, caps), &mut out).expect("the profile builds");
    out
}

fn open_profile(points: &[ProfilePoint]) -> LatheProfile {
    LatheProfile::from_points(points, false)
}

/// Every undirected edge with how many triangles use it, checking each directed edge is used once.
fn edge_uses(mesh: &TriMesh) -> HashMap<(u32, u32), usize> {
    let mut directed = HashSet::new();
    let mut uses = HashMap::new();
    for tri in mesh.indices.chunks_exact(3) {
        for e in 0..3 {
            let (a, b) = (tri[e], tri[(e + 1) % 3]);
            assert_ne!(a, b, "degenerate triangle");
            assert!(directed.insert((a, b)), "directed edge {a}->{b} used twice (bad winding or non-manifold)");
            *uses.entry((a.min(b), a.max(b))).or_insert(0) += 1;
        }
    }
    uses
}

// ------------------------------------------------------------------ ported from Unity

#[test]
fn every_preset_is_manifold_watertight_and_outward() {
    for preset in LathePreset::ALL {
        for segments in [8, 32, 128] {
            for even in [true, false] {
                let r = build(&preset_profile(preset), segments, even, true);
                let label = format!("{} segments={segments} even={even}", preset.name());

                let uses = edge_uses(&r.mesh);
                for (&(a, b), &count) in &uses {
                    assert_eq!(count, 2, "{label}: edge {a}-{b} is used by {count} triangles");
                }
                assert!(r.watertight, "{label}");
                let euler = r.vertex_count() as i64 - uses.len() as i64 + r.triangle_count() as i64;
                assert_eq!(euler, if r.profile_is_loop { 0 } else { 2 }, "{label}: Euler characteristic");
                assert!(r.mesh.signed_volume() > 0.0, "{label}: faces point inward");
            }
        }
    }
}

#[test]
fn mesh_is_bit_exactly_mirror_symmetric() {
    // -0 and 0 compare equal as floats but not as bits, so +0 is added before taking the bits.
    fn key(p: Vec3) -> [u32; 3] {
        [(p.x + 0.0).to_bits(), (p.y + 0.0).to_bits(), (p.z + 0.0).to_bits()]
    }
    fn canonical(a: u32, b: u32, c: u32) -> (u32, u32, u32) {
        if a <= b && a <= c {
            (a, b, c)
        } else if b <= a && b <= c {
            (b, c, a)
        } else {
            (c, a, b)
        }
    }

    for preset in [LathePreset::Vase, LathePreset::Ring] {
        for even in [true, false] {
            let r = build(&preset_profile(preset), 64, even, true);
            let index: HashMap<[u32; 3], u32> =
                r.mesh.positions.iter().enumerate().map(|(i, &p)| (key(p), i as u32)).collect();
            let triangles: HashSet<(u32, u32, u32)> =
                r.mesh.indices.chunks_exact(3).map(|t| canonical(t[0], t[1], t[2])).collect();

            for flip in [Vec3::new(-1.0, 1.0, 1.0), Vec3::new(1.0, 1.0, -1.0)] {
                let map: Vec<u32> = r
                    .mesh
                    .positions
                    .iter()
                    .map(|&p| {
                        *index.get(&key(p * flip)).unwrap_or_else(|| panic!("no exact mirror for {p} across {flip}"))
                    })
                    .collect();
                for (t, tri) in r.mesh.indices.chunks_exact(3).enumerate() {
                    let mirrored = canonical(map[tri[0] as usize], map[tri[2] as usize], map[tri[1] as usize]);
                    assert!(
                        triangles.contains(&mirrored),
                        "{} even={even}: triangle {t} has no mirror across {flip}",
                        preset.name()
                    );
                }
            }
        }
    }
}

#[test]
fn drawing_direction_does_not_change_the_solid() {
    let forward = preset_profile(LathePreset::Vase);
    let mut reversed_points = forward.points().to_vec();
    reversed_points.reverse();
    let reversed = open_profile(&reversed_points);

    let a = build(&forward, 64, true, true).mesh.signed_volume();
    let b = build(&reversed, 64, true, true).mesh.signed_volume();
    assert!(a > 0.0);
    assert!((a - b).abs() <= a * 1e-4, "{a} vs {b}");
}

#[test]
fn uncapped_tube_is_open_and_sphere_volume_converges() {
    let tube = open_profile(&[ProfilePoint::corner(0.5, 0.0), ProfilePoint::corner(0.5, 2.0)]);
    assert!(!build(&tube, 128, true, false).watertight);

    let points: Vec<ProfilePoint> = (0..=8)
        .map(|i| {
            let phi = PI * i as f64 / 8.0;
            let x = if i == 0 || i == 8 { 0.0 } else { phi.sin() as f32 };
            ProfilePoint::smooth(x, (-phi.cos()) as f32)
        })
        .collect();
    let volume = build(&open_profile(&points), 256, true, true).mesh.signed_volume();
    assert!((volume - 4.0 / 3.0 * PI).abs() < 0.01 * 4.19, "sphere volume {volume}");
}

#[test]
fn interior_points_are_held_off_the_axis() {
    let profile = open_profile(&[
        ProfilePoint::smooth(0.0, 0.0),
        ProfilePoint::smooth(0.0, 0.5), // interior on the axis - must be pushed off
        ProfilePoint::smooth(0.4, 1.0),
        ProfilePoint::smooth(0.0, 1.5),
    ]);
    assert!(profile[1].position.x > 0.0);
    assert_eq!(profile[0].position.x, 0.0);
    assert_eq!(profile[3].position.x, 0.0);
    assert!(build(&profile, 64, true, true).watertight);
}

/// The panel reports the final mesh's count from `count` while the preview is built lighter, so
/// the count has to be exactly what `build` makes.
#[test]
fn count_matches_build() {
    let mut builder = LatheBuilder::new();
    for preset in [LathePreset::Vase, LathePreset::Bowl, LathePreset::Egg, LathePreset::Ring] {
        for segments in [8, 256, 1024] {
            for even in [true, false] {
                for caps in [true, false] {
                    let profile = preset_profile(preset);
                    let s = settings(segments, even, caps);
                    let mut out = LatheBuild::default();
                    builder.build(&profile, &s, &mut out).expect("builds");
                    let counts = builder.count(&profile, &s).expect("counts");
                    let label = format!("{} segments={segments} even={even} caps={caps}", preset.name());
                    assert_eq!(counts.triangles, out.triangle_count(), "{label}");
                    assert_eq!(counts.vertices, out.vertex_count(), "{label}");
                    assert!(out.vertex_count() <= MAX_VERTICES, "{label}");
                }
            }
        }
    }
}

// ----------------------------------------------------------------------- new tests

#[test]
fn every_preset_has_positive_volume_at_default_settings_and_any_height() {
    for preset in LathePreset::ALL {
        for height in [0.05, 1.0, 20.0] {
            let mesh = build_mesh(&preset.profile(height), &LatheSettings::default()).expect("builds");
            assert!(
                mesh.signed_volume() > 0.0,
                "{} at height {height}: volume {}",
                preset.name(),
                mesh.signed_volume()
            );
        }
    }
}

#[test]
fn faces_are_counter_clockwise_seen_from_outside() {
    let (radius, height) = (1.5f32, 3.0f32);
    let cylinder = open_profile(&[
        ProfilePoint::corner(0.0, 0.0),
        ProfilePoint::corner(radius, 0.0),
        ProfilePoint::corner(radius, height),
        ProfilePoint::corner(0.0, height),
    ]);
    let mesh = build(&cylinder, 32, false, true).mesh;
    let (mut bottom, mut top, mut wall) = (0, 0, 0);
    for t in 0..mesh.triangle_count() {
        let [a, b, c] = mesh.triangle(t);
        let normal = (b - a).cross(c - a);
        if [a, b, c].iter().all(|p| p.y == 0.0) {
            assert!(normal.y < 0.0, "bottom face points down");
            bottom += 1;
        } else if [a, b, c].iter().all(|p| p.y == height) {
            assert!(normal.y > 0.0, "top face points up");
            top += 1;
        } else {
            let centre = (a + b + c) / 3.0;
            assert!(normal.dot(Vec3::new(centre.x, 0.0, centre.z)) > 0.0, "wall face points away from the axis");
            wall += 1;
        }
    }
    assert!(bottom > 0 && top > 0 && wall > 0);
}

#[test]
fn a_cylinder_has_the_volume_of_a_cylinder() {
    let (radius, height, n) = (1.5f32, 3.0f32, 128usize);
    let with_poles = open_profile(&[
        ProfilePoint::corner(0.0, 0.0),
        ProfilePoint::corner(radius, 0.0),
        ProfilePoint::corner(radius, height),
        ProfilePoint::corner(0.0, height),
    ]);
    let capped_tube = open_profile(&[ProfilePoint::corner(radius, 0.0), ProfilePoint::corner(radius, height)]);

    // The rim is an n-gon, and every other face is flat or on its sides, so the volume is the n-gon
    // prism exactly; the true cylinder is that plus the sliver the polygon leaves out.
    let polygon = 0.5 * n as f64 * (radius as f64).powi(2) * (2.0 * PI / n as f64).sin() * height as f64;
    let cylinder = PI * (radius as f64).powi(2) * height as f64;
    for (name, profile) in [("poles", &with_poles), ("capped tube", &capped_tube)] {
        for even in [true, false] {
            let volume = build(profile, n, even, true).mesh.signed_volume();
            assert!((volume - polygon).abs() < polygon * 1e-5, "{name} even={even}: {volume} vs prism {polygon}");
            assert!((volume - cylinder).abs() < cylinder * 1e-3, "{name} even={even}: {volume} vs cylinder {cylinder}");
        }
    }
}

#[test]
fn a_torus_has_the_volume_of_a_torus() {
    // A circle of radius 0.5 centred 2 from the axis: 2 pi^2 R a^2.
    let points: Vec<ProfilePoint> = (0..12)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / 12.0;
            ProfilePoint::smooth(2.0 + 0.5 * a.cos(), 0.5 * a.sin())
        })
        .collect();
    let profile = LatheProfile::from_points(&points, true);
    let r = build(&profile, 128, true, true);
    assert!(r.profile_is_loop && r.watertight);
    // The section is sampled at the widest ring's vertex spacing (2 pi 2.5 / 128 = 0.12), so the
    // circle of circumference 3.14 becomes a 26-gon that holds about 1% less area; the rings add
    // another 0.04%. Hence a little under, and a 2% tolerance.
    let expected = 2.0 * PI * PI * 2.0 * 0.25;
    let volume = r.mesh.signed_volume();
    assert!(volume < expected && volume > expected * 0.98, "torus volume {volume} vs {expected}");
}

#[test]
fn counts_follow_the_formula_for_the_chosen_segments() {
    // A tube capped at both ends: radius 1, height 2, 64 segments. The spacing that makes the widest
    // ring's faces square is 2 pi / 64, so each cap is round(1 / spacing) = 10 pieces across and the
    // wall round(2 / spacing) = 20 pieces tall: 2 * 10 + 20 + 1 = 41 profile samples, two of them poles.
    let tube = open_profile(&[ProfilePoint::corner(1.0, 0.0), ProfilePoint::corner(1.0, 2.0)]);
    let mut builder = LatheBuilder::new();
    let mut out = LatheBuild::default();

    // Without even triangles every ring has all 64 vertices: 2 + 39 * 64 vertices and, from the two
    // pole fans of 64 and 38 bands of 128, 2 * 64 * 39 triangles.
    builder.build(&tube, &settings(64, false, true), &mut out).unwrap();
    assert_eq!((out.vertex_count(), out.triangle_count()), (2 + 39 * 64, 2 * 64 * 39));
    assert_eq!(out.profile.len(), 41);

    // With them, a cap ring k pieces out has round(64 k / 10 / 4) * 4 vertices: 8 12 20 24 32 40 44
    // 52 56 for k = 1..9, summing to 288. The wall's 21 rings have 64. Each cap is a fan of 8, the
    // bands between its rings (8+12 ... 52+56), and the band out to the rim (56 + 64).
    builder.build(&tube, &settings(64, true, true), &mut out).unwrap();
    assert_eq!(out.vertex_count(), 2 + 21 * 64 + 2 * 288);
    let cap = 8 + (2 * 288 - 8 - 56) + (56 + 64);
    assert_eq!(out.triangle_count(), 2 * cap + 20 * 128);
    assert_eq!((out.vertex_count(), out.triangle_count()), (1922, 3840));

    let counts = builder.count(&tube, &settings(64, true, true)).unwrap();
    assert_eq!((counts.vertices, counts.triangles), (1922, 3840));
}

#[test]
fn vertex_normals_are_unit_and_point_out_of_a_sphere() {
    let points: Vec<ProfilePoint> = (0..=12)
        .map(|i| {
            let phi = PI * i as f64 / 12.0;
            let x = if i == 0 || i == 12 { 0.0 } else { phi.sin() as f32 };
            ProfilePoint::smooth(x, (-phi.cos()) as f32)
        })
        .collect();
    let r = build(&open_profile(&points), 64, true, true);
    assert_eq!(r.normals.len(), r.vertex_count());
    for (p, n) in r.mesh.positions.iter().zip(&r.normals) {
        assert!((n.length() - 1.0).abs() < 1e-5, "normal {n} at {p}");
        assert!(n.dot(p.normalize()) > 0.97, "normal {n} at {p} does not point outward");
    }
}

#[test]
fn a_profile_that_crosses_itself_is_built_but_flagged() {
    let bow_tie = open_profile(&[
        ProfilePoint::corner(0.2, 0.0),
        ProfilePoint::corner(0.8, 1.0),
        ProfilePoint::corner(0.8, 0.0),
        ProfilePoint::corner(0.2, 1.0),
    ]);
    let r = build(&bow_tie, 32, true, true);
    assert!(r.self_intersecting);
    assert!(!build(&preset_profile(LathePreset::Vase), 32, true, true).self_intersecting);
}

#[test]
fn a_curve_swinging_across_the_axis_is_held_off_it_and_flagged() {
    let dipping = open_profile(&[
        ProfilePoint::smooth(0.5, 0.0),
        ProfilePoint::smooth(0.0, 1.0), // pushed to the interior minimum
        ProfilePoint::smooth(0.5, 1.2),
    ]);
    let r = build(&dipping, 128, true, true);
    assert!(r.touches_axis);
    assert!(r.mesh.positions.iter().all(|p| p.x.is_finite() && p.z.is_finite()));
    assert!(r.profile.iter().all(|p| p.x >= 0.0));
    assert!(!build(&preset_profile(LathePreset::Bowl), 128, true, true).touches_axis);
}

#[test]
fn bad_profiles_say_what_is_wrong_and_leave_the_build_empty() {
    let mut builder = LatheBuilder::new();
    let mut out = build(&preset_profile(LathePreset::Vase), 32, true, true);
    assert!(out.vertex_count() > 0);

    let cases: [(LatheProfile, &str); 4] = [
        (open_profile(&[ProfilePoint::smooth(1.0, 1.0)]), "at least 2"),
        (
            LatheProfile::from_points(&[ProfilePoint::smooth(1.0, 0.0), ProfilePoint::smooth(1.0, 1.0)], true),
            "closed loop",
        ),
        (open_profile(&[ProfilePoint::smooth(0.0, 0.0), ProfilePoint::smooth(0.0, 1.0)]), "on the axis"),
        (LatheProfile::new(), "at least 2"),
    ];
    for (profile, expected) in cases {
        let error = builder.build(&profile, &LatheSettings::default(), &mut out).unwrap_err();
        assert!(error.contains(expected), "{error:?} should mention {expected:?}");
        assert!(out.mesh.is_empty() && out.normals.is_empty() && out.profile.is_empty() && !out.watertight);
        assert!(builder.count(&profile, &LatheSettings::default()).is_err());
    }
}

#[test]
fn rebuilding_into_the_same_buffers_gives_the_same_mesh_without_reallocating() {
    let vase = preset_profile(LathePreset::Vase);
    let egg = preset_profile(LathePreset::Egg);
    let s = settings(256, true, true);
    let fresh = build(&vase, 256, true, true);

    let mut builder = LatheBuilder::new();
    let mut out = LatheBuild::default();
    builder.build(&vase, &s, &mut out).unwrap();
    let addresses =
        (out.mesh.positions.as_ptr(), out.normals.as_ptr(), out.mesh.indices.as_ptr(), out.profile.as_ptr());

    builder.build(&egg, &s, &mut out).unwrap();
    assert_ne!(out.mesh, fresh.mesh);
    builder.build(&vase, &s, &mut out).unwrap();
    assert_eq!(out.mesh, fresh.mesh);
    assert_eq!(out.normals, fresh.normals);
    assert_eq!(out.profile, fresh.profile);
    assert_eq!(
        addresses,
        (out.mesh.positions.as_ptr(), out.normals.as_ptr(), out.mesh.indices.as_ptr(), out.profile.as_ptr()),
        "a rebuild at or below the earlier size reuses the buffers"
    );
}

#[test]
fn building_in_parallel_gives_exactly_what_building_on_one_thread_does() {
    for preset in [LathePreset::Vase, LathePreset::Bowl, LathePreset::Ring] {
        let profile = preset_profile(preset);
        let s = settings(1024, true, true);

        let mut parallel = LatheBuild::default();
        LatheBuilder::new().build(&profile, &s, &mut parallel).unwrap();
        assert!(parallel.vertex_count() > 8192 * 4, "big enough to be split");

        let mut serial = LatheBuild::default();
        let mut one_thread = LatheBuilder::new();
        one_thread.parallel_leaf_vertices = usize::MAX;
        one_thread.build(&profile, &s, &mut serial).unwrap();

        assert_eq!(parallel.mesh, serial.mesh, "{}", preset.name());
        assert_eq!(parallel.normals, serial.normals, "{}", preset.name());
    }
}

#[test]
fn a_tall_thin_profile_stays_inside_the_vertex_budget() {
    let needle = open_profile(&[ProfilePoint::corner(0.05, 0.0), ProfilePoint::corner(0.05, 20.0)]);
    let r = build(&needle, 1024, true, true);
    assert!(r.vertex_count() <= MAX_VERTICES, "{} vertices", r.vertex_count());
    assert!(r.vertex_count() > MAX_VERTICES / 4, "budget saturates rather than collapsing: {}", r.vertex_count());
    assert!(r.watertight && r.mesh.signed_volume() > 0.0);
}

#[test]
fn segments_round_to_a_multiple_of_four_in_range() {
    assert_eq!(
        [0, 6, 10, 14, 256, 5000].map(round_segments),
        [8, 8, 8, 16, 256, 1024],
        "halves round to even, as Unity's RoundToInt does"
    );
}

#[test]
fn the_preview_is_capped_and_a_draft_is_half_of_that() {
    let full = settings(1024, true, true);
    assert_eq!(full.for_preview(false).radial_segments, 256);
    assert_eq!(full.for_preview(true).radial_segments, 128);
    assert_eq!(settings(64, false, false).for_preview(false), settings(64, false, false));
    assert_eq!(settings(8, true, true).for_preview(true).radial_segments, 8);
}

#[test]
fn centring_and_scaling_bakes_the_transform_into_the_vertices() {
    let mut r = build(&preset_profile(LathePreset::Vase), 64, true, true);
    let before = r.mesh.signed_volume();
    let (min_y, max_y) = (0.0, 1.3);
    let centre = r.centre_and_scale(2.0);
    assert!((centre - (min_y + max_y) * 0.5).abs() < 1e-5);
    let bounds = r.mesh.bounds();
    assert!((bounds.min.y + bounds.max.y).abs() < 1e-4, "vertically centred: {bounds:?}");
    assert!((bounds.max.y - 1.3).abs() < 1e-4, "twice the half-height");
    assert!((r.mesh.signed_volume() - before * 8.0).abs() < before * 8e-4);
    assert_eq!(LatheBuild::default().centre_and_scale(3.0), 0.0);
}

#[test]
fn build_mesh_is_the_same_as_building_into_a_build() {
    let profile = preset_profile(LathePreset::Cup);
    assert_eq!(build_mesh(&profile, &settings(48, true, true)).unwrap(), build(&profile, 48, true, true).mesh);
}

/// `cargo test -p sculpt-lathe --release -- --ignored --nocapture timing`
#[test]
#[ignore = "timing run"]
fn timing() {
    fn time(label: &str, runs: u32, mut f: impl FnMut()) {
        f();
        let start = Instant::now();
        for _ in 0..runs {
            f();
        }
        println!("{label}: {:.3} ms", start.elapsed().as_secs_f64() * 1000.0 / runs as f64);
    }

    let mut builder = LatheBuilder::new();
    let mut out = LatheBuild::default();
    for preset in [LathePreset::Vase, LathePreset::Bowl, LathePreset::Ring] {
        let profile = preset.profile(1.0);
        for (segments, runs) in [(64, 400), (256, 200), (1024, 20)] {
            let s = settings(segments, true, true);
            builder.build(&profile, &s, &mut out).unwrap();
            let label = format!(
                "{} segments={segments} ({} verts, {} tris)",
                preset.name(),
                out.vertex_count(),
                out.triangle_count()
            );
            time(&label, runs, || builder.build(&profile, &s, &mut out).unwrap());
        }
    }

    let vase = LathePreset::Vase.profile(1.0);
    let full = settings(1024, true, true);
    let mut one_thread = LatheBuilder::new();
    one_thread.parallel_leaf_vertices = usize::MAX;
    time("Vase segments=1024, one thread", 20, || one_thread.build(&vase, &full, &mut out).unwrap());
    time("Vase segments=1024, count only", 200, || {
        builder.count(&vase, &full).unwrap();
    });
}
