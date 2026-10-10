//! Whether a triangle list is a closed surface, counted fast enough to ask
//! after every operation that rebuilds a mesh.
//!
//! A printable solid has every edge shared by exactly two triangles. An edge
//! with one triangle is open: it lies on the rim of a hole. An edge with three
//! or more is where sheets meet, and a slicer cannot tell which side of it is
//! inside. [`edge_census`] counts both, and how many separate rims the open
//! edges form, which is the number of holes a person would count.
//!
//! [`MeshEdges`](crate::adjacency::MeshEdges) answers the same questions, but
//! it first builds every table a tool might walk (triangles per vertex,
//! neighbours, numbered edges, edges per triangle). This is the count alone:
//! every triangle side is dropped into the row of its lower vertex, one flat
//! array for the whole mesh, and each short row is sorted and read off. On a
//! million triangles that is several times quicker (see the timing test).
//!
//! Degenerate triangles, those with two equal corners, are left out, as they
//! are from every table in `adjacency` and for the same reason: a local
//! collapse retires a triangle that way, and it covers nothing.

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering::Relaxed};

use rayon::prelude::*;

use crate::adjacency::is_degenerate;
use crate::mesh::TriMesh;

/// Vertices handed to one rayon task.
const ROWS_PER_TASK: usize = 4096;

/// Most open edges whose rims are told apart. A mesh past this is not a solid
/// with a few holes but a soup (every triangle with corners of its own, say),
/// and how many rims it has helps nobody.
const MOST_RIM_EDGES: usize = 1 << 20;

/// How the edges of a mesh fall short of a closed surface. All zero for a
/// closed one (and for no mesh at all).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EdgeCensus {
    /// Edges with exactly one triangle: the rim of a hole.
    pub open: usize,
    /// Edges with three or more triangles: sheets meeting along a line.
    pub crowded: usize,
    /// The separate rims the open edges form, which is how many holes there
    /// are to see. `None` when there are too many open edges for that to
    /// mean anything (`MOST_RIM_EDGES`).
    pub holes: Option<usize>,
}

impl EdgeCensus {
    /// Every edge has exactly two triangles. Says nothing of winding, nor of
    /// surfaces that pass through each other.
    pub fn is_closed(&self) -> bool {
        self.open == 0 && self.crowded == 0
    }

    pub fn of(mesh: &TriMesh) -> EdgeCensus {
        edge_census(mesh.vertex_count(), &mesh.indices)
    }
}

/// Counts the open and the crowded edges of a triangle list.
///
/// Panics if an index is not below `vertex_count`; a trailing partial triangle
/// in `indices` is ignored. Hidden faces are a matter for the caller: pass the
/// whole index list, since a hidden face is still part of the model.
pub fn edge_census(vertex_count: usize, indices: &[u32]) -> EdgeCensus {
    let indices = &indices[..indices.len() - indices.len() % 3];
    assert!(indices.len() <= u32::MAX as usize, "mesh too large for 32-bit edge slots ({} sides)", indices.len());
    let (ends, slots) = sides_by_lower_vertex(vertex_count, indices);
    let row = |v: usize| {
        let from = if v == 0 { 0 } else { ends[v - 1].load(Relaxed) };
        &slots[from as usize..ends[v].load(Relaxed) as usize]
    };

    let kept = AtomicUsize::new(0);
    let tally = (0..vertex_count)
        .into_par_iter()
        .with_min_len(ROWS_PER_TASK)
        .fold(Tally::default, |mut tally, v| {
            tally.read_row(v as u32, row(v), &kept);
            tally
        })
        .reduce(Tally::default, Tally::merged);
    let holes = (tally.open <= MOST_RIM_EDGES).then(|| rim_count(&tally.rim));
    EdgeCensus { open: tally.open, crowded: tally.crowded, holes }
}

