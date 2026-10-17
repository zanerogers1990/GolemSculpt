//! The viewport's own benchmark and picture check: the 3D drawing alone, with
//! no interface and no scene, so two builds can be compared on it.
//!
//! - `sculpt --bench-render [triangles...]` times frames of a sphere of each
//!   size (1M, 5M and 10M triangles unless told otherwise): orbiting, which
//!   draws everything afresh each frame, and "cursor only", where nothing in
//!   the picture changes (what a pointer move over the model asks for), under
//!   the default lit look, each finish and the matcap; then, lit, with half
//!   the model off the screen and with a third of its faces hidden; then the
//!   same sphere fully painted with four materials and a band of colour
//!   ("painted lit", "painted matcap", and "painted off" with both Show
//!   switches off, which is that mesh through the unpainted pipelines). Each
//!   frame is waited for (`device.poll`), so a time is the whole frame's, the
//!   graphics card's included. Prints a table, and a `RESULT` line per cell
//!   for `--bench-render-ab`.
//! - `sculpt --bench-render-ab <other sculpt.exe> [rounds] [triangles...]`
//!   runs this build's bench and the other's in turns, `rounds` times each
//!   (3 unless told), keeps each cell's best and prints both with the ratio.
//!   Timings swing while other work shares the machine; taking turns and the
//!   best of each keeps that out of the comparison.
//! - `sculpt --render-shots <folder>` draws a fixed set of pictures (every
//!   finish and preset, the matcap, flat shading, glows, a mirrored live copy,
//!   hidden faces, the overlays) at a fixed clock, so two builds' pictures can
//!   be compared pixel for pixel;
//! - `sculpt --compare-shots <folder a> <folder b>` does that comparison.
//!
//! Each opens a small window for a moment: the graphics device is reached
//! through one.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use glam::{Mat4, Vec2, Vec3};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

use crate::camera::{Camera, View};
use crate::doc::Options;
use crate::look::{self, Frame, LookUniform};
use crate::mesh::Mesh;
use crate::mesh::bvh::Aabb;
use crate::mesh::primitives::{self, Primitive};
use crate::render::{flash, DrawItem, Globals, Gpu, Renderer};

const BENCH_SIZE: (u32, u32) = (1920, 1080);
const SHOT_SIZE: (u32, u32) = (960, 540);
/// The clock every picture and frame is drawn at, so a glow is caught at the
/// same moment of its animation in every build.
const CLOCK: f32 = 1.25;
const WARMUP_FRAMES: usize = 5;
const TIMED_FRAMES: usize = 40;

/// Opens a small window, reaches the graphics device through it and hands
/// that to `work`.
fn with_gpu(work: impl FnOnce(&Gpu)) -> Result<(), String> {
    struct Host<F> {
        work: Option<F>,
        error: Option<String>,
    }
    impl<F: FnOnce(&Gpu)> ApplicationHandler for Host<F> {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            let Some(work) = self.work.take() else { return };
            let attrs = Window::default_attributes()
                .with_title("GolemSculpt render bench")
                .with_inner_size(winit::dpi::PhysicalSize::new(480, 270))
                .with_active(false);
            match event_loop.create_window(attrs) {
                Ok(window) => match pollster::block_on(Gpu::new(Arc::new(window))) {
                    Ok(gpu) => work(&gpu),
                    Err(e) => self.error = Some(e),
                },
                Err(e) => self.error = Some(e.to_string()),
            }
            event_loop.exit();
        }

        fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
    }
    let event_loop = EventLoop::new().map_err(|e| format!("Could not start the event loop: {e}"))?;
    let mut host = Host { work: Some(work), error: None };
    event_loop.run_app(&mut host).map_err(|e| e.to_string())?;
    host.error.map_or(Ok(()), Err)
}

fn srgb_to_linear(c: [f32; 3]) -> [f32; 3] {
    c.map(|v| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) })
}

