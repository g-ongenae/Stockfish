#!/usr/bin/env python3
"""Runs the same commands through the port and a reference engine and
compares everything they print, for the commands and options outside a search
(INVENTORY.md): `eval` over positions of the differential samples (standard
and Chess960, in check and not), `d`, `export_net` (the bytes of the files
written), `compiler` (its shape), `help` and its forms, `speedtest` (its
structure and set-up), `Debug Log File` (the log written), `NumaPolicy` (its
messages), the network messages, errors, and the command line. Only the
Python standard library.

    compare_commands.py [--only NAME]... [--list] [--record FILE | --expected FILE]

EXE is the port, REF the reference (default: the shared folder's
oracle/stockfish); RUN_PREFIX wraps the port (an emulator). Each case runs
each engine once, in a fresh empty working directory, and compares standard
output (and standard error and the exit code where the case says so) after
removing what is meant to differ: the start-up line and `id name` (each engine
names itself), `nps` and `time`, the totals of time of `bench`, and the
status of the network replicas (C++ shares its network between processes
and prints "Shared memory.", the port does not and prints what C++ prints
without shared memory). Cases marked native depend on the processors of the
machine (NumaPolicy).

--record FILE writes, for each case that does not depend on the machine, a
digest of what the reference printed; --expected FILE compares the port with
such a file, for the targets that cannot run the reference (CI under
emulation). In those two modes `Available processors` and the thread binding
are normalized too.
"""

import argparse
import gzip
import hashlib
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile

S = os.environ.get("STOCKFISH_SHARED", os.path.expanduser("~/.local/share/stockfish-fork"))
HERE = os.path.dirname(os.path.abspath(__file__))
SAMPLE = os.path.join(HERE, "..", "..", "tests", "differential", "sample")
NET = "nn-18cd32e11a9c.nnue"


def sample_fens(name):
    with gzip.open(os.path.join(SAMPLE, f"{name}.fen.gz"), "rt") as f:
        fens = [l.strip() for l in f if l.strip()]
    with gzip.open(os.path.join(SAMPLE, f"{name}.jsonl.gz"), "rt") as f:
        checkers = [int(json.loads(l)["checkers"], 16) != 0 for l in f if l.startswith("{")]
    return fens, checkers


def eval_cases():
    """eval over positions of the samples: every 4th standard and every 2nd
    Chess960 position, and every position in check."""
    cases = []
    for name, every, chess960 in (("standard", 4, False), ("chess960", 2, True)):
        fens, checkers = sample_fens(name)
        chosen = [f for i, (f, c) in enumerate(zip(fens, checkers)) if i % every == 0 or c]
        in_check = sum(c for i, c in enumerate(checkers) if i % every == 0 or c)
        for k in range(0, len(chosen), 250):
            cmds = [f"setoption name UCI_Chess960 value {'true' if chess960 else 'false'}"]
            for fen in chosen[k:k + 250]:
                cmds += [f"position fen {fen}", "eval"]
            cases.append({"name": f"eval-{name}-{k // 250 + 1}", "stdin": cmds,
                          "count": len(chosen[k:k + 250])})
        print(f"eval: {len(chosen)} {name} positions, {in_check} in check", file=sys.stderr)
    return cases


