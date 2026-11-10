#!/usr/bin/env python3
"""Extract a sample of distinct positions from a PGN stream.

Reads PGN (Lichess database format) on stdin and writes one FEN per line on
stdout. Games are replayed with python-chess and each position reached is kept
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


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--chess960", action="store_true", help="games are Chess960 games")
    parser.add_argument("--max-games", type=int, default=0, help="stop after that many games")
    parser.add_argument("--jobs", type=int, default=2, help="number of worker processes")
    args = parser.parse_args()

    def games():
        stream = sys.stdin
        for n, (fen, movetext) in enumerate(read_games(stream)):
            if args.max_games and n >= args.max_games:
                break
            yield fen, movetext, args.chess960

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
