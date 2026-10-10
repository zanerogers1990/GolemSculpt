//! Mesh topology as flat arrays: which triangles and which vertices touch each
//! vertex, which triangles meet along each edge, where the boundary runs and
//! which pieces are connected.
//!
//! Everything is a compressed-row table (`Csr`): row `i` is
//! `items[offsets[i]..offsets[i + 1]]`, so a whole relation is two allocations
//! however many vertices there are, and a mesh of tens of millions of triangles
//! is built, held and walked without a heap object per vertex. Builds run on
//! rayon.
//!
//! Degenerate triangles, those with two equal corners, are left out of every
//! table. A local collapse retires a triangle by repeating a corner rather than
//! renumbering the ones after it, so without this a rebuilt map would hand a
//! vertex a pile of incident triangles that draw nothing and a self-neighbour,
//! which every smoothing pass would then average the vertex against itself with.
//!
//! Every list is in ascending order, so a build is deterministic whatever the
//! thread timing. (The Unity app listed a vertex's neighbours in the order its
//! triangles first named them, to keep a smoothing sum bit-identical with the
//! build it had replaced; the Rust app sorts, as `Mesh::neighbours` does.)

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};

use rayon::prelude::*;

use crate::mesh::TriMesh;

/// An edge id that names nothing: the slot of a degenerate triangle's edge.
pub const NO_EDGE: u32 = u32::MAX;

/// A component label that names nothing: a vertex no triangle uses, or a
/// degenerate triangle.
pub const NO_COMPONENT: u32 = u32::MAX;

/// Rows of `u32`s in one flat array. Row `i` is `items[offsets[i]..offsets[i + 1]]`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Csr {
    offsets: Vec<u32>,
    items: Vec<u32>,
}

impl Csr {
    pub fn rows(&self) -> usize {
        self.offsets.len().saturating_sub(1)
    }

    #[inline]
    pub fn row(&self, i: usize) -> &[u32] {
        &self.items[self.offsets[i] as usize..self.offsets[i + 1] as usize]
    }

    /// Where row `i` starts; `offsets()[rows()]` is the total item count.
    #[inline]
    pub fn offsets(&self) -> &[u32] {
        &self.offsets
    }

    #[inline]
    pub fn items(&self) -> &[u32] {
        &self.items
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = &[u32]> {
        (0..self.rows()).map(|i| self.row(i))
    }
}

/// Prefix sums of `counts`: `counts.len() + 1` offsets starting at 0.
fn offsets_of(counts: &[u32]) -> Vec<u32> {
    let mut offsets = Vec::with_capacity(counts.len() + 1);
    let mut total = 0u64;
    offsets.push(0);
    for &c in counts {
        total += c as u64;
        offsets.push(total as u32);
    }
    assert!(total <= u32::MAX as u64, "mesh too large for 32-bit adjacency offsets ({total} entries)");
    offsets
}

/// Rows handed to one rayon task.
const ROWS_PER_TASK: usize = 4096;

/// Calls `fill(scratch, row, items_of_row)` for every row of a table laid out
/// by `offsets`, in parallel. The rows are disjoint ranges of `items`, handed
/// out as separate slices, so nothing here is unsafe. `init` makes a scratch
/// value per task for `fill` to reuse.
fn fill_rows<T: Send, S>(
    offsets: &[u32],
    items: &mut [T],
    init: impl Fn() -> S + Sync + Send,
    fill: impl Fn(&mut S, usize, &mut [T]) + Sync + Send,
) {
    let rows = offsets.len() - 1;
    let mut blocks = Vec::with_capacity(rows / ROWS_PER_TASK + 1);
    let mut rest = items;
    let mut first = 0;
    while first < rows {
        let end = (first + ROWS_PER_TASK).min(rows);
        let (block, tail) = rest.split_at_mut((offsets[end] - offsets[first]) as usize);
        blocks.push((first, end, block));
        rest = tail;
        first = end;
    }
    blocks.into_par_iter().for_each_init(init, |scratch, (first, end, block)| {
        let base = offsets[first] as usize;
        for row in first..end {
            let (from, to) = (offsets[row] as usize - base, offsets[row + 1] as usize - base);
            fill(scratch, row, &mut block[from..to]);
        }
    });
}

/// A table whose row `i` holds `counts[i]` items, each written by `fill`.
fn csr_from_counts<S>(
    counts: &[u32],
    init: impl Fn() -> S + Sync + Send,
    fill: impl Fn(&mut S, usize, &mut [u32]) + Sync + Send,
) -> Csr {
    let offsets = offsets_of(counts);
    let mut items = vec![0u32; offsets[counts.len()] as usize];
    fill_rows(&offsets, &mut items, init, fill);
    Csr { offsets, items }
}

/// A triangle with two equal corners covers no area and has no meaningful topology.
#[inline]
pub fn is_degenerate(corners: &[u32]) -> bool {
    corners[0] == corners[1] || corners[1] == corners[2] || corners[0] == corners[2]
}

/// For every vertex, the triangles that use it and the vertices it shares an
/// edge with.
#[derive(Clone, Debug, Default)]
pub struct MeshAdjacency {
    vertex_count: usize,
    triangle_count: usize,
    triangles: Csr,
    neighbours: Csr,
}

impl MeshAdjacency {
    /// Panics if an index is not below `vertex_count`; a trailing partial
    /// triangle in `indices` is ignored.
    pub fn build(vertex_count: usize, indices: &[u32]) -> MeshAdjacency {
        let indices = &indices[..indices.len() - indices.len() % 3];
        let triangles = incident_triangles(vertex_count, indices);
        let neighbours = vertex_neighbours(vertex_count, indices, &triangles);
        MeshAdjacency { vertex_count, triangle_count: indices.len() / 3, triangles, neighbours }
    }

