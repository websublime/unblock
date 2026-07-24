---
name: reference-zsh-glob-nomatch-aborts-ls-existence-check
description: A zsh `ls A* B*` aborts entirely if ANY glob has no match → a "does this root file exist?" check silently runs zero commands and returns nothing; verify with git ls-files / find, not a multi-glob ls
type: gotcha
---

Under zsh (the default shell here), `ls LICENSE* CONTRIBUTING* README*` **aborts the whole
command** the moment ANY single glob has zero matches — zsh's `nomatch` errors out
pre-exec (`no matches found: CONTRIBUTING*`) and `ls` never runs, so NONE of the other
patterns are listed. `2>/dev/null` does **not** save you (the abort is the shell's, before
`ls` starts).

**Consequence seen in T3.7:** a session concluded the root `LICENSE-MIT`/`LICENSE-APACHE` were
missing (they existed), asked Miguel a question on that false premise, and `cp`-overwrote a
pre-existing `LICENSE-APACHE` that had not been created this session (reverted with `git checkout HEAD -- LICENSE-APACHE`).

**Rule:** to check whether a tracked file exists, use `git ls-files | grep -i '^LICENSE'` or
`git cat-file -e HEAD:<path>` or `find . -maxdepth 1 -iname 'LICENSE*'` — NEVER a multi-glob
`ls A* B*`. And before overwriting any file you did not create (`cp`/`>`), inspect the target
first (git status shows `M` vs `??`; `git diff --cached` before commit) — see the "look at the
target before overwriting" safety rule. Ties to [[feedback-macos-probe-masks-linux-ci-path-confinement]]
(another shell-portability trap).
