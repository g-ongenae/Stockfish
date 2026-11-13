//! Port of `src/history.h`: the history tables, read by the move picker and
//! by search, written by search.
//!
//! Every table of the header is here, with the C++ dimensions, value type
//! (`i16` for all of them at this commit) and bounds `D`, laid out in the same
//! row-major order. The tables that C++ declares with `Shared = true`
//! (`PieceToHistory`, hence the continuation histories, the pawn history and
//! the unified correction history) hold atomics accessed with relaxed loads
//! and stores, as `RelaxedAtomic` does, so that they can be shared between
//! threads by `&` later on; the others are plain `i16`, owned by one thread.
//! On every target of the port a relaxed 16-bit load or store is an ordinary
//! one.
//!
//! The update `entry << bonus` of C++ is `update(bonus)`, with the same
//! arithmetic: `int` intermediates, the bonus clamped to `[-D, D]`, a division
//! truncating toward zero, and the conversion back to `i16`. Like C++,
//! `update()` on a shared entry is a load followed by a store, not an atomic
//! read-modify-write: concurrent updates may lose one another, by design.
//!
//! Sizes (C++ is the same): see the README.

use std::sync::atomic::{AtomicI16, Ordering};

use crate::position::Position;
use crate::types::*;

pub const PAWN_HISTORY_BASE_SIZE: usize = 8192; // has to be a power of 2
pub const UINT_16_HISTORY_SIZE: usize = u16::MAX as usize + 1;
pub const CORRHIST_BASE_SIZE: usize = UINT_16_HISTORY_SIZE;
pub const CORRECTION_HISTORY_LIMIT: i32 = 1024;
pub const LOW_PLY_HISTORY_SIZE: usize = 5;

const _: () = assert!(PAWN_HISTORY_BASE_SIZE.is_power_of_two());
const _: () = assert!(CORRHIST_BASE_SIZE.is_power_of_two());

/// The `StatsEntry<T, D>::operator<<` of C++ on the value `val`: the bonus is
/// clamped to `[-D, D]`, and the result computed in `int` then converted to
/// `T` (`i16`). It stays within `[-D, D]` when `val` does.
#[inline(always)]
fn updated<const D: i32>(val: i16, bonus: i32) -> i16 {
    let clamped_bonus = bonus.clamp(-D, D);
    let val = val as i32;
    let v = (val + clamped_bonus - val * clamped_bonus.abs() / D) as i16;
    debug_assert!((v as i32).abs() <= D);
    v
}

/// Checks of the C++ `static_assert`s of `StatsEntry` (`T` is `i16`).
struct Bound<const D: i32>;
impl<const D: i32> Bound<D> {
    const OK: () = assert!(D > 0 && D <= i16::MAX as i32 && D <= i32::MAX / D, "D can lead to overflows");
}

/// `StatsEntry<i16, D>`: one entry of a table owned by a thread.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
#[repr(transparent)]
pub struct StatsEntry<const D: i32>(i16);

impl<const D: i32> StatsEntry<D> {
    #[inline(always)]
    pub const fn new(v: i16) -> Self {
        #[allow(clippy::let_unit_value)] // forces the evaluation of the check
        let _ = Bound::<D>::OK;
        StatsEntry(v)
    }

    /// `operator T()`.
    #[inline(always)]
    pub fn get(self) -> i16 {
        self.0
    }

    /// `operator=(T)`.
    #[inline(always)]
    pub fn set(&mut self, v: i16) {
        self.0 = v;
    }

    /// `operator<<(int bonus)`.
    #[inline(always)]
    pub fn update(&mut self, bonus: i32) {
        self.0 = updated::<D>(self.0, bonus);
    }
}

/// `StatsEntry<i16, D, true>`: one entry of a table that C++ shares between
/// the threads of a NUMA node, a `RelaxedAtomic<i16>`.
#[derive(Default, Debug)]
#[repr(transparent)]
pub struct AtomicStatsEntry<const D: i32>(AtomicI16);

impl<const D: i32> AtomicStatsEntry<D> {
    #[inline(always)]
    pub const fn new(v: i16) -> Self {
        #[allow(clippy::let_unit_value)] // forces the evaluation of the check
        let _ = Bound::<D>::OK;
        AtomicStatsEntry(AtomicI16::new(v))
    }

    /// `operator T()`: a relaxed load.
    #[inline(always)]
    pub fn get(&self) -> i16 {
        self.0.load(Ordering::Relaxed)
    }

