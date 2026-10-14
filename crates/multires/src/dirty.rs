//! Carrying an edit up one level by way of what it changed.
//!
//! [`crate::detail::carry`] and [`crate::refine::raise_over`] go over every
//! vertex of the finer level, to find the few an edit reached and leave the
//! rest as they were. On a model of ten million triangles with a dab made on
//! its coarsest level that is millions of vertices looked at for a few
//! thousand changed. Here the same step starts from the other end: from the
//! coarse vertices that changed, to the fine vertices whose rule reads one of
//! them ([`Refiner::touched_by`]), to those whose base moved, to those whose
//! fan holds one of those (a fan is symmetric: see [`Refiner::fan`]). Only
//! those are worked out, by the very same functions the whole-level pass
//! calls, so the result is the same to the bit (`tests/dirty.rs` runs both
//! ways over the same edits, including none and all).
//!
//! Each level is worked in place on one array, `work`, that ends as the
//! finest level after the edit. Before a level it holds the coarser level as
//! it now is in its first entries and the finer levels as they were beyond;
//! beside it `kept` is the whole of the finest level as it was, of which the
//! coarser level as it was is the first entries too. Each pass reads all it
//! needs before it writes anything, as every per-vertex pass here does.
//!
//! When an edit reaches much of a level (a pose, a scale, a mask filled), the
//! bookkeeping costs more than looking at everything, so that level is done
//! the whole-level way. Which way a level goes changes only how long it takes.

use std::sync::atomic::{AtomicU64, Ordering};

use glam::Vec3;
use rayon::prelude::*;

use crate::detail::{carried_one, carry};
use crate::refine::{Channel, PER_TASK, Refiner, Split, place_one, raise_over, raise_over_one};

/// Above one fine vertex in this many to look at, the whole level is gone
/// over instead. The two ways take about as long when an edit reaches a
/// third of a level, and the dirty way twice as long when it reaches all of
/// it (`timing_of_the_two_ways` in `tests/dirty.rs`).
const WHOLE_ABOVE: usize = 4;

/// Which way a level is carried up: by what changed, the whole level, or
/// whichever is quicker. Only tests ask for one in particular.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Way {
    Quicker,
    #[cfg_attr(not(test), allow(dead_code))]
    Dirty,
    Whole,
}

/// The coarse level's positions in `work` carried up into the finer level's
/// (see the module notes), `changed` listing, ascending, the coarse vertices
/// where `work` and `kept` differ. Gives back where they differ afterwards,
/// ascending: the next level's `changed`.
pub(crate) fn carry_level<R: Refiner>(
    r: &R,
    split: Split,
    kept: &[Vec3],
    work: &mut [Vec3],
    changed: &[u32],
    way: Way,
) -> Vec<u32> {
    let (coarse, total) = (r.kept(), r.total());
    if changed.is_empty() {
        // `work` already holds the finer level as it was, which is what it stays.
        return Vec::new();
    }
    let looked_at = touched(r, changed).members;
    if way == Way::Whole || (way == Way::Quicker && looked_at.len() * WHOLE_ABOVE > total) {
        let carried = carry(r, split, &kept[..coarse], &work[..coarse], &kept[..total]);
        work[..total].par_copy_from(&carried);
        return differing(&carried, kept);
    }
    let (coarse_was, coarse_now) = (&kept[..coarse], &work[..coarse]);
    let base_of = |i: u32| (place_one(r, split, coarse_was, i as usize), place_one(r, split, coarse_now, i as usize));
    // Fine vertices whose base moved: only those a changed vertex touches can have.
    let moved: Vec<u32> = looked_at
        .par_iter()
        .with_min_len(PER_TASK)
        .filter(|&&i| {
            let (was, now) = base_of(i);
            !was.same(now)
        })
        .copied()
        .collect();
    let reached = with_fans(r, &moved).members;
    // The base before and after at every vertex a reached vertex's turn reads.
    let needed = with_fans(r, &reached);
    let bases: Vec<(Vec3, Vec3)> = needed.members.par_iter().with_min_len(PER_TASK).map(|&i| base_of(i)).collect();
    let base = |v: usize| bases[needed.rank(v)];
    let carried: Vec<Vec3> =
        reached.par_iter().with_min_len(PER_TASK / 8).map(|&i| carried_one(r, i as usize, kept[i as usize], base)).collect();
    // The whole-level pass hands back as it was every fine vertex it did not
    // reach, a kept one that changed on the coarser level included.
    for &c in changed {
        work[c as usize] = kept[c as usize];
    }
    for (&i, &p) in reached.iter().zip(&carried) {
        work[i as usize] = p;
    }
    reached.into_iter().filter(|&i| !work[i as usize].same(kept[i as usize])).collect()
}

