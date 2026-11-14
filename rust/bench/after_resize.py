#!/usr/bin/env python3
"""Nothing is left over from a larger pool: in one session of the engine,
searches with several threads (and Hash and MultiPV changed), then `Threads`
back to 1 and the options of the search trace, then sessions of the sample of
the trace played over UCI, which must give the recorded UCI output bit for bit
(README.md, "Threads"). Only the Python standard library.

    after_resize.py [--sessions N] [--pool T,...] ENGINE...

RUN_PREFIX, if set, is put before ENGINE (an emulator)."""

import argparse
import gzip
import json
import os
import re
import subprocess
import sys
import threading
import queue

HERE = os.path.dirname(os.path.abspath(__file__))
SAMPLE = os.path.join(HERE, "..", "..", "tests", "differential", "sample")

OPTIONS = (
    ("Threads", "1"),
    ("Hash", "2"),
    ("MultiPV", "1"),
    ("Skill Level", "20"),
    ("UCI_LimitStrength", "false"),
    ("UCI_ShowWDL", "false"),
    ("SyzygyPath", "<empty>"),
    ("nodestime", "0"),
    ("Ponder", "false"),
)


class Engine:
    def __init__(self, cmd, timeout):
        self.p = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)
        self.q = queue.Queue()
        self.timeout = timeout
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self):
        for line in self.p.stdout:
            self.q.put(line.strip())
        self.q.put(None)

    def send(self, text):
        self.p.stdin.write(text + "\n")
        self.p.stdin.flush()

    def until(self, prefix):
        lines = []
        while True:
            try:
                line = self.q.get(timeout=self.timeout)
            except queue.Empty:
                sys.exit(f"no '{prefix}' within {self.timeout} s (hang)")
            if line is None:
                sys.exit(f"the engine exited before '{prefix}'")
            lines.append(line)
            if line.startswith(prefix):
                return lines


def sessions(name, count):
    with gzip.open(os.path.join(SAMPLE, f"{name}.trace.gz"), "rt") as f:
        found = [line.strip() for line in f if line.strip() and not line.startswith("#")][:count]
    records = []
    with gzip.open(os.path.join(SAMPLE, f"{name}.trace.jsonl.gz"), "rt") as f:
        for line in f:
            r = json.loads(line)
            if r["session"] > count:
                break
            records.append(r)
    return found, records


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--sessions", type=int, default=10000, help="sessions of each set of the sample (all by default)")
    ap.add_argument("--pool", default="4,3", help="the thread counts searched with before")
    ap.add_argument("--timeout", type=float, default=300)
    ap.add_argument("engine", nargs="+")
    args = ap.parse_args()

    cmd = os.environ.get("RUN_PREFIX", "").split() + args.engine
    e = Engine(cmd, args.timeout)
    e.send("uci")
    e.until("uciok")

    # searches with a larger pool, which fill every table, the shared ones
    # included, and leave helpers' results behind
    for t in args.pool.split(","):
        e.send(f"setoption name Threads value {t}")
        e.send("setoption name Hash value 8")
        e.send("setoption name MultiPV value 2")
        e.send("position startpos moves e2e4 c7c5 g1f3")
        e.send("go depth 11")
        e.until("bestmove")
        e.send("position fen r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 10")
        e.send("go nodes 200000")
        e.until("bestmove")

    checked = 0
    for name, chess960 in (("standard", False), ("chess960", True)):
        for option, value in OPTIONS:
            e.send(f"setoption name {option} value {value}")
        e.send(f"setoption name UCI_Chess960 value {'true' if chess960 else 'false'}")
        found, records = sessions(name, args.sessions)
        expected = iter(records)
        for number, session in enumerate(found, 1):
            e.send("ucinewgame")
            for command in (c.strip() for c in session.split(";")):
                if command.startswith("position"):
                    e.send(command)
                    continue
                e.send("isready")
                e.until("readyok")
                e.send(command)
                info, bestmove, ponder = [], None, None
                for line in e.until("bestmove"):
                    words = line.split()
                    if words[:1] == ["info"] and "score" in words:
                        info.append(re.sub(r" (nps|time) \d+", "", line[len("info "):]))
                    elif words[:1] == ["bestmove"]:
                        bestmove = words[1]
                        ponder = words[3] if len(words) > 3 and words[2] == "ponder" else None
                record = next(expected)
                got = {"info": info, "bestmove": bestmove, "ponder": ponder}
                want = {k: record[k] for k in got}
                if got != want:
                    sys.exit(f"{name} session {number} ({session}): differs after the pool {args.pool}:\n"
                             f"  expected {json.dumps(want)}\n  got      {json.dumps(got)}")
                checked += 1
    e.send("quit")
    e.p.wait(timeout=args.timeout)
    print(f"OK: {checked} searches identical to the trace sample after searches with Threads {args.pool}")


if __name__ == "__main__":
    main()
