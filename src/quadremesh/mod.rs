//! Quad remesh: rebuilds a triangle sculpt as an even, quad-dominant mesh at
//! roughly a requested quad count, sitting on the sculpt's surface.
//!
//! The method is Instant Meshes' field-aligned one (Jakob et al. 2015):
//!
//!   prepare (sanitise, split long edges)        -> prepare
//!   hierarchy, curvature guide                  -> hierarchy, curvature
//!   orientation field, position field           -> field
//!   quad extraction                             -> extract
//!   project onto the sculpt, clean up, relax    -> here, cleanup, project
//!
//! Pure computation on plain arrays, so it can run on a worker thread.

mod cleanup;
mod curvature;
mod extract;
mod field;
mod hierarchy;
mod poly;
mod prepare;
mod project;
#[cfg(test)]
mod tests;

use std::time::Instant;

use glam::Vec3;
use rayon::prelude::*;

pub use poly::PolyMesh;
use poly::{Edges, NONE, find, union};

use crate::mesh::{Mesh, Polys};

/// Relaxation passes after extraction.
const RELAX_ITERATIONS: usize = 3;

pub struct QuadRemesh {
    /// The quads (and the odd triangle), on the surface.
    pub mesh: PolyMesh,
    pub report: Report,
}

/// What came out, for the status line and for judging a result.
#[derive(Default, Debug, Clone)]
pub struct Report {
    pub vertices: usize,
    pub quads: usize,
    pub triangles: usize,
    pub other_faces: usize,
    /// Interior vertices with 3, with 5, and with any other number of edges
    /// that is not 4. Fewer is better: these are where edge loops end or turn.
    pub valence3: usize,
    pub valence5: usize,
    pub valence_other: usize,
    pub boundary_edges: usize,
    pub non_manifold_edges: usize,
    /// Spread of edge lengths: standard deviation over mean.
    pub edge_spread: f32,
    pub seconds: f32,
}

impl Report {
    pub fn irregular(&self) -> usize {
        self.valence3 + self.valence5 + self.valence_other
    }

    pub fn of(mesh: &PolyMesh, seconds: f32) -> Report {
        let mut report = Report { vertices: mesh.vertex_count(), seconds, ..Report::default() };
        for face in mesh.faces() {
            match face.len() {
                4 => report.quads += 1,
                3 => report.triangles += 1,
                _ => report.other_faces += 1,
            }
        }
        let edges = Edges::of_poly(mesh);
        let mut valence = vec![0u32; mesh.vertex_count()];
        let mut on_boundary = vec![false; mesh.vertex_count()];
        let (mut sum, mut sum_sq) = (0.0f64, 0.0f64);
        for e in 0..edges.count() {
            let (a, b) = (edges.a[e] as usize, edges.b[e] as usize);
            valence[a] += 1;
            valence[b] += 1;
            match edges.uses[e] {
                2 => {}
                1 => report.boundary_edges += 1,
                _ => report.non_manifold_edges += 1,
            }
            if edges.uses[e] != 2 {
                on_boundary[a] = true;
                on_boundary[b] = true;
            }
            let len = mesh.vertices[a].distance(mesh.vertices[b]) as f64;
            sum += len;
            sum_sq += len * len;
        }
        for v in 0..mesh.vertex_count() {
            if on_boundary[v] {
                continue;
            }
            match valence[v] {
                4 | 0 => {}
                3 => report.valence3 += 1,
                5 => report.valence5 += 1,
                _ => report.valence_other += 1,
            }
        }
        if edges.count() > 0 {
            let mean = sum / edges.count() as f64;
            report.edge_spread = ((sum_sq / edges.count() as f64 - mean * mean).max(0.0).sqrt() / mean) as f32;
        }
        report
    }

    pub fn summary(&self) -> String {
        let faces = self.quads + self.triangles + self.other_faces;
        format!(
            "{} quads + {} other faces ({:.1}% quads), {} poles, in {:.1} s",
            self.quads,
            faces - self.quads,
            100.0 * self.quads as f64 / faces.max(1) as f64,
            self.irregular(),
            self.seconds,
        )
    }
}

