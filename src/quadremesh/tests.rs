use std::path::{Path, PathBuf};

use glam::{Mat3, Vec3};
use sculpt_geom::EdgeCensus;

use super::poly::{Edges, PolyMesh};
use super::{Report, quad_remesh};
use crate::mesh::Mesh;
use crate::mesh::primitives::{self, Primitive};
use crate::remesh::voxel_remesh;

/// Set QUAD_OUT to a folder to get a picture of every test result there.
fn out_dir() -> Option<PathBuf> {
    std::env::var_os("QUAD_OUT").map(PathBuf::from)
}

/// Ellipsoids fused by a voxel remesh into one skin.
fn blobs(parts: &[(Vec3, Vec3)], resolution: u32) -> Mesh {
    let mut positions = Vec::new();
    let mut indices = Vec::new();
    for &(centre, radii) in parts {
        let part = primitives::build(Primitive::Sphere, 20_000, 1.0);
        let base = positions.len() as u32;
        positions.extend(part.positions.iter().map(|&p| p * radii + centre));
        indices.extend(part.indices.iter().map(|&i| i + base));
    }
    voxel_remesh(&Mesh::new(positions, indices, None), resolution, [false; 3]).unwrap()
}

/// A rough figure: torso, head, arms and legs, with the creases a real
/// blockout has where parts meet.
fn figure() -> Mesh {
    let v = Vec3::new;
    blobs(
        &[
            (v(0.0, 0.1, 0.0), v(0.26, 0.40, 0.17)),
            (v(0.0, 0.68, 0.02), v(0.17, 0.20, 0.18)),
            (v(0.0, 0.66, 0.19), v(0.04, 0.05, 0.06)),
            (v(0.45, 0.30, 0.0), v(0.30, 0.075, 0.075)),
            (v(-0.45, 0.30, 0.0), v(0.30, 0.075, 0.075)),
            (v(0.13, -0.62, 0.0), v(0.10, 0.42, 0.10)),
            (v(-0.13, -0.62, 0.0), v(0.10, 0.42, 0.10)),
            (v(0.13, -1.02, 0.08), v(0.08, 0.05, 0.16)),
            (v(-0.13, -1.02, 0.08), v(0.08, 0.05, 0.16)),
        ],
        220,
    )
}

fn run(name: &str, mesh: &Mesh, target: usize) -> Report {
    let result = quad_remesh(&mesh.positions, &mesh.indices, target).unwrap();
    let r = &result.report;
    println!(
        "{name}: target {target}, {} from {} tris | v3 {} v5 {} other {} | boundary {} non-manifold {} | \
         left out {} faces, {} pieces | spread {:.3}",
        r.summary(),
        mesh.triangle_count(),
        r.valence3,
        r.valence5,
        r.valence_other,
        r.boundary_edges,
        r.non_manifold_edges,
        r.left_out_faces,
        r.dropped_pieces,
        r.edge_spread,
    );
    if let Some(dir) = out_dir() {
        std::fs::create_dir_all(&dir).unwrap();
        render(&result.mesh, &dir.join(format!("{name}_{target}.png")));
    }
    result.report
}

#[test]
fn sphere_becomes_quads() {
    let mesh = primitives::build(Primitive::Sphere, 200_000, 0.5);
    let r = run("sphere", &mesh, 2000);
    assert!(r.quads as f32 > 0.9 * (r.quads + r.triangles + r.other_faces) as f32);
    assert!((1400..2800).contains(&r.quads), "{} quads", r.quads);
    assert_eq!(r.boundary_edges + r.non_manifold_edges, 0);
}

#[test]
fn figure_becomes_quads() {
    let mesh = figure();
    let r = run("figure", &mesh, 6000);
    assert!(r.quads as f32 > 0.9 * (r.quads + r.triangles + r.other_faces) as f32);
    assert_eq!(r.non_manifold_edges, 0);
}

#[test]
fn remeshed_mesh_keeps_its_quads() {
    let source = primitives::build(Primitive::Capsule, 50_000, 0.5);
    let (mesh, report) = super::quad_remesh_mesh(&source, 800).unwrap();
    let polys = mesh.polys.as_ref().expect("quads kept");
    assert_eq!(polys.start.len() - 1, report.quads + report.triangles + report.other_faces);
    assert_eq!(mesh.triangle_count(), polys.start.windows(2).map(|f| (f[1] - f[0]) as usize - 2).sum::<usize>());
    // Every polygon lies on the triangles it was cut into: its corners are
    // real vertices, and its first three wind the way the surface faces.
    for f in polys.start.windows(2) {
        let face = &polys.indices[f[0] as usize..f[1] as usize];
        let [a, b, c] = [0, 1, 2].map(|k| mesh.positions[face[k] as usize]);
        let normal = (b - a).cross(c - a);
        assert!(normal.dot(mesh.normals[face[0] as usize]) > 0.0);
    }
    assert!(mesh.bounds().size().distance(source.bounds().size()) < 0.03);
}

