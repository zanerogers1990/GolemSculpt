//! How steadily a muscle follows the pen over a detailed model: measured, not looked at.
//!
//! The bench's own raycast tries every triangle, which is too slow for a model as dense as the ones
//! muscles are drawn on. The muscles here are laid with [`Accel`], a plain uniform grid over the same
//! triangles that gives the same hits, and the tool's own frame then finds them already built.
//!
//! The `measure_*` tests print numbers and are ignored by default:
//! `cargo test -p sculpt-tube jumpy -- --ignored --nocapture --test-threads 1`.

use std::f32::consts::TAU;

use glam::{Vec2, Vec3};
use sculpt_geom::{TriMesh, tri};
use sculpt_toolkit::testing::Bench;
use sculpt_toolkit::{Button, Hit, ObjectId, Ray, Scene, View};

use crate::live::{Frame, LiveTube, Symmetry};
use crate::testing::icosphere;
use crate::{Sample, TubeTool};

// ---- a model with detail on it ---------------------------------------------------------------

fn hash(i: u32) -> f32 {
    let mut x = i.wrapping_mul(0x9E37_79B1) ^ 0x85EB_CA6B;
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    x = x.wrapping_mul(0x297A_2D39);
    x ^= x >> 15;
    x as f32 / u32::MAX as f32 * 2.0 - 1.0
}

/// What is sculpted into the sphere of radius 0.5.
#[derive(Clone, Copy, Debug)]
pub(super) struct Detail {
    pub subdivisions: u32,
    /// Wrinkles: their height and wavelength.
    pub wrinkle: f32,
    pub wavelength: f32,
    /// Each vertex pushed in or out at random by up to this fraction of an edge: the roughness of a
    /// sculpted surface, which is what makes its face normals wander.
    pub rough: f32,
    /// A groove across the front: its depth and half width, running along y at x = `groove_x`.
    pub groove: f32,
    pub groove_width: f32,
    pub groove_x: f32,
    /// A pair of ridges along y at x = +-`ridge_x`.
    pub ridges: f32,
    pub ridge_width: f32,
    pub ridge_x: f32,
}

impl Detail {
    pub const PLAIN: Detail = Detail {
        subdivisions: 6,
        wrinkle: 0.0,
        wavelength: 0.07,
        rough: 0.0,
        groove: 0.0,
        groove_width: 0.02,
        groove_x: 0.0,
        ridges: 0.0,
        ridge_width: 0.015,
        ridge_x: 0.04,
    };

    /// Wrinkles a muscle's width apart and a tenth of its radius high, and a rough surface.
    pub const SCULPTED: Detail = Detail { subdivisions: 7, wrinkle: 0.0045, rough: 0.12, ..Detail::PLAIN };

    pub fn edge(&self) -> f32 {
        0.5 * 1.1 / (1u32 << self.subdivisions) as f32
    }

    pub fn mesh(&self) -> TriMesh {
        let mut mesh = icosphere(self.subdivisions, 0.5, Vec3::ZERO);
        let k = TAU / self.wavelength;
        let rough = self.rough * self.edge();
        for (i, p) in mesh.positions.iter_mut().enumerate() {
            let n = p.normalize();
            let s = n * 0.5;
            let w = 0.5 * ((k * s.x + 0.3).sin() * (k * s.y + 1.1).sin() + (k * (0.8 * s.y + 0.6 * s.z) + 2.0).sin());
            let gauss = |x: f32, width: f32| (-(x * x) / (width * width)).exp();
            let groove = self.groove * gauss(s.x - self.groove_x, self.groove_width) * (s.z > 0.0) as u8 as f32;
            let ridges = self.ridges
                * (gauss(s.x - self.ridge_x, self.ridge_width) + gauss(s.x + self.ridge_x, self.ridge_width))
                * (s.z > 0.0) as u8 as f32;
            *p = n * (0.5 + self.wrinkle * w + rough * hash(i as u32) - groove + ridges);
        }
        mesh
    }
}

// ---- a raycast fast enough for it --------------------------------------------------------------

/// Triangles bucketed in a uniform grid; rays walk it cell by cell. The same nearest hit and face
/// normal as the bench's own raycast.
pub(super) struct Accel {
    pub mesh: TriMesh,
    min: Vec3,
    cell: f32,
    n: [i32; 3],
    starts: Vec<u32>,
    items: Vec<u32>,
}

