# Differential tests

The other tests of this directory (perft, bench signature, `instrumented.py`)
tell whether an engine behaves like Stockfish, not where it differs. This
harness compares engines one component at a time. It consists of

- the `dump` debug command of the engine, which prints the internals of a
  position (hash keys, check and pin bitboards, static evaluation, legal moves,
  static exchange evaluation, move ordering) as one line of JSON,
- the `dumpline` debug command, which plays a line of moves on one position the
  way search does and prints, for each position reached, the state as
  maintained incrementally (keys, rule50 and repetition bookkeeping, the
  evaluation from the incrementally updated NNUE accumulators, a null move and
  a probe move made and undone on the side), then walks back down the line,
- the `searchtrace` debug command of a separate tracing binary, which runs
  deterministic searches and records what they print, a digest of every node
  they visit and every decision they make, and digests of the tables they leave
  behind (see "The search trace"),
- a corpus of 392,703 distinct positions and one of 4,432 lines of play
  (342,044 moves), taken from real games, and the golden files holding what
  this engine prints for each of them, and one of 17,270 searches drawn from
  both,
- scripts to rebuild the corpora and to verify a binary against the golden files.

The golden files describe one exact engine: the commit they were generated at
(`43e01e07`, bench 3464885) with its default network `nn-18cd32e11a9c.nnue`.
They are not meant to follow master: any functional change to the evaluation,
the network, SEE, move ordering or repetition detection changes them, and any
functional change to search changes those of the search trace.

## Quick start

    cd src && make -j2 build                    # the reference engine
    ../tests/differential/verify.sh             # 5,000 positions, kept in git
    ../tests/differential/verify.sh lines       # 150 lines of play, kept in git
    ../tests/differential/make_corpus.sh        # download and rebuild everything
    ../tests/differential/verify.sh full        # 392,703 positions
    ../tests/differential/verify.sh lines-full  # 4,432 lines of play
    # searches: see "The search trace" for verify.sh trace, trace-uci, trace-full, trace-uci-full

    # another binary, and only what it implements so far
    EXE=/path/to/engine ../tests/differential/verify.sh full --fields fen,key,moves

The full modes need the corpora and golden files that `make_corpus.sh` and
`make_trace_corpus.sh` build (260 MB, not in git). On a machine that already
has them in its shared folder, nothing needs to be built: see "The shared
folder" below.

`verify.sh` prints, for the first positions that differ, which field differs and
where, then the number of differing positions per field, and exits with 1:

    position 2244: 2kr3r/ppp3pp/1qnb1n2/3p4/3PN3/1P2P2P/P1PBB2P/R2QKR2 w Q - 1 14
      order_main_deep[2]: expected f1f6, got e2g4
      order_main_shallow[2]: expected f1f6, got e2g4
    FAILED sample/standard.fen.gz: 3 of 4000 positions differ (all fields)
      order_main_deep             3
      order_main_shallow          3

To be verified, a binary has to read UCI commands on its standard input,
understand `setoption name UCI_Chess960 value true`, `dump <file>` and `quit`,
and print the dump lines on its standard output, exactly one per position. Lines
that do not start with `{` are ignored. A harness in another language can also just read the golden
files and compare in process.

## The shared folder

The full corpora, their golden files and the reference binaries live once per
machine, outside every checkout, in the folder named by `STOCKFISH_SHARED`
(default `~/.local/share/stockfish-fork`):

| Path | Holds |
| --- | --- |
| `data/` | What the full modes read: `{standard,chess960}.{fen,jsonl,lines,lines.jsonl,trace,trace.jsonl}.zst`, the `CONTENT.sha256` and `CONTENT.trace.sha256` the scripts wrote (the same as in git), `FILES.sha256` (the compressed files), and `downloads/`, the Lichess dumps. |
| `oracle/` | `stockfish` and `stockfish-trace` built from a recorded commit, the network `nn-18cd32e11a9c.nnue`, and `MANIFEST` (commit, build commands, compiler, bench signatures, checksums). |
| `venv/` | A Python environment made from `requirements.txt` (python-chess 1.11.2), for `make_corpus.sh` only, given to it as `PYTHON=$S/venv/bin/python`. |

The rule each script follows (`shared.sh` defines it):

- `verify.sh` in a full mode reads the golden files from `DATA` if set, else
  from `tests/differential/data` if it holds those of the mode, else from
  `$STOCKFISH_SHARED/data`. It refuses a folder whose `CONTENT*.sha256`
  differs from the one in git, and a corpus that gives no position, line or
  search to compare.
- The binary checked is `EXE`, else `src/stockfish` (`src/stockfish-trace` for
  `trace` and `trace-full`) of the checkout. It is never taken from the shared
  folder unless `EXE` names it, so that a check of another engine cannot end up
  comparing the reference with its own output. `verify.sh` prints the binary
  and the folder it uses on stderr before anything else, and says when the
  binary is one of the shared folder's.
- `make_corpus.sh` and `make_trace_corpus.sh` write into `DATA` (default
  `tests/differential/data`) only, and refuse a `DATA` inside the shared
  folder. They read their inputs from it when the checkout lacks them: the
  Lichess dumps for `make_corpus.sh`, which then downloads nothing, the
  corpora of positions and lines for `make_trace_corpus.sh`.
- `make_corpus.sh` runs `PYTHON` (default `python3`) and stops, before it
  downloads or writes anything, if that cannot import python-chess. It never looks for the
  shared environment by itself: name it with `PYTHON=`. The other scripts,
  `verify.sh` included, need the standard library only.
- The shared folder is read-only on disk, folders included.

A new worktree needs only this:

    S=${STOCKFISH_SHARED:-$HOME/.local/share/stockfish-fork}
    EXE=$S/oracle/stockfish       tests/differential/verify.sh full
    EXE=$S/oracle/stockfish       tests/differential/verify.sh lines-full
    EXE=$S/oracle/stockfish-trace tests/differential/verify.sh trace-full
    EXE=$S/oracle/stockfish       tests/differential/verify.sh trace-uci-full

    # to build the C++ engine of the worktree, with the network of the shared
    # folder rather than a download: make net validates the file the link
    # points to (sha256) and leaves it alone
    ln -s "$S/oracle/nn-18cd32e11a9c.nnue" src/
    cd src && make -j2 build && make -j2 trace-build

To rebuild the corpora, which is only done on purpose (a rebuild that
differs from what git records fails, see below), the shared folder holds all
the inputs, the Python environment included, so that nothing is downloaded or
installed. `DATA` must be a folder outside the shared one, and outside the
checkout to keep the worktree small:

    S=${STOCKFISH_SHARED:-$HOME/.local/share/stockfish-fork}
    export PYTHON=$S/venv/bin/python DATA=/a/folder/outside/the/shared/one
    EXE=$S/oracle/stockfish       tests/differential/make_corpus.sh         # 5 min
    EXE=$S/oracle/stockfish-trace tests/differential/make_trace_corpus.sh   # 7 min

This was checked on 2026-11-13 from a fresh checkout of `d540d33b`: nothing
was downloaded (the dumps were read from `$S/data/downloads`), both scripts
ended with "The corpus and the golden files are the recorded ones", the
samples in git were left unchanged, and the 12 `.zst` files built were
byte-identical to those of `$S/data`. Install nothing into the system Python
or a checkout for this. A venv holds absolute paths, so on another machine
make a new one outside the checkout (`python3 -m venv <dir>` and
`<dir>/bin/pip install -r tests/differential/requirements.txt`) and set
`PYTHON` to its interpreter.

