//! Timings of the whole-mesh operations. Not run by default, and one at a
//! time when they are, so they do not time each other:
//!
//! ```text
//! cargo test -p sculpt-volume --target-dir target/w/volume --release -- --ignored --nocapture --test-threads=1
//! ```

use std::time::Instant;

use glam::Vec3;
use sculpt_geom::TriMesh;

use crate::boolean::{BooleanOp, BooleanSettings, boolean_with};
use crate::bvh::TriangleBvh;
use crate::grid::Grid;
use crate::remesh::{RemeshSettings, remesh_with};
use crate::scan::Scan;
use crate::testing::{Rng, defects, icosphere, shells, sphere_volume, uv_sphere, Defects};

/// Best of three, in milliseconds, with the last result.
fn best_of_three<T>(mut work: impl FnMut() -> T) -> (f64, T) {
    let mut best = f64::INFINITY;
    let mut last = None;
    for _ in 0..3 {
        let start = Instant::now();
        last = Some(work());
        best = best.min(start.elapsed().as_secs_f64() * 1000.0);
    }
    (best, last.unwrap())
}

fn describe(mesh: &TriMesh) -> String {
    format!("{} triangles, {} vertices", mesh.triangle_count(), mesh.vertex_count())
}

#[test]
#[ignore]
fn remesh_a_million_triangle_sphere() {
    let source = icosphere(8, 1.0, Vec3::ZERO);
    println!("source: icosphere, {}", describe(&source));
    for resolution in [256, 512, 1024] {
        for mirror in [[false; 3], [true, false, false]] {
            let settings = RemeshSettings { mirror, ..RemeshSettings::new(resolution) };
            let (ms, out) = best_of_three(|| remesh_with(&source.positions, &source.indices, &settings).unwrap());
            println!("remesh at {resolution}, mirror {:?}: {ms:.1} ms -> {}", mirror[0], describe(&out.mesh));
            // Checked at one size only: the check takes longer than the remesh.
            if resolution == 512 && mirror[0] {
                assert_eq!(defects(&out.mesh), Defects::default());
                assert_eq!(shells(&out.mesh), 1);
                assert!((out.mesh.signed_volume() / sphere_volume(1.0) - 1.0).abs() < 1e-3);
                assert_eq!((out.stats.mirror_skipped, out.stats.skipped_quads), (0, 0));
            }
        }
    }
    let grid = Grid::covering(&source.bounds(), 2.0 / 512.0).unwrap();
    let (ms, scan) = best_of_three(|| Scan::new(&source.positions, &source.indices, &grid).unwrap());
    println!("  of which scan conversion at 512: {ms:.1} ms, {} crossings", scan.crossing_count());
}

/// Twelve triangles, each spanning hundreds of thousands of lattice lines.
#[test]
#[ignore]
fn remesh_a_twelve_triangle_box() {
    let half = Vec3::new(1.0, 0.7, 0.5);
    let source = crate::testing::rotated(&crate::testing::box_mesh(-half, half), Vec3::new(31.0, 17.0, 9.0));
    for resolution in [256, 1024] {
        let settings = RemeshSettings::new(resolution);
        let (ms, out) = best_of_three(|| remesh_with(&source.positions, &source.indices, &settings).unwrap());
        println!("box at {resolution}: {ms:.1} ms -> {}", describe(&out.mesh));
        assert!((out.mesh.signed_volume() / 2.8 - 1.0).abs() < 1e-3);
    }
}

#[test]
#[ignore]
fn remesh_its_own_output_at_ten_million_triangles() {
    let source = icosphere(8, 1.0, Vec3::ZERO);
    let first = remesh_with(&source.positions, &source.indices, &RemeshSettings::new(1024)).unwrap().mesh;
    println!("source: {}", describe(&first));
    for resolution in [512, 1024] {
        let settings = RemeshSettings::new(resolution);
        let (ms, out) = best_of_three(|| remesh_with(&first.positions, &first.indices, &settings).unwrap());
        println!("remesh at {resolution}: {ms:.1} ms -> {}", describe(&out.mesh));
    }
}

