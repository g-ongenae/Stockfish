//! Port of `src/nnue/nnue_accumulator.{h,cpp}`: the accumulators, their stack
//! along the moves of a search, updated forward and backward, and the refresh
//! cache ("Finny tables").
//!
//! The C++ scalar path walks the accumulator one element at a time and applies
//! every feature to it; here each feature row is applied to the whole
//! accumulator in turn. Both add the same i8 weights to the same i16 lanes
//! with wrapping arithmetic, so the result is the same whatever the order.

use super::feature_transformer::FeatureTransformer;
use super::features::{full_threats, k32h3, pp_3wide, ThreatIndexList};
use super::L1;
use crate::bitboard::*;
use crate::position::Position;
use crate::types::*;

type Acc = [i16; L1];

/// The result of the affine transformation of the input features, per
/// perspective.
#[derive(Clone)]
pub struct Accumulator {
    pub accumulation: [Acc; COLOR_NB],
    pub computed: [bool; COLOR_NB],
}

/// An entry of the refresh cache: the piece-square part of an accumulator
/// (with the biases) for the pieces it was last computed for.
#[derive(Clone)]
pub struct Entry {
    pub accumulation: Acc,
    pub pieces: [Piece; SQUARE_NB],
    pub piece_bb: Bitboard,
}

impl Entry {
    /// An entry for an empty board holds the biases only.
    fn clear(&mut self, biases: &[i16; L1]) {
        self.accumulation = *biases;
        self.pieces = [NO_PIECE; SQUARE_NB];
        self.piece_bb = 0;
    }
}

/// Per-thread refresh cache, one entry per piece-square block, king square
/// and perspective: `entries[block][ksq][perspective]`, flattened.
pub struct AccumulatorCaches {
    entries: Box<[Entry]>,
}

impl AccumulatorCaches {
    pub fn new(network: &super::Network) -> AccumulatorCaches {
        let empty = Entry { accumulation: [0; L1], pieces: [NO_PIECE; SQUARE_NB], piece_bb: 0 };
        let mut caches =
            AccumulatorCaches { entries: vec![empty; k32h3::NUM_BLOCKS * SQUARE_NB * COLOR_NB].into_boxed_slice() };
        caches.clear(network);
        caches
    }

    /// A cache to be cleared with a network before use (`clear()`): for a
    /// worker created before the network is loaded.
    pub fn empty() -> AccumulatorCaches {
        let empty = Entry { accumulation: [0; L1], pieces: [NO_PIECE; SQUARE_NB], piece_bb: 0 };
        AccumulatorCaches { entries: vec![empty; k32h3::NUM_BLOCKS * SQUARE_NB * COLOR_NB].into_boxed_slice() }
    }

    pub fn clear(&mut self, network: &super::Network) {
        for e in self.entries.iter_mut() {
            e.clear(&network.feature_transformer().biases);
        }
    }

    #[inline(always)]
    fn index(block: usize, ksq: Square, perspective: Color) -> usize {
        (block * SQUARE_NB + (ksq as usize & 63)) * COLOR_NB + perspective as usize
    }
}

/// An accumulator of the stack, with what the move leading to it changed and
/// the piece-square blocks before that move.
#[derive(Clone)]
pub struct AccumulatorState {
    pub acc: Accumulator,
    pub dirties: Dirties,
    pub block_before: k32h3::BlockState,
}

impl std::ops::Deref for AccumulatorState {
    type Target = Accumulator;
    fn deref(&self) -> &Accumulator {
        &self.acc
    }
}

pub struct AccumulatorStack {
    accumulators: Box<[AccumulatorState]>,
    size: usize,
}

impl Default for AccumulatorStack {
    fn default() -> Self {
        AccumulatorStack::new()
    }
}

// ---------------------------------------------------------------------------
// Applying feature rows

