use std::marker::PhantomData;

/// A slice that several threads may write through at once, provided every
/// thread touches a different set of indices. Used for the per-leaf passes
/// (normals, bounds), where each BVH leaf owns its own contiguous vertex range.
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
        debug_assert!(i < self.len);
        unsafe { self.ptr.add(i).write(value) }
    }

    /// # Safety
    /// No other thread may write index `i` while this call runs.
    #[inline]
    pub unsafe fn read(&self, i: usize) -> T
    where
        T: Copy,
    {
        debug_assert!(i < self.len);
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
