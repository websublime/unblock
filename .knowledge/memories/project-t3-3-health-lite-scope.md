---
name: project-t3-3-health-lite-scope
description: T3.3 unblock-health shipped as HEALTH-LITE in v1 (D29, merged PR #398); full taxonomy + --repair + .recovery/ stay v1.1
type: reference
---

**STATUS: T3.3 ☑ DONE & MERGED — [PR #398](https://github.com/websublime/unblock/pull/398) rebase-merged into main at `af7a540`; STATUS.md T3.3 flipped ◐→☑ (commit `dfa4c52`); branch deleted, tree synced+clean. Both gates PASS.** Mints **D29** (F1-F5) + ratified **F5-A** addendum. F1 recover() stays FeatureNotWired (v1.1); F2 reuse DiagnosticKind::Info (contract-neutral, no CONTRACT_HASH bump); F3 run_doctor(&[String], &WorkspacePaths) storage-free+non-async (engine passes rows; no unblock-storage dep); F4 cli doctor routes through wired Session::doctor(), exit from a SEPARATE integrity_check() read (doctor_exit byte-identical); F5 severity map (JsonlConflictMarkers→Unsafe, rest→Recoverable) + faithful OrphanedLockFile. **F5-A (ratified WAL addendum):** TruncatedWal fires on `0 < len < 32` bytes NOT the beads literal `len < 32` — a 0-byte live WAL is valid under unblock's PERSISTENT-OPEN Session (beads closed the DB before classifying); the literal port false-positives truncated_wal on every healthy workspace (reproduced against the real binary). Verify: mutation 5/5 RED, WAL RATIFY, 1124 tests green.

---

**T3.3 (`unblock-health`) scope was a genuine load-bearing drift; Miguel decided 2026-07-08 = HEALTH-LITE in v1.**

The docs contradicted themselves on FR-16's v1 scope:
- **lite-in-v1 side** (authoritative, wins): PRD §12.4 (LOCKED "Health taxonomy (FR-16) — DEFERRED to v1.1"), PRD §13 M3 row ("unblock-health (lite)"), PRD §5 FR-16 tag + v1.1+ row + RK-5, the whole **roadmap** (lines 52/65/90/404/444: `● lite [v1] · ● full [v1.1]`, `.unblock/.recovery/` = v1.1), and the **crate-plan** `docs/plans/crates/unblock-health.md` (v1 modules `level/error/file_state/doctor/paths` vs `[v1.1]` `anomaly/classify/audit/recovery`, `--repair`, evidence). The full db-state/JSONL drift probes depend on the `Storage::diagnostic_probe(s)` **CF-E seams still commented `[v1.1]`** in spine §3.2 — so drift-detection literally can't be built in v1 without un-deferring CF-E.
- **full-@-T3.3 side** (the DRIFT to reconcile DOWN): impl-plan T3.3 (line ~89) + its AC, STATUS T3.3 row (line ~85), spine §4.1 doctor/recover notes (~1424-1431), D27/AF-1 + PRD FR-16 line ~153 trailing sentence ("land ADDITIVELY over the wired doctor() at T3.3" — ambiguous), CLI `doctor.rs` comment.

**v1-LITE scope T3.3 delivers:** build the empty `unblock-health` crate's v1 modules (integrity_check passthrough + file-state classification `FileAnomaly` + `run_doctor` aggregation + `HealthLevel` 4-variant enum but only 3 produced) + wire `Session::doctor()` off the `FeatureNotWired{"health"}` seam + the integrity `DiagnosticKind` mapping. **STAYS v1.1:** the active 4-state taxonomy, `--repair` (WAL checkpoint/reindex), `.unblock/.recovery/` evidence writer, `classify_workspace` drift-detection, the CF-E Storage probe seams.

**Spec-first reconciliation required BEFORE implement** (PRD contradicts itself → violates PRD-as-SSOT): fix impl-plan T3.3 + STATUS T3.3 + spine §4.1 doctor/recover DOWN to lite; clarify the D27/PRD-FR-16 trailing sentence (doctor() SEAM wired at T3.3 with the lite body; full taxonomy stays v1.1). T3.3 is NOT the M3 gate (that's shutdown+perf: T3.2 ✅, T3.4/T3.5 pending).

Open sub-forks for the Decide phase: (a) does `recover()` get wired in lite or stay `FeatureNotWired`? (b) new `DiagnosticKind` variant vs reuse `Info` + MCP CONTRACT_HASH ripple? (c) health→storage dep (add `unblock-storage` for `&dyn Storage`) vs engine-passes-`Vec<String>`-rows design (Cargo.toml currently lacks unblock-storage)? (d) does CLI `doctor` route through `Session::doctor()` or keep the T3.1 doctor-lite composition? See [[project-unblock-rust-rewrite]].
