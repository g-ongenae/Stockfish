#!/bin/sh
# Builds the C++ driver bench/movepick_check.cpp against the object files of
# a build of src/, with the flags of that build (from a dry run of make):
#
#   make -C ../src -j2 ARCH=x86-64-sse41-popcnt build    # the network in src/
#   ARCH=x86-64-sse41-popcnt bench/build_movepick_check.sh  # -> target/cpp/movepick_check
#
# movepick.cpp is compiled again with SEARCH_TRACE, for the stage and score
# of each move (MovePicker::trace_*, which change nothing else), and replaces
# movepick.o. The link runs in src/, as the engine embeds the network then;
# its temporary files go to target/cpp/tmp rather than /tmp, as LTO with the
# embedded network needs some room.
set -eu

cd "$(dirname "$0")/.."
D=$PWD/target/cpp
ARCH=${ARCH:-x86-64-sse41-popcnt}
COMP=${COMP:-gcc}
mkdir -p "$D/tmp"

cd ../src
cmd=$(make -n -B ARCH="$ARCH" COMP="$COMP" movepick.o | grep -- ' -c -o movepick.o movepick.cpp$')
CXX=${cmd%% *}
FLAGS=${cmd#* }
FLAGS=${FLAGS% -c -o movepick.o movepick.cpp}

$CXX $FLAGS -DSEARCH_TRACE -c movepick.cpp -o "$D/movepick_trace.o"
$CXX $FLAGS -DSEARCH_TRACE -I. -c ../rust/bench/movepick_check.cpp -o "$D/movepick_check.o"
# every object of the engine but main.o and movepick.o
objs=$(ls ./*.o | grep -v -e '^./main.o$' -e '^./movepick.o$')
# shellcheck disable=SC2086
TMPDIR="$D/tmp" $CXX $FLAGS -o "$D/movepick_check" "$D/movepick_check.o" "$D/movepick_trace.o" $objs \
    -lpthread -lrt
echo "built $D/movepick_check"
