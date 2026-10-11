//! Files and damage: exports that never harm the file already there, and
//! points that are not numbers being saved and opened without losing the work.

use super::*;
use crate::mesh::primitives::{self, Primitive};

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("sculpt_io_damage_tests");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// FNV-1a: a fixed fingerprint of a file's bytes.
fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3))
}

fn ball() -> Mesh {
    primitives::build(Primitive::Sphere, 2_000, 0.5)
}

fn one_object(name: &str) -> SavedObject {
    SavedObject {
        name: name.into(),
        visible: true,
        mesh: 0,
        position: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    }
}

/// A mesh with one point that is not a number and one that is infinite.
fn damaged_ball() -> Mesh {
    let mut mesh = ball();
    mesh.positions[10] = Vec3::NAN;
    mesh.positions[20] = Vec3::new(f32::INFINITY, 0.0, f32::NEG_INFINITY);
    mesh
}

fn assert_whole(mesh: &Mesh) {
    assert!(mesh.positions.iter().all(|p| p.is_finite()), "a position is still not a number");
    assert!(mesh.normals.iter().all(|n| n.is_finite()), "a normal is not a number");
    assert!(mesh.mask.iter().all(|m| m.is_finite()), "a mask value is not a number");
}

#[test]
fn an_export_that_fails_leaves_the_file_already_there_whole() {
    let mesh = ball();
    for (name, export) in [
        ("keep.obj", export_obj as fn(&Mesh, Mat4, &Path) -> Result<usize>),
        ("keep.stl", export_stl as fn(&Mesh, Mat4, &Path) -> Result<usize>),
    ] {
        let path = temp(name);
        std::fs::write(&path, b"the model that was already here").unwrap();
        // The new file is written beside the old one first. With that name
        // taken by a folder it cannot be, which is as good as a full disk.
        let beside = temp_beside(&path);
        let _ = std::fs::remove_dir_all(&beside);
        std::fs::create_dir(&beside).unwrap();
        let failed = export(&mesh, Mat4::IDENTITY, &path);
        assert!(failed.is_err(), "{name}: the export should have failed");
        assert_eq!(std::fs::read(&path).unwrap(), b"the model that was already here", "{name} was harmed");
        std::fs::remove_dir(&beside).unwrap();

        // And when it works the file is replaced and nothing is left beside it.
        export(&mesh, Mat4::IDENTITY, &path).unwrap();
        assert_ne!(std::fs::read(&path).unwrap(), b"the model that was already here");
        assert!(!beside.exists(), "{name}: a temporary file was left behind");
    }
}

#[test]
fn exported_bytes_are_those_of_the_models_own_numbers() {
    let mesh = primitives::build(Primitive::Cube, 300, 0.5);
    let models = [Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0)), Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0))];
    // OBJ: a header, then each place's block, as `write_object` makes it.
    let path = temp("bytes.obj");
    export_obj_placed(&mesh, &[("a", models[0]), ("b", models[1])], &path).unwrap();
    let mut want = b"# Exported from Sculpt\n".to_vec();
    want.extend_from_slice(b"o a\n");
    write_object(&mut want, &mesh, models[0], 0).unwrap();
    want.extend_from_slice(b"o b\n");
    write_object(&mut want, &mesh, models[1], mesh.positions.len() as u32).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), want);

    // STL: 80 bytes of header, the count, then 50 bytes a facet; a mirroring
    // place has its corners turned round.
    let path = temp("bytes.stl");
    export_stl_placed(&mesh, &models, &path).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let triangles = mesh.triangle_count();
    assert_eq!(bytes.len(), STL_HEADER + 4 + 2 * triangles * STL_FACET);
    assert_eq!(u32::from_le_bytes(bytes[80..84].try_into().unwrap()) as usize, 2 * triangles);
    assert!(bytes.starts_with(b"Exported from Sculpt (millimetres, Z up)"));
    // Fingerprint taken from the exporter as it was before it wrote through a temporary file.
    assert_eq!(fnv(&bytes), 0x827f_317a_9c2b_0069, "the STL bytes changed");
}

