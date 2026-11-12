#!/bin/bash
# Runs tests/differential/verify.sh with the given arguments and fails unless
# it succeeds AND reports a non-empty comparison for both sets (standard and
# chess960), so that a step cannot pass by comparing nothing.
#
#   verify-sample.sh sample [--fields ...]     at least MIN_POSITIONS per set
#   verify-sample.sh lines  [--fields ...]     at least MIN_LINES per set
#
# EXE and RUN_PREFIX are passed through to verify.sh.

set -o pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
mode=${1:?usage: $0 sample|lines [options of verify.sh]}

out=$(mktemp)
"$ROOT/tests/differential/verify.sh" "$@" | tee "$out"
status=${PIPESTATUS[0]}
if [ "$status" -ne 0 ]; then
  echo "::error::verify.sh $* failed with status $status"
  exit 1
fi

case $mode in
  sample) pattern='^OK .*: ([0-9]+) positions match' min=${MIN_POSITIONS:-1000} ;;
  lines)  pattern='^OK .*: ([0-9]+) lines, [0-9]+ records match' min=${MIN_LINES:-50} ;;
  *) echo "::error::unknown mode $mode"; exit 1 ;;
esac

counts=$(sed -nE "s/$pattern.*/\1/p" "$out")
if [ "$(echo "$counts" | grep -c .)" -ne 2 ]; then
  echo "::error::expected two OK lines from verify.sh $*, got: $counts"
  exit 1
fi
for n in $counts; do
  if [ "$n" -lt "$min" ]; then
    echo "::error::verify.sh $* compared only $n entries in a set (at least $min expected)"
    exit 1
  fi
done
echo "verified: $(echo $counts | tr ' ' '+') entries"
