#!/bin/bash
# Rebuilds the corpus of the search trace and its golden files: chooses the
# sessions from the corpora of positions and of lines of play built by
# make_corpus.sh, runs them with the tracing binary and refreshes the small
# sample kept in git.
#
# The bulk data goes to tests/differential/data (not in git, 7 MB). It takes
# about 7 minutes on a slow machine.
#
# The corpus and the golden files built are compared with the checksums recorded
# in CONTENT.trace.sha256. If they differ the script fails and leaves the sample
# alone, unless called with --update, which records the new ones: do that on
# purpose only, when the engine or its network changed.
#
# Needs zstd, sha256sum and python3. EXE selects the tracing binary (default
# ../../src/stockfish-trace, built with 'make trace-build'), PYTHON the interpreter.

set -euo pipefail

DIR=$(cd "$(dirname "$0")" && pwd)
DATA=${DATA:-$DIR/data}
PYTHON=${PYTHON:-python3}
export EXE=${EXE:-$DIR/../../src/stockfish-trace}

UPDATE=no
[ "${1:-}" = "--update" ] && UPDATE=yes

# Sessions of each set: single searches on positions, and games
POSITIONS_STANDARD=8000
POSITIONS_CHESS960=2000
GAMES_STANDARD=600
GAMES_CHESS960=200

# Sessions of each set kept in the sample, then games added to them
SAMPLE_STANDARD=250
SAMPLE_CHESS960=60
SAMPLE_GAMES_STANDARD=40
SAMPLE_GAMES_CHESS960=15

if [ ! -x "$EXE" ]; then
   echo "No tracing binary at $EXE, build it first (cd src && make -j2 trace-build) or set EXE."
   exit 1
fi

for f in standard.fen standard.lines chess960.fen chess960.lines; do
   if [ ! -f "$DATA/$f.zst" ]; then
      echo "No $DATA/$f.zst, run make_corpus.sh first."
      exit 1
   fi
done

# 1. Choose the sessions and run them
build_set()
{
  name=$1 positions=$2 games=$3
  shift 3

  "$PYTHON" "$DIR/search_trace.py" "$@" select "$DATA/$name.fen.zst" "$DATA/$name.lines.zst" \
     "$DATA/$name.trace.zst" --positions-count "$positions" --games-count "$games"
  "$PYTHON" "$DIR/search_trace.py" "$@" generate "$DATA/$name.trace.zst" "$DATA/$name.trace.jsonl.zst"
}

build_set standard $POSITIONS_STANDARD $GAMES_STANDARD
build_set chess960 $POSITIONS_CHESS960 $GAMES_CHESS960 --chess960

# 2. Compare what was built with what is recorded in git
for f in standard.trace standard.trace.jsonl chess960.trace chess960.trace.jsonl; do
   echo "$(zstd -dc "$DATA/$f.zst" | sha256sum | cut -d' ' -f1)  $f"
done > "$DATA/CONTENT.trace.sha256"

if diff "$DIR/CONTENT.trace.sha256" "$DATA/CONTENT.trace.sha256"; then
   echo "The corpus and the golden files are the recorded ones."
elif [ $UPDATE = yes ]; then
   cp "$DATA/CONTENT.trace.sha256" "$DIR/CONTENT.trace.sha256"
   echo "CONTENT.trace.sha256 updated."
else
   echo "The corpus or the golden files differ from the ones recorded in CONTENT.trace.sha256."
   echo "If the engine or its network changed on purpose, run again with --update."
   exit 1
fi

# 3. Refresh the sample kept in git
mkdir -p "$DIR/sample"
sample_set()
{
  "$PYTHON" "$DIR/search_trace.py" sample "$DATA/$1.trace.zst" "$DATA/$1.trace.jsonl.zst" "$2" \
     "$DIR/sample/$1.trace.gz" "$DIR/sample/$1.trace.jsonl.gz" --games "$3"
}
sample_set standard $SAMPLE_STANDARD $SAMPLE_GAMES_STANDARD
sample_set chess960 $SAMPLE_CHESS960 $SAMPLE_GAMES_CHESS960

du -ch --apparent-size "$DATA"/*.trace*.zst | tail -1
