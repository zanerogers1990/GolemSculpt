//! Paint across the Blender link.
//!
//! **Going**, an object's paint goes as Blender colour attributes on its
//! points (`io/link.rs` has the format): "GolemSculpt Paint", the colour seen
//! here (the layers over the Base Colour), which Blender shows and renders;
//! then each paint layer that has colour, under the layer's name, with its
//! coverage as alpha; and for each material the paint uses, a number
//! attribute under the material's name holding how much of it is at each
//! point (for Blender's shader nodes: an Attribute node of that name).
//!
//! **Coming back**, Blender sends "GolemSculpt Paint" (or, on an object that
//! has none, its active colour attribute: a model painted in Blender, or
//! brought into it with colours). What happens to it:
//!
//! - An object that comes back as itself keeps its own paint layers here:
//!   they are carried onto its new shape point by point, as for any rebuild
//!   (`groups::taking_over`). Blender's colour is then held against the
//!   colour those layers show on the new shape, and where they differ, which
//!   is where it was painted in Blender, it goes on as a new top layer,
//!   "From Blender", covering exactly those points. Where nothing was painted
//!   in Blender, no layer is added. A layer edited in Blender under its own
//!   attribute is not read back: paint on "GolemSculpt Paint" there.
//! - A new object from Blender comes with its colour as its one paint layer,
//!   "From Blender".
//!
//! "Differ" allows two steps in 255 per channel, for the rounding of the way
//! there and back.

use std::borrow::Cow;

use crate::io::painted::{ExportPaint, Painted};
use crate::io::FROM_BLENDER;
use crate::mesh::Mesh;
use crate::mesh::paint::{Channels, MOST_PAINT_LAYERS, PaintLayer, PaintRow, PaintStack, Rgba8};

/// How far apart, in steps of 255 per channel, two colours may be and still be the same.
const SAME: u8 = 2;

/// What became of the colour an object came back with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Came {
    /// It had none, or none that differs from what its layers show.
    Nothing,
    /// A "From Blender" layer was added, covering this many points.
    Layer(usize),
    /// It differs, but the object has as many paint layers as it can have.
    Full,
}

/// Takes the colour a mesh built from Blender came with off it, to be held
/// against the paint it is about to be given (see [`came_back`]).
pub(super) fn take_colour(mesh: &mut Mesh) -> Option<Vec<Rgba8>> {
    let stack = mesh.paint_stack().map(Cow::into_owned);
    mesh.set_paint(None);
    stack?.layers.into_iter().next()?.data.colour
}

/// `mesh`, back from Blender with `theirs` (one colour per point, in the
/// mesh's order) and its own paint layers already carried onto it: a "From
/// Blender" layer on top wherever Blender's colour is not the colour seen
/// here (see the module notes).
pub(super) fn came_back(mesh: &mut Mesh, theirs: Option<Vec<Rgba8>>, paint: &ExportPaint) -> Came {
    let Some(theirs) = theirs.filter(|c| c.len() == mesh.vertex_count()) else { return Came::Nothing };
    let ours: Cow<[[u8; 3]]> = match Painted::of(mesh, paint) {
        Some(painted) => Cow::Owned(painted.seen),
        None => Cow::Owned(vec![paint.base; mesh.vertex_count()]),
    };
    let differs = |c: &Rgba8, o: &[u8; 3]| (0..3).any(|k| c.0[k].abs_diff(o[k]) > SAME);
    let colour: Vec<Rgba8> =
        theirs.iter().zip(ours.iter()).map(|(c, o)| if differs(c, o) { Rgba8([c.0[0], c.0[1], c.0[2], 255]) } else { Rgba8([0; 4]) }).collect();
    let covered = colour.iter().filter(|c| c.0[3] > 0).count();
    if covered == 0 {
        return Came::Nothing;
    }
    let mut stack = mesh.paint_stack().map_or_else(|| PaintStack::new(mesh.vertex_count()), Cow::into_owned);
    if stack.layers.len() >= MOST_PAINT_LAYERS {
        return Came::Full;
    }
    let id = stack.next_id;
    let row = PaintRow::new(id, stack.free_name(FROM_BLENDER));
    stack.layers.push(PaintLayer { row, data: Channels { colour: Some(colour), ..Channels::default() } });
    (stack.next_id, stack.active) = (id + 1, Some(id));
    mesh.set_paint(Some(stack));
    Came::Layer(covered)
}

