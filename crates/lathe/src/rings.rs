//! The rings a profile is revolved into, and the triangles that join consecutive rings.
//!
//! Everything here is exactly mirror-symmetric across both the X and Z planes, positions and
//! topology alike, because this app's mirrored sculpting pairs vertices by position and smooths
//! them over their neighbours - a mesh whose two halves are triangulated differently drifts apart
//! under a mirrored Smooth.

use std::f32::consts::PI;

/// A ring of vertices in the output: where its first vertex is and how many it has. A count of 1
/// is a pole.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RingRef {
    pub start: u32,
    pub count: u32,
}

/// How many vertices a ring of `radius` gets: all `segments` of them, or with `even` a count
/// proportional to its radius (a multiple of 4, at least 4), so triangles stay close to one size
/// all over instead of squeezing into slivers near the poles.
pub(crate) fn ring_vertex_count(radius: f32, spacing: f32, segments: usize, even: bool) -> usize {
    if !even {
        return segments;
    }
    let n = ((2.0 * PI * radius / spacing / 4.0).round_ties_even() as usize) * 4;
    n.clamp(4, segments)
}

/// Unit circle at `count` evenly spaced angles from +X toward +Z, as (cos, sin), with `count` a
/// multiple of 4. Only the first quadrant is computed with trig; the other three are sign flips of
/// it, so a vertex and its reflection across x = 0 or z = 0 are exact negations of each other - bit
/// for bit, not merely within a float's rounding. The quarter points are exact too (cos 90 is 0,
/// not 6e-8), which puts those vertices exactly ON the mirror planes.
pub(crate) fn angle_table(count: usize) -> Vec<[f32; 2]> {
    let mut table = vec![[0.0f32; 2]; count];
    let (q, half) = (count / 4, count / 2);
    for (k, entry) in table.iter_mut().enumerate().take(q + 1) {
        let angle = 2.0 * std::f64::consts::PI * k as f64 / count as f64;
        *entry = [angle.cos() as f32, angle.sin() as f32];
    }
    table[0] = [1.0, 0.0];
    table[q] = [0.0, 1.0];
    for k in q + 1..=half {
        let [c, s] = table[half - k];
        table[k] = [-c, s];
    }
    for k in half + 1..count {
        let [c, s] = table[count - k];
        table[k] = [c, -s];
    }
    table
}

/// Triangles in the band between two rings: the other ring's count against a pole, nothing between
/// two poles, and the sum of the counts otherwise.
pub(crate) fn strip_triangles(a: u32, b: u32) -> usize {
    match (a, b) {
        (1, 1) => 0,
        (1, n) | (n, 1) => n as usize,
        (a, b) => (a + b) as usize,
    }
}

/// Writes the triangles of the band between rings `a` and `b` into `out`, which holds exactly
/// `strip_triangles(a.count, b.count) * 3` indices.
///
/// Rings of different counts are zipped by angle - each step advances whichever ring's next vertex
/// comes first. Only the first quadrant is zipped; the other three are its reflections across
/// x = 0 and z = 0 (and the half-turn that is both), which is what makes the triangulation - not
/// just the vertex positions - exactly mirror-symmetric. A plain zip all the way round would break
/// ties the same way on both sides of the mirror, which is the mirror image of the wrong way on one
/// of them.
pub(crate) fn write_strip(a: RingRef, b: RingRef, out: &mut [u32]) {
    match (a.count, b.count) {
        (1, 1) => {}
        (1, _) => {
            for (k, tri) in out.chunks_exact_mut(3).enumerate() {
                let k = k as u32;
                tri.copy_from_slice(&[a.start, b.start + k, b.start + (k + 1) % b.count]);
            }
        }
        (_, 1) => {
            for (j, tri) in out.chunks_exact_mut(3).enumerate() {
                let j = j as u32;
                tri.copy_from_slice(&[a.start + j, b.start, a.start + (j + 1) % a.count]);
            }
        }
        _ => zip_quadrants(a, b, out),
    }
}