/// The frame's shared shader inputs, as `App::globals` makes them, with the
/// active object's X mirror guide showing when `guides` is set.
pub(crate) fn globals(view: &View, opts: &Options, guides: bool) -> Globals {
    let lin4 = |c: [f32; 3]| {
        let c = srgb_to_linear(c);
        [c[0], c[1], c[2], 0.0]
    };
    let light = |d: [f32; 3], c: [f32; 3], i: f32| {
        let c = srgb_to_linear(c);
        ([d[0], d[1], d[2], 0.0], [c[0] * i, c[1] * i, c[2] * i, 0.0])
    };
    let key = light([-0.492, 0.643, 0.587], [1.0, 0.969, 0.922], 1.56);
    let fill = light([0.754, 0.174, 0.633], [0.859, 0.910, 1.0], 0.40);
    let rim = light([0.410, 0.574, -0.709], [1.0, 1.0, 1.0], 1.08);
    let cavity = |x: f32| if opts.cavity { x } else { 0.0 };
    let base = srgb_to_linear(opts.base_color);
    let bg = srgb_to_linear(opts.background);
    let top = srgb_to_linear(opts.background_top);
    Globals {
        view_proj: view.view_proj.to_cols_array_2d(),
        view: view.view.to_cols_array_2d(),
        cam_pos: view.pos.extend(1.0).to_array(),
        cam_right: view.right.extend(0.0).to_array(),
        cam_up: view.up.extend(0.0).to_array(),
        cam_forward: view.forward.extend(0.0).to_array(),
        base_color: [base[0], base[1], base[2], opts.smoothness],
        bg_bottom: [bg[0], bg[1], bg[2], if opts.gradient_background { 1.0 } else { 0.0 }],
        bg_top: [top[0], top[1], top[2], 1.0],
        shading: [
            if opts.matcap_on { 1.0 } else { 0.0 },
            if opts.flat_shading { 1.0 } else { 0.0 },
            cavity(opts.cavity_ridge),
            cavity(opts.cavity_valley),
        ],
        symmetry: [if guides { 1.0 } else { 0.0 }, 0.0, 0.0, 1.0],
        viewport: [
            view.size.x,
            view.size.y,
            if view.ortho_half_height.is_some() { 0.0 } else { view.tan_half_fov },
            view.size.x / view.size.y,
        ],
        light_dir: [key.0, fill.0, rim.0],
        light_color: [key.1, fill.1, rim.1],
        ambient_sky: lin4([0.42, 0.44, 0.48]),
        ambient_equator: lin4([0.30, 0.30, 0.31]),
        ambient_ground: lin4([0.16, 0.15, 0.14]),
        radial_axis: [0.0; 4],
        radial_spoke: [0.0; 4],
        guide_space: Mat4::IDENTITY.to_cols_array_2d(),
    }
}

/// Turns Show Backfaces on or off, where this build has it.
fn set_backfaces(opts: &mut Options, on: bool) {
    opts.show_backfaces = on;
}

/// What is drawn: meshes by id, and the objects placed from them (the first
/// is the active one).
struct Stage {
    renderer: Renderer,
    meshes: Vec<(u64, Mesh)>,
    objects: Vec<(u64, Mat4)>,
    camera: Camera,
    opts: Options,
    size: (u32, u32),
    target: wgpu::Texture,
    view_target: wgpu::TextureView,
    guides: bool,
    /// The palette painted meshes are drawn with, if any are.
    palette: Option<crate::materials::rows::Upload>,
}

impl Stage {
    fn new(gpu: &Gpu, size: (u32, u32)) -> Stage {
        let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bench target"),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: gpu.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view_target = target.create_view(&Default::default());
        Stage {
            renderer: Renderer::new(gpu, &crate::ui::assets::default_matcap()),
            meshes: Vec::new(),
            objects: Vec::new(),
            camera: Camera::default(),
            opts: Options::default(),
            size,
            target,
            view_target,
            guides: false,
            palette: None,
        }
    }

    /// World bounds of every object.
    fn bounds(&self) -> Aabb {
        let mut out = Aabb::EMPTY;
        for (id, model) in &self.objects {
            let Some((_, mesh)) = self.meshes.iter().find(|(m, _)| m == id) else { continue };
            let b = mesh.bounds();
            for i in 0..8 {
                let corner = Vec3::new(
                    if i & 1 == 0 { b.min.x } else { b.max.x },
                    if i & 2 == 0 { b.min.y } else { b.max.y },
                    if i & 4 == 0 { b.min.z } else { b.max.z },
                );
                out.grow(model.transform_point3(corner));
            }
        }
        out
    }

