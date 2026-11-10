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
