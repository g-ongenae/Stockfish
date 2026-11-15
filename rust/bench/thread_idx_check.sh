#!/bin/sh
# The values the search derives from the index of its thread (the aspiration
# window, the null move reduction), on one thread searching as thread k for
# k = 1 to 7: the UCI output of the sessions of the trace sample, digested,
# one line per k. The port built with the feature thread_idx_offset and the
# C++ engine patched with bench/thread_idx_offset.patch must print the same
# lines; thread_idx.expected holds those of the C++ engine.
#
#   thread_idx_check.sh ENGINE [SESSIONS]
#
# RUN_PREFIX, if set, is put before ENGINE (an emulator).
set -eu
here=$(dirname "$0")
exe=$1
sessions=${2:-10000}
for k in 1 2 3 4 5 6 7; do
    out=$(SF_THREAD_IDX_OFFSET=$k python3 "$here/after_resize.py" --record --sessions "$sessions" "$exe")
    n=$(printf '%s\n' "$out" | wc -l)
    sum=$(printf '%s\n' "$out" | sha256sum | cut -c1-64)
    echo "offset $k searches $n sha256 $sum"
done
