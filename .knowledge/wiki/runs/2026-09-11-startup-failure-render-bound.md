---
name: 2026-09-11-startup-failure-render-bound
description: Bounding the mcp startup-failure diagnostic to a structural summary of the rejected first frame (decision D49, tracker ub-b1a) — a 5 MB attacker-controlled Debug blob on a persisted stderr became a 271-byte sentence, with two design-gate failures and two Verify failures that each found prose claiming more than the tree grades, two interrupted agent sessions whose uncommitted work was recovered from their own worktrees, and both gates closed by a Miguel-ruled escalation rather than a third full round.
type: run
date: 2026-09-11
branch: ub-b1a-debug-bytes-bound
pr: '-'
issues: [ub-b1a, ub-o8s, ub-wx3, ub-kp7, ub-og3, ub-8c4]
---

# Run — the startup-failure render bound (D49)

## Context

Task `ub-b1a` is a P1 bug in the v1.0.1 slot. When a client's first frame is neither `initialize`
nor `ping`, rmcp refuses it, and `unblock mcp` rendered that whole frame's Rust `Debug` into its
startup-failure message with no cap — one byte out per byte in, so a 5,000,000-byte method produced
a 5,000,269-byte message. D48, the decision that moved this message off the JSON-RPC framing
channel, put it on stderr, which an MCP host persists. The work ran on branch
`ub-b1a-debug-bytes-bound`, taken off `main` at 4f0ab59 on 2026-09-09. No pull request is open at
the time of writing.

The lifecycle ran from 2026-09-09 to 2026-09-11, every phase in an isolated worktree. Understand ran
three lenses plus a coordinator. Decide went straight to Miguel, who resolved three forks and
ratified four orchestrator rulings. Spec/Plan drafted through an architect and a project-manager,
landed through one writer, and was checked by a coordinator. The design Review gate failed twice
with four lenses each and closed under a Miguel-ruled escalation of one repairer plus one read-only
recheck lens. Implement ran a `rust-engineer` and then an `mcp-developer` with a coordinator. The
Verify gate failed twice and closed under the same escalation shape. Track is this report, the
tracker re-export and the commit that carries them.

Four interruptions are in the record — session limits inside the design gate's first round and
inside Verify round 1, a crash that killed the Implement team before its first writer returned, and
a session stop mid-repair during the Verify escalation.

## What & why

The change was written against these sections, read rather than restated here.

- `docs/PRD.md` §4 rows D43 (the clip helper and `MAX_ECHOED_BYTES`), D47 clause 8(iii), D48
  clauses 3 and 6(ii), which named this defect and ruled the repair belongs at the message's origin
  under its own decision, D35 (what GA freezes) and NFR-14.
- `docs/plans/01-design-spine.md` §2.1, §2.4 (the sanitization chokepoint) and §5b (the CLI
  lifecycle surface).
- The `unblock-mcp` and `unblock-error` crate plans.
- `docs/PROCESS.md` §3 (the D-id and D-range rules) and §5 (escalation after two failed rounds).

The Understand phase widened the defect. All four JSON-RPC frame shapes leak (Request,
Notification, Response and Error), the string envelope id is a second attacker-controlled field,
and a naive 128-byte clip of the rendered string would cut inside rmcp's structural prefix and tell
the operator nothing. Design Review round 1 added a fourth finding, because rmcp's `TransportError`
renders a transport type name over 98 bytes long, so a wildcard clip would hide the I/O reason —
which is why that variant ended up with its own arm. Two adjacent sinks were found and split out
rather than absorbed, the unbounded stdio line read (`ub-o8s`) and rmcp's post-handshake `Debug`
dump of every frame at a single `-v` (`ub-wx3`).

## Outcome

### What landed

The branch carries three commits on `main` 4f0ab59.

- `docs(d49)` d9e637e carries the spec cascade over 15 files — the D49 row, reciprocal notes on D47
  and D48, and the D-range bump at every live site.
- `fix(d49)` ec2b283 adds one description function over every `ServerInitializeError` variant in
  `crates/unblock-mcp/src/error.rs`, which renders the frame kind, the method and the envelope id,
  each clipped and then `Debug`-quoted, and never renders params, result or the error body. The
  same commit adds a dedicated transport arm, a clipping wildcard, a `test-util` constructor for
  all four frame shapes, the marker fold in `bulk_markdown.rs`, and fifteen new cells — fourteen in
  `error.rs` and the marker value cell in `unblock-error`.
- `ci(d49)` dcfd2bc mints the gate script `scripts/checks/d49-startup-failure-render-claims.sh`,
  with line-anchored code rows and the prose-site rows its D48 sibling carries, wires its step into
  the required `doc-lint` job, and states it in the ci-cd specification and the `docs/PROCESS.md`
  §3 list.

Measured live against the built binary, a first frame whose method is 5,000,000 bytes gives 0 bytes
on stdout, 519 bytes on stderr and a 271-byte message, and `tools/list` with id 7 renders the D49
row's own example byte for byte. Ten named mutations died on a named cell across three independent
runs, and about forty invented ones died the same way. Reverting the marker fold is killed by the
gate script's anchored row and by no cell, exactly as that row states.

### Design Review

- Round 1 (2026-09-09) failed with 17 must-fixes, all four lenses failing it independently. The
  transport type-name finding is the one that changed the design.
