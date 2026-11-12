# Sourced by verify.sh, make_corpus.sh and make_trace_corpus.sh: where the full
# corpora and golden files are looked for when they are not in this checkout.
#
# STOCKFISH_SHARED is a folder outside every checkout holding one copy of them
# (data/) and of the reference binaries and network (oracle/), so that a new
# worktree needs no copy, download or rebuild (see README.md, "The shared
# folder"). It is only ever read: verify.sh reads golden files from it when the
# checkout has none, the corpus scripts read their inputs from it, and none of
# them writes into it.

STOCKFISH_SHARED=${STOCKFISH_SHARED:-${HOME:-}/.local/share/stockfish-fork}
SHARED_DATA=$STOCKFISH_SHARED/data

# resolved PATH: PATH with its existing part resolved (symbolic links, ..),
# whether or not it exists yet
resolved()
{
  local dir=$1 rest=
  while [ ! -d "$dir" ]; do
    rest=/$(basename "$dir")$rest
    dir=$(dirname "$dir")
  done
  echo "$(cd "$dir" && pwd -P)$rest"
}

# refuse_shared_output DIR: stops a script that would write anywhere in the
# shared folder
refuse_shared_output()
{
  [ -d "$STOCKFISH_SHARED" ] || return 0
  case "$(resolved "$1")/" in
    "$(cd "$STOCKFISH_SHARED" && pwd -P)"/*)
      echo "Refusing to write into $1: it is in the shared folder $STOCKFISH_SHARED, which holds the reference golden files."
      echo "Build into another folder (DATA=...), compare, and replace them by hand if that is really meant."
      exit 1 ;;
  esac
}
