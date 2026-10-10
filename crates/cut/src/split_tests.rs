//! Splits with answers that can be checked. The first test is the Unity
//! project's `SplitKeyTests.StraightCutMakesTwoClosedPiecesWithOneMatingFace`;
//! the parts of that file that plan and build registration keys belong with
//! the key tools.

use std::collections::HashSet;
use std::time::Instant;

use glam::{Vec2, Vec3};
use sculpt_geom::{SymmetryOp, TriMesh};

use crate::region::{CutView, ScreenRegion};
use crate::split::{Piece, Split, SplitPass, split};
use crate::testing::{assert_closed, assert_near, cube, cube_sphere, icosphere, ortho_view, perspective_view, torus};

const SCREEN: Vec2 = Vec2::new(1000.0, 1000.0);

fn once(view: CutView) -> [SplitPass; 1] {
    [SplitPass { view, symmetry: SymmetryOp::IDENTITY }]
}

fn line(through: Vec2, degrees: f32) -> ScreenRegion {
    let d = Vec2::from_angle(degrees.to_radians());
    ScreenRegion::half_plane(through - d * 100.0, through + d * 100.0).unwrap()
}

fn face_area(mesh: &TriMesh, face: &[u32]) -> f64 {
    face.iter()
        .map(|&t| {
            let [a, b, c] = mesh.triangle(t as usize);
            (b - a).cross(c - a).length() as f64 * 0.5
        })
        .sum()
}

fn face_centre(mesh: &TriMesh, face: &[u32]) -> Vec3 {
    let (mut sum, mut total) = (Vec3::ZERO, 0.0);
    for &t in face {
        let [a, b, c] = mesh.triangle(t as usize);
        let area = (b - a).cross(c - a).length();
        sum += (a + b + c) / 3.0 * area;
        total += area;
    }
    sum / total
}

/// The positions, bit for bit, of the vertices on the rim of a piece's cut
/// faces: those a cut face shares with the rest of the surface.
fn rim_positions(piece: &Piece) -> HashSet<[u32; 3]> {
    let mut on_face = vec![false; piece.mesh.vertex_count()];
    let mut face_triangle = vec![false; piece.mesh.triangle_count()];
    for &t in piece.cut_faces.iter().flatten() {
        face_triangle[t as usize] = true;
        piece.mesh.indices[t as usize * 3..t as usize * 3 + 3].iter().for_each(|&i| on_face[i as usize] = true);
    }
    let mut rim = HashSet::new();
    for (t, corners) in piece.mesh.indices.chunks_exact(3).enumerate() {
        if !face_triangle[t] {
            for &i in corners.iter().filter(|&&i| on_face[i as usize]) {
                rim.insert(piece.mesh.positions[i as usize].to_array().map(f32::to_bits));
            }
        }
    }
    rim
}

/// Both pieces closed, their volumes adding up to the whole, their rims the
/// same points, and each pair of mating faces in the same place.
#[track_caller]
fn assert_mates(result: &Split, whole: f64, tolerance: f64) {
    assert_closed(&result.outside.mesh, "outside");
    assert_closed(&result.inside.mesh, "inside");
    let (outside, inside) = (result.outside.mesh.signed_volume(), result.inside.mesh.signed_volume());
    assert_near(outside + inside, whole, tolerance, "the two volumes together");
    assert_eq!(result.open_loops, 0);
    assert_eq!(rim_positions(&result.outside), rim_positions(&result.inside), "the two rims are the same points");
    for &(o, i) in &result.face_pairs {
        let (a, b) = (&result.outside.cut_faces[o], &result.inside.cut_faces[i]);
        assert_near(face_area(&result.outside.mesh, a), face_area(&result.inside.mesh, b), 0.01, "mating face areas");
        let apart = face_centre(&result.outside.mesh, a).distance(face_centre(&result.inside.mesh, b));
        assert!(apart < 0.01, "mating faces {apart} apart");
    }
}