    fn frame_camera(&mut self, yaw: f32, pitch: f32) {
        let aspect = self.size.0 as f32 / self.size.1 as f32;
        self.camera.yaw = yaw;
        self.camera.pitch = pitch;
        self.camera.frame(self.bounds(), aspect, false);
    }

    /// Sends every mesh's changes to the graphics card, as a frame does.
    fn sync(&mut self, gpu: &Gpu) {
        let mut painted_curvature = false;
        if let Some(palette) = &self.palette {
            self.renderer.painted.set_palette(gpu, palette);
            painted_curvature = !self.opts.matcap_on && self.opts.paint.show_materials && palette.any_curvature();
        }
        self.renderer.want_curvature = self.opts.look.needs_curvature(self.opts.matcap_on) || painted_curvature;
        self.renderer.wire.enabled = self.opts.wireframe;
        for (id, mesh) in &mut self.meshes {
            self.renderer.sync_mesh(gpu, *id, mesh, false);
        }
    }

    fn view(&self) -> View {
        let size = Vec2::new(self.size.0 as f32, self.size.1 as f32);
        let scene = self.bounds().size().max_element().max(1e-4);
        self.camera.view(Vec2::ZERO, size, self.camera.distance, scene)
    }

    fn inputs(&mut self) -> (Globals, LookUniform, Vec<DrawItem>) {
        let view = self.view();
        let globals = globals(&view, &self.opts, self.guides);
        self.opts.look.track_camera(view.forward, view.up);
        let bounds = self.bounds();
        let look = self.opts.look.uniform(&Frame {
            cam_right: view.right,
            cam_up: view.up,
            cam_forward: view.forward,
            ortho_half_height: view.ortho_half_height,
            eye_offset: view.depth_of(self.camera.pivot) - self.camera.distance,
            scene_min: bounds.min,
            scene_max: bounds.max,
            model_size: bounds.size().max_element(),
            time: CLOCK,
            gradient_background: self.opts.gradient_background,
            smoothness: self.opts.smoothness,
        });
        let items = self
            .objects
            .iter()
            .enumerate()
            .map(|(i, (mesh, model))| {
                let mut model = model.to_cols_array_2d();
                model[3][3] = if i == 0 { 1.0 } else { 0.0 };
                DrawItem { mesh: *mesh, model }
            })
            .collect();
        (globals, look, items)
    }

    /// Draws one frame into the target and waits for the card to finish it.
    fn draw(&mut self, gpu: &Gpu) {
        self.sync(gpu);
        let (globals, look, items) = self.inputs();
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        self.renderer.draw(gpu, &mut encoder, &self.view_target, self.size, &globals, &look, &self.opts, &items);
        gpu.queue.submit([encoder.finish()]);
        let _ = gpu.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
    }
}

// ---- benchmark ---------------------------------------------------------------

/// How the camera moves from one timed frame to the next.
#[derive(Clone, Copy, PartialEq)]
enum Motion {
    /// Turning round the model: everything is drawn afresh.
    Orbit,
    /// Standing still: what a pointer move over the model leaves of a frame.
    Cursor,
}

impl Motion {
    fn name(self) -> &'static str {
        match self {
            Motion::Orbit => "orbit",
            Motion::Cursor => "cursor",
        }
    }
}

/// The looks timed on every size: name and look setting (see `look::apply_spec`).
const LOOKS: [(&str, &str); 6] = [
    ("lit", "matcap=off"),
    ("lure", "matcap=off,finish=lure"),
    ("metal", "matcap=off,finish=metal"),
    ("clay", "matcap=off,finish=clay"),
    ("wood", "matcap=off,finish=wood"),
    ("matcap", "matcap=on"),
];

/// Median milliseconds per frame over the timed frames.
fn time_frames(gpu: &Gpu, stage: &mut Stage, motion: Motion) -> f64 {
    let mut times = Vec::with_capacity(TIMED_FRAMES);
    for i in 0..WARMUP_FRAMES + TIMED_FRAMES {
        if motion == Motion::Orbit {
            stage.camera.add_yaw(0.5);
        }
        let start = Instant::now();
        stage.draw(gpu);
        if i >= WARMUP_FRAMES {
            times.push(start.elapsed().as_secs_f64() * 1000.0);
        }
    }
    times.sort_by(f64::total_cmp);
    times[times.len() / 2]
}

