#!/usr/bin/env python3
"""Extract a sample of distinct positions, or of game lines, from a PGN stream.

Reads PGN (Lichess database format) on stdin and writes one FEN per line on
stdout, or with --lines one line of play per line (see below). Games are replayed with python-chess and each position reached is kept
with a probability that depends on what makes it interesting: positions that are
rare but awkward for a chess engine (en passant, promotions, double checks,
unusual castling rights, few pieces, game ends) are kept far more often than
ordinary ones.

The selection is deterministic: a position is kept when a hash of the position
itself falls below the rate of its category, so the same input always yields the
same output, and a position is selected (or not) no matter how often it occurs.
Positions are deduplicated on piece placement, side to move, castling rights and
en passant square; the first occurrence provides the move counters.

    zstdcat lichess_db_standard_rated_2013-01.pgn.zst | extract_positions.py > out.fen

With --lines, whole games are kept instead, as the arguments of the UCI
'position' command: "startpos moves e2e4 ..." or "fen <FEN> moves ...". A game
is kept with a probability that depends on what it plays along the way
(castling, en passant, promotions, repetitions, long stretches without a capture
or pawn move, length), again decided by a hash of the game itself. One kept game
in four starts from a position in its course instead of its start, given by its
FEN with the counters of the game.

    zstdcat lichess_db_standard_rated_2013-01.pgn.zst | extract_positions.py --lines > out.lines

Requires python-chess (pip install chess).
"""

import argparse
import hashlib
import multiprocessing
import re
import struct
import sys

import chess

# Probability of keeping a position, by category. A position gets the highest
# rate among the categories it belongs to. The values were chosen from the
# category frequencies in the Lichess 2013-01 dump, to end up with a few hundred
# thousand positions in which no category is anecdotal.
RATES = {
    "base": 0.02,  # any position
    "check": 0.08,  # side to move is in check
    "double_check": 1.0,  # ... by two pieces
    "en_passant": 1.0,  # a legal en passant capture exists
    "promotion": 0.4,  # side to move has a pawn on its seventh rank
    "castling": 0.1,  # castling rights other than all, none, or one side's two
    "pieces_7": 0.08,  # at most 7 pieces on the board
    "pieces_5": 0.25,  # at most 5 pieces
    "pieces_3": 0.5,  # at most 3 pieces
    "final": 0.3,  # last position of the game (mates, stalemates, ...)
}

CATEGORIES = tuple(RATES)

# Probability of keeping a game with --lines, by what happens in it. A game gets
# the highest rate among the categories it belongs to. Chosen from the category
# frequencies in the two dumps (see --count), for about 430,000 plies in all.
LINE_RATES = {
    "base": 0.01,  # any game
    "castling_queenside": 0.02,  # a queenside castling move
    "en_passant": 0.1,  # an en passant capture
    "promotion": 0.04,  # a promotion
    "underpromotion": 0.4,  # a promotion to a knight, bishop or rook
    "repetition": 0.04,  # a position occurs twice
    "threefold": 0.12,  # ... three times
    "rule50_40": 0.3,  # 40 plies without a capture or pawn move
    "rule50_80": 1.0,  # 80 plies, close to the fifty-move limit
    "long": 0.4,  # 200 plies or more
    "very_long": 1.0,  # 300 plies or more
}

# The Chess960 dump is about six times smaller, and its castling moves are the
# interesting ones: its games are kept three times as often (at most always).
LINE_RATE_SCALE_CHESS960 = 3

LINE_CATEGORIES = tuple(LINE_RATES)

# Castling rights as python-chess bitboards of rook squares
COMMON_CASTLING = {
    0,
    chess.BB_A1 | chess.BB_H1,
    chess.BB_A8 | chess.BB_H8,
    chess.BB_A1 | chess.BB_H1 | chess.BB_A8 | chess.BB_H8,
}

TAG = re.compile(r'^\[(\w+) "(.*)"\]\s*$')
NOISE = re.compile(r"\{[^}]*\}|\([^)]*\)|\$\d+|\d+\.+|[!?]+")
RESULTS = {"1-0", "0-1", "1/2-1/2", "*"}
PACK = struct.Struct("<9Q")
SCALE = float(1 << 64)


def read_games(stream):
    """Yields (start FEN or None, movetext) for each game of a PGN stream."""
    fen, movetext, in_moves = None, [], False
    for line in stream:
        line = line.strip()
        if line.startswith("["):
            if in_moves:
                yield fen, " ".join(movetext)
                fen, movetext, in_moves = None, [], False
            tag = TAG.match(line)
            if tag and tag.group(1) == "FEN":
                fen = tag.group(2)
        elif line:
            in_moves = True
            movetext.append(line)
    if in_moves:
        yield fen, " ".join(movetext)


