#!/bin/sh
# Builds the C++ driver bench/surface_check.cpp against the object files of a
# build of src/ patched with bench/skill_seed.patch (SRC, a copy of src/ with
# ../scripts next to it), with the flags of that build:
#
#   cp -r ../src ../scripts "$T"/ && patch -d "$T/src" -p2 < bench/skill_seed.patch
#   make -C "$T/src" -j2 ARCH=x86-64-sse41-popcnt build        # the network in src/
#   SRC=$T/src ARCH=x86-64-sse41-popcnt bench/build_surface_check.sh   # -> target/cpp/surface_check
set -eu

cd "$(dirname "$0")/.."
D=$PWD/target/cpp
B=$PWD/bench
ARCH=${ARCH:-x86-64-sse41-popcnt}
COMP=${COMP:-gcc}
SRC=${SRC:?SRC must name the patched copy of src/}
mkdir -p "$D/tmp"

cd "$SRC"
grep -q SF_SKILL_SEED search.cpp || { echo "$SRC is not patched with bench/skill_seed.patch" >&2; exit 1; }
cmd=$(make -n -B ARCH="$ARCH" COMP="$COMP" timeman.o | grep -- ' -c -o timeman.o timeman.cpp$')
CXX=${cmd%% *}
FLAGS=${cmd#* }
FLAGS=${FLAGS% -c -o timeman.o timeman.cpp}

$CXX $FLAGS -I. -c "$B/surface_check.cpp" -o "$D/surface_check.o"
# every object of the engine but main.o
objs=$(ls ./*.o | grep -v -e '^./main.o$')
# shellcheck disable=SC2086
TMPDIR="$D/tmp" $CXX $FLAGS -o "$D/surface_check" "$D/surface_check.o" $objs -lpthread -lrt
echo "built $D/surface_check"
