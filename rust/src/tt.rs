//! Port of `src/tt.{h,cpp}`: the transposition table.
//!
//! As in C++, the table is an array of 32-byte clusters, each holding the
//! low 21 bits of the keys of its three entries packed in 64 bits, then the
//! three 8-byte entries (depth, generation/bound/pv byte, move, value and
//! static evaluation). Every field is a relaxed atomic, as the
//! `RelaxedAtomic` members of C++ are, so that the table can be shared by `&`
//! between threads later on (racy by design, as in C++): with one thread they
//! are plain loads and stores. The only change of representation: the 64-bit
//! `keys` word is held as two 32-bit atomics, low half first, since 32-bit
//! PowerPC has no 64-bit atomics; it is read and written whole by one thread,
//! and the value of the word is all that the search and the trace digest see.
//!
//! The digest of the trace (`trace_digest`) is defined over the values of the
//! fields, not over memory, so it does not depend on the byte order.

use std::sync::atomic::{AtomicI16, AtomicU16, AtomicU32, AtomicU8, Ordering::Relaxed};

use crate::types::*;

pub const DEPTH_UNSEARCHED: Depth = -2;
pub const DEPTH_NONE: Depth = -3;

/// `enum Bound`.
pub type Bound = u8;
pub const BOUND_NONE: Bound = 0;
pub const BOUND_UPPER: Bound = 1;
pub const BOUND_LOWER: Bound = 2;
pub const BOUND_EXACT: Bound = BOUND_UPPER | BOUND_LOWER;

const GENERATION_BITS: u8 = 5;
const GENERATION_MASK: u8 = (1 << GENERATION_BITS) - 1;
const BOUND_SHIFT: u8 = GENERATION_BITS;
const BOUND_MASK: u8 = 0b11 << BOUND_SHIFT;
const PV_SHIFT: u8 = BOUND_SHIFT + 2;
const PV_MASK: u8 = 1 << PV_SHIFT;

pub const CLUSTER_SIZE: usize = 3;
const KEY_BITS: u32 = 21;
const KEY_MASK: u64 = (1 << KEY_BITS) - 1;

/// `TTData`: a copy of the data of an entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TTData {
    pub mv: Move,
    pub value: Value,
    pub eval: Value,
    pub depth: Depth,
    pub bound: Bound,
    pub is_pv: bool,
}

/// `TTEntry`, 8 bytes: see the C++ comment in `tt.cpp`. The depth is stored
/// offset by `DEPTH_NONE`, so that 0 means an empty entry.
#[derive(Default)]
struct TTEntry {
    depth8: AtomicU8,
    gen_bound8: AtomicU8,
    move16: AtomicU16,
    value16: AtomicI16,
    eval16: AtomicI16,
}

impl TTEntry {
    #[inline(always)]
    fn read(&self) -> TTData {
        let gb = self.gen_bound8.load(Relaxed);
        TTData {
            mv: Move(self.move16.load(Relaxed)),
            value: self.value16.load(Relaxed) as Value,
            eval: self.eval16.load(Relaxed) as Value,
            depth: DEPTH_NONE + self.depth8.load(Relaxed) as Depth,
            bound: (gb & BOUND_MASK) >> BOUND_SHIFT,
            is_pv: gb & PV_MASK != 0,
        }
    }

    #[inline(always)]
    fn is_occupied(&self) -> bool {
        self.depth8.load(Relaxed) != 0
    }

    /// Generations count like clocks count hours (0 - 1 == 31). C++ computes
    /// in `int` and keeps the low 5 bits, which a wrapping `u8` subtraction
    /// gives too.
    #[inline(always)]
    fn relative_age(&self, curr_generation: u8) -> u8 {
        curr_generation.wrapping_sub(self.gen_bound8.load(Relaxed)) & GENERATION_MASK
    }
}

/// `Cluster`: the packed 21-bit keys of the three entries, then the entries.
#[derive(Default)]
pub struct Cluster {
    keys_lo: AtomicU32,
    keys_hi: AtomicU32,
    entry: [TTEntry; CLUSTER_SIZE],
}

const _: () = assert!(std::mem::size_of::<Cluster>() == 32, "Suboptimal Cluster size");

impl Cluster {
    #[inline(always)]
    fn keys(&self) -> u64 {
        self.keys_lo.load(Relaxed) as u64 | ((self.keys_hi.load(Relaxed) as u64) << 32)
    }

    #[inline(always)]
    fn set_keys(&self, k: u64) {
        self.keys_lo.store(k as u32, Relaxed);
        self.keys_hi.store((k >> 32) as u32, Relaxed);
    }
}

