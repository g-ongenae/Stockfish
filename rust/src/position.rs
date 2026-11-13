//! Port of `src/position.{h,cpp}`: the board, its state, FEN input and output,
//! hash keys, making and unmaking moves, and the static exchange evaluation.
//!
//! The C++ `Position` keeps a pointer to the current `StateInfo`, and each
//! `StateInfo` points to the previous one; the caller of `do_move()` owns the
//! storage. Here `Position` owns the chain as a vector used as a stack: the
//! current state is the last element (`st` in C++), `previous` is the element
//! before it. `do_move()` pushes and `undo_move()` pops, so nothing is
//! allocated in the hot path once the vector has the capacity for the game
//! plus the search depth (see `Position::new`).
//!
//! `do_move_dirties()` also fills the dirty piece, threat and pawn lists that
//! the NNUE accumulators are updated from. Not ported: `flip()` and the
//! pretty printer.

use std::cell::Cell;

use crate::attacks::*;
use crate::bitboard::*;
use crate::movegen::MoveList;
use crate::types::*;

/// The Zobrist keys, `namespace Zobrist` in C++.
pub struct Zobrist {
    pub psq: [[Key; SQUARE_NB]; PIECE_NB],
    pub enpassant: [Key; FILE_NB],
    pub castling: [Key; CASTLING_RIGHT_NB],
    pub side: Key,
    pub no_pawns: Key,
}

const PIECES: [Piece; 12] =
    [W_PAWN, W_KNIGHT, W_BISHOP, W_ROOK, W_QUEEN, W_KING, B_PAWN, B_KNIGHT, B_BISHOP, B_ROOK, B_QUEEN, B_KING];

const PIECE_TO_CHAR: &[u8; 15] = b" PNBRQK  pnbrqk";

/// The keys are drawn in the order of `Position::init()`, from the same seed.
/// They are part of the output (`key`, `key_after`, ...), so the order of the
/// draws matters: pieces in `Pieces` order and squares a1 to h8, then the en
/// passant files, then the 16 castling right sets, then the side and the key
/// of positions without pawns. Computed at compile time.
pub static ZOBRIST: Zobrist = {
    const fn next(s: u64) -> (u64, u64) {
        let s = s ^ (s >> 12);
        let s = s ^ (s << 25);
        let s = s ^ (s >> 27);
        (s, s.wrapping_mul(2685821657736338717))
    }
    let mut z = Zobrist {
        psq: [[0; SQUARE_NB]; PIECE_NB],
        enpassant: [0; FILE_NB],
        castling: [0; CASTLING_RIGHT_NB],
        side: 0,
        no_pawns: 0,
    };
    let mut s = 1070372u64;
    let mut r;
    let mut i = 0;
    while i < PIECES.len() {
        let mut sq = 0;
        while sq < 64 {
            (s, r) = next(s);
            z.psq[PIECES[i] as usize][sq] = r;
            sq += 1;
        }
        i += 1;
    }
    // pawns on these squares will promote
    let mut f = 0;
    while f < 8 {
        z.psq[W_PAWN as usize][SQ_A8 as usize + f] = 0;
        z.psq[B_PAWN as usize][f] = 0;
        f += 1;
    }
    f = 0;
    while f < 8 {
        (s, r) = next(s);
        z.enpassant[f] = r;
        f += 1;
    }
    let mut cr = 0;
    while cr < 16 {
        (s, r) = next(s);
        z.castling[cr] = r;
        cr += 1;
    }
    (s, r) = next(s);
    z.side = r;
    (_, r) = next(s);
    z.no_pawns = r;
    z
};

/// Marcel van Kervinck's cuckoo algorithm to detect repetitions of positions
/// for 3-fold repetition draws: two hash tables with the Zobrist hashes of the
/// reversible moves (`cuckoo` and `cuckooMove` in C++). First and second hash
/// functions for indexing them:
#[inline(always)]
const fn h1(h: Key) -> usize {
    (h & 0x1fff) as usize
}

#[inline(always)]
const fn h2(h: Key) -> usize {
    ((h >> 16) & 0x1fff) as usize
}

pub struct Cuckoo {
    pub keys: [Key; 8192],
    pub moves: [Move; 8192],
}

/// Filled as in `Position::init()`, in the same order, so that the moves land
/// in the same slots (a key may sit in either of its two slots depending on
/// the order of the insertions). Computed at compile time.
pub static CUCKOO: Cuckoo = {
    let mut c = Cuckoo { keys: [0; 8192], moves: [Move::none(); 8192] };
    let mut count = 0;
    let mut i = 0;
    while i < PIECES.len() {
        let pc = PIECES[i];
        let mut s1 = 0;
        while s1 < 64 {
            let mut s2 = s1 + 1;
            while s2 < 64 {
                // attacks_bb(type_of(pc), s1, 0) for pieces other than pawns
                if type_of(pc) != PAWN && PSEUDO_ATTACKS[type_of(pc) as usize][s1] & (1u64 << s2) != 0 {
                    let mut mv = Move::new(s1 as Square, s2 as Square);
                    let mut key = ZOBRIST.psq[pc as usize][s1] ^ ZOBRIST.psq[pc as usize][s2] ^ ZOBRIST.side;
                    let mut j = h1(key);
                    loop {
                        let (k, m) = (c.keys[j], c.moves[j]);
                        c.keys[j] = key;
                        c.moves[j] = mv;
                        key = k;
                        mv = m;
                        if mv.0 == Move::none().0 {
                            break; // Arrived at an empty slot
                        }
                        // Push the victim to its alternative slot
                        j = if j == h1(key) { h2(key) } else { h1(key) };
                    }
                    count += 1;
                }
                s2 += 1;
            }
            s1 += 1;
        }
        i += 1;
    }
    assert!(count == 3668);
    c
};

/// Information needed to restore a position to its previous state when a
/// move is retracted. The first group of members is copied by `do_move()`,
/// the second one is recomputed.
#[derive(Clone, Default)]
pub struct StateInfo {
    // Copied when making a move
    pub material_key: Key,
    pub pawn_key: Key,
    pub minor_piece_key: Key,
    pub non_pawn_key: [Key; COLOR_NB],
    pub non_pawn_material: [Value; COLOR_NB],
    pub castling_rights: CastlingRights,
    pub rule50: i32,
    pub plies_from_null: i32,
    pub ep_square: Square,

    // Not copied when making a move (will be recomputed anyhow)
    pub key: Key,
    pub checkers_bb: Bitboard,
    pub blockers_for_king: [Bitboard; COLOR_NB],
    pub pinners: [Bitboard; COLOR_NB],
    pub check_squares: [Bitboard; PIECE_TYPE_NB],
    /// Computed on first use by `threats_by()`, as in C++; zero until then.
    pub threats: [Cell<Bitboard>; PIECE_TYPE_NB],
    pub captured_piece: Piece,
    pub repetition: i32,
}

impl StateInfo {
    /// The state `do_move()` starts from: the copied members of `prev`, the
    /// others zero (C++ copies up to `offsetof(StateInfo, key)`).
    #[inline(always)]
    fn copied_from(prev: &StateInfo) -> StateInfo {
        StateInfo {
            material_key: prev.material_key,
            pawn_key: prev.pawn_key,
            minor_piece_key: prev.minor_piece_key,
            non_pawn_key: prev.non_pawn_key,
            non_pawn_material: prev.non_pawn_material,
            castling_rights: prev.castling_rights,
            rule50: prev.rule50,
            plies_from_null: prev.plies_from_null,
            ep_square: prev.ep_square,
            ..StateInfo::default()
        }
    }
}

/// Raised for a FEN the engine does not accept. The message is kept as bytes
/// because, as in C++, it may quote an arbitrary byte of the input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionSetError(pub Vec<u8>);

impl PositionSetError {
    fn new(msg: &str) -> PositionSetError {
        PositionSetError(msg.as_bytes().to_vec())
    }

    fn with_byte(msg: &str, b: u8) -> PositionSetError {
        let mut v = msg.as_bytes().to_vec();
        v.push(b);
        PositionSetError(v)
    }

    pub fn what(&self) -> &[u8] {
        &self.0
    }
}

/// `isspace()` of the C locale, which unlike `u8::is_ascii_whitespace` also
/// accepts the vertical tab.
#[inline]
fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | 0x0B | 0x0C | b'\r')
}

/// Just enough of `std::istringstream` to parse a FEN the way `Position::set()`
/// does: unformatted character reads (`noskipws`), `std::ws` and integer
/// extraction with `skipws`, and the sticky failure state, which matters for
/// truncated FENs.
struct FenStream<'a> {
    s: &'a [u8],
    pos: usize,
    fail: bool,
}

