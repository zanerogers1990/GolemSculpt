//! Brushes on the inside of an open shell: the hollow of a mask or a bowl,
//! where the eye sees the back of the faces, and the inner wall of a thin
//! shell seen from its hollow.

use glam::{Vec2, Vec3};

use super::stroke_tests::{draw, sphere, twins, view_of};
use super::*;

const RADIUS: f32 = 0.1;

/// The back half of a ball (z at or below nought), open toward the camera.
/// Seen from in front, the eye looks into the hollow at the back of its
/// faces: its normals face out of the ball, away from the eye. `turned`
/// gives its triangles the other winding, so the same surface faces the
/// eye. `rippled` puts a fine ripple over it, for the smoothing brushes to
/// take off (it is not a mirror image of itself across x = 0).
fn bowl(turned: bool, rippled: bool) -> Mesh {
    let ball = sphere(60_000);
    let ripple = if rippled { 0.006 } else { 0.0 };
    let positions: Vec<Vec3> =
        ball.positions.iter().map(|&p| p * (1.0 + ripple * (p.x * 90.0).sin() * (p.y * 90.0).sin())).collect();
    let mut indices = Vec::new();
    for t in ball.indices.chunks_exact(3) {
        if t.iter().all(|&v| ball.positions[v as usize].z <= 1e-6) {
            indices.extend_from_slice(&if turned { [t[0], t[2], t[1]] } else { [t[0], t[1], t[2]] });
        }
    }
    Mesh::new(positions, indices, None)
}

/// A stroke across the bottom of the hollow, from `from` to `to` along x,
/// on a surface `depth` from the middle of the ball.
fn across_the_bottom(view: &View, depth: f32, from: f32, to: f32) -> Vec<Vec2> {
    (0..=60)
        .map(|i| {
            let x = from + (to - from) * i as f32 / 60.0;
            view.project(Vec3::new(x, 0.0, -(depth * depth - x * x).sqrt())).unwrap()
        })
        .collect()
}

/// A sculptor holding the brush called `name` as it starts, at a fixed size,
/// with Front Facing set to `front_facing` if given.
fn holding(name: &str, front_facing: Option<bool>) -> Sculptor {
    let mut sculptor = Sculptor::default();
    let mut library = library::Library::default();
    assert!(library.select_named(name, &mut sculptor.settings), "{name}");
    let s = &mut sculptor.settings;
    // Clay's finishing pass is paced by the frame, not the dab.
    (s.screen_space_size, s.radius_world, s.clay_relax) = (false, RADIUS, 0.0);
    if let Some(on) = front_facing {
        s.current_mut().front_facing = on;
    }
    sculptor
}

/// The brushes that lay dabs, base and shipped, that work the surface along
/// or against the way it faces.
const BRUSHES: [&str; 11] =
    ["Standard", "Clay", "Clay Strips", "Inflate", "Layer", "Flatten", "Smooth", "Pinch", "Nudge", "Crease", "Dam Standard"];

/// How far apart two sculpted copies of one surface came out, vertex by
/// vertex. A mesh lays its vertices out in its own order (`Mesh::new`), so
/// they are matched by where they were.
fn apart(a: &Mesh, a_after: &[Vec3], b: &Mesh, b_after: &[Vec3]) -> f32 {
    let key = |p: Vec3| p.to_array().map(f32::to_bits);
    let at: std::collections::HashMap<[u32; 3], usize> = b.positions.iter().enumerate().map(|(i, &p)| (key(p), i)).collect();
    a.positions.iter().enumerate().map(|(i, &p)| a_after[i].distance(b_after[at[&key(p)]])).fold(0.0f32, f32::max)
}

/// A stroke across the bottom of the bowl, the brush called `name`: the
/// bowl after it, how far it moved any vertex, and how far it moved them
/// toward the eye (into the hollow), added up.
fn stroke_in_the_bowl(name: &str, front_facing: Option<bool>, turned: bool) -> (Vec<Vec3>, f32, f64) {
    let mut mesh = bowl(turned, true);
    let (before, normals) = (mesh.positions.clone(), mesh.normals.clone());
    let view = view_of(&mesh);
    let path = across_the_bottom(&view, 0.5, -0.15, 0.15);
    draw(&mut mesh, &mut holding(name, front_facing), &view, &path, 4, Some(&|_| 1.0));
    let (mut most, mut toward) = (0.0f32, 0.0f64);
    for v in 0..before.len() {
        let moved = mesh.positions[v] - before[v];
        most = most.max(moved.length());
        // Out of the ball is away from the eye.
        let out = if turned { -normals[v] } else { normals[v] };
        toward -= moved.dot(out) as f64;
    }
    (mesh.positions, most, toward)
}