#[test]
fn a_model_with_damaged_points_saves_and_the_file_opens() {
    let mesh = damaged_ball();
    let path = temp("damaged_model.sclp");
    save_scene(&[&mesh], &[one_object("Ball")], 0, &path).unwrap();
    let back = load_scene(&path).expect("a file the app wrote must open");
    assert_whole(&back.meshes[0]);
    assert_eq!(back.meshes[0].triangle_count(), mesh.triangle_count());
}

#[test]
fn a_file_that_already_holds_damaged_points_opens_repaired() {
    // As an older build, or a hand-edited file, would leave one: the points
    // are in the file as they are in the model.
    let mesh = damaged_ball();
    let path = temp("damaged_on_disk.sclp");
    let info = Details::default().document(&[one_object("Ball")], 0);
    replace_file(&path, |w| write_scene_raw(w, &mesh, &info)).unwrap();
    let back = load_scene(&path).expect("damaged points are repaired, not refused");
    assert_whole(&back.meshes[0]);
}

/// A scene file written with the mesh's numbers exactly as they are, damaged
/// points included: what a build from before the repair would have left.
fn write_scene_raw(w: &mut impl Write, mesh: &Mesh, info: &[u8]) -> std::io::Result<()> {
    w.write_all(SCENE_MAGIC)?;
    let positions: &[u8] = bytemuck::cast_slice(&mesh.positions);
    let indices: &[u8] = bytemuck::cast_slice(&mesh.indices);
    block(w, MESH, 16 + positions.len() + indices.len())?;
    w.write_all(&(mesh.positions.len() as u64).to_le_bytes())?;
    w.write_all(&(mesh.indices.len() as u64).to_le_bytes())?;
    w.write_all(positions)?;
    w.write_all(indices)?;
    block(w, INFO, info.len())?;
    w.write_all(info)
}

#[test]
fn saving_says_how_many_points_it_mended_and_leaves_the_model_alone() {
    let mesh = damaged_ball();
    let path = temp("says_so.sclp");
    let mended = save_scene_with(&[&mesh], &[one_object("Ball")], 0, &Details::default(), &path).unwrap();
    assert_eq!(mended, 2);
    assert!(mesh.positions[10].is_nan(), "the model in memory is not touched by a save");

    // What was written is a sphere with its two points put back about where they were.
    let back = load_scene(&path).unwrap();
    assert_eq!(back.repaired, 0, "the saved file holds nothing damaged");
    let clean = ball();
    let (lo, hi) = (clean.bounds().min, clean.bounds().max);
    let room = Vec3::splat(0.05);
    let inside = |p: &Vec3| p.cmpge(lo - room).all() && p.cmple(hi + room).all();
    assert!(back.meshes[0].positions.iter().all(inside));

    let sound = save_scene_with(&[&clean], &[one_object("Ball")], 0, &Details::default(), &path).unwrap();
    assert_eq!(sound, 0);
}

#[test]
fn opening_says_how_many_points_it_mended() {
    let mesh = damaged_ball();
    let path = temp("says_so_on_open.sclp");
    let info = Details::default().document(&[one_object("Ball")], 0);
    replace_file(&path, |w| write_scene_raw(w, &mesh, &info)).unwrap();
    let back = load_scene(&path).unwrap();
    assert_eq!(back.repaired, 2);
    assert_whole(&back.meshes[0]);
    assert_eq!(back.meshes[0].triangle_count(), mesh.triangle_count(), "no triangle was dropped");
}

#[test]
fn the_old_one_mesh_file_is_mended_too() {
    let mesh = damaged_ball();
    let path = temp("one_mesh.sclp");
    save_native(&mesh, &path).unwrap();
    let back = load_scene(&path).expect("the old one-mesh file opens");
    assert_eq!(back.repaired, 2);
    assert_whole(&back.meshes[0]);
}

