//! Port of `src/movepick.{h,cpp}`: the move picker, which hands out the moves
//! of a position one at a time, the most promising first.
//!
//! The stages, the scores and the partial insertion sort are those of C++,
//! line for line (only its scalar sort: the AVX-512 variant is an
//! optimization meant to give the same order).
//!
//! # Shape of the API
//!
//! In C++ the picker keeps a `const Position&` and pointers to the history
//! tables, while search makes and unmakes moves on that same position, and
//! writes to those same tables, between two calls of `next_move()`. A Rust
//! picker that borrowed them for its whole life would forbid that. So the
//! picker owns only its own state (the stage, the cursors and the move list,
//! on the stack as in C++), and the position and the tables are lent to each
//! call: [`MovePicker::new`] reads the position (for the TT move and the
//! checkers), [`MovePicker::next_move`] takes the position and a
//! [`Histories`], a bundle of shared references built anew for each call. It
//! must be the same position, in the same state, at every call, as in C++;
//! debug builds check its key. The tables are read when the moves are scored,
//! that is, inside the calls that generate them, exactly when C++ reads them
//! through its pointers: the quiet moves are scored after the good captures
//! were searched, with the histories as they are then.

use crate::bitboard::*;
use crate::history::*;
use crate::movegen::{ExtMove, GenType, MoveList, CAPTURES, EVASIONS, QUIETS};
use crate::position::Position;
use crate::types::*;

/// `enum Stages : i8`.
type Stage = i8;
const TT: Stage = 0;
const INIT_FIRST_STAGE: Stage = 1;
const GOOD_CAPTURE: Stage = 2;
const GOOD_QUIET: Stage = 3;
const BAD_MOVES: Stage = 4;
const QSEARCH: Stage = 5;
const EVASION: Stage = 6;
const PROBCUT: Stage = 7;

/// The tables a picker reads, lent to each call of `next_move()`: the
/// pointers that the C++ constructor stores. The ProbCut picker reads only
/// `capture_history`. The quiescence search fills only
/// `continuation_history[0]` in C++ (an array of one) and the picker reads
/// no other there; the port needs all six, any table will do for the others.
#[derive(Clone, Copy)]
pub struct Histories<'a> {
    pub main_history: &'a ButterflyHistory,
    pub low_ply_history: &'a LowPlyHistory,
    pub capture_history: &'a CapturePieceToHistory,
    /// `(ss - 1)->continuationHistory` to `(ss - 6)->continuationHistory`;
    /// the quiet scores use 0 to 3 and 5.
    pub continuation_history: [&'a PieceToHistory; 6],
    pub shared_history: &'a SharedHistories,
}

/// Sorts moves in descending order up to and including a given limit. The
/// order of the moves below the limit is left unspecified: this is the C++
/// code as written, whose result is part of the output.
fn partial_insertion_sort(moves: &mut [ExtMove], limit: i32) {
    if moves.len() <= 1 {
        return;
    }

    let mut sorted_end = 0;
    for p in 1..moves.len() {
        if moves[p].value >= limit {
            let tmp = moves[p];
            sorted_end += 1;
            moves[p] = moves[sorted_end];
            let mut q = sorted_end;
            while q != 0 && moves[q - 1].value < tmp.value {
                moves[q] = moves[q - 1];
                q -= 1;
            }
            moves[q] = tmp;
        }
    }
}

/// `MovePicker`. See the module documentation for how it is driven.
pub struct MovePicker {
    tt_move: Move,
    /// Indices into `moves` of the C++ pointers `cur`, `endCur`,
    /// `endBadCaptures` and `endBadMoves`.
    cur: usize,
    end_cur: usize,
    end_bad_captures: usize,
    end_bad_moves: usize,
    stage: Stage,
    first_stage: Stage,
    threshold: i32,
    depth: Depth,
    ply: i32,
    skip_quiets: bool,
    /// The key of the position at construction, to check in debug builds
    /// that every call is given the same position.
    #[cfg(debug_assertions)]
    key: Key,
    moves: [ExtMove; MAX_MOVES],
}

impl MovePicker {
    /// The constructor for the main search and the quiescence search (depth
    /// `DEPTH_QS`). The TT move is returned first if it is legal.
    pub fn new(pos: &Position, ttm: Move, d: Depth, pl: i32) -> MovePicker {
        let mut mp = MovePicker::empty();
        mp.renew(pos, ttm, d, pl);
        mp
    }

