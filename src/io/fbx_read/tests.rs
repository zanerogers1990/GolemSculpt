use glam::{Mat4, Quat};

use super::*;
use crate::io::painted::{ExportPaint, Painted};
use crate::io::{export_fbx_objects, export_fbx_painted, import_objects};
use crate::mesh::Mesh;
use crate::mesh::paint::{Channels, PaintLayer, PaintRow, PaintStack};
use crate::mesh::primitives::{self, Primitive};

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("sculpt_io_fbx_read_tests");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// Six times the volume the triangles enclose: above zero when they face outward.
fn volume(raw: &RawMesh) -> f32 {
    let p = &raw.positions;
    raw.indices.chunks_exact(3).map(|t| p[t[0] as usize].dot(p[t[1] as usize].cross(p[t[2] as usize]))).sum()
}

fn near(a: Vec3, b: Vec3, within: f32) -> bool {
    (a - b).length() <= within
}

// ---------------------------------------------------------------- a small FBX writer
//
// Enough of a writer to make the files other programs write and the app's
// exporter does not: models placed by their properties, parents, other axes
// and units, packed arrays and the wide records of 7.5.

enum V {
    I(i32),
    D(f64),
    L(i64),
    S(Vec<u8>),
    Ints(Vec<i32>),
    Doubles(Vec<f64>),
}

fn s(text: &str) -> V {
    V::S(text.as_bytes().to_vec())
}

struct N {
    name: &'static str,
    values: Vec<V>,
    children: Vec<N>,
}

fn n(name: &'static str, values: Vec<V>, children: Vec<N>) -> N {
    N { name, values, children }
}

struct Writer {
    out: Vec<u8>,
    wide: bool,
    packed: bool,
}

impl Writer {
    fn number(&mut self, v: usize) {
        if self.wide {
            self.out.extend((v as u64).to_le_bytes());
        } else {
            self.out.extend((v as u32).to_le_bytes());
        }
    }

    fn patch(&mut self, at: usize, v: usize) {
        if self.wide {
            self.out[at..at + 8].copy_from_slice(&(v as u64).to_le_bytes());
        } else {
            self.out[at..at + 4].copy_from_slice(&(v as u32).to_le_bytes());
        }
    }

    fn close(&mut self) {
        let len = if self.wide { 25 } else { 13 };
        self.out.extend(std::iter::repeat_n(0, len));
    }

    fn record(&mut self, node: &N) {
        let start = self.out.len();
        let width = if self.wide { 8 } else { 4 };
        self.number(0);
        self.number(node.values.len());
        self.number(0);
        self.out.push(node.name.len() as u8);
        self.out.extend(node.name.as_bytes());
        let values_start = self.out.len();
        for value in &node.values {
            self.value(value);
        }
        let values_len = self.out.len() - values_start;
        if !node.children.is_empty() || node.values.is_empty() {
            for child in &node.children {
                self.record(child);
            }
            self.close();
        }
        let end = self.out.len();
        self.patch(start, end);
        self.patch(start + 2 * width, values_len);
    }

    fn array(&mut self, kind: u8, count: usize, raw: Vec<u8>) {
        self.out.push(kind);
        self.out.extend((count as u32).to_le_bytes());
        let (encoding, data) = if self.packed { (1u32, miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6)) } else { (0, raw) };
        self.out.extend(encoding.to_le_bytes());
        self.out.extend((data.len() as u32).to_le_bytes());
        self.out.extend(data);
    }

    fn value(&mut self, value: &V) {
        match value {
            V::I(v) => {
                self.out.push(b'I');
                self.out.extend(v.to_le_bytes());
            }
            V::D(v) => {
                self.out.push(b'D');
                self.out.extend(v.to_le_bytes());
            }
            V::L(v) => {
                self.out.push(b'L');
                self.out.extend(v.to_le_bytes());
            }
            V::S(bytes) => {
                self.out.push(b'S');
                self.out.extend((bytes.len() as u32).to_le_bytes());
                self.out.extend(bytes);
            }
            V::Ints(v) => self.array(b'i', v.len(), v.iter().flat_map(|i| i.to_le_bytes()).collect()),
            V::Doubles(v) => self.array(b'd', v.len(), v.iter().flat_map(|d| d.to_le_bytes()).collect()),
        }
    }
}

