//! Polygon meshes: vertices plus faces of any size, stored flat. This is what
//! the quad remesher produces. The sculpting engine only ever sees triangles
//! ([`PolyMesh::triangulate`]); the polygons are kept for display and export.

use glam::Vec3;
use rayon::prelude::*;

/// "No index": a degenerate face side, a vertex carried up alone, and so on.
pub const NONE: u32 = u32::MAX;

/// Face `f`'s corners are `face_indices[face_start[f]..face_start[f + 1]]`,
/// wound like the engine's triangles: `(b - a).cross(c - a)` points outward.
#[derive(Clone)]
pub struct PolyMesh {
    pub vertices: Vec<Vec3>,
    pub face_start: Vec<u32>,
    pub face_indices: Vec<u32>,
}

impl PolyMesh {
    pub fn with_vertices(vertices: Vec<Vec3>) -> PolyMesh {
        PolyMesh { vertices, face_start: vec![0], face_indices: Vec::new() }
    }

    pub fn vertex_count(&self) -> usize {
        self.vertices.len()
    }

    pub fn face_count(&self) -> usize {
        self.face_start.len() - 1
    }

    #[inline]
    pub fn face(&self, f: usize) -> &[u32] {
        &self.face_indices[self.face_start[f] as usize..self.face_start[f + 1] as usize]
    }

    #[inline]
    pub fn face_size(&self, f: usize) -> usize {
        (self.face_start[f + 1] - self.face_start[f]) as usize
    }

    pub fn faces(&self) -> impl Iterator<Item = &[u32]> {
        (0..self.face_count()).map(|f| self.face(f))
    }

    pub fn add_vertex(&mut self, p: Vec3) -> u32 {
        self.vertices.push(p);
        self.vertices.len() as u32 - 1
    }

    pub fn add_face(&mut self, corners: &[u32]) {
        self.face_indices.extend_from_slice(corners);
        self.face_start.push(self.face_indices.len() as u32);
    }

    /// The same vertices with no faces yet: the start of every rebuild.
    pub fn rebuild(&self) -> PolyMesh {
        PolyMesh::with_vertices(self.vertices.clone())
    }

    /// Drops vertices no face references, renumbering the rest in order.
    pub fn remove_unused_vertices(mut self) -> PolyMesh {
        let mut map = vec![NONE; self.vertices.len()];
        for &v in &self.face_indices {
            map[v as usize] = 0;
        }
        let mut count = 0u32;
        for m in map.iter_mut().filter(|m| **m == 0) {
            *m = count;
            count += 1;
        }
        if count as usize == self.vertices.len() {
            return self;
        }
        let mut vertices = vec![Vec3::ZERO; count as usize];
        for (old, &new) in map.iter().enumerate() {
            if new != NONE {
                vertices[new as usize] = self.vertices[old];
            }
        }
        for v in &mut self.face_indices {
            *v = map[*v as usize];
        }
        self.vertices = vertices;
        self
    }

    /// Triangles for the sculpting engine, winding kept.
    ///
    /// Quads split along their shorter diagonal: the longer one would put a
    /// fold across the flatter direction of a non-planar quad and makes
    /// thinner triangles. Larger faces fan from their first corner; the
    /// remesher's rare pentagons are nearly convex, where a fan is fine.
    pub fn triangulate(&self) -> Vec<u32> {
        let p = &self.vertices;
        let mut tris = Vec::with_capacity(self.face_indices.len() * 3 / 2);
        for f in self.faces() {
            match f.len() {
                0..=2 => {}
                4 => {
                    let [a, b, c, d] = [f[0], f[1], f[2], f[3]];
                    let ac = p[a as usize].distance_squared(p[c as usize]);
                    let bd = p[b as usize].distance_squared(p[d as usize]);
                    if ac <= bd {
                        tris.extend_from_slice(&[a, b, c, a, c, d]);
                    } else {
                        tris.extend_from_slice(&[a, b, d, b, c, d]);
                    }
                }
                n => {
                    for k in 1..n - 1 {
                        tris.extend_from_slice(&[f[0], f[k], f[k + 1]]);
                    }
                }
            }
        }
        tris
    }
}

