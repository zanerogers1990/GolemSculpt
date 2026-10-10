//! The reader against real sculpts from the Unity app.
//!
//! They are a person's work, so they are not in the repository, and they
//! are large (hundreds of megabytes each), so the test is run on request:
//!
//! ```text
//! SCULPT_UNITY_SAMPLES=folder cargo test unity_import -- --nocapture
//! ```
//!
//! It takes every `.sculpt` in that folder, and prints what each holds.
//! Without the variable, or with no sculpt there, it says so and passes.
//! Give it copies: it only reads them, but a copy cannot be harmed. The
//! `.sclp` it saves on the way goes in a `roundtrip` folder beside the
//! samples' own, and is removed again.
//!
//! Each file is first read here by a second, plain reader that follows
//! `SceneSerializer.cs` and does nothing else, so what the file itself says
//! (its counts, its bounds, how closed it is, which way it faces, what the
//! Unity camera saw) is not taken from the reader under test. Then the two
//! are compared, object by object.

use std::path::{Path, PathBuf};
use std::time::Instant;

use glam::{DVec2, DVec3, Mat4, Quat, Vec3};
use serde_json::Value;

use super::super::*;
use crate::camera::Camera;

fn samples() -> Vec<PathBuf> {
    let Some(folder) = std::env::var_os("SCULPT_UNITY_SAMPLES").map(PathBuf::from) else {
        println!("skipped: set SCULPT_UNITY_SAMPLES to a folder of the Unity app's .sculpt files to check them");
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = std::fs::read_dir(&folder)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| is_unity(path))
        .collect();
    found.sort();
    if found.is_empty() {
        println!("skipped: there are no .sculpt files in {}", folder.display());
    }
    found
}

/// One object exactly as the file has it.
struct Plain {
    entry: Value,
    vertices: Vec<Vec3>,
    normals: Vec<Vec3>,
    triangles: Vec<u32>,
    mask: Option<Vec<f32>>,
    hidden: Option<Vec<u8>>,
}

/// The second reader: the file's layout, and nothing clever.
fn read_plainly(path: &Path) -> (Value, Vec<Plain>, i32) {
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(&bytes[..8], b"SCLPTSV\0");
    let int = |at: usize| i32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    let (version, json_length) = (int(8), int(12) as usize);
    let root: Value = serde_json::from_slice(&bytes[16..16 + json_length]).unwrap();
    let mut at = 16 + json_length;
    let mut take = |count: usize| {
        let chunk = &bytes[at..at + count];
        at += count;
        chunk
    };
    let floats = |chunk: &[u8]| -> Vec<f32> {
        chunk.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect()
    };
    let vectors =
        |chunk: &[u8]| -> Vec<Vec3> { floats(chunk).chunks_exact(3).map(|c| Vec3::new(c[0], c[1], c[2])).collect() };
    let mut objects = Vec::new();
    for entry in root["objects"].as_array().unwrap() {
        let verts = entry["vertexCount"].as_u64().unwrap() as usize;
        let corners = entry["triangleIndexCount"].as_u64().unwrap() as usize;
        let vertices = vectors(take(verts * 12));
        let normals = vectors(take(verts * 12));
        let triangles = take(corners * 4).chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
        let mask = entry["hasMask"].as_bool().unwrap_or(false).then(|| floats(take(verts * 4)));
        let hidden = entry["hasHidden"].as_bool().unwrap_or(false).then(|| take(corners / 3).to_vec());
        objects.push(Plain { entry: entry.clone(), vertices, normals, triangles, mask, hidden });
    }
    assert_eq!(at, bytes.len(), "the arrays fill the file exactly");
    (root, objects, version)
}

fn vector(value: &Value) -> Vec3 {
    let part = |key: &str| value[key].as_f64().unwrap_or(0.0) as f32;
    Vec3::new(part("x"), part("y"), part("z"))
}

fn turn(value: &Value) -> Quat {
    Quat::from_xyzw(
        value["x"].as_f64().unwrap() as f32,
        value["y"].as_f64().unwrap() as f32,
        value["z"].as_f64().unwrap() as f32,
        value["w"].as_f64().unwrap() as f32,
    )
}