    pub fn of(mesh: &TriMesh) -> MeshAdjacency {
        MeshAdjacency::build(mesh.vertex_count(), &mesh.indices)
    }

    pub fn vertex_count(&self) -> usize {
        self.vertex_count
    }

    /// Triangles in the index list, degenerate ones included.
    pub fn triangle_count(&self) -> usize {
        self.triangle_count
    }

    /// The triangles using vertex `v`, ascending.
    #[inline]
    pub fn triangles_of(&self, v: usize) -> &[u32] {
        self.triangles.row(v)
    }

    /// The vertices sharing an edge with `v`, ascending, `v` itself not among them.
    #[inline]
    pub fn neighbours_of(&self, v: usize) -> &[u32] {
        self.neighbours.row(v)
    }

    /// How many edges meet at `v`.
    #[inline]
    pub fn valence(&self, v: usize) -> usize {
        self.neighbours.row(v).len()
    }

    /// The whole vertex-to-triangles table, for passes that walk it flat.
    pub fn vertex_triangles(&self) -> &Csr {
        &self.triangles
    }

    /// The whole vertex-to-vertices table.
    pub fn vertex_neighbours(&self) -> &Csr {
        &self.neighbours
    }
}

fn incident_triangles(vertex_count: usize, indices: &[u32]) -> Csr {
    let counters: Vec<AtomicU32> = (0..vertex_count).into_par_iter().map(|_| AtomicU32::new(0)).collect();
    indices.par_chunks_exact(3).for_each(|t| {
        if !is_degenerate(t) {
            for &v in t {
                counters[v as usize].fetch_add(1, Relaxed);
            }
        }
    });
    let counts: Vec<u32> = counters.par_iter().map(|c| c.load(Relaxed)).collect();
    let offsets = offsets_of(&counts);

    // Triangles claim slots in whatever order the threads reach them; sorting
    // each short row afterwards makes the result the same on every run.
    counters.par_iter().zip(offsets.par_iter()).for_each(|(cursor, &start)| cursor.store(start, Relaxed));
    let slots: Vec<AtomicU32> = (0..offsets[vertex_count]).into_par_iter().map(|_| AtomicU32::new(0)).collect();
    indices.par_chunks_exact(3).enumerate().for_each(|(t, corners)| {
        if !is_degenerate(corners) {
            for &v in corners {
                let slot = counters[v as usize].fetch_add(1, Relaxed);
                slots[slot as usize].store(t as u32, Relaxed);
            }
        }
    });
    let mut items: Vec<u32> = slots.into_par_iter().map(AtomicU32::into_inner).collect();
    fill_rows(&offsets, &mut items, || (), |_, _, row| row.sort_unstable());
    Csr { offsets, items }
}

/// The other corners of the triangles around `v`, sorted and without repeats.
fn collect_ring(v: usize, triangles: &Csr, indices: &[u32], ring: &mut Vec<u32>) {
    ring.clear();
    for &t in triangles.row(v) {
        for &o in &indices[t as usize * 3..t as usize * 3 + 3] {
            if o as usize != v {
                ring.push(o);
            }
        }
    }
    ring.sort_unstable();
    ring.dedup();
}

fn vertex_neighbours(vertex_count: usize, indices: &[u32], triangles: &Csr) -> Csr {
    let counts: Vec<u32> = (0..vertex_count)
        .into_par_iter()
        .map_init(Vec::new, |ring, v| {
            collect_ring(v, triangles, indices, ring);
            ring.len() as u32
        })
        .collect();
    csr_from_counts(&counts, Vec::new, |ring, v, row| {
        collect_ring(v, triangles, indices, ring);
        row.copy_from_slice(ring);
    })
}

/// The edges of a mesh, numbered, with the triangles on each.
///
/// Edges are numbered in ascending order of their (lower vertex, higher vertex)
/// pair, so the edges at a vertex that lead to higher vertices are consecutive
/// and `edge_id` is a binary search over a handful of entries.
#[derive(Clone, Debug, Default)]
pub struct MeshEdges {
    edges: Vec<[u32; 2]>,
    /// Id of the first edge whose lower vertex is `v`; `vertex_count + 1` entries.
    first_edge: Vec<u32>,
    edge_triangles: Csr,
    /// Per triangle, the edges of corners (0, 1), (1, 2) and (2, 0).
    triangle_edges: Vec<[u32; 3]>,
}

impl MeshEdges {
    /// `adjacency` must be the adjacency of the same `indices`.
    pub fn build(adjacency: &MeshAdjacency, indices: &[u32]) -> MeshEdges {
        let indices = &indices[..indices.len() - indices.len() % 3];
        assert_eq!(indices.len() / 3, adjacency.triangle_count(), "adjacency is of a different index list");
        let n = adjacency.vertex_count();
        let neighbours = adjacency.vertex_neighbours();

        // Every edge is found once, from its lower vertex: the neighbours above it.
        let above = |v: usize| {
            let row = neighbours.row(v);
            &row[row.partition_point(|&u| (u as usize) < v)..]
        };
        let counts: Vec<u32> = (0..n).into_par_iter().map(|v| above(v).len() as u32).collect();
        let first_edge = offsets_of(&counts);
        let mut edges = vec![[0u32; 2]; first_edge[n] as usize];
        fill_rows(&first_edge, &mut edges, || (), |_, v, row| {
            for (slot, &u) in row.iter_mut().zip(above(v)) {
                *slot = [v as u32, u];
            }
        });

        let counts: Vec<u32> = edges
            .par_iter()
            .map(|&[a, b]| shared_triangles(adjacency, indices, a, b).count() as u32)
            .collect();
        let edge_triangles = csr_from_counts(&counts, || (), |_, e, row| {
            let [a, b] = edges[e];
            for (slot, t) in row.iter_mut().zip(shared_triangles(adjacency, indices, a, b)) {
                *slot = t;
            }
        });

        let mut mesh = MeshEdges { edges, first_edge, edge_triangles, triangle_edges: Vec::new() };
        mesh.triangle_edges = indices
            .par_chunks_exact(3)
            .map(|c| {
                if is_degenerate(c) {
                    return [NO_EDGE; 3];
                }
                let id = |a, b| mesh.edge_id(a, b).expect("every edge of a triangle is in the table");
                [id(c[0], c[1]), id(c[1], c[2]), id(c[2], c[0])]
            })
            .collect();
        mesh
    }

