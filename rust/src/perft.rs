//! Port of `src/perft.h`.

use crate::movegen::MoveList;
use crate::position::Position;
use crate::types::*;
use crate::uci::move_to_string;
use std::io::Write;

/// Counts the leaf nodes up to the given depth. At the root it prints the
/// count below each move, as in C++.
pub fn perft<const ROOT: bool>(pos: &mut Position, depth: Depth, out: &mut dyn Write) -> u64 {
    let mut nodes = 0;
    let leaf = depth == 2;

    for &m in &MoveList::legal(pos) {
        let cnt;
        if ROOT && depth <= 1 {
            cnt = 1;
            nodes += 1;
        } else {
            pos.do_move(m);
            cnt = if leaf { MoveList::legal(pos).len() as u64 } else { perft::<false>(pos, depth - 1, out) };
            nodes += cnt;
            pos.undo_move(m);
        }
        if ROOT {
            let _ = writeln!(out, "{}: {}", move_to_string(m, pos.is_chess960()), cnt);
        }
    }
    nodes
}

/// `Benchmark::perft(fen, depth, isChess960)`.
pub fn perft_fen(fen: &[u8], depth: Depth, chess960: bool, out: &mut dyn Write) -> Result<u64, crate::position::PositionSetError> {
    let mut pos = Position::new();
    pos.set(fen, chess960)?;
    Ok(perft::<true>(&mut pos, depth, out))
}
