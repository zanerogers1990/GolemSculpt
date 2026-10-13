//! Erase Layer on a ball with two layers: what a stroke of it does to the
//! surface and to the layer it rubs out, and everything it must leave alone
//! (the model underneath, the other layer, what is outside the ring, masked
//! and hidden parts). Under radial symmetry it is in
//! `symmetry_tests/erase_layer.rs`.

use glam::Vec2;
use sculpt_multires::Split;

use super::*;
use crate::camera::View;
use crate::mesh::StrokeDelta;
use crate::mesh::layers::LayerRow;
use crate::mesh::levels::{stepped, subdivided};
use crate::sculpt::stroke_tests::{SIZE, across, sculptor_with, sphere, spread, twins, view_of};
use crate::sculpt::{Brush, FalloffCurve, FalloffPreset, PointerSample, Sculptor, alpha};

/// The brush's radius on the model: about a quarter of the ball's.
const RING: f32 = 0.12;
/// Triangles in the ball: some three hundred of its points under the flat of the ring.
const TRIANGLES: usize = 120_000;
/// Half a unit in the last place of a coordinate of the ball (which is half
/// a unit across, a little more with its layers): what one rounding costs.
const ROUNDING: f32 = f32::EPSILON * 0.25;

/// Fine ridges all over the ball, nowhere none: the layer that is rubbed
/// out. A mirror image of itself across X, to the bit.
fn ridges(p: Vec3) -> Vec3 {
    p + p.normalize() * (0.008 * (1.5 + (p.y * 60.0).cos()))
}

/// A broad swell, front to back: the layer that must be left alone.
fn swell(p: Vec3) -> Vec3 {
    p + p.normalize() * (0.006 * (1.5 + (p.z * 9.0).sin()))
}

/// Moves every vertex of the mesh to where `shape` puts it, as one stroke.
fn sculpt(mesh: &mut Mesh, shape: impl Fn(Vec3) -> Vec3 + Sync) {
    let leaves: Vec<u32> = (0..mesh.bvh.leaves.len() as u32).collect();
    mesh.begin_stroke();
    mesh.move_vertices(&leaves, |_, vertex| Some(shape(vertex.pos)));
    mesh.positions_changed(&leaves);
    mesh.end_stroke(false).expect("the stroke moved the mesh");
    mesh.flush_normals();
    let _ = mesh.take_dirty();
}

fn offsets(mesh: &Mesh, layer: u32) -> Vec<Vec3> {
    mesh.layers.as_ref().and_then(|layers| layers.offsets(layer)).expect("the layer is there").to_vec()
}

fn bits(points: &[Vec3]) -> Vec<[u32; 3]> {
    points.iter().map(|p| p.to_array().map(f32::to_bits)).collect()
}

fn same(a: Vec3, b: Vec3) -> bool {
    a.to_array().map(f32::to_bits) == b.to_array().map(f32::to_bits)
}

/// The furthest two shapes are apart at any vertex.
fn apart(a: &[Vec3], b: &[Vec3]) -> f32 {
    assert_eq!(a.len(), b.len());
    a.iter().zip(b).map(|(a, b)| a.distance(*b)).fold(0.0, f32::max)
}

/// A mesh with two layers sculpted on it, "Detail" (the ridges) recording
/// and "Other" (the swell) not.
struct Model {
    mesh: Mesh,
    detail: u32,
    other: u32,
    /// The shape as it shows, and what Detail holds, before any erasing.
    shown: Vec<Vec3>,
    held: Vec<Vec3>,
}

/// Everything about a model a stroke could change, to the bit: positions,
/// rows, and each layer's offsets.
type State = (Vec<[u32; 3]>, Vec<LayerRow>, Vec<[u32; 3]>, Vec<[u32; 3]>);

impl Model {
    /// `mesh` with the two layers, Detail at `percent`.
    fn on(mut mesh: Mesh, percent: u8) -> Model {
        let (detail, _) = mesh.add_layer(Some("Detail".into()));
        sculpt(&mut mesh, ridges);
        let (other, _) = mesh.add_layer(Some("Other".into()));
        sculpt(&mut mesh, swell);
        mesh.record_on_layer(Some(detail)).expect("Detail records again");
        if percent != 100 {
            mesh.set_layer(detail, true, percent).expect("the strength changed");
        }
        let (shown, held) = (mesh.positions.clone(), offsets(&mesh, detail));
        Model { mesh, detail, other, shown, held }
    }