    pub fn of(mesh: &TriMesh, adjacency: &MeshAdjacency) -> MeshEdges {
        MeshEdges::build(adjacency, &mesh.indices)
    }

    /// Number of distinct edges.
    pub fn len(&self) -> usize {
        self.edges.len()
    }

    pub fn is_empty(&self) -> bool {
        self.edges.is_empty()
    }

    /// Every edge as (lower vertex, higher vertex), in edge-id order.
    pub fn edges(&self) -> &[[u32; 2]] {
        &self.edges
    }

    pub fn edge(&self, id: u32) -> [u32; 2] {
        self.edges[id as usize]
    }

    /// The id of the edge between `a` and `b`, whichever way round.
    pub fn edge_id(&self, a: u32, b: u32) -> Option<u32> {
        let (low, high) = (a.min(b) as usize, a.max(b));
        let (from, to) = (*self.first_edge.get(low)? as usize, *self.first_edge.get(low + 1)? as usize);
        let k = self.edges[from..to].binary_search_by_key(&high, |e| e[1]).ok()?;
        Some((from + k) as u32)
    }

    /// The triangles using edge `id`, ascending: two for an interior edge of a
    /// manifold mesh, one on the boundary, more where sheets meet.
    #[inline]
    pub fn triangles_of(&self, id: u32) -> &[u32] {
        self.edge_triangles.row(id as usize)
    }

    /// The whole edge-to-triangles table.
    pub fn edge_triangles(&self) -> &Csr {
        &self.edge_triangles
    }

    /// Triangle `t`'s edges, for its corners (0, 1), (1, 2) and (2, 0); all
    /// `NO_EDGE` for a degenerate triangle.
    #[inline]
    pub fn edges_of_triangle(&self, t: usize) -> [u32; 3] {
        self.triangle_edges[t]
    }