/// `sculpt --bench-render [triangles...]`.
pub fn run(sizes: &[usize]) {
    let result = with_gpu(|gpu| {
        println!("Viewport render benchmark, {}x{}, on {}", BENCH_SIZE.0, BENCH_SIZE.1, gpu.adapter_name);
        println!("Median ms per frame (each frame waited for), {TIMED_FRAMES} frames after {WARMUP_FRAMES} warm-up.");
        println!("{:>10}  {:<16} {:>9} {:>9}", "triangles", "look", "orbit", "cursor");
        for &n in sizes {
            let mut stage = Stage::new(gpu, BENCH_SIZE);
            let mesh = primitives::build(Primitive::Sphere, n, 1.0);
            let tris = mesh.triangle_count();
            stage.meshes.push((1, mesh));
            stage.objects.push((1, Mat4::IDENTITY));
            let row = |stage: &mut Stage, name: &str, spec: &str, motions: &[Motion]| {
                let mut cells = Vec::new();
                for &motion in motions {
                    stage.opts = Options::default();
                    look::apply_spec(&mut stage.opts, spec, 2.0);
                    stage.frame_camera(30.0, -15.0);
                    let ms = time_frames(gpu, stage, motion);
                    println!("RESULT\t{tris}\t{name}\t{}\t{ms:.3}", motion.name());
                    cells.push(ms);
                }
                let cell = |i: usize| cells.get(i).map_or("-".to_string(), |ms| format!("{ms:.2}"));
                println!("{tris:>10}  {name:<16} {:>9} {:>9}", cell(0), cell(1));
            };
            for (name, spec) in LOOKS {
                row(&mut stage, name, spec, &[Motion::Orbit, Motion::Cursor]);
            }
            // Half the model off the left edge of the screen.
            let mut half = Stage::new(gpu, BENCH_SIZE);
            half.meshes.push(stage.meshes.pop().unwrap());
            half.objects.push((1, Mat4::IDENTITY));
            half.opts = Options::default();
            look::apply_spec(&mut half.opts, "matcap=off", 2.0);
            half.frame_camera(30.0, -15.0);
            let aspect = BENCH_SIZE.0 as f32 / BENCH_SIZE.1 as f32;
            let shift = half.camera.distance * half.camera.tan_half_fov() * aspect;
            let right = half.camera.rotation() * Vec3::X;
            let mut times = Vec::new();
            for i in 0..WARMUP_FRAMES + TIMED_FRAMES {
                half.camera.add_yaw(0.5);
                half.camera.pivot = right * shift;
                let start = Instant::now();
                half.draw(gpu);
                if i >= WARMUP_FRAMES {
                    times.push(start.elapsed().as_secs_f64() * 1000.0);
                }
            }
            times.sort_by(f64::total_cmp);
            let ms = times[times.len() / 2];
            println!("RESULT\t{tris}\tlit half-off\torbit\t{ms:.3}");
            println!("{tris:>10}  {:<16} {ms:>9.2} {:>9}", "lit half-off", "-");
            // A third of the faces hidden.
            let (_, mesh) = &mut half.meshes[0];
            let hide: Vec<u32> = (0..mesh.triangle_count() as u32)
                .filter(|&t| {
                    let c = mesh.indices[t as usize * 3..t as usize * 3 + 3]
                        .iter()
                        .map(|&v| mesh.positions[v as usize])
                        .sum::<Vec3>();
                    c.x > 0.6
                })
                .collect();
            mesh.hide(&hide);
            let mut hidden = Stage::new(gpu, BENCH_SIZE);
            hidden.meshes.push(half.meshes.pop().unwrap());
            hidden.objects.push((1, Mat4::IDENTITY));
            row(&mut hidden, "lit hidden", "matcap=off", &[Motion::Orbit]);
            drop(hidden);
            painted_rows(gpu, n);
        }
    });
    if let Err(e) = result {
        eprintln!("{e}");
    }
}