    fn new(percent: u8) -> Model {
        Model::on(sphere(TRIANGLES), percent)
    }

    fn state(&self) -> State {
        let of = |layer| bits(&offsets(&self.mesh, layer));
        (bits(&self.mesh.positions), self.mesh.layer_rows(), of(self.detail), of(self.other))
    }

    /// How much of the way from where it showed to `bare` each vertex has gone.
    fn erased(&self, bare: &[Vec3]) -> Vec<f32> {
        let share = |v: usize| {
            let whole = bare[v] - self.shown[v];
            (self.mesh.positions[v] - self.shown[v]).dot(whole) / whole.length_squared()
        };
        (0..bare.len()).map(share).collect()
    }
}

/// Where every vertex of such a model stands with Detail hidden: the model
/// underneath with the other layer on it, as the Layers card's eye leaves it.
fn without_detail(percent: u8) -> Vec<Vec3> {
    let mut model = Model::new(percent);
    model.mesh.set_layer(model.detail, false, percent).expect("the eye changed the shape");
    model.mesh.positions
}

/// A sculptor holding Erase Layer at a fixed size on the model. `flat` gives
/// it the flat-topped falloff: full weight out to six tenths of the ring.
fn eraser(strength: f32, flat: bool) -> Sculptor {
    let mut sculptor = sculptor_with(Brush::EraseLayer, RING, strength);
    sculptor.settings.falloff[Brush::EraseLayer.index()] = flat.then(|| FalloffCurve::preset(FalloffPreset::Plateau));
    sculptor
}

/// The point of the mesh under `at`.
fn under(mesh: &Mesh, view: &View, at: Vec2) -> Vec3 {
    let (origin, dir) = view.ray(at);
    mesh.raycast(origin, dir).expect("the pointer is over the ball").pos
}

