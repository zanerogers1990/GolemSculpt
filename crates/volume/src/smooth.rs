//! What a remesh does when it is asked to come out smooth rather than as
//! sharp as its source allows: the Remesh card's Smooth.
//!
//! A remesh copies its source exactly (see [`crate::qef`]): every flat face
//! of the old mesh and every edge between two of them. That is right when
//! the old mesh is as fine as the new one or finer. Stepped up to a finer
//! resolution, though, the old triangles are bigger than the new cells, and
//! the old mesh's facets and the zigzag along its creases come back at the
//! finer size, as ripples. Separately, dual contouring leaves a stair-step of
//! its own, one cell high, along a groove that runs across the lattice.
//!
//! Two passes, both measured on a grooved, lumpy ball remeshed at 128 and
//! then stepped up to 300 (roughness: how far each vertex normal turns from
//! its neighbours', on average):
//!
//!   - [`refine`] before the remesh: one step of Loop subdivision, which
//!     rounds the old facets off and halves their size. Roughness 1.23
//!     degrees down to 0.68, with the shape no further from the true one.
//!     Taken only when the old triangles are bigger than the new cells
//!     ([`refine_steps`]); edges that turn sharply stay lines.
//!   - [`relax`] after it: Taubin smoothing, which irons out the cell-sized
//!     stair-steps without shrinking the model. Two passes took the stepped
//!     up ball to 0.55 degrees, as smooth as a remesh of the true shape
//!     (0.59), and a remesh of the true shape to 0.49.
//!
//! What was tried and did nothing visible: smooth normals read at the
//! crossings in place of the face normals, plain Surface Nets, and moving
//! the new vertices back onto the old surface, flat or Phong-curved. The
//! facets are in where the crossings are, not in their normals, and the
//! stair-step is in how the lattice joins them up.
//!
//! Both are deterministic: the same mesh in gives the same bits out,
//! whatever the threads do.

use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering::Relaxed;

use glam::Vec3;
use rayon::prelude::*;

/// An edge whose two faces turn further apart than this (60 degrees) is a
/// line [`refine`] keeps: a cut, the rim of a flat base, a crisp groove.
const CREASE_COSINE: f32 = 0.5;

/// Taubin's two steps: towards the neighbours' average, then back out a
/// little further, so that the model does not shrink.
const SHRINK: f32 = 0.5;
const INFLATE: f32 = -0.53;

/// No refined source bigger than this many triangles: past it, fewer steps.
pub const MAX_REFINED_TRIANGLES: usize = 40_000_000;

/// How many steps of [`refine`] a source of `triangles` with surface `area`
/// wants before a remesh into cells of `cell`: one where its triangles are
/// bigger than the new cells, two where they are much bigger, none where the
/// old mesh is about as fine as the new one or finer, which a remesh
/// already copies well.
///
/// The size of a triangle is taken as the side of an equilateral one of the
/// mean area. Area weighs the big triangles most, and they are the ones
/// whose facets show.
pub fn refine_steps(area: f64, triangles: usize, cell: f32) -> u32 {
    if triangles == 0 || !(area > 0.0 && cell > 0.0) {
        return 0;
    }
    let side = (area / triangles as f64 * 4.0 / 3f64.sqrt()).sqrt();
    let ratio = side / cell as f64;
    let mut steps = if ratio > 3.0 {
        2
    } else if ratio > 1.5 {
        1
    } else {
        0
    };
    while steps > 0 && triangles.saturating_mul(4usize.pow(steps)) > MAX_REFINED_TRIANGLES {
        steps -= 1;
    }
    steps
}

/// Triangles with all three corners different: the ones with any area. A
/// triangle dynamic topology has taken away is kept as one corner three
/// times over until the next rebuild.
pub fn live_triangles(indices: &[u32]) -> usize {
    indices.par_chunks_exact(3).filter(|t| live(t)).count()
}

#[inline]
fn live(t: &[u32]) -> bool {
    t[0] != t[1] && t[1] != t[2] && t[2] != t[0]
}