def cases():
    c = []
    add = lambda name, stdin=None, args=None, **kw: c.append({"name": name, "stdin": stdin, "args": args, **kw})
    c += eval_cases()
    add("eval-moves", ["position startpos moves e2e4 e7e5 g1f3 b8c6 f1b5", "eval",
                       "position fen 8/8/8/8/8/8/8/K1k5 w - - 99 120", "eval",
                       "position fen 4k3/8/8/8/8/8/4q3/4K3 w - - 0 1", "eval",
                       "position fen 7k/8/8/8/8/8/8/K6Q b - - 0 1 moves h8g8", "eval"])
    add("eval-cli", args=["eval"])
    add("bench-eval", args=["bench", "16", "1", "1", "default", "eval"], stderr=True)
    add("d", ["d", "position startpos moves e2e4 c7c5 g1f3", "d", "flip", "d",
              "position fen 4k3/8/8/8/8/8/4q3/4K3 w - - 0 1", "d",
              "position fen 3k4/8/8/8/8/8/8/R2K3R w KQ - 0 1", "d",
              "setoption name UCI_Chess960 value true",
              "position fen bbqnnrkr/pppppppp/8/8/8/8/PPPPPPPP/BBQNNRKR w HFhf - 0 1 moves g2g3", "d",
              "position fen 4k3/8/8/2b5/8/8/8/R3K2R w KQ - 0 1", "d"])
    add("d-cli", args=["d"])
    for h in ("help", "--help", "license", "--license"):
        add(f"{h}-cli", args=[h])
    add("help", ["help", "license", "--help", "--license", "helpme", "Help"])
    add("unknown", ["foo", "  foo  bar ", "# comment", "", "   ", "searchtrace_not", "export_netx", "evalx"])
    # (perft does not wait for a search in C++: never one after a go)
    add("go-messages", ["go perft 2", "go depth 1", "position startpos moves e2e4", "go nodes 1000"])
    add("perft-no-network", ["setoption name EvalFile value nosuch.nnue", "go perft 1"], rc=True)
    add("go-no-network", ["setoption name EvalFile value nosuch.nnue", "go depth 1"], rc=True)
    add("eval-no-network", ["setoption name EvalFile value nosuch.nnue", "eval"], rc=True)
    add("dump-no-network", ["setoption name EvalFile value nosuch.nnue", "dump"], rc=True)
    add("evalfile-back", ["setoption name EvalFile value nosuch.nnue", f"setoption name EvalFile value {NET}",
                          "eval", "setoption name EvalFile value <empty>", "go perft 1"])
    add("options", ["setoption name threads value 2", "setoption name Threads", "setoption",
                    "setoption name", "setoption name Nonexistent value 3", "setoption name Hash value 0",
                    "setoption name Skill Level value abc", "setoption name UCI_Chess960 value maybe",
                    "setoption name Clear Hash", "setoption name Threads value 1", "uci", "isready"])
    add("errors", ["go depth x"], rc=True)
    add("errors-fen", ["position fen 8/8/8/8/8/8/8/8 w - - 0 1"], rc=True)
    add("errors-move", ["position startpos moves e2e5"], rc=True)
    # Debug Log File: the logs written are compared too
    add("log", ["setoption name Debug Log File value log1.txt", "uci", "isready", "position startpos moves e2e4",
                "go depth 3", "d", "eval", "foo", "setoption name Nonexistent value 1",
                "setoption name Debug Log File value <empty>", "d", "isready",
                "setoption name Debug Log File value log2.txt", "go perft 2", "export_net", "isready",
                "setoption name Debug Log File value log1.txt", "isready"],
        files=["log1.txt", "log2.txt"], logs=True)
    add("log-cli", args=["setoption", "name", "Debug", "Log", "File", "value", "log3.txt"], files=["log3.txt"],
        logs=True)
    add("log-bad", ["setoption name Debug Log File value /nonexistent/dir/x.log", "isready"], stderr=True, rc=True)
    add("log-bad-quote", ['setoption name Debug Log File value /nonexistent/a"b\\c.log'], stderr=True, rc=True)
    # export_net: the files written, by their bytes
    add("export-named", ["export_net out.nnue", "isready"], files=["out.nnue"])
    add("export-default", ["export_net", "isready"], files=[NET])
    add("export-cli", args=["export_net", "cli.nnue"], files=["cli.nnue"])
    add("export-other", ["setoption name EvalFile value other.nnue", "export_net", "export_net o2.nnue",
                         f"setoption name EvalFile value {NET}", "export_net o3.nnue", "go perft 1"],
        files=["o2.nnue", "o3.nnue"], link_other=True)
    add("export-failed-load", ["setoption name EvalFile value nosuch.nnue", "export_net f.nnue"], files=["f.nnue"])
    add("export-unwritable", ["export_net /nonexistent/dir/x.nnue", "isready"])
    add("compiler", ["compiler"], shape="compiler")
    add("compiler-cli", args=["compiler"], shape="compiler")
    # speedtest: the structure, the set-up and what is deterministic
    add("speedtest", ["speedtest 1 1 2"], shape="speedtest")
    add("speedtest-clamped", ["speedtest 0 0 0"], shape="speedtest")
    add("speedtest-2", ["speedtest 2 8 2"], shape="speedtest")
    add("speedtest-partial", ["speedtest 1 1 2x"], shape="speedtest")
    # NumaPolicy: its messages, which depend on the processors
    values = ["auto", "system", "hardware", "none", "NONE", "0", "1", "0-1", "0:1", "1:0", "5", "0,0", "0:0",
              "garbage", "0-", "-1", "99999999999999999999", "0-1048577", "0-1048575", ",", ":", "0 1", "3-1",
              "0;1", "<empty>", "zzz", "99999999999999999999999"]
    for v in values:
        add(f"numa-{v}", [f"setoption name NumaPolicy value {v}", "setoption name Threads value 3", "go perft 1",
                          "setoption name Threads value 1", "go perft 1", "export_net e.nnue", "go perft 1"],
            native=True, files=["e.nnue"], rc=True)
    add("numa-sequence", ["setoption name NumaPolicy value 0:1", "setoption name NumaPolicy value zzz",
                          "setoption name Threads value 2", "go perft 1", "setoption name NumaPolicy value none",
                          "go perft 1", "setoption name NumaPolicy value system", "setoption name Threads value 1",
                          "eval"], native=True)
    add("numa-spaces", ["setoption name NumaPolicy value 0, ,1", "setoption name NumaPolicy value  ,1",
                        "setoption name NumaPolicy value 1 x,0", "setoption name NumaPolicy value +1",
                        "setoption name NumaPolicy value +", "go perft 1"], native=True, rc=True)
    add("numa-default", ["go perft 1", "setoption name Threads value 4", "go perft 1"], native=True)
    return c


