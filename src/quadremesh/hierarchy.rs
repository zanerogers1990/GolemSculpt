//! The multi-resolution hierarchy the orientation and position fields are
//! solved on, coarse to fine (Jakob et al. 2015, "Instant Field-Aligned
//! Meshes", section 5).
//!
//! Solving on the finest graph alone would need thousands of smoothing sweeps
//! for information to cross a 450k-vertex model; solved coarsest-first and
//! handed down, each level only has to fix up local detail, so a few sweeps
//! per level are enough.

use std::ops::Range;

use glam::Vec3;
use rayon::prelude::*;

use super::poly::{Edges, NONE, normalize_or};

/// One resolution: a point cloud with a neighbour graph. Level 0 is the
/// conditioned mesh's own vertices and edges. Each coarser level merges pairs
/// of neighbours, so it is a graph over area-weighted clusters, not a mesh:
/// the field solvers only need positions, normals, areas and neighbours.
#[derive(Default)]
pub struct Level {
    pub position: Vec<Vec3>,
    /// Unit length.
    pub normal: Vec<Vec3>,
    /// Surface area each vertex stands for; sums to the mesh area on every level.
    pub area: Vec<f32>,

    /// Neighbours of v: `adj[adj_start[v]..adj_start[v + 1]]`, symmetric, no
    /// self-loops, no duplicates. A link's weight is how many finest-level
    /// edges it stands for.
    pub adj_start: Vec<u32>,
    pub adj: Vec<u32>,
    pub adj_weight: Vec<f32>,

    /// Links between levels. `to_coarse` is empty on the coarsest level;
    /// `fine_a` / `fine_b` are empty on level 0. `fine_b` is NONE where a
    /// vertex was carried up alone rather than merged.
    pub to_coarse: Vec<u32>,
    pub fine_a: Vec<u32>,
    pub fine_b: Vec<u32>,

    /// Level 0 only (empty above it): the vertices on an open edge of the
    /// mesh, one with a triangle on one side only. A closed mesh has none.
    pub open: Vec<bool>,

    /// Graph colouring: the vertices of one phase share no edge, so a solver
    /// can update a whole phase in parallel and still be Gauss-Seidel.
    pub phase_start: Vec<u32>,
    pub phase_verts: Vec<u32>,
}

impl Level {
    pub fn count(&self) -> usize {
        self.position.len()
    }

    #[inline]
    pub fn links(&self, v: usize) -> Range<usize> {
        self.adj_start[v] as usize..self.adj_start[v + 1] as usize
    }
}

pub struct Hierarchy {
    pub levels: Vec<Level>,
}

/// Coarsening stops at this many vertices, or when a level fails to shrink by
/// at least 10% (a graph of isolated vertices cannot merge further).
const MIN_VERTICES: usize = 64;

impl Hierarchy {
    pub fn build(vertices: &[Vec3], triangles: &[u32]) -> Hierarchy {
        let mut levels = vec![finest(vertices, triangles)];
        while levels.last().unwrap().count() > MIN_VERTICES && levels.len() < 48 {
            let fine = levels.last_mut().unwrap();
            let coarse = coarsen(fine);
            if coarse.count() as f64 > fine.count() as f64 * 0.9 {
                fine.to_coarse = Vec::new();
                break;
            }
            levels.push(coarse);
        }
        levels.par_iter_mut().for_each(colour);
        Hierarchy { levels }
    }
}

fn finest(vertices: &[Vec3], triangles: &[u32]) -> Level {
    let n = vertices.len();
    let mut normal = vec![Vec3::ZERO; n];
    let mut area = vec![0.0f64; n];
    for t in triangles.chunks_exact(3) {
        let a = vertices[t[0] as usize];
        // The raw cross product is 2 x area along the normal: area-weighted.
        let cross = (vertices[t[1] as usize] - a).cross(vertices[t[2] as usize] - a);
        let third = cross.length() as f64 / 6.0;
        for &i in t {
            normal[i as usize] += cross;
            area[i as usize] += third;
        }
    }

    let edges = Edges::of_triangles(triangles);
    let mut start = vec![0u32; n + 1];
    let mut open = vec![false; n];
    for e in 0..edges.count() {
        start[edges.a[e] as usize + 1] += 1;
        start[edges.b[e] as usize + 1] += 1;
        if edges.uses[e] != 2 {
            open[edges.a[e] as usize] = true;
            open[edges.b[e] as usize] = true;
        }
    }
    for v in 0..n {
        start[v + 1] += start[v];
    }
    let mut fill = start.clone();
    let mut adj = vec![0u32; start[n] as usize];
    for e in 0..edges.count() {
        let (a, b) = (edges.a[e], edges.b[e]);
        adj[fill[a as usize] as usize] = b;
        fill[a as usize] += 1;
        adj[fill[b as usize] as usize] = a;
        fill[b as usize] += 1;
    }

    Level {
        position: vertices.to_vec(),
        normal: normal.into_iter().map(|n| normalize_or(n, Vec3::Y)).collect(),
        area: area.into_iter().map(|a| a as f32).collect(),
        adj_weight: vec![1.0; adj.len()],
        adj_start: start,
        adj,
        open,
        ..Level::default()
    }
}