CI does not use the shared folder: it checks the samples kept in git with
binaries it builds, and never rebuilds the corpora.

## The dump command

    dump            dump the current position (as set by 'position')
    dump <file>     dump each position of a file of FENs, one per line

Blank lines of the file are skipped; every other line gives exactly one line of
output, in the same order. The positions are streamed, the file can be of any
size (about 25,000 positions per second on an old 3.4 GHz core). Chess960
positions need the `UCI_Chess960` option, as everywhere else in the engine. The
command works on a copy of the position with its own tables, and does not touch
the state of the search.

A position that cannot be set up gives `{"error":"<reason>","input":"<line>"}`.
Any other gives an object with the following members, always in this order.
For brevity the example leaves out most entries of the lists.

    {"fen":"4k3/2p5/3p4/8/8/8/3R4/4K3 w - - 0 1","chess960":false,
     "key":"27ED41D8289EB85D","pawn_key":"18407E41C417AF4A",
     "material_key":"5546BEE4509018E1","minor_piece_key":"0000000000000000",
     "non_pawn_key":["55D5C19F5CFD1B22","AEF2C95BD9AD5886"],
     "checkers":"0000000000000000",
     "blockers_for_king":["0000000000000000","0000000000000000"],
     "pinners":["0000000000000000","0000000000000000"],
     "check_squares":["0028000000000000", ...],"threats":["00000A1400000000", ...],
     "non_pawn_material":[1276,0],"nnue":1627,"eval":1682,
     "moves":["d2d6","d2d1","d2a2", ...],"see":[-1068,0,0, ...],
     "gives_check":[0,0,0, ...],"key_after":["B4B08F10CE893E8E", ...],
     "order_main_shallow":["d2e2","d2d1", ...,"d2d6"],
     "order_main_deep":["d2e2","d2d1", ...,"d2d6"],
     "order_qsearch":["d2d6"]}

Keys and bitboards are strings of 16 uppercase hexadecimal digits, since many
JSON parsers cannot hold a 64-bit integer. In a bitboard, bit 0 is a1, bit 7 is
h1 and bit 63 is h8. Lists of two are indexed by color, white first. Lists of
six are indexed by piece type, from pawn to king.

| Member | Meaning |
| --- | --- |
| `fen` | `Position::fen()` of the position once set up, hence a FEN round trip. In Chess960 the castling rights are the files of the rooks (Shredder-FEN). The en passant square is only there if the capture is legal. |
| `chess960` | Value of the `UCI_Chess960` option. |
| `key` | `Position::key()`, the Zobrist key, including its adjustment for a rule50 counter of 14 or more. |
| `pawn_key`, `material_key`, `minor_piece_key`, `non_pawn_key` | The other keys kept by `Position`; `non_pawn_key` is per color. |
| `checkers` | Pieces giving check to the side to move. |
| `blockers_for_king` | Per color, the pieces of either color that alone stand between the king of that color and an enemy slider. |
| `pinners` | Per color, the sliders of that color pinning an enemy piece to the enemy king. |
| `check_squares` | Per piece type, the squares from which such a piece of the side to move would give check. |
| `threats` | Per piece type, the squares attacked by the opponent's pieces of that type, sliders seeing through the king of the side to move. |
| `non_pawn_material` | Per color, in internal units. |
| `nnue` | Output of the network (`Network::evaluate`) from the point of view of the side to move, in internal units. `null` when in check, as search never evaluates then. |
| `eval` | `Eval::evaluate()` with an optimism of 0: `nnue` after the scaling by material and rule50. `null` when in check. |
| `moves` | Legal moves in the order `MoveList<ALL>` generates them, in UCI notation. Castling is `e1g1` in standard chess and king takes rook in Chess960. |
| `see` | For each move, its static exchange evaluation: the largest `t` for which `see_ge(move, t)` holds, so that `see_ge(move, t) == (t <= see)`. Found by bisection; `see_ge()` was checked to be monotonic for all 8,193 thresholds on every move of the corpus. Castling, en passant and promotions are 0 by definition. |
| `gives_check` | For each move, 1 if it gives check. |
| `key_after` | For each move, `key` after `do_move()`. Pins down the incremental update of that one key, one move deep, including the en passant and castling rules. |
| `order_main_shallow`, `order_main_deep` | The moves in the order the `MovePicker` of the main search returns them, at depth 1 and 16. |
| `order_qsearch` | Same for the `MovePicker` of the quiescence search (depth 0): captures and queen promotions, or all evasions when in check. |

The move pickers are given no TT move, a ply of 0, and history tables in which
every entry is 0. This is not the state of a fresh search, whose tables start
from non-zero constants (see `Search::Worker::clear`): the point is to test the
picker alone. With such tables the order depends on the stages, on the capture
values and SEE thresholds, on the bonus for checks and the penalties for moves
to attacked squares, and on the exact behavior of `partial_insertion_sort`,
which leaves the quiet moves scoring below `-3560 * depth` unsorted: most of
them at depth 1, none at depth 16.

## The dumpline command

    dumpline startpos moves e2e4 ...       dump a line of play
    dumpline fen <fen> moves e2e4 ...      dump a line of play
    dumpline <file>                        dump each line of a file, one per line

A line is given as the arguments of the UCI `position` command. In a file,
blank lines are skipped. The position is set up once, then the moves are made
on it with `do_move()`, each one pushing the NNUE accumulator stack as
`Search::Worker::do_move` does; nothing is ever set up again from a FEN. The
command has its own accumulator stack and refresh cache and works on its own
position, so it does not touch the state of the search.

For a line of `n` moves it prints `n + 2` records: one per position reached, from
the start position (`ply` 0) to the last one, then the walk back. A line that
cannot be set up or played (unknown first word, missing `moves`, invalid FEN,
illegal move) gives instead the single record
`{"error":"<reason>","input":"<line>"}`, and nothing is dumped for it. The moves
are all checked first, on a separate position.

For `dumpline startpos moves e2e4 d7d5 e4d5 g8f6`, the record of ply 2 and the
walk back, with some lists left out:

    {"ply":2,"move":"d7d5","fen":"rnbqkbnr/ppp1pppp/8/3p4/4P3/8/PPPP1PPP/RNBQKBNR w KQkq - 0 2",
     "key":"96270AD18105C788","pawn_key":"88C3B25994A695DA","material_key":"A0710EF633754DD6",
     "minor_piece_key":"41AF5A253E95F502","non_pawn_key":["85DC4DDCD53BCFCC","54B51DF93C8CF372"],
     "non_pawn_material":[8302,8302],"checkers":"0000000000000000",
     "blockers_for_king":[...],"pinners":[...],"check_squares":[...],"threats":[...],
     "captured":null,"plies_from_null":2,"repetition":0,"is_draw":false,
     "has_repeated":false,"upcoming_repetition":false,"nnue":25,"eval":31,
     "null":{"key":"36D22A7548FF9C44","fen":"rnbqkbnr/ppp1pppp/8/3p4/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1",
             "plies_from_null":0,"repetition":0,"captured":null,"nnue":103,
             "reply":{"move":"d5e4","key":"46231B0486CB37BD","pawn_key":"58C7A38C936865EF",
                      "material_key":"5A812EC5E7DDA968","minor_piece_key":"41AF5A253E95F502",
                      "non_pawn_key":["85DC4DDCD53BCFCC","54B51DF93C8CF372"],
                      "checkers":"0000000000000000","plies_from_null":1,"repetition":0,"nnue":-254}},
     "probe":{"move":"a2a3","key":"45094ECD2746B368","pawn_key":"FB18D6E1FB1FBAF6",
              "material_key":"A0710EF633754DD6","minor_piece_key":"41AF5A253E95F502",
              "non_pawn_key":["85DC4DDCD53BCFCC","54B51DF93C8CF372"],
              "checkers":"0000000000000000","plies_from_null":3,"repetition":0,"nnue":56}}
    ...
    {"undo":4,"keys":["06B960AABFBB9995","96270AD18105C788","B46022469E3DD31B","8F8F01D4562F59FB"],
     "fens":[...],"nnues":[-136,25,-24,34],
     "fen":"rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1","key":"8F8F01D4562F59FB", ...}

