---
name: 2026-11-19-ub-kpe-v1-0-x-doctor-probe-fix
description: The v1.0.3 maintenance patch on release/v1.0.x (tracker ub-kpe and its sibling ub-w1j) — unblock doctor now reads the database header before any connection opens and never closes a descriptor on a session file, and MCP sync refuses an import or export whose target is the session database or a sidecar; three design rounds, a ported POSIX-lock harness with lock cells on Linux and macOS, a release helper allowed on the maintenance branch, and the precise-builds backport.
type: run
date: 2026-11-19
branch: ub-kpe-v1-0-3-record
pr: '478'
issues: [ub-kpe, ub-w1j, ub-2vp, ub-c15, ub-y72, ub-k4j]
---

# Run — v1.0.3: the v1.0.x doctor probe keeps the session's locks (ub-kpe, ub-w1j)

## Context

Issue `ub-kpe`: the shipped v1.0.0–v1.0.2 `unblock doctor` reads the 16-byte header of `unblock.db`
through a raw descriptor while the same process holds libsql connections; on Unix that descriptor's
close drops the process's SQLite SHARED lock (measured under `ub-2vp`). Sibling issue `ub-w1j`: an MCP
`sync` import or export whose target is the session database or one of its sidecars does the same, or
renames a JSONL over the live database. The run went from 2026-11-18 to 2026-11-19 on branch
`ub-kpe-v1-0-x-doctor-probe-fix`, cut from tag v1.0.2 (`842cdff`), in the isolated worktree
`../unblock-ub-kpe`. Base: the maintenance branch `release/v1.0.x`, published at `842cdff`. PR: #477
(draft, PR-B, comment 453). Phases and gates: Understand (three read-only lenses, comment 444); Decide
(rulings K1–K6, comment 445); Spec/Plan v1 and rulings QA1–QA6, QB1–QB4 (comment 446); design Review
gate iteration 1 FAIL with rulings G1–G5 (comment 447); iteration 2 PASS-WITH-MUST-FIX with rulings
H1–H3 (comment 448); plan v3 and rulings QC1–QC3 (comment 449; ub-w1j scope, comment 450); iteration 3
PASS-WITH-MUST-FIX, design locked as plan v4 (comment 451); Implement as WIP commits (comment 452);
Verify iteration 1 PASS-WITH-MUST-FIX (comment 455) and the Linux non-vacuity run (comment 456); then
Verify iteration 2 and this Track step. Team: one coordinator, three Understand lenses, three Spec/Plan
planners, three design-Review lenses per iteration (architect, rust-engineer, release/CI/process), one
implementer, three Verify lenses (code-review, QA, rust plus `/security-review`) and one Track agent.

## What & why

The v1.0.x line still ships the probe `ub-2vp` proved harmful: without SHARED, another process's last
close can checkpoint and unlink the `-wal` and `-shm` doctor still has open. Main deleted the probe at
v1.1, so the fix had to land on a maintenance line cut from v1.0.2. Ruling K1 picked the narrow
pre-open read: the CLI resolves the workspace, reads the header, then opens the database and hands the
result to `Session::doctor` and `run_doctor`, so every report doctor produces stays byte-identical.
The design reviews widened the scope to every place a session file could be opened and closed inside
a live session: doctor's JSONL conflict scan when the JSONL path is the database (G2) or a sidecar
(H2), a file swapped between probe and open (G3), and the MCP `sync` targets (H1, split out as
`ub-w1j`). Authoritative text on the branch: PRD §4 D29 (the new clause F6 addendum), the D5 row
amendment and NFR-7 (`ub-w1j`), the D17 amendment (precise-builds); spine `run_doctor` and
`Session::doctor` signatures; the health, engine, sync, config and cli crate plans; ci-cd §2 and §3.3;
`RELEASING.md` §6.

## Outcome

Implemented as 17 WIP commits (`4c39217` … `af2b8f7`); the Track step re-cut them into the atomic
Conventional commits below, whose final tree is identical to `af2b8f7` outside `.knowledge/` and
`.unblock/`. Every code commit passes `cargo check --workspace --all-targets` on its own. Final list
(PB1 … PB12 in plan v4 §6.2 order, then the two Track commits; this report is the last one, `docs(wiki)`):
- `092241d` docs(d29): add clause F6 — doctor reads the database header before any connection opens (ub-kpe)
- `0e47d19` docs(d5): a sync target is never the session database or a sidecar (ub-w1j)
- `a53ac91` docs(ci-cd): specify branch CI, the macOS lock-retention and Windows compile jobs, the dated advisory ignores and releases from release/v1.0.x (ub-kpe, ub-w1j)
- `f3e5a57` build(dist): build only the shipped package (precise-builds) — D17 amendment
- `938102c` docs(d17): mark the precise-builds amendment as backported to the 1.0 line (ub-kpe)
- `652c0b4` build(deps): bump quinn-udp to 0.5.16 and rustix to 1.1.6 (RUSTSEC-2026-0538, RUSTSEC-2026-0486) (ub-kpe)
- `24d7314` build(deps): ignore RUSTSEC-2026-0462 and RUSTSEC-2026-0543 on the 1.0 line, dated (ub-kpe)
- `2323171` feat(config): open storage over an already-resolved context (open_resolved_with_storage) (ub-kpe)
- `31ab0d2` fix(sync,engine): refuse a sync target that is the session database or a sidecar (ub-w1j)
- `ccfbedd` fix(health,engine,cli): read the database header before any connection opens (D29 F6, ub-kpe)
- `84d7741` test(engine,cli): doctor keeps the SQLite SHARED lock on Linux and macOS (ub-kpe)
- `c344577` test(engine,cli): a refused sync target keeps the session's locks (ub-w1j)
- `e66fe94` ci: run CI on release/v1.0.x, the lock-retention cells on Linux and macOS and a Windows compile check (ub-kpe, ub-w1j)
- `d1df98f` feat(xtask): release from the v1.0.x maintenance branch release/v1.0.x (ub-kpe)
- `f88b979` docs(releasing): document maintenance releases from release/v1.0.x (ub-kpe)
- `39a342c` chore(tracker): land the ub-kpe/ub-w1j run-report and re-export the record on the 1.0 line (ub-kpe, ub-w1j)
- `487b9c3` chore(tracker): re-export the record for ub-kpe Track
- `docs(wiki): run-report for the ub-kpe v1.0.x doctor-probe and sync-target fix` (this report)

