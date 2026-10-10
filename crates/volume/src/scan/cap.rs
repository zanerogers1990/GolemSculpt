//! Caps for the holes of a mesh, so that it can be scan-converted as the
//! solid its maker meant.
//!
//! An open surface encloses nothing, so strictly a mesh with a hole has no
//! inside. What is wanted is plain enough all the same: the hole closed over
//! as flatly as its rim allows, whether it is three missing triangles in a
//! scan or the whole open base of a bust. Voting between the lattice axes
//! (`scan/vote.rs`) does that for a small hole, but for a large opening it
//! depends on which way the opening faces: a bowl opening straight along an
//! axis fills to the brim, the same bowl turned 45 degrees fills a third of
//! the way and leaves scraps. A cap does not care which way it faces.
//!
//! So the rim of every hole is found (the edges only one triangle uses,
//! chained into loops) and a fan of triangles is stretched from each rim to
//! its middle. The fan need not be a tidy triangulation: inside is a winding
//! number, and the folds of a fan over a rim that is not convex cancel out
//! there. Where a mesh's triangles do not share vertices, edges are matched
//! by where their ends are rather than by vertex number, so its seams are
//! not taken for holes.

use std::sync::atomic::{AtomicU64, Ordering};

use glam::Vec3;
use rayon::prelude::*;

use crate::parallel;

/// For every vertex, the number of the first vertex at exactly its position.
fn welded(positions: &[Vec3]) -> Vec<u32> {
    let bits = |p: Vec3| [(p.x + 0.0).to_bits(), (p.y + 0.0).to_bits(), (p.z + 0.0).to_bits()];
    let mut order: Vec<([u32; 3], u32)> = positions.par_iter().enumerate().map(|(i, &p)| (bits(p), i as u32)).collect();
    order.par_sort_unstable();
    let mut first = vec![0u32; positions.len()];
    let mut leader = 0;
    for (at, &(place, vertex)) in order.iter().enumerate() {
        if place != order[leader].0 {
            leader = at;
        }
        first[vertex as usize] = order[leader].1;
    }
    first
}

/// The edges only one triangle uses, each as that triangle runs along it and
/// sorted by where it starts. `name` gives the number a vertex goes by.
///
/// Sorting every edge of the mesh to find the few with no partner would cost
/// more than scanning it. Instead the vertices on a rim are found first:
/// round a vertex of a closed surface, the neighbours its triangles go on to
/// are exactly the neighbours its triangles arrive from, so adding up the one
/// and taking away the other leaves nothing; at a vertex on a rim it leaves
/// the two rim neighbours. Only edges between two such vertices are sorted.
fn rim_edges(indices: &[u32], vertices: usize, name: &(impl Fn(u32) -> u32 + Sync)) -> Vec<(u32, u32)> {
    let named = |t: &[u32]| {
        let v = [name(t[0]), name(t[1]), name(t[2])];
        // A triangle folded onto a point or a line has no edges to speak of.
        (v[0] != v[1] && v[1] != v[2] && v[2] != v[0]).then_some(v)
    };
    // The sum, and the sum of squares so that two neighbours cannot cancel
    // two others.
    let balance: Vec<[AtomicU64; 2]> = (0..vertices).map(|_| Default::default()).collect();
    indices.par_chunks_exact(3).filter_map(named).for_each(|v| {
        for c in 0..3 {
            let (next, last) = (v[(c + 1) % 3] as u64, v[(c + 2) % 3] as u64);
            let at = &balance[v[c] as usize];
            at[0].fetch_add(next.wrapping_sub(last), Ordering::Relaxed);
            at[1].fetch_add((next * next).wrapping_sub(last * last), Ordering::Relaxed);
        }
    });
    let on_rim = |v: u32| balance[v as usize].iter().any(|sum| sum.load(Ordering::Relaxed) != 0);

    // Both ends and the direction in one sortable number.
    let pack = |a: u32, b: u32| (a.min(b) as u64) << 32 | (a.max(b) as u64) << 1 | (a < b) as u64;
    let parts: Vec<Vec<u64>> = indices
        .par_chunks_exact(3)
        .filter_map(named)
        .fold(Vec::new, |mut part, v| {
            for c in 0..3 {
                let (from, to) = (v[c], v[(c + 1) % 3]);
                if on_rim(from) && on_rim(to) {
                    part.push(pack(from, to));
                }
            }
            part
        })
        .collect();
    let mut edges = parallel::concat(parts);
    edges.par_sort_unstable();
    let mut rim = Vec::new();
    let mut at = 0;
    while at < edges.len() {
        let same = edges[at..].iter().take_while(|&&e| e >> 1 == edges[at] >> 1).count();
        if same == 1 {
            let (low, high) = ((edges[at] >> 32) as u32, (edges[at] >> 1) as u32 & 0x7FFF_FFFF);
            rim.push(if edges[at] & 1 != 0 { (low, high) } else { (high, low) });
        }
        at += same;
    }
    rim.sort_unstable();
    rim
}

/// Chains rim edges into closed loops of vertices. Where a vertex has more
/// than one way on (two holes touching at a point) any is taken; an end that
/// leads nowhere is dropped.
fn loops(rim: &[(u32, u32)]) -> Vec<Vec<u32>> {
    let mut used = vec![false; rim.len()];
    let next_from = |vertex: u32, used: &mut [bool]| {
        let first = rim.partition_point(|&(from, _)| from < vertex);
        let free = (first..rim.len()).take_while(|&e| rim[e].0 == vertex).find(|&e| !used[e])?;
        used[free] = true;
        Some(rim[free].1)
    };
    let mut out = Vec::new();
    for start in 0..rim.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let mut chain = vec![rim[start].0];
        let mut at = rim[start].1;
        while at != chain[0] {
            chain.push(at);
            match next_from(at, &mut used) {
                Some(next) => at = next,
                None => break,
            }
        }
        if at == chain[0] && chain.len() >= 3 {
            out.push(chain);
        }
    }
    out
}

