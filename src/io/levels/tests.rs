//! Subdivision levels through a saved file and back.

use glam::Quat;

use super::*;
use crate::io::{SavedObject, load_scene, save_scene};
use crate::mesh::levels::{stepped, subdivided, without};
use crate::mesh::primitives::{self, Primitive};

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("sculpt_io_levels_tests");
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

/// A ball of three levels, sculpted, masked and partly hidden on the
/// finest, and stepped to `level`.
fn sculpted_on(level: usize) -> Mesh {
    let ball = primitives::build(Primitive::Sphere, 1_500, 0.5);
    let mut top = subdivided(&subdivided(&ball, Split::Smooth).unwrap().0, Split::Flat).unwrap().0;
    for v in 0..top.positions.len() {
        let p = top.positions[v];
        top.positions[v] = p * (1.0 + 0.02 * (p.x * 31.0).sin() * (p.z * 17.0).cos());
        top.mask[v] = if p.y > 0.2 { 0.5 } else { 0.0 };
    }
    top.hide(&(0..top.triangle_count() as u32).filter(|t| t % 9 < 2).collect::<Vec<_>>());
    if level == 2 { top } else { stepped(&top, level).unwrap().0 }
}

fn print(mesh: &Mesh) -> u64 {
    mesh.levels.as_ref().expect("the mesh has levels").print(mesh)
}

#[test]
fn levels_are_saved_and_come_back_on_the_level_that_was_current() {
    for level in 0..3 {
        let mesh = sculpted_on(level);
        let file = temp(&format!("level{level}.sclp"));
        save_scene(&[&mesh], &[object(0)], 0, &file).unwrap();
        let mut opened = load_scene(&file).unwrap();
        let back = opened.meshes.remove(0);
        let (was, now) = (mesh.levels.as_ref().unwrap(), back.levels.as_ref().expect("the levels came back"));
        assert_eq!((now.count(), now.current(), now.is_flat(2)), (3, level, true));
        assert_eq!(print(&back), print(&mesh), "level {level}: every vertex, mask value and hidden face");
        assert!(now.saved().parts == was.saved().parts, "level {level}: the stack, bit for bit");
        assert_eq!(back.triangle_count(), mesh.triangle_count());
        assert_eq!(back.hidden_triangle_count(), mesh.hidden_triangle_count());
        // It is a working stack: every other level is what the original's is.
        for to in (0..3).filter(|&to| to != level) {
            assert_eq!(print(&stepped(&back, to).unwrap().0), print(&stepped(&mesh, to).unwrap().0), "{level} to {to}");
        }
        // And saving what was opened writes the same levels again.
        let again = temp(&format!("level{level}-again.sclp"));
        save_scene(&[&back], &[object(0)], 0, &again).unwrap();
        assert_eq!(print(&load_scene(&again).unwrap().meshes[0]), print(&mesh));
    }
}

#[test]
fn quads_and_their_levels_are_saved_together() {
    let ball = primitives::build(Primitive::Sphere, 4_000, 0.5);
    let (quads, _) = crate::quadremesh::quad_remesh_mesh(&ball, 200).unwrap();
    let top = subdivided(&subdivided(&quads, Split::Smooth).unwrap().0, Split::Smooth).unwrap().0;
    for mesh in [stepped(&top, 0).unwrap().0, stepped(&top, 1).unwrap().0, top] {
        let file = temp(&format!("quads{}.sclp", mesh.levels.as_ref().unwrap().current()));
        save_scene(&[&mesh], &[object(0)], 0, &file).unwrap();
        let back = load_scene(&file).unwrap().meshes.remove(0);
        assert_eq!(print(&back), print(&mesh));
        let faces = |m: &Mesh| m.polys.as_ref().map(|p| p.start.len() - 1);
        assert!(faces(&back).is_some() && faces(&back) == faces(&mesh));
        assert_eq!(back.levels.as_ref().unwrap().scheme(), sculpt_multires::Scheme::CatmullClark);
    }
}

#[test]
fn a_scene_keeps_each_meshs_own_levels() {
    let plain = primitives::build(Primitive::Cube, 600, 0.5);
    let (low, high) = (sculpted_on(0), sculpted_on(2));
    let file = temp("several.sclp");
    // The middle mesh is shown twice: an object and its instance.
    save_scene(&[&low, &plain, &high], &[object(0), object(1), object(1), object(2)], 0, &file).unwrap();
    let opened = load_scene(&file).unwrap();
    assert_eq!((opened.meshes.len(), opened.objects.len()), (3, 4));
    assert_eq!(print(&opened.meshes[0]), print(&low));
    assert!(opened.meshes[1].levels.is_none(), "a mesh without levels has none after opening");
    assert_eq!(print(&opened.meshes[2]), print(&high));
}