#[inline(always)]
fn add_row(acc: &mut Acc, row: &[i8; L1]) {
    for (a, &w) in acc.iter_mut().zip(row.iter()) {
        *a = a.wrapping_add(w as i16);
    }
}

#[inline(always)]
fn sub_row(acc: &mut Acc, row: &[i8; L1]) {
    for (a, &w) in acc.iter_mut().zip(row.iter()) {
        *a = a.wrapping_sub(w as i16);
    }
}

#[inline(always)]
fn add_acc(acc: &mut Acc, other: &Acc) {
    for (a, &b) in acc.iter_mut().zip(other.iter()) {
        *a = a.wrapping_add(b);
    }
}

#[inline(always)]
fn sub_acc(acc: &mut Acc, other: &Acc) {
    for (a, &b) in acc.iter_mut().zip(other.iter()) {
        *a = a.wrapping_sub(b);
    }
}

/// A list of weight rows to apply, at most a full refresh: 32 piece-square
/// rows plus 256 threat and pawn pair rows.
type Rows<'w> = crate::misc::ValueList<&'w [i8; L1], 288>;

static ZERO_ROW: [i8; L1] = [0; L1];

fn rows<'w>() -> Rows<'w> {
    Rows::new(&ZERO_ROW)
}

/// Lanes of an accumulator updated at a time, a tile being kept in registers
/// while all the rows are applied to it. Measured on x86-64: 32 lanes when
/// the compiler can vectorize the loops, 8 when it cannot (the Pentium Pro
/// class of CPU this port aims at). Any size gives the same values.
#[cfg(any(target_feature = "sse2", target_feature = "neon", target_feature = "simd128"))]
const TILE: usize = 32;
#[cfg(not(any(target_feature = "sse2", target_feature = "neon", target_feature = "simd128")))]
const TILE: usize = 8;
const _: () = assert!(L1 % TILE == 0);

/// dst = src - Σ subs + Σ adds, a tile at a time, so that the accumulator is
/// read and written once whatever the number of rows (the tiling of C++).
#[inline]
fn apply_rows(dst: &mut Acc, src: &Acc, subs: &[&[i8; L1]], adds: &[&[i8; L1]]) {
    for t in (0..L1).step_by(TILE) {
        let mut tile: [i16; TILE] = src[t..t + TILE].try_into().unwrap();
        for row in subs {
            let r: &[i8; TILE] = row[t..t + TILE].try_into().unwrap();
            for (a, &w) in tile.iter_mut().zip(r) {
                *a = a.wrapping_sub(w as i16);
            }
        }
        for row in adds {
            let r: &[i8; TILE] = row[t..t + TILE].try_into().unwrap();
            for (a, &w) in tile.iter_mut().zip(r) {
                *a = a.wrapping_add(w as i16);
            }
        }
        dst[t..t + TILE].copy_from_slice(&tile);
    }
}

/// The same in place.
#[inline]
fn apply_rows_in_place(acc: &mut Acc, subs: &[&[i8; L1]], adds: &[&[i8; L1]]) {
    let src = *acc;
    apply_rows(acc, &src, subs, adds);
}

#[inline]
fn push_psq_rows<'w>(rows: &mut Rows<'w>, list: &k32h3::IndexList, ft: &'w FeatureTransformer, block: usize) {
    for &i in list.as_slice() {
        rows.push(ft.psq_row(block, i));
    }
}

#[inline]
fn push_threat_rows<'w>(rows: &mut Rows<'w>, list: &ThreatIndexList, ft: &'w FeatureTransformer) {
    for &i in list.as_slice() {
        rows.push(ft.threat_row(i));
    }
}

/// Removes then adds piece-square rows of a block.
#[inline]
fn apply_psq_features(
    acc: &mut Acc,
    removed: &k32h3::IndexList,
    added: &k32h3::IndexList,
    ft: &FeatureTransformer,
    block: usize,
) {
    for &i in removed.as_slice() {
        sub_row(acc, ft.psq_row(block, i));
    }
    for &i in added.as_slice() {
        add_row(acc, ft.psq_row(block, i));
    }
}