/// `TTWriter`: the entry a probe chose, to write to later. C++ keeps a
/// pointer to the cluster; the port its index, and writes go through the
/// table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TTWriter {
    cluster: usize,
    index: usize,
}

/// What the trace prints of an entry (`TranspositionTable::TraceEntry`).
pub struct TraceEntry {
    pub cluster: usize,
    pub slot: usize,
    pub key21: u32,
    pub data: TTData,
    pub generation: u8,
}

/// `mul_hi64`: the high 64 bits of the 128-bit product, which C++ computes
/// with `__uint128_t` or, without it, from 32-bit halves; both give the exact
/// value, as `u128` does on every target.
#[inline(always)]
pub fn mul_hi64(a: u64, b: u64) -> u64 {
    ((a as u128 * b as u128) >> 64) as u64
}

pub struct TranspositionTable {
    table: Box<[Cluster]>,
    generation8: AtomicU8,
}

impl TranspositionTable {
    /// `resize(mbSize)`: a table of `mb_size` MiB, cleared. The count of
    /// clusters is computed in `usize` as in C++ (`MaxHashMB` keeps it from
    /// overflowing on 32-bit targets).
    pub fn new(mb_size: usize) -> TranspositionTable {
        let cluster_count = mb_size * 1024 * 1024 / std::mem::size_of::<Cluster>();
        let mut table = Vec::new();
        if table.try_reserve_exact(cluster_count).is_err() {
            eprintln!("Failed to allocate {mb_size}MB for transposition table.");
            std::process::exit(1);
        }
        // Zeroed, as the memset of clear() leaves it in C++
        table.extend(std::iter::repeat_with(Cluster::default).take(cluster_count));
        TranspositionTable { table: table.into_boxed_slice(), generation8: AtomicU8::new(0) }
    }

    /// `clear()`: every entry to zero and the generation to 0. C++ shares the
    /// work between the search threads; with one thread it is one loop.
    pub fn clear(&self) {
        self.generation8.store(0, Relaxed);
        for c in self.table.iter() {
            c.set_keys(0);
            for e in &c.entry {
                e.depth8.store(0, Relaxed);
                e.gen_bound8.store(0, Relaxed);
                e.move16.store(0, Relaxed);
                e.value16.store(0, Relaxed);
                e.eval16.store(0, Relaxed);
            }
        }
    }

    pub fn cluster_count(&self) -> usize {
        self.table.len()
    }

    /// `new_search()`: called at the start of each root search, for the
    /// ageing of the entries.
    pub fn new_search(&self) {
        // Don't overflow into the other bits of TTEntry::genBound8
        let g = self.generation8.load(Relaxed).wrapping_add(1) & GENERATION_MASK;
        self.generation8.store(g, Relaxed);
    }

    /// `generation()`: the current age, written into new entries.
    #[inline(always)]
    pub fn generation(&self) -> u8 {
        self.generation8.load(Relaxed)
    }

    /// `hashfull(maxAge)`: the permille of the entries of the first 1000
    /// clusters that are occupied and at most `max_age` generations old.
    pub fn hashfull(&self, max_age: i32) -> i32 {
        let g = self.generation();
        let mut cnt = 0;
        for c in &self.table[..1000] {
            for e in &c.entry {
                cnt += (e.is_occupied() && e.relative_age(g) as i32 <= max_age) as i32;
            }
        }
        cnt / CLUSTER_SIZE as i32
    }

    /// `get_cluster(key)`: the index of the cluster of a key.
    #[inline(always)]
    fn cluster_index(&self, key: Key) -> usize {
        mul_hi64(key, self.table.len() as u64) as usize
    }