    /// `operator=(T)`: a relaxed store.
    #[inline(always)]
    pub fn set(&self, v: i16) {
        self.0.store(v, Ordering::Relaxed)
    }

    /// `operator<<(int bonus)`: a relaxed load, then a relaxed store, as in
    /// C++ (not an atomic read-modify-write).
    #[inline(always)]
    pub fn update(&self, bonus: i32) {
        self.set(updated::<D>(self.get(), bonus));
    }
}

/// `fill(v)` of a C++ `MultiArray` (`std::array::fill` at every level): sets
/// every entry of a table, or of a part of one, to `v`.
pub trait Fill {
    fn fill_all(&mut self, v: i16);
}

impl<const D: i32> Fill for StatsEntry<D> {
    #[inline]
    fn fill_all(&mut self, v: i16) {
        self.set(v);
    }
}

impl<const D: i32> Fill for AtomicStatsEntry<D> {
    #[inline]
    fn fill_all(&mut self, v: i16) {
        *self.0.get_mut() = v;
    }
}

impl<T: Fill, const N: usize> Fill for [T; N] {
    #[inline]
    fn fill_all(&mut self, v: i16) {
        for e in self.iter_mut() {
            e.fill_all(v);
        }
    }
}

impl<T: Fill> Fill for [T] {
    #[inline]
    fn fill_all(&mut self, v: i16) {
        for e in self.iter_mut() {
            e.fill_all(v);
        }
    }
}

/// `fill(v)` and `operator=(T)` through a shared reference, on the shared
/// tables, with relaxed stores (as C++, whose workers each clear their part
/// of a table shared with the other threads of their NUMA node).
pub trait FillShared {
    fn fill_shared(&self, v: i16);
}

impl<const D: i32> FillShared for AtomicStatsEntry<D> {
    #[inline]
    fn fill_shared(&self, v: i16) {
        self.set(v);
    }
}

impl<T: FillShared, const N: usize> FillShared for [T; N] {
    #[inline]
    fn fill_shared(&self, v: i16) {
        for e in self.iter() {
            e.fill_shared(v);
        }
    }
}

impl<T: FillShared> FillShared for [T] {
    #[inline]
    fn fill_shared(&self, v: i16) {
        for e in self.iter() {
            e.fill_shared(v);
        }
    }
}

/// `StatsType`, the second index of the continuation history block.
pub type StatsType = usize;
pub const NO_CAPTURES: StatsType = 0;
pub const CAPTURES: StatsType = 1;

/// `ButterflyHistory`: `[COLOR_NB][UINT_16_HISTORY_SIZE]`, indexed by the
/// side to move and `Move::raw()`.
pub type ButterflyHistory = [[StatsEntry<7183>; UINT_16_HISTORY_SIZE]; COLOR_NB];

/// `LowPlyHistory`: `[LOW_PLY_HISTORY_SIZE][UINT_16_HISTORY_SIZE]`, indexed
/// by the ply and `Move::raw()`.
pub type LowPlyHistory = [[StatsEntry<7183>; UINT_16_HISTORY_SIZE]; LOW_PLY_HISTORY_SIZE];

/// `CapturePieceToHistory`: `[PIECE_NB][SQUARE_NB][PIECE_TYPE_NB]`, indexed
/// by the moved piece, the destination and the captured piece type.
pub type CapturePieceToHistory = [[[StatsEntry<10692>; PIECE_TYPE_NB]; SQUARE_NB]; PIECE_NB];

/// `PieceToHistory`: `[PIECE_NB][SQUARE_NB]`, shared.
pub type PieceToHistory = [[AtomicStatsEntry<30000>; SQUARE_NB]; PIECE_NB];

/// One entry of the `PawnHistory`: `[PIECE_NB][SQUARE_NB]`, shared.
pub type PawnHistoryEntry = [[AtomicStatsEntry<8192>; SQUARE_NB]; PIECE_NB];

/// `CorrectionHistory<PieceTo>`: `[PIECE_NB][SQUARE_NB]`, owned by a thread.
pub type PieceToCorrectionHistory = [[StatsEntry<CORRECTION_HISTORY_LIMIT>; SQUARE_NB]; PIECE_NB];

/// `TTMoveHistory`: a single entry.
pub type TTMoveHistory = StatsEntry<8192>;