#[inline]
fn apply_threat_features<const SIGN: i32>(acc: &mut Acc, list: &ThreatIndexList, ft: &FeatureTransformer) {
    for &i in list.as_slice() {
        if SIGN > 0 {
            add_row(acc, ft.threat_row(i));
        } else {
            sub_row(acc, ft.threat_row(i));
        }
    }
}

/// `apply_combined`: to = from - removed + added, piece-square rows of the
/// block and threat/pawn pair rows.
#[allow(clippy::too_many_arguments)]
fn apply_combined(
    perspective: Color,
    ft: &FeatureTransformer,
    from: &Accumulator,
    to: &mut Accumulator,
    psq_added: &k32h3::IndexList,
    psq_removed: &k32h3::IndexList,
    thr_added: &ThreatIndexList,
    thr_removed: &ThreatIndexList,
    psq_block: usize,
) {
    debug_assert!(psq_removed.len() == 1 || psq_removed.len() == 2);
    debug_assert!(psq_added.len() == 1 || psq_added.len() == 2);

    let (mut subs, mut adds) = (rows(), rows());
    push_psq_rows(&mut subs, psq_removed, ft, psq_block);
    push_threat_rows(&mut subs, thr_removed, ft);
    push_psq_rows(&mut adds, psq_added, ft, psq_block);
    push_threat_rows(&mut adds, thr_added, ft);

    let p = perspective as usize;
    apply_rows(&mut to.accumulation[p], &from.accumulation[p], subs.as_slice(), adds.as_slice());
}

/// Computes the accumulator of `target` from the computed one of the state
/// next to it: forward from the state before (the move of `target`), or
/// backward from the state after (the move of `computed`, undone).
fn update_accumulator_incremental<const FORWARD: bool>(
    perspective: Color,
    ft: &FeatureTransformer,
    ksq: Square,
    psq_block: usize,
    target: &mut AccumulatorState,
    computed: &AccumulatorState,
) {
    debug_assert!(computed.computed[perspective as usize]);
    debug_assert!(!target.computed[perspective as usize]);

    let mut psq_removed = k32h3::IndexList::new(0);
    let mut psq_added = k32h3::IndexList::new(0);
    let mut thr_removed = ThreatIndexList::new(0);
    let mut thr_added = ThreatIndexList::new(0);

    let dirties = if FORWARD { &target.dirties } else { &computed.dirties };

    if FORWARD {
        full_threats::append_changed_indices(
            perspective,
            ksq,
            &dirties.dirty_threats,
            &mut thr_removed,
            &mut thr_added,
        );
        pp_3wide::append_changed_indices(perspective, ksq, &dirties.dirty_pawn_pairs, &mut thr_removed, &mut thr_added);
        k32h3::append_changed_indices(perspective, ksq, &dirties.dirty_piece, &mut psq_removed, &mut psq_added);
    } else {
        full_threats::append_changed_indices(
            perspective,
            ksq,
            &dirties.dirty_threats,
            &mut thr_added,
            &mut thr_removed,
        );
        pp_3wide::append_changed_indices(perspective, ksq, &dirties.dirty_pawn_pairs, &mut thr_added, &mut thr_removed);
        k32h3::append_changed_indices(perspective, ksq, &dirties.dirty_piece, &mut psq_added, &mut psq_removed);
    }

    apply_combined(
        perspective,
        ft,
        &computed.acc,
        &mut target.acc,
        &psq_added,
        &psq_removed,
        &thr_added,
        &thr_removed,
        psq_block,
    );

    target.acc.computed[perspective as usize] = true;
}

/// The squares where two piece arrays differ.
#[inline]
fn get_changed_pieces(old_pieces: &[Piece; SQUARE_NB], new_pieces: &[Piece; SQUARE_NB]) -> Bitboard {
    let mut changed = 0;
    for sq in 0..SQUARE_NB {
        changed |= ((old_pieces[sq] != new_pieces[sq]) as Bitboard) << sq;
    }
    changed
}