#[test]
#[ignore]
fn boolean_of_two_half_million_triangle_spheres() {
    let a = uv_sphere(500, 500, 1.0, Vec3::new(-0.5, 0.01, 0.02));
    let b = uv_sphere(500, 500, 1.0, Vec3::new(0.5, 0.01, 0.02));
    println!("operands: uv spheres, {} each", describe(&a));
    for resolution in [256, 512] {
        for op in [BooleanOp::Union, BooleanOp::Subtract, BooleanOp::Intersect] {
            let settings = BooleanSettings::new(resolution);
            let (ms, out) = best_of_three(|| boolean_with(&a, &[&b], op, &settings).unwrap());
            println!("{op:?} at {resolution}: {ms:.1} ms -> {}", describe(&out.mesh));
            assert_eq!(defects(&out.mesh), Defects::default());
        }
    }
}

#[test]
#[ignore]
fn a_million_closest_point_queries() {
    let mesh = icosphere(8, 1.0, Vec3::ZERO);
    let (build_ms, bvh) = best_of_three(|| TriangleBvh::new(&mesh));
    println!("tree over {}: {build_ms:.1} ms", describe(&mesh));

    // The triangles' edges are about 0.0043 long.
    let mut rng = Rng::new(5);
    let mut shell = |thickness: f32| -> Vec<Vec3> {
        (0..1_000_000).map(|_| (rng.vec3() - 0.5).normalize() * (1.0 + (rng.next() - 0.5) * thickness)).collect()
    };
    let (on, near) = (shell(0.002), shell(0.04));
    let anywhere: Vec<Vec3> = (0..1_000_000).map(|_| (rng.vec3() - 0.5) * 3.0).collect();
    let cases = [
        (&on, "within a quarter of an edge of the surface"),
        (&near, "within five edges of the surface"),
        (&anywhere, "anywhere in a box round a sphere (its centre is the worst case there is)"),
    ];
    for (points, what) in cases {
        let (ms, found) = best_of_three(|| bvh.closest_many(points));
        assert!(found.iter().all(|c| c.is_some()));
        let start = Instant::now();
        let total: f32 = points[..100_000].iter().map(|&p| bvh.distance(p)).sum();
        let each = start.elapsed().as_secs_f64() * 1e9 / 100_000.0;
        println!("1M closest points {what}: {ms:.1} ms on all cores; {each:.0} ns each on one (checksum {total:.3})");
    }
    // What projecting one mesh onto another looks like: points on the
    // surface, neighbours in the list near each other in space.
    let rebuilt = remesh_with(&mesh.positions, &mesh.indices, &RemeshSettings::new(512)).unwrap().mesh;
    let points = &rebuilt.positions[..1_000_000];
    let (ms, _) = best_of_three(|| bvh.closest_many(points));
    let start = Instant::now();
    let total: f32 = points[..100_000].iter().map(|&p| bvh.distance(p)).sum();
    let each = start.elapsed().as_secs_f64() * 1e9 / 100_000.0;
    println!(
        "1M closest points that are the vertices of a remesh, in order: {ms:.1} ms on all cores; \
         {each:.0} ns each on one (checksum {total:.3})"
    );

    let (ms, inside) = best_of_three(|| {
        use rayon::prelude::*;
        anywhere.par_iter().filter(|&&p| bvh.contains(p)).count()
    });
    println!("1M inside/outside tests: {ms:.1} ms on all cores, {inside} inside");
}