/// The materials the painted rows are painted with: one of each finish
/// that is not plain, the costliest first.
const PAINTED_WITH: [&str; 4] = ["Lure - Blue Flake", "Metal - Rust", "Wood - Golden Oak", "Clay - Terracotta"];

/// The painted rows for a sphere of `n` triangles: all of it painted with
/// four materials in soft-edged wedges and a band of colour, through the
/// paint data's own layer commands (`materials::demo`); drawn painted, with
/// both Show switches off (the same mesh through the unpainted pipelines),
/// and under the matcap.
fn painted_rows(gpu: &Gpu, n: usize) {
    use crate::materials::{Library, custom::Shaders, palette, rows};
    let started = Instant::now();
    let mut mesh = primitives::build(Primitive::Sphere, n, 1.0);
    let tris = mesh.triangle_count();
    let library = Library::default();
    let shaders = Shaders::default();
    let mut paint = crate::mesh::paint::palette::ScenePaint::default();
    let slots: Vec<u8> = PAINTED_WITH
        .iter()
        .filter_map(|name| library.named(name))
        .filter_map(|i| palette::ensure_in_palette(&mut paint, &library.entries()[i].material, 2.0, &shaders).ok())
        .collect();
    if let Err(why) = crate::materials::demo::paint_bands(&mut mesh, &slots, Some([205, 70, 45]), false) {
        eprintln!("could not paint the bench mesh: {why}");
        return;
    }
    eprintln!("painted {tris} triangles in {:.1} s", started.elapsed().as_secs_f32());
    let mut stage = Stage::new(gpu, BENCH_SIZE);
    stage.meshes.push((1, mesh));
    stage.objects.push((1, Mat4::IDENTITY));
    stage.palette = Some(rows::upload(&paint, 2.0, &shaders));
    let cases: [(&str, &str, bool, &[Motion]); 3] = [
        ("painted lit", "matcap=off", true, &[Motion::Orbit, Motion::Cursor]),
        ("painted off", "matcap=off", false, &[Motion::Orbit]),
        ("painted matcap", "matcap=on", true, &[Motion::Orbit]),
    ];
    for (name, spec, shown, motions) in cases {
        let mut cells = Vec::new();
        for &motion in motions {
            stage.opts = Options::default();
            look::apply_spec(&mut stage.opts, spec, 2.0);
            stage.opts.paint.palette = paint.palette.clone();
            (stage.opts.paint.show_polypaint, stage.opts.paint.show_materials) = (shown, shown);
            stage.frame_camera(30.0, -15.0);
            let ms = time_frames(gpu, &mut stage, motion);
            println!("RESULT	{tris}	{name}	{}	{ms:.3}", motion.name());
            cells.push(ms);
        }
        let cell = |i: usize| cells.get(i).map_or("-".to_string(), |ms| format!("{ms:.2}"));
        println!("{tris:>10}  {name:<16} {:>9} {:>9}", cell(0), cell(1));
    }
}

/// One timed cell of a `RESULT` line: triangles, look, motion.
type Cell = (usize, String, String);

/// Runs `exe --bench-render sizes` and reads its `RESULT` lines.
fn bench_of(exe: &Path, sizes: &[usize]) -> BTreeMap<Cell, f64> {
    let mut out = BTreeMap::new();
    let run = std::process::Command::new(exe)
        .arg("--bench-render")
        .args(sizes.iter().map(|n| n.to_string()))
        .output();
    let Ok(run) = run else {
        eprintln!("could not run {}", exe.display());
        return out;
    };
    for line in String::from_utf8_lossy(&run.stdout).lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if let ["RESULT", tris, name, motion, ms] = parts[..] {
            if let (Ok(tris), Ok(ms)) = (tris.parse(), ms.parse()) {
                out.insert((tris, name.to_string(), motion.to_string()), ms);
            }
        }
    }
    out
}