/// In the hollow of an open bowl, where the eye sees the back of the faces,
/// every brush does what it does on the same bowl with its faces turned to
/// face the eye: the side the pen is on is the outside. Standard builds
/// toward the pen (it built away from it), Crease and Dam Standard cut in
/// (they raised a ridge), and the brushes that start with Front Facing on
/// work at all (they did nothing: every vertex faced away).
#[test]
fn inside_an_open_bowl_every_brush_works_as_on_its_faces_turned_round() {
    let (back, front) = (bowl(false, true), bowl(true, true));
    for name in BRUSHES {
        for front_facing in [None, Some(false)] {
            let (inside, inside_most, inside_toward) = stroke_in_the_bowl(name, front_facing, false);
            let (turned, turned_most, turned_toward) = stroke_in_the_bowl(name, front_facing, true);
            let apart = apart(&back, &inside, &front, &turned);
            println!(
                "{name:>13} front facing {front_facing:?}: inside moved {inside_most:.2e}, toward the eye {inside_toward:+.3e}; \
                 turned round {turned_most:.2e}, {turned_toward:+.3e}; apart {apart:.2e}"
            );
            assert!(turned_most > 4e-4, "{name}: the stroke did next to nothing ({turned_most}) even facing the eye");
            assert!(inside_most > 0.5 * turned_most, "{name} {front_facing:?}: inside moved {inside_most}, turned round {turned_most}");
            assert!(
                apart <= 1e-3 * turned_most,
                "{name} {front_facing:?}: inside and turned round came out {apart} apart, of {turned_most}"
            );
        }
    }
}

/// What counts as meeting the surface from behind: the hollow of the bowl
/// does, the front of a ball does not, and nor does one triangle on it wound
/// the wrong way among neighbours that face the eye (a fold in a crumpled
/// patch), which would have turned that one dab round.
#[test]
fn a_fold_on_the_front_of_a_ball_is_not_seen_from_behind() {
    let ball = sphere(60_000);
    let view = view_of(&ball);
    let (origin, dir) = view.ray(view.project(Vec3::new(0.0, 0.0, 0.5)).unwrap());
    let hit = ball.raycast(origin, dir).unwrap();
    assert!(!ball.hit_from_behind(&hit, dir, false), "the front of a ball");
    let mut indices = ball.indices.clone();
    let t = hit.tri as usize * 3;
    indices.swap(t + 1, t + 2);
    // Through the middle of the folded triangle, not a corner it shares.
    let middle = (0..3).map(|k| ball.positions[ball.indices[t + k] as usize]).sum::<Vec3>() / 3.0;
    let (origin, dir) = view.ray(view.project(middle).unwrap());
    let folded = Mesh::new(ball.positions.clone(), indices, None);
    let hit = folded.raycast(origin, dir).unwrap();
    let i = hit.tri as usize * 3;
    let winds = |m: &Mesh| {
        let [a, b, c] = [0, 1, 2].map(|k| m.positions[m.indices[i + k] as usize]);
        (b - a).cross(c - a).dot(dir)
    };
    assert!(winds(&folded) > 0.0, "the ray should meet the folded triangle");
    assert!(!folded.hit_from_behind(&hit, dir, false), "a fold among faces that face the eye");
    let inside = bowl(false, false);
    let (origin, dir) = view.ray(view.project(Vec3::new(0.0, 0.0, -0.5)).unwrap());
    let hit = inside.raycast(origin, dir).unwrap();
    assert!(inside.hit_from_behind(&hit, dir, false), "the hollow of a bowl");
}

/// Standard in the hollow of a bowl raises the surface toward the pen, as
/// it starts (Front Facing on) and with Front Facing off.
#[test]
fn standard_inside_a_bowl_builds_toward_the_pen() {
    for front_facing in [None, Some(false)] {
        let (_, most, toward) = stroke_in_the_bowl("Standard", front_facing, false);
        println!("front facing {front_facing:?}: moved {most:.2e}, toward the eye {toward:+.3e}");
        assert!(most > 0.05 * RADIUS, "Standard moved the inside {most}");
        assert!(toward > 0.0, "Standard built {toward} away from the pen");
    }
}

