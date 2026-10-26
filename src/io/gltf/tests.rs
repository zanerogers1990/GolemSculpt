use super::*;

/// A model file being put together: its binary data and the views and
/// accessors that read it.
struct Pack {
    bin: Vec<u8>,
    views: Vec<String>,
    accessors: Vec<String>,
}

fn floats(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn shorts(values: &[u16]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

impl Pack {
    fn new() -> Pack {
        Pack { bin: Vec::new(), views: Vec::new(), accessors: Vec::new() }
    }

    /// Adds `data` as a view and an accessor on it, returning the accessor's
    /// number.
    fn add(&mut self, data: &[u8], component: u32, count: usize, kind: &str, stride: Option<usize>, normalized: bool) -> usize {
        while self.bin.len() % 4 != 0 {
            self.bin.push(0);
        }
        let stride = stride.map_or(String::new(), |s| format!(r#","byteStride":{s}"#));
        self.views.push(format!(r#"{{"buffer":0,"byteOffset":{},"byteLength":{}{stride}}}"#, self.bin.len(), data.len()));
        self.bin.extend_from_slice(data);
        let normalized = if normalized { r#","normalized":true"# } else { "" };
        self.accessors.push(format!(r#"{{"bufferView":{},"componentType":{component},"count":{count},"type":"{kind}"{normalized}}}"#, self.views.len() - 1));
        self.accessors.len() - 1
    }

    fn points(&mut self, xyz: &[f32]) -> usize {
        self.add(&floats(xyz), 5126, xyz.len() / 3, "VEC3", None, false)
    }

    fn indices(&mut self, list: &[u16]) -> usize {
        self.add(&shorts(list), 5123, list.len(), "SCALAR", None, false)
    }

    /// The description, `rest` being the meshes, nodes and scenes.
    fn json(&self, uri: Option<&str>, rest: &str) -> String {
        let uri = uri.map_or(String::new(), |u| format!(r#","uri":"{u}""#));
        format!(
            r#"{{"asset":{{"version":"2.0"}},"buffers":[{{"byteLength":{}{uri}}}],"bufferViews":[{}],"accessors":[{}],{rest}}}"#,
            self.bin.len(),
            self.views.join(","),
            self.accessors.join(","),
        )
    }

    fn glb(&self, rest: &str) -> Vec<u8> {
        glb(&self.json(None, rest), &self.bin)
    }
}

/// A `.glb` from its description and binary data.
fn glb(json: &str, bin: &[u8]) -> Vec<u8> {
    let mut json = json.as_bytes().to_vec();
    while json.len() % 4 != 0 {
        json.push(b' ');
    }
    let mut bin = bin.to_vec();
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
    let total = 12 + 8 + json.len() + 8 + bin.len();
    let mut out = Vec::new();
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x4E4F_534Au32.to_le_bytes());
    out.extend_from_slice(&json);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x004E_4942u32.to_le_bytes());
    out.extend_from_slice(&bin);
    out
}

fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, &b)| n | (b as u32) << (16 - 8 * i));
        for k in 0..4 {
            if k <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * k) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn open(bytes: &[u8]) -> Result<Vec<RawMesh>> {
    parse(bytes, Path::new("model.glb"))
}

/// A mesh of one primitive with these attributes, in a node of its own.
const ONE_NODE: &str = r#""nodes":[{"mesh":0}],"scenes":[{"nodes":[0]}],"scene":0"#;

fn mesh_json(attributes: &str, extra: &str) -> String {
    format!(r#""meshes":[{{"primitives":[{{"attributes":{{{attributes}}}{extra}}}]}}]"#)
}

const TRIANGLE: [f32; 9] = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];

/// Which way a triangle faces: the z of its normal, for a flat model.
fn facing(raw: &RawMesh, t: usize) -> f32 {
    let p = |k: usize| raw.positions[raw.indices[t * 3 + k] as usize];
    (p(1) - p(0)).cross(p(2) - p(0)).z
}

fn near(a: Vec3, b: Vec3) -> bool {
    (a - b).length() < 1e-3
}

fn has_point(raw: &RawMesh, p: Vec3) -> bool {
    raw.positions.iter().any(|&q| near(q, p))
}

#[test]
fn two_triangles_with_u16_indices_make_a_quad_in_centimetres() {
    let mut pack = Pack::new();
    let p = pack.points(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0]);
    let i = pack.indices(&[0, 1, 2, 0, 2, 3]);
    let rest = format!("{},{ONE_NODE}", mesh_json(&format!(r#""POSITION":{p}"#), &format!(r#","indices":{i}"#)));
    let objects = open(&pack.glb(&rest)).unwrap();
    assert_eq!(objects.len(), 1);
    let raw = &objects[0];
    assert_eq!(raw.positions.len(), 4);
    assert_eq!(raw.indices, vec![0, 1, 2, 0, 2, 3]);
    assert!(near(raw.positions[2], Vec3::new(100.0, 100.0, 0.0)), "metres became centimetres");
    assert!(raw.real_size);
    assert!(raw.polys.is_none() && raw.groups.is_empty() && raw.colours.is_empty());
    assert_eq!(raw.name, "");
}

#[test]
fn nodes_place_two_copies_of_one_mesh_and_are_named() {
    let mut pack = Pack::new();
    let p = pack.points(&TRIANGLE);
    let half = std::f32::consts::FRAC_1_SQRT_2;
    let rest = format!(
        r#"{},"nodes":[
            {{"name":"Parent","translation":[1,0,0],"scale":[2,2,2],"children":[1,2]}},
            {{"name":"Left","mesh":0,"translation":[0,1,0]}},
            {{"mesh":0,"rotation":[0,0,{half},{half}]}}],
          "scenes":[{{"nodes":[0]}}],"scene":0"#,
        mesh_json(&format!(r#""POSITION":{p}"#), "").replace(r#"{"primitives""#, r#"{"name":"Tri","primitives""#),
    );
    let objects = open(&pack.glb(&rest)).unwrap();
    assert_eq!(objects.len(), 2, "each instance is its own object");
    assert_eq!(objects[0].name, "Left");
    assert_eq!(objects[1].name, "Tri", "a node without a name takes its mesh's");
    // Left: parent moves by 1 and doubles, the child lifts by 1 first.
    let left = &objects[0];
    for want in [Vec3::new(100.0, 200.0, 0.0), Vec3::new(300.0, 200.0, 0.0), Vec3::new(100.0, 400.0, 0.0)] {
        assert!(has_point(left, want), "{want} in {:?}", left.positions);
    }
    // Right: turned a quarter about Z (x goes to y, y to minus x), then doubled and moved by 1.
    let right = &objects[1];
    for want in [Vec3::new(100.0, 0.0, 0.0), Vec3::new(100.0, 200.0, 0.0), Vec3::new(-100.0, 0.0, 0.0)] {
        assert!(has_point(right, want), "{want} in {:?}", right.positions);
    }
    assert!(facing(left, 0) > 0.0 && facing(right, 0) > 0.0, "turning does not flip");
}

#[test]
fn a_matrix_places_a_node() {
    let mut pack = Pack::new();
    let p = pack.points(&TRIANGLE);
    let rest = format!(
        r#"{},"nodes":[{{"mesh":0,"matrix":[1,0,0,0, 0,1,0,0, 0,0,1,0, 0,0,3,1]}}],"scenes":[{{"nodes":[0]}}]"#,
        mesh_json(&format!(r#""POSITION":{p}"#), ""),
    );
    let objects = open(&pack.glb(&rest)).unwrap();
    assert!(has_point(&objects[0], Vec3::new(100.0, 0.0, 300.0)));
}

#[test]
fn a_mirroring_node_keeps_the_triangles_facing_outward() {
    let mut pack = Pack::new();
    let p = pack.points(&TRIANGLE);
    let rest = format!(
        r#"{},"nodes":[{{"mesh":0,"scale":[-1,1,1]}}],"scenes":[{{"nodes":[0]}}]"#,
        mesh_json(&format!(r#""POSITION":{p}"#), ""),
    );
    let raw = &open(&pack.glb(&rest)).unwrap()[0];
    assert!(has_point(raw, Vec3::new(-100.0, 0.0, 0.0)));
    assert!(facing(raw, 0) > 0.0, "still faces the way it did");
}

#[test]
fn the_default_scene_picks_which_nodes_are_read() {
    let mut pack = Pack::new();
    let p = pack.points(&TRIANGLE);
    let rest = format!(
        r#"{},"nodes":[{{"name":"A","mesh":0}},{{"name":"B","mesh":0}}],"scenes":[{{"nodes":[0]}},{{"nodes":[1]}}],"scene":1"#,
        mesh_json(&format!(r#""POSITION":{p}"#), ""),
    );
    let objects = open(&pack.glb(&rest)).unwrap();
    assert_eq!(objects.len(), 1);
    assert_eq!(objects[0].name, "B");
}

#[test]
fn without_scenes_every_top_node_is_read() {
    let mut pack = Pack::new();
    let p = pack.points(&TRIANGLE);
    let rest = format!(
        r#"{},"nodes":[{{"name":"Top","children":[1]}},{{"name":"Inner","mesh":0}},{{"name":"Other","mesh":0}}]"#,
        mesh_json(&format!(r#""POSITION":{p}"#), ""),
    );
    let names: Vec<String> = open(&pack.glb(&rest)).unwrap().into_iter().map(|o| o.name).collect();
    assert_eq!(names, vec!["Inner", "Other"]);
}

#[test]
fn a_triangle_strip_and_a_fan_all_face_one_way() {
    let mut pack = Pack::new();
    let strip_points = pack.points(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0]);
    let fan_points = pack.points(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0]);
    let rest = format!(
        r#""meshes":[{{"primitives":[{{"attributes":{{"POSITION":{strip_points}}},"mode":5}}]}},{{"primitives":[{{"attributes":{{"POSITION":{fan_points}}},"mode":6}}]}}],
           "nodes":[{{"mesh":0}},{{"mesh":1}}],"scenes":[{{"nodes":[0,1]}}]"#
    );
    let objects = open(&pack.glb(&rest)).unwrap();
    for raw in &objects {
        assert_eq!(raw.indices.len(), 6, "two triangles");
        assert!((0..2).all(|t| facing(raw, t) > 0.0), "{:?}", raw.indices);
    }
    // The strip's second triangle is (2, 1, 3).
    assert_eq!(objects[0].indices, vec![0, 1, 2, 2, 1, 3]);
    assert_eq!(objects[1].indices, vec![0, 1, 2, 0, 2, 3]);
}

#[test]
fn points_and_lines_have_no_surface() {
    let mut pack = Pack::new();
    let p = pack.points(&TRIANGLE);
    for mode in [0, 1, 2, 3] {
        let rest = format!("{},{ONE_NODE}", mesh_json(&format!(r#""POSITION":{p}"#), &format!(r#","mode":{mode}"#)));
        let objects = open(&pack.glb(&rest)).unwrap();
        assert!(objects.iter().all(|o| o.indices.is_empty()), "mode {mode}");
    }
}

#[test]
fn a_mesh_of_several_primitives_is_one_object_and_vertices_stay_in_place() {
    let mut pack = Pack::new();
    let a = pack.points(&TRIANGLE);
    let b = pack.points(&[0.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.0, 1.0, 1.0]);
    let rest = format!(
        r#""meshes":[{{"primitives":[{{"attributes":{{"POSITION":{a}}}}},{{"attributes":{{"POSITION":{b}}}}}]}}],{ONE_NODE}"#
    );
    let objects = open(&pack.glb(&rest)).unwrap();
    assert_eq!(objects.len(), 1);
    assert_eq!(objects[0].positions.len(), 6);
    assert_eq!(objects[0].indices, vec![0, 1, 2, 3, 4, 5], "the second primitive's corners are numbered after the first's points");
}

#[test]
fn a_seam_split_vertex_is_welded() {
    // Two triangles with their shared edge's points given twice, as an exporter does at a texture seam.
    let mut pack = Pack::new();
    let p = pack.points(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.0, 0.0, 2.0, 0.0, 0.0, 1.0, 1.0, 0.0]);
    let rest = format!("{},{ONE_NODE}", mesh_json(&format!(r#""POSITION":{p}"#), ""));
    let raw = &open(&pack.glb(&rest)).unwrap()[0];
    assert_eq!(raw.positions.len(), 4);
    assert_eq!(raw.indices.len(), 6);
}

#[test]
fn texcoords_are_read_turned_up_and_a_seam_keeps_both_sides_through_the_weld() {
    // Two triangles with their shared edge's points given twice, each side with UVs of its own.
    let mut pack = Pack::new();
    let p = pack.points(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.0, 0.0, 2.0, 0.0, 0.0, 1.0, 1.0, 0.0]);
    let t = pack.add(&floats(&[0.0, 0.0, 0.5, 0.0, 0.5, 0.25, 0.75, 1.0, 1.0, 1.0, 0.75, 0.5]), 5126, 6, "VEC2", None, false);
    let rest = format!("{},{ONE_NODE}", mesh_json(&format!(r#""POSITION":{p},"TEXCOORD_0":{t}"#), ""));
    let raw = &open(&pack.glb(&rest)).unwrap()[0];
    assert_eq!(raw.positions.len(), 4, "the seam is welded");
    let uv = |u: f32, v: f32| Vec2::new(u, v);
    // V runs down a glTF picture and up the app's.
    assert_eq!(raw.uvs, [uv(0.0, 1.0), uv(0.5, 1.0), uv(0.5, 0.75), uv(0.75, 0.0), uv(1.0, 0.0), uv(0.75, 0.5)]);
    assert_eq!(raw.indices[1], raw.indices[3], "one vertex, with a UV on each side");
}

#[test]
fn normalized_texcoords_are_read_and_a_primitive_without_any_has_none() {
    let mut pack = Pack::new();
    let a = pack.points(&TRIANGLE);
    let b = pack.points(&[0.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.0, 1.0, 1.0]);
    let t = pack.add(&shorts(&[0, 0, 65535, 0, 0, 65535]), 5123, 3, "VEC2", None, true);
    let rest = format!(
        r#""meshes":[{{"primitives":[{{"attributes":{{"POSITION":{b}}}}},{{"attributes":{{"POSITION":{a},"TEXCOORD_0":{t}}}}}]}}],{ONE_NODE}"#
    );
    let raw = &open(&pack.glb(&rest)).unwrap()[0];
    let uv = |u: f32, v: f32| Vec2::new(u, v);
    assert_eq!(raw.uvs, [Vec2::ZERO, Vec2::ZERO, Vec2::ZERO, uv(0.0, 1.0), uv(1.0, 1.0), uv(0.0, 0.0)]);
    // UVs of a kind not read (three numbers each) are passed over, and the model still opens.
    let mut pack = Pack::new();
    let p = pack.points(&TRIANGLE);
    let t = pack.add(&floats(&[0.0; 9]), 5126, 3, "VEC3", None, false);
    let rest = format!("{},{ONE_NODE}", mesh_json(&format!(r#""POSITION":{p},"TEXCOORD_0":{t}"#), ""));
    let raw = &open(&pack.glb(&rest)).unwrap()[0];
    assert!(raw.uvs.is_empty() && raw.indices.len() == 3);
}

#[test]
fn colours_as_normalized_bytes_become_srgb_paint() {
    let mut pack = Pack::new();
    let p = pack.points(&TRIANGLE);
    let colours: Vec<u8> = vec![255, 0, 0, 255, 0, 255, 0, 128, 128, 128, 0, 255];
    let c = pack.add(&colours, 5121, 3, "VEC4", None, true);
    let rest = format!("{},{ONE_NODE}", mesh_json(&format!(r#""POSITION":{p},"COLOR_0":{c}"#), ""));
    let raw = &open(&pack.glb(&rest)).unwrap()[0];
    assert_eq!(raw.colours.len(), 3);
    assert_eq!(raw.colours[0], Rgba8([255, 0, 0, 255]));
    assert_eq!(raw.colours[1], Rgba8([0, 255, 0, 128]), "alpha is kept as it is");
    // Half linear light is about 188 of 255 in sRGB.
    let half = raw.colours[2].0;
    assert!(half[0].abs_diff(188) <= 1 && half[1].abs_diff(188) <= 1 && half[2] == 0 && half[3] == 255, "{half:?}");
}

#[test]
fn float_rgb_colours_are_read_and_a_primitive_without_any_is_white() {
    let mut pack = Pack::new();
    let a = pack.points(&TRIANGLE);
    let b = pack.points(&[0.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.0, 1.0, 1.0]);
    let c = pack.add(&floats(&[1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0]), 5126, 3, "VEC3", None, false);
    let rest = format!(
        r#""meshes":[{{"primitives":[{{"attributes":{{"POSITION":{b}}}}},{{"attributes":{{"POSITION":{a},"COLOR_0":{c}}}}}]}}],{ONE_NODE}"#
    );
    let raw = &open(&pack.glb(&rest)).unwrap()[0];
    assert_eq!(raw.colours.len(), 6);
    assert_eq!(raw.colours[0], Rgba8([255; 4]));
    assert_eq!(raw.colours[3], Rgba8([255, 0, 0, 255]));
}

#[test]
fn interleaved_points_are_read_with_their_stride_and_offsets() {
    // Each vertex: x y z then one float of something else, so a stride of 16, and the view starts 4 bytes in.
    let mut data = vec![0u8; 4];
    for v in [[0.0f32, 0.0, 0.0, 9.0], [1.0, 0.0, 0.0, 9.0], [0.0, 1.0, 0.0, 9.0]] {
        data.extend(floats(&v));
    }
    let mut pack = Pack::new();
    let p = pack.add(&data, 5126, 3, "VEC3", Some(16), false);
    // The accessor itself starts 4 bytes into its view.
    pack.accessors[p] = pack.accessors[p].replace(r#""count""#, r#""byteOffset":4,"count""#);
    let rest = format!("{},{ONE_NODE}", mesh_json(&format!(r#""POSITION":{p}"#), ""));
    let raw = &open(&pack.glb(&rest)).unwrap()[0];
    assert!(near(raw.positions[1], Vec3::new(100.0, 0.0, 0.0)));
    assert!(near(raw.positions[2], Vec3::new(0.0, 100.0, 0.0)));
}

#[test]
fn u8_and_u32_indices_are_read() {
    let mut pack = Pack::new();
    let p = pack.points(&TRIANGLE);
    let small = pack.add(&[0, 2, 1], 5121, 3, "SCALAR", None, false);
    let big = pack.add(&[0u32, 1, 2].iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>(), 5125, 3, "SCALAR", None, false);
    for (i, want) in [(small, vec![0, 2, 1]), (big, vec![0, 1, 2])] {
        let rest = format!("{},{ONE_NODE}", mesh_json(&format!(r#""POSITION":{p}"#), &format!(r#","indices":{i}"#)));
        assert_eq!(open(&pack.glb(&rest)).unwrap()[0].indices, want);
    }
}

#[test]
fn a_gltf_can_carry_its_data_as_a_base64_address() {
    let mut pack = Pack::new();
    let p = pack.points(&TRIANGLE);
    let i = pack.indices(&[0, 1, 2]);
    let rest = format!("{},{ONE_NODE}", mesh_json(&format!(r#""POSITION":{p}"#), &format!(r#","indices":{i}"#)));
    let uri = format!("data:application/octet-stream;base64,{}", base64(&pack.bin));
    let json = pack.json(Some(&uri), &rest);
    let raw = &parse(json.as_bytes(), Path::new("model.gltf")).unwrap()[0];
    assert_eq!(raw.positions.len(), 3);
    assert_eq!(raw.indices, vec![0, 1, 2]);
}

#[test]
fn the_base64_reader_handles_every_padding() {
    for n in 0..9usize {
        let data: Vec<u8> = (0..n as u8).map(|b| b.wrapping_mul(37).wrapping_add(200)).collect();
        assert_eq!(base64_decoded(&base64(&data)).unwrap(), data, "{n} bytes");
    }
    assert!(base64_decoded("ab*d").is_err());
}

#[test]
fn a_gltf_can_read_its_data_from_a_file_beside_it() {
    let dir = std::env::temp_dir().join("sculpt_io_gltf_tests").join(format!("beside_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut pack = Pack::new();
    let p = pack.points(&TRIANGLE);
    let rest = format!("{},{ONE_NODE}", mesh_json(&format!(r#""POSITION":{p}"#), ""));
    // A name with a space, written the way a file address has to.
    std::fs::write(dir.join("my model.bin"), &pack.bin).unwrap();
    let json = pack.json(Some("my%20model.bin"), &rest);
    let path = dir.join("model.gltf");
    std::fs::write(&path, &json).unwrap();
    let objects = parse(json.as_bytes(), &path).unwrap();
    assert_eq!(objects[0].indices, vec![0, 1, 2]);
    // The same through the importer's front door.
    let objects = crate::io::import_objects(&path).unwrap();
    assert_eq!(objects.len(), 1);
    // Without the data file the message names it.
    std::fs::remove_file(dir.join("my model.bin")).unwrap();
    let err = parse(json.as_bytes(), &path).map(|_| ()).unwrap_err();
    assert!(err.contains("my model.bin"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_web_address_for_data_is_explained() {
    let mut pack = Pack::new();
    let p = pack.points(&TRIANGLE);
    let rest = format!("{},{ONE_NODE}", mesh_json(&format!(r#""POSITION":{p}"#), ""));
    let json = pack.json(Some("https://example.com/model.bin"), &rest);
    let err = parse(json.as_bytes(), Path::new("model.gltf")).map(|_| ()).unwrap_err();
    assert!(err.contains("web address"), "{err}");
}

#[test]
fn a_truncated_glb_is_an_error_not_a_crash() {
    let mut pack = Pack::new();
    let p = pack.points(&TRIANGLE);
    let rest = format!("{},{ONE_NODE}", mesh_json(&format!(r#""POSITION":{p}"#), ""));
    let whole = pack.glb(&rest);
    assert!(open(&whole).is_ok());
    // Every possible cut of the file is an error or a model, and never a panic.
    for length in 0..whole.len() {
        let result = open(&whole[..length]);
        assert!(result.is_err(), "cut at {length} of {}", whole.len());
    }
    let err = open(&whole[..whole.len() - 10]).map(|_| ()).unwrap_err();
    assert!(err.contains("cut short"), "{err}");
}

#[test]
fn damaged_numbers_are_errors() {
    let mut pack = Pack::new();
    let p = pack.points(&TRIANGLE);
    let good = format!("{},{ONE_NODE}", mesh_json(&format!(r#""POSITION":{p}"#), ""));
    // Points that run past the data.
    let json = pack.json(None, &good).replace(r#""count":3"#, r#""count":3000"#);
    assert!(open(&glb(&json, &pack.bin)).is_err());
    // A count so big that it would overflow.
    let json = pack.json(None, &good).replace(r#""count":3"#, r#""count":18446744073709551615"#);
    assert!(open(&glb(&json, &pack.bin)).is_err());
    // A mesh, view and buffer that are not there.
    for (from, to) in [(r#""mesh":0"#, r#""mesh":7"#), (r#""bufferView":0"#, r#""bufferView":4"#), (r#""buffer":0"#, r#""buffer":3"#)] {
        let json = pack.json(None, &good).replace(from, to);
        assert!(open(&glb(&json, &pack.bin)).is_err(), "{from}");
    }
    // Not a model at all.
    assert!(open(b"hello").is_err());
    assert!(parse(b"{ \"nodes\": 5 }", Path::new("a.gltf")).is_err());
}

#[test]
fn an_index_past_the_points_is_an_error() {
    let mut pack = Pack::new();
    let p = pack.points(&TRIANGLE);
    let i = pack.indices(&[0, 1, 3]);
    let rest = format!("{},{ONE_NODE}", mesh_json(&format!(r#""POSITION":{p}"#), &format!(r#","indices":{i}"#)));
    let err = open(&pack.glb(&rest)).map(|_| ()).unwrap_err();
    assert!(err.contains("point that is not there"), "{err}");
}

#[test]
fn nodes_that_hold_each_other_are_an_error() {
    let mut pack = Pack::new();
    let p = pack.points(&TRIANGLE);
    let rest = format!(
        r#"{},"nodes":[{{"mesh":0,"children":[1]}},{{"children":[0]}}],"scenes":[{{"nodes":[0]}}]"#,
        mesh_json(&format!(r#""POSITION":{p}"#), ""),
    );
    assert!(open(&pack.glb(&rest)).is_err());
}

#[test]
fn a_draco_model_asks_for_an_uncompressed_export() {
    let mut pack = Pack::new();
    let p = pack.points(&TRIANGLE);
    let rest = format!(
        r#""meshes":[{{"primitives":[{{"attributes":{{"POSITION":{p}}},"extensions":{{"KHR_draco_mesh_compression":{{"bufferView":0,"attributes":{{"POSITION":0}}}}}}}}]}}],{ONE_NODE}"#
    );
    let err = open(&pack.glb(&rest)).map(|_| ()).unwrap_err();
    assert!(err.contains("compress") && err.contains("without compression"), "{err}");
}

#[test]
fn a_meshopt_view_and_a_sparse_accessor_are_explained() {
    let mut pack = Pack::new();
    let p = pack.points(&TRIANGLE);
    let rest = format!("{},{ONE_NODE}", mesh_json(&format!(r#""POSITION":{p}"#), ""));
    let mut meshopt = pack.json(None, &rest);
    meshopt = meshopt.replace(r#""byteLength":36"#, r#""byteLength":36,"extensions":{"EXT_meshopt_compression":{"buffer":0,"byteLength":4,"byteStride":12,"count":3,"mode":"ATTRIBUTES"}}"#);
    let err = open(&glb(&meshopt, &pack.bin)).map(|_| ()).unwrap_err();
    assert!(err.contains("compressed"), "{err}");

    let sparse = pack.json(None, &rest).replace(r#""count":3"#, r#""count":3,"sparse":{"count":1}"#);
    let err = open(&glb(&sparse, &pack.bin)).map(|_| ()).unwrap_err();
    assert!(err.contains("sparse"), "{err}");
}

#[test]
fn a_compressed_mesh_nothing_uses_does_not_stop_the_rest() {
    let mut pack = Pack::new();
    let p = pack.points(&TRIANGLE);
    let rest = format!(
        r#""meshes":[{{"primitives":[{{"attributes":{{"POSITION":{p}}}}}]}},{{"primitives":[{{"attributes":{{"POSITION":{p}}},"extensions":{{"KHR_draco_mesh_compression":{{}}}}}}]}}],
           "nodes":[{{"mesh":0}},{{"mesh":1}}],"scenes":[{{"nodes":[0]}},{{"nodes":[1]}}],"scene":0"#
    );
    assert_eq!(open(&pack.glb(&rest)).unwrap().len(), 1);
}

#[test]
fn a_glb_with_a_version_one_header_is_explained() {
    let mut pack = Pack::new();
    let p = pack.points(&TRIANGLE);
    let rest = format!("{},{ONE_NODE}", mesh_json(&format!(r#""POSITION":{p}"#), ""));
    let mut bytes = pack.glb(&rest);
    bytes[4] = 1;
    assert!(open(&bytes).unwrap_err().contains("2.0"));
}
