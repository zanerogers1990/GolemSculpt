//! A solid made its own mirror image: what lies on the positive side of each
//! mirror plane, and its reflection on the negative side in place of whatever
//! was there.
//!
//! A model sculpted with a mirror on is almost never symmetric to the bit:
//! mirrored strokes drift by a few parts in ten million, and the user may
//! have worked one side alone. A surface extracted from such a model and
//! then paired up vertex by vertex comes out symmetric only where the two
//! halves happened to agree, and it is the handful of cells where they did
//! not that mirrored brushes then pull apart. So when a mirror axis is asked
//! for, the question of what the negative half looks like is not put to the
//! source at all. Every sample, edge crossing and corner value on that side
//! is read from its twin on the positive side, as the Unity remesher copied
//! signs from the upper half onto the lower, here for everything a cell is
//! built from rather than for the signs alone (signs from one half against
//! distances from the other is what tore its meshes when the halves really
//! differed).
//!
//! The consequence is meant: with a mirror on, a bump sculpted on the
//! positive side alone appears on both, one on the negative side alone is
//! gone, and a model lying wholly on the negative side leaves nothing.

use glam::Vec3;

use crate::extract::{CellSource, Hermite};
use crate::grid::Grid;
use crate::topology::{self, EDGE_A, EDGE_B};

pub(crate) struct Mirrored<'a, S> {
    source: &'a S,
    /// Samples the source's lattice has on each axis, and where its first
    /// sample falls on the lattice this is read on.
    source_n: [usize; 3],
    shift: [usize; 3],
    /// Samples of the lattice this is read on: the source's, widened until it
    /// is its own mirror image on the mirrored axes.
    n: [usize; 3],
    axes: [bool; 3],
}

/// Bits `at..at + 64` of a row, with nothing beyond either end.
#[inline]
fn window(row: &[u64], at: isize) -> u64 {
    if at <= -64 || at >= row.len() as isize * 64 {
        return 0;
    }
    if at < 0 {
        return row[0] << -at;
    }
    let (word, bit) = (at as usize / 64, at as usize % 64);
    let above = if bit > 0 && word + 1 < row.len() { row[word + 1] << (64 - bit) } else { 0 };
    row[word] >> bit | above
}

/// Overwrites the lower half of a row of `n` bits with the upper half
/// reversed: bit `i` becomes bit `n - 1 - i`. `spare` is scratch.
fn reflect_row(row: &mut [u64], n: usize, spare: &mut Vec<u64>) {
    let half = n / 2;
    let words = half.div_ceil(64);
    spare.clear();
    // Bit `b` of word `w` is sample `64 w + b`, whose twin is `n - 1 - 64 w - b`:
    // the 64 bits ending at `n - 1 - 64 w`, read downwards.
    spare.extend((0..words).map(|w| window(row, n as isize - 64 * (w as isize + 1)).reverse_bits()));
    for (w, &twin) in spare.iter().enumerate() {
        let keep = if (w + 1) * 64 <= half { 0 } else { !0u64 << (half - w * 64) };
        row[w] = row[w] & keep | twin & !keep;
    }
}

impl<'a, S: CellSource> Mirrored<'a, S> {
    /// `source` as read on `grid`, mirrored across the coordinate planes of
    /// `axes`; and the lattice to extract it on.
    pub(crate) fn new(source: &'a S, grid: &Grid, axes: [bool; 3]) -> (Mirrored<'a, S>, Grid) {
        let wide = grid.mirrored(axes);
        let shift = [0, 1, 2].map(|a| (grid.lo[a] - wide.lo[a]) as usize);
        (Mirrored { source, source_n: grid.n, shift, n: wide.n, axes }, wide)
    }

    /// The sample of the source that sample `s` of this lattice is read
    /// from: its twin if it is on the negative side. `None` beyond the
    /// source's lattice, where there is nothing.
    #[inline]
    fn from_sample(&self, axis: usize, s: usize) -> Option<usize> {
        let positive = if self.axes[axis] && s < self.n[axis] / 2 { self.n[axis] - 1 - s } else { s };
        positive.checked_sub(self.shift[axis]).filter(|&i| i < self.source_n[axis])
    }

    /// The cell of the source that `cell` is read from, which axes it is
    /// reflected along to get there, and which mirror planes pass through it.
    fn from_cell(&self, cell: [usize; 3]) -> Option<([usize; 3], usize, usize)> {
        let (mut from, mut flips, mut astride) = (cell, 0, 0);
        for a in (0..3).filter(|&a| self.axes[a]) {
            let plane = self.n[a] / 2 - 1;
            if cell[a] < plane {
                from[a] = self.n[a] - 2 - cell[a];
                flips |= 1 << a;
            } else if cell[a] == plane {
                astride |= 1 << a;
            }
        }
        for a in 0..3 {
            from[a] = from[a].checked_sub(self.shift[a]).filter(|&c| c + 1 < self.source_n[a])?;
        }
        Some((from, flips, astride))
    }
}

