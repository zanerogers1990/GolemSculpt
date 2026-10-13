//! Tests of the kernels' options and of the two brushes added after the
//! Unity app's ten: Smooth's Keep Volume, Peaks, Valleys and Relax; Flatten's
//! Scrape, Fill and Lock Plane; Pinch; Nudge; and how a Crease opens.

use std::f32::consts::TAU;

use glam::{Vec2, Vec3};

use super::stroke_tests::{SIZE, across, draw, height, on_ball, rough_ball, round, sculptor_with, sphere, spread, twins, view_of};
use super::*;
use crate::camera::View;

/// Draws a stroke through `points` with the brush inverted or not (Ctrl, or
/// the right button), four samples to a frame.
fn draw_as(mesh: &mut Mesh, sculptor: &mut Sculptor, view: &View, points: &[Vec2], invert: bool) {
    let sample = |i: usize| PointerSample { pos: points[i], pressure: None, time: i as f64 / 240.0 };
    assert!(sculptor.begin(mesh, view, sample(0), invert, false), "the stroke missed the mesh");
    for i in 1..points.len() {
        sculptor.sample(mesh, view, sample(i));
        if i % 4 == 0 {
            sculptor.frame(mesh, view, 1.0 / 60.0);
            let _ = mesh.take_dirty();
        }
    }
    sculptor.end(mesh, view);
    let _ = mesh.take_dirty();
}

// ------------------------------------------------------------------- Smooth

/// A rod along X, `radius` thick and `length` long, with `around` vertices
/// round it and its ends closed flat: a thin form, like a finger. `rough`
/// is how far its surface ripples in and out, as a share of the radius.
fn rod(radius: f32, length: f32, around: usize, rough: f32) -> Mesh {
    let edge = TAU * radius / around as f32;
    let rings = (length / edge).round() as usize;
    let mut positions = Vec::new();
    for i in 0..=rings {
        let x = length * (i as f32 / rings as f32 - 0.5);
        for j in 0..around {
            let a = TAU * (j as f32 + 0.5 * (i % 2) as f32) / around as f32;
            let r = radius * (1.0 + rough * (5.0 * a).sin() * (x * 60.0).sin());
            positions.push(Vec3::new(x, r * a.cos(), r * a.sin()));
        }
    }
    let at = |i: usize, j: usize| (i * around + j % around) as u32;
    let mut indices = Vec::new();
    for i in 0..rings {
        for j in 0..around {
            indices.extend([at(i, j), at(i, j + 1), at(i + 1, j), at(i, j + 1), at(i + 1, j + 1), at(i + 1, j)]);
        }
    }
    let (left, right) = (positions.len() as u32, positions.len() as u32 + 1);
    positions.extend([Vec3::new(-length * 0.5, 0.0, 0.0), Vec3::new(length * 0.5, 0.0, 0.0)]);
    for j in 0..around {
        indices.extend([left, at(0, j + 1), at(0, j), right, at(rings, j), at(rings, j + 1)]);
    }
    Mesh::new(positions, indices, None)
}

/// How thick the middle of a [`rod`] is and how rough: the mean distance of
/// its surface from its axis there, and how tall the ripple it was made with
/// still stands (read off against the ripple's own pattern, so a rod that
/// has grown thinner toward its middle does not count as a rough one).
fn rod_middle(mesh: &Mesh) -> (f32, f32) {
    let middle: Vec<(f32, f32)> = mesh
        .positions
        .iter()
        .filter(|p| p.x.abs() < 0.12)
        .map(|p| (Vec2::new(p.y, p.z).length(), (5.0 * p.z.atan2(p.y)).sin() * (p.x * 60.0).sin()))
        .collect();
    let mean = middle.iter().map(|m| m.0).sum::<f32>() / middle.len() as f32;
    // A ripple of height 1 comes to a quarter against its own pattern.
    let ripple = 4.0 * middle.iter().map(|&(r, pattern)| (r - mean) * pattern).sum::<f32>() / middle.len() as f32;
    (mean, ripple.abs())
}

/// What a closed mesh holds.
fn volume(mesh: &Mesh) -> f64 {
    let at = |i: u32| mesh.positions[i as usize].as_dvec3();
    mesh.indices.chunks_exact(3).map(|t| at(t[0]).dot(at(t[1]).cross(at(t[2]))) / 6.0).sum()
}

/// A Smooth brush big enough to take in the whole thickness of a thin form.
fn smoother(radius: f32, keep_volume: f32, mode: SmoothMode) -> Sculptor {
    let mut sculptor = sculptor_with(Brush::Smooth, radius, 0.5);
    let bs = sculptor.settings.current_mut();
    (bs.keep_volume, bs.smooth_mode) = (keep_volume, mode);
    sculptor
}

/// Keep Volume on a thin form: a rough rod smoothed again and again keeps
/// its thickness at 100%, loses a good part of it at 0% as it always has,
/// and comes out as smooth either way.
#[test]
fn keep_volume_holds_a_thin_forms_thickness_while_its_bumps_still_go() {
    let radius = 0.08;
    let smoothed = |keep: f32, strokes: usize| {
        let mut mesh = rod(radius, 1.2, 24, 0.06);
        let view = view_of(&mesh);
        let path: Vec<Vec2> = (0..=40).map(|i| view.project(Vec3::new(-0.25 + 0.5 * i as f32 / 40.0, 0.0, radius)).unwrap()).collect();
        let mut sculptor = smoother(0.2, keep, SmoothMode::All);
        sculptor.settings.current_mut().strength = 0.2;
        for _ in 0..strokes {
            draw(&mut mesh, &mut sculptor, &view, &path, 4, None);
        }
        rod_middle(&mesh)
    };
    let (thick, rough) = smoothed(0.0, 0);
    assert!((thick / radius - 1.0).abs() < 0.01 && rough > 0.02 * radius, "the rod as made: {thick} thick, {rough} rough");
    let strokes = 8;
    let mut report = Vec::new();
    for keep in [0.0, 0.5, 1.0] {
        let (now, still_rough) = smoothed(keep, strokes);
        println!(
            "Keep Volume {:.0}%: after {strokes} strokes the rod is {:.1}% as thick, and {:.1}% of its ripple is gone",
            keep * 100.0,
            now / thick * 100.0,
            (1.0 - still_rough / rough) * 100.0
        );
        report.push((now / thick, 1.0 - still_rough / rough));
    }
    let (plain, half, kept) = (report[0], report[1], report[2]);
    assert!(plain.0 < 0.9, "without it the rod should have thinned by a tenth or more, or the test proves nothing: {}", plain.0);
    assert!((kept.0 - 1.0).abs() < 0.03, "at 100% the rod is {} of its thickness", kept.0);
    assert!(half.0 > plain.0 + 0.03 && half.0 < kept.0 - 0.02, "50% is between the two: {} / {} / {}", plain.0, half.0, kept.0);
    for (name, (_, gone)) in [("0%", plain), ("50%", half), ("100%", kept)] {
        assert!(gone > 0.8, "at {name} only {gone} of the ripple went: it must still smooth");
    }
}