/// A whole file of these records, at this version, its arrays packed or not.
fn file(version: u32, packed: bool, top: &[N]) -> Vec<u8> {
    let mut writer = Writer { out: MAGIC.to_vec(), wide: version >= 7500, packed };
    writer.out.extend(version.to_le_bytes());
    for node in top {
        writer.record(node);
    }
    writer.close();
    // What closes a real file; the reader has no need of it.
    writer.out.extend([0u8; 160]);
    writer.out
}

fn p(name: &str, kind: &str, values: Vec<V>) -> N {
    let mut all = vec![s(name), s(kind), s(""), s("A")];
    all.extend(values);
    n("P", all, Vec::new())
}

fn p3(name: &str, v: [f64; 3]) -> N {
    p(name, "Vector3D", v.map(V::D).into())
}

fn p1(name: &str, v: i32) -> N {
    p(name, "int", vec![V::I(v)])
}

/// GlobalSettings for these axes, each (axis, sign), and centimetres per unit.
fn settings(up: (i32, i32), front: (i32, i32), right: (i32, i32), centimetres: f64) -> N {
    n("GlobalSettings", Vec::new(), vec![n("Properties70", Vec::new(), vec![
        p1("UpAxis", up.0),
        p1("UpAxisSign", up.1),
        p1("FrontAxis", front.0),
        p1("FrontAxisSign", front.1),
        p1("CoordAxis", right.0),
        p1("CoordAxisSign", right.1),
        p("UnitScaleFactor", "double", vec![V::D(centimetres)]),
    ])])
}

fn named(name: &str, class: &str) -> V {
    let mut bytes = name.as_bytes().to_vec();
    bytes.extend_from_slice(b"\x00\x01");
    bytes.extend_from_slice(class.as_bytes());
    V::S(bytes)
}

fn geometry(id: i64, name: &str, points: &[[f64; 3]], polygons: &[&[i32]]) -> N {
    let corners: Vec<i32> = polygons.iter().flat_map(|f| f.iter().enumerate().map(|(k, &c)| if k + 1 == f.len() { !c } else { c })).collect();
    n("Geometry", vec![V::L(id), named(name, "Geometry"), s("Mesh")], vec![
        n("Vertices", vec![V::Doubles(points.iter().flatten().copied().collect())], Vec::new()),
        n("PolygonVertexIndex", vec![V::Ints(corners)], Vec::new()),
    ])
}

fn model(id: i64, name: &str, properties: Vec<N>) -> N {
    n("Model", vec![V::L(id), named(name, "Model"), s("Mesh")], vec![n("Properties70", Vec::new(), properties)])
}

fn link(child: i64, parent: i64) -> N {
    n("C", vec![s("OO"), V::L(child), V::L(parent)], Vec::new())
}

fn scene(settings: N, objects: Vec<N>, links: Vec<N>) -> Vec<N> {
    vec![
        n("FBXHeaderExtension", Vec::new(), vec![n("FBXHeaderVersion", vec![V::I(1003)], Vec::new())]),
        settings,
        n("Objects", Vec::new(), objects),
        n("Connections", Vec::new(), links),
    ]
}

fn y_up() -> N {
    settings((1, 1), (2, 1), (0, 1), 1.0)
}

// ---------------------------------------------------------------- through the app's own export

