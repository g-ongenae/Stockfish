//! Port of `src/movegen.{h,cpp}`: legal move generation.
//!
//! The generators only produce legal moves (pinned pieces stay on their line,
//! the king avoids attacked squares), and the order in which they produce them
//! is part of the output: pawn moves (pushes, then promotions, then
//! captures and en passant), knights, bishops, rooks, queens, then the king and
//! castling, each piece from a1 to h8 and each destination from a1 to h8. The
//! C++ templates are const generics here, so each combination is compiled
//! separately as in C++.

use crate::attacks::*;
use crate::bitboard::*;
use crate::position::Position;
use crate::types::*;

pub type GenType = u8;
pub const CAPTURES: GenType = 0;
pub const QUIETS: GenType = 1;
pub const EVASIONS: GenType = 2;
pub const ALL: GenType = 3;

/// `ExtMove`: a move with a score, for the move picker. C++ derives it from
/// `Move` and compares two of them by `value` only (`operator<`); here the
/// move is a member.
#[derive(Clone, Copy, Default, Debug)]
pub struct ExtMove {
    pub mv: Move,
    pub value: i32,
}

/// A list of moves on the stack, as the C++ `MoveList`.
pub struct MoveList {
    moves: [Move; MAX_MOVES],
    len: usize,
}

impl MoveList {
    #[inline(always)]
    fn empty() -> MoveList {
        MoveList { moves: [Move::none(); MAX_MOVES], len: 0 }
    }

    /// `MoveList<T>(pos)`.
    #[inline]
    pub fn new<const T: GenType>(pos: &Position) -> MoveList {
        let mut list = MoveList::empty();
        generate::<T>(pos, &mut list);
        list
    }

    /// `MoveList<ALL>(pos)`: all the legal moves.
    #[inline]
    pub fn legal(pos: &Position) -> MoveList {
        MoveList::new::<ALL>(pos)
    }

    #[inline(always)]
    pub fn push(&mut self, m: Move) {
        self.moves[self.len] = m;
        self.len += 1;
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline(always)]
    pub fn as_slice(&self) -> &[Move] {
        &self.moves[..self.len]
    }

    pub fn contains(&self, m: Move) -> bool {
        self.as_slice().contains(&m)
    }

    pub fn iter(&self) -> std::slice::Iter<'_, Move> {
        self.as_slice().iter()
    }
}

