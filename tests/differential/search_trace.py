#!/usr/bin/env python3
"""Golden files for the search trace: build the corpus, generate, verify, bisect.

The 'searchtrace' debug command of the tracing binary (src/stockfish-trace,
built with 'make trace-build') reads a file of sessions, one per line. A session
is a sequence of UCI commands separated by ';', 'position ...' and 'go depth N'
or 'go nodes N'. It starts from a cleared state, as after 'ucinewgame', and its
searches inherit the transposition table and the histories from each other. The
command prints one JSON record per search: the UCI output, the iterations, the
digests of the tables left behind, and the digest of the stream of events of the
search, checkpointed every 4096 events. The golden file of a corpus of sessions
holds the records printed by the reference engine.

    search_trace.py select POSITIONS LINES OUT     choose the sessions of a corpus
    search_trace.py generate CORPUS GOLDEN         write the golden file of a corpus
    search_trace.py verify CORPUS GOLDEN           compare an engine with a golden file
    search_trace.py verify --uci CORPUS GOLDEN     the same through plain UCI, UCI output only
    search_trace.py bisect CORPUS GOLDEN SESSION   find the first event that differs
    search_trace.py sample CORPUS GOLDEN N OUT_CORPUS OUT_GOLDEN
    search_trace.py stats GOLDEN [--corpus CORPUS] what the corpus covers

EXE selects the engine (default src/stockfish-trace, or src/stockfish with
--uci), REF the reference tracing binary that bisect compares with (default
src/stockfish-trace), RUN_PREFIX wraps both. Only the standard library is needed.
"""

import argparse
import contextlib
import hashlib
import itertools
import json
import os
import re
import shlex
import subprocess
import sys
import tempfile

from differential import ROOT, MISSING, open_read, open_write, open_output, corpus_lines

DEFAULT_TRACE_EXE = ROOT / "src" / "stockfish-trace"
DEFAULT_UCI_EXE = ROOT / "src" / "stockfish"

# Events between two checkpoints of the digest of the event stream
INTERVAL = 4096

# The options 'searchtrace' fixes, which the UCI mode sets the same way
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

# The members of a record that plain UCI output gives
UCI_FIELDS = ("info", "bestmove", "ponder")

# The members that number a record, checked by session_groups()
KEY_FIELDS = ("session", "search")


def engine(default):
    exe = os.environ.get("EXE", str(default))
    return shlex.split(os.environ.get("RUN_PREFIX", "")) + [exe]


def hash64(text, salt):
    return int.from_bytes(hashlib.blake2b((salt + "\0" + text).encode(), digest_size=8).digest(), "little")


def sessions(corpus):
    """Yields the sessions of a corpus, skipping blank lines and comments as the engine does."""
    with open_read(corpus) as src:
        for line in src:
            line = line.decode().strip()
            if line and not line.startswith("#"):
                yield line


def commands(session):
    return [c.strip() for c in session.split(";")]


def count_searches(session):
    return sum(c.split()[:1] == ["go"] for c in commands(session))


# ---------------------------------------------------------------------------
# Running the engine


@contextlib.contextmanager
def run_searchtrace(corpus, chess960, exe=None, window=None):
    """Runs 'searchtrace' over a corpus and yields an iterator on its output lines
    (bytes): the records, and with a window the events and their context."""
    exe = exe or engine(DEFAULT_TRACE_EXE)
    with tempfile.TemporaryDirectory(prefix="sf-trace-") as tmp:
        path = os.path.join(tmp, "sessions.txt")
        with open_read(corpus) as src, open(path, "wb") as dst:
            for line in src:
                dst.write(line)

        cmd = f"searchtrace {path}"
        if window:
            cmd += " events {} {} {}".format(*window)
        commands_ = cmd + "\nquit\n"
        if chess960:
            commands_ = "setoption name UCI_Chess960 value true\n" + commands_

        try:
            proc = subprocess.Popen(exe, stdin=subprocess.PIPE, stdout=subprocess.PIPE)
        except OSError as e:
            sys.exit(f"cannot run {exe[-1]}: {e}")
        proc.stdin.write(commands_.encode())
        proc.stdin.close()

        def lines():
            for line in proc.stdout:
                yield line
            proc.stdout.close()
            if proc.wait() != 0:
                sys.exit(f"{exe[-1]} exited with status {proc.returncode}")

        yield lines()


