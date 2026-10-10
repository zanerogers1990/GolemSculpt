//! Symmetry tests: strokes repeated round an axis, on their own and with the
//! mirror axes, and what mirror-only strokes must keep doing.
//!
//! The radial ones are the Unity app's `RadialSymmetryTests`, driven through
//! the pointer like every other stroke test here rather than through the
//! brushes' internals, with more of this app's own after them. Radial
//! sculpting has to keep an N-fold symmetric model N-fold symmetric, stroke
//! after stroke. Unlike a mirror, a turn by 360/N degrees is not exact in
//! floats for most N, so the sectors agree to rounding rather than to the
//! bit, and that is the floor. What the tests exist to catch is the
//! systematic difference: copies applied one after another against the live
//! mesh near the axis, where their footprints overlap, are handed different
//! work (see `radial.rs`).

use glam::{Vec2, Vec3};
use sculpt_geom::{SymmetryGroup, SymmetryOp};

use super::kernels::SymOp;
use super::*;
use crate::camera::{Camera, View};
use crate::mesh::primitives::{self, Primitive};

const SIZE: Vec2 = Vec2::new(1600.0, 900.0);

fn view_of(mesh: &Mesh) -> View {
    let mut camera = Camera::default();
    camera.frame(mesh.bounds(), SIZE.x / SIZE.y, false);
    camera.view(Vec2::ZERO, SIZE, camera.distance, 1.0)
}

fn line(from: Vec2, to: Vec2, n: usize) -> Vec<Vec2> {
    (0..=n).map(|i| from.lerp(to, i as f32 / n as f32)).collect()
}

/// Drags the pointer along `points` (pixels), a few samples per frame.
fn run_stroke(mesh: &mut Mesh, sculptor: &mut Sculptor, view: &View, points: &[Vec2]) -> Option<StrokeDelta> {
    let sample = |i: usize| PointerSample { pos: points[i], pressure: None, time: i as f64 / 240.0 };
    assert!(sculptor.begin(mesh, view, sample(0), false, false), "stroke missed the mesh");
    for i in 1..points.len() {
        sculptor.sample(mesh, view, sample(i));
        if i % 4 == 0 {
            sculptor.frame(mesh, view, 1.0 / 60.0);
            let _ = mesh.take_dirty();
        }
    }
    let delta = sculptor.end(mesh, view);
    let _ = mesh.take_dirty();
    delta
}

// ------------------------------------------------------------ mirrors alone

/// Every bit of a mesh's positions and mask, folded into one number.
fn fingerprint(mesh: &Mesh) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut fold = |bits: u32| {
        for byte in bits.to_le_bytes() {
            hash = (hash ^ byte as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for p in &mesh.positions {
        p.to_array().into_iter().for_each(|c| fold(c.to_bits()));
    }
    mesh.mask.iter().for_each(|m| fold(m.to_bits()));
    hash
}

/// A stroke that starts to one side, crosses the X plane at an angle close to
/// the Y plane and then runs along it: every way mirrored copies can meet.
fn mirror_path() -> Vec<Vec2> {
    let c = SIZE * 0.5;
    let mut path = line(c + Vec2::new(-110.0, 60.0), c + Vec2::new(6.0, 10.0), 40);
    path.extend(line(c + Vec2::new(6.0, 10.0), c + Vec2::new(14.0, -90.0), 40));
    path
}

/// What each brush leaves behind under each set of mirror axes, as a number
/// per stroke. Not a test of anything by itself: run it before and after a
/// change that must leave mirrored sculpting alone bit for bit, and compare
/// the lines (`cargo test mirror_stroke_fingerprints -- --ignored --nocapture`).
#[test]
#[ignore]
fn mirror_stroke_fingerprints() {
    let axes = [[true, false, false], [true, true, false], [true, true, true], [false, true, true]];
    for mask in [false, true] {
        for brush in Brush::ALL {
            if mask && brush != Brush::Clay {
                continue;
            }
            for symmetry in axes {
                let mut mesh = primitives::build(Primitive::Sphere, 80_000, 0.5);
                let view = view_of(&mesh);
                let mut sculptor = Sculptor::default();
                sculptor.settings.brush = brush;
                sculptor.settings.mask_mode = mask;
                sculptor.settings.symmetry = symmetry;
                sculptor.settings.radius_px = 45.0;
                sculptor.settings.per_brush[brush.index()].strength = 0.6;
                run_stroke(&mut mesh, &mut sculptor, &view, &mirror_path());
                // A second stroke over the first, so the surface it meets is
                // no longer a plain ball.
                sculptor.settings.radius_px = 70.0;
                let c = SIZE * 0.5;
                run_stroke(&mut mesh, &mut sculptor, &view, &line(c + Vec2::new(40.0, -60.0), c + Vec2::new(-30.0, 30.0), 50));
                let name = if mask { "Mask" } else { brush.name() };
                println!("fingerprint {name:>10} {symmetry:?}: {:016x}", fingerprint(&mesh));
            }
        }
    }
}

/// The list of copies a mirror-only stroke keeps is what it was before there
/// were turns: the identity, then each combination of the ticked axes with X
/// counting 1, Y 2 and Z 4, every one a per-axis sign flip applied as exactly
/// that multiply. The walk over mirrored copies leans on all three: the
/// order, the signs, and a mirrored point being the original to the bit.
#[test]
fn mirror_only_strokes_keep_the_copies_they_always_had() {
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut random = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 40) as f32 / (1u64 << 24) as f32 - 0.5
    };
    // Radial symmetry set up but switched off, or on with a count that means
    // off, changes nothing.
    let off = [(false, 8), (true, 0), (true, 1)].map(|(on, count)| Radial { on, count, axis: RadialAxis::X, custom_axis: Vec3::ONE });
    for mask in 0..24usize {
        let mut settings = Settings::default();
        settings.symmetry = [mask & 1 != 0, mask & 2 != 0, mask & 4 != 0];
        settings.radial = off[mask / 8];
        let group = settings.symmetry_group();
        assert!(radial::Walk::new(group.clone()).is_none(), "a mirror-only stroke took the radial walk");
        let ops = symmetry::copies(&group);

        // As the list was built before: each axis in turn doubles it.
        let mut signs = vec![Vec3::ONE];
        for axis in 0..3 {
            if settings.symmetry[axis] {
                let mut flip = Vec3::ONE;
                flip[axis] = -1.0;
                let mirrored: Vec<Vec3> = signs.iter().map(|&s| s * flip).collect();
                signs.extend(mirrored);
            }
        }
        assert_eq!(ops.len(), signs.len(), "axes {:?}", settings.symmetry);
        assert_eq!(ops[0], SymOp::IDENTITY);
        for (op, &sign) in ops.iter().zip(&signs) {
            for axis in 0..3 {
                assert_eq!(op.flips(axis), sign[axis] < 0.0);
            }
            for _ in 0..200 {
                let p = Vec3::new(random(), random() * 1e-3, random() * 1e3);
                let same = |a: Vec3, b: Vec3| a.to_array().map(f32::to_bits) == b.to_array().map(f32::to_bits);
                assert!(same(op.apply_point(p), p * sign) && same(op.apply_dir(p), p * sign), "{op:?} on {p}");
            }
        }
    }
}