#[test]
fn objects_exported_from_the_app_come_back_where_they_stood() {
    let ball = primitives::build(Primitive::Sphere, 2_000, 0.5);
    let block = primitives::build(Primitive::Cube, 500, 0.5);
    let turned = Mat4::from_scale_rotation_translation(Vec3::new(1.0, 2.0, 0.5), Quat::from_rotation_z(0.7), Vec3::new(3.0, -1.0, 2.0));
    let mirrored = Mat4::from_translation(Vec3::new(-4.0, 0.0, 0.0)) * Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0));
    let objects: [(&Mesh, &[(&str, Mat4)]); 2] =
        [(&ball, &[("Ball", Mat4::IDENTITY), ("Ball_MirrorX", mirrored)]), (&block, &[("Block", turned)])];
    let path = temp("places.fbx");
    export_fbx_objects(&objects, &path).unwrap();
    let raws = import_objects(&path).unwrap();

    let names: Vec<&str> = raws.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["Ball", "Ball_MirrorX", "Block"]);
    for (raw, (mesh, place)) in raws.iter().zip([(&ball, Mat4::IDENTITY), (&ball, mirrored), (&block, turned)]) {
        assert!(raw.real_size, "an FBX says its units");
        assert_eq!(raw.positions.len(), mesh.vertex_count());
        assert_eq!(raw.indices.len() / 3, mesh.triangle_count());
        // Points come in the file's order, which is the mesh's.
        for (got, was) in raw.positions.iter().zip(&mesh.positions) {
            assert!(near(*got, place.transform_point3(*was), 1e-5), "{got} against {was}");
        }
        assert!(volume(raw) > 0.0, "{}: the faces look outward, mirrored or not", raw.name);
        assert!(raw.polys.is_none() && raw.colours.is_empty() && raw.groups.is_empty());
    }
}

#[test]
fn quads_come_back_as_quads() {
    // Two quads side by side.
    let corners = vec![Vec3::ZERO, Vec3::X, Vec3::new(1.0, 1.0, 0.0), Vec3::Y, Vec3::new(2.0, 0.0, 0.0), Vec3::new(2.0, 1.0, 0.0)];
    let (mut mesh, to_new) = Mesh::new_mapped(corners.clone(), vec![0, 1, 2, 0, 2, 3, 1, 4, 5, 1, 5, 2], None);
    let quads = [[0usize, 1, 2, 3], [1, 4, 5, 2]];
    mesh.polys = Some(crate::mesh::Polys { start: vec![0, 4, 8], indices: quads.iter().flatten().map(|&i| to_new[i]).collect() });
    let path = temp("quads.fbx");
    export_fbx_objects(&[(&mesh, &[("Strip", Mat4::IDENTITY)])], &path).unwrap();
    let raw = import_objects(&path).unwrap().remove(0);
    let polys = raw.polys.as_ref().expect("the quads are kept");
    assert_eq!(polys.start, [0, 4, 8]);
    for (q, quad) in polys.start.windows(2).zip(quads) {
        let got: Vec<Vec3> = polys.indices[q[0] as usize..q[1] as usize].iter().map(|&i| raw.positions[i as usize]).collect();
        let want: Vec<Vec3> = quad.iter().map(|&i| corners[i]).collect();
        assert_eq!(got, want);
    }
    assert_eq!(raw.indices.len(), 12);
}

#[test]
fn painted_colours_come_back_on_their_vertices() {
    let mut mesh = primitives::build(Primitive::Sphere, 1_000, 0.5);
    let count = mesh.vertex_count();
    let colour = (0..count).map(|v| if mesh.positions[v].y > 0.0 { Rgba8([0, 0, 255, 255]) } else { Rgba8([0; 4]) }).collect();
    let mut stack = PaintStack::new(count);
    let data = Channels { colour: Some(colour), ..Channels::default() };
    stack.layers.push(PaintLayer { row: PaintRow::new(1, "Paint 1".into()), data });
    (stack.next_id, stack.active) = (2, Some(1));
    mesh.set_paint(Some(stack));
    let paint = ExportPaint::default();
    let painted = Painted::of(&mesh, &paint).expect("the ball is painted");
    let path = temp("painted.fbx");
    export_fbx_painted(&[(&mesh, &[("Ball", Mat4::IDENTITY)])], &paint, &path).unwrap();

    let raw = import_objects(&path).unwrap().remove(0);
    assert_eq!(raw.colours.len(), count);
    for (got, seen) in raw.colours.iter().zip(&painted.seen) {
        assert_eq!(*got, Rgba8([seen[0], seen[1], seen[2], 255]));
    }
    assert!(raw.colours.iter().any(|c| c.0 == [0, 0, 255, 255]), "the blue top is there");
}

