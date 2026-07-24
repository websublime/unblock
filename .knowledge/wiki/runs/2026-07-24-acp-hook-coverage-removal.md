---
name: 2026-07-24-acp-hook-coverage-removal
description: First live session after the knowledge-layer landing pull request — the hook-wiring canaries fired for real, branch protection was activated, and the unused ACP tool coverage was removed from the memories write-guard.
type: run
date: 2026-07-24
branch: e4s.7-acp-removal
pr: -
issues: [ub-knowledge-layer-e4s.6, ub-knowledge-layer-e4s.7]
---

# Run — hook canaries go live, branch protection activates, ACP coverage removed

## Context

This was the first live Claude Code session in this repository after the pull request that landed the
`.knowledge/` layer (the repo-public knowledge base: atomic-fact memories plus a wiki of run-reports and
topic runbooks) was merged. The session first exercised the four `.knowledge` PreToolUse hook-wiring
canaries live, for real, inside a running Claude Code session rather than as synthetic script-level
payloads (task ub-knowledge-layer-e4s.6, an unblock task id — unblock is this repo's MCP-based issue
tracker), and activated the `main` branch's protection ruleset. It then carried out this change (task
ub-knowledge-layer-e4s.7): removing Agent Client Protocol tool coverage from the knowledge-memories
write-guard hook. Branch `e4s.7-acp-removal`, worked in an isolated git worktree off `main`. Gate: pre-PR
(the design Review and Verify quality gates run after this Implement hand-off, per the process guide).

## What & why