// ----------------------------------------------------------------- the ball

// The mesh the radial tests sculpt: a ball of rings whose vertex counts
// double toward the equator, so triangles stay even right up to the poles,
// with one more vertex in the middle of every quad (four triangles a quad).
// Its topology maps onto itself under every turn by a multiple of 15 degrees
// about Y and under the X, Y and Z mirrors, so a vertex's partner under each
// symmetry is known by index. (A plain UV sphere's fan of slivers at the pole
// amplifies float noise under Inflate about a hundredfold a frame, for
// mirrors and turns alike, which says nothing about symmetry.)

/// Vertices on the rings nearest the poles: divisible by 3, 4, 6, 8 and 12.
const BASE_COUNT: usize = 24;
/// On the widest rings.
const SEGMENTS: usize = 384;
const RINGS: usize = 192;
const BALL_RADIUS: f32 = 0.5;
const BRUSH_RADIUS: f32 = 0.06;
const STROKE_FRAMES: usize = 16;

// How far apart a vertex and its partner may end up: a fraction of the
// stroke's own displacement, plus a floor. The floor is the size of the step
// Clay's finishing pass leaves out as too small to see (`clay_relax`): a
// vertex a hair over it moves while its twin, a hair under, stays.
const RELATIVE_TOLERANCE: f32 = 1e-4;
const ABSOLUTE_TOLERANCE: f32 = BALL_RADIUS / 4000.0;
const RELATIVE_TOLERANCE_FRONT_FACING: f32 = 2e-2;

/// Which vertex is where: ring `r` (1 to RINGS - 1) has `ring_count[r]`
/// vertices from `ring_start[r]`; band `b`, between rings `b` and `b + 1`, has
/// a centre vertex per quad from `centre_start[b]` when both rings match, and
/// none where the count doubles across it. The poles are vertices 0 (north)
/// and 1 (south).
struct Layout {
    ring_count: Vec<usize>,
    ring_start: Vec<usize>,
    centre_start: Vec<Option<usize>>,
    vertices: usize,
}

impl Layout {
    /// Ring vertex counts are `BASE_COUNT` times a power of two, the largest
    /// that keeps the edge along the ring no shorter than the edge between
    /// rings.
    fn new() -> Layout {
        let mut layout = Layout { ring_count: vec![0; RINGS], ring_start: vec![0; RINGS], centre_start: vec![None; RINGS], vertices: 2 };
        for ring in 1..RINGS {
            let fit = 2.0 * RINGS as f64 * (std::f64::consts::PI * ring as f64 / RINGS as f64).sin();
            let mut count = BASE_COUNT;
            while count * 2 <= SEGMENTS && (count * 2) as f64 <= fit {
                count *= 2;
            }
            layout.ring_count[ring] = count;
            layout.ring_start[ring] = layout.vertices;
            layout.vertices += count;
        }
        for band in 1..RINGS - 1 {
            if layout.ring_count[band] == layout.ring_count[band + 1] {
                layout.centre_start[band] = Some(layout.vertices);
                layout.vertices += layout.ring_count[band];
            }
        }
        layout
    }

    fn ring_vertex(&self, ring: usize, k: i64) -> usize {
        self.ring_start[ring] + k.rem_euclid(self.ring_count[ring] as i64) as usize
    }

    fn centre_vertex(&self, band: usize, k: i64) -> usize {
        self.centre_start[band].expect("a band with centres") + k.rem_euclid(self.ring_count[band] as i64) as usize
    }

    /// Which vertex sits in `dir` (a unit direction): the layout, backwards.
    fn index_of_direction(&self, dir: Vec3) -> usize {
        let polar = (dir.y as f64).clamp(-1.0, 1.0).acos() * RINGS as f64 / std::f64::consts::PI;
        let half_steps = (polar * 2.0).round() as usize;
        if half_steps == 0 {
            return 0;
        }
        if half_steps == 2 * RINGS {
            return 1;
        }
        let turn = (dir.z as f64).atan2(dir.x as f64) / std::f64::consts::TAU;
        let ring = half_steps / 2;
        let count = self.ring_count[ring] as f64;
        if half_steps % 2 == 0 {
            self.ring_vertex(ring, (turn * count).round() as i64)
        } else {
            self.centre_vertex(ring, (turn * count - 0.5).round() as i64)
        }
    }
}

/// The ball's radius in a direction. Symmetric under every turn by a multiple
/// of 360/24 degrees about Y and under the X, Y and Z mirrors, with bumps both
/// ways so no brush meets a flat target.
fn bumpy_radius(polar: f64, azimuth: f64) -> f64 {
    BALL_RADIUS as f64 * (1.0 + 0.04 * (7.0 * polar).sin() + 0.02 * polar.sin() * polar.sin() * (24.0 * azimuth).cos())
}

fn point_on_ball(dir: Vec3) -> Vec3 {
    let polar = (dir.y as f64).clamp(-1.0, 1.0).acos();
    let azimuth = (dir.z as f64).atan2(dir.x as f64);
    dir * bumpy_radius(polar, azimuth) as f32
}

struct Ball {
    mesh: Mesh,
    layout: Layout,
    /// Where every vertex of the mesh started.
    start: Vec<Vec3>,
    /// The direction each vertex was laid out in, by layout index, and where
    /// the mesh keeps that vertex (building it reorders them).
    dirs: Vec<Vec3>,
    in_mesh: Vec<u32>,
}

