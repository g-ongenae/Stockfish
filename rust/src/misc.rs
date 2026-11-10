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
