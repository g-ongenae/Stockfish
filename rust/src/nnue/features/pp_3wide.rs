//! Port of `src/nnue/features/pp_3wide.{h,cpp}`: feature PP_3Wide, one feature
//! per pair of pawns on the same or adjacent files, oriented by the king.

use super::ThreatIndexList;
use crate::bitboard::*;
use crate::nnue::{IndexType, ORIENT_TBL};
use crate::position::Position;
use crate::types::*;

pub const HASH_VALUE: u32 = 0x5c80_fa69;

pub const PAWN_IDS: IndexType = COLOR_NB as IndexType * 48;
pub const DIMENSIONS: IndexType = PAWN_IDS * (PAWN_IDS - 1) / 2;

/// The pawn pair features follow the threat features in the weights, so this
/// must equal full_threats::DIMENSIONS (see feature_transformer.rs).
pub const INDEX_BASE: IndexType = 59808;
const _: () = assert!(INDEX_BASE == super::full_threats::DIMENSIONS);

#[inline(always)]
fn make_pawn_id(color: Color, square: u8) -> IndexType {
    debug_assert!((8..=55).contains(&square));
    48 * color as IndexType + square as IndexType - 8
}

#[inline(always)]
fn pp_idx(a: IndexType, b: IndexType) -> IndexType {
    let hi = a.max(b);
    let lo = a.min(b);
    hi * (hi - 1) / 2 + lo + INDEX_BASE
}

/// The squares that can hold a pawn forming a pair with a pawn on s: its own
/// file and the adjacent ones, on ranks 2 to 7, except s (`pawn_pair_bb`).
static PAWN_PAIR_BB: [Bitboard; SQUARE_NB] = {
    let mut result = [0; SQUARE_NB];
    let mut s = 0;
    while s < SQUARE_NB {
        let file = file_bb_of(s as Square);
        let files = file | shift(file, EAST) | shift(file, WEST);
        result[s] = files & !(RANK_1_BB | RANK_8_BB) & !square_bb(s as Square);
        s += 1;
    }
    result
};

/// The features of the pairs between the pawns of `updated` and the other
/// pawns (each pair once), for one or both perspectives: with two, `ksq[i]`
/// and `out[i]` are those of the perspective `perspective ^ i`.
fn append_indices_impl<const N: usize>(
    perspective: Color,
    ksq: [Square; N],
    updated: Bitboard,
    pawns_w: Bitboard,
    pawns_b: Bitboard,
    out: [&mut ThreatIndexList; N],
) {
    if updated == 0 {
        return;
    }

    let mut out = out;
    let mut orientation = [0u8; N];
    for i in 0..N {
        orientation[i] = ORIENT_TBL[ksq[i] as usize & 63] ^ (56 * (perspective ^ i as Color));
    }

    // Enumerate the changed pawns and their partners once for all perspectives
    let unchanged = (pawns_w | pawns_b) & !updated;
    let mut u = updated;
    while u != 0 {
        let a = pop_lsb(&mut u);
        let partners = PAWN_PAIR_BB[a as usize] & (unchanged | u);
        if partners == 0 {
            continue;
        }

        let a_col = if pawns_b & square_bb(a) != 0 { BLACK } else { WHITE };
        let mut a_id = [0; N];
        for i in 0..N {
            a_id[i] = make_pawn_id(a_col ^ perspective ^ i as Color, a ^ orientation[i]);
        }

        let mut push = |to: Square, paired_color: Color| {
            for i in 0..N {
                let b_id = make_pawn_id(paired_color ^ perspective ^ i as Color, to ^ orientation[i]);
                let index = pp_idx(a_id[i], b_id);
                out[i].push(index as u16);
            }
        };

        for to in Squares(pawns_b & partners) {
            push(to, BLACK);
        }
        for to in Squares(pawns_w & partners) {
            push(to, WHITE);
        }
    }
}

pub fn append_active_indices(perspective: Color, pos: &Position, active: &mut ThreatIndexList) {
    let white = pos.pieces_cp(WHITE, PAWN);
    let black = pos.pieces_cp(BLACK, PAWN);
    append_indices_impl::<1>(perspective, [pos.king_square(perspective)], white | black, white, black, [active]);
}

#[inline]
pub fn append_changed_indices(
    perspective: Color,
    ksq: Square,
    diff: &DirtyPawnPairs,
    removed: &mut ThreatIndexList,
    added: &mut ThreatIndexList,
) {
    let [white_before, black_before] = diff.before;
    let [white_after, black_after] = diff.after;

    if white_before == white_after && black_before == black_after {
        return;
    }

    append_indices_impl::<1>(
        perspective,
        [ksq],
        (white_after & !white_before) | (black_after & !black_before),
        white_after,
        black_after,
        [added],
    );
    append_indices_impl::<1>(
        perspective,
        [ksq],
        (white_before & !white_after) | (black_before & !black_after),
        white_before,
        black_before,
        [removed],
    );
}
