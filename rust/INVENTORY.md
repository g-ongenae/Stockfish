# What the port covers of the C++ engine

This file says, row by row, what of the C++ engine at `43e01e07` (bench
3464885) the port has, built from the C++ sources (`src/`) and from running
the reference binary of the shared folder (`oracle/stockfish`, built from
`src/` with `ARCH=x86-64-sse41-popcnt`, network embedded) and the port side
by side, not from the READMEs. It replaces the sentence "Not ported yet: ..."
that `README.md` grew layer by layer.

States:

- **gated**: ported, and a gate compares it with C++ (the gate is named);
- **ungated**: ported, checked by nothing but reading;
- **no effect**: accepted without effect;
- **differs**: ported with a known difference, said in the row;
- **not ported**.

The column "master" is the state at the start of layer 10 (`2b478a41`); the
column "layer 10" the state after it (empty: unchanged). Syzygy is the next layer; NUMA binding and the network
embedded in the binary are out of scope by decision (see the rows).

The gates named here, besides those of the earlier layers (README.md):

- **commands**: `bench/compare_commands.py`: the port and the C++ engine run
  the same sessions and everything they print is compared (with the exit
  code, standard error and the files written where the case says so), in CI
  against the C++ engine the job builds, on the emulated targets against the
  digests of the C++ output kept in `commands.expected.jsonl`; the cases are
  named (`eval-*`, `export-*`, ...);
- **surface_check**: `bench/surface_check.cpp`, linked against a build of
  `src/` patched with `bench/skill_seed.patch`, and
  `examples/surface_check.rs` print the same lines (natively, with
  `softfloat`, and on the emulated targets against `surface_check.expected`);
- **seeded searches**: `bench/compare_uci.py` with a skill level, the port
  built with the feature `skill_seed` and the patched C++ engine, both seeded
  by `SF_SKILL_SEED`;
- **skill_stats**: `bench/skill_stats.py`, the unpatched engines, compared
  statistically.

## 1. Source files of `src/`

