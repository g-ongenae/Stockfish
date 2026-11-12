//! Port of `src/types.h`: the basic chess types.
//!
//! The C++ enums are plain integers with arithmetic on them. They are kept as
//! integer type aliases and constants here, with the same names and values, so
//! that the two sides read alike and tables are indexed the same way. Only
//! `Move`, which carries behavior, is a newtype.

pub type Key = u64;
pub type Bitboard = u64;

pub const MAX_MOVES: usize = 256;
pub const MAX_PLY: usize = 246;

pub type Color = u8;
pub const WHITE: Color = 0;
pub const BLACK: Color = 1;
pub const COLOR_NB: usize = 2;

pub type CastlingRights = u8;
pub const NO_CASTLING: CastlingRights = 0;
pub const WHITE_OO: CastlingRights = 1;
pub const WHITE_OOO: CastlingRights = WHITE_OO << 1;
pub const BLACK_OO: CastlingRights = WHITE_OO << 2;
pub const BLACK_OOO: CastlingRights = WHITE_OO << 3;
pub const KING_SIDE: CastlingRights = WHITE_OO | BLACK_OO;
pub const QUEEN_SIDE: CastlingRights = WHITE_OOO | BLACK_OOO;
pub const WHITE_CASTLING: CastlingRights = WHITE_OO | WHITE_OOO;
pub const BLACK_CASTLING: CastlingRights = BLACK_OO | BLACK_OOO;
pub const ANY_CASTLING: CastlingRights = WHITE_CASTLING | BLACK_CASTLING;
pub const CASTLING_RIGHT_NB: usize = 16;

pub type Value = i32;
pub const VALUE_ZERO: Value = 0;

pub const PAWN_VALUE: Value = 208;
pub const KNIGHT_VALUE: Value = 781;
pub const BISHOP_VALUE: Value = 825;
pub const ROOK_VALUE: Value = 1276;
pub const QUEEN_VALUE: Value = 2538;

pub type PieceType = u8;
pub const NO_PIECE_TYPE: PieceType = 0;
pub const PAWN: PieceType = 1;
pub const KNIGHT: PieceType = 2;
pub const BISHOP: PieceType = 3;
pub const ROOK: PieceType = 4;
pub const QUEEN: PieceType = 5;
pub const KING: PieceType = 6;
pub const ALL_PIECES: PieceType = 0;
pub const PIECE_TYPE_NB: usize = 8;

pub type Piece = u8;
pub const NO_PIECE: Piece = 0;
pub const W_PAWN: Piece = PAWN;
pub const W_KNIGHT: Piece = 2;
pub const W_BISHOP: Piece = 3;
pub const W_ROOK: Piece = 4;
pub const W_QUEEN: Piece = 5;
pub const W_KING: Piece = 6;
pub const B_PAWN: Piece = PAWN + 8;
pub const B_KNIGHT: Piece = 10;
pub const B_BISHOP: Piece = 11;
pub const B_ROOK: Piece = 12;
pub const B_QUEEN: Piece = 13;
pub const B_KING: Piece = 14;
pub const PIECE_NB: usize = 16;

#[rustfmt::skip]
pub const PIECE_VALUE: [Value; PIECE_NB] = [
    VALUE_ZERO, PAWN_VALUE, KNIGHT_VALUE, BISHOP_VALUE, ROOK_VALUE, QUEEN_VALUE, VALUE_ZERO, VALUE_ZERO,
    VALUE_ZERO, PAWN_VALUE, KNIGHT_VALUE, BISHOP_VALUE, ROOK_VALUE, QUEEN_VALUE, VALUE_ZERO, VALUE_ZERO,
];

pub type Depth = i32;

pub type Square = u8;
#[rustfmt::skip]
mod squares {
    use super::Square;
    pub const SQ_A1: Square = 0;  pub const SQ_B1: Square = 1;  pub const SQ_C1: Square = 2;
    pub const SQ_D1: Square = 3;  pub const SQ_E1: Square = 4;  pub const SQ_F1: Square = 5;
    pub const SQ_G1: Square = 6;  pub const SQ_H1: Square = 7;
    pub const SQ_A8: Square = 56; pub const SQ_H8: Square = 63;
}
pub use squares::*;
pub const SQ_NONE: Square = 64;
pub const SQUARE_NB: usize = 64;