impl Accel {
    pub fn new(mesh: TriMesh) -> Accel {
        let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for p in &mesh.positions {
            min = min.min(*p);
            max = max.max(*p);
        }
        min -= Vec3::splat(1e-4);
        max += Vec3::splat(1e-4);
        let size = max - min;
        let cells = (mesh.triangle_count() as f32 * 0.5).max(1.0);
        let cell = (size.x * size.y * size.z / cells).cbrt().max(1e-5);
        let n = [0, 1, 2].map(|a| ((size[a] / cell).ceil() as i32).max(1));
        let index = |c: [i32; 3]| (c[0] + n[0] * (c[1] + n[1] * c[2])) as usize;
        let total = (n[0] * n[1] * n[2]) as usize;
        let range = |t: usize| {
            let [a, b, c] = mesh.triangle(t);
            let lo = ((a.min(b).min(c) - min) / cell).floor();
            let hi = ((a.max(b).max(c) - min) / cell).floor();
            let clamp = |v: Vec3| [0, 1, 2].map(|k| (v[k] as i32).clamp(0, n[k] - 1));
            (clamp(lo), clamp(hi))
        };
        let mut counts = vec![0u32; total + 1];
        for t in 0..mesh.triangle_count() {
            let (lo, hi) = range(t);
            for z in lo[2]..=hi[2] {
                for y in lo[1]..=hi[1] {
                    for x in lo[0]..=hi[0] {
                        counts[index([x, y, z]) + 1] += 1;
                    }
                }
            }
        }
        for i in 0..total {
            counts[i + 1] += counts[i];
        }
        let starts = counts.clone();
        let mut next = counts;
        let mut items = vec![0u32; starts[total] as usize];
        for t in 0..mesh.triangle_count() {
            let (lo, hi) = range(t);
            for z in lo[2]..=hi[2] {
                for y in lo[1]..=hi[1] {
                    for x in lo[0]..=hi[0] {
                        let c = index([x, y, z]);
                        items[next[c] as usize] = t as u32;
                        next[c] += 1;
                    }
                }
            }
        }
        Accel { mesh, min, cell, n, starts, items }
    }

    /// Every crossing of the line `origin + t dir`, t >= 0, sorted (brute force over a box).
    pub fn crossings(&self, origin: Vec3, dir: Vec3, length: f32) -> Vec<f32> {
        let end = origin + dir * length;
        let (lo, hi) = (origin.min(end), origin.max(end));
        let mut found = Vec::new();
        let cl = |v: Vec3| [0, 1, 2].map(|k| (((v[k] - self.min[k]) / self.cell).floor() as i32).clamp(0, self.n[k] - 1));
        let (a, b) = (cl(lo), cl(hi));
        let mut seen = std::collections::HashSet::new();
        for z in a[2]..=b[2] {
            for y in a[1]..=b[1] {
                for x in a[0]..=b[0] {
                    let c = (x + self.n[0] * (y + self.n[1] * z)) as usize;
                    for &t in &self.items[self.starts[c] as usize..self.starts[c + 1] as usize] {
                        if !seen.insert(t) {
                            continue;
                        }
                        let [p, q, r] = self.mesh.triangle(t as usize);
                        if let Some(d) = tri::ray_hit(origin, dir, p, q, r) {
                            if d <= length {
                                found.push(d);
                            }
                        }
                    }
                }
            }
        }
        found.sort_by(f32::total_cmp);
        // A ray through an edge meets both triangles: one crossing.
        found.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
        found
    }

    pub fn cast(&self, object: ObjectId, ray: &Ray) -> Option<Hit> {
        let (origin, dir) = (ray.origin, ray.dir);
        let inv = dir.recip();
        // Into the grid's box.
        let (mut t0, mut t1) = (0.0f32, f32::MAX);
        let max = self.min + Vec3::new(self.n[0] as f32, self.n[1] as f32, self.n[2] as f32) * self.cell;
        for a in 0..3 {
            let (near, far) = ((self.min[a] - origin[a]) * inv[a], (max[a] - origin[a]) * inv[a]);
            let (near, far) = if near < far { (near, far) } else { (far, near) };
            if near.is_nan() || far.is_nan() {
                if origin[a] < self.min[a] || origin[a] > max[a] {
                    return None;
                }
                continue;
            }
            t0 = t0.max(near);
            t1 = t1.min(far);
        }
        if t0 > t1 {
            return None;
        }
        let start = origin + dir * (t0 + 1e-7);
        let mut c = [0, 1, 2].map(|a| (((start[a] - self.min[a]) / self.cell).floor() as i32).clamp(0, self.n[a] - 1));
        let step = [0, 1, 2].map(|a| if dir[a] > 0.0 { 1 } else { -1 });
        let mut t_max = [0, 1, 2].map(|a| {
            if dir[a] == 0.0 {
                return f32::MAX;
            }
            let boundary = self.min[a] + (c[a] + if dir[a] > 0.0 { 1 } else { 0 }) as f32 * self.cell;
            (boundary - origin[a]) * inv[a]
        });
        let t_delta = [0, 1, 2].map(|a| if dir[a] == 0.0 { f32::MAX } else { self.cell * inv[a].abs() });
        let mut best: Option<(f32, u32)> = None;
        loop {
            let cell = (c[0] + self.n[0] * (c[1] + self.n[1] * c[2])) as usize;
            for &t in &self.items[self.starts[cell] as usize..self.starts[cell + 1] as usize] {
                let [a, b, cc] = self.mesh.triangle(t as usize);
                if let Some(d) = tri::ray_hit(origin, dir, a, b, cc) {
                    if d >= 0.0 && best.is_none_or(|(nearest, _)| d < nearest) {
                        best = Some((d, t));
                    }
                }
            }
            let axis = if t_max[0] < t_max[1] { if t_max[0] < t_max[2] { 0 } else { 2 } } else if t_max[1] < t_max[2] { 1 } else { 2 };
            if best.is_some_and(|(d, _)| d <= t_max[axis]) {
                break;
            }
            c[axis] += step[axis];
            if c[axis] < 0 || c[axis] >= self.n[axis] {
                break;
            }
            t_max[axis] += t_delta[axis];
        }
        let (d, t) = best?;
        let [a, b, cc] = self.mesh.triangle(t as usize);
        let mut normal = (b - a).cross(cc - a).normalize_or(Vec3::Y);
        if normal.dot(dir) > 0.0 {
            normal = -normal;
        }
        let position = origin + dir * d;
        Some(Hit { object, position, normal, distance: (position - origin).dot(dir) })
    }
}

