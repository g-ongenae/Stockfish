#!/bin/bash
# Rebuilds the position corpus and its golden files from the pinned Lichess
# database dumps: downloads them, checks their checksums, extracts the positions,
# dumps them with the engine and refreshes the small sample kept in git.
#
# The dumps are never decompressed to disk. The bulk data goes to
# tests/differential/data (not in git, about 200 MB). It takes a few minutes.
#
# The corpus and the golden files built are compared with the checksums recorded
# in CONTENT.sha256. If they differ the script fails and leaves the sample alone,
# unless called with --update, which records the new ones: do that on purpose
# only, when the engine or its network changed.
#
# Needs curl, zstd, sha256sum and python3 with python-chess (pip install chess).
# EXE selects the engine (default ../../src/stockfish), PYTHON the interpreter.

set -euo pipefail

DIR=$(cd "$(dirname "$0")" && pwd)
DATA=${DATA:-$DIR/data}
PYTHON=${PYTHON:-python3}
export EXE=${EXE:-$DIR/../../src/stockfish}

UPDATE=no
[ "${1:-}" = "--update" ] && UPDATE=yes

# Number of positions of each set kept in the sample
SAMPLE_STANDARD=4000
SAMPLE_CHESS960=1000

if [ ! -x "$EXE" ]; then
   echo "No engine at $EXE, build it first (cd src && make -j2 build) or set EXE."
   exit 1
fi

mkdir -p "$DATA/downloads"

# 1. Download the dumps listed in SHA256SUMS, unless already there, and check them
while read -r _ file; do
   name=$(basename "$file")
   case $name in
      *chess960*) url=https://database.lichess.org/chess960/$name ;;
      *) url=https://database.lichess.org/standard/$name ;;
   esac
   [ -f "$DATA/$file" ] || curl -sSfL -o "$DATA/$file" "$url"
done < "$DIR/SHA256SUMS"
(cd "$DATA" && sha256sum -c "$DIR/SHA256SUMS")

# 2. Extract the positions and dump them
build_set()
{
  name=$1 dump=$2
  shift 2

  zstd -dc "$DATA/downloads/$dump" | "$PYTHON" "$DIR/extract_positions.py" "$@" > "$DATA/$name.candidates.fen"
  "$PYTHON" "$DIR/differential.py" "$@" canonicalize "$DATA/$name.candidates.fen" "$DATA/$name.fen.zst"
  rm "$DATA/$name.candidates.fen"

  "$PYTHON" "$DIR/differential.py" "$@" generate "$DATA/$name.fen.zst" "$DATA/$name.jsonl.zst"
}

build_set standard lichess_db_standard_rated_2013-01.pgn.zst
build_set chess960 lichess_db_chess960_rated_2014-01.pgn.zst --chess960

# 3. Compare what was built with what is recorded in git
for f in standard.fen standard.jsonl chess960.fen chess960.jsonl; do
   echo "$(zstd -dc "$DATA/$f.zst" | sha256sum | cut -d' ' -f1)  $f"
done > "$DATA/CONTENT.sha256"

if diff "$DIR/CONTENT.sha256" "$DATA/CONTENT.sha256"; then
   echo "The corpus and the golden files are the recorded ones."
elif [ $UPDATE = yes ]; then
   cp "$DATA/CONTENT.sha256" "$DIR/CONTENT.sha256"
   echo "CONTENT.sha256 updated."
else
   echo "The corpus or the golden files differ from the ones recorded in CONTENT.sha256."
   echo "If the engine or its network changed on purpose, run again with --update."
   exit 1
fi

# 4. Refresh the sample kept in git
mkdir -p "$DIR/sample"
sample_set()
{
  "$PYTHON" "$DIR/differential.py" sample "$DATA/$1.fen.zst" "$DATA/$1.jsonl.zst" "$2" \
     "$DIR/sample/$1.fen.gz" "$DIR/sample/$1.jsonl.gz"
}
sample_set standard $SAMPLE_STANDARD
sample_set chess960 $SAMPLE_CHESS960

du -sh --apparent-size "$DATA"
