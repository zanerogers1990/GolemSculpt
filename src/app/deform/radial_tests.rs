//! The Lattice under radial symmetry on real meshes: a symmetric model stays
//! symmetric (mirror planes too, the gizmo turned, the object turned and
//! stretched, the world's symmetry), a point on the axis keeps to it, copies
//! crowded together are one drag, a point's copy among the cage is not
//! counted twice, undo and cancel are as they were, and without turns
//! nothing changes.

use glam::{DAffine3, DQuat, DVec3, Mat4, Quat, Vec3};

use super::{copies, ghosts, hold};
use crate::deform::{Deformer, Frame, Kind, Lattice, Params, Target};
use crate::history::History;
use crate::mesh::Mesh;
use crate::mesh::layers::Ride;
use crate::scene::{Space, Symmetry};
use crate::sculpt::{Radial, RadialAxis};

const AT_REST: Frame = Frame { origin: DVec3::ZERO, rotation: DQuat::IDENTITY };

fn bits(points: &[Vec3]) -> Vec<[u32; 3]> {
    points.iter().map(|p| p.to_array().map(f32::to_bits)).collect()
}

/// A closed surface of `rings` bands from pole to pole and `segments`
/// round, about Y: a ball of radius 0.5 pushed out into `points` lobes by
/// `lobes` (0 for a ball). The lobes are placed so that the surface is its
/// own mirror image across X and Z as well as `points`-fold round Y, when
/// `segments` is a multiple of `points` and of 4. Worked out in doubles,
/// so it is symmetric to the rounding of a float.
struct Ball {
    mesh: Mesh,
    rings: usize,
    segments: usize,
}

fn ball(rings: usize, segments: usize, points: u32, lobes: f64) -> Ball {
    use std::f64::consts::{FRAC_PI_2, TAU};
    let at = |r: usize, s: usize| {
        let theta = std::f64::consts::PI * r as f64 / rings as f64;
        let phi = TAU * s as f64 / segments as f64;
        let radius = 0.5 * (1.0 + lobes * (points as f64 * (phi - FRAC_PI_2)).cos() * theta.sin());
        DVec3::new(radius * theta.sin() * phi.cos(), radius * theta.cos(), radius * theta.sin() * phi.sin()).as_vec3()
    };
    let mut positions = vec![Vec3::new(0.0, 0.5, 0.0)];
    for r in 1..rings {
        positions.extend((0..segments).map(|s| at(r, s)));
    }
    positions.push(Vec3::new(0.0, -0.5, 0.0));
    let bottom = positions.len() as u32 - 1;
    let ring = |r: usize, s: usize| (1 + (r - 1) * segments + s % segments) as u32;
    let mut indices = Vec::new();
    for s in 0..segments {
        indices.extend_from_slice(&[0, ring(1, s + 1), ring(1, s)]);
        for r in 1..rings - 1 {
            let (a, b, c, d) = (ring(r, s), ring(r, s + 1), ring(r + 1, s + 1), ring(r + 1, s));
            indices.extend_from_slice(&[a, b, c, a, c, d]);
        }
        indices.extend_from_slice(&[bottom, ring(rings - 1, s), ring(rings - 1, s + 1)]);
    }
    Ball { mesh: Mesh::new(positions, indices, None), rings, segments }
}

impl Ball {
    /// Which band and which step round a place on the undeformed ball (in
    /// its own space) is at: the mesh sorts its vertices, so they are found
    /// by where they are, not by number.
    fn key(&self, p: DVec3) -> (usize, usize) {
        use std::f64::consts::{PI, TAU};
        let r = ((p.x.hypot(p.z)).atan2(p.y) / PI * self.rings as f64).round() as usize;
        if r == 0 || r >= self.rings {
            return (r.min(self.rings), 0);
        }
        let s = (p.z.atan2(p.x).rem_euclid(TAU) / TAU * self.segments as f64).round() as usize % self.segments;
        (r, s)
    }

