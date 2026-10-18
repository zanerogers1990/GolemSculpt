//! Painted drawing without the graphics card: the weight maths of the
//! fragment shader (its CPU twin), which objects take the painted path, what
//! counts as a change of the picture, and the shaders made for each palette.

use super::*;
use crate::materials::custom;

/// A small, repeatable random number generator for the weight tests.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// A vertex's weights as the composite stores them: bytes.
fn bytes(w: [f32; 8]) -> [f32; 8] {
    w.map(|x| (x * 255.0).round() / 255.0)
}

#[test]
fn three_materials_meeting_on_a_triangle_blend_only_convexly() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let all = [true; 8];
    // Each corner all one material, a different one at each: at any point
    // the weights are its barycentric coordinates, and no other material.
    let corners = [0usize, 3, 6].map(|k| {
        let mut w = [0.0; 8];
        w[k] = 1.0;
        w
    });
    for _ in 0..2000 {
        let (a, b) = (rng.next(), rng.next());
        let (a, b) = if a + b > 1.0 { (1.0 - a, 1.0 - b) } else { (a, b) };
        let bary = [a, b, 1.0 - a - b];
        let raw: [f32; 8] = std::array::from_fn(|k| (0..3).map(|c| corners[c][k] * bary[c]).sum());
        let (base, w) = resolve_weights(raw, all);
        let total = base + w.iter().sum::<f32>();
        assert!((total - 1.0).abs() < 1e-5, "{total}");
        assert!(base == 0.0, "the Look's finish has no share where every corner is fully painted");
        for k in [1, 2, 4, 5, 7] {
            assert_eq!(w[k], 0.0, "no material that is at no corner");
        }
        for (c, k) in [0usize, 3, 6].into_iter().enumerate() {
            if bary[c] >= EPS {
                assert!((w[k] - bary[c]).abs() < 0.01, "material {k} at {bary:?}: {w:?}");
            }
        }
    }
    // Any corners of the composite's kind (bytes, summing to at most one,
    // some of the Look's finish left): every point is a mixture.
    for _ in 0..5000 {
        let corner = |rng: &mut Rng| {
            let mut w: [f32; 8] = std::array::from_fn(|_| if rng.next() < 0.4 { rng.next() } else { 0.0 });
            let sum: f32 = w.iter().sum();
            let room = rng.next();
            if sum > room {
                w.iter_mut().for_each(|x| *x *= room / sum);
            }
            bytes(w)
        };
        let c = [corner(&mut rng), corner(&mut rng), corner(&mut rng)];
        let (a, b) = (rng.next(), rng.next());
        let (a, b) = if a + b > 1.0 { (1.0 - a, 1.0 - b) } else { (a, b) };
        let bary = [a, b, 1.0 - a - b];
        let raw: [f32; 8] = std::array::from_fn(|k| (0..3).map(|i| c[i][k] * bary[i]).sum());
        let (base, w) = resolve_weights(raw, all);
        assert!(base >= 0.0 && w.iter().all(|x| *x >= 0.0 && *x <= 1.0));
        assert!((base + w.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        for k in 0..8 {
            let most = c.iter().map(|corner| corner[k]).fold(0.0f32, f32::max);
            // Weights dropped below a byte's step give their share to the rest.
            assert!(w[k] <= most / (1.0 - 8.0 * EPS) + 1e-5 || base == 0.0, "material {k}: {} over {most}", w[k]);
            if most == 0.0 {
                assert_eq!(w[k], 0.0, "no material that is at no corner");
            }
        }
    }
}

#[test]
fn weights_are_made_a_mixture() {
    let none = [false; 8];
    let all = [true; 8];
    assert_eq!(resolve_weights([0.0; 8], all), (1.0, [0.0; 8]), "bare: the Look's finish alone");
    // Rounded bytes that come to more than one are scaled down.
    let (base, w) = resolve_weights([0.6, 0.6, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], all);
    assert_eq!(base, 0.0);
    assert!((w[0] - 0.5).abs() < 1e-6 && (w[1] - 0.5).abs() < 1e-6);
    // A weight below a byte's step is no weight; one with no palette row too.
    let (base, w) = resolve_weights([0.5, 0.001, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], all);
    assert_eq!((base, w[1]), (0.5, 0.0));
    assert_eq!(resolve_weights([0.5; 8], none), (1.0, [0.0; 8]));
    // A sliver of the Look's finish goes to the materials.
    let (base, w) = resolve_weights([0.999, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], all);
    assert_eq!((base, w[0]), (0.0, 1.0));
    // The shader's threshold is this one.
    let wgsl = include_str!("../painted.wgsl");
    let line = wgsl.lines().find(|l| l.starts_with("const PAINT_EPS")).unwrap();
    let value: f32 = line.split('=').nth(1).unwrap().trim().trim_end_matches(';').parse().unwrap();
    assert!((value - EPS).abs() < 1e-9);
}

#[test]
fn only_an_object_with_paint_showing_takes_the_painted_path() {
    assert!(!drawn_painted(false, true, true), "an object without paint is drawn as it always was");
    assert!(drawn_painted(true, true, false));
    assert!(drawn_painted(true, false, true));
    assert!(!drawn_painted(true, false, false), "with both switches off, paint is not drawn at all");
}

#[test]
fn a_paint_upload_is_a_change_of_the_picture() {
    let ranges = vec![10..20, 40..41];
    assert_eq!(Sync::plan(false, 0, 100, false, &[]), Sync::Nothing, "no paint, nothing to send");
    assert_eq!(Sync::plan(false, 100, 100, false, &[]), Sync::Create, "the first paint");
    assert_eq!(Sync::plan(true, 100, 100, false, &ranges), Sync::Write(ranges.clone()), "a stroke: its ranges only");
    assert_eq!(Sync::plan(true, 100, 100, false, &[]), Sync::Nothing);
    assert_eq!(Sync::plan(true, 150, 150, true, &ranges), Sync::Create, "outgrown by dynamic topology: all again");
    assert_eq!(Sync::plan(true, 0, 100, false, &[]), Sync::Drop, "paint taken off");
    assert_eq!(Sync::plan(true, 90, 100, false, &ranges), Sync::Drop, "a composite not fitted to the mesh is not drawn");
    for plan in [Sync::Create, Sync::Write(ranges), Sync::Drop] {
        assert!(plan.changes_picture(), "{plan:?}");
    }
    assert!(!Sync::Nothing.changes_picture());

    // What the scene key reads moves with each: the mesh uploads' stamp (the
    // renderer bumps it on a change of plan) and the palette's.
    let mut uploads = cache::Stamp::default();
    let key = |stamps: &[u64]| {
        let mut key = cache::SceneKey::default();
        key.push(&stamps.to_vec().len()).push(&stamps[0]).push(&stamps[1]);
        key
    };
    let mut palette = cache::Stamp::default();
    let before = key(&[uploads.count(), palette.count()]);
    uploads.bump();
    assert_ne!(key(&[uploads.count(), palette.count()]), before, "a paint stroke's upload");
    let after_stroke = key(&[uploads.count(), palette.count()]);
    let rows = crate::materials::rows::Upload::default();
    palette.note(&[bytemuck::cast_slice(&rows.rows)]);
    palette.note(&[bytemuck::cast_slice(&rows.rows)]);
    let noted = key(&[uploads.count(), palette.count()]);
    assert_ne!(noted, after_stroke, "a palette change");
    palette.note(&[bytemuck::cast_slice(&rows.rows)]);
    assert_eq!(key(&[uploads.count(), palette.count()]), noted, "the same palette again is no change");
}

use super::super::cache;

fn kinds(list: &[Kind]) -> BTreeSet<Kind> {
    list.iter().copied().collect()
}

#[test]
fn the_painted_shader_compiles_for_any_palette() {
    validate(&source(&kinds(&[]), &[])).unwrap();
    for kind in [Kind::Plain, Kind::Lure, Kind::Metal, Kind::Clay, Kind::Wood, Kind::Resin, Kind::Glow, Kind::Custom, Kind::Swatch] {
        validate(&source(&kinds(&[kind]), &[])).unwrap_or_else(|e| panic!("{kind:?}: {e}"));
    }
    let every = kinds(&[Kind::Plain, Kind::Lure, Kind::Metal, Kind::Clay, Kind::Wood, Kind::Resin, Kind::Glow, Kind::Custom]);
    let example = Code {
        id: custom::EXAMPLE_ID.into(),
        function: format!("finish_{}", custom::EXAMPLE_ID),
        source: custom::EXAMPLE_WGSL.into(),
        hash: 1,
        curvature: true,
        back_depth: false,
    };
    let full = source(&every, std::slice::from_ref(&example));
    validate(&full).unwrap();
    assert!(full.contains("case 7u") && full.contains("finish_hammered_copper(s, e.params)"));
    // Only the finishes the palette has are in its shader.
    let clays = generated(&kinds(&[Kind::Clay]), &[]);
    assert!(clays.contains("finish_clay") && !clays.contains("finish_plastic") && !clays.contains("eval_custom"));
}

#[test]
fn no_finish_takes_screen_space_derivatives() {
    // A painted material is shaded inside a branch chosen pixel by pixel,
    // where dpdx, dpdy, fwidth and textureSample give garbage: none of the
    // finish functions, nor anything they call, may use them.
    let shader = concat!(include_str!("../shader.wgsl"), include_str!("../sculpt_pbr.wgsl"), include_str!("../painted.wgsl"));
    let finishes = [Kind::Plain, Kind::Lure, Kind::Metal, Kind::Clay, Kind::Wood, Kind::Resin, Kind::Glow];
    let full = format!("{shader}{}", generated(&kinds(&finishes), &[]));
    let module = naga::front::wgsl::parse_str(&full).unwrap();
    let mut checked = 0;
    for (handle, f) in module.functions.iter() {
        let name = f.name.clone().unwrap_or_default();
        if !(name.starts_with("finish_") || name == "eval_slot" || name == "evaluate_surface") {
            continue;
        }
        for used in custom::reachable(&module, handle) {
            let g = &module.functions[used];
            let what = custom::derivative_in(g);
            assert!(what.is_none(), "{name} reaches {:?}, which uses {what:?}", g.name);
        }
        checked += 1;
    }
    assert!(checked >= 8, "{checked}");
}