impl<'a> IntoIterator for &'a MoveList {
    type Item = &'a Move;
    type IntoIter = std::slice::Iter<'a, Move>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[inline(always)]
fn splat_pawn_moves(list: &mut MoveList, offset: Direction, mut to_bb: Bitboard) {
    while to_bb != 0 {
        let to = pop_lsb(&mut to_bb);
        list.push(Move::new(sq_add(to, -offset), to));
    }
}

#[inline(always)]
fn splat_moves(list: &mut MoveList, from: Square, mut to_bb: Bitboard) {
    while to_bb != 0 {
        list.push(Move::new(from, pop_lsb(&mut to_bb)));
    }
}

#[inline(always)]
fn make_promotions<const TYPE: GenType, const ENEMY: bool>(list: &mut MoveList, d: Direction, to: Square) {
    let all = TYPE == EVASIONS || TYPE == ALL;
    let from = sq_add(to, -d);

    if TYPE == CAPTURES || all {
        list.push(Move::make(PROMOTION, from, to, QUEEN));
    }

    if (TYPE == CAPTURES && ENEMY) || (TYPE == QUIETS && !ENEMY) || all {
        list.push(Move::make(PROMOTION, from, to, ROOK));
        list.push(Move::make(PROMOTION, from, to, BISHOP));
        list.push(Move::make(PROMOTION, from, to, KNIGHT));
    }
}

fn generate_pawn_moves<const US: Color, const TYPE: GenType>(pos: &Position, list: &mut MoveList, target: Bitboard) {
    debug_assert!(TYPE != ALL);

    let them = not_color(US);
    let t_rank7_bb = if US == WHITE { RANK_7_BB } else { RANK_2_BB };
    let t_rank3_bb = if US == WHITE { RANK_3_BB } else { RANK_6_BB };
    let up = pawn_push(US);
    let up_right = if US == WHITE { NORTH_EAST } else { SOUTH_WEST };
    let up_left = if US == WHITE { NORTH_WEST } else { SOUTH_EAST };

    let empty_squares = !pos.pieces();
    let enemies = if TYPE == EVASIONS { pos.checkers() } else { pos.pieces_c(them) };
    let pinned = pos.blockers_for_king(US);
    let ksq = pos.king_square(US);

    let pawns_on_7 = pos.pieces_cp(US, PAWN) & t_rank7_bb;
    let pawns_not_on_7 = pos.pieces_cp(US, PAWN) & !t_rank7_bb;
    let pushable = !pinned | file_bb_of(ksq);
    let can_capture_left = !pinned | antidiag_bb(ksq);
    let can_capture_right = !pinned | diag_bb(ksq);

    // Single and double pawn pushes, no promotions
    if TYPE != CAPTURES {
        let mut b1 = shift(pawns_not_on_7 & pushable, up) & empty_squares;
        let mut b2 = shift(b1 & t_rank3_bb, up) & empty_squares;

        if TYPE == EVASIONS {
            // Consider only blocking squares
            b1 &= target;
            b2 &= target;
        }

        splat_pawn_moves(list, up, b1);
        splat_pawn_moves(list, up + up, b2);
    }

    // Promotions and underpromotions
    if pawns_on_7 != 0 {
        let mut b1 = shift(pawns_on_7 & can_capture_right, up_right) & enemies;
        let mut b2 = shift(pawns_on_7 & can_capture_left, up_left) & enemies;
        let mut b3 = shift(pawns_on_7 & pushable, up) & empty_squares;

        if TYPE == EVASIONS {
            b3 &= target;
        }

        while b1 != 0 {
            make_promotions::<TYPE, true>(list, up_right, pop_lsb(&mut b1));
        }
        while b2 != 0 {
            make_promotions::<TYPE, true>(list, up_left, pop_lsb(&mut b2));
        }
        while b3 != 0 {
            make_promotions::<TYPE, false>(list, up, pop_lsb(&mut b3));
        }
    }

    // Standard and en passant captures
    if TYPE == CAPTURES || TYPE == EVASIONS {
        let b1 = shift(pawns_not_on_7 & can_capture_right, up_right) & enemies;
        let b2 = shift(pawns_not_on_7 & can_capture_left, up_left) & enemies;

        splat_pawn_moves(list, up_right, b1);
        splat_pawn_moves(list, up_left, b2);

        let ep_sq = pos.ep_square();

        // An en passant capture cannot resolve a discovered check
        if ep_sq != SQ_NONE && !(TYPE == EVASIONS && target & square_bb(sq_add(ep_sq, up)) != 0) {
            debug_assert!(rank_of(ep_sq) == relative_rank(US, RANK_6));

            let mut b1 = pawns_not_on_7
                & ((can_capture_right & shift(square_bb(ep_sq), -up_right))
                    | (can_capture_left & shift(square_bb(ep_sq), -up_left)));
            debug_assert!(b1 != 0);

            while b1 != 0 {
                list.push(Move::make(EN_PASSANT, pop_lsb(&mut b1), ep_sq, KNIGHT));
            }
        }
    }
}

fn generate_moves<const PT: PieceType>(pos: &Position, list: &mut MoveList, target: Bitboard, us: Color) {
    debug_assert!(PT != KING && PT != PAWN);

    let ksq = pos.king_square(us);
    let pinned = pos.blockers_for_king(us);

    for from in Squares(pos.pieces_cp(us, PT) & !pinned) {
        let b = attacks_bb(PT, from, pos.pieces()) & target;
        splat_moves(list, from, b);
    }

    // Pinned knights cannot move
    if PT != KNIGHT {
        for from in Squares(pos.pieces_cp(us, PT) & pinned) {
            let b = attacks_bb(PT, from, pos.pieces()) & target & line_bb(ksq, from);
            splat_moves(list, from, b);
        }
    }
}

/// CAPTURES: all legal captures plus queen promotions.
/// QUIETS: all legal non-captures and underpromotions.
/// EVASIONS: all legal check evasions.
/// ALL: all legal moves (evasions in check, else captures then quiets).
pub fn generate<const TYPE: GenType>(pos: &Position, list: &mut MoveList) {
    if TYPE == ALL {
        if pos.checkers() != 0 {
            generate::<EVASIONS>(pos, list);
        } else {
            generate::<CAPTURES>(pos, list);
            generate::<QUIETS>(pos, list);
        }
        return;
    }

    debug_assert!((TYPE == EVASIONS) == (pos.checkers() != 0));

    let us = pos.side_to_move();
    let ksq = pos.king_square(us);
    let mut target: Bitboard = 0;

    // Skip generating non-king moves when in double check
    if TYPE != EVASIONS || !more_than_one(pos.checkers()) {
        target = match TYPE {
            EVASIONS => between_bb(ksq, lsb(pos.checkers())),
            CAPTURES => pos.pieces_c(not_color(us)),
            _ => !pos.pieces(), // QUIETS
        };

        if us == WHITE {
            generate_pawn_moves::<WHITE, TYPE>(pos, list, target);
        } else {
            generate_pawn_moves::<BLACK, TYPE>(pos, list, target);
        }
        generate_moves::<KNIGHT>(pos, list, target, us);
        generate_moves::<BISHOP>(pos, list, target, us);
        generate_moves::<ROOK>(pos, list, target, us);
        generate_moves::<QUEEN>(pos, list, target, us);
    }

    if TYPE == EVASIONS {
        target = !pos.pieces_c(us);
    }

    target &= !pos.threats_by(ALL_PIECES);

    let b = pseudo_attacks(KING, ksq) & target;
    splat_moves(list, ksq, b);

    if TYPE == QUIETS && pos.can_castle(color_castling(us, ANY_CASTLING)) {
        for cr in [color_castling(us, KING_SIDE), color_castling(us, QUEEN_SIDE)] {
            if !pos.can_castle(cr) || pos.castling_impeded(cr) {
                continue;
            }

            let rook_square = pos.castling_rook_square(cr);
            let to = relative_square(us, if rook_square > ksq { SQ_G1 } else { SQ_C1 });

            if between_bb(ksq, to) & pos.threats_by(ALL_PIECES) == 0
                && pos.blockers_for_king(us) & square_bb(rook_square) == 0
            {
                list.push(Move::make(CASTLING, ksq, rook_square, KNIGHT));
            }
        }
    }
}
