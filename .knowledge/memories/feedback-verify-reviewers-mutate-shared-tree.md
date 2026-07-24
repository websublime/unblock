---
name: feedback-verify-reviewers-mutate-shared-tree
description: Verify-gate reviewers reading an uncommitted change in the SHARED tree may mutate source in place (mutation-testing) despite read-only instructions; hand off via a scratchpad patch and reset+re-apply before commit
type: gotcha
---

When a Verify/Review workflow's reviewer agents are pointed at an **uncommitted** change living in the **shared working tree** (main tree on a feature branch), they may **edit source files in place** — e.g. mutation-testing the fix (revert the production change, run the tests, confirm RED, revert back) to prove non-vacuity — **even when told "STRICT read-only, do NOT edit repo files."** Observed in T1.6 (PR #382): a reviewer cycled `// EXPERIMENT:` regressions of the cycle-detection fix into `deps.rs`/`crud.rs` then reverted; the tree happened to settle to canonical, but it was left in flux mid-gate.

**Why it matters:** a reviewer that crashes/aborts mid-mutation (or whose revert is imperfect) would leave a corrupted or experimentally-regressed tree that then gets committed — silently shipping the wrong bytes on a data-integrity path.

**How to apply (the guard that worked):**
1. Have the Implement workflow save the verified change as a **patch in the session scratchpad** (`git add -A && git diff --cached --binary > scratch/<task>-full.patch`) — the scratchpad is outside the repo, so reviewers can't touch it.
2. Before committing at Track: `git reset --hard HEAD` (discard whatever the reviewers left), **re-apply the scratchpad patch**, and **assert byte/md5 equality** of the critical files against the Verify-gate's recorded canonical hashes; then a **clean rebuild** (`cargo clean -p <crate>` to clear stale-incremental glitches) + the full cargo battery before commit.
3. Alternatively, isolate Verify reviewers in their own worktree with the patch applied (but read-only-in-shared + scratchpad-patch handoff is simpler and was sufficient).

The scratchpad-patch handoff also cleanly bridges separate workflows (Spec→Review→Implement→Verify) without fragile cross-worktree branch merges. See [[project-subagents-cargo-sandbox-denied]] and [[project-harness-worktree-bases-off-main]].