Members of a position record, in this order. Members with the name of a member
of `dump` have the same meaning and format, but the value is the one maintained
by the moves rather than computed on a fresh position.

| Member | Meaning |
| --- | --- |
| `ply` | Number of moves made since the start of the line. |
| `move` | The move that led here, `null` at ply 0. |
| `fen` | `Position::fen()`: placement, side to move, castling rights, en passant square, rule50 counter and move number, all updated by `do_move()`. |
| `key`, `pawn_key`, `material_key`, `minor_piece_key`, `non_pawn_key` | The keys, updated incrementally. |
| `non_pawn_material` | Per color, updated incrementally. |
| `checkers` | As set by `do_move()` from its `givesCheck` argument. |
| `blockers_for_king`, `pinners`, `check_squares`, `threats` | The bitboards cached in `StateInfo`, as set by `do_move()` (the threats are computed on first use). Same format as in `dump`. |
| `captured` | `StateInfo::capturedPiece`, as a FEN letter, or `null`. |
| `plies_from_null` | `StateInfo::pliesFromNull`; it counts from the start of the line, not of the game. |
| `repetition` | `StateInfo::repetition`: the distance in plies to the previous occurrence of the position within the reach of rule50 and the last null move, negative if that one was itself a repetition, else 0. |
| `is_draw`, `upcoming_repetition` | `Position::is_draw(ply)` and `upcoming_repetition(ply)`, with `ply` the number of moves since the root of the current search window (see below), as search passes them. |
| `has_repeated` | `Position::has_repeated()`. |
| `nnue` | `Network::evaluate()` on the accumulator stack, as search calls it. `null` when in check, and at the plies that are skipped (see below). |
| `eval` | `Eval::evaluate()` with an optimism of 0, `null` when `nnue` is. |
| `null` | A null move made and undone, as in null move pruning, at the positions that are evaluated: the `key`, `fen`, `plies_from_null`, `repetition` and `captured` after `do_null_move()`, the `nnue` then, from the same accumulators, and a `reply`: a move made after the null move, as the null move search does, given like `probe`. `null` when in check or skipped. |
| `probe` | A move other than the next one of the line, made with an accumulator push and undone with a pop, as search does with the children of a node: its `move`, the keys, `checkers`, `plies_from_null` and `repetition` after it, and the `nnue` then (`null` if it gives check). `null` when skipped or without another legal move. |

The walk back record has `undo`, the number of moves undone; `keys` and `fens`,
the `key` and `fen` after each `undo_move()`, from the next to last position down
to the start position; `nnues`, the evaluation at each of these positions, popping the
accumulator stack on the way, `null` in check and below the root of the last
search window; then the `fen` and the five keys of the start position reached.

What drives the NNUE accumulators, so that a port can reproduce the same
sequence of updates (the values must come out the same whatever the sequence;
the point is to exercise every way of reaching them):

- The accumulator stack is reset at ply 0 and every 64 plies, as at the root of
  each search of a game; the position there is then evaluated through a refresh
  from the cache. The refresh cache is cleared at the start of each line only.
- The positions at plies 3, 6 and 7 of every 8 are skipped: neither evaluated
  nor probed. The next evaluation then chains the updates of several moves,
  forward from the last computed accumulator or, when a king move or a change of
  heavy material in between requires a refresh, backward from the refreshed one
  (`AccumulatorStack::evaluate`). Positions in check are not evaluated either.
- At each position that is evaluated, the null move comes after the evaluation
  (and its reply after it, pushing the accumulator stack on top of the position's
  accumulator), then the probe move, then the move of the line.
- The probe is chosen among the legal moves sorted by their UCI notation, so that
  the order of move generation does not matter. They are grouped in classes:
  special (castling, en passant, promotions), other captures, other king moves,
  and all. At ply `p` the first non-empty class in the preferences number
  `p % 4`, which are (special, captures, all), (king moves, captures, all),
  (captures, special, all) and (all), gives the probe: its move number
  `(p / 4) % size` of the class. The next move of the line is left out of the
  classes; the reply to a null move is chosen the same way, from the null move
  position, with nothing left out.
- On the walk back the positions of the last window are evaluated again,
  reusing the accumulators computed on the way up, including those computed
  backward or as intermediate steps of a chain.

A wrong `undo_move()` or `undo_null_move()` shows up in the records that
follow, since the line goes on from the position they restored.

## The search trace

`dump` and `dumpline` stop where search starts. Between "every member of every
position and line matches" and "the bench signature matches" there would
otherwise be nothing: a port of the search that gives bench 3464012 instead of
3464885 learns nothing from the node count about where it went wrong. The
search trace records what a deterministic single-threaded search does, at three
levels:

- **UCI output** (coarse, cheap): for each search, the `info` lines of each
  iteration (depth, selective depth, score, bound, nodes, hashfull, principal
  variation; time and speed left out), the best move and the ponder move. Any UCI
  engine gives these, so this level can be checked with the default binary, or
  with a port that has no tracing yet (`verify.sh trace-uci`).
- **Events** (fine): every node visited, in order, with what was read (the
  transposition table entry, the static evaluation raw and corrected), every
  pruning, reduction or extension decision that fired, each move the move picker
  gave with its stage and score, each child search with its depth and window,
  each value returned, each write to the transposition table with the entry as
  it is afterwards, and each history update with the entries as they are
  afterwards. The golden files hold only a digest of this stream, checkpointed
  every 4096 events; the stream itself is printed on demand, for one search, to
  find the first event that differs (see "Bisecting").
- **State** (after each search): digests of everything a search leaves for the
  next one: each history table, the correction histories, the transposition
  table. These tell "the search diverged" from "the search matched, but the
  tables it left behind differ", which would otherwise only show in the next
  search.

The searches come in sessions. A session starts from a cleared state, as after
`ucinewgame`, and its searches inherit the transposition table and the
histories from each other, as in a game or in `bench`: the corpus has single
searches on positions, and sequences of searches along games.

### Quick start

    cd src && make -j2 build && make -j2 trace-build   # stockfish and stockfish-trace
    ../tests/differential/verify.sh trace              # 365 sessions, 983 searches, kept in git
    ../tests/differential/verify.sh trace-uci          # the same, UCI output only, with src/stockfish
    ../tests/differential/make_trace_corpus.sh         # rebuild the corpus (needs make_corpus.sh's)
    ../tests/differential/verify.sh trace-full         # 11,032 sessions, 17,270 searches
    ../tests/differential/verify.sh trace-uci-full

    # the first event that differs, in session 12 of the sample
    EXE=/path/to/port ../tests/differential/search_trace.py bisect \
        ../tests/differential/sample/standard.trace.gz ../tests/differential/sample/standard.trace.jsonl.gz 12

`EXE` selects the binary checked (default `src/stockfish-trace`, or
`src/stockfish` for the `trace-uci` modes) and `REF` the reference tracing
binary `bisect` compares it with (default `src/stockfish-trace`).

