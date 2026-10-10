//! Small topology fixes for the extractor's output.
//!
//!  - Interior vertices with only two edges are dissolved: such a vertex is a
//!    kink in an edge shared by two faces and only makes both faces worse.
//!    Removing it from both faces keeps the mesh consistent (each face loses
//!    one corner; a face left with two corners is the sliver it formed and
//!    goes too).
//!  - Adjacent triangles are paired into quads. Extraction leaves triangles
//!    at field singularities, and most of them come in side-by-side pairs: a
//!    quad that was cut in two. Pairs merge best-shape first, and never into
//!    a quad with a corner of 160 degrees or more.
//!  - Short quad strips that run from one leftover triangle to another are
//!    collapsed, which removes both triangles and both of their poles.
//!  - Small stray pieces split off from a larger one are dropped.

use std::collections::HashMap;

use glam::Vec3;

use super::poly::{Edges, NONE, PolyMesh, find};

const MAX_MERGED_CORNER_DEG: f32 = 160.0;

/// Longest strip (in quads) `collapse_triangle_chords` will remove. A strip
/// this long takes one row of quads out of the lattice over that distance;
/// the neighbours stretch by half a quad to cover it, which the relaxation
/// evens out.
const MAX_CHORD_LENGTH: usize = 6;
const CHORD_PASSES: usize = 6;
const FRAGMENT_MAX_FACES: u32 = 32;

pub fn clean(mut mesh: PolyMesh) -> PolyMesh {
    for _ in 0..4 {
        match dissolve_valence_two(&mesh) {
            Some(next) => mesh = next,
            None => break,
        }
    }
    mesh = pair_triangles(mesh);
    for _ in 0..CHORD_PASSES {
        match collapse_triangle_chords(&mesh) {
            Some(next) => mesh = next,
            None => break,
        }
        if let Some(next) = dissolve_valence_two(&mesh) {
            mesh = next;
        }
        mesh = pair_triangles(mesh);
    }
    mesh.remove_unused_vertices()
}

fn dissolve_valence_two(mesh: &PolyMesh) -> Option<PolyMesh> {
    let edges = Edges::of_poly(mesh);
    let n = mesh.vertex_count();
    let mut valence = vec![0u32; n];
    let mut on_boundary = vec![false; n];
    for e in 0..edges.count() {
        let (a, b) = (edges.a[e] as usize, edges.b[e] as usize);
        valence[a] += 1;
        valence[b] += 1;
        if edges.uses[e] != 2 {
            on_boundary[a] = true;
            on_boundary[b] = true;
        }
    }
    let remove: Vec<bool> = (0..n).map(|v| valence[v] == 2 && !on_boundary[v]).collect();
    if !remove.contains(&true) {
        return None;
    }
    let mut out = mesh.rebuild();
    let mut corners: Vec<u32> = Vec::new();
    for face in mesh.faces() {
        corners.clear();
        corners.extend(face.iter().filter(|&&v| !remove[v as usize]));
        if corners.len() >= 3 {
            out.add_face(&corners);
        }
    }
    Some(out)
}

fn pair_triangles(mesh: PolyMesh) -> PolyMesh {
    let edges = Edges::of_poly(&mesh);
    let (face_a, face_b) = edges.faces(&mesh);
    let p = &mesh.vertices;

    let mut candidates: Vec<(f32, u32)> = Vec::new();
    for e in 0..edges.count() {
        if edges.uses[e] != 2 || face_b[e] == NONE {
            continue;
        }
        let (fa, fb) = (face_a[e] as usize, face_b[e] as usize);
        if mesh.face_size(fa) != 3 || mesh.face_size(fb) != 3 {
            continue;
        }
        let Some(quad) = merged_quad(&mesh, fa, fb, edges.a[e], edges.b[e]) else { continue };
        let worst = worst_corner_deg(p, &quad);
        if worst < MAX_MERGED_CORNER_DEG {
            candidates.push(((worst - 90.0).abs(), e as u32));
        }
    }
    if candidates.is_empty() {
        return mesh;
    }
    candidates.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));

    let mut merged: Vec<Option<[u32; 4]>> = vec![None; mesh.face_count()];
    let mut consumed = vec![false; mesh.face_count()];
    for &(_, e) in &candidates {
        let (fa, fb) = (face_a[e as usize] as usize, face_b[e as usize] as usize);
        if consumed[fa] || consumed[fb] {
            continue;
        }
        consumed[fa] = true;
        consumed[fb] = true;
        merged[fa] = merged_quad(&mesh, fa, fb, edges.a[e as usize], edges.b[e as usize]);
    }

    let mut out = mesh.rebuild();
    for f in 0..mesh.face_count() {
        if let Some(quad) = merged[f] {
            out.add_face(&quad);
        } else if !consumed[f] {
            out.add_face(mesh.face(f));
        }
    }
    out
}