/// Keep Volume on a ball: a bumpy ball smoothed all over its front holds
/// what it held, where plain smoothing lets it shrink, and its bumps go as
/// they do without it. On a flat surface there is nothing to give back, and
/// nothing is: the option changes next to nothing there.
#[test]
fn keep_volume_holds_a_bumpy_balls_volume() {
    let smoothed = |keep: f32| {
        let mut mesh = rough_ball(80_000);
        let before = (volume(&mesh), roughness(&mesh));
        let view = view_of(&mesh);
        let mut sculptor = smoother(0.3, keep, SmoothMode::All);
        for k in 0..8 {
            // Back and forth over the front of the ball, a little higher each time.
            let up = Vec2::new(0.0, (k as f32 - 3.5) * 22.0);
            let path: Vec<Vec2> = across(&view, -0.7, 0.7, 60).into_iter().map(|p| p + up).collect();
            draw(&mut mesh, &mut sculptor, &view, &path, 4, None);
        }
        (volume(&mesh) / before.0, 1.0 - roughness(&mesh) / before.1)
    };
    let mut report = Vec::new();
    for keep in [0.0, 0.5, 1.0] {
        let (held, gone) = smoothed(keep);
        println!("Keep Volume {:.0}%: the ball holds {:.2}% of its volume, and {:.1}% of its bumps' height is gone", keep * 100.0, held * 100.0, gone * 100.0);
        report.push((held, gone));
    }
    let lost = |held: f64| (1.0 - held).abs();
    assert!(lost(report[0].0) > 0.002, "plain smoothing took only {} of the volume: the test proves nothing", lost(report[0].0));
    assert!(lost(report[2].0) < lost(report[0].0) * 0.15, "at 100% it lost {} against {} without", lost(report[2].0), lost(report[0].0));
    assert!(lost(report[1].0) < lost(report[0].0) * 0.7 && lost(report[1].0) > lost(report[2].0));
    assert!(report.iter().all(|&(_, gone)| gone > 0.6), "the bumps must still go: {report:?}");
}

/// How tall the bumps of a [`rough_ball`] still stand on its front, read off
/// against the pattern they were made in (so a ball that has been drawn in
/// a little, here more and there less, does not count as a bumpy one).
fn roughness(mesh: &Mesh) -> f32 {
    let front: Vec<(f32, f32)> = mesh
        .positions
        .iter()
        .filter(|p| p.z > 0.4)
        .map(|p| (p.length(), (p.x / p.length() * 45.0).sin() * (p.y / p.length() * 45.0).sin()))
        .collect();
    let mean = front.iter().map(|f| f.0).sum::<f32>() / front.len() as f32;
    (4.0 * front.iter().map(|&(r, pattern)| (r - mean) * pattern).sum::<f32>() / front.len() as f32).abs()
}

/// With Keep Volume at 0 and the mode at All, Smooth is the kernel it always
/// was (the fingerprints say so to the bit); and the option makes no
/// difference to speak of where smoothing takes nothing away, on a flat
/// stretch of surface.
#[test]
fn keep_volume_gives_nothing_back_where_nothing_was_taken() {
    // A big flat-topped slab: a ball squashed nearly flat, bumpy on top.
    let slab = || {
        let ball = rough_ball(80_000);
        Mesh::new(ball.positions.iter().map(|&p| Vec3::new(p.x, p.y, p.z * 0.02 + (p.length() - 0.5))).collect(), ball.indices.clone(), None)
    };
    let smoothed = |keep: f32| {
        let mut mesh = slab();
        let view = view_of(&mesh);
        let mut sculptor = smoother(0.1, keep, SmoothMode::All);
        let c = SIZE * 0.5;
        let path: Vec<Vec2> = (0..=40).map(|i| c + Vec2::new(-80.0 + 4.0 * i as f32, 0.0)).collect();
        draw(&mut mesh, &mut sculptor, &view, &path, 4, None);
        mesh.positions
    };
    let (flat, plain, kept) = (slab().positions, smoothed(0.0), smoothed(1.0));
    let moved = plain.iter().zip(&flat).map(|(a, b)| a.distance(*b)).fold(0.0f32, f32::max);
    let apart = plain.iter().zip(&kept).map(|(a, b)| a.distance(*b)).fold(0.0f32, f32::max);
    assert!(moved > 1e-3, "the stroke smoothed nothing: {moved}");
    assert!(apart < moved * 0.08, "on the flat, Keep Volume moved the surface {apart} where smoothing moved it {moved}");
}

