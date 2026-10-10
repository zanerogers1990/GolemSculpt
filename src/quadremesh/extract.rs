//! Turns solved orientation + position fields into a quad-dominant mesh.
//!
//!  1. Every edge of the finest graph is classified by the lattice offset
//!     between its two vertices' lattice points: equal means the two sample
//!     the same output vertex ("collapse"); one step apart means they sample
//!     neighbouring output vertices (an output edge); anything else
//!     (diagonals, longer jumps) is ignored.
//!  2. Collapse edges are unioned into clusters, one per output vertex,
//!     positioned at the area-weighted mean of its members' lattice points.
//!  3. Output edges link clusters. Vertices left with fewer than two links
//!     are pruned (they cannot bound a face).
//!  4. Each cluster's links are ordered by angle around its normal (a
//!     rotation system) and faces are traced from it. Tracing a rotation
//!     system always yields an orientable surface in which every link borders
//!     exactly two face sides, so the result is manifold along edges by
//!     construction.
//!  5. Faces are cleaned: a face whose corner angles sum like the OUTSIDE of
//!     a polygon is the trace around an open boundary of the input and is
//!     dropped (the hole stays a hole); faces of five or more sides are split
//!     into quads (and one triangle when odd) around a centre vertex; a face
//!     that revisits a vertex is cut into its simple loops.
//!
//! The output positions are the fields' tangent-plane lattice points, not yet
//! on the surface; the caller projects and relaxes them afterwards.

use std::collections::VecDeque;
use std::f64::consts::PI;

use glam::Vec3;
use rayon::prelude::*;

use super::field::{any_tangent, compat_orientation, compat_position_index};
use super::hierarchy::Level;
use super::poly::{PolyMesh, find, normalize_or, union};

const MAX_FACE_SIZE: usize = 64;

const IGNORE: u8 = 0;
const COLLAPSE: u8 = 1;
const OUTPUT: u8 = 2;

/// The mesh, and how many traced loops were left out of it: walks too long
/// to be a face, and loops that read as the outside of an opening. On a model
/// with open edges the second is the trace round each rim and is expected; on
/// a closed model either one leaves a hole.
pub fn extract(level: &Level, q: &[Vec3], o: &[Vec3], scale: f32) -> (PolyMesh, usize) {
    let n = level.count();
    let inv = 1.0 / scale;

    // 1. Classify each edge once, from its lower endpoint.
    let kind: Vec<u8> = (0..n)
        .into_par_iter()
        .flat_map_iter(|v| {
            level.links(v).map(move |k| {
                let u = level.adj[k] as usize;
                if u < v {
                    return IGNORE;
                }
                let (q0, q1) = compat_orientation(q[v], level.normal[v], q[u], level.normal[u]);
                let (i0, i1) = compat_position_index(
                    level.position[v],
                    level.normal[v],
                    q0,
                    o[v],
                    level.position[u],
                    level.normal[u],
                    q1,
                    o[u],
                    scale,
                    inv,
                );
                match (i0[0] - i1[0]).abs() + (i0[1] - i1[1]).abs() {
                    0 => COLLAPSE,
                    1 => OUTPUT,
                    _ => IGNORE,
                }
            })
        })
        .collect();

    // 2. Clusters.
    let mut parent: Vec<u32> = (0..n as u32).collect();
    for v in 0..n {
        for k in level.links(v) {
            if kind[k] == COLLAPSE {
                union(&mut parent, v as u32, level.adj[k]);
            }
        }
    }
    let mut cluster = vec![0u32; n];
    let mut count = 0usize;
    for v in 0..n {
        if find(&mut parent, v as u32) == v as u32 {
            cluster[v] = count as u32;
            count += 1;
        }
    }
    for v in 0..n {
        cluster[v] = cluster[find(&mut parent, v as u32) as usize];
    }

    let mut pos = vec![Vec3::ZERO; count];
    let mut normal = vec![Vec3::ZERO; count];
    let mut weight = vec![0.0f32; count];
    for v in 0..n {
        let c = cluster[v] as usize;
        let w = level.area[v].max(1e-30);
        pos[c] += o[v] * w;
        normal[c] += level.normal[v] * w;
        weight[c] += w;
    }
    for c in 0..count {
        pos[c] /= weight[c];
        normal[c] = normalize_or(normal[c], Vec3::Y);
    }

    // 3. Links between clusters, deduplicated.
    let mut link_keys: Vec<u64> = Vec::new();
    for v in 0..n {
        for k in level.links(v) {
            if kind[k] != OUTPUT {
                continue;
            }
            let (a, b) = (cluster[v], cluster[level.adj[k] as usize]);
            if a != b {
                link_keys.push((a.min(b) as u64) << 32 | a.max(b) as u64);
            }
        }
    }
    link_keys.par_sort_unstable();
    link_keys.dedup();
    let mut neighbours: Vec<Vec<u32>> = vec![Vec::new(); count];
    for key in link_keys {
        let (a, b) = ((key >> 32) as u32, key as u32);
        neighbours[a as usize].push(b);
        neighbours[b as usize].push(a);
    }

    // Prune vertices with fewer than two links, repeatedly: a dangling chain
    // unravels.
    let mut alive = vec![true; count];
    let mut queue: VecDeque<u32> = (0..count as u32).filter(|&c| neighbours[c as usize].len() < 2).collect();
    while let Some(c) = queue.pop_front() {
        if !std::mem::replace(&mut alive[c as usize], false) {
            continue;
        }
        for u in std::mem::take(&mut neighbours[c as usize]) {
            if alive[u as usize] {
                neighbours[u as usize].retain(|&x| x != c);
                if neighbours[u as usize].len() < 2 {
                    queue.push_back(u);
                }
            }
        }
    }

    // 4. Rotation system: links sorted by angle, counter-clockwise about the
    // normal.
    let mut angles: Vec<Vec<f32>> = vec![Vec::new(); count];
    for c in 0..count {
        if neighbours[c].is_empty() {
            continue;
        }
        let s = any_tangent(normal[c]);
        let t = normal[c].cross(s);
        let mut order: Vec<(f32, u32)> = neighbours[c]
            .iter()
            .map(|&u| {
                let d = pos[u as usize] - pos[c];
                (d.dot(t).atan2(d.dot(s)), u)
            })
            .collect();
        order.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        neighbours[c] = order.iter().map(|x| x.1).collect();
        angles[c] = order.iter().map(|x| x.0).collect();
    }

    // Dart (c -> neighbours[c][i]) has index dart_start[c] + i.
    let mut dart_start = vec![0usize; count + 1];
    for c in 0..count {
        dart_start[c + 1] = dart_start[c] + neighbours[c].len();
    }
    let mut used = vec![false; dart_start[count]];

    let mut out = PolyMesh::with_vertices(pos);
    let mut left_out = 0;
    let mut face: Vec<u32> = Vec::new();
    let mut loops: Vec<Vec<u32>> = Vec::new();
    for c0 in 0..count {
        for i0 in 0..neighbours[c0].len() {
            if used[dart_start[c0] + i0] {
                continue;
            }
            // Trace: arriving at u from c, leave along the link just BEFORE c
            // in u's counter-clockwise order. That walks each face with the
            // surface on the left of travel, i.e. wound so that
            // (b - a).cross(c - a) points outward.
            face.clear();
            let (mut c, mut i) = (c0, i0);
            let mut closed = false;
            while face.len() <= MAX_FACE_SIZE {
                let dart = dart_start[c] + i;
                if used[dart] {
                    closed = c == c0 && i == i0;
                    break;
                }
                used[dart] = true;
                face.push(c as u32);
                let u = neighbours[c][i] as usize;
                let back = neighbours[u].iter().position(|&x| x as usize == c).unwrap();
                let links = neighbours[u].len();
                c = u;
                i = (back + links - 1) % links;
            }
            if !closed {
                left_out += 1;
                continue;
            }
            // A face that passes through a vertex twice wraps around a spur
            // or pinches at that vertex: emit each simple loop separately.
            // Two-vertex loops are the spur itself (an edge walked there and
            // back) and cover nothing.
            loops.clear();
            split_into_simple_loops(&face, &mut loops);
            for simple in &loops {
                if simple.len() >= 3 && !emit_face(simple, &neighbours, &angles, &mut out) {
                    left_out += 1;
                }
            }
        }
    }
    (out.remove_unused_vertices(), left_out)
}