/// The two ends of the edge leaving corner `half` of its triangle.
#[inline]
fn ends(indices: &[u32], half: usize) -> (u32, u32) {
    let face = half / 3 * 3;
    (indices[half], indices[face + (half + 1) % 3])
}

/// The corner of `half`'s triangle that is not on its edge.
#[inline]
fn opposite(indices: &[u32], half: usize) -> u32 {
    indices[half / 3 * 3 + (half + 2) % 3]
}

#[inline]
fn edge_key((a, b): (u32, u32)) -> u64 {
    (a.min(b) as u64) << 32 | a.max(b) as u64
}

/// One step of Loop subdivision: every triangle cut into four, the new
/// points on each edge and the old corners moved to where the smooth surface
/// through them runs.
///
/// An edge two faces share is smoothed unless the faces turn more than 60
/// degrees apart; an edge with one face (a hole's rim), or more than two, or
/// whose faces disagree about which way is out, is kept as a line, its new
/// point halfway along it. A corner on two such lines slides along them
/// only; on three or more it stays where it is. The triangles keep their
/// winding; those with a corner twice over, which have no area, are left
/// out.
pub fn refine(positions: &[Vec3], indices: &[u32]) -> (Vec<Vec3>, Vec<u32>) {
    let whole = &indices[..indices.len() / 3 * 3];
    let kept: Vec<u32>;
    let indices = if whole.par_chunks_exact(3).all(live) {
        whole
    } else {
        kept = whole.par_chunks_exact(3).filter(|t| live(t)).flat_map_iter(|t| t.iter().copied()).collect();
        &kept
    };
    let corners = indices.len();
    let low = |h: usize| {
        let (a, b) = ends(indices, h);
        a.min(b)
    };
    let high = |h: u32| {
        let (a, b) = ends(indices, h as usize);
        a.max(b)
    };

    // Every edge once: the triangle corners leaving along it, gathered by the
    // lower-numbered end and then by the other, so that each edge's are a run.
    let (_, halves) = bucket(positions.len(), corners, |h| (low(h), h as u32), |&x, &y| high(x).cmp(&high(y)).then(x.cmp(&y)));
    let half_key = |h: u32| edge_key(ends(indices, h as usize));
    let mut starts: Vec<u32> = (0..halves.len())
        .into_par_iter()
        .filter(|&i| i == 0 || half_key(halves[i]) != half_key(halves[i - 1]))
        .map(|i| i as u32)
        .collect();
    let edge_count = starts.len();
    starts.push(halves.len() as u32);
    let run = |e: usize| &halves[starts[e] as usize..starts[e + 1] as usize];
    let edge_ends = |e: usize| ends(indices, run(e)[0] as usize);
    let edge_of: Vec<AtomicU32> = (0..corners).map(|_| AtomicU32::new(0)).collect();
    (0..edge_count).into_par_iter().for_each(|e| {
        for &h in run(e) {
            edge_of[h as usize].store(e as u32, Relaxed);
        }
    });
    let edge_of: Vec<u32> = edge_of.into_iter().map(AtomicU32::into_inner).collect();

    let normals: Vec<Vec3> = indices
        .par_chunks_exact(3)
        .map(|t| crate::scan::unit_normal(positions[t[0] as usize], positions[t[1] as usize], positions[t[2] as usize]))
        .collect();
    let point = |v: u32| positions[v as usize];

    // The new point on each edge, and whether the edge is a line to keep.
    let (edge_points, lines): (Vec<Vec3>, Vec<bool>) = (0..edge_count)
        .into_par_iter()
        .map(|e| {
            let sides = run(e);
            let (a, b) = edge_ends(e);
            let smooth = match sides {
                &[x, y] => {
                    let (x, y) = (x as usize, y as usize);
                    // Faces wound the same way run the shared edge in opposite directions.
                    ends(indices, x).0 == ends(indices, y).1 && normals[x / 3].dot(normals[y / 3]) > CREASE_COSINE
                }
                _ => false,
            };
            if smooth {
                let (c, d) = (opposite(indices, sides[0] as usize), opposite(indices, sides[1] as usize));
                ((point(a) + point(b)) * 0.375 + (point(c) + point(d)) * 0.125, false)
            } else {
                ((point(a) + point(b)) * 0.5, true)
            }
        })
        .unzip();

    // Each old corner's edges, to move it by.
    let (at, around) = bucket(
        positions.len(),
        edge_count * 2,
        |i| {
            let (a, b) = edge_ends(i / 2);
            (if i % 2 == 0 { a } else { b }, (i / 2) as u32)
        },
        u32::cmp,
    );

    let moved: Vec<Vec3> = (0..positions.len())
        .into_par_iter()
        .map(|v| {
            let mine = &around[at[v] as usize..at[v + 1] as usize];
            let here = positions[v];
            if mine.is_empty() {
                return here;
            }
            let (mut sum, mut line_sum, mut line_count) = (Vec3::ZERO, Vec3::ZERO, 0);
            for &e in mine {
                let (a, b) = edge_ends(e as usize);
                let other = point(if a as usize == v { b } else { a });
                sum += other;
                if lines[e as usize] {
                    line_sum += other;
                    line_count += 1;
                }
            }
            match line_count {
                // A smooth corner, or one where a single line ends.
                0 | 1 => {
                    let n = mine.len() as f32;
                    let beta = if mine.len() == 3 { 3.0 / 16.0 } else { 3.0 / (8.0 * n) };
                    here * (1.0 - n * beta) + sum * beta
                }
                2 => here * 0.75 + line_sum * 0.125,
                _ => here,
            }
        })
        .collect();

    let base = positions.len() as u32;
    let mut out_positions = moved;
    out_positions.extend_from_slice(&edge_points);
    let mut out_indices = vec![0u32; corners * 4];
    out_indices.par_chunks_exact_mut(12).zip(indices.par_chunks_exact(3)).enumerate().for_each(|(f, (out, t))| {
        let [ab, bc, ca] = [0, 1, 2].map(|e| base + edge_of[f * 3 + e]);
        let [a, b, c] = [t[0], t[1], t[2]];
        out.copy_from_slice(&[a, ab, ca, ab, b, bc, ca, bc, c, ab, bc, ca]);
    });
    (out_positions, out_indices)
}

