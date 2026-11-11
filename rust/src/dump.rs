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
