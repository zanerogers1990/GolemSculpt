//! `sculpt --brush-hash`: a fixed run of scripted strokes through the real
//! stroke code, with no window, printing a hash of the mesh after each.
//!
//! It is how a change made only for speed is shown to change nothing else:
//! the run is made by the build before the change and the build after, and
//! every line has to come out the same, to the bit. The strokes cover every
//! brush at two sizes with and without the X mirror, the shipped presets
//! that work a kernel a different way (Clay Strips, the Smooth modes, a
//! dragged print), strokes along the mirror plane, radial symmetry, a mask,
//! hidden faces, sculpt layers, and a mesh dynamic topology has refined,
//! with it on and off.
//!
//! What is hashed is every vertex's position, normal and mask, bit for bit,
//! and what the stroke's undo record says changed (sorted, since the order
//! threads record vertices in is not fixed, and leaving out a vertex whose
//! position before and after is the same bits). Undo and redo are checked
//! to land on the same hashes too.

use glam::{Vec2, Vec3};

use crate::camera::Camera;
use crate::mesh::primitives::{self, Primitive};
use crate::mesh::{Mesh, StrokeDelta};
use crate::sculpt::{Brush, PointerSample, Radial, Sculptor};

const SIZE: Vec2 = Vec2::new(1920.0, 1080.0);

/// FNV-1a, 64 bits: no dependency, and plenty to tell two runs apart.
#[derive(Clone, Copy)]
struct Fnv(u64);

impl Fnv {
    fn new() -> Fnv {
        Fnv(0xcbf2_9ce4_8422_2325)
    }

    fn word(&mut self, w: u32) {
        for b in w.to_le_bytes() {
            self.0 ^= b as u64;
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }

    fn vec(&mut self, v: Vec3) {
        self.word(v.x.to_bits());
        self.word(v.y.to_bits());
        self.word(v.z.to_bits());
    }
}

fn mesh_hash(mesh: &mut Mesh) -> u64 {
    let _ = mesh.take_dirty();
    let mut h = Fnv::new();
    h.word(mesh.vertex_count() as u32);
    h.word(mesh.triangle_count() as u32);
    for &p in &mesh.positions {
        h.vec(p);
    }
    for &n in &mesh.normals {
        h.vec(n);
    }
    for &m in &mesh.mask {
        h.word(m.to_bits());
    }
    for &i in &mesh.indices {
        h.word(i);
    }
    h.0
}

/// What an undo record says changed, as (vertex, before, after) bits.
fn delta_entries(delta: &StrokeDelta, out: &mut Vec<(u32, [u32; 3], [u32; 3])>) {
    let bits = |v: Vec3| [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()];
    match delta {
        StrokeDelta::Positions { verts, before, after, .. } => {
            for ((&v, &b), &a) in verts.iter().zip(before).zip(after) {
                out.push((v, bits(b), bits(a)));
            }
        }
        StrokeDelta::Mask { verts, before, after } => {
            for ((&v, &b), &a) in verts.iter().zip(before).zip(after) {
                out.push((v, [b.to_bits(), 0, 0], [a.to_bits(), 0, 0]));
            }
        }
        StrokeDelta::Layered { stroke, .. } => delta_entries(stroke, out),
        // Paint strokes leave the shape alone; a painted sculpt stroke's shape is hashed.
        StrokeDelta::Painted { shape, .. } => delta_entries(shape, out),
        StrokeDelta::Paint(_) => {}
    }
}

fn delta_hash(delta: &Option<StrokeDelta>) -> (u64, usize) {
    let mut entries = Vec::new();
    if let Some(delta) = delta {
        delta_entries(delta, &mut entries);
    }
    entries.retain(|(_, b, a)| b != a);
    entries.sort_unstable();
    let mut h = Fnv::new();
    for (v, b, a) in &entries {
        h.word(*v);
        b.iter().chain(a).for_each(|&w| h.word(w));
    }
    (h.0, entries.len())
}

/// A path across the screen, `t` from 0 to 1, in pixels.
#[derive(Clone, Copy)]
enum Path {
    /// The benchmark's wavy sweep across the front, over the mirror plane.
    Wave,
    /// Up and down just beside the mirror plane, the dab overlapping its image.
    Along,
    /// Well clear of the mirror plane.
    Aside,
}

impl Path {
    fn at(self, t: f32) -> Vec2 {
        let (cx, cy) = (SIZE.x * 0.5, SIZE.y * 0.5);
        match self {
            Path::Wave => Vec2::new(cx - 180.0 + 360.0 * t, cy + (t * 9.0).sin() * 60.0),
            Path::Along => Vec2::new(cx + 6.0 + (t * 11.0).sin() * 10.0, cy - 160.0 + 320.0 * t),
            Path::Aside => Vec2::new(cx + 150.0 + (t * 7.0).sin() * 30.0, cy - 120.0 + 240.0 * t),
        }
    }
}

struct Run {
    mesh: Mesh,
    camera: Camera,
    all: Fnv,
    failed: usize,
}

impl Run {
    fn new(mesh: Mesh) -> Run {
        let mut camera = Camera::default();
        camera.frame(mesh.bounds(), SIZE.x / SIZE.y, false);
        Run { mesh, camera, all: Fnv::new(), failed: 0 }
    }

