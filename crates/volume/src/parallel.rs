//! Building large arrays in pieces, in parallel, without filling them with
//! zeros first.
//!
//! A remesh at ten million triangles writes most of a gigabyte of arrays, and
//! on Windows the first write to each fresh page of memory is the expensive
//! part. Zeroing an array on one thread and then overwriting it on all of
//! them pays for every page twice and takes the first turn single file. Here
//! each piece of an array is written exactly once, by the thread that works
//! out its contents.

use std::mem::MaybeUninit;

use rayon::prelude::*;

/// One piece of a vector under construction. Whoever is handed it must push
/// exactly as many values as the piece holds.
pub(crate) struct Slots<'a, T> {
    slots: &'a mut [MaybeUninit<T>],
    written: usize,
}

impl<T> Slots<'_, T> {
    /// How many values the piece holds when full.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.slots.len()
    }

    #[inline]
    pub(crate) fn push(&mut self, value: T) {
        self.slots[self.written].write(value);
        self.written += 1;
    }

    #[inline]
    pub(crate) fn extend(&mut self, values: &[T])
    where
        T: Copy,
    {
        for (slot, &value) in self.slots[self.written..self.written + values.len()].iter_mut().zip(values) {
            slot.write(value);
        }
        self.written += values.len();
    }

    fn finish(self) {
        assert_eq!(self.written, self.slots.len(), "a piece of the array was left unfilled");
    }
}

/// Cuts `data` into consecutive pieces ending at each of `ends`.
pub(crate) fn split_at_ends<'a, T>(mut data: &'a mut [T], ends: &[usize]) -> Vec<&'a mut [T]> {
    let mut out = Vec::with_capacity(ends.len());
    let mut at = 0;
    for &end in ends {
        let (head, tail) = std::mem::take(&mut data).split_at_mut(end - at);
        out.push(head);
        data = tail;
        at = end;
    }
    out
}

/// A vector with room for `total` values, and that room cut at `ends`.
fn pieces<'a, T>(out: &'a mut Vec<T>, ends: &[usize]) -> Vec<Slots<'a, T>> {
    let total = ends.last().copied().unwrap_or(0);
    out.reserve_exact(total);
    let spare = &mut out.spare_capacity_mut()[..total];
    split_at_ends(spare, ends).into_iter().map(|slots| Slots { slots, written: 0 }).collect()
}

/// Builds a vector whose piece `i` is elements `ends[i - 1]..ends[i]`, each
/// piece filled by `fill(i, piece)` on whichever thread is free.
pub(crate) fn build<T: Send>(ends: &[usize], fill: impl Fn(usize, &mut Slots<T>) + Sync) -> Vec<T> {
    let mut out = Vec::new();
    pieces(&mut out, ends).into_par_iter().enumerate().for_each(|(i, mut piece)| {
        fill(i, &mut piece);
        piece.finish();
    });
    // SAFETY: the pieces tile the first `ends.last()` elements of the spare
    // capacity exactly, and every one was checked to be completely written.
    // Had any piece panicked instead, `for_each` would have resumed the panic
    // and this line would not be reached.
    unsafe { out.set_len(ends.last().copied().unwrap_or(0)) };
    out
}

/// [`build`] for three vectors cut at the same places and filled together.
/// Also returns whatever each `fill` returned, in order.
pub(crate) fn build3<A: Send, B: Send, C: Send, R: Send>(
    ends: &[usize],
    fill: impl Fn(usize, &mut Slots<A>, &mut Slots<B>, &mut Slots<C>) -> R + Sync,
) -> (Vec<A>, Vec<B>, Vec<C>, Vec<R>) {
    let (mut a, mut b, mut c) = (Vec::new(), Vec::new(), Vec::new());
    let returned = pieces(&mut a, ends)
        .into_par_iter()
        .zip(pieces(&mut b, ends))
        .zip(pieces(&mut c, ends))
        .enumerate()
        .map(|(i, ((mut a, mut b), mut c))| {
            let returned = fill(i, &mut a, &mut b, &mut c);
            a.finish();
            b.finish();
            c.finish();
            returned
        })
        .collect();
    let total = ends.last().copied().unwrap_or(0);
    // SAFETY: as in `build`, for each of the three.
    unsafe {
        a.set_len(total);
        b.set_len(total);
        c.set_len(total);
    }
    (a, b, c, returned)
}

/// Joins the pieces end to end. Large totals are copied in parallel; below a
/// few megabytes, handing the work out costs more than doing it.
pub(crate) fn concat<T: Copy + Send + Sync>(parts: Vec<Vec<T>>) -> Vec<T> {
    const SERIAL_BYTES: usize = 4 << 20;
    let total: usize = parts.iter().map(Vec::len).sum();
    if total * size_of::<T>() <= SERIAL_BYTES {
        let mut out = Vec::with_capacity(total);
        parts.iter().for_each(|p| out.extend_from_slice(p));
        return out;
    }
    let mut end = 0;
    let ends: Vec<usize> = parts
        .iter()
        .map(|p| {
            end += p.len();
            end
        })
        .collect();
    build(&ends, |i, piece| piece.extend(&parts[i]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pieces_are_joined_in_order() {
        let parts = vec![vec![1u32, 2], vec![], vec![3], vec![4, 5, 6]];
        assert_eq!(concat(parts), [1, 2, 3, 4, 5, 6]);
        assert!(concat::<u32>(Vec::new()).is_empty());
        // Enough to take the parallel path.
        let big: Vec<Vec<u64>> = (0..300u64).map(|p| (0..5000).map(|i| p * 5000 + i).collect()).collect();
        let joined = concat(big);
        assert_eq!(joined.len(), 1_500_000);
        assert!(joined.iter().enumerate().all(|(i, &v)| v == i as u64));
    }

    #[test]
    fn a_vector_is_built_from_its_pieces() {
        let ends = [3, 3, 10, 11];
        let built = build(&ends, |i, piece| {
            for n in 0..piece.len() {
                piece.push((i, n));
            }
        });
        assert_eq!(built.len(), 11);
        assert_eq!(built[..4], [(0, 0), (0, 1), (0, 2), (2, 0)]);
        assert_eq!(built[10], (3, 0));
        assert!(build::<u8>(&[], |_, _| {}).is_empty());

        let (a, b, c, sizes) = build3(&ends, |i, a, b, c| {
            for n in 0..a.len() {
                a.push(i as u8);
                b.push(n as u16);
                c.push(1.5f32);
            }
            a.len()
        });
        assert_eq!((a.len(), b.len(), c.len()), (11, 11, 11));
        assert_eq!(sizes, [3, 0, 7, 1]);
        assert_eq!((a[9], b[9], c[9]), (2, 6, 1.5));
    }

    #[test]
    #[should_panic(expected = "left unfilled")]
    fn a_piece_left_short_is_caught() {
        build(&[4], |_, piece| piece.push(1u8));
    }
}