impl FenStream<'_> {
    /// `ss >> c` with `noskipws`.
    fn get(&mut self) -> Option<u8> {
        if self.fail {
            return None;
        }
        match self.s.get(self.pos) {
            Some(&c) => {
                self.pos += 1;
                Some(c)
            }
            None => {
                self.fail = true;
                None
            }
        }
    }

    /// `ss >> std::ws`: sets only eofbit at the end, which is not a failure,
    /// but any later read fails there anyway.
    fn skip_ws(&mut self) {
        if self.fail {
            return;
        }
        while self.pos < self.s.len() && is_space(self.s[self.pos]) {
            self.pos += 1;
        }
    }

    /// `ss >> std::skipws >> n` for an int. Returns None, leaving the variable
    /// unchanged, if the stream is in a failed state or holds only whitespace;
    /// Some(0) and fails if no digits follow; saturates on overflow.
    fn read_int(&mut self) -> Option<i32> {
        if self.fail {
            return None;
        }
        self.skip_ws();
        if self.pos >= self.s.len() {
            self.fail = true;
            return None;
        }
        let mut negative = false;
        if self.s[self.pos] == b'+' || self.s[self.pos] == b'-' {
            negative = self.s[self.pos] == b'-';
            self.pos += 1;
        }
        let start = self.pos;
        let mut v: i64 = 0;
        while self.pos < self.s.len() && self.s[self.pos].is_ascii_digit() {
            v = (v * 10 + (self.s[self.pos] - b'0') as i64).min(1 << 40);
            self.pos += 1;
        }
        if self.pos == start {
            self.fail = true;
            return Some(0);
        }
        let v = if negative { -v } else { v };
        if v > i32::MAX as i64 {
            self.fail = true;
            Some(i32::MAX)
        } else if v < i32::MIN as i64 {
            self.fail = true;
            Some(i32::MIN)
        } else {
            Some(v as i32)
        }
    }
}

pub struct Position {
    board: [Piece; SQUARE_NB],
    by_type_bb: [Bitboard; PIECE_TYPE_NB],
    by_color_bb: [Bitboard; COLOR_NB],
    piece_count: [i32; PIECE_NB],
    castling_rights_mask: [CastlingRights; SQUARE_NB],
    castling_rook_square: [Square; CASTLING_RIGHT_NB],
    castling_path: [Bitboard; CASTLING_RIGHT_NB],
    /// The StateInfo chain, current state last.
    states: Vec<StateInfo>,
    game_ply: i32,
    side_to_move: Color,
    chess960: bool,
}

impl Default for Position {
    fn default() -> Self {
        Position::new()
    }
}

impl Position {
    /// An empty position, with room for `MAX_PLY` states plus some game
    /// history before the state stack has to grow.
    pub fn new() -> Position {
        let mut states = Vec::with_capacity(MAX_PLY + 64);
        states.push(StateInfo::default());
        Position {
            board: [NO_PIECE; SQUARE_NB],
            by_type_bb: [0; PIECE_TYPE_NB],
            by_color_bb: [0; COLOR_NB],
            piece_count: [0; PIECE_NB],
            castling_rights_mask: [0; SQUARE_NB],
            castling_rook_square: [SQ_A1; CASTLING_RIGHT_NB],
            castling_path: [0; CASTLING_RIGHT_NB],
            states,
            game_ply: 0,
            side_to_move: WHITE,
            chess960: false,
        }
    }

    // ---------------------------------------------------------------------
    // Accessors (position.h)

    #[inline(always)]
    pub fn st(&self) -> &StateInfo {
        // The stack is never empty: set() and new() push the root state, and
        // undo_move() never pops it.
        self.states.last().unwrap()
    }

    #[inline(always)]
    fn st_mut(&mut self) -> &mut StateInfo {
        self.states.last_mut().unwrap()
    }

    /// The states from the root to the current one.
    pub fn states(&self) -> &[StateInfo] {
        &self.states
    }

    /// Makes sure the next `n` moves made do not allocate.
    pub fn reserve_states(&mut self, n: usize) {
        self.states.reserve(n);
    }

    #[inline(always)]
    pub fn side_to_move(&self) -> Color {
        self.side_to_move
    }

    #[inline(always)]
    pub fn piece_on(&self, s: Square) -> Piece {
        debug_assert!(is_ok_square(s));
        self.board[s as usize & 63]
    }

    #[inline(always)]
    pub fn empty(&self, s: Square) -> bool {
        self.piece_on(s) == NO_PIECE
    }

    #[inline(always)]
    pub fn moved_piece(&self, m: Move) -> Piece {
        self.piece_on(m.from_sq())
    }

    #[inline(always)]
    pub fn pieces(&self) -> Bitboard {
        self.by_type_bb[ALL_PIECES as usize]
    }

    #[inline(always)]
    pub fn pieces_pt(&self, pt: PieceType) -> Bitboard {
        self.by_type_bb[pt as usize & 7]
    }

    #[inline(always)]
    pub fn pieces_pt2(&self, pt1: PieceType, pt2: PieceType) -> Bitboard {
        self.pieces_pt(pt1) | self.pieces_pt(pt2)
    }

    #[inline(always)]
    pub fn pieces_c(&self, c: Color) -> Bitboard {
        self.by_color_bb[c as usize & 1]
    }

    #[inline(always)]
    pub fn pieces_cp(&self, c: Color, pt: PieceType) -> Bitboard {
        self.pieces_c(c) & self.pieces_pt(pt)
    }

    #[inline(always)]
    pub fn pieces_cp2(&self, c: Color, pt1: PieceType, pt2: PieceType) -> Bitboard {
        self.pieces_c(c) & self.pieces_pt2(pt1, pt2)
    }

    #[inline(always)]
    pub fn count(&self, c: Color, pt: PieceType) -> i32 {
        self.piece_count[make_piece(c, pt) as usize & 15]
    }

    #[inline(always)]
    pub fn count_both(&self, pt: PieceType) -> i32 {
        self.count(WHITE, pt) + self.count(BLACK, pt)
    }

    /// `square<KING>(c)`: the square of the king of the given color.
    #[inline(always)]
    pub fn king_square(&self, c: Color) -> Square {
        debug_assert!(self.count(c, KING) == 1);
        lsb(self.pieces_cp(c, KING))
    }

    #[inline(always)]
    pub fn ep_square(&self) -> Square {
        self.st().ep_square
    }

    #[inline(always)]
    pub fn can_castle(&self, cr: CastlingRights) -> bool {
        self.st().castling_rights & cr != 0
    }

    #[inline(always)]
    pub fn castling_impeded(&self, cr: CastlingRights) -> bool {
        debug_assert!(matches!(cr, WHITE_OO | WHITE_OOO | BLACK_OO | BLACK_OOO));
        self.pieces() & self.castling_path[cr as usize & 15] != 0
    }

    #[inline(always)]
    pub fn castling_rook_square(&self, cr: CastlingRights) -> Square {
        debug_assert!(matches!(cr, WHITE_OO | WHITE_OOO | BLACK_OO | BLACK_OOO));
        self.castling_rook_square[cr as usize & 15]
    }

    #[inline(always)]
    pub fn checkers(&self) -> Bitboard {
        self.st().checkers_bb
    }

    #[inline(always)]
    pub fn blockers_for_king(&self, c: Color) -> Bitboard {
        self.st().blockers_for_king[c as usize & 1]
    }

    #[inline(always)]
    pub fn pinners(&self, c: Color) -> Bitboard {
        self.st().pinners[c as usize & 1]
    }

    #[inline(always)]
    pub fn check_squares(&self, pt: PieceType) -> Bitboard {
        self.st().check_squares[pt as usize & 7]
    }

    #[inline(always)]
    pub fn attackers_to_sq(&self, s: Square) -> Bitboard {
        self.attackers_to(s, self.pieces())
    }

    /// Attacks of the pieces of type `pt` and color `c`, including x-rays
    /// through the king of color ~c when `occupied` leaves it out.
    #[inline(always)]
    pub fn xattacks_by(&self, pt: PieceType, c: Color, occupied: Bitboard) -> Bitboard {
        if pt == PAWN {
            pawn_attacks_bb(c, self.pieces_cp(c, PAWN))
        } else {
            let mut threats = 0;
            for s in Squares(self.pieces_cp(c, pt)) {
                threats |= attacks_bb(pt, s, occupied);
            }
            threats
        }
    }

    /// Squares attacked by the opponent's pieces of type pt (ALL_PIECES for
    /// all), sliders seeing through the king of the side to move.
    #[inline(always)]
    pub fn threats_by(&self, pt: PieceType) -> Bitboard {
        let st = self.st();
        if st.threats[ALL_PIECES as usize].get() == 0 {
            self.compute_threats();
        }
        st.threats[pt as usize & 7].get()
    }

    /// The key, including its adjustment for the rule50 counter.
    #[inline(always)]
    pub fn key(&self) -> Key {
        self.adjust_key50::<false>(self.st().key)
    }

    #[inline(always)]
    fn adjust_key50<const AFTER_MOVE: bool>(&self, k: Key) -> Key {
        let limit = 14 - AFTER_MOVE as i32;
        let rule50 = self.st().rule50;
        if rule50 < limit {
            k
        } else {
            k ^ make_key(((rule50 - limit) / 8) as u64)
        }
    }

    #[inline(always)]
    pub fn pawn_key(&self) -> Key {
        self.st().pawn_key
    }

    #[inline(always)]
    pub fn material_key(&self) -> Key {
        self.st().material_key
    }

    #[inline(always)]
    pub fn minor_piece_key(&self) -> Key {
        self.st().minor_piece_key
    }

    #[inline(always)]
    pub fn non_pawn_key(&self, c: Color) -> Key {
        self.st().non_pawn_key[c as usize & 1]
    }

    #[inline(always)]
    pub fn non_pawn_material(&self, c: Color) -> Value {
        self.st().non_pawn_material[c as usize & 1]
    }

    #[inline(always)]
    pub fn non_pawn_material_both(&self) -> Value {
        self.non_pawn_material(WHITE) + self.non_pawn_material(BLACK)
    }

    #[inline(always)]
    pub fn game_ply(&self) -> i32 {
        self.game_ply
    }