// ---- the tool on it ----------------------------------------------------------------------------

/// The Muscle tool on a sculpted sphere, seen from close up: the sphere fills most of the view, a
/// muscle is about 20 pixels across.
pub(super) fn close_bench(detail: &Detail) -> (Bench, TubeTool, ObjectId, Accel) {
    let mut bench = Bench::new();
    bench.env.view = View::perspective(Vec3::new(0.0, 0.0, 1.6), Vec3::ZERO, 60.0, Vec2::new(800.0, 600.0));
    let mesh = detail.mesh();
    let body = bench.add("Body", mesh.clone());
    let mut tool = TubeTool::muscle();
    bench.enter(&mut tool);
    (bench, tool, body, Accel::new(mesh))
}

/// Builds every live muscle and the draft as the tool's frame would, with the fast raycast.
pub(super) fn build_all(bench: &mut Bench, tool: &mut TubeTool, accel: &Accel, preview: bool) {
    bench.call(|cx| {
        let scene = cx.scene();
        for tube in tool.tubes.iter_mut().chain(tool.pointer.draft.as_mut()) {
            let Some(anchor) = tube.state.anchor else { continue };
            let info = scene.info(anchor).unwrap();
            let frame = Frame { placement: info.placement };
            let symmetry = Symmetry::of(cx.symmetry_of(anchor));
            let ground = Some((info.mesh, info.vertices));
            let cast = |ray: &Ray| accel.cast(anchor, ray);
            tube.build(frame, symmetry, Some(&cast), ground);
            if preview {
                tube.build_preview();
            }
        }
    });
}

/// A muscle's shape where it can be compared from one moment to the next: its centre, top and two
/// sides at evenly spaced places along it.
#[derive(Clone, Debug)]
pub(super) struct Shape {
    pub points: Vec<Vec3>,
    pub length: f32,
    pub centre: Vec<Vec3>,
    pub top: Vec<Vec3>,
    pub sides: Vec<[Vec3; 2]>,
    pub up: Vec<Vec3>,
}

fn lerp_sample(samples: &[Sample], at: f32) -> (Vec3, Vec3, Vec3, f32, f32) {
    let k = samples.partition_point(|s| s.distance < at).clamp(1, samples.len() - 1);
    let (a, b) = (&samples[k - 1], &samples[k]);
    let t = ((at - a.distance) / (b.distance - a.distance).max(1e-12)).clamp(0.0, 1.0);
    let tangent = (b.center - a.center).normalize_or(a.tangent_out);
    (a.center.lerp(b.center, t), a.up.lerp(b.up, t).normalize_or_zero(), tangent, a.radius + (b.radius - a.radius) * t, a.flat)
}

impl Shape {
    /// At `places` evenly spaced fractions of its length, or every `step` along it from its start.
    pub fn of(tube: &LiveTube, places: usize, step: Option<f32>) -> Shape {
        let samples = tube.copies[0].samples.clone();
        let length = samples.last().map_or(0.0, |s| s.distance);
        let mut shape = Shape {
            points: tube.state.points.iter().map(|p| p.position).collect(),
            length,
            centre: Vec::new(),
            top: Vec::new(),
            sides: Vec::new(),
            up: Vec::new(),
        };
        for j in 0..places {
            let at = match step {
                Some(step) => j as f32 * step,
                None => length * j as f32 / (places - 1) as f32,
            };
            if at > length {
                break;
            }
            let (c, up, tangent, r, flat) = lerp_sample(&samples, at);
            let across = up.cross(tangent).normalize_or_zero();
            shape.centre.push(c);
            shape.top.push(c + up * r * flat);
            shape.sides.push([c + across * r, c - across * r]);
            shape.up.push(up);
        }
        shape
    }

    /// The largest move of any of the compared places between two shapes, over the first `upto` of
    /// them, and the largest turn of the way up (degrees).
    pub fn moved(&self, next: &Shape, upto: usize) -> (f32, f32) {
        let n = upto.min(self.centre.len()).min(next.centre.len());
        let mut worst: f32 = 0.0;
        let mut turn: f32 = 0.0;
        for j in 0..n {
            worst = worst.max(self.top[j].distance(next.top[j]));
            worst = worst.max(self.sides[j][0].distance(next.sides[j][0]));
            worst = worst.max(self.sides[j][1].distance(next.sides[j][1]));
            turn = turn.max(self.up[j].dot(next.up[j]).clamp(-1.0, 1.0).acos().to_degrees());
        }
        (worst, turn)
    }
}