    /// The constructor for ProbCut: captures (and queen promotions) whose
    /// static exchange evaluation is at least `th`. The TT move is returned
    /// first if it is such a move and legal (its SEE is not checked).
    pub fn new_probcut(pos: &Position, ttm: Move, th: i32) -> MovePicker {
        let mut mp = MovePicker::empty();
        mp.renew_probcut(pos, ttm, th);
        mp
    }

    /// `new()` on an existing picker: the same state as a new one, but
    /// without clearing its move list (2 KiB), which C++ leaves uninitialized
    /// and which no stage reads before writing. A search that keeps one
    /// picker per ply saves that clearing at every node (about a fifth of the
    /// time of the picker, see README.md).
    pub fn renew(&mut self, pos: &Position, ttm: Move, d: Depth, pl: i32) {
        let first_stage = if pos.checkers() != 0 {
            EVASION
        } else if d > 0 {
            GOOD_CAPTURE
        } else {
            QSEARCH
        };
        let stage = if ttm != Move::none() && pos.legal(ttm) { TT } else { INIT_FIRST_STAGE };
        self.set(pos, ttm, stage, first_stage, 0, d, pl);
    }

    /// `new_probcut()` on an existing picker, as `renew()`.
    pub fn renew_probcut(&mut self, pos: &Position, ttm: Move, th: i32) {
        debug_assert!(pos.checkers() == 0);
        let stage = if ttm != Move::none() && pos.capture_stage(ttm) && pos.legal(ttm) { TT } else { INIT_FIRST_STAGE };
        // depth and ply are left uninitialized in C++, and not read
        self.set(pos, ttm, stage, PROBCUT, th, 0, 0);
    }

    /// A picker to be renewed before use: for the pickers a search keeps,
    /// allocated once.
    pub fn empty_for_search() -> MovePicker {
        MovePicker::empty()
    }

    fn empty() -> MovePicker {
        MovePicker {
            tt_move: Move::none(),
            cur: 0,
            end_cur: 0,
            end_bad_captures: 0,
            end_bad_moves: 0,
            stage: INIT_FIRST_STAGE,
            first_stage: INIT_FIRST_STAGE,
            threshold: 0,
            depth: 0,
            ply: 0,
            skip_quiets: false,
            #[cfg(debug_assertions)]
            key: 0,
            moves: [ExtMove::default(); MAX_MOVES],
        }
    }

    /// Every member but the move list.
    #[allow(clippy::too_many_arguments)]
    #[inline(always)]
    fn set(
        &mut self,
        _pos: &Position,
        tt_move: Move,
        stage: Stage,
        first_stage: Stage,
        threshold: i32,
        depth: Depth,
        ply: i32,
    ) {
        self.tt_move = tt_move;
        self.cur = 0;
        self.end_cur = 0;
        self.end_bad_captures = 0;
        self.end_bad_moves = 0;
        self.stage = stage;
        self.first_stage = first_stage;
        self.threshold = threshold;
        self.depth = depth;
        self.ply = ply;
        self.skip_quiets = false;
        #[cfg(debug_assertions)]
        {
            self.key = _pos.key();
        }
    }