impl Ball {
    fn new() -> Ball {
        let layout = Layout::new();
        let mut dirs = vec![Vec3::ZERO; layout.vertices];
        let mut positions = vec![Vec3::ZERO; layout.vertices];
        let mut put = |index: usize, polar: f64, azimuth: f64| {
            let d = [polar.sin() * azimuth.cos(), polar.cos(), polar.sin() * azimuth.sin()];
            let r = bumpy_radius(polar, azimuth);
            dirs[index] = Vec3::from_array(d.map(|c| c as f32));
            positions[index] = Vec3::from_array(d.map(|c| (c * r) as f32));
        };
        let (pi, tau) = (std::f64::consts::PI, std::f64::consts::TAU);
        put(0, 0.0, 0.0);
        put(1, pi, 0.0);
        for ring in 1..RINGS {
            let count = layout.ring_count[ring];
            for k in 0..count {
                put(layout.ring_vertex(ring, k as i64), pi * ring as f64 / RINGS as f64, tau * k as f64 / count as f64);
            }
        }
        for band in 1..RINGS - 1 {
            if layout.centre_start[band].is_none() {
                continue;
            }
            let count = layout.ring_count[band];
            for k in 0..count {
                let polar = pi * (band as f64 + 0.5) / RINGS as f64;
                put(layout.centre_vertex(band, k as i64), polar, tau * (k as f64 + 0.5) / count as f64);
            }
        }

        let mut indices: Vec<u32> = Vec::with_capacity(layout.vertices * 6);
        // Outward by geometry, decided per triangle, so the rule is itself symmetric.
        let mut tri = |a: usize, mut b: usize, mut c: usize| {
            let n = (positions[b] - positions[a]).cross(positions[c] - positions[a]);
            if n.dot(positions[a] + positions[b] + positions[c]) < 0.0 {
                std::mem::swap(&mut b, &mut c);
            }
            indices.extend([a as u32, b as u32, c as u32]);
        };
        for k in 0..layout.ring_count[1] as i64 {
            tri(0, layout.ring_vertex(1, k), layout.ring_vertex(1, k + 1));
        }
        for k in 0..layout.ring_count[RINGS - 1] as i64 {
            tri(1, layout.ring_vertex(RINGS - 1, k), layout.ring_vertex(RINGS - 1, k + 1));
        }
        for band in 1..RINGS - 1 {
            if layout.centre_start[band].is_some() {
                // Matching rings: each quad cut in four round its centre vertex.
                for k in 0..layout.ring_count[band] as i64 {
                    let (a, b) = (layout.ring_vertex(band, k), layout.ring_vertex(band, k + 1));
                    let (c, d) = (layout.ring_vertex(band + 1, k + 1), layout.ring_vertex(band + 1, k));
                    let m = layout.centre_vertex(band, k);
                    tri(m, a, b);
                    tri(m, b, c);
                    tri(m, c, d);
                    tri(m, d, a);
                }
                continue;
            }
            // The count doubles: three triangles per edge of the smaller ring,
            // a pattern that is symmetric about the middle of that edge, so
            // it survives every turn and mirror.
            let grows = layout.ring_count[band + 1] > layout.ring_count[band];
            let (small, large) = if grows { (band, band + 1) } else { (band + 1, band) };
            for k in 0..layout.ring_count[small] as i64 {
                let (a0, a1) = (layout.ring_vertex(small, k), layout.ring_vertex(small, k + 1));
                let b = [0, 1, 2].map(|step| layout.ring_vertex(large, 2 * k + step));
                tri(a0, b[0], b[1]);
                tri(a0, b[1], a1);
                tri(a1, b[1], b[2]);
            }
        }

        let (mesh, in_mesh) = Mesh::new_mapped(positions, indices, None);
        assert!(in_mesh.iter().all(|&v| v != u32::MAX), "every vertex of the ball is used");
        Ball { start: mesh.positions.clone(), mesh, layout, dirs, in_mesh }
    }

    /// For every vertex of the mesh, the vertex `op` carries it onto.
    fn partners(&self, op: &SymmetryOp) -> Vec<u32> {
        let mut partners = vec![0; self.dirs.len()];
        for (index, &dir) in self.dirs.iter().enumerate() {
            partners[self.in_mesh[index] as usize] = self.in_mesh[self.layout.index_of_direction(op.apply(dir))];
        }
        partners
    }

    /// The same for every copy of a symmetry but the first, with the copy.
    fn partners_under(&self, group: &SymmetryGroup) -> Vec<(SymmetryOp, Vec<u32>)> {
        group.iter().skip(1).map(|op| (*op, self.partners(op))).collect()
    }

    /// The furthest any vertex has moved from where it started.
    fn displacement(&self) -> f32 {
        self.mesh.positions.iter().zip(&self.start).map(|(a, b)| a.distance_squared(*b)).fold(0.0f32, f32::max).sqrt()
    }

    /// Takes a stroke back, leaving the ball as it was built.
    fn undo(&mut self, delta: Option<StrokeDelta>) {
        if let Some(delta) = delta {
            self.mesh.apply_delta(&delta, true);
            self.mesh.flush_normals();
            let _ = self.mesh.take_dirty();
        }
    }
}

/// The largest distance between where a copy of the symmetry carries a
/// vertex and where that vertex's partner is, over every copy and vertex.
fn pair_error(mesh: &Mesh, partners: &[(SymmetryOp, Vec<u32>)]) -> f32 {
    let mut worst = 0.0f32;
    for (op, partner) in partners {
        for (v, &p) in mesh.positions.iter().zip(partner) {
            worst = worst.max(mesh.positions[p as usize].distance_squared(op.apply(*v)));
        }
    }
    worst.sqrt()
}

/// The same for the mask: the largest difference between a vertex's mask and
/// its partner's.
fn mask_pair_error(mesh: &Mesh, partners: &[(SymmetryOp, Vec<u32>)]) -> f32 {
    let mut worst = 0.0f32;
    for (_, partner) in partners {
        for (m, &p) in mesh.mask.iter().zip(partner) {
            worst = worst.max((mesh.mask[p as usize] - m).abs());
        }
    }
    worst
}

/// The camera the radial strokes are made under: above the ball and to the
/// side the strokes are on, looking down at the pole from an angle.
fn view_from_above(mesh: &Mesh) -> View {
    let mut camera = Camera::default();
    camera.frame(mesh.bounds(), SIZE.x / SIZE.y, false);
    (camera.yaw, camera.pitch) = (STROKE_AZIMUTH.to_degrees() - 90.0, 50.0);
    camera.view(Vec2::ZERO, SIZE, camera.distance, 1.0)
}

/// The angle round Y the strokes are made at: no symmetry plane of any of
/// the symmetries tested.
const STROKE_AZIMUTH: f32 = 0.37;