def normalize(text, strict):
    out = []
    for i, line in enumerate(text.split("\n")):
        if i == 0 and line.startswith("Stockfish "):
            continue  # the start-up line
        line = re.sub(r"^id name .*", "id name <name>", line)
        line = re.sub(r" (nps|time) \d+", "", line)
        line = re.sub(r"^((?:<< )?info string Network replica \d+: )(Shared memory\.|Local memory\..*)$", r"\1<allocated>", line)
        line = re.sub(r"^(Total time \(ms\)|Nodes/second)(\s*:).*", r"\1\2", line)
        if not strict:
            line = re.sub(r"Available processors: \S+", "Available processors: <cpus>", line)
            line = re.sub(r"binding: [\d/:]+", "binding: <binding>", line)
        out.append(line)
    return "\n".join(out)


def check_compiler(stdout):
    """The four lines of C++'s compiler_info(), in its layout."""
    lines = [l for l in normalize(stdout, True).split("\n")]
    labels = ["Compiled by                : ", "Compilation architecture   : ", "Compilation settings       : ",
              "Compiler __VERSION__ macro : "]
    body = [l for l in lines if l]
    return len(body) == 4 and all(b.startswith(l) and len(b) > len(l) for b, l in zip(body, labels)) \
        and lines[0] == "" and lines[-1] == "" and lines[-2] == "" and re.match(r".* on \S", body[0])


SPEEDTEST_LABELS = ["Version", "Compiled by", "Compilation architecture", "Compilation settings",
                    "Compiler __VERSION__ macro", "Large pages", "User invocation", "Filled invocation",
                    "Available processors", "Thread count", "Thread binding", "TT size [MiB]",
                    "Hash max, avg [per mille]", "    single search", "    single game", "Total nodes searched",
                    "Total search time [s]", "Nodes/second"]
# the values that do not depend on the machine or on time
SPEEDTEST_EXACT = {"User invocation", "Filled invocation", "Thread count", "Thread binding", "TT size [MiB]",
                   "Hash max, avg [per mille]"}


def speedtest_shape(stderr):
    """The progress lines (each \\r-separated piece), and the report as
    (label, value or a pattern for what varies)."""
    progress, report, seen_sep = [], [], False
    for line in stderr.split("\n"):
        if line.startswith("==="):
            seen_sep = True
            report.append(("===", line))
            continue
        if not seen_sep:
            progress += [p for p in line.split("\r") if p] + ["<newline>"]
            continue
        if " : " in line or line.endswith(": "):
            label, _, value = line.partition(" : ") if " : " in line else (line[:-2], None, "")
            label = label.rstrip()
            if label in SPEEDTEST_EXACT:
                report.append((label, value))
            elif label in ("Total nodes searched", "Nodes/second", "    single search", "    single game"):
                report.append((label, re.sub(r"\d+", "N", value)))
            elif label == "Total search time [s]":
                report.append((label, "float" if re.fullmatch(r"\d+(\.\d+)?", value) else value))
            else:
                report.append((label, "<varies>"))
        elif line:
            report.append(("?", line))
    return progress, report


# a command after which both engines have finished the search a go started
# (setoption waits for it; nothing else does in C++, not even quit)
WAIT = "setoption name Ponder value false"


def run(exe, case, workdir, prefix=""):
    cmd = shlex.split(prefix) + [exe] + (case.get("args") or [])
    if case.get("stdin") is not None:
        lines = []
        for l in case["stdin"]:
            lines.append(l)
            if l.startswith("go") and "perft" not in l:
                lines.append(WAIT)
        case = dict(case, stdin=lines)
    stdin = "".join(l + "\n" for l in case["stdin"]) + "quit\n" if case.get("stdin") is not None else ""
    if case.get("link_other"):
        target = os.path.join(os.path.dirname(os.path.abspath(exe)), NET)
        if not os.path.exists(target):
            target = os.path.join(S, "oracle", NET)
        os.symlink(os.path.realpath(target), os.path.join(workdir, "other.nnue"))
    p = subprocess.run(cmd, input=stdin.encode(), capture_output=True, cwd=workdir, timeout=3600)
    # (bytes that are not UTF-8 stay distinct: surrogateescape)
    res = {"stdout": p.stdout.decode(errors="surrogateescape"), "stderr": p.stderr.decode(errors="surrogateescape"),
           "rc": p.returncode, "files": {}}
    for f in case.get("files", []):
        path = os.path.join(workdir, f)
        if not os.path.exists(path):
            res["files"][f] = None
        elif case.get("logs"):
            with open(path, "rb") as fh:
                res["files"][f] = normalize(fh.read().decode(errors="surrogateescape"), True)
        else:
            h = hashlib.sha256()
            with open(path, "rb") as fh:
                for chunk in iter(lambda: fh.read(1 << 20), b""):
                    h.update(chunk)
            res["files"][f] = h.hexdigest()
            os.remove(path)
    return res


