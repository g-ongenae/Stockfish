#!/usr/bin/env python3
"""Compares two outputs of `examples/tm_check raw` (README.md, "Time
management", "Floating point"): the port's time manager on x86-64 and on
another target. Counts the calls whose doubles differ (originalTimeAdjust and
the two values the optimum and the maximum are truncated from), the largest
relative difference, and the calls whose integers differ, which is what would
change a decision. Prints a summary; exits 1 if an integer differs.

    tm_doubles.py REFERENCE OTHER
"""

import struct
import sys


def f(h):
    return struct.unpack(">d", bytes.fromhex(h))[0]


def main():
    a, b = (open(p) for p in sys.argv[1:3])
    calls = doubles = ints = 0
    worst = 0.0
    first = []
    for la, lb in zip(a, b, strict=True):
        calls += 1
        if la == lb:
            continue
        ia, ra = la.rsplit(" ", 3)[0], la.split()[-3:]
        ib, rb = lb.rsplit(" ", 3)[0], lb.split()[-3:]
        if ia != ib:
            ints += 1
            if len(first) < 5:
                first.append((la.strip(), lb.strip()))
        for x, y in zip(ra, rb):
            if x != y:
                fx, fy = f(x), f(y)
                if fx == fx and fy == fy and fx != 0:
                    worst = max(worst, abs(fx - fy) / abs(fx))
        doubles += ra != rb
    print(f"{calls} calls of init(): doubles differ in {doubles}, largest relative difference {worst:.3g}; "
          f"integers differ in {ints}")
    for x, y in first:
        print(f"  reference {x}\n  other     {y}")
    sys.exit(1 if ints else 0)


if __name__ == "__main__":
    main()