### The tracing binary

`make trace-build` builds `src/stockfish-trace` from the same sources with
`SEARCH_TRACE` defined, in its own directory of objects (`src/trace_build`, made
of links to the sources), so that its objects never mix with those of the
default build. In any other build the trace hooks (`TRACE(...)` in `search.cpp`,
see `searchtrace.h`) expand to empty statements and `search()` and `qsearch()`
are the functions that do the work: the default binary is the same, byte for
byte, as without the trace (checked with `cmp` on the binaries built before and
after). In the tracing binary `search()` and `qsearch()` are thin wrappers
recording the entry and the result of each node around `search_node()` and
`qsearch_node()`, and each hook tests a pointer that is null unless
`searchtrace` is running: its bench signature is 3464885 too, at the same speed.

### The searchtrace command

    searchtrace <file>                          run each session of the file
    searchtrace <file> events <n> <from> <to>   also print the events [from, to) of search n of each session
    searchtrace <file> events all <from> <to>   the same for every search
    searchtrace <file> counts                   also print the number of events of each kind, at the end

A session is one line of the file: `position` and `go` commands separated by
`;`, the first one a `position`. `position` takes the arguments of the UCI
command; `go` only `depth N` and/or `nodes N`, never a time. Blank lines and
lines starting with `#` are skipped. For instance

    position fen 8/8/5pp1/pp5p/6k1/8/3K4/8 w - - 2 64 ; go depth 7
    position startpos moves e2e4 ; go nodes 5000 ; position startpos moves e2e4 e7e5 g1f3 ; go nodes 5000

The command first sets the options a search depends on to fixed values:
`Threads` 1, `Hash` 2 (MB), `MultiPV` 1, `Skill Level` 20, `UCI_LimitStrength`
false, `UCI_ShowWDL` false, `SyzygyPath` empty (no tablebases), `nodestime` 0,
`Ponder` false. `UCI_Chess960` is left as set. Each session then starts with
what `ucinewgame` does (`Engine::search_clear`: the transposition table, the
histories, the NNUE refresh caches, the node counter of the time check), and
each `go` runs a real search through `Engine::go`. With one thread and no time
limit the search is deterministic: the node limit is checked by node count only.

The command works on the engine itself: it leaves the options above set, and
the position, the transposition table and the histories as after the last
search. It prints one record per search, as one line of JSON. A session that
cannot be parsed gives the single record
`{"session":<n>,"error":"<reason>","input":"<line>"}`; a position that cannot be
set up gives such a record with the number of the search that would have
followed, and ends the session.

A record (this one has fewer than 4096 events, hence no checkpoint):

    {"session":1,"search":1,"fen":"rnbqkbnr/pppp1ppp/8/4p3/4P3/8/PPPP1PPP/RNBQKBNR w KQkq - 0 2",
     "go":"depth 3",
     "info":["depth 1 seldepth 4 multipv 1 score cp 25 nodes 33 hashfull 0 tbhits 0 pv g1f3",
             "depth 2 seldepth 3 multipv 1 score cp 47 nodes 69 hashfull 0 tbhits 0 pv g1f3",
             "depth 3 seldepth 2 multipv 1 score cp 47 nodes 99 hashfull 0 tbhits 0 pv g1f3"],
     "bestmove":"g1f3","ponder":null,"nodes":99,
     "iterations":[{"depth":1,"completed":true,"score":70,"uci_score":70,"bound":"exact","seldepth":4,"nodes":33,"pv":"g1f3"}, ...],
     "state":{"main_history":"D002D812F15CF939","low_ply_history":"F72F887E99F853BD",
              "capture_history":"DB00ED5CDCE4E325","continuation_history":"6655B0CDC5297451",
              "pawn_history":"379869F7FDD11A52","correction_history":"DE994B57683E8886",
              "continuation_correction_history":"18FA6D6EA5462325","ttmove_history":0,
              "tt_generation":1,"tt":"A0CB6630C7120847"},
     "events":1437,"digest":"19BF911C77FC5280","checkpoints":[]}

| Member | Meaning |
| --- | --- |
| `session`, `search` | Number of the session in the file (blank lines and comments not counted) and of the search in the session, from 1. |
| `fen` | `Position::fen()` of the root. |
| `go` | The limits, as `depth N`, `nodes N` or `depth N nodes M`. |
| `info` | The `info` lines the search sent, in order, as the UCI command prints them, without `info ` and without the `nps` and `time` members. A position without legal move gives `depth 0 score mate 0` or `depth 0 score cp 0`. |
| `bestmove`, `ponder` | As in the `bestmove` line; `ponder` is `null` when there is none. `(none)` without legal move. |
| `nodes` | `Threads::nodes_searched()` at the end. |
| `iterations` | Each iteration of iterative deepening as it ends, including one stopped by the node limit (`completed` false): `rootDepth`, then for the first root move `score` and `uciScore` (internal units), `bound` from its inexact flags, `seldepth`, the node count then and the `pv`. |
| `state` | Digests of the tables after the search (see below). `low_ply_history` is `null` when no search ran (no legal move): it is filled at the start of each search and not cleared by `ucinewgame`, so it then holds what an earlier search left. |
| `events` | Number of events of the search. |
| `digest`, `checkpoints` | The digest of the whole event stream, and the digest of the stream up to event 4096, 8192, ... |

### Events

Each event is one line of text: a name, then `key=value` fields separated by
single spaces. Values are integers in internal units (booleans 0 or 1,
`VALUE_NONE` 32002), moves in UCI notation (`-` for none, `0000` for the null
move), keys as 16 uppercase hexadecimal digits, squares like `e4`, pieces as FEN
letters. The digest is 64-bit FNV-1a (offset basis `CBF29CE484222325`, prime
`100000001B3`) over the bytes of the events, each followed by `\n`, from the
first event of the search; `checkpoints[k]` is its value after event
`4096 * (k + 1)`. With `events`, each event is printed as `<index> <event>`.

The events are emitted where `search.cpp` makes the decision, in its order, so
that a port with the same structure can emit them at the same points. Where an
event is emitted only when something fires, the absence of the event is the
other outcome.