| C++ file | Port | master | layer 10 |
| --- | --- | --- | --- |
| `attacks.{h,cpp}` | `attacks.rs` | gated: `dump` (`checkers`, `pinners`, ...), perft | |
| `benchmark.{h,cpp}` | `benchmark.rs` | `setup_bench()`: gated by `bench` 3464885, but differs: the list lacks the `ucinewgame` that C++ puts after `setoption Hash` (only the "Total time" of `bench` changes, C++ restarting its clock after the clear); `setup_benchmark()` (`speedtest`) and `BenchmarkPositions`: not ported | `ucinewgame` added (its only effect is the time `bench` reports: gated by reading); `setup_benchmark()` and the positions: gated (surface_check: the commands of 20 argument lists) |
| `bitboard.{h,cpp}` | `bitboard.rs` | gated: `dump`, perft | |
| `dump.{h,cpp}` (the fork's) | `dump.rs` | gated: the four differential gates | |
| `engine.{h,cpp}` | `engine.rs` | partly: `trace_eval()`, `save_network()`, `visualize()` and the NUMA information missing; see sections 2 to 5 | gated (below) |
| `evaluate.{h,cpp}` | `evaluate.rs` | `evaluate()`, `scale_evaluation()`: gated (`dump` `eval`, the trace); `Eval::trace()`: not ported | `trace()`: gated (commands, `eval-*`) |
| `history.h` | `history.rs` | gated: movepick_check, the trace, threads_check | |
| `incbin/` | none | not ported: the network embedded in the binary (section 5) | |
| `main.cpp` | `main.rs` | differs: the start-up line (section 4); `Tune::init()` has nothing to do (no `TUNE()` in this commit) | the start-up line in the shape of C++ (section 4) |
| `Makefile` | `Cargo.toml` | the build: ARCH, PGO, the network download are the C++ build's own | |
| `memory.{h,cpp}` | none (Rust allocator) | not ported: large pages, aligned allocation (section 5); the allocation failures are those of the Rust allocator except the hash (`tt.rs` prints C++'s "Failed to allocate N MB for transposition table.") | |
| `misc.{h,cpp}` | `misc.rs`, `uci.rs`, `engine.rs` | partly: `engine_info()`, `compiler_info()`, the `Debug Log File` logger, `str_to_size_t()` not ported; `dbg_*()` not ported (no call in this commit prints anything: `dbg_print()` runs in `bench` and every second of a search, with every slot empty); `hash_bytes()` only serves the shared memory; `path_from_utf8()`/`CommandLine` (Windows conversions): std's `args_os` and paths | `engine_info()`, `compiler_info()`: in the shape of C++ (section 4), `compiler_info()` gated by shape (commands, `compiler*`); the logger: gated (commands, `log*`); `str_to_size_t()`: in `numa.rs`, gated (commands, `numa-*`) |
| `movegen.{h,cpp}` | `movegen.rs` | gated: `dump` `moves`, perft | |
| `movepick.{h,cpp}` | `movepick.rs` | gated: `dump` orderings, movepick_check, the trace | |
| `nnue/features/*` | `nnue/features/*` | gated: `dump` `nnue`, `dumpline` | |
| `nnue/layers/*` | `nnue/layers.rs` | gated (the scalar path; the SIMD paths of C++ give the same values) | |
| `nnue/network.{h,cpp}` | `nnue/network.rs` | `load()`, `evaluate()`, `verify()`: gated, but `verify()` differs (section 4); `save()` (`export_net`), `trace_evaluate()` (`eval`): not ported; `get_content_hash()`: shared memory only | `save()`: gated (commands, `export-*`, the bytes of the files); `trace_evaluate()`: gated (commands, `eval-*`); `verify()`: as C++, gated (commands) |
| `nnue/nnue_accumulator.{h,cpp}` | `nnue/accumulator.rs` | gated | |
| `nnue/nnue_architecture.h` | `nnue/architecture.rs` | gated | |
| `nnue/nnue_common.h` | `nnue/mod.rs` | gated (reading); the writing side (`write_little_endian`) not ported | writing: gated (`export-*`) |
| `nnue/nnue_feature_transformer.h` | `nnue/feature_transformer.rs` | gated; `write_parameters()` not ported | `write_parameters()`: gated (`export-*`) |
| `nnue/nnue_misc.{h,cpp}` | none | not ported: `trace()`, the table of `eval`; `EvalFile` (which network is loaded) is in `engine.rs` | `trace()` in `evaluate.rs`, gated (commands, `eval-*`) |
| `nnue/nnz_helper.h`, `nnue/simd.h` | none | not ported: SIMD helpers; the scalar path needs neither | |
| `numa.h` | none | not ported: `NumaConfig`, thread binding, replication | `numa.rs`: parsing and messages only, gated on this machine (commands, `numa-*`, native only: they depend on the processors) and by unit tests; binding and replication not ported (section 3) |
| `perft.h` | `perft.rs` | gated: `perft.sh`, `tests/perft.sh` positions | |
| `position.{h,cpp}` | `position.rs` | gated; `operator<<` (`d`) partly: only `Fen` and `Key` | `operator<<`: gated (commands, `d`, `d-cli`) |
| `score.{h,cpp}` | `score.rs` | gated (the trace, `compare`) | |
| `search.{h,cpp}` | `search.rs` | gated (the trace, `compare`, clock sessions); `Skill` ungated; the Syzygy parts in their form without tables | `Skill`: gated (surface_check, seeded searches, skill_stats) |
| `searchtrace.{h,cpp}` (the fork's) | `searchtrace.rs` | gated (`trace`, `trace-full`) | |
| `shm.h`, `shm_unix.h` | none | not ported: the network shared between processes (section 5) | |
| `syzygy/tbprobe.{h,cpp}` | none | not ported (next layer) | |
| `thread.{h,cpp}` | `thread.rs` | gated (threads_check, thread index, stress) for one NUMA node | the binding messages: gated (commands, `numa-*`) |
| `thread_native.h` | `std::thread` with an 8 MiB stack | gated (deep searches on every target) | |
| `timeman.{h,cpp}` | `timeman.rs` | gated (tm_check, clock sessions) | |
| `tt.{h,cpp}` | `tt.rs` | gated (tt_check, the trace) | |
| `tune.{h,cpp}` | none | not ported: the SPSA tuning framework; no `TUNE()` in this commit, so it declares no option and prints nothing | |
| `types.h` | `types.rs` | gated | |
| `uci.{h,cpp}` | `uci.rs` | every command (section 2) |
| `ucioption.{h,cpp}` | `ucioption.rs` | gated: parsing, bounds, printing (`uci`), case-insensitive names; differs: a name or value that is not UTF-8 is replaced lossily (C++ keeps the bytes) | |
| `universal/*` | none | not ported: the C++ fat binary (one executable for several x86/ARM/RISC-V levels) | |

## 2. Commands

`uci.cpp` reads a line, takes its first token and dispatches; the rest of the
line is read as `std::istringstream` reads it, which the port emulates
(`Tokens`, the numeric prefixes of `is >> n`).

| Command | C++ | master | layer 10 |
| --- | --- | --- | --- |
| `uci` | `id name`, `id author`, the options, `uciok` | the options gated (identical, section 3); `id name` differs (section 4) | |
| `isready` | `readyok` | gated; the port also loads the network then if none is loaded (C++ has it from the start) | |
| `ucinewgame` | `search_clear()` | gated (the trace, `compare`) | |
| `setoption name N [value V]` | section 3 | gated | |
| `position startpos [moves ...]`, `position fen F [moves ...]` | sets the position; an illegal move or FEN ends the program (`CRITICAL ERROR`) | gated (the trace, instrumented.py `TestInvalidFEN`, `TestEnPassantSanitization`) | |
| `position` + any other token | ignored | gated (direct comparison) | |
| `go depth/nodes/movetime/mate/infinite/ponder/searchmoves` | | gated (the trace, `compare`, clock sessions) | |
| `go wtime/btime/winc/binc/movestogo` | | gated (tm_check, clock sessions) | |
| `go perft N` | perft after `verify_network()` | differs: the port does not verify the network (runs without it; C++ ends the program if it has none) nor print the network lines (section 4); and C++ does not wait for a running search before perft, the port does | verifies the network as C++, gated (commands, `perft-no-network`, `go-messages`); still waits for a running search (C++'s output would mix with the search's) |
| `go` with an invalid number | `CRITICAL ERROR: ... Invalid argument for 'depth'` | gated (direct comparison) | |
| `go` with negative times | made 1 | gated (direct comparison) | |
| `stop`, `quit`, `ponderhit` | | gated (`compare`, clock sessions, stress) | |
| end of input | as `quit` | gated | |
| `flip` | flips the position | gated (instrumented.py `test_fen_position_2_flip`) | |
| `bench [hash threads limit file limittype]` | the bench, `file` `default`, `current` or a file of FENs, `limittype` `depth`, `nodes`, `movetime`, `perft` or `eval` | gated (3464885; instrumented.py `TestBenchFile`); `limittype eval` prints `info string eval is not ported` | `limittype eval`: gated (commands, `bench-eval`) |
| `speedtest [threads hash seconds]` | 3 warm-up searches, then about 300 timed searches of game positions, and a report on stderr | not ported (`Unknown command`) | gated by structure (commands, `speedtest*`: the progress lines, the labels of the report, the invocations, threads, binding, hash, clamping warnings); its commands by surface_check; its timings depend on the machine |
| `d` | the board, `Fen`, `Key`, `Checkers` (and the tablebase probes when tables are loaded) | differs: only `Fen` and `Key` | gated (commands, `d`, `d-cli`) |
| `eval` | the network's output for each layer stack and the evaluation | not ported | gated (commands, `eval-*`: 2,253 positions of the samples, 1,092 in check; `eval-moves`, `eval-cli`) |
| `dump`, `dumpline` (the fork's) | | gated (the differential gates, instrumented.py) | |
| `searchtrace` (the fork's) | in the tracing binary only (`SEARCH_TRACE`); `stockfish` says `Unknown command` | differs by design: in the port's one binary (README, "The search trace in the port") | |
| `compiler` | the compiler, target and settings | not ported | in the shape of C++ for the Rust compiler (section 4), gated by shape (commands, `compiler*`) |
| `export_net [file]` | writes the network | not ported | gated (commands, `export-*`); without an embedded network: section 5 |
| `help`, `--help`, `license`, `--license` | a fixed text | not ported | gated (commands, `help*`) |
| a line starting with `#`, an empty line | ignored | gated | |
| anything else | `Unknown command: '<line>'. Type help for more information.` | gated | |
| command line: no argument | reads commands from stdin | gated | |
| command line: arguments (`stockfish bench 16 1 13`, `stockfish go depth 5`, `stockfish uci isready`) | joins all arguments with spaces into one command, runs it once, waits for a search it started, exits (`uci isready` is `uci` with an argument) | gated (instrumented.py `TestCLI`) | |

## 3. Options

All 19 options are declared with the names, types, defaults and bounds of
C++ and printed by `uci` as C++ prints them (compared directly in this
layer: the `uci` output of both engines is identical). Values are checked as
C++ does (`Option::operator=`: a spin by `strtoll` within its bounds, a check
`true`/`false`, a string `<empty>` for empty; a refused value is silently
ignored, an unknown name prints `No such option: <name>`); names are
case-insensitive.

| Option | Type, default, bounds | master | layer 10 |
| --- | --- | --- | --- |
| `Debug Log File` | string, empty | no effect (C++ logs stdin and stdout to the file, `>> ` and `<< ` prefixes; a file it cannot open ends the program with `Unable to open debug log file "<path>"` on stderr) | gated (commands, `log`, `log-cli`, `log-bad*`: output and the files written) |
| `NumaPolicy` | string, `auto` | no effect: no message, no binding | messages: gated (commands, `numa-*`, on this machine); no binding (below) |
| `Threads` | spin, 1, 1 to max(1024, 4 × cores): **depends on the machine** (C++ `hardware_concurrency()`, the port `available_parallelism()`; they differ past 256 CPUs or with a restricted affinity on such a machine), not on the word size | gated (threads_check, stress, instrumented.py); the message differs without NUMA binding (section 4) | the CPUs online, as C++ (`/sys/devices/system/cpu/online`, else `available_parallelism()`); the binding suffix gated (commands, `numa-*`) |
| `Hash` | spin, 16, 1 to 33554432 on 64-bit targets, 2048 on 32-bit: **depends on the target** | gated (tt_check, `compare` at 1, 3, 5, 7, 64 MB) | |
| `Clear Hash` | button | gated (threads_check) | |
| `Ponder` | check, false | gated (clock sessions, tm_check) | |
| `MultiPV` | spin, 1, 1 to 256 | gated (`compare`) | |
| `Skill Level` | spin, 20, 0 to 20 | ungated | gated (surface_check, seeded searches, skill_stats) |
| `Move Overhead` | spin, 10, 0 to 5000 | gated (tm_check, clock sessions) | |
| `nodestime` | spin, 0, 0 to 10000 | gated (clock sessions) | |
| `UCI_Chess960` | check, false | gated (every gate, Chess960 halves) | |
| `UCI_LimitStrength` | check, false | ungated | gated (surface_check, seeded searches, skill_stats) |
| `UCI_Elo` | spin, 1320, 1320 to 3190 | ungated | gated (surface_check, seeded searches, skill_stats) |
| `UCI_ShowWDL` | check, false | gated (`compare`) | |
| `SyzygyPath` | string, empty | differs: prints `info string Tablebases are not supported by this port` where C++ loads the tables and prints `info string Found N WDL and M DTZ tablebase files (up to K-man).` | next layer |
| `SyzygyProbeDepth` | spin, 1, 1 to 100 | no effect (no tables) | next layer |
| `Syzygy50MoveRule` | check, true | no effect (no tables) | next layer |
| `SyzygyProbeLimit` | spin, 7, 0 to 7 | no effect (no tables) | next layer |
| `EvalFile` | string, `nn-18cd32e11a9c.nnue` | gated (the default network); differs: C++ takes the default name for its embedded network and looks for any other file in the working directory, then the directory of `argv[0]` (then `DEFAULT_NNUE_DIRECTORY` if the build defines it); the port looks for every name, the default one included, in the working directory then the directory of the executable (`current_exe()`, which differs from `argv[0]`'s when the binary is run through `PATH` or a symbolic link) | the directory of `argv[0]`, and `DEFAULT_NNUE_DIRECTORY` from cargo's build environment, as C++; a network that cannot be read leaves the loaded one in place, as C++; gated (commands, `evalfile-back`, `export-other`, `export-failed-load`) |

**`NumaPolicy` in C++, measured on this machine (one node, CPUs 0-1).** The
value is stored even when refused. `auto` and `system` compute the
configuration from the system (on Linux the CPUs the process may use,
grouped by NUMA node and L3 domain, bundles of up to 32 threads), `hardware`
ignores the affinity, `none` puts every CPU in one node, and anything else is
parsed as nodes separated by `:` of CPUs separated by `,` with `a-b` ranges
(`0:1` makes two nodes even on one): refused if it gives no node, repeats a
CPU, or holds a range of 2^20 CPUs or more, with `info string NumaPolicy:
invalid value '<v>', keeping previous config.` An accepted value prints
`Available processors: <config>` and `Using N threads[ with NUMA node thread
binding: <bound>/<cpus>:...]`. Threads are bound unless the stored value is
`none`, or `auto` and the configuration does not suggest it (one node and no
custom CPUs never does): so after `system`, `hardware`, a custom list, **or a
refused value** (stored as is), C++ binds and says so, even on one node. Each
`go` then prints `Available processors: ...` and the `Using ...` line, and
each network verification one `Network replica K: ...` line per node of a
configuration that needs replication (custom, or more than one node),
`No allocation.` for a node no thread uses yet.

## 4. What the engine prints besides the answer to a search

| Output | C++ | master | layer 10 |
| --- | --- | --- | --- |
| start-up line | `Stockfish dev-YYYYMMDD-<sha8> by the Stockfish developers (see AUTHORS file)` (the date and commit from git at build time, `-m` before the sha if the tree was modified, `nogit` without git) | differs: `Stockfish Rust port (layers 1-9 of the C++ engine at 43e01e07)` | `Stockfish dev-YYYYMMDD[-m]-<sha8> (Rust port) by the Stockfish developers (see AUTHORS file)`: date and commit from git by `build.rs` as the Makefile does, ` (Rust port)` so that a GUI tells the engines apart |
| `id name`, `id author` | `id name Stockfish dev-...`, `id author the Stockfish developers (see AUTHORS file)` | `id name` differs (`Stockfish Rust port`), `id author` same | `id name Stockfish dev-... (Rust port)`, `id author` unchanged |
| the options of `uci`, `uciok`, `readyok` | | gated | |
| `info string Available processors: <config>` | at each `go`, after an accepted `NumaPolicy`; and in `speedtest` | not printed | gated (commands; normalized on the emulated targets) |
| `info string Using N thread(s)` | at `go`, `setoption Threads`, `NumaPolicy`; ` with NUMA node thread binding: ...` when bound | gated without binding (stress test, both engines) | the binding suffix: gated (commands, `numa-*`) |
| `info string NNUE evaluation using <file> (<n>MiB, (<dims>))` | at every network verification: each `go` and `go perft` (`Engine::perft` verifies too), `eval`, `dump`, `dumpline`, each search of `bench` | differs: printed at the first verification after a load only | at every verification, gated (commands); `searchtrace` verifies once, as C++ |
| `info string Network replica K: Shared memory.` (or `Local memory.` with the reason, `No allocation.`) | after each such verification, one per replica | not printed | one line per replica, the port's saying `Local memory. Shared memory not supported by the OS. Local allocation fallback.` (what C++ prints without shared memory) where C++ on Linux says `Shared memory.`; gated with that status normalized (commands) |
| `info string ERROR: ...` (5 lines) when the network cannot be loaded, then exit 1 | at verification | gated for `go`, `go perft`, `eval` (commands, `*-no-network`) |
| `No such option: <name>` | | gated | |
| `Unknown command: '<line>'. Type help for more information.` | | gated | |
| `info string CRITICAL ERROR: Command `<line>` failed. Reason: <reason>` then a blank line, exit 1 | FEN errors (every message of `Position::set`), illegal moves, invalid `go` numbers, missing files of `dump`/`dumpline`/`searchtrace`, `flip` errors | gated (instrumented.py, unit tests, direct comparisons) | |
| `info string NumaPolicy: invalid value '<v>', keeping previous config.` | | not printed | gated (commands, `numa-*`; instrumented.py `test_numa_*`) |
| `info string Found N WDL and M DTZ ...` | `SyzygyPath`, `ucinewgame` with tables | differs (above) | next layer |
| `info string Syzygy based PV extension requires more time, ...` | | not reachable without tables | next layer |
| perft lines `<move>: <count>` and `Nodes searched: N` | | gated (`perft.sh`) | |
| `bench` on stderr: `Position: i/n (<fen>)`, the totals | | gated (format, instrumented.py); `Total time` counted from before the hash and thread set-up (the missing `ucinewgame`, section 1) | fixed |
| `Unable to open file <f>` (bench) on stderr, exit 1 | | gated (instrumented.py `TestBenchFile`) | |
| `speedtest` on stderr | warm-up lines, the report (version, compiler, large pages, invocation, processors, threads, binding, hash, hashfull, nodes, time, speed), clamping warnings `info string speedtest: ...` | not ported | gated by structure (commands, `speedtest*`) |
| `export_net` | `Network saved successfully to <file>` / `Failed to export a net` / `Failed to export a net. No network file is currently loaded. Please load a network file first.` / `Failed to export a net. A non-embedded net can only be saved if the filename is specified` | not ported | gated (commands, `export-*`) |
| `compiler` | `Compiled by : g++ (GNUC) 14.2.0 on Linux`, architecture, settings, `__VERSION__` | not ported | `Compiled by : rustc 1.85.1 on Linux`, the target triple, `64bit`/`32bit` and the extensions of C++'s list the target enables (` SSE2`, ` POPCNT`, ` NEON`, ..., ` DEBUG`), the full `rustc -V`, under C++'s labels; gated by shape |
| `help`/`license` | the fixed text | not ported | gated |
| `d`, `eval` | | section 2 | gated |
| `Unable to open debug log file "<path>"` on stderr, exit 1 | | not printed | gated (commands, `log-bad*`) |
| `Failed to allocate N MB for transposition table.` on stderr, exit 1 | | gated by reading (`tt.rs`) | |
| `Failed to create search thread` | C++ on a failed thread creation, exit 1 | gated by reading (`thread.rs` prints it and exits; this row said "panics" when first pushed, wrongly) | |
| FENs printed (`d`, `bench`, `dump`) | | gated (`dump` `fen`) | |

## 5. Platform behaviour

| Behaviour | C++ | master | layer 10 |
| --- | --- | --- | --- |
| The network embedded in the binary | `incbin` puts the default network (138 MB) in the executable; `EvalFile` set to the default name always means it | not ported: the port reads a file (README, "The network"); without the file, `go`/`dump` end the program with the ERROR lines, as a C++ build with `NNUE_EMBEDDING_OFF` (the wasm builds) does | out of scope; the cost below. `export_net` behaves as in a C++ build without embedding: without a file name it writes the default network to the default name in the working directory (the C++ engine with embedding writes the same bytes, gated); when no network could be loaded at all it prints C++'s `No network file is currently loaded`, where C++ with embedding still has its embedded one |
| The network shared between processes | Linux, macOS, BSD: POSIX shared memory (`shm_open`, keyed by the network's content hash and the executable), so several engines on one machine hold one copy; Windows: a named file mapping; elsewhere (Android): local memory, and `Network replica 1: Local memory. Shared memory not supported by the OS. Local allocation fallback.` | not ported: each process holds its own copy (129 MiB); needs `shm_open`/`mmap` (unsafe, an OS interface the port does not use) | not small, not done; the replica lines say `Local memory. ...` |
| Large pages | the hash and the network: `madvise(MADV_HUGEPAGE)` on Linux, `VirtualAlloc(MEM_LARGE_PAGES)` on Windows with the privilege, 1 GB pages for large hashes; `speedtest` reports `Large pages: yes/no` | not ported (speed only; transparent huge pages may still apply when the system sets them to `always`); needs `madvise` (unsafe FFI) | not small, not done; `speedtest` says `Large pages : no` |
| NUMA: thread binding, the network and shared histories per node | Linux (sysfs, `sched_setaffinity`) and 64-bit Windows (processor groups) only | not ported; behaves as C++ with one node and no binding | messages only (section 3); binding out of scope by decision |
| Prefetching | `__builtin_prefetch` of TT clusters and history entries | not ported: changes no result; the intrinsic is `unsafe` on this toolchain | speed work, later |
| The directory of the binary (network search) | the parent of `argv[0]` (Windows: `GetModuleFileNameW`), `.` if none | differs: `current_exe()` | as C++ (gated by reading) |
| `DEFAULT_NNUE_DIRECTORY` | a third directory for the network, fixed at build time (`-DDEFAULT_NNUE_DIRECTORY=...`) | not ported | read from cargo's build environment (gated by reading) |
| Windows: UTF-8 console, wide `argv`, paths in UTF-8 then the ANSI code page | `set_console_utf8()`, `CommandLine`, `path_from_utf8()` | std's conversions (arguments are UTF-16 → UTF-8 lossily, paths from UTF-8); no Windows CI | |
| `Debug Log File` | stdin and stdout teed to a file | no effect | gated (commands, `log*`) |
| 32-bit targets | `Hash` up to 2048 | gated (emulated targets) | |
| The thread stack | 8 MiB (`TH_STACK_SIZE`) | gated (deep searches, musl) | |

**What embedding the network would cost** (for a decision, not done). The
file is 138,360,528 bytes (the C++ reference binary is 139,214,960 bytes,
the port's 1,241,696). In Rust,
`include_bytes!` of the file puts it in the executable (from 1.2 MB to about
139 MB, compile time and the compiler's memory grow with it), and loading
from it instead of a file changes nothing else. Memory: as now, 129 MiB of
parsed weights, plus the embedded bytes touched once while parsing (clean,
file-backed pages the kernel can drop). A zero-copy variant would use the
embedded bytes themselves as the weights of the feature transformer (the
port keeps them in the file's order already, `i8` read as `byte as i8`), so
that the 129 MiB would be file-backed and shared by every process running
the same binary, which is what C++'s shared memory gives: a larger change to
`nnue/`, without `unsafe`.

## 6. Counts

Filled at the end of the layer.
