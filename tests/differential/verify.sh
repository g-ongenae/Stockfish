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
# EXE selects the binary (default ../../src/stockfish, or ../../src/stockfish-trace
# for trace and trace-full), RUN_PREFIX wraps it. There is no other default: the
# binary checked is never taken from the shared folder unless EXE names it.
#
# The full modes read the golden files from DATA if set, else from
# tests/differential/data if it holds those of the mode, else from the shared
# folder, $STOCKFISH_SHARED/data (default ~/.local/share/stockfish-fork/data,
# see shared.sh). The binary and the folder used are printed on stderr before
# the engine runs. A full mode fails unless the corpus and the golden files hold,
# decompressed, the contents recorded in git (CONTENT.sha256,
# CONTENT.trace.sha256): they are hashed as they are compared, so a gate cannot
# pass on other data, whatever the data folder holds besides.

DIR=$(cd "$(dirname "$0")" && pwd)
PYTHON=${PYTHON:-python3}
. "$DIR/shared.sh"

MODE_NAME=${1:-sample}
case $MODE_NAME in
   sample) data=$DIR/sample ext=gz kind= mode= ;;
   full) data=${DATA:-} ext=zst kind= mode= ;;
   lines) data=$DIR/sample ext=gz kind=.lines mode=--lines ;;
   lines-full) data=${DATA:-} ext=zst kind=.lines mode=--lines ;;
   trace) data=$DIR/sample ext=gz kind=.trace mode= ;;
   trace-full) data=${DATA:-} ext=zst kind=.trace mode= ;;
   trace-uci) data=$DIR/sample ext=gz kind=.trace mode=--uci ;;
   trace-uci-full) data=${DATA:-} ext=zst kind=.trace mode=--uci ;;
   *) echo "usage: $0 [sample|full|lines|lines-full|trace|trace-full|trace-uci|trace-uci-full] [options of 'verify']"; exit 2 ;;
esac
[ $# -gt 0 ] && shift

# The full modes: DATA, else the checkout's data/ if it has this mode's golden
# files, else the shared folder
if [ $ext = zst ] && [ -z "$data" ]; then
   if [ -f "$DIR/data/standard$kind.jsonl.zst" ]; then
      data=$DIR/data
   else
      data=${SHARED_DATA:-$DIR/data}
   fi
fi

if [ ! -f "$data/standard$kind.jsonl.$ext" ]; then
   if [ "$kind" = .trace ]; then
      echo "No golden files in $data, run make_trace_corpus.sh first."
   else
      echo "No golden files in $data, run make_corpus.sh first."
   fi
   exit 1
fi

# The full modes compare the contents with those recorded in git as they read
# them (--content). A data folder that says it was made for other contents (a
# rebuild with another engine or corpus) is refused at once. --content goes
# after the arguments passed on, so that none of them can replace it (argparse
# keeps the last one, abbreviations included).
content=()
if [ $ext = zst ]; then
   manifest=CONTENT$([ "$kind" = .trace ] && echo .trace).sha256
   if [ -f "$data/$manifest" ] && ! cmp -s "$data/$manifest" "$DIR/$manifest"; then
      echo "$data/$manifest differs from $DIR/$manifest: these golden files are not the ones recorded in git."
      exit 1
   fi
   content=(--content "$DIR/$manifest")
fi

# The binary checked: EXE, else the one of this checkout. Said on stderr, with
# a note when it is the reference binary of the shared folder.
if [ "$kind" = .trace ] && [ "$mode" != --uci ]; then
   export EXE=${EXE:-$DIR/../../src/stockfish-trace}
else
   export EXE=${EXE:-$DIR/../../src/stockfish}
fi
note=
for ref in "$STOCKFISH_SHARED"/oracle/stockfish*; do
   if [ -f "$ref" ] && [ "$EXE" -ef "$ref" ]; then
      note=" (the reference binary of the shared folder)"
   fi
done
echo "verify.sh ${MODE_NAME}: engine $EXE$note${RUN_PREFIX:+, run with $RUN_PREFIX}; golden files in $data" >&2

status=0
if [ "$kind" = .trace ]; then
   "$PYTHON" "$DIR/search_trace.py" verify $mode "$data/standard.trace.$ext" "$data/standard.trace.jsonl.$ext" "$@" ${content[@]+"${content[@]}"} || status=1
   "$PYTHON" "$DIR/search_trace.py" --chess960 verify $mode "$data/chess960.trace.$ext" "$data/chess960.trace.jsonl.$ext" "$@" ${content[@]+"${content[@]}"} || status=1
   exit $status
fi

if [ -n "$kind" ]; then
   corpus=lines
else
   corpus=fen
fi

"$PYTHON" "$DIR/differential.py" $mode verify "$data/standard.$corpus.$ext" "$data/standard$kind.jsonl.$ext" "$@" ${content[@]+"${content[@]}"} || status=1
"$PYTHON" "$DIR/differential.py" $mode --chess960 verify "$data/chess960.$corpus.$ext" "$data/chess960$kind.jsonl.$ext" "$@" ${content[@]+"${content[@]}"} || status=1

exit $status
