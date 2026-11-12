//! Port of `src/dump.cpp`: the `dump` debug command, one line of JSON per
//! position, compared with the C++ engine by `tests/differential`.
//!
//! The members are written in the same order and format as in C++. Those of
//! the layers not ported yet (`nnue`, `eval`, `order_main_shallow`,
//! `order_main_deep`, `order_qsearch`) are left out rather than faked.

use crate::movegen::MoveList;
use crate::position::Position;
use crate::types::*;
use crate::uci::move_to_string;

/// No exchange can win or lose more than a queen, hence the SEE value of any
/// move lies strictly inside (-SEE_BOUND, SEE_BOUND).
const SEE_BOUND: i32 = 4096;
const _: () = assert!(QUEEN_VALUE < SEE_BOUND);

fn append_string(out: &mut Vec<u8>, s: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push(b'"');
    for &c in s {
        if c == b'"' || c == b'\\' {
            out.push(b'\\');
            out.push(c);
        } else if c < 0x20 {
            out.extend_from_slice(b"\\u00");
            out.push(HEX[(c >> 4) as usize]);
            out.push(HEX[(c & 15) as usize]);
        } else {
            out.push(c);
        }
    }
    out.push(b'"');
}

/// Keys and bitboards are strings of 16 hexadecimal digits, because many JSON
/// parsers cannot represent a 64-bit integer exactly.
fn append_hex(out: &mut Vec<u8>, v: u64) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    out.push(b'"');
    for shift in (0..16).rev() {
        out.push(HEX[((v >> (4 * shift)) & 15) as usize]);
    }
    out.push(b'"');
}

fn append_hex_list(out: &mut Vec<u8>, values: &[u64]) {
    out.push(b'[');
    for (i, &v) in values.iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        append_hex(out, v);
    }
    out.push(b']');
}

fn append_key(out: &mut Vec<u8>, name: &str) {
    if out.last() != Some(&b'{') {
        out.push(b',');
    }
    append_string(out, name.as_bytes());
    out.push(b':');
}

fn append_int(out: &mut Vec<u8>, v: i32) {
    out.extend_from_slice(v.to_string().as_bytes());
}

fn append_list(out: &mut Vec<u8>, name: &str, moves: &MoveList, mut item: impl FnMut(&mut Vec<u8>, Move)) {
    append_key(out, name);
    out.push(b'[');
    for (i, &m) in moves.iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        item(out, m);
    }
    out.push(b']');
}

/// The exact static exchange evaluation of a move: the largest threshold for
/// which see_ge() holds, found by bisection.
pub fn see_value(pos: &Position, m: Move) -> i32 {
    let (mut lo, mut hi) = (-SEE_BOUND, SEE_BOUND);
    debug_assert!(pos.see_ge(m, lo) && !pos.see_ge(m, hi));

    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        if pos.see_ge(m, mid) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo
}

/// `PositionDumper`: owns the position it works on, so the dump never touches
/// the state of the engine.
#[derive(Default)]
pub struct PositionDumper {
    pos: Position,
}

impl PositionDumper {
    pub fn new() -> PositionDumper {
        PositionDumper { pos: Position::new() }
    }

    /// Appends the dump of the position, without the trailing newline. An
    /// invalid FEN gives an object with an "error" member.
    pub fn dump(&mut self, fen: &[u8], is_chess960: bool, out: &mut Vec<u8>) {
        out.push(b'{');

        let pos = &mut self.pos;
        if let Err(err) = pos.set(fen, is_chess960) {
            append_key(out, "error");
            append_string(out, err.what());
            append_key(out, "input");
            append_string(out, fen);
            out.push(b'}');
            return;
        }

        append_key(out, "fen");
        append_string(out, pos.fen().as_bytes());
        append_key(out, "chess960");
        out.extend_from_slice(if is_chess960 { b"true" } else { b"false" });

        // Hash keys
        append_key(out, "key");
        append_hex(out, pos.key());
        append_key(out, "pawn_key");
        append_hex(out, pos.pawn_key());
        append_key(out, "material_key");
        append_hex(out, pos.material_key());
        append_key(out, "minor_piece_key");
        append_hex(out, pos.minor_piece_key());
        append_key(out, "non_pawn_key");
        append_hex_list(out, &[pos.non_pawn_key(WHITE), pos.non_pawn_key(BLACK)]);

        // Bitboards, lists are indexed by color or by piece type from pawn to king
        append_key(out, "checkers");
        append_hex(out, pos.checkers());
        append_key(out, "blockers_for_king");
        append_hex_list(out, &[pos.blockers_for_king(WHITE), pos.blockers_for_king(BLACK)]);
        append_key(out, "pinners");
        append_hex_list(out, &[pos.pinners(WHITE), pos.pinners(BLACK)]);
        append_key(out, "check_squares");
        let cs: [u64; 6] = std::array::from_fn(|i| pos.check_squares(PAWN + i as PieceType));
        append_hex_list(out, &cs);
        append_key(out, "threats");
        let th: [u64; 6] = std::array::from_fn(|i| pos.threats_by(PAWN + i as PieceType));
        append_hex_list(out, &th);

        append_key(out, "non_pawn_material");
        out.push(b'[');
        append_int(out, pos.non_pawn_material(WHITE));
        out.push(b',');
        append_int(out, pos.non_pawn_material(BLACK));
        out.push(b']');

        // Not ported yet: "nnue" and "eval"

        // Legal moves in generation order, with the properties of each one
        let moves = MoveList::legal(pos);

        append_list(out, "moves", &moves, |out, m| append_string(out, move_to_string(m, is_chess960).as_bytes()));
        append_list(out, "see", &moves, |out, m| append_int(out, see_value(pos, m)));
        append_list(out, "gives_check", &moves, |out, m| out.push(if pos.gives_check(m) { b'1' } else { b'0' }));
        append_list(out, "key_after", &moves, |out, m| {
            pos.do_move(m);
            append_hex(out, pos.key());
            pos.undo_move(m);
        });

        // Not ported yet: "order_main_shallow", "order_main_deep", "order_qsearch"

        out.push(b'}');
    }
}

