#!/usr/bin/env python3
"""The engines on a real clock (README.md, "Time management", gate 3): for a
spread of clocks, the first move of a game from a few positions, the time from
sending `go` to reading `bestmove`, against the optimum and maximum time that
TimeManagement::init() computes for that clock (a copy of its formulas, for
the first move of a game, without nodestime). Each engine is checked to
answer within the maximum plus MARGIN ms; the time used by the port and by the
reference are printed side by side. Standard library only.

    clock_behaviour.py [--margin MS] [--repeat N]

EXE is the port, REF the reference (default: the shared folder's
oracle/stockfish).
"""

import argparse
import math
import os
import statistics
import subprocess
import sys
import time

S = os.environ.get("STOCKFISH_SHARED", os.path.expanduser("~/.local/share/stockfish-fork"))

POSITIONS = [
    ("startpos", 0),
    ("fen r1bq1rk1/pp2bppp/2n1pn2/2pp4/3P4/2PBPN2/PP1N1PPP/R1BQ1RK1 w - - 0 9", 16),
    ("fen r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1", 0),
    ("fen 8/5k2/8/3R4/8/8/2K5/4r3 w - - 30 60", 118),
]

# wtime = btime, inc, movestogo
CLOCKS = [(50, 0, 0), (200, 0, 0), (1000, 0, 0), (1000, 100, 0), (5000, 0, 0), (10000, 100, 0),
          (30000, 0, 0), (60000, 1000, 0), (60000, 0, 20), (10000, 0, 5), (3000, 0, 1)]


def bounds(t, inc, mtg_in, ply, overhead=10):
    """init() for the first move of a game, without nodestime: (optimum, maximum)."""
    scaled = max(1, t)
    mtg = min(mtg_in, 50) if mtg_in else 50
    if scaled < 1000 and mtg_in == 0:
        mtg = int(scaled * 0.05)
    left = max(1, t + inc * (mtg - 1) - overhead * (2 + mtg))
    if mtg_in == 0:
        adjust = 0.3272 * math.log10(left) - 0.4141
        lg = math.log10(scaled / 1000.0)
        optc = min(0.0029869 + 0.00033554 * lg, 0.004905)
        maxc = max(3.3744 + 3.0608 * lg, 3.1441)
        opt = min(0.012112 + math.pow(ply + 3.22713, 0.46866) * optc, 0.19404 * t / left) * adjust
        mx = min(6.873, maxc + ply / 12.352)
    else:
        opt = min((0.88 + ply / 116.4) / mtg, 0.88 * t / left)
        mx = 1.3 + 0.11 * mtg
    if mtg_in != 1:
        opt *= 1 + 0.9 * min(0.0, 0.0)  # equal clocks: no time advantage
    optimum = int(max(1.0, opt * left))
    maximum = int(max(float(optimum), min(0.8097 * t - overhead, mx * optimum)))
    return optimum, maximum


class Engine:
    def __init__(self, exe):
        self.p = subprocess.Popen([exe], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)
        self.send("uci")
        self.until("uciok")

    def send(self, s):
        self.p.stdin.write(s + "\n")
        self.p.stdin.flush()

    def until(self, prefix):
        for line in self.p.stdout:
            if line.startswith(prefix):
                return line
        sys.exit("engine stopped")

    def move_time(self, position, go):
        self.send("ucinewgame")
        self.send(f"position {position}")
        self.send("isready")
        self.until("readyok")
        t = time.perf_counter()
        self.send(go)
        self.until("bestmove")
        return (time.perf_counter() - t) * 1000

    def close(self):
        self.send("quit")
        self.p.wait()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--margin", type=float, default=10.0,
                    help="ms allowed beyond the maximum (default: the default Move Overhead)")
    ap.add_argument("--repeat", type=int, default=2)
    args = ap.parse_args()
    exe = os.environ.get("EXE") or sys.exit("EXE must name the port")
    ref = os.environ.get("REF", os.path.join(S, "oracle", "stockfish"))
    engines = {"port": Engine(exe), "C++": Engine(ref)}
    for e in engines.values():
        e.send("isready")
        e.until("readyok")
    over = 0
    print(f"{'clock':>22} {'ply':>4} {'optimum':>8} {'maximum':>8} {'port ms':>16} {'C++ ms':>16}")
    ratios = []
    for t, inc, mtg in CLOCKS:
        for position, ply in POSITIONS:
            optimum, maximum = bounds(t, inc, mtg, ply)
            go = f"go wtime {t} btime {t}" + (f" winc {inc} binc {inc}" if inc else "") + (f" movestogo {mtg}" if mtg else "")
            used = {}
            for name, e in engines.items():
                v = [e.move_time(position, go) for _ in range(args.repeat)]
                used[name] = v
                if max(v) > maximum + args.margin:
                    over += 1
                    print(f"  OVER: {name} used {max(v):.1f} ms > maximum {maximum} + {args.margin}")
            clock = f"{t}+{inc}" + (f" mtg {mtg}" if mtg else "")
            fmt = lambda v: f"{min(v):7.1f}-{max(v):7.1f}"
            print(f"{clock:>22} {ply:>4} {optimum:>8} {maximum:>8} {fmt(used['port']):>16} {fmt(used['C++']):>16}")
            ratios.append(statistics.mean(used["port"]) / statistics.mean(used["C++"]))
    for e in engines.values():
        e.close()
    print(f"time used, port / C++: median {statistics.median(ratios):.2f}, "
          f"from {min(ratios):.2f} to {max(ratios):.2f}; moves beyond the maximum + {args.margin} ms: {over}")
    sys.exit(1 if over else 0)


if __name__ == "__main__":
    main()
