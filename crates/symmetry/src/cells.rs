//! A set of coarse grid cells: "anywhere near these points", as something that
//! can be asked about millions of other points from many threads.
//!
//! Used to cut a whole model down to the part a local repair can reach before
//! anything expensive is built over it.

use glam::Vec3;
use rayon::prelude::*;

pub(crate) struct CellSet {
    inv_size: f32,
    /// Packed cells, ascending and without repeats.
    keys: Vec<u64>,
}

impl CellSet {
    /// Every cell of side `size` that holds one of `points`, plus the ring of
    /// cells around each: so anything within `size` of a point is in the set.
    pub(crate) fn around(points: impl ParallelIterator<Item = Vec3>, size: f32) -> CellSet {
        let inv_size = 1.0 / size.max(1e-12);
        let mut seeds: Vec<[i32; 3]> = points.map(|p| cell_of(p, inv_size)).collect();
        seeds.par_sort_unstable();
        seeds.dedup();

        let mut keys: Vec<u64> = seeds
            .par_iter()
            .flat_map_iter(|&[x, y, z]| {
                (0..27).map(move |k| pack([x + k % 3 - 1, y + k / 3 % 3 - 1, z + k / 9 - 1]))
            })
            .collect();
        keys.par_sort_unstable();
        keys.dedup();
        CellSet { inv_size, keys }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    #[inline]
    pub(crate) fn contains(&self, p: Vec3) -> bool {
        self.keys.binary_search(&pack(cell_of(p, self.inv_size))).is_ok()
    }
}

#[inline]
fn cell_of(p: Vec3, inv_size: f32) -> [i32; 3] {
    (p * inv_size).floor().as_ivec3().to_array()
}

/// 21 bits per axis. Cells further apart than two million of them can share a
/// key, which only ever makes the set a little more generous than it had to be.
#[inline]
fn pack([x, y, z]: [i32; 3]) -> u64 {
    const BITS: u64 = 0x1F_FFFF;
    ((x as u64 & BITS) << 42) | ((y as u64 & BITS) << 21) | (z as u64 & BITS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holds_the_points_and_everything_within_one_cell_of_them() {
        let points = vec![Vec3::new(0.05, 0.05, 0.05), Vec3::new(-3.0, 2.0, 7.5)];
        let set = CellSet::around(points.par_iter().copied(), 0.1);
        assert!(!set.is_empty());
        for &p in &points {
            assert!(set.contains(p));
            for offset in [Vec3::X, Vec3::NEG_Y, Vec3::ONE, Vec3::NEG_ONE] {
                assert!(set.contains(p + offset * 0.099), "within a cell of {p}");
            }
        }
        assert!(!set.contains(Vec3::new(0.35, 0.05, 0.05)));
        assert!(!set.contains(Vec3::new(1.0, 1.0, 1.0)));
        assert!(CellSet::around(Vec::<Vec3>::new().into_par_iter(), 0.1).is_empty());
    }
}
