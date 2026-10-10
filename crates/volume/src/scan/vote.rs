//! Inside and outside for a mesh with holes.
//!
//! Through a hole, a lattice line enters the surface and never leaves it (or
//! the other way round), and read on its own would paint everything from the
//! surface to the edge of the lattice as solid: a streak. A line through two
//! holes is worse, because it looks perfectly healthy and sees nothing at
//! all. So each sample is judged along all three axes and the majority
//! taken, a line that does not close counting as a vote for outside. One
//! hole spoils one axis's lines through it, the other two outvote it, and
//! the damage is confined to where two axes' lines escape through the same
//! opening: the opening itself, which comes out capped.
//!
//! Asking three lines about every sample of the lattice costs its volume,
//! and at a fine resolution that is most of a second. The votes are instead
//! worked a plane of the lattice at a time, with every pass's verdicts for
//! the plane laid out as bits along x, so that 64 samples are outvoted in
//! three instructions:
//!
//!   - Along x, a line is a row of the plane: its inside runs are filled in.
//!   - Along y, a line crosses the plane's rows. Each run marks the two rows
//!     where it starts and stops, and a running exclusive-or down the rows
//!     fills the runs in.
//!   - Along z, a line pierces the plane at one sample, and from one plane to
//!     the next only the lines with a crossing between them change. Those are
//!     listed by plane once, and each plane made from the one before.
//!
//! What is kept is the outcome, as the runs of inside samples along every x
//! row: the same form the crossings give a closed mesh, and as sparse.

use rayon::prelude::*;

use super::{Pass, row_along_x, set_bits};
use crate::grid::Grid;
use crate::parallel;

/// Planes swept by one task. Each task starts by replaying the z changes up
/// to its first plane, so fewer, longer sweeps waste less.
const SWEEP: usize = 32;

/// The inside runs of every row of samples along x, after the vote.
pub(super) struct Voted {
    /// Where each row's entries start; rows are numbered `j + ny * k`.
    offsets: Vec<u32>,
    /// The samples at which a row changes between outside and inside, in
    /// order, starting from outside.
    changes: Vec<u16>,
}

/// Calls `change` with each sample at which the line's verdict flips between
/// outside and inside, in order. A line that does not close has no verdict.
pub(super) fn line_changes(grid: &Grid, axis: usize, pass: &Pass, line: usize, mut change: impl FnMut(usize)) {
    let range = pass.line(line);
    if range.is_empty() || pass.is_open(&range) {
        return;
    }
    let mut inside = false;
    for crossing in &pass.crossings[range] {
        if (crossing.after != 0) != inside {
            inside = !inside;
            // The first sample this crossing has been passed at.
            let t = crossing.t();
            let at = if t > 0.0 { grid.first_at_or_above(axis, t) } else { grid.first_above(axis, t) };
            if at < grid.n[axis] {
                change(at);
            }
        }
    }
}

/// Every change of verdict along z, as (plane, line) packed for sorting, in
/// order of plane; and where each plane's changes start.
fn changes_by_plane(grid: &Grid, along_z: &Pass) -> (Vec<u64>, Vec<usize>) {
    let [nx, ny, nz] = grid.n;
    let parts: Vec<Vec<u64>> = (0..nx * ny)
        .into_par_iter()
        .with_min_len(4096)
        .fold(Vec::new, |mut part, line| {
            line_changes(grid, 2, along_z, line, |plane| part.push((plane as u64) << 32 | line as u64));
            part
        })
        .collect();
    let mut changes = parallel::concat(parts);
    changes.par_sort_unstable();
    let starts = (0..=nz).map(|plane| changes.partition_point(|&c| (c >> 32) < plane as u64)).collect();
    (changes, starts)
}

/// How many samples one axis's lines call inside, all told.
pub(super) fn inside_count(grid: &Grid, axis: usize, pass: &Pass) -> u64 {
    let lines = pass.offsets.len() - 1;
    let count = |line: usize| {
        let (mut total, mut from) = (0, None);
        line_changes(grid, axis, pass, line, |at| match from.take() {
            Some(start) => total += (at - start) as u64,
            None => from = Some(at),
        });
        // A run still going at the last sample.
        total + from.map_or(0, |start| (grid.n[axis] - start) as u64)
    };
    (0..lines).into_par_iter().with_min_len(4096).map(count).sum()
}

