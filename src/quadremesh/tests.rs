use std::path::{Path, PathBuf};

use glam::{Mat3, Vec3};

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
        "{name}: target {target}, {} from {} tris | v3 {} v5 {} other {} | boundary {} non-manifold {} | spread {:.3}",
        r.summary(),
        mesh.triangle_count(),
        r.valence3,
        r.valence5,
        r.valence_other,
        r.boundary_edges,
        r.non_manifold_edges,
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