    /// For each of `world` (maps of the world) and each vertex, the vertex
    /// its image is, with the ball placed by `model` and not yet deformed.
    fn pairs(&self, model: Mat4, world: &[DAffine3]) -> Vec<Vec<usize>> {
        let model = DAffine3::from_mat4(model.as_dmat4());
        let inverse = model.inverse();
        let at = |v: usize| self.mesh.positions[v].as_dvec3();
        let by_key: std::collections::HashMap<(usize, usize), usize> =
            (0..self.mesh.vertex_count()).map(|v| (self.key(at(v)), v)).collect();
        assert_eq!(by_key.len(), self.mesh.vertex_count(), "every vertex has a place of its own");
        world
            .iter()
            .map(|op| {
                let lift = inverse * *op * model;
                (0..self.mesh.vertex_count()).map(|v| by_key[&self.key(lift.transform_point3(at(v)))]).collect()
            })
            .collect()
    }

    /// How far the ball is from being its own image under each of `world`,
    /// as `pairs` pairs its vertices: for every vertex, the distance from
    /// its image to the vertex that should be there, at the most, as a
    /// share of the ball's size.
    fn asymmetry(&self, model: Mat4, world: &[DAffine3], pairs: &[Vec<usize>]) -> f64 {
        let model = DAffine3::from_mat4(model.as_dmat4());
        let p = |v: usize| model.transform_point3(self.mesh.positions[v].as_dvec3());
        let mut worst = 0.0f64;
        for (op, pairs) in world.iter().zip(pairs) {
            for (v, &there) in pairs.iter().enumerate() {
                worst = worst.max(op.transform_point3(p(v)).distance(p(there)));
            }
        }
        worst / f64::from(self.mesh.bounds().size().max_element())
    }

    /// The two vertices on the axis.
    fn poles(&self) -> Vec<usize> {
        let band = |v: usize| self.key(self.mesh.positions[v].as_dvec3()).0;
        let poles: Vec<usize> = (0..self.mesh.vertex_count()).filter(|&v| band(v) == 0 || band(v) == self.rings).collect();
        assert_eq!(poles.len(), 2);
        poles
    }
}

fn round_y(count: u32) -> Radial {
    Radial { on: true, count, axis: RadialAxis::Y, custom_axis: Vec3::Y }
}

fn lattice(count: [usize; 3], moves: &[([usize; 3], [f32; 3])]) -> Params {
    let mut cage = Lattice::default().with_count(count);
    for &(ijk, by) in moves {
        let i = cage.index(ijk);
        cage.moved[i] = by;
    }
    Params { kind: Kind::Lattice, lattice: cage, ..Params::default() }
}

/// The copies of `symmetry` as maps of the world, for measuring by.
fn in_world(symmetry: &Symmetry, model: Mat4) -> Vec<DAffine3> {
    // The copies in the frame of the world itself are the world's maps.
    copies(symmetry, model, &AT_REST).unwrap()
}

/// Puts `params` on the ball under `symmetry`, placed by `model` and
/// deformed in `frame`, and gives back what the deformation keeps.
fn deformed(ball: &mut Ball, symmetry: &Symmetry, model: Mat4, frame: &Frame, params: &Params) -> Target {
    let copies = copies(symmetry, model, frame).expect("there are turns in it");
    let mut target = Target::begin(&ball.mesh, model, frame);
    let deformer = Deformer::new(params, &target.bounds).unwrap().repeated(&copies);
    target.apply(&mut ball.mesh, Some(&deformer), params.axis);
    target
}

