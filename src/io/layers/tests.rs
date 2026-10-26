//! Sculpt layers through a saved file and back.

use std::collections::HashMap;

use glam::Quat;
use sculpt_multires::Split;

use super::*;
use crate::io::{SavedObject, load_scene, save_scene};
use crate::mesh::levels::{stepped, subdivided};
use crate::mesh::primitives::{self, Primitive};
use crate::mesh::{Mesh, StrokeDelta};

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("sculpt_io_layers_tests");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn object(mesh: usize) -> SavedObject {
    SavedObject {
        name: format!("Object {mesh}"),
        visible: true,
        mesh,
        position: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    }
}

/// Moves every vertex `to` gives a new place, as one stroke.
fn stroke(mesh: &mut Mesh, to: impl Fn(Vec3) -> Option<Vec3> + Sync) -> Option<StrokeDelta> {
    let leaves: Vec<u32> = (0..mesh.bvh.leaves.len() as u32).collect();
    mesh.begin_stroke();
    mesh.move_vertices(&leaves, |_, vertex| to(vertex.pos));
    mesh.positions_changed(&leaves);
    mesh.end_stroke(false)
}

/// `mesh` with three layers sculpted on it: fine detail at 60%, a bump that
/// is switched off, and a swelling that is still recording.
fn layered(mut mesh: Mesh) -> Mesh {
    let (pores, _) = mesh.add_layer(Some("Pores".into()));
    stroke(&mut mesh, |p| Some(p + p.normalize() * 0.01 * (p.x * 40.0).sin() * (p.y * 31.0).cos()));
    mesh.set_layer(pores, true, 60).unwrap();
    let (scar, _) = mesh.add_layer(Some("Scar \u{2014} left cheek".into()));
    stroke(&mut mesh, |p| (p.z > 0.4).then(|| p * 1.03));
    mesh.set_layer(scar, false, 100).unwrap();
    mesh.add_layer(None);
    stroke(&mut mesh, |p| (p.y > 0.1).then(|| p + Vec3::Y * 0.02 * p.y));
    mesh
}

fn ball() -> Mesh {
    primitives::build(Primitive::Sphere, 1_500, 0.5)
}

fn bits(p: Vec3) -> [u32; 3] {
    p.to_array().map(f32::to_bits)
}

/// Everything layers hold, however the mesh happens to be numbered: the
/// rows, and for each vertex (named by where it is) every layer's offset.
fn held(mesh: &Mesh) -> (Vec<crate::mesh::layers::LayerRow>, HashMap<[u32; 3], Vec<[u32; 3]>>) {
    let rows = mesh.layer_rows();
    let layers = mesh.layers.as_ref().expect("the mesh has layers");
    let lists: Vec<&[Vec3]> = rows.iter().map(|row| layers.offsets(row.id).unwrap()).collect();
    let at = |v: usize| (bits(mesh.positions[v]), lists.iter().map(|list| bits(list[v])).collect());
    let by_place: HashMap<_, _> = (0..mesh.vertex_count()).map(at).collect();
    assert_eq!(by_place.len(), mesh.vertex_count(), "no two vertices in one place");
    (rows, by_place)
}

fn reopened(mesh: &Mesh, name: &str) -> (Mesh, Lost) {
    let file = temp(name);
    save_scene(&[mesh], &[object(0)], 0, &file).unwrap();
    let mut opened = load_scene(&file).unwrap();
    (opened.meshes.remove(0), opened.lost)
}

#[test]
fn layers_are_saved_and_come_back_to_the_bit() {
    let mut mesh = layered(ball());
    mesh.hide(&(0..mesh.triangle_count() as u32).filter(|t| t % 7 == 0).collect::<Vec<_>>());
    let (mut back, lost) = reopened(&mesh, "layers.sclp");
    assert_eq!(lost, Lost::default());
    assert!(held(&back) == held(&mesh), "every row and every offset of every layer");
    assert_eq!(back.layer_rows()[2].recording, true, "the layer that was recording still is");
    assert_eq!(back.hidden_triangle_count(), mesh.hidden_triangle_count());
    // They are working layers: the same change of strength gives the same shape.
    let pores = mesh.layer_rows()[0].id;
    mesh.set_layer(pores, true, 0).unwrap();
    back.set_layer(pores, true, 0).unwrap();
    assert!(held(&back) == held(&mesh));
    // And a new layer is numbered on from where the saved ones stopped, deleted ones included.
    let last = mesh.layer_rows()[2].id;
    mesh.remove_layer(last, true).unwrap();
    let (mut back, _) = reopened(&mesh, "layers-less-one.sclp");
    assert_eq!(back.add_layer(None).0, last + 1);
}