def records_of(lines):
    for line in lines:
        if line.startswith(b"{"):
            yield line


def run_uci(corpus, chess960):
    """Plays the sessions of a corpus to a UCI engine and yields, for each search,
    a record with the members the UCI output gives (as bytes, like the engine's)."""
    exe = engine(DEFAULT_UCI_EXE)
    try:
        proc = subprocess.Popen(exe, stdin=subprocess.PIPE, stdout=subprocess.PIPE, bufsize=0)
    except OSError as e:
        sys.exit(f"cannot run {exe[-1]}: {e}")

    def send(text):
        proc.stdin.write((text + "\n").encode())

    def read_until(prefix):
        lines = []
        for line in proc.stdout:
            line = line.decode().strip()
            lines.append(line)
            if line.startswith(prefix):
                return lines
        sys.exit(f"{exe[-1]} stopped before printing '{prefix}'")

    send("uci")
    read_until("uciok")
    for name, value in OPTIONS:
        send(f"setoption name {name} value {value}")
    send(f"setoption name UCI_Chess960 value {'true' if chess960 else 'false'}")
    send("isready")
    read_until("readyok")

    for number, session in enumerate(sessions(corpus), 1):
        send("ucinewgame")
        search = 0
        for command in commands(session):
            if command.startswith("position"):
                send(command)
                continue
            search += 1
            send("isready")
            read_until("readyok")
            send(command)
            info, bestmove, ponder = [], None, None
            for line in read_until("bestmove"):
                words = line.split()
                if words[:1] == ["info"] and "score" in words:
                    # The time and the speed are the only members that depend on the machine
                    text = re.sub(r" (nps|time) \d+", "", line[len("info ") :])
                    info.append(text)
                elif words[:1] == ["bestmove"]:
                    bestmove = words[1]
                    ponder = words[3] if len(words) > 3 and words[2] == "ponder" else None
            record = {"session": number, "search": search, "info": info, "bestmove": bestmove, "ponder": ponder}
            yield json.dumps(record, separators=(",", ":")).encode() + b"\n"
    send("quit")
    proc.stdin.close()
    proc.wait()


def session_groups(corpus, records, source):
    """Pairs each session of a corpus with its records, as (number, session, [(bytes, object)]).

    Ends the program if the records do not have the shape the corpus implies: for
    each session in turn, its searches numbered from 1, one per 'go' of the
    session, or a single error record.
    """
    records = iter(records)
    pending = None
    for number, session in enumerate(sessions(corpus), 1):
        expected = count_searches(session)
        group = []
        while True:
            line = pending if pending is not None else next(records, None)
            pending = None
            if line is None:
                break
            record = json.loads(line)
            if record.get("session") != number:
                pending = line
                break
            group.append((line, record))
        if not group:
            sys.exit(f"{source} gives no record for session {number} of {corpus}")
        if "error" in group[-1][1]:
            # A position that cannot be set up ends the session
            searches = [r.get("search") for _, r in group[:-1]]
        else:
            searches = [r.get("search") for _, r in group]
            if len(searches) != expected:
                sys.exit(f"{source} gives {len(searches)} records for the {expected} searches of session {number}")
        if searches != list(range(1, len(searches) + 1)):
            sys.exit(f"{source} numbers the searches of session {number} as {searches}")
        yield number, session, group
    rest = pending if pending is not None else next(records, None)
    if rest is not None:
        sys.exit(f"{source} gives more records than there are sessions in {corpus}")


# ---------------------------------------------------------------------------
# Building the corpus


