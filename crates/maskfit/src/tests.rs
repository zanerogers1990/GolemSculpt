//! Mask Fit against round shapes whose answers are known: a ball for the head
//! and a shell around it, the inside a sphere a little larger than the head
//! and the outside a larger one again.

use glam::{Mat4, Vec3};

use crate::fit::{Face, refit, survey};

/// A ball of `radius`, triangles facing out (or in, `inward`).
fn ball(radius: f32, inward: bool) -> (Vec<Vec3>, Vec<u32>) {
    let (rings, segs) = (40usize, 64usize);
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
    (p, t)
}

/// Normals pointing the way the triangles face.
fn normals(p: &[Vec3], t: &[u32]) -> Vec<Vec3> {
    let mut n = vec![Vec3::ZERO; p.len()];
    for c in t.chunks_exact(3) {
        let [a, b, d] = [c[0], c[1], c[2]].map(|i| i as usize);
        let f = (p[b] - p[a]).cross(p[d] - p[a]);
        n[a] += f;
        n[b] += f;
        n[d] += f;
    }
    n.iter().map(|v| v.normalize_or_zero()).collect()
}

/// A shell: inside at `inner`, outside at `outer`, as one mesh.
fn shell(inner: f32, outer: f32) -> (Vec<Vec3>, Vec<u32>) {
    let (mut p, mut t) = ball(inner, true);
    let (q, u) = ball(outer, false);
    let base = p.len() as u32;
    p.extend(q);
    t.extend(u.into_iter().map(|i| i + base));
    (p, t)
}

#[test]
fn the_inside_and_the_wall_are_found() {
    let (hp, ht) = ball(10.0, false);
    let face = Face::new(&hp, &ht, Mat4::IDENTITY);
    let (p, t) = shell(10.1, 10.4);
    let s = survey(&p, &normals(&p, &t), &t, &face, 3.0, 2.0);
    let half = p.len() / 2;
    assert_eq!(s.inside_count(), half);
    assert!(s.inside[..half].iter().all(|&i| i) && s.inside[half..].iter().all(|&i| !i));
    for v in 0..half {
        assert!((s.off_face[v] - 0.1).abs() < 0.02, "off face {}", s.off_face[v]);
    }
    let thinnest = s.thinnest().unwrap();
    assert!((thinnest - 0.3).abs() < 0.03, "thinnest {thinnest}");
    assert_eq!(s.thin_count(0.2), 0);
    assert_eq!(s.thin_count(0.5), half);
    assert_eq!(s.close_count(0.3), half, "0.1 off the face is under half a 0.3 gap");
    assert_eq!(s.close_count(0.1), 0);
}

#[test]
fn refit_moves_only_the_inside_to_the_gap() {
    let (hp, ht) = ball(10.0, false);
    let face = Face::new(&hp, &ht, Mat4::IDENTITY);
    // Sculpted: the inside wobbles between touching the face and a long way off.
    let (mut p, t) = shell(10.1, 11.0);
    let half = p.len() / 2;
    for (k, v) in p[..half].iter_mut().enumerate() {
        *v *= if k % 2 == 0 { 1.0 } else { 1.04 };
    }
    let s = survey(&p, &normals(&p, &t), &t, &face, 3.0, 3.0);
    let mask = vec![0.0; p.len()];
    let moved = refit(&p, &mask, None, &s, &face, 0.3, 3.0);
    for v in 0..half {
        assert!((moved[v].length() - 10.3).abs() < 0.02, "inside at {}", moved[v].length());
    }
    assert_eq!(&moved[half..], &p[half..], "the outside does not move");
}

#[test]
fn a_masked_point_stays_and_a_placed_head_is_met_where_it_stands() {
    // The head stands 5 to the right and twice as big: the shell is round it.
    let (hp, ht) = ball(5.0, false);
    let place = Mat4::from_scale_rotation_translation(Vec3::splat(2.0), glam::Quat::IDENTITY, Vec3::X * 5.0);
    let face = Face::new(&hp, &ht, place);
    let (mut p, t) = shell(10.5, 11.0);
    p.iter_mut().for_each(|v| *v += Vec3::X * 5.0);
    let s = survey(&p, &normals(&p, &t), &t, &face, 3.0, 3.0);
    let half = p.len() / 2;
    assert_eq!(s.inside_count(), half);
    let mut mask = vec![0.0; p.len()];
    mask[0] = 1.0;
    let moved = refit(&p, &mask, None, &s, &face, 0.2, 3.0);
    assert_eq!(moved[0], p[0]);
    assert!(((moved[1] - Vec3::X * 5.0).length() - 10.2).abs() < 0.02);
}

