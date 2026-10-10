//! Trims with answers that can be checked: closed results of a known volume.

use std::f64::consts::PI;
use std::time::Instant;

use glam::{Vec2, Vec3};
use sculpt_geom::{SymmetryOp, TriMesh};

use crate::region::{CutView, ScreenRegion};
use crate::testing::{assert_closed, assert_near, cube, cube_sphere, icosphere, ortho_view, perspective_view, torus};
use crate::trim::{CUT_FACE, ON_BOUNDARY_PIXELS, Trimmed, trim, trim_once, trim_passes};

const SCREEN: Vec2 = Vec2::new(1000.0, 800.0);
const CENTRE: Vec2 = Vec2::new(500.0, 400.0);
/// Pixels per world unit under `front`.
const SCALE: f32 = 800.0 / 3.0;

/// Straight on from +Z: +X is right on screen and +Y is up.
fn front() -> CutView {
    ortho_view(Vec3::ZERO, 0.0, 0.0, 1.5, Vec2::ZERO, SCREEN)
}

/// Where a world point lands on screen under `front`.
fn px(x: f32, y: f32) -> Vec2 {
    CENTRE + Vec2::new(x, -y) * SCALE
}

fn cut(mesh: &TriMesh, view: &CutView, region: &ScreenRegion, remove_covered: bool) -> Trimmed {
    trim(&mesh.positions, &mesh.indices, None, view, &SymmetryOp::IDENTITY, region, remove_covered).unwrap()
}

/// A line across the whole screen through `through`, at `degrees` from the
/// screen's x axis.
fn line(through: Vec2, degrees: f32) -> ScreenRegion {
    let d = Vec2::from_angle(degrees.to_radians());
    ScreenRegion::half_plane(through - d * 100.0, through + d * 100.0).unwrap()
}

fn face_area(mesh: &TriMesh, triangles: std::ops::Range<u32>) -> f64 {
    triangles
        .map(|t| {
            let [a, b, c] = mesh.triangle(t as usize);
            (b - a).cross(c - a).length() as f64 * 0.5
        })
        .sum()
}

#[test]
fn a_straight_cut_through_a_sphere_leaves_half() {
    let sphere = icosphere(5);
    let whole = sphere.signed_volume();
    for view in [
        ortho_view(Vec3::ZERO, 25.0, 15.0, 1.5, Vec2::ZERO, SCREEN),
        perspective_view(Vec3::ZERO, -30.0, 20.0, 3.0, Vec2::ZERO, SCREEN),
    ] {
        for degrees in [0.0, 35.0, 90.0, 200.0] {
            // Through the sphere's centre on screen, so the cutting plane
            // passes through its centre in space under either camera.
            let region = line(CENTRE, degrees);
            for remove_covered in [true, false] {
                let half = cut(&sphere, &view, &region, remove_covered);
                assert_closed(&half.mesh, "half sphere");
                assert_near(half.mesh.signed_volume(), whole * 0.5, 0.005, "half sphere volume");
                assert_eq!((half.open_loops, half.approximated_cap_faces), (0, 0));
                assert_eq!(half.cut_faces.len(), 1);
                assert_eq!(half.cut_faces[0].len(), half.cap_triangles);
                // The cut face is a flat disc through the centre.
                assert_near(face_area(&half.mesh, half.cut_faces[0].clone()), PI, 0.01, "cut face area");
                assert_eq!(half.source_triangles.len(), half.mesh.triangle_count());
            }
        }
    }
}

#[test]
fn a_box_over_half_the_screen_halves_a_cube() {
    let mesh = cube(15);
    let view = front();
    // The box's right edge at x = 0.3; its other three edges are off the model.
    let region = ScreenRegion::rect(Vec2::new(-200.0, -200.0), Vec2::new(px(0.3, 0.0).x, 1200.0)).unwrap();
    let right = cut(&mesh, &view, &region, true);
    assert_closed(&right.mesh, "right part");
    assert_near(right.mesh.signed_volume(), 0.7 * 4.0, 1e-3, "right part volume");
    assert!(right.mesh.positions.iter().all(|p| p.x > 0.3 - 1e-3));

    let left = cut(&mesh, &view, &region, false);
    assert_closed(&left.mesh, "left part");
    assert_near(left.mesh.signed_volume(), 1.3 * 4.0, 1e-3, "left part volume");
    assert!(left.mesh.positions.iter().all(|p| p.x < 0.3 + 1e-3));
    assert_near(face_area(&left.mesh, left.cut_faces[0].clone()), 4.0, 1e-3, "cut face area");

    // The face is filled at about the density of the cube around it, not left
    // as a fan of the rim.
    let shell_triangle = 0.5 * (2.0f64 / 15.0).powi(2);
    let cap_triangle = 4.0 / left.cap_triangles as f64;
    assert!(
        cap_triangle < shell_triangle * 3.0 && cap_triangle > shell_triangle / 3.0,
        "{cap_triangle} against {shell_triangle}"
    );
}

