//! Port of `src/nnue/features/full_threats.{h,cpp}`: feature FullThreats, one
//! feature per (attacker, attacked piece, from, to), oriented by the king.

use super::ThreatIndexList;
use crate::attacks::{attacks_bb, PSEUDO_ATTACKS};
use crate::bitboard::*;
use crate::nnue::{IndexType, ORIENT_TBL};
use crate::position::Position;
use crate::types::*;

/// Pawn diagonal threats only target knights and rooks (pawn-pawn relations
/// are the PP_3Wide feature set), so pawns have 4 valid targets.
const NUM_VALID_TARGETS: [IndexType; PIECE_NB] = [0, 4, 10, 8, 8, 10, 0, 0, 0, 4, 10, 8, 8, 10, 0, 0];

/// Hash value embedded in the evaluation file
pub const HASH_VALUE: u32 = 0x2c41_eaea;

/// Number of feature dimensions
pub const DIMENSIONS: IndexType = 59808;

#[rustfmt::skip]
const MAP: [[i32; PIECE_TYPE_NB - 2]; PIECE_TYPE_NB - 2] = [
    [-1,  0, -1,  1, -1, -1],
    [ 0,  1,  2,  3,  4, -1],
    [ 0,  1,  2,  3, -1, -1],
    [ 0,  1,  2,  3, -1, -1],
    [ 0,  1,  2,  3,  4, -1],
    [-1, -1, -1, -1, -1, -1],
];

pub struct ThreatLuts {
    /// [attacker][attacked][from < to]
    lut1: [[[u32; 2]; PIECE_NB]; PIECE_NB],
    /// [pt][from][to]
    lut2: [[[u16; SQUARE_NB]; SQUARE_NB]; PIECE_TYPE_NB],
}

const ALL_PIECES_LIST: [Piece; 12] =
    [W_PAWN, W_KNIGHT, W_BISHOP, W_ROOK, W_QUEEN, W_KING, B_PAWN, B_KNIGHT, B_BISHOP, B_ROOK, B_QUEEN, B_KING];

/// `init_threat_luts()`, computed at compile time as in C++.
pub static THREAT_LUTS: ThreatLuts = {
    let mut t = ThreatLuts { lut1: [[[0; 2]; PIECE_NB]; PIECE_NB], lut2: [[[0; SQUARE_NB]; SQUARE_NB]; PIECE_TYPE_NB] };
    let mut offsets = [0u16; PIECE_TYPE_NB];

    let mut pt = PAWN;
    while pt <= KING {
        let mut offset: u16 = 0;
        let mut from = 0;
        while from < SQUARE_NB {
            let mut to = 0;
            while to < SQUARE_NB {
                let attacks = if pt != PAWN {
                    PSEUDO_ATTACKS[pt as usize][from]
                } else if to < from {
                    PSEUDO_ATTACKS[BLACK as usize][from]
                } else {
                    PSEUDO_ATTACKS[WHITE as usize][from]
                };
                t.lut2[pt as usize][from][to] = offset + (((1u64 << to) - 1) & attacks).count_ones() as u16;
                to += 1;
            }
            if pt != PAWN {
                offset += PSEUDO_ATTACKS[pt as usize][from].count_ones() as u16;
            } else if from >= 8 && from <= 55 {
                // SQ_A2 to SQ_H7
                offset += PSEUDO_ATTACKS[WHITE as usize][from].count_ones() as u16;
            }
            from += 1;
        }
        offsets[pt as usize] = offset;
        pt += 1;
    }

    let mut cumulative_offset: IndexType = 0;
    let mut i = 0;
    while i < ALL_PIECES_LIST.len() {
        let attacker = ALL_PIECES_LIST[i];
        let attacker_type = type_of(attacker);
        let cumulative_piece_offset = offsets[attacker_type as usize] as IndexType;
        let valid_targets = NUM_VALID_TARGETS[attacker as usize];

        let mut k = 0;
        while k < ALL_PIECES_LIST.len() {
            let attacked = ALL_PIECES_LIST[k];
            let enemy = (attacker ^ attacked) == 8;
            let attacked_type = type_of(attacked);

            let map = MAP[attacker_type as usize - 1][attacked_type as usize - 1];
            let semi_excluded = attacker_type == attacked_type && (enemy || attacker_type != PAWN);
            let excluded = map < 0;
            // Unused when excluded, where map is negative
            let feature = if excluded {
                0
            } else {
                cumulative_offset
                    + ((attacked >> 3) as IndexType * (valid_targets / 2) + map as IndexType) * cumulative_piece_offset
            };

            t.lut1[attacker as usize][attacked as usize][0] = if excluded { DIMENSIONS } else { feature };
            t.lut1[attacker as usize][attacked as usize][1] =
                if excluded || semi_excluded { DIMENSIONS } else { feature };
            k += 1;
        }
        cumulative_offset += valid_targets * cumulative_piece_offset;
        i += 1;
    }
    t
};

