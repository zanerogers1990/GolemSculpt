//! Face groups through a saved file and back, and through an OBJ file.

use std::collections::HashMap;

use glam::{Mat4, Quat, Vec3};
use sculpt_multires::Split;

use super::*;
use crate::io::{Lost, SavedObject, export_obj, import, load_scene, save_scene};
use crate::mesh::Polys;
use crate::mesh::levels::{stepped, subdivided};
use crate::mesh::primitives::{self, Primitive};

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("sculpt_io_groups_tests");
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

fn ball() -> Mesh {
    primitives::build(Primitive::Sphere, 3_000, 0.5)
}

fn centre(mesh: &Mesh, t: usize) -> Vec3 {
    let c = &mesh.indices[t * 3..t * 3 + 3];
    (mesh.positions[c[0] as usize] + mesh.positions[c[1] as usize] + mesh.positions[c[2] as usize]) / 3.0
}

/// Three bands up the model and a cap in no group.
fn banded(mut mesh: Mesh) -> Mesh {
    let band = |p: Vec3| {
        if p.y > 0.3 {
            0
        } else if p.y > 0.05 {
            12
        } else if p.y > -0.2 {
            3
        } else {
            700
        }
    };
    let ids = (0..mesh.triangle_count()).map(|t| band(centre(&mesh, t))).collect();
    mesh.set_group_ids(ids);
    mesh
}

/// The group of every triangle, each triangle named by where its corners
/// are: the same however the mesh happens to be numbered.
fn held(mesh: &Mesh) -> HashMap<[[u32; 3]; 3], u16> {
    let corner = |v: u32| mesh.positions[v as usize].to_array().map(f32::to_bits);
    let by_place: HashMap<_, _> = (0..mesh.triangle_count())
        .map(|t| {
            let mut corners = [0, 1, 2].map(|k| corner(mesh.indices[t * 3 + k]));
            corners.sort_unstable();
            (corners, mesh.group_of(t as u32))
        })
        .collect();
    assert_eq!(by_place.len(), mesh.triangle_count(), "no two triangles in one place");
    by_place
}

fn reopened(mesh: &Mesh, name: &str) -> (Mesh, Lost) {
    let file = temp(name);
    save_scene(&[mesh], &[object(0)], 0, &file).unwrap();
    let mut opened = load_scene(&file).unwrap();
    (opened.meshes.remove(0), opened.lost)
}

/// Where the body of a block starts in a saved file, and how long it is.
fn block(bytes: &[u8], tag: &[u8; 4]) -> Option<(usize, usize)> {
    let at = bytes.windows(4).position(|found| found == tag)?;
    Some((at + 12, u64::from_le_bytes(bytes[at + 4..at + 12].try_into().unwrap()) as usize))
}

#[test]
fn groups_are_saved_and_come_back_to_the_bit() {
    let mut mesh = banded(ball());
    // With part hidden and part masked, which have blocks of their own.
    let low: Vec<u32> = (0..mesh.triangle_count() as u32).filter(|&t| centre(&mesh, t as usize).x < -0.3).collect();
    mesh.hide(&low);
    for v in 0..mesh.vertex_count() {
        mesh.mask[v] = if mesh.positions[v].z > 0.2 { 0.75 } else { 0.0 };
    }
    let (back, lost) = reopened(&mesh, "groups.sclp");
    assert_eq!(lost, Lost::default());
    assert!(held(&back) == held(&mesh));
    assert_eq!((back.group_count(), back.hidden_triangle_count()), (3, low.len()));
    // Saved again from what was opened, it is the same again.
    let (again, _) = reopened(&back, "groups-again.sclp");
    assert!(held(&again) == held(&mesh));

    // A mesh without groups writes no block, and opens without any.
    let plain = ball();
    let file = temp("plain.sclp");
    save_scene(&[&plain], &[object(0)], 0, &file).unwrap();
    assert!(block(&std::fs::read(&file).unwrap(), b"GRPS").is_none());
    let opened = load_scene(&file).unwrap();
    assert!(!opened.meshes[0].has_groups() && opened.lost == Lost::default());

    // Two meshes in one file each get their own back.
    let other = {
        let mut other = primitives::build(Primitive::Cube, 1_200, 0.4);
        let ids = (0..other.triangle_count()).map(|t| if centre(&other, t).x > 0.0 { 5 } else { 6 }).collect();
        other.set_group_ids(ids);
        other
    };
    let file = temp("two.sclp");
    save_scene(&[&plain, &mesh, &other], &[object(0), object(1), object(2)], 0, &file).unwrap();
    let opened = load_scene(&file).unwrap();
    assert!(!opened.meshes[0].has_groups());
    assert!(held(&opened.meshes[1]) == held(&mesh) && held(&opened.meshes[2]) == held(&other));
}