(1) Live hook-wiring canaries. Of the four PreToolUse hooks wired by the knowledge-layer landing, three
fired live during this session and behaved exactly as designed: the memories write-guard denied a
`Write` that targeted a nested path under `.knowledge/memories/` (the guard's flat-layout rule); the
bash-guard denied a recursive `rm` reaching into the `.knowledge/` tree; and the pull-request-create gate
first denied a `gh pr create` attempt that would have failed the run-report predicate closed, then
allowed a follow-up attempt once a compliant run-report existed — a deny-then-pass pair, which is a
stronger live proof than a pass alone (a lone pass is indistinguishable from a hook that never ran). The
fourth hook path — the write-guard's fallback logic for locating the edited file's path across differing
payload key shapes (`file_path`, `path`, `abs_path`) — could not be exercised live in this terminal-based
Claude Code session because the Agent Client Protocol write tool is not surfaced by this frontend; that
path had already passed a synthetic, script-level canary at implementation time (piping a crafted payload
on stdin), which is a weaker but still real check.

(2) Branch protection. The `main` branch's ruleset was updated to require the run-report-gate status
check (the CI job that enforces the same substantive-PR predicate the local PreToolUse `pr-create` hook
enforces), verified via the GitHub API.

(3) ACP removal (this change). Miguel decided to drop Agent Client Protocol (`mcp__acp__Write` /
`mcp__acp__Edit`) tool coverage from the knowledge-memories write-guard entirely, because no Agent Client
Protocol frontend is used against this repository — the guard path that specially recognized those two
tool names can never be reached in practice, and the fourth canary named in (1) above is obviated along
with it. This run made that change across the three places that named those tools: the repo's hook
permission allowlist and PreToolUse matcher (`.claude/settings.json`), the write-guard script's tool sets
and a related comment (`scripts/hooks/knowledge-memories-write-guard.py`), and the normative hooks spec
(`docs/plans/ci-cd-and-distribution.md`, the CI/CD and distribution plan, section 2.3). Native `Write`,
`Edit`, and `MultiEdit` coverage of `.knowledge/memories/**` is unchanged and fully intact; only the two
Agent Client Protocol tool names were removed, everywhere they appeared in the enforcement surface.

## Outcome

Task ub-knowledge-layer-e4s.6 (the live hook-wiring canary session plus branch-protection activation) is
closed. This change (task ub-knowledge-layer-e4s.7) edits three files — `.claude/settings.json`,
`scripts/hooks/knowledge-memories-write-guard.py`, `docs/plans/ci-cd-and-distribution.md` — to remove
every reference to the two Agent Client Protocol tool names, with the spec (the CI/CD and distribution
plan, section 2.3) updated in the same change as the code and hook-config edits, plus this run-report. A
`git grep` for the removed tool names across the three edited files returns nothing. Local verification
(JSON parse of the settings file, the doc-lint over the grown corpus, and script-level probes of the
edited write-guard covering existing-file overwrite denial, new-file allow, `Edit` allow, nested-path
denial, and the now-unrecognized Agent Client Protocol tool name falling through to allow) is reported
verbatim to the orchestrator alongside this report; the design Review and Verify quality gates, and the
`cargo xtask knowledge-lint` run against the tracker's git-exported record, remain to be run by the
orchestrator on the merged state (this worktree's local `.unblock/issues.jsonl` snapshot predates both
of these tasks, so a same-worktree `knowledge-lint` run cannot resolve their ids until the orchestrator's
next tracker re-export lands).

## Gotchas

- Agent Client Protocol (commonly abbreviated ACP in this repo's hook code and specs) is a frontend
  integration protocol for desktop apps and IDE bridges, distinct from the terminal-based Claude Code
  session used here; its write/edit tools are simply not present in this environment, which is exactly
  why its coverage was safe to remove and why its canary could only ever be checked at the script level,
  never live, in a terminal session.
- A hook **deny** is a strong live proof that the wiring works; a hook **pass** alone is not, because it
  is indistinguishable from a hook that silently failed to fire at all. The deny-then-pass pair on the
  pull-request-create gate is the more convincing of the two live proofs recorded this session.
- The run-report gate (the enforcement predicate requiring a wiki run-report on every substantive pull
  request, implemented in `scripts/knowledge/run-report-gate.sh` and mirrored by the CI status check) is
  why this change carries a run-report even though it is a small, mechanical, three-file removal — the
  gate's substantive-PR predicate does not carve out an exception for small diffs.
- The write-guard's three-key path fallback (`file_path`, `path`, `abs_path`) was kept exactly as-is;
  only the comment above it was reworded to be frontend-generic, since other, non-Agent-Client-Protocol
  frontends could plausibly vary which key they populate.
- This worktree's committed `.unblock/issues.jsonl` (the tracker's git-backed export) is a snapshot taken
  before tasks ub-knowledge-layer-e4s.6 and ub-knowledge-layer-e4s.7 were created in the live tracker, so
  a `cargo xtask knowledge-lint` run against this worktree's own file cannot resolve either id yet; the
  orchestrator holds the live tracker state and re-exports it in its own commit.

## Glossary

No session-local id codes (of the kind this repo's knowledge-layer glossary rule targets — short
mutation-testing or must-fix labels such as an uppercase letter immediately followed by a number) were
used anywhere in this report. The table below is provided anyway, as instructed, to record the durable,
non-session-local references this report leans on and to satisfy the glossary section's data-row
requirement.

| id | what it is (in words) | where it lives (file:line / doc § / issue id) |
|----|-----------------------|-------------------------------------------------|
| ub-knowledge-layer-e4s.6 | the unblock task for this session's live hook-wiring canaries plus branch-protection activation | the unblock tracker (MCP); its git-exported record is `.unblock/issues.jsonl` |
| ub-knowledge-layer-e4s.7 | the unblock task for this change — removing Agent Client Protocol tool coverage from the memories write-guard | the unblock tracker (MCP); its git-exported record is `.unblock/issues.jsonl` |
| run-report gate | the enforcement predicate requiring a wiki run-report on every substantive pull request | `scripts/knowledge/run-report-gate.sh`; `docs/plans/ci-cd-and-distribution.md` section 2.3; mirrored as a required CI status check |
| ACP | Agent Client Protocol — a frontend integration protocol (desktop apps / IDE bridges); not used against this repository, hence this change | referenced (pre-removal) at `.claude/settings.json`, `scripts/hooks/knowledge-memories-write-guard.py`, `docs/plans/ci-cd-and-distribution.md` section 2.3 |
| PR #428 | the pull request that landed the `.knowledge/` layer (scaffold, lint, gate, hooks, and its first run-report) | GitHub pull request #428 against this repository |

## Links

- ub-knowledge-layer-e4s.6 — the unblock task for this session's live hook-wiring canaries and the
  branch-protection activation; closed as part of this session.
- ub-knowledge-layer-e4s.7 — the unblock task for this change (Agent Client Protocol coverage removal);
  tracked in this run-report and the accompanying commit.
- `docs/plans/ci-cd-and-distribution.md` section 2.3 — the normative hooks spec, updated in the same
  change as the code and config edits.
- `.claude/settings.json`, `scripts/hooks/knowledge-memories-write-guard.py` — the two enforcement
  surfaces edited alongside the spec.
- Pull request: opened by the orchestrator after this Implement hand-off; not yet created at report time.
- Prior related run-report: `.knowledge/wiki/runs/2026-07-23-knowledge-layer-landing.md` (the knowledge
  layer's inaugural landing report).
