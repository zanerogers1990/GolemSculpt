//! The Unity benchmark's stroke, run here on the same mesh, the same brush
//! sizes in world units and the same loop path: `sculpt --bench-matched`.
//! Its lines can be set beside `BrushStrokeBenchmarks.Stroke` in the Unity
//! project, row for row. Headless like `--bench`: no window, no GPU drawing.
//!
//! What is matched: the mesh (the Unity app's noisy icosphere, rebuilt here
//! with the same jitter formula), the brush radius in world units, the strength
//! (0.5), the cursor loop (the same cone, camera and travel per frame), 90
//! frames, mirror on and off, and the frame statistics (frame 0 counts as the
//! stroke's start, as in Unity, and p95 uses the same index).

use std::collections::HashMap;
use std::time::Instant;

use glam::{Vec2, Vec3};

use crate::camera::Camera;
use crate::mesh::Mesh;
use crate::sculpt::{Brush, PointerSample, Sculptor};

const SIZE: Vec2 = Vec2::new(1920.0, 1080.0);
const SPHERE_RADIUS: f32 = 0.5;
const FRAMES: usize = 90;
const DT: f32 = 1.0 / 60.0;
/// The loop's cone half-angle about the view axis, as the Unity test has it.
const LOOP_HALF_ANGLE_DEG: f32 = 28.0;
const BRUSHES: [&str; 5] = ["Clay", "Crease", "Inflate", "Flatten", "Smooth"];

/// The Unity test's camera: 2.2 units back on -Z, looking at the origin.
/// Yaw of 180 degrees turns this app's camera (which looks down -Z from
/// `distance` on +Z) round to the same spot and direction.
fn camera() -> Camera {
    let mut camera = Camera::default();
    camera.yaw = 180.0;
    camera.distance = 2.2;
    camera
}

/// The Unity test's `LoopPoint`: a cone about -Z, tilted up a little, crossing
/// x = 0 twice per lap.
fn loop_point(angle: f32) -> Vec3 {
    let a = LOOP_HALF_ANGLE_DEG.to_radians();
    let dir = Vec3::new(a.sin() * angle.cos(), 0.1 + a.sin() * angle.sin(), -a.cos());
    dir.normalize() * SPHERE_RADIUS
}

/// The Unity test's `SurfaceRadius`: the bumpy sphere's radius along a direction.
fn surface_radius(dir: Vec3) -> f32 {
    SPHERE_RADIUS
        * (1.0
            + 0.04 * (dir.x * 23.0).sin() * (dir.y * 19.0 + 1.3).sin() * (dir.z * 17.0 + 0.7).sin()
            + 0.015 * (dir.x * 71.0 + dir.z * 53.0).sin())
}

/// `SymmetryDriftTests.SurfaceRadius`: the same bumps, taken on absolute coordinates
/// so the mesh is mirror-symmetric.
fn symmetric_surface_radius(dir: Vec3) -> f32 {
    let (x, y, z) = (dir.x.abs(), dir.y.abs(), dir.z.abs());
    SPHERE_RADIUS
        * (1.0
            + 0.04 * (x * 23.0 + 0.4).sin() * (y * 19.0 + 1.3).sin() * (z * 17.0 + 0.7).sin()
            + 0.015 * (x * 71.0 + z * 53.0 + y * 11.0).sin())
}

fn midpoint(verts: &mut Vec<Vec3>, midpoints: &mut HashMap<u64, u32>, i: u32, j: u32) -> u32 {
    let key = if i < j { ((i as u64) << 32) | j as u64 } else { ((j as u64) << 32) | i as u64 };
    if let Some(&existing) = midpoints.get(&key) {
        return existing;
    }
    let made = verts.len() as u32;
    verts.push((verts[i as usize] + verts[j as usize]).normalize());
    midpoints.insert(key, made);
    made
}

