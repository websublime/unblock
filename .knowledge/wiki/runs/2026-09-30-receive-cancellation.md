---
name: 2026-09-30-receive-cancellation
description: Making the MCP transport survive rmcp cancelling its receive() future (tracker ub-nbz, and ub-zja under the new decision D53) — out-of-band replies now leave from a parked task and a half-read request is kept across the drop, measured losses at eight requests in flight fell from most of them to zero, both gates closed by the orchestrator under Miguel's authority after repairs that each introduced a defect of their own, and the disk filled to 98 % mid-gate under parallel scratch clones.
type: run
date: 2026-09-30
branch: ub-nbz-receive-cancellation
pr: '451'
issues: [ub-nbz, ub-zja, ub-0il]
---

# Run — receive() cancellation: parked replies and whole requests (D47 8(v), D53)

## Context

Task `ub-nbz` is the residual D47 clause 8(v) disclosed and left open: rmcp 1.7.0 builds a fresh
`receive()` future on every turn of an unbiased `select!` and drops it when another arm wins, so an
out-of-band reply (`-32700`, or a D47 `-32600`) still waiting for the write lock inside `receive()`
was never written. Understand found a second, untracked class on the same loop head: the
unconditional `line_buf.clear()` threw away the head of a well-formed request whenever the drop
landed mid-line. Miguel filed it as `ub-zja` and it rides the same branch. A third issue, `ub-0il`,
was filed at the design Review gate for a claims-script weakness this run did not fix.

The lifecycle ran 2026-09-30 to 2026-10-01, every phase in an isolated clone or worktree.
Understand ran three lenses (architecture seams, Rust async mechanics, an independent second
harness) and a coordinator who wrote a third harness. Decide was Miguel ruling six forks. Spec/Plan
ran three planners and an integrating coordinator. The design Review gate ran three lenses in
round 1, then a single reviser and a two-lens delta-verify. Implement was one implementer applying
the package by script. The Verify gate ran four lenses (code, QA, Rust, security) and a
coordinator, then one fixer and one re-verify lens. Track is this report and the tracker re-export.

Neither gate closed on a clean round. Each round-1 verdict was PASS WITH MUST-FIXES, each repair
introduced a defect a follow-up lens caught, and each time Miguel authorised the orchestrator to
close the remaining items directly rather than run another round. The issue comments record both
closures as such.

The branch is `ub-nbz-receive-cancellation`, seven work commits on `main` 3dcbf03 plus the Track
commits. Pull request 451 carries it.

## What & why

The change was written against these sections, read rather than restated here.

- `docs/PRD.md` §4 rows D47 (clause 8(v), the residual, and clause (10), the CD-7 tiers), D43 (the
  `-32700` arm), D50 (clauses (2), (3), (5) and (9), the pre-handshake gate's inherited residual),
  D38 clause (2) (no-hang on every return path) and NFR-18.
- `docs/plans/crates/unblock-mcp.md`, the `wire.rs` and `pre_handshake.rs` rows and the D47
  framing bullet.
- `docs/plans/implementation-plan.md`, the D47 and D53 entries.
- `docs/PROCESS.md` §3 for the D-id and D-range rules, §5 for same-session drift and escalation
  after two failed rounds.

Understand verified the mechanism in the rmcp 1.7.0 and tokio 1.52.3 sources and measured it on
the shipped binary. The loss needs a handler response holding the write mutex when the reply asks
for it and another response arriving before the lock is granted, so the rate is set by handler
latency, not by the in-flight count alone: fast `ping` handlers with a promptly draining client
reproduced the recorded shape, slow `issue create` handlers lost nothing. It also found the issue
text misattributed the line-buffer clear to the reply case.

Miguel's rulings at Decide: spawn-and-park as the write shape (each out-of-band reply's write
future is spawned and its handle parked on the transport, settled at the top of every `receive()`
loop and in `close()`), with both arms moving together and `answer_error` deleted; the read-side
hazard as its own issue on the same branch; the D50 gate's code unchanged with its disclosure
rewritten; criterion 1 accepted as replicated with the regime stated instead of the one-harness
figures; both a deterministic unit cell and a serve-loop cell as the regression. The ub-nbz close
is an inline amendment of D47 8(v) with no new id. Miguel separately ruled that `ub-zja`'s new
request-integrity behaviour mints D53, which brings the D1..D53 range bump and a new required
claims script.

## Outcome

### What landed

Seven commits on `main` 3dcbf03, each atomic by file ownership.

- `chore(tracker)` 361b8d7 — the tracker re-export, first so `ub-zja` resolves before any row
  pins it.
- `docs(prd)` 180c1d5 — NFR-18 stops calling the pre-initialize id-less notification fatal, a
  drift D50 left behind, resolved in the same session per PROCESS §5.
- `docs(d53)` a17d7e6 — 15 files. D47 8(v) closed in place with sub-clause (e) disclosing the
  `close()` wait, reciprocal notes on D43, D47 (10) and D50, the D53 row, the range bump at every
  PROCESS §3 site, both roadmaps, the crate plan and the implementation plan.