/// Two views of a polygon mesh with its real edges drawn (not the triangle
/// diagonals), triangles tinted yellow, larger faces pink, and a dot on every
/// pole: blue where 3 edges meet, red where 5, purple otherwise.
pub fn render(mesh: &PolyMesh, path: &Path) {
    const W: usize = 1100;
    const H: usize = 1100;
    let views = [(0.45f32, 0.25f32), (2.75, 0.2)];
    let mut img = image::RgbImage::from_pixel((W * views.len()) as u32, H as u32, image::Rgb([24, 26, 30]));

    let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for &p in &mesh.vertices {
        min = min.min(p);
        max = max.max(p);
    }
    let centre = (min + max) * 0.5;
    let radius = mesh.vertices.iter().map(|p| p.distance(centre)).fold(1e-6f32, f32::max);
    let scale = 0.48 * W.min(H) as f32 / radius;
    let bias = radius * 0.012;

    let edges = Edges::of_poly(mesh);
    let mut valence = vec![0u32; mesh.vertex_count()];
    let mut boundary = vec![false; mesh.vertex_count()];
    for e in 0..edges.count() {
        valence[edges.a[e] as usize] += 1;
        valence[edges.b[e] as usize] += 1;
        if edges.uses[e] != 2 {
            boundary[edges.a[e] as usize] = true;
            boundary[edges.b[e] as usize] = true;
        }
    }

    for (view, &(yaw, pitch)) in views.iter().enumerate() {
        let rot = Mat3::from_rotation_x(pitch) * Mat3::from_rotation_y(yaw);
        let to_screen = |p: Vec3| {
            let v = rot * (p - centre);
            Vec3::new(W as f32 * 0.5 + v.x * scale, H as f32 * 0.5 - v.y * scale, v.z)
        };
        let s: Vec<Vec3> = mesh.vertices.iter().map(|&p| to_screen(p)).collect();
        let mut depth = vec![f32::MIN; W * H];
        let mut put = |x: usize, y: usize, c: [u8; 3]| img.put_pixel((x + view * W) as u32, y as u32, image::Rgb(c));
        let light = Vec3::new(0.35, 0.5, 0.8).normalize();

        for face in mesh.faces() {
            let base = match face.len() {
                4 => Vec3::new(0.62, 0.64, 0.68),
                3 => Vec3::new(0.95, 0.80, 0.25),
                _ => Vec3::new(0.95, 0.45, 0.70),
            };
            for k in 1..face.len() - 1 {
                let (a, b, c) = (s[face[0] as usize], s[face[k] as usize], s[face[k + 1] as usize]);
                let [pa, pb, pc] = [face[0], face[k], face[k + 1]].map(|i| mesh.vertices[i as usize]);
                let normal = (rot * (pb - pa).cross(pc - pa)).normalize_or_zero();
                let shade = 0.30 + 0.70 * normal.dot(light).max(0.0);
                let colour = (base * shade * 255.0).to_array().map(|c| c as u8);
                let area = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
                if area.abs() < 1e-9 {
                    continue;
                }
                let (x0, x1) = (a.x.min(b.x).min(c.x).floor().max(0.0) as usize, a.x.max(b.x).max(c.x).ceil().min(W as f32 - 1.0) as usize);
                let (y0, y1) = (a.y.min(b.y).min(c.y).floor().max(0.0) as usize, a.y.max(b.y).max(c.y).ceil().min(H as f32 - 1.0) as usize);
                for y in y0..=y1 {
                    for x in x0..=x1 {
                        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                        let w0 = ((b.x - px) * (c.y - py) - (b.y - py) * (c.x - px)) / area;
                        let w1 = ((c.x - px) * (a.y - py) - (c.y - py) * (a.x - px)) / area;
                        let w2 = 1.0 - w0 - w1;
                        if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                            continue;
                        }
                        let z = w0 * a.z + w1 * b.z + w2 * c.z;
                        if z > depth[y * W + x] {
                            depth[y * W + x] = z;
                            put(x, y, colour);
                        }
                    }
                }
            }
        }

        let visible = |x: f32, y: f32, z: f32, depth: &[f32]| {
            x >= 0.0 && y >= 0.0 && (x as usize) < W && (y as usize) < H && z + bias >= depth[y as usize * W + x as usize]
        };
        for e in 0..edges.count() {
            let (a, b) = (s[edges.a[e] as usize], s[edges.b[e] as usize]);
            let steps = (b.x - a.x).abs().max((b.y - a.y).abs()).ceil().max(1.0) as usize;
            let colour = if edges.uses[e] == 2 { [20, 22, 26] } else { [255, 140, 0] };
            for i in 0..=steps {
                let p = a.lerp(b, i as f32 / steps as f32);
                if visible(p.x, p.y, p.z, &depth) {
                    put(p.x as usize, p.y as usize, colour);
                }
            }
        }
        for v in 0..mesh.vertex_count() {
            if boundary[v] || valence[v] == 4 || valence[v] == 0 {
                continue;
            }
            let colour = match valence[v] {
                3 => [40, 130, 255],
                5 => [255, 50, 40],
                _ => [200, 60, 255],
            };
            let p = s[v];
            for dy in -4i32..=4 {
                for dx in -4i32..=4 {
                    let (x, y) = (p.x + dx as f32, p.y + dy as f32);
                    if dx * dx + dy * dy <= 16 && visible(x, y, p.z, &depth) {
                        put(x as usize, y as usize, colour);
                    }
                }
            }
        }
    }
    img.save(path).unwrap();
}