#[test]
fn a_cut_exactly_through_a_row_of_vertices_still_closes() {
    // The box's edge runs along x = 0, where the cube has a whole row of
    // vertices: every crossing lands a hair from one of them.
    let mesh = cube(8);
    let view = front();
    let region = ScreenRegion::rect(Vec2::new(-200.0, -200.0), Vec2::new(CENTRE.x, 1200.0)).unwrap();
    for remove_covered in [true, false] {
        let half = cut(&mesh, &view, &region, remove_covered);
        assert_closed(&half.mesh, "half cube");
        assert_near(half.mesh.signed_volume(), 4.0, 1e-3, "half cube volume");
        assert_eq!(half.open_loops, 0);
    }
    // And through the poles of a sphere, where the icosahedron's own vertices
    // sit on the cutting plane.
    let sphere = icosphere(4);
    let half = cut(&sphere, &view, &line(CENTRE, 90.0), true);
    assert_closed(&half.mesh, "half sphere");
    assert_near(half.mesh.signed_volume(), sphere.signed_volume() * 0.5, 0.005, "half sphere volume");
}

#[test]
fn a_box_corner_inside_the_model_cuts_a_square_notch() {
    // The box covers the top-left quarter of the cube as seen from the front,
    // so its corner is inside the silhouette and the cut face bends round it.
    let mesh = cube(15);
    let region = ScreenRegion::rect(Vec2::new(-200.0, -200.0), px(0.1, -0.2)).unwrap();
    let notched = cut(&mesh, &front(), &region, true);
    assert_closed(&notched.mesh, "notched cube");
    assert_near(notched.mesh.signed_volume(), 8.0 - 1.1 * 1.2 * 2.0, 0.01, "notched volume");
    assert_eq!((notched.cut_faces.len(), notched.open_loops, notched.approximated_cap_faces), (1, 0, 0));

    // Every vertex of the cut face lies on one of the two walls, none on a
    // flat sheet stretched across the corner.
    let on_cut_face = cut_face_only_vertices(&notched);
    assert!(on_cut_face.len() > 50);
    for &v in &on_cut_face {
        let p = notched.mesh.positions[v as usize];
        assert!((p.x - 0.1).abs() < 1e-3 || (p.y + 0.2).abs() < 1e-3, "{p} is off both walls");
    }
    // Keeping the covered part instead gives the block that was cut away.
    let block = cut(&mesh, &front(), &region, false);
    assert_closed(&block.mesh, "block");
    assert_near(block.mesh.signed_volume(), 1.1 * 1.2 * 2.0, 0.01, "block volume");
}

/// Vertices used by cut-face triangles and nothing else.
fn cut_face_only_vertices(result: &Trimmed) -> Vec<u32> {
    let mut on_shell = vec![false; result.mesh.vertex_count()];
    let mut on_face = vec![false; result.mesh.vertex_count()];
    for (t, corners) in result.mesh.indices.chunks_exact(3).enumerate() {
        let flags = if result.source_triangles[t] == CUT_FACE { &mut on_face } else { &mut on_shell };
        corners.iter().for_each(|&i| flags[i as usize] = true);
    }
    (0..result.mesh.vertex_count() as u32).filter(|&v| on_face[v as usize] && !on_shell[v as usize]).collect()
}

#[test]
fn a_concave_lasso_cuts_round_its_inside_corner() {
    // Everything except the top-right quarter of the screen: an L whose
    // inside corner sits in the middle of the cube.
    let corner = px(0.15, 0.25);
    let outline = [
        Vec2::new(-50.0, -50.0),
        Vec2::new(corner.x, -50.0),
        corner,
        Vec2::new(1050.0, corner.y),
        Vec2::new(1050.0, 850.0),
        Vec2::new(-50.0, 850.0),
    ];
    let region = ScreenRegion::lasso(&outline).unwrap();
    let mesh = cube(15);
    let kept = cut(&mesh, &front(), &region, true);
    assert_closed(&kept.mesh, "quarter block");
    assert_near(kept.mesh.signed_volume(), 0.85 * 0.75 * 2.0, 0.01, "quarter block volume");
    assert_eq!((kept.cut_faces.len(), kept.open_loops), (1, 0));
    assert!(kept.mesh.positions.iter().all(|p| p.x > 0.15 - 2e-3 && p.y > 0.25 - 2e-3));

    let removed = cut(&mesh, &front(), &region, false);
    assert_closed(&removed.mesh, "the L");
    assert_near(removed.mesh.signed_volume(), 8.0 - 0.85 * 0.75 * 2.0, 0.01, "the L's volume");
}

fn star_outline(centre: Vec2, outer: f32, inner: f32) -> Vec<Vec2> {
    (0..10)
        .map(|i| {
            let a = i as f32 * std::f32::consts::PI / 5.0 + 0.3;
            centre + Vec2::from_angle(a) * if i % 2 == 0 { outer } else { inner }
        })
        .collect()
}

#[test]
fn a_shape_inside_the_silhouette_punches_a_tunnel() {
    let mesh = cube(24);
    let view = front();
    // A box, then a star: the star's wall is concave all the way through.
    let rect = ScreenRegion::rect(px(-0.45, 0.35), px(0.25, -0.55)).unwrap();
    let star = star_outline(px(0.05, -0.1), 0.7 * SCALE, 0.33 * SCALE);
    let star_area = crate::polygon::signed_area(&star).abs() / (SCALE as f64).powi(2);
    let shapes = [(rect, 0.7 * 0.9, 0.002), (ScreenRegion::lasso(&star).unwrap(), star_area, 0.02)];
    for (region, area, tolerance) in &shapes {
        let holed = cut(&mesh, &view, region, true);
        assert_closed(&holed.mesh, "cube with a tunnel");
        assert_near(holed.mesh.signed_volume(), 8.0 - area * 2.0, *tolerance, "tunnelled volume");
        assert_eq!(holed.open_loops, 0);
        assert!(holed.cut_faces.is_empty(), "a tunnel wall is not a cut face");
        assert!(holed.cap_triangles > 100);

        // Cropping to the same shape keeps the plug that came out of the hole.
        let plug = cut(&mesh, &view, region, false);
        assert_closed(&plug.mesh, "plug");
        assert_near(plug.mesh.signed_volume(), area * 2.0, *tolerance * 4.0, "plug volume");
        assert_eq!(plug.open_loops, 0);
    }
}

