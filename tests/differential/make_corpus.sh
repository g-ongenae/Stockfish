#!/bin/bash
# Rebuilds the corpora and their golden files from the pinned Lichess database
# dumps: downloads them, checks their checksums, extracts the positions and the
# lines of play, dumps them with the engine and refreshes the small samples kept
# in git.
#
# The dumps are never decompressed to disk. The bulk data goes to DATA (default
# tests/differential/data, not in git, about 240 MB). It takes about 10 minutes.
# The dumps are read from DATA/downloads, else from the shared folder
# ($STOCKFISH_SHARED/data/downloads, see shared.sh), and downloaded into
# DATA/downloads only when neither has them. The script never writes into the
# shared folder: a DATA inside it is refused.
#
# The corpus and the golden files built are compared with the checksums recorded
# in CONTENT.sha256. If they differ the script fails and leaves the sample alone,
# unless called with --update, which records the new ones: do that on purpose
# only, when the engine or its network changed.
#
# Needs curl, zstd, sha256sum and a Python with python-chess, pinned in
# requirements.txt. EXE selects the engine (default ../../src/stockfish), PYTHON
# the interpreter (default python3). The script never looks for one by itself:
# where the system Python lacks python-chess, use the environment of the shared
# folder rather than installing anything (README.md, "The shared folder"):
#   S=${STOCKFISH_SHARED:-$HOME/.local/share/stockfish-fork}
#   PYTHON=$S/venv/bin/python EXE=$S/oracle/stockfish DATA=<not in $S> make_corpus.sh

set -euo pipefail

DIR=$(cd "$(dirname "$0")" && pwd)
. "$DIR/shared.sh"
DATA=${DATA:-$DIR/data}
PYTHON=${PYTHON:-python3}
export EXE=${EXE:-$DIR/../../src/stockfish}
refuse_shared_output "$DATA"

UPDATE=no
[ "${1:-}" = "--update" ] && UPDATE=yes

# Number of positions, and of lines of play, of each set kept in the sample
SAMPLE_STANDARD=4000
SAMPLE_CHESS960=1000
SAMPLE_LINES_STANDARD=100
SAMPLE_LINES_CHESS960=50

if [ ! -x "$EXE" ]; then
   echo "No engine at $EXE, build it first (cd src && make -j2 build) or set EXE."
   exit 1
fi

if ! "$PYTHON" -c 'import chess' 2> /dev/null; then
   echo "$PYTHON cannot import python-chess (chess==1.11.2, see requirements.txt)."
   echo "Set PYTHON to an interpreter that has it, such as the shared folder's:"
   echo "  PYTHON=${STOCKFISH_SHARED:-<the shared folder>}/venv/bin/python"
   echo "Elsewhere, make an environment outside the checkout from requirements.txt;"
   echo "do not install it into the system Python or a checkout (README.md, \"The shared folder\")."
   exit 1
fi

mkdir -p "$DATA/downloads"
echo "make_corpus.sh: engine $EXE; output in $DATA"

# Where a dump is: in DATA, else in the shared folder, else to be downloaded into DATA
dump_path()
{
  if [ ! -f "$DATA/downloads/$1" ] && [ -n "$SHARED_DATA" ] && [ -f "$SHARED_DATA/downloads/$1" ]; then
    echo "$SHARED_DATA/downloads/$1"
  else
    echo "$DATA/downloads/$1"
  fi
}

# 1. Download the dumps listed in SHA256SUMS, unless already there, and check them
while read -r sum file; do
   name=$(basename "$file")
   case $name in
      *chess960*) url=https://database.lichess.org/chess960/$name ;;
      *) url=https://database.lichess.org/standard/$name ;;
   esac
   path=$(dump_path "$name")
   [ -f "$path" ] || curl -sSfL -o "$path" "$url"
   echo "$sum  $path" | sha256sum -c
done < "$DIR/SHA256SUMS"

# 2. Extract the positions and dump them
build_set()
{
  name=$1 dump=$2
  shift 2

  zstd -dc "$(dump_path "$dump")" | "$PYTHON" "$DIR/extract_positions.py" "$@" > "$DATA/$name.candidates.fen"
  "$PYTHON" "$DIR/differential.py" "$@" canonicalize "$DATA/$name.candidates.fen" "$DATA/$name.fen.zst"
  rm "$DATA/$name.candidates.fen"

  "$PYTHON" "$DIR/differential.py" "$@" generate "$DATA/$name.fen.zst" "$DATA/$name.jsonl.zst"
}

build_set standard lichess_db_standard_rated_2013-01.pgn.zst
build_set chess960 lichess_db_chess960_rated_2014-01.pgn.zst --chess960

# The lines of play are written as extracted, they need no canonical form
build_lines()
{
  name=$1 dump=$2
  shift 2

  zstd -dc "$(dump_path "$dump")" | "$PYTHON" "$DIR/extract_positions.py" --lines "$@" \
     | zstd -q -19 > "$DATA/$name.lines.zst"
  "$PYTHON" "$DIR/differential.py" --lines "$@" generate "$DATA/$name.lines.zst" "$DATA/$name.lines.jsonl.zst"
}

build_lines standard lichess_db_standard_rated_2013-01.pgn.zst
build_lines chess960 lichess_db_chess960_rated_2014-01.pgn.zst --chess960

# 3. Compare what was built with what is recorded in git
for f in standard.fen standard.jsonl chess960.fen chess960.jsonl \
         standard.lines standard.lines.jsonl chess960.lines chess960.lines.jsonl; do
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

sample_lines()
{
  "$PYTHON" "$DIR/differential.py" --lines sample "$DATA/$1.lines.zst" "$DATA/$1.lines.jsonl.zst" "$2" \
     "$DIR/sample/$1.lines.gz" "$DIR/sample/$1.lines.jsonl.gz"
}
sample_lines standard $SAMPLE_LINES_STANDARD
sample_lines chess960 $SAMPLE_LINES_CHESS960

du -sh --apparent-size "$DATA"
