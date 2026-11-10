//! Checks the differential corpus cannot make: the corpus sets every position
//! up from its FEN and goes one move deep, so the incremental updates of
//! `do_move()` beyond `key` are only checked here, by comparing them with a
//! position set up from scratch along whole perft trees.

use stockfish::bitboard::Squares;
use stockfish::movegen::MoveList;
use stockfish::perft::perft;
use stockfish::position::Position;
use stockfish::types::*;

const STANDARD: &[&str] = &[
    "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1",
    "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
    "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1",
    "r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1",
    "rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8",
    "r7/4p3/5p1q/3P4/4pQ2/4pP2/6pp/R3K1kr w Q - 1 3",
    // en passant with pins and discovered checks
    "8/8/8/2k5/3Pp3/8/8/4K3 b - d3 0 1",
    "4k3/8/8/2KPp2r/8/8/8/8 w - e6 0 1",
    "8/5bk1/8/2Pp4/8/1K6/8/8 w - d6 0 1",
    // rule50 counter about to cross the key adjustment
    "4k3/8/8/8/8/8/8/R3K2R w KQ - 12 40",
];

const CHESS960: &[&str] = &[
    "bqnb1rkr/pp3ppp/3ppn2/2p5/5P2/P2P4/NPP1P1PP/BQ1BNRKR w HFhf - 2 9",
    "1rqbkrbn/1ppppp1p/1n6/p1N3p1/8/2P4P/PP1PPPP1/1RQBKRBN w FBfb - 0 9",
    "1rkr3b/1ppn3p/3pB1n1/6q1/R2P4/4N1P1/1P5P/2KRQ1B1 b Ddb - 0 14",
    "qbbnrkr1/p1pppppp/1p4n1/8/2P5/6N1/PPNPPPPP/1BRKBRQ1 b FCge - 1 3",
    "rr6/2kpp3/1ppnb1p1/p4q1p/P4P1P/1PNN2P1/2PP2Q1/1K2RR2 w E - 1 19",
];

fn set(fen: &str, chess960: bool) -> Position {
    let mut pos = Position::new();
    pos.set(fen.as_bytes(), chess960).unwrap_or_else(|e| panic!("{fen}: {}", String::from_utf8_lossy(&e.0)));
    pos
}

/// Everything the dump prints but the moves, as computed by `pos`.
fn snapshot(pos: &Position) -> Vec<u64> {
    let mut v = vec![
        pos.key(),
        pos.pawn_key(),
        pos.material_key(),
        pos.minor_piece_key(),
        pos.non_pawn_key(WHITE),
        pos.non_pawn_key(BLACK),
        pos.non_pawn_material(WHITE) as u64,
        pos.non_pawn_material(BLACK) as u64,
        pos.checkers(),
        pos.blockers_for_king(WHITE),
        pos.blockers_for_king(BLACK),
        pos.pinners(WHITE),
        pos.pinners(BLACK),
        pos.ep_square() as u64,
        pos.rule50_count() as u64,
    ];
    for pt in PAWN..=KING {
        v.push(pos.check_squares(pt));
        v.push(pos.threats_by(pt));
    }
    v
}

/// Walks the tree to `depth`, checking at every node that the incremental
/// state equals the one of the same position set up from its FEN, that
/// undo_move() restores the parent exactly, and that legal() agrees with
/// the move generator on every possible normal move.
fn walk(pos: &mut Position, depth: i32, chess960: bool, nodes: &mut u64) {
    *nodes += 1;
    let fen = pos.fen();
    let fresh = set(&fen, chess960);
    assert_eq!(snapshot(pos), snapshot(&fresh), "incremental state differs from scratch at {fen}");

    let moves = MoveList::legal(pos);
    for from in Squares(!0) {
        for to in Squares(!0) {
            let m = Move::new(from, to);
            if m.is_ok() {
                assert_eq!(pos.legal(m), moves.contains(m), "legal({}) at {fen}", m.0);
            }
        }
    }
    for &m in &moves {
        assert!(pos.legal(m), "generated move {} not legal() at {fen}", m.0);
    }

    if depth == 0 {
        return;
    }
    let before = snapshot(pos);
    for &m in &moves {
        let gives_check = pos.gives_check(m);
        pos.do_move(m);
        assert_eq!(gives_check, pos.checkers() != 0, "gives_check({}) at {fen}", m.0);
        walk(pos, depth - 1, chess960, nodes);
        pos.undo_move(m);
        assert_eq!(pos.fen(), fen);
        assert_eq!(snapshot(pos), before, "undo_move({}) at {fen}", m.0);
    }
}