Content:
- PB3: `git cherry-pick -x b595840` (`precise-builds = true`), so `dist` builds only `unblock-cli` and
  the storage `testkit` stays out of the shipped binary (comment 437).
- PB5, PB5b: quinn-udp 0.5.16 and rustix 1.1.6 lock bumps; dated ignores for RUSTSEC-2026-0462 and
  RUSTSEC-2026-0543 in `deny.toml` and the `audit` job.
- PB6: config `open_resolved_with_storage`, so the workspace is resolved once and config warnings
  print once.
- PB1, PB1b, PB2, PB4, PB11: the branch spec and doc edits (D29 F6, D5, NFR-7, D17, ci-cd, spine,
  crate plans, `RELEASING.md` §6).
- PB7b (`ub-w1j`): `unblock_sync::SessionFiles` and `PathReject::SessionDatabase`; a refused target
  maps to the existing `PATH_TRAVERSAL` with no contract change (QC2).
- PB7 (`ub-kpe`): `DbHeader`, `DbHeaderProbe`, engine `DoctorPreflight`, cli
  `open_session_for_doctor`. A descriptor that turns out to be a session file after a race is kept
  open until the process exits, never closed.
- PB8, PB8b: the ported `posix_locks.rs`, the `doctor_lock_holder` test bin, lock cells kpe0–kpe7,
  kpe1m, kpe6m, kpe_p and w1j0–w1j4.
- PB9: `ci.yml` push trigger for `release/v1.0.x`, the `exclusivity-macos` job, a named Linux step in
  `test (workspace)` and the `windows-check` job.
- PB10: `cargo xtask release` accepts `main` or exactly `release/v1.0.x`, and only 1.0.z versions on
  the latter; the five existing release snapshots are unchanged.

**Local evidence (macOS, 4 cores, comment 452).** fmt, clippy on every touched crate (including engine
`--no-default-features --lib --bins`), check-layering, doc-lint, verify-pins, knowledge-lint, the 12
`scripts/checks`, the targeted tests, `cargo deny`, `cargo audit` with the ignores, and
`cargo tree -p unblock-cli -e features -i unblock-storage` (no testkit) all pass. The byte-identity
matrix over 21 fixtures shows 0 unruled differences; the one ruled difference is that v1.0.2 reported
`jsonl_conflict_markers` when the database's own bytes held a marker line and v1.0.3 does not. Every
mutation (i)–(vii) turns its cell red. On unpatched v1.0.2 on macOS (a throwaway worktree) kpe1, kpe4,
kpe5, kpe6 and w1j0–w1j4 fail as designed, while the controls kpe0, kpe1m, kpe2, kpe3 and kpe6m pass.

**Non-vacuity on Linux (comment 456).** Docker `rust:1.96.0`, `--cpus 4`, tmpfs temp dirs,
`--nocapture`. Unpatched v1.0.2 with only the new test files copied in: kpe1, kpe4i and kpe6i fail with
"the holder lost SQLite's SHARED lock", kpe5 with "SHARED lost"; kpe1's after-doctor evidence line reads
`sh=None proc_locks=none o=LOCKED`, so `F_GETLK` and `/proc/locks` agree that no lock is held. w1j0,
w1j2 and w1j3 got `JSONL_PARSE_ERROR` instead of `PATH_TRAVERSAL`, w1j1 got `SYNC-OK`, and w1j4's
path-less default export wrote over the database. kpe0, kpe1m, kpe2, kpe3, kpe6m and kpe_p pass as
designed. The branch at `8412e00` (source mounted read-only) passes all 16 cells, kpe7 included. With
the macOS red run, the fix cells are shown failing on unpatched v1.0.2 on both OSes.