/// Peaks only lowers and Valleys only raises: on a bumpy ball one takes the
/// tops off and leaves the hollows as deep as they were, and the other
/// fills the hollows and leaves the tops standing.
#[test]
fn peaks_only_lowers_and_valleys_only_fills() {
    let smoothed = |mode: SmoothMode| {
        let mut mesh = rough_ball(120_000);
        let before = mesh.positions.clone();
        let view = view_of(&mesh);
        let mut sculptor = smoother(0.12, 0.0, mode);
        for _ in 0..6 {
            draw(&mut mesh, &mut sculptor, &view, &across(&view, -0.3, 0.3, 60), 4, None);
        }
        // Along the path, where the brush is at its strongest.
        let on_path = |p: &Vec3| p.z > 0.0 && p.y.abs() < 0.03 && round(*p).abs() < 0.25;
        let heights = |positions: &[Vec3]| -> (f32, f32) {
            let there = before.iter().zip(positions).filter(|(was, _)| on_path(was)).map(|(_, now)| height(*now));
            there.fold((f32::MAX, f32::MIN), |(low, high), h| (low.min(h), high.max(h)))
        };
        let (rose, sank) = before.iter().zip(&mesh.positions).fold((0.0f32, 0.0f32), |(rose, sank), (was, now)| {
            let out = (*now - *was).dot(was.normalize());
            (rose.max(out), sank.max(-out))
        });
        (heights(&before), heights(&mesh.positions), rose, sank)
    };
    let ((pit, top), (pit_all, top_all), ..) = smoothed(SmoothMode::All);
    assert!(top > 0.004 && pit < -0.004, "the bumps as made: {pit} to {top}");
    assert!(top_all - pit_all < (top - pit) * 0.5, "All evens both out: {pit_all} to {top_all}, from {pit} to {top}");

    let (_, (pit_peaks, top_peaks), rose, sank) = smoothed(SmoothMode::Peaks);
    println!("Peaks: tops {top} -> {top_peaks}, pits {pit} -> {pit_peaks}; most raised {rose:.2e}, most lowered {sank:.2e}");
    assert!(top_peaks < top * 0.5, "Peaks left the tops at {top_peaks} of {top}");
    assert!(pit_peaks < pit * 0.9, "Peaks filled the pits: {pit} to {pit_peaks}");
    assert!(rose < sank * 0.02, "Peaks raised something by {rose}");

    let (_, (pit_valleys, top_valleys), rose, sank) = smoothed(SmoothMode::Valleys);
    println!("Valleys: tops {top} -> {top_valleys}, pits {pit} -> {pit_valleys}; most raised {rose:.2e}, most lowered {sank:.2e}");
    assert!(pit_valleys > pit * 0.5, "Valleys left the pits at {pit_valleys} of {pit}");
    assert!(top_valleys > top * 0.9, "Valleys lowered the tops: {top} to {top_valleys}");
    assert!(sank < rose * 0.02, "Valleys lowered something by {sank}");
}

/// How even a mesh's triangles are in front of the ball, near its middle:
/// the mean and the worst of each triangle's quality, which is 1 for an
/// equilateral triangle and falls to 0 as it is stretched to a sliver.
fn triangle_quality(mesh: &Mesh) -> (f32, f32) {
    let quality = |t: &[u32]| {
        let [a, b, c] = [t[0], t[1], t[2]].map(|i| mesh.positions[i as usize]);
        let sides = a.distance_squared(b) + b.distance_squared(c) + c.distance_squared(a);
        4.0 * 3f32.sqrt() * 0.5 * (b - a).cross(c - a).length() / sides.max(1e-30)
    };
    let near = |t: &&[u32]| t.iter().all(|&i| mesh.positions[i as usize].z > 0.42);
    let all: Vec<f32> = mesh.indices.chunks_exact(3).filter(near).map(quality).collect();
    (all.iter().sum::<f32>() / all.len() as f32, all.iter().fold(1.0f32, |worst, &q| worst.min(q)))
}

/// A ball whose triangles have been dragged sideways across its front, the
/// way Move leaves them, without the ball itself changing shape: every
/// vertex is still on it.
fn stretched_ball() -> Mesh {
    let ball = sphere(80_000);
    let dragged = ball.positions.iter().map(|&p| {
        let pull = 0.09 * super::kernels::smooth(1.0 - Vec2::new(p.x + 0.03, p.y).length() / 0.16) * f32::from(p.z > 0.0);
        (p + Vec3::X * pull).normalize() * 0.5
    });
    Mesh::new(dragged.collect(), ball.indices.clone(), None)
}

/// Relax evens out stretched triangles and leaves the shape where it was:
/// the ball stays a ball to within a few millionths of its size, where
/// plain smoothing over the same strokes flattens it by a thousand times
/// that.
#[test]
fn relax_evens_out_the_triangles_and_keeps_the_shape() {
    let relaxed = |mode: SmoothMode| {
        let mut mesh = stretched_ball();
        let view = view_of(&mesh);
        let mut sculptor = smoother(0.2, 0.0, mode);
        for k in 0..6 {
            let up = Vec2::new(0.0, (k % 3) as f32 * 30.0 - 30.0);
            let path: Vec<Vec2> = across(&view, -0.35, 0.35, 50).into_iter().map(|p| p + up).collect();
            draw(&mut mesh, &mut sculptor, &view, &path, 4, None);
        }
        let off = mesh.positions.iter().map(|p| (p.length() - 0.5).abs()).fold(0.0f32, f32::max);
        (triangle_quality(&mesh), off)
    };
    let even = triangle_quality(&sphere(80_000));
    let stretched = triangle_quality(&stretched_ball());
    let ((mean, worst), off) = relaxed(SmoothMode::Relax);
    let (_, off_smooth) = relaxed(SmoothMode::All);
    println!("triangle quality (mean, worst): the ball as made {even:.3?}, stretched {stretched:.3?}, relaxed ({mean:.3}, {worst:.3})");
    println!("furthest any vertex is from the ball: relaxed {off:.2e}, smoothed {off_smooth:.2e}");
    assert!(stretched.1 < 0.35 && stretched.0 < even.0 - 0.03, "the patch is not stretched: {stretched:?} against {even:?}");
    assert!(worst > stretched.1 + 0.25, "the worst triangle went from {} to {worst}", stretched.1);
    assert!(mean > stretched.0 + 0.6 * (even.0 - stretched.0), "the mean went from {} to {mean}, of {}", stretched.0, even.0);
    assert!(off < 0.5 * 2e-4, "relaxed, a vertex is {off} off the ball");
    assert!(off_smooth > off * 20.0, "plain smoothing is no further off: {off_smooth} against {off}");
}

