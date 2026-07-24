---
name: project-workspace-discovery-claude-project-dir-d39
description: D39/T3.10 workspace-discovery fix — adopt CLAUDE_PROJECT_DIR + walk-up guard + startup visibility; README two-config-worlds fix; GA-train
type: reference
---

**D39/T3.10 (2026-07-17).** Miguel challenged the README telling users to register the MCP server with an
absolute `--dir` path in a committed config (every dev rewrites it → git clobber war). Investigation
(4-agent workflow) found:

- **README conflates two config worlds.** Absolute `--dir` only fits a USER-GLOBAL client config
  (Claude Desktop, `~/Library/Application Support/Claude/...`, per-user, never committed). A
  PROJECT-SCOPED committed config (`.mcp.json` at repo root, `.vscode/mcp.json`, `.cursor/mcp.json`)
  must NOT hardcode an absolute path. The repo's own committed `mcp.json` already passes NO `--dir`
  (relies on walk-up) — proving the README's "recommended" advice wrong.
- **MCP `roots` capability is DEPRECATED** (SEP-2577, Final, protocol 2026-07-28) — new impls SHOULD NOT
  adopt. Rejected as the mechanism (an initial instinct, wrong).
- **The real seam: `CLAUDE_PROJECT_DIR`.** Claude Code injects it into the spawned stdio child's ENV
  = project root. `unblock` currently discards it. Trap: a literal `${CLAUDE_PROJECT_DIR}` in `.mcp.json`
  args does NOT expand (it's in the child env, not Claude Code's) → `unblock` must read it from its OWN
  process env at startup. VS Code has `${workspaceFolder}`+`cwd` field; Cursor `${workspaceFolder}`.
- **Integrity bug: walk-up is UNGUARDED to `/`** (`crates/unblock-config/src/discovery.rs:129-156`, no
  git-root/$HOME/depth stop). Claude Code desktop app spawns child with `cwd=$HOME`; a `$HOME/.unblock`
  from an old init → server binds the WRONG DB silently. Not just docs — a data-integrity defect.
- Minor: `EnvOverrides.dir` (`env.rs:67`) is parsed but never consumed (dead code); spine never defines
  walk-up start = cwd (spec gap); NO D-id covers discovery at all.

**Miguel decided (3 forks):** (1) adopt CLAUDE_PROJECT_DIR in v1 — precedence `--dir/--db > UNBLOCK_DIR
> CLAUDE_PROJECT_DIR > cwd walk-up` (additive, NOT a D35 semver break); (2) walk-up **guard + startup
visibility** (bound the ascent at $HOME and/or `.git` filesystem existence check — NOT a git op, D13 holds
— AND always report the bound dir to stderr, NFR-14); (3) lands BEFORE GA on the **D37/T3.9 train** (v1.0.0
tag already HELD by D37).

**Ids:** the decision minted as **D39** (D38 = the FR-17 signal-exit fix, landed on its own branch). Task
**T3.10**. Branch off `main`.

**Design-Review gate DONE (2026-07-17, 3 adversarial reviewers + coordinator): all 3 drafts GO-with-must-fixes,
none REDO.** Drafts in scratchpad `spec-out/{spine-and-impl,prd-and-track,readme}.md`.

**Miguel RESOLVED all 4 forks (recommended options):**
- **FORK 1 (CLAUDE_PROJECT_DIR) = ROOT.** Probe its `.unblock`/`_unblock` child, NO walk-up; on a miss fall
  through to the guarded cwd walk-up (this subsumes the old F2). Never ascends above the host-declared root.
- **FORK 3 (guard boundary) = BOTH {`.git` dir, `$HOME`}, INCLUSIVE** (probe the boundary dir before stopping,
  so a repo-root or deliberate `$HOME/.unblock` stays usable). Bound-at-GA = yes.
- **FORK 4 (startup visibility) = PATH + SOURCE tier.** Requires a DELIBERATE additive `source: WorkspaceSource`
  field on the spine-normative `WorkspaceContext`/`ResolvedContext` (spine :1271-1287) + engine destructure ripple
  — must edit the spine struct block, NOT an incidental change.
- **FORK 5 (markers) = `.git` only in v1** (`.hg`/`.svn`/`.jj` additive later).