/// The piece-square feature indices that differ between two piece arrays,
/// given the bitboards of the pieces before and after.
#[allow(clippy::too_many_arguments)]
#[inline]
fn append_changed_psq_indices(
    perspective: Color,
    ksq: Square,
    old_pieces: &[Piece; SQUARE_NB],
    old_bb: Bitboard,
    new_pieces: &[Piece; SQUARE_NB],
    new_bb: Bitboard,
    removed: &mut k32h3::IndexList,
    added: &mut k32h3::IndexList,
) {
    let changed_bb = get_changed_pieces(old_pieces, new_pieces);
    for sq in Squares(changed_bb & old_bb) {
        removed.push(k32h3::make_index(perspective, sq, old_pieces[sq as usize], ksq));
    }
    for sq in Squares(changed_bb & new_bb) {
        added.push(k32h3::make_index(perspective, sq, new_pieces[sq as usize], ksq));
    }
}

/// Updates the accumulator for a king move that stays on its half of the
/// board (no change of orientation, hence of threat features), from the
/// accumulator before the move and the cache entries of both king squares;
/// also brings the cache entry of the new king square up to date.
fn update_accumulator_hybrid(
    perspective: Color,
    pos: &Position,
    ft: &FeatureTransformer,
    target: &mut AccumulatorState,
    computed: &AccumulatorState,
    cache: &mut AccumulatorCaches,
) {
    let dirty_piece = target.dirties.dirty_piece;

    debug_assert!(dirty_piece.pc == make_piece(perspective, KING));
    debug_assert!(dirty_piece.to != SQ_NONE);
    debug_assert!((dirty_piece.from & 0b100) == (dirty_piece.to & 0b100));
    debug_assert!(computed.computed[perspective as usize]);
    debug_assert!(!target.computed[perspective as usize]);

    let old_ksq = dirty_piece.from;
    let new_ksq = dirty_piece.to;
    debug_assert!(old_ksq != new_ksq);

    let current_pieces = pos.piece_array();
    let mut previous_pieces = *current_pieces;
    let mut previous_piece_bb = pos.pieces();

    debug_assert!(previous_pieces[new_ksq as usize] == dirty_piece.pc);

    if dirty_piece.remove_sq != SQ_NONE {
        debug_assert!(dirty_piece.remove_sq == new_ksq);
        previous_pieces[new_ksq as usize] = dirty_piece.remove_pc;
    } else {
        previous_pieces[new_ksq as usize] = NO_PIECE;
        previous_piece_bb &= !square_bb(new_ksq);
    }

    debug_assert!(previous_pieces[old_ksq as usize] == NO_PIECE);
    previous_pieces[old_ksq as usize] = make_piece(perspective, KING);
    previous_piece_bb |= square_bb(old_ksq);

    let psq_block = k32h3::block(pos, perspective);
    let old_index = AccumulatorCaches::index(psq_block, old_ksq, perspective);
    let new_index = AccumulatorCaches::index(psq_block, new_ksq, perspective);

    // Two distinct entries of the cache, one read and one written
    let (old_entry, new_entry) = if old_index < new_index {
        let (lo, hi) = cache.entries.split_at_mut(new_index);
        (&lo[old_index], &mut hi[0])
    } else {
        let (lo, hi) = cache.entries.split_at_mut(old_index);
        (&hi[0], &mut lo[new_index])
    };

    // "Remove" means removing from the cache entry, "add" adding to it, to
    // get the wanted accumulator.
    let mut old_remove = k32h3::IndexList::new(0);
    let mut old_add = k32h3::IndexList::new(0);
    let mut new_remove = k32h3::IndexList::new(0);
    let mut new_add = k32h3::IndexList::new(0);

    append_changed_psq_indices(
        perspective,
        old_ksq,
        &old_entry.pieces,
        old_entry.piece_bb,
        &previous_pieces,
        previous_piece_bb,
        &mut old_remove,
        &mut old_add,
    );
    append_changed_psq_indices(
        perspective,
        new_ksq,
        &new_entry.pieces,
        new_entry.piece_bb,
        current_pieces,
        pos.pieces(),
        &mut new_remove,
        &mut new_add,
    );

    // Also the pawn pair indices
    let mut thr_removed = ThreatIndexList::new(0);
    let mut thr_added = ThreatIndexList::new(0);
    let dirties = &target.dirties;
    full_threats::append_changed_indices(
        perspective,
        new_ksq,
        &dirties.dirty_threats,
        &mut thr_removed,
        &mut thr_added,
    );
    pp_3wide::append_changed_indices(perspective, new_ksq, &dirties.dirty_pawn_pairs, &mut thr_removed, &mut thr_added);

    let p = perspective as usize;

    apply_psq_features(&mut new_entry.accumulation, &new_remove, &new_add, ft, psq_block);

    let to_acc = &mut target.acc.accumulation[p];
    *to_acc = new_entry.accumulation;
    // Adding the old accumulator adds (most of) the threat and pawn pair
    // weights needed, but also the piece-square weights of the wrong king
    // bucket: remove the cached ones of the old king square...
    add_acc(to_acc, &computed.accumulation[p]);
    sub_acc(to_acc, &old_entry.accumulation);
    // ... then put back what the old entry lacked: its removed piece-square
    // features are added back, its added ones removed.
    apply_psq_features(to_acc, &old_add, &old_remove, ft, psq_block);
    apply_threat_features::<-1>(to_acc, &thr_removed, ft);
    apply_threat_features::<1>(to_acc, &thr_added, ft);

    new_entry.pieces = *current_pieces;
    new_entry.piece_bb = pos.pieces();

    target.acc.computed[p] = true;
}

