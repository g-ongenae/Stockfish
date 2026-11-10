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
- a corpus of 392,703 distinct positions and one of 4,432 lines of play
  (342,044 moves), taken from real games, and the golden files holding what
  this engine prints for each of them,
- scripts to rebuild the corpora and to verify a binary against the golden files.

The golden files describe one exact engine: the commit they were generated at
(`43e01e07`, bench 3464885) with its default network `nn-18cd32e11a9c.nnue`.
They are not meant to follow master: any functional change to the evaluation,
the network, SEE, move ordering or repetition detection changes them.

## Quick start

    cd src && make -j2 build                    # the reference engine
    ../tests/differential/verify.sh             # 5,000 positions, kept in git
    ../tests/differential/verify.sh lines       # 150 lines of play, kept in git
    ../tests/differential/make_corpus.sh        # download and rebuild everything
    ../tests/differential/verify.sh full        # 392,703 positions
    ../tests/differential/verify.sh lines-full  # 4,432 lines of play

    # another binary, and only what it implements so far
    EXE=/path/to/engine ../tests/differential/verify.sh full --fields fen,key,moves

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

    {"ply":2,"move":"d7d5","fen":"rnbqkbnr/ppp1pppp/8/3p4/4P3/8/PPPP1PPP/RNBQKBNR w KQkq - 0 2",
     "key":"96270AD18105C788","pawn_key":"88C3B25994A695DA","material_key":"A0710EF633754DD6",
     "minor_piece_key":"41AF5A253E95F502","non_pawn_key":["85DC4DDCD53BCFCC","54B51DF93C8CF372"],
     "non_pawn_material":[8302,8302],"checkers":"0000000000000000","captured":null,
     "plies_from_null":2,"repetition":0,"is_draw":false,"has_repeated":false,
     "upcoming_repetition":false,"nnue":25,"eval":31,
     "null":{"key":"36D22A7548FF9C44","fen":"rnbqkbnr/ppp1pppp/8/3p4/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1","nnue":103},
     "probe":{"move":"e4d5","key":"06B960AABFBB9995","pawn_key":"B8A8F88663E2900B",
              "material_key":"2CC3FFFEAC908A6B","minor_piece_key":"41AF5A253E95F502",
              "non_pawn_key":["85DC4DDCD53BCFCC","54B51DF93C8CF372"],
              "checkers":"0000000000000000","nnue":-136}}
    ...
    {"undo":4,"keys":["06B960AABFBB9995","96270AD18105C788","B46022469E3DD31B","8F8F01D4562F59FB"],
     "nnues":[-136,25,-24,34],"fen":"rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1",
     "key":"8F8F01D4562F59FB", ...}

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
| `captured` | `StateInfo::capturedPiece`, as a FEN letter, or `null`. |
| `plies_from_null` | `StateInfo::pliesFromNull`; it counts from the start of the line, not of the game. |
| `repetition` | `StateInfo::repetition`: the distance in plies to the previous occurrence of the position within the reach of rule50 and the last null move, negative if that one was itself a repetition, else 0. |
| `is_draw`, `upcoming_repetition` | `Position::is_draw(ply)` and `upcoming_repetition(ply)`, with `ply` the number of moves since the root of the current search window (see below), as search passes them. |
| `has_repeated` | `Position::has_repeated()`. |
| `nnue` | `Network::evaluate()` on the accumulator stack, as search calls it. `null` when in check, and at the plies that are skipped (see below). |
| `eval` | `Eval::evaluate()` with an optimism of 0, `null` when `nnue` is. |
| `null` | A null move made and undone, as in null move pruning, at the positions that are evaluated: the `key` and `fen` after `do_null_move()`, and the `nnue` then, from the same accumulators. `null` when in check or skipped. |
| `probe` | A move other than the one of the line, made with an accumulator push and undone with a pop, as search does with the children of a node: its `move`, the keys and `checkers` after it, and the `nnue` then (`null` if it gives check). `null` when skipped or without legal moves. |

The walk back record has `undo`, the number of moves undone; `keys`, the `key`
after each `undo_move()`, from the next to last position down to the start
position; `nnues`, the evaluation at each of these positions, popping the
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
- At each position that is evaluated, the null move comes after the evaluation,
  then the probe move, then the move of the line.
- The probe is chosen among the legal moves sorted by their UCI notation, so that
  the order of move generation does not matter. They are grouped in classes:
  special (castling, en passant, promotions), other captures, other king moves,
  and all. At ply `p` the first non-empty class in the preferences number
  `p % 4`, which are (special, captures, all), (king moves, captures, all),
  (captures, special, all) and (all), gives the probe: its move number
  `(p / 4) % size` of the class.
