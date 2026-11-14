//! Port of the parts of `src/misc.h` the position needs.

/// xorshift64star pseudo-random number generator, as in C++ (`PRNG`). It
/// seeds the Zobrist keys and the magic search, so it must give the same
/// sequence: the magics only change the table layout, but the keys are output.
pub struct Prng {
    s: u64,
}

impl Prng {
    pub fn new(seed: u64) -> Prng {
        assert!(seed != 0);
        Prng { s: seed }
    }

    #[inline]
    pub fn rand(&mut self) -> u64 {
        self.s ^= self.s >> 12;
        self.s ^= self.s << 25;
        self.s ^= self.s >> 27;
        self.s.wrapping_mul(2685821657736338717)
    }

    /// Output values only have 1/8th of their bits set on average.
    #[inline]
    pub fn sparse_rand(&mut self) -> u64 {
        self.rand() & self.rand() & self.rand()
    }
}

/// A list of at most N values on the stack, `ValueList` in C++.
#[derive(Clone)]
pub struct ValueList<T: Copy, const N: usize> {
    values: [T; N],
    size: usize,
}

impl<T: Copy, const N: usize> ValueList<T, N> {
    #[inline(always)]
    pub fn new(fill: T) -> Self {
        ValueList { values: [fill; N], size: 0 }
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        self.size
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.size == 0
    }

    #[inline(always)]
    pub fn clear(&mut self) {
        self.size = 0;
    }

    /// Panics on overflow, where C++ only asserts.
    #[inline(always)]
    pub fn push(&mut self, v: T) {
        self.values[self.size] = v;
        self.size += 1;
    }

    /// Pushes the value only if it is below `max` (`push_back_if_lt`).
    #[inline(always)]
    pub fn push_if_lt(&mut self, v: T, max: T)
    where
        T: PartialOrd,
    {
        if v < max {
            self.push(v);
        }
    }

    #[inline(always)]
    pub fn as_slice(&self) -> &[T] {
        &self.values[..self.size]
    }
}
