//! Catmull-Clark subdivision: the scheme for a mesh with polygon faces.
//!
//! A face of `n` corners becomes `n` quads, one at each corner, meeting at a
//! new vertex in the middle of the face and at a new vertex on each edge. So
//! whatever the faces were (a quad remesh leaves quads, the odd triangle and
//! the odd pentagon), every level above the first is all quads.
//!
//! Numbering, which is part of what a saved file means and must not change:
//! the kept vertices, then one vertex per edge in ascending order of the
//! edge's (lower vertex, higher vertex) pair, then one per face in face order.
//! The quad made at a face's corner takes that corner's place in the face
//! list: quad `q` is at corner `q` of the coarser level's `indices`, and its
//! own corners are the kept vertex, the vertex on the edge leaving it, the
//! face's middle, and the vertex on the edge arriving at it.
//!
//! Each quad is drawn as two triangles cut from its kept vertex to the face's
//! middle. The cut is by rule rather than by the shorter diagonal because a
//! rule gives a mirrored model mirrored triangles, and because this rule
//! nests: cut that way, each finer triangle lies inside exactly one of the
//! coarser quad's two triangles, whichever diagonal that quad was cut along.
//! That is what makes "the triangle this one came from" exact, so a hidden or
//! grouped triangle hands its state to precisely the triangles that cover it.
//!
//! The rules (Catmull and Clark 1978, with the usual boundary rules):
//!
//! * A face's vertex is the mean of its corners.
//! * An edge's vertex is the mean of its two ends and the two face vertices
//!   either side.
//! * A kept vertex with `n` edges moves to `(F + N + (n - 2) P) / n`: `F` the
//!   mean of the face vertices round it, `N` the mean of its neighbours.
//! * On a crease the rules are the cubic curve's, exactly as for triangles
//!   (see [`crate::loop_scheme`] and [`Creases::rule`]): an edge is a crease
//!   when it does not have exactly two faces.

use glam::{DVec3, Vec3};
use rayon::prelude::*;
use sculpt_geom::edge_key::{edge_key, edge_vertices};

use crate::mesh::{Faces, PolyRef};
use crate::refine::{Creases, FIXED, NONE, PER_TASK, Refiner, Rows, SMOOTH, crease_point};

/// What a polygon level keeps so that the level above can be made from it,
/// and remade after it is edited.
#[derive(Clone)]
pub(crate) struct QuadTables {
    /// Every edge as (lower vertex, higher vertex), ascending.
    edges: Vec<[u32; 2]>,
    /// For the face side starting at each corner, its edge.
    side_edge: Vec<u32>,
    /// The first two face sides on each edge, by the corner each starts at;
    /// the second is `NONE` on a rim.
    edge_sides: Vec<[u32; 2]>,
    /// Edges without exactly two faces.
    edge_crease: Vec<bool>,
    /// The face corners at each vertex.
    corners_at: Rows,
    /// The neighbours of each vertex, ascending.
    ring: Rows,
    /// How each vertex moves: `[SMOOTH; 2]`, `[FIXED; 2]`, or its two crease neighbours.
    rule: Vec<[u32; 2]>,
}

