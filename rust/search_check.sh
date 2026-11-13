#!/bin/sh
# The checks of the search beyond verify.sh, for CI and by hand (see
# README.md, "The search"). The network must be in the working directory or
# next to the binary; RUN_PREFIX wraps the binary (an emulator).
#
#   ./search_check.sh bench <engine>          bench gives the signature 3464885
#   ./search_check.sh deep <engine>           the deepest searches of the full
#                                             corpus (tests/search/deep.*),
#                                             all fields of the trace
#   ./search_check.sh trace-subset <n> [--uci] the first n sessions by hash of
#                                             each set of the trace sample
#                                             (search_trace.py sample), EXE the
#                                             engine; all of them if n is larger
#   ./search_check.sh tt <driver>             runs a transposition table driver
#                                             (bench/tt_check.cpp or
#                                             examples/tt_check.rs): what
#                                             tt_check.expected holds
set -eu

cd "$(dirname "$0")"
SIGNATURE=3464885
TRACE=../tests/differential/search_trace.py
SAMPLE=../tests/differential/sample
RUN_PREFIX=${RUN_PREFIX:-}
PYTHON=${PYTHON:-python3}

# checks that a verify output has an OK line per set, with searches compared
check_ok() {
    n=$(grep -cE '^OK .*: [0-9]+ sessions, [1-9][0-9]* searches match' "$1" || true)
    if [ "$n" -ne 2 ]; then
        echo "::error::expected two OK lines with searches compared, got $n"
        exit 1
    fi
}

case "${1:-}" in
bench)
    exe=${2:?engine}
    # shellcheck disable=SC2086
    nodes=$($RUN_PREFIX "$exe" bench 2>&1 >/dev/null | sed -n 's/^Nodes searched *: *//p')
    echo "bench: $nodes nodes"
    if [ "$nodes" != "$SIGNATURE" ]; then
        echo "::error::bench gives $nodes, expected $SIGNATURE"
        exit 1
    fi
    ;;
deep)
    exe=$(cd "$(dirname "${2:?engine}")" && pwd)/$(basename "$2")
    out=$(mktemp)
    EXE=$exe RUN_PREFIX="$RUN_PREFIX" $PYTHON $TRACE verify tests/search/deep.standard.trace.gz tests/search/deep.standard.trace.jsonl.gz | tee "$out"
    EXE=$exe RUN_PREFIX="$RUN_PREFIX" $PYTHON $TRACE --chess960 verify tests/search/deep.chess960.trace.gz tests/search/deep.chess960.trace.jsonl.gz | tee -a "$out"
    check_ok "$out"
    ;;
trace-subset)
    n=${2:?count}
    uci=${3:-}
    : "${EXE:?EXE must name the engine}"
    tmp=$(mktemp -d)
    out=$tmp/out
    for set in standard chess960; do
        flag=""
        [ $set = chess960 ] && flag=--chess960
        $PYTHON $TRACE sample $SAMPLE/$set.trace.gz $SAMPLE/$set.trace.jsonl.gz "$n" "$tmp/$set.trace" "$tmp/$set.trace.jsonl" > /dev/null
        echo "$set: $(wc -l < "$tmp/$set.trace") sessions"
        # shellcheck disable=SC2086
        EXE=$EXE RUN_PREFIX="$RUN_PREFIX" $PYTHON $TRACE $flag verify $uci "$tmp/$set.trace" "$tmp/$set.trace.jsonl" | tee -a "$out"
    done
    check_ok "$out"
    rm -rf "$tmp"
    ;;
tt)
    drv=${2:?driver}
    # shellcheck disable=SC2086
    $RUN_PREFIX "$drv" 100000 1 2 3 5 7 16 33
    ;;
*)
    echo "usage: $0 bench|deep <engine> | trace-subset <n> [--uci] | tt <driver>" >&2
    exit 2
    ;;
esac
