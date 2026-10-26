//! UVs through the files: a saved scene, an OBJ and an FBX, out and back.

use glam::{Mat4, Quat, Vec2, Vec3};

use super::*;
use crate::io::{Lost, SavedObject, export_fbx, export_obj, import, imported_mesh, load_scene, save_scene};
use crate::mesh::primitives::{self, Primitive};

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("sculpt_io_uvs_tests");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn object(mesh: usize) -> SavedObject {
    let (position, rotation, scale) = (Vec3::ZERO, Quat::IDENTITY, Vec3::ONE);
    SavedObject { name: format!("Object {mesh}"), visible: true, mesh, position, rotation, scale }
}

/// A cube of quads whose UVs are its six sides laid out as a cross, a
/// picture four squares wide: eight corners in fourteen places on the
/// picture, so most corners are on a seam.
const CUBE: &str = "\
v -1 -1 1\nv 1 -1 1\nv 1 1 1\nv -1 1 1\nv -1 -1 -1\nv 1 -1 -1\nv 1 1 -1\nv -1 1 -1
vt 0 0.25\nvt 0.25 0.25\nvt 0.5 0.25\nvt 0.75 0.25\nvt 1 0.25
vt 0 0.5\nvt 0.25 0.5\nvt 0.5 0.5\nvt 0.75 0.5\nvt 1 0.5
vt 0.25 0.75\nvt 0.5 0.75\nvt 0.25 0\nvt 0.5 0
f 1/2 2/3 3/8 4/7
f 2/3 6/4 7/9 3/8
f 6/4 5/5 8/10 7/9
f 5/1 1/2 4/7 8/6
f 4/7 3/8 7/12 8/11
f 5/13 6/14 2/3 1/2
";

/// One corner: where it is and its UV, as bits (with -0.0 as 0.0).
type Corner = ([u32; 3], [u32; 2]);

/// Every triangle as its corners, each with its UV, started from its least
/// corner so that it is the same triangle wherever its numbering starts, and
/// the triangles in order: the same for the same surface and UVs however
/// they are numbered.
fn corners(positions: &[Vec3], indices: &[u32], uvs: &[Vec2]) -> Vec<[Corner; 3]> {
    assert_eq!(uvs.len(), indices.len(), "one UV to a corner");
    let corner = |k: usize| -> Corner {
        let p = positions[indices[k] as usize].to_array().map(|x| (x + 0.0).to_bits());
        (p, uvs[k].to_array().map(|x| (x + 0.0).to_bits()))
    };
    let mut all: Vec<[Corner; 3]> = (0..indices.len() / 3)
        .map(|t| {
            let mut three = [corner(t * 3), corner(t * 3 + 1), corner(t * 3 + 2)];
            let least = (0..3).min_by_key(|&c| three[c]).unwrap();
            three.rotate_left(least);
            three
        })
        .collect();
    all.sort_unstable();
    all
}

fn mesh_corners(mesh: &Mesh) -> Vec<[Corner; 3]> {
    corners(&mesh.positions, &mesh.indices, mesh.uvs())
}

fn raw_corners(raw: &crate::io::RawMesh) -> Vec<[Corner; 3]> {
    corners(&raw.positions, &raw.indices, &raw.uvs)
}

