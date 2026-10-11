//! The lattice of sample points that scan conversion, sampled fields and
//! surface extraction all share.
//!
//! Sample `i` on an axis sits at `(lo + i + 0.5) * cell`. The half step is
//! deliberate, and it is the first thing exact symmetry depends on:
//!
//!   - The coordinate planes run through the middle of a column of cells
//!     rather than through a plane of samples. With a sample plane on the
//!     mirror plane, the quads around the lattice edges lying in that plane
//!     are each their own mirror image, and such a quad cannot be split into
//!     two triangles symmetrically: whichever diagonal it takes, its mirror
//!     takes the other. With the plane through cell centres no quad straddles
//!     it, and that column's vertices sit on the plane.
//!   - Sample `i` and its mirror image are computed from integers that are
//!     exact negatives of one another, so their coordinates are bit-for-bit
//!     reflections. A lattice anchored to the bounds instead straddles the
//!     plane by an arbitrary fraction of a cell, and a symmetric model then
//!     comes back asymmetric by construction.

use glam::Vec3;
use sculpt_geom::Aabb;

/// Spare samples kept beyond the bounds on every side, so the surface always
/// closes inside the lattice even where rounding puts a sample on the bounds.
pub const PAD: i64 = 2;

/// Most samples along one axis. Cell coordinates are kept in 16 bits.
pub const MAX_SAMPLES: usize = 60_000;

/// Farthest a lattice may sit from the origin, in cells. Beyond this the
/// model's own single-precision coordinates are coarser than a cell.
const MAX_INDEX: i64 = 1 << 22;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    /// Spacing between neighbouring samples.
    pub cell: f32,
    /// Lattice index of sample 0 on each axis.
    pub lo: [i32; 3],
    /// Samples along each axis. There is one fewer cell.
    pub n: [usize; 3],
}

impl Grid {
    /// The lattice of spacing `cell` that covers `bounds` with [`PAD`] spare
    /// samples all round.
    pub fn covering(bounds: &Aabb, cell: f32) -> Result<Grid, String> {
        Grid::covering_padded(bounds, cell, PAD)
    }

    pub fn covering_padded(bounds: &Aabb, cell: f32, pad: i64) -> Result<Grid, String> {
        let reach = Grid::reach(bounds, cell, pad)?;
        if reach.iter().any(|&(_, count)| count > MAX_SAMPLES as i64) {
            return Err("That resolution is too fine for this model. Try a lower one.".into());
        }
        Ok(Grid { cell, lo: reach.map(|(first, _)| first as i32), n: reach.map(|(_, count)| count as usize) })
    }

    /// The lattice index of the first sample and the number of samples that
    /// cover `bounds` on each axis, with nothing yet said about whether that
    /// many can be afforded: [`crate::budget`] asks before a lattice is made,
    /// and has to be able to hear an answer far too large to make.
    pub(crate) fn reach(bounds: &Aabb, cell: f32, pad: i64) -> Result<[(i64, i64); 3], String> {
        if bounds.is_empty() || !bounds.min.is_finite() || !bounds.max.is_finite() {
            return Err("The model has no size.".into());
        }
        if !(cell > 0.0 && cell.is_finite()) {
            return Err("The cell size must be more than zero.".into());
        }
        let h = cell as f64;
        let mut reach = [(0i64, 0i64); 3];
        for a in 0..3 {
            let first = (bounds.min[a] as f64 / h - 0.5).floor() - pad as f64;
            let last = (bounds.max[a] as f64 / h - 0.5).ceil() + pad as f64;
            if first.abs() > MAX_INDEX as f64 || last.abs() > MAX_INDEX as f64 {
                return Err("The model is too far from its own centre for a grid this fine. \
                            Centre it or lower the resolution."
                    .into());
            }
            reach[a] = (first as i64, (last - first) as i64 + 1);
        }
        Ok(reach)
    }

