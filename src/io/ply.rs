//! Export as PLY (the Stanford polygon file), binary little-endian: the
//! format a painted model is most often wanted in by programs that read
//! vertex colour (MeshLab, CloudCompare, colour 3D printing services), and the
//! one with room for numbers of our own beside the colour.
//!
//! Per vertex: x y z and the normal nx ny nz (float), and for a painted model
//! red green blue alpha (uchar): the colour seen there, the paint over the
//! Base Colour (`io/painted.rs`), alpha always 255 so that no viewer shows the
//! model see-through where nothing was painted. Then one float per material
//! the paint uses, its weight at the vertex from 0 to 1, named after the
//! material (`mat_` and the name with anything but letters and digits made
//! `_`; a `comment material` line in the header gives each one's full name
//! and palette slot). An unpainted model has neither. Faces are
//! `list uchar int vertex_indices`: a quad-remeshed model's quads, else its
//! triangles.
//!
//! As the OBJ, the file is in the app's own units (1 unit is 1 cm) and axes
//! (Y up), each place's position, rotation and scale baked into its points,
//! and an object's live copies are in the file with it, the right way out.

use std::io::Write;
use std::path::Path;

use glam::{Mat3, Mat4};

use super::painted::{ExportPaint, Painted};
use super::{Result, export_points, nothing_to_export, replace_file};
use crate::mesh::Mesh;

/// A material's weight's property name: `mat_` and the name, every
/// character that is not a letter or a digit made `_`. A name another
/// material already took gets `_2`, `_3`...
fn property_names(names: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for name in names {
        let cleaned: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
        let stem = format!("mat_{cleaned}");
        let free = |n: &str| !out.iter().any(|o| o == n);
        let name = if free(&stem) { stem } else { (2..).map(|k| format!("{stem}_{k}")).find(|n| free(n)).unwrap() };
        out.push(name);
    }
    out
}

/// The faces of a mesh as an export writes them: a quad-remeshed mesh's
/// quads, not their halves, and otherwise its triangles.
fn faces(mesh: &Mesh) -> Box<dyn Iterator<Item = &[u32]> + '_> {
    match &mesh.polys {
        // A face's size is one byte in the file.
        Some(polys) if polys.start.windows(2).all(|f| f[1] - f[0] <= 255) => {
            Box::new(polys.start.windows(2).map(|f| &polys.indices[f[0] as usize..f[1] as usize]))
        }
        _ => Box::new(mesh.indices.chunks_exact(3)),
    }
}

