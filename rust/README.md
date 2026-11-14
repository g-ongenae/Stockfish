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
| 7. Search, transposition table (one thread) | `search.{h,cpp}`, `tt.{h,cpp}`, `searchtrace.{h,cpp}`, `score.cpp`, and of `thread`, `engine`, `uci`, `ucioption`, `timeman`, `benchmark` what a search without a clock needs | `search.rs`, `tt.rs`, `searchtrace.rs`, `score.rs`, `thread.rs`, `engine.rs`, `uci.rs`, `ucioption.rs`, `timeman.rs`, `benchmark.rs` | the search trace (`verify.sh trace`, `trace-uci`), `bench`, and the drivers of `tt_check` (see "The search" below) |
| 8. Time management | `timeman.{h,cpp}`, the clock side of `search.cpp` (the end of each iteration, `check_time()`, `start_searching()`), `go wtime/btime/winc/binc/movestogo` of `uci.cpp` | `timeman.rs`, `search.rs`, `uci.rs` | the drivers of `tm_check` and the searches on a clock of `tests/search/clock*` (see "Time management" below) |

`dump` prints all 22 members of the C++ `dump`, and `dumpline` every member
of the C++ `dumpline`.

Not ported yet: more than one thread, Syzygy tablebases, `eval`, `flip`,
`export_net` and the NUMA options (accepted, without effect). Ported but
checked by nothing but reading: skill levels. Checked by direct comparisons
with the reference rather than by the corpus: MultiPV above 1, `searchmoves`,
`go mate`, `go infinite`, pondering, searches on a clock; see "The search",
"What the gates do not see", and "Time management".

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
    # lists (CI reads the same file): every member of dump and of dumpline
    . ./differential.env
    EXE=$PWD/target/release/stockfish ../tests/differential/verify.sh sample --fields $DUMP_FIELDS
    EXE=$PWD/target/release/stockfish ../tests/differential/verify.sh lines --fields $DUMPLINE_FIELDS

    # the full gates: the golden files are read from DATA if set, else from
    # tests/differential/data, else from the shared folder; verify.sh prints
    # the binary and the folder it uses. EXE must name the port: without it
    # verify.sh checks src/stockfish, the C++ engine.
    EXE=$PWD/target/release/stockfish ../tests/differential/verify.sh full --fields $DUMP_FIELDS
    EXE=$PWD/target/release/stockfish ../tests/differential/verify.sh lines-full --fields $DUMPLINE_FIELDS

    # the search (see "The search"): its UCI output and its event and state
    # digests on the sample, then on the full corpus; bench; the deepest
    # searches of the corpus
    EXE=$PWD/target/release/stockfish ../tests/differential/verify.sh trace-uci
    EXE=$PWD/target/release/stockfish ../tests/differential/verify.sh trace
    EXE=$PWD/target/release/stockfish ../tests/differential/verify.sh trace-uci-full
    EXE=$PWD/target/release/stockfish ../tests/differential/verify.sh trace-full
    ./search_check.sh bench target/release/stockfish
    ./search_check.sh deep target/release/stockfish
    # what the trace does not reach, against the reference binary over UCI
    # (REF, default the shared folder's oracle/stockfish)
    ./search_check.sh compare target/release/stockfish
    # the first event that differs in session N of a corpus, against the
    # reference tracing binary
    EXE=$PWD/target/release/stockfish REF=$S/oracle/stockfish-trace \
        ../tests/differential/search_trace.py bisect ../tests/differential/sample/standard.trace.gz \
        ../tests/differential/sample/standard.trace.jsonl.gz N

    # perft, the positions and node counts of tests/perft.sh
    ./perft.sh target/release/stockfish

    # the transposition table against the C++ one (after the build of src/
    # below): the same output, which tt_check.expected holds
    ARCH=x86-64-sse41-popcnt bench/build_tt_check.sh
    cargo build --release -j2 --example tt_check
    ./search_check.sh tt target/cpp/tt_check | cmp - tt_check.expected
    ./search_check.sh tt target/release/examples/tt_check | cmp - tt_check.expected

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
  copies it into its `Network`, and allocates a hash table. (Since layer 7 the
  engine also holds its hash table and histories from the start: see "The
  search", "Memory".)
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
default; layer 7 runs its searches on a thread with 8 MiB (see "The search").

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

## The search

`search.rs` ports `search.cpp` function for function and, inside each
function, statement for statement, with the comments of C++: put the two side
by side. `tt.rs` ports `tt.cpp`, `searchtrace.rs` the C++ tracing build, and
`thread.rs`, `engine.rs`, `uci.rs`, `ucioption.rs`, `timeman.rs`, `score.rs`
and `benchmark.rs` what a search without a clock needs of their C++
counterparts: `go depth/nodes/movetime/mate/infinite/ponder/searchmoves`,
`stop`, `ponderhit`, `ucinewgame`, `setoption` with every option of C++ (same
names, types, defaults and bounds, printed by `uci` as C++ prints them),
`Clear Hash`, the `info` lines with their score formatting (`cp`, `mate`,
bounds, `wdl`), `bestmove` and `ponder`, `bench` with its arguments, and
`searchtrace`.

### Design decisions

- **The search stack, the PV lines and the move pickers are allocated once**,
  in the worker. The stack is a vector of `MAX_PLY + 10` entries (C++
  allocates the same array on the stack of `iterative_deepening()`, zeroed at
  each search: so is this one) and `ss` an index into it. It holds flat
  indices into the continuation and continuation-correction histories instead
  of the C++ pointers (layer 6's API), and a flag instead of the `pv`
  pointer: that pointer is either null or the local line of the parent node,
  so the line of a node at ply `p` is always row `p` of a table of
  `MAX_PLY + 3` lines, which each node empties on entry as the C++
  constructor of its local does. The pickers are two per ply, renewed (layer
  6's API): one for the nodes searched without an excluded move and one for
  the singular extension searches, which run at the ply of a node whose
  picker is in use; no other search runs at the ply of a live picker. There
  is no allocation per node: the root moves and their PVs are allocated per
  search.
- **The position is a copy of the UCI position, history included.** C++ sets
  the root position from the FEN and copies the last `StateInfo`, which links
  to the states of the moves played before: the same position with the same
  history, which a clone of `Position` is. Room for `MAX_PLY + 10` more states
  is reserved before the search, so the state stack does not grow in it.
- **One thread, with the seams of C++.** `Engine`, `ThreadPool`, `Worker`,
  `SearchManager`, the shared transposition table (`Arc`) and the shared
  histories (`Arc<SharedHistories>`) are there, with the C++ division of
  work. The thread layer has to make the pool hold several workers and run
  them; what they share is already shared by `&`: every field of the table
  and every entry of the shared histories is a relaxed atomic, and the
  workers' clears of the shared histories go through shared references
  (`DynStats::clear_range(&self)`, `ContinuationHistoryBlock::fill_shared`),
  which answers the finding of the review of layer 6.
- **The search runs on a thread made for it, with a stack of 8 MiB**
  (`TH_STACK_SIZE`, as C++ asks for its threads), on every target: the default
  stack of a spawned Rust thread is 2 MiB and that of a musl thread 128 KiB.
  The worker moves to that thread for the search and back. A search frame
  takes 1.7 KiB on x86-64 and a quiescence frame 1.3 KiB (most of it the
  buffers of the NNUE evaluation, inlined); a ply holds one search frame,
  sometimes up to three (the singular extension and null move verification
  searches run at the ply of their node), and the leaves a quiescence frame:
  at `MAX_PLY` that is at most about 1.3 MiB, a sixth of the stack. The
  deepest searches of the full corpus (selective depth 30 to 34,
  `tests/search/deep.*`) run in CI on every target, i586 musl included.
- **The clock** is layer 8: see "Time management" below.
- **No tablebases.** Without them C++ ranks no root move and never probes
  (the cardinality is 0); the port keeps the structure (`TbConfig`) and
  `syzygy_extend_pv()` in the form it takes without tables (the draw check at
  the end of the PV).

### The search trace in the port, and what it costs

C++ builds a second binary with `SEARCH_TRACE` defined. The port has one
binary with two instances of the search: the search functions take a
`const TRACE: bool`, every hook is `if TRACE { ... }` (the `trace!` macro of
`searchtrace.rs`), and `go` runs the instance with `false`, from which the
compiler removes the hooks; `searchtrace` runs the instance with `true`, whose
hooks test that a tracer is set, as the C++ ones test a pointer. The events
are emitted where `search.cpp` has its `TRACE(...)`, in its order, formatted
byte for byte as C++ formats them. Both instances give bench 3464885 (the
traced one through `searchtrace` on the sample and the full corpus). The
cost of the second instance is code size only: the binary is 1,207 KiB against 1,050 KiB with
`searchtrace` running the untraced instance (a local build made for the
measurement), and their `bench` speeds are the same within the noise (three
interleaved runs each: 170,047 to 174,685 nodes per second against 170,122
to 173,591).

### The transposition table

`tt.rs` keeps the C++ layout and arithmetic: 32-byte clusters of three
8-byte entries (depth offset by `DEPTH_NONE`, generation/bound/pv byte, move,
value, eval) and 21-bit keys packed in a 64-bit word; the cluster of a key is
`mul_hi64(key, clusterCount)`, through `u128` on every target (the unit test
checks it against the 32-bit-halves fallback of C++); `probe()`, `write()`
with its secondary ageing, `penalize()`, `hashfull()`, `new_search()` with
the 5-bit generation. Every field is a relaxed atomic as in C++, the keys
word as two 32-bit halves (32-bit PowerPC has no 64-bit atomics). The trace
fixes `Hash` at 2 MB; beyond it, `bench/tt_check.cpp` (the C++ table) and
`examples/tt_check.rs` print the same 35 lines over 100,000 random probes,
writes, penalties, new searches and hashfull counts at each of 1, 2, 3, 5, 7,
16 and 33 MB, checked in CI natively against a fresh build of `src/` and on
the emulated targets against `tt_check.expected`. Ten bugs planted one at a
time in `tt.rs` all change that output (the age weight of the replacement,
ties in the choice of the replaced entry, the threshold and the decisive
test of the secondary ageing, the pv bonus of the overwrite test, the floor
of `penalize()`, the kept move, the age bound of `hashfull()`, the cluster
index, the 21-bit key). The searches themselves are compared at other sizes
below.

### Floating point

Four floating-point expressions of this commit reach the search or the
output besides time management's (see "Time management", "Floating
point") and skill levels' (out of scope):

- the reduction table, `int(2872 / 128.0 * std::log(i))`: a constant table in
  the port (`REDUCTIONS`), which a unit test recomputes from the formula.
  Every exact value lies at least 0.0027 from an integer (checked at 60
  digits), so any `ln` within a relative 1e-5 of the true one gives this
  table, the C++ one included;
- `to_cp()`, `round(100 * v / a)` with `a` a cubic in the material: computed
  in doubles as in C++, same order (Rust never fuses operations, the C++
  builds have no FMA). Over all 62 material counts and every non-decisive
  value, `100 * v / a` stays at least 3.1e-7 from a half integer, and the
  double result equals the exact one (checked with rationals), so x87
  extended precision on i586 cannot change it; a unit test checks the margin
  with the target's own arithmetic, and values against the C++ output;
- the WDL model, `int(0.5 + 1000 / (1 + exp((a - v) / b)))` (`UCI_ShowWDL`):
  over every material count and every value, at least 2.5e-6 from an integer,
  so any `exp` accurate to about 1e-9 (glibc's, musl's, x87's) gives C++'s
  result; a unit test checks the margin on each target, and the `wdl` field
  is compared with the reference binary below;
- `Skill` (strength limit): ported, out of scope, checked by nothing.

### What the gates do not see, and what checks it

The corpus of the trace searches to depth 10 at most (selective depth 34),
at 2 MB of hash, with fixed options. The event counts of the corpus are in
`tests/differential/README.md` ("The corpus of the trace"); the port's
`searchtrace ... counts` gives the same since the full gate passes. Never or
almost never fired there: `immediate:max_ply` (never), `skip:root` (never:
MultiPV or `searchmoves` only), `stalemate` (198), `tt_cutoff_refused`
(738), `null_move_verification` (5,035), `tt_cutoff_rule50` (none in
Chess960). Branches without events were counted with a temporarily
instrumented build over the full corpus. Never reached there: the MultiPV
guard for aborted lines, `go mate`'s stop, the `forgottenMate` rollback (the
rollback ran 52 times, for aborted loss searches only), the depth-1 aborted
loss label, the output on aspiration failures past 10M nodes and the
`currmove` lines, the `usePreviousScore` output, the `-VALUE_INFINITE` output
fallback, the depth cap at `MAX_PLY - 1`, and `searchAgainCounter` (time
management only); rare: `is_shuffling()` (24 times), a draw in
`extract_ponder_from_tt()` (53).

So the port is also compared with the reference binary directly over UCI
(`bench/compare_uci.py`, `./search_check.sh compare`, in CI against the C++
engine the job builds): `tests/search/extra*.sessions` (mates and stalemates
at the root and in the tree, `go mate` finding and not finding its mate,
rule50 at 90 to 99 in both variants, `searchmoves`, a search of 11M nodes
with its `currmove` lines and failure outputs), `tests/search/long*.sessions`
(the bench positions to depth 18, Chess960 to depth 20, in one session as
bench runs them), and parts of the trace sample with `Hash` 1, 3, 5, 7 and
64, `UCI_ShowWDL`, MultiPV 3 and 4 (with `UCI_ShowWDL`). Every search
matches, every `info` line included. With MultiPV the guard for aborted lines
and the `usePreviousScore` output are reached (58 and 56 times); a `go mate`
that finds its mate reaches the stop.

These comparisons found the one bug the gates missed: `search_node()` kept
`ss->staticEval` in a local, but the null move verification search (depth 16
and more) runs on the same entry of the stack and leaves its own corrected
evaluation there, which C++ then reads. It showed in `go mate` searches to
depth 22 to 26 at 16 MB, was bisected with the trace at 1 MB (local builds of
both tracing binaries with that size), and is fixed.

Still checked by nothing but reading against C++: `max_ply` in search and
qsearch, the depth cap, the `forgottenMate` rollback and the depth-1 loss
label, the `-VALUE_INFINITE` output fallback, skill levels,
`syzygy_extend_pv()` without tables, and everything of more than one
thread. (Pondering, `go infinite` and `nodestime` are compared since layer
8: see "Time management".)

### Integer widths

`Value`, `Depth` and the scores are `i32` as the C++ `int`; the TT fields
are converted with the C++ truncations (`as u8`, `as i16`). Where C++ mixes
`int` and `u64` (the averages of the root moves, the bonus of
`update_all_stats()`), the signed value is converted to `u64` and the
division is unsigned, which floors instead of truncating: done the same way,
with a unit test. A debug build (overflow and bounds checks, and the C++
assertions as `debug_assert!`) runs the whole trace sample without a panic,
in CI too.

### Memory

Peak resident memory on x86-64 (`getrusage`), against the C++ binary of the
shared folder (`x86-64-sse41-popcnt`):

| | Rust | C++ |
| --- | ---: | ---: |
| started, no network loaded (`isready`) | 49 MiB | 408 MiB |
| a search with `Hash` 1 (`go depth 12`) | 168 MiB | 408 MiB |
| `bench` (`Hash` 16) | 183 MiB | 408 MiB |

What the port's is made of: the network, 129 MiB of weights and its layer
stacks (C++ embeds it in the binary and copies it, hence its 408 MiB from the
start); the histories, 27.9 MiB, which `clear()` fills with non-zero values
(half of it the pawn history, 16 MiB; the continuation history 8 MiB; see "The
tables"); the transposition table, `Hash` MiB, all of it written by its
clear as in C++; the NNUE accumulator stack (1.1 MiB) and refresh cache (0.8
MiB); the move pickers of the worker (2 per ply, 2.1 KiB each: 1 MiB); the
PV lines (124 KiB); the attack tables (0.8 MiB). The search thread's 8 MiB
stack is reserved, not resident: a search uses a few tens of KiB of it.
On the target devices the network is the budget: the histories and the
table come next and are what an option could shrink, but any change of their
sizes changes the output.

### Speed

`bench` (49 positions, depth 13, 3,464,885 nodes), CPU time, three
interleaved runs on the Pentium G3450 of the development machine, spread
about 2%:

| | Nodes per second | |
| --- | ---: | --- |
| C++ `x86-64-sse41-popcnt` (the oracle) | 377,000 to 379,000 | 2.2 times the port |
| Rust, x86-64 baseline (the compiler may use SSE2) | 170,000 to 175,000 | |
| C++ `general-64` (its scalar path) | 56,300 | 3.1 times slower than the port |

The gates, same machine: `verify.sh trace` (sample) 56 s against 29 s for
`stockfish-trace`, `trace-uci` 15 s against 9.5 s; `trace-full` 15 min 0 s
and `trace-uci-full` 3 min 49 s (the reference: about 6 and 3 minutes).

The ratio to C++ is the one of the NNUE evaluation measured with layer 5
(incremental updates 1.9 times slower than the SSE4.1 build, see "Speed and
memory"): the search spends most of its time there, and the search code
itself is not slower than C++'s in any way these numbers show. No speed work
was done in this layer beyond the design (no allocation per node, pickers
renewed rather than created, the trace compiled out). Where the next speed
work should go, in order of evidence:

1. **Profile on the target class first.** This machine has no profiler; the
   numbers above are totals. A `perf` profile of `bench` (x86-64 and an
   ARMv7 board) would say how the time splits between the accumulator
   updates, the network layers, move generation and the picker.
2. **The NNUE accumulator updates**, the largest known gap: C++ fuses the
   two perspectives in one pass over the rows
   (`update_accumulator_incremental_both`), which the port does as two calls,
   and keeps tiles in registers; the threat features change many rows per
   move. Tiling and fusing in the scalar code helps every target, SIMD or
   not.
3. **Bounds checks** on the history and table indices, checked once per
   access in the picker's scoring and in search (README of layer 6: the
   picker is 1.15 to 1.2 times slower than C++'s); proving the indices in
   range once per move (they are `u8` squares and pieces) would remove most.
4. On the actual target devices (no SIMD at all), the comparison that
   matters is with `general-64`, which the port already beats by 3.1 times;
   measuring on such a device, under its compiler's codegen, comes before
   tuning for it.

### For the next layers

- **Threads.** `ThreadPool` holds one worker and moves it to a thread made
  for each search. Several workers need: the pool to keep them (each with
  its own search thread, or the C++ idle loop), `start_thinking()` to set up
  each, `get_best_thread()` and the voting of C++ (`start_searching()` now
  takes the only worker as the best), `nodes_searched()` summed over the
  workers *during* the search (`check_time()` reads it: today the worker's
  own `nodes`, a plain `u64`, since 32-bit PowerPC has no 64-bit atomics; a
  shared counter will need another representation there), the shared
  histories sized `next_power_of_two(threads)` per NUMA node
  (`SharedHistories::new` takes the count), each worker's `clear()` of its
  share (already through `&`), and the transposition table cleared by all.
  The table and the shared histories are already `Arc` and atomic. The
  order of the C++ code that depends on `threadIdx` (`delta`, the null move
  reduction) is ported.
- **Time management**: see "Time management", "For the next layers".
- **The remaining UCI options.** `NumaPolicy`, `Debug Log File`,
  `SyzygyPath` and the other Syzygy options are accepted and ignored (a
  `SyzygyPath` prints an `info string`); `eval`, `flip`, `export_net`,
  `compiler` are not ported. MultiPV, `searchmoves`, `go mate`,
  `UCI_ShowWDL` and other hash sizes are compared with the reference (see
  above), and since layer 8 pondering, `go infinite` and `nodestime`; skill
  levels and `UCI_LimitStrength`/`UCI_Elo` are ported and unchecked.

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
