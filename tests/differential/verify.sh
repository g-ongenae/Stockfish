#!/bin/bash
# Verify the 'dump' and 'dumpline' output of an engine against the golden files.
#
# verify.sh             : check the positions of the sample kept in git (5000)
# verify.sh full        : check all the positions built by make_corpus.sh
# verify.sh lines       : check the lines of play of the sample kept in git
# verify.sh lines-full  : check all the lines of play built by make_corpus.sh
#
# Further arguments go to 'differential.py verify', e.g. to compare only what
# an implementation supports so far: verify.sh sample --fields fen,key,moves
#
# EXE selects the binary (default ../../src/stockfish), RUN_PREFIX wraps it.

DIR=$(cd "$(dirname "$0")" && pwd)
PYTHON=${PYTHON:-python3}

case ${1:-sample} in
   sample) data=$DIR/sample ext=gz kind= mode= ;;
   full) data=${DATA:-$DIR/data} ext=zst kind= mode= ;;
   lines) data=$DIR/sample ext=gz kind=.lines mode=--lines ;;
   lines-full) data=${DATA:-$DIR/data} ext=zst kind=.lines mode=--lines ;;
   *) echo "usage: $0 [sample|full|lines|lines-full] [options of 'differential.py verify']"; exit 2 ;;
esac
[ $# -gt 0 ] && shift

if [ ! -f "$data/standard$kind.jsonl.$ext" ]; then
   echo "No golden files in $data, run make_corpus.sh first."
   exit 1
fi

if [ -n "$kind" ]; then
   corpus=lines
else
   corpus=fen
fi

status=0
"$PYTHON" "$DIR/differential.py" $mode verify "$data/standard.$corpus.$ext" "$data/standard$kind.jsonl.$ext" "$@" || status=1
"$PYTHON" "$DIR/differential.py" $mode --chess960 verify "$data/chess960.$corpus.$ext" "$data/chess960$kind.jsonl.$ext" "$@" || status=1

exit $status