    /// One axis of a lattice, widened until it is its own mirror image across
    /// the coordinate plane: samples `first..-1` lie below the plane and `0..`
    /// above it.
    pub(crate) fn mirrored_reach((first, count): (i64, i64)) -> (i64, i64) {
        let half = (-first).max(first + count).max(1);
        (-half, 2 * half)
    }

    /// Widened along the chosen axes until the lattice is its own mirror image
    /// across those coordinate planes.
    pub fn mirrored(mut self, axes: [bool; 3]) -> Grid {
        for a in (0..3).filter(|&a| axes[a]) {
            let (first, count) = Grid::mirrored_reach((self.lo[a] as i64, self.n[a] as i64));
            (self.lo[a], self.n[a]) = (first as i32, count as usize);
        }
        self
    }

    /// One more sample on every side.
    pub fn grown(&self) -> Grid {
        Grid { cell: self.cell, lo: self.lo.map(|l| l - 1), n: self.n.map(|n| n + 2) }
    }

    #[inline]
    pub fn coord(&self, axis: usize, i: usize) -> f32 {
        ((self.lo[axis] + i as i32) as f32 + 0.5) * self.cell
    }

    #[inline]
    pub fn position(&self, i: usize, j: usize, k: usize) -> Vec3 {
        Vec3::new(self.coord(0, i), self.coord(1, j), self.coord(2, k))
    }

    /// Where sample (i, j, k) lives in an array laid out x fastest, z slowest.
    #[inline]
    pub fn index(&self, i: usize, j: usize, k: usize) -> usize {
        i + self.n[0] * (j + self.n[1] * k)
    }

    pub fn sample_count(&self) -> usize {
        self.n[0] * self.n[1] * self.n[2]
    }

    /// The cell column a coordinate plane runs through the middle of. May lie
    /// outside the lattice when the model is all to one side of the plane.
    #[inline]
    pub fn plane_cell(&self, axis: usize) -> i64 {
        -(self.lo[axis] as i64) - 1
    }

    /// The first sample with a positive coordinate, clamped into `0..=n`.
    #[inline]
    pub fn zero_index(&self, axis: usize) -> usize {
        (-(self.lo[axis] as i64)).clamp(0, self.n[axis] as i64) as usize
    }

    /// The first sample at or above `w` (`n` when there is none).
    ///
    /// An estimate from a division, then settled by comparing against the
    /// sample coordinates themselves: every part of the pipeline has to agree
    /// on which side of a sample a point lies, and only the coordinates the
    /// samples are actually given can be the judge of that.
    #[inline]
    pub fn first_at_or_above(&self, axis: usize, w: f32) -> usize {
        let n = self.n[axis];
        let mut i = self.estimate(axis, w);
        while i > 0 && self.coord(axis, i - 1) >= w {
            i -= 1;
        }
        while i < n && self.coord(axis, i) < w {
            i += 1;
        }
        i
    }

    /// The first sample strictly above `w` (`n` when there is none).
    #[inline]
    pub fn first_above(&self, axis: usize, w: f32) -> usize {
        let n = self.n[axis];
        let mut i = self.estimate(axis, w);
        while i > 0 && self.coord(axis, i - 1) > w {
            i -= 1;
        }
        while i < n && self.coord(axis, i) <= w {
            i += 1;
        }
        i
    }

    /// Roughly the sample at `w`: within one of the right answer, which is
    /// all its callers need. Truncated rather than rounded because rounding
    /// is a library call on a plain x86-64 build, and this runs twelve times
    /// for every source triangle.
    #[inline]
    fn estimate(&self, axis: usize, w: f32) -> usize {
        let i = lattice_index(w as f64 / self.cell as f64) - self.lo[axis] as i64;
        i.clamp(0, self.n[axis] as i64) as usize
    }

    /// The samples whose coordinate lies in `lo..=hi`, as a half-open range of
    /// indices.
    #[inline]
    pub fn span(&self, axis: usize, lo: f32, hi: f32) -> (usize, usize) {
        (self.first_at_or_above(axis, lo), self.first_above(axis, hi))
    }
}