#[test]
fn a_quad_mesh_and_a_mesh_with_levels_and_layers_keep_their_groups_through_a_file() {
    // Quads: the faces come back as quads, each whole in its group.
    let quads = {
        let sign = |bit: u32| if bit == 0 { -0.5 } else { 0.5 };
        let positions: Vec<Vec3> =
            (0..8u32).map(|i| Vec3::new(sign(i & 1), sign(i >> 1 & 1), sign(i >> 2 & 1))).collect();
        let faces: [[u32; 4]; 6] = [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];
        let triangles: Vec<u32> = faces.iter().flat_map(|q| [q[0], q[1], q[2], q[0], q[2], q[3]]).collect();
        let (mut mesh, moved) = Mesh::new_mapped(positions, triangles, None);
        let corners = faces.iter().flatten().map(|&v| moved[v as usize]).collect();
        mesh.polys = Some(Polys { start: (0..=6).map(|f| f * 4).collect(), indices: corners });
        let ids = (0..mesh.triangle_count()).map(|t| if centre(&mesh, t).z > 0.2 { 2 } else { 9 }).collect();
        mesh.set_group_ids(ids);
        mesh
    };
    let (back, lost) = reopened(&quads, "quads.sclp");
    assert!(lost == Lost::default() && back.polys.is_some());
    assert!(held(&back) == held(&quads));
    assert_eq!(back.face_groups().unwrap().iter().filter(|&&id| id == 2).count(), 1);

    // Levels: saved on each level, the groups come back, and so does the exact
    // border of the level it was not saved on.
    let top = banded(subdivided(&subdivided(&ball(), Split::Smooth).unwrap().0, Split::Smooth).unwrap().0);
    let ordered = |mesh: &Mesh| mesh.levels.as_ref().unwrap().ordered_groups(mesh);
    let fine = ordered(&top);
    for level in 0..3 {
        let mut mesh = stepped(&top, level).unwrap().0;
        if level == 1 {
            // A layer rides along untouched.
            mesh = stepped(&mesh, 2).unwrap().0;
            mesh.add_layer(Some("Detail".into()));
            mesh = stepped(&mesh, 1).unwrap().0;
        }
        let (back, lost) = reopened(&mesh, &format!("level{level}.sclp"));
        assert_eq!(lost, Lost::default(), "level {level}");
        assert_eq!(ordered(&back), ordered(&mesh), "level {level}");
        assert_eq!(back.levels.as_ref().unwrap().print(&back), mesh.levels.as_ref().unwrap().print(&mesh));
        assert_eq!(back.layers.is_some(), level == 1);
        assert_eq!(ordered(&stepped(&back, 2).unwrap().0), fine, "saved on level {level}, stepped back to the finest");
        for to in 0..3 {
            assert_eq!(
                ordered(&stepped(&back, to).unwrap().0),
                ordered(&stepped(&mesh, to).unwrap().0),
                "{level} to {to}"
            );
        }
    }
}

