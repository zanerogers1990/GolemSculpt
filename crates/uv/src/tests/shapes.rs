//! Each test model unwrapped and held to the checks. The numbers are printed
//! for `--nocapture`, so a change to the unwrapper can be judged by them.

use super::checks::{Quality, measure};
use super::meshes::{Model, cube, holed_grid, icosphere, loose_squares, torus};
use crate::{Options, Unwrapped, unwrap};

/// Unwraps `model` with `groups`, checks it, prints its numbers.
pub fn unwrap_and_check(name: &str, model: &Model, groups: Option<&[u16]>) -> (Unwrapped, Quality) {
    let out = unwrap(&model.positions, &model.triangles, model.polys(), groups, &Options::default())
        .unwrap_or_else(|e| panic!("{name}: {e}"));
    let quality = measure(model, &out);
    quality.print(&format!("{name} ({} triangles)", model.triangle_count()));
    quality.assert_valid(name);
    (out, quality)
}

#[test]
fn a_sphere_unwraps_into_a_few_charts_with_little_stretch() {
    let (_, q) = unwrap_and_check("sphere", &icosphere(5), None);
    assert!(q.charts >= 2 && q.charts <= 40, "{} charts", q.charts);
    assert!(q.stretch < 1.15, "stretch {}", q.stretch);
    assert!(q.density_spread < 1.5, "density spread {}", q.density_spread);
}

#[test]
fn a_cube_unwraps_into_its_six_sides_with_no_stretch() {
    let (_, q) = unwrap_and_check("cube", &cube(8, false), None);
    assert_eq!(q.charts, 6);
    assert!(q.stretch < 1.0005, "stretch {}", q.stretch);
    assert!(q.density_spread < 1.01, "density spread {}", q.density_spread);
}

#[test]
fn a_torus_unwraps_with_little_stretch() {
    let (_, q) = unwrap_and_check("torus", &torus(96, 48, (0.0, 0.0, 0.0), false), None);
    assert!(q.charts >= 2 && q.charts <= 60, "{} charts", q.charts);
    assert!(q.stretch < 1.15, "stretch {}", q.stretch);
}

#[test]
fn a_flat_grid_with_a_hole_stays_one_chart_laid_flat_as_it_is() {
    let (_, q) = unwrap_and_check("holed grid", &holed_grid(30), None);
    assert_eq!(q.charts, 1);
    assert!(q.stretch < 1.0005, "stretch {}", q.stretch);
}

#[test]
fn a_bumpy_quad_torus_like_a_remeshed_sculpt_unwraps_validly() {
    let model = torus(120, 60, (0.15, 7.0, 5.0), true);
    let (_, q) = unwrap_and_check("bumpy quad torus", &model, None);
    assert!(q.stretch < 1.2, "stretch {}", q.stretch);
}

#[test]
fn charts_fill_a_fair_share_of_the_square() {
    // The packer nests charts into each other's hollows; a sphere's charts
    // should cover well over half the square.
    let (_, q) = unwrap_and_check("sphere coverage", &icosphere(4), None);
    assert!(q.coverage > 0.5, "{:.1}% used", q.coverage * 100.0);
}

#[test]
fn thousands_of_charts_are_packed_in_rows_and_still_valid() {
    // Over the limit for shape packing, so the second, row-packing run is
    // the one handed back.
    let model = loose_squares(2500);
    let started = std::time::Instant::now();
    let (out, q) = unwrap_and_check("2500 loose squares", &model, None);
    println!("  in {:.2} s", started.elapsed().as_secs_f64());
    assert_eq!(q.charts, 2500);

    // The same as the unwrapper packing in rows from the start.
    let points: Vec<[f32; 3]> = model.positions.iter().map(|p| p.to_array()).collect();
    let (page, padding) = crate::page_and_padding(Options::default().padding);
    let in_rows = uvunwrap::Options {
        max_chart_angle: Options::default().max_angle_degrees,
        padding,
        density: uvunwrap::Density::Auto { page_size: page },
        packing: uvunwrap::Packing::Fast,
        ..uvunwrap::Options::default()
    };
    let atlas = uvunwrap::generate(&[uvunwrap::MeshInput::new(&points, &model.triangles)], &in_rows).unwrap();
    assert_eq!(crate::corner_uvs(&atlas), out.uvs);
}

#[test]
fn the_same_model_unwraps_the_same_to_the_bit() {
    let model = torus(64, 32, (0.1, 5.0, 3.0), true);
    let run = || {
        let out = unwrap(&model.positions, &model.triangles, model.polys(), None, &Options::default()).unwrap();
        (out.uvs.iter().map(|p| p.to_array().map(f32::to_bits)).collect::<Vec<_>>(), out.charts, out.stretch.to_bits())
    };
    let first = run();
    for _ in 0..3 {
        assert_eq!(run(), first);
    }
    // And on one thread, as on many.
    let single = rayon_single(run);
    assert_eq!(single, first);
}

/// Runs `work` inside a pool of one thread.
fn rayon_single<T: Send>(work: impl FnOnce() -> T + Send) -> T {
    rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap().install(work)
}

#[test]
fn padding_keeps_charts_apart_by_at_least_the_share_asked_for() {
    // Wide padding, so a gap shows plainly: no two charts' UVs may come
    // closer than the padding, measured point to triangle.
    let model = cube(2, false);
    let options = Options { padding: 0.05, ..Options::default() };
    let out = unwrap(&model.positions, &model.triangles, None, None, &options).unwrap();
    let q = measure(&model, &out);
    q.assert_valid("padded cube");
    let tris: Vec<(u32, [glam::Vec2; 3])> = (0..model.triangle_count())
        .map(|t| (q.island_of[t], [out.uvs[t * 3], out.uvs[t * 3 + 1], out.uvs[t * 3 + 2]]))
        .collect();
    let mut nearest = f32::INFINITY;
    for (ia, a) in &tris {
        for (ib, b) in &tris {
            if ia != ib {
                for &p in a {
                    nearest = nearest.min(point_triangle_distance(p, b));
                }
            }
        }
    }
    println!("padded cube: charts {} apart at the nearest", nearest);
    assert!(nearest >= options.padding * 0.999, "charts only {nearest} apart");
    let edge = out.uvs.iter().map(|p| p.x.min(p.y).min(1.0 - p.x).min(1.0 - p.y)).fold(1.0, f32::min);
    assert!(edge >= options.padding / 2.0 * 0.999, "a chart {edge} from the edge");
}

fn point_triangle_distance(p: glam::Vec2, t: &[glam::Vec2; 3]) -> f32 {
    let inside = (0..3).all(|k| (t[(k + 1) % 3] - t[k]).perp_dot(p - t[k]) >= 0.0);
    if inside {
        return 0.0;
    }
    (0..3)
        .map(|k| {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            let s = ((p - a).dot(b - a) / (b - a).length_squared()).clamp(0.0, 1.0);
            (a + (b - a) * s).distance(p)
        })
        .fold(f32::INFINITY, f32::min)
}