    /// `probe(key)`: whether the entry of the key holds data, a copy of it,
    /// and the writer of that entry; without a match, `false`, empty data, and
    /// the writer of the least valuable entry of the cluster (its depth minus
    /// 8 times its relative age).
    #[inline]
    pub fn probe(&self, key: Key) -> (bool, TTData, TTWriter) {
        let ci = self.cluster_index(key);
        let cluster = &self.table[ci];
        let tte = &cluster.entry;
        let key21 = key & KEY_MASK; // Use the low 21 bits as key inside the cluster

        const KEY_LSB: u64 = 1 | (1 << KEY_BITS) | (1 << (2 * KEY_BITS));
        const KEY_MSB: u64 = KEY_LSB << (KEY_BITS - 1);
        let keys = cluster.keys();
        let key_xors = keys ^ (KEY_LSB * key21);
        // A one in the most significant bit of the first 21-bit block matching
        // key21; garbage may follow it.
        let matches = key_xors.wrapping_sub(KEY_LSB) & !key_xors & KEY_MSB;

        if matches != 0 {
            let i = (matches.trailing_zeros() >> 4) as usize - 1;
            return (tte[i].is_occupied(), tte[i].read(), TTWriter { cluster: ci, index: i });
        }

        // Find an entry to be replaced according to the replacement strategy
        let g = self.generation();
        let mut replace = 0;
        let mut replace_value = tte[0].depth8.load(Relaxed) as i32 - 8 * tte[0].relative_age(g) as i32;
        for (i, e) in tte.iter().enumerate().skip(1) {
            let value = e.depth8.load(Relaxed) as i32 - 8 * e.relative_age(g) as i32;
            if replace_value > value {
                replace = i;
                replace_value = value;
            }
        }

        let none = TTData {
            mv: Move::none(),
            value: VALUE_NONE,
            eval: VALUE_NONE,
            depth: DEPTH_NONE,
            bound: BOUND_NONE,
            is_pv: false,
        };
        (false, none, TTWriter { cluster: ci, index: replace })
    }

    /// `TTWriter::write()`: stores a node's data in the entry of the writer,
    /// possibly over another position's. Arguments in the C++ order.
    #[allow(clippy::too_many_arguments)]
    #[inline]
    pub fn write(
        &self,
        w: TTWriter,
        k: Key,
        v: Value,
        pv: bool,
        b: Bound,
        d: Depth,
        m: Move,
        ev: Value,
        curr_generation: u8,
    ) {
        let cluster = &self.table[w.cluster];
        let shift = KEY_BITS as usize * w.index;
        let keys = cluster.keys();
        let key_diff = ((keys >> shift) ^ k) & KEY_MASK;
        let entry = &cluster.entry[w.index];

        // Preserve the old ttmove if we don't have a new one
        if m != Move::none() || key_diff != 0 {
            entry.move16.store(m.0, Relaxed);
        }

        // Overwrite less valuable entries (cheapest checks first)
        if b == BOUND_EXACT
            || key_diff != 0
            || d - DEPTH_NONE + 2 * pv as i32 > entry.depth8.load(Relaxed) as i32 - 4
            || entry.relative_age(curr_generation) != 0
        {
            debug_assert!(d > DEPTH_NONE);
            debug_assert!(d - DEPTH_NONE < 256);
            debug_assert!(curr_generation <= GENERATION_MASK);

            cluster.set_keys(keys ^ (key_diff << shift));
            entry.depth8.store((d - DEPTH_NONE) as u8, Relaxed);
            entry.gen_bound8.store(curr_generation | (b << BOUND_SHIFT) | ((pv as u8) << PV_SHIFT), Relaxed);
            entry.value16.store(v as i16, Relaxed);
            entry.eval16.store(ev as i16, Relaxed);
        }
        // Secondary aging. Important for elementary mate finding.
        else if entry.depth8.load(Relaxed) as i32 + DEPTH_NONE >= 5
            && (entry.gen_bound8.load(Relaxed) & BOUND_MASK) >> BOUND_SHIFT != BOUND_EXACT
        {
            let v16 = entry.value16.load(Relaxed) as Value;

            // Guard against racy underflows, default to "unoccupied"
            if v16.abs() < VALUE_INFINITE && is_decisive(v16) {
                entry.depth8.store((entry.depth8.load(Relaxed) as i32 - 1).max(0) as u8, Relaxed);
            }
        }
    }

    /// `TTWriter::penalize()`: decrements the stored depth by the penalty.
    #[inline]
    pub fn penalize(&self, w: TTWriter, penalty: i32) {
        let entry = &self.table[w.cluster].entry[w.index];
        // Guard against racy underflows, default to "unoccupied"
        entry.depth8.store((entry.depth8.load(Relaxed) as i32 - penalty).max(0) as u8, Relaxed);
    }

    /// For the search trace: the entry of a writer, where it lies.
    pub fn trace_entry(&self, w: TTWriter) -> TraceEntry {
        let cluster = &self.table[w.cluster];
        let e = &cluster.entry[w.index];
        TraceEntry {
            cluster: w.cluster,
            slot: w.index,
            key21: ((cluster.keys() >> (KEY_BITS as usize * w.index)) & KEY_MASK) as u32,
            data: e.read(),
            generation: e.gen_bound8.load(Relaxed) & GENERATION_MASK,
        }
    }