/// The closed models the closedness tests remesh: each starting shape, and
/// the figure, which has the creases and thin limbs of a real blockout.
fn closed_models() -> Vec<(&'static str, Mesh)> {
    vec![
        ("sphere", primitives::build(Primitive::Sphere, 200_000, 0.5)),
        ("cube", primitives::build(Primitive::Cube, 200_000, 0.5)),
        ("cylinder", primitives::build(Primitive::Cylinder, 200_000, 0.5)),
        ("cone", primitives::build(Primitive::Cone, 200_000, 0.5)),
        ("capsule", primitives::build(Primitive::Capsule, 50_000, 0.5)),
        ("figure", figure()),
    ]
}

/// Quad-remeshes `mesh` and returns what the report says of its edges beside
/// what the mesh that came out really has, counted afresh from its triangles.
fn reported_and_real(name: &str, mesh: &Mesh, target: usize) -> (Report, EdgeCensus) {
    let (out, report) = super::quad_remesh_mesh(mesh, target).unwrap();
    let real = crate::printable::census(&out);
    println!(
        "{name} at {target}: {} | left out {} faces, {} pieces | really {real:?}",
        report.summary(),
        report.left_out_faces,
        report.dropped_pieces
    );
    (report, real)
}

/// A quad remesh says what it made: the report's counts are the real ones,
/// on the triangles that are sculpted and exported, and whatever is not
/// closed is in the line the status area shows.
///
/// The capsule at 2000 and the figure at 100 are the cases the report used to
/// get wrong: every face edge had two faces, so it counted nothing, while
/// the diagonals of two quads lay on one edge and gave it four triangles.
/// (They come out closed now: see the test below.) The bowl is here for a
/// result that has something to report.
#[test]
fn the_report_counts_the_open_and_crowded_edges_the_mesh_really_has() {
    let models = closed_models();
    let model = |name: &str| &models.iter().find(|(n, _)| *n == name).unwrap().1;
    let bowl = open_models().remove(1).1;
    let cases = [
        ("sphere", 800, model("sphere")),
        ("cube", 2000, model("cube")),
        ("capsule", 2000, model("capsule")),
        ("cylinder", 6000, model("cylinder")),
        ("figure", 800, model("figure")),
        ("figure", 100, model("figure")),
        ("bowl", 800, &bowl),
    ];
    for (name, target, mesh) in cases {
        let (report, real) = reported_and_real(name, mesh, target);
        assert_eq!(report.census(), real, "{name} at {target}");
        assert_eq!((report.boundary_edges, report.non_manifold_edges), (real.open, real.crowded), "{name} at {target}");
        match crate::printable::faults(&real) {
            Some(faults) => assert!(report.summary().contains(&faults), "{name} at {target}: {}", report.summary()),
            None => assert!(!report.summary().contains("edge"), "{name} at {target}: {}", report.summary()),
        }
        assert_eq!(real.is_closed(), name != "bowl", "{name} at {target}");
    }
}