/// Every side of every sound triangle, as its higher vertex filed in the row
/// of its lower one. Row `v` of the slots ends at `ends[v]` and starts where
/// row `v - 1` ends. Within a row the order is whatever the threads made it.
fn sides_by_lower_vertex(vertex_count: usize, indices: &[u32]) -> (Vec<AtomicU32>, Vec<AtomicU32>) {
    let cursors: Vec<AtomicU32> = (0..vertex_count).into_par_iter().map(|_| AtomicU32::new(0)).collect();
    for_each_side(indices, |low, _| {
        cursors[low as usize].fetch_add(1, Relaxed);
    });
    // Counts into starts; filling then walks each cursor to its row's end.
    let mut total = 0u32;
    for cursor in &cursors {
        total += cursor.swap(total, Relaxed);
    }
    let slots: Vec<AtomicU32> = (0..total).into_par_iter().map(|_| AtomicU32::new(0)).collect();
    for_each_side(indices, |low, high| {
        let slot = cursors[low as usize].fetch_add(1, Relaxed);
        slots[slot as usize].store(high, Relaxed);
    });
    (cursors, slots)
}

/// Calls `side(lower vertex, higher vertex)` for the three sides of every
/// triangle that is not degenerate, in parallel.
fn for_each_side(indices: &[u32], side: impl Fn(u32, u32) + Sync) {
    indices.par_chunks_exact(3).for_each(|t| {
        if is_degenerate(t) {
            return;
        }
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            side(a.min(b), a.max(b));
        }
    });
}

/// The counts of one task, and the open edges it met.
#[derive(Default)]
struct Tally {
    open: usize,
    crowded: usize,
    rim: Vec<[u32; 2]>,
    /// One row at a time, sorted here: the slots themselves are shared.
    scratch: Vec<u32>,
}

impl Tally {
    /// Counts the edges whose lower vertex is `low`. An edge appears in the
    /// row once for each triangle on it.
    fn read_row(&mut self, low: u32, row: &[AtomicU32], kept: &AtomicUsize) {
        self.scratch.clear();
        self.scratch.extend(row.iter().map(|high| high.load(Relaxed)));
        self.scratch.sort_unstable();
        let mut from = 0;
        while from < self.scratch.len() {
            let high = self.scratch[from];
            let uses = self.scratch[from..].iter().take_while(|&&other| other == high).count();
            match uses {
                1 => {
                    self.open += 1;
                    if kept.fetch_add(1, Relaxed) < MOST_RIM_EDGES {
                        self.rim.push([low, high]);
                    }
                }
                2 => {}
                _ => self.crowded += 1,
            }
            from += uses;
        }
    }

    fn merged(mut self, mut other: Tally) -> Tally {
        self.open += other.open;
        self.crowded += other.crowded;
        self.rim.append(&mut other.rim);
        self
    }
}

/// How many connected groups a set of edges forms: two edges are in one group
/// when they share a vertex, directly or through others.
fn rim_count(rim: &[[u32; 2]]) -> usize {
    let mut vertices: Vec<u32> = rim.iter().flatten().copied().collect();
    vertices.par_sort_unstable();
    vertices.dedup();
    let slot = |v: u32| vertices.binary_search(&v).expect("every rim vertex was listed") as u32;
    let mut parent: Vec<u32> = (0..vertices.len() as u32).collect();
    let mut groups = vertices.len();
    for &[a, b] in rim {
        let (a, b) = (root(&mut parent, slot(a)), root(&mut parent, slot(b)));
        if a != b {
            parent[a.max(b) as usize] = a.min(b);
            groups -= 1;
        }
    }
    groups
}

