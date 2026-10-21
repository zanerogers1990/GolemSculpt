//! `sculpt --brush-feel`: how each brush feels, as numbers.
//!
//! The brushes are judged by hand with the pen, but a few things about a
//! stroke can be measured, and two builds compared by them: how tall one
//! pass along a line builds and how tall three passes over it do, how
//! sharp the ridge it leaves is, how good the triangles under it stay, how
//! much the stroke adds to or takes from the form, and how far the far side
//! of a thin fin moves when only its near side is stroked.
//!
//! Every stroke is drawn through the real stroke code with the pen (a
//! pressure on every sample), on a ball of about 200,000 triangles with the
//! brush a fifth of the ball's radius, along a line across its front. Each
//! brush is drawn at Strength 10%, 50% and 100% at full pressure, at its own
//! Strength, and at its own Strength with the lightest of touches (5%
//! pressure). Every stroke is undone before the next, so each starts on the
//! same surface.
//!
//! The columns, all lengths in brush radii:
//!  - `out1`, `out3`: the most any vertex rose along its own normal, after
//!    one pass and after three overlapping passes (there, back, there) in
//!    one stroke;
//!  - `in1`: the most any sank in one pass;
//!  - `move1`: the most any moved at all in one pass;
//!  - `ridge`: the sharpest the surface curves under the stroke after one
//!    pass, as curvature times radius (the ball itself is 0.2), the largest
//!    and the 99th percentile;
//!  - `angle`: the triangles under the stroke, the mean and the worst of
//!    their smallest angles in degrees (the ball starts near 45 and 35);
//!  - `edgeCV`: how uneven their edges are, the spread over the mean;
//!  - `dV`: the change in the whole mesh's volume, in cubic radii;
//!  - `fin`: the most any vertex on the back of a thin slab (0.4 radii
//!    thick) moved when its front was stroked once;
//!  - `rough`: for the brushes that smooth, how much of a fine noise laid
//!    over the ball is left under the stroke after one pass (1 is all of it);
//!  - `out3s`: the most any vertex rose after three strokes along the line,
//!    one after another (against `out3`, three passes in one stroke: what a
//!    brush that builds to a height per stroke does);
//!  - `ridge3`, `angle3`: `ridge` and `angle` after the three passes in one
//!    stroke (a stroke gone over again should stay a rounded bead, and its
//!    triangles whole);
//!  - `ribs`: for the brushes that work to a plane (Clay, Flatten and the
//!    brushes made of them), how much of a row of ribs across the stroke's
//!    path (a ripple half a radius high from crest to trough, a radius and a
//!    half from one crest to the next) still shows along the middle of the
//!    stroke after one pass: 1 is all of it. A plane that rocks over every
//!    rib leaves more of them;
//!  - `crest`, `crest3`: how sharply the top of what one pass and the three
//!    passes raised curves, as curvature times radius over the vertices that
//!    rose nine tenths as far as the most (low on a rounded bead, high on a
//!    peaked ridge; the ball's own is 0.2).
//!
//!  - `lumps`: for the brushes that smooth, how much of a lumpy ball's lumps
//!    (a third of a radius across) is left along the stroke after one pass;
//!  - `dV10`, `rough10`, `lumps10`: for the Smooth brushes, after ten strokes
//!    one after another: the change in the ball's volume, and what is left
//!    of the fine noise and of the lumps.
//!
//! Move and Snake Hook are a drag of two radii to the side instead of a
//! line, and `move1` is then how far the point under the pen went, as a
//! share of the drag. Snake Hook is also pulled straight out of the ball's
//! outline, four radii on the screen (`pull:`): how far past the outline it
//! reached (`reach`), how thick the pull is half, one and a half and two and
//! a half radii out (`thick`, root mean square off its middle, in radii), and
//! the triangles it pulled out (`angleP`, `edgeCVP`) and the volume (`dVP`).
//!
//! `SCULPT_FEEL_ROWS` (row names, comma separated) runs only those rows, and
//! then also the tuning rows the starting values were picked from.

use glam::{DVec3, Vec2, Vec3};

use crate::camera::{Camera, View};
use crate::mesh::primitives::{self, Primitive};
use crate::mesh::{Mesh, StrokeDelta};
use crate::sculpt::{Brush, BrushSettings, PointerSample, Sculptor, Settings, SmoothMode};

const SIZE: Vec2 = Vec2::new(1920.0, 1080.0);

/// The brush's radius on the model: a fifth of the ball's.
const RADIUS: f32 = 0.1;
const BALL: f32 = 0.5;

/// How far the stroke runs either side of the middle, in radii.
const HALF_LENGTH: f32 = 1.5;

/// Samples a pass is drawn with, four to a frame at 60 frames a second.
const SAMPLES: usize = 96;

/// The lightest touch the light rows are drawn with.
const LIGHT: f32 = 0.05;

struct Target {
    mesh: Mesh,
    view: View,
    /// The line strokes are drawn along, on the screen.
    line: Vec<Vec2>,
    /// The same, on the surface: what "under the stroke" is measured from.
    path: Vec<Vec3>,
}

