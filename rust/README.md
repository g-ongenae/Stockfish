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
| 5. NNUE evaluation | `nnue/*`, `evaluate.{h,cpp}`, the `Dirties` of `do_move` | `nnue/`, `evaluate.rs`, `position.rs` | `nnue`, `eval` |
| Repetitions, null move, `dumpline` | `upcoming_repetition` and its cuckoo tables, `is_draw`, `has_repeated`, `do_null_move`, `dump.cpp` | `position.rs`, `dump.rs` | every member of `dumpline` |

`dump` prints 19 members: all of those of the C++ `dump` except the three of
the move picker. `dumpline` prints every member of the C++ `dumpline`.

Not ported yet: the move picker (`order_main_shallow`, `order_main_deep`,
`order_qsearch`), search, transposition table, threads, and UCI beyond
`setoption name UCI_Chess960`, `setoption name EvalFile`, `position`,
`go perft`, `dump`, `dumpline`, `d` and `quit`. The `dump` output leaves the
members of the move picker out.

## The network

The port reads the network from a file rather than embedding it: the
`EvalFile` option, `nn-18cd32e11a9c.nnue` by default, looked for as given
(relative to the working directory) and then in the directory of the binary,
as the C++ engine does for a network that is not embedded. It is loaded on the
first `dump` or `dumpline`; without it the engine stops with the same messages
as the C++ one. `go perft` and the other commands do not need it.

On the development machine the network is in the shared folder (see
`tests/differential/README.md`, "The shared folder"): link it next to the
binary rather than downloading it again.

    S=${STOCKFISH_SHARED:-$HOME/.local/share/stockfish-fork}
    ln -sf "$S/oracle/nn-18cd32e11a9c.nnue" target/release/

Elsewhere, download it into `src/` and link that:

    make -C ../src net                                  # downloads it into src/, checks its hash
    ln -sf ../../../src/nn-18cd32e11a9c.nnue target/release/

## Build and check

    cd rust
    cargo build --release -j2
    cargo test --release -j2

    # the network, found in the working directory or next to the binary (see
    # "The network": from the shared folder, or downloaded into src/)
    ln -sf "${STOCKFISH_SHARED:-$HOME/.local/share/stockfish-fork}/oracle/nn-18cd32e11a9c.nnue" target/release/

    # the differential harness, on the members ported, which differential.env
    # lists (CI reads the same file): 19 members of dump (all but the move
    # picker), every member of dumpline
    . ./differential.env
    EXE=$PWD/target/release/stockfish ../tests/differential/verify.sh sample --fields $DUMP_FIELDS
    EXE=$PWD/target/release/stockfish ../tests/differential/verify.sh lines --fields $DUMPLINE_FIELDS

    # the full gates: the golden files are read from DATA if set, else from
    # tests/differential/data, else from the shared folder; verify.sh prints
    # the binary and the folder it uses. EXE must name the port: without it
    # verify.sh checks src/stockfish, the C++ engine.
    EXE=$PWD/target/release/stockfish ../tests/differential/verify.sh full --fields $DUMP_FIELDS
    EXE=$PWD/target/release/stockfish ../tests/differential/verify.sh lines-full --fields $DUMPLINE_FIELDS

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
- **The NNUE port is the scalar path of C++, and nothing else.** No SIMD, no
  `unsafe`, no `cfg` on the CPU except the tile size below. The integer widths
  are those of C++: `i16` accumulators updated with wrapping additions of `i8`
  weights (as every C++ path does, so the order of the updates does not change
  them), `u8` outputs of the feature transformer (`clamp(a, 0, 255) *
  clamp(b, 0, 255) / 512`), `i32` sums in the layers, the activations computed
  as in the C++ scalar code (arithmetic shifts, the square in `i64`), and the
  final scaling in `i64` with divisions truncating toward zero. The C++ SIMD
  paths are designed to give the same bytes as the scalar one (their
  saturations only affect values that are clipped anyway), which the corpus
  confirms: the golden files come from the SSE4.1 build.
