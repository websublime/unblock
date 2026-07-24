---
name: reference-bench-gate-cmp-ready-sort-250k-flaky
description: bench-gate NFR-1 tier-ii ceiling `cmp_ready_sort/250000` (500ms, D34) is FLAKY on GitHub runners — thin ~18% margin; flaked once on a docs-only PR then passed on re-run. For a non-perf change, re-run the failed bench-gate before assuming a regression.
type: gotcha
---

The `bench-gate` CI job (NFR-1 hybrid perf gate, D34; ceilings pinned in `xtask/src/bench_gate.rs`) has one **flaky-prone** ceiling: **`cmp_ready_sort/250000` = 500.0 ms**. It runs the closest to its ceiling of any bench (the 250k sort is the heaviest bench, unlike the read benches which have ~4× headroom), so runner noise on a busy GitHub Actions runner can push it over.

Evidence (2026-07-16, PR [#419](https://github.com/websublime/unblock/pull/419) — a **docs + 1 dist-config-line** change that touches NO runtime code): bench-gate FAILED with `cmp_ready_sort/250000 mean 589.561 ms > ceiling 500.0 ms` (~18% over) while every other bench passed with large headroom. `gh run rerun <id> --failed` → the SAME bench passed under 500ms. So it was a runner-noise flake, not a regression.

**Rule:** when `bench-gate` fails on a change that cannot affect perf (docs, config, CI-yaml, non-hot-path), **re-run the failed job first** (`gh run rerun <run-id> --repo websublime/unblock --failed`) — do NOT widen the ceiling inside an unrelated PR (a re-baseline is a D34-governed change needing SF-2 number-equality replication across PRD/ci-cd/roadmap + Miguel's sign-off + a justified commit body). If it fails REPEATEDLY on a non-perf change, that is a pre-existing ceiling-calibration issue to fix as a separate D34 fast-follow (candidates: widen the 250k ceiling with headroom, or demote `cmp_ready_sort/250000` to record-only like `storage_count`/`storage_search`). See [[project-t3-5-perf-budgets-scope]] and [[project-aarch64-windows-no-axoupdater-bit-rc1-release]].
