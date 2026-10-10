//! Timings of the whole-tube and whole-mesh operations. Not run by default:
//!
//! ```text
//! cargo test -p sculpt-tube --target-dir target/w/tube --release -- --ignored --nocapture --test-threads=1
//! ```

use std::time::Instant;

use glam::{Mat4, Vec3};
use sculpt_geom::TriMesh;

use crate::field::{MAX_VOXEL_SAMPLES, TubeField};
use crate::live::PREVIEW_MAX_SAMPLES;
use crate::merge::{self, Stage};
use crate::testing::{assert_closed, field_of, icosphere, shells};
use crate::types::{TubePoint, TubeSettings};

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

/// An arm: six points, twenty radii long, bent, a little tapered.
fn arm(radius: f32) -> (Vec<TubePoint>, TubeSettings) {
    let path = [[0.9, 0.2, 0.0], [1.3, 0.35, 0.1], [1.7, 0.3, 0.3], [2.0, 0.0, 0.4], [2.2, -0.4, 0.3], [2.3, -0.8, 0.1]];
    let points = path.iter().map(|&p| TubePoint::free(Vec3::from(p))).collect();
    (points, TubeSettings { radius, taper: 0.3, ..TubeSettings::default() })
}

fn describe(mesh: &TriMesh) -> String {
    format!("{} triangles", mesh.triangle_count())
}

#[test]
#[ignore]
fn building_a_typical_tube_at_preview_and_at_final_quality() {
    let radius = 0.08;
    let (points, settings) = arm(radius);
    for mirror in [[false; 3], [true, false, false]] {
        println!("an arm of {} points, radius {radius}, mirror X {}:", points.len(), mirror[0]);
        let (ms, field) = best_of_three(|| field_of(&points, &settings, mirror));
        println!("  curve and stations: {ms:.2} ms, {} round cones", field.cone_count());

        let floor = field.min_radius().max(radius * merge::RESOLVED_TIP_FRACTION);
        let preview_cell = (radius / 5.0).min(floor / merge::CELLS_ACROSS_RADIUS);
        for (name, cell, budget) in [
            ("preview", preview_cell, PREVIEW_MAX_SAMPLES),
            ("Keep Separate, no anchor (r / 6)", merge::separate_cell(radius, None, &field), MAX_VOXEL_SAMPLES),
            ("Keep Separate, a dense anchor (r / 12)", merge::separate_cell(radius, Some(0.0), &field), MAX_VOXEL_SAMPLES),
        ] {
            let (ms, built) = best_of_three(|| field.voxelize(cell, mirror, budget).unwrap());
            let (raster_ms, _) = best_of_three(|| field.rasterize(&built.grid));
            let samples = built.grid.sample_count();
            let mesh = built.surface.into_mesh();
            println!(
                "  {name}: {ms:.2} ms (sampling {raster_ms:.2}) for a cell of {:.4}, {samples} samples -> {}",
                built.grid.cell,
                describe(&mesh)
            );
            assert_closed(&mesh, name);
        }
    }

    // The worst preview there is: a long thin tube that fills its budget.
    let long: Vec<TubePoint> = (0..40)
        .map(|i| {
            let a = i as f32 * 0.5;
            TubePoint::free(Vec3::new(a.cos() * (1.0 + a * 0.2), a * 0.15, a.sin() * (1.0 + a * 0.2)))
        })
        .collect();
    let settings = TubeSettings { radius: 0.02, ..TubeSettings::default() };
    let (ms, field) = best_of_three(|| field_of(&long, &settings, [false; 3]));
    println!("a coil of {} points, radius 0.02: curve and stations {ms:.2} ms, {} round cones", long.len(), field.cone_count());
    for (name, budget) in [("preview", PREVIEW_MAX_SAMPLES), ("kept", MAX_VOXEL_SAMPLES)] {
        let (ms, built) = best_of_three(|| field.voxelize(0.004, [false; 3], budget).unwrap());
        let samples = built.grid.sample_count();
        println!("  {name}: {ms:.1} ms for a cell of {:.4}, {samples} samples -> {} triangles", built.grid.cell, built.surface.indices.len() / 3);
    }
}

#[test]
#[ignore]
fn fusing_a_tube_into_a_million_triangle_model() {
    let sphere = icosphere(8, 1.0, Vec3::ZERO);
    println!("model: icosphere, {}", describe(&sphere));
    let (points, settings) = arm(0.08);
    for (mirror, blend) in [([false; 3], 0.0), ([false; 3], 0.08), ([true, false, false], 0.08)] {
        let field: TubeField = field_of(&points, &settings, mirror);
        let (ms, plan) = best_of_three(|| merge::plan(&sphere.positions, &sphere.indices, &Mat4::IDENTITY, &field, 0.08, 0.08));
        println!("mirror X {}, fillet {blend}: plan {ms:.2} ms -> {plan:?}", mirror[0]);
        let stages = std::cell::RefCell::new(Vec::new());
        let start = Instant::now();
        let note = |stage: Stage| {
            stages.borrow_mut().push((stage, start.elapsed().as_secs_f64() * 1000.0));
            true
        };
        let merged = merge::merge(&sphere, &Mat4::IDENTITY, &field, &plan, blend, mirror, &note).unwrap();
        let total = start.elapsed().as_secs_f64() * 1000.0;
        let stages = stages.into_inner();
        for (i, (stage, at)) in stages.iter().enumerate() {
            let end = stages.get(i + 1).map_or(total, |next| next.1);
            println!("  {stage:?}: {:.1} ms", end - at);
        }
        println!("  in all {total:.1} ms -> {}", describe(&merged));
        assert_closed(&merged, "the model with an arm");
        assert_eq!(shells(&merged), 1);
    }
}