impl QuadTables {
    /// `poly` must have passed [`Faces::given`]'s checks or have been made here.
    pub(crate) fn build(vertex_count: usize, poly: PolyRef) -> QuadTables {
        let corners = poly.corners;
        let mut sides: Vec<(u64, u32)> = (0..corners.len())
            .into_par_iter()
            .with_min_len(PER_TASK)
            .map(|c| (edge_key(corners[c], corners[poly.next(c)]), c as u32))
            .collect();
        // Sorted by edge and then by corner, so the edges come out in their
        // numbering order and an edge's sides in the same order on every run.
        sides.par_sort_unstable();

        let mut tables = QuadTables {
            edges: Vec::with_capacity(sides.len() / 2 + 1),
            side_edge: vec![NONE; corners.len()],
            edge_sides: Vec::with_capacity(sides.len() / 2 + 1),
            edge_crease: Vec::with_capacity(sides.len() / 2 + 1),
            corners_at: Rows::default(),
            ring: Rows::default(),
            rule: Vec::new(),
        };
        let mut from = 0;
        while from < sides.len() {
            let key = sides[from].0;
            let run = sides[from..].iter().take_while(|side| side.0 == key).count();
            tables.push_edge(key, &sides[from..from + run]);
            from += run;
        }
        drop(sides);

        tables.corners_at = Rows::build(vertex_count, |put| {
            for (corner, &v) in corners.iter().enumerate() {
                put(v, corner as u32);
            }
        });
        // In edge order, a vertex meets its lower neighbours (ascending) and
        // then its higher ones (ascending): each row comes out sorted.
        let edges = &tables.edges;
        tables.ring = Rows::build(vertex_count, |put| {
            for &[a, b] in edges {
                put(a, b);
                put(b, a);
            }
        });
        tables.rule = tables.rules(vertex_count);
        tables
    }

    fn push_edge(&mut self, key: u64, sides: &[(u64, u32)]) {
        let (a, b) = edge_vertices(key);
        let e = self.edges.len() as u32;
        self.edges.push([a, b]);
        self.edge_sides.push([sides[0].1, sides.get(1).map_or(NONE, |side| side.1)]);
        self.edge_crease.push(sides.len() != 2);
        for side in sides {
            self.side_edge[side.1 as usize] = e;
        }
    }

    fn rules(&self, vertex_count: usize) -> Vec<[u32; 2]> {
        let mut creases = vec![Creases::NONE; vertex_count];
        for (&[a, b], _) in self.edges.iter().zip(&self.edge_crease).filter(|(_, crease)| **crease) {
            creases[a as usize].note(b);
            creases[b as usize].note(a);
        }
        creases.iter().enumerate().map(|(v, creases)| creases.rule(self.ring.row(v).len())).collect()
    }

    pub(crate) fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Near enough what the tables hold: every list that grows with the mesh.
    pub(crate) fn bytes(&self) -> usize {
        self.edges.len() * (8 + 8 + 1)
            + self.side_edge.len() * 4
            + self.corners_at.bytes()
            + self.ring.bytes()
            + self.rule.len() * 8
    }
}

/// A polygon level seen as the maker of the level above.
pub(crate) struct QuadRefiner<'a> {
    poly: PolyRef<'a>,
    tables: &'a QuadTables,
    kept: usize,
    /// The number of the first face's vertex.
    first_face: usize,
}

impl<'a> QuadRefiner<'a> {
    pub(crate) fn new(vertex_count: usize, poly: PolyRef<'a>, tables: &'a QuadTables) -> QuadRefiner<'a> {
        QuadRefiner { poly, tables, kept: vertex_count, first_face: vertex_count + tables.edges.len() }
    }

    fn face_point(&self, p: &[Vec3], f: usize) -> DVec3 {
        let face = self.poly.face(f);
        face.iter().map(|&v| p[v as usize].as_dvec3()).sum::<DVec3>() / face.len() as f64
    }

    fn kept_point(&self, p: &[Vec3], v: usize) -> Vec3 {
        match self.tables.rule[v] {
            [FIXED, _] => p[v],
            [SMOOTH, _] => {
                let (ring, corners) = (self.tables.ring.row(v), self.tables.corners_at.row(v));
                let n = ring.len() as f64;
                let near = ring.iter().map(|&u| p[u as usize].as_dvec3()).sum::<DVec3>() / n;
                let faces: DVec3 = corners.iter().map(|&c| self.face_point(p, self.poly.face_of(c as usize))).sum();
                let faces = faces / corners.len().max(1) as f64;
                ((faces + near + p[v].as_dvec3() * (n - 2.0)) / n).as_vec3()
            }
            [a, b] => crease_point(p, v, a, b),
        }
    }

    fn edge_point(&self, p: &[Vec3], e: usize) -> Vec3 {
        let [a, b] = self.tables.edges[e];
        let ends = p[a as usize].as_dvec3() + p[b as usize].as_dvec3();
        if self.tables.edge_crease[e] {
            return (ends * 0.5).as_vec3();
        }
        let [s, t] = self.tables.edge_sides[e];
        let faces =
            self.face_point(p, self.poly.face_of(s as usize)) + self.face_point(p, self.poly.face_of(t as usize));
        ((ends + faces) * 0.25).as_vec3()
    }

    /// The new vertex on the face side starting at `corner`.
    #[inline]
    fn side_vertex(&self, corner: usize) -> u32 {
        self.kept as u32 + self.tables.side_edge[corner]
    }

    #[inline]
    fn face_vertex(&self, f: usize) -> u32 {
        (self.first_face + f) as u32
    }
}