#[test]
fn a_groups_block_that_cannot_be_read_costs_the_groups_and_nothing_else() {
    let mut mesh = stepped(&banded(subdivided(&ball(), Split::Smooth).unwrap().0), 0).unwrap().0;
    mesh.add_layer(Some("Kept".into()));
    let file = temp("damaged.sclp");
    save_scene(&[&mesh], &[object(0)], 0, &file).unwrap();
    let whole = std::fs::read(&file).unwrap();
    let (body, length) = block(&whole, b"GRPS").expect("the file has the block");
    assert_eq!(
        length,
        HEAD + mesh.triangle_count() * 2 + 2 * 8 + mesh.triangle_count() * 4 * 2,
        "the mesh's list and the finer level's"
    );
    // The second level list's count: after the head, the mesh's own list and the first list's count.
    let second = body + HEAD + mesh.triangle_count() * 2 + 8;
    type Damage = Box<dyn Fn(&mut Vec<u8>)>;
    let damages: [(&str, Damage); 4] = [
        ("a layout this version does not know", Box::new(move |b| b[body + 4] = 9)),
        ("a triangle count that is not the mesh's", Box::new(move |b| b[body + 8] ^= 1)),
        ("more level lists than there are", Box::new(move |b| b[body + 16] += 1)),
        ("a level list longer than the block", Box::new(move |b| b[second + 1] ^= 0x40)),
    ];
    let shape = |mesh: &Mesh| {
        let mut points: Vec<[u32; 3]> = mesh.positions.iter().map(|p| p.to_array().map(f32::to_bits)).collect();
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
        let back = opened.meshes.remove(0);
        assert!(!back.has_groups(), "{what}: the groups are left off");
        assert_eq!(opened.lost, Lost { levels: 0, layers: 0, groups: 1 }, "{what}");
        assert_eq!(shape(&back), shape(&mesh), "{what}: the shape is the one saved");
        assert!(back.levels.is_some() && back.layers.is_some(), "{what}: levels and layers are other blocks");
        // The mesh is whole and works: it steps up, and takes groups again.
        let mut up = stepped(&back, 1).unwrap_or_else(|why| panic!("{what}: {why}")).0;
        assert!(!up.has_groups());
        up.set_group_ids(vec![4; up.triangle_count()]);
        assert_eq!(stepped(&up, 0).unwrap().0.group_count(), 1);
    }
    let said = Lost { levels: 0, layers: 0, groups: 2 }.note().unwrap();
    assert!(
        said.starts_with("The face groups saved with 2 objects could not be read")
            && said.contains("without its groups")
    );
    let sound = load_scene(&file).unwrap();
    assert!(sound.meshes[0].has_groups() && sound.lost == Lost::default());
}

