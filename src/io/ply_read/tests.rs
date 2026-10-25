use glam::{Mat4, Vec3};

use super::*;
use crate::io::{ExportPaint, export_ply_placed};
use crate::mesh::Mesh;
use crate::mesh::paint::palette::{PaletteEntry, ScenePaint};
use crate::mesh::paint::{Channels, Mat8, PaintLayer, PaintRow, PaintStack};
use crate::mesh::primitives::{self, Primitive};

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("sculpt_io_ply_read_tests");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// The one model a file holds.
fn one(bytes: &[u8]) -> RawMesh {
    let mut objects = parse(bytes).expect("the file reads");
    assert_eq!(objects.len(), 1);
    objects.remove(0)
}

/// What a file that cannot be read says.
fn message(bytes: &[u8]) -> String {
    match parse(bytes) {
        Ok(_) => panic!("the file should not read"),
        Err(e) => e,
    }
}

fn polys_of(raw: &RawMesh) -> Option<(Vec<u32>, Vec<u32>)> {
    raw.polys.as_ref().map(|p| (p.start.clone(), p.indices.clone()))
}

/// Numbers written the way a binary file stores them.
struct Bin {
    big: bool,
    bytes: Vec<u8>,
}

impl Bin {
    fn put(&mut self, little: &[u8]) {
        if self.big {
            self.bytes.extend(little.iter().rev());
        } else {
            self.bytes.extend(little);
        }
    }
    fn u8(&mut self, v: u8) {
        self.bytes.push(v);
    }
    fn i16(&mut self, v: i16) {
        self.put(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.put(&v.to_le_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.put(&v.to_le_bytes());
    }
    fn f64(&mut self, v: f64) {
        self.put(&v.to_le_bytes());
    }
}

const BIG_HEADER: &str = "ply\r\nformat binary_big_endian 1.0\r\ncomment made by hand\r\nobj_info nothing\r\n\
    element camera 2\r\nproperty float fov\r\nproperty list uchar short extras\r\n\
    element vertex 5\r\nproperty double x\r\nproperty double y\r\nproperty double z\r\n\
    property float confidence\r\nproperty uchar red\r\nproperty uchar green\r\nproperty uchar blue\r\n\
    element face 2\r\nproperty uchar flags\r\nproperty list uchar uint vertex_indices\r\n\
    property list uchar float texcoord\r\n\
    element edge 1\r\nproperty int a\r\nproperty int b\r\nend_header\r\n";

/// A big-endian file with doubles for positions and, around the vertices and
/// faces, things to step over: an element before them whose records have
/// lists, an unused property in each, and an element after. A square, a point
/// above it, a triangle on the point and a quad on the square.
fn big_endian_file() -> Vec<u8> {
    let mut file = Bin { big: true, bytes: BIG_HEADER.as_bytes().to_vec() };
    // The cameras: the first has no extras, the second three.
    file.f32(60.0);
    file.u8(0);
    file.f32(90.0);
    file.u8(3);
    (1..=3).for_each(|k| file.i16(k));
    let points = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0], [0.5, 0.5, 2.0]];
    for (i, p) in points.iter().enumerate() {
        p.iter().for_each(|&c| file.f64(c));
        file.f32(0.9);
        [10 * i as u8, 20, 30].iter().for_each(|&c| file.u8(c));
    }
    // Each face: a flag, its corners, and some texture coordinates.
    file.u8(7);
    file.u8(3);
    [0, 1, 4].iter().for_each(|&c| file.u32(c));
    file.u8(6);
    (0..6).for_each(|k| file.f32(k as f32));
    file.u8(8);
    file.u8(4);
    [0, 3, 2, 1].iter().for_each(|&c| file.u32(c));
    file.u8(0);
    // The one edge.
    file.u32(0);
    file.u32(1);
    file.bytes
}

#[test]
fn big_endian_doubles_with_other_elements_and_properties_to_step_over() {
    let raw = one(&big_endian_file());
    assert_eq!(raw.positions.len(), 5);
    assert_eq!(raw.positions[1], Vec3::X);
    assert_eq!(raw.positions[4], Vec3::new(0.5, 0.5, 2.0));
    assert_eq!(raw.indices, vec![0, 1, 4, 0, 3, 2, 0, 2, 1]);
    assert_eq!(polys_of(&raw), Some((vec![0, 3, 7], vec![0, 1, 4, 0, 3, 2, 1])));
    let colours: Vec<[u8; 4]> = raw.colours.iter().map(|c| c.0).collect();
    assert_eq!(colours, (0..5).map(|i| [10 * i, 20, 30, 255]).collect::<Vec<_>>());
    assert!(!raw.real_size && raw.name.is_empty() && raw.groups.is_empty());
}

