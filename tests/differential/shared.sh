# Sourced by verify.sh, make_corpus.sh and make_trace_corpus.sh: where the full
# corpora and golden files are looked for when they are not in this checkout.
#
# STOCKFISH_SHARED is a folder outside every checkout holding one copy of them
# (data/) and of the reference binaries and network (oracle/), so that a new
# worktree needs no copy, download or rebuild (see README.md, "The shared
# folder"). It is only ever read: verify.sh reads golden files from it when the
# checkout has none, the corpus scripts read their inputs from it, and none of
# them writes into it. With neither STOCKFISH_SHARED nor HOME set there is no
# shared folder.

if [ -z "${STOCKFISH_SHARED:-}" ] && [ -n "${HOME:-}" ]; then
  STOCKFISH_SHARED=$HOME/.local/share/stockfish-fork
fi
STOCKFISH_SHARED=${STOCKFISH_SHARED:-}
SHARED_DATA=${STOCKFISH_SHARED:+$STOCKFISH_SHARED/data}

# refuse_shared_output DIR: stops a script that is to write into DIR when
# - DIR, its symbolic links and '..' resolved as the system will resolve them
#   (os.path.realpath, existing or not), is the shared folder or inside it,
#   whether the shared folder exists or not;
# - or DIR already holds a symbolic link (DIR itself may be one), which a write
#   could follow anywhere, the shared folder included.
# The shared folder is also read-only on disk, which is what protects it from
# anything else.
refuse_shared_output()
{
  [ -n "$STOCKFISH_SHARED" ] || return 0
  local python=${PYTHON:-python3} inside
  if ! inside=$("$python" -c 'import os, sys
out, root = (os.path.realpath(p) for p in sys.argv[1:])
print("yes" if out == root or out.startswith(root.rstrip(os.sep) + os.sep) else "no")' "$1" "$STOCKFISH_SHARED"); then
    echo "Cannot tell whether $1 is in the shared folder: $python failed."
    exit 1
  fi
  if [ "$inside" = yes ]; then
    echo "Refusing to write into $1: it is in the shared folder $STOCKFISH_SHARED, which holds the reference golden files."
    echo "Build into another folder (DATA=...), compare, and replace them by hand if that is really meant."
    exit 1
  fi
  if [ -d "$1" ] && [ -n "$(find -H "$1" -type l | head -n 1)" ]; then
    echo "Refusing to write into $1: it holds symbolic links, which a write could follow out of it:"
    find -H "$1" -type l | head -n 5
    exit 1
  fi
}
