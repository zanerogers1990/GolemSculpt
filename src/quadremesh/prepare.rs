//! Makes a sculpt safe for the field solve without moving any surface:
//! cut open what is not manifold, drop debris, and split over-long edges.

use glam::Vec3;
use rayon::prelude::*;

use super::poly::{Edges, NONE, find, union};

/// The remesher's conditioned input. It describes the same shape as the
/// sculpt; the only differences are removed debris, cut-open non-manifold
/// spots and extra vertices along long edges.
pub struct Prepared {
    pub vertices: Vec<Vec3>,
    pub triangles: Vec<u32>,
    /// Target quad edge length: sqrt(area / quads).
    pub quad_edge: f32,
    /// Separate pieces of the sculpt left out as debris: each with less area
    /// than one quad. They are gone from the result, so the caller says so.
    pub dropped_pieces: usize,
}

/// Hard ceiling on the conditioned vertex count, whatever the target asks
/// for: protection for a huge target on a tiny model.
pub const MAX_VERTICES: usize = 3_000_000;

pub fn prepare(vertices: &[Vec3], triangles: &[u32], target_quads: usize) -> Prepared {
    // A piece with less area than one quad cannot hold a quad.
    let raw_area = surface_area(vertices, triangles);
    let (v, t, dropped_pieces) = sanitize(vertices, triangles, raw_area / target_quads as f64);
    let area = surface_area(&v, &t);
    let quad_edge = (area / target_quads as f64).sqrt() as f32;
    // The edge limit Instant Meshes uses: half the quad edge, but never more
    // than twice the mesh's mean edge, so stretched regions also come down
    // toward the density of the rest of the model.
    let max_edge = (quad_edge as f64 * 0.5).min(mean_edge_length(&v, &t) * 2.0) as f32;
    let (vertices, triangles) = split_long_edges(v, t, max_edge, MAX_VERTICES);
    Prepared { vertices, triangles, quad_edge, dropped_pieces }
}

pub fn surface_area(v: &[Vec3], t: &[u32]) -> f64 {
    t.par_chunks_exact(3)
        .map(|t| {
            let a = v[t[0] as usize];
            0.5 * (v[t[1] as usize] - a).cross(v[t[2] as usize] - a).length() as f64
        })
        .sum()
}

fn mean_edge_length(v: &[Vec3], t: &[u32]) -> f64 {
    let edges = Edges::of_triangles(t);
    if edges.count() == 0 {
        return 0.0;
    }
    let sum: f64 = (0..edges.count())
        .map(|e| v[edges.a[e] as usize].distance(v[edges.b[e] as usize]) as f64)
        .sum();
    sum / edges.count() as f64
}

/// Imported models are rarely clean (pieces, open edges, edges with three
/// faces, pinched vertices), and every later stage assumes a manifold graph.
/// Repairs are local cuts, never a voxel pass, which would fuse nearby
/// surfaces:
///   1. Faces with a repeated corner, and repeats of the same face, go.
///   2. Every vertex whose faces form more than one fan is split, one copy
///      per fan. Fans join only across proper manifold edges, so a
///      non-manifold or misoriented edge is cut open: it becomes a boundary.
///   3. Pieces with less area than `min_area` are dropped, and counted.
///   4. Unused vertices are removed.
fn sanitize(vertices: &[Vec3], triangles: &[u32], min_area: f64) -> (Vec<Vec3>, Vec<u32>, usize) {
    let mut t = remove_bad_triangles(vertices.len(), triangles);
    let mut v = vertices.to_vec();
    split_fans(&mut v, &mut t);
    let (t, dropped) = remove_small_components(&v, t, min_area);
    let (v, t) = compact(v, t);
    (v, t, dropped)
}

fn remove_bad_triangles(vertex_count: usize, triangles: &[u32]) -> Vec<u32> {
    let n = vertex_count as u32;
    let mut keyed: Vec<([u32; 3], u32)> = triangles
        .chunks_exact(3)
        .enumerate()
        .filter(|(_, t)| t[0] != t[1] && t[1] != t[2] && t[2] != t[0] && t.iter().all(|&i| i < n))
        .map(|(f, t)| {
            let mut key = [t[0], t[1], t[2]];
            key.sort_unstable();
            (key, f as u32)
        })
        .collect();
    keyed.par_sort_unstable();
    let mut keep = vec![false; triangles.len() / 3];
    for (i, (key, f)) in keyed.iter().enumerate() {
        if i == 0 || keyed[i - 1].0 != *key {
            keep[*f as usize] = true;
        }
    }
    triangles
        .chunks_exact(3)
        .zip(&keep)
        .filter(|(_, keep)| **keep)
        .flat_map(|(t, _)| t.iter().copied())
        .collect()
}