#[test]
fn straight_cut_makes_two_closed_pieces_with_one_mating_face() {
    let sphere = icosphere(5);
    // An orthographic camera turned 25 and 15 degrees, 1000 px square, with
    // the cut through a point a little off the sphere's centre.
    let view = ortho_view(Vec3::ZERO, 25.0, 15.0, 1.5, Vec2::ZERO, SCREEN);
    let through = view.projection.project(Vec3::new(0.05, 0.02, 0.0)).unwrap();
    for angle in [0.0, 35.0, 90.0] {
        let result = split(&sphere.positions, &sphere.indices, None, &once(view), &line(through, angle)).unwrap();
        assert_mates(&result, sphere.signed_volume(), 1e-4);
        assert_eq!(result.face_pairs, vec![(0, 0)]);
        assert_eq!((result.outside.cut_faces.len(), result.inside.cut_faces.len()), (1, 1));
        assert!(result.warning().is_none());
        // Roughly half each: the cut passes close to the centre.
        let share = result.inside.mesh.signed_volume() / sphere.signed_volume();
        assert!(share > 0.4 && share < 0.6, "inside is {share} of the sphere");
    }
}

#[test]
fn the_covered_side_is_the_inside_piece() {
    let mesh = cube(10);
    let view = ortho_view(Vec3::ZERO, 0.0, 0.0, 1.5, Vec2::ZERO, SCREEN);
    let per_unit = 1000.0 / 3.0;
    // Over everything to the right of x = 0.35.
    let region = ScreenRegion::rect(Vec2::new(500.0 + 0.35 * per_unit, -100.0), Vec2::new(1100.0, 1100.0)).unwrap();
    let mask: Vec<f32> = mesh.positions.iter().map(|p| 0.5 + 0.25 * p.y).collect();
    let result = split(&mesh.positions, &mesh.indices, Some(&mask), &once(view), &region).unwrap();
    assert_mates(&result, 8.0, 1e-4);
    assert_near(result.inside.mesh.signed_volume(), 0.65 * 4.0, 1e-3, "inside volume");
    assert_near(result.outside.mesh.signed_volume(), 1.35 * 4.0, 1e-3, "outside volume");
    assert!(result.inside.mesh.positions.iter().all(|p| p.x > 0.35 - 1e-3));
    assert!(result.outside.mesh.positions.iter().all(|p| p.x < 0.35 + 1e-3));
    assert_eq!(result.cut_face_triangles, result.outside.cut_faces[0].len() + result.inside.cut_faces[0].len());

    // The mask goes with each piece. It was painted level across x, and the
    // cut face is a plane of constant x, so even the new face follows it.
    for piece in [&result.outside, &result.inside] {
        assert_eq!(piece.mask.len(), piece.mesh.vertex_count());
        for (m, p) in piece.mask.iter().zip(&piece.mesh.positions) {
            assert!((m - (0.5 + 0.25 * p.y)).abs() < 0.13, "mask {m} at {p}");
        }
        let shell: Vec<usize> =
            (0..piece.mesh.vertex_count()).filter(|&v| (piece.mesh.positions[v].x - 0.35).abs() > 1e-3).collect();
        assert!(shell.iter().all(|&v| (piece.mask[v] - (0.5 + 0.25 * piece.mesh.positions[v].y)).abs() < 1e-5));
    }
}

#[test]
fn curved_cuts_still_mate() {
    let sphere = icosphere(5);
    let whole = sphere.signed_volume();
    let view = perspective_view(Vec3::ZERO, 30.0, -20.0, 3.0, Vec2::ZERO, SCREEN);
    let wobbly: Vec<Vec2> = (0..60)
        .map(|i| {
            let a = i as f32 / 60.0 * std::f32::consts::TAU;
            Vec2::new(420.0, 520.0) + Vec2::from_angle(a) * (210.0 + 60.0 * (a * 3.0).cos())
        })
        .collect();
    let shapes = [
        // A box whose corner is inside the silhouette, a lasso across the
        // edge of it, and a box wholly inside it (a plug and its hole).
        ScreenRegion::rect(Vec2::new(-50.0, -50.0), Vec2::new(540.0, 470.0)).unwrap(),
        ScreenRegion::lasso(&wobbly).unwrap(),
        ScreenRegion::rect(Vec2::new(440.0, 430.0), Vec2::new(570.0, 590.0)).unwrap(),
    ];
    for (k, region) in shapes.iter().enumerate() {
        let result = split(&sphere.positions, &sphere.indices, None, &once(view), region).unwrap();
        assert_closed(&result.outside.mesh, "outside");
        assert_closed(&result.inside.mesh, "inside");
        let together = result.outside.mesh.signed_volume() + result.inside.mesh.signed_volume();
        assert_near(together, whole, 0.005, "the two volumes together");
        assert_eq!(result.open_loops, 0);
        assert!(
            result.inside.mesh.signed_volume() > 0.02 * whole && result.outside.mesh.signed_volume() > 0.02 * whole
        );
        if k < 2 {
            // Every cut face has its mate (the lasso's lobes leave more than one).
            assert_mates(&result, whole, 0.005);
            assert!(!result.face_pairs.is_empty());
            assert_eq!(result.face_pairs.len(), result.outside.cut_faces.len());
            assert_eq!(result.face_pairs.len(), result.inside.cut_faces.len());
        } else {
            // A tunnel's wall is not a face anything could be keyed to.
            assert!(result.face_pairs.is_empty());
        }
    }
}

