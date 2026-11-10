#!/bin/bash
# Verify the 'dump' output of an engine against the golden files.
#
# verify.sh             : check the sample kept in git (5000 positions)
# verify.sh full        : check the whole corpus built by make_corpus.sh
#
# Further arguments go to 'differential.py verify', e.g. to compare only what
# an implementation supports so far: verify.sh sample --fields fen,key,moves
#
# EXE selects the binary (default ../../src/stockfish), RUN_PREFIX wraps it.

DIR=$(cd "$(dirname "$0")" && pwd)
PYTHON=${PYTHON:-python3}

case ${1:-sample} in
   sample) data=$DIR/sample ext=gz ;;
   full) data=${DATA:-$DIR/data} ext=zst ;;
   *) echo "usage: $0 [sample|full] [options of 'differential.py verify']"; exit 2 ;;
esac
[ $# -gt 0 ] && shift

if [ ! -f "$data/standard.jsonl.$ext" ]; then
   echo "No golden files in $data, run make_corpus.sh first."
   exit 1
fi

status=0
"$PYTHON" "$DIR/differential.py" verify "$data/standard.fen.$ext" "$data/standard.jsonl.$ext" "$@" || status=1
"$PYTHON" "$DIR/differential.py" --chess960 verify "$data/chess960.fen.$ext" "$data/chess960.jsonl.$ext" "$@" || status=1

exit $status