impl Target {
    fn new(mut mesh: Mesh, surface: impl Fn(f32) -> Vec3) -> Target {
        let mut camera = Camera::default();
        camera.frame(mesh.bounds(), SIZE.x / SIZE.y, false);
        let scene = mesh.bounds().size().max_element();
        let view = camera.view(Vec2::ZERO, SIZE, camera.distance, scene);
        let path: Vec<Vec3> = (0..=SAMPLES).map(|i| surface(i as f32 / SAMPLES as f32)).collect();
        let line = path.iter().map(|&p| view.project(p).expect("the path is in view")).collect();
        let _ = mesh.take_dirty();
        Target { mesh, view, line, path }
    }

    fn ball(mesh: Mesh) -> Target {
        Target::new(mesh, |t| {
            let x = (2.0 * t - 1.0) * HALF_LENGTH * RADIUS;
            Vec3::new(x, 0.0, (BALL * BALL - x * x).sqrt())
        })
    }
}

/// A ball, smooth or with fine noise over it.
fn ball(rough: bool) -> Mesh {
    let mesh = primitives::build(Primitive::Sphere, 200_000, BALL);
    if !rough {
        return mesh;
    }
    let positions = mesh
        .positions
        .iter()
        .enumerate()
        .map(|(i, &p)| {
            let h = (i as u32).wrapping_mul(2_654_435_761) >> 8 & 0xffff;
            p * (1.0 + 0.01 * (h as f32 / 65535.0 - 0.5))
        })
        .collect();
    Mesh::new(positions, mesh.indices.clone(), None)
}

/// A ball with lumps over it the size of the brush's middle (bumps a third
/// of a radius across, a hundredth of the ball's radius high): what a
/// smoothing brush is mostly for.
fn lumpy() -> Mesh {
    let mesh = primitives::build(Primitive::Sphere, 200_000, BALL);
    let positions = mesh.positions.iter().map(|&p| p * (1.0 + 0.012 * (p.x * 90.0).sin() * (p.y * 90.0).sin())).collect();
    Mesh::new(positions, mesh.indices.clone(), None)
}

/// How lumpy the surface is along the stroke: the root mean square of how
/// far the vertices within half a radius of the path stand off the ball.
fn lumpiness(mesh: &Mesh, path: &[Vec3]) -> f64 {
    let (mut sum, mut n) = (0.0f64, 0.0f64);
    for &p in &mesh.positions {
        if p.z > 0.0 && p.x.abs() < 0.8 * HALF_LENGTH * RADIUS && off_path(path, p.normalize() * BALL) < 0.5 * RADIUS {
            sum += (p.length() as f64 - BALL as f64).powi(2);
            n += 1.0;
        }
    }
    (sum / n.max(1.0)).sqrt()
}

/// How far apart the ribs of the ribbed ball are, from crest to crest.
const RIB: f32 = 1.5 * RADIUS;

/// A ball with a row of ribs across the stroke's path: its surface rippled
/// in and out along x, a quarter of a radius each way.
fn ribbed() -> Mesh {
    let mesh = primitives::build(Primitive::Sphere, 200_000, BALL);
    let positions = mesh
        .positions
        .iter()
        .map(|&p| p * (1.0 + 0.025 * RADIUS / BALL * (std::f32::consts::TAU * p.x / RIB).sin()))
        .collect();
    Mesh::new(positions, mesh.indices.clone(), None)
}

/// How tall the ribs stand along the middle of the stroke: the ripple at
/// the ribs' own spacing in how far the surface is from the ball's centre,
/// over the vertices within a fifth of a radius of the path and away from
/// its ends.
fn rib_height(mesh: &Mesh, path: &[Vec3]) -> f64 {
    let k = std::f64::consts::TAU / RIB as f64;
    let (mut s, mut c, mut n) = (0.0f64, 0.0f64, 0.0f64);
    let mut heights = Vec::new();
    for &p in &mesh.positions {
        // Measured on the ball, so that what a stroke raised is still under it.
        if p.z > 0.0 && p.x.abs() < 0.8 * HALF_LENGTH * RADIUS && off_path(path, p.normalize() * BALL) < 0.2 * RADIUS {
            heights.push((p.x as f64, p.length() as f64));
        }
    }
    let mean = heights.iter().map(|h| h.1).sum::<f64>() / heights.len().max(1) as f64;
    for &(x, h) in &heights {
        s += (h - mean) * (k * x).sin();
        c += (h - mean) * (k * x).cos();
        n += 1.0;
    }
    2.0 * (s * s + c * c).sqrt() / n.max(1.0)
}

/// A slab 0.4 radii thick, seen face on.
fn fin() -> Target {
    let mesh = primitives::build(Primitive::Cube, 200_000, BALL);
    let thick = 0.4 * RADIUS;
    let positions = mesh.positions.iter().map(|&p| Vec3::new(p.x, p.y, p.z * thick / BALL)).collect();
    let mesh = Mesh::new(positions, mesh.indices.clone(), None);
    Target::new(mesh, |t| Vec3::new((2.0 * t - 1.0) * HALF_LENGTH * RADIUS, 0.0, 0.5 * thick))
}