#[test]
fn an_obj_file_carries_the_groups_out_and_back() {
    let mesh = banded(ball());
    let path = temp("groups.obj");
    export_obj(&mesh, Mat4::IDENTITY, &path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let names: Vec<&str> = text.lines().filter(|line| line.starts_with("g ")).collect();
    assert_eq!(names, ["g Ungrouped", "g Group_3", "g Group_12", "g Group_700"], "each group once, in order");
    assert_eq!(text.lines().filter(|line| line.starts_with("f ")).count(), mesh.triangle_count());

    // Read back, every face is in the group it left in, by the same number.
    let back = imported_mesh(import(&path).unwrap());
    assert_eq!(back.triangle_count(), mesh.triangle_count());
    let place = |mesh: &Mesh, t: usize| (centre(mesh, t) * 1e4).round().as_ivec3().to_array();
    let was: HashMap<[i32; 3], u16> =
        (0..mesh.triangle_count()).map(|t| (place(&mesh, t), mesh.group_of(t as u32))).collect();
    assert_eq!(was.len(), mesh.triangle_count());
    assert!((0..back.triangle_count()).all(|t| was[&place(&back, t)] == back.group_of(t as u32)));

    // A mesh without groups is written as it always was, with no `g` line, and reads back without any.
    let plain_path = temp("plain.obj");
    export_obj(&ball(), Mat4::IDENTITY, &plain_path).unwrap();
    assert!(!std::fs::read_to_string(&plain_path).unwrap().lines().any(|line| line.starts_with("g")));
    assert!(import(&plain_path).unwrap().groups.is_empty());
}

#[test]
fn a_quad_mesh_goes_out_as_quads_under_its_groups() {
    let n = 6usize;
    let at = |i: usize, j: usize| (j * (n + 1) + i) as u32;
    let positions: Vec<Vec3> = (0..=n).flat_map(|j| (0..=n).map(move |i| Vec3::new(i as f32, j as f32, 0.0))).collect();
    let (mut triangles, mut quads) = (Vec::new(), Vec::new());
    for j in 0..n {
        for i in 0..n {
            let q = [at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1)];
            triangles.extend_from_slice(&[q[0], q[1], q[2], q[0], q[2], q[3]]);
            quads.push(q);
        }
    }
    let (mut mesh, to_new) = Mesh::new_mapped(positions, triangles, None);
    let corners: Vec<u32> = quads.iter().flatten().map(|&v| to_new[v as usize]).collect();
    mesh.polys = Some(Polys { start: (0..=quads.len() as u32).map(|f| f * 4).collect(), indices: corners });
    let ids = (0..mesh.triangle_count()).map(|t| if centre(&mesh, t).x < 2.0 { 1 } else { 2 }).collect();
    mesh.set_group_ids(ids);

    let path = temp("quads.obj");
    export_obj(&mesh, Mat4::IDENTITY, &path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let mut faces: HashMap<&str, usize> = HashMap::new();
    let mut group = "";
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("g ") {
            group = name;
        } else if line.starts_with("f ") {
            assert_eq!(line.split_ascii_whitespace().count(), 5, "a quad is written as a quad: {line}");
            *faces.entry(group).or_default() += 1;
        }
    }
    assert_eq!(faces, HashMap::from([("Group_1", 2 * n), ("Group_2", 4 * n)]));
    // Read back (as triangles, as every OBJ is here), the groups are the same faces.
    let back = imported_mesh(import(&path).unwrap());
    assert_eq!(back.group_triangles(1, true).len(), 4 * n);
    assert!((0..back.triangle_count()).all(|t| (back.group_of(t as u32) == 1) == (centre(&back, t).x < 2.0)));
}

#[test]
fn groups_named_by_another_program_are_read_as_groups() {
    // Three triangles side by side and one off on its own, as text.
    let points = "v 0 0 0\nv 1 0 0\nv 0 1 0\nv 1 1 0\nv 2 0 0\nv 2 1 0\nv 5 5 5\nv 6 5 5\nv 5 6 5\n";
    let read = |faces: &str| super::super::parse_obj(format!("{points}{faces}").as_bytes()).unwrap().groups;
    // Names of any kind become groups in the order they are met; a name met again is the same group.
    assert_eq!(read("g head\nf 1 2 3\ng body\nf 2 4 3\ng head\nf 2 5 4\ng tail fin\nf 7 8 9\n"), [1, 2, 1, 3]);
    // Our own names keep their numbers, and other names take the numbers left free.
    assert_eq!(read("g Group_2\nf 1 2 3\ng arm\nf 2 4 3\ng Group_1\nf 2 5 4\ng leg\nf 7 8 9\n"), [2, 3, 1, 4]);
    // Faces before any `g`, and under the names that mean no group, are in none.
    assert_eq!(read("f 1 2 3\ng default\nf 2 4 3\ng Ungrouped\nf 2 5 4\ng skin\nf 7 8 9\n"), [0, 0, 0, 1]);
    // A polygon is one face: both triangles it is cut into are in its group.
    assert_eq!(read("g a\nf 1 2 4 3\ng b\nf 2 5 6\n"), [1, 1, 2]);
    // One group for everything says nothing, and so does no group at all.
    assert!(read("g Mesh\nf 1 2 3\nf 2 4 3\nf 7 8 9\n").is_empty());
    assert!(read("f 1 2 3\nf 2 4 3\n").is_empty());
    // A triangle the import throws away takes its place in the list with it.
    assert_eq!(read("g a\nf 1 2 3\ng b\nf 2 2 4\ng c\nf 7 8 9\n"), [1, 3]);
}
