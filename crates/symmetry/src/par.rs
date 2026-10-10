//! Small helpers for running per-vertex work across cores with results that do
//! not depend on how the work happened to be scheduled.
//!
//! Symmetry tools are judged on being repeatable: pressing the same button on
//! the same mesh must move the same vertices by the same amounts. So lists are
//! joined in index order and float sums are added in fixed blocks, never in
//! whatever order threads finish.

use rayon::prelude::*;

const BLOCK: usize = 8192;

/// Runs `emit(i, &mut out)` for every `i` in `0..n` and returns everything
/// emitted, in order of `i`.
pub(crate) fn gather<T: Send>(n: usize, emit: impl Fn(usize, &mut Vec<T>) + Sync) -> Vec<T> {
    if n <= BLOCK {
        let mut out = Vec::new();
        (0..n).for_each(|i| emit(i, &mut out));
        return out;
    }
    let blocks: Vec<Vec<T>> = (0..n.div_ceil(BLOCK))
        .into_par_iter()
        .map(|block| {
            let mut out = Vec::new();
            (block * BLOCK..((block + 1) * BLOCK).min(n)).for_each(|i| emit(i, &mut out));
            out
        })
        .collect();
    let mut all = Vec::with_capacity(blocks.iter().map(Vec::len).sum());
    blocks.into_iter().for_each(|block| all.extend(block));
    all
}

/// The indices in `0..n` that `keep` accepts, ascending.
pub(crate) fn indices_where(n: usize, keep: impl Fn(usize) -> bool + Sync) -> Vec<u32> {
    gather(n, |i, out| {
        if keep(i) {
            out.push(i as u32);
        }
    })
}

/// Sum of `value(i)` over `0..n`, identical on every run.
pub(crate) fn sum(n: usize, value: impl Fn(usize) -> f64 + Sync) -> f64 {
    let partial: Vec<f64> = (0..n.div_ceil(BLOCK))
        .into_par_iter()
        .map(|block| (block * BLOCK..((block + 1) * BLOCK).min(n)).map(&value).sum())
        .collect();
    partial.iter().sum()
}

/// How many `i` in `0..n` satisfy `test`.
pub(crate) fn count(n: usize, test: impl Fn(usize) -> bool + Sync) -> usize {
    (0..n).into_par_iter().with_min_len(BLOCK).filter(|&i| test(i)).count()
}

/// Sorts with every core when there is enough to be worth it. The bar is high
/// because the pairing sorts a few thousand candidates a round for thousands of
/// rounds, and waking the pool for each costs more than the sort.
pub(crate) fn sort<T: Ord + Send>(items: &mut [T]) {
    if items.len() > 1 << 16 {
        items.par_sort_unstable();
    } else {
        items.sort_unstable();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gather_keeps_index_order_across_blocks() {
        let n = BLOCK * 3 + 17;
        let got = gather(n, |i, out| {
            if i % 3 == 0 {
                out.push(i);
            }
        });
        assert_eq!(got, (0..n).filter(|i| i % 3 == 0).collect::<Vec<_>>());
        assert_eq!(indices_where(n, |i| i % 7 == 1).len(), count(n, |i| i % 7 == 1));
    }

    #[test]
    fn sum_is_repeatable() {
        let n = BLOCK * 5 + 3;
        let value = |i: usize| 1.0 / (i as f64 + 1.0);
        let first = sum(n, value);
        assert!((0..8).all(|_| sum(n, value) == first));
    }
}