/// How a surface is put together: edges with one triangle on them (a hole's
/// rim), edges with more than two (a seam no solid has), and triangles with
/// two corners the same.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
struct Topology {
    boundary_edges: usize,
    crowded_edges: usize,
    collapsed_triangles: usize,
}

fn topology(triangles: &[u32]) -> Topology {
    let sound = |t: &[u32]| t[0] != t[1] && t[1] != t[2] && t[0] != t[2];
    let mut edges: Vec<u64> = triangles
        .par_chunks_exact(3)
        .filter(|t| sound(t))
        .flat_map_iter(|t| [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])])
        .map(|(a, b)| (a.min(b) as u64) << 32 | a.max(b) as u64)
        .collect();
    edges.par_sort_unstable();
    let (mut boundary_edges, mut crowded_edges) = (0, 0);
    for run in edges.chunk_by(|a, b| a == b) {
        boundary_edges += (run.len() == 1) as usize;
        crowded_edges += (run.len() > 2) as usize;
    }
    Topology {
        boundary_edges,
        crowded_edges,
        collapsed_triangles: triangles.chunks_exact(3).filter(|t| !sound(t)).count(),
    }
}

/// Six times the volume enclosed: positive when the cross product of each
/// triangle's first two edges points out of the solid.
fn six_volumes(points: &[Vec3], triangles: &[u32]) -> f64 {
    let origin = points.first().copied().unwrap_or(Vec3::ZERO).as_dvec3();
    triangles
        .par_chunks_exact(3)
        .map(|t| {
            let p = |i: u32| points[i as usize].as_dvec3() - origin;
            p(t[0]).dot(p(t[1]).cross(p(t[2])))
        })
        .sum()
}

/// The box round a set of world points, and their centre of mass.
fn extent(points: impl ParallelIterator<Item = DVec3>) -> (DVec3, DVec3, DVec3, usize) {
    let (lo, hi, sum, count) = points
        .fold(
            || (DVec3::INFINITY, DVec3::NEG_INFINITY, DVec3::ZERO, 0usize),
            |(lo, hi, sum, count), p| (lo.min(p), hi.max(p), sum + p, count + 1),
        )
        .reduce(
            || (DVec3::INFINITY, DVec3::NEG_INFINITY, DVec3::ZERO, 0usize),
            |a, b| (a.0.min(b.0), a.1.max(b.1), a.2 + b.2, a.3 + b.3),
        );
    (lo, hi, sum / count.max(1) as f64, count)
}

/// What a set of points looks like on screen, in numbers that a mirrored
/// picture cannot share: where it is, how it is spread, and which way it
/// leans (the mixed moments change sign when left and right are swapped).
#[derive(Debug, Clone, Copy)]
struct Picture {
    lo: DVec2,
    hi: DVec2,
    mean: DVec2,
    /// Mean of x*y, x*x*y and x*x*x about the picture's centre.
    lean: [f64; 3],
}

fn picture(points: impl ParallelIterator<Item = DVec2>) -> Picture {
    let zero = || (DVec2::INFINITY, DVec2::NEG_INFINITY, DVec2::ZERO, [0.0f64; 3], 0usize);
    let (lo, hi, sum, lean, count) = points
        .fold(zero, |(lo, hi, sum, lean, count), p| {
            (
                lo.min(p),
                hi.max(p),
                sum + p,
                [lean[0] + p.x * p.y, lean[1] + p.x * p.x * p.y, lean[2] + p.x * p.x * p.x],
                count + 1,
            )
        })
        .reduce(zero, |a, b| {
            (a.0.min(b.0), a.1.max(b.1), a.2 + b.2, [a.3[0] + b.3[0], a.3[1] + b.3[1], a.3[2] + b.3[2]], a.4 + b.4)
        });
    let n = count.max(1) as f64;
    Picture { lo, hi, mean: sum / n, lean: lean.map(|v| v / n) }
}

