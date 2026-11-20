---
name: 2026-11-20-ub-c15-session-file-jsonl-guard
description: The session-file JSONL guard on main for v1.1 (tracker ub-c15) — MCP sync import, dry-run import, import_bd and export refuse a target that is the live session database or one of its -wal/-shm/-journal sidecars, and doctor/recover's export scan, evidence copy and regeneration skip or refuse it, never closing a descriptor on it; recorded as D5, NFR-7 and D58 amendments, with an fd witness, a ported holder/observer lock harness on Linux and macOS, a windows-check compile job and two process corrections.
type: run
date: 2026-11-20
branch: ub-c15-sync-refuses-session-db
pr: '482'
issues: [ub-c15]
---

# Run — the session-file JSONL guard on main (ub-c15)

## Context

Issue `ub-c15`: on main (v1.1, unreleased) a JSONL operation inside a live session could open and
close a descriptor on the session database or a sidecar — which drops every POSIX lock the process
holds on that inode — or rename a JSONL file over the database. It is main's twin of `ub-w1j`, fixed
on `release/v1.0.x` in v1.0.3. The run went from 2026-11-19 to 2026-11-20 on branch
`ub-c15-sync-refuses-session-db` in the isolated worktree `../unblock-c15`, base main `3d84a71`, draft
PR #482. Phases and gates (tracker comments): Understand map (477); Decide, Miguel's rulings 1–11
(478); Spec/Plan v1 (479); design Review gate iteration 1 FAIL (480) and plan v2 (481); iteration 2
FAIL, escalated, with Miguel's three rulings (482); plan v3 as plan v2 plus a binding addendum (484);
Implement as one WIP commit `84fef0f` (485); Verify gate PASS with nits N1–N8 (486); this Track
re-cut. Team: one coordinator, three Understand lenses, Spec/Plan planners, four design-Review lenses
per iteration (architect, Rust, QA, PM), two implementers (code, docs), four Verify lenses (code,
Rust, security, QA) and one Track agent.

## What & why

The hazard had to close before v1.1.0 ships: closing any descriptor on a session-file inode drops the
process's SQLite SHARED lock (the `ub-2vp` mechanism), and a database named like the export file
(`issues.jsonl`, the MCP default export path) could be overwritten by an export or by recover's
regeneration. Ruling 1 widened the scope from MCP `sync` to every sync or health JSONL operation in a
live session. The authoritative text is in PRD §4 D5 (the D5 amendment), NFR-7, PRD §4 D58
(amendments (a)–(f)), D29 clause F6's pointer, spine §4.1, the sync, health, engine, cli and storage
crate plans, ci-cd §2, §2.3.3 and §3.3, `RELEASING.md` §1 and `docs/PROCESS.md` §6. The design:
`unblock_sync::SessionFiles`, a fixed array of at most 8 `(dev, ino)` pairs stat-ed at the db path and
its canonical form; every reader opens through `open_verified`, which keeps a raced session file open
and never closes it; health maps the refusal to `ExportScan::SessionFile` and the advisory
`export_is_session_file`; sync owns the `SQLite` sidecar suffixes above L2.

## Outcome

Design gate: iteration 1 FAIL (10 must-fix, 13 should-fix, 10 nit); iteration 2 FAIL with three
must-fixes, closed by Miguel's escalation ruling (comment 482) and the binding addendum (30 accepted,
1 rejected). Verify: PASS, four lenses PASS-WITH-NITS, no blocker or must-fix; all 13 non-vacuity
mutants turned their expected cells red on macOS; CI run 44714924866 green except the then-pending
`run-report-gate`. Track folded nits N1–N8 into the re-cut, whose tree equals the WIP tree plus those
nits outside `.knowledge/` and `.unblock/`:
- `c390291` docs(spec): a JSONL operation never touches the session database (D5, NFR-7, D58 amendments) (ub-c15)
- `a45cbc8` refactor(sync,health,engine,cli,storage): own the SQLite sidecar suffixes in unblock-sync (ub-c15)
- `699a00b` fix(sync,health,engine): refuse or skip a JSONL target that is a session file (ub-c15)
- `240002b` test(engine,cli,storage): port the holder/observer lock harness (ub-c15)
- `6d57665` test(sync,health,engine,cli): session-file refusal matrix and lock-retention cells (ub-c15)
- `6e7ffc0` ci(windows-check): compile-check the shipped package for x86_64-pc-windows-msvc (ub-c15)
- then this report, and the tracker re-export.