/// One matching step. Every edge is scored `dot(n_i, n_j) * max(A_i / A_j,
/// A_j / A_i)`: merge neighbours that face the same way first, and among
/// those prefer lopsided pairs, which evens out cluster sizes (a small
/// cluster is absorbed before two big ones combine). Edges are then taken
/// greedily in score order, each vertex merging at most once.
fn coarsen(level: &mut Level) -> Level {
    let fine = &*level;
    let n = fine.count();
    let mut scored: Vec<(f32, u32, u32)> = (0..n)
        .into_par_iter()
        .flat_map_iter(|v| {
            fine.links(v).filter_map(move |k| {
                let u = fine.adj[k] as usize;
                if u <= v {
                    return None;
                }
                let (av, au) = (fine.area[v].max(1e-30), fine.area[u].max(1e-30));
                let ratio = if av > au { av / au } else { au / av };
                Some((-fine.normal[v].dot(fine.normal[u]) * ratio, v as u32, u as u32))
            })
        })
        .collect();
    scored.par_sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then((a.1, a.2).cmp(&(b.1, b.2))));

    let mut to_coarse = vec![NONE; n];
    let mut fine_a = Vec::with_capacity(n / 2 + 1);
    let mut fine_b = Vec::with_capacity(n / 2 + 1);
    for &(_, v, u) in &scored {
        if to_coarse[v as usize] != NONE || to_coarse[u as usize] != NONE {
            continue;
        }
        to_coarse[v as usize] = fine_a.len() as u32;
        to_coarse[u as usize] = fine_a.len() as u32;
        fine_a.push(v);
        fine_b.push(u);
    }
    for v in 0..n {
        if to_coarse[v] == NONE {
            to_coarse[v] = fine_a.len() as u32;
            fine_a.push(v as u32);
            fine_b.push(NONE);
        }
    }
    let count = fine_a.len();

    let mut coarse = Level {
        position: vec![Vec3::ZERO; count],
        normal: vec![Vec3::ZERO; count],
        area: vec![0.0; count],
        ..Level::default()
    };
    for c in 0..count {
        let a = fine_a[c] as usize;
        if fine_b[c] == NONE {
            coarse.position[c] = fine.position[a];
            coarse.normal[c] = fine.normal[a];
            coarse.area[c] = fine.area[a];
            continue;
        }
        let b = fine_b[c] as usize;
        let (mut wa, mut wb) = (fine.area[a], fine.area[b]);
        let mut sum = wa + wb;
        if !(sum > 0.0) {
            (wa, wb, sum) = (0.5, 0.5, 1.0);
        }
        coarse.position[c] = (fine.position[a] * wa + fine.position[b] * wb) / sum;
        coarse.normal[c] = normalize_or(fine.normal[a] * wa + fine.normal[b] * wb, fine.normal[a]);
        coarse.area[c] = fine.area[a] + fine.area[b];
    }

    // Coarse neighbours: the union of the children's neighbours, mapped up,
    // minus the cluster itself, with duplicate links merged and their weights
    // summed. `slot` remembers where each coarse neighbour already sits in
    // the list being built.
    let mut c_start = Vec::with_capacity(count + 1);
    let mut c_adj: Vec<u32> = Vec::with_capacity(fine.adj.len() / 2);
    let mut c_weight: Vec<f32> = Vec::with_capacity(fine.adj.len() / 2);
    let mut slot = vec![NONE; count];
    for c in 0..count {
        let first = c_adj.len();
        c_start.push(first as u32);
        for f in [fine_a[c], fine_b[c]] {
            if f == NONE {
                continue;
            }
            for k in fine.links(f as usize) {
                let nc = to_coarse[fine.adj[k] as usize];
                if nc as usize == c {
                    continue;
                }
                let at = slot[nc as usize] as usize;
                if slot[nc as usize] != NONE && at >= first && at < c_adj.len() && c_adj[at] == nc {
                    c_weight[at] += fine.adj_weight[k];
                    continue;
                }
                slot[nc as usize] = c_adj.len() as u32;
                c_adj.push(nc);
                c_weight.push(fine.adj_weight[k]);
            }
        }
    }
    c_start.push(c_adj.len() as u32);

    level.to_coarse = to_coarse;
    coarse.adj_start = c_start;
    coarse.adj = c_adj;
    coarse.adj_weight = c_weight;
    coarse.fine_a = fine_a;
    coarse.fine_b = fine_b;
    coarse
}

/// Greedy colouring in index order: each vertex takes the lowest colour none
/// of its already-coloured neighbours has.
fn colour(level: &mut Level) {
    let n = level.count();
    let mut colour = vec![0u32; n];
    let mut seen_by: Vec<u32> = vec![NONE; 64];
    let mut colours = 0u32;
    for v in 0..n {
        for k in level.links(v) {
            let u = level.adj[k] as usize;
            if u < v {
                let cu = colour[u] as usize;
                if cu >= seen_by.len() {
                    seen_by.resize((cu + 1).max(seen_by.len() * 2), NONE);
                }
                seen_by[cu] = v as u32;
            }
        }
        let c = seen_by.iter().position(|&s| s != v as u32).unwrap_or(seen_by.len()) as u32;
        colour[v] = c;
        colours = colours.max(c + 1);
    }

    let mut phase_start = vec![0u32; colours as usize + 1];
    for &c in &colour {
        phase_start[c as usize + 1] += 1;
    }
    for c in 0..colours as usize {
        phase_start[c + 1] += phase_start[c];
    }
    let mut fill = phase_start.clone();
    let mut phase_verts = vec![0u32; n];
    for v in 0..n {
        let c = colour[v] as usize;
        phase_verts[fill[c] as usize] = v as u32;
        fill[c] += 1;
    }
    level.phase_start = phase_start;
    level.phase_verts = phase_verts;
}
