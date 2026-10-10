//! Vertices bucketed by position, for "which vertices are near this point"
//! without scanning the whole mesh.
//!
//! A snapshot: it copies the positions it is built over, so nothing that moves
//! vertices afterwards can put it out of date, and it can be read from any
//! number of threads at once. That is what the symmetry map and the weld need.
//! (The Unity `VertexSpatialGrid` also followed vertices as a stroke moved
//! them; that job belongs to the sculpting core's own index, not to this one.)
//!
//! Layout is three flat arrays rather than a map of small lists. Every vertex
//! falls in a cubic cell; a cell's bucket is `hash(y, z) + x`, so a run of
//! cells along X is a run of neighbouring buckets, and the vertices of
//! neighbouring buckets sit next to each other in memory. A query for a 3x3x3
//! block of cells is then nine short contiguous reads instead of twenty-seven
//! separate lookups, which is most of what a lookup costs on a mesh too big for
//! the cache.

use glam::{IVec3, Vec3};
use rayon::prelude::*;

pub struct VertexGrid {
    cell_size: f32,
    inv_cell: f32,
    /// Bucket count minus one; the count is a power of two.
    mask: u32,
    /// Bucket `b` owns slots `start[b]..start[b + 1]`.
    start: Vec<u32>,
    /// The vertex in each slot, ascending within a bucket.
    index: Vec<u32>,
    /// That vertex's position, so a scan never leaves these arrays.
    point: Vec<Vec3>,
}