// ---------------------------------------------------------------- files from other programs

#[test]
fn a_z_up_file_in_metres_with_a_parent_and_child_stands_up_at_its_real_size() {
    // As 3ds Max writes: Z up, Y away from the viewer, one unit a metre.
    // Packed arrays and the wide records of 7.5.
    let quad: &[i32] = &[0, 1, 2, 3];
    let top = scene(
        settings((2, 1), (1, -1), (0, 1), 100.0),
        vec![
            model(10, "Parent", vec![p3("Lcl Translation", [0.0, 0.0, 1.0]), p3("Lcl Rotation", [0.0, 0.0, 90.0])]),
            model(11, "Arm", vec![p3("Lcl Translation", [1.0, 0.0, 0.0])]),
            model(12, "Post", Vec::new()),
            geometry(20, "ArmMesh", &[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]], &[quad]),
            geometry(21, "PostMesh", &[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 0.0, 2.0], [0.0, 0.0, 2.0]], &[quad]),
        ],
        vec![link(10, 0), link(11, 10), link(12, 0), link(20, 11), link(21, 12)],
    );
    let bytes = file(7500, true, &top);
    let raws = parse(&bytes).unwrap();
    let names: Vec<&str> = raws.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["Arm", "Post"], "the parent has no mesh of its own");

    // The arm: moved 1 along X under its parent, which turns it a quarter
    // about Z and lifts it a metre; then Z up becomes Y up, in centimetres.
    let arm = &raws[0];
    let want = [[0.0, 100.0, -100.0], [0.0, 100.0, -200.0], [-100.0, 100.0, -200.0], [-100.0, 100.0, -100.0]];
    for (got, want) in arm.positions.iter().zip(want) {
        assert!(near(*got, Vec3::from(want), 1e-3), "{got} against {want:?}");
    }
    assert_eq!(arm.polys.as_ref().unwrap().start, [0, 4]);
    // It lay flat facing up in the file, and does in the app.
    let t = &arm.indices;
    let facing = (arm.positions[t[1] as usize] - arm.positions[t[0] as usize]).cross(arm.positions[t[2] as usize] - arm.positions[t[0] as usize]);
    assert!(facing.normalize().dot(Vec3::Y) > 0.99, "{facing}");

    // The post stood two metres tall along the file's Z, facing the viewer.
    let post = &raws[1];
    let tallest = post.positions.iter().map(|p| p.y).fold(f32::MIN, f32::max);
    assert!((tallest - 200.0).abs() < 1e-3, "{tallest}");
    let want = [[0.0, 0.0, 0.0], [100.0, 0.0, 0.0], [100.0, 200.0, 0.0], [0.0, 200.0, 0.0]];
    for (got, want) in post.positions.iter().zip(want) {
        assert!(near(*got, Vec3::from(want), 1e-3), "{got} against {want:?}");
    }
    let t = &post.indices;
    let facing = (post.positions[t[1] as usize] - post.positions[t[0] as usize]).cross(post.positions[t[2] as usize] - post.positions[t[0] as usize]);
    assert!(facing.normalize().dot(Vec3::Z) > 0.99, "{facing}");
    assert!(raws.iter().all(|r| r.real_size));
}

