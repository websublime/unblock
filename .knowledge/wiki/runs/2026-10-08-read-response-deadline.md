---
name: 2026-10-08-read-response-deadline
description: Bounding every stdout read of the unblock-cli test client McpClient by its 20-second deadline (tracker ub-46o) — read_response now lends its reader to a one-shot helper thread per line and waits with recv_timeout, so an uncorrelatable reply followed by silence fails in about 21 s instead of hanging the job; a fabricated-child self-test pins it, and three mutants that used to hang or would hang (M16, M13, the D50 id-None gate reply) now fail at the deadline.
type: run
date: 2026-10-08
branch: ub-46o-read-response-deadline
pr: '-'
issues: [ub-46o]
---

# Run — a read deadline that binds in the CLI test harness

## Context

Issue ub-46o tracks a defect in `McpClient::read_response` in `crates/unblock-cli/tests/common/mod.rs`.
The loop checked its 20-second deadline only at the top of each iteration and then blocked in
`BufReader::read_line`. A child that wrote one reply the harness could not correlate and then fell
silent hung the cell until cargo or CI killed it. The ub-788 Verify gate saw this twice more (comment
256). The run went off main at 5dbda17 on branch `ub-46o-read-response-deadline`. It ran the full
lifecycle: an Understand team of three read-only lenses, a Decide step with Miguel, a design Review of
three lenses (PASS WITH MUST-FIXES, all folded into the plan), a single implementer in an isolated
worktree, and a Verify gate of two read-only lenses with the orchestrator running every command.

## What & why

Std gives `ChildStdout` no read timeout. A timeout through `fcntl` or `poll` needs unsafe code, which the
workspace forbids, and `tokio::process` cannot serve a synchronous client that runs inside
`#[tokio::test(flavor = "multi_thread")]` cells. Miguel ruled for a per-line ownership hand-off over a
persistent drain thread. A persistent reader would hold the stdout read end open and break the D50
broken-pipe witness, which calls `close_stdout` before any read and needs rmcp's next write to fail.

With the hand-off, `read_line_by` moves the reader into a short-lived thread that reads exactly one line
and sends the reader, the result and the line back over a std `mpsc` channel. The caller waits with
`recv_timeout` for the time left and puts the reader back before any assertion. The framing guard, the
`seen_lines` push and the id match stay on the caller thread. Between calls the reader is idle, so
`close_stdout`, `capture_stdout`, `write_without_reading` and the retained drains behave as before.

Miguel also ruled that the D47 cells in `envelope_id_frames.rs` keep correlating by id, because a request
awaited by its id returning is the property D47 protects. They now fail at the deadline instead of
hanging. The 13 id-correlated cells in `duplicate_key_frames.rs` could not move to sentinel follow in
any case, because rmcp runs each request on its own task and a later ping can be answered first.

## Outcome

- `common/mod.rs` gained `RESPONSE_DEADLINE`, a `response_deadline` field, `set_response_deadline`, and
  `read_line_by`. The timeout panic starts with `timed out awaiting response id=N after 20s` and quotes
  only the lines that call read (each cut to 512 bytes) plus the child's stderr.
- H10 in `mcp_stdout_channel.rs` drives a fabricated `sh` child that answers id 1, writes one id-less
  error frame, and stays open and silent. With a 1 s deadline the second request must time out.
- Prose describing the old hang was rewritten in four test files. The `mcp_stdout_channel.rs` row of the
  CLI crate plan gained one sentence. The v1.0.2 harness bullet in the roadmap and `roadmap.html` now
  records ub-lp9.28, ub-q1u and ub-fh5 with their close dates, ub-46o as fixed, and ub-f1k and ub-vcp as
  still open, so the bullet keeps describing the release scope.