/// Binary PLY of one object drawn at each of `models` (itself and its live
/// copies), all in the one file. Returns how many damaged points were written
/// mended, as `export_obj_placed` does.
pub fn export_ply_placed(mesh: &Mesh, models: &[Mat4], paint: &ExportPaint, path: &Path) -> Result<usize> {
    nothing_to_export(mesh)?;
    let (points, mended) = export_points(mesh);
    let painted = Painted::of(mesh, paint);
    let materials: Vec<(usize, String, String)> = match &painted {
        Some(p) => {
            let names: Vec<String> = p.materials.iter().map(|&k| paint.name_of(p.table[k])).collect();
            let props = property_names(&names);
            p.materials.iter().zip(names).zip(props).map(|((&k, name), prop)| (k, name, prop)).collect()
        }
        None => Vec::new(),
    };
    let n = points.positions.len();
    let vertex_count = n * models.len();
    let face_count = faces(mesh).count() * models.len();
    let with_normals = points.normals.len() == n;
    replace_file(path, |w| {
        writeln!(w, "ply")?;
        writeln!(w, "format binary_little_endian 1.0")?;
        writeln!(w, "comment Exported from GolemSculpt (1 unit = 1 cm, Y up)")?;
        if let Some(p) = &painted {
            writeln!(w, "comment colour: the paint over the Base Colour, sRGB")?;
            for (k, name, prop) in &materials {
                // A comment is one line of text; a name never breaks it.
                let name: String = name.chars().filter(|c| !c.is_control()).collect();
                writeln!(w, "comment material {prop} \"{name}\" (palette slot {})", p.table[*k])?;
            }
        }
        writeln!(w, "element vertex {vertex_count}")?;
        for axis in ["x", "y", "z"] {
            writeln!(w, "property float {axis}")?;
        }
        if with_normals {
            for axis in ["nx", "ny", "nz"] {
                writeln!(w, "property float {axis}")?;
            }
        }
        if painted.is_some() {
            for channel in ["red", "green", "blue", "alpha"] {
                writeln!(w, "property uchar {channel}")?;
            }
        }
        for (_, _, prop) in &materials {
            writeln!(w, "property float {prop}")?;
        }
        writeln!(w, "element face {face_count}")?;
        writeln!(w, "property list uchar int vertex_indices")?;
        writeln!(w, "end_header")?;

        let mut record = Vec::with_capacity(28 + 4 * materials.len());
        for model in models {
            let linear = Mat3::from_mat4(*model);
            let determinant = linear.determinant();
            // Normals go through the inverse transpose, as the OBJ's do.
            let normal_matrix = if determinant.abs() > 1e-20 { linear.inverse().transpose() } else { linear };
            for v in 0..n {
                record.clear();
                let p = model.transform_point3(points.positions[v]);
                record.extend(p.to_array().iter().flat_map(|x| x.to_le_bytes()));
                if with_normals {
                    let q = (normal_matrix * points.normals[v]).normalize_or_zero();
                    record.extend(q.to_array().iter().flat_map(|x| x.to_le_bytes()));
                }
                if let Some(p) = &painted {
                    record.extend(p.seen[v]);
                    record.push(255);
                    for (k, ..) in &materials {
                        record.extend(p.weight(v, *k).to_le_bytes());
                    }
                }
                w.write_all(&record)?;
            }
        }
        for (place, model) in models.iter().enumerate() {
            // A negative scale mirrors the model and turns every face inside out.
            let mirrored = Mat3::from_mat4(*model).determinant() < 0.0;
            let base = (place * n) as i32;
            for face in faces(mesh) {
                record.clear();
                record.push(face.len() as u8);
                let corner = |k: usize| face[if mirrored { face.len() - 1 - k } else { k }] as i32 + base;
                record.extend((0..face.len()).flat_map(|k| corner(k).to_le_bytes()));
                w.write_all(&record)?;
            }
        }
        Ok(())
    })?;
    Ok(mended)
}

#[cfg(test)]
mod tests {
    use glam::Vec3;

    use super::*;
    use crate::mesh::paint::{Channels, Mat8, PaintLayer, PaintRow, PaintStack, Rgba8};
    use crate::mesh::paint::palette::{PaletteEntry, ScenePaint};
    use crate::mesh::primitives::{self, Primitive};

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("sculpt_io_ply_tests");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    /// A sphere painted red on its first half of vertices (full) and with
    /// slot 7 ("Metal - Rust") at weight 200 on every third vertex.
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

    fn scene_paint() -> ExportPaint {
        let entry = PaletteEntry { slot: 7, name: "Metal - Rust".into(), swatch: [0.6, 0.3, 0.1], ..PaletteEntry::default() };
        ExportPaint::of([0.5, 0.5, 0.5], &ScenePaint { palette: vec![entry], ..ScenePaint::default() })
    }

    /// The header's lines, and the bytes after it.
    fn split(bytes: &[u8]) -> (Vec<String>, &[u8]) {
        let end = bytes.windows(11).position(|w| w == b"end_header\n").unwrap() + 11;
        let header = String::from_utf8(bytes[..end].to_vec()).unwrap();
        (header.lines().map(str::to_owned).collect(), &bytes[end..])
    }