/// Refreshes an accumulator: the piece-square part from the cache entry of
/// the king square, brought up to date, and the threat and pawn pair part
/// from the active features.
fn update_accumulator_refresh_cache(
    perspective: Color,
    ft: &FeatureTransformer,
    pos: &Position,
    accumulator: &mut AccumulatorState,
    cache: &mut AccumulatorCaches,
) {
    let psq_block = k32h3::block(pos, perspective);
    let ksq = pos.king_square(perspective);
    let entry = &mut cache.entries[AccumulatorCaches::index(psq_block, ksq, perspective)];
    let mut removed = k32h3::IndexList::new(0);
    let mut added = k32h3::IndexList::new(0);

    append_changed_psq_indices(
        perspective,
        ksq,
        &entry.pieces,
        entry.piece_bb,
        pos.piece_array(),
        pos.pieces(),
        &mut removed,
        &mut added,
    );

    entry.piece_bb = pos.pieces();
    entry.pieces = *pos.piece_array();

    let mut active = ThreatIndexList::new(0);
    full_threats::append_active_indices(perspective, pos, &mut active);
    pp_3wide::append_active_indices(perspective, pos, &mut active);

    let p = perspective as usize;
    accumulator.acc.computed[p] = true;

    let (mut subs, mut adds) = (rows(), rows());
    push_psq_rows(&mut subs, &removed, ft, psq_block);
    push_psq_rows(&mut adds, &added, ft, psq_block);
    apply_rows_in_place(&mut entry.accumulation, subs.as_slice(), adds.as_slice());

    let mut active_rows = rows();
    push_threat_rows(&mut active_rows, &active, ft);
    apply_rows(&mut accumulator.acc.accumulation[p], &entry.accumulation, &[], active_rows.as_slice());
}

// ---------------------------------------------------------------------------
// The stack

impl AccumulatorStack {
    pub const MAX_SIZE: usize = MAX_PLY + 1;

