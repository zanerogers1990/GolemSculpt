//! The tool on the test bench: a ball for the head and a shell over it.

use glam::Vec3;
use sculpt_geom::TriMesh;
use sculpt_toolkit::testing::Bench;
use sculpt_toolkit::{Key, Look, Scene};

use crate::MaskFit;
use crate::fit::Settings;

/// A ball of `radius` (in the app's units), facing out or in.
fn ball(radius: f32, inward: bool) -> TriMesh {
    let (rings, segs) = (32usize, 48usize);
    let mut p = vec![Vec3::Y * radius];
    for i in 1..rings {
        let th = std::f32::consts::PI * i as f32 / rings as f32;
        for j in 0..segs {
            let ph = std::f32::consts::TAU * j as f32 / segs as f32;
            p.push(Vec3::new(th.sin() * ph.cos(), th.cos(), th.sin() * ph.sin()) * radius);
        }
    }
    p.push(-Vec3::Y * radius);
    let at = |i: usize, j: usize| (1 + (i - 1) * segs + j % segs) as u32;
    let last = (p.len() - 1) as u32;
    let mut t = Vec::new();
    for j in 0..segs {
        t.extend([0, at(1, j + 1), at(1, j)]);
        t.extend([last, at(rings - 1, j), at(rings - 1, j + 1)]);
    }
    for i in 1..rings - 1 {
        for j in 0..segs {
            t.extend([at(i, j), at(i, j + 1), at(i + 1, j + 1)]);
            t.extend([at(i, j), at(i + 1, j + 1), at(i + 1, j)]);
        }
    }
    if inward {
        t.chunks_exact_mut(3).for_each(|c| c.swap(1, 2));
    }
    TriMesh::new(p, t)
}

/// A head 100 mm across and a shell round it: inside 1 mm off, wall 1.5 mm.
fn bench() -> (Bench, MaskFit) {
    let mut bench = Bench::new();
    bench.add("Head", ball(10.0, false));
    let mut shell = ball(10.1, true);
    let outer = ball(10.25, false);
    let base = shell.positions.len() as u32;
    shell.positions.extend(outer.positions);
    shell.indices.extend(outer.indices.iter().map(|i| i + base));
    bench.add("Mask", shell);
    let mut tool = MaskFit::default();
    bench.enter(&mut tool);
    (bench, tool)
}

#[test]
fn the_check_marks_a_thin_wall_and_a_close_inside() {
    let (mut bench, mut tool) = bench();
    bench.call(|cx| tool.settle(cx));
    let survey = tool.survey().expect("a look was taken");
    let half = survey.inside.len() / 2;
    assert_eq!(survey.inside_count(), half);
    let drawn = bench.frame(&mut tool);
    let looks: Vec<Look> = drawn.previews().iter().map(|p| p.style.look).collect();
    assert_eq!(looks, vec![Look::Solid, Look::Shell], "red marks on the outside, orange through the shell");
    // Defaults: 3 mm gap, 2 mm wall. A 1.5 mm wall is thin; 1 mm off the face is too close.
    assert!(!drawn.previews()[0].mesh.is_empty() && !drawn.previews()[1].mesh.is_empty());

    tool.set_settings(Settings { gap_mm: 1.0, min_wall_mm: 1.0 });
    bench.call(|cx| tool.settle(cx));
    let drawn = bench.frame(&mut tool);
    assert!(drawn.previews().iter().all(|p| p.mesh.is_empty()), "nothing to mark at a 1 mm gap and wall");
}

#[test]
fn refit_moves_the_inside_as_one_undo_step() {
    let (mut bench, mut tool) = bench();
    bench.call(|cx| tool.settle(cx));
    let mask = bench.doc.active();
    let before = bench.doc.mesh(mask).unwrap().positions.to_vec();
    tool.set_settings(Settings { gap_mm: 0.5, min_wall_mm: 0.5 });
    bench.call(|cx| tool.settle(cx));
    assert!(bench.key(&mut tool, Key::Enter));
    let after = bench.doc.mesh(mask).unwrap().positions.to_vec();
    let half = before.len() / 2;
    for v in 0..half {
        assert!((after[v].length() - 10.05).abs() < 0.01, "inside at {}", after[v].length());
    }
    assert_eq!(&after[half..], &before[half..]);
    let (text, ok) = bench.status.clone().unwrap();
    assert!(ok && text.starts_with("Refitted the inside: "), "{text}");
    assert!(bench.undo(&mut tool));
    assert_eq!(bench.doc.mesh(mask).unwrap().positions, &before[..]);
}

#[test]
fn without_a_head_it_says_what_it_needs() {
    let mut bench = Bench::new();
    bench.add("Mask", ball(1.0, false));
    let mut tool = MaskFit::default();
    bench.enter(&mut tool);
    bench.call(|cx| tool.settle(cx));
    assert!(tool.survey().is_none());
    bench.call(|cx| tool.refit_now(cx));
    let (text, ok) = bench.status.clone().unwrap();
    assert!(!ok && text.starts_with("Needs the head"), "{text}");
}