/// `passes` rounds of Taubin smoothing over a closed mesh: each vertex drawn
/// towards the average of its neighbours, then pushed back out a little
/// further, which takes out wrinkles a few triangles across and leaves the
/// size and the larger forms as they were. Zero passes changes nothing.
pub fn relax(positions: &mut Vec<Vec3>, indices: &[u32], passes: u32) {
    if passes == 0 || positions.is_empty() {
        return;
    }
    let (offsets, neighbours) = neighbours(positions.len(), indices);
    let mut spare = positions.clone();
    for _ in 0..passes {
        for factor in [SHRINK, INFLATE] {
            let from: &[Vec3] = positions;
            spare.par_iter_mut().enumerate().for_each(|(v, out)| {
                let ring = &neighbours[offsets[v] as usize..offsets[v + 1] as usize];
                let here = from[v];
                *out = if ring.is_empty() {
                    here
                } else {
                    let mean = ring.iter().map(|&w| from[w as usize]).sum::<Vec3>() / ring.len() as f32;
                    here + (mean - here) * factor
                };
            });
            std::mem::swap(positions, &mut spare);
        }
    }
}

/// Each vertex's neighbours, once each, in order of number: offsets into the
/// list (one more than there are vertices) and the list.
///
/// A neighbour is the corner after the vertex in each of its triangles,
/// which on a closed surface wound one way is every neighbour exactly once.
fn neighbours(vertices: usize, indices: &[u32]) -> (Vec<u32>, Vec<u32>) {
    let next = |c: usize| indices[c / 3 * 3 + (c + 1) % 3];
    bucket(vertices, indices.len() / 3 * 3, |c| (indices[c], next(c)), u32::cmp)
}