/// Three shapes in a row: how far the middle one is from halfway between the other two, at worst
/// (the jerk of a motion: nothing for a steady one).
pub(super) fn jerk(a: &Shape, b: &Shape, c: &Shape, upto: usize) -> f32 {
    let n = upto.min(a.centre.len()).min(b.centre.len()).min(c.centre.len());
    let mut worst: f32 = 0.0;
    for j in 0..n {
        let off = |x: Vec3, y: Vec3, z: Vec3| (y - (x + z) * 0.5).length();
        worst = worst.max(off(a.top[j], b.top[j], c.top[j]));
        worst = worst.max(off(a.sides[j][0], b.sides[j][0], c.sides[j][0]));
        worst = worst.max(off(a.sides[j][1], b.sides[j][1], c.sides[j][1]));
    }
    worst
}

pub(super) fn stats(values: &[f32]) -> (f32, f32, f32) {
    if values.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    let mut v = values.to_vec();
    v.sort_by(f32::total_cmp);
    let at = |q: f32| v[((v.len() - 1) as f32 * q).round() as usize];
    (at(0.5), at(0.95), v[v.len() - 1])
}

/// A hand-drawn line across the front of the body from `from` to `to` (world points on it), bowed
/// by `bow` pixels and with a tremor of a pixel or two, sampled every pixel.
pub(super) fn hand_path(bench: &Bench, from: Vec3, to: Vec3, bow: f32, tremor: f32, waves: f32) -> Vec<Vec2> {
    let (a, b) = (bench.px(from), bench.px(to));
    let length = a.distance(b);
    let normal = (b - a).perp().normalize();
    let steps = length.ceil() as usize;
    (0..=steps)
        .map(|i| {
            let t = i as f32 / steps as f32;
            let s = i as f32;
            let wobble = tremor * (0.6 * (s * 0.37).sin() + 0.4 * (s * 1.31 + 0.5).sin());
            a.lerp(b, t) + normal * (bow * (std::f32::consts::PI * t * waves).sin() + wobble)
        })
        .collect()
}

pub(super) fn on_front(x: f32, y: f32) -> Vec3 {
    Vec3::new(x, y, (0.25 - x * x - y * y).max(0.0).sqrt())
}

// ---- drawing -----------------------------------------------------------------------------------

#[derive(Debug, Default)]
pub(super) struct DrawReport {
    pub samples: usize,
    pub points_at_end: usize,
    /// Steps on which a point other than the one under the pen appeared, went or moved.
    pub reshuffles: usize,
    /// Per step: the largest move of the muscle away from the pen (in radii), and the pen's own move.
    pub head_move: Vec<f32>,
    pub head_turn: Vec<f32>,
    pub head_jerk: Vec<f32>,
    pub pen_move: Vec<f32>,
    /// The steps on which the part away from the pen moved more than a tenth of a radius.
    pub events: Vec<String>,
    /// How far the muscle moved as the pen came up, at worst (radii).
    pub release_snap: f32,
}

/// Draws along `path`, building the draft after every pointer sample as the frame would, and
/// measures how the part of it away from the pen changes.
pub(super) fn measure_draw(bench: &mut Bench, tool: &mut TubeTool, accel: &Accel, path: &[Vec2]) -> DrawReport {
    let r = tool.settings().radius;
    let step = r * 0.25;
    let mut report = DrawReport::default();
    assert!(bench.press(tool, path[0], Button::Left));
    let mut last: Option<Shape> = None;
    let mut before_last: Option<Shape> = None;
    let mut last_world = tool.pointer.stroke.world.len();
    for &px in &path[1..] {
        bench.drag_to(tool, px);
        let world = tool.pointer.stroke.world.len();
        if world == last_world {
            continue;
        }
        let pen = tool.pointer.stroke.world[world - 1].distance(tool.pointer.stroke.world[world - 2]);
        last_world = world;
        // The tool's own input already built the draft; this only builds it again if a test changed it.
        build_all(bench, tool, accel, false);
        let draft = tool.pointer.draft.as_ref().unwrap();
        if draft.copies.is_empty() || !draft.copies[0].is_valid() {
            continue;
        }
        let shape = Shape::of(draft, 4000, Some(step));
        if let Some(before) = &last {
            // Away from the pen: all but the last three radii (the last two spans move with the pen
            // as a matter of course), and nothing past where the shorter of the two ends.
            let keep = ((before.length.min(shape.length) - 3.0 * r) / step).floor().max(0.0) as usize;
            let (a, b) = (&before.points, &shape.points);
            let settled = |p: &[Vec3]| p[..p.len() - 1].to_vec();
            let (sa, sb) = (settled(a), settled(b));
            let reshuffled = !(sb.len() >= sa.len() && sa.iter().zip(&sb).all(|(x, y)| x == y));
            if reshuffled {
                report.reshuffles += 1;
            }
            if keep > 0 {
                let (moved, turn) = before.moved(&shape, keep);
                report.head_move.push(moved / r);
                report.head_turn.push(turn);
                report.pen_move.push(pen / r);
                if let Some(first) = &before_last {
                    report.head_jerk.push(jerk(first, before, &shape, keep) / r);
                }
                if moved > 0.1 * r {
                    report.events.push(format!(
                        "sample {}: points {} -> {}{}, far part moved {:.3} r",
                        report.samples,
                        a.len(),
                        b.len(),
                        if reshuffled { " (an earlier point moved)" } else { "" },
                        moved / r
                    ));
                }
            }
        }
        report.samples += 1;
        before_last = last.take();
        last = Some(shape);
    }
    bench.release(tool);
    report.points_at_end = tool.point_count(tool.tube_count() - 1);
    build_all(bench, tool, accel, false);
    if let Some(drawn) = &last {
        let laid = Shape::of(tool.tubes.last().unwrap(), 4000, Some(step));
        let n = drawn.centre.len().min(laid.centre.len());
        report.release_snap = drawn.moved(&laid, n).0 / r;
    }
    report
}