    /// For the search trace: the generation, then each cluster as four 64-bit
    /// words, the packed keys then each entry as `depth8 | genBound8 << 8 |
    /// move16 << 16 | value16 << 32 | eval16 << 48`, folded with 64-bit
    /// FNV-1a. Computed from the values, so the same on every byte order.
    pub fn trace_digest(&self) -> u64 {
        let mut d = crate::searchtrace::TraceDigest::new();
        d.word(self.generation() as u64);
        for c in self.table.iter() {
            d.word(c.keys());
            for e in &c.entry {
                d.word(
                    e.depth8.load(Relaxed) as u64
                        | ((e.gen_bound8.load(Relaxed) as u64) << 8)
                        | ((e.move16.load(Relaxed) as u64) << 16)
                        | ((e.value16.load(Relaxed) as u16 as u64) << 32)
                        | ((e.eval16.load(Relaxed) as u16 as u64) << 48),
                );
            }
        }
        d.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mul_hi64_is_exact() {
        assert_eq!(mul_hi64(u64::MAX, u64::MAX), u64::MAX - 1);
        assert_eq!(mul_hi64(1 << 63, 65536), 32768);
        // 0x9E3779B97F4A7C15 / 2^64 = 0.6180339887..., times 98304 = 60755.3...
        assert_eq!(mul_hi64(0x9E3779B97F4A7C15, 98304), 60755);
        // The C++ fallback without __uint128_t, from 32-bit halves
        let fallback = |a: u64, b: u64| {
            let (a_l, a_h, b_l, b_h) = (a as u32 as u64, a >> 32, b as u32 as u64, b >> 32);
            let c1 = (a_l * b_l) >> 32;
            let c2 = a_h * b_l + c1;
            let c3 = a_l * b_h + (c2 as u32 as u64);
            a_h * b_h + (c2 >> 32) + (c3 >> 32)
        };
        let mut x: u64 = 0x0123_4567_89AB_CDEF;
        for _ in 0..10000 {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let n = (x >> 40) | 1;
            assert_eq!(mul_hi64(x, n), fallback(x, n));
        }
    }

    #[test]
    fn cluster_of_a_key() {
        // 2 MiB: 65,536 clusters, the top 16 bits of the key
        let tt = TranspositionTable::new(2);
        assert_eq!(tt.cluster_count(), 65536);
        assert_eq!(tt.cluster_index(0xABCD_0000_0000_0000), 0xABCD);
        // 3 MiB: 98,304 clusters, not a power of two
        let tt = TranspositionTable::new(3);
        assert_eq!(tt.cluster_count(), 98304);
        assert_eq!(tt.cluster_index(u64::MAX), 98303);
        assert_eq!(tt.cluster_index(1 << 63), 49152);
    }

    #[test]
    fn write_then_probe() {
        let tt = TranspositionTable::new(1);
        let key: Key = 0x1234_5678_9ABC_DEF0;
        let (hit, data, w) = tt.probe(key);
        assert!(!hit);
        assert_eq!(data.depth, DEPTH_NONE);
        assert_eq!(w.index, 0);
        tt.write(w, key, -123, true, BOUND_LOWER, 7, Move(0x1234), 45, tt.generation());
        let (hit, data, w2) = tt.probe(key);
        assert!(hit);
        assert_eq!(w2, w);
        assert_eq!(data, TTData { mv: Move(0x1234), value: -123, eval: 45, depth: 7, bound: BOUND_LOWER, is_pv: true });
        // The packed word of the entry, as the trace digests it
        let e = tt.trace_entry(w);
        assert_eq!(e.key21, (key & KEY_MASK) as u32);
        assert_eq!(e.generation, 0);
        // depth8 = 7 + 3 = 10, genBound8 = 0 | 2 << 5 | 1 << 7 = 0xC0
        let c = &tt.table[w.cluster];
        assert_eq!(c.keys(), key & KEY_MASK);
        assert_eq!(c.entry[0].depth8.load(Relaxed), 10);
        assert_eq!(c.entry[0].gen_bound8.load(Relaxed), 0xC0);
        assert_eq!(c.entry[0].value16.load(Relaxed) as u16, 0xFF85);
    }

    #[test]
    fn generation_wraps_at_32() {
        let tt = TranspositionTable::new(1);
        for _ in 0..33 {
            tt.new_search();
        }
        assert_eq!(tt.generation(), 1);
        tt.clear();
        assert_eq!(tt.generation(), 0);
    }
}
