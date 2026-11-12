#!/bin/bash
# Verify the 'dump' and 'dumpline' output of an engine against the golden files,
# and its searches against those of the search trace.
#
# verify.sh             : check the positions of the sample kept in git (5000)
# verify.sh full        : check all the positions built by make_corpus.sh
# verify.sh lines       : check the lines of play of the sample kept in git
# verify.sh lines-full  : check all the lines of play built by make_corpus.sh
# verify.sh trace       : check the searches of the sample kept in git, with a
#                         tracing binary (default ../../src/stockfish-trace)
# verify.sh trace-full  : check all the searches built by make_trace_corpus.sh
# verify.sh trace-uci   : check the UCI output of the searches of the sample, with
#                         any UCI engine (default ../../src/stockfish)
# verify.sh trace-uci-full : the same for all the searches
#
# Further arguments go to 'differential.py verify' (or 'search_trace.py verify'),
# e.g. to compare only what an implementation supports so far:
# verify.sh sample --fields fen,key,moves
#
# EXE selects the binary (default ../../src/stockfish), RUN_PREFIX wraps it.

DIR=$(cd "$(dirname "$0")" && pwd)
PYTHON=${PYTHON:-python3}

case ${1:-sample} in
   sample) data=$DIR/sample ext=gz kind= mode= ;;
   full) data=${DATA:-$DIR/data} ext=zst kind= mode= ;;
   lines) data=$DIR/sample ext=gz kind=.lines mode=--lines ;;
   lines-full) data=${DATA:-$DIR/data} ext=zst kind=.lines mode=--lines ;;
   trace) data=$DIR/sample ext=gz kind=.trace mode= ;;
   trace-full) data=${DATA:-$DIR/data} ext=zst kind=.trace mode= ;;
   trace-uci) data=$DIR/sample ext=gz kind=.trace mode=--uci ;;
   trace-uci-full) data=${DATA:-$DIR/data} ext=zst kind=.trace mode=--uci ;;
   *) echo "usage: $0 [sample|full|lines|lines-full|trace|trace-full|trace-uci|trace-uci-full] [options of 'verify']"; exit 2 ;;
esac
[ $# -gt 0 ] && shift

if [ ! -f "$data/standard$kind.jsonl.$ext" ]; then
   if [ "$kind" = .trace ]; then
      echo "No golden files in $data, run make_trace_corpus.sh first."
   else
      echo "No golden files in $data, run make_corpus.sh first."
   fi
   exit 1
fi

status=0
if [ "$kind" = .trace ]; then
   if [ "$mode" = --uci ]; then
      export EXE=${EXE:-$DIR/../../src/stockfish}
   else
      export EXE=${EXE:-$DIR/../../src/stockfish-trace}
   fi
   "$PYTHON" "$DIR/search_trace.py" verify $mode "$data/standard.trace.$ext" "$data/standard.trace.jsonl.$ext" "$@" || status=1
   "$PYTHON" "$DIR/search_trace.py" --chess960 verify $mode "$data/chess960.trace.$ext" "$data/chess960.trace.jsonl.$ext" "$@" || status=1
   exit $status
fi

if [ -n "$kind" ]; then
   corpus=lines
else
   corpus=fen
fi

"$PYTHON" "$DIR/differential.py" $mode verify "$data/standard.$corpus.$ext" "$data/standard$kind.jsonl.$ext" "$@" || status=1
"$PYTHON" "$DIR/differential.py" $mode --chess960 verify "$data/chess960.$corpus.$ext" "$data/chess960$kind.jsonl.$ext" "$@" || status=1

exit $status