    #[test]
    fn a_painted_model_has_its_colours_and_material_weights_and_faces() {
        let mesh = painted_sphere();
        let paint = scene_paint();
        let aside = Mat4::from_translation(Vec3::new(2.0, 0.0, 0.0));
        let mirrored = Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0));
        let path = temp("painted.ply");
        assert_eq!(export_ply_placed(&mesh, &[aside, mirrored], &paint, &path).unwrap(), 0);
        let bytes = std::fs::read(&path).unwrap();
        let (header, body) = split(&bytes);
        let (n, t) = (mesh.vertex_count(), mesh.triangle_count());
        let want = [
            "ply".to_owned(),
            "format binary_little_endian 1.0".into(),
            "comment Exported from GolemSculpt (1 unit = 1 cm, Y up)".into(),
            "comment colour: the paint over the Base Colour, sRGB".into(),
            "comment material mat_Metal___Rust \"Metal - Rust\" (palette slot 7)".into(),
            format!("element vertex {}", 2 * n),
            "property float x".into(),
            "property float y".into(),
            "property float z".into(),
            "property float nx".into(),
            "property float ny".into(),
            "property float nz".into(),
            "property uchar red".into(),
            "property uchar green".into(),
            "property uchar blue".into(),
            "property uchar alpha".into(),
            "property float mat_Metal___Rust".into(),
            format!("element face {}", 2 * t),
            "property list uchar int vertex_indices".into(),
            "end_header".into(),
        ];
        assert_eq!(header, want);
        let stride = 12 + 12 + 4 + 4;
        assert_eq!(body.len(), 2 * n * stride + 2 * t * 13, "every vertex, then every face, nothing more");

        let f32_at = |at: usize| f32::from_le_bytes(body[at..at + 4].try_into().unwrap());
        let painted = Painted::of(&mesh, &paint).unwrap();
        for v in 0..n {
            let at = v * stride;
            let p = Vec3::new(f32_at(at), f32_at(at + 4), f32_at(at + 8));
            assert!((p - aside.transform_point3(mesh.positions[v])).length() < 1e-6);
            let colour = &body[at + 24..at + 28];
            let want = if v < n / 2 { [255, 0, 0] } else { [128, 128, 128] };
            assert_eq!(colour, [want[0], want[1], want[2], 255], "vertex {v}");
            assert_eq!(painted.seen[v], want);
            let weight = f32_at(at + 28);
            assert_eq!(weight, if v % 3 == 0 { 200.0 / 255.0 } else { 0.0 });
        }
        // The second place's points are the mirror image, its faces walked the other way round.
        let at = (n + 5) * stride;
        assert!((Vec3::new(f32_at(at), f32_at(at + 4), f32_at(at + 8)) - mirrored.transform_point3(mesh.positions[5])).length() < 1e-6);
        let faces_at = 2 * n * stride;
        let int_at = |at: usize| i32::from_le_bytes(body[at..at + 4].try_into().unwrap());
        let first: Vec<i32> = (0..3).map(|k| int_at(faces_at + 1 + 4 * k)).collect();
        assert_eq!((body[faces_at], first), (3, mesh.indices[..3].iter().map(|&i| i as i32).collect::<Vec<_>>()));
        let second = faces_at + t * 13;
        let turned: Vec<i32> = (0..3).map(|k| int_at(second + 1 + 4 * k)).collect();
        let want: Vec<i32> = mesh.indices[..3].iter().rev().map(|&i| i as i32 + n as i32).collect();
        assert_eq!(turned, want);
    }

    #[test]
    fn an_unpainted_model_is_shape_alone_and_quads_stay_quads() {
        let corners = [Vec3::ZERO, Vec3::X, Vec3::new(1.0, 1.0, 0.0), Vec3::Y];
        let (mut mesh, to_new) = Mesh::new_mapped(corners.to_vec(), vec![0, 1, 2, 0, 2, 3], None);
        let quad: Vec<u32> = [0usize, 1, 2, 3].iter().map(|&i| to_new[i]).collect();
        mesh.polys = Some(crate::mesh::Polys { start: vec![0, 4], indices: quad.clone() });
        let path = temp("plain.ply");
        export_ply_placed(&mesh, &[Mat4::IDENTITY], &ExportPaint::default(), &path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let (header, body) = split(&bytes);
        assert!(!header.iter().any(|l| l.contains("red") || l.contains("mat_") || l.contains("colour")));
        assert!(header.contains(&"element vertex 4".to_owned()) && header.contains(&"element face 1".to_owned()));
        assert_eq!(body.len(), 4 * 24 + 1 + 16);
        let face: Vec<u32> = (0..4).map(|k| u32::from_le_bytes(body[97 + 4 * k..101 + 4 * k].try_into().unwrap())).collect();
        assert_eq!((body[96], face), (4, quad));
    }

    #[test]
    fn material_names_become_property_names_of_their_own() {
        let names = ["Metal - Rust", "Metal + Rust", "Clay"].map(String::from);
        assert_eq!(property_names(&names), ["mat_Metal___Rust", "mat_Metal___Rust_2", "mat_Clay"]);
    }
}
