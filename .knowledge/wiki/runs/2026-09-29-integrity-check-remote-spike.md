---
name: 2026-09-29-integrity-check-remote-spike
description: Measuring whether PRAGMA integrity_check survives the move to remote mode (tracker ub-rjq) — the pragma is addressable on both D51 peers and carries soft corruption unchanged, but self-hosted sqld classes it as a write that a read-only token cannot run, a real Turso Cloud database took a minute to check 250k issues against one second on sqld, and three side findings about user_version, foreign keys and opaque client errors were filed rather than absorbed.
type: run
date: 2026-09-29
branch: ub-rjq-integrity-check-remote-spike
pr: '-'
issues: [ub-rjq, ub-wmr, ub-gaf, ub-iw4]
---

# Run — integrity_check in remote mode, measured on both servers

## Context

Issue ub-rjq records that `Storage::integrity_check` has no remote-mode reading anywhere in the corpus. The
Verify gate on ub-b07 found the gap, and ub-b07 is the cascade that landed decision D51's two product modes.
The issue text asks for the same measurement treatment the ub-w3a spike gave the write path, and it forbids
answering the question in a documentation pass. Miguel chose a measurement spike on that basis.

The run went off main at f521d63 on branch `ub-rjq-integrity-check-remote-spike`. The orchestrator worked
solo, with two read-only scouts in the Understand phase. One mapped the code and the normative text. The
other gathered dated vendor evidence. No normative document changed, so no design or Verify gate ran. The
ub-w3a spike ran the same way.

## What & why

D51 makes remote mode one shared database reached over SQL-over-HTTP through the `turso_serverless` crate.
Its second `Storage` implementation carries no SQL engine of its own. Three obligations rest on
`integrity_check`:

- the v1 doctor body in `unblock-health`
- the NFR-2 assertion at 250k issues
- the NFR-5 failure-replay path

Nobody had checked whether the pragma reaches the server at all, what it returns, or what it costs there.

The run read PRD §4 rows D29, D46 and D51, the NFR-2, NFR-5 and NFR-19 entries, spine §3.4 (the remote
backend), and roadmap §4 open questions 1 and 6. None of them says anything about `integrity_check` in remote
mode. Spine §3.4 describes the second implementation and never mentions the check or the doctor.

The probe was a throwaway crate pinned to `turso_serverless =0.1.3`, so every request went to the crate's
hardcoded `/v3` endpoints. It ran against two targets.

| target | engine | version |
|---|---|---|
| libsql-server in Docker, loopback | libSQL | sqld 0.24.33, SQLite 3.47.0 |
| Turso Cloud scratch database, aws-eu-west-1 | Turso engine, journal mode `mvcc` | SQLite 3.50.4 compatible |

## Outcome

`integrity_check` is addressable on both servers, so reaching the pragma is not the problem. The problems
are the statement form on sqld and the cost on Turso Cloud.

| question | sqld | Turso Cloud |
|---|---|---|
| literal `PRAGMA integrity_check`, read-write token | one `ok` row | one `ok` row |
| soft corruption (512 bytes overwritten on one page) | two rows, identical to local sqlite3 | not injectable |
| hard corruption (page-1 header overwritten) | server refuses to start, client sees connection refused | not injectable |
| median cost at 250k issues | 1.0 s | 60.2 s |
| `quick_check` median at 250k | 189 ms | 10.3 s |
| literal pragma, read-only token | refused, classed as a write | works |
| literal pragma, write-blocked namespace | refused | not reproducible |
| `SELECT * FROM pragma_integrity_check()` | works in every case tested | works in every case tested |
| table-valued result column name | `integrity_check` | `message` |

The sqld fixture held 250,000 issues and 250,000 events in 42,254 pages. The Cloud fixture held 250,000
issues and 30,000 events in 32,530 pages, because Cloud inserts into `events` kept failing at about 60
seconds. Cloud was slower on the smaller fixture. NFR-2 bounds `doctor()` at 15 seconds, and `doctor()`
includes this check. The CLI's `doctor` command also runs the check a second time for its exit code.

Hard corruption never reaches the integrity rows on sqld, because the server will not start. It shows up as
unreachability, which is roadmap open question 6's territory. NFR-5's corrupt fixtures exercise the local
file-state classifier, and that classifier has no remote referent.

