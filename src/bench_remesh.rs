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
//! `SCULPT_BENCH_REMESH_FROM=512` remeshes it at 512 first, so that each line
//! is a step up from there, and `SCULPT_BENCH_REMESH_SMOOTH=2` remeshes with
//! the Remesh card's Smooth at 2: the kernel column is then the smoothing and
//! the rebuild together, and Mesh::new is included in it.

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
    let setting = |name: &str| std::env::var(name).ok().and_then(|v| v.trim().parse::<u32>().ok());
    let mesh = match setting("SCULPT_BENCH_REMESH_FROM") {
        Some(from) => match crate::remesh::voxel_remesh(&mesh, from, crate::remesh::AS_IT_IS) {
            Ok(stepped) => stepped,
            Err(e) => return eprintln!("bench-remesh: the first remesh, at {from}: {e}"),
        },
        None => mesh,
    };
    let smooth = setting("SCULPT_BENCH_REMESH_SMOOTH").unwrap_or(0);
    println!("smooth: {smooth}");
    println!("source: {} triangles, {} vertices, {:.0} MB held", mesh.triangle_count(), mesh.vertex_count(), mb(mesh.memory_bytes()));
    println!(
        "{:>6} {:>10} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9} {:>7}",
        "res", "triangles", "kernel", "Mesh::new", "census", "total", "held MB", "peak MB", "noise"
    );
    for resolution in resolutions {
        if smooth > 0 {
            smoothed_line(&mesh, resolution, smooth);
            continue;
        }
        let settings = RemeshSettings::new(resolution);
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
            "{resolution:>6} {:>10} {:>8.2}s {:>8.2}s {:>8.2}s {:>8.2}s {:>9.0} {:>9.0} {:>6.2}{}",
            built.triangle_count(),
            kernel,
            new,
            census,
            total,
            mb(built.memory_bytes()),
            mb(peak_bytes()),
            shading_noise(&built),
            if closed { "" } else { "  (not closed)" },
        );
    }
}

/// One line of a remesh with Smooth, as the Remesh button runs it.
fn smoothed_line(mesh: &Mesh, resolution: u32, smooth: u32) {
    let start = Instant::now();
    let built = match crate::remesh::Copied::of(mesh).remesh(resolution, crate::remesh::AS_IT_IS, smooth) {
        Ok((built, _)) => built,
        Err(e) => return println!("{resolution:>6} refused: {e}"),
    };
    let kernel = start.elapsed().as_secs_f64();
    let t = Instant::now();
    let closed = crate::printable::census(&built).is_closed();
    let census = t.elapsed().as_secs_f64();
    println!(
        "{resolution:>6} {:>10} {kernel:>8.2}s {:>9} {census:>8.2}s {:>8.2}s {:>9.0} {:>9.0} {:>6.2}{}",
        built.triangle_count(),
        "(in it)",
        start.elapsed().as_secs_f64(),
        mb(built.memory_bytes()),
        mb(peak_bytes()),
        shading_noise(&built),
        if closed { "" } else { "  (not closed)" },
    );
}

/// The shading noise a result carries: how far, on average, each vertex
/// normal turns from the mean of its neighbours', in degrees (root mean
/// square). A scaly or pitted rebuild reads high; a clean one of a smooth
/// model under a degree.
fn shading_noise(mesh: &Mesh) -> f32 {
    let mut sums = vec![Vec3::ZERO; mesh.positions.len()];
    for t in mesh.indices.chunks_exact(3) {
        for e in 0..3 {
            sums[t[e] as usize] += mesh.normals[t[(e + 1) % 3] as usize];
        }
    }
    let total: f32 = sums
        .iter()
        .zip(&mesh.normals)
        .map(|(sum, n)| n.angle_between(sum.normalize_or(*n)).to_degrees().powi(2))
        .sum();
    (total / mesh.positions.len().max(1) as f32).sqrt()
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