impl Picture {
    fn differs_from(&self, other: &Picture) -> f64 {
        let lean = (0..3).map(|k| (self.lean[k] - other.lean[k]).abs()).fold(0.0, f64::max);
        (self.lo - other.lo)
            .abs()
            .max_element()
            .max((self.hi - other.hi).abs().max_element())
            .max((self.mean - other.mean).abs().max_element())
            .max(lean)
    }

    /// The same picture with left and right swapped.
    fn mirrored(&self) -> Picture {
        Picture {
            lo: DVec2::new(-self.hi.x, self.lo.y),
            hi: DVec2::new(-self.lo.x, self.hi.y),
            mean: DVec2::new(-self.mean.x, self.mean.y),
            lean: [-self.lean[0], self.lean[1], -self.lean[2]],
        }
    }
}

/// Where the Unity app's camera drew a point of its world, in doubles:
/// `CameraOrbitController.UpdateTransform` with its 60 degree lens.
fn unity_screen(view: &Value, aspect: f64) -> impl Fn(DVec3) -> DVec2 + Sync {
    let number = |key: &str| view[key].as_f64().unwrap();
    let (yaw, pitch, distance) = (number("yaw").to_radians(), number("pitch").to_radians(), number("distance"));
    let ortho = view["orthographic"].as_bool().unwrap_or(false);
    let pivot = vector(&view["pivot"]).as_dvec3();
    let turn = glam::DQuat::from_rotation_y(yaw) * glam::DQuat::from_rotation_x(pitch);
    let tan = (60.0f64.to_radians() * 0.5).tan();
    let eye = pivot + turn * DVec3::new(0.0, 0.0, -distance);
    let back = turn.inverse();
    move |point| {
        let seen = back * (point - eye);
        let half = if ortho { distance * tan } else { seen.z * tan };
        DVec2::new(seen.x / (half * aspect), seen.y / half)
    }
}

/// The same for this app's camera, once it has taken the opened file's view.
fn app_screen(view: &SavedView, aspect: f32) -> impl Fn(Vec3) -> DVec2 + Sync {
    let mut camera = Camera::default();
    assert!(camera.restore_view(view));
    let size = glam::Vec2::new(1000.0 * aspect, 1000.0);
    let seen = camera.view(glam::Vec2::ZERO, size, camera.distance, 10.0);
    move |point| {
        let clip = seen.view_proj.as_dmat4() * point.as_dvec3().extend(1.0);
        DVec2::new(clip.x / clip.w, clip.y / clip.w)
    }
}