pub fn quad_remesh(vertices: &[Vec3], triangles: &[u32], target_quads: usize) -> Result<QuadRemesh, String> {
    let start = Instant::now();
    if triangles.len() < 3 || vertices.is_empty() {
        return Err("Nothing to remesh.".into());
    }
    let input = prepare::prepare(vertices, triangles, target_quads.max(1));
    if input.triangles.is_empty() || !(input.quad_edge > 0.0 && input.quad_edge.is_finite()) {
        return Err("The model has no surface to remesh.".into());
    }
    let scale = input.quad_edge;
    let h = hierarchy::Hierarchy::build(&input.vertices, &input.triangles);
    let curvature = curvature::Curvature::build(&h, scale, target_quads);
    let q = field::solve_orientation(&h, Some(&curvature));
    let o = field::solve_position(&h, &q, scale);
    let extracted = extract::extract(&h.levels[0], &q[0], &o[0], scale);
    let mesh = finish(extracted, &input, scale);
    if mesh.face_count() == 0 {
        return Err("Quad remesh produced no surface. Try more quads.".into());
    }
    let report = Report::of(&mesh, start.elapsed().as_secs_f32());
    Ok(QuadRemesh { mesh, report })
}

/// Fewest and most quads the app asks for.
pub const TARGET_RANGE: std::ops::RangeInclusive<f32> = 100.0..=200_000.0;

/// Quad remesh of a sculpt, as a new sculptable mesh that keeps its quads.
pub fn quad_remesh_mesh(mesh: &Mesh, target_quads: usize) -> Result<(Mesh, Report), String> {
    let QuadRemesh { mesh: poly, report } = quad_remesh(&mesh.positions, &mesh.indices, target_quads)?;
    let triangles = poly.triangulate();
    let PolyMesh { vertices, face_start, mut face_indices } = poly;
    let (mut out, remap) = Mesh::new_mapped(vertices, triangles, None);
    for i in &mut face_indices {
        *i = remap[*i as usize];
    }
    if !face_indices.contains(&NONE) {
        out.polys = Some(Polys { start: face_start, indices: face_indices });
    }
    Ok((out, report))
}

/// Connected-piece id of every triangle (union over shared vertices).
fn triangle_pieces(vertex_count: usize, triangles: &[u32]) -> Vec<u32> {
    let mut parent: Vec<u32> = (0..vertex_count as u32).collect();
    for t in triangles.chunks_exact(3) {
        union(&mut parent, t[0], t[1]);
        union(&mut parent, t[1], t[2]);
    }
    triangles.chunks_exact(3).map(|t| find(&mut parent, t[0])).collect()
}

/// Projects the extracted vertices onto the sculpt, cleans the topology, then
/// relaxes: each interior vertex moves halfway to the mean of its edge
/// neighbours and is projected back, which evens out the lattice's small
/// jitter without leaving the surface. Boundary vertices are only projected,
/// so open edges keep their line.
fn finish(mut mesh: PolyMesh, input: &prepare::Prepared, scale: f32) -> PolyMesh {
    let projector = project::Projector::new(&input.vertices, &input.triangles, scale);
    mesh.vertices.par_iter_mut().for_each(|p| *p = projector.project(*p));
    let mesh = cleanup::clean(mesh);
    let piece = triangle_pieces(input.vertices.len(), &input.triangles);
    let mut mesh = cleanup::remove_fragments(mesh, |p| {
        projector.project_with_triangle(p).1.map_or(NONE, |t| piece[t as usize])
    });

    let n = mesh.vertex_count();
    let edges = Edges::of_poly(&mesh);
    let mut start = vec![0u32; n + 1];
    let mut boundary = vec![false; n];
    for e in 0..edges.count() {
        let (a, b) = (edges.a[e] as usize, edges.b[e] as usize);
        start[a + 1] += 1;
        start[b + 1] += 1;
        if edges.uses[e] != 2 {
            boundary[a] = true;
            boundary[b] = true;
        }
    }
    for i in 0..n {
        start[i + 1] += start[i];
    }
    let mut fill = start.clone();
    let mut nb = vec![0u32; start[n] as usize];
    for e in 0..edges.count() {
        let (a, b) = (edges.a[e], edges.b[e]);
        nb[fill[a as usize] as usize] = b;
        fill[a as usize] += 1;
        nb[fill[b as usize] as usize] = a;
        fill[b as usize] += 1;
    }

    for _ in 0..RELAX_ITERATIONS {
        let cur = &mesh.vertices;
        let next: Vec<Vec3> = (0..n)
            .into_par_iter()
            .map(|i| {
                let ring = &nb[start[i] as usize..start[i + 1] as usize];
                if boundary[i] || ring.is_empty() {
                    return cur[i];
                }
                let mean = ring.iter().map(|&u| cur[u as usize]).sum::<Vec3>() / ring.len() as f32;
                projector.project((cur[i] + mean) * 0.5)
            })
            .collect();
        mesh.vertices = next;
    }
    mesh
}