    #[inline(always)]
    pub fn rule50_count(&self) -> i32 {
        self.st().rule50
    }

    #[inline(always)]
    pub fn is_chess960(&self) -> bool {
        self.chess960
    }

    #[inline(always)]
    pub fn captured_piece(&self) -> Piece {
        self.st().captured_piece
    }

    #[inline(always)]
    pub fn capture(&self, m: Move) -> bool {
        debug_assert!(m.is_ok());
        let mt = m.type_of();
        if mt == NORMAL || mt == PROMOTION {
            !self.empty(m.to_sq())
        } else {
            mt == EN_PASSANT
        }
    }

    #[inline(always)]
    pub fn capture_stage(&self, m: Move) -> bool {
        debug_assert!(m.is_ok());
        let mt = m.type_of();
        if mt == NORMAL {
            return !self.empty(m.to_sq());
        }
        if mt == PROMOTION {
            return !self.empty(m.to_sq()) || m.promotion_type() == QUEEN;
        }
        mt == EN_PASSANT
    }

    // ---------------------------------------------------------------------
    // Board updates

    #[inline(always)]
    fn put_piece(&mut self, pc: Piece, s: Square) {
        let b = square_bb(s);
        self.board[s as usize & 63] = pc;
        self.by_type_bb[ALL_PIECES as usize] |= b;
        self.by_type_bb[type_of(pc) as usize] |= b;
        self.by_color_bb[color_of(pc) as usize & 1] |= b;
        self.piece_count[pc as usize & 15] += 1;
        self.piece_count[make_piece(color_of(pc), ALL_PIECES) as usize & 15] += 1;
    }

    /// `put_piece(pc, s, dts)`: also records the threats that appear.
    #[inline(always)]
    fn put_piece_d(&mut self, pc: Piece, s: Square, dts: Option<&mut DirtyThreats>) {
        self.put_piece(pc, s);
        if let Some(dts) = dts {
            self.update_piece_threats::<true>(pc, true, s, dts, !0);
        }
    }

    #[inline(always)]
    fn remove_piece(&mut self, s: Square) {
        let pc = self.board[s as usize & 63];
        let b = square_bb(s);
        self.by_type_bb[ALL_PIECES as usize] ^= b;
        self.by_type_bb[type_of(pc) as usize] ^= b;
        self.by_color_bb[color_of(pc) as usize & 1] ^= b;
        self.board[s as usize & 63] = NO_PIECE;
        self.piece_count[pc as usize & 15] -= 1;
        self.piece_count[make_piece(color_of(pc), ALL_PIECES) as usize & 15] -= 1;
    }

    /// `remove_piece(s, dts)`: the threats that disappear are recorded before
    /// the piece is removed.
    #[inline(always)]
    fn remove_piece_d(&mut self, s: Square, dts: Option<&mut DirtyThreats>) {
        if let Some(dts) = dts {
            let pc = self.board[s as usize & 63];
            self.update_piece_threats::<true>(pc, false, s, dts, !0);
        }
        self.remove_piece(s);
    }

    #[inline(always)]
    fn move_piece(&mut self, from: Square, to: Square) {
        let pc = self.board[from as usize & 63];
        let from_to = square_bb(from) | square_bb(to);
        self.by_type_bb[ALL_PIECES as usize] ^= from_to;
        self.by_type_bb[type_of(pc) as usize] ^= from_to;
        self.by_color_bb[color_of(pc) as usize & 1] ^= from_to;
        self.board[from as usize & 63] = NO_PIECE;
        self.board[to as usize & 63] = pc;
    }

    /// `move_piece(from, to, dts)`: the rays through both squares are left
    /// out of the discovered threats, as the piece stays on them.
    #[inline(always)]
    fn move_piece_d(&mut self, from: Square, to: Square, mut dts: Option<&mut DirtyThreats>) {
        let pc = self.board[from as usize & 63];
        let from_to = square_bb(from) | square_bb(to);
        if let Some(dts) = dts.as_deref_mut() {
            self.update_piece_threats::<true>(pc, false, from, dts, from_to);
        }
        self.move_piece(from, to);
        if let Some(dts) = dts {
            self.update_piece_threats::<true>(pc, true, to, dts, from_to);
        }
    }

    #[inline(always)]
    fn swap_piece(&mut self, s: Square, pc: Piece) {
        self.remove_piece(s);
        self.put_piece(pc, s);
    }

    /// `swap_piece(s, pc, dts)`. As in C++, the threats of the old piece are
    /// computed once it is off the board, and no discovered threats are
    /// recorded: the square stays occupied.
    #[inline(always)]
    fn swap_piece_d(&mut self, s: Square, pc: Piece, mut dts: Option<&mut DirtyThreats>) {
        let old = self.board[s as usize & 63];
        self.remove_piece(s);
        if let Some(dts) = dts.as_deref_mut() {
            self.update_piece_threats::<false>(old, false, s, dts, !0);
        }
        self.put_piece(pc, s);
        if let Some(dts) = dts {
            self.update_piece_threats::<false>(pc, true, s, dts, !0);
        }
    }

    /// Records the threats a piece put on (or removed from) square `s` makes
    /// and receives, and with COMPUTE_RAY the threats of sliders it uncovers
    /// (or blocks), except along rays containing all of `no_rays_containing`.
    /// The non-AVX512 path of `Position::update_piece_threats`; the order of
    /// the list does not matter, as the updates are additions modulo 2^16.
    fn update_piece_threats<const COMPUTE_RAY: bool>(
        &self,
        pc: Piece,
        put_piece: bool,
        s: Square,
        dts: &mut DirtyThreats,
        no_rays_containing: Bitboard,
    ) {
        #[inline(always)]
        fn can_slider_threat(pc: Piece, slider: Piece) -> bool {
            type_of(pc) != QUEEN || type_of(slider) == QUEEN
        }

        let occupied = self.pieces();
        let (b_attacks, r_attacks) = both_attacks_bb(s, occupied);
        let slider_attacks = b_attacks | r_attacks;
        let occupied_no_k = occupied ^ self.pieces_pt(KING);
        let pt = type_of(pc);
        let sliders = (self.pieces_pt2(BISHOP, QUEEN) & b_attacks) | (self.pieces_pt2(ROOK, QUEEN) & r_attacks);

        let process_sliders = |dts: &mut DirtyThreats, add_direct_attacks: bool| {
            for slider_sq in Squares(sliders) {
                let slider = self.piece_on(slider_sq);
                let ray = ray_pass_bb(slider_sq, s);
                let discovered = ray & slider_attacks & occupied_no_k;

                debug_assert!(!more_than_one(discovered));
                if discovered != 0 && (ray & no_rays_containing) != no_rays_containing {
                    let threatened_sq = lsb(discovered);
                    let threatened_pc = self.piece_on(threatened_sq);
                    if can_slider_threat(threatened_pc, slider) {
                        dts.list.push(DirtyThreat::new(slider, threatened_pc, slider_sq, threatened_sq, !put_piece));
                    }
                }

                if add_direct_attacks && can_slider_threat(pc, slider) {
                    dts.list.push(DirtyThreat::new(slider, pc, slider_sq, s, put_piece));
                }
            }
        };

        // Kings emit no direct threats
        if pt == KING {
            if COMPUTE_RAY {
                process_sliders(dts, false);
            }
            return;
        }

        let threat_targets = match pt {
            PAWN => self.pieces_pt2(KNIGHT, ROOK),
            BISHOP | ROOK => self.pieces_pt2(PAWN, KNIGHT) | self.pieces_pt2(BISHOP, ROOK),
            _ => occupied_no_k,
        };
        let threatened = match pt {
            BISHOP => b_attacks,
            ROOK => r_attacks,
            QUEEN => slider_attacks,
            PAWN => pawn_attacks(color_of(pc), s),
            _ => pseudo_attacks(pt, s),
        } & threat_targets;
        let mut incoming_threats = pseudo_attacks(KNIGHT, s) & self.pieces_pt(KNIGHT);

        if pt == KNIGHT || pt == ROOK {
            incoming_threats |= (pawn_attacks(WHITE, s) & self.pieces_cp(BLACK, PAWN))
                | (pawn_attacks(BLACK, s) & self.pieces_cp(WHITE, PAWN));
        }

        for threatened_sq in Squares(threatened) {
            let threatened_pc = self.piece_on(threatened_sq);
            debug_assert!(threatened_sq != s && threatened_pc != NO_PIECE);
            dts.list.push(DirtyThreat::new(pc, threatened_pc, s, threatened_sq, put_piece));
        }

        if COMPUTE_RAY {
            process_sliders(dts, true);
        } else {
            incoming_threats |= if pt == QUEEN { sliders & self.pieces_pt(QUEEN) } else { sliders };
        }

        for src_sq in Squares(incoming_threats) {
            let src_pc = self.piece_on(src_sq);
            debug_assert!(src_sq != s && src_pc != NO_PIECE);
            dts.list.push(DirtyThreat::new(src_pc, pc, src_sq, s, put_piece));
        }
    }

    /// The board as an array of 64 pieces, `piece_array()` in C++.
    #[inline(always)]
    pub fn piece_array(&self) -> &[Piece; SQUARE_NB] {
        &self.board
    }

    // ---------------------------------------------------------------------
    // Setting up a position