/// `sculpt --bench-render-ab <other sculpt.exe> [rounds] [triangles...]`.
pub fn run_ab(args: &[String]) {
    let Some(other) = args.first().map(PathBuf::from) else {
        eprintln!("--bench-render-ab needs the other build's sculpt.exe");
        return;
    };
    let numbers: Vec<usize> = args[1..].iter().filter_map(|a| a.parse().ok()).collect();
    let (rounds, sizes) = match numbers.split_first() {
        Some((&r, rest)) if r < 100 => (r.max(1), rest.to_vec()),
        _ => (3, numbers.clone()),
    };
    let sizes = if sizes.is_empty() { vec![1_000_000, 5_000_000, 10_000_000] } else { sizes };
    let this = std::env::current_exe().unwrap_or_default();
    let mut best: [BTreeMap<Cell, f64>; 2] = Default::default();
    for round in 0..rounds {
        for (i, exe) in [&other, &this].into_iter().enumerate() {
            eprintln!("round {} of {rounds}: {}", round + 1, exe.display());
            for (cell, ms) in bench_of(exe, &sizes) {
                let slot = best[i].entry(cell).or_insert(f64::INFINITY);
                *slot = slot.min(ms);
            }
        }
    }
    println!("Best of {rounds} runs each, ms per frame. A = {}, B = {}", other.display(), this.display());
    println!("{:>10}  {:<16} {:<7} {:>9} {:>9} {:>8}", "triangles", "look", "motion", "A", "B", "A / B");
    for (cell, &a) in &best[0] {
        let b = best[1].get(cell).copied().unwrap_or(f64::NAN);
        println!("{:>10}  {:<16} {:<7} {a:>9.2} {b:>9.2} {:>7.2}x", cell.0, cell.1, cell.2, a / b);
    }
}

// ---- pictures ------------------------------------------------------------------

/// A sphere with broad bumps and fine ripples, so the finishes that follow
/// the surface's recesses and ridges have some to follow.
fn bumpy_sphere(tris: usize) -> Mesh {
    let sphere = primitives::build(Primitive::Sphere, tris, 1.0);
    let positions = sphere
        .positions
        .iter()
        .map(|&p| {
            let n = p.normalize_or_zero();
            let bump = 0.06 * (5.0 * n.x).sin() * (4.0 * n.y).cos() + 0.012 * (23.0 * n.z + 9.0 * n.x).sin();
            p + n * bump
        })
        .collect();
    Mesh::new(positions, sphere.indices.clone(), None)
}

/// Hides the cap of `mesh` facing the camera's default view, opening it up.
fn open_front(mesh: &mut Mesh) {
    let cap: Vec<u32> = (0..mesh.triangle_count() as u32)
        .filter(|&t| mesh.indices[t as usize * 3..t as usize * 3 + 3].iter().all(|&v| mesh.positions[v as usize].z > 0.5))
        .collect();
    mesh.hide(&cap);
}

/// One picture: its file name, the look setting, and what else is switched on.
struct Shot {
    name: String,
    spec: String,
    open: bool,
    backfaces: bool,
    flash: bool,
    groups: bool,
    ortho: bool,
}

fn shot(name: &str, spec: &str) -> Shot {
    Shot {
        name: name.to_string(),
        spec: spec.to_string(),
        open: false,
        backfaces: false,
        flash: false,
        groups: false,
        ortho: false,
    }
}