/// Directions are signed steps between squares, as in the C++ `Direction`.
pub type Direction = i32;
pub const NORTH: Direction = 8;
pub const EAST: Direction = 1;
pub const SOUTH: Direction = -NORTH;
pub const WEST: Direction = -EAST;
pub const NORTH_EAST: Direction = NORTH + EAST;
pub const SOUTH_EAST: Direction = SOUTH + EAST;
pub const SOUTH_WEST: Direction = SOUTH + WEST;
pub const NORTH_WEST: Direction = NORTH + WEST;

pub type File = u8;
pub const FILE_A: File = 0;
pub const FILE_B: File = 1;
pub const FILE_C: File = 2;
pub const FILE_G: File = 6;
pub const FILE_H: File = 7;
pub const FILE_NB: usize = 8;

pub type Rank = u8;
pub const RANK_1: Rank = 0;
pub const RANK_2: Rank = 1;
pub const RANK_6: Rank = 5;
pub const RANK_8: Rank = 7;
pub const RANK_NB: usize = 8;

#[inline(always)]
pub const fn not_color(c: Color) -> Color {
    c ^ BLACK
}

/// `operator~(Piece)`: the same piece type of the other color.
#[inline(always)]
pub const fn flip_piece_color(pc: Piece) -> Piece {
    pc ^ 8
}

/// `Color & CastlingRights` in C++.
#[inline(always)]
pub const fn color_castling(c: Color, cr: CastlingRights) -> CastlingRights {
    (if c == WHITE { WHITE_CASTLING } else { BLACK_CASTLING }) & cr
}

#[inline(always)]
pub const fn make_square(f: File, r: Rank) -> Square {
    (r << 3) + f
}

#[inline(always)]
pub const fn make_piece(c: Color, pt: PieceType) -> Piece {
    (c << 3) + pt
}

#[inline(always)]
pub const fn type_of(pc: Piece) -> PieceType {
    pc & 7
}

#[inline(always)]
pub const fn color_of(pc: Piece) -> Color {
    debug_assert!(pc != NO_PIECE);
    pc >> 3
}

#[inline(always)]
pub const fn is_ok_square(s: Square) -> bool {
    s <= SQ_H8
}

#[inline(always)]
pub const fn file_of(s: Square) -> File {
    s & 7
}

#[inline(always)]
pub const fn rank_of(s: Square) -> Rank {
    s >> 3
}

#[inline(always)]
pub const fn relative_square(c: Color, s: Square) -> Square {
    s ^ (c * 56)
}

#[inline(always)]
pub const fn relative_rank(c: Color, r: Rank) -> Rank {
    r ^ (c * 7)
}

#[inline(always)]
pub const fn relative_rank_of(c: Color, s: Square) -> Rank {
    relative_rank(c, rank_of(s))
}

#[inline(always)]
pub const fn pawn_push(c: Color) -> Direction {
    if c == WHITE {
        NORTH
    } else {
        SOUTH
    }
}

/// `Square + Direction`. The caller guarantees the result is a square, as in C++.
#[inline(always)]
pub const fn sq_add(s: Square, d: Direction) -> Square {
    (s as i32 + d) as Square
}

#[inline(always)]
pub const fn make_key(seed: u64) -> Key {
    seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407)
}

pub type MoveType = u16;
pub const NORMAL: MoveType = 0;
pub const PROMOTION: MoveType = 1 << 14;
pub const EN_PASSANT: MoveType = 2 << 14;
pub const CASTLING: MoveType = 3 << 14;

/// A move needs 16 bits to be stored, laid out as in C++:
///
/// bit  0- 5: destination square (from 0 to 63)
/// bit  6-11: origin square (from 0 to 63)
/// bit 12-13: promotion piece type - 2 (from KNIGHT-2 to QUEEN-2)
/// bit 14-15: special move flag: promotion (1), en passant (2), castling (3)
///
/// Castling is encoded as "king captures friendly rook", in Chess960 and in
/// standard chess alike.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct Move(pub u16);