    /// `generate_and_score<Type>()`: generates the moves of a kind after
    /// `cur`, scores them, and returns the end of the list.
    #[inline(always)]
    fn generate_and_score<const TYPE: GenType>(&mut self, pos: &Position, h: &Histories) -> usize {
        debug_assert!(TYPE == CAPTURES || TYPE == QUIETS || TYPE == EVASIONS);

        let us = pos.side_to_move();

        let mut threat_by_lesser = [0 as Bitboard; KING as usize + 1];
        if TYPE == QUIETS {
            threat_by_lesser[PAWN as usize] = 0;
            threat_by_lesser[KNIGHT as usize] = pos.threats_by(PAWN);
            threat_by_lesser[BISHOP as usize] = threat_by_lesser[KNIGHT as usize];
            threat_by_lesser[ROOK as usize] =
                pos.threats_by(KNIGHT) | pos.threats_by(BISHOP) | threat_by_lesser[KNIGHT as usize];
            threat_by_lesser[QUEEN as usize] = pos.threats_by(ROOK) | threat_by_lesser[ROOK as usize];
            threat_by_lesser[KING as usize] = 0;
        }

        let ml = MoveList::new::<TYPE>(pos); // move generation

        let mut it = self.cur;
        for &m in ml.iter() {
            let from = m.from_sq();
            let to = m.to_sq();
            let pc = pos.moved_piece(m);
            let pt = type_of(pc);
            let captured_piece = pos.piece_on(to);

            let value = if TYPE == CAPTURES {
                h.capture_history[pc as usize][to as usize][type_of(captured_piece) as usize].get() as i32
                    + 7 * PIECE_VALUE[captured_piece as usize]
            } else if TYPE == QUIETS {
                let (pc_i, to_i) = (pc as usize, to as usize);
                let ch = &h.continuation_history;

                // histories
                let mut value = 2 * h.main_history[us as usize][m.raw() as usize].get() as i32;
                value += 2 * h.shared_history.pawn_entry(pos)[pc_i][to_i].get() as i32;
                value += ch[0][pc_i][to_i].get() as i32;
                value += ch[1][pc_i][to_i].get() as i32;
                value += ch[2][pc_i][to_i].get() as i32;
                value += ch[3][pc_i][to_i].get() as i32;
                value += ch[5][pc_i][to_i].get() as i32;

                // bonus for checks
                value += ((pos.check_squares(pt) & square_bb(to) != 0) && pos.see_ge(m, -75)) as i32 * 16384;

                // penalty for moving to a square threatened by a lesser piece
                // or bonus for escaping an attack by a lesser piece.
                let threat = threat_by_lesser[pt as usize];
                let v = 20 * ((threat & square_bb(from) != 0) as i32 - (threat & square_bb(to) != 0) as i32);
                value += PIECE_VALUE[pt as usize] * v;

                if self.ply < LOW_PLY_HISTORY_SIZE as i32 {
                    value += 8 * h.low_ply_history[self.ply as usize][m.raw() as usize].get() as i32 / (1 + self.ply);
                }

                value
            } else if pos.capture_stage(m) {
                // EVASIONS
                PIECE_VALUE[captured_piece as usize] + (1 << 28)
            } else {
                h.main_history[us as usize][m.raw() as usize].get() as i32
                    + h.continuation_history[0][pc as usize][to as usize].get() as i32
            };

            self.moves[it] = ExtMove { mv: m, value };
            it += 1;
        }
        it
    }

    /// `select([]() { return true; })`: the next move of the list that is not
    /// the TT move.
    #[inline(always)]
    fn select_any(&mut self) -> Move {
        while self.cur < self.end_cur {
            let m = self.moves[self.cur].mv;
            self.cur += 1;
            if m != self.tt_move {
                return m;
            }
        }
        Move::none()
    }

    /// The `select()` of the good quiet moves: those scoring at most the
    /// threshold are set aside as bad moves.
    #[inline(always)]
    fn select_good_quiet(&mut self) -> Move {
        const GOOD_QUIET_THRESHOLD: i32 = -14000;

        while self.cur < self.end_cur {
            let e = self.moves[self.cur];
            if e.mv != self.tt_move {
                if e.value > GOOD_QUIET_THRESHOLD {
                    self.cur += 1;
                    return e.mv;
                }
                self.moves[self.end_bad_moves] = e;
                self.end_bad_moves += 1;
            }
            self.cur += 1;
        }
        Move::none()
    }

    /// The end of the `GOOD_CAPTURE` and `GOOD_QUIET` stages: prepares the
    /// cursors to loop over the bad moves, and returns the first of them.
    #[inline(always)]
    fn start_bad_moves(&mut self) -> Move {
        // Prepare the pointers to loop over the bad captures
        self.cur = 0;
        self.end_cur = self.end_bad_moves;

        self.stage = BAD_MOVES;
        self.select_any()
    }