#[test]
fn a_mesh_without_layers_is_written_as_it_always_was() {
    let file = temp("plain.sclp");
    save_scene(&[&ball()], &[object(0)], 0, &file).unwrap();
    let bytes = std::fs::read(&file).unwrap();
    assert!(!bytes.windows(4).any(|tag| tag == b"LAYR"), "no layers, no block");
    let opened = load_scene(&file).unwrap();
    assert!(opened.meshes[0].layers.is_none() && opened.lost == Lost::default() && opened.lost.note().is_none());
    // A mesh whose layers were all baked is a mesh without layers.
    let mut baked = layered(ball());
    baked.bake_all_layers().unwrap();
    let (back, lost) = reopened(&baked, "baked.sclp");
    assert!(back.layers.is_none() && lost == Lost::default());
}

/// Saved on a level below the finest, each layer's finest offsets are in the
/// file too: stepping up after opening gives what stepping up before saving
/// would have.
#[test]
fn layers_and_levels_are_saved_together_on_any_level() {
    let top = subdivided(&subdivided(&ball(), Split::Smooth).unwrap().0, Split::Smooth).unwrap().0;
    let top = layered(top);
    for level in 0..3 {
        let mesh = if level == 2 { layered(top_again()) } else { stepped(&top, level).unwrap().0 };
        let (back, lost) = reopened(&mesh, &format!("level{level}.sclp"));
        assert_eq!(lost, Lost::default());
        assert!(held(&back) == held(&mesh), "level {level}");
        let (was, now) = (mesh.levels.as_ref().unwrap(), back.levels.as_ref().expect("the levels came back"));
        assert_eq!((now.current(), now.print(&back)), (level, was.print(&mesh)));
        for to in 0..3 {
            let (expected, got) = (stepped(&mesh, to).unwrap().0, stepped(&back, to).unwrap().0);
            assert!(held(&got) == held(&expected), "saved on level {level}, stepped to {to}");
        }
    }

    fn top_again() -> Mesh {
        subdivided(&subdivided(&ball(), Split::Smooth).unwrap().0, Split::Smooth).unwrap().0
    }
}

/// Where the body of a block starts in a saved file, and how long it is.
fn block(bytes: &[u8], tag: &[u8; 4]) -> (usize, usize) {
    let at = bytes.windows(4).position(|found| found == tag).expect("the file has the block");
    (at + 12, u64::from_le_bytes(bytes[at + 4..at + 12].try_into().unwrap()) as usize)
}