#[test]
fn a_binary_file_cut_short_says_which_part_is_missing() {
    let whole = big_endian_file();
    // The last element is not needed, so a file cut inside it still reads.
    assert!(parse(&whole[..whole.len() - 4]).is_ok());
    // Cut inside the last face's corners.
    let faces = message(&whole[..whole.len() - 8 - 5]);
    assert_eq!(faces, "The PLY file ends before all its faces were read");
    // Cut inside the vertices, after the two cameras (16 bytes).
    let header_end = whole.windows(12).position(|w| w == b"end_header\r\n").unwrap() + 12;
    let vertices = message(&whole[..header_end + 16 + 2 * 31 + 10]);
    assert_eq!(vertices, "The PLY file ends before all its vertices were read");
    // Nothing after the header at all.
    assert!(message(&whole[..header_end]).contains("ends before"));
}

#[test]
fn a_count_larger_than_the_file_could_hold_is_refused_before_anything_is_made() {
    let mut binary = b"ply\nformat binary_little_endian 1.0\nelement vertex 4000000000\nproperty float x\nproperty float y\n\
        property float z\nelement face 1\nproperty list uchar int vertex_indices\nend_header\n"
        .to_vec();
    binary.extend([0u8; 40]);
    assert!(message(&binary).contains("ends before all its vertices were read"));
    let text = b"ply\nformat ascii 1.0\nelement vertex 99999999999\nproperty float x\nproperty float y\nproperty float z\nend_header\n0 0 0\n";
    assert!(message(text).contains("ends before all its vertices were read"));
}

#[test]
fn ascii_with_triangles_and_quads_mixed_and_windows_line_ends() {
    let text = "ply\r\nformat ascii 1.0\r\ncomment a square and a triangle\r\nelement vertex 5\r\n\
        property float x\r\nproperty float y\r\nproperty float z\r\n\
        property uchar r\r\nproperty uchar g\r\nproperty uchar b\r\nproperty uchar a\r\n\
        element face 2\r\nproperty list uchar int vertex_index\r\nend_header\r\n\
        0 0 0 255 0 0 128\r\n1 0 0 0 255 0 128\r\n1 1 0 0 0 255 128\r\n0 1 0 9 9 9 128\r\n2 0.5 0 5 6 7 128\r\n\
        4 0 1 2 3\r\n3 1 4 2\r\n";
    let raw = one(text.as_bytes());
    assert_eq!(raw.positions.len(), 5);
    assert_eq!(raw.positions[4], Vec3::new(2.0, 0.5, 0.0));
    assert_eq!(raw.indices, vec![0, 1, 2, 0, 2, 3, 1, 4, 2]);
    assert_eq!(polys_of(&raw), Some((vec![0, 4, 7], vec![0, 1, 2, 3, 1, 4, 2])));
    let colours: Vec<[u8; 4]> = raw.colours.iter().map(|c| c.0).collect();
    assert_eq!(colours, vec![[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255], [9, 9, 9, 255], [5, 6, 7, 255]]);
}

#[test]
fn faces_may_come_first_and_fractions_are_colours_from_0_to_1() {
    let text = "ply\nformat ascii 1.0\nelement face 1\nproperty list uchar int vertex_indices\n\
        element vertex 3\nproperty float x\nproperty float y\nproperty float z\n\
        property float diffuse_red\nproperty double diffuse_green\nproperty float diffuse_blue\nend_header\n\
        3 0 1 2\n0 0 0 1.0 0.5 0\n1 0 0 0 0 0\n0 1 0 0.2 0.2 0.2\n";
    let raw = one(text.as_bytes());
    assert_eq!(raw.indices, vec![0, 1, 2]);
    assert!(raw.polys.is_none());
    let colours: Vec<[u8; 4]> = raw.colours.iter().map(|c| c.0).collect();
    assert_eq!(colours, vec![[255, 128, 0, 255], [0, 0, 0, 255], [51, 51, 51, 255]]);
}

#[test]
fn a_face_naming_a_vertex_that_is_not_there_is_left_out() {
    let text = "ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nproperty float y\nproperty float z\n\
        element face 3\nproperty list uchar int vertex_indices\nend_header\n\
        0 0 0\n1 0 0\n0 1 0\n3 0 1 2\n3 0 1 99\n3 0 -1 2\n";
    let raw = one(text.as_bytes());
    assert_eq!((raw.positions.len(), raw.indices), (3, vec![0, 1, 2]));
    assert!(raw.colours.is_empty());
}

#[test]
fn point_clouds_say_they_have_no_faces() {
    let cloud = "ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nproperty float y\nproperty float z\nend_header\n0 0 0\n1 0 0\n0 1 0\n";
    let e = message(cloud.as_bytes());
    assert!(e.contains("3 points but no faces"), "{e}");
    let empty = cloud.replace("end_header", "element face 0\nproperty list uchar int vertex_indices\nend_header");
    assert!(message(empty.as_bytes()).contains("points but no faces"));
}