/// A scene file in one of the older fixed layouts ("SCLP0002", "SCLP0003"),
/// written by hand from the layout in `load_fixed`: the magic, the counts, each
/// mesh's raw arrays, then each object's record. The mesh's numbers go in as
/// they are, damaged points included. `turned` adds the rotation and scale
/// that "SCLP0003" objects carry and "SCLP0002" objects do not.
fn fixed_layout_file(magic: &[u8; 8], meshes: &[&Mesh], names: &[&str], turned: bool) -> Vec<u8> {
    let mut bytes = magic.to_vec();
    for number in [meshes.len() as u32, names.len() as u32, 0] {
        bytes.extend_from_slice(&number.to_le_bytes());
    }
    for mesh in meshes {
        bytes.extend_from_slice(&(mesh.positions.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&(mesh.indices.len() as u64).to_le_bytes());
        bytes.extend_from_slice(bytemuck::cast_slice(&mesh.positions));
        bytes.extend_from_slice(bytemuck::cast_slice(&mesh.indices));
        bytes.extend_from_slice(bytemuck::cast_slice(&mesh.mask));
    }
    for (number, name) in names.iter().enumerate() {
        bytes.extend_from_slice(&(name.len() as u32).to_le_bytes());
        bytes.extend_from_slice(name.as_bytes());
        bytes.push(1);
        bytes.extend_from_slice(&(number.min(meshes.len() - 1) as u32).to_le_bytes());
        bytes.extend_from_slice(bytemuck::cast_slice(&[1.0f32, 2.0, 3.0]));
        if turned {
            bytes.extend_from_slice(bytemuck::cast_slice(&[0.0f32, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0]));
        }
    }
    bytes
}

#[test]
fn the_older_fixed_layouts_open_with_their_damaged_points_mended() {
    // Point 10 is not a number, point 20 is infinite, and the mask value of
    // point 5 (a point of its own, so a third one) is not a number.
    let mut mesh = damaged_ball();
    mesh.mask[5] = f32::NAN;
    mesh.mask[6] = 1.0;
    assert!(mesh.positions[10].is_nan() && mesh.positions[20].x.is_infinite());
    let clean = ball();
    let (lo, hi) = (clean.bounds().min, clean.bounds().max);
    let room = Vec3::splat(0.05);
    for (magic, turned, name) in [(SCENE_MAGIC_PLACED, false, "placed.sclp"), (SCENE_MAGIC_FIXED, true, "fixed.sclp")] {
        let path = temp(name);
        std::fs::write(&path, fixed_layout_file(magic, &[&mesh], &["Ball"], turned)).unwrap();
        let back = load_scene(&path).unwrap_or_else(|e| panic!("{name} is repaired, not refused: {e}"));
        assert_eq!(back.repaired, 3, "{name}: two positions and one mask value");
        assert_eq!(back.meshes.len(), 1);
        let opened = &back.meshes[0];
        assert_whole(opened);
        assert_eq!(opened.positions.len(), mesh.positions.len(), "{name}: no point was dropped");
        assert_eq!(opened.triangle_count(), mesh.triangle_count(), "{name}: no triangle was dropped");
        assert!(opened.positions.iter().all(|p| p.cmpge(lo - room).all() && p.cmple(hi + room).all()), "{name}");
        assert_eq!(opened.mask.iter().filter(|&&m| m == 1.0).count(), 1, "{name}: the real mask is kept");
        assert_eq!(opened.mask.iter().filter(|&&m| m != 0.0).count(), 1, "{name}: the damaged mask is cleared");
        assert_eq!((back.objects.len(), back.objects[0].name.as_str()), (1, "Ball"), "{name}");
        assert_eq!(back.objects[0].position, Vec3::new(1.0, 2.0, 3.0), "{name}");
    }
}

#[test]
fn a_fixed_layout_with_two_meshes_counts_and_mends_each() {
    let (a, b) = (damaged_ball(), ball());
    let path = temp("fixed_two.sclp");
    std::fs::write(&path, fixed_layout_file(SCENE_MAGIC_FIXED, &[&a, &b], &["A", "B"], true)).unwrap();
    let back = load_scene(&path).unwrap();
    assert_eq!((back.repaired, back.meshes.len(), back.objects.len()), (2, 2, 2));
    back.meshes.iter().for_each(assert_whole);
    assert_eq!(back.objects[1].mesh, 1);
}

#[test]
fn a_mask_that_is_not_a_number_does_not_stop_a_save_or_an_open() {
    let mut mesh = ball();
    mesh.mask[5] = f32::NAN;
    mesh.mask[6] = 1.0;
    let path = temp("bad_mask.sclp");
    assert_eq!(save_scene_with(&[&mesh], &[one_object("Ball")], 0, &Details::default(), &path).unwrap(), 1);
    let back = load_scene(&path).unwrap();
    assert_whole(&back.meshes[0]);
    assert_eq!(back.meshes[0].mask.iter().filter(|&&m| m == 1.0).count(), 1, "the real mask is kept");
}

/// Every number in an OBJ's `v` and `vn` lines, which must all be real numbers.
fn obj_numbers(text: &str) -> Vec<f32> {
    let mut numbers = Vec::new();
    for line in text.lines().filter(|l| l.starts_with("v ") || l.starts_with("vn ")) {
        for word in line.split_ascii_whitespace().skip(1) {
            numbers.push(word.parse::<f32>().unwrap_or_else(|_| panic!("{word:?} is not a number")));
        }
    }
    numbers
}

#[test]
fn an_exported_obj_of_a_damaged_model_holds_only_real_numbers() {
    let mesh = damaged_ball();
    let places = [("a", Mat4::IDENTITY), ("b", Mat4::from_translation(Vec3::new(2.0, 0.0, 0.0)))];
    let path = temp("damaged_export.obj");
    let mended = export_obj_placed(&mesh, &places, &path).unwrap();
    assert_eq!(mended, 2, "the count is of the model's points, not of its places");
    let text = std::fs::read_to_string(&path).unwrap();
    let numbers = obj_numbers(&text);
    assert!(numbers.len() >= 2 * 6 * mesh.positions.len(), "positions and normals of both places");
    assert!(numbers.iter().all(|n| n.is_finite()), "a number in the file is not finite");
    assert!(!text.to_ascii_lowercase().contains("nan") && !text.to_ascii_lowercase().contains("inf"));
    // The same faces as ever, and the model on screen is not touched.
    assert_eq!(text.lines().filter(|l| l.starts_with("f ")).count(), 2 * mesh.triangle_count());
    assert!(mesh.positions[10].is_nan() && mesh.positions[20].x.is_infinite());
    assert_eq!(export_obj(&mesh, Mat4::IDENTITY, &temp("damaged_one.obj")).unwrap(), 2);
}

#[test]
fn an_exported_stl_of_a_damaged_model_holds_only_real_numbers() {
    let mesh = damaged_ball();
    let models = [Mat4::IDENTITY, Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0))];
    let path = temp("damaged_export.stl");
    assert_eq!(export_stl_placed(&mesh, &models, &path).unwrap(), 2);
    let bytes = std::fs::read(&path).unwrap();
    let facets = 2 * mesh.triangle_count();
    assert_eq!(bytes.len(), STL_HEADER + 4 + facets * STL_FACET);
    for facet in bytes[STL_HEADER + 4..].chunks_exact(STL_FACET) {
        for number in facet[..48].chunks_exact(4) {
            assert!(f32::from_le_bytes(number.try_into().unwrap()).is_finite(), "a number in the file is not finite");
        }
    }
    assert!(mesh.positions[10].is_nan() && mesh.positions[20].x.is_infinite(), "the model is not touched");
    // And it reads back as a model of the same size as the sound one.
    let back = import(&path).unwrap();
    assert!(back.positions.iter().all(|p| p.is_finite()));
}

