---
name: 2026-08-07-mcp-stdout-framing-channel
description: Moving the mcp structured error off the JSON-RPC framing channel (decision D48, tracker ub-og3) — a defect five times wider than its report, four residuals split out rather than absorbed, two design-gate failures and a Verify failure that were all the same class of defect, a mutation pass that found five unpinned claims nobody wrote down, and a race-fix that had to be pinned twice.
type: run
date: 2026-08-07
branch: ub-og3-mcp-stdout-channel
pr: '-'
issues: [ub-og3, ub-kp7, ub-b1a, ub-c5o, ub-5v5, ub-gwe]
---

# Run — the mcp stdout framing channel (D48)

## Context

Task `ub-og3` — `unblock mcp` wrote its `StructuredError` payload onto STDOUT, which on the MCP
stdio transport is the JSON-RPC framing channel, so a client parsing frames met an unparseable line
exactly where a frame belongs. Taken as the active v1.0.1 patch-slot item off `main` at 7790db0
(post-D47). Full lifecycle in one session: Understand, Decide, Spec, design Review (two failed
rounds plus an orchestrator-applied third pass under a PROCESS section 5 escalation), Implement,
Verify (one failed round plus a narrow independent recheck), Track. Teams were hand-picked per
phase — four lenses plus a coordinator for Understand and for each gate, single implementers for
the writing phases, every one in an isolated worktree.

## What & why

The issue reported one reproduction: a non-`initialize` first frame killing the server with a
non-frame blob on stdout. The orchestrator re-measured before briefing anyone, and the defect was
wider in two directions at once.

WIDER IN ROUTES. The blob was not one path but a shared terminal renderer reached by every failure
of the command. The likeliest real-world hit was the most mundane and had never been suspected: a
committed `.mcp.json` whose cwd and discovery tier both miss, so the workspace open fails BEFORE the
startup binding line is emitted — stdout carries the blob and **stderr is completely empty**. A
corrupt database, a schema newer than the build and a rejected output-format environment variable
all land the same way.

NARROWER IN CLASS THAN ITS OWN TEXT. The issue asserted that a clean non-`initialize` REQUEST in
that position is answered normally. Measured: it is not. Later, at the design gate, the orchestrator's
own correction of that was itself found wrong — a pre-`initialize` `ping` IS answered and the server
lives on to a normal handshake, which rmcp implements deliberately because the MCP lifecycle
specification permits it. The false version had by then reached three artifacts.

Specs read rather than restated: PRD section 4 (D27, D31, D35, D38, D39, D40, D43, D46, D47), NFR-14
and FR-11 in PRD section 6, the design spine sections 2.3, 2.4 and 5b, the `unblock-cli` crate plan,
and PROCESS section 3 for the decision-identity question.

Decision D48 was minted rather than an addendum: PRD and spine did not merely omit the case, they
affirmatively RULED that FR-11's always-valid-JSON-**on-stdout** rule binds the unsignalled error
path. PROCESS section 3 sends a clause reversal falling out of a new decision to that decision's own
D-id.

## Outcome

LANDED, five commits: the doc-only spec cascade with the D48 row and the D-range bump at every live
site; the exit-boundary role seam; the harness oracle pins a mutation pass proved missing; the
drain-join pin; and three corrections to claims the repair round itself introduced.

FOUR RESIDUALS SPLIT OUT rather than absorbed, each named in the decision row: `ub-kp7` (a first
frame that is neither `initialize` nor `ping` still kills the server), `ub-b1a` (the relocated
message still embeds an unbounded `Debug` rendering of client bytes — raised to high priority when
the amplification was measured), `ub-c5o` (`output::emit_report` still writes to stdout with no
classification — latent, no live path reaches it), `ub-5v5` (no response-size cap — reasoned from
source, never reproduced, and stated in that register).

GATE VERDICTS. Design Review round 1: FAIL, 15 must-fixes and 10 should-fixes. Round 2: FAIL, 4
must-fixes, two of them introduced by round 2 itself — which is why it was closed by hand under a
PROCESS section 5 escalation rather than looped a third time. Verify round 1: FAIL, 6 must-fixes.
The narrow recheck that followed confirmed all six closed and found three further false sentences
written by the repair.

