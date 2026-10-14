//! Headless engine benchmark: `sculpt --bench`. Drives scripted strokes
//! through the real stroke code with no window, and reports per-frame cost.

use std::time::Instant;

use glam::Vec2;

use crate::camera::Camera;
use crate::mesh::primitives::{self, Primitive};
use crate::mesh::{Mesh, StrokeDelta};
use crate::sculpt::{Brush, PointerSample, Sculptor};

const SIZE: Vec2 = Vec2::new(1920.0, 1080.0);

/// Runs one stroke as a series of frames, each with a few pointer samples,
/// then undoes it so the next run starts from the same untouched surface.
/// (Left in place, each stroke would deform the mesh the following ones are
/// timed on, and a later brush could end up measuring almost nothing.)
/// Returns (mean ms, worst ms) per frame including normals, and how many
/// vertices the stroke changed.
pub fn stroke(
    mesh: &mut Mesh,
    sculptor: &mut Sculptor,
    camera: &Camera,
    path: impl Fn(f32) -> Vec2,
    frames: usize,
    samples_per_frame: usize,
) -> (f64, f64, usize) {
    let scene = mesh.bounds().size().max_element();
    let view = camera.view(Vec2::ZERO, SIZE, camera.distance, scene);
    let mut time = 0.0f64;
    let sample = |t: f32, time: f64| PointerSample { pos: path(t), pressure: None, time };
    if !sculptor.begin(mesh, &view, sample(0.0, time), false, false) {
        return (f64::NAN, f64::NAN, 0);
    }
    let total = frames * samples_per_frame;
    let (mut sum, mut worst) = (0.0f64, 0.0f64);
    for f in 0..frames {
        let start = Instant::now();
        for s in 0..samples_per_frame {
            let i = f * samples_per_frame + s + 1;
            time += 1.0 / 60.0 / samples_per_frame as f64;
            sculptor.sample(mesh, &view, sample(i as f32 / total as f32, time));
        }
        sculptor.frame(mesh, &view, 1.0 / 60.0);
        mesh.flush_normals();
        let _ = mesh.take_dirty();
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        sum += ms;
        worst = worst.max(ms);
    }
    let delta = sculptor.end(mesh, &view);
    let changed = match &delta {
        Some(StrokeDelta::Positions { verts, .. }) | Some(StrokeDelta::Mask { verts, .. }) => {
            verts.len()
        }
        // The benchmark's meshes have no sculpt layers.
        Some(StrokeDelta::Layered { .. } | StrokeDelta::Paint(_) | StrokeDelta::Painted { .. }) | None => 0,
    };
    if let Some(delta) = &delta {
        mesh.apply_delta(delta, true);
        let _ = mesh.take_dirty();
    }
    (sum / frames as f64, worst, changed)
}

pub fn run(sizes: &[usize]) {
    println!("threads: {}", rayon::current_num_threads());
    println!("columns: mean/worst ms per frame for r=40px, r=40px+mirror, r=150px, r=150px+mirror");
    for &tris in sizes {
        let t = Instant::now();
        let mut mesh = primitives::build(Primitive::Sphere, tris, 0.5);
        println!(
            "\n== sphere: {} tris, {} verts (built in {:.0} ms) ==",
            mesh.triangle_count(),
            mesh.vertex_count(),
            t.elapsed().as_secs_f64() * 1000.0
        );
        let mut camera = Camera::default();
        camera.frame(mesh.bounds(), SIZE.x / SIZE.y, false);

        let t = Instant::now();
        let view = camera.view(Vec2::ZERO, SIZE, camera.distance, 1.0);
        let mut hits = 0;
        for i in 0..10_000 {
            let px = Vec2::new(700.0 + (i % 100) as f32 * 5.0, 300.0 + (i / 100) as f32 * 5.0);
            let (o, d) = view.ray(px);
            hits += mesh.raycast(o, d).is_some() as u32;
        }
        println!(
            "raycast: {:.2} us each ({hits}/10000 hit)",
            t.elapsed().as_secs_f64() * 1e6 / 10_000.0
        );

        for brush in Brush::ALL {
            let mut line = format!("{:>10}", brush.name());
            let mut changed = Vec::new();
            for &radius_px in &[40.0f32, 150.0] {
                for symmetry in [false, true] {
                    let mut sculptor = Sculptor::default();
                    sculptor.settings.brush = brush;
                    sculptor.settings.radius_px = radius_px;
                    sculptor.settings.symmetry = [symmetry, false, false];
                    sculptor.settings.per_brush[brush.index()].strength = 0.5;
                    // A wavy sweep across the front of the sphere, crossing
                    // the mirror plane, in 40 frames of 4 pointer samples.
                    let path = |t: f32| {
                        Vec2::new(
                            SIZE.x * 0.5 - 180.0 + 360.0 * t,
                            SIZE.y * 0.5 + (t * 9.0).sin() * 60.0,
                        )
                    };
                    let (mean, worst, verts) =
                        stroke(&mut mesh, &mut sculptor, &camera, path, 40, 4);
                    line += &format!("  {mean:>6.2}/{worst:<6.2}");
                    changed.push(format!("{}k", verts / 1000));
                }
            }
            // Vertex counts make a row that touched almost nothing obvious.
            println!("{line}  verts changed: {}", changed.join(" / "));
        }

        // The voxel remesh of the same sphere, as the Remesh button runs it:
        // the rebuild itself and making a sculpt mesh of the result.
        let mut line = format!("{:>10}", "Remesh");
        for resolution in [256, 512] {
            for symmetry in [false, true] {
                let t = Instant::now();
                let rebuilt = crate::remesh::voxel_remesh(&mesh, resolution, [symmetry, false, false]);
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                let tris = rebuilt.map_or(0, |m| m.triangle_count());
                line += &format!("  {ms:>6.0} ms ({}k)", tris / 1000);
            }
        }
        println!("{line}  at 256, 256+mirror, 512, 512+mirror (triangles made)");
    }
}