| Event | Fields | Where |
| --- | --- | --- |
| `start` | `generation`, `root_moves` | `iterative_deepening()`, after the low ply history is filled and the main history decayed. Not emitted without legal move: such a search has no event. |
| `iteration` | `depth` | Each iteration, after `rootDepth++`. |
| `aspiration` | `depth` (adjusted), `alpha`, `beta`, `delta`, `average`, `optimism` (side to move), `seek_mate` | Before each root search of the aspiration loop. |
| `aspiration_result` | `value`, `fail` (`low`, `high`, `none`), `stopped` | After the root search and the stable sort of the root moves. |
| `iteration_end` | `depth`, `completed`, `score`, `uci_score`, `bound`, `seldepth`, `nodes`, `pv` (comma separated) | End of the iteration, after the mate and aborted search handling. |
| `search` | `ply`, `key`, `depth`, `alpha`, `beta`, `node` (`root`, `pv`, `nonpv`), `cut`, `excluded`, `nodes` | Each call of `search()`, before anything, with its arguments (`depth` may be 0 or less: the call goes on to `qsearch`). |
| `qsearch` | `ply`, `key`, `alpha`, `beta`, `node`, `nodes` | Each call of `qsearch()`. |
| `return` | `value` | The value the call returns, whichever `return` it is. |
| `repetition_draw` | `alpha` | `upcoming_repetition()` raised alpha to a draw value. |
| `immediate` | `reason` (`stop`, `draw`, `max_ply`) | Step 2 returns. |
| `mate_distance_cutoff` | | Step 3 returns. |
| `tt` | `hit`, `slot` (cluster:entry), then the entry in that slot as read: `key21`, `move`, `value`, `eval`, `depth`, `bound`, `pv`, `generation`; then `tt_move`, `tt_value` as search uses them (root move, `value_from_tt`), `tt_pv` (`ss->ttPv`, or `pvHit` in qsearch), `tt_capture` (search only) | After the probe. Without a hit the slot is the one chosen for replacement. |
| `static_eval` | search: `raw` (unadjusted, `VALUE_NONE` in check), `static` (corrected), `eval`, `correction` (the correction value), `improving`, `worsening`; qsearch: `raw`, `static`, `stand_pat`, `correction`, or `in_check=1` | Step 5, once `improving` and `opponentWorsening` are set (qsearch: before the stand pat test). |
| `store` | `slot` and the entry after the write, as in `tt` | After each `TTWriter::write()`. |
| `tt_penalize` | the same | After `TTWriter::penalize()` (step 6, case B). |
| `hindsight_extension`, `hindsight_reduction` | `depth` | The depth adjustments based on the prior reduction. |
| `tt_cutoff` | `checked` (the move after was probed) | Step 6 returns. |
| `tt_cutoff_refused` | `next_value` | The check of the position after the TT move refused the cutoff. |
| `tt_cutoff_rule50` | | A cutoff refused for a rule50 counter of 96 or more. |
| `eval_diff_update` | `move`, `bonus`, `main`, `pawn` (if updated) | The quiet move ordering update from the evaluation difference. |
| `razoring` | | Step 8, before the qsearch whose value is returned. |
| `futility_pruning` | `margin` | Step 9 returns. |
| `null_move` | `reduction` | Step 10, after `do_null_move()`. |
| `null_move_result`, `null_move_verification`, `null_move_verification_result` | `value`, `min_ply`, `value` | The null move search, and the verification search at high depth. |
| `iir` | `depth` | Step 11. |
| `probcut` | `beta`, `depth` | Step 12 starts; then `pick`, `skip`, `probcut_move` (after `do_move()`), `value`, `store`, `probcut_cutoff`. |
| `small_probcut` | | Step 13 returns. |
| `pick` | `move`, `stage` (`tt`, `good_capture`, `good_quiet`, `bad`, `evasion`, `qsearch`, `probcut`), `score` (not for the TT move) | Every move the move picker returns, in search, qsearch and ProbCut. |
| `skip` | `move`, `reason` (`excluded`, `root`) | The move is skipped before being counted. |
| `move` | `move`, `count`, `capture`, `check`, `reduction` (search only: `r` after the PV and ttPv terms) | The move is counted. |
| `skip_quiets` | | Each call of `skip_quiet_moves()` (step 15). |
| `prune` | `reason` and values: `capture_futility` (`value`), `capture_see` (`margin`), `continuation_history` (`history`), `quiet_futility` (`value`, `lmr_depth`), `quiet_see` (`lmr_depth`); qsearch: `move_count`, `futility` (`value`), `futility_see` (`threshold`), `not_capture`, `see` | Step 15 and qsearch step 6 `continue`. |
| `singular` | `beta`, `depth` | Step 16, before the excluded search; then `singular_extension` (`extension`, `depth`), `multi_cut` (returns) or `negative_extension`. |
| `lmr` | `reduction` (final `r`), `depth` (`d`), `stat` | Step 18, before the reduced search; then `lmr_research` (`depth`) before the full depth one. |
| `full_depth` | `reduction`, `stat` | Step 19, before the search. |
| `pv_search` | `depth` | Step 20, before the search. |
| `value` | `move`, `value` | After `undo_move()`. |
| `stopped` | | Step 22 returns because the search was stopped. |
| `root_move` | `move`, `score`, `uci_score`, `average`, `mean_squared`, `seldepth`, `bound` | After the update of the root move. |
| `new_best`, `new_alpha`, `cutoff` | `value`, `inc` (search); `alpha`, `depth`; `cutoff_count` (search) | Step 22 (qsearch step 8). |
| `fail_high_adjust`, `no_moves`, `checkmate`, `stalemate` | `value` | Step 23 (qsearch step 9). |
| `all_stats` | `bonus`, `malus` | `update_all_stats()`, followed by its updates. |
| `quiet_update` | `move`, `bonus`, `main`, `low_ply` (ply < 5), `continuation` (the entries at ply - 1, 2, 3, 4, 6 as `update_continuation_histories` updates them, `-` when skipped), `pawn` | After `update_quiet_histories()`. |
| `continuation_update` | `piece`, `to`, `bonus`, `values` | After a direct call of `update_continuation_histories()`. |
| `capture_update` | `move`, `bonus`, `value` | After a capture history update in `update_all_stats()`. |
| `prior_quiet_update`, `prior_capture_update` | `move`, `bonus`, `main`, `pawn`; `to`, `value` | The bonuses for the prior move of a node that failed low. |
| `ttmove_update` | `bonus`, `value` | After each update of the TT move history. |
| `correction_update` | `bonus`, `pawn`, `minor`, `white`, `black`, `continuation` (3 entries) | After `update_correction_history()`. |

The values printed after an update are the entries as they are after it, which
pins down the indexing and the `StatsEntry` gravity formula along with the
bonus.

### State digests

Each table is digested as a sequence of 64-bit little-endian words with 64-bit
FNV-1a over words (`h = (h ^ w) * 100000001B3`, from `CBF29CE484222325`). A table
of 16-bit entries gives one word per four entries, in row-major order of the C++
declaration: `main_history` [color][from-to], `low_ply_history` [ply][from-to],
`capture_history` [piece][to][captured type], `continuation_correction_history`
[piece][to][piece][to]. `continuation_history` is the digest of the words formed
by the digests of its four tables [in check][capture], each [piece][to][piece][to].
`pawn_history` and `correction_history` are the digest of the digests of their
entries (8192 entries of [piece][to]; 65536 entries of [color][pawn, minor,
white, black]). `ttmove_history` and `tt_generation` are plain numbers. `tt` is
the generation, then each cluster as four words: the packed 21-bit keys, then each
entry as `depth8 | genBound8 << 8 | move16 << 16 | value16 << 32 | eval16 << 48`.
A port that lays its tables out differently has to compute the digests in this
order.

### Bisecting

`verify.sh trace` (and `search_trace.py verify`) says, for each search that
differs, which members differ: an `info` line, the state tables, or the events,
as the window of 4096 events where the streams first differ (from the first
checkpoint that differs). `search_trace.py bisect CORPUS GOLDEN SESSION` then
runs that session alone with both the binary checked and the reference tracing
binary, prints the events of that window from both, and shows the first one that
differs, with the nodes open at that event (from the root down: each node, the
move being searched in it, and the event that called the next one) and the last
events in common. A session can be run alone since each one starts from a cleared state.

