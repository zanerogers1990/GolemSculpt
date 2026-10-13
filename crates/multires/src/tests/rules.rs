//! Where the rules put things: known answers for both schemes, flat
//! splitting, and the same result whatever the thread pool.

use glam::Vec3;

use super::fixtures::*;
use crate::{LevelMesh, Split, subdivide};

/// The vertices at which two meshes of the same numbering differ by more than rounding.
fn differing(a: &LevelMesh, b: &LevelMesh) -> Vec<usize> {
    (0..a.positions.len()).filter(|&v| a.positions[v].distance(b.positions[v]) > 1e-6).collect()
}

/// Loop's surface has a known limit: a vertex with `n` neighbours ends at
/// `(1 - n x) P + x * (sum of neighbours)`, `x = 1 / (3 / (8 b) + n)`. On an
/// icosahedron every vertex has five neighbours at the same angle, so its
/// vertices converge on a sphere of a radius that can be written down.
#[test]
fn loop_on_an_icosahedron_converges_on_its_limit_surface() {
    let n = 5.0f64;
    let beta = (0.625 - (0.375 + 0.25 * (std::f64::consts::TAU / n).cos()).powi(2)) / n;
    let chi = 1.0 / (3.0 / (8.0 * beta) + n);
    // Neighbouring corners of a unit icosahedron are 1/sqrt(5) apart in cosine.
    let limit = 1.0 - n * chi * (1.0 - 1.0 / 5.0f64.sqrt());

    let mut mesh = icosahedron();
    let mut error = (1.0 - limit).abs();
    for level in 1..=6 {
        mesh = subdivide(&mesh, Split::Smooth).unwrap().mesh;
        let now = (0..12).map(|v| (mesh.positions[v].length() as f64 - limit).abs()).fold(0.0, f64::max);
        assert!(now < error * 0.3 || now < 2e-6, "level {level}: {now} after {error}");
        error = now;
    }
    // Single precision positions, rounded at each of six levels: this is as near as they get.
    assert!(error < 5e-5, "{error}");
    // And the whole surface is close to a ball of about that size.
    let radii = mesh.positions.iter().map(|p| p.length() as f64);
    let (low, high) = radii.fold((f64::MAX, 0.0f64), |(l, h), r| (l.min(r), h.max(r)));
    assert!(low > limit * 0.97 && high < limit * 1.03, "{low} to {high}, limit {limit}");
}

#[test]
fn an_even_flat_sheet_is_left_where_it_is_but_for_its_corners() {
    // In an even sheet every rule lands a vertex where flat splitting would:
    // only the four corners of the outline, which round off, may differ.
    let corners = |nx: usize, ny: usize| vec![0, nx, ny * (nx + 1), ny * (nx + 1) + nx];
    for mesh in [triangle_grid(5, 4), quad_grid(5, 4)] {
        let (smooth, flat) = (subdivide(&mesh, Split::Smooth).unwrap(), subdivide(&mesh, Split::Flat).unwrap());
        assert_eq!(smooth.mesh.triangles, flat.mesh.triangles, "the split decides positions, never faces");
        assert_eq!(differing(&smooth.mesh, &flat.mesh), corners(5, 4));
        assert_eq!(smooth.mesh.positions[0], Vec3::new(0.125, 0.125, 0.0));
    }
}

#[test]
fn catmull_clark_puts_a_cube_where_the_book_says() {
    let fine = subdivide(&cube(), Split::Smooth).unwrap().mesh;
    for (v, p) in fine.positions.iter().enumerate() {
        // A corner goes to 5/9 of the way out, the middle of an edge to 3/4, the middle of a face stays.
        let (expected, what) = match v {
            0..8 => (5.0 / 9.0 * 3.0f32.sqrt(), "corner"),
            8..20 => (0.75 * 2.0f32.sqrt(), "edge"),
            _ => (1.0, "face"),
        };
        assert!((p.length() - expected).abs() < 1e-6, "{what} {v} at {p}");
    }
    assert!(fine.positions[7].distance(Vec3::splat(5.0 / 9.0)) < 1e-7);
    assert_eq!(fine.positions[20].abs().max_element(), 1.0);
}