/// The options hold with a mask, hidden surface, Front Facing and symmetry
/// as the plain kernel does: masked and hidden vertices do not move in any
/// mode, and under Mirror X each mode leaves the ball a mirror image of
/// itself.
#[test]
fn smooths_options_respect_masks_hidden_parts_and_symmetry() {
    for (mode, keep) in [(SmoothMode::All, 1.0), (SmoothMode::Peaks, 0.0), (SmoothMode::Valleys, 0.0), (SmoothMode::Relax, 0.0)] {
        // A ball rough the same way on both sides of the X plane.
        let ball = sphere(80_000);
        let bumpy = ball.positions.iter().map(|&p| p * (1.0 + 0.012 * (p.x.abs() * 90.0).sin() * (p.y * 90.0).sin()));
        let mut mesh = Mesh::new(bumpy.collect(), ball.indices.clone(), None);
        let twins = twins(&mesh);
        // Masked above, hidden to one side below.
        let leaves: Vec<u32> = (0..mesh.bvh.leaves.len() as u32).collect();
        mesh.begin_stroke();
        mesh.paint_mask(&leaves, |vt| (vt.pos.y > 0.06).then_some(1.0));
        mesh.mask_changed(&leaves);
        mesh.end_stroke(true);
        let before = mesh.positions.clone();
        let view = view_of(&mesh);
        let mut sculptor = smoother(0.15, keep, mode);
        sculptor.settings.symmetry = [true, false, false];
        sculptor.settings.current_mut().front_facing = true;
        let c = SIZE * 0.5;
        let path: Vec<Vec2> = (0..=50).map(|i| c + Vec2::new(-90.0 + 2.2 * i as f32, 20.0 - 0.5 * i as f32)).collect();
        for _ in 0..3 {
            draw(&mut mesh, &mut sculptor, &view, &path, 4, None);
        }
        let (mut moved, mut worst, mut biggest) = (0, 0.0f32, 0.0f32);
        for (v, &t) in twins.iter().enumerate() {
            let went = mesh.positions[v].distance(before[v]);
            if mesh.mask[v] >= 1.0 {
                assert_eq!(went, 0.0, "{mode:?}: a masked vertex moved");
            }
            if mesh.normals[v].z < -0.3 {
                assert_eq!(went, 0.0, "{mode:?}: surface facing away moved");
            }
            moved += usize::from(went > 0.0);
            worst = worst.max(mesh.positions[t as usize].distance(mesh.positions[v] * Vec3::new(-1.0, 1.0, 1.0)));
            biggest = biggest.max(went);
        }
        println!("{mode:?}: {moved} vertices moved, by {biggest:.2e} at most; mirror error {worst:.2e}");
        assert!(moved > 300 && biggest > 1e-4, "{mode:?} did nothing");
        assert!(worst < biggest * 0.01, "{mode:?} is {worst} off its mirror image, having moved {biggest}");
    }
    // Hidden surface is neither moved nor read: a stroke beside a hidden
    // patch moves what shows exactly as far whatever the patch's shape.
    for mode in SmoothMode::ALL {
        let beside = |bulge: f32| {
            let mut mesh = rough_ball(80_000);
            // The patch pushed out before it is hidden, on the mesh itself,
            // so that its vertices are the same ones in the same order.
            let leaves: Vec<u32> = (0..mesh.bvh.leaves.len() as u32).collect();
            mesh.begin_stroke();
            mesh.move_vertices(&leaves, |_, vt| (bulge > 0.0 && vt.pos.z > 0.0 && vt.pos.x < -0.08).then(|| vt.pos * (1.0 + bulge)));
            mesh.positions_changed(&leaves);
            mesh.end_stroke(false);
            let _ = mesh.take_dirty();
            let hide: Vec<u32> = (0..mesh.triangle_count() as u32)
                .filter(|&t| (0..3).any(|c| mesh.positions[mesh.indices[t as usize * 3 + c] as usize].x < -0.05))
                .collect();
            assert!(mesh.hide(&hide).is_some());
            let view = view_of(&rough_ball(80_000));
            let mut sculptor = smoother(0.12, if mode == SmoothMode::All { 1.0 } else { 0.0 }, mode);
            let before = mesh.positions.clone();
            draw(&mut mesh, &mut sculptor, &view, &across(&view, 0.02, 0.3, 30), 4, None);
            let hidden = mesh.hidden_vertices().unwrap().to_vec();
            (mesh.positions, before, hidden)
        };
        let ((plain, plain_before, hidden), (bulged, bulged_before, bulged_hidden)) = (beside(0.0), beside(0.1));
        assert!(hidden == bulged_hidden && hidden.iter().filter(|&&h| h).count() > 1000);
        let (mut worst, mut bulging) = (0.0f32, 0);
        for v in 0..plain.len() {
            if hidden[v] {
                assert_eq!(bulged[v], bulged_before[v], "{mode:?}: a hidden vertex moved");
                bulging += usize::from(bulged_before[v].distance(plain_before[v]) > 0.02 && bulged_before[v].distance(on_ball(0.02)) < 0.2);
            } else if plain_before[v].x > 0.0 {
                // Clear of the hidden patch's rim, whose shape the vertices next to it do read.
                worst = worst.max(plain[v].distance(bulged[v]));
            }
        }
        assert!(bulging > 20, "{mode:?}: only {bulging} bulged vertices are near the stroke: the test proves nothing");
        assert!(plain.iter().zip(&plain_before).any(|(a, b)| a != b), "{mode:?} did nothing");
        assert!(worst < 2e-5, "{mode:?}: the hidden bulge moved the surface that shows by {worst}");
    }
}

// ------------------------------------------------------------------ Flatten

/// A Flatten stroke over a bumpy ball in one mode, inverted or not: where
/// every vertex was and where it went.
fn flattened(mode: FlattenMode, lock: bool, invert: bool) -> (Vec<Vec3>, Vec<Vec3>) {
    let mut mesh = rough_ball(120_000);
    let before = mesh.positions.clone();
    let view = view_of(&mesh);
    let mut sculptor = sculptor_with(Brush::Flatten, 0.1, 0.6);
    let bs = sculptor.settings.current_mut();
    (bs.flatten_mode, bs.lock_plane) = (mode, lock);
    draw_as(&mut mesh, &mut sculptor, &view, &across(&view, -0.25, 0.25, 60), invert);
    (before, mesh.positions)
}

/// How far the vertices of a stroke went out from the ball and in toward it,
/// at the most, and how many moved.
fn out_and_in(before: &[Vec3], after: &[Vec3]) -> (f32, f32, usize) {
    before.iter().zip(after).fold((0.0f32, 0.0f32, 0), |(out, inward, moved), (was, now)| {
        let went = (*now - *was).dot(was.normalize());
        (out.max(went), inward.max(-went), moved + usize::from(now != was))
    })
}