/// The cube read from [`CUBE`], and what its triangles should be: each quad
/// cut as a fan from its first corner, with the file's UVs.
/// `name` is the file it is read from, one for each test, as they run at once.
fn cube(name: &str) -> (Mesh, Vec<[Corner; 3]>) {
    let path = temp(&format!("{name}.obj"));
    std::fs::write(&path, CUBE).unwrap();
    let raw = import(&path).unwrap();
    assert_eq!(raw.positions.len(), 8, "eight corners, however many places on the picture");
    assert_eq!((raw.indices.len(), raw.uvs.len()), (36, 36));
    let quads = raw.polys.as_ref().expect("the quads are kept");
    assert_eq!(quads.start.len(), 7);
    let mut points = Vec::new();
    let mut picture = Vec::new();
    for line in CUBE.lines() {
        let numbers: Vec<f32> = line.split_ascii_whitespace().skip(1).filter_map(|n| n.parse().ok()).collect();
        match line.split_ascii_whitespace().next() {
            Some("v") => points.push(Vec3::new(numbers[0], numbers[1], numbers[2])),
            Some("vt") => picture.push(Vec2::new(numbers[0], numbers[1])),
            _ => {}
        }
    }
    let (mut positions, mut indices, mut uvs) = (Vec::new(), Vec::new(), Vec::new());
    for line in CUBE.lines().filter(|l| l.starts_with("f ")) {
        let face: Vec<(usize, usize)> = line[2..]
            .split_ascii_whitespace()
            .map(|token| {
                let (v, vt) = token.split_once('/').unwrap();
                (v.parse::<usize>().unwrap() - 1, vt.parse::<usize>().unwrap() - 1)
            })
            .collect();
        for k in 1..face.len() - 1 {
            for (v, vt) in [face[0], face[k], face[k + 1]] {
                indices.push(positions.len() as u32);
                positions.push(points[v]);
                uvs.push(picture[vt]);
            }
        }
    }
    let wanted = corners(&positions, &indices, &uvs);
    assert_eq!(raw_corners(&raw), wanted, "as read");
    let mesh = imported_mesh(raw);
    assert!(mesh.has_uvs() && mesh.polys.is_some());
    assert_eq!(mesh_corners(&mesh), wanted, "as built into a mesh");
    (mesh, wanted)
}

/// A ball whose UVs are worked out from where each corner is, each
/// triangle's own, so that every corner of the ball has one.
fn mapped_ball() -> Mesh {
    let mut mesh = primitives::build(Primitive::Sphere, 2_000, 0.5);
    let uvs = (0..mesh.indices.len())
        .map(|k| {
            let p = mesh.positions[mesh.indices[k] as usize];
            Vec2::new(p.z.atan2(p.x) / std::f32::consts::TAU + 0.5, p.y + 0.5 + (k / 3) as f32 * 1e-4)
        })
        .collect();
    mesh.set_uvs(uvs).unwrap();
    mesh
}

// ---------------------------------------------------------------- OBJ

#[test]
fn an_obj_cube_keeps_its_seams_out_and_back() {
    let (mesh, wanted) = cube("cube-seams");
    let path = temp("cube-out.obj");
    export_obj(&mesh, Mat4::IDENTITY, &path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text.lines().filter(|l| l.starts_with("vt ")).count(), 14, "each place on the picture once");
    let face = text.lines().find(|l| l.starts_with("f ")).unwrap();
    assert_eq!(face.split_ascii_whitespace().count(), 5, "a quad stays a quad: {face}");
    assert!(face.split_ascii_whitespace().skip(1).all(|c| c.split('/').count() == 3 && !c.contains("//")), "{face}");

    let raw = import(&path).unwrap();
    assert_eq!(raw.polys.as_ref().map(|p| p.start.len()), Some(7), "six quads");
    assert_eq!(raw_corners(&raw), wanted, "the same UVs on the same corners, to the bit");
}

#[test]
fn an_obj_with_groups_and_several_places_keeps_its_uvs() {
    let (mut mesh, wanted) = cube("cube-groups");
    // Each side a group of its own.
    let ids = (0..mesh.triangle_count()).map(|t| 1 + (t / 2) as u16).collect();
    mesh.set_group_ids(ids);
    let path = temp("cube-places.obj");
    crate::io::export_obj_placed(&mesh, &[("A", Mat4::IDENTITY), ("B", Mat4::from_translation(Vec3::X * 5.0))], &path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text.lines().filter(|l| l.starts_with("vt ")).count(), 28, "each place writes its own");
    let objects = crate::io::import_objects(&path).unwrap();
    assert_eq!(objects.len(), 2);
    assert_eq!(raw_corners(&objects[0]), wanted);
    assert!(!objects[0].groups.is_empty(), "the groups came too");
    let moved: Vec<Vec3> = objects[1].positions.iter().map(|p| *p - Vec3::X * 5.0).collect();
    assert_eq!(corners(&moved, &objects[1].indices, &objects[1].uvs), wanted, "the second place names its own UVs");
}