/// The mesh with a fan of triangles over every hole, or `None` when it has no
/// hole with a rim to stretch one over. The fans face the way the surface
/// round them does.
pub(super) fn capped(positions: &[Vec3], indices: &[u32]) -> Option<(Vec<Vec3>, Vec<u32>)> {
    if positions.len() >= 1 << 31 {
        return None;
    }
    // By vertex number first, which is all a mesh with shared vertices needs.
    // One whose triangles each have corners of their own has a "rim" round
    // every triangle: far more of them than any real mesh has, and the sign
    // that vertices must be matched by position instead.
    let mut rim = rim_edges(indices, positions.len(), &|v| v);
    if rim.len() > indices.len() / 48 {
        let welded = welded(positions);
        rim = rim_edges(indices, positions.len(), &|v| welded[v as usize]);
    }
    let loops = loops(&rim);
    if loops.is_empty() {
        return None;
    }
    let mut positions = positions.to_vec();
    let mut indices = indices.to_vec();
    for rim in &loops {
        let middle = rim.iter().map(|&v| positions[v as usize].as_dvec3()).sum::<glam::DVec3>() / rim.len() as f64;
        let hub = positions.len() as u32;
        positions.push(middle.as_vec3());
        for (i, &from) in rim.iter().enumerate() {
            // Against the rim edge, as the triangle the other side of a
            // shared edge always runs.
            indices.extend_from_slice(&[rim[(i + 1) % rim.len()], from, hub]);
        }
    }
    Some((positions, indices))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{assert_closed, bowl, box_mesh, icosphere, without_near};
    use sculpt_geom::TriMesh;

    #[test]
    fn a_closed_mesh_has_no_rim() {
        for mesh in [icosphere(2, 1.0, Vec3::ZERO), box_mesh(Vec3::ZERO, Vec3::ONE)] {
            assert!(capped(&mesh.positions, &mesh.indices).is_none());
        }
        assert!(capped(&[], &[]).is_none());
    }

    #[test]
    fn a_bowl_gets_a_lid_and_a_sphere_its_missing_patches() {
        let open = bowl(16, 32, 1.0);
        let (positions, indices) = capped(&open.positions, &open.indices).unwrap();
        assert_eq!((positions.len(), indices.len()), (open.positions.len() + 1, open.indices.len() + 32 * 3));
        let closed = TriMesh::new(positions, indices);
        assert_closed(&closed, "lidded bowl");
        // The lid is flat, on the rim.
        assert!(closed.positions.last().unwrap().length() < 1e-5);

        let sphere = icosphere(3, 1.0, Vec3::ZERO);
        let holed = without_near(&without_near(&sphere, Vec3::X, 0.2), Vec3::new(0.0, -0.6, 0.8), 0.25);
        let (positions, indices) = capped(&holed.positions, &holed.indices).unwrap();
        assert_eq!(positions.len(), sphere.positions.len() + 2, "a fan for each of two holes");
        let closed = TriMesh::new(positions, indices);
        assert_closed(&closed, "patched sphere");
        assert!((closed.signed_volume() / sphere.signed_volume() - 1.0).abs() < 0.02);
    }

    #[test]
    fn seams_between_unshared_vertices_are_not_holes() {
        // Every triangle with corners of its own, as an STL file has them.
        let sphere = without_near(&icosphere(2, 1.0, Vec3::ZERO), Vec3::Y, 0.3);
        let soup: Vec<Vec3> = sphere.indices.iter().map(|&i| sphere.positions[i as usize]).collect();
        let loose: Vec<u32> = (0..soup.len() as u32).collect();
        let (positions, indices) = capped(&soup, &loose).unwrap();
        assert_eq!(positions.len(), soup.len() + 1, "one hole, one fan");
        let fan = (indices.len() - loose.len()) / 3;
        assert!((5..12).contains(&fan), "{fan} triangles in the fan");
    }

    #[test]
    fn an_open_sheet_gets_a_cap_that_cancels_it() {
        // Not a hole in anything: the cap is the sheet again, facing the
        // other way, and the two enclose nothing.
        let sheet = [Vec3::ZERO, Vec3::X, Vec3::X + Vec3::Y, Vec3::Y];
        let (positions, indices) = capped(&sheet, &[0, 1, 2, 0, 2, 3]).unwrap();
        let closed = TriMesh::new(positions, indices);
        assert!(closed.signed_volume().abs() < 1e-9);
        assert_eq!(closed.triangle_count(), 6);
    }

    #[test]
    fn two_holes_meeting_at_a_point_are_both_closed() {
        // Two fans of a hexagon removed, touching at its middle.
        let mut positions = vec![Vec3::ZERO];
        positions.extend((0..6).map(|s| {
            let a = s as f32 * std::f32::consts::FRAC_PI_3;
            Vec3::new(a.cos(), a.sin(), 0.0)
        }));
        positions.push(Vec3::new(0.0, 0.0, -1.0));
        // The underside closes it; the top keeps four of its six wedges.
        let mut indices = Vec::new();
        for s in 0..6u32 {
            let (a, b) = (1 + s, 1 + (s + 1) % 6);
            indices.extend_from_slice(&[b, a, 7]);
            if s != 0 && s != 3 {
                indices.extend_from_slice(&[0, a, b]);
            }
        }
        let (positions, indices) = capped(&positions, &indices).unwrap();
        assert_closed(&TriMesh::new(positions, indices), "pinched hexagon");
    }
}