- On the walk back the positions of the last window are evaluated again,
  reusing the accumulators computed on the way up, including those computed
  backward or as intermediate steps of a chain.

A wrong `undo_move()` or `undo_null_move()` shows up in the records that
follow, since the line goes on from the position they restored.

## What it does not cover

The positions of the corpus are set up from their FEN, and `dumpline` only
follows the lines of the corpus with a fixed schedule of evaluations, null moves
and probes. Not covered:

- The search itself: move ordering with real history tables, pruning,
  extensions, the transposition table, time management. Perft and the bench
  signature remain the tests of these.
- Moves after a null move: the null move is undone at once, so
  `plies_from_null` never restarts from 0 along a line, and repetitions are
  never cut short by a null move.
- Repetitions that reach back before the start of a line: the history before
  the start position is unknown to the engine, as with `position fen`.
- Accumulator chains longer than those of the schedule (three moves, or more
  in a sequence of checks) and search windows deeper than 64 plies; the reset
  of the stack is the same as at the root of a search, but a search can also
  reach the same depth through many different paths.
- `Position::see_ge`, `gives_check`, move generation and the move picker after
  moves: they are computed from scratch from the board, so the per-position
  corpus covers them, provided the board itself is right, which the `fen`
  member checks.
- The internal steps of the accumulator updates (dirty pieces, dirty threats,
  the refresh cache entries): only their result, the network output, is
  compared. A port that gets the output right through other steps passes.

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
| probe move made | 155,611 | 63,103 |
| positions evaluated on the walk back | 83,126 | 37,530 |
| in the second search window or later (ply 64+) | 84,357 | 27,992 |
| lines of 200 plies or more | 82 | 24 |
| castling moves (line / probe) | 3,859 / 4,673 | 1,163 / 4,183 |
| en passant captures (line / probe) | 334 / 178 | 145 / 70 |
| promotions (line / probe) | 1,495 / 1,019 | 518 / 384 |
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
position set up from its FEN (keys, material, checkers, `nnue` and `eval`, the
key after the probe move and after the null move, 545,121 positions in all):
they agree, as they must for this engine.

## Files

In git:

| File | |
| --- | --- |
| `make_corpus.sh` | Rebuilds everything below from the dumps, in about 20 minutes on a slow machine. Fails, leaving the sample alone, if what it built is not what `CONTENT.sha256` records; `--update` records the new files instead. |
| `verify.sh` | Verifies a binary against the sample or the full golden files, of positions or of lines. |
| `extract_positions.py` | PGN on stdin to FENs, or with `--lines` to lines of play, on stdout. Needs python-chess (`pip install chess`, 1.11.2 was used). |
| `differential.py` | `generate`, `verify`, `canonicalize`, `sample` and `stats`, with `--lines` for lines of play. Standard library only. |
| `SHA256SUMS` | Checksums of the two dumps. |
| `CONTENT.sha256` | Checksums of the uncompressed corpus and golden files, to tell whether a rebuild gave the same ones. |
| `sample/` | 4,000 + 1,000 positions drawn uniformly from the two sets, with their golden lines (2.7 MB), and 100 + 50 lines of play (`*.lines.gz`, 1.3 MB). Also checked by `tests/instrumented.py`. |

Not in git, in `tests/differential/data` (237 MB):

| File | Size | Uncompressed |
| --- | ---: | ---: |
| `downloads/*.pgn.zst` | 21 MB | never decompressed to disk |
| `standard.fen.zst`, `chess960.fen.zst` | 6.6 MB | 21 MB |
| `standard.jsonl.zst` | 146 MB | 603 MB |
| `chess960.jsonl.zst` | 31 MB | 125 MB |
| `standard.lines.zst`, `chess960.lines.zst` | 0.6 MB | 1.9 MB |
| `standard.lines.jsonl.zst` | 22 MB | 179 MB |
| `chess960.lines.jsonl.zst` | 9.7 MB | 73 MB |

The dumps and the golden files are never decompressed to disk. What is written
uncompressed is small: the candidate positions during a rebuild (17 MB, deleted
once the corpus is written) and the file of FENs handed to the engine, in a
temporary directory (17 MB at most; 1.3 MB of lines). Memory is not bounded independently of the
corpus, though: the extraction and the sampling keep one hash per position, and
`canonicalize` keeps the FENs, which is some tens of MB at this size.