impl Voted {
    pub(super) fn new(grid: &Grid, pass: &[Pass; 3]) -> Voted {
        let [nx, ny, nz] = grid.n;
        let words = nx.div_ceil(64);
        let (z_changes, z_starts) = changes_by_plane(grid, &pass[2]);

        let sweeps: Vec<(Vec<u32>, Vec<u16>)> = (0..nz.div_ceil(SWEEP))
            .into_par_iter()
            .map(|sweep| {
                let planes = sweep * SWEEP..((sweep + 1) * SWEEP).clamp(0, nz);
                let mut z = vec![0u64; ny * words];
                let mut y = vec![0u64; ny * words];
                let mut x = vec![0u64; words];
                let flip = |plane: &mut [u64], line: u64| {
                    let (i, j) = (line as usize % nx, line as usize / nx);
                    plane[j * words + (i >> 6)] ^= 1 << (i & 63);
                };
                // Lines along z are numbered `x + nx * y`.
                z_changes[..z_starts[planes.start]].iter().for_each(|&c| flip(&mut z, c & 0xFFFF_FFFF));

                let mut counts = Vec::with_capacity(planes.len() * ny);
                let mut changes = Vec::new();
                for k in planes {
                    z_changes[z_starts[k]..z_starts[k + 1]].iter().for_each(|&c| flip(&mut z, c & 0xFFFF_FFFF));

                    // Lines along y are numbered `z + nz * x`.
                    y.fill(0);
                    for i in 0..nx {
                        line_changes(grid, 1, &pass[1], k + nz * i, |j| y[j * words + (i >> 6)] ^= 1 << (i & 63));
                    }
                    for j in 1..ny {
                        let (above, row) = y[(j - 1) * words..(j + 1) * words].split_at_mut(words);
                        row.iter_mut().zip(above.iter()).for_each(|(r, a)| *r ^= a);
                    }

                    for j in 0..ny {
                        row_along_x(grid, &pass[0], j, k, &mut x);
                        let before = changes.len();
                        let mut last = 0u64;
                        for w in 0..words {
                            let (a, b, c) = (x[w], y[j * words + w], z[j * words + w]);
                            let inside = a & b | a & c | b & c;
                            let mut edges = inside ^ (inside << 1 | last);
                            last = inside >> 63;
                            while edges != 0 {
                                changes.push((w * 64 + edges.trailing_zeros() as usize) as u16);
                                edges &= edges - 1;
                            }
                        }
                        // A run reaching the end of a row that is a whole
                        // number of words long closes just past it.
                        if last != 0 {
                            changes.push(nx as u16);
                        }
                        counts.push((changes.len() - before) as u32);
                    }
                }
                (counts, changes)
            })
            .collect();

        let mut offsets = Vec::with_capacity(ny * nz + 1);
        let mut total = 0u32;
        for (counts, _) in &sweeps {
            for &count in counts {
                offsets.push(total);
                total += count;
            }
        }
        offsets.push(total);
        Voted { offsets, changes: parallel::concat(sweeps.into_iter().map(|s| s.1).collect()) }
    }

    #[inline]
    fn changes(&self, row: usize) -> &[u16] {
        &self.changes[self.offsets[row] as usize..self.offsets[row + 1] as usize]
    }

    /// Writes the inside bits of a row; returns whether any is set.
    pub(super) fn row(&self, row: usize, out: &mut [u64]) -> bool {
        out.fill(0);
        let changes = self.changes(row);
        for run in changes.chunks_exact(2) {
            set_bits(out, run[0] as usize, run[1] as usize);
        }
        !changes.is_empty()
    }

    /// True when no row has anything inside.
    pub(super) fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// Whether sample `i` of a row is inside.
    pub(super) fn inside(&self, row: usize, i: usize) -> bool {
        self.changes(row).partition_point(|&c| c as usize <= i) % 2 == 1
    }
}