/// `CorrectionBundle<i16, CORRECTION_HISTORY_LIMIT>`: four shared entries.
#[derive(Default, Debug)]
pub struct CorrectionBundle {
    pub pawn: AtomicStatsEntry<CORRECTION_HISTORY_LIMIT>,
    pub minor: AtomicStatsEntry<CORRECTION_HISTORY_LIMIT>,
    pub non_pawn_white: AtomicStatsEntry<CORRECTION_HISTORY_LIMIT>,
    pub non_pawn_black: AtomicStatsEntry<CORRECTION_HISTORY_LIMIT>,
}

impl FillShared for CorrectionBundle {
    /// `operator=(T val)`: sets the four entries.
    #[inline]
    fn fill_shared(&self, v: i16) {
        self.pawn.set(v);
        self.minor.set(v);
        self.non_pawn_white.set(v);
        self.non_pawn_black.set(v);
    }
}

impl Fill for CorrectionBundle {
    /// `operator=(T val)`: sets the four entries.
    #[inline]
    fn fill_all(&mut self, v: i16) {
        self.pawn.fill_all(v);
        self.minor.fill_all(v);
        self.non_pawn_white.fill_all(v);
        self.non_pawn_black.fill_all(v);
    }
}

/// A boxed table of `N` rows equal to `row`. Only one row is ever on the
/// stack: the largest, of `ButterflyHistory` and `LowPlyHistory`, is 128 KiB.
pub fn new_table<T: Copy, const N: usize>(row: T) -> Box<[T; N]> {
    vec![row; N].into_boxed_slice().try_into().unwrap_or_else(|_| unreachable!())
}

pub fn new_butterfly_history() -> Box<ButterflyHistory> {
    new_table([StatsEntry::new(0); UINT_16_HISTORY_SIZE])
}

pub fn new_low_ply_history() -> Box<LowPlyHistory> {
    new_table([StatsEntry::new(0); UINT_16_HISTORY_SIZE])
}

pub fn new_capture_piece_to_history() -> Box<CapturePieceToHistory> {
    new_table([[StatsEntry::new(0); PIECE_TYPE_NB]; SQUARE_NB])
}

pub fn new_piece_to_history() -> Box<PieceToHistory> {
    Box::new(atomic_piece_to())
}

/// A `[PIECE_NB][SQUARE_NB]` table of shared entries, all 0.
fn atomic_piece_to<const D: i32>() -> [[AtomicStatsEntry<D>; SQUARE_NB]; PIECE_NB] {
    std::array::from_fn(|_| std::array::from_fn(|_| AtomicStatsEntry::new(0)))
}

/// `DynStats<T, SizeMultiplier>`: a table of `size * SizeMultiplier` entries
/// of type `T`, allocated once, for the shared histories whose size grows
/// with the number of threads.
pub struct DynStats<T> {
    data: Box<[T]>,
}

impl<T> DynStats<T> {
    fn new(size: usize, entry: impl FnMut(usize) -> T) -> Self {
        DynStats { data: (0..size).map(entry).collect::<Vec<T>>().into_boxed_slice() }
    }
}

impl<T: FillShared> DynStats<T> {
    /// `clear_range(value, threadIdx, numaTotal)`: thread `thread_idx` of
    /// `numa_total` sets its share of the entries to `value`. The bounds are
    /// computed in 64 bits as in C++. Through a shared reference, with
    /// relaxed stores, so that each thread can clear its share of a table
    /// shared between threads (`Search::Worker::clear`).
    pub fn clear_range(&self, value: i16, thread_idx: usize, numa_total: usize) {
        let size = self.data.len();
        let mut start = (thread_idx as u64 * size as u64 / numa_total as u64) as usize;
        debug_assert!(start < size);
        let end = if thread_idx + 1 == numa_total {
            size
        } else {
            ((thread_idx + 1) as u64 * size as u64 / numa_total as u64) as usize
        };

        while start < end {
            self.data[start].fill_shared(value);
            start += 1;
        }
    }
}

impl<T> DynStats<T> {
    pub fn get_size(&self) -> usize {
        self.data.len()
    }
}

impl<T> std::ops::Index<usize> for DynStats<T> {
    type Output = T;
    #[inline(always)]
    fn index(&self, i: usize) -> &T {
        &self.data[i]
    }
}

impl<T> std::ops::IndexMut<usize> for DynStats<T> {
    #[inline(always)]
    fn index_mut(&mut self, i: usize) -> &mut T {
        &mut self.data[i]
    }
}

/// `PawnHistory`: `PAWN_HISTORY_BASE_SIZE` entries per thread, indexed by the
/// pawn key.
pub type PawnHistory = DynStats<PawnHistoryEntry>;