/// [`carry_level`] for a channel (the mask): what [`raise_over`] does, with
/// `settle` putting a moved value back in range.
pub(crate) fn raise_level<R: Refiner, T: Channel>(
    r: &R,
    kept: &[T],
    work: &mut [T],
    changed: &[u32],
    settle: &(impl Fn(T) -> T + Sync),
    way: Way,
) -> Vec<u32> {
    let (coarse, total) = (r.kept(), r.total());
    if changed.is_empty() {
        return Vec::new();
    }
    let looked_at = touched(r, changed).members;
    if way == Way::Whole || (way == Way::Quicker && looked_at.len() * WHOLE_ABOVE > total) {
        let raised = raise_over(r, &kept[..coarse], &work[..coarse], &kept[..total], settle);
        work[..total].par_copy_from(&raised);
        return differing(&raised, kept);
    }
    // A kept vertex is what it is on the coarser level, and every changed one
    // is in `looked_at`, so writing these is the whole of the step.
    let raised: Vec<T> = looked_at
        .par_iter()
        .with_min_len(PER_TASK)
        .map(|&i| raise_over_one(r, &kept[..coarse], &work[..coarse], kept, settle, i as usize))
        .collect();
    for (&i, &value) in looked_at.iter().zip(&raised) {
        work[i as usize] = value;
    }
    looked_at.into_iter().filter(|&i| !work[i as usize].same(kept[i as usize])).collect()
}

/// The fine vertices to look at for an edit of the coarse vertices
/// `changed`: those themselves and every one whose rule reads one of them.
fn touched<R: Refiner>(r: &R, changed: &[u32]) -> Set {
    Set::gathered(r.total(), changed, |c, mark| {
        mark(c);
        r.touched_by(c, mark);
    })
}

/// `set` with every vertex of each one's fan.
fn with_fans<R: Refiner>(r: &R, set: &[u32]) -> Set {
    Set::gathered(r.total(), set, |v, mark| {
        mark(v);
        r.fan(v as usize, |a, b| {
            mark(a);
            mark(b);
        });
    })
}

/// Some of a level's vertices: which, ascending and each once, and a bit
/// for each vertex of the level that says so, from which a member's place
/// in the list is quickly counted. (Sorting the lists instead, and looking
/// places up in a table as long as the level, took most of the time of a
/// step up after a small edit.)
struct Set {
    members: Vec<u32>,
    words: Vec<u64>,
    /// How many members lie below each word's first vertex.
    before: Vec<u32>,
}

impl Set {
    /// Every vertex `each` marks for the members of `from`, below `len`.
    fn gathered(len: usize, from: &[u32], each: impl Fn(u32, &mut dyn FnMut(u32)) + Sync) -> Set {
        let marks: Vec<AtomicU64> = (0..len.div_ceil(64)).map(|_| AtomicU64::new(0)).collect();
        from.par_iter().with_min_len(PER_TASK / 8).for_each(|&v| {
            each(v, &mut |m| {
                let (word, bit) = (&marks[m as usize / 64], 1u64 << (m % 64));
                if word.load(Ordering::Relaxed) & bit == 0 {
                    word.fetch_or(bit, Ordering::Relaxed);
                }
            })
        });
        let words: Vec<u64> = marks.into_iter().map(AtomicU64::into_inner).collect();
        let mut before = Vec::with_capacity(words.len());
        let mut count = 0u32;
        for word in &words {
            before.push(count);
            count += word.count_ones();
        }
        let members = words
            .par_iter()
            .enumerate()
            .with_min_len(PER_TASK / 64)
            .flat_map_iter(|(w, &word)| {
                let mut rest = word;
                std::iter::from_fn(move || {
                    (rest != 0).then(|| {
                        let bit = rest.trailing_zeros();
                        rest &= rest - 1;
                        (w * 64) as u32 + bit
                    })
                })
            })
            .collect();
        Set { members, words, before }
    }

    /// Where member `v` is in `members`.
    #[inline]
    fn rank(&self, v: usize) -> usize {
        let below = self.words[v / 64] & ((1u64 << (v % 64)) - 1);
        self.before[v / 64] as usize + below.count_ones() as usize
    }
}

/// Where `now` differs from the first entries of `was`, ascending.
fn differing<T: Channel>(now: &[T], was: &[T]) -> Vec<u32> {
    (0..now.len() as u32)
        .into_par_iter()
        .with_min_len(PER_TASK)
        .filter(|&i| !now[i as usize].same(was[i as usize]))
        .collect()
}

/// A copy from one slice into another of the same length, on every thread.
trait ParCopy<T> {
    fn par_copy_from(&mut self, from: &[T]);
}

impl<T: Copy + Send + Sync> ParCopy<T> for [T] {
    fn par_copy_from(&mut self, from: &[T]) {
        self.par_chunks_mut(1 << 16).zip(from.par_chunks(1 << 16)).for_each(|(to, from)| to.copy_from_slice(from));
    }
}