/// The whole part of a coordinate measured in cells from the first sample
/// above the origin, kept far inside what 64 bits hold so that the lattice's
/// own offset can be taken from it whatever the coordinate was.
#[inline]
pub(crate) fn lattice_index(cells: f64) -> i64 {
    (cells - 0.5).clamp(-1e15, 1e15) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> Grid {
        Grid::covering(&Aabb::new(Vec3::new(-1.0, -0.3, 0.2), Vec3::new(1.0, 0.7, 0.9)), 0.07).unwrap()
    }

    #[test]
    fn samples_are_exact_mirror_images() {
        let g = grid().mirrored([true, true, false]);
        for axis in 0..2 {
            assert_eq!(g.n[axis] % 2, 0);
            for i in 0..g.n[axis] {
                let twin = g.n[axis] - 1 - i;
                assert_eq!(g.coord(axis, i).to_bits(), (-g.coord(axis, twin)).to_bits());
            }
            let plane = g.plane_cell(axis) as usize;
            assert!(g.coord(axis, plane) < 0.0 && g.coord(axis, plane + 1) > 0.0);
        }
    }

    #[test]
    fn bounds_are_covered_with_room_to_spare() {
        let g = grid();
        let (min, max) = (Vec3::new(-1.0, -0.3, 0.2), Vec3::new(1.0, 0.7, 0.9));
        for a in 0..3 {
            assert!(g.coord(a, 1) < min[a] && g.coord(a, g.n[a] - 2) > max[a]);
        }
    }

    #[test]
    fn index_searches_agree_with_the_coordinates() {
        let g = grid();
        for step in 0..4000 {
            let w = -1.3 + step as f32 * 0.00071;
            let at = g.first_at_or_above(0, w);
            let above = g.first_above(0, w);
            assert!(at == g.n[0] || g.coord(0, at) >= w);
            assert!(at == 0 || g.coord(0, at - 1) < w);
            assert!(above == g.n[0] || g.coord(0, above) > w);
            assert!(above == 0 || g.coord(0, above - 1) <= w);
        }
        // Exactly on a sample: at-or-above includes it, above does not.
        let w = g.coord(0, 7);
        assert_eq!((g.first_at_or_above(0, w), g.first_above(0, w)), (7, 8));
        assert_eq!(g.span(0, w, w), (7, 8));
    }

    #[test]
    fn the_zero_index_stays_inside_the_lattice() {
        let all_above = Grid { cell: 0.5, lo: [3, 0, 0], n: [4; 3] };
        let all_below = Grid { cell: 0.5, lo: [-9, -4, -1], n: [4; 3] };
        assert_eq!([0, 1, 2].map(|a| all_above.zero_index(a)), [0, 0, 0]);
        assert_eq!([0, 1, 2].map(|a| all_below.zero_index(a)), [4, 4, 1]);
        for g in [all_above, all_below] {
            for a in 0..3 {
                let zero = g.zero_index(a);
                assert!(zero == 0 || g.coord(a, zero - 1) < 0.0);
                assert!(zero == g.n[a] || g.coord(a, zero) > 0.0);
            }
        }
        // Coordinates far outside either end land on the ends.
        assert_eq!(all_above.span(0, -1e30, 1e30), (0, 4));
        assert_eq!(all_above.span(0, 1e30, 1e31), (4, 4));
        assert_eq!(all_above.span(0, -1e31, -1e30), (0, 0));
        assert_eq!(all_below.span(0, f32::MIN, f32::MAX), (0, 4));
        assert_eq!(all_below.span(0, f32::NEG_INFINITY, f32::INFINITY), (0, 4));
    }

    #[test]
    fn nonsense_is_refused() {
        let b = Aabb::new(Vec3::ZERO, Vec3::ONE);
        assert!(Grid::covering(&b, 0.0).is_err());
        assert!(Grid::covering(&b, 1e-9).is_err());
        assert!(Grid::covering(&Aabb::EMPTY, 0.1).is_err());
    }
}