- Mutants, each in a scratch worktree with the build outside a process-group alarm. M16 hung D-P5 on main
  (alarm exit 124 at 90 s). With the fix, M16 fails D-P1, D-P2 and D-P5, and M13 fails F1 and F3, each in
  about 21 s with the timeout panic. The D50 id-None mutant fails a scratch id-correlated probe in 23 s,
  and the same probe passes on the unmutated tree. Reverting only the hand-off makes H10 hang (124).
- `cargo test -p unblock-cli` passed in 19 s (28 s at baseline). Five runs each of `mcp_lifecycle` and
  `mcp_stdout_channel` passed. fmt, clippy `-D warnings`, all 12 claims scripts, doc-lint, knowledge-lint
  and the knowledge-layer invariants passed.
- Not measured: how close real replies come to the 20 s ceiling under heavy load.

## Gotchas

- `exec cat >/dev/null`, the form H4 uses, closes the child's stdout because the redirect replaces the
  only write end. A silent-but-open child needs `exec cat 9>&1 >/dev/null`.
- A 1 s deadline set before the first request also has to cover the fabricated shell's startup. H10
  completes one request under the default deadline first, so the bounded window holds only the read
  under test.
- knowledge-lint extracts `M13p` as `M13`, so the glossary keys it that way.
- An earlier attempt overloaded the host. A QA lens started 24 busy loops as artificial load, and they
  outlived a crash of the agent process. The rerun dropped artificial load and ran every cargo command
  one at a time under `nice`, with read-only reviewers.
- A shell command that outlived its timeout left the persistent shell unusable. Later commands ran
  through a fresh process.

## Glossary

| Id | Meaning |
|---|---|
| M16 | D54 mutation-catalogue mutant: `crates/unblock-mcp/src/envelope_id.rs:129` re-wraps the recovered id as a string id. |
| M13 | D54 mutant (called M13p in comments): deletes the BOM strip at `crates/unblock-mcp/src/wire.rs:639`. |
| F1 | `f1_a_bom_prefixed_clean_frame_executes`, `crates/unblock-cli/tests/duplicate_key_frames.rs`. |
| F3 | `f3_a_bom_prefixed_duplicate_frame_is_rejected`, same file. |
| H4 | `the_read_response_guard_is_actually_installed`, `crates/unblock-cli/tests/mcp_stdout_channel.rs`. |
| H9 | `wait_for_joins_the_retained_drains_before_the_buffers_are_read`, same file. |
| H10 | `the_read_deadline_binds_when_no_correlatable_reply_arrives`, same file (new). |
| C2 | `c2_mid_write_sigterm_never_leaves_a_partial_batch`, `crates/unblock-cli/tests/shutdown_failure_injection.rs`. |
| C6 | `c6_second_signal_escalation_never_hangs_and_keeps_a_valid_exit_code`, same file. |
| D-P1 | `the_recovered_id_is_answered_over_real_stdio`, `crates/unblock-cli/tests/envelope_id_frames.rs`. |
| D-P2 | `the_connection_recovers_after_an_answer`, same file. |
| D-P5 | `an_escaped_key_duplicate_is_recovered`, same file. |
| E1 | `a_premature_request_before_initialize_is_answered_on_its_own_id`, `crates/unblock-cli/tests/mcp_lifecycle.rs`. |
| E4 | `a_ping_before_initialize_leaves_the_gate_shut_and_the_handshake_still_completes`, same file. |
| E6 | `a_method_initialize_frame_with_untypeable_params_is_rejected_over_stdio`, same file. |

## Links

- ub-46o — the defect this run fixed.
- ub-f1k — the unblock-mcp `RawDuplexClient` counterpart, still open.
- ub-vcp — the spawn mutex for the same harness, still open.
- ub-788 — the D54 run whose Verify gate re-observed the hang.
- `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-cli/tests/common/mod.rs` — `read_response` and `read_line_by`.
- `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-cli/tests/mcp_stdout_channel.rs` — H10.
- 2026-10-02-parse-error-recovered-id — the run that recorded the M16 and M13 hangs.