#[test]
fn files_that_are_not_ply_or_are_damaged_say_so_in_plain_words() {
    assert!(message(b"solid cube\nfacet normal 0 0 0").contains("not a PLY file"));
    assert!(message(b"").contains("not a PLY file"));
    assert!(message(b"ply\nformat ascii 1.0\nelement vertex 3\n").contains("end_header"));
    assert!(message(b"ply\nformat ascii 1.0\nelement vertex 1\nproperty complex x\nend_header\n").contains("does not know"));
    assert!(message(b"ply\nformat binary_pdp_endian 1.0\nend_header\n").contains("cannot read"));
    assert!(message(b"ply\nelement vertex 1\nproperty float x\nend_header\n0\n").contains("how the file is stored"));
    let no_z = "ply\nformat ascii 1.0\nelement vertex 1\nproperty float x\nproperty float y\nend_header\n0 0\n";
    assert!(message(no_z.as_bytes()).contains("x, y and z"));
    let word = "ply\nformat ascii 1.0\nelement vertex 1\nproperty float x\nproperty float y\nproperty float z\nend_header\n0 banana 0\n";
    assert!(message(word.as_bytes()).contains("not a number"));
    let faceless = "ply\nformat ascii 1.0\nelement vertex 1\nproperty float x\nproperty float y\nproperty float z\n\
        element face 1\nproperty float area\nend_header\n0 0 0\n1\n";
    assert!(message(faceless.as_bytes()).contains("do not list which vertices"));
}

/// The file our export writes for `mesh`, read back.
fn round_trip(mesh: &Mesh, paint: &ExportPaint, name: &str) -> RawMesh {
    let path = temp(name);
    export_ply_placed(mesh, &[Mat4::IDENTITY], paint, &path).unwrap();
    one(&std::fs::read(&path).unwrap())
}

fn assert_same_shape(mesh: &Mesh, raw: &RawMesh) {
    assert_eq!(raw.positions, mesh.positions);
    assert_eq!(raw.indices, mesh.indices);
    let polys = mesh.polys.as_ref().map(|p| (p.start.clone(), p.indices.clone()));
    assert_eq!(polys_of(raw), polys);
}

#[test]
fn our_own_export_of_a_sphere_and_of_a_cube_reads_back_the_same() {
    for (primitive, name) in [(Primitive::Sphere, "sphere.ply"), (Primitive::Cube, "cube.ply")] {
        let mesh = primitives::build(primitive, 300, 0.5);
        let raw = round_trip(&mesh, &ExportPaint::default(), name);
        assert_same_shape(&mesh, &raw);
        assert!(raw.colours.is_empty(), "an unpainted model has no colours");
    }
}

#[test]
fn a_model_of_quads_comes_back_as_quads() {
    let corners = [Vec3::ZERO, Vec3::X, Vec3::new(1.0, 1.0, 0.0), Vec3::Y];
    let (mut mesh, to_new) = Mesh::new_mapped(corners.to_vec(), vec![0, 1, 2, 0, 2, 3], None);
    let quad: Vec<u32> = [0usize, 1, 2, 3].iter().map(|&i| to_new[i]).collect();
    mesh.polys = Some(crate::mesh::Polys { start: vec![0, 4], indices: quad.clone() });
    let raw = round_trip(&mesh, &ExportPaint::default(), "quad.ply");
    assert_same_shape(&mesh, &raw);
    assert_eq!(polys_of(&raw), Some((vec![0, 4], quad)));
}

/// A sphere painted red on its first half of vertices, with a material
/// weight on every third (so the file has an extra float property).
fn painted_sphere() -> Mesh {
    let mut mesh = primitives::build(Primitive::Sphere, 300, 0.5);
    let n = mesh.vertex_count();
    let colour = (0..n).map(|v| if v < n / 2 { Rgba8([255, 0, 0, 255]) } else { Rgba8([0; 4]) }).collect();
    let material = (0..n).map(|v| if v % 3 == 0 { Mat8([7, 200]) } else { Mat8([0, 0]) }).collect();
    let data = Channels { colour: Some(colour), material: Some(material), ..Channels::default() };
    let mut stack = PaintStack::new(n);
    stack.layers.push(PaintLayer { row: PaintRow::new(1, "Paint 1".into()), data });
    (stack.next_id, stack.active, stack.table) = (2, Some(1), vec![7]);
    mesh.set_paint(Some(stack));
    mesh
}

#[test]
fn our_own_export_of_a_painted_sphere_brings_its_colours_back() {
    let mesh = painted_sphere();
    let entry = PaletteEntry { slot: 7, name: "Metal - Rust".into(), swatch: [0.6, 0.3, 0.1], ..PaletteEntry::default() };
    let paint = ExportPaint::of([0.5, 0.5, 0.5], &ScenePaint { palette: vec![entry], ..ScenePaint::default() });
    let raw = round_trip(&mesh, &paint, "painted.ply");
    assert_same_shape(&mesh, &raw);
    let n = mesh.vertex_count();
    assert_eq!(raw.colours.len(), n);
    for (v, colour) in raw.colours.iter().enumerate() {
        let want = if v < n / 2 { [255, 0, 0, 255] } else { [128, 128, 128, 255] };
        assert_eq!(colour.0, want, "vertex {v}");
    }
}