- `fix(mcp)` a724d02 — `ub-nbz`: the parked reply task in `crates/unblock-mcp/src/wire.rs`, eight
  unit cells, and `crates/unblock-mcp/tests/receive_cancellation.rs` with the serve-loop cells.
- `fix(mcp)` 4347cf8 — `ub-zja`: the line buffer is cleared only after a complete line (or after a
  read error, as rmcp does), and end-of-file with a partial line processes it.
- `ci(d53)` 837241d — `scripts/checks/d53-request-integrity-claims.sh`, wired as a required
  `doc-lint` step.
- `ci(claims)` 6c944e6 — the 23 tracker-presence rows across six claims scripts now match the
  issue's own record (`"id":"<id>"`) rather than any record that mentions the id.

The wire bytes did not change, no `ErrorCode` was minted and `CONTRACT_VERSION` stays
`unblock.mcp.v1.10`.

### Before and after

Measured by the implementer on debug binaries built from `main` 3dcbf03 and from the branch, fast
handlers in flight and a promptly draining client, 40 reps per cell unless stated.

| Cell | `main` | branch |
|------|--------|--------|
| out-of-band reply, 8 in flight (four write-mode × arm rows) | 36, 38, 31, 36 lost | 0 |
| out-of-band reply, 4 in flight (same rows) | 19, 19, 24, 13 lost | 0 |
| out-of-band reply, idle | 0 | 0 |
| out-of-band reply, 200 reps at 0 / 4 / 8 in flight | 0 / 73 / 175 lost | 0 / 0 / 0 |
| 20 KB request, 8 pings in flight (`ub-zja`) | 37 lost | 0 |
| 200 KB request, 8 pings in flight | 39 lost | 0 |
| ping split across two writes, 200 reps | 200 lost | 0 |

Nothing was torn in any cell. The Verify QA lens re-measured the `main` side at 8 in flight
independently and got 26 to 37 of 40, which is why the shipped prose states "most of them" with the
measured ranges rather than a bare rate. Fail-before on `main` with the new cells transplanted:
six unit cells failed 5 of 5, the two preservation pins passed 5 of 5, and both serve-loop cells
failed 20 of 20; on the branch the serve-loop cells passed 50 of 50.

### The gate record

- Design Review round 1 (2026-09-30): PASS WITH MUST-FIXES, 18 items. The code held: the bypass
  lens dropped `receive()` at every pending point across 3000 seeds per runtime with no loss or
  tear. What failed were claims, among them a D50 sentence saying a parked task could not save the
  gate's reply (re-run: it does), an "83–97 %" range whose 97 was the original harness's own
  figure, and a mutation loop with no working timeout on macOS.
- Miguel ruled two escalations at that gate: disclose `close()`'s unbounded wait on a stalled peer
  rather than bound it (a bound reintroduces the torn frame the fix removes), and fix the
  record-match weakness of the presence rows in this PR as C5b while filing the matcher weakness as
  `ub-0il`.
- The delta-verify found all 18 items landed and one blocking defect inside the repaired mutation
  loop, plus nine smaller ones. Miguel authorised the orchestrator to close them directly.
- Verify round 1: PASS WITH MUST-FIXES, 6 items — two code and cell items (mutant X5 surviving
  every shipped cell; a re-call after a read error resuming the half-read line where rmcp discards
  it) and four wording items. Security, code and Rust lenses passed.
- The re-verify found all six landed with killing cells, and FAILED on two defects the fix pass
  introduced: R-1 and R-2 (glossary). Miguel authorised the orchestrator to close them directly on
  2026-10-01: the re-wrap moved into C3 with the tip tree unchanged, the five `wire.rs` sentences
  qualified, and D53 clause (3) in `docs/PRD.md` plus the d53 Q7 reason and OK message qualified the
  same way.
- Per-commit replay with fmt and the full gate at the tip (orchestrator, 2026-10-01, fresh clone of
  the branch): every commit is green on fmt, `cargo check`, every claims script, doc-lint,
  knowledge-lint and the knowledge-layer invariants, and on clippy plus the `unblock-mcp` suite at
  the four code and script commits. At the tip every CI-mirroring step exits 0 — 1849 workspace
  tests passed with 0 failed, insta, layering, verify-pins, no-network, the no-default-features
  build, the MCP round trip, deny and audit — except the run-report gate, which was red only
  because this report was not yet committed.

## Gotchas

- `timeout` does not exist on this Mac, so `timeout 900 …; r=$?` recorded 127 for every mutant and
  could never produce the 124 that marks a hang. A perl `fork`/`alarm` wrapper (`$TO`) replaced it;
  the delta-verify then found it mapped a signal-killed child to 0 until it was fixed to report
  128 plus the signal.
- The repaired mutation loop combined the two suite legs with `r=$((r|2))`, so two hangs (124
  each) printed 126, which the loop's own rule reads as "tool missing". On M-Z4, the one catalogue
  mutant that hangs, the loop would have recorded a missing tool.