/// `UnifiedCorrectionHistory`: `CORRHIST_BASE_SIZE` entries per thread, each
/// `[COLOR_NB]` bundles.
pub type UnifiedCorrectionHistory = DynStats<[CorrectionBundle; COLOR_NB]>;

/// `ContinuationHistory`: `[PIECE_NB][SQUARE_NB]` of `PieceToHistory`, one
/// per previous move.
pub const CONTINUATION_HISTORY_SIZE: usize = PIECE_NB * SQUARE_NB;

/// `ContinuationHistoryBlock`: `ContinuationHistory table[2][2]`, indexed by
/// in check, `StatsType`, piece and square. C++ points into it from the
/// search stack (`ss->continuationHistory`); the port gives each of its
/// `PieceToHistory` a flat index, `index(in_check, capture, pc, to)`, which
/// a search stack can hold instead of a pointer. The layout is that of C++.
pub struct ContinuationHistoryBlock {
    table: Box<[PieceToHistory]>,
}

impl ContinuationHistoryBlock {
    pub const LEN: usize = 2 * 2 * CONTINUATION_HISTORY_SIZE;

    fn new() -> Self {
        ContinuationHistoryBlock { table: (0..Self::LEN).map(|_| atomic_piece_to()).collect() }
    }

    /// The flat index of `table[in_check][capture][pc][to]`.
    #[inline(always)]
    pub fn index(in_check: bool, capture: StatsType, pc: Piece, to: Square) -> usize {
        debug_assert!(capture < 2 && (pc as usize) < PIECE_NB && (to as usize) < SQUARE_NB);
        ((in_check as usize * 2 + capture) * PIECE_NB + pc as usize) * SQUARE_NB + to as usize
    }

    /// `table[in_check][capture][pc][to]`.
    #[inline(always)]
    pub fn get(&self, in_check: bool, capture: StatsType, pc: Piece, to: Square) -> &PieceToHistory {
        &self.table[Self::index(in_check, capture, pc, to)]
    }

    /// The `PieceToHistory` of a flat index.
    #[inline(always)]
    pub fn at(&self, index: usize) -> &PieceToHistory {
        &self.table[index]
    }

    /// `table[in_check][capture]`: the `ContinuationHistory` of one kind of
    /// move, as a slice of `PIECE_NB * SQUARE_NB` tables.
    pub fn continuation_history_mut(&mut self, in_check: bool, capture: StatsType) -> &mut [PieceToHistory] {
        let start = Self::index(in_check, capture, 0, 0);
        &mut self.table[start..start + CONTINUATION_HISTORY_SIZE]
    }

    /// Every table, in the order of the C++ layout.
    pub fn tables(&self) -> &[PieceToHistory] {
        &self.table
    }

    /// Every entry of every table set to `v`, through a shared reference
    /// (relaxed stores), as thread 0 of each NUMA node does in C++.
    pub fn fill_shared(&self, v: i16) {
        self.table.fill_shared(v);
    }
}

/// `CorrectionHistory<Continuation>`: `[PIECE_NB][SQUARE_NB]` of
/// `CorrectionHistory<PieceTo>`, owned by a thread, with flat indices as for
/// the continuation history (C++ points into it from the search stack).
pub struct ContinuationCorrectionHistory {
    table: Box<[PieceToCorrectionHistory]>,
}

impl ContinuationCorrectionHistory {
    pub fn new() -> Self {
        ContinuationCorrectionHistory {
            table: vec![[[StatsEntry::new(0); SQUARE_NB]; PIECE_NB]; CONTINUATION_HISTORY_SIZE].into_boxed_slice(),
        }
    }

    /// The flat index of `[pc][to]`.
    #[inline(always)]
    pub fn index(pc: Piece, to: Square) -> usize {
        debug_assert!((pc as usize) < PIECE_NB && (to as usize) < SQUARE_NB);
        pc as usize * SQUARE_NB + to as usize
    }

    #[inline(always)]
    pub fn at(&self, index: usize) -> &PieceToCorrectionHistory {
        &self.table[index]
    }

    #[inline(always)]
    pub fn at_mut(&mut self, index: usize) -> &mut PieceToCorrectionHistory {
        &mut self.table[index]
    }

    pub fn tables(&self) -> &[PieceToCorrectionHistory] {
        &self.table
    }

    pub fn tables_mut(&mut self) -> &mut [PieceToCorrectionHistory] {
        &mut self.table
    }
}

impl Default for ContinuationCorrectionHistory {
    fn default() -> Self {
        Self::new()
    }
}