#[test]
fn a_ring_splits_into_faces_that_pair_up() {
    let ring = torus(1.0, 0.3, 96, 40);
    let whole = ring.signed_volume();
    let view = ortho_view(Vec3::ZERO, 0.0, 0.0, 1.5, Vec2::ZERO, SCREEN);
    // Level, a little above the middle: one ring-shaped face on each piece.
    let level = split(&ring.positions, &ring.indices, None, &once(view), &line(Vec2::new(500.0, 480.0), 0.0)).unwrap();
    assert_mates(&level, whole, 1e-3);
    assert_eq!(level.face_pairs, vec![(0, 0)]);
    // Upright, beside the axis: two discs on each piece, paired left with
    // left... by where they are, whatever order each side found them in.
    let upright =
        split(&ring.positions, &ring.indices, None, &once(view), &line(Vec2::new(520.0, 500.0), 90.0)).unwrap();
    assert_mates(&upright, whole, 1e-3);
    assert_eq!(upright.face_pairs.len(), 2);
    assert_eq!(upright.face_pairs.iter().map(|p| p.0).collect::<HashSet<_>>().len(), 2);
    assert_eq!(upright.face_pairs.iter().map(|p| p.1).collect::<HashSet<_>>().len(), 2);
}

#[test]
fn mirrored_passes_gather_every_covered_part_into_one_piece() {
    let mesh = cube(12);
    let view = ortho_view(Vec3::ZERO, 0.0, 0.0, 1.5, Vec2::ZERO, SCREEN);
    let per_unit = 1000.0 / 3.0;
    // A slab off the right-hand side, beyond x = 0.55, and its mirror image.
    let region = ScreenRegion::rect(Vec2::new(500.0 + 0.55 * per_unit, -100.0), Vec2::new(1100.0, 1100.0)).unwrap();
    let mirror = [SymmetryOp::IDENTITY, SymmetryOp::from_sign(Vec3::new(-1.0, 1.0, 1.0))];
    let passes = SplitPass::all(&[view], &mirror);
    assert_eq!(passes.len(), 2);
    assert_eq!(SplitPass::all(&[view, view], &[]).len(), 2);
    let mask = vec![0.75f32; mesh.vertex_count()];
    let result = split(&mesh.positions, &mesh.indices, Some(&mask), &passes, &region).unwrap();
    assert_mates(&result, 8.0, 1e-4);
    assert_near(result.inside.mesh.signed_volume(), 2.0 * 0.45 * 4.0, 1e-3, "both slabs");
    assert_near(result.outside.mesh.signed_volume(), 1.1 * 4.0, 1e-3, "the middle");
    assert!(result.outside.mesh.positions.iter().all(|p| p.x.abs() < 0.55 + 1e-3));
    assert!(result.inside.mesh.positions.iter().all(|p| p.x.abs() > 0.55 - 1e-3));

    // The first pass's cut face on the middle piece was carried through the
    // second pass, so there are two faces each side and each knows its mate.
    assert_eq!((result.outside.cut_faces.len(), result.inside.cut_faces.len()), (2, 2));
    assert_eq!(result.face_pairs.len(), 2);
    for &(o, i) in &result.face_pairs {
        let (a, b) = (&result.outside.cut_faces[o], &result.inside.cut_faces[i]);
        let x = face_centre(&result.outside.mesh, a).x;
        assert!((x.abs() - 0.55).abs() < 1e-3 && (face_centre(&result.inside.mesh, b).x - x).abs() < 1e-3);
        assert_near(face_area(&result.outside.mesh, a), 4.0, 1e-3, "face area");
    }
    assert!(result.inside.mask.iter().chain(&result.outside.mask).all(|&m| (m - 0.75).abs() < 1e-6));
    assert_eq!(result.inside.mask.len(), result.inside.mesh.vertex_count());
}

