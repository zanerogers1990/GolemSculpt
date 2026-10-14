//! The Paint brush, driven through the pointer like every other stroke test:
//! what each mode does, that nothing moves (but with Zadd), that a stroke is
//! one undo step that goes back and forth exactly, that mirrored strokes
//! are mirror images to the bit, that the sculpt mask and hidden faces keep
//! paint off, and that dynamic topology never refines for it.

use glam::{Vec2, Vec3};

use super::super::stroke_tests::twins;
use super::*;
use crate::camera::{Camera, View};
use crate::mesh::paint::{FillArea, PaintRow, PaintStack};
use crate::mesh::primitives::{self, Primitive};
use crate::mesh::StrokeDelta;
use crate::sculpt::{PointerSample, Sculptor};

const SIZE: Vec2 = Vec2::new(1600.0, 900.0);

fn sphere(tris: usize) -> Mesh {
    primitives::build(Primitive::Sphere, tris, 0.5)
}

/// A ball with one empty paint layer, active.
fn painted_ball(tris: usize) -> (Mesh, u32) {
    let mut mesh = sphere(tris);
    let (id, _) = mesh.add_paint_layer(None).expect("a layer");
    (mesh, id)
}

fn view_of(mesh: &Mesh) -> View {
    let mut camera = Camera::default();
    camera.frame(mesh.bounds(), SIZE.x / SIZE.y, false);
    camera.view(Vec2::ZERO, SIZE, camera.distance, 1.0)
}

fn line(from: Vec2, to: Vec2, n: usize) -> Vec<Vec2> {
    (0..=n).map(|i| from.lerp(to, i as f32 / n as f32)).collect()
}

/// A stroke across the front of the ball, from one side of the X plane
/// over it and along it: every way mirrored copies can meet.
fn path() -> Vec<Vec2> {
    let c = SIZE * 0.5;
    let mut points = line(c + Vec2::new(-110.0, 60.0), c + Vec2::new(6.0, 10.0), 40);
    points.extend(line(c + Vec2::new(6.0, 10.0), c + Vec2::new(14.0, -90.0), 40));
    points
}