def cmd_select(args):
    """Chooses the sessions of a corpus from a corpus of positions and one of lines.

    Every choice depends on a hash of the position or line, so that the selection
    does not depend on the order of the corpora, nor on anything else.
    """
    out = []

    # Single searches from a cleared state, on positions
    with open_read(args.positions) as src:
        fens = [line.decode().strip() for line in src if line.strip()]
    keep = sorted(range(len(fens)), key=lambda i: hash64(fens[i], "position"))[: args.positions_count]
    # And every position close to the 50-move rule, which search treats apart
    keep = set(keep) | {i for i, fen in enumerate(fens) if int(fen.split()[4]) >= args.rule50}
    for i in sorted(keep):
        h = hash64(fens[i], "limit")
        if h % 10 < 7:
            limit = f"go depth {1 + (h >> 8) % 10}"
        else:
            limit = f"go nodes {max(1, round(10 ** (((h >> 8) % 1000) / 1000 * 3.8)))}"
        out.append(f"position fen {fens[i]} ; {limit}")
    del fens

    # Games: successive searches along a line of play, inheriting the state
    lines = list(corpus_lines(args.lines))
    keep = sorted(range(len(lines)), key=lambda i: hash64(lines[i], "game"))[: args.games_count]
    for i in sorted(keep):
        line = lines[i]
        h = hash64(line, "session")
        words = line.split()
        start = words[: words.index("moves")] if "moves" in words else words
        moves = words[len(start) + 1 :]
        step = 1 + h % 2
        searches = args.searches
        span = step * (searches - 1)
        first = (h >> 8) % max(1, len(moves) - span + 1)
        if (h >> 16) % 3 == 0:
            limit = f"go nodes {2000 + (h >> 24) % 10000}"
        else:
            limit = f"go depth {5 + (h >> 24) % 5}"
        plies = [p for p in range(first, first + span + 1, step) if p <= len(moves)]
        # The last search goes back to the first position: its entries are in
        # the table, from older generations
        plies.append(plies[0])
        parts = []
        for p in plies:
            position = "position " + " ".join(start) + (" moves " + " ".join(moves[:p]) if p else "")
            parts += [position, limit]
        out.append(" ; ".join(parts))

    with open_output(args.output) as f:
        for session in out:
            f.write(session.encode() + b"\n")
    print(f"{args.output}: {len(out)} sessions, {sum(count_searches(s) for s in out)} searches")


# ---------------------------------------------------------------------------
# Generating and verifying


def cmd_generate(args):
    count = searches = 0
    digest = hashlib.sha256()
    with run_searchtrace(args.corpus, args.chess960) as lines, open_output(args.golden) as out:
        for number, session, group in session_groups(args.corpus, records_of(lines), "the engine"):
            if "error" in group[-1][1]:
                sys.exit(f"the engine rejects session {number}: {group[-1][0].decode().strip()}")
            for line, _ in group:
                out.write(line)
                digest.update(line)
                searches += 1
            count = number
    print(f"{args.golden}: {count} sessions, {searches} searches, sha256 of the content {digest.hexdigest()}")


def first_window(expected, actual):
    """The window of events where the event streams of two records first differ,
    from their checkpoints, as (from, to), or None if they do not differ."""
    e_points, a_points = expected.get("checkpoints", []), actual.get("checkpoints", [])
    for i, (e, a) in enumerate(zip(e_points, a_points)):
        if e != a:
            return i * INTERVAL, (i + 1) * INTERVAL
    if (
        len(e_points) == len(a_points)
        and expected.get("events") == actual.get("events")
        and expected.get("digest") == actual.get("digest")
    ):
        return None
    start = min(len(e_points), len(a_points)) * INTERVAL
    return start, start + INTERVAL


def differences(expected, actual, fields):
    """Describes how two records differ, as a list of (field, line)."""
    out = []
    names = fields if fields is not None else [f for f in expected if f not in KEY_FIELDS]
    names += [f for f in actual if fields is None and f not in expected and f not in KEY_FIELDS]
    for field in names:
        e, a = expected.get(field, MISSING), actual.get(field, MISSING)
        if e == a:
            continue
        show = lambda v: "no such member" if v is MISSING else json.dumps(v)
        if field in ("checkpoints", "digest", "events"):
            if any(f == "events" for f, _ in out):
                continue
            window = first_window(expected, actual)
            out.append(("events", f"events: the streams differ in events [{window[0]}, {window[1]})"))
        elif field == "state" and isinstance(e, dict) and isinstance(a, dict):
            tables = [t for t in e if e[t] != a.get(t, MISSING)] + [t for t in a if t not in e]
            out.append(("state", "state: " + ", ".join(tables) + " differ"))
        elif isinstance(e, list) and isinstance(a, list):
            index = next((i for i, (x, y) in enumerate(zip(e, a)) if x != y), min(len(e), len(a)))
            if index < min(len(e), len(a)):
                out.append((field, f"{field}[{index}]: expected {show(e[index])}, got {show(a[index])}"))
            else:
                out.append((field, f"{field}: expected {len(e)} entries, got {len(a)}"))
        else:
            out.append((field, f"{field}: expected {show(e)}, got {show(a)}"))
    return out


