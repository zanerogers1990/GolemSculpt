//! How long an unwrap takes at the sizes the app unwraps: a Quad Remesh of a
//! sculpt is 2,000 to 200,000 faces. Run with
//! `cargo test -p sculpt-uv --release -- --ignored --nocapture timing`.

use std::time::Instant;

use super::checks::measure;
use super::meshes::{Model, icosphere, torus};
use crate::{Options, unwrap};

/// Unwraps `model` once, prints the time and the quality, and returns the
/// time in seconds.
fn time(name: &str, model: &Model) -> f64 {
    let start = Instant::now();
    let out = unwrap(&model.positions, &model.triangles, model.polys(), None, &Options::default())
        .unwrap_or_else(|e| panic!("{name}: {e}"));
    let seconds = start.elapsed().as_secs_f64();
    let q = measure(model, &out);
    println!(
        "{name}: {} triangles in {seconds:.2} s on {} threads",
        model.triangle_count(),
        rayon::current_num_threads()
    );
    q.print("  ");
    q.assert_valid(name);
    seconds
}

#[test]
#[ignore]
fn timing_two_hundred_thousand_triangles() {
    // Quads with bumps on them, like a remeshed sculpt: 100,000 quads.
    let bumpy = torus(400, 250, (0.12, 23.0, 11.0), true);
    let quads = time("bumpy quad torus", &bumpy);
    // The same as plain triangles.
    let plain = Model { polys: None, ..torus(400, 250, (0.12, 23.0, 11.0), false) };
    let triangles = time("bumpy triangle torus", &plain);
    // A smooth sphere, which makes big curved charts to lay flat.
    let sphere = time("sphere", &icosphere(7));
    assert!(quads < 5.0 && triangles < 5.0 && sphere < 8.0, "too slow");
}