- Round 2 (2026-09-09) failed with 7 must-fixes, five of them written by the round-1 repair. Under
  `docs/PROCESS.md` §5 this was the second iteration without a pass, so it escalated to Miguel.
- The escalation closed the gate at spec commit d9e637e. One repairer applied the 21 open items,
  and one read-only recheck lens re-measured every number instead of re-reading it. It compiled
  Rust 1.96.0 for the escape-dense case (784 bytes per member, 888 bytes for the whole message),
  measured each arm's fixed text with `printf` and `wc -c`, and reproduced with `clippy-driver` the
  lint that forced the inline format spelling. It found no new false sentence.

### Implement

The team launched on 2026-09-09 and died with its session before the first writer returned. Its
worktree held 607 uncommitted lines, which the orchestrator committed inside that same worktree as
a provisional `fix(d49)`. A fresh `rust-engineer` then fast-forwarded onto it, audited the
recovered code against every clause of the D49 row and every cell the T3.12 acceptance criteria
name, and amended the result.

### Verify

- Round 1 (2026-09-10) failed with 6 must-fixes, every one a sentence and none behaviour: an
  escape-offset counterfactual that three lenses ran and disproved; a claim that the truncation
  marker has one spelling in the tree, which a grep falsifies; a dead-code justification for the
  public test-only constructor; about fifteen stale `file:line` anchors; a sweep sentence whose one
  live hit is this change's own new cell; and three bare-identifier presence rows in the gate
  script that a leftover doc comment satisfies.
- Round 2 (2026-09-10) failed with 5 must-fixes, again all sentences, two of them written by the
  round-1 repair itself. It escalated to Miguel as the second Verify round without a pass.
- The escalation closed the gate at tip dcfd2bc, after one repairer applied the five must-fixes and
  thirteen should-fixes. The recheck lens read rmcp's `service/server.rs` and `transport.rs`
  itself, counted the marker literals and `bulk_markdown.rs`'s test declarations at both revisions,
  found the D48 script's `ub-kp7` row itself, and reported no new false sentence. Its own probe ran
  clippy pedantic, the tests of the three touched crates and doc-lint at exit 0, with all fourteen
  D49 cells in `unblock-mcp` reported as run and none ignored.
- Both closures are weaker than a gate pass, and the issue comments record them as such.

## Gotchas

- A session crash purges the session scratchpad. Every brief, verdict and ruling file vanished, and
  the two implementers worked from the PRD row and the plan blocks alone, so a ruling that matters
  must reach the spec or the issue comments before a team launches.
- The crashed implementer's 607 uncommitted lines came back by committing them inside its own
  worktree, never the shared tree, and fast-forwarding a fresh agent onto that commit.
- About fifteen `file:line` anchors in the spec pointed at the wrong lines once the implementation
  moved the code, and `error.rs` went from 414 lines to 1104. No lint class catches that, and it is
  tracked as `ub-8c4`.
- Each repair round writes new sentences, and new sentences carry new claims. Five of the design
  gate's seven round-2 must-fixes and two of Verify round 2's five were authored by the previous
  repair. What converged both gates was a recheck lens that re-measured the numbers rather than
  re-reading them.
- The gate implementer refused a brief instruction, to drop the word OPEN from the D48 script's
  residual row, because the D49 row forbids that edit in its own words. The brief was the defect.
- rustc's dead-code lint exempts `_`-prefixed names, so "a private `__` constructor would be dead
  code" is false. A controlled rename proved it.
- A bare `downcast_ref::<ServerInitializeError>()` on the snafu source returns `None`, because
  snafu boxes the source; the cell has to downcast the boxed form.
- `format!("{:?}", x)` fires `clippy::uninlined_format_args` under pedantic, so a normative row has
  to spell `format!("{x:?}")`.
- The `unblock-storage` timing cells, the contention-lab p99 stall and the zero-timeout
  cross-holder cell, flake under load and reproduce on `main` at 4f0ab59, so they are not a D49
  regression.
- In zsh a word beginning with `=` is equals-expansion, so `echo ====` aborts the whole compound
  command with "=== not found".

## Glossary

No session-local ids were used in this run.

## Links

- `ub-b1a` — this task; the startup-failure message embedded an unbounded `Debug` rendering of the
  rejected first frame.
- `ub-o8s` — the stdio transport reads a frame line with no length bound, and a cap there needs its
  own decision.
- `ub-wx3` — rmcp `Debug`-dumps every post-handshake client frame to stderr at a single `-v`.
- `ub-kp7` — a first frame that is neither `initialize` nor `ping` kills the MCP server, which is
  the path that reaches this message today.
- `ub-og3` — the D48 channel move named this defect as one of its residuals.
- `ub-8c4` — `file:line` anchors in normative docs rot silently when the anchored code moves.
- Pull request — none open at the time of writing.
- Key files touched — `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-mcp/src/error.rs`,
  `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-mcp/src/tools/bulk_markdown.rs`,
  `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-error/src/sanitize.rs`,
  `/Users/ramosmig/Public/WS-Labs/unblock/scripts/checks/d49-startup-failure-render-claims.sh`,
  `/Users/ramosmig/Public/WS-Labs/unblock/docs/PRD.md` (the D49 row).
- Prior related run-report — `runs/2026-08-07-mcp-stdout-framing-channel.md`, the D48 channel move
  that left this defect open.