    /// The triangle across each of triangle `t`'s three edges, where exactly one
    /// other triangle shares it.
    pub fn triangle_neighbours(&self, t: usize) -> [Option<u32>; 3] {
        self.triangle_edges[t].map(|e| {
            if e == NO_EDGE {
                return None;
            }
            match *self.triangles_of(e) {
                [a, b] if a as usize == t => Some(b),
                [a, b] if b as usize == t => Some(a),
                _ => None,
            }
        })
    }

    /// Edges with exactly one triangle, ascending.
    pub fn boundary_edges(&self) -> Vec<u32> {
        self.edges_where(|tris| tris.len() == 1)
    }

    /// Edges with three or more triangles, ascending.
    pub fn non_manifold_edges(&self) -> Vec<u32> {
        self.edges_where(|tris| tris.len() > 2)
    }

    fn edges_where(&self, keep: impl Fn(&[u32]) -> bool + Sync) -> Vec<u32> {
        (0..self.len() as u32).into_par_iter().filter(|&e| keep(self.triangles_of(e))).collect()
    }

    /// Every edge has exactly two triangles: no boundary and no sheets meeting.
    /// Says nothing of winding; see `is_consistently_wound`.
    pub fn is_closed(&self) -> bool {
        !self.is_empty() && (0..self.len()).into_par_iter().all(|e| self.edge_triangles.row(e).len() == 2)
    }

    /// Every edge shared by exactly two triangles is walked in opposite
    /// directions by them, as the edges of an outward-wound surface are.
    /// Edges with other counts are not judged here; see `boundary_edges` and
    /// `non_manifold_edges`.
    pub fn is_consistently_wound(&self, indices: &[u32]) -> bool {
        (0..self.len()).into_par_iter().all(|e| match *self.edge_triangles.row(e) {
            [first, second] => {
                let [a, b] = self.edges[e];
                walks(indices, first, a, b) != walks(indices, second, a, b)
            }
            _ => true,
        })
    }

    /// The closed loops of vertices along the boundary, each walked in the
    /// direction of its triangles' winding: with the surface on the left seen
    /// from outside, which for an outward-wound mesh runs counter-clockwise
    /// round the surface and clockwise round the hole. Each loop starts at its
    /// lowest vertex and the loops are in ascending order of that vertex.
    ///
    /// A vertex where the boundary pinches may appear in a loop twice. Boundary
    /// edges that do not close up, which only happens on a non-manifold mesh,
    /// are left out; `non_manifold_edges` says whether to expect that.
    pub fn boundary_loops(&self, indices: &[u32]) -> Vec<Vec<u32>> {
        let mut half_edges: Vec<(u32, u32)> =
            self.boundary_edges().par_iter().map(|&e| self.boundary_half_edge(e, indices)).collect();
        half_edges.par_sort_unstable();

        let mut used = vec![false; half_edges.len()];
        let mut loops = Vec::new();
        for first in 0..half_edges.len() {
            if used[first] {
                continue;
            }
            let start = half_edges[first].0;
            let mut path = vec![start];
            let mut current = first;
            let closed = loop {
                used[current] = true;
                let to = half_edges[current].1;
                if to == start {
                    break true;
                }
                path.push(to);
                let from = half_edges.partition_point(|h| h.0 < to);
                let next = (from..half_edges.len()).take_while(|&k| half_edges[k].0 == to).find(|&k| !used[k]);
                match next {
                    Some(k) => current = k,
                    None => break false,
                }
            };
            if closed {
                loops.push(path);
            }
        }
        loops
    }

    /// Boundary edge `e` as its one triangle walks it.
    fn boundary_half_edge(&self, e: u32, indices: &[u32]) -> (u32, u32) {
        let t = self.triangles_of(e)[0] as usize;
        let j = self.triangle_edges[t].iter().position(|&x| x == e).expect("a triangle lists its own edges");
        (indices[t * 3 + j], indices[t * 3 + (j + 1) % 3])
    }
}

/// Whether triangle `t` goes from `a` to `b` along one of its edges.
fn walks(indices: &[u32], t: u32, a: u32, b: u32) -> bool {
    let c = &indices[t as usize * 3..t as usize * 3 + 3];
    (c[0] == a && c[1] == b) || (c[1] == a && c[2] == b) || (c[2] == a && c[0] == b)
}

/// The triangles using both `a` and `b`, ascending, found by scanning the
/// shorter of the two vertices' lists.
fn shared_triangles<'a>(
    adjacency: &'a MeshAdjacency,
    indices: &'a [u32],
    a: u32,
    b: u32,
) -> impl Iterator<Item = u32> + 'a {
    let (scan, other) = if adjacency.triangles_of(a as usize).len() <= adjacency.triangles_of(b as usize).len() {
        (a, b)
    } else {
        (b, a)
    };
    adjacency
        .triangles_of(scan as usize)
        .iter()
        .copied()
        .filter(move |&t| indices[t as usize * 3..t as usize * 3 + 3].contains(&other))
}