MUTATION EVIDENCE. 17 named mutations applied individually, all 17 killed — including the
swapped-sink mutation, which compiles, leaves every in-process cell green, and is caught only by the
spawning end-to-end cells, a prediction that was verified rather than trusted. The mutation pass then
INVENTED more and found five survivors, every one in the shared harness oracle layer, the one layer
with no self-test of its own.

MEASURED FINAL BEHAVIOUR, orchestrator-run against the built binary: all six reachable failure routes
put zero non-frame bytes on stdout and the full structured document on stderr, newline-terminated,
with exit codes unmoved (2, 7, 7, 2, 1, 1); `ping` still answered, clean EOF still exit 0, and
`version` and `migrate` still writing their payloads to stdout.

PROCESS DEVIATIONS, disclosed rather than absorbed: the design gate was closed by an
orchestrator-applied pass instead of a third team round (Miguel ruled it); the Verify gate was closed
by a narrow single-agent recheck plus orchestrator measurement instead of a full re-gate, because the
session budget would not carry one (Miguel ruled it); and Track ran solo rather than as a team, for
the same reason.

## Gotchas

- A pipeline masks the exit status: `cargo test --workspace | tail -25` reported exit 0 from `tail`
  while cargo's own status was unknown. Re-run capturing the command's own status before believing a
  green.
- Every isolated worktree builds its own `target/`. Thirteen worktrees reached ~72 GB inside
  `.claude/worktrees/` and took the machine to 20 GiB free. Remove each worktree as soon as its patch
  is absorbed, not at the end.
- A probe that runs `unblock init` in the wrong directory leaves a stray workspace that later
  walk-up probes then FIND, silently invalidating them — here at `/private/tmp/.unblock`.
- The knowledge bash-guard fails closed on heredoc quoting: write commit messages to a file with the
  Write tool and use `git commit -F`.
- The measured amplification is one-for-one and uncapped: a 5,000,000-byte method name produces
  5,000,422 bytes on stderr. D48 amplifies nothing — those bytes went to stdout before — but under
  the D31 child-per-client topology it moves them to a sink that PERSISTS.
- The self-test written to pin the drain join observed `drains.len()`, which `join_drains` empties
  with `drain(..)` whether or not it joins — so the mutation reproduced the observation exactly. A
  pin must observe something the mutation CANNOT fabricate; the working version uses a grandchild
  that writes a second after the child exits.

## Glossary

No session-local ids appear in this report or in this run's issue comments: every gate finding was
relayed to Miguel and recorded in the tracker by its plain description plus a `file:line`, and the
mutation names used in the issue comments are the source edits themselves rather than codes.

| id | what it is (in words) | where it lives (file:line / doc § / issue id) |
|----|-----------------------|-----------------------------------------------|
| — | none coined | — |

## Links

- `ub-og3` — the stdout framing-channel defect this run closes.
- `ub-kp7` — a first frame that is neither `initialize` nor `ping` kills the server.
- `ub-b1a` — the unbounded `Debug` rendering of client bytes inside the relocated message.
- `ub-c5o` — `output::emit_report` writes to stdout with no notion of a stdout-owning command.
- `ub-5v5` — no response-size cap; a large frame may be written truncated.
- `ub-gwe` — make the codebase graph a stated first move for structural questions (raised in this
  run, when it emerged the graph tooling had never been used and was never indexed).
- Key files: `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-cli/src/exit.rs`,
  `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-cli/src/cli.rs`,
  `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-cli/tests/mcp_stdout_channel.rs`,
  `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-cli/tests/common/mod.rs`,
  `/Users/ramosmig/Public/WS-Labs/unblock/scripts/checks/d48-stdout-channel-claims.sh`.
- Prior related run-report: `runs/2026-08-06-envelope-id-reject.md` (D47 — the decision whose
  residual clause named this defect).