/// `SculptControllerJobParityTests.BuildNoisyIcosphere`: an icosahedron split
/// `subdivisions` times, every vertex pushed out to a bumpy sphere with a
/// per-vertex radial jitter of a fifth of an edge. 7 is 327,680 triangles;
/// 8 is 1,310,720.
fn noisy_icosphere(subdivisions: usize, symmetric: bool) -> (Vec<Vec3>, Vec<u32>) {
    let t = (1.0 + 5f32.sqrt()) * 0.5;
    let mut verts: Vec<Vec3> = [
        Vec3::new(-1.0, t, 0.0), Vec3::new(1.0, t, 0.0), Vec3::new(-1.0, -t, 0.0), Vec3::new(1.0, -t, 0.0),
        Vec3::new(0.0, -1.0, t), Vec3::new(0.0, 1.0, t), Vec3::new(0.0, -1.0, -t), Vec3::new(0.0, 1.0, -t),
        Vec3::new(t, 0.0, -1.0), Vec3::new(t, 0.0, 1.0), Vec3::new(-t, 0.0, -1.0), Vec3::new(-t, 0.0, 1.0),
    ]
    .into_iter()
    .map(Vec3::normalize)
    .collect();
    let mut tris: Vec<u32> = vec![
        0, 11, 5, 0, 5, 1, 0, 1, 7, 0, 7, 10, 0, 10, 11, 1, 5, 9, 5, 11, 4, 11, 10, 2, 10, 7, 6, 7, 1, 8,
        3, 9, 4, 3, 4, 2, 3, 2, 6, 3, 6, 8, 3, 8, 9, 4, 9, 5, 2, 4, 11, 6, 2, 10, 8, 6, 7, 9, 8, 1,
    ];

    for _ in 0..subdivisions {
        let mut midpoints = HashMap::with_capacity(tris.len());
        let mut next = Vec::with_capacity(tris.len() * 4);
        for f in tris.chunks_exact(3) {
            let (a, b, c) = (f[0], f[1], f[2]);
            let ab = midpoint(&mut verts, &mut midpoints, a, b);
            let bc = midpoint(&mut verts, &mut midpoints, b, c);
            let ca = midpoint(&mut verts, &mut midpoints, c, a);
            next.extend_from_slice(&[a, ab, ca, b, bc, ab, c, ca, bc, ab, bc, ca]);
        }
        tris = next;
    }

    let edge = 1.0515 * SPHERE_RADIUS / (1u32 << subdivisions) as f32;
    for (i, v) in verts.iter_mut().enumerate() {
        let u = *v;
        // The hash fraction gives each vertex its own jitter. The symmetric
        // variant hashes the absolute coordinates instead, so mirrored vertices
        // get mirrored jitter (`SymmetryDriftTests.BuildIcosphere`).
        let (radius, hash) = if symmetric {
            let h = (u.x.abs() * 12.9898 + u.y.abs() * 78.233 + u.z.abs() * 37.719).sin() * 43758.5453;
            (symmetric_surface_radius(u), h)
        } else {
            (surface_radius(u), (i as f32 * 12.9898).sin() * 43758.5453)
        };
        let jitter = ((hash - hash.floor()) * 2.0 - 1.0) * 0.2 * edge;
        *v = u * (radius + jitter);
    }
    (verts, tris)
}

/// One stroke along the loop with the brush and settings of one case, as the
/// Unity test's `RunStroke` does. Returns the statistics the Unity `Report`
/// prints: mean, p95 and worst over the frames after the first, and the hit count.
fn stroke(positions: &[Vec3], indices: &[u32], brush: Brush, radius: f32, travel: f32, mirror_x: bool) -> (f64, f64, f64, usize) {
    let mut mesh = Mesh::new(positions.to_vec(), indices.to_vec(), None);
    let camera = camera();
    let view = camera.view(Vec2::ZERO, SIZE, camera.distance, 1.0);

    let mut sculptor = Sculptor::default();
    sculptor.settings.brush = brush;
    sculptor.settings.screen_space_size = false;
    sculptor.settings.radius_world = radius;
    sculptor.settings.symmetry = [mirror_x, false, false];
    sculptor.settings.per_brush[brush.index()].strength = 0.5;

    let loop_radius = 0.5 * LOOP_HALF_ANGLE_DEG.to_radians().sin();
    let travel_per_frame = travel * radius;
    // The screen position whose ray from the camera meets the loop's point for this frame.
    let pixel = |frame: usize| -> Option<Vec2> {
        let angle = frame as f32 * travel_per_frame / loop_radius;
        view.project(loop_point(angle))
    };

    let mut time = 0.0f64;
    let first = pixel(0).unwrap_or(SIZE * 0.5);
    sculptor.begin(&mut mesh, &view, PointerSample { pos: first, pressure: None, time }, false, false);

    let mut frame_ms = Vec::with_capacity(FRAMES);
    let mut hits = 0;
    for frame in 0..FRAMES {
        let Some(px) = pixel(frame) else { continue };
        time += DT as f64;
        let start = Instant::now();
        sculptor.sample(&mut mesh, &view, PointerSample { pos: px, pressure: None, time });
        sculptor.frame(&mut mesh, &view, DT);
        mesh.flush_normals();
        let _ = mesh.take_dirty();
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        if frame > 0 {
            frame_ms.push(ms);
        }
        let (origin, dir) = view.ray(px);
        if mesh.raycast(origin, dir).is_some() {
            hits += 1;
        }
    }
    sculptor.end(&mut mesh, &view);

    frame_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = frame_ms.len().max(1);
    let mean = frame_ms.iter().sum::<f64>() / n as f64;
    let p95 = frame_ms[((n as f32 * 0.95) as usize).min(n - 1)];
    let worst = *frame_ms.last().unwrap_or(&0.0);
    (mean, p95, worst, hits)
}