/// Draws one stroke through `points` with the pen at `pressure`, and gives
/// back its undo record.
fn draw(target: &mut Target, sculptor: &mut Sculptor, points: &[Vec2], pressure: f32) -> Option<StrokeDelta> {
    let (mesh, view) = (&mut target.mesh, &target.view);
    let sample = |i: usize| PointerSample { pos: points[i], pressure: Some(pressure), time: i as f64 / 240.0 };
    if !sculptor.begin(mesh, view, sample(0), false, false) {
        return None;
    }
    for i in 1..points.len() {
        sculptor.sample(mesh, view, sample(i));
        if i % 4 == 0 {
            sculptor.frame(mesh, view, 1.0 / 60.0);
            let _ = mesh.take_dirty();
        }
    }
    let delta = sculptor.end(mesh, view);
    let _ = mesh.take_dirty();
    mesh.flush_normals();
    delta
}

fn undo(target: &mut Target, delta: Option<StrokeDelta>) {
    if let Some(delta) = delta {
        target.mesh.apply_delta(&delta, true);
        let _ = target.mesh.take_dirty();
        target.mesh.flush_normals();
    }
}

fn volume(mesh: &Mesh) -> f64 {
    mesh.indices
        .chunks_exact(3)
        .map(|t| {
            let [a, b, c] = [0, 1, 2].map(|k| mesh.positions[t[k] as usize].as_dvec3());
            a.dot(b.cross(c)) / 6.0
        })
        .sum()
}

/// Distance from `p` to the polyline `path`.
fn off_path(path: &[Vec3], p: Vec3) -> f32 {
    path.windows(2)
        .map(|w| {
            let (a, b) = (w[0], w[1]);
            let ab = b - a;
            let t = ((p - a).dot(ab) / ab.length_squared().max(1e-20)).clamp(0.0, 1.0);
            p.distance(a + ab * t)
        })
        .fold(f32::INFINITY, f32::min)
}

/// The mesh as a stroke found it, for measuring what the stroke did.
struct Before {
    positions: Vec<Vec3>,
    normals: Vec<Vec3>,
    volume: f64,
    /// Vertices within a radius of the stroke's path, and the triangles with
    /// all three corners among them.
    under: Vec<bool>,
    tris: Vec<[u32; 3]>,
}

impl Before {
    fn of(target: &Target) -> Before {
        let mesh = &target.mesh;
        let under: Vec<bool> = mesh.positions.iter().map(|&p| off_path(&target.path, p) < RADIUS).collect();
        let tris = mesh
            .indices
            .chunks_exact(3)
            .map(|t| [t[0], t[1], t[2]])
            .filter(|t| t.iter().all(|&v| under[v as usize]))
            .collect();
        Before { positions: mesh.positions.clone(), normals: mesh.normals.clone(), volume: volume(mesh), under, tris }
    }
}

/// Curvature at `v` times the radius: 1/R on a ball of radius R, positive
/// where the surface bulges out.
fn curvature(mesh: &Mesh, v: u32) -> Option<f32> {
    let ring = mesh.neighbours(v);
    if ring.is_empty() {
        return None;
    }
    let p = mesh.positions[v as usize].as_dvec3();
    let (mut sum, mut len2) = (DVec3::ZERO, 0.0f64);
    for &o in ring {
        let q = mesh.positions[o as usize].as_dvec3();
        sum += q;
        len2 += (q - p).length_squared();
    }
    let n = ring.len() as f64;
    let mean = sum / n;
    let k = -2.0 * (mean - p).dot(mesh.normals[v as usize].as_dvec3()) / (len2 / n).max(1e-30);
    Some((k * RADIUS as f64) as f32)
}

/// The smallest angle of a triangle, in degrees.
fn min_angle(a: Vec3, b: Vec3, c: Vec3) -> f32 {
    let angle = |p: Vec3, q: Vec3, r: Vec3| (q - p).angle_between(r - p).to_degrees();
    angle(a, b, c).min(angle(b, c, a)).min(angle(c, a, b))
}

/// Mean and worst smallest angle, and the edges' spread over their mean,
/// of the triangles under the stroke.
fn quality(positions: &[Vec3], tris: &[[u32; 3]]) -> (f32, f32, f32) {
    let (mut sum, mut worst) = (0.0f64, f32::INFINITY);
    let (mut e1, mut e2, mut en) = (0.0f64, 0.0f64, 0.0f64);
    for t in tris {
        let [a, b, c] = t.map(|v| positions[v as usize]);
        let m = min_angle(a, b, c);
        sum += m as f64;
        worst = worst.min(m);
        for (p, q) in [(a, b), (b, c), (c, a)] {
            let l = p.distance(q) as f64;
            (e1, e2, en) = (e1 + l, e2 + l * l, en + 1.0);
        }
    }
    let n = tris.len().max(1) as f64;
    let mean = e1 / en.max(1.0);
    let spread = ((e2 / en.max(1.0) - mean * mean).max(0.0)).sqrt() / mean.max(1e-30);
    ((sum / n) as f32, worst, spread as f32)
}

