//! Paint as data: the blend formulas against the W3C note's own worked out
//! in double precision, the rules for a vertex made between others, the
//! composite, strokes, undo, dynamic topology, subdivision levels and the
//! carrying of paint onto rebuilt meshes.

use glam::Vec3;
use sculpt_multires::Split;

use super::blend::{self, ColourBlend, Rgba};
use super::*;
use crate::history::History;
use crate::mesh::StrokeDelta;
use crate::mesh::groups::{self, Fit, Source};
use crate::mesh::levels::{stepped, subdivided};
use crate::mesh::primitives::{self, Primitive};

fn sphere(tris: usize) -> Mesh {
    primitives::build(Primitive::Sphere, tris, 0.5)
}

// ------------------------------------------------------------- the formulas

/// The note's formulas, written out again in double precision, apart from
/// the code under test.
mod reference {
    pub fn lum(c: [f64; 3]) -> f64 {
        0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2]
    }
    pub fn clip(c: [f64; 3]) -> [f64; 3] {
        let l = lum(c);
        let n = c[0].min(c[1]).min(c[2]);
        let x = c[0].max(c[1]).max(c[2]);
        let mut c = c;
        if n < 0.0 {
            c = c.map(|v| l + (v - l) * l / (l - n));
        }
        if x > 1.0 {
            c = c.map(|v| l + (v - l) * (1.0 - l) / (x - l));
        }
        c
    }
    pub fn set_lum(c: [f64; 3], l: f64) -> [f64; 3] {
        let d = l - lum(c);
        clip(c.map(|v| v + d))
    }
    pub fn sat(c: [f64; 3]) -> f64 {
        c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2])
    }
    pub fn set_sat(c: [f64; 3], s: f64) -> [f64; 3] {
        let mut i = [0, 1, 2];
        i.sort_by(|&a, &b| c[a].partial_cmp(&c[b]).unwrap());
        let mut out = [0.0; 3];
        if c[i[2]] > c[i[0]] {
            out[i[1]] = (c[i[1]] - c[i[0]]) * s / (c[i[2]] - c[i[0]]);
            out[i[2]] = s;
        }
        out
    }
    fn screen(b: f64, s: f64) -> f64 {
        b + s - b * s
    }
    fn hard(b: f64, s: f64) -> f64 {
        if s <= 0.5 { b * 2.0 * s } else { screen(b, 2.0 * s - 1.0) }
    }
    fn soft(b: f64, s: f64) -> f64 {
        if s <= 0.5 {
            b - (1.0 - 2.0 * s) * b * (1.0 - b)
        } else {
            let d = if b <= 0.25 { ((16.0 * b - 12.0) * b + 4.0) * b } else { b.sqrt() };
            b + (2.0 * s - 1.0) * (d - b)
        }
    }
    pub fn mix(mode: &str, b: [f64; 3], s: [f64; 3]) -> [f64; 3] {
        let each = |f: &dyn Fn(f64, f64) -> f64| [f(b[0], s[0]), f(b[1], s[1]), f(b[2], s[2])];
        match mode {
            "Normal" => s,
            "Multiply" => each(&|b, s| b * s),
            "Screen" => each(&screen),
            "Overlay" => each(&|b, s| hard(s, b)),
            "Darken" => each(&f64::min),
            "Lighten" => each(&f64::max),
            "Hard Light" => each(&hard),
            "Soft Light" => each(&soft),
            "Add" => each(&|b, s| (b + s).min(1.0)),
            "Subtract" => each(&|b, s| (b - s).max(0.0)),
            "Hue" => set_lum(set_sat(s, sat(b)), lum(b)),
            "Saturation" => set_lum(set_sat(b, sat(s)), lum(b)),
            "Colour" => set_lum(s, lum(b)),
            other => panic!("{other}"),
        }
    }
    /// The note's compositing: source `s` with alpha `a_s` over backdrop `b` with alpha `a_b`.
    pub fn over(mode: &str, s: [f64; 3], a_s: f64, b: [f64; 3], a_b: f64) -> ([f64; 3], f64) {
        let m = mix(mode, b, s);
        let ao = a_s + a_b * (1.0 - a_s);
        let mut out = [0.0; 3];
        for k in 0..3 {
            let cs = (1.0 - a_b) * s[k] + a_b * m[k];
            let co = a_s * cs + (1.0 - a_s) * a_b * b[k];
            out[k] = if ao > 0.0 { co / ao } else { 0.0 };
        }
        (out, ao)
    }
}

#[test]
fn every_blend_mode_matches_the_w3c_formula_worked_out_in_double_precision() {
    let colours: [[f64; 3]; 6] = [
        [0.0, 0.0, 0.0],
        [1.0, 1.0, 1.0],
        [0.2, 0.5, 0.9],
        [0.8, 0.3, 0.1],
        [0.5, 0.5, 0.5],
        [0.13, 0.27, 0.71],
    ];
    let alphas = [0.0, 0.3, 0.75, 1.0];
    let mut checked = 0;
    for mode in ColourBlend::ALL {
        for b in colours {
            for s in colours {
                for a_b in alphas {
                    for a_s in alphas {
                        let (want, want_a) = reference::over(mode.name(), s, a_s, b, a_b);
                        let f = |c: [f64; 3]| c.map(|v| v as f32);
                        let got = blend::over(mode, Rgba { rgb: f(s), a: a_s as f32 }, Rgba { rgb: f(b), a: a_b as f32 });
                        assert!((got.a as f64 - want_a).abs() < 1e-6, "{mode:?} alpha");
                        if want_a > 0.0 {
                            for k in 0..3 {
                                let w = want[k].clamp(0.0, 1.0);
                                assert!(
                                    (got.rgb[k] as f64 - w).abs() < 2e-6,
                                    "{mode:?}: {s:?} at {a_s} over {b:?} at {a_b}: {} against {w}",
                                    got.rgb[k]
                                );
                            }
                        }
                        checked += 1;
                    }
                }
            }
        }
    }
    assert_eq!(checked, 13 * 6 * 6 * 4 * 4);
    // A few worked by hand, so the reference itself is checked too.
    let opaque = |mode, s: [f32; 3], b: [f32; 3]| blend::over(mode, Rgba { rgb: s, a: 1.0 }, Rgba { rgb: b, a: 1.0 }).rgb;
    assert_eq!(opaque(ColourBlend::Multiply, [0.5, 1.0, 0.0], [0.5, 0.5, 0.5]), [0.25, 0.5, 0.0]);
    assert_eq!(opaque(ColourBlend::Screen, [0.5, 0.0, 1.0], [0.5, 0.5, 0.5]), [0.75, 0.5, 1.0]);
    assert_eq!(opaque(ColourBlend::Add, [0.75, 0.0, 0.5], [0.5, 0.5, 0.25]), [1.0, 0.5, 0.75]);
    assert_eq!(opaque(ColourBlend::Subtract, [0.75, 0.0, 0.5], [0.5, 0.5, 0.25]), [0.0, 0.5, 0.0]);
    // Half a layer of white Normal over opaque black is mid grey; over nothing it is white, half there.
    let half = blend::over(ColourBlend::Normal, Rgba { rgb: [1.0; 3], a: 0.5 }, Rgba { rgb: [0.0; 3], a: 1.0 });
    assert_eq!((half.rgb, half.a), ([0.5; 3], 1.0));
    let alone = blend::over(ColourBlend::Multiply, Rgba { rgb: [0.2, 0.4, 0.6], a: 0.5 }, Rgba::CLEAR);
    assert!((alone.rgb[0] - 0.2).abs() < 1e-6 && alone.a == 0.5, "a Multiply layer on bare clay is its own colour");
    // Every mode keeps a byte in a file.
    assert!(ColourBlend::ALL.iter().all(|&m| ColourBlend::from_byte(m.to_byte()) == m));
}

