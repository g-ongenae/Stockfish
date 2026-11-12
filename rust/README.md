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
| 6. Move picker, histories | `movepick.{h,cpp}`, `history.h` (every table, the correction histories included) | `movepick.rs`, `history.rs` | `order_main_shallow`, `order_main_deep`, `order_qsearch`, and the drivers of `movepick_check.sh` (see below) |

`dump` prints all 22 members of the C++ `dump`, and `dumpline` every member
of the C++ `dumpline`.

Not ported yet: search, transposition table, threads, and UCI beyond
`setoption name UCI_Chess960`, `setoption name EvalFile`, `position`,
`go perft`, `dump`, `dumpline`, `d` and `quit`. Search will write to the
history tables and drive the move picker; until then the picker is checked
beyond `dump` by the drivers of `movepick_check.sh` (see "The move picker"
below).

## The network

The port reads the network from a file rather than embedding it: the
`EvalFile` option, `nn-18cd32e11a9c.nnue` by default, looked for as given
(relative to the working directory) and then in the directory of the binary,
as the C++ engine does for a network that is not embedded. It is loaded on the
first `dump` or `dumpline`; without it the engine stops with the same messages
as the C++ one. `go perft` and the other commands do not need it. To get it:

    make -C ../src net                                  # downloads it into src/, checks its hash
    ln -sf ../../../src/nn-18cd32e11a9c.nnue target/release/

## Build and check

    cd rust
    cargo build --release -j2
    cargo test --release -j2

    # the network, found in the working directory or next to the binary
    make -C ../src net && ln -sf ../../../src/nn-18cd32e11a9c.nnue target/release/

    # the differential harness, on the members ported, which differential.env
    # lists (CI reads the same file): every member of dump and of dumpline
    . ./differential.env
    EXE=$PWD/target/release/stockfish ../tests/differential/verify.sh sample --fields $DUMP_FIELDS
    EXE=$PWD/target/release/stockfish DATA=/path/to/data ../tests/differential/verify.sh full --fields $DUMP_FIELDS

    EXE=$PWD/target/release/stockfish ../tests/differential/verify.sh lines --fields $DUMPLINE_FIELDS
    EXE=$PWD/target/release/stockfish DATA=/path/to/data ../tests/differential/verify.sh lines-full --fields $DUMPLINE_FIELDS

    # perft, the positions and node counts of tests/perft.sh
    ./perft.sh target/release/stockfish

    # the move picker against the C++ one, beyond dump (see "The move picker"):
    # build src/, then the C++ driver against its objects, and compare
    make -C ../src -j2 ARCH=x86-64-sse41-popcnt build
    ARCH=x86-64-sse41-popcnt bench/build_movepick_check.sh
    cargo build --release -j2 --example movepick_check
    EXE=target/cpp/movepick_check ./movepick_check.sh sample > cpp.txt
    EXE=target/release/examples/movepick_check ./movepick_check.sh sample | cmp - cpp.txt

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

## The move picker

`movepick.rs` and `history.rs` port `src/movepick.{h,cpp}` and `src/history.h`
completely: both constructors (main and quiescence search, ProbCut), every
stage, `score` of captures, quiets and evasions, `partial_insertion_sort` as
written, `skip_quiet_moves`, and the `trace_*` accessors of the tracing build
(compiled in always: they cost nothing unless called, and the search trace of
layer 7 needs them). At this commit there is no `Position::pseudo_legal` and no
`can_move_king_or_pawn`: both constructors check the TT move with
`Position::legal`, which takes any 16-bit value (it was ported with layer 3,
and is now checked on all of them, see below). One quirk kept: a value of type
`NORMAL` with non-zero promotion bits is judged as the plain move, so `legal`
accepts about four values per legal normal move; C++ only asserts against it
in debug builds.

### Shape of the API, for search

In C++ the picker keeps a `const Position&` and pointers to the tables, while
search makes and unmakes moves on that very position and writes to those very
tables between two calls of `next_move()` (`src/search.cpp`: the loop of
`search()` calls `do_move`, recurses, `undo_move`, and updates
`continuationHistory`, `mainHistory` and others before the next call). A Rust
picker borrowing them for its life would forbid that. So:

- `MovePicker::new(&pos, tt_move, depth, ply)` and
  `MovePicker::new_probcut(&pos, tt_move, threshold)` read the position only
  while constructing (the TT move check and the checkers, as in C++);
- `next_move(&pos, &Histories)` is given the position and a `Histories`, a
  `Copy` bundle of shared references to the tables (main, low ply, capture,
  the six continuation histories, the shared histories), built anew for each
  call. Building one is a few pointer copies; the bench below builds one per
  call.