/// The points of a short stroke on the upper half of the ball, centred
/// `from_axis` away from the Y axis and crossing that spot at a slant, one
/// point per frame.
fn stroke_points(from_axis: f32, frames: usize) -> Vec<Vec3> {
    let polar = (from_axis / BALL_RADIUS).clamp(0.0, 1.0).asin();
    let centre = Vec3::new(polar.sin() * STROKE_AZIMUTH.cos(), polar.cos(), polar.sin() * STROKE_AZIMUTH.sin());
    let across = centre.cross(Vec3::new(0.3, 0.2, 1.0)).normalize();
    (0..frames)
        .map(|f| point_on_ball((centre + across * (0.012 * (f as f32 - STROKE_FRAMES as f32 * 0.5))).normalize()))
        .collect()
}

/// Drags the pointer through `points` on the model, a frame per point with a
/// few samples on the way to each.
fn stroke_through(mesh: &mut Mesh, sculptor: &mut Sculptor, view: &View, points: &[Vec3]) -> Option<StrokeDelta> {
    const STEPS: usize = 4;
    let px: Vec<Vec2> = points.iter().map(|&p| view.project(p).expect("the stroke is in front of the camera")).collect();
    let mut time = 0.0;
    let mut sample = |pos: Vec2| {
        time += 1.0 / 60.0 / STEPS as f64;
        PointerSample { pos, pressure: None, time }
    };
    assert!(sculptor.begin(mesh, view, sample(px[0]), false, false), "the stroke missed the ball");
    sculptor.frame(mesh, view, 1.0 / 60.0);
    for pair in px.windows(2) {
        for step in 1..=STEPS {
            sculptor.sample(mesh, view, sample(pair[0].lerp(pair[1], step as f32 / STEPS as f32)));
        }
        sculptor.frame(mesh, view, 1.0 / 60.0);
        let _ = mesh.take_dirty();
    }
    let delta = sculptor.end(mesh, view);
    let _ = mesh.take_dirty();
    delta
}

/// The brushes the Unity tests drive, as this app sets them up.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Tip {
    Clay,
    ClayAccumulate,
    Crease,
    Inflate,
    Flatten,
    Smooth,
}

impl Tip {
    const ALL: [Tip; 6] = [Tip::Clay, Tip::ClayAccumulate, Tip::Crease, Tip::Inflate, Tip::Flatten, Tip::Smooth];

    fn brush(self) -> Brush {
        match self {
            Tip::Clay | Tip::ClayAccumulate => Brush::Clay,
            Tip::Crease => Brush::Crease,
            Tip::Inflate => Brush::Inflate,
            Tip::Flatten => Brush::Flatten,
            Tip::Smooth => Brush::Smooth,
        }
    }

    fn sculptor(self) -> Sculptor {
        let mut sculptor = sculptor_with(self.brush());
        sculptor.settings.current_mut().accumulate = matches!(self, Tip::ClayAccumulate | Tip::Inflate);
        sculptor
    }
}

/// A sculptor holding `brush` at the tests' size on the model.
fn sculptor_with(brush: Brush) -> Sculptor {
    let mut sculptor = Sculptor::default();
    let s = &mut sculptor.settings;
    s.brush = brush;
    s.screen_space_size = false;
    s.radius_world = BRUSH_RADIUS;
    s.per_brush[brush.index()].strength = 0.5;
    sculptor
}

/// Mirror axes, and repeats about Y.
#[derive(Clone, Copy, Debug)]
struct Symmetry {
    mirror: [bool; 3],
    repeats: u32,
}

const RADIAL_3: Symmetry = Symmetry { mirror: [false; 3], repeats: 3 };
const RADIAL_6: Symmetry = Symmetry { mirror: [false; 3], repeats: 6 };
const RADIAL_8: Symmetry = Symmetry { mirror: [false; 3], repeats: 8 };
const RADIAL_12: Symmetry = Symmetry { mirror: [false; 3], repeats: 12 };
const RADIAL_4_MIRROR_X: Symmetry = Symmetry { mirror: [true, false, false], repeats: 4 };

impl Symmetry {
    fn set(self, settings: &mut Settings) {
        settings.symmetry = self.mirror;
        settings.radial = Radial { on: self.repeats > 1, count: self.repeats, axis: RadialAxis::Y, custom_axis: Vec3::Y };
    }

    fn group(self) -> SymmetryGroup {
        let mut settings = Settings::default();
        self.set(&mut settings);
        settings.symmetry_group()
    }
}

fn assert_symmetric(label: &str, ball: &Ball, partners: &[(SymmetryOp, Vec<u32>)], relative: f32) {
    let pair = pair_error(&ball.mesh, partners);
    let displacement = ball.displacement();
    let report = format!("{label}: pair {pair:.2e}, displacement {displacement:.3e}");
    println!("{report}");
    assert!(displacement > 5e-5, "{report}: the stroke barely moved the mesh, so symmetry would prove nothing");
    let allowed = relative * displacement + ABSOLUTE_TOLERANCE;
    assert!(pair <= allowed, "{report}: allowed {allowed:.2e}");
}

// ------------------------------------------------- the Unity app's tests

#[test]
fn the_ball_is_radially_symmetric() {
    let ball = Ball::new();
    for symmetry in [RADIAL_3, RADIAL_6, RADIAL_8, RADIAL_12, RADIAL_4_MIRROR_X] {
        let error = pair_error(&ball.mesh, &ball.partners_under(&symmetry.group()));
        // Not zero: a turn by 360/N rounds. It is the floor every stroke is measured against.
        assert!(error < 1e-6, "{symmetry:?}: {error:.2e}");
    }
}

/// On the axis (every copy lands on the same dab), just off it (all copies
/// overlap), two and a half radii out (neighbouring copies overlap for six
/// repeats and up) and far out (none do).
#[test]
fn a_radial_stroke_stays_symmetric() {
    let mut ball = Ball::new();
    let view = view_from_above(&ball.mesh);
    for symmetry in [RADIAL_3, RADIAL_6, RADIAL_8, RADIAL_12, RADIAL_4_MIRROR_X] {
        let partners = ball.partners_under(&symmetry.group());
        for tip in Tip::ALL {
            for radii_from_axis in [0.0, 0.6, 2.5, 6.0] {
                let mut sculptor = tip.sculptor();
                symmetry.set(&mut sculptor.settings);
                let points = stroke_points(radii_from_axis * BRUSH_RADIUS, STROKE_FRAMES);
                let delta = stroke_through(&mut ball.mesh, &mut sculptor, &view, &points);
                assert_symmetric(&format!("{tip:?} {symmetry:?} {radii_from_axis}r"), &ball, &partners, RELATIVE_TOLERANCE);
                ball.undo(delta);
            }
        }
    }
}