For instance, with the futility margin of step 9 changed from `45 + depth * 4`
to `46 + depth * 4`:

    $ EXE=changed/stockfish-trace ./search_trace.py bisect sample/standard.trace.gz sample/standard.trace.jsonl.gz 1 --before 4 --after 2
    session 1, search 1: rnbqkbnr/pp3ppp/2p1p3/3P4/8/2N3P1/PP1PPP1P/R1BQKBNR b KQkq - 0 4, go nodes 866
      events: the streams differ in events [0, 4096)
    events [0, 4096) of search 1, reference src/stockfish-trace
    nodes open at event 595, in the reference:
      515 search ply=0 key=6AB3E941184342DA depth=2 alpha=-32 beta=-22 node=root cut=0 excluded=- nodes=37
      582 move move=e6d5 count=2 capture=1 check=0 reduction=1676
      583 lmr reduction=-1675 depth=3 stat=752
      584 search ply=1 key=2E23BA34C0E386B5 depth=3 alpha=31 beta=32 node=nonpv cut=1 excluded=- nodes=42
      588 move move=d1a4 count=1 capture=0 check=0 reduction=948
      589 full_depth reduction=7712 stat=-1004
      590 search ply=2 key=B91FF66908B79420 depth=1 alpha=-32 beta=-31 node=nonpv cut=0 excluded=- nodes=43
    last events in common:
      591 tt hit=0 slot=47391:0 key21=000000 move=- value=0 eval=0 depth=-3 bound=none pv=0 generation=0 tt_move=- tt_value=32002 tt_pv=0 tt_capture=0
      592 store slot=47391:0 key21=179420 move=- value=32002 eval=73 depth=-2 bound=none pv=0 generation=1
      593 static_eval raw=73 static=73 eval=73 correction=-122355 improving=1 worsening=1
      594 eval_diff_update move=d1a4 bonus=-62 main=-623 pawn=-2013
    first difference, event 595:
      expected futility_pruning margin=-59
      got      futility_pruning margin=-61
    next events, expected / got:
      596 return value=5
      596 return value=5
      597 value move=d1a4 value=-5
      597 value move=d1a4 value=-5

A port only has to print its own events for the window: the reference events
come from `src/stockfish-trace`. The full trace of one search is
`searchtrace <file> events <n> 0 1000000000`.

### What it was checked to catch

Engines built with one deliberate change each (then reverted), checked with the
full corpora of `dump` and `dumpline`, the sample of the trace through plain
UCI, and the sample of the trace (for the TT ageing rule, the full corpus too):

| Change | bench | `dump` / `dumpline` | `trace-uci` (sample) | `trace` (sample) | First event that differs |
| --- | ---: | --- | --- | --- | --- |
| futility margin of step 9: `45 + depth * 4` to `46 + ...` | 2759672 | pass / pass | 211 | 408 | `futility_pruning margin=-59`, got `-61` |
| malus of `update_all_stats`: `968 * depth - 235` to `- 230` | 2092076 | pass / pass | 211 | 481 | `all_stats bonus=52 malus=733`, got `malus=738` |
| TT replacement: age weight 8 to 7 in `TranspositionTable::probe` | 3654537 | pass / pass | none (2 in the new sample) | none; 29 in the new sample; full corpus: 340, of which 21 change the UCI output | `tt hit=0 slot=31393:0 ...`, got `slot=31393:1`: another entry chosen for replacement |
| low ply history weight in the quiet move scores: `8 *` to `9 *` | 3471622 | pass / pass | 294 | 474 | `pick move=c6c5 stage=good_quiet score=-4796`, got `-4694` |
| threat term of the quiet move scores: `20 *` to `19 *` | 4428661 | `order_main_*` of 733 positions differ / pass | 141 | 392 | `pick move=f8a3 stage=bad score=-21296`, got `-20471` |

The numbers are of searches that differ, in the sample of 250 + 60 sessions
(492 searches) it had first; that sample missed the TT change, so 40 + 15
games were added to it ("new sample", 983 searches). A change that the
histories do not hide from `dump` (the last one) is caught by `dump` too; the
others only by the trace, and the TT one mostly not even in the UCI output.

### The corpus of the trace

`search_trace.py select` chooses, by a hash of each position or line (so that
the choice does not depend on the order of the corpora):

- single searches, each in its own session, on positions of the corpus of
  positions: 8,000 standard and 2,000 Chess960 positions, plus every position
  with a rule50 counter of 80 or more (222 and 10); `go depth 1` to `10` for 70%
  of them, `go nodes N` with N from 1 to about 6,300 (logarithmically spread)
  for the others, so that many searches stop inside an iteration;
- games, from the corpus of lines: 600 standard and 200 Chess960 lines, each a
  session of 8 searches at successive positions of the game, every ply or every
  other ply, then a 9th search of the first position again, whose entries are in
  the table from older generations; `go depth 5` to `9` for two sessions in
  three, `go nodes 2000` to `11999` for the others, the same for all the searches
  of a session.

| Set | Sessions | Searches | Inheriting | `go depth` / `go nodes` | Nodes | Events |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `standard` | 8,822 | 13,524 | 4,702 | 9,357 / 4,167 | 26,157,186 | 395,611,117 |
| `chess960` | 2,210 | 3,746 | 1,536 | 2,647 / 1,099 | 7,492,835 | 117,273,518 |

Of these searches, 299 have no legal move at the root, 5,144 iterations were
stopped by the node limit, 832 searches end with a mate score.

On the Pentium G3450 of this work (2 cores, no AVX2), with another build running
alongside: `make_trace_corpus.sh` takes 400 s, `verify.sh trace-full` 468 s
(about 85,000 nodes per second with the trace, against 375,000 for `bench`), and
`verify.sh trace-uci-full` 192 s; `verify.sh trace` on the sample 36 s. The
limits were chosen for that: a corpus of 33 million nodes, half an hour of work
in all, and golden files of 7 MB.

Number of events of each kind over the whole corpus (`searchtrace <file> counts`):