def cmd_verify(args):
    fields = args.fields.split(",") if args.fields else None
    if args.uci:
        other = [f for f in fields or [] if f not in UCI_FIELDS]
        if other:
            sys.exit(f"plain UCI output does not give {', '.join(other)}")
        fields = fields or list(UCI_FIELDS)
        actual_records = run_uci(args.corpus, args.chess960)
        context = contextlib.nullcontext(actual_records)
    else:
        context = run_searchtrace(args.corpus, args.chess960)

    num_sessions = num_searches = failed = failed_sessions = reported = 0
    by_field = {}
    known = set()
    with context as lines, open_read(args.golden) as golden:
        actual_groups = session_groups(args.corpus, records_of(lines), "the engine")
        expected_groups = session_groups(args.corpus, golden, args.golden)
        for (number, session, expected), (_, _, actual) in zip(expected_groups, actual_groups):
            num_sessions = number
            session_failed = False
            for e_pair, a_pair in itertools.zip_longest(expected, actual):
                num_searches += 1
                e = e_pair[1] if e_pair else {}
                a = a_pair[1] if a_pair else {"error": "no such search"}
                known.update(e)
                if fields is None and e_pair and a_pair and e_pair[0] == a_pair[0]:
                    continue
                diffs = differences(e, a, fields)
                if not diffs:
                    continue
                failed += 1
                session_failed = True
                for field, _ in diffs:
                    by_field[field] = by_field.get(field, 0) + 1
                if reported < args.max_report:
                    reported += 1
                    print(f"session {number}, search {e.get('search', '?')}: {e.get('fen', '?')}, go {e.get('go', '?')}")
                    for _, text in diffs:
                        print(f"  {text}")
            failed_sessions += session_failed
        # The engine must not print more than the corpus asks for
        for _ in actual_groups:
            pass

    unknown = [f for f in fields or [] if f not in known and f not in UCI_FIELDS]
    if unknown:
        sys.exit(f"no such field in the golden file: {', '.join(unknown)}")

    what = f"fields {','.join(fields)}" if fields else "all fields"
    if failed:
        print(
            f"FAILED {args.corpus}: {failed} of {num_searches} searches differ, "
            f"in {failed_sessions} of {num_sessions} sessions ({what})"
        )
        for field, n in sorted(by_field.items(), key=lambda item: -item[1]):
            print(f"  {field:20} {n:8}")
        if not args.uci and "events" in by_field:
            print("  run search_trace.py bisect on a session to see the first event that differs")
        sys.exit(1)
    print(f"OK {args.corpus}: {num_sessions} sessions, {num_searches} searches match ({what})")


# ---------------------------------------------------------------------------
# Bisecting


def read_events(corpus, chess960, exe, search, window):
    """The events of a window of one search, as (context lines, {index: text})."""
    context, events, records = [], {}, []
    with run_searchtrace(corpus, chess960, exe, (search, window[0], window[1])) as lines:
        for line in lines:
            line = line.decode().rstrip("\n")
            if line.startswith("{"):
                records.append(json.loads(line))
            elif line.startswith("context "):
                context.append(line[len("context ") :])
            elif line[:1].isdigit():
                index, _, text = line.partition(" ")
                events[int(index)] = text
    return context, events, records