    /// Allocated once; pushing and popping do not allocate.
    pub fn new() -> AccumulatorStack {
        let state = AccumulatorState {
            acc: Accumulator { accumulation: [[0; L1]; COLOR_NB], computed: [false; COLOR_NB] },
            dirties: Dirties::default(),
            block_before: [0; COLOR_NB],
        };
        AccumulatorStack { accumulators: vec![state; Self::MAX_SIZE].into_boxed_slice(), size: 1 }
    }

    /// Number of states on the stack, the root one included.
    #[inline(always)]
    pub fn len(&self) -> usize {
        self.size
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        false
    }

    #[inline(always)]
    pub fn latest(&self) -> &AccumulatorState {
        &self.accumulators[self.size - 1]
    }

    #[inline(always)]
    fn mut_latest(&mut self) -> &mut AccumulatorState {
        &mut self.accumulators[self.size - 1]
    }

    pub fn reset(&mut self) {
        let st = &mut self.accumulators[0];
        st.dirties.dirty_piece = DirtyPiece::default();
        st.dirties.dirty_threats.list.clear();
        st.dirties.dirty_pawn_pairs = DirtyPawnPairs::default();
        st.acc.computed = [false; COLOR_NB];
        self.size = 1;
    }

    /// Pushes a state for the move about to be made on `pos`, and returns the
    /// dirties for `do_move` to fill.
    pub fn push(&mut self, pos: &Position) -> &mut Dirties {
        assert!(self.size < Self::MAX_SIZE);
        let st = &mut self.accumulators[self.size];
        st.acc.computed = [false; COLOR_NB];
        st.block_before = k32h3::block_state(pos);
        st.dirties.dirty_threats.list.clear();
        st.dirties.dirty_pawn_pairs = DirtyPawnPairs::default();
        self.size += 1;
        &mut st.dirties
    }

    pub fn pop(&mut self) {
        debug_assert!(self.size > 1);
        self.size -= 1;
    }

    /// Brings the accumulators of the current position up to date.
    pub fn evaluate(&mut self, pos: &Position, ft: &FeatureTransformer, cache: &mut AccumulatorCaches) {
        let last_white = self.find_last_usable_accumulator(WHITE, pos);
        let last_black = self.find_last_usable_accumulator(BLACK, pos);

        if self.accumulators[last_white].computed[WHITE as usize]
            && self.accumulators[last_black].computed[BLACK as usize]
        {
            self.forward_update_incremental_both(pos, ft, last_white, last_black);
        } else {
            self.evaluate_side(WHITE, pos, ft, cache, last_white);
            self.evaluate_side(BLACK, pos, ft, cache, last_black);
        }
    }

    fn evaluate_side(
        &mut self,
        perspective: Color,
        pos: &Position,
        ft: &FeatureTransformer,
        cache: &mut AccumulatorCaches,
        last_usable_accum: usize,
    ) {
        const MIN_PC_COUNT_HYBRID: i32 = 15;
        let p = perspective as usize;

        if self.accumulators[last_usable_accum].computed[p] {
            self.forward_update_incremental(perspective, pos, ft, last_usable_accum);
            return;
        }

        let dirty_piece = self.latest().dirties.dirty_piece;
        if dirty_piece.pc == make_piece(perspective, KING)
            && self.latest().block_before[p] as usize == k32h3::block(pos, perspective)
            && self.accumulators[self.size - 2].computed[p]
            && pos.count_both(ALL_PIECES) >= MIN_PC_COUNT_HYBRID
            && (dirty_piece.from & 0b100) == (dirty_piece.to & 0b100)
            && dirty_piece.add_sq == SQ_NONE
        // excludes castling
        {
            let (below, above) = self.accumulators.split_at_mut(self.size - 1);
            update_accumulator_hybrid(perspective, pos, ft, &mut above[0], &below[self.size - 2], cache);
            return;
        }

        update_accumulator_refresh_cache(perspective, ft, pos, self.mut_latest(), cache);
        self.backward_update_incremental(perspective, pos, ft, last_usable_accum);
    }