At every SHA, every `scripts/checks/*.sh`, `cargo xtask doc-lint` and a 4-core `cargo check
--all-targets --locked` of the touched crates passed. Follow-ups outside `ub-c15`: a Windows ADS
stream name passes the sync allowlist; the pre-existing engine `--no-default-features --all-targets`
failure at `tests/interchange.rs:46,168`; pin cells for `..`, a symlinked directory and an APFS case
variant.

## Gotchas

- The claims rows read files through `git grep`, so a test file that is still untracked is invisible
  to them: a cell-name row only resolves once its test file is committed, in the same commit
  (`scripts/checks/d58-health-full-claims.sh`).
- On macOS, stat-ing a `/dev/fd/N` entry reports devfs's device number (1738479643, not the APFS
  16777231), so an fd witness keyed on `(dev, ino)` never matches; it keys on inode plus birth time
  there (`crates/unblock-sync/tests/common/fd_witness.rs`).
- A relative path once landed an edit in the main checkout instead of the worktree (implementer, C2's
  `path.rs`; the Track agent hit the same trap and the edit tool's stale-hash check refused it). Always
  use absolute worktree paths.
- The model provider's 429 rate limit interrupted agents during the run; work resumed from artifacts.
- Main's "Default Ruleset" (19670603) had drifted from branch-protection wording in the docs: the
  required-check verification read the branch-protection endpoint, and the ruleset carried two
  admin bypass actors (`OrganizationAdmin` and a repository role, `bypass_mode: always`); ruling 3 has
  Miguel remove them, and ci-cd §2.3.3 now reads the ruleset endpoints.
- `ub-c15` sits in the `issues:` list of the `ub-kpe` run-report, so its comments dated 2026-11-19
  fall under that page's k6 scan; its glossary was extended in the same commit as this report.

## Glossary

| id | what it is (in words) | where it lives (file:line / doc § / issue id) |
|----|-----------------------|-----------------------------------------------|
| REL | The `release/v1.0.x` branch at b21c73b, carrying PR #477, the v1.0.3 fix of `ub-w1j` | comments 479–484 |
| A1 | Iteration-1 architect finding 1 (release-cut and required-check drift; part of must-fix 1) | comment 480 |
| A2 | Iteration-1 architect finding 2 (spine lacks the engine-root re-export) | comment 480 |
| A3 | Iteration-1 architect finding 3 (`recovery.rs` still said it never opens the database) | comment 480 |
| A4 | Iteration-1 architect finding 4 | comment 480 |
| A5 | Iteration-1 architect finding 5 | comment 480 |
| A6 | Iteration-1 architect finding 6 | comment 480 |
| A7 | Iteration-1 architect finding 7 | comment 480 |
| A8 | Iteration-1 architect finding 8 | comment 480 |
| A9 | Iteration-1 architect finding 9 | comment 480 |
| A10 | Iteration-1 architect finding 10 (FR-8's refusal set lacked the session's files) | comment 480 |
| P1–P11 | Iteration-1 PM findings 1–11; also tracker priorities P1/P2 and the d58 landing rows P1/P2 | comments 477–481; `scripts/checks/d58-health-full-claims.sh` |
| F1 | Ruling: doctor's skip is the advisory `export_is_session_file`, no severity | comments 480, 481 |
| F2 | Ruling: rows go in the existing d58 script; also "R F2", iteration-1 Rust finding F2 (engine optional and dev-dep edges) | comments 480, 481 |
| F3 | Ruling: `windows-check` is a v1.1.0 release-cut precondition | comments 480, 481 |
| F4 | Ruling: the engine holder mirrors the cli doctor's open order, no new cli seam | comments 480, 481 |
| F6 | PRD D29 clause F6, the 1.0-line doctor-probe fix | PRD §4 D29 |
| F-1 | Iteration-1 QA finding F-1 (a requirement starting with `^` never matches); in comment 484 also iteration-2 Rust finding 1 (macOS witness key) | comments 480, 484 |
| F-2 | Iteration-1 QA finding F-2 (vacuous mutants); iteration-2 Rust finding 2 (M-0's expected list) | comments 480, 484 |
| F-3 | Iteration-1 QA finding F-3; iteration-2 Rust finding 3 (E2's data-loss shape) | comments 480, 484 |
| F-4 | Iteration-1 QA finding F-4 (engine test drops the session before the lock read); iteration-2 Rust finding 4 | comments 480, 484 |
| F-5 | Iteration-1 QA finding F-5 (required-check drift); iteration-2 Rust finding 5 (ruleset endpoint) | comments 480, 484 |
| F-6 | Iteration-1 QA finding F-6 (the own-name data-loss shape); iteration-2 Rust finding 6 | comments 480, 484 |
| F-7 | Iteration-1 QA finding F-7; iteration-2 Rust finding 7 | comments 480, 484 |
| F-8 | Iteration-1 QA finding F-8; iteration-2 Rust finding 8 (row Q319) | comments 480, 484 |
| F-9 | Iteration-1 QA finding F-9; iteration-2 Rust finding 9 (one bullet rejected) | comments 480, 484 |
| F-10 | Iteration-1 QA finding F-10 (holder feature gating) | comment 480 |
| F-11 | Iteration-1 QA finding F-11 | comment 480 |
| F-12 | Iteration-1 QA finding F-12 (cell S9 covers three actions) | comment 480 |
| N-1–N-9 | Iteration-2 QA findings 1–9 | comment 484 |
| MF-1 | Iteration-1 must-fix 1 (ruling F3 and the drift not implemented); in comment 482, iteration-2 must-fix 1 (macOS descriptor witness) | comments 480, 482 |
| MF-2 | Iteration-1 must-fix 2 (one row for five insertions); in comment 482, row Q313 split across lines | comments 480, 482 |
| MF-3 | Iteration-1 must-fix 3 (`^` requirements); in comment 482, the missing D-a/D-b text | comments 480, 482 |
| MF-4 | Iteration-1 must-fix 4: spine lacked the engine-root re-export | comment 480 |
| MF-5 | Iteration-1 must-fix 5: "it never opens the database" survived for `recovery.rs` | comment 480 |
| MF-6 | Iteration-1 must-fix 6: an integration cell called crate-internal `open_import_source` | comment 480 |
| MF-7 | Iteration-1 must-fix 7: the holder's modes were not feature-gated | comment 480 |
| MF-8 | Iteration-1 must-fix 8: mutants that could not turn red | comment 480 |
| MF-9 | Iteration-1 must-fix 9: "kept open" cells could not observe a kept descriptor | comment 480 |
| MF-10 | Iteration-1 must-fix 10: engine cells read the lock after dropping the session | comment 480 |
| SH-1–SH-13 | Iteration-1 should-fixes 1–13 | comment 480 |
| NT-1–NT-10 | Iteration-1 nits 1–10 | comment 480 |
| IT2-01–IT2-31 | Iteration-2 deduplicated findings (29 reviewer, 2 coordinator) | comment 484 |
| R1 | Plan risk 1: workspace-internal API break; also iteration-1 Rust finding 1 | comments 479–481 |
| R2 | Plan risk 2: a session-file export now reports `export_is_session_file`; also iteration-1 Rust finding 2; "CODE R2" is a CODE-file risk item | comments 479–481 |
| R3 | Plan risk 3: residual race windows at REL parity; also iteration-1 Rust finding 3 | comments 479–481 |
| R4 | Plan risk 4: Windows canonical-path identity misses hard links; also iteration-1 Rust finding 4 | comments 479–481 |
| R5 | Plan risk 5: first Windows compile outside `release.yml`; also iteration-1 Rust finding 5 | comments 479–481 |
| R6 | Plan risk 6: a read-only WAL connection assumed to hold SHARED; also iteration-1 Rust finding 6 | comments 479–481 |
| R7 | Plan risk 7: at most one leaked descriptor per raced call | comments 479, 481 |
| R8 | Plan risk 8: up to 8 stats on the tokio worker | comments 479, 481 |
| R9 | Plan risk 9: the fd witness relies on macOS `/dev/fd` | comments 479, 481 |
| R10 | Plan risk 10: once required, the macOS context gates every open PR | comments 481, 484 |
| R13 | Risk item 13 of the CODE section file (spine re-export) | comments 480, 484 |
| R27 | The d44 claims row R27, kept byte-identical | `scripts/checks/d44-create-deps-claims.sh` |
| PC6 | The lp9.25 claims row PC6, kept byte-identical | `scripts/checks/ub-lp9.25-dangling-blocker-claims.sh` |
| O1–O22 | Plan overrides 1–22 reconciling the section files (O4: recover carries no advisories; O7: sync owns the suffixes above L2; O8: export takes the set by value) | plan v3 §1.3; comments 479, 481 |
| B2–B26 | Plan text blocks 2–26 (B19a–B19f its ci-cd/RELEASING parts; B23 PROCESS §6; B24 ci-cd §2.3.3) | plan v3 §2; comment 481 |
| C1–C7b | Track commits: C1 spec, C2 refactor, C3 fix, C4 harness, C5 tests, C6 ci, C7a this report, C7b tracker re-export; C0/C1 are also cli cells (below) | plan v3 §7 |
| S1–S14, S7b | Sync cells (refusal matrix S1–S9, in-crate S7b, S10–S13, moved sidecar cells S14); in SPEC, S1–S8 were also derived decisions | plan v3 §4.2 |
| H1–H6 | Health export-scan and evidence-copy cells | plan v3 §4.2 |
| E1–E4 | Engine in-process cells E1–E3; in SPEC, E1–E4 were escalations | plan v3 §4.2 |
| L0–L7 | Engine sync-target lock cells | plan v3 §4.2 |
| D0–D4 | Engine doctor/recover lock cells | plan v3 §4.2 |
| M0 | Cell `w1j3`: refused import through the real `unblock mcp` keeps SHARED | plan v3 §4.2 |
| M1 | Cell `w1j4`: the MCP default export never replaces the database | plan v3 §4.2 |
| M2 | Cell `c15m0`: symlinked database, target aliases refused, locks kept | plan v3 §4.2 |
| C0 | Cli cell `c15e0`: doctor reports an export that is the database and writes nothing | plan v3 §4.2 |
| M-0 | Mutant: every engine call site passes `SessionFiles::none()` | plan v3 §4.4 |
| M-a–M-k | Mutants M-a, M-b1, M-b2, M-c … M-k, each removing one guard | plan v3 §4.4 |
| D-a | Ruling: PROCESS §6's branch-name rule describes practice (`ub-<id>-<slug>`) | comments 482, 484 |
| D-b | Ruling: ci-cd §2.3.3 verifies required checks through rulesets | comments 482, 484 |
| D-1–D-7 | CODE section-file deviations 1–7 | comments 479, 480 |
| X1–X9 | Superseded TESTS section-file items | comments 479–481 |
| QC1 | `ub-w1j` ruling: amend D5 inline, no new D-id | comment 477 |
| QC3 | `ub-w1j` ruling: "never closes a descriptor on that inode and writes nothing" | comments 454, 477 |
| G2 | `ub-kpe` ruling putting doctor's JSONL scan in scope | comments 477, 478 |
| G3 | The `ub-2vp` in-process lock cell pattern in `tests/health.rs` | comment 477 |
| N1 | Verify nit: sync plan said "the ONE owner" of the suffixes | comment 486 |
| N2 | Verify nit: `open_verified` fstat-error wording | comment 486 |
| N3 | Verify nit: spine said every body passes the set by reference | comment 486 |
| N4 | Verify nit: sync-plan key items lacked `session`; also d58 negative row N4 | comment 486 |
| N5 | Verify nit: session-local codes "REL" and "O4" in durable text | comment 486 |
| N6 | Verify nit: missing blank line in `error.rs` | comment 486 |
| N7 | Verify nit: "canonicalized" should read "lexically normalized" | comment 486 |
| N8 | Verify nit: `--show-output` on the storage-testkit `--test health` step | comment 486 |

## Links

- `ub-c15` — a JSONL operation never touches the live session database (this run); `ub-w1j` — the
  v1.0.x twin; `ub-kpe` — v1.0.3 doctor-probe fix; `ub-2vp` — the close-drop measurement; `ub-s82` —
  MCP default export path; `ub-k4j` — a database named like the write lock.
- PR #482. Key files: `crates/unblock-sync/src/path.rs`, `crates/unblock-health/src/recovery.rs`,
  `crates/unblock-engine/src/preopen.rs`, `crates/unblock-sync/tests/session_files.rs`,
  `crates/unblock-engine/tests/sync_target_lock_retention.rs`, `.github/workflows/ci.yml`,
  `scripts/checks/d58-health-full-claims.sh`, `docs/PRD.md`.
- Prior: [2026-11-19-ub-kpe-v1-0-x-doctor-probe-fix](2026-11-19-ub-kpe-v1-0-x-doctor-probe-fix.md),
  [2026-11-18-ub-2vp-doctor-probe-lock-loss](2026-11-18-ub-2vp-doctor-probe-lock-loss.md).