#[test]
fn a_radially_symmetric_model_stays_symmetric_however_the_cage_lies() {
    let turned = Mat4::from_scale_rotation_translation(
        Vec3::new(1.0, 1.3, 0.8),
        Quat::from_rotation_z(0.4) * Quat::from_rotation_x(-0.3),
        Vec3::new(0.3, -0.2, 0.5),
    );
    // On the world's axis, turned an eighth of a half turn: the world's
    // mirror planes are still the ball's.
    let on_the_axis = Mat4::from_scale_rotation_translation(
        Vec3::splat(1.2),
        Quat::from_rotation_y(std::f32::consts::PI / 8.0),
        Vec3::new(0.0, 0.4, 0.0),
    );
    let cases = [
        // Eight times round its own Y, square to everything.
        (8, [false; 3], Space::Local, Mat4::IDENTITY, AT_REST, [3, 3, 3], [0.0, 1.0]),
        // Five times and across X: the box is lopsided about the axis.
        // The object turned and stretched, the gizmo on the world's axes.
        (
            5,
            [true, false, false],
            Space::Local,
            turned,
            Frame { origin: DVec3::new(0.3, -0.2, 0.5), rotation: DQuat::IDENTITY },
            [4, 3, 5],
            [0.0, 1.0],
        ),
        // The world's symmetry, eight times and across X and Z, the gizmo
        // turned against everything and the range a part of the box.
        (
            8,
            [true, false, true],
            Space::World,
            on_the_axis,
            Frame { origin: DVec3::new(0.1, 0.3, -0.2), rotation: DQuat::from_rotation_x(0.3) },
            [3, 4, 3],
            [0.15, 0.8],
        ),
    ];
    for (n, mirror, space, model, frame, count, range) in cases {
        let mut ball = ball(48, 80, n, 0.25);
        let symmetry = Symmetry { mirror, radial: round_y(n), space };
        let world = in_world(&symmetry, model);
        let pairs = ball.pairs(model, &world);
        let before = ball.asymmetry(model, &world, &pairs);
        let moves = [([count[0] - 1, 1, 1], [0.15, 0.1, -0.05]), ([0, 0, count[2] - 1], [-0.1, -0.2, 0.1]), ([1, count[1] - 1, 1], [0.05, 0.2, 0.0])];
        let params = Params { range, ..lattice(count, &moves) };
        let rest = ball.mesh.positions.clone();
        // The cage alone, as the Lattice was under radial symmetry before.
        let mut plain = Target::begin(&ball.mesh, model, &frame);
        let deformer = Deformer::new(&params, &plain.bounds).unwrap();
        plain.apply(&mut ball.mesh, Some(&deformer), params.axis);
        let alone = ball.asymmetry(model, &world, &pairs);
        plain.cancel(&mut ball.mesh);
        assert!(alone > 1e-2, "the cage alone breaks the symmetry: {alone}");
        let target = deformed(&mut ball, &symmetry, model, &frame, &params);
        let moved = ball.mesh.positions.iter().zip(&rest).map(|(a, b)| a.distance(*b)).fold(0.0, f32::max);
        let after = ball.asymmetry(model, &world, &pairs);
        println!(
            "{n} times round Y, mirror {mirror:?}, {space:?}: off by {before:.2e} before, {after:.2e} after \
             (moved up to {moved:.3}; the cage alone {alone:.2e})"
        );
        assert!(moved > 0.02, "the lattice moved the ball: {moved}");
        assert!(after < 2e-6, "{n} {mirror:?} {space:?}: {after:e} after, {before:e} before");
        target.cancel(&mut ball.mesh);
    }
}

#[test]
fn a_point_on_the_axis_moves_along_it_and_as_far_as_one_drag() {
    let mut ball = ball(40, 64, 8, 0.0);
    let symmetry = Symmetry { radial: round_y(8), ..Default::default() };
    let copies = copies(&symmetry, Mat4::IDENTITY, &AT_REST).unwrap();
    assert_eq!(copies.len(), 7);
    let target = Target::begin(&ball.mesh, Mat4::IDENTITY, &AT_REST);
    let bounds = target.bounds;
    let mut params = lattice([3, 3, 3], &[([1, 2, 1], [0.1, 0.2, 0.05])]);
    let top = params.lattice.index([1, 2, 1]);
    hold(&mut params.lattice, top, &bounds, params.axis, bounds.span(1, params.range), &copies);
    let [x, y, z] = params.lattice.moved[top];
    assert!(x.abs() < 1e-6 && z.abs() < 1e-6 && (y - 0.2).abs() < 1e-6, "held on the axis: {:?}", params.lattice.moved[top]);
    assert!(ghosts(&params.lattice, &bounds, 1, bounds.span(1, params.range), &copies).is_empty(), "its copies are itself");

    // The pole goes up the axis as far as it does with no copies at all.
    let one = Deformer::new(&params, &bounds).unwrap();
    let eight = Deformer::new(&params, &bounds).unwrap().repeated(&copies);
    let poles = ball.poles();
    let pole = poles.iter().map(|&v| ball.mesh.positions[v].as_dvec3()).find(|p| p.y > 0.0).unwrap();
    let (alone, together) = (one.apply(pole, DVec3::ZERO).unwrap(), eight.apply(pole, DVec3::ZERO).unwrap());
    assert!(together.x.abs() < 1e-7 && together.z.abs() < 1e-7, "{together}");
    assert!((together.y - alone.y).abs() < 1e-7 && together.y > pole.y + 0.04, "{together} against {alone}");
    drop(target);

    // And on the mesh, every vertex on the axis stays on it.
    let target = deformed(&mut ball, &symmetry, Mat4::IDENTITY, &AT_REST, &params);
    for v in poles {
        let p = ball.mesh.positions[v];
        assert!(p.x.abs() < 1e-6 && p.z.abs() < 1e-6, "{p}");
    }
    target.cancel(&mut ball.mesh);
}