/// Adds the face, split into quads if it has five sides or more. False, and
/// nothing is added, for a loop that reads as the outside of an opening.
fn emit_face(face: &[u32], neighbours: &[Vec<u32>], angles: &[Vec<f32>], out: &mut PolyMesh) -> bool {
    // Interior of a planar k-gon: angles sum to (k - 2) * 180; the same loop
    // traced around its outside sums to (k + 2) * 180. Split the difference.
    // Only for k >= 5: a real opening in the sculpt is many quads around,
    // while a triangle or quad that reads as "outside" is a folded face on a
    // closed surface, and covering a hole that small is harmless anyway.
    let k = face.len();
    if k >= 5 {
        let sum: f64 = (0..k)
            .map(|j| corner_angle(face[(j + k - 1) % k], face[j], face[(j + 1) % k], neighbours, angles))
            .sum();
        if sum > k as f64 * PI {
            return false;
        }
    }
    if k <= 4 {
        out.add_face(face);
        return true;
    }
    // Centre fan: quads (centre, f[2m], f[2m + 1], f[2m + 2]); odd k leaves
    // one triangle.
    let centre = face.iter().map(|&v| out.vertices[v as usize]).sum::<Vec3>() / k as f32;
    let cv = out.add_vertex(centre);
    let mut m = 0;
    while m + 2 <= k - (k & 1) {
        out.add_face(&[cv, face[m], face[m + 1], face[(m + 2) % k]]);
        m += 2;
    }
    if k & 1 == 1 {
        out.add_face(&[cv, face[k - 1], face[0]]);
    }
    true
}

/// Splits a closed walk into simple cycles: whenever a vertex repeats, the
/// stretch between its two visits is a closed loop of its own and is cut out.
fn split_into_simple_loops(walk: &[u32], loops: &mut Vec<Vec<u32>>) {
    let mut stack: Vec<u32> = Vec::with_capacity(walk.len());
    for &v in walk {
        if let Some(at) = stack.iter().rposition(|&x| x == v) {
            loops.push(stack[at..].to_vec());
            stack.truncate(at + 1);
        } else {
            stack.push(v);
        }
    }
    if !stack.is_empty() {
        loops.push(stack);
    }
}

/// Angle of the face corner at c between the link to `prev` and the link to
/// `next`, swept clockwise from prev to next (the face side, given how faces
/// are traced).
fn corner_angle(prev: u32, c: u32, next: u32, neighbours: &[Vec<u32>], angles: &[Vec<f32>]) -> f64 {
    let nb = &neighbours[c as usize];
    let at = |v: u32| angles[c as usize][nb.iter().position(|&x| x == v).unwrap()] as f64;
    let mut sweep = at(prev) - at(next);
    while sweep <= 0.0 {
        sweep += 2.0 * PI;
    }
    while sweep > 2.0 * PI {
        sweep -= 2.0 * PI;
    }
    sweep
}
