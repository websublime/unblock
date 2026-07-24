---
name: feedback-parallel-verify-agents-share-one-worktree-clobber
description: Parallel revert-based Verify agents pointed at ONE isolated worktree clobber each other's mutations → spurious RED; give each its own throwaway worktree
type: gotcha
---

When ≥2 Verify agents run in parallel and each does **revert-the-fix → observe RED → restore** non-vacuity
testing, DO NOT point them all at the same isolated implementer worktree — they mutate the same source files
concurrently and clobber each other. Seen on the no-network hardening (PR #418, 2026-07-16): one lens injected
`return None` into `skip_char_literal` while another was running `cargo test` on the same worktree → a spurious
`test failed` RED that had nothing to do with the artifact. The careful lens caught it by md5-guarding the file
before AND after each run (only accepting results where source == the canonical `impl-out` bytes).

**Why:** the earlier warning [[feedback-verify-reviewers-mutate-shared-tree]] is about the *shared* checkout;
this is a *different* hazard — multiple agents sharing ONE isolated worktree still race each other.

**How to apply:** (1) instruct each parallel Verify agent to create its **own throwaway** `git worktree add`
(pinned to the REV commit or `main`) for any mutation/revert testing, and `git worktree remove` it after —
leave the canonical worktree + the scratchpad `impl-out*` files untouched. (2) Keep the canonical verified
artifact in a **scratchpad `impl-out/` copy** (not just the worktree) and md5-pin it; the orchestrator applies
from that copy, not from the mutated worktree. (3) As orchestrator, always re-run the **authoritative full probe
in the main tree** after applying — treat any agent "RED/hole" finding as needing independent reproduction first
(a shared-worktree race can fabricate it). Related: [[project-sendmessage-resume-runs-in-shared-tree]],
[[feedback-agent-tool-needs-explicit-worktree-isolation]].