// ---- dragging a point ----------------------------------------------------------------------------

#[derive(Debug, Default)]
pub(super) struct DragReport {
    pub point_move: Vec<f32>,
    pub normal_turn: Vec<f32>,
    pub moved: Vec<f32>,
    pub turn: Vec<f32>,
    pub jerk: Vec<f32>,
    pub far_moved: Vec<f32>,
    /// Where along the muscle (0 to 1) the way up turned most, on the steps it turned over 2 degrees.
    pub turn_at: Vec<f32>,
}

/// Lays a muscle through `through`, then drags point `point` by `delta` pixels one pixel at a time,
/// building after every step, and measures.
pub(super) fn measure_drag(
    bench: &mut Bench,
    tool: &mut TubeTool,
    accel: &Accel,
    through: &[Vec3],
    point: usize,
    delta: Vec2,
    after_step: &mut dyn FnMut(&mut TubeTool),
) -> DragReport {
    let px: Vec<Vec2> = through.iter().map(|&p| bench.px(p)).collect();
    bench.press(tool, px[0], Button::Left);
    for pair in px.windows(2) {
        // Three samples a span: enough for the simplification to keep a bend, and each one a build.
        for step in 1..=3 {
            bench.drag_to(tool, pair[0].lerp(pair[1], step as f32 / 3.0));
        }
    }
    bench.release(tool);
    let tube = tool.tube_count() - 1;
    assert_eq!(tool.point_count(tube), through.len(), "the stroke kept its points");
    after_step(tool);
    build_all(bench, tool, accel, false);
    let r = tool.settings().radius;
    let places = 81;
    let mut shapes = vec![Shape::of(&tool.tubes[tube], places, None)];
    let mut normals = vec![tool.points(tube)[point].surface_normal];
    let grab = bench.px(tool.handle_world(tube, point, 0).unwrap());
    bench.press(tool, grab, Button::Left);
    let steps = delta.length().round() as usize;
    for i in 1..=steps {
        bench.drag_to(tool, grab + delta * (i as f32 / steps as f32));
        after_step(tool);
        build_all(bench, tool, accel, false);
        shapes.push(Shape::of(&tool.tubes[tube], places, None));
        normals.push(tool.points(tube)[point].surface_normal);
    }
    bench.release(tool);
    let mut report = DragReport::default();
    // Skip the first step: the drag threshold holds it back for a few pixels.
    for i in 4..shapes.len() {
        let (a, b) = (&shapes[i - 1], &shapes[i]);
        report.point_move.push(a.points[point].distance(b.points[point]) / r);
        report.normal_turn.push(normals[i - 1].dot(normals[i]).clamp(-1.0, 1.0).acos().to_degrees());
        let (moved, turn) = a.moved(b, places);
        report.moved.push(moved / r);
        report.turn.push(turn);
        if turn > 2.0 {
            let n = a.up.len().min(b.up.len());
            let angle = |j: usize| a.up[j].dot(b.up[j]).clamp(-1.0, 1.0).acos();
            let worst = (0..n).max_by(|&x, &y| angle(x).total_cmp(&angle(y))).unwrap();
            report.turn_at.push(worst as f32 / (places - 1) as f32);
        }
        if i >= 5 {
            report.jerk.push(jerk(&shapes[i - 2], a, b, places) / r);
        }
        // The far half: away from the dragged point.
        let far: Vec<usize> = if point == 0 { (places / 2..places).collect() } else { (0..places / 2).collect() };
        let far_move = far
            .iter()
            .filter(|&&j| j < a.top.len() && j < b.top.len())
            .map(|&j| a.top[j].distance(b.top[j]))
            .fold(0.0, f32::max);
        report.far_moved.push(far_move / r);
    }
    report
}

#[derive(Clone, Copy, Debug)]
enum Scenario {
    Draw { sculpted: bool, bow: f32, tremor: f32, waves: f32 },
    Drag { sculpted: bool, middle: bool },
}