#[test]
fn a_tunnel_through_a_sphere_has_a_wall_of_even_quads() {
    let sphere = icosphere(5);
    let (a, b) = (0.35f64, 0.25f64);
    let region = ScreenRegion::rect(px(-a as f32, b as f32), px(a as f32, -b as f32)).unwrap();
    let holed = cut(&sphere, &front(), &region, true);
    assert_closed(&holed.mesh, "sphere with a tunnel");

    // The prism's volume inside the unit sphere, by the midpoint rule.
    let steps = 400;
    let mut removed = 0.0;
    for i in 0..steps {
        for j in 0..steps {
            let (x, y) =
                (-a + 2.0 * a * (i as f64 + 0.5) / steps as f64, -b + 2.0 * b * (j as f64 + 0.5) / steps as f64);
            removed += 2.0 * (1.0 - x * x - y * y).sqrt();
        }
    }
    removed *= (2.0 * a / steps as f64) * (2.0 * b / steps as f64);
    assert_near(holed.mesh.signed_volume(), sphere.signed_volume() - removed, 0.005, "tunnelled volume");

    // The wall is as long as the sphere is thick. Built as rings an edge apart
    // it has no needle reaching from one opening to the other.
    let longest = (0..holed.mesh.triangle_count())
        .filter(|&t| holed.source_triangles[t] == CUT_FACE)
        .flat_map(|t| {
            let [p, q, r] = holed.mesh.triangle(t);
            [p.distance(q), q.distance(r), r.distance(p)]
        })
        .fold(0.0, f32::max);
    assert!(longest < 0.25, "a wall edge {longest} long");
}

#[test]
fn a_cut_across_a_ring_leaves_one_face_with_a_hole() {
    // A torus round the y axis seen from the front, cut along a level line: the
    // cutting plane meets it in two concentric circles, which must be closed
    // by one ring-shaped face, not two discs stacked on each other.
    let (major, minor) = (1.0f64, 0.3f64);
    let ring = torus(major as f32, minor as f32, 96, 40);
    let whole = ring.signed_volume();
    for height in [0.0f64, 0.11, -0.2] {
        // Covers everything above the line on screen: y greater than `height`.
        let region = line(px(0.0, height as f32), 0.0);
        let below = cut(&ring, &front(), &region, true);
        assert_closed(&below.mesh, "lower part of the ring");
        // The part of the tube's circular section below the line.
        let section = minor * minor * (-height / minor).acos() + height * (minor * minor - height * height).sqrt();
        let expected = whole * section / (PI * minor * minor);
        assert_near(below.mesh.signed_volume(), expected, 0.01, "lower part volume");
        assert_eq!((below.cut_faces.len(), below.open_loops, below.approximated_cap_faces), (1, 0, 0));
        let chord = (minor * minor - height * height).sqrt();
        let annulus = PI * ((major + chord).powi(2) - (major - chord).powi(2));
        assert_near(face_area(&below.mesh, below.cut_faces[0].clone()), annulus, 0.02, "ring face area");
        assert!(below.mesh.positions.iter().all(|p| (p.y as f64) < height + 1e-3));
    }

    // Cut the other way, through the axis, the same plane meets the tube in
    // two separate circles: two faces, each a plain disc.
    let halves = cut(&ring, &front(), &line(px(0.07, 0.0), 90.0), true);
    assert_closed(&halves.mesh, "half ring");
    assert_eq!((halves.cut_faces.len(), halves.open_loops), (2, 0));
    for face in &halves.cut_faces {
        assert_near(face_area(&halves.mesh, face.clone()), PI * minor * minor, 0.02, "disc face area");
    }
}

#[test]
fn a_hollow_shell_is_closed_by_a_ring_on_each_side() {
    // A sphere with a cavity: the outer skin, and a smaller sphere turned
    // inside out within it.
    let mut shell = icosphere(4);
    let mut cavity = icosphere(3);
    cavity.positions.iter_mut().for_each(|p| *p *= 0.6);
    cavity.flip();
    shell.append(&cavity);
    let whole = shell.signed_volume();
    assert!(whole > 0.0);

    // Everything beyond x = 0.013 goes: half, less a thin slice of the wall.
    let half = cut(&shell, &front(), &line(px(0.013, 0.0), 90.0), true);
    assert_closed(&half.mesh, "half shell");
    assert_near(half.mesh.signed_volume(), whole * 0.5 + 0.013 * PI * 0.64, 0.01, "half shell volume");
    assert_eq!((half.cut_faces.len(), half.open_loops), (1, 0));
    assert_near(face_area(&half.mesh, half.cut_faces[0].clone()), PI * (1.0 - 0.36), 0.02, "ring face area");
}

