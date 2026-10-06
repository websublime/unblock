---
name: 2026-10-06-libsql-double-close
description: Root-causing the intermittent "bad parameter or other API misuse" in the in-memory parallel first-write stress test (tracker ub-q1u) — libsql 0.9.30 closes every connection handle twice on drop (upstream libsql#2251), so the second close could shut or alias a connection another thread had just opened; a close-once guard statement fixes it for the backend and the raw test helpers, memory_open_lock and the T3.5.1 heavy-load boundary are retired as a misdiagnosis, and a widened-window instrumented libsql proves fails-before and passes-after.
type: run
date: 2026-10-06
branch: ub-q1u-libsql-double-close
pr: '-'
issues: [ub-q1u, ub-lp9.30]
---

# Run — libsql closes every connection handle twice

## Context

Issue ub-q1u tracks an intermittent failure of `libsql::tests::open_in_memory_parallel_first_write_stress`
in `unblock-storage` with `BackendOpaque("SQLite failure: bad parameter or other API misuse")`. The issue
asked to reproduce the failure on purpose, to settle from the libsql API contract whether the production
path or the test harness misused the API, and to fix it at the matching layer. The run went off main at
ae9b9f9 on branch `ub-q1u-libsql-double-close`. It was solo; no reviewer agents ran a design Review or a
Verify gate. ub-lp9.30, the open question about the same failure class reddening the required workspace
job, was linked rather than closed.

## What & why

Earlier runs had attributed this class to a race in SQLite's process-global shared-cache registry. T0.9
added `memory_open_lock` to serialize in-memory opens, and T3.5.1 added a rule that heavy or
high-concurrency work must use `open_local` (the crate plan's OQ-5 and OQ-8 bullets). Reading libsql
0.9.30 showed a different defect. `LibsqlConnection`'s `Drop` calls `local::Connection::disconnect`, and
the inner `local::Connection`'s own `Drop` calls it again. `disconnect` only checks
`Arc::get_mut(drop_ref)`, which passes both times, so `sqlite3_close_v2` runs twice on one handle.
Upstream tracks this as tursodatabase/libsql#2251, still open on main. SQLite requires the handle passed
to `sqlite3_close_v2` not to have been closed already. In SERIALIZED mode, which libsql configures,
SQLite guards its shared-cache list with its own static mutexes, so the registry race had never existed.

Plain loops against the real libsql could not reproduce the failure: 0/300 sequential and 0/600 under
24x oversubscription. The run therefore used a throwaway copy of libsql 0.9.30, patched in with
`cargo --config patch.crates-io` and never committed. One patch traced every `sqlite3_close_v2`. The
other optionally slept between the two closes.

## Outcome

- The trace showed two closes per handle on both `open_in_memory` and `open_local`, the second returning 21 (`SQLITE_MISUSE`).
- Widening only the gap between the closes made the reported test fail 9/20 with the exact error. An open/use/drop churn harness failed on both constructors and hit store aliasing: a committed write was missing from the store that made it, and a migrated store reported `no such table: issues`.
- The fix is `crates/unblock-storage/src/libsql/close_once.rs`. `CloseOnceConnection` pairs each connection with a prepared, never-stepped `SELECT 1` declared after it, so the handle closes once, when the statement drops. `LibsqlStorage::from_database` opens both connections through it. A unit test pins the mechanism, and a mutation that guards an unrelated handle turns it red.
- After the fix the trace showed one close per handle. The widened runs went to 0/100 for the stress test and 0/1920 for the churn harness, with `memory_open_lock` deleted.
- The NFR-16 contract suite gained `contract_concurrent_store_lifecycles_are_isolated`. Against the pre-fix code with the widened libsql it failed 5/5 on both legs, through its isolation assertion. After the fix it passed 0/20 per leg.
- The raw libsql helpers in the `unblock-cli`, `unblock-engine` and `unblock-storage` test suites now open through a local `RawConnection` with the same guard.
- The rustdocs no longer carry the shared-cache diagnosis. The T3.5.1 heavy-load boundary and `seed_corpus`'s file-backed invariant were lifted. The crate plan gained a superseding bullet and kept the old OQ-5 and OQ-8 text verbatim.
- Gates on the branch: `cargo insta test --check --workspace --all-features` exited 0, clippy `-D warnings` was clean for the workspace and for `unblock-storage --features testkit`, doc-lint passed, and the reported test passed 0/1000 sequential and 0/1200 under 24x oversubscription.
- Commits: `fix(storage)`, `test(storage)`, `test`, `docs(plan)`, plus this report with the tracker re-export.

## Gotchas

- Loops alone cannot reproduce a race whose window is a few instructions wide. Widening the window in a patched dependency turned a once-in-tens-of-runs flake into 9/20.
- `cargo --config patch.crates-io.<crate>.path=...` rewrites `Cargo.lock`, so the run backed it up and restored it after each patched build.
- `migrate` takes the D31 `.write.lock` with a zero timeout, so stores sharing one directory fail fast with `DatabaseLocked` when migrated concurrently. The contract suite's temp-file factory now gives each store its own directory (`crates/unblock-storage/tests/contract.rs`).
- A plain `#[tokio::test]` runs on a current-thread runtime, where spawned tasks never overlap on OS threads. The contract runner had to move to `flavor = "multi_thread"` for the lifecycle case to exercise anything.
- The unblock `query` MCP tool discriminates on `kind`, not on the `action` field the AGENTS.md action table suggests.

## Glossary

No session-local ids were used in this run.

## Links

- ub-q1u — the flaky stress test this run root-caused and fixed.
- ub-lp9.30 — the same failure class reddening the required workspace job; it can close once ub-q1u lands.
- https://github.com/tursodatabase/libsql/issues/2251 — the upstream double-close report.
- `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-storage/src/libsql/close_once.rs` — the guard and its unit test.
- `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-storage/src/libsql/mod.rs` — `from_database`, and `open_in_memory` without `memory_open_lock`.
- `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-storage/src/testkit.rs` — `contract_concurrent_store_lifecycles_are_isolated`.
- `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-storage/tests/contract.rs` — the multi-thread runner and the per-store workspace directories.
- `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-cli/tests/common/mod.rs`, `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-engine/tests/common/mod.rs` and `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-storage/tests/migrations.rs` — the guarded raw test connections.
- `/Users/ramosmig/Public/WS-Labs/unblock/docs/plans/crates/unblock-storage.md` — the `close_once.rs` row and the superseding OQ-5 / OQ-8 bullet.
- 2026-09-30-proptest-custom-generator-flake — a prior flake run on a required job.