#[test]
#[ignore]
fn a_sampled_field_to_a_mesh_and_a_mesh_to_a_sampled_field() {
    use rayon::prelude::*;
    use sculpt_geom::Aabb;
    for resolution in [128usize, 256, 400] {
        let grid = Grid::covering(&Aabb::new(Vec3::splat(-1.0), Vec3::splat(1.0)), 2.0 / resolution as f32).unwrap();
        let [nx, ny, _] = grid.n;
        let mut values = vec![0.0f32; grid.sample_count()];
        values.par_chunks_mut(nx).enumerate().for_each(|(row, out)| {
            for (i, v) in out.iter_mut().enumerate() {
                *v = grid.position(i, row % ny, row / ny).length() - 0.9;
            }
        });
        let (ms, surface) = best_of_three(|| crate::sdf::surface_from_sdf(&values, &grid, [false; 3]).unwrap());
        let samples = grid.sample_count() as f64 / 1e6;
        println!("field of {samples:.1}M samples to a mesh: {ms:.1} ms -> {} triangles", surface.indices.len() / 3);
        let (ms, _) = best_of_three(|| crate::sdf::surface_from_sdf(&values, &grid, [true, false, false]).unwrap());
        println!("  the same, exactly mirrored: {ms:.1} ms");

        let mesh = icosphere(7, 0.9, Vec3::ZERO);
        let (ms, sampled) = best_of_three(|| crate::sdf::sample_mesh(&mesh.positions, &mesh.indices, &grid).unwrap());
        let band = sampled.par_iter().filter(|v| v.abs() < crate::sdf::FAR).count();
        println!("  {} into {samples:.1}M samples: {ms:.1} ms, {band} real distances", describe(&mesh));
    }
}

/// Meshes with holes, three ways: as they were first handled (no caps, every
/// sample asked along three axes one at a time), with the same vote worked a
/// plane at a time, and as now (rims capped first, the vote only for what is
/// left). The first two give the same surface; the last a better one.
#[test]
#[ignore]
fn remesh_million_triangle_meshes_with_holes() {
    use crate::extract::{ExtractOptions, extract};
    use crate::scan::BySample;
    use crate::testing::{bowl, open_tube, rotated, without_near};

    let sphere = icosphere(8, 1.0, Vec3::ZERO);
    let pocked = [Vec3::X, Vec3::new(0.3, 0.5, 0.8), Vec3::new(-0.7, 0.1, -0.7)]
        .iter()
        .fold(sphere, |mesh, way| without_near(&mesh, way.normalize(), 0.03));
    let tilt = Vec3::new(35.0, 20.0, 50.0);
    let shapes = [
        (pocked, "sphere with three small holes"),
        (rotated(&bowl(1000, 1000, 1.0), tilt), "bowl, tilted"),
        (rotated(&open_tube(1000, 500, 0.4, 2.0), tilt), "pipe open at both ends, tilted"),
    ];
    for (source, name) in &shapes {
        println!("{name}: {}", describe(source));
        let bounds = source.bounds();
        for resolution in [512, 1024] {
            let grid = Grid::covering(&bounds, bounds.size().max_element() / resolution as f32).unwrap();
            let uncapped = |by_sample: bool| {
                let scan = Scan::build(&source.positions, &source.indices, &grid, false).unwrap();
                let options = ExtractOptions::default();
                let surface = match by_sample {
                    true => extract(&BySample(&scan), &grid, &options),
                    false => extract(&scan, &grid, &options),
                };
                surface.unwrap().indices.len() / 3
            };
            let (by_sample, old) = best_of_three(|| uncapped(true));
            let (by_plane, same) = best_of_three(|| uncapped(false));
            assert_eq!(old, same, "the two votes are one vote");
            let settings = RemeshSettings::new(resolution);
            let (capped, out) = best_of_three(|| remesh_with(&source.positions, &source.indices, &settings).unwrap());
            println!(
                "  at {resolution}: {by_sample:.0} ms a sample at a time, {by_plane:.0} ms a plane at a time \
                 ({old} triangles); {capped:.0} ms capped ({} triangles, {} shell)",
                out.mesh.triangle_count(),
                shells(&out.mesh)
            );
        }
    }
}
