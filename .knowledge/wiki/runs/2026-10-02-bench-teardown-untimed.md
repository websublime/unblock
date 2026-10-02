---
name: 2026-10-02-bench-teardown-untimed
description: Taking per-iteration teardown out of the timed region of the create, mint, claim and import benches (tracker ub-lp9.28) — four iter_batched routines dropped their store and tempdir inside the timing, so ~2-3ms of filesystem work dominated sub-millisecond operations and tripped bench-gate on runner I/O noise; iter_batched_ref fixes it, a teardown probe and an injected 1ms slowdown prove the numbers now track the operation, and the create ceiling is re-derived 15 to 5ms.
type: run
date: 2026-10-02
branch: ub-lp9.28-bench-teardown
pr: -
issues: [ub-lp9.28]
---

# Run — bench teardown out of the timed region

## Context

Task `ub-lp9.28` is the first item of the v1.0.2 flake order Miguel confirmed on 2026-10-02. It
went first because it is the only flake that has turned CI red: bench-gate on `main` on
2026-08-04 and 2026-10-01, and on pull request 453 the same day. Solo session. The branch
`ub-lp9.28-bench-teardown` is stacked on pull request 457, because both re-export the tracker.

## What & why

These were read rather than restated here: PRD NFR-1 and D34 (the two-tier perf gate),
`xtask/src/bench_gate.rs` and `xtask/src/bench_compare.rs`, both bench files, and the storage
crate plan's `benches/storage.rs` row.

The issue measured `storage_create/insert` and `engine_create/mint`. Mapping every batched
routine found four, not two, that take their setup by value: those two, `engine_claim/claim`
(which explains the 9x inflation on the pull request 453 run) and `engine_import/10000`. Their
drop of the store, session and tempdir ran inside the timing. `cmp_ready_sort` returns its input,
and criterion drops returned outputs after the timed region, so it was already sound.

## Outcome

### What landed

- `fix(bench)`: the four routines use `iter_batched_ref`, so criterion drops the inputs after
  timing. The bench file headers say why.
- `fix(bench-gate)`: the storage insert, engine mint and engine claim ceilings go from 15 to 5ms.
  The rule is the one the T3.5.1 read re-tightening used: a generous multiple of the local mean plus
  CI headroom. PRD NFR-1 and the storage plan row repeat the number. The import ceiling stays
  5000ms. The four tier-i baseline entries were re-captured on the machine that captured the rest
  of the file.

### Measured (local, aarch64 macOS, 14 cores, load 5 to 8)

- Against `main` with `--baseline main`: insert 3.15 → 0.119ms (−97%), mint 3.92 → 0.709ms,
  claim 5.70 → 0.511ms, import 545 → 473ms.
- A throwaway teardown-only probe read 2.15ms for the storage DB and 2.85ms for an engine
  workspace, which matches the gap.
- A throwaway 1ms sleep in `Storage::create_issue` moved the fixed insert 0.119 → 1.456ms (12x)
  and the fixed mint 0.709 → 2.157ms (3x). On `main`'s benches the same sleep moved insert
  3.15 → 5.13ms (1.6x) and mint 3.92 → 5.73ms (1.5x).
- A full local `cargo xtask bench-gate` passed with 25 enforced ops. `bench-compare` showed the
  four re-captured ops at 0%. It also flagged four read benches at +10 to +17%; those benches are
  unchanged on this branch, and the machine was loaded.
- `cargo fmt --check`, clippy with `-D warnings` on both bench targets and on xtask, `cargo test
  -p xtask`, `doc-lint`, `knowledge-lint`, every `scripts/checks/*.sh` and the knowledge-layer
  invariants all passed.

## Gotchas

- `--baseline <name>` fails for a bench with no saved baseline, and the error goes to stderr, so a
  new probe group silently vanished from a filtered run. Run new probes without `--baseline`.
- criterion keeps old results in `target/criterion`, so a removed probe group still shows as a
  record-only row in a local `bench-gate` until the directory is cleaned. CI starts clean.
- The 5ms ceiling has not yet been seen on a degraded CI runner, where the operation itself may
  inflate. The first CI runs of this pull request are the check.

## Glossary

No session-local ids were used in this run.

## Links

- `ub-lp9.28` — the create benches measured teardown, not the operation.
- Pull request — none yet, opened from this branch; merging is the human gate.
- Key files touched —
  `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-storage/benches/storage.rs`,
  `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-engine/benches/engine.rs`,
  `/Users/ramosmig/Public/WS-Labs/unblock/xtask/src/bench_gate.rs`,
  `/Users/ramosmig/Public/WS-Labs/unblock/xtask/bench-baseline.json`,
  `/Users/ramosmig/Public/WS-Labs/unblock/docs/PRD.md` (NFR-1).