    /// Initializes the position from a FEN string, validated as strictly as in
    /// C++ and with the same error messages. Standard FEN, Shredder-FEN and
    /// X-FEN castling rights are accepted.
    pub fn set(&mut self, fen: &[u8], is_chess960: bool) -> Result<(), PositionSetError> {
        // std::memset(this, 0, sizeof(Position)) and of the StateInfo
        self.board = [NO_PIECE; SQUARE_NB];
        self.by_type_bb = [0; PIECE_TYPE_NB];
        self.by_color_bb = [0; COLOR_NB];
        self.piece_count = [0; PIECE_NB];
        self.castling_rights_mask = [0; SQUARE_NB];
        self.castling_rook_square = [SQ_A1; CASTLING_RIGHT_NB];
        self.castling_path = [0; CASTLING_RIGHT_NB];
        self.game_ply = 0;
        self.side_to_move = WHITE;
        self.chess960 = false;
        self.states.clear();
        self.states.push(StateInfo { ep_square: SQ_A1, ..StateInfo::default() });

        let mut ss = FenStream { s: fen, pos: 0, fail: false };

        let mut num_pieces = 0;
        let mut file: i32 = FILE_A as i32;
        let mut rank: i32 = RANK_8 as i32;

        // 1. Piece placement
        loop {
            let Some(token) = ss.get() else {
                return Err(PositionSetError::new("Invalid FEN. Unexpected end of stream."));
            };

            if is_space(token) {
                break;
            }

            if token.is_ascii_digit() {
                let diff = (token - b'0') as i32;
                if !(1..=8).contains(&diff) {
                    return Err(PositionSetError::new("Invalid FEN. Invalid number of squares to skip."));
                }
                file += diff;
                if file > FILE_NB as i32 {
                    return Err(PositionSetError::new("Invalid FEN. Invalid file reached."));
                }
            } else if token == b'/' {
                if file != FILE_NB as i32 {
                    return Err(PositionSetError::new("Invalid FEN. Trying to end rank when not at the end of it."));
                }
                rank -= 1;
                file = FILE_A as i32;
                if rank < RANK_1 as i32 {
                    return Err(PositionSetError::new("Invalid FEN. Invalid rank reached."));
                }
            } else {
                if file >= FILE_NB as i32 {
                    return Err(PositionSetError::new("Invalid FEN. Invalid file reached."));
                }
                let Some(idx) = PIECE_TO_CHAR.iter().position(|&c| c == token) else {
                    return Err(PositionSetError::with_byte("Invalid FEN. Invalid piece: ", token));
                };
                num_pieces += 1;
                if num_pieces > 32 {
                    return Err(PositionSetError::new("Invalid FEN. More than 32 pieces on the board."));
                }
                self.put_piece(idx as Piece, make_square(file as File, rank as Rank));
                file += 1;
            }
        }

        if rank != RANK_1 as i32 || file != FILE_NB as i32 {
            return Err(PositionSetError::new("Invalid FEN. Board state encoding ended but cursor not at end."));
        }

        if self.pieces_pt(PAWN) & (RANK_1_BB | RANK_8_BB) != 0 {
            return Err(PositionSetError::new("Unsupported position. Pawns on the first or eighth rank."));
        }

        if self.count(WHITE, KING) != 1 || self.count(BLACK, KING) != 1 {
            return Err(PositionSetError::new("Unsupported position. Incorrect number of kings."));
        }

        for c in [WHITE, BLACK] {
            if self.count(c, PAWN) > 8 {
                return Err(PositionSetError::new(if c == WHITE {
                    "Unsupported position. WHITE has more than 8 pawns."
                } else {
                    "Unsupported position. BLACK has more than 8 pawns."
                }));
            }
            let additional = (self.count(c, KNIGHT) - 2).max(0)
                + (self.count(c, BISHOP) - 2).max(0)
                + (self.count(c, ROOK) - 2).max(0)
                + (self.count(c, QUEEN) - 1).max(0);
            if additional > 8 - self.count(c, PAWN) {
                return Err(PositionSetError::new(if c == WHITE {
                    "Unsupported position. Too many pieces for WHITE."
                } else {
                    "Unsupported position. Too many pieces for BLACK."
                }));
            }
        }

        // 2. Active color
        let Some(token) = ss.get() else {
            return Err(PositionSetError::new("Invalid FEN. Unexpected end of stream."));
        };
        if token != b'w' && token != b'b' {
            return Err(PositionSetError::with_byte("Invalid FEN. Invalid side to move: ", token));
        }
        self.side_to_move = if token == b'w' { WHITE } else { BLACK };
        // `ss.eof()` cannot be set after a successful character read
        match ss.get() {
            Some(t) if is_space(t) => {}
            _ => return Err(PositionSetError::new("Invalid FEN. Expected whitespace after side to move.")),
        }

        // 3. Castling availability. Compatible with 3 standards: normal FEN,
        // Shredder-FEN (the files of the rooks), and X-FEN. Incorrect castling
        // rights are ignored rather than rejected.
        let mut num_castling_rights = 0;
        loop {
            let Some(mut token) = ss.get() else { break };

            if is_space(token) {
                break;
            }

            if num_castling_rights == 0 && token == b'-' {
                ss.skip_ws();
                break;
            }

            num_castling_rights += 1;
            if num_castling_rights > 4 {
                return Err(PositionSetError::new("Invalid FEN. Maximum of 4 castling rights can be specified."));
            }

            let mut rsq = SQ_NONE;
            let mut ksq = SQ_NONE;
            let c = if token.is_ascii_lowercase() { BLACK } else { WHITE };
            let rook = make_piece(c, ROOK);
            let king = make_piece(c, KING);

            token = token.to_ascii_uppercase();

            if token == b'K' || token == b'Q' {
                let dir: i32 = if token == b'K' { -1 } else { 1 };
                let mut sq = relative_square(c, if token == b'K' { SQ_H1 } else { SQ_A1 });
                // Look for a rook and a king for the castling, the king must
                // come later. Only the first rook is noted.
                for _ in 0..7 {
                    let pc = self.piece_on(sq);
                    if pc == king {
                        ksq = sq;
                        break;
                    } else if pc == rook && rsq == SQ_NONE {
                        rsq = sq;
                    }
                    sq = sq_add(sq, dir);
                }
            } else if (b'A'..=b'H').contains(&token) {
                let candidate = make_square(token - b'A', relative_rank(c, RANK_1));
                if self.piece_on(candidate) == rook {
                    rsq = candidate;
                }
                // With castling rights the king is between files B and G.
                let sq = self.king_square(c);
                if relative_rank_of(c, sq) == RANK_1 && file_of(sq) >= FILE_B && file_of(sq) <= FILE_G {
                    ksq = sq;
                }
            } else {
                return Err(PositionSetError::with_byte("Invalid FEN. Expected castling rights. Got: ", token));
            }

            if ksq != SQ_NONE && rsq != SQ_NONE {
                self.set_castling_right(c, rsq);
            }
        }

        // 4. En passant square. Kept only if an en passant capture is legal.
        let mut legal_ep = false;
        let col = ss.get().unwrap_or(b'-');
        if col != b'-' {
            let Some(row) = ss.get() else {
                return Err(PositionSetError::new("Invalid FEN. Unexpected end of stream."));
            };

            if (b'a'..=b'h').contains(&col) && row == (if self.side_to_move == WHITE { b'6' } else { b'3' }) {
                let us = self.side_to_move;
                let them = not_color(us);
                let ep = make_square(col - b'a', row - b'1');
                self.st_mut().ep_square = ep;

                let mut pawns = pawn_attacks(them, ep) & self.pieces_cp(us, PAWN);
                let target = self.pieces_cp(them, PAWN) & square_bb(sq_add(ep, pawn_push(them)));
                let occ = self.pieces() ^ target ^ square_bb(ep);

                // The en passant square is considered only if
                // a) the side to move has a pawn threatening it
                // b) there is an enemy pawn in front of it
                // c) there is no piece on it or behind it
                let enpassant = pawns != 0
                    && target != 0
                    && self.pieces() & (square_bb(ep) | square_bb(sq_add(ep, pawn_push(us)))) == 0;

                // And only if a pawn can take without leaving its king in check
                while enpassant && pawns != 0 && !legal_ep {
                    let s = pop_lsb(&mut pawns);
                    legal_ep =
                        self.attackers_to(self.king_square(us), occ ^ square_bb(s)) & self.pieces_c(them) & !target
                            == 0;
                }
            } else {
                return Err(PositionSetError::new("Invalid FEN. Invalid en-passant square."));
            }
        }

        if !legal_ep {
            self.st_mut().ep_square = SQ_NONE;
        }

        // 5-6. Halfmove clock and fullmove number
        let mut rule50 = 0;
        let mut game_ply = 0;
        if let Some(v) = ss.read_int() {
            rule50 = v;
        }
        if let Some(v) = ss.read_int() {
            game_ply = v;
        }
        self.st_mut().rule50 = rule50;

        if !(0..=32767).contains(&rule50) {
            return Err(PositionSetError::new("Unsupported position. Rule50 counter out of range."));
        }
        if !(0..=100000).contains(&game_ply) {
            return Err(PositionSetError::new("Unsupported position. Game ply out of range."));
        }

        // Convert from fullmove starting from 1 to gamePly starting from 0,
        // handle also common incorrect FEN with fullmove = 0.
        self.game_ply = (2 * (game_ply - 1)).max(0) + (self.side_to_move == BLACK) as i32;

        self.chess960 = is_chess960;
        self.set_state();

        if self.attackers_to_sq(self.king_square(not_color(self.side_to_move))) & self.pieces_c(self.side_to_move) != 0
        {
            return Err(PositionSetError::new("Unsupported position. King can be captured."));
        }

        debug_assert!(self.pos_is_ok());
        Ok(())
    }