/// Scrape only brings the surface down and Fill only brings it up, where
/// Both does both; and with the brush inverted each of the two is the other.
#[test]
fn scrape_only_lowers_and_fill_only_raises() {
    let (before, both) = flattened(FlattenMode::Both, false, false);
    let (out, inward, moved) = out_and_in(&before, &both);
    assert!(moved > 500 && out > 0.002 && inward > 0.002, "Both: {moved} moved, {out} out, {inward} in");

    let (_, scraped) = flattened(FlattenMode::Scrape, false, false);
    let (out, inward, moved) = out_and_in(&before, &scraped);
    println!("Scrape: {moved} vertices moved, {inward:.4} in at most and {out:.2e} out");
    assert!(moved > 300 && inward > 0.002, "Scrape did too little: {moved} moved, {inward} in");
    assert!(out < inward * 0.02, "Scrape raised something by {out}");

    let (_, filled) = flattened(FlattenMode::Fill, false, false);
    let (out, inward, moved) = out_and_in(&before, &filled);
    println!("Fill: {moved} vertices moved, {out:.4} out at most and {inward:.2e} in");
    assert!(moved > 300 && out > 0.002, "Fill did too little: {moved} moved, {out} out");
    assert!(inward < out * 0.02, "Fill lowered something by {inward}");

    // Ctrl with Scrape fills, and with Fill scrapes: the very same strokes.
    assert!(flattened(FlattenMode::Scrape, false, true).1 == filled, "Scrape inverted is not Fill");
    assert!(flattened(FlattenMode::Fill, false, true).1 == scraped, "Fill inverted is not Scrape");
    // And Both inverted is still its own thing: it pushes the bumps further out.
    let (_, contrast) = flattened(FlattenMode::Both, false, true);
    assert!(contrast != both && contrast != scraped && contrast != filled);
}

/// Lock Plane: the whole stroke levels to the plane its first dab found, so
/// a stroke across a ball cuts one flat facet where a plain Flatten follows
/// the ball round; and a stroke that wanders off the plane stops working
/// rather than hauling the surface up to it.
#[test]
fn lock_plane_cuts_one_flat_facet_and_stops_where_the_plane_leaves_the_surface() {
    let radius = 0.12;
    let stroke = |lock: bool, mode: FlattenMode, to: f32| {
        let mut mesh = sphere(120_000);
        let before = mesh.positions.clone();
        let view = view_of(&mesh);
        let mut sculptor = sculptor_with(Brush::Flatten, radius, 1.0);
        // A plane a little under the surface, so there is something to cut.
        sculptor.settings.flatten_offset = -0.15;
        let bs = sculptor.settings.current_mut();
        (bs.flatten_mode, bs.lock_plane) = (mode, lock);
        // One stroke, from the front of the ball round toward its side and
        // back, twice. (A second stroke would begin on the facet the first
        // had cut and lock a plane of its own, lower by the offset again.)
        let there = across(&view, 0.0, to, 60);
        let back: Vec<Vec2> = there.iter().rev().copied().collect();
        let path = [there.clone(), back.clone(), there, back].concat();
        draw(&mut mesh, &mut sculptor, &view, &path, 4, None);
        (before, mesh.positions)
    };
    // The surface right along the first stretch of the path: how high it
    // lies (along Z, the way the ball faces where the stroke begins), at the
    // lowest and the highest.
    let along_the_path = |before: &[Vec3], after: &[Vec3]| {
        let there = before.iter().zip(after).filter(|(was, _)| was.z > 0.0 && was.y.abs() < 0.03 && (0.0..0.1).contains(&was.x));
        there.fold((f32::MAX, f32::MIN, 0), |(low, high, count), (_, now)| (low.min(now.z), high.max(now.z), count + 1))
    };
    for mode in [FlattenMode::Both, FlattenMode::Scrape] {
        let (before, cut) = stroke(true, mode, 0.25);
        let (low, high, count) = along_the_path(&before, &cut);
        println!("{mode:?}, locked: {count} vertices along the path lie at z {low:.4} to {high:.4}");
        assert!(count > 60 && high - low < 0.002, "{mode:?}: the facet is {} from flat (z {low} to {high})", high - low);
        assert!(high < 0.5 - 0.01, "{mode:?}: the facet is at {high}, no lower than the ball");
    }
    // Unlocked, the same stroke follows the ball round: no one plane.
    let (before, followed) = stroke(false, FlattenMode::Both, 0.25);
    let (low, high, _) = along_the_path(&before, &followed);
    println!("unlocked: z {low:.4} to {high:.4}");
    assert!(high - low > 0.006, "unlocked, the path lies at z {low} to {high}");

    // A long way round, where the ball has fallen more than half the
    // brush's size below the plane: nothing is hauled up to it, in either
    // mode that raises the surface, and nothing anywhere is moved further
    // than that half.
    for mode in [FlattenMode::Both, FlattenMode::Fill] {
        let (before, far) = stroke(true, mode, 0.95);
        let most = before.iter().zip(&far).map(|(was, now)| was.distance(*now)).fold(0.0f32, f32::max);
        assert!(most < radius * 0.5, "{mode:?}: a vertex was moved {most}, for a brush of {radius}");
        let round_the_side = before.iter().zip(&far).filter(|(was, _)| was.z > 0.0 && round(**was) > 0.85);
        let (count, moved) = round_the_side.fold((0, 0.0f32), |(count, moved), (was, now)| (count + 1, moved.max(was.distance(*now))));
        println!("{mode:?}, locked, a long way round: moved {most:.4} at most; past 0.85 radians {count} vertices, moved {moved:.2e}");
        assert!(count > 500 && moved < 1e-4, "{mode:?}: round the side, out of the plane's reach, {count} vertices moved up to {moved}");
    }
}

// -------------------------------------------------------------------- Pinch

/// A ball with a soft ridge raised across its front, along Y = 0.
fn ridged_ball() -> Mesh {
    let ball = sphere(200_000);
    let ridged = ball.positions.iter().map(|&p| {
        let rise = 0.03 * (-(p.y / 0.04).powi(2)).exp() * super::kernels::smooth((0.42 - round(p).abs()) / 0.1) * f32::from(p.z > 0.0);
        p * (1.0 + rise / 0.5)
    });
    Mesh::new(ridged.collect(), ball.indices.clone(), None)
}

/// How wide the ridge of a [`ridged_ball`] is in the middle of its length
/// (the spread of its height about its crest, root mean square) and how tall.
fn ridge_shape(mesh: &Mesh) -> (f32, f32) {
    let there = || mesh.positions.iter().filter(|p| p.z > 0.0 && round(**p).abs() < 0.15 && p.y.abs() < 0.12);
    let (weight, second) = there().fold((0.0f64, 0.0f64), |(weight, second), p| {
        let h = height(*p).max(0.0) as f64;
        (weight + h, second + h * (p.y as f64).powi(2))
    });
    ((second / weight).sqrt() as f32, there().map(|p| height(*p)).fold(0.0f32, f32::max))
}