#[test]
fn copies_crowded_together_move_the_clay_no_further_than_one_drag() {
    let ball = ball(32, 64, 8, 0.0);
    let target = Target::begin(&ball.mesh, Mat4::IDENTITY, &AT_REST);
    let bounds = target.bounds;
    // The middle of the box's +X side, pulled out, measured where it was:
    // on the ball's equator.
    let params = lattice([3, 3, 3], &[([2, 1, 1], [0.2, 0.0, 0.0])]);
    let side = crate::deform::lattice::point(&Lattice::default(), params.lattice.index([2, 1, 1]), &bounds, 1, bounds.span(1, params.range));
    assert!(side.distance(DVec3::new(0.5, 0.0, 0.0)) < 1e-5, "{side}");
    let alone = Deformer::new(&params, &bounds).unwrap().apply(side, DVec3::ZERO).unwrap() - side;
    for (n, crowded) in [(2, false), (4, false), (8, true), (32, true)] {
        let symmetry = Symmetry { radial: round_y(n), ..Default::default() };
        let copies = copies(&symmetry, Mat4::IDENTITY, &AT_REST).unwrap();
        let repeated = Deformer::new(&params, &bounds).unwrap().repeated(&copies);
        let together = repeated.apply(side, DVec3::ZERO).unwrap() - side;
        // What the copies would make of it if they were simply added up.
        let added: DVec3 = std::iter::once(DAffine3::IDENTITY)
            .chain(copies.iter().copied())
            .map(|copy| {
                let one = Deformer::new(&params, &bounds).unwrap();
                let from = copy.inverse().transform_point3(side);
                copy.matrix3 * one.apply(from, DVec3::ZERO).map_or(DVec3::ZERO, |to| to - from)
            })
            .sum();
        println!("{n} copies: one drag {:.4}, added up {:.4}, as made {:.4}", alone.x, added.x, together.x);
        // Under the point it moves as far as one drag, crowded or not; not
        // crowded, the copies are simply added up.
        assert!((together.x - alone.x).abs() < 1e-9, "{n} copies: {together} against {alone}");
        assert_eq!(added.x > alone.x * 1.01, crowded, "{n}: {added}");
        if !crowded {
            assert!((together - added).length() < 1e-9, "{n}: {together} against {added}");
        }
    }
}

