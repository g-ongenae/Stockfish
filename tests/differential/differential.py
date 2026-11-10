#!/usr/bin/env python3
"""Golden files for the 'dump' debug commands: generate them and verify against them.

The 'dump' command of Stockfish prints the internals of a position as one line
of JSON. A corpus is a file of FENs, one per line, and its golden file holds the
line printed for each of them by the reference engine. Verifying another binary
(or another build of the same engine) means dumping the corpus again and
comparing, position by position and field by field.

With --lines the corpus is a file of lines of play instead, one per line, as the
arguments of the UCI 'position' command, and the engine is run with 'dumpline'.
It prints one record per position of each line, then one record for the walk
back down the line; they are compared record by record, line by line.

    differential.py generate CORPUS GOLDEN     write the golden file of a corpus
    differential.py verify CORPUS GOLDEN       compare an engine with a golden file
    differential.py canonicalize IN OUT        rewrite FENs the way the engine prints them
    differential.py sample CORPUS GOLDEN N OUT_CORPUS OUT_GOLDEN
    differential.py stats GOLDEN               what the corpus covers
    differential.py --lines ...                the same for a corpus of lines

The engine is taken from the EXE environment variable (default: src/stockfish
next to this checkout) and can be wrapped with RUN_PREFIX, as tests/signature.sh
does. Files ending in .zst (needs the zstd program), .gz or .xz are compressed.
Only the standard library is needed.
"""

import argparse
import contextlib
import gzip
import hashlib
import itertools
import json
import lzma
import os
import pathlib
import shlex
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parent.parent.parent
DEFAULT_EXE = ROOT / "src" / "stockfish"

# Lists with one entry per legal move, in the order of "moves"
PER_MOVE_FIELDS = ("see", "gives_check", "key_after")

# Stands for a member an object does not have, which is not the same as null
MISSING = object()

# The records of 'dumpline' that start the dump of a line: its first position,
# or the error that replaces the whole dump
LINE_STARTS = (b'{"ply":0,', b'{"error"')


@contextlib.contextmanager
def open_read(path):
    """Opens a possibly compressed file for reading lines of bytes."""
    path = str(path)
    if path.endswith(".zst"):
        proc = subprocess.Popen(["zstd", "-dc", path], stdout=subprocess.PIPE)
        try:
            yield proc.stdout
        except BaseException:
            proc.stdout.close()
            proc.wait()
            raise
        # A truncated or damaged file must not pass for a shorter one
        proc.stdout.read()
        proc.stdout.close()
        if proc.wait() != 0:
            sys.exit(f"zstd failed reading {path}")
    elif path.endswith(".gz"):
        with gzip.open(path, "rb") as f:
            yield f
    elif path.endswith(".xz"):
        with lzma.open(path, "rb") as f:
            yield f
    else:
        with open(path, "rb") as f:
            yield f


@contextlib.contextmanager
def open_write(path):
    """Opens a possibly compressed file for writing bytes."""
    path = str(path)
    if path.endswith(".zst"):
        with open(path, "wb") as raw:
            proc = subprocess.Popen(["zstd", "-q", "-9", "-T0"], stdin=subprocess.PIPE, stdout=raw)
            try:
                yield proc.stdin
            finally:
                proc.stdin.close()
                if proc.wait() != 0:
                    sys.exit(f"zstd failed writing {path}")
    elif path.endswith(".gz"):
        # No name nor time in the header, so that the same content gives the same file
        with open(path, "wb") as raw:
            with gzip.GzipFile(filename="", mode="wb", fileobj=raw, compresslevel=9, mtime=0) as f:
                yield f
    elif path.endswith(".xz"):
        with lzma.open(path, "wb") as f:
            yield f
    else:
        with open(path, "wb") as f:
            yield f


@contextlib.contextmanager
def open_output(path):
    """Like open_write(), but the file only appears if everything went well."""
    path = pathlib.Path(path)
    partial = path.with_name(".partial-" + path.name)
    try:
        with open_write(partial) as f:
            yield f
        os.replace(partial, path)
    finally:
        if partial.exists():
            partial.unlink()