#[test]
fn a_layers_block_that_cannot_be_read_costs_the_layers_and_never_the_shape() {
    let mesh = stepped(&layered(subdivided(&ball(), Split::Smooth).unwrap().0), 0).unwrap().0;
    let file = temp("damaged.sclp");
    save_scene(&[&mesh], &[object(0)], 0, &file).unwrap();
    let whole = std::fs::read(&file).unwrap();
    let (body, length) = block(&whole, b"LAYR");
    let first = body + HEAD;
    let name = u16::from_le_bytes([whole[first + 6], whole[first + 7]]) as usize;
    type Damage = Box<dyn Fn(&mut Vec<u8>)>;
    let damages: [(&str, Damage); 6] = [
        ("a layout this version does not know", Box::new(move |b| b[body + 4] = 9)),
        ("more layers than there are", Box::new(move |b| b[body + 8] += 1)),
        ("fewer layers than there are", Box::new(move |b| b[body + 8] -= 1)),
        ("a vertex count that is not the mesh's", Box::new(move |b| b[body + 20] ^= 1)),
        ("a name that is not text", Box::new(move |b| b[first + ROW] = 0xff)),
        ("an offset that is not a number", Box::new(move |b| b[first + ROW + name + 2..][..2].fill(0xff))),
    ];
    let shape = |mesh: &Mesh| {
        let mut points: Vec<[u32; 3]> = mesh.positions.iter().map(|&p| bits(p)).collect();
        points.sort_unstable();
        points
    };
    for (what, damage) in damages {
        let mut bytes = whole.clone();
        damage(&mut bytes);
        assert!(bytes[body..body + length] != whole[body..body + length], "{what}: the block was changed");
        let damaged = temp("damaged-now.sclp");
        std::fs::write(&damaged, &bytes).unwrap();
        let mut opened = load_scene(&damaged).unwrap_or_else(|why| panic!("{what}: the file still opens ({why})"));
        let mut back = opened.meshes.remove(0);
        assert!(back.layers.is_none(), "{what}: the layers are left off");
        assert_eq!(shape(&back), shape(&mesh), "{what}: the shape is the one saved, with the layers in it");
        assert_eq!(opened.lost, Lost { levels: 0, layers: 1, groups: 0, paint: 0, uvs: 0 }, "{what}");
        assert!(back.levels.is_some(), "{what}: the levels are another block, and came back");
        // The mesh is whole and works: it steps up to what the saved one steps up to...
        let up = stepped(&back, 1).unwrap_or_else(|why| panic!("{what}: {why}")).0;
        assert!(up.layers.is_none() && shape(&up) == shape(&stepped(&mesh, 1).unwrap().0), "{what}");
        // ...and takes a new layer, under a number none of the lost ones went by.
        assert_eq!(back.add_layer(None).0, 4, "{what}");
        let up = stepped(&back, 1).unwrap().0;
        assert!(up.layer_rows().len() == 1 && shape(&up) == shape(&stepped(&mesh, 1).unwrap().0), "{what}");
    }
    let said = Lost { levels: 0, layers: 1, groups: 0, paint: 0, uvs: 0 }.note().unwrap();
    assert!(said.starts_with("The sculpt layers saved with 1 object could not be read") && said.contains("the shape is as it was saved"));
    let both = Lost { levels: 2, layers: 1, groups: 0, paint: 0, uvs: 0 }.note().unwrap();
    assert!(both.starts_with("The subdivision levels saved with 2 objects") && both.contains(" The sculpt layers saved with 1 object"));
    // Untouched, it opens with its layers: the damage above is what cost them.
    let sound = load_scene(&file).unwrap();
    assert!(sound.meshes[0].layers.is_some() && sound.lost == Lost::default());
}

#[test]
fn the_length_written_is_the_length_of_what_is_written() {
    let mesh = layered(ball());
    let mut saved = mesh.layers.as_ref().unwrap().saved(None);
    // A name too long for its length to say is cut where a letter ends.
    saved.layers[1].name = "\u{e9}".repeat(40_000);
    // An offset that is not a number is written as no offset.
    saved.layers[0].offsets[5] = Vec3::new(f32::NAN, 1.0, 2.0);
    let mut body = Vec::new();
    write(&mut body, 3, &saved).unwrap();
    assert_eq!(body.len(), length(&saved));
    assert_eq!(u32::from_le_bytes(body[..4].try_into().unwrap()), 3);
    let back = read(&mut &body[4..], body.len() as u64, mesh.vertex_count()).expect("what was written reads back");
    assert_eq!((back.layers.len(), back.recording, back.next_id), (3, saved.recording, saved.next_id));
    assert_eq!(back.layers[1].name.len(), 65_534);
    assert_eq!(back.layers[0].offsets[5], Vec3::ZERO);
    assert!((0..3).all(|l| back.layers[l].id == saved.layers[l].id && back.layers[l].percent == saved.layers[l].percent));
    assert!(back.layers[2].offsets.iter().zip(&saved.layers[2].offsets).all(|(a, b)| bits(*a) == bits(*b)));
    // A body cut short, said to be longer than it is, or for another mesh, is refused whole.
    assert!(read(&mut &body[4..body.len() - 1], body.len() as u64 - 1, mesh.vertex_count()).is_none());
    assert!(read(&mut &body[4..], body.len() as u64 + 4, mesh.vertex_count()).is_none());
    assert!(read(&mut &body[4..], body.len() as u64, mesh.vertex_count() + 1).is_none());
}