// ---------------------------------------------------------------------------
// dumpline

const PIECE_TO_CHAR: &[u8; 15] = b" PNBRQK  pnbrqk";

/// The hash keys kept by Position, as in the output of 'dump'.
fn append_keys(out: &mut Vec<u8>, pos: &Position) {
    append_key(out, "key");
    append_hex(out, pos.key());
    append_key(out, "pawn_key");
    append_hex(out, pos.pawn_key());
    append_key(out, "material_key");
    append_hex(out, pos.material_key());
    append_key(out, "minor_piece_key");
    append_hex(out, pos.minor_piece_key());
    append_key(out, "non_pawn_key");
    append_hex_list(out, &[pos.non_pawn_key(WHITE), pos.non_pawn_key(BLACK)]);
}

/// The bitboards that StateInfo caches, as in the output of 'dump'. The
/// threats are computed lazily, on first use.
fn append_cached_bitboards(out: &mut Vec<u8>, pos: &Position) {
    append_key(out, "checkers");
    append_hex(out, pos.checkers());
    append_key(out, "blockers_for_king");
    append_hex_list(out, &[pos.blockers_for_king(WHITE), pos.blockers_for_king(BLACK)]);
    append_key(out, "pinners");
    append_hex_list(out, &[pos.pinners(WHITE), pos.pinners(BLACK)]);
    append_key(out, "check_squares");
    let cs: [u64; 6] = std::array::from_fn(|i| pos.check_squares(PAWN + i as PieceType));
    append_hex_list(out, &cs);
    append_key(out, "threats");
    let th: [u64; 6] = std::array::from_fn(|i| pos.threats_by(PAWN + i as PieceType));
    append_hex_list(out, &th);
}

fn append_bool(out: &mut Vec<u8>, b: bool) {
    out.extend_from_slice(if b { b"true" } else { b"false" });
}

fn append_captured(out: &mut Vec<u8>, pos: &Position) {
    let pc = pos.captured_piece();
    if pc != NO_PIECE {
        append_string(out, &[PIECE_TO_CHAR[pc as usize]]);
    } else {
        out.extend_from_slice(b"null");
    }
}

/// `std::istringstream >> token` over a line.
fn line_tokens(line: &[u8]) -> impl Iterator<Item = &[u8]> {
    line.split(|&c| matches!(c, b' ' | b'\t' | b'\n' | 0x0B | 0x0C | b'\r')).filter(|t| !t.is_empty())
}

/// Splits the arguments of a 'position' command into a FEN and moves. Returns
/// None if they are malformed (`parse_line` in C++).
fn parse_line(line: &[u8]) -> Option<(Vec<u8>, Vec<&[u8]>)> {
    let mut tokens = line_tokens(line);
    let mut fen = Vec::new();

    match tokens.next()? {
        b"startpos" => {
            fen.extend_from_slice(crate::uci::START_FEN.as_bytes());
            if let Some(t) = tokens.next() {
                if t != b"moves" {
                    return None;
                }
            }
        }
        b"fen" => {
            for t in tokens.by_ref() {
                if t == b"moves" {
                    break;
                }
                fen.extend_from_slice(t);
                fen.push(b' ');
            }
        }
        _ => return None,
    }
    Some((fen, tokens.collect()))
}

/// The UCI notation of a move as a sort key: at most 5 bytes.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct UciKey([u8; 5], u8);

