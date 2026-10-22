//! What a Remesh costs at the resolutions past where the slider used to stop:
//! `sculpt.exe --bench-remesh [resolutions...]`. Headless: no window, no GPU.
//!
//! Each line is one remesh, as the Remesh button runs it, taken apart: the
//! kernel's rebuild (`remesh_with`), making a sculpt mesh of what it gives
//! back (`Mesh::new`: the spatial index, adjacency, normals), and the closed
//! surface check the status line runs after it. Then what the result holds
//! in memory, and the most the whole process has held so far.
//!
//! The model is a lumpy ball unless `SCULPT_BENCH_REMESH_FILE` names a file
//! (`.sclp`, whose selected object is used, or anything Import reads).

use std::time::Instant;

use glam::Vec3;
use sculpt_volume::remesh::{RemeshSettings, remesh_with};

use crate::mesh::Mesh;
use crate::mesh::primitives::{self, Primitive};

/// Where the slider stopped before, and on up to the kernel's own ceiling.
const RESOLUTIONS: [u32; 5] = [1024, 1536, 2048, 3072, 4096];

pub fn run(resolutions: &[usize]) {
    let resolutions: Vec<u32> =
        if resolutions.is_empty() { RESOLUTIONS.to_vec() } else { resolutions.iter().map(|&r| r as u32).collect() };
    println!("threads: {}", rayon::current_num_threads());
    let mesh = match source() {
        Ok(mesh) => mesh,
        Err(e) => return eprintln!("bench-remesh: {e}"),
    };
    println!("source: {} triangles, {} vertices, {:.0} MB held", mesh.triangle_count(), mesh.vertex_count(), mb(mesh.memory_bytes()));
    println!("{:>6} {:>10} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9}", "res", "triangles", "kernel", "Mesh::new", "census", "total", "held MB", "peak MB");
    for resolution in resolutions {
        let settings = RemeshSettings { resolution, mirror: [false; 3], max_triangles: sculpt_volume::budget::MAX_TRIANGLES };
        let start = Instant::now();
        let rebuilt = match remesh_with(&mesh.positions, &mesh.indices, &settings) {
            Ok(rebuilt) => rebuilt,
            Err(e) => {
                println!("{resolution:>6} refused: {e}");
                continue;
            }
        };
        let kernel = start.elapsed().as_secs_f64();
        let t = Instant::now();
        let built = Mesh::new(rebuilt.mesh.positions, rebuilt.mesh.indices, None);
        let new = t.elapsed().as_secs_f64();
        let t = Instant::now();
        let closed = crate::printable::census(&built).is_closed();
        let census = t.elapsed().as_secs_f64();
        let total = start.elapsed().as_secs_f64();
        println!(
            "{resolution:>6} {:>10} {:>8.2}s {:>8.2}s {:>8.2}s {:>8.2}s {:>9.0} {:>9.0}{}",
            built.triangle_count(),
            kernel,
            new,
            census,
            total,
            mb(built.memory_bytes()),
            mb(peak_bytes()),
            if closed { "" } else { "  (not closed)" },
        );
    }
}

fn mb(bytes: usize) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn source() -> Result<Mesh, String> {
    let Ok(path) = std::env::var("SCULPT_BENCH_REMESH_FILE") else { return Ok(lumpy_ball()) };
    let path = std::path::Path::new(&path);
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("sclp")) {
        let mut scene = crate::io::load_scene(path).map_err(|e| e.to_string())?;
        let active = scene.active.min(scene.meshes.len().saturating_sub(1));
        return (!scene.meshes.is_empty()).then(|| scene.meshes.swap_remove(active)).ok_or("the file has no objects".into());
    }
    let raw = crate::io::import(path).map_err(|e| e.to_string())?;
    Ok(Mesh::new(raw.positions, raw.indices, None))
}

/// A ball of a million triangles with lumps of several sizes on it: more
/// surface for its size than a plain ball, as a sculpted model has.
fn lumpy_ball() -> Mesh {
    let ball = primitives::build(Primitive::Sphere, 1_000_000, 0.5);
    let lumps = |p: Vec3| {
        let n = p.normalize_or_zero();
        0.06 * (7.0 * n.x).sin() * (5.0 * n.y).cos() + 0.03 * (17.0 * n.z).sin() * (13.0 * n.x).cos()
            + 0.012 * (41.0 * n.y).sin() * (37.0 * n.z).sin()
    };
    let positions = ball.positions.iter().map(|&p| p * (1.0 + lumps(p))).collect();
    Mesh::new(positions, ball.indices.clone(), None)
}

/// The most memory the process has held at once so far.
#[cfg(windows)]
fn peak_bytes() -> usize {
    #[repr(C)]
    #[derive(Default)]
    struct Counters {
        cb: u32,
        page_faults: u32,
        peak_working_set: usize,
        working_set: usize,
        quota_peak_paged: usize,
        quota_paged: usize,
        quota_peak_non_paged: usize,
        quota_non_paged: usize,
        pagefile: usize,
        peak_pagefile: usize,
    }
    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(process: isize, counters: *mut Counters, cb: u32) -> i32;
    }
    let mut counters = Counters { cb: size_of::<Counters>() as u32, ..Default::default() };
    // SAFETY: the struct is PROCESS_MEMORY_COUNTERS laid out as Windows has it,
    // its size is passed in, and the pseudo-handle needs no closing.
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) };
    if ok != 0 { counters.peak_pagefile.max(counters.peak_working_set) } else { 0 }
}

#[cfg(not(windows))]
fn peak_bytes() -> usize {
    0
}
