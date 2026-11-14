#!/bin/sh
# The checks of the search beyond verify.sh, for CI and by hand (see
# README.md, "The search"). The network must be in the working directory or
# next to the binary; RUN_PREFIX wraps the binary (an emulator).
#
#   ./search_check.sh bench <engine>          bench gives the signature 3464885
#   ./search_check.sh deep <engine>           the deepest searches of the full
#                                             corpus (tests/search/deep.*),
#                                             all fields of the trace
#   REF=<engine> ./search_check.sh compare <engine>
#                                             direct comparisons over UCI with a
#                                             reference (bench/compare_uci.py):
#                                             what the trace does not reach
#   ./search_check.sh clock <engine>          searches on a clock under nodestime
#                                             (part of tests/search/clock*.sessions)
#                                             against the reference's output kept
#                                             in tests/search/clock*.expected.jsonl
#   REF=<engine> ./search_check.sh clock-record <dir>
#                                             writes those expected files into dir
#                                             (CI compares them with the kept ones)
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
compare)
    # against a reference binary over UCI (REF, default the shared folder's
    # oracle/stockfish): the sessions of tests/search, then part of the trace
    # sample with other hash sizes, UCI_ShowWDL and MultiPV
    exe=$(cd "$(dirname "${2:?engine}")" && pwd)/$(basename "$2")
    c="$PYTHON bench/compare_uci.py"
    export EXE=$exe
    $c tests/search/extra.sessions
    $c tests/search/extra960.sessions --chess960
    $c tests/search/long.sessions
    $c tests/search/long960.sessions --chess960
    for h in 1 3 7 64; do
        $c $SAMPLE/standard.trace.gz --option Hash=$h --every 4
    done
    $c $SAMPLE/chess960.trace.gz --option Hash=5 --every 3 --chess960
    $c $SAMPLE/standard.trace.gz --option UCI_ShowWDL=true --every 4
    $c $SAMPLE/standard.trace.gz --option MultiPV=3 --every 6
    $c $SAMPLE/chess960.trace.gz --option MultiPV=4 --option UCI_ShowWDL=true --every 6 --chess960
    # searches on a clock, under nodestime (README.md, "Time management")
    $c tests/search/clock.sessions
    $c tests/search/clockgames.sessions
    $c tests/search/clockgames960.sessions --chess960
    ;;
clock | clock-record)
    # the part of the clock sessions the emulated targets run (a few minutes
    # under qemu): name, then the arguments of compare_uci.py
    mode=$1
    target=${2:?engine or directory}
    if [ "$mode" = clock ]; then
        EXE=$(cd "$(dirname "$target")" && pwd)/$(basename "$target")
        export EXE RUN_PREFIX
    fi
    part() {
        name=$1
        shift
        if [ "$mode" = clock ]; then
            $PYTHON bench/compare_uci.py "$@" --expected "tests/search/$name.expected.jsonl"
        else
            $PYTHON bench/compare_uci.py "$@" --record "$target/$name.expected.jsonl"
        fi
    }
    part clock tests/search/clock.sessions --every 5
    part clockgames tests/search/clockgames.sessions --every 37
    part clockgames960 tests/search/clockgames960.sessions --every 19 --chess960
    ;;
tt)
    drv=${2:?driver}
    # shellcheck disable=SC2086
    $RUN_PREFIX "$drv" 100000 1 2 3 5 7 16 33
    ;;
*)
    echo "usage: $0 bench|deep|compare|clock <engine> | clock-record <dir> | tt <driver>" >&2
    exit 2
    ;;
esac
