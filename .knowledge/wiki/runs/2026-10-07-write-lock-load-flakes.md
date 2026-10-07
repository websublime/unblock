---
name: 2026-10-07-write-lock-load-flakes
description: Making the two D31 write-lock test cells that failed under host load (tracker ub-fh5) load-independent — a stdio gate makes the two-process no-lock collision certain and adds an in-run exclusion witness to the locked half, the fairness cell's 500 ms p99 ceiling is retired to a recorded figure because it measured slow open() calls and a shared tokio runtime rather than the lock, and a paused-clock unit test pins the 25 ms poll the ceiling used to guard.
type: run
date: 2026-10-07
branch: ub-fh5-write-lock-flake
pr: '-'
issues: [ub-fh5]
---

# Run — load-independent D31 write-lock cells

## Context

Issue ub-fh5 tracks two `unblock-storage` test cells that failed under host load. The first is the
no-lock control in `cross_process_write_lock_prevents_id_collision` (`tests/write_lock_two_process.rs`),
which saw no `IdCollision` at all. The second is `write_lock_flock_contention_and_fairness`
(`tests/contention_lab.rs`), which broke its provisional p99 ceiling of 500 ms. The closed duplicate ub-0qm
had folded into ub-fh5 earlier. The run went off main at e702ba6 on branch `ub-fh5-write-lock-flake`. It
was a solo implementation followed by a read-only Verify gate of three reviewer agents (gate protocol,
non-vacuity by mutation, docs and runtime) with the main session coordinating. No design Review gate ran.

## What & why

The issue asked for the remedy to be decided rather than assumed, and required that the locked half keep
its non-vacuity guarantee. A later comment added that the remedy must also handle a load-sensitive latency
assertion. The run read PRD §4 D31 (the cross-process `.write.lock`, including its acceptance of
multi-second p99 stalls and its 25 ms poll), the `tests/contention_lab.rs` row of the storage crate plan,
and the two cells with their `write_lock_race` helper.

Both failures reproduced on purpose under about 2.5x CPU oversubscription (28 busy loops on a 14-core host
at base load 7). The two-process cell failed 14/20 runs and the fairness cell 8/8. The fairness cell
failed harder than reported, because the CLI hit the 30 s lock timeout rather than the p99 ceiling.

The two-process control depended on the two child processes overlapping in time. Under load one child
could finish all 15 mutations before the other had opened the database.

A throwaway probe split one acquire into its syscalls. Under load an uncontended `open()` of
`.write.lock`, with no holder, took p50 80 ms and p99 0.6–1.1 s, while `try_lock` stayed in microseconds.
So the p99 ceiling mostly measured the host. The 30 s timeouts needed the MCP-server burst loop and the CLI
to share one tokio runtime. With the loop on its own runtime thread the probe saw no timeouts in 600
acquisitions, and with the loop in a separate process none in 400.

## Outcome

- `tests/bin/write_lock_race.rs` and `tests/write_lock_two_process.rs` gate each child's first mutation over stdio. A child inside the read-to-insert window prints `GATE=entered` and waits for `GO`, and the test releases nobody until both children have reported. In `locked` mode the first acquire is one `try_lock` on a second handle opened with `lock_timeout_ms = 0`, so the loser reports `GATE=refused`. The locked half asserts one `entered` plus one `refused`, and the control asserts two `entered` and at least one collision.
- `tests/contention_lab.rs` section 3 runs the MCP-server loop on its own runtime on a dedicated thread and stops it before asserting. The p99 is printed but not asserted, and the cell asserts the CLI is never starved to `write_lock_timeout_ms`.
- `src/libsql/lock.rs` gained `contended_acquire_sees_the_release_within_one_poll`, a paused-clock test that pins the 25 ms poll. `tokio`'s `test-util` feature is on for `unblock-storage` dev-dependencies only, and `Cargo.lock` did not change.
- The storage crate plan §5 gained a bullet that records both changes. The `tests/contention_lab.rs` row stays verbatim, following the ub-q1u precedent.
- Under the same load the two-process cell went 0/30, the fairness cell 0/12 (recorded p99 1.2–7.4 s) and the cadence test 0/30.
- Two throwaway mutations were reverted after they turned the cells red. A lock fast path that always succeeds fails the gate assertion, and a 100 ms poll fails the cadence test at a virtual 400 ms.
- The Verify gate returned PASS WITH NOTES from all three reviewers with no must-fix. Its should-fix items were applied. One was a thread that could leak if the CLI loop failed. Another was the poll-cadence tripwire the retired ceiling had provided. The rest were wording that stated an unisolated mechanism as fact or called the new check "the PRD's own".
- Gates on the branch: `cargo insta test --check --workspace` exited 0, clippy `-D warnings` was clean for the workspace and for `unblock-storage --features testkit`, `cargo fmt --check` was clean, and `cargo xtask doc-lint` plus every `scripts/checks` claims gate passed.

## Gotchas

- An uncontended `open()` on this macOS host is slow under load, while the flock calls stay fast. The host runs two EndpointSecurity extensions, which plausibly explains it. Any wall-clock bound around an `open()` inherits that latency.
- Rust's `Stdout` is line-buffered even into a pipe, so `println!` reaches the parent before the child blocks on stdin.
- The parent kept its own `mpsc::Sender` alive at first, so a reader thread that died without reporting would have hung `recv()` instead of failing it. Dropping the parent's sender fixed it.
- A plain `std::thread` is not cancelled when a `#[tokio::test]` unwinds, unlike a `tokio::spawn` task. The stop flag has to be set before any assertion.
- `#[tokio::test(start_paused = true)]` needs tokio's `test-util` feature, which the workspace's `full` feature set does not include.
- A cadence test whose bound reads `POLL_INTERVAL` moves with any change to that constant. The test uses the PRD's literal 25 ms instead.

## Glossary

No session-local ids were used in this run.

## Links

- ub-fh5 — the two load-sensitive D31 cells this run made load-independent.
- ub-0qm — the closed duplicate of the two-process failure.
- ub-q1u — the sibling flake fix whose plan-doc convention this run followed.
- `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-storage/tests/bin/write_lock_race.rs` — the child process and its stdio gate.
- `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-storage/tests/write_lock_two_process.rs` — the gate driver and the gated assertions.
- `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-storage/tests/contention_lab.rs` — section 3 of `write_lock_flock_contention_and_fairness`.
- `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-storage/src/libsql/lock.rs` — the paused-clock cadence test.
- `/Users/ramosmig/Public/WS-Labs/unblock/docs/plans/crates/unblock-storage.md` — the ub-fh5 bullet in §5.
- 2026-10-06-libsql-double-close — the prior storage flake run.