/// The words a status line adds for what became of an object's colour.
pub(super) fn said(name: &str, came: Came) -> Option<String> {
    match came {
        Came::Nothing => None,
        Came::Layer(_) => Some(format!("What was painted on \"{name}\" in Blender is its new paint layer \"{FROM_BLENDER}\".")),
        Came::Full => Some(format!(
            "\"{name}\" was painted in Blender, but has as many paint layers as an object can: merge some and send it again."
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::paint::palette::ScenePaint;
    use crate::mesh::primitives::{self, Primitive};

    fn painted_ball() -> Mesh {
        let mut mesh = primitives::build(Primitive::Sphere, 1_000, 0.5);
        let n = mesh.vertex_count();
        let p = mesh.positions.clone();
        let colour = (0..n).map(|v| if p[v].y > 0.0 { Rgba8([30, 60, 200, 255]) } else { Rgba8([0; 4]) }).collect();
        let mut stack = PaintStack::new(n);
        stack.layers.push(PaintLayer { row: PaintRow::new(1, "Blue".into()), data: Channels { colour: Some(colour), ..Channels::default() } });
        (stack.next_id, stack.active) = (2, Some(1));
        mesh.set_paint(Some(stack));
        mesh
    }

    #[test]
    fn what_blender_did_not_change_adds_no_layer_and_what_it_painted_does() {
        let paint = ExportPaint::of([0.5, 0.5, 0.5], &ScenePaint::default());
        let mut mesh = painted_ball();
        let seen = Painted::of(&mesh, &paint).unwrap().seen;
        let sent: Vec<Rgba8> = seen.iter().map(|c| Rgba8([c[0], c[1], c[2], 255])).collect();
        let before = mesh.paint_stack().unwrap().into_owned();
        assert_eq!(came_back(&mut mesh, Some(sent.clone()), &paint), Came::Nothing);
        assert!(mesh.paint_stack().unwrap().as_ref() == &before, "as it was");

        // A step of rounding is not painting.
        let rounded: Vec<Rgba8> = sent.iter().map(|c| Rgba8([c.0[0].saturating_add(1), c.0[1], c.0[2].saturating_sub(2), 0])).collect();
        assert_eq!(came_back(&mut mesh, Some(rounded), &paint), Came::Nothing);

        // Ten points painted yellow in Blender.
        let mut theirs = sent.clone();
        for c in theirs.iter_mut().step_by(37).take(10) {
            *c = Rgba8([250, 240, 0, 255]);
        }
        assert_eq!(came_back(&mut mesh, Some(theirs.clone()), &paint), Came::Layer(10));
        let stack = mesh.paint_stack().unwrap();
        let names: Vec<&str> = stack.layers.iter().map(|l| l.row.name.as_str()).collect();
        assert_eq!(names, ["Blue", FROM_BLENDER]);
        assert_eq!(stack.active, Some(stack.layers[1].row.id));
        assert!(stack.layers[0] == before.layers[0], "the layers under it are as they were");
        // What is seen now is what Blender showed.
        let now = Painted::of(&mesh, &paint).unwrap().seen;
        assert!(now.iter().zip(&theirs).all(|(n, t)| n[..] == t.0[..3]));
        // A second time, another layer of its own.
        let mut again = theirs.clone();
        again[1] = Rgba8([0, 0, 0, 255]);
        assert_eq!(came_back(&mut mesh, Some(again), &paint), Came::Layer(1));
        assert_eq!(mesh.paint_rows().last().unwrap().name, "From Blender 2");
    }

    #[test]
    fn colour_on_an_unpainted_object_is_held_against_the_base_colour() {
        let paint = ExportPaint::of([0.5, 0.5, 0.5], &ScenePaint::default());
        let mut mesh = primitives::build(Primitive::Sphere, 500, 0.5);
        let n = mesh.vertex_count();
        let base = Rgba8([128, 128, 128, 255]);
        assert_eq!(came_back(&mut mesh, Some(vec![base; n]), &paint), Came::Nothing);
        assert!(!mesh.has_paint());
        let mut theirs = vec![base; n];
        theirs[3] = Rgba8([255, 0, 0, 255]);
        assert_eq!(came_back(&mut mesh, Some(theirs), &paint), Came::Layer(1));
        assert_eq!(mesh.paint_rows()[0].name, FROM_BLENDER);
        assert_eq!(came_back(&mut mesh, None, &paint), Came::Nothing);
        assert_eq!(came_back(&mut mesh, Some(vec![base; 3]), &paint), Came::Nothing, "colours for another mesh");
    }

    #[test]
    fn the_colour_a_mesh_came_with_is_taken_off_it() {
        let mut mesh = painted_ball();
        let want = mesh.paint_stack().unwrap().layers[0].data.colour.clone();
        assert_eq!(take_colour(&mut mesh), want);
        assert!(!mesh.has_paint());
        assert_eq!(take_colour(&mut mesh), None);
    }

    // ------------------------------------------- against a real Blender
    //
    // These two need a Blender, so they are left out of the ordinary run; the
    // commands are in tests/blender_link/README.md ("Paint"). The first writes
    // a package with paint and what Blender should find in it; Blender's side
    // (tests/blender_link/check_paint.py) takes it in, checks every byte,
    // paints ten points yellow and sends it back; the second takes that answer.

    use crate::io::{LinkObject, LinkPaint};
    use crate::mesh::paint::Mat8;
    use crate::mesh::paint::palette::PaletteEntry;

    /// The folder the check works in.
    fn check_folder() -> std::path::PathBuf {
        let folder = std::env::var_os("GOLEMSCULPT_PAINT_CHECK").expect("set GOLEMSCULPT_PAINT_CHECK to a scratch folder");
        let folder = std::path::PathBuf::from(folder);
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    /// The blue ball with a second, half-covered layer of its own and Metal - Rust on its right.
    fn checked_ball() -> (Mesh, ExportPaint) {
        let mut mesh = painted_ball();
        let n = mesh.vertex_count();
        let p = mesh.positions.clone();
        let mut stack = mesh.paint_stack().unwrap().into_owned();
        let dots = (0..n).map(|v| Rgba8([(v * 3 % 256) as u8, 200, (v % 7 * 30) as u8, (v % 3 * 100) as u8])).collect();
        let rust = (0..n).map(|v| if p[v].x > 0.1 { Mat8([4, (128 + v % 128) as u8]) } else { Mat8([0, 0]) }).collect();
        let data = Channels { colour: Some(dots), material: Some(rust), ..Channels::default() };
        stack.layers.push(PaintLayer { row: PaintRow::new(2, "Dots".into()), data });
        (stack.next_id, stack.table) = (3, vec![4]);
        mesh.set_paint(Some(stack));
        let rust = PaletteEntry { slot: 4, name: "Metal - Rust".into(), swatch: [0.6, 0.2, 0.1], ..PaletteEntry::default() };
        (mesh, ExportPaint::of([0.5, 0.5, 0.5], &ScenePaint { palette: vec![rust], ..ScenePaint::default() }))
    }

    #[test]
    #[ignore = "writes the package tests/blender_link/check_paint.py checks in Blender"]
    fn blender_paint_check_package() {
        let folder = check_folder();
        let (mesh, paint) = checked_ball();
        let (mut ball, _) = LinkObject::placed(&mesh, glam::Mat4::IDENTITY);
        (ball.name, ball.reference, ball.mark) = ("Painted Ball".into(), "check:1".into(), "s-check-1".into());
        ball.paint = LinkPaint::of(&mesh, &paint);
        let sent = ball.paint.clone().unwrap();
        crate::io::write_link_package(&folder.join("from_sculpt.gslink"), "golemsculpt", true, &[ball]).unwrap();
        let bytes = |c: &[[u8; 4]]| c.iter().map(|c| c.to_vec()).collect::<Vec<_>>();
        let expect = serde_json::json!({
            "vertices": mesh.vertex_count(),
            "composite": { "attribute": sent.composite_name, "colours": bytes(&sent.composite) },
            "layers": sent.layers.iter().map(|l| serde_json::json!({ "attribute": l.attribute, "colours": bytes(&l.colours) })).collect::<Vec<_>>(),
            "materials": sent.materials.iter().map(|m| serde_json::json!({ "attribute": m.attribute, "weights": m.weights })).collect::<Vec<_>>(),
        });
        std::fs::write(folder.join("expect.json"), expect.to_string()).unwrap();
    }

    #[test]
    #[ignore = "takes in what tests/blender_link/check_paint.py sent back from Blender"]
    fn blender_paint_check_answer() {
        let folder = check_folder();
        let (old, paint) = checked_ball();
        let package = crate::io::read_link_package(&folder.join("from_blender.gslink")).unwrap();
        assert!(!package.paint_lost);
        let entry = package.objects.into_iter().next().unwrap();
        assert_eq!((entry.name.as_str(), entry.same_as.as_str()), ("Painted Ball", ""), "painted there, so changed");
        let came = entry.paint.clone().expect("Blender sent its colour back");
        assert_eq!(came.composite_name, "GolemSculpt Paint");
        // As it went, but for the ten points Blender painted yellow.
        let sent = LinkPaint::of(&old, &paint).unwrap().composite;
        let yellow: Vec<usize> = (0..old.vertex_count()).step_by(37).take(10).collect();
        for (v, (c, s)) in came.composite.iter().zip(&sent).enumerate() {
            let want = if yellow.contains(&v) { [250, 240, 0, 255] } else { *s };
            assert_eq!(*c, want, "point {v}");
        }
        // Taken in as the app takes it: its own layers carried onto the shape Blender sent, and a layer for the yellow.
        let (mut mesh, _) = entry.into_mesh(glam::Mat4::IDENTITY);
        let theirs = take_colour(&mut mesh);
        super::super::super::groups::follow(&old, &mut mesh, crate::mesh::groups::Fit::Whole);
        assert_eq!(came_back(&mut mesh, theirs, &paint), Came::Layer(10));
        let names: Vec<String> = mesh.paint_rows().into_iter().map(|r| r.name).collect();
        assert_eq!(names, ["Blue", "Dots", FROM_BLENDER]);
        let seen = Painted::of(&mesh, &paint).unwrap().seen;
        let yellow_now = seen.iter().filter(|c| **c == [250, 240, 0]).count();
        assert_eq!(yellow_now, 10);
    }
}
