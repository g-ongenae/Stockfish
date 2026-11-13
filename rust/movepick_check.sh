#!/bin/sh
# Runs a move picker driver, bench/movepick_check.cpp (the C++ code) or
# examples/movepick_check.rs (the port), on positions of the differential
# sample, and prints what it prints. The two drivers must print the same.
#
#   EXE=<driver> [RUN_PREFIX=<emulator>] ./movepick_check.sh sample|subset
#
#   sample  the 5,000 positions of tests/differential/sample, with legal()
#           tried on every 16-bit move value in each of them
#   subset  their first 500 standard and 200 Chess960 positions, with legal()
#           on every value in one position of 5: what movepick_check.expected
#           holds, the output of the C++ driver, which CI compares with the
#           port on every target (fast enough under emulation)
#
# See README.md ("What the gate does not cover") for what the drivers do and
# how to build the C++ one.
set -eu

cd "$(dirname "$0")"
SAMPLE=../tests/differential/sample
: "${EXE:?EXE must name the driver}"
RUN_PREFIX=${RUN_PREFIX:-}
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

case "${1:-}" in
sample)
    zcat $SAMPLE/standard.fen.gz > "$TMP/standard.fen"
    zcat $SAMPLE/chess960.fen.gz > "$TMP/chess960.fen"
    every=1
    ;;
subset)
    zcat $SAMPLE/standard.fen.gz | head -500 > "$TMP/standard.fen"
    zcat $SAMPLE/chess960.fen.gz | head -200 > "$TMP/chess960.fen"
    every=5
    ;;
*)
    echo "usage: EXE=<driver> $0 sample|subset" >&2
    exit 2
    ;;
esac

echo "# standard"
$RUN_PREFIX "$EXE" check "$TMP/standard.fen" 0 $every
echo "# chess960"
$RUN_PREFIX "$EXE" check "$TMP/chess960.fen" 1 $every
