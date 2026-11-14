#!/usr/bin/env python3
"""Measurements of the thread pool (README.md, "Threads"), for any UCI
engine with a 'bench' command (the port or the C++ engine). Only the Python
standard library.

    threads_measure.py bench ENGINE --threads 1,2,4 --runs 5 --depth 12
        'bench 16 T D default depth' RUNS times for each T: nodes per second,
        total nodes and time (their spread over the runs), and how often the
        best move of each position agrees with the first run of the first T
        (with T = 1, the single-threaded move)
    threads_measure.py memory ENGINE --threads 1,2,4,8
        peak resident memory of 'setoption Threads T', a search of depth 12 at
        Hash 16, and quit
    threads_measure.py start ENGINE --threads 1,2,4,8 --count 300
        from sending 'go depth 1' to reading 'bestmove', median and 90th
        percentile, after a warm-up
"""

import argparse
import os
import re
import statistics
import subprocess
import sys
import time


def bench(args):
    first_moves = None
    for t in args.threads:
        nps, nodes, ms, agree, total = [], [], [], 0, 0
        for r in range(args.runs):
            p = subprocess.run(args.engine + ["bench", "16", str(t), str(args.depth), "default", "depth"],
                               capture_output=True, text=True)
            moves = [l.split()[1] for l in p.stdout.splitlines() if l.startswith("bestmove")]
            nps.append(int(re.search(r"Nodes/second\s*:\s*(\d+)", p.stderr).group(1)))
            nodes.append(int(re.search(r"Nodes searched\s*:\s*(\d+)", p.stderr).group(1)))
            ms.append(int(re.search(r"Total time \(ms\)\s*:\s*(\d+)", p.stderr).group(1)))
            if first_moves is None:
                first_moves = moves
            else:
                agree += sum(a == b for a, b in zip(moves, first_moves))
                total += len(moves)

        def spread(v):
            return f"median {statistics.median(v):,.0f}, min {min(v):,} max {max(v):,} (+/-{(max(v) - min(v)) / 2 / statistics.median(v) * 100:.1f}%)"

        print(f"Threads {t}: nps {spread(nps)}; nodes {spread(nodes)}; time ms {spread(ms)}"
              + (f"; best move as the reference run: {agree}/{total} ({agree / total * 100:.1f}%)" if total else ""),
              flush=True)


def memory(args):
    for t in args.threads:
        p = subprocess.Popen(args.engine, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
        p.stdin.write(f"setoption name Threads value {t}\nsetoption name Hash value 16\nisready\n"
                      "position startpos\ngo depth 12\n")
        p.stdin.flush()
        for line in p.stdout:
            if line.startswith("bestmove"):
                break
        p.stdin.write("quit\n")
        p.stdin.flush()
        _, status, usage = os.wait4(p.pid, 0)
        print(f"Threads {t}: peak resident {usage.ru_maxrss / 1024:.0f} MiB", flush=True)


def start(args):
    for t in args.threads:
        p = subprocess.Popen(args.engine, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)

        def until(prefix):
            for line in p.stdout:
                if line.startswith(prefix):
                    return
            sys.exit("the engine exited")

        p.stdin.write(f"setoption name Threads value {t}\nisready\n")
        p.stdin.flush()
        until("readyok")
        times = []
        for i in range(args.count + 20):
            p.stdin.write("position startpos\n")
            t0 = time.perf_counter()
            p.stdin.write("go depth 1\n")
            p.stdin.flush()
            until("bestmove")
            if i >= 20:
                times.append((time.perf_counter() - t0) * 1000)
        p.stdin.write("quit\n")
        p.stdin.flush()
        p.wait()
        times.sort()
        print(f"Threads {t}: go depth 1 to bestmove, median {statistics.median(times):.3f} ms, "
              f"90th percentile {times[int(len(times) * 0.9)]:.3f} ms, max {times[-1]:.3f} ms", flush=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("what", choices=["bench", "memory", "start"])
    ap.add_argument("engine", nargs="+")
    ap.add_argument("--threads", default="1,2,4")
    ap.add_argument("--runs", type=int, default=5)
    ap.add_argument("--depth", type=int, default=12)
    ap.add_argument("--count", type=int, default=300)
    args = ap.parse_args()
    args.threads = [int(t) for t in args.threads.split(",")]
    {"bench": bench, "memory": memory, "start": start}[args.what](args)


if __name__ == "__main__":
    main()