    /// Sets castling rights given the color and the rook starting square.
    fn set_castling_right(&mut self, c: Color, rfrom: Square) {
        let kfrom = self.king_square(c);
        let cr = color_castling(c, if kfrom < rfrom { KING_SIDE } else { QUEEN_SIDE });

        self.st_mut().castling_rights |= cr;
        self.castling_rights_mask[kfrom as usize] |= cr;
        self.castling_rights_mask[rfrom as usize] |= cr;
        self.castling_rook_square[cr as usize] = rfrom;

        let kto = relative_square(c, if cr & KING_SIDE != 0 { SQ_G1 } else { SQ_C1 });
        let rto = relative_square(c, if cr & KING_SIDE != 0 { SQ_F1 } else { SQ_D1 });

        self.castling_path[cr as usize] =
            (between_bb(rfrom, rto) | between_bb(kfrom, kto)) & !(square_bb(kfrom) | square_bb(rfrom));
    }

    /// Sets the pins and the check squares. The threats are computed lazily.
    fn set_extra_bitboards(&mut self) {
        self.update_slider_blockers(WHITE);
        self.update_slider_blockers(BLACK);

        let them = not_color(self.side_to_move);
        let ksq = self.king_square(them);
        let (bishop_attacks, rook_attacks) = both_attacks_bb(ksq, self.pieces());

        let st = self.st_mut();
        st.check_squares[PAWN as usize] = pawn_attacks(them, ksq);
        st.check_squares[KNIGHT as usize] = pseudo_attacks(KNIGHT, ksq);
        st.check_squares[BISHOP as usize] = bishop_attacks;
        st.check_squares[ROOK as usize] = rook_attacks;
        st.check_squares[QUEEN as usize] = bishop_attacks | rook_attacks;
        st.check_squares[KING as usize] = 0;

        st.threats[ALL_PIECES as usize].set(0);
    }

    fn compute_threats(&self) {
        let them = not_color(self.side_to_move);
        let occ = self.pieces() ^ square_bb(self.king_square(self.side_to_move));
        let st = self.st();
        let mut all = 0;
        for pt in [PAWN, KNIGHT, BISHOP, ROOK, QUEEN, KING] {
            let t = self.xattacks_by(pt, them, occ);
            st.threats[pt as usize].set(t);
            all |= t;
        }
        st.threats[ALL_PIECES as usize].set(all);
    }

    /// Computes the hash keys and the other data that are then updated
    /// incrementally. Only used when a position is set up.
    fn set_state(&mut self) {
        let us = self.side_to_move;
        let checkers = self.attackers_to_sq(self.king_square(us)) & self.pieces_c(not_color(us));
        {
            let st = self.st_mut();
            st.key = 0;
            st.minor_piece_key = 0;
            st.non_pawn_key = [0, 0];
            st.pawn_key = ZOBRIST.no_pawns;
            st.non_pawn_material = [VALUE_ZERO, VALUE_ZERO];
            st.checkers_bb = checkers;
        }

        self.set_extra_bitboards();

        let mut key = 0;
        let mut pawn_key = ZOBRIST.no_pawns;
        let mut non_pawn_key = [0; COLOR_NB];
        let mut non_pawn_material = [VALUE_ZERO; COLOR_NB];
        let mut minor_piece_key = 0;

        for s in Squares(self.pieces()) {
            let pc = self.piece_on(s);
            let z = ZOBRIST.psq[pc as usize][s as usize];
            key ^= z;

            if type_of(pc) == PAWN {
                pawn_key ^= z;
            } else {
                non_pawn_key[color_of(pc) as usize] ^= z;

                if type_of(pc) != KING {
                    non_pawn_material[color_of(pc) as usize] += PIECE_VALUE[pc as usize];

                    if type_of(pc) <= BISHOP {
                        minor_piece_key ^= z;
                    }
                }
            }
        }

        let ep = self.st().ep_square;
        if ep != SQ_NONE {
            key ^= ZOBRIST.enpassant[file_of(ep) as usize];
        }
        if us == BLACK {
            key ^= ZOBRIST.side;
        }
        key ^= ZOBRIST.castling[self.st().castling_rights as usize];

        let material_key = self.compute_material_key();
        let st = self.st_mut();
        st.key = key;
        st.pawn_key = pawn_key;
        st.non_pawn_key = non_pawn_key;
        st.non_pawn_material = non_pawn_material;
        st.minor_piece_key = minor_piece_key;
        st.material_key = material_key;
    }

    pub fn compute_material_key(&self) -> Key {
        let mut k = 0;
        for pc in PIECES {
            for cnt in 0..self.piece_count[pc as usize] {
                k ^= ZOBRIST.psq[pc as usize][8 + cnt as usize];
            }
        }
        k
    }

    /// FEN of the position, Shredder-FEN castling rights in Chess960.
    pub fn fen(&self) -> String {
        let mut s = String::with_capacity(96);

        for r in (0..8u8).rev() {
            let mut f = 0u8;
            while f < 8 {
                let mut empty_cnt = 0;
                while f < 8 && self.empty(make_square(f, r)) {
                    empty_cnt += 1;
                    f += 1;
                }
                if empty_cnt != 0 {
                    s.push((b'0' + empty_cnt) as char);
                }
                if f < 8 {
                    s.push(PIECE_TO_CHAR[self.piece_on(make_square(f, r)) as usize] as char);
                    f += 1;
                }
            }
            if r > 0 {
                s.push('/');
            }
        }

        s += if self.side_to_move == WHITE { " w " } else { " b " };

        let rights = [(WHITE_OO, b'A', 'K'), (WHITE_OOO, b'A', 'Q'), (BLACK_OO, b'a', 'k'), (BLACK_OOO, b'a', 'q')];
        for (cr, base, letter) in rights {
            if self.can_castle(cr) {
                s.push(if self.chess960 { (base + file_of(self.castling_rook_square(cr))) as char } else { letter });
            }
        }
        if !self.can_castle(ANY_CASTLING) {
            s.push('-');
        }

        let ep = self.ep_square();
        if ep == SQ_NONE {
            s += " - ";
        } else {
            s.push(' ');
            s += &square_to_string(ep);
            s.push(' ');
        }
        s += &format!("{} {}", self.st().rule50, 1 + (self.game_ply - (self.side_to_move == BLACK) as i32) / 2);
        s
    }

    /// Computes the blockers for the king of color c and the pinners of ~c:
    /// the pieces of either color alone between the king and an enemy slider.
    fn update_slider_blockers(&mut self, c: Color) {
        let ksq = self.king_square(c);
        let them = not_color(c);

        let mut blockers = 0;
        let mut pinners = 0;

        // Snipers are sliders that attack 's' when a piece and other snipers are removed
        let snipers = ((pseudo_attacks(ROOK, ksq) & self.pieces_pt2(QUEEN, ROOK))
            | (pseudo_attacks(BISHOP, ksq) & self.pieces_pt2(QUEEN, BISHOP)))
            & self.pieces_c(them);
        let occupancy = self.pieces() ^ snipers;

        for sniper_sq in Squares(snipers) {
            let b = between_bb(ksq, sniper_sq) & occupancy;

            if b != 0 && !more_than_one(b) {
                blockers |= b;
                if b & self.pieces_c(c) != 0 {
                    pinners |= square_bb(sniper_sq);
                }
            }
        }

        let st = self.st_mut();
        st.blockers_for_king[c as usize] = blockers;
        st.pinners[them as usize] = pinners;
    }

    /// All the pieces attacking a square, with the given occupancy for sliders.
    #[inline(always)]
    pub fn attackers_to(&self, s: Square, occupied: Bitboard) -> Bitboard {
        let (bishop_attacks, rook_attacks) = both_attacks_bb(s, occupied);

        (rook_attacks & self.pieces_pt2(ROOK, QUEEN))
            | (bishop_attacks & self.pieces_pt2(BISHOP, QUEEN))
            | (pawn_attacks(BLACK, s) & self.pieces_cp(WHITE, PAWN))
            | (pawn_attacks(WHITE, s) & self.pieces_cp(BLACK, PAWN))
            | (pseudo_attacks(KNIGHT, s) & self.pieces_pt(KNIGHT))
            | (pseudo_attacks(KING, s) & self.pieces_pt(KING))
    }

    // ---------------------------------------------------------------------
    // Properties of moves