/// Each copy is judged from its own turned viewpoint.
#[test]
fn a_radial_stroke_with_front_facing_only_stays_symmetric() {
    let mut ball = Ball::new();
    let view = view_from_above(&ball.mesh);
    let partners = ball.partners_under(&RADIAL_6.group());
    for tip in [Tip::Clay, Tip::Crease, Tip::Inflate] {
        for radii_from_axis in [0.6, 2.5, 6.0] {
            let mut sculptor = tip.sculptor();
            RADIAL_6.set(&mut sculptor.settings);
            sculptor.settings.current_mut().front_facing = true;
            let points = stroke_points(radii_from_axis * BRUSH_RADIUS, STROKE_FRAMES);
            let delta = stroke_through(&mut ball.mesh, &mut sculptor, &view, &points);
            assert_symmetric(&format!("{tip:?} front facing {radii_from_axis}r"), &ball, &partners, RELATIVE_TOLERANCE_FRONT_FACING);
            ball.undo(delta);
        }
    }
}

/// Copies that land on top of each other at the pole act as one dab, not N
/// stacked ones: a stroke across the pole moves the surface about as far with
/// 12 repeats as with none, where applying the copies one after another
/// would have moved it up to 12 times as far.
#[test]
fn coincident_copies_at_the_pole_act_as_one_dab() {
    let mut ball = Ball::new();
    let view = view_from_above(&ball.mesh);
    let points = stroke_points(0.0, STROKE_FRAMES);
    for tip in [Tip::Clay, Tip::ClayAccumulate, Tip::Crease, Tip::Inflate] {
        let mut displacement = |symmetry: Option<Symmetry>| {
            let mut sculptor = tip.sculptor();
            if let Some(symmetry) = symmetry {
                symmetry.set(&mut sculptor.settings);
            }
            let delta = stroke_through(&mut ball.mesh, &mut sculptor, &view, &points);
            let moved = ball.displacement();
            ball.undo(delta);
            moved
        };
        let (single, radial) = (displacement(None), displacement(Some(RADIAL_12)));
        let report = format!("{tip:?}: no symmetry {single:.3e}, radial 12 {radial:.3e} ({:.2}x)", radial / single);
        println!("{report}");
        assert!((0.5..=2.0).contains(&(radial / single)), "{report}");
    }
}

/// The largest symmetry there is to tick, 32 repeats with a mirror, 64
/// copies, through Clay and its finishing pass, which is handed a centre per
/// copy of every dab. Reaching the end of the stroke is the test: the Unity
/// app's pass thinned its centres down to a cap and once looped for ever on
/// more copies than the cap.
#[test]
fn more_copies_than_a_frame_of_clay_usually_has_do_not_stall() {
    let mut ball = Ball::new();
    let view = view_from_above(&ball.mesh);
    let mut sculptor = sculptor_with(Brush::Clay);
    Symmetry { mirror: [true, false, false], repeats: Radial::MAX_COUNT }.set(&mut sculptor.settings);
    sculptor.settings.clay_relax = 0.22;
    assert_eq!(sculptor.settings.symmetry_group().len(), 2 * Radial::MAX_COUNT as usize);
    stroke_through(&mut ball.mesh, &mut sculptor, &view, &stroke_points(6.0 * BRUSH_RADIUS, 3));
    assert!(ball.displacement() > 0.0);
}

// ------------------------------------------------------ this app's own tests

/// One stroke under the sculptor's own symmetry, checked and taken back.
fn stroke_and_check(ball: &mut Ball, view: &View, sculptor: &mut Sculptor, from_axis: f32, label: &str) {
    let partners = ball.partners_under(&sculptor.settings.symmetry_group());
    let delta = stroke_through(&mut ball.mesh, sculptor, view, &stroke_points(from_axis, STROKE_FRAMES));
    assert_symmetric(label, ball, &partners, RELATIVE_TOLERANCE);
    ball.undo(delta);
}

/// A stroke with N repeats leaves a mesh that maps onto itself under a turn
/// of one Nth: every vertex, turned, lands on its partner to within a
/// ten-thousandth of the furthest the stroke moved anything, plus a
/// four-thousandth of the ball's radius (the step Clay's finishing pass
/// treats as too small to make).
#[test]
fn a_stroke_with_n_repeats_maps_onto_itself_under_a_turn_of_one_nth() {
    let mut ball = Ball::new();
    let view = view_from_above(&ball.mesh);
    // Every count the ball's own 24-fold layout can be checked for.
    for repeats in [2, 3, 4, 6, 8, 12, 24] {
        let one_nth = SymmetryOp::rotation(Vec3::Y, 1, repeats as i32);
        let turn = [(one_nth, ball.partners(&one_nth))];
        for (brush, from_axis) in [(Brush::Clay, 0.8), (Brush::Standard, 3.0), (Brush::Inflate, 1.7)] {
            let mut sculptor = sculptor_with(brush);
            Symmetry { mirror: [false; 3], repeats }.set(&mut sculptor.settings);
            let points = stroke_points(from_axis * BRUSH_RADIUS, STROKE_FRAMES);
            let delta = stroke_through(&mut ball.mesh, &mut sculptor, &view, &points);
            assert_symmetric(&format!("{brush:?} x{repeats} at {from_axis}r, one turn"), &ball, &turn, RELATIVE_TOLERANCE);
            ball.undo(delta);
        }
    }
}