fn shots() -> Vec<Shot> {
    let mut out = vec![
        shot("matcap", "matcap=on"),
        shot("matcap_flat", "matcap=on,flat"),
        shot("lit", "matcap=off"),
        shot("lit_flat", "matcap=off,flat"),
        shot("lit_plain", "matcap=off,ao=off,shadows=off,bloom=off,vignette=off"),
        shot("lit_metallic", "matcap=off,metallic=0.8,smoothness=0.8,normal=0.6"),
        shot("lit_gradient", "matcap=off,gradient,light=dramatic"),
        shot("lit_world_lights", "matcap=off,follow=off,rotation=40"),
        shot("lit_wire", "matcap=off,wire"),
        shot("matcap_wire", "matcap=on,wire"),
    ];
    for p in look::LURES {
        out.push(shot(&format!("lure_{}", p.id), &format!("matcap=off,finish=lure:{}", p.id)));
    }
    for p in look::METALS {
        out.push(shot(&format!("metal_{}", p.id), &format!("matcap=off,finish=metal:{}", p.id)));
    }
    for p in look::CLAYS {
        out.push(shot(&format!("clay_{}", p.id), &format!("matcap=off,finish=clay:{}", p.id)));
    }
    for p in look::WOODS {
        out.push(shot(&format!("wood_{}", p.id), &format!("matcap=off,finish=wood:{}", p.id)));
    }
    for p in look::GLOWS {
        out.push(shot(&format!("glow_{}", p.id), &format!("matcap=off,glow={}", p.id)));
    }
    out.push(shot("glow_matcap", &format!("matcap=on,glow={}", look::GLOWS[2].id)));
    out.push(shot("glow_on_metal", &format!("matcap=off,finish=metal,glow={}", look::GLOWS[3].id)));
    out.push(shot("metal_flat", "matcap=off,finish=metal,flat"));
    out.push(shot("lure_flat", "matcap=off,finish=lure,flat"));
    let open = |name: &str, spec: &str, backfaces: bool| Shot { open: true, backfaces, ..shot(name, spec) };
    out.push(open("open_matcap", "matcap=on", false));
    out.push(open("open_lit", "matcap=off", false));
    out.push(open("open_lure", "matcap=off,finish=lure", false));
    out.push(open("open_matcap_backfaces", "matcap=on", true));
    out.push(open("open_lit_backfaces", "matcap=off", true));
    out.push(open("open_lure_backfaces", "matcap=off,finish=lure", true));
    out.push(Shot { flash: true, ..shot("flash", "matcap=on") });
    out.push(Shot { groups: true, ..shot("groups", "matcap=on") });
    out.push(Shot { groups: true, open: true, ..shot("groups_open", "matcap=off") });
    out.push(Shot { ortho: true, ..shot("ortho_lit", "matcap=off") });
    out.push(Shot { ortho: true, ..shot("ortho_clay", "matcap=off,finish=clay") });
    out
}

/// `sculpt --render-shots <folder>`.
pub fn run_shots(folder: &Path) {
    if let Err(e) = std::fs::create_dir_all(folder) {
        eprintln!("could not make {}: {e}", folder.display());
        return;
    }
    let result = with_gpu(|gpu| {
        for shot in shots() {
            let mut stage = Stage::new(gpu, SHOT_SIZE);
            stage.guides = true;
            let mut mesh = bumpy_sphere(150_000);
            if shot.open {
                open_front(&mut mesh);
            }
            if shot.groups {
                let ids = (0..mesh.triangle_count()).map(|t| (t / 4000 % 6) as u16).collect();
                mesh.set_group_ids(ids);
            }
            stage.meshes.push((1, mesh));
            // The object, and a live mirror copy of it across the plane x = 1.3.
            stage.objects.push((1, Mat4::IDENTITY));
            let mirror = Mat4::from_translation(Vec3::new(2.6, 0.0, 0.0)) * Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0));
            stage.objects.push((1, mirror * Mat4::from_rotation_y(0.6)));
            look::apply_spec(&mut stage.opts, &shot.spec, 2.0);
            set_backfaces(&mut stage.opts, shot.backfaces);
            stage.renderer.groups.enabled = shot.groups;
            stage.camera.ortho = shot.ortho;
            stage.frame_camera(-20.0, -12.0);
            // Closer than framed, so the surface has pixels to compare.
            stage.camera.distance *= 0.6;
            if shot.flash {
                stage.sync(gpu);
                let (globals, _, items) = stage.inputs();
                let pulses: Vec<flash::Item> = items
                    .iter()
                    .map(|i| flash::Item { mesh: i.mesh, model: i.model, color: [0.3, 0.7, 1.0, 0.3] })
                    .collect();
                stage.renderer.flash.show(gpu, &globals.view_proj, &pulses);
            }
            stage.draw(gpu);
            let picture = crate::render::read_texture(gpu, &stage.target);
            let path = folder.join(format!("{}.png", shot.name));
            match picture.save(&path) {
                Ok(()) => println!("{}", path.display()),
                Err(e) => eprintln!("could not write {}: {e}", path.display()),
            }
        }
        for (name, spec) in SWEEPS {
            sweep(gpu, folder, name, spec);
        }
    });
    if let Err(e) = result {
        eprintln!("{e}");
    }
}

/// The roughness sweeps: a plain sphere at smoothness 1 down to 0 in steps
/// of a tenth, side by side in one picture, as metal and as plastic.
const SWEEPS: [(&str, &str); 2] = [("sweep_metal", "matcap=off,metallic=1"), ("sweep_dielectric", "matcap=off,metallic=0")];
const SWEEP_CELL: u32 = 160;