The picker owns nothing else: its stage, its cursors (indices rather than
pointers) and its move list (`[ExtMove; 256]`, 2 KiB, on the stack of the
caller as in C++; no allocation per node). The tables are read when the moves
are scored, inside the call that generates them, which is when C++ reads them
through its pointers: the quiet moves are scored after the good captures were
searched, with the histories as they are then. It must be the same position
in the same state at every call; debug builds check its key. In layer 7 the
worker owns its tables and lends them per call; between two calls it holds
`&mut` to them freely. The search stack holds, instead of the C++ pointers
`ss->continuationHistory` and `ss->continuationCorrectionHistory`, flat
indices into `ContinuationHistoryBlock` and `ContinuationCorrectionHistory`
(`index(in_check, capture, pc, to)`, the C++ layout), with
`index(false, NO_CAPTURES, NO_PIECE, 0)` as the sentinel C++ uses.
`examples/movepick_check.rs` drives the picker exactly that way.

`MovePicker::new` must clear the move list (Rust has no uninitialized arrays
without `unsafe`), which C++ does not: about a fifth of the time of the
picker (see below). `renew()` and `renew_probcut()` re-initialize an existing
picker without touching its list, which no stage reads before writing it; a
search that keeps one picker per ply (in its stack, allocated once) and renews
it at each node does not pay for it. The drivers renew one picker for half of
their runs, across positions, and still print what C++ prints.

A search frame holds a picker of 2.1 KiB (and qsearch frames one too), as in
C++: at `MAX_PLY` the recursion needs about as much stack as C++, which runs
its search threads on 8 MiB stacks. Rust gives spawned threads 2 MiB by
default; layer 7 must ask for more.

### The tables

Every table of `history.h` is ported, with its C++ dimensions, value type
(`i16`), bound `D`, row-major layout and index functions; the correction
histories too, although only search reads them, because they share
`SharedHistories` and the update operator with the others. `entry << bonus` is
`entry.update(bonus)`: the bonus clamped to `[-D, D]`, `val + bonus - val *
|bonus| / D` in `i32` with the division truncating toward zero, converted
back to `i16`. The tables C++ declares `Shared` (`PieceToHistory`, hence the
continuation histories, the pawn history and the unified correction history)
hold `AtomicI16` read and written with relaxed loads and stores, as
`RelaxedAtomic` does (an update is a load then a store, not a
read-modify-write, as in C++), so that a later layer can share
`SharedHistories` between threads by `&` without changing its representation.
On every target of the port these are ordinary 16-bit loads and stores. The
others are plain `i16`, owned by a thread. All are allocated once, on the
heap, with the C++ sizes:

| Table | Dimensions | Size |
| --- | --- | ---: |
| `ButterflyHistory` (main history) | 2 x 65,536 | 256 KiB |
| `LowPlyHistory` | 5 x 65,536 | 640 KiB |
| `CapturePieceToHistory` | 16 x 64 x 8 | 16 KiB |
| `PieceToHistory` | 16 x 64 | 2 KiB |
| `ContinuationHistoryBlock` (shared) | 2 x 2 x 16 x 64 `PieceToHistory` | 8 MiB |
| `PawnHistory` (shared) | 8,192 per thread x 16 x 64 | 16 MiB per thread |
| `UnifiedCorrectionHistory` (shared) | 65,536 per thread x 2 x 4 | 1 MiB per thread |
| `ContinuationCorrectionHistory` | 16 x 64 `CorrectionHistory<PieceTo>` (16 x 64) | 2 MiB |
| `TTMoveHistory` | 1 | 2 bytes |

One search thread needs 27.9 MiB of them, half of it the pawn history; that
is a budget to look at for the target devices, but any change of size changes
the output. `dump` allocates what the C++ `dump` does (the first three, one
`PieceToHistory` and `SharedHistories(1)`: 25.9 MiB, all zero). Being zero
and never written, they cost no resident memory: the peak of `dump` on 200
positions is still 136 MiB (407 MiB for the C++ binary), as zeroed
allocations come from the system as untouched pages.

### What the gate does not cover, and what checks it

`dump` builds its pickers with no TT move, at ply 0, with every history entry
0, and never calls `skip_quiet_moves()`. So the 0 mismatches of the gate say
nothing about the TT stages, `legal()` on arbitrary values, the ProbCut
picker, `skip_quiet_moves()`, the low ply term at ply > 0, any index into a
table (a wrong index into zeros reads the right zero), or the update
arithmetic. Two drivers close that gap: `bench/movepick_check.cpp`, built
against the object files of a build of `src/` (so it runs the real C++
`MovePicker`, `Position::legal` and `history.h`; `movepick.cpp` is compiled
again with `SEARCH_TRACE` for the stage and score of each move, which changes
nothing else), and `examples/movepick_check.rs`, the same driver on the port.
They draw everything from the same random stream and must print the same
lines (`movepick_check.sh` runs either one):