#[test]
fn a_vertex_made_between_others_takes_paint_by_the_rules_for_each_channel() {
    let half = [0.5, 0.5];
    // Colour by coverage: red beside nothing is half-covered red, not dark red.
    let c = Rgba8::weighted(&[Rgba8([255, 0, 0, 255]), Rgba8([0, 0, 0, 0])], &half);
    assert_eq!(c, Rgba8([255, 0, 0, 128]));
    let c = Rgba8::weighted(&[Rgba8([255, 0, 0, 255]), Rgba8([0, 0, 255, 255])], &half);
    assert_eq!(c, Rgba8([128, 0, 128, 255]));
    // Material: the heavier slot, at the mean of its weight with nothing at the other end.
    assert_eq!(Mat8::weighted(&[Mat8([3, 200]), Mat8([5, 100])], &half), Mat8([3, 100]));
    assert_eq!(Mat8::weighted(&[Mat8([3, 200]), Mat8([3, 100])], &half), Mat8([3, 150]));
    // A tie goes to the first met.
    assert_eq!(Mat8::weighted(&[Mat8([5, 100]), Mat8([3, 100])], &half), Mat8([5, 50]));
    assert_eq!(Mat8::weighted(&[Mat8([0, 0]), Mat8([0, 0])], &half), Mat8([0, 0]));
    // The rest by the plain mean.
    assert_eq!(RoughMetal8::weighted(&[RoughMetal8([10, 20, 30, 40]), RoughMetal8([20, 40, 60, 80])], &half), RoughMetal8([15, 30, 45, 60]));
    assert_eq!(Unorm8::weighted(&[Unorm8(255), Unorm8(0)], &half), Unorm8(128));
    // One end alone, whole, is that end to the bit.
    for (t, w) in [(Rgba8([7, 9, 11, 3]), [1.0, 0.0, 0.0]), (Rgba8([7, 9, 11, 0]), [1.0, 0.0, 0.0])] {
        assert_eq!(Rgba8::weighted(&[t, Rgba8([200; 4]), Rgba8([1; 4])], &w), t);
    }
}

// --------------------------------------------------------------- fixtures

/// A smooth colour over the model, so a carried value can be checked
/// against where it lands.
fn colour_at(p: Vec3) -> Rgba8 {
    let b = |x: f32| (128.0 + 200.0 * x).round().clamp(0.0, 255.0) as u8;
    Rgba8([b(p.x), b(p.y), b(p.z), 255])
}

/// A ball with two paint layers: a colour, material and roughness layer
/// over all of it, and a half-strength Multiply layer with a mask on top.
fn painted(tris: usize) -> Mesh {
    let mut mesh = sphere(tris);
    paint_on(&mut mesh);
    mesh
}

fn paint_on(mesh: &mut Mesh) {
    let n = mesh.vertex_count();
    let p = &mesh.positions;
    let base = Channels {
        colour: Some(p.iter().map(|&p| colour_at(p)).collect()),
        material: Some(p.iter().map(|p| if p.y > 0.0 { Mat8([4, 200]) } else { Mat8([9, 255]) }).collect()),
        rough_metal: Some(p.iter().map(|p| RoughMetal8([((p.x + 0.5) * 255.0) as u8, 255, 0, 0])).collect()),
        ..Channels::default()
    };
    let top = Channels {
        colour: Some(p.iter().map(|p| Rgba8([255, 64, 0, if p.z > 0.0 { 200 } else { 0 }])).collect()),
        mask: Some(p.iter().map(|p| Unorm8(if p.x > 0.0 { 255 } else { 90 })).collect()),
        ..Channels::default()
    };
    let mut over = PaintRow::new(2, "Rust".into());
    (over.colour_blend, over.opacity, over.mask) = (ColourBlend::Multiply, 180, MaskSource::Paintable);
    let stack = PaintStack {
        layers: vec![
            PaintLayer { row: PaintRow::new(1, "Base".into()), data: base },
            PaintLayer { row: over, data: top },
        ],
        active: Some(2),
        next_id: 3,
        table: vec![4, 9],
        finest: Vec::new(),
        len: n,
    };
    mesh.set_paint(Some(stack));
}

