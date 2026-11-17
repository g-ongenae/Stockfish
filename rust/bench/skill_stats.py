#!/usr/bin/env python3
"""The skill level of the unpatched engines, statistically: for several
positions and levels, the move each engine chooses over many searches (each
after `ucinewgame`, so that only the random numbers of pick_best() differ
from one search to the next), and a chi-square test of homogeneity of the two
distributions per cell, then over all cells. Only the Python standard
library.

    skill_stats.py [--runs N] [--positions K]

EXE is the port, REF the reference (default: the shared folder's
oracle/stockfish). Each engine is one process; the generator is seeded once
per process from the clock, as in play.
"""

import argparse
import collections
import math
import os
import subprocess
import sys

S = os.environ.get("STOCKFISH_SHARED", os.path.expanduser("~/.local/share/stockfish-fork"))

POSITIONS = [
    "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1",
    "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 10",
    "4rrk1/pp1n3p/3q2pQ/2p1pb2/2PP4/2P3N1/P2B2PP/4RRK1 b - - 7 19",
    "r3r1k1/2p2ppp/p1p1bn2/8/1q2P3/2NPQN2/PPP3PP/R4RK1 b - - 2 15",
    "r1bq1rk1/ppp1nppp/4n3/3p3Q/3P4/1BP1B3/PP1N2PP/R4RK1 w - - 1 16",
    "6k1/6p1/6Pp/ppp5/3pn2P/1P3K2/1PP2P2/3N4 b - - 0 1",
    "8/pp2r1k1/2p1p3/3pP2p/1P1P1P1P/P5KR/8/8 w - - 0 1",
    "r2r1n2/pp2bk2/2p1p2p/3q4/3PN1QP/2P3R1/P4PP1/5RK1 w - - 0 1",
]

# (name, options, depth): the depth is the one at which the move is picked
LEVELS = [
    ("Skill Level 0", [("Skill Level", "0")], 1),
    ("Skill Level 4", [("Skill Level", "4")], 5),
    ("Skill Level 8", [("Skill Level", "8")], 9),
    ("UCI_Elo 1600", [("UCI_LimitStrength", "true"), ("UCI_Elo", "1600")], 3),
]


def choices(exe, options, fen, depth, runs):
    p = subprocess.Popen([exe], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)
    for name, value in options:
        p.stdin.write(f"setoption name {name} value {value}\n")
    counts = collections.Counter()
    for _ in range(runs):
        p.stdin.write(f"ucinewgame\nposition fen {fen}\ngo depth {depth}\n")
        p.stdin.flush()
        for line in p.stdout:
            if line.startswith("bestmove"):
                counts[line.split()[1]] += 1
                break
    p.stdin.write("quit\n")
    p.stdin.close()
    p.wait()
    return counts


def chi2_sf(x, k):
    """P(X >= x) for a chi-square of k degrees of freedom: the regularized
    upper incomplete gamma Q(k/2, x/2)."""
    if k <= 0:
        return 1.0
    a, x = k / 2.0, x / 2.0
    if x <= 0:
        return 1.0
    if x < a + 1:  # series for P, then 1 - P
        term = total = 1.0 / a
        n = a
        while abs(term) > total * 1e-15:
            n += 1
            term *= x / n
            total += term
        return max(0.0, 1.0 - total * math.exp(-x + a * math.log(x) - math.lgamma(a)))
    # continued fraction for Q (Lentz)
    b, c, d = x + 1 - a, 1e300, 1.0 / (x + 1 - a)
    h = d
    i = 1
    while True:
        an = -i * (i - a)
        b += 2
        d = an * d + b
        d = 1e-300 if abs(d) < 1e-300 else d
        c = b + an / c
        c = 1e-300 if abs(c) < 1e-300 else c
        d = 1.0 / d
        delta = d * c
        h *= delta
        i += 1
        if abs(delta - 1) < 1e-15:
            break
    return math.exp(-x + a * math.log(x) - math.lgamma(a)) * h


def homogeneity(a, b):
    """Chi-square of a 2 x m table, the moves with an expected count below 5
    pooled; returns (statistic, degrees of freedom)."""
    na, nb = sum(a.values()), sum(b.values())
    moves = sorted(set(a) | set(b), key=lambda m: -(a[m] + b[m]))
    cols, pool = [], [0, 0]
    for m in moves:
        tot = a[m] + b[m]
        if min(na, nb) * tot / (na + nb) >= 5:
            cols.append((a[m], b[m]))
        else:
            pool[0] += a[m]
            pool[1] += b[m]
    if sum(pool):
        cols.append(tuple(pool))
    if len(cols) < 2:
        return 0.0, 0
    stat = 0.0
    for ca, cb in cols:
        tot = ca + cb
        for obs, n in ((ca, na), (cb, nb)):
            exp = n * tot / (na + nb)
            stat += (obs - exp) ** 2 / exp
    return stat, len(cols) - 1


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--runs", type=int, default=400)
    ap.add_argument("--positions", type=int, default=len(POSITIONS))
    args = ap.parse_args()
    exe = os.environ.get("EXE") or sys.exit("EXE must name the port")
    ref = os.environ.get("REF", os.path.join(S, "oracle", "stockfish"))

    total_stat, total_dof = 0.0, 0
    for name, options, depth in LEVELS:
        for i, fen in enumerate(POSITIONS[:args.positions]):
            a = choices(exe, options, fen, depth, args.runs)
            b = choices(ref, options, fen, depth, args.runs)
            stat, dof = homogeneity(a, b)
            total_stat += stat
            total_dof += dof
            top = ", ".join(f"{m} {a[m]}/{b[m]}" for m, _ in (a + b).most_common(4))
            print(f"{name}, position {i + 1}: {len(a + b)} moves; port/C++ {top}; "
                  f"chi2 {stat:.1f}, {dof} dof, p {chi2_sf(stat, dof):.3f}", flush=True)
    print(f"all cells: chi2 {total_stat:.1f}, {total_dof} dof, p {chi2_sf(total_stat, total_dof):.3f}")


if __name__ == "__main__":
    main()