/// Remesh and boolean costs on the source `EnginePerfBenchmarks.RemeshCost` uses
/// (the symmetric icosphere at 7 subdivisions), at the same resolutions. A warm-up
/// at resolution 64 comes first, then the best of two runs, as the Unity test does.
/// The boolean subtracts the symmetric icosphere at 5 subdivisions, moved 0.45 along +X.
pub fn run_volume() {
    println!("threads: {}", rayon::current_num_threads());
    let (positions, indices) = noisy_icosphere(7, true);
    let src_tris = indices.len() / 3;
    println!("\n== symmetric icosphere, 7 subdivisions: {src_tris} tris ==");

    let mesh = Mesh::new(positions.clone(), indices.clone(), None);
    let _ = crate::remesh::voxel_remesh(&mesh, 64, [false; 3]);
    for resolution in [256u32, 512] {
        let mut best = f64::MAX;
        let mut tris = 0;
        for _ in 0..2 {
            let t = Instant::now();
            let out = crate::remesh::voxel_remesh(&mesh, resolution, [false; 3]).expect("remesh");
            best = best.min(t.elapsed().as_secs_f64() * 1000.0);
            tris = out.triangle_count();
        }
        println!("[REMESH] src={src_tris} res={resolution} -> {tris} tris: {best:.0} ms");
    }

    let (cutter_positions, cutter_indices) = noisy_icosphere(5, true);
    let cutter_tris = cutter_indices.len() / 3;
    let cutter_positions: Vec<Vec3> = cutter_positions.into_iter().map(|p| p + Vec3::X * 0.45).collect();
    let target = sculpt_geom::TriMesh::new(positions, indices);
    let cutter = sculpt_geom::TriMesh::new(cutter_positions, cutter_indices);
    println!("\n== boolean: subtract {cutter_tris}-tris icosphere from the {src_tris}-tris one ==");
    let _ = sculpt_volume::boolean(&target, &cutter, sculpt_volume::BooleanOp::Subtract, 64);
    for resolution in [256u32, 512] {
        let mut best = f64::MAX;
        let mut tris = 0;
        for _ in 0..2 {
            let t = Instant::now();
            let out = sculpt_volume::boolean(&target, &cutter, sculpt_volume::BooleanOp::Subtract, resolution)
                .expect("boolean");
            best = best.min(t.elapsed().as_secs_f64() * 1000.0);
            tris = out.triangle_count();
        }
        println!("[BOOLEAN] subtract res={resolution} -> {tris} tris: {best:.0} ms");
    }
}

pub fn run(subdivisions: &[usize]) {
    println!("threads: {}", rayon::current_num_threads());
    println!("matches BrushStrokeBenchmarks.Stroke: noisy icosphere, brush radius in world units, loop path, {FRAMES} frames, strength 0.5");
    for &sub in subdivisions {
        let (positions, indices) = noisy_icosphere(sub, false);
        let tris = indices.len() / 3;
        println!("\n== noisy icosphere subdivision {sub}: {tris} tris, {} verts ==", positions.len());
        for name in BRUSHES {
            let brush = Brush::ALL.iter().copied().find(|b| b.name() == name).expect("the brush exists");
            for radius in [0.05f32, 0.15] {
                for travel in [0.5f32, 2.0] {
                    for mirror_x in [false, true] {
                        let (mean, p95, worst, hits) = stroke(&positions, &indices, brush, radius, travel, mirror_x);
                        println!(
                            "[BENCH] tris={tris} {name:<7} r={radius:.2} travel={travel:.1}r mirrorX={} | frame mean {mean:.2}ms p95 {p95:.2}ms worst {worst:.2}ms | hits {hits}/{FRAMES}",
                            if mirror_x { "on " } else { "off" },
                        );
                    }
                }
            }
        }
    }
}
