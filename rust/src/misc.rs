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

// ---------------------------------------------------------------------------
// The counters of the search threads

/// `RelaxedAtomic<u64>` for a counter that one thread increments (its
/// owner) while the others read it: the `nodes` and `tbHits` of a worker.
/// As in C++, `++` is a relaxed load and a relaxed store, not a
/// read-modify-write, so with one thread it costs a plain increment.
///
/// On 64-bit targets it is an `AtomicU64`. On 32-bit targets a 64-bit
/// atomic store is a locked `cmpxchg8b` loop (i586) or an
/// `ldrexd`/`strexd` loop (ARMv7), and 32-bit PowerPC has none at all, so
/// the counter is two `AtomicU32` halves: the owner increments the low half
/// with a plain store, and only when it wraps (every 2^32 increments) does
/// it update the high half, inside a sequence lock that a reader retries
/// on, so that no reader ever sees a torn value (Boehm, "Can seqlocks get
/// along with programming language memory models?", 2012, the fence-based
/// form). The feature `counter32` selects that form on any target, to test
/// it natively.
#[cfg(not(any(feature = "counter32", target_pointer_width = "32")))]
pub struct RelaxedCounter(std::sync::atomic::AtomicU64);

#[cfg(not(any(feature = "counter32", target_pointer_width = "32")))]
impl RelaxedCounter {
    pub const fn new() -> Self {
        RelaxedCounter(std::sync::atomic::AtomicU64::new(0))
    }

    /// `++counter`, by the owner only.
    #[inline(always)]
    pub fn increment(&self) {
        use std::sync::atomic::Ordering::Relaxed;
        self.0.store(self.0.load(Relaxed).wrapping_add(1), Relaxed);
    }

    /// The value, read by the owner.
    #[inline(always)]
    pub fn get_own(&self) -> u64 {
        self.0.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The value, read by any thread.
    #[inline]
    pub fn get(&self) -> u64 {
        self.0.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Sets the value while no thread increments it (between searches).
    pub fn set(&self, v: u64) {
        self.0.store(v, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(any(feature = "counter32", target_pointer_width = "32"))]
pub struct RelaxedCounter {
    lo: std::sync::atomic::AtomicU32,
    hi: std::sync::atomic::AtomicU32,
    /// Odd while the owner updates the two halves together
    seq: std::sync::atomic::AtomicU32,
}

#[cfg(any(feature = "counter32", target_pointer_width = "32"))]
impl RelaxedCounter {
    pub const fn new() -> Self {
        use std::sync::atomic::AtomicU32;
        RelaxedCounter { lo: AtomicU32::new(0), hi: AtomicU32::new(0), seq: AtomicU32::new(0) }
    }

    /// `++counter`, by the owner only.
    #[inline(always)]
    pub fn increment(&self) {
        use std::sync::atomic::Ordering::Relaxed;
        let lo = self.lo.load(Relaxed).wrapping_add(1);
        if lo != 0 {
            self.lo.store(lo, Relaxed);
        } else {
            self.carry();
        }
    }

    /// The low half wraps: both halves change, inside the sequence lock.
    #[cold]
    #[inline(never)]
    fn carry(&self) {
        use std::sync::atomic::{fence, Ordering::*};
        let s = self.seq.load(Relaxed);
        self.seq.store(s.wrapping_add(1), Relaxed);
        fence(Release);
        self.hi.store(self.hi.load(Relaxed).wrapping_add(1), Relaxed);
        self.lo.store(0, Relaxed);
        self.seq.store(s.wrapping_add(2), Release);
    }

    /// The value, read by the owner: no other thread writes it.
    #[inline(always)]
    pub fn get_own(&self) -> u64 {
        use std::sync::atomic::Ordering::Relaxed;
        (self.hi.load(Relaxed) as u64) << 32 | self.lo.load(Relaxed) as u64
    }

    /// The value, read by any thread: retried while the owner carries into
    /// the high half, so never torn.
    #[inline]
    pub fn get(&self) -> u64 {
        use std::sync::atomic::{fence, Ordering::*};
        loop {
            let s1 = self.seq.load(Acquire);
            let hi = self.hi.load(Relaxed);
            let lo = self.lo.load(Relaxed);
            fence(Acquire);
            let s2 = self.seq.load(Relaxed);
            if s1 == s2 && s1 & 1 == 0 {
                return (hi as u64) << 32 | lo as u64;
            }
            std::hint::spin_loop();
        }
    }

    /// Sets the value while no thread increments it (between searches),
    /// inside the sequence lock, so that a reader never sees it torn.
    pub fn set(&self, v: u64) {
        use std::sync::atomic::{fence, Ordering::*};
        let s = self.seq.load(Relaxed);
        self.seq.store(s.wrapping_add(1), Relaxed);
        fence(Release);
        self.hi.store((v >> 32) as u32, Relaxed);
        self.lo.store(v as u32, Relaxed);
        self.seq.store(s.wrapping_add(2), Release);
    }
}

impl Default for RelaxedCounter {
    fn default() -> Self {
        Self::new()
    }
}

/// `bestMoveChanges` of a worker: incremented by its owner, read and reset
/// by the main thread at the end of its iterations, racing with the owner as
/// in C++ (a reset can be lost, an increment can be lost). Two writers, so
/// no sequence lock: on 32-bit targets a plain `AtomicU32`. It cannot wrap
/// there: it grows by at most one per root move searched, at most 218 per
/// search of the root, a few tens of searches per iteration (the aspiration
/// window doubles at each), 245 iterations: below 2^32.
#[cfg(not(any(feature = "counter32", target_pointer_width = "32")))]
pub type BestMoveChanges = std::sync::atomic::AtomicU64;
#[cfg(any(feature = "counter32", target_pointer_width = "32"))]
pub type BestMoveChanges = std::sync::atomic::AtomicU32;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_counts_across_the_32_bit_boundary() {
        let c = RelaxedCounter::new();
        c.set((1 << 32) - 3);
        for i in 0..6u64 {
            assert_eq!(c.get(), (1 << 32) - 3 + i);
            assert_eq!(c.get_own(), c.get());
            c.increment();
        }
        c.set(u64::MAX);
        c.increment();
        assert_eq!(c.get(), 0);
    }

    #[test]
    fn counter_is_never_torn() {
        // One writer crossing 32-bit boundaries, readers that must only see
        // values that increase and that lie in the ranges written
        use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
        use std::sync::Arc;
        const HALF: u64 = 500_000;
        let c = Arc::new(RelaxedCounter::new());
        let done = Arc::new(AtomicBool::new(false));
        let readers: Vec<_> = (0..2)
            .map(|_| {
                let (c, done) = (c.clone(), done.clone());
                std::thread::spawn(move || {
                    let mut last = 0;
                    let mut reads = 0u64;
                    while !done.load(Relaxed) {
                        let v = c.get();
                        let near = (v + HALF) >> 32 << 32;
                        assert!(v >= last, "decreasing: {v:#x} after {last:#x}");
                        assert!(v == 0 || (v + HALF >= near && v <= near + HALF), "torn: {v:#x}");
                        last = v;
                        reads += 1;
                    }
                    reads
                })
            })
            .collect();
        for k in 1..=4u64 {
            c.set((k << 32) - HALF);
            for _ in 0..2 * HALF {
                c.increment();
            }
            assert_eq!(c.get(), (k << 32) + HALF);
        }
        done.store(true, Relaxed);
        for r in readers {
            assert!(r.join().unwrap() > 0);
        }
    }
}