#[test]
fn catmull_clark_rounds_a_cube_into_a_ball() {
    let mut mesh = cube();
    let mut volume = signed_volume(&mesh);
    for _ in 0..5 {
        mesh = subdivide(&mesh, Split::Smooth).unwrap().mesh;
        let now = signed_volume(&mesh);
        assert!(now < volume && now > 2.6, "{now} after {volume}");
        volume = now;
    }
    let radii = mesh.positions.iter().map(|p| p.length());
    let (low, high) = radii.fold((f32::MAX, 0.0f32), |(l, h), r| (l.min(r), h.max(r)));
    // The limit is not a sphere, but it is within a few percent of one.
    assert!(high / low < 1.05 && low > 0.8 && high < 0.9, "{low} to {high}");
}

#[test]
fn a_flat_split_moves_nothing() {
    let mut rng = Rng(3);
    for mut mesh in [ico_sphere(1), quad_torus(7, 5), cube()] {
        roughen(&mut mesh, 0.1, &mut rng);
        let volume = signed_volume(&mesh);
        let fine = subdivide(&mesh, Split::Flat).unwrap().mesh;
        assert!(same_bits(&fine.positions[..mesh.positions.len()], &mesh.positions));
        assert_eq!(fine.mask[..mesh.mask.len()], mesh.mask);
        if mesh.polys.is_none() {
            // Triangles cut in their own planes enclose what they did. (A bent
            // quad has no plane, so its split shifts the volume a little.)
            assert!((signed_volume(&fine) - volume).abs() < 1e-5);
        }
    }
}

#[test]
fn the_mask_is_carried_up_without_smoothing_and_stays_in_range() {
    let mut rng = Rng(17);
    for mut mesh in [ico_sphere(1), quad_torus(7, 5)] {
        roughen(&mut mesh, 0.0, &mut rng);
        // Hard edges: fully masked or not at all.
        mesh.mask.iter_mut().for_each(|m| *m = m.round());
        let fine = subdivide(&mesh, Split::Smooth).unwrap().mesh;
        assert_eq!(fine.mask[..mesh.mask.len()], mesh.mask, "kept vertices keep their mask");
        assert!(fine.mask.iter().all(|m| (0.0..=1.0).contains(m)));
        // A new vertex between masked vertices is masked, between clear ones clear.
        let flat = subdivide(&LevelMesh { mask: vec![1.0; mesh.mask.len()], ..mesh.clone() }, Split::Smooth).unwrap();
        assert!(flat.mesh.mask.iter().all(|&m| m == 1.0));
        assert!(fine.mask[mesh.mask.len()..].iter().any(|&m| m > 0.0 && m < 1.0), "and in between, between");
    }
}

/// Everything is a gather with a fixed order of summing, so one thread and
/// many must agree to the bit.
#[test]
fn the_result_does_not_depend_on_the_thread_pool() {
    let run = |threads: usize, mesh: &LevelMesh| {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
        pool.install(|| {
            let (mut levels, mut top) = raised(mesh, &[Split::Smooth, Split::Smooth, Split::Flat]);
            roughen(&mut top, 0.01, &mut Rng(9));
            let mut coarse = levels.step_to(0, &top.positions, &top.mask).unwrap().mesh;
            coarse.positions.iter_mut().for_each(|p| p.y += p.x * 0.3);
            coarse.mask[3] = 1.0;
            levels.step_to(3, &coarse.positions, &coarse.mask).unwrap().mesh
        })
    };
    for mesh in [ico_sphere(3), quad_torus(40, 24)] {
        let (one, many) = (run(1, &mesh), run(7, &mesh));
        assert!(same_bits(&one.positions, &many.positions));
        assert_eq!(
            one.mask.iter().map(|m| m.to_bits()).collect::<Vec<_>>(),
            many.mask.iter().map(|m| m.to_bits()).collect::<Vec<_>>()
        );
        assert_eq!((one.triangles, one.polys), (many.triangles, many.polys));
    }
}