fn uci_key(m: Move, chess960: bool) -> UciKey {
    let s = move_to_string(m, chess960);
    let mut k = UciKey([0; 5], s.len() as u8);
    k.0[..s.len()].copy_from_slice(s.as_bytes());
    k
}

impl UciKey {
    fn as_bytes(&self) -> &[u8] {
        &self.0[..self.1 as usize]
    }
}

/// The probe move of a position: a legal move other than the one of the line,
/// made and undone before the line goes on. The moves are sorted by their UCI
/// notation and grouped in classes: special moves (castling, en passant,
/// promotions), other captures, other king moves, and all moves. Depending on
/// the ply, the first non-empty class of a list of preferences is chosen, then
/// a move of that class. `excluded`, the next move of the line, is never chosen.
fn probe_move(pos: &Position, ply: i32, excluded: Move) -> Move {
    const SPECIAL: usize = 0;
    const CAPTURE: usize = 1;
    const KING_MOVE: usize = 2;
    const ANY: usize = 3;
    const PREFERENCES: [[usize; 3]; 4] =
        [[SPECIAL, CAPTURE, ANY], [KING_MOVE, CAPTURE, ANY], [CAPTURE, SPECIAL, ANY], [ANY, ANY, ANY]];

    let mut classes = [[(UciKey([0; 5], 0), Move::none()); MAX_MOVES]; 4];
    let mut sizes = [0usize; 4];
    let mut add = |c: usize, e: (UciKey, Move)| {
        classes[c][sizes[c]] = e;
        sizes[c] += 1;
    };

    for &m in &MoveList::legal(pos) {
        if m == excluded {
            continue;
        }
        let entry = (uci_key(m, pos.is_chess960()), m);
        if m.type_of() != NORMAL {
            add(SPECIAL, entry);
        } else if pos.capture(m) {
            add(CAPTURE, entry);
        } else if type_of(pos.moved_piece(m)) == KING {
            add(KING_MOVE, entry);
        }
        add(ANY, entry);
    }

    for c in PREFERENCES[(ply % 4) as usize] {
        let moves = &mut classes[c][..sizes[c]];
        if !moves.is_empty() {
            // UCI notations are distinct, a stable sort is not needed
            moves.sort_unstable_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
            return moves[(ply as usize / 4) % moves.len()].1;
        }
    }
    Move::none()
}

/// `LineDumper`: sets the start position of a line of play up once, then makes
/// the moves of the line on that same position, and prints the state reached
/// incrementally. See tests/differential/README.md.
#[derive(Default)]
pub struct LineDumper {}

impl LineDumper {
    /// The accumulator stack is reset every WINDOW plies, as at the root of
    /// each search of a game; the refresh cache is only cleared between lines.
    pub const WINDOW: i32 = 64;

    /// Whether the position at a given ply of the line is skipped: neither
    /// evaluated nor probed.
    pub const fn skipped(ply: i32) -> bool {
        ply % 8 == 3 || ply % 8 >= 6
    }

    pub fn new() -> LineDumper {
        LineDumper {}
    }

