# Differential tests

The other tests of this directory (perft, bench signature, `instrumented.py`)
tell whether an engine behaves like Stockfish, not where it differs. This
harness compares engines one component at a time. It consists of

- the `dump` debug command of the engine, which prints the internals of a
  position (hash keys, check and pin bitboards, static evaluation, legal moves,
  static exchange evaluation, move ordering) as one line of JSON,
- a corpus of 392,703 distinct positions taken from real games, and the golden
  files holding what this engine prints for each of them,
- scripts to rebuild the corpus and to verify a binary against the golden files.

The golden files describe one exact engine: the commit they were generated at
(`43e01e07`, bench 3464885) with its default network `nn-18cd32e11a9c.nnue`.
They are not meant to follow master: any functional change to the evaluation,
the network, SEE or move ordering changes them.

## Quick start

    cd src && make -j2 build                    # the reference engine
    ../tests/differential/verify.sh             # 5,000 positions, kept in git
    ../tests/differential/make_corpus.sh        # download and rebuild everything
    ../tests/differential/verify.sh full        # 392,703 positions

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

## What it does not cover

Every position is set up from its FEN, and everything but `key_after` is
computed on that fresh position. A port can therefore pass with a wrong
incremental update of the pawn, material, minor piece or non-pawn keys, or of
the NNUE accumulators, as both are only ever built from scratch here. Nothing
goes deeper than one move either: repetitions, null moves and the state kept
along a line of play are out of reach, and so is the search itself. Comparing
each incremental value with its from-scratch one inside the port covers the
first gap; perft and the bench signature remain the tests of the rest.

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

## Files

In git:

| File | |
| --- | --- |
| `make_corpus.sh` | Rebuilds everything below from the dumps, in about 3 minutes. Fails, leaving the sample alone, if what it built is not what `CONTENT.sha256` records; `--update` records the new files instead. |
| `verify.sh` | Verifies a binary against the sample or the full golden files. |
| `extract_positions.py` | PGN on stdin to FENs on stdout. Needs python-chess (`pip install chess`, 1.11.2 was used). |
| `differential.py` | `generate`, `verify`, `canonicalize`, `sample` and `stats`. Standard library only. |
| `SHA256SUMS` | Checksums of the two dumps. |
| `CONTENT.sha256` | Checksums of the uncompressed corpus and golden files, to tell whether a rebuild gave the same ones. |
| `sample/` | 4,000 + 1,000 positions drawn uniformly from the two sets, with their golden lines (2.7 MB). Also checked by `tests/instrumented.py`. |

Not in git, in `tests/differential/data` (205 MB):

| File | Size | Uncompressed |
| --- | ---: | ---: |
| `downloads/*.pgn.zst` | 21 MB | never decompressed to disk |
| `standard.fen.zst`, `chess960.fen.zst` | 6.6 MB | 21 MB |
| `standard.jsonl.zst` | 146 MB | 603 MB |
| `chess960.jsonl.zst` | 31 MB | 125 MB |

The dumps and the golden files are never decompressed to disk. What is written
uncompressed is small: the candidate positions during a rebuild (17 MB, deleted
once the corpus is written) and the file of FENs handed to the engine, in a
temporary directory (17 MB at most). Memory is not bounded independently of the
corpus, though: the extraction and the sampling keep one hash per position, and
`canonicalize` keeps the FENs, which is some tens of MB at this size.