/// Pinch draws the surface in to the stroke's line, across the way it is
/// going: a soft ridge gets narrower and no lower. Inverted, it spreads the
/// ridge wider. Neither carries the surface along the stroke.
#[test]
fn pinch_narrows_a_ridge_and_its_inverse_widens_it() {
    let pinched = |invert: bool| {
        let mut mesh = ridged_ball();
        let before = mesh.positions.clone();
        let view = view_of(&mesh);
        let mut sculptor = sculptor_with(Brush::Pinch, 0.1, 0.6);
        draw_as(&mut mesh, &mut sculptor, &view, &across(&view, -0.3, 0.3, 80), invert);
        // How far it carried the surface along the stroke and across it,
        // in the middle of its length.
        let (along, over) = before.iter().zip(&mesh.positions).filter(|(was, _)| was.z > 0.0 && round(**was).abs() < 0.15).fold(
            (0.0f32, 0.0f32),
            |(along, over), (was, now)| (along.max((round(*now) - round(*was)).abs() * 0.5), over.max((now.y - was.y).abs())),
        );
        (ridge_shape(&mesh), along, over)
    };
    let (wide, tall) = ridge_shape(&ridged_ball());
    let ((narrower, still_tall), along, over) = pinched(false);
    println!("Pinch: the ridge went from {wide:.4} wide and {tall:.4} tall to {narrower:.4} and {still_tall:.4}; carried {along:.2e} along the stroke, {over:.2e} across");
    assert!(narrower < wide * 0.8, "pinched, the ridge is {narrower} wide, from {wide}");
    assert!((still_tall / tall - 1.0).abs() < 0.08, "pinched, the ridge is {still_tall} tall, from {tall}");
    assert!(over > 0.004 && along < over * 0.05, "it carried the surface {along} along the stroke and {over} across");

    let ((wider, spread_tall), along, over) = pinched(true);
    println!("Pinch inverted: {wider:.4} wide and {spread_tall:.4} tall; carried {along:.2e} along, {over:.2e} across");
    assert!(wider > wide * 1.15, "spread, the ridge is {wider} wide, from {wide}");
    assert!((spread_tall / tall - 1.0).abs() < 0.08);
    assert!(along < over * 0.05);
}

/// A press that never sets off has no line to draw the surface in to, only
/// a point: it pinches toward its own middle (inverted, spreads from it). A
/// brush held still with Build Up on Hold goes on doing that.
#[test]
fn a_pinch_that_never_sets_off_draws_in_to_its_point() {
    for (invert, hold_frames) in [(false, 0), (true, 0), (false, 40)] {
        let mut mesh = sphere(80_000);
        let before = mesh.positions.clone();
        let view = view_of(&mesh);
        let mut sculptor = sculptor_with(Brush::Pinch, 0.12, 1.0);
        sculptor.settings.build_up_on_hold = hold_frames > 0;
        let press = PointerSample { pos: SIZE * 0.5, pressure: None, time: 0.0 };
        assert!(sculptor.begin(&mut mesh, &view, press, invert, false));
        (0..hold_frames).for_each(|_| sculptor.frame(&mut mesh, &view, 1.0 / 60.0));
        sculptor.end(&mut mesh, &view);
        let dabs = sculptor.dabs_applied;
        assert!(if hold_frames > 0 { dabs >= 2 } else { dabs == 1 }, "{dabs} dabs");
        let front = Vec3::new(0.0, 0.0, 0.5);
        let (mut nearer, mut further, mut most) = (0, 0, 0.0f32);
        for (was, now) in before.iter().zip(&mesh.positions).filter(|(was, now)| was != now) {
            let closer = was.distance(front) - now.distance(front);
            nearer += usize::from(closer > 0.0);
            further += usize::from(closer < 0.0);
            most = most.max(closer.abs());
            assert!((now.length() - 0.5).abs() < 0.004, "a pinch carved or raised the surface: {}", now.length() - 0.5);
        }
        println!("invert {invert}, held {hold_frames} frames: {nearer} nearer the point, {further} further, by {most:.4} at most");
        assert!(most > 0.002, "the press did nothing");
        assert!(if invert { nearer == 0 && further > 300 } else { further == 0 && nearer > 300 }, "{nearer} nearer, {further} further");
    }
}

/// A Pinch stroke opens along its line: the first dab waits to learn which
/// way the stroke sets off, so the head of the stroke is not drawn in
/// toward a point and carried along the stroke. (Crease, which sets every
/// vertex back where it began along the stroke as it goes, opens at once as
/// it always has.)
#[test]
fn a_pinch_stroke_opens_along_its_line() {
    let mut mesh = sphere(200_000);
    let before = mesh.positions.clone();
    let view = view_of(&mesh);
    let mut sculptor = sculptor_with(Brush::Pinch, 0.1, 1.0);
    let path = across(&view, -0.3, 0.3, 80);
    let sample = |i: usize| PointerSample { pos: path[i], pressure: None, time: i as f64 / 240.0 };
    assert!(sculptor.begin(&mut mesh, &view, sample(0), false, false));
    let at_the_press = sculptor.dabs_applied;
    for i in 1..path.len() {
        sculptor.sample(&mut mesh, &view, sample(i));
        if i % 4 == 0 {
            sculptor.frame(&mut mesh, &view, 1.0 / 60.0);
            let _ = mesh.take_dirty();
        }
    }
    sculptor.end(&mut mesh, &view);
    // How far the surface was carried along the stroke, at its head and in
    // the middle of it, against how far it was drawn in across.
    let carried = |from: f32, to: f32| {
        let there = before.iter().zip(&mesh.positions).filter(|(was, _)| was.z > 0.0 && (from..to).contains(&round(**was)));
        there.fold((0.0f32, 0.0f32), |(along, over), (was, now)| {
            (along.max((round(*now) - round(*was)).abs() * 0.5), over.max((now.y - was.y).abs()))
        })
    };
    let ((at_head, _), (further_on, drawn_in)) = (carried(-0.6, -0.2), carried(-0.1, 0.1));
    println!("carried along the stroke: {at_head:.2e} at its head, {further_on:.2e} further on; drawn in {drawn_in:.2e} across");
    assert!(drawn_in > 0.01, "the stroke drew the surface in by only {drawn_in}");
    assert!(at_head < drawn_in * 0.05, "the head of the stroke carried the surface {at_head} along it");
    assert_eq!(at_the_press, 0, "the opening dab waits for the stroke to set off");

    // Crease lays its first dab at the press.
    let mut mesh = sphere(40_000);
    let mut sculptor = sculptor_with(Brush::Crease, 0.1, 0.5);
    assert!(sculptor.begin(&mut mesh, &view, sample(0), false, false));
    assert_eq!(sculptor.dabs_applied, 1);
    sculptor.end(&mut mesh, &view);
}