#[test]
fn a_point_and_its_copy_among_the_cage_are_not_counted_twice() {
    let ball = ball(24, 48, 4, 0.0);
    let target = Target::begin(&ball.mesh, Mat4::IDENTITY, &AT_REST);
    let bounds = target.bounds;
    let span = bounds.span(1, [0.0, 1.0]);
    let copies = copies(&Symmetry { radial: round_y(4), ..Default::default() }, Mat4::IDENTITY, &AT_REST).unwrap();
    // A quarter turn of the box is the box: the four side points are each other's copies.
    let mut params = lattice([3, 3, 3], &[([0, 1, 1], [-0.1, 0.0, 0.0]), ([1, 0, 1], [0.0, -0.1, 0.0])]);
    let side = params.lattice.index([2, 1, 1]);
    params.lattice.moved[side] = [0.2, 0.05, 0.0];
    assert_eq!(ghosts(&params.lattice, &bounds, 1, span, &copies).len(), 2 * 3, "three copies of each point off the axis");
    hold(&mut params.lattice, side, &bounds, 1, span, &copies);
    let cage = &params.lattice;
    assert_eq!(cage.moved[cage.index([0, 1, 1])], [0.0; 3], "its copy is its own now");
    assert_eq!(cage.moved[cage.index([1, 0, 1])], [0.0, -0.1, 0.0], "the point on the axis is not a copy of it");
    assert_eq!(cage.moved[side], [0.2, 0.05, 0.0], "off the axis it goes where it was put");
    let shown = ghosts(cage, &bounds, 1, span, &copies);
    assert_eq!(shown.len(), 3);
    let opposite = crate::deform::lattice::point(cage, side, &bounds, 1, span) * DVec3::new(-1.0, 1.0, -1.0);
    assert!(shown.iter().any(|g| g.distance(opposite) < 1e-9), "the half turn of it: {shown:?}");
}

#[test]
fn undo_and_cancel_put_every_vertex_back_to_the_bit() {
    let mut ball = ball(40, 64, 8, 0.25);
    let rest = bits(&ball.mesh.positions);
    let symmetry = Symmetry { mirror: [true, false, false], radial: round_y(8), ..Default::default() };
    let params = lattice([3, 3, 3], &[([2, 1, 1], [0.2, 0.1, 0.0]), ([0, 2, 0], [0.0, 0.1, -0.1])]);
    let target = deformed(&mut ball, &symmetry, Mat4::IDENTITY, &AT_REST, &params);
    assert_ne!(bits(&ball.mesh.positions), rest);
    target.cancel(&mut ball.mesh);
    assert_eq!(bits(&ball.mesh.positions), rest);

    let target = deformed(&mut ball, &symmetry, Mat4::IDENTITY, &AT_REST, &params);
    let after = bits(&ball.mesh.positions);
    let mut history = History::default();
    history.push_stroke(target.end(&mut ball.mesh, Ride::Turned).expect("it moved"));
    assert_eq!(history.undo(&mut ball.mesh), Some(false));
    assert_eq!(bits(&ball.mesh.positions), rest);
    assert_eq!(history.redo(&mut ball.mesh), Some(false));
    assert_eq!(bits(&ball.mesh.positions), after);
}

#[test]
fn masked_clay_stays_put_under_the_copies_too() {
    let mut ball = ball(40, 64, 8, 0.25);
    for v in 0..ball.mesh.vertex_count() {
        ball.mesh.mask[v] = if ball.mesh.positions[v].y < 0.0 { 1.0 } else { 0.0 };
    }
    let rest = ball.mesh.positions.clone();
    let symmetry = Symmetry { radial: round_y(8), ..Default::default() };
    let params = lattice([3, 3, 3], &[([2, 1, 1], [0.2, 0.1, 0.0]), ([2, 0, 1], [0.2, 0.0, 0.0])]);
    let target = deformed(&mut ball, &symmetry, Mat4::IDENTITY, &AT_REST, &params);
    for v in 0..rest.len() {
        if ball.mesh.mask[v] >= 1.0 {
            assert_eq!(ball.mesh.positions[v], rest[v]);
        }
    }
    assert!(ball.mesh.positions.iter().zip(&rest).any(|(a, b)| a != b));
    target.cancel(&mut ball.mesh);
}

#[test]
fn without_turns_there_are_no_copies_and_the_lattice_is_what_it_was_to_the_bit() {
    for symmetry in [
        Symmetry::default(),
        Symmetry { mirror: [true, false, true], ..Default::default() },
        Symmetry { mirror: [true, false, false], radial: Radial { on: false, ..round_y(8) }, ..Default::default() },
    ] {
        assert!(copies(&symmetry, Mat4::IDENTITY, &AT_REST).is_none(), "{symmetry:?}");
    }
    // No copies is no change to the deformer, to the bit.
    let params = lattice([3, 4, 3], &[([2, 1, 1], [0.2, 0.1, 0.0]), ([0, 3, 2], [0.0, 0.1, -0.1])]);
    let mut results = Vec::new();
    for repeat in [false, true] {
        let mut ball = ball(40, 64, 8, 0.25);
        let mut target = Target::begin(&ball.mesh, Mat4::IDENTITY, &AT_REST);
        let deformer = Deformer::new(&params, &target.bounds).unwrap();
        let deformer = if repeat { deformer.repeated(&[]) } else { deformer };
        target.apply(&mut ball.mesh, Some(&deformer), params.axis);
        results.push(bits(&ball.mesh.positions));
        target.cancel(&mut ball.mesh);
    }
    assert!(results[0] == results[1]);
}