def compared(case, res, strict):
    """What is compared of a run, as one string."""
    shape = case.get("shape")
    parts = []
    if shape == "compiler":
        # a shape that is not C++'s fails whatever the other engine prints
        if not check_compiler(res["stdout"]):
            parts.append("compiler shape WRONG:\n" + res["stdout"] + "\n" + str(id(res)))
        else:
            parts.append("compiler shape ok")
    elif shape and shape.startswith("speedtest"):
        progress, report = speedtest_shape(res["stderr"])
        parts.append(normalize(res["stdout"], strict))
        parts.append(json.dumps(progress))
        parts.append(json.dumps(report if shape == "speedtest" else []))
    else:
        parts.append(normalize(res["stdout"], strict))
        if case.get("stderr"):
            parts.append(normalize(res["stderr"], strict))
    if case.get("rc") or shape:
        parts.append(f"exit code {res['rc']}")
    for f, v in sorted(res["files"].items()):
        if v is not None and case.get("logs") and not strict:
            v = normalize(v, False)
        parts.append(f"file {f}: {v}")
    return "\n=====\n".join(parts)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--only", action="append", help="cases whose name starts with this")
    ap.add_argument("--list", action="store_true")
    ap.add_argument("--record")
    ap.add_argument("--expected")
    ap.add_argument("--native", action="store_true", help="only the cases that depend on the machine")
    args = ap.parse_args()

    all_cases = cases()
    if args.only:
        all_cases = [c for c in all_cases if any(c["name"].startswith(o) for o in args.only)]
    if args.native:
        all_cases = [c for c in all_cases if c.get("native")]
    if args.list:
        for c in all_cases:
            print(c["name"])
        return
    portable = bool(args.record or args.expected)
    if portable:
        all_cases = [c for c in all_cases if not c.get("native")]

    exe = os.environ.get("EXE")
    ref = os.environ.get("REF", os.path.join(S, "oracle", "stockfish"))
    if not args.record and not exe:
        sys.exit("EXE must name the port")
    expected = {}
    if args.expected:
        with open(args.expected) as f:
            for line in f:
                r = json.loads(line)
                expected[r["case"]] = r["sha256"]
    record = open(args.record, "w") if args.record else None

    differ = 0
    for case in all_cases:
        base = tempfile.mkdtemp(prefix="cmpcmd-", dir=os.environ.get("CMP_TMP"))
        try:
            got = want = None
            if not args.record:
                d = os.path.join(base, "port")
                os.mkdir(d)
                got = compared(case, run(exe, case, d, os.environ.get("RUN_PREFIX", "")), not portable)
            if not args.expected:
                d = os.path.join(base, "ref")
                os.mkdir(d)
                want = compared(case, run(ref, case, d), not portable)
        finally:
            shutil.rmtree(base, ignore_errors=True)
        if record:
            record.write(json.dumps({"case": case["name"], "sha256": hashlib.sha256(want.encode()).hexdigest()}) + "\n")
            print(f"{case['name']}: recorded", flush=True)
            continue
        if args.expected:
            ok = expected.get(case["name"]) == hashlib.sha256(got.encode()).hexdigest()
            if case["name"] not in expected:
                print(f"{case['name']}: not in {args.expected}")
        else:
            ok = got == want
        print(f"{case['name']}: {'same' if ok else 'DIFFERS'}", flush=True)
        if not ok:
            differ += 1
            if want is not None:
                gl, wl = got.split("\n"), want.split("\n")
                for i, (g, w) in enumerate(zip(gl + [None] * len(wl), wl + [None] * len(gl))):
                    if g != w:
                        print(f"  line {i}: expected {w!r}\n          got      {g!r}")
                        break
            else:
                print("  the port printed:\n" + "\n".join("    " + l for l in got.split("\n")[:40]))
    if record:
        record.close()
        return
    print(f"{len(all_cases) - differ} of {len(all_cases)} cases the same")
    sys.exit(1 if differ else 0)


if __name__ == "__main__":
    main()