/// Under the mirror, a stroke in the hollow of a bowl is the mirror image of
/// itself: each copy of a dab is on the back of its own side's surface too.
#[test]
fn inside_a_bowl_a_mirrored_stroke_stays_a_mirror_image() {
    let flip = Vec3::new(-1.0, 1.0, 1.0);
    for name in ["Standard", "Clay", "Inflate", "Crease", "Flatten"] {
        let mut mesh = bowl(false, false);
        let (before, twins) = (mesh.positions.clone(), twins(&mesh));
        let view = view_of(&mesh);
        let mut sculptor = holding(name, None);
        sculptor.settings.symmetry = [true, false, false];
        // Off the middle and across it, so the copies meet on the plane.
        let path = across_the_bottom(&view, 0.5, -0.05, 0.2);
        draw(&mut mesh, &mut sculptor, &view, &path, 4, Some(&|_| 1.0));
        let (mut worst, mut most) = (0.0f32, 0.0f32);
        for (v, &t) in twins.iter().enumerate() {
            worst = worst.max(mesh.positions[t as usize].distance(mesh.positions[v] * flip));
            most = most.max(mesh.positions[v].distance(before[v]));
        }
        println!("{name}: moved {most:.2e}, mirror error {worst:.2e}");
        // Flatten has little to level on a smooth bowl.
        assert!(most > 2e-4, "{name} did nothing inside the bowl");
        assert!(worst <= most * 1e-5, "{name}: mirror error {worst} of {most}");
    }
}

/// The back half of a ball as a closed shell `thickness` thick: its outer
/// wall the bowl, its inner wall the bowl brought in by `thickness` and
/// turned to face the hollow, and a rim joining the two.
fn shell(thickness: f32) -> Mesh {
    let outer = bowl(false, true);
    let n = outer.positions.len() as u32;
    let scale = (0.5 - thickness) / 0.5;
    let positions: Vec<Vec3> = outer.positions.iter().copied().chain(outer.positions.iter().map(|&p| p * scale)).collect();
    let mut indices = outer.indices.clone();
    for t in outer.indices.chunks_exact(3) {
        indices.extend_from_slice(&[t[0] + n, t[2] + n, t[1] + n]);
    }
    // The rim, along every edge of the outer wall that one triangle has.
    let mut edges = std::collections::HashMap::new();
    for t in outer.indices.chunks_exact(3) {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            *edges.entry((a.min(b), a.max(b))).or_insert(0) += 1;
        }
    }
    for t in outer.indices.chunks_exact(3) {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            if edges[&(a.min(b), a.max(b))] == 1 {
                indices.extend_from_slice(&[b, a, a + n, b, a + n, b + n]);
            }
        }
    }
    Mesh::new(positions, indices, None)
}

/// A stroke across the bottom of the inner wall of a shell `thickness`
/// thick, seen from its hollow: how far it moved the inner wall and the
/// outer one, at the most.
fn stroke_in_the_shell(name: &str, thickness: f32, tune: impl Fn(&mut BrushSettings)) -> (f32, f32) {
    let mut mesh = shell(thickness);
    let (before, normals) = (mesh.positions.clone(), mesh.normals.clone());
    let view = view_of(&mesh);
    let mut sculptor = holding(name, None);
    tune(sculptor.settings.current_mut());
    let path = across_the_bottom(&view, 0.5 - thickness, -0.15, 0.15);
    draw(&mut mesh, &mut sculptor, &view, &path, 4, Some(&|_| 1.0));
    // The outer wall faces out of the ball, the inner one into it; the rim,
    // far from the stroke, is left out.
    let moved = |out: bool| {
        (0..before.len())
            .filter(|&v| before[v].z < -0.1 && (normals[v].dot(before[v]) > 0.0) == out)
            .map(|v| mesh.positions[v].distance(before[v]))
            .fold(0.0f32, f32::max)
    };
    (moved(false), moved(true))
}

/// On the inner wall of a thin shell, seen from its hollow, each brush as it
/// starts works the wall it is on and leaves the outer wall behind it where
/// it was, from a shell a twenty-fifth of the brush thick to one most of it.
/// (Crease and Dam Standard are held to a groove shallower than the shell:
/// one deeper goes through it, as it would through clay.)
#[test]
fn inside_a_thin_shell_a_stroke_leaves_the_outer_wall_alone() {
    for thickness in [0.004, 0.03, 0.08] {
        for name in BRUSHES {
            let cuts = matches!(name, "Crease" | "Dam Standard");
            let shallow = |b: &mut BrushSettings| {
                if cuts {
                    b.strength = 0.05;
                }
            };
            if cuts && thickness < 0.03 {
                continue;
            }
            let (inner, outer) = stroke_in_the_shell(name, thickness, shallow);
            println!("{thickness:.3} thick, {name:>13}: inner wall moved {inner:.2e}, outer {outer:.2e}");
            assert!(inner > 3e-4, "{name} did nothing to the inner wall {thickness} thick");
            assert!(inner < 0.9 * thickness || !cuts, "{name} cut {inner} into a wall {thickness} thick");
            assert_eq!(outer, 0.0, "{name} moved the outer wall of a shell {thickness} thick by {outer}");
        }
    }
}