fn stroke(mesh: &mut Mesh, sculptor: &mut Sculptor, view: &View, points: &[Vec2], invert: bool, shift: bool) -> Option<StrokeDelta> {
    let sample = |i: usize| PointerSample { pos: points[i], pressure: None, time: i as f64 / 240.0 };
    assert!(sculptor.begin(mesh, view, sample(0), invert, shift), "the stroke missed the mesh");
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

/// A sculptor holding the Paint brush at a fixed size, in `mode`.
fn painter(mode: PaintMode) -> Sculptor {
    let mut sculptor = Sculptor::default();
    let s = &mut sculptor.settings;
    s.brush = Brush::Paint;
    s.screen_space_size = false;
    s.radius_world = 0.08;
    s.paint.mode = mode;
    s.paint.colour = [220, 40, 30];
    sculptor
}

fn stack(mesh: &Mesh) -> PaintStack {
    mesh.paint_stack().expect("paint").into_owned()
}

fn texel(mesh: &Mesh, layer: u32, v: usize) -> Texel {
    mesh.paint_stack().unwrap().layer(layer).unwrap().data.texel(v)
}

fn covered(mesh: &Mesh, layer: u32) -> usize {
    (0..mesh.vertex_count()).filter(|&v| texel(mesh, layer, v).colour.0[3] > 0).count()
}

fn most_cover(mesh: &Mesh, layer: u32) -> u8 {
    (0..mesh.vertex_count()).map(|v| texel(mesh, layer, v).colour.0[3]).max().unwrap_or(0)
}

#[test]
fn a_paint_stroke_moves_nothing_and_undoes_and_redoes_exactly() {
    let (mut mesh, layer) = painted_ball(60_000);
    let view = view_of(&mesh);
    let (positions, before) = (mesh.positions.clone(), stack(&mesh));
    let mut sculptor = painter(PaintMode::Paint);
    let delta = stroke(&mut mesh, &mut sculptor, &view, &path(), false, false).expect("an undo step");
    assert!(matches!(delta, StrokeDelta::Paint(_)), "a paint stroke is a paint step alone");
    assert!(mesh.positions == positions, "painting moved the surface");
    assert!(covered(&mesh, layer) > 500, "the stroke painted {} vertices", covered(&mesh, layer));
    assert_eq!(most_cover(&mesh, layer), 255, "at full Flow and Opacity the middle of the stroke is the colour");
    let full = (0..mesh.vertex_count()).find(|&v| texel(&mesh, layer, v).colour.0[3] == 255).unwrap();
    assert_eq!(texel(&mesh, layer, full).colour.0, [220, 40, 30, 255]);
    let after = stack(&mesh);
    mesh.apply_delta(&delta, true);
    assert!(stack(&mesh) == before, "undo did not put the paint back as it was, list for list");
    mesh.apply_delta(&delta, false);
    assert!(stack(&mesh) == after, "redo did not lay the stroke again exactly");
    assert!(mesh.positions == positions);
}

#[test]
fn a_mirrored_stroke_is_its_own_mirror_image_in_every_mode() {
    let (mut mesh, layer) = painted_ball(80_000);
    let twins = twins(&mesh);
    let view = view_of(&mesh);
    let positions = mesh.positions.clone();
    for mode in [PaintMode::Paint, PaintMode::Blur, PaintMode::Smear, PaintMode::Erase] {
        let mut sculptor = painter(mode);
        sculptor.settings.symmetry = [true, false, false];
        sculptor.settings.paint.opacity = 0.8;
        sculptor.settings.per_brush[Brush::Paint.index()].strength = 0.6;
        let points: Vec<Vec2> = if mode == PaintMode::Paint { path() } else { path().into_iter().rev().collect() };
        stroke(&mut mesh, &mut sculptor, &view, &points, false, false);
        let mut worst = 0u8;
        for v in 0..mesh.vertex_count() {
            let (a, b) = (texel(&mesh, layer, v), texel(&mesh, layer, twins[v] as usize));
            let apart = a.colour.0.iter().zip(b.colour.0).map(|(x, y)| x.abs_diff(y)).max().unwrap();
            worst = worst.max(apart);
        }
        // Paint, Erase and Blur are mirror images to the bit. Smear's carried paint is a mean taken over each
        // copy's own footprint, whose vertices come in another order: a byte at most.
        let allowed = if mode == PaintMode::Smear { 1 } else { 0 };
        assert!(worst <= allowed, "{mode:?}: a vertex and its mirror image are {worst} apart");
        assert!(mesh.positions == positions, "{mode:?} moved the surface");
    }
    assert!(covered(&mesh, layer) > 500);
}

#[test]
fn erase_takes_the_layers_paint_away_and_ctrl_erases_too() {
    for ctrl in [false, true] {
        let (mut mesh, layer) = painted_ball(40_000);
        let view = view_of(&mesh);
        mesh.fill_paint(
            &PaintTarget { texel: painter(PaintMode::Paint).settings.paint.texel(), channels: ChannelSet::COLOUR, mode: Toward::Paint },
            FillArea::All,
        )
        .unwrap();
        let mode = if ctrl { PaintMode::Paint } else { PaintMode::Erase };
        let mut sculptor = painter(mode);
        stroke(&mut mesh, &mut sculptor, &view, &path(), ctrl, false).expect("a step");
        // More than half rubbed out: only the middle of the ring takes it all.
        let bare = (0..mesh.vertex_count()).filter(|&v| texel(&mesh, layer, v).colour.0[3] < 128).count();
        let left = (0..mesh.vertex_count()).filter(|&v| texel(&mesh, layer, v).colour.0[3] == 255).count();
        assert!(bare > 300, "ctrl {ctrl}: the eraser left only {bare} vertices bare");
        assert!(left > mesh.vertex_count() / 2, "ctrl {ctrl}: it erased far from the stroke");
    }
}

/// A red half and a bare half: Blur across the border softens it, and
/// leaves what is far from the stroke as it was.
#[test]
fn blur_softens_an_edge_and_only_under_the_ring() {
    let (mut mesh, layer) = painted_ball(60_000);
    let view = view_of(&mesh);
    let _ = mesh.remap_mask(|m, v, _| if m.positions[v as usize].x > 0.0 { 1.0 } else { 0.0 });
    let target = PaintTarget { texel: painter(PaintMode::Paint).settings.paint.texel(), channels: ChannelSet::COLOUR, mode: Toward::Paint };
    mesh.fill_paint(&target, FillArea::SculptMask { invert: false }).unwrap();
    let _ = mesh.remap_mask(|_, _, _| 0.0);
    let before: Vec<u8> = (0..mesh.vertex_count()).map(|v| texel(&mesh, layer, v).colour.0[3]).collect();
    let partial = |m: &Mesh| (0..m.vertex_count()).filter(|&v| (1..255).contains(&texel(m, layer, v).colour.0[3])).count();
    let was = partial(&mesh);
    let mut sculptor = painter(PaintMode::Blur);
    let c = SIZE * 0.5;
    stroke(&mut mesh, &mut sculptor, &view, &line(c + Vec2::new(0.0, -80.0), c + Vec2::new(0.0, 80.0), 40), false, false);
    assert!(partial(&mesh) > was + 50, "blur made {} half-covered vertices, there were {was}", partial(&mesh));
    let far = (0..mesh.vertex_count()).filter(|&v| mesh.positions[v].x.abs() > 0.2 && mesh.positions[v].z > 0.0);
    for v in far {
        assert_eq!(texel(&mesh, layer, v).colour.0[3], before[v], "blur reached a vertex far from the stroke");
    }
}

#[test]
fn shift_blurs_the_paint_with_the_paint_brush_in_hand() {
    let sculptor = painter(PaintMode::Paint);
    assert_eq!(sculptor.effective_brush(true), Brush::Paint, "Shift must not take up Smooth over paint");
    let (mut mesh, layer) = painted_ball(40_000);
    let view = view_of(&mesh);
    let positions = mesh.positions.clone();
    let mut paint = painter(PaintMode::Paint);
    stroke(&mut mesh, &mut paint, &view, &path(), false, false);
    let painted = stack(&mesh);
    let mut shifted = painter(PaintMode::Paint);
    stroke(&mut mesh, &mut shifted, &view, &path(), false, true).expect("Shift blurred the paint");
    assert!(stack(&mesh) != painted && mesh.positions == positions, "Shift should blur the paint and move nothing");
    assert!(covered(&mesh, layer) > 0);
}

/// Red on the left half; a Smear stroke from the red into the bare half
/// carries red a way into it, fading.
#[test]
fn smear_drags_the_paint_along_the_stroke() {
    let (mut mesh, layer) = painted_ball(60_000);
    let view = view_of(&mesh);
    let _ = mesh.remap_mask(|m, v, _| if m.positions[v as usize].x < 0.0 { 1.0 } else { 0.0 });
    let target = PaintTarget { texel: painter(PaintMode::Paint).settings.paint.texel(), channels: ChannelSet::COLOUR, mode: Toward::Paint };
    mesh.fill_paint(&target, FillArea::SculptMask { invert: false }).unwrap();
    let _ = mesh.remap_mask(|_, _, _| 0.0);
    let reds = |m: &Mesh| (0..m.vertex_count()).filter(|&v| m.positions[v].x > 0.08 && texel(m, layer, v).colour.0[3] > 40).count();
    assert_eq!(reds(&mesh), 0);
    let mut sculptor = painter(PaintMode::Smear);
    let c = SIZE * 0.5;
    stroke(&mut mesh, &mut sculptor, &view, &line(c + Vec2::new(-120.0, 0.0), c + Vec2::new(120.0, 0.0), 60), false, false);
    assert!(reds(&mesh) > 50, "smear carried red onto only {} vertices of the bare side", reds(&mesh));
}

#[test]
fn sample_is_a_click_and_picks_up_the_paint() {
    let (mut mesh, _) = painted_ball(20_000);
    let view = view_of(&mesh);
    let target = PaintTarget { texel: painter(PaintMode::Paint).settings.paint.texel(), channels: ChannelSet::COLOUR, mode: Toward::Paint };
    mesh.fill_paint(&target, FillArea::All).unwrap();
    let mut sculptor = painter(PaintMode::Sample);
    let sample = PointerSample { pos: SIZE * 0.5, pressure: None, time: 0.0 };
    assert!(!sculptor.begin(&mut mesh, &view, sample, false, false), "Sample makes no stroke");
    let (origin, dir) = view.ray(SIZE * 0.5);
    let picked = pick_along(&mesh, origin, dir).expect("paint under the middle of the view");
    assert_eq!((picked.colour, picked.material), ([220, 40, 30], None));
    assert!((picked.coverage - 1.0).abs() < 1e-6);
}

#[test]
fn masked_and_hidden_surface_takes_no_paint() {
    let (mut mesh, layer) = painted_ball(40_000);
    let view = view_of(&mesh);
    let _ = mesh.remap_mask(|_, _, _| 1.0);
    let mut sculptor = painter(PaintMode::Paint);
    assert!(stroke(&mut mesh, &mut sculptor, &view, &path(), false, false).is_none(), "masked surface was painted");
    let _ = mesh.remap_mask(|_, _, _| 0.0);
    // The right half hidden: a stroke from the left over the plane paints only what shows.
    let right: Vec<u32> = (0..mesh.triangle_count() as u32)
        .filter(|&t| mesh.indices[t as usize * 3..t as usize * 3 + 3].iter().all(|&v| mesh.positions[v as usize].x > 0.0))
        .collect();
    mesh.hide(&right);
    let mut sculptor = painter(PaintMode::Paint);
    stroke(&mut mesh, &mut sculptor, &view, &path(), false, false).expect("the shown half took paint");
    let hidden = mesh.hidden_vertices().expect("hidden").to_vec();
    let on_hidden = (0..mesh.vertex_count()).filter(|&v| hidden[v] && texel(&mesh, layer, v).colour.0[3] > 0).count();
    assert_eq!(on_hidden, 0, "hidden surface took paint");
    assert!(covered(&mesh, layer) > 100);
}

#[test]
fn opacity_is_a_ceiling_however_often_a_place_is_gone_over() {
    let (mut mesh, layer) = painted_ball(40_000);
    let view = view_of(&mesh);
    let mut sculptor = painter(PaintMode::Paint);
    sculptor.settings.paint.opacity = 0.4;
    let c = SIZE * 0.5;
    // Back and forth over one place in one stroke.
    let mut points = Vec::new();
    for _ in 0..4 {
        points.extend(line(c + Vec2::new(-60.0, 0.0), c + Vec2::new(60.0, 0.0), 20));
        points.extend(line(c + Vec2::new(60.0, 0.0), c + Vec2::new(-60.0, 0.0), 20));
    }
    stroke(&mut mesh, &mut sculptor, &view, &points, false, false);
    assert_eq!(most_cover(&mesh, layer), 102, "40% of 255 is the most one stroke lays");
}

#[test]
fn zadd_raises_what_it_paints_as_one_undo_step() {
    let (mut mesh, layer) = painted_ball(60_000);
    let view = view_of(&mesh);
    let (positions, before) = (mesh.positions.clone(), stack(&mesh));
    let mut sculptor = painter(PaintMode::Paint);
    sculptor.settings.paint.zadd = true;
    sculptor.settings.symmetry = [true, false, false];
    let delta = stroke(&mut mesh, &mut sculptor, &view, &path(), false, false).expect("a step");
    assert!(matches!(delta, StrokeDelta::Painted { .. }), "Zadd's step holds the paint and the shape");
    assert!(covered(&mesh, layer) > 500);
    let raised = mesh.positions.iter().zip(&positions).filter(|(a, b)| a.length() > b.length() + 1e-4).count();
    assert!(raised > 200, "Zadd raised only {raised} vertices");
    let twins = twins(&mesh);
    let off = (0..mesh.vertex_count())
        .map(|v| (mesh.positions[v] * Vec3::new(-1.0, 1.0, 1.0)).distance(mesh.positions[twins[v] as usize]))
        .fold(0.0f32, f32::max);
    assert!(off < 1e-6, "Zadd under Mirror X left the halves {off} apart");
    mesh.apply_delta(&delta, true);
    assert!(mesh.positions == positions && stack(&mesh) == before, "undo left something of the Zadd stroke");
}

#[test]
fn paint_never_refines_even_with_dynamic_topology_on_and_paints_a_refined_mesh() {
    let (mut mesh, layer) = painted_ball(4_000);
    let view = view_of(&mesh);
    // Refined first by a sculpting stroke.
    let mut clay = Sculptor::default();
    clay.settings.brush = Brush::Clay;
    clay.settings.screen_space_size = false;
    clay.settings.radius_world = 0.1;
    clay.dyntopo.enabled = true;
    clay.dyntopo.detail = 0.2;
    let triangles = mesh.triangle_count();
    stroke(&mut mesh, &mut clay, &view, &path(), false, false);
    assert!(mesh.triangle_count() > triangles, "the clay stroke was meant to refine");
    let (triangles, vertices) = (mesh.triangle_count(), mesh.vertex_count());
    for (mode, zadd) in [(PaintMode::Paint, false), (PaintMode::Paint, true), (PaintMode::Blur, false)] {
        let mut sculptor = painter(mode);
        sculptor.dyntopo.enabled = true;
        sculptor.dyntopo.detail = 0.05;
        sculptor.settings.paint.zadd = zadd;
        assert!(!sculptor.may_refine(&mesh, false));
        stroke(&mut mesh, &mut sculptor, &view, &path(), false, false);
        assert_eq!((mesh.triangle_count(), mesh.vertex_count()), (triangles, vertices), "{mode:?} zadd {zadd} refined");
    }
    // The vertices dynamic topology made are painted like the rest.
    let made = (vertices / 2..vertices).filter(|&v| texel(&mesh, layer, v).colour.0[3] > 0).count();
    assert!(made > 0, "none of the refined vertices took paint");
    assert!(mesh.paint_sound().is_ok());
}

#[test]
fn paint_layer_mask_paints_the_layers_own_mask() {
    let (mut mesh, layer) = painted_ball(40_000);
    let view = view_of(&mesh);
    let row = mesh.paint_rows()[0].clone();
    mesh.set_paint_row(PaintRow { mask: crate::mesh::paint::MaskSource::Paintable, ..row }).unwrap();
    let mut sculptor = painter(PaintMode::Paint);
    sculptor.settings.paint.layer_mask = true;
    stroke(&mut mesh, &mut sculptor, &view, &path(), false, false).expect("a step");
    let shut = (0..mesh.vertex_count()).filter(|&v| texel(&mesh, layer, v).mask.0 < 128).count();
    assert!(shut > 300, "the stroke shut the layer's mask at only {shut} vertices");
    assert_eq!(covered(&mesh, layer), 0, "painting the mask laid colour");
    let mut sculptor = painter(PaintMode::Paint);
    sculptor.settings.paint.layer_mask = true;
    stroke(&mut mesh, &mut sculptor, &view, &path(), true, false).expect("a step");
    let shut = (0..mesh.vertex_count()).filter(|&v| texel(&mesh, layer, v).mask.0 < 128).count();
    assert!(shut < 50, "Ctrl should open the mask again, {shut} vertices stayed shut");
}

#[test]
fn a_material_and_roughness_are_laid_with_the_colour() {
    let (mut mesh, layer) = painted_ball(40_000);
    mesh.add_paint_material(7).unwrap();
    let view = view_of(&mesh);
    let mut sculptor = painter(PaintMode::Paint);
    sculptor.settings.paint.slot = Some(7);
    (sculptor.settings.paint.roughness, sculptor.settings.paint.roughness_value) = (true, 0.2);
    stroke(&mut mesh, &mut sculptor, &view, &path(), false, false);
    let full = (0..mesh.vertex_count()).find(|&v| texel(&mesh, layer, v).colour.0[3] == 255).expect("a covered vertex");
    let t = texel(&mesh, layer, full);
    assert_eq!(t.material.0, [7, 255]);
    assert_eq!((t.rough_metal.0[0], t.rough_metal.0[1], t.rough_metal.0[3]), (51, 255, 0));
    // No material picked: the material under the paint is left alone.
    sculptor.settings.paint.slot = None;
    assert!(!sculptor.settings.paint.channels(PaintMode::Paint).any(ChannelSet::MATERIAL));
}

#[test]
fn a_settings_file_from_before_the_paint_brush_gives_its_defaults() {
    let prefs = crate::prefs::Prefs::parse(r#"{ "brush": { "radius_px": 40.0, "brush": "Clay" } }"#);
    let mut settings = crate::sculpt::Settings::default();
    prefs.brush.apply(&mut settings);
    assert_eq!(settings.paint, PaintSettings::default());
    assert_eq!(settings.per_brush[Brush::Paint.index()], crate::sculpt::Settings::default().per_brush[Brush::Paint.index()]);
    // Nonsense comes back in range, and a brush left on Sample starts on Paint.
    let prefs = crate::prefs::Prefs::parse(
        r#"{ "brush": { "paint": { "opacity": 7.0, "mode": "Sample", "zadd_brush": "Move", "colour": [1, 2, 3] } } }"#,
    );
    prefs.brush.apply(&mut settings);
    let p = &settings.paint;
    assert_eq!((p.opacity, p.mode, p.zadd_brush, p.colour), (1.0, PaintMode::Paint, Brush::Standard, [1, 2, 3]));
    // And what is kept comes back.
    let mut kept = PaintSettings { colour: [9, 8, 7], opacity: 0.3, mode: PaintMode::Smear, zadd: true, ..PaintSettings::default() };
    kept.remember([1, 1, 1]);
    let mut out = PaintSettings::default();
    PaintKept::of(&kept).apply(&mut out);
    assert_eq!(out, kept);
}

/// Stay in Group: a stroke begun on one face group paints that group alone.
#[test]
fn a_stroke_kept_to_its_group_paints_that_group_alone() {
    let (mut mesh, layer) = painted_ball(60_000);
    let view = view_of(&mesh);
    let ids: Vec<u16> = (0..mesh.triangle_count())
        .map(|t| {
            let c = mesh.indices[t * 3..t * 3 + 3].iter().map(|&v| mesh.positions[v as usize]).sum::<Vec3>();
            if c.x < 0.0 { 1 } else { 2 }
        })
        .collect();
    mesh.set_group_ids(ids);
    let in_two = mesh.group_vertices(2);
    let in_one = mesh.group_vertices(1);
    mesh.stay_in_groups(&[1]);
    let mut sculptor = painter(PaintMode::Paint);
    stroke(&mut mesh, &mut sculptor, &view, &path(), false, false).expect("a step");
    let strays = (0..mesh.vertex_count()).filter(|&v| in_two[v] && !in_one[v] && texel(&mesh, layer, v).colour.0[3] > 0).count();
    assert_eq!(strays, 0, "the stroke painted the other group");
    assert!(covered(&mesh, layer) > 300);
}

/// An alpha is a stencil: painting through one leaves its picture in the
/// paint, and a picture that is white all over paints as no picture does.
#[test]
fn paint_goes_through_the_brushs_alpha() {
    use crate::sculpt::alpha::{self, AlphaSettings};
    let painted = |alpha: AlphaSettings| {
        let (mut mesh, layer) = painted_ball(60_000);
        let view = view_of(&mesh);
        let mut sculptor = painter(PaintMode::Paint);
        sculptor.settings.alpha[Brush::Paint.index()] = alpha;
        stroke(&mut mesh, &mut sculptor, &view, &path(), false, false);
        (0..mesh.vertex_count()).map(|v| texel(&mesh, layer, v).colour.0[3]).collect::<Vec<u8>>()
    };
    let plain = painted(AlphaSettings::default());
    let raked = painted(AlphaSettings { image: alpha::find("Rake"), ..AlphaSettings::default() });
    let differing = plain.iter().zip(&raked).filter(|(a, b)| a != b).count();
    assert!(differing > 100, "the alpha changed only {differing} vertices' paint");
    assert!(raked.iter().zip(&plain).all(|(r, p)| r <= p), "a picture only ever holds paint back");
}

/// A layer given a Cavity mask takes its snapshot in the same undo step.
#[test]
fn a_cavity_mask_takes_its_snapshot_in_the_same_step() {
    let (mut mesh, layer) = painted_ball(20_000);
    let before = stack(&mesh);
    let row = mesh.paint_rows()[0].clone();
    let mut step = mesh.set_paint_row(PaintRow { mask: crate::mesh::paint::MaskSource::Cavity, ..row }).unwrap();
    assert!(stack(&mesh).layer(layer).unwrap().data.cavity.is_some(), "no snapshot was taken");
    step.swap(&mut mesh);
    assert!(stack(&mesh) == before, "one undo step takes the mask and the snapshot back");
}