#[test]
fn exporting_a_sound_model_mends_nothing_and_writes_what_it_always_did() {
    let mesh = ball();
    let stl = temp("sound.stl");
    let obj = temp("sound.obj");
    assert_eq!(export_stl(&mesh, Mat4::IDENTITY, &stl).unwrap(), 0);
    assert_eq!(export_obj(&mesh, Mat4::IDENTITY, &obj).unwrap(), 0);
    // The OBJ is the model's own positions and normals, untouched by the mending path.
    let text = std::fs::read_to_string(&obj).unwrap();
    let first = text.lines().find(|l| l.starts_with("v ")).unwrap();
    let p = mesh.positions[0];
    assert_eq!(first, format!("v {} {} {}", p.x, p.y, p.z));
    let n = mesh.normals[0];
    let normal = text.lines().find(|l| l.starts_with("vn ")).unwrap();
    assert_eq!(normal, format!("vn {:.5} {:.5} {:.5}", n.x, n.y, n.z));
}

#[test]
fn the_normals_of_a_mended_export_point_the_way_the_surface_does() {
    // A sphere about the origin: a normal points away from the middle.
    let mesh = damaged_ball();
    let points = export_points(&mesh).0;
    for (p, n) in points.positions.iter().zip(points.normals.iter()) {
        assert!(n.is_finite() && (n.length() - 1.0).abs() < 1e-3, "{n:?}");
        assert!(n.dot(p.normalize()) > 0.9, "{n:?} at {p:?}");
    }
}