- **Two changes of loop order, not of arithmetic.** The C++ scalar path
  updates an accumulator one lane at a time across all the feature rows, which
  a compiler cannot vectorize. Here the rows are applied to a tile of lanes at
  a time (32 lanes when the target has SSE2, NEON or wasm SIMD for the
  compiler to use, 8 otherwise; both measured best on this machine). And the
  weights of the first layer are stored input-major, so that the sparse
  product reads contiguous weights. The values are the same; the gates check
  it.
- **Memory.** The network is read into the same arrays as in C++ (129 MiB of
  weights, read through a 64 KiB buffer, so it is never held twice), but only
  the layer stacks that the file stores are allocated, where C++ reserves all
  192. Peak resident memory of `dump` and `dumpline`: 137 MiB, against 343 to
  408 MiB for the C++ binary, which embeds the network in its executable and
  copies it into its `Network`, and allocates a hash table. Before a network is
  needed the port uses 11 MiB.
- **No allocation per move or per evaluation.** The accumulator stack
  (`MAX_PLY + 1` states, 1.1 MiB) and the refresh cache (384 entries, 0.8 MiB)
  are allocated once; the dirty piece, threat and pawn lists and the feature
  index lists are fixed-size arrays (`ValueList`), on the stack or in the
  states. Lists that would overflow in C++ (undefined behavior) panic here.
- **Same structure as C++**: `AccumulatorStack` with `push`/`pop`/`evaluate`,
  `find_last_usable_accumulator`, forward and backward incremental updates,
  `update_accumulator_hybrid` for king moves on one half of the board,
  `update_accumulator_refresh_cache`, `FeatureTransformer`,
  `NetworkArchitecture`, the three feature sets `K32H3`, `FullThreats` and
  `PP_3Wide` in their own modules, `DirtyPiece`/`DirtyThreats`/`DirtyPawnPairs`
  filled by `Position::do_move_dirties`. `update_accumulator_incremental_both`
  is two calls of the one-perspective update (C++ fuses them for speed).

## Speed and memory

Measured on a Pentium G3450 (2 cores, SSE4.1, no AVX2) shared with other
jobs, in CPU time, best of three interleaved runs; the noise is still about
20%. `examples/nnue_bench.rs` and `bench/nnue_bench.cpp` (linked against the
object files of a build of `src/`) make the same evaluations: 78,221 positions
of the corpus from scratch (`dump`: reset the stack, refresh through the
cache) and 70,379 positions along 1,000 lines (`dumpline`-like: one
accumulator push per move, evaluate every position not in check). Both print
the same checksum of the evaluations.

| Evaluations per second | from scratch | incremental |
| --- | ---: | ---: |
| C++ `x86-64-sse41-popcnt` (the golden files) | 66,000 | 170,000 |
| C++ `general-64` (its scalar path) | 10,000 | 35,000 |
| Rust, x86-64 baseline (the compiler may use SSE2) | 46,000 | 89,000 |
| C++ `general-64`, `-fno-tree-vectorize` | 8,700 | 31,600 |
| Rust, `-C no-vectorize-loops -C no-vectorize-slp`, tile of 8 | 16,100 | 32,800 |

Against its scalar counterpart the port is faster: 4.6 times from scratch and
2.6 times incrementally, the loop order above letting the compiler vectorize
it; with vectorization disabled (the closest this machine gets to a CPU
without SIMD) 1.85 times from scratch and on par incrementally. Against the
SSE4.1 build it is 1.4 times slower from scratch and 1.9 times slower
incrementally: that build uses hand-written SSE for everything, in particular
the first layer (sparse `maddubs` over the non-zero inputs) and the
accumulator updates (two rows per instruction, tiles in registers). A SIMD
path in the port would be the place to close that gap, not done: the target
CPUs have no SIMD.

    cargo build --release -j2 --example nnue_bench
    target/release/examples/nnue_bench nn-18cd32e11a9c.nnue positions.fen lines.txt

    # the C++ side: after `make -C ../src build ARCH=...`, compile with the flags
    # that build printed and link with its objects but main.o, in a directory
    # holding the network (the C++ engine embeds it at link time)
    g++ $CXXFLAGS -I../src -c bench/nnue_bench.cpp -o nnue_bench.o
    g++ $CXXFLAGS -o nnue_bench nnue_bench.o $(ls ../src/*.o | grep -v main.o) -lpthread
