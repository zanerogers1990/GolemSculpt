//! Headless benchmark of subdivision levels: `sculpt --bench-levels [triangles...]`.
//!
//! For each size (the finest level's triangles, about; 1M, 5M and 10M unless
//! told otherwise) a ball is subdivided four times, given fine detail all
//! over, and then taken through the changes of level a sculptor makes: down
//! to the coarsest level and back up with nothing edited, the same with a
//! small edit made down there, and the same one level down. Each is run three
//! times and the shortest kept, since other work on the machine only ever
//! adds time.
//!
//! Every line also prints a fingerprint of what the step made (positions,
//! mask, hidden faces, groups and layers in the levels' own numbering, and
//! the normals), so two builds can be checked against each other bit for bit
//! as well as timed. Before the sizes, a small model with a mask, hidden
//! faces, face groups and a sculpt layer is worked through a longer script
//! (subdivide, down, edit, up, delete levels, subdivide again) for the same
//! check; its timings mean nothing.

use std::time::Instant;

use glam::Vec3;
use rayon::prelude::*;
use sculpt_multires::Split;

use crate::mesh::Mesh;
use crate::mesh::levels::{self, stepped, subdivided};
use crate::mesh::primitives::{self, Primitive};

/// Subdivisions above the ball.
const SUBDIVISIONS: usize = 4;

