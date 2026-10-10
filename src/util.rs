//! Small shared pieces: a slice that parallel passes write through, and a bitset.

use std::marker::PhantomData;

/// A slice that several threads may write through at once, provided every
/// thread touches a different set of indices. What keeps the threads apart
/// differs by user: each BVH leaf owns its own contiguous vertex range (the
/// normals, the curvature, the arrays a stroke writes), each vertex owns its own
/// row of the adjacency lists, and the vertices of one colour phase of a
/// quad-remesh sweep share no edge.
///
/// The index is checked on every call, in release builds too: the callers'
/// ranges come from tables (leaf ranges, neighbour lists) that a bug could
/// leave pointing past the end, and a write there would damage memory
/// instead of stopping the app.
#[derive(Clone, Copy)]
pub struct SharedMut<'a, T> {
    ptr: *mut T,
    len: usize,
    _marker: PhantomData<&'a mut [T]>,
}

unsafe impl<T: Send> Send for SharedMut<'_, T> {}
unsafe impl<T: Send> Sync for SharedMut<'_, T> {}

impl<'a, T> SharedMut<'a, T> {
    pub fn new(slice: &'a mut [T]) -> Self {
        Self { ptr: slice.as_mut_ptr(), len: slice.len(), _marker: PhantomData }
    }

    /// # Safety
    /// No other thread may read or write index `i` while this call runs.
    #[inline]
    pub unsafe fn write(&self, i: usize, value: T) {
        assert!(i < self.len, "SharedMut write at {i}, past the end ({})", self.len);
        unsafe { self.ptr.add(i).write(value) }
    }

    /// # Safety
    /// No other thread may write index `i` while this call runs.
    #[inline]
    pub unsafe fn read(&self, i: usize) -> T
    where
        T: Copy,
    {
        assert!(i < self.len, "SharedMut read at {i}, past the end ({})", self.len);
        unsafe { self.ptr.add(i).read() }
    }
}

/// Growable bitset used to accumulate dirty BVH leaves between GPU uploads.
#[derive(Default, Clone)]
pub struct BitSet {
    words: Vec<u64>,
    any: bool,
}

impl BitSet {
    pub fn with_len(len: usize) -> Self {
        Self { words: vec![0; len.div_ceil(64)], any: false }
    }

    /// Makes room for indices up to `len`; what is set stays set.
    pub fn grow(&mut self, len: usize) {
        if self.words.len() < len.div_ceil(64) {
            self.words.resize(len.div_ceil(64), 0);
        }
    }

    #[inline]
    pub fn set(&mut self, i: usize) {
        self.words[i >> 6] |= 1 << (i & 63);
        self.any = true;
    }

    #[inline]
    pub fn get(&self, i: usize) -> bool {
        self.words[i >> 6] & (1 << (i & 63)) != 0
    }

    pub fn any(&self) -> bool {
        self.any
    }

    pub fn clear(&mut self) {
        if self.any {
            self.words.fill(0);
            self.any = false;
        }
    }

    pub fn set_all(&mut self, len: usize) {
        for i in 0..len {
            self.set(i);
        }
    }

    /// Calls `f` with each set index in ascending order.
    pub fn for_each(&self, mut f: impl FnMut(usize)) {
        if !self.any {
            return;
        }
        for (w, &word) in self.words.iter().enumerate() {
            let mut bits = word;
            while bits != 0 {
                f((w << 6) + bits.trailing_zeros() as usize);
                bits &= bits - 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_mut_reads_and_writes_inside_the_slice() {
        let mut values = [1u32, 2, 3];
        let shared = SharedMut::new(&mut values);
        // SAFETY: one thread, so no other touches these indices.
        unsafe {
            shared.write(2, 9);
            assert_eq!(shared.read(2), 9);
            assert_eq!(shared.read(0), 1);
        }
        assert_eq!(values, [1, 2, 9]);
    }

    #[test]
    #[should_panic(expected = "SharedMut write at 3")]
    fn shared_mut_refuses_a_write_past_the_end() {
        // Room to spare, so that a write wrongly let through lands in memory this test owns.
        let mut values = Vec::with_capacity(16);
        values.extend([0u32; 3]);
        let shared = SharedMut::new(&mut values);
        // SAFETY: one thread; the index is out of range on purpose, and must be refused.
        unsafe { shared.write(3, 1) };
    }

    #[test]
    #[should_panic(expected = "SharedMut read at 7")]
    fn shared_mut_refuses_a_read_past_the_end() {
        let mut values = Vec::with_capacity(16);
        values.extend([0u32; 3]);
        let shared = SharedMut::new(&mut values);
        // SAFETY: as above.
        let _ = unsafe { shared.read(7) };
    }
}
