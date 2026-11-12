//! Port of `src/nnue/features/k32h3.{h,cpp}`: feature K32H3, the position of
//! the own king (32 buckets), a piece-square block (3 states of the heavy
//! material) and the piece-square positions. Mirrored so that the king is
//! always on files e..h.

use crate::misc::ValueList;
use crate::nnue::{IndexType, ORIENT_TBL};
use crate::position::Position;
use crate::types::*;

// Unique number for each piece type on each square
const PS_NONE: u16 = 0;
const PS_W_PAWN: u16 = 0;
const PS_B_PAWN: u16 = 64;
const PS_W_KNIGHT: u16 = 2 * 64;
const PS_B_KNIGHT: u16 = 3 * 64;
const PS_W_BISHOP: u16 = 4 * 64;
const PS_B_BISHOP: u16 = 5 * 64;
const PS_W_ROOK: u16 = 6 * 64;
const PS_B_ROOK: u16 = 7 * 64;
const PS_W_QUEEN: u16 = 8 * 64;
const PS_B_QUEEN: u16 = 9 * 64;
const PS_KING: u16 = 10 * 64;
const PS_NB: u16 = 11 * 64;

/// Convention: W - us, B - them. Viewed from the other side, W and B are
/// reversed.
#[rustfmt::skip]
const PIECE_SQUARE_INDEX: [[u16; PIECE_NB]; COLOR_NB] = [
    [PS_NONE, PS_W_PAWN, PS_W_KNIGHT, PS_W_BISHOP, PS_W_ROOK, PS_W_QUEEN, PS_KING, PS_NONE,
     PS_NONE, PS_B_PAWN, PS_B_KNIGHT, PS_B_BISHOP, PS_B_ROOK, PS_B_QUEEN, PS_KING, PS_NONE],
    [PS_NONE, PS_B_PAWN, PS_B_KNIGHT, PS_B_BISHOP, PS_B_ROOK, PS_B_QUEEN, PS_KING, PS_NONE,
     PS_NONE, PS_W_PAWN, PS_W_KNIGHT, PS_W_BISHOP, PS_W_ROOK, PS_W_QUEEN, PS_KING, PS_NONE],
];

/// Hash value embedded in the evaluation file
pub const HASH_VALUE: u32 = 0xab00_5cef;

/// Piece-square blocks, in order of increasing heavy material:
///   0: no rook or queen on the board,
///   1: some rook or queen on the board, but the opponent has no queen,
///   2: the opponent has a queen.
pub const NUM_BLOCKS: usize = 3;

/// Block of each perspective
pub type BlockState = [u8; COLOR_NB];

/// Number of features per block (22,528 piece-squares)
pub const BLOCK_DIMENSIONS: IndexType = (SQUARE_NB as IndexType / 2) * PS_NB as IndexType;

pub const fn block_offset(block: usize) -> IndexType {
    block as IndexType * BLOCK_DIMENSIONS
}

/// Number of feature dimensions (67,584 piece-squares)
pub const DIMENSIONS: IndexType = NUM_BLOCKS as IndexType * BLOCK_DIMENSIONS;

#[rustfmt::skip]
const KING_BUCKETS: [u16; SQUARE_NB] = {
    const fn b(v: u16) -> u16 { v * PS_NB }
    [
        b(28), b(29), b(30), b(31), b(31), b(30), b(29), b(28),
        b(24), b(25), b(26), b(27), b(27), b(26), b(25), b(24),
        b(20), b(21), b(22), b(23), b(23), b(22), b(21), b(20),
        b(16), b(17), b(18), b(19), b(19), b(18), b(17), b(16),
        b(12), b(13), b(14), b(15), b(15), b(14), b(13), b(12),
        b( 8), b( 9), b(10), b(11), b(11), b(10), b( 9), b( 8),
        b( 4), b( 5), b( 6), b( 7), b( 7), b( 6), b( 5), b( 4),
        b( 0), b( 1), b( 2), b( 3), b( 3), b( 2), b( 1), b( 0),
    ]
};

/// Maximum number of simultaneously active features
pub const MAX_ACTIVE_DIMENSIONS: usize = 32;
pub type IndexList = ValueList<u16, MAX_ACTIVE_DIMENSIONS>;

/// `PieceSquareIndex + kingBucket + orient` for each perspective, king square
/// and piece; the square is then xored in (no carry crosses bit 6).
static OFFSETS: [[u16; PIECE_NB]; COLOR_NB * SQUARE_NB] = {
    let mut table = [[0u16; PIECE_NB]; COLOR_NB * SQUARE_NB];
    let mut c = 0;
    while c < COLOR_NB {
        let mut sq = 0;
        while sq < SQUARE_NB {
            let flip = 56 * c;
            let orient = (ORIENT_TBL[sq] as usize ^ flip) as u16;
            let king_bucket = KING_BUCKETS[sq ^ flip];
            let mut pi = 0;
            while pi < PIECE_NB {
                table[c * SQUARE_NB + sq][pi] = PIECE_SQUARE_INDEX[c][pi] + king_bucket + orient;
                pi += 1;
            }
            sq += 1;
        }
        c += 1;
    }
    table
};

/// Index of a feature within its block, for a given king position and
/// another piece on some square.
#[inline(always)]
pub fn make_index(perspective: Color, s: Square, pc: Piece, ksq: Square) -> u16 {
    (s as u16) ^ OFFSETS[perspective as usize * SQUARE_NB + (ksq as usize & 63)][pc as usize & 15]
}

/// The indices of the features changed by a move.
#[inline]
pub fn append_changed_indices(
    perspective: Color,
    ksq: Square,
    diff: &DirtyPiece,
    removed: &mut IndexList,
    added: &mut IndexList,
) {
    removed.push(make_index(perspective, diff.from, diff.pc, ksq));
    if diff.to != SQ_NONE {
        added.push(make_index(perspective, diff.to, diff.pc, ksq));
    }
    if diff.remove_sq != SQ_NONE {
        removed.push(make_index(perspective, diff.remove_sq, diff.remove_pc, ksq));
    }
    if diff.add_sq != SQ_NONE {
        added.push(make_index(perspective, diff.add_sq, diff.add_pc, ksq));
    }
}

#[inline]
pub fn block(pos: &Position, perspective: Color) -> usize {
    if pos.count(not_color(perspective), QUEEN) != 0 {
        2
    } else {
        (pos.pieces_pt2(ROOK, QUEEN) != 0) as usize
    }
}

#[inline]
pub fn block_state(pos: &Position) -> BlockState {
    [block(pos, WHITE) as u8, block(pos, BLACK) as u8]
}

/// Whether the change stored in this DirtyPiece means that a full
/// accumulator refresh is required.
#[inline]
pub fn requires_refresh(diff: &DirtyPiece, perspective: Color, block_before: usize, block_after: usize) -> bool {
    diff.pc == make_piece(perspective, KING) || block_before != block_after
}
