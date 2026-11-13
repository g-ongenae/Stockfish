#!/usr/bin/env python3
"""Plays the same sessions to the port and to a reference engine through UCI,
with options of one's choice, and compares everything they print for each
search but the time and the speed: the `info` lines (with `currmove` lines and
`wdl` if asked), the best move and the ponder move. For what the search trace
fixes or leaves out (other hash sizes, UCI_ShowWDL, MultiPV, searchmoves,
go mate, very long searches), against the reference binary directly.

    compare_uci.py SESSIONS [--chess960] [--option NAME=VALUE]... [--every N]

SESSIONS is a file of sessions in the format of the search trace (one per
line: `position ...` and `go ...` commands separated by `;`), plain or .gz;
`go` may take any argument here (searchmoves, mate, ...), but no clock. Each
session starts with `ucinewgame`. EXE is the port, REF the reference (default:
the shared folder's oracle/stockfish).
"""

import argparse
import gzip
import os
import re
import subprocess
import sys

S = os.environ.get("STOCKFISH_SHARED", os.path.expanduser("~/.local/share/stockfish-fork"))


class Engine:
    def __init__(self, exe, options, chess960):
        self.exe = exe
        self.p = subprocess.Popen([exe], stdin=subprocess.PIPE, stdout=subprocess.PIPE, bufsize=0)
        self.send("uci")
        self.read_until("uciok")
        for name, value in options:
            self.send(f"setoption name {name} value {value}")
        self.send(f"setoption name UCI_Chess960 value {'true' if chess960 else 'false'}")
        self.send("isready")
        self.read_until("readyok")

    def send(self, text):
        self.p.stdin.write((text + "\n").encode())

    def read_until(self, prefix):
        lines = []
        for line in self.p.stdout:
            line = line.decode().strip()
            lines.append(line)
            if line.startswith(prefix):
                return lines
        sys.exit(f"{self.exe} stopped before printing '{prefix}'")

    def search(self, command):
        self.send("isready")
        self.read_until("readyok")
        self.send(command)
        out = []
        for line in self.read_until("bestmove"):
            if line.startswith("info string"):
                continue
            if line.startswith("info") or line.startswith("bestmove"):
                out.append(re.sub(r" (nps|time) \d+", "", line))
        return out

    def close(self):
        self.send("quit")
        self.p.stdin.close()
        self.p.wait()


def sessions(path):
    opener = gzip.open if path.endswith(".gz") else open
    with opener(path, "rt") as f:
        for line in f:
            line = line.strip()
            if line and not line.startswith("#"):
                yield line


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("sessions")
    ap.add_argument("--chess960", action="store_true")
    ap.add_argument("--option", action="append", default=[], help="NAME=VALUE, set on both engines")
    ap.add_argument("--every", type=int, default=1, help="only every N-th session")
    ap.add_argument("--max-report", type=int, default=5)
    args = ap.parse_args()

    exe = os.environ.get("EXE")
    ref = os.environ.get("REF", os.path.join(S, "oracle", "stockfish"))
    if not exe:
        sys.exit("EXE must name the port")
    options = [tuple(o.split("=", 1)) for o in args.option]
    engines = [Engine(exe, options, args.chess960), Engine(ref, options, args.chess960)]

    searches = differ = lines = 0
    for n, session in enumerate(sessions(args.sessions), 1):
        if (n - 1) % args.every:
            continue
        for e in engines:
            e.send("ucinewgame")
        for command in (c.strip() for c in session.split(";")):
            if command.startswith("position"):
                for e in engines:
                    e.send(command)
                continue
            got, expected = (e.search(command) for e in engines)
            searches += 1
            lines += len(expected)
            if got != expected:
                differ += 1
                if differ <= args.max_report:
                    print(f"session {n}: {session}\n  {command}")
                    for i, (g, x) in enumerate(zip(got + [""] * len(expected), expected + [""] * len(got))):
                        if g != x:
                            print(f"  line {i}: expected {x!r}\n          got      {g!r}")
                            break
    for e in engines:
        e.close()
    what = ", ".join(f"{k}={v}" for k, v in options) or "default options"
    status = "FAILED" if differ else "OK"
    print(f"{status} {args.sessions} ({what}): {searches - differ} of {searches} searches match, {lines} lines")
    if differ or searches == 0:
        sys.exit(1)


if __name__ == "__main__":
    main()