/// A stroke through `points`, four samples to a frame, at `pressure` if it
/// is a pen's. Gives back the stroke's undo record.
fn stroke(mesh: &mut Mesh, sculptor: &mut Sculptor, view: &View, points: &[Vec2], pressure: Option<f32>) -> Option<StrokeDelta> {
    let sample = |i: usize| PointerSample { pos: points[i], pressure, time: 50.0 + i as f64 / 240.0 };
    assert!(sculptor.begin(mesh, view, sample(0), false, false), "the stroke missed the mesh");
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

/// A press at `at`, held still for a few frames.
fn press(mesh: &mut Mesh, sculptor: &mut Sculptor, view: &View, at: Vec2) -> Option<StrokeDelta> {
    stroke(mesh, sculptor, view, &[at; 13], None)
}

/// A patch rubbed right out, at full strength under a flat-topped ring, with
/// the layer at 100%, at 40% and at 1%. The surface there is where hiding the layer
/// would put it, to the bit; the layer holds nothing there but the rounding
/// of a position; outside the ring nothing has changed, the other layer has
/// not, nor has the model underneath; undo and redo put back every bit; and
/// the layer raised, hidden and shown again has the patch still gone.
#[test]
fn a_patch_rubbed_right_out_stands_on_the_model_underneath_and_the_layer_holds_nothing_there() {
    for percent in [100u8, 40, 1] {
        let strength = percent as f32 / 100.0;
        let mut model = Model::new(percent);
        let bare = without_detail(percent);
        let (underneath, before) = (model.mesh.model_underneath(), model.state());
        let view = view_of(&model.mesh);
        let centre = under(&model.mesh, &view, SIZE * 0.5);
        let delta = press(&mut model.mesh, &mut eraser(1.0, true), &view, SIZE * 0.5).expect("the stroke is an undo step");
        assert!(matches!(delta, StrokeDelta::Layered { .. }), "the stroke's record holds what the layer gave up");
        let after = model.state();

        let left = offsets(&model.mesh, model.detail);
        let inside: Vec<usize> = (0..bare.len()).filter(|&v| model.shown[v].distance(centre) < 0.55 * RING).collect();
        let outside: Vec<usize> = (0..bare.len()).filter(|&v| model.shown[v].distance(centre) > 1.001 * RING).collect();
        assert!(inside.len() > 150 && outside.len() > 50_000, "{} inside, {} outside", inside.len(), outside.len());
        for &v in &inside {
            assert!(same(model.mesh.positions[v], bare[v]), "{percent}%: vertex {v} is not where hiding the layer puts it");
        }
        for &v in &outside {
            assert!(same(model.mesh.positions[v], model.shown[v]) && same(left[v], model.held[v]), "{percent}%: vertex {v}");
        }
        let held = inside.iter().map(|&v| model.held[v].length()).fold(f32::MAX, f32::min);
        let residue = inside.iter().map(|&v| left[v].abs().max_element()).fold(0.0, f32::max);
        println!("layer at {percent}%: the patch held {held:.4} at least and holds {residue:.2e} at most");
        assert!(held > 0.003 && residue <= 2.0 * ROUNDING / strength, "{residue:e}");

        assert!(after.1 == before.1 && after.3 == before.3, "{percent}%: the rows and the other layer, to the bit");
        let moved = apart(&model.mesh.model_underneath(), &underneath);
        println!("layer at {percent}%: the model underneath, worked out again, is {moved:.2e} from where it was");
        assert!(moved <= 2.0 * ROUNDING, "{moved:e}");

        model.mesh.apply_delta(&delta, true);
        assert!(model.state() == before, "{percent}%: undo");
        model.mesh.apply_delta(&delta, false);
        assert!(model.state() == after, "{percent}%: redo");

        // The slider raised to 100%: what the layer kept of the patch does not show.
        let (whole, bare) = (Model::new(100).shown, without_detail(100));
        if percent != 100 {
            model.mesh.set_layer(model.detail, true, 100).expect("the strength changed");
        }
        let patch = |mesh: &Mesh| inside.iter().map(|&v| mesh.positions[v].distance(bare[v])).fold(0.0, f32::max);
        let around = |mesh: &Mesh| outside.iter().map(|&v| mesh.positions[v].distance(whole[v])).fold(0.0, f32::max);
        println!("raised to 100%: the patch is {:.2e} from the model underneath, the rest {:.2e} from the sculpt", patch(&model.mesh), around(&model.mesh));
        assert!(patch(&model.mesh) <= 4.0 * ROUNDING / strength && around(&model.mesh) <= 4.0 * ROUNDING);
        // Hidden: the model underneath and the other layer, everywhere. Shown again: the patch is still gone.
        model.mesh.set_layer(model.detail, false, 100).expect("the eye changed the shape");
        assert!(apart(&model.mesh.positions, &bare) <= 4.0 * ROUNDING / strength);
        model.mesh.set_layer(model.detail, true, 100).expect("the eye changed the shape");
        assert!(patch(&model.mesh) <= 4.0 * ROUNDING / strength && around(&model.mesh) <= 4.0 * ROUNDING);
    }
}

/// A light stroke takes part of the layer away and leaves the layer holding
/// exactly the rest. Every pass after it takes more, none takes a vertex
/// past where it would stand without the layer, and enough of them leave the
/// middle of the stroke clean.
#[test]
fn a_light_stroke_takes_some_of_the_layer_and_more_passes_the_rest_and_no_more() {
    let mut model = Model::new(100);
    let bare = without_detail(100);
    let view = view_of(&model.mesh);
    let path = across(&view, -0.25, 0.25, 60);
    let mut sculptor = eraser(0.1, false);
    let middle: Vec<usize> = (0..bare.len())
        .filter(|&v| model.shown[v].z > 0.0 && model.shown[v].y.abs() < 0.01 && model.shown[v].x.abs() < 0.06)
        .collect();
    assert!(middle.len() > 20);
    let mut last = vec![0.0f32; bare.len()];
    let mut passes = Vec::new();
    for pass in 0..40 {
        let delta = stroke(&mut model.mesh, &mut sculptor, &view, &path, None);
        assert!(delta.is_some() || pass > 0, "the first pass did nothing");
        let (erased, left) = (model.erased(&bare), offsets(&model.mesh, model.detail));
        for v in 0..bare.len() {
            assert!(erased[v] >= last[v] - 1e-4, "pass {pass}: vertex {v} went back, from {} to {}", last[v], erased[v]);
            assert!(erased[v] <= 1.0 + 1e-4, "pass {pass}: vertex {v} went {} of the way, past the model underneath", erased[v]);
            let rest = model.held[v] * (1.0 - erased[v]);
            assert!(left[v].distance(rest) < 2e-6, "pass {pass}: the layer holds {:?} at {v}, not the {rest:?} left", left[v]);
        }
        passes.push(middle.iter().map(|&v| erased[v]).sum::<f32>() / middle.len() as f32);
        last = erased;
    }
    println!("erased along the middle of the stroke after 1, 2, 5, 40 passes: {:.4} {:.4} {:.4} {:.6}", passes[0], passes[1], passes[4], passes[39]);
    assert!((0.2..0.7).contains(&passes[0]), "one light pass took {}", passes[0]);
    assert!(passes[1] > passes[0] + 0.1 && passes[4] > passes[1] + 0.1);
    assert!(passes[39] > 0.9999, "forty passes left {}", 1.0 - passes[39]);
}

/// With no layer for it to rub out the brush leaves the mesh alone to the
/// bit and makes no undo step, and says what to do: on a mesh with no
/// layers, with none recording, and with the one that records hidden or at
/// 0%.
#[test]
fn with_no_layer_to_rub_out_a_stroke_changes_nothing_and_is_no_undo_step() {
    let try_it = |mesh: &mut Mesh, expected: &str| {
        let before = (bits(&mesh.positions), mesh.layers_print());
        let said = refusal(mesh).expect("the brush has nothing to do, and says so");
        assert!(said.contains(expected), "{said}");
        let view = view_of(mesh);
        let delta = stroke(mesh, &mut eraser(1.0, false), &view, &across(&view, -0.2, 0.2, 40), None);
        assert!(delta.is_none(), "{expected}: a stroke that did nothing is an undo step");
        assert!((bits(&mesh.positions), mesh.layers_print()) == before, "{expected}: the stroke changed something");
    };
    try_it(&mut sphere(TRIANGLES), "this object has no layers");
    let mut model = Model::new(100);
    assert!(refusal(&model.mesh).is_none(), "a layer is recording and shows: there is something to erase");
    model.mesh.record_on_layer(None).expect("recording stopped");
    try_it(&mut model.mesh, "none is");
    model.mesh.record_on_layer(Some(model.detail)).unwrap();
    model.mesh.set_layer(model.detail, false, 100).unwrap();
    try_it(&mut model.mesh, "\"Detail\" is hidden");
    model.mesh.set_layer(model.detail, true, 0).unwrap();
    try_it(&mut model.mesh, "\"Detail\" is at 0%");
    // Recording on the other layer, it is that one the brush rubs out.
    model.mesh.set_layer(model.detail, true, 100).unwrap();
    model.mesh.record_on_layer(Some(model.other)).unwrap();
    let (detail, other) = (offsets(&model.mesh, model.detail), offsets(&model.mesh, model.other));
    let view = view_of(&model.mesh);
    press(&mut model.mesh, &mut eraser(1.0, true), &view, SIZE * 0.5).expect("an undo step");
    assert!(bits(&offsets(&model.mesh, model.detail)) == bits(&detail), "Detail is not recording and is left alone");
    assert!(apart(&offsets(&model.mesh, model.other), &other) > 0.002, "Other is recording and is rubbed out");
}

/// Ctrl and the right button erase as the left does: the brush has no
/// opposite (`erase.rs` says why).
#[test]
fn ctrl_erases_too() {
    let erased = |invert: bool| {
        let mut model = Model::new(100);
        let view = view_of(&model.mesh);
        let mut sculptor = eraser(0.5, false);
        let sample = PointerSample { pos: SIZE * 0.5, pressure: None, time: 1.0 };
        assert!(sculptor.begin(&mut model.mesh, &view, sample, invert, false));
        sculptor.end(&mut model.mesh, &view).expect("an undo step");
        bits(&model.mesh.positions)
    };
    assert!(!Brush::EraseLayer.has_polarity());
    assert!(erased(true) == erased(false));
}

/// Masked vertices are left alone to the bit, hidden ones too, and with
/// Front Facing Only the side turned away from the camera.
#[test]
fn masked_hidden_and_far_side_vertices_are_left_alone() {
    // Masked: the half of the ball left of the middle.
    let mut model = Model::new(100);
    let view = view_of(&model.mesh);
    let centre = under(&model.mesh, &view, SIZE * 0.5);
    for v in 0..model.shown.len() {
        model.mesh.mask[v] = if model.shown[v].x < 0.0 { 1.0 } else { 0.0 };
    }
    press(&mut model.mesh, &mut eraser(1.0, true), &view, SIZE * 0.5).expect("an undo step");
    let near = |model: &Model, v: usize| model.shown[v].distance(centre) < 0.55 * RING;
    let (mut kept, mut gone) = (0, 0);
    for v in 0..model.shown.len() {
        if model.shown[v].x < 0.0 {
            assert!(same(model.mesh.positions[v], model.shown[v]), "masked vertex {v} moved");
            kept += usize::from(near(&model, v));
        } else if near(&model, v) {
            assert!(!same(model.mesh.positions[v], model.shown[v]), "unmasked vertex {v} under the ring stayed");
            gone += 1;
        }
    }
    assert!(kept > 50 && gone > 50, "{kept} masked and {gone} unmasked under the ring");

    // Hidden: every face left of the middle.
    let mut model = Model::new(100);
    let left: Vec<u32> = (0..model.mesh.indices.len() as u32 / 3)
        .filter(|&t| (0..3).all(|c| model.shown[model.mesh.indices[t as usize * 3 + c] as usize].x < 0.0))
        .collect();
    model.mesh.hide(&left).expect("faces were hidden");
    press(&mut model.mesh, &mut eraser(1.0, true), &view, SIZE * 0.5).expect("an undo step");
    let (mut kept, mut gone) = (0, 0);
    for v in 0..model.shown.len() {
        if model.mesh.is_vertex_hidden(v as u32) {
            assert!(same(model.mesh.positions[v], model.shown[v]), "hidden vertex {v} moved");
            kept += usize::from(near(&model, v));
        } else {
            gone += usize::from(near(&model, v) && !same(model.mesh.positions[v], model.shown[v]));
        }
    }
    assert!(kept > 50 && gone > 50, "{kept} hidden and {gone} showing under the ring");

    // Front Facing Only, with a ring that reaches right round the ball.
    for front_facing in [false, true] {
        let mut model = Model::new(100);
        let mut sculptor = sculptor_with(Brush::EraseLayer, 1.2, 1.0);
        sculptor.settings.current_mut().front_facing = front_facing;
        press(&mut model.mesh, &mut sculptor, &view, SIZE * 0.5).expect("an undo step");
        let behind = (0..model.shown.len()).filter(|&v| model.shown[v].z < -0.1);
        let moved = behind.filter(|&v| !same(model.mesh.positions[v], model.shown[v])).count();
        assert_eq!(moved == 0, front_facing, "{moved} vertices moved on the far side");
    }
}

/// A mirrored stroke rubs out a mirror image, in the surface and in the
/// layer, and a stroke down the mirror plane is as strong as the same
/// stroke with no symmetry, on the plane and a third of a ring beside it.
#[test]
fn a_mirrored_stroke_rubs_out_its_mirror_image_and_is_as_strong_on_the_plane_as_beside_it() {
    let ball = sphere(TRIANGLES);
    let twin = twins(&ball);
    let view = view_of(&ball);
    let c = SIZE * 0.5;
    let run = |mirror: bool, path: &[Vec2]| {
        let mut model = Model::new(100);
        let mut sculptor = eraser(0.3, false);
        sculptor.settings.symmetry = [mirror, false, false];
        stroke(&mut model.mesh, &mut sculptor, &view, path, None).expect("an undo step");
        model
    };
    let (shown, bare) = (Model::new(100).shown, without_detail(100));
    let flip = Vec3::new(-1.0, 1.0, 1.0);

    // From one side, across the plane and back along it.
    let (side, plane, along) = (c + Vec2::new(-110.0, 60.0), c + Vec2::new(6.0, 10.0), c + Vec2::new(14.0, -90.0));
    let mut path: Vec<Vec2> = (0..=40).map(|i| side.lerp(plane, i as f32 / 40.0)).collect();
    path.extend((1..=40).map(|i| plane.lerp(along, i as f32 / 40.0)));
    let model = run(true, &path);
    let left = offsets(&model.mesh, model.detail);
    let (mut surface, mut layer, mut moved) = (0.0f32, 0.0f32, 0.0f32);
    for (v, &t) in twin.iter().enumerate() {
        surface = surface.max(model.mesh.positions[t as usize].distance(model.mesh.positions[v] * flip));
        layer = layer.max(left[t as usize].distance(left[v] * flip));
        moved = moved.max(model.mesh.positions[v].distance(model.shown[v]));
    }
    println!("mirrored: moved {moved:.5}; a vertex and its twin are {surface:.2e} apart, and {layer:.2e} in the layer");
    assert!(moved > 0.005 && surface == 0.0 && layer == 0.0);
    assert!(model.erased(&bare).iter().all(|&share| share <= 1.0 + 1e-4), "a vertex went past the model underneath");

    // Up the screen: down the plane, and a third of a ring to one side of it.
    let scale = view.project(Vec3::new(RING, 0.0, 0.5)).unwrap().x - c.x;
    for beside in [0.0, 1.0 / 3.0] {
        let x = c.x + beside * scale;
        let path: Vec<Vec2> = (0..=50).map(|i| Vec2::new(x, c.y + 80.0 - 160.0 * i as f32 / 50.0)).collect();
        let (alone, mirrored) = (run(false, &path).erased(&bare), run(true, &path).erased(&bare));
        let peak = |erased: &[f32]| erased.iter().copied().fold(0.0, f32::max);
        // Where the stroke alone was, from its own middle outward: nearer
        // the plane than that, it is the image's stroke that is the stronger.
        let under_it = (0..bare.len()).filter(|&v| alone[v] > 0.25 * peak(&alone) && shown[v].x >= beside * RING);
        let ratio: Vec<f64> = under_it.map(|v| (mirrored[v] / alone[v]) as f64).collect();
        let (low, high) = ratio.iter().fold((f64::MAX, f64::MIN), |(low, high), &r| (low.min(r), high.max(r)));
        println!(
            "{beside:.2} rings from the plane: the stroke alone erases {:.3} at most, mirrored {:.3}; under it ({} vertices), \
             {low:.3} to {high:.3} of alone",
            peak(&alone),
            peak(&mirrored),
            ratio.len()
        );
        assert!(ratio.len() > 200 && (0.85..=1.06).contains(&low) && (0.85..=1.06).contains(&high));
        if beside == 0.0 {
            // Both copies on one spot are one dab: not a hair stronger.
            assert!(spread(&[peak(&alone) as f64, peak(&mirrored) as f64]) < 1e-3);
        }
    }
}

/// The pen's pressure and Spacing: a light touch erases less than a firm
/// one, and the stroke erases as much at half and at twice the brush's own
/// spacing as it does at that.
#[test]
fn pressure_sets_how_much_is_erased_and_spacing_does_not() {
    let bare = without_detail(100);
    let erased = |spacing: f32, pressure: Option<f32>| {
        let mut model = Model::new(100);
        let view = view_of(&model.mesh);
        let mut sculptor = eraser(0.1, false);
        sculptor.settings.current_mut().spacing = spacing;
        sculptor.settings.pressure_curve = 1.0;
        stroke(&mut model.mesh, &mut sculptor, &view, &across(&view, -0.3, 0.3, 100), pressure).expect("an undo step");
        let in_the_middle = |v: &usize| model.shown[*v].z > 0.0 && model.shown[*v].x.abs() < 0.07;
        let shares = model.erased(&bare);
        (0..bare.len()).filter(in_the_middle).map(|v| shares[v] as f64).sum::<f64>()
    };
    let own = Brush::EraseLayer.default_spacing();
    let at = [erased(own * 0.5, None), erased(own, None), erased(own * 2.0, None)];
    println!("erased at half, its own and twice its spacing: {at:.2?}, {:.1}% apart", spread(&at) * 100.0);
    assert!(at[1] > 10.0 && spread(&at) < 0.03);
    let (light, firm) = (erased(own, Some(0.2)), erased(own, Some(1.0)));
    println!("erased by a light touch {light:.2}, by a firm one {firm:.2}");
    assert!(light > 1.0 && light < 0.5 * firm && (firm - at[1]).abs() < 0.02 * at[1]);
}

/// With an alpha the layer is rubbed out through the picture: nowhere more
/// than the plain brush erases, and by the picture's own pattern. Placed by
/// dragging, the print erases too (it must not raise, as the other brushes'
/// prints do), and a print dragged out and back in again leaves nothing.
#[test]
fn an_alpha_erases_through_its_picture_and_a_dragged_print_erases_too() {
    assert!(Brush::EraseLayer.takes_alpha());
    let bare = without_detail(100);
    let view = view_of(&Model::new(100).mesh);
    let with = |picture: Option<alpha::AlphaSettings>| {
        let mut model = Model::new(100);
        let mut sculptor = eraser(1.0, true);
        if let Some(picture) = picture {
            sculptor.settings.alpha[Brush::EraseLayer.index()] = picture;
        }
        press(&mut model.mesh, &mut sculptor, &view, SIZE * 0.5).expect("an undo step");
        model.erased(&bare)
    };
    let picture = alpha::AlphaSettings { image: alpha::find("Scales"), ..Default::default() };
    assert!(picture.image.is_some(), "the picture ships with the app");
    let (plain, through) = (with(None), with(Some(picture)));
    let under: Vec<usize> = (0..bare.len()).filter(|&v| plain[v] > 0.999).collect();
    assert!((0..bare.len()).all(|v| through[v] <= plain[v] + 1e-4), "the picture erased more than the brush does");
    let (kept, gone) = (under.iter().filter(|&&v| through[v] < 0.3).count(), under.iter().filter(|&&v| through[v] > 0.7).count());
    println!("under the flat of the ring: {} vertices, {kept} mostly kept and {gone} mostly erased by the picture", under.len());
    assert!(under.len() > 150 && kept > under.len() / 30 && gone > under.len() / 30);

    // Drag to Place: out to the right, and then back to where it was pressed.
    let drag = |back: bool| {
        let mut model = Model::new(100);
        let mut sculptor = eraser(1.0, false);
        sculptor.settings.alpha[Brush::EraseLayer.index()] = alpha::AlphaSettings { drag: true, ..picture };
        let at = |px: f32, i: usize| PointerSample { pos: SIZE * 0.5 + Vec2::new(px, 0.0), pressure: None, time: 1.0 + i as f64 / 60.0 };
        assert!(sculptor.begin(&mut model.mesh, &view, at(0.0, 0), false, false));
        let reach: Vec<f32> = if back { vec![30.0, 60.0, 90.0, 40.0, 0.0] } else { vec![30.0, 60.0, 90.0] };
        for (i, px) in reach.into_iter().enumerate() {
            sculptor.sample(&mut model.mesh, &view, at(px, i + 1));
            sculptor.frame(&mut model.mesh, &view, 1.0 / 60.0);
            let _ = model.mesh.take_dirty();
        }
        let delta = sculptor.end(&mut model.mesh, &view);
        (model.erased(&bare), delta.is_some(), bits(&model.mesh.positions) == bits(&model.shown))
    };
    let (print, step, _) = drag(false);
    let moved = print.iter().filter(|&&share| share != 0.0).count();
    let (least, most) = print.iter().fold((f32::MAX, f32::MIN), |(least, most), &share| (least.min(share), most.max(share)));
    println!("a dragged print moved {moved} vertices, from {least:.3} to {most:.3} of the way to the model underneath");
    assert!(step && moved > 200 && least >= -1e-4 && most > 0.5 && most <= 1.0 + 1e-4);
    assert!(drag(true).2, "a print dragged back to nothing left something behind");
}

/// The brush adds no triangles with dynamic topology on, and keeps erasing
/// while the pointer rests, as the other brushes with a place to stop at do.
#[test]
fn it_adds_no_triangles_and_keeps_working_while_the_pen_rests() {
    let mut model = Model::new(100);
    let bare = without_detail(100);
    let view = view_of(&model.mesh);
    let mut sculptor = eraser(0.05, false);
    sculptor.dyntopo.enabled = true;
    assert!(!sculptor.may_refine(&model.mesh, false));
    let triangles = model.mesh.triangle_count();
    let sample = |i: usize| PointerSample { pos: SIZE * 0.5, pressure: None, time: 1.0 + i as f64 / 60.0 };
    assert!(sculptor.begin(&mut model.mesh, &view, sample(0), false, false));
    let peak = |model: &Model| model.erased(&bare).into_iter().fold(0.0, f32::max);
    let opening = peak(&model);
    for _ in 0..30 {
        sculptor.frame(&mut model.mesh, &view, 1.0 / 60.0);
        let _ = model.mesh.take_dirty();
    }
    let rested = peak(&model);
    sculptor.end(&mut model.mesh, &view).expect("an undo step");
    println!("one dab erases {opening:.3} under the pen; held for half a second, {rested:.3}");
    assert!(opening > 0.02 && opening < 0.1 && rested > 3.0 * opening && rested <= 1.0 + 1e-4);
    assert_eq!(model.mesh.triangle_count(), triangles);
}

/// On a lower subdivision level, where a layer can record, the brush rubs
/// the layer out of that level: the surface there goes to where it would
/// stand without the layer, and the level's own share of the layer is gone.
/// Back on the finest level the patch has lost the layer's broad form and
/// kept only the fine detail that lives between the lower level's points;
/// with the layer hidden the model is as it was, and the other layer is as
/// it was.
#[test]
fn on_a_lower_subdivision_level_it_rubs_out_that_levels_share_of_the_layer() {
    let top = Model::on(subdivided(&sphere(24_000), Split::Smooth).expect("the ball subdivides").0, 100).mesh;
    let (detail, other) = (top.layer_rows()[0].id, top.layer_rows()[1].id);
    let ordered = |mesh: &Mesh| mesh.levels.as_ref().expect("the mesh has levels").ordered_positions(mesh);
    let hidden = |mesh: &Mesh, layer: u32| {
        let mut copy = stepped(mesh, 1).expect("a step to the level it is on").0;
        copy.set_layer(layer, false, 100).expect("the eye changed the shape");
        ordered(&copy)
    };
    let (shown_top, bare_top, plain_top) = (ordered(&top), hidden(&top, detail), hidden(&top, other));

    let mut low = stepped(&top, 0).expect("a step down").0;
    assert!(Erased::of(&low).is_some(), "the layer records on the lower level too");
    let (shown, held) = (low.positions.clone(), offsets(&low, detail));
    let view = view_of(&low);
    let centre = under(&low, &view, SIZE * 0.5);
    press(&mut low, &mut eraser(1.0, true), &view, SIZE * 0.5).expect("an undo step");
    let left = offsets(&low, detail);
    let inside: Vec<usize> = (0..shown.len()).filter(|&v| shown[v].distance(centre) < 0.55 * RING).collect();
    assert!(inside.len() > 20, "{} vertices of the lower level under the ring", inside.len());
    for &v in &inside {
        let bare = (shown[v].as_dvec3() - held[v].as_dvec3()).as_vec3();
        assert!(same(low.positions[v], bare) && left[v].abs().max_element() <= 2.0 * ROUNDING, "vertex {v}: {:?}", left[v]);
    }

    let up = stepped(&low, 1).expect("a step up").0;
    let (shown_up, bare_up, plain_up) = (ordered(&up), hidden(&up, detail), hidden(&up, other));
    let front = (0..shown_top.len()).filter(|&v| shown_top[v].distance(centre) < 0.4 * RING);
    let (mut was, mut is, mut count) = (0.0f64, 0.0f64, 0);
    for v in front {
        was += shown_top[v].distance(bare_top[v]) as f64;
        is += shown_up[v].distance(bare_up[v]) as f64;
        count += 1;
    }
    // What the other layer adds: the shape less the shape with it hidden.
    let adds = |shown: &[Vec3], plain: &[Vec3]| shown.iter().zip(plain).map(|(a, b)| *a - *b).collect::<Vec<Vec3>>();
    let (under_model, other_layer) = (apart(&bare_up, &bare_top), apart(&adds(&shown_up, &plain_up), &adds(&shown_top, &plain_top)));
    println!(
        "on the finest level, {count} vertices under the middle of the ring: the layer stood {:.5} on average and stands {:.5}; \
         with it hidden the model is {under_model:.2e} from where it was",
        was / count as f64,
        is / count as f64
    );
    assert!(count > 50 && is < 0.25 * was, "the layer's broad form is still there");
    assert!(under_model < 2e-6, "hiding the layer no longer leaves the model as it was: {under_model:e}");
    assert!(other_layer < 2e-6, "the other layer changed by {other_layer:e}");
    // The lower level, come back to, is the finest level's shape brought
    // down again, as after any stroke made down there: near what the stroke
    // left, not it to the bit (a press of Standard in the same place comes
    // back a thousandth of the ball's size off; the levels' own tests have
    // what a round trip keeps).
    let back = stepped(&up, 0).expect("a step down").0;
    let round_trip = apart(&ordered(&back), &ordered(&low));
    println!("the lower level after a step up and down again is {round_trip:.2e} from where the stroke left it");
    assert!(round_trip < 3e-3);
}