#[test]
fn incremental_state_matches_from_scratch() {
    let mut nodes = 0;
    for fen in STANDARD {
        walk(&mut set(fen, false), 2, false, &mut nodes);
    }
    for fen in CHESS960 {
        walk(&mut set(fen, true), 2, true, &mut nodes);
    }
    assert!(nodes > 10_000);
}

#[test]
fn perft_small() {
    // From tests/perft.sh and https://www.chessprogramming.org/Perft_Results
    let cases: &[(&str, i32, u64, bool)] = &[
        ("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1", 4, 197281, false),
        ("r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq -", 3, 97862, false),
        ("8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - -", 5, 674624, false),
        ("r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1", 4, 422333, false),
        ("rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8", 3, 62379, false),
        ("r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10", 3, 89890, false),
        ("rr6/2kpp3/1ppn2p1/p2b1q1p/P4P1P/1PNN2P1/2PP4/1K2R2R b E - 1 20", 2, 1438, true),
        ("rr6/2kpp3/1ppn2p1/p2b1q1p/P4P1P/1PNN2P1/2PP4/1K2RR2 w E - 0 20", 3, 37340, true),
        ("rr6/2kpp3/1ppnb1p1/p2Q1q1p/P4P1P/1PNN2P1/2PP4/1K2RR2 b E - 2 19", 4, 2237725, true),
        ("rr6/2kpp3/1ppnb1p1/p4q1p/P4P1P/1PNN2P1/2PP2Q1/1K2RR2 w E - 1 19", 4, 2098209, true),
    ];
    for &(fen, depth, expected, chess960) in cases {
        let mut pos = set(fen, chess960);
        assert_eq!(perft::<true>(&mut pos, depth, &mut std::io::sink()), expected, "{fen}");
    }
}

#[test]
fn fen_round_trip_and_errors() {
    for fen in STANDARD {
        // The en passant square is only kept when the capture is legal. In
        // two of the positions it would expose the king, and the C++ engine
        // prints these FENs without it.
        let expected = fen.replace("2KPp2r/8/8/8/8 w - e6", "2KPp2r/8/8/8/8 w - -").replace("1K6/8/8 w - d6", "1K6/8/8 w - -");
        assert_eq!(set(fen, false).fen(), expected);
    }
    for fen in CHESS960 {
        assert_eq!(set(fen, true).fen(), *fen);
    }
    // Messages of the C++ engine, checked against it
    let errors: &[(&str, &str)] = &[
        ("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR", "Invalid FEN. Unexpected end of stream."),
        ("rnbqkbnr/pppppppp/9/8/8/8/PPPPPPPP/RNBQKBNR w - - 0 1", "Invalid FEN. Invalid number of squares to skip."),
        ("rnbqkbnr/ppppxppp/8/8/8/8/PPPPPPPP/RNBQKBNR w - - 0 1", "Invalid FEN. Invalid piece: x"),
        ("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR x - - 0 1", "Invalid FEN. Invalid side to move: x"),
        ("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w", "Invalid FEN. Expected whitespace after side to move."),
        ("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KX - 0 1", "Invalid FEN. Expected castling rights. Got: X"),
        ("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq z6 0 1", "Invalid FEN. Invalid en-passant square."),
        ("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 40000 1", "Unsupported position. Rule50 counter out of range."),
        ("k7/8/8/8/8/8/8/KK6 w - - 0 1", "Unsupported position. Incorrect number of kings."),
        ("k7/8/8/8/8/8/8/KQQQQQQQ w - - 0 1", "Unsupported position. King can be captured."),
        ("rnbqkbnr/ppppp1pp/8/4Pp2/8/8/PPPP1PPP/RNBQKBNR w KQkq f3 0 3", "Invalid FEN. Invalid en-passant square."),
        (" rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1", "Invalid FEN. Board state encoding ended but cursor not at end."),
    ];
    for &(fen, msg) in errors {
        let err = Position::new().set(fen.as_bytes(), false).unwrap_err();
        assert_eq!(String::from_utf8_lossy(&err.0), msg, "{fen}");
    }
    // A truncated FEN after the castling rights is accepted, as in C++
    assert_eq!(set("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq", false).fen(), STANDARD[0]);
}

#[test]
fn see_examples() {
    // Rook takes a pawn defended by a pawn: 208 - 1276
    let pos = set("4k3/8/3p4/4p3/8/8/8/4RK2 w - - 0 1", false);
    let m = Move::new(4, 36); // e1e5
    assert_eq!(stockfish::dump::see_value(&pos, m), 208 - 1276);
    // Undefended pawn
    let pos = set("4k3/8/8/4p3/8/8/8/4RK2 w - - 0 1", false);
    assert_eq!(stockfish::dump::see_value(&pos, m), 208);
}
