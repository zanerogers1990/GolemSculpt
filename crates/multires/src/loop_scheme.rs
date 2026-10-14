//! Loop subdivision: the scheme for a mesh that is all triangles.
//!
//! Every triangle becomes four: one at each corner and one in the middle,
//! joined at a new vertex on each edge. The topology comes from `sculpt-geom`
//! (`MeshAdjacency`, `MeshEdges`), whose edges are numbered in ascending order
//! of their (lower vertex, higher vertex) pair. That order is the numbering of
//! the new vertices: the vertex on edge `e` is `vertices + e`. It is part of
//! what a saved file means, so it must not change.
//!
//! The rules (Loop 1987, with Warren's and Hoppe's boundary rules):
//!
//! * A new vertex on an edge with a triangle each side: 3/8 of each end and
//!   1/8 of each of the two corners opposite.
//! * A kept vertex with `n` neighbours: `1 - n b` of itself and `b` of each
//!   neighbour, with Loop's `b(n)`.
//! * On a crease (see below) a new vertex is the middle of its edge and a
//!   kept one takes 3/4 of itself and 1/8 of each crease neighbour, so the rim
//!   of an open mesh is a smooth curve of its own that the surface inside
//!   never pulls on. A rim that was pulled on would shrink away from where it
//!   was sculpted with every level.
//!
//! An edge is a crease when it does not have exactly two triangles: the rim
//! of a hole (one), or a line where three or more sheets meet, which no smooth
//! surface passes through. How a kept vertex with creases at it moves is in
//! [`Creases::rule`]. A vertex where two fans of triangles touch at a point
//! and nowhere else is smoothed against all its neighbours as if they were one
//! fan, which is finite and harmless and not worth finding. A triangle with
//! two equal corners has no edges in `sculpt-geom`'s tables; it is passed on as
//! four copies of itself, which keeps "four children each" true and still
//! draws nothing.

use std::f64::consts::TAU;

use glam::{DVec3, Vec3};
use rayon::prelude::*;
use sculpt_geom::adjacency::{MeshAdjacency, MeshEdges, NO_EDGE};

use crate::mesh::Faces;
use crate::refine::{Creases, FIXED, PER_TASK, Refiner, SMOOTH, crease_point};

/// Valences whose weight is worked out once per pass rather than per vertex.
const BETAS: usize = 16;

/// What a triangle level keeps so that the level above can be made from it,
/// and remade after it is edited.
#[derive(Clone)]
pub(crate) struct LoopTables {
    adjacency: MeshAdjacency,
    edges: MeshEdges,
    /// How each vertex moves: `[SMOOTH; 2]`, `[FIXED; 2]`, or its two crease neighbours.
    rule: Vec<[u32; 2]>,
}

impl LoopTables {
    /// Every index in `triangles` must be below `vertex_count`.
    pub(crate) fn build(vertex_count: usize, triangles: &[u32]) -> LoopTables {
        let adjacency = MeshAdjacency::build(vertex_count, triangles);
        let edges = MeshEdges::build(&adjacency, triangles);
        let rule = (0..vertex_count)
            .into_par_iter()
            .with_min_len(PER_TASK)
            .map(|v| {
                let ring = adjacency.neighbours_of(v);
                let mut creases = Creases::NONE;
                for &u in ring {
                    let e = edges.edge_id(v as u32, u).expect("neighbours share an edge");
                    if edges.triangles_of(e).len() != 2 {
                        creases.note(u);
                    }
                }
                creases.rule(ring.len())
            })
            .collect();
        LoopTables { adjacency, edges, rule }
    }

    pub(crate) fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Near enough what the tables hold: every list that grows with the mesh.
    pub(crate) fn bytes(&self) -> usize {
        let rows = |c: &sculpt_geom::adjacency::Csr| (c.offsets().len() + c.items().len()) * 4;
        rows(self.adjacency.vertex_triangles())
            + rows(self.adjacency.vertex_neighbours())
            + rows(self.edges.edge_triangles())
            + self.edges.len() * 8
            + (self.adjacency.vertex_count() + 1) * 4
            + self.adjacency.triangle_count() * 12
            + self.rule.len() * 8
    }
}

/// Loop's weight for each neighbour of an interior vertex with `n` of them.
fn loop_beta(n: usize) -> f64 {
    // Six is every vertex of a regular mesh, and there the formula is exactly
    // 1/16 but for the last bit of the cosine.
    if n == 6 {
        return 0.0625;
    }
    let c = 0.375 + 0.25 * (TAU / n as f64).cos();
    (0.625 - c * c) / n as f64
}

/// A triangle level seen as the maker of the level above.
pub(crate) struct LoopRefiner<'a> {
    triangles: &'a [u32],
    tables: &'a LoopTables,
    kept: usize,
    betas: [f64; BETAS],
}