/// The pieces of a mesh that are connected through shared vertices.
#[derive(Clone, Debug, Default)]
pub struct Components {
    /// Number of pieces.
    pub count: usize,
    /// Piece of each vertex, or `NO_COMPONENT` for one no triangle uses.
    pub vertex_label: Vec<u32>,
    /// Piece of each triangle, or `NO_COMPONENT` for a degenerate one.
    pub triangle_label: Vec<u32>,
}

impl Components {
    /// Triangles in each piece.
    pub fn triangle_counts(&self) -> Vec<u32> {
        let mut counts = vec![0u32; self.count];
        for &label in self.triangle_label.iter().filter(|&&l| l != NO_COMPONENT) {
            counts[label as usize] += 1;
        }
        counts
    }
}

/// Splits a mesh into the pieces connected through shared vertices, the way
/// separate shells of one mesh are told apart. Pieces are numbered in ascending
/// order of their lowest vertex.
///
/// A vertex shared by two otherwise separate lumps joins them. Runs in parallel
/// on a lock-free union-find whose roots are always the lowest vertex of their
/// piece, which is also what makes the numbering the same on every run.
pub fn connected_components(vertex_count: usize, indices: &[u32]) -> Components {
    let indices = &indices[..indices.len() - indices.len() % 3];
    let parent: Vec<AtomicU32> = (0..vertex_count).into_par_iter().map(|v| AtomicU32::new(v as u32)).collect();
    let used: Vec<AtomicBool> = (0..vertex_count).into_par_iter().map(|_| AtomicBool::new(false)).collect();
    indices.par_chunks_exact(3).for_each(|t| {
        if is_degenerate(t) {
            return;
        }
        for &v in t {
            used[v as usize].store(true, Relaxed);
        }
        union(&parent, t[0], t[1]);
        union(&parent, t[1], t[2]);
    });

    let root: Vec<u32> = (0..vertex_count)
        .into_par_iter()
        .map(|v| if used[v].load(Relaxed) { find(&parent, v as u32) } else { NO_COMPONENT })
        .collect();
    let mut label_of_root = vec![NO_COMPONENT; vertex_count];
    let mut count = 0;
    for v in 0..vertex_count {
        if root[v] == v as u32 {
            label_of_root[v] = count as u32;
            count += 1;
        }
    }
    let vertex_label: Vec<u32> =
        root.par_iter().map(|&r| if r == NO_COMPONENT { NO_COMPONENT } else { label_of_root[r as usize] }).collect();
    let triangle_label = indices
        .par_chunks_exact(3)
        .map(|t| if is_degenerate(t) { NO_COMPONENT } else { vertex_label[t[0] as usize] })
        .collect();
    Components { count, vertex_label, triangle_label }
}

/// The root of `x`'s set, halving the path on the way up. Parents never exceed
/// their children, so the walk ends, and losing a halving race to another thread
/// is harmless.
fn find(parent: &[AtomicU32], mut x: u32) -> u32 {
    loop {
        let p = parent[x as usize].load(Relaxed);
        if p == x {
            return x;
        }
        let grandparent = parent[p as usize].load(Relaxed);
        if grandparent != p {
            let _ = parent[x as usize].compare_exchange_weak(p, grandparent, Relaxed, Relaxed);
        }
        x = grandparent;
    }
}

