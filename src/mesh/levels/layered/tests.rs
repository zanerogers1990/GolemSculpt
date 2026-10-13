//! Sculpt layers through changes of subdivision level: what a step keeps to
//! the bit, what it carries, and what a layer switched off afterwards leaves.

use glam::{Quat, Vec3};
use sculpt_multires::Split;

use crate::mesh::layers::Ride;
use crate::mesh::levels::{stepped, subdivided, without};
use crate::mesh::primitives::{self, Primitive};
use crate::mesh::{Mesh, StrokeDelta};

fn ball() -> Mesh {
    primitives::build(Primitive::Sphere, 1_500, 0.5)
}

fn bits(points: &[Vec3]) -> Vec<[u32; 3]> {
    points.iter().map(|p| p.to_array().map(f32::to_bits)).collect()
}

/// The furthest two shapes are apart at any vertex.
fn apart(a: &[Vec3], b: &[Vec3]) -> f32 {
    assert_eq!(a.len(), b.len());
    a.iter().zip(b).map(|(a, b)| a.distance(*b)).fold(0.0, f32::max)
}

/// A per-vertex list of the mesh in its level's own numbering, which is the
/// same however the mesh happened to be built.
fn ordered(mesh: &Mesh, list: &[Vec3]) -> Vec<Vec3> {
    match &mesh.levels {
        Some(levels) => levels.vertex.iter().map(|&v| list[v as usize]).collect(),
        None => list.to_vec(),
    }
}

fn shape(mesh: &Mesh) -> Vec<Vec3> {
    ordered(mesh, &mesh.positions)
}

fn offsets(mesh: &Mesh, id: u32) -> Vec<Vec3> {
    let held = mesh.layers.as_ref().and_then(|layers| layers.offsets(id)).expect("the layer is there");
    ordered(mesh, held)
}

/// Moves every vertex `to` gives a new place, as one stroke.
fn stroke(mesh: &mut Mesh, to: impl Fn(Vec3) -> Option<Vec3> + Sync) -> Option<StrokeDelta> {
    let leaves: Vec<u32> = (0..mesh.bvh.leaves.len() as u32).collect();
    mesh.begin_stroke();
    mesh.move_vertices(&leaves, |_, vertex| to(vertex.pos));
    mesh.positions_changed(&leaves);
    mesh.end_stroke(false)
}

/// Fine detail all over: every vertex pushed in or out along its own
/// direction from the middle.
fn fine_detail(p: Vec3) -> Option<Vec3> {
    Some(p + p.normalize() * 0.01 * (p.x * 40.0).sin() * (p.y * 31.0).cos())
}

/// A bump on the front of the ball.
fn bump(height: f32, radius: f32) -> impl Fn(Vec3) -> Option<Vec3> + Sync {
    move |p: Vec3| {
        let d = p.distance(Vec3::Z * 0.5) / radius;
        (d < 1.0).then(|| p + p.normalize() * height * (1.0 - d * d).powi(2))
    }
}

/// `mesh` with two layers sculpted on it: "Pores", fine detail at 60%, and
/// "Scar", a bump, switched off. Nothing is left recording.
fn with_layers(mut mesh: Mesh, detail: impl Fn(Vec3) -> Option<Vec3> + Sync) -> (Mesh, u32, u32) {
    let (pores, _) = mesh.add_layer(Some("Pores".into()));
    stroke(&mut mesh, detail);
    mesh.set_layer(pores, true, 60).unwrap();
    let (scar, _) = mesh.add_layer(Some("Scar".into()));
    stroke(&mut mesh, bump(0.03, 0.2));
    mesh.set_layer(scar, false, 100).unwrap();
    mesh.record_on_layer(None).unwrap();
    (mesh, pores, scar)
}

fn three_levels() -> Mesh {
    let mid = subdivided(&ball(), Split::Smooth).unwrap().0;
    subdivided(&mid, Split::Smooth).unwrap().0
}