def categories(board, in_check, is_final, chess960):
    """Returns the names of the categories the position belongs to."""
    found = ["base"]
    if in_check:
        found.append("check")
        if chess.popcount(board.checkers_mask()) > 1:
            found.append("double_check")
    if board.ep_square is not None and board.has_legal_en_passant():
        found.append("en_passant")
    seventh = chess.BB_RANK_7 if board.turn else chess.BB_RANK_2
    if board.pawns & board.occupied_co[board.turn] & seventh:
        found.append("promotion")
    # In Chess960 the rooks start anywhere, so tell the states apart by count
    rights = board.castling_rights
    if (chess.popcount(rights) in (1, 3)) if chess960 else (rights not in COMMON_CASTLING):
        found.append("castling")
    pieces = chess.popcount(board.occupied)
    if pieces <= 7:
        found.append("pieces_7")
        if pieces <= 5:
            found.append("pieces_5")
            if pieces <= 3:
                found.append("pieces_3")
    if is_final:
        found.append("final")
    return found


def position_hash(board):
    """Hashes what makes two positions the same: everything but the counters."""
    ep = board.ep_square if board.ep_square is not None and board.has_legal_en_passant() else 64
    return hashlib.blake2b(
        PACK.pack(
            board.pawns,
            board.knights,
            board.bishops,
            board.rooks,
            board.queens,
            board.kings,
            board.occupied_co[chess.WHITE],
            board.castling_rights,
            ep * 2 + board.turn,
        ),
        digest_size=8,
    ).digest()


def select(board, in_check, is_final, chess960):
    """Returns (hash, FEN, categories) if the position is kept, else None."""
    found = categories(board, in_check, is_final, chess960)
    digest = position_hash(board)
    if int.from_bytes(digest, "little") >= max(RATES[c] for c in found) * SCALE:
        return None
    if chess960:
        fen = board.shredder_fen(en_passant="legal")
    else:
        fen = board.fen(en_passant="legal")
    return digest, fen, found


def replay(game):
    """Replays one game and returns its selected positions and an error flag."""
    start_fen, movetext, chess960 = game
    selected = []
    try:
        board = chess.Board(start_fen, chess960=chess960) if start_fen else chess.Board()
    except ValueError:
        return selected, True

    tokens = [t for t in NOISE.sub(" ", movetext).split() if t not in RESULTS]
    if not tokens:
        return selected, False

    # The start position is one of the 960, they are added separately
    if not chess960:
        kept = select(board, False, False, chess960)
        if kept:
            selected.append(kept)

    last = len(tokens) - 1
    for i, san in enumerate(tokens):
        try:
            board.push_san(san)
        except ValueError:
            return selected, True
        kept = select(board, san[-1] in "+#", i == last, chess960)
        if kept:
            selected.append(kept)
    return selected, False


def line_categories(moves_info, plies, max_rule50, max_occurrences):
    """Returns the names of the categories a game belongs to."""
    found = ["base"]
    found += sorted(moves_info)
    if max_occurrences >= 2:
        found.append("repetition")
        if max_occurrences >= 3:
            found.append("threefold")
    if max_rule50 >= 40:
        found.append("rule50_40")
        if max_rule50 >= 80:
            found.append("rule50_80")
    if plies >= 200:
        found.append("long")
        if plies >= 300:
            found.append("very_long")
    return found


def start_fen(board, chess960):
    return board.shredder_fen(en_passant="legal") if chess960 else board.fen(en_passant="legal")