/// Index of a feature for a given king position and another piece on some
/// square. An index of DIMENSIONS or more is an excluded feature.
#[inline(always)]
pub fn make_index(perspective: Color, attacker: Piece, from: Square, to: Square, attacked: Piece, ksq: Square) -> IndexType {
    let orientation = ORIENT_TBL[ksq as usize & 63] ^ (56 * perspective);
    let from_oriented = (from ^ orientation) as usize & 63;
    let to_oriented = (to ^ orientation) as usize & 63;

    let swap = 8 * perspective;
    let attacker_oriented = (attacker ^ swap) as usize & 15;
    let attacked_oriented = (attacked ^ swap) as usize & 15;

    THREAT_LUTS.lut1[attacker_oriented][attacked_oriented][(from_oriented < to_oriented) as usize]
        + THREAT_LUTS.lut2[type_of(attacker) as usize][from_oriented][to_oriented] as IndexType
}

/// Every index fits in the u16 of the lists: the largest one, of an excluded
/// queen feature, is DIMENSIONS plus the 1,456 queen attacks on an empty board.
#[inline(always)]
fn push_if_lt(list: &mut ThreatIndexList, index: IndexType) {
    debug_assert!(index < 1 << 16);
    list.push_if_lt(index as u16, DIMENSIONS as u16);
}

/// The indices of the active features, in ascending order of attacker.
pub fn append_active_indices(perspective: Color, pos: &Position, active: &mut ThreatIndexList) {
    let ksq = pos.king_square(perspective);
    let occupied = pos.pieces();
    let pawn_targets = pos.pieces_pt2(KNIGHT, ROOK);
    let minor_slider_targets = pos.pieces_pt2(PAWN, KNIGHT) | pos.pieces_pt2(BISHOP, ROOK);
    let queen_targets = minor_slider_targets | pos.pieces_pt(QUEEN);

    let mut process_pawn_attacks = |c: Color, attk_dir: Direction| {
        let c_pawns = pos.pieces_cp(c, PAWN);
        let attacks = shift(c_pawns, attk_dir) & pawn_targets;
        for to in Squares(attacks) {
            let from = sq_add(to, -attk_dir);
            let attacked = pos.piece_on(to);
            let attacker = make_piece(c, PAWN);
            push_if_lt(active, make_index(perspective, attacker, from, to, attacked, ksq));
        }
    };

    process_pawn_attacks(WHITE, NORTH_EAST);
    process_pawn_attacks(WHITE, NORTH_WEST);
    process_pawn_attacks(BLACK, SOUTH_WEST);
    process_pawn_attacks(BLACK, SOUTH_EAST);

    for c in [WHITE, BLACK] {
        for pt in KNIGHT..KING {
            let attacker = make_piece(c, pt);
            let targets = if pt == KNIGHT || pt == QUEEN { queen_targets } else { minor_slider_targets };
            for from in Squares(pos.pieces_cp(c, pt)) {
                let attacks = attacks_bb(pt, from, occupied) & targets;
                for to in Squares(attacks) {
                    let attacked = pos.piece_on(to);
                    push_if_lt(active, make_index(perspective, attacker, from, to, attacked, ksq));
                }
            }
        }
    }
}

/// The indices of the features changed by a move, from its dirty threats.
/// The fields of the packed DirtyThreat index the tables directly: the low
/// byte is the attacker square, then the attacked square, then the attacked
/// piece and the attacker in one byte (`lut1` flattened).
#[inline]
pub fn append_changed_indices(
    perspective: Color,
    ksq: Square,
    diff: &DirtyThreats,
    removed: &mut ThreatIndexList,
    added: &mut ThreatIndexList,
) {
    let orient = (ORIENT_TBL[ksq as usize & 63] ^ (56 * perspective)) as u32;
    let pair_swap = 0x88 * perspective as u32;

    for dirty in diff.list.as_slice() {
        let raw = dirty.raw();
        let from = ((raw & 0xFF) ^ orient) as usize & 63;
        let to = (((raw >> 8) & 0xFF) ^ orient) as usize & 63;
        let pair = (raw >> 16) & 0xFF;
        let plut2 = &THREAT_LUTS.lut2[((pair >> 4) & 7) as usize];

        let p = (pair ^ pair_swap) as usize;
        let index = THREAT_LUTS.lut1[p >> 4][p & 15][(from < to) as usize] + plut2[from][to] as IndexType;

        if raw >> 31 != 0 {
            push_if_lt(added, index);
        } else {
            push_if_lt(removed, index);
        }
    }
}