#[test]
fn a_later_pass_that_misses_changes_nothing() {
    let sphere = icosphere(4);
    let view = ortho_view(Vec3::ZERO, 0.0, 0.0, 1.5, Vec2::ZERO, SCREEN);
    let region = line(Vec2::new(430.0, 500.0), 90.0);
    let alone = split(&sphere.positions, &sphere.indices, None, &once(view), &region).unwrap();
    // The same view shifted far off to the side: the shape covers nothing there.
    let away = glam::Mat4::from_translation(Vec3::new(40.0, 0.0, 0.0));
    let elsewhere = CutView::new(view.projection.view_proj * away, view.model_to_view * away, Vec2::ZERO, SCREEN);
    let passes = [
        SplitPass { view, symmetry: SymmetryOp::IDENTITY },
        SplitPass { view: elsewhere, symmetry: SymmetryOp::IDENTITY },
    ];
    let both = split(&sphere.positions, &sphere.indices, None, &passes, &region).unwrap();
    assert_eq!(both.outside.mesh, alone.outside.mesh);
    assert_eq!(both.inside.mesh, alone.inside.mesh);
    assert_eq!(both.face_pairs, alone.face_pairs);
}

#[test]
fn splits_that_do_nothing_say_why() {
    let sphere = icosphere(3);
    let view = ortho_view(Vec3::ZERO, 0.0, 0.0, 1.5, Vec2::ZERO, SCREEN);
    let attempt =
        |region: &ScreenRegion| split(&sphere.positions, &sphere.indices, None, &once(view), region).unwrap_err();
    let beside = ScreenRegion::rect(Vec2::new(900.0, 10.0), Vec2::new(990.0, 90.0)).unwrap();
    let over_all = ScreenRegion::rect(Vec2::new(-10.0, -10.0), Vec2::new(1010.0, 1010.0)).unwrap();
    assert_eq!(attempt(&beside), "Nothing split - the shape covered nothing.");
    assert_eq!(attempt(&over_all), "Nothing split - the shape covers the whole object.");
    assert_eq!(split(&[], &[], None, &once(view), &beside).unwrap_err(), "No geometry.");
    assert_eq!(
        split(&sphere.positions, &sphere.indices, None, &[], &beside).unwrap_err(),
        "Nothing split - the shape covered nothing."
    );
}

/// Timing: `cargo test -p sculpt-cut --release -- --ignored --nocapture`.
#[test]
#[ignore]
fn timing_split_of_a_million_triangle_sphere() {
    let sphere = cube_sphere(290);
    let whole = sphere.signed_volume();
    let size = Vec2::new(1920.0, 1080.0);
    let view = perspective_view(Vec3::ZERO, 20.0, 25.0, 3.2, Vec2::ZERO, size);
    let centre = size * 0.5;
    let wavy: Vec<Vec2> = (0..300)
        .map(|i| {
            let a = i as f32 / 300.0 * std::f32::consts::TAU;
            centre + Vec2::new(-260.0, 0.0) + Vec2::from_angle(a) * (330.0 + 40.0 * (a * 6.0).sin())
        })
        .collect();
    let shapes = [
        ("line", line(centre + Vec2::new(31.0, 17.0), 63.0)),
        ("box over half", ScreenRegion::rect(Vec2::new(-100.0, -100.0), Vec2::new(centre.x + 13.0, 1300.0)).unwrap()),
        ("lasso", ScreenRegion::lasso(&wavy).unwrap()),
    ];
    println!(
        "{} triangles, {} vertices, {} threads",
        sphere.triangle_count(),
        sphere.vertex_count(),
        rayon::current_num_threads()
    );
    let mask = vec![0.5f32; sphere.vertex_count()];
    let mirror = SymmetryOp::from_sign(Vec3::new(-1.0, 1.0, 1.0));
    for (name, region) in &shapes {
        for (label, passes) in [
            ("one pass", vec![SplitPass { view, symmetry: SymmetryOp::IDENTITY }]),
            (
                "mirrored",
                vec![SplitPass { view, symmetry: SymmetryOp::IDENTITY }, SplitPass { view, symmetry: mirror }],
            ),
        ] {
            let run = || split(&sphere.positions, &sphere.indices, Some(&mask), &passes, region);
            if run().is_err() {
                println!("split, {name}, {label}: nothing to split");
                continue;
            }
            let start = Instant::now();
            let result = run().unwrap();
            let elapsed = start.elapsed();
            assert_closed(&result.outside.mesh, name);
            assert_closed(&result.inside.mesh, name);
            let (outside, inside) = (result.outside.mesh.signed_volume(), result.inside.mesh.signed_volume());
            println!(
                "split, {name}, {label}: {:.1} ms; {} + {} triangles, {} on the cut, volumes {:.2}% + {:.2}% = {:.3}%",
                elapsed.as_secs_f64() * 1e3,
                result.outside.mesh.triangle_count(),
                result.inside.mesh.triangle_count(),
                result.cut_face_triangles,
                outside / whole * 100.0,
                inside / whole * 100.0,
                (outside + inside) / whole * 100.0
            );
        }
    }
}