    /// One stroke of `sculptor`'s brush along `path`, in `frames` frames of
    /// four samples, then its hashes printed and checked through undo. The
    /// stroke is then undone, so the next starts on the same surface (left
    /// in, strokes pile up into a lump that swallows the camera), unless the
    /// mesh is to `keep` it.
    fn stroke(&mut self, name: &str, sculptor: &mut Sculptor, path: Path, frames: usize, keep: bool) {
        let scene = self.mesh.bounds().size().max_element();
        let view = self.camera.view(Vec2::ZERO, SIZE, self.camera.distance, scene);
        let mesh = &mut self.mesh;
        let before = mesh_hash(mesh);
        let samples = 4;
        let total = frames * samples;
        let mut time = 0.0f64;
        let sample = |i: usize, time: f64| PointerSample { pos: path.at(i as f32 / total as f32), pressure: None, time };
        let began = sculptor.begin(mesh, &view, sample(0, time), false, false);
        if began {
            for f in 0..frames {
                for s in 0..samples {
                    time += 1.0 / 60.0 / samples as f64;
                    sculptor.sample(mesh, &view, sample(f * samples + s + 1, time));
                }
                sculptor.frame(mesh, &view, 1.0 / 60.0);
                let _ = mesh.take_dirty();
            }
        }
        let delta = if began { sculptor.end(mesh, &view) } else { None };
        let after = mesh_hash(mesh);
        let (changes, changed) = delta_hash(&delta);
        let mut round_trip = "ok";
        if let Some(delta) = &delta {
            mesh.apply_delta(delta, true);
            let undone = mesh_hash(mesh);
            mesh.apply_delta(delta, false);
            let redone = mesh_hash(mesh);
            if undone != before || redone != after {
                round_trip = "UNDO MISMATCH";
                self.failed += 1;
            }
            if !keep {
                mesh.apply_delta(delta, true);
            }
        }
        println!("{name:<44} mesh {after:016x}  delta {changes:016x} ({changed:>6} verts)  undo {round_trip}");
        self.all.0 ^= after;
        self.all.0 = self.all.0.wrapping_mul(0x0100_0000_01b3) ^ changes;
    }
}

/// A sculptor holding `brush` at `radius_px`, with the X mirror if `mirror`.
fn holding(brush: Brush, radius_px: f32, mirror: bool) -> Sculptor {
    let mut sculptor = Sculptor::default();
    sculptor.settings.brush = brush;
    sculptor.settings.radius_px = radius_px;
    sculptor.settings.symmetry = [mirror, false, false];
    sculptor.settings.per_brush[brush.index()].strength = 0.5;
    sculptor
}

/// A sculptor holding the library brush called `name`.
fn preset(name: &str, radius_px: f32, mirror: bool) -> Sculptor {
    let mut sculptor = Sculptor::default();
    let found = sculptor.library.select_named(name, &mut sculptor.settings);
    assert!(found, "no brush called {name}");
    sculptor.settings.radius_px = radius_px;
    sculptor.settings.symmetry = [mirror, false, false];
    sculptor
}

fn radial(mut sculptor: Sculptor) -> Sculptor {
    sculptor.settings.radial = Radial { on: true, count: 6, ..Radial::default() };
    sculptor
}

/// Every brush, both sizes, with and without the mirror, then the shipped
/// presets that work a kernel differently, along and beside the plane.
fn every_brush(run: &mut Run) {
    for brush in Brush::ALL {
        for radius in [40.0, 150.0] {
            for mirror in [false, true] {
                let name = format!("{} r{radius} {}", brush.name(), if mirror { "mirror" } else { "plain" });
                run.stroke(&name, &mut holding(brush, radius, mirror), Path::Wave, 30, false);
            }
        }
    }
    for name in ["Clay Strips", "Clay Tubes", "Smooth Peaks", "Smooth Valleys", "Relax", "Keep Volume", "Scales"] {
        for (path, label) in [(Path::Along, "along"), (Path::Aside, "aside")] {
            run.stroke(&format!("{name} {label} mirror"), &mut preset(name, 90.0, true), path, 30, false);
        }
    }
    for name in ["Trim", "Planar", "Polish"] {
        run.stroke(&format!("{name} along mirror"), &mut preset(name, 90.0, true), Path::Along, 30, false);
    }
    // Dam Standard, the Crease preset: beside and along the plane, and both
    // sizes across the front with and without the mirror.
    for (path, label) in [(Path::Along, "along"), (Path::Aside, "aside")] {
        run.stroke(&format!("Dam Standard {label} mirror"), &mut preset("Dam Standard", 90.0, true), path, 30, false);
    }
    for radius in [40.0, 150.0] {
        for mirror in [false, true] {
            let name = format!("Dam Standard r{radius} {}", if mirror { "mirror" } else { "plain" });
            run.stroke(&name, &mut preset("Dam Standard", radius, mirror), Path::Wave, 30, false);
        }
    }
    for brush in [Brush::Clay, Brush::Smooth, Brush::Standard, Brush::Layer, Brush::Move, Brush::SnakeHook, Brush::Inflate] {
        run.stroke(&format!("{} along mirror", brush.name()), &mut holding(brush, 90.0, true), Path::Along, 30, false);
    }
}

/// Radial symmetry, with and without the mirror.
fn radial_strokes(run: &mut Run) {
    for brush in [Brush::Clay, Brush::Smooth, Brush::Standard, Brush::Inflate, Brush::SnakeHook, Brush::Move, Brush::Flatten] {
        for mirror in [false, true] {
            let name = format!("{} radial6 {}", brush.name(), if mirror { "mirror" } else { "plain" });
            run.stroke(&name, &mut radial(holding(brush, 70.0, mirror)), Path::Wave, 24, false);
        }
    }
    run.stroke("Clay Strips radial6 mirror", &mut radial(preset("Clay Strips", 70.0, true)), Path::Wave, 24, false);
    run.stroke("Relax radial6", &mut radial(preset("Relax", 70.0, false)), Path::Wave, 24, false);
}

/// A painted mask and one set as stripes, then brushes over it.
fn masked(run: &mut Run) {
    let mut mask = holding(Brush::Clay, 60.0, true);
    mask.settings.mask_mode = true;
    run.stroke("mask paint mirror", &mut mask, Path::Aside, 20, true);
    let mesh = &mut run.mesh;
    for (m, p) in mesh.mask.iter_mut().zip(&mesh.positions) {
        if p.y > 0.0 {
            *m = (*m).max(((p.x * 31.0).sin() * 0.5 + 0.5).clamp(0.0, 1.0));
        }
    }
    let all: Vec<u32> = (0..mesh.bvh.leaves.len() as u32).collect();
    mesh.mask_changed(&all);
    for brush in [Brush::Clay, Brush::Smooth, Brush::Inflate, Brush::Move, Brush::Standard, Brush::Flatten] {
        run.stroke(&format!("{} masked mirror", brush.name()), &mut holding(brush, 110.0, true), Path::Wave, 24, false);
    }
    run.stroke("Keep Volume masked", &mut preset("Keep Volume", 110.0, true), Path::Wave, 24, false);
}

/// A band of faces hidden across the stroke, then brushes over it.
fn hidden(run: &mut Run) {
    let mesh = &mut run.mesh;
    let hide: Vec<u32> = (0..mesh.triangle_count() as u32)
        .filter(|&t| {
            let i = t as usize * 3;
            let c = (mesh.positions[mesh.indices[i] as usize]
                + mesh.positions[mesh.indices[i + 1] as usize]
                + mesh.positions[mesh.indices[i + 2] as usize])
                / 3.0;
            c.y.abs() < 0.04 && c.x > -0.15
        })
        .collect();
    let _ = mesh.hide(&hide);
    for brush in [Brush::Clay, Brush::Smooth, Brush::Standard, Brush::Move, Brush::Flatten, Brush::SnakeHook, Brush::Nudge] {
        run.stroke(&format!("{} hidden mirror", brush.name()), &mut holding(brush, 110.0, true), Path::Wave, 24, false);
    }
    for name in ["Relax", "Keep Volume", "Clay Strips"] {
        run.stroke(&format!("{name} hidden mirror"), &mut preset(name, 110.0, true), Path::Wave, 24, false);
    }
    let _ = run.mesh.show_all();
}

/// Sculpt layers: strokes recorded into one, and Erase Layer rubbing it out.
fn layered(run: &mut Run) {
    let _ = run.mesh.add_layer(Some("Hash".into()));
    for brush in [Brush::Clay, Brush::Standard, Brush::Smooth, Brush::Inflate] {
        run.stroke(&format!("{} layer mirror", brush.name()), &mut holding(brush, 100.0, true), Path::Wave, 24, true);
    }
    run.stroke("Erase Layer layer mirror", &mut holding(Brush::EraseLayer, 100.0, true), Path::Wave, 24, true);
}

/// Dynamic topology refining a coarse ball, then brushes on what it made,
/// with it on and off.
fn refined(run: &mut Run) {
    let on = |mut sculptor: Sculptor| {
        sculptor.dyntopo.enabled = true;
        sculptor
    };
    for brush in [Brush::Clay, Brush::Standard, Brush::SnakeHook, Brush::Smooth, Brush::Inflate] {
        run.stroke(&format!("{} dyntopo mirror", brush.name()), &mut on(holding(brush, 90.0, true)), Path::Wave, 24, true);
    }
    run.stroke("Clay Strips dyntopo mirror", &mut on(preset("Clay Strips", 90.0, true)), Path::Along, 24, true);
    run.stroke("Clay dyntopo radial6", &mut on(radial(holding(Brush::Clay, 70.0, false))), Path::Wave, 20, true);
    for brush in [Brush::Clay, Brush::Smooth, Brush::Move, Brush::SnakeHook, Brush::Flatten, Brush::Layer] {
        run.stroke(&format!("{} refined mirror", brush.name()), &mut holding(brush, 90.0, true), Path::Wave, 24, true);
    }
    for name in ["Relax", "Keep Volume", "Smooth Peaks", "Smooth Valleys", "Clay Strips"] {
        run.stroke(&format!("{name} refined mirror"), &mut preset(name, 90.0, true), Path::Along, 24, true);
    }
    run.stroke("Dam Standard refined mirror", &mut preset("Dam Standard", 90.0, true), Path::Wave, 24, true);
}

/// Clay Strips and Dam Standard on a coarse ball of their own, refined by
/// dynamic topology and then stroked again: the strokes in [`refined`] ride
/// on whatever the brushes before them left, so these show the two brushes
/// themselves are unchanged when another brush has changed.
fn refined_presets(run: &mut Run) {
    let mut strips = preset("Clay Strips", 90.0, true);
    strips.dyntopo.enabled = true;
    run.stroke("Clay Strips dyntopo mirror (own ball)", &mut strips, Path::Along, 24, true);
    let mut dam = preset("Dam Standard", 90.0, true);
    dam.dyntopo.enabled = true;
    run.stroke("Dam Standard dyntopo mirror (own ball)", &mut dam, Path::Wave, 24, true);
    run.stroke("Clay Strips refined mirror (own ball)", &mut preset("Clay Strips", 90.0, true), Path::Wave, 24, true);
    run.stroke("Dam Standard refined mirror (own ball)", &mut preset("Dam Standard", 90.0, true), Path::Along, 24, true);
}

/// `tris` is the size of the main ball (300,000 triangles unless told).
pub fn run(tris: &[usize]) {
    let tris = tris.first().copied().unwrap_or(300_000);
    println!("threads: {}", rayon::current_num_threads());
    let mut run = Run::new(primitives::build(Primitive::Sphere, tris, 0.5));
    println!("ball: {} triangles, {} vertices, start {:016x}", run.mesh.triangle_count(), run.mesh.vertex_count(), mesh_hash(&mut run.mesh));
    every_brush(&mut run);
    radial_strokes(&mut run);
    masked(&mut run);
    hidden(&mut run);
    layered(&mut run);
    let mut coarse = Run::new(primitives::build(Primitive::Sphere, 20_000, 0.5));
    refined(&mut coarse);
    println!(
        "coarse ball after dynamic topology: {} triangles, {} vertices",
        coarse.mesh.triangle_count(),
        coarse.mesh.vertex_count()
    );
    let mut own = Run::new(primitives::build(Primitive::Sphere, 20_000, 0.5));
    refined_presets(&mut own);
    let (all, failed) = (run.all.0 ^ coarse.all.0.rotate_left(17) ^ own.all.0.rotate_left(29), run.failed + coarse.failed + own.failed);
    println!("ALL {all:016x}  undo mismatches {failed}");
}