#[test]
fn a_mask_survives_the_cut() {
    let sphere = icosphere(5);
    // Linear in position, so the value interpolated along any cut edge is
    // exactly the value at the cut point.
    let paint = |p: Vec3| 0.5 + 0.3 * p.x - 0.15 * p.y;
    let mask: Vec<f32> = sphere.positions.iter().map(|&p| paint(p)).collect();
    let view = ortho_view(Vec3::ZERO, 25.0, 15.0, 1.5, Vec2::ZERO, SCREEN);
    let shapes = [line(CENTRE, 20.0), ScreenRegion::rect(px(-0.3, 0.3), px(0.2, -0.3)).unwrap()];
    for region in &shapes {
        let result =
            trim(&sphere.positions, &sphere.indices, Some(&mask), &view, &SymmetryOp::IDENTITY, region, true).unwrap();
        assert_eq!(result.mask.len(), result.mesh.vertex_count());
        let face_only = cut_face_only_vertices(&result);
        let mut is_face_only = vec![false; result.mesh.vertex_count()];
        face_only.iter().for_each(|&v| is_face_only[v as usize] = true);
        let (lo, hi) = mask.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &m| (lo.min(m), hi.max(m)));
        for (v, (&m, &p)) in result.mask.iter().zip(&result.mesh.positions).enumerate() {
            if is_face_only[v] {
                // Blended from the rim: within the range of what was painted.
                assert!(m >= lo && m <= hi, "cut face mask {m}");
            } else {
                assert!((m - paint(p)).abs() < 1e-4, "mask {m} at {p} should be {}", paint(p));
            }
        }
        assert!(!face_only.is_empty());
    }
    // With no mask in, none comes out.
    assert!(cut(&sphere, &view, &shapes[0], true).mask.is_empty());
}

#[test]
fn untouched_triangles_come_through_exactly_and_say_where_from() {
    let sphere = icosphere(4);
    let result = cut(&sphere, &front(), &line(px(0.21, 0.0), 90.0), true);
    let mut whole = 0;
    for (t, &source) in result.source_triangles.iter().enumerate() {
        if source == CUT_FACE {
            continue;
        }
        let (ours, theirs) = (result.mesh.triangle(t), sphere.triangle(source as usize));
        if ours == theirs {
            whole += 1;
        } else {
            // A clipped piece: in the plane of the triangle it was cut from,
            // and facing the same way.
            let normal = (theirs[1] - theirs[0]).cross(theirs[2] - theirs[0]).normalize();
            assert!(ours.iter().all(|p| (*p - theirs[0]).dot(normal).abs() < 1e-5));
            assert!((ours[1] - ours[0]).cross(ours[2] - ours[0]).dot(normal) > 0.0);
        }
    }
    assert!(whole > sphere.triangle_count() / 4, "{whole} triangles carried across whole");
    assert!(result.cut_faces.iter().all(|face| face.clone().all(|t| result.source_triangles[t as usize] == CUT_FACE)));
}

#[test]
fn cuts_that_do_nothing_say_why() {
    let sphere = icosphere(3);
    let view = front();
    let attempt = |region: &ScreenRegion, remove_covered: bool| {
        trim(&sphere.positions, &sphere.indices, None, &view, &SymmetryOp::IDENTITY, region, remove_covered)
            .unwrap_err()
    };
    let beside = ScreenRegion::rect(Vec2::new(900.0, 10.0), Vec2::new(990.0, 90.0)).unwrap();
    let over_all = ScreenRegion::rect(Vec2::new(-10.0, -10.0), Vec2::new(1010.0, 810.0)).unwrap();
    assert_eq!(attempt(&beside, true), "the shape covered nothing");
    assert_eq!(attempt(&over_all, true), "that would delete the whole object");
    assert_eq!(attempt(&beside, false), "that would delete the whole object");
    assert_eq!(trim(&[], &[], None, &view, &SymmetryOp::IDENTITY, &beside, true).unwrap_err(), "no geometry");

    let none = trim_passes(&sphere.positions, &sphere.indices, None, &[view], &[], &beside, true).unwrap_err();
    assert_eq!(none, "Nothing trimmed - the shape covered nothing.");
    let all = trim_passes(&sphere.positions, &sphere.indices, None, &[view], &[], &over_all, true).unwrap_err();
    assert_eq!(all, "Nothing trimmed - that would delete the whole object.");
    assert_eq!(trim_passes(&[], &[], None, &[view], &[], &beside, true).unwrap_err(), "No geometry.");
}