impl Refiner for QuadRefiner<'_> {
    fn kept(&self) -> usize {
        self.kept
    }

    fn total(&self) -> usize {
        self.first_face + self.poly.face_count()
    }

    fn between(&self, new: usize) -> &[u32] {
        if new < self.first_face { &self.tables.edges[new - self.kept] } else { self.poly.face(new - self.first_face) }
    }

    fn smooth(&self, coarse: &[Vec3], vertex: usize) -> Vec3 {
        if vertex < self.kept {
            self.kept_point(coarse, vertex)
        } else if vertex < self.first_face {
            self.edge_point(coarse, vertex - self.kept)
        } else {
            self.face_point(coarse, vertex - self.first_face).as_vec3()
        }
    }

    fn fan(&self, vertex: usize, mut wedge: impl FnMut(u32, u32)) {
        let poly = &self.poly;
        if vertex < self.kept {
            // The quad at each face corner here: out along one side, back along the other.
            for &c in self.tables.corners_at.row(vertex) {
                wedge(self.side_vertex(c as usize), self.side_vertex(poly.prev(c as usize)));
            }
        } else if vertex < self.first_face {
            // Two quads in each face on the edge: the one at each end of the side.
            for &c in self.tables.edge_sides[vertex - self.kept].iter().filter(|&&c| c != NONE) {
                let middle = self.face_vertex(poly.face_of(c as usize));
                wedge(middle, poly.corners[c as usize]);
                wedge(poly.corners[poly.next(c as usize)], middle);
            }
        } else {
            let (from, to) = poly.range(vertex - self.first_face);
            for c in from..to {
                wedge(self.side_vertex(poly.prev(c)), self.side_vertex(c));
            }
        }
    }

    fn fine_faces(&self) -> Faces {
        let poly = &self.poly;
        let mut quads = vec![0u32; poly.corners.len() * 4];
        quads.par_chunks_exact_mut(4).with_min_len(PER_TASK).enumerate().for_each(|(c, quad)| {
            let middle = self.face_vertex(poly.face_of(c));
            quad.copy_from_slice(&[poly.corners[c], self.side_vertex(c), middle, self.side_vertex(poly.prev(c))]);
        });
        Faces::Quads(quads)
    }

    /// The quad at a corner is cut from that corner to the face's middle. Its
    /// first triangle lies along the face side leaving the corner and its
    /// second along the side arriving, and each side of a face belongs to one
    /// triangle of the face: that triangle is the parent.
    fn parent_triangles(&self) -> Vec<u32> {
        let poly = &self.poly;
        (0..poly.corners.len() * 2)
            .into_par_iter()
            .with_min_len(PER_TASK)
            .map(|t| poly.side_triangle(if t % 2 == 0 { t / 2 } else { poly.prev(t / 2) }))
            .collect()
    }
}

/// For every quad of the level above `poly`, the face it was made in.
pub(crate) fn parent_faces(poly: PolyRef) -> Vec<u32> {
    (0..poly.corners.len()).into_par_iter().with_min_len(PER_TASK).map(|c| poly.face_of(c) as u32).collect()
}