#[test]
fn an_obj_reader_takes_uvs_by_number_from_either_end_and_does_without_where_none_are_named() {
    let text = "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nvt 0.5 0.5\nvt 0.25 0.75 0\nf 1/1 2/-1 3/1\nf 1 3 4\n";
    let path = temp("partial.obj");
    std::fs::write(&path, text).unwrap();
    let raw = import(&path).unwrap();
    let (a, b) = (Vec2::new(0.5, 0.5), Vec2::new(0.25, 0.75));
    // The second face names none, and its corners are at (0, 0).
    assert_eq!(raw.uvs, [a, b, a, Vec2::ZERO, Vec2::ZERO, Vec2::ZERO]);
    // A file with no `vt` at all has no UVs.
    std::fs::write(&path, "v 0 0 0\nv 1 0 0\nv 1 1 0\nf 1 2 3\n").unwrap();
    assert!(import(&path).unwrap().uvs.is_empty());
}

// ---------------------------------------------------------------- FBX

#[test]
fn an_fbx_carries_the_uvs_out_and_back() {
    let (cube, wanted) = cube("cube-fbx");
    let path = temp("cube.fbx");
    export_fbx(&cube, Mat4::IDENTITY, &path).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert!(bytes.windows(14).any(|w| w == b"LayerElementUV") && bytes.windows(5).any(|w| w == b"UVMap"));
    let raw = import(&path).unwrap();
    assert_eq!(raw.polys.as_ref().map(|p| p.start.len()), Some(7), "six quads");
    assert_eq!(raw_corners(&raw), wanted);

    // A triangle mesh, written mirrored: each face turned round, and its UVs with it.
    let ball = mapped_ball();
    let mirror = Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0));
    export_fbx(&ball, mirror, &path).unwrap();
    let raw = import(&path).unwrap();
    let placed: Vec<Vec3> = ball.positions.iter().map(|p| mirror.transform_point3(*p)).collect();
    let turned: Vec<u32> = ball.indices.chunks_exact(3).flat_map(|t| [t[2], t[1], t[0]]).collect();
    let turned_uvs: Vec<Vec2> = ball.uvs().chunks_exact(3).flat_map(|t| [t[2], t[1], t[0]]).collect();
    assert_eq!(raw_corners(&raw), corners(&placed, &turned, &turned_uvs));
}

#[test]
fn a_quad_cut_by_a_seam_goes_to_an_fbx_as_its_triangles_and_the_rest_as_quads() {
    let (mut mesh, _) = cube("cube-seamed");
    // Move one triangle's UVs along, so its quad has a seam down the diagonal.
    let mut uvs = mesh.uvs().to_vec();
    uvs[..3].iter_mut().for_each(|uv| *uv += Vec2::new(3.0, 0.0));
    mesh.set_uvs(uvs).unwrap();
    let path = temp("seamed.fbx");
    export_fbx(&mesh, Mat4::IDENTITY, &path).unwrap();
    let raw = import(&path).unwrap();
    let sizes: Vec<u32> = raw.polys.as_ref().unwrap().start.windows(2).map(|w| w[1] - w[0]).collect();
    assert_eq!(sizes.iter().filter(|&&n| n == 4).count(), 5);
    assert_eq!(sizes.iter().filter(|&&n| n == 3).count(), 2);
    assert_eq!(raw_corners(&raw), mesh_corners(&mesh));
}

// ---------------------------------------------------------------- .sclp