/// A ball of three levels, on its finest, with the two layers.
fn layered() -> (Mesh, u32, u32) {
    with_layers(three_levels(), fine_detail)
}

fn to(mesh: &Mesh, level: usize) -> Mesh {
    stepped(mesh, level).unwrap().0
}

#[test]
fn a_round_trip_with_nothing_edited_gives_every_layer_back_to_the_bit() {
    let (top, pores, scar) = layered();
    let before = (bits(&shape(&top)), top.layer_rows(), bits(&offsets(&top, pores)), bits(&offsets(&top, scar)));
    assert_eq!(top.levels.as_ref().unwrap().layer_bytes(), 0, "on the finest level the mesh's layers are all there is");
    for path in [&[0, 2][..], &[1, 0, 1, 2], &[0, 0, 1, 1, 2], &[2, 2]] {
        let mut mesh = to(&top, path[0]);
        for &level in &path[1..] {
            // On the way: every layer is there, as it was, holding its share of this level.
            assert_eq!(mesh.layer_rows(), before.1, "{path:?}");
            let count = mesh.vertex_count();
            assert_eq!(bits(&offsets(&mesh, pores)), before.2[..count], "{path:?}");
            assert_eq!(bits(&offsets(&mesh, scar)), before.3[..count], "{path:?}");
            let levels = mesh.levels.as_ref().unwrap();
            assert_eq!(levels.layer_bytes() > 0, levels.current() < levels.top());
            mesh = to(&mesh, level);
        }
        let after = (bits(&shape(&mesh)), mesh.layer_rows(), bits(&offsets(&mesh, pores)), bits(&offsets(&mesh, scar)));
        assert!(after == before, "{path:?}");
    }
}

/// The model underneath turned and moved whole on the coarsest level: the
/// stack carries the finer levels round with it, and the layers go round too.
#[test]
fn a_coarse_turn_of_the_model_underneath_turns_the_layers_with_it() {
    let (top, pores, scar) = layered();
    let turn = Quat::from_axis_angle(Vec3::new(0.3, 1.0, 0.2).normalize(), 1.1);
    let moved = |p: Vec3| turn * p + Vec3::new(0.1, -0.05, 0.02);
    let mut low = to(&top, 0);
    stroke(&mut low, |p| Some(moved(p)));
    let mut up = to(&low, 2);

    let expected: Vec<Vec3> = shape(&top).iter().map(|&p| moved(p)).collect();
    assert!(apart(&shape(&up), &expected) < 2e-6, "the stack turned the finest level");
    for layer in [pores, scar] {
        let (was, now) = (offsets(&top, layer), offsets(&up, layer));
        let off = (0..was.len()).map(|v| now[v].distance(turn * was[v])).fold(0.0, f32::max);
        assert!(off < 2e-6, "every offset of both layers went round with the surface, on or off ({off})");
    }
    // So the layer switched off leaves the model underneath, turned, and none of itself.
    let mut flat = layered().0;
    flat.set_layer(pores, false, 60).unwrap();
    let expected: Vec<Vec3> = shape(&flat).iter().map(|&p| moved(p)).collect();
    up.set_layer(pores, false, 60).unwrap();
    let ghost = apart(&shape(&up), &expected);
    println!("a turn of 1.1 rad on the coarsest level: the layer switched off leaves {ghost:e} of itself");
    assert!(ghost < 4e-6, "{ghost}");
}

/// The furthest any vertex of `shape` is off the surface `of`: its distance
/// from that mesh's own vertex along that mesh's normal there. (A vertex that
/// has only slid along a surface has not changed the surface.)
fn off_surface(shape: &[Vec3], of: &Mesh) -> f32 {
    let (points, normals) = (ordered(of, &of.positions), ordered(of, &of.normals));
    (0..shape.len()).map(|v| (shape[v] - points[v]).dot(normals[v]).abs()).fold(0.0, f32::max)
}