#[test]
fn an_inside_pushed_through_the_outside_reads_as_thin() {
    let (hp, ht) = ball(10.0, false);
    let face = Face::new(&hp, &ht, Mat4::IDENTITY);
    let (mut p, t) = shell(10.1, 10.5);
    let n = normals(&p, &t);
    let half = p.len() / 2;
    // A dent pushed in from outside, past the inside.
    let dented: Vec<usize> = (half..p.len()).filter(|&v| p[v].y > 10.4).collect();
    assert!(!dented.is_empty());
    for &v in &dented {
        p[v] *= 9.9 / 10.5;
    }
    let s = survey(&p, &n, &t, &face, 3.0, 3.0);
    for &v in &dented {
        assert!(!s.inside[v], "a dent is still the outside: its normal faces away from the head");
        assert!(s.wall[v] < 0.0, "wall {}", s.wall[v]);
    }
}

/// The whole way a mask is made, on a real head: the face masked, Extract
/// with Offset at the Gap and Thickness at the wall, the inside knocked about
/// as sculpting would, then Refit. Reads the head named by `MASKFIT_HEAD` (an
/// OBJ in millimetres, Y up, like `Mask Head.obj`) and prints what it found:
///
///     MASKFIT_HEAD="D:/Golem sculpts/Mask Heads/Mask Head.obj" cargo test -p sculpt-maskfit --release real_head -- --ignored --nocapture
#[test]
#[ignore]
fn real_head() {
    use sculpt_cut::{ExtractSettings, extract};
    let path = std::env::var("MASKFIT_HEAD").expect("MASKFIT_HEAD names the head");
    let (mut hp, mut ht) = (Vec::new(), Vec::new());
    for line in std::fs::read_to_string(path).unwrap().lines() {
        let mut w = line.split_whitespace();
        match w.next() {
            Some("v") => {
                let v: Vec<f32> = w.map(|x| x.parse().unwrap()).collect();
                hp.push(Vec3::new(v[0], v[1], v[2]) / 10.0); // millimetres to the app's units
            }
            Some("f") => {
                let f: Vec<u32> = w.map(|x| x.split('/').next().unwrap().parse::<u32>().unwrap() - 1).collect();
                for k in 1..f.len() - 1 {
                    ht.extend([f[0], f[k], f[k + 1]]);
                }
            }
            _ => {}
        }
    }
    let hn = normals(&hp, &ht);
    // The face, from brow to chin, with the eyes left open (in units: 10 mm each).
    let eyes = [Vec3::new(3.0, 18.9, 3.4), Vec3::new(-3.0, 18.9, 3.4)];
    let mask: Vec<f32> = hp
        .iter()
        .map(|p| {
            let face = p.z > 3.0 && p.y > 8.0 && p.y < 24.0;
            let eye = eyes.iter().any(|e| (*p - *e).length() < 1.6);
            if face && !eye { 1.0 } else { 0.0 }
        })
        .collect();
    // Extract's thickness and offset are shares of half the longest side.
    let (lo, hi) = hp.iter().fold((Vec3::MAX, Vec3::MIN), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
    let extent = ((hi - lo) * 0.5).max_element();
    let (gap, wall) = (0.3, 0.25); // 3 mm and 2.5 mm
    let settings = ExtractSettings {
        thickness_fraction: wall / extent,
        offset_fraction: gap / extent,
        ..ExtractSettings::default()
    };
    let shell = extract(&hp, &hn, &ht, &mask, &settings).unwrap();
    let (mut p, t) = (shell.positions, shell.indices);
    let face = Face::new(&hp, &ht, Mat4::IDENTITY);
    let mm = |v: f32| v * 10.0;
    let report = |what: &str, p: &[Vec3]| {
        let s = survey(p, &normals(p, &t), &t, &face, 3.0, 1.2);
        let off: Vec<f32> = s.off_face.iter().copied().filter(|v| v.is_finite()).collect();
        let mut hist = [0usize; 8];
        for d in &off {
            hist[((mm(*d) + 2.0).floor().clamp(0.0, 7.0)) as usize] += 1;
        }
        let walls: Vec<f32> = s.wall.iter().copied().filter(|v| v.is_finite()).collect();
        let mut wh = [0usize; 8];
        for d in &walls {
            wh[((mm(*d) + 2.0).floor().clamp(0.0, 7.0)) as usize] += 1;
        }
        println!("  off face mm from -2 by 1: {hist:?}; walls mm from -2 by 1: {wh:?}; outside with no wall in reach: {}", s.wall.iter().filter(|w| w.is_infinite()).count());
        let lo = off.iter().copied().fold(f32::INFINITY, f32::min);
        let hi = off.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        println!(
            "{what}: {} points, {} inside; inside {:.2}..{:.2} mm off the face; thinnest wall {:.2} mm; {} under 2 mm; {} closer than 1.5 mm",
            p.len(),
            s.inside_count(),
            mm(lo),
            mm(hi),
            mm(s.thinnest().unwrap_or(f32::NAN)),
            s.thin_count(0.2),
            s.close_count(gap),
        );
        s
    };
    let flipped = |a: &[Vec3], b: &[Vec3]| {
        t.chunks_exact(3)
            .filter(|c| {
                let n = |q: &[Vec3]| (q[c[1] as usize] - q[c[0] as usize]).cross(q[c[2] as usize] - q[c[0] as usize]);
                n(a).dot(n(b)) < 0.0
            })
            .count()
    };
    let made = report("extracted", &p);
    assert!(made.inside_count() > 1000);
    // Sculpting: the inside pushed in and out by up to 2 mm, smoothly, as a brush would.
    let before = p.clone();
    for (v, q) in p.iter_mut().enumerate() {
        if made.inside[v] {
            let n = Vec3::new(q.x, 0.0, q.z).normalize_or_zero();
            *q += n * 0.2 * (q.y * 1.3).sin() * (q.x * 0.9).cos();
        }
    }
    let sculpted = report("sculpted", &p);
    println!("triangles turned over by the sculpting, against the extract: {}", flipped(&before, &p));
    let mask = vec![0.0; p.len()];
    let fitted = refit(&p, &mask, None, &sculpted, &face, gap, 3.0);
    let after = report("refitted", &fitted);
    println!("triangles turned over by the refit, against the extract: {}", flipped(&before, &fitted));
    if let Ok(out) = std::env::var("MASKFIT_OUT") {
        write_obj(&format!("{out}/extracted.obj"), &before, &t, &made);
        write_obj(&format!("{out}/sculpted.obj"), &p, &t, &sculpted);
        write_obj(&format!("{out}/refitted.obj"), &fitted, &t, &after);
    }
    // All but the tightest hollows (the closed lips, the nostrils, which no shell this thick can follow) sit at the gap.
    let off: Vec<f32> = after.off_face.iter().copied().filter(|v| v.is_finite()).collect();
    let at_gap = off.iter().filter(|d| (*d - gap).abs() < 0.005).count() as f32 / off.len() as f32;
    println!("inside points at the gap after the refit: {:.1}%", at_gap * 100.0);
    assert!(at_gap > 0.9, "{at_gap}");
}

/// A shell as an OBJ in millimetres, each point coloured by what the survey
/// called it: inside blue, outside grey, rim green, thin or close red.
fn write_obj(path: &str, p: &[Vec3], t: &[u32], s: &crate::Survey) {
    let mut text = String::new();
    for (v, q) in p.iter().enumerate() {
        let c = if s.inside[v] {
            if s.off_face[v] < 0.15 { [1.0, 0.2, 0.1] } else { [0.3, 0.5, 1.0] }
        } else if s.wall[v].is_nan() {
            [0.2, 0.9, 0.3]
        } else if s.wall[v] < 0.2 {
            [1.0, 0.2, 0.1]
        } else {
            [0.8, 0.8, 0.8]
        };
        text += &format!("v {} {} {} {} {} {}
", q.x * 10.0, q.y * 10.0, q.z * 10.0, c[0], c[1], c[2]);
    }
    for c in t.chunks_exact(3) {
        text += &format!("f {} {} {}
", c[0] + 1, c[1] + 1, c[2] + 1);
    }
    std::fs::write(path, text).unwrap();
}