/// The unique undirected edges of a mesh, with how every face side maps onto
/// them. Built by sorting packed (min, max) keys: at a million faces a sort is
/// faster and lighter than a hash map, and the edge order is deterministic.
///
/// Face side `k` of a face runs from corner `k` to corner `k + 1`. A side
/// whose two corners are the same vertex belongs to no edge and maps to NONE.
pub struct Edges {
    /// Lower vertex index of each edge.
    pub a: Vec<u32>,
    /// Higher vertex index.
    pub b: Vec<u32>,
    /// Face sides on the edge: 1 = boundary, 2 = interior, 3+ = non-manifold.
    pub uses: Vec<u32>,
    /// Of those, how many run a -> b. A consistently wound interior edge has 1.
    pub forward: Vec<u32>,
    /// Per face side (indexed like the face indices): its edge.
    pub side_edge: Vec<u32>,
}

impl Edges {
    pub fn count(&self) -> usize {
        self.a.len()
    }

    /// Two faces, running the edge in opposite directions.
    #[inline]
    pub fn is_manifold(&self, e: usize) -> bool {
        self.uses[e] == 2 && self.forward[e] == 1
    }

    pub fn of_poly(mesh: &PolyMesh) -> Edges {
        Self::build(&mesh.face_start, &mesh.face_indices)
    }

    pub fn of_triangles(tris: &[u32]) -> Edges {
        let start: Vec<u32> = (0..=tris.len() as u32 / 3).map(|f| f * 3).collect();
        Self::build(&start, tris)
    }

    fn build(start: &[u32], idx: &[u32]) -> Edges {
        let mut keyed: Vec<(u64, u32)> = Vec::with_capacity(idx.len());
        for f in 0..start.len() - 1 {
            let (s, e) = (start[f] as usize, start[f + 1] as usize);
            for c in s..e {
                let a = idx[c];
                let b = idx[if c + 1 < e { c + 1 } else { s }];
                if a != b {
                    keyed.push(((a.min(b) as u64) << 32 | a.max(b) as u64, c as u32));
                }
            }
        }
        keyed.par_sort_unstable();

        let mut edges = Edges {
            a: Vec::new(),
            b: Vec::new(),
            uses: Vec::new(),
            forward: Vec::new(),
            side_edge: vec![NONE; idx.len()],
        };
        for (i, &(key, side)) in keyed.iter().enumerate() {
            if i == 0 || keyed[i - 1].0 != key {
                edges.a.push((key >> 32) as u32);
                edges.b.push(key as u32);
                edges.uses.push(0);
                edges.forward.push(0);
            }
            let e = edges.a.len() - 1;
            edges.side_edge[side as usize] = e as u32;
            edges.uses[e] += 1;
            if idx[side as usize] == edges.a[e] {
                edges.forward[e] += 1;
            }
        }
        edges
    }

    /// The (up to two) faces on each edge, NONE where there is none. Meant
    /// for meshes that are manifold along edges.
    pub fn faces(&self, mesh: &PolyMesh) -> (Vec<u32>, Vec<u32>) {
        let mut fa = vec![NONE; self.count()];
        let mut fb = vec![NONE; self.count()];
        for f in 0..mesh.face_count() {
            for c in mesh.face_start[f]..mesh.face_start[f + 1] {
                let e = self.side_edge[c as usize];
                if e == NONE {
                    continue;
                }
                if fa[e as usize] == NONE {
                    fa[e as usize] = f as u32;
                } else {
                    fb[e as usize] = f as u32;
                }
            }
        }
        (fa, fb)
    }
}

pub fn find(parent: &mut [u32], mut x: u32) -> u32 {
    while parent[x as usize] != x {
        let up = parent[parent[x as usize] as usize];
        parent[x as usize] = up;
        x = up;
    }
    x
}

pub fn union(parent: &mut [u32], a: u32, b: u32) -> bool {
    let (ra, rb) = (find(parent, a), find(parent, b));
    if ra != rb {
        parent[ra as usize] = rb;
    }
    ra != rb
}

/// `v` normalised, or `fallback` when it has no length to speak of.
#[inline]
pub fn normalize_or(v: Vec3, fallback: Vec3) -> Vec3 {
    let len = v.length();
    if len > 1e-20 && len.is_finite() { v / len } else { fallback }
}