/// Every layer's texel at every vertex, by the vertex's place: the same for
/// the same paint however the mesh is numbered.
fn texels_by_place(mesh: &Mesh) -> Vec<([u32; 3], Vec<Texel>)> {
    let stack = mesh.paint_stack().expect("paint");
    let mut out: Vec<([u32; 3], Vec<Texel>)> = (0..mesh.vertex_count())
        .map(|v| (mesh.positions[v].to_array().map(f32::to_bits), stack.layers.iter().map(|l| l.data.texel(v)).collect()))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// The stack and its lists, to compare whole.
fn stack_of(mesh: &Mesh) -> Option<PaintStack> {
    mesh.paint_stack().map(std::borrow::Cow::into_owned)
}

// -------------------------------------------------------------- composite

/// The composite of every vertex, worked out from scratch.
fn full(mesh: &Mesh) -> Vec<Stream> {
    export_streams(mesh).expect("paint")
}

#[test]
fn a_mesh_without_paint_costs_nothing_and_has_no_composite() {
    let mut mesh = sphere(2_000);
    let bytes = mesh.memory_bytes();
    assert!(!mesh.has_paint() && export_streams(&mesh).is_none() && mesh.paint_stream().is_empty());
    assert!(mesh.take_dirty().paint.is_empty());
    assert_eq!(mesh.memory_bytes(), bytes);
    assert_eq!(mesh.paint_print_by_place(), 0);
}

#[test]
fn the_composite_lays_each_layer_on_the_one_under_it() {
    let mut mesh = painted(3_000);
    let dirty = mesh.take_dirty();
    assert_eq!(dirty.paint, vec![0..mesh.vertex_count() as u32], "the first composite is all of it");
    let stream = mesh.paint_stream().to_vec();
    assert_eq!(stream, full(&mesh));
    let stack = mesh.paint_stack().unwrap().into_owned();
    for v in (0..mesh.vertex_count()).step_by(37) {
        let (base, top) = (stack.layers[0].data.texel(v), stack.layers[1].data.texel(v));
        let r = stream[v];
        // The base is opaque, so the colour is the Multiply of the top over it, by the top's reach.
        let reach = 180.0 / 255.0 * top.mask.0 as f64 / 255.0 * top.colour.0[3] as f64 / 255.0;
        let b = [0, 1, 2].map(|k| base.colour.0[k] as f64 / 255.0);
        let s = [0, 1, 2].map(|k| top.colour.0[k] as f64 / 255.0);
        let (want, a) = reference::over("Multiply", s, reach, b, 1.0);
        assert_eq!(r[3], (a * 255.0).round() as u8);
        for k in 0..3 {
            assert!((r[k] as f64 - want[k] * 255.0).abs() <= 1.0, "vertex {v}: {} against {}", r[k], want[k] * 255.0);
        }
        // The material is the base's, at its weight, in its table entry.
        let (slot, w) = (base.material.0[0], base.material.0[1]);
        let k = stack.table.iter().position(|&s| s == slot).unwrap();
        assert_eq!(r[8 + k], w);
        assert_eq!(r[8 + 1 - k], 0);
        // Roughness at full weight; no metal.
        assert_eq!([r[4], r[6], r[5], r[7]], [base.rough_metal.0[0], 255, 0, 0]);
    }
}

#[test]
fn material_normal_and_add_share_out_the_weights_as_the_design_says() {
    let mut w = [0.0f32; MOST_MATERIALS];
    composite::lay_material(&mut w, 0, 0.6, MaterialBlend::Normal);
    composite::lay_material(&mut w, 1, 0.5, MaterialBlend::Normal);
    assert!((w[0] - 0.3).abs() < 1e-6 && (w[1] - 0.5).abs() < 1e-6, "{w:?}");
    // Add: the new one goes up; past all, the others give way in proportion.
    composite::lay_material(&mut w, 2, 0.4, MaterialBlend::Add);
    let total: f32 = w.iter().sum();
    assert!((total - 1.0).abs() < 1e-6 && (w[2] - 0.4).abs() < 1e-6, "{w:?}");
    assert!((w[0] / w[1] - 0.6).abs() < 1e-5, "the others keep their proportions: {w:?}");
    composite::lay_material(&mut w, 2, 0.9, MaterialBlend::Add);
    assert!(w[2] == 1.0 && w[0] == 0.0 && w[1] == 0.0, "{w:?}");
}

#[test]
fn reordering_merging_and_flattening_give_the_composite_expected() {
    let mut mesh = painted(3_000);
    let before = full(&mesh);
    // Reorder: the masked Multiply layer under the opaque base shows nothing of itself.
    let step = mesh.move_paint_layer(2, 0).unwrap();
    let under = full(&mesh);
    let stack = mesh.paint_stack().unwrap().into_owned();
    for v in 0..mesh.vertex_count() {
        let base = stack.layers[1].data.texel(v).colour.0;
        assert_eq!(&under[v][..4], &base[..], "vertex {v}: the base covers it");
    }
    let mut history = History::default();
    history.push_paint(step);
    history.undo(&mut mesh).unwrap();
    assert_eq!(full(&mesh), before);

    // Merge Down of the top into the base: the same to within a byte.
    let merged_step = mesh.merge_paint_down(2).unwrap();
    assert_eq!(mesh.paint_rows().len(), 1);
    assert_eq!(mesh.paint_rows()[0].name, "Base");
    let after = full(&mesh);
    for v in 0..mesh.vertex_count() {
        for k in 0..8 {
            assert!(after[v][k].abs_diff(before[v][k]) <= 1, "vertex {v} byte {k}: {} against {}", after[v][k], before[v][k]);
        }
        assert_eq!(&after[v][8..], &before[v][8..], "one material a vertex here, so none is lost");
    }
    history.push_paint(merged_step);
    history.undo(&mut mesh).unwrap();
    assert_eq!(full(&mesh), before);

    // Flatten All, with a third, hidden layer that is dropped.
    let (id, add) = mesh.add_paint_layer(Some("Hidden")).unwrap();
    history.push_paint(add);
    let filled = mesh
        .fill_paint(
            &PaintTarget { texel: Texel { colour: Rgba8([0, 255, 0, 255]), ..Texel::BARE }, channels: ChannelSet::COLOUR, mode: PaintMode::Paint },
            FillArea::All,
        )
        .unwrap();
    history.push_paint(filled);
    assert!(full(&mesh).iter().all(|r| r[..4] == [0, 255, 0, 255]), "an opaque green layer on top");
    let row = PaintRow { visible: false, ..mesh.paint_rows().into_iter().find(|r| r.id == id).unwrap() };
    history.push_paint(mesh.set_paint_row(row).unwrap());
    assert_eq!(full(&mesh), before, "hidden, it adds nothing");
    let flat = mesh.flatten_paint().unwrap();
    assert_eq!(mesh.paint_rows().len(), 1);
    let after = full(&mesh);
    for v in 0..mesh.vertex_count() {
        for k in 0..8 {
            assert!(after[v][k].abs_diff(before[v][k]) <= 1, "vertex {v} byte {k}");
        }
    }
    history.push_paint(flat);
    let print = stack_of(&mesh);
    history.undo(&mut mesh).unwrap();
    assert_eq!(mesh.paint_rows().len(), 3);
    history.redo(&mut mesh).unwrap();
    assert!(stack_of(&mesh) == print, "redo is the flattened stack again, exactly");
}

#[test]
fn the_composite_worked_out_leaf_by_leaf_is_the_composite_worked_out_whole() {
    let mut mesh = painted(20_000);
    let _ = mesh.take_dirty();
    let n = mesh.vertex_count();
    // A dozen strokes over different parts, the composite brought up to date after each.
    for k in 0..12u32 {
        let centre = mesh.positions[(k as usize * 7919) % n];
        let verts: Vec<u32> = (0..n as u32).filter(|&v| mesh.positions[v as usize].distance(centre) < 0.15).collect();
        let target = PaintTarget {
            texel: Texel { colour: Rgba8([(k * 20) as u8, 30, 200, 255]), material: Mat8([9, 128]), ..Texel::BARE },
            channels: ChannelSet::COLOUR.with(ChannelSet::MATERIAL),
            mode: if k % 3 == 2 { PaintMode::Erase } else { PaintMode::Paint },
        };
        mesh.begin_stroke();
        mesh.paint_dab(&verts, &target, 0.8, |_, v| 0.4 + (v % 3) as f32 * 0.2).unwrap();
        let delta = mesh.end_stroke(false);
        // An eraser over bare paint changes nothing, and records nothing.
        assert!(matches!(delta, Some(StrokeDelta::Paint(_))) || (delta.is_none() && k % 3 == 2), "stroke {k}");
        let dirty = mesh.take_dirty();
        assert!(dirty.paint.iter().map(|r| r.len()).sum::<usize>() < n, "only what changed");
    }
    // The sculpt mask changed under a layer masked by it, and a group under one masked by a group.
    let mut row = mesh.paint_rows()[1].clone();
    row.mask = MaskSource::SculptMask { invert: true };
    mesh.set_paint_row(row).unwrap();
    let _ = mesh.take_dirty();
    let delta = mesh.remap_mask(|m, v, _| if m.positions[v as usize].x > 0.2 { 1.0 } else { 0.0 });
    assert!(delta.is_some());
    let _ = mesh.take_dirty();
    assert_eq!(mesh.paint_stream(), &full(&mesh)[..]);
    let mut row = mesh.paint_rows()[0].clone();
    row.mask = MaskSource::FaceGroup(3);
    mesh.set_paint_row(row).unwrap();
    let _ = mesh.take_dirty();
    let ids = (0..mesh.triangle_count()).map(|t| if t % 3 == 0 { 3 } else { 1 }).collect();
    mesh.set_group_ids(ids);
    let _ = mesh.take_dirty();
    assert_eq!(mesh.paint_stream(), &full(&mesh)[..]);
}

// ----------------------------------------------------------------- strokes

fn red() -> PaintTarget {
    PaintTarget { texel: Texel { colour: Rgba8([255, 0, 0, 255]), ..Texel::BARE }, channels: ChannelSet::COLOUR, mode: PaintMode::Paint }
}

fn fresh_layer(mesh: &mut Mesh) -> u32 {
    mesh.add_paint_layer(None).unwrap().0
}

#[test]
fn opacity_is_a_ceiling_and_a_hundred_light_dabs_land_where_one_heavy_one_does() {
    let mut mesh = sphere(2_000);
    let layer = fresh_layer(&mut mesh);
    let verts: Vec<u32> = (0..mesh.vertex_count() as u32).collect();
    mesh.begin_stroke();
    for _ in 0..100 {
        mesh.paint_begin_dab();
        mesh.paint_dab(&verts, &red(), 0.4, |_, _| 0.07).unwrap();
    }
    let delta = mesh.end_stroke(false).unwrap();
    let light: Vec<Texel> = (0..mesh.vertex_count()).map(|v| mesh.paint_stack().unwrap().layer(layer).unwrap().data.texel(v)).collect();
    assert!(light.iter().all(|t| t.colour == Rgba8([255, 0, 0, 102])), "40% and no more: {:?}", light[0]);

    // The same as one dab that went all the way at once, byte for byte.
    let mut history = History::default();
    history.push_stroke(delta);
    history.undo(&mut mesh).unwrap();
    mesh.begin_stroke();
    mesh.paint_dab(&verts, &red(), 0.4, |_, _| 1.0).unwrap();
    mesh.end_stroke(false).unwrap();
    let heavy: Vec<Texel> = (0..mesh.vertex_count()).map(|v| mesh.paint_stack().unwrap().layer(layer).unwrap().data.texel(v)).collect();
    assert_eq!(light, heavy);

    // Over paint already there the stroke goes from what was there, not from the last dab's bytes:
    // once at its ceiling, dab after dab leaves the texel where it is.
    let blue = PaintTarget { texel: Texel { colour: Rgba8([0, 0, 255, 255]), ..Texel::BARE }, ..red() };
    mesh.begin_stroke();
    mesh.paint_dab(&verts, &blue, 0.5, |_, _| 1.0).unwrap();
    for _ in 0..50 {
        mesh.paint_begin_dab();
        mesh.paint_dab(&verts, &blue, 0.5, |_, _| 0.3).unwrap();
    }
    mesh.end_stroke(false).unwrap();
    let start = Texel { colour: Rgba8([255, 0, 0, 102]), ..Texel::BARE };
    let want = blue.applied(start, 0.5);
    let now = mesh.paint_stack().unwrap().layer(layer).unwrap().data.texel(9);
    assert_eq!(now, want);
}

#[test]
fn the_same_vertex_written_by_two_mirror_copies_of_a_dab_gets_what_one_gives() {
    let mut once = sphere(2_000);
    let mut twice = sphere(2_000);
    let mut swapped = sphere(2_000);
    for mesh in [&mut once, &mut twice, &mut swapped] {
        fresh_layer(mesh);
        mesh.begin_stroke();
    }
    let verts: Vec<u32> = (0..once.vertex_count() as u32).collect();
    let strength = |_: usize, v: u32| (v % 7) as f32 / 7.0;
    let other = |_: usize, v: u32| (v % 5) as f32 / 5.0;
    for _ in 0..6 {
        once.paint_begin_dab();
        once.paint_dab(&verts, &red(), 0.8, |k, v| strength(k, v).max(other(k, v))).unwrap();
        // Two copies of the dab reaching the same vertices, in either order.
        twice.paint_begin_dab();
        twice.paint_dab(&verts, &red(), 0.8, strength).unwrap();
        twice.paint_dab(&verts, &red(), 0.8, other).unwrap();
        swapped.paint_begin_dab();
        swapped.paint_dab(&verts, &red(), 0.8, other).unwrap();
        swapped.paint_dab(&verts, &red(), 0.8, strength).unwrap();
    }
    let all = |mesh: &mut Mesh| {
        mesh.end_stroke(false);
        stack_of(mesh)
    };
    let (a, b, c) = (all(&mut once), all(&mut twice), all(&mut swapped));
    assert!(a == b && b == c, "the overlap is painted once, whatever order the copies come in");
}

#[test]
fn paint_passes_over_hidden_faces_other_groups_and_locked_layers() {
    let mut mesh = sphere(4_000);
    let layer = fresh_layer(&mut mesh);
    let low: Vec<u32> = (0..mesh.triangle_count() as u32)
        .filter(|&t| mesh.positions[mesh.indices[t as usize * 3] as usize].y < -0.2)
        .collect();
    let _ = mesh.hide(&low);
    let verts: Vec<u32> = (0..mesh.vertex_count() as u32).collect();
    mesh.begin_stroke();
    mesh.paint_dab(&verts, &red(), 1.0, |_, _| 1.0).unwrap();
    mesh.end_stroke(false).unwrap();
    let stack = mesh.paint_stack().unwrap().into_owned();
    let hidden = mesh.hidden_vertices().unwrap().to_vec();
    for v in 0..mesh.vertex_count() {
        let painted = stack.layer(layer).unwrap().data.texel(v).colour.0[3] > 0;
        assert_eq!(painted, !hidden[v], "vertex {v}");
    }
    let _ = mesh.show_all();

    // Stay in Group: only the group the stroke is kept to.
    let mut mesh = sphere(4_000);
    let ids = (0..mesh.triangle_count() as u32)
        .map(|t| if mesh.positions[mesh.indices[t as usize * 3] as usize].x > 0.0 { 1 } else { 2 })
        .collect();
    mesh.set_group_ids(ids);
    let layer = fresh_layer(&mut mesh);
    mesh.stay_in_groups(&[1]);
    mesh.begin_stroke();
    mesh.paint_dab(&verts, &red(), 1.0, |_, _| 1.0).unwrap();
    mesh.end_stroke(false).unwrap();
    let on = mesh.group_vertices(1);
    let stack = mesh.paint_stack().unwrap().into_owned();
    assert!((0..mesh.vertex_count()).all(|v| (stack.layer(layer).unwrap().data.texel(v).colour.0[3] > 0) == on[v]));

    // Locked, and outside a stroke: refused, with a reason.
    let mut row = mesh.paint_rows()[0].clone();
    row.locked = true;
    mesh.set_paint_row(row).unwrap();
    mesh.begin_stroke();
    assert!(matches!(mesh.paint_dab(&verts, &red(), 1.0, |_, _| 1.0), Err(PaintRefusal::Locked(_))));
    mesh.end_stroke(false);
    assert_eq!(mesh.paint_dab(&verts, &red(), 1.0, |_, _| 1.0), Err(PaintRefusal::NotInStroke));
    assert!(PaintRefusal::Locked("Paint 1".into()).to_string().contains("locked"));
    let mut bare = sphere(500);
    bare.begin_stroke();
    assert_eq!(bare.paint_dab(&verts[..10], &red(), 1.0, |_, _| 1.0), Err(PaintRefusal::NoLayer));
    bare.end_stroke(false);
}

#[test]
fn a_paint_stroke_is_one_undo_step_and_puts_back_exactly_what_was_there() {
    let mut mesh = painted(3_000);
    let before = stack_of(&mesh);
    let verts: Vec<u32> = (0..mesh.vertex_count() as u32).filter(|v| v % 3 == 0).collect();
    let target = PaintTarget {
        texel: Texel { colour: Rgba8([10, 200, 30, 255]), material: Mat8([4, 255]), rough_metal: RoughMetal8([0, 0, 255, 255]), mask: Unorm8(0) },
        channels: ChannelSet::ALL,
        mode: PaintMode::Paint,
    };
    mesh.begin_stroke();
    mesh.paint_dab(&verts, &target, 0.7, |_, _| 1.0).unwrap();
    let delta = mesh.end_stroke(false).unwrap();
    let after = stack_of(&mesh);
    assert!(after != before);
    let mut history = History::default();
    history.push_stroke(delta);
    history.undo(&mut mesh).unwrap();
    assert!(stack_of(&mesh) == before);
    history.redo(&mut mesh).unwrap();
    assert!(stack_of(&mesh) == after);
}

#[test]
fn every_layer_command_is_one_undo_step_that_swaps_back_and_forth_exactly() {
    let mut mesh = painted(2_000);
    let mut history = History::default();
    type Command = Box<dyn Fn(&mut Mesh) -> Result<PaintStep, PaintRefusal>>;
    let green = PaintTarget { texel: Texel { colour: Rgba8([0, 255, 0, 255]), ..Texel::BARE }, channels: ChannelSet::COLOUR, mode: PaintMode::Paint };
    let commands: Vec<(&str, Command)> = vec![
        ("add", Box::new(|m| m.add_paint_layer(Some("New")).map(|(_, s)| s))),
        ("rename", Box::new(|m| m.rename_paint_layer(1, "Bottom"))),
        ("move", Box::new(|m| m.move_paint_layer(1, 2))),
        ("duplicate", Box::new(|m| m.duplicate_paint_layer(2).map(|(_, s)| s))),
        ("active", Box::new(|m| m.set_paint_active(1))),
        ("fill masked", Box::new(move |m| m.fill_paint(&green, FillArea::SculptMask { invert: true }))),
        ("clear", Box::new(|m| m.clear_paint(2, ChannelSet::COLOUR))),
        ("cavity", Box::new(|m| m.snapshot_paint_cavity(2))),
        ("material", Box::new(|m| m.add_paint_material(17).map(|(_, s)| s))),
        ("tidy", Box::new(|m| m.tidy_paint_materials())),
        ("merge", Box::new(|m| m.merge_paint_down(m.paint_rows()[1].id))),
        ("delete", Box::new(|m| m.delete_paint_layer(m.paint_rows()[0].id))),
        ("flatten", Box::new(|m| m.flatten_paint())),
    ];
    for v in 0..mesh.vertex_count() {
        mesh.mask[v] = if mesh.positions[v].x > 0.1 { 1.0 } else { 0.0 };
    }
    let mut states = vec![stack_of(&mesh)];
    for (what, command) in &commands {
        let step = command(&mut mesh).unwrap_or_else(|why| panic!("{what}: {why}"));
        history.push_paint(step);
        let now = stack_of(&mesh);
        assert!(now != *states.last().unwrap(), "{what} changed something");
        states.push(now);
    }
    for k in (0..commands.len()).rev() {
        history.undo(&mut mesh).unwrap();
        assert!(stack_of(&mesh) == states[k], "undo of {}", commands[k].0);
    }
    for k in 0..commands.len() {
        history.redo(&mut mesh).unwrap();
        assert!(stack_of(&mesh) == states[k + 1], "redo of {}", commands[k].0);
    }
    // Deleting the last layer leaves no paint at all; undo brings it back.
    while let Some(row) = mesh.paint_rows().first().cloned() {
        history.push_paint(mesh.delete_paint_layer(row.id).unwrap());
    }
    assert!(!mesh.has_paint());
    history.undo(&mut mesh).unwrap();
    assert!(mesh.has_paint());
    // A full table refuses a ninth material.
    for slot in 20..40 {
        if let Err(why) = mesh.add_paint_material(slot) {
            assert_eq!(why, PaintRefusal::TableFull);
            assert_eq!(mesh.paint_table().len(), MOST_MATERIALS);
            return;
        }
    }
    panic!("the table took more than {MOST_MATERIALS} materials");
}

// --------------------------------------------------------- dynamic topology

#[test]
fn paint_survives_a_refining_stroke_and_its_undo_and_redo() {
    let mut mesh = painted(6_000);
    let n0 = mesh.vertex_count();
    let before = stack_of(&mesh).unwrap();
    let mut history = History::default();
    mesh.begin_stroke();
    let stats = mesh.refine(&[Vec3::new(0.0, 0.0, 0.5)], 0.2, 0.008, usize::MAX, 10_000_000);
    let delta = mesh.end_stroke(false).expect("the refine is a stroke");
    let n1 = mesh.vertex_count();
    assert!(n1 > n0 + 200, "{stats:?}");
    // Read: as long as the mesh, the old vertices to the bit, the new ones from their neighbours.
    let grown = stack_of(&mesh).unwrap();
    assert_eq!(grown.len, n1);
    for (was, now) in before.layers.iter().zip(&grown.layers) {
        assert_eq!(now.data.colour.as_ref().unwrap().len(), n1);
        assert!(was.data.colour.as_ref().unwrap()[..] == now.data.colour.as_ref().unwrap()[..n0]);
        assert!(was.data.mask.as_ref().map(|l| &l[..]) == now.data.mask.as_ref().map(|l| &l[..n0]));
    }
    // A new vertex's colour is near the colour of where it is.
    for v in n0..n1 {
        let (got, want) = (grown.layers[0].data.texel(v).colour.0, colour_at(mesh.positions[v]).0);
        assert!((0..3).all(|k| got[k].abs_diff(want[k]) <= 10), "vertex {v}: {got:?} against {want:?}");
    }
    assert!(!mesh.take_dirty().paint.is_empty());
    assert_eq!(mesh.paint_stream().len(), n1);
    assert_eq!(mesh.paint_stream(), &full(&mesh)[..]);

    // A paint stroke over the new vertices, then undo of both and redo of both.
    let verts: Vec<u32> = (0..n1 as u32).filter(|&v| mesh.positions[v as usize].z > 0.4).collect();
    history.push_stroke(delta);
    mesh.begin_stroke();
    mesh.paint_dab(&verts, &red(), 1.0, |_, _| 1.0).unwrap();
    history.push_stroke(mesh.end_stroke(false).unwrap());
    let painted_now = stack_of(&mesh).unwrap();
    history.undo(&mut mesh).unwrap();
    assert!(stack_of(&mesh).unwrap() == grown, "the paint stroke undone");
    history.undo(&mut mesh).unwrap();
    assert_eq!(mesh.vertex_count(), n0);
    assert!(stack_of(&mesh).unwrap() == before, "the refine undone: the paint as it was, to the bit");
    let _ = mesh.take_dirty();
    assert_eq!(mesh.paint_stream(), &full(&mesh)[..]);
    history.redo(&mut mesh).unwrap();
    history.redo(&mut mesh).unwrap();
    assert_eq!(mesh.vertex_count(), n1);
    let again = stack_of(&mesh).unwrap();
    assert!(again == painted_now, "redo: the new vertices given their paint again, and the stroke on them");
    let _ = mesh.take_dirty();
    assert_eq!(mesh.paint_stream(), &full(&mesh)[..]);
}

// --------------------------------------------------------- subdivision levels

#[test]
fn paint_goes_up_and_down_the_levels_and_a_round_trip_changes_nothing() {
    let mesh = painted(2_000);
    let (fine, _) = subdivided(&mesh, Split::Smooth).unwrap();
    let up = stack_of(&fine).unwrap();
    assert_eq!(up.len, fine.vertex_count());
    // Each vertex of the finer level has paint near the colour of where it is.
    for v in 0..fine.vertex_count() {
        let (got, want) = (up.layers[0].data.texel(v).colour.0, colour_at(fine.positions[v]).0);
        assert!((0..3).all(|k| got[k].abs_diff(want[k]) <= 6), "vertex {v}: {got:?} against {want:?}");
    }
    // Down and up again with nothing changed: to the bit.
    let (coarse, _) = stepped(&fine, 0).unwrap();
    // (Smooth levels may settle the coarse points a rounding apart, so the texels are compared as a set.)
    let texels = |m: &Mesh| {
        let mut all: Vec<Vec<Texel>> = texels_by_place(m).into_iter().map(|(_, t)| t).collect();
        all.sort_by_key(|t| format!("{t:?}"));
        all
    };
    assert_eq!(texels(&coarse), texels(&mesh), "the coarse level is the mesh it was made from");
    let (back, _) = stepped(&coarse, 1).unwrap();
    assert_eq!(texels_by_place(&back), texels_by_place(&fine));

    // Paint on the coarse level: carried up, and the rest of the finest paint is as it was.
    let mut coarse = coarse;
    let id = coarse.paint_rows()[0].id;
    coarse.set_paint_active(id).unwrap();
    let verts: Vec<u32> = (0..coarse.vertex_count() as u32).filter(|&v| coarse.positions[v as usize].y > 0.35).collect();
    coarse.begin_stroke();
    coarse.paint_dab(&verts, &red(), 1.0, |_, _| 1.0).unwrap();
    coarse.end_stroke(false).unwrap();
    let (back, _) = stepped(&coarse, 1).unwrap();
    let was: std::collections::HashMap<[u32; 3], Vec<Texel>> = texels_by_place(&fine).into_iter().collect();
    let (mut same, mut red_now) = (0, 0);
    for (place, texels) in texels_by_place(&back) {
        let y = f32::from_bits(place[1]);
        let old = &was[&place];
        if y < 0.2 {
            assert_eq!(&texels, old, "far from the stroke the finest paint is as it was");
            same += 1;
        } else if y > 0.4 {
            let c = texels[0].colour.0;
            assert!(c[0] >= 249 && c[1] <= 6 && c[2] <= 6 && c[3] == 255, "under the stroke it is the stroke's colour: {c:?}");
            red_now += 1;
        }
    }
    assert!(same > 1000 && red_now > 50, "{same} {red_now}");
    // And a layer made on the coarse level is carried up from what it holds there.
    let (layer, _) = coarse.add_paint_layer(Some("Coarse")).unwrap();
    let (back, _) = stepped(&coarse, 1).unwrap();
    assert!(back.paint_rows().iter().any(|r| r.id == layer));
    assert_eq!(back.paint_stack().unwrap().len, back.vertex_count());
}

// ---------------------------------------------------- carried onto new meshes

/// How far, on average and at worst, a mesh's base colour is from the
/// colour of where each vertex is (in levels of 255).
fn colour_error(mesh: &Mesh) -> (f32, u8) {
    let stack = mesh.paint_stack().expect("paint");
    let (mut sum, mut worst) = (0.0, 0u8);
    for v in 0..mesh.vertex_count() {
        let (got, want) = (stack.layers[0].data.texel(v).colour.0, colour_at(mesh.positions[v]).0);
        for k in 0..3 {
            let d = got[k].abs_diff(want[k]);
            sum += d as f32;
            worst = worst.max(d);
        }
    }
    (sum / (3 * mesh.vertex_count()) as f32, worst)
}

#[test]
fn a_rebuilt_mesh_takes_its_paint_from_the_nearest_old_point() {
    let old = painted(20_000);
    let turn = glam::Quat::from_rotation_y(0.4) * glam::Quat::from_rotation_x(0.3);
    let other = sphere(9_000);
    let mut new = Mesh::new(other.positions.iter().map(|&p| turn * p).collect(), other.indices.clone(), None);
    assert_eq!(transfer::carry(&mut new, &[Source::of(&old)], Fit::Whole), transfer::Carried::Kept);
    let (mean, worst) = colour_error(&new);
    assert!(mean < 1.0 && worst <= 4, "{mean} {worst}");
    assert_eq!(new.paint_rows(), old.paint_rows());
    assert_eq!(new.paint_table(), old.paint_table());
    // Materials by side, away from the border.
    let stack = new.paint_stack().unwrap();
    for v in 0..new.vertex_count() {
        let y = new.positions[v].y;
        if y.abs() > 0.06 {
            assert_eq!(stack.layers[0].data.texel(v).material.0[0], if y > 0.0 { 4 } else { 9 }, "vertex {v}");
        }
    }
    // Somewhere else altogether: dropped. Nothing painted: nothing.
    let mut far = Mesh::new(other.positions.iter().map(|&p| p * 0.2 + Vec3::X * 3.0).collect(), other.indices.clone(), None);
    assert_eq!(transfer::carry(&mut far, &[Source::of(&old)], Fit::Whole), transfer::Carried::Dropped);
    assert!(!far.has_paint());
    let mut plain = sphere(3_000);
    assert_eq!(transfer::carry(&mut plain, &[Source::of(&sphere(2_000))], Fit::Whole), transfer::Carried::Nothing);
}

#[test]
fn copies_of_an_array_made_real_and_a_renumbered_copy_carry_each_vertex_exactly() {
    let old = painted(3_000);
    let places: [&dyn Fn(Vec3) -> Vec3; 3] = [&|p| p, &|p| p * 1.5 + Vec3::X * 3.0, &|p| Vec3::new(-p.x, p.y + 4.0, p.z)];
    let (mut positions, mut indices) = (Vec::new(), Vec::new());
    for place in places {
        let base = positions.len() as u32;
        positions.extend(old.positions.iter().map(|&p| place(p)));
        indices.extend(old.indices.iter().map(|&i| i + base));
    }
    let (mut made, numbering) = Mesh::new_numbered(positions, indices, None, &[]);
    assert!(transfer::copies(&old, &mut made, &numbering.vertex));
    let (o, m) = (old.paint_stack().unwrap(), made.paint_stack().unwrap());
    for c in 0..3 {
        for j in 0..old.vertex_count() {
            let at = numbering.vertex[c * old.vertex_count() + j] as usize;
            for (a, b) in o.layers.iter().zip(&m.layers) {
                assert_eq!(a.data.texel(j), b.data.texel(at));
            }
        }
    }
    // Not copies: the first one moved.
    let mut moved = Mesh::new(made.positions.iter().map(|&p| p + Vec3::splat(0.01)).collect(), made.indices.clone(), None);
    let all: Vec<u32> = (0..moved.vertex_count() as u32).collect();
    assert!(!transfer::copies(&old, &mut moved, &all));
    // A copy renumbered: every vertex keeps its texels.
    let indices: Vec<u32> = old.indices.chunks_exact(3).rev().flatten().copied().collect();
    let (mut copy, numbering) = Mesh::new_numbered(old.positions.clone(), indices, None, &[]);
    copy.paint = old.paint_renumbered(&numbering.vertex, copy.vertex_count());
    assert_eq!(texels_by_place(&copy), texels_by_place(&old));
    assert_eq!(copy.paint_print_by_place(), old.paint_print_by_place());
}

#[test]
fn shells_joined_keep_their_paint_and_layers_are_matched_by_name() {
    let a = painted(2_000);
    let mut b = sphere(1_000);
    paint_on(&mut b);
    // The second's top layer is renamed: it does not match, and goes on top.
    b.rename_paint_layer(2, "Gold").unwrap();
    let c = sphere(800);
    let shells: Vec<sculpt_geom::TriMesh> =
        [&a, &b, &c].iter().map(|m| sculpt_geom::TriMesh::new(m.positions.clone(), m.indices.clone())).collect();
    let parts: Vec<(&sculpt_geom::TriMesh, glam::Mat4)> = shells.iter().map(|s| (s, glam::Mat4::IDENTITY)).collect();
    let joined = sculpt_geom::join::join(&parts);
    let (mut mesh, vertex) = Mesh::new_mapped(joined.mesh.positions, joined.mesh.indices, None);
    let count = mesh.vertex_count();
    mesh.set_paint(transfer::joined(&[&a, &b, &c], &vertex, count));
    let names: Vec<String> = mesh.paint_rows().into_iter().map(|r| r.name).collect();
    assert_eq!(names, ["Base", "Rust", "Gold"]);
    let stack = mesh.paint_stack().unwrap();
    let (sa, sb) = (a.paint_stack().unwrap(), b.paint_stack().unwrap());
    let (na, nb) = (a.vertex_count(), b.vertex_count());
    for j in 0..na {
        let at = vertex[j] as usize;
        assert_eq!(stack.layers[0].data.texel(at), sa.layers[0].data.texel(j));
        assert_eq!(stack.layers[1].data.texel(at), sa.layers[1].data.texel(j));
        assert_eq!(stack.layers[2].data.texel(at), Texel::BARE);
    }
    for j in 0..nb {
        let at = vertex[na + j] as usize;
        assert_eq!(stack.layers[0].data.texel(at), sb.layers[0].data.texel(j), "Base matched Base");
        assert_eq!(stack.layers[2].data.texel(at), sb.layers[1].data.texel(j), "Gold went on top");
    }
    for j in 0..c.vertex_count() {
        assert!(stack.layers.iter().all(|l| l.data.texel(vertex[na + nb + j] as usize) == Texel::BARE));
    }
}

// --------------------------------------------- the nearest-point search refactor

/// `groups::transfer::nearest_groups` as it was before the search was shared
/// with paint, word for word: the groups it gives must not change.
fn groups_as_before(new: &Mesh, sources: &[&Source], fit: Fit) -> Vec<u16> {
    use crate::mesh::bvh::nearest::closest_on_triangle;
    use glam::Affine3A;
    use rayon::prelude::*;
    let on_surface = match fit {
        Fit::Whole => 0.02,
        Fit::Part => 0.002,
    };
    let placed = |s: &Source, p: Vec3| {
        let mut q = s.place.transform_point3(p);
        if let Some((axis, positive)) = s.fold {
            if (q[axis] > 0.0) != positive {
                q[axis] = -q[axis];
            }
        }
        q
    };
    let nearest_on = |s: &Source, t: u32, q: Vec3| {
        let (p, i) = (&s.mesh.positions, t as usize * 3);
        let [a, b, c] = [0, 1, 2].map(|k| p[s.mesh.indices[i + k] as usize]);
        closest_on_triangle(q, a, b, c)
    };
    let within = (new.bounds().size().length() * on_surface).powi(2);
    let back: Vec<Affine3A> = sources.iter().map(|s| s.place.inverse()).collect();
    let p = &new.positions;
    let runs: Vec<Vec<u16>> = new
        .indices
        .par_chunks(512 * 3)
        .map(|run| {
            let mut last: Vec<Option<u32>> = vec![None; sources.len()];
            let mut ids = Vec::new();
            for t in run.chunks_exact(3) {
                let centre = (p[t[0] as usize] + p[t[1] as usize] + p[t[2] as usize]) / 3.0;
                let mut best = (f32::INFINITY, 0u16);
                for (k, source) in sources.iter().enumerate() {
                    let q = placed(source, centre);
                    let reach = last[k].map_or(f32::INFINITY, |t| nearest_on(source, t, q).distance_squared(q) * 1.001 + 1e-12);
                    let mesh = source.mesh;
                    let Some((found, _)) = mesh.bvh.nearest_triangle(&mesh.positions, &mesh.indices, q, reach, |_| false) else {
                        continue;
                    };
                    last[k] = Some(found);
                    let (mut on, mut here) = (nearest_on(source, found, q), q);
                    if source.place != Affine3A::IDENTITY {
                        (on, here) = (back[k].transform_point3(on), back[k].transform_point3(here));
                    }
                    let apart = on.distance_squared(here);
                    if apart < best.0 {
                        best = (apart, source.ids.get(found as usize).copied().unwrap_or(0));
                    }
                }
                ids.push(if best.0 <= within || fit == Fit::Whole { best.1 } else { 0 });
            }
            ids
        })
        .collect();
    runs.into_iter().flatten().collect()
}

#[test]
fn face_groups_carried_through_the_shared_search_are_exactly_what_they_were() {
    let mut a = sphere(8_000);
    let ids = (0..a.triangle_count()).map(|t| (t % 5) as u16).collect();
    a.set_group_ids(ids);
    let mut b = sphere(3_000);
    let ids = (0..b.triangle_count()).map(|t| 10 + (t % 3) as u16).collect();
    b.set_group_ids(ids);
    let turn = glam::Quat::from_rotation_y(0.3);
    let other = sphere(12_000);
    let new = Mesh::new(other.positions.iter().map(|&p| turn * p * 1.01).collect(), other.indices.clone(), None);
    let placed = glam::Affine3A::from_translation(Vec3::new(0.3, 0.0, 0.0)) * glam::Affine3A::from_scale(Vec3::splat(1.3));
    let source_sets: Vec<Vec<Source>> = vec![
        vec![Source::of(&a)],
        vec![Source { place: placed, ..Source::of(&b) }, Source::of(&a)],
        vec![Source { fold: Some((0, true)), ..Source::of(&a) }],
    ];
    for (k, sources) in source_sets.iter().enumerate() {
        for fit in [Fit::Whole, Fit::Part] {
            let refs: Vec<&Source> = sources.iter().collect();
            // One mesh: building the same triangles twice need not number them the same.
            let mut carried = Mesh::new(new.positions.clone(), new.indices.clone(), None);
            let mut want = groups_as_before(&carried, &refs, fit);
            if fit == Fit::Whole {
                for _ in 0..2 {
                    groups::tidy_specks(&carried, &mut want);
                }
            }
            groups::carry(&mut carried, sources, fit);
            let got = carried.group_ids().to_vec();
            carried.set_group_ids(want);
            assert_eq!(carried.group_ids(), &got[..], "source set {k}, {fit:?}");
        }
    }
}

#[test]
fn the_fingerprint_of_paint_follows_the_paint_and_not_the_numbering() {
    let mesh = painted(2_000);
    let print = mesh.paint_print_by_place();
    assert_ne!(print, 0);
    let mut other = painted(2_000);
    assert_eq!(other.paint_print_by_place(), print);
    other.begin_stroke();
    other.paint_dab(&[5], &red(), 1.0, |_, _| 1.0).unwrap();
    other.end_stroke(false);
    assert_ne!(other.paint_print_by_place(), print);
    assert!(mesh.paint_sound().is_ok());
    // The recovery copy's fingerprint tells a painted scene from the same scene unpainted.
    let plain = sphere(2_000);
    let print_of = |m: &Mesh| crate::recovery::fingerprint(&[m], &[]);
    assert_ne!(print_of(&mesh), print_of(&plain));
    assert_eq!(print_of(&painted(2_000)), print_of(&mesh));
    assert!(crate::recovery::sound(&[&mesh, &other]).is_ok());
}