def cmd_bisect(args):
    with tempfile.TemporaryDirectory(prefix="sf-bisect-") as tmp:
        session = next(itertools.islice(sessions(args.corpus), args.session - 1, None), None)
        if session is None:
            sys.exit(f"{args.corpus} has no session {args.session}")
        single = os.path.join(tmp, "session.txt")
        with open(single, "w") as f:
            f.write(session + "\n")

        expected = None
        with open_read(args.golden) as golden:
            for number, _, group in session_groups(args.corpus, golden, args.golden):
                if number == args.session:
                    expected = [r for _, r in group]
                    break
        if expected is None:
            sys.exit(f"{args.golden} has no records for session {args.session}")
        for r in expected:
            r["session"] = 1

        with run_searchtrace(single, args.chess960) as lines:
            actual = [json.loads(line) for line in records_of(lines)]

        search, window = args.search, None
        if args.window:
            window = tuple(args.window)
        for e, a in zip(expected, actual):
            if search and e.get("search") != search:
                continue
            diffs = differences(e, a, None)
            if diffs:
                search = e["search"]
                print(f"session {args.session}, search {search}: {e.get('fen')}, go {e.get('go')}")
                for _, text in diffs:
                    print(f"  {text}")
                window = window or first_window(e, a)
                break
        else:
            if not args.window:
                print(f"session {args.session} matches the golden file")
                return
            search = search or 1

        if window is None:
            print("The event streams match: the difference is in what the events do not show.")
            return

        ref = engine(DEFAULT_TRACE_EXE)
        ref[-1] = os.environ.get("REF", str(DEFAULT_TRACE_EXE))
        _, ref_events, ref_records = read_events(single, args.chess960, ref, search, window)
        _, got_events, _ = read_events(single, args.chess960, None, search, window)

        ref_record = next((r for r in ref_records if r.get("search") == search), {})
        golden_record = next((r for r in expected if r.get("search") == search), {})
        if ref_record.get("digest") != golden_record.get("digest"):
            print(f"warning: the reference {ref[-1]} does not reproduce the golden file for this search")

        print(f"events [{window[0]}, {window[1]}) of search {search}, reference {ref[-1]}")

        for index in range(window[0], window[1]):
            e, a = ref_events.get(index), got_events.get(index)
            if e == a:
                if e is None:
                    print("the window holds no event on either side")
                    return
                continue
            # The nodes open at that event, from the root down, with the move
            # being searched in each and the event that called the next one
            ref_context, _, _ = read_events(single, args.chess960, ref, search, (index, index + 1))
            print(f"nodes open at event {index}, in the reference:")
            for line in ref_context:
                print(f"  {line}")
            before = [i for i in range(max(window[0], index - args.before), index)]
            print("last events in common:")
            for i in before:
                print(f"  {i} {ref_events[i]}")
            print(f"first difference, event {index}:")
            print(f"  expected {e if e is not None else '(no event)'}")
            print(f"  got      {a if a is not None else '(no event)'}")
            print("next events, expected / got:")
            for i in range(index + 1, min(window[1], index + 1 + args.after)):
                print(f"  {i} {ref_events.get(i, '(none)')}")
                print(f"  {i} {got_events.get(i, '(none)')}")
            return
        print("the events of the window are the same on both sides")


# ---------------------------------------------------------------------------
# Sample and stats


def cmd_sample(args):
    all_sessions = list(sessions(args.corpus))
    hashes = sorted((hash64(s, "sample"), n) for n, s in enumerate(all_sessions, 1))
    keep = {n for _, n in hashes[: args.count]}
    # Games are few in a corpus, but only they inherit state from one search to
    # the next: some more are drawn among them
    games = [(h, n) for h, n in hashes if count_searches(all_sessions[n - 1]) > 1 and n not in keep]
    keep = sorted(keep | {n for _, n in games[: args.games]})
    renumber = {old: new for new, old in enumerate(keep, 1)}

    corpus_content = b"".join(all_sessions[n - 1].encode() + b"\n" for n in keep)
    golden_content = []
    with open_read(args.golden) as golden:
        for number, _, group in session_groups(args.corpus, golden, args.golden):
            if number in renumber:
                for line, _ in group:
                    prefix = f'{{"session":{number},'.encode()
                    assert line.startswith(prefix)
                    golden_content.append(f'{{"session":{renumber[number]},'.encode() + line[len(prefix) :])
    golden_content = b"".join(golden_content)

    for target, content in ((args.out_corpus, corpus_content), (args.out_golden, golden_content)):
        if os.path.exists(target):
            with open_read(target) as old:
                if old.read() == content:
                    print(f"{target}: unchanged")
                    continue
        with open_write(target) as out:
            out.write(content)
        print(f"{target}: {len(keep)} sessions written")