#[test]
fn symmetry_repeats_a_cut_and_a_crop_ignores_it() {
    let mesh = cube(12);
    let view = front();
    // A slab off the right-hand side: x beyond 0.55.
    let region = ScreenRegion::rect(px(0.55, 3.0), px(3.0, -3.0)).unwrap();
    let mirror = [SymmetryOp::IDENTITY, SymmetryOp::from_sign(Vec3::new(-1.0, 1.0, 1.0))];
    let mask = vec![0.25f32; mesh.vertex_count()];

    let once = trim_passes(&mesh.positions, &mesh.indices, None, &[view], &[], &region, true).unwrap();
    assert_near(once.mesh.signed_volume(), 8.0 - 0.45 * 4.0, 1e-3, "one slab off");
    assert_eq!(once.cuts, 1);

    let both = trim_passes(&mesh.positions, &mesh.indices, Some(&mask), &[view], &mirror, &region, true).unwrap();
    assert_closed(&both.mesh, "both slabs off");
    assert_near(both.mesh.signed_volume(), 8.0 - 2.0 * 0.45 * 4.0, 1e-3, "both slabs off");
    assert_eq!((both.cuts, both.open_loops), (2, 0));
    assert!(both.mesh.positions.iter().all(|p| p.x.abs() < 0.55 + 1e-3));
    assert_eq!(both.mask.len(), both.mesh.vertex_count());
    assert!(both.mask.iter().all(|&m| (m - 0.25).abs() < 1e-6), "a flat mask stays flat through two cuts");
    assert_eq!(
        both.message(),
        format!(
            "Trimmed: 1,728 -> {} triangles, {} of them the new cut face.",
            group(both.mesh.triangle_count()),
            group(both.cap_triangles)
        )
    );

    // Cropping keeps what the shape covers. Mirrored it would keep only the
    // overlap of the two slabs, which is nothing, so it runs as drawn.
    let slab = trim_passes(&mesh.positions, &mesh.indices, None, &[view], &mirror, &region, false).unwrap();
    assert_closed(&slab.mesh, "slab");
    assert_near(slab.mesh.signed_volume(), 0.45 * 4.0, 1e-3, "slab volume");
    assert_eq!(slab.cuts, 1);

    // A second place the object is drawn (a live mirror copy, here simply the
    // same object shown shifted): the shape over the copy cuts too.
    let shift = glam::Mat4::from_translation(Vec3::new(-1.1, 0.0, 0.0));
    let copy = CutView::new(view.projection.view_proj * shift, view.model_to_view * shift, Vec2::ZERO, SCREEN);
    let twice = trim_passes(&mesh.positions, &mesh.indices, None, &[view, copy], &[], &region, true).unwrap();
    assert_closed(&twice.mesh, "cut through both views");
    // The copy shows the object 1.1 to the left, so over it the shape covers
    // x beyond 1.65: off the object. That pass misses and only the first lands.
    assert_eq!(twice.cuts, 1);
    // Shown 0.3 to the right instead, the shape covers the copy's x beyond 0.25.
    let nearer = glam::Mat4::from_translation(Vec3::new(0.3, 0.0, 0.0));
    let copy = CutView::new(view.projection.view_proj * nearer, view.model_to_view * nearer, Vec2::ZERO, SCREEN);
    let twice = trim_passes(&mesh.positions, &mesh.indices, None, &[view, copy], &[], &region, true).unwrap();
    assert_closed(&twice.mesh, "cut through both views");
    assert_eq!(twice.cuts, 2);
    assert_near(twice.mesh.signed_volume(), (1.0 + 0.25) * 4.0, 1e-3, "cut at 0.55 and then at 0.25");
}

fn group(n: usize) -> String {
    let s = n.to_string();
    if s.len() > 3 { format!("{},{}", &s[..s.len() - 3], &s[s.len() - 3..]) } else { s }
}

#[test]
fn an_open_mesh_is_cut_and_its_loose_ends_reported() {
    // A cube with its front face missing: the cut runs off the open edge.
    let mut open = cube(6);
    let keep: Vec<u32> = (0..open.triangle_count())
        .filter(|&t| !open.triangle(t).iter().all(|p| p.z > 0.999))
        .flat_map(|t| open.indices[t * 3..t * 3 + 3].to_vec())
        .collect();
    open.indices = keep;
    let view = front();
    let passes =
        trim_passes(&open.positions, &open.indices, None, &[view], &[], &line(px(0.1, 0.0), 90.0), true).unwrap();
    assert!(passes.open_loops > 0);
    assert!(passes.message().ends_with("could not be closed cleanly."), "{}", passes.message());
    assert!(passes.mesh.positions.iter().all(|p| p.x < 0.1 + 1e-3));
}

#[test]
fn limits_hold_at_their_lowest_inputs() {
    use crate::trim as limits;
    assert_eq!(limits::cap_vertex_budget(0), 512);
    assert_eq!(limits::cap_vertex_budget(1), 512);
    assert_eq!(limits::cap_vertex_budget(13), 520);
    assert_eq!(limits::cap_vertex_budget(1_000_000), 60_000);
    assert_eq!(limits::tunnel_ring_count(0.0, 1.0), 0);
    assert_eq!(limits::tunnel_ring_count(1.4, 1.0), 0);
    assert_eq!(limits::tunnel_ring_count(2.6, 1.0), 2);
    assert_eq!(limits::tunnel_ring_count(1e9, 1.0), 400);
    assert_eq!(limits::tunnel_ring_count(f32::NAN, 1.0), 0);
    assert_eq!(limits::tunnel_ring_vertices(0.0, 1.0), 8);
    assert_eq!(limits::tunnel_ring_vertices(1.0, 1.0), 8);
    assert_eq!(limits::tunnel_ring_vertices(20.4, 1.0), 20);
    assert_eq!(limits::tunnel_ring_vertices(1e9, 1.0), 4000);
}

