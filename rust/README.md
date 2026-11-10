# Rust port of Stockfish

A port of the C++ engine of this repository to Rust, aimed at devices with
very little capability (32-bit, single core, no SIMD, little memory). For now
the port must not change the engine's output at all: it reproduces the C++
engine bit for bit at commit `43e01e07` (bench 3464885), and is checked against
it with the differential harness of `tests/differential/`.

## What is ported

| Layer | C++ source | Rust | Checked by `dump` members |
| --- | --- | --- | --- |
| 1. Bitboards, attacks, FEN | `types.h`, `bitboard.h`, `attacks.{h,cpp}`, `Position::set`/`fen` | `types.rs`, `bitboard.rs`, `attacks.rs`, `position.rs` | `fen`, `chess960`, `checkers`, `blockers_for_king`, `pinners`, `check_squares`, `threats` |
| 2. Zobrist keys, material | `Position::init`, `set_state` | `position.rs` | `key`, `pawn_key`, `material_key`, `minor_piece_key`, `non_pawn_key`, `non_pawn_material` |
| 3. Move generation, do/undo | `movegen.cpp`, `do_move`, `undo_move`, `gives_check`, `legal` | `movegen.rs`, `position.rs` | `moves`, `gives_check`, `key_after`, and perft |
| 4. Static exchange evaluation | `Position::see_ge` | `position.rs` | `see` |

Not ported yet: NNUE (`nnue`, `eval`), the move picker (`order_*`), search,
transposition table, threads, and UCI beyond `setoption name UCI_Chess960`,
`position`, `go perft`, `dump`, `d` and `quit`. The `dump` output leaves the
members of those layers out.

## Build and check

    cd rust
    cargo build --release -j2
    cargo test --release -j2

    # the differential harness, only the members ported so far
    FIELDS=fen,chess960,checkers,blockers_for_king,pinners,check_squares,threats,key,pawn_key,material_key,minor_piece_key,non_pawn_key,non_pawn_material,moves,gives_check,key_after,see
    EXE=$PWD/target/release/stockfish ../tests/differential/verify.sh sample --fields $FIELDS
    EXE=$PWD/target/release/stockfish DATA=/path/to/data ../tests/differential/verify.sh full --fields $FIELDS

    # perft, the positions and node counts of tests/perft.sh
    ./perft.sh target/release/stockfish

## Design decisions

- **Same names and structure as the C++ code.** `Position`, `StateInfo`,
  `MoveList`, `generate::<CAPTURES>` and so on; module names follow the C++
  files. Chess types (`Square`, `Piece`, `Color`, ...) are integer aliases with
  the C++ constant names and values, so tables are indexed alike on both sides;
  only `Move` is a newtype. C++ templates are const generics.
- **StateInfo chain.** C++ links `StateInfo`s owned by the callers through
  `previous` pointers. Here `Position` owns them in a vector used as a stack:
  `do_move` pushes, `undo_move` pops, the current state is the last one. The
  vector is created with room for `MAX_PLY + 64` states, so making and
  unmaking moves does not allocate; a search must `reserve_states(MAX_PLY)` at
  the root of a long game. Move lists live on the stack (`[Move; 256]`).
- **Portable by default.** Sliding attacks use the "fancy magic bitboards" of
  the C++ fallback path, with the same PRNG, seeds and table sizes (841 KiB),
  so no SIMD, bit reversal or PEXT is needed. Bit counting and scanning use the
  Rust intrinsics, which compile to `popcnt`/`tzcnt` only when the target
  enables them (`RUSTFLAGS="-C target-cpu=native"`) and otherwise to portable
  code, as the C++ build without `USE_POPCNT`. On a 32-bit target the magic
  index is computed with two 32-bit multiplications and the 32-bit seeds, as
  in C++ without `IS_64BIT`; the `magic32` feature forces that variant on a
  64-bit machine so that it can be tested there.
- **Tables.** Pseudo attacks and Zobrist keys are computed at compile time
  (`const`). The magic, line and between tables are built at startup, as in
  C++ (`attacks::init()`), and are the same size as in C++.
- **Quirks kept on purpose** (each has a comment pointing at the C++): the
  move generation order, the order of the Zobrist draws, the en passant square
  kept only when a capture is legal (both in `set` and in `do_move`), castling
  encoded as king takes rook, the rule50 key adjustment, the material key
  indexing `psq[pc][8 + count]`, the lazy threats, and the parsing of FENs
  through a small emulation of `std::istringstream` (sticky failure, `noskipws`
  character reads, saturating integers), including the error messages.
- **No dependencies.** The JSON output is written by hand.