#[test]
fn a_saved_scene_keeps_the_uvs_to_the_bit() {
    let (cube, _) = cube("cube-saved");
    let ball = mapped_ball();
    let plain = primitives::build(Primitive::Cube, 500, 0.5);
    let file = temp("uvs.sclp");
    save_scene(&[&cube, &ball, &plain], &[object(0), object(1), object(2)], 0, &file).unwrap();
    let opened = load_scene(&file).unwrap();
    assert_eq!(opened.lost, Lost::default());
    assert_eq!(mesh_corners(&opened.meshes[0]), mesh_corners(&cube));
    assert!(opened.meshes[0].polys.is_some(), "with its quads");
    assert_eq!(mesh_corners(&opened.meshes[1]), mesh_corners(&ball));
    assert!(!opened.meshes[2].has_uvs(), "a mesh without UVs opens without");

    // Saved again from what was opened, the same again.
    let again = temp("uvs-again.sclp");
    let meshes: Vec<&Mesh> = opened.meshes.iter().collect();
    save_scene(&meshes, &[object(0), object(1), object(2)], 0, &again).unwrap();
    assert_eq!(mesh_corners(&load_scene(&again).unwrap().meshes[1]), mesh_corners(&ball));
}

/// Where the block of `tag` begins in a saved file, its body, and its length.
fn block(bytes: &[u8], tag: &[u8; 4]) -> Option<(usize, usize, usize)> {
    let at = bytes.windows(4).position(|found| found == tag)?;
    Some((at, at + 12, u64::from_le_bytes(bytes[at + 4..at + 12].try_into().unwrap()) as usize))
}

#[test]
fn a_file_from_before_uvs_opens_without_them_and_a_damaged_block_costs_the_uvs_alone() {
    let mut ball = mapped_ball();
    ball.mask.iter_mut().step_by(3).for_each(|m| *m = 0.5);
    let shape = |mesh: &Mesh| {
        let mut points: Vec<[u32; 3]> = mesh.positions.iter().map(|p| p.to_array().map(f32::to_bits)).collect();
        points.sort_unstable();
        points
    };
    let file = temp("before.sclp");
    save_scene(&[&ball], &[object(0)], 0, &file).unwrap();
    let whole = std::fs::read(&file).unwrap();
    let (at, body, length) = block(&whole, b"UVCO").expect("the file has the block");
    assert_eq!(length, HEAD + ball.indices.len() * 8);

    // Without the block, as a version from before UVs wrote it.
    let mut older = whole.clone();
    older.drain(at..body + length);
    let path = temp("before-now.sclp");
    std::fs::write(&path, &older).unwrap();
    let opened = load_scene(&path).unwrap();
    assert!(!opened.meshes[0].has_uvs() && opened.lost == Lost::default());
    assert_eq!(shape(&opened.meshes[0]), shape(&ball));

    // With a corner count that is not the mesh's, or that does not fill the block.
    type Damage = Box<dyn Fn(&mut Vec<u8>)>;
    let damages: [(&str, Damage); 2] = [
        ("a count that is not the mesh's", Box::new(move |b| b[body + 4] ^= 1)),
        ("a count past the block", Box::new(move |b| b[body + 9] ^= 0x10)),
    ];
    for (what, damage) in damages {
        let mut bytes = whole.clone();
        damage(&mut bytes);
        std::fs::write(&path, &bytes).unwrap();
        let opened = load_scene(&path).unwrap_or_else(|why| panic!("{what}: the file still opens ({why})"));
        let back = &opened.meshes[0];
        assert!(!back.has_uvs(), "{what}");
        assert_eq!(opened.lost, Lost { uvs: 1, ..Lost::default() }, "{what}");
        assert_eq!(shape(back), shape(&ball), "{what}: the shape is the one saved");
        assert_eq!(back.mask.iter().filter(|&&m| m == 0.5).count(), ball.mask.iter().filter(|&&m| m == 0.5).count(), "{what}");
    }
    let said = Lost { uvs: 2, ..Lost::default() }.note().unwrap();
    assert!(said.starts_with("The UVs saved with 2 objects could not be read") && said.ends_with("without its UVs."), "{said}");
}