/// One sweep picture, and a line per sphere of how bright it is on average
/// (0-255 over the pixels it covers), so a sweep that loses or gains light
/// as it gets rougher shows in numbers as well as on sight.
fn sweep(gpu: &Gpu, folder: &Path, name: &str, spec: &str) {
    let steps = 11u32;
    let mut sheet = image::RgbaImage::new(SWEEP_CELL * steps, SWEEP_CELL);
    let mut means = Vec::new();
    for step in 0..steps {
        let smoothness = 1.0 - step as f32 / (steps - 1) as f32;
        let mut stage = Stage::new(gpu, (SWEEP_CELL, SWEEP_CELL));
        stage.meshes.push((1, primitives::build(Primitive::Sphere, 40_000, 1.0)));
        stage.objects.push((1, Mat4::IDENTITY));
        look::apply_spec(&mut stage.opts, &format!("{spec},smoothness={smoothness},vignette=off,bloom=off"), 2.0);
        stage.frame_camera(-20.0, -12.0);
        stage.draw(gpu);
        let cell = crate::render::read_texture(gpu, &stage.target);
        // The sphere is what differs from the corner's background.
        let background = *cell.get_pixel(0, 0);
        let (mut sum, mut count) = (0u64, 0u64);
        for px in cell.pixels() {
            if (0..3).any(|c| px[c].abs_diff(background[c]) > 6) {
                sum += (px[0] as u64 + px[1] as u64 + px[2] as u64) / 3;
                count += 1;
            }
        }
        means.push(sum as f64 / count.max(1) as f64);
        image::imageops::replace(&mut sheet, &cell, (step * SWEEP_CELL) as i64, 0);
    }
    let path = folder.join(format!("{name}.png"));
    match sheet.save(&path) {
        Ok(()) => println!("{}", path.display()),
        Err(e) => eprintln!("could not write {}: {e}", path.display()),
    }
    let row: Vec<String> = means.iter().map(|m| format!("{m:.0}")).collect();
    println!("{name} brightness, smoothness 1 to 0: {}", row.join(" "));
}

/// `sculpt --compare-shots <folder a> <folder b>`: per picture, the largest
/// and the mean difference of any colour channel (0-255), and how many
/// pixels differ by more than 2 and by more than 8.
pub fn compare(a: &Path, b: &Path) {
    let Ok(entries) = std::fs::read_dir(a) else {
        eprintln!("cannot read {}", a.display());
        return;
    };
    let mut names: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    names.retain(|p| p.extension().is_some_and(|e| e == "png"));
    names.sort();
    println!("{:<32} {:>5} {:>9} {:>9} {:>9}", "picture", "max", "mean", "px>2", "px>8");
    let (mut worst, mut total_mean, mut count) = (0u8, 0.0f64, 0usize);
    for path in names {
        let name = path.file_name().unwrap_or_default();
        let (Ok(pa), Ok(pb)) = (image::open(&path), image::open(b.join(name))) else {
            println!("{:<32} missing", name.to_string_lossy());
            continue;
        };
        let (pa, pb) = (pa.to_rgb8(), pb.to_rgb8());
        if pa.dimensions() != pb.dimensions() {
            println!("{:<32} sizes differ", name.to_string_lossy());
            continue;
        }
        let (mut max, mut sum, mut over2, mut over8) = (0u8, 0u64, 0usize, 0usize);
        for (x, y) in pa.pixels().zip(pb.pixels()) {
            let d = (0..3).map(|c| x[c].abs_diff(y[c])).max().unwrap_or(0);
            max = max.max(d);
            sum += (0..3).map(|c| x[c].abs_diff(y[c]) as u64).sum::<u64>();
            over2 += (d > 2) as usize;
            over8 += (d > 8) as usize;
        }
        let mean = sum as f64 / (pa.len() as f64);
        println!("{:<32} {max:>5} {mean:>9.4} {over2:>9} {over8:>9}", name.to_string_lossy());
        worst = worst.max(max);
        total_mean += mean;
        count += 1;
    }
    println!("{count} pictures: largest difference {worst}, mean of means {:.4}", total_mean / count.max(1) as f64);
}