/// Timing: `cargo test -p sculpt-cut --release -- --ignored --nocapture`.
#[test]
#[ignore]
fn timing_trim_of_a_million_triangle_sphere() {
    let sphere = cube_sphere(290);
    let whole = sphere.signed_volume();
    let view = perspective_view(Vec3::ZERO, 20.0, 25.0, 3.2, Vec2::ZERO, Vec2::new(1920.0, 1080.0));
    let centre = Vec2::new(960.0, 540.0);
    let wavy: Vec<Vec2> = (0..300)
        .map(|i| {
            let a = i as f32 / 300.0 * std::f32::consts::TAU;
            centre + Vec2::new(-260.0, 0.0) + Vec2::from_angle(a) * (330.0 + 40.0 * (a * 6.0).sin())
        })
        .collect();
    let shapes = [
        ("line", line(centre + Vec2::new(31.0, 17.0), 63.0)),
        ("box over half", ScreenRegion::rect(Vec2::new(-100.0, -100.0), Vec2::new(centre.x + 13.0, 1300.0)).unwrap()),
        ("box tunnel", ScreenRegion::rect(centre - Vec2::new(90.0, 60.0), centre + Vec2::new(70.0, 80.0)).unwrap()),
        ("lasso", ScreenRegion::lasso(&wavy).unwrap()),
    ];
    println!(
        "{} triangles, {} vertices, {} threads",
        sphere.triangle_count(),
        sphere.vertex_count(),
        rayon::current_num_threads()
    );
    let mask = vec![0.5f32; sphere.vertex_count()];
    for (name, region) in &shapes {
        let run = || {
            trim(&sphere.positions, &sphere.indices, Some(&mask), &view, &SymmetryOp::IDENTITY, region, true).unwrap()
        };
        run();
        let start = Instant::now();
        let result = run();
        let elapsed = start.elapsed();
        assert_closed(&result.mesh, name);
        println!(
            "trim, {name}: {:.1} ms; {} -> {} triangles, {} on the cut, volume {:.1}% of the sphere",
            elapsed.as_secs_f64() * 1e3,
            sphere.triangle_count(),
            result.mesh.triangle_count(),
            result.cap_triangles,
            result.mesh.signed_volume() / whole * 100.0
        );
    }
}

/// The bits of every vertex position, for exact comparisons.
fn position_bits(positions: &[Vec3]) -> Vec<[u32; 3]> {
    positions.iter().map(|p| p.to_array().map(f32::to_bits)).collect()
}

#[test]
fn a_cut_along_a_row_of_vertices_uses_those_vertices() {
    // Through x = 0, where the cube has a row of vertices all the way round.
    // The rim should be made of those vertices, not of new ones beside them.
    let mesh = cube(8);
    let originals: std::collections::HashSet<[u32; 3]> = position_bits(&mesh.positions).into_iter().collect();
    let view = front();
    for (region, remove_covered) in [
        (ScreenRegion::rect(Vec2::new(-200.0, -200.0), Vec2::new(CENTRE.x, 1200.0)).unwrap(), true),
        (ScreenRegion::rect(Vec2::new(-200.0, -200.0), Vec2::new(CENTRE.x, 1200.0)).unwrap(), false),
        (line(CENTRE, 90.0), true),
        (line(CENTRE, 90.0), false),
    ] {
        let half = cut(&mesh, &view, &region, remove_covered);
        assert_closed(&half.mesh, "half cube");
        assert_near(half.mesh.signed_volume(), 4.0, 1e-5, "half cube volume");
        let face_only = cut_face_only_vertices(&half);
        let bits = position_bits(&half.mesh.positions);
        for (v, position) in bits.iter().enumerate() {
            if !face_only.contains(&(v as u32)) {
                assert!(originals.contains(position), "{} is a new vertex", half.mesh.positions[v]);
            }
        }
        let distinct: std::collections::HashSet<&[u32; 3]> = bits.iter().collect();
        assert_eq!(distinct.len(), bits.len(), "no two vertices in the same place");
        // Half the cube's vertices plus the row itself, and the face's own.
        let on_side = mesh
            .positions
            .iter()
            .filter(|p| if half.mesh.positions[0].x > 0.0 { p.x >= 0.0 } else { p.x <= 0.0 })
            .count();
        assert_eq!(half.mesh.vertex_count(), on_side + face_only.len());
        assert_eq!((half.open_loops, half.approximated_cap_faces, half.cut_faces.len()), (0, 0, 1));
    }
}

#[test]
fn an_outline_pinching_the_surface_at_a_vertex_is_cut_the_long_way() {
    // A bow tie pinched almost shut exactly over the middle of the cube's
    // front face. The vertex there is on the outline, with surviving surface
    // right above and right below it, so both rims would have to run through
    // it. That attempt is refused, and the cut made with the vertex on one
    // side instead.
    let mesh = cube(8);
    let view = front();
    let pinch = ON_BOUNDARY_PIXELS * 0.5;
    let outline = [
        px(-1.2, 0.8),
        CENTRE - Vec2::new(0.0, pinch),
        px(1.2, 0.8),
        px(1.2, -0.8),
        CENTRE + Vec2::new(0.0, pinch),
        px(-1.2, -0.8),
    ];
    let region = ScreenRegion::lasso(&outline).unwrap();
    let identity = SymmetryOp::IDENTITY;
    let attempt =
        |band| trim_once(&mesh.positions, &mesh.indices, None, &view, &identity, &region, true, band).unwrap();
    assert!(attempt(ON_BOUNDARY_PIXELS).is_none(), "the first attempt should have been refused");
    assert!(attempt(0.0).is_some());

    let result = cut(&mesh, &view, &region, true);
    assert_closed(&result.mesh, "top and bottom wedges");
    assert_eq!(result.open_loops, 0);
    // Above and below the bow tie: two wedges, each the face less a triangle.
    let wedge = 2.0 - 0.5 * 2.0 * (0.8 / 1.2);
    assert_near(result.mesh.signed_volume(), 2.0 * wedge * 2.0, 0.01, "wedges volume");
}