    /// Tests whether a move of unknown origin (such as a TT move) is legal.
    pub fn legal(&self, m: Move) -> bool {
        let us = self.side_to_move;
        let from = m.from_sq();
        let to = m.to_sq();
        let pc = self.moved_piece(m);

        // Use a slower but simpler function for uncommon cases
        if m.type_of() != NORMAL {
            return MoveList::legal(self).contains(m);
        }

        // Is not a promotion, so the promotion piece must be empty
        debug_assert!(m.promotion_type() == KNIGHT);

        if pc == NO_PIECE || color_of(pc) != us {
            return false;
        }

        // The destination square cannot be occupied by a friendly piece
        if self.pieces_c(us) & square_bb(to) != 0 {
            return false;
        }

        if type_of(pc) == PAWN {
            // Promotions were handled above, so the destination cannot be on the 8th/1st rank
            if (RANK_8_BB | RANK_1_BB) & square_bb(to) != 0 {
                return false;
            }

            let is_capture = pawn_attacks(us, from) & self.pieces_c(not_color(us)) & square_bb(to) != 0;
            let is_single_push = from as i32 + pawn_push(us) == to as i32 && self.empty(to);
            let is_double_push = from as i32 + 2 * pawn_push(us) == to as i32
                && relative_rank_of(us, from) == RANK_2
                && self.empty(to)
                && self.empty(sq_add(to, -pawn_push(us)));

            if !(is_capture || is_single_push || is_double_push) {
                return false;
            }
        } else if attacks_bb(type_of(pc), from, self.pieces()) & square_bb(to) == 0 {
            return false;
        }

        if type_of(pc) != KING {
            if self.checkers() != 0 {
                // In double check, only a king move can evade
                if more_than_one(self.checkers()) {
                    return false;
                }
                // The move must block the check or capture the checker
                if between_bb(self.king_square(us), lsb(self.checkers())) & square_bb(to) == 0 {
                    return false;
                }
            }
            // A non-king move is legal if and only if it is not pinned or it
            // is moving along the ray towards or away from the king.
            self.blockers_for_king(us) & square_bb(from) == 0 || line_bb(from, to) & self.pieces_cp(us, KING) != 0
        } else {
            // A king move is legal if the destination is not attacked
            square_bb(to) & self.threats_by(ALL_PIECES) == 0
        }
    }

    /// Tests whether a legal move gives check.
    pub fn gives_check(&self, m: Move) -> bool {
        debug_assert!(m.is_ok());
        debug_assert!(color_of(self.moved_piece(m)) == self.side_to_move);

        let us = self.side_to_move;
        let them = not_color(us);
        let from = m.from_sq();
        let to = m.to_sq();

        // Is there a direct check?
        if self.check_squares(type_of(self.piece_on(from))) & square_bb(to) != 0 {
            return true;
        }

        // Is there a discovered check?
        if self.blockers_for_king(them) & square_bb(from) != 0 {
            return line_bb(from, to) & self.pieces_cp(them, KING) == 0 || m.type_of() == CASTLING;
        }

        match m.type_of() {
            NORMAL => false,
            PROMOTION => {
                attacks_bb(m.promotion_type(), to, self.pieces() ^ square_bb(from)) & self.pieces_cp(them, KING) != 0
            }
            // En passant capture with check? Direct checks and ordinary
            // discovered checks are handled above, so the only case left is a
            // discovered check through the captured pawn.
            EN_PASSANT => {
                let capsq = make_square(file_of(to), rank_of(from));
                let b = (self.pieces() ^ square_bb(from) ^ square_bb(capsq)) | square_bb(to);
                let (bishop_attacks, rook_attacks) = both_attacks_bb(self.king_square(them), b);

                (rook_attacks & self.pieces_cp2(us, QUEEN, ROOK))
                    | (bishop_attacks & self.pieces_cp2(us, QUEEN, BISHOP))
                    != 0
            }
            _ => {
                // Castling is encoded as 'king captures the rook'
                let rto = relative_square(us, if to > from { SQ_F1 } else { SQ_D1 });
                self.check_squares(ROOK) & square_bb(rto) != 0
            }
        }
    }

    // ---------------------------------------------------------------------
    // Doing and undoing moves

    /// Makes a move, which must be legal, and pushes the new state.
    #[inline]
    pub fn do_move(&mut self, m: Move) {
        let gives_check = self.gives_check(m);
        self.do_move_gc(m, gives_check);
    }

    /// `do_move(m, newSt, givesCheck, ...)` without the NNUE bookkeeping.
    #[inline]
    pub fn do_move_gc(&mut self, m: Move, gives_check: bool) {
        self.do_move_impl(m, gives_check, None);
    }

    /// `do_move(m, newSt, givesCheck, dirties)`: also records in `dirties`
    /// what the NNUE accumulators need to be updated (the moved pieces, the
    /// threats that change and the pawns before and after the move).
    #[inline]
    pub fn do_move_dirties(&mut self, m: Move, gives_check: bool, dirties: &mut Dirties) {
        self.do_move_impl(m, gives_check, Some(dirties));
    }

    fn do_move_impl(&mut self, m: Move, gives_check: bool, mut dirties: Option<&mut Dirties>) {
        debug_assert!(m.is_ok());

        let mut k = self.st().key ^ ZOBRIST.side;

        // Copy the members of the old state that are not recomputed, and
        // make the new state the current one.
        let prev_blockers = self.st().blockers_for_king;
        let new_st = StateInfo::copied_from(self.st());
        self.states.push(new_st);

        // Increment ply counters. rule50 is reset to zero later on in case
        // of a capture or a pawn move.
        self.game_ply += 1;
        {
            let st = self.st_mut();
            st.rule50 += 1;
            st.plies_from_null += 1;
        }

        let us = self.side_to_move;
        let them = not_color(us);
        let from = m.from_sq();
        let mut to = m.to_sq();
        let pc = self.piece_on(from);
        let mut captured = if m.type_of() == EN_PASSANT { make_piece(them, PAWN) } else { self.piece_on(to) };

        if let Some(d) = dirties.as_deref_mut() {
            d.dirty_pawn_pairs.before = [self.pieces_cp(WHITE, PAWN), self.pieces_cp(BLACK, PAWN)];
            d.dirty_piece.pc = pc;
            d.dirty_piece.from = from;
            d.dirty_piece.to = to;
            d.dirty_piece.add_sq = SQ_NONE;
        }

        debug_assert!(color_of(pc) == us);
        debug_assert!(captured == NO_PIECE || color_of(captured) == if m.type_of() != CASTLING { them } else { us });
        debug_assert!(type_of(captured) != KING);

        let z = &ZOBRIST;

        if m.type_of() == CASTLING {
            debug_assert!(pc == make_piece(us, KING));
            debug_assert!(captured == make_piece(us, ROOK));

            let (rfrom, rto) = self.do_castling::<true>(us, from, &mut to, dirties.as_deref_mut());

            let rook_keys = z.psq[captured as usize][rfrom as usize] ^ z.psq[captured as usize][rto as usize];
            k ^= rook_keys;
            self.st_mut().non_pawn_key[us as usize] ^= rook_keys;
            captured = NO_PIECE;
        } else if captured != NO_PIECE {
            let mut capsq = to;

            // If the captured piece is a pawn, update the pawn hash key,
            // otherwise the non-pawn material.
            if type_of(captured) == PAWN {
                if m.type_of() == EN_PASSANT {
                    capsq = sq_add(capsq, -pawn_push(us));

                    debug_assert!(pc == make_piece(us, PAWN));
                    debug_assert!(to == self.st().ep_square);
                    debug_assert!(relative_rank_of(us, to) == RANK_6);
                    debug_assert!(self.piece_on(to) == NO_PIECE);
                    debug_assert!(self.piece_on(capsq) == make_piece(them, PAWN));

                    // Update the board in the en passant case, normal captures are updated later
                    self.remove_piece_d(capsq, dirties.as_deref_mut().map(|d| &mut d.dirty_threats));
                }
                self.st_mut().pawn_key ^= z.psq[captured as usize][capsq as usize];
            } else {
                let zc = z.psq[captured as usize][capsq as usize];
                let st = self.st_mut();
                st.non_pawn_material[them as usize] -= PIECE_VALUE[captured as usize];
                st.non_pawn_key[them as usize] ^= zc;
                if type_of(captured) <= BISHOP {
                    st.minor_piece_key ^= zc;
                }
            }

            if let Some(d) = dirties.as_deref_mut() {
                d.dirty_piece.remove_pc = captured;
                d.dirty_piece.remove_sq = capsq;
            }

            k ^= z.psq[captured as usize][capsq as usize];
            // The count of an en passant pawn is already decremented
            let cnt = self.piece_count[captured as usize] - (m.type_of() != EN_PASSANT) as i32;
            let st = self.st_mut();
            st.material_key ^= z.psq[captured as usize][(8 + cnt) as usize];

            // Reset rule 50 counter
            st.rule50 = 0;
        } else if let Some(d) = dirties.as_deref_mut() {
            d.dirty_piece.remove_sq = SQ_NONE;
        }

        // Update hash key
        k ^= z.psq[pc as usize][from as usize] ^ z.psq[pc as usize][to as usize];

        // Reset en passant square
        let ep = self.st().ep_square;
        if ep != SQ_NONE {
            k ^= z.enpassant[file_of(ep) as usize];
            self.st_mut().ep_square = SQ_NONE;
        }

        // Update castling rights
        {
            let mask = self.castling_rights_mask[from as usize] | self.castling_rights_mask[to as usize];
            let st = self.st_mut();
            k ^= z.castling[st.castling_rights as usize];
            st.castling_rights &= !mask;
            k ^= z.castling[st.castling_rights as usize];
        }

        if type_of(pc) == PAWN {
            // Set the en passant square only if a legal capture exists: the
            // key and the repetition detection depend on it.
            if (to ^ from) == 16 {
                let ep_square = sq_add(to, -pawn_push(us));
                let pawns = pawn_attacks(us, ep_square) & self.pieces_cp(them, PAWN);

                // If there are no pawns attacking the ep square, ep is not possible.
                if pawns != 0 {
                    let ksq = self.king_square(them);
                    let not_blockers = !prev_blockers[them as usize];
                    let no_discovery = square_bb(from) & not_blockers != 0 || file_of(from) == file_of(ksq);

                    // If the pawn gives discovered check, ep is never legal.
                    // Else, if at least one pawn was not a blocker for the
                    // enemy king or lies on the same line as the enemy king
                    // and the en passant square, a legal capture exists.
                    if no_discovery && pawns & (not_blockers | line_bb(ep_square, ksq)) != 0 {
                        self.st_mut().ep_square = ep_square;
                        k ^= z.enpassant[file_of(ep_square) as usize];
                    }
                }
            } else if m.type_of() == PROMOTION {
                let pt = m.promotion_type();
                let promotion = make_piece(us, pt);

                debug_assert!(relative_rank_of(us, to) == RANK_8);
                debug_assert!((KNIGHT..=QUEEN).contains(&pt));

                if let Some(d) = dirties.as_deref_mut() {
                    d.dirty_piece.add_pc = promotion;
                    d.dirty_piece.add_sq = to;
                    d.dirty_piece.to = SQ_NONE;
                }

                // Zobrist::psq[pc][to] is zero, so it need not be cleared
                let zp = z.psq[promotion as usize][to as usize];
                k ^= zp;
                let mk = z.psq[promotion as usize][(8 + self.piece_count[promotion as usize]) as usize]
                    ^ z.psq[pc as usize][(8 + self.piece_count[pc as usize] - 1) as usize];
                let st = self.st_mut();
                st.material_key ^= mk;
                st.non_pawn_key[us as usize] ^= zp;
                if pt <= BISHOP {
                    st.minor_piece_key ^= zp;
                }
                st.non_pawn_material[us as usize] += PIECE_VALUE[promotion as usize];
            }

            let st = self.st_mut();
            st.pawn_key ^= z.psq[pc as usize][from as usize] ^ z.psq[pc as usize][to as usize];
            st.rule50 = 0;
        } else {
            let zm = z.psq[pc as usize][from as usize] ^ z.psq[pc as usize][to as usize];
            let st = self.st_mut();
            st.non_pawn_key[us as usize] ^= zm;
            if type_of(pc) <= BISHOP {
                st.minor_piece_key ^= zm;
            }
        }

        self.st_mut().key = k;

        // Move the piece. The tricky Chess960 castling is handled earlier
        if m.type_of() != CASTLING {
            let to_pc = if m.type_of() == PROMOTION { make_piece(us, m.promotion_type()) } else { pc };

            let mut dts = dirties.as_deref_mut().map(|d| &mut d.dirty_threats);
            if captured != NO_PIECE && m.type_of() != EN_PASSANT {
                self.remove_piece_d(from, dts.as_deref_mut());
                self.swap_piece_d(to, to_pc, dts);
            } else if pc == to_pc {
                self.move_piece_d(from, to, dts);
            } else {
                self.remove_piece_d(from, dts.as_deref_mut());
                self.put_piece_d(to_pc, to, dts);
            }
        }

        let checkers = if gives_check { self.attackers_to_sq(self.king_square(them)) & self.pieces_c(us) } else { 0 };
        {
            let st = self.st_mut();
            st.captured_piece = captured;
            st.checkers_bb = checkers;
        }

        self.side_to_move = them;

        // Update king attacks used for fast check detection
        self.set_extra_bitboards();

        // The repetition info: the ply distance from the previous occurrence
        // of the same position, negative in the 3-fold case, or zero.
        let cur = self.states.len() - 1;
        let st = &self.states[cur];
        let end = st.rule50.min(st.plies_from_null);
        let mut repetition = 0;
        let mut i = 4;
        while i <= end {
            let stp = &self.states[cur - i as usize];
            if stp.key == st.key {
                repetition = if stp.repetition != 0 { -i } else { i };
                break;
            }
            i += 2;
        }
        self.states[cur].repetition = repetition;

        debug_assert!(self.pos_is_ok());

        if let Some(d) = dirties {
            d.dirty_pawn_pairs.after = [self.pieces_cp(WHITE, PAWN), self.pieces_cp(BLACK, PAWN)];

            debug_assert!(d.dirty_piece.pc != NO_PIECE);
            debug_assert!((captured != NO_PIECE || m.type_of() == CASTLING) == (d.dirty_piece.remove_sq != SQ_NONE));
            debug_assert!((d.dirty_piece.add_sq != SQ_NONE) == (m.type_of() == PROMOTION || m.type_of() == CASTLING));
        }
    }