/// A closed model comes out closed: no hole in it, and no edge with more
/// than two triangles.
///
/// It did not always. Measured on 2026-10-10 over these six models at seven
/// targets each, 10 of the 42 runs were not closed, of two kinds:
///
///  - Five open edges round one hole (7 runs: cylinder at 6000, capsule at
///    6000, figure at 800, and others). A five-sided face with a needle-thin
///    corner had that corner read as 360 degrees for 0, which put its angles
///    halfway to those of the trace round an opening, and it was dropped for
///    one. Only a loop by an open edge of the input is asked that now
///    (`extract::is_opening`).
///  - Two to six edges shared by four triangles (4 runs: capsule at 2000,
///    figure at 100 and 300, cone at 60000). Two faces that shared both
///    sides of a corner were each cut along the line between that corner's
///    neighbours (`cleanup::dissolve_doublets` takes the corner out), or two
///    quads standing corner to corner were cut along the same diagonal
///    (`PolyMesh::triangulate` cuts one of them the other way).
///
/// Run with `--nocapture` for a line a run.
#[test]
fn a_closed_model_comes_out_closed() {
    let mut open = Vec::new();
    for (name, mesh) in &closed_models() {
        assert!(crate::printable::census(mesh).is_closed(), "{name} was meant to be closed to begin with");
        for target in [100, 300, 800, 2000, 6000, 20_000, 60_000] {
            let (report, real) = reported_and_real(name, mesh, target);
            assert_eq!(report.census(), real, "{name} at {target}");
            assert_eq!(report.left_out_faces, 0, "{name} at {target}: a closed model has no rim to leave out");
            if !real.is_closed() {
                open.push(format!("{name} at {target}: {}", crate::printable::faults(&real).unwrap()));
            }
        }
    }
    assert!(open.is_empty(), "{} of 42 runs did not come out closed: {}", open.len(), open.join("; "));
}

/// What is left of `mesh` with the triangles whose centre `cut` picks taken away.
fn without(mesh: &Mesh, cut: impl Fn(Vec3) -> bool) -> Mesh {
    let centre = |t: &[u32]| t.iter().map(|&i| mesh.positions[i as usize]).sum::<Vec3>() / 3.0;
    let indices: Vec<u32> = mesh.indices.chunks_exact(3).filter(|t| !cut(centre(t))).flatten().copied().collect();
    Mesh::new(mesh.positions.clone(), indices, None)
}

/// Models with openings, and how many each has. Between them the rims are
/// of every kind the corner angles tell apart, and of the kinds they do not:
/// a hole in a sheet (the sphere with its top off) reads as a face, the rim
/// of a patch (the cap) as the outside of one, and the rim of a bowl or of a
/// tube's end as halfway between.
fn open_models() -> Vec<(&'static str, Mesh, usize)> {
    let sphere = primitives::build(Primitive::Sphere, 200_000, 0.5);
    let cylinder = primitives::build(Primitive::Cylinder, 200_000, 0.5);
    let (r, top) = (sphere.bounds().max.y, cylinder.bounds().max.y);
    vec![
        ("sphere with its top off", without(&sphere, |c| c.y > 0.6 * r), 1),
        ("bowl", without(&sphere, |c| c.y > 0.0), 1),
        ("cap", without(&sphere, |c| c.y < 0.6 * r), 1),
        ("sphere with two holes", without(&sphere, |c| c.y.abs() > 0.8 * r), 2),
        ("tube", without(&cylinder, |c| c.y.abs() > top - 1e-4), 2),
    ]
}

/// A model with openings keeps them, each one opening still: a quad remesh
/// is not the tool that closes a model (Remesh is), and a sleeve or a mask
/// that came back with its openings capped would be a different model.
///
/// Before 2026-10-12 a rim was told from a face by its corner angles alone,
/// which a hole in a sheet passes as a face: the sphere with its top off
/// came back a closed ball at 100 and 300 quads, and the tube with one end
/// or both capped at every target up to 2000.
#[test]
fn a_model_with_openings_keeps_them() {
    for (name, mesh, openings) in &open_models() {
        assert_eq!(crate::printable::census(mesh).holes, Some(*openings), "{name} to begin with");
        for target in [100, 300, 800, 2000, 6000, 20_000] {
            let (out, report) = super::quad_remesh_mesh(mesh, target).unwrap();
            let real = crate::printable::census(&out);
            println!("{name} at {target}: {} | left out {} faces", report.summary(), report.left_out_faces);
            assert_eq!(report.census(), real, "{name} at {target}");
            assert_eq!((real.holes, real.crowded), (Some(*openings), 0), "{name} at {target}: {}", report.summary());
        }
    }
}