#[test]
fn a_mesh_whose_levels_were_deleted_is_saved_as_any_other_mesh() {
    let mut mesh = sculpted_on(1);
    mesh.levels = without(&mesh, true);
    mesh.levels = without(&mesh, false);
    assert!(mesh.levels.is_none());
    let file = temp("none.sclp");
    save_scene(&[&mesh], &[object(0)], 0, &file).unwrap();
    let bytes = std::fs::read(&file).unwrap();
    assert!(!bytes.windows(4).any(|tag| tag == b"LVLS"), "nothing is written for a mesh without levels");
    assert!(load_scene(&file).unwrap().meshes[0].levels.is_none());
}

/// Where the `LVLS` block's body starts in a saved file, and how long it is.
fn block(bytes: &[u8]) -> (usize, usize) {
    let at = bytes.windows(4).position(|tag| tag == b"LVLS").expect("the file has a LVLS block");
    (at + 12, u64::from_le_bytes(bytes[at + 4..at + 12].try_into().unwrap()) as usize)
}

#[test]
fn a_levels_block_that_does_not_fit_costs_the_levels_and_nothing_else() {
    let mesh = sculpted_on(1);
    let file = temp("damaged.sclp");
    save_scene(&[&mesh], &[object(0)], 0, &file).unwrap();
    let whole = std::fs::read(&file).unwrap();
    let (body, length) = block(&whole);
    let u64_at = |bytes: &[u8], at: usize| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap()) as usize;
    // The counts are eleven u64 after the mesh number and the layout number.
    let count = move |n: usize| body + 8 + n * 8;
    let (corners, splits, kept) = (u64_at(&whole, count(1)), u64_at(&whole, count(4)), u64_at(&whole, count(6)));
    let lists = body + 8 + COUNTS * 8;
    type Damage = Box<dyn Fn(&mut Vec<u8>)>;
    let damages: [(&str, Damage); 7] = [
        ("a layout this version does not know", Box::new(move |b| b[body + 4] = 9)),
        ("counts that do not add up", Box::new(move |b| b[count(1)] ^= 1)),
        ("a level that is not there", Box::new(move |b| b[count(5)] = 7)),
        ("a triangle of the coarsest level on a vertex it does not have", Box::new(move |b| b[lists + 3] = 0x7f)),
        ("a way of splitting this version does not know", Box::new(move |b| b[lists + corners * 4] = 5)),
        ("a kept position that is not a number", Box::new(move |b| b[lists + corners * 4 + splits + 3] = 0xff)),
        (
            "a vertex list that is not this mesh's",
            Box::new(move |b| {
                let list = lists + corners * 4 + splits + kept * 16;
                let (first, second) = (b[list..list + 4].to_vec(), b[list + 4..list + 8].to_vec());
                b[list..list + 4].copy_from_slice(&second);
                b[list + 4..list + 8].copy_from_slice(&first);
            }),
        ),
    ];
    for (what, damage) in damages {
        let mut bytes = whole.clone();
        damage(&mut bytes);
        assert_eq!(bytes.len(), whole.len());
        assert!(bytes[body..body + length] != whole[body..body + length], "{what}: the block was changed");
        let damaged = temp("damaged-now.sclp");
        std::fs::write(&damaged, &bytes).unwrap();
        let opened = load_scene(&damaged).unwrap_or_else(|why| panic!("{what}: the file still opens ({why})"));
        let back = &opened.meshes[0];
        assert!(back.levels.is_none(), "{what}: the levels are left off");
        assert_eq!(opened.lost, crate::io::Lost { levels: 1, layers: 0, groups: 0 }, "{what}: and the status line is told");
        // The mesh is the one that was saved: its vertices, mask and hidden faces.
        assert_eq!(
            (back.vertex_count(), back.triangle_count()),
            (mesh.vertex_count(), mesh.triangle_count()),
            "{what}"
        );
        assert_eq!(back.hidden_triangle_count(), mesh.hidden_triangle_count(), "{what}");
    }
    let said = crate::io::Lost { levels: 1, layers: 0, groups: 0 }.note().unwrap();
    assert!(said.starts_with("The subdivision levels saved with 1 object could not be read"), "{said}");
    // Untouched, it opens with its levels: the damage above is what cost them.
    let sound = load_scene(&file).unwrap();
    assert!(sound.meshes[0].levels.is_some() && sound.lost == crate::io::Lost::default());
}

#[test]
fn the_length_written_is_the_length_of_what_is_written() {
    for level in [0, 2] {
        let mesh = sculpted_on(level);
        let saved = mesh.levels.as_ref().unwrap().saved();
        let mut body = Vec::new();
        write(&mut body, 3, &saved).unwrap();
        assert_eq!(body.len(), length(&saved));
        // The reader of the file takes the mesh's number off the front itself.
        let number = u32::from_le_bytes(body[..4].try_into().unwrap());
        let back = read(&mut &body[4..], body.len() as u64).expect("what was written reads back");
        assert_eq!(number, 3);
        assert!(back.parts == saved.parts);
        assert!(back.vertex == saved.vertex && back.triangle == saved.triangle && back.hidden == saved.hidden);
        // A body cut short, or said to be longer than it is, is refused whole.
        assert!(read(&mut &body[4..body.len() - 1], body.len() as u64 - 1).is_none());
        assert!(read(&mut &body[4..], body.len() as u64 + 4).is_none());
    }
}
