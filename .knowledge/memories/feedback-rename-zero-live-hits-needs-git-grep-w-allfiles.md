---
name: feedback-rename-zero-live-hits-needs-git-grep-w-allfiles
description: For a command/name-surface rename, the zero-live-hits attestation MUST be `git grep -nw <token>` over ALL files (incl dotfiles/config), never `rg` scoped to *.rs — rg skips hidden dirs and *.rs misses mcp.json/ci.yml/READMEs/crate-plans
type: gotcha
---

On the T3.4.2 `serve`→`mcp` rename (D32), the Verify gate FAILED twice on the SAME completeness class: the implementer's "zero-live-hits" attestation used `rg` scoped to `*.rs`, which **repeatedly under-counted** and let residuals through — including a **functional regression**: `mcp.json` (the repo-root MCP launch config) still invoked `unblock serve`, which the renamed clap surface rejects with "unrecognized subcommand" (exit 2).

**Why the greps kept missing (both are real traps):**
1. **`rg` skips hidden directories by default** → it never scanned `.github/` (missed `ci.yml` concept-noun comments). Needs `--hidden`.
2. **Scoping to `*.rs`** misses every non-Rust surface that carries the command name: `mcp.json` launch configs, `crates/README.md`, `docs/plans/crates/*.md` crate-plan prose, `Cargo.toml`/`ci.yml` comments.
3. Literal-phrase greps (`serve/migrate`, `single-serve`) under-count vs. the whole-word form.

**The definitive recipe (use this to attest a rename is complete):**
`git grep -nw <token> <branch> -- '*'` (and the capitalized identifier form `git grep -nw Serve`). `git grep` searches tracked dotfiles by default (unlike rg); `-w` word-boundary cleanly matches bare `serve` while EXCLUDING `server`/`serve_with_ct`/`serve_duplex` (`_` is a word char). Then confirm every residual is inside the enumerated STAYS set (rmcp `serve-loop`/`serve_with_ct`/`.serve(`; frozen STATUS history + completed rows; the migration's OWN docs — the D-row, the new task rows, the change-log entry).

**How to apply:** When an Implement/Verify agent claims "zero live hits" for a rename, do NOT trust it — the orchestrator runs `git grep -nw` over the BRANCH (read-only, no checkout: `git grep <pattern> <branch>`) itself to establish the authoritative residual set BEFORE looping another full cycle. Also: the FINAL-SPEC decision-change-checklist edit list omitted `mcp.json`/`crates/README.md`/`unblock-render.md`/`ci.yml` — for a rename, seed the checklist from a repo-wide `git grep -nw`, not from a hand-curated file list. Related: [[feedback-implementer-probe-must-include-cargo-fmt]], [[feedback-macos-probe-masks-linux-ci-path-confinement]].