    /// The latest accumulator that is computed, or that follows a change
    /// requiring a refresh; all the states above it use the current block.
    fn find_last_usable_accumulator(&self, perspective: Color, pos: &Position) -> usize {
        let psq_block = k32h3::block(pos, perspective);

        for curr_idx in (1..self.size).rev() {
            let st = &self.accumulators[curr_idx];
            if st.computed[perspective as usize] {
                return curr_idx;
            }
            // Threat feature refreshes require a king move across the
            // center, a subset of the piece-square refreshes.
            if k32h3::requires_refresh(
                &st.dirties.dirty_piece,
                perspective,
                st.block_before[perspective as usize] as usize,
                psq_block,
            ) {
                return curr_idx;
            }
        }
        0
    }

    fn forward_update_incremental(
        &mut self,
        perspective: Color,
        pos: &Position,
        ft: &FeatureTransformer,
        begin: usize,
    ) {
        debug_assert!(self.accumulators[begin].computed[perspective as usize]);

        let ksq = pos.king_square(perspective);
        let psq_block = k32h3::block(pos, perspective);

        for next in begin + 1..self.size {
            let (below, above) = self.accumulators.split_at_mut(next);
            update_accumulator_incremental::<true>(perspective, ft, ksq, psq_block, &mut above[0], &below[next - 1]);
        }

        debug_assert!(self.latest().computed[perspective as usize]);
    }

    fn backward_update_incremental(&mut self, perspective: Color, pos: &Position, ft: &FeatureTransformer, end: usize) {
        debug_assert!(end < self.size);
        debug_assert!(self.latest().computed[perspective as usize]);

        let ksq = pos.king_square(perspective);
        let psq_block = k32h3::block(pos, perspective);

        for next in (end..self.size - 1).rev() {
            let (below, above) = self.accumulators.split_at_mut(next + 1);
            update_accumulator_incremental::<false>(perspective, ft, ksq, psq_block, &mut below[next], &above[0]);
        }

        debug_assert!(self.accumulators[end].computed[perspective as usize]);
    }

    /// Catches up the lagging perspective, then updates both along the
    /// common suffix (`update_accumulator_incremental_both` in C++ computes
    /// the same two updates in one pass).
    fn forward_update_incremental_both(
        &mut self,
        pos: &Position,
        ft: &FeatureTransformer,
        white_begin: usize,
        black_begin: usize,
    ) {
        debug_assert!(white_begin < self.size && black_begin < self.size);

        let white_ksq = pos.king_square(WHITE);
        let black_ksq = pos.king_square(BLACK);
        let white_block = k32h3::block(pos, WHITE);
        let black_block = k32h3::block(pos, BLACK);
        let shared_begin = white_begin.max(black_begin);

        for next in white_begin + 1..=shared_begin {
            let (below, above) = self.accumulators.split_at_mut(next);
            update_accumulator_incremental::<true>(WHITE, ft, white_ksq, white_block, &mut above[0], &below[next - 1]);
        }
        for next in black_begin + 1..=shared_begin {
            let (below, above) = self.accumulators.split_at_mut(next);
            update_accumulator_incremental::<true>(BLACK, ft, black_ksq, black_block, &mut above[0], &below[next - 1]);
        }
        for next in shared_begin + 1..self.size {
            let (below, above) = self.accumulators.split_at_mut(next);
            update_accumulator_incremental::<true>(WHITE, ft, white_ksq, white_block, &mut above[0], &below[next - 1]);
            update_accumulator_incremental::<true>(BLACK, ft, black_ksq, black_block, &mut above[0], &below[next - 1]);
        }

        debug_assert!(self.latest().computed[WHITE as usize] && self.latest().computed[BLACK as usize]);
    }
}