/// The quad formed by triangles fa and fb across their shared edge (u, w),
/// keeping the winding. None if they do not run the edge in opposite
/// directions.
fn merged_quad(mesh: &PolyMesh, fa: usize, fb: usize, u: u32, w: u32) -> Option<[u32; 4]> {
    let (a, b) = (mesh.face(fa), mesh.face(fb));
    // Rotate fa so its shared side is corner 0 -> corner 1.
    let s = (0..3).find(|&k| {
        let (x, y) = (a[k], a[(k + 1) % 3]);
        (x == u && y == w) || (x == w && y == u)
    })?;
    let (x0, x1, x2) = (a[s], a[(s + 1) % 3], a[(s + 2) % 3]);
    // fb must contain x1 -> x0.
    let t = (0..3).find(|&k| b[k] == x1 && b[(k + 1) % 3] == x0)?;
    let y2 = b[(t + 2) % 3];
    (y2 != x2).then_some([x1, x2, x0, y2])
}

fn worst_corner_deg(p: &[Vec3], quad: &[u32; 4]) -> f32 {
    let mut worst = 0.0f32;
    for k in 0..4 {
        let c = p[quad[k] as usize];
        let (a, b) = (p[quad[(k + 3) & 3] as usize] - c, p[quad[(k + 1) & 3] as usize] - c);
        let (la, lb) = (a.length(), b.length());
        if la <= 0.0 || lb <= 0.0 {
            return 180.0;
        }
        worst = worst.max((a.dot(b) / (la * lb)).clamp(-1.0, 1.0).acos().to_degrees());
    }
    worst
}

/// A lone triangle in a quad mesh is where a strip of quads runs out: the
/// strip's two sides meet at the triangle's far corner, which ends up with
/// five edges. Such strips often run out again a few quads later, at a
/// second triangle. Collapsing the strip (every rung of the ladder, from the
/// first triangle's side to the second's, shrunk to its midpoint) deletes
/// both triangles and the quads between, and leaves plain quad lattice.
///
/// Each triangle tries all three of its sides and keeps the shortest strip
/// that reaches another triangle; strips are then taken shortest first, and
/// never two that share a vertex in one pass (the caller repeats). A strip is
/// skipped if collapsing it would fold some other face onto itself.
fn collapse_triangle_chords(mesh: &PolyMesh) -> Option<PolyMesh> {
    let edges = Edges::of_poly(mesh);
    let (face_a, face_b) = edges.faces(mesh);
    let (face_count, n) = (mesh.face_count(), mesh.vertex_count());
    let side_edge = &edges.side_edge;
    let start = |f: usize| mesh.face_start[f] as usize;

    // Walks the strip leaving a triangle through one of its edges. On
    // success, `rungs` holds the edges to collapse and `strip` the faces that
    // vanish (both triangles included).
    let walk = |tri: usize, edge: u32, rungs: &mut Vec<u32>, strip: &mut Vec<u32>| -> bool {
        rungs.clear();
        strip.clear();
        strip.push(tri as u32);
        let (mut f, mut e) = (tri as u32, edge);
        loop {
            if edges.uses[e as usize] != 2 {
                return false;
            }
            rungs.push(e);
            let g = if face_a[e as usize] == f { face_b[e as usize] } else { face_a[e as usize] };
            if g == NONE || strip.contains(&g) {
                return false;
            }
            strip.push(g);
            let size = mesh.face_size(g as usize);
            if size == 3 {
                return true;
            }
            if size != 4 || strip.len() - 1 > MAX_CHORD_LENGTH {
                return false;
            }
            let s = start(g as usize);
            let Some(k) = (0..4).find(|&k| side_edge[s + k] == e) else { return false };
            e = side_edge[s + ((k + 2) & 3)];
            if e == NONE {
                return false;
            }
            f = g;
        }
    };

    let (mut rungs, mut strip) = (Vec::new(), Vec::new());
    let mut candidates: Vec<(usize, u32, u32)> = Vec::new();
    for f in 0..face_count {
        if mesh.face_size(f) != 3 {
            continue;
        }
        let mut best: Option<(usize, u32)> = None;
        for k in 0..3 {
            let e = side_edge[start(f) + k];
            if e == NONE || !walk(f, e, &mut rungs, &mut strip) {
                continue;
            }
            if best.is_none_or(|(length, _)| strip.len() < length) {
                best = Some((strip.len(), k as u32));
            }
        }
        if let Some((length, side)) = best {
            candidates.push((length, f as u32, side));
        }
    }
    if candidates.is_empty() {
        return None;
    }
    candidates.sort_unstable();

    // Choose strips that share no vertex. chord_of[v]: the strip a vertex
    // belongs to; merge_to[v]: for a rung's two ends, the end that survives.
    let mut chord_of = vec![NONE; n];
    let mut merge_to: Vec<u32> = (0..n as u32).collect();
    let mut face_chord = vec![NONE; face_count];
    let mut chords: Vec<(Vec<u32>, Vec<u32>)> = Vec::new();
    for &(_, tri, side) in &candidates {
        if !walk(tri as usize, side_edge[start(tri as usize) + side as usize], &mut rungs, &mut strip) {
            continue;
        }
        let id = chords.len() as u32;
        let free = strip.iter().all(|&f| mesh.face(f as usize).iter().all(|&v| chord_of[v as usize] == NONE));
        if !free {
            continue;
        }
        // A strip that touches itself would name a vertex on two rungs.
        let mut simple = true;
        for &e in &rungs {
            let (a, b) = (edges.a[e as usize] as usize, edges.b[e as usize] as usize);
            if chord_of[a] == id || chord_of[b] == id {
                simple = false;
                break;
            }
            chord_of[a] = id;
            chord_of[b] = id;
        }
        if !simple {
            for &e in &rungs {
                for v in [edges.a[e as usize] as usize, edges.b[e as usize] as usize] {
                    if chord_of[v] == id {
                        chord_of[v] = NONE;
                    }
                }
            }
            continue;
        }
        for &e in &rungs {
            merge_to[edges.b[e as usize] as usize] = edges.a[e as usize];
        }
        for &f in &strip {
            face_chord[f as usize] = id;
            for &v in mesh.face(f as usize) {
                chord_of[v as usize] = id;
            }
        }
        chords.push((rungs.clone(), strip.clone()));
    }
    if chords.is_empty() {
        return None;
    }

    // A face outside the strip that holds both ends of a rung would lose a
    // corner.
    let mut rejected = vec![false; chords.len()];
    for f in 0..face_count {
        if face_chord[f] != NONE {
            continue;
        }
        let face = mesh.face(f);
        for i in 0..face.len() {
            for j in i + 1..face.len() {
                let id = chord_of[face[i] as usize];
                if id != NONE && merge_to[face[i] as usize] == merge_to[face[j] as usize] {
                    rejected[id as usize] = true;
                }
            }
        }
    }

    let mut out = mesh.rebuild();
    let mut applied = 0;
    for (id, (rungs, strip)) in chords.iter().enumerate() {
        if rejected[id] {
            for &e in rungs {
                merge_to[edges.b[e as usize] as usize] = edges.b[e as usize];
            }
            for &f in strip {
                face_chord[f as usize] = NONE;
            }
            continue;
        }
        for &e in rungs {
            let (a, b) = (edges.a[e as usize] as usize, edges.b[e as usize] as usize);
            out.vertices[a] = (out.vertices[a] + out.vertices[b]) * 0.5;
        }
        applied += 1;
    }
    if applied == 0 {
        return None;
    }
    let mut corners: Vec<u32> = Vec::new();
    for f in 0..face_count {
        if face_chord[f] == NONE {
            corners.clear();
            corners.extend(mesh.face(f).iter().map(|&v| merge_to[v as usize]));
            out.add_face(&corners);
        }
    }
    Some(out)
}