impl VertexGrid {
    /// Buckets `points` into cells `cell_size` across. Any size gives correct
    /// answers; one near the radius most queries will use gives fast ones.
    pub fn build(points: &[Vec3], cell_size: f32) -> VertexGrid {
        assert!(points.len() < u32::MAX as usize, "more vertices than a u32 index can name");
        let cell_size = if cell_size > 1e-12 { cell_size } else { 1e-12 };
        let inv_cell = 1.0 / cell_size;
        let table = points.len().max(16).next_power_of_two();
        let mask = (table - 1) as u32;

        // Bucket in the high half, vertex in the low: one sort groups the
        // vertices by bucket and leaves each bucket in vertex order, so the
        // layout (and every scan's order) is the same on every run.
        let mut keys: Vec<u64> = points
            .par_iter()
            .enumerate()
            .map(|(i, &p)| ((bucket_of(cell_of(p, inv_cell), mask) as u64) << 32) | i as u64)
            .collect();
        keys.par_sort_unstable();

        let mut start = vec![0u32; table + 1];
        for &key in &keys {
            start[(key >> 32) as usize + 1] += 1;
        }
        for b in 0..table {
            start[b + 1] += start[b];
        }
        let index: Vec<u32> = keys.par_iter().map(|&key| key as u32).collect();
        let point: Vec<Vec3> = index.par_iter().map(|&i| points[i as usize]).collect();
        VertexGrid { cell_size, inv_cell, mask, start, index, point }
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    pub fn cell_size(&self) -> f32 {
        self.cell_size
    }

    /// Calls `visit(vertex, position, distance squared)` once for every vertex
    /// within `radius` of `center` (inclusive), in no particular order.
    ///
    /// Exact, not approximate: a caller never has to test the distance again.
    pub fn for_each_within(&self, center: Vec3, radius: f32, mut visit: impl FnMut(u32, Vec3, f32)) {
        if self.index.is_empty() || radius.is_nan() || radius < 0.0 || !center.is_finite() {
            return;
        }
        let radius_sq = radius * radius;
        // A hair wider than asked, so rounding in `center - radius` cannot
        // leave out the cell of a vertex the distance test would accept.
        let reach = radius + (radius + center.abs().max_element()) * 1e-6;
        let lo = cell_of(center - Vec3::splat(reach), self.inv_cell);
        let hi = cell_of(center + Vec3::splat(reach), self.inv_cell);

        // A query far larger than the cells would walk millions of empty rows;
        // past the number of vertices there are, reading them all is cheaper,
        // and just as exact.
        let span = hi.x as i64 - lo.x as i64 + 1;
        let rows = (hi.y as i64 - lo.y as i64 + 1).saturating_mul(hi.z as i64 - lo.z as i64 + 1);
        if span > self.mask as i64 || rows.saturating_mul(span) > self.index.len() as i64 / 4 + 64 {
            for (slot, &p) in self.point.iter().enumerate() {
                let d = p.distance_squared(center);
                if d <= radius_sq {
                    visit(self.index[slot], p, d);
                }
            }
            return;
        }

        for z in lo.z..=hi.z {
            for y in lo.y..=hi.y {
                let row = row_hash(y, z);
                let first = (row.wrapping_add(lo.x as u32) & self.mask) as usize;
                let last = (row.wrapping_add(hi.x as u32) & self.mask) as usize;
                // Other cells hash into these buckets too. The distance test
                // throws nearly all of them out; the cell test after it is what
                // guarantees a vertex is reported from its own row only.
                let mut scan = |from: usize, to: usize| {
                    for slot in self.start[from] as usize..self.start[to] as usize {
                        let p = self.point[slot];
                        let d = p.distance_squared(center);
                        if d > radius_sq {
                            continue;
                        }
                        let cell = cell_of(p, self.inv_cell);
                        if cell.y == y && cell.z == z && cell.x >= lo.x && cell.x <= hi.x {
                            visit(self.index[slot], p, d);
                        }
                    }
                };
                if first <= last {
                    scan(first, last + 1);
                } else {
                    scan(first, self.mask as usize + 1);
                    scan(0, last + 1);
                }
            }
        }
    }

    /// The vertices within `radius` of `center`, written into `out` (which is
    /// cleared first, so one buffer can serve a whole loop of queries).
    pub fn query(&self, center: Vec3, radius: f32, out: &mut Vec<u32>) {
        out.clear();
        self.for_each_within(center, radius, |i, _, _| out.push(i));
    }

    /// The nearest vertex within `radius` of `center` and its squared
    /// distance. Equally near vertices go to the lowest index.
    pub fn nearest(&self, center: Vec3, radius: f32) -> Option<(u32, f32)> {
        self.nearest_where(center, radius, |_, _| true)
    }

    /// The nearest vertex within `radius` that `keep(vertex, position)`
    /// accepts.
    pub fn nearest_where(
        &self,
        center: Vec3,
        radius: f32,
        mut keep: impl FnMut(u32, Vec3) -> bool,
    ) -> Option<(u32, f32)> {
        let mut best: Option<(u32, f32)> = None;
        self.for_each_within(center, radius, |i, p, d| {
            let nearer = best.is_none_or(|(bi, bd)| d < bd || (d == bd && i < bi));
            if nearer && keep(i, p) {
                best = Some((i, d));
            }
        });
        best
    }
}

#[inline]
fn cell_of(p: Vec3, inv_cell: f32) -> IVec3 {
    (p * inv_cell).floor().as_ivec3()
}

/// Where the row of cells at (`y`, `z`) starts in the bucket table. Small,
/// dense cell coordinates are exactly what a surface produces, so every bit of
/// both has to reach the low bits the table keeps (the Unity `CellKeyComparer`
/// exists for the same reason).
#[inline]
fn row_hash(y: i32, z: i32) -> u32 {
    let mut h = (y as u32).wrapping_mul(0x9E37_79B1) ^ (z as u32).wrapping_mul(0x85EB_CA77);
    h ^= h >> 16;
    h = h.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 13;
    h = h.wrapping_mul(0xC2B2_AE35);
    h ^ (h >> 16)
}

#[inline]
fn bucket_of(cell: IVec3, mask: u32) -> u32 {
    row_hash(cell.y, cell.z).wrapping_add(cell.x as u32) & mask
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{Lcg, octa_sphere};

    fn brute(points: &[Vec3], center: Vec3, radius: f32) -> Vec<u32> {
        let r2 = radius * radius;
        (0..points.len() as u32).filter(|&i| points[i as usize].distance_squared(center) <= r2).collect()
    }

    fn cloud(count: usize, seed: u64) -> Vec<Vec3> {
        let mut rng = Lcg::new(seed);
        (0..count).map(|_| rng.in_cube() * 3.0).collect()
    }

    #[test]
    fn query_matches_a_scan_of_every_vertex() {
        let points = cloud(4000, 7);
        let mut rng = Lcg::new(99);
        let mut found = Vec::new();
        for cell in [0.02, 0.15, 0.6, 5.0] {
            let grid = VertexGrid::build(&points, cell);
            assert_eq!(grid.len(), points.len());
            for _ in 0..200 {
                let center = rng.in_cube() * 3.5;
                let radius = rng.unit() * 0.9;
                grid.query(center, radius, &mut found);
                found.sort_unstable();
                assert_eq!(found, brute(&points, center, radius), "cell {cell} radius {radius}");
            }
        }
    }

    #[test]
    fn a_query_much_larger_than_the_cells_is_still_exact() {
        let points = cloud(1500, 3);
        let grid = VertexGrid::build(&points, 0.001);
        let mut found = Vec::new();
        grid.query(Vec3::new(0.3, -0.2, 0.1), 2.5, &mut found);
        found.sort_unstable();
        assert_eq!(found, brute(&points, Vec3::new(0.3, -0.2, 0.1), 2.5));
        grid.query(Vec3::ZERO, 1e9, &mut found);
        assert_eq!(found.len(), points.len());
    }

    #[test]
    fn coincident_vertices_are_all_reported_once() {
        let mut points = cloud(300, 11);
        let twin = points[17];
        points.extend([twin, twin, twin]);
        let grid = VertexGrid::build(&points, 0.05);
        let mut found = Vec::new();
        grid.query(twin, 0.0, &mut found);
        found.sort_unstable();
        assert_eq!(found, vec![17, 300, 301, 302]);
        assert_eq!(grid.nearest(twin, 0.1), Some((17, 0.0)), "ties go to the lowest index");
    }

    #[test]
    fn nearest_agrees_with_a_scan_and_respects_the_filter() {
        let points = cloud(3000, 5);
        let grid = VertexGrid::build(&points, 0.2);
        let mut rng = Lcg::new(1);
        for _ in 0..300 {
            let center = rng.in_cube() * 3.0;
            let odd = |i: u32, _: Vec3| i % 2 == 1;
            let want = brute(&points, center, 0.5)
                .into_iter()
                .filter(|&i| i % 2 == 1)
                .map(|i| (i, points[i as usize].distance_squared(center)))
                .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
            assert_eq!(grid.nearest_where(center, 0.5, odd), want);
        }
    }

    #[test]
    fn empty_and_degenerate_inputs_are_harmless() {
        let grid = VertexGrid::build(&[], 0.1);
        assert!(grid.is_empty());
        assert_eq!(grid.nearest(Vec3::ZERO, 1.0), None);

        let grid = VertexGrid::build(&[Vec3::ONE, Vec3::NEG_ONE], 0.0);
        assert!(grid.cell_size() > 0.0);
        assert_eq!(grid.nearest(Vec3::ONE, 0.5), Some((0, 0.0)));
        assert_eq!(grid.nearest(Vec3::splat(f32::NAN), 0.5), None);
        assert_eq!(grid.nearest(Vec3::ONE, -1.0), None);
    }

    #[test]
    fn every_vertex_of_a_surface_finds_itself_and_its_neighbours() {
        let mesh = octa_sphere(24, 1.0);
        let grid = VertexGrid::build(&mesh.positions, 0.03);
        let mut found = Vec::new();
        for (i, &p) in mesh.positions.iter().enumerate() {
            grid.query(p, 0.12, &mut found);
            found.sort_unstable();
            assert_eq!(found, brute(&mesh.positions, p, 0.12));
            assert!(found.contains(&(i as u32)));
        }
    }

    /// Run with `--release -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn timing_build_and_query_4m() {
        let mesh = octa_sphere(1000, 1.0);
        let n = mesh.vertex_count();
        let spacing = 1.6 / 1000.0;
        let clock = std::time::Instant::now();
        let grid = VertexGrid::build(&mesh.positions, spacing);
        let built = clock.elapsed();

        let clock = std::time::Instant::now();
        let hits: usize = mesh
            .positions
            .par_iter()
            .map(|&p| {
                let mut count = 0;
                grid.for_each_within(p * Vec3::new(-1.0, 1.0, 1.0), spacing * 0.5, |_, _, _| count += 1);
                count
            })
            .sum();
        println!(
            "VertexGrid: {n} vertices, build {:.1} ms, {n} mirrored queries {:.1} ms ({hits} hits)",
            built.as_secs_f64() * 1e3,
            clock.elapsed().as_secs_f64() * 1e3
        );
        assert!(hits >= n);
    }
}