fn run(scenario: Scenario) -> String {
    let detail = |sculpted: bool| if sculpted { Detail::SCULPTED } else { Detail { subdivisions: 7, ..Detail::PLAIN } };
    let surface = |sculpted: bool| if sculpted { "sculpted" } else { "plain" };
    match scenario {
        Scenario::Draw { sculpted, bow, tremor, waves } => {
            let (mut bench, mut tool, _, accel) = close_bench(&detail(sculpted));
            let path = hand_path(&bench, on_front(-0.3, -0.1), on_front(0.3, 0.12), bow, tremor, waves);
            let report = measure_draw(&mut bench, &mut tool, &accel, &path);
            let (m50, m95, mmax) = stats(&report.head_move);
            let (_, t95, tmax) = stats(&report.head_turn);
            let (j50, j95, jmax) = stats(&report.head_jerk);
            let (p50, _, _) = stats(&report.pen_move);
            format!(
                "DRAW {} bow {bow} x{waves} tremor {tremor} | {} samples {} pts {} reshuffles | release snap {:.3} r | far part moves {m50:.3}/{m95:.3}/{mmax:.3} r | jerk {j50:.3}/{j95:.3}/{jmax:.3} | up turns p95 {t95:.2} max {tmax:.2} deg | pen {p50:.3} r",
                surface(sculpted),
                report.samples,
                report.points_at_end,
                report.reshuffles,
                report.release_snap
            ) + &report.events.iter().filter(|_| std::env::var("JUMPY_EVENTS").is_ok()).map(|e| format!("\n        {e}")).collect::<String>()
        }
        Scenario::Drag { sculpted, middle } => {
            let (mut bench, mut tool, _, accel) = close_bench(&detail(sculpted));
            let through = if middle {
                vec![on_front(-0.3, 0.0), on_front(0.0, 0.08), on_front(0.3, 0.0)]
            } else {
                vec![on_front(-0.25, -0.05), on_front(0.2, 0.05)]
            };
            let report = measure_drag(&mut bench, &mut tool, &accel, &through, 1, Vec2::new(10.0, 40.0), &mut |_| {});
            let s = |v: &[f32]| {
                let (a, b, c) = stats(v);
                format!("{a:.3}/{b:.3}/{c:.3}")
            };
            format!(
                "DRAG {} {} | point {} | normal turns {} deg | muscle moves {} r | far half {} | jerk {} | up turns {} deg",
                surface(sculpted),
                if middle { "middle" } else { "end" },
                s(&report.point_move),
                s(&report.normal_turn),
                s(&report.moved),
                s(&report.far_moved),
                s(&report.jerk),
                s(&report.turn)
            ) + &format!(" | turns over 2 deg at {:?}", report.turn_at.iter().map(|t| (t * 100.0).round() as i32).collect::<Vec<_>>())
        }
    }
}