/// Drops small stray pieces the extraction split off. Where a feature is
/// thinner than a quad (an ear, a finger tip), a few faces can close up into
/// a separate little polyhedron. Each output piece is matched to the input
/// piece it lies on (`input_piece_of`, from a vertex position); per input
/// piece the largest output piece is kept, and any other under
/// FRAGMENT_MAX_FACES is dropped. Big extra pieces stay (losing real surface
/// would be worse than an unwanted split) and a genuinely separate small part
/// (an eye) is its own input piece, so it is always kept.
pub fn remove_fragments(mesh: PolyMesh, input_piece_of: impl Fn(Vec3) -> u32) -> PolyMesh {
    let n = mesh.vertex_count();
    let mut parent: Vec<u32> = (0..n as u32).collect();
    for face in mesh.faces() {
        for &v in &face[1..] {
            let (a, b) = (find(&mut parent, face[0]), find(&mut parent, v));
            if a != b {
                parent[a as usize] = b;
            }
        }
    }
    let mut faces = vec![0u32; n];
    for f in 0..mesh.face_count() {
        faces[find(&mut parent, mesh.face(f)[0]) as usize] += 1;
    }

    // Largest output piece per input piece.
    let mut piece_of: Vec<(u32, u32)> = Vec::new();
    let mut largest: HashMap<u32, u32> = HashMap::new();
    for r in 0..n as u32 {
        if faces[r as usize] == 0 {
            continue;
        }
        let input = input_piece_of(mesh.vertices[r as usize]);
        piece_of.push((r, input));
        let best = largest.entry(input).or_insert(r);
        if faces[r as usize] > faces[*best as usize] {
            *best = r;
        }
    }
    let mut drop = vec![false; n];
    for &(r, input) in &piece_of {
        drop[r as usize] = largest[&input] != r && faces[r as usize] < FRAGMENT_MAX_FACES;
    }
    if !drop.contains(&true) {
        return mesh;
    }
    let mut out = mesh.rebuild();
    for f in 0..mesh.face_count() {
        if !drop[find(&mut parent, mesh.face(f)[0]) as usize] {
            out.add_face(mesh.face(f));
        }
    }
    out.remove_unused_vertices()
}