/// Rewrites `t` so every vertex has a single fan, appending the copies to `v`.
fn split_fans(v: &mut Vec<Vec3>, t: &mut [u32]) {
    let count = v.len();
    let edges = Edges::of_triangles(t);

    let mut corner_start = vec![0u32; count + 1];
    for &i in t.iter() {
        corner_start[i as usize + 1] += 1;
    }
    for i in 0..count {
        corner_start[i + 1] += corner_start[i];
    }
    let mut fill = corner_start.clone();
    let mut corners = vec![0u32; t.len()];
    for (c, &i) in t.iter().enumerate() {
        corners[fill[i as usize] as usize] = c as u32;
        fill[i as usize] += 1;
    }

    let mut parent: Vec<u32> = Vec::new();
    let mut sides: Vec<(u32, u32)> = Vec::new();
    let mut copy: Vec<u32> = Vec::new();
    for vert in 0..count {
        let (cs, ce) = (corner_start[vert] as usize, corner_start[vert + 1] as usize);
        let n = ce - cs;
        if n <= 1 {
            continue;
        }
        parent.clear();
        parent.extend(0..n as u32);
        sides.clear();
        for i in 0..n {
            let c = corners[cs + i] as usize;
            let f = c / 3;
            let prev = f * 3 + (c - f * 3 + 2) % 3;
            sides.push((edges.side_edge[c], i as u32));
            sides.push((edges.side_edge[prev], i as u32));
        }
        // The same edge appears once per face around this vertex that has it.
        sides.sort_unstable();
        let mut fans = n;
        for pair in sides.windows(2) {
            let (e, owner) = pair[0];
            if e != NONE && pair[1].0 == e && edges.is_manifold(e as usize) && union(&mut parent, owner, pair[1].1) {
                fans -= 1;
            }
        }
        if fans == 1 {
            continue;
        }
        // The fan holding the first corner keeps the vertex; each other fan
        // gets a copy.
        copy.clear();
        copy.resize(n, NONE);
        let first = find(&mut parent, 0);
        copy[first as usize] = vert as u32;
        for i in 0..n {
            let root = find(&mut parent, i as u32) as usize;
            if copy[root] == NONE {
                copy[root] = v.len() as u32;
                v.push(v[vert]);
            }
            t[corners[cs + i] as usize] = copy[root];
        }
    }
}

/// The triangles of every piece with at least `min_area`, and how many pieces
/// had less.
fn remove_small_components(v: &[Vec3], t: Vec<u32>, min_area: f64) -> (Vec<u32>, usize) {
    let mut parent: Vec<u32> = (0..v.len() as u32).collect();
    for tri in t.chunks_exact(3) {
        union(&mut parent, tri[0], tri[1]);
        union(&mut parent, tri[0], tri[2]);
    }
    let mut area = vec![0.0f64; v.len()];
    for tri in t.chunks_exact(3) {
        let a = v[tri[0] as usize];
        let root = find(&mut parent, tri[0]) as usize;
        area[root] += 0.5 * (v[tri[1] as usize] - a).cross(v[tri[2] as usize] - a).length() as f64;
    }
    let mut kept = Vec::with_capacity(t.len());
    let mut dropped = vec![false; v.len()];
    for tri in t.chunks_exact(3) {
        let root = find(&mut parent, tri[0]) as usize;
        if area[root] >= min_area {
            kept.extend_from_slice(tri);
        } else {
            dropped[root] = true;
        }
    }
    (kept, dropped.iter().filter(|&&d| d).count())
}

fn compact(v: Vec<Vec3>, mut t: Vec<u32>) -> (Vec<Vec3>, Vec<u32>) {
    let mut map = vec![NONE; v.len()];
    for &i in &t {
        map[i as usize] = 0;
    }
    let mut count = 0u32;
    for m in map.iter_mut().filter(|m| **m == 0) {
        *m = count;
        count += 1;
    }
    if count as usize == v.len() {
        return (v, t);
    }
    let mut out = vec![Vec3::ZERO; count as usize];
    for (old, &new) in map.iter().enumerate() {
        if new != NONE {
            out[new as usize] = v[old];
        }
    }
    for i in &mut t {
        *i = map[*i as usize];
    }
    (out, t)
}