#[test]
fn a_vertex_on_the_outline_with_nothing_kept_round_it_leaves_no_speck() {
    // A sphere turned so one vertex points straight up, and a line a hair
    // under that vertex with everything below it going. Every triangle round
    // the vertex goes too, and the vertex with them: not a speck of a pyramid,
    // and not a vertex attached to nothing. (A second, untouched ball above
    // keeps the cut from simply deleting everything.)
    let mut mesh = icosphere(2);
    let turn = glam::Quat::from_rotation_arc(mesh.positions[0], Vec3::Y);
    mesh.positions.iter_mut().for_each(|p| *p = turn * *p);
    let mut ball = icosphere(1);
    ball.positions.iter_mut().for_each(|p| *p = *p * 0.2 + Vec3::new(0.0, 1.3, 0.0));
    let (ball_volume, ball_vertices) = (ball.signed_volume(), ball.vertex_count());
    mesh.append(&ball);

    let view = front();
    // Drawn right to left, the covered side is below the line.
    let region = line(px(0.0, 1.0) + Vec2::new(0.0, ON_BOUNDARY_PIXELS * 0.5), 180.0);
    let left = cut(&mesh, &view, &region, true);
    assert_closed(&left.mesh, "the ball");
    assert_near(left.mesh.signed_volume(), ball_volume, 1e-6, "the ball alone");
    assert_eq!((left.mesh.vertex_count(), left.cap_triangles, left.open_loops), (ball_vertices, 0, 0));

    // With every vertex forced onto one side, the tip survives as a speck.
    let identity = SymmetryOp::IDENTITY;
    let speck = trim_once(&mesh.positions, &mesh.indices, None, &view, &identity, &region, true, 0.0).unwrap().unwrap();
    assert_closed(&speck.mesh, "the ball and a speck");
    assert!(speck.mesh.vertex_count() > ball_vertices && speck.cap_triangles > 0);
}

#[test]
fn a_cut_along_a_face_of_the_model_changes_nothing() {
    // The box's edge runs exactly along the cube's right-hand face. Every
    // vertex of that face is on the outline and nothing is beyond it, so there
    // is nothing to cut away, and nothing with any thickness to crop down to.
    let mesh = cube(6);
    let view = front();
    let beyond = ScreenRegion::rect(px(1.0, 3.0), px(3.0, -3.0)).unwrap();
    let attempt = |remove_covered| {
        trim(&mesh.positions, &mesh.indices, None, &view, &SymmetryOp::IDENTITY, &beyond, remove_covered)
    };
    assert_eq!(attempt(true).unwrap_err(), "the shape covered nothing");
    assert_eq!(attempt(false).unwrap_err(), "that would delete the whole object");
}

#[test]
fn a_mirrored_cut_meets_its_first_cut_on_the_mirror_plane() {
    // The second, mirrored cut passes exactly through the points where the
    // first cut's rim crosses the mirror plane, and through the slivers the
    // first cut left there. It used to come out with four triangles on one
    // edge.
    for n in [30, 60, 120] {
        let sphere = cube_sphere(n);
        let view = perspective_view(Vec3::ZERO, 20.0, 25.0, 3.2, Vec2::ZERO, Vec2::new(1920.0, 1080.0));
        let region = ScreenRegion::rect(Vec2::new(-100.0, -100.0), Vec2::new(973.0, 1300.0)).unwrap();
        let mirror = [SymmetryOp::IDENTITY, SymmetryOp::from_sign(Vec3::new(-1.0, 1.0, 1.0))];
        let wedge = trim_passes(&sphere.positions, &sphere.indices, None, &[view], &mirror, &region, true).unwrap();
        assert_closed(&wedge.mesh, "wedge");
        assert_eq!((wedge.cuts, wedge.open_loops), (2, 0));
        // Mirror-symmetric, so the wedge's centre of mass is on the plane.
        let lean: f64 = wedge
            .mesh
            .indices
            .chunks_exact(3)
            .map(|t| {
                let [a, b, c] = [t[0], t[1], t[2]].map(|i| wedge.mesh.positions[i as usize].as_dvec3());
                a.dot(b.cross(c)) * (a.x + b.x + c.x)
            })
            .sum();
        assert!(lean.abs() / 24.0 < 2e-3 * wedge.mesh.signed_volume(), "leans {lean}");

        // The long way round, with no vertex allowed to be on the outline,
        // leaves a tangle of near-coincident points there. It must close too.
        let first = trim(&sphere.positions, &sphere.indices, None, &view, &mirror[0], &region, true).unwrap();
        for remove_covered in [true, false] {
            let second = trim_once(
                &first.mesh.positions,
                &first.mesh.indices,
                None,
                &view,
                &mirror[1],
                &region,
                remove_covered,
                0.0,
            );
            assert_closed(&second.unwrap().unwrap().mesh, "the second cut, made the long way");
        }
    }
}

/// The volume of the unit ball left after removing everything `removed` says
/// is gone, by counting cells of a grid.
fn ball_volume_left(removed: impl Fn(Vec3) -> bool + Sync) -> f64 {
    use rayon::prelude::*;
    let n = 160;
    let at = |i: usize| (i as f32 + 0.5) / n as f32 * 2.0 - 1.0;
    let left: usize = (0..n * n)
        .into_par_iter()
        .map(|row| {
            let (x, y) = (at(row % n), at(row / n));
            (0..n)
                .filter(|&k| Vec3::new(x, y, at(k)).length_squared() < 1.0 && !removed(Vec3::new(x, y, at(k))))
                .count()
        })
        .sum();
    left as f64 * 8.0 / (n * n * n) as f64
}