impl Move {
    #[inline(always)]
    pub const fn new(from: Square, to: Square) -> Move {
        Move(((from as u16) << 6) + to as u16)
    }

    #[inline(always)]
    pub const fn make(t: MoveType, from: Square, to: Square, pt: PieceType) -> Move {
        Move(t + (((pt - KNIGHT) as u16) << 12) + ((from as u16) << 6) + to as u16)
    }

    #[inline(always)]
    pub const fn from_sq(self) -> Square {
        ((self.0 >> 6) & 0x3F) as Square
    }

    #[inline(always)]
    pub const fn to_sq(self) -> Square {
        (self.0 & 0x3F) as Square
    }

    #[inline(always)]
    pub const fn type_of(self) -> MoveType {
        self.0 & (3 << 14)
    }

    #[inline(always)]
    pub const fn promotion_type(self) -> PieceType {
        (((self.0 >> 12) & 3) as PieceType) + KNIGHT
    }

    #[inline(always)]
    pub const fn is_ok(self) -> bool {
        self.0 != Move::none().0 && self.0 != Move::null().0
    }

    #[inline(always)]
    pub const fn null() -> Move {
        Move(65)
    }

    #[inline(always)]
    pub const fn none() -> Move {
        Move(0)
    }

    #[inline(always)]
    pub const fn raw(self) -> u16 {
        self.0
    }
}

/// What `do_move()` changes on the board, for the NNUE piece-square features.
#[derive(Clone, Copy, Default, Debug)]
pub struct DirtyPiece {
    /// Never NO_PIECE after a move
    pub pc: Piece,
    /// `to` is SQ_NONE for promotions
    pub from: Square,
    pub to: Square,
    /// If {add,remove}_sq is SQ_NONE, {add,remove}_pc is meaningless. Castling
    /// uses add_sq and remove_sq to remove and add the rook.
    pub remove_sq: Square,
    pub add_sq: Square,
    pub remove_pc: Piece,
    pub add_pc: Piece,
}

/// A threat that appears or disappears on the board, packed in 32 bits as in
/// C++, since the feature set indexes its tables with the raw fields.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct DirtyThreat(pub u32);

impl DirtyThreat {
    pub const PC_SQ_OFFSET: u32 = 0;
    pub const THREATENED_SQ_OFFSET: u32 = 8;
    pub const THREATENED_PC_OFFSET: u32 = 16;
    pub const PC_OFFSET: u32 = 20;

    #[inline(always)]
    pub const fn new(pc: Piece, threatened_pc: Piece, pc_sq: Square, threatened_sq: Square, add: bool) -> DirtyThreat {
        DirtyThreat(
            ((add as u32) << 31)
                | ((pc as u32) << Self::PC_OFFSET)
                | ((threatened_pc as u32) << Self::THREATENED_PC_OFFSET)
                | ((threatened_sq as u32) << Self::THREATENED_SQ_OFFSET)
                | ((pc_sq as u32) << Self::PC_SQ_OFFSET),
        )
    }

    #[inline(always)]
    pub const fn raw(self) -> u32 {
        self.0
    }
}

/// `DirtyThreatList`: a non-castling move changes at most 80 threats, a
/// castling move at most 36. C++ adds 16 entries of slack for vector stores.
pub const MAX_DIRTY_THREATS: usize = 96;

#[derive(Clone)]
pub struct DirtyThreats {
    pub list: crate::misc::ValueList<DirtyThreat, MAX_DIRTY_THREATS>,
}

impl Default for DirtyThreats {
    fn default() -> Self {
        DirtyThreats { list: crate::misc::ValueList::new(DirtyThreat(0)) }
    }
}

/// The pawns before and after a move, for the pawn pair features.
#[derive(Clone, Copy, Default, Debug)]
pub struct DirtyPawnPairs {
    pub before: [Bitboard; COLOR_NB],
    pub after: [Bitboard; COLOR_NB],
}

/// Everything `do_move()` records for the NNUE accumulator updates.
#[derive(Clone, Default)]
pub struct Dirties {
    pub dirty_piece: DirtyPiece,
    pub dirty_threats: DirtyThreats,
    pub dirty_pawn_pairs: DirtyPawnPairs,
}