/// Splits every triangle edge longer than `max_edge`, until none is.
///
/// The field solve treats mesh edges as the graph it smooths over and
/// extracts from, so an edge longer than about half a quad spans lattice
/// cells it never samples, and extraction there reads garbage. Sculpting
/// makes exactly those edges: a Move or Snake Hook drag stretches the
/// triangles on its flanks into long needles.
///
/// Split-only, never collapse and never voxelise: every new vertex is an edge
/// midpoint on a flat triangle, so the surface is the same, bit for bit, and
/// nothing closer than a voxel gets fused (fingers, lips, an ear against the
/// head). Each round is red-green refinement: every over-long edge gets a
/// midpoint and each triangle is replaced by the 2, 3 or 4 triangles its
/// marked sides call for. Both triangles on an edge read the same mark, so
/// the result is always conforming, and a round halves every over-long edge.
/// Refinement stops before the round that would pass `max_vertices`.
fn split_long_edges(mut v: Vec<Vec3>, mut t: Vec<u32>, max_edge: f32, max_vertices: usize) -> (Vec<Vec3>, Vec<u32>) {
    let max_sq = max_edge * max_edge;
    for _ in 0..32 {
        let edges = Edges::of_triangles(&t);
        // Midpoint vertex per over-long edge.
        let mut mid = vec![NONE; edges.count()];
        let mut marked = 0usize;
        for e in 0..edges.count() {
            if v[edges.a[e] as usize].distance_squared(v[edges.b[e] as usize]) > max_sq {
                mid[e] = (v.len() + marked) as u32;
                marked += 1;
            }
        }
        if marked == 0 || v.len() + marked > max_vertices {
            break;
        }
        let old = v.len();
        v.resize(old + marked, Vec3::ZERO);
        for e in 0..edges.count() {
            if mid[e] != NONE {
                // From the lower index, so the edge gives the same bits
                // whichever triangle asks.
                let (a, b) = (v[edges.a[e] as usize], v[edges.b[e] as usize]);
                v[mid[e] as usize] = a + (b - a) * 0.5;
            }
        }

        let mid_of = |side: usize| {
            let e = edges.side_edge[side];
            if e == NONE { NONE } else { mid[e as usize] }
        };
        let mut nt: Vec<u32> = Vec::with_capacity(t.len() + marked * 6);
        for f in 0..t.len() / 3 {
            let s = f * 3;
            let m = [mid_of(s), mid_of(s + 1), mid_of(s + 2)];
            let count = m.iter().filter(|&&m| m != NONE).count();
            match count {
                0 => nt.extend_from_slice(&t[s..s + 3]),
                3 => {
                    let (a, b, c) = (t[s], t[s + 1], t[s + 2]);
                    nt.extend_from_slice(&[a, m[0], m[2], m[0], b, m[1], m[2], m[1], c, m[0], m[1], m[2]]);
                }
                _ => {
                    // Rotate so the marked sides come first: side k runs
                    // corner k -> k + 1.
                    let r = if count == 1 {
                        m.iter().position(|&m| m != NONE).unwrap()
                    } else {
                        (m.iter().position(|&m| m == NONE).unwrap() + 1) % 3
                    };
                    let (ca, cb, cc) = (t[s + r], t[s + (r + 1) % 3], t[s + (r + 2) % 3]);
                    let (ma, mb) = (m[r], m[(r + 1) % 3]);
                    if count == 1 {
                        nt.extend_from_slice(&[ca, ma, cc, ma, cb, cc]);
                    } else {
                        // Sides a-b and b-c split: the corner triangle at b,
                        // then the quad (a, ma, mb, c) along its shorter
                        // diagonal.
                        nt.extend_from_slice(&[ma, cb, mb]);
                        let d0 = v[ca as usize].distance_squared(v[mb as usize]);
                        let d1 = v[ma as usize].distance_squared(v[cc as usize]);
                        if d0 <= d1 {
                            nt.extend_from_slice(&[ca, ma, mb, ca, mb, cc]);
                        } else {
                            nt.extend_from_slice(&[ca, ma, cc, ma, mb, cc]);
                        }
                    }
                }
            }
        }
        t = nt;
    }
    (v, t)
}