| Event | `standard` | `chess960` |
| --- | ---: | ---: |
| `all_stats` | 4,763,580 | 1,350,238 |
| `aspiration` | 174,749 | 52,192 |
| `aspiration_result` | 174,749 | 52,192 |
| `capture_update` | 1,834,883 | 579,249 |
| `checkmate` | 20,449 | 4,491 |
| `continuation_update` | 3,706,980 | 1,065,543 |
| `correction_update` | 2,249,122 | 624,109 |
| `cutoff` | 4,919,695 | 1,423,145 |
| `eval_diff_update` | 12,660,161 | 3,823,873 |
| `fail_high_adjust` | 4,528,909 | 1,346,351 |
| `full_depth` | 8,875,575 | 2,492,294 |
| `futility_pruning` | 8,939,141 | 2,758,440 |
| `hindsight_extension` | 545,189 | 162,963 |
| `hindsight_reduction` | 1,161,304 | 322,725 |
| `iir` | 340,387 | 104,629 |
| `immediate:draw` | 729,075 | 502 |
| `immediate:stop` | 6,552 | 1,764 |
| `iteration` | 97,608 | 26,729 |
| `iteration_end` | 97,608 | 26,729 |
| `lmr` | 13,403,024 | 3,799,960 |
| `lmr_research` | 293,016 | 87,981 |
| `mate_distance_cutoff` | 182,895 | 35,121 |
| `move` | 46,550,374 | 14,219,776 |
| `multi_cut` | 112,664 | 39,668 |
| `negative_extension` | 49,285 | 16,101 |
| `new_alpha` | 881,140 | 236,280 |
| `new_best` | 11,795,877 | 3,419,988 |
| `no_moves` | 31,739 | 6,061 |
| `null_move` | 289,832 | 95,735 |
| `null_move_result` | 289,832 | 95,735 |
| `null_move_verification` | 4,831 | 204 |
| `null_move_verification_result` | 4,831 | 204 |
| `pick:bad` | 18,398,688 | 5,909,061 |
| `pick:evasion` | 3,031,432 | 801,181 |
| `pick:good_capture` | 1,712,597 | 566,500 |
| `pick:good_quiet` | 15,765,504 | 4,500,007 |
| `pick:probcut` | 419,409 | 133,366 |
| `pick:qsearch` | 3,957,860 | 1,400,170 |
| `pick:tt` | 4,084,300 | 1,175,863 |
| `prior_capture_update` | 742,455 | 237,476 |
| `prior_quiet_update` | 2,123,357 | 613,646 |
| `probcut` | 1,167,504 | 353,121 |
| `probcut_cutoff` | 335,317 | 110,475 |
| `probcut_move` | 557,062 | 177,512 |
| `prune:capture_futility` | 1,659,097 | 619,456 |
| `prune:capture_see` | 1,926,212 | 612,950 |
| `prune:continuation_history` | 448,338 | 86,975 |
| `prune:futility` | 478,278 | 170,139 |
| `prune:futility_see` | 801,817 | 289,890 |
| `prune:move_count` | 766,287 | 289,752 |
| `prune:not_capture` | 887,476 | 228,502 |
| `prune:quiet_futility` | 11,322,473 | 3,766,167 |
| `prune:quiet_see` | 1,982,697 | 603,704 |
| `prune:see` | 564,911 | 197,250 |
| `pv_search` | 1,590,241 | 461,798 |
| `qsearch` | 8,102,931 | 2,348,382 |
| `quiet_update` | 7,296,151 | 1,998,115 |
| `razoring` | 192,806 | 62,293 |
| `repetition_draw` | 272,174 | 45,032 |
| `return` | 33,213,597 | 9,497,451 |
| `root_move` | 3,240,972 | 1,014,129 |
| `search` | 25,110,666 | 7,149,069 |
| `singular` | 279,462 | 94,257 |
| `singular_extension` | 114,582 | 37,721 |
| `skip:excluded` | 262,354 | 88,860 |
| `skip_quiets` | 5,183,548 | 1,684,777 |
| `small_probcut` | 114,776 | 29,982 |
| `stalemate` | 181 | 17 |
| `stand_pat` | 3,460,430 | 991,603 |
| `start` | 13,279 | 3,692 |
| `static_eval` | 25,569,664 | 7,625,681 |
| `stopped` | 19,711 | 5,658 |
| `store` | 19,308,020 | 5,746,699 |
| `tt` | 26,737,158 | 7,956,090 |
| `tt_cutoff` | 2,380,226 | 680,200 |
| `tt_cutoff_refused` | 600 | 138 |
| `tt_cutoff_rule50` | 131,430 | 0 |
| `tt_penalize` | 77,613 | 26,195 |
| `ttmove_update` | 3,933,232 | 1,118,709 |
| `value` | 26,157,186 | 7,492,835 |

Never reached: `immediate` with `max_ply` (a search 246 plies deep), and `skip`
with `root` (only with `searchmoves` or MultiPV, which the trace leaves out).


## What it does not cover

The positions of the corpus are set up from their FEN, and `dumpline` only
follows the lines of the corpus with a fixed schedule of evaluations, null moves
and probes. Not covered:

- The search itself, for `dump` and `dumpline`: see "The search trace" for what
  the trace covers of it, and below for what it leaves out.
- Play below a null move beyond its one reply: `plies_from_null` never goes
  past 1 there, so a repetition is never cut short by a null move, and no
  accumulator chain runs below one.
- Repetitions that reach back before the start of a line: the history before
  the start position is unknown to the engine, as with `position fen`.
- Accumulator chains longer than those of the schedule (three moves, or more
  in a sequence of checks) and search windows deeper than 64 plies; the reset
  of the stack is the same as at the root of a search, but a search can also
  reach the same depth through many different paths.
- `Position::see_ge`, `gives_check`, move generation and the move picker after
  moves. They work from the board and from the bitboards cached in `StateInfo`;
  `dumpline` checks the board (`fen`) and those bitboards after every move of
  the line, and the per-position corpus checks what is computed from them. The
  cached bitboards are not printed after probe, null and reply moves, nor after
  each `undo_move()` of the walk back (where `key`, `fen` and `nnue` are).
- The internal steps of the accumulator updates (dirty pieces, dirty threats,
  the refresh cache entries): only their result, the network output, is
  compared. A port that gets the output right through other steps passes.

Left out by the search trace:

- Time management: every search has a depth or node limit, never a time, and
  the members of the UCI output that depend on time (`time`, `nps`) are dropped.
  `TimeManagement`, `elapsed()`, the time part of `check_time()`, the
  `bestMoveChanges` and `effort` statistics and the stop decisions at the end of
  an iteration are not exercised; `effort` and `bestMoveChanges` are not even
  traced.
- More than one thread, MultiPV, `searchmoves`, `Skill Level` and
  `UCI_LimitStrength`, pondering, `go mate`, `go infinite`; Syzygy probing in
  search and at the root (no tablebases are loaded). These are fixed to their
  deterministic single values.
- Hash sizes other than 2 MB, and so the index of the clusters for other sizes.
- What happens inside a step without an event: for instance the conditions of a
  pruning that does not fire are not printed, only that it did not fire; the
  value of `lmrDepth` or of the history sums only when they prune; the steps of
  `partial_insertion_sort` only through the order and scores of the moves. Two
  ports that differ there in a way that changes no decision and no value pass.
- The NNUE internals, as for `dumpline`: only the evaluations are printed.
- State that is not carried from one search to the next and not printed:
  the node counter of the time check (`callsCnt`) shows only through where a
  node-limited search stops; the PV arrays only through the root PV.
- The `low_ply_history` of a search with no legal move at the root, which holds
  what an earlier search of the process left (`ucinewgame` does not clear it;
  every search fills it before use). It is reported as `null`.
- A search 246 plies deep (`max_ply`), the root `skip` (see the event counts).

## The corpus