/// Joins the sets of `a` and `b`, hanging the root with the higher index under the lower.
fn union(parent: &[AtomicU32], a: u32, b: u32) {
    let (mut a, mut b) = (a, b);
    loop {
        a = find(parent, a);
        b = find(parent, b);
        if a == b {
            return;
        }
        let (low, high) = (a.min(b), a.max(b));
        // Fails if another thread gave `high` a parent first; look the roots up again.
        if parent[high as usize].compare_exchange(high, low, Relaxed, Relaxed).is_ok() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::testing::{self, Rng};

    /// What the tables must hold, found the slow obvious way.
    fn reference(vertex_count: usize, indices: &[u32]) -> (Vec<Vec<u32>>, Vec<BTreeSet<u32>>) {
        let mut tris = vec![Vec::new(); vertex_count];
        let mut ring = vec![BTreeSet::new(); vertex_count];
        for (t, c) in indices.chunks_exact(3).enumerate() {
            if is_degenerate(c) {
                continue;
            }
            for k in 0..3 {
                tris[c[k] as usize].push(t as u32);
                ring[c[k] as usize].insert(c[(k + 1) % 3]);
                ring[c[k] as usize].insert(c[(k + 2) % 3]);
            }
        }
        (tris, ring)
    }

    #[test]
    fn tables_match_a_naive_build_on_random_soups() {
        let mut rng = Rng(7);
        for (vertices, count) in [(5, 3), (40, 200), (2000, 9000), (30_000, 20_000)] {
            let mesh = testing::random_soup(vertices, count, &mut rng);
            let adjacency = MeshAdjacency::of(&mesh);
            let (tris, ring) = reference(vertices, &mesh.indices);
            for v in 0..vertices {
                assert_eq!(adjacency.triangles_of(v), tris[v], "triangles of {v}");
                assert!(adjacency.neighbours_of(v).iter().copied().eq(ring[v].iter().copied()), "ring of {v}");
                assert_eq!(adjacency.valence(v), ring[v].len());
            }
            assert_eq!(adjacency.triangle_count(), count);
        }
    }

    #[test]
    fn a_build_is_the_same_every_time() {
        let mesh = testing::random_soup(5000, 40_000, &mut Rng(3));
        let first = MeshAdjacency::of(&mesh);
        for _ in 0..4 {
            let again = MeshAdjacency::of(&mesh);
            assert_eq!(first.vertex_triangles(), again.vertex_triangles());
            assert_eq!(first.vertex_neighbours(), again.vertex_neighbours());
        }
    }

    #[test]
    fn degenerate_triangles_and_stray_indices_are_ignored() {
        // One real triangle, one with a repeated corner, and two dangling indices.
        let indices = [0, 1, 2, 3, 3, 4, 2, 4];
        let adjacency = MeshAdjacency::build(5, &indices);
        assert_eq!(adjacency.triangle_count(), 2);
        assert_eq!(adjacency.neighbours_of(0), [1, 2]);
        assert!(adjacency.triangles_of(3).is_empty() && adjacency.neighbours_of(3).is_empty());
        assert!(adjacency.neighbours_of(4).is_empty());
        assert_eq!(adjacency.vertex_triangles().rows(), 5);
        assert_eq!(MeshAdjacency::build(0, &[]).vertex_count(), 0);
    }

    #[test]
    fn a_closed_sphere_has_the_edges_euler_says() {
        let sphere = testing::uv_sphere(24, 48);
        let adjacency = MeshAdjacency::of(&sphere);
        let edges = MeshEdges::of(&sphere, &adjacency);
        let (v, e, f) = (sphere.vertex_count() as i64, edges.len() as i64, sphere.triangle_count() as i64);
        assert_eq!(v - e + f, 2);
        assert_eq!(3 * f, 2 * e);
        assert!(edges.is_closed());
        assert!(edges.is_consistently_wound(&sphere.indices));
        assert!(edges.boundary_edges().is_empty() && edges.non_manifold_edges().is_empty());
        assert!(edges.boundary_loops(&sphere.indices).is_empty());
        let pieces = connected_components(sphere.vertex_count(), &sphere.indices);
        assert_eq!(pieces.count, 1);
        assert_eq!(pieces.triangle_counts(), [sphere.triangle_count() as u32]);
    }

    #[test]
    fn edge_ids_and_triangle_edges_agree() {
        let sphere = testing::uv_sphere(12, 20);
        let adjacency = MeshAdjacency::of(&sphere);
        let edges = MeshEdges::of(&sphere, &adjacency);
        for (id, &[a, b]) in edges.edges().iter().enumerate() {
            assert!(a < b);
            assert_eq!(edges.edge_id(a, b), Some(id as u32));
            assert_eq!(edges.edge_id(b, a), Some(id as u32));
            assert!(edges.triangles_of(id as u32).windows(2).all(|w| w[0] < w[1]));
        }
        assert_eq!(edges.edge_id(0, 1), None, "the poles are not joined");
        assert_eq!(edges.edge_id(0, 100_000), None);
        for t in 0..sphere.triangle_count() {
            let c = &sphere.indices[t * 3..t * 3 + 3];
            let ids = edges.edges_of_triangle(t);
            for k in 0..3 {
                let mut ends = [c[k], c[(k + 1) % 3]];
                ends.sort_unstable();
                assert_eq!(edges.edge(ids[k]), ends);
                assert!(edges.triangles_of(ids[k]).contains(&(t as u32)));
            }
            // Across each edge of a closed manifold there is exactly one other triangle.
            for across in edges.triangle_neighbours(t) {
                let other = across.expect("closed") as usize;
                assert_ne!(other, t);
                assert!(edges.edges_of_triangle(other).iter().any(|e| ids.contains(e)));
            }
        }
    }

    #[test]
    fn a_cube_and_a_tetrahedron_are_closed() {
        for mesh in [testing::cube(), testing::tetrahedron()] {
            let adjacency = MeshAdjacency::of(&mesh);
            let edges = MeshEdges::of(&mesh, &adjacency);
            assert!(edges.is_closed());
            assert_eq!(edges.len() as i64, mesh.vertex_count() as i64 + mesh.triangle_count() as i64 - 2);
        }
    }

    #[test]
    fn one_flipped_triangle_breaks_the_winding_but_not_the_closure() {
        let mut sphere = testing::uv_sphere(10, 16);
        sphere.indices.swap(31, 32);
        let adjacency = MeshAdjacency::of(&sphere);
        let edges = MeshEdges::of(&sphere, &adjacency);
        assert!(edges.is_closed());
        assert!(!edges.is_consistently_wound(&sphere.indices));
    }

    /// Twice the area enclosed by a loop in the XY plane, positive when it runs counter-clockwise.
    fn signed_area(mesh: &TriMesh, loop_: &[u32]) -> f32 {
        (0..loop_.len())
            .map(|k| {
                let (a, b) = (mesh.positions[loop_[k] as usize], mesh.positions[loop_[(k + 1) % loop_.len()] as usize]);
                a.x * b.y - b.x * a.y
            })
            .sum()
    }

    #[test]
    fn a_sheet_has_one_boundary_loop_running_with_its_winding() {
        let (nx, ny) = (5, 3);
        let sheet = testing::grid(nx, ny);
        let adjacency = MeshAdjacency::of(&sheet);
        let edges = MeshEdges::of(&sheet, &adjacency);
        assert!(!edges.is_closed());
        assert!(edges.is_consistently_wound(&sheet.indices));
        assert_eq!(edges.boundary_edges().len(), 2 * (nx + ny));
        let loops = edges.boundary_loops(&sheet.indices);
        assert_eq!(loops.len(), 1);
        assert_eq!(loops[0].len(), 2 * (nx + ny));
        assert_eq!(loops[0][0], 0, "starts at its lowest vertex");
        // The sheet faces +Z with counter-clockwise triangles, so its outline runs counter-clockwise.
        assert!((signed_area(&sheet, &loops[0]) - 2.0 * (nx * ny) as f32).abs() < 1e-4);
    }

    #[test]
    fn a_cube_with_a_face_removed_has_one_square_hole() {
        let mut cube = testing::cube();
        // The first quad is the z = -1 face: its two triangles are the first six indices.
        cube.indices.drain(0..6);
        let adjacency = MeshAdjacency::of(&cube);
        let edges = MeshEdges::of(&cube, &adjacency);
        let loops = edges.boundary_loops(&cube.indices);
        assert_eq!(loops.len(), 1);
        assert_eq!(loops[0].len(), 4);
        let corners: BTreeSet<u32> = loops[0].iter().copied().collect();
        assert_eq!(corners, BTreeSet::from([0, 1, 2, 3]));
        // Seen from outside, here from below, the walls are on the left and the hole is walked
        // clockwise; seen from above, as the area is measured, that is counter-clockwise.
        assert!(signed_area(&cube, &loops[0]) > 0.0);
    }

    #[test]
    fn cutting_the_corner_cells_off_a_sheet_keeps_one_outline() {
        let mut sheet = testing::grid(6, 2);
        // Punch out the first and the last cell (two triangles each).
        let last = sheet.triangle_count() - 2;
        sheet.indices.drain(last * 3..);
        sheet.indices.drain(0..6);
        let adjacency = MeshAdjacency::of(&sheet);
        let edges = MeshEdges::of(&sheet, &adjacency);
        let loops = edges.boundary_loops(&sheet.indices);
        // Each corner cell swaps two outline edges for two others: the outline is still 16 long.
        assert_eq!(loops.len(), 1);
        assert_eq!(loops[0].len(), 2 * (6 + 2));
        assert_eq!(edges.boundary_edges().len(), loops[0].len());
    }

    #[test]
    fn a_sheet_with_an_interior_hole_has_two_loops() {
        let mut sheet = testing::grid(4, 4);
        // The middle cell of a 4 x 4 grid is cell (1, 1): triangles 2 * (1 * 4 + 1) and the next.
        sheet.indices.drain(2 * (4 + 1) * 3..(2 * (4 + 1) + 2) * 3);
        let adjacency = MeshAdjacency::of(&sheet);
        let edges = MeshEdges::of(&sheet, &adjacency);
        let loops = edges.boundary_loops(&sheet.indices);
        assert_eq!(loops.len(), 2);
        assert_eq!(loops[0].len(), 16, "the outline");
        assert_eq!(loops[1].len(), 4, "the hole");
        assert!(signed_area(&sheet, &loops[0]) > 0.0 && signed_area(&sheet, &loops[1]) < 0.0);
    }

    #[test]
    fn three_sheets_on_one_edge_are_non_manifold() {
        // Three triangles fanned round the edge 0-1.
        let indices = [0, 1, 2, 0, 1, 3, 0, 1, 4];
        let adjacency = MeshAdjacency::build(5, &indices);
        let edges = MeshEdges::build(&adjacency, &indices);
        let shared = edges.edge_id(0, 1).unwrap();
        assert_eq!(edges.non_manifold_edges(), [shared]);
        assert_eq!(edges.triangles_of(shared), [0, 1, 2]);
        assert_eq!(edges.triangle_neighbours(0)[0], None, "three triangles are not a neighbour");
        assert!(!edges.is_closed());
    }

    #[test]
    fn components_number_pieces_by_lowest_vertex() {
        let mut mesh = testing::cube();
        let first = mesh.vertex_count();
        mesh.append(&testing::tetrahedron());
        mesh.positions.push(glam::Vec3::ONE); // used by nothing
        mesh.indices.extend_from_slice(&[0, 0, 5]); // degenerate
        let pieces = connected_components(mesh.vertex_count(), &mesh.indices);
        assert_eq!(pieces.count, 2);
        assert!(pieces.vertex_label[..first].iter().all(|&l| l == 0));
        assert!(pieces.vertex_label[first..first + 4].iter().all(|&l| l == 1));
        assert_eq!(*pieces.vertex_label.last().unwrap(), NO_COMPONENT);
        assert_eq!(*pieces.triangle_label.last().unwrap(), NO_COMPONENT);
        assert_eq!(pieces.triangle_counts(), [12, 4]);
    }

    /// The parallel union-find must find the same pieces as the obvious flood fill, on soups full
    /// of chains, hubs and strays.
    #[test]
    fn components_match_a_flood_fill_on_random_graphs() {
        let mut rng = Rng(99);
        for (vertices, count) in [(50, 20), (3000, 1500), (3000, 4000), (100_000, 60_000)] {
            let mesh = testing::random_soup(vertices, count, &mut rng);
            let pieces = connected_components(vertices, &mesh.indices);

            let mut seen = vec![NO_COMPONENT; vertices];
            let (tris, _) = reference(vertices, &mesh.indices);
            let mut next = 0;
            for start in 0..vertices {
                if tris[start].is_empty() || seen[start] != NO_COMPONENT {
                    continue;
                }
                seen[start] = next;
                let mut stack = vec![start];
                while let Some(v) = stack.pop() {
                    for &t in &tris[v] {
                        for &o in &mesh.indices[t as usize * 3..t as usize * 3 + 3] {
                            if seen[o as usize] == NO_COMPONENT {
                                seen[o as usize] = next;
                                stack.push(o as usize);
                            }
                        }
                    }
                }
                next += 1;
            }
            assert_eq!(pieces.count, next as usize);
            assert_eq!(pieces.vertex_label, seen, "labels agree because both number by lowest vertex");
        }
    }

    #[test]
    fn an_empty_mesh_is_fine() {
        let adjacency = MeshAdjacency::build(0, &[]);
        let edges = MeshEdges::build(&adjacency, &[]);
        assert!(edges.is_empty() && !edges.is_closed());
        assert!(edges.boundary_loops(&[]).is_empty());
        assert_eq!(connected_components(0, &[]).count, 0);
        let lonely = MeshAdjacency::build(3, &[]);
        assert!(lonely.neighbours_of(1).is_empty());
    }

    /// A million-triangle sphere through every build; run with `--ignored --nocapture`, release.
    #[test]
    #[ignore]
    fn timing_on_a_million_triangles() {
        use std::time::Instant;
        let sphere = testing::uv_sphere(700, 715);
        println!("{} vertices, {} triangles", sphere.vertex_count(), sphere.triangle_count());
        let t = Instant::now();
        let adjacency = MeshAdjacency::of(&sphere);
        println!("adjacency      {:?}", t.elapsed());
        let t = Instant::now();
        let edges = MeshEdges::of(&sphere, &adjacency);
        println!("edges          {:?} ({} edges)", t.elapsed(), edges.len());
        let t = Instant::now();
        let closed = edges.is_closed() && edges.is_consistently_wound(&sphere.indices);
        println!("closed + wound {:?} ({closed})", t.elapsed());
        let t = Instant::now();
        let pieces = connected_components(sphere.vertex_count(), &sphere.indices);
        println!("components    {:?} ({} piece)", t.elapsed(), pieces.count);
    }
}