/// Counts the ball's layout cannot be checked for by index, about an axis
/// that is none of the object's own: on a plain sphere, every point the
/// stroke raised is found again, as high, one turn of 1/N further round.
/// "As high" is to within 2% of the stroke's own height plus a
/// ten-thousandth of the radius: what the facets of two differently
/// triangulated patches of the same bump differ by.
#[test]
fn odd_counts_about_a_custom_axis_repeat_the_stroke_all_the_way_round() {
    let axis = Vec3::new(0.25, 0.3, 1.0).normalize();
    for repeats in [5u32, 7] {
        let mut mesh = primitives::build(Primitive::Sphere, 320_000, 0.5);
        let view = view_of(&mesh);
        let mut sculptor = sculptor_with(Brush::Standard);
        sculptor.settings.radial = Radial { on: true, count: repeats, axis: RadialAxis::Custom, custom_axis: axis * 3.0 };
        assert_eq!(sculptor.settings.symmetry_group().len(), repeats as usize);
        let c = SIZE * 0.5;
        run_stroke(&mut mesh, &mut sculptor, &view, &line(c + Vec2::new(95.0, -40.0), c + Vec2::new(150.0, 25.0), 40));

        let height = |p: Vec3| p.length() - 0.5;
        let tallest = mesh.positions.iter().map(|&p| height(p)).fold(0.0f32, f32::max);
        assert!(tallest > 0.005, "x{repeats}: the stroke raised only {tallest}");
        let turn = SymmetryOp::rotation(axis, 1, repeats as i32);
        let (mut checked, mut worst) = (0, 0.0f32);
        for &p in mesh.positions.iter().filter(|&&p| height(p) > 0.25 * tallest).step_by(7) {
            let turned = turn.apply(p);
            let out = turned.normalize();
            let hit = mesh.raycast(out * 2.0, -out).expect("the ball is under every direction");
            worst = worst.max((height(hit.pos) - height(turned)).abs());
            checked += 1;
        }
        println!("x{repeats} about a custom axis: {checked} points, tallest {tallest:.4}, worst {worst:.2e}");
        assert!(checked > 100, "x{repeats}: only {checked} raised points");
        assert!(worst <= 0.02 * tallest + 0.5e-4, "x{repeats}: a turned point is off the surface by {worst:.2e}");
    }
}

/// The largest difference in how far two neighbouring vertices moved: a
/// tear, or a cliff, shows up as an edge whose ends went different ways.
fn roughest_edge(ball: &Ball) -> f32 {
    let moved = |v: u32| ball.mesh.positions[v as usize] - ball.start[v as usize];
    let mut worst = 0.0f32;
    for t in ball.mesh.indices.chunks_exact(3) {
        for k in 0..3 {
            worst = worst.max(moved(t[k]).distance_squared(moved(t[(k + 1) % 3])));
        }
    }
    worst.sqrt()
}

/// A stroke through the axis, where every copy lands on every other: the
/// surface moves about as far as it does with no symmetry at all (not N
/// times as far), and no more unevenly (no cliff where the copies' cover
/// thins out, no tear between sectors).
#[test]
fn a_stroke_through_the_axis_neither_piles_up_nor_tears() {
    let mut ball = Ball::new();
    let view = view_from_above(&ball.mesh);
    let points = stroke_points(0.0, STROKE_FRAMES);
    let tips: [(&str, Brush, bool); 8] = [
        ("Clay", Brush::Clay, false),
        ("Clay, accumulating", Brush::Clay, true),
        ("Crease", Brush::Crease, false),
        ("Inflate", Brush::Inflate, true),
        ("Standard", Brush::Standard, true),
        ("Layer", Brush::Layer, false),
        ("Flatten", Brush::Flatten, false),
        ("Smooth", Brush::Smooth, false),
    ];
    for (name, brush, accumulate) in tips {
        for symmetry in [RADIAL_12, Symmetry { mirror: [true, false, false], repeats: 32 }] {
            let mut made = |symmetry: Option<Symmetry>| {
                let mut sculptor = sculptor_with(brush);
                sculptor.settings.current_mut().accumulate = accumulate;
                if let Some(symmetry) = symmetry {
                    symmetry.set(&mut sculptor.settings);
                }
                let delta = stroke_through(&mut ball.mesh, &mut sculptor, &view, &points);
                let result = (ball.displacement(), roughest_edge(&ball));
                assert!(ball.mesh.positions.iter().all(|p| p.is_finite()), "{name}: a vertex went to infinity");
                ball.undo(delta);
                result
            };
            let ((plain, plain_rough), (radial, radial_rough)) = (made(None), made(Some(symmetry)));
            let report = format!(
                "{name} x{}: moved {plain:.3e} alone and {radial:.3e} repeated ({:.2}x), roughest edge {plain_rough:.2e} and {radial_rough:.2e}",
                symmetry.group().len(),
                radial / plain
            );
            println!("{report}");
            // Not below 0.4 either: Inflate comes out lowest, a little over
            // half, because a ring of copies spreads what a line of dabs
            // through one spot would have piled on it.
            assert!((0.4..=2.0).contains(&(radial / plain)), "{report}");
            assert!(radial_rough <= 2.0 * plain_rough + 1e-5, "{report}");
        }
    }
}

/// The grab brushes at the axis. Every copy of a drag that starts on the
/// axis pulls the same vertices its own way, and the pulls are shared out: a
/// vertex on the axis follows the part of the drag that runs along the axis
/// and stays on it, instead of being carried off N times over.
#[test]
fn a_grab_on_the_axis_keeps_the_axis_where_it_is() {
    let mut ball = Ball::new();
    let view = view_from_above(&ball.mesh);
    let pole = ball.in_mesh[0] as usize;
    let top = ball.start[pole];
    // Dragged across the screen from the pole: sideways, and a little up.
    let from = view.project(top).unwrap();
    let path = line(from, from + Vec2::new(70.0, -25.0), 24);
    for brush in [Brush::Move, Brush::SnakeHook] {
        let mut dragged = |symmetry: Option<Symmetry>| {
            let mut sculptor = sculptor_with(brush);
            sculptor.settings.radius_world = 2.0 * BRUSH_RADIUS;
            if let Some(symmetry) = symmetry {
                symmetry.set(&mut sculptor.settings);
            }
            let partners = ball.partners_under(&sculptor.settings.symmetry_group());
            let delta = run_stroke(&mut ball.mesh, &mut sculptor, &view, &path);
            let moved = ball.mesh.positions[pole] - top;
            let result = (moved, ball.displacement(), roughest_edge(&ball), pair_error(&ball.mesh, &partners));
            ball.undo(delta);
            result
        };
        let (alone, alone_most, alone_rough, _) = dragged(None);
        let (repeated, most, rough, pair) = dragged(Some(RADIAL_12));
        println!("{brush:?}: the pole went {alone} alone and {repeated} repeated; most {alone_most:.3e} and {most:.3e}; pair {pair:.2e}");
        assert!(alone.x.hypot(alone.z) > 0.01, "{brush:?}: the drag did not carry the pole sideways: {alone}");
        assert!(repeated.x.hypot(repeated.z) < 1e-5, "{brush:?}: the pole left the axis: {repeated}");
        assert!(most <= alone_most * 1.05 && most > 0.0, "{brush:?}: {most:.3e} against {alone_most:.3e} alone");
        assert!(rough <= 2.0 * alone_rough + 1e-5, "{brush:?}: roughest edge {rough:.2e} against {alone_rough:.2e}");
        assert!(pair <= RELATIVE_TOLERANCE * most + ABSOLUTE_TOLERANCE, "{brush:?}: pair {pair:.2e}");
    }
}