/// The four native layouts, each holding `mesh` with its numbers as they are.
fn every_layout(mesh: &Mesh, name: &str) -> Vec<(String, PathBuf)> {
    let mut files = Vec::new();
    let mut put = |layout: &str, bytes: Vec<u8>| {
        let path = temp(&format!("{name}_{layout}.sclp"));
        std::fs::write(&path, bytes).unwrap();
        files.push((layout.to_owned(), path));
    };
    let mut blocks = Vec::new();
    write_scene_raw(&mut blocks, mesh, &Details::default().document(&[one_object("Ball")], 0)).unwrap();
    put("blocks", blocks);
    put("placed", fixed_layout_file(SCENE_MAGIC_PLACED, &[mesh], &["Ball"], false));
    put("fixed", fixed_layout_file(SCENE_MAGIC_FIXED, &[mesh], &["Ball"], true));
    // The one-mesh file, as `save_native` lays it out.
    let mut one = NATIVE_MAGIC.to_vec();
    one.extend_from_slice(&(mesh.positions.len() as u64).to_le_bytes());
    one.extend_from_slice(&(mesh.indices.len() as u64).to_le_bytes());
    one.extend_from_slice(bytemuck::cast_slice(&mesh.positions));
    one.extend_from_slice(bytemuck::cast_slice(&mesh.indices));
    one.extend_from_slice(bytemuck::cast_slice(&mesh.mask));
    put("one_mesh", one);
    files
}