/// Random shapes over random views of several models: whatever is drawn, the
/// pieces that come back are closed, wound outward and add up to the whole.
#[test]
fn random_cuts_always_give_closed_pieces_that_add_up() {
    use crate::testing::Rng;
    // Fine enough, and seen from far enough, that no triangle is wider on
    // screen than the narrowest shape drawn below. A shape that slips between
    // vertices is not resolved: it is found by the vertices it covers. No
    // sculpt is anywhere near that coarse.
    let models = [
        ("sphere", icosphere(4)),
        ("cube", cube(12)),
        ("ring", torus(1.0, 0.35, 64, 28)),
        ("fine sphere", cube_sphere(24)),
    ];
    let mirrors = [
        SymmetryOp::from_sign(Vec3::new(-1.0, 1.0, 1.0)),
        SymmetryOp::from_sign(Vec3::new(1.0, -1.0, 1.0)),
        SymmetryOp::rotation(Vec3::Y, 1, 3),
    ];
    let mut rng = Rng::new(2026);
    let (mut splits, mut misses) = (0, 0);
    for round in 0..240 {
        let (name, mesh) = &models[round % models.len()];
        let whole = mesh.signed_volume();
        let (yaw, pitch) = (rng.range(-180.0, 180.0), rng.range(-80.0, 80.0));
        // Every so often dead on an axis, where rows of vertices line up.
        let (yaw, pitch) = if round % 5 == 0 { ((yaw / 90.0).round() * 90.0, 0.0) } else { (yaw, pitch) };
        let view = if round % 2 == 0 {
            ortho_view(Vec3::ZERO, yaw, pitch, 1.8, Vec2::ZERO, SCREEN)
        } else {
            perspective_view(Vec3::ZERO, yaw, pitch, rng.range(4.0, 7.0), Vec2::ZERO, SCREEN)
        };
        let at = |rng: &mut Rng| Vec2::new(rng.range(150.0, 850.0), rng.range(150.0, 850.0));
        let region = match round % 3 {
            0 => line(if round % 5 == 0 { SCREEN * 0.5 } else { at(&mut rng) }, rng.range(0.0, 360.0)),
            1 => {
                let half = Vec2::new(rng.range(80.0, 300.0), rng.range(80.0, 300.0));
                let centre = at(&mut rng);
                ScreenRegion::rect(centre - half, centre + half).unwrap()
            }
            _ => {
                let (centre, size, lobes) = (at(&mut rng), rng.range(80.0, 400.0), (rng.range(2.0, 6.0)) as i32 as f32);
                let wobble = rng.range(0.0, 0.5);
                let outline: Vec<Vec2> = (0..48)
                    .map(|i| {
                        let a = i as f32 / 48.0 * std::f32::consts::TAU;
                        centre + Vec2::from_angle(a) * size * (1.0 + wobble * (a * lobes).sin())
                    })
                    .collect();
                ScreenRegion::lasso(&outline).unwrap()
            }
        };
        let mut passes = vec![SplitPass { view, symmetry: SymmetryOp::IDENTITY }];
        if round % 4 == 3 {
            passes.push(SplitPass { view, symmetry: mirrors[(round / 4) % mirrors.len()] });
        }
        let what = format!("round {round}, {name}, yaw {yaw} pitch {pitch}");
        match split(&mesh.positions, &mesh.indices, None, &passes, &region) {
            Ok(result) => {
                splits += 1;
                assert_closed(&result.outside.mesh, &format!("{what}: outside"));
                assert_closed(&result.inside.mesh, &format!("{what}: inside"));
                assert_eq!(result.open_loops, 0, "{what}");
                let together = result.outside.mesh.signed_volume() + result.inside.mesh.signed_volume();
                assert_near(together, whole, 0.02, &what);
            }
            Err(error) => {
                misses += 1;
                assert!(error.starts_with("Nothing split - "), "{what}: {error}");
            }
        }
    }
    assert!(splits > 150, "{splits} splits, {misses} misses");
}