/// Move's Connected Only under radial symmetry: each copy of the grab keeps
/// the piece of surface under its own centre. Two balls either side of the
/// axis with a small gap between them, two repeats: the grab on one ball
/// reaches across the gap, and so does its copy on the other.
#[test]
fn connected_only_holds_for_every_copy_of_a_radial_grab() {
    let ball = primitives::build(Primitive::Sphere, 20_000, 0.5);
    let count = ball.vertex_count() as u32;
    let (mut positions, mut indices) = (Vec::new(), Vec::new());
    for (k, x) in [-0.22f32, 0.22].into_iter().enumerate() {
        positions.extend(ball.positions.iter().map(|&p| p * 0.4 + Vec3::new(x, 0.0, 0.0)));
        indices.extend(ball.indices.iter().map(|&i| i + count * k as u32));
    }
    // On the left ball, close to the gap: the ring reaches the right one.
    let at = Vec3::new(-0.03, 0.0, (0.04f32 - 0.19 * 0.19).sqrt());
    let dragged = |connected: bool| {
        let mut mesh = Mesh::new(positions.clone(), indices.clone(), None);
        let before = mesh.positions.clone();
        let view = view_of(&mesh);
        let mut sculptor = Sculptor::default();
        let s = &mut sculptor.settings;
        (s.brush, s.screen_space_size, s.radius_world, s.move_connected) = (Brush::Move, false, 0.15, connected);
        s.radial = Radial { on: true, count: 2, ..Radial::default() };
        // Up the screen and to the right; the half turn about Y sends the
        // copy's pull up and to the left.
        let from = view.project(at).unwrap();
        run_stroke(&mut mesh, &mut sculptor, &view, &line(from, from + Vec2::new(30.0, -40.0), 12));
        let moved: Vec<(Vec3, Vec3)> =
            before.iter().zip(&mesh.positions).filter(|(b, a)| a != b).map(|(&b, &a)| (b, a - b)).collect();
        moved
    };
    let (all, connected) = (dragged(false), dragged(true));
    // A vertex pulled by the grab alone goes right, by its copy alone left.
    let against = |moved: &[(Vec3, Vec3)]| moved.iter().filter(|(at, by)| at.x * by.x > 1e-7).count();
    assert!(connected.len() > 200 && all.len() > connected.len(), "{} / {}", connected.len(), all.len());
    assert!(against(&all) > 0, "the ring should reach the other ball, or the test proves nothing");
    assert_eq!(against(&connected), 0, "a copy of the grab pulled on the other copy's ball");
    let (left, right) = (connected.iter().filter(|m| m.0.x < 0.0).count(), connected.iter().filter(|m| m.0.x > 0.0).count());
    assert!(left > 100 && left == right, "{left} / {right}");
}

/// One repeat is the stroke on its own, to the bit, and so is none; two is
/// the stroke and its half turn, which about a coordinate axis is a plain
/// sign flip.
#[test]
fn one_repeat_is_no_symmetry_and_two_is_a_half_turn() {
    let mut ball = Ball::new();
    let view = view_from_above(&ball.mesh);
    let points = stroke_points(1.2 * BRUSH_RADIUS, STROKE_FRAMES);
    let mut made = |radial: Radial| {
        let mut sculptor = sculptor_with(Brush::Clay);
        sculptor.settings.radial = radial;
        let copies = sculptor.settings.symmetry_group().len();
        let delta = stroke_through(&mut ball.mesh, &mut sculptor, &view, &points);
        let result = (copies, fingerprint(&ball.mesh));
        ball.undo(delta);
        result
    };
    let off = made(Radial::default());
    assert_eq!(off.0, 1);
    for count in [0, 1] {
        assert_eq!(made(Radial { on: true, count, ..Radial::default() }), off, "{count} repeats is not the stroke alone");
    }
    let two = made(Radial { on: true, count: 2, ..Radial::default() });
    assert_eq!(two.0, 2);
    assert_ne!(two.1, off.1);
    // More than there can be is the most there can be.
    assert_eq!(made(Radial { on: true, count: 4000, ..Radial::default() }).0, Radial::MAX_COUNT as usize);

    let half_turn = Symmetry { mirror: [false; 3], repeats: 2 };
    assert_eq!(half_turn.group()[1], SymmetryOp::from_sign(Vec3::new(-1.0, 1.0, -1.0)));
    for (tip, from_axis) in [(Tip::Clay, 0.0), (Tip::Clay, 0.7), (Tip::Inflate, 1.2), (Tip::Crease, 4.0), (Tip::Smooth, 0.4)] {
        let mut sculptor = tip.sculptor();
        half_turn.set(&mut sculptor.settings);
        stroke_and_check(&mut ball, &view, &mut sculptor, from_axis * BRUSH_RADIUS, &format!("{tip:?} x2 at {from_axis}r"));
    }
}

/// Mirrors and turns together: a mirror plane through the axis (each turn
/// and its reflection), one across it (the stroke repeated on the far half
/// of the ball as well), both, and all three planes with an odd count, where
/// no two settings make the same copy.
#[test]
fn mirror_and_radial_symmetry_combine() {
    let mut ball = Ball::new();
    let view = view_from_above(&ball.mesh);
    let cases = [
        (Symmetry { mirror: [true, false, false], repeats: 3 }, 6),
        (Symmetry { mirror: [true, false, false], repeats: 4 }, 8),
        (Symmetry { mirror: [false, true, false], repeats: 6 }, 12),
        // Mirror Z after mirror X is the half turn an even count already has.
        (Symmetry { mirror: [true, false, true], repeats: 4 }, 8),
        (Symmetry { mirror: [true, true, false], repeats: 4 }, 16),
        (Symmetry { mirror: [true, true, true], repeats: 3 }, 24),
    ];
    for (symmetry, copies) in cases {
        let group = symmetry.group();
        assert_eq!(group.len(), copies, "{symmetry:?}");
        assert!(group.is_group());
        // On the axis, just off it, a few radii out and well clear of it.
        for (tip, from_axis) in [(Tip::Clay, 0.0), (Tip::Inflate, 0.6), (Tip::Crease, 2.5), (Tip::ClayAccumulate, 6.0)] {
            let mut sculptor = tip.sculptor();
            symmetry.set(&mut sculptor.settings);
            let label = format!("{tip:?} {symmetry:?} at {from_axis}r");
            stroke_and_check(&mut ball, &view, &mut sculptor, from_axis * BRUSH_RADIUS, &label);
        }
    }
}