def replay_line(game):
    """Replays one game and returns (line or None, categories, error flag).

    A game with an unreadable move is kept up to that move.
    """
    pgn_fen, movetext, chess960, count_only = game
    try:
        board = chess.Board(pgn_fen, chess960=chess960) if pgn_fen else chess.Board(chess960=chess960)
    except ValueError:
        return None, [], True

    tokens = [t for t in NOISE.sub(" ", movetext).split() if t not in RESULTS]
    start = start_fen(board, chess960) if pgn_fen else None
    moves, kinds = [], set()
    occurrences = {board._transposition_key(): 1}
    max_occurrences = 1
    max_rule50 = 0
    error = False

    for san in tokens:
        try:
            move = board.parse_san(san)
        except ValueError:
            error = True
            break
        if board.is_castling(move) and board.is_queenside_castling(move):
            kinds.add("castling_queenside")
        if board.is_en_passant(move):
            kinds.add("en_passant")
        if move.promotion:
            kinds.add("promotion" if move.promotion == chess.QUEEN else "underpromotion")
        moves.append(board.uci(move))
        board.push(move)
        key = board._transposition_key()
        occurrences[key] = occurrences.get(key, 0) + 1
        max_occurrences = max(max_occurrences, occurrences[key])
        max_rule50 = max(max_rule50, board.halfmove_clock)

    if not moves:
        return None, [], error

    found = line_categories(kinds, len(moves), max_rule50, max_occurrences)
    if count_only:
        return len(moves), found, error

    digest = hashlib.blake2b(((start or "") + " " + " ".join(moves)).encode(), digest_size=8).digest()
    value = int.from_bytes(digest, "little")
    scale = LINE_RATE_SCALE_CHESS960 if chess960 else 1
    if value >= min(1.0, scale * max(LINE_RATES[c] for c in found)) * SCALE:
        return None, found, error

    # One line in four starts in the course of the game, at a ply given by
    # another part of the hash
    begin = 0
    if (value & 3) == 0 and len(moves) > 1:
        begin = 1 + (value >> 2) % (len(moves) - 1)
        board = chess.Board(pgn_fen, chess960=chess960) if pgn_fen else chess.Board(chess960=chess960)
        for uci in moves[:begin]:
            board.push(board.parse_uci(uci))
        start = start_fen(board, chess960)

    head = f"fen {start}" if start else "startpos"
    return f"{head} moves {' '.join(moves[begin:])}", found, error


def main_lines(args, games):
    """The --lines mode of main()."""
    seen = set()
    counts = dict.fromkeys(LINE_CATEGORIES, 0)
    plies = dict.fromkeys(LINE_CATEGORIES, 0)
    num_games = num_errors = num_duplicates = num_lines = num_plies = 0

    with multiprocessing.Pool(args.jobs) as pool:
        # imap() keeps the order of the games, hence the output is deterministic
        for line, found, error in pool.imap(replay_line, games(), chunksize=200):
            num_games += 1
            num_errors += error
            if args.count:
                # Frequencies over all the games, to choose the rates
                if line:
                    for category in found:
                        counts[category] += 1
                        plies[category] += line
                continue
            if not line:
                continue
            digest = hashlib.blake2b(line.encode(), digest_size=8).digest()
            if digest in seen:
                num_duplicates += 1
                continue
            seen.add(digest)
            num_lines += 1
            length = len(line.split(" moves ", 1)[1].split()) if " moves " in line else 0
            num_plies += length
            for category in found:
                counts[category] += 1
                plies[category] += length
            sys.stdout.write(line + "\n")

    print(
        f"{num_games} games ({num_errors} cut short by an unreadable move), "
        f"{num_lines} lines kept with {num_plies} plies, {num_duplicates} duplicates dropped",
        file=sys.stderr,
    )
    for category in LINE_CATEGORIES:
        print(f"  {category:18} {counts[category]:8} lines {plies[category]:9} plies", file=sys.stderr)


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--chess960", action="store_true", help="games are Chess960 games")
    parser.add_argument("--max-games", type=int, default=0, help="stop after that many games")
    parser.add_argument("--jobs", type=int, default=2, help="number of worker processes")
    parser.add_argument("--lines", action="store_true", help="keep whole games as lines of play")
    parser.add_argument(
        "--count", action="store_true", help="with --lines, only count the games and plies of each category"
    )
    args = parser.parse_args()

    def games():
        stream = sys.stdin
        for n, (fen, movetext) in enumerate(read_games(stream)):
            if args.max_games and n >= args.max_games:
                break
            if args.lines:
                yield fen, movetext, args.chess960, args.count
            else:
                yield fen, movetext, args.chess960

    if args.lines:
        main_lines(args, games)
        return

    seen = set()
    counts = dict.fromkeys(CATEGORIES, 0)
    num_games = num_errors = num_duplicates = 0

    out = sys.stdout
    if args.chess960:
        # All the Chess960 start positions, the games only reach some of them
        for n in range(960):
            board = chess.Board.from_chess960_pos(n)
            seen.add(position_hash(board))
            out.write(board.shredder_fen(en_passant="legal") + "\n")

    with multiprocessing.Pool(args.jobs) as pool:
        # imap() keeps the order of the games, hence the output is deterministic
        for selected, error in pool.imap(replay, games(), chunksize=200):
            num_games += 1
            num_errors += error
            for digest, fen, found in selected:
                if digest in seen:
                    num_duplicates += 1
                    continue
                seen.add(digest)
                for category in found:
                    counts[category] += 1
                out.write(fen + "\n")

    print(
        f"{num_games} games ({num_errors} cut short by an unreadable move), "
        f"{len(seen)} positions kept, {num_duplicates} duplicates dropped",
        file=sys.stderr,
    )
    for category in CATEGORIES:
        print(f"  {category:13} {counts[category]:8}", file=sys.stderr)


if __name__ == "__main__":
    main()