// -------------------------------------------------------------------- Nudge

/// One Nudge stroke across the front of the ball, from `from` radians round
/// to `to`: where each vertex was, where it went, and how many dabs.
fn nudged(strength: f32, spacing: f32, from: f32, to: f32, setup: &dyn Fn(&mut Sculptor)) -> (Vec<Vec3>, Vec<Vec3>, u64) {
    let mut mesh = sphere(120_000);
    let before = mesh.positions.clone();
    let view = view_of(&mesh);
    let mut sculptor = sculptor_with(Brush::Nudge, 0.1, strength);
    sculptor.settings.current_mut().spacing = spacing;
    setup(&mut sculptor);
    let dabs = draw(&mut mesh, &mut sculptor, &view, &across(&view, from, to, 60), 4, None);
    (before, mesh.positions, dabs)
}

/// The vertex of `positions` nearest `at`.
fn nearest(positions: &[Vec3], at: Vec3) -> usize {
    (0..positions.len()).min_by(|&a, &b| positions[a].distance(at).total_cmp(&positions[b].distance(at))).unwrap()
}

/// Nudge slides the surface along the stroke: at full Strength what was
/// under the middle of the ring when the stroke began goes with the pen, at
/// half Strength half as far; it stays on the surface (the ball stays a
/// ball); and how far it goes does not depend on Spacing.
#[test]
fn nudge_carries_the_surface_with_the_pen_by_its_strength_within_the_surface() {
    // A short push, half the brush's size: the surface under the middle of
    // the ring is still well inside it at the end.
    let travel = 0.05;
    let push = |strength: f32, spacing: f32| {
        let (before, after, dabs) = nudged(strength, spacing, 0.0, travel / 0.5, &|_| {});
        let v = nearest(&before, on_ball(0.0));
        let went = after[v] - before[v];
        let off = after.iter().map(|p| (p.length() - 0.5).abs()).fold(0.0f32, f32::max);
        // Sideways from the stroke and along it.
        let stray = before.iter().zip(&after).map(|(was, now)| (now.y - was.y).abs()).fold(0.0f32, f32::max);
        (went.x, went.length(), off, stray, dabs)
    };
    let (along, whole, off, stray, dabs) = push(1.0, 0.05);
    println!("Nudge at 100%: the surface under the pen went {along:.4} of {travel} ({dabs} dabs); furthest off the ball {off:.2e}, furthest sideways {stray:.2e}");
    assert!((0.8..=1.0).contains(&(along / travel)), "at 100% the surface went {along}, the pen {travel}");
    assert!(whole < along * 1.02, "it went {whole} in all, {along} along the stroke");
    assert!(off < 0.5 * 4e-4, "a vertex is {off} off the ball: Nudge is meant to slide the surface, not raise or sink it");
    assert!(stray < travel * 0.05, "the surface went {stray} sideways");
    let (half, ..) = push(0.5, 0.05);
    assert!((0.4..=0.56).contains(&(half / along)), "at 50% it went {half}, at 100% {along}");
    // A longer push leaves a trail: the surface the brush has passed over
    // has been carried forward and left there. How far does not depend on
    // Spacing: the steps of a stroke add up to its travel however many
    // there are.
    let trail = |spacing: f32| {
        let (before, after, dabs) = nudged(0.3, spacing, -0.3, 0.3, &|_| {});
        let on_it = before.iter().zip(&after).filter(|(was, _)| was.z > 0.0 && was.y.abs() < 0.015 && round(**was).abs() < 0.12);
        let (count, sum, least) = on_it.fold((0, 0.0f64, f32::MAX), |(count, sum, least), (was, now)| {
            (count + 1, sum + (now.x - was.x) as f64, least.min(now.x - was.x))
        });
        assert!(count > 30 && least > 0.004, "at {spacing}: the trail went forward {least} at the least ({count} vertices)");
        (sum / count as f64, dabs)
    };
    let trails = [0.05, 0.2, 0.4].map(trail);
    let carried = trails.map(|(carried, _)| carried);
    println!("the trail at 5%, 20% and 40% spacing: {carried:.4?} with {:?} dabs, {:.1}% apart", trails.map(|t| t.1), spread(&carried) * 100.0);
    assert!(spread(&carried) < 0.06, "{carried:?}");
}

/// A pen held still nudges nothing, whatever Build Up on Hold says, and
/// neither does a single press: there is no way come to carry anything.
#[test]
fn a_nudge_that_goes_nowhere_moves_nothing() {
    let mut mesh = sphere(40_000);
    let before = mesh.positions.clone();
    let view = view_of(&mesh);
    let mut sculptor = sculptor_with(Brush::Nudge, 0.1, 1.0);
    sculptor.settings.build_up_on_hold = true;
    assert!(sculptor.begin(&mut mesh, &view, PointerSample { pos: SIZE * 0.5, pressure: None, time: 0.0 }, false, false));
    (0..40).for_each(|_| sculptor.frame(&mut mesh, &view, 1.0 / 60.0));
    assert!(sculptor.end(&mut mesh, &view).is_none(), "a nudge that went nowhere left an undo step");
    assert!(mesh.positions == before);
}