/// Every brush, and mask painting, under six repeats and under four with a
/// mirror: next to the axis, where every copy overlaps, and clear of it.
#[test]
fn every_brush_stays_symmetric_under_radial_symmetry() {
    let mut ball = Ball::new();
    let view = view_from_above(&ball.mesh);
    for symmetry in [RADIAL_6, RADIAL_4_MIRROR_X] {
        let partners = ball.partners_under(&symmetry.group());
        for from_axis in [0.6, 6.0] {
            let points = stroke_points(from_axis * BRUSH_RADIUS, STROKE_FRAMES);
            for brush in Brush::ALL {
                let mut sculptor = sculptor_with(brush);
                symmetry.set(&mut sculptor.settings);
                let delta = stroke_through(&mut ball.mesh, &mut sculptor, &view, &points);
                assert_symmetric(&format!("{brush:?} {symmetry:?} at {from_axis}r"), &ball, &partners, RELATIVE_TOLERANCE);
                ball.undo(delta);
                assert!(ball.mesh.positions == ball.start, "{brush:?}: undo did not put every copy of the stroke back");
            }

            let mut sculptor = sculptor_with(Brush::Clay);
            symmetry.set(&mut sculptor.settings);
            sculptor.settings.mask_mode = true;
            sculptor.settings.mask_hardness = 0.2;
            let delta = stroke_through(&mut ball.mesh, &mut sculptor, &view, &points);
            assert!(matches!(delta, Some(StrokeDelta::Mask { .. })));
            let masked = ball.mesh.mask.iter().filter(|&&m| m > 0.0).count();
            let error = mask_pair_error(&ball.mesh, &partners);
            println!("Mask {symmetry:?} at {from_axis}r: {masked} vertices, pair {error:.2e}");
            assert!(masked > 200, "the mask stroke painted only {masked} vertices");
            assert!(ball.displacement() == 0.0, "painting the mask moved the surface");
            assert!(error < 1e-4, "a vertex and its partner carry masks {error:.2e} apart");
            ball.undo(delta);
            assert!(ball.mesh.mask.iter().all(|&m| m == 0.0));
        }
    }
}

/// What strokes cost under radial symmetry on a million triangles, through
/// the axis (where every copy meets every other) and clear of it:
/// `cargo test --release timing_of_radial_strokes -- --ignored --nocapture`.
#[test]
#[ignore]
fn timing_of_radial_strokes() {
    const SCREEN: Vec2 = Vec2::new(1920.0, 1080.0);
    let mut mesh = primitives::build(Primitive::Sphere, 1_000_000, 0.5);
    let mut camera = Camera::default();
    camera.frame(mesh.bounds(), SCREEN.x / SCREEN.y, false);
    // Straight down the Y axis, so the pole is in the middle of the screen.
    camera.pitch = 90.0;
    let symmetries = [
        ("none", Symmetry { mirror: [false; 3], repeats: 1 }),
        ("mirror X", Symmetry { mirror: [true, false, false], repeats: 1 }),
        ("x8", RADIAL_8),
        ("x32", Symmetry { mirror: [false; 3], repeats: 32 }),
        ("x32 + mirror X", Symmetry { mirror: [true, false, false], repeats: 32 }),
    ];
    println!("mean / worst ms per frame of 4 pointer samples, at r = 40 px and r = 150 px");
    for brush in [Brush::Clay, Brush::Inflate, Brush::Smooth, Brush::Crease, Brush::SnakeHook, Brush::Move] {
        for (place, from, to) in [("through the axis", Vec2::new(-130.0, -40.0), Vec2::new(130.0, 40.0)), ("clear of it", Vec2::new(150.0, 0.0), Vec2::new(250.0, 120.0))] {
            for (name, symmetry) in symmetries {
                let mut line = format!("{:>10} {place:>16} {name:>15}", brush.name());
                for radius_px in [40.0, 150.0] {
                    let mut sculptor = Sculptor::default();
                    sculptor.settings.brush = brush;
                    sculptor.settings.radius_px = radius_px;
                    sculptor.settings.per_brush[brush.index()].strength = 0.5;
                    symmetry.set(&mut sculptor.settings);
                    let path = |t: f32| SCREEN * 0.5 + from.lerp(to, t);
                    let (mean, worst, verts) = crate::bench::stroke(&mut mesh, &mut sculptor, &camera, path, 40, 4);
                    line += &format!("  {mean:>7.2} / {worst:<7.2} ({}k verts)", verts / 1000);
                }
                println!("{line}");
            }
        }
    }
}

/// A custom axis tilted against a mirror plane makes copies that are not a
/// closed set (the mirror image of a turn is no turn about the same axis),
/// so nothing can be shared out between them: they are applied one after
/// another, each copy once.
#[test]
fn a_custom_axis_tilted_against_a_mirror_plane_still_sculpts_every_copy() {
    let mut mesh = primitives::build(Primitive::Sphere, 80_000, 0.5);
    let view = view_of(&mesh);
    let mut sculptor = sculptor_with(Brush::Inflate);
    sculptor.settings.symmetry = [true, false, false];
    sculptor.settings.radial = Radial { on: true, count: 3, axis: RadialAxis::Custom, custom_axis: Vec3::new(0.3, 1.0, 0.2) };
    let group = sculptor.settings.symmetry_group();
    assert_eq!(group.len(), 6);
    assert!(!group.is_group());
    let centre = Vec3::new(0.2, 0.1, (0.25f32 - 0.05).sqrt());
    let at = view.project(centre).unwrap();
    run_stroke(&mut mesh, &mut sculptor, &view, &line(at, at + Vec2::new(12.0, 0.0), 8));
    for op in group.iter() {
        let image = op.apply(centre);
        let raised = mesh.positions.iter().filter(|p| p.distance(image) < BRUSH_RADIUS && p.length() > 0.502).count();
        assert!(raised > 5, "nothing was raised round the copy at {image}");
    }
}