    /// Unmakes a move, restoring the position exactly as it was before.
    pub fn undo_move(&mut self, m: Move) {
        debug_assert!(m.is_ok());

        self.side_to_move = not_color(self.side_to_move);

        let us = self.side_to_move;
        let from = m.from_sq();
        let mut to = m.to_sq();
        let mut pc = self.piece_on(to);

        debug_assert!(self.empty(from) || m.type_of() == CASTLING);
        debug_assert!(type_of(self.st().captured_piece) != KING);

        if m.type_of() == PROMOTION {
            debug_assert!(relative_rank_of(us, to) == RANK_8);
            debug_assert!(type_of(pc) == m.promotion_type());

            pc = make_piece(us, PAWN);
            self.swap_piece(to, pc);
        }

        if m.type_of() == CASTLING {
            self.do_castling::<false>(us, from, &mut to, None);
        } else {
            self.move_piece(to, from); // Put the piece back at the source square

            let captured = self.st().captured_piece;
            if captured != NO_PIECE {
                let mut capsq = to;

                if m.type_of() == EN_PASSANT {
                    capsq = sq_add(capsq, -pawn_push(us));
                    debug_assert!(type_of(pc) == PAWN);
                    debug_assert!(self.piece_on(capsq) == NO_PIECE);
                }

                self.put_piece(captured, capsq); // Restore the captured piece
            }
        }

        // Finally make the previous state the current one
        self.states.pop();
        self.game_ply -= 1;

        debug_assert!(self.pos_is_ok());
    }

    /// Does or undoes a castling move, a bit tricky in Chess960 where the
    /// origin and destination squares can overlap. On return `to` is the
    /// destination of the king; returns the origin and destination of the rook.
    fn do_castling<const DO: bool>(
        &mut self,
        us: Color,
        from: Square,
        to: &mut Square,
        dirties: Option<&mut Dirties>,
    ) -> (Square, Square) {
        let king_side = *to > from;
        let rfrom = *to; // Castling is encoded as "king captures friendly rook"
        let rto = relative_square(us, if king_side { SQ_F1 } else { SQ_D1 });
        *to = relative_square(us, if king_side { SQ_G1 } else { SQ_C1 });

        let mut dts = None;
        if let Some(d) = dirties {
            debug_assert!(DO);
            d.dirty_piece.to = *to;
            d.dirty_piece.remove_pc = make_piece(us, ROOK);
            d.dirty_piece.add_pc = make_piece(us, ROOK);
            d.dirty_piece.remove_sq = rfrom;
            d.dirty_piece.add_sq = rto;
            dts = Some(&mut d.dirty_threats);
        }

        // Remove both pieces first since squares could overlap in Chess960
        self.remove_piece_d(if DO { from } else { *to }, dts.as_deref_mut());
        self.remove_piece_d(if DO { rfrom } else { rto }, dts.as_deref_mut());
        self.put_piece_d(make_piece(us, KING), if DO { *to } else { from }, dts.as_deref_mut());
        self.put_piece_d(make_piece(us, ROOK), if DO { rto } else { rfrom }, dts);
        (rfrom, rto)
    }

    /// Flips the side to move without executing any move on the board.
    pub fn do_null_move(&mut self) {
        debug_assert!(self.checkers() == 0);

        // C++ copies up to blockersForKing: the copied members plus the key
        // and the checkers, which are zero here since the side is not in check.
        let mut new_st = StateInfo::copied_from(self.st());
        new_st.key = self.st().key;
        new_st.checkers_bb = self.st().checkers_bb;
        self.states.push(new_st);

        let st = self.st_mut();
        if st.ep_square != SQ_NONE {
            st.key ^= ZOBRIST.enpassant[file_of(st.ep_square) as usize];
            st.ep_square = SQ_NONE;
        }
        st.key ^= ZOBRIST.side;
        st.plies_from_null = 0;
        st.captured_piece = NO_PIECE;

        self.side_to_move = not_color(self.side_to_move);

        self.set_extra_bitboards();

        self.st_mut().repetition = 0;

        debug_assert!(self.pos_is_ok());
    }

    pub fn undo_null_move(&mut self) {
        debug_assert!(self.checkers() == 0);
        self.states.pop();
        self.side_to_move = not_color(self.side_to_move);
    }

    // ---------------------------------------------------------------------
    // Static exchange evaluation