/// Opens every sample, checks it against what the file itself says, and
/// takes it through a save as a `.sclp` and back. One file at a time: they
/// run to hundreds of megabytes each.
#[test]
fn every_sample_opens_as_the_file_says_and_survives_a_save() {
    for path in samples() {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let untouched =
            (std::fs::metadata(&path).unwrap().len(), std::fs::metadata(&path).unwrap().modified().unwrap());
        let megabytes = untouched.0 as f64 / 1e6;
        let (root, plain, version) = read_plainly(&path);
        let began = Instant::now();
        let opened = load(&path).unwrap_or_else(|e| panic!("{e}"));
        let took = began.elapsed().as_secs_f64();
        let scene = &opened.scene;
        println!(
            "\n== {name}: {megabytes:.1} MB, format {version}, opened in {took:.2} s ({:.0} MB/s)",
            megabytes / took
        );
        println!("   saved {} by Unity {}", root["savedUtc"], root["unityVersion"]);

        // Rows: each object, then its instances. An array adds no rows: it is the object's own.
        let mut row = 0;
        let mut first_rows = Vec::new();
        for (index, source) in plain.iter().enumerate() {
            let entry = &source.entry;
            first_rows.push(row);
            let object = &scene.objects[row];
            let mesh = &scene.meshes[object.mesh];
            assert_eq!(object.mesh, index, "one mesh per object of the file, in its order");
            assert_eq!(object.name, entry["name"].as_str().unwrap());
            assert_eq!(object.visible, entry["visible"].as_bool().unwrap());

            // Counts. A build drops vertices no triangle uses and triangles
            // with two corners the same, so those are counted in the file first.
            let shape = topology(&source.triangles);
            let mut used = vec![false; source.vertices.len()];
            source.triangles.iter().for_each(|&i| used[i as usize] = true);
            let unused = used.iter().filter(|u| !**u).count();
            assert_eq!(mesh.vertex_count() + unused, source.vertices.len(), "{name} / {}: vertices", object.name);
            assert_eq!(
                mesh.triangle_count() + shape.collapsed_triangles,
                source.triangles.len() / 3,
                "{name} / {}: triangles",
                object.name
            );

            // As closed as it was, and facing the same way.
            let now = topology(&mesh.indices);
            assert_eq!(
                (now.boundary_edges, now.crowded_edges),
                (shape.boundary_edges, shape.crowded_edges),
                "{name} / {}",
                object.name
            );
            let (then, volume) = (
                six_volumes(&source.vertices, &source.triangles) / 6.0,
                six_volumes(&mesh.positions, &mesh.indices) / 6.0,
            );
            assert!(
                (volume - then).abs() <= 1e-4 * then.abs(),
                "{name} / {}: volume {volume} against {then}",
                object.name
            );
            // The file's own normals say which side Unity lit: its faces look
            // the way the cross product of their edges does.
            let agree = source
                .triangles
                .par_chunks_exact(3)
                .filter(|t| {
                    let [a, b, c] = [0, 1, 2].map(|k| source.vertices[t[k] as usize]);
                    let lit =
                        source.normals[t[0] as usize] + source.normals[t[1] as usize] + source.normals[t[2] as usize];
                    (b - a).cross(c - a).dot(lit) > 0.0
                })
                .count();
            let faces = source.triangles.len() / 3 - shape.collapsed_triangles;
            assert!(
                agree as f64 > 0.995 * faces as f64,
                "{name} / {}: only {agree} of {faces} faces look the way their normals do",
                object.name
            );
            assert!(then > 0.0 && volume > 0.0, "{name} / {}: outward, in the file and here", object.name);

            // Where it is: the file's vertices through Unity's own transform
            // rule, with X negated as Unity's exporters did, against this
            // app's vertices through this app's.
            let (position, rotation, scale) =
                (vector(&entry["position"]), turn(&entry["rotation"]), vector(&entry["scale"]));
            let unity = Mat4::from_scale_rotation_translation(scale, rotation, position).as_dmat4();
            let (lo, hi, centre, _) = extent(
                source
                    .vertices
                    .par_iter()
                    .zip(&used)
                    .filter(|(_, used)| **used)
                    .map(|(v, _)| unity.transform_point3(v.as_dvec3()))
                    .map(|w| DVec3::new(-w.x, w.y, w.z)),
            );
            let here = Mat4::from_scale_rotation_translation(object.scale, object.rotation, object.position).as_dmat4();
            let (lo_here, hi_here, centre_here, _) =
                extent(mesh.positions.par_iter().map(|p| here.transform_point3(p.as_dvec3())));
            let size = (hi - lo).max_element();
            let off = (lo - lo_here)
                .abs()
                .max_element()
                .max((hi - hi_here).abs().max_element())
                .max((centre - centre_here).abs().max_element());
            assert!(off < 1e-5 * size.max(1.0), "{name} / {}: off by {off}", object.name);

            // Mask and hidden faces.
            let masked =
                |mask: &[f32]| (mask.iter().filter(|&&m| m > 0.0).count(), mask.iter().map(|&m| m as f64).sum::<f64>());
            let (masked_then, weight_then) = source.mask.as_deref().map_or((0, 0.0), masked);
            let (masked_now, weight_now) = masked(&mesh.mask);
            assert_eq!(masked_now, masked_then, "{name} / {}: masked vertices", object.name);
            assert!((weight_now - weight_then).abs() < 1e-6 * weight_then.max(1.0));
            let hidden_then = source.hidden.as_deref().map_or(0, |flags| flags.iter().filter(|&&f| f != 0).count());
            assert_eq!(mesh.hidden_triangle_count(), hidden_then, "{name} / {}: hidden faces", object.name);

            println!(
                "   [{row}] \"{}\"{}: {} vertices, {} triangles; {} open edges; volume {volume:.4}; {} masked ({:.1}%); {} hidden",
                object.name,
                if object.visible { "" } else { " (hidden)" },
                mesh.vertex_count(),
                mesh.triangle_count(),
                now.boundary_edges,
                masked_now,
                100.0 * masked_now as f64 / mesh.vertex_count() as f64,
                hidden_then,
            );
            println!(
                "        box {:.4?} to {:.4?} ({:.1} x {:.1} x {:.1} mm); at {:.4?}, scale {:.4?}",
                lo_here.to_array(),
                hi_here.to_array(),
                (hi_here.x - lo_here.x) * 10.0,
                (hi_here.y - lo_here.y) * 10.0,
                (hi_here.z - lo_here.z) * 10.0,
                object.position.to_array(),
                object.scale.to_array(),
            );
            if unused + shape.collapsed_triangles > 0 {
                println!(
                    "        ({unused} vertices no triangle used and {} collapsed triangles were dropped)",
                    shape.collapsed_triangles
                );
            }

            // Its instances, and its live mirror.
            let instances = entry["instances"].as_array().map_or(0, Vec::len);
            let axes = entry["mirrorRepeaterAxes"].as_i64().unwrap_or(0) & 7;
            let mirror = crate::scene::saved_mirror(&scene.details, row);
            assert_eq!(mirror.map_or(0, |m| m.axis_mask as i64), axes, "{name} / {}: live mirror", object.name);
            if let Some(mirror) = mirror {
                let centre = vector(&entry["mirrorRepeaterCenter"]);
                assert_eq!(mirror.center, Vec3::new(-centre.x, centre.y, centre.z));
                println!(
                    "        live mirror across {} through {:?}",
                    sculpt_symmetry::MirrorRepeater::axis_name_of(mirror.axis_mask),
                    mirror.center.to_array()
                );
            }
            for linked in 1..=instances {
                let copy = &scene.objects[row + linked];
                assert_eq!(copy.mesh, index, "{name}: \"{}\" shares the mesh of \"{}\"", copy.name, object.name);
                assert!(!copy.name.contains("(Array"), "{name}: a copy of an array came in as a linked copy");
                println!(
                    "   [{}] \"{}\": linked copy of \"{}\" at {:.4?}",
                    row + linked,
                    copy.name,
                    object.name,
                    copy.position.to_array()
                );
            }
            live_array_sits_where_unity_drew_it(&name, entry, scene, row, source, &opened.extras.objects[index]);
            row += 1 + instances;
        }
        assert_eq!(row, scene.objects.len(), "{name}: every row accounted for");
        let selected = root["primarySelectionIndex"].as_i64().unwrap_or(-1).clamp(0, plain.len() as i64 - 1) as usize;
        assert_eq!(scene.active, first_rows[selected]);
        println!("   selected: \"{}\"", scene.objects[scene.active].name);

        // The picture from the saved camera, object by object: this app's
        // against the Unity app's, and against its mirror image.
        if let Some(view) = scene.details.get::<SavedView>(io::slot::CAMERA) {
            let aspect = 1.6;
            let (unity, here) = (unity_screen(&root["camera"], aspect), app_screen(&view, aspect as f32));
            for (index, source) in plain.iter().enumerate() {
                let entry = &source.entry;
                let object = &scene.objects[first_rows[index]];
                let placed = Mat4::from_scale_rotation_translation(
                    vector(&entry["scale"]),
                    turn(&entry["rotation"]),
                    vector(&entry["position"]),
                )
                .as_dmat4();
                let mut used = vec![false; source.vertices.len()];
                source.triangles.iter().for_each(|&i| used[i as usize] = true);
                let theirs = picture(
                    source
                        .vertices
                        .par_iter()
                        .zip(&used)
                        .filter(|(_, used)| **used)
                        .map(|(v, _)| unity(placed.transform_point3(v.as_dvec3()))),
                );
                let model = Mat4::from_scale_rotation_translation(object.scale, object.rotation, object.position);
                let ours =
                    picture(scene.meshes[object.mesh].positions.par_iter().map(|p| here(model.transform_point3(*p))));
                let (same, flipped) = (ours.differs_from(&theirs), ours.differs_from(&theirs.mirrored()));
                println!(
                    "   picture of \"{}\": differs from Unity's by {same:.2e}, from its mirror image by {flipped:.2e} (screen box x {:.3} to {:.3})",
                    object.name, ours.lo.x, ours.hi.x
                );
                assert!(same < 2e-4, "{name} / {}: the picture differs from Unity's by {same}", object.name);
            }
            println!(
                "   view: yaw {} (Unity's {}), pitch {}, distance {:.4}, pivot {:.4?}, {}",
                view.yaw,
                root["camera"]["yaw"],
                view.pitch,
                view.distance,
                view.pivot,
                if view.ortho { "orthographic" } else { "perspective" }
            );
        } else {
            println!("   no saved view: the app frames the model");
        }

        let look: Look = scene.details.get_over(io::slot::LOOK, &Look::default()).unwrap();
        let shading: Shading = scene.details.get(io::slot::SHADING).unwrap();
        let mirror: Mirror = scene.details.get(io::slot::MIRROR).unwrap();
        let finish = match look.finish {
            Finish::None => "no finish".to_owned(),
            Finish::LurePlastic => format!("lure plastic \"{}\"", look::LURES[look.lure.preset].id),
            Finish::Metal => format!("metal \"{}\"", look::METALS[look.metal.preset].id),
            Finish::Clay => format!("clay \"{}\"", look::CLAYS[look.clay.preset].id),
            Finish::Wood => format!("wood \"{}\"", look::WOODS[look.wood.preset].id),
        };
        println!(
            "   look: matcap {} (\"{}\"), {finish}, base colour {:?}, lighting \"{}\"{}, HDRI {}, background {:?}{}",
            if shading.matcap_on { "on" } else { "off" },
            shading.matcap,
            shading.base_color,
            look::LIGHTING[look.lighting].id,
            if look.five_point { " 5-point" } else { "" },
            look.hdri.path.as_ref().map_or("none".to_owned(), |p| p.display().to_string()),
            shading.background,
            if shading.gradient_background { " (gradient)" } else { "" },
        );
        println!("   mirror axes {:?}, guides {}", [mirror.x, mirror.y, mirror.z], mirror.guides);
        let kept: SavedDynamicTopology = scene.details.get(slot::DYNAMIC_TOPOLOGY).unwrap();
        println!("   dynamic topology {} at detail {}", if kept.enabled { "on" } else { "off" }, kept.detail);
        for (index, extra) in opened.extras.objects.iter().enumerate() {
            println!("   kept for later, object {index} (row {}): {:?}", extra.row, extra.symmetry);
            if let Some(array) = &extra.array {
                println!("        array {:?}, {} points", array.settings, array.points.len());
            }
        }
        println!("   left behind: {:?}", opened.left_behind);
        let mentions_arrays = |notes: &[String]| notes.iter().any(|note| note.to_lowercase().contains("array"));
        assert!(
            !mentions_arrays(&opened.left_behind.objects) && !mentions_arrays(&opened.left_behind.settings),
            "{name}: an array is not left behind any more: {:?}",
            opened.left_behind
        );

        drop(plain);
        survives_a_save(&path, &opened.scene);
        // And the sculpt itself was only ever read.
        assert_eq!(
            (std::fs::metadata(&path).unwrap().len(), std::fs::metadata(&path).unwrap().modified().unwrap()),
            untouched,
            "{name} was touched"
        );
    }
}