#[test]
fn a_blender_style_file_is_turned_upright_by_its_models_own_turn() {
    // Blender's default: the file says Y up, a unit is a metre (100 cm), and
    // each top model carries the -90° turn about X that stands Z-up data up.
    let top = scene(
        settings((1, 1), (2, 1), (0, 1), 100.0),
        vec![
            model(10, "Tower", vec![p3("Lcl Rotation", [-90.0, 0.0, 0.0])]),
            geometry(20, "Tower", &[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 0.0, 3.0], [0.0, 0.0, 3.0]], &[&[0, 1, 2, 3]]),
        ],
        vec![link(10, 0), link(20, 10)],
    );
    let raw = parse(&file(7400, false, &top)).unwrap().remove(0);
    assert_eq!(raw.name, "Tower");
    let tallest = raw.positions.iter().map(|p| p.y).fold(f32::MIN, f32::max);
    assert!((tallest - 300.0).abs() < 1e-3, "three metres tall along Y: {tallest}");
    assert!(raw.positions.iter().all(|p| p.z.abs() < 1e-3));
}

#[test]
fn rotation_order_counts_only_when_rotation_is_active_and_a_shared_mesh_comes_in_twice() {
    let ordered = vec![p1("RotationActive", 1), p1("RotationOrder", 5), p3("Lcl Rotation", [90.0, 0.0, 90.0])];
    // The same, but rotation not active: the order and pre-rotation are passed over.
    let passive = vec![p1("RotationOrder", 5), p3("PreRotation", [0.0, 45.0, 0.0]), p3("Lcl Rotation", [90.0, 0.0, 90.0])];
    let top = scene(
        y_up(),
        vec![
            model(10, "ZYX", ordered),
            model(11, "XYZ", passive),
            geometry(20, "Corner", &[[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], &[&[0, 1, 2]]),
        ],
        vec![link(10, 0), link(11, 0), link(20, 10), link(20, 11)],
    );
    let raws = parse(&file(7400, false, &top)).unwrap();
    assert_eq!(raws.len(), 2, "one object for each model the mesh is in");
    // ZYX turns about Z first, then X; XYZ about X first, then Z.
    let want = [[[0.0, 0.0, 1.0], [-1.0, 0.0, 0.0], [0.0, -1.0, 0.0]], [[0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]]];
    for (raw, want) in raws.iter().zip(want) {
        for (got, want) in raw.positions.iter().zip(want) {
            assert!(near(*got, Vec3::from(want), 1e-5), "{}: {got} against {want:?}", raw.name);
        }
    }
    assert_eq!((raws[0].name.as_str(), raws[1].name.as_str()), ("ZYX", "XYZ"));
}

#[test]
fn a_mirroring_place_keeps_faces_outward_and_pre_rotation_and_geometric_offsets_apply() {
    let tetra: [[f64; 3]; 4] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let faces: [&[i32]; 4] = [&[0, 2, 1], &[0, 1, 3], &[0, 3, 2], &[1, 2, 3]];
    let place = vec![
        p1("RotationActive", 1),
        p3("PreRotation", [0.0, 0.0, 90.0]),
        p3("Lcl Rotation", [90.0, 0.0, 0.0]),
        p3("Lcl Scaling", [-1.0, 1.0, 1.0]),
        p3("GeometricTranslation", [0.0, 0.0, 5.0]),
    ];
    let top = scene(
        y_up(),
        vec![model(10, "Mirror", place), model(11, "Child", vec![p3("Lcl Translation", [0.0, 0.0, 0.0])]), geometry(20, "Tetra", &tetra, &faces), geometry(21, "Tetra", &tetra, &faces)],
        vec![link(10, 0), link(11, 10), link(20, 10), link(21, 11)],
    );
    let raws = parse(&file(7500, false, &top)).unwrap();
    let mirror = &raws[0];
    // Moved 5 along Z within the model, mirrored in X, turned about X, then about Z.
    assert!(near(mirror.positions[0], Vec3::new(5.0, 0.0, 0.0), 1e-5), "{}", mirror.positions[0]);
    assert!(near(mirror.positions[1], Vec3::new(5.0, -1.0, 0.0), 1e-5), "{}", mirror.positions[1]);
    assert!(volume(mirror) > 0.0, "the mirrored faces still look outward");
    assert!(mirror.polys.is_none());

    // The child takes on the mirror's place but not its geometric offset.
    let child = &raws[1];
    assert_eq!(child.name, "Child");
    assert!(near(child.positions[0], Vec3::ZERO, 1e-5), "{}", child.positions[0]);
    assert!(near(child.positions[1], Vec3::new(0.0, -1.0, 0.0), 1e-5), "{}", child.positions[1]);
    assert!(volume(child) > 0.0);
}

#[test]
fn a_mesh_in_no_model_comes_in_under_its_own_name_and_blend_shapes_are_passed_over() {
    let triangle: [[f64; 3]; 3] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    let mut shape = geometry(21, "Smile", &triangle, &[&[0, 1, 2]]);
    shape.values[2] = s("Shape");
    let top = scene(y_up(), vec![geometry(20, "Loose", &triangle, &[&[0, 1, 2]]), shape], Vec::new());
    let raws = parse(&file(7400, true, &top)).unwrap();
    assert_eq!(raws.len(), 1);
    assert_eq!(raws[0].name, "Loose");
    assert_eq!(raws[0].positions.len(), 3);

    let nothing = scene(y_up(), vec![shape_only()], Vec::new());
    let error = parse(&file(7400, false, &nothing)).err().unwrap();
    assert!(error.contains("no mesh"), "{error}");
}

fn shape_only() -> N {
    let mut shape = geometry(21, "Smile", &[[0.0; 3]; 3], &[&[0, 1, 2]]);
    shape.values[2] = s("Shape");
    shape
}

#[test]
fn colours_are_read_with_alpha_as_cover_and_a_layer_that_says_nothing_is_no_paint() {
    let square: [[f64; 3]; 4] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]];
    let coloured = |colours: Vec<f64>, index: Vec<i32>| {
        let mut mesh = geometry(20, "Square", &square, &[&[0, 1, 2, 3]]);
        mesh.children.push(n("LayerElementColor", vec![V::I(0)], vec![
            n("MappingInformationType", vec![s("ByVertice")], Vec::new()),
            n("ReferenceInformationType", vec![s("IndexToDirect")], Vec::new()),
            n("Colors", vec![V::Doubles(colours)], Vec::new()),
            n("ColorIndex", vec![V::Ints(index)], Vec::new()),
        ]));
        parse(&file(7400, false, &scene(y_up(), vec![mesh], Vec::new()))).unwrap().remove(0)
    };
    let raw = coloured(vec![1.0, 0.0, 0.0, 1.0, 0.0, 0.5, 1.0, 0.5], vec![0, 1, 1, 7]);
    // Alpha is how much paint there is; the last index names no colour, and is no paint.
    assert_eq!(raw.colours, [Rgba8([255, 0, 0, 255]), Rgba8([0, 128, 255, 128]), Rgba8([0, 128, 255, 128]), UNPAINTED]);
    // Red on part of it, nothing on the rest, is still paint.
    let raw = coloured(vec![1.0, 0.0, 0.0, 1.0], vec![0, 0, 9, 9]);
    assert_eq!(raw.colours[..2], [Rgba8([255, 0, 0, 255]); 2]);
    // White all over, or colours with no alpha, are what an unpainted model carries.
    assert!(coloured(vec![1.0; 4], vec![0; 4]).colours.is_empty());
    assert!(coloured(vec![0.5, 0.5, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0], vec![0, 1, 0, 1]).colours.is_empty());
    // One plain colour all over is a part coloured so.
    assert_eq!(coloured(vec![0.0, 0.0, 0.0, 1.0], vec![0; 4]).colours, [Rgba8([0, 0, 0, 255]); 4]);
}