/// Entries `0..count`, each a `(group, item)` as `entry` gives it, gathered by
/// group: offsets into the list (one more than there are groups) and the
/// items, each group's sorted by `order`.
///
/// A counting sort rather than one big comparison sort, which on the tens of
/// millions of entries a big remesh has took most of the smoothing's time:
/// counted and placed in parallel, then each group's few sorted, so the list
/// comes out the same on every run, and as a full sort would order it.
fn bucket(
    groups: usize,
    count: usize,
    entry: impl Fn(usize) -> (u32, u32) + Sync,
    order: impl Fn(&u32, &u32) -> std::cmp::Ordering + Sync,
) -> (Vec<u32>, Vec<u32>) {
    let counts: Vec<AtomicU32> = (0..groups).map(|_| AtomicU32::new(0)).collect();
    (0..count).into_par_iter().for_each(|i| {
        counts[entry(i).0 as usize].fetch_add(1, Relaxed);
    });
    let mut offsets = Vec::with_capacity(groups + 1);
    let mut total = 0u32;
    offsets.push(0);
    for count in counts {
        total += count.into_inner();
        offsets.push(total);
    }
    let cursor: Vec<AtomicU32> = offsets[..groups].iter().map(|&o| AtomicU32::new(o)).collect();
    let slots: Vec<AtomicU32> = (0..total).map(|_| AtomicU32::new(0)).collect();
    (0..count).into_par_iter().for_each(|i| {
        let (group, item) = entry(i);
        slots[cursor[group as usize].fetch_add(1, Relaxed) as usize].store(item, Relaxed);
    });
    let mut list: Vec<u32> = slots.into_iter().map(AtomicU32::into_inner).collect();

    // Sorted a block of groups at a time, each block its own slice.
    const BLOCK: usize = 4096;
    let mut blocks = Vec::with_capacity(groups / BLOCK + 1);
    let mut rest = &mut list[..];
    for first in (0..groups).step_by(BLOCK) {
        let last = (first + BLOCK).min(groups);
        let (block, tail) = rest.split_at_mut((offsets[last] - offsets[first]) as usize);
        blocks.push((first, last, block));
        rest = tail;
    }
    blocks.into_par_iter().for_each(|(first, last, block)| {
        let base = offsets[first];
        for g in first..last {
            block[(offsets[g] - base) as usize..(offsets[g + 1] - base) as usize].sort_unstable_by(&order);
        }
    });
    (offsets, list)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Rng, assert_closed, box_mesh, icosphere};
    use sculpt_geom::TriMesh;

    /// Furthest from the centre less nearest to it.
    fn out_of_round(positions: &[Vec3]) -> f32 {
        let lengths = positions.iter().map(|p| p.length());
        lengths.clone().fold(0.0, f32::max) - lengths.fold(f32::INFINITY, f32::min)
    }

    #[test]
    fn a_refined_sphere_is_closed_four_times_as_fine_and_rounder() {
        let sphere = icosphere(2, 1.0, Vec3::ZERO);
        let (positions, indices) = refine(&sphere.positions, &sphere.indices);
        let refined = TriMesh::new(positions, indices);
        assert_closed(&refined, "refined sphere");
        assert_eq!(refined.triangle_count(), sphere.triangle_count() * 4);
        assert_eq!(refined.vertex_count(), sphere.vertex_count() + sphere.triangle_count() * 3 / 2);
        // Loop pulls the surface in a little, and comes out far rounder than
        // the same triangles cut in four where they lie.
        let volume = refined.signed_volume() / sphere.signed_volume();
        assert!(volume > 0.97 && volume < 1.03, "volume ratio {volume}");
        let flat: Vec<Vec3> = sphere.indices.chunks_exact(3).flat_map(|t| {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| sphere.positions[i as usize]);
            [a, (a + b) * 0.5, (b + c) * 0.5, (c + a) * 0.5]
        }).collect();
        let (loop_round, flat_round) = (out_of_round(&refined.positions), out_of_round(&flat));
        assert!(loop_round < flat_round * 0.5, "{loop_round} vs {flat_round}");
    }

    #[test]
    fn a_box_keeps_its_edges_and_corners() {
        let cube = box_mesh(Vec3::splat(-0.5), Vec3::splat(0.5));
        let (positions, indices) = refine(&cube.positions, &cube.indices);
        let refined = TriMesh::new(positions, indices);
        assert_closed(&refined, "refined box");
        for corner in &cube.positions {
            assert!(refined.positions.contains(corner), "corner {corner} moved");
        }
        for p in &refined.positions {
            assert!((p.abs().max_element() - 0.5).abs() < 1e-6, "{p} is off the box");
        }
    }

    #[test]
    fn steps_follow_how_much_bigger_the_old_triangles_are() {
        // The surface of `n` equilateral triangles of side 1.
        let area = |n: usize| 3f64.sqrt() / 4.0 * n as f64;
        assert_eq!(refine_steps(area(1000), 1000, 1.0), 0, "as fine as the new cells");
        assert_eq!(refine_steps(area(1000), 1000, 0.5), 1, "twice as big");
        assert_eq!(refine_steps(area(1000), 1000, 0.25), 2, "four times as big");
        assert_eq!(refine_steps(area(30_000_000), 30_000_000, 0.25), 0, "no room to refine");
        assert_eq!(refine_steps(area(5_000_000), 5_000_000, 0.25), 1, "room for one step");
        assert_eq!(refine_steps(0.0, 1000, 1.0), 0);
    }

    #[test]
    fn relaxing_irons_out_noise_and_keeps_the_size() {
        let mut sphere = icosphere(4, 1.0, Vec3::ZERO);
        let clean_volume = sphere.signed_volume();
        // A wobble of up to 3% in or out on every vertex, each its own.
        let mut rng = Rng::new(7);
        for p in &mut sphere.positions {
            *p *= 1.0 + 0.03 * (2.0 * rng.next() - 1.0);
        }
        let mut positions = sphere.positions.clone();
        relax(&mut positions, &sphere.indices, 3);
        let off = |ps: &[Vec3]| (ps.iter().map(|p| (p.length() - 1.0).powi(2)).sum::<f32>() / ps.len() as f32).sqrt();
        let (noisy, relaxed) = (off(&sphere.positions), off(&positions));
        // Taubin passes the slower part of white noise; the rough half goes.
        assert!(relaxed < noisy * 0.6, "{relaxed} vs {noisy}");
        let volume = TriMesh::new(positions.clone(), sphere.indices.clone()).signed_volume() / clean_volume;
        assert!((volume - 1.0).abs() < 0.01, "volume ratio {volume}");
        // The same bits every time, and nothing at all for no passes.
        let mut again = sphere.positions.clone();
        relax(&mut again, &sphere.indices, 3);
        assert_eq!(again, positions);
        let mut untouched = sphere.positions.clone();
        relax(&mut untouched, &sphere.indices, 0);
        assert_eq!(untouched, sphere.positions);
    }

    #[test]
    fn faces_with_no_area_are_left_out() {
        let sphere = icosphere(2, 1.0, Vec3::ZERO);
        let mut with_gone = sphere.indices.clone();
        with_gone.extend_from_slice(&[5, 5, 5, 7, 7, 7]);
        assert_eq!(live_triangles(&with_gone), sphere.triangle_count());
        assert_eq!(refine(&sphere.positions, &with_gone), refine(&sphere.positions, &sphere.indices));
    }

    #[test]
    fn refining_is_the_same_bits_every_time() {
        let sphere = icosphere(3, 1.0, Vec3::new(0.1, 0.0, -0.2));
        assert_eq!(refine(&sphere.positions, &sphere.indices), refine(&sphere.positions, &sphere.indices));
    }
}