The same probe answered part of roadmap open question 1. `PRAGMA user_version` is readable and writable on
sqld, and a stamp written in the same transaction as DDL commits and rolls back with that DDL. Turso Cloud
refuses every write of `user_version`. The two D51 peers therefore run different engines with different
pragma surfaces, and the roadmap's claim that only the URL and the token differ does not hold at that level.

Three side findings were filed rather than carried in ub-rjq.

- ub-wmr — Turso Cloud refuses every `user_version` write, so the D46 ladder cannot stamp a database there
- ub-gaf — `foreign_keys` starts OFF on Turso Cloud, and one write-token client's setting is seen by every other client
- ub-iw4 — the crate's `query()` path reports a sqld permission refusal or a long Cloud write as an undecodable response body

ub-rjq stays open, as its own text requires. The evidence lives in its comment thread. The step left for
the v1.3 lock is to name this pragma in roadmap §4's open questions, then state in spine §3.4 what the second
implementation returns and what the doctor reports in remote mode.

## Gotchas

- Overriding the sqld container's command drops the image defaults, so HTTP binds to 127.0.0.1 inside the container and the database path changes; pass `--http-listen-addr 0.0.0.0:8080` and `--db-path` explicitly.
- A bind-mount directory deleted on the host and recreated under the same path made the next container exit on `mkdir`; a fresh directory worked.
- sqld's admin config endpoint rejects a body that sets `block_writes` without `block_reads`.
- sqld classes the literal integrity, quick-check, user-version and page-count pragmas as writes even in their read form, and on its cursor endpoint a read-only token then fails with no readable reason.
- Turso Cloud refuses `VACUUM`, so dropping every fixture table left the page count at 32,530; reclaiming the space means recreating the database.
- Turso Cloud writes that ran near 60 seconds died, once with `local diskless state diverged from S3`; seed Cloud fixtures in small chunks and check the row count after every failure.
- A credential file requested at a fixed path never appeared on this machine. The tokens reached the session through the chat and a shell startup file instead, and the token pasted into the chat needs rotating.
- The unblock MCP server was not connected at session start, again, because the workspace launches it through `cargo run`; the first debug build took about 29 minutes across two attempts.
- A scout read main's sha from `.git/packed-refs`, which a newer loose ref had superseded; `git log` gives the live value.

## Glossary

No session-local ids were used in this run.

## Links

- ub-rjq — `PRAGMA integrity_check` has no remote-mode reading; this run measured it and left the spec step for the v1.3 lock.
- ub-wmr — `user_version` cannot be written on Turso Cloud, so the D46 migration ladder cannot run against one of the two D51 peers.
- ub-gaf — on Turso Cloud `foreign_keys` starts OFF and one client's setting is seen by every other client.
- ub-iw4 — the client's cursor path hides a server refusal or timeout behind an undecodable response body.
- ub-w3a — the shared-state spike whose measurement treatment this run repeated for the health path.
- ub-b07 — the D51 cascade whose Verify gate found ub-rjq.
- ub-7m4 — the code-graph name defect that kept every code claim in this run on grep and read.
- Prior reports: `.knowledge/wiki/runs/2026-09-22-shared-state-remote-mode-spike.md` and `.knowledge/wiki/runs/2026-09-23-d51-two-product-modes.md`.

Repository files read during the run.

- `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-storage/src/trait_def.rs` — the `integrity_check` contract.
- `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-storage/src/libsql/diagnostics.rs` — the libsql implementation and its `ok` filter.
- `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-storage/src/libsql/mod.rs` — the open-time pragmas and the checkpoint cadence.
- `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-storage/src/libsql/migrations.rs` — the `user_version` reads and writes.
- `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-engine/src/session/lifecycle.rs` — `Session::doctor`.
- `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-health/src/doctor.rs` — `run_doctor`.
- `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-cli/src/commands/doctor.rs` — the second check and the exit rule.
- `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-storage/tests/scale.rs` and `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-engine/tests/scale.rs` — the NFR-2 cells.
- `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-engine/tests/failure_replay.rs` — the NFR-5 cell.

The only repository files this run changed are this report, the wiki index and the tracker's git record,
`.unblock/issues.jsonl`.