/// `SharedHistories`: the histories shared between the threads of a NUMA
/// node in C++ (the correction, continuation and pawn histories). Every
/// entry is atomic, so a later layer can share one by `&` (in an `Arc`)
/// between threads; for now there is one per search.
pub struct SharedHistories {
    pub correction_history: UnifiedCorrectionHistory,
    pub continuation_history_block: ContinuationHistoryBlock,
    pub pawn_history: PawnHistory,
    size_minus1: usize,
    pawn_hist_size_minus1: usize,
}

impl SharedHistories {
    /// `SharedHistories(threadCount)`: `thread_count` must be a power of two.
    /// All the entries are 0 (C++ leaves them to `Search::Worker::clear()`).
    pub fn new(thread_count: usize) -> Self {
        assert!(thread_count.is_power_of_two());
        let correction_history =
            DynStats::new(thread_count * CORRHIST_BASE_SIZE, |_| std::array::from_fn(|_| CorrectionBundle::default()));
        let pawn_history = DynStats::new(thread_count * PAWN_HISTORY_BASE_SIZE, |_| atomic_piece_to());
        let size_minus1 = correction_history.get_size() - 1;
        let pawn_hist_size_minus1 = pawn_history.get_size() - 1;
        SharedHistories {
            correction_history,
            continuation_history_block: ContinuationHistoryBlock::new(),
            pawn_history,
            size_minus1,
            pawn_hist_size_minus1,
        }
    }

    /// `get_size()`: the number of entries of the correction history.
    pub fn get_size(&self) -> usize {
        self.size_minus1 + 1
    }

    /// `key & sizeMinus1`: as in C++, the mask is widened to the 64 bits of
    /// the key, and the result (below the size) narrowed to an index.
    #[inline(always)]
    fn masked(key: Key, mask: usize) -> usize {
        (key & mask as u64) as usize
    }

    #[inline(always)]
    pub fn pawn_entry(&self, pos: &Position) -> &PawnHistoryEntry {
        &self.pawn_history[Self::masked(pos.pawn_key(), self.pawn_hist_size_minus1)]
    }

    #[inline(always)]
    pub fn pawn_correction_entry(&self, pos: &Position) -> &[CorrectionBundle; COLOR_NB] {
        &self.correction_history[Self::masked(pos.pawn_key(), self.size_minus1)]
    }

    #[inline(always)]
    pub fn minor_piece_correction_entry(&self, pos: &Position) -> &[CorrectionBundle; COLOR_NB] {
        &self.correction_history[Self::masked(pos.minor_piece_key(), self.size_minus1)]
    }

    #[inline(always)]
    pub fn nonpawn_correction_entry<const C: Color>(&self, pos: &Position) -> &[CorrectionBundle; COLOR_NB] {
        &self.correction_history[Self::masked(pos.non_pawn_key(C), self.size_minus1)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_matches_the_formula() {
        // val + clamp(bonus) - val * |clamp(bonus)| / D, truncating
        let mut e = StatsEntry::<7183>::new(0);
        e.update(100);
        assert_eq!(e.get(), 100);
        e.update(100_000); // clamped to 7183: 100 + 7183 - 100 * 7183 / 7183
        assert_eq!(e.get(), 7183);
        e.update(-1);
        assert_eq!(e.get(), 7183 - 1 - 7183 / 7183);
        let mut e = StatsEntry::<7183>::new(-5);
        e.update(-7);
        // -5 - 7 - (-5 * 7) / 7183 = -12 - 0 (-35 / 7183 truncates to 0)
        assert_eq!(e.get(), -12);
        let a = AtomicStatsEntry::<30000>::new(-29_999);
        a.update(-i32::MAX);
        assert_eq!(a.get(), (-29_999 - 30000 - (-29_999 * 30000) / 30000) as i16);
    }

    #[test]
    fn continuation_index_is_row_major() {
        assert_eq!(ContinuationHistoryBlock::index(false, NO_CAPTURES, 0, 0), 0);
        assert_eq!(ContinuationHistoryBlock::index(false, NO_CAPTURES, 0, 1), 1);
        assert_eq!(ContinuationHistoryBlock::index(false, NO_CAPTURES, 1, 0), 64);
        assert_eq!(ContinuationHistoryBlock::index(false, CAPTURES, 0, 0), 1024);
        assert_eq!(ContinuationHistoryBlock::index(true, NO_CAPTURES, 0, 0), 2048);
        assert_eq!(ContinuationHistoryBlock::index(true, CAPTURES, 15, 63), ContinuationHistoryBlock::LEN - 1);
    }
}