@contextlib.contextmanager
def dump_lines(corpus, chess960, lines_mode=False):
    """Runs the engine over a corpus and yields an iterator on its dump lines.

    The iterator ends the program if the engine does not print exactly one line
    per position of the corpus or, with lines_mode, does not start the dump of
    exactly one line of play per line of the corpus.
    """
    exe = os.environ.get("EXE", str(DEFAULT_EXE))
    prefix = shlex.split(os.environ.get("RUN_PREFIX", ""))

    # The engine reads the positions from a plain file
    with tempfile.TemporaryDirectory(prefix="sf-differential-") as tmp:
        fens = os.path.join(tmp, "positions.fen")
        expected = 0
        with open_read(corpus) as src, open(fens, "wb") as dst:
            for line in src:
                dst.write(line)
                expected += bool(line.strip())  # the engine skips blank lines

        commands = f"{'dumpline' if lines_mode else 'dump'} {fens}\nquit\n"
        if chess960:
            commands = "setoption name UCI_Chess960 value true\n" + commands

        try:
            proc = subprocess.Popen(prefix + [exe], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
        except OSError as e:
            sys.exit(f"cannot run {exe}: {e}")
        proc.stdin.write(commands.encode())
        proc.stdin.close()

        def lines():
            count = 0
            # Everything else on stdout is the banner and UCI info strings
            for line in proc.stdout:
                if line.startswith(b"{"):
                    count += not lines_mode or line.startswith(LINE_STARTS)
                    yield line
            proc.stdout.close()
            if proc.wait() != 0:
                sys.exit(f"{exe} exited with status {proc.returncode}")
            if count != expected:
                what = "lines of play" if lines_mode else "dump lines"
                units = "lines" if lines_mode else "positions"
                sys.exit(f"{exe} printed {count} {what} for the {expected} {units} of {corpus}")

        yield lines()


def cmd_generate(args):
    count, starts, digest = 0, 0, hashlib.sha256()
    with dump_lines(args.corpus, args.chess960, args.lines) as lines, open_output(args.golden) as out:
        for line in lines:
            if line.startswith(b'{"error"'):
                what = "line" if args.lines else "position"
                sys.exit(f"the engine rejects a {what} of the corpus: {line.decode().strip()}")
            out.write(line)
            digest.update(line)
            count += 1
            starts += line.startswith(LINE_STARTS)
    what = f"{starts} lines, {count} records" if args.lines else f"{count} positions"
    print(f"{args.golden}: {what}, sha256 of the content {digest.hexdigest()}")


def describe(field, expected, actual, moves):
    """Explains how two values of a field differ, in one line."""
    if isinstance(expected, dict) and isinstance(actual, dict):
        keys = sorted(set(expected) | set(actual))
        sub = next(k for k in keys if expected.get(k, MISSING) != actual.get(k, MISSING))
        return describe(f"{field}.{sub}", expected.get(sub, MISSING), actual.get(sub, MISSING), [])
    if isinstance(expected, list) and isinstance(actual, list):
        if len(expected) != len(actual):
            return f"{field}: expected {len(expected)} entries, got {len(actual)}"
        index = next(i for i, (e, a) in enumerate(zip(expected, actual)) if e != a)
        where = f"[{index}]"
        if field in PER_MOVE_FIELDS and index < len(moves):
            where += f" (move {moves[index]})"
        return f"{field}{where}: expected {expected[index]}, got {actual[index]}"
    show = lambda value: "no such member" if value is MISSING else json.dumps(value)
    return f"{field}: expected {show(expected)}, got {show(actual)}"


def cmd_verify(args):
    if args.lines:
        cmd_verify_lines(args)
        return

    fields = args.fields.split(",") if args.fields else None
    count = failed = 0
    by_field = {}

    with dump_lines(args.corpus, args.chess960) as lines, open_read(args.golden) as golden:
        for expected_line, actual_line in itertools.zip_longest(golden, lines):
            if expected_line is None or actual_line is None:
                which = "fewer" if expected_line is None else "more"
                sys.exit(f"{args.golden} holds {which} positions than {args.corpus}")

            count += 1
            if fields is None and expected_line == actual_line:
                continue

            expected = json.loads(expected_line)
            unknown = [f for f in fields or [] if f not in expected]
            if unknown:
                sys.exit(f"no such field in the golden file: {', '.join(unknown)}")
            try:
                actual = json.loads(actual_line)
            except ValueError:
                actual = {"error": "not JSON: " + actual_line.decode(errors="replace").strip()}

            names = fields if fields is not None else sorted(set(expected) | set(actual))
            differing = [f for f in names if expected.get(f, MISSING) != actual.get(f, MISSING)]
            if not differing:
                continue

            failed += 1
            for field in differing:
                by_field[field] = by_field.get(field, 0) + 1
            if failed <= args.max_report:
                print(f"position {count}: {expected.get('fen', '?')}")
                for field in differing:
                    line = describe(
                        field, expected.get(field, MISSING), actual.get(field, MISSING), expected.get("moves", [])
                    )
                    print(f"  {line}")

    what = f"fields {args.fields}" if fields else "all fields"
    if failed:
        print(f"FAILED {args.corpus}: {failed} of {count} positions differ ({what})")
        for field, n in sorted(by_field.items(), key=lambda item: -item[1]):
            print(f"  {field:20} {n:8}")
        sys.exit(1)
    print(f"OK {args.corpus}: {count} positions match ({what})")


def group_lines(records):
    """Groups the records of 'dumpline' by line of play."""
    group = []
    for record in records:
        if record.startswith(LINE_STARTS) and group:
            yield group
            group = []
        group.append(record)
    if group:
        yield group


def parse_record(line):
    try:
        return json.loads(line)
    except ValueError:
        return {"error": "not JSON: " + line.decode(errors="replace").strip()}


def where(record, line_number):
    """Names a record of 'dumpline' in a report."""
    if "ply" in record:
        move = f" after {record['move']}" if record.get("move") else ""
        return f"line {line_number}, ply {record['ply']}{move}: {record.get('fen', '?')}"
    if "undo" in record:
        return f"line {line_number}, walk back over {record['undo']} plies"
    return f"line {line_number}: {record.get('input', record.get('error', '?'))}"


def cmd_verify_lines(args):
    fields = args.fields.split(",") if args.fields else None
    known = set()
    num_lines = num_records = failed_records = 0
    failed_lines = 0
    by_field = {}
    reported = 0

    with dump_lines(args.corpus, args.chess960, True) as lines, open_read(args.golden) as golden:
        for expected_group, actual_group in itertools.zip_longest(group_lines(golden), group_lines(lines)):
            if expected_group is None or actual_group is None:
                which = "fewer" if expected_group is None else "more"
                sys.exit(f"{args.golden} holds {which} lines than {args.corpus}")
            num_lines += 1
            line_failed = False

            if len(expected_group) != len(actual_group):
                line_failed = True
                by_field["(number of records)"] = by_field.get("(number of records)", 0) + 1
                if reported < args.max_report:
                    reported += 1
                    print(where(json.loads(expected_group[0]), num_lines))
                    print(f"  expected {len(expected_group)} records, got {len(actual_group)}")

            for expected_line, actual_line in itertools.zip_longest(expected_group, actual_group):
                if expected_line is None:
                    break
                num_records += 1
                if actual_line is None:
                    failed_records += 1
                    continue
                if fields is None and expected_line == actual_line:
                    continue

                expected = json.loads(expected_line)
                known.update(expected)
                actual = parse_record(actual_line)
                names = fields if fields is not None else sorted(set(expected) | set(actual))
                differing = [f for f in names if expected.get(f, MISSING) != actual.get(f, MISSING)]
                if not differing:
                    continue

                failed_records += 1
                line_failed = True
                for field in differing:
                    by_field[field] = by_field.get(field, 0) + 1
                if reported < args.max_report:
                    reported += 1
                    print(where(expected, num_lines))
                    for field in differing:
                        print("  " + describe(field, expected.get(field, MISSING), actual.get(field, MISSING), []))
            failed_lines += line_failed

    # A misspelt field would otherwise compare equal everywhere
    unknown = [f for f in fields or [] if f not in known]
    if unknown:
        sys.exit(f"no such field in the golden file: {', '.join(unknown)}")

    what = f"fields {args.fields}" if fields else "all fields"
    if failed_lines:
        print(
            f"FAILED {args.corpus}: {failed_records} of {num_records} records differ, "
            f"in {failed_lines} of {num_lines} lines ({what})"
        )
        for field, n in sorted(by_field.items(), key=lambda item: -item[1]):
            print(f"  {field:20} {n:8}")
        sys.exit(1)
    print(f"OK {args.corpus}: {num_lines} lines, {num_records} records match ({what})")


def cmd_canonicalize(args):
    if args.lines:
        sys.exit("canonicalize only applies to a corpus of positions")

    seen = set()
    count = changed = rejected = 0
    with open_read(args.corpus) as original, dump_lines(args.corpus, args.chess960) as lines:
        with open_output(args.output) as out:
            # dump_lines() checks that there is one line per position
            for fen, line in itertools.zip_longest((f for f in original if f.strip()), lines):
                count += 1
                position = json.loads(line)
                if "error" in position:
                    rejected += 1
                    continue
                canonical = position["fen"]
                changed += canonical != fen.decode().strip()
                if canonical not in seen:
                    seen.add(canonical)
                    out.write(canonical.encode() + b"\n")
    print(
        f"{args.output}: {len(seen)} positions out of {count} "
        f"({changed} rewritten, {rejected} rejected by the engine)"
    )


def cmd_sample(args):
    # Keep the positions (or lines) with the smallest hashes: a uniform sample
    # that does not depend on the order of the corpus, in corpus order.
    with open_read(args.corpus) as corpus:
        hashes = sorted(
            (hashlib.blake2b(fen.strip(), digest_size=8).digest(), n) for n, fen in enumerate(corpus)
        )
    keep = {n for _, n in hashes[: args.count]}

    for source, target in ((args.corpus, args.out_corpus), (args.golden, args.out_golden)):
        with open_read(source) as src:
            units = group_lines(src) if args.lines and source == args.golden else ([line] for line in src)
            content = b"".join(b"".join(unit) for n, unit in enumerate(units) if n in keep)
        if os.path.exists(target):
            with open_read(target) as old:
                if old.read() == content:
                    print(f"{target}: unchanged")
                    continue
        with open_write(target) as out:
            out.write(content)
        print(f"{target}: {len(keep)} {'lines' if args.lines else 'positions'} written")


STANDARD_START = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1"


def parse_placement(placement):
    """Returns the occupancy of white and of black, and the piece on each square."""
    white = black = 0
    board = {}
    square = 56
    for c in placement:
        if c == "/":
            square -= 16
        elif c.isdigit():
            square += int(c)
        else:
            if c.isupper():
                white |= 1 << square
            else:
                black |= 1 << square
            board["abcdefgh"[square % 8] + str(square // 8 + 1)] = c
            square += 1
    return white, black, board


def is_castling(move, board, chess960):
    """Tells whether a move in UCI notation castles."""
    origin, target = move[:2], move[2:4]
    if board[origin] not in "Kk":
        return False
    if chess960:  # the king takes its own rook
        return board.get(target, "") == ("R" if board[origin] == "K" else "r")
    return abs(ord(origin[0]) - ord(target[0])) == 2


def is_en_passant(move, board, ep):
    """Tells whether a move in UCI notation takes en passant."""
    return board.get(move[:2], "") in "Pp" and move[2:4] == ep


def cmd_stats_lines(args):
    counts = {}

    def add(name, condition=True):
        counts[name] = counts.get(name, 0) + bool(condition)

    previous = None
    with open_read(args.golden) as golden:
        for line in golden:
            r = json.loads(line)
            if "undo" in r:
                add("walk backs")
                add("walk back positions evaluated", 0)
                counts["walk back positions evaluated"] += sum(v is not None for v in r["nnues"])
                continue
            if r["ply"] == 0:
                add("lines")
                add("lines starting from a FEN", r["fen"] != STANDARD_START)
            add("positions")
            placement, side, rights, ep, rule50, _ = r["fen"].split()
            _, _, board = parse_placement(placement)
            rule50 = int(rule50)
            if r["ply"] == 0:
                add("lines of 200 plies or more", 0)
            else:
                move = r["move"]
                pplacement, _, _, pep, _, _ = previous["fen"].split()
                _, _, pboard = parse_placement(pplacement)
                add("moves")
                add("castling", is_castling(move, pboard, args.chess960))
                add("en passant", is_en_passant(move, pboard, pep))
                add("promotion", len(move) == 5)
                add("underpromotion", len(move) == 5 and move[4] != "q")
                add("capture", r["captured"] is not None)
                add("check", r["checkers"] != "0000000000000000")
                add("king move", pboard[move[:2]] in "Kk")
            add("evaluated", r["nnue"] is not None)
            add("null move", r["null"] is not None)
            add("probe", r["probe"] is not None)
            if r["probe"]:
                pm = r["probe"]["move"]
                add("probe castling", is_castling(pm, board, args.chess960))
                add("probe en passant", is_en_passant(pm, board, ep))
                add("probe promotion", len(pm) == 5)
                add("probe capture or promotion", pm[2:4] in board or len(pm) == 5)
            add("repetition (distance > 0)", r["repetition"] > 0)
            add("repetition (distance < 0, threefold)", r["repetition"] < 0)
            add("is_draw", r["is_draw"])
            add("has_repeated", r["has_repeated"])
            add("upcoming_repetition", r["upcoming_repetition"])
            add("rule50 >= 14 (key adjusted)", rule50 >= 14)
            add("rule50 >= 80", rule50 >= 80)
            add("rule50 >= 100", rule50 >= 100)
            add("ply >= 64 (second search window)", r["ply"] >= 64)
            if r["ply"] == 200:
                counts["lines of 200 plies or more"] += 1
            previous = r

    for name, n in counts.items():
        print(f"{name:40} {n:9}")


def cmd_stats(args):
    if args.lines:
        cmd_stats_lines(args)
        return

    counts = {}
    castling = {}

    def add(name, condition=True):
        counts[name] = counts.get(name, 0) + bool(condition)

    with open_read(args.golden) as golden:
        for line in golden:
            p = json.loads(line)
            placement, side, rights, ep, rule50, _ = p["fen"].split()
            white, black, board = parse_placement(placement)
            pieces = len(board)
            own = white if side == "w" else black
            us = 0 if side == "w" else 1
            checkers = bin(int(p["checkers"], 16)).count("1")
            blockers = [int(b, 16) for b in p["blockers_for_king"]]
            moves = p["moves"]

            add("positions")
            add("white to move", side == "w")
            add("black to move", side == "b")
            add("in check", checkers >= 1)
            add("in double check", checkers >= 2)
            add("checkmate", checkers and not moves)
            add("stalemate", not checkers and not moves)
            add("en passant capture available", ep != "-")
            add("promotion available", any(len(m) == 5 for m in moves))
            add("pinned piece (side to move)", blockers[us] & own)
            add("pinned piece (either side)", (blockers[0] & white) or (blockers[1] & black))
            add("discovered check candidate", (blockers[1 - us] & own))
            add("move giving check available", any(p["gives_check"]))
            add("move with negative SEE available", any(s < 0 for s in p["see"]))
            add("some castling right left", rights != "-")
            add("castling move available", any(is_castling(m, board, p["chess960"]) for m in moves))
            add("rule50 counter >= 14", int(rule50) >= 14)
            add("32 pieces", pieces == 32)
            add("17 to 31 pieces", 17 <= pieces <= 31)
            add("8 to 16 pieces", 8 <= pieces <= 16)
            add("6 or 7 pieces", 6 <= pieces <= 7)
            add("4 or 5 pieces", 4 <= pieces <= 5)
            add("3 pieces or fewer", pieces <= 3)
            if not p["chess960"]:
                castling[rights] = castling.get(rights, 0) + 1
            else:
                key = f"{sum(c.isupper() for c in rights)} white, {sum(c.islower() for c in rights)} black"
                castling[key] = castling.get(key, 0) + 1

    total = counts["positions"]
    for name, n in counts.items():
        print(f"{name:40} {n:8} {100 * n / total:6.2f}%")
    print("castling rights:")
    for rights, n in sorted(castling.items(), key=lambda item: -item[1]):
        print(f"  {rights:38} {n:8} {100 * n / total:6.2f}%")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument(
        "--chess960",
        action="store_true",
        help="the corpus holds Chess960 positions: run the engine with UCI_Chess960 set",
    )
    parser.add_argument(
        "--lines",
        action="store_true",
        help="the corpus holds lines of play: run the engine with 'dumpline'",
    )
    commands = parser.add_subparsers(dest="command", required=True)

    generate = commands.add_parser("generate")
    generate.add_argument("corpus")
    generate.add_argument("golden")
    generate.set_defaults(run=cmd_generate)

    verify = commands.add_parser("verify")
    verify.add_argument("corpus")
    verify.add_argument("golden")
    verify.add_argument("--fields", help="comma separated list of the only fields to compare")
    verify.add_argument("--max-report", type=int, default=10, help="positions to detail (default 10)")
    verify.set_defaults(run=cmd_verify)

    canonicalize = commands.add_parser("canonicalize")
    canonicalize.add_argument("corpus")
    canonicalize.add_argument("output")
    canonicalize.set_defaults(run=cmd_canonicalize)

    sample = commands.add_parser("sample")
    sample.add_argument("corpus")
    sample.add_argument("golden")
    sample.add_argument("count", type=int)
    sample.add_argument("out_corpus")
    sample.add_argument("out_golden")
    sample.set_defaults(run=cmd_sample)

    stats = commands.add_parser("stats")
    stats.add_argument("golden")
    stats.set_defaults(run=cmd_stats)

    args = parser.parse_args()
    args.run(args)


if __name__ == "__main__":
    main()