/// Saves an opened sample as a `.sclp` and opens that: the same scene. The
/// `.sclp` goes in a folder beside the samples' own, never beside a sculpt,
/// and is removed again.
fn survives_a_save(path: &Path, opened: &SavedScene) {
    let folder = path.parent().unwrap().with_file_name("roundtrip");
    std::fs::create_dir_all(&folder).unwrap();
    let saved_to = folder.join(save_name(path));
    let meshes: Vec<&Mesh> = opened.meshes.iter().collect();
    let began = Instant::now();
    io::save_scene_with(&meshes, &opened.objects, opened.active, &opened.details, &saved_to).unwrap();
    let wrote = began.elapsed().as_secs_f64();
    let began = Instant::now();
    let again = io::load_scene(&saved_to).unwrap();
    println!(
        "   saved as {} ({:.1} MB) in {wrote:.2} s, reopened in {:.2} s: the same scene",
        saved_to.display(),
        std::fs::metadata(&saved_to).unwrap().len() as f64 / 1e6,
        began.elapsed().as_secs_f64()
    );
    std::fs::remove_file(&saved_to).unwrap();
    assert_eq!(
        (again.active, again.objects.len(), again.meshes.len()),
        (opened.active, opened.objects.len(), opened.meshes.len())
    );
    for (a, b) in again.objects.iter().zip(&opened.objects) {
        assert_eq!(
            (&a.name, a.visible, a.mesh, a.position, a.rotation, a.scale),
            (&b.name, b.visible, b.mesh, b.position, b.rotation, b.scale)
        );
    }
    for (a, b) in again.meshes.iter().zip(&opened.meshes) {
        assert!(super::shape(a) == super::shape(b), "a mesh changed on the way through {}", saved_to.display());
    }
    let rows = opened.objects.len();
    assert_eq!(
        super::settings(&again.details, rows),
        super::settings(&opened.details, rows),
        "the settings in {}",
        saved_to.display()
    );
    // The arrays: on the same rows, and the same arrays.
    let arrays = |scene: &SavedScene| -> Vec<Option<ArrayState>> {
        (0..rows)
            .map(|row| scene.details.get_tool_object(row, io::slot::ARRAY).and_then(|a| sculpt_array::state_from_saved(&a)))
            .collect()
    };
    assert_eq!(arrays(&again), arrays(opened), "the arrays in {}", saved_to.display());
    println!("   {} live array(s) kept by the save", arrays(&again).iter().flatten().count());
}

