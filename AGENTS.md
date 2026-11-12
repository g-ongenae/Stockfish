# Instructions for coding agents

## Push and open pull requests on the fork only

This repository is a fork of Stockfish: `g-ongenae/Stockfish`. The official
repository is `official-stockfish/Stockfish`.

- Never push to `official-stockfish/Stockfish`.
- Never open a pull request, an issue or a comment on
  `official-stockfish/Stockfish`.
- Push branches to `g-ongenae/Stockfish` only, and open pull requests there,
  against its `master`.

Two traps make this easy to get wrong:

- In a local checkout, the remote named `origin` may be the official
  repository, with a working push URL. Run `git remote -v` before any push and
  push to the remote that points at `g-ongenae/Stockfish` (usually named
  `fork`). Never run a bare `git push`.
- On a fork, `gh pr create` targets the parent repository by default. Always
  name the repository:

  ```
  gh pr create --repo g-ongenae/Stockfish --base master --head <branch>
  ```

  Afterwards, check that the pull request URL starts with
  `https://github.com/g-ongenae/Stockfish/`.

The same applies to every other `gh` command that writes (`gh pr comment`,
`gh issue create`, `gh pr merge`, ...): pass `--repo g-ongenae/Stockfish`.

## Golden data and oracle: use the shared folder, never copy it

The full corpora and golden files of `tests/differential` (260 MB), the Lichess
dumps they come from, the reference binaries (`stockfish`, `stockfish-trace`)
and the network (`nn-18cd32e11a9c.nnue`, 138 MB) exist once per machine, outside
every checkout, in the folder named by `STOCKFISH_SHARED` (default
`~/.local/share/stockfish-fork`). `oracle/MANIFEST` there says how the binaries
were built and checked. On another machine, set `STOCKFISH_SHARED` or build the
data in-tree as `tests/differential/README.md` describes.

Unless the user asks for it:

- Do not copy the data, the binaries or the network into a worktree, and do
  not create `tests/differential/data` there.
- Do not download the network again (`make net` without a link, see below)
  and do not rebuild the corpora (`make_corpus.sh`, `make_trace_corpus.sh`).
- Do not write into the shared folder. It is read-only on disk; the scripts
  refuse it as an output.

How a fresh worktree uses it:

```
S=${STOCKFISH_SHARED:-$HOME/.local/share/stockfish-fork}

# the full gates of the C++ engine: EXE names the binary checked, the golden
# files are found in the shared folder; verify.sh prints both
EXE=$S/oracle/stockfish       tests/differential/verify.sh full
EXE=$S/oracle/stockfish       tests/differential/verify.sh lines-full
EXE=$S/oracle/stockfish-trace tests/differential/verify.sh trace-full
EXE=$S/oracle/stockfish       tests/differential/verify.sh trace-uci-full

# building the C++ engine with the shared network instead of a download
ln -s "$S/oracle/nn-18cd32e11a9c.nnue" src/ && make -C src -j2 build

# the Rust port: the network next to its binary, EXE always set to the port
cd rust && cargo build --release -j2 && ln -sf "$S/oracle/nn-18cd32e11a9c.nnue" target/release/
. ./differential.env
EXE=$PWD/target/release/stockfish ../tests/differential/verify.sh full --fields $DUMP_FIELDS
EXE=$PWD/target/release/stockfish ../tests/differential/verify.sh lines-full --fields $DUMPLINE_FIELDS
```

Always set `EXE` for a full gate: without it `verify.sh` checks the
checkout's `src/stockfish`, never the shared one, and a gate of the port run
without `EXE` would check the C++ engine instead.