#[test]
fn tunnels_that_cross_inside_the_model_leave_the_right_solid() {
    // One round hole punched through a sphere and repeated three times round
    // the upright axis. The copies meet in the middle: each later tunnel
    // breaks into the ones before it, so its wall has gaps and its openings
    // run along the earlier walls. Off-centre, the meeting is lopsided too.
    let sphere = icosphere(5);
    let view = front();
    let turns: Vec<SymmetryOp> = (0..3).map(|k| SymmetryOp::rotation(Vec3::Y, k, 3)).collect();
    for (centre, radius) in [(Vec2::new(0.0, 0.0), 0.3), (Vec2::new(0.12, 0.05), 0.3), (Vec2::new(0.3, -0.1), 0.22)] {
        let outline: Vec<Vec2> = (0..64)
            .map(|i| {
                px(centre.x, centre.y) + Vec2::from_angle(i as f32 / 64.0 * std::f32::consts::TAU) * radius * SCALE
            })
            .collect();
        let region = ScreenRegion::lasso(&outline).unwrap();
        let drilled = trim_passes(&sphere.positions, &sphere.indices, None, &[view], &turns, &region, true).unwrap();
        assert_closed(&drilled.mesh, "drilled sphere");
        assert_eq!((drilled.cuts, drilled.open_loops), (3, 0));

        let covered = |p: Vec3| view.projection.project(p).is_some_and(|screen| region.contains(screen));
        let expected = ball_volume_left(|p| turns.iter().any(|turn| covered(turn.apply_inverse_point(p))));
        // The mesh is a little smaller than the ball it stands for.
        let expected = expected * sphere.signed_volume() / (4.0 / 3.0 * PI);
        assert_near(drilled.mesh.signed_volume(), expected, 0.01, "drilled volume");
    }
}

#[test]
fn a_cut_face_that_would_double_a_surface_edge_is_made_the_long_way() {
    // Found by cutting at random: the mirrored second cut leaves two vertices
    // on its outline that the surface already joins with an edge, and the cut
    // face, left to itself, joins them again.
    let sphere = cube_sphere(24);
    let view = perspective_view(Vec3::ZERO, 66.78383, -18.12439, 4.1541333, Vec2::ZERO, Vec2::new(1000.0, 1000.0));
    let region = ScreenRegion::rect(Vec2::new(502.0691, 345.85797), Vec2::new(827.275, 536.8148)).unwrap();
    let flip = SymmetryOp::from_sign(Vec3::new(1.0, -1.0, 1.0));
    let first = trim(&sphere.positions, &sphere.indices, None, &view, &SymmetryOp::IDENTITY, &region, true).unwrap();
    let attempt =
        |band| trim_once(&first.mesh.positions, &first.mesh.indices, None, &view, &flip, &region, false, band);
    assert!(attempt(ON_BOUNDARY_PIXELS).unwrap().is_none(), "the first attempt should have been refused");
    let second = trim(&first.mesh.positions, &first.mesh.indices, None, &view, &flip, &region, false).unwrap();
    assert_closed(&second.mesh, "the covered part of the second cut");
    let rest = trim(&first.mesh.positions, &first.mesh.indices, None, &view, &flip, &region, true).unwrap();
    assert_closed(&rest.mesh, "what the second cut leaves");
    assert_near(
        second.mesh.signed_volume() + rest.mesh.signed_volume(),
        first.mesh.signed_volume(),
        1e-3,
        "both parts",
    );
}

/// Timing at the size the app is built for:
/// `cargo test -p sculpt-cut --release -- --ignored --nocapture`.
#[test]
#[ignore]
fn timing_trim_of_a_twenty_million_triangle_ring() {
    let ring = torus(1.0, 0.4, 3200, 3200);
    let size = Vec2::new(1920.0, 1080.0);
    let view = perspective_view(Vec3::ZERO, 20.0, 40.0, 4.0, Vec2::ZERO, size);
    println!(
        "{} triangles, {} vertices, {} threads",
        ring.triangle_count(),
        ring.vertex_count(),
        rayon::current_num_threads()
    );
    let mask = vec![0.5f32; ring.vertex_count()];
    let shapes = [
        ("line", line(size * 0.5 + Vec2::new(31.0, 17.0), 63.0)),
        ("box", ScreenRegion::rect(Vec2::new(-100.0, -100.0), Vec2::new(size.x * 0.5 + 13.0, 700.0)).unwrap()),
    ];
    for (name, region) in &shapes {
        let start = Instant::now();
        let result =
            trim(&ring.positions, &ring.indices, Some(&mask), &view, &SymmetryOp::IDENTITY, region, true).unwrap();
        let elapsed = start.elapsed();
        println!(
            "trim, {name}: {:.0} ms; {} -> {} triangles, {} on the cut in {} faces, {} unclosed",
            elapsed.as_secs_f64() * 1e3,
            ring.triangle_count(),
            result.mesh.triangle_count(),
            result.cap_triangles,
            result.cut_faces.len(),
            result.open_loops
        );
        assert_closed(&result.mesh, name);
    }
}