**Must-fixes to fold into implement (from the gate):** (1) README draft must NOT regress contract id — main is
now `unblock.mcp.v1.5`/8 tools (D37/T3.9 MERGED, PR #422); narrow the README rewrite to lines 104-128, keep
:130-134 verbatim. (2) Precedence FRAMING: `--dir`/`UNBLOCK_DIR` are ONE slot (clap `env=`), and `--db` derives
BEFORE `--dir` → state `--db` (derive) > `--dir`/`UNBLOCK_DIR` (one slot) > CLAUDE_PROJECT_DIR > guarded cwd walk.
(3) `discover_unblock_dir` is `pub` + re-exported (lib.rs:72), not "private"; 3-arg `env` ripple hits tests/discovery.rs
(7 calls) + ~10 in-module tests — migrate all to `MapEnv` (NFR-16). (4) PRD §12/§13/§14 inventory: §13 M3 GA-tag row
(PRD:366) — D39 co-holds v1.0.0; clear §12 Resolved + §14 risk. (5) `EnvOverrides.dir` delete = flagged cleanup rider.

**D-id SEQUENCING (real):** D38 is on the in-flight `t3.2.1-fr17-signal-exit-hang` branch, NOT yet on origin/main
(main = D1..D37 after D37 merge). Doc-lint is data-driven on the PRD §4 row set → a D39 branch WITHOUT a D38 row
can't cleanly bump "D1..D38"→"D1..D39" (unresolved D38 ref). So the impl worktree MUST base off a tree containing
D38 (ff-merge the t3.2.1 branch, per [[project-harness-worktree-bases-off-main]]), OR wait for D38 to merge. Miguel
tacitly approved reserving D38 for the in-flight branch and cutting this as D39/T3.10.

**Implement DONE** (single implementer, worktree, 2 commits on `t3.10-workspace-discovery` off D38 tip `b5745c4`):
`c961bde` docs cascade + `9ae693f` feat. Faithful to all 4 forks; `source` flows end-to-end (verified non-vacuous);
guard inclusive; CLAUDE.md updated D1..D39. One sanctioned deviation: kept `discover_unblock_dir -> PathBuf` public
(preserves ~17 call-sites), carries `source` on an internal `discover_workspace -> DiscoveredWorkspace`.

**Verify gate PASS** (3 reviewers + coordinator, each in own isolated worktree): non-vacuity confirmed (3 mutations
all RED), full 6-gate CI probe green (fmt/clippy/test/insta-no-rebless/doc-lint 19·6/check-layering), every AC has an
executing test. Coordinator caught that the docs reviewer (project-manager, NO Bash) reviewed the DRAFTS not the
commit and its "blockers" were all FALSE-in-commit — see [[reference-project-manager-agent-has-no-bash]].

Finalization rebased the 2 D39 commits onto `rc.4` main (D38/PR#423 merged + the rc.4 bump — a clean cherry-pick)
and folded a host-env-bleed flake fix: scrub `CLAUDE_PROJECT_DIR`/`UNBLOCK_DIR` in the e2e spawn helper, since the
dogfooded repo has a `.unblock/` and those vars leaking in flips the `(via walk-up from cwd)` assertion RED. Also
folded a `db>dir` precedence test, CLAUDE_PROJECT_DIR self-recognition symmetry (accept a value pointing AT a
`.unblock` dir, mirroring `resolve_explicit_dir`), and a stale `wait_for_stderr` doc fix.

**✅ MERGED & CLOSED — [PR #424](https://github.com/websublime/unblock/pull/424) MERGED into main (merge commit
`73973f2`, 2026-07-17).** Local main synced to origin/main; all session worktrees + branches pruned; leak-clean.
Both gates passed (design Review + Verify PASS, 3 mutations RED, full CI probe green).

**Fully closed.** STATUS T3.10 row flipped `☐→☑ done` and committed+pushed DIRECT to main (`93ac363`, 2026-07-17)
— Miguel explicitly authorized the direct-to-main commit this time (overrides the usual no-direct-to-main norm).
Stale `t3.7-readme` local branch deleted. Repo: main in-sync with origin, tree clean, 1 worktree, T3.9 also ☑.
Doc-lint gotcha hit while flipping: `**PROCESS §6**` in a STATUS row trips the doc-lint class-(e) cross-ref check
(`§6 does not resolve` — PROCESS is not a recognized §-qualifier like PRD/spine); dropped the `§6` to pass.

Non-blocking follow-ups (in the PR body, deferred): §13 M3 tag-holder cell full-train cleanup covering BOTH D38+D39;
`doctor`/`migrate` emit no startup line (FORK-4 scopes it to `mcp`; the guard still closes the hazard). Optional:
prune the stale `t3.7-readme` local branch (another task's leftover, origin gone).

Relates to [[project-comments-pull-forward-v1]] (D37 GA hold), [[project-t3-6-release-pipeline-scope]] (GA).