    /// Tests whether the static exchange evaluation of a move is greater than
    /// or equal to the threshold, with an algorithm similar to alpha-beta
    /// pruning with a null window.
    pub fn see_ge(&self, m: Move, threshold: i32) -> bool {
        debug_assert!(m.is_ok());

        // Only deal with normal moves, assume others pass a simple SEE
        if m.type_of() != NORMAL {
            return VALUE_ZERO >= threshold;
        }

        let from = m.from_sq();
        let to = m.to_sq();

        debug_assert!(self.piece_on(from) != NO_PIECE);

        let mut swap = PIECE_VALUE[self.piece_on(to) as usize] - threshold;
        if swap < 0 {
            return false;
        }

        swap = PIECE_VALUE[self.piece_on(from) as usize] - swap;
        if swap <= 0 {
            return true;
        }

        debug_assert!(color_of(self.piece_on(from)) == self.side_to_move);
        // xoring to is important for pinned piece logic
        let mut occupied = self.pieces() ^ square_bb(from) ^ square_bb(to);
        let mut stm = self.side_to_move;
        let mut attackers = self.attackers_to(to, occupied);
        let mut res = 1;

        loop {
            stm = not_color(stm);
            attackers &= occupied;

            // If stm has no more attackers then give up: stm loses
            let mut stm_attackers = attackers & self.pieces_c(stm);
            if stm_attackers == 0 {
                break;
            }

            // Don't allow pinned pieces to attack as long as there are
            // pinners on their original square.
            if self.pinners(not_color(stm)) & occupied != 0 {
                stm_attackers &= !self.blockers_for_king(stm);
                if stm_attackers == 0 {
                    break;
                }
            }

            res ^= 1;

            // Locate and remove the next least valuable attacker, and add to
            // the bitboard 'attackers' any X-ray attackers behind it.
            let mut bb = stm_attackers & self.pieces_pt(PAWN);
            if bb != 0 {
                swap = PAWN_VALUE - swap;
                if swap < res {
                    break;
                }
                occupied ^= least_significant_square_bb(bb);
                attackers |= attacks_bb(BISHOP, to, occupied) & self.pieces_pt2(BISHOP, QUEEN);
                continue;
            }

            bb = stm_attackers & self.pieces_pt(KNIGHT);
            if bb != 0 {
                swap = KNIGHT_VALUE - swap;
                if swap < res {
                    break;
                }
                occupied ^= least_significant_square_bb(bb);
                continue;
            }

            bb = stm_attackers & self.pieces_pt(BISHOP);
            if bb != 0 {
                swap = BISHOP_VALUE - swap;
                if swap < res {
                    break;
                }
                occupied ^= least_significant_square_bb(bb);
                attackers |= attacks_bb(BISHOP, to, occupied) & self.pieces_pt2(BISHOP, QUEEN);
                continue;
            }

            bb = stm_attackers & self.pieces_pt(ROOK);
            if bb != 0 {
                swap = ROOK_VALUE - swap;
                if swap < res {
                    break;
                }
                occupied ^= least_significant_square_bb(bb);
                attackers |= attacks_bb(ROOK, to, occupied) & self.pieces_pt2(ROOK, QUEEN);
                continue;
            }

            bb = stm_attackers & self.pieces_pt(QUEEN);
            if bb != 0 {
                swap = QUEEN_VALUE - swap;
                // implies that the previous recapture was done by a higher
                // rated piece than a Queen (King is excluded)
                debug_assert!(swap >= res);
                occupied ^= least_significant_square_bb(bb);
                let (bishop_attacks, rook_attacks) = both_attacks_bb(to, occupied);
                attackers |=
                    (bishop_attacks & self.pieces_pt2(BISHOP, QUEEN)) | (rook_attacks & self.pieces_pt2(ROOK, QUEEN));
                continue;
            }

            // KING: if we "capture" with the king but the opponent still has
            // attackers, reverse the result.
            return if attackers & !self.pieces_c(stm) != 0 { res ^ 1 != 0 } else { res != 0 };
        }

        res != 0
    }

    // ---------------------------------------------------------------------
    // Draws

    /// Tests whether the position is drawn by the 50-move rule or by
    /// repetition. It does not detect stalemates.
    pub fn is_draw(&self, ply: i32) -> bool {
        if self.st().rule50 > 99 && (self.checkers() == 0 || !MoveList::legal(self).is_empty()) {
            return true;
        }
        self.is_repetition(ply)
    }

    /// A draw score if a position repeats once earlier but strictly after the
    /// root, or repeats twice before or at the root.
    pub fn is_repetition(&self, ply: i32) -> bool {
        let r = self.st().repetition;
        r != 0 && r < ply
    }

    /// Tests whether there has been at least one repetition of positions
    /// since the last capture or pawn move.
    pub fn has_repeated(&self) -> bool {
        let st = self.st();
        let mut end = st.rule50.min(st.plies_from_null);
        let mut idx = self.states.len() - 1;
        while end >= 4 {
            end -= 1;
            if self.states[idx].repetition != 0 {
                return true;
            }
            idx -= 1;
        }
        false
    }

    /// Tests whether the position has a move which draws by repetition. It
    /// matches the outcome of is_draw() over all legal moves.
    pub fn upcoming_repetition(&self, ply: i32) -> bool {
        let st = self.st();
        let end = st.rule50.min(st.plies_from_null);

        if end < 3 {
            return false;
        }

        // stp walks back two states at a time, as `stp->previous->previous`
        let cur = self.states.len() - 1;
        let original_key = st.key;
        let mut stp = cur - 1;
        let mut other = original_key ^ self.states[stp].key ^ ZOBRIST.side;

        let mut i = 3;
        while i <= end {
            stp -= 1;
            other ^= self.states[stp].key ^ self.states[stp - 1].key ^ ZOBRIST.side;
            stp -= 1;

            if other == 0 {
                let move_key = original_key ^ self.states[stp].key;
                let mut j = h1(move_key);
                let found = CUCKOO.keys[j] == move_key || {
                    j = h2(move_key);
                    CUCKOO.keys[j] == move_key
                };
                if found {
                    let mv = CUCKOO.moves[j];
                    let s1 = mv.from_sq();
                    let s2 = mv.to_sq();

                    // For nodes before or at the root, check that the move is
                    // a repetition rather than a move to the current position.
                    if (between_bb(s1, s2) ^ square_bb(s2)) & self.pieces() == 0
                        && (ply > i || self.states[stp].repetition != 0)
                    {
                        return true;
                    }
                }
            }
            i += 2;
        }
        false
    }

    // ---------------------------------------------------------------------
    // Consistency checks

    pub fn material_key_is_ok(&self) -> bool {
        self.compute_material_key() == self.st().material_key
    }

    /// Consistency checks of `pos_is_ok()`, panicking on the first failure.
    pub fn pos_is_ok(&self) -> bool {
        let us = self.side_to_move;
        let them = not_color(us);
        assert!(self.count(WHITE, KING) == 1 && self.count(BLACK, KING) == 1, "pos_is_ok: Kings");
        assert!(
            self.piece_on(self.king_square(WHITE)) == W_KING && self.piece_on(self.king_square(BLACK)) == B_KING,
            "pos_is_ok: Default"
        );
        assert!(self.ep_square() == SQ_NONE || relative_rank_of(us, self.ep_square()) == RANK_6, "pos_is_ok: Default");
        assert!(self.attackers_to_sq(self.king_square(them)) & self.pieces_c(us) == 0, "pos_is_ok: Kings");
        let ep = self.ep_square();
        if ep != SQ_NONE {
            let ksq = self.king_square(us);
            let captured = square_bb(sq_add(ep, pawn_push(them))) & self.pieces_cp(them, PAWN);
            let pawns = pawn_attacks(them, ep) & self.pieces_cp(us, PAWN);
            let potential_checkers = self.pieces_c(them) ^ captured;
            assert!(captured != 0 && pawns != 0, "pos_is_ok: En passant square");
            let occ = self.pieces() ^ captured ^ square_bb(ep);
            assert!(
                self.attackers_to(ksq, occ ^ square_bb(lsb(pawns))) & potential_checkers == 0
                    || self.attackers_to(ksq, occ ^ square_bb(msb(pawns))) & potential_checkers == 0,
                "pos_is_ok: En passant square"
            );
        }
        assert!(
            self.pieces_pt(PAWN) & (RANK_1_BB | RANK_8_BB) == 0
                && self.count(WHITE, PAWN) <= 8
                && self.count(BLACK, PAWN) <= 8,
            "pos_is_ok: Pawns"
        );
        assert!(
            self.pieces_c(WHITE) & self.pieces_c(BLACK) == 0
                && self.pieces_c(WHITE) | self.pieces_c(BLACK) == self.pieces()
                && popcount(self.pieces_c(WHITE)) <= 16
                && popcount(self.pieces_c(BLACK)) <= 16,
            "pos_is_ok: Bitboards"
        );
        for p1 in PAWN..=KING {
            for p2 in p1 + 1..=KING {
                assert!(self.pieces_pt(p1) & self.pieces_pt(p2) == 0, "pos_is_ok: Bitboards");
            }
        }
        for pc in PIECES {
            assert!(
                self.piece_count[pc as usize] == popcount(self.pieces_cp(color_of(pc), type_of(pc)))
                    && self.piece_count[pc as usize] == self.board.iter().filter(|&&p| p == pc).count() as i32,
                "pos_is_ok: Pieces"
            );
        }
        for c in [WHITE, BLACK] {
            for cr in [color_castling(c, KING_SIDE), color_castling(c, QUEEN_SIDE)] {
                if !self.can_castle(cr) {
                    continue;
                }
                assert!(
                    self.piece_on(self.castling_rook_square(cr)) == make_piece(c, ROOK)
                        && self.castling_rights_mask[self.castling_rook_square(cr) as usize] == cr
                        && self.castling_rights_mask[self.king_square(c) as usize] & cr == cr,
                    "pos_is_ok: Castling"
                );
            }
        }
        assert!(self.material_key_is_ok(), "pos_is_ok: materialKey");
        true
    }
}

/// `UCIEngine::square()`.
pub fn square_to_string(s: Square) -> String {
    let mut out = String::with_capacity(2);
    out.push((b'a' + file_of(s)) as char);
    out.push((b'1' + rank_of(s)) as char);
    out
}