/// The root of `x`'s group, halving the path on the way up.
fn root(parent: &mut [u32], mut x: u32) -> u32 {
    while parent[x as usize] != x {
        parent[x as usize] = parent[parent[x as usize] as usize];
        x = parent[x as usize];
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adjacency::{MeshAdjacency, MeshEdges};
    use crate::testing::{self, Rng};

    const CLOSED: EdgeCensus = EdgeCensus { open: 0, crowded: 0, holes: Some(0) };

    #[test]
    fn closed_shapes_have_nothing_to_report() {
        for mesh in [testing::cube(), testing::tetrahedron(), testing::uv_sphere(24, 48)] {
            let census = EdgeCensus::of(&mesh);
            assert_eq!(census, CLOSED);
            assert!(census.is_closed());
        }
        // Two closed shells in one mesh are still closed.
        let mut both = testing::cube();
        both.append(&testing::tetrahedron());
        assert_eq!(EdgeCensus::of(&both), CLOSED);
    }

    #[test]
    fn no_mesh_and_unused_vertices_count_nothing() {
        assert_eq!(edge_census(0, &[]), CLOSED);
        assert_eq!(edge_census(7, &[]), CLOSED);
        // Two stray indices past the last whole triangle are ignored.
        let mut cube = testing::cube();
        cube.indices.extend_from_slice(&[0, 1]);
        assert_eq!(EdgeCensus::of(&cube), CLOSED);
    }

    #[test]
    fn a_cube_with_a_face_removed_has_one_hole_of_four_edges() {
        let mut cube = testing::cube();
        cube.indices.drain(0..6);
        let census = EdgeCensus::of(&cube);
        assert_eq!(census, EdgeCensus { open: 4, crowded: 0, holes: Some(1) });
        assert!(!census.is_closed());
    }

    #[test]
    fn holes_apart_are_counted_apart_and_holes_that_touch_as_one() {
        // A quad out of the top of a sphere and another out of the bottom: the
        // band triangles of ring r start at 2 * segs + (r - 1) * 2 * segs.
        let (rings, segs) = (12, 16);
        let quad = |r: usize, j: usize| (2 * segs + (r - 1) * 2 * segs + 2 * j) * 3;
        let mut sphere = testing::uv_sphere(rings, segs);
        let (high, low) = (quad(2, 3), quad(9, 3));
        sphere.indices.drain(low..low + 6);
        sphere.indices.drain(high..high + 6);
        assert_eq!(EdgeCensus::of(&sphere), EdgeCensus { open: 8, crowded: 0, holes: Some(2) });

        // Two quads side by side: one hole, six edges round it.
        let mut sphere = testing::uv_sphere(rings, segs);
        let at = quad(5, 3);
        sphere.indices.drain(at..at + 12);
        assert_eq!(EdgeCensus::of(&sphere), EdgeCensus { open: 6, crowded: 0, holes: Some(1) });
    }

    #[test]
    fn a_sheet_with_a_hole_in_it_has_two_rims() {
        let mut sheet = testing::grid(4, 4);
        sheet.indices.drain(2 * (4 + 1) * 3..(2 * (4 + 1) + 2) * 3);
        assert_eq!(EdgeCensus::of(&sheet), EdgeCensus { open: 20, crowded: 0, holes: Some(2) });
    }

    #[test]
    fn three_sheets_on_one_edge_are_crowded() {
        let census = edge_census(5, &[0, 1, 2, 0, 1, 3, 0, 1, 4]);
        // The shared edge, and the two free sides of each of the three triangles, all joined at 0 and 1.
        assert_eq!(census, EdgeCensus { open: 6, crowded: 1, holes: Some(1) });
    }

    #[test]
    fn the_same_face_twice_is_crowded_not_closed() {
        let mut cube = testing::cube();
        let again: Vec<u32> = cube.indices[0..3].to_vec();
        cube.indices.extend_from_slice(&again);
        assert_eq!(EdgeCensus::of(&cube), EdgeCensus { open: 0, crowded: 3, holes: Some(0) });
    }

    #[test]
    fn degenerate_triangles_are_left_out() {
        let mut cube = testing::cube();
        cube.indices.extend_from_slice(&[0, 0, 5, 3, 4, 4, 2, 2, 2]);
        assert_eq!(EdgeCensus::of(&cube), CLOSED);
    }

    /// The quick count must agree with the full edge table on meshes full of
    /// everything that can go wrong.
    #[test]
    fn counts_match_the_edge_table_on_random_soups() {
        let mut rng = Rng(11);
        for (vertices, count) in [(5, 3), (40, 200), (2000, 9000), (30_000, 20_000), (300, 20_000)] {
            let mesh = testing::random_soup(vertices, count, &mut rng);
            let edges = MeshEdges::of(&mesh, &MeshAdjacency::of(&mesh));
            let census = EdgeCensus::of(&mesh);
            assert_eq!(census.open, edges.boundary_edges().len(), "{vertices} vertices, {count} triangles");
            assert_eq!(census.crowded, edges.non_manifold_edges().len(), "{vertices} vertices, {count} triangles");
            assert_eq!(census.is_closed(), edges.is_closed() || edges.is_empty());
        }
    }

    #[test]
    fn rims_are_counted_like_boundary_loops_on_a_clean_mesh() {
        // Three cells of the middle row punched out, none touching another or the outline.
        let mut sheet = testing::grid(9, 3);
        for cell in [16, 13, 10] {
            sheet.indices.drain(cell * 6..cell * 6 + 6);
        }
        let edges = MeshEdges::of(&sheet, &MeshAdjacency::of(&sheet));
        let loops = edges.boundary_loops(&sheet.indices);
        let census = EdgeCensus::of(&sheet);
        assert_eq!(census.open, loops.iter().map(Vec::len).sum::<usize>());
        assert_eq!(census.holes, Some(loops.len()));
        assert_eq!(loops.len(), 4, "the outline and three holes in the middle row");
    }

    #[test]
    fn a_soup_of_loose_triangles_is_all_open_edges_and_its_rims_are_not_counted() {
        // Each triangle with three corners of its own: more open edges than rims are told apart for.
        let triangles = MOST_RIM_EDGES / 3 + 1;
        let indices: Vec<u32> = (0..triangles as u32 * 3).collect();
        let census = edge_census(triangles * 3, &indices);
        assert_eq!(census, EdgeCensus { open: triangles * 3, crowded: 0, holes: None });
        // One fewer and they are: a rim round each triangle.
        let census = edge_census(triangles * 3, &indices[..indices.len() - 3]);
        assert_eq!(census.holes, Some(triangles - 1));
    }

    /// The count against the full edge table, on a closed sphere and on one
    /// with a hole; run with `--ignored --nocapture`, release.
    #[test]
    #[ignore]
    fn timing_of_the_census() {
        use std::time::Instant;
        for (rings, segs) in [(700, 715), (2200, 2275), (4400, 4550)] {
            let mut sphere = testing::uv_sphere(rings, segs);
            let best = |work: &dyn Fn() -> EdgeCensus| {
                (0..3)
                    .map(|_| {
                        let t = Instant::now();
                        let census = work();
                        (t.elapsed(), census)
                    })
                    .min_by_key(|(time, _)| *time)
                    .unwrap()
            };
            let (closed_time, closed) = best(&|| EdgeCensus::of(&sphere));
            assert_eq!(closed, CLOSED);
            let t = Instant::now();
            let adjacency = MeshAdjacency::of(&sphere);
            let edges = MeshEdges::of(&sphere, &adjacency);
            let table = (edges.boundary_edges().len(), edges.non_manifold_edges().len());
            let table_time = t.elapsed();
            assert_eq!(table, (0, 0));
            let cut = sphere.indices.len() / 2 / 3 * 3;
            sphere.indices.drain(cut..cut + 3000);
            let (holed_time, holed) = best(&|| EdgeCensus::of(&sphere));
            println!(
                "{:>10} triangles: census {closed_time:.1?} closed, {holed_time:.1?} with {} open edges ({:?} holes); \
                 the full edge table {table_time:.1?}",
                sphere.triangle_count() + 1000,
                holed.open,
                holed.holes,
            );
        }
    }
}