/// Every scenario, each on its own thread. Lengths in muscle radii, per pointer sample (drawing) or
/// per pixel (dragging): median/p95/max. `JUMPY_EVENTS=1` lists the samples on which a drawn muscle
/// moved more than a tenth of a radius away from the pen.
#[test]
#[ignore]
fn measure_steadiness() {
    let scenarios = [
        Scenario::Draw { sculpted: false, bow: 0.0, tremor: 0.0, waves: 1.0 },
        Scenario::Draw { sculpted: false, bow: 25.0, tremor: 1.5, waves: 1.0 },
        Scenario::Draw { sculpted: true, bow: 0.0, tremor: 0.0, waves: 1.0 },
        Scenario::Draw { sculpted: true, bow: 25.0, tremor: 1.5, waves: 1.0 },
        Scenario::Draw { sculpted: true, bow: 40.0, tremor: 1.5, waves: 3.0 },
        Scenario::Drag { sculpted: false, middle: false },
        Scenario::Drag { sculpted: true, middle: false },
        Scenario::Drag { sculpted: true, middle: true },
    ];
    let lines: Vec<String> = std::thread::scope(|scope| {
        let handles: Vec<_> = scenarios
            .iter()
            .map(|&scenario| std::thread::Builder::new().stack_size(16 << 20).spawn_scoped(scope, move || run(scenario)).unwrap())
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for line in lines {
        println!("{line}");
    }
}

// ---- the preview lattice -------------------------------------------------------------------------

/// A laid muscle's stations moved by a fraction of a preview cell at a time, and the preview meshed
/// each time as `LiveTube::build_preview` does: how far its surface is from the true one, and how
/// much that changes from one step to the next. A lattice that followed the shape's bounds would
/// mesh each step a different way; a fixed one only moves the surface.
#[test]
#[ignore]
fn measure_preview_shimmer() {
    let (mut bench, mut tool, _, accel) = close_bench(&Detail { subdivisions: 6, ..Detail::PLAIN });
    let px: Vec<Vec2> = [on_front(-0.25, -0.05), on_front(0.2, 0.05)].iter().map(|&p| bench.px(p)).collect();
    bench.drag(&mut tool, px[0], px[1]);
    build_all(&mut bench, &mut tool, &accel, false);
    let settings = tool.settings();
    let samples = tool.tubes[0].copies[0].samples.clone();
    let field = tool.tubes[0].field();
    // The cell `build_preview` takes for a muscle.
    let thin = settings.thin_radius();
    let floor = field.min_radius().max(thin * 0.6);
    let cell = (thin / 3.5).min(floor / crate::merge::CELLS_ACROSS_RADIUS);
    let direction = Vec3::new(0.37, 0.61, 0.70).normalize();
    // As laid, and without what fills under it (which is mostly inside the model).
    for (what, filled) in [("as laid", true), ("without the fill", false)] {
        let samples: Vec<Sample> =
            samples.iter().map(|s| if filled { *s } else { Sample { under: 0.0, ..*s } }).collect();
        let field_at = |offset: Vec3| {
            let moved: Vec<Sample> = samples.iter().map(|s| Sample { center: s.center + offset, ..*s }).collect();
            let mut field = crate::TubeField::new();
            field.add_tube(&moved, settings.caps);
            field
        };
        let (mut errors, mut extra_moves) = (Vec::new(), Vec::new());
        for step in 0..=20 {
            let offset = direction * cell * step as f32 / 10.0;
            let field = field_at(offset);
            let built = field.voxelize(cell, [false; 3], crate::live::PREVIEW_MAX_SAMPLES).unwrap();
            assert!((built.grid.cell - cell).abs() < 1e-7, "the cell asked for");
            let positions = built.surface.positions;
            errors.push(positions.iter().map(|&p| field.distance(p).abs()).fold(0.0, f32::max) / cell);
            // How far the surface is from the shape a tenth of a cell back, past that tenth: nothing
            // for a surface that only moves with the shape.
            if step > 0 {
                let before = field_at(offset - direction * cell / 10.0);
                let extra = positions.iter().map(|&p| (before.distance(p).abs() - cell / 10.0).max(0.0)).fold(0.0, f32::max);
                extra_moves.push(extra / cell);
            }
        }
        let (e50, e95, emax) = stats(&errors);
        let (m50, m95, mmax) = stats(&extra_moves);
        println!(
            "PREVIEW {what:<16} cell {:.4} ({:.3} r): surface off the true one by {e50:.3}/{e95:.3}/{emax:.3} cells; per 0.1-cell move, beyond the move {m50:.3}/{m95:.3}/{mmax:.3} cells",
            cell,
            cell / settings.radius
        );
    }
}

// ---- pockets under a muscle ----------------------------------------------------------------------

#[derive(Debug, Default)]
pub(super) struct Pockets {
    pub lines: usize,
    /// Lines up through the muscle's footprint that leave the model into air and then meet the muscle.
    pub live: usize,
    pub live_worst: f32,
    pub merged: usize,
    pub merged_worst: f32,
    /// How far the merged top of the muscle stands off the model at its belly, in radii.
    pub top: f32,
}

/// Lays a muscle across the front of a sphere with `detail` sculpted into it, from x = -0.2 to 0.2 at
/// height y, then merges it, and counts the pockets under it: lines along each station's way up
/// through its footprint (seven across, every other station), from well inside the model, that come
/// out of the model into air and then go into the muscle.
pub(super) fn measure_pockets(detail: &Detail, y: f32, float_end: bool) -> Pockets {
    let (mut bench, mut tool, body, accel) = close_bench(detail);
    let px: Vec<Vec2> = [on_front(-0.2, y), on_front(0.2, y)].iter().map(|&p| bench.px(p)).collect();
    bench.press(&mut tool, px[0], Button::Left);
    for step in 1..=6 {
        bench.drag_to(&mut tool, px[0].lerp(px[1], step as f32 / 6.0));
    }
    bench.release(&mut tool);
    if float_end {
        // The end lifted off the model with Free placement: dragged on out past where the sphere
        // curves away under it.
        tool.set_on_surface(false);
        let end = bench.px(tool.handle_world(0, 1, 0).unwrap());
        bench.drag(&mut tool, end, end + Vec2::new(30.0, 0.0));
        assert!(!tool.points(0)[1].is_on_surface());
    }
    bench.frame(&mut tool);
    let r = tool.settings().radius;
    let samples = tool.tubes[0].copies[0].samples.clone();
    let field = tool.tubes[0].field();
    let mut lines = Vec::new();
    for s in samples.iter().step_by(2) {
        let across = s.up.cross(s.tangent_out).normalize_or_zero();
        for o in [-0.85, -0.55, -0.25, 0.0, 0.25, 0.55, 0.85] {
            let base = s.center + across * (o * s.radius) - s.up * (3.0 * r);
            lines.push((base, s.up));
        }
    }
    let mut pockets = Pockets { lines: lines.len(), ..Pockets::default() };
    let length = 6.0 * r;
    for &(base, up) in &lines {
        // Live: from where the line leaves the model, the first place inside the muscle.
        let Some(&out) = accel.crossings(base, up, length).first() else { continue };
        let mut t = out;
        while t < length && field.distance(base + up * t) > 0.0 {
            t += 0.01 * r;
        }
        if t < length && t - out > 0.03 * r {
            pockets.live += 1;
            pockets.live_worst = pockets.live_worst.max((t - out) / r);
        }
    }
    assert!(bench.key(&mut tool, sculpt_toolkit::Key::Enter));
    super::tool::wait_for_merge(&mut bench, &mut tool);
    let merged = bench.doc.mesh(body).unwrap().to_tri_mesh();
    let merged = Accel::new(merged);
    for &(base, up) in &lines {
        let crossings = merged.crossings(base, up, length);
        if crossings.len() >= 3 {
            pockets.merged += 1;
            pockets.merged_worst = pockets.merged_worst.max((crossings[1] - crossings[0]) / r);
        }
    }
    let belly = &samples[samples.len() / 2];
    let model_top = accel.crossings(belly.center - belly.up * 3.0 * r, belly.up, length).first().copied().unwrap_or(0.0);
    let merged_top = merged.crossings(belly.center - belly.up * 3.0 * r, belly.up, length).last().copied().unwrap_or(0.0);
    pockets.top = (merged_top - model_top) / r;
    pockets
}

#[test]
#[ignore]
fn measure_pocket_cases() {
    let r = 0.045;
    let plain = Detail { subdivisions: 6, ..Detail::PLAIN };
    let cases = [
        ("smooth sphere", plain),
        ("groove 0.6r deep", Detail { groove: 0.6 * r, groove_width: 0.5 * r, ..plain }),
        ("groove 1r deep, wide", Detail { groove: 1.0 * r, groove_width: 0.9 * r, ..plain }),
        ("two ridges", Detail { ridges: 0.5 * r, ridge_width: 0.3 * r, ridge_x: 0.7 * r, ..plain }),
        ("sculpted", Detail { subdivisions: 6, ..Detail::SCULPTED }),
        ("floated end", plain),
    ];
    let lines: Vec<String> = std::thread::scope(|scope| {
        let handles: Vec<_> = cases
            .iter()
            .map(|&(name, detail)| {
                std::thread::Builder::new()
                    .stack_size(16 << 20)
                    .spawn_scoped(scope, move || {
                        let p = measure_pockets(&detail, 0.05, name == "floated end");
                        format!(
                            "POCKETS {name:<22} | live: {}/{} lines, worst {:.3} r | merged: {}/{} lines, worst {:.3} r | merged top {:.3} r",
                            p.live, p.lines, p.live_worst, p.merged, p.lines, p.merged_worst, p.top
                        )
                    })
                    .unwrap()
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for line in lines {
        println!("{line}");
    }
}

// ---- what the fixes hold to ------------------------------------------------------------------------

/// A sculpted surface dense enough that its face normals wander, at a muscle radius of 0.045.
fn rough() -> Detail {
    Detail { subdivisions: 6, ..Detail::SCULPTED }
}

#[test]
fn a_point_dragged_over_a_rough_surface_carries_its_muscle_smoothly() {
    let (mut bench, mut tool, _, accel) = close_bench(&rough());
    let through = [on_front(-0.25, -0.05), on_front(0.2, 0.05)];
    let report = measure_drag(&mut bench, &mut tool, &accel, &through, 1, Vec2::new(5.0, 20.0), &mut |_| {});
    let (_, jerk, _) = stats(&report.jerk);
    let (_, moved, _) = stats(&report.moved);
    let (_, point, _) = stats(&report.point_move);
    // Each pixel the point goes, its muscle goes about as far, and steadily: the point's face normal
    // swings by up to twenty degrees from one face to the next, and once swung the whole muscle about
    // with it, by several times the point's own move.
    assert!(moved < point * 1.6, "the muscle moved {moved} r a pixel where its point moved {point} r");
    assert!(jerk < 0.03, "jerk {jerk} r");
}

#[test]
fn a_muscle_drawn_over_a_rough_surface_holds_still_behind_the_pen() {
    let (mut bench, mut tool, _, accel) = close_bench(&rough());
    let path = hand_path(&bench, on_front(-0.2, -0.05), on_front(0.2, 0.08), 0.0, 0.0, 1.0);
    let report = measure_draw(&mut bench, &mut tool, &accel, &path);
    let (_, moved, _) = stats(&report.head_move);
    let (_, pen, _) = stats(&report.pen_move);
    assert!(moved < 0.25 * pen, "the muscle well behind the pen moved {moved} r a sample where the pen moved {pen} r");
}

#[test]
fn a_merged_muscle_leaves_no_pocket_over_a_groove_or_under_a_lifted_end() {
    let r = 0.045;
    let plain = Detail { subdivisions: 6, ..Detail::PLAIN };
    let groove = Detail { groove: 0.6 * r, groove_width: 0.5 * r, ..plain };
    let pockets = measure_pockets(&groove, 0.05, false);
    assert_eq!((pockets.live, pockets.merged), (0, 0), "{pockets:?}");
    let lifted = measure_pockets(&plain, 0.05, true);
    assert_eq!((lifted.live, lifted.merged), (0, 0), "{lifted:?}");
}