/// A limb twisted, bent and pushed on the coarsest level under a layer of
/// detail that stands straight out of its skin: the same limb worked the same
/// way with the layer off from the start is, nearly, the surface that is left
/// when the layer is switched off afterwards.
///
/// Nearly, and the three cases say how nearly. The twist and the bend are
/// poses (they turn what they move, and the layer is turned with the
/// surface); the push is what a brush does (the layer is left standing as it
/// stood). After the twist a tenth of the ghost an unturned layer would
/// leave is left: the model's own fine detail, turned a little differently
/// with the layer on it than without. The bend is a hard one, across a limb
/// as thick as it is long: it stretches the outside of the bend and squeezes
/// the inside, which leans whatever stood there by up to half a radian. On
/// screen the detail leans; the layer is told it still stands straight at
/// full height; so with the layer off the model is left lower by the
/// difference where a bump was, a fifth of its height at the worst, and its
/// vertices to one side. The two figures printed are how far off the plain
/// limb's surface, and how far along it.
#[test]
fn a_coarse_twist_bend_or_push_under_a_detail_layer_leaves_little_of_it() {
    let limb = || {
        let mid = subdivided(&primitives::build(Primitive::Capsule, 1_500, 0.25), Split::Smooth).unwrap().0;
        subdivided(&mid, Split::Smooth).unwrap().0
    };
    let wall = primitives::build(Primitive::Capsule, 1_500, 0.25).bounds().max.y - 0.25;
    assert!(wall > 0.2, "the capsule has a straight wall to work on ({wall})");
    // Detail on the straight wall, standing out square to the limb's axis (Y).
    let detail = move |p: Vec3| {
        let out = Vec3::new(p.x, 0.0, p.z).normalize();
        let fade = (1.0 - (p.y / wall).powi(2)).max(0.0);
        (fade > 0.0).then(|| p + out * 0.01 * fade * (p.y * 60.0).sin() * (p.x * 50.0).cos())
    };
    let eased = move |y: f32| {
        let t = ((y / wall + 0.8) / 1.6).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    let twist = move |p: Vec3| Some(Quat::from_rotation_y(1.2 * eased(p.y)) * p);
    let bend = move |p: Vec3| Some(Quat::from_rotation_z(0.9 * eased(p.y)) * p);
    // The top half pulled sideways, as the Move brush pulls it with a wide radius.
    let push = move |p: Vec3| Some(p + Vec3::new(0.2, 0.0, 0.1) * eased(p.y));
    type Work<'a> = &'a (dyn Fn(Vec3) -> Option<Vec3> + Sync);
    let cases: [(&str, Work, Ride, f32); 3] =
        [("twist", &twist, Ride::Turned, 0.15), ("bend", &bend, Ride::Turned, 0.4), ("push", &push, Ride::Still, 0.4)];
    for (name, work, ride, most) in cases {
        let (top, pores, _) = with_layers(limb(), detail);
        let held = offsets(&top, pores);
        let mut low = to(&top, 0);
        low.layer_ride = ride;
        stroke(&mut low, work);
        let mut up = to(&low, 2);
        let worked = shape(&up);

        // The same with the layer off before the work.
        let (mut plain, plain_pores, _) = with_layers(limb(), detail);
        plain.set_layer(plain_pores, false, 60).unwrap();
        let mut plain_low = to(&plain, 0);
        stroke(&mut plain_low, work);
        let expected = to(&plain_low, 2);

        up.set_layer(pores, false, 60).unwrap();
        let (ghost, slid) = (off_surface(&shape(&up), &expected), apart(&shape(&up), &shape(&expected)));
        // What would have been left had the offsets stayed pointing where they did.
        let unturned: Vec<Vec3> = worked.iter().zip(&held).map(|(p, o)| *p - *o * 0.6).collect();
        let stale = off_surface(&unturned, &expected);
        println!(
            "a {name} of a limb under 6 mm of detail, layer off: {ghost:e} off the plain limb's surface, {slid:e} \
             along it ({stale:e} off it had the layer not turned)"
        );
        if ride == Ride::Turned {
            assert!(stale > 0.002, "the {name} was enough to show a ghost ({stale})");
            assert!(ghost < stale * most, "{name}: {ghost} against {stale}");
        } else {
            // A push turns nothing: the measure is the layer's own height.
            assert!(ghost < 0.006 * most, "{name}: {ghost}");
        }
    }
}

#[test]
fn a_stroke_recorded_on_a_coarse_level_is_the_layers_on_every_level() {
    for percent in [100u8, 40] {
        let (top, pores, _) = layered();
        let was = shape(&top);
        let mut low = to(&top, 0);
        let (swelling, _) = low.add_layer(Some("Swelling".into()));
        low.set_layer(swelling, true, percent);
        assert!(low.set_layer(swelling, true, percent).is_none(), "strength is fixed below the finest level");
        if percent < 100 {
            // Set it where it can be set: on the finest level, then come back down.
            let mut up = to(&low, 2);
            up.set_layer(swelling, true, percent).unwrap();
            low = to(&up, 0);
        }
        stroke(&mut low, bump(0.08, 0.35));
        let coarse_count = offsets(&low, swelling).iter().filter(|o| **o != Vec3::ZERO).count();
        let mut up = to(&low, 2);

        // The shape is the stack's own, to the bit: the same model with no layers at all steps to the same.
        let mut twin = layered().0;
        twin.bake_all_layers().unwrap();
        let mut twin_low = to(&twin, 0);
        stroke(&mut twin_low, bump(0.08, 0.35));
        assert_eq!(bits(&shape(&up)), bits(&shape(&to(&twin_low, 2))), "{percent}%");

        // The other layer was not touched, and the model underneath did not move.
        assert_eq!(bits(&offsets(&up, pores)), bits(&offsets(&top, pores)), "{percent}%");
        let fine_count = offsets(&up, swelling).iter().filter(|o| **o != Vec3::ZERO).count();
        assert!(fine_count > coarse_count * 8, "the layer has the finer vertices' share too ({coarse_count}, {fine_count})");
        let swollen = shape(&up);
        up.set_layer(swelling, false, percent).unwrap();
        let left = apart(&shape(&up), &was);
        println!("recorded at {percent}% on the coarsest level, switched off on the finest: {left:e} left");
        assert!(left < 1e-6, "{left}");
        // And all the way up it stands as much higher as it was recorded lower.
        up.set_layer(swelling, true, 100).unwrap();
        let (v, rose) = (0..was.len()).map(|v| (v, swollen[v].distance(was[v]))).max_by(|a, b| a.1.total_cmp(&b.1)).unwrap();
        let ratio = shape(&up)[v].distance(was[v]) / rose;
        assert!((ratio - 100.0 / percent as f32).abs() < 1e-3, "{ratio}");
    }
}

#[test]
fn subdivide_takes_every_layer_through_the_same_split() {
    for split in [Split::Smooth, Split::Flat] {
        let (base, pores, scar) = with_layers(ball(), fine_detail);
        let (fine, _) = subdivided(&base, split).unwrap();
        assert_eq!(fine.layer_rows(), base.layer_rows());

        // The shape is the stack's own: the same model with its layers baked in subdivides to the same.
        let mut baked = with_layers(ball(), fine_detail).0;
        baked.bake_all_layers().unwrap();
        assert_eq!(bits(&shape(&fine)), bits(&shape(&subdivided(&baked, split).unwrap().0)));

        // Each layer's part of the new level is its own: turned down or up
        // after the split, the shape is the one from turning it before.
        let after = |on: bool, percent: u8, scar_on: bool| {
            let mut fine = subdivided(&with_layers(ball(), fine_detail).0, split).unwrap().0;
            fine.set_layer(pores, on, percent);
            fine.set_layer(scar, scar_on, 100);
            shape(&fine)
        };
        let before = |on: bool, percent: u8, scar_on: bool| {
            let mut base = with_layers(ball(), fine_detail).0;
            base.set_layer(pores, on, percent);
            base.set_layer(scar, scar_on, 100);
            shape(&subdivided(&base, split).unwrap().0)
        };
        for (on, percent, scar_on) in [(true, 0, false), (false, 60, true), (true, 100, true)] {
            let off = apart(&after(on, percent, scar_on), &before(on, percent, scar_on));
            println!("{split:?}: layers set after the split or before it, {off:e} apart");
            assert!(off < 5e-7, "{split:?} {on} {percent} {scar_on}: {off}");
        }
    }
}

#[test]
fn deleting_levels_keeps_the_layers_whole() {
    let (top, pores, scar) = layered();
    let finest = (bits(&offsets(&top, pores)), bits(&offsets(&top, scar)));

    // Delete Lower on the middle level: the levels above keep their numbers, and stepping up is exact.
    let mut mid = to(&top, 1);
    mid.levels = without(&mid, true);
    assert_eq!(mid.level_count(), 2);
    let up = to(&mid, 1);
    assert!((bits(&offsets(&up, pores)), bits(&offsets(&up, scar))) == finest);
    assert_eq!(bits(&shape(&up)), bits(&shape(&top)));

    // Delete Higher on the middle level: it is the finest now, and its layers are whole layers.
    let mut mid = to(&top, 1);
    assert!(mid.layer_strength_refusal().is_some() && mid.set_layer(pores, true, 0).is_none());
    mid.levels = without(&mid, false);
    assert!(mid.levels.as_ref().unwrap().layer_bytes() == 0 && mid.layer_strength_refusal().is_none());
    let count = mid.vertex_count();
    assert_eq!(bits(&offsets(&mid, pores)), finest.0[..count]);
    let (with, held) = (shape(&mid), offsets(&mid, pores));
    mid.set_layer(pores, true, 0).unwrap();
    let under: Vec<Vec3> = with.iter().zip(&held).map(|(p, o)| *p - *o * 0.6).collect();
    assert!(apart(&shape(&mid), &under) < 2e-7);
    // Down and up again from there, with the layer at nothing: still exact.
    let again = to(&to(&mid, 0), 1);
    assert_eq!(bits(&offsets(&again, pores)), bits(&offsets(&mid, pores)));

    // Down to one level, which is a mesh without levels: the layers stay on it.
    let mut low = to(&top, 0);
    low.levels = without(&low, false);
    assert!(low.levels.is_none() && low.layer_rows().len() == 2);
    low.set_layer(scar, true, 100).expect("a mesh without levels has nothing to step up to first");
}

#[test]
fn below_the_finest_level_a_layer_can_be_made_and_baked_but_not_turned_down() {
    let (top, pores, scar) = layered();
    let mut low = to(&top, 0);
    assert!(low.layer_strength_refusal().unwrap().contains("Step up to the finest level"));
    assert!(low.drag_layer(pores).is_none() && low.remove_layer(pores, false).is_none());
    assert!(low.rename_layer(scar, "Old scar").is_some() && low.record_on_layer(Some(scar)).is_some());

    // Bake moves nothing, here or above; the layer is simply part of the model from now on.
    let before = bits(&low.positions);
    let mut step = low.remove_layer(pores, true).expect("bake is allowed on any level");
    assert_eq!(bits(&low.positions), before);
    let up = to(&low, 2);
    assert_eq!(bits(&shape(&up)), bits(&shape(&top)));
    assert_eq!(up.layer_rows().iter().map(|row| row.name.as_str()).collect::<Vec<_>>(), ["Old scar"]);
    assert_eq!(bits(&offsets(&up, scar)), bits(&offsets(&top, scar)));
    // Undo of the bake, then up: the layer is back with every bit of its finest level.
    step.swap(&mut low);
    assert_eq!(bits(&offsets(&to(&low, 2), pores)), bits(&offsets(&top, pores)));

    // A layer made down here and never sculpted on is an empty layer up there.
    let (new, _) = low.add_layer(None);
    let up = to(&low, 2);
    assert!(offsets(&up, new).iter().all(|o| *o == Vec3::ZERO) && up.layer_rows().len() == 3);
    assert_eq!(bits(&shape(&up)), bits(&shape(&top)));
}

/// Index of each vertex's mirror image across X, in the level's numbering.
fn twins(points: &[Vec3]) -> Vec<usize> {
    let key = |p: Vec3| p.to_array().map(|c| (c * 2e5).round() as i64);
    let at: std::collections::HashMap<[i64; 3], usize> = points.iter().enumerate().map(|(v, &p)| (key(p), v)).collect();
    points.iter().map(|&p| at[&key(p * Vec3::new(-1.0, 1.0, 1.0))]).collect()
}

#[test]
fn mirrored_work_on_a_coarse_level_leaves_mirrored_layers_on_the_finest() {
    let top = three_levels();
    let twin = twins(&shape(&top));
    let mirrored = |p: Vec3| p * Vec3::new(-1.0, 1.0, 1.0);
    let lopsided = |list: &[Vec3]| (0..list.len()).map(|v| list[v].distance(mirrored(list[twin[v]]))).fold(0.0, f32::max);
    // Detail that is the same on both sides, a mirrored bend underneath, and a mirrored stroke recorded.
    let even = |p: Vec3| Some(p + p.normalize() * 0.01 * (p.x.abs() * 40.0).sin() * (p.y * 31.0).cos());
    let (top, pores, _) = with_layers(top, even);
    assert!(lopsided(&offsets(&top, pores)) < 1e-7);
    let mut low = to(&top, 0);
    stroke(&mut low, |p| Some(Quat::from_rotation_z(-0.8 * p.x.signum() * (p.x.abs() * 4.0).min(1.0)) * p));
    let (ears, _) = low.add_layer(None);
    stroke(&mut low, |p| (p.x.abs() > 0.3).then(|| p + Vec3::new(p.x.signum() * 0.05, 0.0, 0.0)));
    let up = to(&low, 2);
    let (shape_off, turned, recorded) = (lopsided(&shape(&up)), lopsided(&offsets(&up, pores)), lopsided(&offsets(&up, ears)));
    println!("mirrored coarse work: shape {shape_off:e}, turned layer {turned:e}, recorded layer {recorded:e} from mirrored");
    assert!(shape_off < 2e-6 && turned < 2e-6 && recorded < 2e-6);
}

/// `cargo test timing_of_level_changes_with_layers -- --ignored --nocapture`
#[test]
#[ignore]
fn timing_of_level_changes_with_layers() {
    let time = |what: &str, run: &mut dyn FnMut() -> Mesh| {
        let started = std::time::Instant::now();
        let mesh = run();
        println!("{what}: {:.0} ms", started.elapsed().as_secs_f64() * 1000.0);
        mesh
    };
    let mut top = primitives::build(Primitive::Sphere, 5_000, 0.5);
    for _ in 0..4 {
        top = subdivided(&top, Split::Smooth).unwrap().0;
    }
    println!("{} triangles on the finest of {} levels", top.triangle_count(), top.level_count());
    let plain_low = time("down three levels, no layers", &mut || to(&top, 1));
    time("up again, nothing edited, no layers", &mut || to(&plain_low, 4));
    let (top, _, _) = with_layers(top, fine_detail);
    let mut low = time("down three levels, two layers", &mut || to(&top, 1));
    time("up again, nothing edited, two layers", &mut || to(&low, 4));
    low.layer_ride = Ride::Still;
    stroke(&mut low, |p| Some(p + Vec3::X * 0.2 * p.y.max(0.0)));
    let up = time("up after a push underneath, two layers", &mut || to(&low, 4));
    let mut low = to(&up, 1);
    let (form, _) = low.add_layer(None);
    stroke(&mut low, bump(0.05, 0.3));
    let up = time("up after a stroke recorded on a third layer", &mut || to(&low, 4));
    assert!(up.layers.as_ref().unwrap().offsets(form).is_some());
    time("Subdivide with three layers", &mut || subdivided(&up, Split::Smooth).unwrap().0);
}