- every table filled with distinct values that depend on each entry's position
  in the C++ layout, so that any wrong index reads a wrong value; a digest of
  each table;
- the update operator on every value of `[-D, D]` for each distinct bound and
  entry type, with 40 bonuses (`INT_MIN`, `INT_MAX`, `-D - 1`, `D + 1`, ...);
  then 200,000 updates of random entries of every table with random bonuses
  (some of any `int`), and a digest of each table again;
- for each position: `legal()` on all 65,536 values; the four correction
  entries and a pawn history entry of the position, updated and read; then 10
  pickers: main search at random depths 1 to 20 (1 to 3 half of the time)
  and plies 0 to 7, quiescence
  search, and ProbCut at random thresholds (when not in check), with TT
  moves none, legal, pseudo-legal but illegal (a pinned piece leaving its
  line, a king move to an attacked square, a move not answering a check), and
  random 16-bit values; random continuation histories, the sentinel among
  them; in a third of them `skip_quiet_moves()` before a random move (up to
  the 40th); half of them renewed rather than new; and between two calls of
  `next_move()`, as search does, the move made and unmade on the position and,
  one time in two, random updates of the entries search would update. Each
  move returned goes into a digest with its stage, its score and whether it
  is the TT move: what the `pick` records of the search trace hold;
- then every position again, with every table refilled with values rounded to
  multiples of 1000 (the "coarse" pass, see below).

On the 5,000 positions of the differential sample the two print the same
(CI checks it on x86-64 against a fresh build of `src/`, and on the emulated
targets against `movepick_check.expected`, the C++ output on 700 of them,
which the x86-64 job checks too); also on the full corpus, once, locally (see
the PR). Mutation testing shows that this comparison sees what it is meant
to. Of 19 plausible bugs planted in the port one at a time, 18 change the
output on 500 + 200 positions: a continuation history read at the wrong ply,
the low ply term divided by `2 + ply` or cut at ply 4, `skip_quiet_moves()`
keeping the bad quiets or acting only in `BAD_MOVES`, the ProbCut TT move not
checked to be a capture, the TT move not filtered from the bad moves, evasions
scored with the wrong continuation history, the good quiet threshold off by
one, the sort limit `-3559 * depth`, a wrong `threatByLesser`, the capture SEE
threshold `/ 19`, the update rounding instead of truncating, the pawn history
mask halved, a non-pawn correction entry of the other color, the sort moving
ties, `legal()` allowing double pushes from any rank or king moves along a
checking ray. The 19th, swapping `in_check` and `capture` in
`ContinuationHistoryBlock::index`, changes nothing observable (it only
permutes a private layout that every access goes through) and is rightly not
seen. The first version of the drivers missed two of them (the bad quiets kept
after `skip_quiet_moves()`, which needs a skip late in the quiet stage at
depth 3 or less, and the threshold off by one, which random values never hit
exactly); hence the skips up to the 40th move, depths 1 to 3 half of the time,
and a second, "coarse" pass over every position with all the table values
rounded to multiples of 1000, so that scores fall on thresholds and tie.

### Speed

The bench of the two drivers (`movepick_check bench <fens> 0 400`) makes the
same work on both sides, with the non-zero tables of the check: on 1,000
positions of the sample, a main picker at depths 1, 8 and 16 and a quiescence
picker on each, taking all the moves or the first three (as a node with an
early cutoff), 400 times; CPU time, best of 5 rounds, three interleaved runs
(spread about 2%; the checksums of the moves agree). Pentium G3450, C++
`x86-64-sse41-popcnt` (the picker has no SIMD path below AVX-512), Rust for
the x86-64 baseline:

| Pickers per second | all moves | first 3 moves |
| --- | ---: | ---: |
| C++ | 1,520,000 | 1,880,000 |
| Rust, a new picker each time | 919,000 | 1,143,000 |
| Rust, one picker renewed | 1,270,000 | 1,620,000 |

So the port is 1.65 times slower with `new` and 1.15 to 1.2 times with
`renew`. Not traced further (no profiler on this machine): the rest is
presumably the bounds checks on the history indices and the copy of the
generated moves from a `MoveList` into the picker (C++ does the same copy).
Move generation itself is not slower: perft runs as fast as C++ or faster.
Not tried: generating straight into the picker's list, and checking the
indices once per move instead of per table.

Still checked only by reading: the AVX-512 variant of the sort, left out (the
golden files come from the SSE4.1 build); behavior that needs more than one
thread (none is shared yet); and the C++ `assert`s, which neither release
build evaluates.

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