fn mix(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// What a step made, as one number: the levels' own print, the layers', and
/// every vertex's normal beside its position (summed, so it does not depend
/// on how the mesh happened to be numbered).
fn print(mesh: &Mesh) -> u64 {
    let levels = mesh.levels.as_ref().map_or(0, |levels| levels.print(mesh));
    let normal = |(p, n): (&Vec3, &Vec3)| {
        let bits = [p.x, p.y, p.z, n.x, n.y, n.z].map(f32::to_bits);
        bits.iter().fold(7u64, |print, &b| mix(print ^ b as u64))
    };
    let normals = mesh.positions.par_iter().zip(&mesh.normals).map(normal).reduce(|| 0, u64::wrapping_add);
    mix(levels ^ mix(mesh.layers_print() ^ mix(normals)))
}

/// Moves every vertex `to` gives a new place, as one stroke.
fn stroke(mesh: &mut Mesh, to: impl Fn(Vec3) -> Option<Vec3> + Sync) {
    let leaves: Vec<u32> = (0..mesh.bvh.leaves.len() as u32).collect();
    mesh.begin_stroke();
    mesh.move_vertices(&leaves, |_, vertex| to(vertex.pos));
    mesh.positions_changed(&leaves);
    let _ = mesh.end_stroke(false);
    let _ = mesh.take_dirty();
}

/// Fine detail all over: every vertex pushed in or out along its own
/// direction from the middle.
fn fine_detail(p: Vec3) -> Option<Vec3> {
    Some(p + p.normalize() * 0.004 * (p.x * 90.0).sin() * (p.y * 71.0).cos())
}

/// A small bump on the front of the ball, as one dab of a brush makes.
fn small_edit(p: Vec3) -> Option<Vec3> {
    let d = p.distance(Vec3::Z * 0.5) / 0.12;
    (d < 1.0).then(|| p + p.normalize() * 0.02 * (1.0 - d * d).powi(2))
}

fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

/// The shortest of three runs of `work`, which gives back the time it
/// measured and a print, the same each run.
fn best(mut work: impl FnMut() -> (f64, u64)) -> (f64, u64) {
    let (mut least, print) = work();
    for _ in 0..2 {
        least = least.min(work().0);
    }
    (least, print)
}

/// `low` stepped to `to`, timed.
fn step(low: &Mesh, to: usize) -> (f64, Mesh) {
    let clock = Instant::now();
    let mesh = stepped(low, to).expect("the step").0;
    (ms(clock), mesh)
}

pub fn run(sizes: &[usize]) {
    println!("threads: {}", rayon::current_num_threads());
    check();
    for &triangles in sizes {
        time(triangles);
    }
}

/// The timings for a finest level of about `triangles`.
fn time(triangles: usize) {
    let base = triangles >> (2 * SUBDIVISIONS);
    let mut top = primitives::build(Primitive::Sphere, base, 0.5);
    let mut subdivide_ms = 0.0;
    for _ in 0..SUBDIVISIONS {
        let clock = Instant::now();
        top = subdivided(&top, Split::Smooth).expect("subdivide").0;
        subdivide_ms = ms(clock);
    }
    stroke(&mut top, fine_detail);
    let finest = SUBDIVISIONS;
    println!(
        "\n== {} triangles on the finest of {} levels, {} vertices ==",
        top.triangle_count(),
        top.level_count(),
        top.vertex_count()
    );
    println!("{:>44}: {subdivide_ms:>8.0} ms", "last subdivide");
    let line = |what: &str, (ms, print): (f64, u64)| println!("{what:>44}: {ms:>8.0} ms   print {print:016x}");

    for low in [0, finest - 1] {
        line(
            &format!("down to level {} (from the finest)", low + 1),
            best(|| {
                let (ms, mesh) = step(&top, low);
                (ms, print(&mesh))
            }),
        );
        let down = stepped(&top, low).expect("the step").0;
        line(
            &format!("up from level {}, nothing edited", low + 1),
            best(|| {
                let (ms, mesh) = step(&down, finest);
                (ms, print(&mesh))
            }),
        );
        line(
            &format!("up from level {}, a small edit there", low + 1),
            best(|| {
                let mut edited = stepped(&top, low).expect("the step").0;
                stroke(&mut edited, small_edit);
                let (ms, mesh) = step(&edited, finest);
                (ms, print(&mesh))
            }),
        );
        line(
            &format!("round trip via level {}, nothing edited", low + 1),
            best(|| {
                let (down_ms, down) = step(&top, low);
                let (up_ms, mesh) = step(&down, finest);
                (down_ms + up_ms, print(&mesh))
            }),
        );
    }
}

/// A small model with everything a level change carries, worked through a
/// longer script; prints a fingerprint after each stage.
fn check() {
    let say = |what: &str, mesh: &Mesh| println!("check {what:>40}: print {:016x}", print(mesh));
    let mut mesh = primitives::build(Primitive::Sphere, 1_200, 0.5);
    for _ in 0..3 {
        mesh = subdivided(&mesh, Split::Smooth).expect("subdivide").0;
    }
    stroke(&mut mesh, fine_detail);
    let (layer, _) = mesh.add_layer(Some("Detail".into()));
    stroke(&mut mesh, |p| Some(p + Vec3::X * 0.01 * (p.y * 40.0).sin()));
    mesh.record_on_layer(None);
    let _ = mesh.set_layer(layer, true, 70);
    let _ = mesh.remap_mask(|mesh, v, _| (mesh.positions[v as usize].y * 3.0).clamp(0.0, 1.0));
    let hide: Vec<u32> = (0..mesh.triangle_count() as u32).filter(|t| t % 7 == 0).collect();
    let _ = mesh.hide(&hide);
    let ids: Vec<u16> = (0..mesh.triangle_count()).map(|t| (t % 5) as u16).collect();
    mesh.set_group_ids(ids);
    let _ = mesh.take_dirty();
    say("dressed on the finest of four levels", &mesh);

    let top = 3;
    let low = stepped(&mesh, 0).expect("down").0;
    say("down to level 1", &low);
    say("back up, nothing edited", &stepped(&low, top).expect("up").0);
    let mut low = low;
    stroke(&mut low, small_edit);
    let mid = stepped(&low, 1).expect("up one").0;
    say("a small edit on 1, up to 2", &mid);
    let mut mid = mid;
    stroke(&mut mid, |p| Some(p + Vec3::Y * 0.05 * p.x.max(0.0)));
    let _ = mid.remap_mask(|_, _, m| (m + 0.25).min(1.0));
    let up = stepped(&mid, top).expect("up").0;
    say("a broad edit on 2, up to 4", &up);
    let down = stepped(&up, 2).expect("down").0;
    say("down to 3", &down);
    let up2 = stepped(&down, top).expect("up").0;
    say("up to 4 again", &up2);

    // Delete Higher on level 2, subdivide again, and go down and up.
    let mut cut = stepped(&up2, 1).expect("down").0;
    cut.levels = levels::without(&cut, false);
    say("Delete Higher on level 2", &cut);
    let again = subdivided(&cut, Split::Flat).expect("subdivide").0;
    say("subdivided flat from there", &again);
    let low = stepped(&again, 0).expect("down").0;
    let mut low = low;
    stroke(&mut low, small_edit);
    say("down to 1 with an edit, up to 3", &stepped(&low, 2).expect("up").0);
    // Delete Lower on level 2 of those three.
    let mut cut = stepped(&again, 1).expect("down").0;
    cut.levels = levels::without(&cut, true);
    let up = stepped(&cut, 1).expect("up").0;
    say("Delete Lower on 2, then up", &up);
}