| Set | Source | Games | Positions |
| --- | --- | --- | --- |
| `standard` | [lichess_db_standard_rated_2013-01.pgn.zst](https://database.lichess.org/standard/lichess_db_standard_rated_2013-01.pgn.zst) (17.8 MB) | 121,332 | 327,557 |
| `chess960` | [lichess_db_chess960_rated_2014-01.pgn.zst](https://database.lichess.org/chess960/lichess_db_chess960_rated_2014-01.pgn.zst) (3.6 MB) | 21,926 | 65,146 |

The checksums of the dumps are in `SHA256SUMS`; they are the ones published by
Lichess. `extract_positions.py` replays every game with python-chess and keeps
each position with a probability depending on what it exercises: 2% of ordinary
positions, but every position with a legal en passant capture or a double check,
40% of those with a pawn about to promote, 30% of the final positions of games
(mates, stalemates), 8% of checks, 8% to 50% of endgames with 7 pieces or fewer,
10% of the rarer castling right states. A position is kept when a hash of the
position itself is below its rate, which makes the selection deterministic. The
positions are distinct: two positions with the same pieces, side to move,
castling rights and en passant square are the same, whatever their counters. The
`chess960` set starts with the 960 start positions. In 17 of the Chess960 games
Lichess accepted, back then, a castling move through an occupied square; they
are replayed up to that move.

What the corpus covers, as measured on the golden files by `differential.py
stats`:

| | `standard` | `chess960` |
| --- | ---: | ---: |
| positions | 327,557 | 65,146 |
| in check | 71,748 | 11,972 |
| in double check | 2,172 | 362 |
| checkmate | 10,455 | 1,653 |
| stalemate | 250 | 27 |
| en passant capture available | 10,907 | 2,117 |
| promotion available | 19,577 | 2,639 |
| pinned piece (either side) | 48,586 | 8,972 |
| discovered check candidate | 5,776 | 1,116 |
| move giving check available | 135,841 | 25,279 |
| move with negative SEE available | 240,959 | 51,475 |
| some castling right left | 84,549 | 32,737 |
| castling move available | 23,254 | 11,598 |
| rule50 counter of 14 or more | 10,264 | 1,080 |
| 17 to 32 pieces | 184,458 | 46,987 |
| 8 to 16 pieces | 79,678 | 11,952 |
| 4 to 7 pieces | 50,000 | 5,010 |
| 3 pieces or fewer | 13,421 | 1,197 |

All 16 combinations of castling rights occur in `standard`, the rarest (`Qk`)
200 times, and all 9 combinations of their numbers per side in `chess960`.

### Lines of play

`extract_positions.py --lines` keeps whole games, from the same two dumps. A
game is kept with a probability depending on what happens in it: 1% of all
games, but 2% of those with a queenside castling, 10% of those with an en
passant capture, 4% with a promotion and 40% with an underpromotion, 4% with a
repeated position and 12% with a threefold repetition, 30% of those reaching 40
plies without a capture or pawn move and all of those reaching 80, 40% of the
games of 200 plies or more and all of those of 300. Chess960 games are kept
three times as often. Again a game is kept when a hash of the game itself is
below its rate. One kept game in four starts from a position in its course,
chosen by the hash, given by its FEN with the counters of the game: the line
then starts with a non-zero rule50 counter or en passant square, and no
history. Chess960 lines always start from a FEN.

| Set | Lines | Of which from a FEN | Moves | Records |
| --- | ---: | ---: | ---: | ---: |
| `standard` | 3,038 | 796 | 243,594 | 249,670 |
| `chess960` | 1,394 | 1,394 | 98,450 | 101,238 |

As measured by `differential.py --lines stats`:

| | `standard` | `chess960` |
| --- | ---: | ---: |
| positions (records but the walk backs) | 246,632 | 99,844 |
| evaluated (and null move made) | 141,082 | 57,563 |
| probe move made | 153,180 | 62,310 |
| positions evaluated on the walk back | 83,126 | 37,530 |
| in the second search window or later (ply 64+) | 84,357 | 27,992 |
| lines of 200 plies or more | 82 | 24 |
| castling moves (line / probe) | 3,859 / 3,810 | 1,163 / 4,016 |
| en passant captures (line / probe) | 334 / 78 | 145 / 26 |
| promotions (line / probe) | 1,495 / 1,041 | 518 / 391 |
| underpromotions (line) | 398 | 148 |
| captures (line) | 50,504 | 21,080 |
| king moves (line) | 50,359 | 17,280 |
| checks (line) | 24,107 | 9,208 |
| `repetition` > 0 | 3,205 | 828 |
| `repetition` < 0 (threefold) | 2,609 | 566 |
| `has_repeated` | 13,919 | 3,386 |
| `upcoming_repetition` | 11,584 | 3,121 |
| `is_draw` | 5,427 | 1,302 |
| rule50 counter of 14 or more | 15,579 | 4,009 |
| rule50 counter of 80 or more | 1,318 | 104 |
| rule50 counter of 100 or more | 86 | 2 |

Every value of these golden files was also checked against `dump` on the same
position set up from its FEN (keys, material, the cached bitboards, `nnue` and
`eval`, the key after the probe move, the key and `nnue` after the null move and
the key after its reply, 545,121 positions in all):
they agree, as they must for this engine.

## Files

In git:

| File | |
| --- | --- |
| `make_corpus.sh` | Rebuilds everything below from the dumps, in about 20 minutes on a slow machine. Fails, leaving the sample alone, if what it built is not what `CONTENT.sha256` records; `--update` records the new files instead. |
| `verify.sh` | Verifies a binary against the sample or the full golden files, of positions, of lines or of searches (`trace`, `trace-full`, and through plain UCI `trace-uci`, `trace-uci-full`). |
| `shared.sh` | Sourced by `make_corpus.sh`, `verify.sh` and `make_trace_corpus.sh`: the shared folder (`STOCKFISH_SHARED`) and the refusal to write into it. |
| `make_trace_corpus.sh` | Rebuilds the corpus of the search trace from the corpora of positions and lines, with `src/stockfish-trace`, in about 7 minutes; checks it against `CONTENT.trace.sha256` the same way (`--update` records). |
| `search_trace.py` | `select`, `generate`, `verify` (`--uci` for plain UCI), `bisect`, `sample` and `stats` for the search trace. The records are grouped by session, and each group must have the shape its session implies (searches numbered from 1, one per `go`, or an error ending the session), in the engine output as in the golden file. Standard library only; imports the file helpers of `differential.py`. |
| `extract_positions.py` | PGN on stdin to FENs, or with `--lines` to lines of play, on stdout. Needs python-chess, pinned in `requirements.txt` (1.11.2). |
| `differential.py` | `generate`, `verify`, `canonicalize`, `sample` and `stats`, with `--lines` for lines of play. With `--lines`, the records are parsed before they are grouped by line, and each group must have the shape its corpus line implies (plies 0 to `n` in order, then a walk back over `n` moves with `n` entries in each list, or a single error), in the engine output as in the golden file; `generate` and `sample` refuse to write anything else. Standard library only. |
| `requirements.txt` | The Python packages of `make_corpus.sh`: python-chess 1.11.2, for `extract_positions.py`. The shared folder's `venv/` is made from it. |
| `SHA256SUMS` | Checksums of the two dumps. |
| `CONTENT.sha256` | Checksums of the uncompressed corpus and golden files, to tell whether a rebuild gave the same ones. |
| `CONTENT.trace.sha256` | The same for the search trace. |
| `sample/` | 4,000 + 1,000 positions drawn uniformly from the two sets, with their golden lines (2.7 MB), 100 + 50 lines of play (`*.lines.gz`, 2.0 MB), and 290 + 75 sessions of the search trace (`*.trace.gz`, 983 searches, 0.5 MB: 250 + 60 drawn uniformly, plus 40 + 15 games). Also checked by `tests/instrumented.py`. |

Not in git, in `tests/differential/data` when built there, or in the shared
folder (260 MB):

| File | Size | Uncompressed |
| --- | ---: | ---: |
| `downloads/*.pgn.zst` | 21 MB | never decompressed to disk |
| `standard.fen.zst`, `chess960.fen.zst` | 6.6 MB | 21 MB |
| `standard.jsonl.zst` | 146 MB | 603 MB |
| `chess960.jsonl.zst` | 31 MB | 125 MB |
| `standard.lines.zst`, `chess960.lines.zst` | 0.6 MB | 1.9 MB |
| `standard.lines.jsonl.zst` | 34 MB | 335 MB |
| `chess960.lines.jsonl.zst` | 15 MB | 137 MB |
| `standard.trace.zst`, `chess960.trace.zst` | 0.3 MB | 2.6 MB |
| `standard.trace.jsonl.zst` | 5.5 MB | 31 MB |
| `chess960.trace.jsonl.zst` | 1.6 MB | 9 MB |

The dumps and the golden files are never decompressed to disk. What is written
uncompressed is small: the candidate positions during a rebuild (17 MB, deleted
once the corpus is written) and the file of FENs handed to the engine, in a
temporary directory (17 MB at most; 1.3 MB of lines). Memory is not bounded independently of the
corpus, though: the extraction and the sampling keep one hash per position, and
`canonicalize` keeps the FENs, which is some tens of MB at this size.