**Verify iteration 1 (PR #477 at `8412e00`, comment 455): PASS-WITH-MUST-FIX.** CI run 44484209193: all
20 jobs green, including `test (workspace)` with the Linux lock-cell step, `exclusivity (macOS, D29
clause F6)`, `windows-check`, `audit` and `deny`. The dist `plan` run 44484209200 succeeded, but its log
cannot show the `--package` flag, so the rc.1 build log stays the proof for comment 437. Confirmed: no
sync, health, engine, cli or mcp path opens and closes a descriptor on the database or a sidecar inside
a live session; the MCP diff is empty and `CONTRACT_HASH` is unchanged. Must-fixes: amend D29 F3's
"`sync` is an unused edge in lite" (`docs/PRD.md:119`); run the skipped Docker Linux red run (done,
above); open the plan's residual issue (`ub-k4j`). SHOULDs applied: `SessionFiles::of_database` derives
the sidecar names from the canonicalized database path, with a test; the health docs narrow the "never
closes" claim to the session-safe entry points; `crates/unblock-health.md:57` is brought up to date; the
named lock steps in `ci.yml` run with `--show-output`; the macOS job also runs the sync `session_files`
matrix and the health unit cells. Fixes: WIP `64aea90` and `b4523c6`.

**Verify iteration 2 (PR #477 at `374f6ea`, comment 457): PASS.** All three lenses passed and every
iteration-1 must-fix is resolved. CI run 44487505250: all 20 jobs green, including `test (workspace)`,
`exclusivity (macOS, D29 clause F6)`, `windows-check`, `audit`, `deny` and `bench-gate`; the release
`plan` run 44487505254 succeeded. `SessionFiles::of_database` also covers the sidecars at the
symlink-resolved database path, with the new cell w1j5 green on both OSes; each OS log prints 59
`UB2VP-MEASURE` lines. Declined NITs: the FIFO stat-then-open window, `capture(&Path)` as locked, and the
rc.1 build log as the `--package` proof.

**Leftovers closed before Track (comment 458).** The QA S1 mutation, on a throwaway copy that was then
reverted (log `local://ub-kpe-w1j-mutation.log`): with the sync refusal and the keep-open path
disabled and the refusal-code checks relaxed to log lines, each lock or database assertion fails on its
own: w1j0 and w1j3 lose SHARED (observer `LOCKED`), w1j2 and w1j5 lose the `-shm` DMS lock, w1j1 and
w1j4 report that the database was replaced. With only the symlink-resolution change removed, only w1j5
fails. Code NIT N1: on a timeout the cli observer panic carries the observer's stdout and stderr
(`af2b8f7`).

**Track: done.** The atomic re-cut above, the tracker re-export and this report; the Linux red/green
evidence is comment 456. Not run locally: the Windows target (CI `windows-check`) and `dist plan`
(local dist 0.30.2 is older than the pinned 0.32.0; the rc.1 build log is the proof).

**Residuals.** `ub-k4j` (a database named like the `.write.lock` file); the FIFO stat-then-open window
in the header probe (recorded in the `open_verified` doc, needs `libc`); `ub-c15` (main carries the
`ub-w1j` hazard, including the symlinked-database sidecar case).

**Pending (outside this PR):** Miguel's merge of PR #477; PR-M1 on main; rc.1, the final v1.0.3 and
the `v1.0.2 → v1.0.3` update smoke; PR-M2.

## Gotchas

- **The branch tracker export lacked the issues.** The v1.0.2 snapshot of `.unblock/issues.jsonl`
  had no `ub-kpe` or `ub-2vp`, so a branch run-report failed knowledge-lint k4; the branch report
  commit re-exports the tracker (G1, comment 447).
- **Config key name.** The key is `jsonl_export_filename`, not `jsonl_filename`; the first kpe4 shape
  passed vacuously until it asserted that the JSONL path equals the database path (comment 448).
- **libtest prints without a newline.** `test kpe_observer_entry ... ` precedes the observer's output
  on the same line, so the observer token is searched anywhere in stdout and the run passes `-q`
  (comment 447).
- **A FIFO would hang the probe.** An `O_RDONLY` open of a FIFO blocks, so the probe stats first and
  never opens a non-regular file (comment 447).
- **A `mints no D-id` sentence trips a claims family.** Main's `ub-lp9.25` claims script rejects that
  wording, so the main roadmap uses v1.0.2's wording (comment 451).
- **The lock bump removed crates.** quinn-udp 0.5.16 moves to the locked `windows-sys` 0.61.2, so
  `cargo update --precise` also removed `windows-sys` 0.60 and its target crates; quinn-udp is not
  compiled into any build (comment 452).
- **No PR job built Windows.** The non-unix arms would first break at rc.1, hence the new
  `windows-check` job (comment 451).
- **`fuzz-smoke` runs only on the default branch's schedule.** It is dispatched with
  `--ref release/v1.0.x` before rc.1, and the known `content_hash` crash (`ub-y72`) is re-dispatched at
  most twice (G5, H3).
- **Docker Linux was skipped at first.** The Docker daemon was down during Implement, so the Linux
  red run on unpatched v1.0.2 was missing until Verify iteration 1 flagged it; it ran once Docker
  Desktop was started (comments 452, 455, 456). CI only proves the patched tree green.
- **The local `dist` is older than the pin.** dist 0.30.2 refuses the pinned 0.32.0, and the dist
  `plan` log cannot show the `--package` flag, so the rc.1 build log is the only proof that `dist`
  builds `unblock-cli` alone (comments 452, 455).
- **A symlinked database moves its sidecars.** SQLite puts `-wal`/`-shm` next to the symlink's target,
  so `SessionFiles` built from the path as given missed them; it now also covers the symlink-resolved
  path (comments 455, 457; ub-c15 comment 454).

## Glossary

| id | what it is (in words) | where it lives (file:line / doc § / issue id) |
|----|-----------------------|-----------------------------------------------|
| K1 | Ruling: read the 16-byte header before any libsql connection opens (the narrow pre-open read) | ub-kpe comment 445 |
| K1b | Ruling: a 0-byte `unblock.db` is not a header mismatch | ub-kpe comment 445 |
| K2 | Ruling: record the decision as an inline D29 clause F6 addendum on both lines, no new D-id | ub-kpe comment 445 |
| K3 | Ruling: the record (roadmap, run-report, tracker export) lives on main; the branch carries only the amendments its code needs | ub-kpe comment 445 |
| K4 / K4b | Rulings: release v1.0.3 from `release/v1.0.x`, rc.1 first, port `exclusivity-macos`, smoke before v1.1.0, no merge-back, backport precise-builds | ub-kpe comment 445 |
| K5 | Ruling: engine lock cells with a holder, `F_GETLK` and a fresh observer, on Linux and macOS | ub-kpe comment 445 |
| K6 | Ruling: fix main ci-cd §4's `GITHUB_TOKEN` drift | ub-kpe comment 445 |
| QA1–QA6 | Spec/Plan v1 questions ruled by Miguel (run-report copy, advisories, branch docs, main record, ruleset, axoupdater residual) | ub-kpe comment 446 |
| QB1–QB4 | Plan v1 defaults confirmed by Miguel | ub-kpe comment 446 |
| R1 | The ruled way PR-B passes `run-report-gate`: a branch copy of this report with its own frontmatter and the same body | ub-kpe comment 446 (QA1) |
| R2 | A rejected alternative to R1 | plan v4 §11.1 |
| G1–G5 | Design Review 1 rulings: branch tracker re-export, the JSONL-is-the-db scan, the probe/open swap race, test strength, fuzz-smoke dispatch | ub-kpe comment 447 |
| H1–H3 | Design Review 2 rulings: fix the MCP sync targets (`ub-w1j`), extend doctor's check to sidecars, triage the fuzz crash (`ub-y72`) | ub-kpe comment 448 |
| QC1–QC3 | Plan v3 rulings: D5 amendment plus NFR-7, no contract bump, reworded `ub-w1j` acceptance | ub-kpe comment 449 |
| A1 | Design Review 3 architect must-fix 1: absolute "opens no descriptor" wording becomes "never closes a descriptor on it" | ub-kpe comment 451 |
| A2 | In ub-kpe's comments: Design Review 3 architect must-fix 2: the holder's health and sync arms are cfg-gated by engine feature (merged with M1). In ub-2vp's comments: A second `lock-probe` holder spawned after the unlink in fd6 | ub-kpe comment 451; crates/unblock-storage/tests/exclusive_window.rs:1321 |
| A3 | Design Review 3 architect must-fix 3: one `SessionFiles` type, since `unblock-health` already depends on `unblock-sync` | ub-kpe comment 451 |
| M1 | In ub-kpe's comments: Design Review 3 rust must-fix 1 (the holder arms cfg-gated by engine feature), merged with A2. In ub-2vp's comments: Outcome "loss confirmed": fd1 drops SHARED and O gets LOCKED on every leg, fd5 shows the unlink, controls and guards hold, v1.0.2 Runs 0/1/2/T agree — the outcome of the ub-2vp run | ub-kpe comment 451; plan v2 §8.2; ub-2vp comments 432, 434, 439 |
| M2 | In ub-kpe's comments: Design Review 3 rust must-fix 2: spell out the non-unix arms and add a Windows compile check. In ub-2vp's comments: Outcome "not reproduced": SHARED retained after the probe on every leg | ub-kpe comment 451; plan v2 §8.2; ub-2vp comments 432, 434, 439 |
| M3 | In ub-kpe's comments: Design Review 3 rust must-fix 3: one `pub(crate)` call returning the header and the session set, plus a renamed-probe cell. In ub-2vp's comments: Outcome "environment split": M3a CI Linux vs CI macOS disagree, M3b local macOS vs CI macos-15 disagree | ub-kpe comment 451; plan v2 §8.2; ub-2vp comments 432, 434, 439 |
| F3 | Existing D29 clause: `run_doctor` is pure, non-async and storage-free | PRD §4 D29 |
| F5 | Existing D29 clause: the file-state severity map and the `DatabaseNotSqlite` header check | PRD §4 D29 |
| F5-A | The D29 F5 addendum, precedent for an inline addendum without a new D-id | PRD §4 D29 |
| F6 | New D29 clause: the header is read before any connection opens, and doctor never closes a descriptor on a session file | PRD §4 D29 (branch) |
| (i-narrow) / (i-full) / (ii) / (iii) | Understand's fix options: header only pre-open, the whole classifier pre-open, the header through the existing connection, drop the probe | ub-kpe comment 444 |
| (i)–(vii) | The mutation checks, each a throwaway code edit that must turn its cell red | plan v4 §2.5; ub-kpe comment 452 |
| kpe0 | Control: a holder without doctor keeps SHARED and the observer gets BUSY | `crates/unblock-engine/tests/doctor_lock_retention.rs` |
| kpe1 | The fix cell: SHARED is kept after the real `Session::doctor`; fails on v1.0.2 | same file |
| kpe1m | Positive control: the holder runs the v1.0.2 probe verbatim, SHARED is lost and the observer gets LOCKED | same file |
| kpe2 | A 0-byte database is not a header mismatch (K1b) | same file |
| kpe3 | A non-SQLite file still fails the open as on v1.0.2 | same file |
| kpe4 | The JSONL path is the database (config or hard link) and SHARED is kept (G2) | same file |
| kpe5 | The cli cell: the production `open_session_for_doctor` order keeps SHARED | `crates/unblock-cli/tests/doctor_lock_retention_cli.rs` |
| kpe6 / kpe6m | The sidecar cell (H2) and its `-shm` positive control | `crates/unblock-engine/tests/doctor_lock_retention.rs` |
| kpe7 | A raced open of a session file keeps the descriptor open | same file |
| kpe_p | Linux `/proc/locks` parser canned-lines cell | same file |
| w1j5 | `ub-w1j` cell: a target that links to the sidecars at the symlink-resolved database path is refused and the `-shm` DMS lock is kept | `crates/unblock-engine/tests/sync_target_lock_retention.rs` |
| S1 | Verify iteration 2 QA leftover: the local w1j mutation run | ub-kpe comments 457, 458 |
| N1 | Verify iteration 2 code NIT: the cli observer timeout keeps its output | ub-kpe comments 457, 458 |
| w1j0–w1j4 | `ub-w1j` lock cells: engine holder import, export and sidecar import (w1j0–w1j2), real MCP server import and default export (w1j3, w1j4) | `crates/unblock-engine/tests/sync_target_lock_retention.rs`; `crates/unblock-cli/tests/sync_target_lock_retention_cli.rs` |
| PB1–PB12, PB1b, PB5b, PB7b, PB8b | PR-B's planned commits (PB12 = this report and the tracker re-export) | plan v4 §6.2 |
| PR-B / PR-M1 / PR-M2 | The fix PR into `release/v1.0.x`; the main record PR before rc.1; the main release-record PR after the smoke | plan v4 §6.1 |
| plan v1–v4 | The Spec/Plan document across its review iterations (session-local, not in the repo) | ub-kpe comments 446–451 |
| kpe4i / kpe6i | The in-process halves of kpe4 and kpe6 named in their failure messages | `crates/unblock-engine/tests/doctor_lock_retention.rs` |
| B | In ub-2vp's comments: A one-shot ordinary process that opens, reads and closes (holder mode `read-and-close`); its last close may unlink the sidecars | ub-2vp: exclusive_window_holder.rs:77 |
| O | In ub-2vp's comments: The behavioural observer: a fresh process running `locking_mode=EXCLUSIVE` + `BEGIN EXCLUSIVE` with `busy_timeout=0`, printing `LOCKED` or `BUSY <code>` (mode `observe-exclusive`) | ub-2vp: exclusive_window_holder.rs:76 |
| W | In ub-2vp's comments: The window child running the real exclusive window with the fd7 hooks (mode `window`, testkit only) | ub-2vp: exclusive_window_holder.rs:79 |
| SH | In ub-2vp's comments: The direct observation of who holds SQLite's SHARED range (0x40000002, 510 bytes): `F_GETLK` on both OSes plus `/proc/locks` on Linux | ub-2vp: crates/unblock-storage/tests/common/posix_locks.rs:506 (`shared_range`) |
| P1 / P2 | In ub-2vp's comments: fd7's observation points in the real window: P1 after `lock_exclusively` and before the evidence check, P2 after it | ub-2vp: crates/unblock-storage/src/libsql/health.rs:153, :162 |
| f1 / f5 | In ub-2vp's comments: Existing cells: f1 a WAL reader elsewhere makes `locking_mode=EXCLUSIVE` BUSY (O's detector); f5 `BEGIN EXCLUSIVE` without `locking_mode` misses a reader | ub-2vp: exclusive_window.rs:218, :378 |
| fd0 | In ub-2vp's comments: Baseline: the holder's store holds SHARED and O gets BUSY 5 | ub-2vp: exclusive_window.rs:1084 |
| fd1 | In ub-2vp's comments: The reproduction: the verbatim v1.0.x probe's close drops SHARED | ub-2vp: exclusive_window.rs:1101 |
| fd2 / fd3 | In ub-2vp's comments: Controls: a no-op, and a SQLite-owned close, keep SHARED | ub-2vp: exclusive_window.rs:1169, :1186 |
| fd4 | In ub-2vp's comments: = G1: main's storage pre-open primitives keep SHARED | ub-2vp: exclusive_window.rs:1203 |
| fd5 / fd5c | In ub-2vp's comments: Impact: after the probe another process's last close unlinks the live sidecars; fd5c the control without the probe | ub-2vp: exclusive_window.rs:1245, :1272 |
| fd6 | In ub-2vp's comments: A commit after the unlink is lost while another process holds the new files | ub-2vp: exclusive_window.rs:1321 |
| fd6e / fd6k | In ub-2vp's comments: WIP-only variants of fd6: clean exit (row survives) and SIGKILL (row lost); dropped after M1 | ub-2vp: WIP commit 58057c0; comments 434, 435 |
| fd7 | In ub-2vp's comments: The repair window's sidecar re-read keeps its exclusive lock and puts no lock on `-wal`/`-shm` | ub-2vp: exclusive_window.rs:1403 |
| fd8 | In ub-2vp's comments: The split-brain cell, deliberately not added | ub-2vp: comment 431 OQ3 |
| fdp | In ub-2vp's comments: The storage `/proc/locks` parser canned-lines cell | ub-2vp: exclusive_window.rs:1507 |
| G1 / G3 / G3p | In ub-2vp's comments: Guards for main: G1 = fd4; G3 a refused `gather_preopen` keeps the session's SHARED lock (Linux); G3p the engine parser copy's canned-lines cell | ub-2vp: crates/unblock-engine/tests/health.rs:860, :913 |
| M4 | In ub-2vp's comments: Outcome "HEAD and the real v1.0.2 binary disagree" | ub-2vp: plan v2 §8.2; ub-2vp comments 432, 434, 439 |
| M5 | In ub-2vp's comments: Outcome "instrument invalid": a control or detector red, or the witnesses disagree; hit once in Verify iteration 3 (fd3 baseline, root cause H4) | ub-2vp: plan v2 §8.2; ub-2vp comments 432, 434, 439 |
| M6 | In ub-2vp's comments: Outcome "guard red on main": fd4, fd7 or G3 red with valid instruments; stop and escalate | ub-2vp: plan v2 §8.2; ub-2vp comments 432, 434, 439 |
| M7 | In ub-2vp's comments: Outcome "reproduction/impact split": fd1 and fd5 (or v1.0.2 Runs 1 and 2) contradict | ub-2vp: plan v2 §8.2; ub-2vp comments 432, 434, 439 |
| OQ1–OQ8 | In ub-2vp's comments: Spec/Plan open questions (cell location, guarding pre-open, impact cells, repair window, holding v1.0.2 doctor, which binary, driver location, tracker metadata) | ub-2vp: comment 431 |
| MR1–MR3 | In ub-2vp's comments: Design-Review gate-1 rulings, all option A: two fd7 hooks; the measurement branch point and Verify interplay; a dist build for timing only | ub-2vp: comment 432 |
| GF-1–GF-28 | In ub-2vp's comments: The 28 deduplicated gate-1 findings | ub-2vp: plan v2 §0; comment 432 |
| Arch / Rust / Research #n | In ub-2vp's comments: Gate-1 finding n of the architect, rust-engineer and research-analyst lenses | ub-2vp: plan v2 header; comment 432 |
| (a)–(d) | In ub-2vp's comments: Miguel's Decide rulings: (a) reproduce on HEAD and on real v1.0.2; (b) v1.0.3 or not (deferred, then B1a); (c) permanent CI cell; (d) `nix` accepted | ub-2vp: comments 430, 435 |
| B1a / B1b | In ub-2vp's comments: The ruling (b) options: ship v1.0.3 / let v1.1 carry it | ub-2vp: plan v2 §8.2; comment 435 |
| (a)–(d) of the M5 fix | In ub-2vp's comments: One-buffer read, dedup, bounded re-read of both witnesses (N=10), extra evidence fields | ub-2vp: comment 439 |
| V1 / V1b / V2 | In ub-2vp's comments: Verify rulings: V1 match the device in the parsers; V1b fall back to inode-only when no line carries the stat device (btrfs); V2 fix testkit reaching shipped binaries with `precise-builds` | ub-2vp: comments 436, 438 |
| H1–H4 | In ub-2vp's comments: Hypotheses for the M5 flake: H1 merged range, H2 holder timing, H3 inode/device/pid selection defect (all refuted), H4 seq_file restart skips (root cause, confirmed by the stress run) | ub-2vp: comment 439 |
| S1–S12 | In ub-2vp's comments: The wave-1 spec edits of plan v2 §3.1 (PRD criterion (n), ci-cd rows and §2.1, crate plans, spine setters, (6)(c) citations) | ub-2vp: plan v2 §3.1 |
| C1, C1b, C2, C2b, C3, C4, Cdist, WIP | In ub-2vp's comments: The planned commits: spec, hooks, storage cells, engine cells, measured note, Track; Cdist the D17 fix; WIP the measurement commits | ub-2vp: plan v2 §7; the ub-2vp run-report's Outcome |
| Q278–Q303 | In ub-2vp's comments: d58 claims rows added by ub-2vp: Q278–Q282 spec text, Q283–Q291 storage cells, Q292–Q293 engine cells, Q294 fd6, Q295–Q303 measured note | ub-2vp: scripts/checks/d58-health-full-claims.sh |
| Q242 / Q224–Q226 / Q240–Q241 | In ub-2vp's comments: Existing d58 rows kept intact: the `--test exclusive_window` run line, the f1/f5/f4 cell names, the macOS job key and image | ub-2vp: scripts/checks/d58-health-full-claims.sh; plan v2 §2.4 |
| P20 | In ub-2vp's comments: The ub-lp9.25 row pinning the engine `--test dangling` run line | ub-2vp: scripts/checks/ub-lp9.25-dangling-blocker-claims.sh |
| Run 0 / 1 / 2 / T | In ub-2vp's comments: v1.0.2 procedure steps: Run 0 the `unblock mcp` control; Run 1 the held-doctor observation; Run 2 the impact hold; Run T unpinned timing runs | ub-2vp: plan v2 §5; comment 434 |
| `UB2VP-MEASURE` / `UB2VP-STRESS` | In ub-2vp's comments: Prefixes of the cells' evidence lines and of the draft-only stress line | ub-2vp: exclusive_window.rs:996; comment 440 |
| `PROBE-V1` | In ub-2vp's comments: The holder command that runs the verbatim v1.0.x probe | ub-2vp: exclusive_window_holder.rs:244 |
| iteration 1–4 | In ub-2vp's comments: Verify loop passes, iterations 3–4 authorised by Miguel | ub-2vp: comments 436, 438, 439, 440 |
| attempt 1–20 | In ub-2vp's comments: The twenty sequential re-runs of the `storage testkit` job on `e875482` (attempt 1 = job 134871125096, attempt 20 = job 134901450921) | ub-2vp: CI run 44324250725; comment 441 |
| commits 1–7 | In ub-2vp's comments: Comment 441's numbering of the re-cut history: the five code commits and the two Track commits listed in the ub-2vp run-report's Outcome | ub-2vp: comment 441 |
| plan v2 | In ub-2vp's comments: The Spec/Plan document after gate 1 (session-local, not in the repo) | ub-2vp: comment 431 + 432 |
| A4 | Finding 4 of the ub-c15 iteration-1 design-Review architect lens (verified and folded into plan v2) (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 comment 480 |
| A5 | Finding 5 of the ub-c15 iteration-1 design-Review architect lens (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 comment 480 |
| A6 | Finding 6 of the ub-c15 iteration-1 design-Review architect lens, confirmed against the code (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 comment 480 |
| A7 | Finding 7 of the ub-c15 iteration-1 design-Review architect lens (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 comment 480 |
| A8 | Finding 8 of the ub-c15 iteration-1 design-Review architect lens (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 comment 480 |
| A9 | Finding 9 of the ub-c15 iteration-1 design-Review architect lens (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 comment 480 |
| A10 | Finding 10 of the ub-c15 iteration-1 design-Review architect lens: FR-8's refusal set lacked the session's files (became nit NT-3) (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 comment 480 |
| F1 | Miguel's ruling on a ub-c15 plan-v1 fork: doctor's skip is the advisory `export_is_session_file` (no severity, no planned action) (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 comments 480, 481 |
| F2 | Miguel's ruling on a ub-c15 plan-v1 fork: the claims rows go in the existing d58 script (Q198 moved, one row per D58 insertion, cell-name rows); "R F2" is also iteration-1 Rust finding F2 (the engine's optional and dev-dep edges) (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 comments 480, 481 |
| F4 | Miguel's ruling on a ub-c15 plan-v1 fork: the engine lock holder mirrors the cli doctor's open order, plus separate cli end-to-end cells, no new cli seam (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 comments 480, 481 |
| F-1 | ub-c15 iteration-1 QA finding F-1 (a claims-row requirement starting with `^` can never match; became MF-3); in comment 484, "Rust F-1" is iteration-2 Rust finding 1, the macOS `/dev/fd` witness key (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 comments 480, 484 |
| F-2 | ub-c15 iteration-1 QA finding F-2 (mutants that could not turn red; part of MF-8); in comment 484, iteration-2 Rust finding 2 (the M-0 expected list) (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 comments 480, 484 |
| F-3 | ub-c15 iteration-1 QA finding F-3; in comment 484, iteration-2 Rust finding 3 (the E2 data-loss shape) (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 comments 480, 484 |
| F-4 | ub-c15 iteration-1 QA finding F-4 (the engine test `Workspace` drops the session before the lock read); in comment 484, iteration-2 Rust finding 4 (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 comments 480, 484 |
| F-5 | ub-c15 iteration-1 QA finding F-5 (the release-cut and required-check drift; part of MF-1); in comment 484, iteration-2 Rust finding 5 (the ruleset endpoint) (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 comments 480, 484 |
| F-6 | ub-c15 iteration-1 QA finding F-6 (a database holding a conflict-marker line is regenerated over: the data-loss shape); in comment 484, iteration-2 Rust finding 6 (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 comments 480, 484 |
| F-7 | ub-c15 iteration-1 QA finding F-7; in comment 484, iteration-2 Rust finding 7 (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 comments 480, 484 |
| F-8 | ub-c15 iteration-1 QA finding F-8; in comment 484, iteration-2 Rust finding 8 (the Q319 row) (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 comments 480, 484 |
| F-9 | ub-c15 iteration-1 QA finding F-9; in comment 484, iteration-2 Rust finding 9 (fd-witness and cell details; one bullet rejected) (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 comments 480, 484 |
| F-10 | ub-c15 iteration-1 QA finding F-10: the holder's feature gating under `--no-default-features` (part of MF-7) (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 comment 480 |
| F-11 | ub-c15 iteration-1 QA finding F-11 (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 comment 480 |
| F-12 | ub-c15 iteration-1 QA finding F-12: cell S9 covers three actions, not four, and a cell prefix collided (became NT-7) (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 comment 480 |
| M0 | ub-c15 lock cell `w1j3`: a refused import through the real `unblock mcp` keeps the SHARED lock (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 plan §4.2; `crates/unblock-cli/tests/sync_target_lock_retention_cli.rs` |
| M-0 | ub-c15 non-vacuity mutant: every engine call site passes `SessionFiles::none()` instead of the session's files (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 plan §4.4 |
| MF-1 | ub-c15 iteration-1 must-fix 1: ruling F3 and the required-check drift were not implemented; in comment 482's iteration-2 numbering, the macOS descriptor witness keyed on devfs's device number (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 comments 480, 482 |
| MF-2 | ub-c15 iteration-1 must-fix 2: one claims row covered five D58 insertions; in comment 482's numbering, claims row Q313's anchor and requirement on different lines (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 comments 480, 482 |
| MF-3 | ub-c15 iteration-1 must-fix 3: claims requirements starting with `^` never match; in comment 482's numbering, the missing branch-name and ruleset-verification text (D-a/D-b) (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 comments 480, 482 |
| MF-4 | ub-c15 iteration-1 must-fix 4: the spine lacked the engine-root sidecar re-export the cli consumes (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 comment 480 |
| MF-5 | ub-c15 iteration-1 must-fix 5: "it never opens the database" survived for `recovery.rs` (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 comment 480 |
| MF-6 | ub-c15 iteration-1 must-fix 6: an integration cell called the crate-internal `open_import_source`, so it moved in-crate (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 comment 480 |
| MF-7 | ub-c15 iteration-1 must-fix 7: the lock holder's modes were not feature-gated, so the feature matrix would go red (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 comment 480 |
| MF-8 | ub-c15 iteration-1 must-fix 8: several mutants targeted code that did not exist or could not turn red (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 comment 480 |
| MF-9 | ub-c15 iteration-1 must-fix 9: the "kept open" cells could not observe a kept descriptor (no fd witness) (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 comment 480 |
| MF-10 | ub-c15 iteration-1 must-fix 10: the in-process engine cells read the lock after dropping the session (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 comment 480 |
| R3 | ub-c15 plan risk 3: residual race windows kept at `release/v1.0.x` parity; also iteration-1 Rust finding 3 (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 comments 479–481 |
| R4 | ub-c15 plan risk 4: on Windows the canonical-path identity misses a hard link, harmless there; also iteration-1 Rust finding 4 (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 comments 479–481 |
| R5 | ub-c15 plan risk 5: the first `windows-check` run is v1.1's first Windows compile outside `release.yml`; also iteration-1 Rust finding 5 (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 comments 479–481 |
| R6 | ub-c15 plan risk 6: a read-only WAL connection is assumed to hold SHARED; also iteration-1 Rust finding 6 (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 comments 479–481 |
| R7 | ub-c15 plan risk 7: at most one leaked descriptor per raced call (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 comments 479, 481 |
| R8 | ub-c15 plan risk 8: up to 8 stats and one canonicalize on the tokio worker (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 comments 479, 481 |
| R9 | ub-c15 plan risk 9: the fd witness relies on macOS `/dev/fd`, guarded by a positive control (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 comments 479, 481, 484 |
| R10 | ub-c15 plan risk 10: once the ruleset requires the macOS context, every open PR waits on it (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 comments 481, 484 |
| R13 | Risk item 13 of the ub-c15 CODE plan section file: the spine lacked the engine-root re-export (source of MF-4) (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | ub-c15 comments 480, 484 |
| R27 | The d44 claims row R27, whose `tests/contract.rs` comment had to stay byte-identical through ub-c15's churn (as used in ub-c15 tracker comments 477–484; see the ub-c15 run-report) | `scripts/checks/d44-create-deps-claims.sh` |

## Links

- `ub-kpe` — v1.0.3: stop the v1.0.x doctor probe from dropping the process's POSIX locks (this run);
  `ub-w1j` — v1.0.3: refuse an MCP sync target that is the session database or a sidecar (same PR);
  `ub-c15` — main's twin of `ub-w1j` (v1.1); `ub-y72` — the recurring `content_hash` fuzz crash;
  `ub-2vp` — the measurement of the v1.0.x probe's lock loss; `ub-k4j` — a database named like the
  `.write.lock` file (residual found at Verify).
- Measurement that motivated it: `ub-2vp` and its run-report on main,
  `2026-11-18-ub-2vp-doctor-probe-lock-loss`.
- PR: https://github.com/websublime/unblock/pull/477 (draft).
- Key files: `docs/PRD.md` (D5, D17, D29, NFR-7), `docs/plans/01-design-spine.md`,
  `docs/plans/ci-cd-and-distribution.md`, `docs/plans/crates/unblock-health.md`, `RELEASING.md`,
  `dist-workspace.toml`, `deny.toml`, `.github/workflows/ci.yml`,
  `crates/unblock-health/src/file_state.rs`, `crates/unblock-health/src/doctor.rs`,
  `crates/unblock-engine/src/session/lifecycle.rs`, `crates/unblock-sync/src/path.rs`,
  `crates/unblock-config/src/context.rs`, `crates/unblock-cli/src/commands/doctor.rs`,
  `crates/unblock-engine/tests/doctor_lock_retention.rs`,
  `crates/unblock-engine/tests/common/posix_locks.rs`, `xtask/src/release.rs`.