/// Nudge under a mirror: a stroke along the plane carries the surface as
/// far as the same stroke with no symmetry (its two copies are one stroke
/// there), and leaves the ball a mirror image of itself; so does a stroke
/// beside the plane, each side carried the mirrored way.
#[test]
fn nudge_is_as_strong_on_a_mirror_plane_and_stays_mirrored() {
    let view = view_of(&sphere(120_000));
    // Up the screen, along the X = 0 plane or a third of the brush beside it.
    let up_the_plane = |beside: f32, mirror: bool| {
        let mut mesh = sphere(120_000);
        let before = mesh.positions.clone();
        let twins = twins(&mesh);
        let mut sculptor = sculptor_with(Brush::Nudge, 0.1, 1.0);
        sculptor.settings.symmetry = [mirror, false, false];
        let path: Vec<Vec2> =
            (0..=40).map(|i| view.project(Vec3::new(beside, -0.1 + 0.2 * i as f32 / 40.0, 0.48).normalize() * 0.5).unwrap()).collect();
        draw(&mut mesh, &mut sculptor, &view, &path, 4, None);
        let most = before.iter().zip(&mesh.positions).map(|(was, now)| was.distance(*now)).fold(0.0f32, f32::max);
        let flip = Vec3::new(-1.0, 1.0, 1.0);
        let off = twins.iter().enumerate().map(|(v, &t)| mesh.positions[t as usize].distance(mesh.positions[v] * flip)).fold(0.0f32, f32::max);
        (most, off)
    };
    for beside in [0.0, 0.033] {
        let ((alone, _), (mirrored, off)) = (up_the_plane(beside, false), up_the_plane(beside, true));
        println!("{beside} from the plane: carried {mirrored:.4} mirrored against {alone:.4} alone; mirror error {off:.2e}");
        assert!(alone > 0.03, "the stroke carried nothing: {alone}");
        // Give or take what every stroke that adds is allowed beside a plane (`symmetry_tests`).
        assert!((0.9..=1.2).contains(&(mirrored / alone)), "{beside} from the plane: {mirrored} against {alone} alone");
        assert!(off < mirrored * 0.01, "{off} off its mirror image");
    }
}

// ------------------------------------------------- spacing, and resting

/// The kernels' options keep Spacing's promise (it changes how smooth a
/// stroke is, not how much it does), as the plain kernels do: at half the
/// brush's own spacing, at its own and at twice it, each does the same
/// along the middle of a stroke, give or take a few percent.
#[test]
fn the_options_do_as_much_at_any_spacing() {
    type Setup = fn(&mut BrushSettings);
    let cases: [(&str, Brush, fn() -> Mesh, Setup); 7] = [
        ("Smooth, Keep Volume", Brush::Smooth, || rough_ball(120_000), |b| b.keep_volume = 1.0),
        ("Smooth, Peaks", Brush::Smooth, || rough_ball(120_000), |b| b.smooth_mode = SmoothMode::Peaks),
        ("Smooth, Valleys", Brush::Smooth, || rough_ball(120_000), |b| b.smooth_mode = SmoothMode::Valleys),
        ("Smooth, Relax", Brush::Smooth, stretched_ball, |b| b.smooth_mode = SmoothMode::Relax),
        ("Flatten, Scrape", Brush::Flatten, || rough_ball(120_000), |b| b.flatten_mode = FlattenMode::Scrape),
        ("Flatten, Fill", Brush::Flatten, || rough_ball(120_000), |b| b.flatten_mode = FlattenMode::Fill),
        ("Pinch", Brush::Pinch, ridged_ball, |_| {}),
    ];
    let mut apart = Vec::new();
    for (name, brush, ball, setup) in cases {
        let own = brush.default_spacing();
        let did = [own * 0.5, own, own * 2.0].map(|spacing| {
            let mut mesh = ball();
            let before = mesh.positions.clone();
            let view = view_of(&mesh);
            let mut sculptor = sculptor_with(brush, 0.1, if brush == Brush::Smooth { 0.05 } else { 0.1 });
            let bs = sculptor.settings.current_mut();
            bs.spacing = spacing;
            setup(bs);
            draw(&mut mesh, &mut sculptor, &view, &across(&view, -0.35, 0.35, 120), 4, None);
            let middle = before.iter().zip(&mesh.positions).filter(|(was, _)| was.z > 0.0 && round(**was).abs() < 0.12);
            middle.map(|(was, now)| was.distance(*now) as f64).sum::<f64>()
        });
        println!("{name}: did {did:.4?} at half, its own and twice its spacing, {:.1}% apart", spread(&did) * 100.0);
        assert!(did[1] > 0.0, "{name} did nothing");
        if spread(&did) >= 0.05 {
            apart.push(format!("{name}: {did:?}"));
        }
    }
    assert!(apart.is_empty(), "{apart:#?}");
}

/// Smooth and Flatten keep working while the pointer rests in every mode,
/// as they always have in their plain ones.
#[test]
fn the_options_keep_working_while_the_pointer_rests() {
    type Setup = fn(&mut BrushSettings);
    let cases: [(&str, Brush, Setup); 6] = [
        ("Smooth, Keep Volume", Brush::Smooth, |b| b.keep_volume = 1.0),
        ("Smooth, Peaks", Brush::Smooth, |b| b.smooth_mode = SmoothMode::Peaks),
        ("Smooth, Valleys", Brush::Smooth, |b| b.smooth_mode = SmoothMode::Valleys),
        ("Flatten, Scrape", Brush::Flatten, |b| b.flatten_mode = FlattenMode::Scrape),
        ("Flatten, Fill", Brush::Flatten, |b| b.flatten_mode = FlattenMode::Fill),
        ("Flatten, Lock Plane", Brush::Flatten, |b| b.lock_plane = true),
    ];
    for (name, brush, setup) in cases {
        let rested = |frames: usize| {
            let mut mesh = rough_ball(80_000);
            let before = mesh.positions.clone();
            let view = view_of(&mesh);
            let mut sculptor = sculptor_with(brush, 0.12, 0.3);
            setup(sculptor.settings.current_mut());
            assert!(sculptor.begin(&mut mesh, &view, PointerSample { pos: SIZE * 0.5, pressure: None, time: 0.0 }, false, false));
            (0..frames).for_each(|_| sculptor.frame(&mut mesh, &view, 1.0 / 60.0));
            sculptor.end(&mut mesh, &view);
            let moved = before.iter().zip(&mesh.positions).map(|(was, now)| was.distance(*now) as f64).sum::<f64>();
            (sculptor.dabs_applied, moved)
        };
        let ((one, tap), (many, held)) = (rested(0), rested(60));
        println!("{name}: a tap is {one} dab and moves {tap:.4}; a second's rest is {many} dabs and moves {held:.4}");
        assert!(one == 1 && many >= 10, "{name}: {one} and {many} dabs");
        assert!(tap > 0.0 && held > tap * 2.0, "{name}: a rest did {held}, a tap {tap}");
    }
}