fn zip_quadrants(a: RingRef, b: RingRef, out: &mut [u32]) {
    let (qa, qb) = (a.count / 4, b.count / 4);
    let (mut ja, mut kb) = (0u32, 0u32);
    let mut written = out.chunks_exact_mut(12);
    while ja < qa || kb < qb {
        let advance_a = if ja == qa {
            false
        } else if kb == qb {
            true
        } else {
            // Compare the next angles (ja+1)/a.count and (kb+1)/b.count without division.
            (ja as u64 + 1) * b.count as u64 <= (kb as u64 + 1) * a.count as u64
        };

        // The first-quadrant triangle as (ring, index) corners: ring 0 = A, 1 = B.
        let corners = if advance_a { [(a, ja), (b, kb), (a, ja + 1)] } else { [(a, ja), (b, kb), (b, kb + 1)] };
        let cell = written.next().expect("strip buffer holds four images of every zipped triangle");
        let at = |corner: usize, image: u32| image_vertex(corners[corner].0, corners[corner].1, image);

        // Four images of each first-quadrant triangle. A reflection reverses orientation, so its
        // corners are emitted in reverse to keep the face pointing out.
        cell.copy_from_slice(&[
            // Identity.
            at(0, 0),
            at(1, 0),
            at(2, 0),
            // Across x = 0: angle -> 180 - angle.
            at(0, 1),
            at(2, 1),
            at(1, 1),
            // Across z = 0: angle -> -angle.
            at(0, 2),
            at(2, 2),
            at(1, 2),
            // Both - a half turn, which keeps orientation.
            at(0, 3),
            at(1, 3),
            at(2, 3),
        ]);

        if advance_a {
            ja += 1;
        } else {
            kb += 1;
        }
    }
}

/// Vertex `index` of a ring (an index in the first quadrant, 0..=count/4) as seen in one of the four
/// images: 0 itself, 1 across x = 0, 2 across z = 0, 3 both.
fn image_vertex(ring: RingRef, index: u32, image: u32) -> u32 {
    let count = ring.count;
    let mapped = match image {
        1 => count / 2 - index,
        2 => (count - index) % count,
        3 => (index + count / 2) % count,
        _ => index % count,
    };
    ring.start + mapped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn angle_tables_are_exactly_symmetric_with_exact_quarter_points() {
        for count in [4usize, 8, 12, 64, 100, 256] {
            let t = angle_table(count);
            assert_eq!(t[0], [1.0, 0.0]);
            assert_eq!(t[count / 4], [0.0, 1.0]);
            assert_eq!(t[count / 2], [-1.0, 0.0]);
            assert_eq!(t[3 * count / 4], [0.0, -1.0]);
            for k in 1..count {
                // Across z = 0 (angle -> -angle) and across x = 0 (angle -> 180 - angle).
                assert_eq!(t[count - k], [t[k][0], -t[k][1]]);
                assert_eq!(t[(count / 2 + count - k) % count], [-t[k][0], t[k][1]]);
            }
            let k = count / 8;
            let angle = 2.0 * std::f64::consts::PI * k as f64 / count as f64;
            assert!((t[k][0] as f64 - angle.cos()).abs() < 1e-7 && (t[k][1] as f64 - angle.sin()).abs() < 1e-7);
        }
    }

    #[test]
    fn even_rings_scale_with_radius_and_never_exceed_the_segments() {
        let spacing = 2.0 * PI / 64.0;
        assert_eq!(ring_vertex_count(1.0, spacing, 64, true), 64);
        assert_eq!(ring_vertex_count(0.5, spacing, 64, true), 32);
        assert_eq!(ring_vertex_count(0.001, spacing, 64, true), 4);
        assert_eq!(ring_vertex_count(5.0, spacing, 64, true), 64);
        assert_eq!(ring_vertex_count(0.1, spacing, 64, false), 64);
    }

    #[test]
    fn strip_counts_follow_the_zip_rule() {
        assert_eq!(strip_triangles(1, 1), 0);
        assert_eq!(strip_triangles(1, 24), 24);
        assert_eq!(strip_triangles(24, 1), 24);
        assert_eq!(strip_triangles(8, 24), 32);
    }

    #[test]
    fn a_zipped_band_writes_exactly_the_triangles_it_promises() {
        for (ca, cb) in [(8u32, 8u32), (8, 32), (36, 12), (4, 64)] {
            let (a, b) = (RingRef { start: 0, count: ca }, RingRef { start: ca, count: cb });
            let mut out = vec![u32::MAX; strip_triangles(ca, cb) * 3];
            write_strip(a, b, &mut out);
            assert!(out.iter().all(|&i| i < ca + cb), "every slot written with a real vertex");
        }
    }
}
