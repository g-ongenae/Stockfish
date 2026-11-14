#!/usr/bin/env python3
"""Plays the same sessions to the port and to a reference engine through UCI,
with options of one's choice, and compares everything they print for each
search but the time and the speed: the `info` lines (with `currmove` lines and
`wdl` if asked), the best move and the ponder move. For what the search trace
fixes or leaves out (other hash sizes, UCI_ShowWDL, MultiPV, searchmoves,
go mate, very long searches, clocks), against the reference binary directly.

    compare_uci.py SESSIONS [--chess960] [--option NAME=VALUE]... [--every N]
                   [--record FILE | --expected FILE]

SESSIONS is a file of sessions in the format of the search trace (one per
line: `position ...` and `go ...` commands separated by `;`), plain or .gz.
Here `go` may take any argument (searchmoves, mate, a clock, ...), and a
session may also hold `ucinewgame` and `setoption ...`, sent to both engines
as they come. A `go ponder ... depth N` must be followed by `ponderhit` or
`stop`, a `go infinite ... depth N` by `stop`: that command is sent once the engine has printed the exact `info depth N`
line of its last iteration, so that the search has reached its depth limit and waits for it (the
result does not depend on when the command arrives then). Each session starts
with `ucinewgame`. EXE is the port, REF the reference (default: the shared
folder's oracle/stockfish). RUN_PREFIX wraps the port (an emulator).

A search on a clock is compared only under `nodestime` (set in the session):
time is then counted in nodes and the search is deterministic.

--record FILE writes what the reference prints for each search (the lines
compared) to FILE (.gz to compress it); --expected FILE compares the port with
such a file instead of running the reference, for the targets that cannot run
it (CI under emulation).
"""

import argparse
import gzip
import json
import os
import re
import shlex
import subprocess
import sys

S = os.environ.get("STOCKFISH_SHARED", os.path.expanduser("~/.local/share/stockfish-fork"))


class Engine:
    def __init__(self, exe, options, chess960, prefix=""):
        self.exe = exe
        cmd = shlex.split(prefix) + [exe]
        self.p = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, bufsize=0)
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

    def search(self, command, then=None):
        """Runs `go ...` and returns the lines compared. With `then` (a ponder
        search with a depth limit), sends `then` once the `info depth N` line
        of that limit has been printed."""
        self.send("isready")
        self.read_until("readyok")
        self.send(command)
        lines = []
        if then is not None:
            depth = re.search(r"\bdepth (\d+)", command)
            if not depth or not re.search(r"\b(ponder|infinite)\b", command):
                sys.exit(f"'{then}' after '{command}': only after a go ponder or infinite with a depth limit")
            prefix = f"info depth {depth.group(1)} "
            for line in self.p.stdout:
                line = line.decode().strip()
                lines.append(line)
                # the exact line of the last iteration (not a currmove line,
                # nor a bound printed before a re-search)
                done = line.startswith(prefix) and " pv " in line and "bound" not in line
                if done or line.startswith("bestmove"):
                    break
            if lines and lines[-1].startswith("bestmove"):
                sys.exit(f"{self.exe}: bestmove before '{then}' in a ponder search: {command}")
            self.send(then)
        if not (lines and lines[-1].startswith("bestmove")):
            lines += self.read_until("bestmove")
        out = []
        for line in lines:
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
    ap.add_argument("--verbose", action="store_true", help="a line per search")
    ap.add_argument("--record", help="write the reference's output of each search to this file")
    ap.add_argument("--expected", help="compare with this file (from --record) instead of the reference")
    args = ap.parse_args()

    exe = os.environ.get("EXE")
    ref = os.environ.get("REF", os.path.join(S, "oracle", "stockfish"))
    options = [tuple(o.split("=", 1)) for o in args.option]
    if args.record and args.expected:
        sys.exit("--record and --expected exclude each other")
    if args.record:
        # the reference alone
        port = None
        reference = Engine(ref, options, args.chess960)
        record = (gzip.open if args.record.endswith(".gz") else open)(args.record, "wt")
    else:
        if not exe:
            sys.exit("EXE must name the port")
        port = Engine(exe, options, args.chess960, os.environ.get("RUN_PREFIX", ""))
        reference = None if args.expected else Engine(ref, options, args.chess960)
        record = None
    expected_file = None
    if args.expected:
        expected_file = (gzip.open if args.expected.endswith(".gz") else open)(args.expected, "rt")
    engines = [e for e in (port, reference) if e]

    searches = differ = lines = 0
    for n, session in enumerate(sessions(args.sessions), 1):
        if (n - 1) % args.every:
            continue
        for e in engines:
            e.send("ucinewgame")
        commands = [c.strip() for c in session.split(";")]
        i = 0
        while i < len(commands):
            command = commands[i]
            i += 1
            if command.startswith(("position", "ucinewgame", "setoption")):
                for e in engines:
                    e.send(command)
                continue
            if command in ("ponderhit", "stop"):
                sys.exit(f"session {n}: '{command}' must follow a go ponder or infinite")
            then = None
            if i < len(commands) and commands[i] in ("ponderhit", "stop"):
                then = commands[i]
                i += 1
            expected = reference.search(command, then) if reference else None
            if record:
                record.write(json.dumps({"session": n, "go": command, "lines": expected}) + "\n")
                searches += 1
                lines += len(expected)
                continue
            got = port.search(command, then)
            if expected_file:
                rec = json.loads(expected_file.readline() or "null")
                if not rec or rec["session"] != n or rec["go"] != command:
                    sys.exit(f"{args.expected} does not follow {args.sessions} at session {n}: {command}")
                expected = rec["lines"]
            searches += 1
            if args.verbose:
                print(f"session {n}: {command}: {'same' if got == expected else 'DIFFERS'}, {len(expected)} lines", flush=True)
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
    if record:
        record.close()
        print(f"RECORDED {args.sessions} ({what}) to {args.record}: {searches} searches, {lines} lines")
        return
    if expected_file and expected_file.readline():
        sys.exit(f"{args.expected} holds more searches than {args.sessions}")
    status = "FAILED" if differ else "OK"
    print(f"{status} {args.sessions} ({what}): {searches - differ} of {searches} searches match, {lines} lines")
    if differ or searches == 0:
        sys.exit(1)


if __name__ == "__main__":
    main()
