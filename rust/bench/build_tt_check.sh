#!/bin/sh
# Builds the C++ driver bench/tt_check.cpp against the object files of a
# build of src/, with the flags of that build (from a dry run of make), as
# bench/build_movepick_check.sh does:
#
#   make -C ../src -j2 ARCH=x86-64-sse41-popcnt build    # the network in src/
#   ARCH=x86-64-sse41-popcnt bench/build_tt_check.sh     # -> target/cpp/tt_check
#
# tt.cpp is compiled again with SEARCH_TRACE, for trace_entry() and
# trace_digest() (which change nothing else), and replaces tt.o.
set -eu

cd "$(dirname "$0")/.."
D=$PWD/target/cpp
ARCH=${ARCH:-x86-64-sse41-popcnt}
COMP=${COMP:-gcc}
mkdir -p "$D/tmp"

cd ../src
cmd=$(make -n -B ARCH="$ARCH" COMP="$COMP" tt.o | grep -- ' -c -o tt.o tt.cpp$')
CXX=${cmd%% *}
FLAGS=${cmd#* }
FLAGS=${FLAGS% -c -o tt.o tt.cpp}

$CXX $FLAGS -DSEARCH_TRACE -c tt.cpp -o "$D/tt_trace.o"
$CXX $FLAGS -DSEARCH_TRACE -I. -c ../rust/bench/tt_check.cpp -o "$D/tt_check.o"
# every object of the engine but main.o and tt.o
objs=$(ls ./*.o | grep -v -e '^./main.o$' -e '^./tt.o$')
# shellcheck disable=SC2086
TMPDIR="$D/tmp" $CXX $FLAGS -o "$D/tt_check" "$D/tt_check.o" "$D/tt_trace.o" $objs -lpthread -lrt
echo "built $D/tt_check"
