#!/bin/sh
# Builds the C++ driver bench/threads_check.cpp against the object files of a
# build of src/, with the flags of that build (from a dry run of make), as
# bench/build_tt_check.sh does:
#
#   make -C ../src -j2 ARCH=x86-64-sse41-popcnt build       # the network in src/
#   ARCH=x86-64-sse41-popcnt bench/build_threads_check.sh   # -> target/cpp/threads_check
#
# The driver constructs an Engine, so it is linked with every object of the
# engine but main.o (the network is embedded in them).
set -eu

cd "$(dirname "$0")/.."
D=$PWD/target/cpp
ARCH=${ARCH:-x86-64-sse41-popcnt}
COMP=${COMP:-gcc}
mkdir -p "$D/tmp"

cd ../src
cmd=$(make -n -B ARCH="$ARCH" COMP="$COMP" thread.o | grep -- ' -c -o thread.o thread.cpp$')
CXX=${cmd%% *}
FLAGS=${cmd#* }
FLAGS=${FLAGS% -c -o thread.o thread.cpp}

$CXX $FLAGS -I. -c ../rust/bench/threads_check.cpp -o "$D/threads_check.o"
objs=$(ls ./*.o | grep -v -e '^./main.o$')
# shellcheck disable=SC2086
TMPDIR="$D/tmp" $CXX $FLAGS -o "$D/threads_check" "$D/threads_check.o" $objs -lpthread -lrt
echo "built $D/threads_check"
