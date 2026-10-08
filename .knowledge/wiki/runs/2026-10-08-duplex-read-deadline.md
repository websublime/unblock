---
name: 2026-10-08-duplex-read-deadline
description: Bounding the unblock-mcp test client RawDuplexClient::read_response by one 30-second deadline that spans every read (tracker ub-f1k) — each read_line now runs under tokio::time::timeout_at against a per-call deadline, so an uncorrelatable reply or silence fails fast instead of hanging the suite; a paused-clock self-test with a fabricated duplex peer pins the overall deadline and the clipped panic quote, and the M16 mutant that used to hang the run until it was killed at 1500 s now fails C-P1 in 30.03 s.
type: run
date: 2026-10-08
branch: ub-f1k-duplex-read-deadline
pr: '463'
issues: [ub-f1k]
---

# Run — a read deadline for the MCP duplex test harness

## Context

Issue ub-f1k tracks a defect in `RawDuplexClient::read_response` in `crates/unblock-mcp/tests/common/mod.rs`,
the `unblock-mcp` counterpart of ub-46o. The loop awaited `read_line` with no timeout. It returned only on a
matching id and panicked only on EOF, so a reply the harness could not correlate, or silence, hung the
suite. The ub-788 Verify gate filed it after the M16 mutant hung C-P1 until the whole
`cargo test -p unblock-mcp` run was killed at 1500 s. The branch `ub-f1k-duplex-read-deadline` is cut from
main at c1cab94. The work was first done solo on main and then moved to the branch to rejoin the lifecycle,
so the Understand and Decide record was written late (comment 291). A design Review of three read-only
lenses (Architect, Rust, Research) and a Verify gate of three read-only lenses (code-review, QA, rust)
followed, with the orchestrator as coordinator and a single implementer agent for the Review's changes.

## What & why

The ub-f1k acceptance criteria ask for a bounded read whatever arrives, the NFR-14 every-line-is-JSON check
kept, the D47 id-correlating cells in `envelope_id_duplex.rs` either moved to sentinel follow or relying on
the bounded read, M16 turning red within the deadline, and no sleeps on the passing path.

Decided (Option A): one overall deadline per `read_response` call, `READ_RESPONSE_DEADLINE` = 30 s, applied as
`tokio::time::timeout_at(deadline, read_line)` on every read. A per-read timeout was rejected, because a
stream of non-matching lines resets it forever. Moving C-P1 to sentinel follow was rejected, because it
would leave the other nine callers of the harness unbounded. C-P2 already uses sentinel follow. The bound
is 30 s rather than the CLI twin's 20 s because the duplex is in-process and the extra margin covers host
load. The passing path never waits on the timer.

The design Review returned PASS WITH CHANGES. It confirmed the tokio usage: `Timeout` polls the inner read
first, cooperative-budget starvation cannot happen, and no unblock-mcp test used `start_paused` yet. Its
four changes were a permanent harness self-test at parity with the CLI's H10, three prose fixes, a panic
that quotes only this call's lines clipped like the CLI twin, and the roadmap and crate-plan docs.

## Outcome

- `common/mod.rs` gained `READ_RESPONSE_DEADLINE` (30 s) and `QUOTED_LINE_LIMIT` (512). `read_response`
  reads under one deadline. On expiry it panics with `no response correlated to id=N within 30s` and
  quotes only the lines this call read, each clipped to 512 bytes on a char boundary. `raw_client` is now
  `pub`. The NFR-14 check and `seen_lines`/`seen_ids` are unchanged.
- New `tests/raw_duplex_client.rs` holds
  `the_read_deadline_spans_every_read_and_quotes_only_this_call`. It runs on tokio's paused clock with a
  fabricated peer on a `tokio::io::duplex`. The peer answers id 1, writes one over-long id-less frame,
  then writes an id-less frame and a stringified-id frame every 7 s of virtual time. No single read waits
  a full deadline, so only a deadline spanning every read can end the call. tokio `test-util` is a
  dev-dependency only, with unblock-storage as the precedent.
- Prose that described the old unbounded read was rewritten in `common/mod.rs`, `envelope_id_duplex.rs`,
  `receive_cancellation.rs` and `parse_error_id_duplex.rs`.
- Mutants: M16 fails C-P1 in 30.03 s with the deadline panic instead of hanging. Moving the deadline
  inside the loop (the per-read mutant) turns the new cell red at once, with no wall time spent. Both were
  reverted.
- The Verify gate returned PASS WITH NOTES, zero must-fix. Code-review and QA raised the same note: the
  clip assertion did not pin the upper bound, so a `ceil_char_boundary` mutant survived. It was applied by
  asserting the clipped line between `\n  ` and `\n`. Not applied: `waited` has only a lower bound.
- fmt and clippy `-D warnings` were clean and the full `cargo test -p unblock-mcp` passed.
- Docs: the v1.0.2 harness bullet in the roadmap and `roadmap.html` now records ub-f1k as fixed beside
  ub-46o and keeps ub-vcp open, the `roadmap.html` v1.0.2 card lists `mcp` under touches, and the MCP
  crate plan gained a `tests/raw_duplex_client.rs` entry.

## Gotchas

- Process deviation: the fix was first implemented solo on main and only then moved to a branch, so the
  Understand and Decide record and both gates ran after the code existed (ub-f1k comment 291).
- Host load: after an earlier session overloaded the machine, every cargo run here was throttled with
  `nice`, `-j 2` and `--test-threads=2`, and the gate lenses were forbidden to build.
- codebase-memory `trace_path` on `RawDuplexClient::read_response` returned no inbound callers, because
  method-call edges are not resolved for this test-only module. The graph was stale at c1cab94 and grep
  answered instead.
- Adding the tokio `test-util` dev-dependency rebuilds tokio and its dependents once, about 3.5 minutes at
  `-j 2`.

## Glossary

| Id | Meaning |
|---|---|
| M16 | D54 mutation-catalogue mutant that re-wraps the recovered id as a string id in `envelope_id::scan`, `crates/unblock-mcp/src/envelope_id.rs`. |
| C-P1 | `an_undecodable_id_tools_call_answers_and_executes_nothing`, `crates/unblock-mcp/tests/envelope_id_duplex.rs`; correlates by id. |
| C-P2 | `the_arm_is_installed_on_the_real_serve_path`, same file; observed by sentinel follow. |
| H10 | `the_read_deadline_binds_when_no_correlatable_reply_arrives`, `crates/unblock-cli/tests/mcp_stdout_channel.rs`, the CLI twin of this run's self-test (ub-46o). |

## Links

- ub-f1k — the defect this run fixed.
- https://github.com/websublime/unblock/pull/463 — the pull request.
- ub-46o — the CLI `McpClient` counterpart, fixed in 2026-10-08-read-response-deadline.
- ub-vcp — the CLI spawn mutex, still open.
- ub-788 — the D54 run whose Verify gate filed ub-f1k.
- `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-mcp/tests/common/mod.rs` — `read_response` and its deadline.
- `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-mcp/tests/raw_duplex_client.rs` — the self-test.
- 2026-10-02-parse-error-recovered-id — the run that recorded the M16 hang.
- 2026-10-08-read-response-deadline — the ub-46o run this one mirrors.