- The fix pass's per-commit replay ran `cargo check` but not `cargo fmt --check`, so a rustfmt
  re-wrap of a new C3 cell landing in C4 (R-1) passed it. The CI fmt job would have failed C3 on
  its own.
- A must-fix making the read-error path match rmcp (discard the half-read line) made five `wire.rs`
  sentences about D53's invariant unqualified and one false (R-2). A repair introducing its own
  defect is the pattern the D47 and D50 runs also record; in this run it happened at both gates.
- Scratch clones and target directories from parallel lenses filled the volume to 98 % (about
  400 GB) mid-gate. The re-verify's `roundtrip` step failed with `No space left on device` and
  passed on re-run after scratch targets were freed.
- A fixer aborting its own gate run used `pkill -f cargo`, which can kill every other agent's cargo
  process on the machine.
- `cargo build` for the fm-nodefault gate step overwrote `debug/unblock` with a
  `--no-default-features` binary, so the first "after" binary copied for the raw-byte runs was the
  wrong build; it was rebuilt before any cell ran.
- The presence rows (P2, P15 and their siblings) stayed green with the issue's own record removed,
  because other records mention the id. C5b closes that; the separate weakness that a code row is
  satisfied by a `/* */` block or `#[cfg(test)]` text is `ub-0il`.

## Glossary

| id | what it is (in words) | where it lives (file:line / doc § / issue id) |
|----|-----------------------|-----------------------------------------------|
| C1 | the tracker re-export commit that opens the series | commit 361b8d7 on `ub-nbz-receive-cancellation` |
| C1a | the NFR-18 drift-fix commit | commit 180c1d5 |
| C2 | the D53 spec and cascade commit | commit a17d7e6 (was 20d9c8d, then 0c9c4c0, before the fix rounds) |
| C3 | the `ub-nbz` fix commit, the parked reply task | commit a724d02 (was 74fa1a8, then 48ee953) |
| C4 | the `ub-zja` fix commit, the line buffer kept across a drop | commit 4347cf8 (was 08a1463, then fb3df2a) |
| C5 | the commit adding the d53 claims script and its CI step | commit 837241d |
| C5b | the commit pinning the 23 presence rows to the issue's own record | commit 6c944e6 |
| X5 | the Verify QA mutant in which `close()` returns early when the parked reply failed, leaving the write half in place; killed by `a_send_after_close_is_not_connected_even_when_the_parked_reply_failed` | `crates/unblock-mcp/src/wire.rs`, `mod tests` |
| M-Z4 | the package's catalogue mutant that never sets the line-complete flag, the only one that hangs the suite | the Spec/Plan package mutant catalogue; killed by `an_unterminated_line_split_by_a_dropped_receive_is_delivered_at_eof` in `wire.rs` `mod tests` |
| R-1 | the re-verify finding that C3 was not rustfmt-clean because its cell's re-wrap had landed in C4 | `ub-nbz` comments; closed in C3 |
| R-2 | the re-verify finding that five D53 sentences in `wire.rs` were unqualified or false after the read-error repair | `ub-nbz` comments; closed in C4, `docs/PRD.md` D53 clause (3) and the d53 Q7 row |
| Q7 | the d53 claims-script row anchored on the line-complete check in `receive()` | `scripts/checks/d53-request-integrity-claims.sh:132` |
| P2 | the d50 claims-script row pinning `ub-nbz`'s own tracker record, the reply-loss id D50 clause (5) once named as inherited | `scripts/checks/d50-pre-handshake-gate-claims.sh:135` |
| P15 | the d47 claims-script row pinning `ub-nbz`'s tracker record as the id D47 clause 8(v) cites | `scripts/checks/d47-envelope-id-claims.sh:150` |
| `$TO` | the portable perl timeout wrapper used for every timed cargo run, exit 124 on timeout | the Spec/Plan package §8; not committed |

## Links

- `ub-nbz` — an out-of-band transport reply was lost when rmcp cancelled the `receive()` future
  under concurrent traffic.
- `ub-zja` — a well-formed request read across more than one read was destroyed when `receive()`
  was dropped mid-line.
- `ub-0il` — claims-script code rows can be satisfied by a `/* */` block or `#[cfg(test)]` text.
- `ub-5v5` — rmcp's own send tasks aborted at drain timeout, untouched here.
- Pull request — 451 (`websublime/unblock`), opened 2026-10-01; merging is the human gate.
- Key files touched —
  `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-mcp/src/wire.rs`,
  `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-mcp/tests/receive_cancellation.rs`,
  `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-mcp/src/pre_handshake.rs`,
  `/Users/ramosmig/Public/WS-Labs/unblock/scripts/checks/d53-request-integrity-claims.sh`,
  `/Users/ramosmig/Public/WS-Labs/unblock/docs/PRD.md` (the D47, D50 and D53 rows, NFR-18).
- Prior related run-reports — `runs/2026-08-06-envelope-id-reject.md`, the D47 run that recorded
  this residual and its one-harness figures, and `runs/2026-09-21-pre-handshake-frame-gate.md`, the
  D50 gate that inherited it.