/// Root mean square of how far each vertex under the stroke sits from the
/// middle of its neighbours: what fine noise there is.
fn roughness(mesh: &Mesh, under: &[bool]) -> f64 {
    let (mut sum, mut n) = (0.0f64, 0.0f64);
    for (v, _) in under.iter().enumerate().filter(|(_, u)| **u) {
        let ring = mesh.neighbours(v as u32);
        if ring.is_empty() {
            continue;
        }
        let mean: DVec3 = ring.iter().map(|&o| mesh.positions[o as usize].as_dvec3()).sum::<DVec3>() / ring.len() as f64;
        let n_v = mesh.normals[v].as_dvec3();
        sum += ((mean - mesh.positions[v].as_dvec3()).dot(n_v)).powi(2);
        n += 1.0;
    }
    (sum / n.max(1.0)).sqrt()
}

/// What one pass did, measured against the mesh as it was.
struct Pass {
    out: f32,
    crest: f32,
    sank: f32,
    moved: f32,
    ridge: (f32, f32),
    angle: (f32, f32),
    edges: f32,
    dv: f32,
}

fn measure(target: &Target, before: &Before) -> Pass {
    let mesh = &target.mesh;
    let (mut out, mut sank, mut moved) = (0.0f32, 0.0f32, 0.0f32);
    for ((&p, &p0), &n0) in mesh.positions.iter().zip(&before.positions).zip(&before.normals) {
        let d = p - p0;
        out = out.max(d.dot(n0));
        sank = sank.max(-d.dot(n0));
        moved = moved.max(d.length());
    }
    // The top of what rose: the vertices under the stroke that rose nine
    // tenths as far as the most, and how sharply the surface curves there.
    let (mut top, mut tops) = (0.0f64, 0usize);
    if out > 1e-6 {
        for v in 0..mesh.vertex_count() {
            let rise = (mesh.positions[v] - before.positions[v]).dot(before.normals[v]);
            if before.under[v] && rise >= 0.9 * out {
                if let Some(k) = curvature(mesh, v as u32) {
                    (top, tops) = (top + k as f64, tops + 1);
                }
            }
        }
    }
    let crest = if tops > 0 { (top / tops as f64) as f32 } else { 0.0 };
    let mut ks: Vec<f32> = (0..mesh.vertex_count() as u32)
        .filter(|&v| before.under[v as usize])
        .filter_map(|v| curvature(mesh, v))
        .collect();
    ks.sort_by(f32::total_cmp);
    let ridge = (ks.last().copied().unwrap_or(0.0), ks.get(ks.len() * 99 / 100).copied().unwrap_or(0.0));
    let (mean, worst, edges) = quality(&mesh.positions, &before.tris);
    let r3 = (RADIUS as f64).powi(3);
    Pass {
        out: out / RADIUS,
        crest,
        sank: sank / RADIUS,
        moved: moved / RADIUS,
        ridge,
        angle: (mean, worst),
        edges,
        dv: ((volume(mesh) - before.volume) / r3) as f32,
    }
}