    /// `next_move()`: the next move, or `Move::none()` when there is none
    /// left. `pos` must be the position given to the constructor, in the same
    /// state; `h` the tables of the search (see the module documentation).
    pub fn next_move(&mut self, pos: &Position, h: &Histories) -> Move {
        #[cfg(debug_assertions)]
        debug_assert_eq!(self.key, pos.key(), "MovePicker::next_move() given another position");

        // The C++ switch, its `goto dispatch` and its fallthroughs
        loop {
            match self.stage {
                TT => {
                    self.stage = INIT_FIRST_STAGE;
                    return self.tt_move;
                }

                INIT_FIRST_STAGE => {
                    self.cur = 0;
                    self.end_bad_moves = 0;

                    self.end_cur = if self.first_stage == EVASION {
                        self.generate_and_score::<EVASIONS>(pos, h)
                    } else {
                        self.generate_and_score::<CAPTURES>(pos, h)
                    };

                    partial_insertion_sort(&mut self.moves[self.cur..self.end_cur], i32::MIN);

                    self.stage = self.first_stage;
                    // goto dispatch
                }

                GOOD_CAPTURE => {
                    while self.cur < self.end_cur {
                        let e = self.moves[self.cur];
                        if e.mv != self.tt_move {
                            // C++ divides the int value, truncating toward zero
                            if pos.see_ge(e.mv, -e.value / 18) {
                                self.cur += 1;
                                return e.mv;
                            }
                            self.moves[self.end_bad_moves] = e;
                            self.end_bad_moves += 1;
                        }
                        self.cur += 1;
                    }

                    self.end_bad_captures = self.end_bad_moves;

                    if !self.skip_quiets {
                        // initialize quiet moves
                        self.cur = self.end_bad_moves;
                        self.end_cur = self.generate_and_score::<QUIETS>(pos, h);

                        partial_insertion_sort(&mut self.moves[self.cur..self.end_cur], -3560 * self.depth);

                        self.stage = GOOD_QUIET;
                        // [[fallthrough]] into GOOD_QUIET
                        let m = self.select_good_quiet();
                        if m != Move::none() {
                            return m;
                        }
                    }

                    return self.start_bad_moves();
                }

                // In C++ this case label is inside `if (!skipQuiets)` of the
                // previous one, and a jump to it does not test the condition:
                // after skip_quiet_moves() the select finds nothing to return,
                // as `cur` is past `endCur = endBadCaptures`.
                GOOD_QUIET => {
                    let m = self.select_good_quiet();
                    if m != Move::none() {
                        return m;
                    }
                    return self.start_bad_moves();
                }

                BAD_MOVES | QSEARCH | EVASION => return self.select_any(),

                PROBCUT => {
                    while self.cur < self.end_cur {
                        let m = self.moves[self.cur].mv;
                        self.cur += 1;
                        if m != self.tt_move && pos.see_ge(m, self.threshold) {
                            return m;
                        }
                    }
                    return Move::none();
                }

                _ => unreachable!(),
            }
        }
    }

    /// `skip_quiet_moves()`: no quiet move is returned from now on, except
    /// the TT move if it was not returned yet.
    pub fn skip_quiet_moves(&mut self) {
        self.skip_quiets = true;
        if self.stage == BAD_MOVES || self.stage == GOOD_QUIET {
            // discards current-stage and bad quiet moves
            self.end_bad_moves = self.end_bad_captures;
            self.end_cur = self.end_bad_captures;
        }
    }

    /// For the search trace: whether the move last returned was the TT move.
    /// The TT move is returned from the TT stage, which hands over to the
    /// first stage; every other move is returned past which `cur` has moved.
    pub fn trace_is_tt_move(&self) -> bool {
        self.stage == INIT_FIRST_STAGE
    }

    /// For the search trace: the name of the stage of the move last returned.
    pub fn trace_stage(&self) -> &'static str {
        match self.stage {
            INIT_FIRST_STAGE => "tt",
            GOOD_CAPTURE => "good_capture",
            GOOD_QUIET => "good_quiet",
            BAD_MOVES => "bad",
            QSEARCH => "qsearch",
            EVASION => "evasion",
            PROBCUT => "probcut",
            _ => "?",
        }
    }

    /// For the search trace: the score of the move last returned, which must
    /// not be the TT move.
    pub fn trace_score(&self) -> i32 {
        self.moves[self.cur - 1].value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ext(values: &[i32]) -> Vec<ExtMove> {
        values.iter().enumerate().map(|(i, &v)| ExtMove { mv: Move(i as u16 + 1), value: v }).collect()
    }

    #[test]
    fn partial_insertion_sort_is_the_cpp_one() {
        // Everything at or above the limit sorted, descending, ties in the
        // order they were found; the rest in the order the swaps leave it.
        let mut m = ext(&[5, -10, 7, -20, 7, 3]);
        partial_insertion_sort(&mut m, 0);
        let got: Vec<(u16, i32)> = m.iter().map(|e| (e.mv.0, e.value)).collect();
        // traced by hand on the C++ code
        assert_eq!(got, vec![(3, 7), (5, 7), (1, 5), (6, 3), (2, -10), (4, -20)]);

        // The first move counts as sorted even below the limit
        let mut m = ext(&[-5, 1, -7, 2]);
        partial_insertion_sort(&mut m, 0);
        let got: Vec<i32> = m.iter().map(|e| e.value).collect();
        assert_eq!(got, vec![2, 1, -5, -7]);
    }
}