#[test]
fn a_custom_axis_tilted_against_a_mirror_plane_makes_each_copy_once() {
    let tilted = Radial { on: true, count: 6, axis: RadialAxis::Custom, custom_axis: Vec3::new(0.3, 1.0, 0.2) };
    let symmetry = Symmetry { mirror: [true, false, false], radial: tilted, ..Default::default() };
    let mut settings = crate::sculpt::Settings::default();
    settings.set_symmetry(symmetry.mirror, symmetry.radial, None);
    let group = settings.symmetry_group();
    assert!(!group.is_group());
    let copies = copies(&symmetry, Mat4::IDENTITY, &AT_REST).unwrap();
    assert_eq!(copies.len(), group.len() - 1);
    let mut ball = ball(40, 64, 6, 0.0);
    let rest = ball.mesh.positions.clone();
    let params = lattice([3, 3, 3], &[([2, 1, 1], [0.2, 0.1, 0.0])]);
    let target = deformed(&mut ball, &symmetry, Mat4::IDENTITY, &AT_REST, &params);
    assert!(ball.mesh.positions.iter().all(|p| p.is_finite()));
    let far = ball.mesh.positions.iter().zip(&rest).map(|(a, b)| a.distance(*b)).fold(0.0, f32::max);
    assert!(far > 0.01 && far < 0.3, "{far}");
    target.cancel(&mut ball.mesh);
}

/// One Lattice drag frame on a ball of about two million triangles, eight
/// times round Y, with no copies (as before), the eight turns, and the turns
/// mirrored across X, back to back, best of several:
/// `cargo test --release radial_lattice_timing -- --ignored --nocapture`
#[test]
#[ignore]
fn radial_lattice_timing_on_two_million_triangles() {
    let mut ball = ball(1000, 1000, 8, 0.25);
    let mesh = &mut ball.mesh;
    println!("{} triangles, {} vertices, {} threads", mesh.triangle_count(), mesh.vertex_count(), rayon::current_num_threads());
    let ways: [(&str, Option<Symmetry>); 3] = [
        ("no copies (as before)", None),
        ("8 round Y", Some(Symmetry { radial: round_y(8), ..Default::default() })),
        ("8 round Y + mirror X", Some(Symmetry { mirror: [true, false, false], radial: round_y(8), ..Default::default() })),
    ];
    let mut best = [(f64::MAX, f64::MAX); 3];
    for round in 0..6 {
        for (w, (_, symmetry)) in ways.iter().enumerate() {
            let copies = symmetry.map(|s| copies(&s, Mat4::IDENTITY, &AT_REST).unwrap()).unwrap_or_default();
            let mut target = Target::begin(mesh, Mat4::IDENTITY, &AT_REST);
            let k = 1.0 + round as f32 * 0.05;
            let params = lattice([3, 3, 3], &[([2, 1, 1], [0.2 * k, 0.1, 0.0])]);
            let start = std::time::Instant::now();
            let deformer = Deformer::new(&params, &target.bounds).unwrap().repeated(&copies);
            target.apply(mesh, Some(&deformer), params.axis);
            let pass = start.elapsed().as_secs_f64() * 1000.0;
            mesh.flush_normals();
            let _ = mesh.take_dirty();
            let frame = start.elapsed().as_secs_f64() * 1000.0;
            best[w] = (best[w].0.min(frame), best[w].1.min(pass));
            target.cancel(mesh);
            let _ = mesh.take_dirty();
        }
    }
    for ((name, _), (frame, pass)) in ways.iter().zip(best) {
        println!("{name:>24}: frame {frame:.1} ms, pass alone {pass:.1} ms");
    }
}
