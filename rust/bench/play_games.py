#!/usr/bin/env python3
"""Complete games between the port and the reference on a real clock
(README.md, "Time management", gate 3): each opening of the sample's lines of
play (its first plies) is played twice, colours swapped, single-threaded,
with the clock kept here from the wall time of each move. Reports the games
played, the losses on time, illegal moves and crashes of each engine (the
port's must all be zero), and the score, for information only: it is not a
strength test. With --ponder both engines ponder (two cores).

Needs python-chess: run it with the shared folder's Python
($STOCKFISH_SHARED/venv/bin/python); nothing is to be installed.

    play_games.py [--games N] [--time S] [--inc S] [--ponder] [--plies P]

EXE is the port, REF the reference (default: the shared folder's
oracle/stockfish).
"""

import argparse
import gzip
import os
import sys
import time

import chess
import chess.engine

S = os.environ.get("STOCKFISH_SHARED", os.path.expanduser("~/.local/share/stockfish-fork"))
HERE = os.path.dirname(os.path.abspath(__file__))


def openings(plies):
    path = os.path.join(HERE, "..", "..", "tests", "differential", "sample", "standard.lines.gz")
    with gzip.open(path, "rt") as f:
        for line in f:
            words = line.split()
            if words[0] == "startpos" and "moves" in words:
                yield words[2 : 2 + plies]


def play(white, black, opening, base, inc, ponder):
    """One game; returns (result, reason, loser on time or by fault)."""
    board = chess.Board()
    for m in opening:
        board.push_uci(m)
    clock = {chess.WHITE: base, chess.BLACK: base}
    engines = {chess.WHITE: white, chess.BLACK: black}
    # (python-chess sets the Ponder option itself, from ponder=)
    while not board.is_game_over(claim_draw=True) and board.ply() < 400:
        stm = board.turn
        limit = chess.engine.Limit(white_clock=clock[chess.WHITE], black_clock=clock[chess.BLACK],
                                   white_inc=inc, black_inc=inc)
        t = time.perf_counter()
        try:
            r = engines[stm].play(board, limit, ponder=ponder, game=id(board))
        except chess.engine.EngineError as e:
            return "0-1" if stm == chess.WHITE else "1-0", f"engine error: {e}", stm
        except chess.engine.EngineTerminatedError as e:
            return "0-1" if stm == chess.WHITE else "1-0", f"crash: {e}", stm
        used = time.perf_counter() - t
        clock[stm] -= used
        if clock[stm] < 0:
            return "0-1" if stm == chess.WHITE else "1-0", f"time ({clock[stm] * 1000:.1f} ms)", stm
        clock[stm] += inc
        if r.move is None or r.move not in board.legal_moves:
            return "0-1" if stm == chess.WHITE else "1-0", f"illegal move {r.move}", stm
        board.push(r.move)
    outcome = board.outcome(claim_draw=True)
    return (outcome.result() if outcome else "1/2-1/2"), (outcome.termination.name if outcome else "400 plies"), None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--games", type=int, default=24)
    ap.add_argument("--time", type=float, default=10.0)
    ap.add_argument("--inc", type=float, default=0.1)
    ap.add_argument("--plies", type=int, default=8)
    ap.add_argument("--ponder", action="store_true")
    args = ap.parse_args()
    exe = os.environ.get("EXE") or sys.exit("EXE must name the port")
    ref = os.environ.get("REF", os.path.join(S, "oracle", "stockfish"))

    port = chess.engine.SimpleEngine.popen_uci(exe)
    cpp = chess.engine.SimpleEngine.popen_uci(ref)
    for e in (port, cpp):
        e.configure({"Threads": 1, "Hash": 16})
    score = {"port": 0.0, "C++": 0.0}
    faults = {who: {"time": 0, "illegal": 0, "crash": 0, "error": 0} for who in ("port", "C++")}
    played = 0
    ops = list(openings(args.plies))
    for g in range(args.games):
        opening = ops[(g // 2) % len(ops)]
        port_white = g % 2 == 0
        white, black = (port, cpp) if port_white else (cpp, port)
        t = time.time()
        result, reason, loser = play(white, black, opening, args.time, args.inc, args.ponder)
        played += 1
        pw = {"1-0": 1.0, "0-1": 0.0}.get(result, 0.5)
        score["port"] += pw if port_white else 1 - pw
        score["C++"] += 1 - pw if port_white else pw
        if loser is not None:
            key = reason.split()[0].rstrip(":")
            who = "port" if (loser == chess.WHITE) == port_white else "C++"
            faults[who][{"time": "time", "illegal": "illegal", "crash": "crash"}.get(key, "error")] += 1
            reason += f" ({who})"
        print(f"game {g + 1}: port {'white' if port_white else 'black'}: {result} {reason}, "
              f"{time.time() - t:.0f} s", flush=True)
    port.quit()
    cpp.quit()
    print(f"{played} games at {args.time:g}+{args.inc:g} s{' with ponder' if args.ponder else ''}: "
          f"port {score['port']:g}, C++ {score['C++']:g}")
    for who, f in faults.items():
        print(f"  {who}: losses on time {f['time']}, illegal moves {f['illegal']}, crashes {f['crash']}, "
              f"other errors {f['error']}")
    sys.exit(1 if any(faults["port"].values()) else 0)


if __name__ == "__main__":
    main()