/// `v` with its components along the axes in `flips` negated.
#[inline]
fn reflected(mut v: Vec3, flips: usize) -> Vec3 {
    for a in 0..3 {
        if flips >> a & 1 != 0 {
            v[a] = -v[a];
        }
    }
    v
}

impl<S: CellSource> CellSource for Mirrored<'_, S> {
    fn row(&self, j: usize, k: usize, out: &mut [u64], scratch: &mut Vec<u64>) -> bool {
        let (Some(from_j), Some(from_k)) = (self.from_sample(1, j), self.from_sample(2, k)) else {
            out.fill(0);
            return false;
        };
        if self.shift[0] == 0 && self.source_n[0] == self.n[0] {
            if !self.source.row(from_j, from_k, out, scratch) {
                return false;
            }
        } else {
            // The source's row is shorter and starts part way along this one.
            let mut narrow = std::mem::take(scratch);
            narrow.clear();
            narrow.resize(self.source_n[0].div_ceil(64), 0);
            let any = self.source.row(from_j, from_k, &mut narrow, &mut Vec::new());
            out.fill(0);
            for (w, &bits) in narrow.iter().enumerate().filter(|(_, bits)| **bits != 0) {
                let at = w * 64 + self.shift[0];
                out[at / 64] |= bits << (at % 64);
                if at % 64 != 0 && at / 64 + 1 < out.len() {
                    out[at / 64 + 1] |= bits >> (64 - at % 64);
                }
            }
            *scratch = narrow;
            if !any {
                return false;
            }
        }
        if self.axes[0] {
            reflect_row(out, self.n[0], scratch);
        }
        out.iter().any(|&w| w != 0)
    }

    fn hermite(&self, cell: [usize; 3], mask: u8, out: &mut Hermite) {
        let Some((from, flips, astride)) = self.from_cell(cell) else {
            *out = Hermite::default();
            return;
        };
        let mut read = Hermite::default();
        self.source.hermite(from, topology::flip_mask(mask, flips), &mut read);
        // A cell a mirror plane passes through is its own twin: the face on
        // the negative side is the reflection of the face on the positive.
        for a in (0..3).filter(|&a| astride >> a & 1 != 0) {
            for e in (0..12).filter(|&e| EDGE_A[e] >> a & 1 == 0 && EDGE_B[e] >> a & 1 == 0) {
                let twin = topology::flip_edge(1 << a, e);
                read.t[e] = read.t[twin];
                read.normal[e] = reflected(read.normal[twin], 1 << a);
            }
        }
        for e in (0..12).filter(|&e| topology::edge_crosses(mask, e)) {
            let twin = topology::flip_edge(flips, e);
            // An edge along a reflected axis runs the other way.
            out.t[e] = if flips >> (e / 4) & 1 != 0 { 1.0 - read.t[twin] } else { read.t[twin] };
            out.normal[e] = reflected(read.normal[twin], flips);
        }
    }

    fn corner_values(&self, cell: [usize; 3], mask: u8) -> [f32; 8] {
        let Some((from, flips, astride)) = self.from_cell(cell) else {
            return std::array::from_fn(|c| if mask >> c & 1 != 0 { -1.0 } else { 1.0 });
        };
        let read = self.source.corner_values(from, topology::flip_mask(mask, flips));
        // Corners on the negative side of a plane through the cell take their
        // twins' values; then the whole is reflected into place.
        std::array::from_fn(|c| read[(c ^ flips) | astride])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_is_reflected_about_its_middle() {
        for n in [2usize, 10, 64, 126, 128, 130, 200, 1000] {
            let words = n.div_ceil(64);
            let mut row = vec![0u64; words];
            // A pattern with no symmetry of its own, in the upper half only
            // at first, then with rubbish in the lower half to be replaced.
            let set = |i: usize| i % 3 == 0 || i % 7 == 2 || i == n - 1;
            for i in (n / 2..n).filter(|&i| set(i)) {
                row[i / 64] |= 1 << (i % 64);
            }
            for rubbish in [false, true] {
                if rubbish {
                    (0..n / 2).step_by(2).for_each(|i| row[i / 64] |= 1 << (i % 64));
                }
                reflect_row(&mut row, n, &mut Vec::new());
                for i in 0..n {
                    let want = set(if i < n / 2 { n - 1 - i } else { i });
                    assert_eq!(row[i / 64] >> (i % 64) & 1 != 0, want, "bit {i} of {n}");
                }
                assert!(n % 64 == 0 || row[words - 1] >> (n % 64) == 0, "bits past the end of {n}");
            }
        }
    }

    #[test]
    fn a_window_reads_across_words_and_past_the_ends() {
        let row = [0xFFFF_0000_0000_00FFu64, 0x1];
        assert_eq!(window(&row, 0), row[0]);
        assert_eq!(window(&row, 4), 0x1FFF_F000_0000_000F);
        assert_eq!(window(&row, 64), 1);
        assert_eq!(window(&row, 100), 0);
        assert_eq!(window(&row, -8), 0xFF00_0000_0000_FF00);
        assert_eq!(window(&row, -64), 0);
        assert_eq!(window(&row, 128), 0);
    }
}