def cmd_stats(args):
    counts = {}

    def add(name, n=1):
        counts[name] = counts.get(name, 0) + n

    with open_read(args.golden) as golden:
        for line in golden:
            r = json.loads(line)
            add("searches")
            add("sessions", r["search"] == 1)
            add("searches inheriting the state of another", r["search"] > 1)
            kind = r["go"].split()[0]
            add(f"go {kind}")
            add("nodes", r["nodes"])
            add("events", r["events"])
            add("checkpoints", len(r["checkpoints"]))
            add("no legal move at the root", r["bestmove"] == "(none)")
            add("iterations", len(r["iterations"]))
            add("iterations not completed (stopped)", sum(not it["completed"] for it in r["iterations"]))
            add("mate score at the root", any(" mate " in i for i in r["info"][-1:]))
            add("lower or upper bound reported", sum(" lowerbound" in i or " upperbound" in i for i in r["info"]))
            add("ponder move", r["ponder"] is not None)

    for name, n in counts.items():
        print(f"{name:45} {n:12}")

    if args.corpus:
        # Count the kinds of events by streaming every event of every search
        # through the reference engine: as slow as generating the golden file
        kinds = {}
        with run_searchtrace(args.corpus, args.chess960, window=("all", 0, 1 << 62)) as lines:
            for line in lines:
                if not line[:1].isdigit():
                    continue
                words = line.split()
                kind = words[1].decode()
                if kind in ("prune", "immediate", "skip", "pick") and len(words) > 2:
                    detail = words[3] if kind == "pick" else words[2]
                    kind += ":" + detail.decode().split("=", 1)[1]
                kinds[kind] = kinds.get(kind, 0) + 1
        for name, n in sorted(kinds.items()):
            print(f"{name:45} {n:12}")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument(
        "--chess960", action="store_true", help="the corpus holds Chess960 positions: set UCI_Chess960"
    )
    sub = parser.add_subparsers(dest="command", required=True)

    select = sub.add_parser("select")
    select.add_argument("positions", help="corpus of positions (FENs)")
    select.add_argument("lines", help="corpus of lines of play")
    select.add_argument("output")
    select.add_argument("--positions-count", type=int, default=4000)
    select.add_argument("--games-count", type=int, default=300)
    select.add_argument("--searches", type=int, default=8, help="searches along each game, before the repeated first")
    select.add_argument("--rule50", type=int, default=80, help="also keep every position with this rule50 counter or more")
    select.set_defaults(run=cmd_select)

    generate = sub.add_parser("generate")
    generate.add_argument("corpus")
    generate.add_argument("golden")
    generate.set_defaults(run=cmd_generate)

    verify = sub.add_parser("verify")
    verify.add_argument("corpus")
    verify.add_argument("golden")
    verify.add_argument("--uci", action="store_true", help="drive the engine through plain UCI commands")
    verify.add_argument("--fields", help="comma separated list of the only members to compare")
    verify.add_argument("--max-report", type=int, default=10, help="searches to detail (default 10)")
    verify.set_defaults(run=cmd_verify)

    bisect = sub.add_parser("bisect")
    bisect.add_argument("corpus")
    bisect.add_argument("golden")
    bisect.add_argument("session", type=int, help="number of the session in the corpus, from 1")
    bisect.add_argument("--search", type=int, help="number of the search in the session, from 1")
    bisect.add_argument("--window", type=int, nargs=2, metavar=("FROM", "TO"), help="events to compare")
    bisect.add_argument("--before", type=int, default=12, help="events in common to show")
    bisect.add_argument("--after", type=int, default=4, help="events to show after the first difference")
    bisect.set_defaults(run=cmd_bisect)

    sample = sub.add_parser("sample")
    sample.add_argument("corpus")
    sample.add_argument("golden")
    sample.add_argument("count", type=int)
    sample.add_argument("out_corpus")
    sample.add_argument("out_golden")
    sample.add_argument("--games", type=int, default=0, help="also keep this many more sessions of several searches")
    sample.set_defaults(run=cmd_sample)

    stats = sub.add_parser("stats")
    stats.add_argument("golden")
    stats.add_argument("--corpus", help="also count the events, by running the engine over the corpus")
    stats.set_defaults(run=cmd_stats)

    args = parser.parse_args()
    args.run(args)


if __name__ == "__main__":
    main()