impl<'a> LoopRefiner<'a> {
    pub(crate) fn new(triangles: &'a [u32], tables: &'a LoopTables) -> LoopRefiner<'a> {
        let mut betas = [0.0; BETAS];
        for (n, beta) in betas.iter_mut().enumerate().skip(1) {
            *beta = loop_beta(n);
        }
        LoopRefiner { triangles, tables, kept: tables.adjacency.vertex_count(), betas }
    }

    #[inline]
    fn corners(&self, t: u32) -> &[u32] {
        &self.triangles[t as usize * 3..t as usize * 3 + 3]
    }

    /// The corner of triangle `t` that is neither `a` nor `b`.
    #[inline]
    fn opposite(&self, p: &[Vec3], t: u32, a: u32, b: u32) -> DVec3 {
        let c = self.corners(t);
        let far = if c[0] != a && c[0] != b {
            c[0]
        } else if c[1] != a && c[1] != b {
            c[1]
        } else {
            c[2]
        };
        p[far as usize].as_dvec3()
    }

    fn kept_point(&self, p: &[Vec3], v: usize) -> Vec3 {
        match self.tables.rule[v] {
            [FIXED, _] => p[v],
            [SMOOTH, _] => {
                let ring = self.tables.adjacency.neighbours_of(v);
                let n = ring.len();
                let beta = if n < BETAS { self.betas[n] } else { loop_beta(n) };
                let sum: DVec3 = ring.iter().map(|&u| p[u as usize].as_dvec3()).sum();
                (p[v].as_dvec3() * (1.0 - n as f64 * beta) + sum * beta).as_vec3()
            }
            [a, b] => crease_point(p, v, a, b),
        }
    }

    fn edge_point(&self, p: &[Vec3], e: u32) -> Vec3 {
        let [a, b] = self.tables.edges.edge(e);
        let ends = p[a as usize].as_dvec3() + p[b as usize].as_dvec3();
        match *self.tables.edges.triangles_of(e) {
            [s, t] => {
                let far = self.opposite(p, s, a, b) + self.opposite(p, t, a, b);
                (ends * 0.375 + far * 0.125).as_vec3()
            }
            _ => (ends * 0.5).as_vec3(),
        }
    }

    /// The new vertex on each of triangle `t`'s edges (corners 0-1, 1-2, 2-0),
    /// or `None` for a triangle with two equal corners.
    #[inline]
    fn edge_vertices(&self, t: usize) -> Option<[u32; 3]> {
        let e = self.tables.edges.edges_of_triangle(t);
        (e[0] != NO_EDGE).then(|| e.map(|e| self.kept as u32 + e))
    }
}

impl Refiner for LoopRefiner<'_> {
    fn kept(&self) -> usize {
        self.kept
    }

    fn total(&self) -> usize {
        self.kept + self.tables.edges.len()
    }

    fn between(&self, new: usize) -> &[u32] {
        &self.tables.edges.edges()[new - self.kept]
    }

    fn smooth(&self, coarse: &[Vec3], vertex: usize) -> Vec3 {
        if vertex < self.kept {
            self.kept_point(coarse, vertex)
        } else {
            self.edge_point(coarse, (vertex - self.kept) as u32)
        }
    }

    fn fan(&self, vertex: usize, mut wedge: impl FnMut(u32, u32)) {
        if vertex < self.kept {
            // Each coarse triangle here leaves one fine triangle at this corner.
            for &t in self.tables.adjacency.triangles_of(vertex) {
                let Some(m) = self.edge_vertices(t as usize) else { continue };
                let c = self.corners(t);
                let j = c.iter().position(|&v| v as usize == vertex).expect("a triangle lists its own corners");
                wedge(m[j], m[(j + 2) % 3]);
            }
            return;
        }
        // Each coarse triangle on the edge leaves three fine triangles at its
        // middle: two corner ones and the one in the centre.
        let e = (vertex - self.kept) as u32;
        for &t in self.tables.edges.triangles_of(e) {
            let Some(m) = self.edge_vertices(t as usize) else { continue };
            let c = self.corners(t);
            let j = m.iter().position(|&v| v as usize == vertex).expect("a triangle lists its own edges");
            let (next, last) = ((j + 1) % 3, (j + 2) % 3);
            wedge(m[last], c[j]);
            wedge(m[next], m[last]);
            wedge(c[next], m[next]);
        }
    }

    /// A kept vertex reads itself and its neighbours; a new one the ends of
    /// its edge and the corners opposite. So: the vertex, its neighbours, and
    /// every edge of every triangle round it (its own edges and the far ones).
    fn touched_by(&self, coarse: u32, mut touched: impl FnMut(u32)) {
        touched(coarse);
        self.tables.adjacency.neighbours_of(coarse as usize).iter().for_each(|&u| touched(u));
        for &t in self.tables.adjacency.triangles_of(coarse as usize) {
            if let Some(m) = self.edge_vertices(t as usize) {
                m.into_iter().for_each(&mut touched);
            }
        }
    }

    /// Triangle `t` becomes `4t..4t + 4`: the one at each of its corners, in
    /// corner order and each starting from that corner, then the middle one.
    fn fine_faces(&self) -> Faces {
        let mut fine = vec![0u32; self.triangles.len() * 4];
        fine.par_chunks_exact_mut(12).with_min_len(PER_TASK / 4).enumerate().for_each(|(t, out)| {
            let c = self.corners(t as u32);
            let Some(m) = self.edge_vertices(t) else {
                out.chunks_exact_mut(3).for_each(|child| child.copy_from_slice(c));
                return;
            };
            out.copy_from_slice(&[c[0], m[0], m[2], c[1], m[1], m[0], c[2], m[2], m[1], m[0], m[1], m[2]]);
        });
        Faces::Triangles(fine)
    }

    fn parent_triangles(&self) -> Vec<u32> {
        (0..self.triangles.len() as u32 / 3 * 4).into_par_iter().with_min_len(PER_TASK).map(|t| t / 4).collect()
    }
}
