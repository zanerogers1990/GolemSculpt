//! How long the whole-mesh work takes and what the stack holds, on meshes of
//! the size the app sculpts. Run with
//! `cargo test -p sculpt-multires --release -- --ignored --nocapture timing`.

use std::time::Instant;

use glam::{Quat, Vec3};

use super::fixtures::*;
use crate::{LevelMesh, Levels, Split};

/// The shortest of three runs: other work on the machine only ever adds time.
fn best<T>(mut work: impl FnMut() -> T) -> (f64, T) {
    let mut run = || {
        let start = Instant::now();
        let out = work();
        (start.elapsed().as_secs_f64() * 1000.0, out)
    };
    let mut least = run();
    for _ in 0..2 {
        let next = run();
        if next.0 < least.0 {
            least = next;
        }
    }
    least
}

fn megabytes(bytes: usize) -> f64 {
    bytes as f64 / (1 << 20) as f64
}

/// Times a stack over `base` taken to `levels` levels above it: the last
/// subdivision, steps down and up with and without an edit, and the memory.
fn report(name: &str, base: &LevelMesh, levels: usize) {
    println!("{name}: {} threads", rayon::current_num_threads());
    let (below, mut top) = raised(base, &vec![Split::Smooth; levels - 1]);
    roughen(&mut top, 0.0005, &mut Rng(1));
    let (ms, (mut stack, mut fine)) = best(|| {
        let mut stack = below.clone();
        let fine = stack.subdivide(&top.positions, &top.mask, Split::Smooth).unwrap();
        (stack, fine)
    });
    let last = stack.top();
    println!(
        "  subdivide {} -> {} triangles ({} -> {} vertices): {ms:.0} ms (includes copying the stack below, {:.0} MB)",
        stack.triangle_count(last - 1),
        stack.triangle_count(last),
        stack.vertex_count(last - 1),
        stack.vertex_count(last),
        megabytes(below.memory_bytes())
    );
    roughen(&mut fine, 0.0002, &mut Rng(2));

    let (ms, _) = best(|| {
        let coarse = stack.step_to(0, &fine.positions, &fine.mask).unwrap().mesh;
        stack.step_to(last, &coarse.positions, &coarse.mask).unwrap().mesh.positions.len()
    });
    println!("  down to level 0 and back up, nothing edited: {ms:.0} ms");

    // Bend one half of the model over at a lower level, so that every level's
    // base moves on that side, step back up, and take the step back.
    let turn = Quat::from_axis_angle(Vec3::new(0.2, 1.0, 0.1).normalize(), 0.4);
    for level in [0, last - 1] {
        let mut least = [f64::MAX; 3];
        let mut undo_bytes = 0;
        for _ in 0..3 {
            let clock = Instant::now();
            let mut coarse = stack.step_to(level, &fine.positions, &fine.mask).unwrap().mesh;
            let down = clock.elapsed().as_secs_f64() * 1000.0;
            let unedited = coarse.clone();
            coarse.positions.iter_mut().filter(|p| p.x > 0.0).for_each(|p| *p = turn * *p);
            let clock = Instant::now();
            let stepped = stack.step_to(last, &coarse.positions, &coarse.mask).unwrap();
            let up = clock.elapsed().as_secs_f64() * 1000.0;
            let clock = Instant::now();
            stack.undo_step(&stepped.undo, &stepped.mesh.positions, &stepped.mesh.mask).unwrap();
            let undo = clock.elapsed().as_secs_f64() * 1000.0;
            undo_bytes = stepped.undo.bytes();
            // Back to the finest level as it was, for the next run.
            stack.step_to(last, &unedited.positions, &unedited.mask).unwrap();
            for (least, ms) in least.iter_mut().zip([down, up, undo]) {
                *least = least.min(ms);
            }
        }
        println!(
            "  down to level {level} {:.0} ms; half of it turned and back up {:.0} ms; that step undone {:.0} ms \
             (its record {:.0} MB)",
            least[0],
            least[1],
            least[2],
            megabytes(undo_bytes)
        );
    }

    let (ms, parts) = best(|| stack.to_parts());
    let (ms_back, _) = best(|| Levels::from_parts(parts.clone()).unwrap().level_count());
    println!("  taken apart {ms:.0} ms, put back together {ms_back:.0} ms");

    stack.step_to(0, &fine.positions, &fine.mask).unwrap();
    let top_vertices = stack.vertex_count(last);
    for level in 0..=last {
        let bytes = stack.level_bytes(level);
        println!(
            "  level {level}: {:>9} vertices, {:>6.1} MB held ({:.0} bytes a vertex of that level)",
            stack.vertex_count(level),
            megabytes(bytes),
            bytes as f64 / stack.vertex_count(level).max(1) as f64
        );
    }
    println!(
        "  kept while below the finest level: {:.1} MB ({} bytes a finest vertex); all told {:.1} MB, \
         {:.0} bytes a finest vertex",
        megabytes(stack.kept_bytes()),
        stack.kept_bytes() / top_vertices,
        megabytes(stack.memory_bytes()),
        stack.memory_bytes() as f64 / top_vertices as f64
    );
}

/// A ball of 81,920 triangles taken to 1.3 and then 5.2 million.
#[test]
#[ignore]
fn timing_of_triangle_levels() {
    report("Loop, to 1.3 million triangles", &ico_sphere(6), 2);
    report("Loop, to 5.2 million triangles", &ico_sphere(6), 3);
}

/// A ring of 65,536 quads taken to 1 and then 4 million quads.
#[test]
#[ignore]
fn timing_of_polygon_levels() {
    report("Catmull-Clark, to 1 million quads", &quad_torus(256, 256), 2);
    report("Catmull-Clark, to 4 million quads", &quad_torus(256, 256), 3);
}

/// One subdivision of about a million triangles, a part at a time.
#[test]
#[ignore]
fn timing_of_one_subdivision() {
    println!("{} threads", rayon::current_num_threads());
    for (name, base) in
        [("triangles", ico_sphere(8)), ("quads", raised(&quad_torus(256, 256), &[Split::Smooth, Split::Smooth]).1)]
    {
        let (ms, mut stack) = best(|| start(&base));
        println!(
            "{name}: {} triangles, {} vertices; checked and taken in {ms:.0} ms",
            base.triangles.len() / 3,
            base.positions.len()
        );
        let (ms, fine) = best(|| {
            let mut stack = stack.clone();
            stack.subdivide(&base.positions, &base.mask, Split::Smooth).unwrap().triangles.len() / 3
        });
        println!("  subdivided to {fine} triangles in {ms:.0} ms (with a copy of the one-level stack)");
        let (ms, _) =
            best(|| stack.clone().subdivide(&base.positions, &base.mask, Split::Flat).unwrap().positions.len());
        println!("  split flat in {ms:.0} ms");
        let fine = stack.subdivide(&base.positions, &base.mask, Split::Smooth).unwrap();
        let (ms, _) = best(|| stack.triangles(1).len());
        println!("  of which writing out the finer faces {ms:.0} ms");
        let (ms, _) = best(|| stack.parent_triangles(1).unwrap().len());
        println!("  the parent of every finer triangle {ms:.0} ms");
        let (ms, _) = best(|| stack.values_up(0, 1, &base.mask).unwrap().len());
        println!("  a channel carried up {ms:.0} ms");
        let (ms, _) = best(|| {
            let coarse = stack.step_to(0, &fine.positions, &fine.mask).unwrap().mesh;
            stack.step_to(1, &coarse.positions, &coarse.mask).unwrap().mesh.positions.len()
        });
        println!("  a step down and a step back up {ms:.0} ms");
    }
}