/// How a row of the table holds its brush.
#[derive(Clone, Copy)]
enum Held {
    Base(Brush),
    Preset(&'static str),
    /// A base brush with something of its settings changed, under a name of its own.
    Tuned(&'static str, Brush, fn(&mut BrushSettings)),
    /// The same with something of the kernel's own numbers changed too (Clay Depth).
    Set(&'static str, Brush, fn(&mut Settings)),
    /// A library brush with something of its settings changed.
    PresetTuned(&'static str, &'static str, fn(&mut BrushSettings)),
}

impl Held {
    fn name(self) -> &'static str {
        match self {
            Held::Base(brush) => brush.name(),
            Held::Preset(name) | Held::Tuned(name, ..) | Held::Set(name, ..) | Held::PresetTuned(name, ..) => name,
        }
    }

    /// The kernel the row's brush is over.
    fn kernel(self) -> Brush {
        match self {
            Held::Base(brush) | Held::Tuned(_, brush, _) | Held::Set(_, brush, _) => brush,
            Held::Preset(name) | Held::PresetTuned(_, name, _) => {
                let mut sculptor = Sculptor::default();
                assert!(sculptor.library.select_named(name, &mut sculptor.settings), "no brush {name}");
                sculptor.settings.brush
            }
        }
    }

    fn sculptor(self, strength: Option<f32>) -> Sculptor {
        let mut sculptor = Sculptor::default();
        match self {
            Held::Base(brush) => sculptor.settings.brush = brush,
            Held::Preset(name) => assert!(sculptor.library.select_named(name, &mut sculptor.settings), "no brush {name}"),
            Held::Tuned(_, brush, tune) => {
                sculptor.settings.brush = brush;
                tune(sculptor.settings.current_mut());
            }
            Held::Set(_, brush, tune) => {
                sculptor.settings.brush = brush;
                tune(&mut sculptor.settings);
            }
            Held::PresetTuned(_, name, tune) => {
                assert!(sculptor.library.select_named(name, &mut sculptor.settings), "no brush {name}");
                tune(sculptor.settings.current_mut());
            }
        }
        sculptor.settings.screen_space_size = false;
        sculptor.settings.radius_world = RADIUS;
        if let Some(strength) = strength {
            sculptor.settings.current_mut().strength = strength;
        }
        sculptor
    }

    /// Whether the brush is dragged (Move, Snake Hook) rather than laid along a line.
    fn drags(self) -> bool {
        matches!(self.kernel(), Brush::Move | Brush::SnakeHook)
    }

    /// Whether the brush works to a plane: Clay, Flatten and what is made of them.
    fn levels(self) -> bool {
        matches!(self.kernel(), Brush::Clay | Brush::Flatten)
    }

    fn smooths(self) -> bool {
        matches!(self.kernel(), Brush::Smooth | Brush::Flatten)
    }
}

/// Whether the row called `name` is to be run: all of them, unless
/// `SCULPT_FEEL_ROWS` names some (comma separated), for tuning a few.
fn wanted(name: &str) -> bool {
    match std::env::var("SCULPT_FEEL_ROWS") {
        Ok(rows) if !rows.trim().is_empty() => rows.split(',').any(|row| row.trim() == name),
        _ => true,
    }
}

/// Snake Hook pulled straight out of the ball's outline, four radii, on the
/// screen: how far the pull reached past the outline (`reach`, in radii),
/// how thick it is a quarter and three quarters of the way out (`thick`,
/// the root mean square of how far its vertices are from its middle there,
/// in radii), and how good the triangles that were pulled out stay.
fn pull(bench: &mut Bench, held: Held, strength: Option<f32>, pressure: f32) -> String {
    let target = &mut bench.ball;
    let x0 = 0.96 * BALL;
    let from = Vec3::new(x0, 0.0, (BALL * BALL - x0 * x0).sqrt());
    let (a, b) = (target.view.project(from).unwrap(), target.view.project(from + Vec3::X * 4.0 * RADIUS).unwrap());
    let points: Vec<Vec2> = (0..=SAMPLES).map(|i| a.lerp(b, i as f32 / SAMPLES as f32)).collect();
    let delta = draw(target, &mut held.sculptor(strength), &points, pressure);
    let mesh = &target.mesh;
    let before = &bench.ball_before;
    let reach = mesh.positions.iter().map(|p| p.x).fold(BALL, f32::max) - BALL;
    // Measured across slices of the pull, half, one and a half and two and a
    // half radii out from the outline, over the vertices it carried out
    // there; "-" where the pull did not reach or is down to a few vertices.
    let thick_at = |out: f32| {
        let x = BALL + out * RADIUS;
        let slice: Vec<Vec3> = mesh.positions.iter().copied().filter(|p| (p.x - x).abs() < 0.15 * RADIUS).collect();
        if slice.len() < 6 || reach < (out + 0.2) * RADIUS {
            return "    -".to_owned();
        }
        let mid = slice.iter().map(|p| Vec2::new(p.y, p.z)).sum::<Vec2>() / slice.len() as f32;
        let ms = slice.iter().map(|p| Vec2::new(p.y, p.z).distance_squared(mid)).sum::<f32>() / slice.len() as f32;
        format!("{:>5.3}", ms.sqrt() / RADIUS)
    };
    let thick = [0.5, 1.5, 2.5].map(thick_at).join("/");
    let pulled: Vec<bool> =
        mesh.positions.iter().zip(&before.positions).map(|(p, p0)| p.distance(*p0) > 0.1 * RADIUS).collect();
    let tris: Vec<[u32; 3]> = mesh
        .indices
        .chunks_exact(3)
        .map(|t| [t[0], t[1], t[2]])
        .filter(|t| t.iter().all(|&v| pulled[v as usize]))
        .collect();
    let (mean, worst, edges) = quality(&mesh.positions, &tris);
    let dv = ((volume(mesh) - before.volume) / (RADIUS as f64).powi(3)) as f32;
    undo(target, delta);
    format!(
        "  pull: reach {:>5.2}  thick {thick}  angleP {mean:>5.1}/{worst:>5.1}  edgeCVP {edges:>5.3}  dVP {dv:>8.4}",
        reach / RADIUS
    )
}

/// Ten strokes of a smoothing brush along the line, one after another: how
/// much they took from the ball's volume, in cubic radii, and how much of
/// the fine noise of the rough ball is left under them.
fn ten(bench: &mut Bench, held: Held, strength: Option<f32>, pressure: f32) -> String {
    let line = bench.ball.line.clone();
    let strokes: Vec<Option<StrokeDelta>> =
        (0..10).map(|_| draw(&mut bench.ball, &mut held.sculptor(strength), &line, pressure)).collect();
    let dv = ((volume(&bench.ball.mesh) - bench.ball_before.volume) / (RADIUS as f64).powi(3)) as f32;
    for delta in strokes.into_iter().rev() {
        undo(&mut bench.ball, delta);
    }
    let line = bench.rough.line.clone();
    let strokes: Vec<Option<StrokeDelta>> =
        (0..10).map(|_| draw(&mut bench.rough, &mut held.sculptor(strength), &line, pressure)).collect();
    let left = roughness(&bench.rough.mesh, &bench.rough_before.under) / bench.rough_start;
    for delta in strokes.into_iter().rev() {
        undo(&mut bench.rough, delta);
    }
    let line = bench.lumps.line.clone();
    let strokes: Vec<Option<StrokeDelta>> =
        (0..10).map(|_| draw(&mut bench.lumps, &mut held.sculptor(strength), &line, pressure)).collect();
    let lumps = lumpiness(&bench.lumps.mesh, &bench.lumps.path) / bench.lumps_start;
    for delta in strokes.into_iter().rev() {
        undo(&mut bench.lumps, delta);
    }
    format!("  dV10 {dv:>8.4}  rough10 {left:>5.3}  lumps10 {lumps:>5.3}")
}

/// The drag Move and Snake Hook are given: from the middle of the line two
/// radii to the side, on the screen.
fn drag_points(target: &Target) -> Vec<Vec2> {
    let from = target.line[SAMPLES / 2];
    let to = target.view.project(target.path[SAMPLES / 2] + Vec3::X * 2.0 * RADIUS).unwrap();
    (0..=SAMPLES / 2).map(|i| from.lerp(to, i as f32 / (SAMPLES / 2) as f32)).collect()
}

struct Bench {
    ball: Target,
    ball_before: Before,
    rough: Target,
    rough_before: Before,
    rough_start: f64,
    ribs: Target,
    ribs_start: f64,
    lumps: Target,
    lumps_start: f64,
    fin: Target,
    fin_before: Before,
}

fn row(bench: &mut Bench, held: Held, strength: Option<f32>, pressure: f32) {
    if !wanted(held.name()) {
        return;
    }
    let shown = held.sculptor(strength).settings.current().strength;
    let setting = if pressure < 1.0 { format!("{shown:.3} light") } else { format!("{shown:.3}") };
    let mut line = format!("{:<18} {:<11}", held.name(), setting);
    if held.drags() {
        let points = drag_points(&bench.ball);
        let grabbed = (0..bench.ball.mesh.vertex_count())
            .min_by(|&a, &b| {
                let at = bench.ball.path[SAMPLES / 2];
                bench.ball_before.positions[a].distance(at).total_cmp(&bench.ball_before.positions[b].distance(at))
            })
            .unwrap();
        let delta = draw(&mut bench.ball, &mut held.sculptor(strength), &points, pressure);
        let pass = measure(&bench.ball, &bench.ball_before);
        let went = bench.ball.mesh.positions[grabbed].distance(bench.ball_before.positions[grabbed]) / (2.0 * RADIUS);
        undo(&mut bench.ball, delta);
        line += &format!(
            " follow {went:>5.3}  out1 {:>6.3}  move1 {:>6.3}  angle {:>5.1}/{:>5.1}  edgeCV {:>5.3}  dV {:>8.4}",
            pass.out, pass.moved / 2.0, pass.angle.0, pass.angle.1, pass.edges, pass.dv
        );
        if held.kernel() == Brush::SnakeHook {
            line += &pull(bench, held, strength, pressure);
        }
        println!("{line}");
        return;
    }
    let there: Vec<Vec2> = bench.ball.line.clone();
    let mut thrice = there.clone();
    thrice.extend(there.iter().rev().skip(1));
    thrice.extend(there.iter().skip(1));

    let delta = draw(&mut bench.ball, &mut held.sculptor(strength), &there, pressure);
    let one = measure(&bench.ball, &bench.ball_before);
    undo(&mut bench.ball, delta);
    let delta = draw(&mut bench.ball, &mut held.sculptor(strength), &thrice, pressure);
    let three = measure(&bench.ball, &bench.ball_before);
    undo(&mut bench.ball, delta);
    let strokes: Vec<Option<StrokeDelta>> =
        (0..3).map(|_| draw(&mut bench.ball, &mut held.sculptor(strength), &there, pressure)).collect();
    let separate = measure(&bench.ball, &bench.ball_before);
    for delta in strokes.into_iter().rev() {
        undo(&mut bench.ball, delta);
    }
    let fin_line = bench.fin.line.clone();
    let delta = draw(&mut bench.fin, &mut held.sculptor(strength), &fin_line, pressure);
    let back = bench
        .fin
        .mesh
        .positions
        .iter()
        .zip(&bench.fin_before.positions)
        .zip(&bench.fin_before.normals)
        .filter(|(_, n)| n.z < -0.9)
        .map(|((p, p0), _)| p.distance(*p0))
        .fold(0.0f32, f32::max)
        / RADIUS;
    undo(&mut bench.fin, delta);
    line += &format!(
        " out1 {:>6.3}  out3 {:>6.3}  in1 {:>7.4}  move1 {:>6.3}  ridge {:>6.2}/{:>5.2}  angle {:>5.1}/{:>5.1}  edgeCV {:>5.3}  dV {:>8.4}  fin {:>6.3}",
        one.out, three.out, one.sank, one.moved, one.ridge.0, one.ridge.1, one.angle.0, one.angle.1, one.edges, one.dv, back
    );
    if held.smooths() {
        let rough_line = bench.rough.line.clone();
        let delta = draw(&mut bench.rough, &mut held.sculptor(strength), &rough_line, pressure);
        let left = roughness(&bench.rough.mesh, &bench.rough_before.under) / bench.rough_start;
        undo(&mut bench.rough, delta);
        line += &format!("  rough {left:>5.3}");
        let lump_line = bench.lumps.line.clone();
        let delta = draw(&mut bench.lumps, &mut held.sculptor(strength), &lump_line, pressure);
        let lumps = lumpiness(&bench.lumps.mesh, &bench.lumps.path) / bench.lumps_start;
        undo(&mut bench.lumps, delta);
        line += &format!("  lumps {lumps:>5.3}");
        if held.kernel() == Brush::Smooth {
            line += &ten(bench, held, strength, pressure);
        }
    }
    if held.levels() {
        let rib_line = bench.ribs.line.clone();
        let delta = draw(&mut bench.ribs, &mut held.sculptor(strength), &rib_line, pressure);
        let left = rib_height(&bench.ribs.mesh, &bench.ribs.path) / bench.ribs_start;
        undo(&mut bench.ribs, delta);
        line += &format!("  ribs {left:>5.3}");
    }
    line += &format!(
        "  out3s {:>6.3}  ridge3 {:>6.2}/{:>5.2}  angle3 {:>5.1}/{:>5.1}  crest {:>5.2}  crest3 {:>5.2}",
        separate.out, three.ridge.0, three.ridge.1, three.angle.0, three.angle.1, one.crest, three.crest
    );
    println!("{line}");
}

pub fn run() {
    println!("threads: {}", rayon::current_num_threads());
    let ball_t = Target::ball(ball(false));
    let ball_before = Before::of(&ball_t);
    let rough_t = Target::ball(ball(true));
    let rough_before = Before::of(&rough_t);
    let rough_start = roughness(&rough_t.mesh, &rough_before.under);
    let ribs_t = Target::ball(ribbed());
    let ribs_start = rib_height(&ribs_t.mesh, &ribs_t.path);
    let lumps_t = Target::ball(lumpy());
    let lumps_start = lumpiness(&lumps_t.mesh, &lumps_t.path);
    let fin_t = fin();
    let fin_before = Before::of(&fin_t);
    let (mean, worst, edges) = quality(&ball_before.positions, &ball_before.tris);
    println!(
        "ball: {} triangles, brush radius {RADIUS} on a ball of {BALL}; under the stroke {} vertices, {} triangles, angle {mean:.1}/{worst:.1}, edgeCV {edges:.3}",
        ball_t.mesh.triangle_count(),
        ball_before.under.iter().filter(|u| **u).count(),
        ball_before.tris.len()
    );
    println!("fin: {} triangles, {} thick", fin_t.mesh.triangle_count(), 0.4 * RADIUS);
    let mut bench = Bench {
        ball: ball_t,
        ball_before,
        rough: rough_t,
        rough_before,
        rough_start,
        ribs: ribs_t,
        ribs_start,
        lumps: lumps_t,
        lumps_start,
        fin: fin_t,
        fin_before,
    };
    let base = [
        Brush::Standard,
        Brush::Inflate,
        Brush::Layer,
        Brush::Smooth,
        Brush::Flatten,
        Brush::Pinch,
        Brush::Nudge,
        Brush::Clay,
        Brush::Crease,
        Brush::SnakeHook,
        Brush::Move,
    ];
    for brush in base {
        let held = Held::Base(brush);
        for strength in [Some(0.1), Some(0.5), Some(1.0), None] {
            row(&mut bench, held, strength, 1.0);
        }
        row(&mut bench, held, None, LIGHT);
    }
    for name in ["Clay Strips", "Dam Standard", "Clay Tubes", "Rake", "Surface Noise", "Trim", "Fill", "Planar", "Polish"] {
        row(&mut bench, Held::Preset(name), None, 1.0);
        row(&mut bench, Held::Preset(name), None, LIGHT);
    }
    for name in ["Smooth Peaks", "Smooth Valleys", "Relax", "Keep Volume", "Surface Smooth", "Spread"] {
        row(&mut bench, Held::Preset(name), None, 1.0);
    }
    // The brushes that build, with Accumulate each way, at their own Strength.
    let tuned: [Held; 6] = [
        Held::Tuned("Standard acc", Brush::Standard, |b| b.accumulate = true),
        Held::Tuned("Standard noacc", Brush::Standard, |b| b.accumulate = false),
        Held::Tuned("Inflate acc", Brush::Inflate, |b| b.accumulate = true),
        Held::Tuned("Inflate noacc", Brush::Inflate, |b| b.accumulate = false),
        Held::Tuned("Clay acc", Brush::Clay, |b| b.accumulate = true),
        Held::Tuned("Clay noacc", Brush::Clay, |b| b.accumulate = false),
    ];
    for held in tuned {
        row(&mut bench, held, None, 1.0);
    }
    // What Steady Plane and Relax Normals do: the brushes with them at nought.
    let without: [Held; 4] = [
        Held::Tuned("Clay steady0", Brush::Clay, |b| b.steady_plane = 0.0),
        Held::Tuned("Flatten steady0", Brush::Flatten, |b| b.steady_plane = 0.0),
        Held::Tuned("Inflate relax0", Brush::Inflate, |b| b.relax_normals = 0.0),
        Held::Tuned("Inflate acc relax0", Brush::Inflate, |b| (b.accumulate, b.relax_normals) = (true, 0.0)),
    ];
    for held in without {
        row(&mut bench, held, None, 1.0);
    }
    // What Auto Smooth and Even Out do, and Smooth's two ways of smoothing:
    // the brushes with each of them at nought, at their own Strength.
    let clean: [Held; 9] = [
        Held::Tuned("Smooth surface", Brush::Smooth, |b| b.smooth_mode = SmoothMode::Surface),
        Held::Tuned("Standard smooth0", Brush::Standard, |b| b.auto_smooth = 0.0),
        Held::Tuned("Layer smooth0", Brush::Layer, |b| b.auto_smooth = 0.0),
        Held::Tuned("Inflate tidy0", Brush::Inflate, |b| (b.auto_smooth, b.even_out) = (0.0, 0.0)),
        Held::Tuned("Inflate even0", Brush::Inflate, |b| b.even_out = 0.0),
        Held::Tuned("Snake Hook thick0", Brush::SnakeHook, |b| b.thickness = 0.0),
        // What the plan's starting values for Snake Hook did to a pull.
        Held::Tuned("Snake Hook smooth.2", Brush::SnakeHook, |b| b.auto_smooth = 0.2),
        Held::Tuned("Snake Hook even.2", Brush::SnakeHook, |b| b.even_out = 0.2),
        Held::Tuned("Move even0", Brush::Move, |b| b.even_out = 0.0),
    ];
    for held in clean {
        row(&mut bench, held, None, 1.0);
    }
    // Rows for tuning, run only when asked for by name (`SCULPT_FEEL_ROWS`).
    if std::env::var("SCULPT_FEEL_ROWS").is_ok_and(|rows| !rows.trim().is_empty()) {
        tuning(&mut bench);
    }
}

/// Plain Clay at other Clay Depths, Trim at other Strengths and Snake Hook
/// at other Thicknesses: what the starting values were picked from.
fn tuning(bench: &mut Bench) {
    let rows: [Held; 27] = [
        Held::Set("Clay d0.3", Brush::Clay, |s| s.clay_depth = 0.3),
        Held::Set("Clay d0.4", Brush::Clay, |s| s.clay_depth = 0.4),
        Held::Set("Clay d0.5", Brush::Clay, |s| s.clay_depth = 0.5),
        Held::Set("Clay d0.55", Brush::Clay, |s| s.clay_depth = 0.55),
        Held::Set("Clay d0.6", Brush::Clay, |s| s.clay_depth = 0.6),
        Held::Set("Clay d0.7", Brush::Clay, |s| s.clay_depth = 0.7),
        Held::PresetTuned("Trim s0.62", "Trim", |b| (b.strength, b.steady_plane) = (0.62, 0.5)),
        Held::PresetTuned("Trim s0.711", "Trim", |b| (b.strength, b.steady_plane) = (0.711, 0.5)),
        Held::PresetTuned("Trim st0.3", "Trim", |b| (b.strength, b.steady_plane) = (0.711, 0.3)),
        Held::PresetTuned("Trim st0.3 s0.69", "Trim", |b| (b.strength, b.steady_plane) = (0.69, 0.3)),
        Held::PresetTuned("Trim st0.4 s0.67", "Trim", |b| (b.strength, b.steady_plane) = (0.67, 0.4)),
        Held::PresetTuned("Trim st0", "Trim", |b| (b.strength, b.steady_plane) = (0.711, 0.0)),
        Held::Tuned("Snake t-0.5", Brush::SnakeHook, |b| b.thickness = -0.5),
        Held::Tuned("Snake t0.25", Brush::SnakeHook, |b| b.thickness = 0.25),
        Held::Tuned("Snake t0.5", Brush::SnakeHook, |b| b.thickness = 0.5),
        Held::Tuned("Snake t0.75", Brush::SnakeHook, |b| b.thickness = 0.75),
        Held::Tuned("Snake t1", Brush::SnakeHook, |b| b.thickness = 1.0),
        Held::Tuned("Snake s.02", Brush::SnakeHook, |b| b.auto_smooth = 0.02),
        Held::Tuned("Snake s.05", Brush::SnakeHook, |b| b.auto_smooth = 0.05),
        Held::Tuned("Snake e.1", Brush::SnakeHook, |b| b.even_out = 0.1),
        Held::Tuned("Snake s.2 e.2", Brush::SnakeHook, |b| (b.auto_smooth, b.even_out) = (0.2, 0.2)),
        Held::Tuned("Inflate s.3 e.4", Brush::Inflate, |b| (b.auto_smooth, b.even_out) = (0.3, 0.4)),
        Held::Tuned("Inflate 1.0 tidy0", Brush::Inflate, |b| (b.strength, b.auto_smooth, b.even_out) = (1.0, 0.0, 0.0)),
        Held::Tuned("Inflate 1.0 even0", Brush::Inflate, |b| (b.strength, b.even_out) = (1.0, 0.0)),
        Held::Tuned("Inflate acc 1.0", Brush::Inflate, |b| (b.strength, b.accumulate) = (1.0, true)),
        Held::Tuned("Inflate acc 1.0 even0", Brush::Inflate, |b| (b.strength, b.accumulate, b.even_out) = (1.0, true, 0.0)),
        Held::Tuned("Inflate acc 1.0 tidy0", Brush::Inflate, |b| (b.strength, b.accumulate, b.auto_smooth, b.even_out) = (1.0, true, 0.0, 0.0)),
    ];
    for held in rows {
        row(bench, held, Some(0.5).filter(|_| held.name().starts_with("Clay d")), 1.0);
    }
}