    /// Appends the dump of a line, given as the arguments of the UCI
    /// 'position' command, to `out`: one line per position, then the walk
    /// back, each followed by a newline. A line that cannot be set up or played
    /// gives a single object with an "error" member.
    pub fn dump(&mut self, line: &[u8], is_chess960: bool, out: &mut Vec<u8>) {
        let append_error = |out: &mut Vec<u8>, reason: &[u8]| {
            out.push(b'{');
            append_key(out, "error");
            append_string(out, reason);
            append_key(out, "input");
            append_string(out, line);
            out.extend_from_slice(b"}\n");
        };

        // Check the whole line first, so that a line that cannot be played
        // gives a single error object rather than a truncated dump.
        let Some((fen, tokens)) = parse_line(line) else {
            append_error(out, b"Malformed line, expected startpos or fen, then moves");
            return;
        };
        let mut pos = Position::new();
        if let Err(err) = pos.set(&fen, is_chess960) {
            append_error(out, err.what());
            return;
        }
        let mut moves = Vec::with_capacity(tokens.len());
        pos.reserve_states(tokens.len());
        for token in &tokens {
            let m = crate::uci::to_move(&pos, token);
            if m == Move::none() {
                let mut reason = b"Illegal move ".to_vec();
                reason.extend_from_slice(token);
                reason.extend_from_slice(format!(" at ply {}", moves.len() + 1).as_bytes());
                append_error(out, &reason);
                return;
            }
            moves.push(m);
            pos.do_move(m);
        }

        // Set up once, then only do_move(), undo_move() and their null move kin
        let mut pos = Position::new();
        pos.reserve_states(moves.len() + 2);
        pos.set(&fen, is_chess960).expect("checked above");

        let append_move = |out: &mut Vec<u8>, m: Move| append_string(out, move_to_string(m, is_chess960).as_bytes());

        // A move made, evaluated and undone, as search does with the children
        // of a node
        let append_child = |out: &mut Vec<u8>, pos: &mut Position, m: Move| {
            let gives_check = pos.gives_check(m);
            pos.do_move_gc(m, gives_check);
            out.push(b'{');
            append_key(out, "move");
            append_move(out, m);
            append_keys(out, pos);
            append_key(out, "checkers");
            append_hex(out, pos.checkers());
            append_key(out, "plies_from_null");
            append_int(out, pos.st().plies_from_null);
            append_key(out, "repetition");
            append_int(out, pos.st().repetition);
            out.push(b'}');
            pos.undo_move(m);
        };

        let line_size = moves.len() as i32;
        let mut root = 0;
        let mut ply = 0;

        loop {
            // A new search would start here
            if ply % Self::WINDOW == 0 {
                root = ply;
            }

            let evaluate = !Self::skipped(ply);

            out.push(b'{');
            append_key(out, "ply");
            append_int(out, ply);
            append_key(out, "move");
            if ply > 0 {
                append_move(out, moves[ply as usize - 1]);
            } else {
                out.extend_from_slice(b"null");
            }
            append_key(out, "fen");
            append_string(out, pos.fen().as_bytes());
            append_keys(out, &pos);
            append_key(out, "non_pawn_material");
            out.push(b'[');
            append_int(out, pos.non_pawn_material(WHITE));
            out.push(b',');
            append_int(out, pos.non_pawn_material(BLACK));
            out.push(b']');
            append_cached_bitboards(out, &pos);
            append_key(out, "captured");
            append_captured(out, &pos);

            // Repetition and rule50 bookkeeping, the ply being counted from
            // the root of the current search as in the search itself
            append_key(out, "plies_from_null");
            append_int(out, pos.st().plies_from_null);
            append_key(out, "repetition");
            append_int(out, pos.st().repetition);
            append_key(out, "is_draw");
            append_bool(out, pos.is_draw(ply - root));
            append_key(out, "has_repeated");
            append_bool(out, pos.has_repeated());
            append_key(out, "upcoming_repetition");
            append_bool(out, pos.upcoming_repetition(ply - root));

            // A null move, as in null move pruning, then a reply to it
            append_key(out, "null");
            if evaluate && pos.checkers() == 0 {
                pos.do_null_move();
                out.push(b'{');
                append_key(out, "key");
                append_hex(out, pos.key());
                append_key(out, "fen");
                append_string(out, pos.fen().as_bytes());
                append_key(out, "plies_from_null");
                append_int(out, pos.st().plies_from_null);
                append_key(out, "repetition");
                append_int(out, pos.st().repetition);
                append_key(out, "captured");
                append_captured(out, &pos);
                append_key(out, "reply");
                let reply = probe_move(&pos, ply, Move::none());
                if reply != Move::none() {
                    append_child(out, &mut pos, reply);
                } else {
                    out.extend_from_slice(b"null");
                }
                out.push(b'}');
                pos.undo_null_move();
            } else {
                out.extend_from_slice(b"null");
            }

            // A move on the side, made and undone as search does with its children
            append_key(out, "probe");
            let next = if ply < line_size { moves[ply as usize] } else { Move::none() };
            let probe = if evaluate { probe_move(&pos, ply, next) } else { Move::none() };
            if probe != Move::none() {
                append_child(out, &mut pos, probe);
            } else {
                out.extend_from_slice(b"null");
            }

            out.extend_from_slice(b"}\n");

            if ply == line_size {
                break;
            }

            let m = moves[ply as usize];
            let gives_check = pos.gives_check(m);
            pos.do_move_gc(m, gives_check);
            ply += 1;
        }

        // Walk back down the line
        let (mut keys, mut fens, mut nnues) = (Vec::new(), Vec::new(), Vec::new());

        for ply in (1..=line_size).rev() {
            pos.undo_move(moves[ply as usize - 1]);

            keys.push(if keys.is_empty() { b'[' } else { b',' });
            append_hex(&mut keys, pos.key());
            fens.push(if fens.is_empty() { b'[' } else { b',' });
            append_string(&mut fens, pos.fen().as_bytes());
            nnues.push(if nnues.is_empty() { b'[' } else { b',' });
            nnues.extend_from_slice(b"null");
        }

        out.push(b'{');
        append_key(out, "undo");
        append_int(out, line_size);
        for (name, list) in [("keys", &mut keys), ("fens", &mut fens), ("nnues", &mut nnues)] {
            append_key(out, name);
            if list.is_empty() {
                out.extend_from_slice(b"[]");
            } else {
                out.extend_from_slice(list);
                out.push(b']');
            }
        }
        append_key(out, "fen");
        append_string(out, pos.fen().as_bytes());
        append_keys(out, &pos);
        out.extend_from_slice(b"}\n");
    }
}