/// What opening `path` comes to, with a fault while reading it told apart
/// from a refusal: a damaged file must be refused, never bring the app down.
fn opened(path: &Path) -> std::result::Result<Result<SavedScene>, String> {
    let quiet = std::panic::catch_unwind(|| load_scene(path));
    quiet.map_err(|fault| {
        let said = fault.downcast_ref::<String>().cloned();
        said.or_else(|| fault.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_else(|| "a panic".to_owned())
    })
}

/// A review point, read and not run: nothing holds a file's triangles to the
/// points it has, so a damaged `.sclp` might bring the app down in an
/// ordinary Open. It does not: such a triangle is left out as the mesh is
/// built, in every layout. What was missing is that opening said nothing of
/// it; now it is counted, for the status line to say (`notes_opened`).
#[test]
fn a_file_whose_triangles_name_points_it_does_not_hold_opens_without_them_and_says_so() {
    let sound = ball();
    let points = sound.positions.len() as u32;
    for (what, wrong) in [("one_past_the_last", points), ("far_out", u32::MAX)] {
        for at in [0, 7, sound.indices.len() - 1] {
            let mut mesh = ball();
            mesh.indices[at] = wrong;
            for (layout, path) in every_layout(&mesh, &format!("index_{what}_{at}")) {
                let case = format!("{layout}, {what} at corner {at}");
                let came = opened(&path).unwrap_or_else(|fault| panic!("{case}: opening it panicked: {fault}"));
                let back = came.unwrap_or_else(|e| panic!("{case}: all the rest of the work is refused with it: {e}"));
                assert_eq!((back.left_out, back.repaired), (1, 0), "{case}: one triangle, and it is said");
                let built = &back.meshes[0];
                assert_eq!(built.triangle_count(), sound.triangle_count() - 1, "{case}: every other triangle is there");
                assert!(built.indices.iter().all(|&corner| (corner as usize) < built.positions.len()), "{case}");
                assert_whole(built);
                let said = notes_opened(back.repaired, back.left_out).expect("there is something to say");
                assert!(said.contains("1 damaged triangle in the file was left out"), "{case}: {said}");
            }
        }
    }
    // The same files with nothing wrong in them open whole, and nothing is said.
    for (layout, path) in every_layout(&sound, "index_sound") {
        let back = load_scene(&path).unwrap_or_else(|e| panic!("{layout}: a sound file was refused: {e}"));
        assert_eq!(back.meshes[0].triangle_count(), sound.triangle_count(), "{layout}");
        assert_eq!((back.left_out, notes_opened(back.repaired, back.left_out)), (0, None), "{layout}");
    }
}

/// Triangles are three corners each. Corners left over at the end are part of
/// a triangle that cannot be used, and are counted as one.
#[test]
fn a_file_with_part_of_a_triangle_opens_without_it_and_says_so() {
    for dropped in [1, 2] {
        let mut mesh = ball();
        let whole = mesh.triangle_count() - 1;
        mesh.indices.truncate(mesh.indices.len() - dropped);
        for (layout, path) in every_layout(&mesh, &format!("part_triangle_{dropped}")) {
            let case = format!("{layout}, {dropped} corners short");
            let came = opened(&path).unwrap_or_else(|fault| panic!("{case}: opening it panicked: {fault}"));
            let back = came.unwrap_or_else(|e| panic!("{case}: refused: {e}"));
            assert_eq!((back.left_out, back.meshes[0].triangle_count()), (1, whole), "{case}");
        }
    }
}

/// Damage of both kinds in one file, in two of its meshes: each is counted,
/// and the line that says so says both.
#[test]
fn damaged_points_and_damaged_triangles_in_one_file_are_both_counted() {
    let (mut a, mut b) = (damaged_ball(), ball());
    a.indices[3] = u32::MAX;
    b.indices[0] = b.positions.len() as u32;
    b.indices[9] = b.positions.len() as u32 + 5;
    let path = temp("both_kinds.sclp");
    std::fs::write(&path, fixed_layout_file(SCENE_MAGIC_FIXED, &[&a, &b], &["A", "B"], true)).unwrap();
    let back = load_scene(&path).unwrap();
    assert_eq!((back.repaired, back.left_out), (2, 3));
    back.meshes.iter().for_each(assert_whole);
    let said = notes_opened(back.repaired, back.left_out).unwrap();
    assert!(said.starts_with("2 damaged points in the file") && said.contains("3 damaged triangles in the file were"));
}