/// The object's live array, if it has one, against the file: it is there on the object's own row
/// (and no other), it is the file's array in this app's axes, and its copies, laid out by the
/// Array tool's own layout, sit where the Unity app drew them. "Where Unity drew them" is worked
/// out here from the file's numbers and the Unity app's arithmetic alone (`ArrayLayout.Compute`
/// on the file's own settings and points, then `ArrayRepeater.SyncViews`' transform rule, then X
/// negated as Unity's exporters did), never from the importer's `placements`, which are checked
/// against the tool's layout too. The copy is compared by the box it fills and its centre.
fn live_array_sits_where_unity_drew_it(
    name: &str,
    entry: &Value,
    scene: &SavedScene,
    row: usize,
    source: &Plain,
    extra: &ObjectExtras,
) {
    let object = &scene.objects[row];
    let array = &entry["array"];
    let file_points: Vec<Vec3> =
        array["points"].as_array().map_or(Vec::new(), |points| points.iter().map(vector).collect());
    let has_one = matches!(array["kind"].as_i64().unwrap_or(0), 1..=3) && file_points.len() >= 2;
    let live = scene.details.get_tool_object(row, io::slot::ARRAY).and_then(|a| sculpt_array::state_from_saved(&a));
    assert_eq!(live.is_some(), has_one, "{name} / {}: a live array exactly where the file has one", object.name);
    let Some(live) = live else { return };
    let kind = array["kind"].as_i64().unwrap_or(0) as i32;
    assert_eq!(live.settings.kind.id(), kind);
    assert_eq!(live.settings.count as i64, array["count"].as_i64().unwrap().clamp(2, 100));
    assert_eq!(live.points.len(), file_points.len());
    for (here, there) in live.points.iter().zip(&file_points) {
        assert_eq!(*here, Vec3::new(0.0 - there.x, there.y, there.z), "{name}: a handle point is the file's, flipped");
    }

    // The file's settings, in Unity's axes, and where Unity drew each copy.
    let settings = ArraySettings {
        kind: ArrayKind::from_id(kind),
        count: array["count"].as_i64().unwrap() as i32,
        end_scale: array["endScale"].as_f64().unwrap_or(1.0) as f32,
        mid_scale: array["midScale"].as_f64().unwrap_or(1.0) as f32,
        closed: array["closed"].as_bool().unwrap_or(false),
        follow: array["follow"].as_bool().unwrap_or(true),
        space_by_size: array["spaceBySize"].as_bool().unwrap_or(true),
        axis: array["axis"].as_i64().unwrap_or(1) as i32,
        sweep: array["sweep"].as_f64().unwrap_or(360.0) as f32,
    };
    let unity_layout = sculpt_array::compute_layout(&settings, &file_points);
    let tool_layout = sculpt_array::compute_layout(&live.settings, &live.points);
    assert_eq!(unity_layout.len(), tool_layout.len());

    let (position, rotation, scale) = (vector(&entry["position"]), turn(&entry["rotation"]), vector(&entry["scale"]));
    let mut used = vec![false; source.vertices.len()];
    source.triangles.iter().for_each(|&i| used[i as usize] = true);
    let pose = sculpt_array::Pose::new(object.position, object.rotation, object.scale);
    let mesh = &scene.meshes[object.mesh];
    let (mut worst_box, mut worst_placement) = (0.0_f64, 0.0_f64);
    for (k, (unity_copy, tool_copy)) in unity_layout.iter().zip(&tool_layout).enumerate() {
        // Unity's: the object's place plus the copy's offset, the copy's turn before the object's,
        // the object's scale times the copy's.
        let drawn = Mat4::from_scale_rotation_translation(
            scale * unity_copy.scale,
            unity_copy.rotation * rotation,
            position + unity_copy.offset,
        )
        .as_dmat4();
        let (lo, hi, centre, _) = extent(
            source
                .vertices
                .par_iter()
                .zip(&used)
                .filter(|(_, used)| **used)
                .map(|(v, _)| drawn.transform_point3(v.as_dvec3()))
                .map(|w| DVec3::new(-w.x, w.y, w.z)),
        );
        // The tool's: the same mesh in this app, through the copy's matrix.
        let model = tool_copy.world_matrix(&pose).as_dmat4();
        let (lo_here, hi_here, centre_here, _) =
            extent(mesh.positions.par_iter().map(|p| model.transform_point3(p.as_dvec3())));
        let size = (hi - lo).max_element();
        let off = (lo - lo_here)
            .abs()
            .max_element()
            .max((hi - hi_here).abs().max_element())
            .max((centre - centre_here).abs().max_element());
        assert!(off < 1e-5 * size.max(1.0), "{name} / {}: copy {k} is {off} from where Unity drew it", object.name);
        worst_box = worst_box.max(off);
    }
    // And the importer's own account of where Unity drew them is the tool's layout.
    let kept = &extra.array.as_ref().expect("the importer kept where Unity drew the copies").placements;
    assert_eq!(kept.len(), tool_layout.len());
    for (k, (a, b)) in kept.iter().zip(&tool_layout).enumerate() {
        let (offset, turn, size) = (a.offset.distance(b.offset), a.rotation.angle_between(b.rotation), (a.scale - b.scale).abs());
        assert!(offset < 1e-4 && turn < 1e-4 && size < 1e-5, "{name}: copy {k} placement: {offset} {turn} {size}");
        worst_placement = worst_placement.max(offset as f64);
    }
    println!(
        "        live {:?} array, {} copies, {} points: every copy within {worst_box:.2e} units of where Unity drew it \
         (its box and centre), placements within {worst_placement:.2e}",
        live.settings.kind,
        tool_layout.len(),
        live.points.len()
    );
}

