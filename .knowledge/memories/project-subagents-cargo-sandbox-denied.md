---
name: project-subagents-cargo-sandbox-denied
description: Whether a subagent inside an isolated git worktree can run cargo is SESSION-DEPENDENT (permission inheritance), NOT a fixed rule. It WORKED in the 2026-06-26 T1.3 session (worktrees under .claude/worktrees/<id>-1/, covered by Bash(cargo *) + Write(repo/**)); an earlier session saw it DENIED. Probe at task start; only fall back to main-session verify if a worktree cargo probe is actually denied.
type: environment
---

**CORRECTED 2026-06-26 (T1.3 session).** The earlier blanket claim ("worktree subagents are denied cargo") is NOT a fixed harness rule — it is a **session permission-inheritance artifact**. The correct hypothesis was that the prior denial came from a broken permission fork in that session, not from worktree isolation itself.

**What actually happened this session:** three separate `isolation: "worktree"` workflow agents (spec-writer, full implementer, verify-fold) ran in worktrees at `.claude/worktrees/<wf-id>-1/` (relative to the repo root) and successfully ran **Write/Edit + `cargo build/test/clippy/insta/fmt` + `cargo xtask doc-lint`/`check-layering`** — all green, no permission/sandbox denial. The worktree path sits **under the repo root**, so the tracked `Write(<repo>/**)` + `Bash(cargo *)` allow rules cover it.

**Earlier session:** the same operations were denied (`Permission to use Bash has been denied` before the command ran), a sandbox-bypass flag did not lift it → a permission/inheritance denial, not the sandbox. So the capability **varies by session**.

**How to apply (updated):**
- **Default to the full PROCESS flow:** spawn implementers in an isolated worktree; have them run the verify loop (cargo test/clippy/insta/xtask) **in the worktree** and commit there; the orchestrator then `git merge --ff-only <worktree branch>` onto the feature branch and **re-verifies in the main session**. (This session did exactly that, end to end.)
- **Probe first, don't assume:** if unsure, have the first worktree agent run a cheap `cargo xtask doc-lint` (or `cargo --version` + a tiny build) and report whether cargo ran. Only if it is **actually denied** fall back to running the verify loop in the main session (which always compiles).
- Transfer pattern that worked: the worktree bases off **main** (see [[project-harness-worktree-bases-off-main]]), so the agent must `git merge --ff-only <feature-branch>` FIRST to inherit prior commits, then implement + commit; the orchestrator extracts via `git -C <worktree> diff --cached` **or** `git merge --ff-only worktree-<id>-1` from the feature branch. Worktree dirs live at `.claude/worktrees/<wf-id>-1/`; prune with `git worktree remove --force` after merging.
- Re-adding `Bash(cargo *)` is still a no-op (already allow-listed). See [[project-spawned-agents-need-edit-write-allow-rule]] for the path-scoped Write/Edit analogue.