// ---------------------------------------------------------------- files it turns away

#[test]
fn a_text_fbx_is_turned_away_with_a_word_on_what_to_do() {
    let text = b"; FBX 7.4.0 project file\n; Copyright (C) 1997-2015 Autodesk Inc.\n\nFBXHeaderExtension:  {\n}\n";
    let error = parse(text).err().unwrap();
    assert!(error.contains("text kind") && error.contains("binary"), "{error}");
    let error = parse(b"solid cube\nfacet normal 0 0 1\n").err().unwrap();
    assert!(error.contains("not an FBX"), "{error}");
    assert!(parse(b"").is_err());
}

#[test]
fn an_fbx_from_before_2011_is_turned_away() {
    let mut bytes = MAGIC.to_vec();
    bytes.extend(6100u32.to_le_bytes());
    bytes.extend([0u8; 200]);
    let error = parse(&bytes).err().unwrap();
    assert!(error.contains("old kind") && error.contains("6.1"), "{error}");
}

#[test]
fn a_file_cut_short_is_an_error_and_damage_never_crashes() {
    let ball = primitives::build(Primitive::Sphere, 1_000, 0.5);
    let path = temp("cut.fbx");
    export_fbx_objects(&[(&ball, &[("Ball", Mat4::IDENTITY)])], &path).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert!(parse(&bytes).is_ok());
    // The close after the last record is at most 176 bytes, and the empty record before it 13.
    for cut in (0..bytes.len() - 200).step_by(61) {
        assert!(parse(&bytes[..cut]).is_err(), "cut at {cut}");
    }

    // Bytes changed here and there: an answer either way, never a crash.
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for _ in 0..300 {
        let mut damaged = bytes.clone();
        for _ in 0..1 + next() % 4 {
            let at = FIRST_RECORD + (next() as usize) % (damaged.len() - FIRST_RECORD);
            damaged[at] = next() as u8;
        }
        let _ = parse(&damaged);
    }

    // A packed array whose count claims far more than its bytes could hold.
    let mut top = scene(y_up(), vec![geometry(20, "Big", &[[0.0; 3]; 3], &[&[0, 1, 2]])], Vec::new());
    let bytes = file(7400, true, &top);
    assert!(parse(&bytes).is_ok());
    top[2].children[0].children[0].values[0] = V::Doubles(Vec::new());
    let mut claimed = file(7400, true, &top);
    // The Vertices array's count, at the start of its value.
    let at = claimed.windows(8).position(|w| w == b"Vertices").unwrap() + 9;
    claimed[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(parse(&claimed).is_err());
}

/// Reads the FBX files GOLEMSCULPT_FBX_FILES names (separated by `;`) and
/// prints what each holds.
#[test]
#[ignore = "reads FBX files from elsewhere on the machine"]
fn real_files_import() {
    let list = std::env::var("GOLEMSCULPT_FBX_FILES").expect("set GOLEMSCULPT_FBX_FILES");
    for path in list.split(';').filter(|p| !p.is_empty()) {
        let started = std::time::Instant::now();
        let bytes = std::fs::read(path).unwrap();
        match parse(&bytes) {
            Ok(raws) => {
                let points: usize = raws.iter().map(|r| r.positions.len()).sum();
                let triangles: usize = raws.iter().map(|r| r.indices.len() / 3).sum();
                let quads = raws.iter().filter(|r| r.polys.is_some()).count();
                let painted = raws.iter().filter(|r| !r.colours.is_empty()).count();
                println!(
                    "{path}: {} objects, {points} points, {triangles} triangles, {quads} with polygons, {painted} painted, {:?}",
                    raws.len(),
                    started.elapsed()
                );
                for raw in &raws {
                    let (lo, hi) = raw.positions.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
                    println!("    {:?}: {} points, size {:?}", raw.name, raw.positions.len(), hi - lo);
                }
            }
            Err(error) => println!("{path}: ERROR {error}"),
        }
    }
}
