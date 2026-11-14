#!/bin/sh
# Builds the C++ driver bench/tm_check.cpp against the object files of a
# build of src/, with the flags of that build (from a dry run of make), as
# bench/build_tt_check.sh does:
#
#   make -C ../src -j2 ARCH=x86-64-sse41-popcnt build    # the network in src/
#   ARCH=x86-64-sse41-popcnt bench/build_tm_check.sh     # -> target/cpp/tm_check
#
# The driver links the real TimeManagement (timeman.o) and OptionsMap
# (ucioption.o) of that build.
set -eu

cd "$(dirname "$0")/.."
D=$PWD/target/cpp
ARCH=${ARCH:-x86-64-sse41-popcnt}
COMP=${COMP:-gcc}
mkdir -p "$D/tmp"

cd ../src
cmd=$(make -n -B ARCH="$ARCH" COMP="$COMP" timeman.o | grep -- ' -c -o timeman.o timeman.cpp$')
CXX=${cmd%% *}
FLAGS=${cmd#* }
FLAGS=${FLAGS% -c -o timeman.o timeman.cpp}

$CXX $FLAGS -I. -c ../rust/bench/tm_check.cpp -o "$D/tm_check.o"
# every object of the engine but main.o
objs=$(ls ./*.o | grep -v -e '^./main.o$')
# shellcheck disable=SC2086
TMPDIR="$D/tmp" $CXX $FLAGS -o "$D/tm_check" "$D/tm_check.o" $objs -lpthread -lrt
echo "built $D/tm_check"
